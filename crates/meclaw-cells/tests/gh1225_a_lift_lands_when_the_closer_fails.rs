//! GH #1225 -- the member taking a muted topic up again, and their own
//! request to forget, need no verdict; time is compared as time.
//!
//! The locks, with invented people only:
//! - B10: `takes_up` reads the times of the mute and the turn as times (UTC):
//!   a turn in the same second before the mute lifts nothing, an offset is an
//!   offset;
//! - B1: the lift leaves with the first closer call (phase `prompt`), and a
//!   pass whose closer fails -- an error, an answer that is no JSON, a group
//!   nobody can read, a verdict lost before the writes -- still sends the lifts
//!   and the member's own requests, then ends in the reject `closer_failed`
//!   (carrying `unmuted` and `mute_searched`), never in the sweep;
//! - B9: a window without a single round lifts too;
//! - B2 / E3: an everyday sentence that names no thing (the German for "I do
//!   not want to talk any more.") is no request, read or cited
//!   (`fixtures/mute_<code>.jsonl`, kind `no-request`);
//! - B12 / E2: one colony scenario through the real store -- the closer
//!   refuses, the lift lands on every row of the mute all the same.

#[path = "support/memory_hive_subject.rs"]
mod support;

use meclaw_core::serde_json::{self, Value, json};
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::{emit_all, shipped_script};
use support::*;

const CLOSE: &str = "../../templates/memory-hive/close-glue/config.json";
const MUTE_EN: &str = include_str!("fixtures/mute_en.jsonl");
const MUTE_DE: &str = include_str!("fixtures/mute_de.jsonl");
const AUDIENCE: &str = r#"["agent:a","member:e"]"#;
// OR-KF-48: a mark of this release carries its speaker; the older form
// without one is pinned by its own cold path (gh1225 same session).
const MARK: &str = "forget_request:r1@member:e";
const ASKED: &str = "2026-02-10T09:00:00Z";
const LATER: &str = "2026-03-01T09:00:00Z";
const TAKE_UP: &str = "Did my neighbour ever pay back the money he owes me?";
const OWN: &str = "Please forget what I told you about the bank loan.";

fn config(path: &str) -> Value {
    let path = format!("{}/{}", env!("CARGO_MANIFEST_DIR"), path);
    serde_json::from_str(&std::fs::read_to_string(&path).expect("config")).expect("json")
}

fn tool_calls(out: &[Value]) -> Vec<(String, Value)> {
    let mut found = Vec::new();
    for m in out {
        let phase = m["header"]["phase"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        for x in m["messages"].as_array().into_iter().flatten() {
            if x["type"] == "tool_call"
                && let Some(op) = x["text"]
                    .as_str()
                    .and_then(|t| serde_json::from_str::<Value>(t).ok())
            {
                found.push((phase.clone(), op));
            }
        }
    }
    found
}

fn turn_in(id: &str, sender: &str, text: &str, at: &str, audience: Option<&str>) -> Value {
    let speaker = if sender == "user" {
        "member:e"
    } else {
        "agent:a"
    };
    json!({"id": id, "session_id": "s-2", "sender": sender, "speaker": speaker,
           "content": text, "happened_at": at, "audience_set": audience,
           "channel": "direct:e"})
}

fn turn(id: &str, sender: &str, text: &str, at: &str) -> Value {
    turn_in(id, sender, text, at, Some(AUDIENCE))
}

fn parked(kind: &str, rows: Value) -> Value {
    json!({"key": "close:s-2", "kind": kind, "created_at": "2026-01-01T00:00:00Z",
           "payload": json!({"rows": rows, "truncated": false}).to_string()})
}

/// The meeting rows of a pass over `turns` with the mutes `mutes`, and the
/// parked verdict of group 0 where one is given.
fn meeting(turns: Vec<Value>, mutes: Value, verdict: Option<Value>) -> Value {
    let mut rows = vec![
        parked("turns", Value::Array(turns)),
        parked("facts", json!([])),
        parked("topics", json!([])),
        parked("exceptions", json!([])),
        parked("mutes", mutes),
    ];
    if let Some(v) = verdict {
        rows.push(json!({"key": "close:s-2", "kind": "verdict",
                         "created_at": "2026-01-01T00:00:00Z", "payload": v.to_string()}));
    }
    Value::Array(rows)
}

fn run(context: Value, hop: Value, text: String) -> Vec<Value> {
    emit_all(
        &shipped_script(CLOSE),
        &json!({
            "params": config(CLOSE)["params"].clone(),
            "header": {"context": context, "hop": hop},
            "messages": [{"origin": "tool", "type": "tool_result", "text": text}]
        }),
    )
}

/// A store answer to `phase` (`select`).
fn back(phase: &str, rows: Value) -> Vec<Value> {
    run(
        json!({"mem_phase": phase, "session_id": "s-2"}),
        json!({"operation": "select"}),
        rows.to_string(),
    )
}

/// One close-apply over `turns` with `verdict` (the closer's answer of group 0).
fn close_with(turns: Vec<Value>, mutes: Value, verdict: Value) -> Vec<Value> {
    let mut v = verdict;
    v["close_group"] = json!(0);
    run(
        json!({"mem_phase": "close-apply", "session_id": "s-2", "close_group": "0"}),
        json!({"operation": "select"}),
        meeting(turns, mutes, Some(v)).to_string(),
    )
}

fn nothing() -> Value {
    json!({"nothing_to_add": true, "forget": []})
}

fn mute_at(at: &str) -> Value {
    json!([{"mute_source": MARK, "mute_words": "money neighbour owes",
            "muted_at": at, "unmuted_at": null}])
}

fn lifts(out: &[Value]) -> Vec<Value> {
    tool_calls(out)
        .into_iter()
        .filter(|(p, _)| p == "unmuted")
        .map(|(_, o)| o)
        .collect()
}

/// The searches a pass sent for the request in turn `request`.
fn searches_of(out: &[Value], request: &str) -> Vec<String> {
    tool_calls(out)
        .into_iter()
        .map(|(p, _)| p)
        .filter(|p| p.starts_with("forget-") && p.contains(&format!("|{request}|")))
        .collect()
}

fn routed<'a>(out: &'a [Value], route: &str) -> Vec<&'a Value> {
    out.iter()
        .filter(|m| m["header"]["route"] == route)
        .collect()
}

// ──────────────────────────────────────────────── B10: time as time

#[test]
fn a_turn_in_the_same_second_before_the_mute_lifts_nothing() {
    let muted = "2026-02-10T09:00:00.500000Z";
    for (at, lifted) in [
        // the same second, half a second BEFORE the mute: as strings "Z" > "."
        ("2026-02-10T09:00:00Z", false),
        ("2026-02-10T09:00:01Z", true),
        // 10:00 at +02:00 is 08:00 UTC, before; 11:30 at +02:00 is after
        ("2026-02-10T10:00:00+02:00", false),
        ("2026-02-10T11:30:00+02:00", true),
        ("2026-02-10 09:00:00.400Z", false),
        ("2026-02-10 09:00:00.600Z", true),
    ] {
        let out = close_with(
            vec![turn("t1", "user", TAKE_UP, at)],
            mute_at(muted),
            nothing(),
        );
        assert_eq!(
            lifts(&out).len(),
            if lifted { 3 } else { 0 },
            "{at}: {:?}",
            lifts(&out)
        );
    }
}

// ──────────────────────────────────────────────── B1: no verdict needed

#[test]
fn a_lift_leaves_with_the_first_closer_call() {
    let out = back(
        "prompt",
        meeting(
            vec![turn("t1", "user", TAKE_UP, LATER)],
            mute_at(ASKED),
            None,
        ),
    );
    let lifted = lifts(&out);
    assert_eq!(lifted.len(), 3, "{out:?}");
    for op in &lifted {
        assert_eq!(op["where"]["mute_source"], MARK);
    }
    assert_eq!(
        routed(&out, "close").len(),
        1,
        "the closer is asked: {out:?}"
    );
    assert_the_declaration_admits(CLOSE, &out);
}

/// The salvage of a pass without a verdict, on the meeting rows: the lift,
/// the member's own request, and the reject -- no sweep, no report.
fn assert_salvaged(out: &[Value], why: &str) {
    assert_eq!(lifts(out).len(), 3, "{why}: {out:?}");
    assert!(!searches_of(out, "t2").is_empty(), "{why}: {out:?}");
    let rejects = routed(out, "reject");
    assert_eq!(rejects.len(), 1, "{why}: {out:?}");
    let h = &rejects[0]["header"];
    assert_eq!(h["reject_reason"], "closer_failed", "{why}");
    assert_eq!(h["unmuted"], 1, "{why}");
    assert_eq!(h["mute_searched"], 1, "{why}");
    assert!(
        tool_calls(out).iter().all(|(p, _)| p != "swept"),
        "{why}: no sweep without a verdict"
    );
    assert!(routed(out, "close_report").is_empty(), "{why}");
    assert!(routed(out, "close_write").is_empty(), "{why}");
    assert_the_declaration_admits(CLOSE, out);
}

fn salvage_turns() -> Vec<Value> {
    vec![
        turn("t1", "user", TAKE_UP, LATER),
        turn("t2", "user", OWN, "2026-03-01T09:01:00Z"),
    ]
}

#[test]
fn a_lift_and_an_own_request_land_without_a_verdict() {
    // the verdict phase reads the meeting again instead of rejecting outright
    for (context, hop, text, reason) in [
        (
            json!({"mem_phase": "verdict", "session_id": "s-2", "close_group": "0"}),
            json!({"finish_reason": "error"}),
            String::new(),
            "salvage|failed",
        ),
        (
            json!({"mem_phase": "verdict", "session_id": "s-2", "close_group": "0"}),
            json!({"finish_reason": "stop"}),
            "I can't help with that.".to_string(),
            "salvage|json",
        ),
        (
            json!({"mem_phase": "verdict", "session_id": "s-2", "close_group": "x"}),
            json!({"finish_reason": "stop"}),
            nothing().to_string(),
            "salvage|group:x",
        ),
    ] {
        let out = run(context, hop, text);
        let calls = tool_calls(&out);
        assert_eq!(calls.len(), 1, "{reason}: {out:?}");
        assert_eq!(calls[0].0, reason, "{out:?}");
        assert_eq!(calls[0].1["table"], "scratch");
        assert_eq!(calls[0].1["operation"], "select");
        assert!(routed(&out, "reject").is_empty(), "{reason}: {out:?}");
        assert_the_declaration_admits(CLOSE, &out);
    }
    // ... and the salvage sends what needs no verdict, then rejects
    for reason in ["salvage|failed", "salvage|json", "salvage|group:x"] {
        let out = back(reason, meeting(salvage_turns(), mute_at(ASKED), None));
        assert_salvaged(&out, reason);
        assert!(
            routed(&out, "reject")[0]["messages"][0]["text"]
                .as_str()
                .is_some_and(|t| t.contains("Nothing of a verdict was written")),
            "{out:?}"
        );
    }
    // the apply with its verdict lost (or its group unreadable) has the rows
    for group in ["0", "x"] {
        let out = run(
            json!({"mem_phase": "close-apply", "session_id": "s-2", "close_group": group}),
            json!({"operation": "select"}),
            meeting(salvage_turns(), mute_at(ASKED), None).to_string(),
        );
        assert_salvaged(&out, &format!("close-apply, group {group}"));
    }
}

// ──────────────────────────────────────────────── B9: no round, still a lift

#[test]
fn a_window_without_a_round_still_lifts() {
    let out = back(
        "prompt",
        meeting(
            vec![turn_in("t1", "user", TAKE_UP, LATER, None)],
            mute_at(ASKED),
            None,
        ),
    );
    assert_eq!(lifts(&out).len(), 3, "{out:?}");
    let report = routed(&out, "close_report");
    assert_eq!(report.len(), 1, "{out:?}");
    assert_eq!(report[0]["header"]["unmuted"], 1);
    assert_eq!(report[0]["header"]["unaudienced"], 1);
    assert!(routed(&out, "close").is_empty());
    assert_the_declaration_admits(CLOSE, &out);
}

// ──────────────────────────────────────────────── B2 / E3: no thing, no request

fn cases(kind: &str) -> Vec<Value> {
    let of = |src: &str| -> Vec<Value> {
        src.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<Value>(l).expect("fixture line"))
            .filter(|c| c["test"] == kind)
            .collect()
    };
    let (en, de) = (of(MUTE_EN), of(MUTE_DE));
    assert!(
        !en.is_empty() && !de.is_empty(),
        "{kind}: a case in every language"
    );
    en.into_iter().chain(de).collect()
}

#[test]
fn an_everyday_form_without_a_thing_asks_nothing() {
    for c in cases("no-request") {
        let text = c["text"].as_str().expect("text").to_string();
        // read by the fallback: no search of its own
        let out = close_with(vec![turn("t1", "user", &text, LATER)], json!([]), nothing());
        assert!(searches_of(&out, "t1").is_empty(), "{text}: {out:?}");
        // cited by the closer: not taken at its word
        let out = close_with(
            vec![turn("t1", "user", &text, LATER)],
            json!([]),
            json!({"forget": [{"episode_id": "t1", "words": "money neighbour geld nachbarn"}]}),
        );
        assert!(searches_of(&out, "t1").is_empty(), "{text}: {out:?}");
        let report = routed(&out, "close_report");
        assert_eq!(report[0]["header"]["unseen_refs"], 1, "{text}: {out:?}");
    }
}

/// Review KM-M1 I2 (plan § 1 B2): with `forget_fallback` off the closer's
/// citation is the one way an everyday form is honoured -- and it is taken
/// (searched, no `unseen_refs`), every form of every language.
#[test]
fn a_closer_citation_of_an_everyday_form_is_taken() {
    for c in cases("keeps-aside") {
        let text = c["text"].as_str().expect("text").to_string();
        let words = c["words"].as_str().expect("words").to_string();
        let mut v = json!({"forget": [{"episode_id": "t1", "words": words}]});
        v["close_group"] = json!(0);
        let mut params = config(CLOSE)["params"].clone();
        params["forget_fallback"] = json!(false);
        let out = emit_all(
            &shipped_script(CLOSE),
            &json!({
                "params": params,
                "header": {"context": {"mem_phase": "close-apply", "session_id": "s-2",
                                       "close_group": "0"},
                           "hop": {"operation": "select"}},
                "messages": [{"origin": "tool", "type": "tool_result",
                              "text": meeting(vec![turn("t1", "user", &text, LATER)],
                                              json!([]), Some(v)).to_string()}]
            }),
        );
        assert!(!searches_of(&out, "t1").is_empty(), "{text}: {out:?}");
        let report = routed(&out, "close_report");
        assert_eq!(report[0]["header"]["unseen_refs"], 0, "{text}: {out:?}");
    }
}

// ──────────────────────────────────────────────── B3: a question without its mark

#[test]
fn a_question_without_its_mark_is_still_a_question() {
    let mute = json!([{"mute_source": MARK, "mute_words": "geld nachbarn",
                       "muted_at": ASKED, "unmuted_at": null}]);
    for (asked, lifted) in [
        // speech-to-text writes the question without its mark
        ("Welches Geld meinst du", false),
        ("Welches Geld meinst du?", false),
        ("Which money do you mean", false),
        // a statement of the other side is no question: the yes takes it up
        ("Das Geld ist angekommen.", true),
    ] {
        let out = close_with(
            vec![
                turn("t0", "assistant", asked, LATER),
                turn(
                    "t1",
                    "user",
                    "Ja, das Geld vom Nachbarn.",
                    "2026-03-01T09:00:30Z",
                ),
            ],
            mute.clone(),
            nothing(),
        );
        assert_eq!(
            lifts(&out).len(),
            if lifted { 3 } else { 0 },
            "{asked}: {:?}",
            lifts(&out)
        );
    }
}

// ──────────────────────────────────────────────── B12 / E2: through the colony

/// The store's seed of `table`: the declared schema first, then the rows. The
/// store loads it into a fresh `cell.db` on its first wake -- before the close
/// pass is handled, through its own inserts (the search index included).
fn seed(td: &tempfile::TempDir, table: &str, rows: &[Value]) {
    let store = td.path().join("main/memory-hive/store");
    let schema = read_json(&store.join("config.json"))["params"]["schema"][table].clone();
    let body: String = std::iter::once(json!({ "schema": schema }))
        .chain(rows.iter().cloned())
        .map(|r| serde_json::to_string(&r).expect("serialise") + "\n")
        .collect();
    std::fs::write(store.join(format!("seed/{table}.jsonl")), body).expect("seed");
}

fn episode(id: &str, session: &str, text: &str, at: &str, mark: Option<Value>) -> Value {
    let mut row = json!({"id": id, "session_id": session, "turn_id": id, "sender": "user",
                         "speaker": "member:e", "channel": "direct:e", "audience_set": AUDIENCE,
                         "content": text, "happened_at": at, "recorded_at": at});
    if let Some(unmuted) = mark {
        row["mute_source"] = json!(MARK);
        row["mute_words"] = json!("money neighbour owes");
        row["muted_at"] = json!(ASKED);
        row["unmuted_at"] = unmuted;
    }
    row
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lift_lands_through_the_colony_when_the_closer_refuses() {
    if !shipped() {
        return;
    }
    let td = tempfile::tempdir().expect("tempdir");
    // the mute (an empty and a missing `unmuted_at`), an old episode of the
    // thing the member asks to forget now, and the session being closed
    let mut owed = fact("f-owed", "member", "member", AUDIENCE, ASKED);
    owed["episode_id"] = json!("e-owed");
    owed["mute_source"] = json!(MARK);
    owed["mute_words"] = json!("money neighbour owes");
    owed["muted_at"] = json!(ASKED);
    owed["unmuted_at"] = Value::Null;
    build(&td, &[owed]);
    seed(
        &td,
        "episodes",
        &[
            episode(
                "req-r1",
                "s-1",
                "Please forget the money my neighbour owes me.",
                ASKED,
                Some(json!("")),
            ),
            episode(
                "e-owed",
                "s-0",
                "My neighbour owes me money since spring.",
                "2026-02-01T10:00:00Z",
                Some(Value::Null),
            ),
            episode(
                "e-loan",
                "s-0",
                "The bank loan for the roof worries me.",
                "2026-02-02T10:00:00Z",
                None,
            ),
            episode("t1", "s-2", TAKE_UP, LATER, None),
            episode("t2", "s-2", OWN, "2026-03-01T09:01:00Z", None),
        ],
    );
    // the closer answers, and what it answers is no verdict
    let refusal = json!({
        "id": "chatcmpl-gh1225", "model": "closer",
        "choices": [{"message": {"role": "assistant", "content": "I can't help with that."},
                     "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5}
    });
    let (addr, _stub) = meclaw_testing::mock_http::start_mock_server(
        meclaw_testing::mock_http::MockResponse::ok_json(refusal.to_string().as_bytes()),
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
    let (h, mut rx) = boot(&td).await;
    let db = db(&td);

    h.send(
        MessageBuilder::new(Path::new(HIVE))
            .hop(as_map(&json!({"route": "in_close_pass"})))
            .context(as_map(&json!({"session_id": "s-2", "channel": "direct:e",
                                    "audience_set": AUDIENCE})))
            .body(Body::Inline(json!({"messages": [
                {"origin": "user", "type": "text", "id": "m1", "text": "close"}]})))
            .build(),
    )
    .await;
    let mut seen = Vec::new();
    let reject = next_on(&mut rx, &mut seen, "reject", |m| {
        hop_str(m, "reject_reason") == "closer_failed"
    })
    .await;
    assert_eq!(hop_str(&reject, "unmuted"), "1", "{reject:?}");
    assert_eq!(hop_str(&reject, "mute_searched"), "1", "{reject:?}");
    h.quiesce().await;

    for table in ["episodes", "facts"] {
        let marked = rows(
            &db,
            &format!(
                "SELECT id, COALESCE(muted_at, ''), COALESCE(unmuted_at, '') FROM {table} \
                 WHERE mute_source = '{MARK}' ORDER BY id"
            ),
        );
        assert!(!marked.is_empty(), "{table}: the mute is in the store");
        for r in &marked {
            assert!(
                !r[2].is_empty() && r[2] >= r[1],
                "{table}.{}: unmuted_at {:?} after muted_at {:?}",
                r[0],
                r[2],
                r[1]
            );
        }
    }
    let loan = rows(
        &db,
        "SELECT COALESCE(mute_source, '') FROM episodes WHERE id = 'e-loan'",
    );
    assert_eq!(loan, vec![vec!["forget_request:t2@member:e".to_string()]]);
    assert!(
        seen.iter().all(|m| hop_str(m, "route") != "close_report"),
        "no report without a verdict: {:?}",
        seen.iter().map(|m| hop_str(m, "route")).collect::<Vec<_>>()
    );
}
