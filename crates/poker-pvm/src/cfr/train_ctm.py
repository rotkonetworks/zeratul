#!/usr/bin/env python3
"""CTM-MoE training for poker leaf evaluation.

Uses existing MCCFR blueprints as teachers. Generates training positions
from self-play, queries blueprints for ground truth action distributions,
trains a Mixture of Experts model to generalize beyond the blueprint's
finite info set table.

Usage:
    python train_ctm.py --blueprints strategy_100m.bin strategy_5m_dcfr.bin ...
    python train_ctm.py --generate-data  # step 1: generate training positions
    python train_ctm.py --train          # step 2: train MoE
"""

import struct
import random
import math
import json
import argparse
from pathlib import Path
from dataclasses import dataclass
from typing import List, Dict, Tuple, Optional

# ---------------------------------------------------------------------------
# Blueprint reader (matches Rust strategy.rs format)
# ---------------------------------------------------------------------------

def load_blueprint(path: str) -> Dict[bytes, List[float]]:
    """Load a MCCFR strategy file. Returns {info_set_key: [action_probs]}."""
    data = Path(path).read_bytes()
    pos = 0
    count = struct.unpack_from('<I', data, pos)[0]
    pos += 4

    strategy = {}
    for _ in range(count):
        if pos >= len(data): break
        key_len = data[pos]; pos += 1
        key = data[pos:pos+key_len]; pos += key_len
        num_actions = data[pos]; pos += 1
        probs = []
        for _ in range(num_actions):
            fixed = struct.unpack_from('<H', data, pos)[0]; pos += 2
            probs.append(fixed / 65535.0)
        strategy[key] = probs

    print(f"loaded {path}: {len(strategy)} info sets")
    return strategy

# ---------------------------------------------------------------------------
# Card / board utilities
# ---------------------------------------------------------------------------

RANKS = list(range(13))  # 0=2, 1=3, ..., 12=A
SUITS = list(range(4))   # 0=clubs, 1=diamonds, 2=hearts, 3=spades
DECK = list(range(52))   # card = rank + suit * 13

def card_rank(c): return c % 13
def card_suit(c): return c // 13
def card_str(c):
    r = "23456789TJQKA"[card_rank(c)]
    s = "cdhs"[card_suit(c)]
    return f"{r}{s}"

# ---------------------------------------------------------------------------
# Feature extraction (mirrors ctm.rs exactly)
# ---------------------------------------------------------------------------

NUM_FEATURES = 27
NUM_ACTIONS = 6  # fold, check, call, bet_small, bet_big, allin
NUM_EXPERTS = 6

def extract_board_texture(community: List[int]) -> List[float]:
    f = [0.0] * 13
    if not community: return f

    ranks = [0] * 13
    suits = [0] * 4
    max_rank = 0

    for c in community:
        r, s = card_rank(c), card_suit(c)
        ranks[r] += 1
        suits[s] += 1
        max_rank = max(max_rank, r)

    f[0] = max_rank / 12.0
    pairs = sum(1 for c in ranks if c == 2)
    trips = sum(1 for c in ranks if c >= 3)
    f[1] = pairs / 2.0
    f[2] = float(trips)
    f[3] = 1.0 if pairs > 0 or trips > 0 else 0.0

    max_suit = max(suits)
    f[4] = 1.0 if max_suit >= len(community) else 0.0
    f[5] = 1.0 if max_suit == len(community) - 1 else 0.0
    f[6] = 1.0 if max_suit <= 1 and len(community) >= 3 else 0.0

    straight_possible = False
    for start in range(9):
        count = sum(1 for r in range(start, start+5) if ranks[r] > 0)
        if count >= 3: straight_possible = True; break
    if ranks[12] > 0:
        low = sum(1 for r in range(4) if ranks[r] > 0)
        if low >= 2: straight_possible = True
    f[7] = 1.0 if straight_possible else 0.0

    f[8] = 1.0 if any(s >= 3 for s in suits) else 0.0

    connected = 0
    for i in range(len(community)):
        for j in range(i+1, len(community)):
            if abs(card_rank(community[i]) - card_rank(community[j])) <= 2:
                connected += 1
    f[9] = connected / max(len(community), 1)
    f[10] = (12 - max_rank) / 12.0
    f[11] = min(f[7]*0.3 + f[8]*0.3 + f[9]*0.2 + (1-f[3])*0.2, 1.0)
    f[12] = len(community) / 5.0

    return f

def extract_features(community, pot, hero_stack, villain_stack, current_bet, big_blind, is_ip) -> List[float]:
    features = [0.0] * NUM_FEATURES

    board = extract_board_texture(community)
    features[0:13] = board

    eff = min(hero_stack, villain_stack)
    spr = eff / pot if pot > 0 else 20.0
    features[13] = min(pot / eff if eff > 0 else 0, 2.0) / 2.0
    features[14] = min(spr / 20.0, 1.0)
    features[15] = min(current_bet / pot if pot > 0 else 0, 3.0) / 3.0

    bb = max(big_blind, 1)
    features[16] = min(hero_stack / bb / 200.0, 1.0)
    features[17] = min(villain_stack / bb / 200.0, 1.0)

    # range summary placeholder (will come from Bayesian tracker at inference)
    features[18:22] = [0.5, 0.3, 0.1, 0.1]

    # street one-hot
    nc = len(community)
    features[22] = 1.0 if nc == 0 else 0.0
    features[23] = 1.0 if nc == 3 else 0.0
    features[24] = 1.0 if nc == 4 else 0.0
    features[25] = 1.0 if nc == 5 else 0.0

    features[26] = 1.0 if is_ip else 0.0

    return features

# ---------------------------------------------------------------------------
# Info set key encoding (mirrors abstraction.rs)
# ---------------------------------------------------------------------------

def hand_strength_bucket(hole, community, num_buckets=10):
    """Estimate hand strength via simple rollout (CPU, for data generation)."""
    wins, total = 0, 0
    available = [c for c in DECK if c not in hole and c not in community]

    for _ in range(200):  # 200 rollouts per position
        random.shuffle(available)
        opp = available[:2]
        remaining = 5 - len(community)
        runout = available[2:2+remaining]
        full_board = list(community) + runout

        hero_score = eval_hand(list(hole) + full_board)
        opp_score = eval_hand(list(opp) + full_board)
        if hero_score > opp_score: wins += 2
        elif hero_score == opp_score: wins += 1
        total += 2

    equity = wins / total
    return min(int(equity * num_buckets), num_buckets - 1)

def eval_hand(cards7):
    """Simple 7-card hand evaluator. Returns comparable score."""
    best = 0
    for i in range(7):
        for j in range(i+1, 7):
            hand5 = [cards7[k] for k in range(7) if k != i and k != j]
            score = eval_5(hand5)
            best = max(best, score)
    return best

def eval_5(hand):
    """5-card poker hand evaluator. Returns integer score."""
    ranks = sorted([c % 13 for c in hand], reverse=True)
    suits = [c // 13 for c in hand]
    is_flush = len(set(suits)) == 1

    all_diff = len(set(ranks)) == 5
    is_straight = all_diff and (ranks[0] - ranks[4] == 4)
    is_wheel = ranks == [12, 3, 2, 1, 0]

    if is_flush and (is_straight or is_wheel):
        return (8 << 20) | (3 if is_wheel else ranks[0])

    freq = {}
    for r in ranks: freq[r] = freq.get(r, 0) + 1

    quads = trips = 0
    pairs = []
    kickers = []
    for r in sorted(freq.keys(), reverse=True):
        if freq[r] == 4: quads = r + 1
        elif freq[r] == 3: trips = r + 1
        elif freq[r] == 2: pairs.append(r + 1)
        else: kickers.append(r)

    if quads: return (7 << 20) | ((quads-1) << 4) | kickers[0]
    if trips and pairs: return (6 << 20) | ((trips-1) << 4) | (pairs[0]-1)
    if is_flush: return (5 << 20) | (ranks[0]<<16) | (ranks[1]<<12) | (ranks[2]<<8) | (ranks[3]<<4) | ranks[4]
    if is_straight: return (4 << 20) | ranks[0]
    if is_wheel: return (4 << 20) | 3
    if trips: return (3 << 20) | ((trips-1)<<8) | (kickers[0]<<4) | kickers[1]
    if len(pairs) >= 2: return (2 << 20) | ((pairs[0]-1)<<8) | ((pairs[1]-1)<<4) | kickers[0]
    if pairs: return (1 << 20) | ((pairs[0]-1)<<12) | (kickers[0]<<8) | (kickers[1]<<4) | kickers[2]
    return (ranks[0]<<16) | (ranks[1]<<12) | (ranks[2]<<8) | (ranks[3]<<4) | ranks[4]

# ---------------------------------------------------------------------------
# Training data generation
# ---------------------------------------------------------------------------

@dataclass
class TrainingSample:
    features: List[float]
    target_policy: List[float]  # from blueprint(s)
    target_value: float         # estimated EV
    weight: float               # teacher confidence (100M > 5M > 1M)

def generate_positions(blueprints: Dict[str, Dict[bytes, List[float]]], num_positions: int) -> List[TrainingSample]:
    """Generate training positions by dealing random boards and querying blueprints."""
    samples = []
    teacher_weights = {
        'strategy_100m': 1.0,
        'strategy_5m': 0.5,
        'strategy_1m': 0.3,
        'strategy_100k': 0.1,
    }

    for i in range(num_positions):
        if i % 10000 == 0 and i > 0:
            print(f"  generated {i}/{num_positions} positions")

        deck = list(DECK)
        random.shuffle(deck)
        hole = deck[:2]

        # random street
        street = random.choice([0, 3, 4, 5])
        community = deck[2:2+street]

        # random game state
        big_blind = 10
        pot = random.choice([20, 40, 60, 100, 200, 500])
        hero_stack = random.choice([200, 500, 1000, 2000])
        villain_stack = random.choice([200, 500, 1000, 2000])
        current_bet = random.choice([0, 10, 20, 50, 100])
        is_ip = random.random() > 0.5

        features = extract_features(community, pot, hero_stack, villain_stack, current_bet, big_blind, is_ip)

        # compute hand bucket for blueprint lookup
        bucket = hand_strength_bucket(hole, community)
        # build a simple history key (abstract)
        history_byte = 0  # simplified — just the bucket
        key = bytes([bucket, street, history_byte])

        # query all blueprints, weighted average
        total_weight = 0.0
        policy = [0.0] * NUM_ACTIONS
        found_any = False

        for name, blueprint in blueprints.items():
            w = 1.0
            for prefix, tw in teacher_weights.items():
                if prefix in name: w = tw; break

            # try exact key, then just bucket
            probs = blueprint.get(key) or blueprint.get(bytes([bucket]))
            if probs is None: continue

            found_any = True
            total_weight += w
            # map blueprint actions to our 6-action space
            mapped = map_actions(probs)
            for j in range(NUM_ACTIONS):
                policy[j] += w * mapped[j]

        if not found_any:
            continue  # skip positions with no blueprint data

        for j in range(NUM_ACTIONS):
            policy[j] /= total_weight

        # value estimate from equity
        equity = bucket / 9.0  # rough: bucket 0=worst, 9=best
        value = equity * pot - (1 - equity) * current_bet

        samples.append(TrainingSample(
            features=features,
            target_policy=policy,
            target_value=value / max(pot, 1),  # normalize
            weight=total_weight,
        ))

    print(f"generated {len(samples)} training samples from {num_positions} positions")
    return samples

def map_actions(probs: List[float]) -> List[float]:
    """Map variable-length blueprint action probs to fixed 6-action space."""
    out = [0.0] * NUM_ACTIONS
    # blueprint has: fold, check, call, bet/raise variants
    if len(probs) >= 1: out[0] = probs[0]  # fold
    if len(probs) >= 2: out[1] = probs[1]  # check
    if len(probs) >= 3: out[2] = probs[2]  # call
    if len(probs) >= 4: out[3] = probs[3]  # bet small
    if len(probs) >= 5: out[4] = probs[4]  # bet big
    if len(probs) >= 6: out[5] = probs[5]  # allin
    # normalize
    total = sum(out)
    if total > 0:
        out = [p / total for p in out]
    else:
        out = [1/NUM_ACTIONS] * NUM_ACTIONS
    return out

# ---------------------------------------------------------------------------
# CTM-MoE model (PyTorch)
# ---------------------------------------------------------------------------

def build_and_train(samples: List[TrainingSample], output_path: str, epochs: int = 100):
    """Train CTM-MoE on generated samples."""
    try:
        import torch
        import torch.nn as nn
        import torch.optim as optim
        from torch.utils.data import DataLoader, TensorDataset
    except ImportError:
        print("PyTorch not available. Install with: pip install torch")
        # save samples as JSON for later training
        data = [{'f': s.features, 'p': s.target_policy, 'v': s.target_value, 'w': s.weight} for s in samples]
        Path(output_path.replace('.pt', '.json')).write_text(json.dumps(data))
        print(f"saved {len(samples)} samples as JSON")
        return

    device = torch.device('cuda' if torch.cuda.is_available() else 'cpu')
    print(f"training on {device}")

    # prepare tensors
    X = torch.tensor([s.features for s in samples], dtype=torch.float32)
    Y_policy = torch.tensor([s.target_policy for s in samples], dtype=torch.float32)
    Y_value = torch.tensor([s.target_value for s in samples], dtype=torch.float32)
    W = torch.tensor([s.weight for s in samples], dtype=torch.float32)

    dataset = TensorDataset(X, Y_policy, Y_value, W)
    loader = DataLoader(dataset, batch_size=512, shuffle=True)

    class CTMExpert(nn.Module):
        """One expert: small MLP with variable-step thinking."""
        def __init__(self, input_dim, hidden_dim=128):
            super().__init__()
            self.step_net = nn.Sequential(
                nn.Linear(input_dim + hidden_dim, hidden_dim),
                nn.GELU(),
                nn.Linear(hidden_dim, hidden_dim),
                nn.GELU(),
            )
            self.halt_net = nn.Linear(hidden_dim, 1)
            self.value_head = nn.Linear(hidden_dim, 1)
            self.policy_head = nn.Linear(hidden_dim, NUM_ACTIONS)
            self.init_hidden = nn.Parameter(torch.randn(hidden_dim) * 0.01)

        def forward(self, x, max_steps=8):
            batch = x.shape[0]
            h = self.init_hidden.unsqueeze(0).expand(batch, -1)

            total_value = torch.zeros(batch, device=x.device)
            total_policy = torch.zeros(batch, NUM_ACTIONS, device=x.device)
            remaining = torch.ones(batch, device=x.device)

            for step in range(max_steps):
                inp = torch.cat([x, h], dim=-1)
                h = self.step_net(inp)
                halt_prob = torch.sigmoid(self.halt_net(h)).squeeze(-1)

                v = self.value_head(h).squeeze(-1)
                p = torch.softmax(self.policy_head(h), dim=-1)

                # accumulate weighted by halting probability
                emit = remaining * halt_prob
                total_value += emit * v
                total_policy += emit.unsqueeze(-1) * p
                remaining = remaining * (1 - halt_prob)

            # remainder goes to last step
            v = self.value_head(h).squeeze(-1)
            p = torch.softmax(self.policy_head(h), dim=-1)
            total_value += remaining * v
            total_policy += remaining.unsqueeze(-1) * p

            return total_value, total_policy

    class MoECTM(nn.Module):
        """Mixture of CTM experts with learned router."""
        def __init__(self, input_dim=NUM_FEATURES, num_experts=NUM_EXPERTS, top_k=2):
            super().__init__()
            self.router = nn.Sequential(
                nn.Linear(input_dim, 64),
                nn.GELU(),
                nn.Linear(64, num_experts),
            )
            self.experts = nn.ModuleList([CTMExpert(input_dim) for _ in range(num_experts)])
            self.top_k = top_k

        def forward(self, x):
            # route
            logits = self.router(x)
            gates = torch.softmax(logits, dim=-1)
            topk_vals, topk_idx = gates.topk(self.top_k, dim=-1)
            # normalize top-k gates
            topk_vals = topk_vals / topk_vals.sum(dim=-1, keepdim=True)

            batch = x.shape[0]
            total_value = torch.zeros(batch, device=x.device)
            total_policy = torch.zeros(batch, NUM_ACTIONS, device=x.device)

            for k in range(self.top_k):
                expert_idx = topk_idx[:, k]  # [batch]
                gate_weight = topk_vals[:, k]  # [batch]

                # gather by expert (simple loop over experts)
                for e in range(len(self.experts)):
                    mask = (expert_idx == e)
                    if not mask.any(): continue
                    x_e = x[mask]
                    v_e, p_e = self.experts[e](x_e)
                    g_e = gate_weight[mask]
                    total_value[mask] += g_e * v_e
                    total_policy[mask] += g_e.unsqueeze(-1) * p_e

            return total_value, total_policy, gates

    model = MoECTM().to(device)
    optimizer = optim.AdamW(model.parameters(), lr=1e-3, weight_decay=1e-4)
    scheduler = optim.lr_scheduler.CosineAnnealingLR(optimizer, T_max=epochs)

    print(f"model params: {sum(p.numel() for p in model.parameters()):,}")
    print(f"training {epochs} epochs on {len(samples)} samples")

    for epoch in range(epochs):
        total_loss = 0
        for batch_x, batch_p, batch_v, batch_w in loader:
            batch_x = batch_x.to(device)
            batch_p = batch_p.to(device)
            batch_v = batch_v.to(device)
            batch_w = batch_w.to(device)

            pred_v, pred_p, gates = model(batch_x)

            # weighted MSE for value
            value_loss = (batch_w * (pred_v - batch_v) ** 2).mean()

            # weighted KL divergence for policy
            log_pred = torch.log(pred_p + 1e-8)
            policy_loss = (batch_w.unsqueeze(-1) * batch_p * (torch.log(batch_p + 1e-8) - log_pred)).sum(dim=-1).mean()

            # load balancing loss (encourage all experts to be used)
            expert_usage = gates.mean(dim=0)
            balance_loss = NUM_EXPERTS * (expert_usage ** 2).sum()

            loss = value_loss + policy_loss + 0.01 * balance_loss

            optimizer.zero_grad()
            loss.backward()
            torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
            optimizer.step()

            total_loss += loss.item()

        scheduler.step()

        if (epoch + 1) % 10 == 0 or epoch == 0:
            avg_loss = total_loss / len(loader)
            print(f"epoch {epoch+1}/{epochs}: loss={avg_loss:.4f}")

    # save model
    torch.save({
        'model_state': model.state_dict(),
        'config': {
            'num_features': NUM_FEATURES,
            'num_actions': NUM_ACTIONS,
            'num_experts': NUM_EXPERTS,
            'top_k': 2,
        }
    }, output_path)
    print(f"model saved to {output_path}")

    # also export as ONNX for Rust inference
    try:
        dummy = torch.randn(1, NUM_FEATURES).to(device)
        onnx_path = output_path.replace('.pt', '.onnx')
        torch.onnx.export(model, dummy, onnx_path,
                          input_names=['features'],
                          output_names=['value', 'policy', 'gates'],
                          dynamic_axes={'features': {0: 'batch'}})
        print(f"ONNX exported to {onnx_path}")
    except Exception as e:
        print(f"ONNX export failed: {e}")

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--blueprints', nargs='+', required=True, help='blueprint .bin files')
    parser.add_argument('--num-positions', type=int, default=500000)
    parser.add_argument('--epochs', type=int, default=100)
    parser.add_argument('--output', default='ctm_moe.pt')
    args = parser.parse_args()

    # load all blueprints
    blueprints = {}
    for path in args.blueprints:
        name = Path(path).stem
        blueprints[name] = load_blueprint(path)

    # generate training data
    print(f"\n=== generating {args.num_positions} training positions ===")
    samples = generate_positions(blueprints, args.num_positions)

    # train
    print(f"\n=== training CTM-MoE ===")
    build_and_train(samples, args.output, args.epochs)
