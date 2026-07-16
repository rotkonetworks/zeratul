//! Dispute-resolution dashboard: classify rooms from the durable journal and render
//! the operator's mobile-friendly pages (`/disputes` list, `/dispute/{code}` detail).
//!
//! Pure over journal events — no escrow state, no signing. The money-moving action
//! (`/room/{code}/arbitrate`) lives in main.rs behind the PIN gate; these functions
//! only present evidence and the buttons that POST to it.

use serde_json::Value;

/// Coarse room classification derived from its journal events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// paid out (auto co-signed settlement, or an arbiter ruling that executed)
    Resolved,
    /// a client fault or failed payout is outstanding — needs the operator
    Fault,
    /// operator explicitly deferred (postponed) — still open
    Postponed,
    /// both players co-signed; payout in flight (not a dispute)
    Settled,
    /// created / deposited, game ongoing — nothing to do
    Active,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Resolved => "RESOLVED",
            Status::Fault => "FAULT",
            Status::Postponed => "POSTPONED",
            Status::Settled => "SETTLED",
            Status::Active => "ACTIVE",
        }
    }
    /// Does this room want the operator's eyes?
    pub fn needs_attention(self) -> bool {
        matches!(self, Status::Fault | Status::Postponed)
    }
    fn color(self) -> &'static str {
        match self {
            Status::Fault => "#e5484d",
            Status::Postponed => "#f5a623",
            Status::Resolved => "#30a46c",
            Status::Settled => "#5b9dd9",
            Status::Active => "#8b8b8b",
        }
    }
}

fn kinds(events: &[Value]) -> Vec<&str> {
    events.iter().filter_map(|e| e.get("kind").and_then(|k| k.as_str())).collect()
}

/// Classify a single room's events. Later events win (a payout after a fault ⇒ Resolved).
pub fn classify(events: &[Value]) -> Status {
    let ks = kinds(events);
    // resolution wins outright
    if ks.iter().any(|k| *k == "payout_broadcast") {
        return Status::Resolved;
    }
    // last arbiter ruling, if any
    let last_ruling = events.iter().rev().find_map(|e| {
        if e.get("kind").and_then(|k| k.as_str()) == Some("arbiter_ruling") {
            e.get("data").and_then(|d| d.get("ruling")).and_then(|r| r.as_str())
        } else { None }
    });
    if let Some(r) = last_ruling {
        return if r == "postpone" { Status::Postponed } else { Status::Resolved };
    }
    if ks.iter().any(|k| *k == "client_fault" || *k == "payout_failed") {
        return Status::Fault;
    }
    if ks.iter().any(|k| *k == "settlement_finalized") {
        return Status::Settled;
    }
    Status::Active
}

/// Group all events by room code, preserving first-seen order.
fn group_by_room(all: &[Value]) -> Vec<(String, Vec<Value>)> {
    let mut order: Vec<String> = Vec::new();
    let mut map: std::collections::HashMap<String, Vec<Value>> = std::collections::HashMap::new();
    for e in all {
        if let Some(code) = e.get("code").and_then(|c| c.as_str()) {
            if !map.contains_key(code) { order.push(code.to_string()); }
            map.entry(code.to_string()).or_default().push(e.clone());
        }
    }
    order.into_iter().map(|c| { let v = map.remove(&c).unwrap_or_default(); (c, v) }).collect()
}

const HEAD: &str = r#"<!doctype html><html><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>zk.poker disputes</title><style>
:root{color-scheme:dark}
body{font:15px/1.5 -apple-system,system-ui,sans-serif;background:#0b0b0d;color:#e6e6e6;margin:0;padding:16px;max-width:760px;margin:0 auto}
h1{font-size:19px;margin:.2em 0 1em}
a{color:#7cc4ff;text-decoration:none}
.row{display:flex;align-items:center;gap:10px;padding:12px;border:1px solid #222;border-radius:10px;margin-bottom:8px;background:#141416}
.tag{font-size:11px;font-weight:700;padding:2px 8px;border-radius:20px;color:#000}
.mono{font-family:ui-monospace,monospace;font-size:12px;color:#aaa;word-break:break-all}
.ev{padding:8px 10px;border-left:2px solid #333;margin:0 0 6px 4px}
.ev b{color:#cbd5e1}
.k{display:inline-block;font-size:11px;font-weight:700;color:#000;padding:1px 7px;border-radius:12px;margin-right:6px}
.btns{display:grid;grid-template-columns:1fr 1fr;gap:8px;margin-top:14px}
button{font:600 15px/1 inherit;padding:14px;border:0;border-radius:10px;color:#fff;cursor:pointer}
.pay{background:#30a46c}.ref{background:#5b6472}.post{background:#f5a623;color:#000}.warn{background:#e5484d}
input{font:16px ui-monospace,monospace;width:100%;padding:12px;border:1px solid #333;border-radius:10px;background:#0e0e10;color:#fff;box-sizing:border-box;margin-top:6px}
.muted{color:#888;font-size:13px}
#msg{margin-top:12px;padding:10px;border-radius:8px;display:none}
</style></head><body>"#;

/// Render `/disputes` — attention-needing rooms first, then everything else.
pub fn render_list(all: &[Value]) -> String {
    let rooms = group_by_room(all);
    let mut needs: Vec<(String, Status)> = Vec::new();
    let mut rest: Vec<(String, Status)> = Vec::new();
    for (code, evs) in &rooms {
        let st = classify(evs);
        if st.needs_attention() { needs.push((code.clone(), st)); }
        else { rest.push((code.clone(), st)); }
    }
    let mut h = String::from(HEAD);
    h.push_str("<h1>disputes</h1>");
    if rooms.is_empty() {
        h.push_str("<p class=muted>No journalled rooms yet.</p>");
    }
    let row = |code: &str, st: Status| format!(
        "<a class=row href=\"/dispute/{c}\"><span class=tag style=\"background:{col}\">{lab}</span><span class=mono>{c}</span></a>",
        c = html_escape(code), col = st.color(), lab = st.label());
    if !needs.is_empty() {
        h.push_str("<div class=muted style=\"margin-bottom:6px\">needs attention</div>");
        for (c, s) in &needs { h.push_str(&row(c, *s)); }
    }
    if !rest.is_empty() {
        h.push_str("<div class=muted style=\"margin:14px 0 6px\">all rooms</div>");
        for (c, s) in &rest { h.push_str(&row(c, *s)); }
    }
    h.push_str("</body></html>");
    h
}

/// Render `/dispute/{code}` — evidence timeline + PIN + ruling buttons.
pub fn render_detail(code: &str, events: &[Value]) -> String {
    let st = classify(events);
    let mut h = String::from(HEAD);
    h.push_str(&format!(
        "<h1><a href=/disputes>&larr;</a> room <span class=mono>{}</span></h1>\
         <div><span class=tag style=\"background:{}\">{}</span></div>",
        html_escape(code), st.color(), st.label()));

    if events.is_empty() {
        h.push_str("<p class=muted>No events for this room (may have been created before the journal, or wrong code).</p>");
    } else {
        h.push_str("<h3 style=\"margin-top:18px\">evidence</h3>");
        for e in events {
            let kind = e.get("kind").and_then(|k| k.as_str()).unwrap_or("?");
            let ts = e.get("ts").and_then(|t| t.as_u64()).unwrap_or(0);
            let data = e.get("data").cloned().unwrap_or(Value::Null);
            let pretty = serde_json::to_string_pretty(&data).unwrap_or_default();
            h.push_str(&format!(
                "<div class=ev><span class=k style=\"background:{col}\">{k}</span>\
                 <span class=muted data-ts=\"{ts}\"></span>\
                 <pre class=mono style=\"margin:6px 0 0;white-space:pre-wrap\">{d}</pre></div>",
                col = kind_color(kind), k = html_escape(kind), ts = ts, d = html_escape(&pretty)));
        }
    }

    // action panel — only meaningful for open rooms, but always shown for auditability
    h.push_str(&format!(r#"
    <h3 style="margin-top:20px">rule</h3>
    <div class=muted>Enter operator PIN, then choose. Payouts still require the winning player's share to co-sign (2-of-3) — this authorizes the house share.</div>
    <input id=pin type=password inputmode=numeric autocomplete=off placeholder="operator PIN">
    <div class=btns>
      <button class=pay onclick="rule('pay_a')">Pay Player A</button>
      <button class=pay onclick="rule('pay_b')">Pay Player B</button>
      <button class=ref onclick="rule('refund')">Refund both</button>
      <button class=post onclick="rule('postpone')">Postpone</button>
    </div>
    <div id=msg></div>
    <script>
      document.querySelectorAll('[data-ts]').forEach(function(el){{var t=+el.dataset.ts;if(t)el.textContent=new Date(t).toISOString().replace('T',' ').slice(0,19)+' UTC';}});
      async function rule(r){{
        var pin=document.getElementById('pin').value;
        var msg=document.getElementById('msg');
        if(!pin){{show('#e5484d','enter the PIN first');return;}}
        if(r!=='postpone' && !confirm('Confirm '+r.replace('_',' ')+' for room {code}?'))return;
        show('#333','submitting…');
        try{{
          var res=await fetch('/room/{code}/arbitrate',{{method:'POST',headers:{{'content-type':'application/json'}},body:JSON.stringify({{pin:pin,ruling:r}})}});
          var j=await res.json();
          if(res.ok&&j.ok){{show('#30a46c',j.message||'done');}}
          else{{show('#e5484d',(j.error||('HTTP '+res.status)));}}
        }}catch(e){{show('#e5484d',''+e);}}
      }}
      function show(c,t){{var m=document.getElementById('msg');m.style.display='block';m.style.background=c;m.textContent=t;}}
    </script>"#, code = html_escape(code)));
    h.push_str("</body></html>");
    h
}

fn kind_color(kind: &str) -> &'static str {
    match kind {
        "client_fault" | "payout_failed" => "#e5484d",
        "payout_broadcast" => "#30a46c",
        "settlement_finalized" => "#5b9dd9",
        "arbiter_ruling" => "#f5a623",
        "deposit_detected" => "#7c9",
        _ => "#888",
    }
}

/// Extract the caller IP for the PIN lockout. Behind haproxy the socket peer is
/// 127.0.0.1, so trust `X-Forwarded-For` (first hop) when present.
pub fn client_ip(headers: &axum::http::HeaderMap) -> String {
    headers.get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
     .replace('"', "&quot;").replace('\'', "&#39;")
}
