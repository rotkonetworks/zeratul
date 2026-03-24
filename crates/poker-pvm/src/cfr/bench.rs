//! Benchmark CFR strategy against various opponent types.
//!
//! Measures win rate in bb/100 hands — the standard poker metric.
//! Plays thousands of hands per second in native Rust.
//!
//! Usage:
//!   cfr-bench <strategy.bin> [num_hands]
//!   cfr-bench <strategy_a.bin> --vs <strategy_b.bin> [num_hands]

use poker_pvm::*;
use poker_pvm::cfr::abstraction::*;
use poker_pvm::cfr::strategy::import_strategy;
use poker_pvm::cfr::search::{PokerBot, SearchConfig};
use std::collections::HashMap;
use std::time::Instant;

type Strategy = HashMap<Vec<u8>, Vec<f64>>;

// ── RNG ────────────────────────────────────────────────────

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self { Self(seed) }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn next_u32(&mut self) -> u32 { (self.next() >> 16) as u32 }
    fn next_f64(&mut self) -> f64 { (self.next_u32() as f64) / (u32::MAX as f64) }
    fn gen_range(&mut self, n: usize) -> usize { (self.next_u32() as usize) % n }
}

// ── deck ───────────────────────────────────────────────────

fn shuffle_deck(rng: &mut Rng) -> [u8; 52] {
    let mut deck: [u8; 52] = std::array::from_fn(|i| i as u8);
    for i in (1..52).rev() {
        let j = rng.gen_range(i + 1);
        deck.swap(i, j);
    }
    deck
}

// ── opponent types ─────────────────────────────────────────

#[derive(Clone, Copy)]
enum OpponentType {
    Random,
    CallingStation,
    Rock,
    Tag,
    Lag,
    Maniac,
    CfrStrategy,
}

fn opponent_action(
    otype: OpponentType,
    state: &GameState,
    _hole: &[u8; 2],
    _strategy: &Strategy,
    _history: &[u8],
    rng: &mut Rng,
) -> SignedAction {
    let seat = state.acting_seat;
    let s = seat as usize;
    let max_bet = state.bets.iter().take(state.num_players as usize).copied().max().unwrap_or(0);
    let facing = state.bets[s] < max_bet;
    let stack = state.stacks[s];

    let mk = |action, amount| SignedAction { seat, action, amount, seq: 0, sig: [0; 64] };

    match otype {
        OpponentType::Random => {
            let mut opts = vec![mk(Action::Fold, 0)];
            if !facing { opts.push(mk(Action::Check, 0)); }
            if facing && stack > 0 { opts.push(mk(Action::Call, 0)); }
            if stack >= state.rules.big_blind { opts.push(mk(Action::Bet, state.pot.min(stack))); }
            if stack > 0 { opts.push(mk(Action::AllIn, 0)); }
            opts[rng.gen_range(opts.len())]
        }
        OpponentType::CallingStation => {
            if facing && stack > 0 { mk(Action::Call, 0) }
            else if !facing { mk(Action::Check, 0) }
            else { mk(Action::Fold, 0) }
        }
        OpponentType::Maniac => {
            if stack > 0 { mk(Action::AllIn, 0) }
            else if !facing { mk(Action::Check, 0) }
            else { mk(Action::Fold, 0) }
        }
        OpponentType::Rock => {
            let r0 = _hole[0] % 13;
            let r1 = _hole[1] % 13;
            let premium = (r0 >= 9 && r1 >= 9) || r0 == r1;
            if premium {
                if stack >= state.rules.big_blind { mk(Action::Bet, state.pot.min(stack)) }
                else if facing && stack > 0 { mk(Action::Call, 0) }
                else { mk(Action::Check, 0) }
            } else {
                if facing { mk(Action::Fold, 0) }
                else { mk(Action::Check, 0) }
            }
        }
        OpponentType::Tag => {
            let r0 = _hole[0] % 13;
            let r1 = _hole[1] % 13;
            let hi = r0.max(r1);
            let playable = hi >= 8 || r0 == r1;
            if playable {
                if rng.next_f64() < 0.6 && stack >= state.rules.big_blind {
                    mk(Action::Bet, state.pot.min(stack))
                } else if facing && stack > 0 {
                    mk(Action::Call, 0)
                } else {
                    mk(Action::Check, 0)
                }
            } else {
                if facing { mk(Action::Fold, 0) }
                else { mk(Action::Check, 0) }
            }
        }
        OpponentType::Lag => {
            if rng.next_f64() < 0.5 && stack >= state.rules.big_blind {
                mk(Action::Bet, state.pot.min(stack))
            } else if facing && stack > 0 && rng.next_f64() < 0.7 {
                mk(Action::Call, 0)
            } else if !facing {
                mk(Action::Check, 0)
            } else {
                mk(Action::Fold, 0)
            }
        }
        OpponentType::CfrStrategy => {
            // handled by cfr_action
            mk(Action::Fold, 0) // unreachable
        }
    }
}

// ── CFR strategy action ────────────────────────────────────

fn cfr_action(
    state: &GameState,
    hole: &[u8; 2],
    community: &[u8; 5],
    strategy: &Strategy,
    history: &[u8],
    rng: &mut Rng,
) -> SignedAction {
    let seat = state.acting_seat;
    let s = seat as usize;
    let max_bet = state.bets.iter().take(state.num_players as usize).copied().max().unwrap_or(0);
    let facing = state.bets[s] < max_bet;
    let stack = state.stacks[s];

    let mk = |action: Action, amount: u32| SignedAction { seat, action, amount, seq: 0, sig: [0; 64] };

    // compute info set key
    let street = match state.phase {
        Phase::Preflop => 0u8,
        Phase::Flop => 1,
        Phase::Turn => 2,
        Phase::River => 3,
        _ => 0,
    };
    let hand_bucket = if street == 0 {
        preflop_bucket(hole[0], hole[1])
    } else {
        let board_len = match street { 1 => 3, 2 => 4, 3 => 5, _ => 0 };
        hand_strength_bucket(*hole, &community[..board_len], 10, 200, &mut || rng.next_u32())
    };

    let key = InfoSetKey { hand_bucket, history: history.to_vec(), street };
    let key_bytes = key.to_bytes();

    // look up strategy
    let cfr_probs = strategy.get(&key_bytes);

    if let Some(probs) = cfr_probs {
        // sample from CFR strategy
        // CFR actions: [fold, check/call, small_bet, big_bet/allin]
        let r = rng.next_f64();
        let mut cumul = 0.0;
        let mut chosen = probs.len() - 1;
        for (i, &p) in probs.iter().enumerate() {
            cumul += p;
            if r < cumul { chosen = i; break; }
        }

        match chosen {
            0 => mk(Action::Fold, 0),
            1 => {
                if facing && stack > 0 { mk(Action::Call, 0) }
                else { mk(Action::Check, 0) }
            }
            2 => {
                // small bet (half pot)
                let amount = (state.pot / 2).max(state.rules.big_blind).min(stack);
                if stack >= state.rules.big_blind { mk(Action::Bet, amount) }
                else if facing && stack > 0 { mk(Action::Call, 0) }
                else { mk(Action::Check, 0) }
            }
            _ => {
                // big bet / allin
                if stack > 0 { mk(Action::AllIn, 0) }
                else if !facing { mk(Action::Check, 0) }
                else { mk(Action::Fold, 0) }
            }
        }
    } else {
        // fallback: simplified hand-strength-based
        let r0 = hole[0] % 13;
        let r1 = hole[1] % 13;
        let hi = r0.max(r1);

        if street == 0 {
            if hi >= 10 || r0 == r1 {
                if stack >= state.rules.big_blind { mk(Action::Bet, state.pot.min(stack)) }
                else if facing && stack > 0 { mk(Action::Call, 0) }
                else { mk(Action::Check, 0) }
            } else if hi >= 7 {
                if facing && stack > 0 { mk(Action::Call, 0) }
                else { mk(Action::Check, 0) }
            } else {
                if facing { mk(Action::Fold, 0) }
                else { mk(Action::Check, 0) }
            }
        } else {
            // postflop fallback
            if rng.next_f64() < 0.3 && stack >= state.rules.big_blind {
                mk(Action::Bet, (state.pot / 2).max(state.rules.big_blind).min(stack))
            } else if facing && stack > 0 && rng.next_f64() < 0.4 {
                mk(Action::Call, 0)
            } else if !facing {
                mk(Action::Check, 0)
            } else {
                mk(Action::Fold, 0)
            }
        }
    }
}

// ── match runner ───────────────────────────────────────────

struct MatchStats {
    hands: u32,
    vpip: u32,
    pfr: u32,
    aggressive: u32,
    passive: u32,
    cfr_hits: u32,
    cfr_misses: u32,
}

fn play_match(
    strategy: &Strategy,
    opp_type: OpponentType,
    opp_strategy: &Strategy,
    num_hands: u32,
    rng: &mut Rng,
) -> (i64, MatchStats) {
    let rules = Rules {
        buyin: 1000, small_blind: 5, big_blind: 10,
        turn_timeout_blocks: 6, rake_bps: 0, rake_cap: 0,
    };
    let bb = rules.big_blind as i64;
    let mut total_profit: i64 = 0;
    let mut stats = MatchStats { hands: 0, vpip: 0, pfr: 0, aggressive: 0, passive: 0, cfr_hits: 0, cfr_misses: 0 };

    for hand_i in 0..num_hands {
        let mut state = GameState::new(rules, 2);
        let deck = shuffle_deck(rng);
        let cards = [[deck[0], deck[1]], [deck[2], deck[3]]];
        let community = [deck[4], deck[5], deck[6], deck[7], deck[8]];
        state.deal(&cards, community);

        let initial_stack_0 = rules.buyin as i64;
        let mut history: Vec<u8> = Vec::new();
        let mut actions = 0u32;

        loop {
            if matches!(state.phase, Phase::Showdown | Phase::Settled) { break; }
            if actions > 200 { break; } // safety

            let seat = state.acting_seat as usize;
            let hole = &cards[seat];

            let action = if seat == 0 {
                // CFR bot (seat 0)
                cfr_action(&state, hole, &community, strategy, &history, rng)
            } else {
                match opp_type {
                    OpponentType::CfrStrategy => {
                        cfr_action(&state, hole, &community, opp_strategy, &history, rng)
                    }
                    _ => opponent_action(opp_type, &state, hole, &Strategy::new(), &history, rng),
                }
            };

            // track CFR bot stats
            if seat == 0 {
                match action.action {
                    Action::Bet | Action::Raise | Action::AllIn => {
                        stats.aggressive += 1;
                        if state.phase == Phase::Preflop && actions < 4 {
                            stats.vpip += 1;
                            stats.pfr += 1;
                        }
                    }
                    Action::Call => {
                        stats.passive += 1;
                        if state.phase == Phase::Preflop && actions < 4 {
                            stats.vpip += 1;
                        }
                    }
                    Action::Check => { stats.passive += 1; }
                    _ => {}
                }
            }

            let pot = state.pot;
            let stack = state.stacks[seat];
            let abs = abstract_action(action.action, action.amount, pot, stack);
            history.push(abs);

            match state.apply(&action) {
                Ok(result) => {
                    actions += 1;
                    if result.hand_over {
                        if state.phase == Phase::Showdown {
                            state.showdown();
                        }
                        break;
                    }
                }
                Err(_) => {
                    // invalid action, try fold
                    let fold = SignedAction { seat: action.seat, action: Action::Fold, amount: 0, seq: 0, sig: [0; 64] };
                    if let Ok(r) = state.apply(&fold) {
                        if r.hand_over {
                            if state.phase == Phase::Showdown { state.showdown(); }
                            break;
                        }
                    } else {
                        break;
                    }
                }
            }
        }

        total_profit += state.stacks[0] as i64 - initial_stack_0;
        stats.hands += 1;
    }

    (total_profit, stats)
}

// ── bot match (blueprint + search) ─────────────────────────

fn play_match_bot(
    bot: &mut PokerBot,
    opp_type: OpponentType,
    opp_strategy: &Strategy,
    num_hands: u32,
    rng: &mut Rng,
) -> (i64, MatchStats) {
    let rules = Rules {
        buyin: 1000, small_blind: 5, big_blind: 10,
        turn_timeout_blocks: 6, rake_bps: 0, rake_cap: 0,
    };
    let mut total_profit: i64 = 0;
    let mut stats = MatchStats { hands: 0, vpip: 0, pfr: 0, aggressive: 0, passive: 0, cfr_hits: 0, cfr_misses: 0 };

    for _ in 0..num_hands {
        let mut state = GameState::new(rules, 2);
        let deck = shuffle_deck(rng);
        let cards = [[deck[0], deck[1]], [deck[2], deck[3]]];
        let community = [deck[4], deck[5], deck[6], deck[7], deck[8]];
        state.deal(&cards, community);

        let initial_stack_0 = rules.buyin as i64;
        let mut history: Vec<u8> = Vec::new();
        let mut actions_count = 0u32;

        loop {
            if matches!(state.phase, Phase::Showdown | Phase::Settled) { break; }
            if actions_count > 200 { break; }

            let seat = state.acting_seat as usize;
            let hole = &cards[seat];

            let action = if seat == 0 {
                // Bot (blueprint + search)
                let result = bot.decide(&state, hole, &community, &history);
                if let Some((act, amt)) = bot.sample_action(&result) {
                    if result.from_blueprint { stats.cfr_hits += 1; } else { stats.cfr_misses += 1; }
                    SignedAction { seat: state.acting_seat, action: act, amount: amt, seq: 0, sig: [0; 64] }
                } else {
                    SignedAction { seat: state.acting_seat, action: Action::Fold, amount: 0, seq: 0, sig: [0; 64] }
                }
            } else {
                match opp_type {
                    OpponentType::CfrStrategy => {
                        cfr_action(&state, hole, &community, opp_strategy, &history, rng)
                    }
                    _ => opponent_action(opp_type, &state, hole, &Strategy::new(), &history, rng),
                }
            };

            if seat == 0 {
                match action.action {
                    Action::Bet | Action::Raise | Action::AllIn => {
                        stats.aggressive += 1;
                        if state.phase == Phase::Preflop && actions_count < 4 {
                            stats.vpip += 1; stats.pfr += 1;
                        }
                    }
                    Action::Call => {
                        stats.passive += 1;
                        if state.phase == Phase::Preflop && actions_count < 4 { stats.vpip += 1; }
                    }
                    Action::Check => { stats.passive += 1; }
                    _ => {}
                }
            }

            let pot = state.pot;
            let stack = state.stacks[seat];
            let abs = abstract_action(action.action, action.amount, pot, stack);
            history.push(abs);

            match state.apply(&action) {
                Ok(result) => {
                    actions_count += 1;
                    if result.hand_over {
                        if state.phase == Phase::Showdown { state.showdown(); }
                        break;
                    }
                }
                Err(_) => {
                    let fold = SignedAction { seat: action.seat, action: Action::Fold, amount: 0, seq: 0, sig: [0; 64] };
                    if let Ok(r) = state.apply(&fold) {
                        if r.hand_over {
                            if state.phase == Phase::Showdown { state.showdown(); }
                            break;
                        }
                    } else { break; }
                }
            }
        }

        total_profit += state.stacks[0] as i64 - initial_stack_0;
        stats.hands += 1;
    }

    (total_profit, stats)
}

// ── main ───────────────────────────────────────────────────

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let strat_path = args.get(1).expect("usage: cfr-bench <strategy.bin> [num_hands] [--vs strategy_b.bin] [--bot]");

    let mut num_hands: u32 = 50000;
    let mut vs_path: Option<&str> = None;
    let mut use_bot = false;

    let mut i = 2;
    while i < args.len() {
        if args[i] == "--vs" && i + 1 < args.len() {
            vs_path = Some(&args[i + 1]);
            i += 2;
        } else if args[i] == "--bot" {
            use_bot = true;
            i += 1;
        } else if let Ok(n) = args[i].parse::<u32>() {
            num_hands = n;
            i += 1;
        } else {
            i += 1;
        }
    }

    println!("Loading strategy: {}", strat_path);
    let data = std::fs::read(strat_path).expect("failed to read strategy");
    let strategy = import_strategy(&data);
    println!("  {} info sets", strategy.len());

    let opp_strategy = if let Some(p) = vs_path {
        println!("Loading opponent: {}", p);
        let data = std::fs::read(p).expect("failed to read opponent strategy");
        let s = import_strategy(&data);
        println!("  {} info sets", s.len());
        s
    } else {
        Strategy::new()
    };

    let bb: f64 = 10.0;

    if use_bot {
        println!("\n=== Bot Mode (Blueprint + Real-Time Search) ===");
        let mut bot = PokerBot::new(SearchConfig {
            blueprint: strategy.clone(),
            max_depth: 2,
            iterations: 30, // fast for bench, increase for real play
            ..Default::default()
        });

        let opponents = [
            ("random", OpponentType::Random),
            ("calling_station", OpponentType::CallingStation),
            ("rock", OpponentType::Rock),
            ("tag", OpponentType::Tag),
            ("lag", OpponentType::Lag),
            ("maniac", OpponentType::Maniac),
        ];

        println!("{:<20} {:>10} {:>10} {:>8} {:>8} {:>8} {:>10}", "Opponent", "bb/100", "Result", "VPIP", "PFR", "AF", "BP/Search");
        println!("{}", "-".repeat(78));

        let start = Instant::now();
        for (name, otype) in &opponents {
            let mut rng = Rng::new(0xCAFEBABE);
            let (profit, stats) = play_match_bot(&mut bot, *otype, &Strategy::new(), num_hands, &mut rng);

            let bb100 = (profit as f64 / bb) / (stats.hands as f64 / 100.0);
            let vpip = stats.vpip as f64 / stats.hands as f64 * 100.0;
            let pfr = stats.pfr as f64 / stats.hands as f64 * 100.0;
            let af = stats.aggressive as f64 / stats.passive.max(1) as f64;
            let result = if profit > 0 { "WIN" } else if profit < 0 { "LOSE" } else { "DRAW" };
            let bp_pct = stats.cfr_hits as f64 / (stats.cfr_hits + stats.cfr_misses).max(1) as f64 * 100.0;

            println!("{:<20} {:>+10.1} {:>10} {:>7.1}% {:>7.1}% {:>8.2} {:>9.0}%",
                name, bb100, result, vpip, pfr, af, bp_pct);
        }

        let elapsed = start.elapsed();
        println!("\n{:.1}s total", elapsed.as_secs_f64());
    } else if let Some(_) = vs_path {
        println!("\n{:<20} {:>10} {:>10} {:>8} {:>8} {:>8}", "Matchup", "bb/100", "Result", "VPIP", "PFR", "AF");
        println!("{}", "-".repeat(64));

        let start = Instant::now();
        let mut rng = Rng::new(0xCAFEBABE);
        let (profit, stats) = play_match(&strategy, OpponentType::CfrStrategy, &opp_strategy, num_hands, &mut rng);
        let elapsed = start.elapsed();

        let bb100 = (profit as f64 / bb) / (stats.hands as f64 / 100.0);
        let vpip = stats.vpip as f64 / stats.hands as f64 * 100.0;
        let pfr = stats.pfr as f64 / stats.hands as f64 * 100.0;
        let af = stats.aggressive as f64 / stats.passive.max(1) as f64;
        let result = if profit > 0 { "WIN" } else if profit < 0 { "LOSE" } else { "DRAW" };

        println!("{:<20} {:>+10.1} {:>10} {:>7.1}% {:>7.1}% {:>8.2}",
            "cfr_vs_cfr", bb100, result, vpip, pfr, af);
        println!("\n{} hands in {:.1}s ({:.0} hands/sec)", stats.hands, elapsed.as_secs_f64(),
            stats.hands as f64 / elapsed.as_secs_f64());
    } else {
        let opponents = [
            ("random", OpponentType::Random),
            ("calling_station", OpponentType::CallingStation),
            ("rock", OpponentType::Rock),
            ("tag", OpponentType::Tag),
            ("lag", OpponentType::Lag),
            ("maniac", OpponentType::Maniac),
        ];

        println!("\n{:<20} {:>10} {:>10} {:>8} {:>8} {:>8}", "Opponent", "bb/100", "Result", "VPIP", "PFR", "AF");
        println!("{}", "-".repeat(64));

        let start = Instant::now();

        for (name, otype) in &opponents {
            let mut rng = Rng::new(0xCAFEBABE);
            let (profit, stats) = play_match(&strategy, *otype, &Strategy::new(), num_hands, &mut rng);

            let bb100 = (profit as f64 / bb) / (stats.hands as f64 / 100.0);
            let vpip = stats.vpip as f64 / stats.hands as f64 * 100.0;
            let pfr = stats.pfr as f64 / stats.hands as f64 * 100.0;
            let af = stats.aggressive as f64 / stats.passive.max(1) as f64;
            let result = if profit > 0 { "WIN" } else if profit < 0 { "LOSE" } else { "DRAW" };

            println!("{:<20} {:>+10.1} {:>10} {:>7.1}% {:>7.1}% {:>8.2}",
                name, bb100, result, vpip, pfr, af);
        }

        let elapsed = start.elapsed();
        let total_hands = num_hands as u64 * opponents.len() as u64;
        println!("\n{} total hands in {:.1}s ({:.0} hands/sec)",
            total_hands, elapsed.as_secs_f64(), total_hands as f64 / elapsed.as_secs_f64());

        // variance estimate
        println!("\n--- Variance (vs random, 20 runs × {} hands) ---", num_hands);
        let mut results = Vec::new();
        for seed in 0..20u64 {
            let mut rng = Rng::new(seed * 7919 + 1);
            let (profit, stats) = play_match(&strategy, OpponentType::Random, &Strategy::new(), num_hands, &mut rng);
            let bb100 = (profit as f64 / bb) / (stats.hands as f64 / 100.0);
            results.push(bb100);
        }
        let mean: f64 = results.iter().sum::<f64>() / results.len() as f64;
        let variance: f64 = results.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / results.len() as f64;
        let std = variance.sqrt();
        let min = results.iter().cloned().fold(f64::MAX, f64::min);
        let max = results.iter().cloned().fold(f64::MIN, f64::max);
        let ci = 1.96 * std / (results.len() as f64).sqrt();
        println!("  mean: {:+.1} bb/100", mean);
        println!("  std:  {:.1} bb/100", std);
        println!("  range: [{:+.1}, {:+.1}]", min, max);
        println!("  95% CI: [{:+.1}, {:+.1}]", mean - ci, mean + ci);
    }
}
