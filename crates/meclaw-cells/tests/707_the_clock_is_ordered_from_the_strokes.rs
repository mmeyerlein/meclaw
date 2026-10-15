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
//!
//! The script runs the way a `resident` code cell runs it: one living cell over every
//! message (`support::Screen`, the curator driver, GH #809).

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
    // The next pass is the strike itself: the clock's `in_tick` carries the order's own
    // id as its `schedule_id`, and the pass reads it as `struck`.
    screen.pass(
        json!({"kind": "stroke", "struck": struck.as_str()}),
        101_000,
    );
    let removed: Vec<&Value> = screen
        .lane("due")
        .into_iter()
        .filter(|e| e["op"] == "remove")
        .collect();
    assert!(
        removed.iter().all(|e| e["schedule_id"] != struck.as_str()),
        "the order that struck is not removed: {removed:?}"
    );

    // The counter-probe: the same pass WITHOUT the strike's id takes the standing order
    // back, so the silence above is the `struck` and nothing else.
    let mut other = Screen::new(params());
    other.write(
        note("n1", json!({"context": "system", "relevance": "0.9"})),
        100_000,
    );
    other.pass(json!({"kind": "stroke"}), 101_000);
    assert!(
        other
            .lane("due")
            .iter()
            .any(|e| e["op"] == "remove" && e["schedule_id"] == struck.as_str()),
        "a stroke that names no strike removes the order it replaces: {:?}",
        other.lane("due")
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

/// The clock's answer to an op it refused, as the hive hands it to the curator: the timer
/// names the code and the order back in its header (`timer/emit.rs`, `emit_op_error_for`,
/// GH #904) and the template's edge sets the lane `in_tick_error`.
fn refusal(code: &str, schedule_id: &str) -> Value {
    json!({
        "body": {"messages": [], "meta": {"detail": format!("{code}: refused")}},
        "envelope": {"header": {"hop": {
            "route": "in_tick_error", "msg_type": "timer_op_error",
            "error_code": code, "schedule_id": schedule_id,
        }, "context": {}}},
    })
}

/// The id of the one `add` of the last pass.
fn added(screen: &Screen) -> String {
    orders(screen)
        .into_iter()
        .find(|o| o.0 == "add")
        .expect("an order was added")
        .1
}

#[test]
fn an_order_refused_as_past_runs_its_pass_at_once() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // Wave validation 2, strand v2-flake-b01: the fresh second leaves with one second of
    // lead; under a full suite on a 12-vCPU lane it reached the clock late, the clock
    // refused it (`at_in_past`) and the curator dropped the refusal -- no order stood, no
    // pass came, and the fresh window never settled (710 red, "the note settles").
    let mut screen = Screen::new(params());
    screen.write(
        note("n1", json!({"context": "system", "relevance": "0.9"})),
        100_000,
    );
    let refused = added(&screen);
    assert_eq!(screen.curator("view.alex.n1", "age"), json!("fresh"));

    screen.send(refusal("at_in_past", &refused), 102_500);
    assert_eq!(
        screen.curator("view.alex.n1", "age"),
        json!("settled"),
        "the refused moment has come, so its pass runs now"
    );
    let next = orders(&screen);
    assert!(
        next.iter().any(|o| o.0 == "add" && o.1 != refused),
        "the pass orders what is due next: {next:?}"
    );
    assert!(
        !next.iter().any(|o| o.0 == "remove" && o.1 == refused),
        "the clock never stored the refused order, so nothing takes it back: {next:?}"
    );
    assert!(
        screen.stderr().contains(&refused),
        "the refusal is said, not swallowed: {:?}",
        screen.stderr()
    );
}

#[test]
fn an_order_refused_as_past_twice_is_not_ordered_a_third_time() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // A pass that runs before the refused second (`relevant_until` inside it) orders the
    // same second again -- the same order (GH #681). Refused once more, it is not ordered a
    // third time: a host too loaded for any lead must not turn refusals into a loop.
    let mut screen = Screen::new(params());
    screen.write(
        note(
            "n1",
            json!({"context": "system", "relevance": "0.9", "relevant_until": "100500"}),
        ),
        100_000,
    );
    let refused = added(&screen);

    screen.send(refusal("at_in_past", &refused), 100_000);
    assert_eq!(
        added(&screen),
        refused,
        "the pass of the refusal orders the same second once more"
    );

    screen.send(refusal("at_in_past", &refused), 100_000);
    assert!(
        orders(&screen).is_empty() && screen.patch().is_empty(),
        "the second refusal of one order orders nothing: {:?}",
        orders(&screen)
    );
    assert!(
        screen.stderr().contains(&refused),
        "and it is a finding on stderr, not silence: {:?}",
        screen.stderr()
    );
}

#[test]
fn the_expected_refusals_stay_silent_and_the_orders_stay_as_they_were() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // The normal path is the same order sequence as before the fix -- one `add` per pass,
    // the same second, no extra message, nothing on stderr -- and the refusals the curator
    // expects stay silent: a `remove` of an order that already struck
    // (`schedule_not_found`) and a repeated `add` of a standing order (`schedule_id_exists`).
    let mut screen = Screen::new(params());
    screen.write(
        note("n1", json!({"context": "system", "relevance": "0.9"})),
        100_000,
    );
    let first = orders(&screen);
    assert_eq!(first.len(), 1, "one op: {first:?}");
    assert_eq!(first[0].0, "add");
    assert_eq!(
        at(&screen),
        "1970-01-01T00:01:41Z",
        "the lead is one second"
    );
    assert_eq!(screen.stderr(), "");

    screen.pass(
        json!({"kind": "stroke", "struck": first[0].1.as_str()}),
        101_000,
    );
    let second = orders(&screen);
    assert_eq!(second.len(), 1, "one op: {second:?}");
    assert_eq!(second[0].0, "add");
    assert_eq!(at(&screen), "1970-01-01T00:02:00Z");
    assert_eq!(screen.stderr(), "");

    for (code, id) in [
        ("schedule_not_found", first[0].1.as_str()),
        ("schedule_id_exists", second[0].1.as_str()),
    ] {
        screen.send(refusal(code, id), 101_200);
        assert!(
            screen.last.is_empty() && screen.hops().is_empty(),
            "`{code}` sends nothing: {:?}",
            screen.last
        );
        assert_eq!(screen.stderr(), "", "`{code}` is expected and says nothing");
    }
}

#[test]
fn an_add_the_clock_timed_out_is_ordered_once_more_and_no_further() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // Wave validation 2, strand v2-flake-b01 (B2): an `add` the clock answers with
    // `query_timeout` was not applied (`timer/cell.rs`, the add lane; `cell-types.md`), and
    // the curator dropped it like any refusal -- under database load no order stood and the
    // screen stood still exactly as with `at_in_past`. The pass runs at once and orders
    // whatever is due then; if that order fails as well, it is a finding and nothing is
    // ordered again, so a loaded host cannot turn timeouts into a loop.
    let mut screen = Screen::new(params());
    screen.write(
        note("n1", json!({"context": "system", "relevance": "0.9"})),
        100_000,
    );
    let refused = added(&screen);

    screen.send(refusal("query_timeout", &refused), 100_200);
    assert_eq!(
        screen.curator("view.alex.n1", "age"),
        json!("settled"),
        "the pass of the lost order runs now, and from the next pass on the window is \
         settled, whatever triggered it (display-hive.md § 4.35)"
    );
    let again = orders(&screen);
    assert!(
        again.iter().any(|o| o.0 == "add"),
        "the order that was not applied is ordered once more: {again:?}"
    );
    assert!(
        !again.iter().any(|o| o.0 == "remove" && o.1 == refused),
        "the clock never stored it, so nothing takes it back: {again:?}"
    );
    assert!(
        screen.stderr().contains(&refused) && screen.stderr().contains("query_timeout"),
        "the timeout is said, not swallowed: {:?}",
        screen.stderr()
    );

    let second = added(&screen);
    screen.send(refusal("query_timeout", &second), 100_400);
    assert!(
        orders(&screen).is_empty() && screen.patch().is_empty(),
        "the order of the rescue failed too, so nothing is ordered a third time: {:?}",
        orders(&screen)
    );
    assert!(
        screen.stderr().contains(&second),
        "and it is a finding on stderr, not silence: {:?}",
        screen.stderr()
    );
}

#[test]
fn an_add_timed_out_after_its_moment_runs_its_pass_at_once() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // The same answer as `at_in_past` once the moment has come: the pass runs now.
    let mut screen = Screen::new(params());
    screen.write(
        note("n1", json!({"context": "system", "relevance": "0.9"})),
        100_000,
    );
    let refused = added(&screen);

    screen.send(refusal("query_timeout", &refused), 102_500);
    assert_eq!(
        screen.curator("view.alex.n1", "age"),
        json!("settled"),
        "the moment of the order that was not applied has come, so its pass runs now"
    );
    assert!(
        !orders(&screen)
            .iter()
            .any(|o| o.0 == "remove" && o.1 == refused),
        "nothing takes back an order the clock never stored: {:?}",
        orders(&screen)
    );
}

#[test]
fn a_refusal_no_new_order_can_cure_is_said_and_orders_nothing() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // Every other answer the clock can give the curator's ops (`timer/cell.rs`) is said on
    // stderr and changes nothing: a `remove` that timed out (the order it names was
    // replaced; if it still strikes, that is one more pass, and the order this cell gave
    // last stands), and the refusals of a malformed or unaccepted op, which the same op
    // would meet again.
    let mut screen = Screen::new(params());
    screen.write(
        note("n1", json!({"context": "system", "relevance": "0.9"})),
        100_000,
    );
    let first = added(&screen);
    screen.pass(json!({"kind": "stroke"}), 101_000);
    let ops = orders(&screen);
    assert!(
        ops.contains(&("remove".to_string(), first.clone())),
        "the second pass takes the first order back: {ops:?}"
    );
    let standing = added(&screen);

    screen.send(refusal("query_timeout", &first), 101_200);
    assert!(
        screen.last.is_empty() && screen.hops().is_empty(),
        "a timed-out `remove` sends nothing: {:?}",
        screen.last
    );
    assert!(
        screen.stderr().contains(&first) && screen.stderr().contains("query_timeout"),
        "but it is said: {:?}",
        screen.stderr()
    );

    for code in [
        "invalid_body",
        "op_not_accepted",
        "parse_error",
        "invalid_cron",
        "invalid_params",
        "invalid_input",
        "kind_mismatch",
    ] {
        screen.send(refusal(code, &standing), 101_300);
        assert!(
            screen.last.is_empty() && screen.hops().is_empty(),
            "`{code}` sends nothing: {:?}",
            screen.last
        );
        assert!(
            screen.stderr().contains(code) && screen.stderr().contains(&standing),
            "`{code}` is said, not swallowed: {:?}",
            screen.stderr()
        );
    }
}
