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
//! the bundle on a dedicated engine thread. Because the engine is in-process,
//! the protocol does not go through stdio at all: the host injects
//! `__hostEmit` into the JS context for outbound ops and pushes events
//! straight into the engine queue. Both directions funnel through
//! `ingest_line` below, so they cannot drift apart from the child/embedded
//! paths.

mod draw;
mod tree;
mod style;
mod canvas_draw;
mod widget_state;
mod native_widgets;
mod render;

/// Host side of the BOM surface (window metrics, perf clock, entropy,
/// dialogs). Only the QuickJS frontend consumes it — it backs `bootstrap.js`,
/// and Perry/node frontends bring their own JS environment. So leaving these
/// unused in a Perry build is expected, not a defect.
#[cfg_attr(not(quickjs), allow(dead_code))]
mod bom;

#[cfg(has_embedded_frontend)]
// Compiled in both embedded and quickjs modes; in quickjs mode the Perry
// poll-loop helpers are simply unused, which is expected (not a defect).
#[cfg_attr(quickjs, allow(dead_code, unused_imports))]
mod embedded;

#[cfg(quickjs)]
mod quickjs;

/// scriptc AOT backend (cfg `scriptc`): either a statically linked MSVC
/// runtime + program TU (cfg `scriptc_static`, built by `ui/build-scriptc.mjs`
/// — no DLL, shares the rustc UCRT), or the legacy mingw DLL loaded with
/// LoadLibrary (cfg `scriptc_dll`).
#[cfg(scriptc)]
mod scriptc;

use draw::Cmd as DrawCmd;
use tree::{is_native_tag, Node, Op, Tree};

use gpui::{
    canvas, deferred, div, fill, point, prelude::*, px, relative, rgb, rgba, size, AnyElement, App,
    BorderStyle, Bounds, ClickEvent, Context, Corners, Edges, ElementId, Entity, Focusable,
    FocusHandle, Font, FontWeight, InteractiveElement, ParentElement, PathBuilder,
    Render, ScrollHandle, ScrollWheelEvent, SharedString, StatefulInteractiveElement, Styled,
    TextAlign, TextRun, Window, WindowBounds, WindowOptions, TitlebarOptions,
};
use chrono::NaiveDate;
use gpui_base::Date as GpuiDate;
use gpui_component::date_picker::{DatePicker as GpuiDatePicker, DatePickerEvent, DatePickerState};
use gpui_component::slider::{Slider as GpuiSlider, SliderEvent, SliderState};
// The real dropdown select. `SelectState<D>` is generic over its delegate, so
// the host pins one: plain text options, whose item *value* is the display
// string itself — exactly what the protocol carries.
use gpui_component::select::{Select as GpuiSelect, SelectEvent, SelectState};
use gpui_component::IndexPath;
// Extended bridge: more gpui-component widgets wired through the retained tree.
use gpui_component::input::{Textarea, TextareaState};
use gpui_component::combobox::{Combobox, ComboboxEvent, ComboboxState};
use gpui_component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_component::radio::{Radio, RadioGroup};
use gpui_component::tab::{Tab, TabBar};
use gpui_component::pagination::Pagination;
use gpui_component::breadcrumb::{Breadcrumb, BreadcrumbItem};
use gpui_component::alert::Alert;
use gpui_component::badge::Badge;
use gpui_component::tag::Tag;
use gpui_component::avatar::Avatar;
use gpui_component::separator::Separator;
use gpui_component::skeleton::Skeleton;
use gpui_component::label::Label;
use gpui_component::link::Link;
use gpui_component::collapsible::Collapsible;

/// Delegate backing a host `select` node: the option list as text. `Vec<T>`
/// already implements `SearchableListDelegate`, and `SharedString` an item, so
/// no newtype is needed.
type SelectDelegate = Vec<SharedString>;
/// The concrete state entity stored per `select` node.
type HostSelectState = SelectState<SelectDelegate>;
/// Concrete combobox state: plain text options, value = display string (same
/// delegate shape as `HostSelectState` — `Vec<SharedString>` already implements
/// `SearchableListDelegate`, so no newtype is needed).
type HostComboboxState = ComboboxState<Vec<SharedString>>;
use gpui_platform::application;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
#[cfg(not(any(embedded, quickjs)))]
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, OnceLock};

// ---------------------------------------------------------------------------
// Host logging: writes to the ORIGINAL stderr handle. In embedded mode the
// process std handles are redirected into the frontend pipes, so plain
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

pub(crate) fn log_line(s: &str) {
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
    ($($arg:tt)*) => { crate::log_line(&format!($($arg)*)) };
}

/// `GPUI_TS_LOG_OPS=1` — log every inbound batch's op count.
///
/// Off by default: the frontend polls stats twice a second, so batches are a
/// steady 2/s trickle and the counts only matter when the rendered UI is
/// *missing something*. Comparing this count against the frontend's own
/// `opsSent` is what separates "the frontend never emitted it" from "the host
/// dropped it" — the two failures look identical on screen.
fn log_ops_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| matches!(std::env::var("GPUI_TS_LOG_OPS"), Ok(v) if v != "0"))
}

/// Cumulative inbound ops, reported alongside each batch when the above is on.
static OPS_SEEN: AtomicUsize = AtomicUsize::new(0);

/// `GPUI_TS_DUMP_TREE=1` — dump the retained tree after any substantial batch.
///
/// Only large batches (the mount) are dumped: the steady state is a 1–3 op
/// stats tick twice a second and repeating the tree for those is pure noise.
fn dump_tree_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| matches!(std::env::var("GPUI_TS_DUMP_TREE"), Ok(v) if v != "0"))
}

/// Epoch milliseconds, aligned with the frontend's `Date.now()` so host and
/// frontend trace lines can be paired directly to measure interaction latency.
pub(crate) fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

// Re-export the host log macro for submodules.
pub(crate) use log;
// --- style / draw / widget / native-widget / render code moved to submodules above ---
/// Initial window geometry. Single source of truth: `WindowMetrics` is seeded
/// from these so the engine (which boots *before* the window exists) already
/// reports the right `innerWidth`/`innerHeight`, and the first render replaces
/// them with the real post-decoration values.
pub(crate) const WINDOW_W: f32 = 1180.0;
pub(crate) const WINDOW_H: f32 = 760.0;
pub(crate) const MIN_WINDOW_W: f32 = 700.0;
pub(crate) const MIN_WINDOW_H: f32 = 480.0;

/// Default width of a text field, in the spirit of a browser's `input`
/// (whose intrinsic width is about 20 characters ≈ 173px). A canvas-backed
/// field cannot size itself from content, so *something* has to be definite —
/// a bare `<input>` in CSS does the same thing, just with a different number.
pub(crate) const DEFAULT_INPUT_W: f32 = 220.0;
pub(crate) const DEFAULT_FONT_SIZE: f32 = 13.0;
/// Default width of a native `slider` when the protocol gives no `width`.
pub(crate) const DEFAULT_SLIDER_W: f32 = 220.0;
/// Default width of a native `select` trigger when the protocol gives no
/// `width` (the frontend kit also sends 170 — this only covers a bare tag).
pub(crate) const DEFAULT_SELECT_W: f32 = 170.0;
/// Line box relative to font size, for the single-line fields and canvas text
/// we shape by hand. GPUI's text elements use a similar ratio, so a hand-painted
/// field does not sit visually tighter than the labels around it.
pub(crate) const LINE_RATIO: f32 = 1.35;
/// Scrollbar overlay: 10px wide, thumb at least 24px so it stays grabbable, and
/// painted *over* the content (no layout space reserved) — a `scrollbar_width`
/// of 0 keeps GPUI's own reservation out of the way.
pub(crate) const SCROLLBAR_W: f32 = 10.0;
pub(crate) const SCROLLBAR_MIN_THUMB: f32 = 24.0;
/// `#8b93a7` at 45% — the host's scrollbar grey, matching the demo palette's
/// secondary text so a host-drawn bar does not look foreign.
pub(crate) const SCROLLBAR_THUMB: u32 = 0x8B93_A773;

// ---------------------------------------------------------------------------
// App palette, mirrored from `ui/src/theme.ts`
//
// The gpui-component theme is a shadcn-neutral palette: in dark mode its
// `primary` is *near white*, so a checked checkbox, a switch track, a progress
// fill and a slider thumb all come out white-on-black — nothing like this app's
// blue. Rather than re-skinning each control (which is what produced the
// string of per-widget "style overrides" this file used to carry), the theme's
// semantic colours are pointed at the app palette once, at window setup, so
// every component that asks the theme comes out matching.
// ---------------------------------------------------------------------------
pub(crate) const P_BG: u32 = 0x0F11_17FF;
pub(crate) const P_CARD: u32 = 0x161B_26FF;
pub(crate) const P_ELEVATED: u32 = 0x1C23_33FF;
pub(crate) const P_BORDER: u32 = 0x262D_3DFF;
pub(crate) const P_ACCENT: u32 = 0x4F8C_FFFF;
pub(crate) const P_TEXT: u32 = 0xE8EA_F2FF;
pub(crate) const P_TEXT_SECONDARY: u32 = 0x8B93_A7FF;
pub(crate) const P_WHITE: u32 = 0xFFFF_FFFF;

/// Point gpui-component's semantic colours at the app palette.
///
/// Called *after* `Theme::change`, because that re-applies the whole config
/// from the theme JSON and would wipe direct field writes.
fn apply_app_palette(cx: &mut App) {
    use gpui_component::Theme as CTheme;
    let accent: gpui::Hsla = rgba(P_ACCENT).into();
    let t = CTheme::global_mut(cx);
    // Fills that mean "selected / active / progress": checkbox mark, switch
    // track, slider fill, progress bar, dropdown list selection.
    t.colors.primary = accent;
    t.colors.primary_hover = accent;
    t.colors.primary_active = accent;
    // Drawn *on* primary — the check glyph, the switch knob's contrast.
    t.colors.primary_foreground = rgba(P_WHITE).into();
    t.colors.ring = accent; // focus ring, accent-coloured like the fields
    // Surfaces + hairlines that used to be neutral greys (#2f2f2f).
    t.colors.input = rgba(P_BORDER).into();
    t.colors.border = rgba(P_BORDER).into();
    t.colors.background = rgba(P_BG).into();
    t.colors.foreground = rgba(P_TEXT).into();
    t.colors.muted = rgba(P_ELEVATED).into();
    t.colors.muted_foreground = rgba(P_TEXT_SECONDARY).into();
    t.colors.accent = rgba(P_ELEVATED).into(); // row hover
    t.colors.accent_foreground = rgba(P_TEXT).into();
    // Dropdown / popover surfaces: the app's raised card, not near-black.
    t.colors.popover = rgba(P_ELEVATED).into();
    t.colors.popover_foreground = rgba(P_TEXT).into();
    t.colors.list = rgba(P_ELEVATED).into();
    t.colors.list_active = rgba(P_ELEVATED).into();
    t.colors.list_hover = rgba(P_CARD).into();
    t.colors.list_head = rgba(P_CARD).into();
    t.colors.caret = accent; // text caret in fields
    // Switch: the off-track and its knob are their own theme fields.
    t.colors.switch = rgba(P_ELEVATED).into();
    t.colors.switch_thumb = rgba(P_TEXT_SECONDARY).into();
    // The *legacy* token layer is a cache: components such as Checkbox and
    // Switch read `theme.tokens.primary` (not `colors.primary`), and it is only
    // rebuilt from `colors` inside `apply_config` — i.e. before these writes.
    // Without this line the checkbox keeps the neutral theme's near-white mark.
    t.tokens = (&t.colors).into();
    // The base layer is a projection for host-drawn chrome (scrollbars).
    CTheme::sync_base(cx);
}
struct HostView {
    tree: Tree,
    event_tx: mpsc::Sender<String>,
    /// `Some` in QuickJS mode: window geometry is pushed to the engine as
    /// `{"t":"bom",…}` lines whenever it changes. `None` elsewhere — the Perry
    /// and node frontends have no BOM to receive it.
    metrics: Option<Arc<bom::WindowMetrics>>,
    /// Host-drawn modal for `alert`/`confirm`, if one is up.
    dialog: Option<bom::DialogRequest>,
    /// One focus handle per `input` node: created on first sight, dropped when
    /// the node leaves the tree. Handles are cheap and idempotent, but they must
    /// be *stable* across renders — creating a new one per frame would drop
    /// focus on every repaint.
    focus_handles: HashMap<u64, FocusHandle>,
    /// One scroll handle per scrolling node (`style.overflow == "scroll"`).
    /// The div writes the live offset / max offset / bounds into it during
    /// layout; the scrollbar canvas reads them back at paint time. The handle is
    /// the only channel between "GPUI scrolled something" and "we can draw it".
    scroll_handles: HashMap<u64, ScrollHandle>,
    /// Last offset pushed to the frontend per node — so the poll in `render`
    /// turns a change into exactly one event, not one per frame.
    scroll_reported: HashMap<u64, f32>,
    /// Same idea for text-field focus, which the frontend uses to style its own
    /// focus ring (`focus` / `blur` events).
    focus_reported: HashMap<u64, bool>,
    /// One gpui-component `InputState` per `input` node. The state is the
    /// editing engine (text buffer, caret, selection, undo); the `Input`
    /// element built each frame is just a view of it. Like the focus handles,
    /// entities must be *stable* across renders — recreating per frame would
    /// drop the text, the caret and the focus on every repaint.
    input_states: HashMap<u64, Entity<gpui_component::input::InputState>>,
    /// Last value pushed down per input node, so a controlled `setValue` echo
    /// (the app writes back exactly what the user typed) does not clobber the
    /// field mid-keystroke.
    input_reported: HashMap<u64, String>,
    /// One gpui-component `DatePickerState` per `date` node. Same stability
    /// contract as `input_states`: recreate-per-frame would drop the open/
    /// closed state, the selected date and the calendar entity.
    date_states: HashMap<u64, Entity<DatePickerState>>,
    /// One gpui-component `SliderState` per `slider` node (drag position,
    /// min/max/step). Same stability contract as the other state maps.
    slider_states: HashMap<u64, Entity<SliderState>>,
    /// Last slider value echoed up per node, so a controlled `setValue` echo
    /// does not fight a drag that is still in progress.
    slider_reported: HashMap<u64, String>,
    /// One gpui-component `SelectState` per `select` node (option list,
    /// selection, dropdown open flag). The option list is creation-time — a
    /// different option set is a new select, exactly like the slider scale.
    select_states: HashMap<u64, Entity<HostSelectState>>,
    /// Last select value echoed up per node, so a controlled `setValue` echo
    /// does not re-push the value the widget itself just confirmed.
    select_reported: HashMap<u64, String>,
    /// One gpui-component `TextareaState` per `textarea` node (mirrors
    /// `input_states`): the editing engine for a multi-line field.
    textarea_states: HashMap<u64, Entity<TextareaState>>,
    textarea_reported: HashMap<u64, String>,
    /// One gpui-component `ComboboxState` per `combobox` node: a searchable
    /// single-select whose option list is creation-time (like `select_states`).
    combobox_states: HashMap<u64, Entity<HostComboboxState>>,
    combobox_reported: HashMap<u64, String>,
    /// One gpui-component `ColorPickerState` per `colorpicker` node.
    color_states: HashMap<u64, Entity<ColorPickerState>>,
    color_reported: HashMap<u64, String>,
}
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
                let dropped = total - parsed.len();
                let _ = ops_tx.send_blocking(parsed);
                if log_ops_enabled() {
                    let seen = OPS_SEEN.fetch_add(total, Ordering::Relaxed) + total;
                    log!("[host] batch ops={total} dropped={dropped} total={seen}");
                }
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
    QuickJs,
    /// scriptc AOT DLL loaded at runtime (build.rs cfg=scriptc).
    #[cfg(scriptc)]
    Scriptc,
}

/// First positional argv, i.e. the frontend to run.
fn positional_arg() -> Option<String> {
    std::env::args().skip(1).next()
}

fn resolve_frontend() -> Frontend {
    if let Some(arg) = positional_arg() {
        if arg == "scriptc" {
            #[cfg(scriptc)]
            {
                return Frontend::Scriptc;
            }
            #[cfg(not(scriptc))]
            {
                log!("[host] scriptc backend not compiled in (build/scriptc_fe.dll missing at build time?)");
                std::process::exit(2);
            }
        }
        if arg.ends_with(".js") {
            return Frontend::Child { cmd: "node".to_string(), args: vec![arg] };
        }
        return Frontend::Child { cmd: arg, args: vec![] };
    }
    // Explicit selection only: GPUI_TS_BACKEND=scriptc. QuickJS stays the
    // default even when a scriptc artifact ships next to the exe — a stale one
    // must not silently hijack the primary backend.
    #[cfg(scriptc)]
    {
        if std::env::var("GPUI_TS_BACKEND").as_deref() == Ok("scriptc") {
            // Static build (scriptc_static): the engine is linked into this
            // binary — nothing to resolve.
            #[cfg(scriptc_static)]
            {
                return Frontend::Scriptc;
            }
            // Legacy DLL build (scriptc_dll): the shared library must exist.
            #[cfg(scriptc_dll)]
            {
                let exe_dir = std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|d| d.to_path_buf()));
                let dll_next_to_exe =
                    exe_dir.map(|d| d.join("scriptc_fe.dll")).is_some_and(|p| p.exists());
                if dll_next_to_exe || std::path::Path::new("build/scriptc_fe.dll").exists() {
                    return Frontend::Scriptc;
                }
                log!("[host] GPUI_TS_BACKEND=scriptc but scriptc_fe.dll not found next to exe or in ./build");
                std::process::exit(2);
            }
        }
    }
    #[cfg(quickjs)]
    {
        return Frontend::QuickJs;
    }
    #[cfg(embedded)]
    {
        return Frontend::Embedded;
    }
    #[cfg(not(any(embedded, quickjs, scriptc)))]
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

            application()
                .with_assets(gpui_kit_assets::Assets)
                .run(move |cx: &mut App| {
                let handle = open_host_window(cx, ev_tx, None);
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

            application()
                .with_assets(gpui_kit_assets::Assets)
                .run(move |cx: &mut App| {
                let handle = open_host_window(cx, ev_tx, None);
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
        Frontend::QuickJs => {
            log!("[host] quickjs mode: embedded QuickJS engine");

            // BOM state the engine needs. Seeded with the intended window size
            // because the engine boots before the window exists; the first
            // render replaces it with the real geometry.
            let metrics = bom::WindowMetrics::new(WINDOW_W, WINDOW_H, 1.0, 0.0, 0.0);
            let (dialog_tx, dialog_rx) = async_channel::unbounded::<bom::DialogRequest>();

            let host = quickjs::launch(
                quickjs::save_original_stderr(),
                ops_tx.clone(),
                metrics.clone(),
                dialog_tx,
            );
            log!("[host] quickjs engine booted in {:?}", t_start.elapsed());

            // Ops arrive through the injected `__hostEmit`; events go the other
            // way through the sink. Neither touches stdio.
            run_event_injector(host.sink.clone(), ev_rx);

            let sink = host.sink.clone();
            application()
                .with_assets(gpui_kit_assets::Assets)
                .run(move |cx: &mut App| {
                let handle = open_host_window(cx, ev_tx, Some(metrics));
                spawn_ops_apply(cx, handle, ops_rx);
                spawn_dialog_host(cx, handle, dialog_rx);
                // The QuickJS engine self-drives on its own thread (tick +
                // park/unpark); no perry_poll loop needed here.
            });

            // Window closed → let the app's stdin `end` handler run (it calls
            // process.exit). Best-effort: the engine owns the exit.
            sink.close();
        }
        #[cfg(scriptc)]
        Frontend::Scriptc => {
            #[cfg(scriptc_static)]
            log!("[host] scriptc mode: static AOT runtime (linked in-process)");
            #[cfg(scriptc_dll)]
            log!("[host] scriptc mode: AOT DLL via LoadLibrary FFI");
            let host = scriptc::launch(ops_tx.clone());
            log!("[host] scriptc engine loaded in {:?}", t_start.elapsed());

            // Ops flow through the driver thread into the shared channel
            // (scriptc.rs pushes them via ingest_line); events are queued and
            // drained by the same driver quantum.
            run_event_queuer(host.sink.clone(), ev_rx);

            application()
                .with_assets(gpui_kit_assets::Assets)
                .run(move |cx: &mut App| {
                let handle = open_host_window(cx, ev_tx, None);
                spawn_ops_apply(cx, handle, ops_rx);
                // The scriptc driver thread self-drives (fixed 10 ms quantum);
                // no host-loop timer needed here either.
            });
        }
    }
}

/// Queue events for backends whose engine is drained by a dedicated driver
/// thread (scriptc): no pipe, no unpark — the driver picks them up on its
/// next quantum.
#[cfg(scriptc)]
fn run_event_queuer(sink: scriptc::EventSink, ev_rx: mpsc::Receiver<String>) {
    std::thread::spawn(move || {
        log!("[host] event queuer up");
        for msg in ev_rx {
            log!("[host] ev queue t={}: {msg}", now_ms());
            sink.push(msg);
        }
    });
}

fn open_host_window(
    cx: &mut App,
    ev_tx: mpsc::Sender<String>,
    metrics: Option<Arc<bom::WindowMetrics>>,
) -> gpui::WindowHandle<gpui_component::Root> {
    // gpui-component's global state (theme, root rendering, input machinery).
    // Idempotent per-process; called once before any window opens.
    gpui_component::init(cx);
    // `init` pins Light mode; the dashboard's palette is dark, so flip the
    // component theme to match — otherwise native buttons come out white-on-
    // white against the dark cards. `change` re-applies the theme JSON, so the
    // app palette override has to come after it.
    gpui_component::Theme::change(gpui_component::ThemeMode::Dark, None, cx);
    apply_app_palette(cx);
    let bounds = Bounds {
        origin: point(px(80.0), px(50.0)),
        size: size(px(WINDOW_W), px(WINDOW_H)),
    };
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::from("PerryTS × GPUI")),
            ..Default::default()
        }),
        window_min_size: Some(size(px(MIN_WINDOW_W), px(MIN_WINDOW_H))),
        ..Default::default()
    };

    let handle = cx
        .open_window(options, |window, cx| {
            // HostView is the app; `gpui_component::Root` must be the window's
            // FIRST view — it owns the tooltip / native-menu overlays that
            // popover-style surfaces (Select dropdown, DatePicker calendar)
            // anchor to and dismiss against. Without it the dropdown's
            // `deferred(...)` popup still paints (its Positioner clamps to the
            // window viewport itself), but its `on_mouse_down_out` never sees
            // clicks outside, so the menu could not be closed by clicking
            // elsewhere. Root is what makes the popup behave like a popup.
            let host = cx.new(|_| HostView {
                tree: Tree::new(),
                event_tx: ev_tx,
                metrics,
                dialog: None,
                focus_handles: HashMap::new(),
                scroll_handles: HashMap::new(),
                scroll_reported: HashMap::new(),
                focus_reported: HashMap::new(),
                input_states: HashMap::new(),
                input_reported: HashMap::new(),
                date_states: HashMap::new(),
                slider_states: HashMap::new(),
                slider_reported: HashMap::new(),
                select_states: HashMap::new(),
                select_reported: HashMap::new(),
                textarea_states: HashMap::new(),
                textarea_reported: HashMap::new(),
                combobox_states: HashMap::new(),
                combobox_reported: HashMap::new(),
                color_states: HashMap::new(),
                color_reported: HashMap::new(),
            });
            // `Root::new` needs `&mut Context<Root>`, so it is built by a second
            // `cx.new` rather than inline in the window closure.
            cx.new(|cx| gpui_component::Root::new(host, window, cx))
        })
        .expect("failed to open window");
    cx.activate(true);
    log!("[host] window opened");
    handle
}

/// Deliver `alert`/`confirm` requests from the engine thread to the view.
///
/// The engine blocks inside `bom::show` while this runs, so if the view cannot
/// be reached (window already gone) we must answer anyway — otherwise the
/// engine thread would never wake up.
///
/// QuickJS-only: it exists to serve `bootstrap.js`, and only that frontend has
/// a BOM (see `bom`).
#[cfg_attr(not(quickjs), allow(dead_code))]
fn spawn_dialog_host(
    cx: &mut App,
    handle: gpui::WindowHandle<gpui_component::Root>,
    dialog_rx: async_channel::Receiver<bom::DialogRequest>,
) {
    cx.spawn(async move |cx| {
        while let Ok(req) = dialog_rx.recv().await {
            log!(
                "[host] dialog request: kind={:?} msg={:?}",
                req.kind,
                req.message
            );
            let reply = req.reply.clone();
            // The app view now lives one layer down (Root wraps it): the
            // handle's view type is Root, and the HostView is Root's inner
            // `view()` AnyView.
            let shown = cx.update(|cx| {
                handle.update(cx, |_root, _window, cx| {
                    // `AnyView::downcast` consumes the view, so clone the Arc.
                    if let Ok(host) = _root.view().clone().downcast::<HostView>() {
                        host.update(cx, |view, cx| {
                            view.dialog = Some(req);
                            cx.notify();
                        });
                    } else {
                        log!("[host] host view not found under Root");
                    }
                })
            });
            if shown.is_err() {
                log!("[host] dialog dropped (window gone) — replying default");
                let _ = reply.send(0);
            }
        }
    })
    .detach();
}

fn spawn_ops_apply(
    cx: &mut App,
    handle: gpui::WindowHandle<gpui_component::Root>,
    ops_rx: async_channel::Receiver<Vec<Op>>,
) {
    cx.spawn(async move |cx| {
        while let Ok(ops) = ops_rx.recv().await {
            let _ = cx.update(|cx| {
                let _ = handle.update(cx, |_root, window, cx| {
                    // The app view lives inside Root (see `open_host_window`);
                    // downcast Root's inner AnyView to reach it. `downcast`
                    // consumes the view, so the Arc is cloned.
                    let Ok(host) = _root.view().clone().downcast::<HostView>() else {
                        log!("[host] ops dropped (host view not under Root)");
                        return;
                    };
                    host.update(cx, |view, cx| {
                        if let Some(title) = view.tree.apply(&ops) {
                            window.set_window_title(&title);
                        }
                        if dump_tree_enabled() && ops.len() >= 32 {
                            log!("[host] tree after {} ops:\n{}", ops.len(), view.tree.dump());
                        }
                        // AOT 前端把挂载摊成单 op 行（batch ops=1），≥32 阈值永不触发；
                        // 早期批次（前 400 个 op）也 dump，便于排障。
                        if dump_tree_enabled() && OPS_SEEN.load(Ordering::Relaxed) < 400 {
                            log!("[host] early tree after {} ops:\n{}", ops.len(), view.tree.dump());
                        }
                        cx.notify();
                    });
                });
            });
        }
    })
    .detach();
}
