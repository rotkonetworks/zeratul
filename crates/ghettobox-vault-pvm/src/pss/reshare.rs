//! reshare protocol for provider rotation
//!
//! wraps osst::reshare with HTTP-friendly serialization and provider lifecycle:
//! - dealer: old provider generates subshares for new providers
//! - aggregator: new provider collects and combines subshares
//! - on-chain state: tracks reshare progress and commitments

use curve25519_dalek::{
    ristretto::{CompressedRistretto, RistrettoPoint},
    scalar::Scalar,
};
use osst::reshare::{Aggregator, Dealer, DealerCommitment, ReshareState, SubShare};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};

/// serializable dealer commitment (hex-encoded)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitmentMsg {
    pub dealer_index: u32,
    pub threshold: u32,
    /// hex-encoded commitment points
    pub coefficients: Vec<String>,
}

impl CommitmentMsg {
    pub fn from_commitment(commitment: &DealerCommitment<RistrettoPoint>) -> Self {
        Self {
            dealer_index: commitment.dealer_index,
            threshold: commitment.threshold(),
            coefficients: commitment
                .coefficients
                .iter()
                .map(|p| hex::encode(p.compress().as_bytes()))
                .collect(),
        }
    }

    pub fn to_commitment(&self) -> Option<DealerCommitment<RistrettoPoint>> {
        let coefficients: Option<Vec<RistrettoPoint>> = self
            .coefficients
            .iter()
            .map(|hex_str| {
                let bytes = hex::decode(hex_str).ok()?;
                if bytes.len() != 32 {
                    return None;
                }
                let arr: [u8; 32] = bytes.try_into().ok()?;
                let compressed = CompressedRistretto::from_slice(&arr).ok()?;
                compressed.decompress()
            })
            .collect();

        Some(DealerCommitment {
            dealer_index: self.dealer_index,
            coefficients: coefficients?,
        })
    }
}

/// serializable subshare (hex-encoded scalar)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SubShareMsg {
    pub dealer_index: u32,
    pub player_index: u32,
    pub value: String, // hex-encoded scalar
}

impl SubShareMsg {
    pub fn from_subshare(subshare: &SubShare<Scalar>) -> Self {
        Self {
            dealer_index: subshare.dealer_index,
            player_index: subshare.player_index,
            value: hex::encode(subshare.value.as_bytes()),
        }
    }

    pub fn to_subshare(&self) -> Option<SubShare<Scalar>> {
        let bytes = hex::decode(&self.value).ok()?;
        if bytes.len() != 32 {
            return None;
        }
        let arr: [u8; 32] = bytes.try_into().ok()?;
        let scalar = Scalar::from_canonical_bytes(arr).into_option()?;

        Some(SubShare {
            dealer_index: self.dealer_index,
            player_index: self.player_index,
            value: scalar,
        })
    }
}

/// reshare session state for a provider
#[derive(Debug)]
pub enum ProviderRole {
    /// old provider: generates subshares for new providers
    Dealer(DealerState),
    /// new provider: collects subshares from old providers
    Aggregator(AggregatorState),
    /// both (when staying through reshare)
    Both {
        dealer: DealerState,
        aggregator: AggregatorState,
    },
}

/// dealer state for outgoing reshare
pub struct DealerState {
    dealer: Dealer<RistrettoPoint>,
    num_new_players: u32,
}

impl std::fmt::Debug for DealerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DealerState")
            .field("index", &self.dealer.index())
            .field("num_new_players", &self.num_new_players)
            .finish()
    }
}

impl DealerState {
    pub fn new(index: u32, share: Scalar, new_threshold: u32, num_new_players: u32) -> Self {
        let mut rng = OsRng;
        let dealer = Dealer::new(index, share, new_threshold, &mut rng);

        Self {
            dealer,
            num_new_players,
        }
    }

    pub fn commitment(&self) -> CommitmentMsg {
        CommitmentMsg::from_commitment(self.dealer.commitment())
    }

    pub fn generate_subshare(&self, player_index: u32) -> Option<SubShareMsg> {
        if player_index == 0 || player_index > self.num_new_players {
            return None;
        }

        let subshare = self.dealer.generate_subshare(player_index);
        Some(SubShareMsg::from_subshare(&subshare))
    }

    pub fn generate_all_subshares(&self) -> Vec<SubShareMsg> {
        (1..=self.num_new_players)
            .map(|j| SubShareMsg::from_subshare(&self.dealer.generate_subshare(j)))
            .collect()
    }
}

/// aggregator state for incoming reshare
pub struct AggregatorState {
    aggregator: Aggregator<RistrettoPoint>,
    old_threshold: u32,
    expected_group_key: RistrettoPoint,
}

impl std::fmt::Debug for AggregatorState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AggregatorState")
            .field("player_index", &self.aggregator.player_index())
            .field("count", &self.aggregator.count())
            .field("old_threshold", &self.old_threshold)
            .finish()
    }
}

impl AggregatorState {
    pub fn new(player_index: u32, old_threshold: u32, expected_group_key: RistrettoPoint) -> Self {
        Self {
            aggregator: Aggregator::new(player_index),
            old_threshold,
            expected_group_key,
        }
    }

    pub fn player_index(&self) -> u32 {
        self.aggregator.player_index()
    }

    pub fn count(&self) -> usize {
        self.aggregator.count()
    }

    pub fn has_threshold(&self) -> bool {
        self.aggregator.has_threshold(self.old_threshold)
    }

    /// add a subshare with verification
    pub fn add_subshare(
        &mut self,
        subshare_msg: &SubShareMsg,
        commitment_msg: &CommitmentMsg,
    ) -> Result<bool, ReshareError> {
        let subshare = subshare_msg
            .to_subshare()
            .ok_or(ReshareError::InvalidSubShare)?;
        let commitment = commitment_msg
            .to_commitment()
            .ok_or(ReshareError::InvalidCommitment)?;

        self.aggregator
            .add_subshare(subshare, commitment)
            .map_err(|e| ReshareError::OsstError(format!("{:?}", e)))
    }

    /// finalize reshare and get new share
    pub fn finalize(&self) -> Result<Scalar, ReshareError> {
        self.aggregator
            .finalize(self.old_threshold, &self.expected_group_key)
            .map_err(|e| ReshareError::OsstError(format!("{:?}", e)))
    }
}

/// reshare epoch state (for coordination)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReshareEpoch {
    pub epoch: u64,
    pub old_threshold: u32,
    pub new_threshold: u32,
    pub old_provider_count: u32,
    pub new_provider_count: u32,
    /// hex-encoded group pubkey
    pub group_pubkey: String,
    /// collected commitments from dealers
    pub commitments: Vec<Option<CommitmentMsg>>,
}

impl ReshareEpoch {
    pub fn new(
        epoch: u64,
        old_provider_count: u32,
        old_threshold: u32,
        new_threshold: u32,
        new_provider_count: u32,
        group_pubkey: &RistrettoPoint,
    ) -> Self {
        Self {
            epoch,
            old_threshold,
            new_threshold,
            old_provider_count,
            new_provider_count,
            group_pubkey: hex::encode(group_pubkey.compress().as_bytes()),
            commitments: vec![None; old_provider_count as usize],
        }
    }

    pub fn group_pubkey_point(&self) -> Option<RistrettoPoint> {
        let bytes = hex::decode(&self.group_pubkey).ok()?;
        if bytes.len() != 32 {
            return None;
        }
        let arr: [u8; 32] = bytes.try_into().ok()?;
        let compressed = CompressedRistretto::from_slice(&arr).ok()?;
        compressed.decompress()
    }

    pub fn submit_commitment(&mut self, commitment: CommitmentMsg) -> Result<bool, ReshareError> {
        let idx = commitment
            .dealer_index
            .checked_sub(1)
            .ok_or(ReshareError::InvalidIndex)? as usize;

        if idx >= self.commitments.len() {
            return Err(ReshareError::InvalidIndex);
        }

        if commitment.threshold != self.new_threshold {
            return Err(ReshareError::InvalidCommitment);
        }

        if self.commitments[idx].is_some() {
            return Ok(false); // duplicate
        }

        self.commitments[idx] = Some(commitment);
        Ok(true)
    }

    pub fn commitment_count(&self) -> usize {
        self.commitments.iter().filter(|c| c.is_some()).count()
    }

    pub fn has_quorum(&self) -> bool {
        self.commitment_count() >= self.old_threshold as usize
    }

    /// convert to osst ReshareState for verification
    pub fn to_osst_state(&self) -> Option<ReshareState<RistrettoPoint>> {
        let group_key = self.group_pubkey_point()?;

        let mut state = ReshareState::new(
            self.epoch,
            self.old_provider_count,
            self.old_threshold,
            self.new_threshold,
            self.new_provider_count,
            group_key,
        );

        for commitment_msg in self.commitments.iter().flatten() {
            if let Some(commitment) = commitment_msg.to_commitment() {
                let _ = state.submit_commitment(commitment);
            }
        }

        Some(state)
    }

    /// verify that committed shares reconstruct the expected group key
    pub fn verify_group_key(&self) -> Result<bool, ReshareError> {
        let state = self
            .to_osst_state()
            .ok_or(ReshareError::InvalidCommitment)?;

        state
            .verify_group_key()
            .map_err(|e| ReshareError::OsstError(format!("{:?}", e)))
    }
}

#[derive(Debug)]
pub enum ReshareError {
    InvalidIndex,
    InvalidCommitment,
    InvalidSubShare,
    InsufficientContributions { got: usize, need: usize },
    GroupKeyMismatch,
    OsstError(String),
}

impl std::fmt::Display for ReshareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidIndex => write!(f, "invalid index"),
            Self::InvalidCommitment => write!(f, "invalid commitment"),
            Self::InvalidSubShare => write!(f, "invalid subshare"),
            Self::InsufficientContributions { got, need } => {
                write!(f, "insufficient contributions: got {}, need {}", got, need)
            }
            Self::GroupKeyMismatch => write!(f, "group key mismatch after reshare"),
            Self::OsstError(e) => write!(f, "osst error: {}", e),
        }
    }
}

impl std::error::Error for ReshareError {}

#[cfg(test)]
mod tests {
    use super::*;
    use curve25519_dalek::constants::RISTRETTO_BASEPOINT_POINT;
    use osst::SecretShare;

    fn shamir_split(secret: &Scalar, n: u32, t: u32) -> Vec<SecretShare<Scalar>> {
        let mut rng = OsRng;
        let mut coeffs = vec![*secret];
        for _ in 1..t {
            coeffs.push(Scalar::random(&mut rng));
        }

        (1..=n)
            .map(|i| {
                let x = Scalar::from(i);
                let mut y = Scalar::ZERO;
                let mut x_pow = Scalar::ONE;
                for coeff in &coeffs {
                    y += coeff * x_pow;
                    x_pow *= x;
                }
                SecretShare::new(i, y)
            })
            .collect()
    }

    #[test]
    fn test_reshare_flow() {
        let mut rng = OsRng;

        // setup: 5/3 -> 7/5 reshare
        let secret = Scalar::random(&mut rng);
        let group_pubkey = RISTRETTO_BASEPOINT_POINT * secret;
        let old_shares = shamir_split(&secret, 5, 3);

        let old_n = 5u32;
        let old_t = 3u32;
        let new_n = 7u32;
        let new_t = 5u32;

        // create reshare epoch
        let mut epoch = ReshareEpoch::new(1, old_n, old_t, new_t, new_n, &group_pubkey);

        // create dealer states
        let dealers: Vec<DealerState> = old_shares
            .iter()
            .map(|s| DealerState::new(s.index, s.scalar, new_t, new_n))
            .collect();

        // submit commitments
        for dealer in &dealers {
            let commitment = dealer.commitment();
            epoch.submit_commitment(commitment).unwrap();
        }

        assert!(epoch.has_quorum());
        assert!(epoch.verify_group_key().unwrap());

        // create aggregator states for new players
        let mut aggregators: Vec<AggregatorState> = (1..=new_n)
            .map(|j| AggregatorState::new(j, old_t, group_pubkey))
            .collect();

        // distribute subshares
        for dealer in &dealers {
            let commitment = dealer.commitment();
            for agg in &mut aggregators {
                let subshare = dealer.generate_subshare(agg.player_index()).unwrap();
                agg.add_subshare(&subshare, &commitment).unwrap();
            }
        }

        // finalize and verify
        let mut new_shares = Vec::new();
        for agg in &aggregators {
            assert!(agg.has_threshold());
            let share = agg.finalize().unwrap();
            new_shares.push(share);
        }

        // verify new shares reconstruct secret (using first t)
        let indices: Vec<u32> = (1..=new_t).collect();
        let lagrange =
            osst::compute_lagrange_coefficients::<Scalar>(&indices).unwrap();

        let mut reconstructed = Scalar::ZERO;
        for (i, lambda) in lagrange.iter().enumerate() {
            reconstructed += lambda * new_shares[i];
        }

        assert_eq!(reconstructed, secret);
    }

    #[test]
    fn test_commitment_serialization() {
        let mut rng = OsRng;
        let secret = Scalar::random(&mut rng);
        let shares = shamir_split(&secret, 3, 2);

        let dealer = DealerState::new(shares[0].index, shares[0].scalar, 2, 3);
        let commitment = dealer.commitment();

        // roundtrip through json
        let json = serde_json::to_string(&commitment).unwrap();
        let recovered: CommitmentMsg = serde_json::from_str(&json).unwrap();

        assert_eq!(commitment.dealer_index, recovered.dealer_index);
        assert_eq!(commitment.threshold, recovered.threshold);

        // verify can convert back
        let osst_commitment = recovered.to_commitment().unwrap();
        assert_eq!(osst_commitment.dealer_index, commitment.dealer_index);
    }

    #[test]
    fn test_subshare_serialization() {
        let mut rng = OsRng;
        let secret = Scalar::random(&mut rng);
        let shares = shamir_split(&secret, 3, 2);

        let dealer = DealerState::new(shares[0].index, shares[0].scalar, 2, 3);
        let subshare = dealer.generate_subshare(1).unwrap();

        // roundtrip through json
        let json = serde_json::to_string(&subshare).unwrap();
        let recovered: SubShareMsg = serde_json::from_str(&json).unwrap();

        assert_eq!(subshare.dealer_index, recovered.dealer_index);
        assert_eq!(subshare.player_index, recovered.player_index);
    }

    #[test]
    fn test_epoch_serialization() {
        let mut rng = OsRng;
        let secret = Scalar::random(&mut rng);
        let group_pubkey = RISTRETTO_BASEPOINT_POINT * secret;

        let epoch = ReshareEpoch::new(1, 5, 3, 3, 5, &group_pubkey);

        // roundtrip through json
        let json = serde_json::to_string(&epoch).unwrap();
        let recovered: ReshareEpoch = serde_json::from_str(&json).unwrap();

        assert_eq!(epoch.epoch, recovered.epoch);
        assert_eq!(epoch.old_threshold, recovered.old_threshold);
        assert_eq!(epoch.group_pubkey, recovered.group_pubkey);
    }
}
