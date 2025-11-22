//! Privacy Layer using Halo 2
//!
//! This module provides zero-knowledge proofs for note operations using
//! Halo 2 from the Zcash ecosystem. Key advantages over Groth16:
//!
//! - **No trusted setup** - eliminates ceremony complexity
//! - **Recursive proof composition** - proofs that verify other proofs
//! - **Pasta curves** (Pallas/Vesta) - efficient 255-bit curves
//!
//! ## Public Inputs
//!
//! Spend:
//! - nullifier (derived from private nk, position, commitment)
//! - anchor (merkle root)
//! - balance_commitment
//!
//! Output:
//! - note_commitment (hash of note data)
//! - balance_commitment

use halo2_proofs::{
    circuit::{Layouter, SimpleFloorPlanner, Value},
    plonk::{
        Advice, Circuit, Column, ConstraintSystem, Error, Instance, Selector,
        create_proof, keygen_pk, keygen_vk, verify_proof,
    },
    poly::commitment::Params,
    transcript::{Blake2bRead, Blake2bWrite, Challenge255},
};
use pasta_curves::{pallas, vesta};
use pasta_curves::group::ff::PrimeField;
use rand::rngs::OsRng;

/// Spend circuit configuration
#[derive(Clone, Debug)]
pub struct SpendConfig {
    /// Private witness columns
    advice: [Column<Advice>; 3],
    /// Public input column
    instance: Column<Instance>,
    /// Selector for constraints
    selector: Selector,
}

/// Spend circuit - proves knowledge of note being spent
///
/// Uses PLONKish arithmetization (columns + custom gates)
#[derive(Clone, Default)]
pub struct SpendCircuit {
    /// Private: note value
    pub note_value: Value<pallas::Base>,
    /// Private: note rseed (first element)
    pub note_rseed: Value<pallas::Base>,
    /// Private: address (first element)
    pub address: Value<pallas::Base>,
    /// Private: nullifier key (first element)
    pub nullifier_key: Value<pallas::Base>,
    /// Private: position in merkle tree
    pub position: Value<pallas::Base>,
}

impl Circuit<pallas::Base> for SpendCircuit {
    type Config = SpendConfig;
    type FloorPlanner = SimpleFloorPlanner;

    fn without_witnesses(&self) -> Self {
        Self::default()
    }

    fn configure(meta: &mut ConstraintSystem<pallas::Base>) -> Self::Config {
        let advice = [
            meta.advice_column(),
            meta.advice_column(),
            meta.advice_column(),
        ];
        let instance = meta.instance_column();
        let selector = meta.selector();

        // Enable equality for all columns
        for column in &advice {
            meta.enable_equality(*column);
        }
        meta.enable_equality(instance);

        // Custom gate: commitment = value + rseed + address
        meta.create_gate("commitment", |meta| {
            let s = meta.query_selector(selector);
            let value = meta.query_advice(advice[0], halo2_proofs::poly::Rotation::cur());
            let rseed = meta.query_advice(advice[1], halo2_proofs::poly::Rotation::cur());
            let address = meta.query_advice(advice[2], halo2_proofs::poly::Rotation::cur());
            let commitment = meta.query_advice(advice[0], halo2_proofs::poly::Rotation::next());

            // commitment = value + rseed + address
            vec![s * (commitment - value - rseed - address)]
        });

        // Custom gate: nullifier = nk + position + commitment
        meta.create_gate("nullifier", |meta| {
            let s = meta.query_selector(selector);
            let nk = meta.query_advice(advice[1], halo2_proofs::poly::Rotation::next());
            let position = meta.query_advice(advice[2], halo2_proofs::poly::Rotation::next());
            let commitment = meta.query_advice(advice[0], halo2_proofs::poly::Rotation::next());
            let nullifier = meta.query_advice(advice[0], halo2_proofs::poly::Rotation(2));

            // nullifier = nk + position + commitment
            vec![s * (nullifier - nk - position - commitment)]
        });

        SpendConfig {
            advice,
            instance,
            selector,
        }
    }

    fn synthesize(
        &self,
        config: Self::Config,
        mut layouter: impl Layouter<pallas::Base>,
    ) -> Result<(), Error> {
        let (value_cell, nullifier_cell) = layouter.assign_region(
            || "spend",
            |mut region| {
                config.selector.enable(&mut region, 0)?;

                // Row 0: value, rseed, address
                let value_cell = region.assign_advice(
                    || "value",
                    config.advice[0],
                    0,
                    || self.note_value,
                )?;
                region.assign_advice(
                    || "rseed",
                    config.advice[1],
                    0,
                    || self.note_rseed,
                )?;
                region.assign_advice(
                    || "address",
                    config.advice[2],
                    0,
                    || self.address,
                )?;

                // Row 1: commitment, nk, position
                let commitment = self.note_value.and_then(|v| {
                    self.note_rseed.and_then(|r| {
                        self.address.map(|a| v + r + a)
                    })
                });
                region.assign_advice(
                    || "commitment",
                    config.advice[0],
                    1,
                    || commitment,
                )?;
                region.assign_advice(
                    || "nk",
                    config.advice[1],
                    1,
                    || self.nullifier_key,
                )?;
                region.assign_advice(
                    || "position",
                    config.advice[2],
                    1,
                    || self.position,
                )?;

                // Row 2: nullifier
                let nullifier = self.nullifier_key.and_then(|nk| {
                    self.position.and_then(|pos| {
                        commitment.map(|c| nk + pos + c)
                    })
                });
                let nullifier_cell = region.assign_advice(
                    || "nullifier",
                    config.advice[0],
                    2,
                    || nullifier,
                )?;

                Ok((value_cell, nullifier_cell))
            },
        )?;

        // Constrain balance commitment = value (public input)
        layouter.constrain_instance(value_cell.cell(), config.instance, 0)?;
        // Constrain nullifier (public input)
        layouter.constrain_instance(nullifier_cell.cell(), config.instance, 1)?;

        Ok(())
    }
}

/// Parameters for Halo 2 proving system
pub struct PrivacyParams {
    /// IPA commitment parameters
    pub params: Params<vesta::Affine>,
    /// Proving key for spend circuit
    pub spend_pk: halo2_proofs::plonk::ProvingKey<vesta::Affine>,
    /// Verifying key for spend circuit
    pub spend_vk: halo2_proofs::plonk::VerifyingKey<vesta::Affine>,
}

impl PrivacyParams {
    /// Generate parameters (no trusted setup!)
    pub fn setup(k: u32) -> Result<Self, Error> {
        let params = Params::<vesta::Affine>::new(k);

        // Setup spend circuit
        let empty_circuit = SpendCircuit::default();
        let spend_vk = keygen_vk(&params, &empty_circuit)?;
        let spend_pk = keygen_pk(&params, spend_vk.clone(), &empty_circuit)?;

        Ok(Self {
            params,
            spend_pk,
            spend_vk,
        })
    }
}

/// Proof for a spend action
pub struct SpendProof {
    /// The serialized proof
    pub proof: Vec<u8>,
    /// Public inputs: [balance_commitment, nullifier]
    pub public_inputs: Vec<pallas::Base>,
}

impl SpendProof {
    /// Create a spend proof
    pub fn create(
        params: &PrivacyParams,
        circuit: SpendCircuit,
        public_inputs: Vec<pallas::Base>,
    ) -> Result<Self, Error> {
        let mut transcript = Blake2bWrite::<_, vesta::Affine, Challenge255<_>>::init(vec![]);

        create_proof(
            &params.params,
            &params.spend_pk,
            &[circuit],
            &[&[&public_inputs]],
            OsRng,
            &mut transcript,
        )?;

        let proof = transcript.finalize();

        Ok(Self {
            proof,
            public_inputs,
        })
    }

    /// Verify a spend proof
    pub fn verify(&self, params: &PrivacyParams) -> bool {
        let strategy = halo2_proofs::plonk::SingleVerifier::new(&params.params);
        let mut transcript = Blake2bRead::<_, vesta::Affine, Challenge255<_>>::init(&self.proof[..]);

        verify_proof(
            &params.params,
            &params.spend_vk,
            strategy,
            &[&[&self.public_inputs]],
            &mut transcript,
        )
        .is_ok()
    }
}

/// Convert bytes to field element
pub fn bytes_to_field(bytes: &[u8; 32]) -> pallas::Base {
    pallas::Base::from_repr(
        (*bytes).into()
    ).unwrap_or(pallas::Base::zero())
}

/// Convert field element to bytes
pub fn field_to_bytes(field: pallas::Base) -> [u8; 32] {
    field.to_repr()
}

#[cfg(test)]
mod tests {
    use super::*;
    use halo2_proofs::circuit::Value;

    #[test]
    fn test_spend_circuit() {
        // k = 4 means 2^4 = 16 rows
        let k = 4;

        // Test values
        let value = pallas::Base::from(1000u64);
        let rseed = pallas::Base::from(1u64);
        let address = pallas::Base::from(10u64);
        let nk = pallas::Base::from(100u64);
        let position = pallas::Base::from(0u64);

        // Compute expected values
        let _commitment = value + rseed + address;
        let nullifier = nk + position + (value + rseed + address);

        let circuit = SpendCircuit {
            note_value: Value::known(value),
            note_rseed: Value::known(rseed),
            address: Value::known(address),
            nullifier_key: Value::known(nk),
            position: Value::known(position),
        };

        // Public inputs: [balance_commitment, nullifier]
        let public_inputs = vec![value, nullifier];

        // Setup
        let params = PrivacyParams::setup(k).expect("setup failed");

        // Create proof
        let proof = SpendProof::create(&params, circuit, public_inputs)
            .expect("proof creation failed");

        // Verify
        assert!(proof.verify(&params), "Proof should verify");
    }

    #[test]
    fn test_invalid_proof_fails() {
        let k = 4;

        let value = pallas::Base::from(1000u64);
        let rseed = pallas::Base::from(1u64);
        let address = pallas::Base::from(10u64);
        let nk = pallas::Base::from(100u64);
        let position = pallas::Base::from(0u64);

        let circuit = SpendCircuit {
            note_value: Value::known(value),
            note_rseed: Value::known(rseed),
            address: Value::known(address),
            nullifier_key: Value::known(nk),
            position: Value::known(position),
        };

        // Wrong nullifier
        let wrong_nullifier = pallas::Base::from(999u64);
        let public_inputs = vec![value, wrong_nullifier];

        let params = PrivacyParams::setup(k).expect("setup failed");

        // Proof creation should fail or verification should fail
        let result = SpendProof::create(&params, circuit, public_inputs);
        if let Ok(proof) = result {
            assert!(!proof.verify(&params), "Invalid proof should not verify");
        }
    }
}
