//! meclaw-os -- the drain adapter `memory-drain` (GitHub #101), script level.
//!
//! The memory hive writes one episode per turn (hive spec B.3/D.2), the
//! collector hands a closed session out as ONE batch (C3). Nothing between the
//! two speaks both forms, so a closed day never reached memory. This template is
//! that translation, and it lives OUTSIDE the hive on purpose (R-MD-1): the
//! writer port is unchanged, the stall stays shut, the P15 invariance gate is
//! untouched.
//!
//! Three claims are pinned here, one per group:
//!
//! 1. DECOMPOSITION -- one batch becomes N single-turn episodes in the order of
//!    the day, and nothing is judged, merged or dropped on the way.
//! 2. DETERMINISTIC IDENTITY -- the id an episode travels under is minted from
//!    `session_id` + turn index, never from a fresh uuid, so the same batch
//!    produces the same ids in every run (R-MD-2 no. 2).
//! 3. SELECT BEFORE INSERT -- what was already drained is read out of the
//!    adapter's own ledger BEFORE anything is fired, and an already drained
//!    turn is skipped rather than written twice.
//!
//! Everything runs the shipped `params.script_inline` against real stdin
//! documents, so nothing is mocked and nothing is spent.

use std::io::Write;
use std::process::{Command, Stdio};

use meclaw_core::serde_json::{Value, json};

const DRAIN_CONFIG: &str = "../../templates/memory-drain/drain/config.json";

/// `${VAR:-default}` becomes the default (or the override, when the case names
/// one) -- the same substitution the colony performs at boot.
fn resolve_vars(script: &str, over: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail
            .find('}')
            .expect("unterminated ${...} in script_inline");
        let inner = &tail[..end];
        let (name, default) = match inner.split_once(":-") {
            Some((n, d)) => (n, d),
            None => (inner, ""),
        };
        let value = over
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| *v)
            .unwrap_or(default);
        out.push_str(value);
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn drain_script(over: &[(&str, &str)]) -> String {
    let raw = std::fs::read_to_string(DRAIN_CONFIG).expect("memory-drain drain config");
    let v: Value = meclaw_core::serde_json::from_str(&raw).expect("config json");
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
        over,
    )
}

/// Runs the real script against a real stdin document and returns the emitted
/// messages (an empty multi-send is an empty vector).
fn emit_with(over: &[(&str, &str)], doc: Value) -> Vec<Value> {
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(drain_script(over))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(&meclaw_testing::code_stdin_bytes(&doc))
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "drain exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = meclaw_core::serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "drain stdout is not json ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    match v {
        Value::Array(a) => a,
        other => vec![other],
    }
}

fn emit(doc: Value) -> Vec<Value> {
    emit_with(&[], doc)
}

/// The close batch as the collector emits it (C3 receipt): `messages[]` is the
/// whole day, `rounds` travels next to it, `hop` carries session and counts.
fn batch(session: &str, turns: &[(&str, &str)]) -> Value {
    let msgs: Vec<Value> = turns
        .iter()
        .map(|(origin, text)| json!({"origin": origin, "type": "text", "text": text}))
        .collect();
    json!({
        "header": {
            "hop": {"route": "in_batch", "session_id": session,
                    "turn_count": turns.len().to_string(), "round_count": "0"},
            "context": {"session_id": session}
        },
        "messages": msgs,
        "rounds": []
    })
}

/// The store's reply to an insert: the operation on the hop, the phase in the
/// context (the internal edge promoted it).
fn insert_reply(session: &str, phase: &str) -> Value {
    json!({
        "header": {
            "hop": {"operation": "insert", "rows_affected": 1},
            "context": {"session_id": session, "drain_phase": phase,
                        "drain_origin": "drain"}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "d-park",
                      "text": "{\"rows_affected\": 1}"}]
    })
}

/// The ledger's reply to the probe select: the parked day plus every mark this
/// session left behind, exactly as the store hands rows back.
fn select_reply(session: &str, rows: Value) -> Value {
    json!({
        "header": {
            "hop": {"operation": "select", "rows_affected": rows.as_array().map_or(0, Vec::len)},
            "context": {"session_id": session, "drain_phase": "probe",
                        "drain_origin": "drain"}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "d-probe",
                      "text": rows.to_string()}]
    })
}

/// A parked-day row of the ledger, as the drain wrote it.
fn batch_row(id: &str, turns: &[(&str, &str)]) -> Value {
    let payload: Vec<Value> = turns
        .iter()
        .map(|(origin, text)| json!({"origin": origin, "text": text, "happened_at": ""}))
        .collect();
    json!({"id": id, "kind": "batch", "payload": Value::Array(payload).to_string(),
           "drained_upto": 0})
}

fn mark_row(id: &str, upto: u64) -> Value {
    json!({"id": id, "kind": "mark", "payload": "", "drained_upto": upto})
}

fn hop(m: &Value, key: &str) -> String {
    m["header"][key].as_str().unwrap_or_default().to_string()
}

/// The store args of an emission that goes to the ledger.
fn args(m: &Value) -> Value {
    let text = m["messages"][0]["text"].as_str().expect("tool_call text");
    meclaw_core::serde_json::from_str(text).expect("store args are json")
}

// ============================================================== DECOMPOSITION

/// The batch cannot be turned into episodes in the hop it arrives on: the
/// script keeps no state between hops, and what was already drained lives in
/// the ledger. So the day is parked first -- the same idiom the collector's
/// own close lane uses for exactly the same reason.
#[test]
fn the_batch_is_parked_before_anything_is_asked_or_fired() {
    let out = emit(batch(
        "s1",
        &[("user", "my editor is helix"), ("assistant", "noted")],
    ));
    assert_eq!(out.len(), 1, "one emission: the park insert -- {out:?}");
    assert_eq!(hop(&out[0], "route"), "lstore");
    assert_eq!(hop(&out[0], "phase"), "park");
    assert_eq!(hop(&out[0], "session_id"), "s1");

    let a = args(&out[0]);
    assert_eq!(a["operation"], "insert");
    assert_eq!(a["table"], "drain_log");
    assert_eq!(a["row"]["kind"], "batch");
    assert_eq!(a["row"]["session_id"], "s1");
    assert_eq!(a["row"]["drained_upto"], 0);

    let parked: Value = meclaw_core::serde_json::from_str(
        a["row"]["payload"].as_str().expect("payload is a string"),
    )
    .expect("payload is json");
    assert_eq!(
        parked,
        json!([
            {"origin": "user", "text": "my editor is helix", "happened_at": ""},
            {"origin": "assistant", "text": "noted", "happened_at": ""}
        ]),
        "the day travels whole, in the order it happened"
    );
}

/// A batch without a single conversational turn is not an error and not an
/// empty drain -- it is nothing at all. Terminal, like every other dead end in
/// this family of cells.
#[test]
fn a_batch_without_turns_emits_nothing_at_all() {
    assert!(emit(batch("s1", &[])).is_empty());
}

/// The store's own reply echo is not a batch. It carries no route, so the lane
/// guard never sees it as one.
#[test]
fn the_park_echo_is_never_mistaken_for_a_batch() {
    let out = emit(insert_reply("s1", "park"));
    assert!(
        out.iter()
            .all(|m| hop(m, "route") != "lstore" || hop(m, "phase") != "park"),
        "the echo of a park must not park again -- {out:?}"
    );
}

// =========================================================== SELECT BEFORE INSERT

/// The parked day is read back TOGETHER with everything this session already
/// left behind -- one table, one select, one reply. That read is the whole
/// select-before-insert: nothing is fired before the adapter knows what it
/// already fired.
#[test]
fn the_parked_day_is_read_back_together_with_what_was_already_drained() {
    let out = emit(insert_reply("s1", "park"));
    assert_eq!(out.len(), 1, "one emission: the probe select -- {out:?}");
    assert_eq!(hop(&out[0], "route"), "lstore");
    assert_eq!(hop(&out[0], "phase"), "probe");

    let a = args(&out[0]);
    assert_eq!(a["operation"], "select");
    assert_eq!(a["table"], "drain_log");
    assert_eq!(
        a["where"],
        json!({"session_id": "s1"}),
        "the session is the scope -- a drain never reads another session's marks"
    );
    assert_eq!(
        a["order_by"],
        json!([{"col": "id", "dir": "asc"}]),
        "ids are time-ordered, so the NEWEST parked day is the last row"
    );
    assert!(
        a.get("limit").is_none(),
        "a session's ledger is read whole; a limit would silently forget a mark"
    );
}

/// The messages of an emission, as the writer will read them.
fn turn_of(m: &Value) -> (String, String) {
    (
        m["messages"][0]["origin"]
            .as_str()
            .unwrap_or("")
            .to_string(),
        m["messages"][0]["text"].as_str().unwrap_or("").to_string(),
    )
}

fn episodes(out: &[Value]) -> Vec<&Value> {
    out.iter()
        .filter(|m| hop(m, "route") == "episode")
        .collect()
}

/// One batch, N episodes, in the order of the day -- and every one of them is
/// a SINGLE turn, because that is the only form the hive's writer reads (it
/// takes the first user/assistant text turn it finds).
#[test]
fn one_batch_becomes_one_episode_per_turn_in_the_order_of_the_day() {
    let day = [
        ("user", "my editor is helix"),
        ("assistant", "noted"),
        ("user", "and i cook keto"),
        ("assistant", "noted that too"),
    ];
    let out = emit(select_reply(
        "s1",
        json!([batch_row("2026-08-14-aa", &day)]),
    ));
    let eps = episodes(&out);

    assert_eq!(eps.len(), 4, "four turns, four episodes -- {out:?}");
    for (i, (origin, text)) in day.iter().enumerate() {
        assert_eq!(
            turn_of(eps[i]),
            (origin.to_string(), text.to_string()),
            "episode {i} is the {i}th turn of the day"
        );
        assert_eq!(
            eps[i]["messages"].as_array().map(Vec::len),
            Some(1),
            "an episode carries exactly ONE turn"
        );
    }
}

/// R-MD-2 no. 2, the half this adapter owns: the id an episode travels under
/// is a function of the session and the turn index -- nothing else. Two runs
/// over the same batch mint the same ids, which is the only reason a second
/// delivery can be recognised at all.
#[test]
fn the_episode_id_is_minted_from_the_session_and_the_turn_index() {
    let day = [("user", "a"), ("assistant", "b"), ("user", "c")];
    let first = emit(select_reply(
        "s7",
        json!([batch_row("2026-08-14-aa", &day)]),
    ));
    let second = emit(select_reply(
        "s7",
        json!([batch_row("2026-08-14-aa", &day)]),
    ));

    let ids = |out: &[Value]| -> Vec<String> {
        episodes(out).iter().map(|m| hop(m, "turn_id")).collect()
    };
    assert_eq!(ids(&first), vec!["s7#0", "s7#1", "s7#2"]);
    assert_eq!(
        ids(&first),
        ids(&second),
        "the same batch mints the same ids in every run -- no uuid per run"
    );
    let indexes: Vec<String> = episodes(&first)
        .iter()
        .map(|m| hop(m, "turn_index"))
        .collect();
    assert_eq!(
        indexes,
        vec!["0", "1", "2"],
        "the index travels readably too"
    );
    for m in episodes(&first) {
        assert_eq!(hop(m, "session_id"), "s7");
        assert_eq!(
            hop(m, "happened_at"),
            "",
            "the collector's batch carries no event time; the key is present and empty \
             so the port edge's set_context cannot fail"
        );
    }
}

/// The same emission that fires the episodes says how far the session is
/// drained. One row, one number -- and it is the number the NEXT probe reads.
#[test]
fn the_drain_marks_how_far_the_session_has_been_drained() {
    let day = [("user", "a"), ("assistant", "b"), ("user", "c")];
    let out = emit(select_reply(
        "s1",
        json!([batch_row("2026-08-14-aa", &day)]),
    ));
    let marks: Vec<&Value> = out.iter().filter(|m| hop(m, "route") == "lstore").collect();

    assert_eq!(marks.len(), 1, "exactly one mark -- {out:?}");
    assert_eq!(hop(marks[0], "phase"), "mark");
    let a = args(marks[0]);
    assert_eq!(a["operation"], "insert");
    assert_eq!(a["table"], "drain_log");
    assert_eq!(a["row"]["kind"], "mark");
    assert_eq!(a["row"]["session_id"], "s1");
    assert_eq!(a["row"]["drained_upto"], 3, "all three turns are through");
}

/// The mark's own echo is the end of the chain. Nothing follows it, so nothing
/// dead-letters and nothing loops.
#[test]
fn the_mark_echo_ends_the_chain() {
    assert!(emit(insert_reply("s1", "mark")).is_empty());
}

// =============================================================== IDEMPOTENCE

/// The gate of R-MD-2 no. 2 at script level: the mark says the whole session
/// is through, so the second delivery of the same batch fires NOTHING -- no
/// episode, and not even a second mark. Skip, not a double insert.
#[test]
fn a_session_that_is_already_drained_fires_nothing_at_all() {
    let day = [("user", "a"), ("assistant", "b"), ("user", "c")];
    let out = emit(select_reply(
        "s1",
        json!([
            batch_row("2026-08-14-aa", &day),
            mark_row("2026-08-14-bb", 3)
        ]),
    ));
    assert!(
        out.is_empty(),
        "a drained session is terminal, not a second write -- {out:?}"
    );
}

/// A session that grew since the last drain is drained from where it stopped.
/// The indexes continue rather than restart, so an episode keeps the id it had
/// the first time -- the ledger's number and the minted id are the same
/// arithmetic.
#[test]
fn a_grown_session_is_drained_from_where_it_stopped() {
    let day = [
        ("user", "a"),
        ("assistant", "b"),
        ("user", "c"),
        ("assistant", "d"),
        ("user", "e"),
    ];
    let out = emit(select_reply(
        "s1",
        json!([
            batch_row("2026-08-14-aa", &day),
            mark_row("2026-08-14-bb", 3)
        ]),
    ));
    let eps = episodes(&out);
    assert_eq!(eps.len(), 2, "two new turns, two episodes -- {out:?}");
    assert_eq!(hop(eps[0], "turn_id"), "s1#3");
    assert_eq!(hop(eps[1], "turn_id"), "s1#4");
    assert_eq!(turn_of(eps[0]).1, "d");
    assert_eq!(turn_of(eps[1]).1, "e");

    let marks: Vec<&Value> = out.iter().filter(|m| hop(m, "route") == "lstore").collect();
    assert_eq!(args(marks[0])["row"]["drained_upto"], 5);
}

/// Marks accumulate; the drain trusts the HIGHEST, not the newest row it
/// happens to read last. An out-of-order reply must not un-drain a session.
#[test]
fn the_highest_mark_wins_whatever_order_the_rows_arrive_in() {
    let day = [("user", "a"), ("assistant", "b"), ("user", "c")];
    let out = emit(select_reply(
        "s1",
        json!([
            mark_row("2026-08-14-bb", 3),
            batch_row("2026-08-14-aa", &day),
            mark_row("2026-08-14-ab", 2)
        ]),
    ));
    assert!(out.is_empty(), "3 of 3 is still 3 of 3 -- {out:?}");
}

/// The newest parked day wins: a second close of the same session parks a
/// second row, and the drain must decompose the LATER one (it is the superset,
/// the earlier one is a prefix of it).
#[test]
fn the_newest_parked_day_is_the_one_that_is_decomposed() {
    let first = [("user", "a"), ("assistant", "b")];
    let second = [("user", "a"), ("assistant", "b"), ("user", "c")];
    let out = emit(select_reply(
        "s1",
        json!([
            batch_row("2026-08-14-aa", &first),
            mark_row("2026-08-14-ab", 2),
            batch_row("2026-08-14-ac", &second)
        ]),
    ));
    let eps = episodes(&out);
    assert_eq!(eps.len(), 1, "only the third turn is new -- {out:?}");
    assert_eq!(hop(eps[0], "turn_id"), "s1#2");
    assert_eq!(turn_of(eps[0]).1, "c");
}

/// A reply the drain cannot read is not a reason to write a random amount of
/// episodes. An unparsable payload drains nothing.
#[test]
fn an_unreadable_parked_day_drains_nothing() {
    let out = emit(select_reply(
        "s1",
        json!([{"id": "2026-08-14-aa", "kind": "batch", "payload": "{not json",
                "drained_upto": 0}]),
    ));
    assert!(out.is_empty(), "no day, no episodes -- {out:?}");
}

/// The event time is the caller's, never invented: a batch that carries one
/// per turn hands it on, so a historical replay keeps the bi-temporal split
/// (the writer stamps `recorded_at` from its own clock either way).
#[test]
fn a_batch_that_knows_its_event_times_hands_them_on() {
    let doc = json!({
        "header": {"hop": {"route": "in_batch", "session_id": "s1"},
                   "context": {"session_id": "s1"}},
        "messages": [{"origin": "user", "type": "text", "text": "a",
                      "happened_at": "2020-01-01T00:00:00.000000Z"}]
    });
    let parked: Value = meclaw_core::serde_json::from_str(
        args(&emit(doc)[0])["row"]["payload"]
            .as_str()
            .expect("payload"),
    )
    .expect("payload json");
    assert_eq!(parked[0]["happened_at"], "2020-01-01T00:00:00.000000Z");

    let rows = json!([{"id": "2026-08-14-aa", "kind": "batch",
                       "payload": parked.to_string(), "drained_upto": 0}]);
    let out = emit(select_reply("s1", rows));
    assert_eq!(
        hop(episodes(&out)[0], "happened_at"),
        "2020-01-01T00:00:00.000000Z"
    );
}
