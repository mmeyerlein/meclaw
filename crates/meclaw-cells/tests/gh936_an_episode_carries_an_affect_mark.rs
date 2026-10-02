//! GH #936 -- an episode can carry an affect mark, recall hands it back and can
//! filter by it.
//!
//! The mark is written by a recognizer elsewhere in the colony through the
//! hive's `in_affect` lane and answered on `affect_ack`. Three properties carry
//! the whole contract and are pinned here:
//!
//! 1. **The round decides.** A mark is written only when the asking round may
//!    READ the episode (the same subset rule every read of this hive follows,
//!    `*` universal, an episode without an audience never readable). Unknown
//!    and invisible answer the SAME `not_visible` -- no existence oracle -- and
//!    the acknowledgement never carries the episode text.
//! 2. **Recall hands the mark back.** Every fact candidate carries the mark of
//!    its source episode in the record; the payload carries it when one exists.
//!    The lookup rides in the hydration bundle, never as a store message of its
//!    own (GH #418 ruling R1).
//! 3. **The filter runs after the audience gate.** A candidate the round may not
//!    see is gone before the filter is consulted; a candidate without a mark
//!    falls out once a filter is set.
//!
//! Everything is the REAL shipped `params.script_inline` of `writer` and
//! `recall`, run over stdin with injected store replies; no colony, no model,
//! nothing paid. The hive wiring is read off the shipped `config.json`.

use meclaw_core::serde_json::{self, Value, json};
use meclaw_testing::{emit_all, shipped_script};

const WRITER: &str = "../../templates/memory-hive/writer/config.json";
const RECALL: &str = "../../templates/memory-hive/recall/config.json";
const HIVE: &str = "../../templates/memory-hive/config.json";
const STORE: &str = "../../templates/memory-hive/store/config.json";

const ROUND_EA: &str = r#"["agent:a","member:e"]"#;
const ROUND_EB: &str = r#"["agent:b","member:e"]"#;
const TEXT_EA: &str = "the garden needs covering before the frost";
const TEXT_EB: &str = "the appointment moved to friday";

fn read_json(path: &str) -> Value {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Run a shipped script with a probe program appended -- the script runs over
/// an empty message first (it parks), then the program calls its functions.
fn probe(config: &str, program: &str) -> String {
    let script = shipped_script(config);
    let src = format!(
        concat!(
            "import sys, io, json\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO('{{\"envelope\": {{}}, \"body\": {{}}, \"params\": {{}}}}')\n",
            "_sink, _real = io.StringIO(), sys.stdout\n",
            "sys.stdout = _sink\n",
            "try:\n",
            "    exec(compile(_script, 'cell', 'exec'), globals())\n",
            "except SystemExit:\n",
            "    pass\n",
            "finally:\n",
            "    sys.stdout = _real\n",
            "{}\n"
        ),
        serde_json::to_string(&script).expect("script"),
        program
    );
    let mut child = std::process::Command::new("python3")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("python3");
    {
        use std::io::Write;
        let mut sink = child.stdin.take().expect("stdin");
        sink.write_all(src.as_bytes()).expect("write");
    }
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn args_of(msg: &Value) -> Value {
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    serde_json::from_str(text).expect("store args are json")
}

// ════════════════════════════════════════════════ 1. the pure functions

#[test]
fn the_mark_is_parsed_whole_or_refused_with_its_key() {
    let program = r#"
cases = [
  {"episode_id": "ep-1", "valence": -0.5, "arousal": 0.2, "confidence": 0.9, "source": "tone-1"},
  {"session_id": "s1", "turn_id": "t1", "valence": 1, "arousal": 0, "confidence": 1, "source": "x"},
  {"episode_id": "ep-1", "valence": 2, "arousal": 0.2, "confidence": 0.9, "source": "x"},
  {"episode_id": "ep-1", "valence": 0, "arousal": -0.1, "confidence": 0.9, "source": "x"},
  {"episode_id": "ep-1", "valence": 0, "arousal": 0.1, "confidence": 1.5, "source": "x"},
  {"episode_id": "ep-1", "valence": 0, "arousal": 0.1, "confidence": 0.5, "source": "Bad Source"},
  {"episode_id": "ep-1", "valence": 0, "arousal": 0.1, "confidence": 0.5, "source": "a" * 65},
  {"episode_id": "ep-1", "valence": True, "arousal": 0.1, "confidence": 0.5, "source": "x"},
  {"valence": 0, "arousal": 0.1, "confidence": 0.5, "source": "x"},
  {"episode_id": "ep-1", "session_id": "s1", "turn_id": "t1", "valence": 0, "arousal": 0.1,
   "confidence": 0.5, "source": "x"},
  {"session_id": "s1", "valence": 0, "arousal": 0.1, "confidence": 0.5, "source": "x"},
  {"episode_id": "ep-1", "valence": 0, "arousal": 0.1, "confidence": 0.5, "source": "x",
   "content": "smuggled"},
  "not an object",
]
out = []
for c in cases:
    req, key = parse_affect(c)
    out.append(key if key else "ok")
print(json.dumps(out))
"#;
    assert_eq!(
        probe(WRITER, program),
        r#"["ok", "ok", "valence", "arousal", "confidence", "source", "source", "valence", "episode_id", "episode_id", "turn_id", "content", "affect"]"#
    );
}

#[test]
fn the_round_may_read_what_it_is_a_subset_of() {
    let program = r#"
cases = [
  (["agent:a", "member:e"], '["agent:a","member:e"]'),
  (["member:e"], '["agent:a","member:e"]'),
  (["agent:b", "member:e"], '["agent:a","member:e"]'),
  (["agent:b"], '["*"]'),
  (["agent:a"], None),
  (["agent:a"], ""),
  (["agent:a"], "[]"),
  (["agent:a"], "not json"),
]
print(json.dumps([visible(r, a) for r, a in cases]))
# The round is read the way recall reads it: a blank name is KEPT (a subset of
# no stored set), an empty list is no round at all.
blank = read_round({"audience_now": '["member:e",""]'})
print(json.dumps([sorted(blank), visible(blank, '["agent:a","member:e"]'),
                  read_round({"audience_now": "[]"}), read_round({})]))
"#;
    assert_eq!(
        probe(WRITER, program),
        "[true, true, false, true, false, false, false, false]\n\
         [[\"\", \"member:e\"], false, null, null]"
    );
}

/// The source of one top-level `def` of a script, without its `def` line.
fn body_of(script: &str, name: &str) -> String {
    let head = format!("\ndef {name}(");
    let at = script
        .find(&head)
        .unwrap_or_else(|| panic!("no def {name}"));
    let rest = &script[at + 1..];
    let mut lines = rest.lines();
    lines.next();
    let body: Vec<&str> = lines
        .take_while(|l| l.is_empty() || l.starts_with(' '))
        .collect();
    body.join("\n").trim_end().to_string()
}

#[test]
fn the_writer_reads_the_round_exactly_like_recall() {
    // Review I-3: a variant of the rule is a second rule. The writer's copies
    // are recall's bodies, so a round may mark exactly what it may recall.
    let writer = shipped_script(WRITER);
    let recall = shipped_script(RECALL);
    assert_eq!(
        body_of(&writer, "stored_audience"),
        body_of(&recall, "audience_of")
    );
    assert_eq!(
        body_of(&writer, "read_round"),
        body_of(&recall, "read_audience_now").replace("audience_of(", "stored_audience(")
    );
}

#[test]
fn the_recall_filter_is_parsed_and_applied_to_marks() {
    let program = r#"
f_ok, k1 = parse_affect_filter('{"valence_max": -0.3, "min_confidence": 0.5}')
_, k2 = parse_affect_filter('{"valence_max": -2}')
_, k3 = parse_affect_filter('{"loudness": 1}')
_, k4 = parse_affect_filter('[1]')
_, k5 = parse_affect_filter('nope')
f_min, _ = parse_affect_filter('{"valence_min": 0, "arousal_min": 0.5}')
calm = {"valence": 0.5, "arousal": 0.2, "confidence": 0.8, "source": "x", "at": "t"}
lively = {"valence": 0.5, "arousal": 0.7, "confidence": 0.8, "source": "x", "at": "t"}
none, k6 = parse_affect_filter('')
low = {"valence": -0.6, "arousal": 0.4, "confidence": 0.8, "source": "x", "at": "t"}
high = {"valence": 0.5, "arousal": 0.4, "confidence": 0.8, "source": "x", "at": "t"}
unsure = {"valence": -0.6, "arousal": 0.4, "confidence": 0.2, "source": "x", "at": "t"}
print(json.dumps([k1, k2, k3, k4, k5, none, k6,
                  affect_pass(f_ok, low), affect_pass(f_ok, high),
                  affect_pass(f_ok, unsure), affect_pass(f_ok, None),
                  affect_pass(None, None),
                  parse_mark(json.dumps(low)) == low, parse_mark(None), parse_mark("{}"),
                  affect_pass(f_min, lively), affect_pass(f_min, calm),
                  affect_pass(f_min, low)]))
"#;
    assert_eq!(
        probe(RECALL, program),
        r#"[null, "valence_max", "loudness", "affect_filter", "affect_filter", null, null, true, false, false, false, true, true, null, null, true, false, false]"#
    );
}

// ════════════════════════════════════════════════ 2. the writer round trip

/// The emulated `episodes` table: one episode the round {e,a} took part in, one
/// of the round {e,b}, and the agent's own answer in the first turn.
fn episodes() -> Vec<Value> {
    vec![
        json!({"id": "ep-ea", "session_id": "s1", "turn_id": "t1", "sender": "user",
               "audience_set": ROUND_EA, "content": TEXT_EA,
               "recorded_at": "2026-10-01T10:00:00Z", "affect": null}),
        json!({"id": "ep-agent", "session_id": "s1", "turn_id": "t1", "sender": "assistant",
               "audience_set": ROUND_EA, "content": "noted",
               "recorded_at": "2026-10-01T10:00:01Z", "affect": null}),
        json!({"id": "ep-eb", "session_id": "s2", "turn_id": "t9", "sender": "user",
               "audience_set": ROUND_EB, "content": TEXT_EB,
               "recorded_at": "2026-10-01T11:00:00Z", "affect": null}),
    ]
}

fn matches(row: &Value, clause: &Value) -> bool {
    clause
        .as_object()
        .expect("where")
        .iter()
        .all(|(col, want)| {
            let have = row.get(col).cloned().unwrap_or(Value::Null);
            match want {
                Value::Object(o) if o.contains_key("neq") => o["neq"] != have,
                Value::Object(o) => panic!("the emulated store does not know {o:?}"),
                other => *other == have,
            }
        })
}

/// The answer to one store op of the writer, as the store edge hands it back
/// (the context the edge promoted from the hop rides along).
fn store_answer(op: &Value, table: &mut [Value], round: Option<&str>) -> Value {
    let args = args_of(op);
    let mut ctx = json!({"store_origin": "affect", "mem_phase": op["header"]["phase"],
                         "affect_req": op["header"]["affect_req"]});
    if let Some(r) = round {
        ctx["audience_now"] = json!(r);
    }
    let (rows, affected) = match args["operation"].as_str() {
        Some("select") => {
            let mut hits: Vec<Value> = table
                .iter()
                .filter(|r| matches(r, &args["where"]))
                .map(|r| {
                    let mut out = serde_json::Map::new();
                    for c in args["columns"].as_array().expect("columns") {
                        let c = c.as_str().expect("column");
                        out.insert(c.to_string(), r[c].clone());
                    }
                    Value::Object(out)
                })
                .collect();
            if let Some(n) = args["limit"].as_u64() {
                hits.truncate(usize::try_from(n).unwrap_or(usize::MAX));
            }
            let n = hits.len();
            (Value::Array(hits), n)
        }
        Some("update") => {
            let mut n = 0;
            for r in table.iter_mut().filter(|r| matches(r, &args["where"])) {
                for (k, v) in args["set"].as_object().expect("set") {
                    r[k.as_str()] = v.clone();
                }
                n += 1;
            }
            (json!([]), n)
        }
        other => panic!("the writer asked the store for {other:?}"),
    };
    json!({
        "header": {"context": ctx,
                   "hop": {"operation": args["operation"], "rows_affected": affected}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "s",
                      "text": rows.to_string()}]
    })
}

/// One `in_affect` request as the hive's door hands it to the writer, run
/// through every store round trip it asks for. Returns the final emissions and
/// the table afterwards.
fn mark(affect: Value, round: Option<&str>, tag: Option<&str>) -> (Vec<Value>, Vec<Value>) {
    mark_on(episodes(), affect, round, tag)
}

/// [`mark`] over a table that already holds earlier writes.
fn mark_on(
    mut table: Vec<Value>,
    affect: Value,
    round: Option<&str>,
    tag: Option<&str>,
) -> (Vec<Value>, Vec<Value>) {
    let mut ctx = json!({"store_origin": "affect", "mem_phase": "affect"});
    if let Some(r) = round {
        ctx["audience_now"] = json!(r);
    }
    let mut hop = json!({"route": "in_affect"});
    if let Some(t) = tag {
        hop["affect_tag"] = json!(t);
    }
    let script = shipped_script(WRITER);
    let mut out = emit_all(
        &script,
        &json!({"header": {"context": ctx, "hop": hop},
                "messages": [], "affect": affect}),
    );
    for _ in 0..4 {
        let Some(op) = out.iter().find(|m| m["header"]["route"] == "astore") else {
            return (out, table);
        };
        let answer = store_answer(op, &mut table, round);
        out = emit_all(&script, &answer);
    }
    panic!("the writer never stopped asking the store: {out:?}");
}

fn ack(out: &[Value]) -> &Value {
    assert_eq!(out.len(), 1, "one answer and nothing else: {out:?}");
    assert_eq!(out[0]["header"]["route"], "affect_ack", "{out:?}");
    &out[0]
}

fn affect(addr: Value, valence: f64) -> Value {
    let mut a = addr;
    a["valence"] = json!(valence);
    a["arousal"] = json!(0.4);
    a["confidence"] = json!(0.8);
    a["source"] = json!("tone-reader");
    a
}

#[test]
fn a_round_that_may_read_the_episode_marks_it() {
    let (out, table) = mark(
        affect(json!({"episode_id": "ep-ea"}), -0.6),
        Some(ROUND_EA),
        Some("req-7"),
    );
    let a = ack(&out);
    assert_eq!(a["header"]["error_code"], "", "{a}");
    assert_eq!(a["header"]["episode_id"], "ep-ea", "{a}");
    assert_eq!(a["header"]["affect_tag"], "req-7", "the tag is echoed: {a}");
    assert_eq!(a["affect_ack"], json!({"episode_id": "ep-ea"}), "{a}");
    assert!(
        !a.to_string().contains(TEXT_EA),
        "the acknowledgement never carries the episode text: {a}"
    );
    let stored: Value =
        serde_json::from_str(table[0]["affect"].as_str().expect("the column was written"))
            .expect("the mark is JSON");
    assert_eq!(stored["valence"], json!(-0.6));
    assert_eq!(stored["arousal"], json!(0.4));
    assert_eq!(stored["confidence"], json!(0.8));
    assert_eq!(stored["source"], "tone-reader");
    assert!(stored["at"].is_string(), "the mark says when: {stored}");
    assert!(table[1]["affect"].is_null() && table[2]["affect"].is_null());
}

#[test]
fn the_last_write_wins() {
    let (_, table) = mark(
        affect(json!({"episode_id": "ep-ea"}), -0.6),
        Some(ROUND_EA),
        None,
    );
    let mut second = affect(json!({"episode_id": "ep-ea"}), 0.7);
    second["source"] = json!("second-reader");
    let (out, table) = mark_on(table, second, Some(ROUND_EA), None);
    assert_eq!(ack(&out)["header"]["error_code"], "", "{out:?}");
    let stored: Value =
        serde_json::from_str(table[0]["affect"].as_str().expect("written")).expect("json");
    assert_eq!(
        stored["valence"],
        json!(0.7),
        "no history, the column holds the current reading"
    );
    assert_eq!(stored["source"], "second-reader");
}

#[test]
fn the_turn_address_marks_the_incoming_turn_never_the_agents_answer() {
    let (out, table) = mark(
        affect(json!({"session_id": "s1", "turn_id": "t1"}), 0.3),
        Some(ROUND_EA),
        None,
    );
    let a = ack(&out);
    assert_eq!(a["header"]["error_code"], "", "{a}");
    assert_eq!(a["header"]["episode_id"], "ep-ea", "{a}");
    assert!(
        table[0]["affect"].is_string(),
        "the member's turn: {table:?}"
    );
    assert!(
        table[1]["affect"].is_null(),
        "not the agent's answer: {table:?}"
    );
}

#[test]
fn a_round_that_may_not_read_it_learns_nothing() {
    let (invisible, table) = mark(
        affect(json!({"episode_id": "ep-ea"}), -0.6),
        Some(ROUND_EB),
        None,
    );
    let a = ack(&invisible);
    assert_eq!(a["header"]["error_code"], "not_visible", "{a}");
    assert!(table.iter().all(|r| r["affect"].is_null()), "{table:?}");
    assert!(!a.to_string().contains(TEXT_EA), "{a}");

    // Unknown answers EXACTLY like invisible: same code, same keys, and the
    // only id that comes back is the one the caller sent.
    let (unknown, _) = mark(
        affect(json!({"episode_id": "ep-nowhere"}), -0.6),
        Some(ROUND_EB),
        None,
    );
    let u = ack(&unknown);
    assert_eq!(u["header"]["error_code"], "not_visible", "{u}");
    assert_eq!(u["header"]["episode_id"], "ep-nowhere");
    assert_eq!(a["header"]["episode_id"], "ep-ea");
    let keys = |v: &Value| {
        let mut k: Vec<String> = v["header"]
            .as_object()
            .expect("header")
            .keys()
            .cloned()
            .collect();
        k.sort();
        k
    };
    assert_eq!(keys(a), keys(u), "no oracle in the shape: {a} / {u}");

    // A round with a blank name is read as recall reads it (review I-3): the
    // blank is kept, and no stored set holds it.
    let (blank, table) = mark(
        affect(json!({"episode_id": "ep-ea"}), -0.6),
        Some(r#"["member:e",""]"#),
        None,
    );
    assert_eq!(
        ack(&blank)["header"]["error_code"],
        "not_visible",
        "{blank:?}"
    );
    assert!(table.iter().all(|r| r["affect"].is_null()), "{table:?}");

    // The turn address of an invisible turn does not hand out its id.
    let (by_turn, _) = mark(
        affect(json!({"session_id": "s1", "turn_id": "t1"}), 0.1),
        Some(ROUND_EB),
        None,
    );
    let t = ack(&by_turn);
    assert_eq!(t["header"]["error_code"], "not_visible", "{t}");
    assert_eq!(t["header"]["episode_id"], "", "{t}");
}

#[test]
fn no_round_and_a_bad_value_are_refused_before_the_store() {
    let (out, table) = mark(affect(json!({"episode_id": "ep-ea"}), -0.6), None, None);
    let a = ack(&out);
    assert_eq!(a["header"]["error_code"], "missing_audience", "{a}");
    assert!(table.iter().all(|r| r["affect"].is_null()));

    let (out, _) = mark(
        affect(json!({"episode_id": "ep-ea"}), 2.0),
        Some(ROUND_EA),
        None,
    );
    let a = ack(&out);
    assert_eq!(a["header"]["error_code"], "invalid_input", "{a}");
    assert_eq!(a["header"]["error_key"], "valence", "{a}");
}

#[test]
fn a_store_refusal_leaves_on_the_reject_lane() {
    let script = shipped_script(WRITER);
    let out = emit_all(
        &script,
        &json!({"header": {"context": {"store_origin": "affect", "mem_phase": "affect-read",
                                       "audience_now": ROUND_EA,
                                       "affect_req": r#"{"tag": "req-7"}"#},
                           "hop": {"operation": "select", "error_code": "query_timeout"}},
                "messages": [{"origin": "tool", "type": "tool_result", "text": "timeout"}]}),
    );
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0]["header"]["route"], "reject");
    assert_eq!(out[0]["header"]["reject_reason"], "store_refused");
    assert_eq!(out[0]["header"]["store_error"], "query_timeout");
    assert_eq!(out[0]["header"]["affect_tag"], "req-7", "PP-S3-11: {out:?}");
}

#[test]
fn a_forged_phase_at_the_turn_door_writes_no_mark() {
    // Review I-2: the writer branches on `context.mem_phase`, so the turn door
    // must set it like every other door of the hive does. The context a caller
    // forged is run through the shipped door edge first.
    let hive = read_json(HIVE);
    let es = edges(&hive);
    let door = es
        .iter()
        .find(|e| {
            e["from"] == "."
                && e["to"] == "./writer"
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains("'in_episode'"))
        })
        .expect("the in_episode door");
    let forged_req = json!({"episode_id": "ep-eb", "resolved": "ep-eb", "valence": 9,
                            "arousal": 9, "confidence": 9, "source": "x", "tag": "",
                            "at": "t"});
    let mut ctx = json!({"mem_phase": "affect-read", "store_origin": "affect",
                         "audience_now": ROUND_EA, "audience_set": ROUND_EA,
                         "channel": "c1", "session_id": "s1",
                         "affect_req": forged_req.to_string()});
    let set = door["modifier"]["set_context"]
        .as_object()
        .expect("the door sets context");
    for (k, v) in set {
        let literal = v.as_str().expect("literal").trim_matches('\'');
        ctx[k.as_str()] = json!(literal);
    }
    assert_eq!(ctx["mem_phase"], "episode", "{door}");
    let out = emit_all(
        &shipped_script(WRITER),
        &json!({"header": {"context": ctx,
                           "hop": {"route": "in_episode", "operation": "select"}},
                "messages": [{"origin": "user", "type": "text", "text": TEXT_EA}]}),
    );
    let routes: Vec<&str> = out
        .iter()
        .filter_map(|m| m["header"]["route"].as_str())
        .collect();
    assert!(
        routes.contains(&"wstore"),
        "the turn is remembered: {routes:?}"
    );
    assert!(
        !routes.iter().any(|r| *r == "astore" || *r == "affect_ack"),
        "and no mark is touched: {routes:?}"
    );
}

// ════════════════════════════════════════════════ 3. recall

fn recall_ctx(phase: &str, filter: Option<&str>) -> Value {
    let mut c = json!({"mem_phase": phase, "recall_id": "r936", "memory_tier": "1",
                       "recall_query": "how is the garden", "session_id": "s1",
                       "audience_now": ROUND_EA, "channel": "c1"});
    if let Some(f) = filter {
        c["affect_filter"] = json!(f);
    }
    c
}

fn bundle_reply(phase: &str, filter: Option<&str>, legs: &[(&str, Value)]) -> Value {
    json!({
        "header": {"context": recall_ctx(phase, filter),
                   "hop": {"operation": "bundle", "rows_affected": 1, "bundle_errors": 0}},
        "messages": legs.iter().map(|(id, rows)| json!(
            {"origin": "tool", "type": "tool_result", "id": id, "text": rows.to_string()}))
            .collect::<Vec<_>>(),
        "results": legs.iter().map(|(id, _)| json!(
            {"tool_call_id": id, "operation": "select", "rows_affected": 1, "duration_ms": 0}))
            .collect::<Vec<_>>()
    })
}

fn scratch(leg: &str, payload: &Value) -> Value {
    json!({"request_id": "r936", "leg": leg, "payload": payload.to_string(), "fired": 0})
}

fn fact(id: &str, ep: &str, audience: &str, claim: &str) -> Value {
    json!({"id": id, "episode_id": ep, "subject": "user", "canonical_subject": "user",
           "predicate": "plans", "canonical_predicate": "plans", "claim": claim,
           "fact_kind": "world", "valid_from": "2026-10-01T10:00:00Z", "valid_until": null,
           "expired_at": null, "superseded_by": null, "confidence": 80,
           "channel": "c1", "audience_set": audience, "source": "", "session_id": "s1"})
}

/// A fact released to everyone whose source episode belongs to the round
/// {e,b} only.
fn wide_fact() -> Value {
    let mut f = fact("f-wide", "ep-hidden", r#"["*"]"#, "shares the seed list");
    f["predicate"] = json!("shares");
    f["canonical_predicate"] = json!("shares");
    f
}

fn mark_json(valence: f64) -> String {
    json!({"valence": valence, "arousal": 0.4, "confidence": 0.8,
           "source": "tone-reader", "at": "2026-10-01T10:00:02Z"})
    .to_string()
}

/// The hydration reply of a tier-1 request whose fusion nominated five facts:
/// a marked low one, a marked high one and an unmarked one of the round's own
/// episode set, a `*` fact whose SOURCE episode the round may not read (its
/// mark must not reach the round, review I-1), and one of an episode the round
/// {e,a} may not see. One belief rides along.
fn emit_doc(filter: Option<&str>) -> Value {
    let fused = json!({
        "candidates": [
            {"kind": "fact", "id": "f-low", "score": 0.4, "legs": ["keyword"], "agreement": 1},
            {"kind": "fact", "id": "f-high", "score": 0.3, "legs": ["keyword"], "agreement": 1},
            {"kind": "fact", "id": "f-plain", "score": 0.2, "legs": ["keyword"], "agreement": 1},
            {"kind": "fact", "id": "f-wide", "score": 0.15, "legs": ["keyword"], "agreement": 1},
            {"kind": "fact", "id": "f-eb", "score": 0.1, "legs": ["keyword"], "agreement": 1}
        ],
        "legs_present": ["keyword"], "leg_sizes": {"keyword": 5}, "leg_sizes_raw": {},
        "leg_capped": {}, "semantic_degraded": true
    });
    bundle_reply(
        "t1-emit",
        filter,
        &[
            (
                "r-hyd-fact",
                json!([
                    fact("f-low", "ep-low", ROUND_EA, "covers the beds with leaves"),
                    fact(
                        "f-high",
                        "ep-high",
                        ROUND_EA,
                        "prunes the roses on saturday"
                    ),
                    fact("f-plain", "ep-plain", ROUND_EA, "waters the herbs"),
                    wide_fact(),
                    fact("f-eb", "ep-eb", ROUND_EB, "moved the appointment"),
                ]),
            ),
            (
                "r-hyd-affect",
                json!([
                    {"id": "ep-low", "affect": mark_json(-0.6),
                     "channel": "c1", "audience_set": ROUND_EA},
                    {"id": "ep-high", "affect": mark_json(0.5),
                     "channel": "c1", "audience_set": ROUND_EA},
                    {"id": "ep-plain", "affect": null,
                     "channel": "c1", "audience_set": ROUND_EA},
                    {"id": "ep-hidden", "affect": mark_json(-0.9),
                     "channel": "c2", "audience_set": ROUND_EB},
                    {"id": "ep-eb", "affect": mark_json(-0.9),
                     "channel": "c1", "audience_set": ROUND_EB}
                ]),
            ),
            (
                "r-hyd-read",
                json!([
                    scratch("fused", &fused),
                    scratch(
                        "beliefs",
                        &json!([{"id": "b-1", "statement": "the member likes the garden",
                                 "confidence": 0.7}])
                    )
                ]),
            ),
        ],
    )
}

fn record_ids(out: &[Value]) -> Vec<String> {
    let msg = out
        .iter()
        .find(|m| m.get("recall_diagnostic").is_some())
        .unwrap_or_else(|| panic!("no bundle in {out:#?}"));
    msg["recall_diagnostic"]["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .map(|c| c["id"].as_str().expect("id").to_string())
        .collect()
}

#[test]
fn recall_hands_back_the_mark_of_the_source_episode() {
    let out = emit_all(&shipped_script(RECALL), &emit_doc(None));
    let msg = out
        .iter()
        .find(|m| m.get("recall_diagnostic").is_some())
        .unwrap_or_else(|| panic!("no bundle in {out:#?}"));
    let record = msg["recall_diagnostic"]["candidates"]
        .as_array()
        .expect("candidates");
    let by_id = |id: &str| {
        record
            .iter()
            .find(|c| c["id"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("{id} missing: {record:?}"))
    };
    assert_eq!(by_id("f-low")["affect"]["valence"], json!(-0.6));
    assert_eq!(by_id("f-high")["affect"]["valence"], json!(0.5));
    assert!(
        by_id("f-plain")["affect"].is_null(),
        "unknown is null, never a guess"
    );
    assert!(
        by_id("f-wide")["affect"].is_null(),
        "the fact is visible, its source turn is not: no mark (review I-1)"
    );
    assert!(
        record.iter().all(|c| c["id"] != "f-eb"),
        "the audience gate still decides first: {record:?}"
    );
    // The payload the model reads carries the mark where one exists.
    let bundle: Value = serde_json::from_str(
        msg["system"]["memory"]["bundle"]["text"]
            .as_str()
            .expect("bundle text"),
    )
    .expect("bundle json");
    let marked: Vec<&Value> = bundle["candidates"]
        .as_array()
        .expect("payload candidates")
        .iter()
        .filter(|c| c.get("affect").is_some())
        .collect();
    assert_eq!(marked.len(), 2, "{bundle}");
    assert!(
        marked.iter().all(|c| c["affect"]["valence"] != json!(-0.9)),
        "no mark of an unreadable turn reaches the payload: {bundle}"
    );
    assert_eq!(
        bundle["beliefs"].as_array().map(Vec::len),
        Some(1),
        "without a filter the belief stays: {bundle}"
    );
    assert!(!bundle.to_string().contains("moved the appointment"));
}

#[test]
fn the_filter_keeps_only_marked_candidates_inside_its_bounds() {
    let out = emit_all(
        &shipped_script(RECALL),
        &emit_doc(Some(r#"{"valence_max": -0.3}"#)),
    );
    assert_eq!(
        record_ids(&out),
        ["f-low"],
        "the high mark, the unmarked fact, the fact of an unreadable turn and \
         the invisible episode are out"
    );
    // A belief carries no mark, so a set filter keeps none (review I-4).
    let msg = out
        .iter()
        .find(|m| m.get("recall_diagnostic").is_some())
        .expect("bundle");
    let bundle: Value = serde_json::from_str(
        msg["system"]["memory"]["bundle"]["text"]
            .as_str()
            .expect("bundle text"),
    )
    .expect("bundle json");
    assert_eq!(bundle["beliefs"], json!([]), "{bundle}");
}

#[test]
fn a_malformed_filter_is_refused_at_the_door() {
    let out = emit_all(
        &shipped_script(RECALL),
        &json!({"header": {"context": recall_ctx("", Some(r#"{"valence_max": 3}"#)),
                           "hop": {"phase": "recall"}},
                "messages": []}),
    );
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0]["header"]["route"], "reject");
    assert_eq!(out[0]["header"]["reject_reason"], "invalid_input");
    assert!(
        out[0]["messages"][0]["text"]
            .as_str()
            .is_some_and(|t| t.contains("valence_max")),
        "the refusal names the key: {out:?}"
    );
}

#[test]
fn the_mark_lookup_rides_in_the_hydration_bundle() {
    let legs = json!({
        "kw-ep": [], "kw-fact": [{"kind": "fact", "id": "f1"}],
        "temporal": [], "beliefs": [], "anchors": [],
        "axis": {"f1": ["user", "plans"]},
        "src": {"f1": "ep-1"},
        "model": {"model_id": "m", "dim": 1024}
    });
    let out = emit_all(
        &shipped_script(RECALL),
        &bundle_reply(
            "t1-legs",
            None,
            &[
                ("r-legs-graph", json!({"paths": []})),
                ("r-legs-sem-aud", json!([])),
                (
                    "r-legs-read",
                    json!([scratch("legs", &legs), scratch("sem", &json!([]))]),
                ),
            ],
        ),
    );
    let store: Vec<&Value> = out
        .iter()
        .filter(|m| m["header"]["route"] == "rstore")
        .collect();
    assert_eq!(store.len(), 1, "still ONE store message (R1): {out:?}");
    let call = store[0]["messages"]
        .as_array()
        .expect("calls")
        .iter()
        .find(|t| t["id"] == "r-hyd-affect")
        .unwrap_or_else(|| panic!("no affect lookup: {out:?}"));
    let args: Value = serde_json::from_str(call["text"].as_str().expect("text")).expect("args");
    assert_eq!(args["table"], "episodes");
    assert_eq!(args["where"]["id"]["in"], json!(["ep-1"]));
    let cols = args["columns"].as_array().expect("columns");
    assert!(
        !cols.iter().any(|c| c == "content"),
        "the lookup reads no text: {args}"
    );
    for gate in ["channel", "audience_set"] {
        assert!(
            cols.iter().any(|c| c == gate),
            "the source episode is gated in the same call (review I-1): {args}"
        );
    }
}

#[test]
fn a_fact_only_the_semantic_leg_found_is_projected_on_its_axis() {
    // The semantic companion is an id -> row map in the fusion; its axis keys
    // are read off the ROWS. Before, the keys of the map were walked, the axis
    // page never asked for this fact's axis, and the hit was shown without its
    // successor and without `supersession_unknown`.
    let legs = json!({
        "kw-ep": [], "kw-fact": [], "temporal": [], "self": [], "beliefs": [],
        "anchors": [], "axis": {}, "model": {"model_id": "m", "dim": 1024}
    });
    let out = emit_all(
        &shipped_script(RECALL),
        &bundle_reply(
            "t1-legs",
            None,
            &[
                ("r-legs-graph", json!({"paths": []})),
                (
                    "r-legs-sem-aud",
                    json!([{"id": "f-sem", "episode_id": "ep-1", "channel": "c1",
                            "audience_set": ROUND_EA, "subject": "user",
                            "canonical_subject": "user", "predicate": "lives_in",
                            "canonical_predicate": "lives_in"}]),
                ),
                (
                    "r-legs-read",
                    json!([
                        scratch("legs", &legs),
                        scratch(
                            "sem",
                            &json!([{"kind": "fact", "id": "f-sem", "distance": 10}])
                        )
                    ]),
                ),
            ],
        ),
    );
    let store = out
        .iter()
        .find(|m| m["header"]["route"] == "rstore")
        .unwrap_or_else(|| panic!("no hydration: {out:?}"));
    let axis = store["messages"]
        .as_array()
        .expect("calls")
        .iter()
        .find(|t| t["id"] == "r-hyd-axis")
        .unwrap_or_else(|| panic!("no axis page for the semantic hit: {store}"));
    let args: Value = serde_json::from_str(axis["text"].as_str().expect("text")).expect("args");
    assert_eq!(args["where"]["canonical_subject"]["in"], json!(["user"]));
    assert_eq!(
        args["where"]["canonical_predicate"]["in"],
        json!(["lives_in"])
    );
}

// ════════════════════════════════════════════════ 4. the hive wiring

fn edges(hive: &Value) -> Vec<Value> {
    hive["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .clone()
}

#[test]
fn the_hive_declares_the_lane_and_wires_its_round_trip() {
    let hive = read_json(HIVE);
    let p = &hive["params"];
    let accepts = p["contract"]["accepts"].as_array().expect("accepts");
    let lane = accepts
        .iter()
        .find(|a| a["route"] == "in_affect")
        .unwrap_or_else(|| panic!("no in_affect lane: {accepts:?}"));
    assert_eq!(
        lane["context"],
        json!(["audience_now"]),
        "the round is the reader's round"
    );
    let emits = p["contract"]["emits"].as_array().expect("emits");
    assert!(emits.iter().any(|e| e["route"] == "affect_ack"));
    let drains = p["required_drains"].as_array().expect("drains");
    assert!(
        drains
            .iter()
            .any(|d| d["accepts"] == "in_affect" && d["emits"] == "affect_ack"),
        "a sent mark needs its answer drained: {drains:?}"
    );

    let es = edges(&hive);
    let find = |from: &str, to: &str, needle: &str| {
        es.iter()
            .find(|e| {
                e["from"] == from
                    && e["to"] == to
                    && e["condition"].as_str().is_some_and(|c| c.contains(needle))
            })
            .cloned()
            .unwrap_or_else(|| panic!("no edge {from} -> {to} on {needle}"))
    };
    let door = find(".", "./writer", "'in_affect'");
    assert_eq!(door["modifier"]["set_context"]["store_origin"], "'affect'");
    let out = find("./writer", "./store", "'astore'");
    assert_eq!(
        out["modifier"]["set_context"]["affect_req"],
        "hop.affect_req"
    );
    find("./store", "./writer", "'affect'");
    let exit = find("./writer", ".", "'affect_ack'");
    let cleared: Vec<&str> = exit["modifier"]["delete_context"]
        .as_array()
        .expect("delete_context")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for key in ["affect_req", "affect_filter", "mem_phase", "store_origin"] {
        assert!(cleared.contains(&key), "{key} leaves the hive: {cleared:?}");
    }
    let recall_door = find(".", "./recall", "'in_query'");
    assert!(
        recall_door["modifier"]["set_context"]["affect_filter"]
            .as_str()
            .is_some_and(|c| c.contains("has(hop.affect_filter)")),
        "the filter is promoted for the whole chain: {recall_door}"
    );
}

#[test]
fn the_episodes_table_has_the_column() {
    let store = read_json(STORE);
    assert_eq!(store["params"]["schema"]["episodes"]["affect"], "text");
}
