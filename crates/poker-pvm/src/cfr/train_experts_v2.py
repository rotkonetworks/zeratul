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
NUM_ACTIONS = None  # read from data header

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

def train(features, policies, values, expert_ids, version, output_dir, n_actions, epochs=60, resume_dir=None):
    try:
        import torch
        import torch.nn as nn
        import torch.optim as optim
    except ImportError:
        print("PyTorch not available")
        return

    device = torch.device('cuda' if torch.cuda.is_available() else 'cpu')
    print(f"training on {device}, {len(features)} samples, {n_actions} actions, {epochs} epochs")

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
            self.phead = nn.Linear(hidden, n_actions)
            self.h0 = nn.Parameter(torch.randn(hidden) * 0.01)

        def forward(self, x):
            b = x.shape[0]
            h = self.h0.unsqueeze(0).expand(b, -1)
            tv = torch.zeros(b, device=x.device)
            tp = torch.zeros(b, n_actions, device=x.device)
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

        def forward_weighted(self, x, step_weights=None):
            """forward with per-step loss weighting fed back into the model.
            returns (value, policy, per_step_values) for weighted loss computation."""
            b = x.shape[0]
            h = self.h0.unsqueeze(0).expand(b, -1)
            tv = torch.zeros(b, device=x.device)
            tp = torch.zeros(b, n_actions, device=x.device)
            rem = torch.ones(b, device=x.device)
            step_values = []
            step_policies = []
            for s in range(MAX_THINK):
                h = self.step(torch.cat([x, h], -1))
                halt = torch.sigmoid(self.halt(h)).squeeze(-1)
                v = self.vhead(h).squeeze(-1)
                p = torch.softmax(self.phead(h), -1)
                # apply step weight to emission (amplify weak steps)
                w = step_weights[s] if step_weights is not None else 1.0
                emit = rem * halt * w
                tv += emit * v
                tp += emit.unsqueeze(-1) * p
                rem = rem * (1 - halt)
                step_values.append(v)
                step_policies.append(p)
            tv += rem * self.vhead(h).squeeze(-1)
            tp += rem.unsqueeze(-1) * torch.softmax(self.phead(h), -1)
            # renormalize policy
            tp_sum = tp.sum(dim=-1, keepdim=True).clamp(min=1e-8)
            tp = tp / tp_sum
            return tv, tp, step_values, step_policies

        def forward_diagnostic(self, x, targets_v=None):
            """forward with per-step diagnostics for bound-guided training"""
            b = x.shape[0]
            h = self.h0.unsqueeze(0).expand(b, -1)
            step_jac = []    # jacobian energy per step
            step_vloss = []  # value loss per step
            step_halt = []   # average halt probability per step
            for s in range(MAX_THINK):
                h_prev = h.clone()
                h = self.step(torch.cat([x, h], -1))
                # jacobian proxy: how much this step changes hidden state
                delta = (h - h_prev).norm(dim=-1).mean()
                h_norm = h_prev.norm(dim=-1).mean().clamp(min=1e-6)
                step_jac.append((delta / h_norm).item())
                # per-step value quality
                v_step = self.vhead(h).squeeze(-1)
                if targets_v is not None:
                    step_vloss.append(((v_step - targets_v)**2).mean().item())
                # halt probability
                halt = torch.sigmoid(self.halt(h)).squeeze(-1)
                step_halt.append(halt.mean().item())
            return {
                'jac_energy': step_jac,
                'step_vloss': step_vloss,
                'halt_probs': step_halt,
            }

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
        # resume from previous version if available
        if resume_dir:
            for prev_v in range(version, 0, -1):
                prev_path = f"{resume_dir}/expert_{EXPERT_NAMES[eid]}_v{prev_v}.pt"
                if os.path.exists(prev_path):
                    try:
                        ckpt = torch.load(prev_path, map_location=device, weights_only=True)
                        model.load_state_dict(ckpt['model_state'], strict=False)
                        print(f"    resumed from {prev_path}")
                    except Exception as e:
                        print(f"    resume failed ({e}), training from scratch")
                    break
        opt = optim.AdamW(model.parameters(), lr=1e-3, weight_decay=1e-4)
        sched = optim.lr_scheduler.CosineAnnealingLR(opt, epochs)

        # bound-guided: per-step loss weighting
        step_weights = torch.ones(MAX_THINK, device=device)
        bound_every = 20

        print(f"  expert {EXPERT_NAMES[eid]}: {mask.sum()} samples")
        for epoch in range(epochs):
            perm = torch.randperm(len(ex))
            total_loss = 0; total_vloss = 0; total_ploss = 0; batches = 0
            for j in range(0, len(ex), 512):
                idx = perm[j:j+512]
                bx, bp, bv = ex[idx], ep[idx], ev[idx]
                pv, pp, sv, sp = model.forward_weighted(bx, step_weights)
                vloss = ((pv - bv)**2).mean()
                ploss = (bp * (torch.log(bp + 1e-8) - torch.log(pp + 1e-8))).sum(-1).mean()
                # auxiliary: per-step value supervision (teach each step to predict well)
                aux_vloss = 0.0
                for s in range(MAX_THINK):
                    w = step_weights[s].item() if step_weights is not None else 1.0
                    aux_vloss += w * ((sv[s] - bv)**2).mean() / MAX_THINK
                loss = vloss + ploss + 0.1 * aux_vloss  # 10% auxiliary weight
                opt.zero_grad(); loss.backward()
                torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
                opt.step()
                total_loss += loss.item(); total_vloss += vloss.item(); total_ploss += ploss.item()
                batches += 1
            sched.step()

            if (epoch+1) % bound_every == 0:
                # bound analysis: diagnose per-step thinking quality
                model.eval()
                with torch.no_grad():
                    n_analyze = min(1000, len(ex))
                    diag = model.forward_diagnostic(ex[:n_analyze], ev[:n_analyze])

                    # reweight: steps with high loss get more gradient
                    if diag['step_vloss']:
                        losses_t = torch.tensor(diag['step_vloss'], device=device)
                        if losses_t.max() > 1e-8:
                            step_weights = 1.0 + 2.0 * (losses_t / losses_t.max())

                    avg_loss = total_loss/max(batches,1)
                    avg_v = total_vloss/max(batches,1)
                    avg_p = total_ploss/max(batches,1)
                    jac = ' '.join(f'{j:.3f}' for j in diag['jac_energy'])
                    vloss_str = ' '.join(f'{l:.4f}' for l in diag['step_vloss'])
                    halt_str = ' '.join(f'{h:.2f}' for h in diag['halt_probs'])
                    print(f"    epoch {epoch+1}: loss={avg_loss:.4f} (v={avg_v:.4f} p={avg_p:.4f})")
                    print(f"      jac:  [{jac}]")
                    print(f"      vloss:[{vloss_str}]")
                    print(f"      halt: [{halt_str}]")
                model.train()

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
    parser.add_argument('--resume-dir', default=None, help='directory with previous version models to resume from')
    args = parser.parse_args()

    os.makedirs(args.output_dir, exist_ok=True)
    features, policies, values, expert_ids = load_selfplay_data(args.data)
    n_actions = len(policies[0]) if policies else 8
    train(features, policies, values, expert_ids, args.version, args.output_dir, n_actions, args.epochs, args.resume_dir)
