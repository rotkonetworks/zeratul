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

// ────────────────────────────────────────────────────────────────────────────
// PCZT-driven multi-action signing (unified wire with zafu mnemonic-sign / multisig/sign)
// ────────────────────────────────────────────────────────────────────────────
// Wire (matches zafu/apps/extension/src/routes/popup/send/frost-multisig/relay-protocol.ts):
//   SIGN:<sighash hex>:<alpha1,alpha2,...>:<recipient>:<amount_zat>:<fee_zat>   (host, once)
//   C:<commit1>|<commit2>|...                                                   (both, once — N commits each, pipe-separated)
//   S:<action_idx>:<share>                                                      (both, N times, one per action)

#[derive(Debug, Clone)]
pub struct PayoutSignSecrets {
    pub key_package_hex: String,
    pub ephemeral_seed_hex: String,
}

/// 64-byte SpendAuth signatures, parallel to the bundle's actions; pass directly to
/// `zecli::pczt::complete_pczt_tx`.
pub type ActionSigs = Vec<[u8; 64]>;

/// Host the multi-action signing for a PCZT we built. `sighash` + `alphas` come from
/// `zecli::pczt::PcztState` after `tx_build::build_payout_pczt`. `display_*` fields are
/// what zafu's approval popup shows to the user (recipient = first non-zero output's UA,
/// amount = first non-zero output's zat). The on-chain bundle can have additional outputs;
/// the OVK-decrypt verifier in zafu would catch divergence (skipped here until we publish
/// the unsigned tx hex with the SIGN message).
#[allow(clippy::too_many_arguments)]
pub async fn host_sign_pczt(
    client: &mut FrostRelayClient,
    public_key_package_hex: &str,
    secrets: &PayoutSignSecrets,
    sighash: [u8; 32],
    alphas: &[[u8; 32]],
    display_recipient: &str,
    display_amount_zat: u64,
    fee_zat: u64,
    timeout: Duration,
) -> Result<ActionSigs, SignError> {
    let deadline = Instant::now() + timeout;
    wait_for_peer(client, &deadline).await?;

    let sign_msg = format!(
        "SIGN:{}:{}:{}:{}:{}",
        hex::encode(sighash),
        alphas.iter().map(hex::encode).collect::<Vec<_>>().join(","),
        display_recipient,
        display_amount_zat,
        fee_zat,
    );
    client.send_message(sign_msg.as_bytes()).await?;
    run_multi_rounds(client, public_key_package_hex, secrets, sighash, alphas, &deadline).await
}

/// Join an existing PCZT signing session as a peer. Parses SIGN off the wire to learn
/// the sighash + alphas, then runs the rounds. The peer must supply the
/// `public_key_package_hex` locally (stored in the multisig vault from DKG).
pub async fn join_sign_pczt(
    client: &mut FrostRelayClient,
    public_key_package_hex: &str,
    secrets: &PayoutSignSecrets,
    timeout: Duration,
) -> Result<ActionSigs, SignError> {
    let deadline = Instant::now() + timeout;
    let (sighash, alphas) = wait_for_sign(client, &deadline).await?;
    run_multi_rounds(client, public_key_package_hex, secrets, sighash, &alphas, &deadline).await
}

async fn run_multi_rounds(
    client: &mut FrostRelayClient,
    public_key_package_hex: &str,
    secrets: &PayoutSignSecrets,
    sighash: [u8; 32],
    alphas: &[[u8; 32]],
    deadline: &Instant,
) -> Result<ActionSigs, SignError> {
    let seed_bytes = decode_seed(&secrets.ephemeral_seed_hex)?;
    let n = alphas.len();

    // round 1: one fresh nonce/commitment per action
    let mut my_nonces: Vec<String> = Vec::with_capacity(n);
    let mut my_commits: Vec<String> = Vec::with_capacity(n);
    for _ in 0..n {
        let (nonces, signed_commit) = fs::sign_round1(&seed_bytes, &secrets.key_package_hex)
            .map_err(|e| SignError::Frost(format!("round1: {:?}", e)))?;
        my_nonces.push(nonces);
        my_commits.push(signed_commit);
    }
    client.send_message(format!("C:{}", my_commits.join("|")).as_bytes()).await?;
    let peer_commits_csv = collect_tagged(client, "C:", 1, deadline).await?
        .into_iter().next().unwrap();
    let peer_commits: Vec<String> = peer_commits_csv.split('|').map(|s| s.to_string()).collect();
    if peer_commits.len() != n {
        return Err(SignError::Protocol(format!(
            "peer sent {} commits, expected {}", peer_commits.len(), n,
        )));
    }

    // round 2: per action — sign share, exchange, aggregate
    let mut out: ActionSigs = Vec::with_capacity(n);
    for i in 0..n {
        let all_commits = vec![my_commits[i].clone(), peer_commits[i].clone()];
        let signed_share = fs::spend_sign_round2_signed(
            &seed_bytes, &secrets.key_package_hex, &my_nonces[i],
            &sighash, &alphas[i], &all_commits,
        ).map_err(|e| SignError::Frost(format!("round2 action {}: {:?}", i, e)))?;
        client.send_message(format!("S:{}:{}", i, signed_share).as_bytes()).await?;
        let peer_share = wait_share_for(client, i, deadline).await?;
        let all_shares = vec![signed_share, peer_share];
        let sig_hex = fs::spend_aggregate(
            public_key_package_hex, &sighash, &alphas[i], &all_commits, &all_shares,
        ).map_err(|e| SignError::Frost(format!("aggregate action {}: {:?}", i, e)))?;
        out.push(decode_sig_64(&sig_hex)?);
    }
    Ok(out)
}

async fn wait_for_sign(
    client: &mut FrostRelayClient,
    deadline: &Instant,
) -> Result<([u8; 32], Vec<[u8; 32]>), SignError> {
    loop {
        let remaining = remaining_or_timeout(deadline, "waiting for SIGN")?;
        match client.recv_event_timeout(remaining).await? {
            Some(RelayEvent::Message { payload, .. }) => {
                let text = String::from_utf8(payload)
                    .map_err(|e| SignError::Decode(format!("non-utf8 sign: {}", e)))?;
                let body = text.strip_prefix("SIGN:")
                    .ok_or_else(|| SignError::Protocol(format!("expected SIGN, got: {}", short(&text))))?;
                // SIGN:<sighash>:<alphas>:<recipient>:<amount>:<fee>[:<unsignedTxHex>] — only need first two for crypto
                let mut parts = body.splitn(5, ':');
                let sighash_hex = parts.next().ok_or_else(|| SignError::Protocol("missing sighash".into()))?;
                let alphas_csv = parts.next().ok_or_else(|| SignError::Protocol("missing alphas".into()))?;
                let sighash = decode_32(sighash_hex, "sighash")?;
                let alphas: Vec<[u8; 32]> = alphas_csv.split(',')
                    .map(|h| decode_32(h, "alpha"))
                    .collect::<Result<_, _>>()?;
                return Ok((sighash, alphas));
            }
            Some(RelayEvent::PeerJoined { .. }) => continue,
            Some(RelayEvent::Closed { reason }) => return Err(SignError::Closed(reason)),
            None => return Err(SignError::Timeout("waiting for SIGN".into())),
        }
    }
}

async fn wait_share_for(
    client: &mut FrostRelayClient,
    idx: usize,
    deadline: &Instant,
) -> Result<String, SignError> {
    let prefix = format!("S:{}:", idx);
    loop {
        let remaining = remaining_or_timeout(deadline, &format!("share {}", idx))?;
        match client.recv_event_timeout(remaining).await? {
            Some(RelayEvent::Message { payload, .. }) => {
                let text = String::from_utf8(payload)
                    .map_err(|e| SignError::Decode(format!("non-utf8: {}", e)))?;
                if let Some(body) = text.strip_prefix(&prefix) {
                    return Ok(body.to_string());
                }
                // stale messages (other actions' shares) — skip
            }
            Some(RelayEvent::PeerJoined { .. }) => continue,
            Some(RelayEvent::Closed { reason }) => return Err(SignError::Closed(reason)),
            None => return Err(SignError::Timeout(format!("share {}", idx))),
        }
    }
}

fn decode_sig_64(h: &str) -> Result<[u8; 64], SignError> {
    let bytes = hex::decode(h.trim()).map_err(|e| SignError::Decode(format!("sig hex: {}", e)))?;
    if bytes.len() != 64 {
        return Err(SignError::Decode(format!("sig wrong length: {}", bytes.len())));
    }
    let mut out = [0u8; 64];
    out.copy_from_slice(&bytes);
    Ok(out)
}

// ────────────────────────────────────────────────────────────────────────────
// Single-action signing (5.1c-α — kept for the test harness)
// ────────────────────────────────────────────────────────────────────────────

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

    /// Multi-action variant: 3 actions, same sighash, 3 distinct alphas. Verifies both sides
    /// produce the same Vec<sig> in order — the payout flow's signing leg.
    /// Run with: `cargo test --release -p poker-escrow -- --ignored payout_sign_multi`
    #[tokio::test]
    #[ignore]
    async fn payout_sign_multi() {
        let url = "ws://127.0.0.1:50053/ws";
        let (pkg_hex, shares) = local_2of3_dkg();
        let (kp_a, seed_a) = shares[0].clone();
        let (kp_b, seed_b) = shares[1].clone();

        let sighash = decode_32(&sample_valid_scalar_hex(), "sighash").unwrap();
        let alphas: Vec<[u8; 32]> = (0..3)
            .map(|_| decode_32(&sample_valid_scalar_hex(), "alpha").unwrap())
            .collect();

        let mut alice = FrostRelayClient::connect(url, "alice".into()).await.expect("alice connect");
        let room = alice.create_room().await.expect("create");
        let mut bob = FrostRelayClient::connect(url, "bob".into()).await.expect("bob connect");
        let _ = bob.join_room(&room).await.expect("bob join");

        let secrets_a = PayoutSignSecrets { key_package_hex: kp_a, ephemeral_seed_hex: seed_a };
        let secrets_b = PayoutSignSecrets { key_package_hex: kp_b, ephemeral_seed_hex: seed_b };
        let pkg_for_host = pkg_hex.clone();
        let pkg_for_join = pkg_hex.clone();
        let alphas_for_host = alphas.clone();
        let host = tokio::spawn(async move {
            host_sign_pczt(&mut alice, &pkg_for_host, &secrets_a, sighash, &alphas_for_host,
                "u1test", 1234, 10_000, Duration::from_secs(30)).await
        });
        let join = tokio::spawn(async move {
            join_sign_pczt(&mut bob, &pkg_for_join, &secrets_b, Duration::from_secs(30)).await
        });

        let a = host.await.unwrap().expect("host_sign_pczt");
        let b = join.await.unwrap().expect("join_sign_pczt");
        assert_eq!(a.len(), 3, "host produces 3 sigs");
        assert_eq!(b.len(), 3, "joiner produces 3 sigs");
        for i in 0..3 {
            assert_eq!(a[i], b[i], "action {} sig matches across parties", i);
        }
    }
}
