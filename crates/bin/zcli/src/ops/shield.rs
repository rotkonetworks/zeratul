use crate::address;
use crate::client::ZidecarClient;
use crate::error::Error;
use crate::key::WalletSeed;
use crate::tx;

const MARGINAL_FEE: u64 = 5_000;
const GRACE_ACTIONS: usize = 2;
const MIN_ORCHARD_ACTIONS: usize = 2;

/// ZIP-317 fee for shielding: 0 spends, 1 output (padded to MIN=2), plus transparent inputs
fn compute_shield_fee(n_t_inputs: usize) -> u64 {
    let n_orchard_actions = MIN_ORCHARD_ACTIONS; // max(0, 1, 2) = 2
    let logical_actions = n_orchard_actions + n_t_inputs;
    MARGINAL_FEE * logical_actions.max(GRACE_ACTIONS) as u64
}

pub async fn shield(
    seed: &WalletSeed,
    endpoint: &str,
    fee_override: Option<u64>,
    mainnet: bool,
    script: bool,
) -> Result<(), Error> {
    let taddr = address::transparent_address(seed, mainnet)?;

    let mut client = ZidecarClient::connect(endpoint).await?;

    // fetch UTXOs
    let utxos = client.get_address_utxos(vec![taddr.clone()]).await?;
    if utxos.is_empty() {
        return Err(Error::Transaction("no transparent UTXOs to shield".into()));
    }

    let fee = fee_override.unwrap_or_else(|| compute_shield_fee(utxos.len()));
    let total: u64 = utxos.iter().map(|u| u.value_zat).sum();
    if total <= fee {
        return Err(Error::InsufficientFunds { have: total, need: fee });
    }

    // get current tip for expiry
    let (tip, _) = client.get_tip().await?;

    // convert to tx builder format
    let tx_utxos: Vec<tx::TransparentUtxo> = utxos.iter().map(|u| {
        tx::TransparentUtxo {
            txid: hex::encode(u.txid),
            vout: u.output_index,
            value: u.value_zat,
            script: hex::encode(&u.script),
        }
    }).collect();

    // recipient is our own orchard address
    let recipient = tx::self_shielding_address(seed, mainnet)?;

    if !script {
        eprintln!("shielding {:.8} ZEC ({} UTXOs, fee {:.8} ZEC)",
            (total - fee) as f64 / 1e8,
            tx_utxos.len(),
            fee as f64 / 1e8,
        );
        eprintln!("building transaction (halo 2 proving, this takes a moment)...");
    }

    let tx_bytes = tx::build_shielding_tx(
        seed, &tx_utxos, &recipient, fee, tip, mainnet,
    )?;

    // broadcast
    let result = client.send_transaction(tx_bytes).await?;

    if script {
        println!("{}", serde_json::json!({
            "txid": result.txid,
            "shielded_zat": total - fee,
            "fee_zat": fee,
            "success": result.is_success(),
            "error": result.error_message,
        }));
    } else if result.is_success() {
        println!("txid: {}", result.txid);
    } else {
        return Err(Error::Transaction(format!(
            "broadcast failed ({}): {}", result.error_code, result.error_message
        )));
    }

    Ok(())
}
