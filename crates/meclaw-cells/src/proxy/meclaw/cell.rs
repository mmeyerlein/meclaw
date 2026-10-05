//! `MeclawCell`: the handler half of a `meclaw` proxy.
//!
//! `handle()` is the outgoing direction: judge a message against this side's
//! `emits` lanes, book it in the outbox, post one frame to the address the
//! entry edge stamped, and emit the receipt. `handle_event()` is the incoming
//! direction, whose verdict the mount has already reached, plus the outbox
//! retries the I/O half's clock asks for. No mutex: the books live in the
//! cell's own `cell.db` (GH #1012, `book`), and the cell holds no cursor and
//! no allow-list (A9).
//!
//! GH #1012, the outgoing promise: at-least-once per message, in order per
//! target (`hop.peer_url`). A message is booked before its first attempt; a
//! carrier failure (no connection, no answer in time, a 5xx) is retried with
//! a backoff of 1 s doubling to 300 s, and the message reads exactly one
//! `deferred` receipt; a later success reads `crossed` with `deferred_ms`.
//! A message behind an undelivered one to the same target waits (head of
//! line, never overtaking) and reads its own `deferred` at once. A final
//! verdict (a lane, a policy, a far-side refusal, a 3xx or 4xx) is `refused`
//! as before; once the message was booked, that `refused` comes with a
//! `peer_refused` dead letter (OR-HV-51). Past `params.peer_retry_deadline_s`
//! the message is a `peer_expired` dead letter and an `expired` receipt —
//! never silent. The id and the booking clock travel as request headers
//! (`wire::FRAME_ID_HEADER`, `wire::SENT_MS_HEADER`); the frame stays the
//! 0.47.1 frame (review C1).

use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::{Body, CellOutput, Headers, Message, OriginSink, OutputSink, Path, Uuid};
use serde_json::{Map, Value};
use std::future::Future;
use std::sync::Arc;
use tokio::sync::mpsc;

use super::book::{self, OutRow};
use super::client::PeerClient;
use super::emit::{
    arrived_emission, crossed_emission, deferred_emission, expired_emission, refused_emission,
    stamp_arrival, with_delivery,
};
use super::io::{PeerIo, run_peer_io};
use super::lanes::{self, Direction, Refusal};
use super::mount::{MeclawIo, PeerEvent, PeerReconfig, Wake, now_ms};
use super::params::{Lanes, MeclawParams, refuse_params_update};
use super::wire;

/// The code a `params` slot and a mount that could not be taken are refused
/// with; the house word for a request this cell will not take.
const INVALID_INPUT: &str = "invalid_input";

/// GH #1012 (OR-HV-5): the first retry falls 1 s after the failed attempt,
/// every further one doubles the wait, up to 300 s.
const BACKOFF_FIRST_MS: i64 = 1_000;
/// GH #1012: the longest wait between two attempts.
const BACKOFF_MAX_MS: i64 = 300_000;

/// The `meclaw` proxy. State lives single-threaded in the handler sub-task.
pub struct MeclawCell {
    params: MeclawParams,
    lanes: Arc<Lanes>,
    /// GH #840: the parsed `params.egress`, `None` when the key is absent.
    egress: Option<Vec<String>>,
    client: PeerClient,
    emit_to: Path,
    /// The mount half, consumed exactly once by `split_io`.
    initial_io_cfg: Option<MeclawIo>,
    /// GH #1012: where a `peer_expired` or `peer_refused` dead letter goes.
    colony_inbox: Option<mpsc::Sender<meclaw_colony::ColonyMsg>>,
    /// GH #1012: the way to the I/O half's outbox clock, set by `split_io`.
    wakes: Option<mpsc::UnboundedSender<Wake>>,
    /// GH #1012: whether the books exist in this life's connection.
    book_ready: bool,
    /// GH #1012: `params.peer_retry_deadline_s` in ms.
    deadline_ms: i64,
    /// GH #1015: inbox rows handed on and not yet booked `done`, oldest first.
    handed: std::collections::VecDeque<(String, std::time::Instant)>,
}

/// GH #1015: how long a handed-on inbox row stays `pending`. The hand-on is an
/// emission; the colony logs it in a later writer batch, and a cell cannot see
/// that commit. A row booked `done` before it would be the one loss window a
/// crash leaves (measured in the HV-14 SIGKILL run of #1012). Kept `pending`,
/// the row is raised again at the next start, keyed so the colony drops it
/// when its log already holds it. A writer batch commits in milliseconds; the
/// margin covers a backlogged writer.
const HAND_ON_SETTLE: std::time::Duration = std::time::Duration::from_secs(30);

/// A message that may cross: its lane, the frame and the address.
struct Crossing {
    lane: String,
    fields: Vec<String>,
    frame: Value,
    peer_url: String,
}

impl MeclawCell {
    /// A cell with a client of its own. A client that cannot be built (TLS
    /// init) is a spawn error.
    pub fn new(p: &MeclawParams) -> Result<Self, String> {
        Ok(Self::with_client(
            p,
            PeerClient::with_auth(p.auth.as_ref())?,
        ))
    }

    /// A cell posting through `client` (cheap to clone; the factory builds one
    /// per cell outside the respawn closure).
    pub fn with_client(p: &MeclawParams, client: PeerClient) -> Self {
        Self {
            params: p.clone(),
            lanes: Arc::new(p.lanes.clone()),
            egress: p.egress_origins(),
            client,
            emit_to: p.emit_to_path(),
            initial_io_cfg: None,
            colony_inbox: None,
            wakes: None,
            book_ready: false,
            deadline_ms: i64::try_from(p.peer_retry_deadline_s.saturating_mul(1_000))
                .unwrap_or(i64::MAX),
            handed: std::collections::VecDeque::new(),
        }
    }

    /// Hands the cell its mount half.
    pub fn with_io(mut self, io: MeclawIo) -> Self {
        self.initial_io_cfg = Some(io);
        self
    }

    /// GH #1012: hands the cell the colony's inbox for its dead letters.
    pub fn with_colony_inbox(mut self, tx: mpsc::Sender<meclaw_colony::ColonyMsg>) -> Self {
        self.colony_inbox = Some(tx);
        self
    }

    /// Judges one outgoing message: either it may cross (lane, frame,
    /// address) or the answer is a `refused` receipt. Nothing is sent here.
    fn judge_out(&self, msg: &Message) -> Result<Crossing, Value> {
        let boundary = self.params.boundary.as_str();
        let route = msg.headers.hop.get("route").and_then(Value::as_str);
        let refused = |lane: &str, r: Refusal| refused_emission(lane, boundary, &r);
        // 1. Only an inline body can be projected.
        let body = match &msg.body {
            meclaw_core::Body::Inline(v) => v,
            _ => {
                let r = Refusal::new(
                    wire::LANE_BODY_UNSUPPORTED,
                    "a whole-body blob does not cross a colony boundary; the body must be \
                     inline JSON",
                );
                return Err(refused(route.unwrap_or_default(), r));
            }
        };
        // 2. A `params` slot before anything else, so it cannot pass for a lane
        //    error: every key of this cell is immutable (A9).
        if body.get("params").is_some() {
            let r = Refusal::new(INVALID_INPUT, refuse_params_update());
            return Err(refused(route.unwrap_or_default(), r));
        }
        // 3. The lane is `hop.route`, and it must be one this side emits.
        let Some(route) = route else {
            let r = Refusal::new(
                wire::LANE_UNDECLARED,
                "no hop.route on the message; the lane is what the entry edge stamps there",
            );
            return Err(refused("", r));
        };
        let Some(lane) = lanes::find_lane(&self.lanes, Direction::Emits, route) else {
            let r = Refusal::new(
                wire::LANE_UNDECLARED,
                format!("this side emits no lane {route:?}"),
            );
            return Err(refused(route, r));
        };
        // 4. No hops left: crossing would be one more. `wire::message_frame`
        //    holds the same rule; it is asked first here so the budget is
        //    judged before the body is read.
        if msg.ttl == 0
            && let Err(r) = wire::message_frame(lane, msg, Value::Null, Map::new())
        {
            return Err(refused(route, r));
        }
        // 5. Body and context, against this side's own declaration.
        let projected = lanes::project_body(lane, body).map_err(|r| refused(route, r))?;
        let context = lanes::project_context(lane, &msg.headers.context);
        let frame =
            wire::message_frame(lane, msg, projected, context).map_err(|r| refused(route, r))?;
        // 6. The address is the edge's to stamp (OR-Peer.L1b.2): without one the
        //    peer cannot be reached, and that is what the code says.
        let Some(peer_url) = msg.headers.hop.get("peer_url").and_then(Value::as_str) else {
            let r = Refusal::new(wire::PEER_UNREACHABLE, "no peer_url on the hop");
            return Err(refused(route, r));
        };
        // 6b. GH #840: only to an origin this side lists, judged here and not
        //     in the client, so the verdict falls before the token request and
        //     before any connect. `peer_url` is whatever the message's writer
        //     stamped (any edge, any `header` slot, any `/messages` caller);
        //     posting there unchecked hung `params.auth` on a request to an
        //     address nobody declared (`gh840_a_peer_url_outside_egress_is_refused.rs`
        //     counted the connects on both listeners).
        judge_egress(self.egress.as_deref(), peer_url).map_err(|r| refused(route, r))?;
        Ok(Crossing {
            lane: lane.route.clone(),
            fields: lane.fields.clone(),
            frame,
            peer_url: peer_url.to_string(),
        })
    }

    /// One POST under the operation timeout (hard rule 12), then the far
    /// side's receipt. GH #1012: `id` and `sent_ms` ride as headers, the same
    /// on every attempt of one message.
    async fn post(&self, url: &str, frame: &Value, id: &str, sent_ms: u64) -> Result<(), Refusal> {
        self.client
            .post_delivery(url, frame, id, sent_ms, self.params.external_timeout_ms)
            .await
            .and_then(|a| wire::read_receipt(&a))
    }

    /// GH #1012: hands `message` to the colony as a dead letter. Review M1: a
    /// colony inbox that is closed (the colony shutting down) is named in the
    /// log exactly like a missing one; a dead letter never vanishes without a
    /// line. The outbox row keeps its final state either way.
    async fn dead_letter(
        &self,
        message: Message,
        reason: meclaw_colony::DeadLetterReason,
        id: &str,
    ) {
        let code = reason.as_code();
        match &self.colony_inbox {
            Some(tx) => {
                if tx
                    .send(meclaw_colony::ColonyMsg::DeadLetterMessage { message, reason })
                    .await
                    .is_err()
                {
                    tracing::warn!(
                        id = %id,
                        "proxy/meclaw: a {code} message has no colony inbox to dead-letter into \
                         (the inbox is closed)"
                    );
                }
            }
            None => tracing::warn!(
                id = %id,
                "proxy/meclaw: a {code} message has no colony inbox to dead-letter into"
            ),
        }
    }

    /// GH #1012: the books exist in this life's connection (idempotent DDL,
    /// once per life). `false` when the `cell.db` cannot take them.
    async fn ensure_book(&mut self, db: &mut DbConn) -> bool {
        if !self.book_ready {
            match db.call(|c| book::setup_peer_book(c)).await {
                Ok(()) => self.book_ready = true,
                Err(e) => tracing::warn!(error = %e, "proxy/meclaw: the peer book is not there"),
            }
        }
        self.book_ready
    }

    /// GH #1012: asks the I/O half's clock for a `Retry` of `target` at `at_ms`.
    fn wake(&self, target: &str, at_ms: i64) {
        if let Some(w) = &self.wakes {
            let _ = w.send(Wake {
                target: target.to_string(),
                at_ms,
            });
        }
    }

    /// GH #1012: writes an outbox row back; a failed write is logged, the
    /// row keeps its last committed state and the next pass sees it again.
    async fn book_update(db: &mut DbConn, row: &OutRow, state: &'static str) {
        let row = row.clone();
        if let Err(e) = db.call(move |c| book::outbox_update(c, &row, state)).await {
            tracing::warn!(error = %e, "proxy/meclaw: an outbox row could not be written");
        }
    }

    /// Judges one outgoing message, books it and, when it is first in line,
    /// posts it. The answer is always one receipt: crossed, deferred or refused.
    async fn cross(&mut self, msg: &Message, db: &mut DbConn) -> Value {
        let c = match self.judge_out(msg) {
            Ok(c) => c,
            Err(receipt) => return receipt,
        };
        let boundary = self.params.boundary.clone();
        let id = Uuid::now_v7();
        // Review C1: the frame is posted as built (the 0.47.1 frame); the id
        // and this clock go into the headers of every attempt.
        let sent_ms = now_ms();
        let frame = c.frame;
        let now = i64::try_from(sent_ms).unwrap_or(i64::MAX);
        let row = OutRow {
            id: id.to_string(),
            target: c.peer_url.clone(),
            lane: c.lane.clone(),
            frame: frame.to_string(),
            envelope: envelope_json(msg),
            tries: 0,
            next_at: now,
            created_ms: now,
            deferred: false,
            last_error: None,
        };
        let booked = if self.ensure_book(db).await {
            let r = row.clone();
            db.call(move |conn| book::outbox_enqueue(conn, &r))
                .await
                .map_err(|e| e.to_string())
        } else {
            Err("the peer book is not there".to_string())
        };
        let ahead = match booked {
            Ok(ahead) => ahead,
            Err(e) => {
                // The book cannot be written: one attempt, as before #1012,
                // and the log says why nothing will be retried.
                tracing::warn!(error = %e, "proxy/meclaw: outbox unavailable, one attempt only");
                return match self.post(&c.peer_url, &frame, &row.id, sent_ms).await {
                    Ok(()) => crossed_emission(&c.lane, &boundary, &c.fields),
                    Err(r) => {
                        // OR-HV-51: nothing will retry it, so the `refused`
                        // here is final, and a final non-delivery is a dead
                        // letter too (OR-HV.X-M1.7).
                        self.dead_letter(
                            msg.clone(),
                            meclaw_colony::DeadLetterReason::PeerRefused,
                            &row.id,
                        )
                        .await;
                        refused_emission(&c.lane, &boundary, &r)
                    }
                };
            }
        };
        if let Some(code) = ahead {
            // Head of line: an older message to the same target is not through
            // yet, so this one waits behind it and the clock's next pass takes
            // both, in order. Its one `deferred` receipt is now.
            let r = Refusal::new(
                wire::code_of(&code),
                "an earlier message to the same peer is not through yet; this one waits behind \
                 it, in order",
            );
            return deferred_emission(&c.lane, &boundary, &row.id, &r);
        }
        let mut row = row;
        row.tries = 1;
        match self.post(&row.target, &frame, &row.id, sent_ms).await {
            Ok(()) => {
                Self::book_update(db, &row, book::SENT).await;
                with_delivery(
                    crossed_emission(&c.lane, &boundary, &c.fields),
                    &row.id,
                    None,
                )
            }
            Err(r) if r.transient => {
                row.next_at = i64::try_from(now_ms())
                    .unwrap_or(i64::MAX)
                    .saturating_add(BACKOFF_FIRST_MS);
                row.deferred = true;
                row.last_error = Some(r.error_code.to_string());
                Self::book_update(db, &row, book::PENDING).await;
                self.wake(
                    &row.target,
                    row.next_at
                        .min(row.created_ms.saturating_add(self.deadline_ms)),
                );
                deferred_emission(&c.lane, &boundary, &row.id, &r)
            }
            Err(r) => {
                row.last_error = Some(r.error_code.to_string());
                Self::book_update(db, &row, book::REFUSED).await;
                // OR-HV-51: a final non-delivery is a dead letter as well as
                // a receipt.
                self.dead_letter(
                    msg.clone(),
                    meclaw_colony::DeadLetterReason::PeerRefused,
                    &row.id,
                )
                .await;
                with_delivery(refused_emission(&c.lane, &boundary, &r), &row.id, None)
            }
        }
    }

    /// GH #1012: one pass over the pending rows of `target`, oldest first:
    /// expire what is past the deadline, wait while the head is not due,
    /// otherwise post, and stop at the first carrier failure (head of line).
    async fn retry(&mut self, target: String, sink: &OriginSink, db: &mut DbConn) {
        let later = i64::try_from(now_ms())
            .unwrap_or(i64::MAX)
            .saturating_add(BACKOFF_FIRST_MS);
        if !self.ensure_book(db).await {
            self.wake(&target, later);
            return;
        }
        let t = target.clone();
        let rows = match db.call(move |c| book::outbox_pending(c, &t)).await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(error = %e, "proxy/meclaw: the outbox could not be read");
                self.wake(&target, later);
                return;
            }
        };
        let boundary = self.params.boundary.clone();
        for mut row in rows {
            let now = i64::try_from(now_ms()).unwrap_or(i64::MAX);
            let deadline = row.created_ms.saturating_add(self.deadline_ms);
            if now >= deadline {
                self.expire(row, sink, db).await;
                continue;
            }
            if row.next_at > now {
                self.wake(&row.target, row.next_at.min(deadline));
                break;
            }
            let trace = envelope_trace(&row.envelope);
            let frame: Value = serde_json::from_str(&row.frame).unwrap_or(Value::Null);
            row.tries = row.tries.saturating_add(1);
            let sent_ms = u64::try_from(row.created_ms).unwrap_or(0);
            match self.post(&row.target, &frame, &row.id, sent_ms).await {
                Ok(()) => {
                    Self::book_update(db, &row, book::SENT).await;
                    let fields = lanes::find_lane(&self.lanes, Direction::Emits, &row.lane)
                        .map(|l| l.fields.clone())
                        .unwrap_or_default();
                    let waited = row
                        .deferred
                        .then(|| u64::try_from(now_ms() as i64 - row.created_ms).unwrap_or(0));
                    let content = with_delivery(
                        crossed_emission(&row.lane, &boundary, &fields),
                        &row.id,
                        waited,
                    );
                    emit(sink, &self.emit_to, content, trace).await;
                }
                Err(r) if r.transient => {
                    let wait = BACKOFF_FIRST_MS
                        .saturating_mul(1_i64 << (row.tries - 1).clamp(0, 20))
                        .min(BACKOFF_MAX_MS);
                    row.next_at = i64::try_from(now_ms())
                        .unwrap_or(i64::MAX)
                        .saturating_add(wait);
                    let first = !row.deferred;
                    row.deferred = true;
                    row.last_error = Some(r.error_code.to_string());
                    Self::book_update(db, &row, book::PENDING).await;
                    if first {
                        let content = deferred_emission(&row.lane, &boundary, &row.id, &r);
                        emit(sink, &self.emit_to, content, trace).await;
                    }
                    self.wake(&row.target, row.next_at.min(deadline));
                    break;
                }
                Err(r) => {
                    row.last_error = Some(r.error_code.to_string());
                    Self::book_update(db, &row, book::REFUSED).await;
                    // OR-HV-51, as in `cross`.
                    self.dead_letter(
                        envelope_message(&row.envelope),
                        meclaw_colony::DeadLetterReason::PeerRefused,
                        &row.id,
                    )
                    .await;
                    let content =
                        with_delivery(refused_emission(&row.lane, &boundary, &r), &row.id, None);
                    emit(sink, &self.emit_to, content, trace).await;
                }
            }
        }
    }

    /// GH #1012 part e: the deadline ran out. The row moves to `expired`, the
    /// message becomes a `peer_expired` dead letter and an `expired` receipt.
    async fn expire(&self, row: OutRow, sink: &OriginSink, db: &mut DbConn) {
        Self::book_update(db, &row, book::EXPIRED).await;
        self.dead_letter(
            envelope_message(&row.envelope),
            meclaw_colony::DeadLetterReason::PeerExpired,
            &row.id,
        )
        .await;
        let content = expired_emission(
            &row.lane,
            &self.params.boundary,
            &row.id,
            row.tries,
            row.last_error.as_deref(),
            self.params.peer_retry_deadline_s,
        );
        emit(sink, &self.emit_to, content, envelope_trace(&row.envelope)).await;
    }
}

/// GH #1012: the outgoing message as the outbox keeps it, for the dead letter
/// an expired row becomes (the shape of the colony's own DLQ envelope).
fn envelope_json(m: &Message) -> String {
    let body = match &m.body {
        Body::Inline(v) => v.clone(),
        Body::Blob(u) => Value::String(u.to_string()),
    };
    serde_json::json!({
        "id": m.id.to_string(),
        "trace_id": m.trace_id.to_string(),
        "parent_message_id": m.parent_message_id.map(|u| u.to_string()),
        "correlation_id": m.correlation_id.map(|u| u.to_string()),
        "target": m.target.as_str(),
        "reply_to": m.reply_to.as_ref().map(|p| p.as_str()),
        "ttl": m.ttl,
        "headers": m.headers,
        "body": body,
        "created_at": m.created_at,
    })
    .to_string()
}

/// The trace an outbox row's receipts are carried on.
fn envelope_trace(envelope: &str) -> Option<(Uuid, u32)> {
    let v: Value = serde_json::from_str(envelope).ok()?;
    let trace = Uuid::parse_str(v.get("trace_id")?.as_str()?).ok()?;
    Some((trace, meclaw_core::MESSAGE_DEFAULT_TTL))
}

/// The inverse of [`envelope_json`]: each field read defensively, as the
/// colony's DLQ drain reads its own rows.
fn envelope_message(envelope: &str) -> Message {
    let v: Value = serde_json::from_str(envelope).unwrap_or(Value::Null);
    let uuid = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .and_then(|s| Uuid::parse_str(s).ok())
    };
    let headers: Headers = v
        .get("headers")
        .cloned()
        .and_then(|h| serde_json::from_value(h).ok())
        .unwrap_or_default();
    Message {
        id: uuid("id").unwrap_or_else(Uuid::nil),
        trace_id: uuid("trace_id").unwrap_or_else(Uuid::nil),
        parent_message_id: uuid("parent_message_id"),
        correlation_id: uuid("correlation_id"),
        target: Path::new(v.get("target").and_then(Value::as_str).unwrap_or("")),
        reply_to: v.get("reply_to").and_then(Value::as_str).map(Path::new),
        ttl: v
            .get("ttl")
            .and_then(Value::as_u64)
            .and_then(|t| u32::try_from(t).ok())
            .unwrap_or(0),
        headers,
        body: Body::Inline(v.get("body").cloned().unwrap_or(Value::Null)),
        created_at: v.get("created_at").and_then(Value::as_i64).unwrap_or(0),
    }
}

impl LongRunningCell for MeclawCell {
    type Event = PeerEvent;
    type Reconfig = PeerReconfig;
    type Io = PeerIo;

    /// A second call finds no mount half and gets one on a table of its own:
    /// it serves nobody, which is the honest answer to a wiring nobody made.
    /// GH #1012: each call opens the way to the outbox clock of that half.
    fn split_io(&mut self) -> Self::Io {
        let io = self.initial_io_cfg.take().unwrap_or_else(|| {
            MeclawIo::new(
                &self.params,
                "",
                Arc::new(meclaw_colony::SurfaceRegistry::new()),
            )
        });
        let (tx, rx) = mpsc::unbounded_channel();
        self.wakes = Some(tx);
        PeerIo {
            io,
            wakes: Some(rx),
        }
    }

    /// The mount half; see [`run_peer_io`]. `clippy::manual_async_fn`: the same
    /// stable false positive as `crate::proxy::cell::ProxyCell::run_io`.
    #[allow(clippy::manual_async_fn)]
    fn run_io(
        io: Self::Io,
        events_tx: mpsc::Sender<Self::Event>,
        reconfig_rx: mpsc::Receiver<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send {
        run_peer_io(io, events_tx, reconfig_rx)
    }

    /// The outgoing direction: one receipt per message, crossed, deferred or
    /// refused.
    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a mut self,
        msg: Message,
        sink: &'a OutputSink,
        db: &'a mut DbConn,
        _reconfig_tx: &'a mpsc::Sender<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            let content = self.cross(&msg, db).await;
            let _ = sink
                .push(CellOutput {
                    target: msg.target.clone(),
                    content,
                })
                .await;
        }
    }

    /// The incoming direction: the mount has judged, this only emits. An
    /// accepted frame leaves two emissions, the arrival and this side's own
    /// `crossed` receipt. With the ingress handle
    /// (`contract.ingress.carries_trace`) both, and a refusal of a parsed frame,
    /// carry the frame's trace; without it every emission is an ordinary source
    /// emission with a fresh trace. GH #1012: an arrival from the inbox moves
    /// its row to `done` once both are emitted, and a `Retry` is one outbox
    /// pass.
    #[allow(clippy::manual_async_fn)]
    fn handle_event<'a>(
        &'a mut self,
        event: Self::Event,
        sink: &'a OriginSink,
        db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            let boundary = self.params.boundary.clone();
            let boundary = boundary.as_str();
            match event {
                PeerEvent::Arrived {
                    lane,
                    peer,
                    trace_id,
                    ttl,
                    fields,
                    context,
                    claims,
                    body,
                    delivery,
                } => {
                    let mut receipt = crossed_emission(&lane, boundary, &fields);
                    if let Some(id) = delivery.frame_id {
                        receipt = with_delivery(receipt, &id.to_string(), None);
                    }
                    let mut content =
                        arrived_emission(&lane, &peer, boundary, &context, &claims, body);
                    stamp_arrival(&mut content, &delivery);
                    // GH #1015: the inbox key is durable — the colony derives the
                    // hand-on's ids from it, and drops a repeat it already logged.
                    if let Some(key) = delivery.inbox_key.as_deref() {
                        stamp_delivery_key(
                            &mut content,
                            key,
                            delivery.arrived_ms,
                            delivery.replayed,
                        );
                        stamp_delivery_key(
                            &mut receipt,
                            &format!("{key}:receipt"),
                            delivery.arrived_ms,
                            delivery.replayed,
                        );
                    }
                    // `route()` takes one hop from every input budget. The
                    // frame's `ttl` is already the budget AFTER the crossing
                    // (`wire::message_frame` took that hop), so the input handed
                    // on is one more: the crossing and the delivery to `emit_to`
                    // are one hop, as one edge inside a colony would be
                    // (OR-Peer.L1b.4; L4 case 4 reads exactly one less).
                    let budget = ttl.saturating_add(1);
                    emit(sink, &self.emit_to, content, Some((trace_id, budget))).await;
                    // R-26-17 point 4: a crossing leaves a receipt on BOTH sides.
                    // The far side read this verdict on the wire; this side books
                    // it under its own name, on the frame's trace, with the budget
                    // its sink would give, like a refusal (OR-Peer9, OR-Peer7).
                    let carried = Some((trace_id, meclaw_core::MESSAGE_DEFAULT_TTL));
                    emit(sink, &self.emit_to, receipt, carried).await;
                    // GH #1015: handed on — booked `done` only once it is older
                    // than `HAND_ON_SETTLE` (see there). Until then a crash raises
                    // it again, and the colony drops the repeat by its key.
                    if let Some(key) = delivery.inbox_key {
                        self.handed.push_back((key, std::time::Instant::now()));
                    }
                    let mut settled = Vec::new();
                    while let Some((_, at)) = self.handed.front()
                        && at.elapsed() >= HAND_ON_SETTLE
                    {
                        if let Some((key, _)) = self.handed.pop_front() {
                            settled.push(key);
                        }
                    }
                    if !settled.is_empty()
                        && self.ensure_book(db).await
                        && let Err(e) = db
                            .call(move |c| {
                                for key in &settled {
                                    book::inbox_done(c, key)?;
                                }
                                Ok::<(), rusqlite::Error>(())
                            })
                            .await
                    {
                        tracing::warn!(error = %e, "proxy/meclaw: inbox rows stay pending");
                    }
                }
                PeerEvent::Refused {
                    lane,
                    trace_id,
                    error_code,
                    detail,
                    ..
                } => {
                    let content =
                        refused_emission(&lane, boundary, &Refusal::new(error_code, detail));
                    // A refusal carries no budget of its own: the one its sink
                    // would give (OR-Peer7).
                    let carried = trace_id.map(|t| (t, meclaw_core::MESSAGE_DEFAULT_TTL));
                    emit(sink, &self.emit_to, content, carried).await;
                }
                PeerEvent::MountFailed(e) => {
                    let content = refused_emission("", boundary, &Refusal::new(INVALID_INPUT, e));
                    emit(sink, &self.emit_to, content, None).await;
                }
                PeerEvent::Retry { target } => self.retry(target, sink, db).await,
            }
        }
    }
}

/// GH #840: `Ok` when `peer_url` is an `http(s)` URL without credentials
/// whose origin `egress` lists. Without a list nothing goes out (R-SN-6,
/// fail-closed), and the detail is the migration line. A URL with credentials
/// in it is not echoed (the `auth.token_url` precedent).
fn judge_egress(egress: Option<&[String]>, peer_url: &str) -> Result<(), Refusal> {
    let parsed = reqwest::Url::parse(peer_url);
    let url = parsed
        .as_ref()
        .ok()
        .filter(|u| matches!(u.scheme(), "http" | "https") && u.host().is_some());
    let Some(url) = url else {
        // Review G, Minor 1: userinfo is checked below only for http(s), so
        // this arm never echoes the string itself. A parsed URL is named by
        // scheme and host, which carry no credentials; an unparsable one only
        // when it holds no `@` (`ftp://id:pw@host` and `http://id:pw@[x`
        // echoed the password into the receipt until this fix).
        let shown = match &parsed {
            Ok(u) => match u.host_str() {
                Some(h) => format!("(scheme {:?}, host {h:?})", u.scheme()),
                None => format!("(scheme {:?})", u.scheme()),
            },
            Err(_) => super::params::quoted_url(peer_url),
        };
        return Err(Refusal::new(
            wire::EGRESS_DENIED,
            format!(
                "hop.peer_url {shown} is not an http or https URL, so it cannot be judged \
                 against params.egress; nothing was sent"
            ),
        ));
    };
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Refusal::new(
            wire::EGRESS_DENIED,
            "hop.peer_url carries credentials in the URL; it is not echoed, and nothing was sent",
        ));
    }
    let origin = url.origin().ascii_serialization();
    match egress {
        None => Err(Refusal::new(
            wire::EGRESS_DENIED,
            format!(
                "this cell declares no params.egress, and without it a peer cell sends nothing \
                 out (fail-closed since 0.46.0); list the origin of the gateway in front of the \
                 peer, for example \"egress\": [\"https://<gateway-origin>\"]; this message \
                 was addressed to {origin}"
            ),
        )),
        Some(list) if list.contains(&origin) => Ok(()),
        Some(list) => Err(Refusal::new(
            wire::EGRESS_DENIED,
            format!(
                "the origin {origin} of hop.peer_url is not in params.egress [{}]; nothing was \
                 sent",
                list.join(", ")
            ),
        )),
    }
}

/// One emission from the incoming side: carried when there is a trace to carry
/// and the cell declared the ingress handle, an ordinary source emission
/// otherwise.
async fn emit(sink: &OriginSink, target: &Path, content: Value, carried: Option<(Uuid, u32)>) {
    let out = CellOutput {
        target: target.clone(),
        content,
    };
    match (carried, sink.ingress()) {
        (Some((trace_id, ttl)), Some(ingress)) => {
            if let Err(e) = ingress.emit(out, trace_id, ttl).await {
                tracing::warn!(error = %e, "proxy/meclaw: an ingress emission was refused");
            }
        }
        _ => {
            let _ = sink.emit(out).await;
        }
    }
}

/// GH #1015: stamp the hand-on's durable key (and whether it is a repeat from
/// the inbox) into the emission's hop, where the colony reads it.
/// `arrived_ms` is stored with the inbox row, so a repeat stamps the same
/// time and the colony derives the same ids (their v7 time prefix, when the
/// key is not itself a UUID).
fn stamp_delivery_key(content: &mut Value, key: &str, arrived_ms: u64, replayed: bool) {
    if let Some(h) = content.get_mut("header").and_then(Value::as_object_mut) {
        h.insert(
            meclaw_colony::DELIVERY_KEY_HEADER.to_string(),
            Value::String(key.to_string()),
        );
        h.insert(
            meclaw_colony::DELIVERY_KEY_MS_HEADER.to_string(),
            Value::from(arrived_ms),
        );
        if replayed {
            h.insert(
                meclaw_colony::DELIVERY_REPLAY_HEADER.to_string(),
                Value::Bool(true),
            );
        }
    }
}
