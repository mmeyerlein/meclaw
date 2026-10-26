//! GH #1327 -- an app under the member's open `./apps` reaches the connect
//! point a sealed figure declares below its rim.
//!
//! The figure (`templates/figure`, sealed with `ports: []`) declares the lane
//! `in_note` with `at: ["./simple-agent/curator"]`, and its `simple-agent`
//! declares the same lane at `./curator`. An app stands under `<member>/apps`,
//! which ships open and contract-free on purpose
//! (`templates/member/apps/config.json`, `not_in_scope`). The v-lane from the
//! app onto the curator used to be refused `v_lane_no_connect_point` naming
//! `/m/apps`: row 5 of the rule table ("the target hive owes the connect
//! point") was read for BOTH ends, so the SENDING end's own container was
//! asked for a connect point it never promised. A connect point is a promise
//! the contract makes for what is DELIVERED into it; the caller owes none.
//!
//! The tree is minimal, but the declarations are the shipped ones: the
//! `ports` and the `in_note` lane are read from the figure and simple-agent
//! templates, and `apps`/`channels` are the shipped `config.json` files as
//! they are. The verdict is read off the colony's own edge table.

use meclaw_colony::api_dto::ReadGraphReply;
use meclaw_colony::{CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem};
use meclaw_core::{JsonValue, Path, Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use tokio::sync::oneshot;

const ECHO: &str = r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/dev/null"},
    "contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#;

const APP: &str = "/m/apps/care";
const CURATOR: &str = "/m/channels/fig/simple-agent/curator";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn write(root: &std::path::Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn read_json(rel: &str) -> JsonValue {
    let raw = std::fs::read_to_string(repo(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// A hive `config.json` carrying the shipped template's seal and its shipped
/// `in_note` lane -- nothing else of the template, so the echo tree boots.
fn sealed_hive_with_in_note(template: &str) -> String {
    let tpl = read_json(&format!("templates/{template}/config.json"));
    let ports = tpl["params"]["ports"].clone();
    assert!(
        ports.is_array(),
        "{template} ships sealed (`params.ports`): {ports}"
    );
    let lane = tpl["params"]["contract"]["accepts"]
        .as_array()
        .and_then(|ls| ls.iter().find(|l| l["route"] == json!("in_note")))
        .cloned()
        .unwrap_or_else(|| panic!("{template} declares `in_note` in its accepts"));
    json!({
        "cell": {"type": "hive"},
        "params": {"ports": ports, "contract": {"accepts": [lane]}}
    })
    .to_string()
}

/// ```text
/// /                         root hive
/// └── m                     hive -- the member level (the edge's scope)
///     ├── apps              shipped: open, no ports, no contract
///     │   └── care          echo cell -- the app, the SENDING end
///     └── channels          shipped: open, no ports, no contract
///         └── fig           sealed, `in_note` at ./simple-agent/curator
///             └── simple-agent  sealed, `in_note` at ./curator
///                 └── curator   echo cell -- the connect point
/// ```
fn plant(root: &std::path::Path) {
    write(root, "main/config.json", r#"{"cell":{"type":"hive"}}"#);
    write(root, "main/m/config.json", r#"{"cell":{"type":"hive"}}"#);
    write(
        root,
        "main/m/apps/config.json",
        &read_json("templates/member/apps/config.json").to_string(),
    );
    write(root, "main/m/apps/care/config.json", ECHO);
    write(
        root,
        "main/m/channels/config.json",
        &read_json("templates/member/channels/config.json").to_string(),
    );
    write(
        root,
        "main/m/channels/fig/config.json",
        &sealed_hive_with_in_note("figure"),
    );
    write(
        root,
        "main/m/channels/fig/simple-agent/config.json",
        &sealed_hive_with_in_note("simple-agent"),
    );
    write(
        root,
        "main/m/channels/fig/simple-agent/curator/config.json",
        ECHO,
    );
}

async fn boot(root: &std::path::Path) -> ColonyHandle {
    let h = ColonyHandle::new_with_echo_at(root);
    let mut factories = CellFactoryRegistry::new();
    factories.insert(
        "echo".to_string(),
        std::sync::Arc::new(EchoCellFactory) as std::sync::Arc<dyn meclaw_colony::CellFactory>,
    );
    bootstrap_from_filesystem(root, &factories, &h.runtime())
        .await
        .expect("the tree boots");
    h
}

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

async fn read_graph(h: &ColonyHandle) -> ReadGraphReply {
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

fn edge_from_the_app(lane: Option<&str>) -> JsonValue {
    let mut e = json!({"from": "./apps/care", "to": "./channels/fig/simple-agent/curator"});
    if let Some(lane) = lane {
        e["lane"] = json!(lane);
    }
    json!({"scope": "/m", "diff": {"add_edges": [e]}})
}

async fn the_edge_stands(h: &ColonyHandle) -> bool {
    read_graph(h)
        .await
        .edges
        .iter()
        .any(|e| e.from == APP && e.to == CURATOR)
}

/// The P1 constellation: the v-lane commits, and the edge stands in the edge
/// table afterwards -- the positive receipt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_draws_the_in_note_lane_onto_a_sealed_figures_curator() {
    let td = tempfile::TempDir::new().unwrap();
    plant(td.path());
    let h = boot(td.path()).await;

    let outcome = send_mutation(&h, edge_from_the_app(Some("in_note"))).await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the figure promised this connect point and the app's open container owes none: \
         {outcome:?}"
    );
    assert!(
        the_edge_stands(&h).await,
        "the committed v-lane is in the edge table"
    );

    h.shutdown().await;
}

/// The control: the same edge without the lane is a plain deep edge past the
/// figure's seal, and the seal still refuses it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_edge_without_the_lane_still_meets_the_seal() {
    let td = tempfile::TempDir::new().unwrap();
    plant(td.path());
    let h = boot(td.path()).await;

    let outcome = send_mutation(&h, edge_from_the_app(None)).await;
    match &outcome {
        MutationOutcome::Rejected { error_code, .. } => assert_eq!(
            error_code, "hive_port_boundary",
            "without the lane the figure's seal is untouched: {outcome:?}"
        ),
        MutationOutcome::Committed { .. } => {
            panic!("a deep edge past a sealed rim must not commit: {outcome:?}")
        }
    }
    assert!(!the_edge_stands(&h).await, "and no edge is left behind");

    h.shutdown().await;
}
