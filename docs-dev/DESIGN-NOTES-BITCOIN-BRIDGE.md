# zanchor bitcoin bridge design notes

lessons learned from nomic failure analysis (2024-12-28)

## nomic failure mode

nomic has 60 validators running tendermint consensus with 85% voting power but:
- zero transactions in 35,000+ blocks
- no bitcoin checkpoint signatures being submitted
- ~24+ btc frozen on chain with no way to withdraw
- validators earn nom inflation without signing bitcoin txs

### root cause: no accountability for bitcoin operations

1. **no slashing for missing signatures** - validators get staking rewards regardless of whether they run the signer daemon
2. **emergency disbursal circular dependency** - emergency recovery requires the same validators to sign, if they won't sign checkpoints they won't sign emergency txs either
3. **no signing participation requirements** - staking rewards flow without any proof of bitcoin signer liveness

## zanchor design requirements

### 1. mandatory signing participation

```rust
// every epoch, track signing participation
pub struct SignerParticipation {
    pub validator: AccountId,
    pub checkpoints_available: u32,
    pub checkpoints_signed: u32,
    pub last_signature_block: BlockNumber,
}

// slash or skip rewards if participation < threshold
const MIN_SIGNING_PARTICIPATION: Percent = Percent::from_percent(90);
```

### 2. independent recovery mechanism

do NOT rely on the same signatories for emergency recovery:

options:
- **option a: dao-controlled recovery key** - a 3/5 multisig of trusted parties (auditors, foundation, etc) that can trigger emergency withdrawal if sigset fails
- **option b: timelock to original depositor** - after N blocks, funds automatically become claimable by original btc sender address
- **option c: polkadot governance fallback** - openGov referendum can authorize emergency recovery via xcm

### 3. proof of signer liveness

require validators to submit heartbeat proofs that their signer daemon is running:

```rust
#[pallet::call]
impl<T: Config> Pallet<T> {
    /// validator must call this every N blocks with signed challenge
    pub fn submit_signer_heartbeat(
        origin: OriginFor<T>,
        challenge: [u8; 32],
        signature: Signature,  // signed by their bitcoin signing key
    ) -> DispatchResult {
        // verify signature matches their registered xpub
        // update last_heartbeat timestamp
        // if missed > threshold, reduce rewards or jail
    }
}
```

### 4. progressive slashing for signing failures

```rust
// escalating penalties for missed signatures
fn calculate_signing_penalty(missed_consecutive: u32) -> Balance {
    match missed_consecutive {
        0..=5 => 0,  // grace period
        6..=10 => validator_stake * 0.001,  // 0.1%
        11..=20 => validator_stake * 0.01,  // 1%
        21..=50 => validator_stake * 0.05,  // 5%
        _ => validator_stake * 0.10,  // 10% - likely kick from set
    }
}
```

### 5. circuit breaker with automatic recovery

nomic has circuit breaker but it just stops everything. better design:

```rust
pub enum BridgeState {
    Active,
    CircuitBroken {
        reason: CircuitBreakReason,
        triggered_at: BlockNumber,
        auto_recovery_at: BlockNumber,  // try to resume after cooldown
    },
    EmergencyRecovery {
        initiated_by: RecoveryInitiator,  // dao/governance/timeout
        recovery_address: BitcoinAddress,
    },
}
```

## ibc integration via composable-ibc

use composable's pallet-ibc for cosmos ecosystem connectivity:

```
zanchor (substrate parachain)
    |
    +-- pallet-ibc (composable)
    |       |
    |       +-- ics10-grandpa (polkadot light client)
    |       +-- ics07-tendermint (cosmos light client)
    |
    +-- pallet-frost-bridge (our bitcoin threshold signing)
    |
    +-- xcm (polkadot/kusama ecosystem)
```

### ibc channels needed

1. **cosmos hub** - for atom collateral and ibc routing
2. **osmosis** - dex liquidity for zbtc
3. **noble** - usdc liquidity
4. **stride** - liquid staking integration

### key composable-ibc components to integrate

from `~/rotko/composable-ibc/`:

- `contracts/pallet-ibc/` - core ibc pallet for substrate
- `light-clients/ics10-grandpa/` - polkadot grandpa light client
- `light-clients/ics07-tendermint/` - tendermint light client
- `hyperspace/` - off-chain relayer (like hermes but for substrate)

## frost threshold signing improvements

current pallet-frost-bridge should add:

1. **dkg rotation ceremony** - when validator set changes, run distributed key generation
2. **signing round timeout** - if round doesn't complete in N blocks, slash non-participants
3. **backup signers** - standby validators ready to substitute if primary fails

```rust
pub struct SigningRound<T: Config> {
    pub checkpoint_id: CheckpointId,
    pub started_at: BlockNumber,
    pub timeout_at: BlockNumber,
    pub required_signers: BTreeSet<T::AccountId>,
    pub received_shares: BTreeMap<T::AccountId, SignatureShare>,
    pub backup_signers: Vec<T::AccountId>,  // activated if primary misses deadline
}
```

## on-chain verifiability

unlike nomic where bitcoin signer runs off-chain with no on-chain proof, zanchor should:

1. **submit signature shares on-chain** - creates immutable record of who participated
2. **verify aggregated signature on-chain** - before broadcasting, prove it's valid
3. **store checkpoint txid on-chain** - link to bitcoin tx for auditing

## testing requirements

1. **chaos testing** - randomly kill validator signer daemons, verify recovery works
2. **long-term stall test** - simulate 2 week signing outage, verify emergency recovery
3. **key rotation test** - validator set changes mid-checkpoint, verify continuity
4. **byzantine scenarios** - validators submit wrong shares, verify detection + slashing

## references

- nomic source: https://github.com/nomic-io/nomic
- composable-ibc: https://github.com/ComposableFi/composable-ibc
- frost: https://github.com/ZcashFoundation/frost
- ibc spec: https://github.com/cosmos/ibc
