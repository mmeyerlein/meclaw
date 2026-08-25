//! GH #277 — the five shipped example mutations build one tree, and the same
//! one twice.
//!
//! WHAT THIS FILE IS
//! =================
//! Spec acceptance 7 of `plans/composition-reference/spec.md` asks for a
//! fixture that instantiates the shipped composites and measures the result.
//! No such fixture existed: the "six mutations, 13 templates, 39 edges" of the
//! spec is a design sketch, not a file — no test, no script in the tree carried
//! those numbers. What DOES exist is the five shipped example declarations,
//! whose `"template":` references sum to exactly eleven (they summed to
//! thirteen until GH #298, ruling Q11, took the `memory-drain` node out of two
//! of them):
//!
//! | file                              | `add_nodes` templates |
//! |-----------------------------------|-----------------------|
//! | `examples/hard-shell/grow.json`   | 2                     |
//! | `examples/never-forgets/grow.json`| 3                     |
//! | `examples/meclaw-os/grow.json`    | 4                     |
//! | `examples/meclaw-os/grow-cogny.json`   | 1                |
//! | `examples/meclaw-os/grow-steward.json` | 1                |
//!
//! This file applies those five, verbatim, to ONE colony — each into a scope of
//! its own so the names do not collide — and writes down what the run really
//! produces. Every number below is a MEASURED constant: it was read off the
//! first green run and pinned, never guessed from the spec. A number that moves
//! is a change in the shipped templates or in the substrate, and it has to be
//! looked at.
//!
//! WHY THE CELLS ARE INERT
//! =======================
//! The claim under test is structural — which rows the registry gains, which
//! provenance chain each carries, how many edges the graph holds — not what any
//! cell does with a message. The real cell types (`code`, `store`, `llm`,
//! `timer`, `proxy`, …) live in `meclaw-cells`, which depends on THIS crate, so
//! a test here cannot have them. It registers one lazy no-op factory per
//! cell type the tree names instead: every cell is born `Dormant`, no task ever
//! runs, and nothing in the run depends on cell behaviour. The behavioural half
//! of the same examples is pinned where the real factories live —
//! `crates/meclaw-cells/tests/meclaw_os_example.rs` and
//! `never_forgets_example.rs`.
//!
//! `ref` is deliberately NOT given a factory: it is a template-time type,
//! resolved during staging, and a `ref` cell must never reach the registry.
//! Neither is `hive` — a hive is a scope marker, not an actor.

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
// the inert factory
// ──────────────────────────────────────────────────────────────────────────────

/// A lazy factory that accepts every params block and never runs anything.
///
/// `is_lazy() == true` makes both the bootstrap path and the mutation path
/// register the cell as `Dormant`: a mailbox pair and no task. The `WakeFn` and
/// the `RespawnFn` below are reachable only through a delivery or a restart,
/// neither of which this fixture performs; they are written correctly anyway
/// rather than left as `unimplemented!()`, because a panic on either path would
/// take the whole colony task with it (the panic-free hot-path invariant).
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

        // Pre-wake placeholder stop wiring, same as every lazy factory: the
        // counterparts are dropped here and the colony overwrites the pair with
        // the live one the `WakeFn` returns.
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
// the shipped material
// ──────────────────────────────────────────────────────────────────────────────

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The five shipped declarations, in the order this fixture applies them, each
/// with the scope it goes into. The three examples get a scope each so their
/// `surface`/`sink`/`talky` names do not collide; the two follow-up
/// declarations of `meclaw-os` extend the scope their base built, which is what
/// they do for a reader too.
const DECLARATIONS: [(&str, &str); 5] = [
    ("examples/hard-shell/grow.json", "/hard-shell"),
    ("examples/never-forgets/grow.json", "/never-forgets"),
    ("examples/meclaw-os/grow.json", "/meclaw-os"),
    ("examples/meclaw-os/grow-cogny.json", "/meclaw-os"),
    ("examples/meclaw-os/grow-steward.json", "/meclaw-os"),
];

/// The three seeds, each copied under the scope its declarations address. A
/// seed carries the cells the declaration wires to but does not create
/// (`hard-shell`'s `probe`, `never-forgets`'s `replay` and `memory/keep`).
const SEEDS: [(&str, &str); 3] = [
    ("examples/hard-shell/seed/main", "hard-shell"),
    ("examples/never-forgets/seed/main", "never-forgets"),
    ("examples/meclaw-os/seed/main", "meclaw-os"),
];

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
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

/// Every `cell.type` the copied tree names, minus the two that have no factory
/// by design: `hive` (a scope marker, not an actor) and `ref` (a template-time
/// type, resolved during staging — it must never become a runtime cell type).
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

/// The colony root this fixture runs on: the real `templates/` library, the
/// three shipped seeds side by side under one root hive, and the `.env` the
/// READMEs ask for.
fn build_root(root: &std::path::Path) {
    std::fs::copy(
        repo("examples/meclaw-os/seed/colony.json"),
        root.join("colony.json"),
    )
    .expect("colony.json");
    // The root hive draws no edge of its own: every edge of this colony comes
    // out of one of the five declarations.
    std::fs::create_dir_all(root.join("main")).unwrap();
    std::fs::write(
        root.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    for (seed, scope) in SEEDS {
        copy_tree(&repo(seed), &root.join("main").join(scope));
    }
    copy_tree(&repo("templates"), &root.join("templates"));
    std::fs::write(
        root.join(".env"),
        "OPENROUTER_API_KEY=test-key\n\
         MODEL_BRAIN=gpt-4o-mock\n\
         MODEL_CORE=gpt-4o-mock\n\
         MODEL_CORE_FAST=gpt-4o-mock-fast\n\
         KEEPER_NIGHT_CRON=0 0 0 1 1 *\n",
    )
    .unwrap();
}

fn factories(root: &std::path::Path) -> Vec<(String, Arc<dyn CellFactory>)> {
    let types = cell_types_in(root);
    assert!(
        types.contains("code") && types.contains("store") && types.contains("llm"),
        "the copied tree named no real cell types — the seed/library copy failed: {types:?}"
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
        .expect("the three seeds must boot");
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

/// One shipped declaration, applied verbatim except for its `scope`: the file
/// says `/`, and this colony holds three examples side by side, so it is
/// redirected into the scope that carries its seed. Nothing else is touched.
async fn grow(h: &ColonyHandle, file: &str, scope: &str) -> MutationOutcome {
    let mut payload = read_json(&repo(file));
    payload["scope"] = json!(scope);
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

// ──────────────────────────────────────────────────────────────────────────────
// what one run produces
// ──────────────────────────────────────────────────────────────────────────────

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

/// Everything one application of the five declarations leaves behind.
struct Run {
    rows: Vec<Row>,
    edges: usize,
}

impl Run {
    fn template_born(&self) -> Vec<&Row> {
        self.rows.iter().filter(|r| r.template.is_some()).collect()
    }

    fn distinct_templates(&self) -> BTreeSet<&str> {
        self.rows
            .iter()
            .filter_map(|r| r.template.as_deref())
            .collect()
    }

    fn paths_of(&self, template: &str) -> Vec<&Row> {
        self.rows
            .iter()
            .filter(|r| r.template.as_deref() == Some(template))
            .collect()
    }
}

/// Boot a fresh colony over a fresh temp root, apply all five declarations,
/// shut down cleanly (which flushes the write buffer) and read the persisted
/// result back.
///
/// The `TempDir` is returned alongside so the caller keeps the tree alive.
///
/// NOTE on the direct SQL: this is test-side reading of `colony.db`, not cell
/// code. The database-isolation rule binds cells; a fixture that measures what
/// the substrate persisted has to read what the substrate persisted.
async fn run_the_five() -> (tempfile::TempDir, Run) {
    let td = tempfile::TempDir::new().unwrap();
    build_root(td.path());
    let h = boot(&td).await;

    for (file, scope) in DECLARATIONS {
        let outcome = grow(&h, file, scope).await;
        assert!(
            matches!(outcome, MutationOutcome::Committed { .. }),
            "{file} into {scope} was not committed: {outcome:?}"
        );
    }
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
    let edges: i64 = conn
        .query_row("SELECT count(*) FROM edges", [], |r| r.get(0))
        .unwrap();
    drop(stmt);
    drop(conn);
    (
        td,
        Run {
            rows,
            edges: edges as usize,
        },
    )
}

// ──────────────────────────────────────────────────────────────────────────────
// the measured constants
// ──────────────────────────────────────────────────────────────────────────────
//
// Every number in this block was READ OFF THE RUN, not taken from the spec.
// `plans/composition-reference/spec.md` acceptance 7 sketched "13 templates and
// 39 edges"; the thirteen WAS a real property of the shipped files until GH #298
// (ruling Q11) removed the `memory-drain` node from two of the five, and it is
// checked against the files themselves below. The rest is what the substrate
// really builds.

/// `"template":` references across the five declarations — the only number here
/// that is a property of the FILES rather than of a run. Asserted against the
/// files in [`the_five_declarations_name_eleven_templates`].
///
/// Moved 13 -> 11 with GH #298 (ruling Q11): `memory-drain` left the live stack
/// and with it `examples/never-forgets/grow.json` and
/// `examples/meclaw-os/grow.json`, one node each.
const TEMPLATE_REFERENCES_IN_THE_FIVE: usize = 11;

/// Registry rows the five declarations add that carry a provenance stamp.
/// MEASURED, not assumed: read off the first green run (see
/// [`print_the_measurement`]). Ten templates instantiated across three
/// scopes; a hive root is a scope marker and holds no registry row, so this
/// counts the ACTORS the five declarations brought into being.
///
/// Moved 46 -> 42 with GH #298 (ruling Q11): two `memory-drain` instances, two
/// cells each (`drain`, `ledger`). Moved 42 -> 44 with GH #379: `talky@4.1.0`
/// grew the sidecar `splitter`, and two of the five declarations instantiate a
/// `talky`.
const TEMPLATE_BORN_ROWS: usize = 44;

/// Distinct `registry.template` values across those rows. Fewer than the
/// eleven references above, because three scopes instantiate the same
/// `door`/`terminal`/`talky` — and larger than the six templates the
/// declarations actually NAME, by exactly the four sub-units the composites
/// REFERENCE rather than carry. That difference is the whole point of the wave;
/// [`REFERENCED_SUB_UNITS`] pins it by name.
/// MEASURED, not assumed: read off the first green run.
///
/// Moved 11 -> 10 with GH #298 (ruling Q11): no declaration names
/// `memory-drain` any more, so no registry row claims it.
const DISTINCT_TEMPLATES: usize = 10;

/// The sub-units that appear in the registry although NO declaration names
/// them: they arrive through `talky`'s and `cogny`'s `cell.type: "ref"` cells.
/// Before this wave they were byte copies and every one of them claimed to be
/// an instance of the composite.
const REFERENCED_SUB_UNITS: [&str; 4] = ["collector", "dispatcher", "session-keeper", "summarizer"];

/// Rows in `colony.db`'s `edges` table after all five declarations — the
/// declarations' own wiring plus every edge the instantiated composites draw
/// inside themselves.
/// MEASURED, not assumed: read off the first green run. The spec sketch's 39 is
/// a guess about a fixture that never existed.
///
/// Moved 161 -> 167 with `session-keeper@2.0.3` (GH #343), in two steps of the
/// same change. Both counts are per `talky`, and exactly two of the five
/// declarations instantiate one (`never-forgets`, `meclaw-os`):
///
/// - **+4**: each of the keeper's two cells gained a door out to the hive path
///   for the new `reject` lane. 2 doors x 2 instances.
/// - **+2**: `talky` itself gained the subscriber for that lane
///   (`./session-keeper -> ./errors`), so a store that stopped answering is not
///   a silent room. 1 edge x 2 instances.
///
/// No other declaration moved: `firewall` and `memory-drain` report their
/// refusal on doors they already had, and `cogny` has no keeper.
///
/// Moved 167 -> 154 with GH #298 (ruling Q11), in three parts: -8 for the two
/// `memory-drain` instances' own four internal edges each, -3 for
/// `never-forgets` (four declaration edges touching `./drain` out, one
/// `./talky -> ./memory/keep` in) and -2 for `meclaw-os` (three out, one
/// `./talky -> ./sink` in).
///
/// Moved 154 -> 158 with GH #379: `talky@4.1.0` puts the sidecar `splitter` on
/// the answer path, which is +2 internal edges per instance (`brain ->
/// splitter` replaces `brain -> dispatcher`, and `splitter -> dispatcher` plus
/// `splitter -> .` are new), and two of the five declarations instantiate a
/// `talky`.
///
/// Moved 158 -> 160 with GH #267: `steward@2.0.8` stops opening `colony.db` and
/// asks `/colony/ledger` instead, which is +2 edges per instance
/// (`./meter -> /colony/ledger` and `./probe -> /colony/ledger`). Exactly one of
/// the five declarations grows a `steward` (`examples/meclaw-os`), so the tree
/// gains two. They are the only edges in the tree whose endpoint is neither the
/// hive nor a child of it, and they are sanctioned: `/colony/ledger` is the
/// second entry of `MUTATION_DRAWABLE_VIRTUAL_ENDPOINTS`, beside `/colony/graph`
/// (GH #163).
///
/// Moved 160 -> 156 with GH #284: four declaration edges routed a `reject` or an
/// `error` into a node grown from `terminal`, which accepts the refusal and
/// drops it. All four were deleted rather than re-pointed — none was load-bearing
/// (`firewall` and `steward` declare no `required_drains`, and `talky`'s only
/// pairing is `in_prune -> prune`), so each emission becomes `no_route` and
/// localises itself in the dead-letter queue. One each in
/// `examples/meclaw-os/grow-steward.json` and `examples/never-forgets/grow.json`,
/// two in `examples/meclaw-os/grow.json`.
///
/// Moved 156 -> 160 with GH #55: `talky@4.2.0` draws the two tool lanes it used
/// to ask a parent for (`memory_recall` -> `in_memory_call`, `thread_recall` ->
/// `in_thread_call`) as internal edges of its own graph. Two of the five
/// declarations instantiate a `talky`, so the tree gains exactly four. No
/// declaration changed: this is the composite carrying wiring a parent no longer
/// has to draw, which is the whole of #55's edge half.
const EDGES: usize = 160;

/// Cells that were on disk before the first declaration — the three seeds' own
/// cells (`hard-shell`'s `probe`, `never-forgets`'s `replay`,
/// `memory/episodes` and `memory/keep`). They carry NO template and NO chain,
/// which is what makes the chain assertion below non-vacuous.
/// MEASURED, not assumed: read off the first green run.
const SEED_BORN_ROWS: usize = 4;

// ──────────────────────────────────────────────────────────────────────────────
// the tests
// ──────────────────────────────────────────────────────────────────────────────

/// The template references of the five, measured on the shipped files rather
/// than quoted from the spec sketch.
#[test]
fn the_five_declarations_name_eleven_templates() {
    let mut total = 0usize;
    for (file, _) in DECLARATIONS {
        let v = read_json(&repo(file));
        total += v["diff"]["add_nodes"]
            .as_array()
            .unwrap_or_else(|| panic!("{file} has no add_nodes array"))
            .iter()
            .filter(|n| n.get("template").is_some())
            .count();
    }
    assert_eq!(
        total, TEMPLATE_REFERENCES_IN_THE_FIVE,
        "the shipped declarations changed shape — re-read the module docs table \
         before moving the constant"
    );
}

/// The fixture spec acceptance 7 asked for: all five declarations commit, the
/// tree they build is measured rather than guessed, and it is the SAME tree
/// twice.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_example_mutations_build_the_same_tree_twice() {
    let (_td_a, a) = run_the_five().await;

    assert_eq!(
        a.template_born().len(),
        TEMPLATE_BORN_ROWS,
        "the number of template-born registry rows moved — a shipped template \
         gained or lost a cell. Re-measure and move the constant WITH a note."
    );
    assert_eq!(
        a.distinct_templates().len(),
        DISTINCT_TEMPLATES,
        "distinct registry.template values: {:?}",
        a.distinct_templates()
    );
    // The four sub-units NO declaration names are in the registry anyway: the
    // composites reference them, and staging resolved the references. If a ref
    // silently stopped resolving, this is where it shows.
    let declared: BTreeSet<String> = DECLARATIONS
        .iter()
        .flat_map(|(file, _)| {
            read_json(&repo(file))["diff"]["add_nodes"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|n| n["template"].as_str().map(str::to_string))
        .collect();
    for sub in REFERENCED_SUB_UNITS {
        assert!(
            !declared.contains(sub),
            "{sub} is named by a declaration — it is no longer proof that a ref \
             resolved"
        );
        assert!(
            a.distinct_templates().contains(sub),
            "{sub} is referenced by a composite but no registry row claims it: {:?}",
            a.distinct_templates()
        );
    }
    assert_eq!(a.edges, EDGES, "the persisted edge count moved");
    assert_eq!(
        a.rows.len() - a.template_born().len(),
        SEED_BORN_ROWS,
        "the seeds' own cells: {:?}",
        a.rows
            .iter()
            .filter(|r| r.template.is_none())
            .map(|r| &r.path)
            .collect::<Vec<_>>()
    );

    // `SELECT count(*) FROM registry WHERE template_chain IS NOT NULL` equals
    // the template-born row count: every cell that came from a template records
    // where it came from, and no cell that did not pretends to.
    let with_chain = a.rows.iter().filter(|r| r.template_chain.is_some()).count();
    assert_eq!(
        with_chain,
        a.template_born().len(),
        "template_chain and template disagree about which rows are \
         template-born — chained without a template, or born without a chain: {:?}",
        a.rows
            .iter()
            .filter(|r| r.template.is_some() != r.template_chain.is_some())
            .map(|r| &r.path)
            .collect::<Vec<_>>()
    );

    // Stable across two runs. Not just the counts — the whole persisted shape,
    // path by path, template by template, chain by chain.
    let (_td_b, b) = run_the_five().await;
    assert_eq!(
        b.edges, a.edges,
        "two runs of the same five declarations produced different edge counts"
    );
    assert_eq!(
        b.rows, a.rows,
        "two runs of the same five declarations produced different registry rows"
    );
}

/// How the four constants above are re-measured. Not a check — a readout, so
/// that moving a pinned number is one command and a look, never a guess:
///
/// ```text
/// cargo test -p meclaw-colony --test gh277_the_example_mutations_build_the_same_tree \
///     print_the_measurement -- --ignored --nocapture
/// ```
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn print_the_measurement() {
    let (_td, run) = run_the_five().await;
    println!("TEMPLATE_BORN_ROWS = {}", run.template_born().len());
    println!("DISTINCT_TEMPLATES = {}", run.distinct_templates().len());
    println!("EDGES              = {}", run.edges);
    println!(
        "SEED_BORN_ROWS     = {}",
        run.rows.len() - run.template_born().len()
    );
    println!("distinct templates: {:?}", run.distinct_templates());
    for row in &run.rows {
        println!(
            "  {}\t{}\t{}",
            row.path,
            row.template.as_deref().unwrap_or("-"),
            row.template_chain.as_deref().unwrap_or("-")
        );
    }
}

/// GH #277's stated damage, measured closed: "which cells are instances of
/// `collector`?" had no answer, because a byte copy carries no origin. It has
/// one now — and it names the composite that placed each of them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn which_cells_are_instances_of_collector_has_an_answer() {
    let (_td, run) = run_the_five().await;

    let rows = run.paths_of("collector");
    assert!(
        !rows.is_empty(),
        "no registry row claims to be an instance of `collector` — the question \
         GH #277 asked is unanswerable again"
    );

    let inside_talky: Vec<&str> = rows
        .iter()
        .map(|r| r.path.as_str())
        .filter(|p| p.contains("/talky/collector/"))
        .collect();
    let inside_cogny: Vec<&str> = rows
        .iter()
        .map(|r| r.path.as_str())
        .filter(|p| p.contains("/cogny/collector/"))
        .collect();
    assert!(
        !inside_talky.is_empty(),
        "no collector inside talky: {:?}",
        rows.iter().map(|r| &r.path).collect::<Vec<_>>()
    );
    assert!(
        !inside_cogny.is_empty(),
        "no collector inside cogny: {:?}",
        rows.iter().map(|r| &r.path).collect::<Vec<_>>()
    );

    // Each row's chain names its composite: outermost first, the leaf last.
    for row in &rows {
        let raw = row
            .template_chain
            .as_deref()
            .unwrap_or_else(|| panic!("{} is a collector instance without a chain", row.path));
        let chain: Vec<Value> = meclaw_core::serde_json::from_str(raw)
            .unwrap_or_else(|e| panic!("{} chain is not JSON: {e} ({raw})", row.path));
        let names: Vec<String> = chain
            .iter()
            .map(|e| {
                e.get("template")
                    .or_else(|| e.get(0))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        let composite = if row.path.contains("/talky/") {
            "talky"
        } else {
            "cogny"
        };
        assert_eq!(
            names.first().map(String::as_str),
            Some(composite),
            "{}: the chain does not start at the composite that placed it ({raw})",
            row.path
        );
        assert_eq!(
            names.last().map(String::as_str),
            Some("collector"),
            "{}: the chain does not end at the template the cell really is ({raw})",
            row.path
        );
    }
}
