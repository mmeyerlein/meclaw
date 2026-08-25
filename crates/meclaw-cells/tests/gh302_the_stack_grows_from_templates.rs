//! GH #302 — the whole stack grows from templates.
//!
//! WHAT THIS FILE IS
//! =================
//! `examples/organism/` is a colony with **zero cells checked in**: a
//! `colony.json`, one empty root hive, and five declarations. Applied in order
//! they instantiate the four composition levels of this wave and one channel
//! into the innermost of them — a shell, an organisation, a person, one
//! generation of that person's agent, and a Telegram surface for it.
//!
//! This file is the acceptance of GH #302, one named assertion per bullet:
//!
//! | bullet | assertion |
//! |---|---|
//! | created by instantiating templates, not by hand-writing edges | [`a_the_declarations_hand_write_no_edge_that_reaches_inside_a_template`] |
//! | the registry records the true origin at every level | [`b_the_registry_records_the_true_origin_at_every_level`] |
//! | a second assistant is one instantiation with its own parameters | [`c_a_second_assistant_is_one_instantiation_with_its_own_parameters`] |
//! | a second channel is two instantiations into `channels`, no intermediate hive | [`d_a_second_channel_is_two_instantiations_and_no_intermediate_hive`] |
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

/// The five declarations, in the order an operator applies them: shell, then
/// organisation, then member, then assistant, then channel. Each one carries its
/// own absolute `scope`, so unlike the five of GH #277 they are applied verbatim,
/// scope included — the example is ONE tree, not five side by side.
const DECLARATIONS: [&str; 5] = [
    "examples/organism/grow-os.json",
    "examples/organism/grow-org.json",
    "examples/organism/grow-member.json",
    "examples/organism/grow-assistant.json",
    "examples/organism/grow-channel.json",
];

const ASSISTANT: &str = "/os/orgs/acme/members/alex/assistants/scribe";
const MEMBER: &str = "/os/orgs/acme/members/alex";
const CHANNELS: &str = "/os/orgs/acme/members/alex/assistants/scribe/channels";

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
         MODEL_CLOSER=gpt-4o-mock\n\
         MODEL_DIALECTIC=gpt-4o-mock\n\
         MODEL_DREAMER=gpt-4o-mock\n\
         TELEGRAM_BOT_TOKEN=test-token\n\
         TELEGRAM_BOT_TOKEN_2=test-token-2\n\
         TELEGRAM_ALLOWED_USER_ID=0\n\
         EXAMPLE_CHAT_TOKEN=test-chat-token\n\
         KEEPER_NIGHT_CRON=0 0 0 1 1 *\n",
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
    ack_rx.await.expect("rescan ack");
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

    /// Edges between the `channels` container and its siblings — the fan-in
    /// `assistant@1.0.0` ships, which a second channel must not move (GH #303).
    fn channels_to_siblings(&self) -> usize {
        let siblings = [
            ASSISTANT,
            "/os/orgs/acme/members/alex/assistants/scribe/cogny",
            "/os/orgs/acme/members/alex/assistants/scribe/tools",
        ];
        self.edges
            .iter()
            .filter(|(from, to)| {
                (from == CHANNELS && siblings.contains(&to.as_str()))
                    || (to == CHANNELS && siblings.contains(&from.as_str()))
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
    v["diff"]["add_nodes"][0]["override_params"]["cogny/brain"]["temperature"] = json!("0.9");
    v
}

/// A second channel into the same `channels` container: two instantiations and
/// their pairing edges, one mutation, derived the same way — the two node names
/// change, the second bot brings its own token, and the topology is untouched.
fn second_channel() -> Value {
    let mut v = read_json(&repo("examples/organism/grow-channel.json"));
    v["diff"]["add_nodes"][0]["name"] = json!("channels/telegram-connector-2");
    v["diff"]["add_nodes"][0]["override_params"] = json!({"bot_token": "${TELEGRAM_BOT_TOKEN_2}"});
    v["diff"]["add_nodes"][1]["name"] = json!("channels/talky-2");
    for edge in v["diff"]["add_edges"].as_array_mut().unwrap() {
        for side in ["from", "to"] {
            let renamed = match edge[side].as_str().unwrap_or_default() {
                "./channels/telegram-connector" => "./channels/telegram-connector-2",
                "./channels/talky" => "./channels/talky-2",
                other => other,
            }
            .to_string();
            edge[side] = json!(renamed);
        }
    }
    v
}

// ══════════════════════════════════════════════════ (a) no hand-written edge

/// GH #302, first bullet: *created by instantiating templates, not by
/// hand-writing edges.*
///
/// The measurement is on the FILES, and it is sharper than "few edges": every
/// endpoint of every `add_edges` entry in the five declarations resolves either
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
        assert!(
            !born.is_empty(),
            "{file}: a declaration that instantiates nothing"
        );

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
            for side in ["from", "to"] {
                let raw = edge[side]
                    .as_str()
                    .unwrap_or_else(|| panic!("{file}: an edge without a {side}"));
                let resolved = Path::resolve(&Path::new(&scope), raw).as_str().to_string();
                assert!(
                    allowed.contains(&resolved),
                    "{file}: the edge endpoint {raw:?} resolves to {resolved}, which is \
                     neither a node this diff instantiates nor the open container it is \
                     instantiated into. An endpoint anywhere else reaches into an interior \
                     that belongs to another template — and every internal edge of these \
                     levels came WITH its template. Allowed here: {allowed:?}"
                );
            }
        }
    }
    assert!(
        edges_seen > 0,
        "the five declarations draw no edge at all — the assertion would be vacuous"
    );
}

/// The other half of the same bullet, so that "no hand-written internal edge"
/// cannot be satisfied by a stack that instantiates nothing: the five
/// declarations name the four levels and the two halves of a channel, by
/// PINNED reference, and each pin resolves against the tree it ships with.
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
            "talky".to_string(),
            "telegram-connector".to_string(),
        ],
        "the stack is the four levels plus the two halves of one channel"
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

    // (2) the connector's proxy cell.
    let connector = grown.row(&format!("{CHANNELS}/telegram-connector"));
    assert_eq!(connector.template.as_deref(), Some("telegram-connector"));
    assert_eq!(
        connector.template_version.as_deref(),
        Some(version_of("telegram-connector").as_str())
    );
    let grown_config = read_json(
        &grown
            .root
            .join("os/orgs/acme/members/alex/assistants/scribe/channels/telegram-connector")
            .join("config.json"),
    );
    assert_eq!(
        grown_config["cell"]["type"].as_str(),
        Some("proxy"),
        "the connector is ONE cell since telegram-connector@2.0.0 (GH #303), and the \
         stamped row is that cell's"
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
        "the live tree draws a different number of member-to-container edges than \
         member@1.0.0 declares"
    );
    assert_eq!(
        declared, 9,
        "the member's own edges to and from its assistants container are the member \
         template's, drawn ONCE at member instantiation: two down (the screened turn \
         coming back off ./firewall, the memory hive's bundle) and seven up (turn, \
         recall, extraction, write, turn_write, prune, error). A second agent must not \
         move this number — that is what makes it one instantiation."
    );
}

// ═════════════════════════════ (d) a second channel, no intermediate hive

/// GH #302, fourth bullet: *a second channel is two instantiations into
/// `channels`, still one mutation, with no intermediate hive.*
///
/// Three measurements:
///
/// 1. The mutation is ONE, and it instantiates exactly two nodes.
/// 2. The resulting paths have **no intermediate hive**: each of the two is a
///    direct child of the container, so a channel node sits two segments below
///    the assistant (`channels/<node>`) and four below the member
///    (`assistants/<agent>/channels/<node>`) — the shape the live tree already
///    had, minus the retired `channel` level (GH #303).
/// 3. The eighteen edges between `./channels` and its siblings do not move, and
///    the second channel costs exactly its own wiring again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn d_a_second_channel_is_two_instantiations_and_no_intermediate_hive() {
    let Some(_) = shipped() else { return };
    if !library_is_complete() {
        return;
    }

    let decl = second_channel();
    assert_eq!(
        decl["diff"]["add_nodes"].as_array().map(Vec::len),
        Some(2),
        "a channel is a connector and a talky — two instantiations, one mutation"
    );

    let one = grow(vec![]).await;
    let two = grow(vec![decl]).await;

    // (2) no intermediate hive.
    for node in ["telegram-connector-2", "talky-2"] {
        let path = format!("{CHANNELS}/{node}");
        assert!(
            two.rows.iter().any(|r| r.path == path)
                || two
                    .rows
                    .iter()
                    .any(|r| r.path.starts_with(&format!("{path}/"))),
            "the second channel's {node} is not at {path}; rows: {:?}",
            two.paths()
        );
        let from_assistant: Vec<&str> = path
            .strip_prefix(&format!("{ASSISTANT}/"))
            .unwrap()
            .split('/')
            .collect();
        assert_eq!(
            from_assistant,
            vec!["channels", node],
            "a channel node is a DIRECT child of the container. A third segment here is \
             the retired `channel` hive coming back (GH #303)."
        );
        let from_member: Vec<&str> = path
            .strip_prefix(&format!("{MEMBER}/"))
            .unwrap()
            .split('/')
            .collect();
        assert_eq!(
            from_member.len(),
            4,
            "the shape the live tree already had — <member>/assistants/<agent>/channels/\
             <node>, four segments: {from_member:?}"
        );
    }

    // (3) the fan-in is the template's, drawn once.
    assert_eq!(
        one.channels_to_siblings(),
        18,
        "assistant@1.0.0 ships eighteen edges between ./channels and its siblings \
         (templates/assistant/README.md § What the level owns). Move the README with the \
         number."
    );
    assert_eq!(
        one.channels_to_siblings(),
        two.channels_to_siblings(),
        "a second channel moved the fan-in between ./channels and its siblings from {} to \
         {}. #303's whole ruling is that this number is a property of the TEMPLATE and is \
         drawn once.",
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
    println!("channels<->sibling = {}", grown.channels_to_siblings());
    println!("inside channels    = {}", grown.inside_channels());
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
