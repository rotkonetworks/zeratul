# zafu - zen wallet roadmap

## phase 1: core wallet (MVP)

### onboarding flow
- [ ] splash screen with zafu branding
- [ ] two-path entry: "restore from backup" / "create new wallet"
- [ ] create wallet: password → generate seed → show seed backup screen
- [ ] restore wallet: enter 24-word seed → set birthday height → password
- [ ] seed phrase display with copy button (secure, no screenshots)
- [ ] birthday height picker (block height or date)

### wallet core
- [ ] derive unified spending key from seed
- [ ] derive unified viewing key for balance checking
- [ ] keep spending key in RAM (zeroized on logout/close)
- [ ] session-based encryption (no repeated password entry)
- [ ] proper BIP-39 implementation (replace placeholder)

### sync & verification
- [x] connect to zidecar for sync
- [x] ligerito proof verification (gigaproof)
- [ ] show sync progress with proven block indicator
- [ ] "blocks proven by ligerito" badge in UI
- [ ] background sync with notification

## phase 2: send/receive

### receive
- [ ] unified address display with QR code
- [ ] copy address button
- [ ] address type selector (unified/sapling/transparent)
- [ ] share address via system share sheet

### send
- [ ] address input with validation
- [ ] QR scanner for address
- [ ] amount input with ZEC/USD toggle
- [ ] memo field (512 bytes max)
- [ ] fee estimation display
- [ ] transaction review screen
- [ ] broadcast and confirmation tracking

### transaction history
- [ ] list view of all transactions
- [ ] filter by sent/received/pending
- [ ] transaction detail view
- [ ] personal notes on transactions

## phase 3: address book & contacts

### address book
- [ ] add contact: name + unified address
- [ ] edit/delete contacts
- [ ] quick select when sending
- [ ] import/export contacts (encrypted)

### contact discovery
- [ ] optional: register handle with zidecar
- [ ] resolve handles to addresses
- [ ] contact avatars (generated from address hash)

## phase 4: encrypted chat (z-messages)

### chat core
- [ ] IRC-style chat windows per contact
- [ ] send message via shielded memo (0.0001 ZEC dust)
- [ ] receive and display incoming memos
- [ ] message history stored locally (encrypted)
- [ ] unread message indicators

### chat UX
- [ ] contact list with last message preview
- [ ] chat bubbles with timestamps
- [ ] message status (sent/delivered/read via reply)
- [ ] typing indicator (optional, via dust tx)

### group chat (future)
- [ ] shared viewing key for group
- [ ] group admin management
- [ ] encrypted group metadata

## phase 5: file transfer (magic wormhole)

### wormhole integration
- [ ] generate wormhole code for file
- [ ] send wormhole code via z-message
- [ ] receiver clicks to accept transfer
- [ ] progress indicator for transfer
- [ ] file preview before accept

### file types
- [ ] images with preview
- [ ] documents
- [ ] voice messages (record & send)
- [ ] small videos

## phase 6: settings & security

### settings screen
- [ ] view seed phrase (requires password)
- [ ] change password
- [ ] server selection (zidecar URL)
- [ ] export wallet data
- [ ] delete wallet (with confirmation)

### security
- [ ] biometric unlock option
- [ ] auto-lock timeout
- [ ] secure screen flag (no screenshots)
- [ ] connection over tor (optional)

## phase 7: multiplatform (crux)

### architecture refactor
- [ ] move business logic to crux shared core
- [ ] platform-specific UI shells
- [ ] shared state management

### platforms
- [ ] linux (egui) - current
- [ ] macos (egui)
- [ ] windows (egui)
- [ ] android (jetpack compose)
- [ ] ios (swiftui)
- [ ] web (wasm + egui)

## technical debt

- [ ] proper BIP-39 crate integration
- [ ] handle orchard key derivation
- [ ] proper error handling throughout
- [ ] unit tests for crypto operations
- [ ] integration tests with zidecar
- [ ] CI/CD pipeline

## design principles

1. **zen minimalism** - clean, uncluttered interface
2. **privacy first** - shielded by default, no metadata leaks
3. **proof transparency** - always show when blocks are proven
4. **fast UX** - key in RAM, no repeated password prompts
5. **e2ee messaging** - chat as natural extension of payments
