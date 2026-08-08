//! Paket-7 F1 — end-to-end `contract.emits` enforcement demos for the
//! `code`-Cell (D-010a / D-017).
//!
//! Lives in `meclaw-cells/tests/` (NOT `meclaw-colony/tests/`) because it
//! drives a real `CodeCellFactory` under a live Colony topology — the
//! cross-cutting demo pattern established in Phase 9 V1 (a colony test that
//! needs a cell-factory belongs here, so `meclaw-colony` keeps NO `meclaw-cells`
//! dependency).
//!
//! Demos:
//!   (b)       conforming code emission → arrives UNCHANGED at the CaptureCell
//!             sink (Zero-Drift); the FINAL wire content (incl. the injected
//!             standard headers exit_code/duration_ms/had_stderr) satisfies
//!             `contract.emits` (Auflage A1: validate the post-injection content).
//!   (c)       body-violating emission → `contract_violation` at `reply_to`,
//!             message preserved; SEPARATELY a header-violating emission → also
//!             `contract_violation` (Reviewer-Punkt 1: body AND header enforced).
//!   (c-multi) multi-send with a violation in message 2 → sink receives NOTHING
//!             regular, exactly one `contract_violation` (Auflage A2).
//!   (d)       knob-chain: the same violating output with `validate_emits=false`
//!             passes UNCHANGED (no contract_violation); with `=true` →
//!             contract_violation. Direct flag injection at the cell, because
//!             `cfg!(debug_assertions)` is true in test builds (Plan-Notiz to B5).
//!   (a3)      reconnect pin — MOVED TO PAKET 8 (see the note at the bottom of
//!             this file). The disconnect→reconnect eager-respawn path cannot be
//!             driven for a real stateless `code` cell today because
//!             `stateless_dispatcher` does not fire `peace_tx` on a colony
//!             peace-stop, so a disconnect yields `CellDied{Normal}` →
//!             `registry.remove` (no entry left to reconnect). That is a
//!             pre-existing substrate gap, resolved in Paket 8
//!             (stateless disconnect); this a3 demo became its acceptance demo.
//!             The B5/C2 units already pin the flag-survival construction here.
//!
//! Anti-cascade (phase-6.5/7 demo discipline): `/sink` is a terminal CaptureCell,
//! spawned and resolved BEFORE any probe goes out. The sink receipt is the proof
//! (positive signal: a message arrives ⟺ the emission passed the contract).

use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::{CellFactory, ColonyMsg, MutationOutcome};
use meclaw_core::serde_json::json;
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Generous failure-marker timeout (CLAUDE.md 30s-convention, robust under
/// cargo-parallel load).
const RECV_TIMEOUT: Duration = Duration::from_secs(30);

/// Build a Colony with a real `CodeCellFactory` registered under `"code"`,
/// plus a terminal `/sink` CaptureCell (spawned + resolved BEFORE any probe).
async fn colony_with_sink(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let factory: Arc<dyn CellFactory> = Arc::new(CodeCellFactory);
    let h = ColonyHandle::new_with_factories_at(td, vec![("code".to_string(), factory)]);

    let (sink_tx, sink_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    (h, sink_rx)
}

/// Install a `code` template `name` whose script writes `script_stdout`
/// verbatim, declaring `contract.emits = emits_json` (+ optional
/// `multi_send_capable`). Then load it via `RescanTemplates`.
async fn install_code_template(
    td: &tempfile::TempDir,
    h: &ColonyHandle,
    name: &str,
    emits_json: &str,
    script_stdout: &str,
    multi_send_capable: bool,
) {
    let templates_root = td.path().join("templates");
    let tpl = templates_root.join(name);
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), format!(r#"{{"name":"{name}"}}"#)).unwrap();

    // The script echoes a fixed literal; embed it safely as a python string
    // literal via serde_json quoting.
    let literal = meclaw_core::serde_json::to_string(script_stdout).unwrap();
    let script = format!("import sys; sys.stdout.write({literal})");
    let config = json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 10000},
        "contract": {
            "version": "0.1.0",
            "settings": {},
            "consumes": {},
            "multi_send_capable": multi_send_capable,
            "emits": meclaw_core::serde_json::from_str::<meclaw_core::serde_json::Value>(emits_json).unwrap()
        }
    });
    std::fs::write(
        tpl.join("config.json"),
        meclaw_core::serde_json::to_string(&config).unwrap(),
    )
    .unwrap();

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

/// `add_nodes` a single code cell from `template` at logical `/<node>`.
async fn add_code_node(h: &ColonyHandle, node: &str, template: &str) {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: json!({
                "scope": "/",
                "diff": {"add_nodes": [{"name": node, "template": template}]}
            }),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    let outcome = ack_rx.await.unwrap();
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "add_nodes of /{node} from {template} must commit; got {outcome:?}"
    );
}

/// A probe addressed to `/<node>` with `reply_to=/sink`. Body carries a valid
/// (empty) UBF `messages` array; the cell ignores the input body and emits its
/// fixed script output.
fn probe(node: &str) -> Message {
    MessageBuilder::new(Path::new(&format!("/{node}")))
        .reply_to(Path::new("/sink"))
        .trace_id(Uuid::now_v7())
        .body(Body::Inline(json!({"messages": []})))
        .build()
}

async fn recv(sink_rx: &mut mpsc::Receiver<Message>, what: &str) -> Message {
    tokio::time::timeout(RECV_TIMEOUT, sink_rx.recv())
        .await
        .unwrap_or_else(|_| panic!("sink recv timeout: {what}"))
        .unwrap_or_else(|| panic!("sink channel closed: {what}"))
}

// ── (b) Conforming emission — Zero-Drift, post-injection content passes ──────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_b_conforming_emission_arrives_unchanged_at_sink() {
    let td = tempfile::TempDir::new().unwrap();
    let (h, mut sink_rx) = colony_with_sink(&td).await;

    // Contract requires body.messages:array (required) + header.exit_code:number
    // (required). The script writes a conforming body; the cell injects the
    // standard headers (exit_code/duration_ms/had_stderr) — so the FINAL wire
    // content satisfies BOTH the body and the (injected) header requirement.
    install_code_template(
        &td,
        &h,
        "conform",
        r#"{"body":{"messages":{"type":"array","required":true}},
            "hop":{"exit_code":{"type":"number","required":true}}}"#,
        r#"{"messages":[{"origin":"assistant","type":"text","text":"ok"}]}"#,
        false,
    )
    .await;
    add_code_node(&h, "conform", "conform").await;
    // W2 (A1): reply to /sink now needs a wired edge (identity gone).
    h.add_edge(Uuid::now_v7(), Path::new("/conform"), Path::new("/sink"))
        .await;

    h.send_from(Path::new("/"), probe("conform")).await;
    let m = recv(&mut sink_rx, "conforming emission").await;

    // POSITIVE receipt: a regular message arrived (NOT a contract_violation).
    assert!(
        m.headers.hop.get("error_code").is_none(),
        "conforming emission must NOT carry an error_code; headers: {:?}",
        m.headers
    );
    // The body is unchanged (Zero-Drift): the script's assistant turn survives.
    let body = match &m.body {
        Body::Inline(v) => v.clone(),
        other => panic!("non-inline body at sink: {other:?}"),
    };
    assert_eq!(body["messages"][0]["origin"], "assistant");
    assert_eq!(body["messages"][0]["text"], "ok");
    // The injected standard header is present on the wire (A1: it is part of the
    // validated emission). Colony splits `header` into `m.headers`.
    assert_eq!(
        m.headers.hop["exit_code"], 0,
        "injected exit_code header present on the wire content"
    );
    assert!(
        m.headers.hop.get("duration_ms").is_some(),
        "injected duration_ms header present"
    );
    assert_eq!(
        m.headers.hop["had_stderr"], false,
        "injected had_stderr header"
    );

    h.shutdown().await;
}

// ── (c) Body violation AND header violation → contract_violation ────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_c_body_violation_yields_contract_violation_message_preserved() {
    let td = tempfile::TempDir::new().unwrap();
    let (h, mut sink_rx) = colony_with_sink(&td).await;

    // Contract requires body.messages:array required; the script writes an
    // object WITHOUT messages. inject_standard_headers only touches the header
    // → the body stays violated on the final content.
    install_code_template(
        &td,
        &h,
        "bodyviol",
        r#"{"body":{"messages":{"type":"array","required":true}},"hop":{}}"#,
        r#"{"foo":"bar"}"#,
        false,
    )
    .await;
    add_code_node(&h, "bodyviol", "bodyviol").await;
    // W2 (A1): reply to /sink now needs a wired edge (identity gone).
    h.add_edge(Uuid::now_v7(), Path::new("/bodyviol"), Path::new("/sink"))
        .await;

    h.send_from(Path::new("/"), probe("bodyviol")).await;
    let m = recv(&mut sink_rx, "body-violation receipt").await;

    // Receipt = error-message with error_code:"contract_violation", routed to
    // reply_to (=/sink). The message is preserved (delivered, not dropped).
    assert_eq!(
        m.headers.hop["error_code"], "contract_violation",
        "body violation must yield contract_violation; headers: {:?}",
        m.headers
    );
    assert_eq!(m.headers.hop["finish_reason"], "error");

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_c_header_violation_yields_contract_violation() {
    let td = tempfile::TempDir::new().unwrap();
    let (h, mut sink_rx) = colony_with_sink(&td).await;

    // Contract requires header.must_have:string required; the script writes a
    // valid body but never sets that header. inject_standard_headers adds
    // exit_code/duration_ms/had_stderr but NOT `must_have` → header stays
    // violated post-injection (Reviewer-Punkt 1: header is enforced).
    install_code_template(
        &td,
        &h,
        "hdrviol",
        r#"{"body":{},"hop":{"must_have":{"type":"string","required":true}}}"#,
        r#"{"messages":[{"origin":"assistant","type":"text","text":"x"}]}"#,
        false,
    )
    .await;
    add_code_node(&h, "hdrviol", "hdrviol").await;
    // W2 (A1): reply to /sink now needs a wired edge (identity gone).
    h.add_edge(Uuid::now_v7(), Path::new("/hdrviol"), Path::new("/sink"))
        .await;

    h.send_from(Path::new("/"), probe("hdrviol")).await;
    let m = recv(&mut sink_rx, "header-violation receipt").await;

    assert_eq!(
        m.headers.hop["error_code"], "contract_violation",
        "header violation must yield contract_violation; headers: {:?}",
        m.headers
    );
    assert_eq!(m.headers.hop["finish_reason"], "error");

    h.shutdown().await;
}

// ── (c-multi) multi-send, violation in message 2 → 1 violation, 0 regular ───

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_c_multi_send_violation_in_second_message_nothing_regular() {
    let td = tempfile::TempDir::new().unwrap();
    let (h, mut sink_rx) = colony_with_sink(&td).await;

    // multi_send_capable: the script emits a 2-element array. [0] is valid,
    // [1] violates (no messages). Auflage A2 (two-pass): exactly ONE
    // contract_violation reaches the sink, ZERO regular emissions.
    install_code_template(
        &td,
        &h,
        "multiviol",
        r#"{"body":{"messages":{"type":"array","required":true}},"hop":{}}"#,
        r#"[{"messages":[]},{"foo":"bar"}]"#,
        true,
    )
    .await;
    add_code_node(&h, "multiviol", "multiviol").await;
    // W2 (A1): reply to /sink now needs a wired edge (identity gone).
    h.add_edge(Uuid::now_v7(), Path::new("/multiviol"), Path::new("/sink"))
        .await;

    h.send_from(Path::new("/"), probe("multiviol")).await;
    let m = recv(&mut sink_rx, "multi-send violation receipt").await;
    assert_eq!(
        m.headers.hop["error_code"], "contract_violation",
        "multi-send violation in msg 2 must yield exactly one contract_violation"
    );

    // No further (regular) emission may follow: the first valid message must
    // NOT have been pushed (no partial-emit mix). Drain with a tight window.
    let extra = tokio::time::timeout(Duration::from_millis(500), sink_rx.recv()).await;
    assert!(
        extra.is_err(),
        "exactly ONE message (the violation); no regular emit leaked: got {extra:?}"
    );

    h.shutdown().await;
}

// ── (d) Knob-chain: flag off passes, flag on rejects ────────────────────────
//
// `cfg!(debug_assertions)` is true in test builds, so the colony-resolved flag
// is always `true` here (B5). Demo (d) therefore drives the cell DIRECTLY with
// explicit flag injection (Plan-Notiz to B5; same approach as the C2 flag_off
// unit). The resolve-chain itself is pinned by the B5 + C2 unit tests.

/// Build a `CodeCell` whose script writes `script_stdout`, with a compiled
/// `emits` contract and the given `validate` flag (multi_send_capable=false).
fn direct_code_cell(
    emits_json: &str,
    validate: bool,
    script_stdout: &str,
) -> meclaw_cells::code::CodeCell {
    let emits: meclaw_core::EmitsBlock = meclaw_core::serde_json::from_str(emits_json).unwrap();
    let compiled = Arc::new(meclaw_core::CompiledEmits::compile(&emits).unwrap());
    let literal = meclaw_core::serde_json::to_string(script_stdout).unwrap();
    let script = format!("import sys; sys.stdout.write({literal})");
    let params = meclaw_cells::code::CodeParams {
        runner: "python3".into(),
        script: meclaw_cells::code::Script::Inline(script),
        external_timeout_ms: Some(10_000),
        max_concurrency: None,
    };
    meclaw_cells::code::CodeCell::new(params, false, Some(compiled), validate)
}

/// Drive `cell.handle` once and collect ALL pushed emissions.
async fn drive_collect(cell: &meclaw_cells::code::CodeCell) -> Vec<meclaw_core::CellEmission> {
    use meclaw_colony::StatelessCell;
    let (otx, mut orx) = mpsc::channel(16);
    let sink = meclaw_core::OutputSink::new(
        otx,
        Path::new("/code"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        meclaw_core::Headers::new(),
        None,
    );
    let msg = MessageBuilder::new(Path::new("/code"))
        .body(Body::Inline(json!({"messages": []})))
        .reply_to(Path::new("/sink"))
        .build();
    cell.handle(msg, &sink).await;
    drop(sink);
    let mut outs = Vec::new();
    while let Some(em) = orx.recv().await {
        outs.push(em);
    }
    outs
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_d_knob_chain_off_passes_on_rejects() {
    let emits = r#"{"body":{"messages":{"type":"array","required":true}},"hop":{}}"#;
    // Same violating output (object without messages) for both runs.
    let violating = r#"{"foo":"bar"}"#;

    // Flag OFF (validate=false): the violating emission passes through UNCHANGED.
    let off = direct_code_cell(emits, false, violating);
    let outs_off = drive_collect(&off).await;
    assert_eq!(
        outs_off.len(),
        1,
        "flag-off: exactly one (unmodified) emission"
    );
    assert_ne!(
        outs_off[0].content["header"]["error_code"].as_str(),
        Some("contract_violation"),
        "flag-off: no contract_violation — validation skipped"
    );
    assert_eq!(
        outs_off[0].content["foo"], "bar",
        "flag-off: the violating body passed through unchanged"
    );

    // Flag ON (validate=true): the SAME output → contract_violation.
    let on = direct_code_cell(emits, true, violating);
    let outs_on = drive_collect(&on).await;
    assert_eq!(outs_on.len(), 1, "flag-on: exactly one (error) emission");
    assert_eq!(
        outs_on[0].content["header"]["error_code"], "contract_violation",
        "flag-on: same output now rejected"
    );
}

// ── (a3) Reconnect pin — MOVED TO PAKET 8 (blocked on a substrate gap) ───────
//
// GOAL: disconnect a real `code` cell (remove its last edge → inactive,
// dispatcher peace-stopped) and reconnect it (`add_edges` → eager
// `(entry.respawn)()` re-spawn), then prove a violating emission STILL yields a
// `contract_violation` (Auflage A3: the resolved `validate_emits` flag + compiled
// `emits` survive the respawn/reconnect path, not just the initial spawn).
//
// WHY IT CANNOT BE DRIVEN TEST-ONLY: a `code` cell is STATELESS and is spawned by
// `CodeCellFactory::spawn_cell` via `build_stateless_task`, which wires a REAL
// `stop_tx` into the dispatcher's `stop_rx`. The step-10b disconnect recompute
// (colony.rs) `take()`s that `stop_tx` and fires it; the dispatcher's biased
// stop-arm (`stateless_dispatcher` in `crates/meclaw-colony/src/cell_task.rs`)
// sends `ColonyMsg::Stopped` and RETURNS — but, unlike `cell_task_stateful`
// (cell_task.rs:206) and `cell_task_long_running` (cell_task.rs:424), it does NOT
// fire `peace_tx`. `build_stateless_task` holds `peace_tx` as `_peace_keep` and
// drops it on task end WITHOUT sending. The watcher (`spawn_watcher`, colony.rs)
// therefore sees `peace_rx.await == Err` → emits `CellDied{DeathKind::Normal}` →
// `handle_cell_died` does `registry.remove(&path)` (colony.rs:946-949). The
// registry entry is GONE after the disconnect, so the `add_edges` reconnect has
// no entry to flip `false→true` and no `(entry.respawn)()` to call — the eager
// reconnect path A3 must exercise is unreachable for a real stateless cell.
//
// Empirically confirmed: an attempted demo (code cell + persistent `/anchor ->
// /sink` anchor + `/reconnviol -> /sink` edge, disconnect via `remove_edges`)
// reads the registry `active` flag as `None` (entry removed), not `Some(false)`,
// immediately after the committed disconnect.
//
// CONTRAST — why the existing reconnect demos work and this does not:
//   • `phase_13_5_lifecycle_3b_reconnect.rs`
//     `reconnect_long_running_eager_respawns_immediately_db_continuity` uses a
//     LONG-RUNNING mock whose `cell_task_long_running` fires `peace_tx` on stop →
//     silent exit → NO `CellDied` → entry stays → eager reconnect re-spawns.
//   • `phase_13_5_lifecycle_3b_disconnect.rs`
//     `remove_last_edge_disconnects_cell_active_false_persisted` uses
//     `EchoCellFactory`, whose `SpawnedCellKind::Active` carries a PLACEHOLDER
//     `stop_tx` whose `_stop_rx` is dropped immediately — so the disconnect's
//     `stop_tx.send(())` is a no-op, the dispatcher is never actually stopped,
//     `peace_tx` never drops → NO `CellDied` → entry stays. `CodeCellFactory`
//     does NOT use a placeholder; it wires a real stop_tx, so it hits the gap.
//
// MISSING PRODUCTION CAPABILITY (do NOT implement here — forbidden corridor):
//   `stateless_dispatcher` must fire `peace_tx` on its colony peace-stop arm
//   (mirroring `cell_task_stateful`/`cell_task_long_running`) so a disconnected
//   stateless cell parks as `NotYetSpawned` (entry retained) instead of dying
//   `Normal`. Equivalently, `build_stateless_task` must pass a `peace_tx` the
//   dispatcher fires on stop. Either change lives in
//   `crates/meclaw-colony/src/cell_task.rs` (+ possibly `build_task.rs`), which is
//   production code outside this test-only task's scope. The B5
//   `resolve_validate_emits` unit (meclaw-colony) + the C2 flag-respect unit
//   (meclaw-cells `code/cell.rs`) already pin the flag-survival CONSTRUCTION; only
//   the live disconnect→reconnect SUBSTRATE for a stateless cell is missing.
//
// (a3) is therefore deferred to PAKET 8 (stateless-disconnect-Substrat):
// once `stateless_dispatcher` parks on peace-stop instead of
// dying `Normal`, this demo becomes Paket 8's acceptance demo — a reconnect-
// respawned `code` cell still validates `contract.emits`. Pre-existing substrate
// gap (no-delete l.1397 / reconnect l.1404 / graph-swap switch-back), surfaced
// by this a3 attempt (precedent: hive_endpoint_names, Paket-2-Demo → Paket-5).
