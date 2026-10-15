//! GH #937 (review A) — the tap flag disagrees like the lane does.
//!
//! A tap is a PASSIVE edge (`Edge::tap`): it fires beside the sender's other
//! edges and never suppresses the sender's defaults. Like `lane` it is NOT an
//! identity term — two edges equal on the five routing terms would otherwise
//! both stand and deliver twice. That leaves the GH #564 trap: a regular
//! `add_edges` entry equal to a standing tap (or to a tap in the same diff)
//! was found "already there" by the dedup and silently dropped. The caller got
//! `Committed`, the graph kept only the tap, and the sender's default kept
//! firing — onto exactly the target the declared regular edge should have
//! taken the traffic from.
//!
//! The answer is the lane's answer: a pre-destructive `edge_schema` refusal on
//! both faces (standing edge, and two entries of one diff), with
//! `remove_edges` in the same diff as the way out. The persistence case pins
//! that `tap = true` survives the disk.

use meclaw_colony::api_dto::ReadGraphReply;
use meclaw_colony::{CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem};
use meclaw_core::{JsonValue, Path, Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use tokio::sync::oneshot;

const ECHO: &str = r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/dev/null"},
    "contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#;

fn write(root: &std::path::Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// `/` root hive with three echo cells: `sender`, `watched` (the tap target)
/// and `fallback` (the sender's default target).
async fn boot(td: &tempfile::TempDir) -> ColonyHandle {
    let root = td.path();
    write(root, "main/config.json", r#"{"cell":{"type":"hive"}}"#);
    write(root, "main/sender/config.json", ECHO);
    write(root, "main/watched/config.json", ECHO);
    write(root, "main/fallback/config.json", ECHO);
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

/// `(to, tap, is_default)` of every out-edge of `/sender`, sorted.
async fn sender_edges(h: &ColonyHandle) -> Vec<(String, bool, bool)> {
    let mut v: Vec<_> = read_graph(h)
        .await
        .edges
        .into_iter()
        .filter(|e| e.from == "/sender")
        .map(|e| (e.to, e.tap, e.is_default))
        .collect();
    v.sort();
    v
}

fn refusal_details(outcome: &MutationOutcome) -> &str {
    match outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(error_code, "edge_schema", "{outcome:?}");
            details
        }
        MutationOutcome::Committed { id, .. } => {
            panic!("expected an edge_schema refusal, the mutation committed as {id}")
        }
    }
}

/// Face "standing edge": a tap stands, a regular edge equal on the five terms
/// arrives. Before the fix it committed and was dropped — the default onto
/// `/fallback` kept firing. Now it is refused by name, the pre-state is
/// untouched, and the documented way out (remove the tap in the same diff)
/// lays the regular edge.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_regular_edge_on_a_standing_tap_is_refused_not_swallowed() {
    let td = tempfile::TempDir::new().unwrap();
    let h = boot(&td).await;

    let pre = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[
            {"from":"./sender","to":"./fallback","default":true},
            {"from":"./sender","to":"./watched","tap":true}
        ]}}),
    )
    .await;
    assert!(
        matches!(pre, MutationOutcome::Committed { .. }),
        "default + tap are the pre-state: {pre:?}"
    );

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[
            {"from":"./sender","to":"./watched"}
        ]}}),
    )
    .await;
    let details = refusal_details(&outcome);
    assert!(
        details.contains("tap = true") && details.contains("remove it in the same diff"),
        "the refusal names the standing tap and the way out: {details}"
    );
    assert_eq!(
        sender_edges(&h).await,
        vec![
            ("/fallback".to_string(), false, true),
            ("/watched".to_string(), true, false)
        ],
        "pre-destructive: default and tap stand as before"
    );

    let way_out = send_mutation(
        &h,
        json!({"scope":"/","diff":{
            "remove_edges":[{"match":{"from":"./sender","to":"./watched"}}],
            "add_edges":[{"from":"./sender","to":"./watched"}]
        }}),
    )
    .await;
    assert!(
        matches!(way_out, MutationOutcome::Committed { .. }),
        "the documented migration commits: {way_out:?}"
    );
    assert_eq!(
        sender_edges(&h).await,
        vec![
            ("/fallback".to_string(), false, true),
            ("/watched".to_string(), false, false)
        ],
        "and the REGULAR edge stands — the one that suppresses the default"
    );

    h.shutdown().await;
}

/// Face 1 (inside one diff): a tap and a regular edge equal on the five terms.
/// The apply arm dedups against the growing table, so the second entry was
/// dropped without a word. Now the whole diff is refused, naming both entries.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tap_and_a_regular_edge_inside_one_diff_are_refused() {
    let td = tempfile::TempDir::new().unwrap();
    let h = boot(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[
            {"from":"./sender","to":"./watched","tap":true},
            {"from":"./sender","to":"./watched"}
        ]}}),
    )
    .await;
    let details = refusal_details(&outcome);
    assert!(
        details.contains("add_edges[0]")
            && details.contains("add_edges[1]")
            && details.contains("tap = true")
            && details.contains("tap = false"),
        "the refusal names both entries and both tap flags: {details}"
    );
    assert!(
        sender_edges(&h).await.is_empty(),
        "nothing was applied — the check is pre-destructive"
    );

    h.shutdown().await;
}

/// Two entries that AGREE on the tap flag stay idempotent: one edge, committed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_tap_twice_inside_one_diff_stays_idempotent() {
    let td = tempfile::TempDir::new().unwrap();
    let h = boot(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[
            {"from":"./sender","to":"./watched","tap":true},
            {"from":"./sender","to":"./watched","tap":true}
        ]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "{outcome:?}"
    );
    assert_eq!(
        sender_edges(&h).await,
        vec![("/watched".to_string(), true, false)]
    );

    h.shutdown().await;
}

/// Review Minor 3: `tap = true` survives the disk — write, fresh connection,
/// read. The regular row beside it is the discriminator: a rehydration that
/// handed every edge the same flag would pass a one-row test.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_persisted_tap_survives_the_rehydration() {
    use meclaw_colony::persist::colony_db::ColonyDb;
    use meclaw_colony::persist::writer::ColonyWriteOp;

    let td = tempfile::TempDir::new().unwrap();
    let db_path = td.path().join("colony.db");

    let tapped = Uuid::now_v7();
    let regular = Uuid::now_v7();
    {
        let db = ColonyDb::open(&db_path).expect("open colony.db");
        for (id, to, tap, at) in [
            (tapped, "/watched", true, 1_700_000_000),
            (regular, "/fallback", false, 1_700_000_001),
        ] {
            db.send_op(ColonyWriteOp::InsertEdge {
                id: id.to_string(),
                from: "/sender".to_string(),
                to: to.to_string(),
                created_at: at,
                condition: None,
                modifier: None,
                is_default: false,
                lane: None,
                tap,
            })
            .await;
        }
        db.shutdown_async().await;
    }

    let reopened = ColonyDb::open(&db_path).expect("re-open colony.db");
    let edges = reopened.read_edges().expect("the edge table rehydrates");
    let back = |id: Uuid| {
        edges
            .iter()
            .find(|e| e.id == id)
            .unwrap_or_else(|| panic!("row {id} comes back"))
            .tap
    };
    assert!(
        back(tapped),
        "tap = true survives the disk — else a reboot turns an observer into a regular edge that suppresses the default"
    );
    assert!(!back(regular), "a regular edge stays regular");

    reopened.shutdown_async().await;
}
