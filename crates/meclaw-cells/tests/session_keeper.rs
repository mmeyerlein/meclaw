//! meclaw-os -- the session keeper: a session is a channel GENERATION.
//!
//! A session here is modelled on a phone call: it begins on a channel, it ends
//! on that channel, and the fluid transition in between belongs to it. Three
//! claims are pinned in this file, one per group:
//!
//! 1. THE STAMP -- every inbound turn of a channel carries the same session id
//!    until the session ends. The id is minted at the surface, once, and the
//!    rest of the tree only consumes it.
//! 2. THE CLOSE -- a session ends by ARITHMETIC, not by judgement: a nightly
//!    timer plus an idle threshold. No counselor, no model, no "is the
//!    conversation over?" call.
//! 3. THE NEW GENERATION -- reopening is lazy. Nothing pre-creates a session;
//!    the next turn after a close opens the next generation by itself.
//!
//! Everything runs the shipped `params.script_inline` against real stdin
//! documents, so nothing is mocked and nothing is spent.

use std::io::Write;
use std::process::{Command, Stdio};

const TEMPLATE: &str = "../../templates/session-keeper";

fn config_of(rel: &str) -> serde_json::Value {
    let raw = std::fs::read_to_string(format!("{TEMPLATE}/{rel}")).expect("template config");
    serde_json::from_str(&raw).expect("config json")
}

/// The shipped script, verbatim.
///
/// There is nothing left to substitute: since `session-keeper@2.2.0` the two
/// knobs of `./close` are params of that cell rather than substitution tokens
/// (GH #138), so a case that wants a different idle window hands one down on the
/// stdin document's `params` object -- the same object an `override_params`
/// entry fills at instantiation.
fn script_of(cell: &str) -> String {
    config_of(&format!("{cell}/config.json"))["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string()
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

/// Run the real script against a real stdin document and return the emitted
/// messages.
fn emit_with(
    cell: &str,
    params: serde_json::Value,
    mut doc: serde_json::Value,
) -> Vec<serde_json::Value> {
    doc["params"] = params;
    let out = run_script_on_stdin(
        &script_of(cell),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    assert!(
        out.status.success(),
        "{cell} exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

fn stamp(doc: serde_json::Value) -> Vec<serde_json::Value> {
    emit_with("stamp", serde_json::json!({}), doc)
}

/// The store args of an emitted `kstore` message.
fn op_of(msg: &serde_json::Value) -> serde_json::Value {
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    serde_json::from_str(text).expect("op json")
}

// ================================================================= THE SURFACE

#[test]
fn the_session_row_is_one_generation_of_one_channel() {
    // The store is the whole memory of the keeper: which channel is in which
    // generation, when it last spoke, and whether that generation is over. No
    // conversation content -- the turns belong to the collector's window.
    let sessions = config_of("sessions/config.json");
    assert_eq!(sessions["cell"]["type"], "store");
    let cols = &sessions["params"]["schema"]["sessions"];
    for (col, ty) in [
        ("channel", "text"),
        ("session_id", "text"),
        ("opened_at", "text"),
        ("last_seen", "text"),
        ("closed", "int"),
        ("closed_at", "text"),
    ] {
        assert_eq!(cols[col], ty, "sessions.{col} is the {ty} column");
    }
    // Timestamps are compared, never parsed, in the close pass: a fixed-width
    // UTC stamp orders lexicographically, so "older than the cutoff" is a
    // store-side `lt` and not arithmetic on strings.
    assert_eq!(
        cols["last_seen"], "text",
        "the idle cut is a lexicographic comparison"
    );
}

#[test]
fn the_hive_routes_both_code_cells_to_the_same_store() {
    // Two writers, one state surface. The reply finds its way home by
    // store_origin, exactly like the collector's assemble/window pair.
    let hive = config_of("config.json");
    assert_eq!(hive["cell"]["type"], "hive");
    assert!(
        hive.get("contract").is_none(),
        "a hive is a scope marker, not an actor"
    );
    let edges = hive["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .clone();
    let pair = |from: &str, to: &str| {
        assert!(
            edges.iter().any(|e| e["from"] == from && e["to"] == to),
            "edge {from} -> {to} missing"
        );
    };
    pair("./stamp", "./sessions");
    pair("./sessions", "./stamp");
    pair("./close", "./sessions");
    pair("./sessions", "./close");
    // And the two return lanes are told apart by origin, not by guesswork.
    let origins: Vec<String> = edges
        .iter()
        .filter(|e| e["from"] == "./sessions")
        .map(|e| e["condition"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        origins.iter().any(|c| c.contains("keeper-stamp"))
            && origins.iter().any(|c| c.contains("keeper-close")),
        "store replies are dispatched by store_origin: {origins:?}"
    );
    // The firing needs an edge, not just an emit_to: EVERY cell emission runs
    // through the out-edges of its sender, and one that matches none
    // dead-letters as no_route -- a source emission included.
    pair("./night", "./close");
    let firing_edge = edges
        .iter()
        .find(|e| e["from"] == "./night")
        .expect("night edge");
    assert!(
        firing_edge["condition"]
            .as_str()
            .unwrap_or_default()
            .contains("night-close"),
        "the edge carries the schedule the close pass listens for: {firing_edge}"
    );
}

#[test]
fn an_inbound_turn_asks_which_generation_the_channel_is_in() {
    let out = stamp(serde_json::json!({
        "header": {"context": {"channel": "tg:42"}, "hop": {"route": "in_turn"}},
        "messages": [{"origin": "user", "type": "text", "text": "hello"}]
    }));
    assert_eq!(out.len(), 1, "one lookup, nothing else");
    assert_eq!(out[0]["header"]["route"], "kstore");
    assert_eq!(out[0]["header"]["phase"], "look");
    let op = op_of(&out[0]);
    assert_eq!(op["operation"], "select");
    assert_eq!(op["table"], "sessions");
    assert_eq!(op["where"]["channel"], "tg:42", "sessions are per channel");
    assert_eq!(op["where"]["closed"], 0, "only an OPEN generation counts");
    assert_eq!(op["limit"], 1);
    // The turn itself rides through the lookup on the hop, because a store
    // reply carries the row and not the conversation that asked for it.
    let kept: serde_json::Value = serde_json::from_str(
        out[0]["header"]["keeper_body"]
            .as_str()
            .expect("keeper_body"),
    )
    .expect("kept body json");
    assert_eq!(kept["messages"][0]["text"], "hello");
}

/// The store reply as the hive's own edge delivers it back: the step in
/// context, the operation and the guard signal on the hop.
fn reply_doc(
    origin: &str,
    phase: &str,
    op: &str,
    rows_affected: i64,
    payload: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "header": {"context": {"channel": "tg:42", "ses_phase": phase,
                               "store_origin": origin},
                   "hop": {"operation": op, "rows_affected": rows_affected}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "x",
                      "text": payload.to_string()}]
    })
}

/// A `look` reply that carries a turn through, the way the hive edge promotes
/// it: `keeper_body` in context, the row (or no row) in the payload.
fn look_reply(rows: serde_json::Value, kept: serde_json::Value) -> serde_json::Value {
    let mut doc = reply_doc("keeper-stamp", "look", "select", 1, rows);
    doc["header"]["context"]["keeper_body"] = serde_json::json!(kept.to_string());
    doc
}

fn session_row(session_id: &str, opened_at: &str, last_seen: &str) -> serde_json::Value {
    serde_json::json!({"channel": "tg:42", "session_id": session_id,
                       "opened_at": opened_at, "last_seen": last_seen,
                       "closed": 0, "closed_at": ""})
}

fn turn_body(text: &str) -> serde_json::Value {
    serde_json::json!({"messages": [{"origin": "user", "type": "text", "text": text}]})
}

fn route(out: &[serde_json::Value], route: &str) -> serde_json::Value {
    out.iter()
        .find(|m| m["header"]["route"] == route)
        .unwrap_or_else(|| panic!("no emission on route {route}: {out:?}"))
        .clone()
}

#[test]
fn a_running_generation_keeps_its_id_and_restarts_the_idle_clock() {
    let out = stamp(look_reply(
        serde_json::json!([session_row("tg:42-0001", "0001", "0002")]),
        turn_body("and my shell is fish"),
    ));
    assert_eq!(out.len(), 2, "the touch and the way on, in one multi-send");

    let touch = op_of(&route(&out, "kstore"));
    assert_eq!(touch["operation"], "update");
    assert_eq!(touch["table"], "sessions");
    assert_eq!(touch["where"]["session_id"], "tg:42-0001");
    assert_eq!(
        touch["where"]["closed"], 0,
        "an already sealed generation is never touched again"
    );
    assert!(
        touch["set"]["last_seen"].as_str().unwrap_or_default() > "0002",
        "the idle clock restarts at this turn: {touch}"
    );
    assert!(
        touch["set"].get("session_id").is_none(),
        "the id of a running generation is never rewritten: {touch}"
    );

    let turn = route(&out, "turn");
    assert_eq!(
        turn["header"]["session_id"], "tg:42-0001",
        "turn 2 of a call carries the id of turn 1"
    );
    assert_eq!(turn["messages"][0]["text"], "and my shell is fish");
}

#[test]
fn a_channel_without_an_open_generation_opens_the_next_one_lazily() {
    // Nothing pre-creates a session -- not a boot, not a close, not a timer.
    // The next turn after the end of a call IS the beginning of the next one.
    let out = stamp(look_reply(serde_json::json!([]), turn_body("good morning")));
    assert_eq!(out.len(), 2);

    let open = op_of(&route(&out, "kstore"));
    assert_eq!(open["operation"], "insert");
    assert_eq!(open["table"], "sessions");
    assert_eq!(open["row"]["channel"], "tg:42");
    assert_eq!(open["row"]["closed"], 0);
    let minted = open["row"]["session_id"].as_str().expect("session_id");
    assert!(
        minted.starts_with("tg:42-"),
        "a session id names its channel and its birth: {minted}"
    );
    assert_eq!(
        open["row"]["opened_at"], open["row"]["last_seen"],
        "a fresh generation was last seen when it was born"
    );
    assert!(
        minted.ends_with(open["row"]["opened_at"].as_str().expect("opened_at")),
        "<channel>-<recorded_at>: {minted}"
    );

    let turn = route(&out, "turn");
    assert_eq!(turn["header"]["session_id"], minted);
    assert_eq!(turn["messages"][0]["text"], "good morning");
}

#[test]
fn the_turn_itself_travels_through_the_stamp_unchanged() {
    // The keeper is not an assembler: it adds an id to the envelope and keeps
    // its hands off the body. Every slot the surface sent arrives downstream.
    let kept = serde_json::json!({
        "system": {"identity": {"text": "egon"}},
        "messages": [{"origin": "user", "type": "text", "text": "one"},
                     {"origin": "user", "type": "text", "text": "two"}],
        "attachments": [{"blob_id": "b1", "mime": "image/png"}]
    });
    let out = stamp(look_reply(
        serde_json::json!([session_row("tg:42-0001", "0001", "0002")]),
        kept.clone(),
    ));
    let turn = route(&out, "turn");
    assert_eq!(turn["messages"], kept["messages"]);
    assert_eq!(turn["system"], kept["system"]);
    assert_eq!(
        turn["attachments"], kept["attachments"],
        "an undeclared slot is carried, not swallowed"
    );
}

#[test]
fn a_finished_step_is_terminal() {
    // The stamp sits in a loop with its own store. A reply to the write it just
    // made must not produce a second write, or the ingress feeds itself.
    for phase in ["touch", "open"] {
        let op = if phase == "touch" { "update" } else { "insert" };
        assert!(
            stamp(reply_doc(
                "keeper-stamp",
                phase,
                op,
                1,
                serde_json::json!("ok")
            ))
            .is_empty(),
            "the {phase} reply is the end of the chain"
        );
    }
    let stray = serde_json::json!({
        "header": {"context": {}, "hop": {}},
        "messages": [{"origin": "user", "type": "text", "text": "stray"}]
    });
    assert!(
        stamp(stray).is_empty(),
        "a message without a lane is parked"
    );
}

// =================================================================== THE CLOSE

fn close_with(params: serde_json::Value, doc: serde_json::Value) -> Vec<serde_json::Value> {
    emit_with("close", params, doc)
}

fn close(doc: serde_json::Value) -> Vec<serde_json::Value> {
    close_with(serde_json::json!({}), doc)
}

/// A firing as the timer delivers it: the auto headers of the schedule, no
/// context at all (an `emit_to` message is minted, not routed).
fn firing() -> serde_json::Value {
    serde_json::json!({
        "header": {"context": {},
                   "hop": {"event_id": "e1", "schedule_id": "s1",
                           "schedule_name": "night-close",
                           "scheduled_at": "2026-08-13T22:00:00Z",
                           "fired_at": "2026-08-13T22:00:00Z", "iteration_n": 0}},
        "messages": [{"origin": "user", "type": "text", "text": "night-close"}]
    })
}

fn seconds_back(cutoff: &str) -> i64 {
    let parsed = chrono::DateTime::parse_from_rfc3339(cutoff)
        .unwrap_or_else(|e| panic!("cutoff {cutoff} is not RFC-3339: {e}"));
    (chrono::Utc::now() - parsed.with_timezone(&chrono::Utc)).num_seconds()
}

#[test]
fn the_night_sweep_asks_only_for_the_channels_that_fell_silent() {
    let out = close(firing());
    assert_eq!(out.len(), 1, "one question, asked of the store");
    assert_eq!(out[0]["header"]["route"], "kstore");
    assert_eq!(out[0]["header"]["phase"], "sweep");
    let op = op_of(&out[0]);
    assert_eq!(op["operation"], "select");
    assert_eq!(op["table"], "sessions");
    assert_eq!(
        op["where"]["closed"], 0,
        "a sealed generation is not a candidate"
    );
    assert_eq!(op["limit"], 50, "the shipped `close_limit`");
    // The idle rule is arithmetic: the cutoff is now minus the threshold, and
    // "older than the cutoff" runs in the store as a lexicographic `lt`.
    let cutoff = op["where"]["last_seen"]["lt"].as_str().expect("lt cutoff");
    let back = seconds_back(cutoff);
    assert!(
        (7100..=7300).contains(&back),
        "the shipped `idle_ms` is two hours, got {back}s back"
    );

    let out = close_with(serde_json::json!({"idle_ms": 600000}), firing());
    let cutoff = op_of(&out[0])["where"]["last_seen"]["lt"]
        .as_str()
        .expect("lt cutoff")
        .to_string();
    let back = seconds_back(&cutoff);
    assert!(
        (500..=700).contains(&back),
        "the threshold is a knob, not a constant: {back}s back"
    );
}

#[test]
fn a_message_that_is_not_a_firing_sweeps_nothing() {
    // The close pass shares a store with the stamp and answers a timer. A
    // stray message must not start a sweep, or a busy channel gets closed at
    // noon because something echoed.
    let stray = serde_json::json!({
        "header": {"context": {}, "hop": {}},
        "messages": [{"origin": "user", "type": "text", "text": "hello?"}]
    });
    assert!(close(stray).is_empty());
}

#[test]
fn every_idle_generation_is_sealed_under_a_guard() {
    let rows = serde_json::json!([
        session_row("tg:42-0001", "0001", "0002"),
        {"channel": "tg:7", "session_id": "tg:7-0003", "opened_at": "0003",
         "last_seen": "0004", "closed": 0, "closed_at": ""}
    ]);
    let out = close(reply_doc("keeper-close", "sweep", "select", 2, rows));
    assert_eq!(out.len(), 2, "one seal per idle generation");
    for (msg, sid, ch) in [
        (&out[0], "tg:42-0001", "tg:42"),
        (&out[1], "tg:7-0003", "tg:7"),
    ] {
        assert_eq!(msg["header"]["route"], "kstore");
        assert_eq!(msg["header"]["phase"], "seal");
        assert_eq!(msg["header"]["session_id"], sid);
        assert_eq!(
            msg["header"]["channel"], ch,
            "each seal carries its own channel down its own chain"
        );
        let op = op_of(msg);
        assert_eq!(op["operation"], "update");
        assert_eq!(op["set"]["closed"], 1);
        assert!(
            op["set"]["closed_at"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "a generation records when it ended: {op}"
        );
        assert_eq!(op["where"]["session_id"], sid);
        assert_eq!(
            op["where"]["closed"], 0,
            "the guard: only an OPEN generation can be closed, and only once"
        );
    }
}

#[test]
fn a_sweep_that_finds_nobody_stays_silent() {
    // The normal night on a busy channel: the timer fires twelve times and the
    // colony sees nothing at all. A firing is a question, not an event.
    let out = close(reply_doc(
        "keeper-close",
        "sweep",
        "select",
        0,
        serde_json::json!([]),
    ));
    assert!(out.is_empty(), "no candidate, no emission: {out:?}");
}

#[test]
fn only_the_pass_that_won_the_guard_asks_for_the_close() {
    let mut lost = reply_doc("keeper-close", "seal", "update", 0, serde_json::json!("ok"));
    lost["header"]["context"]["keeper_session"] = serde_json::json!("tg:42-0001");
    assert!(
        close(lost).is_empty(),
        "rows_affected 0: this generation was already sealed, and a second \
         close request would ask the collector for the same batch twice"
    );

    let mut won = reply_doc("keeper-close", "seal", "update", 1, serde_json::json!("ok"));
    won["header"]["context"]["keeper_session"] = serde_json::json!("tg:42-0001");
    let out = close(won);
    assert_eq!(out.len(), 1, "exactly ONE close request per generation");
    assert_eq!(
        out[0]["header"]["route"], "close",
        "the port convention of the close lane"
    );
    assert_eq!(out[0]["header"]["session_id"], "tg:42-0001");
    assert_eq!(out[0]["header"]["channel"], "tg:42");
    assert_eq!(
        out[0]["messages"],
        serde_json::json!([]),
        "a close request carries no conversation turns -- the collector reads \
         the session out of its own store"
    );
}

// ══════════════════════════ THE ROUND A GENERATION WAS OPENED IN (GH #273)

/// The participant set of a round is declared at the door a turn enters by --
/// the door of the talky that holds the generation (ADR-0002 E8), where it is a
/// CONSTANT of that generation's lifetime: a change of the set ends the
/// generation. The keeper records it on the row at the moment the generation is
/// opened, because the night that ends it is a timer and knows nothing about
/// who was there.
///
/// It is recorded, never derived: the `session_id` prefix is a convention of
/// this template and no promise to anyone downstream, and a set that a door did
/// not declare stays empty rather than becoming `["*"]`.
#[test]
fn a_new_generation_records_the_round_it_was_opened_in() {
    let sessions = config_of("sessions/config.json");
    assert_eq!(
        sessions["params"]["schema"]["sessions"]["audience_set"], "text",
        "the row carries the participant set of its generation"
    );

    let mut doc = look_reply(serde_json::json!([]), turn_body("hello"));
    doc["header"]["context"]["audience_set"] =
        serde_json::json!(r#"["member:alex","agent:scribe"]"#);
    let out = stamp(doc);
    let op = op_of(&route(&out, "kstore"));
    assert_eq!(op["operation"], "insert");
    assert_eq!(
        op["row"]["audience_set"], r#"["member:alex","agent:scribe"]"#,
        "the round the door declared is what the row keeps: {op}"
    );
}

/// A door that declares no round leaves the column EMPTY. Nothing here invents
/// a participant set, and least of all the universal one -- a consumer that
/// needs the set refuses the batch visibly instead of writing a row that claims
/// everyone was present.
#[test]
fn a_generation_opened_without_a_round_records_an_empty_one() {
    let out = stamp(look_reply(serde_json::json!([]), turn_body("hello")));
    let op = op_of(&route(&out, "kstore"));
    assert_eq!(op["operation"], "insert");
    assert_eq!(op["row"]["audience_set"], "", "empty, not invented: {op}");
    assert!(
        !serde_json::to_string(&out)
            .unwrap_or_default()
            .contains("\"*\""),
        "no emission of the stamp carries a universal audience: {out:?}"
    );
}

/// Provenance is never rewritten (ADR-0002 E12). A turn arriving into a RUNNING
/// generation restarts the idle clock and touches nothing else -- the round is
/// a property of the generation, and a generation whose round changed would
/// have ended.
#[test]
fn a_running_generation_never_has_its_round_rewritten() {
    let mut doc = look_reply(
        serde_json::json!([session_row("tg:42-0001", "0001", "0002")]),
        turn_body("still here"),
    );
    doc["header"]["context"]["audience_set"] = serde_json::json!(r#"["member:mallory"]"#);
    let out = stamp(doc);
    let op = op_of(&route(&out, "kstore"));
    assert_eq!(op["operation"], "update");
    assert_eq!(
        op["set"].as_object().map(|o| o.len()),
        Some(1),
        "the touch writes the idle clock and nothing else: {op}"
    );
    assert!(op["set"]["last_seen"].is_string(), "{op}");
}

/// The sweep reads the round of every generation it seals, and carries it down
/// that generation's own chain -- the same way it already carries the channel.
/// Without it the seal reply, which is all the close request has, could not say
/// who was there.
#[test]
fn every_seal_carries_the_round_of_its_own_generation() {
    let mut a = session_row("tg:42-0001", "0001", "0002");
    a["audience_set"] = serde_json::json!(r#"["member:alex","agent:scribe"]"#);
    let mut b = serde_json::json!({"channel": "tg:7", "session_id": "tg:7-0003",
                                   "opened_at": "0003", "last_seen": "0004",
                                   "closed": 0, "closed_at": ""});
    b["audience_set"] = serde_json::json!(r#"["member:robin"]"#);

    // The sweep has to ASK for the column, or the rows come back without it.
    let asked = close(firing());
    let cols = op_of(&asked[0])["columns"].clone();
    assert!(
        cols.as_array()
            .is_some_and(|c| c.contains(&serde_json::json!("audience_set"))),
        "the sweep selects the round of its candidates: {cols}"
    );

    let out = close(reply_doc(
        "keeper-close",
        "sweep",
        "select",
        2,
        serde_json::json!([a, b]),
    ));
    assert_eq!(out.len(), 2);
    assert_eq!(
        out[0]["header"]["audience_set"],
        r#"["member:alex","agent:scribe"]"#
    );
    assert_eq!(out[1]["header"]["audience_set"], r#"["member:robin"]"#);
}

/// The close request names the room AND the round of the generation it ends.
/// Both come off the row, promoted back into context by the hive edge that sent
/// the seal, and both leave on the hop -- so the edge that consumes the close
/// has something to promote and does not have to guess.
#[test]
fn the_close_request_names_the_room_and_the_round_of_its_generation() {
    let mut won = reply_doc("keeper-close", "seal", "update", 1, serde_json::json!("ok"));
    won["header"]["context"]["keeper_session"] = serde_json::json!("tg:42-0001");
    won["header"]["context"]["keeper_audience"] =
        serde_json::json!(r#"["member:alex","agent:scribe"]"#);
    let out = close(won);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["header"]["route"], "close");
    assert_eq!(out[0]["header"]["channel"], "tg:42");
    assert_eq!(
        out[0]["header"]["audience_set"],
        r#"["member:alex","agent:scribe"]"#
    );
}

/// A generation whose row carries no round produces a close request with an
/// EMPTY one -- present as a key, empty as a value. Present, because a missing
/// hop key makes a CEL modifier fail and a failed modifier SKIPS the edge, so
/// the close would vanish instead of being refused. Empty, because nothing here
/// knows who was there.
#[test]
fn a_close_of_a_generation_without_a_round_says_so_rather_than_inventing_one() {
    let mut won = reply_doc("keeper-close", "seal", "update", 1, serde_json::json!("ok"));
    won["header"]["context"]["keeper_session"] = serde_json::json!("tg:42-0001");
    let out = close(won);
    assert_eq!(out.len(), 1);
    assert!(
        out[0]["header"].as_object().is_some_and(|h| h
            .get("audience_set")
            .is_some_and(|v| v == &serde_json::json!(""))),
        "the key is there and it is empty: {}",
        out[0]["header"]
    );
}

/// The hive promotes the round of a seal back into context, under a
/// keeper-local name -- the same shape as `keeper_session`. The name is local on
/// purpose: `context.audience_set` is the contract key of the CONSUMER, and it
/// is set by the edge that leaves this hive, not by an edge inside it.
#[test]
fn the_hive_carries_the_round_of_a_seal_down_its_own_chain() {
    let hive = config_of("config.json");
    let edge = hive["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .find(|e| e["from"] == "./close" && e["to"] == "./sessions")
        .expect("close -> sessions")
        .clone();
    assert_eq!(
        edge["modifier"]["set_context"]["keeper_audience"], "hop.audience_set",
        "the seal's round travels with its own chain: {edge}"
    );
}

// ==================================================================== THE NIGHT

#[test]
fn the_night_schedule_is_declared_in_utc_and_lands_on_the_local_night() {
    let night = config_of("night/config.json");
    assert_eq!(night["cell"]["type"], "timer");
    let sched = &night["params"]["schedules"][0];
    assert_eq!(
        sched["emit_to"], "../close",
        "the firing goes to the close pass of this hive"
    );
    assert_eq!(sched["schedule_name"], "night-close");

    // The timer computes in UTC -- always, everywhere, no zone parameter
    // exists. So the shipped default is the UTC IMAGE of the local night, and
    // the README does the sum for the other half of the year.
    // A literal since `session-keeper@2.2.0`, not a `${KEEPER_NIGHT_CRON:-…}`
    // token: the schedule is a param of this timer now (GH #138), which is what
    // makes it addressable by an `override_params` entry naming `schedules`.
    let cron = sched["cron"].as_str().expect("cron").to_string();
    let parser = croner::parser::CronParser::builder()
        .seconds(croner::parser::Seconds::Required)
        .build();
    let parsed = parser.parse(&cron).expect("6-field Quartz pattern");

    use chrono::{TimeZone, Utc};
    let occurrences = |from: chrono::DateTime<Utc>, n: usize| {
        let mut at = from;
        let mut out = Vec::new();
        for _ in 0..n {
            at = parsed
                .find_next_occurrence(&at, false)
                .expect("an occurrence exists");
            out.push(at);
        }
        out
    };

    // Berlin is UTC+2 in summer: 22:00 UTC IS midnight, local.
    let noon = Utc.with_ymd_and_hms(2026, 8, 13, 12, 0, 0).unwrap();
    let night_run = occurrences(noon, 13);
    assert_eq!(
        night_run[0],
        Utc.with_ymd_and_hms(2026, 8, 13, 22, 0, 0).unwrap(),
        "the night opens at 00:00 CEST = 22:00 UTC"
    );
    assert_eq!(
        night_run[1],
        Utc.with_ymd_and_hms(2026, 8, 13, 22, 30, 0).unwrap(),
        "every thirty minutes"
    );
    assert_eq!(
        night_run[11],
        Utc.with_ymd_and_hms(2026, 8, 14, 3, 30, 0).unwrap(),
        "the last firing is 05:30 CEST -- the window is 00:00 until 06:00"
    );
    assert_eq!(
        night_run[12],
        Utc.with_ymd_and_hms(2026, 8, 14, 22, 0, 0).unwrap(),
        "and then nothing until the next night: twelve firings, not a poll"
    );
}

#[test]
fn the_close_pass_is_arithmetic_and_never_deletes() {
    // R-OS-3: no counselor, no model, no "is the conversation over?" call --
    // and No-Delete all the way down. A closed generation keeps its row.
    let script = script_of("close");
    for forbidden in ["\"delete\"", "route\": \"brain", "llm"] {
        assert!(
            !script.contains(forbidden),
            "the close pass must not contain {forbidden}"
        );
    }
    let steps = vec![
        close(firing()),
        close(reply_doc(
            "keeper-close",
            "sweep",
            "select",
            1,
            serde_json::json!([session_row("tg:42-0001", "0001", "0002")]),
        )),
    ];
    for step in steps {
        for msg in step {
            if msg["header"]["route"] != "kstore" {
                continue;
            }
            let op = op_of(&msg);
            assert!(
                op["operation"] == "select" || op["operation"] == "update",
                "the ledger is read and sealed, never emptied: {op}"
            );
        }
    }
}

// ================================================================= IN A COLONY
//
// The script pins above ask what the two passes DO. This group asks the
// question the keeper exists for, and it asks it of a running colony: does a
// channel carry one identity across its turns, does the night end it, and does
// the next turn begin the next generation? Free by construction -- there is no
// model anywhere in these trees, only cells that report what they were given.

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// A fixed schedule id. `${uuid7:*}` is an INSTANTIATION-side substitution
/// (mutation, not bootstrap), so a tree written straight to disk has to carry a
/// real one -- and a fixed one is what lets the test trigger that schedule.
const SCHEDULE_ID: &str = "0190a3f2-0000-7000-8000-00000000c105";
/// Never during a test run: the shipped default is the real night, and a test
/// that boots at 22:30 UTC must not race a real firing.
const NEVER: &str = "0 0 0 1 1 *";

/// The shipped template, copied cell by cell: only `config.json` files travel,
/// so the tree under test IS the template and nothing else.
fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        if from.is_dir() {
            copy_cells(&from, &dst.join(entry.file_name()));
        } else if entry.file_name() == "config.json" {
            std::fs::copy(&from, dst.join("config.json")).unwrap();
        }
    }
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, serde_json::to_string_pretty(v).unwrap()).unwrap();
}

/// A `code` cell config with the contract the substrate validates against.
fn code_cell(script: &str, routes: &[&str]) -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 10000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": {"route": {"type": "string", "values": routes, "required": false}}
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in that exercises the keeper ports.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// Turns a harness message into an inbound turn. The lane is named by the PORT
/// EDGE, which is what makes this a port test and not a script test.
const PROBE: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
sys.stdout.write(json.dumps({"header": {"route": "turn"}, "messages": d.get("messages", [])}))
"#;

/// The stand-in for everything downstream of the stamp: it answers with the
/// session id it was handed, so a conversation's identity can be MEASURED.
const REPORT: &str = r#"
import sys, json
doc = json.load(sys.stdin)
d = doc["body"]
envelope = doc["envelope"]
ctx = (envelope.get("header") or {}).get("context") or {}
sys.stdout.write(json.dumps({"header": {"route": "report"},
                             "messages": [{"origin": "assistant", "type": "text",
                                           "text": str(ctx.get("session_id", ""))}]}))
"#;

/// The stand-in for the collector's `in_close` lane.
const CLOSED: &str = r#"
import sys, json
doc = json.load(sys.stdin)
d = doc["body"]
envelope = doc["envelope"]
ctx = (envelope.get("header") or {}).get("context") or {}
sys.stdout.write(json.dumps({"header": {"route": "closed"},
                             "messages": [{"origin": "assistant", "type": "text",
                                           "text": "closed:" + str(ctx.get("session_id", "")) +
                                                   "|" + str(ctx.get("channel", ""))}]}))
"#;

/// The port wiring a parent draws around the keeper: one ingress lane in, the
/// stamped turn out, the close request out.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": "./probe", "to": "./session-keeper",
         "condition": "hop.route == 'turn'",
         "modifier": {"set_hop": {"route": "'in_turn'"}}},
        {"from": "./session-keeper", "to": "./report",
         "condition": "hop.route == 'turn'",
         "modifier": {"set_context": {"session_id": "hop.session_id"}}},
        {"from": "./report", "to": "/sink"},
        {"from": "./session-keeper", "to": "./closed",
         "condition": "hop.route == 'close'",
         "modifier": {"set_context": {"session_id": "hop.session_id",
                                      "channel": "hop.channel"}}},
        {"from": "./closed", "to": "/park"}
    ]}}})
}

/// The tree, with `idle_ms` for `./close` or `None` for the shipped two hours.
///
/// That knob was an environment line here until GH #138. It is a param of
/// `./close` now, so such a line would be read by NOTHING -- and a sweep that
/// silently kept the shipped two hours would find no candidate, leaving this
/// test waiting for a close that cannot come. Patching the copied config is
/// exactly what an `override_params` entry does to a tree booted from disk: the
/// mutation door writes the same key into the same file
/// (`patch_and_substitute_config`).
fn build_tree(td: &tempfile::TempDir, idle_ms: Option<i64>) {
    let root = td.path();
    std::fs::write(root.join(".env"), "").unwrap();
    write(root, "main/config.json", &main_config());
    let template =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/session-keeper");
    copy_cells(&template, &root.join("main/session-keeper"));
    // Two patches, both about the clock rather than about behaviour.
    let night_path = root.join("main/session-keeper/night/config.json");
    let mut night: Value =
        serde_json::from_str(&std::fs::read_to_string(&night_path).unwrap()).unwrap();
    night["params"]["schedules"][0]["schedule_id"] = json!(SCHEDULE_ID);
    night["params"]["schedules"][0]["cron"] = json!(NEVER);
    std::fs::write(&night_path, serde_json::to_string_pretty(&night).unwrap()).unwrap();
    if let Some(ms) = idle_ms {
        let close_path = root.join("main/session-keeper/close/config.json");
        let mut close: Value =
            serde_json::from_str(&std::fs::read_to_string(&close_path).unwrap()).unwrap();
        close["params"]["idle_ms"] = json!(ms);
        std::fs::write(&close_path, serde_json::to_string_pretty(&close).unwrap()).unwrap();
    }
    write(root, "main/probe/config.json", &code_cell(PROBE, &["turn"]));
    write(
        root,
        "main/report/config.json",
        &code_cell(REPORT, &["report"]),
    );
    write(
        root,
        "main/closed/config.json",
        &code_cell(CLOSED, &["closed"]),
    );
}

async fn boot(
    td: &tempfile::TempDir,
) -> (
    ColonyHandle,
    mpsc::Receiver<Message>,
    mpsc::Receiver<Message>,
) {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("timer".to_string(), Arc::new(TimerCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(64);
    let (park_tx, park_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    h.spawn(Path::new("/park"), move || {
        CaptureCell::new(park_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    (h, sink_rx, park_rx)
}

fn turn(channel: &str, text: &str) -> Message {
    let mut ctx = serde_json::Map::new();
    ctx.insert("channel".into(), json!(channel));
    MessageBuilder::new(Path::new("/probe"))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .context(ctx)
        .ttl(64)
        .build()
}

/// The firing, as an operator or a test drives it: `trigger` runs the schedule
/// once, now, without changing its plan -- and a triggered run is not
/// distinguishable from a cron run (docs/cell-types.md § timer).
fn fire() -> Message {
    MessageBuilder::new(Path::new("/session-keeper/night"))
        .body(Body::Inline(
            json!({"messages": [], "op": "trigger", "schedule_id": SCHEDULE_ID}),
        ))
        .ttl(64)
        .build()
}

fn text_of(m: &Message) -> String {
    match &m.body {
        Body::Inline(v) => v["messages"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        Body::Blob(_) => panic!("inline expected"),
    }
}

async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

/// One turn in, the session id it was stamped with out.
async fn say(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>, ch: &str, text: &str) -> String {
    h.send(turn(ch, text)).await;
    let got = recv_bounded(rx)
        .await
        .unwrap_or_else(|| panic!("no report for turn {text:?}"));
    text_of(&got)
}

fn delivered_to(td: &tempfile::TempDir, path: &str) -> i64 {
    let conn = rusqlite::Connection::open(td.path().join("colony.db")).expect("colony.db");
    conn.query_row(
        "SELECT COUNT(*) FROM message_log WHERE to_path = ?1",
        [path],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

/// Waits until the close pass has been handed `n` messages. A firing produces
/// two: the trigger itself and the store's answer to the sweep. Once the second
/// has landed, the pass has SEEN its candidates and decided -- which is what
/// turns "nothing arrived at the port" from a race into a statement.
async fn await_close_pass(td: &tempfile::TempDir, n: i64) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if delivered_to(td, "/session-keeper/close") >= n {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the close pass saw {} of {n} messages within 30s",
            delivered_to(td, "/session-keeper/close")
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_turn_of_a_call_carries_the_same_session_id() {
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, None);
    let (h, mut sink_rx, _park_rx) = boot(&td).await;

    let a1 = say(&h, &mut sink_rx, "tg:42", "my editor is helix").await;
    let a2 = say(&h, &mut sink_rx, "tg:42", "and my shell is fish").await;
    let a3 = say(&h, &mut sink_rx, "tg:42", "what did i say first?").await;

    assert!(
        a1.starts_with("tg:42-"),
        "the id names the channel it belongs to: {a1}"
    );
    assert_eq!(a1, a2, "turn 2 is the same call as turn 1");
    assert_eq!(a2, a3, "and so is turn 3");

    // A second channel is a second call, not the same one.
    let b1 = say(&h, &mut sink_rx, "tg:7", "hi").await;
    assert!(b1.starts_with("tg:7-"), "{b1}");
    assert_ne!(a3, b1, "two channels are two generations");

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_firing_on_a_channel_that_just_spoke_closes_nothing() {
    let td = tempfile::TempDir::new().unwrap();
    // The shipped idle threshold: two hours of silence. The channel spoke
    // milliseconds ago, so the night finds nothing to end.
    build_tree(&td, None);
    let (h, mut sink_rx, mut park_rx) = boot(&td).await;

    let sid = say(&h, &mut sink_rx, "tg:42", "still talking").await;
    h.send(fire()).await;
    // Trigger + the store's answer to the sweep: the pass has decided.
    await_close_pass(&td, 2).await;
    assert!(
        tokio::time::timeout(Duration::from_secs(1), park_rx.recv())
            .await
            .is_err(),
        "a live conversation is not ended by a clock"
    );

    // And the call goes on, on the same generation.
    let after = say(&h, &mut sink_rx, "tg:42", "see?").await;
    assert_eq!(after, sid, "the session survived the night sweep");

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idle_channel_is_closed_once_and_reopens_on_the_next_turn() {
    let td = tempfile::TempDir::new().unwrap();
    // Zero idle time: every channel counts as silent the moment the sweep runs.
    // The threshold is the only difference to the test above -- everything else
    // about the tree, the wiring and the traffic is identical.
    build_tree(&td, Some(0));
    let (h, mut sink_rx, mut park_rx) = boot(&td).await;

    let sid = say(&h, &mut sink_rx, "tg:42", "good night").await;

    h.send(fire()).await;
    let closed = recv_bounded(&mut park_rx)
        .await
        .expect("the idle generation is closed");
    assert_eq!(
        text_of(&closed),
        format!("closed:{sid}|tg:42"),
        "the close request names the generation AND its channel"
    );

    // The second firing of the same night: the generation is already sealed,
    // and a sealed generation is not a candidate. Missed firings expire, and
    // repeated ones are silent -- that is what makes the timer safe to run
    // twelve times a night.
    h.send(fire()).await;
    await_close_pass(&td, 4).await;
    assert!(
        tokio::time::timeout(Duration::from_secs(1), park_rx.recv())
            .await
            .is_err(),
        "the same session must not be closed twice"
    );

    // Reopening is lazy: THIS turn is the beginning of the next generation.
    let next = say(&h, &mut sink_rx, "tg:42", "good morning").await;
    assert_ne!(next, sid, "a new day, a new generation: {next}");
    assert!(next.starts_with("tg:42-"), "{next}");

    h.shutdown().await;
}
