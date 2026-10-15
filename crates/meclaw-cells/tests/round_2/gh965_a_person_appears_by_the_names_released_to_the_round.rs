//! GH #965 (OR-DP-76) -- the screen topic `people` reads affinity's `list`: the
//! people the round may see the NAME of, each with exactly the released name
//! parts, and nobody else.
//!
//! `{"op": "list"}` on `in_brief` runs the brief's own disclosure rule over all
//! subjects at once: one read of the disclosure rows for the asker (newest first),
//! R-AF-3 (`usable_for`: the round is a subset of the release set) before the
//! newest-row-per-path bookkeeping, then the cut view of each person and the
//! name rule of `who.name` / the slot head (`released_name`). So:
//!
//! - a person no usable row releases a name part of is ABSENT -- no entry, no
//!   empty name (and so no empty card on the screen); nobody gets a reference;
//! - a newer `redact` on the names beats an older release (newest row per path
//!   wins), and a newer row the round may not use shadows nothing;
//! - the list is alphabetical by the released name, so the topic's `person`
//!   card (`people.0`) is a stable pick;
//! - a partial release shows exactly those parts;
//! - a release to a smaller round than the one asking does not count;
//! - the asker is never one of the people; no asker or no round = `[]`;
//! - the decider's request carries the turn's text and the topic descriptions,
//!   never a person's name (R-DP-a).
//!
//! Pure script tests over the SHIPPED `affinity/brief` and `presenter/stage`;
//! the store's answers are played by the test.

use meclaw_core::serde_json::{self, Value, json};
use meclaw_testing::{emit_all, shipped_script};
use std::process::Command;

const BRIEF: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/affinity/brief/config.json"
);

const ROUND: &str = r#"["agent:sam","member:alex"]"#;

fn params() -> Value {
    let cfg: Value =
        serde_json::from_str(&std::fs::read_to_string(BRIEF).expect("brief config")).unwrap();
    let mut p = cfg["params"].clone();
    p.as_object_mut().unwrap().remove("script_inline");
    p
}

fn brief(header: Value, messages: Value) -> Vec<Value> {
    emit_all(
        &shipped_script(BRIEF),
        &json!({"target": "/m/affinity/brief", "header": header, "ttl": 64,
                "messages": messages, "params": params()}),
    )
}

fn ask_list(context: Value) -> Vec<Value> {
    brief(
        json!({"hop": {"route": "in_brief"}, "context": context}),
        json!([{"origin": "assistant", "type": "tool_call", "id": "c1",
                "text": "{\"op\": \"list\"}"}]),
    )
}

/// The store's answer to the lane's read, played back into `phase`.
fn store_answers(phase: &str, carry: &str, rows: Value) -> Vec<Value> {
    brief(
        json!({"hop": {"route": "result", "operation": "select"},
               "context": {"aff_phase": phase, "aff_carry": carry, "aff_subject": "",
                           "aff_audience": "agent:sam", "aff_channel": "*",
                           "aff_slots": "[]", "aff_subscriber": ""}}),
        json!([{"origin": "tool", "type": "tool_result", "id": "s",
                "text": rows.to_string()}]),
    )
}

fn answer_of(out: &[Value]) -> &Value {
    out.iter()
        .find(|m| m["header"]["route"] == "answer")
        .unwrap_or_else(|| panic!("no answer: {out:#?}"))
}

fn disclosure(entity: &str, path: &str, release: &str) -> Value {
    json!({"entity_id": entity, "field_path": path, "mode": "share",
           "audience": "agent:sam", "audience_set": release})
}

/// The whole lane: the list question, the disclosure read, the entity read.
fn listed(disclosures: Value, entities: Value) -> Value {
    let first = ask_list(json!({"asker": "agent:sam", "audience_set": ROUND}));
    assert_eq!(first.len(), 1, "{first:#?}");
    assert_eq!(first[0]["header"]["phase"], "list_disclosure", "{first:#?}");
    let read: Value =
        serde_json::from_str(first[0]["messages"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(read["table"], "disclosure");
    assert_eq!(
        read["where"],
        json!({"audience": {"in": ["agent:sam", "*"]}})
    );
    let second = store_answers(
        "list_disclosure",
        first[0]["header"]["carry"].as_str().unwrap(),
        disclosures,
    );
    if let Some(a) = second.iter().find(|m| m["header"]["route"] == "answer") {
        return a.clone();
    }
    let ent = second
        .iter()
        .find(|m| m["header"]["phase"] == "list_entities")
        .unwrap_or_else(|| panic!("no entity read: {second:#?}"));
    answer_of(&store_answers(
        "list_entities",
        ent["header"]["carry"].as_str().unwrap(),
        entities,
    ))
    .clone()
}

fn entity(id: &str, display: &str, first: &str, last: &str) -> Value {
    json!({"entity_id": id, "display_name": display,
           "aieos": json!({"identity": {"names": {"first": first, "last": last}}}).to_string()})
}

/// Red before GH #965: `op: list` is no op, a request without a subject is
/// denied (`no_subject`) and the answer carries no `people`.
#[test]
fn a_partial_release_shows_exactly_its_parts_and_nobody_else_appears() {
    let a = listed(
        json!([
            disclosure("peer:jonas", "aieos.identity.names.first", ROUND),
            disclosure(
                "peer:mira",
                "*",
                r#"["agent:sam","member:alex","member:bob"]"#
            ),
            // released something, but no name part
            disclosure("peer:hidden", "aieos.motivations", r#"["*"]"#),
            // released to a SMALLER round than the one asking
            disclosure("peer:narrow", "*", r#"["agent:sam"]"#),
            // the asker itself
            disclosure("agent:sam", "*", r#"["*"]"#),
        ]),
        json!([
            entity("peer:jonas", "Jonas Berg", "Jonas", "Berg"),
            entity("peer:mira", "Mira Kowalski", "Mira", "Kowalski"),
        ]),
    );
    assert_eq!(a["op"], "list");
    assert_eq!(a["ok"], true);
    let names: Vec<&str> = a["people"]
        .as_array()
        .expect("people")
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Jonas", "Mira Kowalski"], "{a:#}");
    // Audience review I-P1: a name and nothing else -- no participant
    // reference. The people listed are ABSENT ones; a hash of an identity
    // nobody released is a confirmation oracle and a link across rounds.
    for p in a["people"].as_array().unwrap() {
        let keys: Vec<&String> = p.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["name"], "a name, nothing else: {p}");
    }
    let text = a.to_string();
    for never in ["Berg", "hidden", "narrow", "peer:"] {
        assert!(!text.contains(never), "{never} leaked: {text}");
    }
}

/// A person whose name nothing released to this round is not in the answer at
/// all -- and when nobody is left, the answer is the empty list (the screen then
/// has no data and places no block, not an empty card).
#[test]
fn a_person_without_a_release_never_appears() {
    let a = listed(
        json!([disclosure("peer:hidden", "aieos.motivations", r#"["*"]"#)]),
        json!([]),
    );
    assert_eq!(a["people"], json!([]), "{a:#}");
    let mut red = disclosure("peer:jonas", "aieos.identity.names", ROUND);
    red["mode"] = json!("redact");
    let a = listed(json!([red]), json!([]));
    assert_eq!(
        a["people"],
        json!([]),
        "a redacted name releases nothing: {a:#}"
    );
}

#[test]
fn no_round_or_no_asker_is_the_empty_list() {
    for ctx in [
        json!({"asker": "agent:sam"}),
        json!({"audience_set": ROUND}),
    ] {
        let out = ask_list(ctx.clone());
        let a = answer_of(&out);
        assert_eq!(a["people"], json!([]), "{ctx}: {out:#?}");
        assert!(
            !out.iter().any(|m| m["header"]["route"] == "astore"
                && m["messages"][0]["text"]
                    .as_str()
                    .unwrap_or("")
                    .contains("disclosure")),
            "nothing is read without a round: {out:#?}"
        );
    }
}

/// R-DP-a: the decider sees the turn and the topic descriptions; the `people`
/// topic adds no person data to that call -- its data arrives after the verdict.
#[test]
fn the_deciders_request_carries_no_person() {
    let stage = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates/presenter/stage");
    let driver = r#"
import importlib.util, json, sys
d = sys.argv[1]
spec = importlib.util.spec_from_file_location("stage", d + "/stage.py")
st = importlib.util.module_from_spec(spec)
sys.argv = [d]
spec.loader.exec_module(st)
params = json.load(open(d + "/config.json", encoding="utf-8"))["params"]
known = {t["topic"]: {"manifest": t} for t in params["builtin_topics"]}
print(json.dumps(st.decide_call("who do I know", known, "s1")))
"#;
    let out = Command::new("python3")
        .arg("-c")
        .arg(driver)
        .arg(&stage)
        .output()
        .expect("python3 runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let call: Value = serde_json::from_slice(&out.stdout).expect("json");
    let decide = &call["decide"];
    assert_eq!(decide["state"], "who do I know");
    assert!(
        decide["questions"]["topic"]["options"]["people"].is_string(),
        "{decide:#}"
    );
    let keys: Vec<&String> = decide.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["questions", "state"]);
    let text = call.to_string();
    for never in ["Jonas", "Mira", "peer:", "audience", "people\":["] {
        assert!(
            !text.contains(never),
            "{never} in the decider's request: {text}"
        );
    }
}

fn redact(entity: &str, path: &str, release: &str) -> Value {
    let mut r = disclosure(entity, path, release);
    r["mode"] = json!("redact");
    r
}

fn names(a: &Value) -> Vec<String> {
    a["people"]
        .as_array()
        .unwrap_or_else(|| panic!("people: {a:#}"))
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect()
}

/// Audience review I-P2: the one way a person leaves the list again is a NEWER
/// `redact` row on the names (or above them) -- newest row per path wins, as in
/// the brief. Rows arrive newest first (the lane reads `order_by` newest).
/// Positive control: the same two rows the other way round show the name, so
/// the empty answer is the newer row's doing and not the redact row's alone.
#[test]
fn a_newer_redact_beats_an_older_release() {
    let jonas = || json!([entity("peer:jonas", "Jonas Berg", "Jonas", "Berg")]);
    for path in ["aieos.identity.names", "*"] {
        let a = listed(
            json!([
                redact("peer:jonas", path, ROUND),
                disclosure("peer:jonas", path, ROUND)
            ]),
            jonas(),
        );
        assert_eq!(
            a["people"],
            json!([]),
            "{path}: the newer redact withdraws the release: {a:#}"
        );
        let a = listed(
            json!([
                disclosure("peer:jonas", path, ROUND),
                redact("peer:jonas", path, ROUND)
            ]),
            jonas(),
        );
        assert_eq!(
            names(&a),
            ["Jonas Berg"],
            "{path}: the newer release stands over the older redact: {a:#}"
        );
    }
    // A newer redact the round may NOT use (released to a smaller round) does
    // not shadow an older usable release -- R-AF-3 before newest-wins.
    let a = listed(
        json!([
            redact("peer:jonas", "aieos.identity.names", r#"["agent:sam"]"#),
            disclosure("peer:jonas", "aieos.identity.names", ROUND),
        ]),
        jonas(),
    );
    assert_eq!(names(&a), ["Jonas Berg"], "{a:#}");
}

/// Review minor: the order is the released name's, not the entity id's, so the
/// `person` variant (`people.0`) shows the first person alphabetically.
#[test]
fn the_list_is_alphabetical_by_the_released_name() {
    let a = listed(
        json!([
            disclosure("peer:a", "*", ROUND),
            disclosure("peer:b", "*", ROUND),
            disclosure("peer:c", "aieos.identity.names.first", ROUND),
        ]),
        json!([
            entity("peer:a", "Zoe Adler", "Zoe", "Adler"),
            entity("peer:b", "Anna Weber", "Anna", "Weber"),
            entity("peer:c", "Max Roth", "Max", "Roth"),
        ]),
    );
    assert_eq!(names(&a), ["Anna Weber", "Max", "Zoe Adler"], "{a:#}");
}
