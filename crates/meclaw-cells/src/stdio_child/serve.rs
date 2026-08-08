//! The serve loop: the I/O sub-task's endless run over one child process.
//!
//! Wiring inside the dual-task pattern (`meclaw-colony`'s
//! `cell_task_long_running`):
//!
//! ```text
//!   handler ──ChildCommand──►  serve_child  ──ChildEvent──►  handler
//!            (reconfig mpsc)   owns the child  (events mpsc)
//!                                  │
//!                            stdin/stdout pipes
//! ```
//!
//! A correlated answer does NOT travel over the events channel: it goes back
//! through the `oneshot` the handler attached to its command. That is what
//! keeps `handle()` a plain request/response call — an answer routed through
//! `events` could never reach a `handle()` that is being awaited inside the
//! handler's own `select!` arm.

use crate::stdio_child::error::StdioChildError;
use serde_json::Value as JsonValue;
use std::time::Duration;
use tokio::sync::oneshot;

/// Key used to match an answer frame to the request that is waiting for it.
///
/// Stringified on purpose: JSON-RPC ids may be numbers or strings, and the
/// core does not care which — the consumer's correlator normalises.
pub type CorrelationKey = String;

/// Handler → I/O sub-task.
#[derive(Debug)]
pub enum ChildCommand {
    /// Write one frame to the child.
    ///
    /// With `correlate: Some(key)` and `reply: Some(tx)` the answer whose
    /// correlator key equals `key` is delivered through `tx` instead of the
    /// events channel. Fire-and-forget sends leave both `None`.
    Send {
        /// The frame to write.
        line: JsonValue,
        /// Key the answer will carry, if an answer is expected.
        correlate: Option<CorrelationKey>,
        /// Where to deliver that answer.
        reply: Option<oneshot::Sender<Result<JsonValue, StdioChildError>>>,
    },
    /// Wind the child down deliberately (peace path).
    Shutdown,
}

/// I/O sub-task → handler.
#[derive(Debug, Clone)]
pub enum ChildEvent {
    /// A frame nobody was waiting for: a notification, a progress event, any
    /// server push. This is the path the future `harness` cell type lives on.
    Frame(JsonValue),
    /// A stdout line that was not JSON. Kept verbatim, never fatal.
    Malformed {
        /// The raw line, trimmed.
        raw: String,
    },
    /// The child is gone. Carries how it ended.
    Exited(crate::stdio_child::error::ChildExit),
}

/// Timeouts the serve loop applies on its own behalf.
#[derive(Debug, Clone, Copy)]
pub struct ServeConfig {
    /// A-timeout around each stdin write (rule 12). Bounds the loop against a
    /// child that stops draining its stdin.
    pub write_timeout: Duration,
    /// Grace between stdin-close and SIGKILL when shutting the child down.
    pub kill_grace: Duration,
}

/// The I/O sub-task's endless run over one child process.
///
/// Never returns while the cell is alive — the A1′ lifetime contract
/// (`docs/meclaw-overview.md` § Long-running cells: double task) forbids a
/// clean, voluntary return: it would let the outer `select!` over both join
/// handles win on the I/O side and abort a healthy handler. Once the child is
/// gone this function reports `ChildEvent::Exited` and parks forever; deciding
/// what a dead child means is the handler's business, not the core's. That
/// split is what lets `mcp` restart while a future `harness` cell must NOT
/// (its tasks are not idempotent).
///
/// Generic over the channel payloads so a consumer can keep its OWN event and
/// reconfig enums (the `LongRunningCell` trait fixes those types per cell) and
/// still use this loop: `C` only has to convert into a `ChildCommand`, and
/// `ChildEvent` only has to convert into `E`.
pub async fn serve_child<K, C, E>(
    child: crate::stdio_child::spawn::StdioChild,
    correlator: K,
    cfg: ServeConfig,
    mut cmds: tokio::sync::mpsc::Receiver<C>,
    events: tokio::sync::mpsc::Sender<E>,
) where
    K: Fn(&JsonValue) -> Option<CorrelationKey> + Send,
    C: Into<ChildCommand>,
    E: From<ChildEvent>,
{
    // Split the child so the read future and the write future can coexist in
    // one `select!` — two methods on `&mut StdioChild` could not.
    let crate::stdio_child::spawn::StdioChild {
        mut child,
        mut stdin,
        mut stdout,
    } = child;
    let mut pending: std::collections::HashMap<
        CorrelationKey,
        oneshot::Sender<Result<JsonValue, StdioChildError>>,
    > = std::collections::HashMap::new();
    // The loop yields how the child ended, so the post-mortem drain below can
    // report the same fate to late callers.
    let last_exit = loop {
        tokio::select! {
            cmd = cmds.recv() => match cmd.map(Into::into) {
                Some(ChildCommand::Send { line, correlate, reply }) => {
                    let written = tokio::time::timeout(
                        cfg.write_timeout,
                        crate::stdio_child::frame::write_line(&mut stdin, &line),
                    )
                    .await
                    .unwrap_or(Err(StdioChildError::Timeout));
                    match written {
                        // No await between the write and the insert, so the
                        // answer cannot overtake its own pending entry.
                        Ok(()) => {
                            if let (Some(key), Some(tx)) = (correlate, reply) {
                                pending.insert(key, tx);
                            }
                        }
                        Err(e) => {
                            if let Some(tx) = reply {
                                let _ = tx.send(Err(e));
                            } else {
                                tracing::warn!(error = %e.detail(), "stdio child write failed");
                            }
                        }
                    }
                }
                // An explicit shutdown and a dropped command channel mean the
                // same thing: nobody will talk to this child again.
                Some(ChildCommand::Shutdown) | None => {
                    let reassembled = crate::stdio_child::spawn::StdioChild { child, stdin, stdout };
                    let exit = reassembled.terminate(cfg.kill_grace).await;
                    fail_pending(&mut pending, exit);
                    let _ = events.send(E::from(ChildEvent::Exited(exit))).await;
                    break exit;
                }
            },
            frame = crate::stdio_child::frame::next_frame(&mut stdout) => match frame {
                Ok(Some(crate::stdio_child::frame::Frame::Json(v))) => {
                    match correlator(&v).and_then(|k| pending.remove(&k)) {
                        Some(tx) => { let _ = tx.send(Ok(v)); }
                        None => { let _ = events.send(E::from(ChildEvent::Frame(v))).await; }
                    }
                }
                Ok(Some(crate::stdio_child::frame::Frame::Malformed(raw))) => {
                    let _ = events.send(E::from(ChildEvent::Malformed { raw })).await;
                }
                // EOF or a broken pipe: the child is gone. Answer everyone who
                // is still waiting FIRST, then announce the death — the
                // handler must be able to emit its error reply before it acts
                // on the death (ratified liveness slice D1).
                Ok(None) | Err(_) => {
                    let exit = reap(&mut child, cfg.kill_grace).await;
                    fail_pending(&mut pending, exit);
                    let _ = events.send(E::from(ChildEvent::Exited(exit))).await;
                    break exit;
                }
            },
        }
    };

    // A1′: never return. But parking silently would be wrong: the handler is
    // biased towards its mailbox over its event channel, so it can accept one
    // more message BEFORE it has seen `Exited` — and that message's request
    // would then sit in `cmds` until its A-timeout elapsed, turning a known
    // death into a slow `provider_timeout`. So keep draining and answer every
    // late request immediately with the child's fate.
    drain_after_death(&mut cmds, last_exit).await;
}

/// Post-mortem: answer late requests instantly instead of letting them time
/// out, then park once nobody can ask any more.
async fn drain_after_death<C: Into<ChildCommand>>(
    cmds: &mut tokio::sync::mpsc::Receiver<C>,
    exit: crate::stdio_child::error::ChildExit,
) {
    while let Some(cmd) = cmds.recv().await {
        if let ChildCommand::Send {
            reply: Some(tx), ..
        } = cmd.into()
        {
            let _ = tx.send(Err(StdioChildError::ChildGone(exit)));
        }
    }
    std::future::pending::<()>().await;
}

/// Resolve every outstanding request with the child's fate, so no `handle()`
/// is left awaiting a `oneshot` that will never be sent.
fn fail_pending(
    pending: &mut std::collections::HashMap<
        CorrelationKey,
        oneshot::Sender<Result<JsonValue, StdioChildError>>,
    >,
    exit: crate::stdio_child::error::ChildExit,
) {
    for (_, tx) in pending.drain() {
        let _ = tx.send(Err(StdioChildError::ChildGone(exit)));
    }
}

/// Collect the exit status of a child whose stdout already ended. Bounded by
/// the kill grace, then SIGKILL — a child may close stdout and linger.
async fn reap(
    child: &mut tokio::process::Child,
    grace: std::time::Duration,
) -> crate::stdio_child::error::ChildExit {
    use crate::stdio_child::error::ChildExit;
    use crate::stdio_child::spawn::exit_of;
    match tokio::time::timeout(grace, child.wait()).await {
        Ok(Ok(status)) => exit_of(status),
        Ok(Err(_)) => ChildExit::SpawnLost,
        Err(_) => {
            let _ = child.start_kill();
            match child.wait().await {
                Ok(status) => exit_of(status),
                Err(_) => ChildExit::SpawnLost,
            }
        }
    }
}
