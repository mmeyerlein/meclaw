//! GH #1060: the credential slot of a STATELESS cell (`web_search`, `code`),
//! kept in a task of its own.
//!
//! # Why a task and not the cell
//!
//! [`crate::credential::CredentialSlots`] is `&mut self` state. A
//! [`meclaw_colony::StatelessCell`] has no `&mut self`: its dispatcher runs up
//! to `max_concurrency` workers at once over one shared `Arc` of the cell
//! (`meclaw-colony/src/cell_task.rs`, `stateless_dispatcher`). Turning the two
//! cells into `StatefulCell`s would make every embed and every search serial
//! (`handle(&mut self)`), and a `Mutex` around the slots is what the
//! concurrency model forbids (`AGENTS.md` § Concurrency). So the slots live
//! where state lives in this codebase: in one task, owned there, reached by
//! message. Workers ask over an `mpsc` and get their answer on a `oneshot`.
//!
//! # Why the dispatcher bound grows while the cell runs on a grant
//!
//! A worker that parks keeps its dispatcher permit while it waits for the box.
//! The box itself is a message to the same cell and needs a permit too. With
//! the dispatcher bound at `max_concurrency`, `max_concurrency` parked calls
//! would hold every permit and the box would sit behind them in the mailbox
//! until the round's deadline refused all of them — an embed cell has a bound
//! of 2, so two calls before the first box were enough. A cell on a grant
//! therefore gives its dispatcher room for every call a round can park plus the
//! box ([`dispatcher_bound`]), and bounds the actual work with `max_concurrency`
//! run tickets the slot task hands out ([`GrantSlot::run_ticket`]), taken only
//! once the credential is in hand. The count lives in the task like the slots
//! do — a `Semaphore` in the cell would be shared state by another name. A
//! parked call holds no run ticket (it is not counted twice) and a released
//! call takes one like every other (it does not bypass the bound). A cell
//! without a grant has no slot, no tickets and the dispatcher
//! bound it always had.
//!
//! # What never leaves the task
//!
//! The recipient key pair. The opened secret leaves it once per call, as a
//! [`CallSecret`] (no `Display`, `Debug` = `<sealed>`), for exactly the one
//! request header or environment entry that call needs.

use crate::credential::{
    CREDENTIAL_PENDING_DETAIL, CredentialParams, CredentialSlots, ExpiryCause, ExpiryFn,
    ParkOutcome,
};
use meclaw_core::serde_json::Value;
use meclaw_core::{CellOutput, Message, OutputSink, Path};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

/// The opened credential of one call. A copy of the slot's secret, dropped
/// with the call.
pub(crate) struct CallSecret(String);

impl CallSecret {
    /// The value, for the one place that needs it. A loud name rather than
    /// `Deref`, so every use of the value is a visible line.
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for CallSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<sealed>")
    }
}

/// What a parked caller is eventually told: the secret, or the detail of its
/// receipt.
type Outcome = Result<CallSecret, String>;
type Waiter = oneshot::Sender<Outcome>;

enum Cmd {
    Need {
        reply: oneshot::Sender<Need>,
    },
    Deliver {
        content: Value,
        reply: oneshot::Sender<Delivered>,
    },
    Run {
        reply: oneshot::Sender<RunTicket>,
    },
    Done,
}

/// One of the cell's `max_concurrency` run slots. Dropping it hands the slot
/// back (a `Done` to the task — an unbounded send, so a drop never waits and
/// never fails while the task lives).
pub(crate) struct RunTicket {
    tx: mpsc::UnboundedSender<Cmd>,
}

impl Drop for RunTicket {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Done);
    }
}

enum Need {
    Ready(CallSecret),
    /// This caller opened the round: it emits the request, then waits.
    Ask {
        request: Value,
        wait: oneshot::Receiver<Outcome>,
    },
    Wait(oneshot::Receiver<Outcome>),
    /// The round is full (`credential_wait_max`).
    Full,
}

/// What became of a sealed delivery.
pub(crate) enum Delivered {
    /// The box opened; every parked call of the round runs now.
    Opened,
    /// The box was refused; the detail is the GH #421 wording. The parked calls
    /// of the round (if any) got their receipt already.
    Refused(String),
    /// A box of an earlier round that was re-asked. Not the sender's fault and
    /// not this round's: nothing is answered, the round keeps waiting.
    Late,
}

/// The dispatcher bound of a stateless cell (see the module note): unchanged
/// without a grant; with one, room for every call a round can park plus the box.
#[must_use]
pub(crate) fn dispatcher_bound(max_concurrency: usize, params: &CredentialParams) -> usize {
    match params.grant() {
        None => max_concurrency,
        Some(_) => max_concurrency
            .saturating_add(params.credential_wait_max.max(1))
            .saturating_add(1),
    }
}

/// The grant block of a cell's params (`credential_grant_id`,
/// `credential_wait_max`, `credential_wait_ms` — the `llm` cell's names and
/// defaults). The serde form of [`CredentialParams`] reads only its three keys;
/// the refusal names the block, never a value.
pub(crate) fn parse_credential_params(raw: &Value) -> Result<CredentialParams, String> {
    meclaw_core::serde_json::from_value(raw.clone())
        .map_err(|e| format!("params.credential_*: {e}"))
}

/// The body of a message if it is a sealed delivery (`{"sealed": …}` in an
/// inline body) — the same test the `llm` cell makes.
#[must_use]
pub(crate) fn sealed_delivery(msg: &Message) -> Option<Value> {
    match &msg.body {
        meclaw_core::Body::Inline(v) if v.get("sealed").is_some() => Some(v.clone()),
        _ => None,
    }
}

/// One grant, its slot task and the run gate of the cell that spends it.
pub(crate) struct GrantSlot {
    grant: String,
    tx: mpsc::UnboundedSender<Cmd>,
}

impl GrantSlot {
    /// `None` without a grant. Spawns the slot task, so it must run inside a
    /// tokio runtime (the factories' `spawn_cell` does). `label` names the cell
    /// type in the log lines the task writes.
    #[must_use]
    pub(crate) fn spawn(
        params: &CredentialParams,
        max_concurrency: usize,
        label: &'static str,
    ) -> Option<Self> {
        let grant = params.grant()?.to_string();
        let on_expired: ExpiryFn<Waiter> =
            Arc::new(move |waiters: Vec<Waiter>, cause: ExpiryCause| {
                Box::pin(async move {
                    if let ExpiryCause::Deadline { wait_ms } = cause {
                        tracing::warn!(
                            cell = label,
                            wait_ms,
                            "the sealed credential did not arrive in time"
                        );
                    }
                    for w in waiters {
                        let _ = w.send(Err(CREDENTIAL_PENDING_DETAIL.to_string()));
                    }
                })
            });
        let slots = CredentialSlots::from_params(params, on_expired);
        // Unbounded: every sender is a worker that holds a dispatcher permit
        // (or a ticket being returned), so the queue is bounded by the
        // dispatcher, and a ticket's drop must never wait.
        let (tx, rx) = mpsc::unbounded_channel();
        let runs = Runs {
            limit: max_concurrency.max(1),
            running: 0,
            waiting: std::collections::VecDeque::new(),
            tx: tx.downgrade(),
        };
        tokio::spawn(slot_task(rx, slots, runs, grant.clone(), label));
        Some(Self { grant, tx })
    }

    /// The grant this slot spends.
    #[must_use]
    pub(crate) fn grant(&self) -> &str {
        &self.grant
    }

    /// The credential for one call. Opens a round when there is none: the
    /// request goes out through THIS call's sink to `ask_target` (the reply
    /// target, like every emission of the cell — the edge matching
    /// `hop.route == "credential_request"` decides where it goes), and the call
    /// waits for the box. `Err` carries the receipt's detail.
    pub(crate) async fn credential(
        &self,
        sink: &OutputSink,
        ask_target: &Path,
    ) -> Result<CallSecret, String> {
        let (reply, answer) = oneshot::channel();
        let gone = || CREDENTIAL_PENDING_DETAIL.to_string();
        self.tx.send(Cmd::Need { reply }).map_err(|_| gone())?;
        let wait = match answer.await.map_err(|_| gone())? {
            Need::Ready(secret) => return Ok(secret),
            Need::Full => return Err(gone()),
            Need::Wait(wait) => wait,
            Need::Ask { request, wait } => {
                let _ = sink
                    .push(CellOutput {
                        target: ask_target.clone(),
                        content: request,
                    })
                    .await;
                wait
            }
        };
        wait.await.map_err(|_| gone())?
    }

    /// Hand a sealed delivery to the slot task.
    pub(crate) async fn deliver(&self, content: Value) -> Delivered {
        let (reply, answer) = oneshot::channel();
        let gone = || Delivered::Refused(CREDENTIAL_PENDING_DETAIL.to_string());
        if self.tx.send(Cmd::Deliver { content, reply }).is_err() {
            return gone();
        }
        answer.await.unwrap_or_else(|_| gone())
    }

    /// One of the cell's `max_concurrency` run tickets — taken after the
    /// credential is in hand, held for the call's work. `None` only when the
    /// slot task is gone (the cell is going down); the caller then runs
    /// unticketed rather than hang.
    pub(crate) async fn run_ticket(&self) -> Option<RunTicket> {
        let (reply, answer) = oneshot::channel();
        self.tx.send(Cmd::Run { reply }).ok()?;
        answer.await.ok()
    }
}

/// The run count of the cell, owned by the slot task.
struct Runs {
    limit: usize,
    running: usize,
    waiting: std::collections::VecDeque<oneshot::Sender<RunTicket>>,
    /// Weak, so the task does not keep its own mailbox open: it ends when the
    /// cell and every ticket are gone.
    tx: mpsc::WeakUnboundedSender<Cmd>,
}

impl Runs {
    fn hand_out(&mut self) {
        while self.running < self.limit {
            let Some(reply) = self.waiting.pop_front() else {
                return;
            };
            let Some(tx) = self.tx.upgrade() else {
                return;
            };
            self.running += 1;
            // A caller that gave up drops the ticket inside the `Err`, and its
            // drop sends the `Done` that gives the slot back.
            let _ = reply.send(RunTicket { tx });
        }
    }
}

async fn slot_task(
    mut rx: mpsc::UnboundedReceiver<Cmd>,
    mut slots: CredentialSlots<Waiter>,
    mut runs: Runs,
    grant: String,
    label: &'static str,
) {
    while let Some(cmd) = rx.recv().await {
        match cmd {
            Cmd::Run { reply } => {
                runs.waiting.push_back(reply);
                runs.hand_out();
                // GH #1099 made a wait over `max_concurrency` visible in the
                // dispatcher; a granted cell waits here instead (GH #1060), so
                // the wait is logged the moment it starts, keyed by the grant
                // (re-check R K7, tv-br M10q-b).
                if !runs.waiting.is_empty() {
                    tracing::info!(
                        target: "meclaw::concurrency",
                        cell = label,
                        grant = %grant,
                        max_concurrency = runs.limit as u64,
                        waiting = runs.waiting.len() as u64,
                        "a call waits for a free run ticket"
                    );
                }
            }
            Cmd::Done => {
                runs.running = runs.running.saturating_sub(1);
                runs.hand_out();
            }
            Cmd::Need { reply } => {
                if let Some(s) = slots.secret(&grant) {
                    let _ = reply.send(Need::Ready(CallSecret(s.to_string())));
                    continue;
                }
                let (waiter, wait) = oneshot::channel();
                let answer = match slots.park(&grant, waiter) {
                    ParkOutcome::Asked => match slots.request(&grant) {
                        Ok(request) => Need::Ask { request, wait },
                        Err(e) => {
                            // The round stays open and runs into its deadline,
                            // as the `llm` cell's does.
                            tracing::error!(cell = label, error = %e, "no random source for a credential request");
                            Need::Wait(wait)
                        }
                    },
                    ParkOutcome::Parked => Need::Wait(wait),
                    ParkOutcome::Refused(_) => Need::Full,
                };
                let _ = reply.send(answer);
            }
            Cmd::Deliver { content, reply } => {
                let delivered = match slots.accept_sealed(&content).await {
                    Ok(accepted) => {
                        tracing::info!(
                            cell = label,
                            "credential received sealed and opened in RAM"
                        );
                        let secret = slots.secret(&accepted.grant).map(str::to_string);
                        for w in accepted.released {
                            let _ = w.send(match &secret {
                                Some(s) => Ok(CallSecret(s.clone())),
                                None => Err(CREDENTIAL_PENDING_DETAIL.to_string()),
                            });
                        }
                        Delivered::Opened
                    }
                    Err(refusal) => match refusal.detail() {
                        None => {
                            tracing::warn!(
                                cell = label,
                                "a sealed box of an earlier credential round arrived late and was discarded"
                            );
                            Delivered::Late
                        }
                        Some(detail) => {
                            tracing::warn!(cell = label, detail = %detail, "a sealed credential delivery was refused");
                            for w in refusal.into_refused() {
                                let _ = w.send(Err(detail.clone()));
                            }
                            Delivered::Refused(detail)
                        }
                    },
                };
                let _ = reply.send(delivered);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dispatcher_bound_is_unchanged_without_a_grant() {
        let p = CredentialParams::default();
        assert_eq!(dispatcher_bound(4, &p), 4);
        let p = CredentialParams {
            credential_grant_id: Some(String::new()),
            ..CredentialParams::default()
        };
        assert_eq!(dispatcher_bound(2, &p), 2, "an empty grant is no grant");
    }

    #[test]
    fn a_grant_makes_room_for_the_parked_round_and_the_box() {
        let p = CredentialParams {
            credential_grant_id: Some("g-1".into()),
            credential_wait_max: 16,
            ..CredentialParams::default()
        };
        assert_eq!(dispatcher_bound(2, &p), 2 + 16 + 1);
    }

    /// The red runs of the arithmetic lock below. `RED_RUN_WAIT_MAX = Some(16)`
    /// (the substrate default) makes it fail with `message_timeout` -- 17
    /// ticket waiters behind 2 tickets, the last one waits ceil(17 / 2) = 9
    /// calls plus its own against the shipped budget. With the shipped value
    /// (2) every call fits.
    const RED_RUN_WAIT_MAX: Option<usize> = None;

    /// `RED_RUN_MESSAGE_TIMEOUT_MS = Some(90_000)`, the `message_timeout` of
    /// `file-space/embed` before 034d4bd50, makes the lock fail as well: the
    /// last of 3 waiters behind 2 tickets ends 2 × 42 + 42 = 126 s after its
    /// admission. With T = `params.timeout_ms` the lock reckoned 2 × 20 + 20 =
    /// 60 s and stayed green on that budget (re-check tv-br M6, C-M1).
    const RED_RUN_MESSAGE_TIMEOUT_MS: Option<u64> = None;

    /// `(max_concurrency, credential_wait_max, cell.message_timeout, T)` of the
    /// shipped `file-space/embed`, in ms. `T` is how long one call holds its
    /// run ticket: a `code` cell holds it "until the script has answered"
    /// (`code/cell.rs`), so for the A-timeout of the whole run,
    /// `params.external_timeout_ms` (`code/pool.rs` `Job::timeout`), plus the
    /// `params.attachment_timeout_ms` of the reads under the ticket where a
    /// cell declares it -- not `params.timeout_ms`, which bounds one provider
    /// attempt inside that run (the embed script tries twice within it).
    fn shipped_file_space_embed() -> (usize, usize, u64, u64) {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../templates/file-space/embed/config.json");
        let cfg: Value = meclaw_core::serde_json::from_str(
            &std::fs::read_to_string(&path).expect("file-space/embed config"),
        )
        .expect("config json");
        let p = &cfg["params"];
        let n = |v: &Value, what: &str| v.as_u64().unwrap_or_else(|| panic!("{what}"));
        (
            usize::try_from(n(&p["max_concurrency"], "params.max_concurrency")).expect("mc"),
            usize::try_from(n(&p["credential_wait_max"], "params.credential_wait_max"))
                .expect("wait_max"),
            n(&cfg["cell"]["message_timeout"], "cell.message_timeout"),
            n(&p["external_timeout_ms"], "params.external_timeout_ms")
                + p["attachment_timeout_ms"].as_u64().unwrap_or(0),
        )
    }

    /// GH #1061 lock (b), exact half (TV-Z2: a time statement only in virtual
    /// time). The shipped `file-space/embed` values, unscaled, through the
    /// real `stateless_dispatcher` (its bound = [`dispatcher_bound`], its
    /// `message_timeout` backstop) and the real run tickets of the slot task:
    /// a sustained burst of three full dispatcher bounds, every call holds its
    /// ticket for the whole `T` (`external_timeout_ms`, see
    /// [`shipped_file_space_embed`]), and none dies on the backstop --
    /// ceil((wait_max + 1) / mc) × T + T ≤ message_timeout, run instead of
    /// computed. Under the paused clock every wait point is a tokio timer or
    /// channel (TV-Z7), so the verdict is exact and no co-load can move it.
    /// The positive oracle (TV-Q1, re-check tv-br M6 C-M2): every reply is the
    /// cell's own answer, and the last one comes exactly ceil(burst / mc) × T
    /// after the burst -- the mc bound and a FIFO hand-over without idle time,
    /// an exact statement only the paused clock can make; a call that ran
    /// without its ticket ends the burst early.
    /// WHY here and not only in the real-time burst of
    /// `tests/store_1/gh1061_a_granted_embedder_keeps_its_budget_under_load.rs`:
    /// that one ran python children against a wall-clock budget of 4.5 S and
    /// went red 1 in 160 loaded runs (1 in 80 on the owner's host; tv-br M5,
    /// reply 4 of 15, message_timeout 6.95 s) while the quiet takt was 1.12 s
    /// per round.
    #[tokio::test(start_paused = true)]
    async fn gh1061_every_call_of_a_sustained_burst_gets_its_ticket_inside_message_timeout() {
        /// A stateless cell that does what a granted `code` cell does around its
        /// run: take a run ticket, hold it for the whole `T` (the A-timeout of
        /// the run, the worst case a code cell holds its ticket), answer.
        struct TicketCell {
            slot: GrantSlot,
            op: std::time::Duration,
        }

        impl meclaw_colony::StatelessCell for TicketCell {
            async fn handle(&self, _msg: Message, sink: &OutputSink) {
                let _ticket = self
                    .slot
                    .run_ticket()
                    .await
                    .expect("the slot task hands out a run ticket");
                tokio::time::sleep(self.op).await;
                let _ = sink
                    .push(CellOutput {
                        target: Path::new("/sink"),
                        content: meclaw_core::serde_json::json!({"messages": [{"text": "ok"}]}),
                    })
                    .await;
            }
        }

        let (mc, shipped_wait_max, shipped_mt_ms, op_ms) = shipped_file_space_embed();
        let mt_ms = RED_RUN_MESSAGE_TIMEOUT_MS.unwrap_or(shipped_mt_ms);
        let params = CredentialParams {
            credential_grant_id: Some("g-1061".into()),
            credential_wait_max: RED_RUN_WAIT_MAX.unwrap_or(shipped_wait_max),
            ..CredentialParams::default()
        };
        let bound = dispatcher_bound(mc, &params);
        let cell = Arc::new(TicketCell {
            slot: GrantSlot::spawn(&params, mc, "ticket-test").expect("a grant has a slot"),
            op: std::time::Duration::from_millis(op_ms),
        });
        let burst = 3 * bound;
        let (mail_tx, mail_rx) = mpsc::channel(burst + 1);
        let (out_tx, mut out_rx) = mpsc::channel(4 * burst + 16);
        let dispatcher = tokio::spawn(meclaw_colony::stateless_dispatcher(
            Path::new("/c"),
            mail_rx,
            out_tx,
            cell,
            bound,
            Some(std::time::Duration::from_millis(mt_ms)),
            None,
            None,
            None,
            None,
            None,
            None,
        ));
        let t0 = tokio::time::Instant::now();
        for i in 0..burst {
            let msg = meclaw_core::MessageBuilder::new(Path::new("/c"))
                .reply_to(Path::new("/sink"))
                .trace_id(meclaw_core::Uuid::now_v7())
                .body(meclaw_core::Body::Inline(
                    meclaw_core::serde_json::json!({"messages": [{"text": format!("{i}")}]}),
                ))
                .build();
            mail_tx.send(msg).await.expect("the mailbox is open");
        }
        let mut died = Vec::new();
        let mut foreign = Vec::new();
        let mut answers = 0;
        let mut last = t0;
        while answers < burst {
            let e = out_rx
                .recv()
                .await
                .expect("the dispatcher answers every call");
            if e.target.as_str() == meclaw_core::CONSUMED_MARK_TARGET {
                continue;
            }
            last = tokio::time::Instant::now();
            if e.content["header"]["error_code"].as_str() == Some("message_timeout") {
                died.push(answers);
            } else if e.content["header"]["error_code"].is_string()
                || e.content["messages"][0]["text"] != "ok"
            {
                foreign.push(e.content.to_string());
            }
            answers += 1;
        }
        let setting = format!(
            "mc {mc}, credential_wait_max {}, dispatcher bound {bound}, T {op_ms} ms, \
             message_timeout {mt_ms} ms",
            params.credential_wait_max
        );
        assert!(
            died.is_empty(),
            "replies {died:?} of {burst} died on the backstop waiting for a run ticket ({setting})"
        );
        assert!(
            foreign.is_empty(),
            "every reply is the cell's own answer ({setting}): {foreign:?}"
        );
        let rounds = u32::try_from(burst.div_ceil(mc)).expect("rounds");
        assert_eq!(
            last - t0,
            std::time::Duration::from_millis(op_ms) * rounds,
            "the last reply of {burst} comes exactly ceil({burst} / {mc}) = {rounds} ticket holds \
             after the burst -- no call ran beside the {mc} tickets, no hand-over idled ({setting})"
        );
        drop(mail_tx);
        dispatcher.abort();
    }

    #[test]
    fn a_call_secret_never_prints_its_value() {
        let s = CallSecret("stub-secret-1".into());
        assert_eq!(format!("{s:?}"), "<sealed>");
    }

    /// The INFO line of a call that queues for a run ticket (target, message
    /// and the fields a reader keys it by).
    fn queued_lines(grant: &str, waiting: usize) -> Vec<String> {
        meclaw_testing::log_recorder::global().lines(
            tracing::Level::INFO,
            &[
                "a call waits for a free run ticket",
                &format!(" grant={grant}"),
                &format!(" waiting={waiting}"),
            ],
        )
    }

    /// GH #1099 made a wait over `max_concurrency` visible in the dispatcher;
    /// since GH #1060 a granted cell waits for its run ticket in the slot task
    /// instead, and that wait was silent. The slot task logs a call the moment
    /// it queues -- with the grant and how many calls wait -- and never a call
    /// that got its ticket at once (re-check R K7, tv-br M10q-b: the bound
    /// tests of `support/gh1060.rs` read this line as the cell's own sign
    /// that the calls beyond the bound wait, instead of a stub window).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_call_that_waits_for_a_run_ticket_is_logged_when_it_queues() {
        // The recorder first: installed after the first `Run`, the line a
        // regression would log for the at-once ticket is lost and the negative
        // half below checks nothing (re-check R-delta M4, tv-br M10u).
        let _ = meclaw_testing::log_recorder::global();
        let params = CredentialParams {
            credential_grant_id: Some("g-queue-log".into()),
            ..CredentialParams::default()
        };
        let slot =
            Arc::new(GrantSlot::spawn(&params, 1, "ticket-test").expect("a grant has a slot"));
        let held = meclaw_testing::deadline::hang("the first run ticket", slot.run_ticket())
            .await
            .expect("the slot task hands out the first ticket");
        assert!(
            queued_lines("g-queue-log", 0).is_empty() && queued_lines("g-queue-log", 1).is_empty(),
            "a call that got its ticket at once is not logged as waiting"
        );
        let second = tokio::spawn({
            let slot = Arc::clone(&slot);
            async move { slot.run_ticket().await.is_some() }
        });
        meclaw_testing::wait::until("the queued call is in the log", || async {
            !queued_lines("g-queue-log", 1).is_empty()
        })
        .await;
        drop(held);
        assert!(
            meclaw_testing::deadline::hang("the queued call gets its ticket", second)
                .await
                .expect("the waiter task"),
            "the queued call ran once the ticket came back"
        );
    }
}
