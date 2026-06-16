//! Pre-14 Pass-2 rejection-test B4 — Befund-22 scope-containment boundary.
//!
//! A mutation carries a `scope` (the guard scope; default `/`). Every top-level
//! diff name is resolved relative to that scope and MUST stay within it. Before
//! Pass-1, `MutationError::ScopeOutOfBounds` was declared-but-never-constructed —
//! the boundary was not actually enforced. Pass-1 wired the guard
//! (`validate_scope_containment`, colony.rs § Befund 22). This test is the FIRST
//! verification that the boundary really confines:
//!
//!   * NEGATIVE: a mutation scoped to `/main` whose edge addresses `../escape`
//!     (resolving to `/escape`, OUTSIDE the `/main` prefix) → `Rejected` with
//!     `error_code = "scope_out_of_bounds"`. `add_edges` is used so the
//!     containment guard (which runs BEFORE endpoint/template validation) is the
//!     check that fires — no template/subtree resolution sits in front of it.
//!   * POSITIVE control: the SAME-shaped, in-scope edge (`a -> b`, both under
//!     `/main`) commits — proving the guard rejects on the boundary, not on the
//!     edge shape.

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::Uuid;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use std::sync::Arc;
use tokio::sync::oneshot;

fn echo_factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "echo".to_string(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    )]
}

fn echo_registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert(
        "echo".into(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    );
    r
}

/// Root hive (`td/main` → `/`) containing a NESTED hive `/sub` with two echo
/// cells `/sub/a` and `/sub/b` (the in-scope edge endpoints for the positive
/// control). `/sub` is the guard scope under test; `/escape` lives OUTSIDE it.
fn write_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main/sub/a")).unwrap();
    std::fs::create_dir_all(td.join("main/sub/b")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/sub/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/sub/a/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/sub/a"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/sub/b/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/sub/b"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

async fn send_mutation(h: &ColonyHandle, payload: Value) -> MutationOutcome {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scoped_mutation_addressing_outside_prefix_rejects_scope_out_of_bounds() {
    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path());

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap");

    // ── NEGATIVE: scope "/sub", edge `.` -> `../escape` (resolves to /escape,
    //    outside the /sub prefix) → must REJECT with scope_out_of_bounds. ──
    let outcome = send_mutation(
        &h,
        json!({"scope":"/sub","diff":{"add_edges":[{"from":".","to":"../escape"}]}}),
    )
    .await;
    match outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(
                error_code, "scope_out_of_bounds",
                "out-of-scope edge target must reject with scope_out_of_bounds, got {error_code}"
            );
        }
        other => panic!("expected Rejected{{scope_out_of_bounds}}, got {other:?}"),
    }

    // ── POSITIVE control: SAME shape, in-scope endpoints (`a` -> `b`, both under
    //    /sub) → must COMMIT. Proves the guard rejects on the BOUNDARY, not on
    //    the edge shape. ──
    let outcome = send_mutation(
        &h,
        json!({"scope":"/sub","diff":{"add_edges":[{"from":"a","to":"b"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "in-scope edge a->b must commit, got {outcome:?}"
    );

    h.shutdown().await;
}
