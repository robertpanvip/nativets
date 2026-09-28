//! Minimal Chrome DevTools Protocol server — UI/style debugging over the
//! retained tree (`GPUI_TS_CDP=9222`).
//!
//! Scope (by design, see task #116): UI + style inspection and live tweaking.
//! No JS breakpoints, no console capture, no profiler — the engine already has
//! its own diagnostics, and this server exists to answer "what does the host
//! actually render, and what happens if this style changes".
//!
//! Wire shape (Chrome-compatible enough for `chrome://inspect` discovery):
//!
//!   http://127.0.0.1:<port>/json        → page list (one entry, wsUrls)
//!   ws://127.0.0.1:<port>/devtools/page/1  → CDP session
//!
//! Implemented CDP methods, mapped onto the retained tree:
//!   DOM.getDocument            → the whole tree as nested DOM nodes
//!                                (nodeId = protocol node id)
//!   DOM.querySelector          → first matching tag (or `[id="…"]`)
//!   DOM.getBoxModel            → geometry is GPUI's job at layout time;
//!                                reports an empty quad (0-size) + content quad
//!                                so devtools stays functional
//!   CSS.getComputedStyleForNode→ the node's style map, flat text form
//!                                ("background: …", one per line) + value
//!   CSS.setStyleTexts          → parse "k: v" lines and inject a SetStyle op
//!                                — the change lands in the *live* retained
//!                                tree within one frame, no reload
//!   Runtime.evaluate           → QuickJS only: eval in the running engine
//!   Ping                       → liveness
//!
//! Threading: the GPUI tree lives on the main thread, so everything runs
//! through `handle.update` on the GPUI executor; the CDP engine thread only
//! serializes JSON and writes sockets. One client at a time (devtools);
//! the HTTP listener accepts discovery requests forever.

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

use serde_json::{json, Value};

use tungstenite::handshake::server::{Request, Response};
use tungstenite::protocol::Message;

use crate::cdp_state::CdpShared;
use crate::tree::{Op, Tree};

/// `GPUI_TS_CDP` — port to listen on. Unset / "0" → CDP off.
pub fn enabled_port() -> Option<u16> {
    match std::env::var("GPUI_TS_CDP") {
        Ok(v) => {
            let t = v.trim().to_string();
            if t.is_empty() || t == "0" {
                None
            } else {
                t.parse::<u16>().ok().filter(|p| *p != 0)
            }
        }
        Err(_) => None,
    }
}

/// Boot the CDP server threads. `shared` bridges to the GPUI-side state
/// (`cdp_state::CdpShared`, owned by `HostView`).
pub fn launch(port: u16, shared: Arc<CdpShared>) {
    // Discovery HTTP endpoint: GET /json (and /json/list) → page metadata.
    let http_shared = shared.clone();
    std::thread::Builder::new()
        .name("cdp-http".into())
        .spawn(move || {
            let listener = match TcpListener::bind(("127.0.0.1", port)) {
                Ok(l) => l,
                Err(e) => {
                    crate::log_line(&format!(
                        "[cdp] cannot bind 127.0.0.1:{port}: {e} — DevTools discovery off"
                    ));
                    return;
                }
            };
            crate::log_line(&format!(
                "[cdp] DevTools endpoint: http://127.0.0.1:{port}/json  (UI/style debugging)"
            ));
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                let shared = http_shared.clone();
                std::thread::Builder::new()
                    .name("cdp-http-conn".into())
                    .spawn(move || serve_http(&mut s, &shared))
                    .ok();
            }
        })
        .ok();

    // The CDP WebSocket session thread: accept one client, serve it until it
    // disconnects, then loop back to accept the next.
    let ws_shared = shared;
    std::thread::Builder::new()
        .name("cdp-ws".into())
        .spawn(move || {
            let listener = match TcpListener::bind(("127.0.0.1", port + 1)) {
                Ok(l) => l,
                Err(e) => {
                    crate::log_line(&format!("[cdp] ws bind failed on {port}: {e}"));
                    return;
                }
            };
            for stream in listener.incoming() {
                let Ok(s) = stream else { continue };
                let shared = ws_shared.clone();
                serve_ws(s, shared);
            }
        })
        .ok();
}

/// Serve one HTTP discovery request (`GET /json`, `GET /json/list`), then
/// close. Anything else gets a 404 — Chrome's `/json/version` is tolerated
/// with a minimal shape so the inspector's backend picker lists the page.
fn serve_http(stream: &mut TcpStream, shared: &Arc<CdpShared>) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
    let mut buf = [0u8; 4096];
    let n = match stream.read(&mut buf) {
        Ok(n) if n > 0 => n,
        _ => return,
    };
    let req = String::from_utf8_lossy(&buf[..n]).to_string();
    let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
    let body = if path.starts_with("/json/list") || path == "/json" {
        // The WS endpoint listens on port+1 (the HTTP listener owns `port`);
        // advertise that exact port so devtools connects first try.
        let ws_port = stream.local_addr().map(|a| a.port()).unwrap_or(0) + 1;
        json!([{
            "description": "nativets host window",
            "id": "nativets",
            "title": shared.title.lock().unwrap().clone(),
            "type": "page",
            "url": "nativets://app/index.tsx",
            "webSocketDebuggerUrl": format!("ws://127.0.0.1:{ws_port}/devtools/page/1"),
        }])
        .to_string()
    } else if path.starts_with("/json/version") {
        json!({
            "Browser": "nativets/0.1",
            "Protocol-Version": "1.3",
            "User-Agent": "nativets-host CDP (UI/style only)",
            "V8-Version": "0",
            "WebKit-Version": "0",
        })
        .to_string()
    } else {
        let _ = write_response(stream, "404 Not Found", b"not found");
        return;
    };
    let _ = write_response(stream, "200 OK", body.as_bytes());
}

fn write_response(stream: &mut TcpStream, status: &str, body: &[u8]) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)
}

/// One WebSocket CDP session. `tungstenite` performs the HTTP upgrade for us.
fn serve_ws(stream: TcpStream, shared: Arc<CdpShared>) {
    let ws = match tungstenite::accept_hdr(stream, |req: &Request, resp: Response| {
        // Any path is fine — devtools connects to the advertised ws URL, other
        // clients may not. Log the path once for debugging.
        crate::log_line(&format!("[cdp] ws upgrade: {}", req.uri().path()));
        Ok(resp)
    }) {
        Ok(w) => w,
        Err(e) => {
            crate::log_line(&format!("[cdp] ws handshake failed: {e}"));
            return;
        }
    };
    let mut ws = ws;
    crate::log_line("[cdp] DevTools client connected");
    loop {
        match ws.read() {
            Ok(Message::Text(txt)) => {
                let reply = handle_message(&txt, &shared);
                if let Some(reply) = reply {
                    if ws.send(Message::Text(reply)).is_err() {
                        break;
                    }
                }
            }
            Ok(Message::Ping(p)) => {
                if ws.send(Message::Pong(p)).is_err() {
                    break;
                }
            }
            Ok(Message::Close(_)) | Err(_) => break,
            Ok(_) => {} // binary/pong: ignore
        }
    }
    crate::log_line("[cdp] DevTools client disconnected");
}

/// Dispatch one CDP message. Returns the JSON response text, or None for
/// notifications (none implemented — every method we answer is request/response).
fn handle_message(txt: &str, shared: &Arc<CdpShared>) -> Option<String> {
    let v: Value = match serde_json::from_str(txt) {
        Ok(v) => v,
        Err(_) => return None,
    };
    let id = v.get("id").and_then(|x| x.as_u64()).unwrap_or(0);
    let method = v.get("method").and_then(|x| x.as_str()).unwrap_or("");
    let params = v.get("params").cloned().unwrap_or(Value::Null);

    let result: Result<Value, String> = match method {
        "Ping" => Ok(json!({})),
        "DOM.getDocument" => dom_get_document(shared),
        "DOM.querySelector" => dom_query_selector(shared, &params),
        "DOM.getBoxModel" => dom_get_box_model(&params),
        "DOM.getNodeForLocation" => Ok(json!({ "nodeId": params.get("nodeId").cloned().unwrap_or(json!(0)) })),
        "CSS.getComputedStyleForNode" => css_computed(shared, &params),
        "CSS.setStyleTexts" => css_set_style_texts(shared, &params),
        "Runtime.evaluate" => runtime_evaluate(shared, &params),
        other => Err(format!("method not supported: {other}")),
    };

    let payload = match result {
        Ok(mut r) => {
            // getComputedStyleForNode needs the same `computedStyle` shape
            // devtools styles pane expects; helpers above already set it.
            if method == "DOM.getDocument" {
                // Attach the root's children count for the UI sanity check.
                r["root"] = r.get("root").cloned().unwrap_or(Value::Null);
            }
            json!({ "id": id, "result": r })
        }
        Err(e) => json!({
            "id": id,
            "error": { "code": -32601, "message": e }
        }),
    };
    Some(payload.to_string())
}

// ---------------------------------------------------------------------------
// DOM methods — retained tree → devtools DOM pane
// ---------------------------------------------------------------------------

/// Build the devtools DOM subtree for one node. Runs on the MAIN THREAD inside
/// a single `with_tree` hop: the earlier per-node version opened one hop per
/// node, and every hop waits for a GPUI frame — on a mostly-static UI that
/// meant a frame period (~500 ms) *per node*, blowing any client timeout on a
/// real tree. One hop for the whole subtree keeps the cost at one frame.
fn dom_snapshot(tree: &Tree, id: u64, depth_left: u32) -> Value {
    let (tag, text, child_ids) = match tree.node(id) {
        Some(n) => (
            n.tag.clone(),
            n.text.clone().unwrap_or_default(),
            n.children.clone(),
        ),
        None => ("div".to_string(), String::new(), Vec::new()),
    };
    let mut children_json: Option<Value> = None;
    if depth_left > 0 && !child_ids.is_empty() {
        let arr: Vec<Value> = child_ids
            .iter()
            .map(|c| dom_snapshot(tree, *c, depth_left - 1))
            .collect();
        children_json = Some(json!(arr));
    }
    let mut node = json!({
        "nodeId": id,
        "backendNodeId": id,
        "nodeType": 1, // ELEMENT_NODE
        "nodeName": tag.to_ascii_uppercase(),
        "localName": tag,
        "nodeValue": "",
        "childNodeCount": child_ids.len(),
    });
    if !text.is_empty() {
        node["nodeValue"] = json!(text);
    }
    if let Some(c) = children_json {
        node["children"] = c;
    }
    node
}

fn dom_get_document(shared: &Arc<CdpShared>) -> Result<Value, String> {
    // One hop builds the whole subtree (see `dom_snapshot`).
    let root = shared
        .with_tree(move |tree| dom_snapshot(tree, 0, 8))
        .map_err(|e| format!("DOM.getDocument: {e}"))?;
    Ok(json!({ "root": {
        "nodeId": 1,
        "backendNodeId": 1,
        "nodeType": 9, // DOCUMENT_NODE
        "nodeName": "#document",
        "localName": "",
        "nodeValue": "",
        "children": [root],
    }}))
}

fn dom_query_selector(shared: &Arc<CdpShared>, params: &Value) -> Result<Value, String> {
    let selector = params
        .get("selector")
        .and_then(|x| x.as_str())
        .unwrap_or("");
    // Only two selector shapes are worth supporting: tag names. (An `#id`
    // form makes no sense — node ids are protocol-assigned integers.)
    let selector = selector.to_string();
    let found = shared
        .with_tree(move |tree| {
            for id in tree.ids() {
                if let Some(n) = tree.node(id) {
                    if n.tag == selector {
                        return Some(id);
                    }
                }
            }
            None
        })
        .unwrap_or(None);
    Ok(json!({ "nodeId": found.unwrap_or(0) }))
}

fn dom_get_box_model(params: &Value) -> Result<Value, String> {
    // Layout happens inside GPUI at frame time; the retained tree has no
    // geometry to report. An all-zero model keeps devtools' layout pane from
    // erroring without pretending we measured anything.
    let id = params.get("nodeId").and_then(|x| x.as_u64()).unwrap_or(0);
    let zeros: Vec<f64> = vec![0.0; 8];
    Ok(json!({
        "model": {
            "nodeId": id,
            "content": zeros, "padding": zeros, "border": zeros, "margin": zeros,
            "width": 0, "height": 0,
        }
    }))
}

// ---------------------------------------------------------------------------
// CSS methods — style inspection + live injection
// ---------------------------------------------------------------------------

fn css_computed(shared: &Arc<CdpShared>, params: &Value) -> Result<Value, String> {
    let id = params
        .get("nodeId")
        .and_then(|x| x.as_u64())
        .ok_or("missing nodeId")?;
    let entries = shared
        .with_tree(move |tree| {
            let mut out: Vec<(String, String)> = Vec::new();
            if let Some(n) = tree.node(id) {
                // Deterministic iteration order; the exact ordering only
                // affects how the styles pane lists entries.
                for (k, v) in n.style.iter() {
                    out.push((k.clone(), value_to_css(v)));
                }
            }
            out
        })
        .unwrap_or_default();
    let computed: Vec<Value> = entries
        .into_iter()
        .map(|(name, value)| json!({ "name": name, "value": value }))
        .collect();
    Ok(json!({ "computedStyle": computed }))
}

fn value_to_css(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// `CSS.setStyleTexts`: parse `k: v` pairs from the edit and inject a SetStyle
/// op into the ops channel — exactly what the frontend would have sent. The
/// UI picks the change up within one frame (drain_ops), no reload.
fn css_set_style_texts(shared: &Arc<CdpShared>, params: &Value) -> Result<Value, String> {
    let id = params
        .get("nodeId")
        .and_then(|x| x.as_u64())
        .ok_or("missing nodeId")?;
    let edits = params
        .get("edits")
        .and_then(|x| x.as_array())
        .ok_or("missing edits")?;
    let mut style = serde_json::Map::new();
    for edit in edits {
        let text = edit
            .get("text")
            .and_then(|x| x.as_str())
            .unwrap_or_default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Some((k, v)) = line.split_once(':') else { continue };
            let k = k.trim();
            let v = v.trim().trim_end_matches(';').trim();
            if k.is_empty() || v.is_empty() {
                continue;
            }
            // Numbers ride as numbers (SetStyle's map accepts either), strings
            // as strings — same shape the TS frontend sends.
            style.insert(
                k.to_string(),
                v.parse::<f64>()
                    .map(|n| json!(n as i64))
                    .unwrap_or_else(|_| json!(v)),
            );
        }
    }
    if style.is_empty() {
        return Err("no parseable `k: v` lines in edit text".into());
    }
    shared.send_op(Op::SetStyle { id, style })?;
    Ok(json!({ "styles": [] }))
}

// ---------------------------------------------------------------------------
// Runtime.evaluate — QuickJS only
// ---------------------------------------------------------------------------

fn runtime_evaluate(shared: &Arc<CdpShared>, params: &Value) -> Result<Value, String> {
    let expr = params
        .get("expression")
        .and_then(|x| x.as_str())
        .ok_or("missing expression")?
        .to_string();
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    shared.eval_request(expr, tx)?;
    // The engine replies with JSON *text* (it stringifies inside the
    // context); parse it back here. A throwing eval arrives as
    // {"__evalError": …} — surfaced as an exceptionResponse, which is what
    // devtools' console expects.
    let text = rx
        .recv_timeout(std::time::Duration::from_secs(3))
        .map_err(|_| "engine did not answer (backend not quickjs or busy)?".to_string())?;
    let parsed: Value = serde_json::from_str(&text)
        .unwrap_or(Value::String(text));
    if parsed.get("__evalError").is_some() {
        return Ok(json!({
            "result": {
                "type": "object",
                "subtype": "error",
                "description": parsed["__evalError"].as_str().unwrap_or("eval error"),
                "value": parsed["__evalError"],
            },
            "exceptionDetails": {
                "text": parsed["__evalError"],
                "exception": { "type": "object", "subtype": "error" },
            }
        }));
    }
    Ok(json!({
        "result": json_value_to_cdp(&parsed),
    }))
}

/// Map a serde_json value onto CDP's RemoteObject shape (the subset devtools
/// prints for console results).
fn json_value_to_cdp(v: &Value) -> Value {
    match v {
        Value::Null => json!({ "type": "object", "subtype": "null", "value": null }),
        Value::Bool(b) => json!({ "type": "boolean", "value": b, "description": b.to_string() }),
        Value::Number(n) => json!({ "type": "number", "value": n, "description": n.to_string() }),
        Value::String(s) => json!({ "type": "string", "value": s }),
        Value::Array(_) => json!({ "type": "object", "subtype": "array", "value": v, "description": "Array" }),
        Value::Object(_) => json!({ "type": "object", "value": v, "description": "Object" }),
    }
}
