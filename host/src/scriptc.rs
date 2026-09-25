//! scriptc DLL frontend — LoadLibrary + FFI over `build/scriptc_fe.dll`.
//!
//! The DLL is a scriptc (Vercel Labs TS→native AOT) library-mode artifact:
//! the demo UI compiled to a gnu-flavored COFF shared library exporting five
//! C symbols (`gpts_init` / `gpts_tick` / `gpts_poll` / `gpts_event` /
//! `gpts_set_panic_sink` + `gpts_reset`). Build it with
//! `ui/build-scriptc.mjs` (needs zig — scriptc itself links through it).
//!
//! Same JSONL mutation protocol as every other backend — `tree.rs` is
//! transport-agnostic. The host-pull loop:
//!
//!   * driver thread wakes every TICK (10 ms): `gpts_tick()`, then drains
//!     `gpts_poll()` until it returns an empty line, pushing each line
//!     through the same `ingest_line` path as the other backends;
//!   * events (`gpts_event`) are queued from any thread and drained on the
//!     driver thread — the DLL is single-threaded by contract (library mode
//!     localizes its runtime per instance);
//!   * `gpts_set_panic_sink` routes engine traps to the host log instead of
//!     aborting the process silently.
//!
//! Unlike `embedded.rs` there is no stdio redirection at all: the FFI IS the
//! transport. Unlike `quickjs.rs` the engine has no self-driven thread and no
//! unpark — cadence is the host's fixed 10 ms quantum, the same trade the
//! Perry embedded mode makes.
//!
//! String ownership across the FFI boundary: `gpts_poll` hands out a borrowed
//! pointer into the engine's result arena, valid until the NEXT library call
//! (per the scriptc borrow model). We copy to a Rust String immediately and
//! never free through the DLL boundary. Host→engine strings (`gpts_event`)
//! are likewise borrowed for the call duration only.

use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use crate::tree::Op;

// --- minimal Win32 surface (no external deps) ---
type HModule = *mut c_void;

const LOAD_WITH_ALTERED_SEARCH_PATH: u32 = 0x0000_0008;

unsafe extern "system" {
    fn LoadLibraryExA(name: *const u8, file: *const c_void, flags: u32) -> HModule;
    fn GetProcAddress(module: HModule, name: *const u8) -> *const c_void;
}

fn proc_addr(module: HModule, name: &str) -> *const c_void {
    let mut name_buf = name.as_bytes().to_vec();
    name_buf.push(0); // NUL terminator
    let p = unsafe { GetProcAddress(module, name_buf.as_ptr()) };
    if p.is_null() {
        panic!("scriptc_fe.dll missing export `{name}` — rebuild with ui/build-scriptc.mjs");
    }
    p
}

// --- the five (six) exports, as C ABI fn pointers ---

type InitFn = unsafe extern "C" fn();
type TickFn = unsafe extern "C" fn();
type PollFn = unsafe extern "C" fn(*mut *const u8, *mut usize);
type EventFn = unsafe extern "C" fn(*const u8, usize);
#[allow(dead_code)] // set via GetProcAddress below, not via this alias (yet)
type PanicSinkFn = unsafe extern "C" fn(*mut c_void, *const u8, usize, u64);

struct ScriptcApi {
    init: InitFn,
    tick: TickFn,
    poll: PollFn,
    event: EventFn,
}

/// Everything the driver thread owns. The DLL is single-threaded: every call
/// into it happens on this thread.
///
/// SAFETY: `module` is a raw HMODULE, which makes `Engine` `!Send` even though
/// the handle itself carries no thread affinity — `LoadLibrary`/`FreeLibrary`
/// work from any thread, and we never unload. All OTHER fields (fn pointers,
/// queues, channel senders) are `Send`; the blanket `unsafe impl Send` below
/// documents the invariant that only THIS driver thread calls into the DLL.
struct Engine {
    api: ScriptcApi,
    queue: Arc<Mutex<Vec<String>>>,
    ops_tx: async_channel::Sender<Vec<Op>>,
    #[allow(dead_code)]
    module: HModule, // never unloaded — the app lives as long as the host
}
unsafe impl Send for Engine {}

impl Engine {
    /// Copy one polled line into the ops pipeline (borrow valid until the
    /// next engine call — copy before anything else).
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

/// Load `build/scriptc_fe.dll` relative to the host exe (fall back to CWD),
/// resolve the exports, register the panic sink, run module init, and hand
/// back the event sink for `main`'s event loop.
///
/// The driver thread spawned inside `launch` pushes polled lines straight
/// through `ingest_line` onto the ops channel `main` owns — the same funnel
/// the pipe reader (`run_ops_reader`) uses for the other backends, minus the
/// pipe.
pub struct ScriptcHost {
    /// Inbound event injection (host → engine). Callable from any thread.
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

pub fn launch(ops_tx: async_channel::Sender<Vec<Op>>) -> ScriptcHost {
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
        LoadLibraryExA(name.as_ptr(), std::ptr::null(), LOAD_WITH_ALTERED_SEARCH_PATH)
    };
    assert!(!module.is_null(), "LoadLibraryExA failed for {dll_path:?}");

    let api = ScriptcApi {
        init: unsafe { std::mem::transmute(proc_addr(module, "gpts_init")) },
        tick: unsafe { std::mem::transmute(proc_addr(module, "gpts_tick")) },
        poll: unsafe { std::mem::transmute(proc_addr(module, "gpts_poll")) },
        event: unsafe { std::mem::transmute(proc_addr(module, "gpts_event")) },
    };

    // Panic sink: engine traps land in the host log, not a silent abort.
    // The sink runs on the engine thread; it must not re-enter the engine.
    unsafe extern "C" fn panic_sink(_ctx: *mut c_void, msg: *const u8, len: usize, code: u64) {
        let text = if msg.is_null() { "" } else {
            let bytes = unsafe { std::slice::from_raw_parts(msg, len) };
            std::str::from_utf8(bytes).unwrap_or("<non-utf8 trap>")
        };
        crate::log_line(&format!("[scriptc] ENGINE TRAP code={code:#x}: {text}"));
    }
    let register: unsafe extern "C" fn(
        unsafe extern "C" fn(*mut c_void, *const u8, usize, u64),
        *mut c_void,
    ) = unsafe { std::mem::transmute(proc_addr(module, "gpts_set_panic_sink")) };
    unsafe { register(panic_sink, std::ptr::null_mut()) };

    // Module top level == the tree build; ops buffer fills here.
    unsafe { (api.init)() };

    let queue: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    // Driver thread: the ONLY thread that touches the DLL.
    let engine = Engine {
        api,
        queue: queue.clone(),
        ops_tx,
        module,
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
