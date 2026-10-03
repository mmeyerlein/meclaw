//! GH #960 -- one more word an app declaration may say: `shows: {"at": "./<cell>"}`.
//!
//! An app with screen topics answers the presenter: the presenter asks every
//! such app for its topics (`in_show` naming no app) and one app for the data
//! of a topic (`in_show` with `hop.show_app`), and the app answers on
//! `show_topics` / `show_data`. The word draws the member's half of that:
//!
//! - the question INTO the app, off the container `./apps`, for a question
//!   that names no app or names this one;
//! - the answer OUT of the app onto the container, stamped `hop.show_app` with
//!   the app's name and `hop.show_at` with the declared cell -- the builder's
//!   word, written over whatever the app said, so an app cannot answer for
//!   another.
//!
//! Both edges end at the app's RIM, never on the declared cell: an app may seal
//! itself (`ports: []`), and the rim is the one address a sealed hive keeps
//! open. The app routes its rim to `at` and back in its own graph.
//!
//! The presenter's half is drawn when the PRESENTER is installed, known by its
//! instance name (an instance is named after its template): its question onto
//! the container, the answers back from it, and one DEFAULT edge that answers
//! a topics question no app took with an empty `show_topics` -- a presenter
//! alone in its member asks into an empty container, and that must end at the
//! presenter rather than as `hive_no_route`.
//!
//! Pure script tests: the SHIPPED `classify` and `recipes` are run over stdin,
//! nothing is booted (the booted road is
//! `gh960_an_app_with_shows_reaches_the_presenter.rs`).

use meclaw_core::serde_json::{self, Value, json};
use meclaw_testing::{emit_all, emit_one, shipped_script};

const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);
const CLASSIFY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/classify/config.json"
);

const MEMBER: &str = "/os/orgs/acme/members/alex";

const QUESTION: &str = "has(hop.route) && hop.route == 'in_show' && (!has(hop.show_app) || \
                        hop.show_app == '' || hop.show_app == 'probe-app')";
const ANSWERS: &str = "has(hop.route) && (hop.route == 'show_topics' || hop.route == 'show_data')";
const UNTAKEN: &str =
    "has(hop.route) && hop.route == 'in_show' && (!has(hop.show_app) || hop.show_app == '')";

fn run_recipes(params: &Value) -> Vec<Value> {
    emit_all(
        &shipped_script(RECIPES),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "install_app", "request": "…",
                                         "params": params}).to_string()}],
        }),
    )
}

fn edges(params: &Value) -> Vec<Value> {
    let all = run_recipes(params);
    let first = all.first().expect("an emission");
    assert!(first["header"]["error_code"].is_null(), "refused: {first}");
    first["manifest"][0]["diff"]["add_edges"]
        .as_array()
        .expect("edges")
        .clone()
}

fn payload(out: &Value) -> Value {
    serde_json::from_str(out["messages"][0]["text"].as_str().expect("a payload")).expect("json")
}

fn refusal(params: &Value) -> Value {
    let all = run_recipes(params);
    let first = all.first().expect("an emission");
    assert_eq!(
        first["header"]["error_code"],
        json!("app_declaration_invalid"),
        "{first}"
    );
    assert!(first["manifest"].is_null(), "nothing is rendered: {first}");
    payload(first)
}

fn params_for(app: &str, declaration: Value) -> Value {
    json!({"scope": MEMBER, "app": app, "template": format!("{app}@1.0.0"),
           "screen": "display", "generation": "sam", "ctx": {"member_person": "alex"},
           "declaration": declaration})
}

fn params(declaration: Value) -> Value {
    params_for("probe-app", declaration)
}

fn classify(params: Value) -> Value {
    emit_one(
        &shipped_script(CLASSIFY),
        &json!({
            "target": "/os/builder/classify",
            "header": {"hop": {"route": "in_build"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_call", "id": "c1",
                          "text": json!({"request": "install an app",
                                         "recipe": "install_app",
                                         "params": params}).to_string()}],
        }),
    )
}

fn mentions_show(e: &Value) -> bool {
    let s = e.to_string();
    s.contains("in_show") || s.contains("show_topics") || s.contains("show_data")
}

/// Red before GH #960: `shows` is not in the vocabulary, so the whole
/// declaration is refused (`app_declaration_invalid`, field `declaration`).
#[test]
fn shows_draws_the_question_in_and_the_answer_out_at_the_rim() {
    let got = edges(&params(json!({"shows": {"at": "./show"}})));
    assert_eq!(
        got.len(),
        2,
        "the word draws two edges and nothing else: {got:#?}"
    );

    let question = got
        .iter()
        .find(|e| e["to"] == "./apps/probe-app")
        .unwrap_or_else(|| panic!("no question edge into the app: {got:#?}"));
    assert_eq!(question["from"], "./apps", "off the container: {question}");
    assert_eq!(question["condition"], QUESTION);
    assert!(
        question.get("modifier").is_none(),
        "the question travels as the presenter sent it: {question}"
    );

    let answer = got
        .iter()
        .find(|e| e["from"] == "./apps/probe-app")
        .unwrap_or_else(|| panic!("no answer edge out of the app: {got:#?}"));
    assert_eq!(answer["to"], "./apps", "onto the container: {answer}");
    assert_eq!(answer["condition"], ANSWERS);
    assert_eq!(
        answer["modifier"],
        json!({"set_hop": {"show_app": "'probe-app'", "show_at": "'./show'"}}),
        "the builder names the answering app and its cell: {answer}"
    );

    for e in &got {
        assert!(e.get("lane").is_none(), "plain edges at the rim: {e}");
        assert!(e.get("tap").is_none(), "{e}");
        assert!(e.get("default").is_none(), "{e}");
        assert!(
            !e.to_string().contains("/apps/probe-app/"),
            "no edge reaches past the app's rim (a sealed app keeps only its rim): {e}"
        );
    }
}

/// The declared cell is the app's own: `at` is written into the stamp as it
/// was declared, and another cell gives another stamp.
#[test]
fn the_answer_names_the_cell_the_app_declared() {
    let got = edges(&params(json!({"shows": {"at": "./topics"}})));
    let answer = got
        .iter()
        .find(|e| e["from"] == "./apps/probe-app")
        .expect("an answer edge");
    assert_eq!(answer["modifier"]["set_hop"]["show_at"], "'./topics'");
}

/// `shows` needs no generation and no person: a screen topic belongs to the
/// person's screen, not to a generation, and its round lies in the data.
#[test]
fn shows_needs_neither_a_generation_nor_a_person() {
    let mut p = params(json!({"shows": {"at": "./show"}}));
    let o = p.as_object_mut().expect("params");
    o.remove("generation");
    o.remove("ctx");
    let got = edges(&p);
    assert_eq!(got.len(), 2, "{got:#?}");
    assert!(
        got.iter().all(|e| e["modifier"]["set_context"].is_null()),
        "no round is stamped on these edges: {got:#?}"
    );
    let out = classify(p);
    assert!(
        out["header"]["error_code"].is_null(),
        "the switch asks for nothing: {out}"
    );
}

/// An app without `shows` draws no edge of the presenter's road.
#[test]
fn an_app_without_shows_draws_none() {
    let got = edges(&params(json!({
        "listens": ["answer", "mutation_committed"],
        "offers": [{"kind": "sidecar", "at": "./sink", "section": "probe"}],
        "pins": "./sink"})));
    assert!(!got.iter().any(mentions_show), "{got:#?}");
}

/// The presenter is known by its instance name. Installed, it draws its own
/// three edges whatever else it declares: the question onto the container,
/// the answers back off it, and the DEFAULT that answers an untaken topics
/// question with an empty `show_topics` (Red before GH #960: none of them).
#[test]
fn the_presenter_draws_its_question_its_answers_and_the_empty_answer() {
    let got = edges(&params_for(
        "presenter",
        json!({"listens": ["mutation_committed"]}),
    ));
    let shows: Vec<&Value> = got.iter().filter(|e| mentions_show(e)).collect();
    assert_eq!(shows.len(), 3, "{got:#?}");

    let out = shows
        .iter()
        .find(|e| e["from"] == "./apps/presenter")
        .unwrap_or_else(|| panic!("no question edge: {got:#?}"));
    assert_eq!(out["to"], "./apps");
    assert_eq!(out["condition"], "has(hop.route) && hop.route == 'in_show'");
    assert!(out.get("default").is_none(), "{out}");

    let back = shows
        .iter()
        .find(|e| e["to"] == "./apps/presenter" && e.get("default").is_none())
        .unwrap_or_else(|| panic!("no answer edge: {got:#?}"));
    assert_eq!(back["from"], "./apps");
    assert_eq!(back["condition"], ANSWERS);
    assert!(back.get("modifier").is_none(), "{back}");

    let empty = shows
        .iter()
        .find(|e| e["default"] == json!(true))
        .unwrap_or_else(|| panic!("no default edge: {got:#?}"));
    assert_eq!(empty["from"], "./apps");
    assert_eq!(empty["to"], "./apps/presenter");
    assert_eq!(empty["condition"], UNTAKEN);
    assert_eq!(
        empty["modifier"],
        json!({"set_hop": {"route": "'show_topics'", "show_app": "''", "show_at": "''"}}),
        "the untaken question comes back as an answer of nobody: {empty}"
    );

    // Its other words are drawn as for any app.
    assert!(
        got.iter().any(|e| e["from"] == "./apps"
            && e["to"] == "./apps/presenter"
            && e["condition"] == "has(hop.route) && hop.route == 'mutation_committed'"),
        "{got:#?}"
    );
}

/// Any other app name draws none of the presenter's edges.
#[test]
fn only_the_presenter_draws_the_presenter_edges() {
    let got = edges(&params_for(
        "presenter-two",
        json!({"listens": ["mutation_committed"]}),
    ));
    assert!(!got.iter().any(mentions_show), "{got:#?}");
}

/// The presenter shows nothing of its own: its question would come back to
/// itself.
#[test]
fn the_presenter_cannot_declare_shows() {
    let p = refusal(&params_for("presenter", json!({"shows": {"at": "./show"}})));
    assert_eq!(p["field"], "shows", "{p}");
}

/// Red before GH #960: every bad value was refused as an unknown KEY (field
/// `declaration`), never by the field itself.
#[test]
fn shows_is_checked_by_field() {
    for (bad, field) in [
        (json!("./show"), "shows"),
        (json!(["./show"]), "shows"),
        (json!({}), "shows.at"),
        (json!({"at": "show"}), "shows.at"),
        (json!({"at": "./"}), "shows.at"),
        (json!({"at": "./../x"}), "shows.at"),
        (json!({"at": 3}), "shows.at"),
        // The cell is quoted into a CEL literal on the answer edge.
        (json!({"at": "./x'||true"}), "shows.at"),
        (json!({"at": "./Show"}), "shows.at"),
        (json!({"at": "./show", "topics": []}), "shows"),
        (json!({"at": "./show", "threshold": 0.5}), "shows"),
    ] {
        let p = refusal(&params(json!({"shows": bad.clone()})));
        assert_eq!(p["field"], field, "{bad}: {p}");
        assert!(
            p["reason"].as_str().is_some_and(|r| !r.is_empty()),
            "{bad}: {p}"
        );
    }
    let p = refusal(&params(json!({"shows": {"at": "./show", "topics": []}})));
    assert_eq!(
        p["known"],
        json!(["at"]),
        "the refusal names what IS known: {p}"
    );
}

/// Red before review I-1 of GH #960: `drives[].cell` was only checked for "not
/// absolute, no `..`", so `apps` passed. A drive on the container draws
/// `./apps -> ./apps/<app>` for every `back` lane and `./apps/<app> -> ./apps`
/// for every `out` lane -- an app could hear every other app's `show_data`
/// (rows meant for the presenter only) and forge an `in_show` with any
/// `show_app`. A device stands BESIDE the container, never on it or in it.
#[test]
fn a_drive_cannot_name_the_container_or_an_app() {
    for cell in [
        "apps",
        "./apps",
        "apps/",
        "apps/presenter",
        "./apps/presenter",
        "apps/x",
        "./apps/x/y",
    ] {
        let p = refusal(&params(json!({
            "drives": [{"cell": cell, "out": [], "back": ["show_data", "in_show"]}]
        })));
        assert_eq!(p["field"], "drives[0].cell", "{cell}: {p}");
        assert!(
            p["reason"].as_str().is_some_and(|r| !r.is_empty()),
            "{cell}: {p}"
        );
    }
    // A device beside the container stays allowed, also one whose name only
    // starts with the same letters.
    for cell in ["lamp", "./lamp", "devices/lamp", "apps2", "./appsx/y"] {
        let drawn = edges(&params(json!({
            "drives": [{"cell": cell, "out": ["switch"], "back": ["state"]}]
        })));
        assert!(!drawn.is_empty(), "{cell} is a device beside the container");
    }
}

/// Red before GH #960: the `known` lists of renderer and switch do not carry
/// the word.
#[test]
fn the_vocabulary_knows_shows() {
    let p = refusal(&params(json!({"showz": {"at": "./show"}})));
    assert_eq!(p["field"], "declaration");
    assert!(
        p["known"]
            .as_array()
            .expect("known")
            .contains(&json!("shows")),
        "{p}"
    );

    let mut q = params(json!({}));
    q["declaration"] = json!("shows: ./show");
    let out = classify(q);
    assert_eq!(
        out["header"]["error_code"],
        json!("app_declaration_invalid"),
        "{out}"
    );
    assert!(
        payload(&out)["known"]
            .as_array()
            .expect("known")
            .contains(&json!("shows")),
        "{out}"
    );
}
