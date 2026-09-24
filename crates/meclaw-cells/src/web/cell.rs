//! W8 (GH #380): the handler half of the `web` cell.
//!
//! The double-task split follows the proxy precedent: this half owns the cell
//! state and the `cell.db` and is the only writer; the I/O half
//! ([`crate::web::io`]) owns the listener and never touches either. They talk
//! over two internal channels, which is what keeps one-task-per-actor true for
//! a cell that has a whole HTTP server hanging off it — no locks, because
//! nothing is shared.

use crate::web::assets::{AssetMap, load_assets};
use crate::web::io::{WebIo, run_io};
use crate::web::ops;
use crate::web::output::{self, BundleLeg, OpOutcome};
use crate::web::render::{PageMap, materialize_all};
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, CellOutput, Message, OriginSink, OutputSink};
use std::future::Future;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};

/// What the I/O half tells the handler.
///
/// Two things: the mount was refused, or a browser said something.
pub enum WebEvent {
    /// The name this display declared is not one it could have: another cell
    /// holds it, or it breaks the mount grammar. The cell stays alive and is
    /// reachable through nothing — a display that cannot serve must not take
    /// its cell down, or a name collision would look like a crash loop. The
    /// way out is a params update naming a free mount, which takes effect on
    /// the cell's next life (O-P-2).
    MountFailed(String),
    /// A browser said something on a joined socket.
    ///
    /// The I/O half does not decide what it means, and cannot: sorting an event
    /// into a local `editable` write or a semantic emission needs the database
    /// and a sink, and the I/O half has neither. It forwards, and waits on
    /// `respond` for the one answer the socket owes its client.
    Browser {
        /// Which connection sent it, so a reply can be addressed.
        viewer: String,
        /// Which page it was sent from.
        route: String,
        /// The browser session this event belongs to — the nonce half of the
        /// page's LiveView token, unique per page load.
        session_id: String,
        /// Who the proxy in front said is looking, when the cell was given an
        /// `identity_header` and the request carried it. `None` otherwise, and
        /// then nothing is stamped: an identity this cell invented would be
        /// worse than none (O-P-4).
        user_id: Option<String>,
        /// The event name, verbatim.
        name: String,
        /// The event value, verbatim.
        value: meclaw_core::JsonValue,
        /// Where the handler's verdict goes. The socket turns it into the
        /// `phx_reply` its client is waiting for.
        respond: tokio::sync::oneshot::Sender<EventReply>,
    },
}

/// What the handler says about a browser event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventReply {
    /// Accepted. The diff, if any, travels separately as a push to every
    /// viewer — including the sender, which is why this carries no payload.
    Ok,
    /// Refused, with a reason the client can read. `not_editable` is the one
    /// that matters: a prop the component did not declare `editable` is not
    /// writable from a browser, and nothing was written.
    Error(String),
}

/// What the handler tells the I/O half.
///
/// The substrate's own reconfig channel carries the shutdown signal by being
/// closed — the I/O half treats a closed channel as "the handler is gone" —
/// and nothing else since `web@2.0.0`: the `Rebind` that moved a listener went
/// with the listener. A `Push` travels on the cell's own `push_tx` (minted in
/// [`WebCell`], so a browser event can push too) and is the only variant here.
#[derive(Debug)]
pub enum WebReconfig {
    /// Send this diff to everyone joined on `route`.
    ///
    /// The handler decides *what* changed; the I/O half owns the socket senders
    /// and does the addressing. That split is why this is a message and not a
    /// shared table.
    Push {
        /// Which page's viewers.
        route: String,
        /// The LiveView diff payload, already packed.
        diff: Value,
    },
}

/// The `web` cell's handler half.
pub struct WebCell {
    /// This cell's own path. It is the identity the LiveView session token is
    /// minted for and the container id is derived from — the cell knows its own
    /// address, which is not topology (it learns nothing about anyone else).
    pub(crate) path: String,
    /// Taken by [`LongRunningCell::split_io`] exactly once, before the I/O task
    /// is spawned. `Option` because `split_io` is sync and by-value: there is
    /// no `.await` available to build the state lazily, and the respawn corridor
    /// requires it stay that way.
    pub(crate) io: Option<WebIo>,
    /// The publishing end of the rendered pages. The handler is the only
    /// writer; the listener reads whatever was last published. See the note on
    /// [`WebIo`] for why this is a channel and not a lock.
    pub(crate) pages_tx: watch::Sender<Arc<PageMap>>,
    /// The publishing end of the served files (GH #393).
    ///
    /// Sent exactly once, from `on_start`. No op writes the `assets` table, so
    /// the snapshot cannot go stale — and keeping the publisher here rather
    /// than in the factory keeps one answer to "who writes the seams": the
    /// handler half, always. An op that ever adds a file re-publishes on this
    /// sender and nothing else changes.
    pub(crate) assets_tx: watch::Sender<Arc<AssetMap>>,
    /// The readiness seam (GH #395): `false` until `on_start` has published the
    /// first page snapshot, and the listener answers `503 starting` rather than
    /// `404` for as long as it is.
    ///
    /// Sent from the success arm only, deliberately. If the initial render
    /// fails this display has nothing to serve and never will without a write,
    /// and `503` is the truthful answer to that — a proxy and a health check can
    /// both act on it, where a permanent `404` would report an empty display as
    /// a working one.
    pub(crate) ready_tx: watch::Sender<bool>,
    /// Diffs on their way to the listener, which owns the socket senders.
    ///
    /// Deliberately **not** the substrate's `reconfig` channel. That one has a
    /// job already — the I/O half reads its closing as "the handler is gone" —
    /// and `handle_event` is not handed a sender for it anyway. Minting our own
    /// keeps the shutdown signal unambiguous and lets both halves of the cell
    /// push, which is what a browser event needs.
    pub(crate) push_tx: mpsc::Sender<WebReconfig>,
    /// The name this display is reached under.
    ///
    /// The handler holds it because it is the only side that may change it: an
    /// update is merged over these values and persisted, and the I/O half
    /// reads the result on its next life (O-P-2). Two copies of the same
    /// mutable fact would be a lock in disguise.
    pub(crate) mount: String,
    /// The proxy's identity header, or empty. See [`WebCell::mount`].
    pub(crate) identity_header: String,
    /// GH #833: whose identity header is believed, as written; `None` is the
    /// default. See [`WebCell::mount`].
    pub(crate) trusted_proxies: Option<Vec<String>>,
    /// The live operation-timeout, held for the same reason: a params update
    /// merges over it.
    pub(crate) external_timeout_ms: u64,
}

impl WebCell {
    /// Build a cell around an already-constructed I/O state.
    ///
    /// `params` are the **effective** ones — birth params with the `cell.db`
    /// overlay replayed over them — so a display that was moved comes back
    /// where it was moved to rather than where it was born.
    pub fn new(
        path: String,
        io: WebIo,
        params: &crate::web::params::WebParams,
        pages_tx: watch::Sender<Arc<PageMap>>,
        assets_tx: watch::Sender<Arc<AssetMap>>,
        ready_tx: watch::Sender<bool>,
        push_tx: mpsc::Sender<WebReconfig>,
    ) -> Self {
        Self {
            path,
            io: Some(io),
            pages_tx,
            assets_tx,
            ready_tx,
            push_tx,
            mount: params.mount.clone(),
            identity_header: params.identity_header.clone(),
            trusted_proxies: params.trusted_proxies.clone(),
            external_timeout_ms: params.external_timeout_ms,
        }
    }

    /// Re-render, publish the new pages, and push one diff per affected page.
    ///
    /// The order matters. The pages are published **first**, so a GET arriving
    /// between the write and the push already sees the new content — a viewer
    /// that reloads must never see less than a viewer that stayed connected.
    /// Then ONE diff goes out per route, carrying every slot that moved.
    ///
    /// Per route, not per slot (GH #723). LiveView re-renders the whole
    /// container out of its cached tree on every diff it receives, and it
    /// patches the cache with the keys the diff names. A frame that names only
    /// slot 2 therefore re-renders slots 0 and 1 from the values the cache
    /// still holds — the ones from BEFORE this pass — and writes them over the
    /// DOM. Anything the client had drawn optimistically (§ 5.7) is gone until
    /// the next frame restores it. Measured on the fresh instance (18.09.2026,
    /// Chromium, a real click closing the chat window): two frames 5 ms apart,
    /// `{"2"}` at +167 ms and `{"0"}` at +172 ms, and `data-level` went
    /// `2 -> 0@30 (optimistic) -> 2@172 -> 0@176`. The window blinked, which
    /// § 5.8 forbids. One frame with `{"0": …, "1": …, "2": …}` has no such
    /// gap; the same run proves the client reads that shape, because the
    /// structural arm already sends `["0","1","2","3","s"]` in one frame.
    ///
    /// A structural change (create, move, delete) re-sends the whole packed
    /// tree for that page instead of the slots: the slot list itself changed,
    /// so a positional patch would address the wrong slot. That is the honest
    /// version of "one diff per write" — it is one frame either way, and which
    /// kind it is depends on what actually changed.
    async fn publish_and_push(&self, db: &mut DbConn, touched: &ops::Touched) {
        let Ok(map) = db.call(|conn| materialize_all(conn)).await else {
            tracing::error!(
                path = %self.path,
                "web: could not re-render after a write — the display keeps serving \
                 whatever it last published"
            );
            return;
        };
        let map = Arc::new(map);
        // A send failure means the listener is gone, which happens during
        // shutdown and is not worth a line in the journal.
        let _ = self.pages_tx.send(map.clone());

        // The root-object case: structural, but no slot to name — the whole
        // page changed. Every route gets its packed tree, or the viewers keep
        // a picture the database has left behind.
        if touched.structural && touched.slots.is_empty() {
            for (route, page) in map.iter() {
                let _ = self
                    .push_tx
                    .send(WebReconfig::Push {
                        route: route.clone(),
                        diff: page.packed_tree(),
                    })
                    .await;
            }
            return;
        }

        // Group the touched slots by route — the grouping, not the order, is
        // what makes one frame per route. A bundle arrives here already sorted
        // and deduplicated (see the merge above); a single call carries at most
        // one slot per route by construction (`touched_by`), except for a move,
        // which names the slot it left and the slot it reached.
        let mut by_route: Vec<(&String, Vec<&String>)> = Vec::new();
        for (route, slot_id) in &touched.slots {
            match by_route.iter_mut().find(|(r, _)| *r == route) {
                Some((_, slots)) => slots.push(slot_id),
                None => by_route.push((route, vec![slot_id])),
            }
        }

        for (route, slot_ids) in by_route {
            let Some(page) = map.get(route) else { continue };
            // An object that is no longer a slot on this page cannot be
            // addressed positionally — send the whole tree rather than a patch
            // that misses, or the viewer keeps a picture that is quietly wrong.
            // It is decided for the WHOLE group: a tree for one slot and a
            // patch for its neighbours would be two frames again. With the ops
            // the substrate has today every way to make a slot disappear also
            // sets `structural`, so this arm is a belt on a brace rather than
            // a path of its own — `723_a_route_hears_one_frame_per_pass.rs`
            // locks the reachable shape, a move that leaves one root child for
            // another.
            let unaddressable = slot_ids
                .iter()
                .any(|slot_id| page.slot_of(slot_id).is_none());
            let diff = if touched.structural || unaddressable {
                page.packed_tree()
            } else {
                let mut diff = Map::new();
                for slot_id in slot_ids {
                    if let Some(i) = page.slot_of(slot_id) {
                        diff.insert(i.to_string(), Value::String(page.slots[i].1.clone()));
                    }
                }
                Value::Object(diff)
            };
            let _ = self
                .push_tx
                .send(WebReconfig::Push {
                    route: route.clone(),
                    diff,
                })
                .await;
        }
    }
}

impl WebCell {
    /// Sort one browser event and, if it is a local write, apply it.
    ///
    /// **`object:set` is the local lane** (R-W8-5): a prop the component
    /// declared `editable` is written here and diffed to every viewer, with no
    /// message entering the colony at all. That is the whole point — a drag
    /// must not cost a topology round trip, or dragging a node would be a
    /// conversation with the router.
    ///
    /// A prop that is *not* declared editable is refused with `not_editable`
    /// and **nothing is written**. The declaration is the authorisation: a
    /// browser may move what a component said may be moved, and nothing else.
    /// Everything that is not `object:set` is a semantic event and belongs to
    /// Task 10.
    #[allow(clippy::too_many_arguments)]
    async fn handle_browser_event(
        &mut self,
        name: &str,
        value: &Value,
        route: &str,
        session_id: &str,
        user_id: Option<&str>,
        sink: &OriginSink,
        db: &mut DbConn,
    ) -> (EventReply, ops::Touched) {
        if name != "object:set" {
            // The semantic lane (R-W8-5). Everything that is not a declared
            // local write is somebody else's business: it leaves as an ordinary
            // cell output on `hop.route = "event"`, exactly as the proxy emits a
            // platform turn. Whether anything is listening is a question about
            // the topology, and this cell does not ask it.
            self.emit_semantic(name, value, route, session_id, user_id, sink)
                .await;
            return (EventReply::Ok, ops::Touched::default());
        }
        let (Some(id), Some(prop)) = (
            value.get("id").and_then(Value::as_str),
            value.get("prop").and_then(Value::as_str),
        ) else {
            return (
                EventReply::Error("object:set needs \"id\" and \"prop\"".to_string()),
                ops::Touched::default(),
            );
        };
        let new_value = value.get("value").cloned().unwrap_or(Value::Null);
        let (id, prop) = (id.to_string(), prop.to_string());

        db.call(move |conn| ops::set_editable(conn, &id, &prop, &new_value))
            .await
    }

    /// Apply a runtime params update.
    ///
    /// Merge, then persist. Nothing moves while the cell is alive: the mount
    /// is registered once per life, at the top of the I/O half, and a live
    /// remount would take a running display out from under whichever proxy
    /// rule points at it while its viewers hold sockets on the old name. So a
    /// `mount` an update names is written to the overlay and read by the next
    /// life (O-P-2), which is the same promise a `voice` cell has made since
    /// 1.5.0. A refusal writes nothing.
    ///
    /// Silent on success, in the shape every other cell type's params update
    /// has (`proxy`, `timer`, `mcp`).
    async fn apply_params_update(
        &mut self,
        update: &Map<String, Value>,
        started: std::time::Instant,
        reply_target: meclaw_core::Path,
        sink: &OutputSink,
        db: &mut DbConn,
    ) {
        let refuse = |text: String| {
            output::build_refusal(
                "params",
                "invalid_input",
                text,
                started.elapsed().as_millis() as i64,
            )
        };

        let current = crate::web::params::WebOverlay {
            mount: self.mount.clone(),
            identity_header: self.identity_header.clone(),
            trusted_proxies: self.trusted_proxies.clone(),
            external_timeout_ms: self.external_timeout_ms,
        };
        let (merged, overlay) = match crate::params_overlay::apply_update(&current, update) {
            Ok(ok) => ok,
            Err(e) => {
                let _ = sink
                    .push(CellOutput {
                        target: reply_target,
                        content: refuse(e.detail()),
                    })
                    .await;
                return;
            }
        };

        let now = crate::params_overlay::now_unix_seconds();
        let persist = db
            .call(move |c| crate::params_overlay::persist_params_overlay(c, &overlay, now))
            .await;
        if let Err(e) = persist {
            // Nothing has moved — the update takes effect on the next life —
            // so this is the whole of the damage: the next life reads the
            // birth params, and the sender is told rather than left believing
            // the display was renamed.
            let _ = sink
                .push(CellOutput {
                    target: reply_target,
                    content: refuse(format!(
                        "cell.db params write failed: {e} — the update is not \
                         remembered and the next life starts on the birth params"
                    )),
                })
                .await;
            return;
        }

        self.mount = merged.mount.clone();
        self.identity_header = merged.identity_header.clone();
        self.trusted_proxies = merged.trusted_proxies.clone();
        self.external_timeout_ms = merged.external_timeout_ms;
        db.set_query_timeout(Some(std::time::Duration::from_millis(
            self.external_timeout_ms,
        )));
    }

    /// Emit one semantic browser event on the cell's out-edges.
    ///
    /// `OriginSink` rather than `OutputSink`: a browser event has no parent
    /// message — nobody asked for it — so it is a **source** emission, the same
    /// shape the proxy uses for an inbound platform turn.
    ///
    /// The header carries `event_name`, `session_id` and `route`, and
    /// `user_id` when a proxy in front named the person (`identity_header`).
    /// Promoting any of them into `context` is the ingress **edge's** job via
    /// `set_context`, not this cell's: a cell states what it knows, and an edge
    /// decides what that means for the graph. That is the proxy precedent, and
    /// it is what keeps this cell ignorant of the topology it hangs in.
    ///
    /// The identity is **absent** rather than empty when there is none: a
    /// `hop.user_id` of `""` is a claim about who was looking, and this cell
    /// has nothing to base it on unless a proxy told it (O-P-4).
    async fn emit_semantic(
        &self,
        name: &str,
        value: &Value,
        route: &str,
        session_id: &str,
        user_id: Option<&str>,
        sink: &OriginSink,
    ) {
        let mut header = json!({
            "route": "event",
            "event_name": name,
            "session_id": session_id,
            "page_route": route,
        });
        if let (Some(user_id), Some(header)) = (user_id, header.as_object_mut()) {
            header.insert("user_id".to_string(), json!(user_id));
        }
        let content = json!({
            "header": header,
            "messages": [{
                "origin": "user",
                "type": "text",
                "text": meclaw_core::serde_json::to_string(value).unwrap_or_default(),
            }],
            "event": { "name": name, "value": value },
        });
        // The out-edges decide where this goes: `apply_edges` matches the
        // sender's edges against the merged headers, and `target` is only the
        // name that appears if NOTHING matched (the `no_route` dead-letter). So
        // it is this cell's own path — an honest "I have no particular
        // destination; the topology does" — and a display whose events nobody
        // listens for dead-letters visibly instead of silently.
        let _ = sink
            .emit(meclaw_core::CellOutput {
                target: meclaw_core::Path::new(&self.path),
                content,
            })
            .await;
    }
}

impl LongRunningCell for WebCell {
    type Event = WebEvent;
    type Reconfig = WebReconfig;
    type Io = WebIo;

    fn split_io(&mut self) -> Self::Io {
        self.io
            .take()
            .expect("split_io is called exactly once per spawn, before run_io")
    }

    /// Render what the database already holds, before the first request can
    /// arrive. Without this a freshly booted display would serve 404 for every
    /// seeded route until something happened to write to it.
    ///
    /// This is also the reason `on_start` exists rather than doing the work on
    /// the first event: the `select!` over mailbox and events has no ordering,
    /// so anything that waited for a message would be racing the first GET.
    ///
    /// The served files are published from here too (GH #393), and for the same
    /// reason: a seeded stylesheet nothing had delivered was the bug, and a
    /// stylesheet delivered only after the first write would be the same bug
    /// with a longer fuse. Unlike the pages this snapshot is sent **once** —
    /// no op writes the `assets` table, so there is nothing to re-publish.
    #[allow(clippy::manual_async_fn)]
    fn on_start<'a>(
        &'a mut self,
        _sink: &'a OriginSink,
        db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            let map = db.call(|conn| materialize_all(conn)).await;
            match map {
                Ok(m) => {
                    let _ = self.pages_tx.send(Arc::new(m));
                    // The window GH #395 is about closes here, and only here:
                    // published first, then declared ready, so no request can
                    // observe `ready` while the old empty snapshot is current.
                    let _ = self.ready_tx.send(true);
                }
                Err(e) => tracing::error!(
                    path = %self.path,
                    error = %e,
                    "web: initial render failed — this display serves nothing until a write lands"
                ),
            }
            match db.call(|conn| load_assets(conn)).await {
                Ok(a) => {
                    let _ = self.assets_tx.send(Arc::new(a));
                }
                // A broken read here costs the files and nothing else: the
                // pages are already published, and a display with no stylesheet
                // is still a display.
                Err(e) => tracing::error!(
                    path = %self.path,
                    error = %e,
                    "web: could not read the assets table — this display serves no files"
                ),
            }
        }
    }

    // `attach_liveness` is deliberately left at its default, which drops the
    // mark (GH #7). The liveness mark reports round trips a cell *initiates*, so
    // a stalled poller can be told from an idle one. A listener initiates
    // nothing — it waits to be called — and an idle display is the normal
    // resting state of a page nobody has open. Marking here would either be a
    // lie (marking on a timer) or would report browser traffic as cell liveness.

    fn run_io(
        io: Self::Io,
        events_tx: mpsc::Sender<Self::Event>,
        reconfig_rx: mpsc::Receiver<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send {
        run_io(io, events_tx, reconfig_rx)
    }

    /// The tool-call surface: `object.*`, `query`.
    ///
    /// One `tool_call` turn is a single op and answers with its metadata on the
    /// header. Two or more are a **bundle** and answer with one reply carrying
    /// one turn per op in call order, plus a `results[]` slot — the store's GH
    /// #295 shape, and the counting is of `tool_call` TURNS rather than of
    /// `messages`, because an `llm` cell emits mixed `[tool_call, text]` bodies
    /// and prose alongside a call must not change how the call is answered.
    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a mut self,
        msg: Message,
        sink: &'a OutputSink,
        db: &'a mut DbConn,
        _reconfig_tx: &'a mpsc::Sender<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            let started = std::time::Instant::now();
            let reply_target = msg.reply_to.clone().unwrap_or_else(|| msg.target.clone());

            // The params-update slot (`config.md` § Access), handled FIRST and
            // exclusively: a message that carries it is not a tool call, and
            // reading it as one would refuse a valid update for having no
            // `messages` array. It is also how a display is renamed — on its
            // next life (O-P-2).
            if let Body::Inline(v) = &msg.body
                && let Some(params_val) = v.get("params")
            {
                match params_val.as_object() {
                    Some(update) => {
                        let update = update.clone();
                        self.apply_params_update(&update, started, reply_target, sink, db)
                            .await;
                    }
                    None => {
                        let _ = sink
                            .push(CellOutput {
                                target: reply_target,
                                content: output::build_refusal(
                                    "params",
                                    "invalid_input",
                                    "params slot: not a JSON object".to_string(),
                                    started.elapsed().as_millis() as i64,
                                ),
                            })
                            .await;
                    }
                }
                return;
            }

            let calls = match parse_tool_calls(&msg) {
                Ok(c) => c,
                Err(e) => {
                    // An unreadable message is answered, not dropped: a caller
                    // that patched a display must learn that nothing happened.
                    let body = output::build_refusal(
                        "unknown",
                        "invalid_input",
                        e,
                        started.elapsed().as_millis() as i64,
                    );
                    let _ = sink
                        .push(CellOutput {
                            target: reply_target,
                            content: body,
                        })
                        .await;
                    return;
                }
            };

            let bundle = calls.len() > 1;
            let mut legs: Vec<BundleLeg> = Vec::with_capacity(calls.len());
            let mut single: Option<(OpOutcome, String)> = None;
            // What the whole bundle touched, accumulated. See the push below.
            let mut merged = ops::Touched::default();
            // Whether any leg was the ROOT-object case — structural with no
            // slot to name. It is tracked apart from `merged` because it is the
            // one case `publish_and_push` answers by BROADCASTING, and merging
            // it into a `Touched` that also names slots would hide it.
            let mut merged_root = false;

            for (args, id) in calls {
                let op_started = std::time::Instant::now();
                let (outcome, touched) = db.call(move |conn| ops::apply(conn, &args)).await;

                // One diff per write for a SINGLE call, immediately. A bundle
                // pushes once, after the loop.
                //
                // A bundle is one caller's one intention — a curator pass is
                // exactly that — and the steps inside it are not pictures
                // anybody should see. Measured on the fresh instance
                // (18.09.2026, GH #718): one tap arrived as eight
                // `object.update` legs, five of them root updates, and each of
                // those re-sent the whole packed tree AS IT STOOD mid-bundle,
                // with the window still closed. The browser drew `data-level`
                // `1@34 ms → 0@175 → 1@255` (Chromium) for one tap, and a
                // second browser watching the same state saw only a blink. Per
                // leg that is 7 frames and ~774 KB where the end state is one
                // frame and ~154 KB.
                //
                // `structural` with an empty slot list is the ROOT-object case:
                // `touched_by` cannot name a slot because the whole page is the
                // slot. Skipping it meant an `object.update` on the root never
                // published — the write sat in the database while the served
                // page and every viewer kept the old render indefinitely (the
                // steady-state tick emits nothing, so nothing ever caught up).
                if !outcome.is_error() && (!touched.slots.is_empty() || touched.structural) {
                    if bundle {
                        merged_root |= touched.structural && touched.slots.is_empty();
                        merged.structural |= touched.structural;
                        merged.slots.extend(touched.slots);
                    } else {
                        self.publish_and_push(db, &touched).await;
                    }
                }

                let dur = op_started.elapsed().as_millis() as i64;
                if bundle {
                    legs.push(BundleLeg::from_outcome(
                        &outcome,
                        id.unwrap_or_default(),
                        dur,
                    ));
                } else {
                    single = Some((outcome, id.unwrap_or_default()));
                }
            }

            // The bundle's one push per route, with the database already at
            // the end state: `publish_and_push` re-materialises, so what goes
            // out is what the last leg left behind and never a step before it.
            if bundle && (!merged.slots.is_empty() || merged.structural) {
                if merged_root {
                    // A leg moved a ROOT object, and that is a broadcast: the
                    // whole page changed and no slot can name it. Carrying the
                    // other legs' slots alongside would send the merged
                    // `Touched` down the ADDRESSED path instead and leave the
                    // output whose root moved out of its own pass — a display
                    // lays down one page per output (`display@2.5.0`), and one
                    // curator pass writes the root props of one and a window of
                    // another in the same breath. The broadcast is not a second
                    // frame on top of the addressed one: it sends every route
                    // its whole packed tree, which already contains every slot
                    // the bundle touched.
                    merged.slots.clear();
                } else if merged.structural {
                    // Structural without a root leg: the diff is the whole tree
                    // per route, so ONE entry per route is all of it and a
                    // second would send the same tree twice. Keeping the first
                    // slot of each route keeps the addressing — only the routes
                    // the bundle touched hear it — and drops the duplicates.
                    let mut routes: Vec<String> = Vec::new();
                    merged.slots.retain(|(route, _)| {
                        let first = !routes.iter().any(|r| r == route);
                        if first {
                            routes.push(route.clone());
                        }
                        first
                    });
                } else {
                    merged.slots.sort();
                    merged.slots.dedup();
                }
                self.publish_and_push(db, &merged).await;
            }

            let content = if bundle {
                let (body, headers) =
                    output::build_bundle_result(&legs, started.elapsed().as_millis() as i64);
                merge(headers, body)
            } else {
                let Some((outcome, id)) = single else { return };
                let (body, headers) =
                    output::build_tool_result(&outcome, id, started.elapsed().as_millis() as i64);
                merge(headers, body)
            };

            let _ = sink
                .push(CellOutput {
                    target: reply_target,
                    content,
                })
                .await;
        }
    }

    #[allow(clippy::manual_async_fn)]
    fn handle_event<'a>(
        &'a mut self,
        event: Self::Event,
        _sink: &'a OriginSink,
        _db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            match event {
                WebEvent::Browser {
                    viewer,
                    route,
                    session_id,
                    user_id,
                    name,
                    value,
                    respond,
                } => {
                    let (reply, touched) = self
                        .handle_browser_event(
                            &name,
                            &value,
                            &route,
                            &session_id,
                            user_id.as_deref(),
                            _sink,
                            _db,
                        )
                        .await;
                    // The verdict goes back to the socket that is holding its
                    // client's reply open. A closed channel means the browser
                    // left mid-flight, which is ordinary.
                    let accepted = reply == EventReply::Ok;
                    let _ = respond.send(reply);

                    if accepted && !touched.slots.is_empty() {
                        // The write landed, so everyone looking at that page —
                        // the sender included — gets the new picture: the ONE
                        // slot the write touched, not the whole packed tree
                        // (GH #414 — the structural fake here broadcast ~220 KB
                        // per drag write, and a group drag flooded the viewer
                        // channels until frames dropped). Task 10 handles the
                        // other class of event.
                        self.publish_and_push(_db, &touched).await;
                    }
                    tracing::debug!(path = %self.path, %viewer, %route, %name, "web: browser event");
                }
                WebEvent::MountFailed(err) => {
                    // Loud, and not fatal. The cell keeps running and is
                    // reachable through nothing: a name another cell holds is
                    // an operator's mistake to read in the journal, not a
                    // reason to take a cell — and with it possibly a whole
                    // colony boot — down.
                    tracing::error!(
                        path = %self.path,
                        error = %err,
                        "web: this display is reachable under no name — send it a \
                         params update naming a free mount; it takes effect on the \
                         cell's next life"
                    );
                }
            }
        }
    }
}

/// Fold headers and body into one content value.
fn merge(headers: Map<String, Value>, body: Value) -> Value {
    let mut content = Map::new();
    content.insert("header".into(), Value::Object(headers));
    if let Value::Object(o) = body {
        content.extend(o);
    }
    Value::Object(content)
}

/// Every `tool_call` turn of the inbound body, in call order.
///
/// Turns that are not `tool_call`s are **skipped**, not refused: an `llm` cell
/// really does emit `[tool_call, tool_call, text]` when a model wrote prose
/// beside its calls, and refusing such a body would reject a shape that ships.
/// Only a body with no `tool_call` at all is refused.
fn parse_tool_calls(msg: &Message) -> Result<Vec<(Value, Option<String>)>, String> {
    let Body::Inline(v) = &msg.body else {
        return Err("body must be inline".to_string());
    };
    let messages = v
        .get("messages")
        .and_then(|v| v.as_array())
        .ok_or("missing messages array")?;
    let Some(first) = messages.first() else {
        return Err("messages empty".to_string());
    };
    let picked: Vec<(usize, &Value)> = messages
        .iter()
        .enumerate()
        .filter(|(_, t)| t.get("type").and_then(|v| v.as_str()) == Some("tool_call"))
        .collect();
    if picked.is_empty() {
        let ty = first
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or("turn missing type")?;
        return Err(format!("expected tool_call turn, got {ty}"));
    }
    let bundle = picked.len() > 1;
    let mut calls = Vec::with_capacity(picked.len());
    for (i, turn) in picked {
        let text = turn
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or("missing text")?;
        // With N calls in flight, "not valid JSON" alone would not say WHICH
        // leg is broken, so the index names the turn a reader can find.
        let args: Value = meclaw_core::serde_json::from_str(text).map_err(|e| {
            if bundle {
                format!("tool_call[{i}].text not valid JSON: {e}")
            } else {
                format!("tool_call.text not valid JSON: {e}")
            }
        })?;
        let id = turn.get("id").and_then(|v| v.as_str()).map(str::to_string);
        calls.push((args, id));
    }
    Ok(calls)
}
