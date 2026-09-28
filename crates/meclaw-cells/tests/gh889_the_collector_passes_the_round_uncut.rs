//! GH #889 — the collector collects and passes the round on UNCUT.
//!
//! Until `collector@5.0.0` the collector was two things at once: the fan-in of a
//! round (turn, memory leg, brief, tool iterations) and the owner of the model's
//! window — a rolling read of the last N turns, per-item and per-round byte caps,
//! five curation stages with stubs, recoverability classes, a `thread_recall`
//! tool to undo its own stubs, a `system.budget` sentence, and the episode,
//! close, pack and prune lanes that hung off the history it kept.
//!
//! R-27-1 gives the window to ONE owner per model: the curator hive that stands
//! between the collector and the brain. A second cutter in front of it would cut
//! what the curator never saw, so it could never be the whole owner. What is left
//! here is the fan-in: the collector gathers the running round and hands every
//! item of it on, once and whole, on route `curate` — and nothing of an earlier
//! turn, because the history is the curator's.
//!
//! Three locks:
//!
//! 1. statically, no knob, lane or route of the removed mechanics is left in
//!    `templates/collector/**`, and the seam is named `curate`;
//! 2. behaviourally, over the SHIPPED script: a 20 000-character tool result
//!    reaches `curate` byte for byte, the second iteration carries the first one
//!    with it, and an older turn of the same session never rides along;
//! 3. what the collector still holds past a round is bookkeeping with an end:
//!    the ids an advisor answered under and the peer mark ride on the session's
//!    `depart` rows into the NEXT round (a question the core asks back is
//!    answered by the person later), a newer departure under an id replaces the
//!    older one, a departure falls after seven days on its own clock, and every
//!    round nothing else ends falls at a turn-open once its LAST row is older
//!    than the frist -- no row lives forever without `prune`.
//!
//! Everything runs the shipped `params.script_inline` over real stdin documents.
//! Nothing is mocked, no provider is called, nothing is spent.

#[path = "support/assemble_cell.rs"]
mod assemble_cell;

use assemble_cell::{
    ASSEMBLE, DISPATCHER, SESSION, advice, assemble, bundle_reply, call, calls_of, config_of,
    in_phase, lane, now_ms, on_route, open_advice, repo, run_cell,
};
use serde_json::{Value, json};

/// The knobs GH #889 removes, all nineteen of them.
const GONE_KNOBS: [&str; 19] = [
    "window_turns",
    "window_bytes",
    "turn_chars",
    "tool_chars",
    "round_bytes",
    "memory_chars",
    "context_window",
    "curate_soft",
    "curate_hard",
    "keep_rounds",
    "recoverability",
    "thread_recall",
    "thread_recall_budget",
    "tool_menu",
    "tool_desc_chars",
    "curate_slot_chars",
    "curate_budget_line",
    "turn_write",
    "prune_after_ms",
];

/// The lanes the hive no longer accepts.
const GONE_LANES: [&str; 4] = ["in_thread_call", "in_pack", "in_close", "in_prune"];

/// The routes the collector no longer emits (`condense` was reserved and never
/// sent; it goes with the rest).
const GONE_ROUTES: [&str; 7] = [
    "brain",
    "turn_write",
    "write",
    "pack",
    "pack_ack",
    "prune",
    "condense",
];

const HIVE: &str = "templates/collector/config.json";

/// Every `config.json` below `templates/collector/`, as (relative path, json).
fn collector_configs() -> Vec<(String, Value)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.file_name().and_then(|n| n.to_str()) == Some("config.json") {
                out.push(path);
            }
        }
    }
    let root = repo("templates/collector");
    let mut paths = Vec::new();
    walk(&root, &mut paths);
    paths.sort();
    assert!(
        paths.len() >= 3,
        "the hive, `assemble` and `window` at least: {paths:?}"
    );
    paths
        .into_iter()
        .map(|p| {
            let raw =
                std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            let v: Value =
                serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            (p.display().to_string(), v)
        })
        .collect()
}

fn routes_of(list: &Value) -> Vec<String> {
    list.as_array()
        .expect("a route list")
        .iter()
        .map(|e| e["route"].as_str().expect("route").to_string())
        .collect()
}

/// The knob names the script READS -- `_int("…")`, `_str("…")`, `_list("…")`,
/// `_knob("…")`. A hop key that happens to share a knob's name (`window_turns`
/// stays on the seam, zero-valued) is not a knob; a read is.
fn knobs_read(script: &str) -> Vec<String> {
    let mut out = Vec::new();
    for reader in ["_int(\"", "_str(\"", "_list(\"", "_float(\"", "_knob(\""] {
        let mut rest = script;
        while let Some(i) = rest.find(reader) {
            let tail = &rest[i + reader.len()..];
            let end = tail.find('"').expect("a closing quote");
            out.push(tail[..end].to_string());
            rest = &tail[end..];
        }
    }
    out
}

#[test]
fn collector_declares_no_cap_knob() {
    // 1. No knob of the removed mechanics, anywhere in the hive: not as a param,
    //    not as a declared setting, and not as a read in the script.
    for (path, cfg) in collector_configs() {
        let params = cfg["params"].as_object().cloned().unwrap_or_default();
        let settings = cfg["contract"]["settings"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        for knob in GONE_KNOBS {
            assert!(
                !params.contains_key(knob),
                "{path}: params still carry `{knob}` (GH #889)"
            );
            assert!(
                !settings.contains_key(knob),
                "{path}: contract.settings still declares `{knob}` (GH #889)"
            );
        }
    }
    let assemble_cfg = config_of(ASSEMBLE);
    let script = assemble_cfg["params"]["script_inline"]
        .as_str()
        .expect("script_inline");
    let read = knobs_read(script);
    assert!(
        read.iter().any(|k| k == "max_iter"),
        "the reader finds the knobs that stay: {read:?}"
    );
    for knob in GONE_KNOBS {
        assert!(
            !read.iter().any(|k| k == knob),
            "the script still reads `{knob}` (GH #889): {read:?}"
        );
    }

    // 2. The hive's contract: the removed lanes are no doors, the removed routes
    //    leave nowhere, and the seam is `curate`.
    let hive = config_of(HIVE);
    let accepts = routes_of(&hive["params"]["contract"]["accepts"]);
    let emits = routes_of(&hive["params"]["contract"]["emits"]);
    for lane in GONE_LANES {
        assert!(
            !accepts.iter().any(|r| r == lane),
            "the hive still accepts `{lane}` (GH #889): {accepts:?}"
        );
    }
    for route in GONE_ROUTES {
        assert!(
            !emits.iter().any(|r| r == route),
            "the hive still emits `{route}` (GH #889): {emits:?}"
        );
    }
    assert!(
        emits.iter().any(|r| r == "curate"),
        "the seam is `curate` (GH #889): {emits:?}"
    );

    // 3. The assembler's own route vocabulary says the same.
    let values: Vec<String> = assemble_cfg["contract"]["emits"]["hop"]["route"]["values"]
        .as_array()
        .expect("route values")
        .iter()
        .map(|v| v.as_str().expect("a route name").to_string())
        .collect();
    assert!(values.iter().any(|v| v == "curate"), "{values:?}");
    for route in GONE_ROUTES {
        assert!(
            !values.iter().any(|v| v == route),
            "`assemble` still declares route `{route}` (GH #889): {values:?}"
        );
    }
}

// ───────────────────────────────────────────────────────────── the round, uncut

const SESSION_TURN: &str = "t1";
const QUESTION: &str = "what is in the notes file?";

/// A tool result far above every cap the collector used to apply (`tool_chars`
/// 4000, `round_bytes` 16000).
fn big_result() -> String {
    "0123456789".repeat(2000)
}

fn tool_call(id: &str, name: &str) -> Value {
    json!({"origin": "assistant", "type": "tool_call", "id": id,
           "text": json!({"name": name, "arguments": "{\"path\": \"notes.txt\"}"}).to_string()})
}

fn tool_result(id: &str, text: &str) -> Value {
    json!({"origin": "tool", "type": "tool_result", "id": id, "text": text})
}

/// A materialised `leg-window` row: the turn that opened the round.
fn leg_window_row() -> Value {
    let leg = json!({"turns": [{"role": "user", "text": QUESTION, "consult_id": ""}],
                     "deferred": 0, "deferred_turns": []});
    json!({"turn_id": SESSION_TURN, "iter": 0, "role": "leg-window",
           "turn": leg.to_string(), "fired": 1, "recorded_at": "2026-09-29T10:00:00.000000Z"})
}

fn round_row(iter: i64, role: &str, turn: Value, fired: i64) -> Value {
    json!({"turn_id": SESSION_TURN, "iter": iter, "role": role,
           "turn": turn.to_string(), "fired": fired,
           "recorded_at": "2026-09-29T10:00:01.000000Z"})
}

/// The round-check reply of iteration `iter`: the slate read back after the
/// last result of that iteration parked.
fn round_check(iter: i64, rows: Value) -> Value {
    let mut doc = bundle_reply("round-check", SESSION_TURN, &[("c-round-check-read", rows)]);
    doc["header"]["context"]["iter"] = json!(iter.to_string());
    doc
}

/// The ONE emission on `curate`.
fn curate_of(out: &[Value]) -> Value {
    let seams: Vec<&Value> = out
        .iter()
        .filter(|m| m["header"]["route"] == "curate")
        .collect();
    assert_eq!(seams.len(), 1, "ONE curate: {out:?}");
    seams[0].clone()
}

fn messages(msg: &Value) -> Vec<Value> {
    msg["messages"].as_array().expect("messages").clone()
}

fn position(msgs: &[Value], kind: &str, id: &str) -> usize {
    msgs.iter()
        .position(|m| m["type"] == kind && m["id"] == id)
        .unwrap_or_else(|| panic!("no {kind} `{id}` on curate: {msgs:?}"))
}

#[test]
fn curate_carries_the_whole_round_uncut() {
    let payload = big_result();
    assert_eq!(payload.len(), 20_000);

    // ── the first iteration completes: its result reaches curate WHOLE.
    let first = json!([
        leg_window_row(),
        round_row(0, "assistant", json!([tool_call("c1", "read_file")]), 0),
        round_row(0, "tool", tool_result("c1", &payload), 0),
    ]);
    let out = assemble(&[], round_check(0, first));
    let seam = curate_of(&out);
    let msgs = messages(&seam);
    let got = &msgs[position(&msgs, "tool_result", "c1")];
    assert_eq!(
        got["text"].as_str().map(str::len),
        Some(20_000),
        "the result reaches curate at its full length: {seam}"
    );
    assert_eq!(got["text"], payload, "byte for byte, no stub, no cut");
    assert_eq!(
        msgs[0]["text"], QUESTION,
        "the round opens with its turn: {seam}"
    );
    assert_eq!(seam["header"]["iter"], "1", "{seam}");

    // ── the second iteration completes: it carries the first one with it.
    let second = json!([
        leg_window_row(),
        round_row(0, "assistant", json!([tool_call("c1", "read_file")]), 1),
        round_row(0, "tool", tool_result("c1", &payload), 0),
        round_row(1, "assistant", json!([tool_call("c2", "web_search")]), 0),
        round_row(1, "tool", tool_result("c2", "two hits"), 0),
    ]);
    let out = assemble(&[], round_check(1, second));
    let seam = curate_of(&out);
    let msgs = messages(&seam);
    let call_1 = position(&msgs, "tool_call", "c1");
    let result_1 = position(&msgs, "tool_result", "c1");
    let call_2 = position(&msgs, "tool_call", "c2");
    let result_2 = position(&msgs, "tool_result", "c2");
    assert!(
        call_1 < result_1 && result_1 < call_2 && call_2 < result_2,
        "the round in plain order, the first iteration in front: {msgs:?}"
    );
    assert_eq!(
        msgs[result_1]["text"], payload,
        "the first iteration's result is still whole in the second: {seam}"
    );
    assert_eq!(seam["header"]["iter"], "2", "{seam}");
    assert_eq!(
        msgs.iter().filter(|m| m["id"] == "c1").count(),
        2,
        "each item once -- one call, one result: {msgs:?}"
    );

    // ── a turn opens in a session that still holds an older turn's rows: the
    //    round carries its own turn and nothing of the older one.
    let rows = json!([
        {"id": "2026-09-29T09:00:00.000000Z-aaaa0000", "session_id": "s1", "turn_id": "t0",
         "role": "user", "content": "an older question", "deferred": 0, "consult_id": ""},
        {"id": "2026-09-29T09:00:01.000000Z-aaaa0001", "session_id": "s1", "turn_id": "t0",
         "role": "assistant", "content": "an older answer", "deferred": 0, "consult_id": ""},
        {"id": "2026-09-29T10:00:00.000000Z-bbbb0000", "session_id": "s1", "turn_id": SESSION_TURN,
         "role": "user", "content": QUESTION, "deferred": 0, "consult_id": ""}
    ]);
    let opened = assemble(
        &[],
        bundle_reply(
            "turn-open",
            SESSION_TURN,
            &[
                ("c-open-round", json!([])),
                ("c-open-win", rows),
                ("c-open-scope", json!([])),
                ("c-open-roster", json!([])),
            ],
        ),
    );
    let parked = call(in_phase(&opened, "collect"), "c-collect-row")["row"].clone();
    assert_eq!(parked["role"], "leg-window", "{parked}");
    let out = assemble(
        &[],
        bundle_reply(
            "collect",
            SESSION_TURN,
            &[("c-collect-read", json!([parked]))],
        ),
    );
    let seam = curate_of(&out);
    let texts: Vec<String> = messages(&seam)
        .iter()
        .map(|m| m["text"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        texts,
        vec![QUESTION.to_string()],
        "only the round's own turn rides on curate -- the history is the curator's: {seam}"
    );
    assert!(
        !seam.to_string().contains("an older"),
        "no trace of the older turn anywhere in the emission: {seam}"
    );
}

// ── lock 3: what the collector holds past a round, and when it falls ─────────
//
// OR-KX-63: an id an advisor answered under, and whether the other side spoke
// in the session, outlive the round that showed them -- on the session's
// `depart` rows, the bookkeeping of calls that left, never a history. OR-KX-68:
// every row the collector writes has an end -- a round's rows its answer, or
// the frist at a later turn-open once the round's LAST row is that old (its
// brain call failed); a departure a newer one under the same id, or seven days
// on its own clock (a consult nobody answers, a quiet session's peer mark).
// The frist of a round never reaches a departure: a question asked back and an
// advisor that thinks long keep theirs past it.

/// The turn the consult left from.
const MEMBER_TURN: &str = "chat#0f1e2d3c4b5a6978";

/// A `depart` row as the turn-open read hands it back.
fn depart_row(correlation: &str, body: Value, at: &str) -> Value {
    json!({"correlation": correlation, "turn": body.to_string(), "recorded_at": at})
}

/// The person's words, as a channel hands them to the hive.
fn user_turn(text: &str) -> Value {
    lane(
        "in_turn",
        json!({"turn_id": SESSION_TURN}),
        json!({"turn_id": SESSION_TURN}),
        json!([{"origin": "user", "type": "text", "text": text}]),
    )
}

/// Open a turn over the session's `departs` and return the `curate` of its
/// first assembly: turn-open reply -> the parked window leg -> collect reply.
fn curate_over(departs: Value) -> Value {
    let rows = json!([{"id": "2026-09-29T10:00:00.000000Z-bbbb0000", "session_id": SESSION,
                       "turn_id": SESSION_TURN, "role": "user", "content": "berlin",
                       "deferred": 0, "consult_id": ""}]);
    let opened = assemble(
        &[],
        bundle_reply(
            "turn-open",
            SESSION_TURN,
            &[
                ("c-open-round", json!([])),
                ("c-open-win", rows),
                ("c-open-depart", departs),
                ("c-open-scope", json!([])),
                ("c-open-roster", json!([])),
            ],
        ),
    );
    let parked = call(in_phase(&opened, "collect"), "c-collect-row")["row"].clone();
    curate_of(&assemble(
        &[],
        bundle_reply(
            "collect",
            SESSION_TURN,
            &[("c-collect-read", json!([parked]))],
        ),
    ))
}

fn ids_of(msg: &Value) -> Vec<String> {
    calls_of(msg).into_iter().map(|(id, _)| id).collect()
}

fn pos(ids: &[String], id: &str) -> usize {
    ids.iter()
        .position(|i| i == id)
        .unwrap_or_else(|| panic!("no `{id}` in {ids:?}"))
}

#[test]
fn an_answered_consult_stays_open_in_the_next_round_of_its_session() {
    // The round in which the person answers the core's question carries no
    // advice turn: the id comes off the session's departures -- and only off
    // one the advisor has ANSWERED under; before that there is nothing to
    // reply to (the reading of collector@4.4.1: ids of advice turns only).
    let seam = curate_over(json!([
        depart_row(
            "k-7",
            json!({"id": "call-1", "name": "consult_cogny", "answered": 1}),
            "2026-09-29T09:59:00.000000Z"
        ),
        depart_row(
            "k-8",
            json!({"id": "call-2", "name": "consult_cogny"}),
            "2026-09-29T09:59:30.000000Z"
        ),
    ]));
    assert_eq!(
        seam["system"]["consult"]["open"],
        json!(["k-7"]),
        "the answered id rides into the next round, the unanswered one does not: {seam}"
    );
    let text = seam["system"]["consult"]["text"]
        .as_str()
        .expect("consult text");
    assert!(
        text.starts_with("open consults: k-7\n"),
        "the model can pass the id back because it sees it: {text}"
    );
    assert_eq!(
        seam["system"]["instructions"]["peer"]["text"], "",
        "no peer mark, no peer rule: {seam}"
    );

    // The departure fell -- replaced by a newer one, or past the frist: the
    // slot is SENT empty, because a slot path that is not sent is a path
    // nothing revokes (GH #259).
    let seam = curate_over(json!([]));
    assert_eq!(seam["system"]["consult"]["open"], json!([]), "{seam}");
    assert_eq!(seam["system"]["consult"]["text"], "", "{seam}");
}

#[test]
fn the_peer_mark_keeps_the_peer_rule_beside_the_curators_window() {
    // The curator keeps showing an earlier peer turn; the rule that says whose
    // words those are stays beside it after the round's own rows are gone.
    let seam = curate_over(json!([depart_row(
        "",
        json!({"peer": 1}),
        "2026-09-29T09:58:00.000000Z"
    )]));
    let rule = seam["system"]["instructions"]["peer"]["text"]
        .as_str()
        .expect("peer text");
    assert!(
        rule.contains("someone else's words"),
        "the peer rule stands on the mark alone: {seam}"
    );
    assert_eq!(
        seam["system"]["consult"]["open"],
        json!([]),
        "a peer mark is no consult: {seam}"
    );
}

#[test]
fn a_turn_with_peer_words_leaves_one_peer_mark_in_its_session() {
    let out = assemble(
        &[],
        lane(
            "in_turn",
            json!({"turn_id": SESSION_TURN}),
            json!({"turn_id": SESSION_TURN}),
            json!([{"origin": "peer", "type": "text", "text": "hi, Jonas here"}]),
        ),
    );
    let open = in_phase(&out, "turn-open");
    let drop = call(open, "c-open-peer-drop");
    assert_eq!(drop["operation"], "delete");
    assert_eq!(
        drop["where"],
        json!({"session_id": SESSION, "role": "depart", "correlation": ""}),
        "the older mark of the session goes -- one mark per session: {drop}"
    );
    let mark = call(open, "c-open-peer-mark")["row"].clone();
    assert_eq!(mark["role"], "depart", "{mark}");
    assert_eq!(mark["session_id"], SESSION, "{mark}");
    assert_eq!(
        mark["correlation"], "",
        "no id an advice lookup could ever ask for: {mark}"
    );
    let body: Value = serde_json::from_str(mark["turn"].as_str().expect("turn")).expect("json");
    assert_eq!(body, json!({"peer": 1}), "{mark}");
    let ids = ids_of(open);
    assert!(
        pos(&ids, "c-open-peer-drop") < pos(&ids, "c-open-peer-mark")
            && pos(&ids, "c-open-peer-mark") < pos(&ids, "c-open-depart"),
        "drop, mark, then the read that sees it: {ids:?}"
    );

    // The person's own words leave no mark.
    let ids = ids_of(in_phase(&assemble(&[], user_turn("hello")), "turn-open"));
    assert!(
        !ids.iter().any(|i| i.starts_with("c-open-peer-")),
        "{ids:?}"
    );
}

#[test]
fn an_advice_marks_its_departure_answered_and_restarts_its_clock() {
    // Stage A asks for the departure's body, so stage B can mark it.
    let a = assemble(&[], advice("k-7", "which city?"));
    let look = call(in_phase(&a, "advice-look"), "c-look-depart");
    assert!(
        look["columns"]
            .as_array()
            .expect("columns")
            .contains(&json!("turn")),
        "{look}"
    );

    let body = json!({"id": "call-1", "name": "consult_cogny"});
    let (_key, out) = open_advice(
        "k-7",
        json!([{"turn_id": MEMBER_TURN, "deadline_ms": now_ms() + 600_000,
                "turn": body.to_string()}]),
    );
    let open = in_phase(&out, "turn-open");
    let mark = call(open, "c-open-answered");
    assert_eq!(mark["operation"], "update", "{mark}");
    assert_eq!(mark["table"], "round", "{mark}");
    assert_eq!(
        mark["where"],
        json!({"session_id": SESSION, "role": "depart", "turn_id": MEMBER_TURN,
               "correlation": "k-7"}),
        "exactly the departure the advice was found by: {mark}"
    );
    let set: Value =
        serde_json::from_str(mark["set"]["turn"].as_str().expect("turn json")).expect("json");
    assert_eq!(
        set,
        json!({"id": "call-1", "name": "consult_cogny", "answered": 1}),
        "the body keeps what it said and gains the mark: {mark}"
    );
    assert!(
        mark["set"]["recorded_at"]
            .as_str()
            .is_some_and(|t| t.ends_with('Z')),
        "the frist counts from the advisor's answer: {mark}"
    );
    let ids = ids_of(open);
    assert_eq!(ids[0], "c-open-rekey", "the re-key stays first: {ids:?}");
    assert!(
        pos(&ids, "c-open-answered") < pos(&ids, "c-open-depart"),
        "the advice round reads its own mark: {ids:?}"
    );

    // No departure, nothing to mark.
    let (_key, out) = open_advice("k-9", json!([]));
    assert!(
        !ids_of(in_phase(&out, "turn-open")).contains(&"c-open-answered".to_string()),
        "{out:?}"
    );
}

#[test]
fn a_newer_departure_under_the_same_id_replaces_the_older_one() {
    // The model passing an id back IS the answer to the question the advisor
    // asked under it: the older departure of that id is done.
    let reply = json!({"name": "consult_cogny",
                       "arguments": json!({"consult_id": "k-7", "answer": "Berlin"}).to_string()});
    let doc = json!({
        "header": {"context": {"session_id": SESSION, "turn_id": MEMBER_TURN, "iter": "0"},
                   "hop": {"finish_reason": "tool_calls"}},
        "messages": [
            {"origin": "assistant", "type": "text", "text": "passing that on"},
            {"origin": "assistant", "type": "tool_call", "id": "call-2", "text": reply.to_string()}
        ]
    });
    let dispatched = run_cell(
        DISPATCHER,
        &[("handoff_tools", json!(["consult_cogny"]))],
        doc,
    )
    .0;
    let calls = on_route(&dispatched, "calls");
    let out = assemble(
        &[],
        lane(
            "in_calls",
            json!({"async_calls": calls["header"]["async_calls"],
                   "handoff_calls": calls["header"]["handoff_calls"]}),
            json!({"turn_id": MEMBER_TURN, "iter": "0"}),
            calls["messages"].clone(),
        ),
    );
    let bundle = &out[0];
    assert_eq!(bundle["header"]["phase"], "round-check", "{bundle}");
    let drop = call(bundle, "c-round-check-depart-drop");
    assert_eq!(drop["operation"], "delete", "{drop}");
    assert_eq!(
        drop["where"],
        json!({"session_id": SESSION, "role": "depart", "correlation": {"in": ["k-7"]}}),
        "the older departure under the id, in this session: {drop}"
    );
    let ids = ids_of(bundle);
    assert!(
        pos(&ids, "c-round-check-depart-drop") < pos(&ids, "c-round-check-depart-0"),
        "in front of the new one, which it must not take: {ids:?}"
    );
    let row = call(bundle, "c-round-check-depart-0")["row"].clone();
    assert_eq!(row["correlation"], "k-7", "{row}");
    let body: Value = serde_json::from_str(row["turn"].as_str().expect("turn")).expect("json");
    assert!(
        body.get("answered").is_none(),
        "a fresh departure is not answered yet: {row}"
    );
}

/// The instant `minutes` ago, in the store's fixed-width UTC form.
fn ago(minutes: f64) -> String {
    (chrono::Utc::now() - chrono::Duration::milliseconds((minutes * 60_000.0) as i64))
        .format("%Y-%m-%dT%H:%M:%S%.6fZ")
        .to_string()
}

/// Whether a store `where` selects `row`, in the store's own reading
/// (`store/query/parse.rs`) of the forms the collector writes: bare equality,
/// `neq`, `lt` (a string compare over fixed-width UTC instants) and `in`.
fn selects(filter: &Value, row: &Value) -> bool {
    filter
        .as_object()
        .expect("where object")
        .iter()
        .all(|(col, spec)| {
            let have = &row[col];
            match spec.as_object() {
                None => have == spec,
                Some(op) => {
                    let (k, v) = op.iter().next().expect("one operator");
                    match k.as_str() {
                        "neq" => have != v,
                        "lt" => have.as_str().zip(v.as_str()).is_some_and(|(a, b)| a < b),
                        "in" => v.as_array().expect("in list").contains(have),
                        other => panic!("operator {other} is not read here"),
                    }
                }
            }
        })
}

/// The `where` of every delete one `cstore` emission runs on `table`.
fn deletes_on(msg: &Value, table: &str) -> Vec<Value> {
    calls_of(msg)
        .into_iter()
        .filter(|(_, a)| a["operation"] == "delete" && a["table"] == table)
        .map(|(_, a)| a["where"].clone())
        .collect()
}

/// The turn-open bundle of a plain turn of the person's.
fn turn_open(over: &[(&str, Value)]) -> Value {
    in_phase(&assemble(over, user_turn("hello")), "turn-open").clone()
}

/// A stored `depart` row of this session.
fn stored_depart(correlation: &str, body: Value, at: &str) -> Value {
    json!({"turn_id": MEMBER_TURN, "session_id": SESSION, "iter": 0, "role": "depart",
           "correlation": correlation, "turn": body.to_string(), "fired": 1,
           "recorded_at": at, "deadline_ms": 0})
}

/// No delete of the turn-open touches `row`, and the round-age read does not
/// hand it to the reply as a round row either.
fn survives_the_turn_open(over: &[(&str, Value)], row: &Value) {
    let open = turn_open(over);
    for w in deletes_on(&open, "round") {
        assert!(!selects(&w, row), "the turn-open deletes {row} by {w}");
    }
    let age = call(&open, "c-open-age-round");
    assert!(
        !selects(&age["where"], row),
        "a departure is not a row of a round's age: {age}"
    );
}

#[test]
fn a_departure_lives_by_its_own_clock_not_the_rounds() {
    let before = chrono::Utc::now();
    let open = turn_open(&[]);
    let drop = call(&open, "c-open-stale-depart");
    assert_eq!(drop["operation"], "delete", "{drop}");
    assert_eq!(drop["table"], "round", "{drop}");
    assert_eq!(drop["where"]["role"], "depart", "{drop}");
    assert_eq!(
        drop["where"].as_object().expect("where").len(),
        2,
        "every session -- a quiet session answers nothing any more: {drop}"
    );
    // The seven days of the old prune gate (`prune_after_ms`, collector@4.4.1),
    // as a constant: a departure is bookkeeping of a call that left, and the
    // answer to it comes on the person's clock or the advisor's.
    let cut = drop["where"]["recorded_at"]["lt"].as_str().expect("a cut");
    let cut = chrono::DateTime::parse_from_rfc3339(cut)
        .expect("an RFC 3339 instant")
        .with_timezone(&chrono::Utc);
    let age = before - cut;
    let week = chrono::Duration::days(7);
    assert!(
        age > week - chrono::Duration::seconds(30) && age <= week,
        "seven days: {age:?}"
    );
    // The only delete a plain turn-open runs on `round`.
    assert_eq!(deletes_on(&open, "round").len(), 1, "{open}");
    // The age reads: a round's rows without its departures, the turn rows
    // without the deferred ones, every session -- in front of the open-round
    // check, which a round dead that long must not hold back.
    let round = call(&open, "c-open-age-round");
    assert_eq!(round["operation"], "select", "{round}");
    assert_eq!(
        round["where"],
        json!({"role": {"neq": "depart"}}),
        "{round}"
    );
    let turns = call(&open, "c-open-age-turns");
    assert_eq!(turns["operation"], "select", "{turns}");
    assert_eq!(turns["table"], "turns", "{turns}");
    assert_eq!(
        turns["where"],
        json!({"deferred": 0}),
        "a deferred row waits for its round and leaves with it: {turns}"
    );
    for leg in [&round, &turns] {
        let cols = leg["columns"].as_array().expect("columns");
        assert!(
            cols.contains(&json!("turn_id")) && cols.contains(&json!("recorded_at")),
            "{leg}"
        );
    }
    let ids = ids_of(&open);
    for id in [
        "c-open-stale-depart",
        "c-open-age-turns",
        "c-open-age-round",
    ] {
        assert!(pos(&ids, id) < pos(&ids, "c-open-round"), "{ids:?}");
    }
    assert!(
        pos(&ids, "c-open-stale-depart") < pos(&ids, "c-open-depart"),
        "the departures are read after their own cut: {ids:?}"
    );
}

#[test]
fn an_answered_departure_older_than_the_round_frist_stays_open() {
    // The person answers the advisor's question half an hour later -- an
    // ordinary answer time on a chat channel. The id stays in the slot.
    let row = stored_depart(
        "k-7",
        json!({"id": "call-1", "name": "consult_cogny", "answered": 1}),
        &ago(30.0),
    );
    survives_the_turn_open(&[], &row);
    let seam = curate_over(json!([depart_row(
        "k-7",
        json!({"id": "call-1", "name": "consult_cogny", "answered": 1}),
        row["recorded_at"].as_str().expect("at")
    )]));
    assert_eq!(seam["system"]["consult"]["open"], json!(["k-7"]), "{seam}");
}

#[test]
fn a_late_advice_finds_its_departure_past_the_round_frist() {
    // The advisor thinks for 45 minutes while the person keeps talking: every
    // turn-open in between leaves the departure standing ...
    let body = json!({"id": "call-1", "name": "consult_cogny"});
    let row = stored_depart("k-7", body.clone(), &ago(45.0));
    survives_the_turn_open(&[], &row);
    // ... the advice looks it up by its id alone, whatever its age ...
    let a = assemble(&[], advice("k-7", "which city?"));
    let look = call(in_phase(&a, "advice-look"), "c-look-depart");
    assert!(selects(&look["where"], &row), "{look}");
    // ... and its round carries the member's turn: late, not anonymous (GH #728).
    let (key, out) = open_advice(
        "k-7",
        json!([{"turn_id": MEMBER_TURN, "deadline_ms": now_ms() - 2_400_000,
                "turn": body.to_string()}]),
    );
    assert!(
        key.starts_with(&format!("{MEMBER_TURN}~")),
        "the member's turn is the round's label: {key}"
    );
    call(in_phase(&out, "turn-open"), "c-open-answered");
}

#[test]
fn the_peer_mark_outlives_twenty_quiet_minutes() {
    let mark = stored_depart("", json!({"peer": 1}), &ago(45.0));
    survives_the_turn_open(&[], &mark);
    let seam = curate_over(json!([depart_row(
        "",
        json!({"peer": 1}),
        mark["recorded_at"].as_str().expect("at")
    )]));
    assert!(
        seam["system"]["instructions"]["peer"]["text"]
            .as_str()
            .is_some_and(|t| t.contains("someone else's words")),
        "{seam}"
    );
}

/// `HELD_CONSULTS` as the shipped script states it.
fn held_consults() -> usize {
    let script = config_of(ASSEMBLE)["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string();
    script
        .lines()
        .find_map(|l| l.strip_prefix("HELD_CONSULTS = "))
        .expect("a HELD_CONSULTS constant")
        .trim()
        .parse()
        .expect("a number")
}

#[test]
fn only_the_youngest_answered_ids_are_held() {
    // A departure lives for days now, so the slot shows the newest few ids an
    // advisor answered under -- what the 12-turn window of collector@4.4.1
    // bounded -- and the prompt does not grow with every consult.
    let k = held_consults();
    assert!((1..=8).contains(&k), "a small number: {k}");
    let mut departs: Vec<Value> = (0..k + 2)
        .map(|i| {
            depart_row(
                &format!("k-{i}"),
                json!({"id": format!("call-{i}"), "name": "consult_cogny", "answered": 1}),
                &format!("2026-09-29T09:5{i}:00.000000Z"),
            )
        })
        .collect();
    departs.push(depart_row(
        "k-open",
        json!({"id": "call-x", "name": "consult_cogny"}),
        "2026-09-29T09:59:59.000000Z",
    ));
    let seam = curate_over(Value::Array(departs));
    let want: Vec<Value> = (2..k + 2).map(|i| json!(format!("k-{i}"))).collect();
    assert_eq!(
        seam["system"]["consult"]["open"],
        Value::Array(want),
        "the youngest {k}, oldest first, no unanswered one: {seam}"
    );
}

/// The turn-open reply over what the age reads and the open-round check found.
fn reply_over(
    over: &[(&str, Value)],
    age_round: Value,
    age_turns: Value,
    open: Value,
) -> Vec<Value> {
    let rows = json!([{"id": "2026-09-29T10:00:00.000000Z-bbbb0000", "session_id": SESSION,
                       "turn_id": SESSION_TURN, "role": "user", "content": "berlin",
                       "deferred": 0, "consult_id": ""}]);
    assemble(
        over,
        bundle_reply(
            "turn-open",
            SESSION_TURN,
            &[
                ("c-open-age-turns", age_turns),
                ("c-open-age-round", age_round),
                ("c-open-round", open),
                ("c-open-win", rows),
                ("c-open-depart", json!([])),
                ("c-open-scope", json!([])),
                ("c-open-roster", json!([])),
            ],
        ),
    )
}

fn age_row(tid: &str, role: &str, at: &str) -> Value {
    json!({"turn_id": tid, "session_id": SESSION, "role": role, "iter": 0,
           "fired": 0, "deferred": 0, "recorded_at": at})
}

#[test]
fn a_round_is_as_old_as_its_last_row() {
    // `busy` began 40 minutes ago and wrote its last row a minute ago -- a slow
    // core round, still working; `dead` went quiet 41 minutes ago; `lone` is a
    // turn row with no round behind it (a failed advice lookup).
    let busy = [
        age_row("busy", "leg-window", &ago(40.0)),
        age_row("busy", "assistant", &ago(35.0)),
        age_row("busy", "tool", &ago(1.0)),
    ];
    let dead = [
        age_row("dead", "leg-window", &ago(45.0)),
        age_row("dead", "assistant", &ago(41.0)),
    ];
    let turns = [
        age_row("busy", "user", &ago(40.0)),
        age_row("dead", "user", &ago(45.0)),
        age_row("lone", "advice", &ago(25.0)),
        age_row(SESSION_TURN, "user", &ago(0.0)),
    ];
    let round: Vec<Value> = busy.iter().chain(dead.iter()).cloned().collect();
    let out = reply_over(
        &[],
        Value::Array(round.clone()),
        json!(turns),
        // `dead` is the session's open round: dead that long, it holds no turn back.
        json!([{"turn_id": "dead", "iter": 0, "recorded_at": ago(41.0), "session_id": SESSION}]),
    );
    in_phase(&out, "collect");
    assert!(
        !out.iter().any(|m| m["header"]["phase"] == "defer-w"),
        "a dead round defers nothing: {out:?}"
    );
    let drop = in_phase(&out, "stale-drop");
    let [w_turns, w_round] = [
        call(drop, "c-stale-turns")["where"].clone(),
        call(drop, "c-stale-round")["where"].clone(),
    ];
    assert_eq!(w_turns["deferred"], 0, "{w_turns}");
    assert_eq!(w_round["role"], json!({"neq": "depart"}), "{w_round}");
    for r in &busy {
        assert!(
            !selects(&w_round, r),
            "a working round keeps its early rows: {r}"
        );
    }
    for r in &dead {
        assert!(selects(&w_round, r), "{r}");
    }
    assert!(!selects(&w_turns, &turns[0]), "the busy round's turn stays");
    assert!(selects(&w_turns, &turns[1]), "the dead round's turn goes");
    assert!(
        selects(&w_turns, &turns[2]),
        "a lone row is as old as itself"
    );
    assert!(!selects(&w_turns, &turns[3]), "the turn being opened stays");
}

#[test]
fn the_frist_follows_the_knobs() {
    // Ten idle windows at the shipped knobs (round_idle_ms 120 s), ten late
    // windows when those are longer, and never under ten minutes.
    for (over, minutes) in [
        (vec![], 20.0),
        (vec![("late_after_ms", json!(180_000))], 30.0),
        (
            vec![("round_idle_ms", json!(1)), ("late_after_ms", json!(1))],
            10.0,
        ),
    ] {
        let out = reply_over(
            &over,
            json!([
                age_row("young", "assistant", &ago(minutes - 0.5)),
                age_row("old", "assistant", &ago(minutes + 0.5)),
            ]),
            json!([]),
            json!([]),
        );
        let w = call(in_phase(&out, "stale-drop"), "c-stale-round")["where"].clone();
        assert_eq!(
            w["turn_id"],
            json!({"in": ["old"]}),
            "{minutes} min at {over:?}: {w}"
        );
    }
    // Nothing that old, nothing to drop.
    let out = reply_over(
        &[],
        json!([age_row("young", "assistant", &ago(1.0))]),
        json!([]),
        json!([]),
    );
    assert!(
        !out.iter().any(|m| m["header"]["phase"] == "stale-drop"),
        "{out:?}"
    );
}
