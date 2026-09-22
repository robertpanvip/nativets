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
//!   * because the engine is *in process*, we can inject the transport as host
//!     functions instead of faking a stdio pipe (see `Transport`)
//!
//! Threading: a `QuickJS Context` is `!Send`, so the engine runs on its own
//! dedicated OS thread. The host communicates with it only through an inbound
//! queue (`Arc<Mutex<Vec<String>>>`) and — depending on `Transport` — an
//! injected `__hostEmit` host function.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use rquickjs::function::Args;
use rquickjs::{Array, Context, Ctx, Function, Object, Runtime};

use crate::tree::Op;

// --- minimal Win32 surface (no external deps) ---
type Handle = *mut core::ffi::c_void;
const STD_INPUT_HANDLE: u32 = 0xFFFF_FFF6;
const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5;
const STD_ERROR_HANDLE: u32 = 0xFFFF_FFF4;

unsafe extern "system" {
    fn CreatePipe(r: *mut Handle, w: *mut Handle, attrs: *const u32, size: u32) -> i32;
    fn GetStdHandle(which: u32) -> Handle;
    fn SetStdHandle(which: u32, h: Handle) -> i32;
}

/// Sentinel pushed to the inbound queue when the host shuts down, so the app's
/// `process.stdin.on("end", ...)` handler fires. It can never collide with a
/// real event line (which always starts with `{`), and is never dispatched to
/// the `data` handler.
const QJS_EOF_MARKER: &str = "\u{0}__qjs_eof__\u{0}";

/// Engine tick cadence. Also the idle wake-up period when parked.
const TICK: Duration = Duration::from_millis(10);

// ---------------------------------------------------------------------------
// Transport: how the host and the engine exchange protocol lines
// ---------------------------------------------------------------------------

/// Both transports carry the *same* JSONL protocol lines; only the carrier
/// differs.
///
/// `Pipe` — the historical path, and the only one possible for an
/// **out-of-process** frontend (child mode, Perry child). stdio is redirected
/// into anonymous pipes (`SetStdHandle`), a reader thread parses lines off the
/// out pipe, and a writer thread pushes events into the in pipe where a second
/// reader thread turns them back into lines for the engine to drain.
///
/// `Direct` — only possible *because* the engine is in-process, and the reason
/// it is the default: the TS side gets a `__hostEmit` host function, so a batch
/// goes straight from JS into the host's ops channel (no pipe, no reader
/// thread, no line round-trip), and events are pushed straight into the
/// engine's inbound queue plus an `unpark` — so dispatch happens on the spot
/// instead of on the next tick.
///
/// The frontend picks its carrier at runtime (`typeof __hostEmit`), so a single
/// compiled bundle works with both — switching transports needs no rebuild.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Transport {
    Direct,
    Pipe,
}

impl Transport {
    /// Parse a user-supplied transport name. Unknown values fall back to the
    /// default (Direct) rather than failing the boot.
    pub fn parse(s: &str) -> Transport {
        match s.trim().to_ascii_lowercase().as_str() {
            "pipe" | "stdio" | "pipes" => Transport::Pipe,
            "direct" | "inject" | "injected" => Transport::Direct,
            other => {
                crate::log_line(&format!(
                    "[qjs] unknown transport `{other}` (expected `direct` or `pipe`); using direct"
                ));
                Transport::Direct
            }
        }
    }

    /// `GPUI_TS_TRANSPORT=pipe|direct` (default `direct`).
    pub fn from_env() -> Transport {
        match std::env::var("GPUI_TS_TRANSPORT") {
            Ok(v) if !v.is_empty() => Transport::parse(&v),
            _ => Transport::Direct,
        }
    }
}

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

/// Everything `main` needs from the QuickJS backend. In `Direct` mode both pipe
/// handles are `None` — the transport never touches stdio.
pub struct QuickJsHost {
    /// Writable end of the frontend's stdin pipe (`Pipe` mode only).
    pub frontend: Option<QuickJsFrontend>,
    /// Readable end of the frontend's stdout pipe (`Pipe` mode only).
    pub out_r: Option<File>,
    /// Always available: the injected event channel.
    pub sink: EventSink,
}

fn make_pipe() -> (Handle, Handle) {
    let mut r: Handle = std::ptr::null_mut();
    let mut w: Handle = std::ptr::null_mut();
    let ok = unsafe { CreatePipe(&mut r, &mut w, std::ptr::null(), 0) };
    assert!(ok != 0 && !r.is_null() && !w.is_null(), "CreatePipe failed");
    (r, w)
}

/// Save the original stderr BEFORE any redirection so host diagnostics survive.
pub fn save_original_stderr() -> Handle {
    unsafe { GetStdHandle(STD_ERROR_HANDLE) }
}

/// Host → frontend event writer over a pipe (`Pipe` mode only).
pub struct QuickJsFrontend {
    stdin: RawHandle,
}
// Windows pipe handles have no thread affinity; the writer thread owns it.
unsafe impl Send for QuickJsFrontend {}

impl std::io::Write for QuickJsFrontend {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut f = unsafe { File::from_raw_handle(self.stdin) };
        let r = f.write(buf);
        std::mem::forget(f); // keep the pipe open across calls
        r
    }
    fn flush(&mut self) -> std::io::Result<()> {
        let mut f = unsafe { File::from_raw_handle(self.stdin) };
        let r = f.flush();
        std::mem::forget(f);
        r
    }
}

impl QuickJsFrontend {
    /// Kept API-parallel with `embedded::EmbeddedFrontend`; the event writer
    /// itself goes through the `Write` impl above, so this is unused today.
    #[allow(dead_code)]
    pub fn write_line(&self, line: &str) -> std::io::Result<()> {
        let mut f = unsafe { File::from_raw_handle(self.stdin) };
        let r = f
            .write_all(line.as_bytes())
            .and_then(|_| f.write_all(b"\n"))
            .and_then(|_| f.flush());
        std::mem::forget(f); // keep the pipe open across calls
        r
    }
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
    /// engine → host, handed to the injected `__hostEmit` (Direct mode).
    ops_tx: async_channel::Sender<Vec<Op>>,
    /// Write end of the frontend's stdout pipe; `None` in Direct mode, where
    /// stdout is no longer a protocol channel.
    out_w: Option<usize>,
    stderr_tee: usize,
    transport: Transport,
}

impl Engine {
    fn new(
        out_w: Option<usize>,
        stderr_tee: usize,
        queue: Arc<Mutex<Vec<String>>>,
        ops_tx: async_channel::Sender<Vec<Op>>,
        transport: Transport,
    ) -> Self {
        let rt = Runtime::new().expect("[qjs] Runtime::new failed");
        let ctx = Context::full(&rt).expect("[qjs] Context::full failed");
        Engine {
            rt,
            ctx,
            queue,
            ops_tx,
            out_w,
            stderr_tee,
            transport,
        }
    }

    /// Boot the engine: register host globals, eval bootstrap + app bundle,
    /// then drive the event loop forever on this thread.
    fn start(&self, bundle: String) {
        let bootstrap = r#"
            globalThis.__stdinCbs = {};
            globalThis.__timers = [];
            globalThis.__tid = 0;
            // timers (driven by the host tick via __hostTick)
            globalThis.__hostTick = function () {
                var now = Date.now();
                var ts = globalThis.__timers;
                for (var i = 0; i < ts.length; i++) {
                    var t = ts[i];
                    if (t.due <= now) {
                        try { t.cb(); } catch (e) {}
                        if (t.interval > 0) { t.due = now + t.interval; }
                        else { ts.splice(i, 1); i--; }
                    }
                }
            };
            globalThis.setInterval = function (cb, ms) {
                var id = ++globalThis.__tid;
                globalThis.__timers.push({ id: id, cb: cb, due: Date.now() + (ms || 0), interval: ms || 0 });
                return id;
            };
            globalThis.setTimeout = function (cb, ms) {
                var id = ++globalThis.__tid;
                globalThis.__timers.push({ id: id, cb: cb, due: Date.now() + (ms || 0), interval: 0 });
                return id;
            };
            globalThis.clearInterval = globalThis.clearTimeout = function (id) {
                var ts = globalThis.__timers;
                for (var i = 0; i < ts.length; i++) if (ts[i].id === id) { ts.splice(i, 1); return; }
            };
            // inbound event registration (callbacks stored in JS, no Rust Persistent)
            process.stdin.setEncoding = function () {};
            process.stdin.on = function (ev, cb) { globalThis.__stdinCbs[ev] = cb; };
            // process.exit really exits (the app calls it from stdin 'end')
            process.exit = function (code) { globalThis.__hostExit(code || 0); };
            // console → original stderr (never the protocol channel)
            globalThis.console = {
                log: function () { globalThis.__hostTrace("log", Array.prototype.map.call(arguments, String).join(" ")); },
                info: function () { globalThis.__hostTrace("info", Array.prototype.map.call(arguments, String).join(" ")); },
                warn: function () { globalThis.__hostTrace("warn", Array.prototype.map.call(arguments, String).join(" ")); },
                error: function () { globalThis.__hostTrace("error", Array.prototype.map.call(arguments, String).join(" ")); },
                debug: function () { globalThis.__hostTrace("debug", Array.prototype.map.call(arguments, String).join(" ")); }
            };
        "#;

        self.ctx.with(|ctx| {
            self.register_globals(ctx.clone());
            if let Err(e) = ctx.eval::<(), _>(bootstrap.to_string()) {
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

        // Event loop. In Direct mode we park instead of sleep, so an inbound
        // event wakes us the moment it arrives (worst case still one tick).
        loop {
            match self.transport {
                Transport::Direct => thread::park_timeout(TICK),
                Transport::Pipe => thread::sleep(TICK),
            }
            self.pump();
        }
    }

    /// Register the host-provided globals the TS frontend relies on. Only the
    /// raw-handle-capturing primitives (__hostWrite/__hostTrace/__hostEmit) and
    /// trivial no-ops are Rust functions; everything that must persist a JS
    /// callback (stdin.on, console) is defined in the bootstrap to avoid
    /// capturing `Ctx` (which would be a borrow escape).
    fn register_globals(&self, ctx: Ctx<'_>) {
        let out_w = self.out_w;
        let stderr_tee = self.stderr_tee;

        // --- host primitives (capture usize handles / a channel; no Ctx) ---
        let host_write = Function::new(ctx.clone(), move |s: String| {
            stdout_sink(out_w, stderr_tee, &s);
        });
        let host_trace = Function::new(ctx.clone(), move |level: String, msg: String| {
            trace(stderr_tee, &format!("[console.{}] {}", level, msg));
        });
        let _ = ctx.globals().set("__hostWrite", host_write);
        let _ = ctx.globals().set("__hostTrace", host_trace);

        // --- injected transport (Direct mode only) ---
        // `__hostEmit(line)` hands one protocol line to the host's ops channel
        // on the calling (engine) thread: no pipe write, no reader thread, no
        // line re-splitting. Ops are parsed here rather than downstream because
        // this thread is idle ~99% of the time anyway.
        if self.transport == Transport::Direct {
            let tx = self.ops_tx.clone();
            let host_emit = Function::new(ctx.clone(), move |line: String| {
                crate::ingest_line(&line, &tx);
            });
            let _ = ctx.globals().set("__hostEmit", host_emit);
        }

        // --- process ---
        let process = Object::new(ctx.clone()).unwrap();
        let stdout = Object::new(ctx.clone()).unwrap();
        let stdout_write = Function::new(ctx.clone(), move |s: String| {
            stdout_sink(out_w, stderr_tee, &s);
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

/// Boot the QuickJS engine on its own thread and return the host-side handles
/// for the chosen transport. Mirrors `embedded::launch` for the `Pipe` case.
pub fn launch(
    frontend_stderr_tee: RawHandle,
    transport: Transport,
    ops_tx: async_channel::Sender<Vec<Op>>,
) -> QuickJsHost {
    let queue: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let bundle = load_bundle();
    let tee = frontend_stderr_tee as usize;

    // --- transport-specific wiring (must happen before the engine thread) ---
    let out_w: Option<usize>;
    let stderr_tee: usize;
    let mut pipe_in_w: Option<RawHandle> = None;
    let mut pipe_out_r: Option<File> = None;

    match transport {
        Transport::Pipe => {
            let (out_r, out_w_h) = make_pipe();
            let (in_r, in_w) = make_pipe();
            let (err_r, err_w) = make_pipe();

            unsafe {
                assert!(SetStdHandle(STD_OUTPUT_HANDLE, out_w_h) != 0);
                assert!(SetStdHandle(STD_ERROR_HANDLE, err_w) != 0);
                assert!(SetStdHandle(STD_INPUT_HANDLE, in_r) != 0);
            }

            // tee frontend stderr → original stderr (keep diagnostics off the
            // ops pipe)
            spawn_stderr_tee(err_r as usize, tee);
            // reader: host → frontend events land in the queue (engine drains)
            spawn_stdin_reader(in_r as usize, queue.clone());

            out_w = Some(out_w_h as usize);
            stderr_tee = tee;
            pipe_in_w = Some(in_w);
            pipe_out_r = Some(unsafe { File::from_raw_handle(out_r) });
        }
        Transport::Direct => {
            // Nothing to redirect: the engine reaches the host through injected
            // functions and the shared queue. stderr is used directly (never
            // replaced), so js console output keeps flowing to the terminal.
            out_w = None;
            stderr_tee = tee;
        }
    }

    let engine_thread = thread::Builder::new()
        .name("quickjs".to_string())
        .spawn({
            let queue = queue.clone();
            move || {
                let engine = Engine::new(out_w, stderr_tee, queue, ops_tx, transport);
                engine.start(bundle);
            }
        })
        .expect("[qjs] failed to spawn engine thread");

    QuickJsHost {
        frontend: pipe_in_w.map(|stdin| QuickJsFrontend { stdin }),
        out_r: pipe_out_r,
        sink: EventSink {
            queue,
            engine: engine_thread.thread().clone(),
        },
    }
}

// ---------------------------------------------------------------------------
// plumbing helpers
// ---------------------------------------------------------------------------

fn spawn_stderr_tee(err_r: usize, tee: usize) {
    std::thread::spawn(move || {
        let mut f = unsafe { File::from_raw_handle(err_r as RawHandle) };
        let mut buf = [0u8; 4096];
        loop {
            match f.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let h = tee as RawHandle;
                    if !h.is_null() {
                        let mut t = unsafe { File::from_raw_handle(h) };
                        let _ = t.write_all(&buf[..n]);
                        let _ = t.flush();
                        std::mem::forget(t);
                    }
                }
            }
        }
    });
}

fn spawn_stdin_reader(in_r: usize, queue: Arc<Mutex<Vec<String>>>) {
    std::thread::spawn(move || {
        let f = unsafe { File::from_raw_handle(in_r as RawHandle) };
        let reader = BufReader::new(f);
        for line in reader.lines() {
            match line {
                Ok(l) => queue.lock().unwrap().push(l),
                Err(_) => break,
            }
        }
        // EOF: signal the app's stdin 'end' handler (it calls process.exit).
        queue.lock().unwrap().push(QJS_EOF_MARKER.to_string());
    });
}

fn load_bundle() -> String {
    // 0) bundle compiled-in at build time → truly self-contained exe (no sidecar
    //    file, no env-var fragility). Path is relative to THIS source file.
    //    Regenerate via `node build-qjs.mjs` BEFORE `cargo build` when the TS
    //    frontend changes. (A `cargo:rustc-env` bake of a multi-line bundle
    //    mangles newlines under Windows, so include_str! is the reliable path.)
    const EMBEDDED: &str = include_str!("../../ui/dist/main.js");
    if !EMBEDDED.is_empty() {
        return EMBEDDED.to_string();
    }
    // 1) explicit override via env (handy for live-reload during dev)
    if let Ok(p) = std::env::var("QUICKJS_BUNDLE") {
        if let Ok(s) = std::fs::read_to_string(&p) {
            if !s.is_empty() {
                return s;
            }
        }
    }
    crate::log_line(
        "[qjs] embedded bundle empty — run `node build-qjs.mjs` then rebuild the host",
    );
    String::new()
}

/// Where `process.stdout.write` goes. In `Pipe` mode it *is* the protocol
/// channel. In `Direct` mode stdout is no longer transport, but app code may
/// still print — route it to the diagnostic stderr (prefixed) so it can never
/// corrupt the ops stream.
fn stdout_sink(out_w: Option<usize>, stderr_tee: usize, s: &str) {
    match out_w {
        Some(h) => write_handle(h, s),
        None => {
            for part in s.split('\n') {
                if !part.is_empty() {
                    trace(stderr_tee, &format!("[stdout] {part}"));
                }
            }
        }
    }
}

fn write_handle(h: usize, s: &str) {
    let mut f = unsafe { File::from_raw_handle(h as RawHandle) };
    let _ = f.write_all(s.as_bytes());
    let _ = f.flush();
    std::mem::forget(f);
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
