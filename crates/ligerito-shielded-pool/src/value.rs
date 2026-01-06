//! value types for shielded pool
//!
//! represents assets and amounts in the pool

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

/// asset identifier (32 bytes, derived from asset metadata)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AssetId(pub [u8; 32]);

impl AssetId {
    /// native token asset id
    pub const NATIVE: Self = Self([0u8; 32]);

    /// create from bytes
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// derive asset id from metadata (chain_id, token_address, etc)
    pub fn derive(metadata: &[u8]) -> Self {
        let hash = blake3::hash(metadata);
        Self(*hash.as_bytes())
    }

    /// xcm multilocation asset id
    pub fn from_multilocation(encoded: &[u8]) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"ligerito.asset.xcm.v1");
        hasher.update(encoded);
        Self(*hasher.finalize().as_bytes())
    }
}

/// amount (u128 to match substrate)
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Amount(pub u128);

impl Amount {
    pub const ZERO: Self = Self(0);

    pub fn new(amount: u128) -> Self {
        Self(amount)
    }

    pub fn checked_add(self, other: Self) -> Option<Self> {
        self.0.checked_add(other.0).map(Self)
    }

    pub fn checked_sub(self, other: Self) -> Option<Self> {
        self.0.checked_sub(other.0).map(Self)
    }

    pub fn saturating_add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }

    pub fn saturating_sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }

    pub fn is_zero(&self) -> bool {
        self.0 == 0
    }
}

impl From<u128> for Amount {
    fn from(v: u128) -> Self {
        Self(v)
    }
}

impl From<u64> for Amount {
    fn from(v: u64) -> Self {
        Self(v as u128)
    }
}

impl From<Amount> for u128 {
    fn from(v: Amount) -> Self {
        v.0
    }
}

/// a typed value (asset + amount)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Value {
    pub asset_id: AssetId,
    pub amount: Amount,
}

impl Value {
    pub fn new(asset_id: AssetId, amount: Amount) -> Self {
        Self { asset_id, amount }
    }

    pub fn native(amount: impl Into<Amount>) -> Self {
        Self {
            asset_id: AssetId::NATIVE,
            amount: amount.into(),
        }
    }

    /// encode for commitment
    pub fn to_bytes(&self) -> [u8; 48] {
        let mut bytes = [0u8; 48];
        bytes[..32].copy_from_slice(&self.asset_id.0);
        bytes[32..48].copy_from_slice(&self.amount.0.to_le_bytes());
        bytes
    }

    /// commitment to value (blinded)
    pub fn commit(&self, blinding: &[u8; 32]) -> ValueCommitment {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"ligerito.value.commit.v1");
        hasher.update(&self.to_bytes());
        hasher.update(blinding);
        ValueCommitment(*hasher.finalize().as_bytes())
    }
}

/// pedersen-style value commitment (hiding)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValueCommitment(pub [u8; 32]);

impl ValueCommitment {
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_value_commitment() {
        let v1 = Value::native(1000u64);
        let v2 = Value::native(1000u64);

        let blind1 = [1u8; 32];
        let blind2 = [2u8; 32];

        // same value, same blinding = same commitment
        assert_eq!(v1.commit(&blind1), v2.commit(&blind1));

        // same value, different blinding = different commitment
        assert_ne!(v1.commit(&blind1), v1.commit(&blind2));
    }

    #[test]
    fn test_asset_id_derive() {
        let id1 = AssetId::derive(b"DOT");
        let id2 = AssetId::derive(b"USDC");
        assert_ne!(id1, id2);
    }
}
