//! GH #1323 (R-39-1 point 2) -- a third cell's correction closes ONE axis at
//! once, through the shipped memory hive, and only what the window showed.
//!
//! The locks, with invented people only:
//! - (a) a report with its quote, which the closer's model accepts, supersedes
//!   the open statement of its axis in `facts` -- through the colony and the
//!   real store, with no close pass and no second conversation in between; the
//!   closure is named `correction:<key>` and the receipt says `closed`;
//! - (b) a report the model refuses ends in `correction_report` `refused` /
//!   `model_refused`, and `facts` is exactly what it was;
//! - (c) a verdict whose `replaces` names a statement OUTSIDE the parked window
//!   writes nothing (`refused` / `unseen`), while the same verdict naming a
//!   statement of the window leaves on `close_write` -- the apply phase run on
//!   the shipped script.
//!
//! Free of a real provider by construction: the closer talks to a local stub
//! (GH #1283). Guarded like every template-reading test (GH #49).

// harness_fold: #[path = "support/memory_hive_subject.rs"]
#[allow(unused_imports)]
use crate::support; // harness_fold: mod support;

use meclaw_core::serde_json::{self, Value, json};
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::{emit_all, shipped_script};
use support::*;

const CLOSE: &str = "../../templates/memory-hive/close-glue/config.json";
const AUDIENCE: &str = r#"["agent:a","member:e"]"#;
const SAID: &str = "2026-02-01T10:00:00Z";
const QUOTE: &str = "I moved to Berlin last month, Hamburg is behind me.";

fn config(path: &str) -> Value {
    let path = format!("{}/{}", env!("CARGO_MANIFEST_DIR"), path);
    serde_json::from_str(&std::fs::read_to_string(&path).expect("config")).expect("json")
}

// ──────────────────────────────────────────────── (c) the apply phase

/// The meeting of one correction run: report, window and verdict, parked.
fn meeting(verdict: Value) -> Value {
    meeting_on(verdict, "", "")
}

/// The same meeting with a reported `turn` and the turn the read admitted
/// (`""` when the probe found none of the reporter's round).
fn meeting_on(verdict: Value, turn: &str, admitted: &str) -> Value {
    let report = json!({"subject": "member", "predicate": "lives_in", "quote": QUOTE,
                        "claim": "Berlin", "kind": "fact", "turn": turn, "session_id": "",
                        "note": "", "ref": "r9",
                        "audience": ["agent:a", "member:e"]});
    let window = json!({
        "turn": admitted,
        "axes": [{"subject": "member", "predicate": "lives_in",
                  "statements": [{"id": "f-city", "claim": "Hamburg",
                                  "since": SAID, "last_asserted": SAID}]}],
        "records": {"f-city": {"episode_id": "ep-f-city", "valid_from": SAID,
                               "fact_kind": "world", "confidence": 70,
                               "audience_set": AUDIENCE, "claim": "Hamburg"}},
        "truncated": false
    });
    let row = |kind: &str, payload: Value| {
        json!({"key": "correction:k1", "kind": kind, "created_at": SAID,
               "payload": payload.to_string()})
    };
    json!([
        row("verdict", verdict),
        row("window", window),
        row("report", report)
    ])
}

fn apply(verdict: Value) -> Vec<Value> {
    apply_meeting(meeting(verdict))
}

fn apply_meeting(meeting: Value) -> Vec<Value> {
    let out = emit_all(
        &shipped_script(CLOSE),
        &json!({
            "params": config(CLOSE)["params"].clone(),
            "header": {"context": {"mem_phase": "corr-apply|k1", "audience_set": AUDIENCE,
                                   "channel": "direct:e"},
                       "hop": {"operation": "bundle"}},
            "messages": [
                {"origin": "tool", "type": "tool_result", "id": "c-park-verdict", "text": "[]"},
                {"origin": "tool", "type": "tool_result", "id": "c-meet",
                 "text": meeting.to_string()}
            ]
        }),
    );
    assert_the_declaration_admits(CLOSE, &out);
    out
}

fn routed<'a>(out: &'a [Value], route: &str) -> Vec<&'a Value> {
    out.iter()
        .filter(|m| m["header"]["route"] == route)
        .collect()
}

#[test]
fn a_replaces_outside_the_window_closes_nothing() {
    let out = apply(json!({"decision": "replace", "replaces": "f-elsewhere",
                           "claim": "Berlin", "fact_kind": "world", "reason": "moved"}));
    assert!(routed(&out, "close_write").is_empty(), "{out:?}");
    let receipt = routed(&out, "correction_report");
    assert_eq!(receipt.len(), 1, "{out:?}");
    let head = &receipt[0]["header"];
    assert_eq!(head["result"], "refused", "{out:?}");
    assert_eq!(head["reason"], "unseen", "{out:?}");
    assert_eq!(head["replaces"], "f-elsewhere", "{out:?}");
    assert_eq!(head["correction_ref"], "r9", "{out:?}");
    assert_eq!(head["correction_key"], "k1", "{out:?}");

    // The same verdict on a statement of the window does leave: the guard is
    // the window, not the decision.
    let out = apply(json!({"decision": "replace", "replaces": "f-city",
                           "claim": "Berlin", "fact_kind": "world", "reason": "moved"}));
    let writes = routed(&out, "close_write");
    assert_eq!(writes.len(), 1, "{out:?}");
    let block: Value = serde_json::from_str(
        writes[0]["messages"][0]["text"]
            .as_str()
            .expect("block text"),
    )
    .expect("block json");
    assert_eq!(block["facts"][0]["replaces"], "f-city", "{block}");
    assert_eq!(block["facts"][0]["claim"], "Berlin", "{block}");
    assert_eq!(block["episode_id"], "ep-f-city", "{block}");
    assert_eq!(block["shown"][0]["predicate"], "lives_in", "{block}");
    let receipt = routed(&out, "correction_report");
    assert_eq!(receipt.len(), 1, "{out:?}");
    assert_eq!(receipt[0]["header"]["result"], "closed", "{out:?}");
}

// ──────────────────────────────────────────────── (a) / (b) the colony

fn seed(td: &tempfile::TempDir, table: &str, rows: &[Value]) {
    let store = td.path().join("main/memory-hive/store");
    let schema = read_json(&store.join("config.json"))["params"]["schema"][table].clone();
    let body: String = std::iter::once(json!({ "schema": schema }))
        .chain(rows.iter().cloned())
        .map(|r| serde_json::to_string(&r).expect("serialise") + "\n")
        .collect();
    std::fs::write(store.join(format!("seed/{table}.jsonl")), body).expect("seed");
}

/// A hive with one open statement `f-city` (the member lives in Hamburg) and
/// its turn, the closer pointed at a stub that answers `verdict`.
async fn colony_with(
    td: &tempfile::TempDir,
    verdict: Value,
) -> (
    meclaw_testing::ColonyHandle,
    tokio::sync::mpsc::Receiver<meclaw_core::Message>,
    tokio::task::JoinHandle<()>,
) {
    let mut city = fact("f-city", "member", "member", AUDIENCE, SAID);
    city["predicate"] = json!("lives_in");
    city["canonical_predicate"] = json!("lives_in");
    city["claim"] = json!("Hamburg");
    city["canonical_claim"] = json!("hamburg");
    build(td, &[city]);
    seed(
        td,
        "episodes",
        &[
            json!({"id": "ep-f-city", "session_id": "s-948", "turn_id": "ep-f-city",
                 "sender": "user", "speaker": "member:e", "channel": "direct:e",
                 "audience_set": AUDIENCE, "content": "I live in Hamburg.",
                 "happened_at": SAID, "recorded_at": SAID}),
        ],
    );
    // The turn is still in the queue: a correction speaks for one axis of it,
    // never for the turn, so it must stay there.
    seed(
        td,
        "pending_extraction",
        &[
            json!({"id": "p-city", "episode_id": "ep-f-city", "session_id": "s-948",
                 "channel": "direct:e", "audience_set": AUDIENCE, "sender": "user",
                 "content": "I live in Hamburg.", "token_est": 5, "enqueued_at": SAID,
                 "status": "pending", "batch_id": "", "claimed_at": "", "attempts": 0,
                 "not_before": "", "speaker": "member:e"}),
        ],
    );
    let answer = json!({
        "id": "chatcmpl-gh1323", "model": "closer",
        "choices": [{"message": {"role": "assistant", "content": verdict.to_string()},
                     "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5}
    });
    let (addr, stub) = meclaw_testing::mock_http::start_mock_server(
        meclaw_testing::mock_http::MockResponse::ok_json(answer.to_string().as_bytes()),
    )
    .await;
    let closer = td.path().join("main/memory-hive/closer/config.json");
    let mut cfg = read_json(&closer);
    cfg["params"]["base_url"] = json!(format!("http://{addr}"));
    std::fs::write(
        &closer,
        serde_json::to_string_pretty(&cfg).expect("serialise"),
    )
    .expect("write the closer");
    let (h, rx) = boot(td).await;
    (h, rx, stub)
}

fn report() -> meclaw_core::Message {
    let text = json!({"subject": "member", "predicate": "lives_in", "quote": QUOTE,
                      "claim": "Berlin", "ref": "r1"})
    .to_string();
    MessageBuilder::new(Path::new(HIVE))
        .hop(as_map(&json!({"route": "in_correction"})))
        // The session rides along, as it does when the counselor reports from
        // inside a conversation (review C1: without it the test hid the tool
        // form taking the block).
        .context(as_map(
            &json!({"channel": "direct:e", "audience_set": AUDIENCE, "session_id": "s-948"}),
        ))
        .body(Body::Inline(json!({"messages": [
            {"origin": "user", "type": "text", "id": "c1", "text": text}]})))
        .build()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_quoted_correction_supersedes_its_statement_before_the_next_talk() {
    if !shipped() {
        return;
    }
    let td = tempfile::tempdir().expect("tempdir");
    let verdict = json!({"decision": "replace", "replaces": "f-city", "claim": "Berlin",
                         "fact_kind": "world", "reason": "the member says they moved"});
    let (h, mut rx, _stub) = colony_with(&td, verdict).await;
    let db = db(&td);
    h.send(report()).await;
    let mut seen = Vec::new();
    let receipt = next_on(&mut rx, &mut seen, "correction_report", |_| true).await;
    assert_eq!(hop_str(&receipt, "result"), "closed", "{receipt:?}");
    assert_eq!(hop_str(&receipt, "correction_ref"), "r1", "{receipt:?}");
    assert_eq!(hop_str(&receipt, "replaces"), "f-city", "{receipt:?}");
    let source = format!("correction:{}", hop_str(&receipt, "correction_key"));

    meclaw_testing::wait::until("the correction lands in facts", || {
        let db = db.clone();
        let source = source.clone();
        async move {
            let old = rows(
                &db,
                "SELECT COALESCE(expired_at, '') FROM facts WHERE id = 'f-city'",
            );
            let new = rows(
                &db,
                "SELECT id FROM facts WHERE canonical_predicate = 'lives_in' \
                 AND claim = 'Berlin' AND expired_at IS NULL",
            );
            let named = rows(
                &db,
                &format!("SELECT id FROM facts WHERE closure_source = '{source}'"),
            );
            old.first().is_some_and(|r| !r[0].is_empty()) && new.len() == 1 && !named.is_empty()
        }
    })
    .await;
    h.quiesce().await;
    assert_eq!(
        rows(
            &db,
            "SELECT status FROM pending_extraction WHERE id = 'p-city'"
        ),
        vec![vec!["pending".to_string()]],
        "a correction does not answer for the turn it hangs on"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_correction_changes_nothing() {
    if !shipped() {
        return;
    }
    let td = tempfile::tempdir().expect("tempdir");
    let verdict = json!({"decision": "refuse", "reason": "the quote does not say so"});
    let (h, mut rx, _stub) = colony_with(&td, verdict).await;
    let db = db(&td);
    h.send(report()).await;
    let mut seen = Vec::new();
    let receipt = next_on(&mut rx, &mut seen, "correction_report", |_| true).await;
    assert_eq!(hop_str(&receipt, "result"), "refused", "{receipt:?}");
    assert_eq!(hop_str(&receipt, "reason"), "model_refused", "{receipt:?}");
    assert_eq!(
        hop_str(&receipt, "why"),
        "the quote does not say so",
        "{receipt:?}"
    );
    h.quiesce().await;
    let facts = rows(
        &db,
        "SELECT id, claim, COALESCE(expired_at, ''), COALESCE(closure_source, '') \
         FROM facts ORDER BY id",
    );
    assert_eq!(
        facts,
        vec![vec![
            "f-city".to_string(),
            "Hamburg".to_string(),
            String::new(),
            String::new()
        ]],
        "nothing closed, nothing added"
    );
}

// ──────────────────────────────────────────── review C1: the session rides along

const EXTRACT: &str = "../../templates/memory-hive/extract-glue/config.json";

/// The ops of the `xstore` lane, decoded.
fn store_ops(out: &[Value]) -> Vec<Value> {
    out.iter()
        .filter(|m| m["header"]["route"] == "xstore")
        .map(|m| {
            serde_json::from_str(m["messages"][0]["text"].as_str().expect("op text"))
                .expect("op args")
        })
        .collect()
}

#[test]
fn a_correction_with_its_session_still_parks_its_window() {
    // The block exactly as `corr-apply` writes it, arriving on the ingress with
    // the edge stamps AND the session the reporter spoke in. A receipt `closed`
    // without a parked `shown` would be a false receipt: the ingress would read
    // `replaces` as unseen and leave the wrong value open next to the new one.
    let shown = json!([{"subject": "member", "predicate": "lives_in",
                        "statements": [{"id": "f-city", "claim": "Hamburg",
                                        "since": SAID, "last_asserted": SAID}]}]);
    let block = json!({"episode_id": "ep-f-city", "audience_set": AUDIENCE,
                       "facts": [{"episode_id": "ep-f-city", "subject": "member",
                                  "predicate": "lives_in", "claim": "Berlin",
                                  "fact_kind": "world", "confidence": 70,
                                  "quote": QUOTE, "valid_from": SAID,
                                  "replaces": "f-city"}],
                       "shown": shown});
    let out = emit_all(
        &shipped_script(EXTRACT),
        &json!({
            "params": config(EXTRACT)["params"].clone(),
            "header": {"context": {"mem_phase": "inline", "session_id": "s-948",
                                   "channel": "direct:e", "audience_set": AUDIENCE,
                                   "close_pass": "1", "correction_key": "k1"},
                       "hop": {}},
            "messages": [{"origin": "assistant", "type": "tool_call", "id": "b",
                          "text": block.to_string()}]
        }),
    );
    let ops = store_ops(&out);
    let parked = |kind: &str| {
        ops.iter()
            .filter(|a| a["operation"] == "insert" && a["row"]["kind"] == kind)
            .count()
    };
    assert_eq!(parked("window"), 1, "the window is parked: {ops:?}");
    assert_eq!(
        parked("payload"),
        1,
        "the block is staged on its turn: {ops:?}"
    );
    assert_eq!(
        parked("inline"),
        0,
        "never taken for a `remember` call: {ops:?}"
    );
    assert!(
        !ops.iter().any(|a| a["table"] == "pending_extraction"),
        "and the turn stays in the queue: {ops:?}"
    );
}

// ─────────────────────────────────────── review I1: one round, edges that clean

fn read_phase(axis: Value, turn: Value) -> Vec<Value> {
    emit_all(
        &shipped_script(CLOSE),
        &json!({
            "params": config(CLOSE)["params"].clone(),
            "header": {"context": {"mem_phase": "corr-read|k1|r9", "audience_set": AUDIENCE,
                                   "channel": "direct:e"},
                       "hop": {"operation": "bundle"}},
            "messages": [
                {"origin": "tool", "type": "tool_result", "id": "c-park-report", "text": "[]"},
                {"origin": "tool", "type": "tool_result", "id": "c-axis",
                 "text": axis.to_string()},
                {"origin": "tool", "type": "tool_result", "id": "c-turn",
                 "text": turn.to_string()}
            ]
        }),
    )
}

/// The `rows` payload the read phase parks.
fn parked_rows(out: &[Value]) -> Value {
    // A bundle is one `tool_call` turn per op, its text the op's arguments.
    let op = out
        .iter()
        .filter(|m| m["header"]["route"] == "cstore")
        .flat_map(|m| m["messages"].as_array().cloned().unwrap_or_default())
        .filter_map(|t| serde_json::from_str::<Value>(t["text"].as_str()?).ok())
        .find(|a| a["operation"] == "insert" && a["row"]["kind"] == "rows")
        .unwrap_or_else(|| panic!("the read parks its rows: {out:?}"));
    serde_json::from_str(op["row"]["payload"].as_str().expect("payload")).expect("rows json")
}

#[test]
fn the_window_holds_only_the_reporters_round() {
    let row = |id: &str, audience: &str| {
        json!({"id": id, "episode_id": format!("ep-{id}"), "subject": "member",
               "canonical_subject": "member", "predicate": "lives_in",
               "canonical_predicate": "lives_in", "claim": "Hamburg",
               "canonical_claim": "hamburg", "fact_kind": "world", "confidence": 70,
               "valid_from": SAID, "recorded_at": SAID, "source": "member:e",
               "audience_set": audience})
    };
    let turn =
        |audience: &str| json!([{"id": "ep-new", "audience_set": audience, "channel": "direct:e"}]);
    let out = read_phase(
        json!([
            row("f-here", AUDIENCE),
            row("f-there", r#"["agent:a","member:o"]"#)
        ]),
        turn(AUDIENCE),
    );
    let parked = parked_rows(&out);
    let ids: Vec<&str> = parked["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .filter_map(|r| r["id"].as_str())
        .collect();
    assert_eq!(
        ids,
        vec!["f-here"],
        "GH #933: never another round's statement"
    );
    assert_eq!(
        parked["turn"], "ep-new",
        "the reported turn is of this round"
    );

    // A turn of another round is not admitted: an `add` cannot hang on it.
    let out = read_phase(json!([]), turn(r#"["agent:a","member:o"]"#));
    assert_eq!(parked_rows(&out)["turn"], "", "{out:?}");
}

#[test]
fn every_way_in_drops_the_correction_stamps() {
    // A caller may not set `correction_key` or `close_pass` on a way in: the one
    // would hijack a correction's verdict (or a close pass's), the other would
    // let a front model's block claim the close lane's window.
    let hive = config("../../templates/memory-hive/config.json");
    let ways_in: Vec<&Value> = hive["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .filter(|e| e["from"] == ".")
        .filter(|e| {
            ["./extract-glue", "./close-glue", "./writer"].contains(&e["to"].as_str().unwrap_or(""))
        })
        .collect();
    assert!(ways_in.len() >= 4, "{ways_in:?}");
    for e in ways_in {
        let gone: Vec<&str> = e["modifier"]["delete_context"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        for key in ["correction_key", "close_pass"] {
            assert!(gone.contains(&key), "{key} survives the way in {e}");
        }
    }
}

// ────────────────────────────────────────── review I2: an add hangs on a real turn

#[test]
fn an_add_hangs_only_on_a_turn_of_the_reporters_round() {
    let add = json!({"decision": "add", "claim": "Berlin", "fact_kind": "world",
                     "reason": "new"});
    let out = apply_meeting(meeting_on(add.clone(), "ep-new", ""));
    assert!(routed(&out, "close_write").is_empty(), "{out:?}");
    let receipt = routed(&out, "correction_report");
    assert_eq!(receipt.len(), 1, "{out:?}");
    assert_eq!(receipt[0]["header"]["result"], "refused", "{out:?}");
    assert_eq!(receipt[0]["header"]["reason"], "no_turn", "{out:?}");

    let out = apply_meeting(meeting_on(add, "ep-new", "ep-new"));
    let writes = routed(&out, "close_write");
    assert_eq!(writes.len(), 1, "{out:?}");
    assert_eq!(
        routed(&out, "correction_report")[0]["header"]["result"],
        "added"
    );
}

// ──────────────────────────── review M4: a refused store read names its run

#[test]
fn a_store_refusal_names_the_correction_it_stopped() {
    let out = emit_all(
        &shipped_script(CLOSE),
        &json!({
            "params": config(CLOSE)["params"].clone(),
            "header": {"context": {"mem_phase": "corr-window|k1|r9", "audience_set": AUDIENCE,
                                   "channel": "direct:e"},
                       "hop": {"operation": "select", "error_code": "sql_error"}},
            "messages": [{"origin": "tool", "type": "tool_result", "id": "c-meet",
                          "text": "no such table"}]
        }),
    );
    assert_the_declaration_admits(CLOSE, &out);
    let reject = routed(&out, "reject");
    assert_eq!(reject.len(), 1, "{out:?}");
    let head = &reject[0]["header"];
    assert_eq!(head["reject_reason"], "store_refused", "{out:?}");
    assert_eq!(head["correction_key"], "k1", "{out:?}");
    assert_eq!(head["correction_ref"], "r9", "{out:?}");
    assert_eq!(head["phase"], "correct", "{out:?}");
}

// ───────────────────────── review M2/M3: an axis is found in either spelling

#[test]
fn the_read_unites_every_spelling_of_the_axis() {
    let row = |id: &str, subject: &str| {
        json!({"id": id, "episode_id": format!("ep-{id}"), "subject": subject,
               "canonical_subject": "member", "predicate": "livesIn",
               "canonical_predicate": "lives_in", "claim": "Hamburg",
               "canonical_claim": "hamburg", "fact_kind": "world", "confidence": 70,
               "valid_from": SAID, "recorded_at": SAID, "source": "member:e",
               "audience_set": AUDIENCE})
    };
    let out = emit_all(
        &shipped_script(CLOSE),
        &json!({
            "params": config(CLOSE)["params"].clone(),
            "header": {"context": {"mem_phase": "corr-read|k1|r9", "audience_set": AUDIENCE,
                                   "channel": "direct:e"},
                       "hop": {"operation": "bundle"}},
            "messages": [
                {"origin": "tool", "type": "tool_result", "id": "c-park-report", "text": "[]"},
                {"origin": "tool", "type": "tool_result", "id": "c-axis",
                 "text": json!([row("f-a", "member")]).to_string()},
                {"origin": "tool", "type": "tool_result", "id": "c-axis-3",
                 "text": json!([row("f-a", "member"), row("f-b", "Member")]).to_string()}
            ]
        }),
    );
    let mut ids: Vec<String> = parked_rows(&out)["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .filter_map(|r| r["id"].as_str().map(str::to_owned))
        .collect();
    ids.sort();
    assert_eq!(
        ids,
        vec!["f-a", "f-b"],
        "one row per id, every leg read: {out:?}"
    );
}
