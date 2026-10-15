//! Slice 3 (roadmap Z.135): central contract.emits validation at the outputs
//! arm (flag-gated; the code cell type is always-on in-cell, excluded here).
//!
//! Task 3.2: a non-code cell with `contract.emits.hop.h1 (number, required)`
//! emits in violation of its contract — the emission is DROPPED at the outputs
//! arm: with `input_reply_to` an error reply
//! (`error_code: "contract_violation"`) goes to the sender of the input message,
//! without `input_reply_to` the emission lands in the DLQ
//! (`DeadLetterReason::ContractViolation`). Conforming emissions route
//! normally; `validate_emits == false` (release model, forced via
//! SetNodeContract) turns the check off.
//!
//! The driver is the echo cell (`EchoCellFactory`): it propagates NO input
//! header into its output — its output header comes exclusively from
//! `params.emitted_header` (config-static). Violating = omit the param
//! (emission without a header section → hop `{}` → required h1 missing),
//! conforming = `emitted_header: {key: "h1", value: 42}`. Topology pattern as
//! in `hardening_consumes_ingress.rs` (FS fixtures, `bootstrap_from_filesystem`,
//! capture receipts, DLQ drain, 30s failure markers).

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, DeadLetterReason, NodeContract,
    bootstrap_from_filesystem,
};
use meclaw_core::serde_json::json;
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "echo".to_string(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    )]
}

fn registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    for (name, f) in factories() {
        r.insert(name, f);
    }
    r
}

/// Emitter topology (boots green): `/c` is the VIOLATING echo emitter —
/// `contract.emits.hop.h1 (number, required)` is declared, but WITHOUT
/// `params.emitted_header` → its emission carries no header section and
/// violates the hop schema. `/ok` is the CONFORMING emitter — identical
/// contract, `emitted_header` supplies `h1: 42`. Both echo to `/down`
/// (capture). W2b (ruling A1): the identity fallback is gone — the echo
/// delivery to `/down` needs a wired catch-all out-edge, otherwise the emission
/// no_routes. Catch-all `./c→/down` + `./ok→/down` (edge target == emitted_target, no
/// target change); `/down` is spawned before the bootstrap in every test (A8
/// resolves against the live registry). The violation tests catch `/c`'s
/// emission at the central emits check BEFORE routing — the edge is inert there.
fn write_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main/c")).unwrap();
    std::fs::create_dir_all(td.join("main/ok")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./c","to":"/down"},
            {"from":"./ok","to":"/down"}
        ]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/c/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/down"},
            "contract":{"version":"0.1.0","settings":{},"consumes":{},"emits":{"hop":{"h1":{"type":"number","required":true}}}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/ok/config.json"),
        r#"{"cell":{"type":"echo"},
            "params":{"emitted_target":"/down","emitted_header":{"key":"h1","value":42}},
            "contract":{"version":"0.1.0","settings":{},"consumes":{},"emits":{"hop":{"h1":{"type":"number","required":true}}}}}"#,
    )
    .unwrap();
}

/// A UBF-conformant probe (phase-6 lesson: no InvalidUbfBody DLQ) to `target`.
/// The ECHO OUTPUT also stays valid UBF (messages array) — only the emits.hop
/// contract is violated, not the debug-only UBF check.
fn ubf_probe(target: &str) -> MessageBuilder {
    MessageBuilder::new(Path::new(target)).body(Body::Inline(json!({
        "messages": [{"origin": "user", "type": "text", "text": "emits-probe"}]
    })))
}

/// A non-code cell with an emits contract: a contract-violating emission is NOT
/// routed; input_reply_to receives error_code == "contract_violation".
/// The error reply is the ordering anchor; afterwards a SHORT bounded window on
/// /down proves that the violating emission does NOT reach its target.
// The central emits-validation gate is active only in debug builds
// (debug_assertions). Under `cargo test --release` the gate is off, so this
// contract-violation path produces no error-reply; gate the test to match.
#[cfg(debug_assertions)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn central_emits_violation_with_reply_to_yields_error_reply() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(&td, factories());

    // Capture-Cells VOR Bootstrap registrieren (Anti-Cascade-Lesson).
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    let (down_tx, mut down_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/down"), move || {
        CaptureCell::new(down_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &registry(), &h.runtime())
        .await
        .expect("topology must boot green");

    // Probe to /c WITH reply_to=/sink: the echo emits without header.h1 →
    // central emits violation → error reply instead of an emission.
    let probe = ubf_probe("/c").reply_to(Path::new("/sink")).build();
    h.send(probe).await;

    // Error-Reply am input_reply_to (30s-Failure-Marker-Konvention).
    let received = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("/sink must receive the contract_violation error reply within 30s")
        .expect("CaptureCell channel must deliver a message");
    assert_eq!(received.target.as_str(), "/sink");
    assert_eq!(
        received.headers.hop.get("error_code"),
        Some(&json!("contract_violation")),
        "hop.error_code must be contract_violation, hop: {:?}",
        received.headers.hop
    );
    assert_eq!(
        received.headers.hop.get("finish_reason"),
        Some(&json!("error")),
        "hop.finish_reason must be error, hop: {:?}",
        received.headers.hop
    );
    let body = match &received.body {
        Body::Inline(v) => v.to_string(),
        other => panic!("expected inline UBF body at /sink, got {other:?}"),
    };
    assert!(
        body.contains("h1"),
        "the error reply must carry the violated key in its reason: {body}"
    );

    // The target receipt DOES NOT ARRIVE: the error reply is the anchor — had
    // the emission been routed, it would already be in the pipeline.
    let no_receipt = tokio::time::timeout(Duration::from_millis(300), down_rx.recv()).await;
    assert!(
        no_receipt.is_err(),
        "the violating emission must NOT reach /down, got {no_receipt:?}"
    );

    h.shutdown().await;
}

/// Without input_reply_to → a DLQ entry ContractViolation, the emission is
/// dropped (reason-filtered count pattern from hardening_consumes_ingress.rs).
// Debug-only: see the note on central_emits_violation_with_reply_to_yields_error_reply.
// The dead-letter for a contract violation only occurs while the debug-build gate is on.
#[cfg(debug_assertions)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn central_emits_violation_without_reply_to_dead_letters() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(&td, factories());

    let (down_tx, mut down_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/down"), move || {
        CaptureCell::new(down_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &registry(), &h.runtime())
        .await
        .expect("topology must boot green");

    // Probe to /c WITHOUT reply_to.
    let probe = ubf_probe("/c").build();
    h.send(probe).await;

    // DLQ poll with a 30s deadline; the drain is destructive → accumulate.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut collected = Vec::new();
    loop {
        collected.extend(h.drain_dead_letters().await);
        if collected
            .iter()
            .any(|d| d.reason == DeadLetterReason::ContractViolation)
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no contract_violation dead letter within 30s, got {collected:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let cv_count = collected
        .iter()
        .filter(|d| d.reason == DeadLetterReason::ContractViolation)
        .count();
    assert_eq!(
        cv_count, 1,
        "exactly ONE contract_violation dead letter expected, got {collected:?}"
    );
    let dl = collected
        .iter()
        .find(|d| d.reason == DeadLetterReason::ContractViolation)
        .unwrap();
    assert_eq!(dl.reason.as_code(), "contract_violation");
    assert_eq!(dl.sender_path.as_str(), "/c");
    assert_eq!(dl.resolved_target.as_str(), "/down");

    // Emission dropped: the DLQ entry is the anchor; a short bounded window
    // afterwards proves the absence of the target receipt.
    let no_receipt = tokio::time::timeout(Duration::from_millis(300), down_rx.recv()).await;
    assert!(
        no_receipt.is_err(),
        "the violating emission must NOT reach /down, got {no_receipt:?}"
    );

    h.shutdown().await;
}

/// A conforming emission routes normally: `/ok` supplies `h1: 42` (number) via
/// `params.emitted_header` → positive capture receipt at /down, DLQ empty.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn central_emits_conforming_emission_routes() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(&td, factories());

    let (down_tx, mut down_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/down"), move || {
        CaptureCell::new(down_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &registry(), &h.runtime())
        .await
        .expect("topology must boot green");

    let probe = ubf_probe("/ok").build();
    h.send(probe).await;

    let received = tokio::time::timeout(Duration::from_secs(30), down_rx.recv())
        .await
        .expect("/down must receive the conforming echo receipt within 30s")
        .expect("CaptureCell channel must deliver a message");
    assert_eq!(
        received.headers.hop.get("h1"),
        Some(&json!(42)),
        "hop.h1 must carry the conforming number, hop: {:?}",
        received.headers.hop
    );
    let body = match &received.body {
        Body::Inline(v) => v.to_string(),
        other => panic!("expected inline UBF body at /down, got {other:?}"),
    };
    assert!(
        body.contains("echo from /ok"),
        "the receipt must carry the /ok echo turn — proves the routed \
         conforming emission: {body}"
    );

    // DLQ guard AFTER the flow.
    let dead = h.drain_dead_letters().await;
    assert!(dead.is_empty(), "DLQ must be empty, got {dead:?}");

    h.shutdown().await;
}

/// validate_emits == false (release model) → no check, the contract-violating
/// emission routes too. In debug builds the boot resolves validate_emits=true —
/// the test overrides the entry for /c AFTER the boot via SetNodeContract (same
/// compiled schema, flag off) and thereby proves the flag gate independently of
/// the build profile.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn central_emits_flag_off_skips_check() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(&td, factories());

    let (down_tx, mut down_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/down"), move || {
        CaptureCell::new(down_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &registry(), &h.runtime())
        .await
        .expect("topology must boot green");

    // Override for /c: identical emits contract, but validate_emits=false.
    let emits_block: meclaw_core::EmitsBlock = meclaw_core::serde_json::from_value(json!({
        "hop": {"h1": {"type": "number", "required": true}}
    }))
    .expect("EmitsBlock must deserialize");
    let compiled =
        meclaw_core::CompiledEmits::compile(&emits_block).expect("emits schema must compile");
    let contract = NodeContract {
        header_view: meclaw_colony::mutation::validate::HeaderNodeView::default(),
        emits: Some(Arc::new(compiled)),
        validate_emits: false,
    };
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::SetNodeContract {
            path: Path::new("/c"),
            contract,
            ack: ack_tx,
        })
        .await
        .expect("colony inbox open");
    tokio::time::timeout(Duration::from_secs(30), ack_rx)
        .await
        .expect("SetNodeContract ack within 30s")
        .expect("ack sender not dropped");

    // The same violating probe as in the DLQ test — now it routes.
    let probe = ubf_probe("/c").build();
    h.send(probe).await;

    let received = tokio::time::timeout(Duration::from_secs(30), down_rx.recv())
        .await
        .expect("/down must receive the receipt within 30s (flag off → no validation)")
        .expect("CaptureCell channel must deliver a message");
    let body = match &received.body {
        Body::Inline(v) => v.to_string(),
        other => panic!("expected inline UBF body at /down, got {other:?}"),
    };
    assert!(
        body.contains("echo from /c"),
        "the receipt must carry the /c echo turn — proves the contract-violating \
         emission ROUTES when validate_emits=false: {body}"
    );

    let dead = h.drain_dead_letters().await;
    assert!(dead.is_empty(), "DLQ must be empty, got {dead:?}");

    h.shutdown().await;
}
