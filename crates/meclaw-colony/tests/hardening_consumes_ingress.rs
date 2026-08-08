//! Slice 2 (roadmap Z.136): substrat-seitiger required-consumes-Check an der
//! Delivery-Grenze.
//!
//! Task 2.4: Eine Cell mit `consumes.hop.h1 required:true` im contract wird
//! NICHT aufgerufen, wenn die Message den Key nicht trägt — mit `reply_to`
//! geht eine Error-Message (`error_code: "consumes_violation"`) an den
//! Absender, ohne `reply_to` landet die Message in der DLQ
//! (`DeadLetterReason::ConsumesViolation`). Erfüllte consumes und Cells ohne
//! Deklaration liefern normal (positives Capture-Receipt, DLQ leer).
//!
//! Topologie-Muster wie `hardening_header_locality.rs` (FS-Fixtures mit
//! contract-Blöcken, `bootstrap_from_filesystem`, Capture-Receipts,
//! DLQ-Drain). Der Echo-Konsument `/c` läuft über den `cell_task`-Pfad
//! (Reply-Zweig); der stateful Konsument `/m` (multi_update) läuft über
//! `cell_task_stateful` mit verdrahtetem `colony_inbox_tx` (DLQ-Zweig) —
//! der plain `cell_task` hat per Konvention keinen DLQ-Pfad.

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, DeadLetterReason, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Map, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::{EchoCellFactory, MultiUpdateMockCellFactory};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![
        (
            "echo".to_string(),
            Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
        ),
        (
            "multi_update".to_string(),
            Arc::new(MultiUpdateMockCellFactory) as Arc<dyn CellFactory>,
        ),
    ]
}

fn registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    for (name, f) in factories() {
        r.insert(name, f);
    }
    r
}

/// Konsumenten-Topologie (bootet grün, Muster `hardening_header_locality`):
/// Producer `/p` mit `emits.hop.h1` + Boot-Edge `p → c` erfüllt den
/// 14-B-Fan-in-Check des Konsumenten `/c` (`consumes.hop.h1 required:true`,
/// echo't nach `/sink`). `/n` ist der kontrakt-lose Echo-Konsument (vacuous).
/// `/m` ist der STATEFUL Konsument (multi_update, gleicher consumes-Block) für
/// den DLQ-Zweig — die Boot-Edge `p → m` erfüllt auch seinen Fan-in-Check
/// (ein teilnehmender required-Konsument braucht ≥1 liefernde In-Edge);
/// die Delivery-Grenze prüft die TATSÄCHLICHE Message unabhängig davon.
fn write_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main/p")).unwrap();
    std::fs::create_dir_all(td.join("main/c")).unwrap();
    std::fs::create_dir_all(td.join("main/n")).unwrap();
    std::fs::create_dir_all(td.join("main/m")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./p","to":"./c"},
            {"from":"./p","to":"./m"},
            {"from":"./c","to":"/sink"},
            {"from":"./n","to":"/sink"}
        ]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/p/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/c"},
            "contract":{"version":"0.1.0","settings":{},"consumes":{},"emits":{"hop":{"h1":{"type":"string"}}}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/c/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/sink"},
            "contract":{"version":"0.1.0","settings":{},"consumes":{"hop":{"h1":{"type":"string","required":true}}}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/n/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/sink"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/m/config.json"),
        r#"{"cell":{"type":"multi_update"},"params":{"sink_target":"/sink"},
            "contract":{"version":"0.1.0","settings":{},"consumes":{"hop":{"h1":{"type":"string","required":true}}}}}"#,
    )
    .unwrap();
}

/// UBF-konforme Probe (Phase-6-Lesson: keine InvalidUbfBody-DLQ) an `target`.
fn ubf_probe(target: &str) -> MessageBuilder {
    MessageBuilder::new(Path::new(target)).body(Body::Inline(json!({
        "messages": [{"origin": "user", "type": "text", "text": "consumes-probe"}]
    })))
}

/// reply_to gesetzt + required-Key fehlt → Cell läuft NICHT, reply_to
/// empfängt Error-Message mit hop.error_code == "consumes_violation"
/// (Colony extrahiert `content.header` ins hop-Fach der Folge-Message,
/// Muster `emit_backstop_timeout`/`message_timeout`). Die Error-Reply ist
/// der Ordnungs-Anker; danach beweist ein KURZES bounded Fenster auf /sink,
/// dass der Echo-Receipt des Ziels AUSBLEIBT.
///
/// W2b DIRECT-REPLY-PIN (Ruling A1, ruling 2026-06-12): `/c` trägt jetzt die
/// Catch-all-Out-Edge `./c→/sink` (write_topology). Die `consumes_violation`-
/// Error-Reply MUSS trotzdem DIREKT an `/cap` (reply_to) gehen — via
/// route_with_log, NICHT über `/c`s Out-Edges. Beweis hier doppelt: (1) `/cap`
/// empfängt die Reply (Direkt-Zustellung), (2) `/sink` empfängt NICHTS (die
/// Catch-all-Edge leitet die Error-Reply NICHT um) und (3) kein `no_route`-DLQ-
/// Eintrag von `/c` (die Reply wurde zugestellt, nicht ins Leere geroutet).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_required_hop_key_with_reply_to_yields_error_reply() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(&td, factories());

    // Capture-Cells VOR Bootstrap registrieren (Anti-Cascade-Lesson).
    let (cap_tx, mut cap_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/cap"), move || CaptureCell::new(cap_tx.clone()))
        .await;
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &registry(), &h.runtime())
        .await
        .expect("topology must boot green");

    // Probe an /c: reply_to=/cap, OHNE h1 im hop-Fach.
    let probe = ubf_probe("/c").reply_to(Path::new("/cap")).build();
    h.send(probe).await;

    // Error-Reply am reply_to (30s-Failure-Marker-Konvention).
    let received = tokio::time::timeout(Duration::from_secs(30), cap_rx.recv())
        .await
        .expect("/cap muss innerhalb von 30s die consumes_violation-Error-Reply empfangen")
        .expect("CaptureCell-Channel muss eine Nachricht liefern");
    assert_eq!(received.target.as_str(), "/cap");
    assert_eq!(
        received.headers.hop.get("error_code"),
        Some(&json!("consumes_violation")),
        "hop.error_code muss consumes_violation sein, hop: {:?}",
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
        other => panic!("expected inline UBF body at /cap, got {other:?}"),
    };
    assert!(
        body.contains("required consumes.hop 'h1' missing"),
        "Error-Reply muss den Grund tragen: {body}"
    );

    // Ziel-Cell-Receipt BLEIBT AUS: die Error-Reply ist der Anker — wäre /c
    // gelaufen, läge sein Echo bereits in der Pipeline. Kurzes bounded Fenster.
    let no_receipt = tokio::time::timeout(Duration::from_millis(300), sink_rx.recv()).await;
    assert!(
        no_receipt.is_err(),
        "/c darf die Message NICHT verarbeitet haben (kein Echo an /sink), got {no_receipt:?}"
    );

    // W2b direct-reply pin: die Error-Reply wurde DIREKT zugestellt — kein
    // no_route-DLQ-Eintrag von /c (wäre sie über /c→/sink edge-geroutet bzw.
    // ohne match no_route'd worden, läge hier ein Eintrag).
    let dls = h.drain_dead_letters().await;
    assert!(
        !dls.iter()
            .any(|d| d.reason.as_code() == "no_route" && d.sender_path.as_str() == "/c"),
        "consumes_violation-Error-Reply darf NICHT no_route'n (Direkt-Zustellung), got: {:?}",
        dls.iter()
            .map(|d| (d.sender_path.as_str().to_string(), d.reason.as_code()))
            .collect::<Vec<_>>()
    );

    h.shutdown().await;
}

/// reply_to None + required-Key fehlt → DLQ-Eintrag ConsumesViolation
/// (as_code "consumes_violation"), Cell läuft nicht. Ziel ist der STATEFUL
/// Konsument /m (cell_task_stateful mit colony_inbox_tx → DLQ-Pfad).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_required_key_without_reply_to_dead_letters() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(&td, factories());

    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &registry(), &h.runtime())
        .await
        .expect("topology must boot green");

    // Probe an /m: OHNE reply_to, OHNE h1.
    let probe = ubf_probe("/m").build();
    h.send(probe).await;

    // DLQ-Poll mit 30s-Deadline (der DLQ-Eintrag reist asynchron
    // cell_task → colony inbox); drain ist destruktiv → akkumulieren.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut collected = Vec::new();
    loop {
        collected.extend(h.drain_dead_letters().await);
        if collected
            .iter()
            .any(|d| d.reason == DeadLetterReason::ConsumesViolation)
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "kein consumes_violation-Dead-Letter innerhalb von 30s, got {collected:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let cv_count = collected
        .iter()
        .filter(|d| d.reason == DeadLetterReason::ConsumesViolation)
        .count();
    assert_eq!(
        cv_count, 1,
        "genau EIN consumes_violation-Dead-Letter erwartet, got {collected:?}"
    );
    let dl = collected
        .iter()
        .find(|d| d.reason == DeadLetterReason::ConsumesViolation)
        .unwrap();
    assert_eq!(dl.reason, DeadLetterReason::ConsumesViolation);
    assert_eq!(dl.reason.as_code(), "consumes_violation");
    assert_eq!(dl.resolved_target.as_str(), "/m");

    // Cell lief nicht: multi_update hätte an /sink emittiert. Der DLQ-Eintrag
    // ist der Anker; kurzes bounded Fenster danach.
    let no_receipt = tokio::time::timeout(Duration::from_millis(300), sink_rx.recv()).await;
    assert!(
        no_receipt.is_err(),
        "/m darf die Message NICHT verarbeitet haben (keine Emission an /sink), got {no_receipt:?}"
    );

    h.shutdown().await;
}

/// Key vorhanden + typ-korrekt im hop-Fach → handle() läuft (positives
/// Capture-Receipt mit dem /c-Echo-Turn an /sink), DLQ leer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn satisfied_consumes_delivers_normally() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(&td, factories());

    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &registry(), &h.runtime())
        .await
        .expect("topology must boot green");

    // Probe an /c MIT h1 (string) im hop-Fach.
    let mut hop = Map::new();
    hop.insert("h1".into(), json!("v1"));
    let probe = ubf_probe("/c").hop(hop).build();
    h.send(probe).await;

    let received = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("/sink muss innerhalb von 30s das Echo-Receipt empfangen")
        .expect("CaptureCell-Channel muss eine Nachricht liefern");
    let body = match &received.body {
        Body::Inline(v) => v.to_string(),
        other => panic!("expected inline UBF body at /sink, got {other:?}"),
    };
    assert!(
        body.contains("echo from /c"),
        "Receipt muss den /c-Echo-Turn tragen — beweist, dass /c die Message \
         mit erfülltem consumes EMPFANGEN hat: {body}"
    );

    // DLQ-Wächter NACH dem Fluss.
    let dead = h.drain_dead_letters().await;
    assert!(dead.is_empty(), "DLQ must be empty, got {dead:?}");

    h.shutdown().await;
}

/// Cell ohne consumes-Deklaration bleibt unberührt (vacuous): Message ohne
/// jegliche Header liefert normal (Receipt), DLQ leer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cell_without_consumes_is_unaffected() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(&td, factories());

    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &registry(), &h.runtime())
        .await
        .expect("topology must boot green");

    // Probe an /n: keine Header, kein contract am Ziel.
    let probe = ubf_probe("/n").build();
    h.send(probe).await;

    let received = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("/sink muss innerhalb von 30s das Echo-Receipt empfangen")
        .expect("CaptureCell-Channel muss eine Nachricht liefern");
    let body = match &received.body {
        Body::Inline(v) => v.to_string(),
        other => panic!("expected inline UBF body at /sink, got {other:?}"),
    };
    assert!(
        body.contains("echo from /n"),
        "Receipt muss den /n-Echo-Turn tragen (vacuous consumes): {body}"
    );

    let dead = h.drain_dead_letters().await;
    assert!(dead.is_empty(), "DLQ must be empty, got {dead:?}");

    h.shutdown().await;
}
