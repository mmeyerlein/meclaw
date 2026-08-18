//! GH #189 — `./foo` and `foo` are the same path, and an `add_nodes[].name`
//! written the first way must be reachable as an edge endpoint in the same diff.
//!
//! `validate_edges_and_cycle` inserted every `add_nodes[].name` into the
//! post-state node set **as written**, while the endpoint lookup strips the
//! canonical `./` prefix before deciding which namespace to test in. So a diff
//! that spells the name with the prefix was rejected with
//! `edge_schema: to='./foo' unknown`, while the identical diff spelling it `foo`
//! committed. Two characters, one commit and one reject.
//!
//! The failure is also misleading. It says the endpoint is unknown, when the
//! node is right there in the same diff — the endpoint check was looking in a
//! namespace the insert had never written to.
//!
//! Every other reader on the mutation surface already canonicalises the prefix
//! away before deciding anything: `scoped_name` (#179) strips it, and the
//! endpoint side always has. This one insert was the last place where a spelling
//! survived long enough to become an identity. #166's fix added the
//! resolved-path insert beside it; this is the short-name twin of the same
//! normalisation.
//!
//! The tests assert the persisted graph, not the receipt: an endpoint check that
//! merely stopped rejecting would commit a dead edge and still pass an
//! outcome-only assertion.

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{ColonyMsg, MutationOutcome};
use meclaw_core::{Path, Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mocks::EchoMockCell;
use tokio::sync::oneshot;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

async fn send_mutation(
    h: &ColonyHandle,
    payload: meclaw_core::serde_json::Value,
) -> MutationOutcome {
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

async fn registry_entry(
    h: &ColonyHandle,
    path: &str,
) -> Option<meclaw_colony::api_dto::RegistryEntryDto> {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadRegistryReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 100,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .entries
        .into_iter()
        .find(|e| e.path == path)
}

/// Whether the persisted graph carries `from -> to` — read from `colony.db`,
/// which is what survives a restart and therefore the only proof the edge is
/// real rather than merely un-rejected.
fn edge_persisted(db_dir: &std::path::Path, from: &str, to: &str) -> bool {
    let conn = rusqlite::Connection::open_with_flags(
        db_dir.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM edges WHERE from_path = ?1 AND to_path = ?2",
            [from, to],
            |r| r.get(0),
        )
        .unwrap();
    n > 0
}

/// A hive `/talky` with a live cell inside and an anchor reaching it from
/// outside, plus a single-cell template to hang next to it. The anchor lane is
/// not decoration: without an edge crossing the hive's boundary the whole
/// subtree computes as inactive and a mutation would trip the stop-wiring guard
/// instead of exercising the endpoint check.
async fn colony_with_a_populated_hive() -> (tempfile::TempDir, ColonyHandle) {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "talky/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    let tpl = td.path().join("templates").join("web-fetch-tool");
    write(&tpl, "template.json", r#"{"name":"web-fetch-tool"}"#);
    write(
        &tpl,
        "config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/talky/split"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let factory: std::sync::Arc<dyn meclaw_colony::CellFactory> =
        std::sync::Arc::new(meclaw_testing::factories::EchoCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("echo".to_string(), factory)]);
    rescan_templates(&h, td.path().join("templates")).await;
    h.add_hive_scope(Path::new("/")).await;
    h.add_hive_scope(Path::new("/talky")).await;
    h.spawn(Path::new("/talky/split"), || {
        EchoMockCell::new(Path::new("/talky/split"))
    })
    .await;
    h.spawn(Path::new("/anchor"), || {
        EchoMockCell::new(Path::new("/anchor")).emitted_target(Path::new("/talky/split"))
    })
    .await;
    h.add_edge(
        Uuid::now_v7(),
        Path::new("/anchor"),
        Path::new("/talky/split"),
    )
    .await;
    (td, h)
}

/// The report's diff: a single-segment name written with the canonical `./`
/// prefix, wired in the same breath.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dot_slash_name_is_reachable_as_an_endpoint_in_the_same_diff() {
    let (td, h) = colony_with_a_populated_hive().await;

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "add_nodes": [{"name": "./fetch", "template": "web-fetch-tool"}],
                "add_edges": [{"from": "./anchor", "to": "./fetch"}]
            }
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "'./fetch' and 'fetch' are the same path — the diff that spells it the \
         first way must commit too; got {outcome:?}"
    );
    assert!(
        registry_entry(&h, "/fetch").await.is_some(),
        "the node lands at the path both spellings denote"
    );
    assert!(
        edge_persisted(td.path(), "/anchor", "/fetch"),
        "and the lane is in the persisted graph, not merely un-rejected"
    );

    h.shutdown().await;
}

/// The two spellings must reach the same node whichever side carries the prefix.
/// The name without it and the endpoint with it is the everyday mixture, and it
/// worked only by accident: the endpoint side has always stripped the prefix.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_prefix_may_sit_on_either_side_of_the_diff() {
    let (td, h) = colony_with_a_populated_hive().await;

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "add_nodes": [{"name": "./fetch", "template": "web-fetch-tool"}],
                "add_edges": [{"from": "anchor", "to": "fetch"}]
            }
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "a prefixed name and bare endpoints denote one node; got {outcome:?}"
    );
    assert!(edge_persisted(td.path(), "/anchor", "/fetch"));

    h.shutdown().await;
}

/// GH #166's case, kept green: the prefixed spelling of a MULTI-segment name
/// still reaches the absolute namespace a deep endpoint is looked up in. The
/// canonicalisation happens before the depth decision, so `./talky/fetch` is a
/// deep name and not a short one that happens to contain a slash.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_prefixed_deep_name_still_reaches_its_deep_endpoint() {
    let (td, h) = colony_with_a_populated_hive().await;

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "add_nodes": [{"name": "./talky/fetch", "template": "web-fetch-tool"}],
                "add_edges": [{"from": "./talky/split", "to": "./talky/fetch"}]
            }
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "a prefixed deep name must still be wireable in the same diff; got {outcome:?}"
    );
    assert!(edge_persisted(td.path(), "/talky/split", "/talky/fetch"));

    h.shutdown().await;
}

/// The check stays a check. An endpoint that names nothing — in pre-state or in
/// this diff, with or without the prefix — is still `edge_schema`. A fix that
/// simply waved prefixed endpoints through would pass the tests above and fail
/// this one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_endpoint_nobody_creates_is_still_rejected() {
    let (_td, h) = colony_with_a_populated_hive().await;

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "add_nodes": [{"name": "./fetch", "template": "web-fetch-tool"}],
                "add_edges": [{"from": "./anchor", "to": "./typo"}]
            }
        }),
    )
    .await;
    assert!(
        matches!(
            &outcome,
            MutationOutcome::Rejected { error_code, .. } if error_code == "edge_schema"
        ),
        "an endpoint nobody creates must still reject as edge_schema; got {outcome:?}"
    );

    h.shutdown().await;
}
