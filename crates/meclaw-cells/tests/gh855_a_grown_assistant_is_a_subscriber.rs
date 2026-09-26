//! GH #855 — a generation grown by the builder in a tree that carries a model
//! registry is its subscriber: every brain has a push road onto its
//! composite's `in_model` door, and the first mutation receipt after the
//! commit makes a subscriber row of each, with the start value it was born on.
//!
//! Two halves, the cheap one first:
//!
//! 1. **The recipe.** `grow_level assistant` renders a SECOND declaration at
//!    the builder's `model_registry_scope` -- three push edges, one per brain,
//!    and one announcement edge -- only when that setting is set, only for the
//!    shipped `assistant` template, and only for a generation under that scope.
//!    Without it the manifest is the level and nothing else, byte for byte the
//!    one `gh466` pins. And `meclaw-os` turns it on for its own builder.
//! 2. **The tree.** The shipped shell, grown into the organism's empty seed:
//!    the rendered declarations commit at the door, the edges stand in the
//!    edge table, and the receipt that follows lands three `subscribers` rows
//!    in the registry's own store, read back out of its `cell.db` -- and,
//!    since GH #858, a member grown by the same recipe lands four more, one per
//!    memory cell, on the tokens the environment binds.
//!
//! Every non-registry cell is inert (the device of `gh302`/`gh466`): the claim
//! is about topology and rows, and nothing here may reach a provider.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, RespawnFn, SpawnedCellKind,
    WakeFn, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, JsonValue, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const RECIPES: &str = "templates/builder/recipes/config.json";
const GROW: [&str; 3] = [
    "examples/organism/grow-os.json",
    "examples/organism/grow-org.json",
    "examples/organism/grow-member.json",
];
const MEMBER: &str = "/os/orgs/acme/members/alex";
const GEN: &str = "/os/orgs/acme/members/alex/assistants/scribe";
const SCOPE: &str = "/os/orgs";

fn shipped() -> bool {
    repo(RECIPES).is_file()
        && GROW.iter().all(|f| repo(f).is_file())
        && ["meclaw-os", "llm-registry", "assistant", "talky", "cogny"]
            .iter()
            .all(|t| repo(&format!("templates/{t}/config.json")).is_file())
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The assistant wish, rendered, with the builder's own params as the cell
/// would see them.
fn render(settings: Value, template: &str) -> Vec<Value> {
    render_wish(
        settings,
        json!({"scope": MEMBER, "level": "assistant", "name": "scribe",
               "template": template,
               "ctx": {"model": "${MODEL_CORE}", "model_fast": "${MODEL_CORE_FAST}",
                       "model_surface": "${MODEL_SURFACE}"}}),
    )
}

/// The member wish the organism grows by hand (`grow-member.json`), rendered by
/// the recipe instead -- with the registry road GH #858 adds to it.
fn render_member(settings: Value) -> Vec<Value> {
    let hand = read_json(&repo(GROW[2]));
    let template = hand["diff"]["add_nodes"][0]["template"]
        .as_str()
        .expect("the example names its member")
        .to_string();
    render_wish(
        settings,
        json!({"scope": "/os/orgs/acme", "level": "member", "name": "alex",
               "template": template}),
    )
}

fn render_wish(settings: Value, wish: Value) -> Vec<Value> {
    let script = meclaw_testing::shipped_script(repo(RECIPES).to_str().unwrap());
    let doc = meclaw_testing::code_stdin(&json!({
        "target": "/os/builder/recipes",
        "header": {"hop": {"route": "recipe"}, "context": {}},
        "ttl": 64,
        "params": settings,
        "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                      "text": json!({"recipe": "grow_level", "request": "…",
                                     "params": wish}).to_string()}],
    }));
    let out = meclaw_testing::run_shipped_script(&script, &doc.to_string());
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = meclaw_core::serde_json::from_slice(&out.stdout).expect("json");
    let all = if v.is_array() {
        v.as_array().cloned().unwrap()
    } else {
        vec![v]
    };
    let m = all
        .into_iter()
        .find(|m| m["header"]["operation"] == json!("recipe"))
        .expect("the fast lane answered no manifest");
    assert!(m["header"]["error_code"].is_null(), "{m}");
    m["manifest"].as_array().cloned().expect("a manifest")
}

fn assistant_template() -> String {
    read_json(&repo("examples/organism/grow-assistant.json"))["diff"]["add_nodes"][0]["template"]
        .as_str()
        .expect("the example names its assistant")
        .to_string()
}

#[test]
fn the_recipe_renders_the_registry_road_only_where_a_registry_is() {
    if !shipped() {
        return;
    }
    let template = assistant_template();

    // Off: the level and nothing else.
    let plain = render(json!({}), &template);
    assert_eq!(
        plain.len(),
        1,
        "no registry, no second declaration: {plain:?}"
    );

    // On: the level, byte for byte, and one declaration at the scope.
    let with = render(json!({"model_registry_scope": SCOPE}), &template);
    assert_eq!(with.len(), 2, "{with:?}");
    assert_eq!(
        with[0], plain[0],
        "the level itself does not move by a byte"
    );
    let road = &with[1];
    assert_eq!(road["scope"], SCOPE);
    let edges = road["diff"]["add_edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        road["diff"].get("seed_rows").is_none() && road["diff"].get("add_nodes").is_none(),
        "the road draws edges and writes nothing: a row in the registry's store would widen \
         the scope root to /os, which the shipped broker refuses"
    );
    let pushes: Vec<(String, String)> = edges
        .iter()
        .filter(|e| e["from"] == ".")
        .map(|e| {
            (
                e["to"].as_str().unwrap_or_default().to_string(),
                e["condition"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(pushes.len(), 3, "one push edge per brain: {pushes:?}");
    for (rim, (to, cond)) in ["talky", "talky-chat", "cogny"].iter().zip(&pushes) {
        assert_eq!(to, &format!("./acme/members/alex/assistants/scribe/{rim}"));
        assert!(
            cond.contains("hop.route == 'in_model'")
                && cond.contains(&format!("hop.subscriber == '{GEN}/{rim}/brain'")),
            "{cond}"
        );
    }
    let announce: Vec<&Value> = edges.iter().filter(|e| e["to"] == ".").collect();
    assert_eq!(announce.len(), 1, "{edges:?}");
    assert_eq!(announce[0]["from"], "./acme/members/alex/assistants/scribe");
    assert!(
        announce[0]["condition"]
            .as_str()
            .is_some_and(|c| c.contains("'mutation_committed'")),
        "the announcement rides the mutation receipt: {}",
        announce[0]
    );
    assert_eq!(
        announce[0]["modifier"]["set_context"]["model_generation"],
        json!(format!("'{GEN}'")),
        "the generation the edge speaks for is EDGE truth, never body: {}",
        announce[0]
    );
    // And so are the brains it announces (review of fix round 1, I-2): the
    // context of the same edge, never a hop key a transit could carry in.
    assert_eq!(
        announce[0]["modifier"]["set_hop"],
        json!({"route": "'model_subscribe'"}),
        "{}",
        announce[0]
    );
    let lit = announce[0]["modifier"]["set_context"]["model_announced"]
        .as_str()
        .unwrap_or_default();
    let brains: Value =
        meclaw_core::serde_json::from_str(lit.trim_matches('\'')).expect("a JSON literal");
    // Since GH #858 each brain carries the prose need of its template cell,
    // which the registry translates against its catalogue.
    let need = |cell: &str| {
        read_json(&repo(&format!("templates/{cell}/config.json")))["params"]["requirement"].clone()
    };
    assert_eq!(
        brains,
        json!([
            {"cell_path": format!("{GEN}/talky/brain"), "start_model": "${MODEL_SURFACE}",
             "requirement": need("talky/brain")},
            {"cell_path": format!("{GEN}/talky-chat/brain"), "start_model": "${MODEL_SURFACE}",
             "requirement": need("talky/brain")},
            {"cell_path": format!("{GEN}/cogny/brain"), "start_model": "${MODEL_CORE}",
             "requirement": need("cogny/brain")}
        ])
    );

    // A template that is not the shipped generation names its own brains, and
    // the recipe does not guess them.
    let other = render(json!({"model_registry_scope": SCOPE}), "egon@2.1.0");
    assert_eq!(other.len(), 1, "{other:?}");
    // And a scope the generation does not lie under renders nothing either.
    let elsewhere = render(json!({"model_registry_scope": "/elsewhere"}), &template);
    assert_eq!(elsewhere.len(), 1, "{elsewhere:?}");
}

#[test]
fn the_shell_turns_the_road_on_for_its_own_builder() {
    if !shipped() {
        return;
    }
    let marker = read_json(&repo("templates/meclaw-os/builder/config.json"));
    assert_eq!(
        marker["override_params"]["recipes"]["model_registry_scope"], SCOPE,
        "meclaw-os carries the registry, so its builder has to render the road: {marker}"
    );
    let registry = read_json(&repo("templates/meclaw-os/llm-registry/config.json"));
    assert!(
        registry["cell"]["template"]
            .as_str()
            .is_some_and(|t| t.starts_with("llm-registry@")),
        "{registry}"
    );
    // The bridge from `./orgs` stamps its OWN key and drops any actor a chain
    // carries from upstream: what arrives on it is an announcement and never a
    // command (review I-1 (ii)).
    let shell = read_json(&repo("templates/meclaw-os/config.json"));
    let bridge = shell["params"]["graph"]["edges"]
        .as_array()
        .and_then(|e| {
            e.iter().find(|e| {
                e["from"] == "./orgs"
                    && e["condition"]
                        .as_str()
                        .is_some_and(|c| c.contains("'model_subscribe'"))
            })
        })
        .cloned()
        .expect("the shell bridges the announcement to its registry");
    assert_eq!(
        bridge["modifier"]["set_context"],
        json!({"model_announcer": "'meclaw-os'"}),
        "{bridge}"
    );
    assert_eq!(
        bridge["modifier"]["delete_context"],
        json!(["actor"]),
        "{bridge}"
    );
    let recipes = read_json(&repo(RECIPES));
    assert_eq!(
        recipes["params"]["model_registry_scope"], "",
        "the builder alone renders no road: a tree without a registry keeps its start values"
    );
}

// ─────────────────────────────────────────────────────────────── the tree

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

/// REAL `code` and `store` cells, every other type inert: the registry's hand
/// and store have to run for a row to land, and nothing else may reach out.
fn factories(root: &std::path::Path) -> Vec<(String, Arc<dyn CellFactory>)> {
    cell_types_in(&root.join("templates"))
        .into_iter()
        .map(|t| {
            let f: Arc<dyn CellFactory> = match t.as_str() {
                "code" => Arc::new(CodeCellFactory),
                "store" => Arc::new(StoreCellFactory),
                _ => Arc::new(InertCellFactory),
            };
            (t, f)
        })
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

/// The registry store's `cell.db`, wherever the tree put it.
fn find_store_db(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if p.ends_with("llm-registry/store") && p.join("cell.db").is_file() {
                return Some(p.join("cell.db"));
            }
            if let Some(hit) = find_store_db(&p) {
                return Some(hit);
            }
        }
    }
    None
}

fn subscriber_rows(db: &std::path::Path) -> Vec<(String, String)> {
    let Ok(conn) =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return Vec::new();
    };
    let Ok(mut stmt) =
        conn.prepare("SELECT cell_path, start_model FROM subscribers ORDER BY cell_path")
    else {
        return Vec::new();
    };
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_generation_and_a_member_grown_in_the_shell_become_subscribers() {
    if !shipped() {
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    copy_tree(&repo("examples/organism/seed"), root);
    copy_tree(&repo("templates"), &root.join("templates"));
    std::fs::write(
        root.join(".env"),
        "OPENROUTER_API_KEY=test-key\n\
         MODEL_CORE=m-core\n\
         MODEL_CORE_FAST=m-core-fast\n\
         MODEL_SURFACE=m-surface\n\
         MODEL_CLOSER=m-closer\nMODEL_DIALECTIC=m-dialectic\nMODEL_DREAMER=m\nMODEL_BRAIN=m\n",
    )
    .unwrap();
    let fs = factories(root);
    let h = ColonyHandle::new_with_factories_at(&td, fs.clone());
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in fs {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(root, &registry, &h.runtime())
        .await
        .expect("the empty seed boots");
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: root.join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan");
    ack_rx.await.expect("rescan ack").expect("rescan ran");

    // The shell and the organisation as the organism grows them; the member
    // as the builder's recipe renders it (review rev-L2b2 m-5), so the road GH
    // #858 adds to a member is measured in a colony and not only at the gate.
    for file in &GROW[..2] {
        let outcome = mutate(&h, read_json(&repo(file))).await;
        assert!(
            matches!(outcome, MutationOutcome::Committed { .. }),
            "{file}: {outcome:?}"
        );
    }
    for (i, decl) in render_member(json!({"model_registry_scope": SCOPE}))
        .into_iter()
        .enumerate()
    {
        let outcome = mutate(&h, decl).await;
        assert!(
            matches!(outcome, MutationOutcome::Committed { .. }),
            "declaration {i} of the grown member was refused: {outcome:?}"
        );
    }
    let decls = render(
        json!({"model_registry_scope": SCOPE}),
        &assistant_template(),
    );
    for (i, decl) in decls.into_iter().enumerate() {
        let outcome = mutate(&h, decl).await;
        assert!(
            matches!(outcome, MutationOutcome::Committed { .. }),
            "declaration {i} of the grown assistant was refused: {outcome:?}"
        );
    }

    // The receipt of that commit walks down to the generation, leaves it as the
    // announcement, and the registry writes the rows. Read where they land.
    // Since GH #858 the shell announces its own judge on the same receipts,
    // with no start value (it substitutes nothing, gh302), and not its composer
    // (review rev-L2b2 I-1: born on a local endpoint no catalogue push can
    // land in). The grown
    // member announces its four memory cells, each on its own token as the
    // environment binds it -- `MODEL_JUDGE` unset, so the judge's default.
    let want = vec![
        ("/os/argus/judge".to_string(), String::new()),
        (format!("{GEN}/cogny/brain"), "m-core".to_string()),
        (format!("{GEN}/talky-chat/brain"), "m-surface".to_string()),
        (format!("{GEN}/talky/brain"), "m-surface".to_string()),
        (
            format!("{MEMBER}/memory-hive/closer"),
            "m-closer".to_string(),
        ),
        (
            format!("{MEMBER}/memory-hive/dialectic"),
            "m-dialectic".to_string(),
        ),
        (format!("{MEMBER}/memory-hive/dreamer"), "m".to_string()),
        (
            format!("{MEMBER}/memory-hive/judge"),
            "anthropic/claude-opus-5.5".to_string(),
        ),
    ];
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut got = Vec::new();
    while std::time::Instant::now() < deadline {
        if let Some(db) = find_store_db(root) {
            got = subscriber_rows(&db);
            if got.len() >= want.len() {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert_eq!(
        got, want,
        "the grown generation's three brains and the member's four memory cells are \
         subscribers, with the start values the substitution resolved, beside the shell's own \
         judge"
    );
    h.shutdown().await;
}

// ─────────────────────────────────────────── a push on the shipped road (M-5)

/// Every `llm` cell of the grown tree is inert except ONE brain, which is the
/// real cell pointed at the in-process mock. The claim is about that brain's
/// wire, and no other brain may reach a provider.
struct OneRealBrain {
    path: String,
    base_url: String,
}

impl CellFactory for OneRealBrain {
    fn validate_params(&self, _params: &JsonValue) -> Result<(), String> {
        Ok(())
    }

    fn is_lazy(&self) -> bool {
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        params: JsonValue,
        outputs_tx: mpsc::Sender<meclaw_core::CellEmission>,
        cell_dir: std::path::PathBuf,
        contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        idle_timeout: Option<std::time::Duration>,
        cell_timeout: i64,
        message_timeout: Option<std::time::Duration>,
        blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        if path.as_str() != self.path {
            return Arc::new(InertCellFactory).spawn_cell(
                path,
                params,
                outputs_tx,
                cell_dir,
                contract,
                colony_inbox_tx,
                idle_timeout,
                cell_timeout,
                message_timeout,
                blob_store,
                mailbox_capacity,
            );
        }
        let mut params = params;
        params["base_url"] = json!(self.base_url);
        Arc::new(LlmCellFactory).spawn_cell(
            path,
            params,
            outputs_tx,
            cell_dir,
            contract,
            colony_inbox_tx,
            idle_timeout,
            cell_timeout,
            message_timeout,
            blob_store,
            mailbox_capacity,
        )
    }
}

fn system_text(body: &Value) -> String {
    body["messages"]
        .as_array()
        .and_then(|m| m.first())
        .filter(|m| m["role"] == "system")
        .and_then(|m| m["content"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// Review M-5: a push measured where it lands, on the road the tree ships.
/// The operator posts `override_set target` at the SHELL's rim; the registry
/// resolves it, `meclaw-os` restamps its `update` into `./orgs` as `in_model`,
/// the edge `grow_level assistant` drew carries it onto the generation's
/// `talky`, and the composite's door hands it to the brain. Nothing on that
/// road is drawn by this test. The next provider call of that brain names the
/// replacement's model, and its prompt block is the first thing in the
/// system part.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_push_on_the_shipped_road_reaches_the_grown_brain() {
    if !shipped() {
        return;
    }
    const M2: &str = "test/m2";
    const PROMPT: &str = "P -- the lines test/m2 needs in every system prompt.";
    let brain = format!("{GEN}/talky/brain");
    let mock = MockOpenAI::start(
        (0..12)
            .map(|i| canned_chat_completion(&format!("answer {i}"), "stop"))
            .collect(),
    )
    .await;
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    copy_tree(&repo("examples/organism/seed"), root);
    copy_tree(&repo("templates"), &root.join("templates"));
    // The catalogue row the replacement points at: no endpoint of its own (the
    // brain keeps the one it was born with), a parameter and a prompt block.
    let seed = root.join("templates/llm-registry/store/seed/models.jsonl");
    let mut rows = std::fs::read_to_string(&seed).unwrap();
    if !rows.ends_with('\n') {
        rows.push('\n');
    }
    rows.push_str(
        &json!({"model_id": M2, "provider": "gateway", "base_url": "", "wire_dialect": "",
                "context_window": 32000, "cost_in": 1, "cost_out": 1, "caps": {},
                "traits": {}, "status": "active", "note": "TEST ROW",
                "package": {"max_tokens": 777}, "prompt": PROMPT})
        .to_string(),
    );
    rows.push('\n');
    std::fs::write(&seed, rows).unwrap();
    std::fs::write(
        root.join(".env"),
        "OPENROUTER_API_KEY=test-key\n\
         MODEL_CORE=m-core\n\
         MODEL_CORE_FAST=m-core-fast\n\
         MODEL_SURFACE=m-surface\n\
         MODEL_CLOSER=m\nMODEL_DIALECTIC=m\nMODEL_DREAMER=m\nMODEL_BRAIN=m\n",
    )
    .unwrap();
    let real: Arc<dyn CellFactory> = Arc::new(OneRealBrain {
        path: brain.clone(),
        base_url: format!("{}/v1", mock.base_url),
    });
    let fs: Vec<(String, Arc<dyn CellFactory>)> = factories(root)
        .into_iter()
        .map(|(t, f)| {
            if t == "llm" {
                (t, real.clone())
            } else {
                (t, f)
            }
        })
        .collect();
    let h = ColonyHandle::new_with_factories_at(&td, fs.clone());
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in fs {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(root, &registry, &h.runtime())
        .await
        .expect("the empty seed boots");
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: root.join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan");
    ack_rx.await.expect("rescan ack").expect("rescan ran");
    for file in GROW {
        let outcome = mutate(&h, read_json(&repo(file))).await;
        assert!(
            matches!(outcome, MutationOutcome::Committed { .. }),
            "{file}: {outcome:?}"
        );
    }
    for decl in render(
        json!({"model_registry_scope": SCOPE}),
        &assistant_template(),
    ) {
        let outcome = mutate(&h, decl).await;
        assert!(
            matches!(outcome, MutationOutcome::Committed { .. }),
            "{outcome:?}"
        );
    }
    // The subscriber row first: the replacement is refused for a path nobody
    // subscribed.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if find_store_db(root).is_some_and(|db| subscriber_rows(&db).len() >= 3) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    // The operator, at the shell's rim.
    let cmd = json!({"op": "override_set", "scope": "target", "match": brain, "model_id": M2});
    h.send(
        MessageBuilder::new(Path::new("/os"))
            .hop(json!({"route": "in_hand"}).as_object().cloned().unwrap())
            .body(Body::Inline(json!({"messages": [
                {"origin": "assistant", "type": "tool_call", "id": "op1",
                 "text": cmd.to_string()}]})))
            .ttl(200)
            .build(),
    )
    .await;

    // A turn at the brain, until the push has landed: the push and
    // the turn travel different roads, so the first turns may still see the
    // start value. What is claimed is where the push ENDS.
    let mut last = Value::Null;
    let mut moved = false;
    for _ in 0..10 {
        let before = mock.recorded_requests().await.len();
        // Sent as the composite's own collector, the one sender the seal lets
        // reach the brain; what is measured is the brain's wire, not the turn.
        h.send_from(
            Path::new(&format!("{GEN}/talky/collector")),
            MessageBuilder::new(Path::new(&brain))
                .body(Body::Inline(json!({"messages": [
                    {"origin": "user", "type": "text", "text": "which model?"}]})))
                .ttl(200)
                .build(),
        )
        .await;
        let wait = std::time::Instant::now() + Duration::from_secs(30);
        while mock.recorded_requests().await.len() == before && std::time::Instant::now() < wait {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let reqs = mock.recorded_requests().await;
        if reqs.len() == before {
            let dead: Vec<String> = h
                .drain_dead_letters()
                .await
                .iter()
                .map(|d| {
                    format!(
                        "{} -> {} ({}): {:?} hop={:?}",
                        d.sender_path.as_str(),
                        d.resolved_target.as_str(),
                        d.original_target.as_str(),
                        d.reason,
                        d.message.headers.hop
                    )
                })
                .collect();
            panic!("the grown brain answered no turn; dead letters: {dead:#?}");
        }
        last = reqs[reqs.len() - 1].body.clone();
        if last["model"] == M2 {
            moved = true;
            break;
        }
        assert_eq!(
            last["model"], "m-surface",
            "before the push: the start value: {last}"
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    assert!(moved, "the push never reached the grown brain: {last}");
    assert_eq!(last["max_tokens"], 777, "and its parameters: {last}");
    assert!(
        system_text(&last).starts_with(PROMPT),
        "the model's prompt block is the FIRST thing in the system part: {:?}",
        system_text(&last)
    );
    h.shutdown().await;
}
