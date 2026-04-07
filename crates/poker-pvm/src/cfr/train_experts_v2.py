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
    import numpy as np
    data = Path(path).read_bytes()
    count = struct.unpack_from('<I', data, 0)[0]
    n_feat = struct.unpack_from('<I', data, 4)[0]
    n_act = struct.unpack_from('<I', data, 8)[0]

    print(f"loading {count} samples ({n_feat} features, {n_act} actions)")

    sample_size = n_feat * 4 + n_act * 4 + 4 + 4 + 1
    raw = np.frombuffer(data, dtype=np.uint8, offset=12)
    raw = raw[:count * sample_size].reshape(count, sample_size)

    features = np.frombuffer(raw[:, :n_feat*4].tobytes(), dtype=np.float32).reshape(count, n_feat).copy()
    policies = np.frombuffer(raw[:, n_feat*4:(n_feat+n_act)*4].tobytes(), dtype=np.float32).reshape(count, n_act).copy()
    values = np.frombuffer(raw[:, (n_feat+n_act)*4:(n_feat+n_act+1)*4].tobytes(), dtype=np.float32).reshape(count).copy()
    expert_ids = raw[:, -1].astype(np.int64).copy()

    return features, policies, values, expert_ids

def train(features, policies, values, expert_ids, version, output_dir, n_actions, epochs=60, resume_dir=None, only_expert=None):
    try:
        import torch
        import torch.nn as nn
        import torch.optim as optim
    except ImportError:
        print("PyTorch not available")
        return

    device = torch.device('cuda' if torch.cuda.is_available() else 'cpu')
    print(f"training on {device}, {len(features)} samples, {n_actions} actions, {epochs} epochs")

    # keep data on CPU, move batches to GPU to avoid VRAM OOM
    X = torch.from_numpy(features).float()
    Yp = torch.from_numpy(policies).float()
    Yv = torch.from_numpy(values).float()
    E = torch.from_numpy(expert_ids).long()

    MAX_THINK = 8

    # all experts get max ticks — adaptive halt stops early when policy converges
    EXPERT_MAX_TICKS = {
        'headsup': MAX_THINK,
        'preflop_multi': MAX_THINK,
        'postflop_wet': MAX_THINK,
        'postflop_dry': MAX_THINK,
        'shortstack': MAX_THINK,
        'river_polar': MAX_THINK,
    }

    class CTMExpert(nn.Module):
        def __init__(self, hidden=128, max_ticks=MAX_THINK, kl_halt_threshold=0.01):
            super().__init__()
            self.max_ticks = max_ticks
            self.kl_halt_threshold = kl_halt_threshold
            self.step = nn.Sequential(
                nn.Linear(NUM_FEATURES + hidden, hidden), nn.GELU(),
                nn.Linear(hidden, hidden), nn.GELU())
            self.halt = nn.Linear(hidden, 1)
            self.vhead = nn.Linear(hidden, 1)
            self.phead = nn.Linear(hidden, n_actions)
            self.h0 = nn.Parameter(torch.randn(hidden) * 0.01)

        def forward(self, x, adaptive=False):
            b = x.shape[0]
            h = self.h0.unsqueeze(0).expand(b, -1)
            tv = torch.zeros(b, device=x.device)
            tp = torch.zeros(b, n_actions, device=x.device)
            rem = torch.ones(b, device=x.device)
            prev_p = None
            ticks_used = self.max_ticks
            for s in range(self.max_ticks):
                h = self.step(torch.cat([x, h], -1))
                halt = torch.sigmoid(self.halt(h)).squeeze(-1)
                v = self.vhead(h).squeeze(-1)
                p = torch.softmax(self.phead(h), -1)
                emit = rem * halt
                tv += emit * v
                tp += emit.unsqueeze(-1) * p
                rem = rem * (1 - halt)
                # KL-based adaptive halt: stop when policy stops changing
                if adaptive and prev_p is not None and s >= 1:
                    kl = (p * (torch.log(p + 1e-8) - torch.log(prev_p + 1e-8))).sum(-1).mean()
                    if kl.item() < self.kl_halt_threshold:
                        ticks_used = s + 1
                        break
                prev_p = p.detach()
            # emit remainder
            tv += rem * self.vhead(h).squeeze(-1)
            tp += rem.unsqueeze(-1) * torch.softmax(self.phead(h), -1)
            return tv, tp

        def forward_weighted(self, x, step_weights=None):
            """forward with per-step loss weighting + auxiliary supervision"""
            b = x.shape[0]
            h = self.h0.unsqueeze(0).expand(b, -1)
            tv = torch.zeros(b, device=x.device)
            tp = torch.zeros(b, n_actions, device=x.device)
            rem = torch.ones(b, device=x.device)
            step_values = []
            step_policies = []
            for s in range(self.max_ticks):
                h = self.step(torch.cat([x, h], -1))
                halt = torch.sigmoid(self.halt(h)).squeeze(-1)
                v = self.vhead(h).squeeze(-1)
                p = torch.softmax(self.phead(h), -1)
                w = step_weights[s] if step_weights is not None else 1.0
                emit = rem * halt * w
                tv += emit * v
                tp += emit.unsqueeze(-1) * p
                rem = rem * (1 - halt)
                step_values.append(v)
                step_policies.append(p)
            tv += rem * self.vhead(h).squeeze(-1)
            tp += rem.unsqueeze(-1) * torch.softmax(self.phead(h), -1)
            tp_sum = tp.sum(dim=-1, keepdim=True).clamp(min=1e-8)
            tp = tp / tp_sum
            return tv, tp, step_values, step_policies

        def forward_diagnostic(self, x, targets_v=None):
            """per-step diagnostics for bound analysis"""
            b = x.shape[0]
            h = self.h0.unsqueeze(0).expand(b, -1)
            step_jac = []
            step_vloss = []
            step_halt = []
            step_kl = []
            prev_p = None
            for s in range(self.max_ticks):
                h_prev = h.clone()
                h = self.step(torch.cat([x, h], -1))
                delta = (h - h_prev).norm(dim=-1).mean()
                h_norm = h_prev.norm(dim=-1).mean().clamp(min=1e-6)
                step_jac.append((delta / h_norm).item())
                v_step = self.vhead(h).squeeze(-1)
                if targets_v is not None:
                    step_vloss.append(((v_step - targets_v)**2).mean().item())
                halt = torch.sigmoid(self.halt(h)).squeeze(-1)
                step_halt.append(halt.mean().item())
                p = torch.softmax(self.phead(h), -1)
                if prev_p is not None:
                    kl = (p * (torch.log(p + 1e-8) - torch.log(prev_p + 1e-8))).sum(-1).mean()
                    step_kl.append(kl.item())
                else:
                    step_kl.append(float('inf'))
                prev_p = p
            return {
                'jac_energy': step_jac,
                'step_vloss': step_vloss,
                'halt_probs': step_halt,
                'step_kl': step_kl,
            }

    EXPERT_NAMES = ['headsup', 'preflop_multi', 'postflop_wet', 'postflop_dry', 'shortstack', 'river_polar']

    # train each expert on its data
    for eid in range(6):
        if only_expert and EXPERT_NAMES[eid] != only_expert:
            continue
        mask = E == eid
        if mask.sum() < 100:
            print(f"  expert {EXPERT_NAMES[eid]}: {mask.sum()} samples, skipping")
            continue

        ex = X[mask]
        ep = Yp[mask]
        ev = Yv[mask]
        expert_ticks = EXPERT_MAX_TICKS.get(EXPERT_NAMES[eid], MAX_THINK)
        model = CTMExpert(max_ticks=expert_ticks).to(device)
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
        step_weights = torch.ones(expert_ticks, device=device)
        bound_every = 20

        BATCH = 2048
        print(f"  expert {EXPERT_NAMES[eid]}: {mask.sum()} samples (batch={BATCH})")
        import time as _time
        _t0 = _time.time()
        for epoch in range(epochs):
            perm = torch.randperm(len(ex))
            total_loss = 0; total_vloss = 0; total_ploss = 0; batches = 0
            for j in range(0, len(ex), BATCH):
                idx = perm[j:j+BATCH]
                bx, bp, bv = ex[idx].to(device), ep[idx].to(device), ev[idx].to(device)
                pv, pp, sv, sp = model.forward_weighted(bx, step_weights)
                vloss = ((pv - bv)**2).mean()
                ploss = (bp * (torch.log(bp + 1e-8) - torch.log(pp + 1e-8))).sum(-1).mean()
                # auxiliary: per-step value supervision (teach each step to predict well)
                aux_vloss = 0.0
                n_steps = len(sv)
                for s in range(n_steps):
                    w = step_weights[s].item() if step_weights is not None and s < len(step_weights) else 1.0
                    aux_vloss += w * ((sv[s] - bv)**2).mean() / n_steps
                loss = vloss + ploss + 0.1 * aux_vloss  # 10% auxiliary weight
                opt.zero_grad(); loss.backward()
                torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
                opt.step()
                total_loss += loss.item(); total_vloss += vloss.item(); total_ploss += ploss.item()
                batches += 1
            sched.step()
            avg_l = total_loss/max(batches,1)
            _elapsed = _time.time() - _t0
            print(f"    epoch {epoch+1}/{epochs}: loss={avg_l:.4f} [{_elapsed:.0f}s]")

            if (epoch+1) % bound_every == 0:
                # bound analysis: diagnose per-step thinking quality
                model.eval()
                with torch.no_grad():
                    n_analyze = min(1000, len(ex))
                    diag = model.forward_diagnostic(ex[:n_analyze].to(device), ev[:n_analyze].to(device))

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
                    kl_str = ' '.join(f'{k:.4f}' for k in diag.get('step_kl', []))
                    print(f"    epoch {epoch+1}: loss={avg_loss:.4f} (v={avg_v:.4f} p={avg_p:.4f}) ticks={expert_ticks}")
                    print(f"      jac:  [{jac}]")
                    print(f"      vloss:[{vloss_str}]")
                    print(f"      halt: [{halt_str}]")
                    if kl_str:
                        print(f"      kl:   [{kl_str}]")
                model.train()

        path = f"{output_dir}/expert_{EXPERT_NAMES[eid]}_v{version}.pt"
        torch.save({'model_state': model.state_dict(), 'expert': EXPERT_NAMES[eid], 'version': version}, path)
        print(f"    saved {path}")

    # train router (skip if only training one expert)
    if only_expert:
        return
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
            logits = router(X[idx].to(device))
            loss = nn.CrossEntropyLoss()(logits, E[idx].to(device))
            opt.zero_grad(); loss.backward(); opt.step()
            total_loss += loss.item(); batches += 1
        if (epoch+1) % 10 == 0:
            # evaluate router accuracy in batches to avoid OOM
            correct = 0
            for k in range(0, len(X), 4096):
                bx = X[k:k+4096].to(device)
                be = E[k:k+4096].to(device)
                correct += (router(bx).argmax(-1) == be).sum().item()
            acc = correct / len(X)
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
    parser.add_argument('--expert', default=None, help='train only this expert (e.g. shortstack)')
    args = parser.parse_args()

    os.makedirs(args.output_dir, exist_ok=True)
    features, policies, values, expert_ids = load_selfplay_data(args.data)
    n_actions = policies.shape[1] if len(policies) > 0 else 8
    train(features, policies, values, expert_ids, args.version, args.output_dir, n_actions, args.epochs, args.resume_dir, args.expert)
