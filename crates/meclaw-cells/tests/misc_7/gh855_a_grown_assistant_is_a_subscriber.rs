//! GH #855 — a generation grown by the builder in a tree that carries a model
//! registry is its subscriber: every brain has a push road onto its
//! composite's `in_model` door, and the first mutation receipt after the
//! commit makes a subscriber row of each, with the start value it was born on.
//!
//! Two halves, the cheap one first:
//!
//! 1. **The recipe.** `grow_level assistant` renders a SECOND declaration at
//!    the builder's `model_registry_scope` -- six push edges, one per brain and
//!    since GH #877 one per curator summarizer, and one announcement edge --
//!    only when that setting is set, only for the
//!    shipped `assistant` template, and only for a generation under that scope.
//!    Without it the manifest is the level and nothing else, byte for byte the
//!    one `gh466` pins. And `meclaw-os` turns it on for its own builder.
//! 2. **The tree.** The shipped shell, grown into the organism's empty seed:
//!    the rendered declarations commit at the door, the edges stand in the
//!    edge table, and the receipt that follows lands six `subscribers` rows
//!    in the registry's own store, read back out of its `cell.db` -- and,
//!    since GH #858, a member grown by the same recipe lands four more, one per
//!    memory cell, on the tokens the environment binds.
//!
//! Every non-registry cell is inert (the device of `gh302`/`gh466`): the claim
//! is about topology and rows, and nothing here may reach a provider.
//!
//! The trees boot real `code` and `store` cells, so every arm is in the
//! interleaving class (AGENTS.md rule 11): it waits for the state it claims
//! under a hang mark of `meclaw_testing` and judges that state, never a clock.
//! The colony's log and edge table are asked through the colony (TV-A3), and
//! a final state is read once the event it follows has been seen.

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_colony::api_dto::{MessageLogDto, MessageLogFilter};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, RespawnFn, SpawnedCellKind,
    WakeFn, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, JsonValue, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::{ColonyHandle, wait};
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::collections::BTreeSet;
use std::sync::Arc;
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
    assert_eq!(
        pushes.len(),
        6,
        "one push edge per brain, and since GH #877 one per curator summarizer: {pushes:?}"
    );
    let cells = [
        ("talky", "brain"),
        ("talky-chat", "brain"),
        ("cogny", "brain"),
        ("talky", "curator/summarizer"),
        ("talky-chat", "curator/summarizer"),
        ("cogny", "curator/summarizer"),
    ];
    for ((rim, cell), (to, cond)) in cells.iter().zip(&pushes) {
        // A summarizer's push rides onto the rim it stands behind; that rim's
        // `in_model` door hands it on to the curator (OR-KX-P5).
        assert_eq!(to, &format!("./acme/members/alex/assistants/scribe/{rim}"));
        assert!(
            cond.contains("hop.route == 'in_model'")
                && cond.contains(&format!("hop.subscriber == '{GEN}/{rim}/{cell}'")),
            "{cond}"
        );
    }
    // GH #863: the road has a way back too, one edge per composite, drawn
    // after the announcement -- so the announcement is the edge on the receipt.
    let back: Vec<Value> = edges
        .iter()
        .filter(|e| {
            e["condition"]
                .as_str()
                .is_some_and(|c| c.contains("'model_refused'"))
        })
        .cloned()
        .collect();
    let want: Vec<Value> = ["cogny", "talky", "talky-chat"]
        .iter()
        .map(|rim| {
            json!({"from": format!("./acme/members/alex/assistants/scribe/{rim}"), "to": ".",
                   "condition": "has(hop.route) && hop.route == 'model_refused'"})
        })
        .collect();
    assert_eq!(
        back, want,
        "one way back per composite, and nothing else on it: {edges:?}"
    );
    let announce: Vec<&Value> = edges
        .iter()
        .filter(|e| {
            e["to"] == "."
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains("'mutation_committed'"))
        })
        .collect();
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
             "requirement": need("cogny/brain")},
            {"cell_path": format!("{GEN}/talky/curator/summarizer"), "start_model": "${MODEL_CORE}",
             "requirement": need("curator/summarizer")},
            {"cell_path": format!("{GEN}/talky-chat/curator/summarizer"),
             "start_model": "${MODEL_CORE}", "requirement": need("curator/summarizer")},
            {"cell_path": format!("{GEN}/cogny/curator/summarizer"), "start_model": "${MODEL_CORE}",
             "requirement": need("curator/summarizer")}
        ])
    );

    // A template that is not the shipped generation names its own brains, and
    // the recipe does not guess them.
    let other = render(json!({"model_registry_scope": SCOPE}), "egon@2.1.7");
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
    // GH #1061 (#801): the shell's translator names a grant and asks the
    // shell's broker, and this tree deposits no key -- every translation
    // would be refused and no package would ever be pushed. Anonymous, it
    // speaks to the stub as it did when the key came out of `.env`.
    let marker = root.join("templates/meclaw-os/llm-registry/config.json");
    let mut m: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(&marker).expect("the shell's registry marker"),
    )
    .expect("the marker parses");
    m["override_params"]["translate"]["credential_grant_id"] = json!("");
    std::fs::write(
        &marker,
        meclaw_core::serde_json::to_string_pretty(&m).unwrap(),
    )
    .expect("write the marker");
    std::fs::write(
        root.join(".env"),
        "OPENROUTER_API_KEY=test-key\n\
         MODEL_CORE=m-core\n\
         MODEL_CORE_FAST=m-core-fast\n\
         MODEL_SURFACE=m-surface\n\
         MODEL_CLOSER=m-closer\nMODEL_DIALECTIC=m-dialectic\nMODEL_FILE_SPACE=m-file-space\nMODEL_DREAMER=m\nMODEL_BRAIN=m\n",
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
        (
            format!("{GEN}/cogny/curator/summarizer"),
            "m-core".to_string(),
        ),
        (format!("{GEN}/talky-chat/brain"), "m-surface".to_string()),
        (
            format!("{GEN}/talky-chat/curator/summarizer"),
            "m-core".to_string(),
        ),
        (format!("{GEN}/talky/brain"), "m-surface".to_string()),
        (
            format!("{GEN}/talky/curator/summarizer"),
            "m-core".to_string(),
        ),
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
    // GH #1069: waited on the rows as an event, like the two push tests
    // below. A fixed 30 s window here saw only the judge's row in 3 of 5
    // iterations of a loaded series (build03, 12 threads beside 24 busy
    // loops, 2026-10-07): the generation was still being grown. And waited on
    // the rows THEMSELVES, never on their number: a count of eleven was
    // reached once with the closer's row still missing (K9, build04, 1 of 5
    // under an 80 % quota) -- a number is the wrong event. The generation is
    // grown cell by cell, so the wait carries the boot mark, and the rows are
    // read once more for the verdict. No settle barrier: the grown organism
    // never goes quiet -- its clock cells tick, and work addressed to a cell
    // that is parked lazily waits in that cell's mailbox, which `Settle`
    // counts as pending -- so the barrier only hangs (measured: `HANG: the
    // colony settles`, build01, 2026-10-15).
    //
    // The read after the wait is a claim of its own (GH #1199, #1093): a row
    // once written never goes missing again. Every receipt of the growth
    // announces the member once more, and until 0.64.1 an announcement that
    // read a row as unknown wrote it as a delete and an insert, two legs of a
    // bundle that is no transaction -- this read fell between them (the
    // coverage of a20e7d3fe lost the closer's row here; #1093 the judge's).
    // The registry writes such a row in one step now, pinned deterministically
    // by the `gh1199_*` tests at the end of this file.
    let rows = || find_store_db(root).map_or_else(Vec::new, |db| subscriber_rows(&db));
    wait::boot_until_with(
        "every subscriber row of the grown generation and member",
        || {
            let now = rows();
            let want = &want;
            async move { now == *want }
        },
        rows,
    )
    .await;
    let got = rows();
    assert_eq!(
        got, want,
        "the grown generation's three brains and three curator summarizers (GH #877) and \
         the member's four memory cells are subscribers, with the start values the substitution resolved, beside the shell's own \
         judge"
    );
    // GH #863: and each composite has its way back in the edge table -- three
    // for the generation, one for the member's memory -- at the container the
    // pushes arrive at.
    let mut back = ways_back_committed(&h).await;
    back.sort();
    let mut want_back: Vec<(String, String)> = [
        format!("{GEN}/cogny"),
        format!("{GEN}/talky"),
        format!("{GEN}/talky-chat"),
        format!("{MEMBER}/memory-hive"),
    ]
    .into_iter()
    .map(|from| (from, SCOPE.to_string()))
    .collect();
    want_back.sort();
    assert_eq!(back, want_back, "one way back per composite, and no other");
    h.shutdown().await;
}

/// The committed edges into the container that name the refusal lane,
/// `(from, to)`, out of the colony's edge table -- asked of the colony that
/// owns it (`ReadGraph`), never read out of its database (TV-A3): an edge
/// stands there only once its mutation committed, so the road is real rather
/// than merely un-refused. Matched on the lane's NAME anywhere in the
/// condition or the modifier, not on the one rendered condition: "and no
/// other" has to see a way back in any other form too (review R2 M-6; the form
/// itself is `gh863_every_model_door_has_a_way_back.rs`'s to hold).
async fn ways_back_committed(h: &ColonyHandle) -> Vec<(String, String)> {
    let (ack, rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: Path::new(SCOPE),
            ack,
        })
        .await
        .expect("the colony takes the graph read");
    let graph = meclaw_testing::deadline::hang("the colony answers the graph read", rx)
        .await
        .expect("the colony answered the graph read");
    graph
        .edges
        .into_iter()
        .filter(|e| e.to == SCOPE)
        .filter(|e| {
            e.condition
                .as_deref()
                .unwrap_or_default()
                .contains("model_refused")
                || e.modifier
                    .as_ref()
                    .is_some_and(|m| m.to_string().contains("model_refused"))
        })
        .map(|e| (e.from, e.to))
        .collect()
}

/// `(model_id, refused)` of one subscriber row, read out of the registry
/// store's own `cell.db`; `None` while the row, or the column, is not there.
fn refusal_of(db: &std::path::Path, cell_path: &str) -> Option<(String, String)> {
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .ok()?;
    conn.query_row(
        "SELECT COALESCE(model_id, ''), COALESCE(refused, '') FROM subscribers \
         WHERE cell_path = ?1",
        [cell_path],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .ok()
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

/// The log rows delivered to `to` and below, through the colony (TV-A3).
async fn logged(h: &ColonyHandle, to: &str) -> Vec<MessageLogDto> {
    h.read_log(MessageLogFilter {
        to_path_prefix: Some(to.to_string()),
        ..MessageLogFilter::default()
    })
    .await
}

/// Until the colony's log holds a push naming `model` delivered to `to` --
/// an event of the log, under the hang mark; the failure names what the log
/// did deliver to `to`.
async fn until_pushed(h: &ColonyHandle, to: &str, model: &str, what: &str) {
    let mut moved = h.subscribe_log_seq();
    let last = std::cell::RefCell::new(Vec::<String>::new());
    meclaw_testing::deadline::hang_with(
        what,
        async {
            loop {
                // Marked BEFORE the read: a row committed after it moves the
                // counter, and the wait below wakes for it.
                moved.borrow_and_update();
                let rows = logged(h, to).await;
                if rows.iter().any(|r| is_push(r, to, model)) {
                    return;
                }
                *last.borrow_mut() = rows
                    .iter()
                    .map(|r| format!("{} -> {} {}", r.from_path, r.to_path, r.headers_json))
                    .collect();
                moved.changed().await.expect("the log writer is alive");
            }
        },
        || last.borrow().clone(),
    )
    .await;
}

/// Whether `row` is a push naming `model` delivered on the `in_model` door of
/// `to` exactly.
fn is_push(row: &MessageLogDto, to: &str, model: &str) -> bool {
    row.to_path == to
        && row.headers_json.contains("\"in_model\"")
        && (row.headers_json.contains(model)
            || row
                .body_payload
                .as_deref()
                .unwrap_or_default()
                .contains(model))
}

/// The brain's provider calls, waited for until there are more than `before`:
/// an event of the mock, under the hang mark.
async fn the_next_call(mock: &MockOpenAI, before: usize, what: &str) -> Value {
    wait::until_with(
        what,
        || async { mock.recorded_requests().await.len() > before },
        || before,
    )
    .await;
    let reqs = mock.recorded_requests().await;
    reqs[before].body.clone()
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
    // One canned answer for each of the two turns below, and room to spare.
    let mock = MockOpenAI::start(
        (0..128)
            .map(|i| canned_chat_completion(&format!("answer {i}"), "stop"))
            .collect(),
    )
    .await;
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    copy_tree(&repo("examples/organism/seed"), root);
    copy_tree(&repo("templates"), &root.join("templates"));
    // GH #1061 (#801): the shell's translator names a grant and asks the
    // shell's broker, and this tree deposits no key -- every translation
    // would be refused and no package would ever be pushed. Anonymous, it
    // speaks to the stub as it did when the key came out of `.env`.
    let marker = root.join("templates/meclaw-os/llm-registry/config.json");
    let mut m: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(&marker).expect("the shell's registry marker"),
    )
    .expect("the marker parses");
    m["override_params"]["translate"]["credential_grant_id"] = json!("");
    std::fs::write(
        &marker,
        meclaw_core::serde_json::to_string_pretty(&m).unwrap(),
    )
    .expect("write the marker");
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
         MODEL_CLOSER=m\nMODEL_DIALECTIC=m\nMODEL_FILE_SPACE=m\nMODEL_DREAMER=m\nMODEL_BRAIN=m\n",
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
    // subscribed (`no_subscriber`, templates/llm-registry/hand,
    // `script_inline`: the `override_set` / `scope == "target"` refusal).
    //
    // GH #1069: waited on THE BRAIN's row, as an event. This used to wait for
    // "three rows" inside a fixed 30 s and walk on silently at its end -- but
    // the shell's judge and the member's four memory cells are five rows
    // before the generation's receipt is read at all, so under host load
    // (gate 2026-10-07, 1320 s) the operator's command overtook the brain's
    // own row, was refused, and the push the turns below wait for never left.
    // The generation is grown cell by cell: the boot mark.
    let rows = || find_store_db(root).map_or_else(Vec::new, |db| subscriber_rows(&db));
    wait::boot_until_with(
        "the grown brain's subscriber row",
        || {
            let here = rows().iter().any(|(p, _)| *p == brain);
            async move { here }
        },
        rows,
    )
    .await;

    // Before the push the brain speaks on its start value. One turn, sent as
    // the composite's own curator, the cell that feeds the brain since GH #889
    // (it was the collector before); what is measured is the brain's wire, not
    // the turn.
    let turn = || {
        MessageBuilder::new(Path::new(&brain))
            .body(Body::Inline(json!({"messages": [
                {"origin": "user", "type": "text", "text": "which model?"}]})))
            .ttl(200)
            .build()
    };
    let curator = Path::new(&format!("{GEN}/talky/curator"));
    let before = mock.recorded_requests().await.len();
    h.send_from(curator.clone(), turn()).await;
    let first = the_next_call(&mock, before, "the grown brain's call before the push").await;
    assert_eq!(
        first["model"], "m-surface",
        "before the push: the start value: {first}"
    );

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

    // GH #1069: the push delivered at the brain, waited for as an event in the
    // colony's own log -- a road of several script cells (registry, shell,
    // `./orgs`, `talky` and the door) that host load stretches, so no clock
    // judges it. The brain takes its mailbox in order, so the turn sent after
    // this delivery is read after the push: ONE turn is the whole claim.
    until_pushed(
        &h,
        &brain,
        M2,
        "the push of the replacement delivered to the grown brain",
    )
    .await;
    let before = mock.recorded_requests().await.len();
    h.send_from(curator, turn()).await;
    let last = the_next_call(&mock, before, "the grown brain's call after the push").await;
    assert_eq!(
        last["model"], M2,
        "the push reached the grown brain, and the turn after it names the replacement: {last}"
    );
    assert_eq!(last["max_tokens"], 777, "and its parameters: {last}");
    assert!(
        system_text(&last).starts_with(PROMPT),
        "the model's prompt block is the FIRST thing in the system part: {:?}",
        system_text(&last)
    );
    h.shutdown().await;
}

/// GH #863 (rev-2 I-1): a push the grown brain REFUSES, on the road the tree
/// ships. The operator sets a replacement onto a catalogue row whose endpoint
/// the brain has no `base_url_allow` for; the registry resolves it and pushes
/// it down the same road as above, and the brain refuses it. The refusal leaves
/// `talky` as `model_refused`, the way back `grow_level assistant` drew carries
/// it to `/os/orgs`, the shell's mirror of the push edge restamps it
/// `in_refused`, and the registry marks the row -- the row `show` reads, read
/// here where it lands, in the registry store's own `cell.db`, because the
/// shell's rim has no reader in this tree. Nothing of it dead-letters.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_push_on_the_shipped_road_reaches_show() {
    if !shipped() {
        return;
    }
    const M2: &str = "test/m2";
    let brain = format!("{GEN}/talky/brain");
    let mock = MockOpenAI::start(vec![]).await;
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    copy_tree(&repo("examples/organism/seed"), root);
    copy_tree(&repo("templates"), &root.join("templates"));
    // GH #1061 (#801): the shell's translator names a grant and asks the
    // shell's broker, and this tree deposits no key -- every translation
    // would be refused and no package would ever be pushed. Anonymous, it
    // speaks to the stub as it did when the key came out of `.env`.
    let marker = root.join("templates/meclaw-os/llm-registry/config.json");
    let mut m: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(&marker).expect("the shell's registry marker"),
    )
    .expect("the marker parses");
    m["override_params"]["translate"]["credential_grant_id"] = json!("");
    std::fs::write(
        &marker,
        meclaw_core::serde_json::to_string_pretty(&m).unwrap(),
    )
    .expect("write the marker");
    // The catalogue row the replacement points at: an endpoint of its own,
    // which the brain was not born on and has no allow list for.
    let seed = root.join("templates/llm-registry/store/seed/models.jsonl");
    let mut rows = std::fs::read_to_string(&seed).unwrap();
    if !rows.ends_with('\n') {
        rows.push('\n');
    }
    rows.push_str(
        &json!({"model_id": M2, "provider": "gateway",
                "base_url": "https://elsewhere.example/v1", "wire_dialect": "",
                "context_window": 32000, "cost_in": 1, "cost_out": 1, "caps": {},
                "traits": {}, "status": "active", "note": "TEST ROW", "package": {},
                "prompt": ""})
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
         MODEL_CLOSER=m\nMODEL_DIALECTIC=m\nMODEL_FILE_SPACE=m\nMODEL_DREAMER=m\nMODEL_BRAIN=m\n",
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
    // GH #1069: the brain's own row, as an event -- the operator's command
    // must not overtake it (the push test above says why). The generation is
    // grown cell by cell: the boot mark.
    let rows = || find_store_db(root).map_or_else(Vec::new, |db| subscriber_rows(&db));
    wait::boot_until_with(
        "the grown brain's subscriber row",
        || {
            let here = rows().iter().any(|(p, _)| *p == brain);
            async move { here }
        },
        rows,
    )
    .await;

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

    // GH #1069: the refusal booked on the brain's row, as an event: the row
    // where it lands, read until it says so, under the hang mark. That booking
    // is also the marker for the dead letters below: the colony books a dead
    // letter in the same routing step that hands the message on, so every hop
    // the refusal took to reach its row -- and every copy it shed on the way
    // -- was routed, and any dead letter of it booked, before the row could
    // say so. No settle barrier: the grown organism never goes quiet (see
    // the first test of this file).
    let refusal = || find_store_db(root).and_then(|db| refusal_of(&db, &brain));
    wait::until_with(
        "the brain's refusal booked on its subscriber row",
        || {
            let booked = refusal().is_some_and(|(_, r)| !r.is_empty());
            async move { booked }
        },
        refusal,
    )
    .await;
    let seen = refusal();
    let dead: Vec<String> = h
        .drain_dead_letters()
        .await
        .iter()
        .filter(|d| d.message.headers.hop.contains_key("refused_subscriber"))
        .map(|d| {
            format!(
                "{} -> {}: {:?} hop={:?}",
                d.sender_path.as_str(),
                d.resolved_target.as_str(),
                d.reason,
                d.message.headers.hop
            )
        })
        .collect();
    assert!(
        dead.is_empty(),
        "the refusal dead-lettered on its way back: {dead:#?}"
    );
    let (model, refused) = seen.expect("the brain's subscriber row");
    assert_eq!(model, M2, "the row names what was resolved and sent");
    assert!(
        refused.contains("base_url"),
        "and why the brain refused it, which `show` hands out: {refused:?}"
    );
    assert!(
        mock.recorded_requests().await.is_empty(),
        "no push reached the provider"
    );
    h.shutdown().await;
}

// ---- GH #1199 / #1093: the registry announces a subscriber row in ONE step ----
//
// Every receipt of a growth announces the member again, and announcements in
// flight together each read a row as unknown (measured: `hand_subscribed` five
// times for each memory cell). The store runs one bundle after the other, so
// the interleaving is: both reads, then the first write bundle, then the
// second. These pins replay exactly that order, deterministically: the hand
// is the shipped script (`python3 -c`, as the code cell runs it), the store is
// the shipped registry schema under the store's own ops, and each leg of the
// second write bundle is applied on its own -- the state between two legs is
// what a reader beside the lane (the coverage of a20e7d3fe; #1093) could see.

const PIN_GEN: &str = "/os/orgs/acme/members/alex/assistants/pin";
const PIN_BRAIN: &str = "/os/orgs/acme/members/alex/assistants/pin/talky/brain";

/// The shipped registry store's tables, empty, under the store's own DDL.
fn pin_store() -> rusqlite::Connection {
    let raw = read_json(&repo("templates/llm-registry/store/config.json"));
    let params =
        meclaw_cells::store::StoreParams::parse(&raw["params"]).expect("the store params parse");
    let conn = rusqlite::Connection::open_in_memory().expect("an in-memory store");
    meclaw_cells::store::ddl::apply_declared_schema_ddl(&conn, &params)
        .expect("the declared schema applies");
    conn
}

/// One store op, as the store cell runs it; the outcome's payload.
fn pin_op(conn: &rusqlite::Connection, args: &Value) -> Value {
    let o = meclaw_cells::store::ops::dispatch(conn, args)
        .unwrap_or_else(|e| panic!("the store refused {args} as input: {e}"));
    o.payload
}

/// The shipped hand, run once on `doc` (stdin), as the code cell runs it.
fn pin_hand(doc: &Value) -> Vec<Value> {
    use std::io::Write as _;
    let cfg = read_json(&repo("templates/llm-registry/hand/config.json"));
    let script = cfg["params"]["script_inline"]
        .as_str()
        .expect("the hand's script");
    let mut child = std::process::Command::new("python3")
        .arg("-c")
        .arg(script)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("python3 runs");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(doc.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("the hand ends");
    assert!(
        out.status.success(),
        "the hand failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    meclaw_core::serde_json::from_slice(&out.stdout).expect("the hand answers a JSON list")
}

/// The store ops of a hand message to the store, in call order.
fn pin_legs(msg: &Value) -> Vec<Value> {
    msg["messages"]
        .as_array()
        .expect("a store bundle")
        .iter()
        .map(|t| meclaw_core::serde_json::from_str(t["text"].as_str().unwrap()).unwrap())
        .collect()
}

/// The hand's one store bundle among its messages, by phase.
fn pin_bundle(out: &[Value], phase: &str) -> Value {
    out.iter()
        .find(|m| m["header"]["route"] == "rstore" && m["header"]["phase"] == phase)
        .cloned()
        .unwrap_or_else(|| panic!("no `{phase}` store bundle in {out:#?}"))
}

/// One command or announcement, read against the store as it stands NOW:
/// the hand's `world` read is answered here, and everything the hand decides
/// on it is returned unapplied -- its write bundle runs later, after other
/// bundles.
fn pin_decide(conn: &rusqlite::Connection, doc: &Value) -> Vec<Value> {
    let read = pin_bundle(&pin_hand(doc), "world");
    let results: Vec<Value> = pin_legs(&read)
        .iter()
        .map(|leg| {
            json!({"origin": "tool", "type": "tool_result",
                   "text": pin_op(conn, leg).to_string()})
        })
        .collect();
    pin_hand(&json!({
        "envelope": {"header": {"hop": {"operation": "bundle"}, "context": {
            "lr_phase": "world",
            "lr_carry": read["header"]["carry"]}}},
        "body": {"messages": results},
        "params": {}}))
}

/// The tree's announcement of the brain, as the generation's edge stamps it.
fn pin_announcement() -> Value {
    let entries = json!([{"cell_path": PIN_BRAIN, "start_model": "m-start"}]);
    json!({
        "envelope": {"header": {"hop": {}, "context": {
            "model_announcer": PIN_GEN,
            "model_generation": PIN_GEN,
            "model_announced": entries.to_string()}}},
        "body": {"messages": []},
        "params": {}})
}

/// One announcement of the brain read as unknown: its write bundle (the
/// `landed` phase -- the hand settles the row on the store's answer to it),
/// unapplied.
fn pin_announce(conn: &rusqlite::Connection) -> Value {
    pin_bundle(&pin_decide(conn, &pin_announcement()), "landed")
}

/// The brain's subscriber rows: (model_id, rank, package_hash, refused).
fn pin_rows(conn: &rusqlite::Connection) -> Vec<(String, String, String, String)> {
    let mut st = conn
        .prepare(
            "SELECT coalesce(model_id,''), coalesce(rank,''), coalesce(package_hash,''), \
             coalesce(refused,'') FROM subscribers WHERE cell_path = ?1",
        )
        .unwrap();
    st.query_map([PIN_BRAIN], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

/// The race of GH #1199: two announcements read the brain as unknown, the
/// first one's bundle writes and settles the row (a targeted override moves it
/// off its start value, so a package is pushed), and the cell refuses that
/// package. Returns the store, the second announcement's write bundle (still
/// to run), and the row as it stands before it runs.
fn pin_race() -> (
    rusqlite::Connection,
    Vec<Value>,
    (String, String, String, String),
) {
    let conn = pin_store();
    pin_op(
        &conn,
        &json!({"operation": "insert", "table": "models", "row": {
            "model_id": "m-target", "provider": "p", "base_url": "http://127.0.0.1:9/v1",
            "wire_dialect": "chat", "context_window": 8192, "status": "active"}}),
    );
    pin_op(
        &conn,
        &json!({"operation": "insert", "table": "overrides", "row": {
            "id": "o-1", "scope": "target", "match": PIN_BRAIN, "model_id": "m-target",
            "since": "2026-10-15T00:00:00Z", "decided_by": "operator", "active": 1}}),
    );
    let first = pin_announce(&conn);
    let second = pin_announce(&conn);
    for leg in pin_legs(&first) {
        pin_op(&conn, &leg);
    }
    pin_op(
        &conn,
        &json!({"operation": "update", "table": "subscribers",
                "set": {"refused": "the cell refused the package", "refused_at": "t1"},
                "where": {"cell_path": PIN_BRAIN}}),
    );
    let booked = pin_rows(&conn);
    assert_eq!(booked.len(), 1, "the first announcement wrote one row");
    let booked = booked.into_iter().next().unwrap();
    assert_eq!(
        (booked.0.as_str(), booked.1.as_str()),
        ("m-target", "target"),
        "precondition: the first announcement settled the row off its start value"
    );
    assert!(!booked.2.is_empty(), "precondition: a package was pushed");
    (conn, pin_legs(&second), booked)
}

/// Pin (a): the row never goes missing while the second announcement runs --
/// not after any one of its legs.
#[test]
fn gh1199_a_second_announcement_in_flight_never_takes_the_row_away() {
    if !shipped() {
        return;
    }
    let (conn, legs, _) = pin_race();
    for (i, leg) in legs.iter().enumerate() {
        pin_op(&conn, leg);
        assert_eq!(
            pin_rows(&conn).len(),
            1,
            "after leg {i} of the second announcement ({leg}) the brain has exactly one row"
        );
    }
}

/// Pin (b): what was booked on the row since the first announcement -- the
/// resolution and the refusal -- stands after the second one.
#[test]
fn gh1199_a_second_announcement_in_flight_leaves_the_booked_refusal() {
    if !shipped() {
        return;
    }
    let (conn, legs, booked) = pin_race();
    for leg in &legs {
        pin_op(&conn, leg);
    }
    assert_eq!(
        pin_rows(&conn),
        vec![booked],
        "the late announcement wrote nothing over the settled, refused row"
    );
}

/// Pin (c): a write of the second announcement that fails loses no row. The
/// failure is injected where a loaded store fails it: the insert.
#[test]
fn gh1199_an_announcement_whose_write_fails_keeps_the_row() {
    if !shipped() {
        return;
    }
    let (conn, legs, booked) = pin_race();
    conn.execute_batch(
        "CREATE TRIGGER pin_fail BEFORE INSERT ON subscribers \
         BEGIN SELECT RAISE(ABORT, 'injected insert failure'); END;",
    )
    .unwrap();
    for leg in &legs {
        pin_op(&conn, leg);
    }
    assert_eq!(
        pin_rows(&conn),
        vec![booked],
        "the row the store held survives a failed announcement write"
    );
}

// ---- GH #1199, re-check I1: two DIFFERENT statements of one row in flight ----
//
// An operator `subscribe` that states a tier and the tree's announcement of
// the same brain both read the row as unknown. Whichever bundle runs second
// finds the row standing: its `if_absent` insert writes nothing. The hand
// decides on the store's answer -- `rows_affected` of that leg -- what it
// pushes and journals, so the row and the package the cell runs always agree:
// the loser pushes nothing of its own, reads the row again, and settles it the
// way it settles a known row.

/// The shipped store as a store cell answers ONE hand message: every leg in
/// call order; two or more are a bundle with `results[]` (GH #295), one op
/// carries its metadata on the hop.
fn pin_reply(conn: &rusqlite::Connection, msg: &Value) -> Value {
    let turns = msg["messages"].as_array().expect("a store message");
    let (mut messages, mut results, mut sum, mut errors) = (vec![], vec![], 0i64, 0i64);
    for t in turns {
        let args: Value = meclaw_core::serde_json::from_str(t["text"].as_str().unwrap()).unwrap();
        let o = meclaw_cells::store::ops::dispatch(conn, &args)
            .unwrap_or_else(|e| panic!("the store refused {args} as input: {e}"));
        let text = o
            .error_text
            .clone()
            .unwrap_or_else(|| o.payload.to_string());
        messages.push(
            json!({"origin": "tool", "type": "tool_result", "id": t["id"],
                             "text": text}),
        );
        let mut r = json!({"tool_call_id": t["id"], "operation": o.operation,
                           "rows_affected": o.rows_affected, "duration_ms": 0});
        if let Some(code) = o.error_code {
            r["error_code"] = json!(code);
            errors += 1;
        }
        sum += o.rows_affected;
        results.push(r);
    }
    let (hop, body) = if turns.len() > 1 {
        (
            json!({"operation": "bundle", "rows_affected": sum, "duration_ms": 0,
                   "bundle_errors": errors}),
            json!({"messages": messages, "results": results}),
        )
    } else {
        let mut hop = results[0].clone();
        hop.as_object_mut().unwrap().remove("tool_call_id");
        (hop, json!({"messages": messages}))
    };
    json!({
        "envelope": {"header": {"hop": hop, "context": {
            "lr_phase": msg["header"]["phase"],
            "lr_carry": msg["header"]["carry"]}}},
        "body": body,
        "params": {}})
}

/// Runs everything the hand sends to its store, and everything the hand
/// answers to the store's replies, until nothing goes to the store any more.
/// Returns every other message the hand emitted on the way, in order.
fn pin_drive(conn: &rusqlite::Connection, out: Vec<Value>) -> Vec<Value> {
    let mut queue: std::collections::VecDeque<Value> = out.into();
    let mut seen = vec![];
    let mut rounds = 0;
    while let Some(m) = queue.pop_front() {
        if m["header"]["route"] == "rstore" {
            rounds += 1;
            assert!(rounds < 16, "the hand keeps talking to its store: {m}");
            queue.extend(pin_hand(&pin_reply(conn, &m)));
        } else {
            seen.push(m);
        }
    }
    seen
}

/// An operator's `subscribe` of the brain, stating its tier.
fn pin_operator_subscribe() -> Value {
    let cmd = json!({"op": "subscribe", "cell_path": PIN_BRAIN, "start_model": "m-start",
                     "tier": "fast"});
    json!({
        "envelope": {"header": {"hop": {}, "context": {"actor": "operator"}}},
        "body": {"messages": [{"origin": "assistant", "type": "tool_call", "id": "c-1",
                               "text": cmd.to_string()}]},
        "params": {}})
}

/// A store whose tier `fast` means `m-fast`: the operator's statement moves
/// the brain off its start value (a package is pushed), the tree's does not.
fn pin_tier_store() -> rusqlite::Connection {
    let conn = pin_store();
    pin_op(
        &conn,
        &json!({"operation": "insert", "table": "models", "row": {
            "model_id": "m-fast", "provider": "p", "base_url": "http://127.0.0.1:9/v1",
            "wire_dialect": "chat", "context_window": 8192, "status": "active"}}),
    );
    pin_op(
        &conn,
        &json!({"operation": "insert", "table": "tiers", "row": {
            "tier": "fast", "model_id": "m-fast", "since": "2026-10-15T00:00:00Z",
            "decided_by": "operator", "active": 1}}),
    );
    conn
}

/// The brain's row: (tier, model_id, rank, package_hash).
fn pin_row(conn: &rusqlite::Connection) -> Vec<(String, String, String, String)> {
    let mut st = conn
        .prepare(
            "SELECT coalesce(tier,''), coalesce(model_id,''), coalesce(rank,''), \
             coalesce(package_hash,'') FROM subscribers WHERE cell_path = ?1",
        )
        .unwrap();
    st.query_map([PIN_BRAIN], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

/// The pushes among the hand's messages that reach the brain: (model_id, rank).
fn pin_pushes(seen: &[Value]) -> Vec<(String, String)> {
    seen.iter()
        .filter(|m| m["header"]["route"] == "update" && m["header"]["subscriber"] == PIN_BRAIN)
        .map(|m| {
            (
                m["header"]["model_id"].as_str().unwrap_or("").to_string(),
                m["header"]["rank"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

/// How often the journal booked the brain as subscribed.
fn pin_subscribed(conn: &rusqlite::Connection) -> i64 {
    conn.query_row(
        "SELECT count(*) FROM resolutions WHERE cell_path = ?1 AND reason = 'hand_subscribed'",
        [PIN_BRAIN],
        |r| r.get(0),
    )
    .unwrap()
}

/// Pin (d): the operator's subscription runs second and finds the tree's row.
/// Its tier lands on the row, and the package the cell is sent is the one the
/// row records -- not a package of a statement the store never took.
#[test]
fn gh1199_an_operator_subscription_that_loses_the_race_lands_and_pushes_what_the_row_says() {
    if !shipped() {
        return;
    }
    let conn = pin_tier_store();
    let operator = pin_decide(&conn, &pin_operator_subscribe());
    let tree = pin_decide(&conn, &pin_announcement());
    let tree_seen = pin_drive(&conn, tree);
    assert!(
        pin_pushes(&tree_seen).is_empty(),
        "precondition: the tree's announcement keeps the start value and pushes nothing"
    );
    let seen = pin_drive(&conn, operator);
    let rows = pin_row(&conn);
    assert_eq!(rows.len(), 1, "one row of the brain: {rows:?}");
    let (tier, model_id, rank, hash) = rows[0].clone();
    assert_eq!(
        tier, "fast",
        "the tier the operator stated stands on the row, it was not dropped by the race"
    );
    assert_eq!(
        (model_id.as_str(), rank.as_str()),
        ("m-fast", "tier"),
        "the row resolves the stated tier"
    );
    assert!(!hash.is_empty(), "the row records the package it was sent");
    assert_eq!(
        pin_pushes(&seen).last(),
        Some(&(model_id, rank)),
        "the last package pushed to the cell is the one its row records"
    );
}

/// Pin (e): the tree's announcement runs second and finds the operator's row.
/// It pushes nothing of its own and books no subscription it did not write;
/// what the operator stated and pushed stands.
#[test]
fn gh1199_an_announcement_that_loses_the_race_books_and_pushes_nothing() {
    if !shipped() {
        return;
    }
    let conn = pin_tier_store();
    let tree = pin_decide(&conn, &pin_announcement());
    let operator = pin_decide(&conn, &pin_operator_subscribe());
    let operator_seen = pin_drive(&conn, operator);
    assert_eq!(
        pin_pushes(&operator_seen),
        vec![("m-fast".to_string(), "tier".to_string())],
        "precondition: the operator's subscription pushed its tier's package"
    );
    let booked = pin_row(&conn);
    let seen = pin_drive(&conn, tree);
    assert!(
        pin_pushes(&seen).is_empty(),
        "the announcement that lost pushed nothing: {seen:#?}"
    );
    assert_eq!(
        pin_row(&conn),
        booked,
        "the operator's row stands as it was booked"
    );
    assert_eq!(
        pin_subscribed(&conn),
        1,
        "the journal books the brain subscribed once -- by the statement that wrote the row"
    );
}
