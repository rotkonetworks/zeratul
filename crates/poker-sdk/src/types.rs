//! shared types for poker SDK
//!
//! mirrors ghettobox-primitives types for client use
//! updated for BFT-compliant security tiers and self-custody support

use parity_scale_codec::{Decode, Encode};

// ============================================================
// Security Tiers (BFT compliant: t > 2n/3)
// ============================================================

/// security tier for registrations
/// thresholds designed for proper BFT (t > 2n/3)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub enum SecurityTier {
    /// 2-of-3, any operators - NOT BFT, training mode only
    /// no real value at risk, just learning poker
    Training,
    /// 3-of-4 (75%) - slightly above BFT minimum
    /// low stakes, basic operator requirements
    Casual,
    /// 5-of-7 (71%) - proper BFT with 2 fault tolerance
    /// mid stakes, geo-distributed, TPM preferred
    Standard,
    /// 7-of-9 (78%) - BFT with 2 fault tolerance + established operators
    /// high stakes, multi-jurisdiction, TPM required
    Secure,
    /// 10-of-13 (77%) - maximum security, 3 fault tolerance
    /// no limits, HSM operators only
    Paranoid,
}

impl Default for SecurityTier {
    fn default() -> Self {
        Self::Training
    }
}

impl SecurityTier {
    /// threshold (t) for this tier
    pub fn threshold(&self) -> u32 {
        match self {
            Self::Training => 2,
            Self::Casual => 3,
            Self::Standard => 5,
            Self::Secure => 7,
            Self::Paranoid => 10,
        }
    }

    /// total operators (n) for this tier
    pub fn total_operators(&self) -> u32 {
        match self {
            Self::Training => 3,
            Self::Casual => 4,
            Self::Standard => 7,
            Self::Secure => 9,
            Self::Paranoid => 13,
        }
    }

    /// fault tolerance (how many can fail/be malicious)
    pub fn fault_tolerance(&self) -> u32 {
        self.total_operators() - self.threshold()
    }

    /// is this tier BFT secure? (t > 2n/3)
    pub fn is_bft(&self) -> bool {
        // t > 2n/3 means 3t > 2n
        3 * self.threshold() > 2 * self.total_operators()
    }

    /// allowed PIN guesses
    pub fn allowed_guesses(&self) -> u32 {
        match self {
            Self::Training => 10, // lenient for learning
            Self::Casual => 5,
            Self::Standard => 3,
            Self::Secure => 2,
            Self::Paranoid => 1,
        }
    }

    /// minimum ligerito polynomial degree (PoW difficulty)
    pub fn min_polynomial_degree(&self) -> u32 {
        match self {
            Self::Training => 100, // very easy
            Self::Casual => 1_000,
            Self::Standard => 5_000,
            Self::Secure => 20_000,
            Self::Paranoid => 50_000,
        }
    }

    /// recovery timelock in blocks (0 = instant)
    pub fn recovery_timelock(&self) -> u32 {
        match self {
            Self::Training => 0,
            Self::Casual => 0,
            Self::Standard => 0,
            Self::Secure => 14_400,  // ~24h at 6s blocks
            Self::Paranoid => 43_200, // ~72h at 6s blocks
        }
    }

    /// requires TPM attestation from operators
    pub fn requires_tpm(&self) -> bool {
        matches!(self, Self::Secure | Self::Paranoid)
    }

    /// requires HSM from operators
    pub fn requires_hsm(&self) -> bool {
        matches!(self, Self::Paranoid)
    }

    /// allows real money play
    pub fn allows_real_stakes(&self) -> bool {
        !matches!(self, Self::Training)
    }

    /// maximum stake per table in planck (0 = unlimited)
    pub fn max_stake_per_table(&self) -> u128 {
        match self {
            Self::Training => 0, // no real money
            Self::Casual => 10_000_000_000_000, // 10 KSM
            Self::Standard => 0, // unlimited
            Self::Secure => 0,
            Self::Paranoid => 0,
        }
    }
}

// ============================================================
// Custody Modes
// ============================================================

/// signature scheme for account authentication
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub enum SignatureScheme {
    /// substrate default (schnorr-like)
    Sr25519,
    /// hardware wallet compatible (ledger, etc)
    Ed25519,
    /// ethereum compatible (for bridge users)
    Ecdsa,
}

impl Default for SignatureScheme {
    fn default() -> Self {
        Self::Sr25519
    }
}

/// account custody mode - how keys are managed
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub enum CustodyMode {
    /// ghettobox managed - email+PIN, keys stored with operators
    Managed {
        tier: SecurityTier,
        shard_id: [u8; 32],
    },
    /// self-custody - user controls keys directly
    SelfCustody {
        pubkey: [u8; 32],
        scheme: SignatureScheme,
    },
    /// hybrid - self-custody for signing, managed for history/viewing keys
    Hybrid {
        signing_pubkey: [u8; 32],
        scheme: SignatureScheme,
        viewing_shard: [u8; 32],
        tier: SecurityTier,
    },
}

impl CustodyMode {
    /// is this self-custody (user controls signing key)?
    pub fn is_self_custody(&self) -> bool {
        matches!(self, Self::SelfCustody { .. } | Self::Hybrid { .. })
    }

    /// get the security tier if managed/hybrid
    pub fn tier(&self) -> Option<SecurityTier> {
        match self {
            Self::Managed { tier, .. } => Some(*tier),
            Self::SelfCustody { .. } => None,
            Self::Hybrid { tier, .. } => Some(*tier),
        }
    }
}

// ============================================================
// Signer Abstraction
// ============================================================

/// signer trait for signing transactions
/// implemented by managed (OPRF), self-custody (wallet), or hybrid modes
pub trait Signer {
    /// sign a message
    fn sign(&self, message: &[u8]) -> Result<[u8; 64], SignerError>;

    /// get the public key
    fn public_key(&self) -> [u8; 32];

    /// get the signature scheme
    fn scheme(&self) -> SignatureScheme;

    /// is this a hardware wallet?
    fn is_hardware(&self) -> bool {
        false
    }
}

/// signer errors
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignerError {
    /// signing failed
    SigningFailed,
    /// hardware wallet disconnected
    DeviceDisconnected,
    /// user rejected signing
    UserRejected,
    /// key not available (OPRF reconstruction failed)
    KeyNotAvailable,
    /// timeout waiting for signature
    Timeout,
}

/// managed account signer (placeholder - actual impl uses OPRF)
pub struct ManagedSigner {
    /// reconstructed signing key (temporary, cleared after use)
    signing_key: [u8; 32],
    /// public key
    pub public_key: [u8; 32],
}

impl ManagedSigner {
    /// create from reconstructed key (from OPRF)
    pub fn from_reconstructed(signing_key: [u8; 32]) -> Self {
        let public_key = *blake3::hash(&signing_key).as_bytes();
        Self {
            signing_key,
            public_key,
        }
    }
}

impl Signer for ManagedSigner {
    fn sign(&self, message: &[u8]) -> Result<[u8; 64], SignerError> {
        // simplified signing for now - real impl uses sr25519
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"poker.sign.v1");
        hasher.update(&self.signing_key);
        hasher.update(message);
        let sig_hash = hasher.finalize();

        let mut signature = [0u8; 64];
        signature[..32].copy_from_slice(sig_hash.as_bytes());

        let msg_hash = blake3::hash(message);
        signature[32..].copy_from_slice(msg_hash.as_bytes());

        Ok(signature)
    }

    fn public_key(&self) -> [u8; 32] {
        self.public_key
    }

    fn scheme(&self) -> SignatureScheme {
        SignatureScheme::Sr25519
    }
}

/// self-custody signer (local key)
pub struct LocalSigner {
    secret_key: [u8; 32],
    pub public_key: [u8; 32],
    pub scheme: SignatureScheme,
}

impl LocalSigner {
    /// create from secret key
    pub fn from_secret(secret_key: [u8; 32], scheme: SignatureScheme) -> Self {
        let public_key = *blake3::hash(&secret_key).as_bytes();
        Self {
            secret_key,
            public_key,
            scheme,
        }
    }
}

impl Signer for LocalSigner {
    fn sign(&self, message: &[u8]) -> Result<[u8; 64], SignerError> {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"poker.sign.v1");
        hasher.update(&self.secret_key);
        hasher.update(message);
        let sig_hash = hasher.finalize();

        let mut signature = [0u8; 64];
        signature[..32].copy_from_slice(sig_hash.as_bytes());

        let msg_hash = blake3::hash(message);
        signature[32..].copy_from_slice(msg_hash.as_bytes());

        Ok(signature)
    }

    fn public_key(&self) -> [u8; 32] {
        self.public_key
    }

    fn scheme(&self) -> SignatureScheme {
        self.scheme
    }
}

// ============================================================
// Operator Selection
// ============================================================

/// operator selection preference
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub enum OperatorPreference {
    /// lowest premium operators
    LowestCost,
    /// highest uptime operators
    HighestUptime,
    /// maximize geographic diversity
    GeoDiverse,
    /// minimize latency
    LowLatency,
    /// maximum reputation
    HighestReputation,
    /// balanced (weighted combination)
    Balanced,
}

impl Default for OperatorPreference {
    fn default() -> Self {
        Self::Balanced
    }
}

/// geographic region
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
#[repr(u8)]
pub enum GeoRegion {
    NorthAmerica = 0,
    SouthAmerica = 1,
    WesternEurope = 2,
    EasternEurope = 3,
    AsiaPacific = 4,
    MiddleEast = 5,
    Africa = 6,
    Unknown = 255,
}

impl GeoRegion {
    /// map ISO 3166-1 numeric country code to region
    pub fn from_jurisdiction(code: u16) -> Self {
        match code {
            840 | 124 | 484 => Self::NorthAmerica,
            76 | 32 | 152 | 604 | 170 => Self::SouthAmerica,
            826 | 276 | 250 | 380 | 528 | 756 | 40 | 56 | 724 | 620 | 372 => Self::WesternEurope,
            616 | 203 | 348 | 642 | 100 | 804 => Self::EasternEurope,
            392 | 410 | 702 | 36 | 554 | 158 | 344 | 156 => Self::AsiaPacific,
            784 | 682 | 376 | 792 => Self::MiddleEast,
            710 | 818 | 566 | 404 => Self::Africa,
            _ => Self::Unknown,
        }
    }
}

/// operator capabilities
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode, Default)]
pub struct OperatorCapabilities {
    pub has_tpm: bool,
    pub has_hsm: bool,
    pub uptime_percent: u8,
    pub jurisdiction: u16,
}

impl OperatorCapabilities {
    /// check if operator meets tier requirements
    pub fn meets_tier(&self, tier: SecurityTier) -> bool {
        if tier.requires_hsm() && !self.has_hsm {
            return false;
        }
        if tier.requires_tpm() && !self.has_tpm && !self.has_hsm {
            return false;
        }
        true
    }

    /// get geographic region
    pub fn region(&self) -> GeoRegion {
        GeoRegion::from_jurisdiction(self.jurisdiction)
    }
}

/// operator info for client-side selection
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperatorInfo {
    pub id: u64,
    pub encryption_pubkey: [u8; 32],
    pub account: [u8; 32],
    pub reputation: u32,       // 0-1000000
    pub premium_bps: u16,      // basis points
    pub capabilities: OperatorCapabilities,
}

impl OperatorInfo {
    /// check if eligible for tier
    pub fn eligible_for_tier(&self, tier: SecurityTier) -> bool {
        self.capabilities.meets_tier(tier)
    }
}

/// selection constraints
#[derive(Clone, Debug, Default)]
pub struct SelectionConstraints {
    pub tier: SecurityTier,
    pub max_premium_bps: u16,
    pub min_uptime: u8,
    pub required_regions: Vec<GeoRegion>,
    pub excluded_operators: Vec<u64>,
    pub preferred_regions: Vec<GeoRegion>,
}

impl SelectionConstraints {
    pub fn for_tier(tier: SecurityTier) -> Self {
        Self {
            tier,
            ..Default::default()
        }
    }

    /// check if operator meets constraints
    pub fn operator_eligible(&self, op: &OperatorInfo) -> bool {
        if !op.eligible_for_tier(self.tier) {
            return false;
        }
        if self.max_premium_bps > 0 && op.premium_bps > self.max_premium_bps {
            return false;
        }
        if op.capabilities.uptime_percent < self.min_uptime {
            return false;
        }
        if !self.required_regions.is_empty() {
            if !self.required_regions.contains(&op.capabilities.region()) {
                return false;
            }
        }
        if self.excluded_operators.contains(&op.id) {
            return false;
        }
        true
    }
}

/// selection weights for balanced preference
#[derive(Clone, Copy, Debug)]
pub struct SelectionWeights {
    pub uptime: u8,
    pub reputation: u8,
    pub cost: u8,
    pub diversity: u8,
    pub latency: u8,
}

impl Default for SelectionWeights {
    fn default() -> Self {
        Self {
            uptime: 25,
            reputation: 30,
            cost: 20,
            diversity: 15,
            latency: 10,
        }
    }
}

impl SelectionWeights {
    /// compute weighted score for operator
    pub fn score(&self, op: &OperatorInfo, user_region: Option<GeoRegion>, existing_regions: &[GeoRegion]) -> u64 {
        let mut score: u64 = 0;

        // uptime score
        score += (self.uptime as u64) * (op.capabilities.uptime_percent as u64) * 100;

        // reputation score
        score += (self.reputation as u64) * (op.reputation as u64 / 100);

        // cost score (inverted)
        let cost_score = 10000u64.saturating_sub(op.premium_bps as u64);
        score += (self.cost as u64) * cost_score;

        // diversity bonus
        let region = op.capabilities.region();
        if !existing_regions.contains(&region) {
            score += (self.diversity as u64) * 10000;
        }

        // latency bonus
        if let Some(user_reg) = user_region {
            if region == user_reg {
                score += (self.latency as u64) * 10000;
            }
        }

        score
    }
}

/// select operators for a tier
pub fn select_operators(
    operators: &[OperatorInfo],
    constraints: &SelectionConstraints,
    preference: OperatorPreference,
    user_region: Option<GeoRegion>,
    count: usize,
) -> Vec<u64> {
    let mut eligible: Vec<_> = operators
        .iter()
        .filter(|op| constraints.operator_eligible(op))
        .collect();

    if eligible.len() < count {
        return vec![];
    }

    let weights = SelectionWeights::default();
    let mut selected = Vec::with_capacity(count);
    let mut selected_regions = Vec::new();

    for _ in 0..count {
        // score remaining operators
        let mut best_idx = 0;
        let mut best_score = 0u64;

        for (idx, op) in eligible.iter().enumerate() {
            let score = match preference {
                OperatorPreference::LowestCost => 10000u64.saturating_sub(op.premium_bps as u64),
                OperatorPreference::HighestUptime => op.capabilities.uptime_percent as u64 * 100,
                OperatorPreference::HighestReputation => op.reputation as u64,
                OperatorPreference::GeoDiverse => {
                    if !selected_regions.contains(&op.capabilities.region()) {
                        10000
                    } else {
                        1000
                    }
                }
                OperatorPreference::LowLatency => {
                    if user_region == Some(op.capabilities.region()) {
                        10000
                    } else {
                        1000
                    }
                }
                OperatorPreference::Balanced => weights.score(op, user_region, &selected_regions),
            };

            if score > best_score {
                best_score = score;
                best_idx = idx;
            }
        }

        let chosen = eligible.remove(best_idx);
        selected_regions.push(chosen.capabilities.region());
        selected.push(chosen.id);
    }

    selected
}

// ============================================================
// Channel and Game Types
// ============================================================

/// channel state for state channel protocol
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub enum ChannelState {
    Pending,
    Active,
    Closing,
    Disputed,
    Settled,
}

impl Default for ChannelState {
    fn default() -> Self {
        Self::Pending
    }
}

/// poker action in a hand
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub enum PokerAction {
    Check,
    Fold,
    Call(u128),
    Raise(u128),
    AllIn(u128),
}

/// betting round
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub enum BettingRound {
    Preflop,
    Flop,
    Turn,
    River,
    Showdown,
}

/// hand result for recording
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct HandResult {
    pub hand_number: u64,
    pub pot_distribution: Vec<u128>,
    pub winning_hand: Option<Vec<u8>>,
}

/// participant info for channel
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct Participant {
    pub address: [u8; 32],
    pub deposit: u128,
    pub balance: u128,
    pub seat: u8,
}

/// channel configuration
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct ChannelConfig {
    pub min_deposit: u128,
    pub big_blind: u128,
    pub small_blind: u128,
    pub ante: u128,
    pub security_tier: SecurityTier,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            min_deposit: 1_000_000_000_000, // 1 KSM
            big_blind: 10_000_000_000,      // 0.01 KSM
            small_blind: 5_000_000_000,     // 0.005 KSM
            ante: 0,
            security_tier: SecurityTier::Training,
        }
    }
}

/// signed state update for p2p exchange
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct SignedStateUpdate {
    pub channel_id: [u8; 32],
    pub nonce: u64,
    pub state_hash: [u8; 32],
    pub signatures: Vec<[u8; 64]>,
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_security_tier_bft() {
        // training is NOT BFT (by design)
        assert!(!SecurityTier::Training.is_bft());

        // all other tiers are BFT
        assert!(SecurityTier::Casual.is_bft());
        assert!(SecurityTier::Standard.is_bft());
        assert!(SecurityTier::Secure.is_bft());
        assert!(SecurityTier::Paranoid.is_bft());
    }

    #[test]
    fn test_security_tier_thresholds() {
        // verify BFT thresholds: 3t > 2n
        for tier in [
            SecurityTier::Casual,
            SecurityTier::Standard,
            SecurityTier::Secure,
            SecurityTier::Paranoid,
        ] {
            let t = tier.threshold();
            let n = tier.total_operators();
            assert!(
                3 * t > 2 * n,
                "{:?}: 3*{} = {} should be > 2*{} = {}",
                tier,
                t,
                3 * t,
                n,
                2 * n
            );
        }
    }

    #[test]
    fn test_operator_selection() {
        let operators = vec![
            OperatorInfo {
                id: 1,
                encryption_pubkey: [0u8; 32],
                account: [0u8; 32],
                reputation: 900_000,
                premium_bps: 100,
                capabilities: OperatorCapabilities {
                    has_tpm: true,
                    has_hsm: false,
                    uptime_percent: 99,
                    jurisdiction: 840, // US
                },
            },
            OperatorInfo {
                id: 2,
                encryption_pubkey: [0u8; 32],
                account: [0u8; 32],
                reputation: 800_000,
                premium_bps: 50,
                capabilities: OperatorCapabilities {
                    has_tpm: true,
                    has_hsm: false,
                    uptime_percent: 98,
                    jurisdiction: 276, // DE
                },
            },
            OperatorInfo {
                id: 3,
                encryption_pubkey: [0u8; 32],
                account: [0u8; 32],
                reputation: 700_000,
                premium_bps: 200,
                capabilities: OperatorCapabilities {
                    has_tpm: false,
                    has_hsm: false,
                    uptime_percent: 95,
                    jurisdiction: 392, // JP
                },
            },
        ];

        let constraints = SelectionConstraints::for_tier(SecurityTier::Training);
        let selected = select_operators(
            &operators,
            &constraints,
            OperatorPreference::Balanced,
            Some(GeoRegion::NorthAmerica),
            3,
        );

        assert_eq!(selected.len(), 3);
    }

    #[test]
    fn test_custody_mode() {
        let managed = CustodyMode::Managed {
            tier: SecurityTier::Casual,
            shard_id: [0u8; 32],
        };
        assert!(!managed.is_self_custody());
        assert_eq!(managed.tier(), Some(SecurityTier::Casual));

        let self_custody = CustodyMode::SelfCustody {
            pubkey: [0u8; 32],
            scheme: SignatureScheme::Ed25519,
        };
        assert!(self_custody.is_self_custody());
        assert_eq!(self_custody.tier(), None);

        let hybrid = CustodyMode::Hybrid {
            signing_pubkey: [0u8; 32],
            scheme: SignatureScheme::Ed25519,
            viewing_shard: [0u8; 32],
            tier: SecurityTier::Standard,
        };
        assert!(hybrid.is_self_custody());
        assert_eq!(hybrid.tier(), Some(SecurityTier::Standard));
    }

    #[test]
    fn test_signer() {
        let signer = LocalSigner::from_secret([0x42u8; 32], SignatureScheme::Sr25519);
        let message = b"test message";
        let sig = signer.sign(message).unwrap();
        assert_eq!(sig.len(), 64);
    }
}
