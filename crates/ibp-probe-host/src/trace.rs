//! Host call tracing for ligerito proof generation

use serde::{Deserialize, Serialize};

/// A single host call record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostCallRecord {
    /// Host call ID (e.g., HOST_TCP_PING)
    pub call_id: u32,
    /// Input arguments (serialized)
    pub inputs: Vec<Vec<u8>>,
    /// Output/return value (serialized)
    pub output: Vec<u8>,
    /// Timestamp when call was made
    pub timestamp_ms: u64,
}

/// Complete trace of all host calls during execution
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HostCallTrace {
    /// All host calls in execution order
    pub calls: Vec<HostCallRecord>,
    /// Hash of the guest program
    pub program_hash: [u8; 32],
    /// Execution start time
    pub start_time_ms: u64,
    /// Execution end time
    pub end_time_ms: u64,
}

impl HostCallTrace {
    pub fn new() -> Self {
        Self {
            calls: Vec::new(),
            program_hash: [0; 32],
            start_time_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            end_time_ms: 0,
        }
    }

    pub fn record(&mut self, call: HostCallRecord) {
        self.calls.push(call);
    }

    pub fn finalize(&mut self) {
        self.end_time_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
    }

    /// Compute commitment to the trace
    pub fn commitment(&self) -> [u8; 32] {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        // Simple hash for now - in production use blake2/blake3
        let mut hasher = DefaultHasher::new();

        self.program_hash.hash(&mut hasher);
        self.start_time_ms.hash(&mut hasher);

        for call in &self.calls {
            call.call_id.hash(&mut hasher);
            for input in &call.inputs {
                input.hash(&mut hasher);
            }
            call.output.hash(&mut hasher);
        }

        let hash = hasher.finish();
        let mut result = [0u8; 32];
        result[..8].copy_from_slice(&hash.to_le_bytes());
        result
    }

    /// Serialize for inclusion in ligerito proof
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// Deserialize from bytes
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        serde_json::from_slice(data).ok()
    }
}

/// Witness data for verification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeWitness {
    /// The host call trace
    pub trace: HostCallTrace,
    /// Guest execution output
    pub output: Vec<u8>,
    /// Prover's signature over trace commitment
    pub signature: Vec<u8>,
    /// Prover's public key
    pub prover_pubkey: [u8; 32],
}

impl ProbeWitness {
    pub fn new(trace: HostCallTrace, output: Vec<u8>) -> Self {
        Self {
            trace,
            output,
            signature: Vec::new(),
            prover_pubkey: [0; 32],
        }
    }

    /// Sign the witness with prover's key
    pub fn sign(&mut self, private_key: &[u8; 32], public_key: &[u8; 32]) {
        self.prover_pubkey = *public_key;

        // Simple signature placeholder - use ed25519 in production
        let commitment = self.trace.commitment();
        let mut sig_input = Vec::new();
        sig_input.extend_from_slice(&commitment);
        sig_input.extend_from_slice(private_key);

        // Hash as signature (placeholder)
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        sig_input.hash(&mut hasher);
        self.signature = hasher.finish().to_le_bytes().to_vec();
    }

    /// Verify the witness signature
    pub fn verify(&self) -> bool {
        // Placeholder verification
        !self.signature.is_empty()
    }
}
