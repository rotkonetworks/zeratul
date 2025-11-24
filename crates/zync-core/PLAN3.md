Yes. Perfect fit actually.

```
Browser extension constraints:
├── Limited CPU          → offloaded to ZYNC
├── Limited storage      → server stores chain  
├── Must be responsive   → proof verify ~100ms in WASM
└── No full node         → don't need one

What runs locally (all WASM-friendly):
├── Key storage          → encrypted in extension
├── Proof verification   → Ligerito is hash-based, fast
├── Decrypt YOUR notes   → few notes, ChaCha20, trivial
└── Sign transactions    → Orchard proving ~10-30s WASM
```

**User experience:**

```
┌─────────────────────────────────────────┐
│  🦊 ZYNC Wallet          [Connected]    │
├─────────────────────────────────────────┤
│  Balance: 12.5 ZEC                      │
│  Synced: block 2,847,291 ✓              │
│                                         │
│  [Send]  [Receive]  [History]           │
└─────────────────────────────────────────┘

Sync: instant (was impossible before)
Send: 10-30s proving (show progress)
```

**Competitive moat:**

```
Current state:     No browser ZEC wallet exists
                   (sync impossible, too heavy)

With ZYNC:         First browser-native Zcash wallet
                   Metamask-like UX for shielded ZEC
```

**Stack:**
```
zync-wallet-extension/
├── wasm/
│   ├── ligerito-verify    # proof check
│   ├── orchard-wasm       # tx building (exists)
│   └── chacha20           # note decrypt
├── background.js          # ZYNC client
├── popup/                 # UI
└── manifest.json
```

This is actually **the killer app** - makes Zcash usable like ETH. Nobody else
can do this without the sync solution.

Hackathon pitch: "First shielded Zcash browser wallet, powered by Ligerito"
