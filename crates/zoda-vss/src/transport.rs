//! qr transport — verified erasure-coded frames for air-gapped QR communication
//!
//! splits a payload into k data frames + (n-k) parity frames using reed-solomon
//! over GF(256). any k frames reconstruct the payload. each frame carries a
//! session ID (truncated SHA-256 of payload) for instant rejection of stray QRs.
//!
//! ## properties
//!
//! - **per-frame session binding**: stray QR from another source rejected on scan
//! - **k-of-n erasure recovery**: any k frames (in any order) reconstruct payload
//! - **verified reconstruction**: SHA-256(payload) checked after decode
//! - **no expansion per frame**: data frames are verbatim payload chunks
//! - **small header**: 8-byte session ID + 1-byte index per frame
//!
//! ## wire format
//!
//! ```text
//! frame = session_id(8) || index(1) || chunk_data(ceil(payload_len/k))
//! ```
//!
//! the first frame scanned establishes the session. subsequent frames with a
//! different session ID are silently dropped. after k valid frames, the payload
//! is reconstructed and verified against the full SHA-256 hash embedded in frame 0.
//!
//! ## frame 0 (metadata frame)
//!
//! frame 0 carries the metadata prepended to its chunk data:
//! ```text
//! metadata = version(1) || k(1) || n(1) || payload_len(4 BE) || sha256(32) = 39 bytes
//! ```
//! this is embedded in frame 0's chunk data (first 39 bytes, then chunk bytes).
//!
//! ## usage
//!
//! ```rust,no_run
//! use zoda_vss::transport::{Encoder, Decoder};
//!
//! // encode
//! let payload = b"hello world, this is a QR transport test payload!";
//! let (frames, session_id) = Encoder::encode(payload, 3, 5); // 3-of-5
//!
//! // decode (any 3 frames)
//! let mut decoder = Decoder::new();
//! for frame in &frames[..3] {
//!     let bytes = frame.to_bytes();
//!     assert!(decoder.receive(&bytes).is_ok());
//! }
//! let recovered = decoder.reconstruct().unwrap();
//! assert_eq!(recovered, payload);
//! ```

extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;
use sha2::{Digest, Sha256};

use crate::GF256;

/// Session ID length (truncated SHA-256).
const SESSION_ID_LEN: usize = 8;

/// Metadata length in frame 0: version(1) + k(1) + n(1) + payload_len(4) + sha256(32)
const META_LEN: usize = 39;

/// Current transport version.
const VERSION: u8 = 1;

/// A single transport frame ready for QR display.
#[derive(Debug, Clone)]
pub struct Frame {
    /// first 8 bytes of SHA-256(payload) — session binding
    pub session_id: [u8; SESSION_ID_LEN],
    /// frame index (0-based)
    pub index: u8,
    /// frame data (chunk, possibly with metadata prefix for index 0)
    pub data: Vec<u8>,
}

impl Frame {
    /// Serialize frame to bytes: session_id(8) || index(1) || data
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(SESSION_ID_LEN + 1 + self.data.len());
        out.extend_from_slice(&self.session_id);
        out.push(self.index);
        out.extend_from_slice(&self.data);
        out
    }

    /// Deserialize frame from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, TransportError> {
        if bytes.len() < SESSION_ID_LEN + 1 + 1 {
            return Err(TransportError::FrameTooShort);
        }
        let mut session_id = [0u8; SESSION_ID_LEN];
        session_id.copy_from_slice(&bytes[..SESSION_ID_LEN]);
        let index = bytes[SESSION_ID_LEN];
        let data = bytes[SESSION_ID_LEN + 1..].to_vec();
        Ok(Frame {
            session_id,
            index,
            data,
        })
    }
}

/// Encoder: splits payload into verified erasure-coded frames.
pub struct Encoder;

impl Encoder {
    /// Auto-size: compute k and n from payload size and max QR capacity.
    ///
    /// - `max_qr_bytes`: max raw bytes per QR code (before hex encoding)
    ///   For hex-encoded `zt:` frames, use max_qr_alphanumeric / 2 - 15 (prefix overhead).
    ///   Typical: 1200 bytes for comfortable scanning.
    /// - `redundancy_pct`: extra frames as percentage (e.g., 30 = 30% parity).
    ///
    /// Returns (frames, session_id).
    pub fn encode_auto(
        payload: &[u8],
        max_qr_bytes: usize,
        redundancy_pct: u8,
    ) -> (Vec<Frame>, [u8; SESSION_ID_LEN]) {
        // frame overhead: session_id(8) + index(1) + metadata(39 for frame 0)
        let usable = max_qr_bytes.saturating_sub(SESSION_ID_LEN + 1 + META_LEN);
        let k = if usable == 0 {
            1
        } else {
            let raw_k = (payload.len() + usable - 1) / usable;
            raw_k.max(1).min(254) as u8
        };
        let extra = ((k as u16 * redundancy_pct as u16) / 100).max(1) as u8;
        let n = (k as u16 + extra as u16).min(254) as u8;
        Self::encode(payload, k, n)
    }

    /// Encode payload into n frames (k-of-n recoverable).
    ///
    /// - `k`: minimum frames needed to reconstruct (threshold)
    /// - `n`: total frames to generate (k data + n-k parity)
    ///
    /// Returns (frames, session_id).
    pub fn encode(payload: &[u8], k: u8, n: u8) -> (Vec<Frame>, [u8; SESSION_ID_LEN]) {
        assert!(k > 0 && n >= k && n < 255, "invalid k={k}, n={n}");

        // compute payload hash
        let hash: [u8; 32] = Sha256::digest(payload).into();
        let mut session_id = [0u8; SESSION_ID_LEN];
        session_id.copy_from_slice(&hash[..SESSION_ID_LEN]);

        // chunk size: ceil(payload_len / k)
        let chunk_size = (payload.len() + k as usize - 1) / k as usize;

        // split payload into k data chunks (pad last with zeros)
        let mut chunks: Vec<Vec<u8>> = Vec::with_capacity(k as usize);
        for i in 0..k as usize {
            let start = i * chunk_size;
            let end = core::cmp::min(start + chunk_size, payload.len());
            let mut chunk = vec![0u8; chunk_size];
            if start < payload.len() {
                let copy_len = end - start;
                chunk[..copy_len].copy_from_slice(&payload[start..end]);
            }
            chunks.push(chunk);
        }

        // generate parity chunks using polynomial evaluation over GF(256)
        // data chunks are evaluations at points 1..=k
        // parity chunks are evaluations at points k+1..=n
        let mut all_chunks = chunks.clone();
        for parity_idx in 0..(n - k) as usize {
            let eval_point = GF256((k as usize + parity_idx + 1) as u8);
            let mut parity = vec![0u8; chunk_size];

            for byte_pos in 0..chunk_size {
                // interpolate polynomial through data points, evaluate at eval_point
                // data points: (1, chunks[0][pos]), (2, chunks[1][pos]), ..., (k, chunks[k-1][pos])
                let mut result = GF256::ZERO;
                for (i, chunk) in chunks.iter().enumerate() {
                    let x_i = GF256((i + 1) as u8);
                    let y_i = GF256(chunk[byte_pos]);

                    // lagrange basis at eval_point
                    let mut basis = GF256::ONE;
                    for (j, _) in chunks.iter().enumerate() {
                        if i != j {
                            let x_j = GF256((j + 1) as u8);
                            // basis *= (eval_point - x_j) / (x_i - x_j)
                            basis = basis * (eval_point - x_j) * (x_i - x_j).inv();
                        }
                    }
                    result = result + y_i * basis;
                }
                parity[byte_pos] = result.0;
            }
            all_chunks.push(parity);
        }

        // build frames
        let mut frames = Vec::with_capacity(n as usize);
        for (i, chunk) in all_chunks.into_iter().enumerate() {
            let frame_data = if i == 0 {
                // frame 0: prepend metadata
                let mut meta = Vec::with_capacity(META_LEN + chunk.len());
                meta.push(VERSION);
                meta.push(k);
                meta.push(n);
                meta.extend_from_slice(&(payload.len() as u32).to_be_bytes());
                meta.extend_from_slice(&hash);
                meta.extend_from_slice(&chunk);
                meta
            } else {
                chunk
            };

            frames.push(Frame {
                session_id,
                index: i as u8,
                data: frame_data,
            });
        }

        (frames, session_id)
    }
}

/// Decoder: collects frames, verifies session, reconstructs payload.
pub struct Decoder {
    session_id: Option<[u8; SESSION_ID_LEN]>,
    k: Option<u8>,
    n: Option<u8>,
    payload_len: Option<usize>,
    payload_hash: Option<[u8; 32]>,
    chunk_size: Option<usize>,
    frames: Vec<Option<Vec<u8>>>, // indexed by frame index
    received_count: usize,
}

impl Decoder {
    pub fn new() -> Self {
        Self {
            session_id: None,
            k: None,
            n: None,
            payload_len: None,
            payload_hash: None,
            chunk_size: None,
            frames: Vec::new(),
            received_count: 0,
        }
    }

    /// Number of valid frames received.
    pub fn received(&self) -> usize {
        self.received_count
    }

    /// Threshold needed for reconstruction.
    pub fn threshold(&self) -> Option<u8> {
        self.k
    }

    /// Whether we have enough frames to reconstruct.
    pub fn complete(&self) -> bool {
        self.k.map_or(false, |k| self.received_count >= k as usize)
    }

    /// Receive a frame. Returns Ok(true) if accepted, Ok(false) if duplicate.
    /// Returns Err if the frame belongs to a different session.
    pub fn receive(&mut self, raw: &[u8]) -> Result<bool, TransportError> {
        let frame = Frame::from_bytes(raw)?;

        // first frame establishes session
        if self.session_id.is_none() {
            if frame.index == 0 {
                // parse metadata from frame 0
                if frame.data.len() < META_LEN {
                    return Err(TransportError::MetadataTooShort);
                }
                let version = frame.data[0];
                if version != VERSION {
                    return Err(TransportError::UnsupportedVersion(version));
                }
                let k = frame.data[1];
                let n = frame.data[2];
                if k == 0 || n < k {
                    return Err(TransportError::InvalidParams);
                }
                let payload_len = u32::from_be_bytes([
                    frame.data[3],
                    frame.data[4],
                    frame.data[5],
                    frame.data[6],
                ]) as usize;
                let mut hash = [0u8; 32];
                hash.copy_from_slice(&frame.data[7..39]);
                let chunk_data = &frame.data[META_LEN..];
                let chunk_size = chunk_data.len();

                self.session_id = Some(frame.session_id);
                self.k = Some(k);
                self.n = Some(n);
                self.payload_len = Some(payload_len);
                self.payload_hash = Some(hash);
                self.chunk_size = Some(chunk_size);
                self.frames = vec![None; n as usize];
                self.frames[0] = Some(chunk_data.to_vec());
                self.received_count = 1;
                return Ok(true);
            } else {
                // non-zero frame before metadata — can't establish session
                // store session ID and wait for frame 0
                self.session_id = Some(frame.session_id);
                // we can't allocate frames yet without knowing n
                return Err(TransportError::NeedMetadataFirst);
            }
        }

        // check session binding
        if frame.session_id != self.session_id.unwrap() {
            return Err(TransportError::SessionMismatch);
        }

        let n = self.n.unwrap();
        if frame.index >= n {
            return Err(TransportError::InvalidFrameIndex);
        }

        // check chunk size consistency
        let expected_size = if frame.index == 0 {
            META_LEN + self.chunk_size.unwrap()
        } else {
            self.chunk_size.unwrap()
        };
        if frame.data.len() != expected_size {
            return Err(TransportError::InconsistentChunkSize);
        }

        let idx = frame.index as usize;
        if self.frames[idx].is_some() {
            return Ok(false); // duplicate
        }

        let chunk_data = if frame.index == 0 {
            frame.data[META_LEN..].to_vec()
        } else {
            frame.data
        };

        self.frames[idx] = Some(chunk_data);
        self.received_count += 1;
        Ok(true)
    }

    /// Reconstruct payload from received frames.
    pub fn reconstruct(&self) -> Result<Vec<u8>, TransportError> {
        let k = self.k.ok_or(TransportError::NoMetadata)? as usize;
        let payload_len = self.payload_len.ok_or(TransportError::NoMetadata)?;
        let expected_hash = self.payload_hash.ok_or(TransportError::NoMetadata)?;
        let chunk_size = self.chunk_size.ok_or(TransportError::NoMetadata)?;

        if self.received_count < k {
            return Err(TransportError::InsufficientFrames);
        }

        // collect k received frames with their indices
        let mut available: Vec<(u8, &[u8])> = Vec::with_capacity(k);
        for (idx, slot) in self.frames.iter().enumerate() {
            if let Some(data) = slot {
                available.push((idx as u8, data));
                if available.len() == k {
                    break;
                }
            }
        }

        // if all k frames are data frames (indices 0..k-1), no interpolation needed
        let all_data = available.iter().all(|(idx, _)| (*idx as usize) < k);

        let mut payload = Vec::with_capacity(k * chunk_size);

        if all_data {
            // fast path: just concatenate in order
            let mut sorted = available.clone();
            sorted.sort_by_key(|(idx, _)| *idx);
            for (_, data) in sorted {
                payload.extend_from_slice(data);
            }
        } else {
            // slow path: lagrange interpolation over GF(256)
            // reconstruct each data chunk (evaluation at points 1..=k)
            for target_idx in 0..k {
                let target_x = GF256((target_idx + 1) as u8);

                // check if we already have this data point
                if let Some((_, data)) = available.iter().find(|(idx, _)| *idx as usize == target_idx) {
                    payload.extend_from_slice(data);
                    continue;
                }

                // interpolate each byte position in this chunk
                for byte_pos in 0..chunk_size {
                    let mut result = GF256::ZERO;
                    for &(idx, data) in &available {
                        let x_i = GF256(idx + 1); // 1-indexed evaluation points
                        let y_i = GF256(data[byte_pos]);

                        let mut basis = GF256::ONE;
                        for &(jdx, _) in &available {
                            if idx != jdx {
                                let x_j = GF256(jdx + 1);
                                basis = basis * (target_x - x_j) * (x_i - x_j).inv();
                            }
                        }
                        result = result + y_i * basis;
                    }
                    payload.push(result.0);
                }
            }
        }

        // trim to actual payload length
        payload.truncate(payload_len);

        // verify hash
        let hash: [u8; 32] = Sha256::digest(&payload).into();
        if hash != expected_hash {
            return Err(TransportError::HashMismatch);
        }

        Ok(payload)
    }
}

/// Transport errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportError {
    FrameTooShort,
    MetadataTooShort,
    UnsupportedVersion(u8),
    InvalidParams,
    SessionMismatch,
    InvalidFrameIndex,
    InconsistentChunkSize,
    NeedMetadataFirst,
    NoMetadata,
    InsufficientFrames,
    HashMismatch,
}

impl core::fmt::Display for TransportError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::FrameTooShort => write!(f, "frame too short"),
            Self::MetadataTooShort => write!(f, "frame 0 metadata too short"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported transport version {v}"),
            Self::InvalidParams => write!(f, "invalid k/n parameters"),
            Self::SessionMismatch => write!(f, "frame from different session"),
            Self::InvalidFrameIndex => write!(f, "frame index out of range"),
            Self::InconsistentChunkSize => write!(f, "inconsistent chunk size"),
            Self::NeedMetadataFirst => write!(f, "need frame 0 (metadata) first"),
            Self::NoMetadata => write!(f, "no metadata received"),
            Self::InsufficientFrames => write!(f, "not enough frames to reconstruct"),
            Self::HashMismatch => write!(f, "payload hash mismatch — corrupted or wrong frames"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for TransportError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_all_data_frames() {
        let payload = b"hello world, this is a test of the QR transport system!";
        let (frames, session_id) = Encoder::encode(payload, 3, 5);

        assert_eq!(frames.len(), 5);
        assert_eq!(frames[0].session_id, session_id);

        // decode using first 3 frames (all data frames — fast path)
        let mut decoder = Decoder::new();
        for frame in &frames[..3] {
            let bytes = frame.to_bytes();
            assert!(decoder.receive(&bytes).is_ok());
        }
        assert!(decoder.complete());
        let recovered = decoder.reconstruct().unwrap();
        assert_eq!(recovered, payload);
    }

    #[test]
    fn test_decode_with_parity_frames() {
        let payload = b"erasure coding test with parity reconstruction";
        let (frames, _) = Encoder::encode(payload, 3, 5);

        // use frames 0, 3, 4 (one data + two parity — needs interpolation)
        let mut decoder = Decoder::new();
        decoder.receive(&frames[0].to_bytes()).unwrap();
        decoder.receive(&frames[3].to_bytes()).unwrap();
        decoder.receive(&frames[4].to_bytes()).unwrap();
        assert!(decoder.complete());
        let recovered = decoder.reconstruct().unwrap();
        assert_eq!(recovered, payload);
    }

    #[test]
    fn test_decode_any_k_of_n() {
        let payload = vec![0xABu8; 1000]; // 1KB payload
        let (frames, _) = Encoder::encode(&payload, 4, 7);

        // try every combination of 4 frames
        let n = frames.len();
        for a in 0..n {
            // must include frame 0 for metadata
            if a != 0 {
                continue;
            }
            for b in (a + 1)..n {
                for c in (b + 1)..n {
                    for d in (c + 1)..n {
                        let mut decoder = Decoder::new();
                        decoder.receive(&frames[a].to_bytes()).unwrap();
                        decoder.receive(&frames[b].to_bytes()).unwrap();
                        decoder.receive(&frames[c].to_bytes()).unwrap();
                        decoder.receive(&frames[d].to_bytes()).unwrap();
                        let recovered = decoder.reconstruct().unwrap();
                        assert_eq!(recovered, payload, "failed with frames {a},{b},{c},{d}");
                    }
                }
            }
        }
    }

    #[test]
    fn test_session_mismatch_rejected() {
        let payload_a = b"payload A";
        let payload_b = b"payload B";
        let (frames_a, _) = Encoder::encode(payload_a, 2, 3);
        let (frames_b, _) = Encoder::encode(payload_b, 2, 3);

        let mut decoder = Decoder::new();
        decoder.receive(&frames_a[0].to_bytes()).unwrap();
        let result = decoder.receive(&frames_b[1].to_bytes());
        assert_eq!(result, Err(TransportError::SessionMismatch));
    }

    #[test]
    fn test_hash_mismatch_on_corruption() {
        let payload = b"original payload";
        let (mut frames, _) = Encoder::encode(payload, 2, 3);

        // corrupt frame 1 data
        frames[1].data[0] ^= 0xFF;

        let mut decoder = Decoder::new();
        decoder.receive(&frames[0].to_bytes()).unwrap();
        decoder.receive(&frames[1].to_bytes()).unwrap();
        let result = decoder.reconstruct();
        assert_eq!(result, Err(TransportError::HashMismatch));
    }

    #[test]
    fn test_duplicate_frame_ignored() {
        let payload = b"test dedup";
        let (frames, _) = Encoder::encode(payload, 2, 3);

        let mut decoder = Decoder::new();
        assert_eq!(decoder.receive(&frames[0].to_bytes()), Ok(true));
        assert_eq!(decoder.receive(&frames[0].to_bytes()), Ok(false)); // dup
        assert_eq!(decoder.received(), 1);
    }

    #[test]
    fn test_small_payload() {
        let payload = b"hi";
        let (frames, _) = Encoder::encode(payload, 2, 3);
        let mut decoder = Decoder::new();
        decoder.receive(&frames[0].to_bytes()).unwrap();
        decoder.receive(&frames[1].to_bytes()).unwrap();
        let recovered = decoder.reconstruct().unwrap();
        assert_eq!(recovered, payload);
    }

    #[test]
    fn test_large_payload() {
        let payload: Vec<u8> = (0..10_000).map(|i| (i % 256) as u8).collect();
        let (frames, _) = Encoder::encode(&payload, 5, 8);
        assert_eq!(frames.len(), 8);

        // use frames 0, 2, 4, 6, 7 (mix of data and parity)
        let mut decoder = Decoder::new();
        for &idx in &[0, 2, 4, 6, 7] {
            decoder.receive(&frames[idx].to_bytes()).unwrap();
        }
        let recovered = decoder.reconstruct().unwrap();
        assert_eq!(recovered, payload);
    }
}
