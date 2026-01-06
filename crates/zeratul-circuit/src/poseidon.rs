//! poseidon hash gadget for binary field constraint systems
//!
//! this implements poseidon sponge construction optimized for our binary field.
//! follows the same structure as penumbra's poseidon377 but adapted for GF(2^32).
//!
//! ## design choices
//!
//! - state width: 3 field elements (rate=2, capacity=1) for 2:1 compression
//! - rounds: 8 full + 56 partial + 8 full (conservative for security)
//! - s-box: x^3 in binary field (x * x * x via MUL constraints)
//! - mds matrix: cauchy matrix for good diffusion
//!
//! ## constraint cost
//!
//! each s-box costs 2 MUL constraints (x^2 then x^2 * x)
//! full round: 3 s-boxes = 6 MUL constraints
//! partial round: 1 s-box = 2 MUL constraints
//! total: 8*6 + 56*2 + 8*6 = 48 + 112 + 48 = 208 MUL constraints per hash

use crate::constraint::{CircuitBuilder, WireId, Operand};

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

/// poseidon parameters for binary field GF(2^32)
pub struct PoseidonParams {
    /// number of full rounds at start
    pub rounds_f_beginning: usize,
    /// number of partial rounds
    pub rounds_p: usize,
    /// number of full rounds at end
    pub rounds_f_end: usize,
    /// state width
    pub width: usize,
    /// rate (absorb this many elements per permutation)
    pub rate: usize,
    /// round constants
    pub round_constants: Vec<u32>,
    /// mds matrix (width x width)
    pub mds: Vec<Vec<u32>>,
}

impl Default for PoseidonParams {
    fn default() -> Self {
        Self::new()
    }
}

impl PoseidonParams {
    /// create poseidon parameters for zeratul binary field
    pub fn new() -> Self {
        // conservative parameters for 128-bit security in binary field
        let rounds_f = 8;
        let rounds_p = 56;
        let width = 3;
        let rate = 2;

        // generate round constants deterministically
        // using a simple LFSR seeded with domain separator
        let total_constants = (rounds_f * 2 + rounds_p) * width;
        let round_constants = Self::generate_round_constants(total_constants);

        // cauchy mds matrix for width=3
        // M[i][j] = 1 / (x[i] + y[j]) where x,y are distinct field elements
        let mds = Self::generate_mds_matrix(width);

        Self {
            rounds_f_beginning: rounds_f,
            rounds_p,
            rounds_f_end: rounds_f,
            width,
            rate,
            round_constants,
            mds,
        }
    }

    /// generate round constants using SHAKE128 (XOF)
    ///
    /// CRITICAL: xorshift64 is NOT cryptographically secure
    /// attackers can predict constants and potentially find weaknesses
    ///
    /// we use SHAKE128 seeded with domain separator for:
    /// - 128-bit security level
    /// - nothing-up-my-sleeve generation
    /// - reproducible across implementations
    fn generate_round_constants(count: usize) -> Vec<u32> {
        use sha3::{Shake128, digest::{ExtendableOutput, Update, XofReader}};

        // domain separator for nothing-up-my-sleeve
        let seed = b"zeratul.poseidon.rc.v1";

        let mut hasher = Shake128::default();
        hasher.update(seed);

        // number of bytes needed: 4 bytes per u32 constant
        let num_bytes = count * 4;
        let mut output = vec![0u8; num_bytes];
        hasher.finalize_xof().read(&mut output);

        // convert bytes to u32 constants
        output
            .chunks_exact(4)
            .map(|chunk| u32::from_le_bytes(chunk.try_into().unwrap()))
            .collect()
    }

    /// generate MDS matrix using proper Cauchy construction
    ///
    /// CRITICAL: the MDS property ensures diffusion - every output depends
    /// on every input with maximum branch number
    ///
    /// for a Cauchy matrix M[i][j] = 1 / (x[i] + y[j]) where x,y are
    /// distinct elements. in GF(2^32), we need x[i] != y[j] for all i,j
    ///
    /// we use x = [1, 2, 3] and y = [4, 5, 6] for width=3
    /// then verify the matrix is MDS (all square submatrices are invertible)
    fn generate_mds_matrix(width: usize) -> Vec<Vec<u32>> {
        match width {
            3 => {
                // Cauchy matrix with x = [1, 2, 3], y = [4, 5, 6]
                // M[i][j] = gf32_inv(x[i] XOR y[j]) in GF(2^32)
                //
                // x XOR y values:
                // (1,4)=5, (1,5)=4, (1,6)=7
                // (2,4)=6, (2,5)=7, (2,6)=4
                // (3,4)=7, (3,5)=6, (3,6)=5
                //
                // compute inverses in GF(2^32)
                let mds = vec![
                    vec![gf32_inv(5), gf32_inv(4), gf32_inv(7)],
                    vec![gf32_inv(6), gf32_inv(7), gf32_inv(4)],
                    vec![gf32_inv(7), gf32_inv(6), gf32_inv(5)],
                ];

                // verify MDS property: all 2x2 and 3x3 submatrices must be invertible
                // a matrix is MDS iff all square submatrices have non-zero determinant
                debug_assert!(Self::verify_mds(&mds), "MDS matrix verification failed");

                mds
            },
            _ => {
                // for other widths, generate programmatically
                let x: Vec<u32> = (1..=width as u32).collect();
                let y: Vec<u32> = (width as u32 + 1..=2 * width as u32).collect();

                let mds: Vec<Vec<u32>> = x.iter().map(|&xi| {
                    y.iter().map(|&yj| {
                        gf32_inv(xi ^ yj)
                    }).collect()
                }).collect();

                debug_assert!(Self::verify_mds(&mds), "MDS matrix verification failed");

                mds
            }
        }
    }

    /// verify that a matrix is MDS (Maximum Distance Separable)
    /// all square submatrices must have non-zero determinant in GF(2^32)
    fn verify_mds(matrix: &[Vec<u32>]) -> bool {
        let n = matrix.len();

        // check all 1x1 submatrices (all entries non-zero)
        for row in matrix {
            for &val in row {
                if val == 0 {
                    return false;
                }
            }
        }

        // check all 2x2 submatrices
        for i1 in 0..n {
            for i2 in (i1+1)..n {
                for j1 in 0..n {
                    for j2 in (j1+1)..n {
                        // det = m[i1][j1] * m[i2][j2] - m[i1][j2] * m[i2][j1]
                        // in GF(2^32), - is same as + (XOR)
                        let det = gf32_mul(matrix[i1][j1], matrix[i2][j2])
                            ^ gf32_mul(matrix[i1][j2], matrix[i2][j1]);
                        if det == 0 {
                            return false;
                        }
                    }
                }
            }
        }

        // check full matrix determinant (for 3x3)
        if n == 3 {
            // det = a(ei - fh) - b(di - fg) + c(dh - eg)
            // in GF(2^32), all operations are XOR for addition
            let a = matrix[0][0]; let b = matrix[0][1]; let c = matrix[0][2];
            let d = matrix[1][0]; let e = matrix[1][1]; let f = matrix[1][2];
            let g = matrix[2][0]; let h = matrix[2][1]; let i = matrix[2][2];

            let det = gf32_mul(a, gf32_mul(e, i) ^ gf32_mul(f, h))
                ^ gf32_mul(b, gf32_mul(d, i) ^ gf32_mul(f, g))
                ^ gf32_mul(c, gf32_mul(d, h) ^ gf32_mul(e, g));

            if det == 0 {
                return false;
            }
        }

        true
    }
}

/// domain separators following penumbra's pattern
pub mod domain {
    use sha2::{Sha256, Digest};

    pub fn note_commitment() -> u32 {
        hash_domain(b"zeratul.notecommit")
    }

    pub fn nullifier() -> u32 {
        hash_domain(b"zeratul.nullifier")
    }

    pub fn merkle_node() -> u32 {
        hash_domain(b"zeratul.merkle.node")
    }

    pub fn merkle_leaf() -> u32 {
        hash_domain(b"zeratul.merkle.leaf")
    }

    fn hash_domain(tag: &[u8]) -> u32 {
        let hash = Sha256::digest(tag);
        u32::from_le_bytes(hash[0..4].try_into().unwrap())
    }
}

/// poseidon hash gadget that generates constraints
pub struct PoseidonGadget {
    params: PoseidonParams,
}

impl Default for PoseidonGadget {
    fn default() -> Self {
        Self::new()
    }
}

impl PoseidonGadget {
    pub fn new() -> Self {
        Self {
            params: PoseidonParams::new(),
        }
    }

    /// hash a single field element with domain separator
    /// returns the output wire
    pub fn hash_1(
        &self,
        builder: &mut CircuitBuilder,
        domain_sep: WireId,
        input: WireId,
    ) -> WireId {
        // initial state: [domain_sep, input, 0]
        let zero = builder.add_witness();
        builder.assert_const(zero, 0);

        let state = [domain_sep, input, zero];
        let final_state = self.permutation(builder, state);

        // output is first element of squeezed state
        final_state[0]
    }

    /// hash two field elements with domain separator
    pub fn hash_2(
        &self,
        builder: &mut CircuitBuilder,
        domain_sep: WireId,
        input1: WireId,
        input2: WireId,
    ) -> WireId {
        // absorb both inputs at once (rate=2)
        // state: [domain_sep + input1, input2, 0]
        let combined = self.field_add(builder, domain_sep, input1);
        let zero = builder.add_witness();
        builder.assert_const(zero, 0);

        let state = [combined, input2, zero];
        let final_state = self.permutation(builder, state);

        final_state[0]
    }

    /// hash three field elements with domain separator (for nullifier)
    pub fn hash_3(
        &self,
        builder: &mut CircuitBuilder,
        domain_sep: WireId,
        input1: WireId,
        input2: WireId,
        input3: WireId,
    ) -> WireId {
        // first absorb: [domain_sep + input1, input2, 0]
        let combined = self.field_add(builder, domain_sep, input1);
        let zero = builder.add_witness();
        builder.assert_const(zero, 0);

        let state = [combined, input2, zero];
        let mid_state = self.permutation(builder, state);

        // second absorb: add input3 to rate portion
        let state2 = [
            self.field_add(builder, mid_state[0], input3),
            mid_state[1],
            mid_state[2],
        ];
        let final_state = self.permutation(builder, state2);

        final_state[0]
    }

    /// hash six field elements with domain separator (for note commitment)
    pub fn hash_6(
        &self,
        builder: &mut CircuitBuilder,
        domain_sep: WireId,
        inputs: [WireId; 6],
    ) -> WireId {
        let zero = builder.add_witness();
        builder.assert_const(zero, 0);

        // absorb in pairs (rate=2)
        // round 1: [domain_sep + inputs[0], inputs[1], 0]
        let state = [
            self.field_add(builder, domain_sep, inputs[0]),
            inputs[1],
            zero,
        ];
        let state = self.permutation(builder, state);

        // round 2: add inputs[2], inputs[3]
        let state = [
            self.field_add(builder, state[0], inputs[2]),
            self.field_add(builder, state[1], inputs[3]),
            state[2],
        ];
        let state = self.permutation(builder, state);

        // round 3: add inputs[4], inputs[5]
        let state = [
            self.field_add(builder, state[0], inputs[4]),
            self.field_add(builder, state[1], inputs[5]),
            state[2],
        ];
        let final_state = self.permutation(builder, state);

        final_state[0]
    }

    /// full poseidon permutation
    fn permutation(
        &self,
        builder: &mut CircuitBuilder,
        initial_state: [WireId; 3],
    ) -> [WireId; 3] {
        let mut state = initial_state;
        let mut round_ctr = 0;

        // beginning full rounds
        for _ in 0..self.params.rounds_f_beginning {
            state = self.full_round(builder, state, round_ctr);
            round_ctr += self.params.width;
        }

        // partial rounds
        for _ in 0..self.params.rounds_p {
            state = self.partial_round(builder, state, round_ctr);
            round_ctr += self.params.width;
        }

        // ending full rounds
        for _ in 0..self.params.rounds_f_end {
            state = self.full_round(builder, state, round_ctr);
            round_ctr += self.params.width;
        }

        state
    }

    /// full round: add constants, apply s-box to all, multiply by mds
    fn full_round(
        &self,
        builder: &mut CircuitBuilder,
        state: [WireId; 3],
        round_ctr: usize,
    ) -> [WireId; 3] {
        // add round constants
        let mut state = state;
        for i in 0..3 {
            let rc = self.params.round_constants[round_ctr + i];
            state[i] = self.add_constant(builder, state[i], rc);
        }

        // apply s-box (x^3) to all elements
        for i in 0..3 {
            state[i] = self.sbox(builder, state[i]);
        }

        // mds matrix multiplication
        self.mds_multiply(builder, state)
    }

    /// partial round: add constants, apply s-box to first element only, multiply by mds
    fn partial_round(
        &self,
        builder: &mut CircuitBuilder,
        state: [WireId; 3],
        round_ctr: usize,
    ) -> [WireId; 3] {
        // add round constants
        let mut state = state;
        for i in 0..3 {
            let rc = self.params.round_constants[round_ctr + i];
            state[i] = self.add_constant(builder, state[i], rc);
        }

        // apply s-box only to first element
        state[0] = self.sbox(builder, state[0]);

        // mds matrix multiplication
        self.mds_multiply(builder, state)
    }

    /// s-box: x^3 in binary field GF(2^32)
    /// costs 2 FieldMul constraints
    ///
    /// CRITICAL: must use FieldMul (polynomial multiplication mod irreducible)
    /// NOT integer Mul - the poseidon security argument requires field arithmetic
    fn sbox(&self, builder: &mut CircuitBuilder, x: WireId) -> WireId {
        // x^2 in GF(2^32)
        let x2 = builder.add_witness();
        builder.assert_field_mul(x, x, x2);

        // x^3 = x^2 * x in GF(2^32)
        let x3 = builder.add_witness();
        builder.assert_field_mul(x2, x, x3);

        x3
    }

    /// mds matrix multiplication
    fn mds_multiply(
        &self,
        builder: &mut CircuitBuilder,
        state: [WireId; 3],
    ) -> [WireId; 3] {
        let mut result = [WireId(0); 3];

        for i in 0..3 {
            // result[i] = sum_j (mds[i][j] * state[j])
            let mut sum = builder.add_witness();
            builder.assert_const(sum, 0);

            for j in 0..3 {
                // mds[i][j] * state[j]
                let coeff = self.params.mds[i][j];
                let term = self.mul_constant(builder, state[j], coeff);
                sum = self.field_add(builder, sum, term);
            }

            result[i] = sum;
        }

        result
    }

    /// add a constant to a wire (uses XOR in binary field)
    fn add_constant(&self, builder: &mut CircuitBuilder, wire: WireId, constant: u32) -> WireId {
        let const_wire = builder.add_witness();
        builder.assert_const(const_wire, constant as u64);
        self.field_add(builder, wire, const_wire)
    }

    /// multiply wire by constant in GF(2^32)
    ///
    /// CRITICAL: uses FieldMul for proper field multiplication
    /// this is essential for MDS matrix multiplication security
    fn mul_constant(&self, builder: &mut CircuitBuilder, wire: WireId, constant: u32) -> WireId {
        let const_wire = builder.add_witness();
        builder.assert_const(const_wire, constant as u64);

        let result = builder.add_witness();
        builder.assert_field_mul(wire, const_wire, result);

        result
    }

    /// field addition (XOR in binary field)
    fn field_add(&self, builder: &mut CircuitBuilder, a: WireId, b: WireId) -> WireId {
        let result = builder.add_witness();
        builder.assert_xor(
            Operand::new().with_wire(a),
            Operand::new().with_wire(b),
            Operand::new().with_wire(result),
        );
        result
    }
}

/// compute poseidon hash outside of circuit (for witness generation)
pub fn poseidon_hash(domain_sep: u32, inputs: &[u32]) -> u32 {
    let params = PoseidonParams::new();

    // simple non-circuit implementation
    let mut state = [0u32; 3];

    // absorb domain separator and inputs
    state[0] = domain_sep;
    for (i, &input) in inputs.iter().enumerate() {
        let idx = i % 2;
        state[idx] ^= input;

        if idx == 1 || i == inputs.len() - 1 {
            state = poseidon_permutation(&params, state);
        }
    }

    state[0]
}

fn poseidon_permutation(params: &PoseidonParams, mut state: [u32; 3]) -> [u32; 3] {
    let mut round_ctr = 0;

    // full rounds beginning
    for _ in 0..params.rounds_f_beginning {
        state = poseidon_full_round(params, state, round_ctr);
        round_ctr += params.width;
    }

    // partial rounds
    for _ in 0..params.rounds_p {
        state = poseidon_partial_round(params, state, round_ctr);
        round_ctr += params.width;
    }

    // full rounds end
    for _ in 0..params.rounds_f_end {
        state = poseidon_full_round(params, state, round_ctr);
        round_ctr += params.width;
    }

    state
}

fn poseidon_full_round(params: &PoseidonParams, mut state: [u32; 3], round_ctr: usize) -> [u32; 3] {
    // add round constants
    for i in 0..3 {
        state[i] ^= params.round_constants[round_ctr + i];
    }

    // s-box to all
    for i in 0..3 {
        state[i] = sbox_native(state[i]);
    }

    // mds
    mds_multiply_native(params, state)
}

fn poseidon_partial_round(params: &PoseidonParams, mut state: [u32; 3], round_ctr: usize) -> [u32; 3] {
    // add round constants
    for i in 0..3 {
        state[i] ^= params.round_constants[round_ctr + i];
    }

    // s-box to first only
    state[0] = sbox_native(state[0]);

    // mds
    mds_multiply_native(params, state)
}

fn sbox_native(x: u32) -> u32 {
    // x^3 in GF(2^32)
    let x2 = gf32_mul(x, x);
    gf32_mul(x2, x)
}

fn gf32_mul(a: u32, b: u32) -> u32 {
    // polynomial multiplication in GF(2^32) with reduction
    // irreducible polynomial: x^32 + x^7 + x^3 + x^2 + 1
    let mut result: u64 = 0;
    let mut a64 = a as u64;
    let mut b64 = b as u64;

    for _ in 0..32 {
        if b64 & 1 != 0 {
            result ^= a64;
        }
        a64 <<= 1;
        b64 >>= 1;
    }

    // reduce modulo irreducible polynomial
    let irr: u64 = 0x1_0000_008D; // x^32 + x^7 + x^3 + x^2 + 1
    for i in (32..64).rev() {
        if result & (1 << i) != 0 {
            result ^= irr << (i - 32);
        }
    }

    result as u32
}

/// multiplicative inverse in GF(2^32) using extended euclidean algorithm
/// a^(-1) such that a * a^(-1) = 1
///
/// uses Fermat's little theorem: a^(-1) = a^(2^32 - 2) in GF(2^32)
/// which we compute via repeated squaring
fn gf32_inv(a: u32) -> u32 {
    if a == 0 {
        panic!("cannot invert zero in GF(2^32)");
    }

    // compute a^(2^32 - 2) via repeated squaring
    // 2^32 - 2 = 0xFFFFFFFE
    // binary: 11111111111111111111111111111110
    let mut result = 1u32;
    let mut base = a;
    let mut exp: u32 = 0xFFFFFFFE;

    while exp > 0 {
        if exp & 1 == 1 {
            result = gf32_mul(result, base);
        }
        base = gf32_mul(base, base);
        exp >>= 1;
    }

    // verify: a * result should equal 1
    debug_assert_eq!(gf32_mul(a, result), 1, "inverse verification failed");

    result
}

fn mds_multiply_native(params: &PoseidonParams, state: [u32; 3]) -> [u32; 3] {
    let mut result = [0u32; 3];

    for i in 0..3 {
        for j in 0..3 {
            result[i] ^= gf32_mul(params.mds[i][j], state[j]);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_domain_separators() {
        let nc = domain::note_commitment();
        let nf = domain::nullifier();

        // should be different
        assert_ne!(nc, nf);

        // should be deterministic
        assert_eq!(nc, domain::note_commitment());
    }

    #[test]
    fn test_gf32_mul() {
        // 1 * x = x
        assert_eq!(gf32_mul(1, 0x12345678), 0x12345678);

        // x * 1 = x
        assert_eq!(gf32_mul(0x12345678, 1), 0x12345678);

        // 0 * x = 0
        assert_eq!(gf32_mul(0, 0x12345678), 0);
    }

    #[test]
    fn test_poseidon_hash_deterministic() {
        let domain = domain::note_commitment();
        let inputs = [0x11111111u32, 0x22222222u32];

        let hash1 = poseidon_hash(domain, &inputs);
        let hash2 = poseidon_hash(domain, &inputs);

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_poseidon_hash_different_inputs() {
        let domain = domain::note_commitment();

        let hash1 = poseidon_hash(domain, &[0x11111111]);
        let hash2 = poseidon_hash(domain, &[0x22222222]);

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_poseidon_gadget_builds() {
        let gadget = PoseidonGadget::new();
        let mut builder = CircuitBuilder::new();

        // allocate inputs
        let domain = builder.add_witness();
        let input = builder.add_witness();

        // build hash constraints
        let _output = gadget.hash_1(&mut builder, domain, input);

        let circuit = builder.build();

        // should have substantial constraints (208 MUL + others)
        println!("poseidon hash_1 constraints: {}", circuit.constraints.len());
        println!("poseidon hash_1 wires: {}", circuit.num_wires);

        assert!(circuit.constraints.len() > 100);
    }

    #[test]
    fn test_poseidon_hash_6() {
        let gadget = PoseidonGadget::new();
        let mut builder = CircuitBuilder::new();

        let domain = builder.add_witness();
        let inputs: [WireId; 6] = [
            builder.add_witness(),
            builder.add_witness(),
            builder.add_witness(),
            builder.add_witness(),
            builder.add_witness(),
            builder.add_witness(),
        ];

        let _output = gadget.hash_6(&mut builder, domain, inputs);

        let circuit = builder.build();

        println!("poseidon hash_6 constraints: {}", circuit.constraints.len());
        println!("poseidon hash_6 wires: {}", circuit.num_wires);

        // 3 permutations worth of constraints
        assert!(circuit.constraints.len() > 300);
    }
}
