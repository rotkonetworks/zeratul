//! frost_signer — local harness that stands in for a zafu-wallet FROST participant.
//!
//! It reuses the ESCROW'S OWN modules verbatim (via #[path] includes of the exact same
//! source files main.rs compiles) so the DKG joiner + payout co-signer logic exercised here
//! is byte-for-byte the code the production escrow runs. No forked copies.
//!
//! Two subcommands:
//!   dkg   — join an existing FROST relay room as a NON-host DKG participant, run the DKG dance,
//!           and persist the resulting key share (key_package_hex, ephemeral_seed_hex,
//!           public_key_package_hex, derived UA/UFVK) to a JSON file.
//!   sign  — reload a persisted share and act as the payout co-signer (join_sign_pczt).
//!
//! Usage:
//!   frost_signer dkg  --relay ws://127.0.0.1:50053/ws --room <code> --network main --out share.json
//!   frost_signer sign --relay ws://127.0.0.1:50053/ws --room <code> --share share.json

// ── reuse the escrow's real modules ─────────────────────────────────────────
#[path = "../frost_relay.rs"]
mod frost_relay;
#[path = "../orchard_ua.rs"]
mod orchard_ua;
#[path = "../frost_dkg.rs"]
mod frost_dkg;
#[path = "../payout_signing.rs"]
mod payout_signing;

use std::time::Duration;

use serde::{Deserialize, Serialize};

use frost_relay::FrostRelayClient;

#[derive(Serialize, Deserialize, Debug)]
struct PersistedShare {
    /// this signer's private FROST key package (its 1-of-N piece)
    key_package_hex: String,
    /// group public key package — identical across all parties
    public_key_package_hex: String,
    /// FROST identity seed from dkg_part3 — required to mint fresh signing nonces
    ephemeral_seed_hex: String,
    /// derived group Orchard mainnet UA (u1…)
    orchard_ua: String,
    /// derived group UFVK
    orchard_ufvk: String,
    /// raw 96-byte FVK hex
    orchard_fvk_hex: String,
    /// host-broadcast sk seed
    sk_hex: String,
    /// which relay room this share was minted in
    room: String,
}

#[derive(clap::Parser, Debug)]
#[command(about = "local FROST participant harness (DKG joiner + payout co-signer)")]
struct Args {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(clap::Subcommand, Debug)]
enum Cmd {
    /// Join an existing relay room and run the DKG as a non-host participant.
    Dkg {
        #[arg(long, default_value = "ws://127.0.0.1:50053/ws")]
        relay: String,
        #[arg(long)]
        room: String,
        #[arg(long)]
        nick: String,
        #[arg(long, default_value = "main")]
        network: String,
        #[arg(long, default_value_t = 2)]
        threshold: u16,
        #[arg(long, default_value_t = 3)]
        total: u16,
        #[arg(long)]
        out: String,
        #[arg(long, default_value_t = 120)]
        timeout_secs: u64,
    },
    /// Reload a persisted share and co-sign a payout PCZT (joiner path).
    Sign {
        #[arg(long, default_value = "ws://127.0.0.1:50053/ws")]
        relay: String,
        #[arg(long)]
        room: String,
        #[arg(long)]
        nick: String,
        #[arg(long)]
        share: String,
        #[arg(long, default_value_t = 120)]
        timeout_secs: u64,
    },
    /// Reload a persisted share and HOST a payout signing dance (host_sign_pczt path) over a
    /// caller-supplied dummy sighash + alpha. Stands in for the escrow's run_payout_signing when
    /// no real PCZT/notes exist yet. Creates the relay room and prints its code so a joiner can
    /// attach; produces the aggregated (and thus verified) 64-byte SpendAuth signature(s).
    HostSign {
        #[arg(long, default_value = "ws://127.0.0.1:50053/ws")]
        relay: String,
        #[arg(long)]
        nick: String,
        #[arg(long)]
        share: String,
        /// 32-byte sighash hex (dummy for this proof)
        #[arg(long)]
        sighash: String,
        /// comma-separated 32-byte alpha hex values (one per action)
        #[arg(long)]
        alphas: String,
        /// file to write the created relay room code into (so the joiner can read it)
        #[arg(long)]
        room_out: String,
        #[arg(long, default_value_t = 120)]
        timeout_secs: u64,
    },
}

#[tokio::main]
async fn main() {
    let args = <Args as clap::Parser>::parse();
    match args.cmd {
        Cmd::Dkg { relay, room, nick, network, threshold, total, out, timeout_secs } => {
            run_dkg_joiner(relay, room, nick, network, threshold, total, out, timeout_secs).await;
        }
        Cmd::Sign { relay, room, nick, share, timeout_secs } => {
            run_sign_joiner(relay, room, nick, share, timeout_secs).await;
        }
        Cmd::HostSign { relay, nick, share, sighash, alphas, room_out, timeout_secs } => {
            run_sign_host(relay, nick, share, sighash, alphas, room_out, timeout_secs).await;
        }
    }
}

fn decode32(h: &str) -> [u8; 32] {
    let v = hex::decode(h.trim()).expect("hex decode");
    assert_eq!(v.len(), 32, "expected 32 bytes, got {}", v.len());
    let mut a = [0u8; 32];
    a.copy_from_slice(&v);
    a
}

async fn run_sign_host(
    relay: String,
    nick: String,
    share_path: String,
    sighash_hex: String,
    alphas_csv: String,
    room_out: String,
    timeout_secs: u64,
) {
    let raw = std::fs::read_to_string(&share_path).expect("read share file");
    let share: PersistedShare = serde_json::from_str(&raw).expect("parse share file");
    let sighash = decode32(&sighash_hex);
    let alphas: Vec<[u8; 32]> = alphas_csv.split(',').map(decode32).collect();

    let mut client = match FrostRelayClient::connect(&relay, nick.clone()).await {
        Ok(c) => c,
        Err(e) => { eprintln!("[{}] relay connect: {:?}", nick, e); std::process::exit(1); }
    };
    let room = match client.create_room().await {
        Ok(r) => r,
        Err(e) => { eprintln!("[{}] create_room: {:?}", nick, e); std::process::exit(1); }
    };
    std::fs::write(&room_out, &room).expect("write room_out");
    eprintln!("[{}] hosting signing room {} ({} action(s))", nick, room, alphas.len());

    let secrets = payout_signing::PayoutSignSecrets {
        key_package_hex: share.key_package_hex,
        ephemeral_seed_hex: share.ephemeral_seed_hex,
    };
    // pczt_hex empty — joiner's independent OVK check is skipped in this crypto-only proof.
    let sigs = match payout_signing::host_sign_pczt(
        &mut client,
        &share.public_key_package_hex,
        &secrets,
        sighash,
        &alphas,
        "u1dummy-recipient",
        0,
        10_000,
        "",
        Duration::from_secs(timeout_secs),
    ).await {
        Ok(s) => s,
        Err(e) => { eprintln!("[{}] host_sign_pczt failed: {}", nick, e); std::process::exit(1); }
    };
    eprintln!("[{}] host signing complete — {} action sig(s)", nick, sigs.len());
    for (i, s) in sigs.iter().enumerate() {
        println!("SIG {} {} {}", nick, i, hex::encode(s));
    }
}

async fn run_dkg_joiner(
    relay: String,
    room: String,
    nick: String,
    network: String,
    threshold: u16,
    total: u16,
    out: String,
    timeout_secs: u64,
) {
    let net = orchard_ua::network_from_str(&network);
    let mut client = match FrostRelayClient::connect(&relay, nick.clone()).await {
        Ok(c) => c,
        Err(e) => { eprintln!("[{}] relay connect: {:?}", nick, e); std::process::exit(1); }
    };
    let count = match client.join_room(&room).await {
        Ok(c) => c,
        Err(e) => { eprintln!("[{}] join_room: {:?}", nick, e); std::process::exit(1); }
    };
    eprintln!("[{}] joined room {} — participant count now {}", nick, room, count);

    let out_dkg = match frost_dkg::run_dkg(
        &mut client,
        threshold,
        total,
        count,           // our initial view of room size
        false,           // NOT host
        net,
        Duration::from_secs(timeout_secs),
    ).await {
        Ok(o) => o,
        Err(e) => { eprintln!("[{}] DKG failed: {}", nick, e); std::process::exit(1); }
    };

    let persisted = PersistedShare {
        key_package_hex: out_dkg.key_package_hex,
        public_key_package_hex: out_dkg.public_key_package_hex,
        ephemeral_seed_hex: out_dkg.ephemeral_seed_hex,
        orchard_ua: out_dkg.orchard_ua,
        orchard_ufvk: out_dkg.orchard_ufvk,
        orchard_fvk_hex: out_dkg.orchard_fvk_hex,
        sk_hex: out_dkg.sk_hex,
        room: room.clone(),
    };
    let json = serde_json::to_string_pretty(&persisted).expect("serialize share");
    std::fs::write(&out, &json).expect("write share file");
    eprintln!("[{}] DKG complete — UA={} — share persisted to {}", nick, persisted.orchard_ua, out);
    // machine-readable line for the driver
    println!("DKG_OK {} {}", nick, persisted.orchard_ua);
}

async fn run_sign_joiner(
    relay: String,
    room: String,
    nick: String,
    share_path: String,
    timeout_secs: u64,
) {
    let raw = std::fs::read_to_string(&share_path).expect("read share file");
    let share: PersistedShare = serde_json::from_str(&raw).expect("parse share file");

    let mut client = match FrostRelayClient::connect(&relay, nick.clone()).await {
        Ok(c) => c,
        Err(e) => { eprintln!("[{}] relay connect: {:?}", nick, e); std::process::exit(1); }
    };
    let count = match client.join_room(&room).await {
        Ok(c) => c,
        Err(e) => { eprintln!("[{}] join_room: {:?}", nick, e); std::process::exit(1); }
    };
    eprintln!("[{}] joined signing room {} — count {}", nick, room, count);

    let secrets = payout_signing::PayoutSignSecrets {
        key_package_hex: share.key_package_hex,
        ephemeral_seed_hex: share.ephemeral_seed_hex,
    };
    let sigs = match payout_signing::join_sign_pczt(
        &mut client,
        &share.public_key_package_hex,
        &secrets,
        Duration::from_secs(timeout_secs),
    ).await {
        Ok(s) => s,
        Err(e) => { eprintln!("[{}] join_sign_pczt failed: {}", nick, e); std::process::exit(1); }
    };
    eprintln!("[{}] payout signing complete — {} action sig(s)", nick, sigs.len());
    for (i, s) in sigs.iter().enumerate() {
        println!("SIG {} {} {}", nick, i, hex::encode(s));
    }
}
