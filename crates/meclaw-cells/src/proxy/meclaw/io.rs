//! The I/O half of a `meclaw` proxy: hold the mount, serve what the listener
//! hands over, give the name back last.

use std::sync::Arc;
use tokio::sync::mpsc;

use super::mount::{MeclawIo, PeerEvent, PeerReconfig, mounted_router};
use crate::mount_guard::MountGuard;

/// Register the mount, serve every handed connection, and stay up for the
/// cell's whole life.
///
/// **A1′**: this function does not return while the cell is live. Only the
/// handler closing the reconfig channel ends it; nothing else travels there,
/// because every key of a `meclaw` proxy is immutable.
///
/// The name goes on the table once per life, at the top (ADR-0031). A name
/// another cell holds is reported once as [`PeerEvent::MountFailed`]; the cell
/// stays up and serves nobody, which is an operator's mistake to read, not a
/// reason to tear the cell down.
///
/// The order at the end is `web`'s: the serving task first, the registration
/// LAST, so a connection arriving during the teardown still finds the name and
/// reads `503 surface busy` rather than the API fallback's `404`. There is no
/// shutdown signal to send first: no upgraded socket lives on this mount.
pub async fn run_io(
    io: MeclawIo,
    events_tx: mpsc::Sender<PeerEvent>,
    mut reconfig_rx: mpsc::Receiver<PeerReconfig>,
) {
    let mut io = io;
    io.events_tx = Some(events_tx.clone());
    let surfaces = Arc::clone(&io.surfaces);

    let mut registration: Option<MountGuard> = None;
    let entry = meclaw_colony::SurfaceEntry {
        kind: "proxy",
        cell_path: meclaw_core::Path::new(&io.cell_path),
        // A peer mount answers one POST, never a topic link.
        links: None,
    };
    let handoff = match surfaces.register(&io.mount, entry).await {
        Ok((rx, held)) => {
            registration = Some(MountGuard::new(&surfaces, &io.mount, held));
            Some(rx)
        }
        Err(e) => {
            let _ = events_tx.send(PeerEvent::MountFailed(e.to_string())).await;
            None
        }
    };

    // Every handed connection is served on a task of its own, held in a
    // `JoinSet` inside this `JoinSet`, so all of them end with the cell.
    let mut serving = tokio::task::JoinSet::new();
    if let Some(mut rx) = handoff {
        let io = io.clone();
        serving.spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    handed = rx.recv() => match handed {
                        Some(handed) => {
                            connections.spawn(crate::handed::serve_handed(
                                handed.stream,
                                mounted_router(io.clone()),
                            ));
                        }
                        // A respawn registered over this entry.
                        None => break,
                    },
                    // Reaping; guarded, because `join_next` on an empty set
                    // is `None` at once and would spin.
                    Some(_) = connections.join_next(), if !connections.is_empty() => {}
                }
            }
        });
    }

    // Only the handler going away ends this half (A1′). `PeerReconfig` has no
    // values, so the only thing `recv` can return is `None`.
    if let Some(never) = reconfig_rx.recv().await {
        match never {}
    }
    drop(serving);
    drop(registration);
}
