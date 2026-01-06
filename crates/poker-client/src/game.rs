//! poker game logic for test/offline mode
//!
//! heads-up game with configurable bot opponent

use bevy::prelude::*;
use zk_shuffle::poker::{Card, Rank, Suit, HandCategory, evaluate_best_hand, showdown_holdem};

use crate::ui::{GamePhase, GameState, PlayerInfo};

pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.add_event::<GameAction>()
            .add_event::<BotThink>()
            .init_resource::<TestDeck>()
            .add_systems(Update, (
                handle_game_actions,
                bot_think_system,
                advance_phase_system,
            ).chain());
    }
}

/// game action events from UI
#[derive(Event, Clone, Debug)]
pub enum GameAction {
    Fold,
    Check,
    Call,
    Raise(u32),
    AllIn,
    /// start new hand
    NewHand,
}

/// trigger bot to think
#[derive(Event)]
pub struct BotThink;

/// bot strategy
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BotStrategy {
    /// always calls, never folds or raises
    #[default]
    CallStation,
    /// folds everything except premium hands
    Rock,
    /// raises everything
    Maniac,
    /// random actions
    Fish,
}

/// shuffled deck for test mode
#[derive(Resource)]
pub struct TestDeck {
    pub cards: Vec<Card>,
    pub index: usize,
}

impl Default for TestDeck {
    fn default() -> Self {
        let mut deck = Self {
            cards: Vec::new(),
            index: 0,
        };
        deck.shuffle();
        deck
    }
}

/// get timestamp seed (works on both native and wasm)
fn get_timestamp_seed() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        // use performance.now() in wasm, with fallback
        if let Some(window) = web_sys::window() {
            if let Some(performance) = window.performance() {
                return (performance.now() * 1000000.0) as u64;
            }
        }
        // fallback: use a simple counter
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(12345);
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64
    }
}

impl TestDeck {
    pub fn shuffle(&mut self) {
        // generate standard 52 card deck using zk_shuffle
        self.cards = Card::standard_deck();

        // get seed from platform-appropriate source
        let seed = get_timestamp_seed();

        // fisher-yates shuffle
        let n = self.cards.len();
        let mut state = seed;
        for i in (1..n).rev() {
            // simple lcg prng
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let j = (state as usize) % (i + 1);
            self.cards.swap(i, j);
        }

        self.index = 0;
    }

    pub fn deal(&mut self) -> Option<Card> {
        if self.index < self.cards.len() {
            let card = self.cards[self.index];
            self.index += 1;
            Some(card)
        } else {
            None
        }
    }

    pub fn deal_two(&mut self) -> Option<[Card; 2]> {
        let c1 = self.deal()?;
        let c2 = self.deal()?;
        Some([c1, c2])
    }
}

/// handle player actions
fn handle_game_actions(
    mut actions: EventReader<GameAction>,
    mut game_state: ResMut<GameState>,
    mut bot_think: EventWriter<BotThink>,
    mut deck: ResMut<TestDeck>,
) {
    for action in actions.read() {
        // only process if it's player's turn
        if game_state.active_seat != game_state.local_player_seat {
            continue;
        }

        let player_idx = game_state.players.iter()
            .position(|p| p.seat_index == game_state.local_player_seat);

        let Some(player_idx) = player_idx else { continue };

        match action {
            GameAction::Fold => {
                info!("player folds");
                game_state.players[player_idx].is_folded = true;
                // bot wins pot
                let bot_idx = game_state.players.iter()
                    .position(|p| p.seat_index != game_state.local_player_seat);
                if let Some(bot_idx) = bot_idx {
                    game_state.players[bot_idx].chips += game_state.pot;
                }
                game_state.pot = 0;
                game_state.phase = GamePhase::Showdown;
            }
            GameAction::Check => {
                info!("player checks");
                // switch to bot
                switch_to_bot(&mut game_state, &mut bot_think);
            }
            GameAction::Call => {
                let call_amount = game_state.current_bet.saturating_sub(
                    game_state.players[player_idx].current_bet
                );
                let actual_call = call_amount.min(game_state.players[player_idx].chips);

                info!("player calls ${}", actual_call);
                game_state.players[player_idx].chips -= actual_call;
                game_state.players[player_idx].current_bet += actual_call;
                game_state.pot += actual_call;

                // switch to bot or advance phase
                if all_bets_equal(&game_state) && both_acted(&game_state) {
                    advance_street(&mut game_state, &mut deck);
                } else {
                    switch_to_bot(&mut game_state, &mut bot_think);
                }
            }
            GameAction::Raise(amount) => {
                let raise_total = *amount;
                let current_player_bet = game_state.players[player_idx].current_bet;
                let player_chips = game_state.players[player_idx].chips;
                let to_add = raise_total.saturating_sub(current_player_bet);
                let actual_add = to_add.min(player_chips);

                info!("player raises to ${}", current_player_bet + actual_add);
                game_state.players[player_idx].chips -= actual_add;
                game_state.players[player_idx].current_bet += actual_add;
                game_state.pot += actual_add;
                game_state.current_bet = game_state.players[player_idx].current_bet;

                // switch to bot
                switch_to_bot(&mut game_state, &mut bot_think);
            }
            GameAction::AllIn => {
                let all_in_amount = game_state.players[player_idx].chips;

                info!("player goes all-in for ${}", all_in_amount);
                game_state.players[player_idx].current_bet += all_in_amount;
                game_state.pot += all_in_amount;
                game_state.players[player_idx].chips = 0;

                let new_bet = game_state.players[player_idx].current_bet;
                if new_bet > game_state.current_bet {
                    game_state.current_bet = new_bet;
                }

                // switch to bot
                switch_to_bot(&mut game_state, &mut bot_think);
            }
            GameAction::NewHand => {
                start_new_hand(&mut game_state, &mut deck);
            }
        }
    }
}

/// bot thinking system (call station: always calls)
fn bot_think_system(
    mut think_events: EventReader<BotThink>,
    mut game_state: ResMut<GameState>,
    mut deck: ResMut<TestDeck>,
) {
    for _ in think_events.read() {
        // find bot
        let bot_idx = game_state.players.iter()
            .position(|p| p.seat_index != game_state.local_player_seat);

        let Some(bot_idx) = bot_idx else { continue };

        // only act if it's bot's turn
        if game_state.players[bot_idx].seat_index != game_state.active_seat {
            continue;
        }

        // call station: always calls
        let call_amount = game_state.current_bet.saturating_sub(
            game_state.players[bot_idx].current_bet
        );

        if call_amount == 0 {
            // check
            info!("bot checks");
        } else {
            // call
            let actual_call = call_amount.min(game_state.players[bot_idx].chips);
            info!("bot calls ${}", actual_call);
            game_state.players[bot_idx].chips -= actual_call;
            game_state.players[bot_idx].current_bet += actual_call;
            game_state.pot += actual_call;
        }

        // switch back to player or advance phase
        if all_bets_equal(&game_state) && both_acted(&game_state) {
            advance_street(&mut game_state, &mut deck);
        } else {
            game_state.active_seat = game_state.local_player_seat;
        }
    }
}

/// advance phase after betting round complete
fn advance_phase_system(
    // this runs automatically to check if we should advance
) {
    // phase advancement handled in other systems
}

fn switch_to_bot(game_state: &mut GameState, bot_think: &mut EventWriter<BotThink>) {
    let bot_seat = game_state.players.iter()
        .find(|p| p.seat_index != game_state.local_player_seat)
        .map(|p| p.seat_index);

    if let Some(seat) = bot_seat {
        game_state.active_seat = seat;
        // trigger bot to think after small delay (will be instant for now)
        bot_think.send(BotThink);
    }
}

fn all_bets_equal(game_state: &GameState) -> bool {
    let active_players: Vec<_> = game_state.players.iter()
        .filter(|p| !p.is_folded && p.chips > 0)
        .collect();

    if active_players.len() <= 1 {
        return true;
    }

    let first_bet = active_players[0].current_bet;
    active_players.iter().all(|p| p.current_bet == first_bet || p.chips == 0)
}

fn both_acted(game_state: &GameState) -> bool {
    // for heads-up, both have acted if bets are equal and > 0 or both checked
    let active_players: Vec<_> = game_state.players.iter()
        .filter(|p| !p.is_folded)
        .collect();

    active_players.len() == 2
}

fn advance_street(game_state: &mut GameState, deck: &mut TestDeck) {
    // reset current bets
    for player in &mut game_state.players {
        player.current_bet = 0;
    }
    game_state.current_bet = 0;

    match game_state.phase {
        GamePhase::PreFlop => {
            // deal flop
            game_state.phase = GamePhase::Flop;
            for _ in 0..3 {
                if let Some(card) = deck.deal() {
                    game_state.community_cards.push(card);
                }
            }
            info!("dealing flop: {:?}", game_state.community_cards);
        }
        GamePhase::Flop => {
            // deal turn
            game_state.phase = GamePhase::Turn;
            if let Some(card) = deck.deal() {
                game_state.community_cards.push(card);
                info!("dealing turn: {}", card);
            }
        }
        GamePhase::Turn => {
            // deal river
            game_state.phase = GamePhase::River;
            if let Some(card) = deck.deal() {
                game_state.community_cards.push(card);
                info!("dealing river: {}", card);
            }
        }
        GamePhase::River => {
            // showdown
            game_state.phase = GamePhase::Showdown;

            // reveal bot cards
            let bot_idx = game_state.players.iter()
                .position(|p| p.seat_index != game_state.local_player_seat);
            if let Some(idx) = bot_idx {
                info!("showdown! bot shows: {:?}", game_state.players[idx].hole_cards);
            }

            // evaluate hands and award pot
            let active_players: Vec<_> = game_state.players.iter()
                .filter(|p| !p.is_folded)
                .collect();

            if active_players.len() == 1 {
                // only one player left - they win
                let winner_seat = active_players[0].seat_index;
                if let Some(winner) = game_state.players.iter_mut().find(|p| p.seat_index == winner_seat) {
                    winner.chips += game_state.pot;
                    info!("{} wins ${} (opponent folded)", winner.name, game_state.pot);
                }
            } else if active_players.len() >= 2 && game_state.community_cards.len() == 5 {
                // proper showdown with hand evaluation
                let community: [Card; 5] = [
                    game_state.community_cards[0],
                    game_state.community_cards[1],
                    game_state.community_cards[2],
                    game_state.community_cards[3],
                    game_state.community_cards[4],
                ];

                let mut hole_cards_for_showdown: Vec<(usize, [Card; 2])> = Vec::new();
                for (idx, player) in game_state.players.iter().enumerate() {
                    if !player.is_folded {
                        if let Some(cards) = player.hole_cards {
                            hole_cards_for_showdown.push((idx, cards));
                        }
                    }
                }

                if hole_cards_for_showdown.len() >= 2 {
                    let result = showdown_holdem(&hole_cards_for_showdown, &community);
                    let pot = game_state.pot;
                    let share = pot / result.winners.len() as u32;

                    for &winner_idx in &result.winners {
                        game_state.players[winner_idx].chips += share;
                    }

                    // get winner names and hand description
                    let winner_names: Vec<_> = result.winners.iter()
                        .map(|&i| game_state.players[i].name.clone())
                        .collect();

                    let hand_desc = match result.winning_category {
                        HandCategory::RoyalFlush => "royal flush",
                        HandCategory::StraightFlush => "straight flush",
                        HandCategory::FourOfAKind => "four of a kind",
                        HandCategory::FullHouse => "full house",
                        HandCategory::Flush => "flush",
                        HandCategory::Straight => "straight",
                        HandCategory::ThreeOfAKind => "three of a kind",
                        HandCategory::TwoPair => "two pair",
                        HandCategory::OnePair => "one pair",
                        HandCategory::HighCard => "high card",
                    };

                    if result.winners.len() == 1 {
                        info!("{} wins ${} with {}", winner_names[0], pot, hand_desc);
                    } else {
                        info!("split pot: {} each win ${} with {}", winner_names.join(" and "), share, hand_desc);
                    }
                } else {
                    // fallback - split pot
                    let half_pot = game_state.pot / 2;
                    for player in &mut game_state.players {
                        if !player.is_folded {
                            player.chips += half_pot;
                        }
                    }
                    info!("pot split (insufficient hole cards for showdown)");
                }
            } else {
                // fallback - split pot
                let half_pot = game_state.pot / 2;
                for player in &mut game_state.players {
                    if !player.is_folded {
                        player.chips += half_pot;
                    }
                }
                info!("pot split (incomplete board)");
            }
            game_state.pot = 0;
        }
        _ => {}
    }

    // first to act after flop is button (seat 0) in heads-up
    // but for simplicity, player acts first
    game_state.active_seat = game_state.local_player_seat;
}

fn start_new_hand(game_state: &mut GameState, deck: &mut TestDeck) {
    info!("starting new hand");

    // shuffle deck
    deck.shuffle();

    // reset game state
    game_state.phase = GamePhase::PreFlop;
    game_state.community_cards.clear();
    game_state.pot = 0;
    game_state.current_bet = game_state.big_blind;

    // reset players
    for player in &mut game_state.players {
        player.is_folded = false;
        player.current_bet = 0;
        player.hole_cards = None;
    }

    // deal hole cards
    let player_idx = game_state.players.iter()
        .position(|p| p.seat_index == game_state.local_player_seat);
    let bot_idx = game_state.players.iter()
        .position(|p| p.seat_index != game_state.local_player_seat);

    if let Some(idx) = player_idx {
        game_state.players[idx].hole_cards = deck.deal_two();
        info!("dealt player: {:?}", game_state.players[idx].hole_cards);
    }
    if let Some(idx) = bot_idx {
        game_state.players[idx].hole_cards = deck.deal_two();
        info!("dealt bot: {:?}", game_state.players[idx].hole_cards);
    }

    // post blinds (simplified: button posts SB, other posts BB)
    // in heads-up: button=SB acts first preflop, BB acts first postflop
    if let Some(idx) = player_idx {
        game_state.players[idx].current_bet = game_state.small_blind;
        game_state.players[idx].chips -= game_state.small_blind;
        game_state.pot += game_state.small_blind;
    }
    if let Some(idx) = bot_idx {
        game_state.players[idx].current_bet = game_state.big_blind;
        game_state.players[idx].chips -= game_state.big_blind;
        game_state.pot += game_state.big_blind;
    }

    // player (button/SB) acts first preflop in heads-up
    game_state.active_seat = game_state.local_player_seat;
    game_state.show_betting_controls = true;

    // rotate dealer
    game_state.dealer_seat = game_state.local_player_seat;
}

/// initialize heads-up test game
pub fn setup_headsup_test(game_state: &mut GameState) {
    game_state.players = vec![
        PlayerInfo {
            name: "You".to_string(),
            chips: 1000,
            seat_index: 0,
            ..default()
        },
        PlayerInfo {
            name: "CallBot".to_string(),
            chips: 1000,
            seat_index: 1,
            ..default()
        },
    ];

    game_state.phase = GamePhase::Lobby;
    game_state.local_player_seat = 0;
    game_state.dealer_seat = 0;
    game_state.small_blind = 5;
    game_state.big_blind = 10;
    game_state.pot = 0;
    game_state.current_bet = 0;
    game_state.community_cards.clear();
    game_state.show_betting_controls = false;
}
