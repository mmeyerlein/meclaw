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

/// What the mount half needs: its name, whom it trusts to name the sender, its
/// own boundary name, the contract, the mount table and the way to the handler.
#[derive(Clone)]
pub struct MeclawIo {
    pub(crate) mount: String,
    pub(crate) identity_header: String,
    pub(crate) boundary: String,
    pub(crate) cell_path: String,
    pub(crate) lanes: Arc<Lanes>,
    pub(crate) surfaces: Arc<SurfaceRegistry>,
    /// Set by `run_io`, because the channel exists only there.
    pub(crate) events_tx: Option<mpsc::Sender<PeerEvent>>,
}

impl MeclawIo {
    /// The mount half of the cell at `cell_path`, registering on `surfaces`.
    pub fn new(p: &MeclawParams, cell_path: &str, surfaces: Arc<SurfaceRegistry>) -> Self {
        Self {
            mount: p.mount.clone(),
            identity_header: p.identity_header.clone(),
            boundary: p.boundary.clone(),
            cell_path: cell_path.to_string(),
            lanes: Arc::new(p.lanes.clone()),
            surfaces,
            events_tx: None,
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
        /// The projected body.
        body: Value,
    },
    /// A frame was refused; the far side read the same verdict on the wire.
    Refused {
        /// The lane the frame named, or empty when it named none readable.
        lane: String,
        /// The sender, when the header named one.
        peer: Option<String>,
        /// The frame's trace, when the frame parsed (OR-Peer.L1b.3).
        trace_id: Option<Uuid>,
        /// One of the ten codes.
        error_code: &'static str,
        /// What was refused, in words.
        detail: String,
    },
    /// The name could not be taken; the cell stays up and serves nobody.
    MountFailed(String),
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
    let (receipt, event) = judge(&io, &headers, &body);
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

/// The five checks, in this order: sender, parse, lane, budget, body (its
/// fields, then whether what is left can be delivered at all).
fn judge(io: &MeclawIo, headers: &HeaderMap, body: &[u8]) -> (Value, PeerEvent) {
    // (0) Who sent it: only the header the proxy fills. An empty param and a
    // missing header are the same refusal (fail-closed; this is where the cell
    // departs from `web`, which stamps nothing and carries on).
    let Some(peer) = identity_of(headers, &io.identity_header) else {
        let r = Refusal::new(
            wire::INVALID_FRAME,
            "no authenticated sender header on this mount",
        );
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
    let event = PeerEvent::Arrived {
        lane: lane.route.clone(),
        peer,
        trace_id: frame.trace_id,
        ttl: frame.ttl,
        fields: lane.fields.clone(),
        context,
        body: projected,
    };
    (receipt, event)
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
