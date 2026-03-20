#!/usr/bin/env python3
"""Phase 2: fine-tune CTM-MoE with real hand histories (PHH format).

Parses Pluribus/WSOP hands, extracts features + actual human actions,
routes each decision point to the correct expert, fine-tunes the model.

Run after phase 1 (blueprint training):
    python train_phase2_phh.py --model ctm_moe.pt --phh-dir phh-data/ --output ctm_moe_v2.pt
"""

import os
import re
import json
import struct
import random
import argparse
from pathlib import Path
from dataclasses import dataclass
from typing import List, Dict, Tuple, Optional

NUM_FEATURES = 27
NUM_ACTIONS = 6  # fold, check, call, bet_small, bet_big, allin

# card parsing
RANK_MAP = {'2':0,'3':1,'4':2,'5':3,'6':4,'7':5,'8':6,'9':7,'T':8,'J':9,'Q':10,'K':11,'A':12}
SUIT_MAP = {'c':0,'d':1,'h':2,'s':3}

def parse_card(s):
    """Parse 'Ah' → card index (rank + suit*13)."""
    if len(s) != 2 or s[0] == '?': return None
    r = RANK_MAP.get(s[0])
    su = SUIT_MAP.get(s[1])
    if r is None or su is None: return None
    return r + su * 13

def parse_phh(path):
    """Parse a .phh file into structured hand data."""
    text = Path(path).read_text()

    hand = {}
    # parse key = value lines
    for line in text.split('\n'):
        line = line.strip()
        if line.startswith('#') or '=' not in line: continue
        key, val = line.split('=', 1)
        key = key.strip()
        val = val.strip()
        try:
            hand[key] = eval(val)  # safe enough for this data format
        except:
            hand[key] = val

    if 'actions' not in hand or 'starting_stacks' not in hand:
        return None

    variant = hand.get('variant', '')
    if variant not in ('NT', 'FT'):  # NL or FL texas hold'em only
        return None

    return hand

@dataclass
class DecisionPoint:
    """One decision point extracted from a real hand."""
    features: List[float]
    action_taken: int  # 0-5 index into NUM_ACTIONS
    player_count: int
    street: int  # 0=preflop, 1=flop, 2=turn, 3=river
    expert_id: int  # which expert should train on this

def extract_decisions(hand) -> List[DecisionPoint]:
    """Extract all decision points from a parsed hand."""
    stacks = list(hand['starting_stacks'])
    n = len(stacks)
    blinds = hand.get('blinds_or_straddles', [0]*n)
    big_blind = max(blinds) if blinds else max(hand.get('antes', [1]*n))
    if big_blind == 0: big_blind = 1

    actions = hand['actions']
    community = []
    pot = sum(hand.get('antes', [0]*n)) + sum(blinds)
    current_bet = max(blinds)
    bets = list(blinds) + [0] * (n - len(blinds))
    folded = [False] * n
    decisions = []

    for action_str in actions:
        action_str = action_str.strip().strip("'\"")

        # dealer actions
        if action_str.startswith('d db '):
            # deal board
            cards_str = action_str[5:].strip()
            for i in range(0, len(cards_str), 2):
                c = parse_card(cards_str[i:i+2])
                if c is not None: community.append(c)
            current_bet = 0
            bets = [0] * n
            continue

        if action_str.startswith('d dh '):
            continue  # deal hole cards, skip

        # player action: "pN action [amount]"
        m = re.match(r'p(\d+)\s+(\w+)\s*([\d.]*)', action_str)
        if not m: continue

        player = int(m.group(1)) - 1  # 0-indexed
        act = m.group(2)
        amount = int(float(m.group(3))) if m.group(3) else 0

        if folded[player]: continue

        # determine street
        nc = len(community)
        street = 0 if nc == 0 else (1 if nc == 3 else (2 if nc == 4 else 3))

        # active players
        active = sum(1 for i in range(n) if not folded[i])

        # compute features
        hero_stack = stacks[player]
        # villain = biggest other stack
        villain_stack = max(stacks[i] for i in range(n) if i != player and not folded[i]) if active > 1 else hero_stack
        is_ip = player == 0  # simplified

        features = extract_features(community, pot, hero_stack, villain_stack, current_bet, big_blind, is_ip)

        # map action to our 6-action space
        action_idx = map_phh_action(act, amount, pot, current_bet)

        # route to expert
        expert_id = route_decision(n, street, community, hero_stack, villain_stack, big_blind, pot)

        decisions.append(DecisionPoint(
            features=features,
            action_taken=action_idx,
            player_count=n,
            street=street,
            expert_id=expert_id,
        ))

        # update state
        if act == 'f':
            folded[player] = True
        elif act == 'cc':
            call_amount = current_bet - bets[player]
            stacks[player] -= call_amount
            pot += call_amount
            bets[player] = current_bet
        elif act in ('cbr', 'br', 'r'):
            raise_to = amount
            cost = raise_to - bets[player]
            stacks[player] -= cost
            pot += cost
            bets[player] = raise_to
            current_bet = raise_to
        elif act == 'sm':  # show mucked
            pass

    return decisions

def map_phh_action(act, amount, pot, current_bet):
    """Map PHH action string to 0-5 index."""
    if act == 'f': return 0  # fold
    if act == 'cc' and current_bet == 0: return 1  # check
    if act == 'cc': return 2  # call
    if act in ('cbr', 'br', 'r'):
        if amount == 0: return 3  # bet small (min)
        ratio = amount / max(pot, 1)
        if ratio >= 2.0: return 5  # allin-ish
        if ratio >= 0.75: return 4  # bet big
        return 3  # bet small
    return 1  # default check

def route_decision(n, street, community, hero_stack, villain_stack, big_blind, pot):
    """Route a decision point to expert 0-5."""
    eff_bb = min(hero_stack, villain_stack) / max(big_blind, 1)
    spr = min(hero_stack, villain_stack) / max(pot, 1)

    # expert 0: heads-up
    if n == 2: return 0

    # expert 4: short stack
    if eff_bb < 30 or spr < 3: return 4

    # expert 5: river with big bets
    if street == 3: return 5

    # expert 1: preflop multiway
    if street == 0: return 1

    # experts 2/3: postflop wet/dry
    if community:
        suits = [c // 13 for c in community]
        ranks = sorted([c % 13 for c in community])
        flush_draw = max(suits.count(s) for s in range(4)) >= 3
        connected = any(ranks[i+1] - ranks[i] <= 2 for i in range(len(ranks)-1)) if len(ranks) > 1 else False
        if flush_draw or connected: return 2  # wet
        return 3  # dry

    return 1  # fallback

def extract_features(community, pot, hero_stack, villain_stack, current_bet, big_blind, is_ip):
    """Extract 27 features (same as ctm.rs)."""
    features = [0.0] * NUM_FEATURES

    # board texture (13 features)
    if community:
        ranks = [0]*13; suits = [0]*4; max_rank = 0
        for c in community:
            r, s = c % 13, c // 13
            ranks[r] += 1; suits[s] += 1
            max_rank = max(max_rank, r)
        features[0] = max_rank / 12.0
        pairs = sum(1 for c in ranks if c == 2)
        trips = sum(1 for c in ranks if c >= 3)
        features[1] = pairs / 2.0
        features[2] = float(trips)
        features[3] = 1.0 if pairs > 0 or trips > 0 else 0.0
        max_suit = max(suits)
        features[4] = 1.0 if max_suit >= len(community) else 0.0
        features[6] = 1.0 if max_suit <= 1 and len(community) >= 3 else 0.0
        features[8] = 1.0 if any(s >= 3 for s in suits) else 0.0
        features[12] = len(community) / 5.0

    # pot geometry
    eff = min(hero_stack, villain_stack)
    features[13] = min(pot / eff if eff > 0 else 0, 2.0) / 2.0
    spr = eff / pot if pot > 0 else 20.0
    features[14] = min(spr / 20.0, 1.0)
    features[15] = min(current_bet / pot if pot > 0 else 0, 3.0) / 3.0

    # stacks
    bb = max(big_blind, 1)
    features[16] = min(hero_stack / bb / 200.0, 1.0)
    features[17] = min(villain_stack / bb / 200.0, 1.0)

    # range summary placeholder
    features[18:22] = [0.5, 0.3, 0.1, 0.1]

    # street one-hot
    nc = len(community)
    features[22] = 1.0 if nc == 0 else 0.0
    features[23] = 1.0 if nc == 3 else 0.0
    features[24] = 1.0 if nc == 4 else 0.0
    features[25] = 1.0 if nc == 5 else 0.0

    features[26] = 1.0 if is_ip else 0.0
    return features


def load_and_sort_phh(phh_dir):
    """Load all PHH hands, extract decisions, sort by expert."""
    expert_data = {i: [] for i in range(6)}
    total = 0

    for root, dirs, files in os.walk(phh_dir):
        for f in files:
            if not f.endswith('.phh'): continue
            path = os.path.join(root, f)
            hand = parse_phh(path)
            if hand is None: continue

            decisions = extract_decisions(hand)
            for d in decisions:
                expert_data[d.expert_id].append(d)
                total += 1

    print(f"extracted {total} decision points from PHH data:")
    for i in range(6):
        names = ['heads-up', '3-6 preflop', 'postflop wet', 'postflop dry', 'short stack', 'river']
        print(f"  expert {i} ({names[i]}): {len(expert_data[i])} decisions")

    return expert_data


def fine_tune(model_path, expert_data, output_path, epochs=50):
    """Fine-tune CTM-MoE model with PHH decision data."""
    try:
        import torch
        import torch.nn as nn
        import torch.optim as optim
    except ImportError:
        print("PyTorch not available")
        # save as JSON
        data = {}
        for eid, decisions in expert_data.items():
            data[str(eid)] = [{'f': d.features, 'a': d.action_taken} for d in decisions]
        Path(output_path.replace('.pt', '_phh.json')).write_text(json.dumps(data))
        return

    device = torch.device('cuda' if torch.cuda.is_available() else 'cpu')

    # load phase 1 model
    checkpoint = torch.load(model_path, map_location=device, weights_only=False)
    print(f"loaded model from {model_path}")

    # rebuild model
    from train_ctm import MoECTM
    model = MoECTM()
    model.load_state_dict(checkpoint['model_state'])
    model = model.to(device)

    # prepare per-expert data
    all_features = []
    all_actions = []
    all_expert_targets = []

    for eid, decisions in expert_data.items():
        for d in decisions:
            all_features.append(d.features)
            all_actions.append(d.action_taken)
            all_expert_targets.append(eid)

    if not all_features:
        print("no training data")
        return

    X = torch.tensor(all_features, dtype=torch.float32).to(device)
    Y = torch.tensor(all_actions, dtype=torch.long).to(device)
    E = torch.tensor(all_expert_targets, dtype=torch.long).to(device)

    optimizer = optim.AdamW(model.parameters(), lr=1e-4, weight_decay=1e-4)
    criterion = nn.CrossEntropyLoss()

    print(f"fine-tuning on {len(all_features)} decisions, {epochs} epochs")

    for epoch in range(epochs):
        # shuffle
        perm = torch.randperm(len(all_features))
        X_s, Y_s, E_s = X[perm], Y[perm], E[perm]

        total_loss = 0
        batch_size = 256
        for i in range(0, len(X_s), batch_size):
            bx = X_s[i:i+batch_size]
            by = Y_s[i:i+batch_size]

            pred_v, pred_p, gates = model(bx)
            loss = criterion(pred_p, by)

            # expert routing loss: encourage routing to match target expert
            be = E_s[i:i+batch_size]
            route_loss = nn.CrossEntropyLoss()(model.router(bx), be)

            total_loss_batch = loss + 0.1 * route_loss

            optimizer.zero_grad()
            total_loss_batch.backward()
            torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
            optimizer.step()
            total_loss += total_loss_batch.item()

        if (epoch + 1) % 10 == 0:
            print(f"  epoch {epoch+1}/{epochs}: loss={total_loss / max(len(X_s)//batch_size, 1):.4f}")

    torch.save({
        'model_state': model.state_dict(),
        'config': checkpoint['config'],
        'phh_decisions': len(all_features),
    }, output_path)
    print(f"fine-tuned model saved to {output_path}")


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--model', default='ctm_moe.pt')
    parser.add_argument('--phh-dir', required=True)
    parser.add_argument('--output', default='ctm_moe_v2.pt')
    parser.add_argument('--epochs', type=int, default=50)
    args = parser.parse_args()

    expert_data = load_and_sort_phh(args.phh_dir)
    fine_tune(args.model, expert_data, args.output, args.epochs)
