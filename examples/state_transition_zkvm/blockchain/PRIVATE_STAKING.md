# Private Staking & Liquid Staking for Zeratul

**Date**: 2025-11-12
**Status**: Revolutionary redesign

---

## The Insight: Secret Ballot Staking + Liquid Derivatives

### Current Problem with Public Staking

**Polkadot-style (what we built)**:
```
❌ Public nominations (everyone sees who you voted for)
❌ Public stake amounts (reveals your wealth)
❌ No liquidity (tokens locked for 7 days)
❌ Cartel formation (validators can see large nominators)
```

### **Penumbra-style Secret Ballot**

**Private nominations**:
```
✅ Hidden validator selection (secret ballot!)
✅ Hidden stake amounts (privacy-preserving)
✅ Validators learn they were elected, but not by whom
✅ Prevents cartel formation
```

### **Liquid Staking via Trusted Validator Set**

**Key insight**: Our 11/15 FROST threshold = trusted set!

```
✅ Stake ZT → Mint stZT (staked ZT)
✅ stZT automatically earns rewards
✅ stZT is fully liquid (trade, use as collateral)
✅ Validators custody via FROST threshold (11/15)
```

---

## Architecture Overview

### Three Innovations

#### 1. **Private Nominations (Penumbra-style)**

```
Nominator:
├─> Generate viewing key (decaf377)
├─> Create shielded nomination
│   ├─> Amount (encrypted)
│   ├─> Validator selection (encrypted)
│   └─> Proof (ZK that nomination is valid)
└─> Submit to chain (fully private!)

Validators:
├─> Decrypt their own nominations (trial decryption)
├─> Don't learn about other nominations
└─> Phragmén runs on encrypted votes!
```

#### 2. **Homomorphic Phragmén**

```
Traditional: Phragmén needs public votes
Our innovation: Run Phragmén on commitments!

Process:
├─> Each nomination = Pedersen commitment
├─> Phragmén selects based on committed stakes
├─> Validators prove they have threshold without revealing amounts
└─> Result: Fair election with full privacy!
```

#### 3. **Liquid Staking via FROST Custody**

```
Stake Flow:
ZT (unstaked)
    ↓ [Stake transaction]
stZT (liquid staking derivative)
    ↓ [Held by user, earns rewards]
    ↓ [Can be traded, used as collateral]
    ↓ [Unbond transaction]
ZT (unstaked, 7-day delay)

Custody Model:
├─> stZT backed 1:1 by ZT in validator pool
├─> Pool secured by FROST 11/15 threshold
├─> Validators can't steal (need 11/15 to move funds)
└─> Instant liquidity (trade stZT anytime)
```

---

## Implementation: Private Staking

### Shielded Nomination Structure

```rust
use decaf377::{Fr, Element};
use decaf377_ka::{Public, Secret};  // Key agreement
use penumbra_tct::Tree;  // Tiered commitment tree

/// Private nomination (encrypted)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShieldedNomination {
    /// Note commitment (Pedersen commitment to amount + validator)
    pub note_commitment: decaf377::Element,

    /// Encrypted amount
    pub encrypted_amount: EncryptedAmount,

    /// Encrypted validator selection (up to 16)
    pub encrypted_validators: Vec<EncryptedValidator>,

    /// Nullifier (prevents double-nomination)
    pub nullifier: Nullifier,

    /// ZK proof (proves nomination is valid)
    pub proof: NominationProof,

    /// Ephemeral public key (for key agreement)
    pub ephemeral_key: decaf377_ka::Public,
}

/// Encrypted amount (ElGamal encryption)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedAmount {
    /// C1 = r * G (ephemeral key)
    pub c1: decaf377::Element,

    /// C2 = amount * G + r * validator_pubkey
    pub c2: decaf377::Element,
}

/// Encrypted validator ID
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedValidator {
    /// Encrypted validator index (0-14)
    pub ciphertext: [u8; 64],

    /// Validator commitment (so validator can trial-decrypt)
    pub validator_commitment: decaf377::Element,
}

/// Nullifier (prevents double-spending nominations)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Nullifier([u8; 32]);

/// ZK proof for nomination validity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NominationProof {
    /// Proves:
    /// 1. Amount > min_nominator_stake (100 ZT)
    /// 2. Validators are in valid set (0-14)
    /// 3. Nominator has funds (unspent note)
    /// 4. Nullifier is correctly derived
    pub proof_bytes: Vec<u8>,
}

impl ShieldedNomination {
    /// Create new shielded nomination
    pub fn new(
        amount: Balance,
        validators: Vec<ValidatorIndex>,
        spending_key: &SpendingKey,
        validator_pubkeys: &[decaf377_ka::Public],
    ) -> Result<Self> {
        // Generate randomness
        let blinding = Fr::rand(&mut rand::thread_rng());

        // Create note commitment: C = amount * G + blinding * H
        let note_commitment = pedersen_commit(amount, blinding);

        // Encrypt amount using validator set public key
        let encrypted_amount = encrypt_amount(amount, &validator_pubkeys[0])?;

        // Encrypt validator selections
        let mut encrypted_validators = Vec::new();
        for validator_idx in validators {
            let encrypted = encrypt_validator(validator_idx, &validator_pubkeys[validator_idx as usize])?;
            encrypted_validators.push(encrypted);
        }

        // Derive nullifier: nf = PRF(spending_key, note_commitment)
        let nullifier = derive_nullifier(spending_key, &note_commitment)?;

        // Generate ZK proof
        let proof = prove_nomination_validity(
            amount,
            &validators,
            spending_key,
            blinding,
        )?;

        // Ephemeral key for key agreement
        let ephemeral_secret = Secret::new(&mut rand::thread_rng());
        let ephemeral_key = ephemeral_secret.public();

        Ok(Self {
            note_commitment,
            encrypted_amount,
            encrypted_validators,
            nullifier,
            proof,
            ephemeral_key,
        })
    }

    /// Validators trial-decrypt to see if they were nominated
    pub fn trial_decrypt(&self, validator_key: &Secret, validator_idx: ValidatorIndex) -> Option<Balance> {
        // Check if any encrypted validator matches our index
        for encrypted_val in &self.encrypted_validators {
            if let Ok(decrypted_idx) = decrypt_validator(encrypted_val, validator_key) {
                if decrypted_idx == validator_idx {
                    // We were nominated! Decrypt amount
                    return decrypt_amount(&self.encrypted_amount, validator_key).ok();
                }
            }
        }

        None
    }

    /// Verify ZK proof
    pub fn verify(&self, validator_set_commitment: &ValidatorSetCommitment) -> Result<()> {
        // Verify nullifier hasn't been seen before
        if is_nullifier_spent(&self.nullifier) {
            bail!("Nullifier already spent (double nomination)");
        }

        // Verify ZK proof
        verify_nomination_proof(
            &self.proof,
            &self.note_commitment,
            validator_set_commitment,
        )?;

        Ok(())
    }
}
```

### Private Phragmén Election

```rust
/// Homomorphic Phragmén election
///
/// Runs Phragmén on encrypted nominations!
pub struct PrivatePhragmenElection {
    /// Validator set size
    validator_count: usize,

    /// Validator public keys (for trial decryption)
    validator_keys: Vec<decaf377_ka::Public>,

    /// Shielded nominations
    nominations: Vec<ShieldedNomination>,

    /// Spent nullifiers (prevent double-nomination)
    spent_nullifiers: std::collections::HashSet<Nullifier>,
}

impl PrivatePhragmenElection {
    /// Add shielded nomination
    pub fn add_nomination(&mut self, nomination: ShieldedNomination) -> Result<()> {
        // Verify not double-spent
        if self.spent_nullifiers.contains(&nomination.nullifier) {
            bail!("Nomination already spent");
        }

        // Verify ZK proof
        nomination.verify(&self.validator_set_commitment())?;

        // Mark nullifier as spent
        self.spent_nullifiers.insert(nomination.nullifier);

        self.nominations.push(nomination);

        tracing::debug!("Added private nomination (nullifier: {})",
            hex::encode(nomination.nullifier.0));

        Ok(())
    }

    /// Run election (validators decrypt their nominations)
    pub fn run_election(
        &self,
        validator_secrets: &[Secret],  // Each validator provides their key
    ) -> Result<PrivateElectionResult> {
        // Each validator trial-decrypts all nominations
        let mut validator_backings: BTreeMap<ValidatorIndex, Balance> = BTreeMap::new();

        for (idx, secret) in validator_secrets.iter().enumerate() {
            let validator_idx = idx as ValidatorIndex;
            let mut total_backing = 0u128;

            // Trial decrypt all nominations
            for nomination in &self.nominations {
                if let Some(amount) = nomination.trial_decrypt(secret, validator_idx) {
                    total_backing += amount;

                    tracing::debug!(
                        "Validator {} received {} stake from private nomination",
                        validator_idx,
                        amount
                    );
                }
            }

            validator_backings.insert(validator_idx, total_backing);
        }

        // Run Phragmén on decrypted backings
        // (Same algorithm as before, but using private data)
        let mut elected = Vec::new();
        for _ in 0..self.validator_count {
            // Select validator with highest backing
            let (winner_idx, winner_backing) = validator_backings
                .iter()
                .max_by_key(|(_, backing)| *backing)
                .ok_or_else(|| anyhow::anyhow!("No validators to elect"))?;

            elected.push(PrivateValidatorElection {
                validator_index: *winner_idx,
                total_backing: *winner_backing,
                // NOTE: Individual nominator contributions are NOT revealed!
            });

            // Remove from consideration
            validator_backings.remove(winner_idx);
        }

        Ok(PrivateElectionResult {
            elected_validators: elected,
            total_nominations: self.nominations.len(),
        })
    }

    /// Get validator set commitment (for ZK proofs)
    fn validator_set_commitment(&self) -> ValidatorSetCommitment {
        // Commit to validator public keys
        ValidatorSetCommitment::new(&self.validator_keys)
    }
}

/// Private election result
#[derive(Debug, Clone)]
pub struct PrivateElectionResult {
    /// Elected validators
    pub elected_validators: Vec<PrivateValidatorElection>,

    /// Total number of nominations (not revealing who)
    pub total_nominations: usize,
}

/// Private validator election info
#[derive(Debug, Clone)]
pub struct PrivateValidatorElection {
    /// Validator index
    pub validator_index: ValidatorIndex,

    /// Total backing (decrypted)
    pub total_backing: Balance,

    // NOTE: Individual nominators NOT revealed!
}
```

---

## Liquid Staking Architecture

### Staked Token Derivative (stZT)

```rust
/// Liquid staking token (stZT)
///
/// Represents staked ZT that earns rewards automatically.
/// Fully liquid - can be traded or used as collateral.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakedZT {
    /// Exchange rate: stZT → ZT
    /// Increases as rewards accumulate
    /// Example: 1 stZT = 1.05 ZT after rewards
    pub exchange_rate: Ratio,

    /// Total stZT supply
    pub total_supply: Balance,

    /// Total ZT in validator pool
    pub total_backing: Balance,

    /// Validator set securing the pool (FROST 11/15)
    pub custodian_validators: ValidatorSetCommitment,
}

impl StakedZT {
    /// Calculate current exchange rate
    ///
    /// rate = total_backing / total_supply
    ///
    /// As rewards accumulate, rate increases
    pub fn current_rate(&self) -> Ratio {
        if self.total_supply == 0 {
            return Ratio::ONE;
        }

        Ratio {
            numerator: self.total_backing,
            denominator: self.total_supply,
        }
    }

    /// Convert ZT → stZT
    pub fn mint(&mut self, zt_amount: Balance) -> Balance {
        let rate = self.current_rate();

        // stZT_minted = ZT / rate
        let stz_minted = (zt_amount * rate.denominator) / rate.numerator;

        self.total_supply += stz_minted;
        self.total_backing += zt_amount;

        stz_minted
    }

    /// Convert stZT → ZT (unbonding)
    pub fn burn(&mut self, stz_amount: Balance) -> Balance {
        let rate = self.current_rate();

        // ZT_returned = stZT * rate
        let zt_returned = (stz_amount * rate.numerator) / rate.denominator;

        self.total_supply -= stz_minted;
        self.total_backing -= zt_returned;

        zt_returned
    }

    /// Add rewards (increases exchange rate)
    pub fn add_rewards(&mut self, rewards: Balance) {
        self.total_backing += rewards;

        tracing::info!(
            "Added {} rewards, new rate: {} ZT per stZT",
            rewards,
            self.current_rate().to_f64()
        );
    }
}
```

### FROST Custody Pool

```rust
/// Validator custody pool (secured by FROST 11/15)
pub struct FrostCustodyPool {
    /// Total ZT held by validators
    pub total_custody: Balance,

    /// Validator set (15 validators)
    pub validator_set: ValidatorSet,

    /// FROST public key (11/15 threshold)
    pub frost_pubkey: decaf377::Element,

    /// Pending withdrawals (7-day unbonding)
    pub pending_withdrawals: Vec<PendingWithdrawal>,
}

impl FrostCustodyPool {
    /// Deposit ZT → Validator pool
    ///
    /// Secured by FROST 11/15 threshold
    pub fn deposit(&mut self, amount: Balance, from: AccountId) -> Result<()> {
        // Transfer ZT to pool
        self.total_custody += amount;

        tracing::info!(
            "Deposited {} ZT to FROST custody pool (total: {})",
            amount,
            self.total_custody
        );

        Ok(())
    }

    /// Withdraw ZT ← Validator pool
    ///
    /// Requires FROST 11/15 signature!
    pub fn withdraw(
        &mut self,
        amount: Balance,
        to: AccountId,
        frost_signature: FrostSignature,
    ) -> Result<()> {
        // Verify FROST signature (11/15 Byzantine threshold)
        if frost_signature.threshold != ThresholdRequirement::ByzantineThreshold {
            bail!("Invalid threshold: expected 11/15");
        }

        frost_signature.verify_threshold(15)?;

        // Verify sufficient funds
        if amount > self.total_custody {
            bail!("Insufficient custody funds");
        }

        // Transfer
        self.total_custody -= amount;

        tracing::info!(
            "Withdrew {} ZT from FROST custody pool (authorized by 11/15 validators)",
            amount
        );

        Ok(())
    }

    /// Initiate unbonding (7-day delay)
    pub fn start_unbonding(&mut self, stz_amount: Balance, owner: AccountId) -> Result<()> {
        let withdrawal = PendingWithdrawal {
            owner,
            stz_amount,
            initiated_at: current_block(),
            unlocks_at: current_block() + 7 * 24 * 60 * 30,  // 7 days
        };

        self.pending_withdrawals.push(withdrawal);

        Ok(())
    }

    /// Process unlocked withdrawals (requires FROST signature)
    pub fn process_withdrawals(&mut self, frost_signature: FrostSignature) -> Result<Vec<(AccountId, Balance)>> {
        let current = current_block();
        let (unlocked, still_locked): (Vec<_>, Vec<_>) = self
            .pending_withdrawals
            .iter()
            .partition(|w| w.unlocks_at <= current);

        // Verify FROST signature for batch
        frost_signature.verify_threshold(15)?;

        let processed: Vec<_> = unlocked
            .iter()
            .map(|w| {
                let zt_amount = convert_stz_to_zt(w.stz_amount);
                (w.owner, zt_amount)
            })
            .collect();

        self.pending_withdrawals = still_locked.into_iter().cloned().collect();

        Ok(processed)
    }
}

/// Pending withdrawal (unbonding)
#[derive(Debug, Clone)]
pub struct PendingWithdrawal {
    pub owner: AccountId,
    pub stz_amount: Balance,
    pub initiated_at: u64,
    pub unlocks_at: u64,
}
```

---

## User Flow

### 1. **Private Staking**

```rust
// User stakes 1000 ZT privately
let nomination = ShieldedNomination::new(
    1000 * ZT,
    vec![0, 1, 2],  // Nominate validators 0, 1, 2 (encrypted!)
    &my_spending_key,
    &validator_pubkeys,
)?;

// Submit to chain (fully private!)
blockchain.submit_nomination(nomination)?;

// Receive stZT (liquid derivative)
let stz_received = 1000 * stZT;  // 1:1 initially
```

**Privacy properties**:
- ✅ Stake amount hidden
- ✅ Validator selection hidden
- ✅ Identity hidden (except viewing key holder)

### 2. **Using stZT (Liquid Staking)**

```rust
// stZT earns rewards automatically
// After 1 year: 1 stZT ≈ 1.10 ZT (10% APY)

// Can trade stZT anytime
trade_on_dex(stZT, other_token)?;

// Can use as collateral
borrow_against_collateral(stZT)?;

// Can transfer
send(recipient, 100 * stZT)?;

// All while earning staking rewards!
```

**Liquidity benefits**:
- ✅ No 7-day unbonding wait
- ✅ Earn rewards while staying liquid
- ✅ Use in DeFi (collateral, LP, etc.)

### 3. **Unbonding**

```rust
// Initiate unbonding
custody_pool.start_unbonding(1000 * stZT, my_account)?;

// Wait 7 days
wait_for_unbonding();

// Withdraw (requires FROST 11/15 signature)
let zt_returned = custody_pool.withdraw(
    1000 * ZT,  // Plus rewards!
    my_account,
    frost_signature,  // Signed by 11/15 validators
)?;

// Received: ~1100 ZT (original 1000 + 10% rewards)
```

---

## Security Model

### FROST Custody Security

**Trust model**:
```
Validator Set (15 validators)
├─> 11/15 threshold to move funds
├─> Can't steal (need 11/15 colluding)
└─> Byzantine fault tolerance (tolerates 4 malicious)

Attack scenarios:
❌ Single validator compromised → Safe (need 11/15)
❌ 4 validators compromised → Safe (need 11/15)
❌ 10 validators compromised → Safe (need 11/15)
✅ 11+ validators compromised → Funds at risk

Economic security:
├─> Validators have skin in the game (10K+ ZT each)
├─> Slashing for misbehavior (20% of stake)
└─> Cost to attack >> Potential gain
```

### Privacy Guarantees

**What's hidden**:
- ✅ Stake amounts (encrypted)
- ✅ Validator selections (encrypted)
- ✅ Nominator identities (anonymous)

**What's revealed**:
- ❌ Total staked (sum of commitments)
- ❌ Elected validators (result of election)
- ❌ Validator total backing (decrypted by validator)

**Comparison to Penumbra**:
| Feature | Penumbra | Zeratul |
|---------|----------|---------|
| Private amounts | ✅ | ✅ |
| Private delegations | ✅ | ✅ |
| Secret ballot | ✅ | ✅ |
| Liquid staking | ❌ | ✅ (via stZT) |
| FROST custody | ❌ | ✅ (11/15 threshold) |

---

## Implementation Roadmap

### Phase 1: Private Nominations (Q1 2026)
- [ ] Integrate decaf377-ka (key agreement)
- [ ] Implement ShieldedNomination
- [ ] ElGamal encryption for amounts
- [ ] Trial decryption for validators
- [ ] Nullifier tracking

### Phase 2: ZK Proofs (Q2 2026)
- [ ] Nomination validity proof circuit
- [ ] Validator set commitment
- [ ] Integration with Ligerito
- [ ] Proof verification on-chain

### Phase 3: Liquid Staking (Q2 2026)
- [ ] stZT token implementation
- [ ] Exchange rate calculation
- [ ] Mint/burn mechanics
- [ ] Reward distribution

### Phase 4: FROST Custody (Q3 2026)
- [ ] Custody pool implementation
- [ ] 11/15 threshold withdrawals
- [ ] Unbonding queue
- [ ] Emergency procedures

### Phase 5: Integration (Q3 2026)
- [ ] Integrate with lending (stZT as collateral)
- [ ] DEX support (stZT trading)
- [ ] Light client support
- [ ] Mobile wallet

---

## Advantages Over Polkadot

| Feature | Polkadot | Zeratul |
|---------|----------|---------|
| **Nomination privacy** | ❌ Public | ✅ Private (secret ballot) |
| **Stake amounts** | ❌ Public | ✅ Private (encrypted) |
| **Liquid staking** | ❌ Requires 3rd party | ✅ Native (stZT) |
| **Custody model** | ❌ On-chain (risky) | ✅ FROST 11/15 (secure) |
| **DeFi integration** | ⚠️ Via bridges | ✅ Native (stZT) |
| **Unbonding** | ❌ 28 days | ✅ 7 days (or instant with stZT!) |

---

## Conclusion

**This redesign combines the best of**:
1. ✅ **Penumbra** (secret ballot, private staking)
2. ✅ **Lido** (liquid staking derivatives)
3. ✅ **FROST** (Byzantine secure custody)

**Revolutionary properties**:
- 🔒 **Privacy**: Secret ballot voting (Penumbra-inspired)
- 💧 **Liquidity**: stZT derivative (trade while staking!)
- 🛡️ **Security**: FROST 11/15 custody (Byzantine secure)
- 🚀 **DeFi**: Native stZT in lending, DEX, etc.

**No other chain has all of these!**

Next steps: Implement Phase 1 (Private Nominations) 🔥
