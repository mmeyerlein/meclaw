//! GH #679 -- a due clock inside the screen's hive strikes once, exactly when
//! something is due.
//!
//! Since 1.1.0 the README said "nothing sweeps": an expired view stayed on the
//! screen until somebody else wrote. Now the compose cell predicts the earliest
//! moment at which -- if nothing else happens -- a window's score crosses a
//! rung, a fresh or leaving window's frame is over, a `relevant_until` or a
//! `ttl_ms` runs out, and orders exactly ONE one-shot schedule named `due` from
//! a `timer` cell beside it. The strike comes back as the lane `in_tick`: a
//! pass without a write. A screen on which nothing can change orders nothing
//! (GH #553: a strike with a name and a time is not a poll).
//!
//! The script runs as a subprocess the way a `code` cell runs it; the answer
//! is the list of emissions it would send. Skips when `python3` is absent or
//! the templates do not ship (R2b).

use std::io::Write;
use std::process::{Command, Stdio};

use meclaw_core::serde_json::{Value, json};

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";
const ROOT: &str = "display.root";
const LINGER: u64 = 20000;
const FADE: u64 = 120000;

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

fn read_json(path: &std::path::Path) -> Value {
    meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    )
    .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The wrapper id of a view the persona `alex` owns.
fn wrapper(view_id: &str) -> String {
    format!("view.alex.{view_id}")
}

/// The id of a keyed pane under a view of `alex`.
fn pane_id(view_id: &str, pane: &str) -> String {
    format!("view.alex.{view_id}/c.{pane}")
}

fn pane(pane: &str, props: Value) -> Value {
    let mut props = props;
    props["pane_id"] = json!(pane);
    json!({"component": "display-pane", "props": props, "key": format!("c.{pane}")})
}

/// One row of the `views` table: a component view of `alex` with one tree.
fn component_view(view_id: &str, region: &str, tree: Value) -> Value {
    json!({
        "owner": "alex", "view_id": view_id, "region": region, "ord": 0,
        "kind": "component", "content": tree.to_string(), "components": "[]",
        "ttl_ms": 0, "updated_at": 1,
    })
}

/// Run the shipped script over one document and return every emission it
/// answers with -- one object, an array of them, or none. `None` when there
/// is no `python3` on this host.
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

/// The calls of a bundle emission: each one the parsed `text` of a `tool_call`.
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

fn on_route<'a>(emissions: &'a [Value], route: &str) -> Vec<&'a Value> {
    emissions
        .iter()
        .filter(|e| e["header"]["route"] == route)
        .collect()
}

/// The calls of the one `patch` emission, or none when the pass said nothing.
fn patch_calls(emissions: &[Value]) -> Vec<Value> {
    let patches = on_route(emissions, "patch");
    assert!(patches.len() <= 1, "at most one patch: {emissions:?}");
    patches.first().map(|e| calls_of(e)).unwrap_or_default()
}

/// A read pass: the table holds `views`, the display answers the `query` with
/// `objects`, the clock says `now`.
fn read_pass(views: &[Value], objects: Option<&Value>, now: u64) -> Option<Vec<Value>> {
    read_pass_struck(views, objects, now, None)
}

/// A read pass on a tick that carries the id of the order that just struck.
fn read_pass_struck(
    views: &[Value],
    objects: Option<&Value>,
    now: u64,
    struck: Option<&str>,
) -> Option<Vec<Value>> {
    let mut plan = json!({"views": views, "define": [], "now": now});
    if let Some(sid) = struck {
        plan["struck"] = json!(sid);
    }
    let messages = match objects {
        None => json!([]),
        Some(objs) => json!([{
            "origin": "tool", "type": "tool_result", "id": "d-query",
            "text": json!({"objects": objs}).to_string(),
        }]),
    };
    run(&json!({
        "params": {},
        "body": {"messages": messages},
        "envelope": {"header": {
            "hop": {"operation": "query"},
            "context": {
                "display_origin": "read",
                "display_views": plan.to_string(),
            },
        }},
    }))
}

/// Play the display: apply a bundle's creates, updates and deletes to what it
/// holds. `object.update` merges per key, like the cell's own.
fn apply(held: &mut Value, calls: &[Value]) {
    let list = held.as_array_mut().expect("the display holds a list");
    for c in calls {
        match c["op"].as_str().unwrap_or("") {
            "object.create" => list.push(json!({
                "id": c["id"], "parent": c["parent"], "ord": c["ord"],
                "component": c["component"], "props": c["props"],
            })),
            "object.update" => {
                let obj = list
                    .iter_mut()
                    .find(|o| o["id"] == c["id"])
                    .unwrap_or_else(|| panic!("an update names a held object: {}", c["id"]));
                for (k, v) in c["props"].as_object().expect("props") {
                    obj["props"][k] = v.clone();
                }
            }
            "object.delete" => list.retain(|o| o["id"] != c["id"]),
            _ => {}
        }
    }
}

fn set_prop(held: &mut Value, id: &str, key: &str, value: Value) {
    let obj = held
        .as_array_mut()
        .expect("list")
        .iter_mut()
        .find(|o| o["id"] == id)
        .unwrap_or_else(|| panic!("{id} is held"));
    obj["props"][key] = value;
}

fn prop_of<'a>(held: &'a Value, id: &str, key: &str) -> &'a Value {
    &held
        .as_array()
        .expect("list")
        .iter()
        .find(|o| o["id"] == id)
        .unwrap_or_else(|| panic!("{id} is held"))["props"][key]
}

/// The props a bundle writes on `id`, from its create or its update.
fn written(calls: &[Value], id: &str) -> Option<Value> {
    calls
        .iter()
        .find(|c| (c["op"] == "object.create" || c["op"] == "object.update") && c["id"] == id)
        .map(|c| c["props"].clone())
}

/// `%Y-%m-%dT%H:%M:%SZ` of an epoch in milliseconds, rounded UP to the
/// second -- the timer's `at` form, and never earlier than the moment asked.
fn iso_z(ms: u64) -> String {
    let secs = ms.div_ceil(1000) as i64;
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    // Civil-from-days (Howard Hinnant), enough for a test's clock.
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// A screen holding one window `a` in `main` (conversation, 0.7), settled and
/// in focus, touched at `since`; the root carries no order yet.
fn screen_with_a(since: u64) -> Option<(Vec<Value>, Value)> {
    let views = vec![component_view(
        "a",
        "main",
        pane("a", json!({"context": "conversation", "relevance": 0.7})),
    )];
    let mut held = json!([]);
    let boot = read_pass(&[], None, since)?;
    apply(&mut held, &patch_calls(&boot));
    let arrive = read_pass(&views, Some(&held), since)?;
    apply(&mut held, &patch_calls(&arrive));
    // Settle it: one pass later `a` takes the focus.
    let settle = read_pass(&views, Some(&held), since + 100)?;
    apply(&mut held, &patch_calls(&settle));
    assert_eq!(prop_of(&held, &pane_id("a", "a"), "state"), "focus");
    assert_eq!(prop_of(&held, &pane_id("a", "a"), "since"), &json!(since));
    set_prop(&mut held, ROOT, "due", json!(""));
    Some((views, held))
}

/// Two pinned windows in `aside`, settled, no `ttl_ms`, no order on the root:
/// nothing on this screen can change by itself, so the pass orders no strike.
#[test]
fn a_screen_with_nothing_due_orders_no_tick() {
    if !library_ships() {
        return;
    }
    let views = vec![
        component_view(
            "x",
            "aside",
            pane(
                "x",
                json!({"context": "clock", "relevance": 0.4, "pinned": true}),
            ),
        ),
        component_view(
            "y",
            "aside",
            pane(
                "y",
                json!({"context": "weather", "relevance": 0.4, "pinned": true}),
            ),
        ),
    ];
    let Some(boot) = read_pass(&[], None, 1000) else {
        return;
    };
    let mut held = json!([]);
    apply(&mut held, &patch_calls(&boot));
    let arrive = read_pass(&views, Some(&held), 1000).expect("python3");
    apply(&mut held, &patch_calls(&arrive));
    let settle = read_pass(&views, Some(&held), 2000).expect("python3");
    apply(&mut held, &patch_calls(&settle));
    set_prop(&mut held, ROOT, "due", json!(""));
    let quiet = read_pass(&views, Some(&held), 500_000).expect("python3");
    assert!(
        on_route(&quiet, "due").is_empty(),
        "a screen with nothing due orders nothing: {quiet:?}"
    );
    let root = written(&patch_calls(&quiet), ROOT);
    assert!(
        root.as_ref()
            .is_none_or(|p| p["due"] == "" || p["due"].is_null()),
        "the root carries no order: {root:?}"
    );
}

/// `a` in focus (0.7 against a bar of 0.3) fades from `since + linger_ms`;
/// the first rung it crosses is the midpoint 0.65, and that moment -- solved
/// analytically -- is the one strike the pass orders. The root remembers the
/// order, and the next pass that computes the same moment orders it again
/// without removing it first (GH #690): the clock takes a repeated order as
/// one order.
#[test]
fn a_due_window_orders_exactly_one_tick_at_the_earliest_moment() {
    if !library_ships() {
        return;
    }
    let Some((views, mut held)) = screen_with_a(1000) else {
        return;
    };
    let pass = read_pass(&views, Some(&held), 2000).expect("python3");
    let due = on_route(&pass, "due");
    assert_eq!(due.len(), 1, "exactly one order: {pass:?}");
    let order = due[0];
    assert_eq!(order["op"], "add");
    assert_eq!(order["schedule_name"], "due");
    assert_eq!(order["messages"], json!([]));
    assert_eq!(order["emit_to"], ".");
    assert_eq!(order["emit_body"], json!({"messages": []}));
    let expected = 1000 + LINGER + ((FADE as f64) * (1.0 - 0.65 / 0.7)) as u64 + 1;
    assert_eq!(order["at"], iso_z(expected), "the midpoint comes first");
    let sid = order["schedule_id"].as_str().expect("an id").to_string();
    assert!(sid.len() >= 32, "a uuid: {sid}");
    let root = written(&patch_calls(&pass), ROOT).expect("the root update carries due");
    assert_eq!(root["due"], sid);

    apply(&mut held, &patch_calls(&pass));
    let again = read_pass(&views, Some(&held), 2000).expect("python3");
    let due = on_route(&again, "due");
    assert_eq!(
        due.len(),
        1,
        "the same order again, nothing removed: {again:?}"
    );
    assert_eq!(due[0]["op"], "add");
    assert_eq!(due[0]["schedule_id"], sid);
    assert_eq!(due[0]["at"], iso_z(expected));
    let root = written(&patch_calls(&again), ROOT);
    assert!(
        root.as_ref().is_none_or(|p| p["due"] == sid),
        "and the root keeps its id: {root:?}"
    );
}

/// GH #690: a pass that computes the moment already ordered must not send
/// `remove <id>` and `add <id>` for one id -- the timer marks the row
/// `removed`, the `add` collides with it (`schedule_id_exists`), and the
/// moment never strikes. A same-second pass is exactly one `add` and no
/// `remove` (the clock takes the repeated order as one order); a moment that
/// moved is `remove` the old id, `add` the new one, and the two differ.
#[test]
fn a_pass_that_agrees_with_the_standing_order_orders_it_again_without_removing_it() {
    if !library_ships() {
        return;
    }
    let Some((views, mut held)) = screen_with_a(1000) else {
        return;
    };
    let mut sid = String::new();
    for now in [2000, 2500] {
        let pass = read_pass(&views, Some(&held), now).expect("python3");
        let due = on_route(&pass, "due");
        assert!(
            due.iter().all(|o| o["op"] == "add"),
            "no remove on a same-second pass at {now}: {pass:?}"
        );
        assert_eq!(due.len(), 1, "exactly one add at {now}: {pass:?}");
        let id = due[0]["schedule_id"].as_str().expect("an id");
        if sid.is_empty() {
            sid = id.to_string();
        }
        assert_eq!(id, sid, "the same second is the same order");
        apply(&mut held, &patch_calls(&pass));
    }
    assert_eq!(prop_of(&held, ROOT, "due"), &json!(sid));

    // `b` arrives: fresh for one frame, so the moment moves to `now + 1000`.
    let mut with_b = views.clone();
    with_b.push(component_view(
        "b",
        "main",
        pane("b", json!({"context": "conversation", "relevance": 0.6})),
    ));
    let moved = read_pass(&with_b, Some(&held), 3000).expect("python3");
    let due = on_route(&moved, "due");
    assert_eq!(due.len(), 2, "remove the old, add the new: {moved:?}");
    assert_eq!(due[0]["op"], "remove");
    assert_eq!(due[0]["schedule_id"], sid);
    assert_eq!(due[1]["op"], "add");
    assert_eq!(due[1]["at"], iso_z(4000));
    assert_ne!(
        due[1]["schedule_id"], sid,
        "a different moment is a different order: {moved:?}"
    );
}

/// GH #690, the revisited second: `a` fades and is due at S; `b` arrives,
/// fresh for one frame, so the order moves to C (`remove S`, `add C`); C
/// strikes, `b` settles, and S is the earliest moment again -- `add S`, with
/// no `remove` for C (it struck) and none for S. What compose emits over the
/// chain is `add S`, `remove S`, `add C`, `add S`; the second `add S` meets
/// the clock's removed row, and the clock revives it (pinned on the clock
/// side, `timer::db::tests`).
#[test]
fn a_revisited_second_is_ordered_again_and_never_removed_twice() {
    if !library_ships() {
        return;
    }
    let Some((views, mut held)) = screen_with_a(1000) else {
        return;
    };
    fn orders(pass: &[Value]) -> Vec<(String, String)> {
        on_route(pass, "due")
            .into_iter()
            .map(|o| {
                (
                    o["op"].as_str().expect("op").to_string(),
                    o["schedule_id"].as_str().expect("id").to_string(),
                )
            })
            .collect()
    }
    let mut chain: Vec<(String, String)> = Vec::new();
    let first = read_pass(&views, Some(&held), 2000).expect("python3");
    chain.extend(orders(&first));
    apply(&mut held, &patch_calls(&first));
    let s = chain[0].1.clone();

    let mut with_b = views.clone();
    with_b.push(component_view(
        "b",
        "main",
        pane("b", json!({"context": "conversation", "relevance": 0.6})),
    ));
    let moved = read_pass(&with_b, Some(&held), 3000).expect("python3");
    chain.extend(orders(&moved));
    apply(&mut held, &patch_calls(&moved));
    let c = chain[2].1.clone();
    assert_ne!(c, s);
    assert_eq!(on_route(&moved, "due")[1]["at"], iso_z(4000));

    let struck = read_pass_struck(&with_b, Some(&held), 4001, Some(&c)).expect("python3");
    chain.extend(orders(&struck));
    apply(&mut held, &patch_calls(&struck));
    assert_eq!(prop_of(&held, &pane_id("b", "b"), "age"), "settled");

    assert_eq!(
        chain,
        vec![
            ("add".to_string(), s.clone()),
            ("remove".to_string(), s.clone()),
            ("add".to_string(), c.clone()),
            ("add".to_string(), s.clone()),
        ],
        "add S, remove S, add C, strike C, add S again"
    );
    assert_eq!(prop_of(&held, ROOT, "due"), &json!(s));
}

/// GH #681: an order's id is its due moment. Two passes that read the same
/// stale `due` and compute the same moment order the same id, so the second
/// `add` collides (`schedule_id_exists`, an `in_tick_error` the pass ignores)
/// instead of standing beside the first; a different moment is a different
/// order.
#[test]
fn two_passes_that_agree_on_the_moment_order_the_same_id() {
    if !library_ships() {
        return;
    }
    let Some((views, held)) = screen_with_a(1000) else {
        return;
    };
    let first = read_pass(&views, Some(&held), 2000).expect("python3");
    let second = read_pass(&views, Some(&held), 2000).expect("python3");
    let (a, b) = (on_route(&first, "due"), on_route(&second, "due"));
    assert_eq!(a.len(), 1);
    assert_eq!(b.len(), 1);
    assert_eq!(a[0]["op"], "add");
    assert_eq!(
        a[0]["at"], b[0]["at"],
        "the same screen at the same time is due at the same moment"
    );
    assert_eq!(
        a[0]["schedule_id"], b[0]["schedule_id"],
        "and the same moment is the same order"
    );
    // Two passes a few milliseconds apart (a bootstrap bundle after a swap)
    // compute two due milliseconds for the same second, and the timer only
    // knows seconds: the id is the second's, not the millisecond's. A fresh
    // window is due `now + 1000`, so it is the case where `now` moves the
    // moment.
    let mut with_b = views.clone();
    with_b.push(component_view(
        "b",
        "main",
        pane("b", json!({"context": "conversation", "relevance": 0.6})),
    ));
    let early = read_pass(&with_b, Some(&held), 1990).expect("python3");
    let late = read_pass(&with_b, Some(&held), 2000).expect("python3");
    let (e, l) = (on_route(&early, "due"), on_route(&late, "due"));
    assert_eq!(e.len(), 1);
    assert_eq!(l.len(), 1);
    assert_eq!(
        e[0]["at"],
        iso_z(3000),
        "2990 rounds up to the same second as 3000"
    );
    assert_eq!(e[0]["at"], l[0]["at"], "the same second is the same moment");
    assert_eq!(
        e[0]["schedule_id"], l[0]["schedule_id"],
        "and the id is derived from the second the timer is told"
    );
    let Some((views, held)) = screen_with_a(3000) else {
        return;
    };
    let later = read_pass(&views, Some(&held), 4000).expect("python3");
    let c = on_route(&later, "due");
    assert_eq!(c.len(), 1);
    assert_ne!(
        c[0]["at"], a[0]["at"],
        "a window touched later is due later"
    );
    assert_ne!(
        c[0]["schedule_id"], a[0]["schedule_id"],
        "and a different moment is a different order"
    );
}

/// A frame is a second: a window that just arrived (`fresh`) and a window on
/// its way out (`leaving`) both order the strike for `now + 1000`, which is
/// what lets the next pass settle the one and delete the other.
#[test]
fn a_leaving_or_fresh_window_orders_the_next_second() {
    if !library_ships() {
        return;
    }
    let Some((views, mut held)) = screen_with_a(1000) else {
        return;
    };
    // Fresh: `b` arrives at t=2000 and is fresh for one frame.
    let mut with_b = views.clone();
    with_b.push(component_view(
        "b",
        "main",
        pane("b", json!({"context": "conversation", "relevance": 0.6})),
    ));
    let arrive = read_pass(&with_b, Some(&held), 2000).expect("python3");
    assert_eq!(
        written(&patch_calls(&arrive), &pane_id("b", "b")).expect("b is created")["age"],
        "fresh"
    );
    let due = on_route(&arrive, "due");
    assert_eq!(due.len(), 1, "{arrive:?}");
    assert_eq!(
        due[0]["at"],
        iso_z(3000),
        "the next second, for the fresh one"
    );
    apply(&mut held, &patch_calls(&arrive));
    let settle = read_pass(&with_b, Some(&held), 3001).expect("python3");
    apply(&mut held, &patch_calls(&settle));
    assert_eq!(prop_of(&held, &pane_id("b", "b"), "age"), "settled");

    // Leaving: `b` is withdrawn at t=5000 and stands one more frame.
    let leave = read_pass(&views, Some(&held), 5000).expect("python3");
    let b = written(&patch_calls(&leave), &pane_id("b", "b")).expect("b is laid back");
    assert_eq!(b["age"], "leaving");
    let due = on_route(&leave, "due");
    assert_eq!(due.len(), 2, "remove then add: {leave:?}");
    assert_eq!(
        due[1]["at"],
        iso_z(6000),
        "the next second, for the leaving one"
    );
}

/// The strike is a pass without a write: `in_tick` reads the table (one
/// `select`, nothing else) with `display_request: {"tick": true}`; the store's
/// answer on a tick becomes one `read` like any other pass 2; and the error
/// the timer sends back for an order that already struck is swallowed.
#[test]
fn a_tick_is_a_pass_without_a_write() {
    if !library_ships() {
        return;
    }
    let Some(tick) = run(&json!({
        "params": {},
        "body": {"messages": []},
        "envelope": {"header": {"hop": {"route": "in_tick", "schedule_name": "due"}}},
    })) else {
        return;
    };
    assert_eq!(tick.len(), 1, "{tick:?}");
    assert_eq!(tick[0]["header"]["route"], "views");
    let legs = calls_of(&tick[0]);
    assert_eq!(legs.len(), 1, "a select and nothing else: {legs:?}");
    assert_eq!(legs[0]["operation"], "select");
    let request: Value = meclaw_core::serde_json::from_str(
        tick[0]["header"]["display_request"]
            .as_str()
            .expect("a request"),
    )
    .expect("json");
    assert_eq!(request, json!({"tick": true}));

    let rows = vec![component_view(
        "a",
        "main",
        pane("a", json!({"context": "conversation", "relevance": 0.7})),
    )];
    let after = run(&json!({
        "params": {},
        "body": {"messages": [{
            "origin": "tool", "type": "tool_result", "id": "d-select",
            "text": Value::Array(rows.clone()).to_string(),
        }]},
        "envelope": {"header": {
            "hop": {},
            "context": {
                "display_origin": "views",
                "display_request": json!({"tick": true}).to_string(),
            },
        }},
    }))
    .expect("python3");
    assert_eq!(after.len(), 1, "{after:?}");
    assert_eq!(after[0]["header"]["route"], "read");
    let plan: Value = meclaw_core::serde_json::from_str(
        after[0]["header"]["display_views"]
            .as_str()
            .expect("a plan"),
    )
    .expect("json");
    assert_eq!(plan["views"].as_array().map(Vec::len), Some(1));
    assert_eq!(plan["define"], json!([]));

    let swallowed = run(&json!({
        "params": {},
        "body": {"messages": [], "meta": {"detail": "schedule not found"}},
        "envelope": {"header": {"hop": {
            "route": "in_tick_error", "msg_type": "timer_op_error",
            "error_code": "schedule_not_found",
        }}},
    }))
    .expect("python3");
    assert!(swallowed.is_empty(), "{swallowed:?}");
}

/// The hive carries the clock: a `timer` cell with no schedule of its own
/// (P-C2: the key is left out, not `[]`), three edges word for word, no cron
/// anywhere under the template, the hive still sealed and its lanes unchanged.
#[test]
fn the_hive_wires_the_clock() {
    if !library_ships() {
        return;
    }
    let clock = read_json(&repo("templates/display/clock/config.json"));
    assert_eq!(clock["cell"]["type"], "timer");
    assert!(
        clock["params"].get("schedules").is_none(),
        "the clock carries no schedule of its own: {}",
        clock["params"]
    );
    assert_eq!(clock["params"]["query_timeout_ms"], 5000);
    assert_eq!(clock["contract"]["version"], "1.0.0");

    let hive = read_json(&repo("templates/display/config.json"));
    assert_eq!(hive["params"]["ports"], json!([]));
    let edges = hive["params"]["graph"]["edges"].as_array().expect("edges");
    for wanted in [
        json!({"from": "./compose", "to": "./clock",
               "condition": "has(hop.route) && hop.route == 'due'"}),
        json!({"from": "./clock", "to": "./compose",
               "condition": "has(hop.schedule_name) && hop.schedule_name == 'due'",
               "modifier": {"set_hop": {"route": "'in_tick'"}}}),
        json!({"from": "./clock", "to": "./compose",
               "condition": "has(hop.msg_type) && hop.msg_type == 'timer_op_error'",
               "modifier": {"set_hop": {"route": "'in_tick_error'"}}}),
    ] {
        assert!(edges.contains(&wanted), "missing edge {wanted}");
    }
    assert_eq!(
        hive["params"]["contract"]["accepts"]
            .as_array()
            .map(Vec::len),
        Some(3),
        "the hive's lanes: in_view, in_withdraw, in_notice -- the clock adds none"
    );

    let mut stack = vec![repo("templates/display")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("readable") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "json") {
                let text = std::fs::read_to_string(&path).expect("utf-8");
                assert!(
                    !text.contains("\"cron\""),
                    "{} carries a cron: the screen orders one-shot strikes only",
                    path.display()
                );
            }
        }
    }
}

/// The README no longer says "nothing sweeps": the screen's own clock strikes
/// when a view is due, and the next pass takes it down -- pinned against the
/// mechanism: on a tick, an expired row drops out of the plan, the window it
/// drew is laid back as `leaving`, and the pass after deletes it.
#[test]
fn the_readme_says_the_clock_sweeps() {
    if !library_ships() {
        return;
    }
    let readme = std::fs::read_to_string(repo("templates/display/README.md")).expect("README");
    assert!(
        readme.contains("strikes when a view is due"),
        "the README says the clock strikes when a view is due"
    );
    assert!(
        !readme.contains("**Nothing sweeps.**"),
        "the README still says nothing sweeps"
    );
    assert!(readme.contains("`in_tick`"), "the README names the lane");
    let template = read_json(&repo("templates/display/template.json"));
    let purpose = template["description"]["purpose"]
        .as_str()
        .expect("purpose");
    assert!(purpose.contains("`timer` cell"), "{purpose}");
    assert!(purpose.contains("`llm` cell"), "{purpose}");
    assert!(purpose.contains("`in_notice`"), "{purpose}");

    // The mechanism. A row written at t=1 with one second to live is long
    // expired when the tick reads the table.
    let mut row = component_view(
        "a",
        "main",
        pane("a", json!({"context": "conversation", "relevance": 0.7})),
    );
    row["ttl_ms"] = json!(1000);
    let Some(tick) = run(&json!({
        "params": {},
        "body": {"messages": [{
            "origin": "tool", "type": "tool_result", "id": "d-select",
            "text": Value::Array(vec![row.clone()]).to_string(),
        }]},
        "envelope": {"header": {
            "hop": {},
            "context": {
                "display_origin": "views",
                "display_request": json!({"tick": true}).to_string(),
            },
        }},
    })) else {
        return;
    };
    let plan: Value = meclaw_core::serde_json::from_str(
        tick[0]["header"]["display_views"].as_str().expect("a plan"),
    )
    .expect("json");
    assert_eq!(
        plan["views"],
        json!([]),
        "the expired view drops out of the plan"
    );

    // The screen still draws it: the read pass lays it back as leaving, and
    // the next one deletes it.
    let (_, mut held) = screen_with_a(1000).expect("python3");
    let leave = read_pass(&[], Some(&held), 5000).expect("python3");
    let calls = patch_calls(&leave);
    assert!(
        !calls.iter().any(|c| c["op"] == "object.delete"),
        "one more frame first: {calls:?}"
    );
    assert_eq!(
        written(&calls, &pane_id("a", "a")).expect("laid back")["age"],
        "leaving"
    );
    apply(&mut held, &calls);
    let gone = read_pass(&[], Some(&held), 6001).expect("python3");
    let calls = patch_calls(&gone);
    assert!(
        calls
            .iter()
            .any(|c| c["op"] == "object.delete" && c["id"] == pane_id("a", "a")),
        "the pass after takes it down: {calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|c| c["op"] == "object.delete" && c["id"] == wrapper("a")),
        "and its wrapper with it: {calls:?}"
    );
}

/// The timer is exact to the second and refuses an `at` in the past, so an
/// order is rounded UP: a pass at 2950 that wants "the next second" (3950)
/// orders `:04Z`, never `:03Z` -- which would already be past by the time
/// the order reaches the timer.
#[test]
fn an_order_is_never_earlier_than_the_next_full_second() {
    if !library_ships() {
        return;
    }
    let Some(boot) = read_pass(&[], None, 2950) else {
        return;
    };
    let mut held = json!([]);
    apply(&mut held, &patch_calls(&boot));
    let views = vec![component_view(
        "a",
        "main",
        pane("a", json!({"context": "conversation", "relevance": 0.7})),
    )];
    let arrive = read_pass(&views, Some(&held), 2950).expect("python3");
    let due = on_route(&arrive, "due");
    assert_eq!(due.len(), 1, "{arrive:?}");
    assert_eq!(due[0]["at"], "1970-01-01T00:00:04Z", "rounded up, not down");
    assert_eq!(due[0]["at"], iso_z(3950));
}

/// A window past its `relevant_until` scores with `r` clamped to 0.2, and the
/// clock is ordered for a crossing of THAT score: with the bar at 0 (the
/// window stands alone in `main` below `focus_default`) the only crossing
/// left is zero, at `since + linger + fade + 1`, not the midpoint the
/// unclamped 0.7 would have crossed at 55 s.
#[test]
fn a_window_past_its_relevance_orders_no_empty_strike() {
    if !library_ships() {
        return;
    }
    let views = vec![component_view(
        "a",
        "main",
        pane(
            "a",
            json!({"context": "conversation", "relevance": 0.7, "relevant_until": 3000}),
        ),
    )];
    let Some(boot) = read_pass(&[], None, 1000) else {
        return;
    };
    let mut held = json!([]);
    apply(&mut held, &patch_calls(&boot));
    let arrive = read_pass(&views, Some(&held), 1000).expect("python3");
    apply(&mut held, &patch_calls(&arrive));
    let settle = read_pass(&views, Some(&held), 1100).expect("python3");
    apply(&mut held, &patch_calls(&settle));
    set_prop(&mut held, ROOT, "due", json!(""));

    let past = read_pass(&views, Some(&held), 5000).expect("python3");
    let a = written(&patch_calls(&past), &pane_id("a", "a")).expect("updated");
    assert_eq!(a["score"], 0.2, "clamped after relevant_until: {a}");
    let due = on_route(&past, "due");
    assert_eq!(due.len(), 1, "{past:?}");
    assert_eq!(
        due[0]["at"],
        iso_z(1000 + LINGER + FADE + 1),
        "the zero crossing, not the midpoint of the unclamped score"
    );
}

/// The order that just struck is gone from the timer; a tick pass does not
/// ask to remove it. The strike's `schedule_id` rides `display_request` as
/// `struck`, pass 2 hands it on in the plan, and `due_ops` skips the remove
/// when the standing id is the one that struck.
#[test]
fn a_tick_does_not_remove_the_order_that_struck() {
    if !library_ships() {
        return;
    }
    let Some(tick) = run(&json!({
        "params": {},
        "body": {"messages": []},
        "envelope": {"header": {"hop": {
            "route": "in_tick", "schedule_name": "due", "schedule_id": "abc-123",
        }}},
    })) else {
        return;
    };
    let request: Value = meclaw_core::serde_json::from_str(
        tick[0]["header"]["display_request"]
            .as_str()
            .expect("a request"),
    )
    .expect("json");
    assert_eq!(request, json!({"tick": true, "struck": "abc-123"}));

    let after = run(&json!({
        "params": {},
        "body": {"messages": [{
            "origin": "tool", "type": "tool_result", "id": "d-select", "text": "[]",
        }]},
        "envelope": {"header": {
            "hop": {},
            "context": {"display_origin": "views", "display_request": request.to_string()},
        }},
    }))
    .expect("python3");
    let plan: Value = meclaw_core::serde_json::from_str(
        after[0]["header"]["display_views"]
            .as_str()
            .expect("a plan"),
    )
    .expect("json");
    assert_eq!(plan["struck"], "abc-123");

    let (views, mut held) = screen_with_a(1000).expect("python3");
    let order = read_pass(&views, Some(&held), 2000).expect("python3");
    let sid = on_route(&order, "due")[0]["schedule_id"]
        .as_str()
        .expect("id")
        .to_string();
    apply(&mut held, &patch_calls(&order));
    let struck = read_pass_struck(&views, Some(&held), 30000, Some(&sid)).expect("python3");
    let ops: Vec<_> = on_route(&struck, "due")
        .iter()
        .map(|o| o["op"].clone())
        .collect();
    assert_eq!(
        ops,
        vec![json!("add")],
        "no remove for the order that struck: {struck:?}"
    );
    let other =
        read_pass_struck(&views, Some(&held), 30000, Some("some-other-id")).expect("python3");
    let ops: Vec<_> = on_route(&other, "due")
        .iter()
        .map(|o| o["op"].clone())
        .collect();
    assert_eq!(ops, vec![json!("remove"), json!("add")]);
}
