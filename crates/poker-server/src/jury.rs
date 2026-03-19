//! jury service — sign settlement payloads via nested FROST
//!
//! two implementations:
//! - LocalJury: all shares in-process (testing/demo)
//! - NarsilJury: calls narsild validators over HTTP (production)
//!
//! poker-server doesn't know which — it calls `jury.sign(message)`.

use async_trait::async_trait;
use osst::curve::{OsstPoint, OsstScalar};
use osst::frost;
use osst::nested;
use osst::SecretShare;
use pasta_curves::pallas::{Point as PallasPoint, Scalar as PallasScalar};
use pasta_curves::group::{ff::{Field, PrimeField}, Group, GroupEncoding};
use sha2::{Digest, Sha256, Sha512};

/// result of a jury signing operation
#[derive(Clone, Debug)]
pub struct JurySignature {
    pub r: PallasPoint,
    pub s: PallasScalar,
    pub verified: bool,
}

/// jury service: message in, signature out
#[async_trait]
pub trait JuryService: Send + Sync {
    /// sign a settlement payload. returns None if threshold not met.
    async fn sign(
        &self,
        message: &[u8],
        buyer_share: &SecretShare<PallasScalar>,
    ) -> Option<JurySignature>;
}

// ---------------------------------------------------------------------------
// LocalJury: all shares in-process (demo/testing)
// ---------------------------------------------------------------------------

pub struct LocalJury {
    pub shares: Vec<SecretShare<PallasScalar>>,
    pub threshold: u32,
    pub group_pubkey: PallasPoint,
    pub outer_group_pubkey: PallasPoint,
    pub outer_index: u32,
}

#[async_trait]
impl JuryService for LocalJury {
    async fn sign(
        &self,
        message: &[u8],
        buyer_share: &SecretShare<PallasScalar>,
    ) -> Option<JurySignature> {
        let mut rng = rand::thread_rng();
        let active_indices: Vec<u32> = self.shares[..self.threshold as usize]
            .iter().map(|s| s.index).collect();

        // inner commitment round
        let mut inner_nonces = Vec::new();
        let mut inner_commitments = Vec::new();
        for &k in &active_indices {
            let (nonces, commitments) = nested::inner_commit::<PallasPoint, _>(k, &mut rng);
            inner_nonces.push(nonces);
            inner_commitments.push(commitments);
        }

        let r_nested = nested::aggregate_inner_commitments(&inner_commitments, message);

        // buyer commits for outer FROST
        let (buyer_nonces, buyer_frost_commits) =
            frost::commit::<PallasPoint, _>(buyer_share.index, &mut rng);

        let jury_outer_commits = frost::SigningCommitments {
            index: self.outer_index,
            hiding: r_nested,
            binding: <PallasPoint as Group>::identity(),
        };

        let outer_package = frost::SigningPackage::new(
            message.to_vec(),
            vec![buyer_frost_commits, jury_outer_commits],
        ).ok()?;

        let buyer_sig = frost::sign::<PallasPoint>(
            &outer_package, buyer_nonces, buyer_share, &self.outer_group_pubkey,
        ).ok()?;

        // compute outer params for inner holders
        let outer_indices = outer_package.signer_indices();
        let outer_lambda = osst::compute_lagrange_coefficients::<PallasScalar>(&outer_indices).ok()?;
        let nested_pos = outer_indices.iter().position(|&i| i == self.outer_index)?;

        let outer_gc = {
            let mut r = <PallasPoint as Group>::identity();
            for &idx in &outer_indices {
                let c = outer_package.get_commitments(idx)?;
                let rho = compute_outer_binding(idx, message, &outer_package);
                r = r.add(&c.hiding).add(&c.binding.mul_scalar(&rho));
            }
            r
        };

        let outer_challenge = {
            let mut h = Sha512::new();
            h.update(b"frost-challenge-v1");
            h.update(OsstPoint::compress(&outer_gc));
            h.update(OsstPoint::compress(&self.outer_group_pubkey));
            h.update(message);
            let hash: [u8; 64] = h.finalize().into();
            <PallasScalar as OsstScalar>::from_bytes_wide(&hash)
        };

        let params = nested::InnerSigningParams {
            outer_challenge,
            outer_lambda: outer_lambda[nested_pos],
        };

        let mut inner_sigs = Vec::new();
        for (nonces, &k) in inner_nonces.into_iter().zip(active_indices.iter()) {
            let share = &self.shares[(k - 1) as usize];
            let sig = nested::inner_sign::<PallasPoint>(
                nonces, share, &params, &inner_commitments, &active_indices, message,
            ).ok()?;
            inner_sigs.push(sig);
        }

        let z_nested = nested::aggregate_inner_shares(&inner_sigs);
        let jury_sig_share = frost::SignatureShare {
            index: self.outer_index,
            response: z_nested,
        };

        let signature = frost::aggregate::<PallasPoint>(
            &outer_package,
            &[buyer_sig, jury_sig_share],
            &self.outer_group_pubkey,
            None,
        ).ok()?;

        let verified = frost::verify_signature(&self.outer_group_pubkey, message, &signature);

        Some(JurySignature {
            r: signature.r,
            s: signature.z,
            verified,
        })
    }
}

// ---------------------------------------------------------------------------
// NarsilJury: calls narsild validators over HTTP (production)
// ---------------------------------------------------------------------------

pub struct NarsilJury {
    pub endpoint: String,
    pub outer_group_pubkey: PallasPoint,
    pub outer_index: u32,
    client: reqwest::Client,
}

impl NarsilJury {
    pub fn new(endpoint: &str, outer_group_pubkey: PallasPoint, outer_index: u32) -> Self {
        Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            outer_group_pubkey,
            outer_index,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl JuryService for NarsilJury {
    async fn sign(
        &self,
        message: &[u8],
        buyer_share: &SecretShare<PallasScalar>,
    ) -> Option<JurySignature> {
        // pre-generate buyer nonces before any awaits (ThreadRng is not Send)
        let (buyer_nonces, buyer_frost_commits) = {
            let mut rng = rand::thread_rng();
            frost::commit::<PallasPoint, _>(buyer_share.index, &mut rng)
        };

        // session ID from message hash
        let session_id = {
            let mut h = Sha256::new();
            h.update(b"narsil.session.v1");
            h.update(message);
            let r: [u8; 32] = h.finalize().into();
            r
        };

        // --- round 1: get inner commitments from narsild ---

        let _: serde_json::Value = self.client
            .post(format!("{}/sign/round1", self.endpoint))
            .json(&serde_json::json!({
                "session_id": session_id,
                "message_hex": hex::encode(message),
            }))
            .send().await.ok()?
            .json().await.ok()?;

        // poll until ready
        let commitments = poll_until(|| async {
            let resp: serde_json::Value = self.client
                .post(format!("{}/sign/status", self.endpoint))
                .json(&serde_json::json!({"session_id": session_id}))
                .send().await.ok()?
                .json().await.ok()?;

            let ready = resp.pointer("/round1/ready")?.as_bool()?;
            if !ready { return None; }

            let list: Vec<serde_json::Value> = resp.get("commitments")?
                .as_array()?.clone();
            Some(list)
        }).await?;

        // compute R_nested from commitment list
        let r_nested = compute_r_nested_from_json(&commitments, message);

        let jury_outer_commits = frost::SigningCommitments {
            index: self.outer_index,
            hiding: r_nested,
            binding: <PallasPoint as Group>::identity(),
        };

        let outer_package = frost::SigningPackage::new(
            message.to_vec(),
            vec![buyer_frost_commits, jury_outer_commits],
        ).ok()?;

        // buyer signs their share
        let buyer_sig = frost::sign::<PallasPoint>(
            &outer_package, buyer_nonces, buyer_share, &self.outer_group_pubkey,
        ).ok()?;

        // compute outer challenge + lambda for nested position
        let outer_indices = outer_package.signer_indices();
        let outer_lambda_vec = osst::compute_lagrange_coefficients::<PallasScalar>(&outer_indices).ok()?;
        let nested_pos = outer_indices.iter().position(|&i| i == self.outer_index)?;

        let outer_gc = {
            let mut r = <PallasPoint as Group>::identity();
            for &idx in &outer_indices {
                let c = outer_package.get_commitments(idx)?;
                let rho = compute_outer_binding(idx, message, &outer_package);
                r = r.add(&c.hiding).add(&c.binding.mul_scalar(&rho));
            }
            r
        };

        let outer_challenge = {
            let mut h = Sha512::new();
            h.update(b"frost-challenge-v1");
            h.update(OsstPoint::compress(&outer_gc));
            h.update(OsstPoint::compress(&self.outer_group_pubkey));
            h.update(message);
            let hash: [u8; 64] = h.finalize().into();
            <PallasScalar as OsstScalar>::from_bytes_wide(&hash)
        };

        let outer_lambda = outer_lambda_vec[nested_pos];
        let active_indices: Vec<u32> = commitments.iter()
            .filter_map(|c| c.get("holder_index")?.as_u64().map(|v| v as u32))
            .collect();

        // --- round 2: send outer params, get z_nested ---

        let resp: serde_json::Value = self.client
            .post(format!("{}/sign/round2", self.endpoint))
            .json(&serde_json::json!({
                "session_id": session_id,
                "outer_challenge_hex": scalar_to_hex(&outer_challenge),
                "outer_lambda_hex": scalar_to_hex(&outer_lambda),
                "active_indices": active_indices,
            }))
            .send().await.ok()?
            .json().await.ok()?;

        // get z_nested (might need polling)
        let z_hex = if let Some(z) = resp.get("z_nested").and_then(|v| v.as_str()) {
            z.to_string()
        } else {
            poll_until(|| async {
                let resp: serde_json::Value = self.client
                    .post(format!("{}/sign/status", self.endpoint))
                    .json(&serde_json::json!({"session_id": session_id}))
                    .send().await.ok()?
                    .json().await.ok()?;
                resp.pointer("/round2/z_nested")?.as_str().map(|s| s.to_string())
            }).await?
        };

        let z_nested = scalar_from_hex(&z_hex)?;

        let jury_sig_share = frost::SignatureShare {
            index: self.outer_index,
            response: z_nested,
        };

        let signature = frost::aggregate::<PallasPoint>(
            &outer_package,
            &[buyer_sig, jury_sig_share],
            &self.outer_group_pubkey,
            None,
        ).ok()?;

        let verified = frost::verify_signature(&self.outer_group_pubkey, message, &signature);

        if verified {
            tracing::info!("narsil jury signature verified (R={})",
                hex::encode(&OsstPoint::compress(&signature.r)[..8]));
        } else {
            tracing::error!("narsil jury signature FAILED verification");
        }

        Some(JurySignature {
            r: signature.r,
            s: signature.z,
            verified,
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn compute_outer_binding(
    index: u32,
    message: &[u8],
    package: &frost::SigningPackage<PallasPoint>,
) -> PallasScalar {
    let mut encoded = Vec::new();
    for idx in package.signer_indices() {
        let c = package.get_commitments(idx).unwrap();
        encoded.extend_from_slice(&c.index.to_le_bytes());
        encoded.extend_from_slice(&c.hiding.compress());
        encoded.extend_from_slice(&c.binding.compress());
    }
    let mut h = Sha512::new();
    h.update(b"frost-binding-v1");
    h.update(index.to_le_bytes());
    h.update((message.len() as u64).to_le_bytes());
    h.update(message);
    h.update(&encoded);
    <PallasScalar as OsstScalar>::from_bytes_wide(&h.finalize().into())
}

fn compute_r_nested_from_json(commitments: &[serde_json::Value], message: &[u8]) -> PallasPoint {
    let mut r_agg = <PallasPoint as Group>::identity();
    for c in commitments {
        let idx = c.get("holder_index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let hiding = hex::decode(c.get("hiding").and_then(|v| v.as_str()).unwrap_or("")).unwrap_or_default();
        let binding = hex::decode(c.get("binding").and_then(|v| v.as_str()).unwrap_or("")).unwrap_or_default();

        let d_k = point_from_bytes(&hiding);
        let e_k = point_from_bytes(&binding);

        let rho = {
            let mut h = Sha512::new();
            h.update(b"frostito-inner-bind");
            h.update(idx.to_le_bytes());
            h.update((message.len() as u64).to_le_bytes());
            h.update(message);
            for cc in commitments {
                let ci = cc.get("holder_index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let ch = hex::decode(cc.get("hiding").and_then(|v| v.as_str()).unwrap_or("")).unwrap_or_default();
                let cb = hex::decode(cc.get("binding").and_then(|v| v.as_str()).unwrap_or("")).unwrap_or_default();
                h.update(ci.to_le_bytes());
                h.update(&ch);
                h.update(&cb);
            }
            <PallasScalar as OsstScalar>::from_bytes_wide(&h.finalize().into())
        };

        r_agg = r_agg.add(&d_k).add(&e_k.mul_scalar(&rho));
    }
    r_agg
}

fn point_from_bytes(bytes: &[u8]) -> PallasPoint {
    if bytes.len() != 32 { return <PallasPoint as Group>::identity(); }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(bytes);
    let ct = PallasPoint::from_bytes(&arr.into());
    if bool::from(ct.is_some()) { ct.unwrap() } else { <PallasPoint as Group>::identity() }
}

fn scalar_from_hex(hex_str: &str) -> Option<PallasScalar> {
    let bytes = hex::decode(hex_str).ok()?;
    if bytes.len() != 32 { return None; }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    let ct = PallasScalar::from_repr(arr.into());
    if bool::from(ct.is_some()) { Some(ct.unwrap()) } else { None }
}

fn scalar_to_hex(s: &PallasScalar) -> String {
    hex::encode(s.to_repr().as_ref())
}

/// poll a future until it returns Some, with 500ms intervals, 30 attempts
async fn poll_until<F, Fut, T>(f: F) -> Option<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if let Some(result) = f().await {
            return Some(result);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use osst::redpallas::zcash as redpallas;

    const JURY_N: u32 = 5;
    const JURY_T: u32 = 3;
    const JURY_OUTER_INDEX: u32 = 3;

    #[tokio::test]
    async fn test_local_jury_sign() {
        let mut rng = rand::thread_rng();
        let (player_a_share, _player_b_share, jury_network, _group_pubkey) =
            redpallas::setup_escrow(JURY_N, JURY_T, &mut rng)
                .expect("setup_escrow should succeed");

        let jury = LocalJury {
            shares: jury_network.node_shares,
            threshold: JURY_T,
            group_pubkey: jury_network.outer_verification_share,
            outer_group_pubkey: jury_network.outer_group_pubkey,
            outer_index: JURY_OUTER_INDEX,
        };

        let message = b"settlement: player A wins 500 chips";
        let result = jury.sign(message, &player_a_share).await;

        assert!(result.is_some(), "LocalJury should produce a signature");
        let sig = result.unwrap();
        assert!(sig.verified, "signature must verify");
    }

    #[tokio::test]
    async fn test_local_jury_different_messages() {
        let mut rng = rand::thread_rng();
        let (player_a_share, _player_b_share, jury_network, _group_pubkey) =
            redpallas::setup_escrow(JURY_N, JURY_T, &mut rng)
                .expect("setup_escrow should succeed");

        let jury = LocalJury {
            shares: jury_network.node_shares,
            threshold: JURY_T,
            group_pubkey: jury_network.outer_verification_share,
            outer_group_pubkey: jury_network.outer_group_pubkey,
            outer_index: JURY_OUTER_INDEX,
        };

        // sign two different messages — should produce different signatures
        let sig1 = jury.sign(b"hand 1: player A wins", &player_a_share).await.unwrap();
        let sig2 = jury.sign(b"hand 2: player B wins", &player_a_share).await.unwrap();

        assert!(sig1.verified);
        assert!(sig2.verified);
        assert_ne!(
            OsstPoint::compress(&sig1.r),
            OsstPoint::compress(&sig2.r),
            "different messages should produce different R values"
        );
    }

    #[tokio::test]
    async fn test_local_jury_as_trait_object() {
        let mut rng = rand::thread_rng();
        let (player_a_share, _player_b_share, jury_network, _group_pubkey) =
            redpallas::setup_escrow(JURY_N, JURY_T, &mut rng)
                .expect("setup_escrow should succeed");

        // use as Arc<dyn JuryService> — same as poker-server does
        let jury: std::sync::Arc<dyn JuryService> = std::sync::Arc::new(LocalJury {
            shares: jury_network.node_shares,
            threshold: JURY_T,
            group_pubkey: jury_network.outer_verification_share,
            outer_group_pubkey: jury_network.outer_group_pubkey,
            outer_index: JURY_OUTER_INDEX,
        });

        let result = jury.sign(b"dispute payload hash", &player_a_share).await;
        assert!(result.is_some());
        assert!(result.unwrap().verified);
    }
}
