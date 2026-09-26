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
    ($($arg:tt)*) => { log_line(&format!($($arg)*)) };
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

// ---------------------------------------------------------------------------
// Style application: CSS-ish keys -> GPUI builder calls
//
// Div is move-only, so each optional style goes through a helper that takes
// the element by value and returns it untouched when the value is absent.
//
// Everything below is generic over `Styled` rather than `Div`: a `canvas`
// element (used for `input` fields and `canvas` nodes, see `build_input` /
// `build_canvas`) understands the same style keys, and sharing the mapper is
// what keeps a styled canvas and a styled div from drifting apart.
// ---------------------------------------------------------------------------

/// `#rrggbb` / `#rrggbbaa` — the canvas vocabulary also accepts `#abc` and
/// `rgb()/rgba()` spellings, so style and canvas colors share one parser
/// (`draw::parse_color`). 0xRRGGBBAA is the layout `rgba()` expects.
fn parse_color(s: &str) -> Option<u32> {
    draw::parse_color(s)
}

fn num(v: &Value) -> Option<f32> {
    v.as_f64().map(|f| f as f32)
}

fn with_px<D: Styled>(d: D, v: &Value, f: impl FnOnce(D, f32) -> D) -> D {
    match num(v) {
        Some(x) => f(d, x),
        None => d,
    }
}

fn with_color<D: Styled>(d: D, v: &Value, f: impl FnOnce(D, u32) -> D) -> D {
    match v.as_str().and_then(parse_color) {
        Some(c) => f(d, c),
        None => d,
    }
}

fn with_pct<D: Styled>(d: D, s: &str, f: impl FnOnce(D, f32) -> D) -> D {
    match s.strip_suffix('%').and_then(|p| p.parse::<f32>().ok()) {
        Some(frac) => f(d, frac / 100.0),
        None => d,
    }
}

/// Font weight from the style map: `"bold" | "semibold" | "medium" | 600`.
/// Shared by the style mapper and by the canvas/input painters, which shape
/// text themselves and would otherwise pick a different weight than the
/// `fontSize`-styled div next to them.
fn font_weight_of(v: Option<&Value>) -> FontWeight {
    match v {
        Some(Value::String(s)) if s == "bold" => FontWeight::BOLD,
        Some(Value::String(s)) if s == "semibold" => FontWeight::SEMIBOLD,
        Some(Value::String(s)) if s == "medium" => FontWeight::MEDIUM,
        Some(Value::Number(n)) => FontWeight(n.as_f64().unwrap_or(400.0) as f32),
        _ => FontWeight::NORMAL,
    }
}

fn apply_style<D: Styled>(d: D, key: &str, v: &Value) -> D {
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
        // `grow: n` means CSS `flex: n 1 0%` — "take a share of the *remaining*
        // space", which is what every call site means by it. Setting the grow
        // factor alone leaves `flex-basis` at `auto` (content size), so a child
        // that is taller than the space it was handed overflows its parent
        // instead of scrolling inside it. The `min_*` zeros are the CSS
        // `min-width/height: 0` without which a flex item refuses to shrink
        // below its content size at all.
        "grow" | "flex" => with_px(d, v, |d, x| {
            d.flex_grow(x)
                .flex_shrink(1.0)
                .flex_basis(px(0.0))
                .min_w(px(0.0))
                .min_h(px(0.0))
        }),
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
        // placement — what a dropdown panel needs to leave the flow
        "position" => match v.as_str().unwrap_or("") {
            "absolute" => d.absolute(),
            "relative" => d.relative(),
            // No fixed positioning in GPUI; treating it as absolute keeps a
            // `position: fixed` node at least *on screen* rather than in flow.
            "fixed" => d.absolute(),
            _ => d,
        },
        "top" => with_px(d, v, |d, x| d.top(px(x))),
        "left" => with_px(d, v, |d, x| d.left(px(x))),
        "right" => with_px(d, v, |d, x| d.right(px(x))),
        "bottom" => with_px(d, v, |d, x| d.bottom(px(x))),
        "cursor" => match v.as_str().unwrap_or("") {
            "pointer" => d.cursor_pointer(),
            "text" => d.cursor_text(),
            _ => d,
        },
        // visual
        "background" | "bg" | "backgroundColor" => with_color(d, v, |d, c| d.bg(rgba(c))),
        "borderRadius" | "radius" => match v {
            Value::String(s) if s == "full" || s == "50%" => d.rounded_full(),
            _ => with_px(d, v, |d, x| d.rounded(px(x))),
        },
        "borderWidth" => match num(v) {
            Some(n) if n > 0.0 => d.border_1(),
            _ => d,
        },
        "borderColor" => with_color(d, v, |d, c| d.border_color(rgba(c))),
        "opacity" => with_px(d, v, |d, x| d.opacity(x)),
        // `scroll` is not handled here: a scroll container is built from two
        // nested elements plus a ScrollHandle (see `build_plain`), so only the
        // clipping case is a plain style.
        "overflow" | "overflowY" | "overflowX" => match v.as_str().unwrap_or("") {
            "hidden" | "clip" => d.overflow_hidden(),
            _ => d,
        },
        // text
        "color" => with_color(d, v, |d, c| d.text_color(rgba(c))),
        "fontSize" => with_px(d, v, |d, x| d.text_size(px(x))),
        "fontWeight" => d.font_weight(font_weight_of(Some(v))),
        _ => d,
    }
}

/// Keys that describe the **box** (how big, and where in the parent), as
/// opposed to the content flow inside it.
///
/// A scroll container is two elements — the wrapper owns the box, the scroller
/// owns the flow — so the style map has to be split between them. Getting this
/// wrong is visible: a `width` on both makes the scroller overflow the wrapper.
fn is_box_key(k: &str) -> bool {
    matches!(
        k,
        "width"
            | "height"
            | "minWidth"
            | "minHeight"
            | "maxWidth"
            | "maxHeight"
            | "grow"
            | "flex"
            | "shrink"
            | "position"
            | "top"
            | "left"
            | "right"
            | "bottom"
    )
}

fn is_scroll_container(node: &Node) -> bool {
    let overflow = node
        .style
        .get("overflow")
        .or_else(|| node.style.get("overflowY"))
        .and_then(|v| v.as_str());
    overflow == Some("scroll")
}

/// `display: "contents"` — a fragment. The node contributes **no box**: its
/// children are inlined into its parent's flow (see `build_plain`).
///
/// `Show` and `Fragment` are built on this. Both used to be a plain flex box,
/// which silently became the *containing block* for `position: absolute`
/// descendants — a dropdown panel written inside a `position: relative`
/// component would be placed against the wrapper instead, landing beside the
/// trigger rather than under it.
fn is_transparent(node: &Node) -> bool {
    node.style.get("display").and_then(|v| v.as_str()) == Some("contents")
}

/// Where a scrollbar sits inside its container: just inside the border, plus a
/// hairline so it does not straddle the border line itself.
fn scroll_inset(node: &Node) -> f32 {
    num_of(node, "borderWidth").unwrap_or(0.0) + 1.0
}

// ---------------------------------------------------------------------------
// Host view: retained tree -> GPUI elements
// ---------------------------------------------------------------------------

/// Initial window geometry. Single source of truth: `WindowMetrics` is seeded
/// from these so the engine (which boots *before* the window exists) already
/// reports the right `innerWidth`/`innerHeight`, and the first render replaces
/// them with the real post-decoration values.
const WINDOW_W: f32 = 1180.0;
const WINDOW_H: f32 = 760.0;
const MIN_WINDOW_W: f32 = 700.0;
const MIN_WINDOW_H: f32 = 480.0;

/// Default width of a text field, in the spirit of a browser's `input`
/// (whose intrinsic width is about 20 characters ≈ 173px). A canvas-backed
/// field cannot size itself from content, so *something* has to be definite —
/// a bare `<input>` in CSS does the same thing, just with a different number.
const DEFAULT_INPUT_W: f32 = 220.0;
const DEFAULT_FONT_SIZE: f32 = 13.0;
/// Line box relative to font size, for the single-line fields and canvas text
/// we shape by hand. GPUI's text elements use a similar ratio, so a hand-painted
/// field does not sit visually tighter than the labels around it.
const LINE_RATIO: f32 = 1.35;
/// Scrollbar overlay: 10px wide, thumb at least 24px so it stays grabbable, and
/// painted *over* the content (no layout space reserved) — a `scrollbar_width`
/// of 0 keeps GPUI's own reservation out of the way.
const SCROLLBAR_W: f32 = 10.0;
const SCROLLBAR_MIN_THUMB: f32 = 24.0;
/// `#8b93a7` at 45% — the host's scrollbar grey, matching the demo palette's
/// secondary text so a host-drawn bar does not look foreign.
const SCROLLBAR_THUMB: u32 = 0x8B93_A773;

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
}

/// Deterministic application order for style keys (HashMap iteration is
/// random; compound keys like `padding` must apply before `paddingX`).
fn style_rank(k: &str) -> u8 {
    match k {
        "padding" => 0,
        "paddingX" | "paddingY" => 1,
        "paddingLeft" | "paddingRight" | "paddingTop" | "paddingBottom" => 2,
        "width" | "height" | "minWidth" | "minHeight" | "maxWidth" | "maxHeight" => 3,
        // after the size, before the flow: an absolute box needs its own
        // width/height to be meaningful (`right: 0` alone would stretch it)
        "position" => 4,
        "top" | "left" | "right" | "bottom" => 5,
        "flexDirection" | "gap" | "grow" | "flex" | "shrink" => 6,
        "alignItems" | "align" | "justifyContent" | "justify" => 7,
        "borderWidth" => 8,
        "borderColor" => 9,
        "borderRadius" | "radius" => 10,
        "background" | "bg" | "backgroundColor" => 11,
        "overflow" | "overflowX" | "overflowY" => 12,
        "opacity" => 13,
        "color" => 14,
        "fontSize" => 15,
        "fontWeight" => 16,
        "cursor" => 17,
        _ => 18,
    }
}

impl HostView {
    /// `no_shrink` is set on the direct children of a scroll container. A flex
    /// child's minimum size is 0 in GPUI, where CSS uses `min-height: auto`:
    /// without this the content of a scroller compresses to exactly the viewport
    /// height and the area has nothing left to scroll.
    fn build_node(
        &mut self,
        id: u64,
        no_shrink: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // The node is cloned: the tree must stay borrowable while the subtree
        // below is built (and while host-side per-node state is created).
        let node = self.tree.node(id)?.clone();
        if node.is_canvas() {
            return Some(self.build_canvas(&node, id, no_shrink, cx));
        }
        if node.is_input() {
            return Some(self.build_input(&node, id, no_shrink, window, cx));
        }

        if is_native_tag(&node.tag) {
            return Some(self.build_native(&node, id, window, cx));
        }
        Some(self.build_plain(&node, id, no_shrink, window, cx))
    }

    /// A regular box: styles + text + children (+ click), plus the two-element
    /// dance a scroll container needs.
    ///
    /// Scroll is why this is not a single div: GPUI scrolls an element's whole
    /// child list, so a scrollbar painted as a child would slide away with the
    /// content. The wrapper owns the box and stays put, the scroller owns the
    /// flow, and the bar is an absolutely positioned sibling of the scroller.
    fn build_plain(
        &mut self,
        node: &Node,
        id: u64,
        no_shrink: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let scroll = is_scroll_container(node);
        let mut wrapper = if scroll {
            Some(div().flex().flex_col().relative())
        } else {
            None
        };
        let mut d = div().flex().flex_row();

        // style — sorted for deterministic compound-key semantics
        for (k, v) in sorted_style(node) {
            if wrapper.is_some() && is_box_key(k) {
                // box keys belong to the wrapper (see `is_box_key`)
                let w = wrapper.take().expect("wrapper");
                wrapper = Some(apply_style(w, k, v));
                continue;
            }
            d = apply_style(d, k, v);
        }

        // text content (text nodes carry a string)
        if let Some(text) = &node.text {
            d = d.child(SharedString::from(text.clone()));
        }

        // children
        for child in &node.children {
            let child = *child;
            // A fragment (`display: contents`) is flattened away here rather
            // than built: no box means no element, and its children take its
            // place in *this* node's flow. That is what keeps an extra wrapper
            // from becoming the containing block for absolutely positioned
            // descendants, and what makes `<></>` behave like a real fragment.
            let inlined: Option<Vec<u64>> = self
                .tree
                .node(child)
                .filter(|n| is_transparent(n))
                .map(|n| n.children.clone());
            match inlined {
                Some(grandchildren) => {
                    for gc in grandchildren {
                        if let Some(el) = self.build_node(gc, scroll, window, cx) {
                            d = d.child(el);
                        }
                    }
                }
                None => {
                    if let Some(el) = self.build_node(child, scroll, window, cx) {
                        d = d.child(el);
                    }
                }
            }
        }

        // Content inside a scroller keeps its size (see `build_node`). The box
        // keys sized the wrapper, so the wrapper is what the parent flexes.
        if no_shrink
            && !node.style.contains_key("shrink")
            && !node.style.contains_key("grow")
            && !node.style.contains_key("flex")
        {
            match wrapper.take() {
                Some(w) => wrapper = Some(w.flex_shrink(0.0)),
                None => d = d.flex_shrink(0.0),
            }
        }

        // interactivity: id first (makes the div stateful), then scroll/overflow
        // and listeners — those methods live on StatefulInteractiveElement.
        let element_id = ElementId::from(SharedString::from(format!("node-{}", id)));
        let mut s = d.id(element_id);
        if scroll {
            let handle = self.scroll_handle(id);
            // The scroller must be bounded. `overflow_y_scroll` derives the
            // viewport from this element's own height, but a flex child in a
            // column defaults to "content height" — the container would grow to
            // fit instead of scrolling, and the wheel would do nothing. The box
            // keys sized the *wrapper*, so pin this to it.
            s = s
                .track_scroll(&handle)
                .overflow_y_scroll()
                .h_full()
                .min_h(px(0.0))
                // Scroll chaining: gpui's built-in wheel handling never stops
                // propagation, so with nested scrollers BOTH scroll on one
                // wheel tick. This guard restores the CSS rule: the scroller
                // under the cursor consumes the event while it can still move
                // in the wheeled direction, and only lets it chain to the
                // ancestor once it is AT (or past) its edge.
                //
                // Ordering (why the check reads the post-wheel offset): within
                // one element the guard (`on_scroll_wheel`) registers before
                // the built-in `paint_scroll_listener`, and the bubble phase
                // dispatches in REVERSE registration order — so the built-in
                // handler applies the delta FIRST and this guard runs AFTER
                // it. `offset` is therefore the post-tick position, which is
                // exactly what we want: a down-tick from exactly the top must
                // move the inner scroller (post-offset now inside the range →
                // stop), while an up-tick already at the top produced no real
                // movement (post-offset still at/past the edge → chain).
                .on_scroll_wheel({
                    let wheel_handle = handle.clone();
                    move |ev: &ScrollWheelEvent, window, cx| {
                        let dy = ev.delta.pixel_delta(window.line_height()).y;
                        if dy == px(0.0) {
                            return; // horizontal-only tick: nothing to claim
                        }
                        let max = wheel_handle.max_offset().y;
                        if max <= px(0.0) {
                            return; // nothing to scroll — chain freely
                        }
                        let offset = wheel_handle.offset().y; // post-tick, ≤ 0
                        // offset.y: 0 == top, -max == bottom. dy < 0 scrolls
                        // toward the bottom, dy > 0 toward the top (gpui adds
                        // the delta straight onto the offset cell, unclamped
                        // until the next layout — so past-edge offsets like
                        // -614 with max=338 are normal between events).
                        let at_top = offset >= px(0.0);
                        let at_bottom = offset <= -max;
                        let can_move = (dy < px(0.0) && !at_bottom)
                            || (dy > px(0.0) && !at_top);
                        if can_move {
                            cx.stop_propagation();
                        }
                    }
                });
        }
        if wants(node, "click") {
            s = s.on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                // Exactly one protocol event per physical click: GPUI bubbles
                // clicks host-side (nested clickable divs each fire, child
                // first), but the protocol delivers events to a PRECISE
                // target — bubbling is the frontend runtime's job
                // (io.ts dispatchEvent walks the mount tree). Without this
                // stop, an inner hit produces one event per ancestor that
                // declared onClick.
                cx.stop_propagation();
                // C3 latency: epoch ms aligned with the frontend's Date.now()
                log!("[host] click id={id} t={}", now_ms());
                this.send_event(id, "click", None);
            }));
        }

        let inner: AnyElement = match wrapper {
            None => s.into_any_element(),
            Some(w) => w
                .child(s)
                .child(scrollbar_overlay(self.scroll_handle(id), scroll_inset(node)))
                .into_any_element(),
        };
        // `elevate` is not a style but a paint-order change: GPUI's `deferred`
        // keeps the element in flow and draws it after its ancestors, which is
        // exactly a z-index. Without it a dropdown panel is painted *under* the
        // siblings that follow it in tree order (the footer, the next card).
        match num_of(node, "elevate") {
            Some(p) if p > 0.0 => deferred(inner)
                .with_priority(p as usize)
                .into_any_element(),
            _ => inner,
        }
    }

    /// A text field: a gpui-component `Input` bound to a per-node `InputState`.
    ///
    /// The state (text buffer, caret, selection, undo) is created once per node
    /// and kept in `input_states`; the element built every frame is just a view
    /// of it. User edits are echoed back as `input` events (each keystroke) and
    /// `change` events (Enter) — the same contract the hand-painted field had,
    /// so the frontend's controlled loop does not change.
    ///
    /// Protocol styles land on the *wrapper*: `Input` is `Styled` too, but its
    /// own chrome (border, background, focus ring) comes from the theme, and
    /// letting arbitrary protocol colors fight it produced broken visuals. The
    /// wrapper carries size/padding/flow; the field fills it.
    fn build_input(
        &mut self,
        node: &Node,
        id: u64,
        no_shrink: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.input_state(id, node, window, cx);

        // Controlled push-down: a `setValue` that differs from what the state
        // already holds (and from what we last echoed up) is an external
        // correction — apply it. Our own echo comes back with the same value
        // the state already has, so the comparison is what makes editing
        // mid-string possible (no caret snap per keystroke).
        let tree_value = node.value.clone().unwrap_or_default();
        let live = state.read(cx).value().to_string();
        let echoed = self.input_reported.get(&id).map(|s| *s == live).unwrap_or(false);
        if tree_value != live && !(echoed && tree_value == live) {
            state.update(cx, |st, cx| st.set_value(tree_value.clone(), window, cx));
        }

        let mut wrap = div().flex().flex_row().items_center().overflow_hidden();
        if !node.style.contains_key("width") {
            wrap = wrap.w(px(DEFAULT_INPUT_W));
        }
        if !node.style.contains_key("height") {
            wrap = wrap.h(px(num_of(node, "fontSize").unwrap_or(DEFAULT_FONT_SIZE) * LINE_RATIO + 12.0));
        }
        for (k, v) in sorted_style(node) {
            // The field's text/border colors are the component theme's job;
            // height fights the component's own line layout.
            if matches!(k.as_str(), "color" | "fontSize" | "height") {
                continue;
            }
            wrap = apply_style(wrap, k, v);
        }
        if no_shrink && !node.style.contains_key("shrink") {
            wrap = wrap.flex_shrink(0.0);
        }

        let element_id = ElementId::from(SharedString::from(format!("node-{id}")));
        use gpui_component::Sizable;
        let input = gpui_component::input::Input::new(&state)
            .id(element_id)
            .small()
            .appearance(true)
            .bordered(true)
            .flex_grow(1.0)
            .min_w(px(0.0))
            .h_full();
        wrap.child(input).into_any_element()
    }

    /// The `InputState` for an input node: created on first sight with a
    /// placeholder and the current value, and subscribed once to echo user
    /// edits back to the frontend. Everything else (focus reporting) stays
    /// with the focus-handle poll, which already works for these fields
    /// because `InputState` exposes its own `FocusHandle` — we register it in
    /// `focus_handles` so `sync_focus` sees it.
    fn input_state(
        &mut self,
        id: u64,
        node: &Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<gpui_component::input::InputState> {
        if let Some(st) = self.input_states.get(&id) {
            return st.clone();
        }
        let placeholder = node.placeholder.clone().unwrap_or_default();
        let value = node.value.clone().unwrap_or_default();
        let tx = self.event_tx.clone();
        let seed = value.clone();
        let state = cx.new(|cx| {
            let mut st = gpui_component::input::InputState::new(window, cx)
                .placeholder(placeholder);
            st.set_value(seed, window, cx);
            st
        });
        // Subscribe BEFORE the state is ever rendered: `Change` fires on every
        // user edit; `PressEnter` is the commit. Both are echoed with the
        // state's live value — the frontend's controlled setter decides what
        // to keep and pushes it back via `setValue`, where the `input_reported`
        // bookkeeping collapses the echo into a no-op.
        let echo_tx = tx.clone();
        cx.subscribe(&state, move |_this: &mut HostView, st, ev: &gpui_component::input::InputEvent, _cx| {
            let (kind, value) = match ev {
                gpui_component::input::InputEvent::Change => ("input", st.read(_cx).value().to_string()),
                gpui_component::input::InputEvent::PressEnter { .. } => {
                    ("change", st.read(_cx).value().to_string())
                }
                _ => return,
            };
            let msg = json!({ "t": "event", "target": id, "kind": kind, "value": value }).to_string();
            log!("[host] ev {kind} id={id} t={} {msg}", now_ms());
            let _ = echo_tx.send(msg);
        })
        .detach();
        // Route the component's focus handle through the existing poll so
        // `focus`/`blur` events keep flowing without a second mechanism.
        let fh = state.read(cx).focus_handle(cx).clone();
        self.focus_handles.insert(id, fh);
        self.focus_reported.insert(id, false);
        self.input_reported.insert(id, value.clone());
        self.input_states.insert(id, state.clone());
        state
    }

    /// One gpui-component `DatePickerState` per `date` node (mirrors
    /// `input_state`). The state owns the selected date, the open/closed flag
    /// and the calendar entity; the `DatePicker` element built each frame is a
    /// view of it. Picking echoes `DatePickerEvent::Change` up as a `change`
    /// event carrying the date's ISO string.
    fn date_state(
        &mut self,
        id: u64,
        node: &Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<DatePickerState> {
        if let Some(st) = self.date_states.get(&id) {
            return st.clone();
        }
        let value = node.value.clone().unwrap_or_default();
        let tx = self.event_tx.clone();
        let state = cx.new(|cx| {
            let mut st = DatePickerState::new(window, cx);
            if let Some(nd) = parse_date_value(&value) {
                st.set_date(GpuiDate::from(nd), window, cx);
            }
            st
        });
        cx.subscribe(&state, move |_this: &mut HostView, _st, ev: &DatePickerEvent, _cx| {
            let DatePickerEvent::Change(d) = ev;
            let value = d.to_string();
            let msg = json!({ "t": "event", "target": id, "kind": "change", "value": value })
                .to_string();
            log!("[host] ev change(id=date) id={id} t={} {msg}", now_ms());
            let _ = tx.send(msg);
        })
        .detach();
        // Route the component's focus handle through the existing poll so
        // `focus`/`blur` events keep flowing without a second mechanism.
        let fh = state.read(cx).focus_handle(cx).clone();
        self.focus_handles.insert(id, fh);
        self.focus_reported.insert(id, false);
        self.date_states.insert(id, state.clone());
        state
    }

    /// A native-widget node (`checkbox` / `switch` / `button` / `select`):
    /// rendered as a real `gpui_component` control instead of a styled div.
    ///
    /// The controlled contract is the same as `input`'s: the frontend owns
    /// the state and pushes it through `setValue` (checkbox/switch spell it
    /// `"true"`/`"false"`, select carries the option text), and the control's
    /// user interaction is echoed back as an event — the app decides what the
    /// new state is, its setter re-runs the getter, and the next frame
    /// reflects it. The host never flips a bit on its own.
    fn build_native(
        &mut self,
        node: &Node,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let element_id = ElementId::from(SharedString::from(format!("node-{id}")));
        let checked = node.value.as_deref() == Some("true");
        // Label: the node's own text if set, else the concatenated text of its
        // `text`-tag children. JSX spells `<button>＋</button>` as a child text
        // node (mountText), so the button's own `text` field stays empty and
        // the label has to be gathered from the subtree.
        let mut label = node
            .text
            .clone()
            .or_else(|| node.placeholder.clone())
            .unwrap_or_default();
        if label.is_empty() {
            for child_id in &node.children {
                if let Some(child) = self.tree.node(*child_id) {
                    if child.tag == "text" {
                        if let Some(t) = &child.text {
                            label.push_str(t);
                        }
                    }
                }
            }
        }

        match node.tag.as_str() {
            "checkbox" | "switch" => {
                // Switch and Checkbox are distinct types; each branch finishes
                // its own control and shares only the event handler.
                let is_switch = node.tag == "switch";
                let tx = self.event_tx.clone();
                if is_switch {
                    let mut ctl = gpui_component::switch::Switch::new(element_id).checked(checked);
                    if !label.is_empty() {
                        ctl = ctl.label(label);
                    }
                    ctl.on_click(toggle_handler(id, tx)).into_any_element()
                } else {
                    let mut ctl =
                        gpui_component::checkbox::Checkbox::new(element_id).checked(checked);
                    if !label.is_empty() {
                        ctl = ctl.label(label);
                    }
                    ctl.on_click(toggle_handler(id, tx)).into_any_element()
                }
            }
            "button" => {
                let tx = self.event_tx.clone();
                let mut b = gpui_component::button::Button::new(element_id)
                    .when(!label.is_empty(), |b| b.label(label));
                // Protocol styles pass straight through (Button is Styled);
                // the component's own rounded chrome is kept.
                for (k, v) in sorted_style(node) {
                    if k == "borderRadius" {
                        continue;
                    }
                    b = apply_style(b, k, v);
                }
                b.on_click(move |_ev: &ClickEvent, _window, _cx| {
                    let msg = json!({ "t": "event", "target": id, "kind": "click" })
                        .to_string();
                    log!("[host] ev click id={id} t={} {msg}", now_ms());
                    let _ = tx.send(msg);
                })
                .into_any_element()
            }
            "select" => {
                // A plain styled trigger that opens the host's own popup menu
                // is a bigger cut; for now render the options as a button
                // showing the current value and cycle through the list on
                // click — the controlled contract (value down, change up) is
                // identical to the final dropdown.
                let options = node.select_options();
                let current = node
                    .value
                    .as_deref()
                    .map(str::to_string)
                    .unwrap_or_else(|| options.first().cloned().unwrap_or_default());
                let tx = self.event_tx.clone();
                let mut b = gpui_component::button::Button::new(element_id)
                    .when(!current.is_empty(), |b| b.label(current.clone()));
                for (k, v) in sorted_style(node) {
                    if k == "borderRadius" {
                        continue;
                    }
                    b = apply_style(b, k, v);
                }
                b.on_click(move |_ev: &ClickEvent, _window, _cx| {
                        // Cycle: pick the option after the current one.
                        let next = options
                            .iter()
                            .position(|o| *o == current)
                            .map(|i| (i + 1) % options.len())
                            .unwrap_or(0);
                        let value = options.get(next).cloned().unwrap_or_default();
                        let msg = json!({
                            "t": "event", "target": id, "kind": "change",
                            "value": value
                        })
                        .to_string();
                        log!("[host] ev change id={id} t={} {msg}", now_ms());
                        let _ = tx.send(msg);
                    })
                    .into_any_element()
            }
            "date" => {
                // Native date picker: a real gpui-component `time::DatePicker`
                // driven by a per-node `DatePickerState`. Controlled like `input`
                // — `value` is the selected date as an ISO "YYYY-MM-DD" (or ""),
                // the user picks a day, and `DatePickerEvent::Change` echoes it
                // back as a `change` event with that string.
                let state = self.date_state(id, node, window, cx);
                // Controlled push-down: a `setValue` that differs from what the
                // state holds is an external correction — apply it.
                let tree_value = node.value.clone().unwrap_or_default();
                let live = state.read(cx).date().to_string();
                if !tree_value.is_empty() && tree_value != live {
                    if let Some(nd) = parse_date_value(&tree_value) {
                        let date = GpuiDate::from(nd);
                        state.update(cx, |st, cx| st.set_date(date, window, cx));
                    }
                }
                let mut picker = GpuiDatePicker::new(&state);
                if let Some(ph) = &node.placeholder {
                    if !ph.is_empty() {
                        picker = picker.placeholder(ph.clone());
                    }
                }
                picker.appearance(true).cleanable(false).into_any_element()
            }
            _ => div().into_any_element(),
        }
    }

    /// A `canvas` node: a wrapper for placement + interactivity, and the canvas
    /// itself carrying the size and the display list.
    ///
    /// The size is applied to the *canvas*, not the wrapper: a canvas has no
    /// content, so a wrapper sized only by "shrinks to fit" would collapse to
    /// zero. Placement keys (`position`, `top`, `grow`, …) mean nothing on the
    /// canvas — they describe where the *node* goes in its parent.
    fn build_canvas(
        &mut self,
        node: &Node,
        id: u64,
        no_shrink: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let cmds = node.canvas.clone();
        let mut el = canvas(
            move |_bounds, _window, _cx| (),
            move |bounds, _state, window, cx| paint_cmds(&cmds, bounds.origin, window, cx),
        );
        let mut wrap = div().flex().flex_row();
        for (k, v) in sorted_style(node) {
            if is_placement_key(k) {
                wrap = apply_style(wrap, k, v);
            } else {
                el = apply_style(el, k, v);
            }
        }
        if no_shrink && !node.style.contains_key("shrink") {
            wrap = wrap.flex_shrink(0.0);
        }

        let element_id = ElementId::from(SharedString::from(format!("node-{id}")));
        let mut s = wrap.id(element_id).child(el);
        if wants(node, "click") {
            s = s.on_click(cx.listener(move |this, _ev: &ClickEvent, _window, _cx| {
                log!("[host] canvas click id={id} t={}", now_ms());
                this.send_event(id, "click", None);
            }));
        }
        s.into_any_element()
    }

    fn scroll_handle(&mut self, id: u64) -> ScrollHandle {
        if let Some(h) = self.scroll_handles.get(&id) {
            return h.clone();
        }
        let h = ScrollHandle::new();
        self.scroll_handles.insert(id, h.clone());
        // Seed the reported offset so the first frame does not push a spurious
        // "scrolled to 0" event before the user touched anything.
        self.scroll_reported.insert(id, 0.0);
        h
    }

    fn send_event(&self, target: u64, kind: &str, field: Option<(&str, String)>) {
        let msg = match field {
            Some((k, v)) => json!({ "t": "event", "target": target, "kind": kind, k: v }),
            None => json!({ "t": "event", "target": target, "kind": kind }),
        }
        .to_string();
        log!("[host] ev {kind} id={target} t={} {msg}", now_ms());
        let _ = self.event_tx.send(msg);
    }

    /// Focus changes *noticed during render* → one `focus`/`blur` event each.
    ///
    /// Polled rather than hooked: GPUI has no "focus lost" callback, and the
    /// only place that always runs after focus moved is `render`.
    fn sync_focus(&mut self, window: &Window) {
        let ids: Vec<u64> = self.focus_handles.keys().copied().collect();
        for id in ids {
            let Some(fh) = self.focus_handles.get(&id).cloned() else {
                continue;
            };
            let now = fh.is_focused(window);
            let last = self.focus_reported.get(&id).copied().unwrap_or(false);
            if now != last {
                self.focus_reported.insert(id, now);
                if wants_node(&self.tree, id, "blur") || wants_node(&self.tree, id, "focus") {
                    self.send_event(id, if now { "focus" } else { "blur" }, None);
                }
            }
        }
    }

    /// Scroll movements → `scroll` events with the numbers a frontend needs to
    /// draw its own indicator (`top` / `max` / `viewport` / `content`).
    fn sync_scroll(&mut self) {
        let ids: Vec<u64> = self.scroll_handles.keys().copied().collect();
        for id in ids {
            let Some(h) = self.scroll_handles.get(&id) else {
                continue;
            };
            // Distance travelled, not GPUI's ≤ 0 translation: `top` and `max`
            // must share a frame of reference for a frontend to compute
            // `top / max` without knowing which way the host stores it.
            let top = f32::from(-h.offset().y);
            if self.scroll_reported.get(&id).copied() == Some(top) {
                continue;
            }
            self.scroll_reported.insert(id, top);
            if !wants_node(&self.tree, id, "scroll") {
                continue;
            }
            let max = f32::from(h.max_offset().y);
            let viewport = f32::from(h.bounds().size.height);
            let msg = json!({
                "t": "event",
                "target": id,
                "kind": "scroll",
                "top": round1(top),
                "max": round1(max),
                "viewport": round1(viewport),
                "content": round1(viewport + max),
            })
            .to_string();
            log!("[host] ev scroll id={id} t={} {msg}", now_ms());
            let _ = self.event_tx.send(msg);
        }
    }

    /// Drop per-node host state for nodes that left the tree.
    ///
    /// Without this, a `For` list that rebuilds on every keystroke (as the demo's
    /// does) leaks one focus handle and one scroll handle per removed row.
    fn prune_state(&mut self) {
        let live: HashSet<u64> = self.tree.ids().into_iter().collect();
        self.focus_handles.retain(|k, _| live.contains(k));
        self.scroll_handles.retain(|k, _| live.contains(k));
        self.scroll_reported.retain(|k, _| live.contains(k));
        self.focus_reported.retain(|k, _| live.contains(k));
        self.input_states.retain(|k, _| live.contains(k));
        self.input_reported.retain(|k, _| live.contains(k));
        self.date_states.retain(|k, _| live.contains(k));
    }

    /// Push window geometry to the engine when it changed.
    ///
    /// Done from `render` rather than a resize callback because render is the
    /// one place that always runs after a resize *and* has the scale factor
    /// (a DPI change moves `devicePixelRatio` without touching the size).
    fn sync_metrics(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(metrics) = self.metrics.as_ref() else { return };
        let vp = window.viewport_size();
        let mut changed = metrics.set_inner(vp.width.into(), vp.height.into());
        changed |= metrics.set_dpr(window.scale_factor());
        if let Some(display) = cx.primary_display() {
            let b = display.bounds();
            changed |= metrics.set_screen(b.size.width.into(), b.size.height.into());
        }
        if changed {
            let _ = self.event_tx.send(metrics.line());
        }
    }

    /// `alert` / `confirm` modal, drawn by the host.
    ///
    /// Deliberately *not* protocol-driven: a modal is a host concern (it is what
    /// a native message box would be), so the frontend protocol stays free of
    /// it and the frontend cannot paint itself over the dialog.
    fn build_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.dialog.as_ref()?;
        let confirm = dialog.kind.is_confirm();

        let button = |id: &'static str, label: &'static str, primary: bool, answer: i32| {
            let bg = if primary { 0x4f8cff_ff } else { 0x2a3145_ff };
            let fg = if primary { 0xffff_ffff } else { 0x8b93a7_ff };
            div()
                .id(id)
                .px(px(16.0))
                .py(px(7.0))
                .rounded(px(7.0))
                .bg(rgba(bg))
                .text_color(rgba(fg))
                .text_size(px(13.0))
                .cursor_pointer()
                .child(SharedString::from(label))
                .on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                    this.answer_dialog(answer, cx);
                }))
        };

        let mut buttons = div().flex().flex_row().gap(px(10.0)).justify_end();
        if confirm {
            buttons = buttons.child(button("dialog-cancel", "取消", false, 0));
        }
        buttons = buttons.child(button("dialog-ok", "确定", true, 1));

        let card = div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .w(px(400.0))
            .p(px(20.0))
            .rounded(px(12.0))
            .bg(rgba(0x1c2333_ff))
            .border_1()
            .border_color(rgba(0x262d3d_ff))
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(rgba(0x8b93a7_ff))
                    .child(SharedString::from(if confirm { "确认" } else { "提示" })),
            )
            .child(
                div()
                    .text_size(px(14.0))
                    .text_color(rgba(0xe8eaf2_ff))
                    .child(SharedString::from(dialog.message.clone())),
            )
            .child(buttons);

        Some(
            div()
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .w_full()
                .h_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                // scrim: 60% black, and `occlude` keeps clicks off the tree behind
                .bg(rgba(0x000000_99))
                .occlude()
                .child(card)
                .into_any_element(),
        )
    }

    /// Dismiss the modal and hand the answer to the blocked engine thread.
    fn answer_dialog(&mut self, value: i32, cx: &mut Context<Self>) {
        if let Some(d) = self.dialog.take() {
            log!("[host] dialog answered: {} → {value}", if d.kind.is_confirm() { "confirm" } else { "alert" });
            // The receiver is the engine thread parked in `bom::show`. If it is
            // gone the send fails, which is fine — nobody is waiting anymore.
            let _ = d.reply.send(value);
        }
        cx.notify();
    }
}

// ---------------------------------------------------------------------------
// Node helpers & custom painting
// ---------------------------------------------------------------------------

/// Style entries in deterministic application order (HashMap iteration is
/// random; compound keys like `padding` must apply before `paddingX`).
fn sorted_style(node: &Node) -> Vec<(&String, &Value)> {
    let mut pairs: Vec<(&String, &Value)> = node.style.iter().collect();
    pairs.sort_by_key(|(k, _)| style_rank(k));
    pairs
}

/// Shared `on_click` handler for checkbox/switch: echoes the *new* state back
/// to the frontend as a `change` event. The app decides what to keep — the
/// host never flips a bit on its own (same controlled contract as `input`).
fn toggle_handler(
    id: u64,
    tx: mpsc::Sender<String>,
) -> impl Fn(&bool, &mut Window, &mut App) + 'static {
    move |checked: &bool, _window, _cx| {
        let msg = json!({
            "t": "event", "target": id, "kind": "change",
            "value": if *checked { "true" } else { "false" }
        })
        .to_string();
        log!("[host] ev change id={id} t={} {msg}", now_ms());
        let _ = tx.send(msg);
    }
}

fn num_of(node: &Node, key: &str) -> Option<f32> {
    node.style.get(key).and_then(num)
}

/// Parse a `YYYY-MM-DD` string (the `Date` Display form) into a `NaiveDate`.
/// Returns `None` for empty or unparseable strings — the picker's default state
/// is already cleared, so `None` means "don't touch". We never construct a
/// `Date` variant directly (its tuple field is private outside gpui-base); the
/// public `Date::from(NaiveDate)` is the only way to build one here.
fn parse_date_value(s: &str) -> Option<NaiveDate> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() == 3 {
        if let (Ok(y), Ok(m), Ok(d)) = (
            parts[0].parse::<i32>(),
            parts[1].parse::<u32>(),
            parts[2].parse::<u32>(),
        ) {
            return NaiveDate::from_ymd_opt(y, m, d);
        }
    }
    None
}

/// Keys that place a node in its parent rather than describe the node's own
/// box: for a `canvas` node they belong on the wrapper (the canvas must keep
/// its explicit size) — see `build_canvas`.
fn is_placement_key(k: &str) -> bool {
    matches!(
        k,
        "position" | "top" | "left" | "right" | "bottom" | "grow" | "flex" | "shrink"
    )
}

/// Does this node declare the given host event? (`setEvents` is an allow-list:
/// the host attaches only the listeners the frontend asked for.)
fn wants(node: &Node, kind: &str) -> bool {
    node.events.iter().any(|e| e == kind)
}

fn wants_node(tree: &Tree, id: u64, kind: &str) -> bool {
    tree.node(id).map(|n| wants(n, kind)).unwrap_or(false)
}

/// One decimal place, for the numbers in scroll events (a raw f32 prints as
/// `123.00000762939453`, which makes the protocol stream unreadable).
fn round1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

/// Shape + paint one line of canvas text.
fn paint_text(
    text: &str,
    origin: gpui::Point<gpui::Pixels>,
    color: u32,
    size: f32,
    weight: FontWeight,
    window: &mut Window,
    cx: &mut App,
) -> Option<gpui::ShapedLine> {
    // Canvas text is single-line: newlines and tabs become spaces (wrapping
    // would need a layout pass the hand-painted path does not do).
    let text: String = text
        .chars()
        .map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c })
        .collect();
    if text.is_empty() {
        return None;
    }
    // Clone the Arc: shaping borrows the window, painting needs it mutably.
    let system = window.text_system().clone();
    let run = TextRun {
        len: text.len(),
        font: Font {
            weight,
            ..Font::default()
        },
        color: rgba(color).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = system.shape_line(SharedString::from(text), px(size), &[run], None);
    let _ = line.paint(
        origin,
        px(size * LINE_RATIO),
        TextAlign::Left,
        None,
        window,
        cx,
    );
    Some(line)
}

/// Ellipse as a polyline: 48 segments is smooth at the sizes a dashboard
/// canvas sees, and avoids a fill/stroke implemention of our own (`paint_path`
/// already handles both).
fn ellipse_points(cx: f32, cy: f32, rx: f32, ry: f32) -> Vec<(f32, f32)> {
    let n = 48;
    let mut pts = Vec::with_capacity(n);
    for i in 0..n {
        let a = (i as f32) / (n as f32) * std::f32::consts::TAU;
        pts.push((cx + rx * a.cos(), cy + ry * a.sin()));
    }
    pts
}

fn build_path(
    pts: &[(f32, f32)],
    origin: gpui::Point<gpui::Pixels>,
    close: bool,
    stroke: Option<f32>,
) -> Option<gpui::Path<gpui::Pixels>> {
    if pts.len() < 2 {
        return None;
    }
    let mut b = match stroke {
        Some(w) => PathBuilder::stroke(px(w.max(0.5))),
        None => PathBuilder::fill(),
    };
    b.move_to(point(origin.x + px(pts[0].0), origin.y + px(pts[0].1)));
    for p in &pts[1..] {
        b.line_to(point(origin.x + px(p.0), origin.y + px(p.1)));
    }
    if close {
        b.close();
    }
    b.build().ok()
}

/// Walk a `canvas` node's display list. `origin` is the element's content box,
/// so a command's (0,0) is the box's top-left corner, padding excluded — the
/// CSS-pixel convention `ui/src/canvas2d.ts` documents on the JS side.
fn paint_cmds(
    cmds: &[DrawCmd],
    origin: gpui::Point<gpui::Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    for cmd in cmds {
        match cmd {
            DrawCmd::Rect {
                rect,
                radius,
                paint,
            } => {
                if rect.w <= 0.0 || rect.h <= 0.0 {
                    continue;
                }
                let bounds = Bounds {
                    origin: point(origin.x + px(rect.x), origin.y + px(rect.y)),
                    size: size(px(rect.w), px(rect.h)),
                };
                let mut q = fill(bounds, rgba(paint.fill.unwrap_or(0x0000_0000)));
                if *radius > 0.0 {
                    q.corner_radii = Corners::all(px(*radius));
                }
                if let Some(sc) = paint.stroke {
                    // GPUI strokes inside the bounds (like a CSS border), while
                    // canvas centres the stroke on the edge — a half-pixel
                    // difference nobody has ever noticed, and it needs no
                    // second path allocation.
                    q.border_widths = Edges::all(px(paint.line.max(1.0)));
                    q.border_color = rgba(sc).into();
                    q.border_style = BorderStyle::Solid;
                }
                window.paint_quad(q);
            }
            DrawCmd::Ellipse {
                cx: ecx,
                cy: ecy,
                rx,
                ry,
                paint,
            } => {
                if *rx <= 0.0 || *ry <= 0.0 {
                    continue;
                }
                let pts = ellipse_points(*ecx, *ecy, *rx, *ry);
                if let Some(fc) = paint.fill {
                    if let Some(p) = build_path(&pts, origin, true, None) {
                        window.paint_path(p, rgba(fc));
                    }
                }
                if let Some(sc) = paint.stroke {
                    if let Some(p) = build_path(&pts, origin, true, Some(paint.line)) {
                        window.paint_path(p, rgba(sc));
                    }
                }
            }
            DrawCmd::Line {
                x1,
                y1,
                x2,
                y2,
                stroke,
                line,
            } => {
                let pts = [(*x1, *y1), (*x2, *y2)];
                if let Some(p) = build_path(&pts, origin, false, Some(*line)) {
                    window.paint_path(p, rgba(*stroke));
                }
            }
            DrawCmd::Poly { pts, close, paint } => {
                if let Some(fc) = paint.fill {
                    if let Some(p) = build_path(pts, origin, *close, None) {
                        window.paint_path(p, rgba(fc));
                    }
                }
                if let Some(sc) = paint.stroke {
                    if let Some(p) = build_path(pts, origin, *close, Some(paint.line)) {
                        window.paint_path(p, rgba(sc));
                    }
                }
            }
            DrawCmd::Text {
                x,
                y,
                text,
                fill: color,
                size: text_size,
                weight,
            } => {
                paint_text(
                    text,
                    point(origin.x + px(*x), origin.y + px(*y)),
                    *color,
                    *text_size,
                    if weight == "bold" {
                        FontWeight::BOLD
                    } else if weight == "medium" {
                        FontWeight::MEDIUM
                    } else {
                        FontWeight::NORMAL
                    },
                    window,
                    cx,
                );
            }
        }
    }
}

/// The scrollbar: painted by the host, *not* by the frontend.
///
/// GPUI reserves `scrollbar_width` in layout but paints nothing, and the frontend
/// cannot draw it either — the offset lives in the scroll handle, which only the
/// host can read. Overlay (absolute) so it costs no layout width, and computed
/// from `offset / max_offset` on every paint, so it cannot drift from the
/// content it describes.
/// Paint a scroll container's bar.
///
/// The bar is an **absolutely positioned sibling of the scroller**, not a child:
/// as a child it would be scrolled away with the content.
///
/// `inset` keeps the bar *inside* the container. The two-element split means the
/// wrapper owns the box size while the scroller owns the border, so a bar pinned
/// to `right: 0` of the wrapper lands just **outside** the border line — it reads
/// as a bar drawn next to the card rather than nested in it. Insetting by the
/// border width puts it back where a browser would.
fn scrollbar_overlay(handle: ScrollHandle, inset: f32) -> AnyElement {
    canvas(
        move |_bounds, _window, _cx| (),
        move |bounds, _state, window, _cx| {
            let viewport = f32::from(bounds.size.height);
            // GPUI stores the offset as a translation (it is ≤ 0), while the
            // bar wants a distance travelled — the same frame of reference as
            // the `top` reported by `sync_scroll`.
            let offset = f32::from(-handle.offset().y);
            let max = f32::from(handle.max_offset().y);
            let Some((y, thumb_h)) = draw::scrollbar_thumb(
                viewport,
                viewport + max,
                offset,
                viewport,
                SCROLLBAR_MIN_THUMB,
            ) else {
                return;
            };
            let w = f32::from(bounds.size.width);
            let inner = if w > 6.0 { w - 4.0 } else { w };
            let mut q = fill(
                Bounds {
                    origin: point(bounds.origin.x + px(2.0), bounds.origin.y + px(y)),
                    size: size(px(inner), px(thumb_h)),
                },
                rgba(SCROLLBAR_THUMB),
            );
            q.corner_radii = Corners::all(px(3.0));
            window.paint_quad(q);
        },
    )
    .w(px(SCROLLBAR_W))
    .absolute()
    .top(px(inset))
    .right(px(inset))
    .bottom(px(inset))
    .into_any_element()
}

impl Render for HostView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_metrics(window, cx);
        let root = self
            .build_node(0, false, window, cx)
            .unwrap_or_else(|| div().into_any_element());
        let dialog = self.build_dialog(cx);
        // Both are polls, not hooks: GPUI has no focus-lost or scroll callback,
        // and `render` is the one place that always runs after such a change.
        // They run *after* the tree is built so a change detected here is
        // reflected in the same frame.
        self.sync_focus(window);
        self.sync_scroll();
        self.prune_state();
        div()
            .id("host-root")
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x0f1117))
            .child(root)
            // absolute overlay, so it sits above the tree without taking part
            // in the column layout
            .children(dialog)
    }
}

// ---------------------------------------------------------------------------
// JSONL protocol ingress: reader parses mutation batches, writer ships event
// lines. `hello` lines are diagnostics only.
// ---------------------------------------------------------------------------

/// Parse one protocol line and hand the resulting batch to the UI.
///
/// This is THE protocol dialect — every carrier funnels through it, so the
/// child/embedded pipes (`run_ops_reader`) and the injected QuickJS
/// `__hostEmit` function can never drift apart.
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

            application().run(move |cx: &mut App| {
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

            application().run(move |cx: &mut App| {
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
            application().run(move |cx: &mut App| {
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

            application().run(move |cx: &mut App| {
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
) -> gpui::WindowHandle<HostView> {
    // gpui-component's global state (theme, root rendering, input machinery).
    // Idempotent per-process; called once before any window opens.
    gpui_component::init(cx);
    // `init` pins Light mode; the dashboard's palette is dark, so flip the
    // component theme to match — otherwise native buttons come out white-on-
    // white against the dark cards.
    gpui_component::Theme::change(gpui_component::ThemeMode::Dark, None, cx);
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
        .open_window(options, |_, cx| {
            cx.new(|_| HostView {
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
            })
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
    handle: gpui::WindowHandle<HostView>,
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
            let shown = cx.update(|cx| {
                handle.update(cx, |view, _window, cx| {
                    view.dialog = Some(req);
                    cx.notify();
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
        }
    })
    .detach();
}
