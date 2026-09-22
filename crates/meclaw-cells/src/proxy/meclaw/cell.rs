//! `MeclawCell`: the handler half of a `meclaw` proxy.
//!
//! `handle()` is the outgoing direction: judge a message against this side's
//! `emits` lanes, post one frame to the address the entry edge stamped, and
//! emit the receipt. `handle_event()` is the incoming direction, whose verdict
//! the mount has already reached: it only emits. No mutex, no second task, and
//! the `cell.db` stays empty (A9): no cursor, no dedup, no allow-list.

use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::{CellOutput, Message, OriginSink, OutputSink, Path, Uuid};
use serde_json::{Map, Value};
use std::future::Future;
use std::sync::Arc;
use tokio::sync::mpsc;

use super::client::PeerClient;
use super::emit::{arrived_emission, crossed_emission, refused_emission};
use super::io::run_io;
use super::lanes::{self, Direction, Refusal};
use super::mount::{MeclawIo, PeerEvent, PeerReconfig};
use super::params::{Lanes, MeclawParams, refuse_params_update};
use super::wire;

/// The code a `params` slot and a mount that could not be taken are refused
/// with; the house word for a request this cell will not take.
const INVALID_INPUT: &str = "invalid_input";

/// The `meclaw` proxy. State lives single-threaded in the handler sub-task.
pub struct MeclawCell {
    params: MeclawParams,
    lanes: Arc<Lanes>,
    client: PeerClient,
    emit_to: Path,
    /// The mount half, consumed exactly once by `split_io`.
    initial_io_cfg: Option<MeclawIo>,
}

impl MeclawCell {
    /// A cell with a client of its own. A client that cannot be built (TLS
    /// init) is a spawn error.
    pub fn new(p: &MeclawParams) -> Result<Self, String> {
        Ok(Self::with_client(p, PeerClient::new()?))
    }

    /// A cell posting through `client` (cheap to clone; the factory builds one
    /// per cell outside the respawn closure).
    pub fn with_client(p: &MeclawParams, client: PeerClient) -> Self {
        Self {
            params: p.clone(),
            lanes: Arc::new(p.lanes.clone()),
            client,
            emit_to: p.emit_to_path(),
            initial_io_cfg: None,
        }
    }

    /// Hands the cell its mount half.
    pub fn with_io(mut self, io: MeclawIo) -> Self {
        self.initial_io_cfg = Some(io);
        self
    }

    /// Judges one outgoing message and, when it may cross, posts it. The
    /// answer is always one receipt emission: crossed or refused.
    async fn cross(&self, msg: &Message) -> Value {
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
                return refused(route.unwrap_or_default(), r);
            }
        };
        // 2. A `params` slot before anything else, so it cannot pass for a lane
        //    error: every key of this cell is immutable (A9).
        if body.get("params").is_some() {
            let r = Refusal::new(INVALID_INPUT, refuse_params_update());
            return refused(route.unwrap_or_default(), r);
        }
        // 3. The lane is `hop.route`, and it must be one this side emits.
        let Some(route) = route else {
            let r = Refusal::new(
                wire::LANE_UNDECLARED,
                "no hop.route on the message; the lane is what the entry edge stamps there",
            );
            return refused("", r);
        };
        let Some(lane) = lanes::find_lane(&self.lanes, Direction::Emits, route) else {
            let r = Refusal::new(
                wire::LANE_UNDECLARED,
                format!("this side emits no lane {route:?}"),
            );
            return refused(route, r);
        };
        // 4. No hops left: crossing would be one more. `wire::message_frame`
        //    holds the same rule; it is asked first here so the budget is
        //    judged before the body is read.
        if msg.ttl == 0
            && let Err(r) = wire::message_frame(lane, msg, Value::Null, Map::new())
        {
            return refused(route, r);
        }
        // 5. Body and context, against this side's own declaration.
        let projected = match lanes::project_body(lane, body) {
            Ok(b) => b,
            Err(r) => return refused(route, r),
        };
        let context = lanes::project_context(lane, &msg.headers.context);
        let frame = match wire::message_frame(lane, msg, projected, context) {
            Ok(f) => f,
            Err(r) => return refused(route, r),
        };
        // 6. The address is the edge's to stamp (OR-Peer.L1b.2): without one the
        //    peer cannot be reached, and that is what the code says.
        let Some(peer_url) = msg.headers.hop.get("peer_url").and_then(Value::as_str) else {
            let r = Refusal::new(wire::PEER_UNREACHABLE, "no peer_url on the hop");
            return refused(route, r);
        };
        // 7. One POST under the operation timeout (hard rule 12), then the far
        //    side's receipt.
        let answer = self
            .client
            .post_frame(peer_url, &frame, self.params.external_timeout_ms)
            .await
            .and_then(|a| wire::read_receipt(&a));
        match answer {
            Ok(()) => crossed_emission(&lane.route, boundary, &lane.fields),
            Err(r) => refused(route, r),
        }
    }
}

impl LongRunningCell for MeclawCell {
    type Event = PeerEvent;
    type Reconfig = PeerReconfig;
    type Io = MeclawIo;

    /// A second call finds no mount half and gets one on a table of its own:
    /// it serves nobody, which is the honest answer to a wiring nobody made.
    fn split_io(&mut self) -> Self::Io {
        self.initial_io_cfg.take().unwrap_or_else(|| {
            MeclawIo::new(
                &self.params,
                "",
                Arc::new(meclaw_colony::SurfaceRegistry::new()),
            )
        })
    }

    /// The mount half; see [`run_io`]. `clippy::manual_async_fn`: the same
    /// stable false positive as `crate::proxy::cell::ProxyCell::run_io`.
    #[allow(clippy::manual_async_fn)]
    fn run_io(
        io: Self::Io,
        events_tx: mpsc::Sender<Self::Event>,
        reconfig_rx: mpsc::Receiver<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send {
        run_io(io, events_tx, reconfig_rx)
    }

    /// The outgoing direction: one receipt per message, crossed or refused.
    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a mut self,
        msg: Message,
        sink: &'a OutputSink,
        _db: &'a mut DbConn,
        _reconfig_tx: &'a mpsc::Sender<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            let content = self.cross(&msg).await;
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
    /// emission with a fresh trace.
    #[allow(clippy::manual_async_fn)]
    fn handle_event<'a>(
        &'a mut self,
        event: Self::Event,
        sink: &'a OriginSink,
        _db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            let boundary = self.params.boundary.as_str();
            match event {
                PeerEvent::Arrived {
                    lane,
                    peer,
                    trace_id,
                    ttl,
                    fields,
                    context,
                    body,
                } => {
                    let receipt = crossed_emission(&lane, boundary, &fields);
                    let content = arrived_emission(&lane, &peer, boundary, &context, body);
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
            }
        }
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
