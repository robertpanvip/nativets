//! The retained-tree renderer.
//!
//! Walks the retained tree and produces GPUI elements, plus the per-frame sync
//! polls (focus / scroll / prune), the `alert`/`confirm` modal, and the
//! `Render` entry point.

use crate::log;
use crate::now_ms;
use crate::bom::DialogRequest;
use crate::canvas_draw::{paint_cmds, scrollbar_overlay, round1};
use crate::style::{
    apply_style, sorted_style, is_box_key, is_placement_key, is_scroll_container, is_transparent,
    scroll_inset, num_of, wants, wants_node,
};
use crate::{DEFAULT_INPUT_W, DEFAULT_FONT_SIZE, LINE_RATIO, HostView};
use gpui::{
    prelude::*, AnyElement, ClickEvent, Context, Deferred, ElementId, IntoElement, Render,
    ScrollHandle, ScrollWheelEvent, SharedString, Window, canvas, deferred, div, px, rgb, rgba,
};
use gpui_component::Sizable;
use serde_json::json;
use std::collections::HashSet;
use crate::tree::{is_native_tag, Node, Tree};

impl HostView {
    pub(crate) fn build_node(
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
            return Some(self.build_native(&node, id, no_shrink, window, cx));
        }
        Some(self.build_plain(&node, id, no_shrink, window, cx))
    }

    pub(crate) fn build_plain(
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

    pub(crate) fn build_input(
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

        let mut wrap = div().flex().flex_row().items_center();
        if !node.style.contains_key("width") {
            wrap = wrap.w(px(DEFAULT_INPUT_W));
        }
        if !node.style.contains_key("height") {
            wrap = wrap.h(px(num_of(node, "fontSize").unwrap_or(DEFAULT_FONT_SIZE) * LINE_RATIO + 12.0));
        }
        for (k, v) in sorted_style(node) {
            // The wrapper owns the box (it is what the parent flexes) *and* the
            // field repeats the same size, because `Input` computes its own
            // height from its `Size` (`small()` → 22px) and only the protocol
            // value can override that — a 34px-tall style would otherwise sit
            // in a 34px wrapper as a 22px pill. Placement / flex factors stay
            // on the wrapper: they describe where the *node* goes.
            if is_box_key(k) || is_placement_key(k) || k == "alignItems" || k == "justifyContent" {
                wrap = apply_style(wrap, k, v);
            }
        }
        if no_shrink && !node.style.contains_key("shrink") {
            wrap = wrap.flex_shrink(0.0);
        }

        let element_id = ElementId::from(SharedString::from(format!("node-{id}")));
        use gpui_component::Sizable;
        let mut input = gpui_component::input::Input::new(&state)
            .id(element_id)
            .small()
            .appearance(true)
            .bordered(true)
            .flex_grow(1.0)
            .min_w(px(0.0))
            .h_full();
        for (k, v) in sorted_style(node) {
            if is_placement_key(k) {
                continue; // the wrapper places the node (see above)
            }
            input = apply_style(input, k, v);
        }
        wrap.child(input).into_any_element()
    }

    pub(crate) fn build_canvas(
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

    pub(crate) fn scroll_handle(&mut self, id: u64) -> ScrollHandle {
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

    pub(crate) fn send_event(&self, target: u64, kind: &str, field: Option<(&str, String)>) {
        let msg = match field {
            Some((k, v)) => json!({ "t": "event", "target": target, "kind": kind, k: v }),
            None => json!({ "t": "event", "target": target, "kind": kind }),
        }
        .to_string();
        log!("[host] ev {kind} id={target} t={} {msg}", now_ms());
        let _ = self.event_tx.send(msg);
    }

    pub(crate) fn sync_focus(&mut self, window: &Window) {
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

    pub(crate) fn sync_scroll(&mut self) {
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

    pub(crate) fn prune_state(&mut self) {
        let live: HashSet<u64> = self.tree.ids().into_iter().collect();
        self.focus_handles.retain(|k, _| live.contains(k));
        self.scroll_handles.retain(|k, _| live.contains(k));
        self.scroll_reported.retain(|k, _| live.contains(k));
        self.focus_reported.retain(|k, _| live.contains(k));
        self.input_states.retain(|k, _| live.contains(k));
        self.input_reported.retain(|k, _| live.contains(k));
        self.date_states.retain(|k, _| live.contains(k));
        self.slider_states.retain(|k, _| live.contains(k));
        self.slider_reported.retain(|k, _| live.contains(k));
        self.select_states.retain(|k, _| live.contains(k));
        self.select_reported.retain(|k, _| live.contains(k));
        self.textarea_states.retain(|k, _| live.contains(k));
        self.textarea_reported.retain(|k, _| live.contains(k));
        self.combobox_states.retain(|k, _| live.contains(k));
        self.combobox_reported.retain(|k, _| live.contains(k));
        self.color_states.retain(|k, _| live.contains(k));
        self.color_reported.retain(|k, _| live.contains(k));
    }

    pub(crate) fn sync_metrics(&self, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(crate) fn build_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
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

    pub(crate) fn answer_dialog(&mut self, value: i32, cx: &mut Context<Self>) {
        if let Some(d) = self.dialog.take() {
            log!("[host] dialog answered: {} → {value}", if d.kind.is_confirm() { "confirm" } else { "alert" });
            // The receiver is the engine thread parked in `bom::show`. If it is
            // gone the send fails, which is fine — nobody is waiting anymore.
            let _ = d.reply.send(value);
        }
        cx.notify();
    }
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
