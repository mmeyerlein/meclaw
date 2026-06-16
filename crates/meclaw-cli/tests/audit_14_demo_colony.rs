//! Post-Phase-14-Audit (#8): `examples/demo-colony` als gebauter E2E-Test.
//!
//! Die manuelle 8-Punkte-Demo aus `examples/README.md` hatte bis hier null
//! automatisierte Abdeckung — Bootstrap-Aenderungen konnten die committeten
//! Demo-Fixtures still rot machen. Dieser Test bootet den COMMITTETEN
//! Beispiel-Baum (Kopie in ein TempDir — examples/ bleibt read-only,
//! No-Delete-Policy), wendet `examples/demo-mutation.json` wortgleich via
//! `POST /colony/mutations` an und beweist `/echo` ueber positive Receipts:
//! Registry-Eintrag (bash-Cell) + Message-Log-Row mit `to_path == "/echo"`.
//!
//! Harness-Form 1:1 wie `phase_12_b_demo.rs` (production `run_with_hooks`,
//! kein Test-`ColonyHandle`-Wrapper).

use meclaw_cli::{Cli, run_with_hooks};
use std::net::SocketAddr;

/// Repo-relativer Pfad zu `examples/<name>` (Crate liegt zwei Ebenen tiefer).
fn examples_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
}

/// Kopiert den committeten Beispiel-Baum rekursiv ins TempDir-Root —
/// nie in-place booten (`colony.db`/`cell.db` entstehen zur Laufzeit).
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir_recursive(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), &to).unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_colony_boots_mutates_and_reaches_echo() {
    let td = tempfile::TempDir::new().unwrap();
    copy_dir_recursive(&examples_path("demo-colony"), td.path());

    let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let cli = Cli {
        root: td.path().into(),
        log: None,
        log_level: "warn".into(),
        log_filter: None,
        env: None,
        templates: None,
        rescan_templates: false,
        api: Some(bind),
        daemon: false,
        validate: false,
        strict: false,
        blobs: None,
        tokio_console: false,
        tokio_console_port: 6669,
    };
    let join =
        tokio::spawn(async move { run_with_hooks(cli, Some(addr_tx), Some(shutdown_rx)).await });
    let addr = addr_rx.await.unwrap();
    let client = reqwest::Client::new();

    // Step 1: Boot-Receipt — /health antwortet, der Beispiel-Baum ist geladen.
    let resp = client
        .get(format!("http://{addr}/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    // Step 2: committete Mutation-Datei WORTGLEICH anwenden (kein Inline-JSON —
    // genau die Datei, die die README-Demo postet, ist der Pruefling).
    let mutation: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(examples_path("demo-mutation.json")).unwrap(),
    )
    .unwrap();
    let resp = client
        .post(format!("http://{addr}/colony/mutations"))
        .json(&mutation)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let json: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        json["mutation"]["outcome"], "committed",
        "demo-mutation.json must commit against the committed template tree: {json}"
    );

    // Step 3: positiver Mutation-Receipt — Registry fuehrt /echo als bash-Cell
    // (Template `echo` ist eine bash-Cell, siehe examples/README.md § Layout).
    let resp = client
        .get(format!("http://{addr}/colony/registry"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let json: serde_json::Value = resp.json().await.unwrap();
    let entries = json["registry"].as_array().unwrap();
    let echo = entries
        .iter()
        .find(|e| e["path"] == "/echo")
        .unwrap_or_else(|| panic!("mutation must register /echo, registry: {entries:?}"));
    assert_eq!(echo["cell_type"], "bash");

    // Step 4: /echo ist erreichbar — fire-and-forget 202, dann positiver
    // Routing-Receipt: Message-Log-Row mit to_path == "/echo" via /colony/trace.
    let body = serde_json::json!({
        "target": "/echo",
        "body": {
            "messages": [
                { "origin": "user", "type": "text", "text": "demo ping" }
            ]
        }
    });
    let resp = client
        .post(format!("http://{addr}/messages"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);

    let routed = poll_for_trace_to_path(&client, addr, "/echo").await;
    assert!(
        routed,
        "POST /messages target=/echo must surface as message-log row (to_path=/echo) within 2s"
    );

    // Sekundaer-Check (laut, kein Beweis): /echo darf nicht als UNERREICHBARES
    // Ziel im DLQ landen — wuerde z.B. eine CellInactive-Regression beim
    // Mutations-Spawn aufdecken (externer Sender → /echo dead-lettered).
    //
    // W2d (Substrat, Marcus-Ruling 2026-06-12): die bash-Cell antwortet auf das
    // „demo ping" (KEIN tool_call) mit einer Op-Error-Reply. Ohne `reply_to`
    // emittiert sie diese seit W2d an ihren EIGENEN Pfad (`msg.target` = /echo),
    // nicht mehr an den `/colony/dead_letters`-READ-Endpoint; matcht keine
    // Out-Edge ⇒ sie dead-lettert (sender_path == /echo). Das ist die
    // angekuendigte W2-Regression (Op-Echos erreichen den Sender nicht mehr),
    // KEIN Spawn-Defekt — daher wird die cell-eigene Emission (sender_path ==
    // /echo) hier toleriert; der Guard schaerft auf EXTERNE Sender.
    let resp = client
        .get(format!("http://{addr}/colony/dead_letters"))
        .send()
        .await
        .unwrap();
    let json: serde_json::Value = resp.json().await.unwrap();
    let dlq_hit = json["dead_letters"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .any(|e| e["original_target"] == "/echo" && e["sender_path"] != "/echo");
    assert!(
        !dlq_hit,
        "message to /echo must not dead-letter as unreachable target: {json}"
    );

    // Step 5: Shutdown-Kette wie in phase_12_b_demo (T12-Timeout-Form).
    shutdown_tx.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), join)
        .await
        .expect("run_with_hooks must finish within 30s after shutdown")
        .expect("join handle must not panic")
        .expect("run_with_hooks must return Ok");
}

/// Pollt `GET /colony/trace` bis eine Row mit `to_path == expected` auftaucht
/// (Writer-Task schreibt async). Cap ~2s, 50-ms-Schritte — gleiche Form wie
/// `poll_for_dead_letter` in `phase_12_b_demo.rs`.
async fn poll_for_trace_to_path(
    client: &reqwest::Client,
    addr: SocketAddr,
    expected: &str,
) -> bool {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let resp = client
            .get(format!("http://{addr}/colony/trace?limit=50"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
        let json: serde_json::Value = resp.json().await.unwrap();
        let hit = json["trace"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .any(|e| e["to_path"] == expected);
        if hit || tokio::time::Instant::now() >= deadline {
            return hit;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}
