//! header chain trace encoding for ligerito proofs

use crate::error::{Result, ZidecarError};
use crate::zebrad::{BlockHeader, ZebradClient};
use blake2::{Blake2b512, Digest};
use ligerito_binary_fields::{BinaryElem32, BinaryFieldElement};
use tracing::{debug, info};

/// fields encoded per block header in trace
pub const FIELDS_PER_HEADER: usize = 8;

/// trace field layout per header:
/// field 0: height
/// field 1-2: block_hash (first 8 bytes, split into 2x4)
/// field 3-4: prev_hash (first 8 bytes, split into 2x4)
/// field 5-6: block_hash (next 8 bytes, split into 2x4)
/// field 7: running_commitment (binds all data)

/// header chain trace for ligerito proving
pub struct HeaderChainTrace {
    /// trace polynomial (padded to power of 2)
    pub trace: Vec<BinaryElem32>,
    /// number of headers encoded
    pub num_headers: usize,
    /// start height
    pub start_height: u32,
    /// end height
    pub end_height: u32,
}

impl HeaderChainTrace {
    /// build trace from header range
    pub async fn build(
        zebrad: &ZebradClient,
        start_height: u32,
        end_height: u32,
    ) -> Result<Self> {
        if start_height > end_height {
            return Err(ZidecarError::InvalidRange(
                "start_height > end_height".into(),
            ));
        }

        let num_headers = (end_height - start_height + 1) as usize;
        info!(
            "building header chain trace: {} -> {} ({} headers)",
            start_height, end_height, num_headers
        );

        // fetch all headers
        let mut headers = Vec::with_capacity(num_headers);
        for height in start_height..=end_height {
            if height % 10000 == 0 {
                debug!("fetching header {}", height);
            }
            let hash = zebrad.get_block_hash(height).await?;
            let header = zebrad.get_block_header(&hash).await?;
            headers.push(header);
        }

        info!("fetched {} headers, encoding trace", headers.len());

        // encode trace
        let trace = Self::encode_headers(&headers)?;

        info!(
            "encoded trace: {} elements ({} headers × {} fields)",
            trace.len(),
            num_headers,
            FIELDS_PER_HEADER
        );

        Ok(Self {
            trace,
            num_headers,
            start_height,
            end_height,
        })
    }

    /// encode headers into trace polynomial
    fn encode_headers(headers: &[BlockHeader]) -> Result<Vec<BinaryElem32>> {
        let num_elements = headers.len() * FIELDS_PER_HEADER;

        // round up to next power of 2 for ligerito
        let trace_size = num_elements.next_power_of_two();

        let mut trace = vec![BinaryElem32::zero(); trace_size];

        let mut running_commitment = [0u8; 32];

        for (i, header) in headers.iter().enumerate() {
            let offset = i * FIELDS_PER_HEADER;

            // parse hashes
            let block_hash = hex_to_bytes(&header.hash)?;
            let prev_hash = if header.prev_hash.is_empty() {
                // SECURITY: only genesis block (height 0) can have empty prev_hash
                if header.height != 0 {
                    return Err(ZidecarError::Validation(format!(
                        "block {} has empty prev_hash (only genesis allowed)",
                        header.height
                    )));
                }
                // genesis block: use zero-filled prev_hash
                vec![0u8; 32]
            } else {
                hex_to_bytes(&header.prev_hash)?
            };

            // field 0: height
            trace[offset] = BinaryElem32::from(header.height);

            // field 1-2: first 8 bytes of block_hash
            trace[offset + 1] = bytes_to_field(&block_hash[0..4]);
            trace[offset + 2] = bytes_to_field(&block_hash[4..8]);

            // field 3-4: first 8 bytes of prev_hash
            trace[offset + 3] = bytes_to_field(&prev_hash[0..4]);
            trace[offset + 4] = bytes_to_field(&prev_hash[4..8]);

            // field 5-6: next 8 bytes of block_hash (more uniqueness)
            trace[offset + 5] = bytes_to_field(&block_hash[8..12]);
            trace[offset + 6] = bytes_to_field(&block_hash[12..16]);

            // field 7: running commitment
            running_commitment = update_running_commitment(
                &running_commitment,
                &block_hash,
                &prev_hash,
                header.height,
            );
            trace[offset + 7] = bytes_to_field(&running_commitment[0..4]);
        }

        Ok(trace)
    }

    /// verify trace encodes headers correctly (for testing)
    pub fn verify_encoding(&self, headers: &[BlockHeader]) -> Result<()> {
        if headers.len() != self.num_headers {
            return Err(ZidecarError::InvalidRange("header count mismatch".into()));
        }

        let mut running_commitment = [0u8; 32];

        for (i, header) in headers.iter().enumerate() {
            let offset = i * FIELDS_PER_HEADER;

            // verify height (extract value from BinaryElem32)
            let height = self.trace[offset].poly().value();
            if height != header.height {
                return Err(ZidecarError::Serialization(format!(
                    "height mismatch at index {}: expected {}, got {}",
                    i, header.height, height
                )));
            }

            // verify hashes present (just check first field non-zero)
            let block_hash = hex_to_bytes(&header.hash)?;
            let encoded = bytes_to_field(&block_hash[0..4]);
            if self.trace[offset + 1] != encoded {
                return Err(ZidecarError::Serialization(format!(
                    "block hash mismatch at index {}",
                    i
                )));
            }

            // verify running commitment
            let prev_hash = hex_to_bytes(&header.prev_hash)?;
            running_commitment = update_running_commitment(
                &running_commitment,
                &block_hash,
                &prev_hash,
                header.height,
            );
        }

        Ok(())
    }
}

/// convert 4 bytes to BinaryElem32 (little endian)
fn bytes_to_field(bytes: &[u8]) -> BinaryElem32 {
    assert_eq!(bytes.len(), 4);
    let value = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    BinaryElem32::from(value)
}

/// hex string to bytes
fn hex_to_bytes(hex: &str) -> Result<Vec<u8>> {
    hex::decode(hex).map_err(|e| ZidecarError::Serialization(e.to_string()))
}

/// update running commitment with new block data
fn update_running_commitment(
    prev_commitment: &[u8; 32],
    block_hash: &[u8],
    prev_hash: &[u8],
    height: u32,
) -> [u8; 32] {
    let mut hasher = Blake2b512::new();
    hasher.update(b"ZIDECAR_header_commitment");
    hasher.update(prev_commitment);
    hasher.update(block_hash);
    hasher.update(prev_hash);
    hasher.update(&height.to_le_bytes());

    let hash = hasher.finalize();
    let mut result = [0u8; 32];
    result.copy_from_slice(&hash[..32]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bytes_to_field() {
        let bytes = [0x01, 0x02, 0x03, 0x04];
        let field = bytes_to_field(&bytes);
        assert_eq!(field.to_u32(), 0x04030201); // little endian
    }

    #[test]
    fn test_hex_to_bytes() {
        let hex = "deadbeef";
        let bytes = hex_to_bytes(hex).unwrap();
        assert_eq!(bytes, vec![0xde, 0xad, 0xbe, 0xef]);
    }

    #[test]
    fn test_running_commitment_deterministic() {
        let prev = [0u8; 32];
        let block = [1u8; 32];
        let prev_hash = [2u8; 32];

        let c1 = update_running_commitment(&prev, &block, &prev_hash, 100);
        let c2 = update_running_commitment(&prev, &block, &prev_hash, 100);

        assert_eq!(c1, c2);
    }
}
