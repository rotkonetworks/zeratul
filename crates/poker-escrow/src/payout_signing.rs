//! 2-of-3 FROST sign dance for a single Orchard SpendAuth signature.
//!
//! Phase 5.1c-α scope: drives the relay round-trip with caller-supplied sighash + alpha.
//! Real Orchard tx construction (notes, anchor, builder.prepare to extract alphas) lands in
//! 5.1c-β; this module is the protocol layer underneath.
//!
//! Wire protocol (host = whichever party opened the relay room; usually poker-escrow):
//!   INIT:<public_key_package_hex>:<sighash_hex>:<alpha_hex>     (host, once)
//!   R1:<signed_commitments_hex>                                 (both, once each)
//!   R2:<signed_share_hex>                                       (both, once each)

use std::time::{Duration, Instant};

use frost_spend::orchestrate as fs;

use crate::frost_relay::{FrostRelayClient, RelayError, RelayEvent};

#[derive(Debug, thiserror::Error)]
pub enum SignError {
    #[error("relay: {0}")]
    Relay(#[from] RelayError),
    #[error("frost-spend: {0}")]
    Frost(String),
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("timed out: {0}")]
    Timeout(String),
    #[error("room closed: {0}")]
    Closed(String),
    #[error("decode: {0}")]
    Decode(String),
}

#[derive(Debug, Clone)]
pub struct SignInputs {
    pub public_key_package_hex: String,
    pub key_package_hex: String,
    pub ephemeral_seed_hex: String,
    pub sighash: [u8; 32],
    pub alpha: [u8; 32],
}

#[derive(Debug, Clone)]
pub struct SignOutput {
    /// 64-byte SpendAuth signature, hex-encoded — ready to inject via orchard `append_signatures`
    pub spend_auth_sig_hex: String,
}

/// Run a 2-party FROST signing as the **host** — broadcasts INIT, exchanges R1 + R2, aggregates.
/// Both parties contribute one round-1 commitment and one round-2 share; the host aggregates and
/// returns the 64-byte SpendAuth signature. The peer can verify the aggregated signature against
/// `(sighash, alpha, public_key_package)` independently.
pub async fn host_sign(
    client: &mut FrostRelayClient,
    inputs: &SignInputs,
    timeout: Duration,
) -> Result<SignOutput, SignError> {
    let deadline = Instant::now() + timeout;
    wait_for_peer(client, &deadline).await?;

    let init = format!(
        "INIT:{}:{}:{}",
        inputs.public_key_package_hex,
        hex::encode(inputs.sighash),
        hex::encode(inputs.alpha),
    );
    client.send_message(init.as_bytes()).await?;
    run_local_rounds(client, inputs, &deadline, /* expect_init */ false).await
}

/// Run as the **joiner** — wait for INIT, parse it, then exchange R1 + R2. Returns the same
/// aggregated signature (both parties aggregate independently against identical inputs).
pub async fn join_sign(
    client: &mut FrostRelayClient,
    key_package_hex: String,
    ephemeral_seed_hex: String,
    timeout: Duration,
) -> Result<SignOutput, SignError> {
    let deadline = Instant::now() + timeout;
    let (pkg, sighash, alpha) = wait_for_init(client, &deadline).await?;
    let inputs = SignInputs {
        public_key_package_hex: pkg,
        key_package_hex,
        ephemeral_seed_hex,
        sighash,
        alpha,
    };
    run_local_rounds(client, &inputs, &deadline, /* expect_init */ false).await
}

async fn run_local_rounds(
    client: &mut FrostRelayClient,
    inputs: &SignInputs,
    deadline: &Instant,
    _expect_init: bool,
) -> Result<SignOutput, SignError> {
    let seed_bytes = decode_seed(&inputs.ephemeral_seed_hex)?;

    let (nonces_hex, signed_commit) = fs::sign_round1(&seed_bytes, &inputs.key_package_hex)
        .map_err(|e| SignError::Frost(format!("round1: {:?}", e)))?;
    client.send_message(format!("R1:{}", signed_commit).as_bytes()).await?;
    let peer_commits = collect_tagged(client, "R1:", 1, deadline).await?;
    let all_commits = vec![signed_commit, peer_commits.into_iter().next().unwrap()];

    let signed_share = fs::spend_sign_round2_signed(
        &seed_bytes, &inputs.key_package_hex, &nonces_hex,
        &inputs.sighash, &inputs.alpha, &all_commits,
    ).map_err(|e| SignError::Frost(format!("round2: {:?}", e)))?;
    client.send_message(format!("R2:{}", signed_share).as_bytes()).await?;
    let peer_shares = collect_tagged(client, "R2:", 1, deadline).await?;
    let all_shares = vec![signed_share, peer_shares.into_iter().next().unwrap()];

    let sig_hex = fs::spend_aggregate(
        &inputs.public_key_package_hex, &inputs.sighash, &inputs.alpha,
        &all_commits, &all_shares,
    ).map_err(|e| SignError::Frost(format!("aggregate: {:?}", e)))?;

    Ok(SignOutput { spend_auth_sig_hex: sig_hex })
}

async fn wait_for_peer(client: &mut FrostRelayClient, deadline: &Instant) -> Result<(), SignError> {
    loop {
        let remaining = remaining_or_timeout(deadline, "waiting for peer")?;
        match client.recv_event_timeout(remaining).await? {
            Some(RelayEvent::PeerJoined { count, .. }) if count >= 2 => return Ok(()),
            Some(RelayEvent::PeerJoined { .. }) => continue,
            Some(RelayEvent::Message { .. }) => continue,
            Some(RelayEvent::Closed { reason }) => return Err(SignError::Closed(reason)),
            None => return Err(SignError::Timeout("waiting for peer".into())),
        }
    }
}

async fn wait_for_init(
    client: &mut FrostRelayClient,
    deadline: &Instant,
) -> Result<(String, [u8; 32], [u8; 32]), SignError> {
    loop {
        let remaining = remaining_or_timeout(deadline, "waiting for INIT")?;
        match client.recv_event_timeout(remaining).await? {
            Some(RelayEvent::Message { payload, .. }) => {
                let text = String::from_utf8(payload)
                    .map_err(|e| SignError::Decode(format!("non-utf8 init: {}", e)))?;
                let body = text.strip_prefix("INIT:")
                    .ok_or_else(|| SignError::Protocol(format!("expected INIT, got: {}", short(&text))))?;
                let mut parts = body.splitn(3, ':');
                let pkg = parts.next().ok_or_else(|| SignError::Protocol("INIT missing pkg".into()))?;
                let sighash_hex = parts.next().ok_or_else(|| SignError::Protocol("INIT missing sighash".into()))?;
                let alpha_hex = parts.next().ok_or_else(|| SignError::Protocol("INIT missing alpha".into()))?;
                return Ok((pkg.to_string(), decode_32(sighash_hex, "sighash")?, decode_32(alpha_hex, "alpha")?));
            }
            Some(RelayEvent::PeerJoined { .. }) => continue,
            Some(RelayEvent::Closed { reason }) => return Err(SignError::Closed(reason)),
            None => return Err(SignError::Timeout("waiting for INIT".into())),
        }
    }
}

async fn collect_tagged(
    client: &mut FrostRelayClient,
    tag: &str,
    n: usize,
    deadline: &Instant,
) -> Result<Vec<String>, SignError> {
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let label = || format!("{} collected {}/{}", tag, out.len(), n);
        let remaining = remaining_or_timeout(deadline, &label())?;
        match client.recv_event_timeout(remaining).await? {
            Some(RelayEvent::Message { payload, .. }) => {
                let text = String::from_utf8(payload)
                    .map_err(|e| SignError::Decode(format!("non-utf8: {}", e)))?;
                if let Some(body) = text.strip_prefix(tag) {
                    out.push(body.to_string());
                } else if text.starts_with("INIT:") {
                    continue; // stale, host already broadcast it
                } else {
                    return Err(SignError::Protocol(format!("expected {} tag, got: {}", tag, short(&text))));
                }
            }
            Some(RelayEvent::PeerJoined { .. }) => continue,
            Some(RelayEvent::Closed { reason }) => return Err(SignError::Closed(reason)),
            None => return Err(SignError::Timeout(label())),
        }
    }
    Ok(out)
}

fn decode_32(h: &str, ctx: &str) -> Result<[u8; 32], SignError> {
    let v = hex::decode(h.trim()).map_err(|e| SignError::Decode(format!("{} hex: {}", ctx, e)))?;
    if v.len() != 32 {
        return Err(SignError::Decode(format!("{} wrong length: {}", ctx, v.len())));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&v);
    Ok(out)
}

fn decode_seed(h: &str) -> Result<[u8; 32], SignError> {
    decode_32(h, "ephemeral_seed")
}

fn remaining_or_timeout(deadline: &Instant, ctx: &str) -> Result<Duration, SignError> {
    let now = Instant::now();
    if now >= *deadline {
        Err(SignError::Timeout(ctx.to_string()))
    } else {
        Ok(*deadline - now)
    }
}

fn short(s: &str) -> String {
    let take = s.len().min(48);
    s[..take].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ff::PrimeField;
    use frost_spend::orchestrate::{dkg_part1, dkg_part2, dkg_part3};

    fn sample_valid_scalar_hex() -> String {
        use rand::RngCore;
        let mut rng = rand::thread_rng();
        loop {
            let mut bytes = [0u8; 32];
            rng.fill_bytes(&mut bytes);
            if bool::from(pasta_curves::pallas::Scalar::from_repr(bytes).is_some()) {
                return hex::encode(bytes);
            }
        }
    }

    /// run a 3-party DKG in-process (no relay) to mint a 2-of-3 multisig with all key material
    /// already unwrapped. returns (pkg, [share, seed]_for_each_of_3).
    fn local_2of3_dkg() -> (String, [(String, String); 3]) {
        let r1_a = dkg_part1(3, 2).unwrap();
        let r1_b = dkg_part1(3, 2).unwrap();
        let r1_c = dkg_part1(3, 2).unwrap();
        let bc_for_a = vec![r1_b.broadcast_hex.clone(), r1_c.broadcast_hex.clone()];
        let bc_for_b = vec![r1_a.broadcast_hex.clone(), r1_c.broadcast_hex.clone()];
        let bc_for_c = vec![r1_a.broadcast_hex.clone(), r1_b.broadcast_hex.clone()];
        let r2_a = dkg_part2(&r1_a.secret_hex, &bc_for_a).unwrap();
        let r2_b = dkg_part2(&r1_b.secret_hex, &bc_for_b).unwrap();
        let r2_c = dkg_part2(&r1_c.secret_hex, &bc_for_c).unwrap();
        let all_r2: Vec<String> = r2_a.peer_packages.iter()
            .chain(r2_b.peer_packages.iter())
            .chain(r2_c.peer_packages.iter())
            .cloned().collect();
        let r3_a = dkg_part3(&r2_a.secret_hex, &bc_for_a, &all_r2).unwrap();
        let r3_b = dkg_part3(&r2_b.secret_hex, &bc_for_b, &all_r2).unwrap();
        let r3_c = dkg_part3(&r2_c.secret_hex, &bc_for_c, &all_r2).unwrap();
        (
            r3_a.public_key_package_hex.clone(),
            [
                (r3_a.key_package_hex, r3_a.ephemeral_seed_hex),
                (r3_b.key_package_hex, r3_b.ephemeral_seed_hex),
                (r3_c.key_package_hex, r3_c.ephemeral_seed_hex),
            ],
        )
    }

    /// 2-of-3 FROST SpendAuth signing between two in-process clients on the live relay :50053.
    /// Synthetic sighash + alpha (valid Pallas scalars). Verifies host + joiner converge on the
    /// same 64-byte signature.
    /// Run with: `cargo test --release -p poker-escrow -- --ignored payout_sign_2of3`
    #[tokio::test]
    #[ignore]
    async fn payout_sign_2of3() {
        let url = "ws://127.0.0.1:50053/ws";

        // 2-of-3 multisig via in-process DKG; we use shares 0 + 1 to sign.
        let (pkg_hex, shares) = local_2of3_dkg();
        let (kp_a, seed_a) = shares[0].clone();
        let (kp_b, seed_b) = shares[1].clone();

        let sighash = decode_32(&sample_valid_scalar_hex(), "sighash").unwrap();
        let alpha = decode_32(&sample_valid_scalar_hex(), "alpha").unwrap();

        let mut alice = FrostRelayClient::connect(url, "alice".into()).await.expect("alice connect");
        let room = alice.create_room().await.expect("create");
        let mut bob = FrostRelayClient::connect(url, "bob".into()).await.expect("bob connect");
        let _ = bob.join_room(&room).await.expect("bob join");

        let inputs_a = SignInputs {
            public_key_package_hex: pkg_hex.clone(),
            key_package_hex: kp_a,
            ephemeral_seed_hex: seed_a,
            sighash, alpha,
        };

        let host = tokio::spawn(async move {
            host_sign(&mut alice, &inputs_a, Duration::from_secs(20)).await
        });
        let join = tokio::spawn(async move {
            join_sign(&mut bob, kp_b, seed_b, Duration::from_secs(20)).await
        });

        let a = host.await.unwrap().expect("host_sign");
        let b = join.await.unwrap().expect("join_sign");

        assert_eq!(a.spend_auth_sig_hex, b.spend_auth_sig_hex, "both parties agree on the aggregated sig");
        assert_eq!(a.spend_auth_sig_hex.len(), 128, "SpendAuth sig is 64 bytes (128 hex chars)");
    }
}
