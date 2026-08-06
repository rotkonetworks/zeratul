// keys.rs — FROST group key → Orchard FullViewingKey → addresses
//
// after DKG, the group verifying key is a RedPallas SpendValidatingKey.
// combined with a random SpendingKey (for nk/rivk derivation), this
// produces a FullViewingKey that can receive funds and scan notes.
// spending requires t-of-n FROST signatures.

use orchard::keys::{FullViewingKey, SpendValidatingKey, SpendingKey};
use rand_core::{CryptoRng, RngCore};

use crate::frost_keys::PublicKeyPackage;

/// derive an Orchard FullViewingKey from the FROST group verifying key.
///
/// the group key becomes the SpendValidatingKey (ak). a random SpendingKey
/// is used to derive nk and rivk (these don't affect spend authorization,
/// only note scanning and address derivation).
///
/// this uses orchard's `from_sk_ak` (unstable-frost feature) which is
/// the ZF's official approach for FROST+Orchard integration.
pub fn derive_fvk(
    rng: &mut (impl RngCore + CryptoRng),
    pubkey_package: &PublicKeyPackage,
) -> Option<FullViewingKey> {
    let ak_bytes = pubkey_package.verifying_key().serialize().ok()?;
    let ak = SpendValidatingKey::from_bytes(&ak_bytes)?;

    // generate a random SpendingKey for nk/rivk derivation.
    // this is safe because nk/rivk don't participate in spend authorization —
    // only ak (the FROST group key) controls spending.
    let sk = loop {
        let mut random_bytes = [0u8; 32];
        rng.fill_bytes(&mut random_bytes);
        if let Some(sk) = SpendingKey::from_bytes(random_bytes).into() {
            break sk;
        }
    };

    Some(FullViewingKey::from_sk_ak(&sk, ak))
}

/// extract the SpendValidatingKey (ak) from a FROST key package.
/// this is the participant's view of the group public key.
pub fn group_ak(pubkey_package: &PublicKeyPackage) -> Option<SpendValidatingKey> {
    let ak_bytes = pubkey_package.verifying_key().serialize().ok()?;
    SpendValidatingKey::from_bytes(&ak_bytes)
}

/// derive an Orchard receiving address from the FROST group FVK.
pub fn derive_address(
    fvk: &FullViewingKey,
    diversifier_index: u32,
) -> orchard::Address {
    let diversifier = orchard::keys::Diversifier::from_bytes(
        diversifier_index.to_le_bytes()
            .iter()
            .copied()
            .chain(std::iter::repeat(0))
            .take(11)
            .collect::<Vec<_>>()
            .try_into()
            .unwrap()
    );
    fvk.address(diversifier, orchard::keys::Scope::External)
}
