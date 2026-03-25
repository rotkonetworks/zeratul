#!/usr/bin/env python3
"""Evaluate CTM-MoE v4 quality: run ONNX models on test data, measure policy accuracy + value MSE."""

import struct
import argparse
import numpy as np
from pathlib import Path

NUM_FEATURES = 27
NUM_ACTIONS = 6

def load_selfplay_data(path, max_samples=50000):
    data = Path(path).read_bytes()
    pos = 0
    count = struct.unpack_from('<I', data, pos)[0]; pos += 4
    n_feat = struct.unpack_from('<I', data, pos)[0]; pos += 4
    n_act = struct.unpack_from('<I', data, pos)[0]; pos += 4
    count = min(count, max_samples)

    features, policies, values, expert_ids = [], [], [], []
    for _ in range(count):
        f = list(struct.unpack_from(f'<{n_feat}f', data, pos)); pos += n_feat * 4
        p = list(struct.unpack_from(f'<{n_act}f', data, pos)); pos += n_act * 4
        v = struct.unpack_from('<f', data, pos)[0]; pos += 4
        pot = struct.unpack_from('<I', data, pos)[0]; pos += 4
        eid = data[pos]; pos += 1
        features.append(f); policies.append(p); values.append(v); expert_ids.append(eid)

    return np.array(features, dtype=np.float32), np.array(policies, dtype=np.float32), \
           np.array(values, dtype=np.float32), np.array(expert_ids, dtype=np.int64)

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--data', required=True, help='selfplay .bin file')
    parser.add_argument('--onnx-dir', required=True, help='directory with ONNX models')
    parser.add_argument('--version', type=int, default=4)
    parser.add_argument('--samples', type=int, default=50000)
    args = parser.parse_args()

    try:
        import onnxruntime as ort
    except ImportError:
        print("onnxruntime not available, pip install onnxruntime")
        return

    X, Yp, Yv, E = load_selfplay_data(args.data, args.samples)
    print(f"loaded {len(X)} test samples")

    d = Path(args.onnx_dir)
    v = args.version
    names = ['headsup', 'preflop_multi', 'postflop_wet', 'postflop_dry', 'shortstack', 'river_polar']

    # load router
    router_path = d / f'router_v{v}.onnx'
    router = ort.InferenceSession(str(router_path))
    print(f"loaded router v{v}")

    # load experts
    experts = {}
    for i, name in enumerate(names):
        path = d / f'expert_{name}_v{v}.onnx'
        if path.exists():
            experts[i] = ort.InferenceSession(str(path))
            print(f"loaded expert {name} v{v}")

    # evaluate
    value_errors = []
    policy_kls = []
    expert_stats = {i: {'count': 0, 'val_err': [], 'pol_kl': []} for i in range(6)}

    for idx in range(len(X)):
        feat = X[idx:idx+1]
        true_p = Yp[idx]
        true_v = Yv[idx]
        eid = E[idx]

        # router prediction
        logits = router.run(None, {'features': feat})[0][0]
        exp_l = np.exp(logits - logits.max())
        probs = exp_l / exp_l.sum()
        top2 = np.argsort(probs)[-2:][::-1]

        w0 = probs[top2[0]] / (probs[top2[0]] + probs[top2[1]])
        w1 = 1.0 - w0

        pred_v = 0.0
        pred_p = np.zeros(NUM_ACTIONS)
        for k, (eidx, w) in enumerate([(top2[0], w0), (top2[1], w1)]):
            if eidx not in experts:
                continue
            outs = experts[eidx].run(None, {'features': feat})
            ev = outs[0][0] if len(outs[0].shape) == 1 else outs[0][0][0]
            ep = outs[1][0]
            pred_v += w * ev
            pred_p += w * ep

        psum = pred_p.sum()
        if psum > 1e-8:
            pred_p /= psum

        # metrics
        val_err = (pred_v - true_v) ** 2
        kl = np.sum(true_p * np.log((true_p + 1e-8) / (pred_p + 1e-8)))

        value_errors.append(val_err)
        policy_kls.append(kl)
        expert_stats[eid]['count'] += 1
        expert_stats[eid]['val_err'].append(val_err)
        expert_stats[eid]['pol_kl'].append(kl)

    print(f"\n=== v{v} MoE evaluation on {len(X)} samples ===")
    print(f"value MSE:  {np.mean(value_errors):.4f}")
    print(f"policy KL:  {np.mean(policy_kls):.4f}")
    print()
    for i, name in enumerate(names):
        s = expert_stats[i]
        if s['count'] == 0:
            continue
        print(f"  {name:20s}: n={s['count']:>6}  val_mse={np.mean(s['val_err']):.4f}  pol_kl={np.mean(s['pol_kl']):.4f}")

if __name__ == '__main__':
    main()
