//! GH #478 — two members in one organisation are two ADDRESSES, not one
//! broadcast.
//!
//! `grow_level` renders the transit edge set of a composition level from a
//! table. For a CONTAINER level — an org, a member — that table wrote the six
//! down-edges guarded on the lane alone:
//!
//! ```text
//! ./members -> ./members/editor      has(hop.route) && hop.route == 'in_turn'
//! ```
//!
//! With one child per container that reads like an address. With two it is a
//! broadcast: edges fan out, so one `in_turn` at the container is two
//! deliveries — two inferences, two costs, and the turn in the memory of a
//! member it has nothing to do with. An `in_export` addressed at one member
//! exports all of them; an `in_build_result` reaches all of them.
//!
//! Nothing was red, for the same reason nothing was red in GH #470: the byte
//! pin (`gh466_grow_level_renders_the_level.rs`) compares the renderer against
//! examples generated out of its own table, and every colony grown so far had
//! exactly ONE member per organisation, where an unguarded lane and a correctly
//! addressed one are indistinguishable.
//!
//! One level further down the SAME renderer writes the discriminator correctly
//! — `context.assistant` on the two turn doors and the two transfer doors, and
//! the permissive `!has(...) || ... == name` on `in_build_result`. The address
//! rule of `templates/builder/README.md` is written for that level and holds
//! one storey up unchanged: `Edge.to` is a static path, so N children cost N
//! edges and each one has to say which child it is for.
//!
//! The guard is PERMISSIVE on purpose. Nothing in a grown topology promotes
//! `context.member` or `context.org` today, so a strict `has(...) && ... ==
//! name` would strand every existing colony's turns at the container. Without
//! the key the behaviour is exactly what it was; with it, the turn reaches one
//! child. Both halves are asserted below, against the real edge evaluator.

use meclaw_colony::cel_eval::{evaluate_condition, parse_condition};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_testing::{emit_all, shipped_script};

const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);

/// The `add_edges` of one rendered level, as the shipped script produces them.
fn rendered_edges(params: Value) -> Vec<Value> {
    // The FIRST manifest. Since GH #543 a member wish renders two — the level,
    // and then the screen and the app that member always gets — and what this
    // file is about is the level.
    let out = emit_all(
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
    )
    .into_iter()
    .find(|m| m["header"]["operation"] == json!("recipe"))
    .expect("a first manifest");
    out["manifest"][0]["diff"]["add_edges"]
        .as_array()
        .unwrap_or_else(|| panic!("no rendered edges: {out}"))
        .clone()
}

fn member(name: &str) -> Vec<Value> {
    rendered_edges(json!({"scope": "/os/orgs/newsroom", "level": "member",
                          "name": name, "template": "a-template@1.0.0"}))
}

fn org(name: &str) -> Vec<Value> {
    rendered_edges(json!({"scope": "/os", "level": "org",
                          "name": name, "template": "a-template@1.0.0"}))
}

/// The down-edges of one rendered level: container → child. Those are the ones
/// that have to tell two children apart; the exits all end at the container and
/// carry no address by construction.
fn doors(edges: &[Value]) -> Vec<(String, String)> {
    // The container is `.` since GH #503: a level declares itself AT the hive
    // it grows into, so the down-edges leave the declaration's own scope root.
    edges
        .iter()
        .filter(|e| e["from"] == json!("."))
        .map(|e| {
            (
                e["to"].as_str().unwrap_or_default().to_string(),
                e["condition"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Every lane a container carries down into a child, with the two rendered
/// children that share it. Two children grown into ONE container is exactly the
/// case nobody had built when the table was written.
fn shared_doors(a: &[Value], b: &[Value]) -> Vec<(String, String, String)> {
    let (da, db) = (doors(a), doors(b));
    assert_eq!(
        da.len(),
        db.len(),
        "the two children of one container do not even get the same NUMBER of \
         doors — the table renders per-name, which it must not"
    );
    da.into_iter()
        .zip(db)
        .map(|((to_a, ca), (to_b, cb))| {
            assert_ne!(to_a, to_b, "the fixture grew the same child twice");
            (ca, cb, format!("{to_a} / {to_b}"))
        })
        .collect()
}

#[test]
fn two_members_in_one_organisation_do_not_share_a_condition() {
    let (editor, researcher) = (member("editor"), member("researcher"));
    for (a, b, pair) in shared_doors(&editor, &researcher) {
        assert_ne!(
            a, b,
            "two members of one organisation share the door {pair} under one \
             and the same condition {a:?}: edges fan out, so every message on \
             that lane is delivered TWICE — a turn addressed at one member is \
             answered by both, and its export exports both"
        );
    }
}

#[test]
fn two_organisations_in_one_colony_do_not_share_a_condition() {
    // `_container_level` renders `org` and `member` from ONE table, so the
    // defect is one storey up as well — and there it costs a whole
    // organisation's worth of fan-out per message.
    let (acme, globex) = (org("acme"), org("globex"));
    for (a, b, pair) in shared_doors(&acme, &globex) {
        assert_ne!(
            a, b,
            "two organisations of one colony share the door {pair} under one \
             and the same condition {a:?}"
        );
    }
}

/// Run one described hop through the real edge evaluator against every door of
/// a rendered level, and return the lanes that said yes.
fn accepted(edges: &[Value], context: Value) -> Vec<String> {
    let ctx: Map<String, Value> = context.as_object().expect("a context map").clone();
    doors(edges)
        .into_iter()
        .filter_map(|(_, cond)| {
            let lane = cond
                .split("hop.route == '")
                .nth(1)?
                .split('\'')
                .next()?
                .to_string();
            let mut hop = Map::new();
            hop.insert("route".into(), json!(lane));
            let compiled = parse_condition(&cond).expect("the rendered guard compiles");
            match evaluate_condition(&compiled, &ctx, &hop) {
                Ok(true) => Some(lane),
                _ => None,
            }
        })
        .collect()
}

#[test]
fn the_new_guard_is_permissive_so_a_colony_that_addresses_nobody_is_unchanged() {
    // Nothing in a grown topology promotes `context.member` today. A STRICT
    // guard would therefore stop every turn at the container the day it lands,
    // which is why the permissive form is the one the assistant level already
    // uses for `in_build_result`. Measured through the real evaluator rather
    // than read off the string.
    let editor = member("editor");
    let all: Vec<String> = doors(&editor)
        .into_iter()
        .filter_map(|(_, c)| {
            Some(
                c.split("hop.route == '")
                    .nth(1)?
                    .split('\'')
                    .next()?
                    .to_string(),
            )
        })
        .collect();
    assert_eq!(
        accepted(&editor, json!({})),
        all,
        "a hop that names no member must still reach the member — the guard is \
         additive, and a colony that addresses nobody keeps today's behaviour"
    );
    assert_eq!(
        accepted(&editor, json!({"member": "editor"})),
        all,
        "a hop addressed AT this member must reach it"
    );
    assert!(
        accepted(&editor, json!({"member": "researcher"})).is_empty(),
        "a hop addressed at another member must not reach this one — that is \
         the whole point of the discriminator"
    );
}

#[test]
fn the_public_surfaces_name_the_key_the_renderer_writes() {
    // Development rules § 2d: a behavioural promise on a public template
    // surface gets a drift lock that greps the sentence AND asserts the
    // mechanism. Grepping alone pins a string; asserting alone lets the prose
    // walk away from it. Both halves are here, and the key is read OUT of the
    // rendered guard rather than typed twice.
    let editor = member("editor");
    let acme = org("acme");
    let key = |edges: &[Value]| -> String {
        let (_, cond) = doors(edges).remove(0);
        cond.split("!has(context.")
            .nth(1)
            .and_then(|t| t.split(')').next())
            .unwrap_or_else(|| panic!("no permissive discriminator in {cond:?}"))
            .to_string()
    };
    let (member_key, org_key) = (key(&editor), key(&acme));
    assert_eq!((member_key.as_str(), org_key.as_str()), ("member", "org"));

    for rel in [
        "templates/builder/README.md",
        "templates/builder/template.json",
        "templates/README.md",
    ] {
        let Ok(text) = std::fs::read_to_string(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../")
                .join(rel),
        ) else {
            continue; // R2b: a tree without the surface cannot make the claim
        };
        for k in [&member_key, &org_key] {
            assert!(
                text.contains(&format!("context.{k}")),
                "{rel} does not name context.{k}, which is the key the renderer \
                 writes onto every door of a container level — a promise the \
                 prose no longer carries is one no reader can check"
            );
        }
    }
}
