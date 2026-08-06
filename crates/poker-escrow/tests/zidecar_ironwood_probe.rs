//! Probe: does the escrow's OWN zecli client decode ironwood_actions from the live local zidecar?
//! grpcurl on zidecar.v1.Zidecar/GetCompactBlocks returns 2 ironwoodActions for the deposit block,
//! but the running scanner saw 0. This isolates the escrow's decode path with zero ambiguity.
//!
//!   ZIDECAR_URL=http://127.0.0.1:50051 PROBE_LO=420 PROBE_HI=424 \
//!     cargo test --manifest-path crates/poker-escrow/Cargo.toml --test zidecar_ironwood_probe \
//!     -- --ignored --nocapture

#[tokio::test]
#[ignore = "needs the local regtest zidecar running"]
async fn probe_ironwood_actions() {
    let url = std::env::var("ZIDECAR_URL").unwrap_or_else(|_| "http://127.0.0.1:50051".into());
    let lo: u32 = std::env::var("PROBE_LO").ok().and_then(|s| s.parse().ok()).unwrap_or(420);
    let hi: u32 = std::env::var("PROBE_HI").ok().and_then(|s| s.parse().ok()).unwrap_or(424);

    let client = zecli::client::ZidecarClient::connect(&url).await.expect("connect");
    let blocks = client.get_compact_blocks(lo, hi).await.expect("get_compact_blocks");
    println!("range {lo}..{hi}: {} blocks returned", blocks.len());
    let mut total_iw = 0usize;
    for b in &blocks {
        if !b.actions.is_empty() || !b.ironwood_actions.is_empty() {
            println!("  block {}: {} orchard, {} ironwood", b.height, b.actions.len(), b.ironwood_actions.len());
        }
        total_iw += b.ironwood_actions.len();
    }
    println!("TOTAL ironwood actions decoded by the escrow's zecli: {total_iw}");
}
