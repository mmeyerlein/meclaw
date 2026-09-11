//! GH #543 — a member grows a screen and an app, always, and the OS hands out
//! the mount.
//!
//! WHAT THIS FILE IS
//! =================
//! A person in this substrate is the thing that has input and output devices.
//! Until now the fast lane grew the person and stopped: the screen and the
//! application that draws on it were a second, hand-written act, and every
//! colony that ever wanted one wrote the same two declarations again. Ruling
//! R-0904-4 closes that: **every member gets a screen and an app, always**, the
//! wish is not asked and cannot refuse, and the templates and the mount pattern
//! are the builder's own configuration rather than anything the wish says.
//!
//! Three claims are measured here, and each one is measured positively:
//!
//! 1. **One manifest, in order.** One member wish leaves `recipes` as ONE
//!    `manifest` emission — the member first, its screen and app behind it. The
//!    first declaration is byte-unchanged
//!    (`examples/organism/grow-member.json`), the two behind it are
//!    `examples/organism/grow-screen.json`. That it is one emission and not two
//!    is GH #585: two submissions in the same turn have no order at the front,
//!    and the order is semantics.
//! 2. **The mount is HANDED OUT, never wished for.** `screen_mount` with
//!    `{member}` filled in, written on the screen's own node — since GH #655 a
//!    surface cell has no port and is reached at `/<mount>/` on the colony's one
//!    listener. Since GH #663 nothing is measured off the tree for it at all: a
//!    member's name is unique inside its organisation by construction, so the
//!    count that used to hand out the port was taken and spent on nothing.
//! 3. **The roll-forward holds.** The screen draws into `<member>/channels`, a
//!    scope only the declaration in front of it creates, and a manifest rolls
//!    forward with no rollback — so the order is not a preference: submitted on
//!    its own, before the person stands, the screen is refused, and that is
//!    asserted rather than assumed. Since GH #585 the order lives INSIDE one
//!    submission, where the door keeps it; the file that measures the wish
//!    against the front is
//!    `crates/meclaw-cells/tests/gh585_a_member_wish_is_one_submission.rs`.
//!
//! `the_os_hands_out_the_mount` is the ADR anchor of
//! `plans/adr/0022-the-os-hands-out-what-is-system-near.md`: a colony carries
//! many organisations and ONE OS, and the OS is what allocates the system-near
//! things. The allocation in the builder is the first form of that
//! responsibility — the builder is part of the OS — and never an org's own
//! right.

use meclaw_colony::edge_table::{Edge, EdgeTable, apply_edges};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, ManifestOutcome, MutationDoorOutcome,
    MutationOutcome, RespawnFn, SpawnedCellKind, WakeFn, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Headers, JsonValue, Message, Path, Uuid};
use meclaw_testing::{ColonyHandle, emit_all, shipped_script};
use std::collections::BTreeSet;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

const RECIPES: &str = "templates/builder/recipes/config.json";
const CLASSIFY: &str = "templates/builder/classify/config.json";

/// The organisation the examples are written for, and the member they grow.
const ORG: &str = "/os/orgs/acme";
const MEMBER: &str = "alex";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Every level template this example instantiates, or nothing (GH #49).
fn shipped() -> bool {
    [
        RECIPES,
        CLASSIFY,
        "examples/organism/grow-member.json",
        "examples/organism/grow-screen.json",
        "examples/organism/grow-os.json",
        "examples/organism/grow-org.json",
    ]
    .iter()
    .all(|f| repo(f).is_file())
}

fn library_is_complete() -> bool {
    ["meclaw-os", "org", "member", "display", "colony-view"]
        .iter()
        .all(|n| repo(&format!("templates/{n}/template.json")).is_file())
}

// ──────────────────────────────────────────────────────────────────────────────
// the renderer
// ──────────────────────────────────────────────────────────────────────────────

/// Every emission of `recipes` for one wish.
fn run_recipes(payload: Value) -> Vec<Value> {
    emit_all(
        &shipped_script(repo(RECIPES).to_str().expect("utf-8 path")),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": payload.to_string()}],
        }),
    )
}

/// The manifest emissions of one wish, in the order they left the cell. The
/// `bind` leg is not a manifest and is filtered out here rather than counted.
fn manifests(payload: Value) -> Vec<Value> {
    run_recipes(payload)
        .into_iter()
        .filter(|m| m["header"]["operation"] == json!("recipe"))
        .collect()
}

fn member_wish(template: &str) -> Value {
    json!({"recipe": "grow_level", "request": "grow a member named alex",
           "params": {"scope": ORG, "level": "member", "name": MEMBER,
                      "template": template}})
}

/// The template `examples/organism/grow-member.json` currently names, so a
/// version bump of the member level does not make this file red.
fn member_template() -> String {
    read_json(&repo("examples/organism/grow-member.json"))["diff"]["add_nodes"][0]["template"]
        .as_str()
        .expect("the member example names a template")
        .to_string()
}

/// A declaration reduced to the three keys a declaration has, so a file that
/// omits an empty `ctx` and a renderer that writes one compare equal.
fn normalised(d: &Value) -> Value {
    json!({
        "scope": d["scope"],
        "ctx": if d["ctx"].is_object() { d["ctx"].clone() } else { json!({}) },
        "diff": d["diff"],
    })
}

#[test]
fn a_member_wish_renders_the_member_and_then_its_screen() {
    if !shipped() {
        return;
    }
    let out = manifests(member_wish(&member_template()));
    assert_eq!(
        out.len(),
        1,
        "a member wish renders ONE manifest — the member, then the screen and \
         the app it always gets, in that order — and got {} instead (GH #585)",
        out.len()
    );
    assert_eq!(
        out[0]["header"]["declaration_count"],
        json!(3),
        "the person, the screen and the app: three containers, three storeys, \
         three declarations that cannot share a mutation (GH #503)"
    );
    assert_eq!(
        out[0]["manifest"][0]["scope"],
        json!("/os/orgs/acme/members"),
        "declaration 1 declares itself at the container the member grows into"
    );
    assert_eq!(
        out[0]["manifest"][1]["scope"],
        json!("/os/orgs/acme/members/alex/channels"),
        "the screen stands in the member's own channels, a scope only the \
         declaration in front of it creates"
    );
    assert_eq!(
        out[0]["manifest"][2]["scope"],
        json!("/os/orgs/acme/members/alex/apps"),
        "the app stands in the member's own apps"
    );
    assert!(
        out[0]["header"]["manifest_sha256"]
            .as_str()
            .is_some_and(|d| d.len() == 64),
        "one submission is one digest, and it is what the whole wish is refused \
         or committed under: {:?}",
        out[0]["header"]["manifest_sha256"]
    );
}

/// The ADR anchor. `plans/adr/0022-the-os-hands-out-what-is-system-near.md`.
///
/// A name on the colony's one listener is system-near: two screens under one
/// name is one screen and one silent degradation. It is not in the wish and
/// not in the template — the builder, which is part of the OS, fills its own
/// `screen_mount` in with the member's name. What it handed out before was a
/// port, `screen_port_base + <index>`; the display has none to be given any
/// more, and since GH #663 the index it was made of is not measured either.
#[test]
fn the_os_hands_out_the_mount() {
    if !shipped() {
        return;
    }
    let template = member_template();
    let screen = || -> Value {
        manifests(member_wish(&template))[0]["manifest"][1]["diff"]["add_nodes"][0]
            ["override_params"]
            .clone()
    };
    assert_eq!(
        screen()["web"]["mount"],
        json!("alex-display"),
        "the member's own name is what the screen is reached under, and nothing \
         the tree has to be asked for decides it: a member's name is unique \
         inside its organisation by construction, so two screens are two names \
         without arithmetic"
    );
    assert!(
        screen()["web"]["port"].is_null(),
        "a surface cell has no port since GH #655, so the OS hands out none"
    );
}

/// The three values live twice — as the instance's own `params`, which is what
/// ships and what an operator overrides, and as the floor the script falls back
/// to when a config carries none. Two copies of one number drift; this is the
/// only place they are compared.
#[test]
fn the_shipped_configuration_and_the_renderers_floor_agree() {
    if !shipped() {
        return;
    }
    let cfg = read_json(&repo(RECIPES));
    let script = std::fs::read_to_string(repo(RECIPES)).expect("the recipe config");
    for key in [
        "member_screen_template",
        "member_app_template",
        "screen_mount",
    ] {
        let value = &cfg["params"][key];
        assert!(
            !value.is_null(),
            "`{key}` is not a param of the recipe cell, so an instance cannot \
             override it and the OS cannot be told what it hands out"
        );
        let spelled = match value {
            Value::String(s) => format!("\"{s}\""),
            other => other.to_string(),
        };
        assert!(
            script.contains(&spelled),
            "the shipped `params.{key}` ({spelled}) is not the floor the script \
             falls back to — two copies of one default that can disagree"
        );
        assert_eq!(
            cfg["contract"]["settings"][key]["default"], *value,
            "`contract.settings.{key}.default` publishes a different value than \
             the instance actually carries"
        );
    }
}

#[test]
fn the_screen_manifest_is_the_shipped_example() {
    if !shipped() {
        return;
    }
    // The mount the example carries names the member rather than a position in
    // a band, so there is nothing to read off the tree for it.
    let want = read_json(&repo("examples/organism/grow-screen.json"));
    let decls = want["manifest"].as_array().expect("a manifest of two");
    assert_eq!(
        decls.len(),
        2,
        "grow-screen.json carries the screen and the app and nothing else: the \
         third declaration — the way back into a generation — moved into the \
         assistant level, which is the one place that knows the generation's \
         name"
    );
    assert!(
        decls[0]["diff"]["add_nodes"][0]["override_params"]["web"]["mount"]
            .as_str()
            .is_some(),
        "the screen example names a mount"
    );
    let got = manifests(member_wish(&member_template()));
    // The devices are the TAIL of the one manifest a member wish renders
    // (GH #585); the example file stays the operator-applicable half of it.
    let rendered: Vec<Value> = got[0]["manifest"].as_array().expect("the one manifest")[1..]
        .iter()
        .map(normalised)
        .collect();
    let shipped: Vec<Value> = decls.iter().map(normalised).collect();
    assert_eq!(
        rendered, shipped,
        "the rendered screen manifest and examples/organism/grow-screen.json \
         are not the same bytes — one is generated from the other's table and \
         they cannot drift apart"
    );
}

#[test]
fn the_way_back_from_a_screen_is_part_of_the_assistant_level() {
    if !shipped() {
        return;
    }
    // The member splits `event`/`receipt` on `hop.owner` and hands anything
    // under `/assistants/` to the container. The second hop — into the
    // generation the owner names — is the LEVEL's, because the ordinary
    // `in_turn` door is guarded on `context.assistant` and a screen event
    // carries none. Measured on a live colony: without it a screen event
    // reaches the container and stops there.
    let out = manifests(json!({"recipe": "grow_level", "request": "…",
               "params": {"scope": "/os/orgs/acme/members/alex", "level": "assistant",
                          "name": "scribe", "template": "a-template@1.0.0"}}));
    assert_eq!(
        out.len(),
        1,
        "a wish is ONE manifest, and only a member wish carries the devices in it"
    );
    let edges = out[0]["manifest"][0]["diff"]["add_edges"]
        .as_array()
        .expect("the assistant level's edges")
        .clone();
    assert!(
        edges.iter().any(|e| {
            e["from"] == json!(".")
                && e["to"] == json!("./scribe")
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains("hop.owner.contains('/assistants/scribe/')"))
        }),
        "the assistant level does not draw the way back from a screen — a view \
         event whose owner is this generation reaches the container and stops"
    );
    // and it is no longer in the screen manifest
    let screen = read_json(&repo("examples/organism/grow-screen.json"));
    let raw = screen.to_string();
    assert!(
        !raw.contains("/assistants/"),
        "grow-screen.json still carries the generation's own way back; it \
         belongs to the level that knows the generation's name"
    );
}

/// The rendered `add_edges` of one declaration, in an `EdgeTable` the real
/// router can be asked. `.` is the container the declaration stands in.
fn table_of(decl: &Value, container: &str) -> EdgeTable {
    let abs = |ep: &str| -> String {
        match ep {
            "." | "./" => container.to_string(),
            other => format!("{container}/{}", other.trim_start_matches("./")),
        }
    };
    let mut t = EdgeTable::new();
    for e in decl["diff"]["add_edges"].as_array().expect("add_edges") {
        let condition = e["condition"].as_str().map(|src| {
            meclaw_colony::cel_eval::parse_condition(src)
                .unwrap_or_else(|err| panic!("condition {src:?}: {err}"))
        });
        t.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new(&abs(e["from"].as_str().expect("from"))),
            to: Path::new(&abs(e["to"].as_str().expect("to"))),
            condition,
            modifier: None,
            is_default: e["default"].as_bool().unwrap_or(false),
            lane: None,
        });
    }
    t
}

fn headers(hop: Value, context: Value) -> Headers {
    let obj = |v: Value| match v {
        Value::Object(m) => m,
        _ => meclaw_core::serde_json::Map::new(),
    };
    Headers::from_parts(obj(context), obj(hop))
}

/// GH #543 — the way back and the ordinary door must be EXACT complements.
///
/// `apply_edges` fans out to every matching regular edge; there is no
/// exactly-one semantics in this substrate. `context` is persistent along a
/// trace, and nothing between an agent's answer and the receipt the display
/// sends back deletes `context.assistant` — so a receipt carries the context
/// AND the owner, and two edges that both accept it deliver the same turn to the
/// generation twice. Counted here, positively, on the real router.
#[test]
fn a_display_receipt_reaches_the_generation_exactly_once() {
    if !shipped() {
        return;
    }
    let assistants = "/os/orgs/acme/members/alex/assistants";
    let generation = format!("{assistants}/scribe");
    let out = manifests(json!({"recipe": "grow_level", "request": "…",
               "params": {"scope": "/os/orgs/acme/members/alex", "level": "assistant",
                          "name": "scribe", "template": "a-template@1.0.0"}}));
    let table = table_of(&out[0]["manifest"][0], assistants);
    let here = Path::new(assistants);
    let owner = format!("{generation}/talky");

    // The receipt: the owner the display stamped, AND the context the answer
    // that put the view up was carried on.
    let both = apply_edges(
        &table,
        &here,
        &headers(
            json!({"route": "in_turn", "owner": owner}),
            json!({"assistant": "scribe"}),
        ),
    );
    let hits: Vec<&str> = both
        .iter()
        .filter(|d| d.target.as_str() == generation)
        .map(|d| d.target.as_str())
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "a display receipt carrying both `context.assistant` and `hop.owner` is \
         delivered {} times to {generation} — `apply_edges` fans out to every \
         matching regular edge, so the door and the way back have to be exact \
         complements",
        hits.len()
    );

    // And the case the way back exists for: a screen EVENT, raised on a fresh
    // trace, with an owner and no context at all. It arrives, and it arrives
    // once.
    let event = apply_edges(
        &table,
        &here,
        &headers(json!({"route": "in_turn", "owner": owner}), json!({})),
    );
    let reached: Vec<&str> = event
        .iter()
        .filter(|d| d.target.as_str() == generation)
        .map(|d| d.target.as_str())
        .collect();
    assert_eq!(
        reached.len(),
        1,
        "a screen event whose owner names this generation does not reach it — \
         that is the hop a built colony was measured losing at the container"
    );
}

/// One run of `recipes` with the builder's own `params` handed in.
fn run_recipes_with(payload: Value, params: Value) -> Vec<Value> {
    emit_all(
        &shipped_script(repo(RECIPES).to_str().expect("utf-8 path")),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "params": params,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": payload.to_string()}],
        }),
    )
}

/// **A member's name has to render a mount, and the refusal comes BEFORE the
/// person.**
///
/// The mount grammar is narrower than a node name, and nothing in the substrate
/// restricts a member's name to it. A manifest rolls forward with no rollback,
/// so a wish that got past the renderer would commit the person and then have
/// its screen refused at the door — the half-act GH #585 exists to prevent,
/// re-entered through a name. So the whole wish is refused here, by name, and no
/// manifest is drafted.
///
/// **The grammar is not spelled again here.** The recipe carries a copy of it
/// (`network: deny`, no substrate to ask), and the copy is what drifts when a new
/// API route reserves a seventh name — so every name below is run through
/// `meclaw_colony::surfaces::mount_is_valid`, the original, and the two constants
/// are compared against the recipe's own.
#[test]
fn a_member_name_that_renders_no_mount_refuses_the_whole_wish() {
    if !shipped() {
        return;
    }
    let template = member_template();
    let script = std::fs::read_to_string(repo(RECIPES)).expect("the recipe config");

    // The copy against the original: the two numbers, and every reserved name.
    assert!(
        script.contains(&format!(
            "MOUNT_MAX = {}",
            meclaw_colony::surfaces::MOUNT_MAX
        )),
        "the recipe's own MOUNT_MAX is not the substrate's ({})",
        meclaw_colony::surfaces::MOUNT_MAX
    );
    // The recipe's own tuple, parsed rather than searched for: four of the six
    // names occur elsewhere in this script, so a `contains` sweep would pass on
    // a list that had dropped one.
    let tuple = script
        .split("RESERVED_MOUNTS = (")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .expect("the recipe declares its reserved names as one tuple");
    let copied: BTreeSet<String> = tuple
        .split(',')
        .map(|t| t.trim().trim_matches(|c| c == '\\' || c == '"').to_string())
        .filter(|t| !t.is_empty())
        .collect();
    let original: BTreeSet<String> = meclaw_colony::surfaces::RESERVED_MOUNTS
        .iter()
        .map(|n| n.to_string())
        .collect();
    assert_eq!(
        copied, original,
        "the recipe's copy of the reserved names and the substrate's own list \
         have drifted apart — a member whose screen renders a name the listener \
         already answers would be refused at the door with the person already \
         committed"
    );

    for name in ["Alex", "alex_1", "alex.b"] {
        assert!(
            !meclaw_colony::surfaces::mount_is_valid(&format!("{name}-display")),
            "the fixture is wrong: `{name}-display` IS a mount"
        );
        let wish = json!({"recipe": "grow_level", "request": "grow a member",
                          "params": {"scope": ORG, "level": "member", "name": name,
                                     "template": template}});
        let out = run_recipes(wish);
        assert_eq!(
            out.len(),
            1,
            "a refusal answers ONCE and drafts nothing for {name}: {out:?}"
        );
        assert_eq!(
            out[0]["header"]["error_code"],
            json!("wish_incomplete"),
            "the member {name} renders no mount, and that is a wish a human can \
             answer by naming another one: {:?}",
            out[0]["header"]
        );
        assert!(
            out[0]["manifest"].is_null(),
            "the PERSON must not be drafted either — a manifest rolls forward \
             with no rollback, so half of it is a member with no devices: {:?}",
            out[0]["manifest"]
        );
        let text = out[0]["messages"][0]["text"].as_str().expect("a reason");
        assert!(
            text.contains("[a-z0-9-]")
                && text.contains("params.name")
                && text.contains("params.screen_mount"),
            "the refusal names the grammar and BOTH halves of the name — the \
             member's and the pattern's: {text}"
        );
    }

    // A reserved segment is the other half of the grammar, and the shipped
    // pattern cannot render one — a member called `live` is `live-display`. It
    // takes a pattern that renders the bare name.
    let reserved = run_recipes_with(
        json!({"recipe": "grow_level", "request": "…",
               "params": {"scope": ORG, "level": "member", "name": "live",
                          "template": template}}),
        json!({"screen_mount": "{member}"}),
    );
    assert!(
        !meclaw_colony::surfaces::mount_is_valid("live"),
        "the fixture is wrong: `live` is not a reserved mount"
    );
    assert_eq!(reserved.len(), 1);
    assert_eq!(
        reserved[0]["header"]["error_code"],
        json!("wish_incomplete"),
        "`live` is a name a route on the same listener already answers: {:?}",
        reserved[0]["header"]
    );
    assert!(
        reserved[0]["manifest"].is_null(),
        "and the person is not drafted either: {:?}",
        reserved[0]["manifest"]
    );

    // and the positive control: the shipped name renders one, and the substrate
    // agrees that it is a mount
    assert!(meclaw_colony::surfaces::mount_is_valid("alex-display"));
    assert_eq!(
        manifests(member_wish(&template))[0]["manifest"][1]["diff"]["add_nodes"][0]["override_params"]
            ["web"]["mount"],
        json!("alex-display")
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// the roll-forward, against a real colony
// ──────────────────────────────────────────────────────────────────────────────

struct InertCellFactory;

impl CellFactory for InertCellFactory {
    fn validate_params(&self, _params: &JsonValue) -> Result<(), String> {
        Ok(())
    }

    fn is_lazy(&self) -> bool {
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_cell(
        self: Arc<Self>,
        _path: Path,
        _params: JsonValue,
        _outputs_tx: mpsc::Sender<meclaw_core::CellEmission>,
        _cell_dir: std::path::PathBuf,
        _contract: meclaw_colony::ContractView,
        _colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<std::time::Duration>,
        _blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let capacity = mailbox_capacity.max(1);
        let (sender, receiver) = mpsc::channel::<Message>(capacity);
        let wake: WakeFn = Box::new(|mut rx: mpsc::Receiver<Message>| {
            tokio::spawn(async move { while rx.recv().await.is_some() {} });
            let (stop_tx, _stop_rx) = oneshot::channel::<()>();
            let (_death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
            (stop_tx, death_ack_rx)
        });
        let respawn: RespawnFn = Box::new(move || {
            let (tx, mut rx) = mpsc::channel::<Message>(capacity);
            let (peace_tx, peace_rx) = oneshot::channel::<()>();
            let (_backstop_tx, backstop_rx) = oneshot::channel::<()>();
            let join = tokio::spawn(async move {
                let _peace_keep = peace_tx;
                while rx.recv().await.is_some() {}
            });
            (tx, join, peace_rx, backstop_rx)
        });
        let (stop_tx, _stop_rx) = oneshot::channel::<()>();
        let (_death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
        Ok(SpawnedCellKind::Dormant {
            sender,
            receiver,
            wake,
            stop_tx,
            death_ack_rx,
            respawn,
        })
    }
}

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

fn cell_types_in(root: &std::path::Path) -> BTreeSet<String> {
    fn walk(dir: &std::path::Path, out: &mut BTreeSet<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.file_name().and_then(|n| n.to_str()) == Some("config.json")
                && let Ok(raw) = std::fs::read_to_string(&p)
                && let Ok(v) = meclaw_core::serde_json::from_str::<Value>(&raw)
                && let Some(t) = v["cell"]["type"].as_str()
            {
                out.insert(t.to_string());
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(root, &mut out);
    out.remove("hive");
    out.remove("ref");
    out
}

fn build_root(root: &std::path::Path) {
    copy_tree(&repo("examples/organism/seed"), root);
    copy_tree(&repo("templates"), &root.join("templates"));
    std::fs::write(
        root.join(".env"),
        "OPENROUTER_API_KEY=test-key\n\
         MODEL_BRAIN=gpt-4o-mock\n\
         MODEL_CORE=gpt-4o-mock\n\
         MODEL_CORE_FAST=gpt-4o-mock-fast\n\
         MODEL_SURFACE=gpt-4o-mock-surface\n\
         MODEL_CLOSER=gpt-4o-mock\n\
         MODEL_DIALECTIC=gpt-4o-mock\n\
         MODEL_DREAMER=gpt-4o-mock\n\
         TELEGRAM_BOT_TOKEN=test-token\n\
         TELEGRAM_BOT_TOKEN_2=test-token-2\n\
         TELEGRAM_ALLOWED_USER_ID=0\n\
         EXAMPLE_CHAT_TOKEN=test-chat-token\n",
    )
    .unwrap();
}

async fn boot(td: &tempfile::TempDir) -> ColonyHandle {
    let fs = factories(td.path());
    let h = ColonyHandle::new_with_factories_at(td, fs.clone());
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in fs {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the empty seed of examples/organism must boot");
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan");
    ack_rx
        .await
        .expect("rescan ack")
        .expect("GH #440: the rescan must not have aborted");
    h
}

fn factories(root: &std::path::Path) -> Vec<(String, Arc<dyn CellFactory>)> {
    cell_types_in(&root.join("templates"))
        .into_iter()
        .map(|t| (t, Arc::new(InertCellFactory) as Arc<dyn CellFactory>))
        .collect()
}

async fn mutate(h: &ColonyHandle, payload: Value) -> MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("send mutation");
    ack_rx.await.expect("mutation ack")
}

fn committed(o: &MutationOutcome) -> bool {
    matches!(o, MutationOutcome::Committed { .. })
}

/// One BODY at the door, form unknown to the caller — the door `--apply`,
/// `POST /colony/mutations` and the submitter all knock on. A manifest is a
/// body, not a declaration, and it is judged as one.
async fn knock(h: &ColonyHandle, payload: Value) -> MutationDoorOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::MutationDoor {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("send body");
    ack_rx.await.expect("door ack")
}

/// How many declarations of a manifest the door applied. A manifest rolls
/// forward and stops at the first refusal, so this number IS the verdict: a
/// refused entry leaves everything in front of it standing and everything
/// behind it untouched.
fn applied(o: &MutationDoorOutcome) -> usize {
    match o {
        MutationDoorOutcome::Manifest(ManifestOutcome::Committed { ids }) => ids.len(),
        MutationDoorOutcome::Manifest(ManifestOutcome::Rejected { ids, .. }) => ids.len(),
        _ => 0,
    }
}

/// The paths `/colony/graph` reports under a scope, so a claim about what the
/// grown tree holds is measured rather than asserted.
async fn graph_nodes(h: &ColonyHandle, scope: &str) -> Vec<String> {
    let (ack_tx, ack_rx) = oneshot::channel::<meclaw_colony::api_dto::ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: Path::new(scope),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .nodes
        .iter()
        .map(|n| n.path.to_string())
        .collect()
}

/// The whole claim, against a real colony: the three declarations apply in the
/// order they were rendered, and only in that order.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_screen_lands_after_the_member_in_that_order() {
    if !shipped() || !library_is_complete() {
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    build_root(td.path());
    let h = boot(&td).await;
    assert!(
        committed(&mutate(&h, read_json(&repo("examples/organism/grow-os.json"))).await),
        "the shell must be committed before anything is grown into it"
    );
    assert!(
        committed(&mutate(&h, read_json(&repo("examples/organism/grow-org.json"))).await),
        "the organisation must be committed before a member is grown into it"
    );

    let before = graph_nodes(&h, "/os/orgs/acme/members").await;
    assert!(
        before.is_empty(),
        "the organisation has no members yet: {before:?}"
    );

    let out = manifests(member_wish(&member_template()));
    let decls = out[0]["manifest"].as_array().expect("the one manifest");
    let whole = json!({"manifest": decls});
    let devices = json!({"manifest": decls[1..]});

    // THE ROLL-FORWARD, measured. The screen draws into a scope only the
    // declaration in front of it creates, so on its own it has nowhere to land.
    let early = knock(&h, devices).await;
    assert_eq!(
        applied(&early),
        0,
        "the devices applied something BEFORE the member existed — then the \
         order this recipe renders in would be decoration rather than \
         semantics: {early:?}"
    );
    let whole_outcome = knock(&h, whole).await;
    assert_eq!(
        applied(&whole_outcome),
        3,
        "the wish did not commit as ONE submission of three declarations — the \
         person, the screen, the app (GH #585): {whole_outcome:?}"
    );

    let grown = graph_nodes(&h, "/os/orgs/acme/members/alex").await;
    // A hive leaves no registry row, so both devices are named by an occupant
    // of theirs: the display's own socket, and the app's own layout.
    for want in [
        "/os/orgs/acme/members/alex/channels/display/web",
        "/os/orgs/acme/members/alex/apps/colony-view/layout",
    ] {
        assert!(
            grown.iter().any(|p| p == want),
            "{want} is not in the grown tree: {grown:?}"
        );
    }

    // and the member stands as ONE direct child of the container, which is
    // what makes its name — and therefore its screen's door — unique
    let members = graph_nodes(&h, "/os/orgs/acme/members").await;
    let direct: BTreeSet<&str> = members
        .iter()
        .filter_map(|p| p.strip_prefix("/os/orgs/acme/members/"))
        .map(|rest| rest.split('/').next().unwrap_or(rest))
        .collect();
    assert_eq!(
        direct.len(),
        1,
        "exactly one member stands under the container: {direct:?}"
    );
    h.shutdown().await;
}
