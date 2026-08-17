//! Ask a cell for HTML, and wait for the answer.
//!
//! # The problem this solves
//!
//! A cell cannot answer an HTTP client. `POST /messages` says so in its own
//! documentation: a cell's reply travels the routing cascade, not back over HTTP.
//! For a surface that is fatal — the picture is rendered by a cell, and it has to
//! reach the browser that asked for it.
//!
//! The way out is the colony's egress door (GH #159, [`meclaw_colony::EgressPolicy`]):
//! a message that dies at the root hive **carrying our marker** is handed to a
//! channel instead of the dead-letter queue. This module owns the other end of
//! that channel.
//!
//! # How a reply finds the request that is waiting for it
//!
//! Three keys, all in `context`, all stamped by this module at injection:
//!
//! - [`EGRESS_MARK`] — makes the colony's door claim the message. Without it the
//!   message dead-letters exactly as it always did.
//! - [`REQUEST_ID`] — which waiter this belongs to.
//! - [`SURFACE_PATH`] — which surface it is about, so the cache can be keyed and
//!   so a cell has an identity it can trust: it came from `context`, which is edge
//!   authority, not from a body a model could have written.
//!
//! `context` is the right compartment and `hop` is not: a cell's emission
//! inherits the inbound `context` with a fresh `hop`, which is exactly the
//! survival property a multi-pass render needs. That is proven end to end in
//! `crates/meclaw-colony/tests/gh159_egress_policy.rs`, not assumed here.
//!
//! # No shared mutable state
//!
//! The waiter table and the cache live inside **one task**, reached by a command
//! channel. No `Mutex`, no `RwLock` — the same discipline the substrate holds for
//! cells, for the same reason: a lock here would be a second place where two
//! requests can disagree about who is waiting for what.
//!
//! # The cache, and its one rule
//!
//! Every successful render is kept under its surface path and **replaced** the
//! moment a newer one arrives. So it can never be older than the last change, and
//! a second viewer joining an unchanged surface costs zero cell calls. Without it,
//! N simultaneous joins would be N renders of the same picture.

use meclaw_colony::ColonyMsg;
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid, serde_json::Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// `context` key that makes the colony's egress door claim a message.
pub const EGRESS_MARK: &str = "surface_reply";
/// `context` key naming the request a reply belongs to.
pub const REQUEST_ID: &str = "surface_request";
/// `context` key naming the surface a request is about.
pub const SURFACE_PATH: &str = "surface_path";

/// Why a render did not produce HTML.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderError {
    /// The colony's inbox is gone — it shut down, or never started.
    NoColony,
    /// Nothing came back in time. The waiter is unregistered before this is
    /// returned, so a late reply cannot resolve a future request.
    Timeout,
    /// The cell answered, and what it said was an error.
    CellError(String),
    /// The cell answered with something that is not a surface reply at all.
    Malformed(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::NoColony => write!(f, "colony unavailable"),
            RenderError::Timeout => write!(f, "the surface cell did not answer in time"),
            RenderError::CellError(m) => write!(f, "the surface cell reported: {m}"),
            RenderError::Malformed(m) => write!(f, "the surface cell answered oddly: {m}"),
        }
    }
}

/// What the task is asked to do. Every mutation of the waiter table and the cache
/// is one of these, which is what keeps both inside a single task.
enum Cmd {
    Register {
        id: String,
        waiter: oneshot::Sender<Result<String, RenderError>>,
    },
    Forget {
        id: String,
    },
    Cached {
        surface: String,
        reply: oneshot::Sender<Option<String>>,
    },
}

/// Owns the egress receiver and matches replies to the requests waiting for them.
pub struct Dispatcher {
    colony: mpsc::Sender<ColonyMsg>,
    cmd: mpsc::Sender<Cmd>,
    /// `colony.json::message_default_ttl`, the same budget `POST /messages` gives
    /// an injected message.
    ///
    /// Not optional and not a local constant: a surface render is a real cascade
    /// (render → store → render → root, plus the hive transits), and a message
    /// built without a budget dies as `TtlExpired` before the cell ever runs. The
    /// first live join did exactly that, and the colony log named it.
    message_default_ttl: u32,
}

impl Dispatcher {
    /// Start the dispatcher. The returned handle drives the egress channel and
    /// the waiter table; dropping it leaves every `render` to time out.
    pub fn new(
        colony: mpsc::Sender<ColonyMsg>,
        egress_rx: mpsc::Receiver<Message>,
        message_default_ttl: u32,
    ) -> (Arc<Self>, tokio::task::JoinHandle<()>) {
        let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>(256);
        let join = tokio::spawn(run(cmd_rx, egress_rx));
        (
            Arc::new(Self {
                colony,
                cmd: cmd_tx,
                message_default_ttl,
            }),
            join,
        )
    }

    /// Send a request to the surface's cell and wait for its HTML.
    ///
    /// `request` is handed to the cell verbatim under the body's `surface` slot.
    /// This module does not interpret it: a surface's events are the surface's
    /// vocabulary, and the moment the HTTP layer understood one of them it would
    /// know what is being drawn.
    pub async fn render(
        &self,
        cell_path: &str,
        request: Value,
        timeout: Duration,
    ) -> Result<String, RenderError> {
        let id = Uuid::now_v7().to_string();
        let (waiter_tx, waiter_rx) = oneshot::channel();
        if self
            .cmd
            .send(Cmd::Register {
                id: id.clone(),
                waiter: waiter_tx,
            })
            .await
            .is_err()
        {
            return Err(RenderError::NoColony);
        }

        let msg = MessageBuilder::new(Path::new(cell_path))
            .ttl(self.message_default_ttl)
            .context(request_context(&id, cell_path))
            .body(Body::Inline(meclaw_core::serde_json::json!({
                "surface": request
            })))
            .build();
        if self
            .colony
            .send(ColonyMsg::Route {
                sender_path: Path::new("/"),
                msg,
            })
            .await
            .is_err()
        {
            let _ = self.cmd.send(Cmd::Forget { id }).await;
            return Err(RenderError::NoColony);
        }

        match tokio::time::timeout(timeout, waiter_rx).await {
            Ok(Ok(result)) => result,
            // The task dropped our waiter, which only happens if it is gone.
            Ok(Err(_)) => Err(RenderError::NoColony),
            Err(_) => {
                // Unregister BEFORE returning: a reply that arrives late must not
                // find a slot, or it would resolve somebody else's request.
                let _ = self.cmd.send(Cmd::Forget { id }).await;
                Err(RenderError::Timeout)
            }
        }
    }

    /// The last HTML rendered for a surface, if there is one.
    pub async fn cached(&self, cell_path: &str) -> Option<String> {
        let (tx, rx) = oneshot::channel();
        self.cmd
            .send(Cmd::Cached {
                surface: cell_path.to_string(),
                reply: tx,
            })
            .await
            .ok()?;
        rx.await.ok().flatten()
    }
}

/// The three `context` entries a surface request carries.
fn request_context(id: &str, cell_path: &str) -> meclaw_core::serde_json::Map<String, Value> {
    let mut m = meclaw_core::serde_json::Map::new();
    m.insert(
        EGRESS_MARK.to_string(),
        meclaw_core::serde_json::json!(true),
    );
    m.insert(REQUEST_ID.to_string(), meclaw_core::serde_json::json!(id));
    m.insert(
        SURFACE_PATH.to_string(),
        meclaw_core::serde_json::json!(cell_path),
    );
    m
}

/// The one task that owns the waiter table and the cache.
async fn run(mut cmd_rx: mpsc::Receiver<Cmd>, mut egress_rx: mpsc::Receiver<Message>) {
    let mut waiting: HashMap<String, oneshot::Sender<Result<String, RenderError>>> = HashMap::new();
    let mut cache: HashMap<String, String> = HashMap::new();
    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => match cmd {
                Some(Cmd::Register { id, waiter }) => { waiting.insert(id, waiter); }
                Some(Cmd::Forget { id }) => { waiting.remove(&id); }
                Some(Cmd::Cached { surface, reply }) => {
                    let _ = reply.send(cache.get(&surface).cloned());
                }
                None => return,
            },
            out = egress_rx.recv() => match out {
                Some(msg) => handle_egress(msg, &mut waiting, &mut cache),
                None => return,
            },
        }
    }
}

/// One message off the egress channel.
fn handle_egress(
    msg: Message,
    waiting: &mut HashMap<String, oneshot::Sender<Result<String, RenderError>>>,
    cache: &mut HashMap<String, String>,
) {
    let id = msg
        .headers
        .context
        .get(REQUEST_ID)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let surface = msg
        .headers
        .context
        .get(SURFACE_PATH)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let result = read_reply(&msg);

    // The cache is written even when nobody is waiting any more — a browser that
    // closed mid-render still paid for the render, and the next one should not.
    if let Ok(html) = &result
        && !surface.is_empty()
    {
        cache.insert(surface, html.clone());
    }

    match waiting.remove(&id) {
        Some(waiter) => {
            let _ = waiter.send(result);
        }
        // Normal, not an error: a browser that closed mid-render leaves no waiter.
        None => tracing::debug!(
            request = %id,
            "surface reply arrived with nobody waiting for it"
        ),
    }
}

/// The reply contract, in one place.
///
/// A surface cell answers with `{"surface": {"html": "…"}}` or with
/// `{"surface": {"error": "…"}}`. Anything else is malformed, and saying so beats
/// serving half a page: a canvas that renders partly is harder to diagnose than
/// one that visibly fails.
fn read_reply(msg: &Message) -> Result<String, RenderError> {
    let Body::Inline(body) = &msg.body else {
        return Err(RenderError::Malformed("body is not inline".into()));
    };
    let Some(slot) = body.get("surface") else {
        return Err(RenderError::Malformed(
            "no `surface` slot in the body".into(),
        ));
    };
    if let Some(e) = slot.get("error").and_then(|v| v.as_str()) {
        return Err(RenderError::CellError(e.to_string()));
    }
    match slot.get("html").and_then(|v| v.as_str()) {
        Some(html) => Ok(html.to_string()),
        None => Err(RenderError::Malformed(
            "`surface` carries neither `html` nor `error`".into(),
        )),
    }
}
