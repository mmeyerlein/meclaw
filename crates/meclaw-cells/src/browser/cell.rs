//! The handler half of the `browser` cell.
//!
//! It owns the cell state and the `cell.db` and is the only half that emits. It
//! never touches the browser: what it sends the I/O half are commands, and what
//! comes back are events.
//!
//! Three emission lanes, and every one of them is declared in this cell type's
//! own `contract.emits.hop.route.values` — an undeclared route does not cost a
//! message, it costs the whole send (v2v#13).
//!
//! * `receipt` answers a message and travels on the `OutputSink`, inside the
//!   requester's trace;
//! * `page` and `error` happen while no message is being handled and can only
//!   go out through the `OriginSink`, which starts a trace of its own.

use crate::browser::db;
use crate::browser::error::BrowserError;
use crate::browser::io::{self, BrowserIo};
use crate::browser::pages::Viewport;
use crate::browser::pages::{PageReport, PageState};
use crate::browser::params::BrowserParams;
use crate::browser::parse::{Verb, parse_verb};
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{CellOutput, Message, OriginSink, OutputSink, Path};
use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// The answer channel of one command.
type Answer = oneshot::Sender<Result<Vec<PageReport>, BrowserError>>;

/// What the handler asks the I/O half to do.
///
/// Every variant carries its own answer channel, so `handle` awaits the verb it
/// sent rather than correlating an event to it afterwards.
pub enum BrowserCommand {
    /// Open a page, or make sure it is at this address.
    Open {
        /// The page id.
        page: String,
        /// Where it should be.
        url: String,
        /// The identity it belongs to.
        context: String,
        /// The shape the app asked for, when it asked for one.
        viewport: Option<Viewport>,
        /// Where the answer goes.
        answer: Answer,
    },
    /// Send an open page somewhere else.
    Navigate {
        /// The page id.
        page: String,
        /// Where it should go.
        url: String,
        /// Where the answer goes.
        answer: Answer,
    },
    /// Close a page.
    Close {
        /// The page id.
        page: String,
        /// Where the answer goes.
        answer: Answer,
    },
    /// Close a context, and every page in it.
    ContextClose {
        /// The context name.
        context: String,
        /// Where the answer goes.
        answer: Answer,
    },
    /// Open again, from this cell's own rows, what its last life was holding.
    ///
    /// Nobody outside asks for this (OR-G17): the cell reads its `pages` table
    /// at startup and opens what stands there. There is no `gone` state for an
    /// app to answer, because there is nothing for it to answer.
    Reopen {
        /// The rows, as `cell.db` holds them.
        rows: Vec<crate::browser::db::PageRow>,
        /// Where the answers go.
        answer: Answer,
    },
}

/// What the I/O half tells the handler about.
pub enum BrowserEvent {
    /// A page changed state, address or title.
    Page(PageReport),
    /// A renderer went. The page is suspended; the others answer on.
    Crashed {
        /// What the page looks like now.
        report: PageReport,
    },
    /// The browser could not be started, or did not hold.
    Failed(BrowserError),
    /// The browser this cell was holding is gone, and the cell has to come
    /// back rather than end.
    ///
    /// Separate from [`BrowserEvent::Failed`] because the two ends are
    /// different, not because the words are (wave G, g5, finding B-G2): a
    /// browser that never started leaves a cell with nothing to do and it
    /// parks, while a browser that died leaves a cell holding rows it can
    /// reopen — and OR-G17 says it does, out of its own `pages` table, on the
    /// next life. Getting to a next life is the whole content of this
    /// variant.
    Died(BrowserError),
}

/// The `browser` cell.
pub struct BrowserCell {
    /// Where this cell sits: the target of its source emissions.
    pub path: Path,
    /// The params this life runs on.
    pub params: BrowserParams,
    /// When this cell last reported an interaction, per page (T7).
    pub active_at: HashMap<String, i64>,
    /// The I/O state, taken once per spawn by [`LongRunningCell::split_io`].
    io: Option<BrowserIo>,
    /// This cell's own way to the I/O half.
    ///
    /// The substrate hands `run_io` a receiver and gives the matching sender to
    /// `handle` alone — and `on_start` runs before the first message, with no
    /// sender in sight. So the cell mints a pair of its own and keeps this end,
    /// the same arrangement the `voice` cell has for the same reason.
    own_commands: mpsc::Sender<BrowserCommand>,
}

impl BrowserCell {
    /// A cell around the I/O state the factory built.
    pub fn new(path: Path, params: BrowserParams, io: BrowserIo) -> Self {
        let (own_commands, from_handler) = mpsc::channel(32);
        let mut io = io;
        io.from_handler = Some(from_handler);
        Self {
            path,
            params,
            active_at: HashMap::new(),
            io: Some(io),
            own_commands,
        }
    }

    /// When this cell last told the topology that somebody used `page`.
    pub fn active_at(&self, page: &str) -> Option<i64> {
        self.active_at.get(page).copied()
    }

    /// Remember an interaction, so the next `page` emission can carry it.
    ///
    /// It has to survive that emission: the app writes `touched` from it, and a
    /// value cleared too early would set a window's freshness back to nothing.
    pub fn remember_interaction(&mut self, page: &str, at: i64) {
        self.active_at.insert(page.to_string(), at);
    }

    /// One `page` emission. Source lane, at this cell's own path.
    async fn emit_page(&self, sink: &OriginSink, report: &PageReport) {
        let _ = sink
            .emit(CellOutput {
                target: self.path.clone(),
                content: page_body(report, self.path.as_str()),
            })
            .await;
    }

    /// One `error` emission. Source lane.
    async fn emit_error(&self, sink: &OriginSink, e: &BrowserError, page: &str) {
        let mut header = Map::new();
        header.insert("route".into(), json!("error"));
        header.insert("error_code".into(), json!(e.error_code()));
        header.insert("platform".into(), json!("browser"));
        header.insert("page".into(), json!(page));
        header.insert("owner".into(), json!(self.path.as_str()));
        let content = json!({
            "header": Value::Object(header),
            "messages": [],
            "error_code": e.error_code(),
            "detail": e.detail(),
            "page": page,
        });
        let _ = sink
            .emit(CellOutput {
                target: self.path.clone(),
                content,
            })
            .await;
    }

    /// One `receipt`. It answers a message, so it rides the requester's trace.
    async fn emit_receipt(
        &self,
        sink: &OutputSink,
        msg: &Message,
        call_id: &str,
        error_code: &str,
        page: &str,
        detail: &str,
    ) {
        let target = msg.reply_to.clone().unwrap_or_else(|| msg.target.clone());
        let mut header = Map::new();
        header.insert("route".into(), json!("receipt"));
        header.insert("error_code".into(), json!(error_code));
        header.insert("platform".into(), json!("browser"));
        header.insert("page".into(), json!(page));
        header.insert("owner".into(), json!(self.path.as_str()));
        let content = json!({
            "header": Value::Object(header),
            "messages": [{
                "origin": "tool", "type": "tool_result", "id": call_id,
                "text": json!({"error_code": error_code, "page": page, "detail": detail})
                    .to_string()
            }],
            "error_code": error_code,
            "page": page,
            "detail": detail,
        });
        let _ = sink.push(CellOutput { target, content }).await;
    }

    /// Write what a report says into `cell.db`, or take the row away.
    async fn persist(&self, db: &mut DbConn, report: &PageReport) {
        let row = report.row.clone();
        let closed = report.state == PageState::Closed;
        let _ = db
            .call_with_timeout(move |conn| {
                if closed {
                    db::delete_page(conn, &row.page)
                } else {
                    db::upsert_page(conn, &row)
                }
            })
            .await;
    }
}

/// The body of a `page` emission.
///
/// Every key is always set, empty where unknown — the shape `compose.py refuse`
/// established, and the reason is the reader: an app that has to tell "absent"
/// from "empty" for eight keys writes eight branches.
pub fn page_body(report: &PageReport, owner: &str) -> Value {
    let row = &report.row;
    let viewport =
        crate::browser::pages::viewport_text(row.viewport_w, row.viewport_h, row.viewport_dpr);
    let mut header = Map::new();
    header.insert("route".into(), json!("page"));
    header.insert("platform".into(), json!("browser"));
    // On the hop and in the body both: a hop is single-hop, and an app reading
    // the body should not have to reach for the envelope.
    header.insert("page".into(), json!(row.page));
    header.insert("owner".into(), json!(owner));
    json!({
        "header": Value::Object(header),
        "messages": [],
        "page": row.page,
        "url": row.url,
        "title": report.title,
        "state": report.state.as_str(),
        "context": row.context,
        "viewport": viewport,
        "content_type": report.content_type,
        // Epoch milliseconds AS TEXT, and empty when this emission is not about
        // an interaction: a number would invite an app to do arithmetic on a
        // clock that is not its own.
        "active_at": report.active_at.map(|ms| ms.to_string()).unwrap_or_default(),
    })
}

/// Ask the I/O half for one verb and wait for its answer, under the A-timeout.
///
/// The seam between the two halves of this cell, and the one place in it where
/// a wait can be unbounded. The handler holds the `handle()` call while it
/// waits, so it drains no events while it does — and the I/O half writes its
/// reports onto a bounded channel. Full channel plus unbounded wait is a
/// deadlock, and behind it there is nothing: a long-running cell runs with
/// `cell.timeout: -1` and has no message-timeout backstop (`AGENTS.md`
/// rule 12). So the wait carries `params.external_timeout_ms` like every other
/// I/O in this cell, and running out of it is a named refusal.
pub async fn ask_io(
    commands: &mpsc::Sender<BrowserCommand>,
    command: BrowserCommand,
    wait: oneshot::Receiver<Result<Vec<PageReport>, BrowserError>>,
    verb: &str,
    cap: std::time::Duration,
) -> Result<Vec<PageReport>, BrowserError> {
    // Queue and answer share one deadline: an I/O half that cannot take the
    // command is one that will not answer it either.
    let deadline = tokio::time::Instant::now() + cap;
    match tokio::time::timeout_at(deadline, commands.send(command)).await {
        Err(_) => {
            return Err(BrowserError::CdpTimeout {
                method: verb.to_string(),
            });
        }
        Ok(Err(_)) => {
            return Err(BrowserError::SpawnFailed(
                "this cell has no browser half to ask".to_string(),
            ));
        }
        Ok(Ok(())) => {}
    }
    match tokio::time::timeout_at(deadline, wait).await {
        Err(_) => Err(BrowserError::CdpTimeout {
            method: verb.to_string(),
        }),
        Ok(Err(_)) => Err(BrowserError::BrowserCrashed(
            "the browser went while the request was outstanding".to_string(),
        )),
        Ok(Ok(out)) => out,
    }
}

impl LongRunningCell for BrowserCell {
    type Event = BrowserEvent;
    type Reconfig = BrowserCommand;
    /// `Option`, because `split_io` takes the state by value and runs once per
    /// spawn; a second call would have nothing to give.
    type Io = Option<BrowserIo>;

    fn split_io(&mut self) -> Self::Io {
        self.io.take()
    }

    // The explicit `+ Send` is load-bearing: AFIT does not bind `Send` to the
    // returned future, and the generic `tokio::spawn` in the substrate needs it.
    #[allow(clippy::manual_async_fn)]
    fn run_io(
        io: Self::Io,
        events_tx: mpsc::Sender<Self::Event>,
        commands: mpsc::Receiver<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send {
        async move {
            let Some(io) = io else {
                // A cell built outside a colony, or a second call. There is no
                // browser to run, and A1′ still holds: park until the handler
                // goes.
                let mut commands = commands;
                while commands.recv().await.is_some() {}
                return;
            };
            io::run_io(io, events_tx, commands).await;
        }
    }

    /// Open again what the last life was holding, before anything else runs.
    ///
    /// The slot exists for exactly this (`LongRunningCell::on_start`): a
    /// recovery driven by an I/O event instead would race the mailbox, and a
    /// message handled first would make this life's fresh rows look like the
    /// last life's orphans.
    #[allow(clippy::manual_async_fn)]
    fn on_start<'a>(
        &'a mut self,
        sink: &'a OriginSink,
        db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            let rows = match db.call_with_timeout(|conn| db::open_pages(conn)).await {
                Ok(Ok(rows)) => rows,
                // A table this cell cannot read is not worth a panic on the
                // restart barrier: the cell comes up holding nothing, loudly.
                Ok(Err(e)) => {
                    tracing::error!(
                        path = self.path.as_str(),
                        error = %e,
                        "browser: the pages of the last life could not be read"
                    );
                    Vec::new()
                }
                Err(e) => {
                    tracing::error!(
                        path = self.path.as_str(),
                        error = %e,
                        "browser: reading the pages of the last life timed out"
                    );
                    Vec::new()
                }
            };
            if rows.is_empty() {
                return;
            }
            let (answer, wait) = oneshot::channel();
            if self
                .own_commands
                .send(BrowserCommand::Reopen { rows, answer })
                .await
                .is_err()
            {
                return;
            }
            match wait.await {
                Ok(Ok(reports)) => {
                    for report in reports {
                        self.persist(db, &report).await;
                        self.emit_page(sink, &report).await;
                    }
                }
                Ok(Err(e)) => self.emit_error(sink, &e, "").await,
                Err(_) => {}
            }
        }
    }

    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a mut self,
        msg: Message,
        sink: &'a OutputSink,
        db: &'a mut DbConn,
        commands: &'a mpsc::Sender<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            let parsed = match parse_verb(&msg) {
                Ok(p) => p,
                Err(e) => {
                    self.emit_receipt(sink, &msg, "", "invalid_input", "", &e)
                        .await;
                    return;
                }
            };
            let page = parsed.verb.page().to_string();
            let verb = parsed.verb.name();
            let (answer, wait) = oneshot::channel();
            let command = match parsed.verb {
                Verb::Open {
                    page,
                    url,
                    context,
                    viewport,
                } => BrowserCommand::Open {
                    page,
                    url,
                    context,
                    viewport,
                    answer,
                },
                Verb::Navigate { page, url } => BrowserCommand::Navigate { page, url, answer },
                Verb::Close { page } => BrowserCommand::Close { page, answer },
                Verb::ContextClose { context } => BrowserCommand::ContextClose { context, answer },
            };
            let cap = Duration::from_millis(self.params.external_timeout_ms);
            match ask_io(commands, command, wait, verb, cap).await {
                Err(e) => {
                    self.emit_receipt(
                        sink,
                        &msg,
                        &parsed.call_id,
                        e.error_code(),
                        &page,
                        &e.detail(),
                    )
                    .await;
                }
                Ok(reports) => {
                    for report in &reports {
                        self.persist(db, report).await;
                    }
                    self.emit_receipt(sink, &msg, &parsed.call_id, "ok", &page, "")
                        .await;
                }
            }
        }
    }

    #[allow(clippy::manual_async_fn)]
    fn handle_event<'a>(
        &'a mut self,
        event: Self::Event,
        sink: &'a OriginSink,
        db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            match event {
                BrowserEvent::Page(report) | BrowserEvent::Crashed { report } => {
                    let mut report = report;
                    // The interaction has to survive the emission that carried
                    // it. An app writes its window's freshness from `active_at`,
                    // and a field that went empty on the next state change would
                    // set that freshness back to nothing — so the cell remembers
                    // the last one per page and stamps every emission with it.
                    if let Some(at) = report.active_at {
                        self.remember_interaction(&report.row.page, at);
                    } else {
                        report.active_at = self.active_at(&report.row.page);
                    }
                    if report.state == PageState::Closed {
                        self.active_at.remove(&report.row.page);
                    }
                    self.persist(db, &report).await;
                    self.emit_page(sink, &report).await;
                }
                BrowserEvent::Failed(e) => {
                    self.emit_error(sink, &e, "").await;
                }
                BrowserEvent::Died(e) => {
                    // The app hears WHY first: after the next line there is no
                    // handler left to say it.
                    self.emit_error(sink, &e, "").await;
                    // And then the cell goes down loudly, which is the point.
                    // `spawn_watcher` reads a clean exit of either half as
                    // `DeathKind::Normal`, and `handle_cell_died` REMOVES a
                    // cell that ended normally instead of restarting it —
                    // `colony.rs:1561`, the line the browser cell was actually
                    // logging when its child was killed: `cell ended normally,
                    // removing from registry path=/browser`, `restart_count`
                    // 0, no page reopened (wave G, g5, finding B-G2). Only
                    // `Panic` reaches the `one_for_one` restart, and a
                    // long-running cell runs with `cell.timeout: -1`, so the
                    // `message_timeout` backstop is not there to reach it
                    // with either. The `mcp` cell writes the same sentence for
                    // the same reason (`mcp/io.rs:96`). The restart is what
                    // makes OR-G17 true: the next life reads its `pages` table
                    // in `on_start` and opens every row again as `reopened`.
                    panic!("browser: {}", e.detail());
                }
            }
        }
    }
}

#[cfg(test)]
mod seam_tests {
    use super::*;
    use crate::browser::pages::{PageState, Viewport};

    /// One command, which never reaches anybody.
    fn a_command() -> (
        BrowserCommand,
        oneshot::Receiver<Result<Vec<PageReport>, BrowserError>>,
    ) {
        let (answer, wait) = oneshot::channel();
        (
            BrowserCommand::Close {
                page: "card-1".to_string(),
                answer,
            },
            wait,
        )
    }

    #[tokio::test]
    async fn a_full_command_channel_is_a_refusal_and_never_a_wait() {
        // The deadlock this cap exists for: the handler holds `handle()` while
        // it waits, so it drains no events; the I/O half is blocked writing a
        // report onto a channel nobody is draining and never reaches its
        // `commands` arm. Both halves wait forever, and `cell.timeout: -1`
        // means nothing behind them ever notices.
        let (commands, _keep) = mpsc::channel::<BrowserCommand>(1);
        let (filler, _) = a_command();
        commands.send(filler).await.expect("the one slot");
        let (command, wait) = a_command();
        let started = std::time::Instant::now();
        let e = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            ask_io(
                &commands,
                command,
                wait,
                "in_close",
                Duration::from_millis(200),
            ),
        )
        .await
        .expect("the seam ends on its own, which is the whole point")
        .expect_err("nobody took the command");
        assert_eq!(e.error_code(), "cdp_timeout");
        assert!(
            e.detail().contains("in_close"),
            "a caller with three windows in flight has to learn which one: {}",
            e.detail()
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "and it ends at the A-timeout, not at a backstop that does not exist: {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn an_answer_that_never_comes_is_the_same_refusal() {
        let (commands, mut taken) = mpsc::channel::<BrowserCommand>(4);
        let (command, wait) = a_command();
        // Taken, and then never answered: the oneshot sender is held.
        // The two halves run CONCURRENTLY: `ask_io` is a future, and a
        // receiver awaited before it is polled waits for a send that has not
        // been made yet.
        let (out, held) = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            futures_util::future::join(
                ask_io(
                    &commands,
                    command,
                    wait,
                    "in_close",
                    Duration::from_millis(200),
                ),
                taken.recv(),
            ),
        )
        .await
        .expect("the wait ends on its own");
        assert!(held.is_some(), "the command did arrive");
        let e = out.expect_err("nobody answered");
        assert_eq!(e.error_code(), "cdp_timeout");
    }

    #[tokio::test]
    async fn an_answer_that_does_come_is_handed_straight_back() {
        let (commands, mut taken) = mpsc::channel::<BrowserCommand>(4);
        let (command, wait) = a_command();
        let report = PageReport {
            row: crate::browser::db::PageRow {
                page: "card-1".to_string(),
                url: "https://example.com/".to_string(),
                context: "alex".to_string(),
                viewport_w: Viewport::default().width,
                viewport_h: Viewport::default().height,
                viewport_dpr: 1.0,
                mobile: false,
                state: "closed".to_string(),
                opened_at: 1,
                updated_at: 2,
            },
            state: PageState::Closed,
            title: String::new(),
            content_type: String::new(),
            active_at: None,
        };
        let answered = report.clone();
        let answering = async {
            match taken.recv().await.expect("the command arrives") {
                BrowserCommand::Close { answer, .. } => {
                    let _ = answer.send(Ok(vec![answered]));
                }
                _ => panic!("the command that was sent"),
            }
        };
        let (out, ()) = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            futures_util::future::join(
                ask_io(
                    &commands,
                    command,
                    wait,
                    "in_close",
                    Duration::from_secs(30),
                ),
                answering,
            ),
        )
        .await
        .expect("the answer travels well inside the marker");
        assert_eq!(out.expect("the answer"), vec![report]);
    }
}
