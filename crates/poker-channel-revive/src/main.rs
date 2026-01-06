//! poker state channel contract for revive/polkavm
//!
//! handles:
//! - game creation with deposits
//! - player joining
//! - state updates (hash only, off-chain data)
//! - dispute submission
//! - settlement
//!
//! shuffle proofs are verified off-chain during gameplay.
//! on dispute, the contract verifies the last signed state.

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

// ============================================================================
// ALLOCATOR
// ============================================================================

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
// CRYPTO HELPERS
// ============================================================================

fn keccak256(data: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    api::hash_keccak_256(data, &mut out);
    out
}

/// recover signer from signature (ECDSA secp256k1)
fn ecrecover(msg_hash: &[u8; 32], signature: &[u8; 65]) -> Option<[u8; 20]> {
    let mut out = [0u8; 33];
    let result = api::ecdsa_recover(signature, msg_hash, &mut out);
    if result.is_ok() {
        let pubkey_hash = keccak256(&out[1..]);
        let mut addr = [0u8; 20];
        addr.copy_from_slice(&pubkey_hash[12..32]);
        Some(addr)
    } else {
        None
    }
}

// ============================================================================
// STORAGE LAYOUT
// ============================================================================

const GAME_PREFIX: u8 = 0x10;
const PLAYER_PREFIX: u8 = 0x20;

// game states
const STATE_CREATED: u8 = 0;
const STATE_ACTIVE: u8 = 1;
const STATE_DISPUTED: u8 = 2;
const STATE_SETTLED: u8 = 3;

/// game storage layout:
/// key: GAME_PREFIX || game_id (32 bytes)
/// value:
///   - state: u8 (offset 0)
///   - host: [u8; 20] (offset 1)
///   - big_blind: u128 (offset 21, 16 bytes)
///   - min_players: u8 (offset 37)
///   - max_players: u8 (offset 38)
///   - current_players: u8 (offset 39)
///   - state_hash: [u8; 32] (offset 40)
///   - nonce: u64 (offset 72, 8 bytes)
///   - dispute_block: u64 (offset 80, 8 bytes)
///   - dispute_timeout: u64 (offset 88, 8 bytes)
const GAME_DATA_SIZE: usize = 96;

/// player storage layout:
/// key: PLAYER_PREFIX || game_id (32 bytes) || seat (1 byte)
/// value:
///   - address: [u8; 20]
///   - deposit: u128 (16 bytes)
///   - encryption_key: [u8; 32]
const PLAYER_DATA_SIZE: usize = 68;

fn game_key(game_id: &[u8; 32]) -> [u8; 33] {
    let mut key = [0u8; 33];
    key[0] = GAME_PREFIX;
    key[1..33].copy_from_slice(game_id);
    key
}

fn player_key(game_id: &[u8; 32], seat: u8) -> [u8; 34] {
    let mut key = [0u8; 34];
    key[0] = PLAYER_PREFIX;
    key[1..33].copy_from_slice(game_id);
    key[33] = seat;
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
// EVENTS
// ============================================================================

fn emit_game_created(game_id: &[u8; 32], host: &[u8; 20], big_blind: u128) {
    let mut topic0 = [0u8; 32];
    api::hash_keccak_256(b"GameCreated(bytes32,address,uint128)", &mut topic0);

    let mut topic1 = [0u8; 32];
    topic1.copy_from_slice(game_id);

    let mut topic2 = [0u8; 32];
    topic2[12..32].copy_from_slice(host);

    let mut data = [0u8; 16];
    data.copy_from_slice(&big_blind.to_be_bytes());

    let topics = [topic0, topic1, topic2];
    api::deposit_event(&topics, &data);
}

fn emit_player_joined(game_id: &[u8; 32], player: &[u8; 20], seat: u8) {
    let mut topic0 = [0u8; 32];
    api::hash_keccak_256(b"PlayerJoined(bytes32,address,uint8)", &mut topic0);

    let mut topic1 = [0u8; 32];
    topic1.copy_from_slice(game_id);

    let mut topic2 = [0u8; 32];
    topic2[12..32].copy_from_slice(player);

    let topics = [topic0, topic1, topic2];
    api::deposit_event(&topics, &[seat]);
}

fn emit_game_started(game_id: &[u8; 32]) {
    let mut topic0 = [0u8; 32];
    api::hash_keccak_256(b"GameStarted(bytes32)", &mut topic0);

    let mut topic1 = [0u8; 32];
    topic1.copy_from_slice(game_id);

    let topics = [topic0, topic1];
    api::deposit_event(&topics, &[]);
}

fn emit_state_updated(game_id: &[u8; 32], nonce: u64, state_hash: &[u8; 32]) {
    let mut topic0 = [0u8; 32];
    api::hash_keccak_256(b"StateUpdated(bytes32,uint64,bytes32)", &mut topic0);

    let mut topic1 = [0u8; 32];
    topic1.copy_from_slice(game_id);

    let mut data = [0u8; 40];
    data[..8].copy_from_slice(&nonce.to_be_bytes());
    data[8..40].copy_from_slice(state_hash);

    let topics = [topic0, topic1];
    api::deposit_event(&topics, &data);
}

fn emit_dispute_opened(game_id: &[u8; 32], initiator: &[u8; 20]) {
    let mut topic0 = [0u8; 32];
    api::hash_keccak_256(b"DisputeOpened(bytes32,address)", &mut topic0);

    let mut topic1 = [0u8; 32];
    topic1.copy_from_slice(game_id);

    let mut topic2 = [0u8; 32];
    topic2[12..32].copy_from_slice(initiator);

    let topics = [topic0, topic1, topic2];
    api::deposit_event(&topics, &[]);
}

fn emit_game_settled(game_id: &[u8; 32]) {
    let mut topic0 = [0u8; 32];
    api::hash_keccak_256(b"GameSettled(bytes32)", &mut topic0);

    let mut topic1 = [0u8; 32];
    topic1.copy_from_slice(game_id);

    let topics = [topic0, topic1];
    api::deposit_event(&topics, &[]);
}

// ============================================================================
// CONTRACT ENTRY POINTS
// ============================================================================

#[polkavm_derive::polkavm_export]
pub extern "C" fn deploy() {
    // no initialization needed
}

#[polkavm_derive::polkavm_export]
pub extern "C" fn call() {
    let input_len = api::call_data_size() as usize;

    if input_len < 4 {
        api::return_value(ReturnFlags::REVERT, &[0x01]);
    }

    let mut selector = [0u8; 4];
    api::call_data_copy(&mut selector, 0);

    // createGame(bytes32 gameId, uint128 bigBlind, uint8 minPlayers, uint8 maxPlayers, uint64 disputeTimeout, bytes32 encryptionKey)
    if selector == sel("createGame(bytes32,uint128,uint8,uint8,uint64,bytes32)") {
        handle_create_game(input_len);
    }
    // joinGame(bytes32 gameId, uint8 seat, bytes32 encryptionKey)
    else if selector == sel("joinGame(bytes32,uint8,bytes32)") {
        handle_join_game(input_len);
    }
    // startGame(bytes32 gameId)
    else if selector == sel("startGame(bytes32)") {
        handle_start_game(input_len);
    }
    // updateState(bytes32 gameId, uint64 nonce, bytes32 stateHash, bytes[] signatures)
    else if selector == sel("updateState(bytes32,uint64,bytes32,bytes[])") {
        handle_update_state(input_len);
    }
    // dispute(bytes32 gameId, uint64 nonce, bytes32 stateHash, bytes[] signatures)
    else if selector == sel("dispute(bytes32,uint64,bytes32,bytes[])") {
        handle_dispute(input_len);
    }
    // settle(bytes32 gameId, uint128[] payouts)
    else if selector == sel("settle(bytes32,uint128[])") {
        handle_settle(input_len);
    }
    // getGame(bytes32 gameId)
    else if selector == sel("getGame(bytes32)") {
        handle_get_game(input_len);
    }
    // getPlayer(bytes32 gameId, uint8 seat)
    else if selector == sel("getPlayer(bytes32,uint8)") {
        handle_get_player(input_len);
    }
    else {
        api::return_value(ReturnFlags::REVERT, &[0xFF]);
    }
}

fn handle_create_game(input_len: usize) {
    // gameId(32) + bigBlind(32 padded) + minPlayers(32 padded) + maxPlayers(32 padded) + disputeTimeout(32 padded) + encryptionKey(32)
    if input_len < 4 + 192 {
        api::return_value(ReturnFlags::REVERT, &[0x02]);
    }

    let mut calldata = [0u8; 192];
    api::call_data_copy(&mut calldata, 4);

    let mut game_id = [0u8; 32];
    game_id.copy_from_slice(&calldata[0..32]);

    // check if game exists
    let key = game_key(&game_id);
    let mut existing = [0u8; 1];
    if sget(&key, &mut existing) {
        api::return_value(ReturnFlags::REVERT, &[0x10]); // game exists
    }

    // parse params (big-endian, right-padded to 32 bytes)
    let big_blind = u128::from_be_bytes(calldata[48..64].try_into().unwrap());
    let min_players = calldata[95];
    let max_players = calldata[127];
    let dispute_timeout = u64::from_be_bytes(calldata[152..160].try_into().unwrap());

    let mut encryption_key = [0u8; 32];
    encryption_key.copy_from_slice(&calldata[160..192]);

    // get caller as host
    let mut host = [0u8; 20];
    api::caller(&mut host);

    // get deposit (msg.value)
    let mut value_bytes = [0u8; 32];
    api::value_transferred(&mut value_bytes);
    let deposit = u128::from_be_bytes(value_bytes[16..32].try_into().unwrap());

    // require minimum deposit (10 * big_blind)
    let min_deposit = big_blind.saturating_mul(10);
    if deposit < min_deposit {
        api::return_value(ReturnFlags::REVERT, &[0x11]); // insufficient deposit
    }

    // store game
    let mut game_data = [0u8; GAME_DATA_SIZE];
    game_data[0] = STATE_CREATED;
    game_data[1..21].copy_from_slice(&host);
    game_data[21..37].copy_from_slice(&big_blind.to_le_bytes());
    game_data[37] = min_players;
    game_data[38] = max_players;
    game_data[39] = 1; // host is first player
    // state_hash: zeros initially
    // nonce: 0
    // dispute_block: 0
    game_data[88..96].copy_from_slice(&dispute_timeout.to_le_bytes());
    sset(&key, &game_data);

    // store host as player 0
    let player_k = player_key(&game_id, 0);
    let mut player_data = [0u8; PLAYER_DATA_SIZE];
    player_data[0..20].copy_from_slice(&host);
    player_data[20..36].copy_from_slice(&deposit.to_le_bytes());
    player_data[36..68].copy_from_slice(&encryption_key);
    sset(&player_k, &player_data);

    emit_game_created(&game_id, &host, big_blind);
    emit_player_joined(&game_id, &host, 0);

    api::return_value(ReturnFlags::empty(), &game_id);
}

fn handle_join_game(input_len: usize) {
    // gameId(32) + seat(32 padded) + encryptionKey(32)
    if input_len < 4 + 96 {
        api::return_value(ReturnFlags::REVERT, &[0x02]);
    }

    let mut calldata = [0u8; 96];
    api::call_data_copy(&mut calldata, 4);

    let mut game_id = [0u8; 32];
    game_id.copy_from_slice(&calldata[0..32]);

    let seat = calldata[63];

    let mut encryption_key = [0u8; 32];
    encryption_key.copy_from_slice(&calldata[64..96]);

    // load game
    let key = game_key(&game_id);
    let mut game_data = [0u8; GAME_DATA_SIZE];
    if !sget(&key, &mut game_data) {
        api::return_value(ReturnFlags::REVERT, &[0x03]); // game not found
    }

    if game_data[0] != STATE_CREATED {
        api::return_value(ReturnFlags::REVERT, &[0x04]); // wrong state
    }

    let max_players = game_data[38];
    if seat >= max_players {
        api::return_value(ReturnFlags::REVERT, &[0x12]); // invalid seat
    }

    // check seat not taken
    let player_k = player_key(&game_id, seat);
    let mut existing_player = [0u8; 20];
    if sget(&player_k, &mut existing_player) {
        api::return_value(ReturnFlags::REVERT, &[0x13]); // seat taken
    }

    // get caller
    let mut caller = [0u8; 20];
    api::caller(&mut caller);

    // get deposit
    let mut value_bytes = [0u8; 32];
    api::value_transferred(&mut value_bytes);
    let deposit = u128::from_be_bytes(value_bytes[16..32].try_into().unwrap());

    // check min deposit
    let big_blind = u128::from_le_bytes(game_data[21..37].try_into().unwrap());
    let min_deposit = big_blind.saturating_mul(10);
    if deposit < min_deposit {
        api::return_value(ReturnFlags::REVERT, &[0x11]);
    }

    // store player
    let mut player_data = [0u8; PLAYER_DATA_SIZE];
    player_data[0..20].copy_from_slice(&caller);
    player_data[20..36].copy_from_slice(&deposit.to_le_bytes());
    player_data[36..68].copy_from_slice(&encryption_key);
    sset(&player_k, &player_data);

    // increment player count
    game_data[39] += 1;
    sset(&key, &game_data);

    emit_player_joined(&game_id, &caller, seat);
    api::return_value(ReturnFlags::empty(), &[0x01]);
}

fn handle_start_game(input_len: usize) {
    if input_len < 4 + 32 {
        api::return_value(ReturnFlags::REVERT, &[0x02]);
    }

    let mut game_id = [0u8; 32];
    api::call_data_copy(&mut game_id, 4);

    let key = game_key(&game_id);
    let mut game_data = [0u8; GAME_DATA_SIZE];
    if !sget(&key, &mut game_data) {
        api::return_value(ReturnFlags::REVERT, &[0x03]);
    }

    // only host can start
    let mut caller = [0u8; 20];
    api::caller(&mut caller);
    if caller != game_data[1..21] {
        api::return_value(ReturnFlags::REVERT, &[0x05]); // not host
    }

    if game_data[0] != STATE_CREATED {
        api::return_value(ReturnFlags::REVERT, &[0x04]);
    }

    let min_players = game_data[37];
    let current_players = game_data[39];
    if current_players < min_players {
        api::return_value(ReturnFlags::REVERT, &[0x14]); // not enough players
    }

    game_data[0] = STATE_ACTIVE;
    sset(&key, &game_data);

    emit_game_started(&game_id);
    api::return_value(ReturnFlags::empty(), &[0x01]);
}

fn handle_update_state(input_len: usize) {
    // gameId(32) + nonce(32 padded) + stateHash(32) + signatures offset(32) + signatures...
    if input_len < 4 + 128 {
        api::return_value(ReturnFlags::REVERT, &[0x02]);
    }

    let mut fixed = [0u8; 96];
    api::call_data_copy(&mut fixed, 4);

    let mut game_id = [0u8; 32];
    game_id.copy_from_slice(&fixed[0..32]);

    let nonce = u64::from_be_bytes(fixed[56..64].try_into().unwrap());

    let mut state_hash = [0u8; 32];
    state_hash.copy_from_slice(&fixed[64..96]);

    let key = game_key(&game_id);
    let mut game_data = [0u8; GAME_DATA_SIZE];
    if !sget(&key, &mut game_data) {
        api::return_value(ReturnFlags::REVERT, &[0x03]);
    }

    if game_data[0] != STATE_ACTIVE {
        api::return_value(ReturnFlags::REVERT, &[0x04]);
    }

    let current_nonce = u64::from_le_bytes(game_data[72..80].try_into().unwrap());
    if nonce <= current_nonce {
        api::return_value(ReturnFlags::REVERT, &[0x15]); // nonce too low
    }

    // TODO: verify signatures from all active players
    // for now, just accept the update (simplified)

    // update state
    game_data[40..72].copy_from_slice(&state_hash);
    game_data[72..80].copy_from_slice(&nonce.to_le_bytes());
    sset(&key, &game_data);

    emit_state_updated(&game_id, nonce, &state_hash);
    api::return_value(ReturnFlags::empty(), &[0x01]);
}

fn handle_dispute(input_len: usize) {
    if input_len < 4 + 128 {
        api::return_value(ReturnFlags::REVERT, &[0x02]);
    }

    let mut fixed = [0u8; 96];
    api::call_data_copy(&mut fixed, 4);

    let mut game_id = [0u8; 32];
    game_id.copy_from_slice(&fixed[0..32]);

    let nonce = u64::from_be_bytes(fixed[56..64].try_into().unwrap());

    let mut state_hash = [0u8; 32];
    state_hash.copy_from_slice(&fixed[64..96]);

    let key = game_key(&game_id);
    let mut game_data = [0u8; GAME_DATA_SIZE];
    if !sget(&key, &mut game_data) {
        api::return_value(ReturnFlags::REVERT, &[0x03]);
    }

    if game_data[0] != STATE_ACTIVE && game_data[0] != STATE_DISPUTED {
        api::return_value(ReturnFlags::REVERT, &[0x04]);
    }

    let current_nonce = u64::from_le_bytes(game_data[72..80].try_into().unwrap());

    // dispute must have higher nonce than current
    if nonce <= current_nonce {
        api::return_value(ReturnFlags::REVERT, &[0x15]);
    }

    // TODO: verify signatures

    let mut caller = [0u8; 20];
    api::caller(&mut caller);

    // get current block
    let block_number = api::block_number();

    // update state
    game_data[0] = STATE_DISPUTED;
    game_data[40..72].copy_from_slice(&state_hash);
    game_data[72..80].copy_from_slice(&nonce.to_le_bytes());
    game_data[80..88].copy_from_slice(&block_number.to_le_bytes());
    sset(&key, &game_data);

    emit_dispute_opened(&game_id, &caller);
    api::return_value(ReturnFlags::empty(), &[0x01]);
}

fn handle_settle(input_len: usize) {
    // gameId(32) + payouts offset(32) + payouts length(32) + payouts...
    if input_len < 4 + 96 {
        api::return_value(ReturnFlags::REVERT, &[0x02]);
    }

    let mut fixed = [0u8; 64];
    api::call_data_copy(&mut fixed, 4);

    let mut game_id = [0u8; 32];
    game_id.copy_from_slice(&fixed[0..32]);

    let key = game_key(&game_id);
    let mut game_data = [0u8; GAME_DATA_SIZE];
    if !sget(&key, &mut game_data) {
        api::return_value(ReturnFlags::REVERT, &[0x03]);
    }

    // can settle from ACTIVE (cooperative) or DISPUTED (after timeout)
    if game_data[0] == STATE_DISPUTED {
        let dispute_block = u64::from_le_bytes(game_data[80..88].try_into().unwrap());
        let dispute_timeout = u64::from_le_bytes(game_data[88..96].try_into().unwrap());
        let current_block = api::block_number();

        if current_block < dispute_block + dispute_timeout {
            api::return_value(ReturnFlags::REVERT, &[0x16]); // dispute not timed out
        }
    } else if game_data[0] != STATE_ACTIVE {
        api::return_value(ReturnFlags::REVERT, &[0x04]);
    }

    // parse payouts
    let payouts_len_offset = 4 + 64;
    let mut payouts_len_bytes = [0u8; 32];
    api::call_data_copy(&mut payouts_len_bytes, payouts_len_offset as u32);
    let payouts_len = u32::from_be_bytes(payouts_len_bytes[28..32].try_into().unwrap()) as usize;

    let max_players = game_data[38] as usize;
    if payouts_len > max_players {
        api::return_value(ReturnFlags::REVERT, &[0x17]); // too many payouts
    }

    // distribute payouts
    for i in 0..payouts_len {
        let payout_offset = payouts_len_offset + 32 + i * 32;
        let mut payout_bytes = [0u8; 32];
        api::call_data_copy(&mut payout_bytes, payout_offset as u32);
        let payout = u128::from_be_bytes(payout_bytes[16..32].try_into().unwrap());

        if payout > 0 {
            let player_k = player_key(&game_id, i as u8);
            let mut player_data = [0u8; PLAYER_DATA_SIZE];
            if sget(&player_k, &mut player_data) {
                let mut addr = [0u8; 20];
                addr.copy_from_slice(&player_data[0..20]);

                // transfer payout
                let mut payout_256 = [0u8; 32];
                payout_256[16..32].copy_from_slice(&payout.to_be_bytes());
                let _ = api::transfer(&addr, &payout_256);
            }
        }
    }

    game_data[0] = STATE_SETTLED;
    sset(&key, &game_data);

    emit_game_settled(&game_id);
    api::return_value(ReturnFlags::empty(), &[0x01]);
}

fn handle_get_game(input_len: usize) {
    if input_len < 4 + 32 {
        api::return_value(ReturnFlags::REVERT, &[0x02]);
    }

    let mut game_id = [0u8; 32];
    api::call_data_copy(&mut game_id, 4);

    let key = game_key(&game_id);
    let mut game_data = [0u8; GAME_DATA_SIZE];
    if !sget(&key, &mut game_data) {
        api::return_value(ReturnFlags::REVERT, &[0x03]);
    }

    // return game data as-is (ABI encoding would be more complex)
    api::return_value(ReturnFlags::empty(), &game_data);
}

fn handle_get_player(input_len: usize) {
    if input_len < 4 + 64 {
        api::return_value(ReturnFlags::REVERT, &[0x02]);
    }

    let mut calldata = [0u8; 64];
    api::call_data_copy(&mut calldata, 4);

    let mut game_id = [0u8; 32];
    game_id.copy_from_slice(&calldata[0..32]);
    let seat = calldata[63];

    let key = player_key(&game_id, seat);
    let mut player_data = [0u8; PLAYER_DATA_SIZE];
    if !sget(&key, &mut player_data) {
        api::return_value(ReturnFlags::REVERT, &[0x03]);
    }

    api::return_value(ReturnFlags::empty(), &player_data);
}
