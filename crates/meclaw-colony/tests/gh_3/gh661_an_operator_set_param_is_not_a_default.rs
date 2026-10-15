//! GH #661 — a default that has no meaning is declared, and the door enforces it.
//!
//! A template's `params` are defaults, and a wish that leaves them alone is
//! legal. Some of them are not defaults in any useful sense: a URL pointing at
//! localhost, an empty identity, an empty allowlist. They are the SHAPE of a
//! value, and a node grown with them cannot do the one thing it was grown for.
//!
//! Measured on 0.35.0: a wish that described the wiring of a telephone channel
//! grew its signal half with every shipped default. The mutation committed, the
//! receipt was green, and the colony had a channel that would never have reached
//! any switch. The `description` beside those params said in prose that an
//! operator has to set them — and prose no door reads is exactly how that colony
//! got built green.
//!
//! So the statement moves into `contract.settings`, per param, beside `secret`:
//! `operator_set: true`. The colony is the single write authority, so the colony
//! is what enforces it, at stage 4, pre-destructively, whoever submitted the
//! mutation.
//!
//! Two properties of the check are decisions rather than details, and both are
//! measured here:
//!
//! - **The act is checked, not the value** (OR-A2). An entry that sets the param
//!   to exactly the shipped default comes through. A value comparison would have
//!   to run after `${VAR}` substitution, in a different phase, and it would read
//!   the one honest thing an operator can say — *yes, this default is right
//!   here* — as an omission.
//! - **Stage 4 collects.** Three unset params are three named violations in one
//!   refusal, not three round trips.

use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome};
use meclaw_core::serde_json::json;
use meclaw_core::{JsonValue, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::PersistCellFactory;
use std::sync::Arc;
use tokio::sync::oneshot;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
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

async fn rescan_templates(h: &ColonyHandle, templates_root: std::path::PathBuf) {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap().expect("the rescan must not abort");
}

fn persist_factory() -> Arc<dyn CellFactory> {
    Arc::new(PersistCellFactory {
        spawn_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
    }) as Arc<dyn CellFactory>
}

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![("persist_mock".to_string(), persist_factory())]
}

fn registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert("persist_mock".into(), persist_factory());
    r
}

/// The shipped default of the one param the templates below declare
/// `operator_set` — a URL pointing at the machine the cell happens to run on,
/// which is the shape of the measured case.
const SHIPPED: &str = "ws://127.0.0.1:7777/line";

/// `line`: one cell, one `operator_set` param beside an ordinary one.
/// `three`: the same shape with three of them, for the collecting half.
/// `plain`: the control — same cell type, nothing declared.
fn write_templates(root: &std::path::Path) {
    let line = root.join("templates").join("line");
    write(&line, "template.json", r#"{"name":"line"}"#);
    write(
        &line,
        "config.json",
        &format!(
            r#"{{"cell":{{"type":"persist_mock"}},
                 "params":{{"ws_url":"{SHIPPED}","retries":3}},
                 "contract":{{"version":"0.1.0","consumes":{{}},"settings":{{
                     "ws_url":{{"type":"string","secret":false,"operator_set":true,
                                "default":"{SHIPPED}",
                                "description":"where the line is"}},
                     "retries":{{"type":"number","default":3,
                                 "description":"a working value"}}}}}}}}"#
        ),
    );

    let three = root.join("templates").join("three");
    write(&three, "template.json", r#"{"name":"three"}"#);
    write(
        &three,
        "config.json",
        r#"{"cell":{"type":"persist_mock"},
            "params":{"ws_url":"","user_id":"","callers":{}},
            "contract":{"version":"0.1.0","consumes":{},"settings":{
                "ws_url":{"type":"string","operator_set":true,"default":""},
                "user_id":{"type":"string","operator_set":true,"default":""},
                "callers":{"type":"object","operator_set":true,"default":{}}}}}"#,
    );

    let plain = root.join("templates").join("plain");
    write(&plain, "template.json", r#"{"name":"plain"}"#);
    write(
        &plain,
        "config.json",
        r#"{"cell":{"type":"persist_mock"},"params":{"ws_url":""},
            "contract":{"version":"0.1.0","consumes":{},"settings":{
                "ws_url":{"type":"string","default":""}}}}"#,
    );
}

async fn boot(td: &tempfile::TempDir) -> ColonyHandle {
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write_templates(td.path());
    let h = ColonyHandle::new_with_factories_at(td, factories());
    rescan_templates(&h, td.path().join("templates")).await;
    meclaw_colony::bootstrap_from_filesystem(td.path(), &registry(), &h.runtime())
        .await
        .expect("the colony boots");
    h
}

/// The details of a mutation that must be refused with `operator_param_unset`.
fn unset_details(outcome: &MutationOutcome) -> String {
    match outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(error_code, "operator_param_unset", "{outcome:?}");
            details.clone()
        }
        other => panic!("expected an operator_param_unset rejection, got {other:?}"),
    }
}

/// The instance's params, read off the disk the mutation wrote.
fn instance_params(td: &std::path::Path, rel: &str) -> JsonValue {
    let raw = std::fs::read_to_string(td.join(rel).join("config.json"))
        .unwrap_or_else(|e| panic!("read {rel}/config.json: {e}"));
    let v: JsonValue = meclaw_core::serde_json::from_str(&raw).expect("config json");
    v["params"].clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_node_grown_without_its_operator_set_param_is_refused() {
    let td = tempfile::TempDir::new().unwrap();
    let h = boot(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[{"name":"l1","template":"line"}]}}),
    )
    .await;
    let details = unset_details(&outcome);
    assert!(
        details.contains("ws_url"),
        "the refusal names the param that was left alone: {details}"
    );
    assert!(
        details.contains("operator_set"),
        "and why it is not a default: {details}"
    );
    assert!(
        details.contains("line"),
        "and the template that declares it: {details}"
    );
    assert!(
        details.contains(SHIPPED),
        "and the shipped value it would have grown with: {details}"
    );
    assert!(
        !details.contains("retries"),
        "an ordinary default is left alone — it is a working value, and naming \
         it here would train an operator to ignore the list: {details}"
    );
    assert!(
        !td.path().join("main/l1").exists(),
        "and nothing is materialised: the refusal is pre-destructive"
    );

    // The control: the same cell type and the same empty default, with nothing
    // declared, still commits. The check is the declaration and not the value.
    let plain = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[{"name":"p1","template":"plain"}]}}),
    )
    .await;
    assert!(
        matches!(plain, MutationOutcome::Committed { .. }),
        "a template that declares nothing keeps behaving exactly as it did; got {plain:?}"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_entry_with_the_value_commits() {
    let td = tempfile::TempDir::new().unwrap();
    let h = boot(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[
            {"name":"l1","template":"line",
             "override_params":{"ws_url":"ws://switch.invalid:7777/line"}}
        ]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the entry says what the value is, which is the whole of what was asked; got {outcome:?}"
    );
    assert_eq!(
        instance_params(td.path(), "main/l1")["ws_url"],
        json!("ws://switch.invalid:7777/line"),
        "and the value reaches the instance"
    );

    h.shutdown().await;
}

/// OR-A2 — what is checked is the ACT, not the value.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn setting_it_to_the_shipped_default_is_setting_it() {
    let td = tempfile::TempDir::new().unwrap();
    let h = boot(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[
            {"name":"l1","template":"line","override_params":{"ws_url":SHIPPED}}
        ]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "\"yes, this default is right here\" is the one honest thing an operator \
         can say about such a param, and reading it as an omission would make the \
         declaration unanswerable; got {outcome:?}"
    );
    assert_eq!(
        instance_params(td.path(), "main/l1")["ws_url"],
        json!(SHIPPED)
    );

    h.shutdown().await;
}

/// Stage 4 collects — so does this check.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_missing_params_are_named_in_one_refusal() {
    let td = tempfile::TempDir::new().unwrap();
    let h = boot(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[{"name":"t1","template":"three"}]}}),
    )
    .await;
    let details = unset_details(&outcome);
    for key in ["ws_url", "user_id", "callers"] {
        assert!(
            details.contains(key),
            "all three are named in the one refusal, {key} is missing: {details}"
        );
    }
    match &outcome {
        MutationOutcome::Rejected { violations, .. } => {
            assert_eq!(
                violations.len(),
                3,
                "three unset params are three violations, not one summary and \
                 not three round trips: {violations:?}"
            );
        }
        other => panic!("{other:?}"),
    }

    h.shutdown().await;
}

/// OR-A3 — both doors that grow a node from a template ask the same question.
///
/// `stage.rs` maps `swap_nodes[].with.params` onto the very `override_params`
/// contract an `add_nodes` entry writes, so a swap that re-instantiates the
/// class is the measured defect with a different operation name in front of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_swap_that_regrows_the_class_is_refused_the_same_way() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/t2/config.json",
        r#"{"cell":{"type":"persist_mock"},"params":{"p":1},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    let h = boot(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"swap_nodes":[
            {"match":{"name":"t2"},
             "with":{"template":"line","name":"t3","params":{"retries":5}}}
        ]}}),
    )
    .await;
    let details = unset_details(&outcome);
    assert!(
        details.contains("ws_url") && details.contains("operator_set"),
        "the swap side names the param and why it is not a default: {details}"
    );
    assert!(
        !td.path().join("main/t3").exists(),
        "and the successor is not staged: pre-destructive at this door too"
    );

    // The counter-proof: the same swap, with the value named.
    let good = send_mutation(
        &h,
        json!({"scope":"/","diff":{"swap_nodes":[
            {"match":{"name":"t2"},
             "with":{"template":"line","name":"t3",
                     "params":{"ws_url":"ws://switch.invalid:7777/line"}}}
        ]}}),
    )
    .await;
    assert!(
        matches!(good, MutationOutcome::Committed { .. }),
        "a swap that says what the value is commits; got {good:?}"
    );
    assert_eq!(
        instance_params(td.path(), "main/t3")["ws_url"],
        json!("ws://switch.invalid:7777/line")
    );

    h.shutdown().await;
}

/// The swap door collects too — three unset params, one refusal.
///
/// Stage 4 collects, and the briefing the design lane reads promises it without
/// qualification (*naming every such param of the entry at once*), as does
/// `docs/config*.md` for both doors. A door that names one param sends the
/// submitter round three times and makes that sentence false at one of the two
/// places it covers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_swap_names_every_unset_param_in_one_refusal() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/t2/config.json",
        r#"{"cell":{"type":"persist_mock"},"params":{"p":1},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    let h = boot(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"swap_nodes":[
            {"match":{"name":"t2"},
             "with":{"template":"three","name":"t3","params":{}}}
        ]}}),
    )
    .await;
    let details = unset_details(&outcome);
    for key in ["ws_url", "user_id", "callers"] {
        assert!(
            details.contains(key),
            "the swap door names all three in the one refusal, {key} is missing: {details}"
        );
    }
    assert_eq!(
        details.lines().count(),
        3,
        "one line per unset param, the shape the add door writes: {details}"
    );
    assert!(!td.path().join("main/t3").exists(), "and nothing is staged");

    h.shutdown().await;
}

/// OR-A4 — an `adopt` entry is exempt, a RESUME is not.
///
/// An `adopt` entry names no template (`adopt` and `template` are mutually
/// exclusive), so there is no declaration for it to meet: it takes over a tree
/// somebody already placed, exactly as it lies.
///
/// A resume stages the template tree AGAIN and writes `config.json` again, which
/// is precisely how the silent defaults get onto disk — so it is judged. That
/// deviates on purpose from stage 3, where `validate_requires` skips resumes:
/// there the subject is ctx keys a reconnect never consumes, here it is params a
/// resume rewrites.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn adopt_is_exempt_and_a_resume_is_not() {
    let td = tempfile::TempDir::new().unwrap();
    let h = boot(&td).await;

    // The adopt target: a valid cell placed after the boot, so it is on disk and
    // unregistered — the shape `adopt` exists for.
    write(
        td.path(),
        "main/taken/config.json",
        r#"{"cell":{"type":"persist_mock"},"params":{"ws_url":""},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    let adopted = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[
            {"name":"taken","adopt":{"type":"persist_mock"}}
        ]}}),
    )
    .await;
    assert!(
        matches!(adopted, MutationOutcome::Committed { .. }),
        "an adopt entry names no template and therefore no declaration; got {adopted:?}"
    );

    // The resume: an `add_nodes` at a path that already stands, naming the
    // template again and bringing no value.
    let resumed = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[{"name":"taken","template":"line"}]}}),
    )
    .await;
    let details = unset_details(&resumed);
    assert!(
        details.contains("ws_url"),
        "a resume writes the template's params again, so it owes the same \
         declaration a fresh instantiation owes: {details}"
    );

    h.shutdown().await;
}
