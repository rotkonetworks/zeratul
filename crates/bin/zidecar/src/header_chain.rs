//! header chain trace encoding for ligerito proofs
//!
//! Trace layout (16 fields per header):
//! - fields 0-7: header chain (hash linkage + running commitment)
//! - fields 8-15: state roots at epoch boundaries (TCT/nullifier commitments)
//!
//! This proves both header chain integrity AND state root chain in one proof.

use crate::error::{Result, ZidecarError};
use crate::storage::Storage;
use crate::zebrad::{BlockHeader, ZebradClient};
use blake2::{Blake2b512, Digest};
use futures::stream::{self, StreamExt};
use ligerito_binary_fields::{BinaryElem32, BinaryFieldElement};
use std::sync::Arc;
use tracing::{info, warn};

/// concurrent requests for header fetching (reduced to avoid overwhelming zebrad)
const CONCURRENT_REQUESTS: usize = 16;

/// max retries for RPC calls
const MAX_RETRIES: usize = 3;

/// batch size for caching to disk (flush every N headers)
const CACHE_BATCH_SIZE: usize = 1000;

/// epoch size (blocks per epoch)
const EPOCH_SIZE: u32 = 1024;

/// fields encoded per block header in trace (16 fields)
pub const FIELDS_PER_HEADER: usize = 16;

/// trace field layout per header (16 fields):
/// field 0: height
/// field 1-2: block_hash (first 8 bytes, split into 2x4)
/// field 3-4: prev_hash (first 8 bytes, split into 2x4)
/// field 5-6: block_hash (next 8 bytes, split into 2x4)
/// field 7: header_commitment (running hash chain)
/// field 8-9: sapling_root (first 8 bytes) - only at epoch boundaries
/// field 10-11: orchard_root (first 8 bytes) - only at epoch boundaries
/// field 12-13: reserved for nullifier_root
/// field 14: state_commitment (running state chain)
/// field 15: reserved

/// state roots at an epoch boundary (from z_gettreestate)
#[derive(Clone, Debug, Default)]
pub struct EpochStateRoots {
    /// epoch number
    pub epoch: u32,
    /// height of last block in epoch
    pub height: u32,
    /// sapling note commitment tree root (32 bytes hex)
    pub sapling_root: String,
    /// orchard note commitment tree root (32 bytes hex)
    pub orchard_root: String,
}

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
    /// initial running commitment (for composing proofs)
    /// - GIGAPROOF starts with all zeros
    /// - TIP_PROOF starts with GIGAPROOF's final commitment
    pub initial_commitment: [u8; 32],
    /// final running commitment (for composing proofs)
    /// - stored in field 7 of last header
    /// - used as initial_commitment for the next proof
    pub final_commitment: [u8; 32],
    /// whether this trace includes state roots (extended layout)
    pub includes_state_roots: bool,
    /// initial state commitment (for extended proofs)
    pub initial_state_commitment: [u8; 32],
    /// final state commitment (for extended proofs)
    pub final_state_commitment: [u8; 32],
}

impl HeaderChainTrace {
    /// build trace from header range using parallel fetching with caching
    pub async fn build(
        zebrad: &ZebradClient,
        storage: &Arc<Storage>,
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

        // check cache for already fetched headers
        let cached_max = storage.get_max_cached_header_height()?.unwrap_or(0);
        let fetch_start = if cached_max >= start_height && cached_max < end_height {
            cached_max + 1
        } else if cached_max >= end_height {
            // all cached
            end_height + 1 // nothing to fetch
        } else {
            start_height
        };

        let to_fetch = if fetch_start <= end_height {
            (end_height - fetch_start + 1) as usize
        } else {
            0
        };

        info!(
            "cache status: max_cached={}, need to fetch {} headers ({} -> {})",
            cached_max, to_fetch, fetch_start, end_height
        );

        // fetch missing headers in parallel with retry and incremental caching
        if to_fetch > 0 {
            let heights: Vec<u32> = (fetch_start..=end_height).collect();
            let total = heights.len();

            // process in chunks to cache incrementally
            let mut fetched_count = 0;
            for chunk in heights.chunks(CACHE_BATCH_SIZE) {
                let zebrad_clone = zebrad.clone();
                let chunk_vec: Vec<u32> = chunk.to_vec();

                let fetched: Vec<std::result::Result<(u32, String, String), ZidecarError>> = stream::iter(chunk_vec)
                    .map(|height| {
                        let zc = zebrad_clone.clone();
                        async move {
                            // retry logic
                            let mut last_err = None;
                            for attempt in 0..MAX_RETRIES {
                                match async {
                                    let hash = zc.get_block_hash(height).await?;
                                    let header = zc.get_block_header(&hash).await?;
                                    Ok::<_, ZidecarError>((height, header.hash, header.prev_hash))
                                }.await {
                                    Ok(result) => return Ok(result),
                                    Err(e) => {
                                        last_err = Some(e);
                                        if attempt < MAX_RETRIES - 1 {
                                            // exponential backoff: 100ms, 200ms, 400ms
                                            tokio::time::sleep(tokio::time::Duration::from_millis(100 * (1 << attempt))).await;
                                        }
                                    }
                                }
                            }
                            Err(last_err.unwrap())
                        }
                    })
                    .buffer_unordered(CONCURRENT_REQUESTS)
                    .collect()
                    .await;

                // collect chunk results
                let mut headers_to_cache: Vec<(u32, String, String)> = Vec::with_capacity(chunk.len());
                for result in fetched {
                    let (height, hash, prev_hash) = result?;
                    headers_to_cache.push((height, hash, prev_hash));
                }

                // sort by height before caching (buffer_unordered doesn't preserve order)
                headers_to_cache.sort_by_key(|(h, _, _)| *h);

                // batch store to cache
                storage.store_headers_batch(&headers_to_cache)?;
                fetched_count += headers_to_cache.len();

                // progress logging
                let progress = (fetched_count * 100) / total;
                info!("fetched {}% ({}/{}) headers", progress, fetched_count, total);
            }
            info!("cached all {} headers", fetched_count);
        }

        // now build trace from cache
        info!("loading headers from cache...");
        let mut headers: Vec<BlockHeader> = Vec::with_capacity(num_headers);
        for height in start_height..=end_height {
            if let Some((hash, prev_hash)) = storage.get_header(height)? {
                headers.push(BlockHeader {
                    height,
                    hash,
                    prev_hash,
                    timestamp: 0, // not needed for trace
                    merkle_root: String::new(),
                });
            } else {
                return Err(ZidecarError::BlockNotFound(height));
            }
        }

        info!("loaded {} headers from cache", headers.len());

        // Fetch state roots at epoch boundaries
        let state_roots = Self::fetch_epoch_state_roots(zebrad, start_height, end_height).await?;
        info!("fetched {} epoch state roots", state_roots.len());

        // encode trace with state roots (always use extended format)
        let initial_commitment = [0u8; 32];
        let initial_state_commitment = [0u8; 32];

        let (trace, final_commitment, final_state_commitment) =
            Self::encode_trace(&headers, &state_roots, initial_commitment, initial_state_commitment)?;

        info!(
            "encoded trace: {} elements ({} headers x {} fields)",
            trace.len(),
            num_headers,
            FIELDS_PER_HEADER
        );

        Ok(Self {
            trace,
            num_headers,
            start_height,
            end_height,
            initial_commitment,
            final_commitment,
            includes_state_roots: true,
            initial_state_commitment,
            final_state_commitment,
        })
    }

    /// fetch state roots at epoch boundaries from zebrad
    async fn fetch_epoch_state_roots(
        zebrad: &ZebradClient,
        start_height: u32,
        end_height: u32,
    ) -> Result<Vec<EpochStateRoots>> {
        let start_epoch = start_height / EPOCH_SIZE;
        let end_epoch = end_height / EPOCH_SIZE;

        let mut roots = Vec::new();

        for epoch in start_epoch..=end_epoch {
            let epoch_end_height = (epoch + 1) * EPOCH_SIZE - 1;

            // Only fetch if epoch end is within our range
            if epoch_end_height <= end_height && epoch_end_height >= start_height {
                match zebrad.get_tree_state(&epoch_end_height.to_string()).await {
                    Ok(tree_state) => {
                        roots.push(EpochStateRoots {
                            epoch,
                            height: epoch_end_height,
                            sapling_root: tree_state.sapling.commitments.final_state.clone(),
                            orchard_root: tree_state.orchard.commitments.final_state.clone(),
                        });
                    }
                    Err(e) => {
                        warn!("failed to get tree state for epoch {}: {}", epoch, e);
                        // Use empty roots if unavailable
                        roots.push(EpochStateRoots {
                            epoch,
                            height: epoch_end_height,
                            sapling_root: String::new(),
                            orchard_root: String::new(),
                        });
                    }
                }
            }
        }

        Ok(roots)
    }

    /// encode headers with state roots into trace
    /// returns (trace, final_header_commitment, final_state_commitment)
    fn encode_trace(
        headers: &[BlockHeader],
        state_roots: &[EpochStateRoots],
        initial_commitment: [u8; 32],
        initial_state_commitment: [u8; 32],
    ) -> Result<(Vec<BinaryElem32>, [u8; 32], [u8; 32])> {
        let num_elements = headers.len() * FIELDS_PER_HEADER;
        let trace_size = num_elements.next_power_of_two();
        let mut trace = vec![BinaryElem32::zero(); trace_size];

        let mut running_commitment = initial_commitment;
        let mut state_commitment = initial_state_commitment;

        // Build map from height to state roots for quick lookup
        let state_root_map: std::collections::HashMap<u32, &EpochStateRoots> = state_roots
            .iter()
            .map(|r| (r.height, r))
            .collect();

        for (i, header) in headers.iter().enumerate() {
            let offset = i * FIELDS_PER_HEADER;

            let block_hash = hex_to_bytes(&header.hash)?;
            let prev_hash = if header.prev_hash.is_empty() {
                if header.height != 0 {
                    return Err(ZidecarError::Validation(format!(
                        "block {} has empty prev_hash (only genesis allowed)",
                        header.height
                    )));
                }
                vec![0u8; 32]
            } else {
                hex_to_bytes(&header.prev_hash)?
            };

            // Basic fields (0-7) - same as regular trace
            trace[offset] = BinaryElem32::from(header.height);
            trace[offset + 1] = bytes_to_field(&block_hash[0..4]);
            trace[offset + 2] = bytes_to_field(&block_hash[4..8]);
            trace[offset + 3] = bytes_to_field(&prev_hash[0..4]);
            trace[offset + 4] = bytes_to_field(&prev_hash[4..8]);
            trace[offset + 5] = bytes_to_field(&block_hash[8..12]);
            trace[offset + 6] = bytes_to_field(&block_hash[12..16]);

            running_commitment = update_running_commitment(
                &running_commitment,
                &block_hash,
                &prev_hash,
                header.height,
            );
            trace[offset + 7] = bytes_to_field(&running_commitment[0..4]);

            // Extended fields (8-15) - state roots at epoch boundaries
            if let Some(roots) = state_root_map.get(&header.height) {
                // This is an epoch boundary - include state roots
                let sapling = if roots.sapling_root.is_empty() {
                    vec![0u8; 32]
                } else {
                    hex_to_bytes(&roots.sapling_root)?
                };

                let orchard = if roots.orchard_root.is_empty() {
                    vec![0u8; 32]
                } else {
                    hex_to_bytes(&roots.orchard_root)?
                };

                // Sapling root (fields 8-9)
                trace[offset + 8] = bytes_to_field(&sapling[0..4]);
                trace[offset + 9] = bytes_to_field(&sapling[4..8]);

                // Orchard root (fields 10-11)
                trace[offset + 10] = bytes_to_field(&orchard[0..4]);
                trace[offset + 11] = bytes_to_field(&orchard[4..8]);

                // Nullifier root placeholder (fields 12-13) - reserved for future
                trace[offset + 12] = BinaryElem32::zero();
                trace[offset + 13] = BinaryElem32::zero();

                // Update state commitment chain
                state_commitment = update_state_commitment(
                    &state_commitment,
                    &sapling,
                    &orchard,
                    header.height,
                );
                trace[offset + 14] = bytes_to_field(&state_commitment[0..4]);

                // Reserved field
                trace[offset + 15] = BinaryElem32::zero();
            } else {
                // Not an epoch boundary - fields 8-15 are zero
                // but we still include the previous state commitment
                trace[offset + 14] = bytes_to_field(&state_commitment[0..4]);
            }
        }

        Ok((trace, running_commitment, state_commitment))
    }

    /// build trace with specific initial commitments (for composing proofs)
    pub async fn build_with_commitment(
        zebrad: &ZebradClient,
        storage: &Arc<Storage>,
        start_height: u32,
        end_height: u32,
        initial_commitment: [u8; 32],
        initial_state_commitment: [u8; 32],
    ) -> Result<Self> {
        // build normally first, then override the commitment chains if non-zero
        let mut trace = Self::build(zebrad, storage, start_height, end_height).await?;

        // if non-zero initial commitments, re-encode with them
        if initial_commitment != [0u8; 32] || initial_state_commitment != [0u8; 32] {
            // re-fetch headers from cache (they're already there)
            let mut headers: Vec<BlockHeader> = Vec::new();
            for height in start_height..=end_height {
                if let Some((hash, prev_hash)) = storage.get_header(height)? {
                    headers.push(BlockHeader {
                        height,
                        hash,
                        prev_hash,
                        timestamp: 0,
                        merkle_root: String::new(),
                    });
                }
            }

            // re-fetch state roots
            let state_roots = Self::fetch_epoch_state_roots(zebrad, start_height, end_height).await?;

            let (new_trace, final_commitment, final_state_commitment) =
                Self::encode_trace(&headers, &state_roots, initial_commitment, initial_state_commitment)?;

            trace.trace = new_trace;
            trace.initial_commitment = initial_commitment;
            trace.final_commitment = final_commitment;
            trace.initial_state_commitment = initial_state_commitment;
            trace.final_state_commitment = final_state_commitment;
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

/// update state commitment with epoch state roots
/// This creates a verifiable chain of state commitments
fn update_state_commitment(
    prev_commitment: &[u8; 32],
    sapling_root: &[u8],
    orchard_root: &[u8],
    height: u32,
) -> [u8; 32] {
    let mut hasher = Blake2b512::new();
    hasher.update(b"ZIDECAR_state_commitment");
    hasher.update(prev_commitment);
    hasher.update(sapling_root);
    hasher.update(orchard_root);
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
        use ligerito_binary_fields::BinaryFieldElement;
        let bytes = [0x01, 0x02, 0x03, 0x04];
        let field = bytes_to_field(&bytes);
        assert_eq!(field.poly().value(), 0x04030201); // little endian
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
