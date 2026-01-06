# p2p layer

zk.poker uses iroh for peer-to-peer communication. players connect directly when possible, with relay fallback for NAT traversal.

## why p2p

```
server-mediated (traditional):
  player A → server → player B
  - server sees all messages
  - server can censor/manipulate
  - server is single point of failure
  - server costs money to run

peer-to-peer (zk.poker):
  player A ←→ player B
  - direct communication
  - no intermediary sees content
  - no single point of failure
  - minimal infrastructure costs
```

## iroh overview

iroh is a QUIC-based p2p library:

```
features:
  - connection establishment
  - NAT traversal (hole punching)
  - relay fallback
  - multiplexing
  - encryption (noise protocol)

properties:
  - low latency (UDP-based)
  - reliable delivery (QUIC)
  - connection migration
  - 0-RTT resumption
```

## node identification

each player has a unique node ID:

```rust
use iroh::net::NodeId;

/// node ID derived from ghettobox key
fn derive_node_id(signing_key: &SigningKey) -> NodeId {
    // node ID is the public key
    NodeId::from_bytes(signing_key.verifying_key().to_bytes())
}

// example node ID (32 bytes, base32 encoded):
// 5oj...xyz
```

## connection establishment

```
direct connection flow:

  1. exchange addresses
     A: "I'm at 192.168.1.5:8080, 87.65.4.3:12345"
     B: "I'm at 10.0.0.2:8080, 98.76.5.4:54321"

  2. attempt direct connections (parallel)
     A tries B's addresses
     B tries A's addresses

  3. NAT hole punching
     coordinated attempt to traverse NATs

  4. connection established
     first successful path wins

  5. upgrade to direct (if relay)
     continue trying direct in background
```

## message types

```rust
/// messages between poker clients
enum PeerMessage {
    /// game action (bet, fold, etc)
    GameAction(GameAction),

    /// state update with signature
    StateUpdate(SignedState),

    /// shuffle proof
    ShuffleProof(ShuffleProof),

    /// card reveal (decryption share)
    CardReveal(CardReveal),

    /// channel close request
    CloseRequest(CloseRequest),

    /// ping for latency measurement
    Ping(u64),

    /// pong response
    Pong(u64),
}
```

## sending messages

```rust
use iroh::net::endpoint::Connection;

async fn send_message(
    conn: &Connection,
    msg: PeerMessage,
) -> Result<(), NetworkError> {
    // serialize message
    let bytes = bincode::serialize(&msg)?;

    // open bidirectional stream
    let (mut send, mut recv) = conn.open_bi().await?;

    // send with length prefix
    send.write_all(&(bytes.len() as u32).to_le_bytes()).await?;
    send.write_all(&bytes).await?;
    send.finish().await?;

    // wait for acknowledgment
    let mut ack = [0u8; 1];
    recv.read_exact(&mut ack).await?;

    Ok(())
}
```

## receiving messages

```rust
async fn receive_messages(
    conn: Connection,
    tx: mpsc::Sender<PeerMessage>,
) {
    loop {
        // accept incoming stream
        let (mut send, mut recv) = match conn.accept_bi().await {
            Ok(stream) => stream,
            Err(_) => break,  // connection closed
        };

        // read length prefix
        let mut len_bytes = [0u8; 4];
        recv.read_exact(&mut len_bytes).await?;
        let len = u32::from_le_bytes(len_bytes) as usize;

        // read message
        let mut buffer = vec![0u8; len];
        recv.read_exact(&mut buffer).await?;

        // deserialize
        let msg: PeerMessage = bincode::deserialize(&buffer)?;

        // send acknowledgment
        send.write_all(&[1]).await?;

        // forward to application
        tx.send(msg).await?;
    }
}
```

## latency measurement

```rust
struct LatencyTracker {
    samples: VecDeque<Duration>,
    last_ping: Instant,
}

impl LatencyTracker {
    async fn ping(&mut self, conn: &Connection) -> Duration {
        let start = Instant::now();
        let nonce = rand::random();

        // send ping
        send_message(conn, PeerMessage::Ping(nonce)).await?;

        // wait for pong (with matching nonce)
        let pong = receive_pong(conn, nonce).await?;

        let rtt = start.elapsed();
        self.samples.push_back(rtt);

        // keep last 10 samples
        while self.samples.len() > 10 {
            self.samples.pop_front();
        }

        rtt
    }

    fn average_latency(&self) -> Duration {
        let sum: Duration = self.samples.iter().sum();
        sum / self.samples.len() as u32
    }
}
```

## encryption

all connections are encrypted:

```
noise protocol handshake:
  1. exchange ephemeral keys
  2. derive shared secret
  3. encrypt all subsequent traffic

properties:
  - forward secrecy
  - identity hiding
  - replay protection
```

## reconnection

handle dropped connections:

```rust
async fn maintain_connection(
    endpoint: &Endpoint,
    peer: NodeId,
) -> Connection {
    loop {
        match endpoint.connect(peer, "zk-poker").await {
            Ok(conn) => return conn,
            Err(_) => {
                // exponential backoff
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

// connection monitoring
async fn monitor_connection(conn: &Connection) {
    loop {
        if conn.is_closed() {
            // trigger reconnection
            break;
        }

        // periodic health check
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
```

## bandwidth usage

```
typical message sizes:
  game action: ~100 bytes
  state update: ~500 bytes
  shuffle proof: ~10 KB
  card reveal: ~200 bytes

per hand (~10 actions):
  ~15 KB total

bandwidth for continuous play:
  ~50 hands/hour = 750 KB/hour
  very lightweight
```
