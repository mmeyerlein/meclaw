//! GH #949 -- one more word an app declaration may say: `candidates: "./<cell>"`.
//!
//! `candidates` is the push-candidate twin of `pins` (GH #916): ONE edge from
//! the declared cell of the app onto the generation, `candidate` restamped
//! `in_candidate`, which the generation hands to the curator of each of its
//! brains. It names a cell inside the app, it needs the generation the app is
//! installed for, and an app that declares none gets no edge -- a `candidate`
//! it emits stays unrouted. The edge stamps the member's round (review I-1),
//! so the wish names the person (`ctx.member_person`).
//!
//! Pure script tests: the SHIPPED `classify` and `recipes` are run over stdin,
//! nothing is booted. Same technique and same assertions as
//! `gh916_an_app_declares_close_and_pins.rs`, for the new word.

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
/// The round the edge stamps, as a CEL literal: the generation `sam` is the
/// agent, `ctx.member_person` the person (GH #949, review C-1/I-1).
const MEMBER_ROUND: &str = r#"'["agent:sam","member:alex"]'"#;

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
    payload(first)
}

fn params(declaration: Value) -> Value {
    json!({"scope": MEMBER, "app": "probe-app", "template": "probe-app@1.0.0",
           "screen": "display", "generation": "sam", "ctx": {"member_person": "alex"},
           "declaration": declaration})
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

/// The edges whose source is the declared cell.
fn from_cell<'a>(got: &'a [Value], cell: &str) -> Vec<&'a Value> {
    got.iter().filter(|e| e["from"] == json!(cell)).collect()
}

/// Red before GH #949: `candidates` is not in the vocabulary, so the whole
/// declaration is refused (`app_declaration_invalid`, field `declaration`) and
/// `edges()` fails on the refusal.
#[test]
fn candidates_draws_one_edge_from_the_cell_onto_the_generation() {
    let got = edges(&params(json!({"candidates": "./sink"})));
    let candidate = from_cell(&got, "./apps/probe-app/sink");
    assert_eq!(candidate.len(), 1, "{got:#?}");
    assert_eq!(candidate[0]["to"], "./assistants/sam");
    assert_eq!(
        candidate[0]["condition"],
        "has(hop.route) && hop.route == 'candidate'"
    );
    assert_eq!(
        candidate[0]["modifier"]["set_hop"]["route"],
        "'in_candidate'"
    );
    assert_eq!(
        candidate[0]["modifier"]["set_context"],
        json!({"audience_set": MEMBER_ROUND}),
        "the edge stamps the member's round and nothing else"
    );
    assert!(
        candidate[0].get("lane").is_none(),
        "a plain edge onto the generation's rim"
    );
    assert!(candidate[0].get("default").is_none(), "{:#?}", candidate[0]);
    assert!(candidate[0].get("tap").is_none(), "{:#?}", candidate[0]);
    // Nothing else: the word draws its one edge and no observer, no binding.
    assert_eq!(got.len(), 1, "{got:#?}");
}

/// `pins` and `candidates` are two words with two lanes. Declared on the SAME
/// cell they draw two edges, each restamping its own route, and neither takes
/// the other's lane.
#[test]
fn pins_and_candidates_on_one_cell_are_two_edges() {
    let got = edges(&params(json!({"pins": "./sink", "candidates": "./sink"})));
    let out = from_cell(&got, "./apps/probe-app/sink");
    assert_eq!(out.len(), 2, "{got:#?}");
    let routes: Vec<(&str, &str)> = out
        .iter()
        .map(|e| {
            (
                e["condition"].as_str().expect("a condition"),
                e["modifier"]["set_hop"]["route"]
                    .as_str()
                    .expect("a restamp"),
            )
        })
        .collect();
    assert!(
        routes.contains(&("has(hop.route) && hop.route == 'pin'", "'in_pin'")),
        "{routes:?}"
    );
    assert!(
        routes.contains(&(
            "has(hop.route) && hop.route == 'candidate'",
            "'in_candidate'"
        )),
        "{routes:?}"
    );
    assert!(
        out.iter().all(|e| e["to"] == "./assistants/sam"),
        "{out:#?}"
    );
}

/// The candidates cell may be another cell than the pins cell; each word
/// starts at the cell IT names.
#[test]
fn candidates_start_at_the_cell_they_name() {
    let got = edges(&params(
        json!({"pins": "./pinner", "candidates": "./pusher"}),
    ));
    let pinner = from_cell(&got, "./apps/probe-app/pinner");
    let pusher = from_cell(&got, "./apps/probe-app/pusher");
    assert_eq!(pinner.len(), 1, "{got:#?}");
    assert_eq!(pusher.len(), 1, "{got:#?}");
    assert_eq!(pinner[0]["modifier"]["set_hop"]["route"], "'in_pin'");
    assert_eq!(pusher[0]["modifier"]["set_hop"]["route"], "'in_candidate'");
}

#[test]
fn an_app_without_candidates_draws_none() {
    let got = edges(&params(json!({
        "offers": [{"kind": "sidecar", "at": "./sink", "section": "probe"}],
        "listens": ["answer"],
        "pins": "./sink"})));
    assert!(
        !got.iter().any(|e| {
            let s = e.to_string();
            s.contains("in_candidate") || s.contains("'candidate'")
        }),
        "{got:#?}"
    );
}

/// Red before GH #949: the `known` list of the refusal does not carry the word.
#[test]
fn the_vocabulary_knows_candidates() {
    let p = refusal(&params(json!({"candidatez": "./sink"})));
    assert_eq!(p["field"], "declaration");
    assert!(
        p["known"]
            .as_array()
            .expect("known")
            .contains(&json!("candidates")),
        "{p}"
    );
}

/// Red before GH #949: every bad value was refused as an unknown KEY (field
/// `declaration`), never by the field itself.
#[test]
fn candidates_names_a_cell_inside_the_app() {
    for bad in [
        json!("sink"),
        json!("./"),
        json!("./../x"),
        json!(3),
        json!(["./sink"]),
        json!({"at": "./sink"}),
    ] {
        let p = refusal(&params(json!({"candidates": bad.clone()})));
        assert_eq!(p["field"], "candidates", "{bad}: {p}");
    }
}

/// The generation is needed at the switch (classify) and at the renderer.
/// Red before GH #949: classify routed a `candidates`-only declaration to the
/// recipe without a generation.
#[test]
fn candidates_needs_the_generation_at_the_switch() {
    let mut p = params(json!({"candidates": "./sink"}));
    p.as_object_mut().expect("params").remove("generation");
    let out = classify(p.clone());
    assert_eq!(
        out["header"]["error_code"],
        json!("recipe_params_incomplete"),
        "{out}"
    );
    assert_eq!(payload(&out)["missing"], json!(["generation"]), "{out}");

    // The renderer refuses it as well -- a cell knows no topology and must not
    // build on who stands in front of it.
    let all = run_recipes(&p);
    let first = all.first().expect("an emission");
    assert_eq!(
        first["header"]["error_code"],
        json!("recipe_params_incomplete"),
        "{first}"
    );
    assert!(first["manifest"].is_null(), "{first}");
}

/// The switch's own refusal of a declaration that is not an object names the
/// vocabulary too, and it has to carry the new word.
#[test]
fn the_switch_names_candidates_among_the_known_words() {
    let mut p = params(json!({}));
    p["declaration"] = json!("candidates: ./sink");
    let out = classify(p);
    assert_eq!(
        out["header"]["error_code"],
        json!("app_declaration_invalid"),
        "{out}"
    );
    assert!(
        payload(&out)["known"]
            .as_array()
            .expect("known")
            .contains(&json!("candidates")),
        "{out}"
    );
}

/// GH #949 (review I-1) -- a candidate leaves in its MEMBER's round. The round an
/// app's message carries is written by the app's own template, so the edge
/// the builder draws stamps `["agent:<generation>","member:<person>"]` over
/// it; a wish with `candidates` that names no person renders nothing and is
/// asked, at the renderer and at the switch, with one question.
///
/// Red before the fix: the edge stamps no round and the wish renders.
#[test]
fn a_candidate_leaves_in_the_members_round_and_the_wish_names_the_person() {
    let got = edges(&params(json!({"candidates": "./sink"})));
    let out = from_cell(&got, "./apps/probe-app/sink");
    assert_eq!(out.len(), 1, "{got:#?}");
    assert_eq!(
        out[0]["modifier"]["set_context"]["audience_set"],
        json!(MEMBER_ROUND),
        "the edge stamps the member's round: {got:#?}"
    );

    let mut p = params(json!({"candidates": "./sink"}));
    p.as_object_mut().expect("params").remove("ctx");
    let all = run_recipes(&p);
    let first = all.first().expect("an emission");
    assert_eq!(
        first["header"]["error_code"],
        json!("wish_incomplete"),
        "{first}"
    );
    assert!(first["manifest"].is_null(), "nothing is rendered: {first}");
    let asked = payload(first);
    assert_eq!(asked["missing"], json!(["ctx.member_person"]), "{asked}");
    let out = classify(p);
    assert_eq!(
        out["header"]["error_code"],
        json!("wish_incomplete"),
        "{out}"
    );
    assert_eq!(
        payload(&out),
        asked,
        "one question at the switch and the renderer"
    );
}

/// GH #949 (review I-R1) -- the person is written INTO the round literal, a JSON
/// string inside a CEL string, so it is checked before it is: `x","*` closed the
/// JSON string without breaking the CEL one and stamped
/// `["agent:sam","member:x","*"]`, a round that covers every row. A person with
/// a quote, a backslash, a dollar sign, `*`, a comma or a control character is
/// no identity: the wish is asked again, with the same question at the renderer
/// and at the switch, and nothing is rendered -- no edge, no round.
///
/// Red before the fix: the wish rendered and the edge stamped `*`.
#[test]
fn a_person_that_would_break_the_round_is_asked_again() {
    for person in [
        r#"x","*"#,
        r#"x","member:y"#,
        "x'",
        "x\\",
        "x$",
        "*",
        "x,y",
        "x\ny",
        "x\u{2028}y",
        " ",
    ] {
        let mut p = params(json!({"candidates": "./sink"}));
        p["ctx"] = json!({"member_person": person});
        let all = run_recipes(&p);
        assert!(
            all.iter().all(|e| e["manifest"].is_null()),
            "{person:?}: something was rendered: {all:?}"
        );
        let first = all.first().expect("an emission");
        assert_eq!(
            first["header"]["error_code"],
            json!("wish_incomplete"),
            "{person:?}: {first}"
        );
        let asked = payload(first);
        assert_eq!(asked["missing"], json!(["ctx.member_person"]), "{asked}");
        let out = classify(p);
        assert_eq!(
            out["header"]["error_code"],
            json!("wish_incomplete"),
            "{person:?}: {out}"
        );
        assert_eq!(
            payload(&out),
            asked,
            "{person:?}: one question at both cells"
        );
    }
}
