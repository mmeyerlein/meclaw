//! GH #679 -- a classified notice becomes a window of its sender, and a
//! channel's failure is one of them.
//!
//! The screen takes a third lane, `in_notice`: a body with a `class` out of a
//! closed list and a `text`, and the compose cell wraps it into a prose view
//! owned by the sender's `reply_to` -- title the class word, body the text,
//! relevance and `ttl_ms` from the class defaults unless the body says
//! otherwise. A channel's own failure arrives on the same lane re-stamped by
//! the member's graph, with `hop.error_code` and nothing else: the cell
//! translates the code through a table and never shows `meta.detail`. A prose
//! view written on `in_view` carries the same hints into its wrapper, and one
//! that says no context stands in its owner's.
//!
//! The script runs as a subprocess the way a `code` cell runs it. Skips when
//! `python3` is absent or the templates do not ship (R2b).

use std::io::Write;
use std::process::{Command, Stdio};

use meclaw_core::serde_json::{Value, json};

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";
const SENDER: &str = "/os/orgs/acme/members/alex/channels";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

/// Run the shipped script over one document and return every emission.
fn run(doc: &Value) -> Option<Vec<Value>> {
    let mut child = Command::new("python3")
        .arg(repo(COMPOSE))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(doc.to_string().as_bytes())
        .expect("the document reaches the script");
    let out = child.wait_with_output().expect("the script ends");
    assert!(
        out.status.success(),
        "compose.py failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let answer: Value =
        meclaw_core::serde_json::from_slice(&out.stdout).expect("the answer is JSON");
    Some(match answer {
        Value::Array(list) => list,
        one @ Value::Object(_) => vec![one],
        other => panic!("emissions are objects: {other}"),
    })
}

fn calls_of(emission: &Value) -> Vec<Value> {
    emission["messages"]
        .as_array()
        .expect("a bundle has messages")
        .iter()
        .map(|turn| {
            assert_eq!(turn["type"], "tool_call");
            meclaw_core::serde_json::from_str(turn["text"].as_str().expect("a call"))
                .expect("a call is JSON")
        })
        .collect()
}

/// A notice on `in_notice` from `SENDER`, with `hop` on top of the route.
fn notice(body: Value, hop: Value) -> Option<Vec<Value>> {
    let mut hop = hop;
    hop["route"] = json!("in_notice");
    run(&json!({
        "params": {},
        "body": body,
        "envelope": {"reply_to": SENDER, "header": {"hop": hop}},
    }))
}

/// The row the one `views` emission inserts, and the emission itself.
fn inserted(emissions: &[Value]) -> (Value, Value) {
    assert_eq!(emissions.len(), 1, "one store bundle: {emissions:?}");
    let e = &emissions[0];
    assert_eq!(e["header"]["route"], "views", "{e}");
    let legs = calls_of(e);
    assert_eq!(
        legs.iter()
            .map(|l| l["operation"].clone())
            .collect::<Vec<_>>(),
        json!(["select", "delete", "insert"])
            .as_array()
            .unwrap()
            .clone(),
        "the same bundle a view write builds"
    );
    (legs[2]["row"].clone(), e.clone())
}

fn content_of(row: &Value) -> Value {
    meclaw_core::serde_json::from_str(row["content"].as_str().expect("content is text"))
        .expect("content is JSON")
}

/// A `note` with a text becomes a prose view of the sender: relevance 0.4
/// and five minutes, as the class defaults say, under a name derived from
/// the text.
#[test]
fn a_notice_becomes_a_window_of_its_sender() {
    if !library_ships() {
        return;
    }
    let Some(out) = notice(
        json!({"class": "note", "text": "the kettle is on"}),
        json!({}),
    ) else {
        return;
    };
    let (row, e) = inserted(&out);
    assert_eq!(row["kind"], "prose");
    assert_eq!(row["owner"], SENDER);
    assert_eq!(row["region"], "main");
    let view_id = row["view_id"].as_str().expect("a view id");
    assert!(
        view_id.starts_with("notice-note-") && view_id.len() == "notice-note-".len() + 8,
        "{view_id}"
    );
    assert!(
        view_id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
        "{view_id}"
    );
    assert_eq!(row["ttl_ms"], 300000);
    let content = content_of(&row);
    assert_eq!(content["title"], "note");
    assert_eq!(content["body"], "the kettle is on");
    assert_eq!(content["class"], "note");
    assert_eq!(content["relevance"], 0.4);
    assert_eq!(
        content["context"], "~os~orgs~acme~members~alex~channels",
        "an application's notice stands in its owner's context"
    );
    // The same text twice is the same view: the second write replaces it.
    let again = notice(
        json!({"class": "note", "text": "the kettle is on"}),
        json!({}),
    )
    .expect("python3");
    assert_eq!(inserted(&again).0["view_id"], view_id);
    let request: Value = meclaw_core::serde_json::from_str(
        e["header"]["display_request"]
            .as_str()
            .expect("the request rides the hop"),
    )
    .expect("json");
    assert_eq!(request["owner"], SENDER);
    assert_eq!(request["view_id"], view_id);
}

/// A channel's failure carries only its `error_code`: the class is
/// `system_error`, the text comes out of the table, the context is `system`,
/// and the detail of the failure never reaches the screen. A code the table
/// does not know is still shown, with the code in it.
#[test]
fn a_channel_failure_is_translated_and_never_shows_its_detail() {
    if !library_ships() {
        return;
    }
    let Some(out) = notice(
        json!({"messages": [], "meta": {"detail": "socket closed 1011"}}),
        json!({"error_code": "stt_failed", "kind": "voice"}),
    ) else {
        return;
    };
    let (row, _) = inserted(&out);
    let content = content_of(&row);
    assert_eq!(content["class"], "system_error");
    assert_eq!(content["title"], "system error");
    assert_eq!(content["body"], "The microphone did not catch that.");
    assert_eq!(content["context"], "system");
    assert_eq!(content["relevance"], 0.9);
    assert_eq!(row["ttl_ms"], 60000);
    assert!(
        row["view_id"]
            .as_str()
            .is_some_and(|v| v.starts_with("notice-system-error-")),
        "the class word is spelt with hyphens in a view id: {}",
        row["view_id"]
    );
    let whole = Value::Array(out.clone()).to_string();
    assert!(!whole.contains("1011"), "the detail leaked: {whole}");
    assert!(!whole.contains("socket"), "the detail leaked: {whole}");

    let unknown =
        notice(json!({"messages": []}), json!({"error_code": "xyz_failed"})).expect("python3");
    let content = content_of(&inserted(&unknown).0);
    assert_eq!(content["body"], "A part of the colony failed: xyz_failed");
    assert_eq!(content["class"], "system_error");
}

/// A class outside the closed list is refused with `invalid_notice`, and so
/// is a notice with nothing to say; nothing is written either time.
#[test]
fn an_unknown_class_is_refused_by_name() {
    if !library_ships() {
        return;
    }
    let Some(out) = notice(json!({"class": "shout", "text": "hey"}), json!({})) else {
        return;
    };
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0]["header"]["route"], "receipt");
    assert_eq!(out[0]["receipt"]["error_code"], "invalid_notice");
    assert_eq!(out[0]["receipt"]["owner"], SENDER);
    assert_eq!(out[0]["header"]["owner"], SENDER);

    let mute = notice(json!({"class": "note"}), json!({})).expect("python3");
    assert_eq!(mute[0]["header"]["route"], "receipt");
    assert_eq!(mute[0]["receipt"]["error_code"], "invalid_notice");

    let orphan = run(&json!({
        "params": {},
        "body": {"class": "note", "text": "hey"},
        "envelope": {"header": {"hop": {"route": "in_notice"}}},
    }))
    .expect("python3");
    assert_eq!(orphan[0]["receipt"]["error_code"], "owner_unknown");
}

/// A read pass over one prose view of `alex`, on a bare screen.
fn prose_pass(content: Value) -> Option<Vec<Value>> {
    let row = json!({
        "owner": "alex", "view_id": "p", "region": "main", "ord": 0,
        "kind": "prose", "content": content.to_string(), "components": "[]",
        "ttl_ms": 0, "updated_at": 1,
    });
    run(&json!({
        "params": {},
        "body": {"messages": []},
        "envelope": {"header": {
            "hop": {"operation": "query"},
            "context": {
                "display_origin": "read",
                "display_views": json!({"views": [row], "define": [], "now": 5000}).to_string(),
            },
        }},
    }))
}

fn created(emissions: &[Value], id: &str) -> Value {
    let patch = emissions
        .iter()
        .find(|e| e["header"]["route"] == "patch")
        .expect("a patch");
    calls_of(patch)
        .into_iter()
        .find(|c| c["op"] == "object.create" && c["id"] == id)
        .unwrap_or_else(|| panic!("{id} is created"))["props"]
        .clone()
}

/// The hints a prose view carries in its `content` reach the wrapper the
/// screen renders it as, and the curator scores with them: `kitchen` at 0.8
/// is the last touched context, so the score is 0.8. A prose view that says
/// no context stands in its owner's (OR-C-Bau-6): the answer somebody just
/// wrote weighs 1.0 and is visible.
#[test]
fn a_prose_view_carries_its_hints_into_the_wrapper() {
    if !library_ships() {
        return;
    }
    let Some(out) = prose_pass(json!({
        "title": "Soup", "body": "Stir it", "context": "kitchen", "relevance": 0.8,
        "class": "important_note", "pinned": true, "relevant_until": 9000,
    })) else {
        return;
    };
    let props = created(&out, "view.alex.p");
    assert_eq!(props["context"], "kitchen");
    assert_eq!(props["relevance"], 0.8);
    assert_eq!(props["class"], "important_note");
    assert_eq!(props["pinned"], true);
    assert_eq!(props["relevant_until"], 9000);
    assert_eq!(props["score"], 0.8, "{props}");
    assert_eq!(props["state"], "relevant", "fresh, so at most relevant");

    let plain = prose_pass(json!({"title": "Hi", "body": "there"})).expect("python3");
    let props = created(&plain, "view.alex.p");
    assert_eq!(props["context"], "alex", "no hint: the owner's context");
    assert_eq!(
        props["relevance"], "",
        "no hint: written empty, read as the default"
    );
    assert_eq!(props["class"], "");
    assert_eq!(props["pinned"], false);
    assert_eq!(props["relevant_until"], 0);
    assert_eq!(props["score"], 0.5, "1.0 x 0.5: visible over a bar of 0.3");
    assert_eq!(
        props["state"], "ambient",
        "visible, under the midpoint 0.65"
    );
}

/// The README names the port and its refusal, and the code keeps both.
#[test]
fn the_readme_names_the_notice_port() {
    if !library_ships() {
        return;
    }
    let readme = std::fs::read_to_string(repo("templates/display/README.md")).expect("README");
    assert!(readme.contains("`in_notice`"), "the README names the port");
    assert!(
        readme.contains("`invalid_notice`"),
        "the README names the refusal"
    );
    let hive: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(repo("templates/display/config.json")).expect("config"),
    )
    .expect("json");
    let accepts = hive["params"]["contract"]["accepts"]
        .as_array()
        .expect("accepts");
    assert!(
        accepts.iter().any(|a| a["route"] == "in_notice"),
        "the hive accepts in_notice"
    );
    let edges = hive["params"]["graph"]["edges"].as_array().expect("edges");
    assert!(edges.contains(&json!({
        "from": ".", "to": "./compose",
        "condition": "has(hop.route) && hop.route == 'in_notice'",
    })));
}
