**ZYNC: Zero-knowledge sYNChronization for Zcash**

*A Ligerito-powered wallet sync protocol*

---

## Threat Model & Goals

```
Actors:
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│   Client    │     │   Server    │     │  Zcash Node │
│  (mobile)   │◄───►│  (ZYNC)     │◄───►│  (zcashd)   │
│             │     │             │     │             │
│ has: ivk    │     │ has: ivk    │     │ has: chain  │
│ wants: notes│     │ computes:   │     │             │
│             │     │ proofs      │     │             │
└─────────────┘     └─────────────┘     └─────────────┘

Trust assumptions (same as lightwalletd):
✓ Server knows viewing key (can see incoming notes)
✓ Server CANNOT spend (no ask/nsk)
✓ Server CANNOT forge sync proofs
✓ Server CANNOT hide notes from client
✓ Server CANNOT inject fake notes
✓ Client verifies everything cryptographically
```

---

## The Core Problem

```rust
// current lightwalletd: O(chain_length) trial decryptions
for block in blockchain {
    for tx in block.txs {
        for output in tx.shielded_outputs {
            if let Some(note) = try_decrypt(output, ivk) {
                wallet.add_note(note);  // 99.99% of attempts fail
            }
        }
    }
}
// 2M+ blocks × ~1000 outputs = billions of decryption attempts
// mobile: 10+ minutes on fast connection
```

---

## ZYNC Solution

```
                    ZYNC Architecture
                    
┌────────────────────────────────────────────────────────┐
│                     ZYNC Server                        │
│                                                        │
│  ┌──────────────┐    ┌──────────────┐                 │
│  │ Block Scanner │───►│Trace Builder │                 │
│  │ (per wallet)  │    │              │                 │
│  └──────────────┘    └──────┬───────┘                 │
│                             │                          │
│                             ▼                          │
│                    ┌──────────────┐                    │
│                    │  Ligerito    │                    │
│                    │  Prover      │                    │
│                    └──────┬───────┘                    │
│                           │                            │
│                           ▼                            │
│              ┌─────────────────────────┐              │
│              │     EpochProof          │              │
│              │  - state_before         │              │
│              │  - state_after          │              │
│              │  - discovered_notes     │              │
│              │  - ligerito_commitment  │              │
│              └─────────────────────────┘              │
└────────────────────────────────────────────────────────┘
                           │
                           │ ~500KB per epoch
                           ▼
┌────────────────────────────────────────────────────────┐
│                     ZYNC Client                        │
│                                                        │
│  ┌──────────────────────────────────────────────────┐ │
│  │              Verifier (<100ms)                    │ │
│  │                                                   │ │
│  │  1. Check epoch chain hashes                     │ │
│  │  2. Verify each Ligerito proof                   │ │
│  │  3. Confirm state transitions                    │ │
│  │  4. Index discovered notes                       │ │
│  └──────────────────────────────────────────────────┘ │
└────────────────────────────────────────────────────────┘
```

---

## State Machine

```rust
/// commitment to wallet state (32 bytes, fits in single hash)
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct WalletStateCommitment([u8; 32]);

/// full wallet state 
pub struct WalletState {
    /// sparse merkle root: nullifier -> bool (seen or not)
    pub nullifier_set_root: [u8; 32],
    
    /// sparse merkle root: note_commitment -> (note, spent_flag)
    pub owned_notes_root: [u8; 32],
    
    /// incremental merkle tree frontier (for witness generation)
    /// this is the same structure Orchard uses
    pub note_tree_frontier: Frontier<NOTE_TREE_DEPTH>,
    
    /// chain position
    pub block_height: u32,
    pub block_hash: [u8; 32],
}

impl WalletState {
    pub fn commit(&self) -> WalletStateCommitment {
        // domain-separated hash
        WalletStateCommitment(blake2b(
            b"ZYNC_state_v1",
            &[
                &self.nullifier_set_root,
                &self.owned_notes_root, 
                &self.note_tree_frontier.root(),
                &self.block_height.to_le_bytes(),
                &self.block_hash,
            ].concat()
        ))
    }
    
    /// genesis state for fresh wallet
    pub fn genesis() -> Self {
        Self {
            nullifier_set_root: EMPTY_SMT_ROOT,
            owned_notes_root: EMPTY_SMT_ROOT,
            note_tree_frontier: Frontier::empty(),
            block_height: ORCHARD_ACTIVATION_HEIGHT,
            block_hash: ORCHARD_ACTIVATION_HASH,
        }
    }
}
```

---

## State Transition Function

```rust
/// what the prover must prove correct execution of
pub fn transition(
    state: &WalletState,
    block: &CompactBlock,
    ivk: &IncomingViewingKey,
) -> (WalletState, Vec<DiscoveredNote>, Vec<SpentNote>) {
    let mut new_state = state.clone();
    let mut discovered = vec![];
    let mut spent = vec![];
    
    for tx in &block.transactions {
        // process outputs: trial decrypt, maybe discover note
        for (action_idx, action) in tx.orchard_actions.iter().enumerate() {
            // update global note commitment tree (always)
            new_state.note_tree_frontier.append(action.cmx);
            let position = new_state.note_tree_frontier.position();
            
            // trial decrypt
            if let Some(note) = try_decrypt_note(action, ivk) {
                discovered.push(DiscoveredNote {
                    note,
                    position,
                    block_height: block.height,
                    tx_idx: tx.index,
                });
                
                // add to owned notes SMT
                new_state.owned_notes_root = smt_insert(
                    new_state.owned_notes_root,
                    action.cmx,
                    NoteData { note, position, spent: false },
                );
            }
        }
        
        // process nullifiers: mark spent if ours
        for nf in &tx.orchard_nullifiers {
            // insert into global nullifier set
            new_state.nullifier_set_root = smt_insert(
                new_state.nullifier_set_root,
                nf,
                true,
            );
            
            // check if this spends one of our notes
            if let Some(note_cm) = find_note_by_nullifier(&state, nf, ivk) {
                spent.push(SpentNote {
                    nullifier: *nf,
                    note_commitment: note_cm,
                    block_height: block.height,
                });
                
                // mark spent in owned notes
                new_state.owned_notes_root = smt_update(
                    new_state.owned_notes_root,
                    note_cm,
                    |data| data.spent = true,
                );
            }
        }
    }
    
    new_state.block_height = block.height;
    new_state.block_hash = block.hash;
    
    (new_state, discovered, spent)
}
```

---

## Polynomial Encoding for Ligerito

```rust
/// how we encode the sync trace as multilinear polynomial
/// 
/// key insight: we're proving EXECUTION TRACE not arbitrary circuit
/// this is highly structured and encodes naturally

pub const EPOCH_SIZE: usize = 1024;        // blocks per epoch  
pub const MAX_ACTIONS_PER_BLOCK: usize = 512;
pub const FIELDS_PER_ACTION: usize = 8;

/// polynomial size: 2^{10 + 9 + 3} = 2^22 elements
/// ~16MB, proves in <1s on M1

pub struct SyncTrace {
    /// coefficients indexed by:
    /// [block_idx: 10 bits][action_idx: 9 bits][field: 3 bits]
    pub coefficients: Vec<BinaryElem32>,
}

/// fields per action (3 bits = 8 fields)
#[repr(u8)]
pub enum ActionField {
    CmxLow = 0,         // note commitment (low 128 bits)
    CmxHigh = 1,        // note commitment (high 128 bits)
    EncKeyLow = 2,      // ephemeral key (low)
    EncKeyHigh = 3,     // ephemeral key (high)
    DecryptSuccess = 4, // 1 if decryption succeeded, 0 otherwise
    NoteValue = 5,      // value if decrypted (or 0)
    NullifierMatch = 6, // 1 if nullifier matches owned note
    StateUpdate = 7,    // encoded SMT update
}

impl SyncTrace {
    pub fn from_blocks(
        blocks: &[CompactBlock],
        ivk: &IncomingViewingKey,
        initial_state: &WalletState,
    ) -> Self {
        let mut coeffs = vec![BinaryElem32::zero(); 1 << 22];
        
        for (block_idx, block) in blocks.iter().enumerate() {
            for (action_idx, action) in block.all_actions().enumerate() {
                let base_idx = (block_idx << 12) | (action_idx << 3);
                
                // encode action fields
                coeffs[base_idx | 0] = encode_low(action.cmx);
                coeffs[base_idx | 1] = encode_high(action.cmx);
                coeffs[base_idx | 2] = encode_low(action.epk);
                coeffs[base_idx | 3] = encode_high(action.epk);
                
                // trial decrypt result
                let decrypt_result = try_decrypt(action, ivk);
                coeffs[base_idx | 4] = BinaryElem32::from(decrypt_result.is_some() as u32);
                coeffs[base_idx | 5] = decrypt_result
                    .map(|n| BinaryElem32::from(n.value as u32))
                    .unwrap_or(BinaryElem32::zero());
                    
                // nullifier check (for action's nullifier field)
                let nf_match = check_nullifier_ownership(action.nf, ivk, initial_state);
                coeffs[base_idx | 6] = BinaryElem32::from(nf_match as u32);
                
                // state delta encoding
                coeffs[base_idx | 7] = encode_state_delta(/*...*/);
            }
        }
        
        Self { coefficients: coeffs }
    }
}
```

---

## Constraint System (what sumcheck verifies)

```rust
/// constraints proven via sumcheck over the trace polynomial
/// 
/// these are NOT R1CS gates - they're structured sumcheck claims
/// tailored to our specific computation

pub struct ZyncConstraints;

impl ZyncConstraints {
    /// constraint 1: block chain consistency
    /// each block's prev_hash must match hash of previous block
    pub fn block_chain_constraint(
        trace: &SyncTrace,
        r: &[BinaryElem128],  // sumcheck randomness
    ) -> BinaryElem128 {
        // Σ_i eq(r, i) * (trace.block_hash[i-1] - trace.prev_hash[i])
        // must equal 0
        sumcheck_eval(trace, r, |block_idx, _| {
            if block_idx == 0 { return BinaryElem128::zero(); }
            let prev_hash = trace.get_block_hash(block_idx - 1);
            let claimed_prev = trace.get_prev_hash(block_idx);
            prev_hash - claimed_prev
        })
    }
    
    /// constraint 2: decryption validity
    /// if decrypt_success = 1, the note must be validly decrypted
    /// (this is checked via a commitment to ivk inside the proof)
    pub fn decryption_constraint(
        trace: &SyncTrace,
        r: &[BinaryElem128],
        ivk_commitment: &[u8; 32],
    ) -> BinaryElem128 {
        // Σ_{i,j} eq(r, (i,j)) * decrypt_success[i,j] * 
        //         (expected_plaintext[i,j] - actual_plaintext[i,j])
        sumcheck_eval(trace, r, |block_idx, action_idx| {
            let success = trace.get_decrypt_success(block_idx, action_idx);
            if success.is_zero() { return BinaryElem128::zero(); }
            
            // verify decryption matches commitment
            let ciphertext = trace.get_ciphertext(block_idx, action_idx);
            let claimed_note = trace.get_decrypted_note(block_idx, action_idx);
            
            // check: decrypt(ciphertext, ivk) == claimed_note
            // ivk is committed, verifier checks commitment consistency
            verify_decryption_gadget(ciphertext, ivk_commitment, claimed_note)
        })
    }
    
    /// constraint 3: SMT update consistency  
    /// state transitions must follow merkle update rules
    pub fn state_update_constraint(
        trace: &SyncTrace,
        r: &[BinaryElem128],
        initial_state: &WalletStateCommitment,
        final_state: &WalletStateCommitment,
    ) -> BinaryElem128 {
        // verify merkle path updates are consistent
        // initial_state --[updates]--> final_state
        sumcheck_eval(trace, r, |block_idx, action_idx| {
            let update = trace.get_state_update(block_idx, action_idx);
            verify_smt_update_gadget(update)
        })
    }
    
    /// constraint 4: completeness (no notes hidden)
    /// for each action, either decrypt failed OR note was added
    pub fn completeness_constraint(
        trace: &SyncTrace,
        r: &[BinaryElem128],
    ) -> BinaryElem128 {
        sumcheck_eval(trace, r, |block_idx, action_idx| {
            let success = trace.get_decrypt_success(block_idx, action_idx);
            let added = trace.get_note_added(block_idx, action_idx);
            // must have: success == added (if decrypted, must be added)
            success - added
        })
    }
}
```

---

## Epoch Proof Structure

```rust
/// proof for one epoch (~1000 blocks, ~2.5 days of chain)
pub struct EpochProof {
    /// chain linkage
    pub epoch_number: u32,
    pub prev_epoch_hash: [u8; 32],  // hash(EpochProof_{n-1}) or genesis
    
    /// state transition
    pub state_before: WalletStateCommitment,
    pub state_after: WalletStateCommitment,
    
    /// discovered data (encrypted for privacy, client decrypts)
    pub discovered_notes_enc: Vec<EncryptedNoteRecord>,
    pub spent_nullifiers_enc: Vec<EncryptedNullifierRecord>,
    
    /// ivk commitment (verifier checks consistency)
    pub ivk_commitment: [u8; 32],
    
    /// the ligerito proof
    pub ligerito_proof: FinalizedLigeritoProof<BinaryElem32, BinaryElem128>,
    
    /// auxiliary sumcheck data
    pub sumcheck_auxiliary: SumcheckAuxiliary,
}

impl EpochProof {
    pub fn hash(&self) -> [u8; 32] {
        blake2b(b"ZYNC_epoch_v1", &[
            &self.epoch_number.to_le_bytes(),
            &self.prev_epoch_hash,
            &self.state_before.0,
            &self.state_after.0,
            &self.ligerito_proof.root_commitment(),
        ].concat())
    }
    
    pub fn verify(
        &self,
        ivk: &IncomingViewingKey,
        expected_prev_hash: &[u8; 32],
    ) -> Result<(), ZyncError> {
        // 1. check epoch chain linkage
        if self.prev_epoch_hash != *expected_prev_hash {
            return Err(ZyncError::ChainLinkage);
        }
        
        // 2. check ivk commitment matches our key
        if self.ivk_commitment != ivk.commit() {
            return Err(ZyncError::WrongViewingKey);
        }
        
        // 3. verify ligerito proof
        let verifier_config = hardcoded_config_22_verifier();
        if !ligerito::verify(&verifier_config, &self.ligerito_proof)? {
            return Err(ZyncError::InvalidProof);
        }
        
        // 4. verify sumcheck constraints bind to state transition
        self.verify_sumcheck_binding()?;
        
        Ok(())
    }
}
```

---

## Client/Server Protocol

```rust
// ============= SERVER SIDE =============

pub struct ZyncServer {
    node: ZcashNodeClient,
    wallets: DashMap<WalletId, RegisteredWallet>,
}

struct RegisteredWallet {
    ivk: IncomingViewingKey,
    ivk_commitment: [u8; 32],
    last_synced_epoch: u32,
    cached_state: WalletState,
}

impl ZyncServer {
    /// register wallet, return initial state
    pub async fn register(
        &self,
        ivk: IncomingViewingKey,
    ) -> Result<(WalletId, WalletState)> {
        let wallet_id = WalletId::random();
        let ivk_commitment = ivk.commit();
        
        self.wallets.insert(wallet_id, RegisteredWallet {
            ivk,
            ivk_commitment,
            last_synced_epoch: 0,
            cached_state: WalletState::genesis(),
        });
        
        Ok((wallet_id, WalletState::genesis()))
    }
    
    /// generate sync proof from client's last epoch to tip
    pub async fn sync(
        &self,
        wallet_id: WalletId,
        from_epoch: u32,
    ) -> Result<SyncResponse> {
        let wallet = self.wallets.get(&wallet_id)
            .ok_or(ZyncError::UnknownWallet)?;
        
        let tip_height = self.node.get_tip_height().await?;
        let tip_epoch = tip_height / EPOCH_SIZE as u32;
        
        let mut epoch_proofs = vec![];
        let mut state = wallet.cached_state.clone();
        
        for epoch in from_epoch..=tip_epoch {
            let start_height = epoch * EPOCH_SIZE as u32;
            let end_height = ((epoch + 1) * EPOCH_SIZE as u32).min(tip_height);
            
            let blocks = self.node
                .get_compact_blocks(start_height..end_height)
                .await?;
            
            // build trace
            let trace = SyncTrace::from_blocks(&blocks, &wallet.ivk, &state);
            
            // generate ligerito proof
            let config = hardcoded_config_22(PhantomData, PhantomData);
            let ligerito_proof = ligerito::prove(&config, &trace.coefficients)?;
            
            // compute state transition
            let (new_state, discovered, spent) = 
                execute_epoch(&blocks, &wallet.ivk, &state);
            
            epoch_proofs.push(EpochProof {
                epoch_number: epoch,
                prev_epoch_hash: epoch_proofs.last()
                    .map(|p| p.hash())
                    .unwrap_or(GENESIS_EPOCH_HASH),
                state_before: state.commit(),
                state_after: new_state.commit(),
                discovered_notes_enc: encrypt_notes(&discovered, &wallet.ivk),
                spent_nullifiers_enc: encrypt_nullifiers(&spent, &wallet.ivk),
                ivk_commitment: wallet.ivk_commitment,
                ligerito_proof,
                sumcheck_auxiliary: compute_auxiliary(&trace),
            });
            
            state = new_state;
        }
        
        Ok(SyncResponse {
            epoch_proofs,
            final_state: state,
        })
    }
}

// ============= CLIENT SIDE =============

pub struct ZyncClient {
    ivk: IncomingViewingKey,
    state: WalletState,
    epoch_chain_tip: [u8; 32],
    notes: BTreeMap<NoteCommitment, OwnedNote>,
}

impl ZyncClient {
    pub fn new(ivk: IncomingViewingKey) -> Self {
        Self {
            ivk,
            state: WalletState::genesis(),
            epoch_chain_tip: GENESIS_EPOCH_HASH,
            notes: BTreeMap::new(),
        }
    }
    
    /// apply sync update from server
    pub fn apply_sync(&mut self, response: SyncResponse) -> Result<SyncSummary> {
        let mut discovered_count = 0;
        let mut spent_count = 0;
        
        for proof in &response.epoch_proofs {
            // verify proof
            proof.verify(&self.ivk, &self.epoch_chain_tip)?;
            
            // verify state continuity
            if proof.state_before != self.state.commit() {
                return Err(ZyncError::StateMismatch);
            }
            
            // decrypt and index discovered notes
            for enc_note in &proof.discovered_notes_enc {
                let note = enc_note.decrypt(&self.ivk)?;
                self.notes.insert(note.commitment, OwnedNote {
                    note: note.clone(),
                    spent: false,
                });
                discovered_count += 1;
            }
            
            // mark spent notes
            for enc_nf in &proof.spent_nullifiers_enc {
                let nf = enc_nf.decrypt(&self.ivk)?;
                if let Some(owned) = self.notes.values_mut()
                    .find(|n| n.note.nullifier(&self.ivk) == nf) 
                {
                    owned.spent = true;
                    spent_count += 1;
                }
            }
            
            // update state
            self.state = response.final_state.clone();
            self.epoch_chain_tip = proof.hash();
        }
        
        Ok(SyncSummary {
            epochs_synced: response.epoch_proofs.len(),
            notes_discovered: discovered_count,
            notes_spent: spent_count,
            new_balance: self.balance(),
        })
    }
    
    pub fn balance(&self) -> Amount {
        self.notes.values()
            .filter(|n| !n.spent)
            .map(|n| n.note.value)
            .sum()
    }
    
    pub fn spendable_notes(&self) -> Vec<SpendableNote> {
        self.notes.values()
            .filter(|n| !n.spent)
            .map(|n| SpendableNote {
                note: n.note.clone(),
                witness: self.state.note_tree_frontier
                    .witness(n.note.position),
            })
            .collect()
    }
}
```

---

## Performance Analysis

```
EPOCH PARAMETERS:
- Blocks per epoch: 1024 (~2.5 days)
- Max actions/block: 512
- Trace polynomial: 2^22 elements
- Field: GF(2^32) -> GF(2^128)

PROVER (server):
┌────────────────────┬─────────────┐
│ Operation          │ Time        │
├────────────────────┼─────────────┤
│ Fetch blocks       │ ~500ms      │
│ Build trace        │ ~200ms      │
│ Ligerito prove     │ ~1.5s       │
│ Total per epoch    │ ~2.2s       │
└────────────────────┴─────────────┘

VERIFIER (client):
┌────────────────────┬─────────────┐
│ Operation          │ Time        │
├────────────────────┼─────────────┤
│ Ligerito verify    │ ~50ms       │
│ Sumcheck checks    │ ~20ms       │
│ State binding      │ ~10ms       │
│ Total per epoch    │ ~80ms       │
└────────────────────┴─────────────┘

PROOF SIZE:
┌────────────────────┬─────────────┐
│ Component          │ Size        │
├────────────────────┼─────────────┤
│ Ligerito proof     │ ~200KB      │
│ State commitments  │ 64B         │
│ Encrypted notes    │ ~1KB/note   │
│ Auxiliary data     │ ~50KB       │
│ Total (sparse)     │ ~300KB      │
└────────────────────┴─────────────┘

FULL CHAIN SYNC (4 years, ~600 epochs):
┌────────────────────┬─────────────┐
│ Metric             │ Value       │
├────────────────────┼─────────────┤
│ Total proof size   │ ~180MB      │
│ Verification time  │ ~48s        │
│ vs current scan    │ hours       │
└────────────────────┴─────────────┘

INCREMENTAL SYNC (1 day behind):
┌────────────────────┬─────────────┐
│ Metric             │ Value       │
├────────────────────┼─────────────┤
│ Epochs to sync     │ 1           │
│ Proof size         │ ~300KB      │
│ Verification time  │ ~80ms       │
└────────────────────┴─────────────┘
```

---

## Security Properties

```
SOUNDNESS:
- Ligerito: 100-bit security (148 queries, GF(2^128))
- Epoch chain: hash collision resistance (256-bit)
- Total: min(100, 128) = 100-bit security

COMPLETENESS:
- Honest server always produces valid proofs
- Constraint 4 ensures no notes hidden

ZERO-KNOWLEDGE:
- Trace polynomial hides ivk (committed, not revealed)
- Note data encrypted in proof
- Only reveals: number of notes, approximate timing

BINDING:
- State commitments are collision-resistant
- Cannot claim different states for same epoch

POST-QUANTUM:
- Ligerito: hash-based, post-quantum secure
- State commitments: hash-based, post-quantum secure
- Encryption: needs PQ upgrade (separate concern)
```

---

## Implementation Roadmap

```
WEEK 1: Core Structures
├── [ ] WalletState + commitment
├── [ ] SyncTrace polynomial encoding
├── [ ] EpochProof structure
├── [ ] Basic constraint definitions
└── [ ] Integration with your ligerito crate

WEEK 2: Prover + Server
├── [ ] Trace builder from compact blocks
├── [ ] Sumcheck constraint implementation
├── [ ] EpochProof generation
├── [ ] Server API (register, sync)
└── [ ] Mock lightwalletd integration

WEEK 3: Verifier + Demo
├── [ ] Client verification logic
├── [ ] CLI: zync-server, zync-client
├── [ ] Benchmark suite
├── [ ] Comparison vs lightwalletd
└── [ ] Documentation + submission

DELIVERABLES:
├── zync-core        # state, proofs, constraints
├── zync-server      # proof generation service  
├── zync-client      # verification + wallet
├── zync-cli         # demo binaries
└── benchmarks/      # performance comparison
```

---

## Open Design Questions

```
1. REORG HANDLING
   Option A: Checkpoint only at finalized blocks (10+ confirms)
   Option B: Include reorg proofs (proves old chain, then new)
   → Recommend A for simplicity

2. VIEWING KEY PRIVACY
   Current: Server has ivk (same as lightwalletd)
   Future: FMD integration (fuzzy message detection)
   → Out of scope for hackathon

3. MULTIPLE ADDRESSES
   Same ivk handles all diversified addresses
   Separate ivk = separate ZYNC registration
   → Simple model works

4. STORAGE OPTIMIZATION  
   Option A: Client stores all epoch proofs
   Option B: Server provides proofs on demand + checkpoint
   → B more practical for mobile

5. INCREMENTAL TREE WITNESSES
   Need merkle paths for spending
   Include frontier in state, witnesses on demand
   → Standard approach from librustzcash
```

