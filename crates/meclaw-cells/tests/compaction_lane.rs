//! GH #120 — the context-compaction lane of a store-backed tool loop.
//!
//! The loop rebuilds its thread from the store on every round, so the thread
//! grows monotonically: round six carries round one's tool result for the sixth
//! time. Caps bound one item and eviction drops whole turns; neither CONDENSES.
//! This is the missing memory capability — the loop has retrieval and lacks
//! condensation.
//!
//! The lane under test is topology, not substrate: an accumulator on the edge
//! that leaves the brain, a threshold on the re-entry edge, and a three-cell
//! hive (`prep` → `condense` → `emit`) that folds the old rounds into ONE
//! summary row. Three pins, one per rule the lane adopts:
//!
//! 1. **The window shrinks and the tail stays fresh.** After the fold the brain
//!    prompt is the user turn, the summary, and the rounds behind the boundary —
//!    byte-pinned, because "smaller" is not the claim; "exactly this" is.
//! 2. **No-Delete.** The store still holds every folded row. What was compacted
//!    is the VIEW, never the record (the pruning discipline of GH #76).
//! 3. **Iterative and cut at a round boundary.** The second fold reads the first
//!    summary instead of starting over, its prompt demands the four fixed
//!    sections, and it never cuts between an assistant `tool_call` and its
//!    `tool_result`.

#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "support_14b.rs"]
mod support;

use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, MessageBuilder, Path};
use mock_openai::{MockOpenAI, canned_chat_completion, canned_tool_calls};
use std::time::Duration;
use support::{
    boot, copy_tree_patch_base_url, emit_dot_if_requested, example_dir, live_graph, recv_bounded,
};

/// The checked-in reference tree.
const FIXTURE: &str = "gh120-compaction-lane";

/// What the condenser is scripted to answer. Deterministic stand-in for a
/// model's prose — the lane is what is under test, not the writing.
const SUMMARY_1: &str = "GOAL: add four numbers. PROGRESS: three calc rounds done. \
KEY DECISIONS: keep using calc. NEXT STEPS: finish the sum.";
/// The second fold's answer. Its prompt must contain [`SUMMARY_1`].
const SUMMARY_2: &str = "GOAL: add four numbers. PROGRESS: seven calc rounds done. \
KEY DECISIONS: keep using calc. NEXT STEPS: answer.";

/// A budget sized for the shape: `4 + rounds * 12` plus the compaction chain,
/// rounded up. The fixture deliberately does not restore ttl (that would make
/// it differ from the walkthrough's base tree in two ways at once).
const SIZED_TTL: u32 = 200;

/// One canned tool round per iteration, then the final answer.
async fn brain(rounds: usize) -> MockOpenAI {
    let mut script = Vec::new();
    for i in 0..rounds {
        script.push(canned_tool_calls(vec![(
            Box::leak(format!("call-{i}").into_boxed_str()),
            "calc",
            r#"{"x":1,"y":1}"#,
        )]));
    }
    script.push(canned_chat_completion("done", "stop"));
    MockOpenAI::start(script).await
}

/// Points the `condense` llm cell at its OWN mock, after
/// `copy_tree_patch_base_url` has pointed every llm cell at the brain. Two
/// mocks instead of one shared script: the condenser's prompt is the thing
/// asserted on, and an interleaved script would make that assertion depend on
/// call ordering rather than on content.
fn point_condenser_at(root: &std::path::Path, base_url: &str) {
    let p = root.join("main/compact/condense/config.json");
    let mut v: Value = meclaw_core::serde_json::from_str(&std::fs::read_to_string(&p).unwrap())
        .expect("the fixture must carry a condense llm cell");
    v["params"]["base_url"] = Value::String(base_url.to_string());
    std::fs::write(&p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

/// The fixture's ingress turn.
fn user_probe(turn_id: &str, ttl: u32) -> meclaw_core::Message {
    let tool_schema = json!({
        "type": "function",
        "function": {"name": "calc", "description": "calc", "parameters": {"type": "object", "properties": {}}}
    });
    let tool_schema_str = meclaw_core::serde_json::to_string(&tool_schema).unwrap();
    let mut headers = meclaw_core::serde_json::Map::new();
    headers.insert("turn_id".into(), json!(turn_id));
    MessageBuilder::new(Path::new("/capture"))
        .body(Body::Inline(json!({
            "system": {"tools": {"calc": {"text": tool_schema_str}}},
            "messages": [{"origin": "user", "type": "text", "text": "add four numbers"}]
        })))
        .context(headers)
        .ttl(ttl)
        .build()
}

/// Every `thread` row of the store, as `(iter, role)` pairs plus the raw turn
/// text — the No-Delete receipt.
fn thread_rows(td: &std::path::Path) -> Vec<(i64, String, String)> {
    let db = td.join("main/store/cell.db");
    let conn = rusqlite::Connection::open(db).expect("the store cell must have a cell.db");
    let mut stmt = conn
        .prepare("SELECT iter, role, turn FROM thread ORDER BY rowid")
        .expect("the thread table must exist");
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .unwrap();
    rows.map(|r| r.unwrap()).collect()
}

/// The user-visible prompt text of one captured provider request, joined — used
/// to assert what the condenser was asked, without depending on the wire's
/// message split.
fn prompt_text(req: &mock_openai::OpenAiRequestSnapshot) -> String {
    req.messages()
        .map(|ms| {
            ms.iter()
                .map(|m| m["content"].as_str().unwrap_or_default().to_string())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

// ───── 1. the compacted window ─────

/// The pin the issue asks for: a loop that ran four tool rounds re-enters the
/// brain with the user turn, ONE summary line, and the tail round — not with
/// four rounds of tool results. Byte-pinned, because the claim is the exact
/// shape of the rebuilt window, not merely that it got smaller.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_compacted_window_is_the_user_turn_the_summary_and_the_fresh_tail() {
    let mock_brain = brain(4).await;
    let mock_condense = MockOpenAI::start(vec![canned_chat_completion(SUMMARY_1, "stop")]).await;
    let td = tempfile::TempDir::new().unwrap();
    copy_tree_patch_base_url(
        &example_dir(FIXTURE),
        td.path(),
        &format!("{}/v1", mock_brain.base_url),
    );
    point_condenser_at(td.path(), &format!("{}/v1", mock_condense.base_url));
    let (h, mut sink_rx, _park_rx) = boot(&td).await;
    h.send(user_probe("t-compact", SIZED_TTL)).await;

    let fin = recv_bounded(&mut sink_rx)
        .await
        .expect("the loop must reach an answer");
    assert_eq!(fin.headers.hop["finish_reason"], "stop");

    // The condenser ran exactly once — the threshold is a threshold, not a
    // per-round tax.
    let folds = mock_condense.recorded_requests().await;
    assert_eq!(
        folds.len(),
        1,
        "four rounds cross the token threshold exactly once"
    );

    // The brain call AFTER the fold is the fifth: four tool rounds, then the
    // answering call. Its window is the compacted one.
    let calls = mock_brain.recorded_requests().await;
    assert_eq!(calls.len(), 5, "four tool rounds plus the answering call");
    let window = calls[4].messages().expect("the request carries messages[]");
    assert_eq!(
        Value::Array(window.clone()),
        json!([
            {"role": "user", "content": "add four numbers"},
            {"role": "assistant", "content": SUMMARY_1},
            {"role": "assistant", "content": null, "tool_calls": [
                {"id": "call-3", "type": "function",
                 "function": {"name": "calc", "arguments": "{\"x\":1,\"y\":1}"}}
            ]},
            {"role": "tool", "tool_call_id": "call-3", "content": "42"},
        ]),
        "the compacted window is the user turn, the summary, and the tail round"
    );

    // The round BEFORE the boundary is gone from the window and only from it.
    let rendered = prompt_text(&calls[4]);
    assert!(
        !rendered.contains("call-0"),
        "the folded rounds must not travel again: {rendered}"
    );

    // The structural half of the claim: the fire lane is PARTITIONED into two
    // edges, and the lane is a hive of three existing cell types — no new cell
    // type, no substrate.
    let (nodes, edges) = live_graph(&h, &["/", "/tool-loop", "/compact"]).await;
    for (path, kind) in [
        ("/compact/prep", "code"),
        ("/compact/condense", "llm"),
        ("/compact/emit", "code"),
    ] {
        assert!(
            nodes.iter().any(|n| n.path == path && n.cell_type == kind),
            "{path} must be a {kind} cell"
        );
    }
    let fire: Vec<&str> = edges
        .iter()
        .filter(|e| e.from == "/tool-loop/collector")
        .map(|e| e.to.as_str())
        .filter(|t| *t == "/llm" || *t == "/compact/prep")
        .collect();
    assert_eq!(
        fire.len(),
        2,
        "the fire lane leaves on exactly two edges — brain or fold: {fire:?}"
    );
    emit_dot_if_requested(FIXTURE, &nodes, &edges);
    h.shutdown().await;
}

// ───── 2. No-Delete ─────

/// The compaction is a read-time cut of the WINDOW; the store keeps the record.
/// Same discipline as the collector's prune lane (GH #76): a cap never deletes,
/// and a row leaves only where deletion is the declared job.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_store_still_holds_every_folded_round_after_the_compaction() {
    let mock_brain = brain(4).await;
    let mock_condense = MockOpenAI::start(vec![canned_chat_completion(SUMMARY_1, "stop")]).await;
    let td = tempfile::TempDir::new().unwrap();
    copy_tree_patch_base_url(
        &example_dir(FIXTURE),
        td.path(),
        &format!("{}/v1", mock_brain.base_url),
    );
    point_condenser_at(td.path(), &format!("{}/v1", mock_condense.base_url));
    let (h, mut sink_rx, _park_rx) = boot(&td).await;
    h.send(user_probe("t-nodelete", SIZED_TTL)).await;
    recv_bounded(&mut sink_rx)
        .await
        .expect("the loop must reach an answer");

    let rows = thread_rows(td.path());
    let roles: Vec<(i64, &str)> = rows.iter().map(|(i, r, _)| (*i, r.as_str())).collect();
    assert_eq!(
        roles,
        vec![
            (0, "user"),
            (0, "assistant"),
            (0, "tool"),
            (1, "assistant"),
            (1, "tool"),
            (2, "assistant"),
            (2, "tool"),
            (3, "assistant"),
            (3, "tool"),
            (2, "summary"),
        ],
        "every round survives the fold; the summary is an ADDITION, not a replacement"
    );
    // The folded content is still readable, verbatim, in the rows the window
    // stopped carrying.
    assert!(
        rows.iter()
            .any(|(i, r, t)| *i == 0 && r == "assistant" && t.contains("call-0")),
        "the first round's assistant turn is still in the store"
    );
    h.shutdown().await;
}

// ───── 3. iterative, sectioned, and cut at a round boundary ─────

/// The three rules the lane adopts, in one run of eight rounds (two folds):
///
/// - **iterative** — the second fold's prompt carries the first summary, so the
///   condenser refines instead of starting from scratch;
/// - **fixed sections** — the shipped instruction names goal / progress / key
///   decisions / next steps, so two folds are comparable;
/// - **never cut at a tool_result** — the fold's transcript ends after a
///   `tool_result`, never between an assistant `tool_call` and its answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_second_fold_reads_the_first_summary_and_cuts_at_a_round_boundary() {
    let mock_brain = brain(8).await;
    let mock_condense = MockOpenAI::start(vec![
        canned_chat_completion(SUMMARY_1, "stop"),
        canned_chat_completion(SUMMARY_2, "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    copy_tree_patch_base_url(
        &example_dir(FIXTURE),
        td.path(),
        &format!("{}/v1", mock_brain.base_url),
    );
    point_condenser_at(td.path(), &format!("{}/v1", mock_condense.base_url));
    let (h, mut sink_rx, _park_rx) = boot(&td).await;
    h.send(user_probe("t-iterative", 400)).await;
    recv_bounded(&mut sink_rx)
        .await
        .expect("the loop must reach an answer");

    let folds = mock_condense.recorded_requests().await;
    assert_eq!(folds.len(), 2, "eight rounds cross the threshold twice");

    // Fixed sections: the instruction is shipped by the glue cell, not invented
    // per call, and both folds get the same one.
    for (n, f) in folds.iter().enumerate() {
        let text = prompt_text(f);
        for section in ["GOAL", "PROGRESS", "KEY DECISIONS", "NEXT STEPS"] {
            assert!(
                text.contains(section),
                "fold {n} must demand the fixed section {section}: {text}"
            );
        }
    }

    // Iterative: the second prompt carries the first summary.
    let second = prompt_text(&folds[1]);
    assert!(
        second.contains(SUMMARY_1),
        "the second fold must refine the first summary instead of starting over: {second}"
    );

    // Never cut at a tool_result: what the second fold folds ends with the
    // result of its last round, and the round kept back does not appear at all.
    assert!(
        second.contains("call-6") && !second.contains("call-7"),
        "the fold cuts between rounds — the tail round stays out of it: {second}"
    );
    let last_line = second.lines().rfind(|l| !l.trim().is_empty());
    assert_eq!(
        last_line,
        Some("- result call-6: 42"),
        "the folded transcript ends after a tool result, never between a \
         tool_call and its answer: {second}"
    );
    h.shutdown().await;
}

// ───── the lane does not fire when there is nothing to fold ─────

/// A loop short enough never to cross the threshold must not pay for the lane:
/// no condenser call, no summary row, and the window is the plain cumulative
/// thread the walkthrough describes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_short_loop_never_enters_the_compaction_lane() {
    let mock_brain = brain(2).await;
    let mock_condense = MockOpenAI::start(vec![canned_chat_completion(SUMMARY_1, "stop")]).await;
    let td = tempfile::TempDir::new().unwrap();
    copy_tree_patch_base_url(
        &example_dir(FIXTURE),
        td.path(),
        &format!("{}/v1", mock_brain.base_url),
    );
    point_condenser_at(td.path(), &format!("{}/v1", mock_condense.base_url));
    let (h, mut sink_rx, _park_rx) = boot(&td).await;
    h.send(user_probe("t-short", SIZED_TTL)).await;
    recv_bounded(&mut sink_rx)
        .await
        .expect("the loop must reach an answer");

    assert!(
        mock_condense.recorded_requests().await.is_empty(),
        "two rounds stay under the threshold — the lane costs nothing"
    );
    assert!(
        !thread_rows(td.path())
            .iter()
            .any(|(_, r, _)| r == "summary"),
        "no fold, no summary row"
    );
    // Give the colony a moment to settle before the temp dir goes away.
    tokio::time::sleep(Duration::from_millis(50)).await;
    h.shutdown().await;
}
