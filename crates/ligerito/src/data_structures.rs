use binary_fields::BinaryFieldElement;
use merkle_tree::{CompleteMerkleTree, MerkleRoot, BatchedMerkleProof};
use serde::{Serialize, Deserialize};

#[cfg(feature = "prover")]
use reed_solomon::ReedSolomon;

/// Prover configuration (only with prover feature)
#[cfg(feature = "prover")]
pub struct ProverConfig<T: BinaryFieldElement, U: BinaryFieldElement> {
    pub recursive_steps: usize,
    pub initial_dims: (usize, usize),
    pub dims: Vec<(usize, usize)>,
    pub initial_k: usize,
    pub ks: Vec<usize>,
    pub initial_reed_solomon: ReedSolomon<T>,
    pub reed_solomon_codes: Vec<ReedSolomon<U>>,
}

/// Verifier configuration
#[derive(Clone, Debug)]
pub struct VerifierConfig {
    pub recursive_steps: usize,
    pub initial_dim: usize,
    pub log_dims: Vec<usize>,
    pub initial_k: usize,
    pub ks: Vec<usize>,
}

/// Recursive Ligero witness (prover side only)
#[cfg(feature = "prover")]
pub struct RecursiveLigeroWitness<T: BinaryFieldElement> {
    pub mat: Vec<Vec<T>>,  // Row-major matrix
    pub tree: CompleteMerkleTree,
}

/// Recursive Ligero commitment
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecursiveLigeroCommitment {
    pub root: MerkleRoot,
}

impl RecursiveLigeroCommitment {
    pub fn size_of(&self) -> usize {
        self.root.size_of()
    }
}

/// Recursive Ligero proof
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecursiveLigeroProof<T: BinaryFieldElement> {
    pub opened_rows: Vec<Vec<T>>,
    pub merkle_proof: BatchedMerkleProof,
}

impl<T: BinaryFieldElement> RecursiveLigeroProof<T> {
    pub fn size_of(&self) -> usize {
        self.opened_rows.iter()
            .map(|row| row.len() * std::mem::size_of::<T>())
            .sum::<usize>()
            + self.merkle_proof.size_of()
    }
}

/// Final Ligero proof
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FinalLigeroProof<T: BinaryFieldElement> {
    pub yr: Vec<T>,
    pub opened_rows: Vec<Vec<T>>,
    pub merkle_proof: BatchedMerkleProof,
}

impl<T: BinaryFieldElement> FinalLigeroProof<T> {
    pub fn size_of(&self) -> usize {
        self.yr.len() * std::mem::size_of::<T>()
            + self.opened_rows.iter()
                .map(|row| row.len() * std::mem::size_of::<T>())
                .sum::<usize>()
            + self.merkle_proof.size_of()
    }
}

/// Sumcheck transcript
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SumcheckTranscript<T: BinaryFieldElement> {
    pub transcript: Vec<(T, T, T)>,  // Quadratic polynomial coefficients
}

impl<T: BinaryFieldElement> SumcheckTranscript<T> {
    pub fn size_of(&self) -> usize {
        self.transcript.len() * 3 * std::mem::size_of::<T>()
    }
}

/// Complete Ligerito proof (builder pattern)
pub struct LigeritoProof<T: BinaryFieldElement, U: BinaryFieldElement> {
    pub initial_ligero_cm: Option<RecursiveLigeroCommitment>,
    pub initial_ligero_proof: Option<RecursiveLigeroProof<T>>,
    pub recursive_commitments: Vec<RecursiveLigeroCommitment>,
    pub recursive_proofs: Vec<RecursiveLigeroProof<U>>,
    pub final_ligero_proof: Option<FinalLigeroProof<U>>,
    pub sumcheck_transcript: Option<SumcheckTranscript<U>>,
}

impl<T: BinaryFieldElement, U: BinaryFieldElement> LigeritoProof<T, U> {
    pub fn new() -> Self {
        Self {
            initial_ligero_cm: None,
            initial_ligero_proof: None,
            recursive_commitments: Vec::new(),
            recursive_proofs: Vec::new(),
            final_ligero_proof: None,
            sumcheck_transcript: None,
        }
    }
}

/// Finalized Ligerito proof
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FinalizedLigeritoProof<T: BinaryFieldElement, U: BinaryFieldElement> {
    pub initial_ligero_cm: RecursiveLigeroCommitment,
    pub initial_ligero_proof: RecursiveLigeroProof<T>,
    pub recursive_commitments: Vec<RecursiveLigeroCommitment>,
    pub recursive_proofs: Vec<RecursiveLigeroProof<U>>,
    pub final_ligero_proof: FinalLigeroProof<U>,
    pub sumcheck_transcript: SumcheckTranscript<U>,
}

impl<T: BinaryFieldElement, U: BinaryFieldElement> FinalizedLigeritoProof<T, U> {
    pub fn size_of(&self) -> usize {
        self.initial_ligero_cm.size_of()
            + self.initial_ligero_proof.size_of()
            + self.recursive_commitments.iter()
                .map(|c| c.size_of())
                .sum::<usize>()
            + self.recursive_proofs.iter()
                .map(|p| p.size_of())
                .sum::<usize>()
            + self.final_ligero_proof.size_of()
            + self.sumcheck_transcript.size_of()
    }
}

/// Finalize a proof builder into a complete proof
pub fn finalize<T: BinaryFieldElement, U: BinaryFieldElement>(
    proof: LigeritoProof<T, U>,
) -> crate::Result<FinalizedLigeritoProof<T, U>> {
    Ok(FinalizedLigeritoProof {
        initial_ligero_cm: proof.initial_ligero_cm
            .ok_or(crate::LigeritoError::InvalidProof)?,
        initial_ligero_proof: proof.initial_ligero_proof
            .ok_or(crate::LigeritoError::InvalidProof)?,
        recursive_commitments: proof.recursive_commitments,
        recursive_proofs: proof.recursive_proofs,
        final_ligero_proof: proof.final_ligero_proof
            .ok_or(crate::LigeritoError::InvalidProof)?,
        sumcheck_transcript: proof.sumcheck_transcript
            .ok_or(crate::LigeritoError::InvalidProof)?,
    })
}
