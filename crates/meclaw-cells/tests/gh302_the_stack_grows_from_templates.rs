//! GH #302 — the whole stack grows from templates.
//!
//! WHAT THIS FILE IS
//! =================
//! `examples/organism/` is a colony with **zero cells checked in**: a
//! `colony.json`, one empty root hive, and six declarations. Applied in order
//! they instantiate the four composition levels of this wave and one channel
//! into the THIRD of them — a shell, an organisation, a person, one Telegram
//! channel of that person, and one generation of that person's agent.
//!
//! Since GH #454 the channel is a node in `<member>/channels`, not in
//! `<assistant>/channels`. A channel belongs to the PERSON and the assistant is
//! addressed THROUGH it: the connector's own edge stamps `context.assistant`
//! on the way up, the member's `./assistants` container reads that key to pick
//! a generation, and the answer comes back down `./assistants -> ./channels`
//! guarded on `context.channel_node`. What that buys is measured in
//! [`d_a_second_channel_is_one_instantiation_in_the_member`]: one bot can serve
//! two generations of one person, and a generation swap does not take the chat
//! account with it.
//!
//! This file is the acceptance of GH #302, one named assertion per bullet:
//!
//! | bullet | assertion |
//! |---|---|
//! | created by instantiating templates, not by hand-writing edges | [`a_the_declarations_hand_write_no_edge_that_reaches_inside_a_template`] |
//! | the registry records the true origin at every level | [`b_the_registry_records_the_true_origin_at_every_level`] |
//! | a second assistant is one instantiation with its own parameters | [`c_a_second_assistant_is_one_instantiation_with_its_own_parameters`] |
//! | a second channel is ONE instantiation in the member, and the assistant never learns of it | [`d_a_second_channel_is_one_instantiation_in_the_member`] |
//!
//! WHY THE CELLS ARE INERT
//! =======================
//! Every claim here is structural — which rows the registry gains, which
//! provenance chain each carries, which edges the graph holds, how many segments
//! a grown path has. None of it depends on what a cell does with a message, and
//! two of the cell types this stack instantiates would do something the moment
//! they were spawned for real: the `proxy` of the connector opens a long poll
//! against Telegram, and the `timer`s of the broker, the control loop and the
//! memory hive carry crons. So this file registers one lazy no-op factory per
//! cell type the library names — the same device
//! `crates/meclaw-colony/tests/gh277_the_example_mutations_build_the_same_tree.rs`
//! uses, and for the same reason. The behavioural half of this stack is pinned
//! where the real factories run: `gh302_meclaw_os_shell.rs`,
//! `gh302_org_is_a_namespace.rs`, `gh302_member_holds_the_memory.rs` and
//! `gh302_assistant_wires_channels_once.rs`.
//!
//! `ref` deliberately gets no factory: it is a template-time type, resolved
//! during staging, and a `ref` cell must never reach the registry. Neither does
//! `hive` — a hive is a scope marker, not an actor.
//!
//! Guarded like every other template-reading test (GH #49): the public export
//! ships a subset of the library, and a template that did not travel is skipped
//! rather than judged.

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, RespawnFn, SpawnedCellKind,
    WakeFn, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{JsonValue, Message, Path, Uuid};
use meclaw_testing::ColonyHandle;
use std::collections::BTreeSet;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

// ──────────────────────────────────────────────────────────────────────────────
// the shipped material
// ──────────────────────────────────────────────────────────────────────────────

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The six declarations, in the order an operator applies them: shell, then
/// organisation, then member, then assistant, then channel, then the credential
/// v-lanes (GH #560). Each one carries its own absolute `scope`, so unlike the
/// five of GH #277 they are applied verbatim, scope included — the example is ONE
/// tree, not six side by side.
const DECLARATIONS: [&str; 6] = [
    "examples/organism/grow-os.json",
    "examples/organism/grow-org.json",
    "examples/organism/grow-member.json",
    "examples/organism/grow-assistant.json",
    "examples/organism/grow-channel.json",
    "examples/organism/grow-credentials.json",
];

const ASSISTANT: &str = "/os/orgs/acme/members/alex/assistants/scribe";
const MEMBER: &str = "/os/orgs/acme/members/alex";
/// The channel container. Since GH #454 it stands in the MEMBER — a channel
/// belongs to the person, not to one of their generations.
const CHANNELS: &str = "/os/orgs/acme/members/alex/channels";
/// The one channel the shipped declaration instantiates into it.
const CHANNEL_NODE: &str = "telegram";

/// `examples/organism`, or `None` when this tree did not ship it (GH #49).
fn shipped() -> Option<std::path::PathBuf> {
    let p = repo("examples/organism");
    p.join("grow-os.json").is_file().then_some(p)
}

/// Whether the whole library this example instantiates travelled with the tree.
fn library_is_complete() -> bool {
    [
        "meclaw-os",
        "org",
        "member",
        "assistant",
        "talky",
        "telegram-connector",
        "cogny",
        "collector",
        "access",
    ]
    .iter()
    .all(|n| repo(&format!("templates/{n}/template.json")).is_file())
}

/// A JSON array as a slice, or an empty slice — a missing `add_edges` is a
/// declaration that draws none, not a defect.
fn arr(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The version a shipped `template.json` declares, so that nothing in this file
/// is a literal that a bump can silently falsify.
fn version_of(template: &str) -> String {
    let v = read_json(&repo(&format!("templates/{template}/template.json")));
    v["version"]
        .as_str()
        .unwrap_or_else(|| panic!("templates/{template}/template.json declares no version"))
        .to_string()
}

// ──────────────────────────────────────────────────────────────────────────────
// the inert factory
// ──────────────────────────────────────────────────────────────────────────────

/// A lazy factory that accepts every params block and never runs anything.
///
/// `is_lazy() == true` makes the mutation path register each cell as `Dormant`:
/// a mailbox pair and no task. The `WakeFn` and the `RespawnFn` are reachable
/// only through a delivery or a restart, neither of which this fixture performs;
/// they are written correctly anyway rather than left as `unimplemented!()`,
/// because a panic on either path would take the whole colony task with it (the
/// panic-free hot-path invariant).
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
                // Held for the task's lifetime: dropping it at task end is what
                // tells the watcher the cell died, exactly like a real factory.
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

// ──────────────────────────────────────────────────────────────────────────────
// growing the tree
// ──────────────────────────────────────────────────────────────────────────────

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

/// Every `cell.type` the library names, minus the two that have no factory by
/// design: `hive` and `ref`.
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

/// The colony root: the example's own seed, the real `templates/` library, and
/// an `.env` whose every value is a placeholder. Not one of these tokens is a
/// credential; the example itself carries none either.
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

fn factories(root: &std::path::Path) -> Vec<(String, Arc<dyn CellFactory>)> {
    let types = cell_types_in(&root.join("templates"));
    assert!(
        types.contains("code") && types.contains("store") && types.contains("llm"),
        "the library copy named no real cell types — it failed: {types:?}"
    );
    types
        .into_iter()
        .map(|t| (t, Arc::new(InertCellFactory) as Arc<dyn CellFactory>))
        .collect()
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

/// One registry row, as the persisted `colony.db` holds it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Row {
    path: String,
    template: Option<String>,
    template_version: Option<String>,
    /// The raw JSON of `registry.template_chain`, or `None` for a cell that was
    /// not born from a template.
    template_chain: Option<String>,
}

impl Row {
    /// The chain as `(name, version)` pairs, outermost first.
    fn chain(&self) -> Vec<(String, Option<String>)> {
        let raw = self
            .template_chain
            .as_deref()
            .unwrap_or_else(|| panic!("{}: template-born without a chain", self.path));
        let parsed: Vec<Value> = meclaw_core::serde_json::from_str(raw)
            .unwrap_or_else(|e| panic!("{}: chain is not JSON: {e} ({raw})", self.path));
        parsed
            .iter()
            .map(|e| {
                let name = e
                    .get("template")
                    .or_else(|| e.get(0))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let version = e
                    .get("template_version")
                    .or_else(|| e.get(1))
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                (name, version)
            })
            .collect()
    }
}

/// Everything one application of the declarations leaves behind.
struct Grown {
    /// Kept so the grown tree stays on disk for the length of the assertion.
    _td: tempfile::TempDir,
    root: std::path::PathBuf,
    rows: Vec<Row>,
    edges: Vec<(String, String)>,
}

impl Grown {
    fn row(&self, path: &str) -> &Row {
        self.rows
            .iter()
            .find(|r| r.path == path)
            .unwrap_or_else(|| panic!("no registry row at {path}; rows: {:?}", self.paths()))
    }

    fn paths(&self) -> Vec<&str> {
        self.rows.iter().map(|r| r.path.as_str()).collect()
    }

    /// Edges between the `channels` container and everything outside it — the
    /// fan-in the MEMBER template ships since GH #454, which a second channel
    /// must not move (GH #303's ruling, one level up).
    ///
    /// Written as "one endpoint IS the container, the other is not below it"
    /// rather than against a written-down list of siblings: the member's
    /// occupants are the member template's business, and a list here would have
    /// to be re-derived by hand every time one moves.
    fn channels_to_siblings(&self) -> usize {
        let below = format!("{CHANNELS}/");
        self.edges
            .iter()
            .filter(|(from, to)| {
                (from == CHANNELS && !to.starts_with(&below))
                    || (to == CHANNELS && !from.starts_with(&below))
            })
            .count()
    }

    /// Every edge with at least one endpoint inside the assistant's subtree —
    /// the number GH #454 predicts a second CHANNEL cannot move, because the
    /// assistant has no channels any more.
    fn touching_the_assistant(&self) -> usize {
        let below = format!("{ASSISTANT}/");
        self.edges
            .iter()
            .filter(|(from, to)| {
                from == ASSISTANT
                    || to == ASSISTANT
                    || from.starts_with(&below)
                    || to.starts_with(&below)
            })
            .count()
    }

    /// Edges with at least one endpoint strictly below the `channels` container:
    /// what one channel instantiation costs.
    fn inside_channels(&self) -> usize {
        let prefix = format!("{CHANNELS}/");
        self.edges
            .iter()
            .filter(|(from, to)| from.starts_with(&prefix) || to.starts_with(&prefix))
            .count()
    }
}

/// Boot a fresh colony over a fresh temp root, apply the five shipped
/// declarations plus whatever `extras` the caller adds, read the graph, shut
/// down cleanly (which flushes the write buffer) and read the registry back.
///
/// NOTE on the direct SQL: this is test-side reading of `colony.db`, not cell
/// code. The database-isolation rule binds cells; a fixture that measures what
/// the substrate persisted has to read what the substrate persisted.
async fn grow(extras: Vec<Value>) -> Grown {
    let td = tempfile::TempDir::new().unwrap();
    build_root(td.path());
    let h = boot(&td).await;

    for file in DECLARATIONS {
        let outcome = mutate(&h, read_json(&repo(file))).await;
        assert!(
            matches!(outcome, MutationOutcome::Committed { .. }),
            "{file} was not committed: {outcome:?}"
        );
    }
    for (i, extra) in extras.into_iter().enumerate() {
        let outcome = mutate(&h, extra).await;
        assert!(
            matches!(outcome, MutationOutcome::Committed { .. }),
            "extra mutation #{i} was not committed: {outcome:?}"
        );
    }

    let (ack_tx, ack_rx) = oneshot::channel::<meclaw_colony::api_dto::ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: Path::new("/"),
            ack: ack_tx,
        })
        .await
        .unwrap();
    let edges: Vec<(String, String)> = ack_rx
        .await
        .unwrap()
        .edges
        .iter()
        .map(|e| (e.from.to_string(), e.to.to_string()))
        .collect();

    h.shutdown().await;

    let conn = rusqlite::Connection::open_with_flags(
        td.path().join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open colony.db read-only");
    let mut stmt = conn
        .prepare(
            "SELECT path, template, template_version, template_chain \
             FROM registry ORDER BY path",
        )
        .unwrap();
    let rows: Vec<Row> = stmt
        .query_map([], |r| {
            Ok(Row {
                path: r.get(0)?,
                template: r.get(1)?,
                template_version: r.get(2)?,
                template_chain: r.get(3)?,
            })
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    drop(stmt);
    drop(conn);

    let root = td.path().join("main");
    Grown {
        _td: td,
        root,
        rows,
        edges,
    }
}

/// The sixth mutation of GH #302's third bullet: a second assistant under the
/// same member, derived from the shipped declaration by changing the name and
/// the parameter — which is exactly the edit an operator makes.
fn second_assistant() -> Value {
    let raw = std::fs::read_to_string(repo("examples/organism/grow-assistant.json")).unwrap();
    let mut v: Value = meclaw_core::serde_json::from_str(&raw.replace("scribe", "aide")).unwrap();
    v["diff"]["add_nodes"][0]["override_params"]["cogny/brain"]["temperature"] = json!(0.9);
    v
}

/// The second channel of the SAME person: ONE instantiation into the member's
/// `channels` container, one mutation, derived from the shipped declaration the
/// way an operator derives it — the node name changes, the second bot brings its
/// own token, and the channel name it stamps and guards on changes with it.
///
/// Nothing about the assistant appears in this edit, and that is the point of
/// GH #454: a channel is wired to the container it stands in, and the generation
/// it reaches is named in `context.assistant`, not in an endpoint.
fn second_channel() -> Value {
    let mut v = read_json(&repo("examples/organism/grow-channel.json"));
    // Since GH #503 the declaration stands AT `channels`, so the node is named
    // bare and its endpoint is `./<name>`. The path in the tree is the same one.
    let old_node = format!("./{CHANNEL_NODE}");
    let new_node = format!("./{SECOND_CHANNEL_NODE}");
    v["diff"]["add_nodes"][0]["name"] = json!(SECOND_CHANNEL_NODE);
    v["diff"]["add_nodes"][0]["override_params"] = json!({"bot_token": "${TELEGRAM_BOT_TOKEN_2}"});

    // The channel NAME is a value, not a path: it is stamped by the connector's
    // own edge and read back by the guard on the way down. Both spellings move
    // together or the answer would come back to the wrong bot.
    let old_name = format!("'{CHANNEL_NODE}'");
    let new_name = format!("'{SECOND_CHANNEL_NODE}'");
    for edge in v["diff"]["add_edges"].as_array_mut().unwrap() {
        for side in ["from", "to"] {
            if edge[side].as_str() == Some(old_node.as_str()) {
                edge[side] = json!(new_node);
            }
        }
        if let Some(c) = edge["condition"].as_str() {
            edge["condition"] = json!(c.replace(&old_name, &new_name));
        }
        if let Some(ch) = edge["modifier"]["set_context"]["channel"].as_str()
            && ch == old_name
        {
            edge["modifier"]["set_context"]["channel"] = json!(new_name);
        }
    }
    v
}

/// The name of the second channel — a second bot of the same person, reaching
/// the same assistant.
const SECOND_CHANNEL_NODE: &str = "telegram-2";

// ══════════════════════════════════════════════════ (a) no hand-written edge

/// GH #302, first bullet: *created by instantiating templates, not by
/// hand-writing edges.*
///
/// The measurement is on the FILES, and it is sharper than "few edges": every
/// endpoint of every `add_edges` entry in the six declarations resolves either
/// to the root path of a node the SAME diff instantiates, or to the **open
/// container** that node is instantiated into — the address the level above
/// ships for precisely this purpose (`orgs`, `members`, `assistants`,
/// `channels`). Not one endpoint names anything else, so not one reaches into an
/// interior that belongs to another template: every edge inside a level came
/// WITH that level.
#[test]
fn a_the_declarations_hand_write_no_edge_that_reaches_inside_a_template() {
    let Some(_) = shipped() else { return };

    let mut edges_seen = 0usize;
    for file in DECLARATIONS {
        let decl = read_json(&repo(file));
        let scope = decl["scope"]
            .as_str()
            .unwrap_or_else(|| panic!("{file}: no scope"))
            .to_string();
        let born: Vec<String> = arr(&decl["diff"]["add_nodes"])
            .iter()
            .filter_map(|n| n["name"].as_str().map(str::to_string))
            .collect();
        if born.is_empty() {
            // GH #559/#560: a V-LANE declaration instantiates nothing and
            // reaches into an interior ON PURPOSE — that is the whole of what a
            // v-lane is, and the rule above would refuse the one edge form the
            // substrate now sanctions. It is not unchecked: the connect point is
            // the TARGET template's own `at`, judged by Stage 6 at the mutation
            // door and statically by `scripts/check_tree_rules.py` R5, and the
            // refusal is measured in
            // `gh560_a_members_brain_gets_its_sealed_key.rs`. What this file
            // still asserts about such a declaration is that it declares itself
            // as one: every edge in it names its lane.
            let named = arr(&decl["diff"]["add_edges"]);
            assert!(
                !named.is_empty()
                    && named
                        .iter()
                        .all(|e| e["lane"].as_str().is_some_and(|l| !l.is_empty())),
                "{file}: a declaration that instantiates nothing and is not a \
                 v-lane declaration either — an edge that names no lane may not \
                 reach into another template's interior"
            );
            edges_seen += named.len();
            continue;
        }

        // The two addresses a level's own instantiation is allowed to name: the
        // node it creates, and the OPEN CONTAINER the level above ships for
        // exactly this purpose (`orgs`, `members`, `assistants`, `channels`) —
        // which is the parent of that node. Anything else would be an edge
        // reaching into an interior that belongs to another template.
        let mut allowed: BTreeSet<String> = BTreeSet::new();
        for n in &born {
            let p = Path::resolve(&Path::new(&scope), n);
            allowed.insert(p.parent().as_str().to_string());
            allowed.insert(p.as_str().to_string());
        }

        for edge in arr(&decl["diff"]["add_edges"]) {
            edges_seen += 1;
            // GH #562 — the one endpoint that may go deeper, and it says so on
            // itself: an edge naming a `lane` is a v-lane (ADR-0020), and a
            // v-lane ends on an occupant of the node being born, at an address
            // that node's OWN contract named under `at`. So the permission
            // still comes from the template rather than from the hand that
            // writes the declaration — which is the whole of what this test is
            // about — it is just pronounced in the contract instead of implied
            // by the shape of the endpoint. An edge with no `lane` is judged
            // exactly as before.
            let v_lane = edge.get("lane").and_then(|l| l.as_str()).is_some();
            for side in ["from", "to"] {
                let raw = edge[side]
                    .as_str()
                    .unwrap_or_else(|| panic!("{file}: an edge without a {side}"));
                let resolved = Path::resolve(&Path::new(&scope), raw).as_str().to_string();
                let inside_a_born_node = v_lane
                    && born.iter().any(|n| {
                        let p = Path::resolve(&Path::new(&scope), n);
                        resolved.starts_with(&format!("{}/", p.as_str()))
                    });
                assert!(
                    allowed.contains(&resolved) || inside_a_born_node,
                    "{file}: the edge endpoint {raw:?} resolves to {resolved}, which is \
                     neither a node this diff instantiates nor the open container it is \
                     instantiated into. An endpoint anywhere else reaches into an interior \
                     that belongs to another template — and every internal edge of these \
                     levels came WITH its template, or the edge names the `lane` that makes \
                     it a v-lane. Allowed here: {allowed:?}"
                );
            }
        }
    }
    assert!(
        edges_seen > 0,
        "the six declarations draw no edge at all — the assertion would be vacuous"
    );
}

/// The other half of the same bullet, so that "no hand-written internal edge"
/// cannot be satisfied by a stack that instantiates nothing: the five
/// declarations name the four levels and the one node a channel is, by
/// PINNED reference, and each pin resolves against the tree it ships with.
///
/// A channel used to be TWO nodes here — a connector and a talky beside it,
/// paired by fifteen hand-written edges. Since GH #454 the conversation surface
/// ships INSIDE `assistant@2.0.0` (`<assistant>/surface`, a ref on talky), so
/// the channel declaration instantiates the connector and nothing else. `talky`
/// therefore no longer appears in this list, while still standing in the grown
/// tree — which is exactly the difference between what a declaration names and
/// what a template carries.
#[test]
fn a_the_five_declarations_name_the_whole_stack_by_pinned_reference() {
    let Some(_) = shipped() else { return };
    if !library_is_complete() {
        return;
    }

    let mut named: Vec<String> = Vec::new();
    for file in DECLARATIONS {
        let decl = read_json(&repo(file));
        for node in arr(&decl["diff"]["add_nodes"]) {
            let reference = node["template"]
                .as_str()
                .unwrap_or_else(|| panic!("{file}: a node without a template"))
                .to_string();
            let (short, version) = reference.split_once('@').unwrap_or_else(|| {
                panic!(
                    "{file}: {reference:?} is a bare name. A bare name resolves to the \
                     highest version on disk, and a tree that silently adopts a newer \
                     level is the drift `registry.template_chain` exists to make visible \
                     (templates/org/README.md § Instantiating one)"
                )
            });
            assert_eq!(
                version_of(short),
                version,
                "{file} pins {reference}, but the tree ships {short}@{}",
                version_of(short)
            );
            named.push(short.to_string());
        }
    }
    named.sort();
    assert_eq!(
        named,
        vec![
            "assistant".to_string(),
            "meclaw-os".to_string(),
            "member".to_string(),
            "org".to_string(),
            "telegram-connector".to_string(),
        ],
        "the stack is the four levels plus the ONE node a channel is (GH #454). \
         `talky` is not named by any declaration any more: it arrives as \
         `<assistant>/surface`, a ref of `assistant@2.0.0`."
    );
}

// ══════════════════════════════════════════════ (b) origin at every level

/// GH #302, second bullet: *the registry records the true origin at every
/// level.*
///
/// Three statements, and the third is the one the bullet is about:
///
/// 1. `template` and `template_chain` are coupled per row — a row has both or
///    neither. A row that claims a template without a chain would be the state
///    GH #277 closed.
/// 2. The connector's `proxy` cell is stamped with its OWN template and version.
///    It is instantiated directly by the channel declaration, so its chain is a
///    one-element chain: the leaf stamp IS the whole origin, and the assertion
///    says so rather than pretending the level names are in there. (`config.rs`,
///    `NodeProvenance::template_chain`: *"a node instantiated from a ref-free
///    template carries a one-element chain"*.)
/// 3. A node that arrived through `ref`s carries the outer levels, **outermost
///    first**, its own template last — three deep inside the assistant
///    (`assistant` → `cogny` → `collector`) and two deep inside the shell
///    (`meclaw-os` → `access`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn b_the_registry_records_the_true_origin_at_every_level() {
    let Some(_) = shipped() else { return };
    if !library_is_complete() {
        return;
    }
    let grown = grow(vec![]).await;

    // (1) coupled per row.
    let mismatched: Vec<&str> = grown
        .rows
        .iter()
        .filter(|r| r.template.is_some() != r.template_chain.is_some())
        .map(|r| r.path.as_str())
        .collect();
    assert!(
        mismatched.is_empty(),
        "template and template_chain disagree about which rows are template-born — \
         chained without a template, or born without a chain: {mismatched:?}"
    );
    assert!(
        grown.rows.iter().all(|r| r.template.is_some()),
        "the seed checks in ZERO cells, so every registry row of this colony is \
         template-born: {:?}",
        grown
            .rows
            .iter()
            .filter(|r| r.template.is_none())
            .map(|r| &r.path)
            .collect::<Vec<_>>()
    );

    // (2) the connector's proxy cell. Since GH #454 it stands in the MEMBER's
    // container, which is the whole of what the move changed for the registry:
    // the row's path lost the assistant, and its chain never had one.
    let connector = grown.row(&format!("{CHANNELS}/{CHANNEL_NODE}"));
    assert_eq!(connector.template.as_deref(), Some("telegram-connector"));
    assert_eq!(
        connector.template_version.as_deref(),
        Some(version_of("telegram-connector").as_str())
    );
    let grown_config = read_json(
        &grown
            .root
            .join(format!("os/orgs/acme/members/alex/channels/{CHANNEL_NODE}"))
            .join("config.json"),
    );
    assert_eq!(
        grown_config["cell"]["type"].as_str(),
        Some("proxy"),
        "the connector is ONE cell since telegram-connector@2.0.0 (GH #303), and the \
         stamped row is that cell's"
    );
    assert!(
        !grown
            .paths()
            .iter()
            .any(|p| p.starts_with(&format!("{ASSISTANT}/channels"))),
        "a channel row stands under the assistant — since GH #454 `channels` is the \
         MEMBER's container: {:?}",
        grown.paths()
    );
    assert_eq!(
        connector.chain(),
        vec![(
            "telegram-connector".to_string(),
            Some(version_of("telegram-connector"))
        )],
        "the connector is instantiated directly by the channel declaration, from a \
         ref-free template: its chain is one element and the leaf stamp is the whole \
         origin. An outer level would appear here only if a composite had placed it."
    );

    // (3) the outer levels, outermost first. `cogny/collector` is a HIVE and
    // therefore has no registry row of its own — a hive is a scope marker, not
    // an actor — so the deepest CELL of that ref chain is the one that carries
    // the stamp.
    let deep = grown.row(&format!("{ASSISTANT}/cogny/collector/assemble"));
    assert_eq!(
        deep.chain(),
        vec![
            ("assistant".to_string(), Some(version_of("assistant"))),
            ("cogny".to_string(), Some(version_of("cogny"))),
            ("collector".to_string(), Some(version_of("collector"))),
        ],
        "three levels of origin, outermost first, the node's own template last. This is \
         the question GH #277 could not answer and GH #302 asks at every level: an update \
         addressing `assistant` finds this node through the first hop, one addressing \
         `collector` through the last."
    );
    assert_eq!(
        deep.template.as_deref(),
        deep.chain().last().map(|(n, _)| n.as_str()),
        "the leaf stamp is a projection of the chain's last element"
    );

    let broker = grown.row("/os/access/store");
    assert_eq!(
        broker
            .chain()
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>(),
        vec!["meclaw-os", "access"],
        "the shell's broker is a ref of `meclaw-os`, and the row says so"
    );
}

// ═══════════════════════════════════ (c) a second assistant, own parameters

/// GH #302, third bullet: *a second assistant is one instantiation with its own
/// parameters.*
///
/// One mutation, derived from the shipped declaration by changing the name and
/// the parameter. Nothing else moves: the member's own edges to `./assistants`
/// are the member template's and are not re-drawn, and both agents are stamped
/// with the same `assistant` version.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c_a_second_assistant_is_one_instantiation_with_its_own_parameters() {
    let Some(_) = shipped() else { return };
    if !library_is_complete() {
        return;
    }

    let first = read_json(&repo("examples/organism/grow-assistant.json"));
    let second = second_assistant();
    assert_ne!(
        first["diff"]["add_nodes"][0]["override_params"],
        second["diff"]["add_nodes"][0]["override_params"],
        "the second assistant is only a second instantiation if it carries its OWN \
         parameters"
    );
    assert_eq!(
        second["diff"]["add_nodes"].as_array().map(Vec::len),
        Some(1),
        "one instantiation, not a tree written out"
    );

    let grown = grow(vec![second]).await;

    // An assistant is a HIVE, so the level itself owns no registry row. What
    // says "this cell is part of an assistant" is the first element of its
    // provenance chain — which is precisely the origin-at-every-level property
    // of the second bullet, used here to identify two instances of one level.
    let a = grown.row(&format!("{ASSISTANT}/cogny/brain"));
    let b = grown.row(&format!("{MEMBER}/assistants/aide/cogny/brain"));
    let origin = |r: &Row| r.chain().first().cloned().unwrap();
    assert_eq!(
        origin(a),
        ("assistant".to_string(), Some(version_of("assistant"))),
        "the first agent is an instance of the assistant level"
    );
    assert_eq!(
        origin(a),
        origin(b),
        "both agents are instances of the SAME level at the SAME version — what \
         differs between them is parameters, not a second hand-built tree"
    );

    // The parameters really are the instance's own: read them back off the two
    // grown trees rather than off the declaration that asked for them.
    let temperature = |agent: &str| -> Value {
        read_json(
            &grown
                .root
                .join(format!(
                    "os/orgs/acme/members/alex/assistants/{agent}/cogny/brain"
                ))
                .join("config.json"),
        )["params"]["temperature"]
            .clone()
    };
    assert_ne!(
        temperature("scribe"),
        temperature("aide"),
        "both agents were grown from one template and the override did not reach the \
         tree — an override that commits and does nothing is the R10 defect"
    );

    // The member wired the CONTAINER once; a second agent adds only its own
    // doors and exits, never a second copy of the member's fan-in.
    let member_fan_in = grown
        .edges
        .iter()
        .filter(|(from, to)| {
            from == &format!("{MEMBER}/assistants") || to == &format!("{MEMBER}/assistants")
        })
        .filter(|(from, to)| {
            !from.starts_with(&format!("{MEMBER}/assistants/"))
                && !to.starts_with(&format!("{MEMBER}/assistants/"))
        })
        .count();
    // Read off `templates/member/config.json` rather than written down here, so
    // the live number and the shipped number are one statement.
    let declared =
        arr(&read_json(&repo("templates/member/config.json"))["params"]["graph"]["edges"])
            .iter()
            .filter(|e| e["from"] == json!("./assistants") || e["to"] == json!("./assistants"))
            .count();
    assert_eq!(
        member_fan_in, declared,
        "the live tree draws a different number of member-to-container edges than the \
         `member` template on disk declares"
    );
    assert_eq!(
        declared, 27,
        "the member's own edges to and from its assistants container are the member \
         template's, drawn ONCE at member instantiation. ELEVEN reach the container: the \
         screened turn coming back off ./firewall, the memory hive's bundle \u{2014} as the \
         DEFAULT since GH #533, so a bundle addressed to the asker OUTSIDE the member takes \
         the level's own exit instead \u{2014} the memory hive's REFUSAL of a recall an asker \
         inside raised, re-stamped to `in_bundle` and told from the hive's other refusals by \
         the same reply-to token (GH #533), the \
         builder's answer since GH #425, since GH #459 a screen's `event` and `receipt` \
         off ./channels, re-stamped to `in_turn` with the lane on `hop.kind`, because the \
         assistant level has no event lane and did not grow one, and \u{2014} since GH #475 \
         \u{2014} the two transfer lanes `in_export` and `in_import`, carried in plain, so \
         that the session ledger of a NAMED generation can leave and come back, and \
         \u{2014} since GH #552 \u{2014} the memory's own two answers, `tool_result` \
         re-stamped to `in_tool` and `tool_schemas` re-stamped to `in_menu` carrying \
         `context.tool_answerer = 'memory'`, and \u{2014} since GH #553 \u{2014} the mutation \
         door's receipt on `mutation_committed`, which the level carries into BOTH of its \
         containers so that an agent's tool menu and a person's screen follow a graph \
         change instead of asking a timer for it. SIXTEEN \
         leave it: recall, extraction, write, turn_write, prune, error, `build`, the \
         second fan-out of `write` that fires the close pass into the memory hive since \
         GH #447, the second fan-out of `turn_write` that writes the EPISODE into that same \
         hive since GH #527 \u{2014} the only path in this substrate from a conversation into \
         an `episodes` table, and the one this level declined until then \u{2014} the TWO exits \
         an `answer` has since GH #454 (one down to `./channels` \
         guarded on `context.channel_node`, one out at the rim as the guarded default for a \
         turn that arrived through the member's own door), `pack_ack` since GH #458 \
         \u{2014} the receipt of an identity ./affinity pushed INTO a generation, which nothing \
         at this level consumes and nothing at this level can \u{2014} and, since GH #555, \
         the TWO receipt lanes of that generation's session keeper: `export_done`, which \
         says the keeper wrote its own ledger out and where, and `dump`, the receipt of one \
         applied import part. Neither is consumed here any more \u{2014} the cell that read \
         a transfer document at this level is gone with the ruling that gave every store \
         its own files \u{2014} plus \u{2014} since GH #552 \u{2014} the memory \
         road's other half: `tool`, on the one tool name that leaves a generation, and \
         `schemas`, the menu tick that asks what it looks like. The push itself draws no \
         edge here: producer and \
         consumer are siblings, so it addresses \
         `<member>/assistants/<agent>` at its own path. A second agent must not move this \
         number \u{2014} that is what makes it one instantiation."
    );
}

// ═══════════════ (d) a second channel, in the member, invisible to the agent

/// GH #302's fourth bullet, re-cut by GH #454: *a second channel is ONE
/// instantiation into the MEMBER's `channels` container, still one mutation,
/// with no intermediate hive — and the assistant does not learn of it.*
///
/// The bullet used to read "two instantiations": a channel was a connector and
/// a talky beside it, standing in `<assistant>/channels`. Both halves of that
/// moved. The conversation surface came home into `assistant@2.0.0`
/// (`<assistant>/surface`), so a channel is ONE node; and the container went up
/// a level to the person, so a second bot is a second way to REACH the same
/// generations rather than a second thing inside one of them.
///
/// Four measurements:
///
/// 1. The mutation is ONE and it instantiates exactly ONE node.
/// 2. The resulting path has **no intermediate hive**: the node is a direct
///    child of the container, two segments below the member
///    (`channels/<node>`) — and NOTHING of it stands below the assistant.
/// 3. The nine edges between `./channels` and the rest of the member are the
///    MEMBER template's and do not move — three from GH #454 and six more from
///    GH #455/#459, when a screen became one of these channels; the second
///    channel costs exactly its own wiring again.
/// 4. **The assistant never learns of it.** Every edge touching the assistant's
///    subtree is identical in count before and after, because a channel is
///    wired to the container it stands in and names the generation it reaches
///    in `context.assistant` instead of in an endpoint. This is the assertion
///    that would have been impossible to write before GH #454: a second channel
///    used to cost the assistant its whole fan-in again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn d_a_second_channel_is_one_instantiation_in_the_member() {
    let Some(_) = shipped() else { return };
    if !library_is_complete() {
        return;
    }

    let decl = second_channel();
    assert_eq!(
        decl["diff"]["add_nodes"].as_array().map(Vec::len),
        Some(1),
        "a channel is ONE node since GH #454 — the connector. The talky beside it became \
         `<assistant>/surface` and is not a channel's business any more"
    );
    assert_eq!(
        decl["scope"].as_str(),
        Some(format!("{MEMBER}/channels").as_str()),
        "a channel is instantiated into the PERSON's own channel container, not \
         into one of their generations. Since GH #503 the declaration stands AT \
         that container: the node lands at the same path either way, and the \
         scope root is the address the broker judges"
    );
    assert_eq!(
        arr(&decl["diff"]["add_edges"]).len(),
        3,
        "three edges and no more: the raw turn up (stamping route, channel, chat, user, \
         audience and the assistant it is addressed to), the connector's error up, and the \
         answer back down guarded on `context.channel_node`"
    );

    let one = grow(vec![]).await;
    let two = grow(vec![decl]).await;

    // (2) no intermediate hive, and nothing under the assistant.
    let path = format!("{CHANNELS}/{SECOND_CHANNEL_NODE}");
    assert!(
        two.rows.iter().any(|r| r.path == path)
            || two
                .rows
                .iter()
                .any(|r| r.path.starts_with(&format!("{path}/"))),
        "the second channel is not at {path}; rows: {:?}",
        two.paths()
    );
    let from_member: Vec<&str> = path
        .strip_prefix(&format!("{MEMBER}/"))
        .unwrap()
        .split('/')
        .collect();
    assert_eq!(
        from_member,
        vec!["channels", SECOND_CHANNEL_NODE],
        "a channel node is a DIRECT child of the member's container. A third segment here \
         is the retired `channel` hive coming back (GH #303); an `assistants/<agent>` \
         prefix is the level GH #454 took it out of."
    );
    assert!(
        !two.paths()
            .iter()
            .any(|p| p.starts_with(&format!("{ASSISTANT}/channels"))),
        "the second channel landed under the assistant: {:?}",
        two.paths()
    );

    // (3) the fan-in is the member template's, drawn once. Read off
    // `templates/member/config.json` rather than written down here, so the live
    // number and the shipped number are one statement.
    let declared =
        arr(&read_json(&repo("templates/member/config.json"))["params"]["graph"]["edges"])
            .iter()
            .filter(|e| e["from"] == json!("./channels") || e["to"] == json!("./channels"))
            .count();
    assert_eq!(
        declared, 9,
        "the member ships nine edges between `./channels` and the rest of itself. Three \
         are the chat channel's own, from GH #454: the screened turn into `./firewall`, \
         the answer coming back from `./assistants` guarded on `context.channel_node`, and \
         the connector's error out at the rim. Six more arrived with GH #455/#459, when \
         a SCREEN became one of these channels: `./apps -> ./channels` carries an app's \
         `view` towards the display named in the edge that leaves the app, and the way \
         back off the screen is split on the OWNER the display stamped — `event` and \
         `receipt` into `./assistants` for `hop.owner.contains('/assistants/')`, the \
         same pair into `./apps` for `'/apps/'`, and one catch-all out at the rim on \
         `error` for an owner this level cannot place, carrying the original lane on \
         `hop.kind`. A second channel must not move any of them \
         (templates/member/README.md § Why a container carries no contract). Move the \
         README with the number."
    );
    assert_eq!(
        one.channels_to_siblings(),
        declared,
        "the live tree draws a different number of container-to-member edges than the \
         `member` template on disk declares"
    );
    assert_eq!(
        one.channels_to_siblings(),
        two.channels_to_siblings(),
        "a second channel moved the fan-in between ./channels and the rest of the member \
         from {} to {}. #303's whole ruling — one level up since #454 — is that this \
         number is a property of the TEMPLATE and is drawn once.",
        one.channels_to_siblings(),
        two.channels_to_siblings()
    );
    assert_eq!(
        two.inside_channels(),
        one.inside_channels() * 2,
        "a second channel costs exactly its own wiring again: {} -> {}",
        one.inside_channels(),
        two.inside_channels()
    );

    // (4) the assistant learns nothing.
    assert_eq!(
        one.touching_the_assistant(),
        two.touching_the_assistant(),
        "a second channel moved the assistant's edge count from {} to {}. Since GH #454 a \
         channel is wired to the member's container and names the generation it reaches in \
         `context.assistant`, so an agent's own topology is not a function of how many \
         bots the person owns — which is the whole reason the level moved.",
        one.touching_the_assistant(),
        two.touching_the_assistant()
    );
    assert!(
        one.touching_the_assistant() > 0,
        "the assistant has no edges at all — the assertion above would be vacuous"
    );
}

/// How the numbers in `examples/organism/README.md` are re-measured. Not a
/// check — a readout, so that moving a stated number is one command and a look,
/// never a guess:
///
/// ```text
/// cargo test -p meclaw-cells --test gh302_the_stack_grows_from_templates \
///     print_the_measurement -- --ignored --nocapture
/// ```
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn print_the_measurement() {
    let grown = grow(vec![]).await;
    println!("registry rows      = {}", grown.rows.len());
    println!("edges              = {}", grown.edges.len());
    println!("channels<->member  = {}", grown.channels_to_siblings());
    println!("inside channels    = {}", grown.inside_channels());
    println!("touching assistant = {}", grown.touching_the_assistant());
    let mut declared = 0usize;
    for file in DECLARATIONS {
        let d = read_json(&repo(file));
        println!(
            "{file}: {} nodes, {} edges",
            arr(&d["diff"]["add_nodes"]).len(),
            arr(&d["diff"]["add_edges"]).len()
        );
        declared += arr(&d["diff"]["add_edges"]).len();
    }
    println!("declared edges     = {declared}");
    let distinct: BTreeSet<&str> = grown
        .rows
        .iter()
        .filter_map(|r| r.template.as_deref())
        .collect();
    println!("distinct templates = {} {:?}", distinct.len(), distinct);
}
