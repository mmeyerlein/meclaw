//! The peer mount: one route, one POST, one receipt.
//!
//! The colony's one listener decides a connection by its first path segment and
//! hands the stream, unread, to the cell that registered the name
//! (`crate::handed::serve_handed`). This module is what answers it: exactly one
//! route, `POST /<mount>/`, whose body is a wire-version-1 frame and whose
//! answer is the receipt.
//!
//! The whole verdict falls here, from the pure functions of `lanes` and `wire`:
//! acceptance is the last thing the boundary can report, and what the edge table
//! does with an arrival afterwards never reaches the cell. The handler half only
//! emits. The one thing an answer waits for is the bounded send to that half,
//! so backpressure crosses as slowness and, at worst, as the far side's
//! `peer_timeout`.
//!
//! GH #1012: an accepted frame is committed to the inbox (`book::peer_inbox`)
//! BEFORE the answer, so a `200` is a promise the next life keeps: a crash
//! between the answer and the handler's work is replayed at the start. The
//! delivery id and the sender's clock come from the request headers
//! `X-Meclaw-Frame-Id` / `X-Meclaw-Sent-Ms` (`wire::read_delivery`), never
//! from the frame, which stays the 0.47.1 frame (review C1). The inbox key is
//! the sender and the id (review M5): an id already booked FOR THAT SENDER is
//! answered `crossed` with `duplicate: true` and raises nothing; a request
//! without an id is booked under a local key and never deduplicated. A mount
//! whose inbox cannot be written answers `503`, which the sender retries.
//!
//! Commit and hand-on are one step of the inbox writer (`io::run_inbox`), not
//! of the request (review C2): hyper drops a request future whose client went
//! away, and a hand-on inside it could be dropped after the commit, leaving a
//! committed row nobody raised until the next start while every retry read
//! `duplicate`. Once the write is queued, the writer commits it and hands it
//! on whether or not anyone still waits for the answer, in commit order.

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header::CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use meclaw_colony::SurfaceRegistry;
use meclaw_core::Uuid;
use serde_json::{Map, Value};
use std::sync::Arc;
use tokio::sync::mpsc;

use super::lanes::{self, Direction, Refusal};
use super::params::{Lanes, MeclawParams};
use super::wire;
use meclaw_colony::surfaces::ProxyNet;

/// What the mount half needs: its name, whom it trusts to name the sender, its
/// own boundary name, the contract, the mount table and the way to the handler.
#[derive(Clone)]
pub struct MeclawIo {
    pub(crate) mount: String,
    pub(crate) identity_header: String,
    /// GH #833: the proxies whose `identity_header` is believed.
    pub(crate) trusted: Arc<Vec<ProxyNet>>,
    /// GH #833: whether the connection this copy serves came from an address
    /// in [`Self::trusted`]. Set per connection by [`Self::for_connection`] at
    /// the handoff — the peer address is the connection's, decided once
    /// (O-639-6) — and `false` on a copy no connection has reached.
    pub(crate) peer_trusted: bool,
    pub(crate) boundary: String,
    pub(crate) cell_path: String,
    pub(crate) lanes: Arc<Lanes>,
    pub(crate) surfaces: Arc<SurfaceRegistry>,
    /// Set by `run_io`, because the channel exists only there.
    pub(crate) events_tx: Option<mpsc::Sender<PeerEvent>>,
    /// GH #1012: the cell's own `cell.db`, home of the inbox. `None` keeps the
    /// pre-#1012 mount (answer once the event is queued); the factory always
    /// sets it.
    pub(crate) cell_db: Option<std::path::PathBuf>,
    /// GH #1012: the A-timeout around an inbox write (`query_timeout_ms`).
    pub(crate) query_timeout_ms: u64,
    /// GH #1012: the inbox writer, set by `run_io` once the book is open.
    pub(crate) inbox: Option<mpsc::Sender<InboxWrite>>,
}

/// GH #1012: one inbox write, answered once the row is committed AND, for a
/// new row, the arrival handed on: `Ok(true)` for a new row, `Ok(false)` for
/// an id already booked (nothing handed on), `Err` when the row could not be
/// written.
#[derive(Debug)]
pub struct InboxWrite {
    pub(crate) key: String,
    pub(crate) frame: String,
    pub(crate) at_ms: i64,
    /// The arrival the writer hands on after the commit (review C2).
    pub(crate) event: PeerEvent,
    pub(crate) done: tokio::sync::oneshot::Sender<Result<bool, String>>,
}

/// GH #1012: what one arrival carries besides its body: the sender's id and
/// clock, this side's clock, and the inbox key the handler books it under.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Delivery {
    /// The frame's `id`, when the sender set one.
    pub frame_id: Option<Uuid>,
    /// The frame's `sent_ms`, when the sender set one.
    pub sent_ms: Option<u64>,
    /// This side's clock (Unix ms) when the frame was judged.
    pub arrived_ms: u64,
    /// The inbox row of this arrival (the frame id, or a local key for a frame
    /// without one); `None` on a mount without an inbox.
    pub inbox_key: Option<String>,
}

/// GH #1012: the handler half asks the I/O half to deliver a `Retry` for
/// `target` at `at_ms` (Unix ms). The earliest wake per target wins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wake {
    /// The `hop.peer_url` whose outbox queue is due.
    pub target: String,
    /// When, in Unix ms.
    pub at_ms: i64,
}

/// Unix ms of this side's clock.
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

impl MeclawIo {
    /// The mount half of the cell at `cell_path`, registering on `surfaces`.
    pub fn new(p: &MeclawParams, cell_path: &str, surfaces: Arc<SurfaceRegistry>) -> Self {
        Self {
            mount: p.mount.clone(),
            identity_header: p.identity_header.clone(),
            trusted: Arc::new(p.trusted()),
            peer_trusted: false,
            boundary: p.boundary.clone(),
            cell_path: cell_path.to_string(),
            lanes: Arc::new(p.lanes.clone()),
            surfaces,
            events_tx: None,
            cell_db: None,
            query_timeout_ms: p.query_timeout_ms,
            inbox: None,
        }
    }

    /// GH #1012: keep the inbox in the cell's own `cell.db` at `path`.
    pub fn with_cell_db(mut self, path: std::path::PathBuf) -> Self {
        self.cell_db = Some(path);
        self
    }

    /// The state one handed connection is served with: this mount's state,
    /// plus whether the connection's peer address is a trusted proxy.
    pub(crate) fn for_connection(&self, peer: std::net::IpAddr) -> Self {
        Self {
            peer_trusted: meclaw_colony::surfaces::admits(&self.trusted, peer),
            ..self.clone()
        }
    }
}

/// What the mount tells the handler half. The verdict has already fallen.
#[derive(Debug)]
pub enum PeerEvent {
    /// A frame crossed: projected body and context, the trace and the budget it
    /// carried, and the sender the proxy in front named.
    Arrived {
        /// The lane it crossed on.
        lane: String,
        /// The sending colony, from the identity header and nowhere else.
        peer: String,
        /// The trace the frame carried.
        trace_id: Uuid,
        /// The hops the frame had left after its crossing.
        ttl: u32,
        /// The fields this side's lane let cross; this side's `crossed`
        /// receipt names them (OR-Peer9).
        fields: Vec<String>,
        /// The `context` keys the lane lets cross.
        context: Map<String, Value>,
        /// What the sender claimed about its turns — `peer_origins` and
        /// `peer_speakers` (GH #847, [`lanes::restamp_as_peer`]); empty for a
        /// body without turns.
        claims: Map<String, Value>,
        /// The projected body, every turn stamped `origin: "peer"`.
        body: Value,
        /// GH #1012: id, clocks and inbox key of this arrival.
        delivery: Delivery,
    },
    /// A frame was refused; the far side read the same verdict on the wire.
    Refused {
        /// The lane the frame named, or empty when it named none readable.
        lane: String,
        /// The sender, when the header named one.
        peer: Option<String>,
        /// The frame's trace, when the frame parsed (OR-Peer.L1b.3).
        trace_id: Option<Uuid>,
        /// One of the eleven codes.
        error_code: &'static str,
        /// What was refused, in words.
        detail: String,
    },
    /// The name could not be taken; the cell stays up and serves nobody.
    MountFailed(String),
    /// GH #1012: the outbox queue for this target is due another attempt.
    Retry {
        /// The `hop.peer_url` whose queue is due.
        target: String,
    },
}

/// Nothing travels on the reconfig channel: `params` of a `meclaw` proxy are
/// immutable, so the channel closing is the whole signal.
#[derive(Debug)]
pub enum PeerReconfig {}

/// The router a handed connection is served with: exactly one route.
///
/// Registered as `/<mount>/` rather than nested: `nest` does not answer the
/// prefix WITH a trailing slash (see `web::io::get_root`), and `/<mount>/` is
/// the one URL a peer is configured with.
pub(crate) fn mounted_router(io: MeclawIo) -> Router {
    let mount = io.mount.clone();
    Router::new()
        .route(&format!("/{mount}/"), axum::routing::post(post_frame))
        .with_state(io)
}

/// `POST /<mount>/`: judge the frame, tell the handler half, answer the receipt.
async fn post_frame(State(io): State<MeclawIo>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(events_tx) = io.events_tx.clone() else {
        // Only reachable if a router were built outside `run_io`.
        return (StatusCode::SERVICE_UNAVAILABLE, "no handler\n").into_response();
    };
    let (mut receipt, mut event) = judge(&io, io.peer_trusted, &headers, &body);
    // GH #1012: an arrival is committed before it is answered.
    let booking = match (io.inbox.as_ref(), &mut event) {
        (Some(inbox), PeerEvent::Arrived { peer, delivery, .. }) => {
            // Review M5: the key is the sender AND its id, so two senders
            // that happen to pick one id never swallow each other's frame.
            // A request without an id gets a local key: booked and replayed
            // like any other, but never equal to another's, so never a
            // duplicate.
            let key = match delivery.frame_id {
                Some(id) => format!("{peer}:{id}"),
                None => Uuid::now_v7().to_string(),
            };
            delivery.inbox_key = Some(key.clone());
            let at_ms = i64::try_from(delivery.arrived_ms).unwrap_or(i64::MAX);
            Some((inbox.clone(), key, at_ms))
        }
        _ => None,
    };
    if let Some((inbox, key, at_ms)) = booking {
        let frame = arrival_to_json(&event);
        let (done, answer) = tokio::sync::oneshot::channel();
        // Review C2: from here on the writer owns the arrival. If this
        // request is dropped (the sender gave up), the queued write is still
        // committed and handed on.
        let write = InboxWrite {
            key,
            frame,
            at_ms,
            event,
            done,
        };
        let booked = match inbox.send(write).await {
            Ok(()) => answer
                .await
                .unwrap_or_else(|_| Err("the inbox writer stopped".into())),
            Err(_) => Err("the inbox writer stopped".into()),
        };
        match booked {
            // Committed and handed on by the writer; a closed channel does
            // not withhold the `200`, the next life replays the row.
            Ok(true) => {}
            Ok(false) => {
                if let Some(obj) = receipt.as_object_mut() {
                    obj.insert("duplicate".to_string(), Value::Bool(true));
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "proxy/meclaw: the inbox write failed");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "the inbox could not be written\n",
                )
                    .into_response();
            }
        }
        return (
            StatusCode::OK,
            [(CONTENT_TYPE, "application/json")],
            receipt.to_string(),
        )
            .into_response();
    }
    // Both ways: the far side reads the receipt, this side keeps its own. An
    // answer the handler could not take would be a receipt nobody here holds,
    // so it is not given: the far side reads a non-200, i.e. `peer_unreachable`.
    if events_tx.send(event).await.is_err() {
        return (StatusCode::SERVICE_UNAVAILABLE, "the cell is going away\n").into_response();
    }
    (
        StatusCode::OK,
        [(CONTENT_TYPE, "application/json")],
        receipt.to_string(),
    )
        .into_response()
}

/// The five checks, in this order: sender, parse (the frame, then the
/// delivery headers), lane, budget, body (its fields, then whether what is
/// left can be delivered at all).
///
/// `peer_trusted` is the connection's verdict, passed in rather than looked
/// up, so the whole judgement stays a pure function of its arguments. The
/// parameter is the one that counts: `judge` never reads `io.peer_trusted`,
/// and a caller that holds a different verdict than the copy it passes is
/// judged by the parameter.
fn judge(
    io: &MeclawIo,
    peer_trusted: bool,
    headers: &HeaderMap,
    body: &[u8],
) -> (Value, PeerEvent) {
    // (0) Who sent it: only the header the proxy fills. An empty param and a
    // missing header are the same refusal (fail-closed; this is where the cell
    // departs from `web`, which stamps nothing and carries on).
    let claimed = identity_of(headers, &io.identity_header);
    let Some(peer) = claimed.as_ref().filter(|_| peer_trusted).cloned() else {
        // GH #833: a header on a connection from outside `trusted_proxies` is
        // a line any client that reaches the port can write — a direct POST
        // with a forged sender arrived as that sender until 0.45.0. It counts
        // as missing. The detail says which of the two it was, so an operator
        // whose proxy on another host is not listed reads the list, not the
        // header; the value itself is never echoed, and no sender is booked
        // (OR-AG-5: `invalid_frame`, no new code).
        let detail = if claimed.is_some() {
            "the sender header came from an address this mount does not trust as a proxy \
             (params.trusted_proxies)"
        } else {
            "no authenticated sender header on this mount"
        };
        let r = Refusal::new(wire::INVALID_FRAME, detail);
        return refuse(io, String::new(), None, None, r, None);
    };
    // (1) Parse: JSON first, then the frame itself (`v` before anything else).
    let raw: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => {
            let r = Refusal::new(wire::INVALID_FRAME, format!("frame: not JSON: {e}"));
            return refuse(io, String::new(), Some(peer), None, r, None);
        }
    };
    let frame = match wire::parse_message_frame(&raw) {
        Ok(f) => f,
        Err(r) => {
            // Echo only: a lane name the frame could not stand behind.
            let lane = raw
                .get("lane")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            return refuse(io, lane, Some(peer), None, r, None);
        }
    };
    let trace = Some(frame.trace_id);
    // (1a) GH #1012, review C1: the delivery id and the sender's clock, from
    // the headers. Missing is the old way (no dedup); unreadable is a frame
    // that does not parse.
    let delivery_header = |name: &'static str| match headers.get(name) {
        None => Ok(None),
        Some(v) => v.to_str().map(Some).map_err(|_| {
            Refusal::new(
                wire::INVALID_FRAME,
                format!("header {name}: not visible ASCII"),
            )
        }),
    };
    let read = delivery_header(wire::FRAME_ID_HEADER).and_then(|id| {
        delivery_header(wire::SENT_MS_HEADER).and_then(|ms| wire::read_delivery(id, ms))
    });
    let (frame_id, sent_ms) = match read {
        Ok(d) => d,
        Err(r) => return refuse(io, frame.lane, Some(peer), trace, r, None),
    };
    // (2) Lane: this side's own declaration, never the other side's.
    let Some(lane) = lanes::find_lane(&io.lanes, Direction::Accepts, &frame.lane) else {
        let r = Refusal::new(
            wire::LANE_UNDECLARED,
            format!("this side accepts no lane {:?}", frame.lane),
        );
        return refuse(io, frame.lane, Some(peer), trace, r, None);
    };
    // (3) Budget, at the frame and before any emitter is called (A4).
    if frame.ttl == 0 {
        let r = Refusal::new(
            wire::TTL_EXHAUSTED,
            "the frame arrived with no hops left; delivering it would have been one more",
        );
        return refuse(
            io,
            lane.route.clone(),
            Some(peer),
            trace,
            r,
            Some(&lane.because),
        );
    }
    // (4) Body form and fields, against this side's declaration.
    let projected = match lanes::project_body(lane, &frame.body) {
        Ok(b) => b,
        Err(r) => {
            return refuse(
                io,
                lane.route.clone(),
                Some(peer),
                trace,
                r,
                Some(&lane.because),
            );
        }
    };
    // (4a) GH #847 / R-SN-3: every arriving turn is `peer`; what the sender
    // claimed moves to the hop. Before the deliverability check, because a
    // `speaker` the sender left on a turn it calls `user` is no valid body —
    // moved out, the turn is.
    let (projected, claims) = lanes::restamp_as_peer(projected);
    // (4b) What is accepted must be deliverable. A debug colony dead-letters an
    // emission whose body is no UBF body (`invalid_ubf_body`), a release colony
    // passes it on unchecked, and either way that happens after the cell has
    // answered: a projection with no central slot left (a foreign sender
    // without `messages`) was answered `crossed` and never reached the sink
    // (gh617 proof, `a_frame_whose_body_has_no_central_slot_is_refused_not_lost`).
    // Judged here with the colony's own validator, in both builds, it becomes
    // a refusal both sides book.
    if let Some(r) = undeliverable(&projected) {
        return refuse(
            io,
            lane.route.clone(),
            Some(peer),
            trace,
            r,
            Some(&lane.because),
        );
    }
    let context = lanes::project_context(lane, &frame.context);
    let receipt = wire::crossed_receipt(&lane.route, &io.boundary, &lane.fields);
    let delivery = Delivery {
        frame_id,
        sent_ms,
        arrived_ms: now_ms(),
        inbox_key: None,
    };
    let event = PeerEvent::Arrived {
        lane: lane.route.clone(),
        peer,
        trace_id: frame.trace_id,
        ttl: frame.ttl,
        fields: lane.fields.clone(),
        context,
        claims,
        body: projected,
        delivery,
    };
    (receipt, event)
}

/// GH #1012: an arrival as the inbox keeps it. The verdict has fallen, so the
/// row holds the judged arrival, not the raw frame: a replay re-emits exactly
/// what this life would have emitted.
pub(crate) fn arrival_to_json(event: &PeerEvent) -> String {
    let PeerEvent::Arrived {
        lane,
        peer,
        trace_id,
        ttl,
        fields,
        context,
        claims,
        body,
        delivery,
    } = event
    else {
        return String::new();
    };
    serde_json::json!({
        "lane": lane, "peer": peer, "trace_id": trace_id.to_string(), "ttl": ttl,
        "fields": fields, "context": context, "claims": claims, "body": body,
        "frame_id": delivery.frame_id.map(|u| u.to_string()),
        "sent_ms": delivery.sent_ms, "arrived_ms": delivery.arrived_ms,
    })
    .to_string()
}

/// GH #1012: the inverse of [`arrival_to_json`], for the replay at the start;
/// `None` for a row this build cannot read (it stays `pending` and is named in
/// the log, never dropped silently).
pub(crate) fn arrival_from_json(key: &str, row: &str) -> Option<PeerEvent> {
    let v: Value = serde_json::from_str(row).ok()?;
    let str_of = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let obj_of = |k: &str| {
        v.get(k)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
    };
    Some(PeerEvent::Arrived {
        lane: str_of("lane")?,
        peer: str_of("peer")?,
        trace_id: Uuid::parse_str(&str_of("trace_id")?).ok()?,
        ttl: u32::try_from(v.get("ttl")?.as_u64()?).ok()?,
        fields: serde_json::from_value(v.get("fields")?.clone()).ok()?,
        context: obj_of("context"),
        claims: obj_of("claims"),
        body: v.get("body")?.clone(),
        delivery: Delivery {
            frame_id: str_of("frame_id").and_then(|s| Uuid::parse_str(&s).ok()),
            sent_ms: v.get("sent_ms").and_then(Value::as_u64),
            arrived_ms: v.get("arrived_ms").and_then(Value::as_u64).unwrap_or(0),
            inbox_key: Some(key.to_string()),
        },
    })
}

/// The UBF central slots; a body must carry at least one of them.
const CENTRAL_SLOTS: [&str; 3] = ["system", "messages", "attachments"];

/// `invalid_frame` when the projected body is no UBF body, judged by the same
/// validator the colony applies to every emission. The detail names the slots
/// and never a value: the validator's own message quotes the body it rejects,
/// and the body is exactly what a refusal must not repeat.
fn undeliverable(projected: &Value) -> Option<Refusal> {
    meclaw_core::validate_ubf_body(projected).err()?;
    let present: Vec<&str> = CENTRAL_SLOTS
        .into_iter()
        .filter(|k| projected.get(*k).is_some())
        .collect();
    let detail = if present.is_empty() {
        format!(
            "the body carries none of the central slots ({}); it would reach nobody, so it \
             is not accepted",
            CENTRAL_SLOTS.join(", ")
        )
    } else {
        format!(
            "the central slot(s) {} do not form a valid body; it would reach nobody, so it is \
             not accepted",
            present.join(", ")
        )
    };
    Some(Refusal::new(wire::INVALID_FRAME, detail))
}

/// One refusal, both ways: the receipt for the wire and the event for home.
fn refuse(
    io: &MeclawIo,
    lane: String,
    peer: Option<String>,
    trace_id: Option<Uuid>,
    r: Refusal,
    because: Option<&str>,
) -> (Value, PeerEvent) {
    let receipt = wire::refused_receipt(&lane, &io.boundary, &r, because);
    let event = PeerEvent::Refused {
        lane,
        peer,
        trace_id,
        error_code: r.error_code,
        detail: r.detail,
    };
    (receipt, event)
}

/// The sender the reverse proxy named, if the operator named a header and the
/// request carries a non-empty value in it. A local copy of `web`'s reader
/// (which is private to that cell); unlike `web`, `None` here is a refusal.
fn identity_of(headers: &HeaderMap, identity_header: &str) -> Option<String> {
    if identity_header.is_empty() {
        return None;
    }
    headers
        .get(identity_header)
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn io() -> MeclawIo {
        let p = MeclawParams::parse(&json!({
            "platform": "meclaw", "mount": "peer", "identity_header": "X-Meclaw-Peer",
            "boundary": "south", "emit_to": "/sink", "lanes": {
                "accepts": [{"route": "topic", "fields": ["topic"], "because": "a subject"}],
                "emits": []}
        }))
        .expect("params");
        MeclawIo::new(&p, "/friend", Arc::new(SurfaceRegistry::new()))
    }

    fn signed() -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("x-meclaw-peer", "north".parse().expect("a header"));
        h
    }

    const FRAME: &[u8] = br#"{"v": 1, "type": "message", "lane": "topic",
        "trace_id": "0192f1b0-0000-7000-8000-000000000001", "ttl": 5, "context": {},
        "body": {"messages": [], "topic": "gardening"}}"#;

    /// GH #833: the same signed frame, judged twice — the connection's verdict
    /// is the only input that differs, and no HTTP is involved.
    #[test]
    fn a_signed_frame_crosses_only_on_a_trusted_connection() {
        let io = io();
        let (receipt, event) = judge(&io, true, &signed(), FRAME);
        assert_eq!(receipt["result"], json!("crossed"), "{receipt}");
        assert!(matches!(event, PeerEvent::Arrived { ref peer, .. } if peer == "north"));

        let (receipt, event) = judge(&io, false, &signed(), FRAME);
        assert_eq!(receipt["error_code"], json!(wire::INVALID_FRAME));
        match event {
            PeerEvent::Refused { peer, detail, .. } => {
                assert_eq!(peer, None, "a header nobody vouched for names nobody");
                assert!(detail.contains("params.trusted_proxies"), "{detail}");
                assert!(
                    !detail.contains("north"),
                    "the claimed value is never echoed"
                );
            }
            o => panic!("expected a refusal, got {o:?}"),
        }
    }

    /// No header at all keeps its old detail, trusted or not: the list is not
    /// the reason a frame nobody signed is refused.
    #[test]
    fn an_unsigned_frame_keeps_the_old_detail_wherever_it_comes_from() {
        let io = io();
        for trusted in [true, false] {
            let (receipt, _) = judge(&io, trusted, &HeaderMap::new(), FRAME);
            assert_eq!(
                receipt["detail"],
                json!("no authenticated sender header on this mount"),
                "trusted = {trusted}"
            );
        }
    }

    #[test]
    fn a_copy_no_connection_reached_trusts_nobody_and_the_handoff_decides() {
        let io = io();
        assert!(!io.peer_trusted, "fail-closed before any handoff");
        assert!(
            io.for_connection("127.0.0.1".parse().expect("ip"))
                .peer_trusted
        );
        assert!(
            io.for_connection("::ffff:127.0.0.9".parse().expect("ip"))
                .peer_trusted
        );
        assert!(
            !io.for_connection("192.0.2.1".parse().expect("ip"))
                .peer_trusted
        );
    }
}
