//! Paket-5 T5 — P10 demo/integration tests, driven through the FULL mutation
//! path (`handle_mutation` via `ColonyMsg::Mutation`), not unit-level `validate`
//! calls. Test-only; no production code is touched.
//!
//! Two demos prove the P10 fixes end-to-end:
//!
//! (d) `malformed_match_per_arm_rejects_not_skips` — for each match-bearing arm,
//!     a diff entry with a MISSING mandatory match field causes the WHOLE
//!     mutation to be REJECTED (`error_code:"schema"`) — no silent skip, no
//!     partial commit. The KEY regression vs. the old behavior: a malformed
//!     `remove_edges` `match` used to be silently skipped at apply-time; now it
//!     rejects at validate-time, BEFORE any destructive effect. We prove no
//!     partial effect by asserting a legitimately-referenced edge in the SAME
//!     diff is NOT removed.
//!
//! (e) `same_name_in_foreign_scope_not_matched_by_validator` — a node/hive with
//!     the SAME short-name exists in a FOREIGN scope. A `swap_nodes` /
//!     `remove_nodes` `match.name` issued under a scope that does NOT contain a
//!     matching node must NOT match the foreign-scope node: the validator
//!     rejects with `match_no_hit`. Before the P10b scope-filter the colony-
//!     global hive-name set let it pass the validator (false-positive), then
//!     apply hit the wrong/no node.
//!
//! How a rejected mutation is observed E2E: `handle_mutation` returns a
//! `MutationOutcome` on the `ColonyMsg::Mutation.ack` oneshot (see
//! `phase_13_5_a5_demo.rs::send_mutation`). A rejected mutation yields
//! `MutationOutcome::Rejected { error_code, .. }` — we assert on `error_code`.
//! Pre-state intactness is checked via fresh-rusqlite probes on the persisted
//! `edges` / registry (the same probe style as `phase_13_5_a5_demo.rs`), taken
//! after a `shutdown()` BARRIER so the fire-and-forget writer has flushed.

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::Uuid;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use std::sync::Arc;
use tokio::sync::oneshot;

// ──────────────────────────────────────────────────────────────────────────────
// Shared helpers (mirrors the phase_13_5_a5_demo harness)
// ──────────────────────────────────────────────────────────────────────────────

/// `(name, factory)` list with `echo` registered.
fn echo_factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "echo".to_string(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    )]
}

/// Same list as a `CellFactoryRegistry` for `bootstrap_from_filesystem`.
fn echo_registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert(
        "echo".into(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    );
    r
}

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// Send a mutation and await its `MutationOutcome` (the reject-observation API).
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

/// Assert an outcome is a reject carrying the expected `error_code`.
fn assert_rejected(outcome: &MutationOutcome, expected_code: &str, label: &str) {
    match outcome {
        MutationOutcome::Rejected { error_code, .. } => assert_eq!(
            error_code, expected_code,
            "{label}: expected reject error_code={expected_code:?}, got {error_code:?}"
        ),
        MutationOutcome::Committed { .. } => {
            panic!("{label}: expected REJECT ({expected_code}), but mutation COMMITTED")
        }
    }
}

async fn all_registry(h: &ColonyHandle) -> Vec<meclaw_colony::api_dto::RegistryEntryDto> {
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
    ack_rx.await.unwrap().entries
}

async fn registry_entry(
    h: &ColonyHandle,
    path: &str,
) -> Option<meclaw_colony::api_dto::RegistryEntryDto> {
    all_registry(h).await.into_iter().find(|e| e.path == path)
}

/// Fresh-rusqlite probe: does an edge `from -> to` exist in the persisted
/// `edges` table? (Style copied from `phase_13_5_a5_demo.rs::edge_persisted`.)
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

// ──────────────────────────────────────────────────────────────────────────────
// Demo (d): malformed match per arm → reject (not silent skip), no partial effect.
// ──────────────────────────────────────────────────────────────────────────────

/// For every match-bearing arm, a diff entry with a MISSING mandatory match
/// field rejects the WHOLE mutation with `error_code:"schema"` — no silent skip,
/// no partial commit. The KEY regression: malformed `remove_edges` used to be
/// silently skipped; now it rejects PRE-destructively, so a legitimate edge that
/// the SAME diff references is left intact (proven via a fresh-rusqlite probe).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_match_per_arm_rejects_not_skips() {
    let td = tempfile::TempDir::new().unwrap();
    // Root hive + two echo cells /a and /b under /main → logical /a, /b.
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/a/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/a"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        td.path(),
        "main/b/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/b"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap must succeed");

    // Establish a legitimate pre-state edge /a -> /b that later malformed
    // remove_edges diffs ALSO reference legitimately. It must survive every
    // reject (proof of "no partial effect").
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"a","to":"b"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "seed add_edges a->b must commit; got {outcome:?}"
    );

    // ── remove_edges: match missing `from` (but a separate legit /a -> /b also
    //    in the diff). The WHOLE mutation must reject (schema), and /a -> /b must
    //    NOT be removed. ──
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"remove_edges":[
            {"match":{"to":"b"}},                 // malformed: `from` missing
            {"match":{"from":"a","to":"b"}}       // legit reference in same diff
        ]}}),
    )
    .await;
    assert_rejected(&outcome, "schema", "remove_edges match missing `from`");

    // ── remove_edges: match missing `to` (separate case). ──
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"remove_edges":[
            {"match":{"from":"a"}},               // malformed: `to` missing
            {"match":{"from":"a","to":"b"}}       // legit reference in same diff
        ]}}),
    )
    .await;
    assert_rejected(&outcome, "schema", "remove_edges match missing `to`");

    // ── remove_nodes: match missing `name`. ──
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"remove_nodes":[{"match":{}}]}}),
    )
    .await;
    assert_rejected(&outcome, "schema", "remove_nodes match missing `name`");

    // ── swap_nodes: match missing `name`. ──
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"swap_nodes":[
            {"match":{},"with":{"name":"b"}}      // match.name missing
        ]}}),
    )
    .await;
    assert_rejected(&outcome, "schema", "swap_nodes match missing `name`");

    // Pre-state intact: /a and /b still registered, no partial structural change.
    assert!(
        registry_entry(&h, "/a").await.is_some(),
        "/a must remain registered after all rejects"
    );
    assert!(
        registry_entry(&h, "/b").await.is_some(),
        "/b must remain registered after all rejects"
    );

    // BARRIER: flush the fire-and-forget writer before the edge probe.
    h.shutdown().await;

    // The legit /a -> /b edge that the malformed remove_edges diffs ALSO
    // referenced was NOT removed — the malformed entry rejected the WHOLE
    // mutation before any destructive effect (the P10a fix vs. the old silent
    // skip-and-partial-apply).
    assert!(
        edge_persisted(td.path(), "/a", "/b"),
        "edge /a -> /b must survive every malformed-match reject (no partial effect)"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// Demo (e): same short-name in a FOREIGN scope is NOT matched by the validator.
// ──────────────────────────────────────────────────────────────────────────────

/// Two same-named nodes live in two different scopes:
///   - a hive `shared` under scope `/x` (FS path `/x/shared`) — the `swap_nodes`
///     `match.name` case (only `swap_nodes` accepts a hive as a match source, via
///     the scope-filtered `hive_match_names`; this is the direct P10b fix);
///   - a cell `dup` under scope `/x` (FS path `/x/dup`) — the `remove_nodes`
///     `match.name` case (`remove_nodes`/`add_nodes` match against the scope-
///     filtered `registry_names`).
/// A `match.name` issued under scope `/y` — which contains NEITHER — must reject
/// with `match_no_hit`: the foreign-scope `/x` node must NOT satisfy the short-
/// name match. Before the scope filter the global name set let it PASS the
/// validator (false-positive), then apply hit the wrong/no node. A positive
/// control under the OWNING scope `/x` proves the reject is the scope filter at
/// work, not a blanket rejection of the name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn same_name_in_foreign_scope_not_matched_by_validator() {
    let td = tempfile::TempDir::new().unwrap();
    // Root hive.
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    // Scope /x: a hive named `shared` (the swap_nodes match source), a cell `dup`
    // (the remove_nodes match source), and a cell `target` (a valid swap `with`).
    write(
        td.path(),
        "main/x/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/x/shared/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/x/dup/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/x/dup"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        td.path(),
        "main/x/target/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/x/target"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    // Scope /y: a sibling hive containing NEITHER `shared` nor `dup`, just an
    // unrelated cell `other` so /y is a real, populated scope.
    write(
        td.path(),
        "main/y/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/y/other/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/y/other"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap must succeed");

    // ── swap_nodes scope=/y, match.name="shared": the only `shared` node is the
    //    hive in the FOREIGN scope /x. The scope-filtered `hive_match_names` must
    //    NOT see it → match_no_hit (NOT a false-positive pass; this is the direct
    //    P10b fix). ──
    let outcome = send_mutation(
        &h,
        json!({"scope":"/y","diff":{"swap_nodes":[
            {"match":{"name":"shared"},"with":{"name":"other"}}
        ]}}),
    )
    .await;
    assert_rejected(
        &outcome,
        "match_no_hit",
        "swap_nodes scope=/y match.name=shared (foreign hive in /x)",
    );

    // ── remove_nodes scope=/y, match.name="dup": the only `dup` node is the cell
    //    in the FOREIGN scope /x. The scope-filtered `registry_names` must NOT see
    //    it → match_no_hit (A2 scope-binding companion). ──
    let outcome = send_mutation(
        &h,
        json!({"scope":"/y","diff":{"remove_nodes":[
            {"match":{"name":"dup"}}
        ]}}),
    )
    .await;
    assert_rejected(
        &outcome,
        "match_no_hit",
        "remove_nodes scope=/y match.name=dup (foreign cell in /x)",
    );

    // ── Positive control: the SAME match.name="shared" UNDER its OWNING scope /x
    //    DOES resolve (the hive is in-scope there), proving the rejects above are
    //    the scope filter at work, not a blanket name rejection. swap_nodes
    //    scope=/x, match.name="shared" → with.name="target" (an in-scope cell)
    //    commits (swings the hive's external edges — here none — onto target). ──
    let outcome = send_mutation(
        &h,
        json!({"scope":"/x","diff":{"swap_nodes":[
            {"match":{"name":"shared"},"with":{"name":"target"}}
        ]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "swap_nodes scope=/x match.name=shared with target must COMMIT (in-scope); got {outcome:?}"
    );

    h.shutdown().await;
}
