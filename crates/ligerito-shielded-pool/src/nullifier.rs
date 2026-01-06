//! nullifiers for preventing double-spends
//!
//! when a note is spent, its nullifier is published
//! if nullifier already exists in the set, the spend is rejected

use crate::keys::NullifierKey;
use crate::note::NoteCommitment;
use crate::NULLIFIER_DOMAIN;

/// position in the state commitment tree
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Position(pub u64);

impl Position {
    pub fn new(pos: u64) -> Self {
        Self(pos)
    }

    pub fn to_bytes(&self) -> [u8; 8] {
        self.0.to_le_bytes()
    }
}

/// nullifier - unique identifier for a spent note
///
/// derived from:
/// - nullifier key (secret, from spend key)
/// - note commitment
/// - position in tree
///
/// this ensures only the owner can compute the nullifier,
/// and each note has exactly one nullifier
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Nullifier(pub [u8; 32]);

impl Nullifier {
    /// derive nullifier for a note
    pub fn derive(
        nk: &NullifierKey,
        commitment: &NoteCommitment,
        position: Position,
    ) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(NULLIFIER_DOMAIN);
        hasher.update(&nk.0);
        hasher.update(&commitment.0);
        hasher.update(&position.to_bytes());
        Self(*hasher.finalize().as_bytes())
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.0
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl AsRef<[u8]> for Nullifier {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// nullifier set - tracks spent notes
/// in production this would be a sparse merkle tree or similar
#[cfg(feature = "std")]
pub struct NullifierSet {
    nullifiers: std::collections::HashSet<Nullifier>,
}

#[cfg(feature = "std")]
impl NullifierSet {
    pub fn new() -> Self {
        Self {
            nullifiers: std::collections::HashSet::new(),
        }
    }

    /// check if nullifier exists (note already spent)
    pub fn contains(&self, nullifier: &Nullifier) -> bool {
        self.nullifiers.contains(nullifier)
    }

    /// insert nullifier (mark note as spent)
    /// returns false if already exists (double-spend attempt)
    pub fn insert(&mut self, nullifier: Nullifier) -> bool {
        self.nullifiers.insert(nullifier)
    }

    /// number of spent notes
    pub fn len(&self) -> usize {
        self.nullifiers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nullifiers.is_empty()
    }
}

#[cfg(feature = "std")]
impl Default for NullifierSet {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::SpendKey;
    use crate::note::{Note, Rseed};
    use crate::value::{AssetId, Value};

    #[test]
    fn test_nullifier_derivation() {
        let sk = SpendKey::from_phrase("test", "");
        let nk = sk.nullifier_key();
        let addr = sk.address(0);
        let value = Value::new(AssetId::NATIVE, 1000u64.into());
        let note = Note::new(value, addr, Rseed([1u8; 32]));
        let commitment = note.commit();
        let pos = Position::new(42);

        let nf = Nullifier::derive(&nk, &commitment, pos);

        // same inputs = same nullifier
        let nf2 = Nullifier::derive(&nk, &commitment, pos);
        assert_eq!(nf, nf2);

        // different position = different nullifier
        let nf3 = Nullifier::derive(&nk, &commitment, Position::new(43));
        assert_ne!(nf, nf3);

        // different note = different nullifier
        let note2 = Note::new(value, addr, Rseed([2u8; 32]));
        let nf4 = Nullifier::derive(&nk, &note2.commit(), pos);
        assert_ne!(nf, nf4);
    }

    #[test]
    fn test_nullifier_set() {
        let mut set = NullifierSet::new();
        let nf = Nullifier([1u8; 32]);

        assert!(!set.contains(&nf));
        assert!(set.insert(nf));
        assert!(set.contains(&nf));
        assert!(!set.insert(nf)); // double-spend rejected
    }
}
