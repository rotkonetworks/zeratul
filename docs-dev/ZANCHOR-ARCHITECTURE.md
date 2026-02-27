# zanchor architecture

a trust-minimized bitcoin/zcash bridge for polkadot using threshold frost signatures with mandatory participation enforcement.

## design philosophy

1. **validators are signers** - collators who produce blocks must also sign custody transactions. no separation of concerns that lets one group do nothing while collecting rewards.

2. **on-chain signature aggregation** - partial signatures submitted as transactions, aggregated on-chain. creates immutable audit trail and enables slashing for non-participation.

3. **spv verification, not trust** - bitcoin/zcash light clients verify transaction inclusion. no need to trust relayers beyond data availability.

4. **economic finality through collateral** - signers lock collateral that can be slashed for misbehavior. recovery paths always exist.

## core pallets

### pallet-btc-relay (adapted from interbtc)

bitcoin spv light client. stores block headers, validates pow, provides merkle proof verification.

```rust
pub trait BtcRelay {
    fn initialize(header: BlockHeader, height: u32);
    fn store_block_header(header: BlockHeader);
    fn verify_transaction_inclusion(
        tx_id: H256Le,
        merkle_proof: PartialTransactionProof,
        confirmations: u32,
    ) -> Result<(), Error>;
}
```

key storage:
- `BlockHeaders: Map<H256Le, RichBlockHeader>` - indexed by block hash
- `BestBlock: H256Le` - current chain tip
- `BestBlockHeight: u32` - height of chain tip
- `ChainsIndex: Map<u32, Vec<H256Le>>` - forks by height

### pallet-zcash-light (existing)

zcash compact block verification using sapling/orchard proofs. similar to btc-relay but for zcash note commitment tree.

### pallet-frost-bridge (enhanced)

threshold signature management with mandatory participation.

```rust
pub trait FrostBridge {
    // registration
    fn register_signer(encryption_key: [u8; 32], stake: Balance);
    fn deregister_signer();

    // dkg ceremony
    fn submit_dkg_commitment(round: u8, commitment: [u8; 32]);
    fn submit_dkg_shares(to: Vec<u16>, encrypted_shares: Vec<u8>);
    fn finalize_dkg() -> Result<[u8; 32], Error>;  // returns group pubkey

    // signing
    fn request_signature(tx_data: Vec<u8>, deadline: BlockNumber);
    fn submit_partial_signature(request_id: u64, partial_sig: FrostSignature);
    fn aggregate_signatures(request_id: u64) -> Result<FrostSignature, Error>;

    // liveness
    fn submit_heartbeat(challenge_response: [u8; 64]);
    fn report_missing_signer(signer: AccountId, round_id: u64);
}
```

key storage:
- `Signers: Map<AccountId, SignerInfo>` - active signers
- `GroupPublicKey: [u8; 32]` - current custody key
- `SigningQueue: Map<u64, SigningRequest>` - pending signing requests
- `SignerParticipation: Map<AccountId, ParticipationStats>` - liveness tracking
- `LastHeartbeat: Map<AccountId, BlockNumber>` - heartbeat timestamps

### pallet-custody (new)

manages deposits and withdrawals with slashing.

```rust
#[derive(Encode, Decode, TypeInfo)]
pub struct DepositRequest {
    pub id: u64,
    pub depositor: AccountId,
    pub btc_address: BtcAddress,  // generated deposit address
    pub amount: Balance,
    pub btc_txid: Option<H256Le>,  // filled when confirmed
    pub status: DepositStatus,
}

#[derive(Encode, Decode, TypeInfo)]
pub struct WithdrawalRequest {
    pub id: u64,
    pub requester: AccountId,
    pub btc_dest: BtcAddress,
    pub amount: Balance,
    pub created_at: BlockNumber,
    pub deadline: BlockNumber,
    pub checkpoint_id: Option<u64>,
    pub status: WithdrawalStatus,
}

pub trait Custody {
    fn request_deposit() -> DepositRequest;
    fn confirm_deposit(request_id: u64, btc_txid: H256Le, merkle_proof: Proof);
    fn request_withdrawal(dest: BtcAddress, amount: Balance);
    fn execute_withdrawal(request_id: u64, signed_tx: Vec<u8>);
    fn slash_non_participant(signer: AccountId, round_id: u64);
    fn initiate_emergency_recovery(recovery_address: Vec<u8>);
}
```

### pallet-ibc (from composable)

ibc protocol for cosmos ecosystem interop. enables zBTC/zZEC transfers to osmosis, noble, etc.

## signing flow

```
1. withdrawal request arrives
   └── validated, added to pending queue

2. checkpoint creation (every N blocks or M pending withdrawals)
   └── pallet-frost-bridge::create_checkpoint()
   └── constructs bitcoin tx spending reserve utxo
   └── adds outputs: new reserve + all pending withdrawals

3. signing round begins
   └── signers have T blocks to submit partial sigs
   └── each signer calls submit_partial_signature()
   └── non-participants tracked in SignerParticipation

4. aggregation
   └── when threshold (t-of-n) partial sigs collected
   └── aggregate into final schnorr signature
   └── store complete signed tx

5. broadcast
   └── anyone can broadcast signed tx to bitcoin
   └── btc-relay confirms inclusion after N confirmations
   └── withdrawal marked complete, zBTC burned
```

## slashing conditions

```rust
pub enum SlashReason {
    // signing failures
    MissedSigningRound { consecutive: u32 },
    InvalidPartialSignature,
    DkgFailure,

    // liveness failures
    MissedHeartbeat { blocks: u32 },
    OfflineToLong { blocks: u32 },

    // byzantine behavior
    DoubleSign,
    EquivocationProof,
}

impl SlashAmount {
    fn calculate(reason: SlashReason, stake: Balance) -> Balance {
        match reason {
            MissedSigningRound { consecutive: 1..=3 } => 0,  // grace period
            MissedSigningRound { consecutive: 4..=10 } => stake * 1 / 1000,  // 0.1%
            MissedSigningRound { consecutive: 11..=20 } => stake * 1 / 100,  // 1%
            MissedSigningRound { consecutive: 21.. } => stake * 5 / 100,     // 5%

            InvalidPartialSignature => stake * 10 / 100,  // 10% - intentional?
            DkgFailure => stake * 1 / 100,  // 1% - might be network issue

            MissedHeartbeat { blocks } if blocks > 1000 => stake * 1 / 100,
            OfflineToLong { blocks } if blocks > 5000 => stake * 5 / 100,

            DoubleSign => stake,  // 100% - unforgivable
            EquivocationProof => stake,  // 100%

            _ => 0,
        }
    }
}
```

## emergency recovery

unlike nomic where same signers control emergency path:

```rust
pub enum RecoveryPath {
    // option 1: governance
    OpenGovReferendum {
        track: FellowshipTrack,
        recovery_address: BtcAddress,
    },

    // option 2: timelock to depositors
    // each deposit has recovery_address set at deposit time
    // after RECOVERY_PERIOD blocks without checkpoint, funds claimable
    TimelockRecovery {
        deposit_id: u64,
        recovery_address: BtcAddress,
        unlock_block: BlockNumber,
    },

    // option 3: trusted federation fallback
    // n-of-m multisig of known entities (auditors, foundation, etc)
    // only activates after prolonged failure
    FederationRecovery {
        threshold: u32,
        signers: Vec<PublicKey>,
        activation_delay: BlockNumber,
    },
}
```

## crate dependencies

from interbtc (need modernization to stable2509):
- `bitcoin` - btc primitives, merkle proofs, script parsing
- `btc-relay` - spv light client pallet

from composable-ibc:
- `pallet-ibc` - ibc protocol implementation
- `ics10-grandpa` - polkadot light client for cosmos chains
- `ics07-tendermint` - cosmos light client

existing in zanchor:
- `pallet-frost-bridge` - frost threshold signatures (needs liveness additions)
- `pallet-zcash-light` - zcash light client
- `pallet-osst-threshold` - oblivious secret sharing

new pallets needed:
- `pallet-custody` - deposit/withdrawal management
- `pallet-slashing` - economic penalties

## testing strategy

1. **unit tests** - each pallet in isolation with mock dependencies
2. **integration tests** - full runtime with zombienet
3. **chaos tests** - kill signers randomly, verify recovery
4. **long-running tests** - simulate weeks of operation, verify no leaks
5. **adversarial tests** - byzantine signer behavior, invalid proofs

## migration path

1. phase 1: btc-relay + frost-bridge with basic custody
2. phase 2: slashing and liveness enforcement
3. phase 3: ibc integration for cosmos ecosystem
4. phase 4: zcash support (same frost infrastructure)
5. phase 5: emergency recovery mechanisms

## lessons from nomic

1. ✗ validators earned rewards without signing → ✓ mandatory signing participation
2. ✗ emergency recovery used same signers → ✓ independent recovery paths
3. ✗ off-chain signing with no audit trail → ✓ on-chain partial sig submission
4. ✗ no heartbeat/liveness checks → ✓ periodic heartbeat requirement
5. ✗ no slashing for non-participation → ✓ progressive slashing schedule
