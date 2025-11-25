//! compact block builder from full zcash blocks

use crate::error::{Result, ZidecarError};
use crate::zebrad::{ZebradClient, BlockHeader};
use tracing::debug;

/// compact action for trial decryption
#[derive(Debug, Clone)]
pub struct CompactAction {
    pub cmx: Vec<u8>,              // 32 bytes
    pub ephemeral_key: Vec<u8>,    // 32 bytes
    pub ciphertext: Vec<u8>,       // 52 bytes (compact)
    pub nullifier: Vec<u8>,        // 32 bytes
}

/// compact block with only scanning data
#[derive(Debug, Clone)]
pub struct CompactBlock {
    pub height: u32,
    pub hash: Vec<u8>,
    pub actions: Vec<CompactAction>,
}

impl CompactBlock {
    /// build compact block from zebrad
    pub async fn from_zebrad(
        zebrad: &ZebradClient,
        height: u32,
    ) -> Result<Self> {
        let hash_str = zebrad.get_block_hash(height).await?;
        let block = zebrad.get_block(&hash_str, 1).await?;

        let mut actions = Vec::new();

        // fetch transactions and extract orchard actions
        for txid in &block.tx {
            match zebrad.get_raw_transaction(txid).await {
                Ok(tx) => {
                    if let Some(orchard) = tx.orchard {
                        for action in orchard.actions {
                            actions.push(CompactAction {
                                cmx: hex_to_bytes(&action.cmx)?,
                                ephemeral_key: hex_to_bytes(&action.ephemeral_key)?,
                                // take first 52 bytes of encrypted ciphertext
                                ciphertext: hex_to_bytes(&action.enc_ciphertext)?
                                    .into_iter()
                                    .take(52)
                                    .collect(),
                                nullifier: hex_to_bytes(&action.nullifier)?,
                            });
                        }
                    }
                }
                Err(e) => {
                    debug!("failed to fetch tx {}: {}", txid, e);
                    // skip tx if unavailable
                }
            }
        }

        let hash = hex_to_bytes(&hash_str)?;

        Ok(Self {
            height,
            hash,
            actions,
        })
    }

    /// build compact blocks for range
    pub async fn fetch_range(
        zebrad: &ZebradClient,
        start_height: u32,
        end_height: u32,
    ) -> Result<Vec<Self>> {
        let mut blocks = Vec::new();

        for height in start_height..=end_height {
            let block = Self::from_zebrad(zebrad, height).await?;
            blocks.push(block);
        }

        Ok(blocks)
    }
}

fn hex_to_bytes(hex: &str) -> Result<Vec<u8>> {
    hex::decode(hex).map_err(|e| ZidecarError::Serialization(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hex_to_bytes() {
        let hex = "deadbeef";
        let bytes = hex_to_bytes(hex).unwrap();
        assert_eq!(bytes, vec![0xde, 0xad, 0xbe, 0xef]);
    }
}
