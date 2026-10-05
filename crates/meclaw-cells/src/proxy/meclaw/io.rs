//! The I/O half of a `meclaw` proxy: hold the mount, serve what the listener
//! hands over, give the name back last.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

use super::book;
use super::mount::{
    InboxWrite, MeclawIo, PeerEvent, PeerReconfig, Wake, arrival_from_json, mounted_router, now_ms,
};
use crate::mount_guard::MountGuard;
use meclaw_colony::DbConn;

/// GH #1012: the I/O half as the cell hands it over: the mount state, plus
/// the receiving end of the handler's wake requests for the outbox.
pub struct PeerIo {
    /// The mount half.
    pub io: MeclawIo,
    /// The handler's wake requests; `None` for a mount nobody sends from.
    pub wakes: Option<mpsc::UnboundedReceiver<Wake>>,
}

/// [`run_peer_io`] for a mount without an outbox clock (tests, and the
/// pre-#1012 call shape).
pub async fn run_io(
    io: MeclawIo,
    events_tx: mpsc::Sender<PeerEvent>,
    reconfig_rx: mpsc::Receiver<PeerReconfig>,
) {
    run_peer_io(PeerIo { io, wakes: None }, events_tx, reconfig_rx).await
}

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
///
/// GH #1012, before the name goes on the table: the book is opened, every
/// inbox row not yet `done` is handed to the handler in arrival order (the
/// replay), and every target with a pending outbox row is due at once. A book
/// that cannot be opened is reported as [`PeerEvent::MountFailed`] and the
/// name is not taken: a mount that cannot keep its promise answers nobody,
/// and the far side retries. After the mount, this half is the outbox clock:
/// it turns the handler's [`Wake`]s into [`PeerEvent::Retry`]s.
pub async fn run_peer_io(
    pio: PeerIo,
    events_tx: mpsc::Sender<PeerEvent>,
    mut reconfig_rx: mpsc::Receiver<PeerReconfig>,
) {
    let PeerIo { mut io, mut wakes } = pio;
    io.events_tx = Some(events_tx.clone());
    let surfaces = Arc::clone(&io.surfaces);

    let mut due: HashMap<String, i64> = HashMap::new();
    let mut writer = None;
    let mut book_ok = true;
    if let Some(path) = io.cell_db.clone() {
        match open_book(path, io.query_timeout_ms).await {
            Ok((db, replay, targets)) => {
                for (key, row) in replay {
                    match arrival_from_json(&key, &row) {
                        Some(ev) => {
                            if events_tx.send(ev).await.is_err() {
                                return;
                            }
                        }
                        None => tracing::warn!(
                            key = %key,
                            "proxy/meclaw: an inbox row this build cannot read stays pending"
                        ),
                    }
                }
                let now = i64::try_from(now_ms()).unwrap_or(i64::MAX);
                for (target, _) in targets {
                    due.insert(target, now);
                }
                let (tx, rx) = mpsc::channel::<InboxWrite>(64);
                io.inbox = Some(tx);
                writer = Some(tokio::spawn(run_inbox(db, rx, events_tx.clone())));
            }
            Err(e) => {
                book_ok = false;
                let _ = events_tx
                    .send(PeerEvent::MountFailed(format!(
                        "the peer book in cell.db could not be opened: {e}"
                    )))
                    .await;
            }
        }
    }

    let mut registration: Option<MountGuard> = None;
    let entry = meclaw_colony::SurfaceEntry {
        kind: "proxy",
        cell_path: meclaw_core::Path::new(&io.cell_path),
        // A peer mount answers one POST, never a topic link.
        links: None,
    };
    // A book that could not be opened takes no name (reported above).
    let registered = if book_ok {
        Some(surfaces.register(&io.mount, entry).await)
    } else {
        None
    };
    let handoff = match registered {
        None => None,
        Some(Ok((rx, held))) => {
            registration = Some(MountGuard::new(&surfaces, &io.mount, held));
            Some(rx)
        }
        Some(Err(e)) => {
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
                            // GH #833: the connection's address decides,
                            // once, whether its identity header counts.
                            connections.spawn(crate::handed::serve_handed(
                                handed.stream,
                                mounted_router(io.for_connection(handed.peer.ip())),
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
    // values, so the only thing `recv` can return is `None`. Until then this
    // half is the outbox clock (GH #1012).
    loop {
        let next = due.values().min().copied();
        let sleep = async move {
            match next {
                Some(at) => {
                    let wait = u64::try_from(at).unwrap_or(0).saturating_sub(now_ms());
                    tokio::time::sleep(Duration::from_millis(wait)).await;
                }
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            r = reconfig_rx.recv() => {
                if let Some(never) = r {
                    match never {}
                }
                break;
            }
            w = next_wake(&mut wakes) => match w {
                Some(Wake { target, at_ms }) => {
                    let at = due.entry(target).or_insert(at_ms);
                    *at = (*at).min(at_ms);
                }
                // The handler's sender is gone: no more wakes, the clock
                // still delivers the ones it holds.
                None => wakes = None,
            },
            () = sleep => {
                let now = i64::try_from(now_ms()).unwrap_or(i64::MAX);
                let fired: Vec<String> = due
                    .iter()
                    .filter(|(_, at)| **at <= now)
                    .map(|(t, _)| t.clone())
                    .collect();
                for target in fired {
                    due.remove(&target);
                    if events_tx.send(PeerEvent::Retry { target }).await.is_err() {
                        break;
                    }
                }
            }
        }
    }
    drop(serving);
    if let Some(w) = writer {
        w.abort();
    }
    drop(registration);
}

/// The next wake, or never when there is no sender.
async fn next_wake(wakes: &mut Option<mpsc::UnboundedReceiver<Wake>>) -> Option<Wake> {
    match wakes {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// What the book holds at the start: the connection the inbox writer keeps,
/// the inbox rows to replay, and the targets with pending outbox rows.
type OpenedBook = (DbConn, Vec<(String, String)>, Vec<(String, i64)>);

/// GH #1012: opens the cell's own `cell.db` (WAL, `synchronous = NORMAL`,
/// busy timeout: `open_or_create_cell_db`), sets up the books and reads what
/// is still open in them. The handler half holds a second connection to the
/// same file; SQLite's WAL serialises the two writers.
async fn open_book(path: std::path::PathBuf, timeout_ms: u64) -> Result<OpenedBook, String> {
    let work = tokio::task::spawn_blocking(move || -> rusqlite::Result<_> {
        let conn = meclaw_colony::persist::cell_db::open_or_create_cell_db(&path)?;
        book::setup_peer_book(&conn)?;
        let replay = book::inbox_pending(&conn)?;
        let targets = book::outbox_targets(&conn)?;
        Ok((conn, replay, targets))
    });
    match tokio::time::timeout(Duration::from_millis(timeout_ms), work).await {
        Ok(Ok(Ok((conn, replay, targets)))) => Ok((
            DbConn::wrap(conn, Some(Duration::from_millis(timeout_ms))),
            replay,
            targets,
        )),
        Ok(Ok(Err(e))) => Err(e.to_string()),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(format!("not open within {timeout_ms} ms")),
    }
}

/// GH #1012: the one inbox writer of this life. Each write is committed (or
/// refused) under the `query_timeout_ms` A-timeout; a new row's arrival is
/// handed to the handler half, and only then does the answer go back. The
/// mount answers `200` only on `Ok`.
///
/// Review C2: commit and hand-on happen HERE, on this task, not in the
/// request future. hyper drops the future of a request whose client went
/// away (the sender's own `peer_timeout` while the handler's channel is
/// full); with the hand-on in that future the row stayed committed and
/// `pending`, nothing was raised until the next start, and every retry read
/// `duplicate`. This task does not care whether anyone still waits for the
/// answer. One task, one queue: arrivals are handed on in commit order.
async fn run_inbox(
    mut db: DbConn,
    mut rx: mpsc::Receiver<InboxWrite>,
    events_tx: mpsc::Sender<PeerEvent>,
) {
    while let Some(w) = rx.recv().await {
        let InboxWrite {
            key,
            frame,
            at_ms,
            event,
            done,
        } = w;
        let res = db
            .call_with_timeout(move |c| book::inbox_insert(c, &key, &frame, at_ms))
            .await;
        let answer = match res {
            Ok(Ok(fresh)) => Ok(fresh),
            Ok(Err(e)) => Err(e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        // A closed channel (the handler is going away) leaves the row
        // `pending`; the next life replays it, so the answer stays `Ok`.
        if matches!(answer, Ok(true)) {
            let _ = events_tx.send(event).await;
        }
        let _ = done.send(answer);
    }
}
