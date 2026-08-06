// sign.rs — FROST threshold signing bound to Orchard sighash
//
// the coordinator builds an unsigned transaction, extracts the sighash
// and per-action alpha randomizers, then distributes these to signers.
// each signer produces a FROST signature share over the sighash using
// alpha as the FROST randomizer. the coordinator aggregates shares
// into a final RedPallas signature per action.
//
// the resulting signatures are indistinguishable from single-signer
// Orchard SpendAuth signatures.

use std::collections::BTreeMap;

use rand_core::{CryptoRng, RngCore};

use crate::{
    frost, frost_keys, round1, round2,
    Identifier, RandomizedParams, Randomizer, SigningPackage,
};

/// data the coordinator sends to signers for one signing session.
/// contains everything a signer needs to verify what they're signing
/// and produce a FROST share.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SigningRequest {
    /// the transaction sighash (32 bytes) — what gets signed
    pub sighash: Vec<u8>,
    /// per-action alpha randomizers (one per orchard spend)
    pub alphas: Vec<Vec<u8>>,
    /// human-readable transaction summary for review-then-sign
    pub summary: TransactionSummary,
}

/// human-readable summary so signers know what they're authorizing
#[derive(serde::Serialize, serde::Deserialize)]
pub struct TransactionSummary {
    pub recipient: String,
    pub amount_zat: u64,
    pub fee_zat: u64,
    pub memo: Option<String>,
    pub num_actions: usize,
}

/// a signer's output for one action: the FROST signature share
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SigningResponse {
    pub shares: Vec<String>, // hex-encoded SignatureShare per action
}

/// produce FROST commitment for signing (round 1)
pub fn signer_round1(
    rng: &mut (impl RngCore + CryptoRng),
    key_package: &frost_keys::KeyPackage,
) -> (round1::SigningNonces, round1::SigningCommitments) {
    round1::commit(key_package.signing_share(), rng)
}

/// produce FROST signature share for one action (round 2)
///
/// alpha is the Orchard per-action randomizer from the unsigned transaction.
/// sighash is the transaction sighash (same for all actions).
pub fn signer_round2(
    key_package: &frost_keys::KeyPackage,
    nonces: &round1::SigningNonces,
    sighash: &[u8; 32],
    alpha_bytes: &[u8; 32],
    all_commitments: &BTreeMap<Identifier, round1::SigningCommitments>,
) -> Result<round2::SignatureShare, frost::Error> {
    // alpha IS the FROST randomizer — same mathematical operation
    // (added to the signing key: rsk = ask_share + alpha)
    let alpha = Randomizer::deserialize(alpha_bytes)
        .map_err(|_| frost::Error::MalformedSigningKey)?;

    let signing_package = SigningPackage::new(all_commitments.clone(), sighash);

    round2::sign(&signing_package, nonces, key_package, alpha)
}

/// aggregate FROST signature shares into a final RedPallas signature
/// that can be injected into the Orchard transaction.
pub fn coordinator_aggregate(
    pubkey_package: &frost_keys::PublicKeyPackage,
    sighash: &[u8; 32],
    alpha_bytes: &[u8; 32],
    all_commitments: &BTreeMap<Identifier, round1::SigningCommitments>,
    shares: &BTreeMap<Identifier, round2::SignatureShare>,
) -> Result<[u8; 64], frost::Error> {
    let alpha = Randomizer::deserialize(alpha_bytes)
        .map_err(|_| frost::Error::MalformedSigningKey)?;

    let signing_package = SigningPackage::new(all_commitments.clone(), sighash);

    let randomized_params = RandomizedParams::from_randomizer(
        pubkey_package.verifying_key(),
        alpha,
    );

    let signature = crate::aggregate(
        &signing_package, shares, pubkey_package, &randomized_params,
    )?;

    let sig_bytes = signature.serialize()
        .map_err(|_| frost::Error::MalformedSignature)?;
    let sig_array: [u8; 64] = sig_bytes.try_into()
        .map_err(|_| frost::Error::MalformedSignature)?;
    Ok(sig_array)
}
