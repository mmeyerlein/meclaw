//! GH #277 (W3): `/colony/templates/rescan` scans the TEMPLATE LIBRARY — on
//! every transport.
//!
//! The endpoint has two doors. The HTTP door (`POST /colony/templates/rescan`)
//! hands the colony `ColonyHandle.templates_root` — the `--templates` path,
//! i.e. the library. The EDA door (a cell emitting to
//! `/colony/templates/rescan`) handed it the COLONY ROOT instead, so a rescan
//! triggered from inside the colony walked `main/`, `blobs/`, `staging/` — the
//! whole workspace — and registered as a class whatever `template.json` it
//! tripped over on the way.
//!
//! That divergence was survivable while a repeated name was merely shadowed.
//! Ruling Q7 (GH #277) made a repeated name a HARD scan error, and the builder
//! hive's promote step keeps the approved draft in `<root>/staging/` as history
//! while moving a copy into `<root>/templates/`. Two directories, one name,
//! both inside the colony root: the scan aborts, NOTHING is registered, and the
//! deploy that follows fails with `template_missing`. Q7 is right; the scan
//! path was wrong.
//!
//! Pinned here: with the draft parked in `staging/` under the same name as the
//! promoted class, the EDA rescan still registers exactly the promoted class.

use meclaw_colony::{
    CellFactoryRegistry, ColonyConfig, ColonyDb, ColonyMsg, ColonyTaskConfig, colony_task,
};
use meclaw_core::{MessageBuilder, Path};
use tokio::sync::{mpsc, oneshot};

fn write_template(dir: &std::path::Path, name: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("template.json"),
        format!(r#"{{"name":"{name}","version":"1.0.0"}}"#),
    )
    .unwrap();
    std::fs::write(dir.join("config.json"), r#"{"cell":{"type":"hive"}}"#).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn eda_rescan_scans_the_library_not_the_whole_workspace() {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();

    // The library: one promoted class.
    write_template(&root.join("templates/notes-unit"), "notes-unit");
    // The builder's history: the very same draft, parked OUTSIDE the library.
    // `staging/` is not a template library and must not be scanned as one.
    write_template(&root.join("staging/req-1/notes-unit"), "notes-unit");
    // An instantiated tree that still carries its class marker — also outside
    // the library, also none of the scanner's business.
    write_template(&root.join("main/notes"), "notes-unit");

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

    // The EDA door: a routed message, exactly as the builder's promote cell
    // emits it once the copy has landed in the library.
    inbox_tx
        .send(ColonyMsg::Route {
            sender_path: Path::new("/"),
            msg: MessageBuilder::new(Path::new("/colony/templates/rescan")).build(),
        })
        .await
        .unwrap();

    // The read runs through the same inbox, so it is ordered behind the rescan
    // — no sleep, no poll.
    let (ack_tx, ack_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::ReadTemplates {
            cell_type: None,
            name: None,
            limit: 100,
            ack: ack_tx,
        })
        .await
        .unwrap();
    let reply = ack_rx.await.unwrap();

    let names: Vec<&str> = reply.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["notes-unit"],
        "the EDA rescan must register the promoted class from templates/ — \
         the staging draft and the instantiated tree carry the same name but \
         are not library entries"
    );
    assert!(
        reply.entries[0]
            .filesystem_path
            .ends_with("templates/notes-unit"),
        "registered path must be the library copy, got {:?}",
        reply.entries[0].filesystem_path
    );

    let (s_ack_tx, s_ack_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::Shutdown { ack: s_ack_tx })
        .await
        .unwrap();
    s_ack_rx.await.unwrap();
    join.await.unwrap();
}
