# On-Chain PolkaVM Verification

**Status**: Design Phase
**Date**: 2025-11-12

## Major Insight: On-Chain Verification via Polkadot SDK

We can use **Polkadot SDK's `pallet_revive`** and **`sc-executor-polkavm`** to run PolkaVM verification **directly in the runtime**! This is much better than just light client verification.

## Three Verification Strategies

```
┌──────────────────────────────────────────────────────────────┐
│                  AccidentalComputerProof                     │
│                   (ZODA shards ~MB)                          │
└────────────┬─────────────────────────┬───────────────────────┘
             │                         │
             │                         │
     ┌───────▼──────────┐    ┌────────▼─────────┐
     │  Strategy 1:     │    │  Strategy 2:     │
     │  Full Nodes      │    │  On-Chain        │
     │  (Native)        │    │  (PolkaVM)       │
     │                  │    │                  │
     │  verify_         │    │  Ligerito        │
     │  accidental_     │    │  Verifier        │
     │  computer()      │    │  Guest           │
     │                  │    │                  │
     │  ~1-5ms          │    │  Runs in         │
     │  ✅ Fast DA+ZK   │    │  Runtime         │
     │                  │    │                  │
     │                  │    │  via pallet_     │
     │                  │    │  revive          │
     │                  │    │                  │
     │                  │    │  ~20-30ms        │
     │                  │    │  ✅ Consensus    │
     └──────────────────┘    └──────────────────┘
                                      │
                                      │
                             ┌────────▼─────────┐
                             │  Strategy 3:     │
                             │  Light Clients   │
                             │  (Off-chain)     │
                             │                  │
                             │  Download        │
                             │  guest binary    │
                             │                  │
                             │  Run locally     │
                             │  via polkavm     │
                             │                  │
                             │  ~20-30ms        │
                             │  ✅ No bandwidth │
                             └──────────────────┘
```

## Strategy Comparison

| Feature | Strategy 1: Native | Strategy 2: On-Chain PolkaVM | Strategy 3: Light Client |
|---------|-------------------|------------------------------|--------------------------|
| **Where** | Full nodes | Runtime (all nodes) | Client device |
| **Execution** | Native Rust | PolkaVM sandbox | PolkaVM sandbox |
| **Proof Input** | ZODA shards (~MB) | Succinct proof (~KB) | Succinct proof (~KB) |
| **Speed** | ~1-5ms | ~20-30ms | ~20-30ms |
| **Consensus** | ❌ Off-chain | ✅ On-chain | ❌ Off-chain |
| **Gas Cost** | Free | ~XXX gas | Free |
| **Use Case** | Primary verification | Consensus-critical | Sync without state |

## Why On-Chain PolkaVM Verification?

### Problem with Current Design

**Current**: AccidentalComputer verification is **off-chain**
```rust
// blockchain/src/application.rs
fn apply_state_transitions(proofs: &[AccidentalComputerProof]) -> Result<[u8; 32]> {
    // This runs OUTSIDE consensus - different nodes might disagree!
    for proof in proofs {
        if !verify_accidental_computer(config, proof)? {
            bail!("Invalid proof");
        }
    }
    // Update NOMT state
}
```

**Issue**: If there's a bug in `verify_accidental_computer()`, different nodes might:
- Accept different proofs
- Reach different state roots
- Fork the chain!

### Solution: On-Chain Verification

**With pallet_revive**: Verification runs **in the runtime** (consensus)
```rust
// New design using pallet_revive
#[pallet::call]
impl<T: Config> Pallet<T> {
    pub fn verify_state_transition(
        origin: OriginFor<T>,
        proof: LigeritoSuccinctProof,
    ) -> DispatchResult {
        // Extract succinct proof from AccidentalComputer
        let succinct_proof = extract_succinct_proof(&proof)?;

        // Call PolkaVM guest verifier IN THE RUNTIME
        let result = Self::call_polkavm_verifier(&succinct_proof)?;

        ensure!(result, Error::<T>::InvalidProof);

        // Update state (consensus guaranteed)
        Self::update_commitments(&proof)?;

        Ok(())
    }
}
```

**Benefits**:
- ✅ All nodes run same verification (consensus)
- ✅ Deterministic (PolkaVM sandbox)
- ✅ Gas metered (DoS protection)
- ✅ Still fast (~20-30ms per proof)

## Polkadot SDK Components We Can Use

### 1. `sc-executor-polkavm` (Client-Side)

**Location**: `substrate/client/executor/polkavm/`

**Purpose**: Execute PolkaVM programs outside the runtime (for light clients)

**Key Code**:
```rust
pub fn create_runtime<H>(blob: &polkavm::ProgramBlob)
    -> Result<Box<dyn WasmModule>, WasmError>
where
    H: HostFunctions,
{
    let engine = Engine::new(&Config::from_env()?)?;
    let module = Module::from_blob(&engine, &ModuleConfig::default(), blob)?;

    let mut linker = Linker::new();
    for function in H::host_functions() {
        linker.define_untyped(function.name(), |caller| {
            call_host_function(&mut caller, function)
        })?;
    }

    let instance_pre = linker.instantiate_pre(&module)?;
    Ok(Box::new(InstancePre(instance_pre)))
}
```

**Usage for Light Clients**:
```rust
// Light client (off-chain)
let blob = ProgramBlob::parse(&verifier_binary)?;
let runtime = create_runtime::<HostFunctions>(&blob)?;
let instance = runtime.new_instance()?;
let result = instance.call("verify", &proof_bytes)?;
```

### 2. `pallet_revive` (Runtime)

**Location**: `substrate/frame/revive/`

**Purpose**: Execute PolkaVM smart contracts **in the runtime** (on-chain)

**Key Features**:
- Executes PolkaVM programs with gas metering
- Full Substrate runtime integration
- Host function support
- Deterministic execution

**Usage for On-Chain Verification**:
```rust
// In runtime (on-chain)
use pallet_revive::Call;

impl Pallet<T> {
    fn call_polkavm_verifier(proof: &LigeritoSuccinctProof) -> Result<bool> {
        // Deploy Ligerito verifier as a contract
        let verifier_address = Self::verifier_contract_address();

        // Call the verifier contract
        let result = <pallet_revive::Pallet<T>>::call(
            origin,
            verifier_address,
            0, // value
            Weight::from_parts(1_000_000_000, 0), // gas limit
            None, // storage deposit limit
            proof.encode(), // input data
        )?;

        // Decode result (0 = invalid, 1 = valid)
        Ok(result.result.is_ok() && result.return_value[0] == 1)
    }
}
```

## Implementation Plan

### Option A: Hybrid Approach (Recommended)

**Use both native AND on-chain verification**:

```rust
pub enum VerificationStrategy {
    /// Fast path: Native AccidentalComputer verification
    /// - Used by full nodes
    /// - Off-chain (fast but not consensus)
    Native,

    /// Consensus path: On-chain PolkaVM verification
    /// - Used for dispute resolution
    /// - On-chain (slower but consensus-critical)
    OnChain,
}

impl Pallet<T> {
    /// Fast verification (off-chain)
    fn verify_native(proof: &AccidentalComputerProof) -> Result<bool> {
        verify_accidental_computer(&config, proof)
    }

    /// Consensus verification (on-chain)
    fn verify_on_chain(proof: &LigeritoSuccinctProof) -> DispatchResult {
        // Extract succinct proof
        let succinct = extract_succinct_proof(proof)?;

        // Call PolkaVM verifier in runtime
        let result = Self::call_polkavm_verifier(&succinct)?;

        ensure!(result, Error::<T>::InvalidProof);
        Ok(())
    }
}
```

**Workflow**:
1. Block producer does native verification (fast)
2. Other validators check via native (fast)
3. If dispute: Challenge triggers on-chain verification
4. On-chain verification is source of truth

### Option B: Full On-Chain (Purist)

**Always verify on-chain**:

```rust
#[pallet::call]
impl<T: Config> Pallet<T> {
    pub fn submit_proof(
        origin: OriginFor<T>,
        proof: AccidentalComputerProof,
    ) -> DispatchResultWithPostInfo {
        let who = ensure_signed(origin)?;

        // Extract succinct proof (cheap)
        let succinct = extract_succinct_proof(&proof, 24)?;

        // Verify on-chain via PolkaVM (expensive but consensus)
        Self::verify_on_chain(&succinct)?;

        // Update state
        Self::update_commitments(&proof)?;

        Ok(().into())
    }
}
```

**Trade-offs**:
- ✅ Perfect consensus (no disputes possible)
- ✅ Simpler design (one path)
- ❌ Higher gas costs (~20-30ms per proof)
- ❌ Lower throughput

## Architecture: On-Chain Verifier Pallet

### Pallet Structure

```rust
// blockchain/src/verifier/mod.rs

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use frame_support::pallet_prelude::*;
    use frame_system::pallet_prelude::*;

    #[pallet::config]
    pub trait Config: frame_system::Config + pallet_revive::Config {
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

        /// Maximum proof size to accept
        #[pallet::constant]
        type MaxProofSize: Get<u32>;

        /// Gas limit for verification
        #[pallet::constant]
        type VerificationGasLimit: Get<Weight>;
    }

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    /// Address of the deployed Ligerito verifier contract
    #[pallet::storage]
    pub type VerifierContract<T: Config> = StorageValue<_, T::AccountId, OptionQuery>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// Proof verified successfully
        ProofVerified {
            sender_old: [u8; 32],
            sender_new: [u8; 32],
            receiver_old: [u8; 32],
            receiver_new: [u8; 32],
        },

        /// Verifier contract deployed
        VerifierDeployed {
            address: T::AccountId,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        /// Proof is too large
        ProofTooLarge,
        /// Proof verification failed
        InvalidProof,
        /// Verifier contract not deployed
        VerifierNotDeployed,
        /// PolkaVM execution failed
        ExecutionFailed,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Deploy Ligerito verifier contract
        #[pallet::weight(T::VerificationGasLimit::get())]
        #[pallet::call_index(0)]
        pub fn deploy_verifier(
            origin: OriginFor<T>,
            verifier_code: Vec<u8>,
        ) -> DispatchResult {
            ensure_root(origin)?;

            // Deploy PolkaVM contract via pallet_revive
            let deployer = T::AccountId::decode(&mut &[0u8; 32][..])?;

            let deploy_result = <pallet_revive::Pallet<T>>::bare_instantiate(
                deployer,
                0, // value
                Weight::from_parts(10_000_000_000, 0),
                None, // storage deposit
                pallet_revive::Code::Upload(verifier_code),
                vec![], // data
                vec![], // salt
                pallet_revive::DebugInfo::Skip,
                pallet_revive::CollectEvents::Skip,
            );

            ensure!(deploy_result.result.is_ok(), Error::<T>::ExecutionFailed);

            let address = deploy_result.account_id;
            VerifierContract::<T>::put(&address);

            Self::deposit_event(Event::VerifierDeployed { address });

            Ok(())
        }

        /// Verify state transition proof on-chain
        #[pallet::weight(T::VerificationGasLimit::get())]
        #[pallet::call_index(1)]
        pub fn verify_proof(
            origin: OriginFor<T>,
            proof: LigeritoSuccinctProof,
        ) -> DispatchResult {
            let _who = ensure_signed(origin)?;

            // Check proof size
            ensure!(
                proof.proof_bytes.len() <= T::MaxProofSize::get() as usize,
                Error::<T>::ProofTooLarge
            );

            // Get verifier contract address
            let verifier_address = VerifierContract::<T>::get()
                .ok_or(Error::<T>::VerifierNotDeployed)?;

            // Prepare input: [config_size: u32][proof_bytes]
            let mut input = Vec::new();
            input.extend_from_slice(&proof.config_size.to_le_bytes());
            input.extend_from_slice(&proof.proof_bytes);

            // Call verifier contract
            let call_result = <pallet_revive::Pallet<T>>::bare_call(
                T::AccountId::decode(&mut &[0u8; 32][..])?,
                verifier_address,
                0, // value
                T::VerificationGasLimit::get(),
                None, // storage deposit
                input,
                pallet_revive::DebugInfo::Skip,
                pallet_revive::CollectEvents::Skip,
                pallet_revive::Determinism::Enforced,
            );

            // Check execution succeeded
            ensure!(call_result.result.is_ok(), Error::<T>::ExecutionFailed);

            // Check return value (exit code 0 = valid)
            let return_data = call_result.result.unwrap();
            ensure!(!return_data.is_empty() && return_data[0] == 0, Error::<T>::InvalidProof);

            // Emit event
            Self::deposit_event(Event::ProofVerified {
                sender_old: proof.sender_commitment_old,
                sender_new: proof.sender_commitment_new,
                receiver_old: proof.receiver_commitment_old,
                receiver_new: proof.receiver_commitment_new,
            });

            Ok(())
        }
    }
}
```

### Contract Deployment

**Step 1**: Build Ligerito verifier for PolkaVM
```bash
cd examples/polkavm_verifier

# Build for PolkaVM target
. ../../polkaports/activate.sh polkavm
cargo build --release --target riscv64-zkvm-elf

# Output: target/riscv64-zkvm-elf/release/polkavm_verifier
```

**Step 2**: Deploy to runtime
```rust
// One-time deployment (sudo call)
let verifier_binary = std::fs::read("target/riscv64-zkvm-elf/release/polkavm_verifier")?;

RuntimeCall::ZKVerifier(
    pallet_zk_verifier::Call::deploy_verifier {
        verifier_code: verifier_binary,
    }
).dispatch(RawOrigin::Root)?;
```

**Step 3**: Use for verification
```rust
// Anyone can call this
RuntimeCall::ZKVerifier(
    pallet_zk_verifier::Call::verify_proof {
        proof: succinct_proof,
    }
).dispatch(origin)?;
```

## Integration with Existing Code

### Update Application Layer

```rust
// blockchain/src/application.rs

impl Application {
    /// Apply state transitions with on-chain verification
    pub fn apply_state_transitions(
        &self,
        proofs: &[AccidentalComputerProof],
    ) -> Result<[u8; 32]> {
        for proof in proofs {
            // Option A: Native verification (fast path)
            if self.config.use_native_verification {
                if !verify_accidental_computer(&self.config, proof)? {
                    bail!("Invalid proof (native verification)");
                }
            } else {
                // Option B: On-chain verification (consensus)
                let succinct = extract_succinct_proof(proof, 24)?;

                // Submit to runtime for verification
                self.runtime_call(
                    RuntimeCall::ZKVerifier(
                        pallet_zk_verifier::Call::verify_proof { proof: succinct }
                    )
                )?;
            }
        }

        // Update NOMT state
        self.update_nomt_state(proofs)
    }
}
```

## Gas Costs & Performance

### Verification Costs

| Operation | Gas Cost | Time |
|-----------|----------|------|
| Native verification | 0 (off-chain) | ~1-5ms |
| Extract succinct proof | ~100k gas | ~1ms |
| PolkaVM verification | ~5-10M gas | ~20-30ms |
| **Total on-chain** | **~5-10M gas** | **~20-30ms** |

### Comparison with EVM

For reference, EVM zkSNARK verification:
- Groth16: ~280k gas (~3ms)
- PLONK: ~400k gas (~4ms)

Ligerito via PolkaVM:
- ~5-10M gas (~20-30ms)
- **But**: Verifies binary field proofs (faster proving!)

### Throughput Impact

**Block Production**:
- Target: 6 second blocks (JAM-style)
- Available: ~6000ms per block
- Per-proof cost: ~20-30ms on-chain

**Maximum throughput**:
- Conservative: 200 proofs per block (~30ms each)
- Aggressive: 300 proofs per block (~20ms each)

**Realistic**: 100-150 proofs per block (with other operations)

## Benefits of On-Chain Verification

### 1. Consensus Guaranteed

**Problem**: Native verification is off-chain
- Different nodes might disagree
- State divergence possible
- Chain forks

**Solution**: On-chain verification
- All nodes run same code
- Deterministic PolkaVM
- Guaranteed consensus

### 2. Dispute Resolution

**Fraud Proof Pattern**:
```rust
// Optimistic: Accept proofs without verification
impl Pallet<T> {
    pub fn submit_optimistic(proof: AccidentalComputerProof) -> DispatchResult {
        // Store proof without verification
        Proofs::<T>::insert(proof_id, proof);

        // Challenge period: 100 blocks
        Ok(())
    }

    pub fn challenge_proof(proof_id: u32) -> DispatchResult {
        let proof = Proofs::<T>::get(proof_id)?;

        // Force on-chain verification
        Self::verify_on_chain(&proof)?;

        // If invalid: Slash submitter, reward challenger
        Ok(())
    }
}
```

**Benefits**:
- Fast path: No verification (instant)
- Slow path: On-chain verification (dispute)
- Economic security via slashing

### 3. Upgradability

**Deploy new verifier without hard fork**:
```rust
// Deploy Ligerito v2 verifier
RuntimeCall::ZKVerifier(
    pallet_zk_verifier::Call::deploy_verifier {
        verifier_code: ligerito_v2_binary,
    }
).dispatch(RawOrigin::Root)?;
```

**Compare**:
- Native: Requires node upgrade
- On-chain: Just deploy new contract

## Security Considerations

### 1. Determinism

**Critical**: PolkaVM execution must be deterministic

**pallet_revive guarantees**:
- Deterministic RISC-V execution
- Gas metering (no infinite loops)
- Memory bounds
- No floating point non-determinism

### 2. Gas Exhaustion DoS

**Attack**: Submit proof that exhausts gas

**Mitigation**:
```rust
#[pallet::constant]
type VerificationGasLimit: Get<Weight> = ConstU64<10_000_000_000>;

pub fn verify_proof(proof: LigeritoSuccinctProof) -> DispatchResult {
    // Metered via PolkaVM gas
    Self::call_polkavm_verifier(&proof)?;
    Ok(())
}
```

### 3. Code Injection

**Attack**: Deploy malicious verifier

**Mitigation**:
```rust
pub fn deploy_verifier(origin: OriginFor<T>, code: Vec<u8>) -> DispatchResult {
    // Only root can deploy
    ensure_root(origin)?;

    // Future: Add code hash whitelist
    let code_hash = blake3::hash(&code);
    ensure!(
        AllowedVerifiers::<T>::contains_key(code_hash),
        Error::<T>::UnauthorizedCode
    );

    Ok(())
}
```

## Roadmap

### Phase 1: Research & Design (This Session)
- ✅ Understand pallet_revive architecture
- ✅ Understand sc-executor-polkavm
- ✅ Design on-chain verifier pallet
- ✅ Document verification strategies

### Phase 2: Implementation (1 week)
- [ ] Create `pallet_zk_verifier` skeleton
- [ ] Integrate with pallet_revive
- [ ] Wire up Ligerito verifier guest
- [ ] Test deployment and verification
- [ ] Gas benchmarking

### Phase 3: Integration (1 week)
- [ ] Update application layer
- [ ] Add hybrid verification mode
- [ ] Implement fraud proof pattern
- [ ] End-to-end testing

### Phase 4: Optimization (1 week)
- [ ] Optimize proof extraction
- [ ] Cache verifier instances
- [ ] Batch verification
- [ ] Performance tuning

## Comparison: All Three Strategies

### Strategy 1: Native (Current)
```rust
// Off-chain, fast, but no consensus
verify_accidental_computer(&config, &proof)?;
```

**Pros**: Very fast (~1-5ms)
**Cons**: No consensus guarantee

### Strategy 2: On-Chain PolkaVM (New!)
```rust
// In runtime, consensus guaranteed
RuntimeCall::ZKVerifier(verify_proof { proof })?;
```

**Pros**: Consensus, deterministic, upgradeable
**Cons**: Gas costs, ~20-30ms per proof

### Strategy 3: Light Client (Previous Session)
```rust
// Off-chain, sandboxed, no state
let client = LightClient::new(config)?;
client.verify_via_polkavm(&succinct_proof).await?;
```

**Pros**: No bandwidth, sandboxed
**Cons**: No consensus, client-side only

## Recommended Approach

**Hybrid**: Use all three!

```
┌─────────────────────────────────────────────────┐
│              Block Production                   │
│                                                 │
│  1. Native verification (fast path)            │
│     verify_accidental_computer() ~1-5ms        │
│                                                 │
│  2. Include proofs in block                     │
│     Block { proofs: Vec<AccidentalComputer> }  │
└─────────────────┬───────────────────────────────┘
                  │
                  ▼
┌─────────────────────────────────────────────────┐
│              Block Validation                   │
│                                                 │
│  Option A: Native (optimistic)                  │
│    - Fast validation                            │
│    - Challenge period for disputes              │
│                                                 │
│  Option B: On-chain (pessimistic)               │
│    - Always verify via PolkaVM                  │
│    - Perfect consensus                          │
└─────────────────┬───────────────────────────────┘
                  │
                  ▼
┌─────────────────────────────────────────────────┐
│            Light Client Sync                    │
│                                                 │
│  - Extract succinct proofs                      │
│  - Verify via local PolkaVM                     │
│  - No full state needed                         │
└─────────────────────────────────────────────────┘
```

## Next Steps

1. **Create `pallet_zk_verifier`** based on design above
2. **Deploy Ligerito verifier** as PolkaVM contract
3. **Test on-chain verification** with real proofs
4. **Benchmark gas costs** and throughput
5. **Implement fraud proof pattern** (optional)

## Conclusion

Using **Polkadot SDK's pallet_revive**, we can run **PolkaVM verification directly in the runtime**! This gives us:

✅ **Consensus-critical verification** (all nodes agree)
✅ **Deterministic execution** (PolkaVM sandbox)
✅ **Upgradeable verifier** (no hard forks)
✅ **Gas metering** (DoS protection)
✅ **Reasonable cost** (~20-30ms per proof)

This is a **major improvement** over off-chain verification and opens up exciting possibilities like fraud proofs and optimistic rollups!
