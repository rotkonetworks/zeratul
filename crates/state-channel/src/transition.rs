//! state transitions for poker
//!
//! all valid game actions that can be applied to state

use alloc::vec::Vec;
use scale_codec::{Decode, Encode};
use scale_info::TypeInfo;

use crate::state::*;
use crate::types::*;

/// poker actions that can be taken
#[derive(Clone, Debug, Encode, Decode, TypeInfo, PartialEq, Eq)]
pub enum Action {
    /// start a new hand (from lobby or after complete)
    StartHand,

    /// submit shuffle proof for your encryption layer
    SubmitShuffle {
        /// commitment to encrypted deck after your shuffle
        commitment: ShuffleCommitment,
        /// ligerito proof of valid permutation
        proof: LigeritoProof,
    },

    /// post small blind
    PostSmallBlind,

    /// post big blind
    PostBigBlind,

    /// fold hand
    Fold,

    /// check (bet 0 when no bet to call)
    Check,

    /// call current bet
    Call,

    /// raise by amount (total bet = current_bet + raise_amount)
    Raise { amount: Balance },

    /// go all-in
    AllIn,

    /// reveal hole cards for showdown
    RevealCards {
        /// tokens to decrypt your hole cards
        tokens: Vec<RevealToken>,
    },

    /// submit reveal tokens for community cards
    RevealCommunity {
        /// tokens for the cards being revealed
        tokens: Vec<RevealToken>,
    },

    /// timeout claim - player took too long
    ClaimTimeout { seat: Seat },
}

/// apply action to state, returning result
pub fn apply_action(
    state: &mut PokerState,
    actor: Seat,
    action: Action,
    timestamp: u64,
) -> TransitionResult {
    // verify it's the actor's turn (for actions that require it)
    if requires_turn(&action) && state.action_seat != actor {
        return TransitionResult::Invalid(TransitionError::NotYourTurn);
    }

    match action {
        Action::StartHand => apply_start_hand(state),
        Action::SubmitShuffle { commitment, proof } => {
            apply_submit_shuffle(state, actor, commitment, proof)
        }
        Action::PostSmallBlind => apply_post_blind(state, actor, state.big_blind / 2),
        Action::PostBigBlind => apply_post_blind(state, actor, state.big_blind),
        Action::Fold => apply_fold(state, actor),
        Action::Check => apply_check(state, actor),
        Action::Call => apply_call(state, actor),
        Action::Raise { amount } => apply_raise(state, actor, amount),
        Action::AllIn => apply_all_in(state, actor),
        Action::RevealCards { tokens } => apply_reveal_cards(state, actor, tokens),
        Action::RevealCommunity { tokens } => apply_reveal_community(state, actor, tokens),
        Action::ClaimTimeout { seat } => apply_timeout(state, seat, timestamp),
    }
}

fn requires_turn(action: &Action) -> bool {
    matches!(
        action,
        Action::Fold | Action::Check | Action::Call | Action::Raise { .. } | Action::AllIn
    )
}

fn apply_start_hand(state: &mut PokerState) -> TransitionResult {
    if state.phase != Phase::Lobby && state.phase != Phase::Complete {
        return TransitionResult::Invalid(TransitionError::InvalidPhase);
    }

    // advance hand number
    state.hand_number += 1;

    // move dealer button
    if let Some(next_dealer) = state.next_active_seat(state.dealer_seat) {
        state.dealer_seat = next_dealer;
    }

    // reset state for new hand
    state.phase = Phase::Shuffling;
    state.pot = Pot::default();
    state.side_pots.clear();
    state.community_cards.clear();
    state.shuffle_commitment = ShuffleCommitment::default();
    state.shuffle_proof = None;
    state.reveal_tokens.clear();
    state.current_bet = 0;
    state.min_raise = state.big_blind;

    // reset player states (keep chip counts)
    for player in &mut state.players {
        player.current_bet = 0;
        player.total_bet_this_hand = 0;
        player.is_folded = false;
        player.is_all_in = false;
        player.hole_card_indices = None;
        player.revealed_cards = None;
    }

    // first player to shuffle is dealer
    state.action_seat = state.dealer_seat;

    TransitionResult::Continue
}

fn apply_submit_shuffle(
    state: &mut PokerState,
    actor: Seat,
    commitment: ShuffleCommitment,
    proof: LigeritoProof,
) -> TransitionResult {
    if state.phase != Phase::Shuffling {
        return TransitionResult::Invalid(TransitionError::InvalidPhase);
    }

    // TODO: verify ligerito proof locally
    // In production: ligerito::verify(&proof, &commitment)
    // For now, assume valid (verification happens in dispute)

    // update commitment (each player layer)
    state.shuffle_commitment = commitment;

    // store the latest proof (all proofs should be stored for disputes)
    state.shuffle_proof = Some(proof);

    // next player shuffles
    if let Some(next) = state.next_active_seat(actor) {
        if next == state.dealer_seat {
            // all players have shuffled, deal cards
            return transition_to_preflop(state);
        }
        state.action_seat = next;
    }

    TransitionResult::Continue
}

fn transition_to_preflop(state: &mut PokerState) -> TransitionResult {
    state.phase = Phase::PreFlop;

    // assign hole cards (indices 0-1 to first player, 2-3 to second, etc.)
    let mut card_idx = 0u8;
    for player in &mut state.players {
        player.hole_card_indices = Some([card_idx, card_idx + 1]);
        card_idx += 2;
    }

    // blinds
    let sb_seat = state.small_blind_seat();
    let bb_seat = state.big_blind_seat();
    let sb_amount = state.big_blind / 2;
    let bb_amount = state.big_blind;

    // post small blind
    let sb_idx = state.players.iter().position(|p| p.seat == sb_seat);
    if let Some(idx) = sb_idx {
        let player = &mut state.players[idx];
        let actual = sb_amount.min(player.chips);
        player.chips -= actual;
        player.current_bet = actual;
        player.total_bet_this_hand = actual;
        if player.chips == 0 {
            player.is_all_in = true;
        }
        state.pot.total += actual;
    }

    // post big blind
    let bb_idx = state.players.iter().position(|p| p.seat == bb_seat);
    if let Some(idx) = bb_idx {
        let player = &mut state.players[idx];
        let actual = bb_amount.min(player.chips);
        player.chips -= actual;
        player.current_bet = actual;
        player.total_bet_this_hand = actual;
        if player.chips == 0 {
            player.is_all_in = true;
        }
        state.pot.total += actual;
        state.current_bet = actual;
    }

    // action to player after big blind
    state.action_seat = state.next_active_seat(bb_seat).unwrap_or(sb_seat);

    TransitionResult::Continue
}

fn apply_post_blind(state: &mut PokerState, actor: Seat, amount: Balance) -> TransitionResult {
    let current_bet = state.current_bet;
    let player_idx = state.players.iter().position(|p| p.seat == actor);

    if let Some(idx) = player_idx {
        let player = &mut state.players[idx];
        let actual = amount.min(player.chips);
        player.chips -= actual;
        player.current_bet = actual;
        player.total_bet_this_hand = actual;

        if player.chips == 0 {
            player.is_all_in = true;
        }

        state.pot.total += actual;

        if actual > current_bet {
            state.current_bet = actual;
        }
    }

    TransitionResult::Continue
}

fn apply_fold(state: &mut PokerState, actor: Seat) -> TransitionResult {
    if let Some(player) = state.get_player_mut(actor) {
        player.is_folded = true;
    }

    // check if only one player left
    if state.active_player_count() == 1 {
        return finish_hand_single_winner(state);
    }

    advance_action(state);
    TransitionResult::Continue
}

fn apply_check(state: &mut PokerState, actor: Seat) -> TransitionResult {
    // can only check if no bet to call
    if let Some(player) = state.get_player(actor) {
        if player.current_bet < state.current_bet {
            return TransitionResult::Invalid(TransitionError::InvalidBetAmount);
        }
    }

    advance_action(state);
    TransitionResult::Continue
}

fn apply_call(state: &mut PokerState, actor: Seat) -> TransitionResult {
    let player_idx = state.players.iter().position(|p| p.seat == actor);
    if let Some(idx) = player_idx {
        let current_bet = state.current_bet;
        let player = &mut state.players[idx];
        let to_call = current_bet - player.current_bet;
        let actual = to_call.min(player.chips);

        player.chips -= actual;
        player.current_bet += actual;
        player.total_bet_this_hand += actual;

        if player.chips == 0 {
            player.is_all_in = true;
        }

        state.pot.total += actual;
    }

    advance_action(state);
    TransitionResult::Continue
}

fn apply_raise(state: &mut PokerState, actor: Seat, raise_amount: Balance) -> TransitionResult {
    if raise_amount < state.min_raise {
        return TransitionResult::Invalid(TransitionError::InvalidBetAmount);
    }

    let player_idx = state.players.iter().position(|p| p.seat == actor);
    if let Some(idx) = player_idx {
        let current_bet = state.current_bet;
        let player = &mut state.players[idx];
        let to_call = current_bet - player.current_bet;
        let total_bet = to_call + raise_amount;

        if total_bet > player.chips {
            return TransitionResult::Invalid(TransitionError::InsufficientChips);
        }

        player.chips -= total_bet;
        player.current_bet += total_bet;
        player.total_bet_this_hand += total_bet;

        let new_current_bet = player.current_bet;

        if player.chips == 0 {
            player.is_all_in = true;
        }

        state.pot.total += total_bet;
        state.current_bet = new_current_bet;
        state.min_raise = raise_amount;
    }

    advance_action(state);
    TransitionResult::Continue
}

fn apply_all_in(state: &mut PokerState, actor: Seat) -> TransitionResult {
    let player_idx = state.players.iter().position(|p| p.seat == actor);
    if let Some(idx) = player_idx {
        let current_bet = state.current_bet;
        let player = &mut state.players[idx];
        let all_in_amount = player.chips;
        player.chips = 0;
        player.current_bet += all_in_amount;
        player.total_bet_this_hand += all_in_amount;
        player.is_all_in = true;

        let new_current_bet = player.current_bet;

        state.pot.total += all_in_amount;

        if new_current_bet > current_bet {
            state.current_bet = new_current_bet;
        }
    }

    advance_action(state);
    TransitionResult::Continue
}

fn apply_reveal_cards(
    state: &mut PokerState,
    actor: Seat,
    tokens: Vec<RevealToken>,
) -> TransitionResult {
    if state.phase != Phase::Showdown {
        return TransitionResult::Invalid(TransitionError::InvalidPhase);
    }

    // TODO: verify chaum-pedersen proofs on reveal tokens
    // store tokens for potential dispute
    state.reveal_tokens.extend(tokens.clone());

    // mark player cards as revealed
    if let Some(player) = state.get_player_mut(actor) {
        if let Some(indices) = player.hole_card_indices {
            player.revealed_cards = Some(indices);
        }
    }

    // check if all active players have revealed
    let all_revealed = state
        .players
        .iter()
        .filter(|p| !p.is_folded)
        .all(|p| p.revealed_cards.is_some());

    if all_revealed {
        return determine_winner(state);
    }

    TransitionResult::Continue
}

fn apply_reveal_community(
    state: &mut PokerState,
    _actor: Seat,
    tokens: Vec<RevealToken>,
) -> TransitionResult {
    // store tokens
    state.reveal_tokens.extend(tokens.clone());

    // add revealed card indices to community
    for token in &tokens {
        if !state.community_cards.contains(&token.card_index) {
            state.community_cards.push(token.card_index);
        }
    }

    // advance phase based on revealed community cards
    match state.community_cards.len() {
        3 if state.phase == Phase::PreFlop => {
            state.phase = Phase::Flop;
            reset_betting_round(state);
        }
        4 if state.phase == Phase::Flop => {
            state.phase = Phase::Turn;
            reset_betting_round(state);
        }
        5 if state.phase == Phase::Turn => {
            state.phase = Phase::River;
            reset_betting_round(state);
        }
        _ => {}
    }

    TransitionResult::Continue
}

fn apply_timeout(state: &mut PokerState, seat: Seat, current_time: u64) -> TransitionResult {
    const TIMEOUT_SECONDS: u64 = 60;

    if current_time - state.last_action_time < TIMEOUT_SECONDS {
        return TransitionResult::Invalid(TransitionError::InvalidPhase);
    }

    // timeout player folds
    if let Some(player) = state.get_player_mut(seat) {
        player.is_folded = true;
    }

    if state.active_player_count() == 1 {
        return finish_hand_single_winner(state);
    }

    advance_action(state);
    TransitionResult::Continue
}

fn advance_action(state: &mut PokerState) {
    // check if betting round complete
    if state.is_betting_round_complete() {
        advance_phase(state);
        return;
    }

    // find next player to act
    if let Some(next) = state.next_active_seat(state.action_seat) {
        state.action_seat = next;
    }
}

fn advance_phase(state: &mut PokerState) {
    match state.phase {
        Phase::PreFlop => {
            // reveal flop (community cards 20, 21, 22 after hole cards)
            state.phase = Phase::Flop;
        }
        Phase::Flop => {
            state.phase = Phase::Turn;
        }
        Phase::Turn => {
            state.phase = Phase::River;
        }
        Phase::River => {
            state.phase = Phase::Showdown;
        }
        _ => {}
    }

    reset_betting_round(state);
}

fn reset_betting_round(state: &mut PokerState) {
    state.current_bet = 0;
    state.min_raise = state.big_blind;

    for player in &mut state.players {
        player.current_bet = 0;
    }

    // action starts left of dealer
    state.action_seat = state
        .next_active_seat(state.dealer_seat)
        .unwrap_or(state.dealer_seat);
}

fn finish_hand_single_winner(state: &mut PokerState) -> TransitionResult {
    // find the one remaining player
    let winner = state.players.iter().find(|p| !p.is_folded);

    if let Some(w) = winner {
        let winner_seat = w.seat;
        let pot_total = state.pot.total;

        if let Some(winner_player) = state.get_player_mut(winner_seat) {
            winner_player.chips += pot_total;
        }

        state.phase = Phase::Complete;
        return TransitionResult::Finished {
            payouts: vec![(winner_seat, pot_total)],
        };
    }

    TransitionResult::Continue
}

fn determine_winner(state: &mut PokerState) -> TransitionResult {
    // TODO: use ligerito_shuffle::poker::showdown_holdem to determine winner
    // For now, split pot among active players (placeholder)

    let active_players: Vec<Seat> = state
        .players
        .iter()
        .filter(|p| !p.is_folded)
        .map(|p| p.seat)
        .collect();

    let pot_total = state.pot.total;
    let share = pot_total / active_players.len() as u128;

    let mut payouts = Vec::new();
    for seat in &active_players {
        if let Some(player) = state.get_player_mut(*seat) {
            player.chips += share;
        }
        payouts.push((*seat, share));
    }

    state.phase = Phase::Complete;
    TransitionResult::Finished { payouts }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_participants() -> Vec<Participant> {
        vec![
            Participant {
                account: PublicKey::from_raw([1u8; 32]),
                seat: 0,
                stake: 1000,
                encryption_key: vec![],
            },
            Participant {
                account: PublicKey::from_raw([2u8; 32]),
                seat: 1,
                stake: 1000,
                encryption_key: vec![],
            },
            Participant {
                account: PublicKey::from_raw([3u8; 32]),
                seat: 2,
                stake: 1000,
                encryption_key: vec![],
            },
        ]
    }

    #[test]
    fn test_start_hand() {
        let mut state = PokerState::new(H256::zero(), &mock_participants(), 10);
        let result = apply_action(&mut state, 0, Action::StartHand, 0);

        assert_eq!(result, TransitionResult::Continue);
        assert_eq!(state.phase, Phase::Shuffling);
        assert_eq!(state.hand_number, 1);
    }

    #[test]
    fn test_fold_wins_pot() {
        let mut state = PokerState::new(H256::zero(), &mock_participants(), 10);

        // start hand and skip to preflop
        state.phase = Phase::PreFlop;
        state.pot.total = 100;
        state.action_seat = 0;

        // player 0 folds
        let _ = apply_action(&mut state, 0, Action::Fold, 0);
        assert!(state.players[0].is_folded);

        // player 1 folds
        state.action_seat = 1;
        let result = apply_action(&mut state, 1, Action::Fold, 0);

        // player 2 wins
        match result {
            TransitionResult::Finished { payouts } => {
                assert_eq!(payouts.len(), 1);
                assert_eq!(payouts[0].0, 2); // seat 2 wins
            }
            _ => panic!("expected finished"),
        }
    }
}
