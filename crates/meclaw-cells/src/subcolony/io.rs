//! The I/O sub-task: one child colony process for the life of the cell.
//!
//! The shape is a line, not a cycle — the opposite of `harness`:
//!
//! ```text
//!   spawn → ready handshake → serve forever → (child dies) → park
//! ```
//!
//! A child colony is not one unit of work, it IS the cell's ability to answer.
//! Its death therefore ends the connection rather than completing a task, and
//! the handler turns that into the ordinary supervisor path (`mcp`'s decision,
//! not `harness`'s).
//!
//! `run_io` never returns while the cell is alive (A1′). Every failure path below
//! reports and then parks; returning would let the outer `select!` win on the I/O
//! side and abort a perfectly healthy handler.

use crate::stdio_child::{ChildCommand, ChildEvent, ChildSpec, ServeConfig, StdioChild};
use crate::subcolony::params::SubcolonyParams;
use crate::subcolony::wire;
use std::future::Future;
use std::time::Duration;
use tokio::sync::mpsc;

/// Handler → I/O. Everything this cell says to its child is a child command;
/// there is no "start a task" verb, because there is nothing to start.
#[derive(Debug)]
pub enum SubcolonyReconfig {
    /// A frame for the running child.
    Child(ChildCommand),
}

impl TryFrom<SubcolonyReconfig> for ChildCommand {
    type Error = ();
    fn try_from(r: SubcolonyReconfig) -> Result<Self, ()> {
        match r {
            SubcolonyReconfig::Child(c) => Ok(c),
        }
    }
}

/// I/O → handler.
#[derive(Debug, Clone)]
pub enum SubcolonyEvent {
    /// The child booted and speaks our protocol. Carries its release version,
    /// which is reported and never asserted.
    Ready {
        /// The child's `CARGO_PKG_VERSION`, as it announced it.
        version: String,
    },
    /// The child could not be brought up. Deterministic: a restart would repeat
    /// it, so the cell stays up and refuses instead.
    BootFailed {
        /// One of the cell's closed error-code set.
        error_code: &'static str,
        /// What went wrong.
        detail: String,
    },
    /// Something happened on the running child's connection.
    Child(ChildEvent),
}

impl From<ChildEvent> for SubcolonyEvent {
    fn from(e: ChildEvent) -> Self {
        SubcolonyEvent::Child(e)
    }
}

/// The I/O sub-task's birth state: the configuration needed to start the child.
#[derive(Debug)]
pub struct SubcolonyIo {
    params: SubcolonyParams,
}

impl SubcolonyIo {
    /// Build the I/O state from the cell's params.
    pub fn new(params: SubcolonyParams) -> Self {
        Self { params }
    }
}

/// How to start the child colony.
///
/// `--stdio-format json` is set by the facade and is not configurable: the wire
/// is part of the boundary, not a tuning knob. `--daemon` and `--api` are never
/// set — the child must die when its stdin closes, which is how the cell stops
/// it, and a child colony has no business opening a port.
fn child_spec(params: &SubcolonyParams) -> ChildSpec {
    let mut env = params.env.clone();
    for key in &params.env_passthrough {
        if let Ok(value) = std::env::var(key) {
            env.push((key.clone(), value));
        }
    }
    let mut args = vec![
        "--root".to_string(),
        params.root.display().to_string(),
        "--stdio-format".to_string(),
        "json".to_string(),
    ];
    args.extend(params.extra_args.iter().cloned());
    ChildSpec {
        program: params.command.clone(),
        args,
        env,
        cwd: Some(params.root.clone()),
        kill_grace_ms: params.kill_grace_ms,
        // A child colony spawns cells that spawn processes of their own.
        process_group: true,
        // Secret isolation, the deliberate plus of a process boundary: the child
        // sees the passthrough list and nothing else of this colony's env.
        env_clear: true,
        // A child colony is a colony, not a cell running foreign code: it has
        // no `params.sandbox` of its own and its cells carry their own
        // profiles. GH #85 deliberately leaves this untouched.
        sandbox: None,
    }
}

/// The child spec, exposed for the configuration pin in the lifecycle tests.
///
/// What it asserts is the containment configuration, not the mechanism: the
/// process-group reaping itself is proven with a negative control in
/// `stdio_child_reaping.rs`, and duplicating that here would test P7 twice.
pub fn child_spec_for_test(params: &SubcolonyParams) -> ChildSpec {
    child_spec(params)
}

/// I/O sub-task. Never returns voluntarily (A1′).
#[allow(clippy::manual_async_fn)]
pub fn run_io(
    io: SubcolonyIo,
    events_tx: mpsc::Sender<SubcolonyEvent>,
    reconfig_rx: mpsc::Receiver<SubcolonyReconfig>,
) -> impl Future<Output = ()> + Send {
    async move {
        let params = io.params;
        let events_tx = events_tx;
        let mut reconfig_rx = reconfig_rx;

        let serve = ServeConfig {
            write_timeout: Duration::from_millis(params.external_timeout_ms),
            kill_grace: Duration::from_millis(params.kill_grace_ms),
        };

        let mut child = match StdioChild::spawn(&child_spec(&params)) {
            Ok(c) => c,
            Err(e) => {
                return boot_failed(
                    &events_tx,
                    &mut reconfig_rx,
                    "subcolony_unavailable",
                    e.detail(),
                )
                .await;
            }
        };

        // A-timeout (rule 12) on the one bounded operation of a boot: saying
        // hello. A colony that has not finished its filesystem bootstrap within
        // the budget is not going to.
        let hello = tokio::time::timeout(
            Duration::from_millis(params.boot_timeout_ms),
            child.read_frame(),
        )
        .await;
        let frame = match hello {
            Err(_) => {
                let exit = child.terminate(serve.kill_grace).await;
                return boot_failed(
                    &events_tx,
                    &mut reconfig_rx,
                    "boot_timeout",
                    format!("no ready frame within the boot budget ({})", exit.detail()),
                )
                .await;
            }
            Ok(Ok(Some(crate::stdio_child::Frame::Json(v)))) => v,
            Ok(other) => {
                let detail = match other {
                    Ok(Some(crate::stdio_child::Frame::Malformed(raw))) => {
                        format!("first line was not JSON: {raw}")
                    }
                    Ok(None) => "the child ended before saying anything".to_string(),
                    Err(e) => e.detail(),
                    Ok(Some(_)) => unreachable!("json frame handled above"),
                };
                let exit = child.terminate(serve.kill_grace).await;
                return boot_failed(
                    &events_tx,
                    &mut reconfig_rx,
                    "subcolony_unavailable",
                    format!("{detail} ({})", exit.detail()),
                )
                .await;
            }
        };

        // Strict protocol assert (D4). Deterministic — a restart cannot fix a
        // child that speaks another wire, so the cell refuses rather than
        // burning its restart budget on a certainty.
        if let Err(e) = wire::assert_protocol(&frame) {
            let exit = child.terminate(serve.kill_grace).await;
            return boot_failed(
                &events_tx,
                &mut reconfig_rx,
                "protocol_mismatch",
                format!("{} ({})", e.detail, exit.detail()),
            )
            .await;
        }

        let version = frame
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        if events_tx
            .send(SubcolonyEvent::Ready { version })
            .await
            .is_err()
        {
            let _ = child.terminate(serve.kill_grace).await;
            return std::future::pending::<()>().await;
        }

        // Serve until the child is gone, then keep answering late callers with
        // its fate instead of letting them time out, then park. All three are
        // `serve_child`'s epilogue — the `mcp` shape, not `harness`'s.
        crate::stdio_child::serve::serve_child(
            child,
            wire::correlation_key,
            serve,
            reconfig_rx,
            events_tx,
        )
        .await;
        std::future::pending::<()>().await
    }
}

/// Report a boot that did not happen, then answer every request with the same
/// fate forever.
///
/// The draining is what makes the failure fast rather than merely eventual: a
/// request that arrived while the boot was still running would otherwise sit in
/// the channel until its own A-timeout elapsed.
async fn boot_failed(
    events_tx: &mpsc::Sender<SubcolonyEvent>,
    cmds: &mut mpsc::Receiver<SubcolonyReconfig>,
    error_code: &'static str,
    detail: impl Into<String>,
) {
    let detail = detail.into();
    let _ = events_tx
        .send(SubcolonyEvent::BootFailed {
            error_code,
            detail: detail.clone(),
        })
        .await;
    while let Some(SubcolonyReconfig::Child(cmd)) = cmds.recv().await {
        if let ChildCommand::Send {
            reply: Some(tx), ..
        } = cmd
        {
            let _ = tx.send(Err(crate::stdio_child::StdioChildError::Spawn(
                detail.clone(),
            )));
        }
    }
    std::future::pending::<()>().await
}
