//! Diagnostic: does a JS bundle parse + evaluate under the *embedded* QuickJS?
//!
//! This exists because the two failures we actually hit here are invisible to
//! `node` and to esbuild's own output check:
//!
//!   1. **Invalid UTF-8** — esbuild (without `charset: "ascii"`) emits
//!      Latin-1-supplement code points (e.g. `×` U+00D7) as a *lone* byte,
//!      which is not valid UTF-8. QuickJS's parser is strict and rejects it,
//!      reporting a position that points misleadingly at the top of the file
//!      (`<input>:1:9`), not at the offending byte.
//!   2. **Syntax QuickJS rejects** but the host build happily accepts.
//!
//! Usage:
//!   cargo run --release --example qjsprobe -- ../ui/dist/main.js
//!
//! Exit code 0 = parsed and ran; 1 = failed (reason printed).

use rquickjs::{Context, Object, Runtime};

/// Minimal `process`/timer/console stand-ins so the bundle's top level can
/// actually execute. Without these, a *successful* parse would still surface as
/// "process is not defined" and you couldn't tell parse failure from runtime.
/// (`process` is a host-provided global — QuickJS has no notion of it.)
const STUBS: &str = r#"
    globalThis.__noop = function () {};
    globalThis.console = { log: __noop, info: __noop, warn: __noop, error: __noop, debug: __noop };
    globalThis.setInterval = function () { return 0; };
    globalThis.setTimeout = function () { return 0; };
    globalThis.clearInterval = globalThis.clearTimeout = function () {};
    globalThis.process = {
        stdout: { write: __noop },
        stderr: { write: __noop },
        stdin: { setEncoding: __noop, on: __noop },
        env: {},
        argv: [],
        exit: __noop
    };
"#;

fn main() {
    let path = match std::env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: qjsprobe <bundle.js>");
            std::process::exit(2);
        }
    };
    // Read as BYTES, not String — `read_to_string` would panic on invalid
    // UTF-8 and make the check below unreachable (that's the whole point).
    let raw = std::fs::read(&path).expect("read bundle");

    // --- check 1: strict UTF-8 (this is the trap that cost the most time) ---
    let src = match std::str::from_utf8(&raw) {
        Ok(s) => {
            println!("[utf8] valid UTF-8 ({} bytes)", s.len());
            s
        }
        Err(e) => {
            let i = e.valid_up_to();
            let lo = i.saturating_sub(30);
            let hi = (i + 30).min(raw.len());
            println!(
                "[utf8] INVALID at byte {i} (line ~{}): {:?}",
                raw[..i].iter().filter(|b| **b == b'\n').count() + 1,
                String::from_utf8_lossy(&raw[lo..hi])
            );
            println!("       fix: esbuild option `charset: \"ascii\"`");
            std::process::exit(1);
        }
    };

    // --- check 2: parse + evaluate under QuickJS ---
    let rt = Runtime::new().expect("runtime");
    let ctx = Context::full(&rt).expect("context");
    ctx.with(|ctx| {
        // stubs first, so only a genuine parse/exec failure is reported
        let _ = ctx.eval::<(), _>(STUBS);
        match ctx.eval::<(), _>(src) {
            Ok(_) => println!("[qjs] OK — parsed and evaluated"),
            Err(_) => {
                // rquickjs's Debug/Display collapse to "Exception"; read the
                // real message off the caught exception object.
                let exc = ctx.catch();
                let msg = exc
                    .get::<Object>()
                    .ok()
                    .and_then(|o| o.get::<_, String>("message").ok())
                    .unwrap_or_else(|| "<no message>".into());
                println!("[qjs] FAIL — {msg}");
                std::process::exit(1);
            }
        }
    });
}
