#!/usr/bin/env python3
"""Train CTM-MoE v2 from Rust self-play binary data.

Reads selfplay_vN.bin (produced by cfr-selfplay), trains per-expert
CTM models + router, exports PyTorch + ONNX.

Usage:
    python train_experts_v2.py --data selfplay_v1.bin --version 1 --output-dir models/
"""

import struct
import argparse
import os
from pathlib import Path

NUM_FEATURES = 27
NUM_ACTIONS = 9  # fold, check, call, bet_25, bet_50, bet_75, bet_100, bet_200, allin

def load_selfplay_data(path):
    """Load binary self-play data: [count, n_feat, n_act, samples...]"""
    data = Path(path).read_bytes()
    pos = 0
    count = struct.unpack_from('<I', data, pos)[0]; pos += 4
    n_feat = struct.unpack_from('<I', data, pos)[0]; pos += 4
    n_act = struct.unpack_from('<I', data, pos)[0]; pos += 4

    print(f"loading {count} samples ({n_feat} features, {n_act} actions)")

    features, policies, values, expert_ids = [], [], [], []
    for _ in range(count):
        f = list(struct.unpack_from(f'<{n_feat}f', data, pos)); pos += n_feat * 4
        p = list(struct.unpack_from(f'<{n_act}f', data, pos)); pos += n_act * 4
        v = struct.unpack_from('<f', data, pos)[0]; pos += 4
        pot = struct.unpack_from('<I', data, pos)[0]; pos += 4
        eid = data[pos]; pos += 1

        features.append(f)
        policies.append(p)
        values.append(v)
        expert_ids.append(eid)

    return features, policies, values, expert_ids

def train(features, policies, values, expert_ids, version, output_dir, epochs=60):
    try:
        import torch
        import torch.nn as nn
        import torch.optim as optim
    except ImportError:
        print("PyTorch not available")
        return

    device = torch.device('cuda' if torch.cuda.is_available() else 'cpu')
    print(f"training on {device}, {len(features)} samples, {epochs} epochs")

    X = torch.tensor(features, dtype=torch.float32).to(device)
    Yp = torch.tensor(policies, dtype=torch.float32).to(device)
    Yv = torch.tensor(values, dtype=torch.float32).to(device)
    E = torch.tensor(expert_ids, dtype=torch.long).to(device)

    MAX_THINK = 8

    class CTMExpert(nn.Module):
        def __init__(self, hidden=128):
            super().__init__()
            self.step = nn.Sequential(
                nn.Linear(NUM_FEATURES + hidden, hidden), nn.GELU(),
                nn.Linear(hidden, hidden), nn.GELU())
            self.halt = nn.Linear(hidden, 1)
            self.vhead = nn.Linear(hidden, 1)
            self.phead = nn.Linear(hidden, NUM_ACTIONS)
            self.h0 = nn.Parameter(torch.randn(hidden) * 0.01)

        def forward(self, x):
            b = x.shape[0]
            h = self.h0.unsqueeze(0).expand(b, -1)
            tv = torch.zeros(b, device=x.device)
            tp = torch.zeros(b, NUM_ACTIONS, device=x.device)
            rem = torch.ones(b, device=x.device)
            for _ in range(MAX_THINK):
                h = self.step(torch.cat([x, h], -1))
                halt = torch.sigmoid(self.halt(h)).squeeze(-1)
                v = self.vhead(h).squeeze(-1)
                p = torch.softmax(self.phead(h), -1)
                emit = rem * halt
                tv += emit * v
                tp += emit.unsqueeze(-1) * p
                rem = rem * (1 - halt)
            tv += rem * self.vhead(h).squeeze(-1)
            tp += rem.unsqueeze(-1) * torch.softmax(self.phead(h), -1)
            return tv, tp

    EXPERT_NAMES = ['headsup', 'preflop_multi', 'postflop_wet', 'postflop_dry', 'shortstack', 'river_polar']

    # train each expert on its data
    for eid in range(6):
        mask = E == eid
        if mask.sum() < 100:
            print(f"  expert {EXPERT_NAMES[eid]}: {mask.sum()} samples, skipping")
            continue

        ex = X[mask]
        ep = Yp[mask]
        ev = Yv[mask]
        model = CTMExpert().to(device)
        opt = optim.AdamW(model.parameters(), lr=1e-3, weight_decay=1e-4)
        sched = optim.lr_scheduler.CosineAnnealingLR(opt, epochs)

        print(f"  expert {EXPERT_NAMES[eid]}: {mask.sum()} samples")
        for epoch in range(epochs):
            perm = torch.randperm(len(ex))
            total_loss = 0; batches = 0
            for j in range(0, len(ex), 512):
                idx = perm[j:j+512]
                bx, bp, bv = ex[idx], ep[idx], ev[idx]
                pv, pp = model(bx)
                vloss = ((pv - bv)**2).mean()
                ploss = (bp * (torch.log(bp + 1e-8) - torch.log(pp + 1e-8))).sum(-1).mean()
                loss = vloss + ploss
                opt.zero_grad(); loss.backward()
                torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
                opt.step()
                total_loss += loss.item(); batches += 1
            sched.step()
            if (epoch+1) % 20 == 0:
                print(f"    epoch {epoch+1}: loss={total_loss/max(batches,1):.4f}")

        path = f"{output_dir}/expert_{EXPERT_NAMES[eid]}_v{version}.pt"
        torch.save({'model_state': model.state_dict(), 'expert': EXPERT_NAMES[eid], 'version': version}, path)
        print(f"    saved {path}")

    # train router
    class Router(nn.Module):
        def __init__(self):
            super().__init__()
            self.net = nn.Sequential(nn.Linear(NUM_FEATURES, 64), nn.GELU(), nn.Linear(64, 6))
        def forward(self, x):
            return self.net(x)

    router = Router().to(device)
    opt = optim.AdamW(router.parameters(), lr=1e-3)
    print(f"  router: {len(X)} samples")
    for epoch in range(30):
        perm = torch.randperm(len(X))
        total_loss = 0; batches = 0
        for j in range(0, len(X), 256):
            idx = perm[j:j+256]
            logits = router(X[idx])
            loss = nn.CrossEntropyLoss()(logits, E[idx])
            opt.zero_grad(); loss.backward(); opt.step()
            total_loss += loss.item(); batches += 1
        if (epoch+1) % 10 == 0:
            acc = (router(X).argmax(-1) == E).float().mean()
            print(f"    epoch {epoch+1}: loss={total_loss/batches:.4f} acc={acc:.3f}")

    path = f"{output_dir}/router_v{version}.pt"
    torch.save({'model_state': router.state_dict(), 'version': version}, path)
    print(f"    saved {path}")

if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--data', required=True)
    parser.add_argument('--version', type=int, default=1)
    parser.add_argument('--output-dir', default='models')
    parser.add_argument('--epochs', type=int, default=60)
    args = parser.parse_args()

    os.makedirs(args.output_dir, exist_ok=True)
    features, policies, values, expert_ids = load_selfplay_data(args.data)
    train(features, policies, values, expert_ids, args.version, args.output_dir, args.epochs)
