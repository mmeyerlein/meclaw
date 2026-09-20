//! GH #744 — two passes in one round trip must not lose a write (display-hive.md
//! § 4.1, § 5.8, S-024).
//!
//! Every event starts a fresh read pass: `absorb` selects the table, the reply
//! computes the state, and a third leg writes the ONE state row of § 3.1. Between
//! the `select` and that write lies a full message round trip, and the curator is
//! a `code` cell with no memory between messages
//! (`crates/meclaw-cells/src/code/harness.rs`: warm == cold). So two taps that
//! arrive inside one round trip are handed the SAME state row, both compute on
//! it, and the second write overwrites the first.
//!
//! Measured on the twin `e25t` (18.09.2026,
//! `plans/welle-h3-2026-09-18/messungen/B-klickserie.md` § 3 run `M13-h3-clock`):
//! ten real mouse clicks on one tile, ten tap passes, and two of them 36 ms
//! apart both took the OPEN branch — after ten taps the window stood open where
//! § 5.8/S-024 says an even count ends put away. The same report's run
//! `M13-h3-timer` caught the other shape: three `app_write` passes 18–46 ms after
//! a tap pass carried the state from before the tap.
//!
//! `gh702_ten_taps_in_ten_seconds.rs` cannot see this: it hands every pass the
//! row the pass before it wrote, which is exactly the serialisation the colony
//! does NOT have.
//!
//! What is locked here: the state write is a compare-and-set on the `updated_at`
//! the pass read, and the reply is no longer swallowed — `rows_affected == 0`
//! means somebody wrote in between, so the same pass runs again on the new row.
//! Two taps on a closed window therefore end with the window CLOSED, whatever
//! order the colony delivered them in.
//!
//! Skips when the templates do not ship (R2b).

mod support;

use meclaw_core::serde_json::{Value, json};
use support::{Screen, component_view, library_ships, pane, raw, window_id};

/// The state-write emission of one pass: the `views` bundle marked `state`.
fn state_write(emissions: &[Value]) -> Value {
    let found: Vec<&Value> = emissions
        .iter()
        .filter(|e| {
            e["header"]["route"] == "views"
                && e["header"]["display_request"]
                    .as_str()
                    .unwrap_or("")
                    .contains("\"state\"")
        })
        .collect();
    assert_eq!(
        found.len(),
        1,
        "a pass that ran writes its state row exactly once: {emissions:?}"
    );
    found[0].clone()
}

/// The operations of a bundle, parsed out of its `tool_call` turns.
fn legs(emission: &Value) -> Vec<Value> {
    emission["messages"]
        .as_array()
        .expect("a bundle has messages")
        .iter()
        .map(|turn| {
            meclaw_core::serde_json::from_str(turn["text"].as_str().expect("a call"))
                .expect("a call is JSON")
        })
        .collect()
}

/// The `views` table as far as the state row is concerned, and the store's own
/// answer to a write of it.
///
/// This is the half `Screen` cannot stand in for: `Screen` applies every state
/// write unconditionally, which is the blind write this lock is about. Here the
/// row is written only when the condition the pass sent still holds, and the
/// caller is told how many rows that moved — which is what the store reports
/// (`crates/meclaw-cells/src/store/ops.rs`, `op_update`).
///
/// The rows are a LIST and not one row, because the table has no primary key: a
/// store schema declaration carries column types and nothing else
/// (`templates/display/views/config.json`, `not_in_scope`), so `(owner, view_id)`
/// is an identity the curator keeps by hand. A writer that leaves two rows behind
/// is exactly what this list can see and a single `Value` could not.
struct StateStore {
    rows: Vec<Value>,
}

impl StateStore {
    fn new() -> Self {
        StateStore { rows: Vec::new() }
    }

    /// The one state row, or none. Panics when the writer left two behind.
    fn row(&self) -> Option<&Value> {
        assert!(
            self.rows.len() <= 1,
            "the table holds {} state rows: the identity `(owner, view_id)` is the \
             curator's to keep, and the second row satisfies no later `where \
             updated_at` ever again — the screen has no broom for its own table: {:?}",
            self.rows.len(),
            self.rows
        );
        self.rows.first()
    }

    /// One state write, as the store runs it. Returns `rows_affected`.
    ///
    /// Two shapes, and both are the curator's: the birth is `delete` + `insert` in
    /// ONE message (that pair IS the identity), every later pass is one `update`
    /// that names the version it read.
    fn write(&mut self, emission: &Value) -> i64 {
        let ops = legs(emission);
        let mut affected = 0;
        for op in &ops {
            match op["operation"].as_str().unwrap_or("") {
                "delete" => {
                    assert_eq!(
                        ops.len(),
                        2,
                        "a delete belongs to the birth bundle: {ops:?}"
                    );
                    self.rows.clear();
                }
                "insert" => {
                    assert_eq!(
                        ops.len(),
                        2,
                        "a row is created only in the two-leg bundle whose delete goes \
                         first, or two passes that both found nothing leave two rows: {ops:?}"
                    );
                    self.rows.push(op["row"].clone());
                    affected = 1;
                }
                "update" => {
                    assert_eq!(ops.len(), 1, "the conditional write is one leg: {ops:?}");
                    let want = &op["where"]["updated_at"];
                    assert!(
                        !want.is_null(),
                        "the update carries no condition on what the pass read: {op:?}"
                    );
                    for row in self.rows.iter_mut() {
                        if row["updated_at"] != *want {
                            continue;
                        }
                        for (key, value) in op["set"].as_object().expect("the update sets columns")
                        {
                            row[key.clone()] = value.clone();
                        }
                        affected += 1;
                    }
                }
                other => panic!("a state write writes rows, never {other:?}: {op:?}"),
            }
        }
        affected
    }
}

/// One state write the store REFUSED (`rows_affected: 0`), answered to the cell.
///
/// The request is handed in whole, so a case can walk one mark through collision after
/// collision without a state row anywhere: what the cell answers to a refusal is a
/// function of the mark alone.
fn refused_write(screen: &Screen, request: &Value) -> Vec<Value> {
    let doc = json!({
        "params": screen.params,
        "body": {
            "messages": [{
                "origin": "tool", "type": "tool_result", "id": "s-update", "text": "null",
            }],
        },
        "envelope": {"header": {
            "hop": {"operation": "update", "rows_affected": 0},
            "context": {"display_origin": "views",
                        "display_request": request.to_string()},
        }},
    });
    raw(&doc)
}

/// The store's reply to a state write, handed back to the cell as pass 2 sees it.
///
/// `rows_affected` rides the hop the way the store stamps it on a single-op reply
/// (`crates/meclaw-cells/src/store/output.rs`, `build_tool_result`), and the
/// request the writer sent rides `context.display_request`, promoted off the hop
/// by the hive's own edge.
fn write_reply(screen: &Screen, emission: &Value, rows_affected: i64) -> Vec<Value> {
    let request = emission["header"]["display_request"]
        .as_str()
        .expect("the state write names itself on the hop");
    let doc = json!({
        "params": screen.params,
        "body": {
            "messages": [{
                "origin": "tool", "type": "tool_result", "id": "s-update", "text": "null",
            }],
            "results": [{
                "tool_call_id": "s-update", "operation": "update",
                "rows_affected": rows_affected, "duration_ms": 1,
            }],
        },
        "envelope": {"header": {
            "hop": {"operation": "update", "rows_affected": rows_affected},
            "context": {"display_origin": "views", "display_request": request},
        }},
    });
    raw(&doc)
}

/// The event one absorbed request stands for: what the repeated pass will run.
fn request_of(emission: &Value) -> Value {
    meclaw_core::serde_json::from_str(
        emission["header"]["display_request"]
            .as_str()
            .unwrap_or("{}"),
    )
    .expect("a request is JSON")
}

/// Two taps on the tile of ONE closed window, handed the same state row.
///
/// The colony's shape, step by step: tap A selects, tap B selects before A's
/// write has landed, A computes and writes, B computes on A's input and writes.
/// The second write is the one that must not land.
#[test]
fn two_taps_in_one_round_trip_end_with_the_window_closed() {
    if !library_ships() {
        return;
    }
    let mut screen = Screen::new(json!({}));
    // Relevance 0.5 against the default bar of 0.3 is a score of 0.25: the window
    // is present with a tile and is NOT open, so a tap has something to open.
    screen.write(
        component_view(
            "a",
            "main",
            pane("a", json!({"context": "work", "relevance": "0.5"})),
        ),
        1000,
    );
    let window = window_id("alex", "a");
    assert_eq!(
        screen.curator(&window, "open"),
        json!(false),
        "not yet open"
    );

    // The store, caught up with what the screen has written so far.
    let mut store = StateStore::new();
    store.rows.push(screen.state.clone());
    let stale = screen.state.clone();

    // --- tap A: computed on `stale`, and it lands ---------------------------
    screen.state = stale.clone();
    screen.pass(json!({"kind": "tap", "for": window.as_str()}), 2000);
    let a_write = state_write(&screen.last);
    assert_eq!(store.write(&a_write), 1, "the first write finds its row");
    assert_eq!(
        screen.curator(&window, "open"),
        json!(true),
        "tap A found a closed window and opened it (§ 5.1)"
    );

    // --- tap B: handed the SAME row, so it opens the window a second time ---
    screen.state = stale.clone();
    screen.pass(json!({"kind": "tap", "for": window.as_str()}), 2036);
    let b_write = state_write(&screen.last);
    assert_eq!(
        store.write(&b_write),
        0,
        "the second write computed on a row that has moved on -- it must be refused, \
         or the tap of pass A is gone (§ 4.1: every event acts on the state the one \
         before it left)"
    );

    // --- the refusal comes back, and the pass runs again --------------------
    let again = write_reply(&screen, &b_write, 0);
    assert_eq!(
        again.len(),
        1,
        "a refused state write repeats its own pass, once: {again:?}"
    );
    assert_eq!(again[0]["header"]["route"], "views", "{again:?}");
    let request = request_of(&again[0]);
    assert_eq!(
        request["tap"], window,
        "the repeat carries the SAME mark -- the tap must not be invented anew: {request}"
    );
    assert_eq!(
        legs(&again[0])
            .iter()
            .map(|op| op["operation"].clone())
            .collect::<Vec<Value>>(),
        vec![json!("select")],
        "the repeat reads and writes nothing of its own (OR-F6)"
    );

    // --- and the repeat runs on what the store now holds ---------------------
    screen.state = store.row().expect("one state row stands").clone();
    screen.pass(json!({"kind": "tap", "for": window.as_str()}), 2100);
    assert_eq!(
        store.write(&state_write(&screen.last)),
        1,
        "the repeat writes on the row it read"
    );
    assert_eq!(
        screen.curator(&window, "open"),
        json!(false),
        "two taps on a closed window end closed (§ 5.8, S-024: an even count ends put away)"
    );
}

/// Two passes that both find NO state row leave exactly one behind.
///
/// The birth is the one write with no version to compare against, and the table
/// it writes into declares no primary key: a store schema carries column types
/// and nothing else (`templates/display/views/config.json`, `not_in_scope`), so
/// `(owner, view_id)` is an identity the curator keeps by writing `delete` then
/// `insert` in ONE message. A bare `insert` would be the one hole a
/// compare-and-set cannot close afterwards: the second row satisfies no later
/// `where updated_at` ever again, and nothing on this screen sweeps it away.
///
/// The case is a colony's boot, not a rarity — several applications write in the
/// same breath, and the run `M13-h3-timer` measured three passes 18–46 ms apart
/// (`plans/welle-h3-2026-09-18/messungen/B-klickserie.md` § 3).
#[test]
fn two_first_passes_leave_one_state_row() {
    if !library_ships() {
        return;
    }
    let mut screen = Screen::new(json!({}));
    let mut store = StateStore::new();

    // Two passes, both handed the state the colony had before either of them: none.
    let mut births = Vec::new();
    for now in [1000u64, 1036] {
        screen.state = Value::Null;
        screen.write(
            component_view(
                "a",
                "main",
                pane("a", json!({"context": "work", "relevance": "0.5"})),
            ),
            now,
        );
        births.push(state_write(&screen.last));
    }
    for birth in &births {
        assert_eq!(
            legs(birth)
                .iter()
                .map(|op| op["operation"].clone())
                .collect::<Vec<Value>>(),
            vec![json!("delete"), json!("insert")],
            "the birth holds the identity itself: delete on the pair, then insert, in \
             one message and in that order"
        );
        store.write(birth);
    }
    assert!(
        store.row().is_some(),
        "and one row stands when both passes are through"
    );
}

/// A pass that loses round after round still lands, and a give-up names its owner.
///
/// The repeat is not a gamble: every round has exactly ONE winner, so the field of
/// contenders shrinks by one each time and a set of N passes that started together needs
/// at most N-1 repeats for its last member. The bound is therefore the FAN-OUT of the
/// hive, not a constant -- and the fan-out of this screen is every application that
/// writes in one breath.
///
/// Measured on the twin (18.09.2026, 39 minutes of the acceptance runs, 401 state writes
/// read back out of the colony's own log with the store's `rows_affected` beside each
/// one): seven applications wrote in the same breath, 190 of the 401 writes were refused
/// and repeated -- and FOURTEEN passes ran out of repeats and lost their event, all of
/// them `app_write` on the chat or on a card, in exactly the windows where the screen
/// flickered. The receipt they left carried an empty `owner`, so the application whose
/// write was dropped was never told.
///
/// So this case walks one mark through more collisions than the measured fan-out, and
/// asks for two things: every one of them is another read pass, and the mark keeps what
/// it needs to be one.
#[test]
fn a_pass_keeps_its_place_through_more_collisions_than_the_hive_has_writers() {
    if !library_ships() {
        return;
    }
    let screen = Screen::new(json!({}));
    // An application's write: the mark the colony hands back carries its owner, which is
    // the only thing that makes a give-up readable by anybody.
    let mut mark = json!({"withdraw": false, "owner": "alex", "view_id": "a",
                          "row": {"owner": "alex", "view_id": "a"}});
    // Seven writers were measured; ten collisions is past that.
    for round in 0..10u64 {
        let out = refused_write(&screen, &json!({"state": true, "retry": mark}));
        assert_eq!(
            out.len(),
            1,
            "collision {round}: a refused write runs its pass again, once -- a screen \
             that gives up here drops the event, which is the defect this file is about: \
             {out:?}"
        );
        assert_eq!(
            out[0]["header"]["route"], "views",
            "collision {round}: giving up is not an answer while the fan-out of the hive \
             is bigger than the count: {out:?}"
        );
        assert_eq!(
            legs(&out[0])
                .iter()
                .map(|op| op["operation"].clone())
                .collect::<Vec<Value>>(),
            vec![json!("select")],
            "collision {round}: the repeat reads and writes nothing of its own (OR-F6)"
        );
        let again = request_of(&out[0]);
        assert_eq!(
            again["owner"], "alex",
            "collision {round}: the mark keeps its owner: {again}"
        );
        assert_eq!(
            again["retries"],
            json!(round + 1),
            "collision {round}: every repeat counts itself: {again}"
        );
        mark = again;
    }
}

/// And the cap, when it is finally reached: the event is lost, and that is said to the
/// application whose view it was rather than into a dead letter nobody reads.
#[test]
fn the_pass_that_gives_up_names_the_view_it_lost() {
    if !library_ships() {
        return;
    }
    let screen = Screen::new(json!({}));
    let mut mark = json!({"withdraw": false, "owner": "alex", "view_id": "a",
                          "row": {"owner": "alex", "view_id": "a"}});
    // Past the cap, wherever it stands: the walk ends either with a repeat that counts
    // itself or with the one receipt this case is about.
    let mut receipt = None;
    for _ in 0..200 {
        let out = refused_write(&screen, &json!({"state": true, "retry": mark}));
        assert_eq!(out.len(), 1, "one answer per refused write: {out:?}");
        if out[0]["header"]["route"] == "receipt" {
            receipt = Some(out[0].clone());
            break;
        }
        mark = request_of(&out[0]);
    }
    let receipt = receipt.expect("the repeats stop somewhere, or this cell loops for ever");
    assert_eq!(
        receipt["receipt"]["error_code"], "store_failed",
        "the give-up is a refusal, not a silence: {receipt}"
    );
    assert_eq!(
        receipt["receipt"]["owner"], "alex",
        "and it names the owner of the view whose write was dropped -- a receipt with an \
         empty owner fails every owner guard by construction and dead-letters where the \
         application never sees it (fourteen of them were measured on the twin): {receipt}"
    );
    assert_eq!(receipt["receipt"]["view_id"], "a", "{receipt}");
    assert_eq!(
        receipt["header"]["owner"], "alex",
        "and on the hop too: {receipt}"
    );
}

/// The state row is a version, and the version moves with every write.
///
/// Without this the compare-and-set is only as good as the clock: two writes in
/// the same millisecond would both carry the same `updated_at`, and a third pass
/// that read the first could still overwrite the second.
#[test]
fn every_state_write_moves_the_row_it_compares_against() {
    if !library_ships() {
        return;
    }
    let mut screen = Screen::new(json!({}));
    screen.write(
        component_view(
            "a",
            "main",
            pane("a", json!({"context": "work", "relevance": "0.5"})),
        ),
        1000,
    );
    let window = window_id("alex", "a");
    let mut seen: Vec<i64> = vec![screen.state["updated_at"].as_i64().expect("a stamp")];
    // The birth is the two-leg bundle; every pass after it is the one-leg update.
    // Three passes inside ONE millisecond: the clock cannot tell them apart, so
    // the row has to.
    for _ in 0..3 {
        screen.pass(json!({"kind": "tap", "for": window.as_str()}), 2000);
        seen.push(
            state_write(&screen.last)["messages"]
                .as_array()
                .and_then(|legs| legs.last())
                .expect("the state write carries a leg")["text"]
                .as_str()
                .map(|t| {
                    let op: Value = meclaw_core::serde_json::from_str(t).expect("a call is JSON");
                    let stamp = if op["operation"] == "update" {
                        op["set"]["updated_at"].clone()
                    } else {
                        op["row"]["updated_at"].clone()
                    };
                    stamp.as_i64().expect("a stamp is a number")
                })
                .expect("the state write carries a row"),
        );
    }
    for pair in seen.windows(2) {
        assert!(
            pair[1] > pair[0],
            "a state write has to move the version it compares against: {seen:?}"
        );
    }
}

/// GH #765, way A (the owner's ruling, 19.09.): the client gets the patch only once the
/// state row has landed.
///
/// The compare-and-set above keeps the STORE right. It did nothing for the browsers: the
/// pass sent its patch in the same breath as its write, so a pass whose write was refused
/// had already drawn a state that never became the screen's — 190 of 401 measured writes
/// were refused, and every one of them had patched. What the person saw was a window that
/// flashed open and shut, and the client-side hold of GH #744 was the brake against it.
///
/// So the patch rides the write's own request and is emitted from the REPLY: `rows_affected
/// 1` draws it, `rows_affected 0` draws nothing at all and repeats the pass instead. The
/// price is one message round trip of latency per event, and it is the point rather than a
/// cost — a drawing that is never taken back is worth a round trip.
#[test]
fn the_patch_waits_for_the_state_row() {
    if !library_ships() {
        return;
    }
    let mut screen = Screen::new(json!({}));
    screen.write(
        component_view(
            "a",
            "main",
            pane("a", json!({"context": "work", "relevance": "0.5"})),
        ),
        1000,
    );

    // --- the pass itself draws nothing ---------------------------------------
    assert!(
        screen.last.iter().all(|e| e["header"]["route"] != "patch"),
        "the pass sent its patch before its write had landed: {:?}",
        screen.last
    );
    let birth = state_write(&screen.last);
    let request = request_of(&birth);
    let carried = request["patch"]
        .as_array()
        .unwrap_or_else(|| panic!("the write carries the patch it owes: {request}"));
    assert!(
        !carried.is_empty(),
        "a pass that rendered something carries its calls: {request}"
    );

    // --- the write lands, and THEN the browsers hear it -----------------------
    let landed = write_reply(&screen, &birth, 1);
    let drawn: Vec<&Value> = landed
        .iter()
        .filter(|e| e["header"]["route"] == "patch")
        .collect();
    assert_eq!(
        drawn.len(),
        1,
        "the landed write draws exactly one patch: {landed:?}"
    );
    let sent: Vec<Value> = drawn[0]["messages"]
        .as_array()
        .expect("a bundle has messages")
        .iter()
        .map(|turn| {
            meclaw_core::serde_json::from_str(turn["text"].as_str().expect("a call"))
                .expect("a call is JSON")
        })
        .collect();
    assert_eq!(
        sent,
        carried.clone(),
        "and it draws what the pass computed, call for call -- nothing is recomputed on \
         the reply: {landed:?}"
    );

    // --- a refused write draws nothing and repeats ----------------------------
    let window = window_id("alex", "a");
    screen.pass(json!({"kind": "tap", "for": window.as_str()}), 2000);
    let tap = state_write(&screen.last);
    let refused = refused_write(&screen, &request_of(&tap));
    assert!(
        refused.iter().all(|e| e["header"]["route"] != "patch"),
        "a refused write may not draw: the state it rendered never became the screen's \
         (GH #765, way A): {refused:?}"
    );
    assert_eq!(
        refused.len(),
        1,
        "one answer per refused write: {refused:?}"
    );
    assert_eq!(
        refused[0]["header"]["route"], "views",
        "what it does instead is run its own pass again: {refused:?}"
    );
    assert!(
        request_of(&refused[0])["patch"].is_null(),
        "and the repeat carries no patch of its own -- the next pass computes it fresh on \
         the row the store now holds: {refused:?}"
    );
}

/// The store's reply to a state write that says nothing about the row it moved.
///
/// Neither `results[]` nor the hop names an `update` leg. A reply of this shape is what a
/// truncated, foreign or future answer looks like from inside the cell — it is not an
/// error (nothing carries an `error_code`), it simply does not say whether the row landed.
fn mute_write_reply(screen: &Screen, request: &Value) -> Vec<Value> {
    let doc = json!({
        "params": screen.params,
        "body": {
            "messages": [{
                "origin": "tool", "type": "tool_result", "id": "s-update", "text": "null",
            }],
        },
        "envelope": {"header": {
            "hop": {},
            "context": {"display_origin": "views",
                        "display_request": request.to_string()},
        }},
    });
    raw(&doc)
}

/// The store's reply to the BIRTH bundle: `delete` then `insert`, answered per leg.
///
/// The first creation has no version to compare against, so it is the two-leg bundle of
/// `state_write_ops` and the store answers it in `results[]` with `operation: "bundle"` on
/// the hop (`crates/meclaw-cells/src/store/output.rs`, `build_bundle_result`).
fn birth_reply(screen: &Screen, emission: &Value) -> Vec<Value> {
    let request = emission["header"]["display_request"]
        .as_str()
        .expect("the state write names itself on the hop");
    let doc = json!({
        "params": screen.params,
        "body": {
            "messages": [{
                "origin": "tool", "type": "tool_result", "id": "s-insert", "text": "null",
            }],
            "results": [
                {"tool_call_id": "s-delete", "operation": "delete",
                 "rows_affected": 0, "duration_ms": 1},
                {"tool_call_id": "s-insert", "operation": "insert",
                 "rows_affected": 1, "duration_ms": 1},
            ],
        },
        "envelope": {"header": {
            "hop": {"operation": "bundle", "rows_affected": 1},
            "context": {"display_origin": "views", "display_request": request},
        }},
    });
    raw(&doc)
}

/// GH #765, way A: "the store did not say" is not "the store agreed".
///
/// The landing check is the whole of way A — a patch may leave only once the row it
/// renders stands in the store. A check that draws whenever it cannot read a refusal
/// inverts that promise for every reply it cannot parse: a truncated body, a hop without
/// the leg it expects, a store that answers in a shape this cell does not know. None of
/// those carry an `error_code`, so `bundle_failed` lets them through, and "unknown" then
/// means "drawn" — which is precisely the state way A forbids to leave the cell.
///
/// So the branch is positive on both spellings and on nothing else: the update that moved
/// its one row, and the birth bundle that inserted it. Everything else draws nothing and
/// runs the pass again on the row the store now holds.
#[test]
fn a_reply_that_does_not_say_does_not_draw() {
    if !library_ships() {
        return;
    }
    let mut screen = Screen::new(json!({}));
    screen.write(
        component_view(
            "a",
            "main",
            pane("a", json!({"context": "work", "relevance": "0.5"})),
        ),
        1000,
    );

    // --- the birth is recognised by its OWN legs, not by the absence of an update ------
    let birth = state_write(&screen.last);
    let born = birth_reply(&screen, &birth);
    assert!(
        born.iter().any(|e| e["header"]["route"] == "patch"),
        "the birth bundle lands with `insert`, and a landed birth draws: {born:?}"
    );

    // --- a reply that names no leg at all draws nothing and repeats --------------------
    let window = window_id("alex", "a");
    screen.pass(json!({"kind": "tap", "for": window.as_str()}), 2000);
    let tap = state_write(&screen.last);
    let mute = mute_write_reply(&screen, &request_of(&tap));
    assert!(
        mute.iter().all(|e| e["header"]["route"] != "patch"),
        "a reply the cell cannot read is not a landing: drawing on it means way A promises \
         `rows_affected 1` and delivers `anything but 0` (GH #765): {mute:?}"
    );
    assert_eq!(mute.len(), 1, "one answer, and it is the repeat: {mute:?}");
    assert_eq!(
        mute[0]["header"]["route"], "views",
        "what it does instead is run its own pass again on the row the store holds: {mute:?}"
    );
    assert!(
        request_of(&mute[0])["patch"].is_null(),
        "and it carries no patch of its own: {mute:?}"
    );
}
