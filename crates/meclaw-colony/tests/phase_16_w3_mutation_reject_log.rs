//! Phase-16 W3 (Ruling A6): Validate-Stage-Rejects are durably logged.
//!
//! Before A6 a validate-stage reject (template_missing, scope_out_of_bounds,
//! naming_collision, …) left NOTHING in `mutation_log` — only a synchronous
//! reply + a trace. The `/colony/mutations` audit showed committed/in_flight/
//! failed, never rejects (the K-H2 radar note: schema-rejects were invisible in
//! the audit log). A6 closes the gap: every validate-reject writes a
//! `status='rejected'` row carrying error_code + reason + trace_id + timestamp,
//! while the synchronous reject-reply to the requester stays UNCHANGED.
//!
//! Five pins (a–e):
//! (a) reject ⇒ a rejected row with all five fields.
//! (b) the synchronous Rejected reply is unchanged (Bestands-Pin).
//! (c) `/colony/mutations` shows rejected next to committed.
//! (d) a successful mutation ⇒ only a committed row, no duplicate reject row.
//! (e) Validate-Reject vs. Apply-failed are cleanly separated (status='rejected',
//!     committed_at NULL, error_code set — distinct from the committed path).

use meclaw_colony::api_dto::{MutationLogDto, ReadMutationsAuditReply};
use meclaw_colony::{ColonyMsg, MutationOutcome};
use meclaw_core::{Path, Uuid};
use meclaw_testing::ColonyHandle;

/// Phase-11 T16: create + load a template directory for `name`/`cell_type`.
///
/// GH #294: `params` is the template's own params block, written verbatim. A
/// template must DECLARE the param a mutation overrides, so the tests that set
/// `emitted_target` hand in a declaration for it — while
/// [`apply_stage_spawn_failure_logs_failed_not_rejected`] deliberately hands in
/// `{}`, because its whole subject is a node that reaches the spawn step
/// WITHOUT that param.
async fn setup_template(h: &ColonyHandle, name: &str, cell_type: &str, params: &str) {
    let root = h.tempdir_path();
    let templates_root = root.join("templates");
    let tpl = templates_root.join(name);
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), format!(r#"{{"name":"{name}"}}"#)).unwrap();
    std::fs::write(
        tpl.join("config.json"),
        format!(
            r#"{{"cell":{{"type":"{cell_type}"}},"params":{params},"contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
        ),
    )
    .unwrap();
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .expect("GH #440: the rescan must not have aborted");
}

/// Send a mutation with an explicit `trace_id` and await the outcome.
async fn send_mutation(
    h: &ColonyHandle,
    payload: meclaw_core::serde_json::Value,
    reply_to: Option<Path>,
    trace_id: Uuid,
) -> MutationOutcome {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to,
            trace_id,
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap()
}

/// Read the full `/colony/mutations` audit log.
async fn read_audit(h: &ColonyHandle) -> Vec<MutationLogDto> {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel::<ReadMutationsAuditReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadMutationsAudit {
            since: None,
            limit: 1000,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap().entries
}

/// A mutation that always rejects at the validate stage: template not in registry
/// → `template_missing`.
fn reject_payload() -> meclaw_core::serde_json::Value {
    meclaw_core::serde_json::json!({
        "scope": "/",
        "diff": { "add_nodes": [{"name": "ghost_cell", "template": "ghost"}] }
    })
}

/// A mutation that commits: add an echo node under a loaded echo template.
/// `emitted_target` is required for the echo cell to spawn — set it so the mutation
/// reaches `committed` instead of failing at the spawn step.
fn commit_payload(name: &str) -> meclaw_core::serde_json::Value {
    meclaw_core::serde_json::json!({
        "scope": "/",
        "diff": { "add_nodes": [{
            "name": name,
            "template": "echo",
            "override_params": {"emitted_target": "/"}
        }]}
    })
}

/// (a) A validate-stage reject writes a `status='rejected'` row carrying all five
/// fields: status / error_code / reason (failure_reason) / trace_id / created_at.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn validate_reject_writes_rejected_row_with_five_fields() {
    let h = ColonyHandle::new_with_echo();
    let trace = Uuid::parse_str("019ebb7e-0000-7000-8000-000000000a6a").unwrap();

    let outcome = send_mutation(&h, reject_payload(), None, trace).await;
    assert!(
        matches!(outcome, MutationOutcome::Rejected { .. }),
        "expected Rejected, got {outcome:?}"
    );

    let rows = read_audit(&h).await;
    let row = rows
        .iter()
        .find(|r| r.status == "rejected")
        .expect("a rejected row exists in the audit log");
    // Five fields.
    assert_eq!(row.status, "rejected");
    assert_eq!(
        row.error_code.as_deref(),
        Some("template_missing"),
        "error_code field"
    );
    let reason = row.failure_reason.as_deref().unwrap_or("");
    assert!(!reason.is_empty(), "reason (failure_reason) is populated");
    assert_eq!(
        row.trace_id.as_deref(),
        Some(trace.to_string().as_str()),
        "trace_id field carries the request's trace"
    );
    assert!(row.created_at > 0, "timestamp field is set");

    h.shutdown().await;
}

/// (b) Bestands-Pin: the synchronous Rejected reply to the requester is unchanged
/// — the audit row is ADDITIVE, the requester still learns the outcome
/// immediately with the same error_code + details it always got.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn synchronous_reject_reply_is_unchanged() {
    let h = ColonyHandle::new_with_echo();
    let trace = Uuid::now_v7();

    let outcome = send_mutation(&h, reject_payload(), None, trace).await;
    match outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(error_code, "template_missing");
            assert!(!details.is_empty(), "details still delivered synchronously");
        }
        other => panic!("expected Rejected, got {other:?}"),
    }

    h.shutdown().await;
}

/// (c) `/colony/mutations` shows a rejected row next to a committed one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn audit_read_shows_rejected_next_to_committed() {
    let h = ColonyHandle::new_with_echo();
    setup_template(&h, "echo", "echo", r#"{"emitted_target":"/unset"}"#).await;

    let committed = send_mutation(&h, commit_payload("alpha"), None, Uuid::now_v7()).await;
    assert!(
        matches!(committed, MutationOutcome::Committed { .. }),
        "expected Committed, got {committed:?}"
    );
    let rejected = send_mutation(&h, reject_payload(), None, Uuid::now_v7()).await;
    assert!(matches!(rejected, MutationOutcome::Rejected { .. }));

    let statuses: Vec<String> = read_audit(&h).await.into_iter().map(|r| r.status).collect();
    assert!(
        statuses.iter().any(|s| s == "committed"),
        "audit has a committed row, got {statuses:?}"
    );
    assert!(
        statuses.iter().any(|s| s == "rejected"),
        "audit has a rejected row, got {statuses:?}"
    );

    h.shutdown().await;
}

/// (d) A successful mutation leaves exactly ONE committed row for its id — no
/// duplicate reject row, no extra bookkeeping.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn committed_mutation_leaves_single_committed_row_no_reject() {
    let h = ColonyHandle::new_with_echo();
    setup_template(&h, "echo", "echo", r#"{"emitted_target":"/unset"}"#).await;

    let id = match send_mutation(&h, commit_payload("beta"), None, Uuid::now_v7()).await {
        MutationOutcome::Committed { id } => id,
        other => panic!("expected Committed, got {other:?}"),
    };

    let rows = read_audit(&h).await;
    let for_id: Vec<&MutationLogDto> = rows.iter().filter(|r| r.id == id).collect();
    assert_eq!(
        for_id.len(),
        1,
        "exactly one row for the committed mutation"
    );
    assert_eq!(for_id[0].status, "committed");
    assert!(
        !rows.iter().any(|r| r.status == "rejected"),
        "a successful mutation writes no rejected row"
    );

    h.shutdown().await;
}

/// (e′) The demarcation from the other side: an APPLY-stage failure (echo node
/// without the required `emitted_target` param → spawn fails AFTER validation, after the
/// `in_flight` row is written) leaves a `status='failed'` row — NEVER a
/// `rejected` one. Validate-Reject and Apply-failed are distinct classes; the
/// `INSERT OR IGNORE` reject path must not conflate them into a duplicate row.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn apply_stage_spawn_failure_logs_failed_not_rejected() {
    // GH #404 changed how this spot is reached, not what it asserts. The
    // provocation used to be an `echo` node without its `emitted_target`: the
    // mutation lane did not deserialize the params it wrote, so a params defect
    // passed validation and died at the spawn step. The boot-parity guard
    // refuses that during staging now, as `invalid_params`, pre-destructively —
    // which is the whole point of #404 and would make this test assert the
    // wrong class if it kept the old provocation.
    //
    // The apply-stage failure itself is still real: a spawn can fail for
    // reasons no parse can see. `SpawnRefusesCellFactory` is a test factory
    // that diverges from the parser invariant deliberately (validates
    // everything, spawns nothing), which is the only honest way to stand in
    // that spot now that no shipped factory may.
    let td = tempfile::TempDir::new().expect("tempdir");
    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![(
            "spawn_refuses".to_string(),
            std::sync::Arc::new(meclaw_testing::SpawnRefusesCellFactory)
                as std::sync::Arc<dyn meclaw_colony::CellFactory>,
        )],
    );
    setup_template(&h, "norefuse", "spawn_refuses", "{}").await;

    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": { "add_nodes": [{"name": "norefuse", "template": "norefuse"}] }
        }),
        None,
        Uuid::now_v7(),
    )
    .await;
    let id = match outcome {
        MutationOutcome::Rejected { id, error_code, .. } => {
            assert_eq!(error_code, "spawn", "fails at apply/spawn, not validate");
            id.expect("apply-stage failure carries the mutation id")
        }
        other => panic!("expected apply-stage Rejected(spawn), got {other:?}"),
    };

    let rows = read_audit(&h).await;
    let for_id: Vec<&MutationLogDto> = rows.iter().filter(|r| r.id == id).collect();
    assert_eq!(
        for_id.len(),
        1,
        "exactly one row for the apply-failed mutation — no duplicate reject row"
    );
    assert_eq!(
        for_id[0].status, "failed",
        "apply-stage failure is logged as 'failed', not 'rejected'"
    );
    assert!(
        !rows.iter().any(|r| r.id == id && r.status == "rejected"),
        "no 'rejected' row is written for an apply-stage failure"
    );

    h.shutdown().await;
}

/// (e) Validate-Reject vs. Apply-failed are cleanly separated. A rejected row is
/// `status='rejected'`, `committed_at IS NULL`, `error_code` set — structurally
/// distinct from the committed path (committed_at set, error_code NULL). They are
/// never conflated into one status.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reject_row_is_structurally_distinct_from_committed() {
    let h = ColonyHandle::new_with_echo();
    setup_template(&h, "echo", "echo", r#"{"emitted_target":"/unset"}"#).await;

    send_mutation(&h, commit_payload("gamma"), None, Uuid::now_v7()).await;
    send_mutation(&h, reject_payload(), None, Uuid::now_v7()).await;

    let rows = read_audit(&h).await;
    let rejected = rows
        .iter()
        .find(|r| r.status == "rejected")
        .expect("rejected row");
    let committed = rows
        .iter()
        .find(|r| r.status == "committed")
        .expect("committed row");

    // Reject: terminal, never committed, carries a code.
    assert!(
        rejected.committed_at.is_none(),
        "a validate-reject never commits → committed_at NULL"
    );
    assert!(
        rejected.error_code.is_some(),
        "reject carries an error_code"
    );

    // Committed: distinct status, committed_at set, no reject error_code.
    assert_ne!(committed.status, rejected.status);
    assert!(
        committed.committed_at.is_some(),
        "committed row has commit ts"
    );
    assert!(
        committed.error_code.is_none(),
        "committed row carries no reject error_code"
    );

    h.shutdown().await;
}
