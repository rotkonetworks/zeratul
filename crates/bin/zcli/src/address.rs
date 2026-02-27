// from zafu-wasm — BIP32 + transparent address derivation, orchard key derivation

use orchard::keys::{FullViewingKey, Scope, SpendingKey};
use sha2::Digest as _;

use crate::error::Error;
use crate::key::WalletSeed;

// -- BIP32 HD key derivation (from zafu-wasm) --

struct Bip32Key {
    key: [u8; 32],
    chain_code: [u8; 32],
}

fn bip32_master_key(seed: &[u8]) -> Bip32Key {
    use hmac::{Hmac, Mac};
    use sha2::Sha512;

    let mut mac = Hmac::<Sha512>::new_from_slice(b"Bitcoin seed")
        .expect("HMAC accepts any key length");
    mac.update(seed);
    let result = mac.finalize().into_bytes();

    let mut key = [0u8; 32];
    let mut chain_code = [0u8; 32];
    key.copy_from_slice(&result[..32]);
    chain_code.copy_from_slice(&result[32..]);

    Bip32Key { key, chain_code }
}

fn bip32_derive_child(parent: &Bip32Key, index: u32, hardened: bool) -> Result<Bip32Key, Error> {
    use hmac::{Hmac, Mac};
    use k256::elliptic_curve::sec1::ToEncodedPoint;
    use k256::elliptic_curve::PrimeField;
    use sha2::Sha512;

    let mut mac = Hmac::<Sha512>::new_from_slice(&parent.chain_code)
        .expect("HMAC accepts any key length");

    let child_index = if hardened { index | 0x80000000 } else { index };

    if hardened {
        mac.update(&[0x00]);
        mac.update(&parent.key);
    } else {
        let secret_key = k256::SecretKey::from_slice(&parent.key)
            .map_err(|e| Error::Address(format!("invalid parent key: {}", e)))?;
        let pubkey = secret_key.public_key();
        let compressed = pubkey.to_encoded_point(true);
        mac.update(compressed.as_bytes());
    }

    mac.update(&child_index.to_be_bytes());
    let result = mac.finalize().into_bytes();

    let il = &result[..32];
    let ir = &result[32..];

    let mut parent_bytes = k256::FieldBytes::default();
    parent_bytes.copy_from_slice(&parent.key);
    let parent_scalar = k256::Scalar::from_repr(parent_bytes);
    if bool::from(parent_scalar.is_none()) {
        return Err(Error::Address("invalid parent scalar".into()));
    }
    let parent_scalar = parent_scalar.unwrap();

    let mut il_bytes = k256::FieldBytes::default();
    il_bytes.copy_from_slice(il);
    let il_scalar = k256::Scalar::from_repr(il_bytes);
    if bool::from(il_scalar.is_none()) {
        return Err(Error::Address("invalid IL scalar".into()));
    }
    let il_scalar = il_scalar.unwrap();

    let child_scalar = il_scalar + parent_scalar;

    let mut key = [0u8; 32];
    key.copy_from_slice(&child_scalar.to_repr());

    let mut chain_code = [0u8; 32];
    chain_code.copy_from_slice(ir);

    Ok(Bip32Key { key, chain_code })
}

// -- transparent address --

/// HASH160 = RIPEMD160(SHA256(data))  (from zafu-wasm)
fn hash160(data: &[u8]) -> [u8; 20] {
    let sha = sha2::Sha256::digest(data);
    let ripe = ripemd::Ripemd160::digest(sha);
    let mut out = [0u8; 20];
    out.copy_from_slice(&ripe);
    out
}

/// base58check encode with version prefix
fn base58check_encode(version: &[u8], payload: &[u8]) -> String {
    let mut data = Vec::with_capacity(version.len() + payload.len() + 4);
    data.extend_from_slice(version);
    data.extend_from_slice(payload);
    let checksum = sha2::Sha256::digest(sha2::Sha256::digest(&data));
    data.extend_from_slice(&checksum[..4]);

    const ALPHABET: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let mut num = data.clone();
    let mut out = Vec::new();
    while !num.iter().all(|&b| b == 0) {
        let mut rem = 0u32;
        for byte in num.iter_mut() {
            let acc = (rem << 8) | (*byte as u32);
            *byte = (acc / 58) as u8;
            rem = acc % 58;
        }
        out.push(ALPHABET[rem as usize]);
    }
    for &b in data.iter() {
        if b == 0 { out.push(b'1'); } else { break; }
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

/// derive transparent private key from WalletSeed (public API for tx.rs)
pub fn derive_transparent_key(seed: &WalletSeed) -> Result<[u8; 32], Error> {
    derive_transparent_privkey(seed.as_bytes())
}

/// derive transparent private key from seed at m/44'/133'/0'/0/0
fn derive_transparent_privkey(seed: &[u8]) -> Result<[u8; 32], Error> {
    let master = bip32_master_key(seed);
    let c44 = bip32_derive_child(&master, 44, true)?;
    let c133 = bip32_derive_child(&c44, 133, true)?;
    let c0 = bip32_derive_child(&c133, 0, true)?;
    let c_change = bip32_derive_child(&c0, 0, false)?;
    let c_index = bip32_derive_child(&c_change, 0, false)?;
    Ok(c_index.key)
}

/// derive transparent address (t1...) from seed
pub fn transparent_address(seed: &WalletSeed, mainnet: bool) -> Result<String, Error> {
    let privkey = derive_transparent_privkey(seed.as_bytes())?;
    transparent_address_from_privkey(&privkey, mainnet)
}

/// derive transparent address from a raw secp256k1 private key
fn transparent_address_from_privkey(privkey: &[u8; 32], mainnet: bool) -> Result<String, Error> {
    let signing_key = k256::ecdsa::SigningKey::from_slice(privkey)
        .map_err(|e| Error::Address(format!("invalid privkey: {}", e)))?;
    let pubkey = signing_key.verifying_key().to_encoded_point(true);
    let pkh = hash160(pubkey.as_bytes());

    // zcash t-addr version: mainnet=0x1cb8, testnet=0x1d25
    let version = if mainnet { &[0x1c, 0xb8][..] } else { &[0x1d, 0x25][..] };
    Ok(base58check_encode(version, &pkh))
}

/// derive orchard unified address from seed
pub fn orchard_address(seed: &WalletSeed, mainnet: bool) -> Result<String, Error> {
    let seed_bytes = seed.as_bytes();
    if seed_bytes.len() != 64 {
        return Err(Error::Address("seed must be 64 bytes".into()));
    }

    let coin_type = if mainnet { 133 } else { 1 };

    let sk = SpendingKey::from_zip32_seed(seed_bytes, coin_type, zip32::AccountId::ZERO)
        .map_err(|_| Error::Address("failed to derive spending key".into()))?;

    let fvk = FullViewingKey::from(&sk);
    let addr = fvk.address_at(0u64, Scope::External);

    // encode as unified address using zcash_keys
    encode_unified_address(&addr, mainnet)
}

/// encode an orchard address as a unified address string
fn encode_unified_address(addr: &orchard::Address, mainnet: bool) -> Result<String, Error> {
    use zcash_address::unified::Encoding;

    let raw = addr.to_raw_address_bytes();
    let items = vec![zcash_address::unified::Receiver::Orchard(raw)];
    let ua = zcash_address::unified::Address::try_from_items(items)
        .map_err(|e| Error::Address(format!("UA construction: {}", e)))?;
    #[allow(deprecated)]
    let network = if mainnet {
        zcash_address::Network::Main
    } else {
        zcash_address::Network::Test
    };
    Ok(ua.encode(&network))
}

/// get the full viewing key from seed (for display/export)
pub fn full_viewing_key(seed: &WalletSeed, mainnet: bool) -> Result<FullViewingKey, Error> {
    let coin_type = if mainnet { 133 } else { 1 };
    let sk = SpendingKey::from_zip32_seed(seed.as_bytes(), coin_type, zip32::AccountId::ZERO)
        .map_err(|_| Error::Address("failed to derive spending key".into()))?;
    Ok(FullViewingKey::from(&sk))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::load_mnemonic_seed;

    const TEST_MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon \
         abandon abandon abandon abandon abandon abandon abandon abandon \
         abandon abandon abandon abandon abandon abandon abandon art";

    #[test]
    fn transparent_addr_starts_with_t1() {
        let seed = load_mnemonic_seed(TEST_MNEMONIC).unwrap();
        let addr = transparent_address(&seed, true).unwrap();
        assert!(addr.starts_with("t1"), "got: {}", addr);
    }

    #[test]
    fn transparent_addr_testnet_starts_with_tm() {
        let seed = load_mnemonic_seed(TEST_MNEMONIC).unwrap();
        let addr = transparent_address(&seed, false).unwrap();
        assert!(addr.starts_with("tm"), "got: {}", addr);
    }

    #[test]
    fn transparent_addr_deterministic() {
        let seed = load_mnemonic_seed(TEST_MNEMONIC).unwrap();
        let a1 = transparent_address(&seed, true).unwrap();
        let a2 = transparent_address(&seed, true).unwrap();
        assert_eq!(a1, a2);
    }

    #[test]
    fn transparent_addr_matches_zafu_wasm() {
        // cross-check: derive the same way zafu-wasm does via BIP32
        let mnemonic = bip39::Mnemonic::parse(TEST_MNEMONIC).unwrap();
        let seed_bytes = mnemonic.to_seed("");

        let master = bip32_master_key(&seed_bytes);
        let c = bip32_derive_child(&master, 44, true).unwrap();
        let c = bip32_derive_child(&c, 133, true).unwrap();
        let c = bip32_derive_child(&c, 0, true).unwrap();
        let c = bip32_derive_child(&c, 0, false).unwrap();
        let c = bip32_derive_child(&c, 0, false).unwrap();

        use k256::elliptic_curve::sec1::ToEncodedPoint;
        let sk = k256::ecdsa::SigningKey::from_slice(&c.key).unwrap();
        let pubkey = sk.verifying_key().to_encoded_point(true);
        let pkh = hash160(pubkey.as_bytes());
        let addr = base58check_encode(&[0x1c, 0xb8], &pkh);
        assert!(addr.starts_with("t1"));

        // now derive via our high-level API (uses mnemonic→seed directly)
        let wallet_seed = load_mnemonic_seed(TEST_MNEMONIC).unwrap();
        let addr2 = transparent_address(&wallet_seed, true).unwrap();

        // for mnemonic, WalletSeed.bytes == mnemonic.to_seed("") since we use standard bip39
        assert_eq!(addr, addr2);
    }

    #[test]
    fn orchard_addr_roundtrip() {
        let seed = load_mnemonic_seed(TEST_MNEMONIC).unwrap();
        let addr = orchard_address(&seed, true).unwrap();
        assert!(addr.starts_with("u1"), "got: {}", addr);
    }
}
