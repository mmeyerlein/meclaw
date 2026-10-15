//! GH #283 — the issue's own reproduction, and the rewiring its point 3 asks
//! for. This file is the ACCEPTANCE receipt of the strand: Tasks 1–6 built the
//! default edge (router, declaration, persistence, identity, rewiring survival,
//! mutation surface), and every assertion here is expected green on its first
//! run. A red assertion in this file is a defect in one of those tasks, never a
//! reason to soften an assertion.
//!
//! # The shape the issue reports
//!
//! A dispatching cell with three out-edges: two guarded arms naming a tool, and
//! a catch-all for everything else. Written the only way the substrate offered
//! until now — an unconditional third edge — the catch-all is an ALWAYS edge:
//! it fires BESIDE the arm that matched, so a message naming a known tool is
//! delivered twice. The workaround the shipped instance used instead was an
//! eight-term negation chain on the catch-all's condition, which has to be
//! edited every time a tool is added or renamed.
//!
//! Both halves are asserted, in this order:
//!
//!   1. [`a_plain_catchall_still_fires_beside_the_matching_arm`] — the issue's
//!      TODAY behaviour, over a plain unconditional edge. It stays in the file
//!      as the record of what was broken, and it is deliberately still green:
//!      the ruling preserves always-edge semantics for every edge that does not
//!      carry the flag, so this half is a lock on "the flag changed nothing for
//!      the edges that do not carry it", not a red test.
//!   2. [`a_default_catchall_delivers_once_per_frame`] — the same topology with
//!      `"default": true` on that same third edge. One delivery per frame,
//!      zero dead letters.
//!
//! And the rewiring of the issue's point 3:
//!
//!   3. [`an_eight_way_tool_surface_needs_no_negation_chain`] — eight positive
//!      arms plus ONE guarded default. The guard is what keeps the default from
//!      becoming a silent swallow-all: traffic the guard excludes still reaches
//!      nothing and still dead-letters as `no_route`.
//!
//! # How the frames carry `hop.tool_name`
//!
//! The edge conditions the issue quotes read the `hop` compartment, which is
//! the emitting cell's own contract output (`Headers::carry_context_with_hop`
//! replaces `hop` at every emission, so a `hop` key set on the INGRESS probe is
//! gone by the time the dispatcher's emission is routed). [`HopStamperCell`]
//! therefore lifts the probe's `context` into its emission's `content.header`
//! block, which is exactly the wire shape the outputs arm splits into the `hop`
//! compartment of the follow-up message.

use meclaw_colony::{CellFactoryRegistry, DeadLetter, DeadLetterReason, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Cell, CellOutput, Message, MessageBuilder, OutputSink, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::future::Future;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

// ── The dispatching cell ─────────────────────────────────────────────────────

/// A cell that emits once per inbound message and stamps the message's
/// `context` verbatim into its `content.header` block.
///
/// `content.header` is the section the outputs arm splits off
/// (`split_content_header`) and carries as the `hop` compartment of the
/// follow-up, so an edge condition over `hop.<key>` sees exactly the keys the
/// probe put into `context`. The emitted `target` is irrelevant to routing — a
/// matching out-edge overlays it, and without one the emission dead-letters as
/// `no_route` whatever it says.
struct HopStamperCell;

impl Cell for HopStamperCell {
    #[allow(clippy::manual_async_fn)]
    fn handle(&mut self, msg: Message, sink: &OutputSink) -> impl Future<Output = ()> + Send {
        let sink = sink.clone();
        let header = msg.headers.context.clone();
        async move {
            let mut content = Map::new();
            content.insert(
                "messages".into(),
                json!([{"origin": "assistant", "type": "text", "text": "dispatch"}]),
            );
            content.insert("header".into(), Value::Object(header));
            let _ = sink
                .push(CellOutput {
                    target: Path::new("/unrouted"),
                    content: Value::Object(content),
                })
                .await;
        }
    }
}

// ── Harness ──────────────────────────────────────────────────────────────────

const DISPATCH: &str = "/dispatch";

/// One ingress frame: a probe at the dispatcher carrying `context` pairs that
/// the dispatcher lifts into `hop`.
fn frame(pairs: &[(&str, &str)]) -> Message {
    let mut context: Map<String, Value> = Map::new();
    for (k, v) in pairs {
        context.insert((*k).to_string(), json!(*v));
    }
    MessageBuilder::new(Path::new(DISPATCH))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"ping"}]}),
        ))
        .context(context)
        .ttl(8)
        .build()
}

/// Bounded wait for a delivery (30s failure-marker convention). A miss prints
/// the dead-letter queue, which is where a mis-routed emission ends up.
async fn recv_one(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>, ctx: &str) -> Message {
    match tokio::time::timeout(Duration::from_secs(30), rx.recv()).await {
        Ok(Some(m)) => m,
        Ok(None) => panic!("{ctx}: capture channel closed"),
        Err(_) => {
            let dlq = h.drain_dead_letters().await;
            panic!("{ctx}: no delivery within 30s; DLQ: {dlq:?}")
        }
    }
}

/// Assert nothing arrives. The semantic discriminator of this file, so the
/// window is short on purpose: a double delivery leaves the SAME `apply_edges`
/// call as the delivery the test already has in hand, so by the time one sink
/// has its message the wrong one would already be in flight.
async fn assert_silent(rx: &mut mpsc::Receiver<Message>, ctx: &str) {
    let got = tokio::time::timeout(Duration::from_millis(700), rx.recv()).await;
    assert!(
        got.is_err(),
        "{ctx}: this sink must stay silent, got {got:?}"
    );
}

/// Bounded wait for the dead-letter queue to hold something. The DLQ write is
/// fire-and-forget (backpressure is deliberately kept off the routing path), so
/// a single drain right after the send would race it.
async fn wait_for_dead_letters(h: &ColonyHandle) -> Vec<DeadLetter> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let dlq = h.drain_dead_letters().await;
        if !dlq.is_empty() || tokio::time::Instant::now() >= deadline {
            return dlq;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A colony whose only file is a hive declaring `edges_json` as its
/// `params.graph`. Every node is spawned into the registry BEFORE the boot, so
/// the boot sees a graph whose endpoints all exist.
///
/// Returns the dispatcher's colony plus one receiver per name in `sinks`.
async fn colony_with(
    edges_json: &str,
    sinks: &[(&str, mpsc::Sender<Message>)],
) -> (TempDir, ColonyHandle) {
    let td = TempDir::new().unwrap();
    let cfg = td.path().join("main");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join("config.json"),
        format!(r#"{{"cell":{{"type":"hive"}},"params":{{"graph":{{"edges":{edges_json}}}}}}}"#),
    )
    .unwrap();

    let h = ColonyHandle::new_with_factories_at(&td, Vec::new());

    h.spawn(Path::new(DISPATCH), || HopStamperCell).await;
    for (path, tx) in sinks {
        let tx = tx.clone();
        h.spawn(Path::new(path), move || CaptureCell::new(tx.clone()))
            .await;
    }

    bootstrap_from_filesystem(td.path(), &CellFactoryRegistry::new(), &h.runtime())
        .await
        .expect("bootstrap succeeds");

    (td, h)
}

const ARM_ALPHA: &str = r#"{"from":"/dispatch","to":"/arm_alpha","condition":"has(hop.tool_name) && hop.tool_name == 'alpha'"}"#;
const ARM_BETA: &str = r#"{"from":"/dispatch","to":"/arm_beta","condition":"has(hop.tool_name) && hop.tool_name == 'beta'"}"#;

// ── 1. The issue's TODAY behaviour ───────────────────────────────────────────

/// The record of the defect GH #283 reports, kept executable.
///
/// Three out-edges, the third one a plain unconditional catch-all. A frame
/// naming `alpha` is delivered TWICE — once through the arm that matched and
/// once through the catch-all that matches everything. That is the double
/// delivery the issue reports, and it is still the behaviour of an edge that
/// does not carry the flag: the ruling changes nothing for those. So this test
/// is a LOCK on the unchanged half, not a red test waiting to be fixed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_plain_catchall_still_fires_beside_the_matching_arm() {
    let (alpha_tx, mut alpha_rx) = mpsc::channel(16);
    let (beta_tx, mut beta_rx) = mpsc::channel(16);
    let (catch_tx, mut catch_rx) = mpsc::channel(16);

    let edges = format!(r#"[{ARM_ALPHA},{ARM_BETA},{{"from":"/dispatch","to":"/catchall"}}]"#);
    let (_td, h) = colony_with(
        &edges,
        &[
            ("/arm_alpha", alpha_tx),
            ("/arm_beta", beta_tx),
            ("/catchall", catch_tx),
        ],
    )
    .await;

    // Frame one: `alpha`. The arm takes it — and so does the catch-all.
    h.send(frame(&[("tool_name", "alpha")])).await;
    let got = recv_one(&h, &mut alpha_rx, "the alpha arm").await;
    assert_eq!(got.target, Path::new("/arm_alpha"));
    let also = recv_one(&h, &mut catch_rx, "the unconditional catch-all").await;
    assert_eq!(
        also.target,
        Path::new("/catchall"),
        "an unconditional out-edge is an ALWAYS edge: it fires BESIDE the arm that matched, \
         which is the double delivery GH #283 reports"
    );
    assert_silent(&mut beta_rx, "the beta arm does not match 'alpha'").await;

    // Frame two: `gamma`. No arm matches, and the catch-all is the only
    // consumer — the one case in which an always-edge looks like a default.
    h.send(frame(&[("tool_name", "gamma")])).await;
    let got = recv_one(&h, &mut catch_rx, "the catch-all on an unmatched tool").await;
    assert_eq!(got.target, Path::new("/catchall"));
    assert_silent(&mut alpha_rx, "the alpha arm does not match 'gamma'").await;
    assert_silent(&mut beta_rx, "the beta arm does not match 'gamma'").await;

    assert!(
        h.drain_dead_letters().await.is_empty(),
        "every frame found a consumer — nothing may dead-letter"
    );
    h.shutdown().await;
}

// ── 2. The same topology, with the flag ──────────────────────────────────────

/// The issue's reproduction over the construct this strand built: the third
/// edge carries `"default": true` and nothing else changes.
///
/// One delivery per frame, zero dead letters — the assertion the issue asks for
/// in its point 2.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_default_catchall_delivers_once_per_frame() {
    let (alpha_tx, mut alpha_rx) = mpsc::channel(16);
    let (beta_tx, mut beta_rx) = mpsc::channel(16);
    let (catch_tx, mut catch_rx) = mpsc::channel(16);

    let edges = format!(
        r#"[{ARM_ALPHA},{ARM_BETA},{{"from":"/dispatch","to":"/catchall","default":true}}]"#
    );
    let (_td, h) = colony_with(
        &edges,
        &[
            ("/arm_alpha", alpha_tx),
            ("/arm_beta", beta_tx),
            ("/catchall", catch_tx),
        ],
    )
    .await;

    // Frame one: `alpha` reaches the arm, and the catch-all stays silent. That
    // silence is the whole of the change.
    h.send(frame(&[("tool_name", "alpha")])).await;
    let got = recv_one(&h, &mut alpha_rx, "the alpha arm").await;
    assert_eq!(got.target, Path::new("/arm_alpha"));
    assert_silent(
        &mut catch_rx,
        "a default must not fire beside the arm that matched",
    )
    .await;
    assert_silent(&mut beta_rx, "the beta arm does not match 'alpha'").await;

    // Frame two: `gamma` reaches the catch-all only — the default is the
    // declared consumer of what would otherwise dead-letter as `no_route`.
    h.send(frame(&[("tool_name", "gamma")])).await;
    let got = recv_one(&h, &mut catch_rx, "the default lane").await;
    assert_eq!(got.target, Path::new("/catchall"));
    assert_silent(&mut alpha_rx, "the alpha arm does not match 'gamma'").await;
    assert_silent(&mut beta_rx, "the beta arm does not match 'gamma'").await;

    assert!(
        h.drain_dead_letters().await.is_empty(),
        "one delivery per frame, zero dead letters"
    );
    h.shutdown().await;
}

// ── 3. The rewiring the issue's point 3 asks for ─────────────────────────────

const TOOLS: [&str; 8] = [
    "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta",
];

/// The replacement for the eight-term `!=` chain the issue quotes from the
/// shipped instance: eight positive arms, plus ONE guarded default.
///
/// Three assertions, and the third is the load-bearing one:
///
///   * a known tool reaches exactly its arm and NOT the default;
///   * `hop.route == 'tool'` with an unknown tool name reaches the default,
///     exactly once — no arm has to be edited when a tool is added;
///   * `hop.route == 'write'`, the traffic the guard excludes, reaches NOTHING
///     and dead-letters as `no_route`. That is what proves the guard does the
///     work the negation chain used to do, and that a default is not a silent
///     swallow-all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_eight_way_tool_surface_needs_no_negation_chain() {
    // One shared channel for the eight arms: each delivery carries its own
    // `target`, so the arm that took a frame is read off the message rather
    // than off which channel spoke.
    let (arm_tx, mut arm_rx) = mpsc::channel(16);
    let (def_tx, mut def_rx) = mpsc::channel(16);

    let mut edges: Vec<String> = TOOLS
        .iter()
        .map(|t| {
            format!(
                r#"{{"from":"/dispatch","to":"/tool_{t}","condition":"has(hop.tool_name) && hop.tool_name == '{t}'"}}"#
            )
        })
        .collect();
    edges.push(
        r#"{"from":"/dispatch","to":"/tool_default","default":true,"condition":"hop.route == 'tool'"}"#
            .to_string(),
    );

    let arm_paths: Vec<String> = TOOLS.iter().map(|t| format!("/tool_{t}")).collect();
    let mut sinks: Vec<(&str, mpsc::Sender<Message>)> = arm_paths
        .iter()
        .map(|p| (p.as_str(), arm_tx.clone()))
        .collect();
    sinks.push(("/tool_default", def_tx));

    let (_td, h) = colony_with(&format!("[{}]", edges.join(",")), &sinks).await;

    // 1. A known tool: exactly its arm, and the default stays silent.
    h.send(frame(&[("route", "tool"), ("tool_name", "delta")]))
        .await;
    let got = recv_one(&h, &mut arm_rx, "the delta arm").await;
    assert_eq!(got.target, Path::new("/tool_delta"));
    assert_silent(
        &mut def_rx,
        "the default must not fire beside the arm that matched",
    )
    .await;

    // 2. An unknown tool on the guarded route: the default, exactly once.
    h.send(frame(&[("route", "tool"), ("tool_name", "omega")]))
        .await;
    let got = recv_one(&h, &mut def_rx, "the guarded default").await;
    assert_eq!(got.target, Path::new("/tool_default"));
    assert_silent(&mut arm_rx, "no arm names 'omega'").await;
    assert_silent(&mut def_rx, "the default fires once, not twice").await;

    assert!(
        h.drain_dead_letters().await.is_empty(),
        "both routed frames found a consumer"
    );

    // 3. The traffic the guard excludes: no arm, no default, `no_route`.
    h.send(frame(&[("route", "write")])).await;
    let dlq = wait_for_dead_letters(&h).await;
    assert_eq!(
        dlq.len(),
        1,
        "the excluded route must dead-letter exactly once, got {dlq:?}"
    );
    assert!(
        matches!(dlq[0].reason, DeadLetterReason::NoRoute),
        "a guarded default does not swallow what its guard excludes — that frame is still \
         `no_route`, got {:?}",
        dlq[0].reason
    );
    assert_silent(&mut arm_rx, "no arm takes the excluded route").await;
    assert_silent(
        &mut def_rx,
        "the guard excludes this frame from the default",
    )
    .await;

    h.shutdown().await;
}
