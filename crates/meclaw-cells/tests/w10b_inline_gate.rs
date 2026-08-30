//! meclaw-os W10b -- the `remember` tool form of the inline ingress, and the
//! quality gate the v8 inline-extraction review asks for (its sections 5.1,
//! 6.3, E4/E7).
//!
//! Two halves, both measured against the SHIPPED `extract-glue` script.
//!
//! **The form.** Until now the inline ingress took a block that NAMED the turn
//! it spoke for. A `remember` tool call cannot: an `episodes.id` is a uuid the
//! hive's writer mints, and a front model standing in the middle of a turn has
//! never seen one. A model-invented id covers nothing at best and the wrong turn
//! at worst -- so the block names no turn, and the hive BINDS it: the newest
//! `user` episode of the session the call travelled in, which is the turn the
//! per-turn lane of wave 9 wrote milliseconds earlier under
//! `turn_id = "<session_id>#<index>"`. The formula is not recomputed here; the
//! row it produced is looked up. Same reasoning as `w9a`: one minter.
//!
//! **The gate.** Neither the P15 invariance gate nor the drain's answer gate
//! measures what inline extraction risks (review section 4.2): both compare
//! answers before and after a run, and a fifth predicate spelling makes the
//! answer worse BEFORE anything runs. So the review defines two metrics, and
//! this file is where they become computable instead of quoted:
//!
//! * **M1 -- predicate spread per axis.** `count(distinct predicate)` grouped by
//!   subject, over the facts one block stages. One relation must arrive on ONE
//!   axis however the model spelled it. This is the number GitHub #53 would have
//!   been caught by.
//! * **M2 -- validity closed on arrival.** The number of staged facts whose
//!   `valid_until` already lies in the past. Must be **0**: such a fact is
//!   invisible to the as-of leg and visible to keyword and semantic, which is
//!   worse than a duplicate (damage 2 of GitHub #53).
//!
//! Both are read off the store ops the real script emits, so the gate measures
//! the lane rather than a description of it. Nothing here costs anything: no
//! provider, no colony, one python process per case.

use std::collections::BTreeMap;
use std::io::Write;
use std::process::{Command, Stdio};

const GLUE_CONFIG: &str = "../../templates/memory-hive/extract-glue/config.json";

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty string --
/// the same substitution the colony performs when it instantiates the template.
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

fn glue_script() -> String {
    let raw = std::fs::read_to_string(GLUE_CONFIG).expect("extract-glue config");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
    )
}

/// Run a shipped script over a real stdin document, handing the script to
/// python3 **on stdin** instead of in argv.
///
/// A single argv string is capped at 128 KiB (`MAX_ARG_STRLEN`) and the shipped
/// scripts have grown to within a few KB of that line, so `python3 -c <whole
/// script>` is a harness that breaks on size rather than on behaviour (GH #279,
/// precedent 89a522e4). stdin carries the program, so the document rides inside
/// it and is put under `sys.stdin` before the script runs. From there the script
/// executes exactly as `python3 -c` ran it: same `__main__` globals, same
/// stdout, same exit status.
fn run_script_on_stdin(script: &str, stdin_doc: &str) -> std::process::Output {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        serde_json::to_string(script).unwrap(),
        serde_json::to_string(stdin_doc).unwrap(),
    );
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    // Dropped, not merely borrowed: python reads until EOF.
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    child.wait_with_output().expect("wait")
}

fn emit(doc: serde_json::Value) -> Vec<serde_json::Value> {
    let out = run_script_on_stdin(
        &glue_script(),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    assert!(
        out.status.success(),
        "extract-glue exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// The room the `remember` calls of this file are written in.
const CHANNEL: &str = "c-w10b";

/// The round a `remember` call is written in: the person whose turn is being
/// answered and the agent writing the answer that carries the call. An inline
/// block is emitted INSIDE the answering turn, so the audience of that turn is
/// its audience -- and it is the round the facts here speak about
/// (`subject: "user"`, and in one case a third party `ada` who is TALKED ABOUT
/// rather than present, which is exactly why she is not in the set).
/// Deliberately not `["*"]`: a universal set would let every case below pass
/// against a write path with no gate at all.
const AUDIENCE: &str = r#"["member:user","agent:assistant"]"#;

/// The `remember` call as the dispatcher hands it over and the port edge stamps
/// it: a `tool_call` turn whose text is the raw arguments string, the session of
/// the conversation still in the context (the seam edge promoted it), the two
/// keys the `inline-extraction` port prescribes -- and the provenance the gate
/// requires since #244, which the same edge promotes: who was present
/// (`audience_set`) and where (`channel`).
fn remember(session: &str, arguments: &str) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "inline", "mem_phase": "inline",
                        "session_id": session,
                        "audience_set": AUDIENCE, "channel": CHANNEL},
            "hop": {"route": "tool", "tool_name": "remember", "async": "1"}
        },
        "messages": [{"origin": "assistant", "type": "tool_call", "id": "call-1",
                      "text": arguments}]
    })
}

/// The store's answer to the bind select: the newest `user` episode of the
/// session, exactly one row.
fn bind_echo(batch_id: &str, session: &str, rows: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "extract", "mem_phase": "inline-bind",
                        "batch_id": batch_id, "session_id": session},
            "hop": {"operation": "select", "rows_affected": 1}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                      "text": rows.to_string()}]
    })
}

/// The store's answer to the insert of the parked payload.
fn park_echo(phase: &str, batch_id: &str, session: &str) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "extract", "mem_phase": phase,
                        "batch_id": batch_id, "session_id": session},
            "hop": {"operation": "insert", "rows_affected": 1}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r", "text": "[]"}]
    })
}

/// The store's answer to the meeting-point select of the parked payload.
fn payload_echo(batch_id: &str, session: &str, payload: &str) -> serde_json::Value {
    let rows = serde_json::json!([{"key": batch_id, "kind": "inline", "payload": payload}]);
    serde_json::json!({
        "header": {
            "context": {"store_origin": "extract", "mem_phase": "inline-apply",
                        "batch_id": batch_id, "session_id": session},
            "hop": {"operation": "select", "rows_affected": 1}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                      "text": rows.to_string()}]
    })
}

fn args_of(msg: &serde_json::Value) -> serde_json::Value {
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    serde_json::from_str(text).expect("op args")
}

fn store_ops(msgs: &[serde_json::Value]) -> Vec<serde_json::Value> {
    msgs.iter()
        .filter(|m| m["header"]["route"] == "xstore")
        .map(args_of)
        .collect()
}

/// The batch id the lane minted for this block -- it is the scratch key the whole
/// chain hangs on.
fn batch_id_of(msgs: &[serde_json::Value]) -> String {
    msgs.iter()
        .find(|m| m["header"]["route"] == "xstore")
        .and_then(|m| m["header"]["batch_id"].as_str())
        .expect("every store op of this lane names its batch")
        .to_string()
}

fn rejected(msgs: &[serde_json::Value]) -> bool {
    msgs.iter().any(|m| m["header"]["route"] == "reject")
}

/// The queue op that takes covered turns out of `pending_extraction`.
fn queue_op(msgs: &[serde_json::Value]) -> Option<serde_json::Value> {
    store_ops(msgs)
        .into_iter()
        .find(|a| a["table"] == "pending_extraction")
}

/// The payload the lane staged for the dedup phase.
fn staged(msgs: &[serde_json::Value]) -> Option<serde_json::Value> {
    store_ops(msgs)
        .into_iter()
        .find(|a| a["table"] == "scratch" && a["row"]["kind"] == "payload")
        .map(|a| {
            serde_json::from_str(a["row"]["payload"].as_str().expect("payload string"))
                .expect("payload json")
        })
}

/// The payload the lane parked while it resolves the turn.
fn parked(msgs: &[serde_json::Value]) -> Option<String> {
    store_ops(msgs)
        .into_iter()
        .find(|a| a["table"] == "scratch" && a["row"]["kind"] == "inline")
        .map(|a| {
            a["row"]["payload"]
                .as_str()
                .expect("parked string")
                .to_string()
        })
}

/// The select the lane emits to find the turn its block speaks for.
fn bind_select(msgs: &[serde_json::Value]) -> Option<serde_json::Value> {
    store_ops(msgs)
        .into_iter()
        .find(|a| a["table"] == "episodes" && a["operation"] == "select")
}

fn facts_of(payload: &serde_json::Value) -> Vec<serde_json::Value> {
    payload["facts"].as_array().cloned().unwrap_or_default()
}

// ---------------------------------------------------------------- the metrics

/// **M1 -- predicate spread per axis.** `count(distinct predicate)` grouped by
/// subject, over the facts a block stages. The review's own words: the count of
/// DISTINCT predicates per axis is the number that would have caught #53. A
/// `GROUP BY` in the store, a `BTreeMap` here.
fn predicate_spread(facts: &[serde_json::Value]) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in facts {
        let subject = f["subject"].as_str().unwrap_or_default().to_string();
        let predicate = f["predicate"].as_str().unwrap_or_default().to_string();
        let axis = out.entry(subject).or_default();
        if !axis.contains(&predicate) {
            axis.push(predicate);
        }
    }
    for axis in out.values_mut() {
        axis.sort();
    }
    out
}

/// **M2 -- validity closed on arrival.** The number of staged facts whose
/// `valid_until` already lies in the past. `now` is passed in as the instant the
/// measurement is taken against; the lane itself compares against its own clock.
fn closed_on_arrival(facts: &[serde_json::Value], now: &str) -> usize {
    facts
        .iter()
        .filter(|f| match f["valid_until"].as_str() {
            Some(until) => !until.is_empty() && until < now,
            None => false,
        })
        .count()
}

/// Drives one `remember` block through the whole bind chain and returns the
/// facts it finally staged, plus the coverage op it emitted.
fn run_remember(
    session: &str,
    arguments: &str,
    episode_id: &str,
) -> (Vec<serde_json::Value>, serde_json::Value) {
    let first = emit(remember(session, arguments));
    let bid = batch_id_of(&first);
    let payload = parked(&first).expect("the block is parked while the turn is resolved");

    let second = emit(park_echo("inline-turn", &bid, session));
    let select = bind_select(&second).expect("the lane asks the store which turn it is answering");
    assert_eq!(batch_id_of(&second), bid, "the chain keeps its key");
    assert_eq!(
        select["table"], "episodes",
        "the turn is resolved against the episodes"
    );

    let rows = serde_json::json!([
        {"id": episode_id, "turn_id": format!("{session}#0"), "session_id": session,
         "sender": "user", "recorded_at": "2026-08-15T09:00:00.000000Z"}
    ]);
    let third = emit(bind_echo(&bid, session, rows));
    let bid3 = batch_id_of(&third);

    let fourth = emit(payload_echo(&bid3, session, &payload));
    let staged = staged(&fourth).expect("the bound block stages its facts");
    let queue = queue_op(&fourth).expect("the bound block covers the turn it was bound to");
    (facts_of(&staged), queue)
}

fn one_fact_args(subject: &str, predicate: &str, claim: &str) -> String {
    serde_json::json!({
        "facts": [{"subject": subject, "predicate": predicate, "claim": claim,
                   "fact_kind": "world", "confidence": 90}]
    })
    .to_string()
}

// ------------------------------------------------------------------- the form

#[test]
fn a_remember_call_arrives_as_a_tool_call_and_is_read_as_one() {
    // The seam that would have made the whole lane silently dead. The dispatcher
    // emits a tool call as a turn of type `tool_call` whose text is the raw
    // arguments string; the inline ingress used to read only `text` and
    // `tool_result` turns and would have seen an empty payload -- "not JSON",
    // rejected, every single time, with the answer none the wiser.
    let msgs = emit(remember(
        "s1",
        &one_fact_args("user", "favorite_color", "Blau"),
    ));
    assert!(
        !rejected(&msgs),
        "the arguments of a remember call are the payload: {msgs:?}"
    );
    assert!(
        parked(&msgs).is_some(),
        "and the block is parked while its turn is resolved: {msgs:?}"
    );
}

#[test]
fn a_block_that_names_no_turn_is_bound_to_the_session_it_travelled_in() {
    // A front model cannot name an episode: the id is a uuid the hive's writer
    // mints. So the block names none and the hive resolves it -- the newest
    // `user` episode of the session, which is the turn the per-turn lane wrote
    // while this answer was being generated.
    let first = emit(remember(
        "s1",
        &one_fact_args("user", "diet", "isst ketogen"),
    ));
    let bid = batch_id_of(&first);
    let second = emit(park_echo("inline-turn", &bid, "s1"));
    let select = bind_select(&second).expect("the lane resolves the turn it speaks for");
    assert_eq!(
        select["where"]["session_id"], "s1",
        "scoped to the conversation it came from"
    );
    assert_eq!(
        select["where"]["sender"], "user",
        "the turn being ANSWERED, never the answer -- the answer may not even be stored yet"
    );
    assert_eq!(
        select["limit"], 1,
        "exactly one turn, deterministically the newest"
    );
    assert_eq!(
        select["order_by"][0]["dir"], "desc",
        "newest first, or the block would hang on the first turn of the day"
    );
}

#[test]
fn the_bound_block_hangs_its_facts_on_the_turn_the_store_named() {
    // The whole point: the fact carries the episode the hive knows, so the
    // coverage of GitHub #52 bites, the event time comes off the episode instead
    // of the ingest clock, and the night can find both.
    let (facts, queue) = run_remember(
        "s1",
        &one_fact_args("user", "favorite_color", "Blau"),
        "ep-uuid-1",
    );
    assert_eq!(facts.len(), 1);
    assert_eq!(
        facts[0]["episode_id"], "ep-uuid-1",
        "the fact hangs on the resolved turn"
    );
    assert_eq!(queue["operation"], "update");
    assert_eq!(queue["set"]["status"], "inline");
    assert_eq!(
        queue["where"]["episode_id"]["in"],
        serde_json::json!(["ep-uuid-1"]),
        "and the resolved turn is the one that leaves the queue"
    );
    assert_eq!(
        queue["where"]["status"], "pending",
        "a row a batch already claimed is left to the batch that owns it"
    );
}

#[test]
fn a_session_the_store_has_no_turn_for_binds_nothing_and_writes_nothing() {
    // The safe direction, and the one that keeps the safety net a net: a block
    // whose turn is not in the store yet (the per-turn lane is off, or the
    // episode has not landed) covers NOTHING. The turn stays `pending` and the
    // batch lane extracts it later -- one extraction too late is a delay, a fact
    // hung on the wrong turn is a defect.
    let first = emit(remember(
        "s1",
        &one_fact_args("user", "diet", "isst ketogen"),
    ));
    let bid = batch_id_of(&first);
    let _ = emit(park_echo("inline-turn", &bid, "s1"));
    let msgs = emit(bind_echo(&bid, "s1", serde_json::json!([])));
    assert!(
        rejected(&msgs),
        "an unbindable block leaves through the reject port: {msgs:?}"
    );
    assert!(
        store_ops(&msgs).is_empty(),
        "and writes nothing at all: {msgs:?}"
    );
}

#[test]
fn a_remember_call_with_a_broken_payload_costs_the_answer_nothing() {
    // Guard 1 of the inline design, at the hive end: a malformed block is a
    // reject and zero store writes. The answer lane never sees it -- it left the
    // dispatcher on its own route before this message was even routed.
    let msgs = emit(remember("s1", "{\"facts\": [ this is not json"));
    assert!(
        rejected(&msgs),
        "garbage leaves through the reject port: {msgs:?}"
    );
    assert!(store_ops(&msgs).is_empty(), "and writes nothing: {msgs:?}");
}

#[test]
fn a_block_that_names_its_turn_keeps_the_lane_it_always_had() {
    // The operator probe and the replay form (README section Probes) name the
    // episode themselves. Nothing about that path moves: no bind, no park, cover
    // and stage in one hop.
    let payload = serde_json::json!({
        "episode_id": "e1",
        "facts": [{"episode_id": "e1", "subject": "user", "predicate": "favorite_color",
                   "claim": "Blau", "fact_kind": "world", "confidence": 90}]
    })
    .to_string();
    let msgs = emit(remember("s1", &payload));
    assert!(
        parked(&msgs).is_none(),
        "a block that names its turn needs no bind"
    );
    assert!(staged(&msgs).is_some(), "it stages straight away");
    let queue = queue_op(&msgs).expect("and covers the turn it named");
    assert_eq!(
        queue["where"]["episode_id"]["in"],
        serde_json::json!(["e1"])
    );
}

// ------------------------------------------------------------------- the gate

#[test]
fn m1_one_relation_written_three_ways_arrives_on_one_axis() {
    // METRIC 1, the one GitHub #53 would have been caught by. A front model
    // writing one turn cannot see the four spellings an axis already has, so the
    // contract asks for a KEY and the lane enforces the key's SYNTAX: lower
    // case, snake_case, no spaces. Three spellings of one relation, one axis.
    let args = serde_json::json!({
        "facts": [
            {"subject": "user", "predicate": "Favorite Color", "claim": "Blau",
             "fact_kind": "world", "confidence": 90},
            {"subject": "user", "predicate": "favorite color", "claim": "Blue",
             "fact_kind": "world", "confidence": 80},
            {"subject": "user", "predicate": "FavoriteColor", "claim": "blau",
             "fact_kind": "world", "confidence": 70}
        ]
    })
    .to_string();
    let (facts, _) = run_remember("s1", &args, "ep-uuid-1");
    let spread = predicate_spread(&facts);
    assert_eq!(
        spread.get("user").map(|a| a.len()),
        Some(1),
        "M1 = 1: one relation, one axis -- got {spread:?}"
    );
    assert_eq!(
        spread["user"][0], "favorite_color",
        "and the axis is the key form the contract asks for"
    );
}

#[test]
fn m1_does_not_merge_two_relations_into_one() {
    // The counter-direction, and the reason this is SYNTAX and not
    // canonicalisation: `preferred_editor` and `favorite_editor` are a synonym
    // pair, and merging them needs the whole axis in front of it. That is the
    // night's job (0.2.0 alias table); a lane standing inside one turn that
    // guessed at it would be the GC and the extractor pulling in opposite
    // directions on the same axis.
    let args = serde_json::json!({
        "facts": [
            {"subject": "user", "predicate": "favorite_editor", "claim": "Helix",
             "fact_kind": "world", "confidence": 90},
            {"subject": "user", "predicate": "preferred_editor", "claim": "Helix",
             "fact_kind": "world", "confidence": 90}
        ]
    })
    .to_string();
    let (facts, _) = run_remember("s1", &args, "ep-uuid-1");
    let spread = predicate_spread(&facts);
    assert_eq!(
        spread.get("user").map(|a| a.len()),
        Some(2),
        "two relations stay two axes, for the night to judge: {spread:?}"
    );
}

#[test]
fn m2_a_validity_that_lies_in_the_past_never_reaches_the_store() {
    // METRIC 2, and damage 2 of GitHub #53 made mechanical. A `valid_until`
    // taken from the range a QUESTION asked about closes the fact on arrival:
    // the as-of leg never sees it while keyword and semantic still do. One
    // statement visible to some legs and invisible to others is worse than a
    // duplicate, and it is invisible from the outside.
    let args = serde_json::json!({
        "facts": [{"subject": "user", "predicate": "favorite_editor", "claim": "Helix",
                   "fact_kind": "world", "valid_from": "2026-08-05",
                   "valid_until": "2026-08-08", "confidence": 90}]
    })
    .to_string();
    let (facts, _) = run_remember("s1", &args, "ep-uuid-1");
    assert_eq!(
        facts.len(),
        1,
        "the fact itself survives -- only its closure falls away"
    );
    assert!(
        facts[0]["valid_until"].is_null(),
        "M2 = 0: a validity already in the past is dropped, got {}",
        facts[0]
    );
}

#[test]
fn m2_a_validity_in_the_future_is_a_statement_about_the_world_and_survives() {
    // The direction that must NOT be dropped: "the lease runs until March" is a
    // legitimate claim with an end, and a lane that threw every `valid_until`
    // away would lose it.
    let args = serde_json::json!({
        "facts": [{"subject": "user", "predicate": "lives_in", "claim": "Elvese",
                   "fact_kind": "world", "valid_until": "2099-03-01", "confidence": 90}]
    })
    .to_string();
    let (facts, _) = run_remember("s1", &args, "ep-uuid-1");
    assert_eq!(
        facts[0]["valid_until"], "2099-03-01",
        "a validity in the future is kept"
    );
}

#[test]
fn the_gate_runs_over_one_example_batch() {
    // THE GATE (review section 6.3), in the form the other gates of this repo
    // take: one defined corpus, two numbers, a pass condition written down.
    //
    //   M1  distinct predicates per axis     must be 1 per relation the batch states
    //   M2  facts closed on arrival          must be 0
    //
    // The batch is deliberately the WORST realistic block: one relation in three
    // spellings, a second relation on the same subject, a third subject, and a
    // validity lifted out of a question's date range -- every failure mode #52
    // and #53 measured, in one payload.
    let args = serde_json::json!({
        "facts": [
            {"subject": "user", "predicate": "Favorite Editor", "claim": "Helix",
             "fact_kind": "world", "confidence": 90},
            {"subject": "user", "predicate": "favorite editor", "claim": "Zed",
             "fact_kind": "world", "valid_until": "2026-08-08", "confidence": 85},
            {"subject": "user", "predicate": "favorite_editor", "claim": "Helix",
             "fact_kind": "world", "confidence": 80},
            {"subject": "user", "predicate": "lives_in", "claim": "Elvese",
             "fact_kind": "world", "confidence": 95},
            {"subject": "ada", "predicate": "Has Pet", "claim": "a cat",
             "fact_kind": "world", "confidence": 70}
        ]
    })
    .to_string();
    let (facts, queue) = run_remember("s1", &args, "ep-uuid-1");

    let spread = predicate_spread(&facts);
    assert_eq!(
        spread,
        BTreeMap::from([
            ("ada".to_string(), vec!["has_pet".to_string()]),
            (
                "user".to_string(),
                vec!["favorite_editor".to_string(), "lives_in".to_string()]
            ),
        ]),
        "M1: two relations on `user`, one on `ada` -- five facts, three axes"
    );

    // M2 is measured against an instant AFTER the run, so a fact the lane let
    // through with a past validity would count.
    let now = "2099-01-01T00:00:00.000000Z";
    assert_eq!(
        closed_on_arrival(&facts, now),
        0,
        "M2 must be 0 -- a fact closed on arrival is invisible to the as-of leg: {facts:?}"
    );

    // And the third property the gate leans on: the block still covers exactly
    // the one turn it was bound to, so the batch lane buys no second opinion.
    assert_eq!(
        queue["where"]["episode_id"]["in"],
        serde_json::json!(["ep-uuid-1"])
    );
}
