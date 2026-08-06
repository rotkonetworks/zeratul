#!/usr/bin/env python3
"""
Autoresearch-style grid search for optimal CTM neuroplasticity config.

Searches: LR schedule, sync ratio, epoch count, grad clip, session length.
Each experiment trains a CTMExpert with sync, then measures plasticity
score (gradient-free adaptation via sync accumulation).

~10 seconds per experiment on CPU. Full grid in ~2 hours.

Usage:
    python plasticity_search.py
    python plasticity_search.py --quick   # fast subset (~15 min)
"""

import sys
import os
import json
import random
import itertools
import time
from datetime import datetime

import numpy as np
import torch
import torch.nn as nn

from plasticity_bench import (
    NUM_FEATURES, NUM_ACTIONS, MAX_THINK_STEPS, OPPONENTS,
    OpponentProfile, generate_opponent_hands, evaluate_strategy,
    compute_plasticity_score, extract_features, DECK,
)


class CTMExpertSearch(nn.Module):
    def __init__(self, hidden_dim=128, n_synch=64):
        super().__init__()
        self.step = nn.Sequential(
            nn.Linear(NUM_FEATURES + hidden_dim, hidden_dim), nn.GELU(),
            nn.Linear(hidden_dim, hidden_dim), nn.GELU())
        self.halt = nn.Linear(hidden_dim, 1)
        self.vhead = nn.Linear(hidden_dim + n_synch, 1)
        self.phead = nn.Linear(hidden_dim + n_synch, NUM_ACTIONS)
        self.h0 = nn.Parameter(torch.randn(hidden_dim) * 0.01)
        self.n_actions, self.hidden_dim, self.n_synch = NUM_ACTIONS, hidden_dim, n_synch
        self.register_buffer('sync_left', torch.randperm(hidden_dim)[:n_synch])
        self.register_buffer('sync_right', torch.randperm(hidden_dim)[:n_synch])
        self.decay = nn.Parameter(torch.zeros(n_synch))
        self._alpha = None
        self._beta = None

    def reset_memory(self):
        self._alpha = None
        self._beta = None

    def forward(self, x, persist=False):
        batch = x.shape[0]
        h = self.h0.unsqueeze(0).expand(batch, -1)
        r = torch.exp(-self.decay.clamp(0, 15))
        if persist and self._alpha is not None:
            # Detach the INITIAL state but let within-hand sync stay in graph
            alpha = self._alpha.detach().unsqueeze(0).expand(batch, -1) + 0  # +0 puts in graph
            beta = self._beta.detach().unsqueeze(0).expand(batch, -1) + 0
        else:
            alpha = torch.zeros(batch, self.n_synch, device=x.device)
            beta = torch.ones(batch, self.n_synch, device=x.device)
        total_v = torch.zeros(batch, device=x.device)
        total_p = torch.zeros(batch, self.n_actions, device=x.device)
        remaining = torch.ones(batch, device=x.device)
        for s in range(MAX_THINK_STEPS):
            h = self.step(torch.cat([x, h], dim=-1))
            alpha = r * alpha + h[:, self.sync_left] * h[:, self.sync_right]
            beta = r * beta + 1
            sync = alpha / torch.sqrt(beta)
            halt_p = torch.sigmoid(self.halt(h)).squeeze(-1)
            h_sync = torch.cat([h, sync], dim=-1)
            emit = remaining * halt_p
            total_v += emit * self.vhead(h_sync).squeeze(-1)
            total_p += emit.unsqueeze(-1) * torch.softmax(self.phead(h_sync), dim=-1)
            remaining = remaining * (1 - halt_p)
        h_sync = torch.cat([h, sync], dim=-1)
        total_v += remaining * self.vhead(h_sync).squeeze(-1)
        total_p += remaining.unsqueeze(-1) * torch.softmax(self.phead(h_sync), dim=-1)
        if persist:
            self._alpha = alpha.mean(0).detach()
            self._beta = beta.mean(0).detach()
        return total_v, total_p


def run_experiment(config):
    """Run one plasticity experiment. Returns results dict."""
    torch.manual_seed(42)
    random.seed(42)

    epochs = config['epochs']
    lr = config['lr']
    lr_schedule = config['lr_schedule']
    sync_ratio = config['sync_ratio']
    grad_clip = config['grad_clip']
    session_len = config['session_len']
    warmup_hands = config.get('warmup_hands', 10)

    model = CTMExpertSearch(n_synch=64)
    optimizer = torch.optim.AdamW(model.parameters(), lr=lr)

    if lr_schedule == 'cosine':
        scheduler = torch.optim.lr_scheduler.CosineAnnealingLR(optimizer, T_max=epochs, eta_min=lr * 0.01)
    elif lr_schedule == 'step':
        scheduler = torch.optim.lr_scheduler.StepLR(optimizer, step_size=max(1, epochs // 3), gamma=0.3)
    else:
        scheduler = None

    all_opps = list(OPPONENTS.values())
    balanced = OpponentProfile('balanced', 'GTO', overrides={}, optimal_exploit={})

    best_loss = float('inf')
    best_state = None
    t0 = time.time()

    for epoch in range(epochs):
        total_loss = 0
        n = 0
        for opp in all_opps + [balanced]:
            model.train()

            # Warmup: accumulate sync without gradients
            model.reset_memory()
            if sync_ratio > 0:
                warmup = generate_opponent_hands(opp, n_hands=warmup_hands)
                with torch.no_grad():
                    for hand in warmup:
                        model(torch.tensor([hand['features']], dtype=torch.float32), persist=True)

            # Train session
            session = generate_opponent_hands(opp, n_hands=session_len)
            for hand in session:
                x = torch.tensor([hand['features']], dtype=torch.float32)
                yp = torch.tensor([hand['policy']], dtype=torch.float32)
                yv = torch.tensor([hand['value']], dtype=torch.float32)
                use_persist = random.random() < sync_ratio
                v, p = model(x, persist=use_persist)
                loss = ((v - yv) ** 2).mean() + (-(yp * torch.log(p + 1e-8)).sum(-1)).mean()
                optimizer.zero_grad()
                loss.backward()
                torch.nn.utils.clip_grad_norm_(model.parameters(), grad_clip)
                optimizer.step()
                total_loss += loss.item()
                n += 1

        if scheduler:
            scheduler.step()

        avg = total_loss / max(n, 1)
        if avg < best_loss and not (avg != avg):  # not NaN
            best_loss = avg
            best_state = {k: v.clone() for k, v in model.state_dict().items()}

    train_time = time.time() - t0

    if best_state is None:
        return {**config, 'status': 'diverged', 'plasticity': 0, 'train_time': train_time}

    model.load_state_dict(best_state)
    model.eval()

    # Measure plasticity: 200 observation hands, no gradients
    scores = []
    for opp_name, opp in OPPONENTS.items():
        model.reset_memory()
        bl = evaluate_strategy(model, opp, n_hands=200)
        model.reset_memory()
        with torch.no_grad():
            for hand in generate_opponent_hands(opp, n_hands=200):
                model(torch.tensor([hand['features']], dtype=torch.float32), persist=True)
        ad = evaluate_strategy(model, opp, n_hands=200)
        sc = compute_plasticity_score(bl, ad, opp)
        scores.append(sc)

    mean_plasticity = float(np.mean(scores))
    return {
        **config,
        'status': 'ok',
        'plasticity': mean_plasticity,
        'best_loss': float(best_loss),
        'train_time': train_time,
        'per_opponent': {name: float(sc) for name, sc in zip(OPPONENTS.keys(), scores)},
    }


def main():
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument('--quick', action='store_true', help='Fast subset')
    parser.add_argument('--output', default='plasticity_search_results.json')
    args = parser.parse_args()

    if args.quick:
        grid = {
            'epochs': [40, 60],
            'lr': [1e-3, 3e-3],
            'lr_schedule': ['cosine'],
            'sync_ratio': [0.0, 0.3, 0.7, 1.0],
            'grad_clip': [0.5, 1.0],
            'session_len': [20],
            'warmup_hands': [10],
        }
    else:
        grid = {
            'epochs': [30, 50, 70],
            'lr': [5e-4, 1e-3, 3e-3, 1e-2],
            'lr_schedule': ['cosine', 'step', 'none'],
            'sync_ratio': [0.0, 0.1, 0.3, 0.5, 0.7, 1.0],
            'grad_clip': [0.1, 0.5, 1.0, 5.0],
            'session_len': [10, 20, 50],
            'warmup_hands': [5, 15],
        }

    # Generate all combinations
    keys = list(grid.keys())
    combos = list(itertools.product(*[grid[k] for k in keys]))
    configs = [dict(zip(keys, combo)) for combo in combos]

    # Shuffle so we see diverse results early
    random.shuffle(configs)

    print(f"Plasticity search: {len(configs)} experiments")
    print(f"Output: {args.output}")
    print()

    results = []
    best_plasticity = 0
    best_config = None

    for i, config in enumerate(configs):
        t0 = time.time()
        try:
            result = run_experiment(config)
        except Exception as e:
            result = {**config, 'status': f'error: {e}', 'plasticity': 0, 'train_time': 0}

        results.append(result)
        dt = time.time() - t0

        p = result['plasticity']
        s = result['status']
        if p > best_plasticity:
            best_plasticity = p
            best_config = config

        tag = "***NEW BEST***" if p == best_plasticity and p > 0 else ""
        print(f"[{i+1:3d}/{len(configs)}] {dt:5.1f}s | plasticity={p:.4f} | loss={result.get('best_loss', 0):.3f} | "
              f"sync={config['sync_ratio']:.1f} lr={config['lr']:.0e} clip={config['grad_clip']} "
              f"ep={config['epochs']} sched={config['lr_schedule']} sess={config['session_len']} "
              f"| {s} {tag}")

        # Save incrementally
        if (i + 1) % 10 == 0:
            with open(args.output, 'w') as f:
                json.dump({'results': results, 'best': best_config,
                           'best_plasticity': best_plasticity}, f, indent=2)

    # Final save
    with open(args.output, 'w') as f:
        json.dump({'results': results, 'best': best_config,
                   'best_plasticity': best_plasticity}, f, indent=2)

    print()
    print("=" * 60)
    print(f"BEST PLASTICITY: {best_plasticity:.4f}")
    print(f"CONFIG: {best_config}")
    print("=" * 60)

    # Top 10
    ranked = sorted(results, key=lambda r: r['plasticity'], reverse=True)
    print("\nTop 10:")
    for r in ranked[:10]:
        print(f"  plasticity={r['plasticity']:.4f} sync={r['sync_ratio']} lr={r['lr']:.0e} "
              f"clip={r['grad_clip']} ep={r['epochs']} sched={r['lr_schedule']} "
              f"sess={r['session_len']}")


if __name__ == '__main__':
    main()
