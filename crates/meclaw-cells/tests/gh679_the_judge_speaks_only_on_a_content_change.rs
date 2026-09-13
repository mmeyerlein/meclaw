//! GH #679 -- a minimal judge re-judges the whole screen on a content change
//! and never on a tick.
//!
//! Beside the compose cell stands `judge`, an `llm` cell with one prompt and
//! one JSON schema: the bar (`focus`), a weight per context, and per window an
//! optional `hidden` or `relevance`. The compose cell asks it after a pass
//! that touched at least one window -- never after a tick, never after a
//! write that changed nothing, and not twice inside `judge_min_interval_ms`
//! -- and only when the knob `judge` is `on`. The verdict comes back on the
//! lane `in_verdict` as a pass without a write that sets the root's `focus`,
//! `weights` and `judged_at` and per window `judged_hidden` /
//! `judged_relevance`; a verdict that is late, an error, or not JSON changes
//! nothing, because the floor has already drawn.
//!
//! The script runs as a subprocess the way a `code` cell runs it; no provider
//! is spoken to. Skips when `python3` is absent or the templates do not ship
//! (R2b).

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

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

fn read_json(path: &std::path::Path) -> Value {
    meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    )
    .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn pane_id(view_id: &str, pane: &str) -> String {
    format!("view.alex.{view_id}/c.{pane}")
}

fn pane(pane: &str, props: Value) -> Value {
    let mut props = props;
    props["pane_id"] = json!(pane);
    json!({"component": "display-pane", "props": props, "key": format!("c.{pane}")})
}

fn component_view(view_id: &str, region: &str, tree: Value) -> Value {
    json!({
        "owner": "alex", "view_id": view_id, "region": region, "ord": 0,
        "kind": "component", "content": tree.to_string(), "components": "[]",
        "ttl_ms": 0, "updated_at": 1,
    })
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

fn on_route<'a>(emissions: &'a [Value], route: &str) -> Vec<&'a Value> {
    emissions
        .iter()
        .filter(|e| e["header"]["route"] == route)
        .collect()
}

fn patch_calls(emissions: &[Value]) -> Vec<Value> {
    on_route(emissions, "patch")
        .first()
        .map(|e| calls_of(e))
        .unwrap_or_default()
}

/// A read pass with the judge `on`, over a plan that may carry a verdict.
fn read_pass(
    views: &[Value],
    objects: Option<&Value>,
    now: u64,
    judge: &str,
    verdict: Option<Value>,
) -> Option<Vec<Value>> {
    let messages = match objects {
        None => json!([]),
        Some(objs) => json!([{
            "origin": "tool", "type": "tool_result", "id": "d-query",
            "text": json!({"objects": objs}).to_string(),
        }]),
    };
    let mut plan = json!({"views": views, "define": [], "now": now});
    if let Some(v) = verdict {
        plan["verdict"] = v;
    }
    run(&json!({
        "params": {"judge": judge},
        "body": {"messages": messages},
        "envelope": {"header": {
            "hop": {"operation": "query"},
            "context": {"display_origin": "read", "display_views": plan.to_string()},
        }},
    }))
}

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

fn prop_of<'a>(held: &'a Value, id: &str, key: &str) -> &'a Value {
    &held
        .as_array()
        .expect("list")
        .iter()
        .find(|o| o["id"] == id)
        .unwrap_or_else(|| panic!("{id} is held"))["props"][key]
}

fn a_view() -> Vec<Value> {
    vec![component_view(
        "a",
        "main",
        pane(
            "a",
            json!({"context": "weather", "relevance": 0.7, "title": "Sunny"}),
        ),
    )]
}

/// A screen holding `a` (weather, 0.7), settled and in focus, touched at 1000.
fn settled_screen() -> Option<(Vec<Value>, Value)> {
    let views = a_view();
    let mut held = json!([]);
    let boot = read_pass(&[], None, 1000, "on", None)?;
    apply(&mut held, &patch_calls(&boot));
    let arrive = read_pass(&views, Some(&held), 1000, "on", None)?;
    apply(&mut held, &patch_calls(&arrive));
    let settle = read_pass(&views, Some(&held), 1100, "on", None)?;
    apply(&mut held, &patch_calls(&settle));
    assert_eq!(prop_of(&held, &pane_id("a", "a"), "state"), "focus");
    Some((views, held))
}

/// A pass that touches a window asks the judge exactly once, with the
/// guideline as instructions and the situation as one JSON user turn that
/// names the touched window; the same pass with the knob `off` asks nothing.
#[test]
fn a_content_change_asks_the_judge_once() {
    if !library_ships() {
        return;
    }
    let Some(boot) = read_pass(&[], None, 1000, "on", None) else {
        return;
    };
    assert!(
        on_route(&boot, "judge").is_empty(),
        "a bare screen touches nothing: {boot:?}"
    );
    let mut held = json!([]);
    apply(&mut held, &patch_calls(&boot));
    let arrive = read_pass(&a_view(), Some(&held), 2000, "on", None).expect("python3");
    let asked = on_route(&arrive, "judge");
    assert_eq!(asked.len(), 1, "exactly one question: {arrive:?}");
    let q = asked[0];
    let instructions = q["system"]["instructions"]["text"]
        .as_str()
        .expect("the guideline rides as instructions");
    assert!(
        instructions.contains("as little as possible"),
        "{instructions}"
    );
    assert!(
        instructions.contains("\"focus\""),
        "the schema is in the prompt"
    );
    let turns = q["messages"].as_array().expect("messages");
    assert_eq!(turns.len(), 1, "one user turn");
    assert_eq!(turns[0]["origin"], "user");
    let situation: Value =
        meclaw_core::serde_json::from_str(turns[0]["text"].as_str().expect("text"))
            .expect("the situation is JSON");
    let windows = situation["windows"].as_array().expect("windows[]");
    let a = windows
        .iter()
        .find(|w| w["id"] == pane_id("a", "a"))
        .expect("the touched window is in the situation");
    assert_eq!(a["touched"], true);
    assert_eq!(a["context"], "weather");
    assert_eq!(a["region"], "main");
    assert!(a["text"].as_str().is_some_and(|t| t.contains("Sunny")));
    assert_eq!(situation["now"], 2000);
    assert!(situation["preferences"].is_array());

    let quiet = read_pass(&a_view(), Some(&held), 2000, "off", None).expect("python3");
    assert!(
        on_route(&quiet, "judge").is_empty(),
        "the knob is off: {quiet:?}"
    );
}

/// Nothing touched, nothing asked: a write with identical props, a tick, and
/// a touch inside `judge_min_interval_ms` of the last verdict.
#[test]
fn a_tick_and_an_unchanged_write_ask_nothing() {
    if !library_ships() {
        return;
    }
    let Some((views, mut held)) = settled_screen() else {
        return;
    };
    let same = read_pass(&views, Some(&held), 5000, "on", None).expect("python3");
    assert!(
        on_route(&same, "judge").is_empty(),
        "identical props are no touch: {same:?}"
    );
    // A tick is a pass over the same table with nothing touched.
    let tick = read_pass(&views, Some(&held), 9000, "on", None).expect("python3");
    assert!(on_route(&tick, "judge").is_empty(), "a tick: {tick:?}");

    // The judge spoke at 10000; a touch at 11000 is inside the interval.
    let verdict = json!({"focus": 0.3, "weights": {"weather": 1.0}, "windows": []});
    let judged = read_pass(&views, Some(&held), 10000, "on", Some(verdict)).expect("python3");
    assert!(
        on_route(&judged, "judge").is_empty(),
        "a verdict pass never asks again: {judged:?}"
    );
    apply(&mut held, &patch_calls(&judged));
    assert_eq!(prop_of(&held, ROOT, "judged_at"), &json!(10000));
    let mut touched = views.clone();
    touched[0] = component_view(
        "a",
        "main",
        pane(
            "a",
            json!({"context": "weather", "relevance": 0.7, "title": "Rain"}),
        ),
    );
    let soon = read_pass(&touched, Some(&held), 11000, "on", None).expect("python3");
    assert!(
        on_route(&soon, "judge").is_empty(),
        "inside judge_min_interval_ms the floor judges alone: {soon:?}"
    );
    let later = read_pass(&touched, Some(&held), 14000, "on", None).expect("python3");
    assert_eq!(on_route(&later, "judge").len(), 1, "{later:?}");
}

/// The judge's answer on `in_verdict` is a pass without a write carrying the
/// verdict -- fenced JSON tolerated -- and the read pass that follows writes
/// the bar, the weights, `judged_at`, and hides the window the verdict named.
/// An error and a non-JSON answer change nothing.
#[test]
fn a_verdict_sets_the_bar_and_the_weights_and_the_floor_reads_them() {
    if !library_ships() {
        return;
    }
    let Some((views, mut held)) = settled_screen() else {
        return;
    };
    let text = format!(
        "```json\n{}\n```",
        json!({"focus": 0.8, "weights": {"weather": 0},
               "windows": [{"id": pane_id("a", "a"), "hidden": true}]})
    );
    let answer = run(&json!({
        "params": {"judge": "on"},
        "body": {"messages": [{"origin": "assistant", "type": "text", "text": text}]},
        "envelope": {"header": {
            "hop": {"route": "in_verdict", "finish_reason": "stop", "model": "x"},
            // The context of the read pass that asked travels with the reply.
            "context": {"display_origin": "read", "display_views": "{}"},
        }},
    }))
    .expect("python3");
    assert_eq!(answer.len(), 1, "{answer:?}");
    assert_eq!(answer[0]["header"]["route"], "views");
    let legs = calls_of(&answer[0]);
    assert_eq!(legs.len(), 1, "a select and nothing else: {legs:?}");
    let request: Value = meclaw_core::serde_json::from_str(
        answer[0]["header"]["display_request"]
            .as_str()
            .expect("request"),
    )
    .expect("json");
    assert_eq!(request["tick"], true);
    assert_eq!(request["verdict"]["focus"], 0.8);

    // Pass 2 on a tick that carries a verdict hands it on in the plan.
    let after = run(&json!({
        "params": {"judge": "on"},
        "body": {"messages": [{
            "origin": "tool", "type": "tool_result", "id": "d-select",
            "text": Value::Array(views.clone()).to_string(),
        }]},
        "envelope": {"header": {
            "hop": {},
            "context": {"display_origin": "views", "display_request": request.to_string()},
        }},
    }))
    .expect("python3");
    let plan: Value = meclaw_core::serde_json::from_str(
        after[0]["header"]["display_views"].as_str().expect("plan"),
    )
    .expect("json");
    assert_eq!(plan["verdict"], request["verdict"]);

    // Pass 3 applies it.
    let read = read_pass(
        &views,
        Some(&held),
        7000,
        "on",
        Some(plan["verdict"].clone()),
    )
    .expect("python3");
    apply(&mut held, &patch_calls(&read));
    assert_eq!(prop_of(&held, ROOT, "focus"), &json!(0.8));
    let weights: Value = meclaw_core::serde_json::from_str(
        prop_of(&held, ROOT, "weights")
            .as_str()
            .expect("weights is text"),
    )
    .expect("json");
    assert_eq!(weights["weather"], 0.0);
    assert_eq!(prop_of(&held, ROOT, "judged_at"), &json!(7000));
    assert_eq!(prop_of(&held, &pane_id("a", "a"), "judged_hidden"), true);
    assert_eq!(prop_of(&held, &pane_id("a", "a"), "state"), "hidden");
    assert_eq!(prop_of(&held, &pane_id("a", "a"), "score"), 0.0);

    for (hop, body) in [
        (
            json!({"route": "in_verdict", "finish_reason": "error", "error_code": "timeout"}),
            json!({"messages": [], "meta": {"detail": "no model"}}),
        ),
        (
            json!({"route": "in_verdict", "finish_reason": "stop"}),
            json!({"messages": [{"origin": "assistant", "type": "text", "text": "I think so"}]}),
        ),
    ] {
        let nothing = run(&json!({
            "params": {"judge": "on"},
            "body": body,
            "envelope": {"header": {"hop": hop}},
        }))
        .expect("python3");
        assert!(nothing.is_empty(), "the floor stands: {nothing:?}");
    }
}

/// The hive carries the judge: an `llm` cell with an empty model as shipped,
/// a JSON answer asked for, a small budget, and two edges word for word.
#[test]
fn the_hive_wires_the_judge() {
    if !library_ships() {
        return;
    }
    let judge = read_json(&repo("templates/display/judge/config.json"));
    assert_eq!(judge["cell"]["type"], "llm");
    assert_eq!(judge["params"]["model"], "${DISPLAY_JUDGE_MODEL:-}");
    assert_eq!(judge["contract"]["settings"]["model"]["default"], "");
    assert_eq!(judge["contract"]["settings"]["api_key"]["secret"], true);
    assert_eq!(
        judge["params"]["provider_extra"]["response_format"]["type"],
        "json_object"
    );
    assert!(
        judge["params"]["max_tokens"]
            .as_u64()
            .is_some_and(|n| n <= 1000)
    );
    // Measured on a throw-away colony: with 4 s two verdicts in nine timed
    // out against a provider answering in 2-4 s (OR-C-Bau-9).
    assert_eq!(judge["params"]["external_timeout_ms"], 8000);
    assert!(
        judge["cell"]["message_timeout"]
            .as_u64()
            .is_some_and(|b| b > 8000 * 2),
        "the backstop stays well above the operation timeout"
    );
    assert_eq!(judge["params"]["system_order"], json!(["instructions"]));

    let hive = read_json(&repo("templates/display/config.json"));
    let edges = hive["params"]["graph"]["edges"].as_array().expect("edges");
    for wanted in [
        json!({"from": "./compose", "to": "./judge",
               "condition": "has(hop.route) && hop.route == 'judge'"}),
        json!({"from": "./judge", "to": "./compose",
               "condition": "has(hop.finish_reason)",
               "modifier": {"set_hop": {"route": "'in_verdict'"}}}),
    ] {
        assert!(edges.contains(&wanted), "missing edge {wanted}");
    }
    assert_eq!(hive["params"]["ports"], json!([]));
}

/// The README says what the judge is and when it speaks, and the code keeps it.
#[test]
fn the_readme_names_the_judge() {
    if !library_ships() {
        return;
    }
    let readme = std::fs::read_to_string(repo("templates/display/README.md")).expect("README");
    assert!(readme.contains("re-judges the whole screen"), "the section");
    assert!(readme.contains("never on a tick"), "the rule");
    assert!(readme.contains("`judge_min_interval_ms`"), "the knob");
    assert!(readme.contains("`in_verdict`"), "the lane");
}

/// A verdict rules for `linger_ms + fade_ms` from `judged_at` and no longer:
/// once the situation it judged has faded, the floor judges again -- so a
/// judge that fell silent after a verdict never leaves the screen empty
/// (OR-C-Bau-7). The bar of 0.8 hides a new conversation window at 0.7 while
/// the verdict stands; after it expires the floor's 0.3 shows it.
#[test]
fn a_verdict_expires_and_the_floor_judges_again() {
    if !library_ships() {
        return;
    }
    let Some((views, mut held)) = settled_screen() else {
        return;
    };
    let verdict = json!({"focus": 0.8, "weights": {"weather": 0.0}, "windows": []});
    let judged = read_pass(&views, Some(&held), 3000, "on", Some(verdict)).expect("python3");
    apply(&mut held, &patch_calls(&judged));
    assert_eq!(prop_of(&held, ROOT, "focus"), &json!(0.8));

    let mut two = views.clone();
    two.push(component_view(
        "c",
        "main",
        pane(
            "c",
            json!({"context": "conversation", "relevance": 0.7, "title": "Hi"}),
        ),
    ));
    // While the verdict stands: 1.0 x 0.7 = 0.7 < 0.8, hidden.
    let soon = read_pass(&two, Some(&held), 12000, "on", None).expect("python3");
    let c = patch_calls(&soon)
        .into_iter()
        .find(|call| call["id"] == pane_id("c", "c"))
        .expect("c is created");
    assert_eq!(c["props"]["state"], "hidden", "{c}");
    // 150 s later the verdict has expired: the floor sets 0.3 and shows it.
    let late = read_pass(&two, Some(&held), 150000, "on", None).expect("python3");
    let calls = patch_calls(&late);
    let root = calls
        .iter()
        .find(|call| call["id"] == ROOT)
        .expect("the root is updated");
    assert_eq!(root["props"]["focus"], 0.3, "{root}");
    let c = calls
        .iter()
        .find(|call| call["id"] == pane_id("c", "c"))
        .expect("c is created");
    assert_ne!(c["props"]["state"], "hidden", "{c}");
    assert_eq!(c["props"]["score"], 0.7);
}

/// The brake counts from the QUESTION, not from the answer: two touches 500 ms
/// apart with no verdict in between ask once, and the root remembers when it
/// asked (`asked_at`).
#[test]
fn two_touches_inside_the_interval_ask_once() {
    if !library_ships() {
        return;
    }
    let Some((views, mut held)) = settled_screen() else {
        return;
    };
    let mut touched = views.clone();
    touched[0] = component_view(
        "a",
        "main",
        pane(
            "a",
            json!({"context": "weather", "relevance": 0.7, "title": "Rain"}),
        ),
    );
    let first = read_pass(&touched, Some(&held), 20000, "on", None).expect("python3");
    assert_eq!(on_route(&first, "judge").len(), 1, "{first:?}");
    let root = patch_calls(&first)
        .into_iter()
        .find(|call| call["id"] == ROOT)
        .expect("the root is updated");
    assert_eq!(root["props"]["asked_at"], 20000);
    apply(&mut held, &patch_calls(&first));

    touched[0] = component_view(
        "a",
        "main",
        pane(
            "a",
            json!({"context": "weather", "relevance": 0.7, "title": "Hail"}),
        ),
    );
    let second = read_pass(&touched, Some(&held), 20500, "on", None).expect("python3");
    assert!(
        on_route(&second, "judge").is_empty(),
        "inside the interval since the question: {second:?}"
    );
    let third = read_pass(&touched, Some(&held), 23500, "on", None).expect("python3");
    assert_eq!(on_route(&third, "judge").len(), 1, "{third:?}");
}

/// What the judge reads is the guideline the README states, word for word,
/// and every window comes with its owner and view id in the clear.
#[test]
fn the_judge_reads_the_guideline_and_the_owners() {
    if !library_ships() {
        return;
    }
    let Some(boot) = read_pass(&[], None, 1000, "on", None) else {
        return;
    };
    let mut held = json!([]);
    apply(&mut held, &patch_calls(&boot));
    let arrive = read_pass(&a_view(), Some(&held), 2000, "on", None).expect("python3");
    let q = on_route(&arrive, "judge")[0];
    let instructions = q["system"]["instructions"]["text"].as_str().expect("text");
    let readme = std::fs::read_to_string(repo("templates/display/README.md")).expect("README");
    for sentence in [
        "Focus is the state of the whole screen, not the highlighting of one active element.",
        "The screen has no agenda of its own.",
        "Priority is dynamic.",
        "show as little as possible at every moment -- and everything that truly matters at that moment.",
    ] {
        assert!(
            instructions.contains(sentence),
            "the guideline, word for word: {sentence}"
        );
        assert!(
            readme
                .replace("**", "")
                .replace('*', "")
                .replace('\n', " ")
                .contains(sentence),
            "the README states it: {sentence}"
        );
    }
    let situation: Value =
        meclaw_core::serde_json::from_str(q["messages"][0]["text"].as_str().expect("text"))
            .expect("json");
    let a = &situation["windows"][0];
    assert_eq!(a["owner"], "alex");
    assert_eq!(a["view_id"], "a");
    assert_eq!(a["since"], 2000);
}

/// A verdict's `hidden` and `relevance` on a window fade with the verdict
/// (OR-C-Bau-11): a pinned clock the judge hid stays hidden while the verdict
/// stands and comes back by itself once `linger_ms + fade_ms` have passed
/// since `judged_at` -- without a touch, without a next verdict.
#[test]
fn a_verdicts_hidden_fades_with_the_verdict() {
    if !library_ships() {
        return;
    }
    let views = vec![component_view(
        "clock",
        "aside",
        pane(
            "c",
            json!({"context": "ambient", "relevance": 0.4, "pinned": true, "title": "12:00"}),
        ),
    )];
    let mut held = json!([]);
    let Some(boot) = read_pass(&[], None, 1000, "on", None) else {
        return;
    };
    apply(&mut held, &patch_calls(&boot));
    let arrive = read_pass(&views, Some(&held), 1000, "on", None).expect("python3");
    apply(&mut held, &patch_calls(&arrive));
    let settle = read_pass(&views, Some(&held), 1100, "on", None).expect("python3");
    apply(&mut held, &patch_calls(&settle));
    let id = pane_id("clock", "c");
    assert_ne!(
        prop_of(&held, &id, "state"),
        "hidden",
        "visible before the verdict"
    );

    let verdict = json!({"focus": 0.3, "weights": {"ambient": 1.0},
                         "windows": [{"id": id, "hidden": true}]});
    let judged = read_pass(&views, Some(&held), 3000, "on", Some(verdict)).expect("python3");
    apply(&mut held, &patch_calls(&judged));
    assert_eq!(prop_of(&held, &id, "judged_hidden"), true);
    assert_eq!(prop_of(&held, &id, "state"), "hidden");

    // While the verdict stands (140 s from 3000): still hidden, untouched.
    let soon = read_pass(&views, Some(&held), 100_000, "on", None).expect("python3");
    apply(&mut held, &patch_calls(&soon));
    assert_eq!(prop_of(&held, &id, "state"), "hidden");
    assert_eq!(prop_of(&held, &id, "judged_hidden"), true);

    // The verdict has faded: the judged props go with it and the clock stands.
    let late = read_pass(&views, Some(&held), 150_000, "on", None).expect("python3");
    let calls = patch_calls(&late);
    let props = calls
        .iter()
        .find(|c| c["op"] == "object.update" && c["id"] == id)
        .expect("the clock is updated")["props"]
        .clone();
    assert_eq!(props["judged_hidden"], false, "{props}");
    assert_eq!(props["judged_relevance"], "", "{props}");
    assert_ne!(props["state"], "hidden", "{props}");
    assert_eq!(props["score"], 0.4, "pinned: no decay, its own relevance");
    apply(&mut held, &calls);
    assert_ne!(prop_of(&held, &id, "state"), "hidden");
}
