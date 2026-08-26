//! GH #420 / #45 — the SHIPPED `collector/assemble` answers the same under a
//! persistent namespace as under a fresh one.
//!
//! `resident` is the one mode that does not rebuild the script's `globals`
//! between messages, so it is the one mode where a name left over from message
//! N can be read by message N+1. `assemble` is a flat top-level script with
//! early `park()` exits and ~50 names that are bound only inside a branch —
//! exactly the shape where such a leak could hide, and it sits on the seam of
//! every shipped conversational agent. Inspection cannot settle that over 95 kB;
//! a differential can.
//!
//! So: the same message stream, twice. Once through a `warm` cell (fresh
//! namespace per message — semantically identical to `cold` by construction)
//! and once through a `resident` cell (one child, one namespace, for the whole
//! stream). The emissions must match, message for message, byte for byte.
//!
//! The stream deliberately CROSSES lanes and phases, because a leak between two
//! runs of the SAME lane would be invisible: a name that message N left behind
//! is only dangerous if message N+1 walks a different path and reads it.

use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::{CellFactory, SpawnedCellKind};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, MessageBuilder, Path};
use std::sync::Arc;

/// The shipped assembler, exactly as it travels in the template.
fn assemble_config() -> Value {
    let raw = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../templates/collector/assemble/config.json"),
    )
    .expect("templates/collector/assemble/config.json");
    meclaw_core::serde_json::from_str(&raw).expect("assemble config is json")
}

/// The shipped params with `runner_mode` forced to `mode` — everything else,
/// the script and all twenty knobs included, byte-identical to the template.
fn params_in_mode(mode: &str) -> Value {
    let mut cfg = assemble_config();
    let p = cfg["params"].as_object_mut().expect("params object");
    p.insert("runner_mode".into(), Value::String(mode.into()));
    // The A-timeout is generous on purpose: a resident child that has to be
    // replaced mid-stream must not turn into a timeout and mask the answer.
    p.insert("external_timeout_ms".into(), json!(30000));
    Value::Object(p.clone())
}

type Cell = (
    tokio::sync::mpsc::Sender<meclaw_core::Message>,
    tokio::sync::mpsc::Receiver<meclaw_core::CellEmission>,
    tempfile::TempDir,
);

fn spawn(mode: &str) -> Cell {
    let (otx, orx) = tokio::sync::mpsc::channel(256);
    let td = tempfile::TempDir::new().unwrap();
    let (itx, _irx) = tokio::sync::mpsc::channel(8);
    let spawned = Arc::new(CodeCellFactory)
        .spawn_cell(
            Path::new("/assemble"),
            params_in_mode(mode),
            otx,
            td.path().to_path_buf(),
            meclaw_colony::ContractView {
                multi_send_capable: true,
                ..Default::default()
            },
            itx,
            None,
            0,
            None,
            None,
            1000,
        )
        .expect("the shipped assemble params spawn");
    match spawned {
        SpawnedCellKind::Active { sender, .. } => (sender, orx, td),
        SpawnedCellKind::Dormant { .. } => unreachable!("code spawns Active"),
    }
}

fn map_of(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

/// A message as a port edge delivers it: the lane on the hop, the session in
/// context. Mirrors `collector_window::lane_doc`.
fn lane(route: &str, messages: Value) -> meclaw_core::Message {
    MessageBuilder::new(Path::new("/assemble"))
        .context(map_of(
            json!({"session_id":"s1","turn_id":"t1","iter":"0","channel":"c1",
                   "audience_set":"a1","speaker":"sp"}),
        ))
        .hop(map_of(json!({"route": route})))
        .body(Body::Inline(json!({ "messages": messages })))
        .reply_to(Path::new("/sink"))
        .build()
}

/// A store reply as the hive's own edge delivers it back. Mirrors
/// `collector_window::reply_doc` for the non-bundle operations.
fn reply(phase: &str, op: &str, rows_affected: i64, payload: Value) -> meclaw_core::Message {
    MessageBuilder::new(Path::new("/assemble"))
        .context(map_of(json!({"session_id":"s1","turn_id":"t1","iter":"0",
                   "col_phase":phase,"store_origin":"collector"})))
        .hop(map_of(
            json!({"operation":op,"rows_affected":rows_affected}),
        ))
        .body(Body::Inline(json!({
            "messages": [{"origin":"tool","type":"tool_result","id":"x",
                          "text": payload.to_string()}]
        })))
        .reply_to(Path::new("/sink"))
        .build()
}

/// The stream. Every consecutive pair takes a DIFFERENT path through the
/// script, which is what makes a leftover binding observable at all.
fn stream() -> Vec<meclaw_core::Message> {
    vec![
        lane(
            "in_turn",
            json!([{"origin":"user","type":"text","text":"first question"}]),
        ),
        reply("turn-open", "insert", 1, json!([])),
        lane(
            "in_answer",
            json!([{"origin":"assistant","type":"text","text":"an answer"}]),
        ),
        lane(
            "in_tool",
            json!([{"origin":"tool","type":"tool_result","id":"c1","text":"tool said so"}]),
        ),
        lane(
            "in_calls",
            json!([{"origin":"assistant","type":"tool_call","id":"c1","text":"{}"}]),
        ),
        lane("in_close", json!([])),
        lane("in_round_sweep", json!([])),
        lane("in_prune", json!([])),
        reply("ans-w", "insert", 1, json!([])),
        reply("tw-scan", "select", 0, json!([])),
        reply("sweep", "select", 0, json!([])),
        reply("prune-ledger", "select", 0, json!([])),
        lane(
            "in_bundle",
            json!([{"origin":"tool","type":"tool_result","id":"m1","text":"{}"}]),
        ),
        lane(
            "in_memory_call",
            json!([{"origin":"assistant","type":"tool_call","id":"mc1",
                    "text":"{\"name\":\"memory_recall\",\"arguments\":{}}"}]),
        ),
        lane(
            "in_thread_call",
            json!([{"origin":"assistant","type":"tool_call","id":"tc1",
                    "text":"{\"name\":\"thread_recall\",\"arguments\":{}}"}]),
        ),
        lane("in_advice", json!([])),
        // …and round again, so a name left by the SECOND pass over a lane is
        // seen by a path that already ran once.
        lane(
            "in_turn",
            json!([{"origin":"user","type":"text","text":"second question"}]),
        ),
        lane(
            "in_answer",
            json!([{"origin":"assistant","type":"text","text":"another answer"}]),
        ),
        lane("in_close", json!([])),
        lane(
            "in_turn",
            json!([{"origin":"user","type":"text","text":"third question"}]),
        ),
    ]
}

/// Replace the parts of an emission that are nondeterministic BY DESIGN.
///
/// The assembler mints a turn id and stamps a clock on every open, so two runs
/// of the same stream can never be byte-equal on those. Scrubbed here rather
/// than excluded from the comparison, so the SHAPE of every id and timestamp
/// still has to match — a run that emitted one where the other emitted none
/// still fails.
///
/// Three patterns, and nothing else: a uuid, an ISO-8601 instant, and a run of
/// eight-or-more hex digits (the random suffix of a row id). Everything the
/// script actually decided — lanes, phases, routes, ops, tables, columns,
/// caps, contents — is compared verbatim.
fn scrub(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(scrub_str(s)),
        Value::Array(a) => Value::Array(a.iter().map(scrub).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), scrub(x))).collect()),
        other => other.clone(),
    }
}

fn scrub_str(s: &str) -> String {
    let b: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if let Some(n) = uuid_at(&b, i) {
            out.push_str("<uuid>");
            i += n;
        } else if let Some(n) = instant_at(&b, i) {
            out.push_str("<ts>");
            i += n;
        } else if let Some(n) = hexrun_at(&b, i) {
            out.push_str("<hex>");
            i += n;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

fn hex(c: char) -> bool {
    c.is_ascii_hexdigit()
}

/// 8-4-4-4-12 hex with dashes.
fn uuid_at(b: &[char], i: usize) -> Option<usize> {
    let groups = [8usize, 4, 4, 4, 12];
    let mut at = i;
    for (g, len) in groups.iter().enumerate() {
        if g > 0 {
            if b.get(at) != Some(&'-') {
                return None;
            }
            at += 1;
        }
        for _ in 0..*len {
            if !b.get(at).copied().is_some_and(hex) {
                return None;
            }
            at += 1;
        }
    }
    Some(at - i)
}

/// `YYYY-MM-DDTHH:MM:SS` plus an optional fraction and an optional `Z`.
fn instant_at(b: &[char], i: usize) -> Option<usize> {
    let shape = "dddd-dd-ddTdd:dd:dd";
    let mut at = i;
    for want in shape.chars() {
        let got = *b.get(at)?;
        match want {
            'd' if got.is_ascii_digit() => {}
            c if c == got => {}
            _ => return None,
        }
        at += 1;
    }
    if b.get(at) == Some(&'.') {
        at += 1;
        while b.get(at).copied().is_some_and(|c| c.is_ascii_digit()) {
            at += 1;
        }
    }
    if b.get(at) == Some(&'Z') {
        at += 1;
    }
    Some(at - i)
}

/// A run of eight or more hex digits, not part of a longer word.
fn hexrun_at(b: &[char], i: usize) -> Option<usize> {
    if i > 0 && b[i - 1].is_ascii_alphanumeric() {
        return None;
    }
    let mut at = i;
    while b.get(at).copied().is_some_and(hex) {
        at += 1;
    }
    let n = at - i;
    if n >= 8
        && !b
            .get(at)
            .copied()
            .is_some_and(|c| c.is_ascii_alphanumeric())
    {
        Some(n)
    } else {
        None
    }
}

/// Drive `mode` over the stream, collecting the emissions of each message
/// separately. Strictly serial: one message in, its emissions out, then the
/// next — so the transcript is ordered even for the `warm` pool.
async fn transcript(mode: &str) -> Vec<Vec<Value>> {
    let (tx, mut rx, _td) = spawn(mode);
    let mut out = Vec::new();
    for msg in stream() {
        tx.send(msg).await.expect("mailbox open");
        let mut per_message = Vec::new();
        // A `code` cell emits zero (park), one, or many messages per input.
        // Wait a little past the first for a fan-out, and accept silence as an
        // answer in its own right.
        loop {
            match tokio::time::timeout(std::time::Duration::from_millis(600), rx.recv()).await {
                Ok(Some(em)) => {
                    let mut c = em.content.clone();
                    // `duration_ms` is a measurement and is MEANT to differ —
                    // it is the whole point of the mode.
                    if let Some(h) = c.get_mut("header").and_then(|h| h.as_object_mut()) {
                        h.remove("duration_ms");
                    }
                    per_message.push(scrub(&c));
                }
                Ok(None) => panic!("the cell dropped its output channel"),
                Err(_) => break,
            }
        }
        out.push(per_message);
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_resident_assembler_answers_exactly_as_a_fresh_one_does() {
    let warm = transcript("warm").await;
    let resident = transcript("resident").await;

    assert_eq!(
        warm.len(),
        resident.len(),
        "the two runs saw a different number of messages"
    );
    for (i, (w, r)) in warm.iter().zip(resident.iter()).enumerate() {
        assert_eq!(
            w, r,
            "message {i} of the stream differs between a fresh namespace and a \
             persistent one — the shipped assembler carries state across messages"
        );
    }

    // A transcript of nothing would make the comparison above vacuous.
    let emitted: usize = warm.iter().map(Vec::len).sum();
    assert!(
        emitted >= 8,
        "the stream produced only {emitted} emissions — it is not exercising the \
         script and the comparison proves nothing"
    );
}
