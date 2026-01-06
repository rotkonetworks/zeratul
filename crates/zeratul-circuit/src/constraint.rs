//! constraint system for ligerito-based zk proofs
//!
//! adapts binius64's constraint model for binary field operations:
//! - and constraints: A & B ^ C = 0
//! - xor constraints: A ^ B ^ C = 0 (linear, "free")
//! - hash constraints: output = H(input) (via lookup or gadget)
//!
//! key difference from accidental_computer: witness is encoded as
//! multilinear polynomial, verifier sees only commitments + proofs,
//! never the raw witness values.

use ligerito_binary_fields::{BinaryElem32, BinaryFieldElement};

#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec, string::String};

/// wire index into witness vector
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WireId(pub usize);

impl WireId {
    pub fn index(self) -> usize {
        self.0
    }
}

/// operand in a constraint (xor combination of shifted wires)
#[derive(Debug, Clone, Default)]
pub struct Operand {
    /// wires xor'd together to form this operand
    pub terms: Vec<(WireId, ShiftOp)>,
}

impl Operand {
    pub fn new() -> Self {
        Self { terms: Vec::new() }
    }

    /// add a wire term
    pub fn with_wire(mut self, wire: WireId) -> Self {
        self.terms.push((wire, ShiftOp::None));
        self
    }

    /// add a shifted wire term
    pub fn with_shifted(mut self, wire: WireId, shift: ShiftOp) -> Self {
        self.terms.push((wire, shift));
        self
    }

    /// evaluate operand against witness (xor all terms)
    pub fn evaluate(&self, witness: &[u64]) -> u64 {
        self.terms.iter().fold(0u64, |acc, (wire, shift)| {
            let val = witness[wire.0];
            let shifted = shift.apply(val);
            acc ^ shifted
        })
    }

    /// evaluate as binary field element
    pub fn evaluate_field(&self, witness: &[BinaryElem32]) -> BinaryElem32 {
        self.terms.iter().fold(BinaryElem32::zero(), |acc, (wire, shift)| {
            let val = witness[wire.0];
            // for binary field shifts, we work on the underlying u32
            let shifted_bits = shift.apply(val.poly().value() as u64) as u32;
            let shifted = BinaryElem32::from(shifted_bits);
            acc.add(&shifted)
        })
    }
}

/// shift operation on a wire value
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShiftOp {
    /// no shift
    None,
    /// logical left shift
    Sll(u8),
    /// logical right shift
    Srl(u8),
    /// arithmetic right shift (preserves sign)
    Sar(u8),
}

impl Default for ShiftOp {
    fn default() -> Self {
        Self::None
    }
}

impl ShiftOp {
    /// apply shift to 64-bit value
    pub fn apply(self, value: u64) -> u64 {
        match self {
            ShiftOp::None => value,
            ShiftOp::Sll(n) => value << (n as u32),
            ShiftOp::Srl(n) => value >> (n as u32),
            ShiftOp::Sar(n) => ((value as i64) >> (n as u32)) as u64,
        }
    }
}

/// constraint types in the circuit
#[derive(Debug, Clone)]
pub enum Constraint {
    /// bitwise and: A & B ^ C = 0
    /// (non-linear, costs 1x)
    And { a: Operand, b: Operand, c: Operand },

    /// bitwise xor: A ^ B ^ C = 0
    /// (linear, "free" - just adds to witness polynomial)
    Xor { a: Operand, b: Operand, c: Operand },

    /// equality: A = B
    /// (linear constraint)
    Eq { a: Operand, b: Operand },

    /// integer multiplication: A * B = (hi, lo) as 128-bit result
    /// this is standard schoolbook multiplication in Z/2^64Z
    /// (non-linear, costs ~3-4x)
    Mul { a: Operand, b: Operand, hi: WireId, lo: WireId },

    /// GF(2^32) field multiplication: A * B = C in the binary field
    /// this is polynomial multiplication modulo the irreducible polynomial
    /// CRITICAL: distinct from integer Mul - field multiplication wraps differently
    ///
    /// in GF(2^32), multiplication is:
    ///   a(x) * b(x) mod p(x) where p(x) is the irreducible polynomial
    ///   for our field: p(x) = x^32 + x^7 + x^3 + x^2 + 1 (0x1_0000_008D)
    ///
    /// this is used in poseidon s-box for x^3 computation
    FieldMul { a: WireId, b: WireId, result: WireId },

    /// assert wire equals constant
    AssertConst { wire: WireId, value: u64 },

    /// range check: wire < 2^n
    /// IMPORTANT: for ZK soundness, this requires bit decomposition
    /// the verifier cannot just trust the prover's claim about range
    Range { wire: WireId, bits: u8 },

    /// range check with explicit bit decomposition for ZK soundness
    /// bits must satisfy: bits[i] in {0,1} and wire = sum(bits[i] * 2^i)
    /// this is the ZK-sound version of Range
    RangeDecomposed { wire: WireId, bits: Vec<WireId> },
}

impl Constraint {
    /// check if constraint is satisfied by witness
    pub fn check(&self, witness: &[u64]) -> bool {
        match self {
            Constraint::And { a, b, c } => {
                let va = a.evaluate(witness);
                let vb = b.evaluate(witness);
                let vc = c.evaluate(witness);
                (va & vb) ^ vc == 0
            }
            Constraint::Xor { a, b, c } => {
                let va = a.evaluate(witness);
                let vb = b.evaluate(witness);
                let vc = c.evaluate(witness);
                va ^ vb ^ vc == 0
            }
            Constraint::Eq { a, b } => {
                a.evaluate(witness) == b.evaluate(witness)
            }
            Constraint::Mul { a, b, hi, lo } => {
                let va = a.evaluate(witness) as u128;
                let vb = b.evaluate(witness) as u128;
                let product = va * vb;
                let vhi = witness[hi.0] as u128;
                let vlo = witness[lo.0] as u128;
                product == (vhi << 64) | vlo
            }
            Constraint::FieldMul { a, b, result } => {
                // GF(2^32) field multiplication
                let va = BinaryElem32::from(witness[a.0] as u32);
                let vb = BinaryElem32::from(witness[b.0] as u32);
                let vresult = BinaryElem32::from(witness[result.0] as u32);
                va.mul(&vb) == vresult
            }
            Constraint::AssertConst { wire, value } => {
                witness[wire.0] == *value
            }
            Constraint::Range { wire, bits } => {
                witness[wire.0] < (1u64 << *bits)
            }
            Constraint::RangeDecomposed { wire, bits } => {
                // verify each bit is 0 or 1
                for bit_wire in bits {
                    if witness[bit_wire.0] > 1 {
                        return false;
                    }
                }
                // verify wire = sum(bits[i] * 2^i)
                let mut reconstructed = 0u64;
                for (i, bit_wire) in bits.iter().enumerate() {
                    reconstructed |= witness[bit_wire.0] << i;
                }
                witness[wire.0] == reconstructed
            }
        }
    }

    /// check constraint on binary field witness
    pub fn check_field(&self, witness: &[BinaryElem32]) -> bool {
        match self {
            Constraint::And { a, b, c } => {
                let va = a.evaluate_field(witness);
                let vb = b.evaluate_field(witness);
                let vc = c.evaluate_field(witness);
                // and in binary field is multiplication
                va.mul(&vb).add(&vc) == BinaryElem32::zero()
            }
            Constraint::Xor { a, b, c } => {
                let va = a.evaluate_field(witness);
                let vb = b.evaluate_field(witness);
                let vc = c.evaluate_field(witness);
                va.add(&vb).add(&vc) == BinaryElem32::zero()
            }
            Constraint::Eq { a, b } => {
                a.evaluate_field(witness) == b.evaluate_field(witness)
            }
            // for integer mul, convert to u64 and check
            Constraint::Mul { .. } => {
                let witness_u64: Vec<u64> = witness.iter()
                    .map(|x| x.poly().value() as u64)
                    .collect();
                self.check(&witness_u64)
            }
            // field multiplication is native in binary field
            Constraint::FieldMul { a, b, result } => {
                let va = witness[a.0];
                let vb = witness[b.0];
                let vresult = witness[result.0];
                va.mul(&vb) == vresult
            }
            Constraint::AssertConst { wire, value } => {
                witness[wire.0].poly().value() as u64 == *value
            }
            Constraint::Range { wire, bits } => {
                (witness[wire.0].poly().value() as u64) < (1u64 << *bits)
            }
            Constraint::RangeDecomposed { wire, bits } => {
                // verify each bit is 0 or 1
                for bit_wire in bits {
                    let val = witness[bit_wire.0].poly().value();
                    if val > 1 {
                        return false;
                    }
                }
                // verify wire = sum(bits[i] * 2^i)
                let mut reconstructed = 0u32;
                for (i, bit_wire) in bits.iter().enumerate() {
                    reconstructed |= (witness[bit_wire.0].poly().value() as u32) << i;
                }
                witness[wire.0].poly().value() == reconstructed
            }
        }
    }
}

/// circuit builder for constructing constraint systems
#[derive(Debug, Clone)]
pub struct CircuitBuilder {
    /// number of witness wires
    num_wires: usize,
    /// number of public input wires
    num_public: usize,
    /// constraints
    constraints: Vec<Constraint>,
    /// wire labels for debugging
    #[cfg(feature = "std")]
    #[allow(dead_code)]
    labels: std::collections::HashMap<WireId, String>,
}

impl Default for CircuitBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl CircuitBuilder {
    pub fn new() -> Self {
        Self {
            num_wires: 0,
            num_public: 0,
            constraints: Vec::new(),
            #[cfg(feature = "std")]
            labels: std::collections::HashMap::new(),
        }
    }

    /// allocate a new witness wire
    pub fn add_witness(&mut self) -> WireId {
        let id = WireId(self.num_wires);
        self.num_wires += 1;
        id
    }

    /// allocate a new public input wire
    pub fn add_public(&mut self) -> WireId {
        let id = self.add_witness();
        self.num_public += 1;
        id
    }

    /// allocate multiple witness wires
    pub fn add_witnesses(&mut self, n: usize) -> Vec<WireId> {
        (0..n).map(|_| self.add_witness()).collect()
    }

    /// add a constraint
    pub fn add_constraint(&mut self, constraint: Constraint) {
        self.constraints.push(constraint);
    }

    /// assert A & B = C
    pub fn assert_and(&mut self, a: Operand, b: Operand, c: Operand) {
        self.add_constraint(Constraint::And { a, b, c });
    }

    /// assert A ^ B = C
    pub fn assert_xor(&mut self, a: Operand, b: Operand, c: Operand) {
        self.add_constraint(Constraint::Xor { a, b, c });
    }

    /// assert A = B
    pub fn assert_eq(&mut self, a: Operand, b: Operand) {
        self.add_constraint(Constraint::Eq { a, b });
    }

    /// assert wire equals constant
    pub fn assert_const(&mut self, wire: WireId, value: u64) {
        self.add_constraint(Constraint::AssertConst { wire, value });
    }

    /// assert wire < 2^bits (simple range check, not ZK-sound without decomposition)
    pub fn assert_range(&mut self, wire: WireId, bits: u8) {
        self.add_constraint(Constraint::Range { wire, bits });
    }

    /// assert wire < 2^bits with ZK-sound bit decomposition
    /// allocates `bits` new wires for the bit decomposition
    /// returns the allocated bit wires
    pub fn assert_range_decomposed(&mut self, wire: WireId, bits: u8) -> Vec<WireId> {
        let bit_wires: Vec<WireId> = (0..bits).map(|_| self.add_witness()).collect();
        self.add_constraint(Constraint::RangeDecomposed {
            wire,
            bits: bit_wires.clone(),
        });
        bit_wires
    }

    /// GF(2^32) field multiplication: a * b = result in the binary field
    pub fn assert_field_mul(&mut self, a: WireId, b: WireId, result: WireId) {
        self.add_constraint(Constraint::FieldMul { a, b, result });
    }

    /// build the circuit
    pub fn build(self) -> Circuit {
        Circuit {
            num_wires: self.num_wires,
            num_public: self.num_public,
            constraints: self.constraints,
        }
    }

    pub fn num_wires(&self) -> usize {
        self.num_wires
    }

    pub fn num_public(&self) -> usize {
        self.num_public
    }
}

/// compiled circuit ready for proving
#[derive(Debug, Clone)]
pub struct Circuit {
    pub num_wires: usize,
    pub num_public: usize,
    pub constraints: Vec<Constraint>,
}

impl Circuit {
    /// check all constraints against witness
    pub fn check(&self, witness: &[u64]) -> Result<(), usize> {
        if witness.len() < self.num_wires {
            return Err(0); // not enough witness values
        }
        for (i, constraint) in self.constraints.iter().enumerate() {
            if !constraint.check(witness) {
                return Err(i);
            }
        }
        Ok(())
    }

    /// check all constraints against binary field witness
    pub fn check_field(&self, witness: &[BinaryElem32]) -> Result<(), usize> {
        if witness.len() < self.num_wires {
            return Err(0);
        }
        for (i, constraint) in self.constraints.iter().enumerate() {
            if !constraint.check_field(witness) {
                return Err(i);
            }
        }
        Ok(())
    }

    /// get number of and constraints (main cost metric)
    pub fn num_and_constraints(&self) -> usize {
        self.constraints.iter()
            .filter(|c| matches!(c, Constraint::And { .. }))
            .count()
    }

    /// get number of integer mul constraints
    pub fn num_mul_constraints(&self) -> usize {
        self.constraints.iter()
            .filter(|c| matches!(c, Constraint::Mul { .. }))
            .count()
    }

    /// get number of field mul constraints
    pub fn num_field_mul_constraints(&self) -> usize {
        self.constraints.iter()
            .filter(|c| matches!(c, Constraint::FieldMul { .. }))
            .count()
    }

    /// get number of range decomposed constraints
    pub fn num_range_decomposed_constraints(&self) -> usize {
        self.constraints.iter()
            .filter(|c| matches!(c, Constraint::RangeDecomposed { .. }))
            .count()
    }
}

/// witness for a circuit execution
#[derive(Debug, Clone)]
pub struct Witness {
    /// wire values (u64)
    pub values: Vec<u64>,
    /// public input indices
    pub public_indices: Vec<usize>,
}

impl Witness {
    pub fn new(num_wires: usize, num_public: usize) -> Self {
        Self {
            values: vec![0u64; num_wires],
            public_indices: (0..num_public).collect(),
        }
    }

    /// set wire value
    pub fn set(&mut self, wire: WireId, value: u64) {
        self.values[wire.0] = value;
    }

    /// get wire value
    pub fn get(&self, wire: WireId) -> u64 {
        self.values[wire.0]
    }

    /// convert to binary field elements
    pub fn to_field(&self) -> Vec<BinaryElem32> {
        self.values.iter()
            .map(|&v| BinaryElem32::from(v as u32))
            .collect()
    }

    /// get public inputs
    pub fn public_inputs(&self) -> Vec<u64> {
        self.public_indices.iter()
            .map(|&i| self.values[i])
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_and_constraint() {
        let mut builder = CircuitBuilder::new();
        let a = builder.add_witness();
        let b = builder.add_witness();
        let c = builder.add_witness();

        // a & b = c
        builder.assert_and(
            Operand::new().with_wire(a),
            Operand::new().with_wire(b),
            Operand::new().with_wire(c),
        );

        let circuit = builder.build();

        // valid: 0b1010 & 0b1100 = 0b1000
        let valid = [0b1010u64, 0b1100, 0b1000];
        assert!(circuit.check(&valid).is_ok());

        // invalid: wrong result
        let invalid = [0b1010u64, 0b1100, 0b1111];
        assert!(circuit.check(&invalid).is_err());
    }

    #[test]
    fn test_xor_constraint() {
        let mut builder = CircuitBuilder::new();
        let a = builder.add_witness();
        let b = builder.add_witness();
        let c = builder.add_witness();

        // a ^ b ^ c = 0 (meaning a ^ b = c)
        builder.assert_xor(
            Operand::new().with_wire(a),
            Operand::new().with_wire(b),
            Operand::new().with_wire(c),
        );

        let circuit = builder.build();

        // valid: 5 ^ 3 = 6
        let valid = [5u64, 3, 6];
        assert!(circuit.check(&valid).is_ok());

        // invalid
        let invalid = [5u64, 3, 7];
        assert!(circuit.check(&invalid).is_err());
    }

    #[test]
    fn test_shift_operand() {
        let mut builder = CircuitBuilder::new();
        let a = builder.add_witness();
        let c = builder.add_witness();

        // (a << 1) ^ c = 0
        builder.assert_xor(
            Operand::new().with_shifted(a, ShiftOp::Sll(1)),
            Operand::new(),
            Operand::new().with_wire(c),
        );

        let circuit = builder.build();

        // valid: (5 << 1) = 10
        let valid = [5u64, 10];
        assert!(circuit.check(&valid).is_ok());

        // invalid
        let invalid = [5u64, 11];
        assert!(circuit.check(&invalid).is_err());
    }

    #[test]
    fn test_witness() {
        let mut witness = Witness::new(3, 1);
        witness.set(WireId(0), 42);
        witness.set(WireId(1), 100);
        witness.set(WireId(2), 200);

        assert_eq!(witness.get(WireId(0)), 42);
        assert_eq!(witness.public_inputs(), vec![42]);
    }

    #[test]
    fn test_complex_circuit() {
        let mut builder = CircuitBuilder::new();

        // public inputs
        let pub_a = builder.add_public();
        let pub_b = builder.add_public();

        // witness
        let w = builder.add_witness();

        // constraint: pub_a & pub_b = w
        builder.assert_and(
            Operand::new().with_wire(pub_a),
            Operand::new().with_wire(pub_b),
            Operand::new().with_wire(w),
        );

        let circuit = builder.build();
        assert_eq!(circuit.num_wires, 3);
        assert_eq!(circuit.num_public, 2);
        assert_eq!(circuit.num_and_constraints(), 1);

        let valid = [0xFF00u64, 0x0FF0, 0x0F00];
        assert!(circuit.check(&valid).is_ok());
    }
}
