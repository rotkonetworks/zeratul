//! Fiat-Shamir transcript implementations
//! Supports both Merlin and SHA256-based transcripts

use binary_fields::BinaryFieldElement;
use merkle_tree::MerkleRoot;

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
    
    /// Get a query index
    fn get_query(&mut self, max: usize) -> usize;
    
    /// Get multiple distinct queries
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
        let mut bytes = vec![0u8; std::mem::size_of::<F>()];
        self.transcript.challenge_bytes(b"challenge", &mut bytes);
        
        // Build field element from bytes
        let mut result = F::zero();
        let mut power = F::one();
        
        for byte in bytes {
            for i in 0..8 {
                if (byte >> i) & 1 == 1 {
                    result = result.add(&power);
                }
                power = power.add(&power);
            }
        }
        
        result
    }

    fn get_query(&mut self, max: usize) -> usize {
        let mut bytes = [0u8; 8];
        self.transcript.challenge_bytes(b"query", &mut bytes);
        let value = u64::from_le_bytes(bytes);
        (value as usize) % max + 1
    }

    fn get_distinct_queries(&mut self, max: usize, count: usize) -> Vec<usize> {
        let mut queries = Vec::with_capacity(count);
        let mut seen = std::collections::HashSet::new();

        while queries.len() < count {
            let q = self.get_query(max);
            if seen.insert(q) {
                queries.push(q);
            }
        }

        queries.sort_unstable();
        queries
    }
}

/// SHA256-based Fiat-Shamir transcript (Julia-compatible)
pub struct Sha256Transcript {
    hasher: sha2::Sha256,
    counter: u32,
}

impl Sha256Transcript {
    pub fn new(seed: i32) -> Self {
        use sha2::Digest;
        
        let mut hasher = sha2::Sha256::new();
        hasher.update(&seed.to_le_bytes());
        
        Self {
            hasher,
            counter: 0,
        }
    }
    
    fn squeeze_rng(&mut self) -> rand::rngs::StdRng {
        use sha2::Digest;
        use rand::SeedableRng;
        
        self.hasher.update(&self.counter.to_le_bytes());
        self.counter += 1;
        
        let digest = self.hasher.clone().finalize();
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&digest[..32]);
        rand::rngs::StdRng::from_seed(seed)
    }
}

impl Transcript for Sha256Transcript {
    fn absorb_root(&mut self, root: &MerkleRoot) {
        use sha2::Digest;
        
        if let Some(hash) = &root.root {
            self.hasher.update(hash);
        }
    }

    fn absorb_elems<F: BinaryFieldElement>(&mut self, elems: &[F]) {
        use sha2::Digest;
        
        let bytes = unsafe {
            std::slice::from_raw_parts(
                elems.as_ptr() as *const u8,
                elems.len() * std::mem::size_of::<F>()
            )
        };
        self.hasher.update(bytes);
    }

    fn absorb_elem<F: BinaryFieldElement>(&mut self, elem: F) {
        use sha2::Digest;
        
        let bytes = unsafe {
            std::slice::from_raw_parts(
                &elem as *const F as *const u8,
                std::mem::size_of::<F>()
            )
        };
        self.hasher.update(bytes);
    }

    fn get_challenge<F: BinaryFieldElement>(&mut self) -> F {
        use rand::Rng;
        
        let mut rng = self.squeeze_rng();
        let mut result = F::zero();
        let num_bits = std::mem::size_of::<F>() * 8;
        
        for i in 0..num_bits {
            if rng.gen_bool(0.5) {
                let mut bit = F::one();
                for _ in 0..i {
                    bit = bit.add(&bit);
                }
                result = result.add(&bit);
            }
        }
        
        result
    }

    fn get_query(&mut self, max: usize) -> usize {
        use rand::Rng;
        
        let mut rng = self.squeeze_rng();
        rng.gen_range(1..=max)
    }

    fn get_distinct_queries(&mut self, max: usize, count: usize) -> Vec<usize> {
        let mut queries = Vec::with_capacity(count);
        let mut seen = std::collections::HashSet::new();

        while queries.len() < count {
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
    
    /// Create SHA256 transcript (Julia-compatible)
    pub fn new_sha256(seed: i32) -> Self {
        Self::new(TranscriptType::Sha256(seed))
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
