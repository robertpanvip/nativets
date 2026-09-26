//! Per-control retained state.
//!
//! One gpui-component state entity per node, plus the subscribe-once echo
//! wiring that turns user edits into protocol events. The host never mutates
//! control state on its own — the frontend owns it and pushes corrections via
//! `setValue`.

use crate::log;
use crate::now_ms;
use crate::style::{hsla_to_hex, num_of, option_index, parse_hex_color};
use crate::{HostSelectState, HostComboboxState, SelectDelegate, HostView};
use gpui::{prelude::*, App, Context, Entity, Focusable, Window, SharedString};
use serde_json::json;
use chrono::NaiveDate;
use gpui_base::Date as GpuiDate;
use gpui_component::date_picker::{DatePickerEvent, DatePickerState};
use gpui_component::slider::{SliderEvent, SliderState};
use gpui_component::select::{SelectEvent, SelectState};
use gpui_component::IndexPath;
use gpui_component::input::{InputState, InputEvent, TextareaState};
use gpui_component::combobox::{ComboboxEvent, ComboboxState};
use gpui_component::color_picker::{ColorPickerEvent, ColorPickerState};
use crate::tree::Node;

impl HostView {
    pub(crate) fn input_state(
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

    pub(crate) fn date_state(
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

    pub(crate) fn slider_state(
        &mut self,
        id: u64,
        node: &Node,
        cx: &mut Context<Self>,
    ) -> Entity<SliderState> {
        if let Some(st) = self.slider_states.get(&id) {
            return st.clone();
        }
        let min = num_of(node, "min").unwrap_or(0.0);
        let max = num_of(node, "max").unwrap_or(100.0);
        let step = num_of(node, "step").unwrap_or(1.0);
        let seed = node
            .value
            .clone()
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(min);
        let tx = self.event_tx.clone();
        let state = cx.new(|_| {
            SliderState::new()
                .min(min)
                .max(max)
                .step(step)
                .default_value(seed)
        });
        cx.subscribe(&state, move |_this: &mut HostView, _st, ev: &SliderEvent, _cx| {
            let value = match ev {
                SliderEvent::Change(v) => ("input", v.end()),
                SliderEvent::Release(v) => ("change", v.end()),
            };
            let text = format_value(value.1);
            let msg = json!({ "t": "event", "target": id, "kind": value.0, "value": text })
                .to_string();
            log!("[host] ev {}(slider) id={id} t={} {msg}", value.0, now_ms());
            let _ = tx.send(msg);
        })
        .detach();
        let live = slider_value_text(&state, cx);
        self.slider_reported.insert(id, live);
        self.slider_states.insert(id, state.clone());
        state
    }

    pub(crate) fn select_state(
        &mut self,
        id: u64,
        node: &Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<HostSelectState> {
        if let Some(st) = self.select_states.get(&id) {
            return st.clone();
        }
        let options: SelectDelegate = node
            .select_options()
            .into_iter()
            .map(SharedString::from)
            .collect();
        let seed = node.value.clone().unwrap_or_default();
        let seed_ix = options
            .iter()
            .position(|o| **o == *seed)
            .map(|row| IndexPath::default().row(row));
        let tx = self.event_tx.clone();
        let state = cx.new(|cx| SelectState::new(options, seed_ix, window, cx));
        cx.subscribe(
            &state,
            move |_this: &mut HostView, _st, ev: &SelectEvent<SelectDelegate>, _cx| {
                let SelectEvent::Confirm(value) = ev;
                let Some(value) = value else { return };
                let msg = json!({
                    "t": "event", "target": id, "kind": "change",
                    "value": value.to_string()
                })
                .to_string();
                log!("[host] ev change(select) id={id} t={} {msg}", now_ms());
                let _ = tx.send(msg);
            },
        )
        .detach();
        // Route the component's focus handle through the existing poll so
        // `focus`/`blur` events keep flowing without a second mechanism.
        let fh = state.read(cx).focus_handle(cx).clone();
        self.focus_handles.insert(id, fh);
        self.focus_reported.insert(id, false);
        let live = select_value_text(&state, cx);
        self.select_reported.insert(id, live);
        self.select_states.insert(id, state.clone());
        state
    }

    pub(crate) fn textarea_state(
        &mut self,
        id: u64,
        node: &Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextareaState> {
        if let Some(st) = self.textarea_states.get(&id) {
            return st.clone();
        }
        let value = node.value.clone().unwrap_or_default();
        let tx = self.event_tx.clone();
        let seed = value.clone();
        let state = cx.new(|cx| {
            let mut st = TextareaState::new(window, cx);
            st.set_value(seed, window, cx);
            st
        });
        cx.subscribe(
            &state,
            move |_this: &mut HostView, st, ev: &gpui_component::input::InputEvent, _cx| {
                let (kind, value) = match ev {
                    gpui_component::input::InputEvent::Change => {
                        ("input", st.read(_cx).value().to_string())
                    }
                    gpui_component::input::InputEvent::PressEnter { .. } => {
                        ("change", st.read(_cx).value().to_string())
                    }
                    _ => return,
                };
                let msg =
                    json!({ "t": "event", "target": id, "kind": kind, "value": value }).to_string();
                log!("[host] ev {kind} id={id} t={} {msg}", now_ms());
                let _ = tx.send(msg);
            },
        )
        .detach();
        let fh = state.read(cx).focus_handle(cx).clone();
        self.focus_handles.insert(id, fh);
        self.focus_reported.insert(id, false);
        self.textarea_reported.insert(id, value.clone());
        self.textarea_states.insert(id, state.clone());
        state
    }

    pub(crate) fn combobox_state(
        &mut self,
        id: u64,
        node: &Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<HostComboboxState> {
        if let Some(st) = self.combobox_states.get(&id) {
            return st.clone();
        }
        let options: Vec<SharedString> = node
            .select_options()
            .into_iter()
            .map(SharedString::from)
            .collect();
        let seed = node.value.clone().unwrap_or_default();
        let seed_idx = option_index(&node.select_options(), &seed);
        let selected = seed_idx
            .map(|i| vec![IndexPath::default().row(i)])
            .unwrap_or_default();
        let tx = self.event_tx.clone();
        let state = cx.new(|cx| ComboboxState::new(options, selected, window, cx));
        cx.subscribe(
            &state,
            move |_this: &mut HostView, _st, ev: &ComboboxEvent<Vec<SharedString>>, _cx| {
                let ComboboxEvent::Confirm(vals) = ev else { return };
                let value = vals.first().map(|v| v.to_string()).unwrap_or_default();
                let msg = json!({
                    "t": "event", "target": id, "kind": "change", "value": value
                })
                .to_string();
                log!("[host] ev change(combobox) id={id} t={} {msg}", now_ms());
                let _ = tx.send(msg);
            },
        )
        .detach();
        let fh = state.read(cx).focus_handle(cx).clone();
        self.focus_handles.insert(id, fh);
        self.focus_reported.insert(id, false);
        self.combobox_reported.insert(id, seed);
        self.combobox_states.insert(id, state.clone());
        state
    }

    pub(crate) fn color_state(
        &mut self,
        id: u64,
        node: &Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ColorPickerState> {
        if let Some(st) = self.color_states.get(&id) {
            return st.clone();
        }
        let value = node.value.clone().unwrap_or_default();
        let tx = self.event_tx.clone();
        let state = cx.new(|cx| ColorPickerState::new(window, cx));
        if let Some(h) = parse_hex_color(&value) {
            state.update(cx, |st, cx| {
                st.set_value(h, window, cx);
            });
        }
        cx.subscribe(
            &state,
            move |_this: &mut HostView, _st, ev: &ColorPickerEvent, _cx| {
                let ColorPickerEvent::Change(Some(h)) = ev else { return };
                let msg = json!({
                    "t": "event", "target": id, "kind": "change",
                    "value": hsla_to_hex(*h)
                })
                .to_string();
                log!("[host] ev change(color) id={id} t={} {msg}", now_ms());
                let _ = tx.send(msg);
            },
        )
        .detach();
        self.color_reported.insert(id, value.clone());
        self.color_states.insert(id, state.clone());
        state
    }
}

pub(crate) fn slider_value_text(state: &Entity<SliderState>, cx: &App) -> String {
    format_value(state.read(cx).value().end())
}

pub(crate) fn select_value_text(state: &Entity<HostSelectState>, cx: &App) -> String {
    state
        .read(cx)
        .selected_value()
        .map(|v| v.to_string())
        .unwrap_or_default()
}

fn format_value(v: f32) -> String {
    let r = (v * 10.0).round() / 10.0;
    if (r - r.trunc()).abs() < f32::EPSILON {
        format!("{}", r as i64)
    } else {
        format!("{r}")
    }
}

pub(crate) fn parse_date_value(s: &str) -> Option<NaiveDate> {
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
