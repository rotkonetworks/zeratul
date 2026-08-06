#!/usr/bin/env python3
"""Export MoE PyTorch weights to flat binary format for native Rust CTM.

Extracts weight matrices from each expert + router into a single .bin file
that NativeMoE can load without ONNX or PyTorch.

Format:
  u32: num_experts
  u32: input_dim (27)
  u32: hidden_dim (128)
  u32: num_actions (9)
  For each expert:
    u8: max_ticks
    [step_net_0.weight, step_net_0.bias] — spectral_norm stores as weight_orig
    [step_net_2.weight, step_net_2.bias]
    [halt.weight, halt.bias]
    [vhead.weight, vhead.bias]
    [phead.weight, phead.bias]
    [h0]
  Router:
    [linear1.weight, linear1.bias]
    [linear2.weight, linear2.bias]
"""
import struct
import sys
import torch
from pathlib import Path

def write_tensor(f, t):
    """Write tensor as f32 little-endian"""
    data = t.detach().cpu().float().contiguous().numpy().tobytes()
    f.write(data)

def extract_expert(model_path):
    """Load expert .pt and extract weight matrices"""
    raw = torch.load(model_path, map_location='cpu', weights_only=True)
    # model_state contains the actual weights
    state = raw.get('model_state', raw)
    if isinstance(state, dict) and not any(k.startswith('step') or k.startswith('halt') for k in state):
        # Try unwrapping OrderedDict
        for v in state.values():
            if hasattr(v, 'keys') and any(str(k).startswith('step') for k in v.keys()):
                state = v
                break

    def get_weight(prefix):
        # Try spectral norm format first
        key = f"{prefix}.parametrizations.weight.original"
        if key in state:
            return state[key]
        key = f"{prefix}.weight"
        if key in state:
            return state[key]
        raise KeyError(f"No weight found for {prefix}, keys: {[k for k in state.keys() if prefix in k]}")

    def get_bias(prefix):
        return state[f"{prefix}.bias"]

    # step = Sequential(spectral_norm(Linear(in+hid, hid)), GELU, spectral_norm(Linear(hid, hid)), GELU)
    # In state_dict: step.0.parametrizations.weight.original, step.0.bias, step.2.parametrizations.weight.original, step.2.bias
    step0_w = get_weight("step.0")
    step0_b = get_bias("step.0")
    step2_w = get_weight("step.2")
    step2_b = get_bias("step.2")
    halt_w = state["halt.weight"]
    halt_b = state["halt.bias"]
    vhead_w = state["vhead.weight"]
    vhead_b = state["vhead.bias"]
    phead_w = state["phead.weight"]
    phead_b = state["phead.bias"]
    h0 = state["h0"]

    return {
        'step0_w': step0_w, 'step0_b': step0_b,
        'step2_w': step2_w, 'step2_b': step2_b,
        'halt_w': halt_w, 'halt_b': halt_b,
        'vhead_w': vhead_w, 'vhead_b': vhead_b,
        'phead_w': phead_w, 'phead_b': phead_b,
        'h0': h0,
    }

def extract_router(model_path):
    raw = torch.load(model_path, map_location='cpu', weights_only=True)
    # Flatten nested dicts
    state = {}
    def flatten(d, prefix=''):
        for k, v in d.items():
            key = f"{prefix}{k}" if not prefix else f"{prefix}.{k}"
            if hasattr(v, 'shape'):
                state[key] = v
            elif hasattr(v, 'items'):
                flatten(v, key)
    flatten(raw)
    # Find the linear layers
    def find(suffix):
        for k, v in state.items():
            if k.endswith(suffix):
                return v
        raise KeyError(f"No key ending with {suffix}, keys: {list(state.keys())}")
    return {
        'l1_w': find('0.weight'), 'l1_b': find('0.bias'),
        'l2_w': find('2.weight'), 'l2_b': find('2.bias'),
    }

def main():
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument('--version', type=int, default=5)
    parser.add_argument('--model-dir', default='models/')
    parser.add_argument('--output', default='models/moe_native.bin')
    args = parser.parse_args()

    expert_names = ['preflop_multi', 'postflop_wet', 'postflop_dry', 'shortstack', 'river_polar']
    expert_ticks = [6, 8, 8, 3, 6]

    experts = []
    for name in expert_names:
        path = Path(args.model_dir) / f"expert_{name}_v{args.version}.pt"
        if not path.exists():
            print(f"  {name}: not found, skipping")
            continue
        e = extract_expert(path)
        hidden = e['h0'].shape[0]
        input_dim = e['step0_w'].shape[1] - hidden
        n_actions = e['phead_w'].shape[0]
        print(f"  {name}: in={input_dim} hid={hidden} act={n_actions} step0={list(e['step0_w'].shape)}")
        experts.append(e)

    router_path = Path(args.model_dir) / f"router_v{args.version}.pt"
    router = extract_router(router_path)
    print(f"  router: {list(router['l1_w'].shape)} -> {list(router['l2_w'].shape)}")

    # Write binary
    with open(args.output, 'wb') as f:
        input_dim = experts[0]['step0_w'].shape[1] - experts[0]['h0'].shape[0]
        hidden_dim = experts[0]['h0'].shape[0]
        n_actions = experts[0]['phead_w'].shape[0]

        f.write(struct.pack('<I', len(experts)))
        f.write(struct.pack('<I', input_dim))
        f.write(struct.pack('<I', hidden_dim))
        f.write(struct.pack('<I', n_actions))

        for i, e in enumerate(experts):
            f.write(struct.pack('<B', expert_ticks[i]))
            write_tensor(f, e['step0_w'])
            write_tensor(f, e['step0_b'])
            write_tensor(f, e['step2_w'])
            write_tensor(f, e['step2_b'])
            write_tensor(f, e['halt_w'])
            write_tensor(f, e['halt_b'])
            write_tensor(f, e['vhead_w'])
            write_tensor(f, e['vhead_b'])
            write_tensor(f, e['phead_w'])
            write_tensor(f, e['phead_b'])
            write_tensor(f, e['h0'])

        # Router
        write_tensor(f, router['l1_w'])
        write_tensor(f, router['l1_b'])
        write_tensor(f, router['l2_w'])
        write_tensor(f, router['l2_b'])

    print(f"\nwrote {args.output} ({Path(args.output).stat().st_size} bytes)")

if __name__ == '__main__':
    main()
