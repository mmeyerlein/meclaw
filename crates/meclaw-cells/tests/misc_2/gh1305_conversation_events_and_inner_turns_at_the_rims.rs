//! GH #1305 -- conversation events and inner turns, the keeper's and the rims' half.
//!
//! WHY this file exists. The keeper's `close` named a session, a channel and a
//! round, and nothing else: an app that wanted to know WHY a conversation
//! ended (the night, a change of the round), WHEN it had begun, or where to
//! reach the person had to guess, and nothing at all left when a conversation
//! began. A turn without a speaking person had no door of its own: it came in
//! as the person's turn on the person's channel, and an answer that chose to
//! say nothing went out as an empty message.
//!
//! Pinned here, on the shipped scripts and the shipped edges:
//! - every `close` carries `close_reason` (`round`, `night`, `idle`),
//!   `opened_at`, `closed_at` and `channel_node`, read off the generation's own
//!   row; talky hands them to the curator's `in_close` on the hop;
//! - with the keeper's knob `conversation_events`, ONE `opened` leaves when a
//!   turn opens a generation, and none on a turn of a running one; off, the
//!   stamp's output is byte for byte what it was;
//! - an inner turn (`hop.kind == "inner"`) enters on a channel of its own,
//!   `inner:<source>`, addressed back to `deliver_channel_node`; without the
//!   node or without a round the keeper refuses it; an empty answer to it is
//!   silence -- nothing goes to the channel, and with the dispatcher's knob
//!   `conversation_events` on `inner_silent` leaves the rim;
//! - `opened`, `closed` and `inner_silent` leave talky and the assistant rim.

use meclaw_colony::config::{EdgeSpec, HiveParams};
use meclaw_colony::edge_table::{Edge, EdgeTable, apply_edges};
use meclaw_core::serde_json::{self as sj, Value, json};
use meclaw_core::{Headers, Path, Uuid};
use std::path::PathBuf;

const STAMP: &str = "templates/session-keeper/stamp/config.json";
const CLOSE: &str = "templates/session-keeper/close/config.json";
const KEEPER: &str = "templates/session-keeper/config.json";
const DISPATCHER: &str = "templates/dispatcher/config.json";
const CURATOR: &str = "templates/curator/config.json";
const TALKY: &str = "templates/talky/config.json";
const ASSISTANT: &str = "templates/assistant/config.json";
const COLLECTOR: &str = "templates/collector/assemble/config.json";

const OPENED: &str = "2026-10-24T08:00:00.000000Z";
const ROUND_A: &str = "[\"member:a\"]";
const ROUND_B: &str = "[\"member:b\"]";

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// R2b / GH #49: a tree without the templates skips.
fn shipped() -> bool {
    repo(STAMP).is_file() && repo(ASSISTANT).is_file()
}

fn read_json(rel: &str) -> Value {
    let p = repo(rel);
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    sj::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The shipped script's stdout, byte for byte.
fn run_raw(cell: &str, mut doc: Value, params: Value) -> String {
    doc["params"] = params;
    let out = meclaw_testing::run_shipped_script(
        &meclaw_testing::shipped_script(&repo(cell).to_string_lossy()),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    assert!(
        out.status.success(),
        "{cell} exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8")
}

fn run(cell: &str, doc: Value, params: Value) -> Vec<Value> {
    match sj::from_str::<Value>(&run_raw(cell, doc, params)).expect("json") {
        Value::Array(a) => a,
        one => vec![one],
    }
}

fn routed<'a>(out: &'a [Value], route: &str) -> Vec<&'a Value> {
    out.iter()
        .filter(|m| m["header"]["route"] == json!(route))
        .collect()
}

fn meta_of(msg: &Value) -> Value {
    sj::from_str(msg["header"]["keeper_meta"].as_str().expect("keeper_meta")).expect("meta json")
}

/// A store reply to the stamp or the close pass, the way the hive edge
/// carries it back.
fn reply(origin: &str, phase: &str, op: &str, rows: i64, payload: Value, ctx: Value) -> Value {
    let mut context = json!({"channel": "tg:42", "ses_phase": phase, "store_origin": origin});
    for (k, v) in ctx.as_object().cloned().unwrap_or_default() {
        context[k] = v;
    }
    json!({
        "header": {"context": context, "hop": {"operation": op, "rows_affected": rows}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "x",
                      "text": payload.to_string()}]
    })
}

/// A bundle reply: `(leg id, operation, rows_affected, rows)` per leg.
fn bundle(phase: &str, legs: &[(&str, &str, i64, Value)], ctx: Value) -> Value {
    let mut context = json!({"channel": "tg:42", "ses_phase": phase,
                             "store_origin": "keeper-stamp"});
    for (k, v) in ctx.as_object().cloned().unwrap_or_default() {
        context[k] = v;
    }
    json!({
        "header": {"context": context,
                   "hop": {"operation": "bundle", "rows_affected": 1, "bundle_errors": 0}},
        "messages": legs.iter().map(|l| json!({"origin": "tool", "type": "tool_result",
                                               "id": l.0, "text": l.3.to_string()}))
                        .collect::<Vec<_>>(),
        "results": legs.iter().map(|l| json!({"tool_call_id": l.0, "operation": l.1,
                                              "rows_affected": l.2, "duration_ms": 0}))
                       .collect::<Vec<_>>()
    })
}

fn row(sid: &str, round: &str) -> Value {
    json!({"channel": "tg:42", "session_id": sid, "opened_at": OPENED,
           "last_seen": OPENED, "closed": 0, "closed_at": "", "audience_set": round,
           "owed_turn": "", "channel_node": "telegram"})
}

fn assert_close_carries(close: &Value, reason: &str, closed_at: &str) {
    let h = &close["header"];
    assert_eq!(h["close_reason"], reason, "{close}");
    assert_eq!(h["opened_at"], OPENED, "{close}");
    assert_eq!(h["channel_node"], "telegram", "{close}");
    if closed_at.is_empty() {
        assert!(!h["closed_at"].as_str().unwrap_or("").is_empty(), "{close}");
    } else {
        assert_eq!(h["closed_at"], closed_at, "{close}");
    }
}

// ------------------------------------------------------------ the close names why

#[test]
fn gh1305_round_change_closes_with_reason_round() {
    if !shipped() {
        return;
    }
    // A turn of round B finds the open generation of round A: the seal leaves
    // with the row's facts riding the store round trip.
    let look = reply(
        "keeper-stamp",
        "look",
        "select",
        1,
        json!([row("tg:42-old", ROUND_A)]),
        json!({"keeper_body": "{}", "audience_set": ROUND_B, "turn_id": "t9"}),
    );
    let out = run(STAMP, look, json!({}));
    let seal = out
        .iter()
        .find(|m| m["header"]["phase"] == "seal")
        .expect("a seal");
    let meta = meta_of(seal);
    assert_eq!(meta["reason"], "round");
    assert_eq!(meta["opened_at"], OPENED);
    assert_eq!(meta["channel_node"], "telegram");
    let stamp = meta["closed_at"].as_str().expect("closed_at").to_string();
    assert!(!stamp.is_empty());
    // The seal's answer: the close carries them.
    let done = bundle(
        "seal",
        &[
            ("seal-done", "update", 1, json!([])),
            ("seal-owed", "update", 0, json!([])),
        ],
        json!({"keeper_session": "tg:42-old", "keeper_audience": ROUND_A,
               "keeper_meta": seal["header"]["keeper_meta"].clone()}),
    );
    let out = run(STAMP, done, json!({}));
    let close = routed(&out, "close");
    assert_eq!(close.len(), 1, "{out:?}");
    assert_close_carries(close[0], "round", &stamp);
}

#[test]
fn gh1305_an_owed_round_close_names_round_off_the_row() {
    if !shipped() {
        return;
    }
    let ack = bundle(
        "ack",
        &[
            ("ack-sealed", "update", 1, json!([])),
            ("ack-open", "update", 0, json!([])),
            (
                "ack-row",
                "select",
                1,
                json!([{"channel": "tg:42", "audience_set": ROUND_A, "opened_at": OPENED,
                        "closed_at": "2026-10-24T09:00:00.000000Z", "channel_node": "telegram"}]),
            ),
        ],
        json!({"keeper_session": "tg:42-old"}),
    );
    let out = run(STAMP, ack, json!({}));
    let close = routed(&out, "close");
    assert_eq!(close.len(), 1, "{out:?}");
    assert_close_carries(close[0], "round", "2026-10-24T09:00:00.000000Z");
}

fn night_params(from: &str, until: &str) -> Value {
    json!({"night_from_utc": from, "night_until_utc": until})
}

#[test]
fn gh1305_night_close_reports_reason_night() {
    if !shipped() {
        return;
    }
    let sweep = reply(
        "keeper-close",
        "sweep",
        "select",
        1,
        json!([row("tg:42-a", ROUND_A)]),
        json!({}),
    );
    // A window that is the whole day: every firing is a night firing.
    let out = run(CLOSE, sweep.clone(), night_params("00:00", "24:00"));
    let seal = routed(&out, "kstore");
    assert_eq!(seal.len(), 1, "{out:?}");
    let meta = meta_of(seal[0]);
    assert_eq!(meta["reason"], "night");
    assert_eq!(meta["opened_at"], OPENED);
    assert_eq!(meta["channel_node"], "telegram");
    // An empty window: a sweep an operator forces is an idle close.
    let out = run(CLOSE, sweep, night_params("22:00", "22:00"));
    assert_eq!(meta_of(routed(&out, "kstore")[0])["reason"], "idle");
    // The seal's answer sends the close with the facts.
    let sealed = reply(
        "keeper-close",
        "seal",
        "update",
        1,
        json!([]),
        json!({"keeper_session": "tg:42-a", "keeper_audience": ROUND_A,
               "keeper_meta": seal[0]["header"]["keeper_meta"].clone()}),
    );
    let out = run(CLOSE, sealed, night_params("00:00", "24:00"));
    let close = routed(&out, "close");
    assert_eq!(close.len(), 1, "{out:?}");
    assert_close_carries(close[0], "night", meta["closed_at"].as_str().unwrap());
}

#[test]
fn gh1305_a_released_close_keeps_the_seal_instant() {
    if !shipped() {
        return;
    }
    let mut owed = row("tg:42-o", ROUND_A);
    owed["closed"] = json!(1);
    owed["closed_at"] = json!("2026-10-24T10:00:00.000000Z");
    owed["owed_turn"] = json!("t1");
    let out = run(
        CLOSE,
        reply(
            "keeper-close",
            "owed",
            "select",
            1,
            json!([owed]),
            json!({}),
        ),
        night_params("00:00", "24:00"),
    );
    let rel = routed(&out, "kstore");
    assert_eq!(rel.len(), 1, "{out:?}");
    let meta = meta_of(rel[0]);
    assert_eq!(meta["closed_at"], "2026-10-24T10:00:00.000000Z");
    // Review M1 (ruled): a turn of another round sealed this generation; the
    // night only releases it. The reason is the cause -- `round` -- also
    // inside the night window.
    assert_eq!(meta["reason"], "round");
}

// ------------------------------------------------------------ opened

fn open_reply() -> Value {
    reply(
        "keeper-stamp",
        "open",
        "insert",
        1,
        json!([]),
        json!({"keeper_session": "tg:42-new", "keeper_body": "{\"messages\": []}",
               "keeper_meta": json!({"opened_at": OPENED}).to_string(),
               "channel_node": "telegram", "audience_set": ROUND_A, "turn_id": "t1"}),
    )
}

#[test]
fn gh1305_opened_once_per_generation() {
    if !shipped() {
        return;
    }
    let on = json!({"conversation_events": "1"});
    let out = run(STAMP, open_reply(), on.clone());
    let opened = routed(&out, "opened");
    assert_eq!(opened.len(), 1, "{out:?}");
    assert_eq!(routed(&out, "turn").len(), 1, "the held turn still leaves");
    let h = &opened[0]["header"];
    assert_eq!(h["session_id"], "tg:42-new");
    assert_eq!(h["channel"], "tg:42");
    assert_eq!(h["channel_node"], "telegram");
    assert_eq!(h["audience_set"], ROUND_A);
    assert_eq!(h["opened_at"], OPENED);
    assert_eq!(opened[0]["messages"], json!([]), "no text on opened");
    // A turn of a running generation opens nothing.
    let touch = reply(
        "keeper-stamp",
        "look",
        "select",
        1,
        json!([row("tg:42-new", ROUND_A)]),
        json!({"keeper_body": "{}", "audience_set": ROUND_A, "turn_id": "t2"}),
    );
    let out = run(STAMP, touch, on);
    assert!(routed(&out, "opened").is_empty(), "{out:?}");
}

#[test]
fn gh1305_events_off_sends_nothing() {
    if !shipped() {
        return;
    }
    let cfg = read_json(STAMP);
    assert_eq!(cfg["params"]["conversation_events"], json!("0"));
    let off = run_raw(STAMP, open_reply(), json!({}));
    assert_eq!(
        off,
        run_raw(STAMP, open_reply(), json!({"conversation_events": "0"}))
    );
    assert!(!off.contains("opened\""), "{off}");
}

#[test]
fn gh1305_the_open_writes_the_channel_node_on_the_row() {
    if !shipped() {
        return;
    }
    let look = reply(
        "keeper-stamp",
        "look",
        "select",
        1,
        json!([]),
        json!({"keeper_body": "{}", "audience_set": ROUND_A, "turn_id": "t1",
               "channel_node": "telegram"}),
    );
    let out = run(STAMP, look, json!({}));
    let open = out
        .iter()
        .find(|m| m["header"]["phase"] == "open")
        .expect("an open");
    let op: Value = sj::from_str(open["messages"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(op["row"]["channel_node"], "telegram");
    assert_eq!(meta_of(open)["opened_at"], op["row"]["opened_at"]);
}

// ------------------------------------------------------------ inner turns at the keeper

fn inner_turn(ctx: Value) -> Value {
    let mut context = json!({"channel": "inner:orga", "inner_source": "orga"});
    for (k, v) in ctx.as_object().cloned().unwrap_or_default() {
        context[k] = v;
    }
    json!({"header": {"context": context, "hop": {"route": "in_turn"}},
           "messages": [{"origin": "user", "type": "text", "text": "a reminder"}]})
}

#[test]
fn gh1305_inner_turn_without_round_is_refused() {
    if !shipped() {
        return;
    }
    let out = run(
        STAMP,
        inner_turn(json!({"channel_node": "telegram"})),
        json!({}),
    );
    let rej = routed(&out, "reject");
    assert_eq!(rej.len(), 1, "{out:?}");
    assert_eq!(rej[0]["header"]["reject_reason"], "no_round");
    assert!(routed(&out, "kstore").is_empty(), "no generation for it");
    let out = run(
        STAMP,
        inner_turn(json!({"audience_set": ROUND_A})),
        json!({}),
    );
    let rej = routed(&out, "reject");
    assert_eq!(rej.len(), 1, "{out:?}");
    assert_eq!(rej[0]["header"]["reject_reason"], "inner_unrouted");
    // Both present: it runs like any turn.
    let out = run(
        STAMP,
        inner_turn(json!({"audience_set": ROUND_A, "channel_node": "telegram"})),
        json!({}),
    );
    assert_eq!(routed(&out, "kstore").len(), 1, "{out:?}");
}

// ------------------------------------------------------------ silence

fn answer_doc(ctx: Value, text: &str) -> Value {
    json!({"header": {"context": ctx, "hop": {"finish_reason": "stop"}},
           "messages": [{"origin": "assistant", "type": "text", "text": text}]})
}

#[test]
fn gh1305_empty_inner_answer_is_silence_with_a_receipt() {
    if !shipped() {
        return;
    }
    let on = json!({"conversation_events": "1"});
    let inner = json!({"inner_source": "orga", "turn_id": "i1"});
    let out = run(DISPATCHER, answer_doc(inner.clone(), "  \n "), on.clone());
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0]["header"]["route"], "answer");
    assert_eq!(out[0]["header"]["silent"], "1");
    // The knob off (the shipped default): silent all the same -- nothing is
    // said -- but marked "0", so no receipt leaves the rim.
    let cfg = read_json(DISPATCHER);
    assert_eq!(cfg["params"]["conversation_events"], json!("0"));
    let off = run(DISPATCHER, answer_doc(inner.clone(), " "), json!({}));
    assert_eq!(off[0]["header"]["silent"], "0", "{off:?}");
    // A said inner answer is no silence; a person's empty answer is as before.
    let said = run(DISPATCHER, answer_doc(inner, "Shall I?"), on.clone());
    assert!(said[0]["header"].get("silent").is_none(), "{said:?}");
    assert_eq!(
        run_raw(
            DISPATCHER,
            answer_doc(json!({"turn_id": "t"}), " "),
            on.clone()
        ),
        run_raw(
            DISPATCHER,
            answer_doc(json!({"turn_id": "t", "inner_source": ""}), " "),
            json!({})
        )
    );
    // At the rim: the marked answer reaches the collector on today's edge,
    // the mark on the hop; the collector's marked, empty answer leaves as
    // `inner_silent`, never on `answer`; a "0" answer never comes back out
    // of the collector at all (the fan-in only drops its round).
    let t = table("/t", TALKY);
    let to_col = decide(
        &t,
        "/t/dispatcher",
        &json!({}),
        &json!({"route": "answer", "silent": "1"}),
    );
    assert_eq!(to_col.len(), 1);
    assert_eq!(to_col[0].0, "/t/collector");
    assert_eq!(to_col[0].1.hop["route"], "in_answer");
    assert_eq!(to_col[0].1.hop["silent"], "1");
    let ctx = json!({"inner_source": "orga", "turn_id": "i1", "channel_node": "telegram"});
    let out = decide(
        &t,
        "/t/collector",
        &ctx,
        &json!({"route": "answer", "silent": "1"}),
    );
    assert_eq!(
        out.len(),
        1,
        "{:?}",
        out.iter().map(|d| &d.1.hop).collect::<Vec<_>>()
    );
    assert_eq!(out[0].0, "/t");
    assert_eq!(out[0].1.hop["route"], "inner_silent");
    assert_eq!(out[0].1.hop["inner_source"], "orga");
    assert_eq!(out[0].1.hop["turn_id"], "i1");
    // A said answer keeps today's single road.
    let out = decide(&t, "/t/collector", &json!({}), &json!({"route": "answer"}));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].1.hop["route"], "answer");
}

#[test]
fn gh1305_a_silent_answer_ends_its_round_and_says_nothing() {
    if !shipped() {
        return;
    }
    let doc = |silent: &str| {
        json!({"header": {"context": {"turn_id": "i1", "session_id": "inner:orga-1",
                                      "inner_source": "orga"},
                          "hop": {"route": "in_answer", "finish_reason": "stop",
                                  "silent": silent}},
               "messages": [{"origin": "assistant", "type": "text", "text": " "}]})
    };
    for (silent, answers) in [("1", 1usize), ("0", 0usize)] {
        let out = run(COLLECTOR, doc(silent), json!({}));
        let ans = routed(&out, "answer");
        assert_eq!(ans.len(), answers, "{silent}: {out:?}");
        if answers == 1 {
            assert_eq!(ans[0]["header"]["silent"], "1");
            assert_eq!(ans[0]["messages"], json!([]), "nothing said");
        }
        let drop = routed(&out, "cstore");
        assert_eq!(drop.len(), 1, "the round ends either way: {out:?}");
        assert!(
            drop[0]["messages"].to_string().contains("c-drop-round"),
            "{out:?}"
        );
    }
}

// ------------------------------------------------------------ the edges

fn table(base: &str, rel: &str) -> EdgeTable {
    let hp: HiveParams =
        sj::from_value(read_json(rel)["params"].clone()).unwrap_or_else(|e| panic!("{rel}: {e}"));
    let mut t = EdgeTable::new();
    for spec in &hp.graph.edges {
        let spec: &EdgeSpec = spec;
        let at = |ep: &str| match ep {
            "." => base.to_string(),
            other => format!("{base}/{}", other.trim_start_matches("./")),
        };
        let mut e = Edge {
            id: Uuid::now_v7(),
            from: Path::new(&at(&spec.from)),
            to: Path::new(&at(&spec.to)),
            condition: spec.condition.as_deref().map(|src| {
                meclaw_colony::cel_eval::parse_condition(src)
                    .unwrap_or_else(|e| panic!("condition {src:?}: {e}"))
            }),
            modifier: None,
            is_default: spec.is_default,
            lane: spec.lane.clone(),
            tap: spec.tap,
        };
        e.modifier = spec.modifier.as_ref().map(|m| {
            meclaw_colony::cel_eval::parse_modifier(m)
                .unwrap_or_else(|(k, e)| panic!("{rel}: modifier {k}: {e}"))
        });
        t.insert(e);
    }
    t
}

fn decide(t: &EdgeTable, from: &str, ctx: &Value, hop: &Value) -> Vec<(String, Headers)> {
    let hs = Headers::from_parts(
        ctx.as_object().cloned().expect("context"),
        hop.as_object().cloned().expect("hop"),
    );
    apply_edges(t, &Path::new(from), &hs)
        .into_iter()
        .map(|d| (d.target.as_str().to_string(), d.headers_out))
        .collect()
}

#[test]
fn gh1305_inner_turn_enters_on_its_own_channel() {
    if !shipped() {
        return;
    }
    let t = table("/t", TALKY);
    let ctx = json!({"channel": "tg:42", "audience_set": ROUND_A});
    let out = decide(
        &t,
        "/t",
        &ctx,
        &json!({"route": "in_turn", "kind": "inner", "inner_source": "orga",
                "deliver_channel_node": "telegram"}),
    );
    assert_eq!(
        out.len(),
        1,
        "one door: {:?}",
        out.iter().map(|d| &d.0).collect::<Vec<_>>()
    );
    assert_eq!(out[0].0, "/t/session-keeper");
    let c = &out[0].1.context;
    assert_eq!(c["channel"], "inner:orga");
    assert_eq!(c["channel_node"], "telegram");
    assert_eq!(c["inner_source"], "orga");
    assert_eq!(c["audience_set"], ROUND_A, "the round is the one handed in");
    // Without the node the door still opens -- the keeper refuses it by name.
    let out = decide(
        &t,
        "/t",
        &ctx,
        &json!({"route": "in_turn", "kind": "inner", "inner_source": "orga"}),
    );
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].1.context["channel_node"], "");
}

#[test]
fn gh1305_no_kind_inner_means_todays_bytes() {
    if !shipped() {
        return;
    }
    let t = table("/t", TALKY);
    let ctx = json!({"channel": "tg:42", "audience_set": ROUND_A, "channel_node": "telegram"});
    for hop in [
        json!({"route": "in_turn"}),
        json!({"route": "in_turn", "kind": "person"}),
    ] {
        let out = decide(&t, "/t", &ctx, &hop);
        assert_eq!(out.len(), 1, "{hop}");
        assert_eq!(out[0].0, "/t/session-keeper");
        assert_eq!(
            Value::Object(out[0].1.context.clone()),
            ctx,
            "the person's turn passes the door untouched"
        );
        assert_eq!(Value::Object(out[0].1.hop.clone()), hop);
    }
}

#[test]
fn gh1305_the_close_hands_its_facts_to_the_curator() {
    if !shipped() {
        return;
    }
    let t = table("/t", TALKY);
    let hop = json!({"route": "close", "session_id": "s", "channel": "tg:42",
                     "audience_set": ROUND_A, "close_reason": "round", "opened_at": OPENED,
                     "closed_at": "x", "channel_node": "telegram"});
    let out = decide(&t, "/t/session-keeper", &json!({}), &hop);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].0, "/t/curator");
    let h = &out[0].1.hop;
    assert_eq!(h["route"], "in_close");
    for (k, v) in [
        ("close_reason", "round"),
        ("opened_at", OPENED),
        ("closed_at", "x"),
        ("channel_node", "telegram"),
    ] {
        assert_eq!(h[k], v, "{k}");
    }
    // A close from a keeper before #1305 still crosses, the facts empty.
    let old = json!({"route": "close", "session_id": "s", "channel": "tg:42",
                     "audience_set": ROUND_A});
    let out = decide(&t, "/t/session-keeper", &json!({}), &old);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].1.hop["close_reason"], "");
}

#[test]
fn gh1305_the_keeper_carries_its_facts_and_lets_opened_out() {
    if !shipped() {
        return;
    }
    let k = table("/k", KEEPER);
    for cell in ["stamp", "close"] {
        let out = decide(
            &k,
            &format!("/k/{cell}"),
            &json!({}),
            &json!({"route": "kstore", "phase": "seal", "channel": "c", "session_id": "s",
                    "keeper_body": "", "turn_id": "", "audience_set": "",
                    "keeper_meta": "{\"reason\":\"round\"}"}),
        );
        assert_eq!(out[0].0, "/k/sessions");
        assert_eq!(
            out[0].1.context["keeper_meta"], "{\"reason\":\"round\"}",
            "{cell}"
        );
        let out = decide(
            &k,
            &format!("/k/{cell}"),
            &json!({"keeper_meta": "{}", "keeper_session": "s"}),
            &json!({"route": "close"}),
        );
        assert_eq!(out[0].0, "/k");
        assert!(!out[0].1.context.contains_key("keeper_meta"), "{cell}");
    }
    let out = decide(
        &k,
        "/k/stamp",
        &json!({"keeper_meta": "{}", "keeper_body": "{}"}),
        &json!({"route": "opened"}),
    );
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].0, "/k");
    assert!(!out[0].1.context.contains_key("keeper_meta"));
    let keeper_emits: Vec<String> = read_json(KEEPER)["params"]["contract"]["emits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["route"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(keeper_emits.contains(&"opened".to_string()));
}

#[test]
fn gh1305_events_leave_the_talky_rim() {
    if !shipped() {
        return;
    }
    let t = table("/t", TALKY);
    let out = decide(
        &t,
        "/t/session-keeper",
        &json!({}),
        &json!({"route": "opened"}),
    );
    assert_eq!(
        out.iter().map(|d| d.0.as_str()).collect::<Vec<_>>(),
        vec!["/t"]
    );
    let out = decide(&t, "/t/curator", &json!({}), &json!({"route": "closed"}));
    assert_eq!(
        out.iter().map(|d| d.0.as_str()).collect::<Vec<_>>(),
        vec!["/t"]
    );
    let c = read_json(CURATOR);
    let cur = table("/c", CURATOR);
    let out = decide(
        &cur,
        "/c/writer",
        &json!({"cur_close": "{}"}),
        &json!({"route": "closed"}),
    );
    assert_eq!(
        out.iter().map(|d| d.0.as_str()).collect::<Vec<_>>(),
        vec!["/c"]
    );
    assert!(!out[0].1.context.contains_key("cur_close"));
    for (rel, cfg) in [(TALKY, read_json(TALKY)), (CURATOR, c)] {
        let emits: Vec<String> = cfg["params"]["contract"]["emits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["route"].as_str().unwrap_or("").to_string())
            .collect();
        assert!(emits.contains(&"closed".to_string()), "{rel}");
    }
}

#[test]
fn gh1305_events_cross_the_assistant_rim() {
    if !shipped() {
        return;
    }
    let a = table("/a", ASSISTANT);
    for from in ["/a/talky", "/a/talky-chat"] {
        for lane in ["opened", "closed", "inner_silent"] {
            let out = decide(&a, from, &json!({}), &json!({"route": lane}));
            assert_eq!(
                out.iter().map(|d| d.0.as_str()).collect::<Vec<_>>(),
                vec!["/a"],
                "{from} {lane}"
            );
        }
    }
    for rel in [TALKY, ASSISTANT] {
        let emits: Vec<String> = read_json(rel)["params"]["contract"]["emits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["route"].as_str().unwrap_or("").to_string())
            .collect();
        for lane in ["opened", "closed", "inner_silent"] {
            assert!(emits.contains(&lane.to_string()), "{rel}: {lane}");
        }
    }
}

// ------------------------------------------------------------ the inner answer finds its channel

const MEMBER: &str = "templates/member/config.json";

/// Review I1: the answer of an inner turn rides `context.channel_node` out of
/// talky (the inner door set it from `hop.deliver_channel_node`), through the
/// assistant, and the member's `./assistants -> ./channels` edge delivers it
/// to that channel -- with talky's own marker `inner_source` gone at its rim.
#[test]
fn gh1305_inner_answer_reaches_the_delivery_channel() {
    if !shipped() || !repo(MEMBER).is_file() {
        return;
    }
    let answer = json!({"route": "answer", "turn_id": "i1"});
    // talky: the collector's answer of the inner round leaves on `answer`.
    let t = table("/t", TALKY);
    let inner = json!({"channel": "inner:orga", "channel_node": "telegram",
                       "inner_source": "orga", "turn_id": "i1", "audience_set": ROUND_A});
    let out = decide(&t, "/t/collector", &inner, &answer);
    let rim: Vec<_> = out.iter().filter(|d| d.0 == "/t").collect();
    assert_eq!(
        rim.len(),
        1,
        "{:?}",
        out.iter().map(|d| &d.0).collect::<Vec<_>>()
    );
    let ctx = &rim[0].1.context;
    assert_eq!(
        ctx["channel_node"], "telegram",
        "the node survives talky's rim"
    );
    assert!(
        !ctx.contains_key("inner_source"),
        "talky's marker stays inside: {ctx:?}"
    );
    let ctx = Value::Object(ctx.clone());
    // assistant: out of its talky, unchanged node.
    let a = table("/a", ASSISTANT);
    let out = decide(&a, "/a/talky", &ctx, &answer);
    let rim: Vec<_> = out.iter().filter(|d| d.0 == "/a").collect();
    assert_eq!(
        rim.len(),
        1,
        "{:?}",
        out.iter().map(|d| &d.0).collect::<Vec<_>>()
    );
    assert_eq!(rim[0].1.context["channel_node"], "telegram");
    let ctx = Value::Object(rim[0].1.context.clone());
    // member: the delivery edge takes it to the channel the node names.
    let m = table("/m", MEMBER);
    let out = decide(&m, "/m/assistants", &ctx, &answer);
    let to_channels: Vec<_> = out.iter().filter(|d| d.0 == "/m/channels").collect();
    assert_eq!(
        to_channels.len(),
        1,
        "the inner answer reaches the channels: {:?}",
        out.iter().map(|d| &d.0).collect::<Vec<_>>()
    );
    assert_eq!(to_channels[0].1.context["channel_node"], "telegram");
}

// ------------------------------------------------------------ against the base

/// The base script (5bb96a731) of a cell, run over the same stdin.
fn run_base_raw(name: &str, mut doc: Value, params: Value) -> String {
    doc["params"] = params;
    let p = repo(&format!(
        "crates/meclaw-cells/tests/misc_2/gh1305_base/{name}.json"
    ));
    let out = meclaw_testing::run_shipped_script(
        &meclaw_testing::shipped_script(&p.to_string_lossy()),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    assert!(
        out.status.success(),
        "{name} (base) exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8")
}

/// What leaves the stamp toward its neighbours, minus what this strand adds
/// on purpose and always: the store bundles (`kstore`, they carry
/// `keeper_meta`) and the four facts on `close`.
fn outward(raw: &str) -> Vec<Value> {
    let all = match sj::from_str::<Value>(raw).expect("json") {
        Value::Array(a) => a,
        one => vec![one],
    };
    all.into_iter()
        .filter(|m| m["header"]["route"] != json!("kstore"))
        .map(|mut m| {
            if m["header"]["route"] == json!("close") {
                let h = m["header"].as_object_mut().expect("header");
                for k in ["close_reason", "opened_at", "closed_at", "channel_node"] {
                    h.remove(k);
                }
            }
            m
        })
        .collect()
}

/// Review M4: without the knob the stamp says what the base said -- compared
/// against the base script itself, not new code against new code.
#[test]
fn gh1305_the_stamp_off_says_what_the_base_said() {
    if !shipped() {
        return;
    }
    let person = json!({"header": {"context": {"channel": "tg:42", "audience_set": ROUND_A,
                                               "turn_id": "t1"},
                                   "hop": {"route": "in_turn"}},
                        "messages": [{"origin": "user", "type": "text", "text": "hello"}]});
    let touch = reply(
        "keeper-stamp",
        "look",
        "select",
        1,
        json!([row("tg:42-new", ROUND_A)]),
        json!({"keeper_body": "{}", "audience_set": ROUND_A, "turn_id": "t2"}),
    );
    let done = bundle(
        "seal",
        &[
            ("seal-done", "update", 1, json!([])),
            ("seal-owed", "update", 0, json!([])),
        ],
        json!({"keeper_session": "tg:42-old", "keeper_audience": ROUND_A}),
    );
    for (what, doc) in [
        ("turn", person),
        ("open", open_reply()),
        ("touch", touch),
        ("seal", done),
    ] {
        let new = run_raw(STAMP, doc.clone(), json!({}));
        let base = run_base_raw("stamp", doc, json!({}));
        assert_eq!(
            outward(&new),
            outward(&base),
            "{what}: new {new}\nbase {base}"
        );
    }
}

/// Review M4: the dispatcher without the knob, byte for byte against the base,
/// for a person's answer, an empty one and a said inner one.
#[test]
fn gh1305_the_dispatcher_off_is_the_base_byte_for_byte() {
    if !shipped() {
        return;
    }
    for (what, ctx, text) in [
        ("person", json!({"turn_id": "t"}), "Hello there."),
        ("person-empty", json!({"turn_id": "t"}), " "),
        (
            "inner-said",
            json!({"turn_id": "i1", "inner_source": "orga"}),
            "Shall I?",
        ),
    ] {
        assert_eq!(
            run_raw(DISPATCHER, answer_doc(ctx.clone(), text), json!({})),
            run_base_raw("dispatcher", answer_doc(ctx, text), json!({})),
            "{what}"
        );
    }
}

/// Review M5: an inner turn that came without an id is refused without one.
#[test]
fn gh1305_an_idless_inner_refusal_names_no_turn_id() {
    if !shipped() {
        return;
    }
    let out = run(
        STAMP,
        inner_turn(json!({"audience_set": ROUND_A})),
        json!({}),
    );
    let rej = routed(&out, "reject");
    assert_eq!(rej.len(), 1, "{out:?}");
    assert_eq!(rej[0]["header"]["reject_reason"], "inner_unrouted");
    assert!(rej[0]["header"].get("turn_id").is_none(), "{:?}", rej[0]);
}
