//! The bridge between the CDP engine threads and the GPUI main thread.
//!
//! `CdpShared` is created once at boot (quickjs mode) and cloned into every
//! CDP thread. It carries:
//!
//! * a request queue — the tree is owned by `HostView` on the GPUI main
//!   thread, so reads proxy onto the main thread via `with_tree` and are
//!   served by `drain_requests` at frame start (called from `render`). A
//!   mutex around the tree would either block frames or need full snapshots;
//!   the request hop is simpler and CDP traffic is rare.
//! * an ops sender — `CSS.setStyleTexts` injects `Op::SetStyle` here, exactly
//!   like the JS frontend does, so the change flows through `drain_ops` and
//!   lands in the live tree within one frame.
//! * a QuickJS eval mailbox — `Runtime.evaluate` hands `(expr, reply)` over;
//!   the engine thread picks it up on its next tick.
//!
//! When the proxy hop cannot be served (window closed / app exiting), the
//! accessor errors out and CDP replies with a protocol error.

use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use crate::tree::{Op, Tree};

/// One in-flight CDP request, resolved by the GPUI main thread.
enum Req {
    /// Read-only tree access (closure runs on the main thread, result hops
    /// back through the channel).
    Tree(Box<dyn FnOnce(&Tree) + Send>),
}

/// Shared CDP state. `Arc`-cloned into every CDP thread.
pub struct CdpShared {
    reqs: Mutex<Vec<Req>>,
    /// ops injector — batches land in the shared ops channel and are applied
    /// by the normal `drain_ops` path within one frame.
    pub(crate) ops_tx: async_channel::Sender<Vec<Op>>,
    /// QuickJS eval mailbox: (expression, reply channel). The reply carries
    /// JSON *text* — the engine stringifies inside the context (an rquickjs
    /// value can never cross a thread), and cdp.rs parses it back.
    pub(crate) eval_tx: Mutex<Option<Sender<(String, Sender<String>)>>>,
    /// "A request is waiting" signal → the GPUI-side pump (`spawn_cdp_pump`).
    /// `None` until the window side attaches; tree reads then fall back to the
    /// per-frame render drain.
    notify: Mutex<Option<async_channel::Sender<()>>>,
    /// Window title for the /json discovery listing.
    pub(crate) title: Mutex<String>,
}

impl CdpShared {
    pub fn new(ops_tx: async_channel::Sender<Vec<Op>>) -> Arc<Self> {
        Arc::new(CdpShared {
            reqs: Mutex::new(Vec::new()),
            ops_tx,
            eval_tx: Mutex::new(None),
            notify: Mutex::new(None),
            title: Mutex::new(String::from("nativets × GPUI")),
        })
    }

    /// GPUI side registers the wake-up channel (called once at window setup).
    pub fn attach_notify(&self, tx: async_channel::Sender<()>) {
        *self.notify.lock().unwrap() = Some(tx);
    }

    /// Run a read-only closure over the tree on the GPUI main thread. The
    /// request is served by whichever main-thread consumer gets there first:
    /// the `spawn_cdp_pump` wake (signalled below — the usual path) or the
    /// per-frame `render` drain (fallback). Returns `Err` when the main
    /// thread is gone (app shutting down) or slow (>2s).
    pub fn with_tree<R: Send + 'static>(
        &self,
        f: impl FnOnce(&Tree) -> R + Send + 'static,
    ) -> Result<R, String> {
        let (tx, rx): (Sender<R>, Receiver<R>) = std::sync::mpsc::channel();
        self.reqs.lock().unwrap().push(Req::Tree(Box::new(move |tree| {
            let _ = tx.send(f(tree));
        })));
        // Wake the pump *after* the request is visible in the queue, so the
        // drain it triggers is guaranteed to see this request.
        if let Some(n) = self.notify.lock().unwrap().as_ref() {
            let _ = n.try_send(());
        }
        rx.recv_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| "host main thread unreachable".to_string())
    }

    /// Inject one op — the same path the JS frontend's `__hostEmit` takes.
    pub fn send_op(&self, op: Op) -> Result<(), String> {
        self.ops_tx
            .send_blocking(vec![op])
            .map_err(|_| "ops channel closed".to_string())
    }

    /// Register the QuickJS eval mailbox (called at engine boot).
    pub fn attach_eval(&self, tx: Sender<(String, Sender<String>)>) {
        *self.eval_tx.lock().unwrap() = Some(tx);
    }

    /// Request an eval on the engine thread. `Err` when no engine is attached.
    pub fn eval_request(&self, expr: String, reply: Sender<String>) -> Result<(), String> {
        let guard = self.eval_tx.lock().unwrap();
        match guard.as_ref() {
            Some(tx) => tx.send((expr, reply)).map_err(|e| e.to_string()),
            None => Err("no JS engine attached (backend is not quickjs?)".to_string()),
        }
    }

    /// Is a tree request waiting? The GPUI-side pump (`spawn_cdp_pump`) polls
    /// this — a mutex check, no frame work — and only spins up the main-thread
    /// drain when DevTools actually asked for something. Without it, tree
    /// reads would wait for the app's next *own* frame, which on a static UI
    /// is only the ~500 ms heartbeat.
    pub fn has_requests(&self) -> bool {
        !self.reqs.lock().unwrap().is_empty()
    }

    /// Serve every queued CDP tree request. Called from `HostView::render`
    /// (per frame) and from `spawn_cdp_pump` (main thread, on demand) — both
    /// run on the GPUI main thread where `&Tree` is legal, and `mem::take`
    /// makes each drain atomic, so double-serves are impossible.
    pub fn drain_requests(&self, tree: &Tree) {
        let reqs: Vec<Req> = {
            let mut g = self.reqs.lock().unwrap();
            std::mem::take(&mut *g)
        };
        for r in reqs {
            match r {
                Req::Tree(f) => f(tree),
            }
        }
    }
}
