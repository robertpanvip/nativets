//! Drawing vocabulary shared by canvas nodes and host-drawn scrollbars.
//!
//! The frontend never paints; it ships a **display list**. `ui/src/canvas2d.ts`
//! mimics `CanvasRenderingContext2D` and lowers each call to one of the five
//! primitives below, the host walks the list and calls gpui's low-level paint
//! API. Two consequences worth stating:
//!
//!   * the wire format is tiny and stable, so a canvas redraw is one
//!     `setCanvas` op instead of a subtree rebuild;
//!   * the host stays the only thing that knows about pixels, fonts and GPUI,
//!     which is what lets the same frontend run on a backend with no canvas at
//!     all (the Perry one simply never receives a `setCanvas`).
//!
//! Coordinates are logical (CSS) pixels relative to the element's content box,
//! y down — the convention a browser canvas has inside a padded parent.
//!
//! Everything here is pure data + arithmetic (no GPUI types), so it is unit
//! tested by `cargo test` without a window.

use serde_json::Value;

/// Cap on commands per canvas. A display list is rebuilt on every redraw, so a
/// runaway loop in the frontend must not be able to grow host memory without
/// bound — this is a protocol-level budget, not a rendering limit.
pub const MAX_CMDS: usize = 8192;

/// A filled/stroked shape's paint inputs. A shape may have neither (a no-op
/// path, which a browser also allows) — hence `Option` rather than a default.
#[derive(Debug, Clone, PartialEq)]
pub struct Paint {
    pub fill: Option<u32>,
    pub stroke: Option<u32>,
    /// Stroke width in px; ignored when `stroke` is `None`.
    pub line: f32,
}

impl Paint {
    fn new(fill: Option<u32>, stroke: Option<u32>, line: f32) -> Paint {
        Paint { fill, stroke, line }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    Rect {
        rect: RectF,
        radius: f32,
        paint: Paint,
    },
    /// Circles are the `rx == ry` case; `arc()` with a partial sweep is lowered
    /// to [`Cmd::Poly`] by the frontend, so one primitive covers both.
    Ellipse {
        cx: f32,
        cy: f32,
        rx: f32,
        ry: f32,
        paint: Paint,
    },
    Line {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        stroke: u32,
        line: f32,
    },
    Poly {
        pts: Vec<(f32, f32)>,
        close: bool,
        paint: Paint,
    },
    Text {
        x: f32,
        y: f32,
        text: String,
        fill: u32,
        size: f32,
        weight: String,
    },
}

// ---------------------------------------------------------------------------
// colors
// ---------------------------------------------------------------------------

/// `#rgb` / `#rrggbb` / `#rrggbbaa` -> `0xRRGGBBAA`.
///
/// A canvas color may also arrive as a bare number (`0xff0000`) or as a
/// `rgb(...)`/`rgba(...)` string, because that is what people paste into a
/// `ctx.fillStyle` in real code. Anything unparsable is `None` and the
/// enclosing command simply loses that channel.
pub fn parse_color(s: &str) -> Option<u32> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        return match hex.len() {
            3 => {
                // #abc -> #aabbcc
                let mut out = 0u32;
                for c in hex.chars() {
                    let d = c.to_digit(16)?;
                    out = (out << 8) | (d * 17);
                }
                Some((out << 8) | 0xFF)
            }
            6 => u32::from_str_radix(hex, 16).ok().map(|v| (v << 8) | 0xFF),
            8 => u32::from_str_radix(hex, 16).ok(),
            _ => None,
        };
    }
    if let Some(rest) = s.strip_prefix("rgb(").or_else(|| s.strip_prefix("rgba(")) {
        let inner = rest.strip_suffix(')')?;
        let parts: Vec<&str> = inner.split(',').map(|p| p.trim()).collect();
        if parts.len() < 3 {
            return None;
        }
        let mut ch = [0u32; 4];
        ch[3] = 255;
        for i in 0..parts.len().min(4) {
            let v: f32 = parts[i].parse().ok()?;
            ch[i] = if i == 3 {
                // alpha may be 0..1 (CSS) or 0..255 (sloppy callers)
                let a = if v <= 1.0 { v * 255.0 } else { v };
                a.round().clamp(0.0, 255.0) as u32
            } else {
                v.round().clamp(0.0, 255.0) as u32
            };
        }
        return Some((ch[0] << 24) | (ch[1] << 16) | (ch[2] << 8) | ch[3]);
    }
    // bare hex without '#'
    u32::from_str_radix(s, 16).ok().map(|v| {
        if s.len() <= 6 { (v << 8) | 0xFF } else { v }
    })
}

/// Scale a color's alpha channel — the host's equivalent of `ctx.globalAlpha`.
pub fn with_alpha(color: u32, alpha: f32) -> u32 {
    let a = (color & 0xFF) as f32 * alpha.clamp(0.0, 1.0);
    (color & 0xFFFF_FF00) | (a.round().clamp(0.0, 255.0) as u32)
}

// ---------------------------------------------------------------------------
// parsing
// ---------------------------------------------------------------------------

fn f32_of(v: Option<&Value>, fallback: f32) -> f32 {
    match v {
        Some(Value::Number(n)) => n.as_f64().map(|f| f as f32).unwrap_or(fallback),
        // numbers also come as strings from hand-built op streams
        Some(Value::String(s)) => s.parse::<f32>().unwrap_or(fallback),
        _ => fallback,
    }
}

fn color_of(v: Option<&Value>) -> Option<u32> {
    match v {
        Some(Value::String(s)) => parse_color(s),
        Some(Value::Number(n)) => n.as_u64().map(|n| n as u32),
        _ => None,
    }
}

/// Parse one display list. Returns the parsed commands and how many entries
/// were rejected — the caller logs the count so a frontend emitting nonsense
/// shows up as a number in the host log rather than as a silently empty canvas.
pub fn parse_cmds(v: &Value) -> (Vec<Cmd>, usize) {
    let Some(arr) = v.as_array() else {
        return (Vec::new(), 0);
    };
    let mut out: Vec<Cmd> = Vec::with_capacity(arr.len().min(MAX_CMDS));
    let mut skipped = 0usize;
    for item in arr.iter() {
        if out.len() >= MAX_CMDS {
            skipped += 1;
            continue;
        }
        match parse_cmd(item) {
            Some(c) => out.push(c),
            None => skipped += 1,
        }
    }
    (out, skipped)
}

fn parse_cmd(v: &Value) -> Option<Cmd> {
    let o = v.as_object()?;
    let alpha = f32_of(o.get("a"), 1.0);
    let paint = || {
        Paint::new(
            color_of(o.get("fill")).map(|c| with_alpha(c, alpha)),
            color_of(o.get("stroke")).map(|c| with_alpha(c, alpha)),
            f32_of(o.get("line"), 1.0).max(0.0),
        )
    };
    match o.get("k")?.as_str()? {
        "rect" => Some(Cmd::Rect {
            rect: RectF {
                x: f32_of(o.get("x"), 0.0),
                y: f32_of(o.get("y"), 0.0),
                w: f32_of(o.get("w"), 0.0),
                h: f32_of(o.get("h"), 0.0),
            },
            radius: f32_of(o.get("r"), 0.0).max(0.0),
            paint: paint(),
        }),
        "ellipse" => Some(Cmd::Ellipse {
            cx: f32_of(o.get("cx"), 0.0),
            cy: f32_of(o.get("cy"), 0.0),
            rx: f32_of(o.get("rx"), 0.0).max(0.0),
            ry: f32_of(o.get("ry"), 0.0).max(0.0),
            paint: paint(),
        }),
        "line" => {
            // A line has no fill channel by definition; keeping `stroke` a plain
            // colour (not Paint) makes that unrepresentable rather than ignored.
            let stroke = color_of(o.get("stroke")).map(|c| with_alpha(c, alpha))?;
            Some(Cmd::Line {
                x1: f32_of(o.get("x1"), 0.0),
                y1: f32_of(o.get("y1"), 0.0),
                x2: f32_of(o.get("x2"), 0.0),
                y2: f32_of(o.get("y2"), 0.0),
                stroke,
                line: f32_of(o.get("line"), 1.0).max(0.0),
            })
        }
        "poly" => {
            let arr = o.get("pts")?.as_array()?;
            let mut pts: Vec<(f32, f32)> = Vec::with_capacity(arr.len());
            for p in arr.iter() {
                let pair = p.as_array()?;
                if pair.len() < 2 {
                    return None;
                }
                pts.push((f32_of(pair.first(), 0.0), f32_of(pair.get(1), 0.0)));
            }
            if pts.len() < 2 {
                return None;
            }
            Some(Cmd::Poly {
                pts,
                close: o.get("close").and_then(|c| c.as_bool()).unwrap_or(false),
                paint: paint(),
            })
        }
        "text" => {
            let fill = color_of(o.get("fill")).map(|c| with_alpha(c, alpha))?;
            Some(Cmd::Text {
                x: f32_of(o.get("x"), 0.0),
                y: f32_of(o.get("y"), 0.0),
                text: o.get("t")?.as_str()?.to_string(),
                fill,
                size: f32_of(o.get("size"), 13.0).max(1.0),
                weight: o
                    .get("weight")
                    .and_then(|w| w.as_str())
                    .unwrap_or("normal")
                    .to_string(),
            })
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// scrollbar geometry
// ---------------------------------------------------------------------------

/// Where the scrollbar thumb goes, in track-local coordinates.
///
/// `None` when there is nothing to scroll (`content <= viewport`), which is the
/// signal to skip painting the bar entirely — a permanently visible bar on a
/// short list is the classic tell of a hand-rolled scroll container.
///
/// `min_thumb` matters: with `thumb = track * viewport/content` a list of 500
/// items in a 200px viewport yields a 0.4px sliver, which is neither visible
/// nor draggable.
pub fn scrollbar_thumb(
    viewport: f32,
    content: f32,
    offset: f32,
    track: f32,
    min_thumb: f32,
) -> Option<(f32, f32)> {
    if viewport <= 0.0 || track <= 0.0 || content <= viewport + 0.5 {
        return None;
    }
    let thumb = (track * (viewport / content)).max(min_thumb.min(track));
    let travel = (track - thumb).max(0.0);
    let max_offset = (content - viewport).max(0.0);
    let progress = if max_offset <= 0.0 {
        0.0
    } else {
        (offset / max_offset).clamp(0.0, 1.0)
    };
    Some((progress * travel, thumb))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_hex_forms() {
        assert_eq!(parse_color("#fff"), Some(0xFFFF_FFFF));
        assert_eq!(parse_color("#4f8cff"), Some(0x4F8C_FFFF));
        assert_eq!(parse_color("#4f8cff80"), Some(0x4F8C_FF80));
        assert_eq!(parse_color("4f8cff"), Some(0x4F8C_FFFF));
        assert_eq!(parse_color("#gg0000"), None);
    }

    #[test]
    fn parses_css_rgb_forms() {
        assert_eq!(parse_color("rgb(255, 0, 0)"), Some(0xFF00_00FF));
        assert_eq!(parse_color("rgba(0,255,0,0.5)"), Some(0x00FF_0080));
        // 0..255 alpha (what people actually pass when they mean 128)
        assert_eq!(parse_color("rgba(0,0,255,128)"), Some(0x0000_FF80));
    }

    #[test]
    fn alpha_scales_only_the_alpha_channel() {
        assert_eq!(with_alpha(0x4F8C_FFFF, 0.5), 0x4F8C_FF80);
        assert_eq!(with_alpha(0x4F8C_FF00, 1.0), 0x4F8C_FF00);
        assert_eq!(with_alpha(0x4F8C_FFFF, 2.0), 0x4F8C_FFFF);
    }

    #[test]
    fn parses_rect_with_radius_and_paint() {
        let (cmds, skipped) = parse_cmds(&json!([
            {"k":"rect","x":1,"y":2,"w":30,"h":10,"r":4,"fill":"#4f8cff","stroke":"#ffffff","line":2}
        ]));
        assert_eq!(skipped, 0);
        assert_eq!(
            cmds,
            vec![Cmd::Rect {
                rect: RectF { x: 1.0, y: 2.0, w: 30.0, h: 10.0 },
                radius: 4.0,
                paint: Paint::new(Some(0x4F8C_FFFF), Some(0xFFFF_FFFF), 2.0),
            }]
        );
    }

    #[test]
    fn applies_global_alpha_to_every_channel() {
        let (cmds, _) = parse_cmds(&json!([
            {"k":"ellipse","cx":0,"cy":0,"rx":5,"ry":5,"fill":"#000000","a":0.25}
        ]));
        match &cmds[0] {
            Cmd::Ellipse { paint, .. } => assert_eq!(paint.fill, Some(0x0000_0040)),
            other => panic!("wrong cmd: {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_and_malformed_entries() {
        let (cmds, skipped) = parse_cmds(&json!([
            {"k":"nope"},
            {"k":"line","x1":0,"y1":0,"x2":1,"y2":1},          // no stroke
            {"k":"poly","pts":[[0,0]]},                        // < 2 points
            {"k":"text","x":0,"y":0,"fill":"#fff"},            // no t
            {"k":"line","x1":0,"y1":0,"x2":1,"y2":1,"stroke":"#fff"}
        ]));
        assert_eq!(cmds.len(), 1);
        assert_eq!(skipped, 4);
    }

    #[test]
    fn clamps_cmd_count_to_the_protocol_budget() {
        let many: Vec<Value> = (0..MAX_CMDS + 10)
            .map(|i| json!({"k":"rect","x":i,"y":0,"w":1,"h":1,"fill":"#ffffff"}))
            .collect();
        let (cmds, skipped) = parse_cmds(&Value::Array(many));
        assert_eq!(cmds.len(), MAX_CMDS);
        assert_eq!(skipped, 10);
    }

    #[test]
    fn non_array_yields_nothing() {
        let (cmds, skipped) = parse_cmds(&json!({"k":"rect"}));
        assert!(cmds.is_empty());
        assert_eq!(skipped, 0);
    }

    #[test]
    fn no_thumb_when_content_fits() {
        assert_eq!(scrollbar_thumb(300.0, 200.0, 0.0, 300.0, 24.0), None);
        // exactly-equal content is also "fits" (0.5px tolerance for float slack)
        assert_eq!(scrollbar_thumb(300.0, 300.0, 0.0, 300.0, 24.0), None);
    }

    #[test]
    fn thumb_tracks_progress_along_the_track() {
        // 300px viewport, 900px content, 300px track ⇒ thumb is 1/3 of the track
        let (y0, h) = scrollbar_thumb(300.0, 900.0, 0.0, 300.0, 24.0).unwrap();
        assert_eq!(y0, 0.0);
        assert_eq!(h, 100.0);
        // fully scrolled ⇒ thumb ends exactly at the track's end
        let (y1, h1) = scrollbar_thumb(300.0, 900.0, 600.0, 300.0, 24.0).unwrap();
        assert_eq!(h1, 100.0);
        assert_eq!(y1, 200.0);
        // half way
        let (ym, _) = scrollbar_thumb(300.0, 900.0, 300.0, 300.0, 24.0).unwrap();
        assert_eq!(ym, 100.0);
    }

    #[test]
    fn thumb_never_shrinks_below_the_minimum_or_leaves_the_track() {
        // 20 items in view out of 5000 ⇒ raw ratio would be a sliver
        let (_, h) = scrollbar_thumb(200.0, 50_000.0, 0.0, 200.0, 24.0).unwrap();
        assert_eq!(h, 24.0);
        // out-of-range offsets clamp instead of painting outside the track
        let (y, h2) = scrollbar_thumb(200.0, 400.0, 99_999.0, 200.0, 24.0).unwrap();
        assert_eq!(h2, 100.0);
        assert_eq!(y, 100.0);
        let (y_neg, _) = scrollbar_thumb(200.0, 400.0, -50.0, 200.0, 24.0).unwrap();
        assert_eq!(y_neg, 0.0);
    }
}
