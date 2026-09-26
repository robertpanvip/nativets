//! Style application: CSS-ish keys -> GPUI builder calls.
//!
//! `Div` is move-only, so each optional style goes through a helper that takes
//! the element by value and returns it untouched when the value is absent.
//! Shared by the retained-tree host across every element kind (div, canvas,
//! and the gpui-component controls).

use gpui::{prelude::*, px, relative, rgba, rgb, Styled, FontWeight, Hsla, Rgba};
use serde_json::Value;
use crate::tree::{Node, Tree};

pub(crate) fn parse_color(s: &str) -> Option<u32> {
    crate::draw::parse_color(s)
}

pub(crate) fn num(v: &Value) -> Option<f32> {
    v.as_f64().map(|f| f as f32)
}

pub(crate) fn with_px<D: Styled>(d: D, v: &Value, f: impl FnOnce(D, f32) -> D) -> D {
    match num(v) {
        Some(x) => f(d, x),
        None => d,
    }
}

pub(crate) fn with_color<D: Styled>(d: D, v: &Value, f: impl FnOnce(D, u32) -> D) -> D {
    match v.as_str().and_then(parse_color) {
        Some(c) => f(d, c),
        None => d,
    }
}

pub(crate) fn with_pct<D: Styled>(d: D, s: &str, f: impl FnOnce(D, f32) -> D) -> D {
    match s.strip_suffix('%').and_then(|p| p.parse::<f32>().ok()) {
        Some(frac) => f(d, frac / 100.0),
        None => d,
    }
}

pub(crate) fn font_weight_of(v: Option<&Value>) -> FontWeight {
    match v {
        Some(Value::String(s)) if s == "bold" => FontWeight::BOLD,
        Some(Value::String(s)) if s == "semibold" => FontWeight::SEMIBOLD,
        Some(Value::String(s)) if s == "medium" => FontWeight::MEDIUM,
        Some(Value::Number(n)) => FontWeight(n.as_f64().unwrap_or(400.0) as f32),
        _ => FontWeight::NORMAL,
    }
}

pub(crate) fn apply_style<D: Styled>(d: D, key: &str, v: &Value) -> D {
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

pub(crate) fn is_box_key(k: &str) -> bool {
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

pub(crate) fn is_scroll_container(node: &Node) -> bool {
    let overflow = node
        .style
        .get("overflow")
        .or_else(|| node.style.get("overflowY"))
        .and_then(|v| v.as_str());
    overflow == Some("scroll")
}

pub(crate) fn is_transparent(node: &Node) -> bool {
    node.style.get("display").and_then(|v| v.as_str()) == Some("contents")
}

pub(crate) fn scroll_inset(node: &Node) -> f32 {
    num_of(node, "borderWidth").unwrap_or(0.0) + 1.0
}

pub(crate) fn style_rank(k: &str) -> u8 {
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

pub(crate) fn sorted_style(node: &Node) -> Vec<(&String, &Value)> {
    let mut pairs: Vec<(&String, &Value)> = node.style.iter().collect();
    pairs.sort_by_key(|(k, _)| style_rank(k));
    pairs
}

pub(crate) fn num_of(node: &Node, key: &str) -> Option<f32> {
    node.style.get(key).and_then(num)
}

pub(crate) fn is_placement_key(k: &str) -> bool {
    matches!(
        k,
        "position" | "top" | "left" | "right" | "bottom" | "grow" | "flex" | "shrink"
    )
}

pub(crate) fn wants(node: &Node, kind: &str) -> bool {
    node.events.iter().any(|e| e == kind)
}

pub(crate) fn wants_node(tree: &Tree, id: u64, kind: &str) -> bool {
    tree.node(id).map(|n| wants(n, kind)).unwrap_or(false)
}

pub(crate) fn option_index(options: &[String], value: &str) -> Option<usize> {
    if value.is_empty() {
        return None;
    }
    if let Ok(i) = value.parse::<usize>() {
        if i < options.len() {
            return Some(i);
        }
    }
    options.iter().position(|o| o == value)
}

pub(crate) fn parse_hex_color(s: &str) -> Option<gpui::Hsla> {
    let s = s.trim_start_matches('#').trim();
    let bytes = s.as_bytes();
    let _ = bytes;
    let (r, g, b) = match s.len() {
        6 => (
            u8::from_str_radix(&s[0..2], 16).ok()?,
            u8::from_str_radix(&s[2..4], 16).ok()?,
            u8::from_str_radix(&s[4..6], 16).ok()?,
        ),
        8 => (
            u8::from_str_radix(&s[0..2], 16).ok()?,
            u8::from_str_radix(&s[2..4], 16).ok()?,
            u8::from_str_radix(&s[4..6], 16).ok()?,
        ),
        _ => return None,
    };
    let rgba: gpui::Rgba = gpui::rgb(((r as u32) << 16) | ((g as u32) << 8) | (b as u32));
    Some(rgba.into())
}

pub(crate) fn hsla_to_hex(h: gpui::Hsla) -> String {
    let rgba: gpui::Rgba = h.into();
    let r = (rgba.r.clamp(0.0, 1.0) * 255.0).round() as u8;
    let g = (rgba.g.clamp(0.0, 1.0) * 255.0).round() as u8;
    let b = (rgba.b.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", r, g, b)
}
