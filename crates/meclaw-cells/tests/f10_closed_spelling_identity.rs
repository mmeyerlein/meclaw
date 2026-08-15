//! 0.3.x follow-up F10 -- a closure across two spellings does not hide the merge
//! it just proved (GitHub #73).
//!
//! Found on the first live run of scenario C6, during the wave-final measurement
//! of the 0.3.x follow-ups. A statement written under spelling A was closed by a
//! statement written under spelling B. That closure is the strongest evidence a
//! memory ever produces that A and B are one relation -- and it is exactly the
//! event after which the nightly identity questions stop seeing A at all: the
//! predicate inventory and the entity context are built from OPEN rows, and A is
//! open nowhere any more. The alias verdict that would unite the two spellings
//! into one chain is therefore never proposed, in precisely the case where it is
//! most certainly right.
//!
//! Two things had to change, and the second is the one that is easy to miss.
//!
//! 1. The two identity questions see the spellings of recently closed rows.
//! 2. They get a READ of their own to see them by. The scan the round already
//!    had widens on `expired_at >= delta_to - 7 days`, and `expired_at` is an
//!    EVENT time -- the instant a statement stopped being TRUE, not the instant
//!    the closure was WRITTEN. A closure written last night about a change dated
//!    last spring is outside that window, so the widening the review lane needs
//!    is the wrong bound for this question. The identity read is bounded by ROWS
//!    instead, most recently ended first: a bound that holds whatever the clock
//!    in the column says.
//!
//! Everything here runs the REAL `params.script_inline` of `dream-glue` against
//! injected store replies, so no model is called and nothing costs anything.
//! What a judge then DOES with the question it can finally see is scenario C6.

use std::io::Write;
use std::process::{Command, Stdio};

const GLUE_CONFIG: &str = "../../templates/memory-hive/dream-glue/config.json";
const RUN: &str = "run-f10";
const TO: &str = "2026-08-09T03:00:00Z";

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
    let raw = std::fs::read_to_string(GLUE_CONFIG).expect("dream-glue config");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
    )
}

/// Run the real script with a real stdin document and return the emitted messages.
fn emit(doc: serde_json::Value) -> Vec<serde_json::Value> {
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(glue_script())
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
        "dream-glue exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// One store reply as the edge delivers it: the run keys in the context, the
/// operation in the hop, the projected rows as the first message's text.
fn store_reply(phase: &str, operation: &str, rows: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "dream", "mem_phase": phase,
                        "dream_run": RUN, "dream_to": TO},
            "hop": {"operation": operation, "rows_affected": 1}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r", "text": rows.to_string()}]
    })
}

fn args_of(msg: &serde_json::Value) -> serde_json::Value {
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    serde_json::from_str(text).expect("op args")
}

fn phases(msgs: &[serde_json::Value]) -> Vec<String> {
    msgs.iter()
        .map(|m| m["header"]["phase"].as_str().unwrap_or("").to_string())
        .collect()
}

/// The op of one phase out of a fan, by the phase it hangs on.
fn op_at(msgs: &[serde_json::Value], phase: &str) -> serde_json::Value {
    msgs.iter()
        .find(|m| m["header"]["phase"] == phase)
        .map(args_of)
        .unwrap_or_else(|| panic!("no op emitted at phase {phase}: {msgs:?}"))
}

/// A closed row, as the identity read projects it: spelling A, ended by a
/// statement on spelling B.
fn closed_rows() -> serde_json::Value {
    serde_json::json!([
        {"id": "a", "subject": "user", "canonical_subject": "user",
         "predicate": "Lieblingseditor", "canonical_predicate": "Lieblingseditor",
         "claim": "favorite editor is helix", "expired_at": "2026-06-05T00:00:00Z",
         "superseded_by": "b"}
    ])
}

#[test]
fn the_identity_questions_get_a_read_of_their_own() {
    // The round used to go straight from the refusal log to the fact scan. It now
    // asks the closed rows first, because the scan's own window answers a
    // different question (which extractor closures need reviewing) with a clock
    // that cannot answer this one.
    let msgs = emit(store_reply(
        "canon-scan-fetch",
        "insert",
        serde_json::json!([]),
    ));
    assert_eq!(phases(&msgs), vec!["canon-closed"]);
    let args = args_of(&msgs[0]);
    assert_eq!(args["operation"], "select");
    assert_eq!(args["table"], "facts");
    assert_eq!(
        args["where"]["expired_at"]["is_null"],
        serde_json::json!(false),
        "the read asks for the CLOSED rows, which is the set the open scan cannot see"
    );
}

#[test]
fn the_identity_read_is_bounded_by_rows_and_never_by_the_event_clock() {
    // The half that is easy to get wrong. `expired_at` says when a statement
    // stopped being TRUE, never when somebody wrote the closure down -- so a
    // cutoff derived from `delta_to` drops exactly the closure that was written
    // last night about a change dated last spring. That is the C6 shape: the
    // closure ends a statement in June and the night runs in August.
    let args = args_of(
        &emit(store_reply(
            "canon-scan-fetch",
            "insert",
            serde_json::json!([]),
        ))[0],
    );
    let filter = &args["where"]["expired_at"];
    for forbidden in ["gte", "gt", "or_null"] {
        assert!(
            filter.get(forbidden).is_none(),
            "the identity read must not bound itself on the event clock, got {filter}"
        );
    }
    assert!(
        args["limit"].as_i64().unwrap_or(0) > 0,
        "and it is bounded, on ROWS: {args}"
    );
    assert_eq!(
        args["order_by"][0]["col"], "expired_at",
        "most recently ended first, so the bound cuts the tail and not the head: {args}"
    );
    assert_eq!(args["order_by"][0]["dir"], "desc");
}

#[test]
fn a_closed_spelling_is_parked_and_the_open_scan_still_follows() {
    // The read is a detour, never a replacement: the chain still hangs on the
    // scan, and the closed identities travel next to it in `scratch` -- the
    // meeting point the join-less store forces on every two-set question here.
    let msgs = emit(store_reply("canon-closed", "select", closed_rows()));
    let park = op_at(&msgs, "canon-parked");
    assert_eq!(park["table"], "scratch");
    assert_eq!(park["row"]["kind"], "canon-closed");
    let parked: serde_json::Value =
        serde_json::from_str(park["row"]["payload"].as_str().expect("payload")).expect("closed");
    assert_eq!(
        parked["predicates"]["Lieblingseditor"],
        serde_json::json!(["user"]),
        "the spelling the closure took out of the open set, with the subject it was used on"
    );
    assert_eq!(
        parked["context"]["user"],
        serde_json::json!(["favorite editor is helix"]),
        "and the claim, for the entity-pair question's context"
    );
    let scan = op_at(&msgs, "canon-scan");
    assert_eq!(scan["table"], "facts");
    assert_eq!(scan["operation"], "select");
}

#[test]
fn a_store_that_closed_nothing_goes_straight_on() {
    // A night on a store with no closure parks nothing and emits exactly the scan
    // it emitted before this package -- which is what lets the verdicts of such a
    // night stay pinned across it.
    let msgs = emit(store_reply("canon-closed", "select", serde_json::json!([])));
    assert_eq!(phases(&msgs), vec!["canon-scan"]);
}

/// The parked halves, as the meeting-point select hands them back.
fn meeting_point(closed: Option<serde_json::Value>) -> serde_json::Value {
    let scan = serde_json::json!({
        // the open side AFTER the closure: spelling A is nowhere any more
        "predicates": {"favorite_editor": ["user:u1"], "lives_in": ["user"]},
        "context": {"user:u1": ["favorite editor is vscode"],
                    "user": ["lives in Sonnenhof"]}
    });
    let mut rows = vec![
        serde_json::json!({"key": RUN, "kind": "canon-scan", "payload": scan.to_string()}),
        serde_json::json!({"key": RUN, "kind": "canon-pairs",
                           "payload": serde_json::json!(
                               [{"left": "user", "right": "user:u1", "score": 0.6}]).to_string()}),
    ];
    if let Some(c) = closed {
        rows.push(serde_json::json!({"key": RUN, "kind": "canon-closed",
                                     "payload": c.to_string()}));
    }
    serde_json::Value::Array(rows)
}

/// The question one night puts to the judge.
fn judge_payload(closed: Option<serde_json::Value>) -> serde_json::Value {
    let msgs = emit(store_reply("canon-ask", "select", meeting_point(closed)));
    let ask = msgs
        .iter()
        .find(|m| m["header"]["route"] == "judge")
        .unwrap_or_else(|| panic!("the night asked nothing: {msgs:?}"));
    serde_json::from_str(ask["messages"][0]["text"].as_str().expect("payload")).expect("payload")
}

#[test]
fn the_predicate_question_sees_the_spelling_the_closure_took_away() {
    // The whole of #73 in one assertion. Without the closed half the judge is
    // shown `favorite_editor` and `lives_in` and can propose nothing about the
    // spelling that just lost its last open row.
    let without = judge_payload(None);
    let spellings: Vec<&str> = without["predicates"]
        .as_array()
        .expect("predicates")
        .iter()
        .map(|p| p["predicate"].as_str().expect("predicate"))
        .collect();
    assert!(
        !spellings.contains(&"Lieblingseditor"),
        "guard: the open side really has lost the spelling, else this case proves nothing"
    );

    let with = judge_payload(Some(serde_json::json!({
        "predicates": {"Lieblingseditor": ["user"]},
        "context": {"user": ["favorite editor is helix"]}
    })));
    let spellings: Vec<&str> = with["predicates"]
        .as_array()
        .expect("predicates")
        .iter()
        .map(|p| p["predicate"].as_str().expect("predicate"))
        .collect();
    assert!(
        spellings.contains(&"Lieblingseditor") && spellings.contains(&"favorite_editor"),
        "both spellings reach the alias question, so the merge can be proposed: {spellings:?}"
    );
    assert_eq!(
        with["predicates"]
            .as_array()
            .expect("predicates")
            .iter()
            .map(|p| p["predicate"].as_str().expect("predicate"))
            .collect::<Vec<_>>(),
        {
            let mut s = spellings.clone();
            s.sort();
            s
        },
        "still sorted, so two runs over one store put the same question"
    );
}

#[test]
fn the_entity_context_gains_the_closed_side_without_losing_the_open_one() {
    // The pair question judges two nodes by what is known about each. A subject
    // whose facts were all closed by facts written under another spelling would
    // otherwise arrive as a name with nothing behind it -- and that is the one
    // pair where the answer matters most.
    let with = judge_payload(Some(serde_json::json!({
        "predicates": {"Lieblingseditor": ["user"]},
        "context": {"user": ["favorite editor is helix"]}
    })));
    let pair = &with["entity_pairs"][0];
    let left: Vec<&str> = pair["left_facts"]
        .as_array()
        .expect("left_facts")
        .iter()
        .map(|f| f.as_str().expect("fact"))
        .collect();
    assert!(
        left.contains(&"favorite editor is helix"),
        "the closed claim is context about who this side is: {left:?}"
    );
    assert!(
        left.contains(&"lives in Sonnenhof"),
        "and the open claim keeps its slot: {left:?}"
    );
}

#[test]
fn a_spelling_that_is_still_open_elsewhere_is_not_doubled() {
    // The merge is a union, so a closed row on a spelling the memory still holds
    // open changes nothing about the question -- which is what makes the plain
    // union safe: the only spelling it can add is one nothing else carries.
    let with = judge_payload(Some(serde_json::json!({
        "predicates": {"favorite_editor": ["user:u1"]},
        "context": {"user:u1": ["favorite editor is vscode"]}
    })));
    let entries: Vec<&serde_json::Value> = with["predicates"]
        .as_array()
        .expect("predicates")
        .iter()
        .filter(|p| p["predicate"] == "favorite_editor")
        .collect();
    assert_eq!(entries.len(), 1, "one entry per spelling: {entries:?}");
    assert_eq!(entries[0]["subjects"], serde_json::json!(["user:u1"]));
    let left: Vec<&str> = with["entity_pairs"][0]["right_facts"]
        .as_array()
        .expect("right_facts")
        .iter()
        .map(|f| f.as_str().expect("fact"))
        .collect();
    assert_eq!(
        left,
        vec!["favorite editor is vscode"],
        "and a claim already there is not listed twice"
    );
}
