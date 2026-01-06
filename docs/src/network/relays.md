# relay nodes

relay nodes forward traffic when direct connections fail. they enable connectivity for players behind restrictive NATs.

## when relays are needed

```
direct connection works when:
  - both players have public IPs
  - at least one player's NAT allows hole punching
  - firewall allows UDP

relay needed when:
  - symmetric NAT on both sides
  - corporate firewall blocks UDP
  - carrier-grade NAT (CGNAT)
  - strict firewall policies
```

## how relays work

```
relay forwarding:

  player A                    relay                    player B
      │                         │                         │
      │   establish session     │                         │
      │ ───────────────────────▶│                         │
      │                         │                         │
      │                         │◀─── establish session ──│
      │                         │                         │
      │   encrypted msg         │                         │
      │ ───────────────────────▶│   encrypted msg         │
      │                         │ ───────────────────────▶│
      │                         │                         │
      │                         │   encrypted response    │
      │   encrypted response    │◀─────────────────────── │
      │◀─────────────────────── │                         │
      │                         │                         │

relay sees:
  - encrypted blobs
  - message sizes
  - timing

relay doesn't see:
  - message contents
  - game state
  - player actions
```

## iroh relays

iroh provides relay infrastructure:

```rust
use iroh::net::relay::RelayMode;
use iroh::net::Endpoint;

// configure endpoint with relay
let endpoint = Endpoint::builder()
    .relay_mode(RelayMode::Default)  // use iroh's public relays
    .bind()
    .await?;

// or specify custom relay
let endpoint = Endpoint::builder()
    .relay_mode(RelayMode::Custom(vec![
        RelayUrl::from("https://relay.zkpoker.com"),
    ]))
    .bind()
    .await?;
```

## relay selection

choose best relay:

```rust
struct RelayMetrics {
    /// round-trip latency
    latency: Duration,
    /// packet loss rate
    loss_rate: f64,
    /// current load
    load: f64,
}

fn select_relay(
    my_location: GeoLocation,
    available_relays: &[RelayInfo],
) -> RelayUrl {
    available_relays.iter()
        .map(|r| (r, estimate_latency(my_location, r.location)))
        .min_by_key(|(_, latency)| *latency)
        .unwrap()
        .0
        .url
        .clone()
}
```

## relay fallback

try direct first, fall back to relay:

```
connection strategy:

  1. exchange addresses (including relay URLs)

  2. try direct connection (parallel)
     - all known addresses
     - NAT hole punching

  3. if direct fails after timeout (2s)
     - connect via relay
     - game can start

  4. continue trying direct in background
     - upgrade to direct if successful
     - seamless for players
```

```rust
async fn connect_with_fallback(
    endpoint: &Endpoint,
    peer: &NodeAddr,
) -> Result<Connection, ConnectError> {
    // try direct first
    match tokio::time::timeout(
        Duration::from_secs(2),
        endpoint.connect_direct(peer),
    ).await {
        Ok(Ok(conn)) => return Ok(conn),
        _ => {}  // continue to relay
    }

    // fall back to relay
    endpoint.connect_relay(peer).await
}
```

## relay latency

relayed connections add latency:

```
typical latencies:
  direct connection: 20-50ms
  same-region relay: +10-20ms
  cross-region relay: +50-100ms

for poker:
  - 100ms latency is acceptable
  - actions are turn-based
  - not real-time action game
```

## running a relay

operators can run their own relays:

```bash
# start iroh relay server
iroh-relay --bind 0.0.0.0:443 --tls-cert cert.pem --tls-key key.pem

# configure client to use
let endpoint = Endpoint::builder()
    .relay_mode(RelayMode::Custom(vec![
        RelayUrl::from("https://my-relay.example.com"),
    ]))
    .bind()
    .await?;
```

## relay economics

```
free tier:
  - use public relays (best-effort)
  - may have capacity limits
  - shared with other users

premium ($2/month):
  - dedicated relay access
  - guaranteed capacity
  - priority routing
  - lower latency

relay costs:
  - bandwidth: ~$0.01/GB
  - typical poker session: 10MB
  - 1000 sessions/month: $0.10
  - mostly marketing/support costs
```

## privacy

relays don't compromise privacy:

```
relay threat model:
  - relay operator is honest-but-curious
  - wants to learn game information
  - won't modify messages

protections:
  - all traffic encrypted end-to-end
  - relay can't decrypt
  - relay can't forge messages
  - relay can only see metadata

metadata visible to relay:
  - node IDs of players
  - session duration
  - message sizes/timing
  - cannot determine game state
```

## redundancy

handle relay failures:

```rust
async fn connect_with_redundancy(
    endpoint: &Endpoint,
    peer: &NodeAddr,
    relays: &[RelayUrl],
) -> Result<Connection, ConnectError> {
    // try relays in parallel
    let futures: Vec<_> = relays.iter()
        .map(|relay| endpoint.connect_via_relay(peer, relay))
        .collect();

    // return first successful
    let (conn, _, _) = futures::future::select_all(futures).await;
    conn
}
```

## relay monitoring

operators should monitor:

```
metrics to track:
  - connections per second
  - bandwidth usage
  - latency percentiles
  - error rates

alerts:
  - high latency (>200ms)
  - connection failures
  - capacity limits
```
