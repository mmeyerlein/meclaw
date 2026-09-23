//! GH #679 -- a classified notice becomes a window of its sender, and a
//! channel's failure is one of them.
//!
//! The screen takes a third lane, `in_notice`: a body with a `class` out of a
//! closed list and a `text`, and the compose cell wraps it into a prose view
//! owned by the sender's `reply_to` -- title the class word, body the text,
//! relevance and `ttl_ms` from the class defaults unless the body says
//! otherwise. A channel's own failure arrives on the same lane re-stamped by
//! the member's graph, with `hop.error_code` and nothing else: the cell
//! translates the code through a table and never shows `meta.detail`.
//!
//! The fourth lock is the other half of the same door: what a prose view LOOKS
//! like once the pass has drawn it. Since display 2.4.0 that is the pass's own
//! rendering values (display-hive.md § 3.1) -- `rung`, `level`, `layer`, `age`,
//! `front`, `led`, `since`, `score` -- and no longer `data-state` or
//! `data-plane`: those attributes were struck, so asking for them here would be
//! asking for a sentence that no longer exists.
//!
//! And the fifth: a prose view is a WINDOW like any other (§ 2 Window), so what
//! it declares about itself reaches the curator. A `system_error` notice at 0.9
//! outranks a silent note -- the defect this file used to pin is built.
//!
//! An accepted notice goes through `support::Screen` -- one living curator, the
//! way a `resident` code cell runs it since display 2.7.0 (GH #809) -- and is
//! read where it lands: the write bundle on the store lane and the row in the
//! store. A refused one is a single document with no state behind it and goes
//! through `support::raw`. Skips when the templates do not ship (R2b).

mod support;

use meclaw_core::serde_json::{Value, json};
use support::{Screen, library_ships, raw, repo, window_id};

const SENDER: &str = "/os/orgs/acme/members/alex/channels";

/// A screen with one output, so the knobs say nothing of their own.
fn screen() -> Screen {
    Screen::new(json!({
        "screens": {"monitor": {"display_type": "monitor", "inputs": ["pointer"]}},
        "default_screen": "monitor",
    }))
}

/// The lane `in_notice` from `SENDER`, with `hop` on top of the route.
fn notice_doc(body: Value, hop: Value) -> Value {
    let mut hop = hop;
    hop["route"] = json!("in_notice");
    json!({
        "body": body,
        "envelope": {"reply_to": SENDER, "header": {"hop": hop, "context": {}}},
    })
}

/// A notice the door refuses: one document, no curator behind it.
fn notice(body: Value, hop: Value) -> Vec<Value> {
    let mut doc = notice_doc(body, hop);
    doc["params"] = json!({});
    raw(&doc)
}

/// A notice the living curator takes: the row its write bundle inserts, and the
/// small mark that bundle carries on its hop.
fn accepted(screen: &mut Screen, body: Value, hop: Value, now: u64) -> (Value, Value) {
    screen.send(notice_doc(body, hop), now);
    assert!(
        screen.lane("receipt").is_empty(),
        "an accepted notice has nothing to say back: {:?}",
        screen.lane("receipt")
    );
    inserted(screen)
}

/// The row the one write bundle of the last message inserts, and its mark.
///
/// Since display 2.7.0 the bundle is `delete` + `insert` and nothing before it:
/// the cell holds the rows in memory, so no `select` asks what stood there
/// (GH #809). Two more legs may follow -- the curator's rest row, when its
/// content changed -- and they are the curator's, not the sender's.
fn inserted(screen: &Screen) -> (Value, Value) {
    let writes: Vec<&Value> = screen
        .hops()
        .iter()
        .filter(|h| h["route"] == "views" && !h["request"]["write"].is_null())
        .collect();
    assert_eq!(writes.len(), 1, "one write bundle: {:?}", screen.hops());
    let hop = writes[0];
    let ops: Vec<&str> = hop["ops"]
        .as_array()
        .expect("the ops of the bundle")
        .iter()
        .map(|o| o.as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        ops[..2],
        ["delete", "insert"],
        "the same bundle a view write builds: {ops:?}"
    );
    assert!(!ops.contains(&"select"), "nothing is read back: {ops:?}");
    let calls = hop["calls"].as_array().expect("the calls of the bundle");
    assert_eq!(calls[0]["where"]["owner"], SENDER, "the sender's name goes");
    let row = calls[1]["row"].clone();
    assert!(
        screen
            .table()
            .iter()
            .any(|r| r["owner"] == row["owner"] && r["view_id"] == row["view_id"]),
        "and the row landed in the store"
    );
    (row, hop["request"].clone())
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
    let mut screen = screen();
    let (row, request) = accepted(
        &mut screen,
        json!({"class": "note", "text": "the kettle is on"}),
        json!({}),
        1000,
    );
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
    // § 3.3: a numeric hint travels as TEXT. A notice is a view like any other and
    // goes on the same wire, so the door of § 4.6 takes it back.
    assert_eq!(content["relevance"], "0.4");
    assert_eq!(
        content["context"], "~os~orgs~acme~members~alex~channels",
        "an application's notice stands in its owner's context"
    );
    // The same text twice is the same view: the second write replaces it.
    let (again, _) = accepted(
        &mut screen,
        json!({"class": "note", "text": "the kettle is on"}),
        json!({}),
        2000,
    );
    assert_eq!(again["view_id"], view_id);
    // The mark on the hop says whose write it was -- enough for a refused leg to
    // become a receipt to the sender, and nothing the screen is drawn from.
    assert_eq!(request["write"]["owner"], SENDER);
    assert_eq!(request["write"]["view_id"], view_id);
    assert_eq!(request["write"]["withdraw"], false);
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
    let mut screen = screen();
    let (row, _) = accepted(
        &mut screen,
        json!({"messages": [], "meta": {"detail": "socket closed 1011"}}),
        json!({"error_code": "stt_failed", "kind": "voice"}),
        1000,
    );
    let content = content_of(&row);
    assert_eq!(content["class"], "system_error");
    assert_eq!(content["title"], "system error");
    assert_eq!(content["body"], "The microphone did not catch that.");
    assert_eq!(content["context"], "system");
    assert_eq!(content["relevance"], "0.9");
    assert_eq!(row["ttl_ms"], 60000);
    assert!(
        row["view_id"]
            .as_str()
            .is_some_and(|v| v.starts_with("notice-system-error-")),
        "the class word is spelt with hyphens in a view id: {}",
        row["view_id"]
    );
    // Nowhere: not in what the cell said, not in what it sent the store or the
    // display, not in the store.
    let whole = json!([screen.last, screen.hops(), screen.table()]).to_string();
    assert!(!whole.contains("1011"), "the detail leaked: {whole}");
    assert!(
        !whole.contains("socket closed"),
        "the detail leaked: {whole}"
    );

    let (unknown, _) = accepted(
        &mut screen,
        json!({"messages": []}),
        json!({"error_code": "xyz_failed"}),
        2000,
    );
    let content = content_of(&unknown);
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
    let out = notice(json!({"class": "shout", "text": "hey"}), json!({}));
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0]["header"]["route"], "receipt");
    assert_eq!(out[0]["receipt"]["error_code"], "invalid_notice");
    assert_eq!(out[0]["receipt"]["owner"], SENDER);
    assert_eq!(out[0]["header"]["owner"], SENDER);

    let mute = notice(json!({"class": "note"}), json!({}));
    assert_eq!(mute[0]["header"]["route"], "receipt");
    assert_eq!(mute[0]["receipt"]["error_code"], "invalid_notice");

    let orphan = raw(&json!({
        "params": {},
        "body": {"class": "note", "text": "hey"},
        "envelope": {"header": {"hop": {"route": "in_notice"}}},
    }));
    assert_eq!(orphan[0]["receipt"]["error_code"], "owner_unknown");
}

/// One prose row of `alex`, the shape `notice_row` writes: the hints stand
/// FLAT in the content beside `title` and `body`, not as the props of a
/// window node.
fn prose_row(content: Value) -> Value {
    json!({
        "owner": "alex", "view_id": "p", "region": "main", "ord": 0,
        "kind": "prose", "content": content.to_string(), "components": "[]",
        "ttl_ms": 0, "updated_at": 1,
    })
}

/// That row written through the door at 5000, on a bare screen with one output.
/// The cell reads the hints off the row the way `hints_of_row` does: a prose row
/// is FLAT, so its content IS the hints and there is no window node to unwrap --
/// an empty view is exactly the defect the test below pins as fixed.
fn drawn(content: Value) -> Screen {
    let mut screen = screen();
    screen.write(prose_row(content), 5000);
    screen
}

/// The props the pass writes on a prose window are the RENDERING values of
/// display-hive.md § 3.1 -- systemwide, the same on every output -- plus the
/// title and body out of the content. `data-state` and `data-plane` are gone
/// with display 2.4.0: what a window is, `rung` says; where it stands,
/// `level` says.
#[test]
fn a_prose_view_wears_the_passs_rendering_values() {
    if !library_ships() {
        return;
    }
    let screen = drawn(json!({
        "title": "Soup", "body": "Stir it", "context": "kitchen", "relevance": "0.8",
        "class": "important_note", "pinned": true, "relevant_until": "9000",
    }));
    let oid = window_id("alex", "p");
    let props = screen
        .props(&oid)
        .unwrap_or_else(|| panic!("{oid} is drawn"));

    assert_eq!(props["title"], "Soup");
    assert_eq!(props["body"], "Stir it");
    assert_eq!(props["owner"], "alex");
    assert_eq!(props["view_id"], "p");
    assert_eq!(props["region"], "main", "the region it was written into");

    // The eight rendering values, each present and each a string: the template
    // language reads an `int 0` as empty, and `data-level=""` matches no rule,
    // so a number that is drawn travels as text (§ 3.3).
    for key in [
        "rung", "level", "layer", "age", "front", "led", "since", "score",
    ] {
        assert!(
            props[key].is_string(),
            "`{key}` is no rendering value on a prose window: {props}"
        );
    }
    assert_eq!(
        props["rung"], "relevant",
        "0.5 x 0.8 x 1, over the bar of 0.3"
    );
    assert_eq!(props["level"], "1", "open, so it stands on the canvas");
    assert_eq!(props["layer"], "canvas", "and on the canvas ladder");
    assert_eq!(props["age"], "fresh", "written in this pass");
    assert_eq!(props["since"], "5000", "the pass's own `now`");
    assert_eq!(props["pinned"], "1", "the hint it declared");

    // The struck attributes: the template does not name them any more, so a
    // window that still carried them would be drawing against a sheet that has
    // no rule left for any of them.
    let script = std::fs::read_to_string(repo(support::COMPOSE)).expect("the script ships");
    let template = script
        .split_once("PROSE_TEMPLATE = (")
        .expect("the prose template is in the script")
        .1
        .split_once("\n)")
        .expect("and it ends")
        .0;
    for gone in [
        "data-state",
        "data-plane",
        "data-modal",
        "data-on-canvas",
        "data-screen",
        "data-profile",
    ] {
        assert!(
            !template.contains(gone),
            "`{gone}` was struck with display 2.4.0: {template}"
        );
    }
    for stays in ["data-rung", "data-level", "data-layer", "data-age"] {
        assert!(template.contains(stays), "`{stays}` draws it: {template}");
    }

    // And the curator's own memory stands in the cell, not on the object: the
    // rendering above is a rendering of it (§ 3.1).
    assert_eq!(screen.curator(&oid, "rung"), "relevant");
    assert_eq!(screen.curator(&oid, "open"), true);
    assert_eq!(screen.curator(&oid, "present"), true);
}

/// A prose view is a WINDOW (§ 2 Window), so the hints it declares reach the
/// curator: `hints_of_row` reads a `prose` row FLAT, because `notice_row`
/// writes `context`, `relevance` and `class` flat into the content.
///
/// This used to be the other way round -- the lock pinned the defect that a
/// notice saying it is loud scored exactly like one saying nothing. It is
/// built: a `system_error` at 0.9 outranks a silent note, and that is what is
/// pinned here now.
#[test]
fn a_prose_views_hints_reach_the_curator() {
    if !library_ships() {
        return;
    }
    let loud = drawn(json!({
        "title": "system error", "body": "The microphone did not catch that.",
        "context": "system", "relevance": "0.9", "class": "system_error",
    }));
    let silent = drawn(json!({"title": "Hi", "body": "there"}));
    let oid = window_id("alex", "p");

    // 0.5 (an unnamed context weighs 0.5) x 0.9 x 1 against the floor's own
    // 0.5 x 0.5 x 1 for a view that declares no relevance at all (§ 4.14).
    assert_eq!(
        loud.curator(&oid, "score"),
        0.45,
        "the declared 0.9 is read"
    );
    assert_eq!(
        silent.curator(&oid, "score"),
        0.25,
        "and the default is 0.5"
    );
    assert_eq!(loud.props(&oid).expect("drawn")["score"], "0.45");
    assert_eq!(silent.props(&oid).expect("drawn")["score"], "0.25");

    // And the difference is not academic: the loud one crosses the bar and
    // stands large, the silent one stays a tile.
    assert_eq!(loud.curator(&oid, "rung"), "relevant");
    assert_eq!(silent.curator(&oid, "rung"), "ambient");
    assert_eq!(loud.props(&oid).expect("drawn")["level"], "1");
    assert_eq!(silent.props(&oid).expect("drawn")["level"], "0");
}

/// The README names the port, and the hive's own contract keeps it.
///
/// It no longer names `invalid_notice`: the README was recut against
/// display-hive.md and carries no list of `error_code` strings at all, so an
/// assert on one would pin a sentence that is not there. The refusal itself is
/// pinned where it is produced, by `an_unknown_class_is_refused_by_name`.
#[test]
fn the_readme_names_the_notice_port() {
    if !library_ships() {
        return;
    }
    let readme = std::fs::read_to_string(repo("templates/display/README.md")).expect("README");
    assert!(readme.contains("`in_notice`"), "the README names the port");
    assert!(
        readme.contains("`in_notice` turns a classified message"),
        "and says what it does with one"
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
