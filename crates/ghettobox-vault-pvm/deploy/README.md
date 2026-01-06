# ghettobox deployment

## local development

### option 1: dev mode (simple, no relay chain)

start zanchor in dev mode:
```bash
cd ~/rotko/zeratul/crates/zanchor
cargo build --release -p zanchor-cli
./target/release/zanchor-cli --dev --tmp
```

this gives you a local chain at ws://127.0.0.1:9944 with:
- alice, bob, charlie dev accounts (pre-funded)
- shielded pool pallet
- 6-second block time

### option 2: zombienet (full parachain)

requires: polkadot, polkadot-omni-node

```bash
# install zombienet (one-time)
cargo install zombienet

# build zanchor runtime
cd ~/rotko/zeratul/crates/zanchor
cargo build --release -p zanchor-runtime

# spawn network
cd ~/rotko/ghettobox
zombienet spawn deploy/zombienet.toml
```

endpoints:
- relay alice: ws://127.0.0.1:9944
- relay bob: ws://127.0.0.1:9945
- para collator 1: ws://127.0.0.1:9988
- para collator 2: ws://127.0.0.1:9989

### option 3: testnet (paseo)

connect to ghettobox testnet on paseo:
```bash
# in poker-client, set endpoint
CHAIN_ENDPOINT=wss://ghettobox.rotko.net cargo run --release
```

## vault providers (for identity recovery)

start 3 vault providers for threshold key recovery:

```bash
# terminal 1
cd ~/rotko/ghettobox
cargo run -p vault-pvm -- --port 3001 --provider 1

# terminal 2
cargo run -p vault-pvm -- --port 3002 --provider 2

# terminal 3
cargo run -p vault-pvm -- --port 3003 --provider 3
```

## poker client

```bash
cd ~/rotko/zeratul/crates/poker-client
POKER_DEBUG=1 cargo run --release
```

keys:
- F1: toggle competitive mode (vsync)
- F2: toggle blockout training
- F12: toggle debug mode

## p2p multiplayer

player 1 (host):
```bash
cargo run --release
# click "create table" in lobby
# share the 3-word code
```

player 2:
```bash
cargo run --release
# enter the 3-word code to join
```

the p2p connection uses iroh QUIC with NAT holepunching.

## architecture

```
poker-client
    ├── auth (ghettobox identity)
    ├── lobby (table discovery)
    ├── p2p (iroh networking)
    ├── mental_poker (card shuffling)
    ├── chat + voice
    └── chain-client (testnet connection)

vault-pvm
    └── pss (proactive secret sharing)
        ├── threshold recovery
        └── provider reshare
```
