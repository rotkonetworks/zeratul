//! orchard transaction builder
//! builds, signs, and submits transactions to zebrad

use anyhow::{anyhow, Result};
use orchard::{
    builder::{Builder, BundleType},
    bundle::Authorized,
    keys::{FullViewingKey, OutgoingViewingKey, Scope, SpendingKey},
    note::Note,
    tree::{Anchor, MerklePath},
    value::NoteValue,
    Address,
};
use rand_core::OsRng;
use zip32::AccountId;

/// spendable note with merkle path
pub struct SpendableNote {
    pub note: Note,
    pub merkle_path: MerklePath,
    pub fvk: FullViewingKey,
}

/// transaction request
pub struct TransferRequest {
    pub recipient: Address,
    pub amount: u64,        // zatoshis
    pub memo: Option<[u8; 512]>,
}

/// transaction builder for orchard
pub struct OrchardTxBuilder {
    spending_key: SpendingKey,
    fvk: FullViewingKey,
    ovk: OutgoingViewingKey,
}

impl OrchardTxBuilder {
    /// create new builder from seed phrase and account index
    pub fn from_seed(seed: &[u8; 64], account_index: u32) -> Result<Self> {
        let account = AccountId::try_from(account_index)
            .map_err(|_| anyhow!("invalid account index"))?;

        let sk = SpendingKey::from_zip32_seed(seed, 133, account)
            .map_err(|_| anyhow!("key derivation failed"))?;

        let fvk = FullViewingKey::from(&sk);
        let ovk = fvk.to_ovk(Scope::External);

        Ok(Self {
            spending_key: sk,
            fvk,
            ovk,
        })
    }

    /// build a transaction
    pub fn build_transfer(
        &self,
        spendable_notes: Vec<SpendableNote>,
        request: TransferRequest,
        anchor: Anchor,
    ) -> Result<Vec<u8>> {
        // calculate total available
        let total_available: u64 = spendable_notes.iter()
            .map(|n| n.note.value().inner())
            .sum();

        if request.amount > total_available {
            return Err(anyhow!("insufficient funds: have {} need {}", total_available, request.amount));
        }

        // create builder
        let mut builder = Builder::new(BundleType::DEFAULT, anchor);

        // add spends (notes being consumed)
        for sn in spendable_notes {
            builder.add_spend(sn.fvk, sn.note, sn.merkle_path)
                .map_err(|e| anyhow!("add spend failed: {:?}", e))?;
        }

        // add output to recipient
        let output_value = NoteValue::from_raw(request.amount);
        builder.add_output(
            Some(self.ovk.clone()),
            request.recipient,
            output_value,
            request.memo,
        ).map_err(|e| anyhow!("add output failed: {:?}", e))?;

        // add change output if needed
        let change = total_available.saturating_sub(request.amount);
        if change > 0 {
            let change_addr = self.fvk.address_at(0u32, Scope::Internal);
            let change_value = NoteValue::from_raw(change);
            builder.add_output(
                Some(self.ovk.clone()),
                change_addr,
                change_value,
                None,
            ).map_err(|e| anyhow!("add change output failed: {:?}", e))?;
        }

        // build unsigned bundle
        let (unauthorized, _meta) = builder.build(&mut OsRng)
            .map_err(|e| anyhow!("bundle build failed: {:?}", e))?
            .ok_or_else(|| anyhow!("empty bundle"))?;

        // create proof (requires proving key)
        // NOTE: proving key generation is expensive - should be cached
        let pk = orchard::circuit::ProvingKey::build();
        let proven = unauthorized.create_proof(&pk, &mut OsRng)
            .map_err(|e| anyhow!("proof creation failed: {:?}", e))?;

        // apply signatures
        let sighash = [0u8; 32]; // TODO: compute actual sighash from tx
        let signed = proven.apply_signatures(&mut OsRng, sighash, &[orchard::keys::SpendAuthorizingKey::from(&self.spending_key)])
            .map_err(|e| anyhow!("signing failed: {:?}", e))?;

        // serialize bundle
        // NOTE: this is just the orchard bundle, needs to be wrapped in full transaction
        let bundle_bytes = serialize_bundle(&signed)?;

        Ok(bundle_bytes)
    }

    /// get the default receiving address
    pub fn default_address(&self) -> Address {
        self.fvk.address_at(0u32, Scope::External)
    }
}

/// serialize authorized bundle (simplified)
fn serialize_bundle(bundle: &orchard::bundle::Bundle<Authorized, i64>) -> Result<Vec<u8>> {
    // TODO: proper serialization per zcash protocol
    // for now just return placeholder
    let _ = bundle;
    Ok(vec![])
}

/// JSON-RPC client for zebrad
pub struct ZebradRpc {
    url: String,
    client: reqwest::blocking::Client,
}

impl ZebradRpc {
    pub fn new(url: &str) -> Self {
        Self {
            url: url.to_string(),
            client: reqwest::blocking::Client::new(),
        }
    }

    /// submit raw transaction
    pub fn send_raw_transaction(&self, tx_hex: &str) -> Result<String> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendrawtransaction",
            "params": [tx_hex]
        });

        let resp = self.client
            .post(&self.url)
            .json(&body)
            .send()
            .map_err(|e| anyhow!("rpc request failed: {}", e))?;

        let result: serde_json::Value = resp.json()
            .map_err(|e| anyhow!("rpc response parse failed: {}", e))?;

        if let Some(err) = result.get("error") {
            if !err.is_null() {
                return Err(anyhow!("rpc error: {}", err));
            }
        }

        result.get("result")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("no txid in response"))
    }

    /// get blockchain info
    pub fn get_blockchain_info(&self) -> Result<serde_json::Value> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getblockchaininfo",
            "params": []
        });

        let resp = self.client
            .post(&self.url)
            .json(&body)
            .send()
            .map_err(|e| anyhow!("rpc request failed: {}", e))?;

        let result: serde_json::Value = resp.json()
            .map_err(|e| anyhow!("rpc response parse failed: {}", e))?;

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_from_seed() {
        // test seed (DO NOT USE IN PRODUCTION)
        let seed = [0u8; 64];
        let builder = OrchardTxBuilder::from_seed(&seed, 0);
        assert!(builder.is_ok());
    }
}
