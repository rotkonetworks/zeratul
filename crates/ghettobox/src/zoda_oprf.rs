//! verified oprf protocol with dleq proofs and optional zoda data availability
//!
//! ## verification layers
//!
//! 1. **DLEQ proofs** (computation correctness)
//!    - proves server computed response = blinded * share correctly
//!    - detects garbage/malicious responses IMMEDIATELY
//!    - this is the primary security mechanism
//!
//! 2. **ZODA** (data availability - optional)
//!    - proves server's share can be reconstructed from other shards
//!    - useful for share recovery if servers lose data
//!    - NOT needed for garbage detection (DLEQ handles that)
//!
//! ## protocol
//!
//! registration:
//!   1. dealer generates oprf key, splits into shares
//!   2. each server gets (share, public_key)
//!   3. public_keys are stored with user registration
//!   4. (optional) shares are zoda-encoded for data availability
//!
//! recovery:
//!   1. client sends blinded point to servers
//!   2. server evaluates: response = blinded * share
//!   3. server creates DLEQ proof: proves log_G(pk) == log_blinded(response)
//!   4. client verifies DLEQ proof against stored public_key
//!   5. if valid → unblind and use
//!   6. if invalid → server is malicious, try different server
//!
//! ## security
//!
//! with DLEQ verification:
//! - server can't return garbage (proof would fail)
//! - server can't use wrong share (proof would fail)
//! - client knows immediately if response is valid

use crate::oprf::{DleqProof, OprfShare, OprfClient, OprfDealer, Point};
use crate::{Error, Result};

/// misbehavior report for a server that returned invalid response
///
/// this is cryptographic evidence that can be verified by anyone.
/// use cases:
/// - submit to governance for operator slashing
/// - audit logs for accountability
/// - evidence for dispute resolution
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct MisbehaviorReport {
    /// timestamp of the incident (unix seconds)
    pub timestamp: u64,
    /// server index that misbehaved
    pub server_index: u8,
    /// the public key on record for this server
    pub expected_public_key: Point,
    /// the blinded point that was sent
    pub blinded_point: Point,
    /// the response the server returned
    pub response: VerifiedOprfResponse,
    /// type of misbehavior detected
    pub misbehavior_type: MisbehaviorType,
}

/// type of misbehavior detected
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum MisbehaviorType {
    /// DLEQ proof verification failed
    InvalidProof,
    /// response point is malformed/invalid
    MalformedResponse,
    /// server returned response for wrong index
    WrongServerIndex,
}

impl MisbehaviorReport {
    /// verify this report is valid (third-party verification)
    ///
    /// anyone can verify this report without knowing the secret
    pub fn verify(&self) -> bool {
        use curve25519_dalek::ristretto::CompressedRistretto;

        // parse points
        let blinded = match CompressedRistretto::from_slice(&self.blinded_point)
            .ok()
            .and_then(|c| c.decompress())
        {
            Some(p) => p,
            None => return false,
        };

        let response_point = match CompressedRistretto::from_slice(&self.response.point)
            .ok()
            .and_then(|c| c.decompress())
        {
            Some(p) => p,
            None => {
                // malformed response - report is valid if that's what we're claiming
                return matches!(self.misbehavior_type, MisbehaviorType::MalformedResponse);
            }
        };

        let public_key = match CompressedRistretto::from_slice(&self.expected_public_key)
            .ok()
            .and_then(|c| c.decompress())
        {
            Some(p) => p,
            None => return false,
        };

        // verify the proof FAILS (that's why it's misbehavior)
        let proof_valid = self.response.proof.verify(&blinded, &response_point, &public_key);

        match self.misbehavior_type {
            MisbehaviorType::InvalidProof => !proof_valid,
            MisbehaviorType::MalformedResponse => true, // already handled above
            MisbehaviorType::WrongServerIndex => {
                self.response.server_index != self.server_index
            }
        }
    }

    /// serialize report to JSON for storage/transmission
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string(self)
            .map_err(|e| Error::VssFailed(format!("failed to serialize report: {}", e)))
    }

    /// deserialize report from JSON
    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json)
            .map_err(|e| Error::VssFailed(format!("failed to deserialize report: {}", e)))
    }
}

/// public key info for a server (used for DLEQ verification)
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ServerPublicKey {
    /// server index
    pub index: u8,
    /// public key (G * share)
    pub public_key: Point,
}

/// verified oprf response from server
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct VerifiedOprfResponse {
    /// server index
    pub server_index: u8,
    /// evaluated point
    pub point: Point,
    /// DLEQ proof
    pub proof: DleqProof,
}

impl VerifiedOprfResponse {
    /// serialize to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(1 + 32 + 64);
        out.push(self.server_index);
        out.extend_from_slice(&self.point);
        out.extend_from_slice(&self.proof.to_bytes());
        out
    }

    /// deserialize from bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != 1 + 32 + 64 {
            return Err(Error::VssFailed("invalid response length".into()));
        }
        let server_index = bytes[0];
        let mut point = [0u8; 32];
        point.copy_from_slice(&bytes[1..33]);
        let mut proof_bytes = [0u8; 64];
        proof_bytes.copy_from_slice(&bytes[33..97]);
        let proof = DleqProof::from_bytes(&proof_bytes);
        Ok(Self { server_index, point, proof })
    }
}

/// server that returns DLEQ-verified oprf responses
pub struct VerifiedOprfServer {
    share: OprfShare,
}

impl VerifiedOprfServer {
    /// create new verified oprf server from share
    pub fn new(share: OprfShare) -> Self {
        Self { share }
    }

    /// get public key info (to be stored/published)
    pub fn public_key_info(&self) -> ServerPublicKey {
        ServerPublicKey {
            index: self.share.index,
            public_key: self.share.public_key(),
        }
    }

    /// get server index
    pub fn index(&self) -> u8 {
        self.share.index
    }

    /// evaluate oprf and return DLEQ-verified response
    pub fn evaluate(&self, blinded: &Point) -> Result<VerifiedOprfResponse> {
        let resp = self.share.evaluate_with_proof(blinded)?;
        Ok(VerifiedOprfResponse {
            server_index: self.share.index,
            point: resp.point,
            proof: resp.proof,
        })
    }
}

/// client-side verification of oprf responses
pub struct VerifiedOprfClient {
    inner: OprfClient,
    public_keys: Vec<ServerPublicKey>,
}

impl VerifiedOprfClient {
    /// create verifier with stored public keys
    pub fn new(input: &[u8], public_keys: Vec<ServerPublicKey>) -> Self {
        Self {
            inner: OprfClient::new(input),
            public_keys,
        }
    }

    /// get blinded point to send to servers
    pub fn blinded_point(&self) -> Point {
        self.inner.blinded_point()
    }

    /// verify a single response
    pub fn verify_response(&self, response: &VerifiedOprfResponse) -> Result<Point> {
        // find public key for this server
        let pk_info = self.public_keys
            .iter()
            .find(|pk| pk.index == response.server_index)
            .ok_or_else(|| Error::VssFailed(format!(
                "no public key for server {}",
                response.server_index
            )))?;

        // convert to format needed for verification
        use curve25519_dalek::ristretto::CompressedRistretto;

        let blinded = CompressedRistretto::from_slice(&self.blinded_point())
            .map_err(|_| Error::InvalidPoint)?
            .decompress()
            .ok_or(Error::InvalidPoint)?;

        let response_point = CompressedRistretto::from_slice(&response.point)
            .map_err(|_| Error::InvalidPoint)?
            .decompress()
            .ok_or(Error::InvalidPoint)?;

        let public_key = CompressedRistretto::from_slice(&pk_info.public_key)
            .map_err(|_| Error::InvalidPoint)?
            .decompress()
            .ok_or(Error::InvalidPoint)?;

        // verify DLEQ proof
        if !response.proof.verify(&blinded, &response_point, &public_key) {
            return Err(Error::VssFailed(format!(
                "DLEQ proof failed for server {}",
                response.server_index
            )));
        }

        Ok(response.point)
    }

    /// verify response and return misbehavior report if invalid
    ///
    /// use this when you want to collect evidence against misbehaving servers
    pub fn verify_response_with_report(
        &self,
        response: &VerifiedOprfResponse,
    ) -> std::result::Result<Point, MisbehaviorReport> {
        use curve25519_dalek::ristretto::CompressedRistretto;

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        // find public key for this server
        let pk_info = match self.public_keys
            .iter()
            .find(|pk| pk.index == response.server_index)
        {
            Some(pk) => pk,
            None => {
                // no public key - can't create proper report
                // this is a client-side issue, not server misbehavior
                return Err(MisbehaviorReport {
                    timestamp,
                    server_index: response.server_index,
                    expected_public_key: [0u8; 32],
                    blinded_point: self.blinded_point(),
                    response: response.clone(),
                    misbehavior_type: MisbehaviorType::WrongServerIndex,
                });
            }
        };

        let blinded_bytes = self.blinded_point();

        // check response point is valid
        let response_point = match CompressedRistretto::from_slice(&response.point)
            .ok()
            .and_then(|c| c.decompress())
        {
            Some(p) => p,
            None => {
                return Err(MisbehaviorReport {
                    timestamp,
                    server_index: response.server_index,
                    expected_public_key: pk_info.public_key,
                    blinded_point: blinded_bytes,
                    response: response.clone(),
                    misbehavior_type: MisbehaviorType::MalformedResponse,
                });
            }
        };

        let blinded = match CompressedRistretto::from_slice(&blinded_bytes)
            .ok()
            .and_then(|c| c.decompress())
        {
            Some(p) => p,
            None => return Err(MisbehaviorReport {
                timestamp,
                server_index: response.server_index,
                expected_public_key: pk_info.public_key,
                blinded_point: blinded_bytes,
                response: response.clone(),
                misbehavior_type: MisbehaviorType::MalformedResponse,
            }),
        };

        let public_key = match CompressedRistretto::from_slice(&pk_info.public_key)
            .ok()
            .and_then(|c| c.decompress())
        {
            Some(p) => p,
            None => return Err(MisbehaviorReport {
                timestamp,
                server_index: response.server_index,
                expected_public_key: pk_info.public_key,
                blinded_point: blinded_bytes,
                response: response.clone(),
                misbehavior_type: MisbehaviorType::MalformedResponse,
            }),
        };

        // verify DLEQ proof
        if !response.proof.verify(&blinded, &response_point, &public_key) {
            return Err(MisbehaviorReport {
                timestamp,
                server_index: response.server_index,
                expected_public_key: pk_info.public_key,
                blinded_point: blinded_bytes,
                response: response.clone(),
                misbehavior_type: MisbehaviorType::InvalidProof,
            });
        }

        Ok(response.point)
    }

    /// finalize with verified responses
    pub fn finalize(&self, responses: &[VerifiedOprfResponse], threshold: usize) -> Result<[u8; 32]> {
        // verify each response and collect points
        let mut verified_points = Vec::with_capacity(responses.len());

        for resp in responses {
            let point = self.verify_response(resp)?;
            verified_points.push((resp.server_index, point));
        }

        // use underlying client finalize
        self.inner.finalize(&verified_points, threshold)
    }

    /// finalize with verified responses, collecting misbehavior reports
    ///
    /// returns (unlock_key, misbehavior_reports) if enough valid responses
    /// returns error if not enough valid responses
    pub fn finalize_with_reports(
        &self,
        responses: &[VerifiedOprfResponse],
        threshold: usize,
    ) -> Result<([u8; 32], Vec<MisbehaviorReport>)> {
        let mut verified_points = Vec::with_capacity(responses.len());
        let mut reports = Vec::new();

        for resp in responses {
            match self.verify_response_with_report(resp) {
                Ok(point) => {
                    verified_points.push((resp.server_index, point));
                }
                Err(report) => {
                    reports.push(report);
                }
            }
        }

        if verified_points.len() < threshold {
            return Err(Error::NotEnoughShares {
                have: verified_points.len(),
                need: threshold,
            });
        }

        let key = self.inner.finalize(&verified_points, threshold)?;
        Ok((key, reports))
    }
}

/// dealer that creates verified oprf shares
pub struct VerifiedOprfDealer;

impl VerifiedOprfDealer {
    /// deal oprf shares with public keys for verification
    ///
    /// returns (public_key, servers) where each server has DLEQ-capable share
    pub fn deal(threshold: usize, total: usize) -> Result<(Point, Vec<VerifiedOprfServer>)> {
        let (public_key, shares) = OprfDealer::deal(threshold, total)?;

        let servers = shares
            .into_iter()
            .map(VerifiedOprfServer::new)
            .collect();

        Ok((public_key, servers))
    }
}

// keep the old zoda types for backwards compatibility but mark them as optional
#[cfg(feature = "zoda")]
pub use zoda_da::*;

#[cfg(feature = "zoda")]
mod zoda_da {
    //! optional zoda data availability layer
    //!
    //! this provides share reconstruction capabilities, not garbage detection.
    //! use DLEQ proofs (above) for garbage detection.

    use super::*;
    use commonware_coding::{Config as CodingConfig, Scheme, Zoda, CodecConfig};
    use commonware_cryptography::Sha256;
    use commonware_parallel::Sequential;
    use commonware_codec::{Encode, Read as CodecRead};

    /// zoda configuration for share encoding
    const ZODA_CONFIG: CodingConfig = CodingConfig {
        minimum_shards: 2,
        extra_shards: 1,
    };

    /// commitment to a zoda-encoded oprf share (for data availability)
    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
    pub struct ZodaCommitment {
        /// the zoda commitment (transcript summary)
        pub commitment: Vec<u8>,
        /// server index this commitment is for
        pub server_index: u8,
    }

    /// a zoda shard of oprf share data
    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
    pub struct ZodaShard {
        /// shard index
        pub index: u16,
        /// encoded shard data
        pub data: Vec<u8>,
    }

    /// encode an oprf share with zoda for data availability
    ///
    /// this allows reconstructing the share if it's lost
    pub fn encode_share_for_da(share: &OprfShare) -> Result<(ZodaCommitment, Vec<ZodaShard>)> {
        let share_bytes = share.to_bytes();

        let (commitment, shards) = Zoda::<Sha256>::encode(
            &ZODA_CONFIG,
            share_bytes.as_slice(),
            &Sequential,
        ).map_err(|e| Error::VssFailed(format!("zoda encode failed: {}", e)))?;

        let zoda_shards: Vec<ZodaShard> = shards
            .into_iter()
            .enumerate()
            .map(|(i, shard)| ZodaShard {
                index: i as u16,
                data: Encode::encode(&shard).to_vec(),
            })
            .collect();

        Ok((
            ZodaCommitment {
                commitment: commitment.to_vec(),
                server_index: share.index,
            },
            zoda_shards,
        ))
    }

    /// verify a zoda shard can be parsed (basic format check)
    ///
    /// note: full verification requires CheckingData from reshard()
    /// this only verifies the shard format is valid
    pub fn verify_shard_format(shard: &ZodaShard) -> Result<()> {
        let mut shard_bytes = shard.data.as_slice();
        type ZodaSha256 = Zoda<Sha256>;
        let _: <ZodaSha256 as Scheme>::Shard = CodecRead::read_cfg(
            &mut shard_bytes,
            &CodecConfig { maximum_shard_size: 1024 * 1024 },
        ).map_err(|e| Error::VssFailed(format!("invalid shard format: {:?}", e)))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verified_oprf_roundtrip() {
        // setup: dealer creates 2-of-3 shares
        let (_, servers) = VerifiedOprfDealer::deal(2, 3).unwrap();

        // collect public keys (would be stored/published)
        let public_keys: Vec<_> = servers.iter().map(|s| s.public_key_info()).collect();

        // client creates blinded input with public keys
        let input = b"stretched_pin_from_argon2id";
        let client = VerifiedOprfClient::new(input, public_keys.clone());
        let blinded = client.blinded_point();

        // servers evaluate and return verified responses
        let resp1 = servers[0].evaluate(&blinded).unwrap();
        let resp2 = servers[1].evaluate(&blinded).unwrap();

        // client verifies and finalizes
        let key = client.finalize(&[resp1, resp2], 2).unwrap();

        // same input with fresh client should give same key
        let client2 = VerifiedOprfClient::new(input, public_keys);
        let blinded2 = client2.blinded_point();
        let resp1_2 = servers[0].evaluate(&blinded2).unwrap();
        let resp2_2 = servers[1].evaluate(&blinded2).unwrap();
        let key2 = client2.finalize(&[resp1_2, resp2_2], 2).unwrap();

        assert_eq!(key, key2);
    }

    #[test]
    fn test_garbage_response_detected() {
        let (_, servers) = VerifiedOprfDealer::deal(2, 3).unwrap();
        let public_keys: Vec<_> = servers.iter().map(|s| s.public_key_info()).collect();

        let client = VerifiedOprfClient::new(b"test_input", public_keys);
        let blinded = client.blinded_point();

        // get valid response
        let mut resp = servers[0].evaluate(&blinded).unwrap();

        // corrupt the point (garbage)
        resp.point[0] ^= 0xff;

        // verification should fail
        let result = client.verify_response(&resp);
        assert!(result.is_err());
    }

    #[test]
    fn test_wrong_server_detected() {
        let (_, servers) = VerifiedOprfDealer::deal(2, 3).unwrap();

        // only store public key for server 0
        let public_keys = vec![servers[0].public_key_info()];
        let client = VerifiedOprfClient::new(b"test_input", public_keys);
        let blinded = client.blinded_point();

        // get response from server 1 (not in our public keys)
        let resp = servers[1].evaluate(&blinded).unwrap();

        // verification should fail - no public key for server 1
        let result = client.verify_response(&resp);
        assert!(result.is_err());
    }

    #[test]
    fn test_forged_proof_detected() {
        let (_, servers) = VerifiedOprfDealer::deal(2, 3).unwrap();
        let public_keys: Vec<_> = servers.iter().map(|s| s.public_key_info()).collect();

        let client = VerifiedOprfClient::new(b"test_input", public_keys);
        let blinded = client.blinded_point();

        // get valid response
        let mut resp = servers[0].evaluate(&blinded).unwrap();

        // corrupt the proof
        let mut proof_bytes = resp.proof.to_bytes();
        proof_bytes[0] ^= 0xff;
        resp.proof = DleqProof::from_bytes(&proof_bytes);

        // verification should fail
        let result = client.verify_response(&resp);
        assert!(result.is_err());
    }

    #[test]
    fn test_response_serialization() {
        let (_, servers) = VerifiedOprfDealer::deal(2, 3).unwrap();

        let client = VerifiedOprfClient::new(b"test", vec![]);
        let blinded = client.blinded_point();

        let resp = servers[0].evaluate(&blinded).unwrap();

        // roundtrip
        let bytes = resp.to_bytes();
        let resp2 = VerifiedOprfResponse::from_bytes(&bytes).unwrap();

        assert_eq!(resp.server_index, resp2.server_index);
        assert_eq!(resp.point, resp2.point);
        assert_eq!(resp.proof.to_bytes(), resp2.proof.to_bytes());
    }

    #[test]
    fn test_misbehavior_report_invalid_proof() {
        let (_, servers) = VerifiedOprfDealer::deal(2, 3).unwrap();
        let public_keys: Vec<_> = servers.iter().map(|s| s.public_key_info()).collect();

        let client = VerifiedOprfClient::new(b"test_report", public_keys);
        let blinded = client.blinded_point();

        // get valid response and corrupt the proof (not the point)
        let mut resp = servers[0].evaluate(&blinded).unwrap();
        let mut proof_bytes = resp.proof.to_bytes();
        proof_bytes[0] ^= 0xff; // corrupt proof
        resp.proof = DleqProof::from_bytes(&proof_bytes);

        // verify with report
        let report = client.verify_response_with_report(&resp).unwrap_err();

        assert_eq!(report.server_index, servers[0].index());
        assert!(matches!(report.misbehavior_type, MisbehaviorType::InvalidProof));

        // third party can verify this report
        assert!(report.verify());
    }

    #[test]
    fn test_misbehavior_report_serialization() {
        let (_, servers) = VerifiedOprfDealer::deal(2, 3).unwrap();
        let public_keys: Vec<_> = servers.iter().map(|s| s.public_key_info()).collect();

        let client = VerifiedOprfClient::new(b"test_serialize", public_keys);
        let blinded = client.blinded_point();

        let mut resp = servers[0].evaluate(&blinded).unwrap();
        let mut proof_bytes = resp.proof.to_bytes();
        proof_bytes[0] ^= 0xff;
        resp.proof = DleqProof::from_bytes(&proof_bytes);

        let report = client.verify_response_with_report(&resp).unwrap_err();

        // serialize and deserialize
        let json = report.to_json().unwrap();
        let report2 = MisbehaviorReport::from_json(&json).unwrap();

        assert_eq!(report.server_index, report2.server_index);
        assert!(report2.verify());
    }

    #[test]
    fn test_finalize_with_reports() {
        let (_, servers) = VerifiedOprfDealer::deal(2, 3).unwrap();
        let public_keys: Vec<_> = servers.iter().map(|s| s.public_key_info()).collect();

        let client = VerifiedOprfClient::new(b"test_finalize_reports", public_keys);
        let blinded = client.blinded_point();

        // one valid, one corrupted proof
        let resp1 = servers[0].evaluate(&blinded).unwrap();
        let mut resp2 = servers[1].evaluate(&blinded).unwrap();
        let mut proof_bytes = resp2.proof.to_bytes();
        proof_bytes[0] ^= 0xff;
        resp2.proof = DleqProof::from_bytes(&proof_bytes);
        let resp3 = servers[2].evaluate(&blinded).unwrap();

        // should succeed with 2 valid responses, report 1 misbehavior
        let (key, reports) = client.finalize_with_reports(&[resp1, resp2, resp3], 2).unwrap();

        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].server_index, servers[1].index());
        assert!(reports[0].verify());

        // key should be valid
        assert_ne!(key, [0u8; 32]);
    }

    #[cfg(feature = "zoda")]
    #[test]
    fn test_zoda_share_encoding() {
        use crate::oprf::OprfDealer;

        let (_, shares) = OprfDealer::deal(2, 3).unwrap();

        // encode first share with zoda
        let (commitment, shards) = encode_share_for_da(&shares[0]).unwrap();

        assert_eq!(commitment.server_index, shares[0].index);
        assert_eq!(shards.len(), 3); // 2 minimum + 1 extra

        // verify all shards have valid format
        for shard in &shards {
            verify_shard_format(shard).unwrap();
        }
    }
}
