//! GH #466 — the organism the builder draws is the organism the tree already
//! has, and it boots.
//!
//! WHAT THIS FILE IS
//! =================
//! `examples/organism/` grows a shell, an organisation, a person, one
//! generation of that person's agent and one Telegram channel, out of five
//! hand-written declarations. Four of those five are LEVEL declarations, and
//! the transit edges in them are the part a model rewrote from scratch on every
//! build — 17 + 17 + 11 + 3 edges of pure addressing, with the child's name
//! substituted in.
//!
//! Since `builder@1` the fast lane renders them. This file applies the
//! RENDERED ones to a real colony and asks the only question that settles it:
//! **is the tree the same?** Same registry rows with the same provenance chain,
//! same hive scopes, same edge set — against the tree the shipped declarations
//! build, which is itself the reference `gh302` and `gh422` measure.
//!
//! It is deliberately not a comparison of two JSON files. The byte comparison
//! lives in `gh466_grow_level_renders_the_level.rs` and is the cheaper, sharper
//! half; this one answers what a byte comparison cannot — that what the recipe
//! renders survives the mutation door, stages, registers and comes up ACTIVE.
//! An identical body that refuses at declaration 2 would pass the first test
//! and fail here.
//!
//! No model is involved anywhere, and that is the point rather than a
//! convenience: `recipes` is a `code` cell with the network denied. The whole
//! four-level walk costs four python starts.
//!
//! WHY THE CELLS ARE INERT — and the guard
//! =======================================
//! Same device and same reason as `gh302_the_stack_grows_from_templates.rs` and
//! `gh422_the_manifest_grows_the_same_stack.rs`: every claim is structural, and
//! two of the cell types this stack names would reach outward the moment they
//! were spawned for real. And like every template-reading test (GH #49), a tree
//! that did not ship the example or the library is SKIPPED, never judged.

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

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The five declarations, in the order the manifest bundles them.
const DECLARATIONS: [&str; 5] = [
    "examples/organism/grow-os.json",
    "examples/organism/grow-org.json",
    "examples/organism/grow-member.json",
    "examples/organism/grow-assistant.json",
    "examples/organism/grow-channel.json",
];

/// The `builder`'s fast lane, which renders the four level declarations.
const RECIPES: &str = "templates/builder/recipes/config.json";

/// `examples/organism` plus the recipe that has to reproduce it, or nothing
/// when this tree shipped neither (GH #49).
fn shipped() -> bool {
    DECLARATIONS.iter().all(|f| repo(f).is_file()) && repo(RECIPES).is_file()
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

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

// ──────────────────────────────────────────────────────────────────────────────
// the inert factory (the device of gh302, copied deliberately: gh302 is the
// measured reference and must not move because this file exists)
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

/// The colony root: the example's own seed, the real library, and an `.env`
/// whose every value is a placeholder.
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
    cell_types_in(&root.join("templates"))
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

/// One body at the door, form unknown to the caller — exactly what `--apply`
/// and `POST /colony/mutations` do.
/// One registry row, as the persisted `colony.db` holds it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Row {
    path: String,
    template: Option<String>,
    template_version: Option<String>,
    template_chain: Option<String>,
}

/// Everything one run leaves behind.
struct Grown {
    _td: tempfile::TempDir,
    rows: Vec<Row>,
    /// The hive scopes the run registered. A hive is a scope marker, not an
    /// actor, so it leaves no registry row — and a declaration whose subtree is
    /// all containers would otherwise look like it did nothing.
    hives: Vec<String>,
    edges: Vec<(String, String)>,
    /// The registry's own status word per path — `"active"` or `"inactive"`.
    /// It is kept BESIDE `rows` rather than inside them, so the provenance
    /// comparison of the first measurement stays exactly the set of columns it
    /// was written against.
    statuses: Vec<(String, String)>,
}

impl Grown {
    fn paths(&self) -> Vec<&str> {
        self.rows.iter().map(|r| r.path.as_str()).collect()
    }

    /// The status the registry persisted for `path`, or `None` when no row of
    /// that name exists at all. The two must never look alike (§ 2c): a node
    /// that was never grown has no row, a node born asleep has one.
    fn status_of(&self, path: &str) -> Option<&str> {
        self.statuses
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, s)| s.as_str())
    }
}

/// Read the graph, shut down cleanly (which flushes the write buffer) and read
/// the registry back.
///
/// NOTE on the direct SQL: test-side reading of `colony.db`, not cell code. A
/// fixture that measures what the substrate persisted has to read it.
async fn harvest(td: tempfile::TempDir, h: ColonyHandle) -> Grown {
    let (ack_tx, ack_rx) = oneshot::channel::<meclaw_colony::api_dto::ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: Path::new("/"),
            ack: ack_tx,
        })
        .await
        .unwrap();
    let mut edges: Vec<(String, String)> = ack_rx
        .await
        .unwrap()
        .edges
        .iter()
        .map(|e| (e.from.to_string(), e.to.to_string()))
        .collect();
    edges.sort();
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
    let mut stmt = conn
        .prepare("SELECT path, status FROM registry ORDER BY path")
        .unwrap();
    let statuses: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    drop(stmt);
    let mut stmt = conn
        .prepare("SELECT path FROM hive_scopes ORDER BY path")
        .unwrap();
    let hives: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    drop(stmt);
    drop(conn);

    Grown {
        _td: td,
        rows,
        hives,
        edges,
        statuses,
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// the same four levels, rendered rather than written
// ──────────────────────────────────────────────────────────────────────────────

/// The four level declarations, as `builder/recipes` renders them. The
/// parameters are what a wish carries: a scope, a level, a name, a template, and
/// for a channel the agent its turns default to. Everything else — every guard,
/// every modifier, every literal — comes out of the recipe's table.
fn rendered_levels() -> Vec<Value> {
    let script = meclaw_testing::shipped_script(
        repo(RECIPES)
            .to_str()
            .expect("a utf-8 path to the recipe config"),
    );
    let mut wishes = vec![
        json!({"scope": "/os", "level": "org", "name": "acme"}),
        json!({"scope": "/os/orgs/acme", "level": "member", "name": "alex"}),
        json!({"scope": "/os/orgs/acme/members/alex", "level": "assistant",
               "name": "scribe",
               "ctx": {"model": "${MODEL_CORE}", "model_fast": "${MODEL_CORE_FAST}",
                       "model_surface": "${MODEL_SURFACE}"},
               "override_params": {"cogny/brain": {"temperature": 0.2}}}),
        // GH #517 -- and, for a channel, the PERSON its turns are spoken
        // with: the round is provenance and is never derived from the path.
        json!({"scope": "/os/orgs/acme/members/alex", "level": "channel",
               "name": "telegram", "assistant": "scribe",
               "ctx": {"member_person": "alex"}}),
    ];
    // The template each wish names comes OFF the shipped declaration. What is
    // under test is the addressing, not which version of a level the tree
    // currently carries -- and pinning a version here would make this file red
    // for somebody else's bump.
    for (wish, file) in wishes.iter_mut().zip(DECLARATIONS[1..].iter()) {
        wish["template"] = read_json(&repo(file))["diff"]["add_nodes"][0]["template"].clone();
    }
    wishes
        .into_iter()
        .map(|params| {
            // The FIRST declaration. Since GH #543 a member wish renders the
            // level AND the screen and the app that member always gets, and
            // since GH #585 the three ride in ONE manifest — what this file
            // compares against the four shipped declarations is the level.
            let member = params["level"] == json!("member");
            let out = meclaw_testing::emit_all(
                &script,
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
            .expect("the fast lane answered no manifest at all");
            assert!(
                out["header"]["error_code"].is_null(),
                "the fast lane refused a level it is supposed to render: {out}"
            );
            let decls = out["manifest"].as_array().expect("a manifest").clone();
            assert_eq!(
                decls.len(),
                if member { 3 } else { 1 },
                "the level is not the declaration this manifest leads with: {decls:?}"
            );
            decls[0].clone()
        })
        .collect()
}

/// A mutation outcome, reduced to what two runs can be compared on: whether it
/// committed, and if not, under which code.
fn verdict(outcome: &MutationOutcome) -> String {
    match outcome {
        MutationOutcome::Committed { .. } => "committed".to_string(),
        other => {
            let s = format!("{other:?}");
            // The id is a uuid minted per attempt and differs by construction.
            match (s.find("error_code: "), s.find(", details")) {
                (Some(a), Some(b)) if b > a => format!("refused {}", &s[a + 12..b]),
                _ => "refused".to_string(),
            }
        }
    }
}

/// Boot, apply the shell, then apply four level declarations — either the
/// shipped files or the rendered ones. The shell is not a level: it is the birth
/// of the tree everything else is grown into, and no recipe claims it.
async fn grow(levels: Vec<Value>) -> (Grown, Vec<String>) {
    let td = tempfile::TempDir::new().unwrap();
    build_root(td.path());
    let h = boot(&td).await;
    let outcome = mutate(&h, read_json(&repo(DECLARATIONS[0]))).await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the shell was not committed: {outcome:?}"
    );
    let mut verdicts = Vec::new();
    for decl in levels {
        verdicts.push(verdict(&mutate(&h, decl).await));
    }
    (harvest(td, h).await, verdicts)
}

fn shipped_levels() -> Vec<Value> {
    DECLARATIONS[1..]
        .iter()
        .map(|f| read_json(&repo(f)))
        .collect()
}

// ──────────────────────────────────────────────────────────────────────────────
// the measurements
// ──────────────────────────────────────────────────────────────────────────────

/// The whole point, in one assertion: four wishes, no model, and the organism
/// that comes up is the organism the tree already had.
///
/// The door's VERDICT is compared before the tree is, and that ordering is
/// deliberate. This file owns "the rendered body and the written body are the
/// same body"; whether the written body applies is owned by
/// `gh422_the_manifest_grows_the_same_stack.rs` and
/// `gh302_the_stack_grows_from_templates.rs`. So a library whose refs are
/// mid-bump makes those two red and leaves this one making its own statement —
/// out loud, with the refusal named, never as a silent pass.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_rendered_levels_grow_the_tree_the_declarations_grow() {
    if !shipped() || !library_is_complete() {
        return;
    }
    let (hand, hand_says) = grow(shipped_levels()).await;
    let (drawn, drawn_says) = grow(rendered_levels()).await;

    assert_eq!(
        drawn_says, hand_says,
        "the door answered the rendered levels differently from the written          ones — the two are supposed to be the same body"
    );
    assert_eq!(
        &drawn_says[..2],
        &["committed".to_string(), "committed".to_string()],
        "the organisation and the person did not even reach the registry, so          nothing below is worth comparing (§ 2c: an empty result and a          forgotten call must never look alike)"
    );
    if drawn_says.iter().any(|v| v != "committed") {
        // Named, counted, and not skipped in silence: the reference itself is
        // refusing, which is not this file's claim to make.
        eprintln!(
            "NOTICE gh466: the shipped example does not apply against this              library either ({hand_says:?}); the rendered/written equivalence is              asserted, the grown tree is not. See gh422/gh302."
        );
    }

    assert_eq!(
        drawn.paths(),
        hand.paths(),
        "the rendered levels registered a different set of cells"
    );
    assert_eq!(
        drawn.rows, hand.rows,
        "same paths, different provenance — a row's template, version or chain          moved, which means a level was grown from something else"
    );
    assert_eq!(
        drawn.hives, hand.hives,
        "the hive scopes differ; a container that is not a scope is a level          with no room in it"
    );
    assert_eq!(
        drawn.edges, hand.edges,
        "the ADDRESSING differs. This is the assertion the whole recipe exists          for: an edge set that is nearly right is a subtree that boots and          answers nothing on the lanes nobody drew"
    );
}

/// A level stands, and it is reachable in both directions. Activity is derived
/// from the edges alone, so this is what "grown, not merely staged" means.
///
/// The person is the level under test rather than the generation, because a
/// member is grown from a template with no refs below it — so this assertion
/// stays about the recipe even while the composites are mid-bump.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rendered_level_is_addressable_from_its_container_and_answers_back() {
    if !shipped() || !library_is_complete() {
        return;
    }
    let (drawn, says) = grow(rendered_levels()).await;
    assert_eq!(says[1], "committed", "the person was not grown: {says:?}");
    let members = "/os/orgs/acme/members";
    let alex = format!("{members}/alex");
    let down = drawn
        .edges
        .iter()
        .filter(|(from, to)| from == members && *to == alex)
        .count();
    let up = drawn
        .edges
        .iter()
        .filter(|(from, to)| *from == alex && to == members)
        .count();
    assert_eq!(
        (down, up),
        (7, 13),
        "the person is not wired the way the level wires one — seven doors down          (in_turn, in_recall, in_brief, in_propose, in_build_result, in_export and,          since GH #553, mutation_committed — the mutation door's receipt, which          every child of a container hears because it carries no context to be          addressed by), thirteen exits back up — `bundle` since GH #533, the answer to the question the second of those doors takes, and `dump` since GH #555, the receipt of an applied import part, which used to end inside the member in a cell that read it and said nothing; a level reached by a door with no exit is a level          that answers into nothing, and a level with no in_export door cannot be          asked for its memory at all (GH #470)"
    );
    assert!(
        drawn.rows.iter().any(|r| r.path.starts_with(&alex)),
        "no cell of the grown person reached the registry"
    );
}

/// GH #472 — the recipe's `channel` level comes to the world ASLEEP, and the
/// levels beside it do not.
///
/// `grow_level` renders `birth` top-level on the `add_nodes` entry, in the
/// door's own GH #437 vocabulary, and the table hands the `channel` level
/// `"inactive"` (ruling GH #468): a connector opens its upstream the moment it
/// owns a task, so growing one from a wish must not take the bot token away
/// from whoever is still holding it. Arming is a second, deliberate mutation.
///
/// The measurement is a POSITIVE one on both halves. Asleep is read off a
/// registry ROW that exists — no-delete semantics, the node is registered and
/// addressable, it merely has no task — and never off a missing row, because
/// "born asleep" and "never grown" would then look alike
/// (`docs/development-rules.md` § 2c). And the counter-probe is what makes the
/// `inactive` mean anything: at least one level grown in the SAME run stands
/// `active`, so a colony that quietly built nothing cannot pass.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_rendered_channel_level_is_born_asleep_beside_awake_siblings() {
    if !shipped() || !library_is_complete() {
        return;
    }
    let (drawn, says) = grow(rendered_levels()).await;
    assert_eq!(says[3], "committed", "the channel was not grown: {says:?}");

    let alex = "/os/orgs/acme/members/alex";
    let telegram = format!("{alex}/channels/telegram");

    // Half one: the connector STANDS, and it stands dark.
    let status = drawn.status_of(&telegram).unwrap_or_else(|| {
        panic!(
            "the recipe's channel level left no registry row at all — a node \
             born inactive is REGISTERED and addressable, and that is the whole \
             difference between parking a connector and never growing one \
             (no-delete, GH #437). Rows seen: {:?}",
            drawn.paths()
        )
    });
    assert_eq!(
        status, "inactive",
        "the rendered channel came up ACTIVE. `grow_level` writes \
         birth: \"inactive\" for this one level (GH #472 at the door, ruling \
         GH #468 behind it) precisely so the new connector does not open its \
         long poll at birth and steal `getUpdates` from the connector that is \
         still running; a channel that boots awake takes the bot token with it"
    );

    // Half two: the counter-probe. Without it, `inactive` above would be just
    // as green on a colony that grew nothing and persisted nobody.
    let awake: Vec<&str> = drawn
        .statuses
        .iter()
        .filter(|(p, s)| {
            p.starts_with(&format!("{alex}/")) && !p.starts_with(&telegram) && s == "active"
        })
        .map(|(p, _)| p.as_str())
        .collect();
    assert!(
        !awake.is_empty(),
        "nothing the same run grew stands ACTIVE, so the `inactive` above says \
         nothing about `birth` — it is equally what an empty colony looks like. \
         The door's default is awake and only the channel level departs from it \
         (§ 2c: an empty result and a forgotten call must never look alike). \
         Statuses under {alex}: {:?}",
        drawn.statuses
    );
}
