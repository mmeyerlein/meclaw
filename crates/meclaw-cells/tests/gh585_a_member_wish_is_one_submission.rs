//! GH #585 — one wish is ONE submission, and the order inside it is the door's
//! to keep.
//!
//! WHAT THIS FILE IS
//! =================
//! Since GH #543 a member wish rendered TWO manifests: the person first, the
//! screen and the app second. The order between them was semantics — the second
//! draws into `<member>/channels`, a scope only the first creates — but the two
//! left `recipes` as two SUBMISSIONS in the same turn, and a front has no
//! ordering across two submissions. Measured on a fresh colony: the device
//! manifest reached the door first, was refused with `edge_schema` for a scope
//! that did not exist yet, and the member committed after it. The result was a
//! member without a screen and without an app, and the wish reported success.
//!
//! Two things are measured here, in this order, because the second is only the
//! right repair if the first holds:
//!
//! 1. **The door already sequences one submission.** A manifest is an ordered
//!    list rolled off entry by entry through the same `handle_mutation` a single
//!    body takes, so entry 2 is judged against the tree entry 1 just grew. That
//!    is measured against a real colony rather than read off a doc comment —
//!    one submission carrying the member, its screen and its app commits all
//!    three.
//! 2. **The wish is one submission.** Every manifest a member wish renders is
//!    knocked on the door in the WORST order the front could pick. There is
//!    nothing to get wrong when there is only one, which is exactly the claim:
//!    the ordering that used to live between two submissions now lives inside
//!    one, where the door owns it.
//!
//! The worst order is `.rev()` and not a coin toss on purpose. The defect was
//! never that the front reorders reliably — it is that the front has no order
//! at all, so a test that submits in the emitted order measures nothing and a
//! test that randomises is red only sometimes. Reversing is the deterministic
//! form of "the order was lost".

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, ManifestOutcome, MutationDoorOutcome,
    MutationOutcome, RespawnFn, SpawnedCellKind, WakeFn, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{JsonValue, Message, Path, Uuid};
use meclaw_testing::{ColonyHandle, emit_all, shipped_script};
use std::collections::BTreeSet;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

const RECIPES: &str = "templates/builder/recipes/config.json";

/// The organisation the examples are written for, and the member they grow.
const ORG: &str = "/os/orgs/acme";
const MEMBER: &str = "alex";

/// What a member always gets, named by an occupant of each device: a hive
/// leaves no registry row, so the display is named by its own socket and the
/// app by its own layout.
const DEVICES: [&str; 2] = [
    "/os/orgs/acme/members/alex/channels/display/web",
    "/os/orgs/acme/members/alex/apps/colony-view/layout",
];

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
        "examples/organism/grow-member.json",
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

/// The manifest emissions of one wish, in the order they left the cell. The
/// `bind` leg is not a manifest and is filtered out here rather than counted.
fn manifests(payload: Value, member_index: &str) -> Vec<Value> {
    emit_all(
        &shipped_script(repo(RECIPES).to_str().expect("utf-8 path")),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe", "member_index": member_index},
                       "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": payload.to_string()}],
        }),
    )
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

/// Every declaration of every manifest a wish rendered, flattened into the one
/// list the door would roll off. Written so it says the same thing before and
/// after the repair: what changes is how many EMISSIONS carry these entries,
/// not which entries there are or what order they stand in.
fn all_declarations(out: &[Value]) -> Vec<Value> {
    out.iter()
        .flat_map(|m| {
            m["manifest"]
                .as_array()
                .expect("a recipe emission carries its manifest as an array")
                .clone()
        })
        .collect()
}

// ──────────────────────────────────────────────────────────────────────────────
// against a real colony
// ──────────────────────────────────────────────────────────────────────────────

/// 1 — the door's own semantics, measured rather than assumed.
///
/// A manifest is an ORDERED list and the colony rolls it off entry by entry
/// through the very `handle_mutation` a single body takes, so an entry is
/// judged against the tree the entries in front of it just grew. If that were
/// not so, the repair below would be wrong and the fall-back would be a lane
/// that chains the devices on the member's `mutation_committed` receipt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_door_judges_a_later_entry_against_the_tree_an_earlier_one_grew() {
    if !shipped() || !library_is_complete() {
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    build_root(td.path());
    let h = boot(&td).await;
    grow_the_shell(&h).await;

    let decls = all_declarations(&manifests(member_wish(&member_template()), "0"));
    assert_eq!(
        decls.len(),
        3,
        "a member wish is the person, the screen and the app — three \
         declarations, however many emissions carry them: {decls:?}"
    );
    let outcome = knock(&h, json!({"manifest": decls})).await;
    assert_eq!(
        applied(&outcome),
        3,
        "ONE submission carrying the member and then its two devices did not \
         apply all three — the door does not judge a later entry against the \
         tree an earlier one grew, and the repair for GH #585 is the receipt \
         lane instead: {outcome:?}"
    );
    let grown = graph_nodes(&h, "/os/orgs/acme/members/alex").await;
    for want in DEVICES {
        assert!(
            grown.iter().any(|p| p == want),
            "{want} is not in the grown tree: {grown:?}"
        );
    }
    h.shutdown().await;
}

/// 2 — the wish, through the door, in the worst order the front could pick.
///
/// This is the defect of GH #585 stated as a test: the builder used to hand the
/// front two submissions in one turn, and a front has no ordering across two.
/// Submitting them reversed is what the colony measured on the first e-build,
/// made deterministic — and a member wish that leaves as ONE submission has no
/// order left to lose.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_wish_survives_the_worst_order_the_front_can_pick() {
    if !shipped() || !library_is_complete() {
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    build_root(td.path());
    let h = boot(&td).await;
    grow_the_shell(&h).await;

    let out = manifests(member_wish(&member_template()), "0");
    assert!(
        !out.is_empty(),
        "a member wish rendered no manifest at all: {out:?}"
    );
    // The front has no ordering, so the unfavourable order is the honest one.
    for one in out.iter().rev() {
        let outcome = knock(&h, json!({"manifest": one["manifest"]})).await;
        assert!(
            matches!(
                outcome,
                MutationDoorOutcome::Manifest(ManifestOutcome::Committed { .. })
            ),
            "a submission of the member wish was refused when the front handed \
             the submissions over in the other order — a member wish must be \
             ONE submission, so that the order between its declarations is the \
             door's to keep: {outcome:?}"
        );
    }

    let grown = graph_nodes(&h, "/os/orgs/acme/members/alex").await;
    for want in DEVICES {
        assert!(
            grown.iter().any(|p| p == want),
            "the member grew without {want} — this is the silent half of GH \
             #585: the wish reports success and the person has no device: \
             {grown:?}"
        );
    }

    // and the only trace the defect ever left is empty too
    let refused = rejected_rows(&h).await;
    assert!(
        refused.is_empty(),
        "the member wish left refusals in `mutation_log`: {refused:?}"
    );
    h.shutdown().await;
}

// ──────────────────────────────────────────────────────────────────────────────
// the colony these two tests grow into
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

/// The two storeys a member wish needs under it, applied the way the shipped
/// example applies them.
async fn grow_the_shell(h: &ColonyHandle) {
    for file in ["grow-os.json", "grow-org.json"] {
        let outcome = mutate(h, read_json(&repo("examples/organism").join(file))).await;
        assert!(
            matches!(outcome, MutationOutcome::Committed { .. }),
            "{file} must be committed before a member is grown into it: {outcome:?}"
        );
    }
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

/// One BODY at the door, form unknown to the caller — the door `--apply`,
/// `POST /colony/mutations` and the submitter all knock on.
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
/// forward and stops at the first refusal, so this number IS the verdict.
fn applied(o: &MutationDoorOutcome) -> usize {
    match o {
        MutationDoorOutcome::Manifest(ManifestOutcome::Committed { ids }) => ids.len(),
        MutationDoorOutcome::Manifest(ManifestOutcome::Rejected { ids, .. }) => ids.len(),
        _ => 0,
    }
}

/// The paths `/colony/graph` reports under a scope.
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

/// Every refused row `mutation_log` carries, as `<scope> <error_code>`. The
/// silent defect of GH #585 left exactly one, and it was the only trace.
async fn rejected_rows(h: &ColonyHandle) -> Vec<String> {
    let (ack_tx, ack_rx) = oneshot::channel::<meclaw_colony::api_dto::ReadMutationsAuditReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadMutationsAudit {
            since: None,
            limit: 1000,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .entries
        .into_iter()
        .filter(|e| e.status == "rejected")
        .map(|e| {
            format!(
                "{} {}",
                e.scope,
                e.error_code.unwrap_or_else(|| "?".to_string())
            )
        })
        .collect()
}
