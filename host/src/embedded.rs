//! Embedded frontend: Perry-compiled TS staticlib linked into this process.
//!
//! Replaces the child-process stdio transport with in-process anonymous pipes:
//!
//!   1. Create 3 anonymous pipes (in / out / err)
//!   2. SetStdHandle BEFORE any std usage — Perry's console.log/process.stdin
//!      resolve handles lazily at first use
//!   3. Pre-write one JSONL probe line: Perry's stdin subsystem does a
//!      synchronous read during init and blocks forever on an empty pipe
//!   4. perry_module_init() — runs TS top-level, init ops (JSONL) land in out pipe
//!   5. Host drives the event loop: perry_poll() / perry_has_work() /
//!      perry_next_wake_ms() (upstream #1088 FFI facade)
//!
//! The JSONL mutation protocol is unchanged — the tree.rs parser and the
//! frontend code are transport-agnostic.
//!
//! cfg map:
//!   * `has_embedded_frontend` — set by build.rs whenever the embedded code
//!     path must compile; keeps the pipe plumbing + save_original_stderr.
//!   * `embedded` — set only when the perry archives actually link; gates the
//!     Perry C ABI declarations and `launch()`.

use std::fs::File;
use std::io::{Read, Write};
use std::os::windows::io::{FromRawHandle, RawHandle};

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

fn make_pipe() -> (Handle, Handle) {
    let mut r: Handle = std::ptr::null_mut();
    let mut w: Handle = std::ptr::null_mut();
    let ok = unsafe { CreatePipe(&mut r, &mut w, std::ptr::null(), 0) };
    assert!(ok != 0 && !r.is_null() && !w.is_null(), "CreatePipe failed");
    (r, w)
}

/// Save the original stderr BEFORE redirection so host diagnostics survive.
/// Available in both modes (plain handle read, no perry dependency).
pub fn save_original_stderr() -> Handle {
    unsafe { GetStdHandle(STD_ERROR_HANDLE) }
}

// ---------------------------------------------------------------------------
// Perry C ABI (#1088 staticlib contract) — only linkable in embedded mode
// ---------------------------------------------------------------------------
#[cfg(embedded)]
mod perry {
    unsafe extern "C" {
        pub fn perry_module_init();
        pub fn perry_poll() -> i32;
        pub fn perry_has_work() -> i32;
        pub fn perry_next_wake_ms() -> f64;
        /// The stdlib pump body (async_bridge: promise resolutions + readline
        /// / fs / ws / http event dispatch). In child-process mode perry's own
        /// generated event loop calls it every tick; in embedded mode the
        /// STDLIB_PUMP_FN indirect register/read chain is unreliable across
        /// the /FORCE:MULTIPLE symbol-dedup boundary, so the host calls the
        /// exported symbol directly. Node-API parity: one poll = microtasks +
        /// timers (perry_poll) + one stdlib pump tick.
        pub fn js_stdlib_process_pending() -> i32;
    }
}
#[cfg(embedded)]
// Re-exported for the host loop; which of these are actually called depends on
// the build mode (the QuickJS path doesn't drive Perry's poll loop at all).
#[allow(unused_imports)]
pub use perry::{perry_has_work, perry_module_init, perry_next_wake_ms, perry_poll};
#[cfg(embedded)]
pub use perry::js_stdlib_process_pending;

/// Host → Perry event line writer (in pipe write end).
pub struct EmbeddedFrontend {
    stdin: RawHandle,
}
// Windows pipe handles have no thread affinity; the writer thread owns it.
unsafe impl Send for EmbeddedFrontend {}

impl std::io::Write for EmbeddedFrontend {
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
impl EmbeddedFrontend {
    pub fn write_line(&self, line: &str) -> std::io::Result<()> {
        let mut f = unsafe { File::from_raw_handle(self.stdin) };
        let r = f.write_all(line.as_bytes()).and_then(|_| f.write_all(b"\n"));
        let flush = f.flush();
        std::mem::forget(f); // keep the pipe open across calls
        r.and(flush)
    }
}

/// Redirect stdio into pipes, initialize the Perry module, return:
///   * the event writer (host → frontend)
///   * the ops reader (frontend → host, JSONL lines)
///
/// `frontend_stderr_tee`: handle that receives a live copy of everything the
/// frontend writes to its stderr (PERRY_UI_TRACE diagnostics etc.). Pass the
/// ORIGINAL stderr handle saved before redirection — makes embedded mode as
/// observable as child-process mode.
#[cfg(embedded)]
pub fn launch(frontend_stderr_tee: RawHandle) -> (EmbeddedFrontend, File) {
    let (out_r, out_w) = make_pipe();
    let (in_r, in_w) = make_pipe();
    let (err_r, err_w) = make_pipe();

    unsafe {
        assert!(SetStdHandle(STD_OUTPUT_HANDLE, out_w) != 0);
        assert!(SetStdHandle(STD_ERROR_HANDLE, err_w) != 0);
        assert!(SetStdHandle(STD_INPUT_HANDLE, in_r) != 0);
    }
    // out_w / err_w / in_r stay open (owned by the "frontend side" now);
    // we intentionally leak the raw handles — closing them would kill the
    // frontend's ends.

    // Pre-write a probe line BEFORE init: Perry's stdin subsystem performs a
    // synchronous read during initialization and blocks on an empty pipe.
    // {"t":"ping"} has no registered handler on the frontend side — a no-op.
    {
        let mut w = unsafe { File::from_raw_handle(in_w) };
        let _ = w.write_all(b"{\"t\":\"ping\"}\n");
        let _ = w.flush();
        std::mem::forget(w);
    }

    // Drain frontend stderr and tee it to the original stderr handle
    // (keeps diagnostics away from the ops pipe, but visible in the host log).
    {
        let err_file = unsafe { File::from_raw_handle(err_r) };
        // Capture as usize: raw handles are !Send, usize is Send, and the
        // 2024 edition's disjoint capture would otherwise grab the raw
        // `*mut c_void` field right out of any wrapper struct.
        let tee_addr = frontend_stderr_tee as usize;
        std::thread::spawn(move || {
            let mut f = err_file;
            let mut buf = [0u8; 4096];
            loop {
                match f.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let h = tee_addr as RawHandle;
                        if !h.is_null() {
                            let mut t = unsafe { File::from_raw_handle(h) };
                            let _ = t.write_all(&buf[..n]);
                            let _ = t.flush();
                            std::mem::forget(t); // raw handle must stay open
                        }
                    }
                }
            }
        });
    }

    unsafe { perry_module_init() };

    (EmbeddedFrontend { stdin: in_w }, unsafe { File::from_raw_handle(out_r) })
}
