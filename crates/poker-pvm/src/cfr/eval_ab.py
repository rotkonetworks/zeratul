#!/usr/bin/env python3
"""A/B: Compare blueprint-only vs blueprint+MoE decisions.

Loads self-play data (blueprint decisions as ground truth from MCCFR),
then measures how MoE modifies decisions and whether those modifications
improve expected value.

Key insight: if MoE policy diverges from blueprint but the value prediction
is more accurate, the MoE is adding signal beyond the blueprint.
"""

import struct
import argparse
import numpy as np
from pathlib import Path

NUM_FEATURES = 27
NUM_ACTIONS = 6
ACTION_NAMES = ['fold', 'check', 'call', 'bet', 'raise', 'allin']

def load_data(path, max_samples=100000):
    data = Path(path).read_bytes()
    pos = 0
    count = struct.unpack_from('<I', data, pos)[0]; pos += 4
    n_feat = struct.unpack_from('<I', data, pos)[0]; pos += 4
    n_act = struct.unpack_from('<I', data, pos)[0]; pos += 4
    count = min(count, max_samples)

    features, policies, values, pots, expert_ids = [], [], [], [], []
    for _ in range(count):
        f = list(struct.unpack_from(f'<{n_feat}f', data, pos)); pos += n_feat * 4
        p = list(struct.unpack_from(f'<{n_act}f', data, pos)); pos += n_act * 4
        v = struct.unpack_from('<f', data, pos)[0]; pos += 4
        pot = struct.unpack_from('<I', data, pos)[0]; pos += 4
        eid = data[pos]; pos += 1
        features.append(f); policies.append(p); values.append(v)
        pots.append(pot); expert_ids.append(eid)

    return (np.array(features, dtype=np.float32), np.array(policies, dtype=np.float32),
            np.array(values, dtype=np.float32), np.array(pots, dtype=np.int32),
            np.array(expert_ids, dtype=np.int64))

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--data', required=True)
    parser.add_argument('--onnx-dir', required=True)
    parser.add_argument('--version', type=int, default=4)
    parser.add_argument('--samples', type=int, default=100000)
    args = parser.parse_args()

    import onnxruntime as ort

    X, Yp, Yv, Pots, E = load_data(args.data, args.samples)
    print(f"loaded {len(X)} samples")

    d = Path(args.onnx_dir)
    v = args.version
    names = ['headsup', 'preflop_multi', 'postflop_wet', 'postflop_dry', 'shortstack', 'river_polar']

    router = ort.InferenceSession(str(d / f'router_v{v}.onnx'))
    experts = {}
    for i, name in enumerate(names):
        path = d / f'expert_{name}_v{v}.onnx'
        if path.exists():
            experts[i] = ort.InferenceSession(str(path))

    # get MoE predictions for all samples
    moe_policies = np.zeros_like(Yp)
    moe_values = np.zeros(len(X))

    for idx in range(len(X)):
        feat = X[idx:idx+1]
        logits = router.run(None, {'features': feat})[0][0]
        exp_l = np.exp(logits - logits.max())
        probs = exp_l / exp_l.sum()
        top2 = np.argsort(probs)[-2:][::-1]
        w0 = probs[top2[0]] / (probs[top2[0]] + probs[top2[1]])
        w1 = 1.0 - w0

        pv, pp = 0.0, np.zeros(NUM_ACTIONS)
        for eidx, w in [(top2[0], w0), (top2[1], w1)]:
            if eidx not in experts: continue
            outs = experts[eidx].run(None, {'features': feat})
            ev = outs[0].flat[0]
            ep = outs[1][0]
            pv += w * ev; pp += w * ep

        psum = pp.sum()
        if psum > 1e-8: pp /= psum
        moe_policies[idx] = pp
        moe_values[idx] = pv

    # === Analysis ===
    print(f"\n{'='*60}")
    print(f"A/B: Blueprint vs Blueprint+MoE (v{v})")
    print(f"{'='*60}")

    # 1. Action agreement
    bp_actions = Yp.argmax(axis=1)
    moe_actions = moe_policies.argmax(axis=1)
    agreement = (bp_actions == moe_actions).mean()
    print(f"\nAction agreement: {agreement:.1%}")

    # 2. Where they disagree, who's action is "bolder"?
    disagree_mask = bp_actions != moe_actions
    n_disagree = disagree_mask.sum()
    if n_disagree > 0:
        bp_dis = bp_actions[disagree_mask]
        moe_dis = moe_actions[disagree_mask]
        moe_more_aggressive = (moe_dis > bp_dis).sum()
        moe_more_passive = (moe_dis < bp_dis).sum()
        print(f"Disagreements: {n_disagree} ({100*n_disagree/len(X):.1f}%)")
        print(f"  MoE more aggressive: {moe_more_aggressive} ({100*moe_more_aggressive/n_disagree:.1f}%)")
        print(f"  MoE more passive:    {moe_more_passive} ({100*moe_more_passive/n_disagree:.1f}%)")

    # 3. Policy entropy (higher = more mixed/balanced)
    def entropy(p):
        p = p + 1e-8
        return -(p * np.log(p)).sum(axis=-1)

    bp_ent = entropy(Yp).mean()
    moe_ent = entropy(moe_policies).mean()
    print(f"\nPolicy entropy: blueprint={bp_ent:.3f}  MoE={moe_ent:.3f}")
    if moe_ent > bp_ent:
        print(f"  MoE plays more mixed (harder to exploit)")
    else:
        print(f"  MoE plays more polarized")

    # 4. Value calibration: does MoE predict outcomes better?
    bp_val_mse = ((Yv - 0.0)**2).mean()  # blueprint has no explicit value, assume 0 (neutral)
    moe_val_mse = ((Yv - moe_values)**2).mean()
    print(f"\nValue MSE: baseline(0)={bp_val_mse:.4f}  MoE={moe_val_mse:.4f}")
    if moe_val_mse < bp_val_mse:
        print(f"  MoE value head is informative (beats trivial predictor)")
    else:
        print(f"  MoE value head not yet beating trivial predictor")

    # 5. Per-expert breakdown
    print(f"\nPer-expert action agreement:")
    for i, name in enumerate(names):
        mask = E == i
        if mask.sum() < 10: continue
        agree = (bp_actions[mask] == moe_actions[mask]).mean()
        ent_bp = entropy(Yp[mask]).mean()
        ent_moe = entropy(moe_policies[mask]).mean()
        print(f"  {name:20s}: n={mask.sum():>6}  agree={agree:.1%}  entropy bp={ent_bp:.3f} moe={ent_moe:.3f}")

    # 6. Biggest disagreements by pot size
    if n_disagree > 0:
        big_pots = Pots[disagree_mask]
        top_idx = np.argsort(big_pots)[-5:][::-1]
        print(f"\nTop 5 disagreements by pot size:")
        dis_indices = np.where(disagree_mask)[0]
        for rank, ti in enumerate(top_idx):
            real_idx = dis_indices[ti]
            print(f"  pot={Pots[real_idx]:>5}  blueprint={ACTION_NAMES[bp_actions[real_idx]]}  "
                  f"MoE={ACTION_NAMES[moe_actions[real_idx]]}  "
                  f"expert={names[E[real_idx]] if E[real_idx] < 6 else '?'}  "
                  f"outcome={Yv[real_idx]:+.2f}")

if __name__ == '__main__':
    main()
