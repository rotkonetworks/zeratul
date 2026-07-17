//! ntfy dispute alerts — one HTTP POST to a topic when a room needs the operator.
//!
//! Deliberately dumb: the notification carries only the room code, a coarse
//! classification, and a click-through link to the dashboard. No signatures, keys,
//! or addresses — all sensitive adjudication lives behind the PIN on the dashboard,
//! not in a push that may transit a third party (ntfy.sh) or a phone lock screen.
//!
//! Config via env (both optional; notifier is a no-op if `ESCROW_NTFY_URL` is unset):
//!   ESCROW_NTFY_URL   — full ntfy topic URL, e.g. https://ntfy.rotko.net/zkpoker-<secret>
//!   ESCROW_DASHBOARD_BASE — public base for the click link (default https://zkbtc.org)

use std::sync::OnceLock;

struct NtfyCfg {
    url: String,
    dashboard_base: String,
    /// optional bearer token for auth-protected ntfy servers (e.g. ntfy.rotko.net)
    token: Option<String>,
}

static NTFY: OnceLock<Option<NtfyCfg>> = OnceLock::new();

/// Configure the notifier. `ntfy_url = None` (or empty) → notifier disabled (no-op).
/// `token` (empty → None) becomes an `Authorization: Bearer` header for locked-down servers.
pub fn init(ntfy_url: Option<String>, dashboard_base: String, token: Option<String>) {
    let cfg = match ntfy_url {
        Some(u) if !u.trim().is_empty() => {
            let token = token.filter(|t| !t.trim().is_empty());
            tracing::info!("ntfy alerts: enabled ({}, auth={})", u, token.is_some());
            Some(NtfyCfg { url: u, dashboard_base, token })
        }
        _ => {
            tracing::info!("ntfy alerts: disabled (ESCROW_NTFY_URL unset)");
            None
        }
    };
    let _ = NTFY.set(cfg);
}

/// Fire a dispute alert. Fire-and-forget: spawns the POST so the money path never
/// blocks on notification IO, and swallows errors (an alert failing must not break
/// escrow logic). No-op if the notifier is disabled.
///
/// `tag` is an ntfy emoji tag (e.g. "warning", "money_with_wings"); `code` is the
/// room, appended to the dashboard base as the click-through target.
pub fn dispute_alert(title: &str, message: &str, tag: &str, code: &str) {
    let Some(Some(cfg)) = NTFY.get() else { return };
    let url = cfg.url.clone();
    let click = format!("{}/dispute/{}", cfg.dashboard_base.trim_end_matches('/'), code);
    let (title, message, tag) = (title.to_string(), message.to_string(), tag.to_string());
    let token = cfg.token.clone();
    tokio::spawn(async move {
        let mut req = reqwest::Client::new()
            .post(&url)
            .header("Title", title)
            .header("Tags", tag)
            .header("Priority", "high")
            .header("Click", click)
            .body(message)
            .timeout(std::time::Duration::from_secs(8));
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        let res = req.send().await;
        match res {
            Ok(r) if r.status().is_success() => {}
            Ok(r) => tracing::warn!("ntfy alert non-2xx: {}", r.status()),
            Err(e) => tracing::warn!("ntfy alert failed: {}", e),
        }
    });
}
