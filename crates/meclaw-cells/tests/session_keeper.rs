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
    // conversation content -- the turns belong to the curator's ledger (the
    // collector's window until GH #889).
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
        // GH #953: the channel turn id whose final answer the generation
        // still owes ('' = nothing; NULL = a row from before the column,
        // not yet visited by the night's heal, GH #954).
        ("owed_turn", "text"),
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
    // Sessions are per channel and round: the lookup reads the channel, the
    // round decides among its open generations once the rows are back.
    assert_eq!(
        op["where"]["channel"], "tg:42",
        "sessions are per channel and round"
    );
    assert_eq!(op["where"]["closed"], 0, "only an OPEN generation counts");
    // because GH #940 / ADR-0002 E8: EVERY open generation of the channel is
    // read (newest first, bounded), not the newest one alone.
    assert_eq!(op["limit"], 16);
    assert_eq!(op["order_by"][0]["col"], "opened_at");
    assert_eq!(op["order_by"][0]["dir"], "desc");
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
    // GH #954: the open is the claim of the round, and the turn waits for it.
    assert_eq!(out.len(), 1, "the open alone, the turn held: {out:?}");

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

    // The insert's reply: the claim stands, the held turn leaves with the id.
    let held = route(&out, "kstore")["header"]["keeper_body"].clone();
    let mut doc = reply_doc("keeper-stamp", "open", "insert", 1, serde_json::json!([]));
    doc["header"]["context"]["keeper_session"] = serde_json::json!(minted);
    doc["header"]["context"]["keeper_body"] = held;
    let out = stamp(doc);
    let turn = route(&out, "turn");
    assert_eq!(turn["header"]["session_id"], minted);
    assert_eq!(turn["messages"][0]["text"], "good morning");
}

/// GH #954: the open is the claim of its round. A racing first turn that met
/// the unique index `sessions_open_round` looks again, the held turn riding
/// along, and joins whatever generation it then finds; the index itself is
/// declared on the store.
#[test]
fn a_first_turn_that_loses_the_claim_of_its_round_looks_again() {
    let sessions = config_of("sessions/config.json");
    assert_eq!(
        sessions["params"]["indexes"]["sessions_open_round"],
        serde_json::json!({"table": "sessions",
                           "on": ["channel", "audience_set", "closed_at"],
                           "unique": true}),
        "one open generation of a round per channel is the store's to hold"
    );

    let held = turn_body("we raced").to_string();
    let mut doc = reply_doc("keeper-stamp", "open", "insert", 0, serde_json::json!([]));
    doc["header"]["hop"]["error_code"] = serde_json::json!("unique_violation");
    doc["header"]["context"]["keeper_session"] = serde_json::json!("tg:42-0009");
    doc["header"]["context"]["keeper_body"] = serde_json::json!(held);
    let out = stamp(doc);
    assert_eq!(
        out.len(),
        1,
        "a lost claim is no refusal, it looks again: {out:?}"
    );
    let look = op_of(&route(&out, "kstore"));
    assert_eq!(look["operation"], "select");
    assert_eq!(
        look["where"],
        serde_json::json!({"channel": "tg:42", "closed": 0})
    );
    assert_eq!(route(&out, "kstore")["header"]["phase"], "look");
    assert_eq!(
        route(&out, "kstore")["header"]["keeper_body"],
        held,
        "the held turn rides along"
    );

    // The second look finds the winner's generation: the turn runs in it.
    let out = stamp(look_reply(
        serde_json::json!([session_row("tg:42-0008", "0008", "0008")]),
        turn_body("we raced"),
    ));
    assert_eq!(route(&out, "turn")["header"]["session_id"], "tg:42-0008");
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
    // `open` is no longer an end (GH #954): its reply hands the held turn
    // on, exactly once -- `a_channel_without_an_open_generation_opens_the_next_one_lazily`.
    assert!(
        stamp(reply_doc(
            "keeper-stamp",
            "touch",
            "update",
            1,
            serde_json::json!("ok")
        ))
        .is_empty(),
        "the touch reply is the end of the chain"
    );
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

/// The emission of `out` on store phase `phase` (exactly one).
fn on_phase(out: &[serde_json::Value], phase: &str) -> serde_json::Value {
    let hits: Vec<&serde_json::Value> = out
        .iter()
        .filter(|m| m["header"]["phase"] == phase)
        .collect();
    assert_eq!(hits.len(), 1, "one emission on phase {phase}: {out:?}");
    hits[0].clone()
}

#[test]
fn the_night_sweep_asks_only_for_the_channels_that_fell_silent() {
    let out = close(firing());
    // Three questions since GH #953/#954, each a read: the idle sweep, the
    // sentinel for an answer that never came, and a page of the heal.
    assert_eq!(
        out.len(),
        3,
        "sweep, owed and heal-page, asked of the store: {out:?}"
    );
    for m in &out {
        assert_eq!(m["header"]["route"], "kstore");
        assert_eq!(op_of(m)["operation"], "select", "a firing only asks: {m}");
    }
    let sweep = on_phase(&out, "sweep");
    let op = op_of(&sweep);
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

    // GH #953: the sentinel asks for sealed generations that still owe an
    // answer and fell silent past the same cutoff -- `neq ''` lets no NULL
    // (a row from before the column) through.
    let owed = op_of(&on_phase(&out, "owed"));
    assert_eq!(owed["where"]["closed"], 1, "{owed}");
    assert_eq!(owed["where"]["owed_turn"], serde_json::json!({"neq": ""}));
    assert_eq!(owed["where"]["last_seen"]["lt"].as_str(), Some(cutoff));
    // GH #954: the heal visits sealed rows the column has not marked yet, a
    // page at a time (the shipped `heal_limit`), oldest stamp first.
    let heal = op_of(&on_phase(&out, "heal-page"));
    assert_eq!(
        heal["where"],
        serde_json::json!({"closed": 1, "owed_turn": {"is_null": true}})
    );
    assert_eq!(heal["limit"], 200, "the shipped `heal_limit`");
    assert_eq!(heal["order_by"][0]["col"], "closed_at");

    let out = close_with(serde_json::json!({"idle_ms": 600000}), firing());
    let cutoff = op_of(&on_phase(&out, "sweep"))["where"]["last_seen"]["lt"]
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
        // GH #953: two hours of silence mean the owed answer is not coming;
        // the seal clears the mark with it.
        assert_eq!(op["set"]["owed_turn"], "", "{op}");
    }
    // GH #954 (review I-1): one pass, one distinct `closed_at` per sealed
    // row -- two rows of one round sealed with one `now` collide under the
    // unique index `sessions_open_round`, and the next wake cannot build it.
    assert_ne!(
        op_of(&out[0])["set"]["closed_at"],
        op_of(&out[1])["set"]["closed_at"],
        "two generations sealed by one pass share a stamp"
    );
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
         close request would ask the curator for the same batch twice"
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
        "a close request carries no conversation turns -- the curator reads \
         the session out of its own ledger (GH #889; the collector's store before)"
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

/// The round is a constant of its generation (ADR-0002 E8), and since GH #940
/// the keeper enforces it: a turn whose round differs from the open generation
/// of its channel SEALS that generation -- under the round it was opened in --
/// and runs on in a new generation of its own round. Provenance is still never
/// rewritten (ADR-0002 E12): a running generation's row keeps the round of the
/// turn that opened it, and a turn of another round ends it instead of
/// renaming it.
///
/// Asked of a running colony with a real store, one channel, turn by turn:
///
/// * round A twice (two spellings of one set) is ONE generation;
/// * a turn without a round in between seals nothing and runs in the
///   round-less generation of the channel;
/// * round B ends A's generation, round A again ends B's: three rounded
///   generations, each with exactly one round, each sealed one carrying its
///   OLD round in its `close`;
/// * two turns of a new round at the same time: the seal is a guarded update,
///   exactly one of them wins it, and exactly one `close` leaves.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_round_change_ends_the_generation() {
    const CH: &str = "tg:42";
    const ROUND_A: &str = r#"["member:alex","agent:scribe"]"#;
    const ROUND_A_RESPELLED: &str = r#"["agent:scribe", "member:alex"]"#;
    const ROUND_B: &str = r#"["member:robin","agent:scribe"]"#;

    let td = tempfile::TempDir::new().unwrap();
    // Every turn here carries a channel turn id (`turn_in`), so its
    // generation owes the turn's final answer until the stand-in downstream
    // acknowledges it (GH #953): whether a seal finds the mark already
    // cleared (`seal-done`, close at once) or not (`seal-owed`, close on the
    // acknowledgement), exactly one close of the sealed generation arrives.
    build_tree_with(&td, None, true);
    let (h, mut sink_rx, mut park_rx) = boot(&td).await;

    // Round A, twice: one generation, whatever the spelling of the set.
    let a1 = say_in(&h, &mut sink_rx, CH, "the first turn of round a", ROUND_A).await;
    let a2 = say_in(&h, &mut sink_rx, CH, "the second", ROUND_A_RESPELLED).await;
    assert!(a1.starts_with("tg:42-"), "{a1}");
    assert_eq!(a1, a2, "the same round twice is one generation");

    // A turn without a round is no evidence that the round changed: it runs in
    // the round-less generation of the channel and seals nothing.
    let none = say_in(&h, &mut sink_rx, CH, "a door that declares no round", "").await;
    assert_ne!(
        none, a1,
        "a round-less turn never joins a generation of a round"
    );
    // The seal would have left in the same emission as the open of `none`, so
    // once that row stands, a seal would be logged too.
    until_generation(&td, &none).await;
    assert_eq!(
        seal_traffic(&td).asked,
        0,
        "a turn without a round sealed a generation"
    );
    assert!(
        generations(&td).iter().all(|g| g.closed == 0),
        "a turn without a round closed a generation: {:?}",
        generations(&td)
    );

    // Round B ends the generation of round A, under round A.
    let b1 = say_in(&h, &mut sink_rx, CH, "round b speaks", ROUND_B).await;
    assert!(
        b1 != a1 && b1 != none,
        "round b opens its own generation: {b1}"
    );
    let closed = recv_bounded(&mut park_rx)
        .await
        .expect("a turn of round b ends the generation of round a");
    assert_eq!(text_of(&closed), format!("closed:{a1}|{CH}"));
    assert_eq!(
        round_of_close(&closed),
        ROUND_A,
        "the close carries the round its generation was OPENED in, never the round \
         of the turn that ended it"
    );
    assert_close_carries_no_turn(&closed, ROUND_A);

    // Round A again: a NEW generation (the sealed one stays sealed), and the
    // generation of round B is over.
    let a3 = say_in(&h, &mut sink_rx, CH, "round a is back", ROUND_A).await;
    assert!(
        a3 != a1 && a3 != b1 && a3 != none,
        "a round that comes back opens a new generation: {a3}"
    );
    let closed = recv_bounded(&mut park_rx)
        .await
        .expect("a turn of round a ends the generation of round b");
    assert_eq!(text_of(&closed), format!("closed:{b1}|{CH}"));
    assert_eq!(round_of_close(&closed), ROUND_B);
    assert_close_carries_no_turn(&closed, ROUND_B);

    // Two turns of round B at the same time. Whichever looked first, the
    // generation of round A is flipped by exactly one guarded update, and only
    // the seal that flipped it hands it over.
    let before = seal_traffic(&td);
    h.send(turn_in(CH, "round b, once", ROUND_B)).await;
    h.send(turn_in(CH, "round b, twice", ROUND_B)).await;
    let mut raced = Vec::new();
    for _ in 0..2 {
        let got = recv_bounded(&mut sink_rx)
            .await
            .expect("both turns of the race are stamped");
        raced.push(text_of(&got));
    }
    let after = await_seals_answered(&td, before.asked).await;
    assert_eq!(
        after.won - before.won,
        1,
        "exactly one seal of the race flipped the generation of round a: \
         before {before:?}, after {after:?}"
    );
    let closed = recv_bounded(&mut park_rx)
        .await
        .expect("the race ends the generation of round a");
    assert_eq!(text_of(&closed), format!("closed:{a3}|{CH}"));
    assert_eq!(round_of_close(&closed), ROUND_A);
    assert_close_carries_no_turn(&closed, ROUND_A);

    // The store: every generation with exactly one round, the rounded ones
    // sealed, the round-less one never sealed by a turn that had a round.
    for sid in &raced {
        assert!(
            ![&a1, &b1, &a3, &none].contains(&sid),
            "a turn of the race ran in an older generation: {sid}"
        );
        until_generation(&td, sid).await;
    }
    let rows = generations(&td);
    let row = |sid: &str| {
        rows.iter()
            .find(|g| g.session_id == sid)
            .unwrap_or_else(|| panic!("no row for {sid}: {rows:?}"))
            .clone()
    };
    for (sid, round, closed) in [
        (&a1, ROUND_A, 1),
        (&b1, ROUND_B, 1),
        (&a3, ROUND_A, 1),
        (&none, "", 0),
    ] {
        let g = row(sid);
        assert_eq!(
            (g.audience_set.as_str(), g.closed),
            (round, closed),
            "generation {sid}: {rows:?}"
        );
    }
    for sid in &raced {
        let g = row(sid);
        assert_eq!(
            (g.audience_set.as_str(), g.closed),
            (ROUND_B, 0),
            "the race runs in round b: {rows:?}"
        );
    }

    h.shutdown().await;
}

/// The store's answer to a bundle as the hive edge delivers it back (GH #295):
/// `hop.operation` 'bundle', one `tool_result` per leg and the per-leg
/// metadata in `results[]`, both tied to the leg by its id. `legs` is
/// `(id, operation, rows_affected, rows)`.
fn bundle_reply(
    phase: &str,
    session: &str,
    legs: &[(&str, &str, i64, serde_json::Value)],
) -> serde_json::Value {
    let rows_affected: i64 = legs.iter().map(|l| l.2).sum();
    let turns: Vec<serde_json::Value> = legs
        .iter()
        .map(|l| {
            serde_json::json!({"origin": "tool", "type": "tool_result", "id": l.0,
                               "text": l.3.to_string()})
        })
        .collect();
    let results: Vec<serde_json::Value> = legs
        .iter()
        .map(|l| {
            serde_json::json!({"tool_call_id": l.0, "operation": l.1,
                               "rows_affected": l.2, "duration_ms": 0})
        })
        .collect();
    serde_json::json!({
        "header": {"context": {"channel": "tg:42", "ses_phase": phase,
                               "store_origin": "keeper-stamp",
                               "keeper_session": session},
                   "hop": {"operation": "bundle", "rows_affected": rows_affected,
                           "bundle_errors": 0}},
        "messages": turns,
        "results": results
    })
}

/// The tool_call legs of one emitted store message, by id.
fn legs_of(msg: &serde_json::Value) -> std::collections::BTreeMap<String, serde_json::Value> {
    msg["messages"]
        .as_array()
        .expect("legs")
        .iter()
        .map(|m| {
            assert_eq!(m["type"], "tool_call", "{msg}");
            (
                m["id"].as_str().expect("leg id").to_string(),
                serde_json::from_str(m["text"].as_str().expect("leg text")).expect("leg json"),
            )
        })
        .collect()
}

/// The seal a round change sends is a guarded bundle (GH #940, GH #953): two
/// legs, both guarded by `closed = 0`, so at most one flips the row. A
/// generation that owes no answer (`seal-done`) is handed over at once; one
/// whose last turn still waits for its final answer (`seal-owed`) is not --
/// the acknowledgement of that answer sends the close (see
/// [`the_answer_a_sealed_generation_owed_hands_it_over`]). Either way it is
/// handed over under the round it was opened in, never under the round of the
/// turn that ended it -- which is in context and must not leave on this
/// message.
#[test]
fn only_the_turn_that_won_the_seal_hands_the_generation_over() {
    let old_round = r#"["member:alex","agent:scribe"]"#;
    let new_round = r#"["member:robin"]"#;

    // The look of a turn of round b finds the open generation of round a.
    let mut row = session_row("tg:42-0001", "0001", "0002");
    row["audience_set"] = serde_json::json!(old_round);
    let mut doc = look_reply(serde_json::json!([row]), turn_body("round b speaks"));
    doc["header"]["context"]["audience_set"] = serde_json::json!(new_round);
    doc["header"]["context"]["turn_id"] = serde_json::json!("turn-b");
    let out = stamp(doc);
    let seal = on_phase(&out, "seal");
    assert_eq!(seal["header"]["session_id"], "tg:42-0001");
    assert_eq!(seal["header"]["audience_set"], old_round);
    let legs = legs_of(&seal);
    assert_eq!(
        legs.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["seal-done", "seal-owed"],
        "ONE bundle, two legs: {seal}"
    );
    let (done, owed) = (&legs["seal-done"], &legs["seal-owed"]);
    assert_eq!(
        done["where"],
        serde_json::json!({"session_id": "tg:42-0001", "closed": 0,
                           "owed_turn": {"or_null": {"eq": ""}}}),
        "seal-done takes only a generation that owes nothing: {done}"
    );
    assert_eq!(done["set"]["owed_turn"], "");
    assert_eq!(
        owed["where"],
        serde_json::json!({"session_id": "tg:42-0001", "closed": 0}),
        "seal-owed takes the rest: {owed}"
    );
    assert!(
        owed["set"].get("owed_turn").is_none(),
        "seal-owed leaves the mark standing for the acknowledgement: {owed}"
    );
    for leg in [done, owed] {
        assert_eq!(leg["operation"], "update");
        assert_eq!(leg["set"]["closed"], 1);
        assert_eq!(
            leg["set"]["closed_at"], done["set"]["closed_at"],
            "both legs carry the one stamp of this seal"
        );
    }

    let seal_reply = |done: i64, owed: i64| {
        let mut doc = bundle_reply(
            "seal",
            "tg:42-0001",
            &[
                ("seal-done", "update", done, serde_json::json!("ok")),
                ("seal-owed", "update", owed, serde_json::json!("ok")),
            ],
        );
        doc["header"]["context"]["keeper_audience"] = serde_json::json!(old_round);
        doc["header"]["context"]["audience_set"] = serde_json::json!(new_round);
        doc
    };
    assert!(
        stamp(seal_reply(0, 0)).is_empty(),
        "neither leg flipped the row: a parallel turn sealed first, and its close is the one"
    );
    assert!(
        stamp(seal_reply(0, 1)).is_empty(),
        "sealed while it owes an answer: the acknowledgement of that answer sends \
         the close, not the seal (GH #953)"
    );
    let out = stamp(seal_reply(1, 0));
    assert_eq!(out.len(), 1, "exactly ONE close request per generation");
    assert_eq!(out[0]["header"]["route"], "close");
    assert_eq!(out[0]["header"]["session_id"], "tg:42-0001");
    assert_eq!(out[0]["header"]["channel"], "tg:42");
    assert_eq!(
        out[0]["header"]["audience_set"], old_round,
        "the close carries the round the generation was opened in"
    );
    assert_eq!(out[0]["messages"], serde_json::json!([]));
}

/// GH #953 (review I-3, OR-NL-163): a turn makes its generation owe its final
/// answer -- the open and the touch write the channel's turn id into
/// `owed_turn` -- and the acknowledgement of that answer (`in_answered`, the
/// curator writer's `turn_write` behind the wall insert) clears it. If the
/// generation was sealed while it waited, the acknowledgement is what hands
/// it over: after the answer stands on the wall, by an event, not by a lead.
#[test]
fn the_answer_a_sealed_generation_owed_hands_it_over() {
    // The open and the touch record what the turn is owed.
    let mut doc = look_reply(serde_json::json!([]), turn_body("hello"));
    doc["header"]["context"]["turn_id"] = serde_json::json!("turn-1");
    let open = op_of(&route(&stamp(doc), "kstore"));
    assert_eq!(open["row"]["owed_turn"], "turn-1", "{open}");
    let mut doc = look_reply(
        serde_json::json!([session_row("tg:42-0001", "0001", "0002")]),
        turn_body("and again"),
    );
    doc["header"]["context"]["turn_id"] = serde_json::json!("turn-2");
    let touch = op_of(&on_phase(&stamp(doc), "touch"));
    assert_eq!(touch["set"]["owed_turn"], "turn-2", "{touch}");

    // The acknowledgement: only the FINAL answer, of a known session and turn.
    let answered = |origin: &str, turn: &str, session: &str| {
        stamp(serde_json::json!({
            "header": {"context": {"channel": "tg:42", "turn_id": turn},
                       "hop": {"route": "in_answered", "session_id": session}},
            "messages": [{"origin": origin, "type": "text", "text": "the answer"}]
        }))
    };
    assert!(
        answered("user", "turn-2", "tg:42-0001").is_empty(),
        "the person's own turn acknowledges nothing"
    );
    assert!(
        answered("assistant", "turn-2", "").is_empty(),
        "an answer without a session acknowledges nothing"
    );
    let out = answered("assistant", "turn-2", "tg:42-0001");
    assert_eq!(out.len(), 1, "ONE bundle to the store: {out:?}");
    let ack = route(&out, "kstore");
    let legs = legs_of(&ack);
    assert_eq!(
        legs["ack-sealed"]["where"],
        serde_json::json!({"session_id": "tg:42-0001",
                           "owed_turn": {"in": ["turn-2", "*"]}, "closed": 1}),
        "the mark of this turn, or the turn-less mark '*'"
    );
    assert_eq!(
        legs["ack-open"]["where"],
        serde_json::json!({"session_id": "tg:42-0001",
                           "owed_turn": {"in": ["turn-2", "*"]}, "closed": 0})
    );
    for id in ["ack-sealed", "ack-open"] {
        assert_eq!(legs[id]["operation"], "update");
        assert_eq!(legs[id]["set"], serde_json::json!({"owed_turn": ""}));
    }
    assert_eq!(legs["ack-row"]["operation"], "select");
    assert_eq!(
        legs["ack-row"]["where"],
        serde_json::json!({"session_id": "tg:42-0001"})
    );

    // The store's answer, as the hive edge carries it back.
    let phase = ack["header"]["phase"]
        .as_str()
        .expect("ack phase")
        .to_string();
    let reply = |sealed: i64, open: i64| {
        stamp(bundle_reply(
            &phase,
            "tg:42-0001",
            &[
                ("ack-sealed", "update", sealed, serde_json::json!("ok")),
                ("ack-open", "update", open, serde_json::json!("ok")),
                (
                    "ack-row",
                    "select",
                    1,
                    serde_json::json!([{"channel": "tg:42",
                                        "audience_set": r#"["member:alex","agent:scribe"]"#}]),
                ),
            ],
        ))
    };
    assert!(
        reply(0, 1).is_empty(),
        "the generation is still open: the call goes on, nothing is handed over"
    );
    assert!(
        reply(0, 0).is_empty(),
        "the mark is a later turn's, or the night released it and closed it itself"
    );
    let out = reply(1, 0);
    assert_eq!(out.len(), 1, "exactly ONE close: {out:?}");
    assert_eq!(out[0]["header"]["route"], "close");
    assert_eq!(out[0]["header"]["session_id"], "tg:42-0001");
    assert_eq!(out[0]["header"]["channel"], "tg:42");
    assert_eq!(
        out[0]["header"]["audience_set"], r#"["member:alex","agent:scribe"]"#,
        "room and round come off the row, as the night reads them"
    );
    assert_eq!(out[0]["messages"], serde_json::json!([]));
}

/// GH #953: a lookup reply WITHOUT a turn id still owes its final answer --
/// the open and the touch write the mark '*' -- and an answer without a turn
/// id acknowledges exactly that mark. Since Re-Review R-1 a turn on `in_turn`
/// never gets here without an id (`a_turn_without_an_id_gets_one_at_the_door`);
/// '*' is the fallback of a reply that lost it (one in flight across the
/// upgrade), which must not close a sealed generation by path length again.
#[test]
fn a_turn_without_an_id_owes_its_answer_under_the_star() {
    let open = op_of(&route(
        &stamp(look_reply(serde_json::json!([]), turn_body("hello"))),
        "kstore",
    ));
    assert_eq!(open["row"]["owed_turn"], "*", "{open}");
    let touch = op_of(&on_phase(
        &stamp(look_reply(
            serde_json::json!([session_row("tg:42-0001", "0001", "0002")]),
            turn_body("and again"),
        )),
        "touch",
    ));
    assert_eq!(touch["set"]["owed_turn"], "*", "{touch}");

    let out = stamp(serde_json::json!({
        "header": {"context": {"channel": "tg:42"},
                   "hop": {"route": "in_answered", "session_id": "tg:42-0001"}},
        "messages": [{"origin": "assistant", "type": "text", "text": "the answer"}]
    }));
    assert_eq!(out.len(), 1, "ONE bundle to the store: {out:?}");
    let legs = legs_of(&route(&out, "kstore"));
    for (id, closed) in [("ack-sealed", 1), ("ack-open", 0)] {
        assert_eq!(
            legs[id]["where"],
            serde_json::json!({"session_id": "tg:42-0001", "owed_turn": "*",
                               "closed": closed}),
            "{id}: an answer without a turn id clears only the mark '*'"
        );
        assert_eq!(legs[id]["set"], serde_json::json!({"owed_turn": ""}));
    }
}

/// GH #953, Re-Review R-1 (Fix-Runde 2): a turn that reaches the keeper
/// WITHOUT a channel turn id (the Telegram text road) gets one at this door,
/// minted from the substrate's own trace of the turn plus the stamp's instant
/// -- so two id-less turns owe their answers under two marks, and the answer
/// to the earlier one can no longer acknowledge the debt of the later one.
/// The id rides the lookup on the hop (the keeper's store edge promotes it to
/// `context.turn_id`, and the turn leaves with it). A turn that carries a
/// channel id keeps it untouched; `'*'` stays only as the fallback of a
/// lookup reply that carries no id at all.
#[test]
fn a_turn_without_an_id_gets_one_at_the_door() {
    let id_of = |trace: &str, ctx: serde_json::Value| -> String {
        let out = stamp(serde_json::json!({
            "header": {"context": ctx, "hop": {"route": "in_turn"}},
            "trace_id": trace,
            "messages": [{"origin": "user", "type": "text", "text": "hello"}]
        }));
        assert_eq!(out.len(), 1, "one lookup: {out:?}");
        assert_eq!(out[0]["header"]["phase"], "look");
        out[0]["header"]["turn_id"]
            .as_str()
            .expect("the lookup carries a turn id")
            .to_string()
    };
    let first = id_of("trace-1", serde_json::json!({"channel": "tg:42"}));
    let second = id_of("trace-2", serde_json::json!({"channel": "tg:42"}));
    assert!(
        !first.is_empty() && first != "*",
        "minted, never empty or '*': {first}"
    );
    assert!(
        first.contains("trace-1"),
        "the substrate's trace names the turn: {first}"
    );
    assert_ne!(first, second, "two id-less turns, two marks");
    assert_eq!(
        id_of(
            "trace-3",
            serde_json::json!({"channel": "tg:42", "turn_id": "turn-9"})
        ),
        "turn-9",
        "a channel id is carried, never replaced"
    );
}

/// Review M-3: an insert the store answered without an error AND without a
/// row is not a lost claim (that is `unique_violation`) -- and parking it
/// swallowed the held turn whole: no answer, no reject, nothing in any log.
/// The turn runs on under the id it minted, and a reject says the row is
/// missing.
#[test]
fn an_open_that_wrote_no_row_hands_the_turn_on_and_says_so() {
    let mut doc = reply_doc("keeper-stamp", "open", "insert", 0, serde_json::json!([]));
    doc["header"]["context"]["keeper_session"] = serde_json::json!("tg:42-0009");
    doc["header"]["context"]["keeper_body"] =
        serde_json::json!(turn_body("is anybody there").to_string());
    let out = stamp(doc);
    assert_eq!(out.len(), 2, "the held turn and a reject: {out:?}");
    let turn = route(&out, "turn");
    assert_eq!(turn["header"]["session_id"], "tg:42-0009");
    assert_eq!(turn["messages"][0]["text"], "is anybody there");
    let reject = route(&out, "reject");
    assert_eq!(reject["header"]["reject_reason"], "store_refused");
    assert_eq!(reject["header"]["store_operation"], "insert");
    assert!(
        !reject["header"]["store_error"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "the reject names what went wrong: {reject}"
    );
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
    let cols = op_of(&on_phase(&asked, "sweep"))["columns"].clone();
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

/// OR-BC.R.6, pinned at the config: the close a round change sends leaves the
/// stamp in the context of the turn that ended the generation. Its edge out of
/// the hive therefore drops everything the keeper's other exits drop PLUS the
/// round and the participants of that turn -- otherwise a parent that does not
/// overwrite `context.audience_set` from the hop would hand the old
/// generation's close to the curator under the NEW round, with the new chat and
/// user beside it.
#[test]
fn the_close_a_round_change_sends_leaves_the_triggering_turn_behind() {
    let hive = config_of("config.json");
    let edges = hive["params"]["graph"]["edges"].as_array().expect("edges");
    let dropped = |from: &str, route: &str| -> std::collections::BTreeSet<String> {
        let edge = edges
            .iter()
            .find(|e| {
                e["from"] == from
                    && e["to"] == "."
                    && e["condition"]
                        .as_str()
                        .is_some_and(|c| c.contains(&format!("hop.route == '{route}'")))
            })
            .unwrap_or_else(|| panic!("{from} -> . on {route}"));
        edge["modifier"]["delete_context"]
            .as_array()
            .unwrap_or_else(|| panic!("{from} -> . on {route} deletes nothing: {edge}"))
            .iter()
            .map(|k| k.as_str().expect("key").to_string())
            .collect()
    };
    let keeper = dropped("./stamp", "turn");
    let close = dropped("./stamp", "close");
    for key in keeper.iter().map(String::as_str).chain([
        "asker",
        "audience_now",
        "audience_set",
        "chat_id",
        "counterpart",
        "counterpart_name",
        "turn_id",
        "user_id",
    ]) {
        assert!(
            close.contains(key),
            "the close out of the stamp keeps `{key}` of the triggering turn: {close:?}"
        );
    }
    // The night's close never had a turn in context; its list stays the
    // keeper's own, and the stamp's close is a superset of it.
    assert!(
        dropped("./close", "close").is_subset(&close),
        "the stamp's close drops less than the night's"
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

/// [`REPORT`] plus the acknowledgement the curator's writer gives a turn's
/// final answer (`turn_write`, GH #953): an `answered` message carrying the
/// session and the channel turn id, which the parent wires into the keeper's
/// `in_answered` the way talky wires `./curator -> ./session-keeper`. Here the
/// answer is "written" the moment the turn is stamped -- this tree has no
/// wall, only the order: stamped, answered, acknowledged.
const REPORT_ANSWERED: &str = r#"
import sys, json
doc = json.load(sys.stdin)
envelope = doc["envelope"]
ctx = (envelope.get("header") or {}).get("context") or {}
sid = str(ctx.get("session_id", ""))
tid = str(ctx.get("turn_id", ""))
sys.stdout.write(json.dumps([
    {"header": {"route": "report"},
     "messages": [{"origin": "assistant", "type": "text", "text": sid}]},
    {"header": {"route": "answered", "session_id": sid, "turn_id": tid},
     "messages": [{"origin": "assistant", "type": "text", "text": "the answer to " + tid}]}]))
"#;

/// The stand-in for the close lane downstream -- the curator's `in_close` since
/// GH #889, the collector's before. Its second message is the round the close
/// names on its hop (GH #940: a round change seals under the round of the
/// generation); the parent lifts it into `close_round`, never into
/// `context.audience_set`, so whatever `audience_set` arrives here is what the
/// keeper let through. Its third message is every key of the TRIGGERING turn
/// still in context (OR-BC.R.6): a close that a round change sends mid-turn
/// must not leave with the new round, the chat, the user, the counterpart or
/// the turn id of the turn that ended the generation.
const CLOSED: &str = r#"
import sys, json
doc = json.load(sys.stdin)
d = doc["body"]
envelope = doc["envelope"]
ctx = (envelope.get("header") or {}).get("context") or {}
TURN_KEYS = ["asker", "audience_now", "audience_set", "chat_id", "counterpart",
             "counterpart_name", "turn_id", "user_id"]
sys.stdout.write(json.dumps({"header": {"route": "closed"},
                             "messages": [{"origin": "assistant", "type": "text",
                                           "text": "closed:" + str(ctx.get("session_id", "")) +
                                                   "|" + str(ctx.get("channel", ""))},
                                          {"origin": "assistant", "type": "text",
                                           "text": str(ctx.get("close_round", ""))},
                                          {"origin": "assistant", "type": "text",
                                           "text": json.dumps({k: ctx[k] for k in TURN_KEYS if k in ctx},
                                                              sort_keys=True)}]}))
"#;

/// The port wiring a parent draws around the keeper: one ingress lane in, the
/// stamped turn out, the close request out -- plus the door the operator's
/// firing goes through.
///
/// GH #612: `./session-keeper/night` is not an address from outside. The keeper
/// declares `params.ports: []`, so the only address it has is its own path, and
/// a message that names an interior cell of it is refused `hive_boundary`. A
/// parent may reach the timer -- birth topology is authorship -- and this is what
/// that looks like: the parent draws the lane and the caller names the parent.
fn main_config(answered: bool) -> Value {
    let mut cfg = json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": ".", "to": "./session-keeper/night",
         "condition": "hop.route == 'fire_night'"},
        {"from": "./probe", "to": "./session-keeper",
         "condition": "hop.route == 'turn'",
         "modifier": {"set_hop": {"route": "'in_turn'"}}},
        {"from": "./session-keeper", "to": "./report",
         "condition": "hop.route == 'turn'",
         "modifier": {"set_context": {"session_id": "hop.session_id"}}},
        // Only the report: with `answered` the stand-in also emits the
        // acknowledgement, which goes back into the keeper and never to the sink.
        {"from": "./report", "to": "/sink", "condition": "hop.route == 'report'"},
        {"from": "./session-keeper", "to": "./closed",
         "condition": "hop.route == 'close'",
         "modifier": {"set_context": {"session_id": "hop.session_id",
                                      "channel": "hop.channel",
                                      "close_round": "has(hop.audience_set) ? hop.audience_set : ''"}}},
        {"from": "./closed", "to": "/park"}
    ]}}});
    if answered {
        // The acknowledgement of a turn's final answer (GH #953), drawn the
        // way talky draws `./curator -> ./session-keeper` on `turn_write`.
        cfg["params"]["graph"]["edges"]
            .as_array_mut()
            .expect("edges")
            .push(json!({"from": "./report", "to": "./session-keeper",
                         "condition": "hop.route == 'answered'",
                         "modifier": {"set_hop": {"route": "'in_answered'"},
                                      "set_context": {"session_id": "hop.session_id",
                                                      "turn_id": "hop.turn_id"}}}));
    }
    cfg
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
    build_tree_with(td, idle_ms, false);
}

/// [`build_tree`], and with `answered` the stand-in downstream also
/// acknowledges every turn's final answer into the keeper (GH #953) -- the
/// tree for turns that carry a channel turn id, whose generation owes that
/// answer until it is acknowledged.
fn build_tree_with(td: &tempfile::TempDir, idle_ms: Option<i64>, answered: bool) {
    let root = td.path();
    std::fs::write(root.join(".env"), "").unwrap();
    write(root, "main/config.json", &main_config(answered));
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
        &if answered {
            code_cell(REPORT_ANSWERED, &["report", "answered"])
        } else {
            code_cell(REPORT, &["report"])
        },
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
///
/// Addressed at the LEVEL, on the lane the level opened (see [`main_config`]).
/// The keeper is a sealed hive, so its own path is its only address and the
/// timer inside it is reached through the door its parent drew -- GH #612.
fn fire() -> Message {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("route".into(), json!("fire_night"));
    MessageBuilder::new(Path::new("/"))
        .hop(hop)
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

/// A turn spoken in a round. The round is declared at the door the turn
/// enters by, so it rides in context beside the channel; `""` is a door that
/// declares none.
fn turn_in(channel: &str, text: &str, round: &str) -> Message {
    let mut ctx = serde_json::Map::new();
    ctx.insert("channel".into(), json!(channel));
    if !round.is_empty() {
        ctx.insert("audience_set".into(), json!(round));
    }
    // What a channel door stamps beside the round (OR-BC.R.6): the close a
    // round change sends must leave all of it behind.
    ctx.insert("chat_id".into(), json!(42));
    ctx.insert("user_id".into(), json!("u-7"));
    ctx.insert("counterpart".into(), json!("member:robin"));
    ctx.insert("turn_id".into(), json!(format!("turn-{text}")));
    MessageBuilder::new(Path::new("/probe"))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .context(ctx)
        .ttl(64)
        .build()
}

/// One turn of a round in, the session id it was stamped with out.
async fn say_in(
    h: &ColonyHandle,
    rx: &mut mpsc::Receiver<Message>,
    ch: &str,
    text: &str,
    round: &str,
) -> String {
    h.send(turn_in(ch, text, round)).await;
    let got = recv_bounded(rx)
        .await
        .unwrap_or_else(|| panic!("no report for turn {text:?}"));
    text_of(&got)
}

/// The keys of the triggering turn that reached the close receiver, as a JSON
/// object (OR-BC.R.6).
fn turn_keys_at_close(m: &Message) -> serde_json::Map<String, Value> {
    match &m.body {
        Body::Inline(v) => serde_json::from_str(v["messages"][2]["text"].as_str().unwrap_or("{}"))
            .expect("the stand-in prints a JSON object"),
        Body::Blob(_) => panic!("inline expected"),
    }
}

/// A close a round change sent carries nothing of the turn that ended the
/// generation (OR-BC.R.6): no `audience_set` of the NEW round (absent, or the
/// generation's own old round), no chat, user, counterpart or turn id. The
/// parent here draws no `audience_set` promotion, so this is the keeper's own
/// `delete_context` on `./stamp -> .` speaking.
fn assert_close_carries_no_turn(m: &Message, old_round: &str) {
    let mut keys = turn_keys_at_close(m);
    if let Some(r) = keys.remove("audience_set") {
        assert_eq!(
            r.as_str(),
            Some(old_round),
            "the close of a generation carried the round of the turn that ended it"
        );
    }
    assert!(
        keys.is_empty(),
        "the close of a round change carried the triggering turn's context: {keys:?}"
    );
}

/// The round a `close` named, as the stand-in downstream received it.
fn round_of_close(m: &Message) -> String {
    match &m.body {
        Body::Inline(v) => v["messages"][1]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        Body::Blob(_) => panic!("inline expected"),
    }
}

/// The `seal` traffic of the stamp so far, read off the colony's own log: the
/// guarded seals it asked of its store, the answers that came back, and how
/// many of those flipped a generation (`rows_affected >= 1`). Since GH #953 a
/// seal is ONE bundle of two guarded legs (`seal-done`, `seal-owed`), answered
/// with `operation: "bundle"` and the SUM of the legs' `rows_affected` on the
/// header -- at most one leg can flip the row, so the sum is the flip.
#[derive(Clone, Copy, Debug, Default)]
struct SealTraffic {
    asked: usize,
    answered: usize,
    won: usize,
}

fn seal_traffic(td: &tempfile::TempDir) -> SealTraffic {
    let conn = rusqlite::Connection::open(td.path().join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare("SELECT to_path, headers FROM message_log WHERE to_path IN (?1, ?2)")
        .expect("message_log");
    let logged: Vec<(String, String)> = st
        .query_map(["/session-keeper/sessions", "/session-keeper/stamp"], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .expect("query")
        .filter_map(Result::ok)
        .collect();
    let mut t = SealTraffic::default();
    for (to, headers) in logged {
        let h: Value = serde_json::from_str(&headers).unwrap_or(Value::Null);
        if h["context"]["ses_phase"] != "seal" {
            continue;
        }
        if to == "/session-keeper/sessions" {
            t.asked += 1;
        } else if h["hop"]["operation"] == "update" || h["hop"]["operation"] == "bundle" {
            t.answered += 1;
            if h["hop"]["rows_affected"].as_i64().unwrap_or(0) >= 1 {
                t.won += 1;
            }
        }
    }
    t
}

/// Until more seals than `asked_before` were asked and every one of them was
/// answered: from then on the stamp has every answer that can send a `close`.
async fn await_seals_answered(td: &tempfile::TempDir, asked_before: usize) -> SealTraffic {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let t = seal_traffic(td);
        if t.asked > asked_before && t.answered == t.asked {
            return t;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the seals were not all answered within 30s: {t:?} (asked before: {asked_before})"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// One row of the keeper's own store.
#[derive(Clone, Debug)]
struct Generation {
    session_id: String,
    audience_set: String,
    closed: i64,
}

fn generations(td: &tempfile::TempDir) -> Vec<Generation> {
    let db = td.path().join("main/session-keeper/sessions/cell.db");
    if !db.is_file() {
        return Vec::new();
    }
    let conn = rusqlite::Connection::open(db).expect("sessions cell.db");
    let Ok(mut st) = conn.prepare(
        "SELECT session_id, COALESCE(audience_set, ''), closed FROM sessions ORDER BY opened_at",
    ) else {
        return Vec::new();
    };
    let rows: Vec<Generation> = st
        .query_map([], |r| {
            Ok(Generation {
                session_id: r.get(0)?,
                audience_set: r.get(1)?,
                closed: r.get(2)?,
            })
        })
        .expect("query")
        .filter_map(Result::ok)
        .collect();
    rows
}

/// Until the keeper's store holds the generation `sid`.
async fn until_generation(td: &tempfile::TempDir, sid: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !generations(td).iter().any(|g| g.session_id == sid) {
        assert!(
            std::time::Instant::now() < deadline,
            "generation {sid} was not written within 30s: {:?}",
            generations(td)
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Deliveries from `from` to `to`, optionally only those whose
/// `context.ses_phase` is `phase`.
fn delivered(td: &tempfile::TempDir, from: &str, to: &str, phase: Option<&str>) -> i64 {
    let conn = rusqlite::Connection::open(td.path().join("colony.db")).expect("colony.db");
    conn.query_row(
        "SELECT COUNT(*) FROM message_log WHERE from_path = ?1 AND to_path = ?2 \
         AND (?3 IS NULL OR json_extract(headers, '$.context.ses_phase') = ?3)",
        rusqlite::params![from, to, phase],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

/// Waits until the close pass has DECIDED on `firings` firings: the store
/// answered the idle sweep of each, and every question the pass asked of its
/// store has its answer. Only then does "nothing arrived at the port" become a
/// statement rather than a race.
///
/// Counted by phase and not by message count (GH #953/#954): a firing asks
/// three things since then -- the sweep, the sentinel for an owed answer, a
/// page of the heal -- so a raw count of deliveries to the pass no longer says
/// which of them were answered.
async fn await_close_pass(td: &tempfile::TempDir, firings: i64) {
    const CLOSE: &str = "/session-keeper/close";
    const SESSIONS: &str = "/session-keeper/sessions";
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let sweeps = delivered(td, SESSIONS, CLOSE, Some("sweep"));
        let asked = delivered(td, CLOSE, SESSIONS, None);
        let answered = delivered(td, SESSIONS, CLOSE, None);
        if sweeps >= firings && answered == asked {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the close pass did not decide on {firings} firing(s) within 30s: sweeps \
             answered {sweeps}, questions asked {asked}, answered {answered}"
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
    // The store answered the sweep and every other question: the pass has
    // decided.
    await_close_pass(&td, 1).await;
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
    await_close_pass(&td, 2).await;
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
