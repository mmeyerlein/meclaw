//! Phase-13.5-A1 demo: conditional routing via a CEL edge condition.
//!
//! Topology (assert_single_root_dir: `td/main` -> mc path `/`):
//!   /router        (EchoCellFactory, echo_to=/router -- the edge overlays the
//!                   emit target anyway; the self path is only a fallback for
//!                   the impossible case that NO edge matches)
//!   /branch_gold   (CaptureCell -- registry-only via h.spawn, no FS config)
//!   /branch_std    (CaptureCell -- registry-only via h.spawn, no FS config)
//!   Edges (in /config.json -- root hive):
//!     from=/router to=/branch_gold  condition="context.tier == 'gold'"
//!     from=/router to=/branch_std   condition="context.tier != 'gold'"
//!
//! Proof:
//!   - A probe with context.tier='gold'  -> /branch_gold receives; /branch_std does not.
//!   - A probe with context.tier='basic' -> /branch_std receives; /branch_gold does not.
//!
//! Survival mechanics (two-compartment model): the edge condition is evaluated
//! at the EMIT of the /router echo cell. On a cell emission `input.hop` decays;
//! only `context` travels through (`carry_context_with_hop`). The source probe
//! therefore establishes `tier` in `context` (ingress-at-birth), so the value
//! survives the router cell hop and the edge can read it.
//!
//! A CEL mechanism proof -- NOT the phase-14 tool loop.
//!
//! Anti-cascade discipline (phase-6.5 lesson): /branch_gold + /branch_std MUST
//! be registered via h.spawn(...) BEFORE bootstrap_from_filesystem runs, so the
//! edges resolve at the first router emit.

use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::{Body, MessageBuilder, Path, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::fs::{self, create_dir_all};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

fn factories_with_echo() -> CellFactoryRegistry {
    let mut r: CellFactoryRegistry = CellFactoryRegistry::new();
    let echo: Arc<dyn CellFactory> = Arc::new(EchoCellFactory);
    r.insert("echo".into(), echo);
    r
}

fn write_topology(td: &std::path::Path) {
    // `td/main` is the single root dir -> it is mapped to mc path `/`.
    // Daraus folgt: `td/main/router/config.json` -> mc-path `/router`.
    create_dir_all(td.join("main/router")).unwrap();
    fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"/router","to":"/branch_gold","condition":"context.tier == 'gold'"},
            {"from":"/router","to":"/branch_std","condition":"context.tier != 'gold'"}
        ]}}}"#,
    )
    .unwrap();
    // The router echoes to itself -- the edge overlays the target. The self-loop
    // would only apply if NO edge matched; in both tests exactly one ALWAYS does.
    fs::write(
        td.join("main/router/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/router"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_13_5_a1_conditional_routing_gold_branch() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());

    let h = ColonyHandle::new();
    let (gold_tx, mut gold_rx) = mpsc::channel(8);
    let (std_tx, mut std_rx) = mpsc::channel(8);

    // Anti-cascade: register the sinks BEFORE bootstrap so the edge targets resolve.
    h.spawn(Path::new("/branch_gold"), move || {
        CaptureCell::new(gold_tx.clone())
    })
    .await;
    h.spawn(Path::new("/branch_std"), move || {
        CaptureCell::new(std_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &factories_with_echo(), &h.runtime())
        .await
        .expect("bootstrap with cel-condition edges succeeds");

    // Probe with tier=gold -- the gold edge must match. tier lives in context so
    // it survives the /router cell hop (input.hop decays on a cell emission).
    let mut headers = meclaw_core::serde_json::Map::new();
    headers.insert("tier".into(), json!("gold"));
    let probe_gold = MessageBuilder::new(Path::new("/router"))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"hi"}]}),
        ))
        .context(headers)
        .build();
    h.send(probe_gold).await;

    let gold_msg = match tokio::time::timeout(Duration::from_secs(30), gold_rx.recv()).await {
        Ok(Some(m)) => m,
        Ok(None) => panic!("gold rx closed"),
        Err(_) => {
            let dlq = h.drain_dead_letters().await;
            panic!("gold branch must receive within 3s; DLQ: {dlq:?}");
        }
    };
    assert_eq!(
        gold_msg.target,
        Path::new("/branch_gold"),
        "tier=gold routed to gold branch via CEL condition"
    );

    // std must have received NOTHING.
    assert!(
        std_rx.try_recv().is_err(),
        "std branch must NOT receive tier=gold message"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_13_5_a1_conditional_routing_std_branch() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());

    let h = ColonyHandle::new();
    let (gold_tx, mut gold_rx) = mpsc::channel(8);
    let (std_tx, mut std_rx) = mpsc::channel(8);

    h.spawn(Path::new("/branch_gold"), move || {
        CaptureCell::new(gold_tx.clone())
    })
    .await;
    h.spawn(Path::new("/branch_std"), move || {
        CaptureCell::new(std_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &factories_with_echo(), &h.runtime())
        .await
        .expect("bootstrap with cel-condition edges succeeds");

    // Probe with tier=basic -- the not-gold edge must match. tier lives in
    // context so it survives the /router cell hop (input.hop decays on emit).
    let mut headers = meclaw_core::serde_json::Map::new();
    headers.insert("tier".into(), json!("basic"));
    let probe_std = MessageBuilder::new(Path::new("/router"))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"hi"}]}),
        ))
        .context(headers)
        .build();
    h.send(probe_std).await;

    let std_msg = match tokio::time::timeout(Duration::from_secs(30), std_rx.recv()).await {
        Ok(Some(m)) => m,
        Ok(None) => panic!("std rx closed"),
        Err(_) => {
            let dlq = h.drain_dead_letters().await;
            panic!("std branch must receive within 3s; DLQ: {dlq:?}");
        }
    };
    assert_eq!(
        std_msg.target,
        Path::new("/branch_std"),
        "tier=basic routed to std branch via CEL condition (not-gold)"
    );

    // gold must have received NOTHING.
    assert!(
        gold_rx.try_recv().is_err(),
        "gold branch must NOT receive tier=basic message"
    );

    h.shutdown().await;
}

// ---------------------------------------------------------------------------
// T10b: E2E-Demo modifier.set + modifier.delete am Empfaenger gemessen.
//
// Proof: the modifier changes the headers of the message the receiver actually
// sees -- not just at the apply_edges return (T7/T8) or at the cascade builder
// (T8.5), but at the real receiver cell mailbox.
//
// Mechanics (no EchoMockCell header propagation needed): the colony itself
// merged input.headers + cell.content.header + edge.modifier in
// `headers_out` (see colony.rs:1093-1111 build_follow_up_with) before the
// follow-up is built and routed to the receiver mailbox. The EchoCell is only a
// vehicle for the output emit -- the header propagation happens in the
// Substrat.
// ---------------------------------------------------------------------------

fn write_modifier_topology(td: &std::path::Path, modifier_json: &str) {
    // Only /router lives in the FS. /receiver is registered registry-only via
    // h.spawn(...) BEFORE bootstrap (CaptureCell -- anti-cascade discipline).
    // If /receiver also lived in the FS, the bootstrap walk would overwrite the
    // pre-registered CaptureCell with an echo cell.
    create_dir_all(td.join("main/router")).unwrap();
    let config = format!(
        r#"{{"cell":{{"type":"hive"}},"params":{{"graph":{{"edges":[
            {{"from":"/router","to":"/receiver","modifier":{modifier_json}}}
        ]}}}}}}"#
    );
    fs::write(td.join("main/config.json"), config).unwrap();
    // The router nominally echoes to itself -- the edge overlays the target
    // anyway. The self-fallback is only for the impossible no-match case.
    fs::write(
        td.join("main/router/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/router"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_13_5_a1_modifier_set_inserts_header_at_receiver() {
    // Topology: /router -> [edge with modifier.set tier='gold'] -> /receiver
    // A probe without a tier header to /router; the receiver MUST see
    // msg.headers["tier"]
    // == "gold" sehen.
    let td = TempDir::new().unwrap();
    write_modifier_topology(td.path(), r#"{"set_hop":{"tier":"'gold'"}}"#);

    let h = ColonyHandle::new();
    let (rx_tx, mut rx_rx) = mpsc::channel(8);

    // Anti-Cascade: receiver-Sink VOR bootstrap registrieren. Die FS-Spec
    // (main/receiver/config.json) only counts for edge resolution -- the actual
    // mailbox owner is the registry-spawned CaptureCell, which overwrites the
    // echo-cell registration from the bootstrap.
    h.spawn(Path::new("/receiver"), move || {
        CaptureCell::new(rx_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &factories_with_echo(), &h.runtime())
        .await
        .expect("bootstrap with modifier.set edge succeeds");

    // Probe WITHOUT tier-Header.
    let probe = MessageBuilder::new(Path::new("/router"))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"hi"}]}),
        ))
        .build();
    h.send(probe).await;

    let msg = match tokio::time::timeout(Duration::from_secs(30), rx_rx.recv()).await {
        Ok(Some(m)) => m,
        Ok(None) => panic!("receiver rx closed"),
        Err(_) => {
            let dlq = h.drain_dead_letters().await;
            panic!("receiver must receive within 3s; DLQ: {dlq:?}");
        }
    };
    assert_eq!(
        msg.headers.hop.get("tier"),
        Some(&meclaw_core::serde_json::Value::String("gold".into())),
        "E2E-PROOF: modifier.set='gold' propagates all the way to msg.headers at the receiver; got: {:?}",
        msg.headers.hop
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_13_5_a1_modifier_delete_removes_header_at_receiver() {
    // Topology: /router -> [edge with delete_context debug] -> /receiver
    // A probe WITH a debug and a keep header (in context) to /router; the
    // receiver sees no debug header, but the keep header stays.
    //
    // Survival: the /router cell hop sits between the probe and the
    // edge/receiver. debug/keep must survive it -> context (input.hop would
    // decay). The modifier therefore targets context: delete_context["debug"].
    let td = TempDir::new().unwrap();
    write_modifier_topology(td.path(), r#"{"delete_context":["debug"]}"#);

    let h = ColonyHandle::new();
    let (rx_tx, mut rx_rx) = mpsc::channel(8);

    h.spawn(Path::new("/receiver"), move || {
        CaptureCell::new(rx_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &factories_with_echo(), &h.runtime())
        .await
        .expect("bootstrap with modifier.delete edge succeeds");

    let mut headers = meclaw_core::serde_json::Map::new();
    headers.insert("debug".into(), json!("trace-id-xyz"));
    headers.insert("keep".into(), json!("yes"));
    let probe = MessageBuilder::new(Path::new("/router"))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"hi"}]}),
        ))
        .context(headers)
        .build();
    h.send(probe).await;

    let msg = match tokio::time::timeout(Duration::from_secs(30), rx_rx.recv()).await {
        Ok(Some(m)) => m,
        Ok(None) => panic!("receiver rx closed"),
        Err(_) => {
            let dlq = h.drain_dead_letters().await;
            panic!("receiver must receive within 3s; DLQ: {dlq:?}");
        }
    };
    assert!(
        msg.headers.context.get("debug").is_none(),
        "E2E-PROOF: delete_context='debug' removes the header all the way to the receiver; got: {:?}",
        msg.headers.context
    );
    assert_eq!(
        msg.headers.context.get("keep"),
        Some(&meclaw_core::serde_json::Value::String("yes".into())),
        "non-deleted context headers survive the cell hop"
    );

    h.shutdown().await;
}
