#!/usr/bin/env python3
"""
Neuroplasticity benchmark for poker CTM-MoE.

Measures whether the CTM can adapt its strategy after seeing new opponent
patterns, WITHOUT full retraining. This is the core test for whether
CTM's sync-based memory enables real-time adaptation.

The test:
  1. Train CTM-MoE to play GTO (Nash equilibrium) from blueprints
  2. Evaluate against a balanced opponent → baseline exploitability
  3. Introduce an exploitable opponent (e.g., folds river 80%)
  4. Run compact_memory / plastic update with opponent's tendencies
  5. Evaluate again → did the CTM adapt? Does it bet river more?

Plasticity score:
  0.0 = no adaptation (strategy unchanged after seeing new data)
  1.0 = perfect adaptation (found the optimal exploit immediately)

Usage:
    python plasticity_bench.py --expert headsup --version 2 --opponent fold_river
    python plasticity_bench.py --expert headsup --version 2 --opponent call_station
    python plasticity_bench.py --all  # run all opponent profiles
"""

import os
import json
import random
import math
import argparse
from dataclasses import dataclass
from typing import List, Dict, Tuple
from pathlib import Path

import numpy as np

# Reuse feature extraction + model from train_experts
from train_experts import (
    NUM_FEATURES, NUM_ACTIONS, MAX_THINK_STEPS, EXPERTS,
    extract_features, classify_situation, DECK
)

try:
    import torch
    import torch.nn as nn
    import torch.nn.functional as F
    HAS_TORCH = True
except ImportError:
    HAS_TORCH = False


class CTMExpert(nn.Module):
    """Matches the v2 trained model state dict layout."""
    def __init__(self, input_dim=NUM_FEATURES, hidden_dim=128, n_actions=NUM_ACTIONS):
        super().__init__()
        self.step = nn.Sequential(
            nn.Linear(input_dim + hidden_dim, hidden_dim), nn.GELU(),
            nn.Linear(hidden_dim, hidden_dim), nn.GELU(),
        )
        self.halt = nn.Linear(hidden_dim, 1)
        self.vhead = nn.Linear(hidden_dim, 1)
        self.phead = nn.Linear(hidden_dim, n_actions)
        self.h0 = nn.Parameter(torch.randn(hidden_dim) * 0.01)
        self.n_actions = n_actions

    def forward(self, x, max_steps=MAX_THINK_STEPS):
        batch = x.shape[0]
        h = self.h0.unsqueeze(0).expand(batch, -1)
        total_v = torch.zeros(batch, device=x.device)
        total_p = torch.zeros(batch, self.n_actions, device=x.device)
        remaining = torch.ones(batch, device=x.device)
        for s in range(max_steps):
            h = self.step(torch.cat([x, h], dim=-1))
            halt_p = torch.sigmoid(self.halt(h)).squeeze(-1)
            v = self.vhead(h).squeeze(-1)
            p = torch.softmax(self.phead(h), dim=-1)
            emit = remaining * halt_p
            total_v += emit * v
            total_p += emit.unsqueeze(-1) * p
            remaining = remaining * (1 - halt_p)
        total_v += remaining * self.vhead(h).squeeze(-1)
        total_p += remaining.unsqueeze(-1) * torch.softmax(self.phead(h), dim=-1)
        return total_v, total_p


# ---------------------------------------------------------------------------
# Exploitable opponent profiles
# ---------------------------------------------------------------------------

@dataclass
class OpponentProfile:
    """An opponent with a known exploitable tendency."""
    name: str
    description: str
    # Policy overrides per street: {street: [fold, check, call, bet_small, bet_med, bet_large]}
    overrides: Dict[int, List[float]]
    # The optimal exploit: what a perfect adapter would do
    optimal_exploit: Dict[int, List[float]]

OPPONENTS = {
    'fold_river': OpponentProfile(
        name='fold_river',
        description='Folds to any river bet 80% of the time',
        overrides={
            3: [0.80, 0.05, 0.10, 0.02, 0.02, 0.01],  # river: fold 80%
        },
        optimal_exploit={
            3: [0.0, 0.0, 0.0, 0.0, 0.5, 0.5],  # always bet river (any size)
        },
    ),
    'call_station': OpponentProfile(
        name='call_station',
        description='Calls any bet 90% of the time, never raises',
        overrides={
            0: [0.02, 0.03, 0.90, 0.02, 0.02, 0.01],  # preflop
            1: [0.02, 0.03, 0.90, 0.02, 0.02, 0.01],  # flop
            2: [0.02, 0.03, 0.90, 0.02, 0.02, 0.01],  # turn
            3: [0.02, 0.03, 0.90, 0.02, 0.02, 0.01],  # river
        },
        optimal_exploit={
            # Value bet big with strong hands (they always call)
            1: [0.0, 0.0, 0.1, 0.0, 0.0, 0.9],  # flop: bet large
            2: [0.0, 0.0, 0.1, 0.0, 0.0, 0.9],  # turn: bet large
            3: [0.0, 0.0, 0.1, 0.0, 0.0, 0.9],  # river: bet large
        },
    ),
    'overbluffer': OpponentProfile(
        name='overbluffer',
        description='Bets large on every street regardless of hand strength',
        overrides={
            1: [0.0, 0.0, 0.0, 0.05, 0.15, 0.80],
            2: [0.0, 0.0, 0.0, 0.05, 0.15, 0.80],
            3: [0.0, 0.0, 0.0, 0.05, 0.15, 0.80],
        },
        optimal_exploit={
            # Call down light (they're bluffing too much)
            1: [0.0, 0.0, 0.8, 0.0, 0.1, 0.1],
            2: [0.0, 0.0, 0.8, 0.0, 0.1, 0.1],
            3: [0.0, 0.0, 0.8, 0.0, 0.1, 0.1],
        },
    ),
    'nit': OpponentProfile(
        name='nit',
        description='Only plays premium hands, folds everything else preflop',
        overrides={
            0: [0.85, 0.05, 0.05, 0.02, 0.02, 0.01],  # preflop: fold 85%
        },
        optimal_exploit={
            0: [0.0, 0.0, 0.0, 0.3, 0.4, 0.3],  # steal blinds constantly
        },
    ),
}


# ---------------------------------------------------------------------------
# Generate hands against a specific opponent
# ---------------------------------------------------------------------------

def generate_opponent_hands(opponent: OpponentProfile, n_hands=5000):
    """Generate training samples that reflect opponent's tendencies.

    These are the hands the CTM would observe during play against this opponent.
    The samples include the opponent's actual actions (not GTO).
    """
    samples = []
    for _ in range(n_hands):
        deck = list(DECK)
        random.shuffle(deck)
        hole = deck[:2]
        street = random.choice([0, 1, 2, 3])
        community = deck[2:2 + [0, 3, 4, 5][street]]
        big_blind = 10
        hero_stack = random.choice([500, 1000, 2000])
        villain_stack = random.choice([500, 1000, 2000])
        pot = random.choice([20, 40, 60, 100, 200])
        current_bet = random.choice([0, 10, 20, 50])
        is_ip = random.random() > 0.5

        features = extract_features(
            community, pot, hero_stack, villain_stack,
            current_bet, big_blind, is_ip)

        # Opponent's policy for this street
        if street in opponent.overrides:
            policy = list(opponent.overrides[street])
        else:
            # Default: roughly GTO-ish
            policy = [0.15, 0.20, 0.25, 0.15, 0.15, 0.10]

        # Normalize
        total = sum(policy)
        policy = [p / total for p in policy]

        # Value estimate from opponent perspective
        equity = random.random()
        value = (equity * pot - (1 - equity) * current_bet) / max(pot, 1)

        samples.append({
            'features': features,
            'policy': policy,
            'value': value,
            'street': street,
        })

    return samples


# ---------------------------------------------------------------------------
# Plasticity mechanisms
# ---------------------------------------------------------------------------

def add_plastic_adapter(model, rank=8):
    """Add a LoRA-like plastic adapter to CTMExpert.

    This is the mechanism for rapid adaptation without full retraining.
    Only the adapter weights get updated during compact_memory.
    """
    D = model.phead.in_features  # hidden_dim
    n_act = model.phead.out_features

    model.plastic_A = nn.Parameter(torch.zeros(D, rank))
    model.plastic_B_v = nn.Parameter(torch.zeros(rank, 1))  # value adapter
    model.plastic_B_p = nn.Parameter(torch.zeros(rank, n_act))  # policy adapter
    model.plastic_gate = nn.Parameter(torch.tensor(0.0))

    # Patch forward to include plastic pathway
    original_forward = model.forward

    def plastic_forward(x, max_steps=MAX_THINK_STEPS):
        v, p = original_forward(x, max_steps)
        gate = torch.sigmoid(model.plastic_gate)
        if gate > 1e-4:
            # Get last hidden state for plastic readout
            batch = x.shape[0]
            h = model.h0.unsqueeze(0).expand(batch, -1)
            for s in range(max_steps):
                h = model.step(torch.cat([x, h], dim=-1))
            plastic_h = h @ model.plastic_A  # [B, rank]
            v = v + gate * (plastic_h @ model.plastic_B_v).squeeze(-1)
            p = p + gate * torch.softmax(plastic_h @ model.plastic_B_p, dim=-1)
            p = p / p.sum(dim=-1, keepdim=True)  # renormalize
        return v, p

    model.forward = plastic_forward
    return model


def compact_memory(model, opponent_samples, lr=1e-3, steps=50):
    """Compact observed opponent patterns into plastic adapter weights.

    Only updates plastic_A, plastic_B_v, plastic_B_p, plastic_gate.
    Main model weights stay frozen.

    This is the neuroplasticity mechanism: the model observes the opponent's
    tendencies and permanently adjusts its strategy via the adapter.
    """
    device = next(model.parameters()).device

    plastic_params = [model.plastic_A, model.plastic_B_v,
                      model.plastic_B_p, model.plastic_gate]
    optimizer = torch.optim.Adam(plastic_params, lr=lr)

    X = torch.tensor([s['features'] for s in opponent_samples],
                      dtype=torch.float32).to(device)
    Yp = torch.tensor([s['policy'] for s in opponent_samples],
                       dtype=torch.float32).to(device)
    Yv = torch.tensor([s['value'] for s in opponent_samples],
                       dtype=torch.float32).to(device)

    # Freeze everything except plastic params
    for p in model.parameters():
        p.requires_grad_(False)
    for p in plastic_params:
        p.requires_grad_(True)

    model.train()
    for step in range(steps):
        idx = torch.randperm(len(X))[:256]
        v_pred, p_pred = model(X[idx])
        v_loss = ((v_pred - Yv[idx]) ** 2).mean()
        p_loss = -(Yp[idx] * torch.log(p_pred + 1e-8)).sum(-1).mean()
        loss = v_loss + p_loss
        optimizer.zero_grad()
        loss.backward()
        optimizer.step()

    # Unfreeze
    for p in model.parameters():
        p.requires_grad_(True)

    gate = torch.sigmoid(model.plastic_gate).item()
    return {'final_loss': loss.item(), 'gate': gate, 'steps': steps}


# ---------------------------------------------------------------------------
# Evaluation
# ---------------------------------------------------------------------------

def evaluate_strategy(model, opponent: OpponentProfile, n_hands=2000):
    """Evaluate model's strategy against an opponent.

    Returns per-street action distributions and exploitation metrics.
    """
    model.eval()
    device = next(model.parameters()).device

    street_actions = {s: np.zeros(NUM_ACTIONS) for s in range(4)}
    street_counts = {s: 0 for s in range(4)}
    total_value = 0.0

    with torch.no_grad():
        for _ in range(n_hands):
            deck = list(DECK)
            random.shuffle(deck)
            hole = deck[:2]
            street = random.choice([0, 1, 2, 3])
            community = deck[2:2 + [0, 3, 4, 5][street]]
            big_blind = 10
            hero_stack = 1000
            villain_stack = 1000
            pot = random.choice([20, 40, 60, 100])
            current_bet = random.choice([0, 10, 20])
            is_ip = True

            features = extract_features(
                community, pot, hero_stack, villain_stack,
                current_bet, big_blind, is_ip)

            x = torch.tensor([features], dtype=torch.float32).to(device)
            v, p = model(x)

            policy = p[0].cpu().numpy()
            street_actions[street] += policy
            street_counts[street] += 1
            total_value += v[0].item()

    # Normalize
    for s in range(4):
        if street_counts[s] > 0:
            street_actions[s] /= street_counts[s]

    return {
        'street_actions': {s: a.tolist() for s, a in street_actions.items()},
        'street_counts': street_counts,
        'mean_value': total_value / n_hands,
    }


def compute_plasticity_score(baseline_eval, adapted_eval, opponent: OpponentProfile):
    """Compute plasticity score: how much did the model shift toward the optimal exploit?

    Score = cosine similarity between (adaptation direction) and (optimal exploit direction).
    Averaged across streets where the opponent has exploitable tendencies.
    """
    scores = []
    for street, optimal in opponent.optimal_exploit.items():
        if street not in baseline_eval['street_actions']:
            continue

        baseline = np.array(baseline_eval['street_actions'][street])
        adapted = np.array(adapted_eval['street_actions'][street])
        optimal = np.array(optimal)

        # Direction of adaptation
        delta = adapted - baseline
        # Direction of optimal exploit
        target_delta = optimal - baseline

        # Cosine similarity
        dot = np.dot(delta, target_delta)
        norm_d = np.linalg.norm(delta) + 1e-8
        norm_t = np.linalg.norm(target_delta) + 1e-8
        cos_sim = dot / (norm_d * norm_t)

        # Scale by magnitude: did it move enough?
        magnitude = np.linalg.norm(delta) / (np.linalg.norm(target_delta) + 1e-8)
        magnitude = min(magnitude, 1.0)

        score = max(0, cos_sim) * magnitude
        scores.append(score)

    return float(np.mean(scores)) if scores else 0.0


# ---------------------------------------------------------------------------
# Main benchmark
# ---------------------------------------------------------------------------

def run_benchmark(expert_name, version, opponent_name, model_dir='../../models',
                  compact_steps=50, compact_lr=1e-3, plastic_rank=8):
    """Run the full plasticity benchmark for one expert × one opponent."""

    if not HAS_TORCH:
        print("PyTorch not available")
        return None

    device = torch.device('cuda' if torch.cuda.is_available() else 'cpu')

    # Load trained expert
    model_path = f"{model_dir}/expert_{expert_name}_v{version}.pt"
    if not os.path.exists(model_path):
        print(f"Model not found: {model_path}")
        return None

    ckpt = torch.load(model_path, map_location=device, weights_only=False)
    model = CTMExpert().to(device)
    model.load_state_dict(ckpt['model_state'])
    print(f"Loaded {model_path} ({ckpt.get('samples', '?')} training samples)")

    # Add plastic adapter
    model = add_plastic_adapter(model, rank=plastic_rank)
    model = model.to(device)

    opponent = OPPONENTS[opponent_name]
    print(f"\nOpponent: {opponent.name} — {opponent.description}")

    # 1. Baseline evaluation (before seeing opponent)
    print("\n1. Baseline evaluation (GTO-trained model)...")
    baseline = evaluate_strategy(model, opponent)
    for s in sorted(baseline['street_actions']):
        actions = baseline['street_actions'][s]
        street_name = ['preflop', 'flop', 'turn', 'river'][s]
        print(f"   {street_name}: fold={actions[0]:.2f} check={actions[1]:.2f} "
              f"call={actions[2]:.2f} bet_s={actions[3]:.2f} bet_m={actions[4]:.2f} bet_l={actions[5]:.2f}")

    # 2. Generate opponent hands (what we observe during play)
    print(f"\n2. Observing {opponent.name} for 5000 hands...")
    opponent_hands = generate_opponent_hands(opponent, n_hands=5000)

    # 3. Compact memory (plastic update)
    print(f"\n3. compact_memory ({compact_steps} steps, lr={compact_lr})...")
    compact_stats = compact_memory(model, opponent_hands, lr=compact_lr, steps=compact_steps)
    print(f"   Final loss: {compact_stats['final_loss']:.4f}, "
          f"gate: {compact_stats['gate']:.4f}")

    # 4. Post-adaptation evaluation
    print("\n4. Post-adaptation evaluation...")
    adapted = evaluate_strategy(model, opponent)
    for s in sorted(adapted['street_actions']):
        actions = adapted['street_actions'][s]
        street_name = ['preflop', 'flop', 'turn', 'river'][s]
        print(f"   {street_name}: fold={actions[0]:.2f} check={actions[1]:.2f} "
              f"call={actions[2]:.2f} bet_s={actions[3]:.2f} bet_m={actions[4]:.2f} bet_l={actions[5]:.2f}")

    # 5. Compute plasticity score
    score = compute_plasticity_score(baseline, adapted, opponent)

    print(f"\n{'='*50}")
    print(f"PLASTICITY SCORE: {score:.4f}")
    print(f"  0.0 = no adaptation")
    print(f"  1.0 = perfect exploit")
    print(f"{'='*50}")

    # Show what changed
    print(f"\nStrategy shifts (adapted - baseline):")
    for street, optimal in opponent.optimal_exploit.items():
        street_name = ['preflop', 'flop', 'turn', 'river'][street]
        b = np.array(baseline['street_actions'].get(street, [0]*NUM_ACTIONS))
        a = np.array(adapted['street_actions'].get(street, [0]*NUM_ACTIONS))
        delta = a - b
        opt = np.array(optimal)
        print(f"  {street_name}:")
        print(f"    shifted: {[f'{d:+.3f}' for d in delta]}")
        print(f"    optimal: {[f'{o:.3f}' for o in opt]}")

    return {
        'expert': expert_name,
        'opponent': opponent_name,
        'plasticity_score': score,
        'compact_stats': compact_stats,
        'baseline_value': baseline['mean_value'],
        'adapted_value': adapted['mean_value'],
    }


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description='CTM Neuroplasticity Benchmark')
    parser.add_argument('--expert', default='headsup')
    parser.add_argument('--version', type=int, default=2)
    parser.add_argument('--opponent', default='fold_river',
                        choices=list(OPPONENTS.keys()))
    parser.add_argument('--all', action='store_true',
                        help='Run all opponent profiles')
    parser.add_argument('--model-dir', default='../../models')
    parser.add_argument('--compact-steps', type=int, default=50)
    parser.add_argument('--compact-lr', type=float, default=1e-3)
    parser.add_argument('--plastic-rank', type=int, default=8)
    args = parser.parse_args()

    if args.all:
        results = []
        for opp_name in OPPONENTS:
            print(f"\n{'#'*60}")
            print(f"# OPPONENT: {opp_name}")
            print(f"{'#'*60}")
            r = run_benchmark(
                args.expert, args.version, opp_name, args.model_dir,
                args.compact_steps, args.compact_lr, args.plastic_rank)
            if r:
                results.append(r)

        print(f"\n\n{'='*60}")
        print("PLASTICITY BENCHMARK SUMMARY")
        print(f"{'='*60}")
        for r in results:
            print(f"  {r['opponent']:15s}: plasticity={r['plasticity_score']:.4f} "
                  f"value_shift={r['adapted_value']-r['baseline_value']:+.4f} "
                  f"gate={r['compact_stats']['gate']:.4f}")
        if results:
            mean_score = np.mean([r['plasticity_score'] for r in results])
            print(f"\n  MEAN PLASTICITY: {mean_score:.4f}")
    else:
        run_benchmark(
            args.expert, args.version, args.opponent, args.model_dir,
            args.compact_steps, args.compact_lr, args.plastic_rank)
