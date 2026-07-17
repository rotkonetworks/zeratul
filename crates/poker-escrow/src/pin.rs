//! Operator-PIN gate for the dispute dashboard's money-moving actions.
//!
//! The escrow stores only an **argon2 hash** of the PIN (via `ESCROW_ARBITER_PIN_HASH`),
//! never the raw PIN. Generate the hash with `poker-escrow --hash-pin <PIN>`.
//!
//! Brute-force defence is a hard **per-IP lockout: 3 wrong tries → 15-minute lock**.
//! (A 6-digit PIN is only 1e6 combos; the lockout, not argon2, is the real wall — argon2
//! just protects the PIN if the hash itself ever leaks.) Behind haproxy the caller IP
//! comes from `X-Forwarded-For`, so the handler passes that, not the socket peer.
//!
//! Fail-closed: if no hash is configured, every check returns `NotConfigured` and the
//! caller MUST reject — the dashboard cannot move money without a configured PIN.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use argon2::{Argon2, PasswordHash, PasswordVerifier};

const MAX_TRIES: u32 = 3;
const LOCKOUT: Duration = Duration::from_secs(15 * 60);

static PIN_HASH: OnceLock<Option<String>> = OnceLock::new();
static ATTEMPTS: OnceLock<Mutex<HashMap<String, Attempts>>> = OnceLock::new();

#[derive(Default)]
struct Attempts {
    fails: u32,
    locked_until: Option<Instant>,
}

#[derive(Debug)]
pub enum PinResult {
    Ok,
    Wrong { remaining: u32 },
    Locked { secs: u64 },
    NotConfigured,
}

/// Store the argon2 PHC hash string (or None to leave the gate closed).
pub fn init(hash: Option<String>) {
    let configured = hash.as_deref().map(|h| !h.trim().is_empty()).unwrap_or(false);
    let _ = PIN_HASH.set(hash.filter(|h| !h.trim().is_empty()));
    let _ = ATTEMPTS.set(Mutex::new(HashMap::new()));
    tracing::info!("arbiter PIN gate: {}", if configured { "configured" } else { "NOT configured (dashboard actions disabled)" });
}

/// Compute an argon2 hash of a PIN — used by the `--hash-pin` helper. Random salt.
pub fn hash_pin(pin: &str) -> Result<String, String> {
    use argon2::password_hash::{PasswordHasher, SaltString, rand_core::OsRng};
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(pin.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("argon2 hash: {}", e))
}

/// Verify a submitted PIN for `ip`, enforcing the per-IP lockout. Constant-time in the
/// argon2 comparison. Resets the counter on success.
pub fn check(ip: &str, pin: &str) -> PinResult {
    let Some(hash_opt) = PIN_HASH.get() else { return PinResult::NotConfigured };
    let Some(hash) = hash_opt else { return PinResult::NotConfigured };
    let Some(att_lock) = ATTEMPTS.get() else { return PinResult::NotConfigured };

    let mut attempts = att_lock.lock().unwrap_or_else(|e| e.into_inner());
    let entry = attempts.entry(ip.to_string()).or_default();

    // still locked?
    if let Some(until) = entry.locked_until {
        match until.checked_duration_since(Instant::now()) {
            Some(remaining) if !remaining.is_zero() => {
                return PinResult::Locked { secs: remaining.as_secs() };
            }
            _ => {
                // lock expired — reset the window
                entry.fails = 0;
                entry.locked_until = None;
            }
        }
    }

    let parsed = match PasswordHash::new(hash) {
        Ok(p) => p,
        Err(e) => { tracing::error!("arbiter PIN hash malformed: {}", e); return PinResult::NotConfigured; }
    };

    if Argon2::default().verify_password(pin.as_bytes(), &parsed).is_ok() {
        entry.fails = 0;
        entry.locked_until = None;
        PinResult::Ok
    } else {
        entry.fails += 1;
        if entry.fails >= MAX_TRIES {
            entry.locked_until = Some(Instant::now() + LOCKOUT);
            tracing::warn!("arbiter PIN: ip={} LOCKED after {} fails", ip, entry.fails);
            PinResult::Locked { secs: LOCKOUT.as_secs() }
        } else {
            PinResult::Wrong { remaining: MAX_TRIES - entry.fails }
        }
    }
}
