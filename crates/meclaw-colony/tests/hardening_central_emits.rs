//! Slice 3 (roadmap Z.135): zentrale contract.emits-Validierung am
//! outputs-Arm (flag-gated; code-Typ in-cell always-on, hier ausgenommen).
//!
//! Task 3.2: Eine Nicht-code-Cell mit `contract.emits.hop.h1 (number,
//! required)` emittiert contract-widrig — die Emission wird am outputs-Arm
//! GEDROPPT: mit `input_reply_to` geht eine Error-Reply
//! (`error_code: "contract_violation"`) an den Absender der Input-Message,
//! ohne `input_reply_to` landet die Emission in der DLQ
//! (`DeadLetterReason::ContractViolation`). Konforme Emissionen routen
//! normal; `validate_emits == false` (Release-Modell, via SetNodeContract
//! erzwungen) schaltet den Check ab.
//!
//! Treiber ist die Echo-Cell (`EchoCellFactory`): sie propagiert KEINEN
//! Input-Header in ihren Output — ihr Output-Header kommt ausschließlich aus
//! `params.emitted_header` (config-statisch). Verletzend = Param weglassen
//! (Emission ohne header-Sektion → hop `{}` → required h1 fehlt), konform =
//! `emitted_header: {key: "h1", value: 42}`. Topologie-Muster wie
//! `hardening_consumes_ingress.rs` (FS-Fixtures, `bootstrap_from_filesystem`,
//! Capture-Receipts, DLQ-Drain, 30s-Failure-Marker).

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

/// Emitter-Topologie (bootet grün): `/c` ist der VERLETZENDE Echo-Emitter —
/// `contract.emits.hop.h1 (number, required)` deklariert, aber OHNE
/// `params.emitted_header` → seine Emission trägt keine header-Sektion und
/// verletzt das hop-Schema. `/ok` ist der KONFORME Emitter — identischer
/// Contract, `emitted_header` liefert `h1: 42`. Beide echo'n nach `/down`
/// (Capture). W2b (Ruling A1): der Identity-Fallback ist weg — die Echo-
/// Zustellung an `/down` braucht eine verdrahtete Catch-all-Out-Edge, sonst
/// no_routet die Emission. Catch-all `./c→/down` + `./ok→/down` (Edge-Ziel ==
/// echo_to, keine Ziel-Änderung); `/down` ist in jedem Test vor dem Bootstrap
/// gespawnt (A8 resolvt gegen die Live-Registry). Die Violation-Tests fangen
/// `/c`s Emission am zentralen emits-Check VOR dem Routing ab — Edge dort inert.
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
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/down"},
            "contract":{"version":"0.1.0","settings":{},"consumes":{},"emits":{"hop":{"h1":{"type":"number","required":true}}}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/ok/config.json"),
        r#"{"cell":{"type":"echo"},
            "params":{"echo_to":"/down","emitted_header":{"key":"h1","value":42}},
            "contract":{"version":"0.1.0","settings":{},"consumes":{},"emits":{"hop":{"h1":{"type":"number","required":true}}}}}"#,
    )
    .unwrap();
}

/// UBF-konforme Probe (Phase-6-Lesson: keine InvalidUbfBody-DLQ) an `target`.
/// Der ECHO-OUTPUT bleibt ebenfalls valides UBF (messages-Array) — nur der
/// emits.hop-Contract wird verletzt, nicht der debug-only UBF-Check.
fn ubf_probe(target: &str) -> MessageBuilder {
    MessageBuilder::new(Path::new(target)).body(Body::Inline(json!({
        "messages": [{"origin": "user", "type": "text", "text": "emits-probe"}]
    })))
}

/// Nicht-code-Cell mit emits-Contract: contract-widrige Emission wird NICHT
/// geroutet; input_reply_to empfängt error_code == "contract_violation".
/// Die Error-Reply ist der Ordnungs-Anker; danach beweist ein KURZES bounded
/// Fenster auf /down, dass die verletzende Emission ihr Ziel NICHT erreicht.
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

    // Probe an /c MIT reply_to=/sink: das Echo emittiert ohne header.h1 →
    // zentrale emits-Verletzung → Error-Reply statt Emission.
    let probe = ubf_probe("/c").reply_to(Path::new("/sink")).build();
    h.send(probe).await;

    // Error-Reply am input_reply_to (30s-Failure-Marker-Konvention).
    let received = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("/sink muss innerhalb von 30s die contract_violation-Error-Reply empfangen")
        .expect("CaptureCell-Channel muss eine Nachricht liefern");
    assert_eq!(received.target.as_str(), "/sink");
    assert_eq!(
        received.headers.hop.get("error_code"),
        Some(&json!("contract_violation")),
        "hop.error_code muss contract_violation sein, hop: {:?}",
        received.headers.hop
    );
    assert_eq!(
        received.headers.hop.get("finish_reason"),
        Some(&json!("error")),
        "hop.finish_reason muss error sein, hop: {:?}",
        received.headers.hop
    );
    let body = match &received.body {
        Body::Inline(v) => v.to_string(),
        other => panic!("expected inline UBF body at /sink, got {other:?}"),
    };
    assert!(
        body.contains("h1"),
        "Error-Reply muss den verletzten Key im Grund tragen: {body}"
    );

    // Ziel-Receipt BLEIBT AUS: die Error-Reply ist der Anker — wäre die
    // Emission geroutet worden, läge sie bereits in der Pipeline.
    let no_receipt = tokio::time::timeout(Duration::from_millis(300), down_rx.recv()).await;
    assert!(
        no_receipt.is_err(),
        "die verletzende Emission darf /down NICHT erreichen, got {no_receipt:?}"
    );

    h.shutdown().await;
}

/// Ohne input_reply_to → DLQ-Eintrag ContractViolation, Emission gedroppt
/// (reason-gefiltertes Count-Muster aus hardening_consumes_ingress.rs).
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

    // Probe an /c OHNE reply_to.
    let probe = ubf_probe("/c").build();
    h.send(probe).await;

    // DLQ-Poll mit 30s-Deadline; drain ist destruktiv → akkumulieren.
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
            "kein contract_violation-Dead-Letter innerhalb von 30s, got {collected:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let cv_count = collected
        .iter()
        .filter(|d| d.reason == DeadLetterReason::ContractViolation)
        .count();
    assert_eq!(
        cv_count, 1,
        "genau EIN contract_violation-Dead-Letter erwartet, got {collected:?}"
    );
    let dl = collected
        .iter()
        .find(|d| d.reason == DeadLetterReason::ContractViolation)
        .unwrap();
    assert_eq!(dl.reason.as_code(), "contract_violation");
    assert_eq!(dl.sender_path.as_str(), "/c");
    assert_eq!(dl.resolved_target.as_str(), "/down");

    // Emission gedroppt: der DLQ-Eintrag ist der Anker; kurzes bounded
    // Fenster danach beweist das Ausbleiben des Ziel-Receipts.
    let no_receipt = tokio::time::timeout(Duration::from_millis(300), down_rx.recv()).await;
    assert!(
        no_receipt.is_err(),
        "die verletzende Emission darf /down NICHT erreichen, got {no_receipt:?}"
    );

    h.shutdown().await;
}

/// Konforme Emission routet normal: `/ok` liefert `h1: 42` (number) via
/// `params.emitted_header` → positives Capture-Receipt an /down, DLQ leer.
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
        .expect("/down muss innerhalb von 30s das konforme Echo-Receipt empfangen")
        .expect("CaptureCell-Channel muss eine Nachricht liefern");
    assert_eq!(
        received.headers.hop.get("h1"),
        Some(&json!(42)),
        "hop.h1 muss die konforme number tragen, hop: {:?}",
        received.headers.hop
    );
    let body = match &received.body {
        Body::Inline(v) => v.to_string(),
        other => panic!("expected inline UBF body at /down, got {other:?}"),
    };
    assert!(
        body.contains("echo from /ok"),
        "Receipt muss den /ok-Echo-Turn tragen — beweist die geroutete \
         konforme Emission: {body}"
    );

    // DLQ-Wächter NACH dem Fluss.
    let dead = h.drain_dead_letters().await;
    assert!(dead.is_empty(), "DLQ must be empty, got {dead:?}");

    h.shutdown().await;
}

/// validate_emits == false (Release-Modell) → keine Prüfung, auch die
/// contract-widrige Emission routet. In Debug-Builds resolved der Boot
/// validate_emits=true — der Test überschreibt den Eintrag für /c NACH dem
/// Boot via SetNodeContract (gleiches kompiliertes Schema, Flag aus) und
/// beweist damit das Flag-Gate unabhängig vom Build-Profil.
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

    // Override für /c: identischer emits-Contract, aber validate_emits=false.
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

    // Identische verletzende Probe wie im DLQ-Test — jetzt routet sie.
    let probe = ubf_probe("/c").build();
    h.send(probe).await;

    let received = tokio::time::timeout(Duration::from_secs(30), down_rx.recv())
        .await
        .expect("/down muss innerhalb von 30s das Receipt empfangen (Flag aus → keine Prüfung)")
        .expect("CaptureCell-Channel muss eine Nachricht liefern");
    let body = match &received.body {
        Body::Inline(v) => v.to_string(),
        other => panic!("expected inline UBF body at /down, got {other:?}"),
    };
    assert!(
        body.contains("echo from /c"),
        "Receipt muss den /c-Echo-Turn tragen — beweist, dass die \
         contract-widrige Emission bei validate_emits=false ROUTET: {body}"
    );

    let dead = h.drain_dead_letters().await;
    assert!(dead.is_empty(), "DLQ must be empty, got {dead:?}");

    h.shutdown().await;
}
