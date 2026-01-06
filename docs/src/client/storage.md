# local storage

the client stores some data locally for performance and convenience. sensitive data is never stored unencrypted.

## what's stored locally

```
local storage contents:

  ┌─────────────────────────────────────────────┐
  │ settings/preferences                         │
  │   - UI preferences                           │
  │   - signing policy                           │
  │   - notification settings                    │
  └─────────────────────────────────────────────┘

  ┌─────────────────────────────────────────────┐
  │ session data (optional)                      │
  │   - encrypted session token                  │
  │   - last used address                        │
  │   - recent tables                            │
  └─────────────────────────────────────────────┘

  ┌─────────────────────────────────────────────┐
  │ hand history (optional)                      │
  │   - completed hands                          │
  │   - for analysis purposes                    │
  │   - can be disabled                          │
  └─────────────────────────────────────────────┘

  ┌─────────────────────────────────────────────┐
  │ channel state cache                          │
  │   - latest channel states                    │
  │   - for quick reconnection                   │
  │   - synced with peers                        │
  └─────────────────────────────────────────────┘
```

## what's NOT stored

```
never stored locally:
  ✗ private keys
  ✗ seeds / mnemonics
  ✗ PIN (only in memory during derivation)
  ✗ access keys
  ✗ unencrypted sensitive data

these exist only:
  - in memory (during session)
  - in vault nodes (encrypted)
```

## storage location

```
platform-specific paths:

linux:
  ~/.local/share/zk-poker/

macos:
  ~/Library/Application Support/zk-poker/

windows:
  %APPDATA%\zk-poker\

structure:
  zk-poker/
  ├── config.toml       # settings
  ├── session.enc       # encrypted session (if saved)
  ├── channels/         # channel state cache
  │   └── <channel_id>/
  └── history/          # hand history (optional)
      └── hands.db
```

## settings storage

```rust
#[derive(Serialize, Deserialize)]
struct ClientSettings {
    /// signing policy configuration
    signing_policy: SigningPolicy,

    /// UI preferences
    ui: UiSettings,

    /// network preferences
    network: NetworkSettings,

    /// privacy settings
    privacy: PrivacySettings,
}

#[derive(Serialize, Deserialize)]
struct UiSettings {
    theme: Theme,
    table_felt_color: Color,
    card_back_style: CardBackStyle,
    sound_enabled: bool,
    animations_enabled: bool,
}

#[derive(Serialize, Deserialize)]
struct PrivacySettings {
    /// store hand history locally
    save_hand_history: bool,
    /// share stats with opponents (HUD)
    share_stats: bool,
    /// remember last used email
    remember_email: bool,
}
```

## session storage

encrypted session for quick login:

```rust
struct EncryptedSession {
    /// encrypted session token
    ciphertext: Vec<u8>,
    /// nonce for decryption
    nonce: [u8; 12],
    /// key derivation salt
    salt: [u8; 32],
}

fn save_session(
    session_token: &[u8],
    device_key: &[u8; 32],  // from device keychain
) -> EncryptedSession {
    let nonce = random_nonce();
    let ciphertext = chacha20poly1305::encrypt(
        device_key,
        &nonce,
        session_token,
    );

    EncryptedSession { ciphertext, nonce, salt: [0; 32] }
}
```

## channel state cache

for recovery and quick startup:

```rust
struct ChannelCache {
    /// channel identifier
    channel_id: [u8; 32],
    /// latest known state
    latest_state: ChannelState,
    /// game state details
    game_state: Option<GameState>,
    /// peer address
    peer_addr: NodeAddr,
    /// last update timestamp
    updated_at: Timestamp,
}

// on startup:
// 1. load cached state
// 2. connect to peer
// 3. sync any missed updates
// 4. resume game if in progress
```

## hand history

optional local history for analysis:

```rust
struct HandRecord {
    /// unique hand identifier
    hand_id: [u8; 32],
    /// when hand was played
    timestamp: Timestamp,
    /// table configuration
    table_config: TableConfig,
    /// all actions taken
    actions: Vec<HandAction>,
    /// final result
    result: HandResult,
    /// hole cards (if shown)
    my_cards: Option<[Card; 2]>,
    /// opponent cards (if shown)
    opponent_cards: Option<[Card; 2]>,
}

// stored in SQLite database
// can export to CSV/JSON
// premium feature: sync across devices
```

## clearing data

```rust
fn clear_all_data() {
    // remove settings
    fs::remove_file(settings_path())?;

    // remove session
    fs::remove_file(session_path())?;

    // remove channel cache
    fs::remove_dir_all(channels_path())?;

    // remove hand history
    fs::remove_dir_all(history_path())?;
}

fn clear_sensitive_only() {
    // keep settings, remove everything else
    fs::remove_file(session_path())?;
    fs::remove_dir_all(channels_path())?;
}
```

## backup

```
local data backup:

  important to backup:
    - hand history (if you want to keep it)
    - settings (convenience)

  not needed to backup:
    - session (will re-login)
    - channel cache (syncs from peers)

  backup location:
    - export to file
    - cloud storage (user's choice)
```

## storage limits

```
typical storage usage:

  settings: ~10 KB
  session: ~1 KB
  channel cache: ~100 KB per channel
  hand history: ~5 KB per hand

  after 1000 hands: ~5 MB total

  auto-cleanup:
    - old channel caches (30 days)
    - very old hand history (optional)
```
