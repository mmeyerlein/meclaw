//! GH #47: a batch pipe must not lose the answers that were still in flight
//! when stdin hit EOF.
//!
//! The measurement of 2026-06-15 (`docs/roadmap.md` § Async-Cell-Shutdown-Drain)
//! piped 20 lines into a colony with a real `llm` cell and got 0 answers back,
//! while the same 20 lines with stdin held open produced all 20. This test is
//! that measurement, without a provider: the cell sleeps 300 ms inside its
//! `handle()`, which is the same thing the substrate sees during an HTTP call —
//! an awaiting handler above an empty mailbox.
//!
//! Nothing here is mocked: a real `meclaw` process, real stdin, real stdout.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

const LINES: usize = 20;

/// Root hive "/" with a conditional ingress edge and a return edge, plus a
/// `code` cell at "/echo" whose script sleeps before answering.
///
/// The edge shape is the one `stdio_bridge_demo.rs` uses: the ingress edge
/// carries `!has(hop.finish_reason)` so a reply is not fed back into the cell,
/// and the return edge leaves `enqueue_hive_transit` with no decision at "/",
/// which is what routes it to the egress channel and thus to stdout.
///
/// The script reads its turns from `payload["body"]`, and that one word is
/// load-bearing: a `code` cell is handed THREE objects on stdin since 0.9.0 —
/// `envelope`, `body`, `params` (`docs/cell-types.md` § code, "Die drei Objekte
/// auf stdin"). The first draft of this fixture read `payload["messages"]`,
/// which is never present, so every echo came back as the empty string. The
/// count of answers was right and the content was silently empty — a shape that
/// would have let the drain look broken (or, worse, look fine) for a reason that
/// has nothing to do with the drain.
pub fn write_slow_echo_fixture(root: &std::path::Path, sleep_ms: u64) {
    let echo_dir = root.join("main/echo");
    std::fs::create_dir_all(&echo_dir).unwrap();
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

    let script = format!(
        r#"
import sys, json, time
payload = json.loads(sys.stdin.read())
turns = payload["body"].get("messages", [])
text = turns[-1]["text"] if turns else ""
time.sleep({})
print(json.dumps({{"header": {{"finish_reason": "assistant"}},
                   "messages": [{{"origin": "assistant", "type": "text", "text": text}}]}}))
"#,
        sleep_ms as f64 / 1000.0
    );

    std::fs::write(
        echo_dir.join("config.json"),
        serde_json::json!({
            "cell": {"type": "code"},
            "params": {
                "runner": "python3",
                "script_inline": script,
                "external_timeout_ms": 30000
            },
            "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
        })
        .to_string()
        .as_bytes(),
    )
    .unwrap();
}

/// The core acceptance case: 20 lines in, stdin closed IMMEDIATELY, 20 lines out.
///
/// Note what this test does NOT do: it does not read stdout before closing
/// stdin. `stdio_bridge_demo.rs` and `stdio_json_format.rs` both had to, and
/// both said so in their doc comments — that workaround was the defect, written
/// down. Here stdin is dropped first, which is what a pipe does; and as of the
/// same commit that made this test pass, those two close stdin first as well.
#[test]
fn a_batch_pipe_delivers_every_answer_after_eof() {
    let td = tempfile::TempDir::new().unwrap();
    write_slow_echo_fixture(td.path(), 300);

    let mut child = Command::new(env!("CARGO_BIN_EXE_meclaw"))
        .arg("--root")
        .arg(td.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("meclaw must start");

    {
        let mut stdin = child.stdin.take().expect("stdin piped");
        for i in 0..LINES {
            writeln!(stdin, "line-{i}").unwrap();
        }
        // The whole point: EOF now, while every answer is still in flight.
    }

    let stdout = child.stdout.take().expect("stdout piped");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut lines = Vec::new();
        for l in BufReader::new(stdout).lines() {
            match l {
                Ok(l) => lines.push(l),
                Err(_) => break,
            }
        }
        let _ = tx.send(lines);
    });

    // Generous failure marker (30 s convention); the run should take ~1 s.
    let lines = rx
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("the process must close stdout, not hang");
    let status = child.wait().expect("the process must end");

    assert!(
        status.success(),
        "a drained shutdown exits 0, was: {status:?}"
    );
    assert_eq!(
        lines.len(),
        LINES,
        "every piped line must get its answer back — got {} of {LINES}: {lines:?}",
        lines.len()
    );
    for i in 0..LINES {
        assert!(
            lines.iter().any(|l| l == &format!("line-{i}")),
            "answer for line-{i} missing from {lines:?}"
        );
    }
}
