//! GH #1042 -- a `remember` block written in a duplex call's delegation round
//! binds to the turn of the conversation it answers, and that turn lives under
//! the generation the session keeper minted, not under the call id.
//!
//! Measured at the receiver (a phone member, two calls on one line): the
//! delegation round travels with `context.session_id` = the call id, because a
//! delegation comes straight from the channel and never passes the keeper's
//! stamp. The person's turns of the same call go the other road -- channel,
//! firewall, keeper, curator, writer -- and are stored under the keeper's
//! generation (`phone-<instant>`), which outlives a single call. The only
//! episode under the call id is the delegation's own answer (`sender`
//! `assistant`), so the bind page held no turn, and every inline block of
//! every delegation was dead-lettered with "no turn to bind to" (7 of 7 call
//! ids in the store, each holding exactly one assistant row and no user row).
//!
//! The fix keeps the session page as the first read and adds ONE fallback:
//! a session page without a single `user`/`peer` turn is followed by a page of
//! the room (`channel`), and the block binds only inside the room's OPEN
//! generation -- the one the keeper would stamp the next turn with. A
//! generation ends in exactly two ways (session-keeper README): a turn of
//! another round seals it, or the room stays silent longer than the keeper's
//! idle window. So the page reaches back no further than that window, and the
//! generation is the one of the room's newest turn of a person -- which must be
//! of the block's own round. Review I1 of the first build: without that bound
//! the page held every generation of room and round, and a block whose turn
//! was not written yet bound to a turn hours or days old (the same store: the
//! previous evening's generation ended at 21:19, the next one opened at 06:25).
//!
//! Everything below runs the SHIPPED `extract-glue` script, one python process
//! per step; no provider, no colony. Times are relative to the wall clock,
//! because the open generation is a statement about NOW.

use std::io::Write;
use std::process::{Command, Stdio};

const GLUE_CONFIG: &str = "../../templates/memory-hive/extract-glue/config.json";
const KEEPER_CLOSE_CONFIG: &str = "../../templates/session-keeper/close/config.json";

/// The room: one line, one room, a redial is the same conversation
/// (`templates/freeswitch/README.md` section *Two conversations, or one?*).
const CHANNEL: &str = "phone";

/// The round of the call as the edge promotes it: unsorted and without spaces,
/// the way a door spells it.
const AUDIENCE: &str = r#"["member:rita","agent:companion"]"#;

/// The same round as the episode writer stores it: normalised (deduplicated,
/// sorted) and serialised by `json.dumps`, `", "` between the items. The
/// fallback read must use THIS spelling, or the store matches nothing.
const AUDIENCE_STORED: &str = r#"["agent:companion", "member:rita"]"#;

/// The keeper's generation, opened by the first call of the morning and still
/// open during the second call.
const GENERATION: &str = "phone-2026-10-06T06:25:08.981275Z";

/// The generation of the evening before, sealed by the night after two hours
/// of silence (measured: its last turn 21:19, the next generation 06:25).
const OLD_GENERATION: &str = "phone-2026-10-05T16:59:29.062343Z";

/// A round the line ran in between: somebody else joined the call.
const OTHER_ROUND_STORED: &str = r#"["agent:companion", "member:rita", "member:sam"]"#;

/// The second call's id: what `context.session_id` carries in its delegation.
const CALL_2: &str = "07336cd9-af4e-49a7-94ca-42ee467b1d64";

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

/// The script rides on stdin, not in argv (GH #279: argv is capped at 128 KiB).
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

/// The context of the delegation round as it reaches the memory hive: the call
/// id as the session, the room and the round promoted by the edges. The store
/// echoes it back on every answer, which is why each step below carries it.
fn context(phase: &str, batch_id: &str) -> serde_json::Value {
    let mut c = base_context(phase, batch_id);
    if CLOSE_PASS.with(|f| f.get()) {
        c["close_pass"] = serde_json::json!("1");
    }
    c
}

thread_local! {
    /// Set by the one test that drives a close-pass block through the chain.
    static CLOSE_PASS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn base_context(phase: &str, batch_id: &str) -> serde_json::Value {
    serde_json::json!({
        "store_origin": if phase == "inline" { "inline" } else { "extract" },
        "mem_phase": phase, "batch_id": batch_id,
        "session_id": CALL_2, "call_id": CALL_2,
        "engine": "duplex", "delegation_id": "item_1",
        "audience_set": AUDIENCE, "channel": CHANNEL
    })
}

/// The memory section of the companion's first answer in the second call: a
/// block that names no turn, as every sidecar block does.
fn sidecar_block(arguments: &str) -> serde_json::Value {
    serde_json::json!({
        "header": {"context": context("inline", ""),
                   "hop": {"route": "tool", "tool_name": "remember", "async": "1"}},
        "messages": [{"origin": "assistant", "type": "tool_call", "id": "call-1",
                      "text": arguments}]
    })
}

fn store_echo(
    phase: &str,
    batch_id: &str,
    operation: &str,
    rows: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "header": {"context": context(phase, batch_id),
                   "hop": {"operation": operation, "rows_affected": 1}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                      "text": rows.to_string()}]
    })
}

fn args_of(msg: &serde_json::Value) -> serde_json::Value {
    serde_json::from_str(msg["messages"][0]["text"].as_str().expect("op text")).expect("op args")
}

fn store_ops(msgs: &[serde_json::Value]) -> Vec<(String, serde_json::Value)> {
    msgs.iter()
        .filter(|m| m["header"]["route"] == "xstore")
        .map(|m| {
            (
                m["header"]["phase"].as_str().unwrap_or("").to_string(),
                args_of(m),
            )
        })
        .collect()
}

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

fn episodes_select(msgs: &[serde_json::Value]) -> Option<(String, serde_json::Value)> {
    store_ops(msgs)
        .into_iter()
        .find(|(_, a)| a["table"] == "episodes" && a["operation"] == "select")
}

fn one_fact_args() -> String {
    serde_json::json!({
        "facts": [{"subject": "user", "predicate": "has_pet", "claim": "has a tomcat called Moritz",
                   "fact_kind": "world", "confidence": 90}]
    })
    .to_string()
}

/// An instant `secs` seconds before now, in the writer's spelling.
fn ago(secs: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::seconds(secs))
        .format("%Y-%m-%dT%H:%M:%S%.6fZ")
        .to_string()
}

fn row_in(id: &str, session: &str, sender: &str, secs_ago: i64, round: &str) -> serde_json::Value {
    serde_json::json!({"id": id, "turn_id": format!("{session}#{id}"), "session_id": session,
                       "sender": sender, "speaker": "", "recorded_at": ago(secs_ago),
                       "channel": CHANNEL, "audience_set": round})
}

fn row(id: &str, session: &str, sender: &str, secs_ago: i64) -> serde_json::Value {
    row_in(id, session, sender, secs_ago, AUDIENCE_STORED)
}

/// The session page of the delegation: the only row under the call id is the
/// delegation's own answer, written by the same per-turn lane.
fn call_page() -> serde_json::Value {
    serde_json::json!([row("ep-deleg", CALL_2, "assistant", 1)])
}

/// The room page: the generation, newest first. Call 2's first user turn on
/// top (under the delegation's own answer), call 1's turns below, and the
/// generation's first turn three hours back -- still the open generation,
/// because the room never fell silent for two hours since.
fn room_page() -> serde_json::Value {
    serde_json::json!([
        row("ep-deleg", CALL_2, "assistant", 1),
        row("ep-call2-a1", GENERATION, "assistant", 4),
        row("ep-call2-u1", GENERATION, "user", 5),
        row("ep-call1-a9", GENERATION, "assistant", 40),
        row("ep-call1-u9", GENERATION, "user", 41),
        row("ep-call1-u1", GENERATION, "user", 3 * 3600),
    ])
}

/// Steps one and two of the chain: the block is parked, the session page is
/// asked for. Returns the batch id and the parked payload.
fn park() -> (String, String) {
    let first = emit(sidecar_block(&one_fact_args()));
    let bid = batch_id_of(&first);
    let payload = store_ops(&first)
        .into_iter()
        .find(|(_, a)| a["table"] == "scratch" && a["row"]["kind"] == "inline")
        .map(|(_, a)| a["row"]["payload"].as_str().expect("parked").to_string())
        .expect("the block is parked while the turn is resolved");
    let second = emit(store_echo(
        "inline-turn",
        &bid,
        "insert",
        serde_json::json!([]),
    ));
    let (phase, select) = episodes_select(&second).expect("the session page is read first");
    assert_eq!(phase, "inline-bind");
    assert_eq!(
        select["where"]["session_id"], CALL_2,
        "the first read stays the session page -- every channel whose turns and \
         answers share one session binds exactly as before"
    );
    (bid, payload)
}

// ------------------------------------------------------------------- the lock

#[test]
fn the_first_answer_of_a_second_call_binds_to_that_calls_user_turn() {
    // THE LOCK of GH #1042: a second call by the same member on the same line.
    // The session page of the delegation holds no turn of the person -- before
    // the fix this was the dead letter "no turn to bind to".
    let (bid, payload) = park();
    let third = emit(store_echo("inline-bind", &bid, "select", call_page()));
    assert!(
        !rejected(&third),
        "a session page without a turn of the person is not the end of the bind: {third:?}"
    );
    let (phase, room) = episodes_select(&third)
        .expect("the bind reads the generation of the room and the round instead");
    assert_eq!(phase, "inline-bind-room");
    assert_eq!(
        room["where"]["channel"], CHANNEL,
        "the room of the call, not its session"
    );

    let fourth = emit(store_echo("inline-bind-room", &bid, "select", room_page()));
    assert!(!rejected(&fourth), "the room page binds: {fourth:?}");
    let (phase, meet) = store_ops(&fourth)
        .into_iter()
        .find(|(_, a)| a["table"] == "scratch" && a["operation"] == "select")
        .expect("the bound block goes on to the meeting read");
    assert_eq!(phase, "inline-apply");
    assert_eq!(
        meet["where"]["key"], bid,
        "the parked block is met under its own key"
    );
    let carried = fourth
        .iter()
        .find(|m| m["header"]["phase"] == "inline-apply")
        .and_then(|m| m["header"]["batch_id"].as_str())
        .expect("the resolved turn rides in the batch key")
        .to_string();
    assert_eq!(
        carried,
        format!("{bid}|ep-call2-u1|{GENERATION}"),
        "bound to the SECOND call's user turn -- not to the delegation's own answer, \
         and not to a turn of the first call"
    );

    let fifth = emit(store_echo(
        "inline-apply",
        &carried,
        "select",
        serde_json::json!([{"key": bid, "kind": "inline", "payload": payload}]),
    ));
    let staged = store_ops(&fifth)
        .into_iter()
        .find(|(_, a)| a["table"] == "scratch" && a["row"]["kind"] == "payload")
        .map(|(_, a)| {
            serde_json::from_str::<serde_json::Value>(a["row"]["payload"].as_str().unwrap())
                .unwrap()
        })
        .expect("the bound block stages its fact");
    let facts = staged["facts"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        facts.len(),
        1,
        "the fact of the block reaches the store: {staged}"
    );
    assert_eq!(
        facts[0]["episode_id"], "ep-call2-u1",
        "and hangs on the second call's turn"
    );
    let cover = store_ops(&fifth)
        .into_iter()
        .find(|(_, a)| a["table"] == "pending_extraction")
        .map(|(_, a)| a)
        .expect("the bound turn leaves the queue");
    assert_eq!(
        cover["where"]["episode_id"]["in"],
        serde_json::json!(["ep-call2-u1"])
    );
}

#[test]
fn the_room_read_reaches_back_no_further_than_the_keepers_idle_window() {
    // The page is the ROOM, without the call's own session, and only as far
    // back as a generation can stay open: the keeper ends a generation that was
    // silent for its idle window (session-keeper `close`, `idle_ms`). Every
    // round of the room is on the page -- a newer turn of another round is how
    // the block learns its own round's generation was sealed -- and the round
    // is compared on the rows, never widened (see the round test below).
    let (bid, _) = park();
    let third = emit(store_echo("inline-bind", &bid, "select", call_page()));
    let (_, room) = episodes_select(&third).expect("room read");
    assert_eq!(room["where"]["channel"], CHANNEL);
    assert_eq!(
        room["where"]["session_id"],
        serde_json::json!({"neq": CALL_2}),
        "the call's own answers are not the conversation: {room}"
    );
    let cutoff = room["where"]["recorded_at"]["gte"]
        .as_str()
        .unwrap_or_else(|| panic!("the room read is bounded in time: {room}"))
        .to_string();
    let lo = ago(2 * 3600 + 60);
    let hi = ago(2 * 3600 - 60);
    assert!(
        lo < cutoff && cutoff < hi,
        "the bound is the keeper's idle window (two hours), got {cutoff}"
    );
    assert!(
        room["columns"]
            .as_array()
            .is_some_and(|c| c.iter().any(|x| x == "audience_set")),
        "the round of every row is read, so it can be compared: {room}"
    );
    assert_eq!(
        room["where"]["sender"],
        serde_json::json!({"in": ["user", "peer", "assistant"]}),
        "the same three roles as the session page: the answers stay the boundaries"
    );
    assert_eq!(room["order_by"][0]["dir"], "desc", "newest first");
    assert!(
        room["limit"].as_u64().unwrap_or(0) > 1,
        "a page, as the session read"
    );
}

#[test]
fn the_idle_window_is_the_keepers_own() {
    // One number, three spellings, one owner: the keeper decides when a
    // generation ends, and the bind must not reach further back than that.
    let glue: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(GLUE_CONFIG).unwrap()).unwrap();
    let keeper: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(KEEPER_CLOSE_CONFIG).unwrap()).unwrap();
    let idle = keeper["params"]["idle_ms"]
        .as_u64()
        .expect("keeper idle_ms");
    assert_eq!(glue["params"]["generation_idle_ms"].as_u64(), Some(idle));
    assert_eq!(
        glue["contract"]["settings"]["generation_idle_ms"]["default"].as_u64(),
        Some(idle)
    );
    assert!(
        glue_script().contains(&format!("_int(\"generation_idle_ms\", {idle})")),
        "the script's literal fallback is the keeper's value"
    );
}

#[test]
fn a_turn_of_an_old_closed_generation_is_never_bound() {
    // THE LOCK of review I1. The morning's first call: the keeper has opened a
    // new generation, but its first user turn is not written yet when the
    // delegation binds. What the room still holds is the evening before -- a
    // generation the night sealed after two hours of silence. Bound there, the
    // fact would inherit a turn nine hours old and a session that is closed.
    let (bid, _) = park();
    let _ = emit(store_echo("inline-bind", &bid, "select", call_page()));
    let page = serde_json::json!([
        row("ep-deleg", CALL_2, "assistant", 1),
        row("ep-old-a", OLD_GENERATION, "assistant", 9 * 3600),
        row("ep-old-u", OLD_GENERATION, "user", 9 * 3600 + 8),
    ]);
    let msgs = emit(store_echo("inline-bind-room", &bid, "select", page));
    assert!(
        rejected(&msgs),
        "a turn of a closed generation binds nothing: {msgs:?}"
    );
    assert!(
        store_ops(&msgs).is_empty(),
        "write-free, no further read: {msgs:?}"
    );
}

#[test]
fn a_generation_sealed_by_another_round_is_never_bound() {
    // The other way a generation ends: a turn of another round on the line.
    // Rita alone, then Sam joins for a few minutes -- the keeper seals Rita's
    // generation -- and a block of Rita's round arrives before the turn that
    // opens her NEW generation is written. Her old turns are minutes old and of
    // her own round, and still closed.
    let (bid, _) = park();
    let _ = emit(store_echo("inline-bind", &bid, "select", call_page()));
    let page = serde_json::json!([
        row("ep-deleg", CALL_2, "assistant", 1),
        row_in("ep-sam-a", "phone-b", "assistant", 30, OTHER_ROUND_STORED),
        row_in("ep-sam-u", "phone-b", "user", 31, OTHER_ROUND_STORED),
        row("ep-rita-a", GENERATION, "assistant", 300),
        row("ep-rita-u", GENERATION, "user", 301),
    ]);
    let msgs = emit(store_echo("inline-bind-room", &bid, "select", page));
    assert!(
        rejected(&msgs),
        "the room moved on to another round, the block's generation is sealed: {msgs:?}"
    );
    assert!(store_ops(&msgs).is_empty(), "{msgs:?}");
}

#[test]
fn only_the_open_generation_is_counted() {
    // Rows past the window are not the open generation, even when a store
    // hands them back: the bind compares the time itself as well.
    let (bid, _) = park();
    let _ = emit(store_echo("inline-bind", &bid, "select", call_page()));
    let page = serde_json::json!([
        row("ep-gen-a", GENERATION, "assistant", 3),
        row("ep-gen-u", GENERATION, "user", 4),
        row("ep-old-u", OLD_GENERATION, "user", 3 * 3600),
    ]);
    let msgs = emit(store_echo("inline-bind-room", &bid, "select", page));
    let carried = msgs
        .iter()
        .find(|m| m["header"]["phase"] == "inline-apply")
        .and_then(|m| m["header"]["batch_id"].as_str())
        .unwrap_or_else(|| panic!("bound in the open generation: {msgs:?}"))
        .to_string();
    assert_eq!(carried, format!("{bid}|ep-gen-u|{GENERATION}"));
}

#[test]
fn an_empty_session_page_also_falls_back_to_the_room() {
    // The delegation's own answer is written concurrently with this bind and
    // may not have landed: the session page is then empty, and the room is
    // still where the person's turn is.
    let (bid, _) = park();
    let third = emit(store_echo(
        "inline-bind",
        &bid,
        "select",
        serde_json::json!([]),
    ));
    assert!(!rejected(&third), "{third:?}");
    let (phase, _) = episodes_select(&third).expect("room read");
    assert_eq!(phase, "inline-bind-room");
}

#[test]
fn a_room_without_a_turn_of_the_person_still_binds_nothing() {
    // The fallback is read once. A room page without a user/peer turn is the
    // old safe direction: write-free, the reject port, no third read.
    let (bid, _) = park();
    let _ = emit(store_echo("inline-bind", &bid, "select", call_page()));
    let msgs = emit(store_echo("inline-bind-room", &bid, "select", call_page()));
    assert!(
        rejected(&msgs),
        "unbindable leaves through the reject port: {msgs:?}"
    );
    assert!(
        store_ops(&msgs).is_empty(),
        "and writes nothing, nor reads again: {msgs:?}"
    );
}

#[test]
fn a_session_page_with_a_turn_of_the_person_binds_without_the_room() {
    // Every channel whose turns and answers share one session (Telegram, the
    // chat surface, a talky turn of a phone call) keeps the old road byte for
    // byte: the session page binds, no second read.
    let (bid, _) = park();
    let page = serde_json::json!([row("ep-own", CALL_2, "user", 2)]);
    let third = emit(store_echo("inline-bind", &bid, "select", page));
    assert!(episodes_select(&third).is_none(), "no room read: {third:?}");
    let carried = third
        .iter()
        .find(|m| m["header"]["phase"] == "inline-apply")
        .and_then(|m| m["header"]["batch_id"].as_str())
        .expect("bound on the session page");
    assert_eq!(carried, format!("{bid}|ep-own"));
}

#[test]
fn a_topic_a_delegation_opens_lands_under_the_generation_it_binds_to() {
    // Review finding A of the first build: the movement of the conversation was
    // written under `context.session_id` -- the call id -- while the turn it
    // opened on lives in the generation. The keeper's close of the generation
    // asks for the topics of THAT session, so a topic under the call id was
    // never seen again. It now takes the session of the turn it hangs on.
    let args = serde_json::json!({
        "facts": [],
        "topic": {"movement": "start", "name": "the tomcat Moritz"}
    })
    .to_string();
    let first = emit(sidecar_block(&args));
    let bid = batch_id_of(&first);
    let payload = store_ops(&first)
        .into_iter()
        .find(|(_, a)| a["table"] == "scratch" && a["row"]["kind"] == "inline")
        .map(|(_, a)| a["row"]["payload"].as_str().expect("parked").to_string())
        .expect("a topic-only block is parked as well");
    let _ = emit(store_echo(
        "inline-turn",
        &bid,
        "insert",
        serde_json::json!([]),
    ));
    let _ = emit(store_echo("inline-bind", &bid, "select", call_page()));
    let fourth = emit(store_echo("inline-bind-room", &bid, "select", room_page()));
    let carried = fourth
        .iter()
        .find(|m| m["header"]["phase"] == "inline-apply")
        .and_then(|m| m["header"]["batch_id"].as_str())
        .unwrap_or_else(|| panic!("bound: {fourth:?}"))
        .to_string();
    let fifth = emit(store_echo(
        "inline-apply",
        &carried,
        "select",
        serde_json::json!([{"key": bid, "kind": "inline", "payload": payload}]),
    ));
    let topic = store_ops(&fifth)
        .into_iter()
        .find(|(_, a)| a["table"] == "topics")
        .map(|(_, a)| a)
        .unwrap_or_else(|| panic!("the movement is written: {fifth:?}"));
    assert_eq!(topic["row"]["session_id"], GENERATION, "{topic}");
    assert_eq!(topic["row"]["opened_episode_id"], "ep-call2-u1");
}

#[test]
fn a_topic_on_the_old_road_keeps_the_session_it_travelled_in() {
    // The session page bound: the turn's session IS the block's, and the key
    // carries nothing new -- the old road byte for byte.
    let args = serde_json::json!({
        "facts": [],
        "topic": {"movement": "start", "name": "the tomcat Moritz"}
    })
    .to_string();
    let first = emit(sidecar_block(&args));
    let bid = batch_id_of(&first);
    let payload = store_ops(&first)
        .into_iter()
        .find(|(_, a)| a["table"] == "scratch" && a["row"]["kind"] == "inline")
        .map(|(_, a)| a["row"]["payload"].as_str().unwrap().to_string())
        .unwrap();
    let page = serde_json::json!([row("ep-own", CALL_2, "user", 2)]);
    let third = emit(store_echo("inline-bind", &bid, "select", page));
    let carried = third
        .iter()
        .find(|m| m["header"]["phase"] == "inline-apply")
        .and_then(|m| m["header"]["batch_id"].as_str())
        .unwrap()
        .to_string();
    assert_eq!(carried, format!("{bid}|ep-own"));
    let fourth = emit(store_echo(
        "inline-apply",
        &carried,
        "select",
        serde_json::json!([{"key": bid, "kind": "inline", "payload": payload}]),
    ));
    let topic = store_ops(&fourth)
        .into_iter()
        .find(|(_, a)| a["table"] == "topics")
        .map(|(_, a)| a)
        .unwrap_or_else(|| panic!("{fourth:?}"));
    assert_eq!(topic["row"]["session_id"], CALL_2);
}

#[test]
fn a_close_pass_block_never_reads_the_room() {
    // Review M2: for a close-pass block the context's round is the round that
    // REQUESTED the close, and that round is never a source. A close-pass
    // block names its turns; should one ever reach the tool form, the room
    // fallback stays shut and the old reject stands.
    CLOSE_PASS.with(|f| f.set(true));
    let first = emit(sidecar_block(&one_fact_args()));
    let bid = batch_id_of(&first);
    let third = emit(store_echo("inline-bind", &bid, "select", call_page()));
    CLOSE_PASS.with(|f| f.set(false));
    assert!(
        episodes_select(&third).is_none(),
        "no room read for a close pass: {third:?}"
    );
    assert!(rejected(&third), "{third:?}");
}
