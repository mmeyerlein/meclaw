//! GH #466 — a LEVEL is a recipe, and the table it renders from is pinned
//! against the worked example rather than described beside it.
//!
//! Growing a child into a composition level was a paragraph the composer
//! rewrote from scratch on every build: a fixed set per level, the same edges
//! every time, with the child's name substituted in. A model that writes them by
//! hand writes most of them, and a level wired with nine of its eleven edges
//! boots and answers nothing on the two nobody drew.
//!
//! The counts themselves are deliberately NOT written here. They live in the
//! README of the template and are read back out of it below — GH #470 is what
//! that discipline is for: the two container levels sat at thirteen edges from
//! before the member had a memory export, and this file could not see it,
//! because the examples it diffs against were generated out of the same table.
//! `gh470_a_grown_container_level_carries_its_export_lanes.rs` is the second
//! opinion that was missing.
//!
//! So `recipes` renders them from a table, and this file is what stops the table
//! from becoming a second opinion: every level is rendered through the SHIPPED
//! script and compared **byte for byte** against the declaration
//! `examples/organism` already carries. The example and the recipe cannot drift
//! apart, because one is generated and diffed against the other.
//!
//! It also holds the COUNTS to one definition. `docs/development-rules.md` § 2d:
//! a number in template prose is either derived from the code inside the test or
//! it appears exactly once. Here it is derived — the README table's numbers are
//! read out of the prose and checked against what the renderer produced, and so
//! are the ones the briefing tells the composer.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_all, emit_one, shipped_script};
use std::path::{Path, PathBuf};

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

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// The manifest a wish renders. Since GH #543 a member wish renders the person
/// AND the screen and the app that person always gets, and since GH #585 the
/// three ride in ONE manifest, in that order — this file is about the LEVEL,
/// which is the declaration up front. The two behind it are owned by
/// `gh543_a_member_always_gets_its_screen.rs`.
fn run_recipes(payload: Value) -> Value {
    let all = emit_all(
        &shipped_script(RECIPES),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": payload.to_string()}],
        }),
    );
    assert_eq!(
        all.len(),
        1,
        "a wish that is not a member's renders exactly one manifest and no \
         binding: {all:?}"
    );
    all.into_iter().next().expect("one emission")
}

/// The same, for the one wish that renders more than one thing.
fn first_manifest(payload: Value) -> Value {
    emit_all(
        &shipped_script(RECIPES),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe", "member_index": "0"},
                       "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": payload.to_string()}],
        }),
    )
    .into_iter()
    .find(|m| m["header"]["operation"] == json!("recipe"))
    .expect("a first manifest")
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

/// One rendered declaration for a level, as `grow_level` produces it.
fn grow(params: Value) -> Value {
    let member = params["level"] == json!("member");
    let wish = json!({"recipe": "grow_level", "request": "…", "params": params});
    let out = if member {
        first_manifest(wish)
    } else {
        run_recipes(wish)
    };
    let decls = out["manifest"]
        .as_array()
        .unwrap_or_else(|| panic!("no manifest: {out}"));
    // The LEVEL is one declaration: the node and its edges are one decision, and
    // a manifest rolls forward — a node that landed without its edges is the
    // island this recipe exists to prevent. Since GH #585 a member wish carries
    // two more behind it, the devices that member always gets, in the same
    // manifest; the level is the one in front either way.
    assert_eq!(
        decls.len(),
        if member { 3 } else { 1 },
        "the level is not the declaration this manifest leads with: {decls:?}"
    );
    decls[0].clone()
}

/// The example's declarations, normalised to the three keys a declaration has.
/// `ctx` is defaulted to `{}` exactly as `normalise` defaults it, so a file that
/// omits the block and a renderer that writes an empty one still compare equal.
///
/// A file is one declaration or a `manifest` of several. Since GH #503 a level
/// declares itself AT the container it grows into, so two levels that used to
/// share one mutation — a screen into `./channels`, an app into `./apps` — have
/// two scope roots and cannot: `grow-screen.json` carries them as a manifest.
fn examples(file: &str) -> Option<Vec<Value>> {
    let raw = read(&repo(&format!("examples/organism/{file}")))?;
    let v: Value = meclaw_core::serde_json::from_str(&raw).expect("the example is json");
    let decls = match v["manifest"].as_array() {
        Some(list) => list.clone(),
        None => vec![v],
    };
    Some(
        decls
            .into_iter()
            .map(|d| {
                json!({
                    "scope": d["scope"],
                    "ctx": if d["ctx"].is_object() { d["ctx"].clone() } else { json!({}) },
                    "diff": d["diff"],
                })
            })
            .collect(),
    )
}

/// The six levels the table carries, each with the parameters the recipe needs
/// and the example it must reproduce. The two that share `grow-screen.json`
/// carry the declaration of it they own.
struct Level {
    /// the level's name in the recipe's table
    name: &'static str,
    /// the wish, minus the template — that comes off the example
    params: Value,
    /// the shipped declaration it has to reproduce
    file: &'static str,
    /// which declaration of that file this level owns, when one file carries
    /// more than one (GH #503: a screen and an app no longer share a scope)
    index: usize,
}

fn levels() -> Vec<Level> {
    vec![
        Level {
            name: "org",
            params: json!({"scope": "/os", "level": "org", "name": "acme"}),
            file: "grow-org.json",
            index: 0,
        },
        Level {
            name: "member",
            params: json!({"scope": "/os/orgs/acme", "level": "member", "name": "alex"}),
            file: "grow-member.json",
            index: 0,
        },
        Level {
            name: "assistant",
            params: json!({"scope": "/os/orgs/acme/members/alex", "level": "assistant",
                   "name": "scribe",
                   "ctx": {"model": "${MODEL_CORE}", "model_fast": "${MODEL_CORE_FAST}",
                           "model_surface": "${MODEL_SURFACE}"},
                   "override_params": {"cogny/brain": {"temperature": 0.2}}}),
            file: "grow-assistant.json",
            index: 0,
        },
        Level {
            name: "channel",
            // GH #517 -- the round is a thing the WISH says. `alex` is the
            // person here AND the folder, which is precisely why this example
            // could not see the defect: the two coincide.
            params: json!({"scope": "/os/orgs/acme/members/alex", "level": "channel",
                   "name": "telegram", "assistant": "scribe",
                   "ctx": {"member_person": "alex"}}),
            file: "grow-channel.json",
            index: 0,
        },
        // GH #543 — the two devices a member always gets, at the names the
        // renderer gives them and on the port `screen_port_base` hands the
        // first member of an organisation.
        Level {
            name: "screen",
            params: json!({"scope": "/os/orgs/acme/members/alex", "level": "screen",
                   "name": "display",
                   "override_params": {"web": {"port": 7900}}}),
            file: "grow-screen.json",
            index: 0,
        },
        Level {
            name: "app",
            params: json!({"scope": "/os/orgs/acme/members/alex", "level": "app",
                   "name": "colony-view", "screen": "display"}),
            file: "grow-screen.json",
            index: 1,
        },
    ]
}

/// level -> how many transit edges it renders. Derived, never written down here.
fn rendered_counts() -> Vec<(&'static str, usize)> {
    levels()
        .into_iter()
        .map(|lv| {
            // Any template at all: the COUNT is a property of the parent level,
            // and the whole point of the table is that it does not depend on
            // what the child is filled with.
            let mut params = lv.params;
            params["template"] = json!("a-template@1.0.0");
            let d = grow(params);
            let n = d["diff"]["add_edges"].as_array().expect("add_edges").len();
            (lv.name, n)
        })
        .collect()
}

#[test]
fn every_level_renders_the_declaration_the_example_carries() {
    let mut compared = 0usize;
    for lv in levels() {
        let (name, mut params, file, index) = (lv.name, lv.params, lv.file, lv.index);
        let Some(decls) = examples(file) else {
            continue; // a tree without the examples cannot make this assertion
        };
        let want = &decls[index];
        // The template comes OFF the example rather than out of this file. What
        // is under test is the edge set, and a version bump of a level template
        // is not a defect in the table — it would only make this test lie about
        // which one it is.
        params["template"] = want["diff"]["add_nodes"][0]["template"].clone();
        let got = grow(params);
        compared += 1;

        // The node: the same name and the same template, at the same depth.
        let want_nodes = want["diff"]["add_nodes"].as_array().expect("add_nodes");
        let got_nodes = got["diff"]["add_nodes"].as_array().expect("add_nodes");
        assert_eq!(
            got_nodes.len(),
            1,
            "{name}: one node per level, not a bundle"
        );
        assert_eq!(&got_nodes[0], &want_nodes[0], "{name}: the node differs");

        // The edges: byte for byte, IN ORDER. Order is semantics here for the
        // same reason it is in every other recipe — a manifest rolls forward.
        assert_eq!(
            got["diff"]["add_edges"], want["diff"]["add_edges"],
            "{name}: the rendered transit edges are not the ones \
             examples/organism/{file} carries"
        );
        // The SCOPE ROOT, and it is the whole of GH #503: a level declares
        // itself AT the container it grows into, which is the address the
        // broker judges — `/os/orgs` for the first organisation of a colony,
        // never `/os`.
        assert_eq!(got["scope"], want["scope"], "{name}: scope");
    }
    assert!(
        compared >= 6,
        "only {compared} of six levels were compared — the examples moved, the \
         table did not (§ 2c: an empty result and a forgotten call must never \
         look alike)"
    );
}

#[test]
fn the_readme_publishes_the_counts_and_the_renderer_owns_them() {
    let Some(readme) = read(&repo("templates/builder/README.md")) else {
        return;
    };
    // The prose says "an organisation gets 13 transit edges, a member 13, an
    // assistant 11, a channel 3, a screen 2, an app 2". Read the bolded numbers
    // in that order and hold them to what the renderer actually produced.
    let counts = rendered_counts();
    let flat = readme.split_whitespace().collect::<Vec<_>>().join(" ");
    let start = flat.find("an organisation gets").unwrap_or_else(|| {
        panic!("the README no longer publishes the per-level counts anywhere this test can find")
    });
    let rest = &flat[start..];
    let end = rest.find(" and they are").unwrap_or(rest.len());
    let numbers: Vec<usize> = rest[..end]
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    assert_eq!(
        numbers,
        counts.iter().map(|(_, n)| *n).collect::<Vec<_>>(),
        "the README's per-level counts and the renderer disagree ({counts:?}) — \
         § 2d: a number in template prose is derived from the code or it appears \
         exactly once, and this one is derived"
    );
}

#[test]
fn the_briefing_tells_the_composer_the_same_counts() {
    // The grammar block that teaches a level names the numbers in words, because
    // it is prose a model reads. Words drift from digits silently; this is the
    // only place the two are compared.
    let out = compose_leg(emit_all(
        &shipped_script(BRIEF),
        &json!({
            "target": "/os/builder/brief",
            "header": {"hop": {"route": "brief"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "user", "type": "text", "id": "",
                          "text": "grow something"}],
        }),
    ));
    let text = out["system"]["instructions"]["text"]
        .as_str()
        .expect("instructions")
        .to_string();
    let words = [
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
        "twenty",
        "twenty-one",
        "twenty-two",
        "twenty-three",
        "twenty-four",
    ];
    for (name, n) in rendered_counts() {
        let word = words.get(n - 1).copied().unwrap_or("?");
        let label = match name {
            "org" => "organisation",
            other => other,
        };
        let phrase = if label == "organisation" {
            format!("{word} edges for an {label}")
        } else {
            format!("{word} for an {label}")
        };
        let alt = format!("{word} for a {label}");
        assert!(
            text.contains(&phrase) || text.contains(&alt),
            "the briefing does not tell the composer that a {label} costs {n} \
             edges (looked for {phrase:?} / {alt:?})"
        );
    }
}

#[test]
fn a_level_the_table_does_not_carry_is_refused_by_name() {
    let out = run_recipes(json!({"recipe": "grow_level", "request": "…",
        "params": {"scope": "/os", "level": "department", "name": "x",
                   "template": "org@1.4.0"}}));
    assert_eq!(
        out["header"]["error_code"],
        json!("level_unknown"),
        "a level nobody wrote an edge set for is NAMED, never rendered as \
         something close: a subtree wired from the wrong table boots and \
         answers nothing"
    );
    assert!(
        out["manifest"].is_null(),
        "no manifest slot on a refusal — an empty manifest is a failure wearing \
         the face of an honest answer"
    );
    // and the switch refuses it one cell earlier, before an inference is bought
    let early = run_classify(json!({"request": "…", "recipe": "grow_level",
        "params": {"scope": "/os", "level": "department", "name": "x",
                   "template": "org@1.4.0"}}));
    assert_eq!(early["header"]["error_code"], json!("level_unknown"));
    assert_eq!(early["header"]["route"], json!("error"));
}

#[test]
fn a_named_grow_level_missing_its_per_level_parameter_is_refused_not_downgraded() {
    // A channel needs the agent its turns default to. Naming the recipe and
    // leaving it out must be an error — the same rule the other three recipes
    // live under, because a typo would otherwise silently buy an inference.
    let out = run_classify(json!({"request": "…", "recipe": "grow_level",
        "params": {"scope": "/os/orgs/acme/members/alex", "level": "channel",
                   "name": "telegram", "template": "telegram-connector@2.0.1"}}));
    assert_eq!(
        out["header"]["error_code"],
        json!("recipe_params_incomplete")
    );
    let payload: Value =
        meclaw_core::serde_json::from_str(out["messages"][0]["text"].as_str().expect("payload"))
            .expect("json payload");
    assert_eq!(payload["missing"], json!(["assistant"]));
}

#[test]
fn the_grow_sentence_takes_the_fast_lane_and_a_half_sentence_does_not() {
    let full = run_classify(json!({
        "request": "grow an assistant named scribe from assistant@2.5.0 under \
                    /os/orgs/acme/members/alex",
        "ctx": {"model": "m", "model_fast": "f", "model_surface": "s"}}));
    assert_eq!(full["header"]["route"], json!("recipe"));
    assert_eq!(full["header"]["recipe"], json!("grow_level"));

    // No template named: the sentence is not complete, so it goes to the model.
    // That is NOT the forbidden downgrade — nothing was NAMED, so nothing is
    // being quietly re-asked.
    let partial = run_classify(json!({
        "request": "grow an assistant named scribe under /os/orgs/acme/members/alex"}));
    assert_eq!(
        partial["header"]["route"],
        json!("design"),
        "a half-read wish belongs in the design lane, not in a recipe that \
         guesses the missing half"
    );

    // A channel without its default agent is likewise incomplete for the fast
    // lane, and likewise not an error: nobody named a recipe.
    let chan = run_classify(json!({
        "request": "grow a channel named telegram from telegram-connector@2.0.1 \
                    under /os/orgs/acme/members/alex"}));
    assert_eq!(chan["header"]["route"], json!("design"));
}

#[test]
fn the_fast_lane_still_costs_no_model_and_no_network() {
    // The whole point of a fourth recipe is that the most common structural
    // wish there is stops buying an inference. `recipes` is a `code` cell with
    // the network denied; asserting the sandbox here keeps a later "just let it
    // look one thing up" honest.
    let raw = read(&repo("templates/builder/recipes/config.json")).expect("recipes config");
    let cfg: Value = meclaw_core::serde_json::from_str(&raw).expect("json");
    assert_eq!(cfg["params"]["sandbox"]["network"], json!("deny"));
    assert_eq!(cfg["cell"]["type"], json!("code"));
}

#[test]
fn the_composer_budget_in_the_readme_is_the_one_the_cell_declares() {
    // The design lane still exists for the wish that is not exactly one recipe,
    // and its completion budget moved in this change. A number in template prose
    // that nothing derives is a number that goes stale (§ 2d).
    let Some(readme) = read(&repo("templates/builder/README.md")) else {
        return;
    };
    let raw = read(&repo("templates/builder/compose/config.json")).expect("compose config");
    let cfg: Value = meclaw_core::serde_json::from_str(&raw).expect("json");
    let max = cfg["params"]["max_tokens"].as_u64().expect("max_tokens");
    // The README writes it with a thin space for readability: "32 768".
    let spelled = format!("{} {}", max / 1000, max % 1000);
    assert!(
        readme.contains(&spelled),
        "the README does not name the composer's completion budget ({spelled})"
    );
    let timeout = cfg["params"]["external_timeout_ms"]
        .as_u64()
        .expect("timeout");
    let backstop = cfg["cell"]["message_timeout"].as_u64().expect("backstop");
    assert!(
        backstop > timeout,
        "B generous, A precise — the substrate backstop must outlast the call it \
         is backing (docs/meclaw-overview.md § Timeouts)"
    );
}

// ---------------------------------------------- the credential lanes (GH #560)

/// The wish that grows a generation with no key of its own. The three values
/// inside `credential` are the ones `examples/organism/grow-credentials.json`
/// carries, so the rendered edges and the shipped example can be compared byte
/// for byte — the same discipline the six levels above run under.
fn credentialled_wish() -> Value {
    json!({"scope": "/os/orgs/acme/members/alex", "level": "assistant",
           "name": "scribe", "template": "assistant@2.5.0",
           "ctx": {"model": "${MODEL_CORE}", "model_fast": "${MODEL_CORE_FAST}",
                   "model_surface": "${MODEL_SURFACE}"},
           "override_params": {"cogny/brain": {"temperature": 0.2}},
           "credential": {"cred_ref": "cred:example-provider:primary",
                          "subject": "member:alex",
                          "expires_at": "2099-01-01T00:00:00.000000Z",
                          "rule_id": "alex-credential-read"}})
}

fn grown_with_credential() -> Vec<Value> {
    let out = run_recipes(json!({"recipe": "grow_level", "request": "…",
                                 "params": credentialled_wish()}));
    out["manifest"]
        .as_array()
        .unwrap_or_else(|| panic!("no manifest: {out}"))
        .clone()
}

/// The rows of a `seed_rows` block with the three fields a renderer cannot
/// reproduce from a shipped file replaced by a sentinel.
///
/// Everything else — target, table, order, every column of every grant — is
/// compared verbatim. What is blanked is exactly what is generated at render
/// time or numbered by hand: `issued_at` and `at` are the moment the manifest
/// was drawn, and the event `id` is a sequence number in the shipped file and
/// the consumer's own name in the renderer.
fn without_the_stamps(v: &Value) -> Value {
    match v {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, val)| {
                    let val = match k.as_str() {
                        "issued_at" | "at" | "id" => json!("<stamped at render time>"),
                        _ => without_the_stamps(val),
                    };
                    (k.clone(), val)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_the_stamps).collect()),
        other => other.clone(),
    }
}

#[test]
fn the_credential_lanes_are_the_ones_the_example_carries() {
    let Some(want) = examples("grow-credentials.json") else {
        return; // a tree without the examples cannot make this assertion
    };
    let decls = grown_with_credential();
    assert_eq!(
        decls.len(),
        1,
        "since `builder@1.6.1` the credential form is ONE declaration: stage 6 \
         reads a newborn's contracts out of the whole staged subtree (GH #567), \
         so the connect point `talky`/`cogny` declare is found in the same act \
         that gives birth to their generation"
    );
    let got = &decls[0];
    assert_eq!(
        got["scope"], want[0]["scope"],
        "the credentialled declaration stands at the MEMBER — an edge lives in \
         the graph of the lowest common ancestor of its two endpoints, and a \
         brain and the member's broker share only that one"
    );
    // At the member the child is named through its container, exactly as the
    // `subscribe` form names it: the declaration stands one storey higher, so
    // the node name and every endpoint carry the container.
    assert_eq!(
        got["diff"]["add_nodes"][0]["name"],
        json!("assistants/scribe")
    );

    // The four v-lanes are the LAST four edges of the one declaration: the
    // level's own transit edges are drawn first and the credential road behind
    // them, and order is semantics here as everywhere else.
    let edges = got["diff"]["add_edges"].as_array().expect("add_edges");
    let want_edges = want[0]["diff"]["add_edges"]
        .as_array()
        .expect("the example's credential edges");
    assert!(
        edges.len() > want_edges.len(),
        "the one declaration carries the level's transit edges as well as the \
         credential road — it has {} edges",
        edges.len()
    );
    assert_eq!(
        &edges[edges.len() - want_edges.len()..],
        want_edges.as_slice(),
        "the rendered credential v-lanes are not the ones \
         examples/organism/grow-credentials.json carries"
    );
    // And the grants travel in the same breath, byte for byte but for the
    // stamps: the example stays the byte truth of this road.
    assert_eq!(
        without_the_stamps(&got["diff"]["seed_rows"]),
        without_the_stamps(&want[0]["diff"]["seed_rows"]),
        "the rendered grants are not the ones \
         examples/organism/grow-credentials.json carries"
    );
}

#[test]
fn both_brains_give_up_the_key_they_have_and_name_a_grant() {
    let decls = grown_with_credential();
    let overrides = &decls[0]["diff"]["add_nodes"][0]["override_params"];
    for (asker, handle) in [
        ("talky", "grant:example-provider-primary@member-alex/talky"),
        ("cogny", "grant:example-provider-primary@member-alex/cogny"),
    ] {
        let slot = &overrides[format!("{asker}/brain")];
        // The empty key is the SWITCH: a brain asks for a credential only while
        // it holds none, and both brain templates ship a non-empty
        // `api_key`. Set the grant beside one and the four v-lanes carry
        // nothing — quietly, because a model that answers looks like a model
        // that answers.
        assert_eq!(
            slot["api_key"],
            json!(""),
            "{asker}: the grant without the empty key is an inert lane"
        );
        assert_eq!(slot["credential_grant_id"], json!(handle));
    }
    // and what the wish itself set is still there
    assert_eq!(overrides["cogny/brain"]["temperature"], json!(0.2));
}

#[test]
fn the_grants_travel_in_the_same_declaration_as_the_lanes() {
    let decls = grown_with_credential();
    let rows = decls[0]["diff"]["seed_rows"]
        .as_array()
        .expect("the credential declaration seeds the grants");
    let tables: Vec<&str> = rows
        .iter()
        .map(|r| r["table"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        tables,
        vec!["grants", "grant_events"],
        "a grant and the event that says it was granted are one act"
    );
    for row in rows {
        assert_eq!(row["target"], json!("./access/store"));
        assert_eq!(
            row["rows"].as_array().expect("rows").len(),
            2,
            "ONE grant per consumer: the answer edge is addressed by \
             `hop.grant_id`, and two brains sharing a handle would each be \
             handed the other's sealed box"
        );
    }
    // The handle in the row and the handle on the edge are the same string —
    // written once by the renderer, which is what makes building it rather than
    // asking for it honest.
    let first = &rows[0]["rows"][0];
    assert_eq!(
        first["grant_id"],
        json!("grant:example-provider-primary@member-alex/talky")
    );
    assert_eq!(first["cred_ref"], json!("cred:example-provider:primary"));
    assert_eq!(first["subject"], json!("member:alex"));
    assert_eq!(first["requester"], json!("agent:scribe/talky"));
    // The horizon is the wish's, never the renderer's.
    assert_eq!(first["expires_at"], json!("2099-01-01T00:00:00.000000Z"));
}

#[test]
fn a_generation_grown_without_the_block_is_the_one_the_level_always_grew() {
    let mut plain = credentialled_wish();
    plain["credential"] = Value::Null;
    let out = run_recipes(json!({"recipe": "grow_level", "request": "…", "params": plain}));
    let decls = out["manifest"].as_array().expect("manifest");
    assert_eq!(
        decls.len(),
        1,
        "the credential lanes are OPT-IN: writing a person's provider key into \
         an agent's reach is not something a level does to every generation \
         grown from it without being asked"
    );
    assert!(
        decls[0]["diff"]["seed_rows"].is_null(),
        "nothing is seeded for a generation nobody wired that way"
    );
    let overrides = &decls[0]["diff"]["add_nodes"][0]["override_params"];
    assert!(
        overrides["talky/brain"].is_null(),
        "no key is taken away from a generation that was not asked about"
    );
}
