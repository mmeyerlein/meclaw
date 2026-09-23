//! GH #765 — the curator runs one message at a time (owner ruling 2026-09-19).
//!
//! WHY. There is one screen state and the rendering is a function of it, so the
//! curator is meant to work in event order. Until here the compose cell was a
//! `code` cell that declared no `params.max_concurrency`, and the default of the
//! stateless dispatcher is four (`crates/meclaw-cells/src/code/params.rs`,
//! `effective_max_concurrency`). Four workers read, compute and write the same
//! row, which is why #744 had to make the state write a compare-and-set with a
//! repeat cap of sixteen, and why #745 measured it: over 39 minutes of the
//! acceptance runs on the twin, 190 of 401 state writes came back
//! `rows_affected 0` and were repeated, and fourteen passes ran out of repeats
//! and lost their event
//! (`plans/welle-h3-2026-09-18/berichte/taps-report.md` § 21).
//!
//! WHAT IS LOCKED HERE, in two halves, because the declaration and the effect
//! are two different claims:
//!
//!   1. the template says it — `params.max_concurrency` is 1 on the compose
//!      cell, and it survives the `resident`/`warm` reading of
//!      `CodeParams::effective_max_concurrency`;
//!   2. the substrate does it — a `code` cell spawned with the curator's own
//!      runner params handles its messages one at a time and in the order they
//!      arrived. The stateless dispatcher takes the permit BEFORE it spawns the
//!      worker (`crates/meclaw-colony/src/cell_task.rs`,
//!      `stateless_dispatcher`), so a permit of one is a queue in mailbox order
//!      rather than four workers racing; and the warm pool is sized by the same
//!      number, so there is exactly one child.
//!
//! A PASS AND A MESSAGE: until display 2.7.0 a PASS was two messages of this
//! cell with a store round trip between them, and serialising the messages did
//! not remove the overlap across that trip (measured on the twin: the same
//! refused compare-and-set writes with four workers and with one,
//! `plans/welle-p-2026-09-19/berichte/g0-t1.md`). Since GH #809 the curator runs
//! `resident` and keeps its state in memory, so a pass IS one message and the
//! order locked here is the order of the passes; `809_ten_taps_land_in_order.rs`
//! holds what that means for a finger.
//!
//! Skips when the templates do not ship (R2b).

use meclaw_cells::code::{CodeCellFactory, CodeParams};
use meclaw_colony::{CellFactory, SpawnedCellKind};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, MessageBuilder, Path};
use std::sync::Arc;

/// The repository root, the way every display lock resolves it.
fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// Whether the template library travels in this tree (it does not in the published one).
fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

/// The compose cell's `params` block as the template ships it.
fn compose_params() -> Value {
    let raw = std::fs::read_to_string(repo("templates/display/compose/config.json"))
        .expect("the curator's config travels with the template");
    let cfg: Value = meclaw_core::serde_json::from_str(&raw).expect("a config is JSON");
    cfg["params"].clone()
}

/// The template declares the serialisation, and the code cell reads it as one.
#[test]
fn the_curator_declares_one_message_at_a_time() {
    if !library_ships() {
        return;
    }
    let params = compose_params();
    assert_eq!(
        params["max_concurrency"],
        json!(1),
        "the compose cell has to NAME the number: an absent `max_concurrency` is four \
         workers computing the one state row in the same instant \
         (`effective_max_concurrency`), and the owner's ruling is that the curator works \
         in event order"
    );
    let parsed = CodeParams::parse(&params).expect("the curator's params parse");
    assert_eq!(
        parsed.effective_max_concurrency(),
        1,
        "and the value the factory hands to the dispatcher's semaphore and to the warm \
         pool is that same one"
    );
}

/// Four messages into a cell carrying the curator's runner params: four runs, no two of
/// them at once, and in the order they arrived.
///
/// The script is not the curator — what is under test is the substrate around it, so the
/// body is the smallest one that can answer the two questions. It takes an exclusive
/// marker file and keeps it for the length of its run: a second run that finds the marker
/// standing says so, and that is an overlap no state row could survive. Beside it every
/// run writes its own number, so the file reads as the order the messages were handled in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_curators_runner_params_handle_one_message_at_a_time() {
    if !library_ships() {
        return;
    }
    let template = compose_params();
    let stage = tempfile::TempDir::new().expect("a stage for the marker");
    let script = format!(
        "import json, os, sys, time\n\
         d = json.load(sys.stdin)\n\
         here = {dir:?}\n\
         busy = os.path.join(here, 'busy')\n\
         try:\n\
         \x20   os.close(os.open(busy, os.O_CREAT | os.O_EXCL | os.O_WRONLY))\n\
         \x20   overlap = 0\n\
         except OSError:\n\
         \x20   overlap = 1\n\
         with open(os.path.join(here, 'order'), 'a') as fh:\n\
         \x20   fh.write('%s %s\\n' % (d['body'].get('n'), overlap))\n\
         time.sleep(0.2)\n\
         if not overlap:\n\
         \x20   os.unlink(busy)\n\
         sys.stdout.write(json.dumps({{'messages': []}}))\n",
        dir = stage.path().to_str().expect("a utf-8 stage")
    );
    // The curator's own params, with two things taken out of the way: the script,
    // because what is under test is the substrate around it and not the pass, and
    // the sandbox, because a marker file on a stage of this test's own is not the
    // filesystem the shipped profile is about. `max_concurrency` is NOT named here
    // -- it travels as the template has it, so an ABSENT key is the pre-#765 tree
    // and fails this case exactly as a wrong one does.
    let mut raw = template.clone();
    let obj = raw.as_object_mut().expect("params are an object");
    obj.insert("script_inline".into(), json!(script));
    obj.insert("external_timeout_ms".into(), json!(20000));
    obj.remove("sandbox");

    let (otx, mut orx) = tokio::sync::mpsc::channel(16);
    let cell_dir = tempfile::TempDir::new().expect("a cell dir");
    let (itx, _irx) = tokio::sync::mpsc::channel(8);
    let spawned = Arc::new(CodeCellFactory)
        .spawn_cell(
            Path::new("/compose"),
            raw,
            otx,
            cell_dir.path().to_path_buf(),
            meclaw_colony::ContractView::default(),
            itx,
            None,
            0,
            None,
            None,
            64,
        )
        .expect("the curator's runner params spawn a cell");
    let sender = match spawned {
        SpawnedCellKind::Active { sender, .. } => sender,
        SpawnedCellKind::Dormant { .. } => unreachable!("code spawns Active"),
    };

    // Four at once into the mailbox: the queue is the point, so nothing waits for
    // an answer before the next message goes in.
    const EVENTS: usize = 4;
    for n in 0..EVENTS {
        sender
            .send(
                MessageBuilder::new(Path::new("/compose"))
                    .body(Body::Inline(json!({"messages": [], "n": n})))
                    .reply_to(Path::new("/sink"))
                    .build(),
            )
            .await
            .expect("the mailbox takes the event");
    }
    for n in 0..EVENTS {
        let em = tokio::time::timeout(std::time::Duration::from_secs(30), orx.recv())
            .await
            .unwrap_or_else(|_| panic!("event {n} was answered"))
            .expect("the cell answers every event");
        assert_eq!(
            em.content["header"]["exit_code"], 0,
            "event {n} ran: {:?}",
            em.content
        );
    }

    let order = std::fs::read_to_string(stage.path().join("order")).expect("the runs wrote");
    let runs: Vec<(&str, &str)> = order
        .lines()
        .filter_map(|line| line.split_once(' '))
        .collect();
    assert_eq!(
        runs.len(),
        EVENTS,
        "every event ran exactly once: {order:?}"
    );
    assert!(
        runs.iter().all(|(_, overlap)| *overlap == "0"),
        "two runs of the curator stood in the same moment, and both of them compute the \
         one state row of the screen: {order:?}"
    );
    assert_eq!(
        runs.iter().map(|(n, _)| *n).collect::<Vec<&str>>(),
        (0..EVENTS).map(|n| n.to_string()).collect::<Vec<String>>(),
        "and they ran in the order the events arrived: {order:?}"
    );
}
