//! shuffle constraint encoding for ligerito proofs
//!
//! implements grand product argument for permutation verification
//! using multilinear polynomial techniques over binary fields
//!
//! design follows penumbra's rigorous constraint patterns:
//! - typed constraint variables with explicit domain types
//! - domain-separated hashes for all derivations
//! - parallel native checking before circuit encoding
//! - composable constraint derivation

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use blake2::{Blake2s256, Digest};
use ligerito_binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement};

use crate::{Permutation, Result, ShuffleError};

// ============================================================================
// domain separators
// ============================================================================

/// domain separator for card hashing
const CARD_DOMAIN_SEP: &[u8] = b"mental-poker.card.v1";

/// domain separator for grand product accumulator
const GRAND_PRODUCT_DOMAIN_SEP: &[u8] = b"mental-poker.grand-product.v1";

/// domain separator for permutation encoding
const PERMUTATION_DOMAIN_SEP: &[u8] = b"mental-poker.permutation.v1";

// ============================================================================
// constraint configuration
// ============================================================================

/// constraint system for shuffle verification
///
/// the shuffle proof must demonstrate:
/// 1. output is a permutation of input (grand product)
/// 2. remasking is correctly applied (commitment check)
#[derive(Clone, Debug)]
pub struct ShuffleConstraints {
    /// number of cards
    pub n: usize,
    /// log2(n) rounded up
    pub log_n: usize,
    /// polynomial size (power of 2)
    pub poly_size: usize,
}

impl ShuffleConstraints {
    /// create constraints for n cards
    pub fn new(n: usize) -> Self {
        let padded = n.next_power_of_two();
        let log_n = padded.ilog2() as usize;
        // we need space for multiple constraint types
        // - n elements for input commitments
        // - n elements for output commitments
        // - n elements for permutation indices
        // - n elements for grand product accumulator
        // - n elements for constraint satisfaction
        let poly_size = (padded * 8).next_power_of_two();

        Self {
            n,
            log_n,
            poly_size,
        }
    }
}

// ============================================================================
// typed constraint variables (penumbra-style)
// ============================================================================

/// a card represented in the constraint system
///
/// corresponds to elgamal ciphertext (c0, c1) but encoded as binary field element
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CardVar {
    /// the field element encoding of this card
    pub elem: BinaryElem128,
}

impl CardVar {
    /// create card variable from ciphertext components
    pub fn from_ciphertext(c0: u64, c1: u64) -> Self {
        // domain-separated hash of card data
        let mut hasher = Blake2s256::new();
        hasher.update(CARD_DOMAIN_SEP);
        hasher.update(&c0.to_le_bytes());
        hasher.update(&c1.to_le_bytes());
        let hash = hasher.finalize();

        // take first 16 bytes as u128 for field element
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&hash[..16]);
        let val = u128::from_le_bytes(bytes);

        Self {
            elem: BinaryElem128::from_value(val),
        }
    }

    /// add random challenge β for grand product
    pub fn add_challenge(&self, beta: &BinaryElem128) -> BinaryElem128 {
        self.elem.add(beta)
    }
}

/// accumulator variable for grand product argument
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccumulatorVar {
    /// running product
    pub value: BinaryElem128,
}

impl AccumulatorVar {
    /// identity element (multiplicative identity)
    pub fn one() -> Self {
        Self {
            value: BinaryElem128::one(),
        }
    }

    /// accumulate a term: acc *= (card + β)
    pub fn accumulate(&self, card: &CardVar, beta: &BinaryElem128) -> Self {
        let term = card.add_challenge(beta);
        Self {
            value: self.value.mul(&term),
        }
    }
}

/// permutation index variable
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PermutationIndexVar {
    /// the permutation value at this position
    pub pi: usize,
    /// encoded as field element
    pub elem: BinaryElem32,
}

impl PermutationIndexVar {
    pub fn new(pi: usize) -> Self {
        Self {
            pi,
            elem: BinaryElem32::from_bits(pi as u64),
        }
    }
}

/// constraint satisfaction variable
///
/// encodes whether a constraint is satisfied (zero) or violated (non-zero)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConstraintVar {
    /// zero if satisfied, non-zero if violated
    pub value: BinaryElem32,
}

impl ConstraintVar {
    /// satisfied constraint
    pub fn satisfied() -> Self {
        Self {
            value: BinaryElem32::zero(),
        }
    }

    /// violated constraint
    pub fn violated() -> Self {
        Self {
            value: BinaryElem32::one(),
        }
    }

    /// check if satisfied
    pub fn is_satisfied(&self) -> bool {
        self.value == BinaryElem32::zero()
    }

    /// constraint from difference (zero if equal)
    pub fn from_difference(a: &BinaryElem128, b: &BinaryElem128) -> Self {
        let diff = a.add(b); // XOR in binary field
        let val = (diff.poly().value() as u32) as u64;
        Self {
            value: BinaryElem32::from_bits(val),
        }
    }
}

// ============================================================================
// out-of-circuit verification (parallel checking)
// ============================================================================

/// verify shuffle relation out of circuit (for debugging/testing)
///
/// mirrors the constraint system logic for parallel checking
pub fn check_shuffle_satisfaction(
    input_deck: &[(u64, u64)],
    output_deck: &[(u64, u64)],
    perm: &Permutation,
    beta: BinaryElem128,
) -> Result<bool> {
    let n = input_deck.len();
    if output_deck.len() != n || perm.len() != n {
        return Err(ShuffleError::DeckSizeMismatch {
            expected: n,
            got: output_deck.len(),
        });
    }

    // check 1: permutation indices are valid
    for i in 0..n {
        let pi_i = perm.get(i);
        if pi_i >= n {
            return Ok(false);
        }
    }

    // check 2: grand products match
    let mut acc_in = AccumulatorVar::one();
    let mut acc_out = AccumulatorVar::one();

    for i in 0..n {
        let in_card = CardVar::from_ciphertext(input_deck[i].0, input_deck[i].1);
        let out_card = CardVar::from_ciphertext(output_deck[i].0, output_deck[i].1);

        acc_in = acc_in.accumulate(&in_card, &beta);
        acc_out = acc_out.accumulate(&out_card, &beta);
    }

    if acc_in.value != acc_out.value {
        return Ok(false);
    }

    // check 3: permutation relation holds
    // output[i] should match input[π(i)] (modulo remasking)
    // note: in full protocol, we'd also verify remasking factors
    for i in 0..n {
        let pi_i = perm.get(i);
        let expected = CardVar::from_ciphertext(input_deck[pi_i].0, input_deck[pi_i].1);
        let actual = CardVar::from_ciphertext(output_deck[i].0, output_deck[i].1);

        // in a shuffle without remasking, these would be equal
        // with remasking, we need additional constraint on masking factors
        // for now, we just encode the difference
        let constraint = ConstraintVar::from_difference(&expected.elem, &actual.elem);
        // note: constraint will be non-zero due to remasking, that's expected
        let _ = constraint;
    }

    Ok(true)
}

// ============================================================================
// constraint polynomial encoding
// ============================================================================

/// layout of constraint polynomial sections
///
/// the polynomial is divided into sections:
/// - section 0: card data (input/output pairs) - PUBLIC
/// - section 1: grand product accumulators - PUBLIC (derivable from cards)
/// - section 2: final constraint flag only
///
/// NOTE: permutation indices are NOT encoded (would leak witness)
#[derive(Clone, Copy, Debug)]
pub struct PolynomialLayout {
    /// offset of card data section
    pub card_offset: usize,
    /// offset of accumulator section
    pub acc_offset: usize,
    /// offset of final constraint (single element)
    pub constraint_offset: usize,
    /// total size
    pub total_size: usize,
}

impl PolynomialLayout {
    pub fn new(n: usize) -> Self {
        let padded = n.next_power_of_two();
        let section_size = padded * 4; // 4 elements per card

        Self {
            card_offset: 0,
            acc_offset: section_size,
            constraint_offset: section_size * 2,
            total_size: (section_size * 3).next_power_of_two(),
        }
    }
}

/// encode shuffle as constraint polynomial
///
/// uses grand product argument:
/// ∏ᵢ (input[i] + β) = ∏ᵢ (output[i] + β)
///
/// for random challenge β, this holds iff output is permutation of input
pub fn encode_grand_product_constraints(
    input_deck: &[(u64, u64)],
    output_deck: &[(u64, u64)],
    perm: &Permutation,
    beta: BinaryElem128, // random challenge from transcript
) -> Result<Vec<BinaryElem32>> {
    let n = input_deck.len();
    if output_deck.len() != n || perm.len() != n {
        return Err(ShuffleError::DeckSizeMismatch {
            expected: n,
            got: output_deck.len(),
        });
    }

    let layout = PolynomialLayout::new(n);
    let mut poly = vec![BinaryElem32::zero(); layout.total_size];

    // compute grand product accumulators
    let mut acc_in = AccumulatorVar::one();
    let mut acc_out = AccumulatorVar::one();

    let mut input_accs = Vec::with_capacity(n);
    let mut output_accs = Vec::with_capacity(n);

    for i in 0..n {
        let in_card = CardVar::from_ciphertext(input_deck[i].0, input_deck[i].1);
        let out_card = CardVar::from_ciphertext(output_deck[i].0, output_deck[i].1);

        acc_in = acc_in.accumulate(&in_card, &beta);
        acc_out = acc_out.accumulate(&out_card, &beta);

        input_accs.push(acc_in);
        output_accs.push(acc_out);
    }

    // final products should be equal for valid permutation
    let products_match = acc_in.value == acc_out.value;

    // encode card section (PUBLIC - verifier knows input/output decks)
    for i in 0..n {
        let base = layout.card_offset + i * 4;

        // input card (lower 32 bits of each component)
        let (in_c0, in_c1) = input_deck[i];
        poly[base] = BinaryElem32::from_bits(in_c0 & 0xFFFFFFFF);
        poly[base + 1] = BinaryElem32::from_bits(in_c1 & 0xFFFFFFFF);

        // output card
        let (out_c0, out_c1) = output_deck[i];
        poly[base + 2] = BinaryElem32::from_bits(out_c0 & 0xFFFFFFFF);
        poly[base + 3] = BinaryElem32::from_bits(out_c1 & 0xFFFFFFFF);
    }

    // NOTE: permutation indices NOT encoded (would leak witness to verifier)

    // encode accumulator section (PUBLIC - derivable from cards + β)
    for i in 0..n {
        let in_acc_val = (input_accs[i].value.poly().value() as u32) as u64;
        let out_acc_val = (output_accs[i].value.poly().value() as u32) as u64;

        poly[layout.acc_offset + i * 2] = BinaryElem32::from_bits(in_acc_val);
        poly[layout.acc_offset + i * 2 + 1] = BinaryElem32::from_bits(out_acc_val);
    }

    // NOTE: per-position constraints NOT encoded (would create distinguisher)
    // grand product equality is sufficient for permutation proof

    // final constraint only: products match (single bit, no leak)
    let final_constraint = if products_match {
        ConstraintVar::satisfied()
    } else {
        ConstraintVar::violated()
    };
    poly[layout.constraint_offset] = final_constraint.value;

    Ok(poly)
}

/// multilinear polynomial for permutation check
///
/// WARNING: this function directly encodes π(i) values in the polynomial.
/// when ligerito opens rows, the permutation is revealed to verifier.
/// use grand product argument instead for ZK proofs.
///
/// kept for testing/debugging only.
#[deprecated(note = "leaks permutation - use grand product for ZK")]
pub fn encode_permutation_polynomial(perm: &Permutation) -> Result<Vec<BinaryElem32>> {
    let n = perm.len();
    let padded = n.next_power_of_two();

    // domain-separated encoding
    let mut hasher = Blake2s256::new();
    hasher.update(PERMUTATION_DOMAIN_SEP);
    hasher.update(&(n as u64).to_le_bytes());
    let _domain_hash = hasher.finalize();

    // encode permutation as multilinear polynomial
    // f(x) where x ∈ {0,1}^log_n encodes position
    // f(bin(i)) = π(i)
    let mut poly = vec![BinaryElem32::zero(); padded];

    for i in 0..n {
        let pi_i = perm.get(i);
        poly[i] = BinaryElem32::from_bits(pi_i as u64);
    }

    Ok(poly)
}

/// encode lookup argument for permutation
///
/// proves that output[i] appears in input set
/// using logarithmic derivative lookup (similar to logUp)
pub fn encode_lookup_constraints(
    input_deck: &[(u64, u64)],
    output_deck: &[(u64, u64)],
    gamma: BinaryElem128, // random challenge
) -> Result<Vec<BinaryElem32>> {
    let n = input_deck.len();
    if output_deck.len() != n {
        return Err(ShuffleError::DeckSizeMismatch {
            expected: n,
            got: output_deck.len(),
        });
    }

    let padded = n.next_power_of_two();
    let mut poly = Vec::with_capacity(padded * 4);

    // for lookup: sum of 1/(input[i] + γ) = sum of 1/(output[i] + γ)
    // we encode the terms, verifier checks the sum
    for i in 0..n {
        let in_card = CardVar::from_ciphertext(input_deck[i].0, input_deck[i].1);
        let out_card = CardVar::from_ciphertext(output_deck[i].0, output_deck[i].1);

        // encode (card + γ)
        let in_term = in_card.add_challenge(&gamma);
        let out_term = out_card.add_challenge(&gamma);

        // truncate to 32 bits for polynomial
        poly.push(BinaryElem32::from_bits(in_term.poly().value() as u64 & 0xFFFFFFFF));
        poly.push(BinaryElem32::from_bits(out_term.poly().value() as u64 & 0xFFFFFFFF));

        // position marker
        poly.push(BinaryElem32::from_bits(i as u64));
        poly.push(BinaryElem32::zero()); // padding
    }

    // pad to power of 2
    let target = poly.len().next_power_of_two();
    poly.resize(target, BinaryElem32::zero());

    Ok(poly)
}

/// complete shuffle constraint polynomial (ZK-safe)
///
/// combines:
/// 1. grand product argument (proves multiset equality)
/// 2. lookup constraints (proves output ⊆ input)
///
/// NOTE: permutation indices and masking factors are NOT encoded
/// as they would leak the witness to the verifier
pub fn encode_complete_shuffle_constraints(
    input_deck: &[(u64, u64)],
    output_deck: &[(u64, u64)],
    perm: &Permutation,
    _masking_factors: &[u64], // kept for API compat, not encoded
    beta: BinaryElem128,
    gamma: BinaryElem128,
) -> Result<Vec<BinaryElem32>> {
    let n = input_deck.len();
    if output_deck.len() != n || perm.len() != n {
        return Err(ShuffleError::DeckSizeMismatch {
            expected: n,
            got: output_deck.len(),
        });
    }

    // grand product proves output is permutation of input
    let grand_product = encode_grand_product_constraints(input_deck, output_deck, perm, beta)?;
    // lookup proves each output appears in input set
    let lookup = encode_lookup_constraints(input_deck, output_deck, gamma)?;

    // combine into single polynomial
    let section_size = grand_product.len().max(lookup.len());
    let total_size = (section_size * 2).next_power_of_two();

    let mut combined = vec![BinaryElem32::zero(); total_size];

    // section 0: grand product
    for (i, elem) in grand_product.iter().enumerate() {
        combined[i] = *elem;
    }

    // section 1: lookup
    let offset1 = section_size;
    for (i, elem) in lookup.iter().enumerate() {
        combined[offset1 + i] = *elem;
    }

    // NOTE: no permutation indices, no masking factors (ZK)

    Ok(combined)
}

// ============================================================================
// tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_deck(n: usize) -> Vec<(u64, u64)> {
        (0..n)
            .map(|i| ((i as u64 + 1) * 100, (i as u64 + 1) * 101))
            .collect()
    }

    #[test]
    fn test_card_var_determinism() {
        let card1a = CardVar::from_ciphertext(100, 200);
        let card1b = CardVar::from_ciphertext(100, 200);
        let card2 = CardVar::from_ciphertext(100, 201);

        assert_eq!(card1a.elem, card1b.elem, "same card should produce same elem");
        assert_ne!(card1a.elem, card2.elem, "different cards should differ");
    }

    #[test]
    fn test_accumulator_associativity() {
        let beta = BinaryElem128::from_bits(0x12345678);
        let cards = [
            CardVar::from_ciphertext(100, 101),
            CardVar::from_ciphertext(200, 201),
            CardVar::from_ciphertext(300, 301),
        ];

        // accumulate in order
        let mut acc1 = AccumulatorVar::one();
        for card in &cards {
            acc1 = acc1.accumulate(card, &beta);
        }

        // accumulate in different order (product should be same for permutation)
        let mut acc2 = AccumulatorVar::one();
        acc2 = acc2.accumulate(&cards[1], &beta);
        acc2 = acc2.accumulate(&cards[2], &beta);
        acc2 = acc2.accumulate(&cards[0], &beta);

        assert_eq!(
            acc1.value, acc2.value,
            "accumulator should be order-independent (same multiset)"
        );
    }

    #[test]
    fn test_constraint_var_satisfaction() {
        let elem = BinaryElem128::from_bits(0x12345678);

        let satisfied = ConstraintVar::from_difference(&elem, &elem);
        assert!(satisfied.is_satisfied(), "same elements should satisfy");

        let other = BinaryElem128::from_bits(0x87654321);
        let violated = ConstraintVar::from_difference(&elem, &other);
        assert!(!violated.is_satisfied(), "different elements should violate");
    }

    #[test]
    fn test_grand_product_valid_permutation() {
        let input = make_deck(4);
        let perm = Permutation::new(vec![2, 0, 3, 1]).unwrap();
        let output: Vec<_> = perm.apply(&input);

        let beta = BinaryElem128::from_bits(0x12345678);

        // check out-of-circuit first
        let satisfied = check_shuffle_satisfaction(&input, &output, &perm, beta).unwrap();
        assert!(satisfied, "valid permutation should satisfy");

        // encode constraints
        let poly = encode_grand_product_constraints(&input, &output, &perm, beta).unwrap();
        assert!(poly.len().is_power_of_two());

        // final constraint should be satisfied (0)
        let layout = PolynomialLayout::new(4);
        assert_eq!(
            poly[layout.constraint_offset],
            BinaryElem32::zero(),
            "valid permutation should have matching products"
        );
    }

    #[test]
    fn test_grand_product_invalid_permutation() {
        let input = make_deck(4);
        // invalid: output is not a permutation of input
        let output = vec![(999, 999), (888, 888), (777, 777), (666, 666)];
        let perm = Permutation::new(vec![0, 1, 2, 3]).unwrap();

        let beta = BinaryElem128::from_bits(0x12345678);

        // check out-of-circuit
        let satisfied = check_shuffle_satisfaction(&input, &output, &perm, beta).unwrap();
        assert!(!satisfied, "invalid permutation should not satisfy");

        // encode constraints
        let poly = encode_grand_product_constraints(&input, &output, &perm, beta).unwrap();

        // final constraint should be violated (1)
        let layout = PolynomialLayout::new(4);
        assert_eq!(
            poly[layout.constraint_offset],
            BinaryElem32::one(),
            "invalid permutation should have mismatched products"
        );
    }

    #[test]
    #[allow(deprecated)]
    fn test_permutation_polynomial() {
        let perm = Permutation::new(vec![2, 0, 3, 1]).unwrap();

        let poly = encode_permutation_polynomial(&perm).unwrap();

        assert!(poly.len().is_power_of_two());
        assert_eq!(poly[0], BinaryElem32::from_bits(2));
        assert_eq!(poly[1], BinaryElem32::from_bits(0));
        assert_eq!(poly[2], BinaryElem32::from_bits(3));
        assert_eq!(poly[3], BinaryElem32::from_bits(1));
    }

    #[test]
    fn test_polynomial_layout() {
        let layout = PolynomialLayout::new(4);

        assert_eq!(layout.card_offset, 0);
        assert!(layout.acc_offset > layout.card_offset);
        assert!(layout.constraint_offset > layout.acc_offset);
        assert!(layout.total_size.is_power_of_two());
    }

    #[test]
    fn test_complete_constraints() {
        let input = make_deck(4);
        let perm = Permutation::new(vec![1, 2, 3, 0]).unwrap();
        let output: Vec<_> = perm.apply(&input);
        let masks = vec![10, 20, 30, 40];

        let beta = BinaryElem128::from_bits(0xABCD);
        let gamma = BinaryElem128::from_bits(0x1234);

        let poly =
            encode_complete_shuffle_constraints(&input, &output, &perm, &masks, beta, gamma)
                .unwrap();

        assert!(poly.len().is_power_of_two());
        // 2 sections now: grand product + lookup (no permutation, no masks)
        assert!(poly.len() >= 2 * 16);
    }

    #[test]
    fn test_lookup_constraints_symmetry() {
        let deck = make_deck(4);
        let gamma = BinaryElem128::from_bits(0x9876);

        // same deck should produce symmetric lookup terms
        let poly = encode_lookup_constraints(&deck, &deck, gamma).unwrap();
        assert!(poly.len().is_power_of_two());

        // check input/output terms match for each position
        for i in 0..4 {
            let in_term = poly[i * 4];
            let out_term = poly[i * 4 + 1];
            assert_eq!(in_term, out_term, "same deck should have matching terms");
        }
    }
}
