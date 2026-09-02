//! GH #503 — a grown level declares itself AT the container it grows into, and
//! that is what the broker judges.
//!
//! `grow_level` rendered every level one storey too high: the declaration's
//! scope was the container's PARENT and the container travelled inside the node
//! name (`{"scope": "/os", "add_nodes": [{"name": "orgs/acme"}]}`,
//! `./orgs -> ./orgs/acme`). For every level but one that is invisible — a
//! member is grown at `/os/orgs/<org>`, an assistant at
//! `…/members/<member>`, and both lie under the prefix the shipped broker rule
//! allows. The FIRST organisation of a colony does not: its scope root is `/os`,
//! and `templates/access/store/seed/policy.jsonl`'s `colony.mutate.default`
//! carries `scope_match.scope_prefix: "/os/orgs"`. Measured on a colony grown
//! from `examples/meclaw-os/seed-ref`, one wish, the fast lane: the front door
//! answered `requester_not_permitted` and the mutation log stayed empty, while
//! the very same manifest applied by an operator committed. The manifest was
//! right; only its scope root was out of reach — so every colony's first
//! organisation was an operator act, which is the one step of a two-stage build
//! that cannot be drawn.
//!
//! Since GH #487 `.` resolves to the declaration's own scope inside
//! `add_edges`, so the narrow spelling is drawable at all. What this file pins:
//!
//!   * every level renders scope = `<wish scope>/<container>`, a BARE node name
//!     and `.`-relative edges;
//!   * the ABSOLUTE edges are unchanged by the move — the same two paths as the
//!     wide form resolved to, so nothing in a grown topology differs;
//!   * the first organisation's scope root now lies under the prefix the
//!     shipped policy row grants, and the old one did not;
//!   * the exception is ONE LEVEL and TWO SWITCHES: an assistant grown with
//!     `subscribe` (the identity door) or with `credential` (the credential
//!     road, GH #560) still renders the wide form, because both reach a SIBLING
//!     of the container — `./affinity` and `./access` — that no declaration
//!     standing inside it can name, and an edge lives in the graph of the
//!     lowest common ancestor of its endpoints;
//!   * and the README sentence that promises all of this (development rules
//!     § 2d: the drift lock greps the sentence AND asserts the mechanism).

use meclaw_core::Path;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_one, shipped_script};
use std::path::PathBuf;

const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .join(rel)
}

fn grow(params: Value) -> Value {
    let out = emit_one(
        &shipped_script(RECIPES),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "grow_level", "request": "…",
                                         "params": params}).to_string()}],
        }),
    );
    let decls = out["manifest"]
        .as_array()
        .unwrap_or_else(|| panic!("no manifest: {out}"));
    assert_eq!(decls.len(), 1, "a level is ONE declaration");
    decls[0].clone()
}

const MEMBER: &str = "/os/orgs/acme/members/alex";

/// The six levels, with the scope the WISH names and the container each one is
/// grown into. The container is the recipe's own table, read here as an
/// address: it is the storey the declaration now stands on.
fn levels() -> Vec<(&'static str, &'static str, &'static str, Value)> {
    vec![
        ("org", "/os", "orgs", json!({"name": "acme"})),
        (
            "member",
            "/os/orgs/acme",
            "members",
            json!({"name": "alex"}),
        ),
        ("assistant", MEMBER, "assistants", json!({"name": "scribe"})),
        (
            "channel",
            MEMBER,
            "channels",
            // GH #517 -- the person the channel's round names, from the wish
            json!({"name": "telegram", "assistant": "scribe",
                   "ctx": {"member_person": "alex"}}),
        ),
        (
            "screen",
            MEMBER,
            "channels",
            json!({"name": "display-desk"}),
        ),
        (
            "app",
            MEMBER,
            "apps",
            json!({"name": "colony-view", "screen": "display-desk"}),
        ),
    ]
}

fn rendered(level: &str, scope: &str, extra: &Value) -> Value {
    let mut params = json!({"scope": scope, "level": level,
                            "template": "a-template@1.0.0"});
    for (k, v) in extra.as_object().expect("params") {
        params[k] = v.clone();
    }
    grow(params)
}

#[test]
fn every_level_declares_itself_at_the_container_it_grows_into() {
    for (level, scope, container, extra) in levels() {
        let decl = rendered(level, scope, &extra);
        let name = extra["name"].as_str().expect("a name");

        assert_eq!(
            decl["scope"],
            json!(format!("{}/{container}", scope.trim_end_matches('/'))),
            "{level}: the declaration must stand AT its container — the scope \
             root is what the broker judges, and one storey up asks for more \
             reach than the change needs"
        );
        assert_eq!(
            decl["diff"]["add_nodes"][0]["name"],
            json!(name),
            "{level}: the node is named bare — the container it lands in is the \
             declaration's scope, not part of the name"
        );

        // Every endpoint is one of exactly two spellings: the scope root, or a
        // direct child of it. Anything else would reach outside the container
        // the declaration stands in.
        //
        // GH #562 — with one exception, and it is a DECLARED one: an edge that
        // names a `lane` is a v-lane (ADR-0020), whose whole point is to end
        // INSIDE the child on an occupant the child's own contract names under
        // `at`. It still ends inside the container, so the scope root is
        // unaffected; what it does not do is stop at the child's rim. An edge
        // without `lane` is judged exactly as before.
        let child = format!("./{name}");
        for edge in decl["diff"]["add_edges"].as_array().expect("add_edges") {
            let v_lane = edge.get("lane").and_then(|l| l.as_str());
            for side in ["from", "to"] {
                let raw = edge[side].as_str().expect("an endpoint");
                let deep = v_lane.is_some() && raw.starts_with(&format!("{child}/"));
                assert!(
                    raw == "." || raw == child || deep,
                    "{level}: the endpoint {raw:?} is neither `.` (the \
                     container) nor {child:?} (the child) nor an occupant of \
                     the child on a declared lane — a level draws nothing else"
                );
            }
        }
    }
}

#[test]
fn the_absolute_edges_are_the_ones_the_wide_form_drew() {
    // The whole claim of the change: only the SCOPE ROOT moves. Resolved
    // against their own declaration, the narrow endpoints are the very paths
    // `./<container>` and `./<container>/<name>` resolved to from one storey
    // up, so no grown topology differs by a byte.
    for (level, scope, container, extra) in levels() {
        let decl = rendered(level, scope, &extra);
        let name = extra["name"].as_str().expect("a name");
        let narrow_scope = decl["scope"].as_str().expect("a scope").to_string();
        let wide = Path::new(scope);
        let (want_container, want_child) = (
            Path::resolve(&wide, &format!("./{container}")),
            Path::resolve(&wide, &format!("./{container}/{name}")),
        );

        for edge in decl["diff"]["add_edges"].as_array().expect("add_edges") {
            // A v-lane's deep end is measured the same way, against the wide
            // form's own spelling of it: what must not move is the ABSOLUTE
            // path, and a v-lane's is `<container>/<name>/<occupant>` read from
            // either storey (GH #562).
            let v_lane = edge.get("lane").and_then(|l| l.as_str());
            for side in ["from", "to"] {
                let raw = edge[side].as_str().expect("an endpoint");
                let got = Path::resolve(&Path::new(&narrow_scope), raw);
                let deep = v_lane.is_some()
                    && got
                        .as_str()
                        .starts_with(&format!("{}/", want_child.as_str()))
                    && Path::resolve(
                        &wide,
                        raw.replacen("./", &format!("./{container}/"), 1).as_str(),
                    ) == got;
                assert!(
                    got == want_container || got == want_child || deep,
                    "{level}: {raw:?} resolves to {} — the wide form drew {} \
                     and {}, and the absolute edges must not move",
                    got.as_str(),
                    want_container.as_str(),
                    want_child.as_str()
                );
            }
        }
    }
}

#[test]
fn the_first_organisation_now_asks_for_a_scope_root_the_shipped_policy_grants() {
    // The mechanism half, and the prefix is READ rather than typed: the rule
    // that refused the build is the one asserted against.
    let raw = std::fs::read_to_string(repo("templates/access/store/seed/policy.jsonl"))
        .expect("the shipped policy seed");
    let prefix = raw
        .lines()
        .filter_map(|l| meclaw_core::serde_json::from_str::<Value>(l).ok())
        .find(|r| r["rule_id"] == json!("colony.mutate.default"))
        .and_then(|r| {
            r["scope_match"]["scope_prefix"]
                .as_str()
                .map(str::to_string)
        })
        .expect("colony.mutate.default carries a scope_prefix");

    let under = |path: &str| path == prefix || path.starts_with(&format!("{prefix}/"));

    let org = rendered("org", "/os", &json!({"name": "acme"}));
    let scope = org["scope"].as_str().expect("a scope");
    assert!(
        under(scope),
        "the first organisation of a colony is declared at {scope}, which the \
         shipped `colony.mutate.default` rule ({prefix}) does not permit — the \
         front door answers `requester_not_permitted` and the one build every \
         colony starts with is an operator act again"
    );
    assert!(
        !under("/os"),
        "the fixture no longer measures anything: `/os` — the scope root the \
         wide form asked for — is inside the prefix, so this test would pass \
         with the defect back in"
    );
}

#[test]
fn the_identity_door_keeps_the_wide_declaration_and_says_so() {
    // The first of the two switches that keep the wide form, and it is a
    // reachability fact. `./affinity` is a SIBLING of `./assistants`: from a
    // declaration standing in the container the only spellings that reach it
    // are `../affinity` and an absolute path, and `mutation::validate` refuses
    // both in an edge endpoint. Splitting the door into a second declaration
    // does not survive `templates/submit/gate` either — its form check accepts
    // an `in_pack` edge only when the target is under the requester or is
    // created by THAT SAME declaration (GH #479), and a declaration that only
    // draws edges creates nothing.
    //
    // The second switch is `credential` (GH #560), and since `builder@1.6.1`
    // it takes the wide form for the same reason: the road ends inside the
    // newborn generation and its other end is the member's own `./access`, a
    // sibling of `./assistants` — so the member is the lowest common ancestor,
    // and an edge lives in the graph of that level. Pinned in
    // `gh466_grow_level_renders_the_level.rs` and, at the mutation door, in
    // `gh567_the_credentialled_wish_is_one_act.rs` (GH #567).
    let mut params = json!({"scope": MEMBER, "level": "assistant", "name": "scribe",
                            "template": "a-template@1.0.0"});
    params["subscribe"] = json!(true);
    let decl = grow(params);

    assert_eq!(
        decl["scope"],
        json!(MEMBER),
        "a subscribe wish renders the declaration at the MEMBER, because that is \
         the only scope from which both `./affinity` and the new generation are \
         nameable"
    );
    assert_eq!(
        decl["diff"]["add_nodes"][0]["name"],
        json!("assistants/scribe"),
        "and the node carries its container again, because the edges around it do"
    );

    // The gate's rule, mirrored: the `in_pack` edge ends at a node this very
    // declaration creates -- or, since GH #561, inside one, because the pack
    // rides a v-lane to a brain RIM of the generation. That is what makes the
    // wide form submittable and the split one not.
    let scope = Path::new(decl["scope"].as_str().expect("a scope"));
    let creates: Vec<String> = decl["diff"]["add_nodes"]
        .as_array()
        .expect("add_nodes")
        .iter()
        .map(|n| {
            Path::resolve(&scope, n["name"].as_str().expect("a name"))
                .as_str()
                .to_string()
        })
        .collect();
    let door = decl["diff"]["add_edges"]
        .as_array()
        .expect("add_edges")
        .iter()
        .find(|e| e["modifier"]["set_hop"]["route"] == json!("'in_pack'"))
        .expect("the identity door is drawn on request");
    assert_eq!(
        door["from"],
        json!("./affinity"),
        "the push leaves the affinity hive"
    );
    let target = Path::resolve(&scope, door["to"].as_str().expect("a target"))
        .as_str()
        .to_string();
    assert!(
        creates
            .iter()
            .any(|c| &target == c || target.starts_with(&format!("{c}/"))),
        "the `in_pack` edge ends at {target}, which is neither a node this \
         declaration creates nor anything inside one — `templates/submit/gate` \
         refuses that with `subscribe_target_not_self`, and no version of this \
         recipe may render it"
    );
}

#[test]
fn the_readme_publishes_the_form_the_renderer_writes() {
    // Development rules § 2d: a behavioural promise on a public template
    // surface gets a drift lock in the same commit — one test that greps the
    // sentence AND asserts the mechanism. The mechanism is the three tests
    // above; this is the sentence, and the containers in it are read out of
    // what the renderer produced rather than typed twice.
    let Ok(readme) = std::fs::read_to_string(repo("templates/builder/README.md")) else {
        return; // R2b: a tree without the template cannot make this assertion
    };
    let flat = readme.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flat.contains("declares itself at the container it grows into"),
        "the README no longer says where a level declares itself — the form is \
         a promise on a public surface and it is published exactly once"
    );
    for (level, scope, _container, extra) in levels() {
        let decl = rendered(level, scope, &extra);
        let rendered_scope = decl["scope"].as_str().expect("a scope").to_string();
        // The section spells the scopes with the placeholders a reader has
        // (`<org>`, `<member>`), so what is checked is the trailing container
        // segment of each rendered scope — the part that is the claim.
        let container = rendered_scope.rsplit('/').next().expect("a container");
        assert!(
            flat.contains(&format!("/{container}`")),
            "the README's form section does not name `{container}`, which is \
             where a {level} declares itself"
        );
    }
    assert!(
        flat.contains("keeps the wide form"),
        "the README does not name the exception (the identity door and, since \
         `builder@1.6.1`, the credential road), so a reader meeting either \
         would read it as drift"
    );
}
