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
//! the protocol does not go through stdio at all: the host injects ABI host
//! functions into the JS context (`__hostOps` — a typed argument array, no
//! JSON round-trip on the engine side) for outbound ops, and pushes
//! [`protocol::OutEvent`] structs straight into the engine queue where they
//! become typed arguments of a JS callback. The child/embedded/scriptc paths
//! keep the JSONL wire format: GPUI-side emit sites hand over structured
//! events, and `protocol::OutEvent::to_line` is the single serialization
//! point — neither side of those paths can drift.

mod draw;
mod tree;
mod protocol;
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

#[cfg(all(has_embedded_frontend, any(feature = "be-quickjs", feature = "be-perry")))]
// Compiled in both embedded and quickjs modes; in quickjs mode the Perry
// poll-loop helpers are simply unused, which is expected (not a defect).
#[cfg_attr(quickjs, allow(dead_code, unused_imports))]
mod embedded;

#[cfg(quickjs)]
mod quickjs;

/// Chrome DevTools Protocol server (UI/style debugging). `GPUI_TS_CDP=9222`
/// turns it on; threads and the GPUI-side bridge live in `cdp`/`cdp_state`.
#[cfg_attr(not(quickjs), allow(dead_code))]
mod cdp;
#[cfg_attr(not(quickjs), allow(dead_code))]
mod cdp_state;

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
#[cfg(not(any(all(embedded, feature = "be-perry"), quickjs)))]
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
    #[cfg(all(has_embedded_frontend, any(feature = "be-quickjs", feature = "be-perry")))]
    {
        let h = embedded::save_original_stderr();
        LOG_HANDLE.store(h as usize, Ordering::Relaxed);
    }
    #[cfg(not(all(has_embedded_frontend, any(feature = "be-quickjs", feature = "be-perry"))))]
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
    #[cfg(all(has_embedded_frontend, any(feature = "be-quickjs", feature = "be-perry")))]
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
pub(crate) fn log_ops_enabled() -> bool {
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
    /// Structured outbound events (the ABI channel). quickjs injects them as
    /// typed JS callback arguments; the JSONL backends serialize at the last
    /// moment via `OutEvent::to_line` (see `run_event_writer`).
    event_tx: mpsc::Sender<protocol::OutEvent>,
    /// Ops inbound channel, drained synchronously at the top of every render
    /// (`drain_ops`). `Some` in every backend today; a backend that wires ops
    /// elsewhere (e.g. its own GPUI task) can pass `None` to opt out.
    ops_rx: Option<async_channel::Receiver<Vec<Op>>>,
    /// `setTitle` op arriving inside a `drain_ops` batch: `Tree::apply` returns
    /// the title but the platform window handle is only reachable from
    /// `render(&mut Window)`, so it is parked here and flushed at frame start.
    pending_title: Option<String>,
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
    /// Last tick's raw post-tick offset per scrolling node, remembered by the
    /// wheel-chain guard (see `build_plain`). Layout clamps the offset cell, so
    /// a "pre" reconstructed as `offset - dy` is only exact when no frame ran
    /// between ticks; this anchor survives that. `Rc<Cell<_>>` because the
    /// guard closure is an `Fn` and outlives the borrow of `self`.
    scroll_last_raw: HashMap<u64, std::rc::Rc<std::cell::Cell<f32>>>,
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
    /// Optimistic selection overrides for stateful picker widgets (tabs/radio/
    /// pagination): written synchronously in the click callback so the next
    /// frame shows the new selection *before* the JS round-trip (~1 frame
    /// period + engine latency, measured ≈88 ms) echoes the `setValue` back.
    /// The echo applies the same value, so clearing the override on apply is
    /// visually a no-op — unless the app rejects the click, in which case the
    /// override clearing correctly snaps back to the app-owned state.
    tab_override: HashMap<u64, usize>,
    /// CDP bridge (quickjs builds): served tree reads at frame start, so the
    /// DevTools DOM/CSS panes see the same retained tree the renderer walks.
    #[allow(dead_code)]
    cdp_shared: Option<Arc<cdp_state::CdpShared>>,
}
pub(crate) fn ingest_line(line: &str, ops_tx: &async_channel::Sender<Vec<Op>>) {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return;
    };
    match v.get("t").and_then(|t| t.as_str()) {
        Some("hello") => {
            log!("[host] frontend hello: {line}");
        }
        Some("log") => {
            // Frontend diagnostics ({"t":"log","msg":…}). console.* is NOT a
            // reliable carrier here: perry's Node-style console writes stdout
            // — which IS the ops pipe in embedded mode — and non-JSON lines
            // are dropped above. The explicit channel lands on every backend.
            let msg = v.get("msg").and_then(|m| m.as_str()).unwrap_or("");
            log!("[frontend] {msg}");
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
                    log!(
                        "[host] batch ops={total} dropped={dropped} total={seen} t={}",
                        now_ms()
                    );
                }
            }
        }
        _ => {}
    }
}

pub(crate) fn run_ops_reader<R: std::io::Read + Send + 'static>(
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

fn run_event_writer<W: std::io::Write + Send + 'static>(mut w: W, ev_rx: mpsc::Receiver<protocol::OutEvent>) {
    std::thread::spawn(move || {
        for ev in ev_rx {
            let msg = ev.to_line();
            log!("[host] {} t={}: {msg}", ev.log_tag(), now_ms());
            if writeln!(w, "{msg}").and_then(|_| w.flush()).is_err() {
                log!("[host] ev write FAILED (frontend stdin closed?)");
                break;
            }
        }
    });
}

/// Injected transport (QuickJS `Direct` mode): hand each structured event
/// straight to the engine's inbound queue and unpark its tick. No
/// serialization at all — `quickjs.rs::pump` turns the enum into typed
/// arguments of the `__hostEventSink` JS callback.
///
/// Compared with `run_event_writer` this drops a pipe write, a blocking read,
/// a line split, a UTF-8 decode AND the JSON round-trip.
#[cfg(quickjs)]
fn run_event_injector(sink: quickjs::EventSink, ev_rx: mpsc::Receiver<protocol::OutEvent>) {
    std::thread::spawn(move || {
        log!("[host] event injector up");
        for ev in ev_rx {
            log!("[host] {} t={}", ev.log_tag(), now_ms());
            sink.push_event(ev);
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
    #[cfg(all(embedded, feature = "be-perry"))]
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

/// `GPUI_TS_BACKEND` — runtime backend selection. All three engines may be
/// linked into one binary; this picks the one that boots.
fn backend_env() -> Option<String> {
    std::env::var("GPUI_TS_BACKEND")
        .ok()
        .map(|s| s.to_ascii_lowercase())
}

/// Build-time default backend, injected by the CLI as GPUI_TS_DEFAULT_BACKEND
/// (build.rs `option_env!` → cargo:rustc-cfg=default_scriptc / default_perry).
/// Lets `nativets build --backend scriptc` produce an exe that boots scriptc
/// with zero runtime configuration, while GPUI_TS_BACKEND still overrides.
fn default_backend() -> &'static str {
    #[cfg(default_scriptc)]
    {
        "scriptc"
    }
    #[cfg(default_perry)]
    {
        "perry"
    }
    #[cfg(not(any(default_scriptc, default_perry)))]
    {
        "quickjs"
    }
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
        if arg == "perry" {
            #[cfg(all(embedded, feature = "be-perry"))]
            {
                return Frontend::Embedded;
            }
            #[cfg(not(all(embedded, feature = "be-perry")))]
            {
                log!("[host] perry backend not compiled in (PERRY_NO_EMBED set / archives missing / built without feature be-perry)");
                std::process::exit(2);
            }
        }
        if arg.ends_with(".js") {
            return Frontend::Child { cmd: "node".to_string(), args: vec![arg] };
        }
        return Frontend::Child { cmd: arg, args: vec![] };
    }
    // No explicit selection: GPUI_TS_BACKEND wins, then the build-time
    // default (GPUI_TS_DEFAULT_BACKEND injected by the CLI), then quickjs.
    // A build-time default only ever points at a backend that was actually
    // linked in (the CLI drives both flags), and a stale artifact from
    // another backend cannot silently hijack the boot choice because the
    // cfg pair is regenerated on every CLI build.
    match backend_env().as_deref() {
        Some("scriptc") => {
            #[cfg(scriptc)]
            {
                return Frontend::Scriptc;
            }
            #[cfg(not(scriptc))]
            {
                log!("[host] GPUI_TS_BACKEND=scriptc but scriptc backend not compiled in");
                std::process::exit(2);
            }
        }
        Some("perry") => {
            #[cfg(all(embedded, feature = "be-perry"))]
            {
                return Frontend::Embedded;
            }
            #[cfg(not(all(embedded, feature = "be-perry")))]
            {
                log!("[host] GPUI_TS_BACKEND=perry but perry backend not compiled in");
                std::process::exit(2);
            }
        }
        Some("quickjs") => {}
        Some(other) => {
            log!("[host] unknown GPUI_TS_BACKEND={other} (quickjs|perry|scriptc) — falling back to default");
        }
        None => {}
    }
    match default_backend() {
        "scriptc" => {
            #[cfg(scriptc)]
            {
                return Frontend::Scriptc;
            }
            #[cfg(not(scriptc))]
            {
                log!("[host] default backend scriptc not compiled in — falling back");
            }
        }
        "perry" => {
            #[cfg(all(embedded, feature = "be-perry"))]
            {
                return Frontend::Embedded;
            }
            #[cfg(not(all(embedded, feature = "be-perry")))]
            {
                log!("[host] default backend perry not compiled in — falling back");
            }
        }
        _ => {}
    }
    #[cfg(quickjs)]
    {
        return Frontend::QuickJs;
    }
    #[cfg(all(embedded, feature = "be-perry"))]
    {
        return Frontend::Embedded;
    }
    #[allow(unreachable_code)]
    {
        // The `use std::path::Path` at the top is feature-conditional; be
        // fully explicit here so every remaining combination compiles.
        use std::path::Path;
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
    let (ev_tx, ev_rx) = mpsc::channel::<protocol::OutEvent>();
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
                let handle = open_host_window(cx, ev_tx, ops_rx.clone(), None, None);
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
        #[cfg(all(embedded, feature = "be-perry"))]
        Frontend::Embedded => {
            log!("[host] embedded mode: perry staticlib in-process");
            // launch() starts the ops reader BEFORE perry_module_init: the
            // init writes the whole mount batch synchronously and a ~4 KB
            // anonymous-pipe buffer with no reader deadlocks it (blank window).
            let ef = embedded::launch(embedded::save_original_stderr(), ops_tx.clone());
            log!("[host] perry_module_init done in {:?}", t_start.elapsed());

            run_event_writer(ef, ev_rx);

            application()
                .with_assets(gpui_kit_assets::Assets)
                .run(move |cx: &mut App| {
                let handle = open_host_window(cx, ev_tx, ops_rx.clone(), None, None);
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
                        #[cfg(feature = "be-perry")]
                        {
                            embedded::js_stdlib_process_pending();
                        }
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

            // CDP (UI/style debugging) — off unless GPUI_TS_CDP=<port>. The
            // shared bridge is always constructed in quickjs mode (cheap);
            // only the server threads are conditional.
            let cdp_shared = cdp_state::CdpShared::new(ops_tx.clone());
            if let Some(port) = cdp::enabled_port() {
                cdp::launch(port, cdp_shared.clone());
            }

            let host = quickjs::launch(
                quickjs::save_original_stderr(),
                ops_tx.clone(),
                metrics.clone(),
                dialog_tx,
                Some(cdp_shared.clone()),
            );
            log!("[host] quickjs engine booted in {:?}", t_start.elapsed());

            // Ops arrive through the injected `__hostEmit`; events go the other
            // way through the sink. Neither touches stdio.
            run_event_injector(host.sink.clone(), ev_rx);

            let sink = host.sink.clone();
            let cdp_for_view = cdp_shared.clone();
            let cdp_for_pump = cdp_shared.clone();
            application()
                .with_assets(gpui_kit_assets::Assets)
                .run(move |cx: &mut App| {
                let handle = open_host_window(cx, ev_tx, ops_rx.clone(), Some(metrics), Some(cdp_for_view));
                spawn_ops_apply(cx, handle, ops_rx);
                spawn_dialog_host(cx, handle, dialog_rx);
                spawn_cdp_pump(cx, handle, cdp_for_pump);
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
                let handle = open_host_window(cx, ev_tx, ops_rx.clone(), None, None);
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
fn run_event_queuer(sink: scriptc::EventSink, ev_rx: mpsc::Receiver<protocol::OutEvent>) {
    std::thread::spawn(move || {
        log!("[host] event queuer up");
        for ev in ev_rx {
            log!("[host] {} t={}", ev.log_tag(), now_ms());
            sink.push(ev.to_line());
        }
    });
}

fn open_host_window(
    cx: &mut App,
    ev_tx: mpsc::Sender<protocol::OutEvent>,
    ops_rx: async_channel::Receiver<Vec<Op>>,
    metrics: Option<Arc<bom::WindowMetrics>>,
    #[allow(unused_variables)] cdp_shared: Option<Arc<cdp_state::CdpShared>>,
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
            title: Some(SharedString::from("nativets × GPUI")),
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
                ops_rx: Some(ops_rx),
                pending_title: None,
                metrics,
                dialog: None,
                focus_handles: HashMap::new(),
                scroll_handles: HashMap::new(),
                scroll_last_raw: HashMap::new(),
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
                tab_override: HashMap::new(),
                cdp_shared,
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

/// CDP tree-read pump (quickjs + DevTools). `drain_requests` already runs
/// once per frame in `render`, but that only helps when the app renders —
/// a static UI idles at the ~500 ms heartbeat, so DevTools' DOM/CSS reads
/// would each wait up to half a second (the earlier per-node hop version
/// multiplied that by the node count and blew client timeouts outright).
///
/// Instead of a poll loop, this waits on the request queue itself: the CDP
/// thread signals an "incoming" channel when it enqueues, we wake (~≤50 ms
/// wake latency, no busy polling), drain directly on the main thread through
/// the window handle, and go back to sleep. `drain_requests` uses `mem::take`
/// so this and the per-frame render drain can never double-serve a request.
#[cfg_attr(not(quickjs), allow(dead_code))]
fn spawn_cdp_pump(
    cx: &mut App,
    handle: gpui::WindowHandle<gpui_component::Root>,
    cdp_shared: Arc<cdp_state::CdpShared>,
) {
    let (notify_tx, notify_rx) = async_channel::unbounded::<()>();
    cdp_shared.attach_notify(notify_tx);
    cx.spawn(async move |cx| {
        while notify_rx.recv().await.is_ok() {
            // Coalesce a burst: one wake serves everything queued so far.
            while notify_rx.try_recv().is_ok() {}
            let _ = cx.update(|cx| {
                let _ = handle.update(cx, |_root, _window, cx| {
                    let Ok(host) = _root.view().clone().downcast::<HostView>() else {
                        return;
                    };
                    host.update(cx, |view, _cx| {
                        if let Some(sh) = view.cdp_shared.as_ref() {
                            sh.drain_requests(&view.tree);
                        }
                    });
                });
            });
        }
    })
    .detach();
}

fn spawn_ops_apply(
    cx: &mut App,
    handle: gpui::WindowHandle<gpui_component::Root>,
    ops_rx: async_channel::Receiver<Vec<Op>>,
) {    cx.spawn(async move |cx| {
        while let Ok(ops) = ops_rx.recv().await {
            // Split the latency: channel wait (ops sat in the queue before the
            // GPUI executor polled us) vs apply itself. Log EVERY batch when
            // ops-logging is on — the heartbeat (clock) batches give a send→recv
            // baseline that click batches must be compared against.
            let has_set_value = ops.iter().any(|o| matches!(o, tree::Op::SetValue { .. }));
            if log_ops_enabled() {
                log!(
                    "[host] recv batch ops={}{} t={}",
                    ops.len(),
                    if has_set_value { " incl SetValue" } else { "" },
                    now_ms()
                );
            }
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
                        let mut set_values: Vec<(u64, String)> = Vec::new();
                        if let Some(title) =
                            view.tree.apply_with_set_values(&ops, &mut set_values)
                        {
                            window.set_window_title(&title);
                        }
                        // The app's JS has caught up with any optimistic click:
                        // drop the overrides it echoed (same value) or rejected
                        // (snaps back).
                        for (id, _) in &set_values {
                            view.tab_override.remove(id);
                        }
                        if log_ops_enabled() {
                            log!(
                                "[host] applied {} ops{} t={}",
                                ops.len(),
                                if has_set_value { " incl SetValue" } else { "" },
                                now_ms()
                            );
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
