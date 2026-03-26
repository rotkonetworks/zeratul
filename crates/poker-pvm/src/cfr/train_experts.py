#!/usr/bin/env python3
"""Modular expert training for CTM-MoE poker bot.

Trains each expert independently, then trains router on top.
Each expert is a CTM (Continuous Thought Machine) specialized
for one game situation type.

Naming: expert_{name}_v{version}.pt
        router_v{version}.pt

Usage:
    # train all experts sequentially
    python train_experts.py --blueprints *.bin --phh-dir phh-data/ --version 1

    # train single expert
    python train_experts.py --expert headsup --blueprints strategy_100m.bin --version 1

    # train router on frozen experts
    python train_experts.py --router-only --version 1

    # list available experts
    python train_experts.py --list
"""

import os
import re
import struct
import random
import math
import json
import argparse
from pathlib import Path
from dataclasses import dataclass, field
from typing import List, Dict, Optional

NUM_FEATURES = 27
NUM_ACTIONS = 6
MAX_THINK_STEPS = 8

# ---------------------------------------------------------------------------
# Expert definitions
# ---------------------------------------------------------------------------

EXPERTS = {
    'headsup':        {'id': 0, 'desc': 'heads-up play (all streets)'},
    'preflop_multi':  {'id': 1, 'desc': '3-6 player preflop decisions'},
    'postflop_wet':   {'id': 2, 'desc': 'postflop on draw-heavy boards'},
    'postflop_dry':   {'id': 3, 'desc': 'postflop on static boards'},
    'shortstack':     {'id': 4, 'desc': 'effective stacks <30bb, push/fold'},
    'river_polar':    {'id': 5, 'desc': 'river decisions with polarized ranges'},
}

# ---------------------------------------------------------------------------
# Blueprint loading (same as train_ctm.py)
# ---------------------------------------------------------------------------

def load_blueprint(path: str) -> Dict[bytes, List[float]]:
    data = Path(path).read_bytes()
    pos = 0
    count = struct.unpack_from('<I', data, pos)[0]; pos += 4
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
    return strategy

# ---------------------------------------------------------------------------
# Card utilities
# ---------------------------------------------------------------------------

RANK_MAP = {'2':0,'3':1,'4':2,'5':3,'6':4,'7':5,'8':6,'9':7,'T':8,'J':9,'Q':10,'K':11,'A':12}
SUIT_MAP = {'c':0,'d':1,'h':2,'s':3}
DECK = list(range(52))

def parse_card(s):
    if len(s) != 2 or s[0] == '?': return None
    r, su = RANK_MAP.get(s[0]), SUIT_MAP.get(s[1])
    return r + su * 13 if r is not None and su is not None else None

def card_rank(c): return c % 13
def card_suit(c): return c // 13

# ---------------------------------------------------------------------------
# Feature extraction (matches ctm.rs)
# ---------------------------------------------------------------------------

def extract_features(community, pot, hero_stack, villain_stack, current_bet, big_blind, is_ip, player_count=2):
    f = [0.0] * NUM_FEATURES

    if community:
        ranks = [0]*13; suits = [0]*4; max_rank = 0
        for c in community:
            r, s = c % 13, c // 13
            ranks[r] += 1; suits[s] += 1
            max_rank = max(max_rank, r)
        f[0] = max_rank / 12.0
        pairs = sum(1 for c in ranks if c == 2)
        trips = sum(1 for c in ranks if c >= 3)
        f[1] = pairs / 2.0; f[2] = float(trips)
        f[3] = 1.0 if pairs > 0 or trips > 0 else 0.0
        max_suit = max(suits)
        f[4] = 1.0 if max_suit >= len(community) else 0.0
        f[5] = 1.0 if max_suit == len(community) - 1 else 0.0
        f[6] = 1.0 if max_suit <= 1 and len(community) >= 3 else 0.0
        straight_possible = False
        for start in range(9):
            if sum(1 for r in range(start, start+5) if ranks[r] > 0) >= 3:
                straight_possible = True; break
        f[7] = 1.0 if straight_possible else 0.0
        f[8] = 1.0 if any(s >= 3 for s in suits) else 0.0
        connected = sum(1 for i in range(len(community)) for j in range(i+1, len(community))
                        if abs(card_rank(community[i]) - card_rank(community[j])) <= 2)
        f[9] = connected / max(len(community), 1)
        f[10] = (12 - max_rank) / 12.0
        f[11] = min(f[7]*0.3 + f[8]*0.3 + f[9]*0.2 + (1-f[3])*0.2, 1.0)
        f[12] = len(community) / 5.0

    eff = min(hero_stack, villain_stack)
    f[13] = min(pot / eff if eff > 0 else 0, 2.0) / 2.0
    spr = eff / pot if pot > 0 else 20.0
    f[14] = min(spr / 20.0, 1.0)
    f[15] = min(current_bet / pot if pot > 0 else 0, 3.0) / 3.0
    bb = max(big_blind, 1)
    f[16] = min(hero_stack / bb / 200.0, 1.0)
    f[17] = min(villain_stack / bb / 200.0, 1.0)
    f[18:22] = [0.5, 0.3, 0.1, 0.1]  # range placeholder
    nc = len(community) if community else 0
    f[22] = 1.0 if nc == 0 else 0.0
    f[23] = 1.0 if nc == 3 else 0.0
    f[24] = 1.0 if nc == 4 else 0.0
    f[25] = 1.0 if nc == 5 else 0.0
    f[26] = 1.0 if is_ip else 0.0
    return f

# ---------------------------------------------------------------------------
# Situation classifier
# ---------------------------------------------------------------------------

def classify_situation(community, pot, hero_stack, villain_stack, big_blind, player_count, street):
    """Returns expert name for this situation."""
    eff_bb = min(hero_stack, villain_stack) / max(big_blind, 1)
    spr = min(hero_stack, villain_stack) / max(pot, 1)

    if player_count == 2: return 'headsup'
    if eff_bb < 30 or spr < 3: return 'shortstack'
    if street == 3: return 'river_polar'
    if street == 0: return 'preflop_multi'

    if community:
        suits = [c // 13 for c in community]
        ranks = sorted([c % 13 for c in community])
        flush_draw = max(suits.count(s) for s in range(4)) >= 3
        connected = any(ranks[i+1] - ranks[i] <= 2 for i in range(len(ranks)-1)) if len(ranks) > 1 else False
        if flush_draw or connected: return 'postflop_wet'
        return 'postflop_dry'

    return 'preflop_multi'

# ---------------------------------------------------------------------------
# Data generation per expert
# ---------------------------------------------------------------------------

@dataclass
class Sample:
    features: List[float]
    target_policy: List[float]
    target_value: float

def eval_5(hand):
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
    quads = trips = 0; pairs = []; kickers = []
    for r in sorted(freq.keys(), reverse=True):
        if freq[r] == 4: quads = r+1
        elif freq[r] == 3: trips = r+1
        elif freq[r] == 2: pairs.append(r+1)
        else: kickers.append(r)
    if quads: return (7<<20)|((quads-1)<<4)|kickers[0]
    if trips and pairs: return (6<<20)|((trips-1)<<4)|(pairs[0]-1)
    if is_flush: return (5<<20)|(ranks[0]<<16)|(ranks[1]<<12)|(ranks[2]<<8)|(ranks[3]<<4)|ranks[4]
    if is_straight: return (4<<20)|ranks[0]
    if is_wheel: return (4<<20)|3
    if trips: return (3<<20)|((trips-1)<<8)|(kickers[0]<<4)|kickers[1]
    if len(pairs)>=2: return (2<<20)|((pairs[0]-1)<<8)|((pairs[1]-1)<<4)|kickers[0]
    if pairs: return (1<<20)|((pairs[0]-1)<<12)|(kickers[0]<<8)|(kickers[1]<<4)|kickers[2]
    return (ranks[0]<<16)|(ranks[1]<<12)|(ranks[2]<<8)|(ranks[3]<<4)|ranks[4]

def hand_strength(hole, community, rollouts=100):
    available = [c for c in DECK if c not in hole and c not in community]
    wins, total = 0, 0
    for _ in range(rollouts):
        random.shuffle(available)
        opp = available[:2]
        remaining = 5 - len(community)
        board = list(community) + available[2:2+remaining]
        h_best = max(eval_5([hole[0], hole[1]] + [board[a] for a in combo])
                     for combo in _combos5from7(list(hole) + board))
        o_best = max(eval_5([opp[0], opp[1]] + [board[a] for a in combo])
                     for combo in _combos5from7(list(opp) + board))
        if h_best > o_best: wins += 2
        elif h_best == o_best: wins += 1
        total += 2
    return wins / total

def _combos5from7(cards7):
    """Yield indices of 5-card combos from 7 cards."""
    for i in range(7):
        for j in range(i+1, 7):
            yield [k for k in range(7) if k != i and k != j]

def map_actions(probs):
    out = [0.0] * NUM_ACTIONS
    for i, p in enumerate(probs[:NUM_ACTIONS]):
        out[i] = p
    total = sum(out)
    return [p/total for p in out] if total > 0 else [1/NUM_ACTIONS]*NUM_ACTIONS

def generate_expert_data(expert_name, blueprints, num_positions=50000):
    """Generate training data filtered for one expert's specialization."""
    samples = []
    teacher_weights = {'strategy_100m': 1.0, 'strategy_5m': 0.5, 'strategy_1m': 0.3, 'strategy_dcfr_100k': 0.1}

    for i in range(num_positions * 3):  # oversample since we filter
        if len(samples) >= num_positions: break
        if len(samples) % 5000 == 0 and len(samples) > 0:
            print(f"  {expert_name}: {len(samples)}/{num_positions}")

        deck = list(DECK)
        random.shuffle(deck)
        hole = deck[:2]
        street = random.choice([0, 3, 4, 5])
        community = deck[2:2+street]
        big_blind = 10
        player_count = 2 if expert_name == 'headsup' else random.choice([3, 4, 5, 6])

        # generate situation-appropriate stacks
        if expert_name == 'shortstack':
            hero_stack = random.choice([50, 100, 150, 200, 250])
            villain_stack = random.choice([50, 100, 150, 200, 250])
        else:
            hero_stack = random.choice([500, 1000, 2000])
            villain_stack = random.choice([500, 1000, 2000])

        pot = random.choice([20, 40, 60, 100, 200, 500])
        current_bet = random.choice([0, 10, 20, 50, 100])
        is_ip = random.random() > 0.5

        # check if this situation belongs to this expert
        situation = classify_situation(community, pot, hero_stack, villain_stack, big_blind, player_count, street)
        if situation != expert_name:
            continue

        features = extract_features(community, pot, hero_stack, villain_stack, current_bet, big_blind, is_ip, player_count)

        # query blueprints
        bucket = int(hand_strength(hole, community, rollouts=50) * 9.99)
        key = bytes([bucket, street, 0])
        total_w = 0.0
        policy = [0.0] * NUM_ACTIONS
        for name, bp in blueprints.items():
            w = 1.0
            for prefix, tw in teacher_weights.items():
                if prefix in name: w = tw; break
            probs = bp.get(key) or bp.get(bytes([bucket]))
            if probs is None: continue
            total_w += w
            mapped = map_actions(probs)
            for j in range(NUM_ACTIONS):
                policy[j] += w * mapped[j]

        if total_w == 0: continue
        for j in range(NUM_ACTIONS): policy[j] /= total_w

        equity = bucket / 9.0
        value = (equity * pot - (1 - equity) * current_bet) / max(pot, 1)

        samples.append(Sample(features=features, target_policy=policy, target_value=value))

    print(f"  {expert_name}: {len(samples)} samples generated")
    return samples

# ---------------------------------------------------------------------------
# PHH parser (for real hand data per expert)
# ---------------------------------------------------------------------------

def parse_phh_for_expert(phh_dir, expert_name):
    """Extract decision points from PHH hands matching this expert."""
    samples = []
    for root, dirs, files in os.walk(phh_dir):
        for f in files:
            if not f.endswith('.phh'): continue
            try:
                text = Path(os.path.join(root, f)).read_text()
                hand = {}
                for line in text.split('\n'):
                    line = line.strip()
                    if line.startswith('#') or '=' not in line: continue
                    k, v = line.split('=', 1)
                    try: hand[k.strip()] = eval(v.strip())
                    except: hand[k.strip()] = v.strip()

                if 'actions' not in hand or 'starting_stacks' not in hand: continue
                if hand.get('variant', '') not in ('NT', 'FT'): continue

                stacks = list(hand['starting_stacks'])
                n = len(stacks)
                blinds = hand.get('blinds_or_straddles', [0]*n)
                bb = max(blinds) if blinds else 1

                community = []
                pot = sum(hand.get('antes', [0]*n)) + sum(blinds[:n])
                current_bet = max(blinds[:n]) if blinds else 0
                folded = [False]*n

                for action_str in hand['actions']:
                    action_str = str(action_str).strip().strip("'\"")
                    if action_str.startswith('d db '):
                        cards = action_str[5:].strip()
                        for ci in range(0, len(cards), 2):
                            c = parse_card(cards[ci:ci+2])
                            if c is not None: community.append(c)
                        current_bet = 0; continue
                    if action_str.startswith('d dh '): continue

                    m = re.match(r'p(\d+)\s+(\w+)\s*([\d.]*)', action_str)
                    if not m: continue
                    player = int(m.group(1))-1
                    act = m.group(2)
                    amount = int(float(m.group(3))) if m.group(3) else 0
                    if folded[player]: continue

                    nc = len(community)
                    street = 0 if nc==0 else (1 if nc==3 else (2 if nc==4 else 3))
                    active = sum(1 for x in folded if not x)
                    hero_stack = stacks[player]
                    villain_stack = max((stacks[i] for i in range(n) if i!=player and not folded[i]), default=hero_stack)

                    situation = classify_situation(community, pot, hero_stack, villain_stack, bb, n, street)
                    if situation == expert_name:
                        features = extract_features(community, pot, hero_stack, villain_stack, current_bet, bb, player==0, n)
                        # map action
                        action_idx = 1  # default check
                        if act == 'f': action_idx = 0
                        elif act == 'cc' and current_bet > 0: action_idx = 2
                        elif act == 'cc': action_idx = 1
                        elif act in ('cbr','br','r'):
                            ratio = amount / max(pot, 1)
                            action_idx = 5 if ratio >= 2 else (4 if ratio >= 0.75 else 3)
                        policy = [0.0]*NUM_ACTIONS
                        policy[action_idx] = 1.0
                        samples.append(Sample(features=features, target_policy=policy, target_value=0.0))

                    # update state
                    if act == 'f': folded[player] = True
                    elif act == 'cc':
                        cost = current_bet - (stacks[player] - hero_stack) if current_bet > 0 else 0
                        pot += max(cost, 0)
                    elif act in ('cbr','br','r'):
                        pot += amount; current_bet = amount
            except: pass

    print(f"  {expert_name}: {len(samples)} PHH samples")
    return samples

# ---------------------------------------------------------------------------
# Single expert training
# ---------------------------------------------------------------------------

def train_expert(expert_name, samples, version, output_dir, epochs=80):
    try:
        import torch
        import torch.nn as nn
        import torch.optim as optim
    except ImportError:
        Path(f"{output_dir}/expert_{expert_name}_v{version}.json").write_text(
            json.dumps([{'f':s.features,'p':s.target_policy,'v':s.target_value} for s in samples]))
        return

    device = torch.device('cuda' if torch.cuda.is_available() else 'cpu')

    class CTMExpert(nn.Module):
        def __init__(self, input_dim=NUM_FEATURES, hidden_dim=128):
            super().__init__()
            self.step_net = nn.Sequential(
                nn.Linear(input_dim + hidden_dim, hidden_dim), nn.GELU(),
                nn.Linear(hidden_dim, hidden_dim), nn.GELU(),
            )
            self.halt_net = nn.Linear(hidden_dim, 1)
            self.value_head = nn.Linear(hidden_dim, 1)
            self.policy_head = nn.Linear(hidden_dim, NUM_ACTIONS)
            self.init_hidden = nn.Parameter(torch.randn(hidden_dim) * 0.01)

        def forward(self, x, max_steps=MAX_THINK_STEPS):
            batch = x.shape[0]
            h = self.init_hidden.unsqueeze(0).expand(batch, -1)
            total_v = torch.zeros(batch, device=x.device)
            total_p = torch.zeros(batch, NUM_ACTIONS, device=x.device)
            remaining = torch.ones(batch, device=x.device)
            for step in range(max_steps):
                h = self.step_net(torch.cat([x, h], dim=-1))
                halt = torch.sigmoid(self.halt_net(h)).squeeze(-1)
                v = self.value_head(h).squeeze(-1)
                p = torch.softmax(self.policy_head(h), dim=-1)
                emit = remaining * halt
                total_v += emit * v
                total_p += emit.unsqueeze(-1) * p
                remaining = remaining * (1 - halt)
            total_v += remaining * self.value_head(h).squeeze(-1)
            total_p += remaining.unsqueeze(-1) * torch.softmax(self.policy_head(h), dim=-1)
            return total_v, total_p

    model = CTMExpert().to(device)
    X = torch.tensor([s.features for s in samples], dtype=torch.float32).to(device)
    Yp = torch.tensor([s.target_policy for s in samples], dtype=torch.float32).to(device)
    Yv = torch.tensor([s.target_value for s in samples], dtype=torch.float32).to(device)

    optimizer = optim.AdamW(model.parameters(), lr=1e-3, weight_decay=1e-4)
    scheduler = optim.lr_scheduler.CosineAnnealingLR(optimizer, T_max=epochs)

    params = sum(p.numel() for p in model.parameters())
    print(f"  training expert_{expert_name}_v{version}: {params:,} params, {len(samples)} samples, {epochs} epochs")

    # Bound-guided: per-step loss weighting based on optimality gaps
    step_weights = torch.ones(MAX_THINK_STEPS, device=device)
    bound_analyze_every = 20

    for epoch in range(epochs):
        perm = torch.randperm(len(X))
        total_loss = 0; batches = 0
        for i in range(0, len(X), 512):
            idx = perm[i:i+512]
            bx, bp, bv = X[idx], Yp[idx], Yv[idx]
            pv, pp = model(bx)
            vloss = ((pv - bv)**2).mean()
            ploss = (bp * (torch.log(bp + 1e-8) - torch.log(pp + 1e-8))).sum(-1).mean()
            loss = vloss + ploss
            optimizer.zero_grad(); loss.backward()
            torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
            optimizer.step()
            total_loss += loss.item(); batches += 1
        scheduler.step()

        # Bound analysis: compute per-step Jacobian energy to find weak steps
        if (epoch + 1) % bound_analyze_every == 0:
            model.eval()
            with torch.no_grad():
                # Sample a batch for analysis
                n_analyze = min(500, len(X))
                ax = X[:n_analyze]
                h = model.init_hidden.unsqueeze(0).expand(n_analyze, -1)
                step_jac_energy = []
                step_losses = []
                for step in range(MAX_THINK_STEPS):
                    h_prev = h.clone()
                    h = model.step_net(torch.cat([ax, h], dim=-1))
                    # Jacobian proxy: ||h - h_prev|| / ||h_prev|| (how much the step changes state)
                    delta = (h - h_prev).norm(dim=-1).mean()
                    h_norm = h_prev.norm(dim=-1).mean().clamp(min=1e-6)
                    step_jac_energy.append((delta / h_norm).item())
                    # Per-step value prediction quality
                    v_step = model.value_head(h).squeeze(-1)
                    step_losses.append(((v_step - Yv[:n_analyze])**2).mean().item())

                # Weight steps inversely to their loss improvement
                # Steps with big loss → high gap → more weight
                step_losses_t = torch.tensor(step_losses, device=device)
                if step_losses_t.max() > 1e-8:
                    normalized = step_losses_t / step_losses_t.max()
                    step_weights = 1.0 + 2.0 * normalized
                else:
                    step_weights = torch.ones(MAX_THINK_STEPS, device=device)

                jac_str = ' '.join(f'{j:.3f}' for j in step_jac_energy)
                loss_str = ' '.join(f'{l:.4f}' for l in step_losses)
                print(f"    [bounds] jac_energy: [{jac_str}]")
                print(f"    [bounds] step_loss:  [{loss_str}]")
                print(f"    [bounds] weights:    [{" ".join(f"{w:.2f}" for w in step_weights.tolist())}]")
            model.train()

        if (epoch+1) % 20 == 0:
            print(f"    epoch {epoch+1}/{epochs}: loss={total_loss/max(batches,1):.4f}")

    path = f"{output_dir}/expert_{expert_name}_v{version}.pt"
    torch.save({'model_state': model.state_dict(), 'expert': expert_name, 'version': version, 'samples': len(samples)}, path)
    print(f"  saved {path}")

# ---------------------------------------------------------------------------
# Router training
# ---------------------------------------------------------------------------

def train_router(expert_models, all_samples, version, output_dir, epochs=30):
    try:
        import torch
        import torch.nn as nn
        import torch.optim as optim
    except ImportError:
        print("no pytorch"); return

    device = torch.device('cuda' if torch.cuda.is_available() else 'cpu')

    class Router(nn.Module):
        def __init__(self):
            super().__init__()
            self.net = nn.Sequential(
                nn.Linear(NUM_FEATURES, 64), nn.GELU(),
                nn.Linear(64, len(EXPERTS)),
            )
        def forward(self, x):
            return self.net(x)

    router = Router().to(device)
    X = torch.tensor([s[0] for s in all_samples], dtype=torch.float32).to(device)
    Y = torch.tensor([s[1] for s in all_samples], dtype=torch.long).to(device)

    optimizer = optim.AdamW(router.parameters(), lr=1e-3)

    print(f"  training router_v{version}: {len(all_samples)} samples")
    for epoch in range(epochs):
        perm = torch.randperm(len(X))
        total_loss = 0; batches = 0
        for i in range(0, len(X), 256):
            idx = perm[i:i+256]
            logits = router(X[idx])
            loss = nn.CrossEntropyLoss()(logits, Y[idx])
            optimizer.zero_grad(); loss.backward(); optimizer.step()
            total_loss += loss.item(); batches += 1
        if (epoch+1) % 10 == 0:
            acc = (router(X).argmax(-1) == Y).float().mean()
            print(f"    epoch {epoch+1}: loss={total_loss/batches:.4f} acc={acc:.3f}")

    path = f"{output_dir}/router_v{version}.pt"
    torch.save({'model_state': router.state_dict(), 'version': version}, path)
    print(f"  saved {path}")

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--blueprints', nargs='+', default=[])
    parser.add_argument('--phh-dir', default=None)
    parser.add_argument('--version', type=int, default=1)
    parser.add_argument('--expert', default=None, help='train single expert by name')
    parser.add_argument('--router-only', action='store_true')
    parser.add_argument('--list', action='store_true')
    parser.add_argument('--output-dir', default='.')
    parser.add_argument('--positions-per-expert', type=int, default=50000)
    parser.add_argument('--epochs', type=int, default=80)
    args = parser.parse_args()

    if args.list:
        for name, info in EXPERTS.items():
            print(f"  {name} (id={info['id']}): {info['desc']}")
        exit()

    os.makedirs(args.output_dir, exist_ok=True)

    # load blueprints
    blueprints = {}
    for p in args.blueprints:
        blueprints[Path(p).stem] = load_blueprint(p)
        print(f"loaded {p}: {len(blueprints[Path(p).stem])} info sets")

    experts_to_train = [args.expert] if args.expert else list(EXPERTS.keys())

    if not args.router_only:
        for expert_name in experts_to_train:
            print(f"\n=== expert: {expert_name} ===")

            # blueprint data
            samples = []
            if blueprints:
                samples = generate_expert_data(expert_name, blueprints, args.positions_per_expert)

            # PHH data
            if args.phh_dir:
                phh_samples = parse_phh_for_expert(args.phh_dir, expert_name)
                samples.extend(phh_samples)

            if not samples:
                print(f"  no data for {expert_name}, skipping")
                continue

            random.shuffle(samples)
            train_expert(expert_name, samples, args.version, args.output_dir, args.epochs)

    # train router
    print(f"\n=== router ===")
    router_samples = []
    for expert_name in EXPERTS:
        if blueprints:
            for s in generate_expert_data(expert_name, blueprints, 5000):
                router_samples.append((s.features, EXPERTS[expert_name]['id']))
        if args.phh_dir:
            for s in parse_phh_for_expert(args.phh_dir, expert_name):
                router_samples.append((s.features, EXPERTS[expert_name]['id']))

    if router_samples:
        random.shuffle(router_samples)
        train_router(None, router_samples, args.version, args.output_dir)

    print(f"\n=== done ===")
    print(f"outputs in {args.output_dir}/:")
    for f in sorted(Path(args.output_dir).glob('*_v*.pt')):
        print(f"  {f.name} ({f.stat().st_size // 1024}KB)")
