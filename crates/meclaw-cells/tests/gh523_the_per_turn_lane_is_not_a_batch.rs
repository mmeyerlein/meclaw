//! GH #523 -- a per-turn lane is not a batch, and `memory-drain` is the wrong
//! thing to put in front of it.
//!
//! Measured under parallel load on the private `w10b_remember_colony` colony:
//! one turn reached the memory as TWO `assistant` episodes under
//! `"<session_id>#0"`, no `user` episode at all, and an empty dead-letter queue.
//! Nothing had been refused and nothing had been lost in routing -- the wrong
//! thing was written, and the annotation of that turn then had nothing to bind
//! to.
//!
//! **The cause is the wiring, not the collector.** Ruling Q11 (GH #298)
//! retracted the edge that carries the collector's `turn_write` route into
//! `memory-drain`'s `in_batch` lane: since then the route hands out ONE TURN per
//! message, in the shape the memory hive's `in_episode` door reads, and the
//! adapter has nothing left to adapt. `templates/memory-drain/README.md` says so
//! ("No per-turn cadence"), `w9a_per_turn_colony.rs` measures the replacement,
//! and `member@1.5.0` ships it (GH #527). Two private tests still drew the
//! retracted edge, and this file is what keeps a third from being written.
//!
//! Why the retracted edge cannot work, stated as the mechanism rather than as a
//! rule: the adapter's ledger is a PER-SESSION HIGH-WATER MARK over ONE closed
//! batch -- the newest parked `batch` row plus a single `drained_upto` number.
//! It parks the batch, then reads the ledger back to learn how far the session
//! has already reached memory. A per-turn cadence hands it two batches of one
//! session at once, and the two steps of one delivery then straddle the two
//! steps of the other:
//!
//!   park(user) -> park(assistant) -> probe(user) -> probe(assistant)
//!
//! Both probes read the same ledger, both see the ASSISTANT batch as "the
//! parked day" (the loop keeps the last `batch` row it walks over), and neither
//! sees a mark yet. So the assistant turn leaves twice under index 0 and the
//! user turn never leaves at all. That is the dump above, exactly.
//!
//! Nothing here costs anything: one python process per case, no colony, no
//! provider. The two halves are the measurement (part 1) and the lock (part 2).

use std::io::Write;
use std::process::{Command, Stdio};

use meclaw_core::serde_json::{Value, json};

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty
/// string -- the same substitution the colony performs when it instantiates a
/// template (the form `w10b_inline_gate.rs` uses).
fn resolve_vars(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail
            .find('}')
            .expect("unterminated ${...} in script_inline");
        if let Some((_, default)) = tail[..end].split_once(":-") {
            out.push_str(default);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn drain_script() -> String {
    let raw =
        std::fs::read_to_string(repo("templates/memory-drain/drain/config.json")).expect("config");
    let v: Value = meclaw_core::serde_json::from_str(&raw).expect("config json");
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
    )
}

/// Run a shipped script over a real stdin document, handing the script to
/// python3 **on stdin** instead of in argv (GH #279: a single argv string is
/// capped at 128 KiB and the shipped scripts sit close to that line).
fn run_script_on_stdin(script: &str, stdin_doc: &str) -> std::process::Output {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        meclaw_core::serde_json::to_string(script).unwrap(),
        meclaw_core::serde_json::to_string(stdin_doc).unwrap(),
    );
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    child.wait_with_output().expect("wait")
}

fn emit(doc: Value) -> Vec<Value> {
    let out = run_script_on_stdin(
        &drain_script(),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    assert!(
        out.status.success(),
        "drain exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    meclaw_core::serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

const SESSION: &str = "c-523-2026-08-29T18:00:00.000000Z";

/// One turn as the collector's `turn_write` route hands it out: one message,
/// one speaker, and nothing else in `messages[]`.
fn turn(origin: &str, text: &str) -> Value {
    json!({"origin": origin, "text": text, "happened_at": ""})
}

/// The ledger's answer to the `probe` select: every row this session ever left,
/// oldest first, exactly as the adapter reads it back.
fn probe_echo(rows: Value) -> Value {
    json!({
        "header": {
            "context": {"drain_origin": "drain", "drain_phase": "probe",
                        "session_id": SESSION},
            "hop": {"operation": "select"}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                      "text": rows.to_string()}]
    })
}

fn batch_row(id: &str, turns: Value) -> Value {
    json!({"id": id, "kind": "batch", "payload": turns.to_string(), "drained_upto": 0})
}

fn episodes_of(out: &[Value]) -> Vec<(String, String)> {
    out.iter()
        .filter(|m| m["header"]["route"] == "episode")
        .map(|m| {
            (
                m["header"]["turn_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                m["messages"][0]["origin"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

// ═══════════════════════════ 1. the measurement: what the retracted edge does

/// TWO per-turn deliveries of one session, both parked before either probe runs
/// -- the interleaving a live colony produces the moment the answer follows the
/// question closely enough, which against a mock wire is always and under CPU
/// contention is often.
///
/// Both probes read the same two rows and no mark. The result is the dump in
/// the issue: the assistant turn under `#0`, twice, and the user turn nowhere.
#[test]
fn two_per_turn_batches_of_one_session_hand_out_the_same_turn_twice() {
    let ledger = json!([
        batch_row("t1", json!([turn("user", "my favourite editor is Helix")])),
        batch_row("t2", json!([turn("assistant", "Noted -- Helix it is.")])),
    ]);

    // The user turn's own delivery. It parked FIRST and probes FIRST, and it
    // still hands out the assistant's turn: the probe walks every `batch` row of
    // the session and keeps the last one it sees, because a session has exactly
    // one parked day in the shape this adapter is for.
    let first = episodes_of(&emit(probe_echo(ledger.clone())));
    assert_eq!(
        first,
        vec![(format!("{SESSION}#0"), "assistant".to_string())],
        "the user turn is gone and the assistant's stands under its index"
    );

    // The assistant's own delivery, probing before the first mark has landed --
    // the mark rides in the same emission as the episodes it covers, so it
    // reaches the ledger one hop after the second probe was already queued.
    let second = episodes_of(&emit(probe_echo(ledger)));
    assert_eq!(
        second, first,
        "and the second probe hands out the very same turn under the very same \
         id: two episodes, one turn_id, one speaker -- GH #523's dump"
    );
}

/// The same ledger with the session's ONE batch in it -- the shape ADR 0012
/// keeps this adapter for -- drains whole and in order. The adapter is not
/// broken; it is being handed something it never promised to take.
#[test]
fn one_batch_per_session_is_what_this_adapter_promises_and_it_holds() {
    let day = json!([
        turn("user", "my favourite editor is Helix"),
        turn("assistant", "Noted -- Helix it is."),
    ]);
    let out = episodes_of(&emit(probe_echo(json!([batch_row("t1", day)]))));
    assert_eq!(
        out,
        vec![
            (format!("{SESSION}#0"), "user".to_string()),
            (format!("{SESSION}#1"), "assistant".to_string()),
        ],
        "one closed session, N episodes, in the order of the day"
    );
}

// ═════════════════════════════════ 2. the lock: nobody draws the edge again

/// Every place a topology can be written in this repo, read for the one edge
/// ruling Q11 retracted: a condition that names the `turn_write` route and a
/// modifier that stamps the `in_batch` lane.
///
/// It is a text scan and not a graph walk on purpose -- the edge is written as
/// a `json!` literal in a test, as a `config.json` in a template and as a
/// manifest in an example, and what all three have in common is the two CEL
/// fragments standing next to each other. The pattern is the CEL, never the
/// bare names: prose about the retraction says "in_batch" too, and a lock that
/// tripped on its own explanation would be deleted within the week.
/// `w9a_per_turn_colony.rs` shows the edge that belongs there instead.
#[test]
fn no_topology_hands_the_per_turn_route_to_the_batch_adapter() {
    let mut found: Vec<String> = Vec::new();
    for dir in ["templates", "examples", "crates", "tests"] {
        walk(&repo(dir), &mut |path| {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !matches!(ext, "json" | "rs" | "manifest") {
                return;
            }
            // This file itself carries both fragments -- as the pattern it
            // searches for. A lock that trips on its own source is a lock
            // nobody keeps.
            if path.file_name().and_then(|n| n.to_str())
                == Some(file!().rsplit('/').next().unwrap())
            {
                return;
            }
            let Ok(text) = std::fs::read_to_string(path) else {
                return;
            };
            for (offset, _) in text.match_indices("hop.route == 'turn_write'") {
                let window = &text[offset..text.len().min(offset + 500)];
                if window.contains("'in_batch'") {
                    found.push(format!(
                        "{}: {}",
                        path.display(),
                        window.lines().next().unwrap_or("")
                    ));
                }
            }
        });
    }
    assert!(
        found.is_empty(),
        "the `turn_write` route is wired into a `in_batch` lane again. Ruling Q11 \
         (GH #298) retracted that edge and GH #523 measured what it costs: the \
         adapter's ledger is a per-session high-water mark over ONE closed batch, \
         so two per-turn deliveries of one session overwrite each other between \
         park and probe -- the same turn leaves twice and the other never leaves. \
         Wire `turn_write` at the memory hive's `in_episode` door instead, the way \
         `member@1.5.0` and `w9a_per_turn_colony.rs` do. Found: {found:#?}"
    );
}

fn walk(dir: &std::path::Path, f: &mut impl FnMut(&std::path::Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // `target/` is build output, `archive/` is history that must keep
            // describing the tree as it was.
            if matches!(
                path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                "target" | "archive" | ".git"
            ) {
                continue;
            }
            walk(&path, f);
        } else {
            f(&path);
        }
    }
}
