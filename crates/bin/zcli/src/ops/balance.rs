use crate::address;
use crate::client::ZidecarClient;
use crate::error::Error;
use crate::key::WalletSeed;
use crate::wallet::Wallet;

#[derive(Debug, serde::Serialize)]
pub struct Balance {
    pub transparent: u64,
    pub shielded: u64,
    pub total: u64,
}

pub async fn get_balance(
    seed: &WalletSeed,
    endpoint: &str,
    mainnet: bool,
) -> Result<Balance, Error> {
    let taddr = address::transparent_address(seed, mainnet)?;

    let mut client = ZidecarClient::connect(endpoint).await?;

    // transparent balance from UTXOs
    let utxos = client.get_address_utxos(vec![taddr]).await?;
    let transparent: u64 = utxos.iter().map(|u| u.value_zat).sum();

    // shielded balance from local wallet
    let wallet = Wallet::open(&Wallet::default_path())?;
    let (shielded, _notes) = wallet.shielded_balance()?;

    Ok(Balance {
        transparent,
        shielded,
        total: transparent + shielded,
    })
}
