//! Task 6 (neu): stdio-Bridge Phasen-Demo + EOF-Lifecycle-Tests.
//!
//! Demo (Step 6.1): `echo "ping" | meclaw --root <fixture>` mit einer
//! synchronen, deterministischen code-Cell, echter Return-Edge und CEL-Bedingung
//! auf der Ingress-Edge. Kein HTTP, kein Mock-Server.
//!
//! Fixture-Topologie:
//!   <root>/main/config.json       — Root-Hive `/`
//!                                   Ingress-Edge: `from="." to="./echo" condition="!has(hop.finish_reason)"`
//!                                   Return-Edge:  `from="./echo" to="."`
//!   <root>/main/echo/config.json  — `type: "code"` (inline Python, emittiert finish_reason="assistant")
//!
//! Flow (edge-getrieben, Task-2-Egress via enqueue_hive_transit):
//!   Bridge-Ingress sendet stdin-Zeile (kein hop.finish_reason) → root-hive "/" →
//!   Ingress-Edge matcht (condition true) → routes zu "/echo".
//!   code-Cell: Python-Skript setzt header.finish_reason="assistant" →
//!   emittiert assistant-Turn zu msg.target="/".
//!   Colony outputs_rx: sender="/echo", apply_edges → Return-Edge matcht →
//!   HiveTransit { hive_path="/", msg }.
//!   enqueue_hive_transit("/"): apply_edges vom Hive "/" → Ingress-Edge prüft
//!   condition !has(hop.finish_reason) → false (hop.finish_reason="assistant") →
//!   Edge überspringen → decisions leer → egress_tx → stdout (Task-2-Egress).
//!
//! EOF-Tests (Step 6.2):
//! - Direct-Mode: stdin-EOF → Prozess beendet Exit 0 ohne Signal.
//! - --daemon + sofortiges stdin-EOF → Prozess läuft weiter (erst Signal
//!   beendet ihn) — beweist EOF-Ignorierung im Daemon-Modus.

use std::io::Write as _;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Python-Skript für die Echo-Cell: ignoriert Eingabe, emittiert immer "pong".
///
/// Setzt `header.finish_reason = "assistant"` im Content-Header. Dieser Wert
/// landet im `hop`-Fach der Message-Headers. Der Ingress-Edge im Root-Hive
/// prüft `!has(hop.finish_reason)` — damit leitet er nur User-Nachrichten
/// weiter (kein finish_reason gesetzt) und lässt Antwort-Nachrichten durch
/// (finish_reason == "assistant" → Edge-Bedingung false → decisions empty
/// → enqueue_hive_transit → egress).
const ECHO_SCRIPT: &str = r#"
import sys, json
# Sets finish_reason so the ingress edge condition `!has(hop.finish_reason)` skips
# the reply message, leaving enqueue_hive_transit with empty decisions -> egress.
print(json.dumps({"header": {"finish_reason": "assistant"}, "messages": [{"origin": "assistant", "type": "text", "text": "pong"}]}))
"#;

/// Schreibt die Fixture-Topologie für den Demo-Test.
///
/// Root-Hive "/" mit zwei Edges:
/// - Ingress-Edge: from="." to="./echo" (kein Filter — alle Nachrichten zu /echo)
/// - Return-Edge: from="./echo" to="." (triggert HiveTransit auf "/")
///
/// code-Cell bei "/echo": inline Python, gibt immer "pong" zurück.
fn write_echo_fixture(root: &std::path::Path) {
    let echo_dir = root.join("main/echo");
    std::fs::create_dir_all(&echo_dir).unwrap();

    // Root-Hive mit bedingter Ingress-Edge und Return-Edge.
    //
    // Ingress-Edge (bedingt): `from="." to="./echo" condition="!has(hop.finish_reason)"`
    //   → leitet nur User-Nachrichten an /echo (kein hop.finish_reason gesetzt).
    //   → Antwort-Nachrichten (hop.finish_reason=="assistant") werden NICHT weitergeleitet.
    //
    // Return-Edge (unbedingt): `from="./echo" to="."`
    //   → In outputs_rx: sender="/echo", apply_edges findet Return-Edge → HiveTransit("/").
    //   → enqueue_hive_transit("/") prüft apply_edges vom Hive "/":
    //     Ingress-Edge hat condition=!has(hop.finish_reason) → false für Antwort.
    //     decisions leer → egress-Kanal (Task-2-Egress, enqueue_hive_transit).
    std::fs::write(
        root.join("main/config.json"),
        serde_json::json!({
            "cell": {"type": "hive"},
            "params": {"graph": {"edges": [
                {"from": ".", "to": "./echo", "condition": "!has(hop.finish_reason)"},
                {"from": "./echo", "to": "."}
            ]}}
        })
        .to_string()
        .as_bytes(),
    )
    .unwrap();

    // code-Cell: inline Python, immer "pong", kein HTTP.
    // Params-Format: `script_inline` (flacher Key, nicht verschachtelt) — CodeParams::parse.
    std::fs::write(
        echo_dir.join("config.json"),
        serde_json::json!({
            "cell": {"type": "code"},
            "params": {
                "runner": "python3",
                "script_inline": ECHO_SCRIPT,
                "external_timeout_ms": 5000
            },
            "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
        })
        .to_string()
        .as_bytes(),
    )
    .unwrap();
}

// ─── Step 6.1: Demo-Test — stdin → assistant-stdout ──────────────────────────

/// Startet meclaw mit einer synchronen code-Cell-Fixture (kein HTTP, kein Mock),
/// pipet "ping" als stdin hinein und erwartet "pong" auf stdout sowie Exit 0.
///
/// Positives Receipt: stdout enthält "pong" ←→ Bridge + Colony + code-Cell +
/// Return-Edge + Task-2-Egress laufen korrekt durch.
///
/// Egress-Pfad (edge-getrieben):
///   code-Cell emittiert an target="/" → Colony outputs_rx: sender="/echo",
///   apply_edges findet Return-Edge from="/echo" to="/" → HiveTransit("/")
///   → enqueue_hive_transit(egress_tx) → egress-Kanal → stdout.
///
/// Test-Strategie: stdout wird VOR dem stdin-EOF gelesen (blockierend, max 10s).
/// Erst wenn die erste stdout-Zeile ("pong") angekommen ist, wird stdin
/// geschlossen → EOF → Shutdown. Das vermeidet den Race zwischen Cell-Worker
/// und Colony-Shutdown.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn direct_mode_stdin_line_produces_assistant_stdout() {
    // Fixture schreiben.
    let td = tempfile::TempDir::new().unwrap();
    write_echo_fixture(td.path());

    // meclaw-Prozess mit gepipetem stdin + stdout starten.
    let root_path = td.path().to_path_buf();
    let (status, stdout_content) = tokio::task::spawn_blocking(move || {
        let mut child = Command::new(env!("CARGO_BIN_EXE_meclaw"))
            .arg("--root")
            .arg(&root_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn meclaw");

        let mut child_stdin = child.stdin.take().expect("stdin");
        let child_stdout = child.stdout.take().expect("stdout");

        // Stdout-Leser: liest eine Zeile blockierend (max 10s bis "pong" kommt).
        // Strategie: erst "ping" senden, dann blockierend warten bis "pong"
        // erscheint, DANN stdin schließen → EOF → Shutdown.
        // So ist der Cell-Output garantiert vor dem Shutdown.
        child_stdin.write_all(b"ping\n").expect("write");
        // stdout-flush durch flush() nicht nötig (unbuffered write_all).

        let stdout_line = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let stdout_line_clone = stdout_line.clone();
        let reader_thread = std::thread::spawn(move || {
            use std::io::BufRead as _;
            let reader = std::io::BufReader::new(child_stdout);
            // Lese EINE Zeile (blockiert bis "pong\n" kommt).
            if let Some(Ok(l)) = reader.lines().next() {
                *stdout_line_clone.lock().unwrap() = l;
            }
        });

        // Warte max 10s auf die erste stdout-Zeile.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if reader_thread.is_finished() {
                break;
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                panic!("meclaw did not produce stdout within 10s");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = reader_thread.join();

        // Erst jetzt stdin schließen → EOF → Shutdown.
        drop(child_stdin);

        // Auf Prozess-Ende warten (max 5s: Shutdown nach EOF ist schnell).
        let exit_deadline = std::time::Instant::now() + Duration::from_secs(5);
        let status = loop {
            match child.try_wait().expect("try_wait") {
                Some(s) => break s,
                None => {
                    if std::time::Instant::now() >= exit_deadline {
                        let _ = child.kill();
                        panic!("meclaw did not exit within 5s after stdin-EOF");
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        };

        let content = stdout_line.lock().unwrap().clone();
        (status, content)
    })
    .await
    .expect("spawn_blocking");

    // Assertions.
    assert_eq!(
        status.code(),
        Some(0),
        "Direct-Mode must exit 0 after stdin-EOF; stdout: {stdout_content:?}"
    );
    assert!(
        stdout_content.contains("pong"),
        "stdout must contain 'pong'; got: {stdout_content:?}"
    );
}

// ─── Step 6.2: EOF-Lifecycle-Tests ───────────────────────────────────────────

/// Direct-Mode: stdin-EOF → Prozess beendet Exit 0 ohne Signal.
/// Nutzt eine einfache Root-Hive-Topologie (keine Cell nötig).
#[test]
fn direct_mode_eof_exits_zero() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(td.path().join("main")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        br#"{"cell":{"type":"hive"}}"#,
    )
    .unwrap();

    // `output()` schließt stdin sofort (kein Input) → EOF → graceful Shutdown.
    // Kein Quiesce-Wait: EOF triggert direkt Shutdown.
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_meclaw"))
        .arg("--root")
        .arg(td.path())
        .output()
        .expect("spawn meclaw");

    assert_eq!(
        output.status.code(),
        Some(0),
        "Direct-Mode stdin-EOF must exit 0; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Gegenprobe: `--daemon` + sofortiges stdin-EOF → Prozess läuft weiter
/// (EOF wird ignoriert). Erst ein Signal beendet ihn.
///
/// Semantisches Timing-Diskriminierungsfenster: 5s nach Start noch am Leben
/// beweist, dass der Daemon-Mode den EOF-Arm NICHT verdrahtet hat.
/// Aufräumen via SIGKILL (child.kill()).
#[test]
fn daemon_mode_eof_does_not_exit() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(td.path().join("main")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        br#"{"cell":{"type":"hive"}}"#,
    )
    .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_meclaw"))
        .arg("--root")
        .arg(td.path())
        .arg("--daemon")
        .stdin(Stdio::null()) // stdin sofort EOF
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn meclaw --daemon");

    // Semantisches Timing: 5s. Daemon darf NICHT von EOF getrieben beendet sein.
    std::thread::sleep(Duration::from_secs(5));

    let still_running = child.try_wait().expect("try_wait").is_none();
    assert!(
        still_running,
        "--daemon must ignore stdin-EOF and keep running (process exited unexpectedly)"
    );

    child.kill().expect("kill daemon process");
    let _ = child.wait();
}
