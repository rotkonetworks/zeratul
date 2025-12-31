//! P2P Escrow Contract with VSS Verification
//!
//! Minimal Revive/PolkaVM contract for cross-chain escrow using
//! verifiable Shamir secret sharing.
//!
//! Flow:
//! 1. Seller creates escrow with commitment (Merkle root of shares)
//! 2. Buyer verifies their share off-chain against commitment
//! 3. Seller funds escrow on target chain (Zcash/Penumbra)
//! 4. Buyer sends fiat
//! 5a. Happy: Seller confirms, sends their share to buyer
//! 5b. Dispute: Arbitrators vote, chain reveals share_C to winner

#![feature(alloc_error_handler)]
#![no_main]
#![no_std]
#![allow(static_mut_refs)]

use uapi::{HostFn, HostFnImpl as api, ReturnFlags, StorageFlags};

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe {
        core::arch::asm!("unimp");
        core::hint::unreachable_unchecked();
    }
}

// Minimal bump allocator
mod alloc_support {
    use core::{
        alloc::{GlobalAlloc, Layout},
        sync::atomic::{AtomicUsize, Ordering},
    };

    pub struct BumpAllocator {
        offset: AtomicUsize,
    }

    const HEAP_SIZE: usize = 64 * 1024;

    #[link_section = ".bss.heap"]
    static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];

    unsafe impl GlobalAlloc for BumpAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let align = layout.align().max(8);
            let size = layout.size();
            let mut offset = self.offset.load(Ordering::Relaxed);
            loop {
                let aligned = (offset + align - 1) & !(align - 1);
                if aligned + size > HEAP_SIZE {
                    return core::ptr::null_mut();
                }
                match self.offset.compare_exchange_weak(
                    offset,
                    aligned + size,
                    Ordering::SeqCst,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        let heap_ptr = HEAP.as_ptr() as *mut u8;
                        return heap_ptr.add(aligned);
                    }
                    Err(o) => offset = o,
                }
            }
        }
        unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
    }

    #[global_allocator]
    static GLOBAL: BumpAllocator = BumpAllocator {
        offset: AtomicUsize::new(0),
    };

    #[alloc_error_handler]
    fn alloc_error(_layout: Layout) -> ! {
        unsafe {
            core::arch::asm!("unimp");
            core::hint::unreachable_unchecked();
        }
    }
}

// ============================================================================
// BINARY FIELD GF(2^32) ARITHMETIC
// ============================================================================

/// Binary field element in GF(2^32) using polynomial representation
/// Irreducible polynomial: x^32 + x^7 + x^3 + x^2 + 1
#[derive(Clone, Copy, PartialEq, Eq)]
struct BF32(u32);

impl BF32 {
    const ZERO: Self = Self(0);
    const ONE: Self = Self(1);

    /// Irreducible polynomial for GF(2^32): x^32 + x^7 + x^3 + x^2 + 1
    const IRREDUCIBLE: u32 = 0x8D;

    #[inline(always)]
    fn add(self, other: Self) -> Self {
        Self(self.0 ^ other.0)
    }

    #[inline(always)]
    fn mul(self, other: Self) -> Self {
        let mut a = self.0 as u64;
        let mut b = other.0 as u64;
        let mut result: u64 = 0;

        while b != 0 {
            if b & 1 != 0 {
                result ^= a;
            }
            a <<= 1;
            if a & (1 << 32) != 0 {
                a ^= (1 << 32) | (Self::IRREDUCIBLE as u64);
            }
            b >>= 1;
        }

        Self(result as u32)
    }

    fn from_le_bytes(bytes: [u8; 4]) -> Self {
        Self(u32::from_le_bytes(bytes))
    }

    fn to_le_bytes(self) -> [u8; 4] {
        self.0.to_le_bytes()
    }
}

// ============================================================================
// MERKLE TREE VERIFICATION
// ============================================================================

fn keccak256(data: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    api::hash_keccak_256(data, &mut out);
    out
}

fn hash_share_values(values: &[BF32; 8]) -> [u8; 32] {
    let mut data = [0u8; 32];
    for (i, v) in values.iter().enumerate() {
        data[i * 4..(i + 1) * 4].copy_from_slice(&v.to_le_bytes());
    }
    keccak256(&data)
}

fn verify_merkle_proof_slice(
    root: &[u8; 32],
    leaf_hash: &[u8; 32],
    proof: &[[u8; 32]],
    index: usize,
) -> bool {
    let mut current = *leaf_hash;
    let mut idx = index;

    for sibling in proof {
        let mut combined = [0u8; 64];
        if idx % 2 == 0 {
            combined[..32].copy_from_slice(&current);
            combined[32..].copy_from_slice(sibling);
        } else {
            combined[..32].copy_from_slice(sibling);
            combined[32..].copy_from_slice(&current);
        }
        current = keccak256(&combined);
        idx /= 2;
    }

    current == *root
}

fn verify_share(
    commitment: &[u8; 32],
    share_index: usize,
    share_values: &[BF32; 8],
    merkle_proof: &[[u8; 32]],
) -> bool {
    let leaf_hash = hash_share_values(share_values);
    verify_merkle_proof_slice(commitment, &leaf_hash, merkle_proof, share_index)
}

// ============================================================================
// STORAGE LAYOUT
// ============================================================================

const ESCROW_PREFIX: u8 = 0x10;
const SHARE_PREFIX: u8 = 0x20;

// Escrow states
const STATE_CREATED: u8 = 0;
const STATE_BUYER_CONFIRMED: u8 = 1;
const STATE_PAYMENT_SENT: u8 = 2;
const STATE_COMPLETED: u8 = 3;
const STATE_DISPUTED: u8 = 4;
const STATE_RESOLVED_BUYER: u8 = 5;
const STATE_RESOLVED_SELLER: u8 = 6;

/// Escrow storage layout:
/// Key: ESCROW_PREFIX || escrow_id (32 bytes)
/// Value:
///   - state: u8 (offset 0)
///   - commitment: [u8; 32] (offset 1)
///   - escrow_pubkey: [u8; 32] (offset 33)
///   - seller: [u8; 20] (offset 65)
///   - buyer: [u8; 20] (offset 85) - only after confirmation
///   - created_block: u64 (offset 105) - for indexing

fn escrow_key(escrow_id: &[u8; 32]) -> [u8; 33] {
    let mut key = [0u8; 33];
    key[0] = ESCROW_PREFIX;
    key[1..33].copy_from_slice(escrow_id);
    key
}

fn share_key(escrow_id: &[u8; 32]) -> [u8; 33] {
    let mut key = [0u8; 33];
    key[0] = SHARE_PREFIX;
    key[1..33].copy_from_slice(escrow_id);
    key
}

#[inline(always)]
fn sset(key: &[u8], value: &[u8]) {
    let _ = api::set_storage(StorageFlags::empty(), key, value);
}

#[inline(always)]
fn sget(key: &[u8], out: &mut [u8]) -> bool {
    let mut slice = &mut out[..];
    api::get_storage(StorageFlags::empty(), key, &mut slice).is_ok()
}

// ============================================================================
// FUNCTION SELECTORS
// ============================================================================

#[inline(always)]
fn sel(signature: &str) -> [u8; 4] {
    let mut h = [0u8; 32];
    api::hash_keccak_256(signature.as_bytes(), &mut h);
    [h[0], h[1], h[2], h[3]]
}

// ============================================================================
// EVENTS - Comprehensive for indexer
// ============================================================================

// Event signatures:
// EscrowCreated(bytes32 indexed escrowId, address indexed seller, bytes32 commitment, bytes32 escrowPubkey)
// BuyerJoined(bytes32 indexed escrowId, address indexed buyer)
// PaymentSent(bytes32 indexed escrowId, address indexed buyer)
// PaymentConfirmed(bytes32 indexed escrowId)
// DisputeOpened(bytes32 indexed escrowId, address indexed initiator)
// DisputeResolved(bytes32 indexed escrowId, address indexed winner, bytes32 shareC)

fn emit_escrow_created(escrow_id: &[u8; 32], seller: &[u8; 20], commitment: &[u8; 32], escrow_pubkey: &[u8; 32]) {
    let mut topic0 = [0u8; 32];
    api::hash_keccak_256(b"EscrowCreated(bytes32,address,bytes32,bytes32)", &mut topic0);

    let mut topic1 = [0u8; 32];
    topic1.copy_from_slice(escrow_id);

    let mut topic2 = [0u8; 32];
    topic2[12..32].copy_from_slice(seller);

    // Data: commitment || escrowPubkey
    let mut data = [0u8; 64];
    data[..32].copy_from_slice(commitment);
    data[32..64].copy_from_slice(escrow_pubkey);

    let topics = [topic0, topic1, topic2];
    api::deposit_event(&topics, &data);
}

fn emit_buyer_joined(escrow_id: &[u8; 32], buyer: &[u8; 20]) {
    let mut topic0 = [0u8; 32];
    api::hash_keccak_256(b"BuyerJoined(bytes32,address)", &mut topic0);

    let mut topic1 = [0u8; 32];
    topic1.copy_from_slice(escrow_id);

    let mut topic2 = [0u8; 32];
    topic2[12..32].copy_from_slice(buyer);

    let topics = [topic0, topic1, topic2];
    api::deposit_event(&topics, &[]);
}

fn emit_payment_sent(escrow_id: &[u8; 32], buyer: &[u8; 20]) {
    let mut topic0 = [0u8; 32];
    api::hash_keccak_256(b"PaymentSent(bytes32,address)", &mut topic0);

    let mut topic1 = [0u8; 32];
    topic1.copy_from_slice(escrow_id);

    let mut topic2 = [0u8; 32];
    topic2[12..32].copy_from_slice(buyer);

    let topics = [topic0, topic1, topic2];
    api::deposit_event(&topics, &[]);
}

fn emit_payment_confirmed(escrow_id: &[u8; 32], seller: &[u8; 20]) {
    let mut topic0 = [0u8; 32];
    api::hash_keccak_256(b"PaymentConfirmed(bytes32,address)", &mut topic0);

    let mut topic1 = [0u8; 32];
    topic1.copy_from_slice(escrow_id);

    let mut topic2 = [0u8; 32];
    topic2[12..32].copy_from_slice(seller);

    let topics = [topic0, topic1, topic2];
    api::deposit_event(&topics, &[]);
}

fn emit_dispute_opened(escrow_id: &[u8; 32], initiator: &[u8; 20]) {
    let mut topic0 = [0u8; 32];
    api::hash_keccak_256(b"DisputeOpened(bytes32,address)", &mut topic0);

    let mut topic1 = [0u8; 32];
    topic1.copy_from_slice(escrow_id);

    let mut topic2 = [0u8; 32];
    topic2[12..32].copy_from_slice(initiator);

    let topics = [topic0, topic1, topic2];
    api::deposit_event(&topics, &[]);
}

fn emit_dispute_resolved(escrow_id: &[u8; 32], winner: &[u8; 20], share_c: &[u8; 32]) {
    let mut topic0 = [0u8; 32];
    api::hash_keccak_256(b"DisputeResolved(bytes32,address,bytes32)", &mut topic0);

    let mut topic1 = [0u8; 32];
    topic1.copy_from_slice(escrow_id);

    let mut topic2 = [0u8; 32];
    topic2[12..32].copy_from_slice(winner);

    let topics = [topic0, topic1, topic2];
    api::deposit_event(&topics, share_c);
}

// ============================================================================
// CONTRACT ENTRY POINTS
// ============================================================================

#[polkavm_derive::polkavm_export]
pub extern "C" fn deploy() {
    // No initialization needed
}

#[polkavm_derive::polkavm_export]
pub extern "C" fn call() {
    let input_len = api::call_data_size() as usize;

    if input_len < 4 {
        api::return_value(ReturnFlags::REVERT, &[0x01]); // Invalid input
    }

    let mut selector = [0u8; 4];
    api::call_data_copy(&mut selector, 0);

    // createEscrow(bytes32 escrowId, bytes32 commitment, bytes32 escrowPubkey, bytes32 shareC)
    // escrowId is now user-provided for determinism
    if selector == sel("createEscrow(bytes32,bytes32,bytes32,bytes32)") {
        if input_len < 4 + 128 {
            api::return_value(ReturnFlags::REVERT, &[0x02]);
        }

        let mut calldata = [0u8; 128];
        api::call_data_copy(&mut calldata, 4);

        let mut escrow_id = [0u8; 32];
        let mut commitment = [0u8; 32];
        let mut escrow_pubkey = [0u8; 32];
        let mut share_c = [0u8; 32];

        escrow_id.copy_from_slice(&calldata[0..32]);
        commitment.copy_from_slice(&calldata[32..64]);
        escrow_pubkey.copy_from_slice(&calldata[64..96]);
        share_c.copy_from_slice(&calldata[96..128]);

        // Check if escrow already exists
        let key = escrow_key(&escrow_id);
        let mut existing = [0u8; 1];
        if sget(&key, &mut existing) {
            api::return_value(ReturnFlags::REVERT, &[0x10]); // Escrow already exists
        }

        // Get caller as seller
        let mut seller = [0u8; 20];
        api::caller(&mut seller);

        // Store escrow data
        // Format: state(1) || commitment(32) || escrow_pubkey(32) || seller(20) = 85 bytes
        let mut escrow_data = [0u8; 85];
        escrow_data[0] = STATE_CREATED;
        escrow_data[1..33].copy_from_slice(&commitment);
        escrow_data[33..65].copy_from_slice(&escrow_pubkey);
        escrow_data[65..85].copy_from_slice(&seller);
        sset(&key, &escrow_data);

        // Store share_C
        sset(&share_key(&escrow_id), &share_c);

        emit_escrow_created(&escrow_id, &seller, &commitment, &escrow_pubkey);
        api::return_value(ReturnFlags::empty(), &escrow_id);
    }

    // confirmEscrow(bytes32 escrowId)
    // Buyer confirms by calling (caller becomes buyer)
    else if selector == sel("confirmEscrow(bytes32)") {
        if input_len < 4 + 32 {
            api::return_value(ReturnFlags::REVERT, &[0x02]);
        }

        let mut calldata = [0u8; 32];
        api::call_data_copy(&mut calldata, 4);

        let mut escrow_id = [0u8; 32];
        escrow_id.copy_from_slice(&calldata[0..32]);

        let key = escrow_key(&escrow_id);
        let mut escrow_data = [0u8; 105];
        if !sget(&key, &mut escrow_data[..85]) {
            api::return_value(ReturnFlags::REVERT, &[0x03]); // Escrow not found
        }

        if escrow_data[0] != STATE_CREATED {
            api::return_value(ReturnFlags::REVERT, &[0x04]); // Wrong state
        }

        // Caller becomes buyer
        let mut buyer = [0u8; 20];
        api::caller(&mut buyer);

        // Ensure buyer != seller
        if buyer == escrow_data[65..85] {
            api::return_value(ReturnFlags::REVERT, &[0x11]); // Seller cannot be buyer
        }

        escrow_data[0] = STATE_BUYER_CONFIRMED;
        escrow_data[85..105].copy_from_slice(&buyer);
        sset(&key, &escrow_data[..105]);

        emit_buyer_joined(&escrow_id, &buyer);
        api::return_value(ReturnFlags::empty(), &[0x01]);
    }

    // markPaymentSent(bytes32 escrowId)
    else if selector == sel("markPaymentSent(bytes32)") {
        if input_len < 4 + 32 {
            api::return_value(ReturnFlags::REVERT, &[0x02]);
        }

        let mut calldata = [0u8; 32];
        api::call_data_copy(&mut calldata, 4);

        let mut escrow_id = [0u8; 32];
        escrow_id.copy_from_slice(&calldata[0..32]);

        let key = escrow_key(&escrow_id);
        let mut escrow_data = [0u8; 105];
        if !sget(&key, &mut escrow_data) {
            api::return_value(ReturnFlags::REVERT, &[0x03]);
        }

        let mut caller = [0u8; 20];
        api::caller(&mut caller);
        if caller != escrow_data[85..105] {
            api::return_value(ReturnFlags::REVERT, &[0x05]); // Not buyer
        }

        if escrow_data[0] != STATE_BUYER_CONFIRMED {
            api::return_value(ReturnFlags::REVERT, &[0x04]);
        }

        escrow_data[0] = STATE_PAYMENT_SENT;
        sset(&key, &escrow_data);

        emit_payment_sent(&escrow_id, &caller);
        api::return_value(ReturnFlags::empty(), &[0x01]);
    }

    // confirmPayment(bytes32 escrowId)
    else if selector == sel("confirmPayment(bytes32)") {
        if input_len < 4 + 32 {
            api::return_value(ReturnFlags::REVERT, &[0x02]);
        }

        let mut calldata = [0u8; 32];
        api::call_data_copy(&mut calldata, 4);

        let mut escrow_id = [0u8; 32];
        escrow_id.copy_from_slice(&calldata[0..32]);

        let key = escrow_key(&escrow_id);
        let mut escrow_data = [0u8; 105];
        if !sget(&key, &mut escrow_data) {
            api::return_value(ReturnFlags::REVERT, &[0x03]);
        }

        let mut caller = [0u8; 20];
        api::caller(&mut caller);
        if caller != escrow_data[65..85] {
            api::return_value(ReturnFlags::REVERT, &[0x06]); // Not seller
        }

        if escrow_data[0] != STATE_PAYMENT_SENT {
            api::return_value(ReturnFlags::REVERT, &[0x04]);
        }

        escrow_data[0] = STATE_COMPLETED;
        sset(&key, &escrow_data);

        emit_payment_confirmed(&escrow_id, &caller);
        api::return_value(ReturnFlags::empty(), &[0x01]);
    }

    // dispute(bytes32 escrowId)
    else if selector == sel("dispute(bytes32)") {
        if input_len < 4 + 32 {
            api::return_value(ReturnFlags::REVERT, &[0x02]);
        }

        let mut calldata = [0u8; 32];
        api::call_data_copy(&mut calldata, 4);

        let mut escrow_id = [0u8; 32];
        escrow_id.copy_from_slice(&calldata[0..32]);

        let key = escrow_key(&escrow_id);
        let mut escrow_data = [0u8; 105];
        if !sget(&key, &mut escrow_data) {
            api::return_value(ReturnFlags::REVERT, &[0x03]);
        }

        // Can dispute from BUYER_CONFIRMED or PAYMENT_SENT
        if escrow_data[0] != STATE_BUYER_CONFIRMED && escrow_data[0] != STATE_PAYMENT_SENT {
            api::return_value(ReturnFlags::REVERT, &[0x04]);
        }

        let mut caller = [0u8; 20];
        api::caller(&mut caller);
        let is_buyer = caller == escrow_data[85..105];
        let is_seller = caller == escrow_data[65..85];
        if !is_buyer && !is_seller {
            api::return_value(ReturnFlags::REVERT, &[0x07]); // Not party
        }

        escrow_data[0] = STATE_DISPUTED;
        sset(&key, &escrow_data);

        emit_dispute_opened(&escrow_id, &caller);
        api::return_value(ReturnFlags::empty(), &[0x01]);
    }

    // resolveDispute(bytes32 escrowId, bool toBuyer)
    else if selector == sel("resolveDispute(bytes32,bool)") {
        if input_len < 4 + 64 {
            api::return_value(ReturnFlags::REVERT, &[0x02]);
        }

        let mut calldata = [0u8; 64];
        api::call_data_copy(&mut calldata, 4);

        let mut escrow_id = [0u8; 32];
        escrow_id.copy_from_slice(&calldata[0..32]);
        let to_buyer = calldata[63] != 0;

        let key = escrow_key(&escrow_id);
        let mut escrow_data = [0u8; 105];
        if !sget(&key, &mut escrow_data) {
            api::return_value(ReturnFlags::REVERT, &[0x03]);
        }

        if escrow_data[0] != STATE_DISPUTED {
            api::return_value(ReturnFlags::REVERT, &[0x04]);
        }

        let mut share_c = [0u8; 32];
        sget(&share_key(&escrow_id), &mut share_c);

        let winner: [u8; 20] = if to_buyer {
            escrow_data[0] = STATE_RESOLVED_BUYER;
            let mut w = [0u8; 20];
            w.copy_from_slice(&escrow_data[85..105]);
            w
        } else {
            escrow_data[0] = STATE_RESOLVED_SELLER;
            let mut w = [0u8; 20];
            w.copy_from_slice(&escrow_data[65..85]);
            w
        };

        sset(&key, &escrow_data);
        emit_dispute_resolved(&escrow_id, &winner, &share_c);
        api::return_value(ReturnFlags::empty(), &[0x01]);
    }

    // getEscrow(bytes32 escrowId) -> (uint8 state, bytes32 commitment, bytes32 escrowPubkey, address seller, address buyer)
    else if selector == sel("getEscrow(bytes32)") {
        if input_len < 4 + 32 {
            api::return_value(ReturnFlags::REVERT, &[0x02]);
        }

        let mut calldata = [0u8; 32];
        api::call_data_copy(&mut calldata, 4);

        let mut escrow_id = [0u8; 32];
        escrow_id.copy_from_slice(&calldata[0..32]);

        let key = escrow_key(&escrow_id);
        let mut escrow_data = [0u8; 105];

        // Try reading 105 bytes first (with buyer), fall back to 85 (without)
        let has_buyer = sget(&key, &mut escrow_data);
        if !has_buyer {
            // Try 85 bytes (no buyer yet)
            if !sget(&key, &mut escrow_data[..85]) {
                api::return_value(ReturnFlags::REVERT, &[0x03]); // Not found
            }
        }

        // ABI encode response:
        // state (uint8 padded to 32 bytes)
        // commitment (32 bytes)
        // escrowPubkey (32 bytes)
        // seller (address padded to 32 bytes)
        // buyer (address padded to 32 bytes)
        let mut response = [0u8; 160];
        response[31] = escrow_data[0]; // state
        response[32..64].copy_from_slice(&escrow_data[1..33]); // commitment
        response[64..96].copy_from_slice(&escrow_data[33..65]); // escrowPubkey
        response[108..128].copy_from_slice(&escrow_data[65..85]); // seller (right-aligned)
        if escrow_data[0] >= STATE_BUYER_CONFIRMED {
            response[140..160].copy_from_slice(&escrow_data[85..105]); // buyer
        }

        api::return_value(ReturnFlags::empty(), &response);
    }

    // verifyShare(bytes32 commitment, uint8 index, bytes32[8] shareValues, bytes32[] proof)
    else if selector == sel("verifyShare(bytes32,uint8,bytes32[8],bytes32[])") {
        if input_len < 4 + 32 + 32 + 256 + 32 + 32 {
            api::return_value(ReturnFlags::REVERT, &[0x02]);
        }

        let mut fixed_data = [0u8; 320];
        api::call_data_copy(&mut fixed_data, 4);

        let mut commitment = [0u8; 32];
        commitment.copy_from_slice(&fixed_data[0..32]);

        let share_index = fixed_data[63] as usize;

        let mut share_values = [BF32::ZERO; 8];
        for i in 0..8 {
            let offset = 64 + i * 32;
            let mut bytes = [0u8; 4];
            bytes.copy_from_slice(&fixed_data[offset + 28..offset + 32]);
            share_values[i] = BF32::from_le_bytes([bytes[3], bytes[2], bytes[1], bytes[0]]);
        }

        let proof_len_offset = 4 + 320;
        let mut proof_len_bytes = [0u8; 32];
        api::call_data_copy(&mut proof_len_bytes, proof_len_offset as u32);
        let proof_len = u32::from_be_bytes([
            proof_len_bytes[28], proof_len_bytes[29],
            proof_len_bytes[30], proof_len_bytes[31]
        ]) as usize;

        if proof_len > 10 {
            api::return_value(ReturnFlags::REVERT, &[0x08]);
        }

        let mut proof = [[0u8; 32]; 10];
        for i in 0..proof_len {
            let offset = proof_len_offset + 32 + i * 32;
            api::call_data_copy(&mut proof[i], offset as u32);
        }

        let valid = verify_share(&commitment, share_index, &share_values, &proof[..proof_len]);

        if valid {
            api::return_value(ReturnFlags::empty(), &[0x01]);
        } else {
            api::return_value(ReturnFlags::REVERT, &[0x09]);
        }
    }

    else {
        api::return_value(ReturnFlags::REVERT, &[0xFF]);
    }
}
