//! GH #849 -- turns of peer and group channels are remembered in full, with the
//! channel's audience as a set and the speaker as the source.
//!
//! Measured before the fix: the memory hive's `writer` kept only `user` and
//! `assistant` turns, so a `peer` turn -- the words of another participant of a
//! channel -- fell away without a trace; `extract-glue` bound a sidecar block
//! only to the newest `user` episode; `memory-drain` filtered to the same two
//! origins; and no column recorded WHO stated a fact. A claim one participant
//! made about another could therefore only be forgotten or be written as if the
//! member had said it about themselves.
//!
//! The rule (owner ruling R-SN-4): what a peer says is that peer's statement.
//! It is kept -- episode AND facts -- with the audience of the channel as the
//! set (usable only while the current participants are a subset of it) and the
//! participant reference of the speaker as `source`; recall shows it as
//! "<ref> says: ...", and it never closes a statement of another source.
//!
//! Every script below is the REAL shipped `params.script_inline`, run over stdin
//! against injected store replies; no colony and no model.

use meclaw_core::serde_json::{self, Value, json};
use meclaw_testing::{emit_all, shipped_script};

const WRITER: &str = "../../templates/memory-hive/writer/config.json";
const EXTRACT: &str = "../../templates/memory-hive/extract-glue/config.json";
const RECALL: &str = "../../templates/memory-hive/recall/config.json";
const DREAM: &str = "../../templates/memory-hive/dream-glue/config.json";
const STORE: &str = "../../templates/memory-hive/store/config.json";
const DRAIN: &str = "../../templates/memory-drain/drain/config.json";

/// The participant reference of the peer (affinity's `sha256(identity)[:8]`).
const REF: &str = "3a47fe3e";
const AUDIENCE: &str = "[\"agent:helper\", \"member:alex\", \"peer:sam\"]";

fn args_of(msg: &Value) -> Value {
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    serde_json::from_str(text).unwrap_or(Value::Null)
}

fn config_of(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("config")).expect("json")
}

// ------------------------------------------------------------------ the writer

fn write_turn(turn: Value) -> Vec<Value> {
    emit_all(
        &shipped_script(WRITER),
        &json!({
            "header": {"context": {"audience_set": AUDIENCE, "channel": "group:trip",
                                   "session_id": "s-1", "turn_id": "s-1#4",
                                   "agent_id": "agent:helper", "speaker": "member:alex"}},
            "messages": [turn]
        }),
    )
}

fn episode_row(out: &[Value]) -> Value {
    let wstore = out
        .iter()
        .find(|m| m["header"]["route"] == "wstore")
        .unwrap_or_else(|| panic!("the turn was not written: {out:?}"));
    args_of(wstore)["row"].clone()
}

fn queue_item(out: &[Value]) -> Value {
    let enq = out
        .iter()
        .find(|m| m["header"]["route"] == "enqueue")
        .expect("the turn was not queued");
    args_of(enq)
}

#[test]
fn the_writer_keeps_a_peer_turn_with_its_speaker_reference() {
    let out = write_turn(json!({"origin": "peer", "type": "text",
                                "text": "Alex agreed to drive on Saturday.",
                                "speaker": "Sam", "speaker_ref": REF}));
    let ep = episode_row(&out);
    assert_eq!(ep["sender"], "peer", "{ep}");
    assert_eq!(
        ep["speaker"], REF,
        "the speaker column must name the reference: {ep}"
    );
    let aud: Value = serde_json::from_str(ep["audience_set"].as_str().expect("set")).expect("json");
    assert_eq!(aud, json!(["agent:helper", "member:alex", "peer:sam"]));
    assert_eq!(ep["channel"], "group:trip");
    let item = queue_item(&out);
    assert_eq!(
        item["speaker"], REF,
        "the queue item lost the speaker: {item}"
    );
    assert_eq!(item["sender"], "peer");
}

#[test]
fn a_peer_turn_without_a_reference_never_borrows_the_agent_id() {
    let out = write_turn(json!({"origin": "peer", "type": "text", "text": "hi"}));
    assert_eq!(episode_row(&out)["speaker"], "");
}

#[test]
fn a_user_turn_is_written_as_before() {
    let out = write_turn(json!({"origin": "user", "type": "text", "text": "hello"}));
    let ep = episode_row(&out);
    assert_eq!(ep["sender"], "user");
    assert_eq!(ep["speaker"], "member:alex");
    assert_eq!(queue_item(&out)["speaker"], "member:alex");
}

// ---------------------------------------------------------- the extraction lane

fn extract(phase: &str, op: &str, rows: Value, extra_ctx: Value) -> Vec<Value> {
    let mut ctx = json!({"mem_phase": phase, "batch_id": "inline:b1", "session_id": "s-1",
                         "audience_set": AUDIENCE, "channel": "group:trip"});
    if let (Some(c), Some(x)) = (ctx.as_object_mut(), extra_ctx.as_object()) {
        for (k, v) in x {
            c.insert(k.clone(), v.clone());
        }
    }
    emit_all(
        &shipped_script(EXTRACT),
        &json!({
            "header": {"context": ctx, "hop": {"operation": op, "rows_affected": 1}},
            "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                          "text": rows.to_string()}]
        }),
    )
}

#[test]
fn the_queue_row_carries_the_speaker() {
    let item = json!({"episode_id": "ep-p", "session_id": "s-1", "channel": "group:trip",
                      "audience_set": AUDIENCE, "sender": "peer", "speaker": REF,
                      "content": "x", "token_est": 1});
    let out = emit_all(
        &shipped_script(EXTRACT),
        &json!({
            "header": {"context": {"mem_phase": "enqueue", "audience_set": AUDIENCE,
                                   "channel": "group:trip"}},
            "messages": [{"origin": "assistant", "type": "text", "text": item.to_string()}]
        }),
    );
    let row = &args_of(&out[0])["row"];
    assert_eq!(args_of(&out[0])["table"], "pending_extraction");
    assert_eq!(row["speaker"], REF, "{row}");
    let schema = &config_of(STORE)["params"]["schema"];
    assert_eq!(schema["pending_extraction"]["speaker"], "text");
    assert_eq!(schema["facts"]["source"], "text");
}

#[test]
fn the_episode_page_names_the_source_of_each_turn() {
    let eps = json!([
        {"id": "ep-p", "happened_at": "2026-09-20T10:00:00Z", "session_id": "s-1",
         "channel": "group:trip", "audience_set": AUDIENCE, "sender": "peer", "speaker": REF},
        {"id": "ep-u", "happened_at": "2026-09-20T09:00:00Z", "session_id": "s-1",
         "channel": "group:trip", "audience_set": AUDIENCE, "sender": "user",
         "speaker": "member:alex"}
    ]);
    // The select that feeds the page has to ask for the two columns.
    let fetch = extract("eps-fetch", "insert", json!([]), json!({}));
    let cols = args_of(&fetch[0])["columns"].clone();
    for c in ["sender", "speaker"] {
        assert!(
            cols.as_array().expect("cols").iter().any(|x| x == c),
            "{cols}"
        );
    }
    let out = extract("known-eps", "select", eps, json!({}));
    let parked = args_of(&out[0]);
    let map: Value =
        serde_json::from_str(parked["row"]["payload"].as_str().expect("payload")).expect("json");
    assert_eq!(map["ep-p"]["source"], REF, "{map}");
    assert_eq!(
        map["ep-u"]["source"], "",
        "the member's own turn has no source: {map}"
    );
}

/// The `apply` meeting: payload, known set, episode map and (for a close pass)
/// the window of what it was shown.
fn apply(facts: Value, eps: Value, window: Option<Value>) -> Vec<Value> {
    let mut rows = vec![
        json!({"key": "inline:b1", "kind": "payload",
               "payload": json!({"facts": facts, "entities": [], "edges": []}).to_string()}),
        json!({"key": "inline:b1", "kind": "known", "payload": "[]"}),
        json!({"key": "inline:b1", "kind": "episodes", "payload": eps.to_string()}),
    ];
    if let Some(w) = window {
        rows.push(json!({"key": "inline:b1", "kind": "window", "payload": w.to_string()}));
    }
    extract("apply", "select", Value::Array(rows), json!({}))
}

fn staged(episode: &str, claim: &str, replaces: &str) -> Value {
    json!({"episode_id": episode, "subject": "member:alex", "predicate": "agreed_to",
           "claim": claim, "fact_kind": "experience", "valid_from": "2026-09-20T10:00:00Z",
           "valid_until": null, "confidence": 70, "replaces": replaces,
           "claim_hash": format!("h-{claim}")})
}

fn eps_map() -> Value {
    json!({
        "ep-p": {"happened_at": "2026-09-20T10:00:00Z", "session_id": "s-1",
                 "channel": "group:trip", "audience_set": AUDIENCE, "source": REF},
        "ep-u": {"happened_at": "2026-09-20T09:00:00Z", "session_id": "s-1",
                 "channel": "group:trip", "audience_set": AUDIENCE, "source": ""}
    })
}

fn inserted_facts(out: &[Value]) -> Vec<Value> {
    out.iter()
        .map(args_of)
        .filter(|a| a["operation"] == "insert" && a["table"] == "facts")
        .map(|a| a["row"].clone())
        .collect()
}

fn fact_closures(out: &[Value]) -> Vec<Value> {
    out.iter()
        .map(args_of)
        .filter(|a| a["operation"] == "update" && a["table"] == "facts")
        .collect()
}

#[test]
fn a_fact_from_a_peer_turn_carries_the_source_and_the_channel_set() {
    let out = apply(
        json!([staged("ep-p", "drives on Saturday", "")]),
        eps_map(),
        None,
    );
    let facts = inserted_facts(&out);
    assert_eq!(facts.len(), 1, "{out:?}");
    assert_eq!(facts[0]["source"], REF, "{}", facts[0]);
    assert_eq!(facts[0]["audience_set"], AUDIENCE);
    assert_eq!(facts[0]["channel"], "group:trip");
    let own = apply(json!([staged("ep-u", "likes maps", "")]), eps_map(), None);
    assert_eq!(
        inserted_facts(&own)[0]["source"],
        "",
        "a user turn is not attributed"
    );
}

fn window(source: &str) -> Value {
    let mut st = json!({"id": "f-own", "claim": "declined to drive",
                        "since": "2026-09-01T00:00:00Z",
                        "last_asserted": "2026-09-01T00:00:00Z"});
    if !source.is_empty() {
        st["source"] = json!(source);
    }
    json!([{"subject": "member:alex", "predicate": "agreed_to", "statements": [st]}])
}

#[test]
fn a_peer_claim_about_the_member_closes_no_fact_of_another_source() {
    // The window says f-own is the member's own statement (no source). The peer
    // episode's fact names it in `replaces` -- and must not end it.
    let out = apply(
        json!([staged("ep-p", "agreed to drive", "f-own")]),
        eps_map(),
        Some(window("")),
    );
    assert_eq!(
        inserted_facts(&out).len(),
        1,
        "the peer's claim itself is kept"
    );
    assert!(
        fact_closures(&out).is_empty(),
        "a claim by {REF} closed the member's own statement: {:?}",
        fact_closures(&out)
    );
    let refused: Vec<Value> = out
        .iter()
        .map(args_of)
        .filter(|a| a["table"] == "scratch" && a["row"]["kind"] == "extract-refusals")
        .collect();
    assert_eq!(refused.len(), 1, "the refusal is not receipted: {out:?}");
    // The reverse direction: the member's own statement never ends the peer's.
    let back = apply(
        json!([staged("ep-u", "agreed to drive", "f-own")]),
        eps_map(),
        Some(window(REF)),
    );
    assert!(
        fact_closures(&back).is_empty(),
        "{:?}",
        fact_closures(&back)
    );
}

#[test]
fn within_one_source_a_replacement_still_closes_and_the_where_pins_the_source() {
    let out = apply(
        json!([staged("ep-p", "agreed to drive", "f-own")]),
        eps_map(),
        Some(window(REF)),
    );
    let closes = fact_closures(&out);
    assert_eq!(closes.len(), 1, "{out:?}");
    assert_eq!(closes[0]["where"]["id"], "f-own");
    assert_eq!(
        closes[0]["where"]["source"],
        json!({"eq": REF}),
        "{}",
        closes[0]
    );
    // And for the member's own side the where accepts the pre-column NULL too.
    let own = apply(
        json!([staged("ep-u", "agreed to drive", "f-own")]),
        eps_map(),
        Some(window("")),
    );
    let closes = fact_closures(&own);
    assert_eq!(closes.len(), 1, "{own:?}");
    assert_eq!(closes[0]["where"]["source"], json!({"or_null": {"eq": ""}}));
}

// ----------------------------------------------------------------- the night

/// The shared statement helper of recall and dream-glue, probed in each script.
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

#[test]
fn the_chain_never_lets_one_source_end_another() {
    // Two assertions of one claim on one axis: the member's own, then the
    // peer's. Without the source in the statement identity the later one would
    // end the earlier as a "re-assertion".
    let program = r#"
rows = [
  {"id": "a", "canonical_subject": "member:alex", "canonical_predicate": "agreed_to",
   "claim": "drive", "valid_from": "2026-09-01", "recorded_at": "2026-09-01", "source": ""},
  {"id": "b", "canonical_subject": "member:alex", "canonical_predicate": "agreed_to",
   "claim": "drive", "valid_from": "2026-09-20", "recorded_at": "2026-09-20",
   "source": "3a47fe3e"},
]
chain = build_chains(rows)[("member:alex", "agreed_to")]
print(json.dumps([next_reassertion(chain, 0), statement_key(rows[0]) == statement_key(rows[1])]))
"#;
    for cfg in [RECALL, DREAM] {
        assert_eq!(probe(cfg, program), "[null, false]", "{cfg}");
    }
}

#[test]
fn the_night_withdraws_a_closure_that_crossed_sources() {
    // A closure some author wrote across sources (the judge, an older ingress):
    // the arithmetic of the night takes it back, whoever signed it.
    let program = r#"
rows = [
  {"id": "a", "canonical_subject": "member:alex", "canonical_predicate": "agreed_to",
   "claim": "no", "valid_from": "2026-09-01", "recorded_at": "2026-09-01", "source": "",
   "expired_at": "2026-09-20", "superseded_by": "b", "closure_source": "judge:r0"},
  {"id": "b", "canonical_subject": "member:alex", "canonical_predicate": "agreed_to",
   "claim": "yes", "valid_from": "2026-09-20", "recorded_at": "2026-09-20",
   "source": "3a47fe3e", "expired_at": None, "superseded_by": None, "closure_source": ""},
]
print(json.dumps([list(t) for t in derive_supersessions(rows)]))
"#;
    assert_eq!(
        probe(DREAM, program),
        "[[\"a\", null, null], [\"b\", null, null]]"
    );
}

#[test]
fn the_night_reads_the_source_of_every_fact_it_chains() {
    let script = shipped_script(DREAM);
    for marker in ["\"sup-axes\")", "\"canon-scan\")"] {
        let at = script.find(marker).unwrap_or_else(|| panic!("no {marker}"));
        let head = &script[..at];
        let call = &head[head.rfind("\"columns\"").expect("columns")..];
        assert!(
            call.contains("\"source\""),
            "{marker}: the select does not read source"
        );
    }
}

// ------------------------------------------------------------------ recall

fn tier0(audience_now: Value) -> Vec<Value> {
    let fact = json!({"id": "f-p", "subject": "member:alex", "canonical_subject": "member:alex",
                      "predicate": "plans", "canonical_predicate": "plans",
                      "claim": "drives on Saturday", "fact_kind": "foresight",
                      "valid_from": "2026-09-20T10:00:00Z", "valid_until": null,
                      "expired_at": null, "confidence": 70, "channel": "group:trip",
                      "audience_set": AUDIENCE, "source": REF});
    emit_all(
        &shipped_script(RECALL),
        &json!({
            "header": {
                "context": {"mem_phase": "legs", "recall_id": "r1", "memory_tier": "0",
                            "recall_query": "", "recall_as_of": "2026-09-21T00:00:00Z",
                            "recall_window_from": "", "recall_window_to": "",
                            "audience_now": audience_now, "channel": "group:trip",
                            "session_id": "s-1"},
                "hop": {"operation": "bundle", "rows_affected": 1, "bundle_errors": 0}
            },
            "messages": [
                {"origin": "tool", "type": "tool_result", "id": "r-leg-episodes", "text": "[]"},
                {"origin": "tool", "type": "tool_result", "id": "r-leg-beliefs", "text": "[]"},
                {"origin": "tool", "type": "tool_result", "id": "r-leg-foresight",
                 "text": json!([fact]).to_string()}
            ],
            "results": [
                {"tool_call_id": "r-leg-episodes", "operation": "select", "rows_affected": 0},
                {"tool_call_id": "r-leg-beliefs", "operation": "select", "rows_affected": 0},
                {"tool_call_id": "r-leg-foresight", "operation": "select", "rows_affected": 1}
            ]
        }),
    )
}

fn bundle_text(out: &[Value]) -> String {
    out.iter()
        .find(|m| m["header"]["route"] == "bundle")
        .unwrap_or_else(|| panic!("no bundle: {out:?}"))["messages"][0]["text"]
        .as_str()
        .expect("text")
        .to_string()
}

#[test]
fn recall_says_who_said_it_and_only_inside_the_set() {
    let inside = bundle_text(&tier0(json!(["member:alex", "peer:sam"])));
    assert!(
        inside.contains(&format!("{REF} says: drives on Saturday")),
        "the claim is not attributed: {inside}"
    );
    let outside = bundle_text(&tier0(json!(["member:alex", "member:zoe"])));
    assert!(
        !outside.contains("drives on Saturday"),
        "a participant outside the set was told: {outside}"
    );
    // Every fact read of recall asks for the column it renders.
    let script = shipped_script(RECALL);
    let foresight = &script[script.find("(\"r-leg-foresight\",").expect("leg")..];
    let call = &foresight[..foresight.find("\"limit\"").expect("limit")];
    assert!(
        call.contains("\"source\""),
        "the tier-0 leg does not read source"
    );
}

// ------------------------------------------------------------------ the drain

#[test]
fn the_drain_passes_a_peer_turn_with_its_speaker() {
    let batch = json!({
        "header": {"context": {"session_id": "s-1", "audience_set": AUDIENCE,
                               "channel": "group:trip"},
                   "hop": {"route": "in_batch"}},
        "messages": [
            {"origin": "user", "type": "text", "text": "shall we go?"},
            {"origin": "peer", "type": "text", "text": "Alex agreed to drive.",
             "speaker": "Sam", "speaker_ref": REF}
        ]
    });
    let out = emit_all(&shipped_script(DRAIN), &batch);
    let parked = args_of(&out[0]);
    let turns: Value =
        serde_json::from_str(parked["row"]["payload"].as_str().expect("payload")).expect("json");
    assert_eq!(
        turns.as_array().expect("turns").len(),
        2,
        "the peer turn was dropped: {turns}"
    );
    assert_eq!(turns[1]["origin"], "peer");
    assert_eq!(turns[1]["speaker_ref"], REF);
    // And the episode it becomes carries the speaker to the hive's writer.
    let probe_doc = json!({
        "header": {"context": {"session_id": "s-1", "drain_phase": "probe",
                               "audience_set": AUDIENCE, "channel": "group:trip"},
                   "hop": {"operation": "select"}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                      "text": json!([{"id": "1", "kind": "batch",
                                      "payload": turns.to_string(),
                                      "drained_upto": 0}]).to_string()}]
    });
    let eps = emit_all(&shipped_script(DRAIN), &probe_doc);
    let peer = eps
        .iter()
        .find(|m| m["header"]["route"] == "episode" && m["messages"][0]["origin"] == "peer")
        .unwrap_or_else(|| panic!("no peer episode: {eps:?}"));
    assert_eq!(peer["messages"][0]["speaker_ref"], REF);
    assert_eq!(peer["messages"][0]["speaker"], "Sam");
}

/// Review of L1, M-6 (fix strand): an import document is not a checked
/// identity. A reference outside the schema's form (`^[0-9a-f]{8}([0-9a-f]{4})?$`)
/// is dropped and a name longer than the schema's 120 characters is cut, so
/// the episode the drain emits is a body the schema accepts -- not an
/// `invalid_ubf_body` in a debug colony and a silently dropped field in a
/// release one.
#[test]
fn the_drain_keeps_only_a_speaker_the_schema_accepts() {
    let long = "N".repeat(130);
    let batch = json!({
        "header": {"context": {"session_id": "s-1", "audience_set": AUDIENCE,
                               "channel": "group:trip"},
                   "hop": {"route": "in_batch"}},
        "messages": [
            {"origin": "peer", "type": "text", "text": "one", "speaker": long,
             "speaker_ref": "3A47FE3E"},
            {"origin": "peer", "type": "text", "text": "two", "speaker": "Sam",
             "speaker_ref": "3a47fe3e/../x"},
            {"origin": "peer", "type": "text", "text": "three", "speaker_ref": "3a47fe3e0b1c"}
        ]
    });
    let out = emit_all(&shipped_script(DRAIN), &batch);
    let parked = args_of(&out[0]);
    let turns: Value =
        serde_json::from_str(parked["row"]["payload"].as_str().expect("payload")).expect("json");
    let turns = turns.as_array().expect("turns");
    assert_eq!(turns.len(), 3, "{turns:?}");
    assert!(
        turns[0].get("speaker_ref").is_none() && turns[1].get("speaker_ref").is_none(),
        "a reference outside the schema's form travels on: {turns:?}"
    );
    assert_eq!(
        turns[0]["speaker"].as_str().map(|s| s.chars().count()),
        Some(120),
        "a name is cut to the schema's 120 characters: {turns:?}"
    );
    assert_eq!(turns[1]["speaker"], "Sam");
    assert_eq!(
        turns[2]["speaker_ref"], "3a47fe3e0b1c",
        "12 hex digits are a reference"
    );
}

// ------------------------------------- fix round 1: the turn a block answers

/// Review finding C1 (orchestrator ruling OR-SN-67). A sidecar or `remember`
/// block names no episode; the ingress used to hang it on the newest `user` or
/// `peer` episode of the session. In a channel with several speakers that is
/// the wrong turn as soon as two of them arrive together, or one lands while
/// the brain answers the other: A's words were filed as B's, and a peer's
/// words next to the member's own were filed as the member's own side.
///
/// The bind now reads the recent episodes of all three roles, newest first,
/// and fails closed whenever the turns it cannot tell apart name more than one
/// source. `rows` is that page.
fn bind(rows: Value) -> Vec<Value> {
    extract("inline-bind", "select", rows, json!({}))
}

fn ep(id: &str, sender: &str, speaker: &str, at: &str) -> Value {
    json!({"id": id, "turn_id": format!("s-1#{id}"), "session_id": "s-1", "sender": sender,
           "speaker": speaker, "recorded_at": at})
}

/// The episode a bind resolved, off the batch key the meeting read carries.
fn bound_to(out: &[Value]) -> Option<String> {
    out.iter()
        .find(|m| m["header"]["phase"] == "inline-apply")
        .and_then(|m| m["header"]["batch_id"].as_str())
        .map(|b| {
            b.split_once('|')
                .map(|(_, e)| e.to_string())
                .unwrap_or_default()
        })
}

fn refused_as(out: &[Value]) -> Option<String> {
    out.iter()
        .find(|m| m["header"]["route"] == "reject")
        .and_then(|m| m["header"]["reject_reason"].as_str())
        .map(str::to_string)
}

const B: &str = "5c0d9e21";

#[test]
fn the_bind_reads_the_recent_turns_of_every_role_with_their_speaker() {
    let out = extract("inline-turn", "insert", json!([]), json!({}));
    let select = args_of(&out[0]);
    assert_eq!(select["table"], "episodes");
    assert_eq!(
        select["where"]["sender"],
        json!({"in": ["user", "peer", "assistant"]}),
        "the bind has to see where the last answer stands: {select}"
    );
    assert!(
        select["columns"]
            .as_array()
            .expect("cols")
            .iter()
            .any(|c| c == "speaker"),
        "{select}"
    );
    assert!(
        select["limit"].as_u64().unwrap_or(0) > 1,
        "one row cannot say whether it is the only speaker: {select}"
    );
    assert_eq!(select["order_by"][0]["dir"], "desc");
}

#[test]
fn two_peers_in_one_arrival_bind_nothing() {
    let out = bind(json!([
        ep("ep-b", "peer", B, "2026-09-20T10:00:02Z"),
        ep("ep-a", "peer", REF, "2026-09-20T10:00:01Z"),
        ep("ep-x", "assistant", "agent:helper", "2026-09-20T09:59:00Z"),
        ep("ep-u", "user", "member:alex", "2026-09-20T09:58:00Z"),
    ]));
    assert_eq!(
        bound_to(&out),
        None,
        "a block after [peer A, peer B] was filed under one of them: {out:?}"
    );
    assert_eq!(
        refused_as(&out).as_deref(),
        Some("ambiguous_speaker"),
        "{out:?}"
    );
    assert!(
        out.iter().all(|m| m["header"]["route"] == "reject"),
        "a refused bind writes nothing: {out:?}"
    );
}

#[test]
fn a_peer_next_to_the_member_binds_nothing() {
    let out = bind(json!([
        ep("ep-p", "peer", REF, "2026-09-20T10:00:02Z"),
        ep("ep-u", "user", "member:alex", "2026-09-20T10:00:01Z"),
        ep("ep-x", "assistant", "agent:helper", "2026-09-20T09:59:00Z"),
    ]));
    assert_eq!(bound_to(&out), None, "{out:?}");
    assert_eq!(refused_as(&out).as_deref(), Some("ambiguous_speaker"));
    // And the other order: the member's turn on top, the peer's below it.
    let out = bind(json!([
        ep("ep-u", "user", "member:alex", "2026-09-20T10:00:02Z"),
        ep("ep-p", "peer", REF, "2026-09-20T10:00:01Z"),
    ]));
    assert_eq!(
        bound_to(&out),
        None,
        "a peer's words would have become the own side: {out:?}"
    );
}

#[test]
fn a_turn_that_may_have_arrived_after_the_answer_binds_nothing() {
    // The newest assistant episode may be THIS answer's own (it is written
    // concurrently), and the peer turn above it may have arrived while the
    // brain answered the one below.
    let out = bind(json!([
        ep("ep-b", "peer", B, "2026-09-20T10:00:05Z"),
        ep("ep-x", "assistant", "agent:helper", "2026-09-20T10:00:04Z"),
        ep("ep-a", "peer", REF, "2026-09-20T10:00:01Z"),
    ]));
    assert_eq!(bound_to(&out), None, "{out:?}");
    assert_eq!(refused_as(&out).as_deref(), Some("ambiguous_speaker"));
}

#[test]
fn the_answered_peer_turn_binds_when_the_answer_is_on_top() {
    // The answer's own episode landed first: the turns below it up to the
    // previous answer are the ones it answered, and they have one speaker.
    let out = bind(json!([
        ep("ep-x2", "assistant", "agent:helper", "2026-09-20T10:00:04Z"),
        ep("ep-a", "peer", REF, "2026-09-20T10:00:01Z"),
        ep("ep-x1", "assistant", "agent:helper", "2026-09-20T09:59:00Z"),
        ep("ep-u", "user", "member:alex", "2026-09-20T09:58:00Z"),
    ]));
    assert_eq!(bound_to(&out).as_deref(), Some("ep-a"), "{out:?}");
}

#[test]
fn a_session_with_one_speaker_binds_exactly_as_before() {
    let out = bind(json!([
        ep("ep-u2", "user", "member:alex", "2026-09-20T10:00:02Z"),
        ep("ep-x", "assistant", "agent:helper", "2026-09-20T10:00:01Z"),
        ep("ep-u1", "user", "member:alex", "2026-09-20T10:00:00Z"),
    ]));
    assert_eq!(bound_to(&out).as_deref(), Some("ep-u2"), "{out:?}");
    // A follow-up answer (two answers after one turn) still binds the turn.
    let out = bind(json!([
        ep("ep-x2", "assistant", "agent:helper", "2026-09-20T10:00:03Z"),
        ep("ep-x1", "assistant", "agent:helper", "2026-09-20T10:00:02Z"),
        ep("ep-u1", "user", "member:alex", "2026-09-20T10:00:00Z"),
    ]));
    assert_eq!(bound_to(&out).as_deref(), Some("ep-u1"), "{out:?}");
    // One peer as the only speaker binds too.
    let out = bind(json!([
        ep("ep-a2", "peer", REF, "2026-09-20T10:00:02Z"),
        ep("ep-a1", "peer", REF, "2026-09-20T10:00:01Z"),
    ]));
    assert_eq!(bound_to(&out).as_deref(), Some("ep-a2"), "{out:?}");
}

/// Review of N, m1: a full bind page of ONE source whose run reaches the end
/// of the page may continue below it -- B spoke, then A spoke more than a page
/// of times with no answer between, then the block came. Filing it as A would
/// take B's turn in the same run for A's. So the one-source shortcut, too,
/// fails closed on a full page whose run touches its end; a page with an
/// answer inside it binds as before.
#[test]
fn a_full_page_of_one_speaker_that_runs_to_its_end_binds_nothing() {
    let run: Vec<Value> = (0..32)
        .map(|i| {
            ep(
                &format!("ep-a{i}"),
                "peer",
                REF,
                &format!("2026-09-20T10:{:02}:00Z", 59 - i),
            )
        })
        .collect();
    let out = bind(Value::Array(run.clone()));
    assert_eq!(
        bound_to(&out),
        None,
        "a full one-speaker page that may continue below was bound: {out:?}"
    );
    assert_eq!(refused_as(&out).as_deref(), Some("ambiguous_speaker"));
    // With the previous answer inside the page, the run is whole: it binds.
    let mut closed = run[..31].to_vec();
    closed.push(ep(
        "ep-x",
        "assistant",
        "agent:helper",
        "2026-09-20T09:00:00Z",
    ));
    let out = bind(Value::Array(closed));
    assert_eq!(bound_to(&out).as_deref(), Some("ep-a0"), "{out:?}");
}

#[test]
fn a_peer_turn_without_a_reference_is_never_the_own_side() {
    let out = bind(json!([ep("ep-p", "peer", "", "2026-09-20T10:00:02Z")]));
    assert_eq!(bound_to(&out), None, "{out:?}");
    assert_eq!(refused_as(&out).as_deref(), Some("ambiguous_speaker"));
    // And on the close pass's road, where the block names its episode: the
    // episode map marks the turn unattributed and no fact is minted from it.
    let page = json!([{"id": "ep-p", "happened_at": "2026-09-20T10:00:00Z", "session_id": "s-1",
                       "channel": "group:trip", "audience_set": AUDIENCE, "sender": "peer",
                       "speaker": ""}]);
    let parked = args_of(&extract("known-eps", "select", page, json!({}))[0]);
    let map: Value =
        serde_json::from_str(parked["row"]["payload"].as_str().expect("payload")).expect("json");
    let out = apply(json!([staged("ep-p", "agreed to drive", "")]), map, None);
    assert!(
        inserted_facts(&out).is_empty(),
        "a peer's words without a reference were minted as the own side: {out:?}"
    );
}

// --------------------------- fix round 1: a belief keeps the source it rests on

/// Review finding I1. A belief is derived from facts and got their audience
/// by code (#244) -- but not their source, so a belief resting on what a peer
/// said read as `- belief: <statement>`, i.e. as settled knowledge.
fn night_beliefs(beliefs: Value, facts: Value) -> Vec<Value> {
    let gather = json!({
        "header": {"context": {"mem_phase": "belief-audience-park", "dream_run": "run-1",
                               "dream_to": "2026-09-21T03:00:00Z"},
                   "hop": {"operation": "select"}},
        "messages": [{"origin": "tool", "type": "tool_result", "text": facts.to_string()}]
    });
    let carried = emit_all(&shipped_script(DREAM), &gather)
        .iter()
        .map(args_of)
        .find(|a| a["operation"] == "insert" && a["row"].get("payload").is_some())
        .map(|a| json!({"kind": a["row"]["kind"], "payload": a["row"]["payload"]}))
        .expect("the night carries what it knows about the facts");
    let scratch = json!([
        {"kind": "verdicts", "payload": json!({"beliefs": beliefs}).to_string()},
        {"kind": "beliefs", "payload": "[]"},
        carried,
    ]);
    emit_all(
        &shipped_script(DREAM),
        &json!({
            "header": {"context": {"mem_phase": "apply-run", "dream_run": "run-1",
                                   "dream_to": "2026-09-21T03:00:00Z"},
                       "hop": {"operation": "select"}},
            "messages": [{"origin": "tool", "type": "tool_result", "text": scratch.to_string()}]
        }),
    )
}

fn belief(statement: &str, sources: &[&str]) -> Value {
    json!({"holder": "self", "statement": statement, "confidence": 70, "active": true,
           "source_fact_ids": sources})
}

#[test]
fn a_belief_reads_the_source_of_the_facts_it_rests_on() {
    let out = extract_select_of_belief_audience();
    assert!(
        out["columns"]
            .as_array()
            .expect("cols")
            .iter()
            .any(|c| c == "source"),
        "{out}"
    );
}

fn extract_select_of_belief_audience() -> Value {
    let doc = json!({
        "header": {"context": {"mem_phase": "belief-audience", "dream_run": "run-1",
                               "dream_to": "2026-09-21T03:00:00Z"},
                   "hop": {"operation": "insert"}},
        "messages": [{"origin": "tool", "type": "tool_result", "text": "[]"}]
    });
    args_of(&emit_all(&shipped_script(DREAM), &doc)[0])
}

#[test]
fn a_belief_over_two_sources_is_refused_and_one_over_a_peer_is_attributed() {
    let facts = json!([
        {"id": "f-own", "audience_set": AUDIENCE, "source": ""},
        {"id": "f-a", "audience_set": AUDIENCE, "source": REF},
        {"id": "f-a2", "audience_set": AUDIENCE, "source": REF},
        {"id": "f-b", "audience_set": AUDIENCE, "source": B},
    ]);
    let out = night_beliefs(
        json!([
            belief("mixed own and peer", &["f-own", "f-a"]),
            belief("mixed two peers", &["f-a", "f-b"]),
            belief("the member agreed to drive", &["f-a", "f-a2"]),
            belief("the member likes maps", &["f-own"]),
        ]),
        facts,
    );
    let inserts: Vec<Value> = out
        .iter()
        .map(args_of)
        .filter(|a| a["table"] == "beliefs" && a["operation"] == "insert")
        .map(|a| a["row"].clone())
        .collect();
    let statements: Vec<&str> = inserts
        .iter()
        .map(|r| r["statement"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        statements,
        vec!["the member agreed to drive", "the member likes maps"],
        "a belief over mixed sources was written: {inserts:?}"
    );
    assert_eq!(inserts[0]["observer"], REF, "{}", inserts[0]);
    assert!(
        inserts[1]["observer"].is_null(),
        "the own side stays unattributed: {}",
        inserts[1]
    );
    let close = out
        .iter()
        .map(args_of)
        .find(|a| a["table"] == "consolidation_log")
        .expect("the run closes");
    let receipt: Value =
        serde_json::from_str(close["set"]["verdicts"].as_str().expect("verdicts")).expect("json");
    let refused = receipt["belief_refusals"].as_array().expect("receipted");
    assert_eq!(refused.len(), 2, "{receipt}");
}

fn tier0_with(beliefs: Value, episodes: Value, audience_now: Value) -> Vec<Value> {
    emit_all(
        &shipped_script(RECALL),
        &json!({
            "header": {
                "context": {"mem_phase": "legs", "recall_id": "r1", "memory_tier": "0",
                            "recall_query": "", "recall_as_of": "2026-09-21T00:00:00Z",
                            "recall_window_from": "", "recall_window_to": "",
                            "audience_now": audience_now, "channel": "group:trip",
                            "session_id": "s-1"},
                "hop": {"operation": "bundle", "rows_affected": 1, "bundle_errors": 0}
            },
            "messages": [
                {"origin": "tool", "type": "tool_result", "id": "r-leg-episodes",
                 "text": episodes.to_string()},
                {"origin": "tool", "type": "tool_result", "id": "r-leg-beliefs",
                 "text": beliefs.to_string()},
                {"origin": "tool", "type": "tool_result", "id": "r-leg-foresight", "text": "[]"}
            ],
            "results": [
                {"tool_call_id": "r-leg-episodes", "operation": "select", "rows_affected": 1},
                {"tool_call_id": "r-leg-beliefs", "operation": "select", "rows_affected": 1},
                {"tool_call_id": "r-leg-foresight", "operation": "select", "rows_affected": 0}
            ]
        }),
    )
}

#[test]
fn recall_renders_a_peer_sourced_belief_attributed() {
    let beliefs = json!([
        {"id": "b1", "holder": "self", "statement": "the member agreed to drive",
         "confidence": 70, "active": 1, "updated_at": "2026-09-21T03:00:00Z",
         "audience_set": AUDIENCE, "observer": REF},
        {"id": "b2", "holder": "self", "statement": "the member likes maps",
         "confidence": 70, "active": 1, "updated_at": "2026-09-21T03:00:00Z",
         "audience_set": AUDIENCE, "observer": null}
    ]);
    let text = bundle_text(&tier0_with(
        beliefs,
        json!([]),
        json!(["member:alex", "peer:sam"]),
    ));
    assert!(
        text.contains(&format!("- belief: {REF} says: the member agreed to drive")),
        "{text}"
    );
    assert!(text.contains("- belief: the member likes maps"), "{text}");
    let script = shipped_script(RECALL);
    for leg in ["(\"r-leg-beliefs\",", "(\"r-fan-beliefs\","] {
        let at = &script[script.find(leg).unwrap_or_else(|| panic!("{leg}"))..];
        let call = &at[..at.find("\"limit\"").expect("limit")];
        assert!(
            call.contains("\"observer\""),
            "{leg} does not read observer"
        );
    }
}

// ------------------------ fix round 1: an episode hit says who spoke (M5)

#[test]
fn an_episode_hit_from_a_peer_turn_names_the_speaker() {
    let eps = json!([
        {"id": "e1", "session_id": "s-1", "sender": "peer", "speaker": REF,
         "content": "Alex agreed to drive.", "happened_at": "2026-09-20T10:00:00Z",
         "recorded_at": "2026-09-20T10:00:00Z", "channel": "group:trip",
         "audience_set": AUDIENCE},
        {"id": "e2", "session_id": "s-1", "sender": "user", "speaker": "member:alex",
         "content": "shall we go?", "happened_at": "2026-09-20T09:59:00Z",
         "recorded_at": "2026-09-20T09:59:00Z", "channel": "group:trip",
         "audience_set": AUDIENCE}
    ]);
    let text = bundle_text(&tier0_with(
        json!([]),
        eps,
        json!(["member:alex", "peer:sam"]),
    ));
    assert!(
        text.contains(&format!("- peer {REF}: Alex agreed to drive.")),
        "{text}"
    );
    assert!(text.contains("- user: shall we go?"), "{text}");
    // Tier 1/2: both episode reads carry the column, and the one helper every
    // episode candidate is named by renders the same form.
    let script = shipped_script(RECALL);
    for leg in ["(\"r-leg-episodes\",", "(\"r-hyd-ep\","] {
        let at = &script[script.find(leg).unwrap_or_else(|| panic!("{leg}"))..];
        let call = &at[..at.find("\"where\"").expect("where")];
        assert!(call.contains("\"speaker\""), "{leg} does not read speaker");
    }
    let program = r#"
print(json.dumps([said_by({"sender": "peer", "speaker": "3a47fe3e"}),
                  said_by({"sender": "peer", "speaker": ""}),
                  said_by({"sender": "user", "speaker": "member:alex"})]))
"#;
    assert_eq!(
        probe(RECALL, program),
        "[\"peer 3a47fe3e\", \"peer\", \"user\"]"
    );
}

// ------------ fix round 1: a crossed judge closure on a full axis page (M4)

#[test]
fn a_full_axis_page_still_withdraws_a_closure_across_sources() {
    let rows = json!([
        {"id": "a", "canonical_subject": "member:alex", "canonical_predicate": "agreed_to",
         "claim": "no", "valid_from": "2026-09-01", "recorded_at": "2026-09-01", "source": "",
         "expired_at": "2026-09-20", "superseded_by": "b", "closure_source": "judge:r0"},
        {"id": "b", "canonical_subject": "member:alex", "canonical_predicate": "agreed_to",
         "claim": "yes", "valid_from": "2026-09-20", "recorded_at": "2026-09-20",
         "source": REF, "expired_at": null, "superseded_by": null, "closure_source": ""},
        {"id": "c", "canonical_subject": "member:alex", "canonical_predicate": "likes",
         "claim": "maps", "valid_from": "2026-09-02", "recorded_at": "2026-09-02", "source": "",
         "expired_at": null, "superseded_by": null, "closure_source": ""}
    ]);
    let out = emit_all(
        &shipped_script(DREAM),
        &json!({
            "header": {"context": {"mem_phase": "sup-axes", "dream_run": "run-1",
                                   "dream_to": "2026-09-21T03:00:00Z"},
                       "hop": {"operation": "select", "rows_affected": 3}},
            "params": {"dream_axis_limit": 3},
            "messages": [{"origin": "tool", "type": "tool_result", "text": rows.to_string()}]
        }),
    );
    let withdrawn: Vec<Value> = out
        .iter()
        .map(args_of)
        .filter(|a| a["table"] == "facts" && a["operation"] == "update")
        .collect();
    assert_eq!(
        withdrawn.len(),
        1,
        "a full page left the closure across sources standing: {out:?}"
    );
    assert_eq!(withdrawn[0]["where"]["id"], "a");
    assert!(withdrawn[0]["set"]["expired_at"].is_null());
    assert!(withdrawn[0]["set"]["superseded_by"].is_null());
    assert_eq!(withdrawn[0]["set"]["closure_source"], "");
}
