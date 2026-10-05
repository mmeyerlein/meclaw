//! GH #1016 — **one greeting per call**, whichever half of the channel gives it.
//!
//! Measured on a live colony (2026-09-28, an inbound call on a `freeswitch`
//! channel whose media half runs a duplex provider): the caller was greeted
//! twice, two seconds apart. Two paths each did what they were built to do:
//!
//! - the media half's `params.duplex.greeting` had the voice model greet the
//!   caller the moment the session opened (OR-L25), and
//! - the signalling half raised its arrival turn *"The caller is on the line.
//!   Greet them."* (GH #614, GH #665), which the member answered like every
//!   turn it is handed (ADR-0025). The answer came back as `in_speak`, a duplex
//!   cell appends what it is told to say, and the model said the second
//!   greeting.
//!
//! GH #614 already promised ONE greeting per inbound call, and kept that promise
//! for the cascade, where the arrival turn IS the greeting. With a duplex
//! greeting the promise breaks, and only the colony knows which half greets: a
//! `ref` to `voice` and an inline signalling script share no parameters. So the
//! signalling half is told — `params.greeting`, `turn` (the default, the
//! cascade) or `media` (the media half greets, the arrival raises no turn).
//!
//! The arrival is driven straight through the shipped script: the second pass
//! of `call_incoming`, the store's answer to the `arrival` booking, is a single
//! JSON document on stdin. No colony is needed to see what the script says.

use meclaw_core::serde_json::{self, Value, json};
use std::io::Write;
use std::process::{Command, Stdio};

const SIGNAL: &str = "templates/freeswitch/signal/config.json";
const README: &str = "templates/freeswitch/README.md";

/// Synthetic on purpose: no real caller reaches a lock.
const A_CALL: &str = "call-gh1016";
const A_NUMBER: &str = "+15550100042";
const A_USER_ID: &str = "members/synthetic-member";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn signal() -> Option<Value> {
    let raw = std::fs::read_to_string(repo(SIGNAL)).ok()?;
    Some(serde_json::from_str(&raw).expect("the signal half parses"))
}

fn skip() -> bool {
    if signal().is_none() {
        eprintln!("templates/freeswitch did not travel into this tree -- skipped (GH #49)");
        return true;
    }
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("no python3 -- skipped");
        return true;
    }
    false
}

/// The arrival pass of the shipped script under the shipped params, with
/// `greeting` set as given (`None` = the key absent, as every colony built
/// before GH #1016 has it). Returns the emissions the script printed.
fn arrive(greeting: Option<&str>) -> Vec<Value> {
    let cfg = signal().expect("checked by skip()");
    let script = cfg["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string();
    let mut params = cfg["params"].clone();
    let p = params.as_object_mut().expect("params is an object");
    p.remove("script_inline");
    p.remove("sandbox");
    if let Some(g) = greeting {
        p.insert("greeting".into(), json!(g));
    }
    // The second pass of `call_incoming`: the store answered the `arrival`
    // booking, the select beside the insert found no call running.
    let doc = json!({
        "envelope": {"header": {"hop": {}, "context": {
            "phone_origin": "book",
            "phone_phase": "arrival",
            "phone_call": A_CALL,
            "phone_number": A_NUMBER,
            "phone_user": A_USER_ID,
            "phone_verified": A_USER_ID,
        }}},
        "body": {"messages": [{"origin": "tool", "type": "tool_result",
                               "id": "b-active", "text": "[]"}]},
        "params": params,
    });
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(&script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3 runs");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(doc.to_string().as_bytes())
        .expect("the document goes in");
    let done = child.wait_with_output().expect("the script ends");
    assert!(
        done.status.success(),
        "the arrival pass exits 0: {}",
        String::from_utf8_lossy(&done.stderr)
    );
    serde_json::from_slice::<Vec<Value>>(&done.stdout).unwrap_or_else(|e| {
        panic!(
            "the script prints a list of emissions ({e}): {}",
            String::from_utf8_lossy(&done.stdout)
        )
    })
}

fn route(m: &Value) -> &str {
    m["header"]["route"].as_str().unwrap_or_default()
}

fn turns(out: &[Value]) -> Vec<&Value> {
    out.iter().filter(|m| route(m) == "turn").collect()
}

fn accepted(out: &[Value]) -> &Value {
    out.iter()
        .find(|m| route(m) == "call_accepted")
        .unwrap_or_else(|| panic!("the call is taken and says so: {out:#?}"))
}

/// The e28 pattern, repaired: the media half greets, so the arrival is booked,
/// handed to the media half and receipted — and raises no turn a member would
/// answer with a second greeting.
#[test]
fn a_media_greeting_raises_no_arrival_turn() {
    if skip() {
        return;
    }
    let out = arrive(Some("media"));
    assert!(
        turns(&out).is_empty(),
        "with `greeting: media` the media half's own greeting is the ONE greeting \
         of this call; an arrival turn is answered (ADR-0025) and the answer is \
         spoken as a second one (measured 2026-09-28: two greetings two seconds \
         apart). Got: {out:#?}"
    );
    // Everything that is not a sentence stays exactly as it was.
    accepted(&out);
    assert!(
        out.iter()
            .any(|m| route(m) == "book" && m["header"]["phase"] == "live"),
        "the row still goes live: {out:#?}"
    );
    assert!(
        out.iter()
            .any(|m| route(m) == "in_session" && m["header"]["user_id"] == A_USER_ID),
        "the media half is still told who is speaking (GH #979): {out:#?}"
    );
}

/// The cascade is untouched: no key, or `turn`, is the arrival turn of GH #614
/// and GH #665 — the one greeting of a call whose media half cannot greet.
#[test]
fn a_turn_greeting_keeps_the_one_arrival_turn() {
    if skip() {
        return;
    }
    for greeting in [None, Some("turn")] {
        let out = arrive(greeting);
        let t = turns(&out);
        assert_eq!(
            t.len(),
            1,
            "greeting {greeting:?}: exactly one turn: {out:#?}"
        );
        assert_eq!(
            t[0]["messages"][0]["text"], "The caller is on the line. Greet them.",
            "greeting {greeting:?}: the arrival sentence of GH #665"
        );
        assert!(
            accepted(&out)["header"].get("greeting_fallback").is_none(),
            "greeting {greeting:?}: a readable value is no fallback"
        );
    }
}

/// A value nobody can read greets by turn — a caller greeted twice is a fault,
/// a caller greeted never is a worse one — and it says so on the receipt, the
/// way an unreadable `second_call` does (`hop.policy_fallback`).
#[test]
fn an_unreadable_greeting_falls_back_to_the_turn_and_says_so() {
    if skip() {
        return;
    }
    let out = arrive(Some("duplex"));
    assert_eq!(turns(&out).len(), 1, "the fallback is the turn: {out:#?}");
    assert_eq!(
        accepted(&out)["header"]["greeting_fallback"],
        "duplex",
        "and the receipt names the text nobody could read: {out:#?}"
    );
    // Case and blanks are read, not refused.
    assert!(
        turns(&arrive(Some(" Media "))).is_empty(),
        "` Media ` reads as media"
    );
}

/// Prose and mechanism in one place: the setting is declared with exactly the
/// values the script knows, and the README tells an operator when to set it.
#[test]
fn the_setting_is_declared_where_an_operator_reads_it() {
    if skip() {
        return;
    }
    let cfg = signal().expect("checked");
    let setting = &cfg["contract"]["settings"]["greeting"];
    assert_eq!(setting["default"], "turn", "{setting:#}");
    assert_eq!(setting["values"], json!(["turn", "media"]), "{setting:#}");
    assert!(
        setting["description"]
            .as_str()
            .is_some_and(|d| d.contains("duplex.greeting")),
        "the description names the media-half setting it has to agree with: {setting:#}"
    );
    let readme = std::fs::read_to_string(repo(README)).expect("README");
    assert!(
        readme.contains("`greeting`") && readme.contains("GH #1016"),
        "the README says when a colony sets `greeting: media`"
    );
}
