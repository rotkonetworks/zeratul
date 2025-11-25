//! IBP Monitoring Probe - PolkaVM Guest Program
//!
//! This program runs inside PolkaVM and defines the monitoring logic for IBP endpoints.
//! Host functions provide actual network I/O, while this guest defines what to check
//! and how to interpret results.
//!
//! The execution trace + ligerito proof proves the monitoring logic was executed correctly.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use core::panic::PanicInfo;

// ============================================================================
// Host Function Declarations (implemented by host runtime)
// ============================================================================

// Host call numbers - must match host implementation
const HOST_TCP_PING: u32 = 0x100;
const HOST_WSS_CONNECT: u32 = 0x101;
const HOST_WSS_SUBSCRIBE: u32 = 0x102;
const HOST_RPC_CALL: u32 = 0x103;
const HOST_RELAY_FINALIZED: u32 = 0x104;
const HOST_TIMESTAMP: u32 = 0x105;
const HOST_READ_INPUT: u32 = 0x106;
const HOST_WRITE_OUTPUT: u32 = 0x107;

/// Raw host call - implemented via ecalli instruction
#[inline(never)]
unsafe fn host_call(call_id: u32, a0: u32, a1: u32, a2: u32, a3: u32) -> u32 {
    let result: u32;
    core::arch::asm!(
        "ecalli {call_id}",
        call_id = in(reg) call_id,
        in("a0") a0,
        in("a1") a1,
        in("a2") a2,
        in("a3") a3,
        lateout("a0") result,
        options(nostack)
    );
    result
}

// ============================================================================
// Host Function Wrappers
// ============================================================================

/// TCP ping - returns latency in ms, or u32::MAX on failure
fn tcp_ping(endpoint_ptr: u32, endpoint_len: u32, port: u16, timeout_ms: u32) -> u32 {
    unsafe { host_call(HOST_TCP_PING, endpoint_ptr, endpoint_len, port as u32, timeout_ms) }
}

/// WebSocket connect - returns connection handle or 0 on failure, latency in high bits
fn wss_connect(url_ptr: u32, url_len: u32, timeout_ms: u32) -> u64 {
    let low = unsafe { host_call(HOST_WSS_CONNECT, url_ptr, url_len, timeout_ms, 0) };
    let high = unsafe { host_call(HOST_WSS_CONNECT + 1, 0, 0, 0, 0) }; // get high bits
    ((high as u64) << 32) | (low as u64)
}

/// Subscribe to new blocks via WebSocket
fn wss_subscribe_blocks(handle: u32) -> u32 {
    unsafe { host_call(HOST_WSS_SUBSCRIBE, handle, 0, 0, 0) }
}

/// JSON-RPC call - returns response length, data written to output buffer
fn rpc_call(
    url_ptr: u32,
    url_len: u32,
    method_ptr: u32,
    method_len: u32,
    out_ptr: u32,
    out_len: u32,
) -> u32 {
    // Pack into two calls due to register limits
    let setup = unsafe { host_call(HOST_RPC_CALL, url_ptr, url_len, method_ptr, method_len) };
    if setup == 0 {
        return 0;
    }
    unsafe { host_call(HOST_RPC_CALL + 1, out_ptr, out_len, 0, 0) }
}

/// Get relay chain finalized block - returns (block_number, hash_ptr)
fn relay_finalized_block(hash_out: u32) -> u32 {
    unsafe { host_call(HOST_RELAY_FINALIZED, hash_out, 0, 0, 0) }
}

/// Current timestamp in milliseconds
fn timestamp_ms() -> u64 {
    let low = unsafe { host_call(HOST_TIMESTAMP, 0, 0, 0, 0) };
    let high = unsafe { host_call(HOST_TIMESTAMP + 1, 0, 0, 0, 0) };
    ((high as u64) << 32) | (low as u64)
}

/// Read input data from host
fn read_input(buf: u32, max_len: u32) -> u32 {
    unsafe { host_call(HOST_READ_INPUT, buf, max_len, 0, 0) }
}

/// Write output data to host
fn write_output(buf: u32, len: u32) {
    unsafe { host_call(HOST_WRITE_OUTPUT, buf, len, 0, 0) };
}

// ============================================================================
// Data Structures
// ============================================================================

/// Input: which endpoint to check and SLA requirements
#[repr(C)]
pub struct ProbeInput {
    /// Endpoint URL (null-terminated in buffer)
    pub endpoint_offset: u32,
    pub endpoint_len: u32,
    /// Network (polkadot, kusama, etc.)
    pub network_offset: u32,
    pub network_len: u32,
    /// SLA requirements
    pub max_ping_ms: u32,
    pub max_wss_connect_ms: u32,
    pub max_rpc_latency_ms: u32,
    pub require_data_accuracy: bool,
}

/// Output: monitoring results
#[repr(C)]
pub struct ProbeOutput {
    /// Timing results
    pub ping_latency_ms: u32,
    pub wss_connect_latency_ms: u32,
    pub rpc_latency_ms: u32,

    /// Data integrity
    pub endpoint_block_number: u32,
    pub endpoint_block_hash: [u8; 32],
    pub relay_block_number: u32,
    pub relay_block_hash: [u8; 32],
    pub data_matches_relay: bool,

    /// Health assessment
    pub ping_healthy: bool,
    pub wss_healthy: bool,
    pub rpc_healthy: bool,
    pub data_healthy: bool,
    pub overall_healthy: bool,

    /// Timestamp
    pub checked_at_ms: u64,
}

// ============================================================================
// Memory Management (simple bump allocator)
// ============================================================================

const HEAP_SIZE: usize = 64 * 1024; // 64KB
static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];
static mut HEAP_POS: usize = 0;

fn alloc(size: usize) -> *mut u8 {
    unsafe {
        let aligned = (HEAP_POS + 7) & !7; // 8-byte align
        if aligned + size > HEAP_SIZE {
            return core::ptr::null_mut();
        }
        HEAP_POS = aligned + size;
        HEAP.as_mut_ptr().add(aligned)
    }
}

// ============================================================================
// Main Entry Point
// ============================================================================

#[no_mangle]
pub extern "C" fn check_endpoint() -> u32 {
    // Allocate buffers
    let input_buf = alloc(1024) as u32;
    let output_buf = alloc(core::mem::size_of::<ProbeOutput>()) as u32;
    let rpc_buf = alloc(256) as u32;
    let hash_buf = alloc(32) as u32;

    if input_buf == 0 || output_buf == 0 {
        return 1; // allocation failed
    }

    // Read input from host
    let input_len = read_input(input_buf, 1024);
    if input_len == 0 {
        return 2; // no input
    }

    // Parse input (simplified - assumes fixed layout)
    let input = unsafe { &*(input_buf as *const ProbeInput) };

    let endpoint_ptr = input_buf + input.endpoint_offset;
    let endpoint_len = input.endpoint_len;

    // Initialize output
    let output = unsafe { &mut *(output_buf as *mut ProbeOutput) };
    *output = ProbeOutput {
        ping_latency_ms: u32::MAX,
        wss_connect_latency_ms: u32::MAX,
        rpc_latency_ms: u32::MAX,
        endpoint_block_number: 0,
        endpoint_block_hash: [0; 32],
        relay_block_number: 0,
        relay_block_hash: [0; 32],
        data_matches_relay: false,
        ping_healthy: false,
        wss_healthy: false,
        rpc_healthy: false,
        data_healthy: false,
        overall_healthy: false,
        checked_at_ms: 0,
    };

    // Record start time
    let start_time = timestamp_ms();

    // ========================================
    // 1. TCP Ping Check
    // ========================================
    output.ping_latency_ms = tcp_ping(endpoint_ptr, endpoint_len, 443, 5000);
    output.ping_healthy = output.ping_latency_ms < input.max_ping_ms;

    // ========================================
    // 2. WebSocket Connect Check
    // ========================================
    // Build wss:// URL
    let wss_result = wss_connect(endpoint_ptr, endpoint_len, 10000);
    let wss_handle = (wss_result & 0xFFFFFFFF) as u32;
    output.wss_connect_latency_ms = (wss_result >> 32) as u32;
    output.wss_healthy = wss_handle != 0 && output.wss_connect_latency_ms < input.max_wss_connect_ms;

    // ========================================
    // 3. RPC Call Check (chain_getFinalizedHead)
    // ========================================
    let method = b"chain_getFinalizedHead\0";
    let method_ptr = method.as_ptr() as u32;
    let method_len = method.len() as u32 - 1; // exclude null

    let rpc_start = timestamp_ms();
    let rpc_result_len = rpc_call(endpoint_ptr, endpoint_len, method_ptr, method_len, rpc_buf, 256);
    output.rpc_latency_ms = (timestamp_ms() - rpc_start) as u32;

    output.rpc_healthy = rpc_result_len > 0 && output.rpc_latency_ms < input.max_rpc_latency_ms;

    // Parse RPC result to get block hash
    if rpc_result_len >= 32 {
        unsafe {
            core::ptr::copy_nonoverlapping(
                rpc_buf as *const u8,
                output.endpoint_block_hash.as_mut_ptr(),
                32,
            );
        }
    }

    // ========================================
    // 4. Data Accuracy Check (compare with relay)
    // ========================================
    if input.require_data_accuracy {
        output.relay_block_number = relay_finalized_block(hash_buf);

        // Copy relay hash
        unsafe {
            core::ptr::copy_nonoverlapping(
                hash_buf as *const u8,
                output.relay_block_hash.as_mut_ptr(),
                32,
            );
        }

        // Compare hashes
        output.data_matches_relay = output.endpoint_block_hash == output.relay_block_hash;
        output.data_healthy = output.data_matches_relay;
    } else {
        output.data_healthy = true; // not required
    }

    // ========================================
    // 5. Overall Health Assessment
    // ========================================
    output.overall_healthy =
        output.ping_healthy && output.wss_healthy && output.rpc_healthy && output.data_healthy;

    output.checked_at_ms = start_time;

    // Write output to host
    write_output(output_buf, core::mem::size_of::<ProbeOutput>() as u32);

    // Return 0 for success
    0
}

/// Check multiple endpoints (batch mode)
#[no_mangle]
pub extern "C" fn check_endpoints_batch() -> u32 {
    // Read endpoint count
    let count_buf = alloc(4) as u32;
    read_input(count_buf, 4);
    let count = unsafe { *(count_buf as *const u32) };

    let mut all_healthy = true;

    for _ in 0..count {
        let result = check_endpoint();
        if result != 0 {
            all_healthy = false;
        }
    }

    if all_healthy {
        0
    } else {
        1
    }
}

// ============================================================================
// Panic Handler
// ============================================================================

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

// ============================================================================
// Allocator (required for alloc crate)
// ============================================================================

use core::alloc::{GlobalAlloc, Layout};

struct GuestAllocator;

unsafe impl GlobalAlloc for GuestAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        alloc(layout.size())
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // bump allocator doesn't dealloc
    }
}

#[global_allocator]
static ALLOCATOR: GuestAllocator = GuestAllocator;
