// transaction building — ported from zafu-wasm without wasm_bindgen
// supports: shielding (t→z) and orchard spend (z→z, z→t)

use k256::ecdsa::{signature::hazmat::PrehashSigner, SigningKey};
use k256::elliptic_curve::sec1::ToEncodedPoint;
use orchard::builder::{Builder, BundleType};
use orchard::bundle::Flags;
use orchard::keys::{FullViewingKey, Scope, SpendingKey};
use orchard::tree::Anchor;
use orchard::value::NoteValue;
use rand::rngs::OsRng;
use zcash_protocol::value::ZatBalance;

use crate::error::Error;
use crate::key::WalletSeed;

// -- from zafu-wasm --

fn blake2b_256_personal(personalization: &[u8; 16], data: &[u8]) -> [u8; 32] {
    let h = blake2b_simd::Params::new()
        .hash_length(32)
        .personal(personalization)
        .hash(data);
    let mut out = [0u8; 32];
    out.copy_from_slice(h.as_bytes());
    out
}

fn hash160(data: &[u8]) -> [u8; 20] {
    use sha2::Digest;
    let sha = sha2::Sha256::digest(data);
    let ripe = ripemd::Ripemd160::digest(sha);
    let mut out = [0u8; 20];
    out.copy_from_slice(&ripe);
    out
}

fn make_p2pkh_script(pubkey_hash: &[u8; 20]) -> Vec<u8> {
    let mut s = Vec::with_capacity(25);
    s.push(0x76); // OP_DUP
    s.push(0xa9); // OP_HASH160
    s.push(0x14); // push 20 bytes
    s.extend_from_slice(pubkey_hash);
    s.push(0x88); // OP_EQUALVERIFY
    s.push(0xac); // OP_CHECKSIG
    s
}

fn compact_size(n: u64) -> Vec<u8> {
    if n < 0xfd {
        vec![n as u8]
    } else if n <= 0xffff {
        let mut v = vec![0xfd];
        v.extend_from_slice(&(n as u16).to_le_bytes());
        v
    } else if n <= 0xffffffff {
        let mut v = vec![0xfe];
        v.extend_from_slice(&(n as u32).to_le_bytes());
        v
    } else {
        let mut v = vec![0xff];
        v.extend_from_slice(&n.to_le_bytes());
        v
    }
}

fn serialize_orchard_bundle(
    bundle: &orchard::Bundle<orchard::bundle::Authorized, ZatBalance>,
    out: &mut Vec<u8>,
) -> Result<(), Error> {
    let actions = bundle.actions();
    let n = actions.len();

    out.extend_from_slice(&compact_size(n as u64));

    for action in actions.iter() {
        out.extend_from_slice(&action.cv_net().to_bytes());
        out.extend_from_slice(&action.nullifier().to_bytes());
        out.extend_from_slice(&<[u8; 32]>::from(action.rk()));
        out.extend_from_slice(&action.cmx().to_bytes());
        out.extend_from_slice(&action.encrypted_note().epk_bytes);
        out.extend_from_slice(&action.encrypted_note().enc_ciphertext);
        out.extend_from_slice(&action.encrypted_note().out_ciphertext);
    }

    out.push(bundle.flags().to_byte());
    out.extend_from_slice(&bundle.value_balance().to_i64_le_bytes());
    out.extend_from_slice(&bundle.anchor().to_bytes());

    let proof_bytes = bundle.authorization().proof().as_ref();
    out.extend_from_slice(&compact_size(proof_bytes.len() as u64));
    out.extend_from_slice(proof_bytes);

    for action in actions.iter() {
        out.extend_from_slice(&<[u8; 64]>::from(action.authorization()));
    }

    out.extend_from_slice(&<[u8; 64]>::from(
        bundle.authorization().binding_signature(),
    ));

    Ok(())
}

fn compute_orchard_digest<A: orchard::bundle::Authorization>(
    bundle: &orchard::Bundle<A, ZatBalance>,
) -> Result<[u8; 32], Error> {
    let mut compact_data = Vec::new();
    let mut memos_data = Vec::new();
    let mut noncompact_data = Vec::new();

    for action in bundle.actions().iter() {
        compact_data.extend_from_slice(&action.nullifier().to_bytes());
        compact_data.extend_from_slice(&action.cmx().to_bytes());
        let enc = &action.encrypted_note().enc_ciphertext;
        let epk = &action.encrypted_note().epk_bytes;
        compact_data.extend_from_slice(epk);
        compact_data.extend_from_slice(&enc[..52]);

        memos_data.extend_from_slice(&enc[52..564]);

        noncompact_data.extend_from_slice(&action.cv_net().to_bytes());
        noncompact_data.extend_from_slice(&<[u8; 32]>::from(action.rk()));
        noncompact_data.extend_from_slice(&enc[564..580]);
        noncompact_data.extend_from_slice(&action.encrypted_note().out_ciphertext);
    }

    let compact_digest = blake2b_256_personal(b"ZTxIdOrcActCHash", &compact_data);
    let memos_digest = blake2b_256_personal(b"ZTxIdOrcActMHash", &memos_data);
    let noncompact_digest = blake2b_256_personal(b"ZTxIdOrcActNHash", &noncompact_data);

    let mut orchard_data = Vec::new();
    orchard_data.extend_from_slice(&compact_digest);
    orchard_data.extend_from_slice(&memos_digest);
    orchard_data.extend_from_slice(&noncompact_digest);
    orchard_data.push(bundle.flags().to_byte());
    orchard_data.extend_from_slice(&bundle.value_balance().to_i64_le_bytes());
    orchard_data.extend_from_slice(&bundle.anchor().to_bytes());

    Ok(blake2b_256_personal(b"ZTxIdOrchardHash", &orchard_data))
}

// -- transparent UTXO for shielding --

#[derive(Debug, Clone)]
pub struct TransparentUtxo {
    pub txid: String,
    pub vout: u32,
    pub value: u64,
    pub script: String,
}

// -- shielding transaction (t→z) --

pub fn build_shielding_tx(
    seed: &WalletSeed,
    utxos: &[TransparentUtxo],
    recipient_addr: &orchard::Address,
    fee: u64,
    anchor_height: u32,
    mainnet: bool,
) -> Result<Vec<u8>, Error> {
    // derive transparent signing key at m/44'/133'/0'/0/0
    let privkey = crate::address::derive_transparent_key(seed)?;
    let signing_key = SigningKey::from_slice(&privkey)
        .map_err(|e| Error::Transaction(format!("invalid signing key: {}", e)))?;
    let pubkey = k256::PublicKey::from(signing_key.verifying_key());
    let compressed_pubkey = pubkey.to_encoded_point(true);
    let pubkey_bytes = compressed_pubkey.as_bytes();
    let pubkey_hash = hash160(pubkey_bytes);
    let our_script = make_p2pkh_script(&pubkey_hash);

    // sort utxos by value descending, select enough to cover fee
    let mut selected = utxos.to_vec();
    selected.sort_by(|a, b| b.value.cmp(&a.value));

    let total_in: u64 = selected.iter().map(|u| u.value).sum();
    if total_in < fee {
        return Err(Error::InsufficientFunds { have: total_in, need: fee });
    }
    let shielded_value = total_in - fee;

    // build orchard bundle (output only, spends disabled)
    let bundle_type = BundleType::Transactional {
        flags: Flags::SPENDS_DISABLED,
        bundle_required: true,
    };
    let mut builder = Builder::new(bundle_type, Anchor::empty_tree());

    builder.add_output(None, *recipient_addr, NoteValue::from_raw(shielded_value), [0u8; 512])
        .map_err(|e| Error::Transaction(format!("add_output: {:?}", e)))?;

    let mut rng = OsRng;
    let (unauthorized, _meta) = builder
        .build::<ZatBalance>(&mut rng)
        .map_err(|e| Error::Transaction(format!("bundle build: {:?}", e)))?
        .ok_or_else(|| Error::Transaction("builder produced no bundle".into()))?;

    // halo 2 proving
    let pk = orchard::circuit::ProvingKey::build();
    let proven = unauthorized
        .create_proof(&pk, &mut rng)
        .map_err(|e| Error::Transaction(format!("create_proof: {:?}", e)))?;

    // ZIP-244 sighash computation
    let n_inputs = selected.len();
    let branch_id: u32 = 0x4DEC4DF0; // NU6.1
    let expiry_height = anchor_height.saturating_add(100);

    let mut prevout_data = Vec::new();
    let mut sequence_data = Vec::new();
    let mut amounts_data = Vec::new();
    let mut scripts_data = Vec::new();

    for utxo in &selected {
        let txid_be = hex::decode(&utxo.txid)
            .map_err(|_| Error::Transaction("invalid utxo txid hex".into()))?;
        if txid_be.len() != 32 {
            return Err(Error::Transaction("txid must be 32 bytes".into()));
        }
        let mut txid_le = txid_be.clone();
        txid_le.reverse();

        prevout_data.extend_from_slice(&txid_le);
        prevout_data.extend_from_slice(&utxo.vout.to_le_bytes());
        sequence_data.extend_from_slice(&0xffffffffu32.to_le_bytes());
        amounts_data.extend_from_slice(&utxo.value.to_le_bytes());

        let script_bytes = hex::decode(&utxo.script)
            .unwrap_or_else(|_| our_script.clone());
        scripts_data.extend_from_slice(&compact_size(script_bytes.len() as u64));
        scripts_data.extend_from_slice(&script_bytes);
    }

    let header_data = {
        let mut d = Vec::new();
        d.extend_from_slice(&(5u32 | (1u32 << 31)).to_le_bytes());
        d.extend_from_slice(&0x26A7270Au32.to_le_bytes());
        d.extend_from_slice(&branch_id.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes());
        d.extend_from_slice(&expiry_height.to_le_bytes());
        d
    };
    let header_digest = blake2b_256_personal(b"ZTxIdHeadersHash", &header_data);
    let prevouts_digest = blake2b_256_personal(b"ZTxIdPrevoutHash", &prevout_data);
    let sequence_digest = blake2b_256_personal(b"ZTxIdSequencHash", &sequence_data);
    let outputs_digest = blake2b_256_personal(b"ZTxIdOutputsHash", &[]);

    let sapling_digest = blake2b_256_personal(b"ZTxIdSaplingHash", &[]);
    let orchard_digest = compute_orchard_digest(&proven)?;

    let amounts_digest = blake2b_256_personal(b"ZTxTrAmountsHash", &amounts_data);
    let scriptpubkeys_digest = blake2b_256_personal(b"ZTxTrScriptsHash", &scripts_data);

    let sighash_personal = {
        let mut p = [0u8; 16];
        p[..12].copy_from_slice(b"ZcashTxHash_");
        p[12..16].copy_from_slice(&branch_id.to_le_bytes());
        p
    };

    // sign each transparent input
    let mut signed_scripts: Vec<Vec<u8>> = Vec::new();

    for i in 0..n_inputs {
        let utxo = &selected[i];
        let txid_be = hex::decode(&utxo.txid)
            .map_err(|e| Error::Other(format!("bad utxo txid hex: {e}")))?;
        let mut txid_le = txid_be.clone();
        txid_le.reverse();

        let script_bytes = hex::decode(&utxo.script)
            .unwrap_or_else(|_| our_script.clone());

        let mut txin_data = Vec::new();
        txin_data.extend_from_slice(&txid_le);
        txin_data.extend_from_slice(&utxo.vout.to_le_bytes());
        txin_data.extend_from_slice(&utxo.value.to_le_bytes());
        txin_data.extend_from_slice(&compact_size(script_bytes.len() as u64));
        txin_data.extend_from_slice(&script_bytes);
        txin_data.extend_from_slice(&0xffffffffu32.to_le_bytes());

        // ZIP-244 S.2g: hash per-input data separately
        let txin_sig_digest = blake2b_256_personal(b"Zcash___TxInHash", &txin_data);

        let mut sig_input = Vec::new();
        sig_input.push(0x01); // SIGHASH_ALL
        sig_input.extend_from_slice(&prevouts_digest);
        sig_input.extend_from_slice(&amounts_digest);
        sig_input.extend_from_slice(&scriptpubkeys_digest);
        sig_input.extend_from_slice(&sequence_digest);
        sig_input.extend_from_slice(&outputs_digest);
        sig_input.extend_from_slice(&txin_sig_digest);

        let transparent_sig_digest = blake2b_256_personal(b"ZTxIdTranspaHash", &sig_input);

        let mut sighash_input = Vec::new();
        sighash_input.extend_from_slice(&header_digest);
        sighash_input.extend_from_slice(&transparent_sig_digest);
        sighash_input.extend_from_slice(&sapling_digest);
        sighash_input.extend_from_slice(&orchard_digest);

        let sighash = blake2b_256_personal(&sighash_personal, &sighash_input);

        let sig: k256::ecdsa::Signature = signing_key.sign_prehash(&sighash)
            .map_err(|e| Error::Transaction(format!("ECDSA signing: {}", e)))?;
        let sig_der = sig.to_der();

        let mut script_sig = Vec::new();
        script_sig.push((sig_der.as_bytes().len() + 1) as u8);
        script_sig.extend_from_slice(sig_der.as_bytes());
        script_sig.push(0x01); // SIGHASH_ALL
        script_sig.push(pubkey_bytes.len() as u8);
        script_sig.extend_from_slice(pubkey_bytes);

        signed_scripts.push(script_sig);
    }

    // apply orchard binding signature
    // ZIP-244 S.2: when vin is non-empty, the verifier uses transparent_sig_digest
    // (not the txid transparent_digest) for the sighash. For the binding signature
    // (SignableInput::Shielded), hash_type=SIGHASH_ALL, no per-input data.
    let txin_sig_digest_empty = blake2b_256_personal(b"Zcash___TxInHash", &[]);
    let binding_transparent_digest = {
        let mut d = Vec::new();
        d.push(0x01); // SIGHASH_ALL
        d.extend_from_slice(&prevouts_digest);
        d.extend_from_slice(&amounts_digest);
        d.extend_from_slice(&scriptpubkeys_digest);
        d.extend_from_slice(&sequence_digest);
        d.extend_from_slice(&outputs_digest);
        d.extend_from_slice(&txin_sig_digest_empty);
        blake2b_256_personal(b"ZTxIdTranspaHash", &d)
    };

    let txid_sighash = {
        let mut d = Vec::new();
        d.extend_from_slice(&header_digest);
        d.extend_from_slice(&binding_transparent_digest);
        d.extend_from_slice(&sapling_digest);
        d.extend_from_slice(&orchard_digest);
        blake2b_256_personal(&sighash_personal, &d)
    };

    let authorized = proven
        .apply_signatures(&mut rng, txid_sighash, &[])
        .map_err(|e| Error::Transaction(format!("apply_signatures: {:?}", e)))?;

    // serialize v5 transaction
    let mut tx = Vec::new();

    // header
    tx.extend_from_slice(&(5u32 | (1u32 << 31)).to_le_bytes());
    tx.extend_from_slice(&0x26A7270Au32.to_le_bytes());
    tx.extend_from_slice(&branch_id.to_le_bytes());
    tx.extend_from_slice(&0u32.to_le_bytes()); // nLockTime
    tx.extend_from_slice(&expiry_height.to_le_bytes());

    // transparent inputs
    tx.extend_from_slice(&compact_size(n_inputs as u64));
    for (i, utxo) in selected.iter().enumerate() {
        let txid_be = hex::decode(&utxo.txid)
            .map_err(|e| Error::Other(format!("bad utxo txid hex: {e}")))?;
        let mut txid_le = txid_be.clone();
        txid_le.reverse();
        tx.extend_from_slice(&txid_le);
        tx.extend_from_slice(&utxo.vout.to_le_bytes());
        tx.extend_from_slice(&compact_size(signed_scripts[i].len() as u64));
        tx.extend_from_slice(&signed_scripts[i]);
        tx.extend_from_slice(&0xffffffffu32.to_le_bytes());
    }

    // transparent outputs (none)
    tx.extend_from_slice(&compact_size(0));

    // sapling (none)
    tx.extend_from_slice(&compact_size(0));
    tx.extend_from_slice(&compact_size(0));

    // orchard bundle
    serialize_orchard_bundle(&authorized, &mut tx)?;

    Ok(tx)
}

// -- orchard spend transaction (z→t, z→z) --

pub fn build_orchard_spend_tx(
    seed: &WalletSeed,
    spends: &[(orchard::Note, orchard::tree::MerklePath)],
    t_outputs: &[(String, u64)],                          // z→t: (t-address, amount)
    z_outputs: &[(orchard::Address, u64, [u8; 512])],     // z→z: (addr, amount, memo)
    fee: u64,
    anchor: Anchor,
    anchor_height: u32,
    mainnet: bool,
) -> Result<Vec<u8>, Error> {
    let coin_type = if mainnet { 133 } else { 1 };
    let sk = SpendingKey::from_zip32_seed(seed.as_bytes(), coin_type, zip32::AccountId::ZERO)
        .map_err(|_| Error::Transaction("failed to derive spending key".into()))?;
    let fvk = FullViewingKey::from(&sk);
    let ask = orchard::keys::SpendAuthorizingKey::from(&sk);

    // compute change
    let total_in: u64 = spends.iter().map(|(n, _)| n.value().inner()).sum();
    let total_t: u64 = t_outputs.iter().map(|(_, v)| *v).sum();
    let total_z: u64 = z_outputs.iter().map(|(_, v, _)| *v).sum();
    let total_out = total_t + total_z + fee;
    if total_in < total_out {
        return Err(Error::InsufficientFunds { have: total_in, need: total_out });
    }
    let change = total_in - total_out;

    // build orchard bundle
    let bundle_type = BundleType::Transactional {
        flags: Flags::ENABLED,
        bundle_required: true,
    };
    let mut builder = Builder::new(bundle_type, anchor);

    let n_spends = spends.len();
    for (note, path) in spends {
        builder.add_spend(fvk.clone(), note.clone(), path.clone())
            .map_err(|e| Error::Transaction(format!("add_spend: {:?}", e)))?;
    }

    // z→z outputs
    for (addr, amount, memo) in z_outputs {
        let ovk = Some(fvk.to_ovk(Scope::External));
        builder.add_output(ovk, *addr, NoteValue::from_raw(*amount), *memo)
            .map_err(|e| Error::Transaction(format!("add_output: {:?}", e)))?;
    }

    // change output (back to self, internal scope)
    if change > 0 {
        let change_addr = fvk.address_at(0u64, Scope::Internal);
        let ovk = Some(fvk.to_ovk(Scope::Internal));
        builder.add_output(ovk, change_addr, NoteValue::from_raw(change), [0u8; 512])
            .map_err(|e| Error::Transaction(format!("add_output (change): {:?}", e)))?;
    }

    let mut rng = OsRng;
    let (unauthorized, _meta) = builder
        .build::<ZatBalance>(&mut rng)
        .map_err(|e| Error::Transaction(format!("bundle build: {:?}", e)))?
        .ok_or_else(|| Error::Transaction("builder produced no bundle".into()))?;

    // halo 2 proving (rayon parallelism is automatic via halo2's multicore feature)
    let pk = orchard::circuit::ProvingKey::build();
    let proven = unauthorized
        .create_proof(&pk, &mut rng)
        .map_err(|e| Error::Transaction(format!("create_proof: {:?}", e)))?;

    // serialize transparent outputs for z→t
    let t_output_scripts: Vec<Vec<u8>> = t_outputs.iter().map(|(addr, _)| {
        decode_t_address_script(addr, mainnet)
    }).collect::<Result<_, _>>()?;

    // ZIP-244 sighash
    let branch_id: u32 = 0x4DEC4DF0; // NU6.1
    let expiry_height = anchor_height.saturating_add(100);

    let header_data = {
        let mut d = Vec::new();
        d.extend_from_slice(&(5u32 | (1u32 << 31)).to_le_bytes());
        d.extend_from_slice(&0x26A7270Au32.to_le_bytes());
        d.extend_from_slice(&branch_id.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes());
        d.extend_from_slice(&expiry_height.to_le_bytes());
        d
    };
    let header_digest = blake2b_256_personal(b"ZTxIdHeadersHash", &header_data);

    let transparent_digest = if t_outputs.is_empty() {
        blake2b_256_personal(b"ZTxIdTranspaHash", &[])
    } else {
        let prevouts_digest = blake2b_256_personal(b"ZTxIdPrevoutHash", &[]);
        let sequence_digest = blake2b_256_personal(b"ZTxIdSequencHash", &[]);
        let mut outputs_data = Vec::new();
        for (i, (_, amount)) in t_outputs.iter().enumerate() {
            outputs_data.extend_from_slice(&amount.to_le_bytes());
            outputs_data.extend_from_slice(&compact_size(t_output_scripts[i].len() as u64));
            outputs_data.extend_from_slice(&t_output_scripts[i]);
        }
        let outputs_digest = blake2b_256_personal(b"ZTxIdOutputsHash", &outputs_data);
        let mut d = Vec::new();
        d.extend_from_slice(&prevouts_digest);
        d.extend_from_slice(&sequence_digest);
        d.extend_from_slice(&outputs_digest);
        blake2b_256_personal(b"ZTxIdTranspaHash", &d)
    };

    let sapling_digest = blake2b_256_personal(b"ZTxIdSaplingHash", &[]);
    let orchard_digest = compute_orchard_digest(&proven)?;

    let sighash_personal = {
        let mut p = [0u8; 16];
        p[..12].copy_from_slice(b"ZcashTxHash_");
        p[12..16].copy_from_slice(&branch_id.to_le_bytes());
        p
    };
    let sighash = {
        let mut d = Vec::new();
        d.extend_from_slice(&header_digest);
        d.extend_from_slice(&transparent_digest);
        d.extend_from_slice(&sapling_digest);
        d.extend_from_slice(&orchard_digest);
        blake2b_256_personal(&sighash_personal, &d)
    };

    // sign orchard spends
    let signing_keys: Vec<orchard::keys::SpendAuthorizingKey> =
        (0..n_spends).map(|_| ask.clone()).collect();
    let authorized = proven
        .apply_signatures(&mut rng, sighash, &signing_keys)
        .map_err(|e| Error::Transaction(format!("apply_signatures: {:?}", e)))?;

    // serialize v5 transaction
    let mut tx = Vec::new();

    // header
    tx.extend_from_slice(&(5u32 | (1u32 << 31)).to_le_bytes());
    tx.extend_from_slice(&0x26A7270Au32.to_le_bytes());
    tx.extend_from_slice(&branch_id.to_le_bytes());
    tx.extend_from_slice(&0u32.to_le_bytes()); // nLockTime
    tx.extend_from_slice(&expiry_height.to_le_bytes());

    // transparent inputs (none for orchard spend)
    tx.extend_from_slice(&compact_size(0));

    // transparent outputs
    if t_outputs.is_empty() {
        tx.extend_from_slice(&compact_size(0));
    } else {
        tx.extend_from_slice(&compact_size(t_outputs.len() as u64));
        for (i, (_, amount)) in t_outputs.iter().enumerate() {
            tx.extend_from_slice(&amount.to_le_bytes());
            tx.extend_from_slice(&compact_size(t_output_scripts[i].len() as u64));
            tx.extend_from_slice(&t_output_scripts[i]);
        }
    }

    // sapling (none)
    tx.extend_from_slice(&compact_size(0));
    tx.extend_from_slice(&compact_size(0));

    // orchard bundle
    serialize_orchard_bundle(&authorized, &mut tx)?;

    Ok(tx)
}

/// decode a t-address to a P2PKH scriptPubKey
fn decode_t_address_script(addr: &str, mainnet: bool) -> Result<Vec<u8>, Error> {
    let decoded = base58_decode(addr)
        .map_err(|_| Error::Address(format!("invalid base58 in t-address: {}", addr)))?;
    let expected = if mainnet { [0x1c, 0xb8] } else { [0x1d, 0x25] };
    if decoded.len() != 22 || decoded[..2] != expected {
        return Err(Error::Address(format!("invalid transparent address: {}", addr)));
    }
    let mut pkh = [0u8; 20];
    pkh.copy_from_slice(&decoded[2..]);
    Ok(make_p2pkh_script(&pkh))
}

/// base58check decode (returns version + payload, checksum verified)
fn base58_decode(s: &str) -> Result<Vec<u8>, Error> {
    const ALPHABET: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

    // decode base58 to big integer (as byte vec)
    let mut num: Vec<u8> = vec![0];
    for &c in s.as_bytes() {
        let val = ALPHABET.iter().position(|&a| a == c)
            .ok_or_else(|| Error::Address("invalid base58 character".into()))? as u32;
        let mut carry = val;
        for byte in num.iter_mut().rev() {
            carry += (*byte as u32) * 58;
            *byte = (carry & 0xff) as u8;
            carry >>= 8;
        }
        while carry > 0 {
            num.insert(0, (carry & 0xff) as u8);
            carry >>= 8;
        }
    }

    // leading '1's → leading 0x00 bytes
    let leading = s.bytes().take_while(|&b| b == b'1').count();
    // strip leading zeros from num
    let start = num.iter().position(|&b| b != 0).unwrap_or(num.len());
    let mut result = vec![0u8; leading];
    result.extend_from_slice(&num[start..]);

    // verify checksum (last 4 bytes)
    if result.len() < 4 {
        return Err(Error::Address("base58check too short".into()));
    }
    let (payload, checksum) = result.split_at(result.len() - 4);
    use sha2::Digest;
    let hash = sha2::Sha256::digest(sha2::Sha256::digest(payload));
    if &hash[..4] != checksum {
        return Err(Error::Address("base58check checksum mismatch".into()));
    }
    Ok(payload.to_vec())
}

/// parse an orchard address from a unified address string (from zafu-wasm)
pub fn parse_orchard_address(addr_str: &str, mainnet: bool) -> Result<orchard::Address, Error> {
    use zcash_keys::address::Address as ZkAddress;
    use zcash_protocol::consensus::{MainNetwork, TestNetwork};

    let decoded = if mainnet {
        ZkAddress::decode(&MainNetwork, addr_str)
    } else {
        ZkAddress::decode(&TestNetwork, addr_str)
    };

    match decoded {
        Some(ZkAddress::Unified(ua)) => {
            let orchard_addr = ua.orchard()
                .ok_or_else(|| Error::Address("unified address has no orchard receiver".into()))?;
            let raw_bytes = orchard_addr.to_raw_address_bytes();
            Option::from(orchard::Address::from_raw_address_bytes(&raw_bytes))
                .ok_or_else(|| Error::Address("invalid orchard address bytes".into()))
        }
        Some(_) => Err(Error::Address("not a unified address".into())),
        None => Err(Error::Address("failed to decode address".into())),
    }
}

/// derive the orchard recipient address from seed (for self-shielding)
pub fn self_shielding_address(seed: &WalletSeed, mainnet: bool) -> Result<orchard::Address, Error> {
    let coin_type = if mainnet { 133 } else { 1 };
    let sk = SpendingKey::from_zip32_seed(seed.as_bytes(), coin_type, zip32::AccountId::ZERO)
        .map_err(|_| Error::Address("failed to derive spending key".into()))?;
    let fvk = FullViewingKey::from(&sk);
    Ok(fvk.address_at(0u64, Scope::External))
}
