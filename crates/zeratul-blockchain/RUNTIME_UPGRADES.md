# Runtime Upgrades for Zeratul

**Date**: 2025-11-12
**Status**: Design phase

---

## Overview

Inspired by Polkadot's forkless upgrade mechanism, Zeratul will support **on-chain runtime upgrades** without requiring hard forks.

### Key Insight: ZODA + PolkaVM = Upgradeable Runtime

```
Traditional Blockchain:
└─> Hard fork required for protocol changes
    ├─> Coordinated node upgrade
    ├─> Risk of chain split
    └─> Downtime during upgrade

Zeratul:
└─> On-chain ZODA upgrade
    ├─> New PolkaVM bytecode published
    ├─> Validators execute via FROST governance (13/15)
    ├─> Light clients automatically use new runtime
    └─> Zero downtime!
```

---

## Polkadot's Runtime Model

### How Polkadot Does It

**Runtime = Wasm Blob**:
```
Substrate Runtime (Rust)
└─> Compiled to Wasm
    └─> Stored on-chain
        └─> Executed by native runtime
```

**Upgrade Process**:
1. Governance proposes new runtime (Wasm blob)
2. Token holders vote
3. If approved, runtime stored on-chain
4. Next block uses new runtime
5. **No node restart required!**

**Storage**:
```
Chain State:
├─> :code → [Wasm bytecode blob]
└─> :heappages → [Memory allocation]
```

---

## Zeratul's Runtime Model

### Our Approach: PolkaVM + ZODA

**Runtime = PolkaVM Program**:
```
Zeratul Runtime (Rust)
└─> Compiled to PolkaVM bytecode
    └─> Encoded as ZODA
        ├─> Executable (PolkaVM can run it)
        ├─> Committed (Ligerito proof)
        └─> Verifiable (light clients)
```

**Upgrade Process**:
1. Governance proposes new runtime (PolkaVM bytecode)
2. 13/15 validators approve (Supermajority FROST)
3. Runtime encoded as ZODA + Ligerito proof
4. Published on-chain
5. **Validators execute immediately**
6. **Light clients verify via Ligerito**

---

## Architecture

### Runtime Storage

```rust
/// On-chain runtime specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeSpec {
    /// Runtime version
    pub version: RuntimeVersion,

    /// PolkaVM bytecode
    pub bytecode: Vec<u8>,

    /// ZODA encoding of bytecode
    pub zoda_encoding: Vec<BinaryElem32>,

    /// Ligerito proof (for light client verification)
    pub ligerito_proof: LigeritoProof,

    /// FROST signature (13/15 supermajority)
    pub frost_signature: FrostSignature,

    /// Activation block
    pub activates_at: u64,
}

/// Runtime version (semantic versioning)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeVersion {
    /// Spec name (e.g., "zeratul")
    pub spec_name: &'static str,

    /// Implementation name (e.g., "zeratul-node")
    pub impl_name: &'static str,

    /// Major version (breaking changes)
    pub major: u32,

    /// Minor version (features)
    pub minor: u32,

    /// Patch version (fixes)
    pub patch: u32,

    /// Transaction version (encoding changes)
    pub transaction_version: u32,

    /// State version (storage format changes)
    pub state_version: u32,
}

impl RuntimeVersion {
    /// Check if this version can upgrade from another
    pub fn can_upgrade_from(&self, other: &RuntimeVersion) -> bool {
        // Major version must match (no breaking changes without migration)
        if self.major != other.major {
            return false;
        }

        // Version must be newer
        (self.minor > other.minor) ||
        (self.minor == other.minor && self.patch > other.patch)
    }
}
```

### Runtime Registry

```rust
/// Runtime registry (on-chain)
pub struct RuntimeRegistry {
    /// Current active runtime
    current_runtime: RuntimeSpec,

    /// Pending runtime upgrade (waiting for activation)
    pending_upgrade: Option<RuntimeSpec>,

    /// Runtime history
    history: Vec<RuntimeSpec>,

    /// Governance parameters
    config: RuntimeGovernanceConfig,
}

#[derive(Debug, Clone)]
pub struct RuntimeGovernanceConfig {
    /// Minimum delay before activation (blocks)
    pub min_activation_delay: u64,

    /// Required FROST threshold (13/15 supermajority)
    pub threshold: ThresholdRequirement,

    /// Maximum runtime size (bytes)
    pub max_runtime_size: usize,
}

impl Default for RuntimeGovernanceConfig {
    fn default() -> Self {
        Self {
            min_activation_delay: 43_200,  // 24 hours
            threshold: ThresholdRequirement::Supermajority,
            max_runtime_size: 10_000_000,  // 10 MB
        }
    }
}

impl RuntimeRegistry {
    /// Propose runtime upgrade
    pub fn propose_upgrade(
        &mut self,
        bytecode: Vec<u8>,
        version: RuntimeVersion,
        activates_at: u64,
    ) -> Result<()> {
        // Validate size
        if bytecode.len() > self.config.max_runtime_size {
            bail!("Runtime too large: {} > {} bytes",
                bytecode.len(),
                self.config.max_runtime_size
            );
        }

        // Validate version
        if !version.can_upgrade_from(&self.current_runtime.version) {
            bail!("Invalid version upgrade");
        }

        // Validate activation delay
        let current_block = /* get current block */;
        if activates_at < current_block + self.config.min_activation_delay {
            bail!("Activation too soon");
        }

        // Encode as ZODA
        let (zoda_encoding, ligerito_proof) = self.encode_runtime(&bytecode)?;

        let spec = RuntimeSpec {
            version,
            bytecode,
            zoda_encoding,
            ligerito_proof,
            frost_signature: FrostSignature::default(),  // Will be added later
            activates_at,
        };

        self.pending_upgrade = Some(spec);

        tracing::info!(
            "Proposed runtime upgrade to v{}.{}.{} (activates at block {})",
            version.major,
            version.minor,
            version.patch,
            activates_at
        );

        Ok(())
    }

    /// Validators sign upgrade (FROST 13/15)
    pub fn sign_upgrade(&mut self, signature: FrostSignature) -> Result<()> {
        let spec = self.pending_upgrade
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("No pending upgrade"))?;

        // Verify threshold
        if signature.threshold != ThresholdRequirement::Supermajority {
            bail!("Invalid threshold: expected Supermajority (13/15)");
        }

        spec.frost_signature = signature;

        tracing::info!("Runtime upgrade signed by 13/15 validators");

        Ok(())
    }

    /// Apply upgrade if activation block reached
    pub fn try_apply_upgrade(&mut self, current_block: u64) -> Result<bool> {
        if let Some(ref spec) = self.pending_upgrade {
            if current_block >= spec.activates_at {
                // Verify FROST signature
                spec.frost_signature.verify_threshold(15)?;

                // Archive old runtime
                self.history.push(self.current_runtime.clone());

                // Activate new runtime
                self.current_runtime = spec.clone();
                self.pending_upgrade = None;

                tracing::info!(
                    "Activated runtime v{}.{}.{} at block {}",
                    spec.version.major,
                    spec.version.minor,
                    spec.version.patch,
                    current_block
                );

                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Encode runtime as ZODA
    fn encode_runtime(&self, bytecode: &[u8]) -> Result<(Vec<BinaryElem32>, LigeritoProof)> {
        // TODO: Actual ZODA encoding
        // For now, placeholder
        Ok((vec![], LigeritoProof { proof_data: vec![] }))
    }

    /// Get current runtime version
    pub fn current_version(&self) -> &RuntimeVersion {
        &self.current_runtime.version
    }

    /// Get runtime history
    pub fn history(&self) -> &[RuntimeSpec] {
        &self.history
    }
}
```

---

## Upgrade Types

### 1. **Minor Upgrade** (Non-Breaking)

**Example**: Add new RPC endpoint, optimize existing function

**Process**:
```
v1.2.0 → v1.3.0
├─> No state migration needed
├─> Backward compatible
└─> Activation: 24 hours
```

**Code**:
```rust
// Old runtime (v1.2.0)
fn transfer(from: Account, to: Account, amount: Balance) {
    // Basic transfer
}

// New runtime (v1.3.0)
fn transfer(from: Account, to: Account, amount: Balance) {
    // Optimized transfer (25% faster)
    // + New function: transfer_batch
}
```

### 2. **Major Upgrade** (Breaking)

**Example**: Change transaction format, add new pallet

**Process**:
```
v1.9.0 → v2.0.0
├─> State migration required
├─> Multi-phase upgrade
└─> Activation: 1 week
```

**Migration Steps**:
1. **Phase 1**: Publish migration plan
2. **Phase 2**: Run migration script (24-hour window)
3. **Phase 3**: Activate new runtime
4. **Phase 4**: Verify migration success

### 3. **Emergency Upgrade** (Hotfix)

**Example**: Fix critical bug, patch security vulnerability

**Process**:
```
v1.2.0 → v1.2.1
├─> Fast-track governance (6-hour delay)
├─> Requires 13/15 validators online
└─> Activation: 6 hours
```

---

## Light Client Impact

### Automatic Runtime Updates

**Traditional**:
```
Light Client
├─> Hardcoded protocol rules
├─> Must update app for protocol changes
└─> Can't verify if protocol changed
```

**Zeratul**:
```
Light Client
├─> Downloads RuntimeSpec from chain
├─> Verifies Ligerito proof
├─> Caches ZODA encoding
└─> Automatically uses new runtime!

No app update needed!
```

**Implementation**:
```rust
pub struct LightClient {
    /// Current runtime
    runtime: RuntimeSpec,

    /// Ligerito verifier
    verifier: LigeritoVerifier,
}

impl LightClient {
    /// Sync runtime from chain
    pub async fn sync_runtime(&mut self) -> Result<()> {
        // Fetch runtime from validator
        let spec = self.fetch_runtime_spec().await?;

        // Verify Ligerito proof
        if !self.verifier.verify(&spec.ligerito_proof)? {
            bail!("Invalid runtime proof");
        }

        // Verify FROST signature (13/15)
        spec.frost_signature.verify_threshold(15)?;

        // Update runtime
        self.runtime = spec;

        tracing::info!("Updated runtime to v{}.{}.{}",
            self.runtime.version.major,
            self.runtime.version.minor,
            self.runtime.version.patch
        );

        Ok(())
    }
}
```

---

## Governance Integration

### Proposal Types

**1. Runtime Upgrade Proposal**:
```rust
pub struct RuntimeUpgradeProposal {
    /// New bytecode
    pub bytecode: Vec<u8>,

    /// Version bump
    pub new_version: RuntimeVersion,

    /// Activation delay
    pub activation_delay: u64,

    /// Proposer
    pub proposer: AccountId,

    /// Deposit (1000 ZT)
    pub deposit: Balance,
}
```

**2. Emergency Upgrade** (Fast-track):
```rust
pub struct EmergencyUpgradeProposal {
    /// Patch bytecode
    pub bytecode: Vec<u8>,

    /// Version (patch only: v1.2.0 → v1.2.1)
    pub new_version: RuntimeVersion,

    /// Justification (security advisory, bug report)
    pub justification: String,

    /// Fast-track: 6-hour activation
    pub activation_delay: u64,  // 10,800 blocks
}
```

### Voting Process

```rust
/// Runtime upgrade voting
pub struct RuntimeUpgradeVote {
    /// Proposal ID
    pub proposal_id: u64,

    /// Voting period (7 days)
    pub voting_period: u64,

    /// Votes from validators
    pub validator_votes: BTreeMap<ValidatorIndex, bool>,

    /// Result
    pub result: Option<VoteResult>,
}

#[derive(Debug, Clone, Copy)]
pub enum VoteResult {
    /// Passed (13/15 validators)
    Approved,

    /// Failed (<13/15)
    Rejected,

    /// Expired (voting period ended)
    Expired,
}

impl RuntimeUpgradeVote {
    /// Check if proposal passes
    pub fn check_result(&self, validator_set: &ValidatorSet) -> VoteResult {
        let approvals = self.validator_votes.values().filter(|&&v| v).count();

        if approvals >= 13 {
            VoteResult::Approved
        } else if self.is_expired() {
            VoteResult::Expired
        } else {
            VoteResult::Rejected
        }
    }
}
```

---

## Migration Framework

### State Migration

For major upgrades that change storage format:

```rust
/// Migration trait
pub trait Migration {
    /// Migration version
    fn version(&self) -> (u32, u32, u32);

    /// Check if migration is needed
    fn needs_migration(&self, current_version: &RuntimeVersion) -> bool;

    /// Execute migration
    fn migrate(&self, state: &mut State) -> Result<()>;

    /// Verify migration succeeded
    fn verify(&self, state: &State) -> Result<()>;
}

/// Example: Add new field to Account
pub struct MigrationV2_0_0;

impl Migration for MigrationV2_0_0 {
    fn version(&self) -> (u32, u32, u32) {
        (2, 0, 0)
    }

    fn needs_migration(&self, current_version: &RuntimeVersion) -> bool {
        current_version.major < 2
    }

    fn migrate(&self, state: &mut State) -> Result<()> {
        // Migrate all accounts to add new "reputation" field
        for account in state.accounts_mut() {
            account.reputation = 0;  // Default value
        }

        tracing::info!("Migrated {} accounts to v2.0.0", state.account_count());

        Ok(())
    }

    fn verify(&self, state: &State) -> Result<()> {
        // Verify all accounts have reputation field
        for account in state.accounts() {
            if account.reputation == 0 {
                // Expected default
            }
        }

        Ok(())
    }
}
```

### Migration Execution

```rust
pub struct MigrationCoordinator {
    /// Registered migrations
    migrations: Vec<Box<dyn Migration>>,
}

impl MigrationCoordinator {
    /// Execute all pending migrations
    pub fn execute_migrations(
        &self,
        current_version: &RuntimeVersion,
        state: &mut State,
    ) -> Result<()> {
        let mut executed = Vec::new();

        for migration in &self.migrations {
            if migration.needs_migration(current_version) {
                tracing::info!(
                    "Executing migration to v{}.{}.{}",
                    migration.version().0,
                    migration.version().1,
                    migration.version().2
                );

                migration.migrate(state)?;
                migration.verify(state)?;

                executed.push(migration.version());
            }
        }

        if !executed.is_empty() {
            tracing::info!("Executed {} migrations", executed.len());
        }

        Ok(())
    }
}
```

---

## Comparison to Polkadot

### Similarities ✅

1. **Forkless upgrades**: No hard forks needed
2. **On-chain storage**: Runtime stored on-chain
3. **Governance-gated**: Requires supermajority approval
4. **Versioning**: Semantic versioning with compatibility checks
5. **Migration framework**: Support for state migrations

### Differences 🔄

| Feature | Polkadot | Zeratul |
|---------|----------|---------|
| **Runtime format** | Wasm bytecode | PolkaVM bytecode |
| **Commitment** | Hash | ZODA encoding |
| **Proof** | None | Ligerito proof |
| **Approval** | OpenGov (token voting) | FROST (13/15 validators) |
| **Light clients** | Trust Wasm hash | Verify Ligerito proof |
| **Execution** | Native or Wasm | PolkaVM only |

### Unique Advantages 🚀

**Zeratul's ZODA + PolkaVM approach**:
1. ✅ **Verifiable by light clients**: Ligerito proof
2. ✅ **Efficient**: PolkaVM faster than Wasm for certain operations
3. ✅ **Deterministic**: ZODA encoding is canonical
4. ✅ **Byzantine secure**: FROST 13/15 threshold

---

## Implementation Roadmap

### Phase 1: Basic Runtime Registry (Q1 2026)
- [x] Design runtime storage format
- [ ] Implement RuntimeSpec structure
- [ ] Add version checking
- [ ] Basic upgrade proposal

### Phase 2: FROST Integration (Q2 2026)
- [ ] 13/15 supermajority voting
- [ ] FROST signature on runtime
- [ ] Validator coordination
- [ ] Activation logic

### Phase 3: ZODA Encoding (Q2 2026)
- [ ] Encode PolkaVM as ZODA
- [ ] Generate Ligerito proofs
- [ ] Light client verification
- [ ] Caching layer

### Phase 4: Migration Framework (Q3 2026)
- [ ] Migration trait
- [ ] State migration tools
- [ ] Verification framework
- [ ] Rollback mechanism

### Phase 5: Production Hardening (Q4 2026)
- [ ] Emergency upgrades
- [ ] Comprehensive testing
- [ ] External audit
- [ ] First mainnet upgrade

---

## Security Considerations

### Upgrade Attacks

**1. Malicious Runtime**:
```
Attack: Validator submits backdoored runtime
Defense: 13/15 threshold + public review period (24 hours)
Result: ✅ Requires 13/15 validators to collude
```

**2. Premature Activation**:
```
Attack: Bypass activation delay
Defense: Hard-coded minimum delay (24 hours)
Result: ✅ Impossible to activate early
```

**3. Invalid ZODA Encoding**:
```
Attack: Submit runtime with invalid Ligerito proof
Defense: Proof verification required before activation
Result: ✅ Light clients reject invalid proofs
```

**4. Replay Attack**:
```
Attack: Re-submit old runtime
Defense: Version must be strictly increasing
Result: ✅ Cannot downgrade versions
```

---

## Conclusion

**Zeratul's runtime upgrade system combines the best of Polkadot with ZODA/FROST innovations**:

1. ✅ **Forkless upgrades** (like Polkadot)
2. ✅ **Verifiable by light clients** (unique to ZODA)
3. ✅ **Byzantine secure** (FROST 13/15)
4. ✅ **Zero downtime** (like Polkadot)

**This is revolutionary** because:
- No other chain has verifiable runtimes for light clients
- ZODA encoding provides both commitment and execution
- FROST threshold provides Byzantine security

**Next steps**: Implement Phase 1 (Basic Runtime Registry)

---

## References

- [Polkadot Runtime Upgrades](https://wiki.polkadot.network/docs/maintain-guides-how-to-upgrade)
- [Substrate Frame](https://docs.substrate.io/reference/frame-pallets/)
- [Runtime Versioning](https://docs.substrate.io/build/upgrade-the-runtime/)
- [ZODA Paper](https://angeris.github.io/papers/zoda.pdf)
