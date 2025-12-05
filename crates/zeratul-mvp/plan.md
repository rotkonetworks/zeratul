# Zeratul MVP - Implementation Plan

## Phase 1: Core Infrastructure (DONE)

- [x] Types, state, accumulator
- [x] Ligerito proofs for block production
- [x] Instant BFT consensus (leaderless)
- [x] 2D ZODA data availability
- [x] Service registry
- [x] Basic P2P networking (litep2p)
- [x] Epoch-based validator changes (design in todo.md)

## Phase 2: Adopt from Smoldot

Rather than reinventing, take proven code from smoldot full node:

### From smoldot/full-node:
- **Block sync protocol** - `smoldot/full-node/src/network_service/`
- **Chain database** - Block storage, state trie
- **Grandpa/finality** - Adapt for our 2/3+1 BFT
- **Transaction pool** - Adapt for WorkPackage mempool
- **JSON-RPC** - For external interaction

### Adaptation needed:
- Replace WASM runtime with CoreVM/PolkaVM
- Replace grandpa with our instant BFT
- Replace extrinsics with WorkPackages

## Phase 3: CoreVM Integration (PRIORITY)

Focus area - connect polkavm-pcvm to zeratul-mvp:

```
Browser                          Zeratul Chain
┌─────────────────┐              ┌─────────────────┐
│ CoreVM (WASM)   │              │ polkavm-pcvm    │
│ ─────────────   │  WorkPackage │ verify_sound()  │
│ Run RISC-V code │────────────▶│                 │
│ Generate trace  │  + proof    │ If valid:       │
│ Prove with      │              │ Accumulate      │
│ Ligerito        │              │ to service      │
└─────────────────┘              └─────────────────┘
```

### Tasks:
1. [ ] Add polkavm-pcvm dependency to zeratul-mvp (feature flag exists)
2. [ ] Update accumulator.verify_work_package() to call verify_sound()
3. [ ] Define WorkPackage proof format (SoundPolkaVMProof serialized)
4. [ ] Service registry tracks program_hash for each service
5. [ ] Browser SDK: CoreVM WASM + prove workflow

### Integration points:

```rust
// accumulator.rs - update verify_work_package
#[cfg(feature = "polkavm")]
fn verify_work_package(&self, package: &WorkPackage, state: &State) -> VerifyResult {
    // Get expected program hash from service registry
    let expected_program = state.service_program_hash(package.service)?;

    // Deserialize proof
    let proof: SoundPolkaVMProof = bincode::deserialize(&package.proof)?;

    // Get expected state roots from service
    let expected_initial = state.service_state_root(package.service);

    // Verify using polkavm-pcvm
    let config = ligerito::hardcoded_config_12_verifier();
    polkavm_pcvm::verify_sound(
        &proof,
        &config,
        expected_program,
        expected_initial,
        proof.final_state_root,  // New state
    )?;

    VerifyResult::Valid
}
```

## Phase 4: Testnet Deployment

1. Deploy 3+ validators on Linux servers
2. Genesis with initial services
3. Browser client for submitting work
4. Block explorer / monitoring

## Current Priority

**Focus on zidecar and zanchor** - the deployed infrastructure.
CoreVM integration can proceed in parallel as time allows.

## References

- smoldot: https://github.com/smol-dot/smoldot
- polkavm: https://github.com/koute/polkavm
- polkaports: https://github.com/paritytech/polkaports
