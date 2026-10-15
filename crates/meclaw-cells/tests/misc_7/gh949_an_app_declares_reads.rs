//! GH #949 -- one more word an app declaration may say: `reads: "./<cell>"`.
//!
//! An app that runs a round of its own (a nightly pass, a digest) had no way
//! into the curator's ledger: the model's `history_*` tools are the model's,
//! and the memory hive's recall is the only pull an app had. `reads` opens one
//! lane, audience-gated at the curator (`./reader`, locked in
//! `gh949_an_app_reads_its_round_from_the_ledger.rs`), and draws exactly its
//! road at the member:
//!
//! - ONE question edge from the declared cell onto the generation, `read`
//!   restamped `in_read` and stamped `context.read_caller` with the app's
//!   name -- the edge writes the stamp, so an app cannot answer for another.
//!   The round is stamped too (review C-1): the member's,
//!   `["agent:<generation>","member:<ctx.member_person>"]`, over whatever
//!   round the app's message carries -- an app reads at most what its member
//!   may see, and a wish that names no person is asked (`wish_incomplete`).
//!   A `read` that carries `hop.error_code` is an answer and never a
//!   question, so a cell that echoes what it got cannot send it round again.
//! - ONE way back per brain (`talky`, `talky-chat`, `cogny`): a v-lane `read`
//!   from the brain's rim -- the lane DOCKS there (`at` at the assistant) --
//!   onto the same cell, guarded on the stamp, which it deletes on arrival.
//!   An answer reaches the app that asked and no other reader.
//!
//! It names a cell inside the app, it needs the generation the app is
//! installed for, and an app that declares none gets no edge.
//!
//! Pure script tests: the SHIPPED `classify` and `recipes` are run over stdin,
//! nothing is booted. Same technique as `gh949_an_app_declares_candidates.rs`.

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
const BRAINS: [&str; 3] = ["talky", "talky-chat", "cogny"];

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

fn from_cell<'a>(got: &'a [Value], cell: &str) -> Vec<&'a Value> {
    got.iter().filter(|e| e["from"] == json!(cell)).collect()
}

fn to_cell<'a>(got: &'a [Value], cell: &str) -> Vec<&'a Value> {
    got.iter().filter(|e| e["to"] == json!(cell)).collect()
}

/// Red before GH #949: `reads` is not in the vocabulary, so the whole
/// declaration is refused (`app_declaration_invalid`, field `declaration`) and
/// `edges()` fails on the refusal.
#[test]
fn reads_draws_one_question_edge_onto_the_generation() {
    let got = edges(&params(json!({"reads": "./sink"})));
    let out = from_cell(&got, "./apps/probe-app/sink");
    assert_eq!(out.len(), 1, "{got:#?}");
    let q = out[0];
    assert_eq!(q["to"], "./assistants/sam");
    assert_eq!(
        q["condition"], "has(hop.route) && hop.route == 'read' && !has(hop.error_code)",
        "a `read` that carries an error code is an answer, never a question"
    );
    assert_eq!(q["modifier"]["set_hop"]["route"], "'in_read'");
    assert_eq!(
        q["modifier"]["set_context"],
        json!({"read_caller": "'probe-app'", "audience_set": MEMBER_ROUND}),
        "the edge stamps the asker and the member's round, and nothing else"
    );
    assert!(
        q.get("lane").is_none(),
        "a plain edge onto the generation's rim"
    );
    assert!(q.get("default").is_none(), "{q:#?}");
    assert!(q.get("tap").is_none(), "{q:#?}");
}

/// The way back: one v-lane per brain, docking at the brain's rim, guarded on
/// the asker's stamp and deleting it on arrival. Nothing else is drawn.
#[test]
fn reads_draws_one_way_back_per_brain_for_the_asker_only() {
    let got = edges(&params(json!({"reads": "./sink"})));
    let back = to_cell(&got, "./apps/probe-app/sink");
    assert_eq!(back.len(), BRAINS.len(), "{got:#?}");
    for brain in BRAINS {
        let from = format!("./assistants/sam/{brain}");
        let e = back
            .iter()
            .find(|e| e["from"] == json!(from))
            .unwrap_or_else(|| panic!("no way back from {from}: {got:#?}"));
        assert_eq!(e["lane"], "read", "a v-lane names its lane: {e:#?}");
        assert_eq!(
            e["condition"],
            "has(hop.route) && hop.route == 'read' && has(context.read_caller) \
             && context.read_caller == 'probe-app'"
        );
        assert_eq!(
            e["modifier"],
            json!({"delete_context": ["read_caller"]}),
            "the stamp ends where the answer arrives"
        );
        assert!(
            e.get("tap").is_none() && e.get("default").is_none(),
            "{e:#?}"
        );
    }
    // One question, three ways back, nothing else.
    assert_eq!(got.len(), 1 + BRAINS.len(), "{got:#?}");
}

/// Two apps that read the same generation each hear only their own answers:
/// every way back names its own app, and no edge of one app carries the other's
/// name.
#[test]
fn two_readers_of_one_generation_hear_only_their_own() {
    let a = edges(&params_for("probe-app", json!({"reads": "./sink"})));
    let b = edges(&params_for("other-app", json!({"reads": "./sink"})));
    for (mine, theirs, drawn) in [
        ("probe-app", "other-app", &a),
        ("other-app", "probe-app", &b),
    ] {
        for e in drawn {
            let s = e.to_string();
            assert!(
                !s.contains(theirs),
                "{mine} draws an edge naming {theirs}: {e}"
            );
        }
        for e in to_cell(drawn, &format!("./apps/{mine}/sink")) {
            assert!(
                e["condition"]
                    .as_str()
                    .is_some_and(|c| c.ends_with(&format!("context.read_caller == '{mine}'"))),
                "{e:#?}"
            );
        }
    }
}

#[test]
fn reads_start_at_the_cell_they_name() {
    let got = edges(&params(
        json!({"candidates": "./pusher", "reads": "./reader"}),
    ));
    assert_eq!(
        from_cell(&got, "./apps/probe-app/reader").len(),
        1,
        "{got:#?}"
    );
    assert_eq!(
        to_cell(&got, "./apps/probe-app/reader").len(),
        3,
        "{got:#?}"
    );
    let pusher = from_cell(&got, "./apps/probe-app/pusher");
    assert_eq!(pusher.len(), 1, "{got:#?}");
    assert_eq!(pusher[0]["modifier"]["set_hop"]["route"], "'in_candidate'");
}

#[test]
fn an_app_without_reads_draws_none() {
    let got = edges(&params(json!({
        "offers": [{"kind": "sidecar", "at": "./sink", "section": "probe"}],
        "listens": ["answer"],
        "pins": "./sink",
        "candidates": "./sink"})));
    assert!(
        !got.iter().any(|e| {
            let s = e.to_string();
            s.contains("in_read") || s.contains("'read'") || s.contains("read_caller")
        }),
        "{got:#?}"
    );
}

/// Red before GH #949: the `known` list of the refusal does not carry the word.
#[test]
fn the_vocabulary_knows_reads() {
    let p = refusal(&params(json!({"readz": "./sink"})));
    assert_eq!(p["field"], "declaration");
    assert!(
        p["known"]
            .as_array()
            .expect("known")
            .contains(&json!("reads")),
        "{p}"
    );
}

/// Red before GH #949: every bad value was refused as an unknown KEY (field
/// `declaration`), never by the field itself.
#[test]
fn reads_names_a_cell_inside_the_app() {
    for bad in [
        json!("sink"),
        json!("./"),
        json!("./../x"),
        json!(3),
        json!(["./sink"]),
        json!({"at": "./sink"}),
    ] {
        let p = refusal(&params(json!({"reads": bad.clone()})));
        assert_eq!(p["field"], "reads", "{bad}: {p}");
    }
}

/// The generation is needed at the switch (classify) and at the renderer.
/// Red before GH #949: classify routed a `reads`-only declaration to the
/// recipe without a generation.
#[test]
fn reads_needs_the_generation_at_the_switch() {
    let mut p = params(json!({"reads": "./sink"}));
    p.as_object_mut().expect("params").remove("generation");
    let out = classify(p.clone());
    assert_eq!(
        out["header"]["error_code"],
        json!("recipe_params_incomplete"),
        "{out}"
    );
    assert_eq!(payload(&out)["missing"], json!(["generation"]), "{out}");

    let all = run_recipes(&p);
    let first = all.first().expect("an emission");
    assert_eq!(
        first["header"]["error_code"],
        json!("recipe_params_incomplete"),
        "{first}"
    );
    assert!(first["manifest"].is_null(), "{first}");
}

#[test]
fn the_switch_names_reads_among_the_known_words() {
    let mut p = params(json!({}));
    p["declaration"] = json!("reads: ./sink");
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
            .contains(&json!("reads")),
        "{out}"
    );
}

/// GH #949 (review C-1) -- a read leaves in its MEMBER's round. The round an
/// app's message carries is written by the app's own template, so the edge
/// the builder draws stamps `["agent:<generation>","member:<person>"]` over
/// it; a wish with `reads` that names no person renders nothing and is
/// asked, at the renderer and at the switch, with one question.
///
/// Red before the fix: the edge stamps no round and the wish renders.
#[test]
fn a_read_asks_in_the_members_round_and_the_wish_names_the_person() {
    let got = edges(&params(json!({"reads": "./sink"})));
    let out = from_cell(&got, "./apps/probe-app/sink");
    assert_eq!(out.len(), 1, "{got:#?}");
    assert_eq!(
        out[0]["modifier"]["set_context"]["audience_set"],
        json!(MEMBER_ROUND),
        "the edge stamps the member's round: {got:#?}"
    );

    let mut p = params(json!({"reads": "./sink"}));
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
