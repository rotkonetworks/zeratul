//! Fiat-Shamir transcript implementations with 0-based indexing
//! 
//! Updated to use 0-based indexing throughout for better performance
use binary_fields::BinaryFieldElement;
use merkle_tree::MerkleRoot;
use sha2::{Sha256, Digest};
use rand::{Rng, SeedableRng};
use rand::rngs::StdRng;
use std::collections::HashSet;

/// Trait for Fiat-Shamir transcripts
pub trait Transcript: Send + Sync {
    /// Absorb a Merkle root
    fn absorb_root(&mut self, root: &MerkleRoot);

    /// Absorb field elements
    fn absorb_elems<F: BinaryFieldElement>(&mut self, elems: &[F]);

    /// Absorb a single field element
    fn absorb_elem<F: BinaryFieldElement>(&mut self, elem: F);

    /// Get a field element challenge
    fn get_challenge<F: BinaryFieldElement>(&mut self) -> F;

    /// Get a query index (0-based)
    fn get_query(&mut self, max: usize) -> usize;

    /// Get multiple distinct queries (0-based)
    /// Returns min(count, max) queries to avoid infinite loops
    fn get_distinct_queries(&mut self, max: usize, count: usize) -> Vec<usize>;
}

/// Merlin-based Fiat-Shamir transcript (recommended)
pub struct MerlinTranscript {
    transcript: merlin::Transcript,
}

impl MerlinTranscript {
    pub fn new(domain: &'static [u8]) -> Self {
        Self {
            transcript: merlin::Transcript::new(domain),
        }
    }
}

impl Transcript for MerlinTranscript {
    fn absorb_root(&mut self, root: &MerkleRoot) {
        if let Some(hash) = &root.root {
            self.transcript.append_message(b"merkle_root", hash);
        }
    }

    fn absorb_elems<F: BinaryFieldElement>(&mut self, elems: &[F]) {
        let bytes = unsafe {
            std::slice::from_raw_parts(
                elems.as_ptr() as *const u8,
                elems.len() * std::mem::size_of::<F>()
            )
        };
        self.transcript.append_message(b"field_elements", bytes);
    }

    fn absorb_elem<F: BinaryFieldElement>(&mut self, elem: F) {
        let bytes = unsafe {
            std::slice::from_raw_parts(
                &elem as *const F as *const u8,
                std::mem::size_of::<F>()
            )
        };
        self.transcript.append_message(b"field_element", bytes);
    }

    fn get_challenge<F: BinaryFieldElement>(&mut self) -> F {
        let field_bytes = std::mem::size_of::<F>();
        let mut bytes = vec![0u8; field_bytes];

        // Get initial challenge bytes
        self.transcript.challenge_bytes(b"challenge", &mut bytes);

        // Convert bytes to field element
        let mut result = F::zero();
        let bits_needed = match field_bytes {
            4 => 32,   // BinaryElem32
            16 => 128, // BinaryElem128
            _ => field_bytes * 8,
        };

        // Create a more diverse bit pattern
        let mut bit_count = 0;
        for (_byte_idx, &byte) in bytes.iter().enumerate() {
            for bit_idx in 0..8 {
                if bit_count >= bits_needed {
                    break;
                }

                if (byte >> bit_idx) & 1 == 1 {
                    // Create x^bit_count where x is the primitive element
                    let mut power = if bit_count == 0 {
                        F::one()
                    } else {
                        // Use a primitive element (not 1) for the base
                        let mut base = F::from_bits(2); // x in GF(2^n)
                        let mut result = F::one();
                        for _ in 0..bit_count {
                            result = result.mul(&base);
                        }
                        result
                    };
                    result = result.add(&power);
                }
                bit_count += 1;
            }
            if bit_count >= bits_needed {
                break;
            }
        }

        // If we got all ones (which happens when bytes = [1, 0, 0, ...])
        // or all zeros, we need to ensure diversity
        if result == F::one() || result == F::zero() {
            // Mix in the byte position to create diversity
            self.transcript.append_message(b"retry", &bytes);
            self.transcript.challenge_bytes(b"challenge_retry", &mut bytes);

            // XOR with position-based pattern to ensure different challenges
            for i in 0..4 {
                if i < field_bytes {
                    bytes[i] ^= (i as u8 + 1) * 17; // Use prime multiplier for better distribution
                }
            }

            // Recompute with mixed bytes
            result = F::zero();
            bit_count = 0;
            for (_byte_idx, &byte) in bytes.iter().enumerate() {
                for bit_idx in 0..8 {
                    if bit_count >= bits_needed {
                        break;
                    }

                    if (byte >> bit_idx) & 1 == 1 {
                        // Create x^bit_count where x is the primitive element
                        let mut power = if bit_count == 0 {
                            F::one()
                        } else {
                            // Use a primitive element (not 1) for the base
                            let mut base = F::from_bits(2); // x in GF(2^n)
                            let mut result = F::one();
                            for _ in 0..bit_count {
                                result = result.mul(&base);
                            }
                            result
                        };
                        result = result.add(&power);
                    }
                    bit_count += 1;
                }
                if bit_count >= bits_needed {
                    break;
                }
            }
        }

        result
    }

    fn get_query(&mut self, max: usize) -> usize {
        let mut bytes = [0u8; 8];
        self.transcript.challenge_bytes(b"query", &mut bytes);
        let value = u64::from_le_bytes(bytes);
        (value as usize) % max  // Returns 0..max-1 (0-based)
    }

    fn get_distinct_queries(&mut self, max: usize, count: usize) -> Vec<usize> {
        // Can't get more distinct queries than max available
        let actual_count = count.min(max);
        let mut queries = Vec::with_capacity(actual_count);
        let mut seen = HashSet::new();

        while queries.len() < actual_count {
            let q = self.get_query(max);
            if seen.insert(q) {
                queries.push(q);
            }
        }

        queries.sort_unstable();
        queries
    }
}

/// SHA256-based Fiat-Shamir transcript (Julia-compatible mode)
pub struct Sha256Transcript {
    hasher: Sha256,
    counter: u32,
    julia_compatible: bool,
}

impl Sha256Transcript {
    pub fn new(seed: i32) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(&seed.to_le_bytes());

        Self {
            hasher,
            counter: 0,
            julia_compatible: false,
        }
    }

    /// Create a Julia-compatible transcript (1-based queries)
    pub fn new_julia_compatible(seed: i32) -> Self {
        let mut transcript = Self::new(seed);
        transcript.julia_compatible = true;
        transcript
    }

    fn squeeze_rng(&mut self) -> StdRng {
        self.hasher.update(&self.counter.to_le_bytes());
        self.counter += 1;

        let digest = self.hasher.clone().finalize();
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&digest[..32]);
        StdRng::from_seed(seed)
    }
}

impl Transcript for Sha256Transcript {
    fn absorb_root(&mut self, root: &MerkleRoot) {
        if let Some(hash) = &root.root {
            self.hasher.update(hash);
        }
    }

    fn absorb_elems<F: BinaryFieldElement>(&mut self, elems: &[F]) {
        let bytes = unsafe {
            std::slice::from_raw_parts(
                elems.as_ptr() as *const u8,
                elems.len() * std::mem::size_of::<F>()
            )
        };
        self.hasher.update(bytes);
    }

    fn absorb_elem<F: BinaryFieldElement>(&mut self, elem: F) {
        let bytes = unsafe {
            std::slice::from_raw_parts(
                &elem as *const F as *const u8,
                std::mem::size_of::<F>()
            )
        };
        self.hasher.update(bytes);
    }

    fn get_challenge<F: BinaryFieldElement>(&mut self) -> F {
        let mut rng = self.squeeze_rng();
        
        // Generate random bytes and convert to field element
        match std::mem::size_of::<F>() {
            4 => {
                // BinaryElem32
                let value: u32 = rng.gen();
                F::from_bits(value as u64)
            }
            16 => {
                // BinaryElem128
                // Generate 128 bits of randomness
                let low: u64 = rng.gen();
                let high: u64 = rng.gen();
                
                // For BinaryElem128, we need to properly construct the field element
                // The from_bits might only use the lower 64 bits, so we need a different approach
                let mut result = F::zero();
                
                // Set bits 0-63
                for i in 0..64 {
                    if (low >> i) & 1 == 1 {
                        let bit_value = F::from_bits(1u64 << i);
                        result = result.add(&bit_value);
                    }
                }
                
                // Set bits 64-127
                // Pre-compute 2^64 once
                let mut power_of_2_64 = F::from_bits(1u64 << 63);
                power_of_2_64 = power_of_2_64.add(&power_of_2_64); // 2^64

                // Build up powers incrementally
                let mut current_power = power_of_2_64;
                for i in 0..64 {
                    if (high >> i) & 1 == 1 {
                        result = result.add(&current_power);
                    }
                    if i < 63 {
                        current_power = current_power.add(&current_power); // Double for next bit
                    }
                }
                
                result
            }
            _ => {
                // Generic fallback for other sizes
                let mut result = F::zero();
                let num_bits = std::mem::size_of::<F>() * 8;

                // Handle first 64 bits
                for i in 0..num_bits.min(64) {
                    if rng.gen_bool(0.5) {
                        let bit_value = F::from_bits(1u64 << i);
                        result = result.add(&bit_value);
                    }
                }

                // Handle bits beyond 64 if needed
                if num_bits > 64 {
                    // Pre-compute 2^64
                    let mut power_of_2_64 = F::from_bits(1u64 << 63);
                    power_of_2_64 = power_of_2_64.add(&power_of_2_64);

                    // Build up powers incrementally
                    let mut current_power = power_of_2_64;
                    for i in 64..num_bits {
                        if rng.gen_bool(0.5) {
                            result = result.add(&current_power);
                        }
                        if i < num_bits - 1 {
                            current_power = current_power.add(&current_power);
                        }
                    }
                }
                
                result
            }
        }
    }

    fn get_query(&mut self, max: usize) -> usize {
        let mut rng = self.squeeze_rng();
        if self.julia_compatible {
            rng.gen_range(1..=max) - 1  // Generate 1-based, return 0-based
        } else {
            rng.gen_range(0..max)  // Direct 0-based
        }
    }

    fn get_distinct_queries(&mut self, max: usize, count: usize) -> Vec<usize> {
        // Can't get more distinct queries than max available
        let actual_count = count.min(max);
        let mut queries = Vec::with_capacity(actual_count);
        let mut seen = HashSet::new();

        while queries.len() < actual_count {
            let q = self.get_query(max);
            if seen.insert(q) {
                queries.push(q);
            }
        }

        queries.sort_unstable();
        queries
    }
}

/// Factory for creating transcripts
pub enum TranscriptType {
    Merlin,
    Sha256(i32), // seed
}

/// Wrapper type that can hold either transcript implementation
pub enum FiatShamir {
    Merlin(MerlinTranscript),
    Sha256(Sha256Transcript),
}

impl FiatShamir {
    /// Create a new transcript
    pub fn new(transcript_type: TranscriptType) -> Self {
        match transcript_type {
            TranscriptType::Merlin => {
                FiatShamir::Merlin(MerlinTranscript::new(b"ligerito-v1"))
            }
            TranscriptType::Sha256(seed) => {
                FiatShamir::Sha256(Sha256Transcript::new(seed))
            }
        }
    }

    /// Create Merlin transcript (recommended)
    pub fn new_merlin() -> Self {
        Self::new(TranscriptType::Merlin)
    }

    /// Create SHA256 transcript (Julia-compatible with 1-based indexing)
    pub fn new_sha256(seed: i32) -> Self {
        // Always use Julia-compatible mode for SHA256 to match the Julia implementation
        let mut transcript = Sha256Transcript::new(seed);
        transcript.julia_compatible = true;
        FiatShamir::Sha256(transcript)
    }
}

// Implement Transcript trait for the wrapper
impl Transcript for FiatShamir {
    fn absorb_root(&mut self, root: &MerkleRoot) {
        match self {
            FiatShamir::Merlin(t) => t.absorb_root(root),
            FiatShamir::Sha256(t) => t.absorb_root(root),
        }
    }

    fn absorb_elems<F: BinaryFieldElement>(&mut self, elems: &[F]) {
        match self {
            FiatShamir::Merlin(t) => t.absorb_elems(elems),
            FiatShamir::Sha256(t) => t.absorb_elems(elems),
        }
    }

    fn absorb_elem<F: BinaryFieldElement>(&mut self, elem: F) {
        match self {
            FiatShamir::Merlin(t) => t.absorb_elem(elem),
            FiatShamir::Sha256(t) => t.absorb_elem(elem),
        }
    }

    fn get_challenge<F: BinaryFieldElement>(&mut self) -> F {
        match self {
            FiatShamir::Merlin(t) => t.get_challenge(),
            FiatShamir::Sha256(t) => t.get_challenge(),
        }
    }

    fn get_query(&mut self, max: usize) -> usize {
        match self {
            FiatShamir::Merlin(t) => t.get_query(max),
            FiatShamir::Sha256(t) => t.get_query(max),
        }
    }

    fn get_distinct_queries(&mut self, max: usize, count: usize) -> Vec<usize> {
        match self {
            FiatShamir::Merlin(t) => t.get_distinct_queries(max, count),
            FiatShamir::Sha256(t) => t.get_distinct_queries(max, count),
        }
    }
}
