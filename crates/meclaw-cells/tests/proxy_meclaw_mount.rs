//! The peer mount: who may cross, what it answers, and what it leaves behind.
use meclaw_cells::proxy::meclaw::{
    cell::MeclawCell,
    io::run_io,
    mount::{MeclawIo, PeerEvent, PeerReconfig},
    params::MeclawParams,
};
use meclaw_colony::{LongRunningCell, SurfaceRegistry};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Uuid, serde_json};
use meclaw_testing::surface_listener::{surface_listener, wait_for_mount};
use std::sync::Arc;
use tokio::sync::mpsc;

const HDR: &str = "X-Meclaw-Peer";

fn params(identity_header: &str) -> Value {
    json!({"platform": "meclaw", "mount": "peer", "identity_header": identity_header,
        "boundary": "south", "emit_to": "/sink", "lanes": {
            "accepts": [{"route": "topic", "fields": ["topic"], "because": "a subject, never who said it"}],
            "emits": [{"route": "proposal", "fields": ["proposal"], "because": "one proposal"}]}})
}

/// A mounted cell, its listener's address, the handler half's event channel, and the
/// reconfig sender whose drop is the way out.
async fn mounted(
    identity_header: &str,
) -> (
    std::net::SocketAddr,
    mpsc::Receiver<PeerEvent>,
    Arc<SurfaceRegistry>,
    mpsc::Sender<PeerReconfig>,
) {
    let surfaces = Arc::new(SurfaceRegistry::new());
    let p = MeclawParams::parse(&params(identity_header)).expect("params");
    let (events_tx, events_rx) = mpsc::channel(16);
    let (reconfig_tx, reconfig_rx) = mpsc::channel(1);
    tokio::spawn(run_io(
        MeclawIo::new(&p, "/friend", Arc::clone(&surfaces)),
        events_tx,
        reconfig_rx,
    ));
    wait_for_mount(&surfaces, "peer").await;
    let (addr, _join) = surface_listener(Arc::clone(&surfaces)).await;
    (addr, events_rx, surfaces, reconfig_tx)
}

/// One POST on the peer mount, with or without the identity header.
async fn post(addr: std::net::SocketAddr, peer: Option<&str>, frame: &Value) -> Value {
    let mut req = reqwest::Client::new()
        .post(format!("http://{addr}/peer/"))
        .json(frame);
    if let Some(p) = peer {
        req = req.header(HDR, p);
    }
    let resp = req.send().await.expect("the mount answers");
    assert_eq!(resp.status(), 200, "a refusal is an answer, not an error");
    resp.json().await.expect("a receipt frame")
}

/// A frame as a meclaw sender projects it: the (empty) turn list always rides
/// along, and a body without any central slot is refused as undeliverable.
fn frame(lane: &str, ttl: u32, trace: Uuid) -> Value {
    json!({"v": 1, "type": "message", "lane": lane, "trace_id": trace.to_string(), "ttl": ttl,
        "context": {}, "body": {"messages": [], "topic": "gardening"}})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_mount_is_held_for_the_life_and_given_back_last() {
    let (_addr, _rx, surfaces, reconfig_tx) = mounted(HDR).await;
    assert!(
        surfaces.table().await.iter().any(|r| r.mount == "peer"),
        "the name is on the table"
    );
    drop(reconfig_tx); // the handler goes away: the only way out (A1')
    for _ in 0..100 {
        if surfaces.take_handoff("peer").await.is_none() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("the mount was never given back");
}

/// A7, fail-closed, and the one place this cell departs from `web`: no header on
/// the request, and no header NAMED in the params, are the same refusal.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_post_nobody_signed_is_an_invalid_frame_however_the_signature_is_missing() {
    let (addr, mut rx, _s, _t) = mounted(HDR).await;
    let r = post(addr, None, &frame("topic", 5, Uuid::now_v7())).await;
    assert_eq!(r["result"], json!("refused"));
    assert_eq!(r["error_code"], json!("invalid_frame"));
    assert_eq!(
        r["boundary"],
        json!("south"),
        "the refusal names the boundary"
    );
    assert!(
        matches!(rx.recv().await, Some(PeerEvent::Refused { .. })),
        "and it is kept locally"
    );
    let (addr, _rx, _s, _t) = mounted("").await;
    let r = post(addr, Some("north"), &frame("topic", 5, Uuid::now_v7())).await;
    assert_eq!(
        r["error_code"],
        json!("invalid_frame"),
        "a peer mount with no header named is an open door"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lane_this_side_does_not_accept_is_refused_on_the_wire_and_kept_locally() {
    let (addr, mut rx, _s, _t) = mounted(HDR).await;
    let r = post(addr, Some("north"), &frame("gossip", 5, Uuid::now_v7())).await;
    assert_eq!(r["error_code"], json!("lane_undeclared"));
    match rx.recv().await.expect("a local receipt") {
        PeerEvent::Refused { error_code, .. } => assert_eq!(error_code, "lane_undeclared"),
        o => panic!("expected a refusal, got {o:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_accepted_post_crosses_and_carries_the_trace_and_one_hop_less() {
    let (addr, mut rx, _s, _t) = mounted(HDR).await;
    let trace = Uuid::now_v7();
    let r = post(addr, Some("north"), &frame("topic", 5, trace)).await;
    assert_eq!(
        (r["result"].clone(), r["fields"].clone()),
        (json!("crossed"), json!(["topic"])),
        "the receipt says what crossed"
    );
    let event = rx.recv().await.expect("an arrival");
    match &event {
        PeerEvent::Arrived {
            lane,
            peer,
            trace_id,
            ttl,
            body,
            ..
        } => {
            assert_eq!((lane.as_str(), peer.as_str()), ("topic", "north"));
            assert_eq!(*trace_id, trace, "one conversation stays one trace");
            assert_eq!(
                *ttl, 5,
                "the far side decremented; the emitter takes ttl - 1"
            );
            assert_eq!(
                body,
                &json!({"messages": [], "topic": "gardening"}),
                "and nothing else came along"
            );
        }
        o => panic!("expected an arrival, got {o:?}"),
    }
    // R-26-17 point 4: a crossing leaves a receipt on BOTH sides. The handler
    // half emits the arrival and this side's own `crossed` receipt, both on the
    // frame's trace (OR-Peer9).
    let (tx, mut out) = mpsc::channel(8);
    let sink =
        meclaw_core::OriginSink::new(tx, meclaw_core::Path::new("/friend"), 64).with_ingress();
    let mut cell =
        MeclawCell::new(&MeclawParams::parse(&params(HDR)).expect("params")).expect("cell");
    let mut db =
        meclaw_colony::DbConn::wrap(rusqlite::Connection::open_in_memory().expect("db"), None);
    cell.handle_event(event, &sink, &mut db).await;
    let mut emitted = Vec::new();
    while let Ok(Some(e)) =
        tokio::time::timeout(std::time::Duration::from_millis(500), out.recv()).await
    {
        emitted.push(e);
    }
    let receipts: Vec<_> = emitted
        .iter()
        .filter(|e| e.content["header"]["route"] == json!("receipt"))
        .collect();
    assert_eq!(receipts.len(), 1, "exactly one local receipt per crossing");
    let h = &receipts[0].content["header"];
    assert_eq!(
        (
            h["peer_event"].clone(),
            h["boundary"].clone(),
            h["lane"].clone(),
            h["fields"].clone()
        ),
        (
            json!("crossed"),
            json!("south"),
            json!("topic"),
            json!(["topic"])
        ),
        "this side books the same verdict under its own name"
    );
    assert_eq!(
        receipts[0].content["messages"],
        json!([]),
        "a receipt lifts no turn"
    );
    assert_eq!(receipts[0].trace_id, trace, "on the frame's trace");
    assert!(
        emitted
            .iter()
            .any(|e| e.content["header"]["route"] == json!("topic") && e.trace_id == trace),
        "next to the arrival itself"
    );
}

/// Drive `handle()` once on the `proposal` lane and return the emitted `header`.
/// `peer_url: None` leaves the hop without an address (OR-Peer.L1b.2).
async fn handle_once(peer_url: Option<String>, timeout_ms: u64) -> Value {
    use meclaw_core::{Body, Headers, MessageBuilder, OutputSink, Path};
    let mut v = params(HDR);
    v["external_timeout_ms"] = json!(timeout_ms);
    let mut cell = MeclawCell::new(&MeclawParams::parse(&v).expect("params")).expect("cell");
    let (tx, mut rx) = mpsc::channel(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/friend"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        Headers::new(),
        None,
    );
    let mut hop = serde_json::Map::new();
    hop.insert("route".into(), json!("proposal"));
    hop.insert("peer".into(), json!("north"));
    if let Some(u) = peer_url {
        hop.insert("peer_url".into(), json!(u));
    }
    let msg = MessageBuilder::new(Path::new("/friend"))
        .ttl(5)
        .hop(hop)
        .body(Body::Inline(json!({"proposal": "a walk"})))
        .build();
    let (rc_tx, _rc_rx) = mpsc::channel(1);
    let mut db =
        meclaw_colony::DbConn::wrap(rusqlite::Connection::open_in_memory().expect("db"), None);
    cell.handle(msg, &sink, &mut db, &rc_tx).await;
    let em = tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv())
        .await
        .expect("a receipt within 30s")
        .expect("a receipt");
    em.content["header"].clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_declared_outbound_lane_posts_to_the_peer_url_and_emits_a_crossed_receipt() {
    let answer = meclaw_cells::proxy::meclaw::wire::crossed_receipt("proposal", "north", &[]);
    let (peer, join) = meclaw_testing::mock_http::start_mock_server(
        meclaw_testing::mock_http::MockResponse::ok_json(answer.to_string().as_bytes()),
    )
    .await;
    let h = handle_once(Some(format!("http://{peer}/peer/")), 5000).await;
    assert_eq!(
        h["route"],
        json!("receipt"),
        "ADR-0025: the lane of a receipt is `receipt`"
    );
    assert_eq!(
        (h["peer_event"].clone(), h["boundary"].clone()),
        (json!("crossed"), json!("south"))
    );
    join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_three_ways_out_of_a_crossing_each_name_themselves() {
    let dead = meclaw_testing::ports::free_port();
    assert_eq!(
        handle_once(Some(format!("http://127.0.0.1:{dead}/peer/")), 5000).await["error_code"],
        json!("peer_unreachable")
    );
    // A listener that accepts and never writes: the budget, not the connection, is what runs out.
    let l = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let silent = l.local_addr().expect("addr");
    // The accepted sockets are KEPT: one dropped at the end of a loop turn is a
    // closed connection, which the client reads as unreachable, not as silence.
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = l.accept().await {
            held.push(socket);
        }
    });
    assert_eq!(
        handle_once(Some(format!("http://{silent}/peer/")), 300).await["error_code"],
        json!("peer_timeout")
    );
    let h = handle_once(None, 5000).await; // no address on the hop at all
    assert_eq!(h["error_code"], json!("peer_unreachable"), "OR-Peer.L1b.2");
}

/// One optional spec of `ty`. Nothing in this contract is required: `arrived`
/// carries a body with no `messages[]`, so a required key would break one of the
/// three emission shapes against the substrate's `emits` validation.
fn str_spec(ty: &str) -> Value {
    json!({"type": ty, "required": false})
}

/// The canonical contract block of § 7, held against the check that runs at boot
/// (`bootstrap.rs:1064`) and at mutation staging (`stage.rs:795`) — the shape
/// `web_template.rs:123` uses for the shipped `web` template. The second half is
/// the mount itself: a boundary has no default name.
#[test]
fn the_canonical_contract_block_of_a_peer_proxy_is_complete() {
    let cfg: meclaw_colony::config::ParsedConfig = serde_json::from_value(json!({
        "cell": {"type": "proxy", "timeout": -1},
        "params": params(HDR),
        "contract": {"version": "1.0.0", "settings": {},
            "emits": {"body": {"messages": str_spec("array")},
                "hop": {"route": str_spec("string"), "peer": str_spec("string"),
                    "boundary": str_spec("string"), "lane": str_spec("string"),
                    "fields": str_spec("array"), "error_code": str_spec("string"),
                    "peer_event": {"type": "string", "values": ["crossed", "refused"],
                        "required": false}}},
            "consumes": {"body": {"messages": str_spec("array")},
                "hop": {"route": str_spec("string"), "peer": str_spec("string"),
                    "peer_url": str_spec("string")}},
            "ingress": {"carries_trace": true},
            "capabilities": ["network:proxy", "db:own"]}
    }))
    .expect("the document parses");
    meclaw_colony::config::validate_contract_presence(&cfg.contract)
        .expect("version, settings and consumes are all there");
    let mut v = params(HDR);
    v.as_object_mut().expect("object").remove("mount");
    assert!(
        MeclawParams::parse(&v).is_err(),
        "there is no default name for a boundary"
    );
}

/// A name another cell holds is an operator's mistake to read, not a reason to
/// die: one `MountFailed`, the other cell keeps its mount, and this one stays up.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_taken_mount_is_reported_once_and_the_cell_stays_up() {
    let surfaces = Arc::new(SurfaceRegistry::new());
    let (_held_rx, _held) = surfaces
        .register(
            "peer",
            meclaw_colony::SurfaceEntry {
                kind: "web",
                cell_path: meclaw_core::Path::new("/someone-else"),
                links: None,
            },
        )
        .await
        .expect("the first holder takes the name");
    let p = MeclawParams::parse(&params(HDR)).expect("params");
    let (events_tx, mut events_rx) = mpsc::channel(16);
    let (reconfig_tx, reconfig_rx) = mpsc::channel(1);
    let io = tokio::spawn(run_io(
        MeclawIo::new(&p, "/friend", Arc::clone(&surfaces)),
        events_tx,
        reconfig_rx,
    ));
    let ev = tokio::time::timeout(std::time::Duration::from_secs(30), events_rx.recv())
        .await
        .expect("a report within 30s")
        .expect("an event");
    assert!(matches!(ev, PeerEvent::MountFailed(_)), "got {ev:?}");
    assert!(
        !io.is_finished(),
        "the I/O half stays up after a refused mount"
    );
    let rows = surfaces.table().await;
    assert_eq!(rows.iter().filter(|r| r.mount == "peer").count(), 1);
    assert!(
        rows.iter().any(|r| r.mount == "peer" && r.kind == "web"),
        "the holder keeps it"
    );
    drop(reconfig_tx);
    tokio::time::timeout(std::time::Duration::from_secs(30), io)
        .await
        .expect("the handler going away ends the I/O half")
        .expect("no panic");
}
