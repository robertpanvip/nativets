//! Retained UI tree on the host side.
//!
//! The TypeScript frontend (compiled by Perry to a native binary) sends
//! mutation operations as JSON lines over stdout; this module stores the
//! resulting element tree and applies each batch. Rendering walks the tree
//! and builds GPUI elements every frame (immediate-mode underneath).

use serde_json::{Map, Value};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum Op {
    Create {
        id: u64,
        tag: String,
        text: Option<String>,
    },
    SetText {
        id: u64,
        text: String,
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
        }
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
                Op::Create { id, tag, text } => {
                    let mut n = Node::new(tag.clone());
                    n.text = text.clone();
                    self.nodes.insert(*id, n);
                }
                Op::SetText { id, text } => {
                    if let Some(n) = self.nodes.get_mut(id) {
                        n.text = Some(text.clone());
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
}
