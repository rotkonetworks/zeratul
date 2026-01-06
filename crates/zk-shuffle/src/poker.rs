//! poker game logic for mental poker
//!
//! implements:
//! - card representation (rank, suit)
//! - hand evaluation (high card through royal flush)
//! - winner determination
//! - hand comparison

#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};

use core::cmp::Ordering;

// ============================================================================
// card representation
// ============================================================================

/// card rank (2-14, where 14 = ace)
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Rank {
    Two = 2,
    Three = 3,
    Four = 4,
    Five = 5,
    Six = 6,
    Seven = 7,
    Eight = 8,
    Nine = 9,
    Ten = 10,
    Jack = 11,
    Queen = 12,
    King = 13,
    Ace = 14,
}

impl Rank {
    /// all ranks in order
    pub const ALL: [Rank; 13] = [
        Rank::Two,
        Rank::Three,
        Rank::Four,
        Rank::Five,
        Rank::Six,
        Rank::Seven,
        Rank::Eight,
        Rank::Nine,
        Rank::Ten,
        Rank::Jack,
        Rank::Queen,
        Rank::King,
        Rank::Ace,
    ];

    /// create rank from u8 value
    pub fn from_value(v: u8) -> Option<Self> {
        match v {
            2 => Some(Rank::Two),
            3 => Some(Rank::Three),
            4 => Some(Rank::Four),
            5 => Some(Rank::Five),
            6 => Some(Rank::Six),
            7 => Some(Rank::Seven),
            8 => Some(Rank::Eight),
            9 => Some(Rank::Nine),
            10 => Some(Rank::Ten),
            11 => Some(Rank::Jack),
            12 => Some(Rank::Queen),
            13 => Some(Rank::King),
            14 => Some(Rank::Ace),
            _ => None,
        }
    }

    /// rank as u8 value
    pub fn value(self) -> u8 {
        self as u8
    }

    /// display character
    pub fn char(self) -> char {
        match self {
            Rank::Two => '2',
            Rank::Three => '3',
            Rank::Four => '4',
            Rank::Five => '5',
            Rank::Six => '6',
            Rank::Seven => '7',
            Rank::Eight => '8',
            Rank::Nine => '9',
            Rank::Ten => 'T',
            Rank::Jack => 'J',
            Rank::Queen => 'Q',
            Rank::King => 'K',
            Rank::Ace => 'A',
        }
    }
}

/// card suit
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Suit {
    Clubs = 0,
    Diamonds = 1,
    Hearts = 2,
    Spades = 3,
}

impl Suit {
    /// all suits
    pub const ALL: [Suit; 4] = [Suit::Clubs, Suit::Diamonds, Suit::Hearts, Suit::Spades];

    /// create from u8 value
    pub fn from_value(v: u8) -> Option<Self> {
        match v {
            0 => Some(Suit::Clubs),
            1 => Some(Suit::Diamonds),
            2 => Some(Suit::Hearts),
            3 => Some(Suit::Spades),
            _ => None,
        }
    }

    /// suit as u8 value
    pub fn value(self) -> u8 {
        self as u8
    }

    /// display character
    pub fn char(self) -> char {
        match self {
            Suit::Clubs => '♣',
            Suit::Diamonds => '♦',
            Suit::Hearts => '♥',
            Suit::Spades => '♠',
        }
    }
}

/// a playing card
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Card {
    pub rank: Rank,
    pub suit: Suit,
}

impl Card {
    /// create new card
    pub fn new(rank: Rank, suit: Suit) -> Self {
        Self { rank, suit }
    }

    /// create from index 0-51
    pub fn from_index(index: u8) -> Option<Self> {
        if index >= 52 {
            return None;
        }
        let rank = Rank::from_value((index / 4) + 2)?;
        let suit = Suit::from_value(index % 4)?;
        Some(Self { rank, suit })
    }

    /// convert to index 0-51
    pub fn to_index(self) -> u8 {
        (self.rank.value() - 2) * 4 + self.suit.value()
    }

    /// create standard 52-card deck
    pub fn standard_deck() -> Vec<Card> {
        let mut deck = Vec::with_capacity(52);
        for rank in Rank::ALL {
            for suit in Suit::ALL {
                deck.push(Card::new(rank, suit));
            }
        }
        deck
    }
}

impl core::fmt::Display for Card {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}{}", self.rank.char(), self.suit.char())
    }
}

// ============================================================================
// hand ranking
// ============================================================================

/// poker hand category (ordered by strength)
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum HandCategory {
    HighCard = 0,
    OnePair = 1,
    TwoPair = 2,
    ThreeOfAKind = 3,
    Straight = 4,
    Flush = 5,
    FullHouse = 6,
    FourOfAKind = 7,
    StraightFlush = 8,
    RoyalFlush = 9,
}

/// evaluated hand with ranking info
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HandRank {
    /// hand category
    pub category: HandCategory,
    /// primary ranks for comparison (highest first)
    /// e.g., for two pair: [higher pair rank, lower pair rank, kicker]
    pub ranks: [u8; 5],
}

impl HandRank {
    fn new(category: HandCategory, ranks: [u8; 5]) -> Self {
        Self { category, ranks }
    }
}

impl PartialOrd for HandRank {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HandRank {
    fn cmp(&self, other: &Self) -> Ordering {
        // first compare category
        match self.category.cmp(&other.category) {
            Ordering::Equal => {}
            ord => return ord,
        }
        // then compare ranks lexicographically
        self.ranks.cmp(&other.ranks)
    }
}

// ============================================================================
// hand evaluation
// ============================================================================

/// evaluate a 5-card poker hand
pub fn evaluate_hand(cards: &[Card; 5]) -> HandRank {
    let mut ranks: Vec<u8> = cards.iter().map(|c| c.rank.value()).collect();
    ranks.sort_by(|a, b| b.cmp(a)); // descending

    let suits: Vec<u8> = cards.iter().map(|c| c.suit.value()).collect();

    // check flush
    let is_flush = suits.iter().all(|&s| s == suits[0]);

    // check straight
    let is_straight = is_straight_ranks(&ranks);

    // check for wheel (A-2-3-4-5)
    let is_wheel = ranks == [14, 5, 4, 3, 2];

    // count rank frequencies
    let mut counts = [0u8; 15];
    for &r in &ranks {
        counts[r as usize] += 1;
    }

    // find groups
    let mut quads = Vec::new();
    let mut trips = Vec::new();
    let mut pairs = Vec::new();
    let mut singles = Vec::new();

    for r in (2..=14).rev() {
        match counts[r] {
            4 => quads.push(r as u8),
            3 => trips.push(r as u8),
            2 => pairs.push(r as u8),
            1 => singles.push(r as u8),
            _ => {}
        }
    }

    // determine hand category
    if is_flush && is_straight {
        if ranks[0] == 14 && ranks[1] == 13 {
            // royal flush: A-K-Q-J-T of same suit
            return HandRank::new(HandCategory::RoyalFlush, [14, 13, 12, 11, 10]);
        }
        // straight flush
        let high = if is_wheel { 5 } else { ranks[0] };
        return HandRank::new(HandCategory::StraightFlush, [high, 0, 0, 0, 0]);
    }

    if !quads.is_empty() {
        // four of a kind
        let quad_rank = quads[0];
        let kicker = singles.first().copied().unwrap_or(0);
        return HandRank::new(HandCategory::FourOfAKind, [quad_rank, kicker, 0, 0, 0]);
    }

    if !trips.is_empty() && !pairs.is_empty() {
        // full house
        return HandRank::new(
            HandCategory::FullHouse,
            [trips[0], pairs[0], 0, 0, 0],
        );
    }

    if is_flush {
        return HandRank::new(
            HandCategory::Flush,
            [ranks[0], ranks[1], ranks[2], ranks[3], ranks[4]],
        );
    }

    if is_straight || is_wheel {
        let high = if is_wheel { 5 } else { ranks[0] };
        return HandRank::new(HandCategory::Straight, [high, 0, 0, 0, 0]);
    }

    if !trips.is_empty() {
        // three of a kind
        let kickers: Vec<u8> = singles.iter().take(2).copied().collect();
        return HandRank::new(
            HandCategory::ThreeOfAKind,
            [
                trips[0],
                kickers.get(0).copied().unwrap_or(0),
                kickers.get(1).copied().unwrap_or(0),
                0,
                0,
            ],
        );
    }

    if pairs.len() >= 2 {
        // two pair
        let kicker = singles.first().copied().unwrap_or(0);
        return HandRank::new(
            HandCategory::TwoPair,
            [pairs[0], pairs[1], kicker, 0, 0],
        );
    }

    if pairs.len() == 1 {
        // one pair
        let kickers: Vec<u8> = singles.iter().take(3).copied().collect();
        return HandRank::new(
            HandCategory::OnePair,
            [
                pairs[0],
                kickers.get(0).copied().unwrap_or(0),
                kickers.get(1).copied().unwrap_or(0),
                kickers.get(2).copied().unwrap_or(0),
                0,
            ],
        );
    }

    // high card
    HandRank::new(
        HandCategory::HighCard,
        [ranks[0], ranks[1], ranks[2], ranks[3], ranks[4]],
    )
}

/// check if sorted ranks form a straight
fn is_straight_ranks(ranks: &[u8]) -> bool {
    if ranks.len() != 5 {
        return false;
    }
    // check consecutive (already sorted descending)
    for i in 0..4 {
        if ranks[i] != ranks[i + 1] + 1 {
            return false;
        }
    }
    true
}

/// evaluate best 5-card hand from 7 cards (texas hold'em)
pub fn evaluate_best_hand(cards: &[Card]) -> Option<HandRank> {
    if cards.len() < 5 {
        return None;
    }

    let mut best: Option<HandRank> = None;

    // generate all 5-card combinations
    for i in 0..cards.len() {
        for j in (i + 1)..cards.len() {
            for k in (j + 1)..cards.len() {
                for l in (k + 1)..cards.len() {
                    for m in (l + 1)..cards.len() {
                        let hand = [cards[i], cards[j], cards[k], cards[l], cards[m]];
                        let rank = evaluate_hand(&hand);

                        best = Some(match best {
                            None => rank,
                            Some(ref b) if rank > *b => rank,
                            Some(b) => b,
                        });
                    }
                }
            }
        }
    }

    best
}

// ============================================================================
// winner determination
// ============================================================================

/// result of comparing two hands
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareResult {
    Win,
    Lose,
    Tie,
}

/// compare two hands, returning result from perspective of hand1
pub fn compare_hands(hand1: &HandRank, hand2: &HandRank) -> CompareResult {
    match hand1.cmp(hand2) {
        Ordering::Greater => CompareResult::Win,
        Ordering::Less => CompareResult::Lose,
        Ordering::Equal => CompareResult::Tie,
    }
}

/// determine winner(s) from multiple hands
///
/// returns indices of winning players (multiple if tie)
pub fn determine_winners(hands: &[HandRank]) -> Vec<usize> {
    if hands.is_empty() {
        return Vec::new();
    }

    let mut best_rank = &hands[0];
    let mut winners = vec![0];

    for (i, hand) in hands.iter().enumerate().skip(1) {
        match hand.cmp(best_rank) {
            Ordering::Greater => {
                best_rank = hand;
                winners.clear();
                winners.push(i);
            }
            Ordering::Equal => {
                winners.push(i);
            }
            Ordering::Less => {}
        }
    }

    winners
}

/// player hand for showdown
#[derive(Clone, Debug)]
pub struct PlayerHand {
    /// player identifier
    pub player_id: usize,
    /// hole cards (2 cards for hold'em)
    pub hole_cards: Vec<Card>,
    /// best 5-card hand rank
    pub hand_rank: HandRank,
}

impl PlayerHand {
    /// create from hole cards and community cards
    pub fn from_holdem(
        player_id: usize,
        hole_cards: [Card; 2],
        community: &[Card; 5],
    ) -> Self {
        let mut all_cards = Vec::with_capacity(7);
        all_cards.extend_from_slice(&hole_cards);
        all_cards.extend_from_slice(community);

        let hand_rank = evaluate_best_hand(&all_cards).expect("7 cards should have 5-card hand");

        Self {
            player_id,
            hole_cards: hole_cards.to_vec(),
            hand_rank,
        }
    }
}

/// showdown result
#[derive(Clone, Debug)]
pub struct ShowdownResult {
    /// winning player indices
    pub winners: Vec<usize>,
    /// all player hands evaluated
    pub hands: Vec<PlayerHand>,
    /// winning hand category
    pub winning_category: HandCategory,
}

/// run showdown for texas hold'em
pub fn showdown_holdem(
    hole_cards: &[(usize, [Card; 2])], // (player_id, [card1, card2])
    community: &[Card; 5],
) -> ShowdownResult {
    let hands: Vec<PlayerHand> = hole_cards
        .iter()
        .map(|(id, cards)| PlayerHand::from_holdem(*id, *cards, community))
        .collect();

    let ranks: Vec<HandRank> = hands.iter().map(|h| h.hand_rank.clone()).collect();
    let winners = determine_winners(&ranks);

    let winning_category = if !winners.is_empty() {
        hands[winners[0]].hand_rank.category
    } else {
        HandCategory::HighCard
    };

    ShowdownResult {
        winners,
        hands,
        winning_category,
    }
}

// ============================================================================
// tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn card(rank: u8, suit: u8) -> Card {
        Card::new(Rank::from_value(rank).unwrap(), Suit::from_value(suit).unwrap())
    }

    #[test]
    fn test_card_index_roundtrip() {
        for i in 0..52 {
            let c = Card::from_index(i).unwrap();
            assert_eq!(c.to_index(), i);
        }
    }

    #[test]
    fn test_standard_deck() {
        let deck = Card::standard_deck();
        assert_eq!(deck.len(), 52);

        // check all unique
        let mut seen = std::collections::HashSet::new();
        for c in &deck {
            assert!(seen.insert(*c));
        }
    }

    #[test]
    fn test_high_card() {
        let hand = [
            card(14, 0), // A♣
            card(10, 1), // T♦
            card(7, 2),  // 7♥
            card(5, 3),  // 5♠
            card(2, 0),  // 2♣
        ];
        let rank = evaluate_hand(&hand);
        assert_eq!(rank.category, HandCategory::HighCard);
        assert_eq!(rank.ranks[0], 14); // ace high
    }

    #[test]
    fn test_one_pair() {
        let hand = [
            card(10, 0), // T♣
            card(10, 1), // T♦
            card(7, 2),  // 7♥
            card(5, 3),  // 5♠
            card(2, 0),  // 2♣
        ];
        let rank = evaluate_hand(&hand);
        assert_eq!(rank.category, HandCategory::OnePair);
        assert_eq!(rank.ranks[0], 10); // pair of tens
    }

    #[test]
    fn test_two_pair() {
        let hand = [
            card(10, 0), // T♣
            card(10, 1), // T♦
            card(7, 2),  // 7♥
            card(7, 3),  // 7♠
            card(2, 0),  // 2♣
        ];
        let rank = evaluate_hand(&hand);
        assert_eq!(rank.category, HandCategory::TwoPair);
        assert_eq!(rank.ranks[0], 10); // higher pair
        assert_eq!(rank.ranks[1], 7);  // lower pair
    }

    #[test]
    fn test_three_of_a_kind() {
        let hand = [
            card(10, 0), // T♣
            card(10, 1), // T♦
            card(10, 2), // T♥
            card(7, 3),  // 7♠
            card(2, 0),  // 2♣
        ];
        let rank = evaluate_hand(&hand);
        assert_eq!(rank.category, HandCategory::ThreeOfAKind);
        assert_eq!(rank.ranks[0], 10);
    }

    #[test]
    fn test_straight() {
        let hand = [
            card(10, 0), // T♣
            card(9, 1),  // 9♦
            card(8, 2),  // 8♥
            card(7, 3),  // 7♠
            card(6, 0),  // 6♣
        ];
        let rank = evaluate_hand(&hand);
        assert_eq!(rank.category, HandCategory::Straight);
        assert_eq!(rank.ranks[0], 10); // ten-high straight
    }

    #[test]
    fn test_wheel_straight() {
        let hand = [
            card(14, 0), // A♣
            card(2, 1),  // 2♦
            card(3, 2),  // 3♥
            card(4, 3),  // 4♠
            card(5, 0),  // 5♣
        ];
        let rank = evaluate_hand(&hand);
        assert_eq!(rank.category, HandCategory::Straight);
        assert_eq!(rank.ranks[0], 5); // wheel (5-high)
    }

    #[test]
    fn test_flush() {
        let hand = [
            card(14, 2), // A♥
            card(10, 2), // T♥
            card(7, 2),  // 7♥
            card(5, 2),  // 5♥
            card(2, 2),  // 2♥
        ];
        let rank = evaluate_hand(&hand);
        assert_eq!(rank.category, HandCategory::Flush);
        assert_eq!(rank.ranks[0], 14); // ace-high flush
    }

    #[test]
    fn test_full_house() {
        let hand = [
            card(10, 0), // T♣
            card(10, 1), // T♦
            card(10, 2), // T♥
            card(7, 3),  // 7♠
            card(7, 0),  // 7♣
        ];
        let rank = evaluate_hand(&hand);
        assert_eq!(rank.category, HandCategory::FullHouse);
        assert_eq!(rank.ranks[0], 10); // tens full of sevens
        assert_eq!(rank.ranks[1], 7);
    }

    #[test]
    fn test_four_of_a_kind() {
        let hand = [
            card(10, 0), // T♣
            card(10, 1), // T♦
            card(10, 2), // T♥
            card(10, 3), // T♠
            card(7, 0),  // 7♣
        ];
        let rank = evaluate_hand(&hand);
        assert_eq!(rank.category, HandCategory::FourOfAKind);
        assert_eq!(rank.ranks[0], 10);
    }

    #[test]
    fn test_straight_flush() {
        let hand = [
            card(10, 2), // T♥
            card(9, 2),  // 9♥
            card(8, 2),  // 8♥
            card(7, 2),  // 7♥
            card(6, 2),  // 6♥
        ];
        let rank = evaluate_hand(&hand);
        assert_eq!(rank.category, HandCategory::StraightFlush);
        assert_eq!(rank.ranks[0], 10);
    }

    #[test]
    fn test_royal_flush() {
        let hand = [
            card(14, 3), // A♠
            card(13, 3), // K♠
            card(12, 3), // Q♠
            card(11, 3), // J♠
            card(10, 3), // T♠
        ];
        let rank = evaluate_hand(&hand);
        assert_eq!(rank.category, HandCategory::RoyalFlush);
    }

    #[test]
    fn test_compare_hands() {
        let pair = evaluate_hand(&[
            card(10, 0),
            card(10, 1),
            card(7, 2),
            card(5, 3),
            card(2, 0),
        ]);
        let trips = evaluate_hand(&[
            card(8, 0),
            card(8, 1),
            card(8, 2),
            card(5, 3),
            card(2, 0),
        ]);

        assert_eq!(compare_hands(&trips, &pair), CompareResult::Win);
        assert_eq!(compare_hands(&pair, &trips), CompareResult::Lose);
        assert_eq!(compare_hands(&pair, &pair), CompareResult::Tie);
    }

    #[test]
    fn test_determine_winners() {
        let hands = vec![
            evaluate_hand(&[card(10, 0), card(10, 1), card(7, 2), card(5, 3), card(2, 0)]),
            evaluate_hand(&[card(8, 0), card(8, 1), card(8, 2), card(5, 3), card(2, 0)]),
            evaluate_hand(&[card(14, 0), card(10, 1), card(7, 2), card(5, 3), card(2, 0)]),
        ];

        let winners = determine_winners(&hands);
        assert_eq!(winners, vec![1]); // three of a kind wins
    }

    #[test]
    fn test_determine_winners_tie() {
        let hands = vec![
            evaluate_hand(&[card(10, 0), card(10, 1), card(7, 2), card(5, 3), card(2, 0)]),
            evaluate_hand(&[card(10, 2), card(10, 3), card(7, 0), card(5, 1), card(2, 2)]),
        ];

        let winners = determine_winners(&hands);
        assert_eq!(winners, vec![0, 1]); // tie
    }

    #[test]
    fn test_evaluate_best_hand_7_cards() {
        let cards = vec![
            card(14, 0), // A♣
            card(14, 1), // A♦
            card(10, 2), // T♥
            card(10, 3), // T♠
            card(7, 0),  // 7♣
            card(5, 1),  // 5♦
            card(2, 2),  // 2♥
        ];

        let rank = evaluate_best_hand(&cards).unwrap();
        assert_eq!(rank.category, HandCategory::TwoPair);
        assert_eq!(rank.ranks[0], 14); // aces and tens
        assert_eq!(rank.ranks[1], 10);
    }

    #[test]
    fn test_showdown_holdem() {
        let community = [
            card(10, 0), // T♣
            card(9, 1),  // 9♦
            card(8, 2),  // 8♥
            card(2, 3),  // 2♠
            card(3, 0),  // 3♣
        ];

        let hole_cards = vec![
            (0, [card(11, 0), card(7, 1)]), // player 0: J-7 (straight J-high)
            (1, [card(14, 0), card(14, 1)]), // player 1: A-A (pair of aces)
        ];

        let result = showdown_holdem(&hole_cards, &community);

        assert_eq!(result.winners, vec![0]); // straight beats pair
        assert_eq!(result.winning_category, HandCategory::Straight);
    }

    #[test]
    fn test_hand_comparison_kicker() {
        // both have pair of aces, but different kickers
        let hand1 = evaluate_hand(&[
            card(14, 0), // A♣
            card(14, 1), // A♦
            card(13, 2), // K♥ (kicker)
            card(5, 3),
            card(2, 0),
        ]);
        let hand2 = evaluate_hand(&[
            card(14, 2), // A♥
            card(14, 3), // A♠
            card(12, 0), // Q♣ (kicker)
            card(5, 1),
            card(2, 2),
        ]);

        assert_eq!(compare_hands(&hand1, &hand2), CompareResult::Win);
    }
}
