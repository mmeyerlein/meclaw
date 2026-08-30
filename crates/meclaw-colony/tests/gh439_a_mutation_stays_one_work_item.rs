//! GH #439 — the yield is a courtesy to the RUNTIME, not an opening for the
//! LOOP. A mutation is applied inside one select-arm await, and nothing the
//! colony would route may slip between two of its cells.
//!
//! Why this is worth a lock rather than a comment: the registry is mutated
//! INCREMENTALLY (`registry.insert` per cell in the spawn step, edges and hive
//! scopes even earlier), there is no snapshot swap, and two RUNTIME conditions
//! after the spawn step can still reject the mutation and take every one of
//! those registrations back (`rollback_registered_nodes`). A loop that routed
//! in between would deliver into a half-built subtree it may have to un-build,
//! and a message delivered to a cell that is then rolled back is simply gone.
//!
//! Until this file existed the invariant was prose in `colony.rs`. GH #439 adds
//! a `yield_now().await` per cell, which is exactly the change a later agent
//! would be tempted to widen into "and then the loop can route in between".
//! This is the wall.

use meclaw_colony::{CellFactory, ColonyMsg, MutationOutcome};
use meclaw_core::{Message, Path, Uuid, serde_json::json};
use meclaw_testing::topologies::phase_3b::CaptureCell;
use meclaw_testing::{ColonyHandle, MessageBuilder};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

const LEAVES: usize = 16;
const PROBES: u32 = 32;

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "echo".to_string(),
        Arc::new(meclaw_testing::factories::echo::EchoCellFactory) as Arc<dyn CellFactory>,
    )]
}

fn setup(dir: &std::path::Path) {
    let root_cell = dir.join("main");
    std::fs::create_dir_all(&root_cell).unwrap();
    std::fs::write(
        root_cell.join("config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    let tpl = dir.join("templates").join("big");
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), r#"{"name":"big"}"#).unwrap();
    std::fs::write(
        tpl.join("config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    for i in 0..LEAVES {
        let leaf = tpl.join(format!("c{i}"));
        std::fs::create_dir_all(&leaf).unwrap();
        std::fs::write(
            leaf.join("config.json"),
            r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/main/sink"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        )
        .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_is_delivered_while_a_subtree_is_being_instantiated() {
    let td = tempfile::TempDir::new().unwrap();
    setup(td.path());
    let h = ColonyHandle::new_with_factories_at(&td, factories());

    // A live capture cell that records every delivery, and WHEN it happened
    // relative to the mutation's own verdict.
    let (cap_tx, mut cap_rx) = mpsc::channel::<Message>(256);
    let cap_path = Path::new("/cap");
    h.spawn(cap_path.clone(), move || CaptureCell::new(cap_tx.clone()))
        .await;

    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .expect("GH #440: the rescan must not have aborted");

    // Fire the mutation WITHOUT awaiting it, then immediately queue a stream of
    // messages for the capture cell. The mutation is taken from the inbox first
    // (it was sent first); the probes sit behind it while it runs.
    let (mut_ack_tx, mut_ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: json!({
                "scope": "/",
                "ctx": {},
                "diff": {"add_nodes": [{"name": "stack", "template": "big"}]}
            }),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: mut_ack_tx,
        })
        .await
        .unwrap();
    for _ in 0..PROBES {
        h.send(MessageBuilder::new("/cap").build()).await;
    }

    // The observer: ONE task owns both the capture channel and the mutation's
    // verdict, and it reads the verdict in a `biased` arm ahead of the channel —
    // so no delivery can be classified without polling the verdict first.
    //
    // The line used to be drawn across two tasks: an `AtomicBool` flipped by the
    // main task the instant `mut_ack_rx` resolved, read by the observer on every
    // delivery. That hop is unbounded. The colony sends the verdict and returns
    // straight to its loop, where all 32 probes are already queued behind the
    // mutation; under load it routes every one of them before the main task is
    // scheduled to flip the flag. Each delivery then counts as "before", and the
    // test reports an interleaving the substrate never performed. Measured
    // 2026-08-28 with the CPUs saturated: 11 of 30 runs red, every one of them
    // with the same signature (before=32, after=0) — a real interleaving would
    // show a PARTIAL split, since routing resumes mid-staging, not all at once.
    let (before, after, verdict) = {
        let observer = tokio::spawn(async move {
            let mut ack = mut_ack_rx;
            let mut verdict = None;
            let (mut before, mut after) = (0u32, 0u32);
            while verdict.is_none() || before + after < PROBES {
                tokio::select! {
                    biased;
                    r = &mut ack, if verdict.is_none() => verdict = Some(r),
                    m = cap_rx.recv() => match m {
                        Some(_) if verdict.is_some() => after += 1,
                        Some(_) => before += 1,
                        None => break,
                    },
                }
            }
            (before, after, verdict)
        });
        // Failure marker, generous per the 30s convention.
        tokio::time::timeout(Duration::from_secs(30), observer)
            .await
            .expect("the probes and the verdict must all land within the marker")
            .expect("the observer task must not panic")
    };

    let outcome = verdict
        .expect("the mutation must answer")
        .expect("an outcome");
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the instantiation must commit: {outcome:?}"
    );

    // Positive half: the probes WERE routable and all of them arrived — so the
    // window under test was real traffic, not an empty channel.
    assert_eq!(
        after, PROBES,
        "every probe must reach the capture cell once the mutation is done \
         (before={before}, after={after})"
    );
    // The invariant: none of them was delivered while the subtree was half
    // built. The discriminator stays tight — a loop that routed between two
    // cells would deliver during a staging pass that takes milliseconds, and the
    // verdict arm above is polled before every single delivery is counted.
    assert_eq!(
        before, 0,
        "no delivery may be interleaved with an instantiation"
    );

    h.shutdown().await;
}
