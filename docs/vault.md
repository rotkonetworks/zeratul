# ghettobox vault

pin-protected secret recovery service. users register secrets protected by a PIN, and can recover them later by providing the correct PIN. after too many wrong guesses, the secret is destroyed.

## builds

two vault implementations:

| binary | sealing | sandbox | use case |
|--------|---------|---------|----------|
| `ghettobox-vault` | software or TPM | none | production with TPM |
| `ghettobox-vault-pvm` | software (polkavm host) | polkavm | sandboxed guest code |

build both:
```sh
cargo build --release -p ghettobox-vault
cargo build --release -p ghettobox-vault-pvm
```

## running

### vault-pvm (recommended for testing)

```sh
./target/release/ghettobox-vault-pvm \
  --port 4200 \
  --data-dir /var/lib/ghettobox-vault \
  --index 1
```

options:
- `--port` - HTTP API port (default: 4200)
- `--data-dir` - storage directory for keys and database
- `--index` - provider index 1-3 (for multi-provider setups)
- `--bind` - bind address (default: 0.0.0.0)
- `--metrics-port` - prometheus metrics port (default: port + 1000)
- `--blob` - path to polkavm guest blob (default: vault-guest.polkavm)

### vault with TPM

```sh
./target/release/ghettobox-vault \
  --mode tpm \
  --port 4200 \
  --data-dir /var/lib/ghettobox-vault
```

### vault software mode

```sh
./target/release/ghettobox-vault \
  --mode software \
  --port 4200 \
  --data-dir /var/lib/ghettobox-vault
```

## OS permissions

### software mode (vault-pvm or vault --mode software)

no special permissions. needs:
- read/write to `$DATA_DIR/`
- network bind on specified port

### TPM mode (vault --mode tpm)

requires TPM device access:
```sh
# check TPM exists
ls -la /dev/tpm*

# add user to tpm group
sudo usermod -aG tpm $USER

# or run as root (not recommended for production)
```

devices tried in order:
1. `/dev/tpmrm0` - resource manager (preferred)
2. `/dev/tpm0` - direct access

SELinux/AppArmor may need policy exceptions for TPM access.

## data directory layout

```
$DATA_DIR/
├── node.key          # ed25519 signing key (0600)
└── db/               # sled database
    └── ...
```

the signing key is generated on first run. back it up - losing it means losing ability to unseal existing registrations.

## HTTP API

### GET /
node info
```json
{
  "version": "0.1.0",
  "index": 1,
  "pubkey": "abc123...",
  "registrations": 42,
  "mode": "polkavm"
}
```

### GET /health
returns `ok`

### POST /register
register a new secret
```sh
curl -X POST http://localhost:4200/register \
  -H "Content-Type: application/json" \
  -d '{
    "user_id": [32 bytes as array],
    "unlock_tag": [16 bytes as array],
    "encrypted_share": [bytes as array],
    "allowed_guesses": 5
  }'
```

response:
```json
{
  "ok": true,
  "node_index": 1,
  "signature": "hex..."
}
```

### POST /recover
recover a secret
```sh
curl -X POST http://localhost:4200/recover \
  -H "Content-Type: application/json" \
  -d '{
    "user_id": [32 bytes as array],
    "unlock_tag": [16 bytes as array]
  }'
```

success response:
```json
{
  "ok": true,
  "share": {
    "index": 1,
    "data": "hex..."
  },
  "guesses_remaining": 4,
  "error": null
}
```

wrong PIN response:
```json
{
  "ok": false,
  "share": null,
  "guesses_remaining": 3,
  "error": "invalid pin, 3 guesses remaining"
}
```

### GET /status/{user_id}
check registration status
```sh
curl http://localhost:4200/status/0102030405...  # 32 byte hex user_id
```

response:
```json
{
  "registered": true,
  "guesses_remaining": 5,
  "locked": false
}
```

## PSS reshare endpoints (vault-pvm only)

for proactive secret sharing with provider rotation:

### GET /reshare/status
```json
{
  "configured": true,
  "provider_index": 1,
  "reshare_active": false,
  "epoch": null,
  "commitment_count": null,
  "has_quorum": null
}
```

### POST /reshare/epoch
start a new reshare epoch
```sh
curl -X POST http://localhost:4200/reshare/epoch \
  -H "Content-Type: application/json" \
  -d '{
    "epoch": 1,
    "old_threshold": 3,
    "new_threshold": 3,
    "old_provider_count": 5,
    "new_provider_count": 7
  }'
```

### GET /reshare/epoch
get current epoch state

### POST /reshare/commitment
submit dealer commitment

### GET /reshare/commitment
get our dealer commitment

### GET /reshare/subshare/{player_index}
get subshare for a new provider

### GET /reshare/verify
verify group key reconstruction after quorum

## metrics

prometheus metrics on `--metrics-port` (default: API port + 1000):
```
vault_requests_total{endpoint="register"}
vault_requests_total{endpoint="recover"}
vault_registrations_total
vault_recoveries_total
vault_failed_attempts_total
vault_lockouts_total
vault_registrations_current
vault_request_duration_seconds{endpoint="..."}
```

## multi-provider setup

for 2-of-3 threshold recovery, run 3 vault instances:

```sh
# provider 1
./ghettobox-vault-pvm --port 4201 --index 1 --data-dir /data/vault1

# provider 2
./ghettobox-vault-pvm --port 4202 --index 2 --data-dir /data/vault2

# provider 3
./ghettobox-vault-pvm --port 4203 --index 3 --data-dir /data/vault3
```

clients distribute shares to all 3 and need responses from any 2 to recover.

## security notes

- **PIN stretching**: client should stretch PIN with argon2id before sending unlock_tag
- **rate limiting**: after `allowed_guesses` wrong attempts, registration is deleted
- **TPM dictionary attack protection**: hardware TPM enforces additional rate limits
- **no secret logging**: vault never logs secrets or PINs
- **key isolation**: each provider has independent signing key, cannot access others' shares
