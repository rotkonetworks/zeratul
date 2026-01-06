//! hand evaluation for texas hold'em
//!
//! fast 7-card evaluator using lookup tables + combinatorics

use parity_scale_codec::{Decode, Encode};

/// card representation: 0-51 (suit * 13 + rank)
/// rank: 0=2, 1=3, ..., 8=T, 9=J, 10=Q, 11=K, 12=A
/// suit: 0=clubs, 1=diamonds, 2=hearts, 3=spades
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Encode, Decode)]
pub struct Card(pub u8);

impl Card {
    pub fn new(rank: u8, suit: u8) -> Self {
        debug_assert!(rank < 13 && suit < 4);
        Self(suit * 13 + rank)
    }

    pub fn rank(self) -> u8 {
        self.0 % 13
    }

    pub fn suit(self) -> u8 {
        self.0 / 13
    }

    /// rank as 2-14 (2=2, ..., A=14)
    pub fn rank_value(self) -> u8 {
        self.rank() + 2
    }

    pub fn from_str(s: &str) -> Option<Self> {
        let bytes = s.as_bytes();
        if bytes.len() != 2 {
            return None;
        }
        let rank = match bytes[0] {
            b'2'..=b'9' => bytes[0] - b'2',
            b'T' | b't' => 8,
            b'J' | b'j' => 9,
            b'Q' | b'q' => 10,
            b'K' | b'k' => 11,
            b'A' | b'a' => 12,
            _ => return None,
        };
        let suit = match bytes[1] {
            b'c' | b'C' => 0,
            b'd' | b'D' => 1,
            b'h' | b'H' => 2,
            b's' | b'S' => 3,
            _ => return None,
        };
        Some(Self::new(rank, suit))
    }

    pub fn to_string(self) -> String {
        let rank_char = match self.rank() {
            0..=7 => (b'2' + self.rank()) as char,
            8 => 'T',
            9 => 'J',
            10 => 'Q',
            11 => 'K',
            12 => 'A',
            _ => '?',
        };
        let suit_char = match self.suit() {
            0 => 'c',
            1 => 'd',
            2 => 'h',
            3 => 's',
            _ => '?',
        };
        format!("{}{}", rank_char, suit_char)
    }
}

impl std::fmt::Display for Card {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_string())
    }
}

/// hand ranking category
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Encode, Decode)]
#[repr(u8)]
pub enum HandRank {
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

impl HandRank {
    pub fn name(&self) -> &'static str {
        match self {
            Self::HighCard => "High Card",
            Self::OnePair => "One Pair",
            Self::TwoPair => "Two Pair",
            Self::ThreeOfAKind => "Three of a Kind",
            Self::Straight => "Straight",
            Self::Flush => "Flush",
            Self::FullHouse => "Full House",
            Self::FourOfAKind => "Four of a Kind",
            Self::StraightFlush => "Straight Flush",
            Self::RoyalFlush => "Royal Flush",
        }
    }
}

/// evaluated hand with rank and kickers for comparison
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub struct EvaluatedHand {
    /// hand category
    pub rank: HandRank,
    /// primary value (e.g., pair rank)
    pub primary: u8,
    /// secondary value (e.g., second pair, or kicker)
    pub secondary: u8,
    /// remaining kickers (up to 3)
    pub kickers: [u8; 3],
}

impl EvaluatedHand {
    /// comparison value for sorting (higher = better)
    pub fn score(&self) -> u32 {
        let rank_val = (self.rank as u32) << 20;
        let primary_val = (self.primary as u32) << 16;
        let secondary_val = (self.secondary as u32) << 12;
        let k0 = (self.kickers[0] as u32) << 8;
        let k1 = (self.kickers[1] as u32) << 4;
        let k2 = self.kickers[2] as u32;
        rank_val | primary_val | secondary_val | k0 | k1 | k2
    }
}

impl PartialOrd for EvaluatedHand {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for EvaluatedHand {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.score().cmp(&other.score())
    }
}

/// evaluate best 5-card hand from 7 cards (2 hole + 5 board)
pub fn evaluate_hand(cards: &[Card]) -> EvaluatedHand {
    debug_assert!(cards.len() >= 5 && cards.len() <= 7);

    // count ranks and suits
    let mut rank_counts = [0u8; 13];
    let mut suit_counts = [0u8; 4];
    let mut suit_cards: [Vec<u8>; 4] = Default::default();

    for card in cards {
        let r = card.rank();
        let s = card.suit();
        rank_counts[r as usize] += 1;
        suit_counts[s as usize] += 1;
        suit_cards[s as usize].push(r);
    }

    // check for flush
    let flush_suit = suit_counts.iter().position(|&c| c >= 5);

    // check for straight (including ace-low)
    fn find_straight(rank_mask: u16) -> Option<u8> {
        // check A-high to 5-high straights
        let masks = [
            (0b1111100000000u16, 12u8), // A-K-Q-J-T
            (0b0111110000000u16, 11u8), // K-Q-J-T-9
            (0b0011111000000u16, 10u8), // Q-J-T-9-8
            (0b0001111100000u16, 9u8),  // J-T-9-8-7
            (0b0000111110000u16, 8u8),  // T-9-8-7-6
            (0b0000011111000u16, 7u8),  // 9-8-7-6-5
            (0b0000001111100u16, 6u8),  // 8-7-6-5-4
            (0b0000000111110u16, 5u8),  // 7-6-5-4-3
            (0b0000000011111u16, 4u8),  // 6-5-4-3-2
            (0b1000000001111u16, 3u8),  // 5-4-3-2-A (wheel)
        ];
        for (mask, high) in masks {
            if rank_mask & mask == mask {
                return Some(high);
            }
        }
        None
    }

    // create rank bitmask
    let mut rank_mask: u16 = 0;
    for (i, &count) in rank_counts.iter().enumerate() {
        if count > 0 {
            rank_mask |= 1 << i;
        }
    }

    // check for straight flush
    if let Some(suit) = flush_suit {
        let mut flush_mask: u16 = 0;
        for &r in &suit_cards[suit] {
            flush_mask |= 1 << r;
        }
        if let Some(high) = find_straight(flush_mask) {
            let rank = if high == 12 {
                HandRank::RoyalFlush
            } else {
                HandRank::StraightFlush
            };
            return EvaluatedHand {
                rank,
                primary: high,
                secondary: 0,
                kickers: [0, 0, 0],
            };
        }
    }

    // find quads, trips, pairs
    let mut quads = Vec::new();
    let mut trips = Vec::new();
    let mut pairs = Vec::new();
    let mut singles = Vec::new();

    for (rank, &count) in rank_counts.iter().enumerate() {
        match count {
            4 => quads.push(rank as u8),
            3 => trips.push(rank as u8),
            2 => pairs.push(rank as u8),
            1 => singles.push(rank as u8),
            _ => {}
        }
    }

    // sort descending
    quads.sort_by(|a, b| b.cmp(a));
    trips.sort_by(|a, b| b.cmp(a));
    pairs.sort_by(|a, b| b.cmp(a));
    singles.sort_by(|a, b| b.cmp(a));

    // four of a kind
    if !quads.is_empty() {
        let kicker = trips.first().copied()
            .or_else(|| pairs.first().copied())
            .or_else(|| singles.first().copied())
            .unwrap_or(0);
        return EvaluatedHand {
            rank: HandRank::FourOfAKind,
            primary: quads[0],
            secondary: kicker,
            kickers: [0, 0, 0],
        };
    }

    // full house
    if !trips.is_empty() && (!pairs.is_empty() || trips.len() >= 2) {
        let pair_rank = if trips.len() >= 2 {
            trips[1]
        } else {
            pairs[0]
        };
        return EvaluatedHand {
            rank: HandRank::FullHouse,
            primary: trips[0],
            secondary: pair_rank,
            kickers: [0, 0, 0],
        };
    }

    // flush
    if let Some(suit) = flush_suit {
        let mut flush_ranks: Vec<u8> = suit_cards[suit].clone();
        flush_ranks.sort_by(|a, b| b.cmp(a));
        return EvaluatedHand {
            rank: HandRank::Flush,
            primary: flush_ranks[0],
            secondary: flush_ranks[1],
            kickers: [
                flush_ranks.get(2).copied().unwrap_or(0),
                flush_ranks.get(3).copied().unwrap_or(0),
                flush_ranks.get(4).copied().unwrap_or(0),
            ],
        };
    }

    // straight
    if let Some(high) = find_straight(rank_mask) {
        return EvaluatedHand {
            rank: HandRank::Straight,
            primary: high,
            secondary: 0,
            kickers: [0, 0, 0],
        };
    }

    // three of a kind
    if !trips.is_empty() {
        let kickers = get_kickers(&[&pairs, &singles], 2);
        return EvaluatedHand {
            rank: HandRank::ThreeOfAKind,
            primary: trips[0],
            secondary: kickers[0],
            kickers: [kickers[1], 0, 0],
        };
    }

    // two pair
    if pairs.len() >= 2 {
        let kicker = pairs.get(2).copied()
            .or_else(|| singles.first().copied())
            .unwrap_or(0);
        return EvaluatedHand {
            rank: HandRank::TwoPair,
            primary: pairs[0],
            secondary: pairs[1],
            kickers: [kicker, 0, 0],
        };
    }

    // one pair
    if !pairs.is_empty() {
        let kickers = get_kickers(&[&singles], 3);
        return EvaluatedHand {
            rank: HandRank::OnePair,
            primary: pairs[0],
            secondary: kickers[0],
            kickers: [kickers[1], kickers[2], 0],
        };
    }

    // high card
    let all_singles: Vec<u8> = singles.clone();
    EvaluatedHand {
        rank: HandRank::HighCard,
        primary: all_singles.get(0).copied().unwrap_or(0),
        secondary: all_singles.get(1).copied().unwrap_or(0),
        kickers: [
            all_singles.get(2).copied().unwrap_or(0),
            all_singles.get(3).copied().unwrap_or(0),
            all_singles.get(4).copied().unwrap_or(0),
        ],
    }
}

fn get_kickers(sources: &[&Vec<u8>], count: usize) -> Vec<u8> {
    let mut all: Vec<u8> = sources.iter().flat_map(|v| v.iter().copied()).collect();
    all.sort_by(|a, b| b.cmp(a));
    all.truncate(count);
    while all.len() < count {
        all.push(0);
    }
    all
}

/// compare two hands, returns ordering (Greater = first hand wins)
pub fn compare_hands(hand1: &[Card], hand2: &[Card]) -> std::cmp::Ordering {
    let eval1 = evaluate_hand(hand1);
    let eval2 = evaluate_hand(hand2);
    eval1.cmp(&eval2)
}

/// find winners among multiple hands
/// returns indices of winning hands (may be multiple for splits)
pub fn find_winners(hands: &[Vec<Card>]) -> Vec<usize> {
    if hands.is_empty() {
        return vec![];
    }

    let evaluations: Vec<EvaluatedHand> = hands.iter().map(|h| evaluate_hand(h)).collect();
    let max_score = evaluations.iter().map(|e| e.score()).max().unwrap();

    evaluations
        .iter()
        .enumerate()
        .filter(|(_, e)| e.score() == max_score)
        .map(|(i, _)| i)
        .collect()
}

/// describe hand in human-readable format
pub fn describe_hand(eval: &EvaluatedHand) -> String {
    let rank_name = |r: u8| match r {
        0 => "Two",
        1 => "Three",
        2 => "Four",
        3 => "Five",
        4 => "Six",
        5 => "Seven",
        6 => "Eight",
        7 => "Nine",
        8 => "Ten",
        9 => "Jack",
        10 => "Queen",
        11 => "King",
        12 => "Ace",
        _ => "?",
    };

    match eval.rank {
        HandRank::RoyalFlush => "Royal Flush".to_string(),
        HandRank::StraightFlush => format!("{}-high Straight Flush", rank_name(eval.primary)),
        HandRank::FourOfAKind => format!("Four {}s", rank_name(eval.primary)),
        HandRank::FullHouse => format!("{}s full of {}s", rank_name(eval.primary), rank_name(eval.secondary)),
        HandRank::Flush => format!("{}-high Flush", rank_name(eval.primary)),
        HandRank::Straight => format!("{}-high Straight", rank_name(eval.primary)),
        HandRank::ThreeOfAKind => format!("Three {}s", rank_name(eval.primary)),
        HandRank::TwoPair => format!("{}s and {}s", rank_name(eval.primary), rank_name(eval.secondary)),
        HandRank::OnePair => format!("Pair of {}s", rank_name(eval.primary)),
        HandRank::HighCard => format!("{} high", rank_name(eval.primary)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cards(s: &str) -> Vec<Card> {
        s.split_whitespace()
            .map(|c| Card::from_str(c).unwrap())
            .collect()
    }

    #[test]
    fn test_card_parsing() {
        let c = Card::from_str("As").unwrap();
        assert_eq!(c.rank(), 12);
        assert_eq!(c.suit(), 3);
        assert_eq!(c.to_string(), "As");

        let c2 = Card::from_str("2c").unwrap();
        assert_eq!(c2.rank(), 0);
        assert_eq!(c2.suit(), 0);
    }

    #[test]
    fn test_royal_flush() {
        let hand = cards("As Ks Qs Js Ts 2c 3d");
        let eval = evaluate_hand(&hand);
        assert_eq!(eval.rank, HandRank::RoyalFlush);
    }

    #[test]
    fn test_straight_flush() {
        let hand = cards("9h 8h 7h 6h 5h 2c 3d");
        let eval = evaluate_hand(&hand);
        assert_eq!(eval.rank, HandRank::StraightFlush);
        assert_eq!(eval.primary, 7); // 9-high
    }

    #[test]
    fn test_four_of_a_kind() {
        let hand = cards("Ac Ad Ah As 2c 3d 4h");
        let eval = evaluate_hand(&hand);
        assert_eq!(eval.rank, HandRank::FourOfAKind);
        assert_eq!(eval.primary, 12); // aces
    }

    #[test]
    fn test_full_house() {
        let hand = cards("Ac Ad Ah Ks Kd 2c 3d");
        let eval = evaluate_hand(&hand);
        assert_eq!(eval.rank, HandRank::FullHouse);
        assert_eq!(eval.primary, 12); // aces
        assert_eq!(eval.secondary, 11); // kings
    }

    #[test]
    fn test_flush() {
        let hand = cards("Ah Kh Qh Jh 9h 2c 3d");
        let eval = evaluate_hand(&hand);
        assert_eq!(eval.rank, HandRank::Flush);
    }

    #[test]
    fn test_straight() {
        let hand = cards("Ac Kd Qh Js Tc 2c 3d");
        let eval = evaluate_hand(&hand);
        assert_eq!(eval.rank, HandRank::Straight);
        assert_eq!(eval.primary, 12); // ace-high
    }

    #[test]
    fn test_wheel_straight() {
        let hand = cards("Ac 2d 3h 4s 5c 9c Td");
        let eval = evaluate_hand(&hand);
        assert_eq!(eval.rank, HandRank::Straight);
        assert_eq!(eval.primary, 3); // 5-high (wheel)
    }

    #[test]
    fn test_three_of_a_kind() {
        let hand = cards("Ac Ad Ah 5s 6c 2c 3d");
        let eval = evaluate_hand(&hand);
        assert_eq!(eval.rank, HandRank::ThreeOfAKind);
    }

    #[test]
    fn test_two_pair() {
        let hand = cards("Ac Ad Kh Ks 6c 2c 3d");
        let eval = evaluate_hand(&hand);
        assert_eq!(eval.rank, HandRank::TwoPair);
        assert_eq!(eval.primary, 12); // aces
        assert_eq!(eval.secondary, 11); // kings
    }

    #[test]
    fn test_one_pair() {
        let hand = cards("Ac Ad 5h 6s 7c 2c 3d");
        let eval = evaluate_hand(&hand);
        assert_eq!(eval.rank, HandRank::OnePair);
        assert_eq!(eval.primary, 12); // aces
    }

    #[test]
    fn test_high_card() {
        let hand = cards("Ac Kd Qh Js 9c 2c 3d");
        let eval = evaluate_hand(&hand);
        assert_eq!(eval.rank, HandRank::HighCard);
        assert_eq!(eval.primary, 12); // ace
    }

    #[test]
    fn test_compare_hands() {
        let flush = cards("Ah Kh Qh Jh 9h 2c 3d");
        let straight = cards("Ac Kd Qh Js Tc 2c 3d");
        assert_eq!(compare_hands(&flush, &straight), std::cmp::Ordering::Greater);

        let pair_aces = cards("Ac Ad 5h 6s 7c 2c 3d");
        let pair_kings = cards("Kc Kd 5h 6s 7c 2c 3d");
        assert_eq!(compare_hands(&pair_aces, &pair_kings), std::cmp::Ordering::Greater);
    }

    #[test]
    fn test_find_winners() {
        let hands = vec![
            cards("Ac Ad 5h 6s 7c 2c 3d"),  // pair of aces
            cards("Kc Kd 5h 6s 7c 2c 3d"),  // pair of kings
            cards("Ac As 5h 6s 7c 2c 3d"),  // pair of aces (tie)
        ];
        let winners = find_winners(&hands);
        assert_eq!(winners, vec![0, 2]); // both ace pairs win
    }

    #[test]
    fn test_describe_hand() {
        let hand = cards("Ac Ad Ah Ks Kd 2c 3d");
        let eval = evaluate_hand(&hand);
        let desc = describe_hand(&eval);
        assert_eq!(desc, "Aces full of Kings");
    }
}
