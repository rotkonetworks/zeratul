#!/usr/bin/env python3
"""Export CTM-MoE PyTorch models to ONNX format."""

import argparse
import os
from pathlib import Path

NUM_FEATURES = 27
NUM_ACTIONS = 9  # must match training
MAX_THINK = 8

def export(model_dir, version, output_dir):
    try:
        import torch
        import torch.nn as nn
    except ImportError:
        print("PyTorch not available")
        return

    os.makedirs(output_dir, exist_ok=True)

    class CTMExpert(nn.Module):
        def __init__(self, hidden=128):
            super().__init__()
            self.step = nn.Sequential(
                nn.utils.parametrizations.spectral_norm(nn.Linear(NUM_FEATURES + hidden, hidden)),
                nn.GELU(),
                nn.utils.parametrizations.spectral_norm(nn.Linear(hidden, hidden)),
                nn.GELU())
            self.halt = nn.Linear(hidden, 1)
            self.vhead = nn.Linear(hidden, 1)
            self.phead = nn.Linear(hidden, NUM_ACTIONS)
            self.h0 = nn.Parameter(torch.randn(hidden) * 0.01)

        def forward(self, x):
            b = x.shape[0]
            h = self.h0.unsqueeze(0).expand(b, -1)
            tv = torch.zeros(b)
            tp = torch.zeros(b, NUM_ACTIONS)
            rem = torch.ones(b)
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

    class Router(nn.Module):
        def __init__(self):
            super().__init__()
            self.net = nn.Sequential(nn.Linear(NUM_FEATURES, 64), nn.GELU(), nn.Linear(64, 6))
        def forward(self, x):
            return self.net(x)

    EXPERT_NAMES = ['headsup', 'preflop_multi', 'postflop_wet', 'postflop_dry', 'shortstack', 'river_polar']
    dummy = torch.randn(1, NUM_FEATURES)

    for name in EXPERT_NAMES:
        pt_path = f"{model_dir}/expert_{name}_v{version}.pt"
        if not os.path.exists(pt_path):
            print(f"  {name}: not found, skipping")
            continue

        model = CTMExpert()
        checkpoint = torch.load(pt_path, map_location='cpu', weights_only=True)
        model.load_state_dict(checkpoint['model_state'])
        model.eval()

        onnx_path = f"{output_dir}/expert_{name}_v{version}.onnx"
        torch.onnx.export(model, dummy, onnx_path,
            input_names=['features'], output_names=['value', 'policy'],
            dynamic_axes={'features': {0: 'batch'}},
            opset_version=17)
        print(f"  {name} -> {onnx_path}")

    # router
    rt_path = f"{model_dir}/router_v{version}.pt"
    if os.path.exists(rt_path):
        router = Router()
        checkpoint = torch.load(rt_path, map_location='cpu', weights_only=True)
        router.load_state_dict(checkpoint['model_state'])
        router.eval()

        onnx_path = f"{output_dir}/router_v{version}.onnx"
        torch.onnx.export(router, dummy, onnx_path,
            input_names=['features'], output_names=['logits'],
            dynamic_axes={'features': {0: 'batch'}},
            opset_version=17)
        print(f"  router -> {onnx_path}")

if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--model-dir', required=True)
    parser.add_argument('--version', type=int, required=True)
    parser.add_argument('--output-dir', required=True)
    args = parser.parse_args()
    export(args.model_dir, args.version, args.output_dir)
