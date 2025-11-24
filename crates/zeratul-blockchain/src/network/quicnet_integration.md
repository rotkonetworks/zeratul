# Integrating quicnet for Zeratul Networking

## Why Use quicnet?

`quicnet` (in `../quicnet`) already implements:
✅ QUIC transport with Quinn
✅ Ed25519 identity-based auth
✅ Self-signed TLS certificates
✅ Mutual authentication
✅ Peer management
✅ WebTransport support (optional)

This is 90% of what we need for JAMNP-S! Just need to adapt the protocol.

## Differences: quicnet vs JAMNP-S

| Feature | quicnet | JAMNP-S (JAM spec) | Adaptation |
|---------|---------|-------------------|------------|
| **Identity** | Ed25519 | Ed25519 | ✅ Same |
| **TLS Certs** | Self-signed | Self-signed | ✅ Same |
| **Alternative Name** | Base32 PeerId | Base32 from spec | ⚠️ Check encoding matches |
| **ALPN** | `quicnet/1` | `jamnp-s/0/{genesis}` | ⚠️ Change ALPN |
| **Streams** | Single bi-stream | UP/CE protocols | ⚠️ Add stream routing |
| **Auth** | Challenge-response | TLS mutual auth | ✅ TLS is primary |

## Integration Plan

### Phase 1: Add quicnet Dependency
```toml
[dependencies]
quicnet = { path = "../../quicnet" }
```

### Phase 2: Wrap quicnet Peer
```rust
use quicnet::{Identity, Peer, PeerId};

pub struct ValidatorPeer {
    inner: Peer,
    genesis_hash: [u8; 32],
    stream_handlers: HashMap<u8, Box<dyn StreamHandler>>,
}

impl ValidatorPeer {
    pub fn new(
        bind_addr: SocketAddr,
        identity: Identity,
        genesis_hash: [u8; 32],
    ) -> Result<Self> {
        // Create quicnet peer
        let inner = Peer::new(bind_addr, identity)?;

        // TODO: Override ALPN to jamnp-s/0/{genesis}

        Ok(Self {
            inner,
            genesis_hash,
            stream_handlers: HashMap::new(),
        })
    }

    pub fn register_handler(&mut self, kind: StreamKind, handler: Box<dyn StreamHandler>) {
        self.stream_handlers.insert(kind.to_byte(), handler);
    }

    pub async fn connect(&self, addr: SocketAddr) -> Result<Connection> {
        self.inner.dial(addr, None).await.map(|(conn, _)| conn)
    }

    pub async fn accept(&self) -> Result<IncomingConnection> {
        self.inner.accept().await
    }
}
```

### Phase 3: Stream Protocol Layer
```rust
// Wrap quinn::Connection to add stream kind routing
pub struct JamConnection {
    conn: quinn::Connection,
    handlers: Arc<HashMap<u8, Box<dyn StreamHandler>>>,
}

impl JamConnection {
    pub async fn open_stream(&self, kind: StreamKind) -> Result<JamStream> {
        let mut stream = self.conn.open_bi().await?;

        // Send stream kind byte (JAM spec)
        stream.0.write_all(&[kind.to_byte()]).await?;

        Ok(JamStream { send: stream.0, recv: stream.1, kind })
    }

    pub async fn handle_incoming(&self) -> Result<()> {
        loop {
            let stream = self.conn.accept_bi().await?;
            let handlers = self.handlers.clone();

            tokio::spawn(async move {
                if let Err(e) = handle_stream(stream, handlers).await {
                    warn!("Stream error: {}", e);
                }
            });
        }
    }
}

async fn handle_stream(
    mut stream: (quinn::SendStream, quinn::RecvStream),
    handlers: Arc<HashMap<u8, Box<dyn StreamHandler>>>,
) -> Result<()> {
    // Read stream kind byte
    let mut kind_byte = [0u8; 1];
    stream.1.read_exact(&mut kind_byte).await?;

    let kind = StreamKind::from_byte(kind_byte[0])
        .ok_or_else(|| anyhow::anyhow!("Invalid stream kind"))?;

    // Find handler
    let handler = handlers.get(&kind_byte[0])
        .ok_or_else(|| anyhow::anyhow!("No handler for stream kind"))?;

    // Read message
    let mut size_bytes = [0u8; 4];
    stream.1.read_exact(&mut size_bytes).await?;
    let size = u32::from_le_bytes(size_bytes) as usize;

    let mut data = vec![0u8; size];
    stream.1.read_exact(&mut data).await?;

    // Handle
    let response = handler.handle_stream(kind, data)?;

    // Send response
    if !response.is_empty() {
        stream.0.write_all(&(response.len() as u32).to_le_bytes()).await?;
        stream.0.write_all(&response).await?;
    }

    stream.0.finish()?;
    Ok(())
}
```

### Phase 4: DKG Integration
```rust
pub struct DKGNetwork {
    peer: ValidatorPeer,
    dkg_manager: Arc<Mutex<DKGManager>>,
}

impl DKGNetwork {
    pub fn new(peer: ValidatorPeer, dkg_manager: Arc<Mutex<DKGManager>>) -> Self {
        let mut peer = peer;

        // Register DKG handlers
        peer.register_handler(
            StreamKind::DKGBroadcast,
            Box::new(DKGBroadcastHandler::new(dkg_manager.clone())),
        );

        Self { peer, dkg_manager }
    }

    pub async fn broadcast_dkg(&self, epoch: u64, bmsg: BroadcastMsg) -> Result<()> {
        let validators = self.get_validators(epoch)?;

        for validator in validators {
            let conn = self.peer.connect(validator.address).await?;
            let mut stream = conn.open_stream(StreamKind::DKGBroadcast).await?;

            let msg = DKGBroadcast::new(epoch, &self.peer.identity.public_key(), bmsg.clone());
            stream.send(&bincode::serialize(&msg)?).await?;
        }

        Ok(())
    }
}
```

## Alternative Name Verification

quicnet already implements base32 encoding for PeerIds. Let's verify it matches JAM spec:

**JAM spec**: `N(k) = "e" + B(E32^-1(k), 52)` where B is base32 with alphabet `abcdefghijklmnopqrstuvwxyz234567`

**quicnet**: Check `src/identity.rs` for PeerId encoding

If they match: ✅ No changes needed
If different: Modify quicnet's encoding to match JAM

## ALPN Modification

Need to change ALPN from `quicnet/1` to `jamnp-s/0/{genesis_hash}`:

```rust
// In peer.rs, modify:
crypto.alpn_protocols = vec![
    format!("jamnp-s/0/{}", hex::encode(&self.genesis_hash[..4])).as_bytes().to_vec()
];
```

Or: Fork quicnet and add ALPN customization option

## WebTransport for Light Clients

quicnet already supports WebTransport via feature flag:
```toml
quicnet = { path = "../../quicnet", features = ["webtransport"] }
```

This gives us browser compatibility for free!

```rust
// Enable WebTransport endpoint
let web_peer = Peer::new_web(bind_addr, identity, cert_path)?;

// Browsers can connect via:
// const transport = new WebTransport("https://validator.example.com:443");
```

## Testing Plan

1. **Unit tests**: Stream routing, message framing
2. **Integration tests**: 2-node DKG, 4-node consensus
3. **Network tests**: Deploy to testnet, measure latency
4. **Browser tests**: WebTransport from browser light client

## Implementation Steps

- [x] Document integration plan
- [ ] Add quicnet dependency to Cargo.toml
- [ ] Verify alternative name encoding matches JAM
- [ ] Add ALPN customization (or fork quicnet)
- [ ] Implement stream kind routing layer
- [ ] Add DKG protocol handlers
- [ ] Test 4-validator local network
- [ ] Enable WebTransport for light clients
- [ ] Deploy to testnet

## Benefits

✅ **Reuse tested code**: quicnet is already battle-tested
✅ **WebTransport included**: Browser support for free
✅ **Less code to maintain**: Focus on blockchain logic, not networking
✅ **Faster development**: Working QUIC in hours, not weeks
