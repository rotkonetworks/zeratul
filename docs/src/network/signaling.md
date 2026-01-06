# signaling

signaling helps peers find each other before establishing direct connections.

## the problem

```
peer discovery challenge:
  - A wants to connect to B
  - A knows B's node ID
  - A doesn't know B's IP addresses
  - B might be behind NAT

solutions:
  1. DHT (distributed hash table)
  2. signaling server
  3. blockchain publish
  4. out-of-band exchange
```

## iroh relay tickets

iroh uses relay tickets for initial contact:

```rust
use iroh::net::relay::RelayUrl;
use iroh::net::NodeAddr;

/// information needed to contact a peer
struct PeerInfo {
    /// unique node identifier
    node_id: NodeId,
    /// direct addresses (if known)
    addresses: Vec<SocketAddr>,
    /// relay URL for fallback
    relay_url: Option<RelayUrl>,
}

// encoded as "ticket" for sharing
let ticket = NodeAddr::new(node_id)
    .with_direct_addresses(addresses)
    .with_relay_url(relay_url);

// share as string
let ticket_str = ticket.to_string();
// e.g., "node:5oj...xyz?relay=https://relay.example.com&addr=1.2.3.4:5678"
```

## matchmaking flow

```
player discovery:

  1. player A creates table
     - registers with matchmaking service
     - includes: node ticket, table config

  2. player B browses tables
     - queries matchmaking service
     - receives: table info + A's ticket

  3. B connects to A
     - uses ticket to establish p2p connection
     - no further signaling needed

  ┌──────────┐         ┌──────────────┐         ┌──────────┐
  │ player A │         │ matchmaking  │         │ player B │
  └────┬─────┘         └──────┬───────┘         └────┬─────┘
       │                      │                      │
       │  register table      │                      │
       │ ────────────────────▶│                      │
       │                      │                      │
       │                      │     browse tables    │
       │                      │◀──────────────────── │
       │                      │                      │
       │                      │     table list       │
       │                      │ ────────────────────▶│
       │                      │                      │
       │◀───────────────────────── p2p connect ──────│
       │                      │                      │
```

## matchmaking service

simple table registry:

```rust
struct MatchmakingService {
    /// active tables by ID
    tables: HashMap<TableId, TableListing>,
}

struct TableListing {
    /// table configuration
    config: TableConfig,
    /// creator's node address
    creator_addr: NodeAddr,
    /// creation timestamp
    created_at: Timestamp,
    /// current status
    status: TableStatus,
}

enum TableStatus {
    /// waiting for opponent
    Open,
    /// game in progress
    InProgress,
    /// table closed
    Closed,
}

impl MatchmakingService {
    fn list_tables(&self, filter: &TableFilter) -> Vec<TableListing> {
        self.tables.values()
            .filter(|t| t.status == TableStatus::Open)
            .filter(|t| filter.matches(&t.config))
            .cloned()
            .collect()
    }

    fn register_table(&mut self, listing: TableListing) -> TableId {
        let id = TableId::random();
        self.tables.insert(id, listing);
        id
    }
}
```

## direct invite

skip matchmaking with direct invite:

```
direct invite flow:

  1. A generates invite link
     - includes node ticket
     - optionally: table config

  2. A shares link with B
     - messaging app
     - email
     - QR code

  3. B clicks link
     - client parses ticket
     - connects directly to A
     - skips matchmaking entirely

invite format:
  zkpoker://join?ticket=<base64-node-addr>&stakes=1-2&type=nlhe
```

## DHT discovery

decentralized alternative:

```
DHT approach:
  1. A publishes address to DHT
     key: hash(A's node ID)
     value: A's current addresses

  2. B looks up A in DHT
     searches for hash(A's node ID)
     receives: A's addresses

  3. B connects directly

pros:
  - no central service
  - censorship resistant

cons:
  - slower lookup
  - addresses may be stale
```

## address updates

addresses change over time:

```rust
/// announce address changes
async fn announce_addresses(
    matchmaking: &MatchmakingClient,
    endpoint: &Endpoint,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(60));

    loop {
        interval.tick().await;

        // get current addresses
        let addrs = endpoint.local_endpoints().await;

        // update matchmaking service
        matchmaking.update_addresses(addrs).await?;
    }
}
```

## private tables

tables not listed publicly:

```
private table flow:

  1. A creates private table
     - not registered with matchmaking
     - generates invite code

  2. A shares code with friends
     - out of band

  3. friends join with code
     - direct connection to A

benefit:
  - not discoverable by strangers
  - invite-only access
```

## signaling server

minimal coordination server:

```rust
struct SignalingServer {
    /// pending connections by table
    pending: HashMap<TableId, PendingConnection>,
}

struct PendingConnection {
    /// creator's node address
    creator: NodeAddr,
    /// joiner's node address (once connected)
    joiner: Option<NodeAddr>,
}

impl SignalingServer {
    /// exchange addresses for connection
    async fn exchange(
        &mut self,
        table_id: TableId,
        my_addr: NodeAddr,
    ) -> NodeAddr {
        match self.pending.get_mut(&table_id) {
            Some(pending) if pending.joiner.is_none() => {
                // I'm the joiner
                pending.joiner = Some(my_addr);
                pending.creator.clone()
            }
            Some(pending) => {
                // I'm the creator, joiner already connected
                pending.joiner.clone().unwrap()
            }
            None => {
                // I'm the creator, create entry
                self.pending.insert(table_id, PendingConnection {
                    creator: my_addr,
                    joiner: None,
                });
                // wait for joiner...
            }
        }
    }
}
```

## security

signaling doesn't see game content:

```
signaling server knows:
  - node IDs of players
  - when connections happen
  - table configurations

signaling server doesn't know:
  - game actions
  - cards
  - balances
  - any encrypted content

all game communication is p2p
signaling only for discovery
```
