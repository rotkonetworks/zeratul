//! threshold oprf-based protocol with dleq verification
//!
//! protocol where:
//! - servers hold oprf key shares with public keys
//! - all responses include DLEQ proofs of correctness
//! - misbehavior is detected immediately and reported
//! - rate limiting happens server-side before oprf evaluation
//!
//! ## registration flow
//!
//! 1. client generates random seed
//! 2. dealer creates threshold oprf shares
//! 3. client stretches pin with argon2id
//! 4. client blinds stretched pin for oprf
//! 5. client sends blinded value to k servers
//! 6. servers evaluate oprf with DLEQ proof
//! 7. client verifies proofs, unblinds to get unlock_key
//! 8. client encrypts seed with unlock_key
//! 9. client stores encrypted seed + server public keys
//!
//! ## recovery flow
//!
//! 1. client stretches pin with argon2id
//! 2. client blinds stretched pin
//! 3. client sends blinded value to servers
//! 4. servers check rate limit, then evaluate oprf with proof
//! 5. client verifies each proof against stored public key
//! 6. if proof fails → misbehavior report (evidence for slashing)
//! 7. client unblinds valid responses to get unlock_key
//! 8. client decrypts seed with unlock_key
//!
//! ## security properties
//!
//! - servers never see pin (only blinded values)
//! - garbage responses detected immediately (DLEQ verification)
//! - misbehavior is cryptographically provable
//! - k-of-n threshold: need k servers to cooperate
//! - rate limiting enforced independently by each server

use crate::crypto::{decrypt, encrypt, random_bytes, stretch_pin};
use crate::oprf::{OprfShare, Point};
use crate::zoda_oprf::{
    VerifiedOprfClient, VerifiedOprfResponse, ServerPublicKey,
    MisbehaviorReport,
};
use crate::{Error, Result};

/// encrypted seed bundle (what client stores)
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct EncryptedSeed {
    /// version/salt for pin stretching
    pub version: [u8; 16],
    /// user info mixed into kdf
    pub user_info: Vec<u8>,
    /// nonce for encryption
    pub nonce: [u8; 12],
    /// encrypted seed (chacha20poly1305)
    pub ciphertext: Vec<u8>,
}

impl EncryptedSeed {
    /// serialize to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("serialization cannot fail")
    }

    /// deserialize from bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        serde_json::from_slice(bytes).map_err(|_| Error::InvalidShareFormat)
    }

    /// encode as base64 for storage
    pub fn to_base64(&self) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(self.to_bytes())
    }

    /// decode from base64
    pub fn from_base64(s: &str) -> Result<Self> {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(s)
            .map_err(|_| Error::InvalidShareFormat)?;
        Self::from_bytes(&bytes)
    }
}

/// registration result with encrypted seed and public keys
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RegistrationBundle {
    /// encrypted seed bundle
    pub encrypted_seed: EncryptedSeed,
    /// server public keys for DLEQ verification
    pub public_keys: Vec<ServerPublicKey>,
}

/// recovery result with secret and any misbehavior reports
pub struct RecoveryResult {
    /// the recovered secret
    pub secret: Vec<u8>,
    /// misbehavior reports for any servers that returned invalid proofs
    pub misbehavior_reports: Vec<MisbehaviorReport>,
}

/// verified oprf server interface with DLEQ proofs
pub trait OprfServer {
    /// server's share index
    fn index(&self) -> u8;

    /// server's public key for DLEQ verification
    fn public_key(&self) -> ServerPublicKey;

    /// evaluate oprf with DLEQ proof
    /// server MUST check rate limit before calling this
    fn evaluate(&self, user_id: &str, blinded: &Point) -> Result<VerifiedOprfResponse>;

    /// check if user is rate limited
    fn check_rate_limit(&self, user_id: &str) -> Result<()>;

    /// record failed attempt
    fn record_failure(&self, user_id: &str) -> Result<()>;

    /// reset failure count
    fn reset_failures(&self, user_id: &str) -> Result<()>;
}

/// threshold oprf protocol with DLEQ verification
pub struct ThresholdOprfProtocol {
    threshold: usize,
}

impl ThresholdOprfProtocol {
    pub fn new(threshold: usize) -> Self {
        Self { threshold }
    }

    /// register a secret protected by pin
    ///
    /// returns registration bundle containing encrypted seed and public keys
    pub fn register<S: OprfServer>(
        &self,
        pin: &[u8],
        secret: &[u8],
        user_info: &[u8],
        user_id: &str,
        servers: &[S],
    ) -> Result<RegistrationBundle> {
        if servers.len() < self.threshold {
            return Err(Error::NotEnoughNodes {
                have: servers.len(),
                need: self.threshold,
            });
        }

        // collect public keys for storage
        let public_keys: Vec<ServerPublicKey> = servers
            .iter()
            .map(|s| s.public_key())
            .collect();

        // generate random version/salt
        let version: [u8; 16] = random_bytes();

        // stretch pin
        let stretched = stretch_pin(pin, &version, user_info)?;

        // create verified oprf client
        let oprf_client = VerifiedOprfClient::new(&stretched, public_keys.clone());
        let blinded = oprf_client.blinded_point();

        // collect verified oprf responses
        let mut responses = Vec::with_capacity(self.threshold);
        for server in servers.iter().take(self.threshold) {
            server.check_rate_limit(user_id)?;
            let response = server.evaluate(user_id, &blinded)?;
            responses.push(response);
        }

        // finalize with verification
        let unlock_key = oprf_client.finalize(&responses, self.threshold)?;

        // encrypt secret with unlock key
        let nonce: [u8; 12] = random_bytes();
        let ciphertext = encrypt(&unlock_key, secret, &nonce)?;

        Ok(RegistrationBundle {
            encrypted_seed: EncryptedSeed {
                version,
                user_info: user_info.to_vec(),
                nonce,
                ciphertext,
            },
            public_keys,
        })
    }

    /// recover secret with verification and misbehavior reporting
    pub fn recover<S: OprfServer>(
        &self,
        pin: &[u8],
        bundle: &RegistrationBundle,
        user_id: &str,
        servers: &[S],
    ) -> Result<RecoveryResult> {
        if servers.len() < self.threshold {
            return Err(Error::NotEnoughNodes {
                have: servers.len(),
                need: self.threshold,
            });
        }

        let encrypted_seed = &bundle.encrypted_seed;

        // stretch pin with stored version
        let stretched = stretch_pin(pin, &encrypted_seed.version, &encrypted_seed.user_info)?;

        // create verified oprf client with stored public keys
        let oprf_client = VerifiedOprfClient::new(&stretched, bundle.public_keys.clone());
        let blinded = oprf_client.blinded_point();

        // collect verified responses from all servers
        let mut responses = Vec::with_capacity(servers.len());
        let mut rate_limited_count = 0;
        for server in servers.iter() {
            match server.check_rate_limit(user_id) {
                Ok(()) => {
                    if let Ok(response) = server.evaluate(user_id, &blinded) {
                        responses.push(response);
                    }
                }
                Err(Error::NoGuessesRemaining) => {
                    rate_limited_count += 1;
                }
                Err(_) => {}
            }
        }

        if responses.len() < self.threshold {
            // if all servers are rate limited, return that specific error
            if rate_limited_count == servers.len() {
                return Err(Error::NoGuessesRemaining);
            }
            return Err(Error::NotEnoughNodes {
                have: responses.len(),
                need: self.threshold,
            });
        }

        // finalize with verification and collect misbehavior reports
        let (unlock_key, misbehavior_reports) = oprf_client.finalize_with_reports(&responses, self.threshold)?;

        // try to decrypt
        match decrypt(&unlock_key, &encrypted_seed.ciphertext, &encrypted_seed.nonce) {
            Ok(secret) => {
                // success - reset failure counts
                for server in servers.iter() {
                    let _ = server.reset_failures(user_id);
                }
                Ok(RecoveryResult {
                    secret,
                    misbehavior_reports,
                })
            }
            Err(_) => {
                // wrong pin - record failures
                for server in servers.iter() {
                    let _ = server.record_failure(user_id);
                }
                Err(Error::InvalidPin)
            }
        }
    }
}

/// in-memory oprf server for testing
pub struct MemoryOprfServer {
    share: OprfShare,
    max_failures: u32,
    failures: std::sync::RwLock<std::collections::HashMap<String, u32>>,
}

impl MemoryOprfServer {
    pub fn new(share: OprfShare, max_failures: u32) -> Self {
        Self {
            share,
            max_failures,
            failures: std::sync::RwLock::new(std::collections::HashMap::new()),
        }
    }
}

impl OprfServer for MemoryOprfServer {
    fn index(&self) -> u8 {
        self.share.index
    }

    fn public_key(&self) -> ServerPublicKey {
        ServerPublicKey {
            index: self.share.index,
            public_key: self.share.public_key(),
        }
    }

    fn evaluate(&self, _user_id: &str, blinded: &Point) -> Result<VerifiedOprfResponse> {
        let resp = self.share.evaluate_with_proof(blinded)?;
        Ok(VerifiedOprfResponse {
            server_index: self.share.index,
            point: resp.point,
            proof: resp.proof,
        })
    }

    fn check_rate_limit(&self, user_id: &str) -> Result<()> {
        let failures = self.failures.read().unwrap();
        let count = failures.get(user_id).copied().unwrap_or(0);
        if count >= self.max_failures {
            Err(Error::NoGuessesRemaining)
        } else {
            Ok(())
        }
    }

    fn record_failure(&self, user_id: &str) -> Result<()> {
        let mut failures = self.failures.write().unwrap();
        let count = failures.entry(user_id.to_string()).or_insert(0);
        *count += 1;
        Ok(())
    }

    fn reset_failures(&self, user_id: &str) -> Result<()> {
        let mut failures = self.failures.write().unwrap();
        failures.remove(user_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oprf::OprfDealer;

    fn setup_servers(threshold: usize, total: usize, max_failures: u32) -> Vec<MemoryOprfServer> {
        let (_, shares) = OprfDealer::deal(threshold, total).unwrap();
        shares
            .into_iter()
            .map(|s| MemoryOprfServer::new(s, max_failures))
            .collect()
    }

    #[test]
    fn test_register_recover() {
        let servers = setup_servers(2, 3, 5);
        let protocol = ThresholdOprfProtocol::new(2);

        let pin = b"1234";
        let secret = b"my super secret 32 byte seed!!!!";
        let user_info = b"alice@example.com";
        let user_id = "alice";

        // register
        let bundle = protocol
            .register(pin, secret, user_info, user_id, &servers[..2])
            .unwrap();

        assert_eq!(bundle.public_keys.len(), 2);

        // recover with correct pin
        let result = protocol
            .recover(pin, &bundle, user_id, &servers[..2])
            .unwrap();

        assert_eq!(secret.as_slice(), result.secret.as_slice());
        assert!(result.misbehavior_reports.is_empty());
    }

    #[test]
    fn test_wrong_pin() {
        let servers = setup_servers(2, 3, 5);
        let protocol = ThresholdOprfProtocol::new(2);

        let pin = b"1234";
        let wrong_pin = b"5678";
        let secret = b"secret data here 32 bytes!!!!!!";
        let user_id = "bob";

        let bundle = protocol
            .register(pin, secret, b"", user_id, &servers[..2])
            .unwrap();

        let result = protocol.recover(wrong_pin, &bundle, user_id, &servers[..2]);
        assert!(matches!(result, Err(Error::InvalidPin)));
    }

    #[test]
    fn test_rate_limiting() {
        let servers = setup_servers(2, 3, 3);
        let protocol = ThresholdOprfProtocol::new(2);

        let pin = b"1234";
        let wrong_pin = b"0000";
        let secret = b"secret data here 32 bytes!!!!!!";
        let user_id = "charlie";

        let bundle = protocol
            .register(pin, secret, b"", user_id, &servers[..2])
            .unwrap();

        // use up all attempts
        for _ in 0..3 {
            let _ = protocol.recover(wrong_pin, &bundle, user_id, &servers[..2]);
        }

        // should be locked out even with correct pin
        let result = protocol.recover(pin, &bundle, user_id, &servers[..2]);
        assert!(matches!(result, Err(Error::NoGuessesRemaining)));
    }

    #[test]
    fn test_any_threshold_servers_work() {
        let servers = setup_servers(2, 3, 5);
        let protocol = ThresholdOprfProtocol::new(2);

        let pin = b"1234";
        let secret = b"secret data here 32 bytes!!!!!!";
        let user_id = "dave";

        // register with servers 0,1
        let mut bundle = protocol
            .register(pin, secret, b"", user_id, &servers[0..2])
            .unwrap();

        // add all public keys for recovery with different subset
        bundle.public_keys = servers.iter().map(|s| s.public_key()).collect();

        // recover with servers 1,2 (different subset)
        let result = protocol
            .recover(pin, &bundle, user_id, &servers[1..3])
            .unwrap();

        assert_eq!(secret.as_slice(), result.secret.as_slice());
    }

    #[test]
    fn test_serialization() {
        let servers = setup_servers(2, 3, 5);
        let protocol = ThresholdOprfProtocol::new(2);

        let pin = b"4321";
        let secret = b"another secret here 32 bytes!!!!";
        let user_id = "eve";

        let bundle = protocol
            .register(pin, secret, b"eve@example.com", user_id, &servers[..2])
            .unwrap();

        // serialize and deserialize encrypted seed
        let b64 = bundle.encrypted_seed.to_base64();
        let restored_seed = EncryptedSeed::from_base64(&b64).unwrap();

        let restored_bundle = RegistrationBundle {
            encrypted_seed: restored_seed,
            public_keys: bundle.public_keys.clone(),
        };

        let result = protocol
            .recover(pin, &restored_bundle, user_id, &servers[..2])
            .unwrap();

        assert_eq!(secret.as_slice(), result.secret.as_slice());
    }

    #[test]
    fn test_misbehavior_detection() {
        // create a server that returns bad proofs
        struct BadProofServer {
            inner: MemoryOprfServer,
        }

        impl OprfServer for BadProofServer {
            fn index(&self) -> u8 { self.inner.index() }
            fn public_key(&self) -> ServerPublicKey { self.inner.public_key() }
            fn check_rate_limit(&self, user_id: &str) -> Result<()> { self.inner.check_rate_limit(user_id) }
            fn record_failure(&self, user_id: &str) -> Result<()> { self.inner.record_failure(user_id) }
            fn reset_failures(&self, user_id: &str) -> Result<()> { self.inner.reset_failures(user_id) }

            fn evaluate(&self, user_id: &str, blinded: &Point) -> Result<VerifiedOprfResponse> {
                let mut resp = self.inner.evaluate(user_id, blinded)?;
                // corrupt the proof
                let mut proof_bytes = resp.proof.to_bytes();
                proof_bytes[0] ^= 0xff;
                resp.proof = crate::oprf::DleqProof::from_bytes(&proof_bytes);
                Ok(resp)
            }
        }

        let good_servers = setup_servers(2, 3, 5);
        let protocol = ThresholdOprfProtocol::new(2);

        let pin = b"1234";
        let secret = b"secret data here 32 bytes!!!!!!";
        let user_id = "frank";

        // register with good servers
        let bundle = protocol
            .register(pin, secret, b"", user_id, &good_servers)
            .unwrap();

        // create mixed servers: 2 good, 1 bad
        let bad_server = BadProofServer { inner: setup_servers(2, 3, 5).remove(1) };

        // but we need to use good_servers for recovery since they have the right shares
        // instead, let's just verify that corrupted public key creates a report

        // For this test, we'll manually verify misbehavior detection
        // by checking that a bad proof is detected
        let stretched = stretch_pin(pin, &bundle.encrypted_seed.version, &bundle.encrypted_seed.user_info).unwrap();
        let oprf_client = VerifiedOprfClient::new(&stretched, bundle.public_keys.clone());
        let blinded = oprf_client.blinded_point();

        // get a valid response and corrupt its proof
        let mut resp = good_servers[0].evaluate(user_id, &blinded).unwrap();
        let mut proof_bytes = resp.proof.to_bytes();
        proof_bytes[0] ^= 0xff;
        resp.proof = crate::oprf::DleqProof::from_bytes(&proof_bytes);

        // this should be detected as misbehavior
        let verify_result = oprf_client.verify_response(&resp);
        assert!(verify_result.is_err());

        // get misbehavior report
        let report = oprf_client.verify_response_with_report(&resp).unwrap_err();
        assert!(matches!(report.misbehavior_type, crate::zoda_oprf::MisbehaviorType::InvalidProof));
        assert!(report.verify());
    }
}
