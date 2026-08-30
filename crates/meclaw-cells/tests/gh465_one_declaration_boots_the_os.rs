//! GH #465 — stage one: one declaration boots the OS, and the OS says what it
//! needs before it runs.
//!
//! WHAT THIS FILE IS
//! =================
//! A built organism arrives in two stages. **Stage one** instantiates the
//! colony shell from a root tree that contains no cell at all: an empty hive,
//! one edge, and ONE `cell.type: "ref"` marker naming `meclaw-os`. **Stage two**
//! is the authoring path inside that shell growing the rest.
//!
//! `examples/meclaw-os/seed-ref/` is stage one, and this file is the only proof
//! stage one may cite. Until it existed the mechanism was pinned one directory
//! over — `examples/organism/seed-ref` and
//! `gh424_the_seed_grows_itself.rs` — while the folder named after the shell
//! shipped `grow-*.json` MUTATION declarations with hand-written edges, which
//! are a different claim: a mutation names nodes AND edges, and the point of
//! stage one is that it names one node and nothing else.
//!
//! THE FOUR THINGS MEASURED
//! ========================
//! 1. **The declaration is one line and it resolves.** The marker names the
//!    exact version the shipped library holds — a seed that fell behind a
//!    template bump would boot into a shell nobody meant.
//! 2. **The boot grows the whole shell.** Every cell the template tree carries
//!    stands afterwards, at the addresses the tree names, with no dead letter;
//!    and the marker is GONE, because a marker consumes itself.
//! 3. **The one edge.** The root hive draws exactly one, `./os` onto the
//!    mutation door on the `mutate` lane. It is the whole birth topology: the
//!    submitter inside the shell emits `mutate`, and nothing else in the colony
//!    may reach `/colony/mutations`. An edge is a mutation, so an authoring
//!    path that cannot reach the door can never draw itself one — which is why
//!    this edge is checked in rather than grown.
//! 4. **A missing requirement is refused BEFORE anything is written.** With the
//!    one required key absent from the colony's `.env`, the boot answers
//!    `requirement_missing` naming the key, and the marker is still a marker on
//!    disk: no cell, no `colony.db` row, nothing to clean up.
//!
//! WHAT THIS FILE IS NOT
//! =====================
//! It is not the claim that a `ref` marker can grow a whole organism. A marker
//! declares a NODE and never an EDGE, so stage one grows exactly the outermost
//! level; everything under `./os/orgs` is stage two's business and arrives as a
//! manifest. `examples/organism/` is where the whole stack from one file is
//! measured (`gh422`, `gh423`, `gh424`).
//!
//! And it is not a scaffolding test. The root tree is three checked-in files.
//! A generator that writes a colony root before its first boot produces a tree
//! nobody can diff and gets the two things that matter — the marker and the one
//! edge — wrong in silence; the whole reason this seed is in the repository is
//! that it is READ, not produced.
//!
//! Guarded like every template-reading test (GH #49): a tree that did not ship
//! the example or the library is SKIPPED, never judged.

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, RespawnFn, SpawnedCellKind, WakeFn,
    bootstrap_from_filesystem,
};
use meclaw_core::serde_json::Value;
use meclaw_core::{JsonValue, Message, Path};
use meclaw_testing::ColonyHandle;
use std::collections::BTreeSet;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

/// The seed under test, and the shell it declares.
const SEED: &str = "examples/meclaw-os/seed-ref";
const SHELL: &str = "meclaw-os";
/// Where the shell stands once the marker is fulfilled.
const AT: &str = "os";
/// The one edge the root tree draws, and the lane it takes.
const DOOR: &str = "/colony/mutations";
const DOOR_LANE: &str = "mutate";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Did the seed and the shell travel with this tree (GH #49)?
fn shipped() -> bool {
    repo(&format!("{SEED}/main/{AT}/config.json")).is_file()
        && repo(&format!("templates/{SHELL}/template.json")).is_file()
}

// ──────────────────────────────────────────────────────────────────────────────
// the inert factory (the device of gh424 / gh302 / gh277, copied for the same
// reason: what is measured is the TREE the boot writes, never what a cell does)
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
// the root under test
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

/// The shipped seed plus the real library, with `env` written as the colony's
/// own `.env`. `env` is the ONLY variable of this harness — everything else is
/// the tree as it ships.
fn build_root(root: &std::path::Path, env: &str) {
    copy_tree(&repo(SEED), root);
    copy_tree(&repo("templates"), &root.join("templates"));
    std::fs::write(root.join(".env"), env).unwrap();
}

fn factories(root: &std::path::Path) -> Vec<(String, Arc<dyn CellFactory>)> {
    cell_types_in(&root.join("templates"))
        .into_iter()
        .map(|t| (t, Arc::new(InertCellFactory) as Arc<dyn CellFactory>))
        .collect()
}

/// Scan the templates FIRST, then boot — the order production keeps, and the
/// reason a growth has a registry to resolve against.
async fn boot(td: &tempfile::TempDir) -> Result<ColonyHandle, String> {
    let fs = factories(td.path());
    let h = ColonyHandle::new_with_factories_at(td, fs.clone());
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
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in fs {
        registry.insert(name, f);
    }
    match bootstrap_from_filesystem(td.path(), &registry, &h.runtime()).await {
        Ok(_) => Ok(h),
        Err(e) => {
            let rendered = format!("{:?}", e.items());
            h.shutdown().await;
            Err(rendered)
        }
    }
}

/// A `.env` that holds every key the shell declares as required.
fn complete_env() -> String {
    let requires = read_json(&repo(&format!("templates/{SHELL}/template.json")));
    let env = requires["requires"]["env"]
        .as_object()
        .expect("the shell declares `requires.env`");
    let mut out = String::new();
    for (key, decl) in env {
        if decl["required"].as_bool().unwrap_or(true) {
            out.push_str(&format!("{key}=placeholder-for-a-test\n"));
        }
    }
    assert!(
        !out.is_empty(),
        "the shell declares no required key at all — then test 4 measures nothing"
    );
    out
}

// ──────────────────────────────────────────────────────────────────────────────
// 1 + 3 — the declaration and the one edge, read off the files
// ──────────────────────────────────────────────────────────────────────────────

/// The marker names the exact version the shipped library holds.
///
/// The same role `gh424`'s `the_seed_ref_names_a_version_the_library_holds`
/// plays for the organism seed: without it the seed falls behind a template
/// bump unnoticed and stage one boots a shell nobody meant.
#[test]
fn the_declaration_names_the_version_the_library_ships() {
    if !shipped() {
        eprintln!("skipped: {SEED} did not ship (GH #49)");
        return;
    }
    let marker = read_json(&repo(&format!("{SEED}/main/{AT}/config.json")));
    assert_eq!(
        marker["cell"]["type"], "ref",
        "the declaration is a `ref` marker; anything else is a checked-in cell"
    );
    let declared = read_json(&repo(&format!("templates/{SHELL}/template.json")));
    let version = declared["version"]
        .as_str()
        .expect("the shell declares a version");
    assert_eq!(
        marker["cell"]["template"].as_str(),
        Some(format!("{SHELL}@{version}").as_str()),
        "the declaration must name the version the library ships"
    );
    // And nothing else is in the file: a marker that carried params would be a
    // configuration decision hiding inside a declaration.
    let cell = marker["cell"].as_object().expect("cell is an object");
    assert_eq!(
        cell.keys().collect::<Vec<_>>(),
        vec!["template", "type"],
        "the marker declares a type and a template and nothing more"
    );
}

/// The root tree is three files, no cell, and exactly one edge — the mutation
/// door.
#[test]
fn the_seed_is_an_empty_hive_and_one_edge_onto_the_mutation_door() {
    if !shipped() {
        eprintln!("skipped: {SEED} did not ship (GH #49)");
        return;
    }
    // No cell is checked in: the only two `config.json` are the root hive and
    // the marker.
    let mut configs: Vec<String> = Vec::new();
    fn walk(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk(&p, base, out);
            } else if p.file_name().and_then(|n| n.to_str()) == Some("config.json") {
                out.push(p.strip_prefix(base).unwrap().to_string_lossy().into_owned());
            }
        }
    }
    walk(&repo(SEED), &repo(SEED), &mut configs);
    configs.sort();
    assert_eq!(
        configs,
        vec![
            "main/config.json".to_string(),
            format!("main/{AT}/config.json")
        ],
        "stage one checks in a root hive and a declaration, and nothing else"
    );

    let root = read_json(&repo(&format!("{SEED}/main/config.json")));
    assert_eq!(root["cell"]["type"], "hive");
    let edges = root["params"]["graph"]["edges"]
        .as_array()
        .expect("the root hive declares a graph");
    assert_eq!(
        edges.len(),
        1,
        "the root tree draws ONE edge; every further edge is a mutation's to draw"
    );
    let e = &edges[0];
    assert_eq!(e["from"], format!("./{AT}"), "the edge leaves the shell");
    assert_eq!(
        e["to"], DOOR,
        "the edge reaches the mutation door — the birth topology, or nothing"
    );
    let condition = e["condition"].as_str().unwrap_or_default();
    assert!(
        condition.contains(&format!("'{DOOR_LANE}'")),
        "the edge takes the `{DOOR_LANE}` lane only; an unconditioned edge onto the door \
         would hand every emission of the shell the whole tree: {condition}"
    );

    // The `.env.example` beside the seed carries names and no secret.
    let example = std::fs::read_to_string(repo(&format!("{SEED}/.env.example")))
        .expect("an `.env.example` ships beside the seed");
    let declared = read_json(&repo(&format!("templates/{SHELL}/template.json")));
    let env = declared["requires"]["env"]
        .as_object()
        .expect("the shell declares `requires.env`");
    for key in env.keys() {
        assert!(
            example
                .lines()
                .any(|l| l.trim_start().starts_with(&format!("{key}="))),
            "`{key}` is declared by the shell and missing from {SEED}/.env.example"
        );
    }
    for line in example.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, _) = line.split_once('=').unwrap_or_else(|| {
            panic!("{SEED}/.env.example: `{line}` is neither a comment nor `KEY=value`")
        });
        assert!(
            env.contains_key(key),
            "`{key}` stands in {SEED}/.env.example and is declared nowhere — an operator would \
             set a value that goes nowhere"
        );
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// 2 — the boot
// ──────────────────────────────────────────────────────────────────────────────

/// One declaration, and the whole shell stands.
///
/// The counts are DERIVED from the template tree rather than written down: a
/// number in a test is a second opinion, and the question here is precisely
/// whether the boot produced what the library describes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_declaration_grows_the_whole_shell() {
    if !shipped() {
        eprintln!("skipped: {SEED} did not ship (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    build_root(td.path(), &complete_env());
    let h = boot(&td).await.expect("the declaration must be fulfilled");

    // The edges the colony holds after the growth: the root tree's one, plus
    // every edge the shell brought with it.
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
    assert!(
        edges
            .iter()
            .any(|(from, to)| from == &format!("/{AT}") && to == DOOR),
        "the birth edge onto the mutation door did not survive the growth: {edges:?}"
    );

    // Nothing was refused, dropped or misrouted on the way.
    let dead = h.drain_dead_letters().await;
    assert!(
        dead.is_empty(),
        "a boot that grows a declaration must produce no dead letter: {dead:?}"
    );

    h.shutdown().await;

    // NOTE on the direct SQL: test-side reading of `colony.db`, not cell code —
    // the device of `gh424_the_seed_grows_itself`.
    let conn = rusqlite::Connection::open_with_flags(
        td.path().join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open colony.db read-only");
    let mut stmt = conn
        .prepare("SELECT path FROM registry ORDER BY path")
        .unwrap();
    let rows: Vec<String> = stmt
        .query_map([], |r| r.get(0))
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

    // Every cell of the grown tree has a registry row, and every registry row
    // stands under the shell — the root tree contributed none, because it
    // checked none in.
    let grown = count_cells(&td.path().join("main").join(AT));
    assert!(
        grown > 20,
        "the shell is a real tree, not a stub: {grown} cells on disk"
    );
    assert_eq!(
        rows.len(),
        grown,
        "every cell the growth wrote must have a registry row: {rows:?}"
    );
    for row in &rows {
        assert!(
            row.starts_with(&format!("/{AT}/")),
            "`{row}` stands outside the shell, and the root tree checked in no cell"
        );
    }

    // The occupants of the template are the hive scopes under `/os`, by name.
    let mut occupants: Vec<String> = std::fs::read_dir(repo(&format!("templates/{SHELL}")))
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| format!("/{AT}/{}", e.file_name().to_string_lossy()))
        .collect();
    occupants.sort();
    for occupant in &occupants {
        assert!(
            hives.contains(occupant),
            "`{occupant}` is an occupant of the template and no hive scope of the colony: {hives:?}"
        );
    }
    assert!(
        hives.contains(&format!("/{AT}")),
        "the shell itself is a hive"
    );

    // And the marker consumed itself: what stands at its address is the shell.
    let grown_marker = read_json(&td.path().join("main").join(AT).join("config.json"));
    assert_eq!(
        grown_marker["cell"]["type"], "hive",
        "the marker must have been replaced by what it named; a marker that survived its own \
         growth would be re-grown on every boot"
    );
}

/// Cells (never hives, never markers) under `dir`.
fn count_cells(dir: &std::path::Path) -> usize {
    let mut n = 0;
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let p = entry.path();
        if p.is_dir() {
            n += count_cells(&p);
        } else if p.file_name().and_then(|s| s.to_str()) == Some("config.json") {
            let v = read_json(&p);
            match v["cell"]["type"].as_str() {
                Some("hive") | Some("ref") | None => {}
                Some(_) => n += 1,
            }
        }
    }
    n
}

// ──────────────────────────────────────────────────────────────────────────────
// 4 — the requirement, refused before anything is written
// ──────────────────────────────────────────────────────────────────────────────

/// A declared key the colony does not hold refuses the boot as
/// `requirement_missing`, and nothing is written.
///
/// The refusal is the mutation door's own — `validate_requires`, stage 3 — run
/// against the marker read as the one-entry diff it is
/// (`crates/meclaw-colony/src/bootstrap_grow.rs`). Before GH #465 the boot was
/// the one instantiating path that never read a `requires` block: the shell was
/// grown, and the missing key surfaced at the first cycle of the control loop —
/// or, for a key with an empty default, not at all.
///
/// **Pre-destructive** is asserted on the filesystem, not inferred from the
/// error: the marker is still a marker, it has no children, and there is no
/// `colony.db` row for anything under it. A refusal that leaves half a tree
/// behind is worse than a late one, because the next boot resumes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_missing_requirement_refuses_the_boot_before_a_byte_is_written() {
    if !shipped() {
        eprintln!("skipped: {SEED} did not ship (GH #49)");
        return;
    }
    let declared = read_json(&repo(&format!("templates/{SHELL}/template.json")));
    let env = declared["requires"]["env"]
        .as_object()
        .expect("the shell declares `requires.env`");
    let (missing, decl) = env
        .iter()
        .find(|(_, d)| d["required"].as_bool().unwrap_or(true))
        .expect("the shell declares at least one required key");

    // Everything the shell asks for EXCEPT the one key under test.
    let mut incomplete = String::new();
    for (key, d) in env {
        if key != missing && d["required"].as_bool().unwrap_or(true) {
            incomplete.push_str(&format!("{key}=placeholder-for-a-test\n"));
        }
    }

    let td = tempfile::TempDir::new().unwrap();
    build_root(td.path(), &incomplete);
    let err = match boot(&td).await {
        Err(rendered) => rendered,
        Ok(h) => {
            h.shutdown().await;
            panic!("a declaration the colony cannot satisfy must not be grown");
        }
    };

    assert!(
        err.contains("requirement_missing"),
        "the boot must refuse with the door's own code: {err}"
    );
    assert!(
        err.contains(missing.as_str()),
        "the refusal must name the key an operator has to go and set: {err}"
    );
    if let Some(because) = decl["because"].as_str() {
        // The first clause is enough: the whole sentence carries backticks and
        // punctuation this assertion has no business re-typing.
        let head = because.split(',').next().unwrap_or(because);
        assert!(
            err.contains(head),
            "the refusal must quote the template's own `because`, so a reader learns WHY the \
             key exists and not only that it is absent: {err}"
        );
    }

    // Pre-destructive, measured on disk.
    let marker_dir = td.path().join("main").join(AT);
    let marker = read_json(&marker_dir.join("config.json"));
    assert_eq!(
        marker["cell"]["type"], "ref",
        "the marker must still be a marker: nothing was staged over it"
    );
    let children: Vec<String> = std::fs::read_dir(&marker_dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        children,
        vec!["config.json".to_string()],
        "the refused growth left something behind: {children:?}"
    );
    let db = td.path().join("colony.db");
    if db.is_file() {
        let conn =
            rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM registry WHERE path LIKE ?1",
                [format!("/{AT}/%")],
                |r| r.get(0),
            )
            .unwrap_or(0);
        assert_eq!(n, 0, "the refused growth persisted {n} registry rows");
    }
}
