//! Genesis configuration presets for Zanchor runtime

use crate::{
    AccountId, AuraConfig, BalancesConfig, ParachainInfoConfig,
    PolkadotXcmConfig, RuntimeGenesisConfig, SudoConfig, EXISTENTIAL_DEPOSIT,
};

use alloc::{vec, vec::Vec};
use cumulus_primitives_core::ParaId;
use frame_support::build_struct_json_patch;
use serde_json::Value;
use sp_consensus_aura::sr25519::AuthorityId as AuraId;
use sp_genesis_builder::PresetId;
use xcm::latest::prelude::XCM_VERSION;

// Well-known development accounts (same as Sr25519Keyring)
// These are derived from the standard substrate development mnemonics
fn alice_public() -> [u8; 32] {
    // Alice's sr25519 public key
    hex_literal::hex!("d43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d")
}

fn bob_public() -> [u8; 32] {
    // Bob's sr25519 public key
    hex_literal::hex!("8eaf04151687736326c9fea17e25fc5287613693c912909cb226aa4794f26a48")
}

fn charlie_public() -> [u8; 32] {
    hex_literal::hex!("90b5ab205c6974c9ea841be688864633dc9ca8a357843eeacf2314649965fe22")
}

fn dave_public() -> [u8; 32] {
    hex_literal::hex!("306721211d5404bd9da88e0204360a1a9ab8b87c66c1bc2fcdd37f3c2222cc20")
}

fn eve_public() -> [u8; 32] {
    hex_literal::hex!("e659a7a1628cdd93febc04a4e0646ea20e9f5f0ce097d9a05290d4a9e054df4e")
}

fn ferdie_public() -> [u8; 32] {
    hex_literal::hex!("1cbd2d43530a44705ad088af313e18f80b53ef16b36177cd4b77b846f2a5f07c")
}

fn well_known_accounts() -> Vec<AccountId> {
    vec![
        alice_public().into(),
        bob_public().into(),
        charlie_public().into(),
        dave_public().into(),
        eve_public().into(),
        ferdie_public().into(),
    ]
}

/// Default parachain ID for Paseo testnet
pub const PASEO_PARA_ID: u32 = 5082;

/// Development parachain ID (for local testing)
pub const DEV_PARA_ID: u32 = 2000;

fn testnet_genesis(
    authorities: Vec<AuraId>,
    endowed_accounts: Vec<AccountId>,
    root: AccountId,
    id: ParaId,
) -> Value {
    build_struct_json_patch!(RuntimeGenesisConfig {
        balances: BalancesConfig {
            balances: endowed_accounts
                .iter()
                .cloned()
                .map(|k| (k, 1_000_000 * EXISTENTIAL_DEPOSIT))
                .collect::<Vec<_>>(),
        },
        parachain_info: ParachainInfoConfig { parachain_id: id },
        aura: AuraConfig {
            authorities: authorities,
        },
        polkadot_xcm: PolkadotXcmConfig {
            safe_xcm_version: Some(XCM_VERSION),
        },
        sudo: SudoConfig { key: Some(root) },
    })
}

fn development_config_genesis() -> Value {
    testnet_genesis(
        // Authorities
        vec![
            AuraId::from(sp_core::sr25519::Public::from_raw(alice_public())),
        ],
        // Endowed accounts
        well_known_accounts(),
        // Root key
        alice_public().into(),
        // Para ID
        DEV_PARA_ID.into(),
    )
}

fn local_testnet_genesis() -> Value {
    testnet_genesis(
        // Authorities
        vec![
            AuraId::from(sp_core::sr25519::Public::from_raw(alice_public())),
            AuraId::from(sp_core::sr25519::Public::from_raw(bob_public())),
        ],
        // Endowed accounts
        well_known_accounts(),
        // Root key
        alice_public().into(),
        // Para ID
        DEV_PARA_ID.into(),
    )
}

/// Paseo testnet genesis (production-like config)
fn paseo_genesis() -> Value {
    // For Paseo, you'd replace these with real keys
    testnet_genesis(
        // Authorities - placeholder, will be updated with real collator keys
        vec![
            AuraId::from(sp_core::sr25519::Public::from_raw(alice_public())),
        ],
        // Endowed accounts
        vec![
            alice_public().into(),
        ],
        // Root key (sudo)
        alice_public().into(),
        // Para ID - will be assigned by coretime
        PASEO_PARA_ID.into(),
    )
}

/// Provides the JSON representation of predefined genesis config for given `id`.
pub fn get_preset(id: &PresetId) -> Option<Vec<u8>> {
    let patch = match id.as_ref() {
        sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET => local_testnet_genesis(),
        sp_genesis_builder::DEV_RUNTIME_PRESET => development_config_genesis(),
        "paseo" => paseo_genesis(),
        _ => return None,
    };
    Some(
        serde_json::to_string(&patch)
            .expect("serialization to json is expected to work. qed.")
            .into_bytes(),
    )
}

/// List of supported presets.
pub fn preset_names() -> Vec<PresetId> {
    vec![
        PresetId::from(sp_genesis_builder::DEV_RUNTIME_PRESET),
        PresetId::from(sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET),
        PresetId::from("paseo"),
    ]
}
