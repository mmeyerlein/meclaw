//! Slice 1 (roadmap Z.138): 14-B-Lokalität läuft bei Runtime-Mutationen.
//!
//! Task 1.2: Smoke-Test für die `ColonyMsg::SetNodeContract`-Variante.
//! Task 1.4: semantische Locality-Tests — Negativ-Rejects sind das
//! Builder-Feedback, `error_code == "edge_schema"` ist Vertrag; die
//! Teilnahme-Regel (edge-loser Node trägt keine Obligation) hält
//! `remove_nodes`-Disconnects legal.

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, NodeContract,
    bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

fn echo_factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "echo".to_string(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    )]
}

fn echo_registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert(
        "echo".into(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    );
    r
}

/// Hop-Topologie (bootet grün): Producer `/p` mit `emits.hop.h1`, Konsument
/// `/c` mit `consumes.hop.h1 required:true`, Boot-Edge `p → c` (Fan-in-Check
/// im Bootstrap erfüllt), plus dritte Cell `/t` OHNE `emits.hop.h1`.
/// `/c` echo't nach `/sink` — Capture-Receipt-Ziel im Gutfall-Test; in den
/// Reject-/Disconnect-Tests fließen keine Messages, der Wert ist dort inert.
fn write_hop_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main/p")).unwrap();
    std::fs::create_dir_all(td.join("main/c")).unwrap();
    std::fs::create_dir_all(td.join("main/t")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./p","to":"./c"}]}}}"#,
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
        td.join("main/t/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/c"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

/// Context-Topologie (bootet grün): Setter-Edge `s → c2` mit
/// `modifier.set_context.c1` versorgt den Konsumenten `/c2`
/// (`consumes.context.c1 required:true`); die zweite Edge `x → c2` hält `/c2`
/// nach dem Setter-Kill im post_state teilnehmend (≥1 inzidente Edge).
fn write_context_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main/s")).unwrap();
    std::fs::create_dir_all(td.join("main/x")).unwrap();
    std::fs::create_dir_all(td.join("main/c2")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./s","to":"./c2","modifier":{"set_context":{"c1":"'v1'"}}},
            {"from":"./x","to":"./c2"}
        ]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/s/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/c2"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/x/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/c2"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/c2/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/s"},
            "contract":{"version":"0.1.0","settings":{},"consumes":{"context":{"c1":{"type":"string","required":true}}}}}"#,
    )
    .unwrap();
}

/// Transit-Topologie (F1 / K-H1-Shape, bootet erst seit dem F1-Fix grün):
/// `entry → /sub` trägt `set_hop.hmark`, `/sub → /sub/cellA` ist der
/// Hive-Transit, `cellA` deklariert EHRLICH `consumes.hop.hmark
/// required:true`. Dritte Cell `/x` (edge-frei, keine required Keys) als
/// Quelle für die Mutations-Zwillinge.
fn write_transit_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main/entry")).unwrap();
    std::fs::create_dir_all(td.join("main/x")).unwrap();
    std::fs::create_dir_all(td.join("main/sub/cellA")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./entry","to":"./sub","modifier":{"set_hop":{"hmark":"'HM-R2'"}}},
            {"from":"./sub","to":"./sub/cellA"}
        ]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/entry/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/sub"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/x/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/entry"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/sub/config.json"),
        r#"{"cell":{"type":"hive"}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/sub/cellA/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/sink"},
            "contract":{"version":"0.1.0","settings":{},"consumes":{"hop":{"hmark":{"type":"string","required":true}}}}}"#,
    )
    .unwrap();
}

/// Mutation senden + Outcome über den ack-oneshot lesen
/// (Muster: phase_11_contract_via_mutation.rs).
async fn send_mutation(h: &ColonyHandle, payload: Value) -> MutationOutcome {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("colony inbox open");
    tokio::time::timeout(Duration::from_secs(30), ack_rx)
        .await
        .expect("mutation ack within 30s")
        .expect("ack sender not dropped")
}

/// add_edges, deren Quelle den required consumes.hop-Key NICHT liefert,
/// wird pre-destruktiv rejected (Fan-in-Schnittmenge, 14-B).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutation_add_edge_breaking_hop_fanin_is_rejected_edge_schema() {
    let td = TempDir::new().unwrap();
    write_hop_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("hop topology must boot green");

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"t","to":"c"}]}}),
    )
    .await;
    match outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(error_code, "edge_schema", "details: {details}");
            assert!(
                details.contains("14-B locality"),
                "details must carry the 14-B locality marker, got: {details}"
            );
        }
        other => panic!("expected Rejected(edge_schema), got {other:?}"),
    }
    h.shutdown().await;
}

/// remove_edges, das den einzigen set_context-Setter-Pfad eines required
/// consumes.context-Konsumenten kappt, wird rejected.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutation_remove_edge_breaking_context_reachability_is_rejected() {
    let td = TempDir::new().unwrap();
    write_context_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("context topology must boot green");

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"remove_edges":[{"match":{"from":"s","to":"c2"}}]}}),
    )
    .await;
    match outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(error_code, "edge_schema", "details: {details}");
        }
        other => panic!("expected Rejected(edge_schema), got {other:?}"),
    }
    h.shutdown().await;
}

/// remove_nodes-Disconnect eines hop-Konsumenten bleibt LEGAL
/// (Teilnahme-Regel: edge-loser Node trägt keine Obligation).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutation_disconnect_of_hop_consumer_is_committed() {
    let td = TempDir::new().unwrap();
    write_hop_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("hop topology must boot green");

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"remove_nodes":[{"match":{"name":"c"}}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "disconnect of the hop consumer must commit (participation rule), got {outcome:?}"
    );
    h.shutdown().await;
}

/// Gutfall: add_edges mit modifier.set_hop, der den required Key liefert →
/// Committed; danach POSITIVES Capture-Receipt (CLAUDE.md-Disziplin): eine
/// Probe fließt über die neue Edge `t → c` (set_hop liefert h1) durch den
/// Konsumenten `/c` bis `/sink` — der Receipt-Body trägt die Echo-Turns von
/// `/t` UND `/c` und beweist, dass `/c` die Message empfangen hat
/// ("Message-Fluss danach intakt"). Erst dann der DLQ-Wächter (leer).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutation_add_edge_satisfying_hop_via_set_hop_is_committed() {
    let td = TempDir::new().unwrap();
    write_hop_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());

    // /sink (CaptureCell) VOR Bootstrap registrieren (Anti-Cascade-Lesson).
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("hop topology must boot green");

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[
            {"from":"t","to":"c","modifier":{"set_hop":{"h1":"'v1'"}}},
            // W2b (Ruling A1): /c's echo to /sink needs a wired catch-all out-edge
            // (identity-fallback gone). Both endpoints are live (/c registered, /sink
            // spawned pre-bootstrap), so it rides this same mutation rather than the
            // shared write_hop_topology (whose reject/disconnect tests don't spawn /sink).
            {"from":"c","to":"sink"}
        ]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "set_hop satisfies the required key — must commit, got {outcome:?}"
    );

    // Probe → /t: /t echo't nach /c, die NEUE Edge t→c (set_hop h1) greift,
    // /c echo't nach /sink. UBF-konformer Body (Phase-6-Lesson, keine
    // InvalidUbfBody-DLQ).
    let probe = MessageBuilder::new(Path::new("/t"))
        .body(Body::Inline(json!({
            "messages": [{"origin": "user", "type": "text", "text": "hop-probe"}]
        })))
        .build();
    h.send(probe).await;

    // Positives Receipt (30s-Failure-Marker-Konvention).
    let received = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("/sink muss innerhalb von 30s ein Receipt empfangen — beweist t→c→sink-Fluss")
        .expect("CaptureCell-Channel muss eine Nachricht liefern");
    assert_eq!(
        received.target.as_str(),
        "/sink",
        "Receipt-Target muss /sink sein, got {}",
        received.target.as_str()
    );
    let body = match &received.body {
        Body::Inline(v) => v.to_string(),
        other => panic!("expected inline UBF body at /sink, got {other:?}"),
    };
    assert!(
        body.contains("echo from /t"),
        "Receipt muss den /t-Echo-Turn tragen (Probe lief über /t): {body}"
    );
    assert!(
        body.contains("echo from /c"),
        "Receipt muss den /c-Echo-Turn tragen — beweist, dass der Konsument /c \
         die Message über die neue set_hop-Edge EMPFANGEN hat: {body}"
    );

    // DLQ-Wächter NACH dem Fluss: kein Dead-Letter-Eintrag im Gutfall.
    let dead = h.drain_dead_letters().await;
    assert!(dead.is_empty(), "DLQ must be empty, got {dead:?}");
    h.shutdown().await;
}

/// F1-Zwilling (Pflicht-Punkt 1, Gutfall): die K-H1-Transit-Topologie bootet
/// mit ehrlichem Contract, und eine UNVERWANDTE Mutation committet — die
/// post_state-Re-Validierung in `handle_mutation` läuft denselben
/// Transit-Walk wie der Boot-Pfad (vor dem Fix hätte sie hier transit-blind
/// rejected, obwohl die Mutation `cellA` gar nicht berührt).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutation_unrelated_edge_commits_with_live_transit_required_hop() {
    let td = TempDir::new().unwrap();
    write_transit_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("transit topology with honest required hop must boot green (F1)");

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"x","to":"entry"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "unrelated edge must commit — post-state walk crosses the transit, got {outcome:?}"
    );
    h.shutdown().await;
}

/// F1-Zwilling (Negativ): add_edges, das eine key-lose Quelle IN die Hive
/// verdrahtet, leert die Transit-Schnittmenge von `cellA`s required
/// `hop.hmark` → pre-destruktiver Reject `edge_schema` mit 14-B-Marker
/// (der Mutations-Pfad-Check darf nicht vakuos werden).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutation_add_edge_breaking_transit_intersection_is_rejected() {
    let td = TempDir::new().unwrap();
    write_transit_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("transit topology with honest required hop must boot green (F1)");

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"x","to":"sub"}]}}),
    )
    .await;
    match outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(error_code, "edge_schema", "details: {details}");
            assert!(
                details.contains("14-B locality"),
                "details must carry the 14-B locality marker, got: {details}"
            );
        }
        other => panic!("expected Rejected(edge_schema), got {other:?}"),
    }
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn set_node_contract_acks() {
    let h = ColonyHandle::new();

    let contract = NodeContract {
        header_view: meclaw_colony::mutation::validate::HeaderNodeView::default(),
        emits: None,
        validate_emits: false,
    };

    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::SetNodeContract {
            path: Path::new("/a"),
            contract,
            ack: ack_tx,
        })
        .await
        .expect("colony inbox open");

    // 30s-Failure-Marker-Konvention (robust gegen cargo-parallel-Last).
    tokio::time::timeout(Duration::from_secs(30), ack_rx)
        .await
        .expect("SetNodeContract ack within 30s")
        .expect("ack sender not dropped");
}
