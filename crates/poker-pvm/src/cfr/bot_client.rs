//! cfr-bot: standalone poker bot that connects via WebSocket.
//!
//! Connects to the relay server as a regular player.
//! No special access — uses the same protocol as the browser client.
//!
//! Modes:
//!   Single table:  cfr-bot --server ws://host/room/ws --strategy strategy.bin
//!   Multi table:   cfr-bot --lobby ws://host/ws/lobby --strategy strategy.bin --max-tables 4
//!   Named:         cfr-bot --server ws://host/room/ws --name "alice"

use std::sync::Arc;
use std::time::Duration;

/// shared config across all bot threads
struct BotConfig {
    strategy: Arc<Vec<u8>>,
    #[cfg(feature = "onnx")]
    moe: Option<Arc<poker_pvm::cfr::inference::OnnxMoE>>,
    #[cfg(feature = "onnx")]
    moe_weight: f32,
    base_name: String,
}

impl BotConfig {
    fn make_brain(&self) -> poker_pvm::cfr::brain::Brain {
        use poker_pvm::cfr::brain::*;
        #[cfg(feature = "onnx")]
        if let Some(ref moe) = self.moe {
            return Brain::full_with_neural(&self.strategy, moe.clone(), self.moe_weight);
        }
        Brain::full_stack(&self.strategy)
    }
}

/// run one bot on one table — blocking, runs until disconnect
fn run_bot(config: &BotConfig, server_url: &str, bot_name: &str) {
    let mut brain = config.make_brain();

    // connect
    let (mut socket, _) = match tungstenite::connect(server_url) {
        Ok(s) => s,
        Err(e) => { eprintln!("[{}] connect failed: {}", bot_name, e); return; }
    };
    println!("[{}] connected to {}", bot_name, server_url);

    // join
    let join = serde_json::json!({"type": "Join", "name": bot_name});
    socket.send(tungstenite::Message::Text(join.to_string())).unwrap();

    // state
    let mut my_seat: Option<u8> = None;
    let mut my_cards: Option<[u8; 2]> = None;
    let mut community_cards = [0u8; 5];
    let mut hand_number = 0u32;
    let mut button = 0u8;
    let mut stacks = vec![0u64; 10];
    let mut bets = vec![0u64; 10];
    let mut pot = 0u64;

    let mut rng_state: u64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64;
    // xorshift64 → u32 → [0, 1) — must cast to u32 before dividing or sleeps balloon to hours
    let mut rng_f = move || -> f64 {
        rng_state ^= rng_state << 13;
        rng_state ^= rng_state >> 7;
        rng_state ^= rng_state << 17;
        ((rng_state >> 16) as u32) as f64 / u32::MAX as f64
    };

    loop {
        let msg = match socket.read() {
            Ok(tungstenite::Message::Text(t)) => t.to_string(),
            Ok(tungstenite::Message::Close(_)) => { println!("disconnected"); break; }
            Ok(_) => continue,
            Err(e) => { eprintln!("error: {}", e); break; }
        };

        // debug: print all messages
        eprintln!("[recv] {}", &msg[..msg.len().min(200)]);

        let v: serde_json::Value = match serde_json::from_str(&msg) {
            Ok(v) => v,
            Err(_) => continue,
        };

        match v["type"].as_str().unwrap_or("") {
            "Seated" => {
                my_seat = Some(v["seat"].as_u64().unwrap_or(0) as u8);
                println!("[seated] seat {}", my_seat.unwrap());
            }
            "Waiting" => { println!("[waiting] for opponent"); }
            "HandStarted" => {
                hand_number = v["hand_number"].as_u64().unwrap_or(0) as u32;
                button = v["button"].as_u64().unwrap_or(0) as u8;
                if let Some(s) = v["stacks"].as_array() {
                    stacks = s.iter().map(|x| x.as_u64().unwrap_or(0)).collect();
                }
                bets = vec![0; stacks.len()];
                pot = 0;
                community_cards = [0; 5];

                if let Some(cards) = v.get("your_cards").and_then(|c| c.as_array()) {
                    if cards.len() == 2 {
                        my_cards = Some([parse_card(&cards[0]), parse_card(&cards[1])]);
                    }
                }

                // build GameState for brain
                let gs = build_game_state(&stacks, &bets, pot, &community_cards, 0,
                    poker_pvm::Phase::Preflop, my_seat.unwrap_or(0), 2, hand_number, button);
                brain.new_hand(&gs);
                if let Some(cards) = my_cards {
                    brain.set_hero_cards(my_seat.unwrap_or(0), cards, &gs);
                }

                println!("[hand #{}] cards: {:?}", hand_number, my_cards);
            }
            "BlindsPosted" => {
                // update bets/pot from blinds
                if let Some(sb) = v.get("small_blind").and_then(|x| x.as_array()) {
                    if sb.len() == 2 {
                        let seat = sb[0].as_u64().unwrap_or(0) as usize;
                        let amt = sb[1].as_u64().unwrap_or(0);
                        if seat < bets.len() { bets[seat] = amt; }
                        pot += amt;
                    }
                }
                if let Some(bb) = v.get("big_blind").and_then(|x| x.as_array()) {
                    if bb.len() == 2 {
                        let seat = bb[0].as_u64().unwrap_or(0) as usize;
                        let amt = bb[1].as_u64().unwrap_or(0);
                        if seat < bets.len() { bets[seat] = amt; }
                        pot += amt;
                    }
                }
            }
            "CommunityCards" => {
                if let Some(cards) = v.get("cards").and_then(|c| c.as_array()) {
                    let old_count = community_cards.iter().filter(|&&c| c > 0 || cards.len() > 0).count();
                    for (i, c) in cards.iter().enumerate().take(5) {
                        community_cards[i] = parse_card(c);
                    }
                    let phase_str = v["phase"].as_str().unwrap_or("?");
                    let new_cards: Vec<u8> = cards.iter().skip(old_count.min(cards.len())).map(|c| parse_card(c)).collect();
                    if !new_cards.is_empty() {
                        let gs = build_game_state(&stacks, &bets, pot, &community_cards,
                            cards.len() as u8, phase_from_str(phase_str),
                            my_seat.unwrap_or(0), 2, hand_number, button);
                        brain.reveal_community(&new_cards, &gs);
                    }
                    println!("[{}] {:?}", phase_str, community_cards.iter().take(cards.len()).collect::<Vec<_>>());
                }
                bets = vec![0; stacks.len()]; // reset bets on new street
            }
            "PlayerActed" => {
                let seat = v["seat"].as_u64().unwrap_or(255) as u8;
                let action_str = v["action"].as_str().unwrap_or("?");
                let amount = v["amount"].as_u64().unwrap_or(0);

                if Some(seat) != my_seat {
                    // observe opponent action
                    let pvm_action = action_from_str(action_str);
                    let cc = community_cards.iter().filter(|&&c| c > 0).count() as u8;
                    let gs = build_game_state(&stacks, &bets, pot, &community_cards,
                        cc, phase_from_count(cc), my_seat.unwrap_or(0), 2, hand_number, button);
                    brain.observe_action(seat, pvm_action, amount as u32, &gs);
                    println!("[opp] {} {}", action_str, if amount > 0 { format!("{}", amount) } else { String::new() });
                }

                // update state
                if seat < bets.len() as u8 {
                    bets[seat as usize] += amount;
                    if seat < stacks.len() as u8 {
                        stacks[seat as usize] = v["new_stack"].as_u64().unwrap_or(stacks[seat as usize]);
                    }
                }
            }
            "PotUpdate" => {
                if let Some(pots) = v.get("pots").and_then(|p| p.as_array()) {
                    pot = pots.iter().map(|p| p["amount"].as_u64().unwrap_or(0)).sum();
                }
            }
            "ActionRequired" => {
                let seat = v["seat"].as_u64().unwrap_or(255) as u8;
                if my_seat != Some(seat) { continue; }

                let think_ms = 1000 + (rng_f() * 7000.0) as u64;
                std::thread::sleep(Duration::from_millis(think_ms));

                let cc = community_cards.iter().filter(|&&c| c > 0).count() as u8;
                let gs = build_game_state(&stacks, &bets, pot, &community_cards,
                    cc, phase_from_count(cc), seat, 2, hand_number, button);
                let cards = my_cards.unwrap_or([0, 0]);
                let decision = brain.decide(&gs, &cards, &community_cards);

                let (mut action, mut amount) = decision.sample(rng_f())
                    .unwrap_or((poker_pvm::Action::Fold, 0));

                let valid = v.get("valid_actions").and_then(|a| a.as_array());
                // brain may suggest an action the server doesn't accept (e.g. check preflop as SB);
                // coerce to a legal one: call > check > fold
                if !action_is_valid(action, valid) {
                    if has_valid(valid, "call") { action = poker_pvm::Action::Call; amount = 0; }
                    else if has_valid(valid, "check") { action = poker_pvm::Action::Check; amount = 0; }
                    else { action = poker_pvm::Action::Fold; amount = 0; }
                }

                let action_json = to_server_action(action, amount, valid);
                println!("[act] {} (thought {}ms)", action_json, think_ms);
                let _ = socket.send(tungstenite::Message::Text(action_json.into()));
            }
            "PotAwarded" => {
                let seat = v["seat"].as_u64().unwrap_or(255) as u8;
                let amount = v["amount"].as_u64().unwrap_or(0);
                let won = my_seat == Some(seat);
                println!("[{}] {}", if won { "WIN" } else { "LOSE" }, amount);
            }
            "Showdown" => { println!("[showdown]"); }
            "HandComplete" => {
                if let Some(s) = v["stacks"].as_array() {
                    stacks = s.iter().map(|x| x.as_u64().unwrap_or(0)).collect();
                }
                println!("[complete] stacks: {:?}", &stacks[..2]);
            }
            "Error" => { eprintln!("[error] {}", v["message"].as_str().unwrap_or("?")); }
            "Chat" => {
                let from = v["name"].as_str().unwrap_or("?");
                let text = v["text"].as_str().unwrap_or("");
                println!("[chat] {}: {}", from, text);
            }
            _ => {}
        }
    }
}

fn parse_card(v: &serde_json::Value) -> u8 {
    let r = match v.get("rank").and_then(|r| r.as_str()).unwrap_or("2") {
        "2" => 0, "3" => 1, "4" => 2, "5" => 3, "6" => 4, "7" => 5,
        "8" => 6, "9" => 7, "10" | "T" => 8, "J" => 9, "Q" => 10, "K" => 11, "A" => 12,
        _ => 0,
    };
    let s = match v.get("suit").and_then(|s| s.as_str()).unwrap_or("c") {
        "c" | "clubs" => 0, "d" | "diamonds" => 1, "h" | "hearts" => 2, "s" | "spades" => 3,
        _ => 0,
    };
    r + s * 13
}

fn action_from_str(s: &str) -> poker_pvm::Action {
    match s {
        "fold" => poker_pvm::Action::Fold,
        "check" => poker_pvm::Action::Check,
        "call" => poker_pvm::Action::Call,
        "bet" => poker_pvm::Action::Bet,
        "raise" => poker_pvm::Action::Raise,
        "allin" => poker_pvm::Action::AllIn,
        _ => poker_pvm::Action::Check,
    }
}

fn phase_from_str(s: &str) -> poker_pvm::Phase {
    match s {
        "preflop" => poker_pvm::Phase::Preflop,
        "flop" => poker_pvm::Phase::Flop,
        "turn" => poker_pvm::Phase::Turn,
        "river" => poker_pvm::Phase::River,
        _ => poker_pvm::Phase::Preflop,
    }
}

fn phase_from_count(cc: u8) -> poker_pvm::Phase {
    match cc {
        0 => poker_pvm::Phase::Preflop,
        3 => poker_pvm::Phase::Flop,
        4 => poker_pvm::Phase::Turn,
        5 => poker_pvm::Phase::River,
        _ => poker_pvm::Phase::Preflop,
    }
}

fn build_game_state(
    stacks: &[u64], bets: &[u64], pot: u64, community: &[u8; 5],
    cc: u8, phase: poker_pvm::Phase, acting: u8, num_players: u8,
    hand_number: u32, button: u8,
) -> poker_pvm::GameState {
    let mut gs_stacks = [0u32; poker_pvm::MAX_SEATS];
    let mut gs_bets = [0u32; poker_pvm::MAX_SEATS];
    let mut seat_state = [poker_pvm::SeatState::Empty; poker_pvm::MAX_SEATS];
    for (i, &s) in stacks.iter().enumerate().take(poker_pvm::MAX_SEATS) {
        gs_stacks[i] = s as u32;
        if s > 0 || bets.get(i).copied().unwrap_or(0) > 0 { seat_state[i] = poker_pvm::SeatState::Active; }
    }
    for (i, &b) in bets.iter().enumerate().take(poker_pvm::MAX_SEATS) {
        gs_bets[i] = b as u32;
    }
    poker_pvm::GameState {
        stacks: gs_stacks, bets: gs_bets, pot: pot as u32,
        community: *community, community_count: cc, phase,
        acting_seat: acting, num_players, hand_number, button, seat_state,
        cards: [[0; 2]; poker_pvm::MAX_SEATS],
        round_actions: 0, last_aggressor: 0, action_count: 0,
        last_action_hash: [0; 32], rake: 0,
        rules: poker_pvm::Rules { buyin: 1000, small_blind: 5, big_blind: 10, turn_timeout_blocks: 6, rake_bps: 0, rake_cap: 0 },
    }
}

fn has_valid(valid: Option<&Vec<serde_json::Value>>, kind: &str) -> bool {
    valid.map(|arr| arr.iter().any(|a| a["kind"].as_str() == Some(kind))).unwrap_or(false)
}

fn action_is_valid(action: poker_pvm::Action, valid: Option<&Vec<serde_json::Value>>) -> bool {
    let kind = match action {
        poker_pvm::Action::Fold => "fold",
        poker_pvm::Action::Check => "check",
        poker_pvm::Action::Call => "call",
        poker_pvm::Action::Bet => "bet",
        poker_pvm::Action::Raise => "raise",
        poker_pvm::Action::AllIn => "allin",
    };
    has_valid(valid, kind)
}

fn to_server_action(action: poker_pvm::Action, amount: u32, valid: Option<&Vec<serde_json::Value>>) -> String {
    match action {
        poker_pvm::Action::Fold => r#"{"type":"Action","action":"fold"}"#.to_string(),
        poker_pvm::Action::Check => r#"{"type":"Action","action":"check"}"#.to_string(),
        poker_pvm::Action::Call => r#"{"type":"Action","action":"call"}"#.to_string(),
        poker_pvm::Action::Bet => {
            let min = valid.and_then(|v| v.iter().find(|a| a["kind"].as_str() == Some("bet")))
                .and_then(|a| a["min_amount"].as_u64()).unwrap_or(10);
            let amt = (amount as u64).max(min);
            format!(r#"{{"type":"Action","action":"bet","amount":{}}}"#, amt)
        }
        poker_pvm::Action::Raise => {
            let min = valid.and_then(|v| v.iter().find(|a| a["kind"].as_str() == Some("raise")))
                .and_then(|a| a["min_amount"].as_u64()).unwrap_or(20);
            let amt = (amount as u64).max(min);
            format!(r#"{{"type":"Action","action":"raise","amount":{}}}"#, amt)
        }
        poker_pvm::Action::AllIn => r#"{"type":"Action","action":"allin"}"#.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Main: single table or multi-table mode
// ---------------------------------------------------------------------------

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mut servers: Vec<String> = Vec::new();
    let mut strategy_path = "strategy.bin".to_string();
    let mut base_name = "bot".to_string();
    let mut _moe_dir: Option<String> = None;
    let mut _moe_weight: f32 = 0.3;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--server" => { if i+1 < args.len() { servers.push(args[i+1].clone()); i += 1; } }
            "--strategy" => { if i+1 < args.len() { strategy_path = args[i+1].clone(); i += 1; } }
            "--name" => { if i+1 < args.len() { base_name = args[i+1].clone(); i += 1; } }
            "--moe-dir" => { if i+1 < args.len() { _moe_dir = Some(args[i+1].clone()); i += 1; } }
            "--moe-weight" => { if i+1 < args.len() { _moe_weight = args[i+1].parse().unwrap_or(0.3); i += 1; } }
            _ => {}
        }
        i += 1;
    }

    let strategy = std::fs::read(&strategy_path)
        .unwrap_or_else(|e| { eprintln!("failed to load {}: {}", strategy_path, e); std::process::exit(1); });

    println!("cfr-bot v9");
    println!("strategy: {} ({} bytes)", strategy_path, strategy.len());
    println!("tables: {}", servers.len());

    let config = Arc::new(BotConfig {
        strategy: Arc::new(strategy),
        #[cfg(feature = "onnx")]
        moe: _moe_dir.as_ref().and_then(|dir| {
            use poker_pvm::cfr::inference::OnnxMoE;
            match OnnxMoE::load(dir) {
                Ok(m) => { println!("MoE: {} (weight: {})", dir, _moe_weight); Some(Arc::new(m)) }
                Err(e) => { eprintln!("MoE failed: {}", e); None }
            }
        }),
        #[cfg(feature = "onnx")]
        moe_weight: _moe_weight,
        base_name: base_name.clone(),
    });

    if servers.is_empty() {
        eprintln!("usage: cfr-bot --server ws://host/room/ws --strategy strategy.bin");
        eprintln!("       cfr-bot --server ws://host/room1/ws --server ws://host/room2/ws");
        std::process::exit(1);
    }

    if servers.len() == 1 {
        // single table — run in main thread
        run_bot(&config, &servers[0], &base_name);
    } else {
        // multi-table — one thread per table
        println!("multi-table: {} tables", servers.len());
        let mut handles = Vec::new();
        for (idx, url) in servers.iter().enumerate() {
            let config = config.clone();
            let url = url.clone();
            let name = format!("{}_{}", base_name, idx);
            handles.push(std::thread::spawn(move || {
                run_bot(&config, &url, &name);
            }));
        }
        for h in handles {
            let _ = h.join();
        }
    }
}
