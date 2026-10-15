//! GH #1102 -- a channel's credential road is APPLIED, not only rendered.
//!
//! `gh1061_a_recipe_grown_voice_channel_is_granted` pins what the recipe
//! renders for a channel that spends a key: the grant rows, the ask from the
//! cell to the member's broker and one answer per handle. It reads a string.
//! This file hands the same rendered declaration to a real mutation door on a
//! colony grown from the shipped templates, and the door has to say
//! `Committed`.
//!
//! WHY IT COULD NOT COMMIT BEFORE. The recipe drew both edges with `lane`
//! (`credential_request`, `in_sealed`), which makes them v-lanes (GH #559). The
//! channel cell stands at `<member>/channels/<name>`, one level below the
//! declaration's scope, and its target hive `channels` declares no contract on
//! purpose (`templates/member/channels/config.json`: open, unsealed, no
//! `params.contract`). Rule row 5 of `port_boundary::v_lane_verdict` then
//! refuses every such edge with `v_lane_no_connect_point`, and the whole wish
//! with it. The form the colony takes for an endpoint under an open,
//! contract-free hive is the plain deep edge without `lane`; the guards that
//! address the ask and the answer are unchanged.
//!
//! The harness (inert factory, root copy, boot) is the device of
//! `gh567_the_credentialled_wish_is_one_act.rs`, copied deliberately so that
//! file does not move because this one exists. A tree without the example or
//! the library is SKIPPED, never judged (GH #49).

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

/// The member `examples/organism/grow-member.json` creates.
const MEMBER: &str = "/os/orgs/acme/members/alex";

const PREFIX: [&str; 3] = [
    "examples/organism/grow-os.json",
    "examples/organism/grow-org.json",
    "examples/organism/grow-member.json",
];

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> bool {
    repo("examples/organism/grow-os.json").is_file()
        && [
            "meclaw-os",
            "org",
            "member",
            "access",
            "telegram-connector",
            "voice",
        ]
        .iter()
        .all(|n| repo(&format!("templates/{n}/template.json")).is_file())
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn version_of(template: &str) -> String {
    let v = read_json(&repo(&format!("templates/{template}/template.json")));
    v["version"]
        .as_str()
        .unwrap_or_else(|| panic!("templates/{template}/template.json declares no version"))
        .to_string()
}

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

/// The example's seed, the real library, and an `.env` of placeholders only.
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
         MODEL_DIALECTIC=gpt-4o-mock\nMODEL_FILE_SPACE=gpt-4o-mock\n\
         MODEL_DREAMER=gpt-4o-mock\n\
         TELEGRAM_BOT_TOKEN=test-token\n\
         TELEGRAM_BOT_TOKEN_2=test-token-2\n\
         TELEGRAM_ALLOWED_USER_ID=0\n\
         EXAMPLE_CHAT_TOKEN=test-chat-token\n",
    )
    .unwrap();
}

async fn boot(td: &tempfile::TempDir) -> ColonyHandle {
    let types = cell_types_in(&td.path().join("templates"));
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
        .expect("the rescan must not have aborted");
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

/// A channel wish with a credential, through the script the builder SHIPS.
fn rendered_channel(name: &str, template: &str, extra: Value) -> Vec<Value> {
    let mut params = json!({
        "scope": MEMBER, "level": "channel", "name": name,
        "template": format!("{template}@{}", version_of(template)),
        "assistant": "scribe", "ctx": {"member_person": "alex"}});
    for (k, v) in extra.as_object().expect("extra params") {
        params[k] = v.clone();
    }
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

const EXPIRES: &str = "2099-01-01T00:00:00.000000Z";

/// Both channel templates that spend a key: one flat handle (Telegram) and
/// two blocks with shipped handles (voice).
fn wishes() -> Vec<(&'static str, Vec<Value>)> {
    vec![
        (
            "telegram",
            rendered_channel(
                "telegram",
                "telegram-connector",
                json!({"bind_chat": "12345",
                       "credential": {"cred_ref": "cred:example-bot", "subject": "member:alex",
                                      "expires_at": EXPIRES}}),
            ),
        ),
        (
            "voice",
            rendered_channel(
                "voice",
                "voice",
                json!({"credential": {"stt_cred_ref": "cred:example-stt",
                                      "tts_cred_ref": "cred:example-tts",
                                      "subject": "member:alex", "expires_at": EXPIRES}}),
            ),
        ),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1102_a_channel_wish_with_a_credential_commits_with_its_road() {
    if !shipped() {
        return; // GH #49
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

    let broker = format!("{MEMBER}/access");
    for (name, decls) in wishes() {
        assert!(!decls.is_empty(), "{name}: the recipe rendered nothing");
        for d in &decls {
            let outcome = mutate(&h, d.clone()).await;
            assert!(
                matches!(outcome, MutationOutcome::Committed { .. }),
                "{name}: the channel and its credential road were refused -- GH #1102: \
                 a v-lane onto a cell under the contract-free `channels` hive earns \
                 v_lane_no_connect_point: {outcome:?}"
            );
        }

        // The positive receipt: the colony's own graph, not the diff it was sent.
        let (ack_tx, ack_rx) = oneshot::channel::<meclaw_colony::api_dto::ReadGraphReply>();
        h.inbox_tx
            .send(ColonyMsg::ReadGraph {
                scope: Path::new("/"),
                ack: ack_tx,
            })
            .await
            .unwrap();
        let edges = ack_rx.await.unwrap().edges;
        let cell = format!("{MEMBER}/channels/{name}");
        let cond =
            |e: &&meclaw_colony::api_dto::GraphEdgeDto| e.condition.clone().unwrap_or_default();
        let asks = edges
            .iter()
            .filter(|e| e.from == cell && e.to == broker)
            .filter(|e| cond(e).contains("hop.route == 'credential_request'"))
            .count();
        assert_eq!(asks, 1, "{name}: one ask from the cell to its broker");
        let answers = edges
            .iter()
            .filter(|e| e.from == broker && e.to == cell)
            .filter(|e| cond(e).contains("hop.grant_id == 'grant:"))
            .count();
        let want = if name == "voice" { 2 } else { 1 };
        assert_eq!(
            answers, want,
            "{name}: one answer per handle the cell reads"
        );
    }

    h.shutdown().await;
}
