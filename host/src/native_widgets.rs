//! `build_native`: maps each native-tag node to a real gpui-component control.
//!
//! This is the bridge's extension point — add a new native widget by adding an
//! arm to `build_native` and (if stateful) a constructor in `widget_state.rs`.

use crate::log;
use crate::now_ms;
use crate::style::{
    apply_style, sorted_style, is_box_key, is_placement_key, parse_hex_color, hsla_to_hex, num_of,
    option_index, wants,
};
use crate::widget_state::{select_value_text, slider_value_text};
use crate::{
    DEFAULT_INPUT_W, DEFAULT_FONT_SIZE, LINE_RATIO, DEFAULT_SELECT_W, DEFAULT_SLIDER_W,
};
use gpui::{
    prelude::*, AnyElement, App, ClickEvent, ElementId, SharedString, Styled, InteractiveElement,
    Window, div, px,
};
use std::sync::mpsc;
use gpui_component::Sizable;
use gpui_component::Size;
use gpui_component::switch::Switch;
use gpui_component::checkbox::Checkbox;
use gpui_component::button::Button;
use gpui_component::select::Select as GpuiSelect;
use gpui_component::date_picker::DatePicker as GpuiDatePicker;
use gpui_component::input::Textarea;
use gpui_component::combobox::Combobox;
use gpui_component::color_picker::ColorPicker;
use gpui_component::radio::{Radio, RadioGroup};
use gpui_component::tab::{Tab, TabBar};
use gpui_component::pagination::Pagination;
use gpui_component::breadcrumb::{Breadcrumb, BreadcrumbItem};
use gpui_component::alert::Alert;
use gpui_component::badge::Badge;
use gpui_component::tag::{Tag, TagVariant};
use gpui_component::avatar::Avatar;
use gpui_component::separator::Separator;
use gpui_component::skeleton::Skeleton;
use gpui_component::label::Label;
use gpui_component::link::Link;
use gpui_component::collapsible::Collapsible;
use gpui_component::progress::Progress;
use gpui_component::spinner::Spinner;
use gpui_component::slider::Slider as GpuiSlider;
use gpui_base::Date as GpuiDate;
use serde_json::json;
use crate::tree::Node;
use crate::HostView;
use crate::widget_state::parse_date_value;

impl HostView {
    pub(crate) fn build_native(
        &mut self,
        node: &Node,
        id: u64,
        no_shrink: bool,
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
                // its own control and shares only the event handler. Both are
                // `Styled` and refine the protocol style onto the outer row
                // (the one carrying the label) *after* their themed chrome, so
                // a protocol `color` / `fontSize` / `width` really lands. There
                // is no wrapper here (each control owns its own box), hence no
                // key is skipped.
                let is_switch = node.tag == "switch";
                let tx = self.event_tx.clone();
                if is_switch {
                    let mut ctl = gpui_component::switch::Switch::new(element_id).checked(checked);
                    if !label.is_empty() {
                        ctl = ctl.label(label);
                    }
                    for (k, v) in sorted_style(node) {
                        ctl = apply_style(ctl, k, v);
                    }
                    ctl.on_click(toggle_handler(id, tx)).into_any_element()
                } else {
                    let mut ctl =
                        gpui_component::checkbox::Checkbox::new(element_id).checked(checked);
                    if !label.is_empty() {
                        ctl = ctl.label(label);
                    }
                    for (k, v) in sorted_style(node) {
                        ctl = apply_style(ctl, k, v);
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
                // Real gpui-component `Select`: trigger with the selected
                // option, dropdown anchored under it (Root overlay + deferred
                // Positioner), outside-click and Escape close it — the whole
                // interaction is the component's, not ours. Controlled like
                // the other stateful widgets: a `setValue` that differs from
                // the live selection (and from our last echo) is pushed down
                // via `set_selected_value`; a user pick comes back as
                // `SelectEvent::Confirm` (see `select_state`).
                let state = self.select_state(id, node, window, cx);
                let tree_value = node.value.clone().unwrap_or_default();
                let live = select_value_text(&state, cx);
                let echoed = self.select_reported.get(&id).map(|s| *s == live).unwrap_or(false);
                if tree_value != live && !(echoed && tree_value == live) && !tree_value.is_empty()
                {
                    let wanted = SharedString::from(tree_value.clone());
                    state.update(cx, |st, cx| {
                        st.set_selected_value(&wanted, window, cx);
                    });
                    self.select_reported.insert(id, tree_value.clone());
                }
                let mut sel = GpuiSelect::new(&state).appearance(true);
                // Placement keys belong on the wrapper (mirrors `build_canvas`):
                // the parent flexes the wrapper, the trigger fills it.
                let mut wrap = div().flex().flex_row().items_center();
                for (k, v) in sorted_style(node) {
                    if is_placement_key(k) {
                        wrap = apply_style(wrap, k, v);
                    } else if k == "width" {
                        // Width drives the wrapper; the dropdown matches the
                        // trigger width on its own (menu_width defaults to the
                        // trigger's measured bounds).
                        wrap = apply_style(wrap, k, v);
                    } else {
                        // color/fontSize flow through: Select is Styled and
                        // refines its trigger.
                        sel = apply_style(sel, k, v);
                    }
                }
                if !node.style.contains_key("width") {
                    wrap = wrap.w(px(DEFAULT_SELECT_W));
                }
                wrap.child(sel).into_any_element()
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
                picker = picker.appearance(true).cleanable(false);
                // Same split as `build_input`: the wrapper owns the box (a
                // picker with no definite width anchors its calendar to a
                // zero-width trigger, which reads as "the popup is in the
                // wrong place"), the picker gets everything visual.
                let mut wrap = div().flex().flex_row().items_center();
                for (k, v) in sorted_style(node) {
                    if is_box_key(k) || is_placement_key(k) || k == "alignItems" {
                        wrap = apply_style(wrap, k, v);
                    } else {
                        picker = apply_style(picker, k, v);
                    }
                }
                if !node.style.contains_key("width") {
                    // No explicit width → fill the parent column, mirroring the
                    // `Input` label wrapper's `width: "full"`. Without this the
                    // picker lands at the default 220 and reads as narrower than
                    // the field stacked above it.
                    wrap = wrap.w_full().flex_grow(1.0);
                }
                wrap.child(picker).into_any_element()
            }
            "progress" => {
                // Stateless display widget: `value` (0..=100) drives the bar,
                // `loading: 1` flips it into the indeterminate animation. The
                // bar re-reads every prop per frame, so no host-side state is
                // needed — the controlled contract is just `setValue` down.
                let value = node
                    .value
                    .as_deref()
                    .and_then(|v| v.parse::<f32>().ok())
                    .unwrap_or(0.0);
                use gpui_component::Sizable;
                let mut bar = gpui_component::progress::Progress::new(element_id)
                    .value(value)
                    .loading(num_of(node, "loading").unwrap_or(0.0) != 0.0)
                    .w_full();
                // `height` maps to the bar's thickness (the pill), not the box
                // height — a progress bar with a `height: 12` style expects a
                // 12px-thick bar, which is what Size::Size spells.
                if let Some(h) = num_of(node, "height") {
                    bar = bar.with_size(gpui_component::Size::Size(px(h.clamp(2.0, 20.0))));
                }
                // Placement keys belong on the wrapper (mirrors `build_canvas`):
                // the parent flexes the wrapper, the bar fills it.
                let mut wrap = div().flex().flex_row().items_center();
                for (k, v) in sorted_style(node) {
                    if is_placement_key(k) || k == "height" {
                        wrap = apply_style(wrap, k, v);
                    } else {
                        bar = apply_style(bar, k, v);
                    }
                }
                wrap.child(bar).into_any_element()
            }
            "spinner" => {
                // Fully stateless: the rotation is a gpui animation. `size`
                // style scales the icon, `color` tints it.
                let mut sp = gpui_component::spinner::Spinner::new();
                if let Some(s) = num_of(node, "fontSize") {
                    use gpui_component::Sizable;
                    sp = sp.with_size(gpui_component::Size::Size(px(s)));
                }
                let mut wrap = div().flex().flex_row().items_center().justify_center();
                for (k, v) in sorted_style(node) {
                    wrap = apply_style(wrap, k, v);
                }
                wrap.child(sp).into_any_element()
            }
            "rating" => {
                // Star rating; the state lives in the widget's keyed window
                // state, seeded from `value` per render when it changes. A
                // click emits the new value as `change`.
                let value = node
                    .value
                    .as_deref()
                    .and_then(|v| v.parse::<f32>().ok())
                    .unwrap_or(0.0) as usize;
                let max = num_of(node, "max").unwrap_or(5.0).max(1.0) as usize;
                let tx = self.event_tx.clone();
                let mut r = gpui_component::rating::Rating::new(element_id)
                    .value(value)
                    .max(max);
                if wants(node, "change") || wants(node, "input") {
                    r = r.on_click(move |new: &usize, _window, _cx| {
                        let msg = json!({
                            "t": "event", "target": id, "kind": "change",
                            "value": new.to_string()
                        })
                        .to_string();
                        log!("[host] ev change(rating) id={id} t={} {msg}", now_ms());
                        let _ = tx.send(msg);
                    });
                }
                r.into_any_element()
            }
            // ---- Extended bridge: more gpui-component widgets ----
            "textarea" => {
                // Real gpui-component `Textarea`: a multi-line `InputState`.
                // Controlled like `input` — `value` is the text, edits echo
                // `input`/`change` back (see `textarea_state`).
                let state = self.textarea_state(id, node, window, cx);
                let tree_value = node.value.clone().unwrap_or_default();
                let live = state.read(cx).value().to_string();
                let echoed =
                    self.textarea_reported.get(&id).map(|s| *s == live).unwrap_or(false);
                if tree_value != live && !(echoed && tree_value == live) {
                    state.update(cx, |st, cx| st.set_value(tree_value.clone(), window, cx));
                }
                let mut wrap = div().flex().flex_row().items_center();
                if !node.style.contains_key("width") {
                    wrap = wrap.w(px(DEFAULT_INPUT_W));
                }
                if !node.style.contains_key("height") {
                    wrap = wrap.h(px(
                        num_of(node, "fontSize").unwrap_or(DEFAULT_FONT_SIZE) * LINE_RATIO + 30.0,
                    ));
                }
                for (k, v) in sorted_style(node) {
                    if is_box_key(k)
                        || is_placement_key(k)
                        || k == "alignItems"
                        || k == "justifyContent"
                    {
                        wrap = apply_style(wrap, k, v);
                    }
                }
                if no_shrink && !node.style.contains_key("shrink") {
                    wrap = wrap.flex_shrink(0.0);
                }
                let mut ta = Textarea::new(&state)
                    .appearance(true)
                    .bordered(true)
                    .flex_grow(1.0)
                    .min_w(px(0.0))
                    .h_full();
                for (k, v) in sorted_style(node) {
                    if is_placement_key(k) {
                        continue;
                    }
                    ta = apply_style(ta, k, v);
                }
                wrap.child(ta).into_any_element()
            }
            "combobox" => {
                // Searchable single-select (real gpui-component `Combobox`).
                // Controlled like `select`: `value` is the chosen option text, a
                // pick echoes `ComboboxEvent::Confirm` (see `combobox_state`).
                let state = self.combobox_state(id, node, window, cx);
                let tree_value = node.value.clone().unwrap_or_default();
                let live = state
                    .read(cx)
                    .selected_value()
                    .map(|v| v.to_string())
                    .unwrap_or_default();
                let echoed =
                    self.combobox_reported.get(&id).map(|s| *s == live).unwrap_or(false);
                if tree_value != live && !(echoed && tree_value == live) && !tree_value.is_empty() {
                    let wanted = SharedString::from(tree_value.clone());
                    state.update(cx, |st, cx| {
                        st.set_selected_values(&[wanted], window, cx);
                    });
                    self.combobox_reported.insert(id, tree_value.clone());
                }
                let mut sel = Combobox::new(&state).appearance(true).cleanable(false);
                let mut wrap = div().flex().flex_row().items_center();
                for (k, v) in sorted_style(node) {
                    if is_placement_key(k) || k == "width" {
                        wrap = apply_style(wrap, k, v);
                    } else if k == "color" || k == "fontSize" {
                        sel = apply_style(sel, k, v);
                    }
                }
                if !node.style.contains_key("width") {
                    wrap = wrap.w(px(DEFAULT_SELECT_W));
                }
                wrap.child(sel).into_any_element()
            }
            "colorpicker" => {
                // Real gpui-component `ColorPicker`; `value` is `#rrggbb`.
                // Controlled like the other stateful widgets (see `color_state`):
                // only re-apply when the frontend value *changes* (tracked via
                // `color_reported`), so a non-lossless `#rrggbb` ↔ `Hsla` round
                // trip can never spin `setValue` every frame.
                let state = self.color_state(id, node, window, cx);
                let tree_value = node.value.clone().unwrap_or_default();
                let reported = self
                    .color_reported
                    .get(&id)
                    .cloned()
                    .unwrap_or_default();
                if tree_value != reported && !tree_value.is_empty() {
                    if let Some(h) = parse_hex_color(&tree_value) {
                        state.update(cx, |st, cx| st.set_value(h, window, cx));
                        self.color_reported.insert(id, tree_value.clone());
                    }
                }
                ColorPicker::new(&state).into_any_element()
            }
            "radio" => {
                // Radio group: `options` become the choices, `value` is the
                // selected index (or label). A click echoes the chosen index as a
                // `change` event; the app owns the selection.
                let options = node.select_options();
                let idx = option_index(&options, &node.value.clone().unwrap_or_default());
                let mut group = RadioGroup::new(element_id).selected_index(idx);
                let tx = self.event_tx.clone();
                group = group.on_click(move |i: &usize, _window, _cx| {
                    let msg = json!({
                        "t": "event", "target": id, "kind": "change", "value": i.to_string()
                    })
                    .to_string();
                    log!("[host] ev change(radio) id={id} t={} {msg}", now_ms());
                    let _ = tx.send(msg);
                });
                for (i, opt) in options.iter().enumerate() {
                    group = group.child(
                        Radio::new(ElementId::from(SharedString::from(format!("node-{id}-r{i}"))))
                            .label(opt.clone()),
                    );
                }
                group.into_any_element()
            }
            "tabs" => {
                // Tab bar: `options` become the tab labels, `value` is the
                // selected index. A click echoes the chosen index as `change`.
                let options = node.select_options();
                let idx = option_index(&options, &node.value.clone().unwrap_or_default())
                    .unwrap_or(0);
                let mut bar = TabBar::new(element_id).selected_index(idx);
                let tx = self.event_tx.clone();
                bar = bar.on_click(move |i: &usize, _window, _cx| {
                    let msg = json!({
                        "t": "event", "target": id, "kind": "change", "value": i.to_string()
                    })
                    .to_string();
                    log!("[host] ev change(tabs) id={id} t={} {msg}", now_ms());
                    let _ = tx.send(msg);
                });
                for opt in options.iter() {
                    bar = bar.child(Tab::new().label(opt.clone()));
                }
                bar.into_any_element()
            }
            "pagination" => {
                // Pager: `total` (style) is the page count, `value` is the
                // current 1-based page. A click echoes the target page as
                // `change`.
                let total = num_of(node, "total")
                    .or_else(|| Some(node.select_options().len() as f32))
                    .unwrap_or(1.0)
                    .max(1.0) as usize;
                let page = node
                    .value
                    .as_deref()
                    .and_then(|v| v.parse::<usize>().ok())
                    .filter(|p| *p >= 1 && *p <= total)
                    .unwrap_or(1);
                let mut pg = Pagination::new(element_id)
                    .total_pages(total)
                    .current_page(page);
                let tx = self.event_tx.clone();
                pg = pg.on_click(move |p: &usize, _window, _cx| {
                    let msg = json!({
                        "t": "event", "target": id, "kind": "change", "value": p.to_string()
                    })
                    .to_string();
                    log!("[host] ev change(pagination) id={id} t={} {msg}", now_ms());
                    let _ = tx.send(msg);
                });
                pg.into_any_element()
            }
            "breadcrumb" => {
                // Breadcrumb: `options` become the crumbs. A click echoes the
                // clicked crumb index as `change`.
                let options = node.select_options();
                let tx = self.event_tx.clone();
                let mut bc = Breadcrumb::new();
                for (i, opt) in options.iter().enumerate() {
                    let tx = tx.clone();
                    bc = bc.child(
                        BreadcrumbItem::new(opt.clone()).on_click(
                            move |_ev: &ClickEvent, _window, _cx| {
                                let msg = json!({
                                    "t": "event", "target": id, "kind": "change",
                                    "value": i.to_string()
                                })
                                .to_string();
                                log!("[host] ev change(breadcrumb) id={id} t={} {msg}", now_ms());
                                let _ = tx.send(msg);
                            },
                        ),
                    );
                }
                bc.into_any_element()
            }
            "alert" => {
                // Banner; `text` is the message, an optional `close` event wires
                // the dismiss button.
                let message = SharedString::from(node.text.clone().unwrap_or_default());
                let mut al = Alert::new(element_id, message);
                if wants(node, "close") {
                    let tx = self.event_tx.clone();
                    al = al.on_close(move |_ev: &ClickEvent, _window, _cx| {
                        let msg =
                            json!({ "t": "event", "target": id, "kind": "close" }).to_string();
                        log!("[host] ev close(alert) id={id} t={} {msg}", now_ms());
                        let _ = tx.send(msg);
                    });
                }
                al.into_any_element()
            }
            "badge" => {
                let mut b = Badge::new();
                for child in self.build_children(id, no_shrink, window, cx) {
                    b = b.child(child);
                }
                b.into_any_element()
            }
            "tag" => {
                let mut t = Tag::new();
                if let Some(v) = node.style.get("variant").and_then(|v| v.as_str()) {
                    t = match v {
                        "secondary" => t.with_variant(gpui_component::tag::TagVariant::Secondary),
                        "danger" => t.with_variant(gpui_component::tag::TagVariant::Danger),
                        "success" => t.with_variant(gpui_component::tag::TagVariant::Success),
                        "warning" => t.with_variant(gpui_component::tag::TagVariant::Warning),
                        "info" => t.with_variant(gpui_component::tag::TagVariant::Info),
                        _ => t,
                    };
                }
                for child in self.build_children(id, no_shrink, window, cx) {
                    t = t.child(child);
                }
                t.into_any_element()
            }
            "avatar" => {
                let mut av = Avatar::new();
                if let Some(name) = node.text.clone() {
                    if !name.is_empty() {
                        av = av.name(name);
                    }
                }
                av.into_any_element()
            }
            "separator" => {
                let vertical = node
                    .style
                    .get("orientation")
                    .and_then(|v| v.as_str())
                    .or_else(|| node.style.get("direction").and_then(|v| v.as_str()))
                    == Some("vertical");
                if vertical {
                    Separator::vertical()
                } else {
                    Separator::horizontal()
                }
                .into_any_element()
            }
            "skeleton" => {
                let mut wrap = div().h(px(16.0)).w_full();
                for (k, v) in sorted_style(node) {
                    if is_box_key(k) || is_placement_key(k) {
                        wrap = apply_style(wrap, k, v);
                    }
                }
                wrap.child(Skeleton::new()).into_any_element()
            }
            "label" => {
                Label::new(node.text.clone().unwrap_or_default()).into_any_element()
            }
            "link" => {
                let mut l = Link::new(element_id);
                if let Some(href) = node.style.get("href").and_then(|v| v.as_str()) {
                    l = l.href(href);
                }
                if wants(node, "click") {
                    let tx = self.event_tx.clone();
                    l = l.on_click(move |_ev: &ClickEvent, _window, _cx| {
                        let msg =
                            json!({ "t": "event", "target": id, "kind": "click" }).to_string();
                        log!("[host] ev click(link) id={id} t={} {msg}", now_ms());
                        let _ = tx.send(msg);
                    });
                }
                for child in self.build_children(id, no_shrink, window, cx) {
                    l = l.child(child);
                }
                l.into_any_element()
            }
            "collapsible" => {
                // Controlled expand/collapse: `value` is "true"/"false". The
                // node's children become the revealed content.
                let open = node.value.as_deref() == Some("true");
                let mut c = Collapsible::new().open(open);
                let mut content = div().flex().flex_col();
                for child in self.build_children(id, no_shrink, window, cx) {
                    content = content.child(child);
                }
                c = c.content(content);
                c.into_any_element()
            }
            "slider" => {
                // Real drag slider bound to a per-node `SliderState`. min/max/
                // step ride the style map; `value` is the thumb position as a
                // number string. Dragging echoes `input` per tick and `change`
                // on release (see `slider_state`).
                let state = self.slider_state(id, node, cx);
                // Controlled push-down: an external `setValue` that differs
                // from the live value (and from our last echo) is applied.
                // During a drag the host's own echo arrives while the drag is
                // still moving, so comparing against `slider_reported` keeps
                // the thumb from fighting the pointer.
                let tree_value = node.value.clone().unwrap_or_default();
                let live = slider_value_text(&state, cx);
                let echoed = self.slider_reported.get(&id).map(|s| *s == live).unwrap_or(false);
                if tree_value != live && !(echoed && tree_value == live) {
                    if let Some(v) = tree_value.parse::<f32>().ok() {
                        state.update(cx, |st, cx| {
                            let clamped = st.min_value().max(v.min(st.max_value()));
                            let stepped = if st.step_value() > 0.0 {
                                let steps = ((clamped - st.min_value()) / st.step_value()).round();
                                st.min_value() + steps * st.step_value()
                            } else {
                                clamped
                            };
                            st.set_value(stepped, window, cx);
                        });
                        self.slider_reported.insert(id, tree_value.clone());
                    }
                }
                let s = GpuiSlider::new(&state).w_full();
                let mut wrap = div().flex().flex_row().items_center();
                if !node.style.contains_key("width") && !node.style.contains_key("grow") {
                    wrap = wrap.w(px(DEFAULT_SLIDER_W));
                }
                for (k, v) in sorted_style(node) {
                    if matches!(k.as_str(), "min" | "max" | "step") {
                        continue; // scale keys: creation-time (see `slider_state`)
                    }
                    wrap = apply_style(wrap, k, v);
                }
                wrap.child(s).into_any_element()
            }
            _ => div().into_any_element(),
        }
    }

    pub(crate) fn build_children(
        &mut self,
        id: u64,
        no_shrink: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let child_ids = self
            .tree
            .node(id)
            .map(|n| n.children.clone())
            .unwrap_or_default();
        let mut out = Vec::with_capacity(child_ids.len());
        for cid in child_ids {
            if let Some(el) = self.build_node(cid, no_shrink, window, cx) {
                out.push(el);
            }
        }
        out
    }
}

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
