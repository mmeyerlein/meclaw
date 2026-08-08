//! Substrat-Fix Befund 6 — `add_edges` onto nodes added in the SAME diff,
//! using the canonical `./name` mutation endpoint form (overview § Variablen-
//! Substitution example: `{ "from": "./dispatcher", "to": "./session_..." }`).
//!
//! Before the fix the `./`-prefixed endpoint rejected as `edge_schema`
//! ("from='./a' unknown") because validate compared the raw string against the
//! bare `add_nodes` short-names, and `resolve_scoped_path` (apply) string-joined
//! `/./a` — a path the routing layer never matches. Spec § Mutation format:
//! "an `add_edges` edge may point to a node that newly arrives in the same diff
//! via `add_nodes`."
//!
//! This drives the real mutation path and proves BOTH halves:
//!   * the composite add_nodes + add_edges diff COMMITS, and
//!   * the persisted edge endpoints are NORMALISED (`/a` → `/b`, not `/./a`),
//!     read back via `ColonyMsg::ReadGraph`.

use meclaw_colony::api_dto::ReadGraphReply;
use meclaw_colony::{ColonyMsg, MutationOutcome};
use meclaw_core::{JsonValue, Path, Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use tokio::sync::oneshot;

async fn send_mutation(h: &ColonyHandle, payload: JsonValue) -> MutationOutcome {
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
        .unwrap();
    ack_rx.await.unwrap()
}

async fn rescan_templates(h: &ColonyHandle, templates_root: std::path::PathBuf) {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap();
}

async fn read_graph_root(h: &ColonyHandle) -> ReadGraphReply {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: Path::new("/"),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap()
}

fn write_echo_template(ws: &std::path::Path) {
    let tpl = ws.join("templates/echo-tpl");
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), r#"{"name":"echo-tpl"}"#).unwrap();
    std::fs::write(
        tpl.join("config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/sink"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_edges_dot_slash_onto_same_diff_nodes_commits_with_normalised_edge() {
    let td = tempfile::TempDir::new().unwrap();
    // Root hive `main` → logical `/`; cells land at `/a`, `/b`.
    std::fs::create_dir_all(td.path().join("main")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    write_echo_template(td.path());

    let h = ColonyHandle::new_with_echo_at(td.path());
    rescan_templates(&h, td.path().join("templates")).await;

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "add_nodes": [
                    {"name": "a", "template": "echo-tpl"},
                    {"name": "b", "template": "echo-tpl"}
                ],
                "add_edges": [{"from": "./a", "to": "./b"}]
            }
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "composite add_nodes + ./-add_edges must commit (Befund 6); got {outcome:?}"
    );

    let graph = read_graph_root(&h).await;
    let edge = graph
        .edges
        .iter()
        .find(|e| e.from.ends_with("/a") && e.to.ends_with("/b"))
        .unwrap_or_else(|| panic!("edge a->b must exist; graph edges: {:?}", graph.edges));
    // Endpoints normalised — NO `./` segment leaked into the stored path.
    assert!(
        !edge.from.contains("/./") && !edge.to.contains("/./"),
        "edge endpoints must be normalised (no `/./`): from={}, to={}",
        edge.from,
        edge.to
    );
    assert_eq!(edge.from, "/a", "from normalised to /a");
    assert_eq!(edge.to, "/b", "to normalised to /b");

    h.shutdown().await;
}
