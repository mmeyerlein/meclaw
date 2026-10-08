//! GH #967 — every value written into a round literal is checked.
//!
//! The round a channel stamps on its turns, and the round an app's pins,
//! candidates, reads and tool results leave in, is a JSON array inside a CEL
//! string: `'["agent:<a>","member:<p>"]'`. GH #949 checked the person; the
//! assistant (`_channel_level`) and the generation (`install_app`) beside it
//! were written in unchecked, so `x","*` stamped `["agent:x","*",...]` -- a
//! round that covers every row. Now all three are asked again, the same way
//! the person is (`wish_incomplete`, nothing rendered), and the builder's fast
//! lane (`classify`, `told`) checks the person exactly as the main lane does.
//!
//! The Rust half of #967 (a `//` comment opens no literal in `token_quoting`)
//! is the unit test `a_comment_does_not_open_a_literal` in
//! `crates/meclaw-colony/src/mutation/substitute.rs`.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_one, shipped_script};

const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);
const CLASSIFY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/classify/config.json"
);

const MEMBER_DIR: &str = "/os/orgs/acme/members/alex";
/// Each one would end the name's JSON string or the CEL string around it, or
/// widen the round (`*`, a comma), or hide in a control character.
const BAD: [&str; 8] = [r#"x","*"#, "x'", "x\\", "x$", "*", "x,y", "x\ty", "x\""];

fn run_recipes(payload: Value) -> Value {
    emit_one(
        &shipped_script(RECIPES),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": payload.to_string()}],
        }),
    )
}

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

fn payload(out: &Value) -> Value {
    meclaw_core::serde_json::from_str(out["messages"][0]["text"].as_str().expect("a payload"))
        .expect("json payload")
}

fn channel_wish(assistant: &str, person: &str) -> Value {
    json!({"recipe": "grow_level", "request": "…", "params": {
        "scope": MEMBER_DIR, "level": "channel", "name": "telegram",
        "template": "telegram-connector@2.2.0", "assistant": assistant,
        "bind_chat": "4711", "ctx": {"member_person": person}}})
}

fn app_wish(generation: &str, person: &str) -> Value {
    json!({"recipe": "install_app", "request": "…", "params": {
        "scope": "/os/orgs/acme/members/alex", "app": "pinner",
        "template": "pinner@1.0.0", "screen": "display", "generation": generation,
        "ctx": {"member_person": person},
        "declaration": {"pins": "./pin", "candidates": "./pin", "reads": "./pin"}}})
}

/// Asked again, nothing rendered, the parameter named.
fn assert_asked(out: &Value, missing: &str, what: &str) {
    assert_eq!(
        out["header"]["error_code"],
        json!("wish_incomplete"),
        "{what}: {out}"
    );
    assert!(out["manifest"].is_null(), "{what}: rendered anyway: {out}");
    assert_eq!(payload(out)["missing"], json!([missing]), "{what}");
}

/// Red before the fix: the assistant and the generation rendered with `*` in
/// the round; the person was refused already (GH #949) and stays refused.
#[test]
fn a_round_literal_takes_only_checked_names() {
    for bad in BAD {
        assert_asked(
            &run_recipes(channel_wish(bad, "alex")),
            "params.assistant",
            &format!("assistant {bad:?}"),
        );
        let app = run_recipes(app_wish(bad, "owner"));
        assert_asked(&app, "params.generation", &format!("generation {bad:?}"));
        // Review M-2: the app is asked about its generation, not about "the
        // assistant this is for" -- the channel's question.
        let asked = payload(&app)["reason"].as_str().unwrap_or("").to_string();
        assert!(
            asked.contains("generation") && asked.contains("app"),
            "generation {bad:?}: the app is asked the channel's question: {asked:?}"
        );
        assert_asked(
            &run_recipes(channel_wish("scribe", bad)),
            "ctx.member_person",
            &format!("person {bad:?}"),
        );
    }
    // Plain names render as before, and the turn is born with its round under
    // the stamped key too (GH #972, R-NL-4).
    let out = run_recipes(channel_wish("scribe", "alex"));
    let edges = out["manifest"][0]["diff"]["add_edges"]
        .as_array()
        .unwrap_or_else(|| panic!("no manifest: {out}"))
        .clone();
    let ingress = edges
        .iter()
        .find(|e| e["modifier"]["set_context"]["audience_set"].is_string())
        .expect("the ingress edge");
    let round = r#"'["agent:scribe","member:alex"]'"#;
    assert_eq!(
        ingress["modifier"]["set_context"]["audience_set"],
        json!(round)
    );
    assert_eq!(
        ingress["modifier"]["set_context"]["turn_round"],
        json!(round)
    );
    let out = run_recipes(app_wish("scribe", "owner"));
    assert!(
        out["manifest"].is_array(),
        "a plain generation renders: {out}"
    );
}

/// Red before the fix: the fast lane's `told` only asked "is anything there",
/// so a sentence with `x","*` as the person took the recipe route. It now asks
/// what the main lane asks (`person_named`) and falls through to the design
/// lane, whose composer asks the question.
#[test]
fn told_checks_the_person_like_the_main_lane() {
    let sentence =
        format!("grow a channel named telegram from telegram-connector@2.2.0 under {MEMBER_DIR}");
    for bad in BAD {
        let out = run_classify(json!({"request": sentence, "assistant": "scribe",
                                      "ctx": {"member_person": bad}}));
        assert_eq!(
            out["header"]["route"],
            json!("design"),
            "{bad:?}: the fast lane took a person the main lane refuses: {out}"
        );
    }
    let told = run_classify(json!({"request": sentence, "assistant": "scribe",
                                   "ctx": {"member_person": "alex"}}));
    assert_eq!(told["header"]["route"], json!("recipe"), "{told}");
}
