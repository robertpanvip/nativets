//! QuickJS-embedded frontend (replaces the Perry runtime layer).
//!
//! Same JSONL mutation protocol as `embedded.rs` — the tree.rs parser and the
//! TS frontend are transport-agnostic, so nothing in the UI host changes.
//!
//! Why QuickJS instead of Perry here:
//!   * we own the event loop: `pump()` drains promise jobs + fires timers +
//!     dispatches queued stdin lines on a 10ms tick (no 500ms I/O quantum,
//!     no indirect stdlib-pump registration chain that silently dropped
//!     stdin dispatch under /FORCE:MULTIPLE)
//!   * no duplicate Rust std in the link (Perry bundles one) → /FORCE:MULTIPLE
//!     and its symbol-splitting hazards disappear
//!   * QuickJS C core is ~1-2MB vs Perry's 5.9MB runtime → smaller exe
//!   * `process` is a host-provided shim (QuickJS has no `process` global) —
//!     this is the only frontend-facing contract we must honor (runtime.ts
//!     already speaks Node-ish: stdout/stdin.on/env/exit, setInterval,
//!     Promise, JSON)
//!
//! Threading: a `QuickJS Context` is `!Send`, so the engine runs on its own
//! dedicated OS thread. The host communicates with it only through the same
//! stdin/stdout pipes used by the Perry path (plus a shared stdin queue).

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use rquickjs::{Array, Context, Ctx, Function, Object, Runtime};
use rquickjs::function::Args;

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

/// Sentinel pushed to the stdin queue when the host closes its write end, so
/// the app's `process.stdin.on("end", ...)` handler fires. It can never collide
/// with a real event line (which always starts with `{`), and is never
/// dispatched to the `data` handler.
const QJS_EOF_MARKER: &str = "\u{0}__qjs_eof__\u{0}";

fn make_pipe() -> (Handle, Handle) {
    let mut r: Handle = std::ptr::null_mut();
    let mut w: Handle = std::ptr::null_mut();
    let ok = unsafe { CreatePipe(&mut r, &mut w, std::ptr::null(), 0) };
    assert!(ok != 0 && !r.is_null() && !w.is_null(), "CreatePipe failed");
    (r, w)
}

/// Save the original stderr BEFORE redirection so host diagnostics survive.
pub fn save_original_stderr() -> Handle {
    unsafe { GetStdHandle(STD_ERROR_HANDLE) }
}

/// Host → frontend event writer (in pipe write end).
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

/// Engine state owned by the dedicated engine thread.
struct Engine {
    /// Owns the JS runtime. Never read directly, but must stay alive for as
    /// long as `ctx` lives — dropping it would invalidate the context.
    #[allow(dead_code)]
    rt: Runtime,
    ctx: Context,
    queue: Arc<Mutex<Vec<String>>>,
    out_w: usize,   // raw handle as usize (Send-safe inside the thread)
    stderr_tee: usize,
}

impl Engine {
    fn new(out_w: usize, stderr_tee: usize, queue: Arc<Mutex<Vec<String>>>) -> Self {
        let rt = Runtime::new().expect("[qjs] Runtime::new failed");
        let ctx = Context::full(&rt).expect("[qjs] Context::full failed");
        Engine {
            rt,
            ctx,
            queue,
            out_w,
            stderr_tee,
        }
    }

    /// Boot the engine: register host globals, eval bootstrap + app bundle,
    /// then drive the event loop forever on this thread.
    fn start(&self, bundle: String) {
        let bootstrap = r#"
            globalThis.__stdinCbs = {};
            globalThis.__timers = [];
            globalThis.__tid = 0;
            // timers (driven by the host 10ms tick via __hostTick)
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
            // stdin event registration (callbacks stored in JS, no Rust Persistent)
            process.stdin.setEncoding = function () {};
            process.stdin.on = function (ev, cb) { globalThis.__stdinCbs[ev] = cb; };
            // process.exit really exits (the app calls it from stdin 'end')
            process.exit = function (code) { globalThis.__hostExit(code || 0); };
            // console → original stderr (never the ops pipe)
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

        // Event loop: 10ms tick (matches the Perry path's heartbeat cadence).
        loop {
            thread::sleep(Duration::from_millis(10));
            self.pump();
        }
    }

    /// Register the host-provided globals the TS frontend relies on. Only the
    /// raw-handle-capturing primitives (__hostWrite/__hostTrace) and trivial
    /// no-ops are Rust functions; everything that must persist a JS callback
    /// (stdin.on, console) is defined in the bootstrap to avoid capturing Ctx.
    fn register_globals(&self, ctx: Ctx<'_>) {
        let out_w = self.out_w;
        let stderr_tee = self.stderr_tee;

        // --- host primitives (capture usize handles only; no Ctx) ---
        let host_write = Function::new(ctx.clone(), move |s: String| {
            write_handle(out_w, &s);
        });
        let host_trace = Function::new(ctx.clone(), move |level: String, msg: String| {
            trace(stderr_tee, &format!("[console.{}] {}", level, msg));
        });
        let _ = ctx.globals().set("__hostWrite", host_write);
        let _ = ctx.globals().set("__hostTrace", host_trace);

        // --- process ---
        let process = Object::new(ctx.clone()).unwrap();
        let stdout = Object::new(ctx.clone()).unwrap();
        let stdout_write = Function::new(ctx.clone(), move |s: String| {
            write_handle(out_w, &s);
        });
        let _ = stdout.set("write", stdout_write);
        let _ = process.set("stdout", stdout);

        // stderr → original stderr tee (never the ops pipe; trace must stay
        // off the JSONL protocol stream). `stderr_tee` is Copy (usize).
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

    /// One tick: drain promise jobs, fire due timers, dispatch stdin lines.
    fn pump(&self) {
        self.ctx.with(|ctx| {
            // 1) drain native promise jobs (microtasks)
            while ctx.execute_pending_job() {}

            // 2) fire due timers (JS __hostTick)
            if let Ok(f) = ctx.globals().get::<_, Function>("__hostTick") {
                let _ = f.call::<(), ()>(());
            }

            // 3) dispatch queued stdin lines to the 'data' handler; an EOF
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
        });
    }
}

/// Redirect stdio into pipes, boot the QuickJS engine on its own thread,
/// return (event writer, ops reader) — identical shape to embedded::launch.
pub fn launch(frontend_stderr_tee: RawHandle) -> (QuickJsFrontend, File) {
    let (out_r, out_w) = make_pipe();
    let (in_r, in_w) = make_pipe();
    let (err_r, err_w) = make_pipe();

    unsafe {
        assert!(SetStdHandle(STD_OUTPUT_HANDLE, out_w) != 0);
        assert!(SetStdHandle(STD_ERROR_HANDLE, err_w) != 0);
        assert!(SetStdHandle(STD_INPUT_HANDLE, in_r) != 0);
    }

    let queue: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    // tee frontend stderr → original stderr (keep diagnostics off the ops pipe)
    spawn_stderr_tee(err_r as usize, frontend_stderr_tee as usize);
    // reader: host → frontend events land in the queue (engine drains it)
    spawn_stdin_reader(in_r as usize, queue.clone());

    let out_w_u = out_w as usize;
    let stderr_tee_u = frontend_stderr_tee as usize;
    let bundle = load_bundle();
    thread::spawn(move || {
        let engine = Engine::new(out_w_u, stderr_tee_u, queue);
        engine.start(bundle);
    });

    (QuickJsFrontend { stdin: in_w }, unsafe { File::from_raw_handle(out_r) })
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
