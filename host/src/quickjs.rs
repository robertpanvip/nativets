//! QuickJS-embedded frontend (replaces the Perry runtime layer).
//!
//! Same JSONL mutation protocol as `embedded.rs` — the tree.rs parser and the
//! TS frontend are transport-agnostic, so nothing in the UI host changes.
//!
//! Why QuickJS instead of Perry here:
//!   * we own the event loop: `pump()` drains promise jobs + fires timers +
//!     dispatches queued events on a self-driven tick (no 500ms I/O quantum,
//!     no indirect stdlib-pump registration chain that silently dropped
//!     stdin dispatch under /FORCE:MULTIPLE)
//!   * no duplicate Rust std in the link (Perry bundles one) → /FORCE:MULTIPLE
//!     and its symbol-splitting hazards disappear
//!   * QuickJS C core is ~1-2MB vs Perry's 5.9MB runtime → smaller exe
//!   * because the engine is *in process*, the transport is an injected host
//!     function rather than a faked stdio pipe (see the Transport note below)
//!
//! Threading: a `QuickJS Context` is `!Send`, so the engine runs on its own
//! dedicated OS thread. The host communicates with it only through an inbound
//! queue (`Arc<Mutex<Vec<String>>>`) and the injected `__hostEmit` host
//! function.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use rquickjs::function::Args;
use rquickjs::{Array, Context, Ctx, Function, Object, Runtime};

use crate::bom;
use crate::tree::Op;

// --- minimal Win32 surface (no external deps) ---
type Handle = *mut core::ffi::c_void;
const STD_ERROR_HANDLE: u32 = 0xFFFF_FFF4;

unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
}

/// Sentinel pushed to the inbound queue when the host shuts down, so the app's
/// `process.stdin.on("end", ...)` handler fires. It can never collide with a
/// real event line (which always starts with `{`), and is never dispatched to
/// the `data` handler.
const QJS_EOF_MARKER: &str = "\u{0}__qjs_eof__\u{0}";

/// Engine tick cadence. Also the idle wake-up period when parked.
const TICK: Duration = Duration::from_millis(10);

/// Prefix that marks a host-internal BOM geometry push on the event channel.
/// `{"t":"bom"` — checked instead of parsing every line, and can never collide
/// with an app event line (those are `{"t":"event"…}`) or with `QJS_EOF_MARKER`.
const BOM_LINE_PREFIX: &str = "{\"t\":\"bom\"";

/// The JS environment installed before the app bundle runs (timers, `process`
/// shim, console, and the BOM). Kept as a real `.js` file rather than a Rust
/// raw string so it is editable/lintable and so the node test suite can load
/// the exact same source. `build.rs` declares a rerun-if-changed on it.
const BOOTSTRAP_JS: &str = include_str!("bootstrap.js");

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------
//
// There is exactly one carrier now: the host injects `__hostEmit` as a JS
// global, so a batch of ops goes straight from the engine into the host's ops
// channel, and events are pushed straight into the engine's inbound queue plus
// an `unpark`.
//
// The historical alternative — faking a stdio pipe between the host and an
// in-process engine (`SetStdHandle` redirection, a reader thread parsing lines
// off the out pipe, a writer thread pushing events into the in pipe, then a
// second reader turning them back into lines) — has been removed. It existed
// because the Perry-era frontend was a real child process, but once the engine
// moved in-process it bought nothing and cost two threads plus one full tick of
// latency per event (see docs/gpui-ts-plan.md). It also kept the misleading
// "stdio *is* the protocol" framing alive.
//
// Out-of-process frontends (`Frontend::Child`, `Frontend::Embedded`) keep their
// own stdio/pipe wiring in `main.rs` and `embedded.rs`; nothing here applies to
// them.

// ---------------------------------------------------------------------------
// Host-side handles
// ---------------------------------------------------------------------------

/// Inbound event injection (host → engine). `push` is callable from any thread
/// and wakes the parked engine thread immediately, so an event is dispatched
/// without waiting for the next tick.
#[derive(Clone)]
pub struct EventSink {
    queue: Arc<Mutex<Vec<String>>>,
    engine: thread::Thread,
}

impl EventSink {
    pub fn push(&self, line: String) {
        self.queue.lock().unwrap().push(line);
        self.engine.unpark();
    }

    /// Ask the engine to shut down: the app's stdin `end` handler runs, which
    /// normally calls `process.exit`. Async by design — the handler owns the
    /// exit (and does it on the engine thread, where the JS context lives).
    pub fn close(&self) {
        self.push(QJS_EOF_MARKER.to_string());
    }
}

/// Everything `main` needs from the QuickJS backend.
pub struct QuickJsHost {
    /// The injected event channel — the only carrier there is.
    pub sink: EventSink,
}

/// Save the original stderr BEFORE any redirection so host diagnostics survive.
pub fn save_original_stderr() -> Handle {
    unsafe { GetStdHandle(STD_ERROR_HANDLE) }
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// Engine state owned by the dedicated engine thread.
struct Engine {
    /// Owns the JS runtime. Never read directly, but must stay alive for as
    /// long as `ctx` lives — dropping it would invalidate the context.
    #[allow(dead_code)]
    rt: Runtime,
    ctx: Context,
    /// host → engine, drained every tick.
    queue: Arc<Mutex<Vec<String>>>,
    /// engine → host, handed to the injected `__hostEmit`.
    ops_tx: async_channel::Sender<Vec<Op>>,
    stderr_tee: usize,
    /// BOM: window/display geometry, written by the GPUI thread.
    metrics: Arc<bom::WindowMetrics>,
    /// BOM: `alert`/`confirm` requests → UI thread.
    dialogs: async_channel::Sender<bom::DialogRequest>,
}

impl Engine {
    fn new(
        stderr_tee: usize,
        queue: Arc<Mutex<Vec<String>>>,
        ops_tx: async_channel::Sender<Vec<Op>>,
        metrics: Arc<bom::WindowMetrics>,
        dialogs: async_channel::Sender<bom::DialogRequest>,
    ) -> Self {
        let rt = Runtime::new().expect("[qjs] Runtime::new failed");
        let ctx = Context::full(&rt).expect("[qjs] Context::full failed");
        Engine {
            rt,
            ctx,
            queue,
            ops_tx,
            stderr_tee,
            metrics,
            dialogs,
        }
    }

    /// Boot the engine: register host globals, eval bootstrap + app bundle,
    /// then drive the event loop forever on this thread.
    fn start(&self, bundle: String) {
        self.ctx.with(|ctx| {
            self.register_globals(ctx.clone());
            if let Err(e) = ctx.eval::<(), _>(BOOTSTRAP_JS) {
                trace(self.stderr_tee, &format!("[qjs] bootstrap eval error: {:?}", e));
            }
            if bundle.is_empty() {
                crate::log_line(
                    "[qjs] empty bundle — no UI (run `node ui/build-qjs.mjs` then rebuild the host)",
                );
                return;
            }
            // Hand the bundle to the engine as a global string, then run it via
            // an indirect eval wrapped in try/catch so a load-time throw surfaces
            // its real JS message + stack (rquickjs's `Error` Debug is just
            // "Exception", so we report from JS instead).
            let _ = ctx.globals().set("__BUNDLE", bundle);
            let runner = r#"
                (function () {
                    try {
                        (0, eval)(globalThis.__BUNDLE);
                        globalThis.__hostTrace("info", "[qjs] app bundle evaluated");
                    } catch (e) {
                        var info = "type=" + typeof e;
                        try { info += " ctor=" + (e && e.constructor && e.constructor.name); } catch (_) {}
                        try { info += " msg=" + (e && e.message); } catch (_) {}
                        globalThis.__hostTrace("error", "[qjs] BUNDLE_THROW: " + info + " | stack=" + ((e && e.stack) || "none"));
                    }
                })();
            "#;
            if let Err(e) = ctx.eval::<(), _>(runner.to_string()) {
                trace(self.stderr_tee, &format!("[qjs] runner eval error: {}", e));
            }
        });

        // Event loop. We *park* rather than sleep, so an inbound event wakes us
        // the moment it arrives (worst case still one tick).
        loop {
            thread::park_timeout(TICK);
            self.pump();
        }
    }

    /// Register the host-provided globals the TS frontend relies on. Only the
    /// raw-handle-capturing primitives (__hostWrite/__hostTrace/__hostEmit) and
    /// trivial no-ops are Rust functions; everything that must persist a JS
    /// callback (stdin.on, console) is defined in the bootstrap to avoid
    /// capturing `Ctx` (which would be a borrow escape).
    fn register_globals(&self, ctx: Ctx<'_>) {
        let stderr_tee = self.stderr_tee;

        // --- host primitives (capture usize handles / a channel; no Ctx) ---
        let host_write = Function::new(ctx.clone(), move |s: String| {
            stdout_sink(stderr_tee, &s);
        });
        let host_trace = Function::new(ctx.clone(), move |level: String, msg: String| {
            trace(stderr_tee, &format!("[console.{}] {}", level, msg));
        });
        let _ = ctx.globals().set("__hostWrite", host_write);
        let _ = ctx.globals().set("__hostTrace", host_trace);

        // --- the transport ---
        // `__hostEmit(line)` hands one protocol line to the host's ops channel
        // on the calling (engine) thread: no pipe write, no reader thread, no
        // line re-splitting. Ops are parsed here rather than downstream because
        // this thread is idle ~99% of the time anyway.
        //
        // This global is *the* carrier — the frontend requires it (it used to
        // feature-detect `typeof __hostEmit` and fall back to writing JSONL on
        // stdout, which is gone; see the Transport note at the top).
        let tx = self.ops_tx.clone();
        let host_emit = Function::new(ctx.clone(), move |line: String| {
            crate::ingest_line(&line, &tx);
        });
        let _ = ctx.globals().set("__hostEmit", host_emit);

        // --- process ---
        let process = Object::new(ctx.clone()).unwrap();
        let stdout = Object::new(ctx.clone()).unwrap();
        let stdout_write = Function::new(ctx.clone(), move |s: String| {
            stdout_sink(stderr_tee, &s);
        });
        let _ = stdout.set("write", stdout_write);
        let _ = process.set("stdout", stdout);

        // stderr → original stderr tee (never a protocol pipe; trace must stay
        // off the JSONL stream). `stderr_tee` is Copy (usize).
        let stderr = Object::new(ctx.clone()).unwrap();
        let stderr_write = Function::new(ctx.clone(), move |s: String| {
            trace(stderr_tee, &s);
        });
        let _ = stderr.set("write", stderr_write);
        let _ = process.set("stderr", stderr);

        let stdin = Object::new(ctx.clone()).unwrap();
        let stdin_setenc = Function::new(ctx.clone(), |_enc: String| {});
        let _ = stdin.set("setEncoding", stdin_setenc);
        // `on` is installed by the bootstrap (needs to store a JS callback)
        let _ = process.set("stdin", stdin);

        let env = Object::new(ctx.clone()).unwrap();
        for (k, v) in std::env::vars() {
            let _ = env.set(k, v);
        }
        // Engine marker for the frontend: the app reports it in the status bar
        // ("frontend: <engine>"). Perry/node runs see the ambient env instead.
        let _ = env.set("GPUI_TS_ENGINE", "quickjs");
        let _ = process.set("env", env);

        let argv = Array::new(ctx.clone()).unwrap();
        let _ = argv.set(0, "host");
        let _ = process.set("argv", argv);

        // `process.exit` is a JS wrapper (bootstrap) over this real exit hook.
        // Return type pinned to `()` — `std::process::exit` diverges (`!`), which
        // is not an `IntoJs` return type for rquickjs.
        let host_exit = Function::new(ctx.clone(), |code: i32| -> () {
            std::process::exit(code);
        });
        let _ = ctx.globals().set("__hostExit", host_exit);

        // --- BOM primitives (consumed by bootstrap.js) ---
        // Window geometry: pull once at boot. Live updates ride the event
        // channel as `{"t":"bom",…}` lines (see `pump`).
        let metrics = self.metrics.clone();
        let host_window = Function::new(ctx.clone(), move || -> String { metrics.json() });
        let _ = ctx.globals().set("__hostWindow", host_window);

        // Monotonic clock for `performance.now()`.
        let host_perf = Function::new(ctx.clone(), || -> f64 { bom::perf_now_ms() });
        let _ = ctx.globals().set("__hostPerfNow", host_perf);

        // OS CSPRNG for `crypto.getRandomValues` / `crypto.randomUUID`.
        // Clamped: JS must not be able to ask for an unbounded allocation.
        let host_entropy = Function::new(ctx.clone(), |n: i32| -> String {
            bom::entropy_hex(n.clamp(0, 65536) as usize)
        });
        let _ = ctx.globals().set("__hostEntropy", host_entropy);

        // alert/confirm. `bom::show` BLOCKS until the user answers — that is
        // the browser semantic (`confirm()` really does stop JS). It returns
        // `None` only when the UI side is gone, and then we must not block.
        let dialogs = self.dialogs.clone();
        let host_dialog = Function::new(
            ctx.clone(),
            move |kind: String, message: String, _title: String| -> i32 {
                let k = bom::DialogKind::parse(&kind);
                match bom::show(&dialogs, k, message) {
                    Some(v) => v,
                    None if k.is_confirm() => 0,
                    None => 1,
                }
            },
        );
        let _ = ctx.globals().set("__hostDialog", host_dialog);

        // The host is Windows-only today (the pipe/handle FFI above is Win32),
        // so `navigator.platform`/`userAgent` can report it honestly.
        let _ = process.set("platform", "win32");
        let _ = ctx.globals().set("process", process);
    }

    /// One tick: drain promise jobs, fire due timers, dispatch inbound events.
    ///
    /// Microtasks are drained after *each* phase rather than only once at the
    /// top. The frontend batches its ops with `Promise.resolve().then(flush)`,
    /// so a click handler that ran in phase 3 only reaches the host if we drain
    /// again right after it — otherwise the flush waits for the next tick and
    /// adds a full TICK (~10ms) of visible latency to every interaction.
    fn pump(&self) {
        self.ctx.with(|ctx| {
            // 1) drain promise jobs left over from the previous tick
            while ctx.execute_pending_job() {}

            // 2) fire due timers (JS __hostTick)
            if let Ok(f) = ctx.globals().get::<_, Function>("__hostTick") {
                let _ = f.call::<(), ()>(());
            }
            while ctx.execute_pending_job() {}

            // 3) dispatch queued event lines to the 'data' handler; an EOF
            //    marker instead triggers the app's 'end' handler.
            if let Ok(cbs) = ctx.globals().get::<_, Object>("__stdinCbs") {
                let lines: Vec<String> = {
                    let mut q = self.queue.lock().unwrap();
                    std::mem::take(&mut *q)
                };
                for line in lines {
                    if line == QJS_EOF_MARKER {
                        if let Ok(end) = cbs.get::<_, Function>("end") {
                            let _ = end.call::<(), ()>(());
                        }
                        continue;
                    }
                    // BOM geometry pushes are host-internal. Apply them and keep
                    // them off the app's stdin stream — the app would only see
                    // an unknown `t` it has no handler for, and a future app
                    // that switches on `t` would be surprised by a private one.
                    if line.starts_with(BOM_LINE_PREFIX) {
                        if let Ok(f) = ctx.globals().get::<_, Function>("__bomLine") {
                            let mut a = Args::new(ctx.clone(), 1);
                            let _ = a.push_arg(line);
                            let _ = f.call_arg::<()>(a);
                        }
                        continue;
                    }
                    if let Ok(cb) = cbs.get::<_, Function>("data") {
                        let mut a = Args::new(ctx.clone(), 1);
                        let _ = a.push_arg(line + "\n");
                        let _ = cb.call_arg::<()>(a);
                    }
                }
            }
            // 4) flush the batches those handlers just queued
            while ctx.execute_pending_job() {}
        });
    }
}

/// Boot the QuickJS engine on its own thread and return the host-side handle.
///
/// Nothing is redirected: the engine reaches the host through injected
/// functions (`__hostEmit` for outbound ops, `__hostWrite`/`__hostTrace` for
/// diagnostics) and the shared queue, and stderr keeps pointing at the real
/// terminal so JS `console` output flows through.
pub fn launch(
    frontend_stderr_tee: RawHandle,
    ops_tx: async_channel::Sender<Vec<Op>>,
    metrics: Arc<bom::WindowMetrics>,
    dialogs: async_channel::Sender<bom::DialogRequest>,
) -> QuickJsHost {
    let queue: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let bundle = load_bundle();
    let stderr_tee = frontend_stderr_tee as usize;

    let engine_thread = thread::Builder::new()
        .name("quickjs".to_string())
        .spawn({
            let queue = queue.clone();
            move || {
                let engine = Engine::new(stderr_tee, queue, ops_tx, metrics, dialogs);
                engine.start(bundle);
            }
        })
        .expect("[qjs] failed to spawn engine thread");

    QuickJsHost {
        sink: EventSink {
            queue,
            engine: engine_thread.thread().clone(),
        },
    }
}

// ---------------------------------------------------------------------------
// plumbing helpers
// ---------------------------------------------------------------------------

/// Marker appended by `gpui-ts build` to a copy of the host, immediately before
/// the application's JS bundle. Picked so a JS source file cannot contain it by
/// accident.
pub const TAIL_MARKER: &[u8] = b"\n<<<GPUI_TS_BUNDLE_v1>>>\n";

/// How much of the executable is scanned for the marker. The bundle is a few
/// hundred KB and the host ~11MB, so there is no point reading all of it.
const TAIL_SCAN_BYTES: u64 = 8 * 1024 * 1024;

/// Read a bundle appended to *this* executable.
///
/// This is what makes the published package toolchain-free: `gpui-ts build`
/// copies the prebuilt host and appends marker + bundle, so someone with no Rust
/// toolchain still ends up with one self-contained exe (Windows' PE loader
/// ignores trailing bytes). It wins over the compiled-in bundle on purpose — a
/// *published* host carries this repo's demo app, and the appended one is the
/// user's.
fn bundle_from_self() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let mut f = File::open(&exe).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_SCAN_BYTES);
    if start > 0 {
        f.seek(SeekFrom::Start(start)).ok()?;
    }
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    // The *last* marker wins, so a bundle that itself contains the marker (in a
    // string literal, say) still resolves to the real trailing one.
    let at = buf.windows(TAIL_MARKER.len()).rposition(|w| w == TAIL_MARKER)?;
    let js = &buf[at + TAIL_MARKER.len()..];
    // A linker may pad the file; trailing NULs are not part of the bundle.
    let end = js.iter().rposition(|b| *b != 0).map_or(0, |i| i + 1);
    let js = &js[..end];
    if js.is_empty() {
        return None;
    }
    match std::str::from_utf8(js) {
        Ok(s) => Some(s.to_string()),
        Err(_) => {
            crate::log_line("[qjs] appended bundle is not valid UTF-8 — ignored");
            None
        }
    }
}

fn load_bundle() -> String {
    // 0) Bundle appended to this executable by `gpui-ts build` — the shape the
    //    npm package ships (prebuilt host + user JS, no Rust toolchain needed).
    if let Some(js) = bundle_from_self() {
        return js;
    }
    // 1) explicit override via env (handy for live-reload during dev)
    if let Ok(p) = std::env::var("QUICKJS_BUNDLE") {
        if let Ok(s) = std::fs::read_to_string(&p) {
            if !s.is_empty() {
                return s;
            }
        }
    }
    // 2) bundle compiled-in at build time → truly self-contained exe (no sidecar
    //    file, no env-var fragility). Path is relative to THIS source file.
    //    Regenerate via `node build-qjs.mjs` BEFORE `cargo build` when the TS
    //    frontend changes. (A `cargo:rustc-env` bake of a multi-line bundle
    //    mangles newlines under Windows, so include_str! is the reliable path.)
    const EMBEDDED: &str = include_str!("../../ui/dist/main.js");
    if !EMBEDDED.is_empty() {
        return EMBEDDED.to_string();
    }
    crate::log_line(
        "[qjs] embedded bundle empty — run `node build-qjs.mjs` then rebuild the host",
    );
    String::new()
}

/// Where `process.stdout.write` goes: the diagnostic stderr, prefixed, so app
/// prints can never be mistaken for (or corrupt) protocol traffic. stdout used
/// to be the ops channel; it is now a plain output sink.
fn stdout_sink(stderr_tee: usize, s: &str) {
    for part in s.split('\n') {
        if !part.is_empty() {
            trace(stderr_tee, &format!("[stdout] {part}"));
        }
    }
}

fn trace(h: usize, s: &str) {
    if h == 0 {
        return;
    }
    let mut f = unsafe { File::from_raw_handle(h as RawHandle) };
    let _ = f.write_all(s.as_bytes());
    let _ = f.write_all(b"\n");
    let _ = f.flush();
    std::mem::forget(f);
}
