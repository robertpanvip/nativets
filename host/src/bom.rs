//! Host side of the BOM (Browser Object Model) surface.
//!
//! `bootstrap.js` gives the frontend `window`, `navigator`, `performance`,
//! `crypto`, events, rAF and dialogs. The JS is the whole visible API; this
//! module only provides the four things JS cannot know on its own:
//!
//!   * [`WindowMetrics`] — real window/display geometry (the GPUI thread is the
//!     only writer, the engine thread only reads)
//!   * [`perf_now_ms`]  — a monotonic clock (`Date.now()` is wall clock and
//!     jumps on NTP/DST steps, which is wrong for `performance.now()`)
//!   * [`entropy_hex`]  — the OS CSPRNG behind `crypto.getRandomValues`
//!   * [`DialogRequest`] — a channel for `alert` / `confirm`
//!
//! Deliberately plain atomics + std channels: the engine thread reads metrics
//! and blocks on dialogs, and must never be able to deadlock against the UI
//! thread.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::Instant;

// ---------------------------------------------------------------------------
// window / display metrics
// ---------------------------------------------------------------------------

/// Logical (CSS-pixel) geometry, mirroring `window.innerWidth` & friends.
///
/// Written by the GPUI thread on every render, read by the engine thread.
/// Atomics rather than a `Mutex` so a read can never block or be blocked.
#[derive(Debug)]
pub struct WindowMetrics {
    inner_w: AtomicU32,
    inner_h: AtomicU32,
    /// f32 bit pattern — there is no `AtomicF32`.
    dpr: AtomicU32,
    screen_w: AtomicU32,
    screen_h: AtomicU32,
}

impl WindowMetrics {
    pub fn new(inner_w: f32, inner_h: f32, dpr: f32, screen_w: f32, screen_h: f32) -> Arc<Self> {
        Arc::new(WindowMetrics {
            inner_w: AtomicU32::new(px(inner_w)),
            inner_h: AtomicU32::new(px(inner_h)),
            dpr: AtomicU32::new(dpr.to_bits()),
            screen_w: AtomicU32::new(px(screen_w)),
            screen_h: AtomicU32::new(px(screen_h)),
        })
    }

    pub fn set_inner(&self, w: f32, h: f32) -> bool {
        let (w, h) = (px(w), px(h));
        let changed = self.inner_w.swap(w, Ordering::Relaxed) != w
            || self.inner_h.swap(h, Ordering::Relaxed) != h;
        changed
    }

    pub fn set_dpr(&self, dpr: f32) -> bool {
        let bits = dpr.to_bits();
        self.dpr.swap(bits, Ordering::Relaxed) != bits
    }

    pub fn set_screen(&self, w: f32, h: f32) -> bool {
        let (w, h) = (px(w), px(h));
        self.screen_w.swap(w, Ordering::Relaxed) != w
            || self.screen_h.swap(h, Ordering::Relaxed) != h
    }

    pub fn inner(&self) -> (u32, u32) {
        (
            self.inner_w.load(Ordering::Relaxed),
            self.inner_h.load(Ordering::Relaxed),
        )
    }

    pub fn dpr(&self) -> f32 {
        f32::from_bits(self.dpr.load(Ordering::Relaxed))
    }

    /// Payload of `__hostWindow()`: the boot pull for the JS side.
    pub fn json(&self) -> String {
        let (iw, ih) = self.inner();
        let (sw, sh) = (
            self.screen_w.load(Ordering::Relaxed),
            self.screen_h.load(Ordering::Relaxed),
        );
        format!(
            "{{\"innerWidth\":{iw},\"innerHeight\":{ih},\"dpr\":{},\"screenWidth\":{sw},\"screenHeight\":{sh}}}",
            trim_f32(self.dpr())
        )
    }

    /// Payload pushed on the event channel when the geometry changes; the
    /// bootstrap applies it and fires a `resize` event.
    ///
    /// Short keys because unlike `json()` this travels the live event path.
    pub fn line(&self) -> String {
        let (iw, ih) = self.inner();
        let (sw, sh) = (
            self.screen_w.load(Ordering::Relaxed),
            self.screen_h.load(Ordering::Relaxed),
        );
        format!(
            "{{\"t\":\"bom\",\"w\":{iw},\"h\":{ih},\"dpr\":{},\"sw\":{sw},\"sh\":{sh}}}",
            trim_f32(self.dpr())
        )
    }
}

/// Round to whole pixels: `innerWidth` is an integer in every browser, and
/// fractional values would make the JS side report `1180.0000000001`-style
/// churn on every resize for no benefit.
fn px(v: f32) -> u32 {
    if v.is_finite() && v > 0.0 {
        v.round() as u32
    } else {
        0
    }
}

/// Keep the DPR readable in logs and stable across pushes (`1` not `1.00000`).
fn trim_f32(v: f32) -> String {
    if !v.is_finite() || v <= 0.0 {
        return "1".to_string();
    }
    let s = format!("{:.3}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() { "1".to_string() } else { s.to_string() }
}

// ---------------------------------------------------------------------------
// monotonic clock
// ---------------------------------------------------------------------------

/// Milliseconds since the first call, monotonic.
pub fn perf_now_ms() -> f64 {
    static T0: OnceLock<Instant> = OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

// ---------------------------------------------------------------------------
// entropy
// ---------------------------------------------------------------------------

/// `n` random bytes as a lowercase hex string (2n chars), for
/// `crypto.getRandomValues` / `crypto.randomUUID`.
///
/// Backed by the OS CSPRNG. `getrandom` is already in the dependency graph via
/// gpui, so this adds no new download.
pub fn entropy_hex(n: usize) -> String {
    let mut buf = vec![0u8; n];
    if getrandom::getrandom(&mut buf).is_err() {
        // Only reachable if the platform RNG is unavailable. Fall back to a
        // hashed counter — NOT cryptographically secure, hence the warning;
        // silently returning zeros or a constant would be far worse.
        crate::log_line("[bom] WARNING getrandom failed — crypto.* falls back to a weak source");
        let mut h = std::collections::hash_map::RandomState::new();
        for b in buf.iter_mut() {
            use std::hash::{BuildHasher, Hasher};
            let mut hasher = h.build_hasher();
            hasher.write_u64(perf_now_ms() as u64);
            h = std::collections::hash_map::RandomState::new();
            *b = hasher.finish() as u8;
        }
    }
    let mut s = String::with_capacity(n * 2);
    for b in buf {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0x0f) as u32, 16).unwrap());
    }
    s
}

// ---------------------------------------------------------------------------
// dialogs
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DialogKind {
    Alert,
    Confirm,
}

impl DialogKind {
    /// Unknown kinds degrade to `Alert` — an unknown string must never make the
    /// engine block forever waiting for an answer the UI will not send.
    pub fn parse(s: &str) -> DialogKind {
        match s {
            "confirm" => DialogKind::Confirm,
            _ => DialogKind::Alert,
        }
    }

    pub fn is_confirm(self) -> bool {
        self == DialogKind::Confirm
    }
}

/// One dialog to show. `reply` is the engine's blocking side: whoever dismisses
/// the dialog must send exactly one value (`1` = OK, `0` = cancel).
pub struct DialogRequest {
    pub kind: DialogKind,
    pub message: String,
    pub reply: mpsc::Sender<i32>,
}

/// Ask the UI thread to show a dialog.
///
/// `None` when the UI side is gone (window closed) — the caller must then
/// return immediately rather than block, or the engine would hang forever.
pub fn show(tx: &async_channel::Sender<DialogRequest>, kind: DialogKind, message: String) -> Option<i32> {
    let (reply_tx, reply_rx) = mpsc::channel::<i32>();
    let req = DialogRequest {
        kind,
        message,
        reply: reply_tx,
    };
    if tx.send_blocking(req).is_err() {
        crate::log_line("[bom] dialog channel closed — returning without blocking");
        return None;
    }
    // Blocks until the user answers, which is what a browser does (JS really
    // does stop). No timeout on purpose: a timeout would silently turn a
    // "waiting for the user" state into a wrong answer.
    reply_rx.recv().ok()
}
