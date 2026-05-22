//! DKG over a FROST relay, using zafu's tagged wire protocol:
//!   Host R1:    "R1:T:N:SK:<fvk_sk_hex>:<part1_broadcast_hex>"
//!   Joiner R1:  "R1:<part1_broadcast_hex>"
//!   R2:         "R2:<peer_package_hex>"      (each peer sends N-1)
//!   FVK echo:   "FVK:<orchard_ufvk_string>"  (all must agree)

use std::time::{Duration, Instant};

use ff::PrimeField;
use frost_spend::orchestrate as fs;
use rand::RngCore;

use crate::frost_relay::{FrostRelayClient, RelayError, RelayEvent};

#[derive(Debug)]
pub struct DkgOutput {
    pub key_package_hex: String,
    pub public_key_package_hex: String,
    pub orchard_ua: String,
    pub orchard_ufvk: String,
    /// raw 96-byte FVK hex — feeds the zidecar compact-block scanner
    pub orchard_fvk_hex: String,
    /// host-generated sk for nk/rivk; needed to derive diversified addresses post-DKG
    pub sk_hex: String,
    /// FROST identity seed produced by dkg_part3 — required by `sign_round1` for fresh
    /// per-action nonces during payout signing.
    pub ephemeral_seed_hex: String,
}

#[derive(Debug, thiserror::Error)]
pub enum DkgError {
    #[error("relay: {0}")]
    Relay(#[from] RelayError),
    #[error("frost-spend: {0}")]
    Frost(String),
    #[error("ua: {0}")]
    Ua(String),
    #[error("timed out: {0}")]
    Timeout(String),
    #[error("room closed: {0}")]
    Closed(String),
    #[error("protocol: {0}")]
    Protocol(String),
}

/// host samples the fvk-seed sk + embeds it in R1 so every party derives the same UA/UFVK.
pub async fn run_dkg(
    client: &mut FrostRelayClient,
    threshold: u16,
    total: u16,
    initial_count: u32,
    is_host: bool,
    network: zcash_address::Network,
    timeout: Duration,
) -> Result<DkgOutput, DkgError> {
    let deadline = Instant::now() + timeout;
    wait_for_full_room(client, total as u32, initial_count, &deadline).await?;

    // round 1
    let r1 = fs::dkg_part1(total, threshold)
        .map_err(|e| DkgError::Frost(format!("part1: {:?}", e)))?;
    let host_sk_hex = if is_host { Some(sample_valid_sk_hex()) } else { None };
    let r1_text = match &host_sk_hex {
        Some(sk) => format!("R1:{}:{}:SK:{}:{}", threshold, total, sk, r1.broadcast_hex),
        None => format!("R1:{}", r1.broadcast_hex),
    };
    client.send_message(r1_text.as_bytes()).await?;

    let (r1_peers, learned_sk) =
        collect_r1(client, (total - 1) as usize, &deadline).await?;
    let sk_hex = host_sk_hex
        .or(learned_sk)
        .ok_or_else(|| DkgError::Protocol("no host SK seen in R1".into()))?;

    // round 2
    let r2 = fs::dkg_part2(&r1.secret_hex, &r1_peers)
        .map_err(|e| DkgError::Frost(format!("part2: {:?}", e)))?;
    for pkg in &r2.peer_packages {
        client.send_message(format!("R2:{}", pkg).as_bytes()).await?;
    }
    let expected_r2 = ((total - 1) as usize).pow(2);
    let r2_peers = collect_tagged(client, "R2:", expected_r2, &deadline).await?;

    // round 3
    let r3 = fs::dkg_part3(&r2.secret_hex, &r1_peers, &r2_peers)
        .map_err(|e| DkgError::Frost(format!("part3: {:?}", e)))?;

    let sk_bytes = decode_sk(&sk_hex)?;
    let addr_bytes = fs::derive_address_from_sk(&r3.public_key_package_hex, sk_bytes, 0)
        .map_err(|e| DkgError::Frost(format!("derive_address_from_sk: {:?}", e)))?;
    let ua = crate::orchard_ua::encode_unified(addr_bytes, network).map_err(DkgError::Ua)?;
    let fvk_bytes = crate::orchard_ua::fvk_bytes_from_sk(&r3.public_key_package_hex, sk_bytes)
        .map_err(DkgError::Ua)?;
    let ufvk = crate::orchard_ua::encode_ufvk_from_sk(&r3.public_key_package_hex, sk_bytes, network)
        .map_err(DkgError::Ua)?;

    // FVK echo — bail on disagreement before persisting
    client.send_message(format!("FVK:{}", ufvk).as_bytes()).await?;
    let peer_fvks = collect_tagged(client, "FVK:", (total - 1) as usize, &deadline).await?;
    for peer in &peer_fvks {
        if peer != &ufvk {
            let our_tail = &ufvk[ufvk.len().saturating_sub(8)..];
            let their_tail = &peer[peer.len().saturating_sub(8)..];
            return Err(DkgError::Protocol(format!(
                "FVK mismatch: ours ends …{}, peer ends …{}",
                our_tail, their_tail,
            )));
        }
    }

    Ok(DkgOutput {
        key_package_hex: r3.key_package_hex,
        public_key_package_hex: r3.public_key_package_hex,
        orchard_ua: ua,
        orchard_ufvk: ufvk,
        orchard_fvk_hex: hex::encode(fvk_bytes),
        sk_hex,
        ephemeral_seed_hex: r3.ephemeral_seed_hex,
    })
}

/// sample 32 bytes in Pallas scalar field (Orchard SpendingKey::from_bytes validity range)
fn sample_valid_sk_hex() -> String {
    let mut rng = rand::thread_rng();
    loop {
        let mut bytes = [0u8; 32];
        rng.fill_bytes(&mut bytes);
        let scalar = pasta_curves::pallas::Scalar::from_repr(bytes);
        if bool::from(scalar.is_some()) {
            return hex::encode(bytes);
        }
    }
}

fn decode_sk(hex_string: &str) -> Result<[u8; 32], DkgError> {
    let bytes = hex::decode(hex_string.trim())
        .map_err(|e| DkgError::Frost(format!("sk hex decode: {}", e)))?;
    if bytes.len() != 32 {
        return Err(DkgError::Frost(format!("sk wrong length: {}", bytes.len())));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

async fn wait_for_full_room(
    client: &mut FrostRelayClient,
    total: u32,
    initial_count: u32,
    deadline: &Instant,
) -> Result<(), DkgError> {
    if initial_count >= total {
        return Ok(());
    }
    loop {
        let remaining = remaining_or_timeout(deadline, "waiting for peers")?;
        match client.recv_event_timeout(remaining).await? {
            Some(RelayEvent::PeerJoined { count, .. }) if count >= total => return Ok(()),
            Some(RelayEvent::PeerJoined { .. }) => continue,
            Some(RelayEvent::Message { .. }) => continue, // pre-DKG noise
            Some(RelayEvent::Closed { reason }) => return Err(DkgError::Closed(reason)),
            None => return Err(DkgError::Timeout("waiting for peers".into())),
        }
    }
}

/// returns (peer broadcasts, host-supplied sk if any)
async fn collect_r1(
    client: &mut FrostRelayClient,
    n: usize,
    deadline: &Instant,
) -> Result<(Vec<String>, Option<String>), DkgError> {
    let mut broadcasts = Vec::with_capacity(n);
    let mut sk: Option<String> = None;
    while broadcasts.len() < n {
        let label = || format!("R1 collected {}/{}", broadcasts.len(), n);
        let remaining = remaining_or_timeout(deadline, &label())?;
        match client.recv_event_timeout(remaining).await? {
            Some(RelayEvent::Message { payload, .. }) => {
                let text = String::from_utf8(payload)
                    .map_err(|e| DkgError::Frost(format!("non-utf8 dkg payload: {}", e)))?;
                let (broadcast, host_sk) = parse_r1(&text)?;
                if let Some(s) = host_sk {
                    sk = Some(s);
                }
                broadcasts.push(broadcast);
            }
            Some(RelayEvent::PeerJoined { .. }) => continue,
            Some(RelayEvent::Closed { reason }) => return Err(DkgError::Closed(reason)),
            None => return Err(DkgError::Timeout(label())),
        }
    }
    Ok((broadcasts, sk))
}

fn parse_r1(text: &str) -> Result<(String, Option<String>), DkgError> {
    let body = text
        .strip_prefix("R1:")
        .ok_or_else(|| DkgError::Protocol(format!("expected R1: tag, got: {}", short(text))))?;
    if let Some(HostMeta { sk, broadcast }) = parse_host_prefix(body) {
        return Ok((broadcast, Some(sk)));
    }
    Ok((body.to_string(), None))
}

struct HostMeta {
    sk: String,
    broadcast: String,
}

/// peel the host-only "T:N:SK:<sk>:<broadcast>" header off; None on joiner R1
fn parse_host_prefix(body: &str) -> Option<HostMeta> {
    if !body.chars().next()?.is_ascii_digit() {
        return None;
    }
    let (_t, rest) = body.split_once(':')?;
    let (_n, rest) = rest.split_once(':')?;
    let rest = rest.strip_prefix("SK:")?;
    let (sk_hex, broadcast) = rest.split_once(':')?;
    if sk_hex.len() != 64 || !sk_hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(HostMeta {
        sk: sk_hex.to_string(),
        broadcast: broadcast.to_string(),
    })
}

/// collect n messages with the given tag, returning bodies with tag stripped
async fn collect_tagged(
    client: &mut FrostRelayClient,
    tag: &str,
    n: usize,
    deadline: &Instant,
) -> Result<Vec<String>, DkgError> {
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let label = || format!("{} collected {}/{}", tag, out.len(), n);
        let remaining = remaining_or_timeout(deadline, &label())?;
        match client.recv_event_timeout(remaining).await? {
            Some(RelayEvent::Message { payload, .. }) => {
                let text = String::from_utf8(payload)
                    .map_err(|e| DkgError::Frost(format!("non-utf8 dkg payload: {}", e)))?;
                let body = text.strip_prefix(tag).ok_or_else(|| {
                    DkgError::Protocol(format!("expected {} tag, got: {}", tag, short(&text)))
                })?;
                out.push(body.to_string());
            }
            Some(RelayEvent::PeerJoined { .. }) => continue,
            Some(RelayEvent::Closed { reason }) => return Err(DkgError::Closed(reason)),
            None => return Err(DkgError::Timeout(label())),
        }
    }
    Ok(out)
}

fn remaining_or_timeout(deadline: &Instant, ctx: &str) -> Result<Duration, DkgError> {
    let now = Instant::now();
    if now >= *deadline {
        Err(DkgError::Timeout(ctx.to_string()))
    } else {
        Ok(*deadline - now)
    }
}

fn short(s: &str) -> String {
    let take = s.len().min(32);
    s[..take].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frost_relay::FrostRelayClient;

    /// `cargo test --release -p poker-escrow -- --ignored dkg_3_of_3` (needs relay on :50053)
    #[tokio::test]
    #[ignore]
    async fn dkg_3_of_3() {
        let url = "ws://127.0.0.1:50053/ws";

        // coordinator creates the room
        let mut alice = FrostRelayClient::connect(url, "alice".into()).await.unwrap();
        let room = alice.create_room().await.unwrap();

        // two joiners join
        let mut bob = FrostRelayClient::connect(url, "bob".into()).await.unwrap();
        let bob_count = bob.join_room(&room).await.unwrap();
        let mut carol = FrostRelayClient::connect(url, "carol".into()).await.unwrap();
        let carol_count = carol.join_room(&room).await.unwrap();

        let alice_handle = tokio::spawn(async move {
            run_dkg(&mut alice, 3, 3, 1, true, zcash_address::Network::Test, Duration::from_secs(15)).await
        });
        let bob_handle = tokio::spawn(async move {
            run_dkg(&mut bob, 3, 3, bob_count, false, zcash_address::Network::Test, Duration::from_secs(15)).await
        });
        let carol_handle = tokio::spawn(async move {
            run_dkg(&mut carol, 3, 3, carol_count, false, zcash_address::Network::Test, Duration::from_secs(15)).await
        });

        let a = alice_handle.await.unwrap().expect("alice dkg");
        let b = bob_handle.await.unwrap().expect("bob dkg");
        let c = carol_handle.await.unwrap().expect("carol dkg");

        assert_eq!(a.public_key_package_hex, b.public_key_package_hex, "alice vs bob group key");
        assert_eq!(b.public_key_package_hex, c.public_key_package_hex, "bob vs carol group key");
        assert_eq!(a.orchard_ua, b.orchard_ua, "alice vs bob UA");
        assert_eq!(b.orchard_ua, c.orchard_ua, "bob vs carol UA");
        assert_eq!(a.orchard_ufvk, b.orchard_ufvk, "alice vs bob UFVK");
        assert!(a.orchard_ua.starts_with("utest1"), "expected testnet UA, got {}", a.orchard_ua);
        assert!(a.orchard_ufvk.starts_with("uviewtest1"), "expected testnet UFVK, got {}", a.orchard_ufvk);
        assert_ne!(a.key_package_hex, b.key_package_hex, "alice and bob should not share the same piece");
        assert_ne!(b.key_package_hex, c.key_package_hex);
    }

    #[test]
    fn parse_r1_host_and_joiner() {
        let sk_hex = "0".repeat(64);
        let host = format!("R1:2:3:SK:{}:abcdef", sk_hex);
        let (b, s) = parse_r1(&host).unwrap();
        assert_eq!(b, "abcdef");
        assert_eq!(s.as_deref(), Some(sk_hex.as_str()));

        let joiner = "R1:abcdef".to_string();
        let (b, s) = parse_r1(&joiner).unwrap();
        assert_eq!(b, "abcdef");
        assert_eq!(s, None);

        let bad = "R2:abcdef";
        assert!(parse_r1(bad).is_err());
    }
}
