//! ghettobox-vault-pvm - polkavm sandboxed vault host
//!
//! runs the vault guest in a polkavm sandbox with:
//! - no direct filesystem access
//! - no network access
//! - controlled storage via host imports

mod pss;

use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Nonce,
};
use clap::Parser;
use ed25519_dalek::{Signer, SigningKey};
use metrics::{counter, gauge, histogram};
use metrics_exporter_prometheus::PrometheusBuilder;
use polkavm::{BackendKind, Caller, Config, Engine, Linker, Module, ModuleConfig, ProgramBlob};
use serde::{Deserialize, Serialize};
use rand::rngs::OsRng;
use rand::RngCore;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

/// network security policy - allowlist model
/// only global unicast addresses on permitted ports
#[derive(Clone)]
struct NetworkPolicy {
    /// maximum simultaneous sockets per vm instance
    max_sockets: usize,
    /// maximum buffer size for read operations
    max_read_buffer: usize,
    /// maximum total bytes transferred per connection
    max_bytes_per_conn: usize,
    /// allowed destination ports (empty = all allowed)
    allowed_ports: Vec<u16>,
}

impl Default for NetworkPolicy {
    fn default() -> Self {
        Self {
            max_sockets: 8,
            max_read_buffer: 64 * 1024,      // 64KB max read buffer
            max_bytes_per_conn: 1024 * 1024, // 1MB max per connection
            allowed_ports: vec![80, 443, 8080, 8443],
        }
    }
}

impl NetworkPolicy {
    fn is_addr_allowed(&self, addr: &SocketAddr) -> Result<(), &'static str> {
        // check port allowlist
        if !self.allowed_ports.is_empty() && !self.allowed_ports.contains(&addr.port()) {
            return Err("port not in allowlist");
        }

        match addr.ip() {
            IpAddr::V4(v4) => self.check_ipv4(v4),
            IpAddr::V6(v6) => self.check_ipv6(v6),
        }
    }

    /// ALLOWLIST: only permit global unicast IPv4
    fn check_ipv4(&self, ip: Ipv4Addr) -> Result<(), &'static str> {
        let o = ip.octets();

        // block all non-global ranges per IANA
        if o[0] == 0                                          // 0.0.0.0/8 - current network
            || o[0] == 10                                     // 10.0.0.0/8 - private
            || o[0] == 127                                    // 127.0.0.0/8 - loopback
            || (o[0] == 100 && o[1] >= 64 && o[1] <= 127)     // 100.64.0.0/10 - CGNAT
            || (o[0] == 169 && o[1] == 254)                   // 169.254.0.0/16 - link-local
            || (o[0] == 172 && o[1] >= 16 && o[1] <= 31)      // 172.16.0.0/12 - private
            || (o[0] == 192 && o[1] == 0 && o[2] == 0)        // 192.0.0.0/24 - IETF protocol
            || (o[0] == 192 && o[1] == 0 && o[2] == 2)        // 192.0.2.0/24 - TEST-NET-1
            || (o[0] == 192 && o[1] == 88 && o[2] == 99)      // 192.88.99.0/24 - 6to4 anycast
            || (o[0] == 192 && o[1] == 168)                   // 192.168.0.0/16 - private
            || (o[0] == 198 && (o[1] == 18 || o[1] == 19))    // 198.18.0.0/15 - benchmarking
            || (o[0] == 198 && o[1] == 51 && o[2] == 100)     // 198.51.100.0/24 - TEST-NET-2
            || (o[0] == 203 && o[1] == 0 && o[2] == 113)      // 203.0.113.0/24 - TEST-NET-3
            || o[0] >= 224                                    // 224.0.0.0/4+ - multicast/reserved
        {
            return Err("non-global ipv4 blocked");
        }

        Ok(())
    }

    fn check_ipv6(&self, ip: Ipv6Addr) -> Result<(), &'static str> {
        let segments = ip.segments();

        // 6to4 tunneling 2002::/16 - embeds IPv4, must validate the embedded address
        if segments[0] == 0x2002 {
            let embedded_v4 = Ipv4Addr::new(
                (segments[1] >> 8) as u8,
                (segments[1] & 0xff) as u8,
                (segments[2] >> 8) as u8,
                (segments[2] & 0xff) as u8,
            );
            return self.check_ipv4(embedded_v4);
        }

        // teredo 2001:0000::/32 - embeds IPv4 XOR'd with 0xffffffff
        if segments[0] == 0x2001 && segments[1] == 0x0000 {
            let obfuscated = ((segments[6] as u32) << 16) | (segments[7] as u32);
            let embedded_v4 = Ipv4Addr::from(obfuscated ^ 0xffffffff);
            return self.check_ipv4(embedded_v4);
        }

        // ALLOWLIST: only permit global unicast (2000::/3)
        // this automatically blocks: loopback, link-local, unique-local,
        // site-local, multicast, ipv4-mapped, ipv4-compatible, etc.
        if (segments[0] & 0xe000) != 0x2000 {
            return Err("non-global ipv6 blocked");
        }

        Ok(())
    }
}

/// per-socket state tracking bytes transferred
struct SocketState {
    stream: TcpStream,
    bytes_read: usize,
    bytes_written: usize,
    deadline: Instant,
}

/// TLS socket state - host terminates TLS, guest sees plaintext
struct TlsSocketState {
    stream: rustls::StreamOwned<rustls::ClientConnection, TcpStream>,
    bytes_read: usize,
    bytes_written: usize,
    deadline: Instant,
}
use tower_http::cors::CorsLayer;
use tracing::info;

pub type VmError = Box<dyn std::error::Error + Send + Sync>;

/// derive seal key from signing key using HKDF-SHA256
/// returns Zeroizing wrapper to ensure key is cleared from memory
fn derive_seal_key(signing_key: &SigningKey) -> Zeroizing<[u8; 32]> {
    use hkdf::Hkdf;
    use sha2::Sha256;

    const SALT: &[u8] = b"ghettobox-vault-v1";

    let hk = Hkdf::<Sha256>::new(Some(SALT), signing_key.as_bytes());
    let mut seal_key = Zeroizing::new([0u8; 32]);
    hk.expand(b"seal-key", seal_key.as_mut())
        .expect("32 bytes is valid for HKDF-SHA256");
    seal_key
}

/// create TLS client config with system roots
fn make_tls_config() -> Arc<rustls::ClientConfig> {
    let root_store = rustls::RootCertStore::from_iter(
        webpki_roots::TLS_SERVER_ROOTS.iter().cloned()
    );

    let config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();

    Arc::new(config)
}

/// ghettobox-vault-pvm - polkavm sandboxed vault
#[derive(Parser)]
#[command(name = "ghettobox-vault-pvm")]
#[command(about = "ghettobox vault - polkavm sandboxed")]
#[command(version)]
struct Args {
    /// port to listen on
    #[arg(short, long, default_value = "4200")]
    port: u16,

    /// path to vault guest blob
    #[arg(short, long, default_value = "vault-guest.polkavm")]
    blob: String,

    /// data directory
    #[arg(short, long)]
    data_dir: Option<String>,

    /// vault index (1-3)
    #[arg(short, long, default_value = "1")]
    index: u8,

    /// bind address
    #[arg(long, default_value = "0.0.0.0")]
    bind: String,

    /// metrics port
    #[arg(long)]
    metrics_port: Option<u16>,
}

/// state passed to polkavm host functions
struct VmState {
    db: sled::Db,
    signing_key: SigningKey,
    seal_key: Zeroizing<[u8; 32]>,
    index: u8,
    // networking state
    sockets: HashMap<u32, SocketState>,
    tls_sockets: HashMap<u32, TlsSocketState>,
    next_fd: u32,
    net_policy: NetworkPolicy,
    tls_config: Arc<rustls::ClientConfig>,
}

/// app state for http handlers
struct AppState {
    #[allow(dead_code)]
    engine: Engine,
    module: Module,
    db: sled::Db,
    signing_key: SigningKey,
    index: u8,
    tls_config: Arc<rustls::ClientConfig>,
}

// === request/response types ===

#[derive(Deserialize)]
struct RegisterRequest {
    user_id: [u8; 32],
    unlock_tag: [u8; 16],
    encrypted_share: Vec<u8>,
    allowed_guesses: u32,
}

#[derive(Serialize)]
struct RegisterResponse {
    ok: bool,
    node_index: u8,
    signature: String,
}

#[derive(Deserialize)]
struct RecoverRequest {
    user_id: [u8; 32],
    unlock_tag: [u8; 16],
}

#[derive(Serialize)]
struct RecoverResponse {
    ok: bool,
    share: Option<ShareData>,
    guesses_remaining: u32,
    error: Option<String>,
}

#[derive(Serialize)]
struct ShareData {
    index: u8,
    data: String,
}

#[derive(Serialize)]
struct StatusResponse {
    registered: bool,
    guesses_remaining: u32,
    locked: bool,
}

#[derive(Serialize)]
struct NodeInfoResponse {
    version: String,
    index: u8,
    pubkey: String,
    registrations: u64,
    mode: String,
}

// result codes from guest
const OK: u32 = 0;
const ERR_ALREADY_REGISTERED: u32 = 1;
const ERR_NOT_FOUND: u32 = 2;
const ERR_LOCKED_OUT: u32 = 3;
const ERR_WRONG_PIN: u32 = 4;

/// create linker with host functions
fn create_linker() -> Result<Linker<VmState, VmError>, polkavm::Error> {
    let mut linker = Linker::new();

    // host_db_get: (key_ptr, key_len, val_ptr, val_cap) -> val_len
    linker.define_typed(
        "host_db_get",
        |caller: Caller<VmState>, key_ptr: u32, key_len: u32, val_ptr: u32, val_cap: u32| -> Result<u32, VmError> {
            let key = caller.instance.read_memory(key_ptr, key_len)?;
            match caller.user_data.db.get(&key)? {
                Some(val) => {
                    let len = val.len().min(val_cap as usize);
                    caller.instance.write_memory(val_ptr, &val[..len])?;
                    Ok(len as u32)
                }
                None => Ok(0),
            }
        },
    )?;

    // host_db_set: (key_ptr, key_len, val_ptr, val_len)
    // limits: key <= 256 bytes, value <= 64KB
    linker.define_typed(
        "host_db_set",
        |caller: Caller<VmState>, key_ptr: u32, key_len: u32, val_ptr: u32, val_len: u32| -> Result<(), VmError> {
            if key_len > 256 || val_len > 65536 {
                return Ok(()); // silently reject oversized
            }
            let key = caller.instance.read_memory(key_ptr, key_len)?;
            let val = caller.instance.read_memory(val_ptr, val_len)?;
            caller.user_data.db.insert(key, val)?;
            Ok(())
        },
    )?;

    // host_db_del: (key_ptr, key_len)
    linker.define_typed(
        "host_db_del",
        |caller: Caller<VmState>, key_ptr: u32, key_len: u32| -> Result<(), VmError> {
            let key = caller.instance.read_memory(key_ptr, key_len)?;
            caller.user_data.db.remove(key)?;
            Ok(())
        },
    )?;

    // host_seal: (data_ptr, data_len, sealed_ptr, sealed_cap) -> sealed_len
    // encrypts data with chacha20poly1305 using node's seal key
    // output format: nonce(12) || ciphertext(data_len + 16)
    // limit: data <= 64KB
    linker.define_typed(
        "host_seal",
        |caller: Caller<VmState>, data_ptr: u32, data_len: u32, sealed_ptr: u32, sealed_cap: u32| -> Result<u32, VmError> {
            if data_len > 65536 {
                return Ok(0);
            }
            let data = caller.instance.read_memory(data_ptr, data_len)?;

            let cipher = ChaCha20Poly1305::new_from_slice(caller.user_data.seal_key.as_ref())?;
            let mut nonce_bytes = [0u8; 12];
            OsRng.fill_bytes(&mut nonce_bytes);
            let nonce = Nonce::from_slice(&nonce_bytes);

            let ciphertext = cipher.encrypt(nonce, data.as_ref())
                .map_err(|e| format!("seal failed: {}", e))?;

            let sealed_len = 12 + ciphertext.len();
            if sealed_len > sealed_cap as usize {
                return Ok(0);
            }

            let mut sealed = Vec::with_capacity(sealed_len);
            sealed.extend_from_slice(&nonce_bytes);
            sealed.extend_from_slice(&ciphertext);

            caller.instance.write_memory(sealed_ptr, &sealed)?;
            Ok(sealed_len as u32)
        },
    )?;

    // host_unseal: (sealed_ptr, sealed_len, data_ptr, data_cap) -> data_len
    // decrypts sealed data, returns 0 on failure
    linker.define_typed(
        "host_unseal",
        |caller: Caller<VmState>, sealed_ptr: u32, sealed_len: u32, data_ptr: u32, data_cap: u32| -> Result<u32, VmError> {
            if sealed_len < 12 + 16 {
                return Ok(0);
            }
            let sealed = caller.instance.read_memory(sealed_ptr, sealed_len)?;

            let nonce = Nonce::from_slice(&sealed[..12]);
            let ciphertext = &sealed[12..];

            let cipher = ChaCha20Poly1305::new_from_slice(caller.user_data.seal_key.as_ref())?;
            let plaintext = match cipher.decrypt(nonce, ciphertext) {
                Ok(p) => p,
                Err(_) => return Ok(0),
            };

            if plaintext.len() > data_cap as usize {
                return Ok(0);
            }

            caller.instance.write_memory(data_ptr, &plaintext)?;
            Ok(plaintext.len() as u32)
        },
    )?;

    // host_sign: (data_ptr, data_len, sig_ptr)
    // signs data with ed25519 and writes 64-byte signature
    linker.define_typed(
        "host_sign",
        |caller: Caller<VmState>, data_ptr: u32, data_len: u32, sig_ptr: u32| -> Result<(), VmError> {
            let data = caller.instance.read_memory(data_ptr, data_len)?;
            let signature = caller.user_data.signing_key.sign(&data);
            caller.instance.write_memory(sig_ptr, &signature.to_bytes())?;
            Ok(())
        },
    )?;

    // host_get_index: () -> u32
    linker.define_typed(
        "host_get_index",
        |caller: Caller<VmState>| -> u32 {
            caller.user_data.index as u32
        },
    )?;

    // === networking host functions ===
    // security: all limits enforced by host, guest is untrusted

    // host_tcp_connect: (addr_ptr, addr_len, timeout_ms) -> fd (0 = error)
    linker.define_typed(
        "host_tcp_connect",
        |caller: Caller<VmState>, addr_ptr: u32, addr_len: u32, timeout_ms: u32| -> Result<u32, VmError> {
            // limit address length to prevent DoS
            if addr_len > 256 {
                return Ok(0);
            }

            // check socket limit
            if caller.user_data.sockets.len() >= caller.user_data.net_policy.max_sockets {
                return Ok(0);
            }

            let addr_bytes = caller.instance.read_memory(addr_ptr, addr_len)?;
            let addr_str = match std::str::from_utf8(&addr_bytes) {
                Ok(s) => s,
                Err(_) => return Ok(0),
            };

            // resolve and validate destination BEFORE connecting
            let resolved: SocketAddr = match addr_str.to_socket_addrs() {
                Ok(mut addrs) => match addrs.next() {
                    Some(a) => a,
                    None => return Ok(0),
                },
                Err(_) => return Ok(0),
            };

            // enforce network policy
            if let Err(_reason) = caller.user_data.net_policy.is_addr_allowed(&resolved) {
                // don't leak why it failed to guest
                return Ok(0);
            }

            // cap timeout to 30 seconds max
            let timeout_ms = timeout_ms.min(30_000);
            let timeout = Duration::from_millis(timeout_ms as u64);

            let stream = match TcpStream::connect_timeout(&resolved, timeout) {
                Ok(s) => s,
                Err(_) => return Ok(0),
            };

            // set per-operation timeouts (shorter than deadline)
            let op_timeout = Duration::from_millis((timeout_ms / 2).max(1000) as u64);
            stream.set_read_timeout(Some(op_timeout)).ok();
            stream.set_write_timeout(Some(op_timeout)).ok();
            stream.set_nodelay(true).ok();

            // allocate fd with overflow check
            let fd = caller.user_data.next_fd;
            caller.user_data.next_fd = caller.user_data.next_fd.checked_add(1).unwrap_or(1);

            // skip fd 0 (reserved for error)
            let fd = if fd == 0 {
                caller.user_data.next_fd = 2;
                1
            } else {
                fd
            };

            let state = SocketState {
                stream,
                bytes_read: 0,
                bytes_written: 0,
                deadline: Instant::now() + Duration::from_secs(60), // 60s total deadline
            };

            caller.user_data.sockets.insert(fd, state);
            Ok(fd)
        },
    )?;

    // host_tcp_read: (fd, buf_ptr, buf_cap, timeout_ms) -> bytes_read
    linker.define_typed(
        "host_tcp_read",
        |caller: Caller<VmState>, fd: u32, buf_ptr: u32, buf_cap: u32, _timeout_ms: u32| -> Result<u32, VmError> {
            let policy = caller.user_data.net_policy.clone();

            let sock_state = match caller.user_data.sockets.get_mut(&fd) {
                Some(s) => s,
                None => return Ok(0),
            };

            // check deadline
            if Instant::now() > sock_state.deadline {
                // deadline exceeded, close socket
                caller.user_data.sockets.remove(&fd);
                return Ok(0);
            }

            // enforce buffer size limit
            let buf_cap = (buf_cap as usize).min(policy.max_read_buffer);

            // check bytes limit
            let remaining = policy.max_bytes_per_conn.saturating_sub(sock_state.bytes_read);
            if remaining == 0 {
                return Ok(0);
            }
            let buf_cap = buf_cap.min(remaining);

            let mut buf = vec![0u8; buf_cap];
            let n = match sock_state.stream.read(&mut buf) {
                Ok(n) => n,
                Err(_) => return Ok(0),
            };

            sock_state.bytes_read += n;
            caller.instance.write_memory(buf_ptr, &buf[..n])?;
            Ok(n as u32)
        },
    )?;

    // host_tcp_write: (fd, buf_ptr, buf_len) -> bytes_written
    linker.define_typed(
        "host_tcp_write",
        |caller: Caller<VmState>, fd: u32, buf_ptr: u32, buf_len: u32| -> Result<u32, VmError> {
            let policy = caller.user_data.net_policy.clone();

            // limit write size
            let buf_len = (buf_len as usize).min(policy.max_read_buffer);

            let buf = caller.instance.read_memory(buf_ptr, buf_len as u32)?;

            let sock_state = match caller.user_data.sockets.get_mut(&fd) {
                Some(s) => s,
                None => return Ok(0),
            };

            // check deadline
            if Instant::now() > sock_state.deadline {
                caller.user_data.sockets.remove(&fd);
                return Ok(0);
            }

            // check bytes limit
            let remaining = policy.max_bytes_per_conn.saturating_sub(sock_state.bytes_written);
            if remaining == 0 {
                return Ok(0);
            }
            let to_write = buf.len().min(remaining);

            let n = match sock_state.stream.write(&buf[..to_write]) {
                Ok(n) => n,
                Err(_) => return Ok(0),
            };

            sock_state.bytes_written += n;
            Ok(n as u32)
        },
    )?;

    // host_tcp_close: (fd)
    linker.define_typed(
        "host_tcp_close",
        |caller: Caller<VmState>, fd: u32| -> Result<(), VmError> {
            caller.user_data.sockets.remove(&fd);
            Ok(())
        },
    )?;

    // host_dns_resolve: (name_ptr, name_len, result_ptr, result_cap) -> result_len
    linker.define_typed(
        "host_dns_resolve",
        |caller: Caller<VmState>, name_ptr: u32, name_len: u32, result_ptr: u32, result_cap: u32| -> Result<u32, VmError> {
            // limit name length
            if name_len > 253 {
                return Ok(0);
            }

            let name_bytes = caller.instance.read_memory(name_ptr, name_len)?;
            let name = match std::str::from_utf8(&name_bytes) {
                Ok(s) => s,
                Err(_) => return Ok(0),
            };

            // resolve hostname:port (use port 80 to check policy)
            let lookup = format!("{}:80", name);
            let addr = match lookup.to_socket_addrs() {
                Ok(mut addrs) => addrs.next(),
                Err(_) => return Ok(0),
            };

            let addr = match addr {
                Some(a) => a,
                None => return Ok(0),
            };

            // check policy - don't return IPs for blocked destinations
            if caller.user_data.net_policy.is_addr_allowed(&addr).is_err() {
                return Ok(0);
            }

            let result = match addr {
                SocketAddr::V4(v4) => {
                    let octets = v4.ip().octets();
                    if result_cap >= 4 {
                        caller.instance.write_memory(result_ptr, &octets)?;
                        4
                    } else {
                        0
                    }
                }
                SocketAddr::V6(v6) => {
                    let octets = v6.ip().octets();
                    if result_cap >= 16 {
                        caller.instance.write_memory(result_ptr, &octets)?;
                        16
                    } else {
                        0
                    }
                }
            };

            Ok(result)
        },
    )?;

    // === TLS networking - host terminates TLS, guest sees plaintext ===

    // host_tls_connect: (host_ptr, host_len, port, timeout_ms) -> fd (0 = error)
    linker.define_typed(
        "host_tls_connect",
        |caller: Caller<VmState>, host_ptr: u32, host_len: u32, port: u32, timeout_ms: u32| -> Result<u32, VmError> {
            // limit hostname length
            if host_len > 253 {
                return Ok(0);
            }

            // check socket limit (shared with TCP)
            let total_sockets = caller.user_data.sockets.len() + caller.user_data.tls_sockets.len();
            if total_sockets >= caller.user_data.net_policy.max_sockets {
                return Ok(0);
            }

            let host_bytes = caller.instance.read_memory(host_ptr, host_len)?;
            let hostname = match std::str::from_utf8(&host_bytes) {
                Ok(s) => s,
                Err(_) => return Ok(0),
            };

            // resolve hostname
            let addr_str = format!("{}:{}", hostname, port);
            let resolved: SocketAddr = match addr_str.to_socket_addrs() {
                Ok(mut addrs) => match addrs.next() {
                    Some(a) => a,
                    None => return Ok(0),
                },
                Err(_) => return Ok(0),
            };

            // enforce network policy
            if caller.user_data.net_policy.is_addr_allowed(&resolved).is_err() {
                return Ok(0);
            }

            // cap timeout
            let timeout_ms = timeout_ms.min(30_000);
            let timeout = Duration::from_millis(timeout_ms as u64);

            // TCP connect
            let tcp_stream = match TcpStream::connect_timeout(&resolved, timeout) {
                Ok(s) => s,
                Err(_) => return Ok(0),
            };

            tcp_stream.set_read_timeout(Some(Duration::from_secs(15))).ok();
            tcp_stream.set_write_timeout(Some(Duration::from_secs(15))).ok();
            tcp_stream.set_nodelay(true).ok();

            // TLS handshake
            let server_name = match rustls::pki_types::ServerName::try_from(hostname.to_string()) {
                Ok(sn) => sn,
                Err(_) => return Ok(0),
            };

            let tls_conn = match rustls::ClientConnection::new(
                caller.user_data.tls_config.clone(),
                server_name,
            ) {
                Ok(c) => c,
                Err(_) => return Ok(0),
            };

            let mut tls_stream = rustls::StreamOwned::new(tls_conn, tcp_stream);

            // complete handshake by doing a zero-byte write
            use std::io::Write;
            if tls_stream.flush().is_err() {
                return Ok(0);
            }

            // allocate fd
            let fd = caller.user_data.next_fd;
            caller.user_data.next_fd = caller.user_data.next_fd.checked_add(1).unwrap_or(1);
            let fd = if fd == 0 {
                caller.user_data.next_fd = 2;
                1
            } else {
                fd
            };

            let state = TlsSocketState {
                stream: tls_stream,
                bytes_read: 0,
                bytes_written: 0,
                deadline: Instant::now() + Duration::from_secs(60),
            };

            caller.user_data.tls_sockets.insert(fd, state);
            Ok(fd)
        },
    )?;

    // host_tls_read: (fd, buf_ptr, buf_cap) -> bytes_read
    linker.define_typed(
        "host_tls_read",
        |caller: Caller<VmState>, fd: u32, buf_ptr: u32, buf_cap: u32| -> Result<u32, VmError> {
            let policy = caller.user_data.net_policy.clone();

            let sock_state = match caller.user_data.tls_sockets.get_mut(&fd) {
                Some(s) => s,
                None => return Ok(0),
            };

            // check deadline
            if Instant::now() > sock_state.deadline {
                caller.user_data.tls_sockets.remove(&fd);
                return Ok(0);
            }

            // enforce buffer size limit
            let buf_cap = (buf_cap as usize).min(policy.max_read_buffer);

            // check bytes limit
            let remaining = policy.max_bytes_per_conn.saturating_sub(sock_state.bytes_read);
            if remaining == 0 {
                return Ok(0);
            }
            let buf_cap = buf_cap.min(remaining);

            let mut buf = vec![0u8; buf_cap];
            let n = match sock_state.stream.read(&mut buf) {
                Ok(n) => n,
                Err(_) => return Ok(0),
            };

            sock_state.bytes_read += n;
            caller.instance.write_memory(buf_ptr, &buf[..n])?;
            Ok(n as u32)
        },
    )?;

    // host_tls_write: (fd, buf_ptr, buf_len) -> bytes_written
    linker.define_typed(
        "host_tls_write",
        |caller: Caller<VmState>, fd: u32, buf_ptr: u32, buf_len: u32| -> Result<u32, VmError> {
            let policy = caller.user_data.net_policy.clone();

            // limit write size
            let buf_len = (buf_len as usize).min(policy.max_read_buffer);

            let buf = caller.instance.read_memory(buf_ptr, buf_len as u32)?;

            let sock_state = match caller.user_data.tls_sockets.get_mut(&fd) {
                Some(s) => s,
                None => return Ok(0),
            };

            // check deadline
            if Instant::now() > sock_state.deadline {
                caller.user_data.tls_sockets.remove(&fd);
                return Ok(0);
            }

            // check bytes limit
            let remaining = policy.max_bytes_per_conn.saturating_sub(sock_state.bytes_written);
            if remaining == 0 {
                return Ok(0);
            }
            let to_write = buf.len().min(remaining);

            let n = match sock_state.stream.write(&buf[..to_write]) {
                Ok(n) => n,
                Err(_) => return Ok(0),
            };

            sock_state.bytes_written += n;
            Ok(n as u32)
        },
    )?;

    // host_tls_close: (fd)
    linker.define_typed(
        "host_tls_close",
        |caller: Caller<VmState>, fd: u32| -> Result<(), VmError> {
            caller.user_data.tls_sockets.remove(&fd);
            Ok(())
        },
    )?;

    Ok(linker)
}

/// run a guest function with full VM setup
async fn run_guest(
    state: &Arc<Mutex<AppState>>,
    func: &'static str,
    input: Vec<u8>,
) -> Result<Vec<u8>, String> {
    let (module, db, signing_key, index, tls_config) = {
        let s = state.lock().await;
        (s.module.clone(), s.db.clone(), s.signing_key.clone(), s.index, s.tls_config.clone())
    };

    tokio::task::spawn_blocking(move || {
        let linker = create_linker().map_err(|e| e.to_string())?;
        let mut instance = linker.instantiate_pre(&module)
            .map_err(|e| e.to_string())?
            .instantiate()
            .map_err(|e| e.to_string())?;

        let mut vm_state = VmState {
            db,
            signing_key: signing_key.clone(),
            seal_key: derive_seal_key(&signing_key),
            index,
            sockets: HashMap::new(),
            tls_sockets: HashMap::new(),
            next_fd: 1,
            net_policy: NetworkPolicy::default(),
            tls_config,
        };

        // allocate and call
        let input_size = input.len() as u32;
        let input_ptr = instance.sbrk(input_size + 4096)
            .map_err(|e| e.to_string())?
            .ok_or("sbrk failed")?;
        let output_ptr = input_ptr + input_size;

        instance.write_memory(input_ptr, &input).map_err(|e| e.to_string())?;
        let output_len: u32 = instance
            .call_typed_and_get_result(&mut vm_state, func, (input_ptr, input_size, output_ptr))
            .map_err(|e| format!("{:?}", e))?;
        instance.read_memory(output_ptr, output_len).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

// === handlers ===

async fn register(
    State(state): State<Arc<Mutex<AppState>>>,
    Json(req): Json<RegisterRequest>,
) -> Result<Json<RegisterResponse>, (StatusCode, String)> {
    let start = Instant::now();
    counter!("vault_requests_total", "endpoint" => "register").increment(1);

    let mut input = Vec::with_capacity(56 + req.encrypted_share.len());
    input.extend_from_slice(&req.user_id);
    input.extend_from_slice(&req.unlock_tag);
    input.extend_from_slice(&req.allowed_guesses.to_le_bytes());
    input.extend_from_slice(&(req.encrypted_share.len() as u32).to_le_bytes());
    input.extend_from_slice(&req.encrypted_share);

    let output = run_guest(&state, "register", input).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    if output.len() < 4 {
        return Err((StatusCode::INTERNAL_SERVER_ERROR, "invalid response".into()));
    }

    let result = u32::from_le_bytes([output[0], output[1], output[2], output[3]]);

    match result {
        OK => {
            if output.len() < 69 {
                return Err((StatusCode::INTERNAL_SERVER_ERROR, "invalid response length".into()));
            }
            let node_index = output[4];
            let signature = hex::encode(&output[5..69]);

            counter!("vault_registrations_total").increment(1);
            // note: can't easily get db.len() here without re-locking
            histogram!("vault_request_duration_seconds", "endpoint" => "register")
                .record(start.elapsed().as_secs_f64());

            Ok(Json(RegisterResponse {
                ok: true,
                node_index,
                signature,
            }))
        }
        ERR_ALREADY_REGISTERED => {
            counter!("vault_errors_total", "endpoint" => "register", "error" => "conflict").increment(1);
            Err((StatusCode::CONFLICT, "already registered".into()))
        }
        _ => Err((StatusCode::INTERNAL_SERVER_ERROR, "guest error".into())),
    }
}

async fn recover(
    State(state): State<Arc<Mutex<AppState>>>,
    Json(req): Json<RecoverRequest>,
) -> Result<Json<RecoverResponse>, (StatusCode, String)> {
    let start = Instant::now();
    counter!("vault_requests_total", "endpoint" => "recover").increment(1);

    let mut input = Vec::with_capacity(48);
    input.extend_from_slice(&req.user_id);
    input.extend_from_slice(&req.unlock_tag);

    let output = run_guest(&state, "recover", input).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    if output.len() < 4 {
        return Err((StatusCode::INTERNAL_SERVER_ERROR, "invalid response".into()));
    }

    let result = u32::from_le_bytes([output[0], output[1], output[2], output[3]]);

    match result {
        OK => {
            if output.len() < 13 {
                return Err((StatusCode::INTERNAL_SERVER_ERROR, "invalid response length".into()));
            }
            let remaining = u32::from_le_bytes([output[4], output[5], output[6], output[7]]);
            let share_len = u32::from_le_bytes([output[8], output[9], output[10], output[11]]) as usize;
            if output.len() < 13 + share_len {
                return Err((StatusCode::INTERNAL_SERVER_ERROR, "invalid share length".into()));
            }
            let node_index = output[12];
            let share_data = hex::encode(&output[13..13 + share_len]);

            counter!("vault_recoveries_total").increment(1);
            histogram!("vault_request_duration_seconds", "endpoint" => "recover")
                .record(start.elapsed().as_secs_f64());

            Ok(Json(RecoverResponse {
                ok: true,
                share: Some(ShareData {
                    index: node_index,
                    data: share_data,
                }),
                guesses_remaining: remaining,
                error: None,
            }))
        }
        ERR_NOT_FOUND => {
            counter!("vault_errors_total", "endpoint" => "recover", "error" => "not_found").increment(1);
            Err((StatusCode::NOT_FOUND, "not registered".into()))
        }
        ERR_LOCKED_OUT => {
            counter!("vault_lockouts_total").increment(1);
            Ok(Json(RecoverResponse {
                ok: false,
                share: None,
                guesses_remaining: 0,
                error: Some("no guesses remaining, registration deleted".into()),
            }))
        }
        ERR_WRONG_PIN => {
            let remaining = u32::from_le_bytes([output[4], output[5], output[6], output[7]]);
            counter!("vault_failed_attempts_total").increment(1);
            Ok(Json(RecoverResponse {
                ok: false,
                share: None,
                guesses_remaining: remaining,
                error: Some(format!("invalid pin, {} guesses remaining", remaining)),
            }))
        }
        _ => Err((StatusCode::INTERNAL_SERVER_ERROR, "guest error".into())),
    }
}

async fn status(
    State(state): State<Arc<Mutex<AppState>>>,
    Path(user_id): Path<String>,
) -> Result<Json<StatusResponse>, (StatusCode, String)> {
    let user_bytes = hex::decode(&user_id)
        .map_err(|_| (StatusCode::BAD_REQUEST, "invalid user_id".into()))?;

    if user_bytes.len() != 32 {
        return Err((StatusCode::BAD_REQUEST, "user_id must be 32 bytes".into()));
    }

    let output = run_guest(&state, "status", user_bytes).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    if output.len() < 6 {
        return Err((StatusCode::INTERNAL_SERVER_ERROR, "invalid response".into()));
    }

    Ok(Json(StatusResponse {
        registered: output[0] == 1,
        guesses_remaining: u32::from_le_bytes([output[1], output[2], output[3], output[4]]),
        locked: output[5] == 1,
    }))
}

async fn node_info(State(state): State<Arc<Mutex<AppState>>>) -> Json<NodeInfoResponse> {
    let state = state.lock().await;
    Json(NodeInfoResponse {
        version: env!("CARGO_PKG_VERSION").into(),
        index: state.index,
        pubkey: hex::encode(state.signing_key.verifying_key().to_bytes()),
        registrations: state.db.len() as u64,
        mode: "polkavm".into(),
    })
}

async fn health() -> &'static str {
    "ok"
}

#[tokio::main]
async fn main() {
    // install ring crypto provider for rustls
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("failed to install rustls crypto provider");

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("ghettobox_vault_pvm=info".parse().unwrap()),
        )
        .init();

    let args = Args::parse();

    // setup prometheus
    let metrics_port = args.metrics_port.unwrap_or(args.port + 1000);
    let metrics_addr: std::net::SocketAddr = format!("{}:{}", args.bind, metrics_port)
        .parse()
        .expect("invalid metrics address");

    PrometheusBuilder::new()
        .with_http_listener(metrics_addr)
        .install()
        .expect("failed to install prometheus");

    // setup data dir
    let data_dir = args.data_dir.unwrap_or_else(|| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        format!("{}/.ghettobox-vault-pvm", home)
    });
    std::fs::create_dir_all(&data_dir).expect("failed to create data dir");

    // open database
    let db_path = format!("{}/db", data_dir);
    let db = sled::open(&db_path).expect("failed to open database");

    // load or generate signing key
    let key_path = format!("{}/node.key", data_dir);
    let signing_key = if std::path::Path::new(&key_path).exists() {
        let key_bytes = std::fs::read(&key_path).expect("failed to read key");
        let key_arr: [u8; 32] = key_bytes.try_into().expect("invalid key length");
        SigningKey::from_bytes(&key_arr)
    } else {
        let key = SigningKey::generate(&mut rand::thread_rng());
        std::fs::write(&key_path, key.to_bytes()).expect("failed to write key");
        // set restrictive permissions (unix only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))
                .expect("failed to set key permissions");
        }
        key
    };

    // load polkavm blob (supports both raw .polkavm blobs and .elf files)
    let blob_data = std::fs::read(&args.blob)
        .unwrap_or_else(|_| panic!("failed to read blob: {}", args.blob));

    // detect ELF magic and convert if needed
    let blob_bytes = if blob_data.starts_with(b"\x7fELF") {
        info!("converting ELF to polkavm blob...");
        let linker_config = polkavm_linker::Config::default();
        polkavm_linker::program_from_elf(
            linker_config,
            polkavm_linker::TargetInstructionSet::Latest,
            &blob_data,
        ).expect("failed to convert ELF to polkavm blob")
    } else {
        blob_data
    };

    let blob = ProgramBlob::parse(blob_bytes.into())
        .expect("failed to parse polkavm blob");

    // create engine and module - use interpreter backend for portability
    let mut config = Config::from_env().unwrap_or_default();
    config.set_backend(Some(BackendKind::Interpreter));
    let engine = Engine::new(&config).expect("failed to create polkavm engine");

    let module_config = ModuleConfig::default();
    let module = Module::from_blob(&engine, &module_config, blob)
        .expect("failed to load polkavm module");

    // create TLS config once at startup (avoid reparsing webpki roots per request)
    let tls_config = make_tls_config();

    let pubkey = hex::encode(signing_key.verifying_key().to_bytes());
    info!("ghettobox-vault-pvm v{}", env!("CARGO_PKG_VERSION"));
    info!("  index: {}", args.index);
    info!("  mode: polkavm (sandboxed)");
    info!("  pubkey: {}", pubkey);
    info!("  data: {}", data_dir);
    info!("  blob: {}", args.blob);
    info!("  bind: {}:{}", args.bind, args.port);
    info!("  metrics: {}:{}", args.bind, metrics_port);

    gauge!("vault_registrations_current").set(db.len() as f64);

    let state = Arc::new(Mutex::new(AppState {
        engine,
        module,
        db,
        signing_key,
        index: args.index,
        tls_config,
    }));

    // create reshare state (separate from polkavm app state)
    let reshare_state = Arc::new(Mutex::new(pss::http::ReshareAppState::new()));

    // reshare sub-router with its own state
    let reshare_router = Router::new()
        .route("/epoch", get(pss::http::get_epoch))
        .route("/epoch", post(pss::http::start_epoch))
        .route("/commitment", post(pss::http::submit_commitment))
        .route("/commitment", get(pss::http::get_commitment))
        .route("/subshare/{player_index}", get(pss::http::get_subshare))
        .route("/status", get(pss::http::reshare_status))
        .route("/verify", get(pss::http::verify_group_key))
        .with_state(reshare_state);

    let app = Router::new()
        .route("/", get(node_info))
        .route("/health", get(health))
        .route("/register", post(register))
        .route("/recover", post(recover))
        .route("/status/{user_id}", get(status))
        .nest("/reshare", reshare_router)
        .layer(DefaultBodyLimit::max(128 * 1024)) // 128KB max request body
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = format!("{}:{}", args.bind, args.port);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    info!("listening on {}", addr);

    axum::serve(listener, app).await.unwrap();
}
