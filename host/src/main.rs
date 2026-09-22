//! nativets host — TypeScript frontend, native GPUI rendering.
//!
//! One JSONL mutation protocol, three frontend topologies:
//!
//! ```text
//!   TypeScript/TSX frontend --Perry--> native binary   (child process)
//!   TypeScript/TSX frontend --Perry--> staticlib       (embedded, perry runtime)
//!   TypeScript/TSX frontend --esbuild-> JS bundle      (embedded, QuickJS runtime)
//!        |                                        |  JSONL mutation protocol
//!        v                                        v
//!   Solid-style reactive code         this host: retained tree -> GPUI (GPU)
//! ```
//!
//! Child mode spawns the frontend process and pipes its stdio. Embedded mode
//! (cfg `embedded`, enabled by build.rs when perry staticlibs are present)
//! links the frontend INTO this process: stdio is redirected into anonymous
//! pipes, `perry_module_init()` runs the TS top level, and the host drives
//! Perry's event loop with `perry_poll()` (upstream #1088).
//!
//! QuickJS mode (cfg `quickjs`, the default) links `rquickjs` instead and runs
//! the bundle on a dedicated engine thread. Being in-process, it can carry the
//! protocol two ways — `Transport::Direct` injects `__hostEmit` into the JS
//! context and pushes events straight into the engine queue, while
//! `Transport::Pipe` keeps the historical stdio carrier (see `quickjs.rs`).
//! Both funnel through `ingest_line` below, so they cannot drift apart.

mod tree;

#[cfg(has_embedded_frontend)]
// Compiled in both embedded and quickjs modes; in quickjs mode the Perry
// poll-loop helpers are simply unused, which is expected (not a defect).
#[cfg_attr(quickjs, allow(dead_code, unused_imports))]
mod embedded;

#[cfg(quickjs)]
mod quickjs;

use tree::{Op, Tree};

use gpui::{
    div, prelude::*, px, relative, rgb, rgba, size, point, AnyElement, App, Bounds, ClickEvent,
    Context, Div, ElementId, FontWeight, InteractiveElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, TitlebarOptions,
};
use gpui_platform::application;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
#[cfg(not(any(embedded, quickjs)))]
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};

// ---------------------------------------------------------------------------
// Host logging: writes to the ORIGINAL stderr handle. In embedded mode the
// process std handles are redirected into the frontend pipes, so plain
// eprintln! would leak into the ops channel — always go through `log!`.
// ---------------------------------------------------------------------------

static LOG_HANDLE: AtomicUsize = AtomicUsize::new(0);

fn init_logging() {
    #[cfg(has_embedded_frontend)]
    {
        let h = embedded::save_original_stderr();
        LOG_HANDLE.store(h as usize, Ordering::Relaxed);
    }
    #[cfg(not(has_embedded_frontend))]
    {
        LOG_HANDLE.store(usize::MAX, Ordering::Relaxed); // plain eprintln fallback
    }
}

fn log_line(s: &str) {
    let h = LOG_HANDLE.load(Ordering::Relaxed);
    if h == 0 {
        return;
    }
    if h == usize::MAX {
        eprintln!("{s}");
        return;
    }
    #[cfg(has_embedded_frontend)]
    {
        use std::fs::File;
        use std::os::windows::io::FromRawHandle;
        let mut f = unsafe { File::from_raw_handle(h as *mut core::ffi::c_void) };
        let _ = f.write_all(s.as_bytes());
        let _ = f.write_all(b"\n");
        let _ = f.flush();
        std::mem::forget(f); // raw handle must not be closed
    }
}

macro_rules! log {
    ($($arg:tt)*) => { log_line(&format!($($arg)*)) };
}

/// Epoch milliseconds, aligned with the frontend's `Date.now()` so host and
/// frontend trace lines can be paired directly to measure interaction latency.
pub(crate) fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Style application: CSS-ish keys -> GPUI builder calls
//
// Div is move-only, so each optional style goes through a helper that takes
// the element by value and returns it untouched when the value is absent.
// ---------------------------------------------------------------------------

/// Parses `#rrggbb` / `#rrggbbaa` into the u32 layout gpui's `rgba()` expects:
/// 0xRRGGBBAA (big-endian bytes -> r, g, b, a).
fn parse_color(s: &str) -> Option<u32> {
    let hex = s.strip_prefix('#')?;
    match hex.len() {
        6 => u32::from_str_radix(hex, 16).ok().map(|v| (v << 8) | 0xFF),
        8 => u32::from_str_radix(hex, 16).ok(),
        _ => None,
    }
}

fn num(v: &Value) -> Option<f32> {
    v.as_f64().map(|f| f as f32)
}

fn with_px(d: Div, v: &Value, f: impl FnOnce(Div, f32) -> Div) -> Div {
    match num(v) {
        Some(x) => f(d, x),
        None => d,
    }
}

fn with_color(d: Div, v: &Value, f: impl FnOnce(Div, u32) -> Div) -> Div {
    match v.as_str().and_then(parse_color) {
        Some(c) => f(d, c),
        None => d,
    }
}

fn with_pct(d: Div, s: &str, f: impl FnOnce(Div, f32) -> Div) -> Div {
    match s.strip_suffix('%').and_then(|p| p.parse::<f32>().ok()) {
        Some(frac) => f(d, frac / 100.0),
        None => d,
    }
}

fn apply_style(d: Div, key: &str, v: &Value) -> Div {
    match key {
        // layout
        "flexDirection" | "direction" => match v.as_str().unwrap_or("") {
            "column" => d.flex_col(),
            _ => d.flex_row(),
        },
        "gap" => with_px(d, v, |d, x| d.gap(px(x))),
        "padding" => with_px(d, v, |d, x| d.p(px(x))),
        "paddingX" => with_px(d, v, |d, x| d.px(px(x))),
        "paddingY" => with_px(d, v, |d, x| d.py(px(x))),
        "paddingLeft" => with_px(d, v, |d, x| d.pl(px(x))),
        "paddingRight" => with_px(d, v, |d, x| d.pr(px(x))),
        "paddingTop" => with_px(d, v, |d, x| d.pt(px(x))),
        "paddingBottom" => with_px(d, v, |d, x| d.pb(px(x))),
        "width" => match v {
            Value::Number(_) => with_px(d, v, |d, x| d.w(px(x))),
            Value::String(s) => match s.as_str() {
                "full" => d.w_full(),
                "auto" => d.w_auto(),
                p => with_pct(d, p, |d, f| d.w(relative(f))),
            },
            _ => d,
        },
        "height" => match v {
            Value::Number(_) => with_px(d, v, |d, x| d.h(px(x))),
            Value::String(s) => match s.as_str() {
                "full" => d.h_full(),
                "auto" => d.h_auto(),
                p => with_pct(d, p, |d, f| d.h(relative(f))),
            },
            _ => d,
        },
        "minWidth" => with_px(d, v, |d, x| d.min_w(px(x))),
        "minHeight" => with_px(d, v, |d, x| d.min_h(px(x))),
        "maxWidth" => with_px(d, v, |d, x| d.max_w(px(x))),
        "maxHeight" => with_px(d, v, |d, x| d.max_h(px(x))),
        "grow" | "flex" => with_px(d, v, |d, x| d.flex_grow(x)),
        "shrink" => with_px(d, v, |d, x| d.flex_shrink(x)),
        "alignItems" | "align" => match v.as_str().unwrap_or("") {
            "center" => d.items_center(),
            "start" => d.items_start(),
            "end" => d.items_end(),
            "stretch" => d.items_stretch(),
            "baseline" => d.items_baseline(),
            _ => d,
        },
        "justifyContent" | "justify" => match v.as_str().unwrap_or("") {
            "center" => d.justify_center(),
            "start" => d.justify_start(),
            "end" => d.justify_end(),
            "between" | "space-between" => d.justify_between(),
            _ => d,
        },
        // visual
        "background" | "bg" | "backgroundColor" => with_color(d, v, |d, c| d.bg(rgba(c))),
        "borderRadius" | "radius" => with_px(d, v, |d, x| d.rounded(px(x))),
        "borderWidth" => match num(v) {
            Some(n) if n > 0.0 => d.border_1(),
            _ => d,
        },
        "borderColor" => with_color(d, v, |d, c| d.border_color(rgba(c))),
        "opacity" => with_px(d, v, |d, x| d.opacity(x)),
        // text
        "color" => with_color(d, v, |d, c| d.text_color(rgba(c))),
        "fontSize" => with_px(d, v, |d, x| d.text_size(px(x))),
        "fontWeight" => match v {
            Value::String(s) if s == "bold" => d.font_weight(FontWeight::BOLD),
            Value::String(s) if s == "semibold" => d.font_weight(FontWeight::SEMIBOLD),
            Value::String(s) if s == "medium" => d.font_weight(FontWeight::MEDIUM),
            Value::Number(n) => d.font_weight(FontWeight(n.as_f64().unwrap_or(400.0) as f32)),
            _ => d,
        },
        _ => d,
    }
}

// ---------------------------------------------------------------------------
// Host view: retained tree -> GPUI elements
// ---------------------------------------------------------------------------

struct HostView {
    tree: Tree,
    event_tx: mpsc::Sender<String>,
}

/// Deterministic application order for style keys (HashMap iteration is
/// random; compound keys like `padding` must apply before `paddingX`).
fn style_rank(k: &str) -> u8 {
    match k {
        "padding" => 0,
        "paddingX" | "paddingY" => 1,
        "paddingLeft" | "paddingRight" | "paddingTop" | "paddingBottom" => 2,
        "width" | "height" | "minWidth" | "minHeight" | "maxWidth" | "maxHeight" => 3,
        "flexDirection" | "gap" | "grow" | "flex" | "shrink" => 4,
        "alignItems" | "align" | "justifyContent" | "justify" => 5,
        "borderWidth" => 6,
        "borderColor" => 7,
        "borderRadius" | "radius" => 8,
        "background" | "bg" | "backgroundColor" => 9,
        "opacity" => 10,
        "color" => 11,
        "fontSize" => 12,
        "fontWeight" => 13,
        _ => 14,
    }
}

impl HostView {
    fn build_node(&self, id: u64, cx: &mut Context<Self>) -> Option<AnyElement> {
        let node = self.tree.node(id)?;
        let mut d = div().flex().flex_row();

        // style — sorted for deterministic compound-key semantics
        let mut pairs: Vec<(&String, &Value)> = node.style.iter().collect();
        pairs.sort_by_key(|(k, _)| style_rank(k));
        for (k, v) in pairs {
            d = apply_style(d, k, v);
        }

        // text content (text nodes carry a string)
        if let Some(text) = &node.text {
            d = d.child(SharedString::from(text.clone()));
        }

        // children
        for child in &node.children {
            if let Some(el) = self.build_node(*child, cx) {
                d = d.child(el);
            }
        }

        // interactivity: id first (makes the div stateful), then scroll/overflow
        // and listeners — those methods live on StatefulInteractiveElement.
        let element_id = ElementId::from(SharedString::from(format!("node-{}", id)));
        let mut s = d.id(element_id);
        if node.style.get("overflow").and_then(|v| v.as_str()) == Some("scroll") {
            s = s.overflow_y_scroll();
        }
        if node.events.iter().any(|e| e == "click") {
            s = s.on_click(cx.listener(move |this, _ev: &ClickEvent, _window, _cx| {
                let msg = json!({ "t": "event", "target": id, "kind": "click" }).to_string();
                // C3 latency: epoch ms aligned with the frontend's Date.now()
                log!("[host] click id={id} t={}", now_ms());
                let _ = this.event_tx.send(msg);
            }));
        }

        Some(s.into_any_element())
    }
}

impl Render for HostView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let root = self
            .build_node(0, cx)
            .unwrap_or_else(|| div().into_any_element());
        div()
            .id("host-root")
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x0f1117))
            .child(root)
    }
}

// ---------------------------------------------------------------------------
// JSONL transport (shared by both modes): reader parses mutation batches,
// writer ships event lines. `hello` lines are diagnostics only.
// ---------------------------------------------------------------------------

/// Parse one protocol line and hand the resulting batch to the UI.
///
/// This is THE protocol dialect — every carrier funnels through it, so the
/// pipe transport (`run_ops_reader`) and the injected transport (the QuickJS
/// `__hostEmit` host function) can never drift apart.
pub(crate) fn ingest_line(line: &str, ops_tx: &async_channel::Sender<Vec<Op>>) {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return;
    };
    match v.get("t").and_then(|t| t.as_str()) {
        Some("hello") => {
            log!("[host] frontend hello: {line}");
        }
        Some("batch") => {
            if let Some(ops) = v.get("ops").and_then(|o| o.as_array()) {
                let total = ops.len();
                let parsed: Vec<Op> = ops.iter().filter_map(tree::parse_op).collect();
                if parsed.len() != total {
                    // 协议层告警：parse_op 静默丢弃说明前端发了不合法的 op
                    log!(
                        "[host] WARNING batch dropped {}/{} ops: {}",
                        total - parsed.len(),
                        total,
                        line
                    );
                }
                let _ = ops_tx.send_blocking(parsed);
            }
        }
        _ => {}
    }
}

fn run_ops_reader<R: std::io::Read + Send + 'static>(
    r: R,
    ops_tx: async_channel::Sender<Vec<Op>>,
) {
    std::thread::spawn(move || {
        let reader = BufReader::new(r);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            ingest_line(&line, &ops_tx);
        }
        log!("[host] frontend stream closed");
    });
}

fn run_event_writer<W: std::io::Write + Send + 'static>(mut w: W, ev_rx: mpsc::Receiver<String>) {
    std::thread::spawn(move || {
        for msg in ev_rx {
            log!("[host] ev write t={}: {msg}", now_ms());
            if writeln!(w, "{msg}").and_then(|_| w.flush()).is_err() {
                log!("[host] ev write FAILED (frontend stdin closed?)");
                break;
            }
        }
    });
}

/// Injected transport (QuickJS `Direct` mode): hand each event straight to the
/// engine's inbound queue and unpark its tick.
///
/// Compared with `run_event_writer` this drops a pipe write, a blocking read,
/// a line split and a UTF-8 decode — and because `push` unparks the engine,
/// dispatch happens immediately instead of on the next 10ms tick.
#[cfg(quickjs)]
fn run_event_injector(sink: quickjs::EventSink, ev_rx: mpsc::Receiver<String>) {
    std::thread::spawn(move || {
        log!("[host] event injector up");
        for msg in ev_rx {
            log!("[host] ev inject t={}: {msg}", now_ms());
            sink.push(msg);
        }
    });
}

// ---------------------------------------------------------------------------
// Frontend selection
// ---------------------------------------------------------------------------

enum Frontend {
    /// External process (perry-app.exe or node ui/dist/main.js) — for
    /// cross-mode comparison and non-linked builds.
    Child { cmd: String, args: Vec<String> },
    /// Perry staticlib linked into this process (build.rs cfg).
    #[cfg(embedded)]
    Embedded,
    /// QuickJS engine linked into this process (build.rs cfg=quickjs).
    #[cfg(quickjs)]
    QuickJs { transport: quickjs::Transport },
}

/// Transport selection: `--transport pipe|direct` wins over
/// `GPUI_TS_TRANSPORT=pipe|direct`, default `direct`.
#[cfg(quickjs)]
fn transport_from_args() -> quickjs::Transport {
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        if a == "--transport" {
            if let Some(v) = it.next() {
                return quickjs::Transport::parse(&v);
            }
        }
        if let Some(v) = a.strip_prefix("--transport=") {
            return quickjs::Transport::parse(v);
        }
    }
    quickjs::Transport::from_env()
}

/// First positional argv (skipping transport flags), i.e. the frontend to run.
fn positional_arg() -> Option<String> {
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        if a == "--transport" {
            let _ = it.next();
            continue;
        }
        if a.starts_with("--transport=") {
            continue;
        }
        return Some(a);
    }
    None
}

fn resolve_frontend() -> Frontend {
    if let Some(arg) = positional_arg() {
        if arg.ends_with(".js") {
            return Frontend::Child { cmd: "node".to_string(), args: vec![arg] };
        }
        return Frontend::Child { cmd: arg, args: vec![] };
    }
    #[cfg(quickjs)]
    {
        return Frontend::QuickJs { transport: transport_from_args() };
    }
    #[cfg(embedded)]
    {
        return Frontend::Embedded;
    }
    #[cfg(not(any(embedded, quickjs)))]
    {
        if Path::new("build/perry-app.exe").exists() {
            return Frontend::Child { cmd: "build/perry-app.exe".to_string(), args: vec![] };
        }
        if Path::new("ui/dist/main.js").exists() {
            return Frontend::Child {
                cmd: "node".to_string(),
                args: vec!["ui/dist/main.js".to_string()],
            };
        }
        log!("[host] no frontend found: pass a .exe path (Perry-compiled) or a .js path (node) as argv[1]");
        std::process::exit(2);
    }
}

fn main() {
    init_logging();
    let t_start = std::time::Instant::now();

    let frontend = resolve_frontend();

    // --- per-mode transport wiring ---
    let (ev_tx, ev_rx) = mpsc::channel::<String>();
    let (ops_tx, ops_rx) = async_channel::unbounded::<Vec<Op>>();

    match frontend {
        Frontend::Child { cmd, args } => {
            let mut c = Command::new(&cmd)
                .args(&args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap_or_else(|e| {
                    log!("[host] failed to spawn frontend `{cmd}`: {e}");
                    std::process::exit(1);
                });
            log!("[host] frontend spawned: {cmd} {}", args.join(" "));
            let stdout = c.stdout.take().expect("child stdout");
            let stdin = c.stdin.take().expect("child stdin");
            let child = Arc::new(std::sync::Mutex::new(c));

            run_event_writer(stdin, ev_rx);
            run_ops_reader(stdout, ops_tx);

            application().run(move |cx: &mut App| {
                let handle = open_host_window(cx, ev_tx);
                spawn_ops_apply(cx, handle, ops_rx);

                // watchdog: if the frontend dies, close the app
                cx.spawn(async move |cx| loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(2))
                        .await;
                    let exited =
                        child.lock().unwrap().try_wait().map_or(true, |s| s.is_some());
                    if exited {
                        log!("[host] frontend exited, closing");
                        let _ = cx.update(|cx| cx.quit());
                        break;
                    }
                })
                .detach();
            });
        }
        #[cfg(embedded)]
        Frontend::Embedded => {
            log!("[host] embedded mode: perry staticlib in-process");
            let (ef, out_r) = embedded::launch(embedded::save_original_stderr());
            log!("[host] perry_module_init done in {:?}", t_start.elapsed());

            run_event_writer(ef, ev_rx);
            run_ops_reader(out_r, ops_tx);

            application().run(move |cx: &mut App| {
                let handle = open_host_window(cx, ev_tx);
                spawn_ops_apply(cx, handle, ops_rx);

                // Drive Perry's event loop from the GPUI executor: drains
                // microtasks + stdlib pump (timers / stdin / fs / fetch).
                // The 10ms quantum matches the frontend's keepalive heartbeat.
                // perry_poll covers microtasks+timers; the stdlib pump
                // (readline stdin dispatch etc.) is driven explicitly — see
                // embedded.rs for why the indirect STDLIB_PUMP_FN register
                // chain can't be trusted under /FORCE:MULTIPLE.
                cx.spawn(async move |cx| loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(10))
                        .await;
                    unsafe {
                        embedded::perry_poll();
                        embedded::js_stdlib_process_pending();
                    }
                })
                .detach();
            });
        }
        #[cfg(quickjs)]
        Frontend::QuickJs { transport } => {
            log!("[host] quickjs mode: embedded QuickJS engine (transport={transport:?})");
            let host = quickjs::launch(quickjs::save_original_stderr(), transport, ops_tx.clone());
            log!("[host] quickjs engine booted in {:?}", t_start.elapsed());

            // Pipe mode is byte-for-byte the historical path (kept as the
            // regression baseline and the remote-debug path). Direct mode skips
            // stdio entirely and is delivered through the injected sink.
            if transport == quickjs::Transport::Pipe {
                run_event_writer(
                    host.frontend.expect("pipe transport: stdin handle"),
                    ev_rx,
                );
                run_ops_reader(host.out_r.expect("pipe transport: stdout reader"), ops_tx);
            } else {
                run_event_injector(host.sink.clone(), ev_rx);
            }

            let sink = host.sink.clone();
            application().run(move |cx: &mut App| {
                let handle = open_host_window(cx, ev_tx);
                spawn_ops_apply(cx, handle, ops_rx);
                // The QuickJS engine self-drives on its own thread (tick +
                // park/unpark); no perry_poll loop needed here.
            });

            // Window closed → let the app's stdin `end` handler run (it calls
            // process.exit). Best-effort: the engine owns the exit.
            sink.close();
        }
    }
}

fn open_host_window(cx: &mut App, ev_tx: mpsc::Sender<String>) -> gpui::WindowHandle<HostView> {
    let bounds = Bounds {
        origin: point(px(80.0), px(50.0)),
        size: size(px(1180.0), px(760.0)),
    };
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::from("PerryTS × GPUI")),
            ..Default::default()
        }),
        window_min_size: Some(size(px(700.0), px(480.0))),
        ..Default::default()
    };

    let handle = cx
        .open_window(options, |_, cx| {
            cx.new(|_| HostView {
                tree: Tree::new(),
                event_tx: ev_tx,
            })
        })
        .expect("failed to open window");
    cx.activate(true);
    log!("[host] window opened");
    handle
}

fn spawn_ops_apply(
    cx: &mut App,
    handle: gpui::WindowHandle<HostView>,
    ops_rx: async_channel::Receiver<Vec<Op>>,
) {
    cx.spawn(async move |cx| {
        while let Ok(ops) = ops_rx.recv().await {
            let _ = cx.update(|cx| {
                let _ = handle.update(cx, |view, window, cx| {
                    if let Some(title) = view.tree.apply(&ops) {
                        window.set_window_title(&title);
                    }
                    cx.notify();
                });
            });
        }
    })
    .detach();
}
