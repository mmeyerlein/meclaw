//! GH #567 (half 2) — the credentialled wish is ONE act, and the mutation door
//! agrees.
//!
//! `gh466_grow_level_renders_the_level.rs` pins what the recipe RENDERS: one
//! declaration whose last four edges are the ones
//! `examples/organism/grow-credentials.json` carries. That is a statement about
//! a string. This file is the second opinion the string needs — the same
//! rendered declaration, handed to a real mutation door on a real colony grown
//! from the shipped templates, has to come back `Committed`, and the four
//! v-lanes have to be IN the graph the colony itself publishes, each carrying
//! the lane it declared.
//!
//! WHY IT COULD NOT COMMIT BEFORE. The lane ends on
//! `<member>/assistants/<gen>/talky/brain`, three levels inside the node the
//! declaration gives birth to, and the connect point that makes it legal
//! (`"at": ["./brain"]`) is declared by `talky`/`cogny` — several `ref` hops
//! below the template root. Until half 1 (`contracts_from_template_subtree`)
//! stage 6 read a newborn's contract from the template ROOT alone, so these
//! four edges drawn beside the `add_nodes` that creates their target's
//! great-grandparent earned `v_lane_no_connect_point` and the recipe had to
//! draw them one declaration later.
//!
//! WHY THE CELLS ARE INERT — and the guard: same device and same reason as
//! `gh302_the_stack_grows_from_templates.rs`. Every claim here is structural,
//! and two of the cell types this stack names would reach outward the moment
//! they were spawned for real. Like every template-reading test (GH #49), a
//! tree that did not ship the example or the library is SKIPPED, never judged.

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, RespawnFn, SpawnedCellKind,
    WakeFn, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{JsonValue, Message, Path, Uuid};
use meclaw_testing::{ColonyHandle, emit_one, shipped_script};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);

/// The member the wish grows its generation under — the one
/// `examples/organism/grow-member.json` creates.
const MEMBER: &str = "/os/orgs/acme/members/alex";
/// The generation the wish names. Not `scribe`: the three declarations applied
/// first stop at the member, so nothing stands in `assistants` yet, and a name
/// of its own keeps this file readable beside the example.
const GENERATION: &str = "penna";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The three declarations that have to stand before a generation can be grown:
/// shell, organisation, person. The assistant is what this file renders.
const PREFIX: [&str; 3] = [
    "examples/organism/grow-os.json",
    "examples/organism/grow-org.json",
    "examples/organism/grow-member.json",
];

/// `examples/organism`, or `false` when this tree did not ship it (GH #49).
fn shipped() -> bool {
    repo("examples/organism/grow-os.json").is_file()
}

/// Whether the whole library this example instantiates travelled with the tree.
fn library_is_complete() -> bool {
    [
        "meclaw-os",
        "org",
        "member",
        "assistant",
        "talky",
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

/// The version a shipped `template.json` declares, so that nothing here is a
/// literal a bump can silently falsify.
fn version_of(template: &str) -> String {
    let v = read_json(&repo(&format!("templates/{template}/template.json")));
    v["version"]
        .as_str()
        .unwrap_or_else(|| panic!("templates/{template}/template.json declares no version"))
        .to_string()
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
// the colony
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
fn cell_types_in(root: &std::path::Path) -> std::collections::BTreeSet<String> {
    fn walk(dir: &std::path::Path, out: &mut std::collections::BTreeSet<String>) {
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
    let mut out = std::collections::BTreeSet::new();
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
         EXAMPLE_CHAT_TOKEN=test-chat-token\n\
         KEEPER_NIGHT_CRON=0 0 0 1 1 *\n",
    )
    .unwrap();
}

async fn boot(td: &tempfile::TempDir) -> ColonyHandle {
    let types = cell_types_in(&td.path().join("templates"));
    assert!(
        types.contains("code") && types.contains("store") && types.contains("llm"),
        "the library copy named no real cell types — it failed: {types:?}"
    );
    let fs: Vec<(String, Arc<dyn CellFactory>)> = types
        .into_iter()
        .map(|t| (t, Arc::new(InertCellFactory) as Arc<dyn CellFactory>))
        .collect();
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

// ──────────────────────────────────────────────────────────────────────────────
// the wish, through the SHIPPED renderer
// ──────────────────────────────────────────────────────────────────────────────

/// The manifest the fast lane renders for a generation that holds no key of its
/// own — run through the script the template actually ships, never a copy.
fn rendered_wish() -> Vec<Value> {
    let params = json!({
        "scope": MEMBER, "level": "assistant", "name": GENERATION,
        "template": format!("assistant@{}", version_of("assistant")),
        "ctx": {"model": "${MODEL_CORE}", "model_fast": "${MODEL_CORE_FAST}",
                "model_surface": "${MODEL_SURFACE}"},
        "credential": {"cred_ref": "cred:example-provider:primary",
                       "subject": "member:alex",
                       "expires_at": "2099-01-01T00:00:00.000000Z",
                       "rule_id": "alex-credential-read"}});
    let out = emit_one(
        &shipped_script(RECIPES),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "grow_level", "request": "…",
                                         "params": params}).to_string()}],
        }),
    );
    out["manifest"]
        .as_array()
        .unwrap_or_else(|| panic!("no manifest: {out}"))
        .clone()
}

/// The same wish with the `credential` block taken out — the level as it has
/// always been rendered, through the same shipped script.
fn rendered_plain_wish() -> Vec<Value> {
    let params = json!({
        "scope": MEMBER, "level": "assistant", "name": GENERATION,
        "template": format!("assistant@{}", version_of("assistant")),
        "ctx": {"model": "${MODEL_CORE}", "model_fast": "${MODEL_CORE_FAST}",
                "model_surface": "${MODEL_SURFACE}"}});
    let out = emit_one(
        &shipped_script(RECIPES),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "grow_level", "request": "…",
                                         "params": params}).to_string()}],
        }),
    );
    out["manifest"]
        .as_array()
        .unwrap_or_else(|| panic!("no manifest: {out}"))
        .clone()
}

/// The four v-lanes the credential road is, as absolute paths, in the order the
/// renderer draws them — each with the substring its guard has to carry.
///
/// The ANSWER leg is addressed by `hop.grant_id`, and the handle it names is
/// that consumer's and nobody else's: two brains sharing one would each be
/// handed the other's sealed box. A guard that merely EXISTS would not say
/// that, so the check is the handle rather than `is_some()`.
fn expected_lanes() -> Vec<(String, String, &'static str, String)> {
    let mut want = Vec::new();
    for rim in ["talky", "cogny"] {
        let brain = format!("{MEMBER}/assistants/{GENERATION}/{rim}/brain");
        let broker = format!("{MEMBER}/access");
        let handle = format!("grant:example-provider-primary@member-alex/{rim}");
        want.push((
            brain.clone(),
            broker.clone(),
            "credential_request",
            "hop.route == 'credential_request'".to_string(),
        ));
        want.push((
            broker,
            brain,
            "in_sealed",
            format!("hop.grant_id == '{handle}'"),
        ));
    }
    want
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_credentialled_generation_commits_as_one_declaration() {
    if !shipped() || !library_is_complete() {
        return; // GH #49: a tree without the material is skipped, never judged
    }
    let td = tempfile::TempDir::new().unwrap();
    build_root(td.path());
    let h = boot(&td).await;

    for file in PREFIX {
        let outcome = mutate(&h, read_json(&repo(file))).await;
        assert!(
            matches!(outcome, MutationOutcome::Committed { .. }),
            "{file} was not committed: {outcome:?}"
        );
    }

    let decls = rendered_wish();
    assert_eq!(
        decls.len(),
        1,
        "the wish is ONE act since `builder@1.6.1`; the door below is what says \
         whether it may be: {decls:?}"
    );
    let outcome = mutate(&h, decls[0].clone()).await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the generation and its credential road refused as one declaration — \
         this is the whole of GH #567: {outcome:?}"
    );

    // The POSITIVE receipt: the colony's own topology, not the diff it was sent.
    let (ack_tx, ack_rx) = oneshot::channel::<meclaw_colony::api_dto::ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: Path::new("/"),
            ack: ack_tx,
        })
        .await
        .unwrap();
    let edges = ack_rx.await.unwrap().edges;

    for (from, to, lane, guard) in expected_lanes() {
        let found = edges
            .iter()
            .find(|e| e.from == from && e.to == to && e.lane.as_deref() == Some(lane))
            .unwrap_or_else(|| {
                panic!(
                    "no v-lane `{lane}` from {from} to {to} in the published graph; \
                     the credential edges it does carry: {:?}",
                    edges
                        .iter()
                        .filter(|e| e.lane.is_some())
                        .map(|e| (&e.from, &e.to, &e.lane))
                        .collect::<Vec<_>>()
                )
            });
        assert!(
            found
                .condition
                .as_deref()
                .unwrap_or_default()
                .contains(&guard),
            "the guard on this v-lane does not name `{guard}` — a lane addressed \
             by nothing carries everything, and an answer leg addressed by the \
             wrong handle hands a brain somebody else's sealed box: {found:?}"
        );
    }

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_wish_without_the_block_grows_the_same_generation() {
    // The control: the credential road is OPT-IN, and the level it hangs on is
    // the level either way. The SAME wish with the block taken out is rendered
    // again through the shipped script and committed — so a red case above is
    // about the four edges and never about the generation itself.
    if !shipped() || !library_is_complete() {
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    build_root(td.path());
    let h = boot(&td).await;
    for file in PREFIX {
        assert!(
            matches!(
                mutate(&h, read_json(&repo(file))).await,
                MutationOutcome::Committed { .. }
            ),
            "{file}"
        );
    }
    let plain = rendered_plain_wish();
    assert_eq!(plain.len(), 1, "a level has always been one declaration");
    assert!(
        plain[0]["diff"]["seed_rows"].is_null(),
        "nothing is seeded for a generation nobody wired that way"
    );
    let outcome = mutate(&h, plain[0].clone()).await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the level itself must commit — otherwise the credentialled case above \
         proves nothing about the road: {outcome:?}"
    );
    h.shutdown().await;
}
