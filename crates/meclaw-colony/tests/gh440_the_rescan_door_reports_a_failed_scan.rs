//! GH #440 (Ruling L3-S): a rescan that ABORTED must not answer `ok`.
//!
//! `scan_templates_dir` refuses a duplicate template name and names both
//! directories (GH #277, ruling Q7 — that refusal stays). The EDA door has
//! always forwarded those words verbatim. The HTTP door did not: its ack was
//! `oneshot::Sender<()>`, so `post_rescan` had exactly one return value and it
//! said `ok`. An operator whose tree carried two `talky`s learned nothing here
//! and found out at the next boot, which exits 1.
//!
//! The verbatim half of this file is inherited from
//! `gh355_a_failed_rescan_stops_the_promotion.rs`, which read
//! `templates/builder-hive` and dies with it (this wave). Its assertion was
//! that the scanner's own words survive un-tidied; deliberately awkward on
//! purpose — braces, quotes and a path-shaped field are exactly what a
//! "helpful" rewrite would smooth away, and smoothing it away is the bug.

use meclaw_colony::{
    CellFactoryRegistry, ColonyConfig, ColonyDb, ColonyMsg, ColonyTaskConfig, colony_task,
};
use tokio::sync::{mpsc, oneshot};

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
    std::fs::write(p, body).expect("write");
}

/// Two directories, one name. The scan must abort and say both paths.
fn two_templates_of_one_name(templates: &std::path::Path) {
    write(
        templates,
        "one/template.json",
        r#"{"name":"ledger-unit","version":"1.0.0"}"#,
    );
    write(
        templates,
        "one/config.json",
        r#"{"cell":{"type":"code"},"params":{"script_inline":"pass"}}"#,
    );
    write(
        templates,
        "two/template.json",
        r#"{"name":"ledger-unit","version":"2.0.0"}"#,
    );
    write(
        templates,
        "two/config.json",
        r#"{"cell":{"type":"code"},"params":{"script_inline":"pass"}}"#,
    );
}

/// Boot form taken verbatim from `gh277_rescan_scans_the_templates_root.rs` —
/// integration tests share no code, so the shape is copied rather than
/// factored out.
struct Booted {
    inbox_tx: mpsc::Sender<ColonyMsg>,
    join: tokio::task::JoinHandle<()>,
}

fn boot(root: &std::path::Path) -> Booted {
    let (inbox_tx, inbox_rx) = mpsc::channel(16);
    let (outputs_tx, outputs_rx) = mpsc::channel(16);
    let db = ColonyDb::open(&root.join("colony.db")).expect("open colony.db");
    let join = tokio::spawn(colony_task(ColonyTaskConfig::new(
        inbox_tx.clone(),
        inbox_rx,
        outputs_tx.clone(),
        outputs_rx,
        db,
        CellFactoryRegistry::new(),
        root.to_path_buf(),
        ColonyConfig::default(),
        None,
        None,
    )));
    Booted { inbox_tx, join }
}

async fn shutdown(b: Booted) {
    let (ack_tx, ack_rx) = oneshot::channel();
    b.inbox_tx
        .send(ColonyMsg::Shutdown { ack: ack_tx })
        .await
        .expect("send shutdown");
    ack_rx.await.expect("shutdown ack");
    b.join.await.expect("colony task");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_duplicate_name_reaches_the_caller_of_the_rescan_message() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let templates = td.path().join("templates");
    two_templates_of_one_name(&templates);

    let booted = boot(td.path());

    let (ack_tx, ack_rx) = oneshot::channel();
    booted
        .inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: templates.clone(),
            ack: ack_tx,
        })
        .await
        .expect("send");
    let outcome = ack_rx.await.expect("ack");

    let err = outcome.expect_err(
        "the scan aborted on a duplicate name, so the ack must carry the refusal — \
         an `Ok(())` here is the defect: the caller cannot tell a finished scan \
         from an aborted one",
    );
    assert!(
        err.contains("DuplicateName")
            && err.contains("ledger-unit")
            // The two directory paths are the half an operator acts on: without
            // them the message names a collision nobody can locate. The fixture
            // this assertion inherited from (gh355) was deliberately awkward for
            // exactly this reason -- braces, quotes and two path-shaped fields
            // are what a "helpful" rewrite smooths away, and smoothing them away
            // is the bug.
            && err.contains("first:")
            && err.contains("second:"),
        "the scanner's own words must survive verbatim, un-tidied: {err}",
    );

    shutdown(booted).await;
}

/// The counter-case: a library the scanner accepts still answers `Ok`. Without
/// it the repair could have been "always report an error" and nothing would
/// say so.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_clean_library_still_answers_ok() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let templates = td.path().join("templates");
    write(
        &templates,
        "ledger-unit/template.json",
        r#"{"name":"ledger-unit","version":"1.0.0"}"#,
    );
    write(
        &templates,
        "ledger-unit/config.json",
        r#"{"cell":{"type":"code"},"params":{"script_inline":"pass"}}"#,
    );

    let booted = boot(td.path());

    let (ack_tx, ack_rx) = oneshot::channel();
    booted
        .inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: templates.clone(),
            ack: ack_tx,
        })
        .await
        .expect("send");
    ack_rx
        .await
        .expect("ack")
        .expect("a library without a duplicate name scans cleanly");

    shutdown(booted).await;
}
