//! GH #809 (display 2.7.0): the screen state lies in the CELL, and the store keeps only
//! what a restart cannot compute anew.
//!
//! `compose` runs `resident`: the curator's state stays in memory from one message to the
//! next, so no pass reads it out of the store and no pass writes it back. What the store
//! holds beside the app rows is ONE small rest row (`display` / `screen-rest`, OR-D3): the
//! curator's memory of each window that no app row says -- `since`, `dismissed_at`,
//! `led_until`, `verdict_cleared`, `topic_dupe`, the verdict, `withdrawn` -- plus the
//! judge, the bar, the weights and the errors already said. It is written only when that
//! content changes (OR-D4), so a clock stroke that moves nothing of it writes nothing.
//!
//! The row of display 2.5.0-2.6.x (`screen-state`, the whole state) is not written any
//! more. A killed child rebuilds its memory out of the app rows and the rest row.
//!
//! The script runs the way a `resident` code cell runs it: one living cell over every
//! message (`support::Screen`, the curator driver).

mod support;

use meclaw_core::serde_json::{Value, json};
use support::{Screen, component_view, library_ships, pane, window_id};

fn params() -> Value {
    json!({"screens": {"monitor": {"display_type": "monitor", "inputs": ["pointer"]}},
           "default_screen": "monitor"})
}

fn note(view_id: &str, relevance: &str) -> Value {
    component_view(
        view_id,
        "main",
        pane(
            view_id,
            json!({"title": view_id, "context": "system", "relevance": relevance,
                   "topic": format!("note:{view_id}")}),
        ),
    )
}

/// Three notes, one after the other. `n1` at relevance 0.5 scores 0.25 against the default
/// bar of 0.3: present, NOT open -- so a tap on it opens it and leads it (§ 5.1, § 4.19).
fn three_notes() -> Screen {
    let mut screen = Screen::new(params());
    screen.write(note("n1", "0.5"), 100_000);
    screen.write(note("n2", "0.9"), 100_500);
    screen.write(note("n3", "0.9"), 101_000);
    screen
}

/// The rows of the curator's own in the store (`owner` `display`).
fn own_rows(screen: &Screen) -> Vec<Value> {
    screen
        .table()
        .into_iter()
        .filter(|r| r["owner"] == "display")
        .collect()
}

/// The one rest row and its parsed content.
fn rest(screen: &Screen) -> (Value, Value) {
    let rows: Vec<Value> = screen
        .table()
        .into_iter()
        .filter(|r| r["owner"] == "display" && r["view_id"] == "screen-rest")
        .collect();
    assert_eq!(rows.len(), 1, "exactly one rest row: {rows:?}");
    let content: Value =
        meclaw_core::serde_json::from_str(rows[0]["content"].as_str().expect("content"))
            .expect("the rest row's content is JSON");
    (rows[0].clone(), content)
}

#[test]
fn the_store_holds_the_app_rows_and_one_rest_row() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let screen = three_notes();

    let apps: Vec<Value> = screen
        .table()
        .into_iter()
        .filter(|r| r["owner"] == "alex")
        .map(|r| r["view_id"].clone())
        .collect();
    assert_eq!(apps.len(), 3, "the three app rows stand: {apps:?}");
    assert!(
        own_rows(&screen)
            .iter()
            .all(|r| r["view_id"] != "screen-state"),
        "no state row: the state lies in the cell (GH #809): {:?}",
        own_rows(&screen)
    );
    assert_eq!(
        own_rows(&screen).len(),
        1,
        "the curator keeps one row of its own, the rest row: {:?}",
        own_rows(&screen)
    );

    // The row's shape, column for column (OR-D3).
    let (row, content) = rest(&screen);
    assert_eq!(row["kind"], "state", "{row}");
    assert_eq!(row["region"], "main", "{row}");
    assert_eq!(row["ord"], 0, "{row}");
    assert_eq!(row["components"], "[]", "{row}");
    assert_eq!(row["ttl_ms"], 0, "{row}");
    assert_eq!(
        row["updated_at"], 101_000,
        "written by the last write that changed it: {row}"
    );

    // And its content: the memory no app row carries, nothing more.
    assert_eq!(content["v"], 1, "{content}");
    for key in ["judge", "bar", "weights", "said", "views"] {
        assert!(
            !content[key].is_null(),
            "the rest row carries `{key}`: {content}"
        );
    }
    assert!(
        content["judge"].get("called").is_none(),
        "`judge.called` is set anew in every pass and is not kept (OR-D22): {content}"
    );
    let n1 = &content["views"][window_id("alex", "n1")];
    assert_eq!(n1["since"], 100_000, "the touch of the write: {content}");
    for key in [
        "dismissed_at",
        "led_until",
        "verdict_cleared",
        "topic_dupe",
        "verdict",
        "withdrawn",
    ] {
        assert!(!n1[key].is_null(), "a window keeps `{key}`: {n1}");
    }
    for key in ["rung", "level", "open", "present", "score", "rank"] {
        assert!(
            n1.get(key).is_none(),
            "`{key}` is computed anew by every pass and is not kept: {n1}"
        );
    }
    assert_eq!(
        content["views"].as_object().map(|v| v.len()),
        Some(3),
        "one entry per window: {content}"
    );
}

#[test]
fn a_stroke_leaves_the_store_as_it_was() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = three_notes();
    let before = screen.table();
    screen.pass(json!({"kind": "stroke"}), 102_000);
    assert_eq!(
        screen.table(),
        before,
        "a stroke changes nothing the rest row keeps, so it writes nothing (OR-D4)"
    );
    assert!(
        screen.hops().iter().all(|h| h["route"] != "views"),
        "not even a bundle that writes the same row again: {:?}",
        screen.hops()
    );
}

#[test]
fn a_tap_that_leads_a_window_moves_the_rest_row() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = three_notes();
    let n1 = window_id("alex", "n1");
    assert_eq!(screen.curator(&n1, "open"), json!(false), "not yet open");

    screen.pass(json!({"kind": "tap", "for": n1.as_str()}), 103_000);
    assert_eq!(
        screen.curator(&n1, "led_until"),
        json!(123_000),
        "the finger holds the window up for the linger (§ 4.19)"
    );

    // The tap wrote nothing but the rest row: delete + insert of `(display, screen-rest)`,
    // one bundle of its own, because this turn carried no app write.
    let bundles: Vec<&Value> = screen
        .hops()
        .iter()
        .filter(|h| h["route"] == "views")
        .collect();
    assert_eq!(bundles.len(), 1, "one store bundle: {bundles:?}");
    assert_eq!(
        bundles[0]["ops"],
        json!(["delete", "insert"]),
        "the rest row's identity is kept by hand (no primary key): {bundles:?}"
    );
    assert_eq!(bundles[0]["request"], json!({"rest": true}), "{bundles:?}");

    let (row, content) = rest(&screen);
    assert_eq!(row["updated_at"], 103_000, "{row}");
    assert_eq!(
        content["views"][n1.as_str()]["led_until"],
        123_000,
        "and the rest row keeps the lead: {content}"
    );
    assert_eq!(content["views"][n1.as_str()]["since"], 103_000, "{content}");
}

/// What the rest row is for: a killed child reads it back at its boot, and the curator
/// values it kept are the ones the next pass computes on.
#[test]
fn a_killed_cell_reads_its_memory_back_out_of_the_rest_row() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = three_notes();
    let n1 = window_id("alex", "n1");
    screen.pass(json!({"kind": "tap", "for": n1.as_str()}), 103_000);

    screen.kill();
    screen.pass(json!({"kind": "stroke"}), 104_000);
    let boot: Vec<Value> = screen
        .hops()
        .iter()
        .take(2)
        .map(|h| json!([h["route"], h["ops"]]))
        .collect();
    assert_eq!(
        boot,
        vec![json!(["views", ["select"]]), json!(["read", ["query"]])],
        "a fresh child reads the store once and the tree once: {:?}",
        screen.hops()
    );
    for (key, expected) in [
        ("since", json!(103_000)),
        ("led_until", json!(123_000)),
        ("dismissed_at", json!(0)),
        ("open", json!(true)),
    ] {
        assert_eq!(
            screen.curator(&n1, key),
            expected,
            "`{key}` came back out of the rest row, not guessed anew"
        );
    }
}
