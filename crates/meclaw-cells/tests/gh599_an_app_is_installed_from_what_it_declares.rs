//! GH #599 — an app is installed from what it DECLARES, and the builder draws
//! the wiring.
//!
//! Until this recipe an app was installed by a manifest somebody wrote by hand:
//! the member README published one, every app repository carried its own, and
//! the live colony was repaired twice where the two had drifted apart (the
//! observer edges lost their channel guard, `withdraw` joined the view edge,
//! every talky v-lane grew a `talky-chat` twin, and a device road appeared that
//! the README never had). The fifth builder recipe, `install_app`, renders that
//! wiring from ONE block the app's template carries — `template.json` → `app` —
//! and the wish that asks for it hands the block over verbatim, because the
//! recipe reads nothing but its stdin (no model, no network, no disk).
//!
//! What this file holds:
//!
//! 1. the shipped app (`colony-view`) renders exactly what `grow_level
//!    level=app` renders — the member's own screen app is the same act;
//! 2. four declarations render the edge sets a live colony carries for those
//!    four apps, compared as sets of `(from, to, lane, condition, modifier)` —
//!    the identity the edge table itself uses;
//! 3. a word outside the closed vocabulary is refused by name, with `known`;
//! 4. a declaration that offers something needs the generation it offers to;
//! 5. the install block of `templates/member/README.md` is what the recipe
//!    renders, so the README cannot drift from the mechanism (§ 2d).
//!
//! Pure script tests: the SHIPPED `classify` and `recipes` are run over stdin,
//! nothing is booted.

use meclaw_core::serde_json::{self, Value, json};
use meclaw_testing::{emit_all, emit_one, shipped_script};
use std::collections::BTreeSet;
use std::path::PathBuf;

const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);
const CLASSIFY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/classify/config.json"
);
const LIVE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/gh599_live_install_edges.json"
);

/// The member every wish in this file installs into.
const MEMBER: &str = "/os/orgs/acme/members/alex";

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .join(rel)
}

fn read_json(path: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Every emission of the renderer for one recipe payload.
fn run_recipes(recipe: &str, params: &Value) -> Vec<Value> {
    emit_all(
        &shipped_script(RECIPES),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": recipe, "request": "…",
                                         "params": params}).to_string()}],
        }),
    )
}

/// The one manifest a successful render carries.
fn manifest(recipe: &str, params: &Value) -> Vec<Value> {
    let all = run_recipes(recipe, params);
    let first = all
        .first()
        .unwrap_or_else(|| panic!("the renderer emitted nothing for {recipe}"));
    assert!(
        first["header"]["error_code"].is_null(),
        "`{recipe}` was refused: {first}"
    );
    first["manifest"]
        .as_array()
        .unwrap_or_else(|| panic!("no manifest on the render of {recipe}: {first}"))
        .clone()
}

/// The refusal of a render, as `(error_code, payload)`.
fn refusal(recipe: &str, params: &Value) -> (String, Value) {
    let all = run_recipes(recipe, params);
    let first = all.first().expect("an emission");
    let code = first["header"]["error_code"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a refusal, got a render: {first}"))
        .to_string();
    let payload: Value = serde_json::from_str(
        first["messages"][0]["text"]
            .as_str()
            .expect("the refusal carries a payload"),
    )
    .expect("the refusal payload is json");
    (code, payload)
}

/// An edge endpoint made absolute against the scope of its declaration.
fn absolute(scope: &str, rel: &str) -> String {
    let base = scope.trim_end_matches('/');
    match rel {
        "." | "./" => base.to_string(),
        r if r.starts_with("./") => format!("{base}/{}", &r[2..]),
        r => r.to_string(),
    }
}

/// The identity of an edge, as the edge table holds it: endpoints, lane,
/// condition, modifier (canonical — `serde_json` keeps object keys sorted).
fn norm(scope: &str, e: &Value) -> String {
    json!([
        absolute(scope, e["from"].as_str().unwrap_or_default()),
        absolute(scope, e["to"].as_str().unwrap_or_default()),
        e.get("lane").cloned().unwrap_or(Value::Null),
        e["condition"].as_str().unwrap_or_default(),
        e.get("modifier").cloned().unwrap_or(Value::Null),
        e.get("default").cloned().unwrap_or(Value::Null),
    ])
    .to_string()
}

fn edge_set(manifest: &[Value]) -> (BTreeSet<String>, usize) {
    let mut set = BTreeSet::new();
    let mut n = 0;
    for decl in manifest {
        let scope = decl["scope"].as_str().expect("a declaration has a scope");
        for e in decl["diff"]["add_edges"].as_array().into_iter().flatten() {
            set.insert(norm(scope, e));
            n += 1;
        }
    }
    (set, n)
}

fn node_set(manifest: &[Value]) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    for decl in manifest {
        let scope = decl["scope"].as_str().expect("a declaration has a scope");
        for n in decl["diff"]["add_nodes"].as_array().into_iter().flatten() {
            set.insert(format!(
                "{}={}",
                absolute(
                    scope,
                    &format!("./{}", n["name"].as_str().unwrap_or_default())
                ),
                n["template"].as_str().unwrap_or_default()
            ));
        }
    }
    set
}

/// The `app` block the shipped colony-view template carries.
fn colony_view() -> (String, Value) {
    let tpl = read_json(&repo("templates/colony-view/template.json"));
    let version = format!(
        "colony-view@{}",
        tpl["version"].as_str().expect("a template has a version")
    );
    let app = tpl["app"].clone();
    assert!(
        app.is_object(),
        "templates/colony-view/template.json carries no `app` declaration — the \
         installer has nothing to read: {tpl}"
    );
    (version, app)
}

fn install_params(name: &str, entry: &Value, declaration: &Value) -> Value {
    let mut p = json!({
        "scope": MEMBER,
        "app": name,
        "template": entry["template"],
        "screen": entry["screen"],
        "declaration": declaration,
    });
    if let Some(g) = entry.get("generation") {
        p["generation"] = g.clone();
    }
    p
}

// ════════════════════════════════════════════════════════════════ the locks

/// **The shipped app renders its own level.** `colony-view` is the app every
/// member gets with its screen, grown by `grow_level level=app` inside a member
/// wish. Installed from its declaration it has to be the SAME wiring — the two
/// declarations stand at different scopes (`<member>/apps` and `<member>`), so
/// both are made absolute before they are compared.
#[test]
fn the_colony_view_declaration_renders_the_app_level() {
    let (template, declaration) = colony_view();
    let installed = manifest(
        "install_app",
        &json!({"scope": MEMBER, "app": "colony-view", "template": template,
                "screen": "display", "declaration": declaration}),
    );
    let grown = manifest(
        "grow_level",
        &json!({"scope": MEMBER, "level": "app", "name": "colony-view",
                "template": template, "screen": "display"}),
    );
    let (inst, n_inst) = edge_set(&installed);
    let (grow, n_grow) = edge_set(&grown);
    assert_eq!(
        inst, grow,
        "the declaration of colony-view renders a different wiring than the level \
         a member wish grows it with.\ninstall_app: {installed:#?}\ngrow_level: {grown:#?}"
    );
    assert_eq!(n_inst, n_grow, "and no edge twice");
    assert_eq!(
        node_set(&installed),
        node_set(&grown),
        "and the same node, under the same template"
    );
}

/// **Four declarations, four live wirings.** Each app's declaration renders
/// the set of edges a live colony carries for it — no edge missing, none extra,
/// none twice. The expected set is the fixture, which records where each list came
/// from; the counts are its lengths and stand nowhere else.
#[test]
fn each_attachment_kind_renders_the_edges_the_live_colony_carries() {
    let live = read_json(&PathBuf::from(LIVE));
    let apps = live.as_object().expect("the fixture is an object");
    let builder_readme = std::fs::read_to_string(repo("templates/builder/README.md"))
        .expect("templates/builder/README.md");
    let mut seen = 0;
    for (name, entry) in apps.iter().filter(|(k, _)| !k.starts_with('_')) {
        let declaration = if name == "colony-view" {
            colony_view().1
        } else {
            entry["declaration"].clone()
        };
        let rendered = manifest("install_app", &install_params(name, entry, &declaration));
        assert_eq!(rendered.len(), 1, "one declaration per app: {rendered:#?}");
        assert_eq!(
            rendered[0]["scope"],
            json!(MEMBER),
            "an app is installed at its member: {rendered:#?}"
        );
        let (got, n_got) = edge_set(&rendered);
        let expected: Vec<Value> = entry["edges"].as_array().expect("edges").clone();
        let want: BTreeSet<String> = expected.iter().map(|e| norm(MEMBER, e)).collect();
        let missing: Vec<&String> = want.difference(&got).collect();
        let extra: Vec<&String> = got.difference(&want).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "`{name}`: the rendered wiring is not the live one.\nmissing: {missing:#?}\n\
             extra: {extra:#?}"
        );
        assert_eq!(
            n_got,
            expected.len(),
            "`{name}`: {n_got} edges rendered for {} live ones — an edge drawn twice",
            expected.len()
        );
        // § 2d: the builder README publishes these counts once, and they are
        // derived here rather than repeated.
        let row = format!("| `{name}` | {n_got} |");
        assert!(
            builder_readme.contains(&row),
            "templates/builder/README.md § An app is a declaration does not publish \
             `{row}` — the count moved and the prose did not"
        );
        assert_eq!(
            node_set(&rendered),
            BTreeSet::from([format!(
                "{MEMBER}/apps/{name}={}",
                entry["template"].as_str().unwrap_or_default()
            )]),
            "`{name}`: one node, `apps/<app>`, from the named template"
        );
        seen += 1;
    }
    assert_eq!(
        seen, 4,
        "the fixture lost an app — a lock over nothing is green"
    );
}

/// **A word outside the vocabulary is refused by name.** The vocabulary of a
/// declaration is closed: a lane the recipe has no rule for would otherwise be
/// dropped without a sound, and an app that listens to `gossip` would install
/// green and hear nothing.
#[test]
fn an_unknown_word_in_a_declaration_is_refused_by_name() {
    let base = json!({"scope": MEMBER, "app": "showcase", "template": "showcase@1.0.0",
                      "screen": "display", "generation": "sam"});
    let cases = [
        (
            json!({"screen": {"out": ["view"], "back": ["event", "receipt"]},
                   "listens": ["turn", "gossip"]}),
            "listens",
            "mutation_committed",
        ),
        (
            json!({"screen": {"out": ["view", "shout"], "back": ["event", "receipt"]}}),
            "screen.out",
            "withdraw",
        ),
        (
            json!({"screen": {"out": ["view"], "back": ["event", "receipt"]},
                   "offers": [{"kind": "widget", "at": "./show"}]}),
            "offers[0].kind",
            "sidecar",
        ),
        (
            json!({"screen": {"out": ["view"], "back": ["event", "receipt"]},
                   "listens": [], "hears": ["turn"]}),
            "declaration",
            "listens",
        ),
    ];
    for (declaration, field, one_known) in cases {
        let mut p = base.clone();
        p["declaration"] = declaration.clone();
        let (code, payload) = refusal("install_app", &p);
        assert_eq!(
            code, "app_declaration_invalid",
            "{declaration}: refused under the wrong code: {payload}"
        );
        assert_eq!(payload["field"], json!(field), "{declaration}: {payload}");
        assert!(
            payload["known"]
                .as_array()
                .is_some_and(|k| k.contains(&json!(one_known))),
            "{declaration}: the refusal has to name what IS known: {payload}"
        );
        assert!(
            payload["reason"].as_str().is_some_and(|r| !r.is_empty()),
            "{declaration}: and say why: {payload}"
        );
    }
}

/// **An offer needs the generation it is offered to.** A tool is a v-lane from
/// one generation's surface; a declaration that offers one cannot be rendered
/// without the generation's name, and the switch says so before the renderer
/// is reached — while a declaration that offers nothing does not need one.
#[test]
fn a_declaration_that_offers_needs_a_generation() {
    let classify = |params: Value| {
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
    };
    let offers = json!({"screen": {"out": ["view"], "back": ["event", "receipt"]},
                        "offers": [{"kind": "tool", "at": "./timer", "tools": ["set_timer"]}]});
    let base = json!({"scope": MEMBER, "app": "ambient", "template": "ambient@0.6.3",
                      "screen": "display", "declaration": offers});

    let out = classify(base.clone());
    assert_eq!(
        out["header"]["error_code"],
        json!("recipe_params_incomplete"),
        "an offer without a generation reached the renderer: {out}"
    );
    let payload: Value =
        serde_json::from_str(out["messages"][0]["text"].as_str().expect("a payload"))
            .expect("json");
    assert_eq!(payload["missing"], json!(["generation"]), "{payload}");

    let mut with = base.clone();
    with["generation"] = json!("sam");
    let out = classify(with);
    assert_eq!(out["header"]["route"], json!("recipe"), "{out}");
    assert_eq!(out["header"]["recipe"], json!("install_app"), "{out}");

    let quiet = json!({"scope": MEMBER, "app": "chat", "template": "chat@0.3.3",
                       "screen": "display",
                       "declaration": {"screen": {"out": ["view"], "back": ["event", "receipt"]},
                                       "listens": ["turn"]}});
    let out = classify(quiet);
    assert_eq!(
        out["header"]["route"],
        json!("recipe"),
        "an app that offers nothing needs no generation: {out}"
    );

    let mut missing = base.clone();
    missing
        .as_object_mut()
        .expect("an object")
        .remove("declaration");
    missing["generation"] = json!("sam");
    let out = classify(missing);
    assert_eq!(
        out["header"]["error_code"],
        json!("recipe_params_incomplete")
    );
}

/// **An offer is closed like every other block of the declaration** (T4 review
/// minor 1). A key the offer's kind has no rule for — `sections` beside a tool
/// offer, a misspelt `tool` — would otherwise be dropped without a sound, and
/// the app would install green with an offer that says less than it was
/// written to say.
#[test]
fn an_offer_with_a_key_its_kind_has_no_rule_for_is_refused_by_name() {
    let base = json!({"scope": MEMBER, "app": "showcase", "template": "showcase@1.0.0",
                      "screen": "display", "generation": "sam"});
    let cases = [
        (
            json!({"kind": "tool", "at": "./show", "tools": ["x"], "section": "y"}),
            "tools",
        ),
        (
            json!({"kind": "sidecar", "at": "./show", "section": "y", "tool": ["x"]}),
            "section",
        ),
    ];
    for (offer, one_known) in cases {
        let mut p = base.clone();
        p["declaration"] = json!({"screen": {"out": ["view"], "back": ["event", "receipt"]},
                                  "offers": [offer.clone()]});
        let (code, payload) = refusal("install_app", &p);
        assert_eq!(code, "app_declaration_invalid", "{offer}: {payload}");
        assert_eq!(payload["field"], json!("offers[0]"), "{offer}: {payload}");
        assert!(
            payload["known"]
                .as_array()
                .is_some_and(|k| k.contains(&json!(one_known)) && k.contains(&json!("at"))),
            "{offer}: the refusal names the keys that kind takes: {payload}"
        );
    }
}

/// **The switch and the renderer name the same five declaration keys** (T4
/// review minor 2). `classify` refuses a declaration that is no object and
/// lists the keys a declaration may carry; the renderer refuses a foreign key
/// and lists its own. Two lists of one vocabulary are two answers the day one
/// of them moves — this holds them together.
#[test]
fn the_switch_and_the_renderer_list_the_same_declaration_keys() {
    let mut p = json!({"scope": MEMBER, "app": "showcase", "template": "showcase@1.0.0",
                       "screen": "display", "generation": "sam",
                       "declaration": {"hears": ["turn"]}});
    let (_, renderer) = refusal("install_app", &p);
    p["declaration"] = json!("the app block");
    let out = emit_one(
        &shipped_script(CLASSIFY),
        &json!({
            "target": "/os/builder/classify",
            "header": {"hop": {"route": "in_build"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_call", "id": "c1",
                          "text": json!({"request": "install an app",
                                         "recipe": "install_app",
                                         "params": p}).to_string()}],
        }),
    );
    let switch: Value =
        serde_json::from_str(out["messages"][0]["text"].as_str().expect("a payload"))
            .expect("json");
    let sorted = |v: &Value| {
        let mut k: Vec<String> = v["known"]
            .as_array()
            .unwrap_or_else(|| panic!("no known list: {v}"))
            .iter()
            .map(|x| x.as_str().expect("a key").to_string())
            .collect();
        k.sort();
        k
    };
    assert_eq!(
        sorted(&switch),
        sorted(&renderer),
        "switch {switch} vs renderer {renderer}"
    );
}

/// The fenced ```json blocks of `templates/member/README.md` § *Installing an
/// app*, in order.
fn readme_blocks() -> Vec<String> {
    let text = std::fs::read_to_string(repo("templates/member/README.md"))
        .expect("templates/member/README.md");
    let start = text
        .find("\n### Installing an app\n")
        .expect("the README has lost its § Installing an app");
    let rest = &text[start + 1..];
    let end = rest[4..].find("\n### ").map_or(rest.len(), |i| i + 4);
    let section = &rest[..end];
    let mut blocks = Vec::new();
    let mut body = section;
    while let Some(open) = body.find("```json\n") {
        let after = &body[open + 8..];
        let close = after.find("\n```").expect("a fenced block closes");
        blocks.push(after[..close].to_string());
        body = &after[close + 4..];
    }
    blocks
}

/// **The README's install block is what the recipe renders.** The section
/// carries the WISH (a `build_topology` call naming `install_app`) and the
/// declaration it renders; with the placeholders filled in, rendering the one
/// has to produce the other, edge for edge and in order.
#[test]
fn the_readme_install_block_is_what_the_recipe_renders() {
    let fill = |s: &str| {
        s.replace("<member>", MEMBER)
            .replace("<app>", "showcase")
            .replace("<version>", "1.0.0")
            .replace("<gen>", "sam")
            .replace("<screen>", "display")
            .replace("<section>", "display")
            .replace("<device>", "browser")
    };
    let blocks = readme_blocks();
    assert_eq!(
        blocks.len(),
        2,
        "§ Installing an app carries exactly two json blocks — the wish and what it \
         renders: {blocks:#?}"
    );
    let wish: Value = serde_json::from_str(&fill(&blocks[0]))
        .unwrap_or_else(|e| panic!("the wish block is not json ({e}): {}", blocks[0]));
    let shown: Value = serde_json::from_str(&fill(&blocks[1]))
        .unwrap_or_else(|e| panic!("the rendered block is not json ({e}): {}", blocks[1]));
    assert_eq!(wish["recipe"], json!("install_app"), "{wish}");
    let rendered = manifest("install_app", &wish["params"]);
    assert_eq!(
        json!(rendered),
        json!([shown]),
        "the README shows a wiring the recipe does not render — one of them moved. \
         Render the wish and paste the result, with the placeholders put back."
    );
    let (set, n) = edge_set(&rendered);
    assert_eq!(set.len(), n, "the README example draws an edge twice");
}
