//! Retained UI tree on the host side.
//!
//! The TypeScript frontend (compiled by Perry to a native binary) sends
//! mutation operations as JSON lines over stdout; this module stores the
//! resulting element tree and applies each batch. Rendering walks the tree
//! and builds GPUI elements every frame (immediate-mode underneath).

use crate::draw;
use serde_json::{Map, Value};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum Op {
    Create {
        id: u64,
        tag: String,
        text: Option<String>,
        /// Text-field hint, `input` nodes only (see `Node::placeholder`).
        placeholder: Option<String>,
        /// Native `select`: the option list, fixed at creation.
        #[allow(dead_code)]
        options: Vec<String>,
    },
    SetText {
        id: u64,
        text: String,
    },
    /// Text-field value, `input` nodes only.
    ///
    /// A separate op from `setText` on purpose: `text` is *content* the host
    /// renders as-is, `value` is *state* the host both renders and edits. Once
    /// the two share an op the host cannot tell a frontend echo (which must not
    /// move the caret) from an external update (which must).
    SetValue {
        id: u64,
        value: String,
    },
    /// Replace a `canvas` node's display list (see `draw::Cmd`).
    SetCanvas {
        id: u64,
        cmds: Vec<draw::Cmd>,
    },
    SetStyle {
        id: u64,
        style: Map<String, Value>,
    },
    SetEvents {
        id: u64,
        events: Vec<String>,
    },
    Append {
        id: u64,
        parent: u64,
    },
    Remove {
        id: u64,
    },
    Clear {
        id: u64,
    },
    SetTitle {
        title: String,
    },
}

pub fn parse_op(v: &Value) -> Option<Op> {
    let obj = v.as_object()?;
    let id = obj.get("id").and_then(|x| x.as_u64());
    match obj.get("op").and_then(|x| x.as_str())? {
        "create" => Some(Op::Create {
            id: id?,
            tag: obj
                .get("tag")
                .and_then(|x| x.as_str())
                .unwrap_or("div")
                .to_string(),
            text: obj.get("text").and_then(|x| x.as_str()).map(String::from),
            placeholder: obj
                .get("placeholder")
                .and_then(|x| x.as_str())
                .map(String::from),
            options: obj
                .get("options")
                .and_then(|x| x.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|o| o.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
        }),
        // 容错：前端若漏了 String 收敛，setText 的 text 会是 JSON number
        // （数字 signal 直传）。与其整条丢弃，不如收成字符串。
        "setText" => {
            let t = obj.get("text")?;
            let text = match t {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => return None,
            };
            Some(Op::SetText { id: id?, text })
        }
        // Same tolerance as setText: a number would mean an unconverted signal.
        "setValue" => {
            let v = obj.get("value")?;
            let value = match v {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => return None,
            };
            Some(Op::SetValue { id: id?, value })
        }
        "setCanvas" => {
            let (cmds, skipped) = draw::parse_cmds(obj.get("cmds")?);
            if skipped > 0 {
                crate::log_line(&format!(
                    "[host] WARNING canvas id={:?} skipped {} malformed cmds",
                    id, skipped
                ));
            }
            Some(Op::SetCanvas { id: id?, cmds })
        }
        "setStyle" => Some(Op::SetStyle {
            id: id?,
            style: obj.get("style")?.as_object()?.clone(),
        }),
        "setEvents" => Some(Op::SetEvents {
            id: id?,
            events: obj
                .get("events")
                .and_then(|x| x.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|e| e.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
        }),
        "append" => Some(Op::Append {
            id: id?,
            parent: obj.get("parent").and_then(|x| x.as_u64())?,
        }),
        "remove" => Some(Op::Remove { id: id? }),
        "clear" => Some(Op::Clear { id: id? }),
        "setTitle" => Some(Op::SetTitle {
            title: obj.get("title").and_then(|x| x.as_str())?.to_string(),
        }),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub struct Node {
    /// Retained for protocol fidelity and debugging only. GPUI has no notion of
    /// an HTML tag — every node renders as `div()`, and semantics live entirely
    /// in `style`. Keeping the tag makes the retained tree legible when dumped
    /// and leaves room for a future per-tag mapping.
    #[allow(dead_code)]
    pub tag: String,
    pub text: Option<String>,
    pub style: Map<String, Value>,
    pub events: Vec<String>,
    pub children: Vec<u64>,
    pub parent: Option<u64>,
    /// Text-field state (`tag == "input"`).
    ///
    /// The **frontend owns the value** (it is a controlled component: the host
    /// echoes every edit back as an event and the app decides what to keep),
    /// while the **host owns the caret** — it is the only side that knows what
    /// the user typed. See `main.rs::input_key` for the edit rules.
    pub value: Option<String>,
    pub placeholder: Option<String>,
    /// Caret position as a *character* index (not a byte offset): the value can
    /// be CJK, where neither bytes nor UTF-16 units match what a user counts.
    pub caret: usize,
    /// Display list for `tag == "canvas"` (see `draw::Cmd`).
    pub canvas: Vec<draw::Cmd>,
    /// Native `select`: the option list, fixed at creation.
    pub options: Vec<String>,
}

impl Node {
    fn new(tag: String) -> Self {
        Node {
            tag,
            text: None,
            style: Map::new(),
            events: Vec::new(),
            children: Vec::new(),
            parent: None,
            value: None,
            placeholder: None,
            caret: 0,
            canvas: Vec::new(),
            options: Vec::new(),
        }
    }

    /// Is this a text field? Kept as one predicate so `build_node`, the key
    /// handler and the dump can never disagree about what an input is.
    pub fn is_input(&self) -> bool {
        self.tag == "input"
    }

    /// Is this one of the native gpui-component widget tags?
    #[allow(dead_code)]
    pub fn is_native(&self) -> bool {
        is_native_tag(&self.tag)
    }

    /// The `select` option list (empty for every other tag).
    pub fn select_options(&self) -> Vec<String> {
        self.options.clone()
    }

    pub fn is_canvas(&self) -> bool {
        self.tag == "canvas"
    }
}

pub struct Tree {
    nodes: HashMap<u64, Node>,
}

impl Tree {
    pub fn new() -> Self {
        let mut nodes = HashMap::new();
        // id 0 is the implicit root container.
        nodes.insert(0u64, Node::new("div".to_string()));
        Tree { nodes }
    }

    /// Applies a batch; returns a window title if a `setTitle` op was present.
    pub fn apply(&mut self, ops: &[Op]) -> Option<String> {
        let mut title = None;
        for op in ops {
            match op {
                Op::Create { id, tag, text, placeholder, options } => {
                    let mut n = Node::new(tag.clone());
                    n.text = text.clone();
                    n.placeholder = placeholder.clone();
                    n.options = options.clone();
                    if tag == "input" {
                        n.value = Some(text.clone().unwrap_or_default());
                    }
                    self.nodes.insert(*id, n);
                }
                Op::SetText { id, text } => {
                    if let Some(n) = self.nodes.get_mut(id) {
                        n.text = Some(text.clone());
                    }
                }
                Op::SetValue { id, value } => {
                    if let Some(n) = self.nodes.get_mut(id) {
                        // Only move the caret when the value really changed.
                        // A controlled component echoes every keystroke back as
                        // `setValue`, and snapping the caret to the end on each
                        // echo would make editing mid-string impossible.
                        let changed = n.value.as_deref() != Some(value.as_str());
                        n.value = Some(value.clone());
                        if changed {
                            n.caret = value.chars().count();
                        }
                    }
                }
                Op::SetCanvas { id, cmds } => {
                    if let Some(n) = self.nodes.get_mut(id) {
                        n.canvas = cmds.clone();
                    }
                }
                Op::SetStyle { id, style } => {
                    if let Some(n) = self.nodes.get_mut(id) {
                        n.style = style.clone();
                    }
                }
                Op::SetEvents { id, events } => {
                    if let Some(n) = self.nodes.get_mut(id) {
                        n.events = events.clone();
                    }
                }
                Op::Append { id, parent } => {
                    if !self.nodes.contains_key(id) {
                        continue;
                    }
                    // detach from current parent first
                    if let Some(old_parent) = self.nodes.get(id).and_then(|n| n.parent) {
                        if let Some(p) = self.nodes.get_mut(&old_parent) {
                            p.children.retain(|c| c != id);
                        }
                    }
                    self.nodes.get_mut(id).unwrap().parent = Some(*parent);
                    if let Some(p) = self.nodes.get_mut(parent) {
                        if !p.children.contains(id) {
                            p.children.push(*id);
                        }
                    }
                }
                Op::Remove { id } => {
                    self.remove_subtree(*id);
                }
                Op::Clear { id } => {
                    let children = self
                        .nodes
                        .get(id)
                        .map(|n| n.children.clone())
                        .unwrap_or_default();
                    for c in children {
                        self.remove_subtree(c);
                    }
                    if let Some(n) = self.nodes.get_mut(id) {
                        n.children.clear();
                    }
                }
                Op::SetTitle { title: t } => {
                    title = Some(t.clone());
                }
            }
        }
        title
    }

    fn remove_subtree(&mut self, id: u64) {
        let children = self
            .nodes
            .get(&id)
            .map(|n| n.children.clone())
            .unwrap_or_default();
        for c in children {
            self.remove_subtree(c);
        }
        if let Some(parent) = self.nodes.get(&id).and_then(|n| n.parent) {
            if let Some(p) = self.nodes.get_mut(&parent) {
                p.children.retain(|c| *c != id);
            }
        }
        self.nodes.remove(&id);
    }

    pub fn node(&self, id: u64) -> Option<&Node> {
        self.nodes.get(&id)
    }

    /// Every live node id. Used to prune host-side per-node state (focus
    /// handles, scroll handles) that would otherwise leak for removed nodes.
    pub fn ids(&self) -> Vec<u64> {
        self.nodes.keys().copied().collect()
    }

    /// Flat, parent-first dump of the retained tree.
    ///
    /// Diagnostic only (see `GPUI_TS_DUMP_TREE` in the host): when a frontend
    /// shows less than it should, dumping what actually got retained is the
    /// fastest way to tell "the frontend never built it" from "it is built but
    /// invisible". Sorting children by insertion order — not by id — matters,
    /// because ids are assigned depth-first and reveal nothing about layout.
    pub fn dump(&self) -> String {
        let mut out = String::new();
        self.dump_from(0, 0, &mut out);
        out
    }

    fn dump_from(&self, id: u64, depth: usize, out: &mut String) {
        let Some(n) = self.nodes.get(&id) else { return };
        out.push_str(&"  ".repeat(depth));
        out.push('#');
        out.push_str(&id.to_string());
        out.push(' ');
        out.push_str(&n.tag);
        if n.is_input() {
            // Value *and* caret: a field whose caret sits at the wrong index
            // looks identical to a correct one in a screenshot.
            out.push_str(&format!(
                " value={:?} caret={} placeholder={:?}",
                n.value.as_deref().unwrap_or(""),
                n.caret,
                n.placeholder.as_deref().unwrap_or("")
            ));
        }
        if n.is_canvas() {
            out.push_str(&format!(" cmds={}", n.canvas.len()));
        }
        if !n.events.is_empty() {
            out.push_str(&format!(" events={:?}", n.events));
        }
        if let Some(t) = &n.text {
            // Text is the only bit worth reading; truncate hard so multi-byte
            // CJK can't blow the line up.
            let t: String = t.chars().take(28).collect();
            out.push_str(" \"");
            out.push_str(&t);
            out.push('"');
        }
        out.push('\n');
        for c in &n.children {
            self.dump_from(*c, depth + 1, out);
        }
    }
}

/// Which tags render as native `gpui_component` controls (see
/// `main.rs::build_native`). Everything else stays a styled `div`.
pub fn is_native_tag(tag: &str) -> bool {
    matches!(
        tag,
        "checkbox" | "switch" | "button" | "select" | "date" | "progress" | "slider" | "spinner"
            | "rating"
    )
}
