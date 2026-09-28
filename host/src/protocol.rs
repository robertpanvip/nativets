//! Structured host → frontend event vocabulary (the ABI half of the protocol).
//!
//! GPUI-side code (render, native widgets, per-control state) constructs an
//! [`OutEvent`] directly — no `json!`, no string building. What happens next
//! depends on the backend:
//!
//! * **quickjs** (in-process engine): the event rides the engine queue as the
//!   struct itself and is handed to a JS callback with typed arguments
//!   (`__hostEventSink(kind, target, …)` — see `quickjs.rs::pump`). Zero JSON
//!   in either direction; serialization is skipped entirely.
//! * **child / perry-embedded / scriptc** (out-of-process or host-pull
//!   engines): [`OutEvent::to_line`] is the ONE serialization point back to
//!   the JSONL line the frontends already parse (`io-core::handleLine`).
//!
//! Keeping the enum here (rather than strings in flight) means the emit sites
//! cannot drift from the wire format: the field set is checked by the
//! compiler, and `to_line` is the single place that spells it out.

use serde_json::json;

/// Scroll event payload — the four numbers a frontend needs to render its own
/// scrollbars / infinite lists. `top` and `max` share a frame of reference
/// (distance travelled, not GPUI's ≤ 0 translation) — see `render.rs`.
#[derive(Debug, Clone)]
pub struct Scroll {
    pub top: f32,
    pub max: f32,
    pub viewport: f32,
    pub content: f32,
}

/// One host → frontend message. Every variant today is an event or a BOM
/// geometry push; `{"t":"now"}` (host wall clock) is scriptc-only and never
/// flows through this channel.
#[derive(Debug, Clone)]
pub enum OutEvent {
    /// A user interaction / control state event (`{"t":"event",…}`).
    Ev {
        target: u64,
        kind: String,
        /// `value` events (input/textarea/select/date/slider/rating/…).
        value: Option<String>,
        /// Scroll events carry geometry instead of a value.
        scroll: Option<Scroll>,
    },
    /// BOM geometry push (`{"t":"bom",…}`) — applied by `bootstrap.js` and
    /// surfaced as a `resize` event.
    Bom {
        w: u32,
        h: u32,
        dpr: f32,
        sw: u32,
        sh: u32,
    },
}

impl OutEvent {
    /// Event with no payload (click / focus / blur / close).
    pub fn plain(target: u64, kind: &str) -> OutEvent {
        OutEvent::Ev {
            target,
            kind: kind.to_string(),
            value: None,
            scroll: None,
        }
    }

    /// Event carrying a string value (input / change).
    pub fn value(target: u64, kind: &str, value: impl Into<String>) -> OutEvent {
        OutEvent::Ev {
            target,
            kind: kind.to_string(),
            value: Some(value.into()),
            scroll: None,
        }
    }

    /// Scroll event with rounded geometry (rounding stays at the call site).
    pub fn scroll(top: f32, max: f32, viewport: f32, content: f32, target: u64) -> OutEvent {
        OutEvent::Ev {
            target,
            kind: "scroll".to_string(),
            value: None,
            scroll: Some(Scroll {
                top,
                max,
                viewport,
                content,
            }),
        }
    }

    /// BOM geometry push.
    pub fn bom(w: u32, h: u32, dpr: f32, sw: u32, sh: u32) -> OutEvent {
        OutEvent::Bom { w, h, dpr, sw, sh }
    }

    /// Short human-readable form for the event log (`[host] ev …` lines the
    /// E2E harness greps). Payload values included; scroll geometry summarized.
    pub fn log_tag(&self) -> String {
        match self {
            OutEvent::Ev {
                target,
                kind,
                value: Some(v),
                ..
            } => format!("ev {kind} id={target} value={v:?}"),
            OutEvent::Ev { target, kind, .. } => format!("ev {kind} id={target}"),
            OutEvent::Bom { w, h, .. } => format!("bom {w}x{h}"),
        }
    }

    /// The JSONL line for out-of-process backends. This is the single
    /// serialization point — the field set here is the wire contract that
    /// `io-core.ts::handleLine` parses.
    pub fn to_line(&self) -> String {
        match self {
            OutEvent::Ev {
                target,
                kind,
                value,
                scroll,
            } => match (value, scroll) {
                (Some(v), _) => json!({
                    "t": "event", "target": target, "kind": kind, "value": v
                })
                .to_string(),
                (None, Some(s)) => json!({
                    "t": "event", "target": target, "kind": "scroll",
                    "top": s.top, "max": s.max,
                    "viewport": s.viewport, "content": s.content
                })
                .to_string(),
                (None, None) => json!({
                    "t": "event", "target": target, "kind": kind
                })
                .to_string(),
            },
            OutEvent::Bom { w, h, dpr, sw, sh } => json!({
                "t": "bom", "w": w, "h": h, "dpr": dpr, "sw": sw, "sh": sh
            })
            .to_string(),
        }
    }
}
