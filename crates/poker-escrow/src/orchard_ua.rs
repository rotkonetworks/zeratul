//! Orchard UA + UFVK encoding for FROST-derived group keys.

use zcash_address::unified::{Address as UnifiedAddress, Encoding, Fvk, Receiver, Ufvk};
use zcash_protocol::consensus::NetworkType as Network;

pub fn encode_unified(raw: [u8; 43], network: Network) -> Result<String, String> {
    let receiver = Receiver::Orchard(raw);
    let ua = UnifiedAddress::try_from_items(vec![receiver])
        .map_err(|e| format!("UA assembly failed: {}", e))?;
    Ok(ua.encode(&network))
}

/// mirrors zcli zcash-wasm `frost_derive_ufvk` so all DKG parties land on the same string
pub fn encode_ufvk_from_sk(
    public_key_package_hex: &str,
    sk_bytes: [u8; 32],
    network: Network,
) -> Result<String, String> {
    let bytes = fvk_bytes_from_sk(public_key_package_hex, sk_bytes)?;
    let ufvk = Ufvk::try_from_items(vec![Fvk::Orchard(bytes)])
        .map_err(|e| format!("UFVK assembly failed: {}", e))?;
    Ok(ufvk.encode(&network))
}

/// raw 96-byte Orchard FVK from the DKG group pubkey + host-broadcast sk
pub fn fvk_bytes_from_sk(
    public_key_package_hex: &str,
    sk_bytes: [u8; 32],
) -> Result<[u8; 96], String> {
    let pubkeys: frost_spend::frost_keys::PublicKeyPackage =
        frost_spend::orchestrate::from_hex(public_key_package_hex)
            .map_err(|e| format!("pkg parse: {:?}", e))?;
    let fvk = frost_spend::keys::derive_fvk_from_sk(sk_bytes, &pubkeys)
        .ok_or_else(|| "derive_fvk_from_sk returned None".to_string())?;
    Ok(fvk.to_bytes())
}

pub fn network_from_str(s: &str) -> Network {
    match s {
        "main" | "mainnet" => Network::Main,
        "regtest" => Network::Regtest,
        _ => Network::Test,
    }
}
