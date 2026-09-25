//! scriptc frontend — two link shapes behind one contract.
//!
//! * **static (preferred, `cfg(scriptc_static)`)** — the vendored MSVC runtime
//!   (`vendor/scriptc-runtime`) plus the program TU `ui/build-scriptc.mjs` emits
//!   (`build/scriptc/sc-main.lib.c`, profile `emission:"c"`) are compiled
//!   straight into this binary by `build.rs` via the `cc` crate. No DLL, no
//!   runtime `LoadLibrary`, and the runtime shares rustc's UCRT instance — the
//!   same embedding shape as the QuickJS backend. The five (six) entry points
//!   are ordinary `extern "C"` symbols resolved by the linker.
//! * **DLL (legacy, `cfg(scriptc_dll)`)** — `build/scriptc_fe.dll`, a
//!   mingw-CRT scriptc library-mode artifact, resolved at runtime with
//!   `LoadLibraryExA`. Kept as a fallback for a zig-linked build.
//!
//! Either way the engine exports the same ABI and follows the same protocol:
//! `gpts_init` runs the module top level (the whole tree build), `gpts_tick`
//! advances the runtime, `gpts_poll` drains one JSONL mutation line at a time,
//! and `gpts_event` injects host→engine events. `tree.rs` is
//! transport-agnostic — it only ever sees the lines.
//!
//! The host-pull loop (one dedicated driver thread, fixed 10 ms quantum):
//!
//!   * wake every TICK: drain inbound events (`gpts_event`), `gpts_tick()`,
//!     then drain `gpts_poll()` until it returns an empty line, pushing each
//!     line through the same `ingest_line` path as the other backends;
//!   * the engine is single-threaded by contract (library mode localizes its
//!     runtime per instance), so EVERY call into it happens on this thread;
//!   * the panic sink routes engine traps to the host log instead of aborting
//!     the process silently.
//!
//! String ownership across the boundary: `gpts_poll` hands out a borrowed
//! pointer valid until the NEXT engine call (the scriptc borrow model). We copy
//! to a Rust `String` immediately and never free through the boundary.
//! Host→engine strings (`gpts_event`) are likewise borrowed for the call only.

use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use crate::tree::Op;

// --- the engine ABI, as plain C symbols in the static build ---
#[cfg(scriptc_static)]
unsafe extern "C" {
    fn gpts_init();
    fn gpts_tick();
    fn gpts_poll(out: *mut *const u8, out_len: *mut usize);
    fn gpts_event(ptr: *const u8, len: usize);
    fn gpts_set_panic_sink(
        f: unsafe extern "C" fn(*mut c_void, *const u8, usize, u64),
        ctx: *mut c_void,
    );
}

// --- minimal Win32 surface for the legacy DLL path (no external deps) ---
#[cfg(scriptc_dll)]
mod win32 {
    use std::ffi::c_void;
    pub type HModule = *mut c_void;
    pub const LOAD_WITH_ALTERED_SEARCH_PATH: u32 = 0x0000_0008;
    unsafe extern "system" {
        pub fn LoadLibraryExA(name: *const u8, file: *const c_void, flags: u32) -> HModule;
        pub fn GetProcAddress(module: HModule, name: *const u8) -> *const c_void;
    }
    pub fn proc_addr(module: HModule, name: &str) -> *const c_void {
        let mut name_buf = name.as_bytes().to_vec();
        name_buf.push(0);
        let p = unsafe { GetProcAddress(module, name_buf.as_ptr()) };
        if p.is_null() {
            panic!("scriptc_fe.dll missing export `{name}` — rebuild with ui/build-scriptc.mjs");
        }
        p
    }
}

// --- the engine entries, as fn pointers so both shapes share one driver ---
type InitFn = unsafe extern "C" fn();
type TickFn = unsafe extern "C" fn();
type PollFn = unsafe extern "C" fn(*mut *const u8, *mut usize);
type EventFn = unsafe extern "C" fn(*const u8, usize);
#[cfg(scriptc_dll)]
type PanicSinkFn = unsafe extern "C" fn(*mut c_void, *const u8, usize, u64);

struct ScriptcApi {
    init: InitFn,
    tick: TickFn,
    poll: PollFn,
    event: EventFn,
}

/// Resolve the engine entries according to the link shape chosen by build.rs,
/// registering the panic sink as it goes (each shape knows its own module).
#[cfg(scriptc_static)]
fn resolve_api() -> ScriptcApi {
    // Static: the symbols are already in this binary.
    unsafe { gpts_set_panic_sink(panic_sink, std::ptr::null_mut()) };
    ScriptcApi {
        init: gpts_init,
        tick: gpts_tick,
        poll: gpts_poll,
        event: gpts_event,
    }
}

#[cfg(scriptc_dll)]
fn resolve_api() -> ScriptcApi {
    // Resolve the DLL next to the exe first (single-file story), then CWD
    // (dev runs from repo root).
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Some(dir) = exe_dir {
        candidates.push(dir.join("scriptc_fe.dll"));
    }
    candidates.push("build/scriptc_fe.dll".into());

    let dll_path = candidates
        .iter()
        .find(|p| p.exists())
        .unwrap_or_else(|| {
            panic!(
                "scriptc_fe.dll not found (looked in {:?}) — build it with ui/build-scriptc.mjs",
                candidates
            )
        })
        .clone();

    let mut name = dll_path.as_os_str().as_encoded_bytes().to_vec();
    name.push(0);
    let module = unsafe {
        win32::LoadLibraryExA(name.as_ptr(), std::ptr::null(), win32::LOAD_WITH_ALTERED_SEARCH_PATH)
    };
    assert!(!module.is_null(), "LoadLibraryExA failed for {dll_path:?}");

    // We never FreeLibrary: the app lives as long as the host, so the module
    // handle is intentionally dropped here (the DLL stays loaded).
    let register: unsafe extern "C" fn(PanicSinkFn, *mut c_void) =
        unsafe { std::mem::transmute(win32::proc_addr(module, "gpts_set_panic_sink")) };
    unsafe { register(panic_sink, std::ptr::null_mut()) };

    ScriptcApi {
        init: unsafe { std::mem::transmute(win32::proc_addr(module, "gpts_init")) },
        tick: unsafe { std::mem::transmute(win32::proc_addr(module, "gpts_tick")) },
        poll: unsafe { std::mem::transmute(win32::proc_addr(module, "gpts_poll")) },
        event: unsafe { std::mem::transmute(win32::proc_addr(module, "gpts_event")) },
    }
}

/// Route engine traps to the host log. The sink runs on the engine thread and
/// must not re-enter the engine.
unsafe extern "C" fn panic_sink(_ctx: *mut c_void, msg: *const u8, len: usize, code: u64) {
    let text = if msg.is_null() {
        ""
    } else {
        let bytes = unsafe { std::slice::from_raw_parts(msg, len) };
        std::str::from_utf8(bytes).unwrap_or("<non-utf8 trap>")
    };
    crate::log_line(&format!("[scriptc] ENGINE TRAP code={code:#x}: {text}"));
}

/// Everything the driver thread owns. Only THIS thread calls into the engine.
struct Engine {
    api: ScriptcApi,
    queue: Arc<Mutex<Vec<String>>>,
    ops_tx: async_channel::Sender<Vec<Op>>,
}

impl Engine {
    /// Copy one polled line into the ops pipeline (the borrow is valid only
    /// until the next engine call — copy before anything else).
    fn drain_poll(&self) {
        let mut ptr: *const u8 = std::ptr::null();
        let mut len: usize = 0;
        unsafe { (self.api.poll)(&mut ptr, &mut len) };
        if len == 0 || ptr.is_null() {
            return;
        }
        let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
        if let Ok(line) = std::str::from_utf8(bytes) {
            let owned = line.to_string();
            crate::ingest_line(&owned, &self.ops_tx);
        }
    }

    fn drain_events(&self) {
        let pending: Vec<String> = std::mem::take(&mut self.queue.lock().unwrap());
        for ev in pending {
            unsafe { (self.api.event)(ev.as_ptr(), ev.len()) };
        }
    }
}

/// Handed back to `main`'s event loop for host→engine event injection.
pub struct ScriptcHost {
    /// Callable from any thread; drained on the driver thread.
    pub sink: EventSink,
}

#[derive(Clone)]
pub struct EventSink {
    queue: Arc<Mutex<Vec<String>>>,
}

impl EventSink {
    pub fn push(&self, line: String) {
        self.queue.lock().unwrap().push(line);
    }
}

/// Resolve the engine (which also registers the panic sink), run module init
/// (the tree build — ops buffer fills here), and spawn the fixed-quantum driver
/// thread that pushes polled lines through `ingest_line` onto `ops_tx`.
pub fn launch(ops_tx: async_channel::Sender<Vec<Op>>) -> ScriptcHost {
    let api = resolve_api();

    // Module top level == the tree build; ops buffer fills here.
    unsafe { (api.init)() };

    let queue: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let engine = Engine {
        api,
        queue: queue.clone(),
        ops_tx,
    };
    std::thread::Builder::new()
        .name("scriptc-driver".into())
        .spawn(move || driver_loop(engine))
        .expect("spawn scriptc driver");

    ScriptcHost {
        sink: EventSink { queue },
    }
}

/// Fixed-quantum driver: tick → drain events in → drain ops out.
/// The 10 ms quantum matches the other backends' heartbeats; `ingest_line`
/// fans out into the same ops channel `spawn_ops_apply` consumes.
fn driver_loop(engine: Engine) {
    // Emit whatever the module init built, before the first tick.
    loop {
        let mut ptr: *const u8 = std::ptr::null();
        let mut len: usize = 0;
        unsafe { (engine.api.poll)(&mut ptr, &mut len) };
        if len == 0 || ptr.is_null() {
            break;
        }
        let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
        if let Ok(line) = std::str::from_utf8(bytes) {
            let owned = line.to_string();
            crate::ingest_line(&owned, &engine.ops_tx);
        }
    }
    loop {
        std::thread::sleep(std::time::Duration::from_millis(10));
        engine.drain_events();
        unsafe { (engine.api.tick)() };
        engine.drain_poll();
    }
}
