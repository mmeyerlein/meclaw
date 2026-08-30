//! GH #450 -- `hold`, the firewall's third verdict: a turn is parked whole
//! until a person answers, and it still ends on exactly one of the two lanes
//! the screen always had.
//!
//! "Exactly one of two lanes" is load-bearing: it is what makes the `pass` edge
//! provably the only route into the agent, and it is why a refused arrival
//! `mark` emits nothing rather than a second verdict. `hold` does not break
//! that promise -- it is not a third ANSWER about the turn, it is NO ANSWER
//! YET. What the `hold` lane carries is a notice; the turn itself ends later on
//! `pass` or `reject`.
//!
//! Five claims are pinned here:
//!
//! 1. THE VERDICT IS ASKED LAST. Every rule that can end the turn outright ends
//!    it first: a deny beats a hold, an allowlist refusal beats a hold, and the
//!    rate limit is measured before the turn is parked -- a held turn books its
//!    arrival, so a hold row is not a free flood channel.
//! 2. THE TURN IS PARKED, AND THE ROW IS WRITTEN BEFORE ANYONE IS TOLD. A
//!    notice naming a hold that does not exist is a lie an operator would act
//!    on, so the `hold` lane waits for the insert.
//! 3. RELEASE PUTS THE TURN BACK BYTE-IDENTICAL, and the answer is written
//!    before the turn moves -- measured end to end on a booted colony.
//! 4. AN UNANSWERED HOLD EXPIRES WITH A RECEIPT. It is never a silent parking
//!    bay: the timeout lands on the `reject` lane every parent already drains,
//!    and an expired hold can never become a `pass`.
//! 5. THE PILE IS BOUNDED TWICE, and the prose says the same numbers the code
//!    does (development-rules § 2d).

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

#[path = "support_14b.rs"]
mod support;

use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use support::{boot, recv_bounded};
use tokio::sync::mpsc;

const TEMPLATE: &str = "../../templates/firewall";
const T0: &str = "2026-08-14T10:00:00.000000Z";

// ======================================================================= SCRIPT

fn config_of(rel: &str) -> Value {
    let raw = std::fs::read_to_string(format!("{TEMPLATE}/{rel}")).expect("template config");
    meclaw_core::serde_json::from_str(&raw).expect("config json")
}

fn resolve_vars(script: &str, over: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail.find('}').expect("unterminated ${...}");
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

fn script_of(cell: &str, over: &[(&str, &str)]) -> String {
    let v = config_of(&format!("{cell}/config.json"));
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
        over,
    )
}

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

fn emit_from(cell: &str, over: &[(&str, &str)], doc: Value) -> Vec<Value> {
    let out = run_script_on_stdin(
        &script_of(cell, over),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    assert!(
        out.status.success(),
        "{cell} exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    meclaw_core::serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

fn hop_str(msg: &Value, key: &str) -> String {
    msg["header"][key].as_str().unwrap_or_default().to_string()
}

fn rule(rule_id: &str, kind: &str, field: &str, value: &str, action: &str) -> Value {
    json!({"rule_id": rule_id, "kind": kind, "field": field,
           "value": value, "action": action})
}

fn probe_body(text: &str) -> Value {
    json!({"messages": [{"origin": "user", "type": "text", "text": text}]})
}

/// The store's reply to the screen's `rules` or `rate` select.
fn screen_reply(phase: &str, body: &Value, rows: Value, hold: &str) -> Value {
    json!({
        "header": {
            "context": {"store_origin": "firewall", "fw_phase": phase,
                        "channel": "tg:42", "user_id": "42",
                        "fw_body": body.to_string(), "fw_now": T0,
                        "fw_hold": hold},
            "hop": {"operation": "select"}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "fw-x",
                      "text": rows.to_string()}]
    })
}

/// The store's reply to one of the warden's ops, dispatched back by the phase
/// the hive edge parked in context.
fn warden_reply(h: &Value, phase: &str, op: &str, rows: Value) -> Value {
    json!({
        "header": {
            "context": {"store_origin": "fwwarden", "wd_phase": phase,
                        "wd_body": h["wd_body"], "wd_meta": h["wd_meta"],
                        "wd_now": h["wd_now"]},
            "hop": {"operation": op}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "wd-x",
                      "text": rows.to_string()}]
    })
}

fn op_of(msg: &Value) -> Value {
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    meclaw_core::serde_json::from_str(text).expect("op json")
}

// ------------------------------------------------ (1) the verdict is asked last

#[test]
fn a_deny_row_beats_a_hold_row_for_the_same_sender() {
    let body = probe_body("hello");
    let rows = json!([
        rule("a-deny", "sender", "user_id", "42", "reject"),
        rule("z-hold", "sender", "user_id", "42", "hold"),
    ]);
    let out = emit_from("screen", &[], screen_reply("rules", &body, rows, ""));
    assert_eq!(out.len(), 1);
    assert_eq!(hop_str(&out[0], "route"), "reject");
    assert_eq!(hop_str(&out[0], "reject_reason"), "sender_denied");
    assert_eq!(hop_str(&out[0], "rule_id"), "a-deny");
}

#[test]
fn an_allowlist_refusal_beats_a_hold_row_too() {
    let body = probe_body("hello");
    let rows = json!([
        rule("a-allow", "sender", "user_id", "someone-else", "allow"),
        rule("z-hold", "sender", "channel", "tg:42", "hold"),
    ]);
    let out = emit_from("screen", &[], screen_reply("rules", &body, rows, ""));
    assert_eq!(hop_str(&out[0], "reject_reason"), "sender_not_allowed");
}

#[test]
fn a_matched_hold_row_rides_the_rate_hop_rather_than_ending_the_turn() {
    // The hold is DECIDED in the rules phase and APPLIED after the budget, so
    // the rate limit is measured on a turn that is going to be parked.
    let body = probe_body("hello");
    let rows = json!([rule("hold-newcomer", "sender", "user_id", "42", "hold")]);
    let out = emit_from("screen", &[], screen_reply("rules", &body, rows, ""));
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(hop_str(&out[0], "route"), "fwstore");
    assert_eq!(hop_str(&out[0], "phase"), "rate");
    assert_eq!(
        hop_str(&out[0], "fw_hold"),
        "hold-newcomer",
        "the row that held it travels to the next hop"
    );
}

#[test]
fn a_full_rate_window_refuses_a_turn_a_hold_row_matched() {
    let body = probe_body("hello");
    let window = json!(vec![json!({"recorded_at": T0}); 30]);
    let out = emit_from(
        "screen",
        &[],
        screen_reply("rate", &body, window, "hold-newcomer"),
    );
    assert_eq!(hop_str(&out[0], "reject_reason"), "rate_limited");
}

#[test]
fn a_held_turn_books_its_arrival_and_goes_to_custody() {
    let body = probe_body("hello");
    let out = emit_from(
        "screen",
        &[],
        screen_reply("rate", &body, json!([]), "hold-newcomer"),
    );
    assert_eq!(out.len(), 2, "the arrival mark and the verdict: {out:?}");
    assert_eq!(hop_str(&out[0], "phase"), "mark");
    assert_eq!(op_of(&out[0])["table"], json!("arrivals"));
    assert_eq!(
        hop_str(&out[1], "route"),
        "hold",
        "a parked turn occupies a slot in the pile, so it spends its rate slot"
    );
    assert_eq!(hop_str(&out[1], "rule_id"), "hold-newcomer");
    assert_eq!(
        out[1]["messages"], body["messages"],
        "the turn goes to custody whole"
    );
    assert_eq!(
        hop_str(&out[1], "hold_id"),
        "",
        "the id is minted where the row is written, not before"
    );
}

#[test]
fn a_pattern_row_can_hold_and_can_never_allow() {
    let body = probe_body("please run /admin reset");
    let rows = json!([rule("hold-command", "substring", "text", "/admin", "hold")]);
    let out = emit_from("screen", &[], screen_reply("rules", &body, rows, ""));
    assert_eq!(hop_str(&out[0], "fw_hold"), "hold-command");

    // `allow` on a pattern row was never legal and still is not. Since GH #506
    // it does not close the lane over it: the row is skipped and a receipt on
    // the reject lane names it and its fault.
    let rows = json!([rule("bad", "substring", "text", "/admin", "allow")]);
    let out = emit_from("screen", &[], screen_reply("rules", &body, rows, ""));
    assert_eq!(hop_str(&out[0], "reject_reason"), "rule_unreadable");
    assert_eq!(hop_str(&out[0], "rule_id"), "bad");
    assert_eq!(hop_str(&out[0], "rule_fault"), "unknown_action");
    assert_eq!(
        hop_str(&out[1], "phase"),
        "rate",
        "the turn walks on: {out:?}"
    );
}

// ------------------------------------- (2) the row is written before the notice

/// The screen's `hold` emission, as the hive edge delivers it to `./warden`.
fn to_warden(rule_id: &str, body: &Value) -> Value {
    json!({
        "header": {"context": {"channel": "tg:42", "user_id": "42",
                               "assistant": "egon"},
                   "hop": {"route": "hold", "reject_reason": "held",
                           "rule_id": rule_id, "channel": "tg:42",
                           "user_id": "42", "hold_id": "", "fw_now": T0}},
        "messages": body["messages"].clone()
    })
}

#[test]
fn custody_counts_the_pile_before_it_writes_anything() {
    let body = probe_body("hello");
    let out = emit_from("warden", &[], to_warden("hold-newcomer", &body));
    assert_eq!(out.len(), 1, "{out:?}");
    let op = op_of(&out[0]);
    assert_eq!(op["operation"], json!("select"));
    assert_eq!(op["table"], json!("held"));
    assert_eq!(
        op["where"]["status"],
        json!("held"),
        "the cap is not advice: nothing is written before the pile is counted"
    );
}

#[test]
fn the_notice_waits_for_the_insert_and_then_carries_the_turn() {
    let body = probe_body("hello");
    let read = emit_from("warden", &[], to_warden("hold-newcomer", &body));
    let insert = emit_from(
        "warden",
        &[],
        warden_reply(&read[0]["header"], "pile", "select", json!([])),
    );
    assert_eq!(op_of(&insert[0])["operation"], json!("insert"));
    let row = &op_of(&insert[0])["row"];
    assert_eq!(row["status"], json!("held"));
    assert_eq!(
        row["body"], body,
        "the held turn is stored WHOLE, not summarised"
    );
    assert_eq!(
        row["context"]["assistant"],
        json!("egon"),
        "and so is the context its ingress edge promoted"
    );

    let notice = emit_from(
        "warden",
        &[],
        warden_reply(&insert[0]["header"], "park", "insert", json!([])),
    );
    assert_eq!(notice.len(), 1);
    assert_eq!(hop_str(&notice[0], "route"), "hold");
    assert_eq!(
        hop_str(&notice[0], "hold_id"),
        row["hold_id"].as_str().unwrap()
    );
    assert!(!hop_str(&notice[0], "expires_at").is_empty());
    assert_eq!(notice[0]["messages"], body["messages"]);
}

#[test]
fn a_refused_insert_refuses_the_turn_rather_than_losing_it() {
    let body = probe_body("hello");
    let read = emit_from("warden", &[], to_warden("hold-newcomer", &body));
    let insert = emit_from(
        "warden",
        &[],
        warden_reply(&read[0]["header"], "pile", "select", json!([])),
    );
    let mut doc = warden_reply(&insert[0]["header"], "park", "insert", json!([]));
    doc["header"]["hop"]["error_code"] = json!("db_locked");
    let out = emit_from("warden", &[], doc);
    assert_eq!(hop_str(&out[0], "route"), "reject");
    assert_eq!(
        hop_str(&out[0], "reject_reason"),
        "store_refused",
        "custody has no fail-open half: a hold whose row was never written \
         would be a turn that vanished"
    );
}

// ------------------------------------------------ (4) the timeout, with receipt

fn pile_row(hold_id: &str, expires_at: &str) -> Value {
    json!({"hold_id": hold_id, "expires_at": expires_at, "channel": "tg:42",
           "user_id": "42", "rule_id": "hold-newcomer"})
}

#[test]
fn a_sweep_writes_the_expiry_first_and_emits_the_receipt_from_its_reply() {
    let sweep = emit_from(
        "warden",
        &[],
        json!({"header": {"context": {}, "hop": {"route": "in_sweep"}}, "messages": []}),
    );
    let write = emit_from(
        "warden",
        &[],
        warden_reply(
            &sweep[0]["header"],
            "sweep",
            "select",
            json!([pile_row("old1", "2020-01-01T00:00:00.000000Z")]),
        ),
    );
    assert_eq!(write.len(), 1, "the write comes first: {write:?}");
    let op = op_of(&write[0]);
    assert_eq!(op["operation"], json!("update"));
    assert_eq!(op["set"]["status"], json!("expired"));
    assert_eq!(op["where"]["hold_id"]["in"], json!(["old1"]));

    let receipt = emit_from(
        "warden",
        &[],
        warden_reply(&write[0]["header"], "expire", "update", json!([])),
    );
    assert_eq!(receipt.len(), 1);
    assert_eq!(hop_str(&receipt[0], "route"), "reject");
    assert_eq!(
        hop_str(&receipt[0], "reject_reason"),
        "hold_expired",
        "there is no silent parking bay: an unanswered hold leaves a receipt \
         on the lane every parent already drains"
    );
    assert_eq!(hop_str(&receipt[0], "hold_id"), "old1");
    assert_eq!(hop_str(&receipt[0], "rule_id"), "hold-newcomer");
}

#[test]
fn releasing_an_expired_hold_can_never_produce_a_pass() {
    let ask = emit_from("warden", &[], release_doc("h1", "release"));
    let stored = json!({"hold_id": "h1", "status": "held",
                        "expires_at": "2020-01-01T00:00:00.000000Z",
                        "channel": "tg:42", "user_id": "42",
                        "rule_id": "hold-newcomer", "reason": "held",
                        "context": {}, "body": probe_body("hello"),
                        "held_at": T0});
    let decide = emit_from(
        "warden",
        &[],
        warden_reply(&ask[0]["header"], "find", "select", json!([stored])),
    );
    assert_eq!(op_of(&decide[0])["set"]["status"], json!("expired"));
    let out = emit_from(
        "warden",
        &[],
        warden_reply(&decide[0]["header"], "decide", "update", json!([])),
    );
    assert_eq!(hop_str(&out[0], "route"), "reject");
    assert_eq!(hop_str(&out[0], "reject_reason"), "hold_expired");
}

fn release_doc(hold_id: &str, decision: &str) -> Value {
    json!({"header": {"context": {},
                      "hop": {"route": "in_release", "hold_id": hold_id,
                              "decision": decision, "decided_by": "operator"}},
           "messages": []})
}

#[test]
fn a_release_names_a_hold_or_it_is_refused() {
    for (doc, reason) in [
        (release_doc("", "release"), "release_unaddressed"),
        (release_doc("h1", "maybe"), "release_unknown_decision"),
    ] {
        let out = emit_from("warden", &[], doc);
        assert_eq!(hop_str(&out[0], "reject_reason"), reason, "{out:?}");
    }
    let ask = emit_from("warden", &[], release_doc("nope", "release"));
    let out = emit_from(
        "warden",
        &[],
        warden_reply(&ask[0]["header"], "find", "select", json!([])),
    );
    assert_eq!(hop_str(&out[0], "reject_reason"), "hold_unknown");
}

#[test]
fn a_person_can_also_say_no() {
    let ask = emit_from("warden", &[], release_doc("h1", "refuse"));
    let stored = json!({"hold_id": "h1", "status": "held",
                        "expires_at": "2099-01-01T00:00:00.000000Z",
                        "channel": "tg:42", "user_id": "42",
                        "rule_id": "hold-newcomer", "reason": "held",
                        "context": {}, "body": probe_body("hello"),
                        "held_at": T0});
    let decide = emit_from(
        "warden",
        &[],
        warden_reply(&ask[0]["header"], "find", "select", json!([stored])),
    );
    assert_eq!(op_of(&decide[0])["set"]["status"], json!("refused"));
    assert_eq!(
        op_of(&decide[0])["where"]["status"],
        json!("held"),
        "the where clause is what makes two answers about one hold produce one \
         outcome"
    );
    let out = emit_from(
        "warden",
        &[],
        warden_reply(&decide[0]["header"], "decide", "update", json!([])),
    );
    assert_eq!(hop_str(&out[0], "reject_reason"), "hold_refused");
}

// --------------------------------------------------- (5) the pile is bounded

#[test]
fn a_full_pile_refuses_the_turn_and_never_drops_it() {
    let body = probe_body("hello");
    let read = emit_from("warden", &[], to_warden("hold-newcomer", &body));
    let full: Vec<Value> = (0..100)
        .map(|i| pile_row(&format!("h{i}"), "2099-01-01T00:00:00.000000Z"))
        .collect();
    let out = emit_from(
        "warden",
        &[],
        warden_reply(&read[0]["header"], "pile", "select", json!(full)),
    );
    assert_eq!(out.len(), 1, "nothing is written: {out:?}");
    assert_eq!(hop_str(&out[0], "route"), "reject");
    assert_eq!(hop_str(&out[0], "reject_reason"), "hold_pile_full");
    assert_eq!(out[0]["messages"], body["messages"]);
}

#[test]
fn the_hold_max_knob_cannot_be_raised_past_the_hardline_ceiling() {
    let body = probe_body("hello");
    let read = emit_from(
        "warden",
        &[("FIREWALL_HOLD_MAX", "1000000")],
        to_warden("hold-newcomer", &body),
    );
    let full: Vec<Value> = (0..1024)
        .map(|i| pile_row(&format!("h{i}"), "2099-01-01T00:00:00.000000Z"))
        .collect();
    let out = emit_from(
        "warden",
        &[("FIREWALL_HOLD_MAX", "1000000")],
        warden_reply(&read[0]["header"], "pile", "select", json!(full)),
    );
    assert_eq!(hop_str(&out[0], "reject_reason"), "hardline_blocked");
    assert_eq!(hop_str(&out[0], "rule_id"), "hardline:hold-ceiling");
}

// ------------------------------------------------- drift lock, both halves

#[test]
fn the_hold_prose_says_the_numbers_the_code_says() {
    // development-rules § 2d: grep the surface AND assert the mechanism. The
    // two defaults are read out of the shipped contract, so the README cannot
    // drift away from them.
    let warden = config_of("warden/config.json");
    let ttl = warden["contract"]["settings"]["firewall_hold_ttl_ms"]["default"]
        .as_i64()
        .expect("ttl default");
    let max = warden["contract"]["settings"]["firewall_hold_max"]["default"]
        .as_i64()
        .expect("max default");

    let readme = std::fs::read_to_string(format!("{TEMPLATE}/README.md")).expect("README");
    assert!(
        readme.contains(&format!("| `FIREWALL_HOLD_TTL_MS` | `{ttl}` |")),
        "the knob table must name the shipped TTL default ({ttl})"
    );
    assert!(
        readme.contains(&format!("| `FIREWALL_HOLD_MAX` | `{max}` |")),
        "the knob table must name the shipped pile cap ({max})"
    );
    assert!(
        readme.contains(&format!("(default {max})")),
        "and the prose must derive it from the same place"
    );

    let flat = readme
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('*', "")
        .to_lowercase();
    for sentence in [
        "every turn ends on exactly one of `pass` and `reject`",
        "a held one just ends later",
        "an expired hold can never become a `pass`",
        "the row is written before anyone is told, and the answer is written before the turn moves",
    ] {
        assert!(flat.contains(sentence), "README must say: {sentence}");
    }
}

#[test]
fn the_shipped_seed_carries_one_disabled_hold_example_and_no_live_one() {
    let raw = std::fs::read_to_string(format!("{TEMPLATE}/rules/seed/rules.jsonl")).expect("seed");
    let rows: Vec<Value> = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| meclaw_core::serde_json::from_str(l).expect("seed row"))
        .filter(|v: &Value| v.get("rule_id").is_some())
        .collect();
    let holds: Vec<&Value> = rows
        .iter()
        .filter(|r| r["action"] == json!("hold"))
        .collect();
    assert_eq!(holds.len(), 1, "one example, so the shape can be read");
    assert_eq!(
        holds[0]["enabled"],
        json!(0),
        "a hold row enabled in a colony that wired no hold lane would park \
         turns nobody was told about"
    );
}

// ================================================================== COLONY

fn copy_template(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let name = entry.file_name();
        if from.is_dir() {
            copy_template(&from, &dst.join(name));
        } else if name == "config.json" || name.to_string_lossy().ends_with(".jsonl") {
            std::fs::copy(&from, dst.join(name)).unwrap();
        }
    }
}

fn template_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/firewall")
}

const RULE_SCHEMA: &str = r#"{"schema": {"rule_id": "text", "kind": "text", "field": "text", "value": "text", "action": "text", "enabled": "int", "note": "text"}}"#;

/// The wiring of § *A turn that waits for a person*: `pass` into the agent,
/// `reject` into the drain, and the notice into a place a person reads. Both
/// the drain and the notice land on `/park` here, told apart by `hop.route`.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": "./fw", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'pass'"},
        {"from": "./fw", "to": "/park",
         "condition": "has(hop.route) && hop.route == 'reject'"},
        {"from": "./fw", "to": "/park",
         "condition": "has(hop.route) && hop.route == 'hold'"}
    ]}}})
}

fn build_tree(td: &tempfile::TempDir, env: &str, rows: &[Value]) {
    let root = td.path();
    std::fs::write(root.join(".env"), env).unwrap();
    let main = root.join("main");
    std::fs::create_dir_all(&main).unwrap();
    std::fs::write(
        main.join("config.json"),
        meclaw_core::serde_json::to_string_pretty(&main_config()).unwrap(),
    )
    .unwrap();
    copy_template(&template_dir(), &main.join("fw"));
    let mut out = String::from(RULE_SCHEMA);
    for r in rows {
        out.push('\n');
        out.push_str(&r.to_string());
    }
    out.push('\n');
    std::fs::write(main.join("fw/rules/seed/rules.jsonl"), out).unwrap();
}

fn hold_row() -> Value {
    json!({"rule_id": "hold-newcomer", "kind": "sender", "field": "user_id",
           "value": "42", "action": "hold", "enabled": 1,
           "note": "GH #450 fixture"})
}

fn turn_at(text: &str, at: &str) -> Message {
    let mut hop = Map::new();
    hop.insert("route".into(), json!("in_turn"));
    hop.insert("channel".into(), json!("tg:42"));
    hop.insert("user_id".into(), json!("42"));
    hop.insert("recorded_at".into(), json!(at));
    MessageBuilder::new(Path::new("/fw"))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .hop(hop)
        .ttl(64)
        .build()
}

fn release(hold_id: &str, decision: &str) -> Message {
    let mut hop = Map::new();
    hop.insert("route".into(), json!("in_release"));
    hop.insert("hold_id".into(), json!(hold_id));
    hop.insert("decision".into(), json!(decision));
    hop.insert("decided_by".into(), json!("operator"));
    MessageBuilder::new(Path::new("/fw"))
        .body(Body::Inline(json!({"messages": []})))
        .hop(hop)
        .ttl(64)
        .build()
}

fn sweep() -> Message {
    let mut hop = Map::new();
    hop.insert("route".into(), json!("in_sweep"));
    MessageBuilder::new(Path::new("/fw"))
        .body(Body::Inline(json!({"messages": []})))
        .hop(hop)
        .ttl(64)
        .build()
}

fn hop_of<'a>(m: &'a Message, key: &str) -> Option<&'a Value> {
    m.headers.hop.get(key)
}

fn inline(m: &Message) -> &Value {
    match &m.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("inline expected"),
    }
}

async fn silent(rx: &mut mpsc::Receiver<Message>) -> bool {
    tokio::time::timeout(Duration::from_secs(3), rx.recv())
        .await
        .ok()
        .flatten()
        .is_none()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_held_turn_reaches_nobody_until_a_person_releases_it() {
    let td = tempfile::TempDir::new().unwrap();
    // The TTL is measured from the turn's OWN stamp (the clock is a seam), and
    // this fixture stamps the turn in the past on purpose, so the release test
    // buys a window wide enough that the answer is what decides, not the wall
    // clock.
    build_tree(&td, "FIREWALL_HOLD_TTL_MS=999999999999\n", &[hold_row()]);
    let (h, mut sink_rx, mut park_rx) = boot(&td).await;

    let sent = json!({"messages": [{"origin": "user", "type": "text",
                                    "text": "hello, I am new here"}]});
    h.send(turn_at("hello, I am new here", T0)).await;

    let notice = recv_bounded(&mut park_rx).await.expect("the hold notice");
    assert_eq!(hop_of(&notice, "route"), Some(&json!("hold")));
    assert_eq!(hop_of(&notice, "rule_id"), Some(&json!("hold-newcomer")));
    let hold_id = hop_of(&notice, "hold_id")
        .and_then(|v| v.as_str())
        .expect("the notice names the parked turn")
        .to_string();
    assert!(!hold_id.is_empty());
    assert_eq!(
        inline(&notice),
        &sent,
        "the notice carries the turn, so a person can see what is waiting"
    );
    assert!(
        silent(&mut sink_rx).await,
        "nothing downstream sees a held turn"
    );

    h.send(release(&hold_id, "release")).await;
    let got = recv_bounded(&mut sink_rx).await.expect("the released turn");
    assert_eq!(hop_of(&got, "route"), Some(&json!("pass")));
    assert_eq!(hop_of(&got, "hold_id"), Some(&json!(hold_id)));
    assert_eq!(
        inline(&got),
        &sent,
        "a released turn is byte-identical -- the firewall is a gate, not a \
         rewriter, however long the turn waited"
    );
    assert_eq!(
        hop_of(&got, "ctx_channel"),
        Some(&json!("tg:42")),
        "the dimensions its ingress edge promoted come back as hop keys"
    );

    // The same release again changes no row and delivers nothing a second time.
    h.send(release(&hold_id, "release")).await;
    let again = recv_bounded(&mut park_rx).await.expect("the second answer");
    assert_eq!(again.headers.hop.get("route"), Some(&json!("reject")));
    assert_eq!(
        again.headers.hop.get("reject_reason"),
        Some(&json!("hold_not_pending"))
    );
    assert!(silent(&mut sink_rx).await, "one hold, one delivery");

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unanswered_hold_expires_with_a_receipt_on_the_reject_lane() {
    let td = tempfile::TempDir::new().unwrap();
    // One millisecond of patience, measured from the turn's own stamp.
    build_tree(&td, "FIREWALL_HOLD_TTL_MS=1\n", &[hold_row()]);
    let (h, mut sink_rx, mut park_rx) = boot(&td).await;

    h.send(turn_at("hello, I am new here", T0)).await;
    let notice = recv_bounded(&mut park_rx).await.expect("the hold notice");
    let hold_id = hop_of(&notice, "hold_id")
        .and_then(|v| v.as_str())
        .expect("hold_id")
        .to_string();

    h.send(sweep()).await;
    let receipt = recv_bounded(&mut park_rx)
        .await
        .expect("the expiry receipt");
    assert_eq!(hop_of(&receipt, "route"), Some(&json!("reject")));
    assert_eq!(
        hop_of(&receipt, "reject_reason"),
        Some(&json!("hold_expired")),
        "a hold that is never answered is not a leak and not a parking bay"
    );
    assert_eq!(hop_of(&receipt, "hold_id"), Some(&json!(hold_id)));

    // And it can never be released afterwards.
    h.send(release(&hold_id, "release")).await;
    let refused = recv_bounded(&mut park_rx)
        .await
        .expect("the refused release");
    assert_eq!(refused.headers.hop.get("route"), Some(&json!("reject")));
    assert!(
        silent(&mut sink_rx).await,
        "an expired hold can never become a pass"
    );

    h.shutdown().await;
}
