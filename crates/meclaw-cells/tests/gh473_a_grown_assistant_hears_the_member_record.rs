//! GH #473 — a grown assistant hears the member's record.
//!
//! WHAT WAS BROKEN
//! ===============
//! A generation grown by `grow_level` came up with an EMPTY `system` tree.
//! Nothing in a grown topology writes a durable slot into an agent's brain, and
//! the one lane that can — `in_pack` (GH #458) — was not part of the assistant
//! level. The member's own `./affinity` holds the record of the person, and the
//! member's shipped graph carries an affinity `answer` outward only while
//! `hop.subscriber == ''`: a PUSH, which by definition names a subscriber,
//! reached nothing. So the agent introduced itself as whatever the model is by
//! default, beside a hive that knew exactly who it was talking for.
//!
//! `templates/builder/recipes/config.json` now renders two more edges behind
//! the level's eleven when `params.subscribe` is set — the identity door, in
//! the form `templates/affinity/README.md` publishes. This file asks whether
//! that door is real, and it asks it in two halves that meet:
//!
//!   (a) THE EDGES EXIST. The rendered declaration goes through a running
//!       colony's mutation door and the two edges are read back out of the
//!       colony's own graph. Edge existence is its own measurement in this
//!       substrate — a manifest that renders an edge and a colony that carries
//!       one are two different statements, and only the second one routes.
//!
//!   (b) THE DOOR CARRIES A RECORD. A second colony boots `affinity` beside a
//!       grown assistant and wires them with the edge set the SAME render
//!       produced — read out of the manifest, not written down here — and an
//!       affinity push crosses it: it lands as a durable `system.identity` row
//!       in the assistant's own `cell.db` AND it is readable in the system
//!       prompt the assistant's next turn sends to the provider.
//!
//! BOTH SIDES ARE POSITIVE SIGNALS
//! ===============================
//! The counter-probe never argues from silence. Before the subscription the
//! assistant answers a REAL turn and the mock provider records a REAL request —
//! and the disclosed material is missing from it. Without the two edges the
//! affinity's push is caught on a drain of its own, so the push demonstrably
//! HAPPENED, and the brain that never heard it answers another real turn whose
//! prompt still says nothing about the person. Every claim on both sides is a
//! thing that arrived.
//!
//! WHY THE MUTATION DOOR AND NOT `/os/submit`
//! ==========================================
//! The submit gate refuses an `in_pack` edge that does not end at the
//! REQUESTER's own hive (`subscribe_target_not_self`, GH #458) — which is why
//! the recipe renders the door only on request. A manifest of this shape
//! therefore cannot travel the submission front at all today; it is applied at
//! `ColonyMsg::Mutation`, the door behind `POST /colony/mutations` and
//! `meclaw --apply`. The other half of the subscription — the `subscribers`
//! ROW — is not renderable in any manifest: it is a store write and stays a
//! `subscribe` op through affinity's own `./gate` (ruling R-Subscribe), which
//! is exactly how this file creates it.
//!
//! WHY `talky` STANDS IN THE GROWN SLOT IN (b)
//! ===========================================
//! The level's edges are pure ADDRESSING and name no template — the recipe is
//! told which one to grow. The shipped `assistant` composite fans one pack out
//! to three `llm` cells and carries a `tools` hive that would need six more
//! cell types spawned for a claim this file does not make; that fan-out is
//! `gh458_the_door_in_the_wall.rs`'s pin. So the smallest composite that
//! accepts `in_pack`, answers a turn with exactly one provider call and drains
//! `pack_ack` stands at the grown address, and the addressing under test is the
//! rendered one, byte for byte.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, RespawnFn, SpawnedCellKind,
    WakeFn, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, JsonValue, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

// ══════════════════════════════════════════════════════ the shipped tree

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn templates_root() -> std::path::PathBuf {
    repo("templates")
}

/// The `builder`'s fast lane — the file that renders the level under test.
const RECIPES: &str = "templates/builder/recipes/config.json";

/// The reference organism. Its first three declarations build the scaffolding
/// half (a) grows into, and its assistant declaration is where the TEMPLATE
/// name comes from: pinning a version here would make this file red for
/// somebody else's bump.
const SHELL: &str = "examples/organism/grow-os.json";
const ORG: &str = "examples/organism/grow-org.json";
const MEMBER: &str = "examples/organism/grow-member.json";
const ASSISTANT: &str = "examples/organism/grow-assistant.json";

/// Where the level is grown, and under which name. Both halves use the same
/// pair so the rendered strings of one are the rendered strings of the other.
const SCOPE: &str = "/os/orgs/acme/members/alex";
const NAME: &str = "scribe";

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// R2b / GH #49: a tree that did not ship what a test reads SKIPS rather than
/// failing. Half (a) needs the example and the whole library it instantiates.
fn organism_is_shipped() -> bool {
    [SHELL, ORG, MEMBER, ASSISTANT, RECIPES]
        .iter()
        .all(|f| repo(f).is_file())
        && [
            "meclaw-os",
            "org",
            "member",
            "assistant",
            "talky",
            "cogny",
            "collector",
            "access",
            "tools",
        ]
        .iter()
        .all(|n| repo(&format!("templates/{n}/template.json")).is_file())
}

/// Every file the affinity hive is made of, and the composite that stands at
/// the grown address. Half (b) needs both and nothing else.
const AFFINITY_FILES: &[&str] = &[
    "config.json",
    "store/config.json",
    "brief/config.json",
    "gate/config.json",
    "push/config.json",
    "clock/config.json",
    "store/seed/entities.jsonl",
    "store/seed/relations.jsonl",
    "store/seed/trust.jsonl",
    "store/seed/disclosure.jsonl",
    "store/seed/subscribers.jsonl",
];

fn shipped(name: &str, files: &[&str]) -> Option<std::path::PathBuf> {
    let root = templates_root().join(name);
    files
        .iter()
        .all(|rel| root.join(rel).exists())
        .then_some(root)
}

/// The shipped template, copied the way instantiation copies it: `config.json`
/// files, the seed tables next to them, and a `ref` resolved to the tree it
/// names (GH #277).
fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    let src = &resolve_template_ref(src);
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let name = entry.file_name();
        if from.is_dir() {
            copy_cells(&from, &dst.join(name));
        } else if name == "config.json"
            || src.file_name().is_some_and(|d| d == "seed")
                && std::path::Path::new(&name)
                    .extension()
                    .is_some_and(|e| e == "jsonl")
        {
            std::fs::copy(&from, dst.join(name)).unwrap();
        }
    }
}

fn resolve_template_ref(dir: &std::path::Path) -> std::path::PathBuf {
    let mut dir = dir.to_path_buf();
    for _ in 0..8 {
        let Ok(raw) = std::fs::read_to_string(dir.join("config.json")) else {
            return dir;
        };
        let Ok(v) = meclaw_core::serde_json::from_str::<Value>(&raw) else {
            return dir;
        };
        if v["cell"]["type"] != "ref" {
            return dir;
        }
        let reference = v["cell"]["template"]
            .as_str()
            .expect("a ref cell names a template");
        dir = templates_root().join(reference.split('@').next().unwrap_or_default());
    }
    panic!("template ref chain does not terminate at {}", dir.display());
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

fn patch(root: &std::path::Path, rel: &str, f: impl FnOnce(&mut Value)) {
    let p = root.join(rel);
    let mut v: Value = meclaw_core::serde_json::from_str(&std::fs::read_to_string(&p).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    f(&mut v);
    std::fs::write(&p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

// ══════════════════════════════════════════════ the level, as it is rendered

/// One `grow_level` wish for the assistant level, run through the SHIPPED fast
/// lane. `subscribe` is the only difference between the two calls, which is
/// what makes the difference between their outputs the door itself.
fn rendered_declaration(subscribe: bool) -> Value {
    let script = meclaw_testing::shipped_script(
        repo(RECIPES)
            .to_str()
            .expect("a utf-8 path to the recipe config"),
    );
    // The template and the birth context come OFF the reference declaration.
    // What is under test is the ADDRESSING; pinning a version or a model key
    // here would make this file red for somebody else's bump, and the mutation
    // door is the authority on what a template demands (`requirement_missing`).
    let reference = read_json(&repo(ASSISTANT));
    let mut params = json!({
        "scope": SCOPE,
        "level": "assistant",
        "name": NAME,
        "template": reference["diff"]["add_nodes"][0]["template"].clone(),
        "ctx": reference["ctx"].clone(),
    });
    if subscribe {
        params["subscribe"] = json!(true);
    }
    let out = meclaw_testing::emit_one(
        &script,
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                "text": json!({"recipe": "grow_level", "request": "…",
                               "params": params}).to_string()}],
        }),
    );
    assert!(
        out["header"]["error_code"].is_null(),
        "the fast lane refused the level it is supposed to render: {out}"
    );
    let decls = out["manifest"].as_array().expect("a manifest").clone();
    assert_eq!(decls.len(), 1, "a level is ONE declaration");
    decls[0].clone()
}

fn rendered_edges(subscribe: bool) -> Vec<Value> {
    rendered_declaration(subscribe)["diff"]["add_edges"]
        .as_array()
        .expect("a level declares its edges")
        .clone()
}

/// One edge with both endpoints resolved against the declaration it stands in.
/// Since GH #503 the two calls below do not stand at the same scope root — a
/// plain level declares itself at `<member>/assistants`, a subscribing one at
/// the member, because `./affinity` is a SIBLING of that container — so the
/// difference between them is taken on the edges the DOOR will draw.
fn resolved(edge: &Value, scope: &str) -> Value {
    let root = meclaw_core::Path::new(scope);
    let mut out = edge.clone();
    for side in ["from", "to"] {
        let raw = out[side].as_str().expect("an endpoint").to_string();
        out[side] = json!(meclaw_core::Path::resolve(&root, &raw).as_str());
    }
    out
}

/// The rendered set, re-spelled for the colony this file BOOTS — which stands
/// in a `talky` for the generation, not a whole `assistant`.
///
/// Re-spelled relative to the MEMBER, which is the storey that colony stands
/// on: since GH #503 a plain level declares itself one storey lower, at
/// `<member>/assistants`, and spells its child `./<name>`. Resolving and
/// rebasing changes the address an edge is written at and nothing about the
/// edge.
///
/// GH #562 put four MEMORY v-lanes in the assistant's set, and a v-lane ends on
/// an occupant of the generation (`./scribe/talky`, `./scribe/cogny`). The
/// stand-in has neither, so those four endpoints would dangle at boot — a fact
/// about the FIXTURE and not about the render. They are dropped here, and only
/// here: `rendered_edges` (what the recipe produced) is untouched, and the
/// identity door this file is actually about — whose own v-lanes end on the
/// BRAIN of each rim (GH #561) and are wired by `build_tree` — is kept, because
/// that is the thing under test.
fn member_relative_edges(subscribe: bool) -> Vec<Value> {
    resolved_edges(subscribe)
        .into_iter()
        .filter(|e| {
            !matches!(
                e.get("lane").and_then(|l| l.as_str()),
                Some("recall" | "in_bundle")
            )
        })
        .map(|mut e| {
            for side in ["from", "to"] {
                let abs = e[side].as_str().expect("an endpoint").to_string();
                e[side] = json!(
                    abs.strip_prefix(&format!("{SCOPE}/"))
                        .map(|rest| format!("./{rest}"))
                        .unwrap_or_else(|| ".".to_string())
                );
            }
            e
        })
        .collect()
}

fn resolved_edges(subscribe: bool) -> Vec<Value> {
    let decl = rendered_declaration(subscribe);
    let scope = decl["scope"].as_str().expect("a scope").to_string();
    decl["diff"]["add_edges"]
        .as_array()
        .expect("a level declares its edges")
        .iter()
        .map(|e| resolved(e, &scope))
        .collect()
}

/// The identity door, DERIVED rather than transcribed: whatever `subscribe`
/// adds to the level and nothing else. If the recipe ever renders the door in
/// another shape, this file follows it there instead of testing a copy of it.
fn identity_door() -> Vec<Value> {
    let base = resolved_edges(false);
    let decl = rendered_declaration(true);
    let scope = decl["scope"].as_str().expect("a scope").to_string();
    // Kept in the spelling the subscribing declaration writes — that is the
    // declaration these two edges stand in, and the one they are submitted with.
    let door: Vec<Value> = rendered_edges(true)
        .into_iter()
        .filter(|e| !base.contains(&resolved(e, &scope)))
        .collect();
    assert_eq!(
        door.len(),
        4,
        "`subscribe` must add exactly the four v-lanes of the door — one push \
         per brain rim and one receipt drain per rim, because a lane and its \
         receipt are ONE decision (GH #458) and since GH #561 the pack ends at \
         the rims rather than at the generation's own path: {door:?}"
    );
    door
}

/// One rendered edge, picked by the route its condition names.
///
/// The route is a key against the REST of the level — no other edge here
/// conditions on `answer` or `pack_ack` — but since GH #561 it is no longer a
/// key inside the door itself: there are FOUR edges, one per brain rim per
/// direction, so `answer` matches TWO of them and this returns whichever comes
/// first. That is sound for what the helper is asked, and only for that: the
/// two push edges differ in their TARGET rim and are identical in the guard
/// this file reads out of them (`hop.subscriber` names the generation, because
/// a subscription is one row about one agent — the fan-out is the two edges).
/// A claim about one particular rim reads `identity_door()` and filters on
/// `to`.
fn door_edge(route: &str) -> Value {
    identity_door()
        .into_iter()
        .find(|e| {
            e["condition"]
                .as_str()
                .unwrap_or_default()
                .contains(&format!("hop.route == '{route}'"))
        })
        .unwrap_or_else(|| panic!("the door has no `{route}` edge: {:?}", identity_door()))
}

/// The single-quoted literal that follows `key` in a rendered CEL condition.
/// The subscriber address the door matches on is a STRING inside a guard, and
/// the `subscribers` row this file writes has to carry exactly that string —
/// so it is read out of the guard rather than written twice.
fn quoted_after(condition: &str, key: &str) -> String {
    let at = condition
        .find(key)
        .unwrap_or_else(|| panic!("no `{key}` in the rendered condition {condition:?}"));
    let rest = &condition[at + key.len()..];
    let open = rest
        .find('\'')
        .unwrap_or_else(|| panic!("no literal after `{key}` in {condition:?}"));
    let tail = &rest[open + 1..];
    let close = tail
        .find('\'')
        .unwrap_or_else(|| panic!("unterminated literal after `{key}` in {condition:?}"));
    tail[..close].to_string()
}

/// The address the door matches on, as the recipe spelled it.
fn subscriber_literal() -> String {
    quoted_after(
        door_edge("answer")["condition"]
            .as_str()
            .unwrap_or_default(),
        "hop.subscriber ==",
    )
}

// ══════════════════════════════════════════ (a) the door reaches the graph

/// The inert factory of `gh302`/`gh466`, copied deliberately: half (a)'s claims
/// are all structural, and several cell types of this stack would reach outward
/// the moment they were spawned for real.
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

async fn boot_organism(td: &tempfile::TempDir) -> ColonyHandle {
    let root = td.path();
    copy_tree(&repo("examples/organism/seed"), root);
    copy_tree(&repo("templates"), &root.join("templates"));
    // The keeper's nightly close sweep, pushed to a date this run cannot reach.
    // It was a `KEEPER_NIGHT_CRON` line in the `.env` below until GH #138: the
    // schedule is a LITERAL of `session-keeper/night`'s own params now, so such
    // a line is read by nothing at all -- the sweep would fire into this run and
    // nobody would say so. The library copy is this tree's own, so writing the
    // key into it is what an `override_params` entry does to a staged config
    // (`crates/meclaw-cells/tests/gh138_keeper_summarizer_dispatcher_params.rs`
    // is the proof that the timer plans on what it finds there).
    meclaw_testing::quiet_keeper_night(&root.join("templates/session-keeper"));
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
    let fs: Vec<(String, Arc<dyn CellFactory>)> = cell_types_in(&root.join("templates"))
        .into_iter()
        .map(|t| (t, Arc::new(InertCellFactory) as Arc<dyn CellFactory>))
        .collect();
    let h = ColonyHandle::new_with_factories_at(td, fs.clone());
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in fs {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(root, &registry, &h.runtime())
        .await
        .expect("the empty seed of examples/organism must boot");
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: root.join("templates"),
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

/// One body at the door, form unknown to the caller — exactly what
/// `POST /colony/mutations` and `meclaw --apply` hand it.
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

fn committed(outcome: &MutationOutcome, what: &str) {
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "{what} was not committed: {outcome:?}"
    );
}

/// Every edge the colony carries, as `(from, to, condition)`. Read off the
/// colony's own graph rather than off the manifest: whether an edge EXISTS is a
/// separate measurement from whether one was declared.
async fn colony_edges(h: &ColonyHandle) -> Vec<(String, String, String)> {
    let (ack_tx, ack_rx) = oneshot::channel::<meclaw_colony::api_dto::ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: Path::new("/"),
            ack: ack_tx,
        })
        .await
        .expect("read graph");
    let mut edges: Vec<(String, String, String)> = ack_rx
        .await
        .expect("graph reply")
        .edges
        .iter()
        .map(|e| {
            (
                e.from.to_string(),
                e.to.to_string(),
                e.condition.clone().unwrap_or_default(),
            )
        })
        .collect();
    edges.sort();
    edges
}

/// A rendered relative edge, as the colony spells it once it stands under
/// `SCOPE`. The recipe writes `./x`; the graph carries `/os/…/alex/x`.
fn absolute(edge: &Value) -> (String, String, String) {
    let abs = |p: &str| format!("{SCOPE}/{}", p.trim_start_matches("./"));
    (
        abs(edge["from"].as_str().expect("an edge has a from")),
        abs(edge["to"].as_str().expect("an edge has a to")),
        edge["condition"].as_str().unwrap_or_default().to_string(),
    )
}

/// Grow the reference scaffolding and then ONE assistant level, rendered with
/// or without the door, and hand back the graph the colony ended up with.
async fn grown_graph(subscribe: bool) -> Vec<(String, String, String)> {
    let td = tempfile::TempDir::new().unwrap();
    let h = boot_organism(&td).await;
    // The shell, the organisation and the person come off the reference
    // example: they are the ground the level is grown into, not the claim.
    for (file, what) in [
        (SHELL, "the shell"),
        (ORG, "the org"),
        (MEMBER, "the person"),
    ] {
        committed(&mutate(&h, read_json(&repo(file))).await, what);
    }
    committed(
        &mutate(&h, rendered_declaration(subscribe)).await,
        "the rendered assistant level",
    );
    let edges = colony_edges(&h).await;
    h.shutdown().await;
    edges
}

/// (a) The whole of the first half: a level rendered with `subscribe` puts the
/// two identity-door edges into a running colony's graph, and the SAME level
/// rendered without it puts neither there.
///
/// Both directions matter. That the door appears is the feature; that it stays
/// away otherwise is the reason it is opt-in at all — the submit gate refuses
/// an `in_pack` edge that does not end at the requester's own hive, so a level
/// that always drew one could be grown by nobody but the brain being grown.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_rendered_identity_door_reaches_the_colonys_own_graph() {
    if !organism_is_shipped() {
        return;
    }
    let door: Vec<(String, String, String)> = identity_door().iter().map(absolute).collect();

    let with = grown_graph(true).await;
    for e in &door {
        assert!(
            with.contains(e),
            "the rendered door edge {e:?} never reached the colony's graph — a \
             manifest that declares an edge and a colony that routes one are two \
             different statements"
        );
    }

    let without = grown_graph(false).await;
    for e in &door {
        assert!(
            !without.contains(e),
            "the door is OPT-IN and this level did not ask for it, yet {e:?} \
             stands in the graph"
        );
    }

    // And nothing else moved: the eleven edges of the plain level are in both
    // graphs, so what `subscribe` changed is the door and only the door.
    for e in member_relative_edges(false).iter().map(absolute) {
        assert!(
            with.contains(&e) && without.contains(&e),
            "the level's own edge {e:?} is missing from one of the two graphs; \
             `subscribe` is supposed to ADD a door, not rewrite the level"
        );
    }
}

// ═══════════════════════════ (b) the door carries the member's own record

/// A fixed schedule id for the affinity's push clock, the one timer in this
/// colony that has to tick. `${uuid7:…}` is an INSTANTIATION-side substitution,
/// so a tree written straight to disk carries a literal; the generation's own
/// timers get theirs from `quiesce`, which also stops them.
const CLOCK_ID: &str = "01916f00-0000-7000-8000-000000000473";
/// Never during a test run: the shipped default is the real night.
const NEVER: &str = "0 0 0 1 1 *";

/// The round these turns are spoken in, in the affinity vocabulary the audience
/// gate speaks. Never `["*"]` — a universal set would let a pin pass over a
/// path that had lost the real one.
const AUDIENCE_CEL: &str = r#"'["member:alex","agent:scribe"]'"#;

/// The person whose record the member's affinity holds, and the marker that
/// proves the pack went through the audience filter and came out readable.
const SUBJECT: &str = "entity:alex";
const DISCLOSED: &str = "Kern";

/// The subscribing side, copied in shape from `gh458`: the actor and the
/// SUBSCRIBER ride on the hop so the port edge can promote them into context. A
/// subscription names a cell that will be handed somebody's briefs — that
/// address is a routing decision, so it belongs to an edge, never to a body.
const WRITER: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
raw = str(msgs[-1].get("text", "{}")) if msgs else "{}"
try:
    a = json.loads(raw or "{}")
except Exception:
    a = {}
if not isinstance(a, dict):
    a = {}
sys.stdout.write(json.dumps({
    "header": {"route": "propose", "actor": "member:alex",
               "subscriber": str(a.get("subscriber") or "")},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "w473",
                  "text": raw}]}))
"#;

/// The channel side: it raises a turn for the grown generation. A real member
/// channel stamps the same three context keys; here they are literals on the
/// harness edge, which is where addressing belongs.
const SURFACE: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
sys.stdout.write(json.dumps({"header": {"route": "turn"},
                             "messages": d.get("messages", [])}))
"#;

fn code_cell(script: &str, routes: &[&str], extra_hop: Value) -> Value {
    let mut hop = json!({"route": {"type": "string", "values": routes, "required": false}});
    if let Some(extra) = extra_hop.as_object() {
        for (k, v) in extra {
            hop[k] = v.clone();
        }
    }
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 15000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": hop
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in beside a grown assistant.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// The colony of half (b): the harness lanes a parent would draw, plus the
/// level's OWN edge set exactly as the recipe rendered it.
///
/// The affinity's `answer` is drained to `/park` UNCONDITIONALLY, in both
/// variants. That drain is what makes the counter-probe positive — without the
/// door the push still leaves the affinity and is caught, so "the brain never
/// heard it" is a statement about a message that demonstrably travelled. In the
/// door variant the push fans out to both, which changes nothing about where
/// it lands.
fn main_config(level_edges: &[Value]) -> Value {
    let mut edges = vec![
        // affinity's write port: actor and subscriber become EDGE truth
        json!({"from": "./writer", "to": "./affinity",
               "condition": "has(hop.route) && hop.route == 'propose'",
               "modifier": {"set_hop": {"route": "'in_propose'"},
                            "set_context": {
                                "actor": "hop.actor",
                                "subscriber": "has(hop.subscriber) ? hop.subscriber : ''"}}}),
        json!({"from": "./affinity", "to": "/sink",
               "condition": "has(hop.route) && (hop.route == 'ack' || hop.route == 'error')"}),
        json!({"from": "./affinity", "to": "/park",
               "condition": "has(hop.route) && hop.route == 'answer'"}),
        // the channel a member would own, raising a turn for this generation
        json!({"from": "./surface", "to": "./assistants",
               "condition": "has(hop.route) && hop.route == 'turn'",
               "modifier": {"set_hop": {"route": "'in_turn'"},
                            "set_context": {"channel": "'telegram'",
                                            "audience_set": AUDIENCE_CEL,
                                            "assistant": format!("'{NAME}'")}}}),
        json!({"from": "./assistants", "to": "/sink",
               "condition": "has(hop.route) && (hop.route == 'answer' || hop.route == 'pack_ack')"}),
        json!({"from": "./assistants", "to": "/park",
               "condition": "has(hop.route) && hop.route != 'answer' && hop.route != 'pack_ack'"}),
    ];
    edges.extend(level_edges.iter().cloned());
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}})
}

/// The tree half (b) boots: the shipped affinity, the grown address filled with
/// the smallest composite that answers on both lanes, and the level's rendered
/// edges between them.
fn build_tree(
    td: &tempfile::TempDir,
    affinity: &std::path::Path,
    agent: &std::path::Path,
    level_edges: &[Value],
    base_url: &str,
) -> String {
    let root = td.path();
    std::fs::write(root.join(".env"), "OPENROUTER_API_KEY=test-key\n").unwrap();
    write(root, "main/config.json", &main_config(level_edges));
    write(
        root,
        "main/writer/config.json",
        &code_cell(
            WRITER,
            &["propose"],
            json!({"actor": {"type": "string", "required": false},
                   "subscriber": {"type": "string", "required": false}}),
        ),
    );
    write(
        root,
        "main/surface/config.json",
        &code_cell(SURFACE, &["turn"], json!({})),
    );
    // The container the level is grown into is a HIVE — a scope marker, which
    // is what makes `./assistants` an edge endpoint at all.
    write(
        root,
        "main/assistants/config.json",
        &json!({"cell": {"type": "hive"}}),
    );
    copy_cells(affinity, &root.join("main/affinity"));

    // Where the grown generation stands, read off the rendered door rather than
    // spelled here: the address the guard matches on IS the address the agent
    // has to have.
    let target = subscriber_literal();
    let rel = format!("main/{}", target.trim_start_matches("./"));
    copy_cells(agent, &root.join(&rel));

    patch(root, "main/affinity/clock/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!(CLOCK_ID);
        // Since GH #138 the cadence is a literal of `./clock`'s own params, so
        // it is written here beside the schedule_id -- the form an
        // `override_params` entry takes at instantiation. An
        // `AFFINITY_PUSH_CRON=` line in the `.env` would be read by nothing at
        // all and would say nothing about it.
        // Two seconds, so the push tick fires several times inside the
        // test's own budget.
        v["params"]["schedules"][0]["cron"] = json!("*/2 * * * * *");
    });
    // Every timer and every brain of the generation, WALKED rather than listed
    // (GH #561 — a generation has three timers and two brains, and a list here
    // would be a copy of the composite's tree that rots on the next occupant).
    // The two reasons are the shipped ones: `${uuid7:*}` is an instantiation
    // substitution, so a tree written straight to disk carries the literal, and
    // a keeper or menu tick during a run would ask for a hive this colony has
    // not got.
    quiesce(&root.join(&rel), &mut 0, base_url);
    rel
}

/// One walk of the grown generation: a fixed schedule id and a cron that never
/// fires for every timer, and the mock provider for every brain.
fn quiesce(dir: &std::path::Path, counter: &mut u32, base_url: &str) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            quiesce(&p, counter, base_url);
            continue;
        }
        if p.file_name().and_then(|n| n.to_str()) != Some("config.json") {
            continue;
        }
        let mut v = read_json(&p);
        match v["cell"]["type"].as_str().unwrap_or_default() {
            "timer" => {
                let Some(schedules) = v["params"]["schedules"].as_array_mut() else {
                    continue;
                };
                for sched in schedules.iter_mut() {
                    *counter += 1;
                    sched["schedule_id"] = json!(format!(
                        "0190a3f2-0000-7000-8000-{:012}",
                        473_000 + *counter
                    ));
                    sched["cron"] = json!(NEVER);
                }
            }
            "llm" => {
                v["params"]["base_url"] = json!(base_url);
                v["params"]["model"] = json!("gpt-4o-mock");
            }
            _ => continue,
        }
        std::fs::write(&p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
    }
}

async fn boot_colony(
    td: &tempfile::TempDir,
) -> (
    ColonyHandle,
    mpsc::Receiver<Message>,
    mpsc::Receiver<Message>,
) {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        let mut fs: Vec<(String, Arc<dyn CellFactory>)> = vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("timer".to_string(), Arc::new(TimerCellFactory)),
            ("llm".to_string(), Arc::new(LlmCellFactory)),
        ];
        // The tool surface of a real generation, and every one of these would
        // reach outward the moment it was spawned for real. No pack and no turn
        // of this file touches one; they are registered rather than deleted so
        // the tree that boots is the tree that ships (GH #561).
        for tool in ["bash", "edit", "file", "web_fetch", "web_search"] {
            fs.push((tool.to_string(), Arc::new(InertCellFactory)));
        }
        fs
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(64);
    let (park_tx, park_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    h.spawn(Path::new("/park"), move || {
        CaptureCell::new(park_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    (h, sink_rx, park_rx)
}

fn to(target: &str, text: &str) -> Message {
    MessageBuilder::new(Path::new(target))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(400)
        .build()
}

fn hop_of(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// The next message on `rx` whose `hop.route` matches. 30s is the failure
/// marker convention; several two-second push ticks fit inside it.
async fn recv_route(rx: &mut mpsc::Receiver<Message>, route: &str) -> Message {
    let mut seen: Vec<String> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let Ok(Some(m)) = tokio::time::timeout(left, rx.recv()).await else {
            panic!("no `{route}` arrived within 30s; saw {seen:?}");
        };
        if hop_of(&m, "route") == route {
            return m;
        }
        seen.push(hop_of(&m, "route"));
    }
}

/// The generation's OWN durable state: the `system` table of its brain's
/// `cell.db`. Nothing else in this colony can write a row into it, and it is
/// what the next system prompt is concatenated from.
fn brain_slots(td: &tempfile::TempDir, rel: &str) -> Vec<(String, String)> {
    let p = td.path().join(rel).join("talky/brain/cell.db");
    if !p.exists() {
        return Vec::new();
    }
    let Ok(conn) = rusqlite::Connection::open(&p) else {
        return Vec::new();
    };
    let Ok(mut stmt) = conn.prepare("SELECT slot_path, value FROM system ORDER BY slot_path")
    else {
        return Vec::new();
    };
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)));
    match rows {
        Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

/// The system prompt of the n-th provider call, composed the way the wire
/// composed it (the helper of `gh258_the_push_lane_reaches_the_prompt.rs`).
async fn composed_system_prompt(mock: &MockOpenAI, nth: usize) -> String {
    let reqs = mock.recorded_requests().await;
    let req = reqs.get(nth).unwrap_or_else(|| {
        panic!(
            "the generation must have called the provider at least {} time(s); it \
             called {}",
            nth + 1,
            reqs.len()
        )
    });
    let msgs = req.messages().expect("an OpenAI request has messages[]");
    msgs.iter()
        .filter(|m| m.get("role").and_then(|v| v.as_str()) == Some("system"))
        .filter_map(|m| m.get("content").and_then(|v| v.as_str()))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The subscription, as a `subscribe` op through affinity's own `./gate`. It is
/// the half of GH #473 no manifest can carry: a `subscribers` row is a store
/// write (ruling R-Subscribe). The body says WHAT is subscribed to and in how
/// many slots; WHERE the pushes go comes off the edge.
async fn subscribe(h: &ColonyHandle, sink: &mut mpsc::Receiver<Message>) {
    let op = json!({"op": "subscribe", "subject": SUBJECT, "channel": "telegram",
                    "slots": ["identity"], "subscriber": subscriber_literal()});
    h.send(to(
        "/writer",
        &meclaw_core::serde_json::to_string(&op).unwrap(),
    ))
    .await;
    let ack = recv_route(sink, "ack").await;
    let outcome = match &ack.body {
        Body::Inline(v) => v["messages"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        Body::Blob(_) => String::new(),
    };
    assert!(
        outcome.contains("accepted"),
        "the subscription has to exist before the push lane can carry anything: \
         {outcome}"
    );
}

/// One turn through the grown generation, answered by the mock. The returned
/// message is the composite's own `answer`, taken off the container hive — a
/// real receipt, so the provider call behind it is a real one.
async fn one_turn(h: &ColonyHandle, sink: &mut mpsc::Receiver<Message>, text: &str) -> Message {
    h.send(to("/surface", text)).await;
    recv_route(sink, "answer").await
}

/// Poll the brain's own `cell.db` until the pushed slot appears. The write is
/// the LAST thing that happens on this lane and it happens off the receipt's
/// thread; 30s is the failure marker, the 20ms step only decides how fast a
/// green run finishes.
async fn await_identity(td: &tempfile::TempDir, rel: &str) -> String {
    for _ in 0..1500 {
        let slots = brain_slots(td, rel);
        if let Some((_, v)) = slots.iter().find(|(p, _)| p == "identity") {
            return v.clone();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!(
        "the pushed identity never reached the generation's own cell.db; it holds \
         {:?}",
        brain_slots(td, rel)
    );
}

fn shipped_pair() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    Some((
        shipped("affinity", AFFINITY_FILES)?,
        // GH #561 — the generation itself, and it has to be the real one: the
        // pack ends at `<generation>/talky` and `<generation>/cogny` now, so a
        // bare surface standing at the grown address would leave both door
        // edges pointing at nothing and the measurement would be about the
        // fixture rather than about the lane.
        shipped(
            "assistant",
            &["config.json", "talky/config.json", "cogny/config.json"],
        )?,
    ))
}

/// (b) The whole claim of GH #473 in one colony: a generation grown with the
/// identity door stops introducing itself as a generic model, because its
/// `system` tree is no longer empty.
///
/// Three measurements, in the order they happen and each one positive:
///
/// 1. BEFORE. A real turn is answered and the provider records a real request —
///    whose system prompt says nothing about the person, and whose brain holds
///    no `identity` row.
/// 2. THE PUSH. The subscription is written through affinity's gate, the clock
///    ticks, the door restamps the push as `in_pack` and the receipt comes back
///    on the drain the same render provided.
/// 3. AFTER. A second real turn, a second real request — and the disclosed
///    material is readable in the system prompt the model was sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_grown_assistant_hears_the_member_record() {
    let Some((affinity, agent)) = shipped_pair() else {
        return;
    };
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("before", "stop"),
        canned_chat_completion("after", "stop"),
        canned_chat_completion("spare", "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    let rel = build_tree(
        &td,
        &affinity,
        &agent,
        &member_relative_edges(true),
        &mock.base_url,
    );
    let (h, mut sink, _park) = boot_colony(&td).await;

    // 1. Before. The generation answers, so its brain ran and its prompt is
    //    whatever it composes with an empty `system` tree.
    one_turn(&h, &mut sink, "who are you?").await;
    let before = composed_system_prompt(&mock, 0).await;
    assert!(
        !before.contains(DISCLOSED),
        "nothing has been pushed yet, so the person cannot be in the prompt: \
         {before}"
    );
    assert!(
        !brain_slots(&td, &rel).iter().any(|(p, _)| p == "identity"),
        "an ungrown `system` tree is the whole premise; the brain already holds \
         {:?}",
        brain_slots(&td, &rel)
    );

    // 2. The push, through the door the recipe rendered.
    subscribe(&h, &mut sink).await;
    let receipt = recv_route(&mut sink, "pack_ack").await;
    assert_eq!(
        hop_of(&receipt, "error_code"),
        "",
        "the pushed pack must be ACCEPTED by the door: {:?}",
        receipt.headers.hop
    );
    assert_eq!(
        hop_of(&receipt, "pack_slots"),
        "identity",
        "the pack the affinity rendered is the pack the door wrote: {:?}",
        receipt.headers.hop
    );
    let identity = await_identity(&td, &rel).await;
    assert!(
        identity.contains(DISCLOSED),
        "the slot must carry what the affinity DISCLOSED — the pack went through \
         the audience filter and came out readable: {identity:?}"
    );

    // 3. After. A second real turn, and the record is in the prompt.
    one_turn(&h, &mut sink, "and now?").await;
    let after = composed_system_prompt(&mock, 1).await;
    assert!(
        after.contains(DISCLOSED),
        "the pushed identity must reach the PROMPT, not merely the cell.db — a \
         row nobody concatenates is an agent that still answers as a generic \
         model: {after}"
    );

    h.shutdown().await;
}

/// The counter-probe, and it argues from something that arrived rather than
/// from something that did not. The SAME colony minus the two rendered edges:
/// the subscription is written, the affinity pushes, and the push is caught on
/// its own drain — so it happened. It simply had nowhere to go, and the
/// generation's next real turn goes out with the same empty `system` tree it
/// booted with.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_the_rendered_door_the_same_push_reaches_no_brain() {
    let Some((affinity, agent)) = shipped_pair() else {
        return;
    };
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("before", "stop"),
        canned_chat_completion("after", "stop"),
        canned_chat_completion("spare", "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    // The level as it is rendered WITHOUT `subscribe` — same eleven edges, no
    // door. Everything else in this colony is what the test above boots.
    let rel = build_tree(
        &td,
        &affinity,
        &agent,
        &member_relative_edges(false),
        &mock.base_url,
    );
    let (h, mut sink, mut park) = boot_colony(&td).await;

    one_turn(&h, &mut sink, "who are you?").await;
    subscribe(&h, &mut sink).await;

    // The push LEFT the affinity: this is the arrival that makes the claim
    // below a measurement instead of a silence.
    let push = recv_route(&mut park, "answer").await;
    assert_eq!(
        hop_of(&push, "subscriber"),
        subscriber_literal(),
        "the push must be addressed at the grown generation, or this probe is \
         about some other message: {:?}",
        push.headers.hop
    );

    // And it reached no brain. Asserted after a second REAL turn, so the brain
    // has demonstrably run again since the push was made.
    one_turn(&h, &mut sink, "and now?").await;
    let after = composed_system_prompt(&mock, 1).await;
    assert!(
        !after.contains(DISCLOSED),
        "without the door the pushed record must not be in the prompt; the level \
         would not be opt-in if it were: {after}"
    );
    assert!(
        !brain_slots(&td, &rel).iter().any(|(p, _)| p == "identity"),
        "without the door nothing may have been written into the generation's \
         own cell.db; it holds {:?}",
        brain_slots(&td, &rel)
    );

    h.shutdown().await;
}
