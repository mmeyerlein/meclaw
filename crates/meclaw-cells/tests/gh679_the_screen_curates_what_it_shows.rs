//! GH #679 -- the screen curates what it shows.
//!
//! Before this, no window on the screen carried a state: the sheet's ladder
//! waited for an attribute nobody wrote, and every window had the same weight.
//! Now every window carries `context` and `relevance` as hints, the root
//! carries `focus` (the bar, 0-1) and per-context `weights`, and the compose
//! cell scores each window (`w x r x decay`), hides what falls below the bar,
//! gives exactly one settled window in `main` the focus rung, and lets an
//! application say only `urgent` and `hidden`.
//!
//! The script is run the way a `code` cell runs it: as a subprocess, with the
//! read-pass document on stdin, and the bundle it answers is what the display
//! would receive. The objects a pass creates are what the display holds on the
//! next one, so a second pass reads the first pass's answer. Skips when
//! `python3` is absent or the templates do not ship, like every other
//! interpreter guard in this tree (R2b).

use std::io::Write;
use std::process::{Command, Stdio};

use meclaw_core::serde_json::{Value, json};

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

/// The six props the curator writes on a window, and on nothing else.
const CURATOR: [&str; 6] = [
    "state",
    "age",
    "since",
    "score",
    "judged_relevance",
    "judged_hidden",
];

/// The wrapper id of a view the persona `alex` owns.
fn wrapper(view_id: &str) -> String {
    format!("view.alex.{view_id}")
}

/// The id of a keyed pane under a view of `alex`.
fn pane_id(view_id: &str, pane: &str) -> String {
    format!("view.alex.{view_id}/c.{pane}")
}

/// One `display-pane` tree node with its own key, and whatever props the
/// application says about it.
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

/// One row of the `views` table: a prose view of `alex`.
fn prose_view(view_id: &str, region: &str, title: &str, body: &str) -> Value {
    json!({
        "owner": "alex", "view_id": view_id, "region": region, "ord": 0,
        "kind": "prose", "content": json!({"title": title, "body": body}).to_string(),
        "components": "[]", "ttl_ms": 0, "updated_at": 1,
    })
}

/// Run the shipped script over one document on stdin, the way a `code` cell
/// does, and return the calls of the bundle it answers with -- each one the
/// parsed `text` of a `tool_call` turn. An empty answer is an empty list.
/// `None` when there is no `python3` on this host.
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
    // Since the due clock (GH #679) a read pass may answer with the patch
    // AND up to two timer orders beside it; the patch is what the display
    // gets, and it is the one bundle read here.
    let emissions = match answer {
        Value::Array(list) => list,
        one @ Value::Object(_) => vec![one],
        other => panic!("emissions are objects: {other}"),
    };
    let patches: Vec<&Value> = emissions
        .iter()
        .filter(|e| e["header"]["route"] == "patch")
        .collect();
    assert!(patches.len() <= 1, "at most one patch: {emissions:?}");
    let calls = match patches.first() {
        None => Vec::new(),
        Some(emission) => emission["messages"]
            .as_array()
            .expect("a bundle has messages")
            .iter()
            .map(|turn| {
                assert_eq!(turn["type"], "tool_call");
                meclaw_core::serde_json::from_str(turn["text"].as_str().expect("a call"))
                    .expect("a call is JSON")
            })
            .collect(),
    };
    Some(calls)
}

/// A read pass: the table holds `views`, the display answers the `query` with
/// `objects` (nothing at all before the first bootstrap), the clock says `now`,
/// and the cell runs with `params` on top of the shipped ones.
fn read_pass_with(
    views: &[Value],
    objects: Option<&Value>,
    now: u64,
    params: Value,
) -> Option<Vec<Value>> {
    let messages = match objects {
        None => json!([]),
        Some(objs) => json!([{
            "origin": "tool",
            "type": "tool_result",
            "id": "d-query",
            "text": json!({"objects": objs}).to_string(),
        }]),
    };
    let doc = json!({
        "params": params,
        "body": {"messages": messages},
        "envelope": {"header": {
            "hop": {"operation": "query"},
            "context": {
                "display_origin": "read",
                "display_views": json!({"views": views, "define": [], "now": now}).to_string(),
            },
        }},
    });
    run(&doc)
}

fn read_pass(views: &[Value], objects: Option<&Value>, now: u64) -> Option<Vec<Value>> {
    read_pass_with(views, objects, now, json!({}))
}

/// The objects a bootstrap creates, as the display would hold them and answer
/// a later `query` with.
fn held_after(calls: &[Value]) -> Value {
    let mut held = json!([]);
    apply(&mut held, calls);
    held
}

/// Play the display: apply a bundle's creates, updates and deletes to what it
/// holds, so the next pass reads what this one wrote. `object.update` merges
/// per key, like the cell's own.
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

/// The empty screen: root, regions and microphone, as a bootstrap leaves them.
fn bare_screen() -> Option<Value> {
    Some(held_after(&read_pass(&[], None, 1000)?))
}

/// The props a bundle writes on `id` -- from its create or its update, or
/// `None` when the bundle leaves it alone.
fn written(calls: &[Value], id: &str) -> Option<Value> {
    calls
        .iter()
        .find(|c| (c["op"] == "object.create" || c["op"] == "object.update") && c["id"] == id)
        .map(|c| c["props"].clone())
}

/// The `state` a bundle writes on `id`, or `None` when it writes none.
fn state_of(calls: &[Value], id: &str) -> Option<String> {
    written(calls, id)?["state"].as_str().map(str::to_string)
}

fn ops(calls: &[Value]) -> Vec<String> {
    calls
        .iter()
        .map(|c| format!("{} {}", c["op"].as_str().unwrap_or(""), c["id"]))
        .collect()
}

/// Every call that carries a `state`, as `(id, state)`.
fn states(calls: &[Value]) -> Vec<(String, String)> {
    calls
        .iter()
        .filter_map(|c| {
            Some((
                c["id"].as_str()?.to_string(),
                c["props"]["state"].as_str()?.to_string(),
            ))
        })
        .collect()
}

/// Two panes in `main` (`a` at 0.7, `b` at 0.5) and a prose view in `aside`,
/// on a screen that holds nothing yet.
fn three_views() -> Vec<Value> {
    vec![
        component_view(
            "a",
            "main",
            pane("a", json!({"context": "conversation", "relevance": 0.7})),
        ),
        component_view(
            "b",
            "main",
            pane("b", json!({"context": "conversation", "relevance": 0.5})),
        ),
        prose_view("p", "aside", "Weather", "Sunny, 21 degrees"),
    ]
}

/// A fresh window is at most `relevant`: on the pass that brings the three
/// windows nothing has the focus. One pass later, all of them settled, exactly
/// one window carries `focus`, it is the highest score in `main`, and the
/// window in `aside` never does.
#[test]
fn exactly_one_window_has_focus() {
    if !library_ships() {
        return;
    }
    let Some(base) = bare_screen() else {
        return;
    };
    let views = three_views();
    let first = read_pass(&views, Some(&base), 1000).expect("python3 answered once already");
    let seen = states(&first);
    assert!(!seen.is_empty(), "no call carries state: {:?}", ops(&first));
    assert!(
        seen.iter().all(|(_, s)| s != "focus"),
        "a fresh window is never the focus: {seen:?}"
    );
    for id in [wrapper("p"), pane_id("a", "a"), pane_id("b", "b")] {
        let props = written(&first, &id).unwrap_or_else(|| panic!("{id} is created"));
        assert_eq!(props["age"], "fresh", "{id} arrives fresh: {props}");
        assert!(props["since"].is_number(), "{id} carries since: {props}");
        assert!(props["score"].is_number(), "{id} carries a score: {props}");
    }

    let mut held = base.clone();
    apply(&mut held, &first);
    let second = read_pass(&views, Some(&held), 2000).expect("python3");
    let focus: Vec<_> = states(&second)
        .into_iter()
        .filter(|(_, s)| s == "focus")
        .collect();
    assert_eq!(
        focus.len(),
        1,
        "exactly one window takes the focus: {:?}",
        states(&second)
    );
    assert_eq!(focus[0].0, pane_id("a", "a"), "the highest score in main");
    // All three windows were touched in the same pass, and a tie on `since`
    // falls to the greatest id: the prose view's, whose context is its
    // owner's (`alex`, since a prose view without a hint stands in its
    // owner's context). So `conversation` weighs 0.5: `a` 0.35 keeps the
    // focus as the highest in main, `b` 0.25 falls under the bar of 0.3 --
    // hidden, and never the focus.
    assert_eq!(
        state_of(&second, &pane_id("b", "b")).as_deref(),
        Some("hidden")
    );
    // The prose view: 1.0 x 0.5 = 0.5, visible, under the midpoint 0.65 --
    // ambient, and in `aside` never the focus.
    let aside = state_of(&second, &wrapper("p"));
    assert!(
        matches!(
            aside.as_deref(),
            Some("hidden") | Some("relevant") | Some("ambient")
        ),
        "the aside window is never the focus: {aside:?}"
    );
    for id in [wrapper("p"), pane_id("a", "a"), pane_id("b", "b")] {
        assert_eq!(
            written(&second, &id).expect("updated")["age"],
            "settled",
            "{id}"
        );
    }
}

/// The curator writes on windows and on nothing else: a content component
/// under a pane carries none of the six props, the regions carry none, and
/// the root carries the bar and the weights.
#[test]
fn the_curator_touches_only_windows() {
    if !library_ships() {
        return;
    }
    let Some(base) = bare_screen() else {
        return;
    };
    let tree = json!({
        "component": "display-pane",
        "props": {"pane_id": "a", "context": "conversation", "relevance": 0.7},
        "key": "c.a",
        "children": [{"component": "display-value", "props": {"value": "21"}, "key": "v"}],
    });
    let views = vec![component_view("a", "main", tree)];
    let calls = read_pass(&views, Some(&base), 1000).expect("python3 answered once already");
    let value = written(&calls, "view.alex.a/c.a/v").expect("the value is created");
    for key in CURATOR {
        assert!(
            value.get(key).is_none(),
            "a content component carries no `{key}`: {value}"
        );
    }
    for region in ["display.region.main", "display.region.aside"] {
        if let Some(props) = written(&calls, region) {
            for key in CURATOR {
                assert!(
                    props.get(key).is_none(),
                    "{region} carries no `{key}`: {props}"
                );
            }
        }
    }
    let root = written(&calls, "display.root").expect("the root is brought up to date");
    assert!(
        root["focus"].is_number(),
        "the root carries the bar: {root}"
    );
    let weights: Value = meclaw_core::serde_json::from_str(
        root["weights"]
            .as_str()
            .expect("the weights are a JSON string"),
    )
    .expect("the weights parse");
    assert_eq!(
        weights,
        json!({"conversation": 1.0}),
        "the touched context weighs 1"
    );
}

/// A screen with the bar at 0.3 (a judge wrote it), weights on `conversation`
/// only, and one settled window `c` in `aside` with relevance 0.3 in the
/// `ambient` context: its score is 0.5 x 0.3 = 0.15, below the bar, hidden.
/// The same window on a screen with an empty `main` and no judge: the floor
/// lowers the bar to 0, and 0.15 is visible -- `ambient`, under the midpoint.
#[test]
fn a_window_below_the_bar_is_hidden() {
    if !library_ships() {
        return;
    }
    let Some(base) = bare_screen() else {
        return;
    };
    let views = vec![component_view(
        "c",
        "aside",
        pane("c", json!({"context": "ambient", "relevance": 0.3})),
    )];
    let first = read_pass(&views, Some(&base), 1000).expect("python3 answered once already");
    let mut held = base.clone();
    apply(&mut held, &first);
    // A judge spoke: the bar stands at 0.3, and the weights favour the conversation.
    let mut judged = held.clone();
    {
        let root = judged
            .as_array_mut()
            .expect("a list")
            .iter_mut()
            .find(|o| o["id"] == "display.root")
            .expect("root");
        root["props"]["focus"] = json!("0.3");
        root["props"]["judged_at"] = json!(1);
        root["props"]["weights"] = json!(json!({"conversation": 1.0}).to_string());
    }
    let calls = read_pass(&views, Some(&judged), 2000).expect("python3");
    let c = written(&calls, &pane_id("c", "c")).expect("c is updated");
    assert_eq!(c["score"], 0.15, "0.5 x 0.3: {c}");
    assert_eq!(c["state"], "hidden", "below the bar: {c}");

    // No judge, and nothing in main: the floor lowers the bar to 0.
    let mut floor = held.clone();
    {
        let root = floor
            .as_array_mut()
            .expect("a list")
            .iter_mut()
            .find(|o| o["id"] == "display.root")
            .expect("root");
        root["props"]["weights"] = json!(json!({"conversation": 1.0}).to_string());
    }
    let calls = read_pass(&views, Some(&floor), 2000).expect("python3");
    let c = written(&calls, &pane_id("c", "c")).expect("c is updated");
    assert_eq!(
        c["state"], "ambient",
        "visible on an empty screen, under the midpoint: {c}"
    );
    let root = written(&calls, "display.root").expect("the root is updated");
    assert_eq!(root["focus"], 0.0, "an empty main lowers the bar: {root}");
}

/// The state is the curator's word. An application may say `urgent` -- the
/// window takes the urgent rung and nothing else is the focus -- or `hidden`.
/// Any other word (`focus`) is dropped at the door, and the window gets the
/// rung its score earns. Two urgent windows: the younger stays, the older is
/// `relevant`.
#[test]
fn an_app_may_say_urgent_and_hidden_and_nothing_else() {
    if !library_ships() {
        return;
    }
    let Some(base) = bare_screen() else {
        return;
    };
    let settle = |views: &[Value]| -> Value {
        let first = read_pass(views, Some(&base), 1000).expect("python3");
        let mut held = base.clone();
        apply(&mut held, &first);
        held
    };
    let plain = vec![
        component_view(
            "a",
            "main",
            pane("a", json!({"context": "talk", "relevance": 0.7})),
        ),
        component_view(
            "b",
            "main",
            pane("b", json!({"context": "talk", "relevance": 0.5})),
        ),
        component_view(
            "c",
            "aside",
            pane("c", json!({"context": "talk", "relevance": 0.5})),
        ),
    ];
    let held = settle(&plain);

    // `urgent` on b: b is urgent, and nobody is the focus.
    let mut views = plain.clone();
    views[1] = component_view(
        "b",
        "main",
        pane(
            "b",
            json!({"context": "talk", "relevance": 0.5, "state": "urgent"}),
        ),
    );
    let calls = read_pass(&views, Some(&held), 2000).expect("python3");
    assert_eq!(
        state_of(&calls, &pane_id("b", "b")).as_deref(),
        Some("urgent")
    );
    assert!(
        states(&calls).iter().all(|(_, s)| s != "focus"),
        "an urgent window leaves no room for a focus: {:?}",
        states(&calls)
    );

    // `hidden` on c: hidden.
    let mut views = plain.clone();
    views[2] = component_view(
        "c",
        "aside",
        pane(
            "c",
            json!({"context": "talk", "relevance": 0.5, "state": "hidden"}),
        ),
    );
    let calls = read_pass(&views, Some(&held), 2000).expect("python3");
    assert_eq!(
        state_of(&calls, &pane_id("c", "c")).as_deref(),
        Some("hidden")
    );

    // `focus` on a: the word never reaches the curator; a is the focus because
    // its score says so, and the plain views say the same.
    let mut views = plain.clone();
    views[0] = component_view(
        "a",
        "main",
        pane(
            "a",
            json!({"context": "talk", "relevance": 0.7, "state": "focus"}),
        ),
    );
    let claimed = read_pass(&views, Some(&held), 2000).expect("python3");
    let earned = read_pass(&plain, Some(&held), 2000).expect("python3");
    assert_eq!(
        states(&claimed),
        states(&earned),
        "a claimed focus changes nothing"
    );
    assert_eq!(
        state_of(&earned, &pane_id("a", "a")).as_deref(),
        Some("focus")
    );
    // And a window that claims `relevant` (no app word either) is the same.
    let mut views = plain.clone();
    views[1] = component_view(
        "b",
        "main",
        pane(
            "b",
            json!({"context": "talk", "relevance": 0.5, "state": "relevant"}),
        ),
    );
    let claimed = read_pass(&views, Some(&held), 2000).expect("python3");
    assert_eq!(
        states(&claimed),
        states(&earned),
        "a claimed rung changes nothing"
    );

    // Two urgent windows, a since 1000 and b since 2000: the younger stays.
    let mut both = plain.clone();
    both[0] = component_view(
        "a",
        "main",
        pane(
            "a",
            json!({"context": "talk", "relevance": 0.7, "state": "urgent"}),
        ),
    );
    both[1] = component_view(
        "b",
        "main",
        pane(
            "b",
            json!({"context": "talk", "relevance": 0.5, "state": "urgent"}),
        ),
    );
    let mut held = settle(&both);
    {
        let list = held.as_array_mut().expect("a list");
        list.iter_mut()
            .find(|o| o["id"] == pane_id("b", "b"))
            .expect("b")["props"]["since"] = json!(2000);
    }
    let calls = read_pass(&both, Some(&held), 3000).expect("python3");
    assert_eq!(
        state_of(&calls, &pane_id("b", "b")).as_deref(),
        Some("urgent"),
        "the younger"
    );
    assert_eq!(
        state_of(&calls, &pane_id("a", "a")).as_deref(),
        Some("relevant"),
        "the older"
    );
}

// ---------------------------------------------------------------------------
// Touch, since, decay, pinned, relevant_until (Task 2)

/// Set one prop on one held object.
fn set_held(held: &mut Value, id: &str, key: &str, value: Value) {
    held.as_array_mut()
        .expect("a list")
        .iter_mut()
        .find(|o| o["id"] == id)
        .unwrap_or_else(|| panic!("the display holds {id}"))["props"][key] = value;
}

/// Bring `views` onto the bare screen and return what the display holds.
fn settled(views: &[Value]) -> Option<Value> {
    let base = bare_screen()?;
    let first = read_pass(views, Some(&base), 1000)?;
    let mut held = base;
    apply(&mut held, &first);
    Some(held)
}

/// `a` has the focus (since 1000) and `b` stands relevant (since 900), both
/// at 0.7. `b` comes back with another title: it is touched, its `since` is
/// now, and at equal scores the younger touch wins -- `b` takes the focus at
/// once, `a` steps down to relevant.
#[test]
fn a_touched_window_takes_the_focus_at_once() {
    if !library_ships() {
        return;
    }
    let views = vec![
        component_view(
            "a",
            "main",
            pane(
                "a",
                json!({"context": "talk", "relevance": 0.7, "title": "A"}),
            ),
        ),
        component_view(
            "b",
            "main",
            pane(
                "b",
                json!({"context": "talk", "relevance": 0.7, "title": "B"}),
            ),
        ),
    ];
    let Some(mut held) = settled(&views) else {
        return;
    };
    set_held(&mut held, &pane_id("a", "a"), "since", json!(1000));
    set_held(&mut held, &pane_id("a", "a"), "state", json!("focus"));
    set_held(&mut held, &pane_id("b", "b"), "since", json!(900));
    set_held(&mut held, &pane_id("b", "b"), "state", json!("relevant"));
    let mut touched = views.clone();
    touched[1] = component_view(
        "b",
        "main",
        pane(
            "b",
            json!({"context": "talk", "relevance": 0.7, "title": "B, again"}),
        ),
    );
    let calls = read_pass(&touched, Some(&held), 5000).expect("python3");
    let b = written(&calls, &pane_id("b", "b")).expect("b is updated");
    assert_eq!(b["since"], 5000, "a touch is the moment of the touch: {b}");
    assert_eq!(b["state"], "focus", "the younger touch wins the tie: {b}");
    let a = written(&calls, &pane_id("a", "a")).expect("a is updated");
    assert_eq!(a["state"], "relevant", "{a}");
    assert!(
        a.get("since").is_none() || a["since"] == 1000,
        "a was not touched: {a}"
    );
}

/// The focus window, untouched, fades from its last touch: after `linger_ms`
/// plus half of `fade_ms` its score is halved (0.35) and `since` still says
/// 1000 -- since is the last touch, never a change of rung (P-C1). Alone in
/// `main` it keeps the focus while it is visible; after the whole fade its
/// score is 0 and it is hidden.
#[test]
fn a_focus_decays_and_stands_since_untouched() {
    if !library_ships() {
        return;
    }
    let views = vec![component_view(
        "a",
        "main",
        pane("a", json!({"context": "talk", "relevance": 0.7})),
    )];
    let Some(mut held) = settled(&views) else {
        return;
    };
    set_held(&mut held, &pane_id("a", "a"), "since", json!(1000));
    set_held(&mut held, &pane_id("a", "a"), "state", json!("focus"));
    let calls = read_pass(&views, Some(&held), 1000 + 20000 + 60000).expect("python3");
    let a = written(&calls, &pane_id("a", "a")).expect("a is updated");
    assert_eq!(a["score"], 0.35, "decay 0.5 after half the fade: {a}");
    assert_eq!(
        a["state"], "focus",
        "still the highest in main while visible: {a}"
    );
    assert!(
        a.get("since").is_none() || a["since"] == 1000,
        "since stands: {a}"
    );
    // Since is the last touch: the display still holds 1000 and the cell
    // writes nothing else.
    assert!(
        !calls.iter().any(|c| c["id"] == pane_id("a", "a")
            && c["props"]["since"] != json!(1000)
            && c["props"].get("since").is_some()),
        "since is never moved by a change of rung: {calls:?}"
    );

    let calls = read_pass(&views, Some(&held), 1000 + 20000 + 120000).expect("python3");
    let a = written(&calls, &pane_id("a", "a")).expect("a is updated");
    assert_eq!(a["score"], 0.0, "faded out: {a}");
    assert_eq!(a["state"], "hidden", "{a}");
}

/// `pinned` freezes the decay: far past the fade the window scores as if it
/// had just been touched. `relevant_until` in the past caps the relevance at
/// 0.2, and 0.2 is below the bar of 0.3 -- hidden.
#[test]
fn pinned_and_relevant_until_are_hints_the_score_reads() {
    if !library_ships() {
        return;
    }
    let views = vec![
        component_view(
            "a",
            "main",
            pane(
                "a",
                json!({"context": "talk", "relevance": 0.7, "pinned": true}),
            ),
        ),
        component_view(
            "b",
            "main",
            pane(
                "b",
                json!({"context": "talk", "relevance": 0.7, "relevant_until": 2000}),
            ),
        ),
    ];
    let Some(mut held) = settled(&views) else {
        return;
    };
    set_held(&mut held, &pane_id("a", "a"), "since", json!(1000));
    set_held(&mut held, &pane_id("b", "b"), "since", json!(1000));
    let calls = read_pass(&views, Some(&held), 1000 + 20000 + 500000).expect("python3");
    let a = written(&calls, &pane_id("a", "a")).expect("a is updated");
    assert_eq!(a["score"], 0.7, "pinned: decay 1 far past the fade: {a}");
    assert_eq!(a["state"], "focus", "{a}");
    let b = written(&calls, &pane_id("b", "b")).expect("b is updated");
    assert_eq!(b["state"], "hidden", "past relevant_until, and faded: {b}");

    // relevant_until alone, inside the linger: r is capped at 0.2, under the bar.
    let calls = read_pass(&views, Some(&held), 3000).expect("python3");
    let b = written(&calls, &pane_id("b", "b")).expect("b is updated");
    assert_eq!(b["score"], 0.2, "r capped at 0.2 past relevant_until: {b}");
    assert_eq!(b["state"], "hidden", "0.2 is below the bar of 0.3: {b}");
}

/// `components()` as the shipped script defines them. `None` without python3.
fn components() -> Option<Vec<Value>> {
    let out = Command::new("python3")
        .arg("-c")
        .arg(
            "import importlib.util, json, sys\n\
             spec = importlib.util.spec_from_file_location('compose', sys.argv[1])\n\
             m = importlib.util.module_from_spec(spec)\n\
             spec.loader.exec_module(m)\n\
             print(json.dumps(m.components()))",
        )
        .arg(repo(COMPOSE))
        .output()
        .ok()?;
    assert!(
        out.status.success(),
        "compose.py did not load:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = meclaw_core::serde_json::from_slice(&out.stdout).expect("components() is JSON");
    Some(v.as_array().expect("a list").clone())
}

/// The `web` cell refuses an undeclared prop, so a hint an application may
/// send has to stand in the window's schema: the five hints and the four
/// curator props beside `state` and `age`, on the three tree windows; `tone`
/// on the two that have a title to colour.
#[test]
fn the_windows_declare_the_hints_in_their_schema() {
    if !library_ships() {
        return;
    }
    let Some(list) = components() else {
        return;
    };
    let schema = |name: &str| -> Value {
        list.iter()
            .find(|c| c["name"] == name)
            .unwrap_or_else(|| panic!("components() defines {name}"))["prop_schema"]
            .clone()
    };
    let expected = json!({
        "context": "text", "relevance": "text", "class": "text",
        "pinned": "boolean", "relevant_until": "int",
        "since": "text", "score": "text",
        "judged_relevance": "text", "judged_hidden": "boolean",
    });
    for name in ["display-pane", "display-panel", "display-overlay"] {
        let s = schema(name);
        for (key, ty) in expected.as_object().expect("a map") {
            assert_eq!(&s[key], ty, "{name} declares `{key}` as {ty}: {s}");
        }
        assert_eq!(s["state"], "text", "{name}");
        assert_eq!(s["age"], "text", "{name}");
    }
    for name in ["display-pane", "display-panel"] {
        assert_eq!(schema(name)["tone"], "text", "{name} declares tone");
    }
}

// ---------------------------------------------------------------------------
// Two frames: a window leaves one strike later (Task 3)

fn deletes(calls: &[Value]) -> Vec<String> {
    calls
        .iter()
        .filter(|c| c["op"] == "object.delete")
        .map(|c| c["id"].as_str().unwrap_or("").to_string())
        .collect()
}

/// The display holds window `b` with a child; the table no longer has it.
/// The pass that notices does not delete: it lays `b` back with
/// `age: leaving` and touches nothing under it. The next pass finds it
/// already leaving and deletes the child, then the window, then its wrapper.
#[test]
fn a_window_that_leaves_stands_one_more_frame() {
    if !library_ships() {
        return;
    }
    let tree_b = json!({
        "component": "display-pane",
        "props": {"pane_id": "b", "context": "talk", "relevance": 0.5},
        "key": "c.b",
        "children": [{"component": "display-value", "props": {"value": "21"}, "key": "v"}],
    });
    let views = vec![
        component_view(
            "a",
            "main",
            pane("a", json!({"context": "talk", "relevance": 0.7})),
        ),
        component_view("b", "main", tree_b),
    ];
    let Some(held) = settled(&views) else {
        return;
    };
    let without_b = vec![views[0].clone()];
    let first = read_pass(&without_b, Some(&held), 2000).expect("python3");
    assert!(
        deletes(&first).is_empty(),
        "nothing is deleted on the frame a window leaves in: {:?}",
        ops(&first)
    );
    let b = written(&first, &pane_id("b", "b")).expect("b is laid back as leaving");
    assert_eq!(b["age"], "leaving", "{b}");
    assert!(
        written(&first, "view.alex.b/c.b/v").is_none(),
        "the child under a ghost is not touched: {:?}",
        ops(&first)
    );

    let mut next = held.clone();
    apply(&mut next, &first);
    let second = read_pass(&without_b, Some(&next), 3000).expect("python3");
    assert_eq!(
        deletes(&second),
        vec!["view.alex.b/c.b/v", "view.alex.b/c.b", "view.alex.b"],
        "leaf first, then the window, then its wrapper: {:?}",
        ops(&second)
    );
}

/// A window that is on its way out and comes back in the same breath does not
/// fly in again: it is `settled`, never `fresh`.
#[test]
fn a_window_that_returns_while_leaving_does_not_fly_in_again() {
    if !library_ships() {
        return;
    }
    let views = vec![
        component_view(
            "a",
            "main",
            pane("a", json!({"context": "talk", "relevance": 0.7})),
        ),
        component_view(
            "b",
            "main",
            pane("b", json!({"context": "talk", "relevance": 0.5})),
        ),
    ];
    let Some(mut held) = settled(&views) else {
        return;
    };
    set_held(&mut held, &pane_id("b", "b"), "age", json!("leaving"));
    let calls = read_pass(&views, Some(&held), 2000).expect("python3");
    let b = written(&calls, &pane_id("b", "b")).expect("b is updated");
    assert_eq!(b["age"], "settled", "no second entrance: {b}");
}

/// `c` arrived fresh on the last pass at 0.8 beside the focus `a` at 0.7.
/// The next pass, with nothing touched, settles `c` and hands it the focus;
/// `a` steps down to relevant.
#[test]
fn a_fresh_window_takes_the_focus_on_the_next_pass() {
    if !library_ships() {
        return;
    }
    let views = vec![
        component_view(
            "a",
            "main",
            pane("a", json!({"context": "talk", "relevance": 0.7})),
        ),
        component_view(
            "c",
            "main",
            pane("c", json!({"context": "talk", "relevance": 0.8})),
        ),
    ];
    let Some(mut held) = settled(&views) else {
        return;
    };
    set_held(&mut held, &pane_id("a", "a"), "since", json!(1000));
    set_held(&mut held, &pane_id("a", "a"), "state", json!("focus"));
    set_held(&mut held, &pane_id("c", "c"), "since", json!(5000));
    set_held(&mut held, &pane_id("c", "c"), "state", json!("relevant"));
    set_held(&mut held, &pane_id("c", "c"), "age", json!("fresh"));
    let calls = read_pass(&views, Some(&held), 5500).expect("python3");
    let c = written(&calls, &pane_id("c", "c")).expect("c is updated");
    assert_eq!(c["state"], "focus", "{c}");
    assert_eq!(c["age"], "settled", "{c}");
    let a = written(&calls, &pane_id("a", "a")).expect("a is updated");
    assert_eq!(a["state"], "relevant", "{a}");
}

/// `a` stands relevant and visible; the judge's weights push its score under
/// the bar. It is hidden AND leaving in the same update, so the sheet plays
/// the leave before `display: none` takes hold. The next pass keeps it -- the
/// view lives -- as hidden and settled, and deletes nothing.
#[test]
fn a_window_pushed_below_the_bar_leaves_first() {
    if !library_ships() {
        return;
    }
    let views = vec![
        component_view(
            "a",
            "main",
            pane("a", json!({"context": "talk", "relevance": 0.5})),
        ),
        component_view(
            "b",
            "main",
            pane("b", json!({"context": "other", "relevance": 0.7})),
        ),
    ];
    let Some(mut held) = settled(&views) else {
        return;
    };
    set_held(&mut held, &pane_id("a", "a"), "state", json!("relevant"));
    set_held(&mut held, "display.root", "focus", json!("0.3"));
    set_held(&mut held, "display.root", "judged_at", json!(1500));
    set_held(
        &mut held,
        "display.root",
        "weights",
        json!(json!({"talk": 0.2, "other": 1.0}).to_string()),
    );
    let first = read_pass(&views, Some(&held), 2000).expect("python3");
    let a = written(&first, &pane_id("a", "a")).expect("a is updated");
    assert_eq!(a["score"], 0.1, "0.2 x 0.5: {a}");
    assert_eq!(a["state"], "hidden", "{a}");
    assert_eq!(a["age"], "leaving", "hidden and leaving in one update: {a}");
    assert!(deletes(&first).is_empty(), "{:?}", ops(&first));

    let mut next = held.clone();
    apply(&mut next, &first);
    let second = read_pass(&views, Some(&next), 3000).expect("python3");
    assert!(
        deletes(&second).is_empty(),
        "the view lives: {:?}",
        ops(&second)
    );
    let a = written(&second, &pane_id("a", "a")).expect("a is updated");
    assert_eq!(a["state"], "hidden", "{a}");
    assert_eq!(a["age"], "settled", "{a}");
}

// ---------------------------------------------------------------------------
// The knobs (Task 5)

/// The seven dials of the judgement stand in `params` and in
/// `contract.settings` with the same default, byte for byte -- and the cell
/// reads them: with `linger_ms: 5` and `fade_ms: 10` a window untouched for
/// 16 ms has faded to nothing and is hidden.
#[test]
fn the_knobs_stand_in_params_and_settings_alike() {
    if !library_ships() {
        return;
    }
    let cfg: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(repo("templates/display/compose/config.json")).expect("config"),
    )
    .expect("config.json parses");
    let expected = json!({
        "linger_ms": 20000,
        "fade_ms": 120000,
        "focus_default": 0.3,
        "ground": "day",
        "judge": "off",
        "judge_min_interval_ms": 3000,
        "notice_defaults": {
            "system_error": [0.9, 60000], "error": [0.8, 60000],
            "warning": [0.7, 120000], "important_note": [0.7, 300000],
            "note": [0.4, 300000],
        },
    });
    for (knob, value) in expected.as_object().expect("a map") {
        assert_eq!(&cfg["params"][knob], value, "params.{knob} as shipped");
        let setting = &cfg["contract"]["settings"][knob];
        assert_eq!(
            &setting["default"], value,
            "contract.settings.{knob}.default"
        );
        assert_eq!(setting["secret"], false, "{knob} is no secret");
        assert!(
            setting["description"]
                .as_str()
                .is_some_and(|d| !d.is_empty()),
            "{knob} carries a description"
        );
        let ty = setting["type"].as_str().unwrap_or("");
        let fits = match value {
            Value::Number(_) => ty == "number",
            Value::String(_) => ty == "string",
            Value::Object(_) => ty == "object",
            _ => false,
        };
        assert!(fits, "{knob}: type {ty:?} fits the default {value}");
    }

    let views = vec![component_view(
        "a",
        "main",
        pane("a", json!({"context": "talk", "relevance": 0.7})),
    )];
    let Some(mut held) = settled(&views) else {
        return;
    };
    set_held(&mut held, &pane_id("a", "a"), "since", json!(1000));
    let calls = read_pass_with(
        &views,
        Some(&held),
        1016,
        json!({"linger_ms": 5, "fade_ms": 10}),
    )
    .expect("python3");
    let a = written(&calls, &pane_id("a", "a")).expect("a is updated");
    assert_eq!(a["score"], 0.0, "faded in 15 ms: {a}");
    assert_eq!(a["state"], "hidden", "{a}");
    // And with the shipped knobs the same window stands: 16 ms is inside the linger.
    let calls = read_pass(&views, Some(&held), 1016).expect("python3");
    let a = written(&calls, &pane_id("a", "a")).expect("a is updated");
    assert_eq!(a["state"], "focus", "{a}");
}

// ---------------------------------------------------------------------------
// The README names the rules the code keeps (Task 6, drift lock)

/// The README says three things about the judgement -- one focus in `main`,
/// hidden below the bar, two frames to leave -- and the cell does each of
/// them. Read together, so a sentence cannot outlive its mechanism.
#[test]
fn the_readme_names_the_rules_the_code_keeps() {
    if !library_ships() {
        return;
    }
    let readme = std::fs::read_to_string(repo("templates/display/README.md")).expect("README");
    assert!(
        readme.starts_with("# `display@2.2.3`"),
        "the H1 names the version"
    );
    for sentence in [
        "exactly one window in `main`",
        "below the bar",
        "two frames",
        "## The screen curates what it shows",
        "A judgement of relevance, not of layout.",
        "`aside` never carries the focus rung",
    ] {
        assert!(readme.contains(sentence), "the README says: {sentence}");
    }
    let template: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(repo("templates/display/template.json")).expect("template.json"),
    )
    .expect("template.json parses");
    assert_eq!(template["version"], "2.2.3");
    assert!(
        template["description"]["purpose"]
            .as_str()
            .or_else(|| template["purpose"].as_str())
            .is_some_and(|p| p.contains("Since 2.2.0 the screen curates what it shows")),
        "the purpose says what 2.2.0 brought: {template}"
    );

    // And the mechanism: one focus in main after a settle, the aside window
    // never it, a window under the bar hidden, and a leaving window standing
    // one more frame.
    let Some(base) = bare_screen() else {
        return;
    };
    let views = three_views();
    let first = read_pass(&views, Some(&base), 1000).expect("python3");
    let mut held = base.clone();
    apply(&mut held, &first);
    let second = read_pass(&views, Some(&held), 2000).expect("python3");
    let focus: Vec<_> = states(&second)
        .into_iter()
        .filter(|(_, s)| s == "focus")
        .collect();
    assert_eq!(focus.len(), 1, "exactly one window in main has the focus");
    assert_eq!(focus[0].0, pane_id("a", "a"));
    apply(&mut held, &second);
    set_held(&mut held, "display.root", "focus", json!("0.9"));
    set_held(&mut held, "display.root", "judged_at", json!(1500));
    let strict = read_pass(&views, Some(&held), 3000).expect("python3");
    let mut after = held.clone();
    apply(&mut after, &strict);
    for id in [pane_id("a", "a"), pane_id("b", "b"), wrapper("p")] {
        let state = after
            .as_array()
            .expect("a list")
            .iter()
            .find(|o| o["id"] == id)
            .expect("held")["props"]["state"]
            .clone();
        assert_eq!(state, "hidden", "{id} is below the bar 0.9");
    }
    let gone = read_pass(&views[..2], Some(&held), 3000).expect("python3");
    assert!(deletes(&gone).is_empty(), "the first frame deletes nothing");
    assert_eq!(
        written(&gone, &wrapper("p")).expect("p is laid back")["age"],
        "leaving"
    );
}

// ---------------------------------------------------------------------------
// Fix round 1 (review C-1, I-2)

/// The weights a bundle writes on the root, parsed.
fn root_weights(calls: &[Value]) -> Value {
    let root = written(calls, "display.root").expect("the root is updated");
    meclaw_core::serde_json::from_str(root["weights"].as_str().expect("weights are a JSON string"))
        .expect("the weights parse")
}

/// Without a judge the floor has one rule and no memory: the context of the
/// last touched window weighs 1, every other context 0.5. A weather window
/// that had the focus loses half its score the moment the conversation is
/// touched, and drops to ambient. With a judge's map on the root the map
/// stands; a touch only adds a context the map does not know.
#[test]
fn the_floor_forgets_the_context_before_the_last_touch() {
    if !library_ships() {
        return;
    }
    let weather = component_view(
        "w",
        "main",
        pane("w", json!({"context": "weather", "relevance": 0.7})),
    );
    let talk = component_view(
        "t",
        "main",
        pane("t", json!({"context": "conversation", "relevance": 0.7})),
    );
    let Some(mut held) = settled(std::slice::from_ref(&weather)) else {
        return;
    };
    let second = read_pass(std::slice::from_ref(&weather), Some(&held), 2000).expect("python3");
    assert_eq!(
        state_of(&second, &pane_id("w", "w")).as_deref(),
        Some("focus")
    );
    apply(&mut held, &second);

    let both = vec![weather.clone(), talk.clone()];
    let third = read_pass(&both, Some(&held), 5000).expect("python3");
    assert_eq!(
        root_weights(&third),
        json!({"conversation": 1.0}),
        "the last touched context weighs 1, weather is forgotten"
    );
    let w = written(&third, &pane_id("w", "w")).expect("w is updated");
    assert_eq!(w["score"], 0.35, "0.5 x 0.7: {w}");
    // Frame N: the conversation window is fresh and no candidate yet, so the
    // old holder keeps the rung for this one frame. Frame N+1: the settled
    // conversation window takes the focus, and weather at 0.35 is under the
    // midpoint 0.65 -- ambient.
    assert_eq!(
        w["state"], "focus",
        "the old holder keeps the rung one frame: {w}"
    );
    let mut after = held.clone();
    apply(&mut after, &third);
    let fourth = read_pass(&both, Some(&after), 6000).expect("python3");
    assert_eq!(
        state_of(&fourth, &pane_id("t", "t")).as_deref(),
        Some("focus")
    );
    let w = written(&fourth, &pane_id("w", "w")).expect("w is updated");
    assert_eq!(
        w["state"], "ambient",
        "half the score, under the midpoint: {w}"
    );

    // A judge spoke: its map stands, and a touch in a context it knows
    // changes nothing about it; a touch in a context it does not know is 1.
    set_held(&mut held, "display.root", "judged_at", json!(4000));
    set_held(&mut held, "display.root", "focus", json!("0.3"));
    set_held(
        &mut held,
        "display.root",
        "weights",
        json!(json!({"weather": 1.0, "conversation": 0.2}).to_string()),
    );
    let judged = read_pass(&both, Some(&held), 5000).expect("python3");
    assert_eq!(
        root_weights(&judged),
        json!({"weather": 1.0, "conversation": 0.2}),
        "the judge's map stands under a touch in a context it knows"
    );
    let other = vec![
        weather.clone(),
        component_view(
            "o",
            "main",
            pane("o", json!({"context": "other", "relevance": 0.7})),
        ),
    ];
    let unknown = read_pass(&other, Some(&held), 5000).expect("python3");
    assert_eq!(
        root_weights(&unknown),
        json!({"weather": 1.0, "conversation": 0.2, "other": 1.0}),
        "a touch in a context the judge does not know weighs 1"
    );
}

/// `object.update` merges per key, so a prop the application said once and
/// leaves out later stands on the screen. Leaving it out is not a touch:
/// `since` stays, and the window keeps fading. Changing it is.
#[test]
fn a_prop_left_out_is_not_a_touch() {
    if !library_ships() {
        return;
    }
    let with_tone = vec![component_view(
        "a",
        "main",
        pane(
            "a",
            json!({"context": "talk", "relevance": 0.7, "tone": "accent"}),
        ),
    )];
    let Some(mut held) = settled(&with_tone) else {
        return;
    };
    set_held(&mut held, &pane_id("a", "a"), "since", json!(1000));
    let without = vec![component_view(
        "a",
        "main",
        pane("a", json!({"context": "talk", "relevance": 0.7})),
    )];
    let calls = read_pass(&without, Some(&held), 5000).expect("python3");
    let a = written(&calls, &pane_id("a", "a"));
    assert!(
        a.as_ref()
            .is_none_or(|p| p.get("since").is_none() || p["since"] == 1000),
        "a prop left out is not a touch: {a:?}"
    );
    let changed = vec![component_view(
        "a",
        "main",
        pane(
            "a",
            json!({"context": "talk", "relevance": 0.7, "tone": "muted"}),
        ),
    )];
    let calls = read_pass(&changed, Some(&held), 5000).expect("python3");
    let a = written(&calls, &pane_id("a", "a")).expect("a is updated");
    assert_eq!(a["since"], 5000, "a changed prop is a touch: {a}");
}

/// The screen compares a window's OWN props, and an answer that lives in a
/// child component is invisible to that comparison -- so the application
/// says `touched` (an epoch, as text) when the window's content is new
/// (GH #689). A changed `touched` is a touch: `since` is now, and nothing
/// else on the pane needs to move. The same props including `touched` are
/// not a touch. And since the `web` cell refuses an undeclared prop, the
/// four windows declare `touched` in their schema, and a prose view's
/// `touched` rides through to its wrapper.
#[test]
fn a_changed_touched_is_a_touch_and_nothing_else_needs_to_move() {
    if !library_ships() {
        return;
    }
    let Some(list) = components() else {
        return;
    };
    for name in [
        "display-pane",
        "display-panel",
        "display-overlay",
        "display-view-prose",
    ] {
        let schema = &list
            .iter()
            .find(|c| c["name"] == name)
            .unwrap_or_else(|| panic!("components() defines {name}"))["prop_schema"];
        assert_eq!(
            schema["touched"], "text",
            "{name} declares touched: {schema}"
        );
    }
    let speech = |touched: &str, answer: &str| {
        let mut node = pane(
            "a",
            json!({"context": "talk", "relevance": 0.7, "title": "Egon", "touched": touched}),
        );
        node["children"] = json!([{"component": "display-text", "props": {"text": answer}}]);
        vec![component_view("a", "main", node)]
    };
    let Some(mut held) = settled(&speech("1", "first answer")) else {
        return;
    };
    set_held(&mut held, &pane_id("a", "a"), "since", json!(1000));
    // The same window, the same `touched`, a DIFFERENT child text: not a
    // touch -- the child is not compared, which is what keeps a clock that
    // rewrites a child every twenty seconds from touching its window.
    let calls =
        read_pass(&speech("1", "a different child text"), Some(&held), 5000).expect("python3");
    let a = written(&calls, &pane_id("a", "a"));
    assert!(
        a.as_ref()
            .is_none_or(|p| p.get("since").is_none() || p["since"] == 1000),
        "identical props including touched are not a touch: {a:?}"
    );
    // A new answer in the child, and the application says so.
    let calls = read_pass(&speech("2", "second answer"), Some(&held), 5000).expect("python3");
    let a = written(&calls, &pane_id("a", "a")).expect("a is updated");
    assert_eq!(a["since"], 5000, "a changed touched is a touch: {a}");
    assert_eq!(a["touched"], "2", "the hint stands on the window: {a}");
    assert_eq!(
        a["state"], "focus",
        "the touched window is the focus candidate: {a}"
    );
    // A prose view says it in its content, and the wrapper carries it.
    let mut prose = prose_view("p", "main", "Egon", "an answer");
    prose["content"] =
        json!(json!({"title": "Egon", "body": "an answer", "touched": "7"}).to_string());
    let base = bare_screen().expect("python3");
    let calls = read_pass(&[prose], Some(&base), 1000).expect("python3");
    let p = written(&calls, &wrapper("p")).expect("the prose wrapper is added");
    assert_eq!(
        p["touched"], "7",
        "a prose view's touched rides through: {p}"
    );
}
