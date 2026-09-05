//! GH #472 — `birth` is a recipe parameter, so a level can be grown asleep
//! through the one door instead of through a second one.
//!
//! `add_nodes[].birth` (GH #437) lets a whole subtree come to the world
//! registered, addressable and TASKLESS. It is the mechanism a connector needs:
//! a channel has to exist in the topology before its upstream is real. The
//! builder could not express it — `classify` forwarded four optional keys and
//! `grow_level` wrote three node keys, `birth` was in neither, and the briefing
//! called `override_params` "the only optional one". So a channel grown from a
//! wish was always born awake, and getting it asleep meant routing the draft
//! through the operator's lifecycle door instead of submitting it as a manifest:
//! a second door for what is one decision.
//!
//! What is pinned here is the whole of the switch-and-render half, and one thing
//! beyond it: the vocabulary. The recipe does not get to invent a third birth
//! state, because the door would refuse it — so the two words are read out of
//! the shipped script and held against `Birth`'s own wire spellings. A recipe
//! that rendered `asleep` would produce a manifest that stops at declaration
//! one, and the expensive way to learn about a typo is at the door.
//!
//! The BOOTED half — a rendered channel that comes up `inactive` in a running
//! colony — lives in `gh466_a_recipe_grown_level_boots.rs`, beside the harness
//! that already grows the levels.

use meclaw_colony::mutation::Birth;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_all, emit_one, shipped_script};
use std::path::PathBuf;

const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);
const CLASSIFY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/classify/config.json"
);
const BRIEF: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/brief/config.json"
);

/// The leg of the brief that reaches `./compose`. Since GH #477 the cell is a
/// multi-send: the store leg that parks the question and the instructions in
/// the round table travels first, the briefing itself second.
fn compose_leg(all: Vec<Value>) -> Value {
    all.into_iter()
        .find(|m| m["header"]["route"] == "compose")
        .expect("the brief's leg to the composer")
}

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .join(rel)
}

/// The renderer's FIRST answer to a wish, header and manifest alike.
///
/// First, because since GH #543 a member wish renders two manifests — the level,
/// and then the devices that member always gets. The birth state under test is
/// a property of the level, which is the first one.
fn run_recipes(params: Value) -> Value {
    let all = emit_all(
        &shipped_script(RECIPES),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe", "member_index": "0"},
                       "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "grow_level", "request": "…",
                                         "params": params}).to_string()}],
        }),
    );
    all.into_iter()
        .next()
        .expect("the renderer answers a wish at all")
}

/// One emission of the switch, for a wish written as a sentence.
fn run_classify(args: Value) -> Value {
    emit_one(
        &shipped_script(CLASSIFY),
        &json!({
            "target": "/os/builder/classify",
            "header": {"hop": {"route": "in_build"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_call", "id": "c1",
                          "text": args.to_string()}],
        }),
    )
}

/// The `add_nodes` entry a level renders, or the header of the refusal.
fn node(params: Value) -> Result<Value, Value> {
    let out = run_recipes(params);
    match out["manifest"].as_array() {
        Some(decls) if decls.len() == 1 => Ok(decls[0]["diff"]["add_nodes"][0].clone()),
        _ => Err(out["header"].clone()),
    }
}

/// The parameters of one level, minus whatever the test is varying.
fn level_params(level: &str) -> Value {
    let member = "/os/orgs/acme/members/alex";
    match level {
        "org" => json!({"scope": "/os", "level": "org", "name": "acme",
                        "template": "a-template@1.0.0"}),
        "member" => json!({"scope": "/os/orgs/acme", "level": "member", "name": "alex",
                           "template": "a-template@1.0.0"}),
        "assistant" => json!({"scope": member, "level": "assistant", "name": "scribe",
                              "template": "a-template@1.0.0"}),
        // GH #517 -- a channel renders nothing at all without the person its
        // round is spoken with; it asks instead.
        "channel" => json!({"scope": member, "level": "channel", "name": "telegram",
                            "assistant": "scribe", "template": "a-template@1.0.0",
                            "ctx": {"member_person": "alex"}}),
        "screen" => json!({"scope": member, "level": "screen", "name": "display-desk",
                           "template": "a-template@1.0.0"}),
        "app" => json!({"scope": member, "level": "app", "name": "colony-view",
                        "screen": "display-desk", "template": "a-template@1.0.0"}),
        other => panic!("no such level: {other}"),
    }
}

const LEVELS: [&str; 6] = ["org", "member", "assistant", "channel", "screen", "app"];

// ══════════════════════════════════════════════════════ what gets rendered

/// The state travels TOP-LEVEL on the entry, beside `template` — the place the
/// door reads it. Inside `override_params` it would be a cell's own setting, and
/// the door would never look there.
#[test]
fn the_birth_state_is_written_beside_the_template_and_not_inside_the_params() {
    let mut params = level_params("assistant");
    params["birth"] = json!("inactive");
    params["override_params"] = json!({"cogny/brain": {"temperature": 0.2}});
    let n = node(params).expect("the level renders");

    assert_eq!(
        n["birth"],
        json!("inactive"),
        "`birth` belongs on the add_nodes entry itself: {n}"
    );
    assert!(
        n["override_params"]["birth"].is_null()
            && n["override_params"]["cogny/brain"]["birth"].is_null(),
        "`birth` is a property of the PLACEMENT, not a param of a cell — it must \
         not leak into override_params: {n}"
    );
}

/// A channel is the one level whose default is not the door's, and the other
/// five say nothing — a diff that says nothing behaves exactly as it did.
#[test]
fn a_channel_is_the_one_level_born_asleep_and_the_others_say_nothing() {
    let mut asleep = Vec::new();
    for level in LEVELS {
        let n = node(level_params(level)).expect("the level renders");
        match n["birth"].as_str() {
            None => {}
            Some(state) => {
                assert_eq!(
                    state,
                    Birth::WIRE_INACTIVE,
                    "{level}: a level that renders a default renders the asleep one"
                );
                asleep.push(level);
            }
        }
    }
    assert_eq!(
        asleep,
        vec!["channel"],
        "exactly one level is born asleep by default. A connector opens its \
         upstream the moment it has a task (GH #468); every other level is a \
         composition of cells that wait to be addressed, and a default asleep \
         there would be a colony that boots and answers nothing"
    );
}

/// The key a caller filled in is a decision. It beats the level's default in
/// both directions, and the state it names is written out rather than left to
/// a default the reader of the manifest cannot see.
#[test]
fn a_named_birth_state_beats_the_levels_default() {
    let mut awake = level_params("channel");
    awake["birth"] = json!(Birth::WIRE_ACTIVE);
    assert_eq!(
        node(awake).expect("renders")["birth"],
        json!(Birth::WIRE_ACTIVE),
        "a wish that asks for the channel awake gets it, spelled out"
    );

    let mut asleep = level_params("member");
    asleep["birth"] = json!(Birth::WIRE_INACTIVE);
    assert_eq!(
        node(asleep).expect("renders")["birth"],
        json!(Birth::WIRE_INACTIVE),
        "any level can be grown asleep, not just the one that defaults to it"
    );
}

// ═══════════════════════════════════════════════════════ what gets refused

/// Named, never guessed at — the rule the level name already follows. The door
/// refuses an unknown state pre-destructively, so nothing is LOST by passing it
/// through; what is lost is the name of the refusal, one hop from the wish.
#[test]
fn an_unknown_birth_state_is_refused_by_name_and_no_manifest_comes_back() {
    // at the renderer
    let mut params = level_params("channel");
    params["birth"] = json!("asleep");
    let refusal = node(params).expect_err("a third birth state must not render");
    assert_eq!(
        refusal["error_code"],
        json!("birth_unknown"),
        "the renderer answers a birth state it does not know by name: {refusal}"
    );

    // and at the switch, BEFORE an inference could be bought
    let early = run_classify(json!({
        "recipe": "grow_level", "request": "…",
        "params": {"scope": "/os", "level": "org", "name": "acme",
                   "template": "org@1.4.0", "birth": "dormant"}}));
    assert_eq!(
        early["header"]["route"],
        json!("error"),
        "the switch must refuse it rather than send it on: {early}"
    );
    assert_eq!(
        early["header"]["error_code"],
        json!("birth_unknown"),
        "a typo in one argument must be refused by its own name, not downgraded \
         into the design lane where a model would answer a different question"
    );
}

/// § 2d, and the reason this file names no words of its own: the two spellings
/// are the DOOR's, read out of the shipped script and held against `Birth`.
/// A third one here would render a manifest that stops at declaration one.
#[test]
fn the_recipe_speaks_the_doors_own_vocabulary_and_no_word_of_its_own() {
    let mut found = Vec::new();
    for path in [RECIPES, CLASSIFY] {
        let script = shipped_script(path);
        let (_, rest) = script
            .split_once("BIRTH_STATES = (")
            .unwrap_or_else(|| panic!("{path} no longer declares the birth vocabulary"));
        let (list, _) = rest.split_once(')').expect("a closed tuple");
        let states: Vec<String> = list
            .split(',')
            .map(|s| s.trim().trim_matches('"').to_string())
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(
            states,
            vec![Birth::WIRE_ACTIVE, Birth::WIRE_INACTIVE],
            "{path} declares a birth vocabulary the mutation door does not \
             share. `Birth::parse` refuses everything else, so a third word \
             here is a manifest that stops at declaration one"
        );
        found.push(path);
    }
    assert_eq!(found.len(), 2, "both halves of the fast lane were checked");
}

// ═════════════════════════════════════════════════════ what the words say

/// The wish is a sentence before it is a key. Both spellings a request actually
/// uses are read, and an explicit argument still wins — a key a caller filled in
/// is a decision, a sentence is a reading of one.
#[test]
fn the_sentence_is_read_for_asleep_and_for_awake() {
    let grow = |sentence: &str, extra: Value| {
        let mut args = json!({"request": sentence, "assistant": "scribe"});
        if let Some(map) = extra.as_object() {
            for (k, v) in map {
                args[k] = v.clone();
            }
        }
        let out = run_classify(args);
        assert_eq!(
            out["header"]["route"],
            json!("recipe"),
            "the sentence must still take the fast lane: {out}"
        );
        let text = out["messages"][0]["text"].as_str().expect("a payload");
        let payload: Value = meclaw_core::serde_json::from_str(text).expect("json");
        payload["params"]["birth"].clone()
    };

    const AWAKE_WISH: &str = "grow a channel named telegram from a-template@1.0.0 under \
         /os/orgs/acme/members/alex, born awake";
    const ASLEEP_WISH: &str = "grow an assistant named scribe from a-template@1.0.0 under \
         /os/orgs/acme/members/alex, born asleep";

    assert_eq!(
        grow(ASLEEP_WISH, json!({})),
        json!(Birth::WIRE_INACTIVE),
        "a wish that says asleep is read as asleep"
    );
    // GH #517 -- the channel sentence carries the person its round is spoken
    // with, because a wish that does not name one is not complete for the fast
    // lane at all and falls through to the design lane, where it is asked.
    assert_eq!(
        grow(AWAKE_WISH, json!({"ctx": {"member_person": "alex"}})),
        json!(Birth::WIRE_ACTIVE),
        "and a wish that says awake overrides the level's own default"
    );
    assert_eq!(
        grow(ASLEEP_WISH, json!({"birth": Birth::WIRE_ACTIVE})),
        json!(Birth::WIRE_ACTIVE),
        "the argument beats the sentence: a key a caller filled in is a decision"
    );
}

/// § 2d, the prose half. Both public surfaces the builder speaks through say
/// that a channel is the level born asleep, and the mechanism above is what
/// makes the sentence true. A sentence with no test behind it is a wish.
#[test]
fn both_public_surfaces_say_which_level_sleeps_and_the_renderer_agrees() {
    let readme = std::fs::read_to_string(repo("templates/builder/README.md"))
        .expect("the builder README travels with the template");
    let flat = readme.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flat.contains("`channel`, which is born **asleep**")
            || flat.contains("`channel`, which is born asleep"),
        "the README's parameter table no longer says which level defaults to \
         asleep, and it is the only place the default is published"
    );

    let brief = compose_leg(emit_all(
        &shipped_script(BRIEF),
        &json!({
            "target": "/os/builder/brief",
            "header": {"hop": {"route": "brief"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "user", "type": "text", "id": "",
                          "text": "grow something"}],
        }),
    ));
    let text = brief["system"]["instructions"]["text"]
        .as_str()
        .expect("the briefing carries instructions");
    assert!(
        text.contains("BIRTH --"),
        "the design lane is never taught the key it is allowed to write, which \
         is the half of #472 that is not about the fast lane at all"
    );
    assert!(
        text.contains("a channel is grown asleep unless the wish says otherwise"),
        "the briefing must name the one default that is not the door's, or a \
         model will write it out on every channel it draws"
    );

    // The mechanism, in the same test: the sentence above is true of the table.
    let rendered = node(level_params("channel")).expect("renders");
    assert_eq!(
        rendered["birth"],
        json!(Birth::WIRE_INACTIVE),
        "both surfaces say a channel is born asleep and the renderer does not"
    );
}
