//! Phase-13.5-A1 Demo: konditionales Routing via CEL-Edge-condition.
//!
//! Topologie (assert_single_root_dir: `td/main` -> mc-path `/`):
//!   /router        (EchoCellFactory, echo_to=/router -- Edge ueberlagert
//!                   emit-target sowieso; self-Pfad ist nur Fallback fuer
//!                   den unmoeglichen Fall, dass KEINE Edge matched)
//!   /branch_gold   (CaptureCell -- registry-only via h.spawn, kein FS-config)
//!   /branch_std    (CaptureCell -- registry-only via h.spawn, kein FS-config)
//!   Edges (in /config.json -- root hive):
//!     from=/router to=/branch_gold  condition="context.tier == 'gold'"
//!     from=/router to=/branch_std   condition="context.tier != 'gold'"
//!
//! Beweis:
//!   - Probe mit context.tier='gold'  -> /branch_gold empfaengt; /branch_std nicht.
//!   - Probe mit context.tier='basic' -> /branch_std empfaengt; /branch_gold nicht.
//!
//! Survival-Mechanik (Zwei-Faecher-Modell): die Edge-Condition wird beim EMIT
//! der /router-Echo-Cell ausgewertet. Bei einer Cell-Emission verfaellt
//! `input.hop`; nur `context` reist durch (`carry_context_with_hop`). Die
//! Source-Probe etabliert `tier` daher in `context` (Ingress-at-birth), damit
//! der Wert den Router-Cell-Hop ueberlebt und die Edge ihn lesen kann.
//!
//! CEL-Mechanismus-Beweis -- NICHT der Phase-14-Tool-Loop.
//!
//! Anti-Cascade-Disziplin (Phase-6.5-Lesson): /branch_gold + /branch_std
//! MUESSEN via h.spawn(...) registriert sein, BEVOR bootstrap_from_filesystem laeuft,
//! damit die Edges beim ersten Router-Emit resolved sind.

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
    // `td/main` ist der single-root-dir -> wird auf mc-path `/` gemappt.
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
    // Router echoed auf sich selbst -- Edge ueberlagert target. Self-Loop wuerde
    // nur greifen, wenn KEINE Edge matched; in beiden Tests trifft IMMER genau eine.
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

    // Anti-Cascade: Sinks VOR bootstrap registrieren, damit Edge-Targets resolved sind.
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

    // Probe mit tier=gold -- die gold-Edge muss matchen. tier in context, damit
    // es den /router-Cell-Hop ueberlebt (input.hop verfaellt bei Cell-Emission).
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

    // std darf NICHTS empfangen haben.
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

    // Probe mit tier=basic -- die not-gold-Edge muss matchen. tier in context,
    // damit es den /router-Cell-Hop ueberlebt (input.hop verfaellt bei Emit).
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

    // gold darf NICHTS empfangen haben.
    assert!(
        gold_rx.try_recv().is_err(),
        "gold branch must NOT receive tier=basic message"
    );

    h.shutdown().await;
}

// ---------------------------------------------------------------------------
// T10b: E2E-Demo modifier.set + modifier.delete am Empfaenger gemessen.
//
// Beweis: modifier veraendert die headers der Message, die der Empfaenger
// tatsaechlich sieht -- nicht nur am apply_edges-Return (T7/T8) oder am
// cascade-builder (T8.5), sondern am echten receiver-cell-Mailbox.
//
// Mechanik (kein EchoMockCell-Header-Propagation noetig): die Colony selbst
// merged input.headers + cell.content.header + edge.modifier in
// `headers_out` (siehe colony.rs:1093-1111 build_follow_up_with), bevor der
// follow-up gebaut und an die receiver-Mailbox geroutet wird. EchoCell ist
// nur Vehikel fuer den Output-Emit -- die Header-Propagation passiert im
// Substrat.
// ---------------------------------------------------------------------------

fn write_modifier_topology(td: &std::path::Path, modifier_json: &str) {
    // Nur /router lebt im FS. /receiver wird registry-only via h.spawn(...)
    // VOR bootstrap registriert (CaptureCell -- Anti-Cascade-Disziplin).
    // Wuerde /receiver auch im FS liegen, ueberschriebe der Bootstrap-Walk
    // die vorab registrierte CaptureCell mit einer Echo-Cell.
    create_dir_all(td.join("main/router")).unwrap();
    let config = format!(
        r#"{{"cell":{{"type":"hive"}},"params":{{"graph":{{"edges":[
            {{"from":"/router","to":"/receiver","modifier":{modifier_json}}}
        ]}}}}}}"#
    );
    fs::write(td.join("main/config.json"), config).unwrap();
    // Router echoed nominell auf sich selbst -- die Edge ueberlagert das Target
    // sowieso. Self-Fallback ist nur fuer den unmoeglichen No-Match-Fall.
    fs::write(
        td.join("main/router/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/router"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_13_5_a1_modifier_set_inserts_header_at_receiver() {
    // Topologie: /router -> [Edge mit modifier.set tier='gold'] -> /receiver
    // Probe ohne tier-Header an /router; Empfaenger MUSS msg.headers["tier"]
    // == "gold" sehen.
    let td = TempDir::new().unwrap();
    write_modifier_topology(td.path(), r#"{"set_hop":{"tier":"'gold'"}}"#);

    let h = ColonyHandle::new();
    let (rx_tx, mut rx_rx) = mpsc::channel(8);

    // Anti-Cascade: receiver-Sink VOR bootstrap registrieren. Die FS-Spec
    // (main/receiver/config.json) zaehlt nur zur Edge-Resolution -- der
    // tatsaechliche Mailbox-Owner ist die registry-spawned CaptureCell, die
    // die Echo-Cell-Registrierung aus dem Bootstrap ueberschreibt.
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
        "E2E-PROOF: modifier.set='gold' propagiert bis msg.headers am Empfaenger; got: {:?}",
        msg.headers.hop
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_13_5_a1_modifier_delete_removes_header_at_receiver() {
    // Topologie: /router -> [Edge mit delete_context debug] -> /receiver
    // Probe MIT debug- und keep-Header (in context) an /router; Empfaenger sieht
    // kein debug-Header, aber das keep-Header bleibt.
    //
    // Survival: zwischen Probe und Edge/receiver liegt der /router-Cell-Hop.
    // debug/keep muessen ihn ueberleben -> context (input.hop verfiele). Der
    // Modifier zielt entsprechend auf context: delete_context["debug"].
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
        "E2E-PROOF: delete_context='debug' entfernt das Header bis zum Empfaenger; got: {:?}",
        msg.headers.context
    );
    assert_eq!(
        msg.headers.context.get("keep"),
        Some(&meclaw_core::serde_json::Value::String("yes".into())),
        "non-deleted context headers ueberleben den Cell-Hop"
    );

    h.shutdown().await;
}
