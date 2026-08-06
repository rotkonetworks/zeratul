// orchestrate.rs — high-level FROST multisig flows with signed envelopes
//
// everything both zcli (native) and zafu (wasm) need for DKG and signing.
// all inputs/outputs are hex-encoded strings for transport-agnostic use.
// all FROST round messages are wrapped in ed25519-signed envelopes.
//
// two signing modes:
//   - generic: sign arbitrary messages with random rerandomization (test/utility)
//   - spend: sign Orchard sighash with alpha as FROST randomizer (transactions)

use std::collections::BTreeMap;

use ed25519_consensus::SigningKey;
use rand_core::OsRng;

use crate::{
    dkg, frost, frost_keys, round1, round2, aggregate,
    Identifier, RandomizedParams, Randomizer, SigningPackage,
    message::{self, SignedMessage, identifier_from_vk},
};

// ── error ──

#[derive(Debug)]
pub enum Error {
    Frost(String),
    Serialize(String),
    Verify(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Frost(s) => write!(f, "frost: {}", s),
            Error::Serialize(s) => write!(f, "serialize: {}", s),
            Error::Verify(s) => write!(f, "verify: {}", s),
        }
    }
}

impl std::error::Error for Error {}

// ── hex serialization ──

pub fn to_hex<T: serde::Serialize>(val: &T) -> Result<String, Error> {
    let json = serde_json::to_vec(val)
        .map_err(|e| Error::Serialize(e.to_string()))?;
    Ok(hex::encode(json))
}

pub fn from_hex<T: serde::de::DeserializeOwned>(hex_str: &str) -> Result<T, Error> {
    let bytes = hex::decode(hex_str)
        .map_err(|e| Error::Serialize(format!("bad hex: {}", e)))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| Error::Serialize(format!("deserialize: {}", e)))
}

pub fn signing_key_from_seed(seed: &[u8; 32]) -> SigningKey {
    SigningKey::from(*seed)
}

fn id_from_vk(vk: &ed25519_consensus::VerificationKey) -> Result<Identifier, Error> {
    identifier_from_vk(vk).map_err(Error::Verify)
}

fn verify_signed(msg: &SignedMessage) -> Result<(ed25519_consensus::VerificationKey, &[u8]), Error> {
    msg.verify().map_err(Error::Verify)
}

fn load_ephemeral_sk(state: &serde_json::Value) -> Result<SigningKey, Error> {
    let seed_hex = state["ephemeral_seed"].as_str()
        .ok_or_else(|| Error::Serialize("missing ephemeral_seed".into()))?;
    let seed = hex::decode(seed_hex)
        .map_err(|e| Error::Serialize(format!("bad seed: {}", e)))?;
    let seed: [u8; 32] = seed.try_into()
        .map_err(|_| Error::Serialize("seed must be 32 bytes".into()))?;
    Ok(signing_key_from_seed(&seed))
}

fn build_signing_package_from_signed(
    message: &[u8],
    signed_commitments_hex: &[String],
) -> Result<SigningPackage, Error> {
    let mut commitment_map = BTreeMap::new();
    for hex_str in signed_commitments_hex {
        let signed: SignedMessage = from_hex(hex_str)?;
        let (vk, payload) = verify_signed(&signed)?;
        let pkg_hex: String = serde_json::from_slice(payload)
            .map_err(|e| Error::Serialize(format!("parse commitment: {}", e)))?;
        commitment_map.insert(id_from_vk(&vk)?, from_hex(&pkg_hex)?);
    }
    Ok(SigningPackage::new(commitment_map, message))
}

fn extract_signed_commitments(
    signed_commitments_hex: &[String],
) -> Result<BTreeMap<Identifier, round1::SigningCommitments>, Error> {
    let mut map = BTreeMap::new();
    for hex_str in signed_commitments_hex {
        let signed: SignedMessage = from_hex(hex_str)?;
        let (vk, payload) = verify_signed(&signed)?;
        let pkg_hex: String = serde_json::from_slice(payload)
            .map_err(|e| Error::Serialize(format!("parse commitment: {}", e)))?;
        map.insert(id_from_vk(&vk)?, from_hex(&pkg_hex)?);
    }
    Ok(map)
}

// ── dealer (trusted, generates everything) ──

pub struct DealerResult {
    pub packages: Vec<String>,
    pub public_key_package_hex: String,
}

pub fn dealer_keygen(min_signers: u16, max_signers: u16) -> Result<DealerResult, Error> {
    let dealer_sk = message::ephemeral_identity(&mut OsRng);

    let mut participants: Vec<(SigningKey, Identifier)> = Vec::new();
    for _ in 0..max_signers {
        let sk = message::ephemeral_identity(&mut OsRng);
        let id = id_from_vk(&sk.verification_key())?;
        participants.push((sk, id));
    }

    let identifiers: Vec<Identifier> = participants.iter().map(|(_, id)| *id).collect();
    let (shares, pubkeys) = frost_keys::generate_with_dealer(
        max_signers, min_signers,
        frost_keys::IdentifierList::Custom(&identifiers),
        OsRng,
    ).map_err(|e| Error::Frost(format!("dealer keygen: {}", e)))?;

    let public_key_package_hex = to_hex(&pubkeys)?;

    let mut packages = Vec::new();
    for (sk, id) in &participants {
        let share = shares.get(id)
            .ok_or_else(|| Error::Frost("missing share".into()))?;
        let key_pkg: frost_keys::KeyPackage = share.clone().try_into()
            .map_err(|e: frost::Error| Error::Frost(format!("share verify: {}", e)))?;

        let bundle = serde_json::json!({
            "ephemeral_seed": hex::encode(sk.as_bytes()),
            "key_package": to_hex(&key_pkg)?,
            "public_key_package": &public_key_package_hex,
        });
        let bundle_bytes = serde_json::to_vec(&bundle)
            .map_err(|e| Error::Serialize(e.to_string()))?;

        let signed = SignedMessage::sign(&dealer_sk, &bundle_bytes);
        packages.push(to_hex(&signed)?);
    }

    Ok(DealerResult { packages, public_key_package_hex })
}

// ── DKG (interactive, no trusted dealer) ──

pub struct Dkg1Result {
    pub secret_hex: String,
    pub broadcast_hex: String,
}

pub fn dkg_part1(max_signers: u16, min_signers: u16) -> Result<Dkg1Result, Error> {
    let sk = message::ephemeral_identity(&mut OsRng);
    let vk = sk.verification_key();
    let id = id_from_vk(&vk)?;

    let (secret, package) = dkg::part1(id, max_signers, min_signers, OsRng)
        .map_err(|e| Error::Frost(format!("dkg part1: {}", e)))?;

    let payload = serde_json::to_vec(&to_hex(&package)?)
        .map_err(|e| Error::Serialize(e.to_string()))?;
    let signed = SignedMessage::sign(&sk, &payload);

    let secret_state = serde_json::json!({
        "ephemeral_seed": hex::encode(sk.as_bytes()),
        "frost_secret": to_hex(&secret)?,
    });

    Ok(Dkg1Result {
        secret_hex: to_hex(&secret_state)?,
        broadcast_hex: to_hex(&signed)?,
    })
}

pub struct Dkg2Result {
    pub secret_hex: String,
    pub peer_packages: Vec<String>,
}

pub fn dkg_part2(secret_hex: &str, peer_broadcasts_hex: &[String]) -> Result<Dkg2Result, Error> {
    let secret_state: serde_json::Value = from_hex(secret_hex)?;
    let sk = load_ephemeral_sk(&secret_state)?;
    let frost_secret: dkg::round1::SecretPackage = from_hex(
        secret_state["frost_secret"].as_str()
            .ok_or_else(|| Error::Serialize("missing frost_secret".into()))?
    )?;

    let mut round1_pkgs = BTreeMap::new();
    for hex_str in peer_broadcasts_hex {
        let signed: SignedMessage = from_hex(hex_str)?;
        let (vk, payload) = verify_signed(&signed)?;
        let pkg_hex: String = serde_json::from_slice(payload)
            .map_err(|e| Error::Serialize(format!("parse: {}", e)))?;
        round1_pkgs.insert(id_from_vk(&vk)?, from_hex(&pkg_hex)?);
    }

    let (secret2, round2_pkgs) = dkg::part2(frost_secret, &round1_pkgs)
        .map_err(|e| Error::Frost(format!("dkg part2: {}", e)))?;

    let mut peer_packages = Vec::new();
    for (id, pkg) in &round2_pkgs {
        let payload = serde_json::to_vec(&serde_json::json!({
            "recipient": to_hex(id)?,
            "package": to_hex(pkg)?,
        })).map_err(|e| Error::Serialize(e.to_string()))?;
        peer_packages.push(to_hex(&SignedMessage::sign(&sk, &payload))?);
    }

    let secret2_state = serde_json::json!({
        "ephemeral_seed": hex::encode(sk.as_bytes()),
        "frost_secret": to_hex(&secret2)?,
    });

    Ok(Dkg2Result {
        secret_hex: to_hex(&secret2_state)?,
        peer_packages,
    })
}

pub struct Dkg3Result {
    pub key_package_hex: String,
    pub public_key_package_hex: String,
    pub ephemeral_seed_hex: String,
}

pub fn dkg_part3(
    secret_hex: &str,
    round1_broadcasts_hex: &[String],
    round2_packages_hex: &[String],
) -> Result<Dkg3Result, Error> {
    let secret_state: serde_json::Value = from_hex(secret_hex)?;
    let sk = load_ephemeral_sk(&secret_state)?;
    let frost_secret: dkg::round2::SecretPackage = from_hex(
        secret_state["frost_secret"].as_str()
            .ok_or_else(|| Error::Serialize("missing frost_secret".into()))?
    )?;

    let mut round1_pkgs = BTreeMap::new();
    for hex_str in round1_broadcasts_hex {
        let signed: SignedMessage = from_hex(hex_str)?;
        let (vk, payload) = verify_signed(&signed)?;
        let pkg_hex: String = serde_json::from_slice(payload)
            .map_err(|e| Error::Serialize(format!("parse: {}", e)))?;
        round1_pkgs.insert(id_from_vk(&vk)?, from_hex(&pkg_hex)?);
    }

    // derive our own identifier to filter round2 packages addressed to us
    let our_id = id_from_vk(&sk.verification_key())?;

    let mut round2_pkgs = BTreeMap::new();
    for hex_str in round2_packages_hex {
        let signed: SignedMessage = from_hex(hex_str)?;
        let (vk, payload) = verify_signed(&signed)?;
        let inner: serde_json::Value = serde_json::from_slice(payload)
            .map_err(|e| Error::Serialize(format!("parse: {}", e)))?;

        // only accept packages addressed to us
        let recipient_hex = inner["recipient"].as_str()
            .ok_or_else(|| Error::Serialize("missing recipient".into()))?;
        let recipient: Identifier = from_hex(recipient_hex)?;
        if recipient != our_id {
            continue; // not for us — skip
        }

        let pkg = from_hex(
            inner["package"].as_str()
                .ok_or_else(|| Error::Serialize("missing package".into()))?
        )?;
        round2_pkgs.insert(id_from_vk(&vk)?, pkg);
    }

    let (key_pkg, pub_pkg) = dkg::part3(&frost_secret, &round1_pkgs, &round2_pkgs)
        .map_err(|e| Error::Frost(format!("dkg part3: {}", e)))?;

    Ok(Dkg3Result {
        key_package_hex: to_hex(&key_pkg)?,
        public_key_package_hex: to_hex(&pub_pkg)?,
        ephemeral_seed_hex: hex::encode(sk.as_bytes()),
    })
}

// ── generic signing (arbitrary messages, random rerandomization) ──

pub fn sign_round1(
    ephemeral_seed: &[u8; 32],
    key_package_hex: &str,
) -> Result<(String, String), Error> {
    let sk = signing_key_from_seed(ephemeral_seed);
    let key_pkg: frost_keys::KeyPackage = from_hex(key_package_hex)?;
    let (nonces, commitments) = round1::commit(key_pkg.signing_share(), &mut OsRng);

    let payload = serde_json::to_vec(&to_hex(&commitments)?)
        .map_err(|e| Error::Serialize(e.to_string()))?;
    let signed = SignedMessage::sign(&sk, &payload);

    Ok((to_hex(&nonces)?, to_hex(&signed)?))
}

pub fn generate_randomizer(
    ephemeral_seed: &[u8; 32],
    message: &[u8],
    signed_commitments_hex: &[String],
) -> Result<String, Error> {
    let sk = signing_key_from_seed(ephemeral_seed);
    let signing_package = build_signing_package_from_signed(message, signed_commitments_hex)?;
    let randomizer = Randomizer::new(OsRng, &signing_package)
        .map_err(|e| Error::Frost(format!("randomizer: {}", e)))?;

    let payload = serde_json::to_vec(&to_hex(&randomizer)?)
        .map_err(|e| Error::Serialize(e.to_string()))?;
    to_hex(&SignedMessage::sign(&sk, &payload))
}

pub fn sign_round2(
    ephemeral_seed: &[u8; 32],
    key_package_hex: &str,
    nonces_hex: &str,
    message: &[u8],
    signed_commitments_hex: &[String],
    signed_randomizer_hex: &str,
) -> Result<String, Error> {
    let sk = signing_key_from_seed(ephemeral_seed);
    let key_pkg: frost_keys::KeyPackage = from_hex(key_package_hex)?;
    let nonces: round1::SigningNonces = from_hex(nonces_hex)?;

    let signed_rand: SignedMessage = from_hex(signed_randomizer_hex)?;
    let (_, rand_payload) = verify_signed(&signed_rand)?;
    let randomizer: Randomizer = from_hex(
        &serde_json::from_slice::<String>(rand_payload)
            .map_err(|e| Error::Serialize(format!("parse randomizer: {}", e)))?
    )?;

    let signing_package = build_signing_package_from_signed(message, signed_commitments_hex)?;

    let share = round2::sign(&signing_package, &nonces, &key_pkg, randomizer)
        .map_err(|e| Error::Frost(format!("sign round2: {}", e)))?;

    let payload = serde_json::to_vec(&to_hex(&share)?)
        .map_err(|e| Error::Serialize(e.to_string()))?;
    to_hex(&SignedMessage::sign(&sk, &payload))
}

pub fn aggregate_shares(
    public_key_package_hex: &str,
    message: &[u8],
    signed_commitments_hex: &[String],
    signed_shares_hex: &[String],
    signed_randomizer_hex: &str,
) -> Result<String, Error> {
    let pubkeys: frost_keys::PublicKeyPackage = from_hex(public_key_package_hex)?;

    let signed_rand: SignedMessage = from_hex(signed_randomizer_hex)?;
    let (_, rand_payload) = verify_signed(&signed_rand)?;
    let randomizer: Randomizer = from_hex(
        &serde_json::from_slice::<String>(rand_payload)
            .map_err(|e| Error::Serialize(format!("parse randomizer: {}", e)))?
    )?;

    let signing_package = build_signing_package_from_signed(message, signed_commitments_hex)?;
    let randomized_params = RandomizedParams::from_randomizer(
        pubkeys.verifying_key(), randomizer,
    );

    let mut share_map = BTreeMap::new();
    for hex_str in signed_shares_hex {
        let signed: SignedMessage = from_hex(hex_str)?;
        let (vk, payload) = verify_signed(&signed)?;
        let share = from_hex(
            &serde_json::from_slice::<String>(payload)
                .map_err(|e| Error::Serialize(format!("parse share: {}", e)))?
        )?;
        share_map.insert(id_from_vk(&vk)?, share);
    }

    let signature = aggregate(
        &signing_package, &share_map, &pubkeys, &randomized_params,
    ).map_err(|e| Error::Frost(format!("aggregate: {}", e)))?;

    to_hex(&signature)
}

// ── spend authorization (sighash-bound, alpha = FROST randomizer) ──

/// derive the raw Orchard address bytes for the FROST multisig wallet.
/// returns 43-byte raw address. caller is responsible for UA encoding.
pub fn derive_address_raw(
    public_key_package_hex: &str,
    diversifier_index: u32,
) -> Result<[u8; 43], Error> {
    let pubkeys: frost_keys::PublicKeyPackage = from_hex(public_key_package_hex)?;
    let fvk = crate::keys::derive_fvk(&mut OsRng, &pubkeys)
        .ok_or_else(|| Error::Frost("failed to derive FVK from group key".into()))?;
    let addr = crate::keys::derive_address(&fvk, diversifier_index);
    Ok(addr.to_raw_address_bytes())
}

/// sighash-bound signing round 2: produce FROST share for one Orchard action.
/// alpha (per-action randomizer from unsigned tx) IS the FROST randomizer.
/// share is wrapped in a SignedMessage for sender authentication.
pub fn spend_sign_round2(
    key_package_hex: &str,
    nonces_hex: &str,
    sighash: &[u8; 32],
    alpha: &[u8; 32],
    signed_commitments_hex: &[String],
) -> Result<String, Error> {
    let key_pkg: frost_keys::KeyPackage = from_hex(key_package_hex)?;
    let nonces: round1::SigningNonces = from_hex(nonces_hex)?;
    let commitment_map = extract_signed_commitments(signed_commitments_hex)?;

    let share = crate::sign::signer_round2(
        &key_pkg, &nonces, sighash, alpha, &commitment_map,
    ).map_err(|e| Error::Frost(format!("spend sign round2: {}", e)))?;

    to_hex(&share)
}

/// authenticated variant: wraps share in SignedMessage for identity binding.
/// use this when shares transit an untrusted channel (relay, memos).
pub fn spend_sign_round2_signed(
    ephemeral_seed: &[u8; 32],
    key_package_hex: &str,
    nonces_hex: &str,
    sighash: &[u8; 32],
    alpha: &[u8; 32],
    signed_commitments_hex: &[String],
) -> Result<String, Error> {
    let sk = signing_key_from_seed(ephemeral_seed);
    let key_pkg: frost_keys::KeyPackage = from_hex(key_package_hex)?;
    let nonces: round1::SigningNonces = from_hex(nonces_hex)?;
    let commitment_map = extract_signed_commitments(signed_commitments_hex)?;

    let share = crate::sign::signer_round2(
        &key_pkg, &nonces, sighash, alpha, &commitment_map,
    ).map_err(|e| Error::Frost(format!("spend sign round2: {}", e)))?;

    let payload = serde_json::to_vec(&to_hex(&share)?)
        .map_err(|e| Error::Serialize(e.to_string()))?;
    to_hex(&SignedMessage::sign(&sk, &payload))
}

/// coordinator: aggregate FROST shares into final SpendAuth signature [u8; 64].
/// this signature can be injected directly into the Orchard transaction.
///
/// accepts both raw shares (legacy, for local-only use) and signed shares
/// (authenticated, for relay/memo transport). signed shares are verified
/// and mapped by sender identity; raw shares use ordinal position.
pub fn spend_aggregate(
    public_key_package_hex: &str,
    sighash: &[u8; 32],
    alpha: &[u8; 32],
    signed_commitments_hex: &[String],
    shares_hex: &[String],
) -> Result<String, Error> {
    let pubkeys: frost_keys::PublicKeyPackage = from_hex(public_key_package_hex)?;
    let commitment_map = extract_signed_commitments(signed_commitments_hex)?;

    let mut share_map = BTreeMap::new();
    for hex_str in shares_hex {
        // try to decode as SignedMessage first (authenticated shares)
        if let Ok(signed) = from_hex::<SignedMessage>(hex_str) {
            if let Ok((vk, payload)) = verify_signed(&signed) {
                if let Ok(share_hex) = serde_json::from_slice::<String>(payload) {
                    if let Ok(share) = from_hex(&share_hex) {
                        share_map.insert(id_from_vk(&vk)?, share);
                        continue;
                    }
                }
            }
        }
        // fallback: raw share (legacy, positional mapping)
        let share = from_hex(hex_str)?;
        share_map.insert(
            *commitment_map.keys().nth(share_map.len())
                .ok_or_else(|| Error::Frost("more shares than commitments".into()))?,
            share,
        );
    }

    let sig_bytes = crate::sign::coordinator_aggregate(
        &pubkeys, sighash, alpha, &commitment_map, &share_map,
    ).map_err(|e| Error::Frost(format!("spend aggregate: {}", e)))?;

    Ok(hex::encode(sig_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// helper: extract ephemeral_seed and key_package from a dealer package
    fn unwrap_dealer_pkg(pkg_hex: &str) -> (([u8; 32]), String) {
        let signed: SignedMessage = from_hex(pkg_hex).unwrap();
        let bundle: serde_json::Value = serde_json::from_slice(&signed.payload).unwrap();
        let seed_hex = bundle["ephemeral_seed"].as_str().unwrap();
        let kp_hex = bundle["key_package"].as_str().unwrap();
        let seed = hex::decode(seed_hex).unwrap();
        let mut seed_arr = [0u8; 32];
        seed_arr.copy_from_slice(&seed);
        (seed_arr, kp_hex.to_string())
    }

    #[test]
    fn test_dealer_keygen_and_spend_sign_2of3() {
        // dealer generates 2-of-3 key packages
        let dealer = dealer_keygen(2, 3).expect("dealer keygen failed");
        assert_eq!(dealer.packages.len(), 3);

        let (seed1, kp1) = unwrap_dealer_pkg(&dealer.packages[0]);
        let (seed2, kp2) = unwrap_dealer_pkg(&dealer.packages[1]);

        // derive address — should produce valid 43-byte raw orchard address
        let addr = derive_address_raw(&dealer.public_key_package_hex, 0)
            .expect("derive address failed");
        assert_eq!(addr.len(), 43, "orchard address should be 43 bytes");

        // sighash can be any 32 bytes, alpha must be a valid Pallas scalar
        let sighash = [0xaa; 32];
        // use a small alpha that's definitely a valid scalar (< field modulus)
        let mut alpha = [0u8; 32];
        alpha[0] = 0x01; // small valid scalar

        // round 1: nonces + commitments
        let (nonces1, commitments1) = sign_round1(&seed1, &kp1).expect("p1 round1");
        let (nonces2, commitments2) = sign_round1(&seed2, &kp2).expect("p2 round1");

        let all_commitments = vec![commitments1.clone(), commitments2.clone()];

        // round 2: spend-auth shares (authenticated — wrapped in SignedMessage)
        let share1 = spend_sign_round2_signed(&seed1, &kp1, &nonces1, &sighash, &alpha, &all_commitments)
            .expect("p1 spend sign");
        let share2 = spend_sign_round2_signed(&seed2, &kp2, &nonces2, &sighash, &alpha, &all_commitments)
            .expect("p2 spend sign");

        // aggregate (verifies sender identity from SignedMessage, maps by FROST identifier)
        let sig = spend_aggregate(
            &dealer.public_key_package_hex, &sighash, &alpha,
            &all_commitments, &[share1, share2],
        ).expect("spend aggregate");

        assert_eq!(sig.len(), 128, "SpendAuth sig should be 64 bytes");
        eprintln!("2-of-3 FROST SpendAuth: {}...{}", &sig[..16], &sig[112..]);
    }

    #[test]
    fn test_full_dkg_and_sign() {
        // simulate 3-party DKG without a trusted dealer

        // round 1: all 3 participants
        let r1_a = dkg_part1(3, 2).expect("dkg part1 A");
        let r1_b = dkg_part1(3, 2).expect("dkg part1 B");
        let r1_c = dkg_part1(3, 2).expect("dkg part1 C");

        let all_broadcasts = vec![
            r1_a.broadcast_hex.clone(),
            r1_b.broadcast_hex.clone(),
            r1_c.broadcast_hex.clone(),
        ];

        // round 2: each participant processes OTHER participants' broadcasts
        let bc_for_a = vec![r1_b.broadcast_hex.clone(), r1_c.broadcast_hex.clone()];
        let bc_for_b = vec![r1_a.broadcast_hex.clone(), r1_c.broadcast_hex.clone()];
        let bc_for_c = vec![r1_a.broadcast_hex.clone(), r1_b.broadcast_hex.clone()];

        let r2_a = dkg_part2(&r1_a.secret_hex, &bc_for_a).expect("dkg part2 A");
        let r2_b = dkg_part2(&r1_b.secret_hex, &bc_for_b).expect("dkg part2 B");
        let r2_c = dkg_part2(&r1_c.secret_hex, &bc_for_c).expect("dkg part2 C");

        // pass ALL round2 packages to each participant — dkg_part3 filters by recipient
        let all_r2: Vec<String> = r2_a.peer_packages.iter()
            .chain(r2_b.peer_packages.iter())
            .chain(r2_c.peer_packages.iter())
            .cloned().collect();

        let r3_a = dkg_part3(&r2_a.secret_hex, &bc_for_a, &all_r2).expect("dkg part3 A");
        let r3_b = dkg_part3(&r2_b.secret_hex, &bc_for_b, &all_r2).expect("dkg part3 B");

        // all participants should derive the same public key package
        assert_eq!(r3_a.public_key_package_hex, r3_b.public_key_package_hex,
            "participants should agree on public key package");

        // derive address from DKG result
        let addr = derive_address_raw(&r3_a.public_key_package_hex, 0)
            .expect("derive address from DKG");
        assert_eq!(addr.len(), 43);

        // now sign with 2 of 3 (A and B)
        let sighash = [0xcc; 32];
        let mut alpha = [0u8; 32];
        alpha[0] = 0x02; // valid small Pallas scalar

        let seed_a = hex::decode(&r3_a.ephemeral_seed_hex).unwrap();
        let seed_b = hex::decode(&r3_b.ephemeral_seed_hex).unwrap();
        let mut seed_a_arr = [0u8; 32];
        let mut seed_b_arr = [0u8; 32];
        seed_a_arr.copy_from_slice(&seed_a);
        seed_b_arr.copy_from_slice(&seed_b);

        let (nonces_a, commit_a) = sign_round1(&seed_a_arr, &r3_a.key_package_hex)
            .expect("sign round1 A");
        let (nonces_b, commit_b) = sign_round1(&seed_b_arr, &r3_b.key_package_hex)
            .expect("sign round1 B");

        let all_commits = vec![commit_a.clone(), commit_b.clone()];

        let share_a = spend_sign_round2(
            &r3_a.key_package_hex, &nonces_a, &sighash, &alpha, &all_commits,
        ).expect("spend sign A");
        let share_b = spend_sign_round2(
            &r3_b.key_package_hex, &nonces_b, &sighash, &alpha, &all_commits,
        ).expect("spend sign B");

        let sig = spend_aggregate(
            &r3_a.public_key_package_hex,
            &sighash, &alpha,
            &all_commits,
            &[share_a, share_b],
        ).expect("spend aggregate");

        assert_eq!(sig.len(), 128);
        eprintln!("DKG + 2-of-3 FROST SpendAuth: {}...{}", &sig[..16], &sig[112..]);
    }
}
