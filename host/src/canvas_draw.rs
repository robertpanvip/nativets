//! Canvas display-list painting: shapes + text, and the host-drawn scrollbar.
//!
//! The frontend ships a list of `DrawCmd`s; the host shapes + paints them onto
//! a GPUI canvas. The scrollbar is painted here too, because the scroll offset
//! lives in a `ScrollHandle` only the host can read.

use gpui::{
    prelude::*, AnyElement, App, Bounds, BorderStyle, Corners, Edges, Fill, Font, FontWeight, Path,
    PathBuilder, Point, ScrollHandle, ShapedLine, SharedString, TextAlign, TextRun, Window, canvas,
    fill, point, px, rgba, size,
};
use crate::draw::Cmd as DrawCmd;
use crate::{LINE_RATIO, SCROLLBAR_W, SCROLLBAR_MIN_THUMB, SCROLLBAR_THUMB};

pub(crate) fn round1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

pub(crate) fn paint_text(
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

pub(crate) fn ellipse_points(cx: f32, cy: f32, rx: f32, ry: f32) -> Vec<(f32, f32)> {
    let n = 48;
    let mut pts = Vec::with_capacity(n);
    for i in 0..n {
        let a = (i as f32) / (n as f32) * std::f32::consts::TAU;
        pts.push((cx + rx * a.cos(), cy + ry * a.sin()));
    }
    pts
}

pub(crate) fn build_path(
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

pub(crate) fn paint_cmds(
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

pub(crate) fn scrollbar_overlay(handle: ScrollHandle, inset: f32) -> AnyElement {
    canvas(
        move |_bounds, _window, _cx| (),
        move |bounds, _state, window, _cx| {
            let viewport = f32::from(bounds.size.height);
            // GPUI stores the offset as a translation (it is ≤ 0), while the
            // bar wants a distance travelled — the same frame of reference as
            // the `top` reported by `sync_scroll`.
            let offset = f32::from(-handle.offset().y);
            let max = f32::from(handle.max_offset().y);
            let Some((y, thumb_h)) = crate::draw::scrollbar_thumb(
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
