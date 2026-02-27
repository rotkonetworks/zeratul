# zanchor ibc integration todo

## phase 1: integrate composable pallet-ibc

### add dependencies to zanchor workspace

```toml
# in crates/zanchor/Cargo.toml [workspace.dependencies]
pallet-ibc = { git = "https://github.com/ComposableFi/composable-ibc", default-features = false }
ibc = { git = "https://github.com/ComposableFi/composable-ibc", default-features = false }
ibc-primitives = { git = "https://github.com/ComposableFi/composable-ibc", default-features = false }
```

### required runtime configuration

from ~/rotko/composable-ibc/contracts/pallet-ibc/README.md:

```rust
impl pallet_ibc::Config for Runtime {
    type TimeProvider = Timestamp;
    type RuntimeEvent = RuntimeEvent;
    type NativeCurrency = Balances;
    type NativeAssetId = NativeAssetId;
    type AssetId = AssetId;
    const PALLET_PREFIX: &'static [u8] = b"ibc/";
    const LIGHT_CLIENT_PROTOCOL: pallet_ibc::LightClientProtocol =
        pallet_ibc::LightClientProtocol::Grandpa;
    type ExpectedBlockTime = ExpectedBlockTime;
    type Fungibles = Assets;
    type AccountIdConversion = ibc_primitives::IbcAccount;
    type IbcDenomToAssetIdConversion = AssetIdProcessor;
    type WeightInfo = ();
    type Router = Router;
    type MinimumConnectionDelay = MinimumConnectionDelay;
    type ParaId = parachain_info::Pallet<Runtime>;
    type RelayChain = RelayChainId;
    type AdminOrigin = EnsureRoot<AccountId>;
    type SentryOrigin = EnsureRoot<AccountId>;
    type SpamProtectionDeposit = SpamProtectionDeposit;
}
```

### light client requirements

for connecting to cosmos chains:
- ics07-tendermint light client (from composable-ibc)

for other parachains connecting to us:
- ics10-grandpa light client runs on cosmos side

### tasks

- [ ] add pallet-ibc to zanchor runtime
- [ ] implement AssetIdProcessor for zBTC/zZEC denom mapping
- [ ] implement ModuleRouter for routing ibc packets
- [ ] add ics20 transfer module for token transfers
- [ ] configure light client protocol (grandpa for polkadot finality)

## phase 2: frost-bridge liveness tracking

### storage additions needed

```rust
/// signer participation stats per epoch
#[pallet::storage]
pub type SignerParticipation<T: Config> = StorageMap<
    _,
    Blake2_128Concat,
    T::AccountId,
    ParticipationStats,
>;

#[derive(Encode, Decode, TypeInfo, MaxEncodedLen)]
pub struct ParticipationStats {
    pub signing_rounds_available: u32,
    pub signing_rounds_participated: u32,
    pub last_participation_block: u32,
    pub consecutive_misses: u32,
}

/// signer heartbeat timestamps
#[pallet::storage]
pub type LastHeartbeat<T: Config> = StorageMap<
    _,
    Blake2_128Concat,
    T::AccountId,
    u32,  // block number
>;

/// bridge operational state
#[pallet::storage]
pub type BridgeState<T: Config> = StorageValue<_, BridgeOperationalState, ValueQuery>;

#[derive(Encode, Decode, TypeInfo, MaxEncodedLen, Default)]
pub enum BridgeOperationalState {
    #[default]
    Active,
    CircuitBroken { reason: CircuitBreakReason, since: u32 },
    EmergencyRecovery { initiated_at: u32 },
}
```

### new calls needed

```rust
#[pallet::call]
impl<T: Config> Pallet<T> {
    /// submit signer heartbeat proving liveness
    #[pallet::weight(10_000)]
    pub fn submit_heartbeat(
        origin: OriginFor<T>,
        challenge_response: [u8; 64],  // signed challenge
    ) -> DispatchResult;

    /// report missing signer (anyone can call)
    #[pallet::weight(10_000)]
    pub fn report_missing_signer(
        origin: OriginFor<T>,
        signer: T::AccountId,
        signing_round_id: u64,
    ) -> DispatchResult;

    /// initiate emergency recovery (privileged)
    #[pallet::weight(100_000)]
    pub fn initiate_emergency_recovery(
        origin: OriginFor<T>,
        recovery_address: Vec<u8>,  // bitcoin/zcash address
    ) -> DispatchResult;
}
```

### hooks needed

```rust
#[pallet::hooks]
impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
    fn on_finalize(n: BlockNumberFor<T>) {
        // check for signing round timeouts
        Self::process_expired_signing_rounds(n);

        // check heartbeat liveness
        Self::check_signer_liveness(n);

        // update participation stats at epoch boundaries
        if Self::is_epoch_boundary(n) {
            Self::finalize_epoch_participation();
        }
    }
}
```

## phase 3: slashing integration

### connect to pallet-staking or custom slashing

options:
1. use substrate's pallet-staking slashing hooks
2. custom slash handler that interacts with collator selection
3. integrate with pallet-escrow-arbitration for economic penalties

### slash conditions

1. missing N consecutive signing rounds
2. submitting invalid partial signature
3. missing heartbeats for M blocks
4. dkg participation failure

## phase 4: relayer infrastructure

### hyperspace relayer

composable-ibc includes hyperspace relayer:
- ~/rotko/composable-ibc/hyperspace/

this is the off-chain component that:
- watches for ibc packets on both chains
- submits relay transactions
- handles light client updates

### tasks

- [ ] study hyperspace configuration for substrate<->cosmos
- [ ] set up hyperspace for zanchor<->osmosis testnet
- [ ] document relayer requirements
- [ ] create docker compose for easy deployment

## phase 5: testing

### integration tests

- [ ] ibc connection handshake substrate<->cosmos
- [ ] ics20 token transfer both directions
- [ ] frost signing with ibc withdrawal destination
- [ ] circuit breaker triggers correctly
- [ ] emergency recovery path works

### chaos tests

- [ ] kill 1/3 of signers, verify signing still works
- [ ] kill >1/3 signers, verify circuit breaker triggers
- [ ] simulate 2 week stall, verify emergency recovery
- [ ] validator set rotation during active signing

## references

- composable-ibc: ~/rotko/composable-ibc/
- hyperspace relayer: ~/rotko/composable-ibc/hyperspace/
- ibc spec: https://github.com/cosmos/ibc
- ics20: https://github.com/cosmos/ibc/tree/main/spec/app/ics-020-fungible-token-transfer
