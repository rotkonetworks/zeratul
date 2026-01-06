//! ghettobox vault guest - runs inside polkavm sandbox
//!
//! pure protocol logic - no hardware access
//! all crypto operations delegated to host (can use TPM or software)

#![no_std]
#![no_main]

extern crate alloc;

// simple bump allocator
mod allocator {
    use core::alloc::{GlobalAlloc, Layout};
    use core::cell::UnsafeCell;

    const HEAP_SIZE: usize = 64 * 1024; // 64KB

    #[repr(C, align(16))]
    struct Heap {
        data: [u8; HEAP_SIZE],
    }

    pub struct BumpAlloc {
        heap: UnsafeCell<Heap>,
        next: UnsafeCell<usize>,
    }

    unsafe impl Sync for BumpAlloc {}

    impl BumpAlloc {
        pub const fn new() -> Self {
            BumpAlloc {
                heap: UnsafeCell::new(Heap { data: [0; HEAP_SIZE] }),
                next: UnsafeCell::new(0),
            }
        }
    }

    unsafe impl GlobalAlloc for BumpAlloc {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let next = &mut *self.next.get();
            let heap = &mut *self.heap.get();

            let align = layout.align();
            let size = layout.size();

            let aligned = (*next + align - 1) & !(align - 1);
            let new_next = aligned + size;

            if new_next > HEAP_SIZE {
                return core::ptr::null_mut();
            }

            *next = new_next;
            heap.data.as_mut_ptr().add(aligned)
        }

        unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
            // bump allocator doesn't deallocate
        }
    }
}

#[global_allocator]
static ALLOCATOR: allocator::BumpAlloc = allocator::BumpAlloc::new();

use alloc::vec::Vec;
use subtle::ConstantTimeEq;

// === host imports ===
// all hardware operations delegated to host

#[polkavm_derive::polkavm_import]
extern "C" {
    // database operations
    fn host_db_get(key_ptr: u32, key_len: u32, val_ptr: u32, val_cap: u32) -> u32;
    fn host_db_set(key_ptr: u32, key_len: u32, val_ptr: u32, val_len: u32);
    fn host_db_del(key_ptr: u32, key_len: u32);

    // crypto operations (host can use TPM or software)
    fn host_seal(data_ptr: u32, data_len: u32, sealed_ptr: u32, sealed_cap: u32) -> u32;
    fn host_unseal(sealed_ptr: u32, sealed_len: u32, data_ptr: u32, data_cap: u32) -> u32;
    fn host_sign(data_ptr: u32, data_len: u32, sig_ptr: u32);

    // node info
    fn host_get_index() -> u32;

    // === networking ===
    // networking runs inside sandbox - untrusted input parsing is contained
    // host just provides socket operations, guest parses all network data

    // tcp: connect to host:port, returns fd or 0 on error
    // addr format: "host:port" as bytes
    fn host_tcp_connect(addr_ptr: u32, addr_len: u32, timeout_ms: u32) -> u32;

    // tcp: read from socket, returns bytes read or 0 on error/eof
    fn host_tcp_read(fd: u32, buf_ptr: u32, buf_cap: u32, timeout_ms: u32) -> u32;

    // tcp: write to socket, returns bytes written or 0 on error
    fn host_tcp_write(fd: u32, buf_ptr: u32, buf_len: u32) -> u32;

    // tcp: close socket
    fn host_tcp_close(fd: u32);

    // dns: resolve hostname to ip, returns length written or 0 on error
    // result format: 4 bytes for ipv4 or 16 bytes for ipv6
    fn host_dns_resolve(name_ptr: u32, name_len: u32, result_ptr: u32, result_cap: u32) -> u32;

    // === TLS networking ===
    // host terminates TLS - guest sees plaintext but wire is encrypted

    // tls: connect to host:port with TLS, returns fd or 0 on error
    fn host_tls_connect(host_ptr: u32, host_len: u32, port: u32, timeout_ms: u32) -> u32;

    // tls: read plaintext from TLS connection
    fn host_tls_read(fd: u32, buf_ptr: u32, buf_cap: u32) -> u32;

    // tls: write plaintext to TLS connection
    fn host_tls_write(fd: u32, buf_ptr: u32, buf_len: u32) -> u32;

    // tls: close TLS connection
    fn host_tls_close(fd: u32);
}

// === result codes ===

const OK: u32 = 0;
const ERR_ALREADY_REGISTERED: u32 = 1;
const ERR_NOT_FOUND: u32 = 2;
const ERR_LOCKED_OUT: u32 = 3;
const ERR_WRONG_PIN: u32 = 4;
const ERR_CRYPTO: u32 = 5;

// === helper functions ===

fn db_get(key: &[u8]) -> Option<Vec<u8>> {
    let mut buf = [0u8; 4096];
    let len = unsafe {
        host_db_get(
            key.as_ptr() as u32,
            key.len() as u32,
            buf.as_mut_ptr() as u32,
            buf.len() as u32,
        )
    };
    if len == 0 {
        None
    } else {
        Some(buf[..len as usize].to_vec())
    }
}

fn db_set(key: &[u8], val: &[u8]) {
    unsafe {
        host_db_set(
            key.as_ptr() as u32,
            key.len() as u32,
            val.as_ptr() as u32,
            val.len() as u32,
        )
    }
}

fn db_del(key: &[u8]) {
    unsafe { host_db_del(key.as_ptr() as u32, key.len() as u32) }
}

fn seal(data: &[u8]) -> Vec<u8> {
    let mut buf = [0u8; 4096];
    let len = unsafe {
        host_seal(
            data.as_ptr() as u32,
            data.len() as u32,
            buf.as_mut_ptr() as u32,
            buf.len() as u32,
        )
    };
    buf[..len as usize].to_vec()
}

fn unseal(sealed: &[u8]) -> Option<Vec<u8>> {
    let mut buf = [0u8; 4096];
    let len = unsafe {
        host_unseal(
            sealed.as_ptr() as u32,
            sealed.len() as u32,
            buf.as_mut_ptr() as u32,
            buf.len() as u32,
        )
    };
    if len == 0 {
        None
    } else {
        Some(buf[..len as usize].to_vec())
    }
}

fn sign(data: &[u8]) -> [u8; 64] {
    let mut sig = [0u8; 64];
    unsafe { host_sign(data.as_ptr() as u32, data.len() as u32, sig.as_mut_ptr() as u32) };
    sig
}

fn get_index() -> u8 {
    unsafe { host_get_index() as u8 }
}

// === networking helpers ===

/// tcp socket handle
#[derive(Clone, Copy)]
pub struct TcpSocket(u32);

impl TcpSocket {
    /// connect to address (e.g. "example.com:80")
    pub fn connect(addr: &str, timeout_ms: u32) -> Option<Self> {
        let fd = unsafe {
            host_tcp_connect(addr.as_ptr() as u32, addr.len() as u32, timeout_ms)
        };
        if fd == 0 {
            None
        } else {
            Some(TcpSocket(fd))
        }
    }

    /// read into buffer, returns bytes read
    pub fn read(&self, buf: &mut [u8], timeout_ms: u32) -> usize {
        unsafe {
            host_tcp_read(self.0, buf.as_mut_ptr() as u32, buf.len() as u32, timeout_ms) as usize
        }
    }

    /// write buffer, returns bytes written
    pub fn write(&self, buf: &[u8]) -> usize {
        unsafe { host_tcp_write(self.0, buf.as_ptr() as u32, buf.len() as u32) as usize }
    }

    /// write all data
    pub fn write_all(&self, mut buf: &[u8]) -> bool {
        while !buf.is_empty() {
            let n = self.write(buf);
            if n == 0 {
                return false;
            }
            buf = &buf[n..];
        }
        true
    }

    /// close the socket
    pub fn close(self) {
        unsafe { host_tcp_close(self.0) }
    }
}

/// resolve hostname to ip addresses
pub fn dns_resolve(name: &str) -> Option<[u8; 4]> {
    let mut result = [0u8; 16];
    let len = unsafe {
        host_dns_resolve(
            name.as_ptr() as u32,
            name.len() as u32,
            result.as_mut_ptr() as u32,
            result.len() as u32,
        )
    };
    if len == 4 {
        Some([result[0], result[1], result[2], result[3]])
    } else {
        None
    }
}

/// TLS socket handle - host terminates TLS, guest sees plaintext
#[derive(Clone, Copy)]
pub struct TlsSocket(u32);

impl TlsSocket {
    /// connect to host:port with TLS
    pub fn connect(host: &str, port: u16, timeout_ms: u32) -> Option<Self> {
        let fd = unsafe {
            host_tls_connect(host.as_ptr() as u32, host.len() as u32, port as u32, timeout_ms)
        };
        if fd == 0 {
            None
        } else {
            Some(TlsSocket(fd))
        }
    }

    /// read plaintext into buffer
    pub fn read(&self, buf: &mut [u8]) -> usize {
        unsafe { host_tls_read(self.0, buf.as_mut_ptr() as u32, buf.len() as u32) as usize }
    }

    /// write plaintext
    pub fn write(&self, buf: &[u8]) -> usize {
        unsafe { host_tls_write(self.0, buf.as_ptr() as u32, buf.len() as u32) as usize }
    }

    /// write all data
    pub fn write_all(&self, mut buf: &[u8]) -> bool {
        while !buf.is_empty() {
            let n = self.write(buf);
            if n == 0 {
                return false;
            }
            buf = &buf[n..];
        }
        true
    }

    /// close connection
    pub fn close(self) {
        unsafe { host_tls_close(self.0) }
    }
}

/// https get request, returns response body
pub fn https_get(url: &str, timeout_ms: u32) -> Option<Vec<u8>> {
    // parse url: https://host:port/path or https://host/path
    let url = url.strip_prefix("https://").unwrap_or(url);
    let (host_port, path) = match url.find('/') {
        Some(i) => (&url[..i], &url[i..]),
        None => (url, "/"),
    };

    let (host, port): (&str, u16) = if let Some(i) = host_port.find(':') {
        let port_str = &host_port[i + 1..];
        let port = port_str.parse().ok()?;
        (&host_port[..i], port)
    } else {
        (host_port, 443)
    };

    let sock = TlsSocket::connect(host, port, timeout_ms)?;

    // send request
    let mut req = Vec::new();
    req.extend_from_slice(b"GET ");
    req.extend_from_slice(path.as_bytes());
    req.extend_from_slice(b" HTTP/1.1\r\nHost: ");
    req.extend_from_slice(host.as_bytes());
    req.extend_from_slice(b"\r\nConnection: close\r\n\r\n");

    if !sock.write_all(&req) {
        sock.close();
        return None;
    }

    // read response
    let mut response = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        let n = sock.read(&mut buf);
        if n == 0 {
            break;
        }
        response.extend_from_slice(&buf[..n]);
    }
    sock.close();

    // find body after \r\n\r\n
    let header_end = response.windows(4).position(|w| w == b"\r\n\r\n")?;

    Some(response[header_end + 4..].to_vec())
}

/// simple http get request, returns response body
pub fn http_get(url: &str, timeout_ms: u32) -> Option<Vec<u8>> {
    // parse url: http://host:port/path or http://host/path
    let url = url.strip_prefix("http://").unwrap_or(url);
    let (host_port, path) = match url.find('/') {
        Some(i) => (&url[..i], &url[i..]),
        None => (url, "/"),
    };

    let (host, port) = if let Some(i) = host_port.find(':') {
        (&host_port[..i], &host_port[i + 1..])
    } else {
        (host_port, "80")
    };

    // build address
    let mut addr = Vec::new();
    addr.extend_from_slice(host.as_bytes());
    addr.push(b':');
    addr.extend_from_slice(port.as_bytes());

    let addr_str = unsafe { core::str::from_utf8_unchecked(&addr) };
    let sock = TcpSocket::connect(addr_str, timeout_ms)?;

    // send request
    let mut req = Vec::new();
    req.extend_from_slice(b"GET ");
    req.extend_from_slice(path.as_bytes());
    req.extend_from_slice(b" HTTP/1.1\r\nHost: ");
    req.extend_from_slice(host.as_bytes());
    req.extend_from_slice(b"\r\nConnection: close\r\n\r\n");

    if !sock.write_all(&req) {
        sock.close();
        return None;
    }

    // read response
    let mut response = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        let n = sock.read(&mut buf, timeout_ms);
        if n == 0 {
            break;
        }
        response.extend_from_slice(&buf[..n]);
    }
    sock.close();

    // find body after \r\n\r\n
    let header_end = response
        .windows(4)
        .position(|w| w == b"\r\n\r\n")?;

    Some(response[header_end + 4..].to_vec())
}

// === registration storage ===
// format: unlock_tag(16) || allowed_guesses(4) || attempted_guesses(4) || sealed_share(var)

fn pack_registration(unlock_tag: &[u8; 16], allowed_guesses: u32, attempted: u32, sealed: &[u8]) -> Vec<u8> {
    let mut data = Vec::with_capacity(16 + 4 + 4 + sealed.len());
    data.extend_from_slice(unlock_tag);
    data.extend_from_slice(&allowed_guesses.to_le_bytes());
    data.extend_from_slice(&attempted.to_le_bytes());
    data.extend_from_slice(sealed);
    data
}

fn unpack_registration(data: &[u8]) -> Option<([u8; 16], u32, u32, &[u8])> {
    if data.len() < 24 {
        return None;
    }
    let mut tag = [0u8; 16];
    tag.copy_from_slice(&data[..16]);
    let allowed = u32::from_le_bytes([data[16], data[17], data[18], data[19]]);
    let attempted = u32::from_le_bytes([data[20], data[21], data[22], data[23]]);
    Some((tag, allowed, attempted, &data[24..]))
}

// === exported functions ===

/// register a share
/// input: user_id(32) || unlock_tag(16) || allowed_guesses(4) || share_len(4) || share(var)
/// output: result(4) || node_index(1) || signature(64)
#[polkavm_derive::polkavm_export]
extern "C" fn register(input_ptr: u32, input_len: u32, output_ptr: u32) -> u32 {
    let input = unsafe { core::slice::from_raw_parts(input_ptr as *const u8, input_len as usize) };

    if input.len() < 32 + 16 + 4 + 4 {
        return ERR_CRYPTO;
    }

    let user_id = &input[..32];
    let unlock_tag: &[u8; 16] = input[32..48].try_into().unwrap();
    let allowed_guesses = u32::from_le_bytes([input[48], input[49], input[50], input[51]]);
    let share_len = u32::from_le_bytes([input[52], input[53], input[54], input[55]]) as usize;

    if input.len() < 56 + share_len {
        return ERR_CRYPTO;
    }
    let share = &input[56..56 + share_len];

    // check if already registered
    if db_get(user_id).is_some() {
        return ERR_ALREADY_REGISTERED;
    }

    // seal the share (host does crypto - can use TPM)
    let sealed = seal(share);

    // store registration
    let reg_data = pack_registration(unlock_tag, allowed_guesses.min(10), 0, &sealed);
    db_set(user_id, &reg_data);

    // sign user_id || unlock_tag (host does crypto - can use TPM)
    let mut sig_data = [0u8; 48];
    sig_data[..32].copy_from_slice(user_id);
    sig_data[32..48].copy_from_slice(unlock_tag);
    let signature = sign(&sig_data);

    // write output: result(4) || node_index(1) || signature(64)
    let output = unsafe { core::slice::from_raw_parts_mut(output_ptr as *mut u8, 69) };
    output[..4].copy_from_slice(&OK.to_le_bytes());
    output[4] = get_index();
    output[5..69].copy_from_slice(&signature);

    69
}

/// recover a share
/// input: user_id(32) || unlock_tag(16)
/// output: result(4) || guesses_remaining(4) || share_len(4) || node_index(1) || share(var)
#[polkavm_derive::polkavm_export]
extern "C" fn recover(input_ptr: u32, input_len: u32, output_ptr: u32) -> u32 {
    let input = unsafe { core::slice::from_raw_parts(input_ptr as *const u8, input_len as usize) };

    if input.len() < 48 {
        return 0;
    }

    let user_id = &input[..32];
    let unlock_tag: [u8; 16] = input[32..48].try_into().unwrap();

    let reg_data = match db_get(user_id) {
        Some(d) => d,
        None => {
            let output = unsafe { core::slice::from_raw_parts_mut(output_ptr as *mut u8, 4) };
            output[..4].copy_from_slice(&ERR_NOT_FOUND.to_le_bytes());
            return 4;
        }
    };

    let (stored_tag, allowed, attempted, sealed) = match unpack_registration(&reg_data) {
        Some(r) => r,
        None => {
            let output = unsafe { core::slice::from_raw_parts_mut(output_ptr as *mut u8, 4) };
            output[..4].copy_from_slice(&ERR_CRYPTO.to_le_bytes());
            return 4;
        }
    };

    // check if locked out
    if attempted >= allowed {
        db_del(user_id);
        let output = unsafe { core::slice::from_raw_parts_mut(output_ptr as *mut u8, 8) };
        output[..4].copy_from_slice(&ERR_LOCKED_OUT.to_le_bytes());
        output[4..8].copy_from_slice(&0u32.to_le_bytes());
        return 8;
    }

    // check unlock tag - MUST be constant-time to prevent timing oracle
    let tag_matches: bool = unlock_tag.ct_eq(&stored_tag).into();
    if !tag_matches {
        // increment attempts
        let new_attempted = attempted + 1;
        let new_reg = pack_registration(&stored_tag, allowed, new_attempted, sealed);
        db_set(user_id, &new_reg);

        let remaining = allowed.saturating_sub(new_attempted);
        let output = unsafe { core::slice::from_raw_parts_mut(output_ptr as *mut u8, 8) };
        output[..4].copy_from_slice(&ERR_WRONG_PIN.to_le_bytes());
        output[4..8].copy_from_slice(&remaining.to_le_bytes());
        return 8;
    }

    // unseal share (host does crypto - can use TPM)
    let share = match unseal(sealed) {
        Some(s) => s,
        None => {
            let output = unsafe { core::slice::from_raw_parts_mut(output_ptr as *mut u8, 4) };
            output[..4].copy_from_slice(&ERR_CRYPTO.to_le_bytes());
            return 4;
        }
    };

    let remaining = allowed - attempted;
    let output_len = 13 + share.len();
    let output = unsafe { core::slice::from_raw_parts_mut(output_ptr as *mut u8, output_len) };
    output[..4].copy_from_slice(&OK.to_le_bytes());
    output[4..8].copy_from_slice(&remaining.to_le_bytes());
    output[8..12].copy_from_slice(&(share.len() as u32).to_le_bytes());
    output[12] = get_index();
    output[13..13 + share.len()].copy_from_slice(&share);

    output_len as u32
}

/// check status
/// input: user_id(32)
/// output: registered(1) || guesses_remaining(4) || locked(1)
#[polkavm_derive::polkavm_export]
extern "C" fn status(input_ptr: u32, _input_len: u32, output_ptr: u32) -> u32 {
    let user_id = unsafe { core::slice::from_raw_parts(input_ptr as *const u8, 32) };

    let output = unsafe { core::slice::from_raw_parts_mut(output_ptr as *mut u8, 6) };

    match db_get(user_id) {
        None => {
            output[0] = 0; // not registered
            output[1..5].copy_from_slice(&0u32.to_le_bytes());
            output[5] = 0; // not locked
        }
        Some(reg_data) => {
            if let Some((_, allowed, attempted, _)) = unpack_registration(&reg_data) {
                output[0] = 1; // registered
                output[1..5].copy_from_slice(&allowed.saturating_sub(attempted).to_le_bytes());
                output[5] = if attempted >= allowed { 1 } else { 0 };
            } else {
                output[0] = 0;
                output[1..5].copy_from_slice(&0u32.to_le_bytes());
                output[5] = 0;
            }
        }
    }

    6
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe {
        core::arch::asm!("unimp", options(noreturn));
    }
}
