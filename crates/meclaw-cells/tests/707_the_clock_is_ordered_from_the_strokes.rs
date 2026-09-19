//! display-hive.md § 4.34: the curator ORDERS a stroke for every moment at which something
//! in the state changes by itself -- the `ttl_ms` of a standing view, the end of a linger,
//! the end of a fade, a `relevant_until`, and one second after a pass in which a window
//! carried `fresh` or `leaving`. No polling. `state["strokes"]` is that whole ordered list;
//! the clock cell is told the EARLIEST of them and keeps the order.
//!
//! What the clock binding has learned stays (GH #681, GH #690): the order's id is derived
//! from the SECOND it is due, so two passes that agree on the moment order the same order
//! and the clock takes the second as the same one; the order that just struck is gone from
//! the timer and is not removed; an order is never earlier than the next full second.
//!
//! This replaces `gh679_a_due_clock_sweeps_what_is_due.rs`: `next_due` computed ONE moment
//! out of the object tree and ordered bar crossings that the reference model does not have.

mod support;

use meclaw_core::serde_json::{Value, json};
use support::{Screen, component_view, library_ships, pane};

fn params() -> Value {
    json!({"linger_ms": 20000, "fade_ms": 120000,
           "screens": {"monitor": {"display_type": "monitor", "inputs": ["pointer"]}},
           "default_screen": "monitor"})
}

fn note(view_id: &str, props: Value) -> Value {
    let mut p = props;
    p["title"] = json!(view_id);
    p["topic"] = json!(format!("note:{view_id}"));
    component_view(view_id, "main", pane(view_id, p))
}

/// The timer ops of the last pass, as `(op, schedule_id)`.
fn orders(screen: &Screen) -> Vec<(String, String)> {
    screen
        .lane("due")
        .iter()
        .map(|e| {
            (
                e["op"].as_str().unwrap_or("").to_string(),
                e["schedule_id"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

/// The `at` of the one `add` of the last pass.
fn at(screen: &Screen) -> String {
    screen
        .lane("due")
        .iter()
        .find(|e| e["op"] == "add")
        .map(|e| e["at"].as_str().unwrap_or("").to_string())
        .expect("an order was added")
}

#[test]
fn the_earliest_stroke_of_the_state_is_what_the_clock_is_told() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(params());
    screen.write(
        note("n1", json!({"context": "system", "relevance": "0.9"})),
        100_000,
    );
    // S-045: a fresh window orders the next full second, because the pass after it makes
    // the window `settled` and only then does it compete by § 4.20.
    let strokes = screen.screen_state()["strokes"].clone();
    assert_eq!(
        strokes.as_array().unwrap().first().unwrap(),
        &json!(101_000),
        "the fresh second stands first: {strokes}"
    );
    assert_eq!(at(&screen), "1970-01-01T00:01:41Z");
    let added = orders(&screen).into_iter().filter(|o| o.0 == "add").count();
    assert_eq!(added, 1, "exactly one order a pass");
}

#[test]
fn the_list_is_the_whole_answer_and_the_next_moment_follows() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(params());
    screen.write(
        note("n1", json!({"context": "system", "relevance": "0.9"})),
        100_000,
    );
    // One second later the window is `settled`; what is left is the end of the linger and
    // the end of the fade -- and the clock is told the earlier of the two.
    screen.pass(json!({"kind": "stroke"}), 101_000);
    let strokes: Vec<i64> = screen.screen_state()["strokes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_i64().unwrap())
        .collect();
    assert_eq!(strokes, vec![120_000, 240_000], "linger end, then fade end");
    assert_eq!(at(&screen), "1970-01-01T00:02:00Z");
}

#[test]
fn two_passes_that_agree_on_the_moment_order_the_same_id() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // GH #681: a random id would let two passes a few milliseconds apart each add an order
    // of their own. The id is derived from the SECOND, so the second `add` is the same
    // order and the clock acknowledges it without changing anything.
    let mut screen = Screen::new(params());
    screen.write(
        note("n1", json!({"context": "system", "relevance": "0.9"})),
        100_000,
    );
    // Past the fresh second: from here the earliest stroke is the end of the linger, and
    // two passes 400 ms apart agree on it.
    screen.pass(json!({"kind": "stroke"}), 101_000);
    let first = orders(&screen);
    screen.pass(json!({"kind": "stroke"}), 101_400);
    let second = orders(&screen);
    let id_of =
        |o: &Vec<(String, String)>| o.iter().find(|x| x.0 == "add").expect("an add").1.clone();
    assert_eq!(
        id_of(&first),
        id_of(&second),
        "the same second is the same order"
    );
    // GH #690: and it is never removed and added again in one pass -- a `remove` marks the
    // timer's row removed, and an `add` of the same id right after it collided with it.
    assert!(
        !second
            .iter()
            .any(|o| o.0 == "remove" && o.1 == id_of(&second)),
        "the standing order is not removed before it is ordered again: {second:?}"
    );
}

#[test]
fn a_stroke_does_not_remove_the_order_that_struck() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // The order that fired is gone from the timer. Removing it is answered
    // `schedule_not_found`, which the hive's edge turns into `in_tick_error`.
    let mut screen = Screen::new(params());
    screen.write(
        note("n1", json!({"context": "system", "relevance": "0.9"})),
        100_000,
    );
    let struck = orders(&screen)
        .into_iter()
        .find(|o| o.0 == "add")
        .expect("an add")
        .1;
    // The next pass is the strike itself: it carries the order's own id as `struck`.
    let plan_struck = struck.clone();
    let objects = screen.held.clone();
    let plan = json!({
        "views": screen.rows, "state": screen.state, "define": [], "now": 101_000,
        "event": {"kind": "stroke"}, "struck": plan_struck,
    });
    let doc = json!({
        "params": screen.params,
        "body": {"messages": [{"origin": "tool", "type": "tool_result", "id": "d-query",
                               "text": json!({"objects": objects}).to_string()}]},
        "envelope": {"header": {
            "hop": {"operation": "query"},
            "context": {"display_origin": "read", "display_views": plan.to_string()},
        }},
    });
    let emissions = support::raw(&doc);
    let removed: Vec<&Value> = emissions
        .iter()
        .filter(|e| e["header"]["route"] == "due" && e["op"] == "remove")
        .collect();
    assert!(
        removed.iter().all(|e| e["schedule_id"] != struck.as_str()),
        "the order that struck is not removed: {removed:?}"
    );
}

#[test]
fn an_order_is_never_earlier_than_the_next_full_second() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // The timer is exact to the second and refuses an `at` in the past, so the earliest
    // order a pass can place is `now + 1000`, rounded up like `iso_z`.
    let mut screen = Screen::new(params());
    screen.write(
        note(
            "n1",
            json!({"context": "system", "relevance": "0.9", "relevant_until": "100500"}),
        ),
        100_000,
    );
    assert_eq!(at(&screen), "1970-01-01T00:01:41Z");
}

#[test]
fn an_empty_screen_orders_nothing_and_takes_its_order_back() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(params());
    screen.pass(json!({"kind": "stroke"}), 100_000);
    assert!(orders(&screen).is_empty(), "nothing due, nothing ordered");

    screen.write(
        note("n1", json!({"context": "system", "relevance": "0.9"})),
        100_000,
    );
    let standing = orders(&screen)
        .into_iter()
        .find(|o| o.0 == "add")
        .expect("an add")
        .1;
    screen.withdraw("alex", "n1");
    screen.pass(
        json!({"kind": "app_withdraw", "oid": "view.alex.n1"}),
        101_000,
    );
    // The window leaves in one more pass (§ 4.35), so one second is still ordered; the pass
    // after that has nothing left and takes the standing order back.
    screen.pass(json!({"kind": "stroke"}), 102_000);
    let last = orders(&screen);
    assert_eq!(last.len(), 1, "one op: {last:?}");
    assert_eq!(
        last[0].0, "remove",
        "the last order is taken back and none is placed"
    );
    assert_ne!(standing, "", "there was an order to take back");
}
