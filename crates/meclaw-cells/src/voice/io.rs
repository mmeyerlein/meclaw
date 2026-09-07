//! The I/O half of the `voice` cell: the listener, the session registry, and
//! the loop that keeps both alive for the cell's whole life (wave voice-cell).
//!
//! This half owns the axum server, the WebSocket connections it accepts and the
//! provider sockets those connections drive. It holds no cell state, no
//! `cell.db` handle and no `OutputSink`; what it learns from the outside world
//! it pushes to the handler as a [`VoiceEvent`], and what the handler decides
//! comes back as a [`VoiceReconfig`].
//!
//! # Why audio never reaches the handler
//!
//! A sample is not a decision. Audio arrives here, goes straight into the
//! speech-to-text session of the connection it arrived on, and leaves here as
//! synthesised bytes — it never enters a mailbox and never crosses the channel
//! to the handler. What crosses is what a turn machine can act on: transcripts,
//! turn boundaries, control frames, the end of a synthesis. That is what makes
//! a real-time channel affordable on a substrate whose unit of work is a
//! message.
//!
//! # The one shared table
//!
//! [`VoiceIoShared::sessions`] maps a session identity to the connection task
//! serving it, so the handler can address a client it never sees. It is behind
//! a mutex for the same reason `web`'s `ViewerRegistry` is: it is not cell
//! state but a table of live socket senders, owned by this half, written by
//! whichever connection task starts or ends, and read to address one of them.
//! No `.await` is held across any of its critical sections.
//!
//! # Who is allowed to wait for a client (GH #593)
//!
//! [`run_io`] is not. It reads the handler's commands and the `Rebind` that
//! moves the listener out of the same loop, so a wait for one client is a wait
//! for the listener — and a `Rebind` queued behind a client that had stopped
//! reading was simply never seen, until the handler's ack timeout refused a
//! perfectly good update.
//!
//! So delivery is two hops. The registry does not hold a connection's own
//! channel; it holds the queue of a [`deliver`] task, one per connection, and
//! *that* task is the one allowed to park until the connection has room.
//! Backpressure is unchanged where it belongs (spec § 2): a slow client still
//! gets every command, in order, because somebody waits for it — just not the
//! loop that has to stay reachable. Two things end that patience: the wait is
//! capped by the cell's operation timeout (hard rule 12), **or** 64 further
//! commands pile up behind an already full connection channel — 128 in flight,
//! which no client is merely slow enough to justify. Past either, the
//! connection is given up on, every command it still holds is spoken for (a
//! `Speak` as a failed [`VoiceEvent::SpeakEnded`], for as long as this
//! connection is still the session's), and the session leaves the table with a
//! `Disconnected`. Nothing is dropped in silence; what ends is the connection.

use axum::Router;
use meclaw_colony::IoLivenessMark;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{Mutex, mpsc, watch};

use crate::voice::cell::{VoiceEvent, VoiceReconfig};
use crate::voice::contract::{SttProvider, TtsProvider};
use crate::voice::service::router;
use crate::voice::wire::{CLOSE_SESSION_REPLACED, Mode, ServerFrame, SpeakEndReason};

/// What one connection task can be told to do.
///
/// Synthesised audio does **not** travel this channel. It has one of its own,
/// polled by the connection task beside this one, so that a `CancelSpeak`
/// arriving mid-synthesis is acted on at once instead of queueing behind the
/// audio it is meant to stop.
#[derive(Debug)]
pub enum ToConnection {
    /// Send this text frame to the client.
    Frame(ServerFrame),
    /// Close the connection with this code.
    Close(u16),
    /// Start one synthesis.
    Speak {
        /// The id the `speak_start`/`speak_end` pair carries.
        speak_id: String,
        /// What to say.
        text: String,
    },
    /// Stop the running synthesis, if there is one.
    CancelSpeak,
}

/// One live connection, as the registry holds it.
struct SessionHandle {
    /// Which connection this is — a later one displaces an earlier one, and the
    /// displaced task must not then delete its successor's entry.
    conn_id: u64,
    /// Where to reach that connection's [`deliver`] task.
    ///
    /// Not the connection task's own channel: nothing that addresses a session
    /// from [`run_io`] may wait on the client behind it (GH #593), so the wait
    /// lives one hop further out, in a task of this connection's own.
    dispatch: mpsc::Sender<ToConnection>,
}

/// How many commands may wait for one connection before it is given up on.
///
/// The second buffer of the two-hop delivery: [`deliver`] parks on the
/// connection's own channel, and this is what fills up behind it while it does.
/// A client that is 64 commands behind *and* has a full connection channel is
/// not slow, it is gone.
const DISPATCH_QUEUE: usize = 64;

/// Everything a request task and a connection task need.
///
/// Built once by [`run_io`] and shared as `Arc` — it is axum's state and the
/// connection tasks' handle in one, because they need exactly the same things.
pub struct VoiceIoShared {
    /// The recognition provider, shared by every connection.
    pub stt: Arc<dyn SttProvider>,
    /// The synthesis provider, if one is configured.
    pub tts: Option<Arc<dyn TtsProvider>>,
    /// The mode a connection gets when its query string does not say.
    pub default_mode: Mode,
    /// A-timeout (hard rule 12) around a provider's first answer.
    pub external_timeout: Duration,
    /// The longest silence tolerated between two chunks of a running synthesis.
    pub idle_timeout: Duration,
    /// How much audio one outbound binary frame may carry, in milliseconds;
    /// `0` sends every synthesis chunk exactly as its provider produced it.
    pub audio_out_frame_ms: u32,
    /// Whether the handler turns markdown into speech text before a synthesis.
    ///
    /// The I/O half never applies it — the handler does, before the text is
    /// queued. It is carried here so the declaration can report it (`hello`,
    /// `GET /info`), and it is a plain copy rather than shared state: the two
    /// halves talk over channels, so a value moved by a `params` update reaches
    /// the DECLARATION on the next respawn while the rewriting itself is in
    /// force at once.
    pub speak_plain: bool,
    /// How long a released `hold` boundary waits for the recognition provider's
    /// own end of turn before the handler cuts the turn, in milliseconds.
    ///
    /// The I/O half never applies it either — the handler owns the boundary and
    /// arms the cap over this seam. It is carried here for the DECLARATION
    /// (`hello`, `GET /info`), and it is a plain copy for the same reason
    /// `speak_plain` is: a `params` update is in force at the next `release`,
    /// while the declaration follows on the next respawn.
    pub release_grace_ms: u64,
    /// Where semantic events go — the handler is the only reader.
    pub events_tx: mpsc::Sender<VoiceEvent>,
    /// Issue #7: proof that a provider round trip happened.
    pub liveness: IoLivenessMark,
    /// Who is connected, and how many disconnects are still unspoken.
    sessions: Mutex<Registry>,
    /// What is left to wait for, as a channel rather than a number.
    ///
    /// A rebind has to announce its new address *after* the connections the old
    /// one accepted have reported themselves gone — otherwise the handler reads
    /// `Bound` and only later learns that half its session table is dead. The
    /// number published here is [`Registry::outstanding`], so [`Self::drained`]
    /// can wait for zero without asking again and again.
    live: watch::Sender<usize>,
}

/// The connection table and the reports it still owes.
///
/// # Why the second number
///
/// A connection leaves the table and *then* emits its `Disconnected`, so the
/// table alone goes empty one moment too early. With one connection that was
/// invisible; with two, the second could empty the table and publish zero while
/// the first was still between its own removal and its own emit — and the
/// rebind would announce `Bound` in that gap. Counting the unspoken reports
/// alongside the live entries closes it: the number only reaches zero when
/// every connection is both gone and *reported* gone.
#[derive(Default)]
struct Registry {
    /// Who is connected, under which session identity.
    live: HashMap<String, SessionHandle>,
    /// Disconnects that have been decided but not yet emitted.
    pending_reports: usize,
}

impl Registry {
    /// Live connections plus disconnects still owed.
    fn outstanding(&self) -> usize {
        self.live.len() + self.pending_reports
    }
}

impl VoiceIoShared {
    /// Take over `session_id` for `conn_id`.
    ///
    /// Returns the displaced connection's sender when this session was already
    /// held, so the caller can close it with [`CLOSE_SESSION_REPLACED`]. The
    /// displacement is the whole point of a client-chosen session identity: a
    /// reconnect must be able to claim the address it had.
    async fn claim(
        &self,
        session_id: &str,
        conn_id: u64,
        dispatch: mpsc::Sender<ToConnection>,
    ) -> Option<mpsc::Sender<ToConnection>> {
        let mut registry = self.sessions.lock().await;
        let displaced = registry
            .live
            .insert(session_id.to_string(), SessionHandle { conn_id, dispatch })
            .map(|old| old.dispatch);
        // `send_replace`, not `send`: this sender has no receiver of its own,
        // and `send` would refuse — silently leaving the count at zero and a
        // rebind never waiting for anything.
        let _ = self.live.send_replace(registry.outstanding());
        displaced
    }

    /// Give up `session_id`, but only if `conn_id` still holds it.
    ///
    /// A displaced connection ends after its successor registered, so an
    /// unconditional remove would delete the live entry. The verdict is also
    /// what decides who reports the `Disconnected`: the task that really left
    /// the table, exactly once.
    /// Whoever removes the entry owes the report, and the debt is booked in the
    /// same critical section — so the entry is never gone and unaccounted for,
    /// not even between two instructions.
    async fn release(&self, session_id: &str, conn_id: u64) -> bool {
        let mut registry = self.sessions.lock().await;
        match registry.live.get(session_id) {
            Some(h) if h.conn_id == conn_id => {
                registry.live.remove(session_id);
                registry.pending_reports += 1;
                // Unchanged total: one live entry became one owed report.
                let _ = self.live.send_replace(registry.outstanding());
                true
            }
            _ => false,
        }
    }

    /// Whether `conn_id` is still the connection this session names.
    ///
    /// The question a report has to ask before it is sent. A `Speak` that could
    /// not be delivered is answered with a failed `SpeakEnded` *for a session
    /// identity* — and that identity may have moved on: a client that
    /// reconnects with the same `?session=` displaces the old connection, and a
    /// rebind evicts every one of them. Reporting without asking would hand the
    /// **new** connection's session the failure of the old one's synthesis, or
    /// speak for a session that is already gone.
    async fn holds(&self, session_id: &str, conn_id: u64) -> bool {
        self.sessions
            .lock()
            .await
            .live
            .get(session_id)
            .is_some_and(|h| h.conn_id == conn_id)
    }

    /// Settle what [`Self::release`] booked, after the report went out.
    ///
    /// Called *after* the disconnect has been emitted, never before: a waiter
    /// released by the count would otherwise run ahead of the event it is
    /// waiting for, which is the whole thing [`Self::drained`] exists to
    /// prevent.
    async fn finish_report(&self, reported: bool) {
        let mut registry = self.sessions.lock().await;
        if reported {
            registry.pending_reports = registry.pending_reports.saturating_sub(1);
        }
        let _ = self.live.send_replace(registry.outstanding());
    }

    /// Wait until nothing is left to wait for, or until `limit` runs out.
    ///
    /// The bound is the point: a client whose socket is wedged must cost a
    /// rebind a moment, not the listener. What is still registered when the
    /// deadline passes is reported by its own task whenever it does end.
    async fn drained(&self, limit: Duration) {
        let mut live = self.live.subscribe();
        if *live.borrow_and_update() == 0 {
            return;
        }
        let _ = tokio::time::timeout(limit, async {
            while live.changed().await.is_ok() {
                if *live.borrow_and_update() == 0 {
                    return;
                }
            }
        })
        .await;
    }

    /// Hand one session's [`deliver`] task a command, without ever waiting on
    /// the client behind it.
    ///
    /// # Why this never blocks (GH #593)
    ///
    /// This runs in [`next_round`], the same loop that reads a `Rebind`. Until
    /// this cell's second wave it waited here — a client that had stopped
    /// reading filled its connection channel, the send parked, and a `Rebind`
    /// queued behind it was never seen; the handler's own ack timeout ended the
    /// cycle by refusing an update that was perfectly good. So the wait moved
    /// one hop out, into [`deliver`], and what is left here is a `try_send`
    /// that either fits or does not.
    ///
    /// A command that does not fit is **not dropped in silence**: a `Speak`
    /// becomes a failed [`VoiceEvent::SpeakEnded`] on the handler's error lane,
    /// and the session is given up with a `Disconnected` either way. Two full
    /// buffers is not backpressure any more, it is a connection nobody is on.
    async fn send_to(&self, session_id: &str, cmd: ToConnection) {
        let handle = self
            .sessions
            .lock()
            .await
            .live
            .get(session_id)
            .map(|h| (h.conn_id, h.dispatch.clone()));
        // The handler owns the session table that decides what an unknown
        // session means; it emits `unknown_session` on its error lane. This
        // half only says that nothing was delivered.
        let Some((conn_id, dispatch)) = handle else {
            tracing::debug!(%session_id, "voice: no live connection for this session");
            return;
        };
        match dispatch.try_send(cmd) {
            Ok(()) => {}
            Err(TrySendError::Full(cmd)) => {
                self.undeliverable(session_id, conn_id, cmd, "the client is not reading")
                    .await;
            }
            Err(TrySendError::Closed(cmd)) => {
                self.undeliverable(session_id, conn_id, cmd, "the connection ended")
                    .await;
            }
        }
    }

    /// One command that will never reach its client, and the end of the session
    /// it was addressed to.
    ///
    /// The only `.await` this leaves in a [`next_round`] arm is [`Self::emit`],
    /// and that one is the handler waiting for itself: the events channel is
    /// the handler's own seam, block-only by the spec, and the handler's rebind
    /// already carries the timeout that breaks that cycle. What #593 was about
    /// — waiting for a *client* — is gone from this loop.
    async fn undeliverable(&self, session_id: &str, conn_id: u64, cmd: ToConnection, why: &str) {
        report_undeliverable(self, session_id, conn_id, cmd, why).await;
        self.drop_session(session_id, conn_id).await;
    }

    /// Give up a session and report the disconnect, if `conn_id` still held it.
    async fn drop_session(&self, session_id: &str, conn_id: u64) {
        let reported = self.release(session_id, conn_id).await;
        if reported {
            self.emit(VoiceEvent::Disconnected {
                session_id: session_id.to_string(),
            })
            .await;
        }
        // Only now: the count is what a rebind waits on, and it must not fall
        // to zero until the disconnect it stands for has actually been
        // reported.
        self.finish_report(reported).await;
    }

    /// Drop, and report, whoever is still in the table after a rebind waited.
    ///
    /// [`Self::drained`] gives the connections of the old listener a bounded
    /// moment to report themselves gone. Whoever has not is on a socket that no
    /// longer has a listener behind it and cannot be reached — a wedged client
    /// is exactly that case. Leaving the row would leak it into the handler's
    /// session table for the rest of the cell's life, which is the very thing
    /// the drain exists to prevent, so the rebind reports it instead: after a
    /// rebind, no connection of the old address is left, and every one of them
    /// was announced.
    async fn evict_remaining(&self) {
        let gone: Vec<String> = {
            let mut registry = self.sessions.lock().await;
            if registry.live.is_empty() {
                return;
            }
            let ids: Vec<String> = registry.live.keys().cloned().collect();
            // Dropping the handles drops their dispatch senders, which ends the
            // `deliver` tasks and, with them, the connections' own channels.
            registry.live.clear();
            registry.pending_reports += ids.len();
            let _ = self.live.send_replace(registry.outstanding());
            ids
        };
        for session_id in &gone {
            self.emit(VoiceEvent::Disconnected {
                session_id: session_id.clone(),
            })
            .await;
        }
        let mut registry = self.sessions.lock().await;
        registry.pending_reports = registry.pending_reports.saturating_sub(gone.len());
        let _ = self.live.send_replace(registry.outstanding());
    }

    /// Ask every live connection to close with `code`.
    ///
    /// The table is deliberately **not** emptied here. Each connection task
    /// removes its own entry when it ends and reports the `Disconnected` that
    /// goes with it; a drain would delete the entries first, every
    /// [`Self::release`] would then find nothing, and no disconnect would ever
    /// reach the handler — whose own session table would keep growing across
    /// rebinds and could never answer `unknown_session`. What is still there
    /// once the rebind's bounded wait is over is emptied by
    /// [`Self::evict_remaining`], which reports every row it removes.
    async fn close_all(&self, code: u16) {
        let handles: Vec<mpsc::Sender<ToConnection>> = {
            let registry = self.sessions.lock().await;
            registry.live.values().map(|h| h.dispatch.clone()).collect()
        };
        for tx in handles {
            deliver_close(tx, code);
        }
    }

    /// Emit one event to the handler, blocking on a full channel.
    ///
    /// The block is the design (spec § 2): a handler that cannot keep up stalls
    /// its own socket reader, TCP does the rest, and nothing is dropped.
    pub(crate) async fn emit(&self, event: VoiceEvent) {
        if self.events_tx.send(event).await.is_err() {
            tracing::debug!("voice: the handler is gone, event dropped");
        }
    }
}

/// Hand one connection its close code, however busy it is.
///
/// Not `try_send`: a connection whose dispatch queue is momentarily full would
/// silently keep running — and a *displaced* one would go on emitting under a
/// session identity that now belongs to somebody else. Not an `await` either,
/// because the callers hold no lock they may block under and one wedged client
/// must not stall a rebind. So the send gets a task of its own; it ends as soon
/// as the close is queued or the [`deliver`] task drops its receiver — which,
/// since #593, a wedged connection does within `external_timeout` instead of
/// only when its socket finally dies.
fn deliver_close(tx: mpsc::Sender<ToConnection>, code: u16) {
    tokio::spawn(async move {
        let _ = tx.send(ToConnection::Close(code)).await;
    });
}

/// A connection id, unique within this process.
///
/// Not a security value and never leaves the process: it only has to tell two
/// connections claiming the same session apart. The atomic is the same
/// deliberate one `web`'s connection ids use — the substrate's rule bans shared
/// mutable state in a **cell actor**, and this is a counter in the I/O half,
/// touched once per connection.
fn next_conn_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Everything the I/O half owns, handed over once per spawn by `split_io`.
pub struct VoiceIo {
    /// The address to bind.
    pub bind: String,
    /// The port to bind.
    pub port: u16,
    /// The speech-to-text adapter, shared by every connection.
    pub stt: Arc<dyn SttProvider>,
    /// The text-to-speech adapter, or `None` with the echo provider.
    pub tts: Option<Arc<dyn TtsProvider>>,
    /// The mode a connection starts in without a `?mode=`.
    pub default_mode: Mode,
    /// Operation-timeout around every provider I/O (hard rule 12, A).
    pub external_timeout: Duration,
    /// Idle deadline per provider socket; elapsing is the reconnect path.
    pub idle_timeout: Duration,
    /// Outbound frame length in milliseconds, straight from `params`.
    ///
    /// A field rather than an argument of [`VoiceIo::new`]: every caller that
    /// does not care wants the shipped default, and the one that does — the
    /// factory, which builds this half from the effective params of the life
    /// about to start — names it in one line.
    pub audio_out_frame_ms: u32,
    /// Whether the handler turns markdown into speech text before a synthesis.
    /// Carried for the declaration only; see [`VoiceIoShared::speak_plain`].
    pub speak_plain: bool,
    /// How long a released `hold` boundary waits for the provider's end.
    /// Carried for the declaration only; see
    /// [`VoiceIoShared::release_grace_ms`].
    pub release_grace_ms: u64,
    /// Where the connection tasks report everything semantic.
    ///
    /// Set at construction, and replaced by the `LongRunningCell::run_io` impl
    /// with the sender the substrate minted for this spawn — the substrate owns
    /// the receiving half, so the value given here is what a cell built outside
    /// a colony (a fixture, a unit test) reports to.
    ///
    /// (`release_grace_ms` sits above, beside `speak_plain`, for the same
    /// reason: both are the handler's behaviour and this half's declaration.)
    pub events_tx: mpsc::Sender<VoiceEvent>,
    /// The mark the I/O half sets after every successful provider round trip
    /// (issue #7). Attached by `LongRunningCell::attach_liveness`.
    pub liveness: meclaw_colony::io_liveness::IoLivenessMark,
    /// The command seam from the handler.
    ///
    /// **Contract addendum for strand t4.** The substrate hands `run_io` a
    /// `reconfig_rx`, but only `handle` is given the matching sender — and this
    /// cell issues most of its commands from `handle_event`, which the trait
    /// gives no sender at all. So the cell mints its own pair
    /// ([`crate::voice::cell::VoiceCell::new`] keeps the sender and puts the
    /// receiver here) and every [`VoiceReconfig`] travels on it. The same split
    /// the `web` cell has between its `push` channel and the substrate's
    /// reconfig channel, for the same reason.
    ///
    /// `run_io` must therefore select over **both**: this one for commands, and
    /// the substrate's `reconfig_rx` for its closing, which is how the I/O half
    /// learns the handler is gone.
    pub from_handler: Option<mpsc::Receiver<VoiceReconfig>>,
}

impl VoiceIo {
    /// Assemble the I/O half. Sync and await-free: this runs inside the
    /// respawn corridor (phase-5 tripwire).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        bind: String,
        port: u16,
        stt: Arc<dyn SttProvider>,
        tts: Option<Arc<dyn TtsProvider>>,
        default_mode: Mode,
        external_timeout: Duration,
        idle_timeout: Duration,
        events_tx: mpsc::Sender<VoiceEvent>,
    ) -> Self {
        Self {
            bind,
            port,
            stt,
            tts,
            default_mode,
            external_timeout,
            idle_timeout,
            audio_out_frame_ms: crate::voice::params::DEFAULT_AUDIO_OUT_FRAME_MS,
            speak_plain: crate::voice::params::DEFAULT_SPEAK_PLAIN,
            release_grace_ms: crate::voice::params::DEFAULT_RELEASE_GRACE_MS,
            events_tx,
            liveness: meclaw_colony::io_liveness::IoLivenessMark::disabled(),
            from_handler: None,
        }
    }
}

/// What ended one serving round.
enum Round {
    /// The params moved: serve this address next. Carried as an address rather
    /// than an open listener, because the old socket is only released when the
    /// round's `serve` future is dropped.
    Rebind {
        /// The address to bind.
        bind: String,
        /// The port to bind.
        port: u16,
        /// Where the handler waits for the verdict.
        ack: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    /// The server future ended while the cell is still live (GH #592).
    ///
    /// Not a reason to return, and handled exactly like a bind that failed:
    /// report it and go on in a round with no listener, so a later `Rebind`
    /// can still put the cell back on the air.
    ServeEnded,
    /// The cell is going away.
    Done,
}

/// The I/O loop: bind, serve, and stay up for the cell's whole life.
///
/// **A1′**: this function must not return voluntarily while the cell is live.
/// Only the handler closing the reconfigure channel ends it — a server future
/// that ends is reported like a failed bind and leaves the next round without
/// a listener (GH #592), never a return. A round with no
/// listener is an ordinary round — a port that was taken at boot costs no
/// restart, because a later `Rebind` can still name one this cell can have
/// (the shape `web` arrived at in GH #410, for the same reason).
pub async fn run_io(io: VoiceIo, reconfig_rx: mpsc::Receiver<VoiceReconfig>) {
    run_io_with(io, reconfig_rx, serve_forever).await
}

/// Serve `router` on `l` until the server future ends.
///
/// The one place the axum server future is built, and the reason it is a
/// function: `axum::serve` without graceful shutdown does not end on its own,
/// so what [`run_io_with`] does when it *does* end is otherwise unreachable
/// and untestable. A test passes a factory that ends at once (GH #592). This
/// is a seam, not a mock — the shipped path is this function and nothing else.
pub(crate) async fn serve_forever(l: tokio::net::TcpListener, router: Router) {
    // Not swallowed: the round is about to be reported as `listener ended`,
    // and this is the only place the reason for it exists.
    if let Err(e) = std::future::IntoFuture::into_future(axum::serve(l, router)).await {
        tracing::error!(error = %e, "voice: the server stopped with an error");
    }
}

/// [`run_io`] with the server future left open as a parameter — see
/// [`serve_forever`] for why.
pub(crate) async fn run_io_with<F, Fut>(
    mut io: VoiceIo,
    mut reconfig_rx: mpsc::Receiver<VoiceReconfig>,
    serve_factory: F,
) where
    F: Fn(tokio::net::TcpListener, Router) -> Fut + Send,
    Fut: std::future::Future<Output = ()> + Send,
{
    // Two command seams, one loop. The substrate hands `run_io` its own
    // `reconfig_rx`, but only `handle` is given the matching sender — and this
    // cell issues most of its commands from `handle_event`. So the cell mints
    // its own pair and puts the receiver in `from_handler` (see [`VoiceIo`]).
    // Both are drained here: either one closing means the handler is gone,
    // which is the only thing that ends this function (A1′). A cell built
    // outside a colony leaves `from_handler` empty and speaks on the
    // substrate's channel alone.
    let mut from_handler = io.from_handler.take();
    let shared = Arc::new(VoiceIoShared {
        stt: io.stt,
        tts: io.tts,
        default_mode: io.default_mode,
        external_timeout: io.external_timeout,
        idle_timeout: io.idle_timeout,
        audio_out_frame_ms: io.audio_out_frame_ms,
        speak_plain: io.speak_plain,
        release_grace_ms: io.release_grace_ms,
        events_tx: io.events_tx,
        liveness: io.liveness,
        sessions: Mutex::new(Registry::default()),
        live: watch::channel(0).0,
    });
    let mut bind = io.bind;
    let mut port = io.port;

    let mut listener = match bind_addr(&bind, port).await {
        Ok(l) => Some(l),
        Err(e) => {
            shared.emit(VoiceEvent::BindFailed(e)).await;
            None
        }
    };

    loop {
        let round = match listener.take() {
            Some(l) => {
                let bound = l
                    .local_addr()
                    .map(|a| a.to_string())
                    .unwrap_or_else(|_| "unknown".to_string());
                shared.emit(VoiceEvent::Bound(bound)).await;

                // The scope matters: `serve` owns the listener and the socket
                // is only released when `serve` is dropped, which happens at
                // the end of this block — before the next address is bound.
                let serve = serve_factory(l, router(shared.clone()));
                tokio::pin!(serve);
                tokio::select! {
                    _ = &mut serve => Round::ServeEnded,
                    r = next_round(&shared, &mut reconfig_rx, &mut from_handler) => r,
                }
            }
            // Nothing to serve on, and still not a reason to return: the
            // reconfigure channels keep being drained, so a later address can
            // still arrive.
            None => next_round(&shared, &mut reconfig_rx, &mut from_handler).await,
        };

        let (next_bind, next_port, ack) = match round {
            Round::Rebind { bind, port, ack } => (bind, port, ack),
            // A1': the server future ending is not the cell ending. The socket
            // is already released — `serve` was dropped with the block above —
            // so this is the same state a failed bind leaves behind, reported
            // the same way, with the address that stopped answering. The live
            // connections keep their tasks: nothing moved, and a connection
            // outlives the listener that accepted it.
            Round::ServeEnded => {
                shared
                    .emit(VoiceEvent::BindFailed(format!(
                        "{bind}:{port}: listener ended"
                    )))
                    .await;
                continue;
            }
            Round::Done => {
                shared.close_all(1001).await;
                return;
            }
        };

        // The old socket is closed at this point, so this is the first moment
        // the new address can be bound.
        let attempt = bind_addr(&next_bind, next_port).await;
        // Answered before anything else: the handler is parked on this oneshot
        // and drains no events while it waits, so the `Bound` above must not be
        // able to reach a full events channel ahead of the verdict.
        let _ = ack.send(attempt.as_ref().map(|_| ()).map_err(String::clone));
        listener = match attempt {
            Ok(l) => {
                bind = next_bind;
                port = next_port;
                // Every live connection was accepted on a socket that no longer
                // exists. Dropping them is the honest state: the client
                // reconnects against the address it now resolves to, and a
                // registry still naming them would address connections nobody
                // can reach.
                //
                // Waited out rather than merely asked for: the handler must
                // read `Disconnected` for the old connections *before* the
                // `Bound` of the address they are not on, or its own session
                // table would carry rows for sockets nobody can reach.
                shared.close_all(1001).await;
                shared.drained(shared.external_timeout).await;
                // A connection nobody is reading cannot report itself gone in
                // time. It is dropped here, like every other connection of the
                // old address, rather than left in a table that outlives the
                // socket it names.
                shared.evict_remaining().await;
                Some(l)
            }
            // The value passed the parser and still cannot be a listening
            // address. Put the cell back where it was, so a typo costs a moment
            // rather than the listener.
            Err(e) => {
                shared.emit(VoiceEvent::BindFailed(e)).await;
                match bind_addr(&bind, port).await {
                    Ok(l) => Some(l),
                    Err(e) => {
                        shared.emit(VoiceEvent::BindFailed(e)).await;
                        None
                    }
                }
            }
        };
    }
}

/// Bind one address, with the failure text an operator can act on.
async fn bind_addr(addr: &str, port: u16) -> Result<tokio::net::TcpListener, String> {
    tokio::net::TcpListener::bind((addr, port))
        .await
        .map_err(|e| format!("{addr}:{port}: {e}"))
}

/// Serve the handler's commands until one of them ends the round.
///
/// Everything addressed at a session is dispatched here and the loop continues;
/// only a `Rebind` — or the handler going away — ends a round.
async fn next_round(
    shared: &Arc<VoiceIoShared>,
    reconfig_rx: &mut mpsc::Receiver<VoiceReconfig>,
    from_handler: &mut Option<mpsc::Receiver<VoiceReconfig>>,
) -> Round {
    loop {
        let command = tokio::select! {
            // `biased;` so the order is a decision, not a coin toss: the
            // substrate's own seam first. It is a tiebreak, not the fix — in
            // this cell *both* seams can carry a `Rebind` (the cell mints its
            // own channel and sends every command on it; a fixture built
            // without a colony speaks on the substrate's), so no ordering
            // between the two channels could keep a rebind from queueing
            // behind a frame. What keeps it moving is that no arm below waits
            // on a client any more (GH #593).
            biased;

            c = reconfig_rx.recv() => c,
            c = recv_opt(from_handler) => c,
        };
        match command {
            // A closed channel is the handler going away, on either seam.
            None => return Round::Done,
            Some(VoiceReconfig::Rebind { bind, port, ack }) => {
                return Round::Rebind { bind, port, ack };
            }
            Some(VoiceReconfig::ToClient { session_id, frame }) => {
                shared
                    .send_to(&session_id, ToConnection::Frame(frame))
                    .await;
            }
            Some(VoiceReconfig::Speak {
                session_id,
                speak_id,
                text,
            }) => {
                shared
                    .send_to(&session_id, ToConnection::Speak { speak_id, text })
                    .await;
            }
            Some(VoiceReconfig::CancelSpeak { session_id }) => {
                shared.send_to(&session_id, ToConnection::CancelSpeak).await;
            }
            Some(VoiceReconfig::Close { session_id, code }) => {
                shared.send_to(&session_id, ToConnection::Close(code)).await;
            }
            Some(VoiceReconfig::ArmReleaseGrace {
                session_id,
                ms,
                token,
            }) => {
                // A task of its own, because this loop may not wait for
                // anything (GH #593) and a released boundary least of all: the
                // whole point of the grace is that the rest of the cell keeps
                // running while one turn waits for its provider. Nothing here
                // is cancelled when the provider answers first — the answer is
                // a token the handler has moved past, and dropping it costs one
                // message rather than a handle table in the half that does not
                // know what a turn is.
                let shared = shared.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                    shared
                        .emit(VoiceEvent::ReleaseGraceExpired { session_id, token })
                        .await;
                });
            }
        }
    }
}

/// Receive from the cell's own command channel, or wait forever when the cell
/// did not mint one.
async fn recv_opt(rx: &mut Option<mpsc::Receiver<VoiceReconfig>>) -> Option<VoiceReconfig> {
    match rx {
        None => std::future::pending().await,
        Some(rx) => rx.recv().await,
    }
}

/// Register a new connection and displace whoever held its session.
///
/// Called by the connection task itself, so that a handshake that never
/// completes cannot leave an entry behind. The order of the two events is the
/// one a reader expects: the old connection is reported gone before the new one
/// is reported connected.
pub(crate) async fn register(
    shared: &Arc<VoiceIoShared>,
    session_id: &str,
    conn_id: u64,
    to_conn: mpsc::Sender<ToConnection>,
    mode: Mode,
) {
    let dispatch = spawn_delivery(shared.clone(), session_id.to_string(), conn_id, to_conn);
    if let Some(old) = shared.claim(session_id, conn_id, dispatch).await {
        deliver_close(old, CLOSE_SESSION_REPLACED);
        shared
            .emit(VoiceEvent::Disconnected {
                session_id: session_id.to_string(),
            })
            .await;
    }
    shared
        .emit(VoiceEvent::Connected {
            session_id: session_id.to_string(),
            mode,
        })
        .await;
}

/// Give up a session and report the disconnect, if this connection still held it.
pub(crate) async fn unregister(shared: &Arc<VoiceIoShared>, session_id: &str, conn_id: u64) {
    shared.drop_session(session_id, conn_id).await;
}

/// Start the one task allowed to wait for this connection.
///
/// # The hop that #593 added
///
/// Between the registry and a connection there is now a task instead of a
/// channel. It exists for one reason: **somebody has to be allowed to wait for
/// a client, and it must not be [`run_io`]**. Backpressure stays block-only
/// (spec § 2) — a client that is merely slow still gets every command, in
/// order, because this task parks on its channel until there is room. What
/// changed is *who* parks: the loop that reads a `Rebind` no longer does.
///
/// The wait is capped by the cell's operation timeout (hard rule 12). A
/// connection that cannot take one command within `external_timeout` is not
/// slow; it is a socket whose reader has stopped. This task then gives up on it
/// — reporting every command it still holds, so a `Speak` ends as a failure on
/// the handler's error lane rather than as a synthesis that never answers —
/// and takes the session out of the registry with a `Disconnected`. The other
/// way to the same end is a count rather than a clock: once [`DISPATCH_QUEUE`]
/// commands wait behind an already full connection channel,
/// [`VoiceIoShared::send_to`] gives up without waiting for the deadline.
fn spawn_delivery(
    shared: Arc<VoiceIoShared>,
    session_id: String,
    conn_id: u64,
    to_conn: mpsc::Sender<ToConnection>,
) -> mpsc::Sender<ToConnection> {
    let (dispatch, rx) = mpsc::channel(DISPATCH_QUEUE);
    tokio::spawn(deliver(shared, session_id, conn_id, to_conn, rx));
    dispatch
}

/// Forward commands to one connection until it stops taking them.
async fn deliver(
    shared: Arc<VoiceIoShared>,
    session_id: String,
    conn_id: u64,
    to_conn: mpsc::Sender<ToConnection>,
    mut rx: mpsc::Receiver<ToConnection>,
) {
    // `recv` yields what is already buffered before it yields `None`, so the
    // ordinary ending — the registry dropped this session's sender — delivers
    // the backlog first and needs no drain of its own.
    while let Some(cmd) = rx.recv().await {
        // `reserve`, not `send`: it is cancel-safe, so the timeout below cannot
        // swallow the command it was waiting to hand over.
        let why = match tokio::time::timeout(shared.external_timeout, to_conn.reserve()).await {
            Ok(Ok(permit)) => {
                permit.send(cmd);
                continue;
            }
            Ok(Err(_)) => "the connection ended",
            Err(_) => "the client is not reading",
        };
        report_undeliverable(&shared, &session_id, conn_id, cmd, why).await;
        // Nothing else will be taken: refuse further commands (`send_to` then
        // reports them itself) and speak for everything already queued.
        rx.close();
        while let Some(cmd) = rx.recv().await {
            report_undeliverable(&shared, &session_id, conn_id, cmd, why).await;
        }
        shared.drop_session(&session_id, conn_id).await;
        return;
    }
}

/// Say, on the handler's lanes, that one command never reached its client.
///
/// A `Speak` is the one command the handler is *waiting* on — it counts on
/// exactly one `SpeakEnded` per `Speak` — so it gets the failure it would
/// otherwise wait for forever. The others need no answer of their own: the
/// `Disconnected` that follows is what the handler acts on, and a frame for a
/// connection that is over is not news twice.
///
/// # Why the report is addressed, not just named
///
/// A `SpeakEnded` carries a *session identity*, and this backlog belongs to one
/// *connection*. Between the two there is a gap: a client that reconnects with
/// the same `?session=` displaces the old connection, and a rebind evicts every
/// connection of the old address. A report sent without asking whether
/// `conn_id` still holds `session_id` would land on the successor — a live call
/// told that a synthesis it never ordered failed — or on nobody at all. So the
/// event only goes out while this connection is still the session's; otherwise
/// the loss is a log line, which is what it is: nobody is waiting for it any
/// more.
async fn report_undeliverable(
    shared: &VoiceIoShared,
    session_id: &str,
    conn_id: u64,
    cmd: ToConnection,
    why: &str,
) {
    match cmd {
        ToConnection::Speak { speak_id, .. } if shared.holds(session_id, conn_id).await => {
            shared
                .emit(VoiceEvent::SpeakEnded {
                    session_id: session_id.to_string(),
                    speak_id,
                    reason: SpeakEndReason::Failed,
                    detail: Some(format!("nothing was said: {why}")),
                })
                .await;
        }
        _ => {
            tracing::warn!(%session_id, %why, "voice: a command did not reach its client");
        }
    }
}

/// A fresh connection id and the channel its task will listen on.
pub(crate) fn new_connection_slot() -> (
    u64,
    mpsc::Sender<ToConnection>,
    mpsc::Receiver<ToConnection>,
) {
    let (tx, rx) = mpsc::channel(64);
    (next_conn_id(), tx, rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::providers::echo::EchoStt;

    /// GH #592 — A1′: a server future that ends parks the round, it does not
    /// end `run_io`.
    ///
    /// `axum::serve` without graceful shutdown never finishes on its own, so
    /// the arm that used to answer it with `Round::Done` was unreachable in
    /// practice — and the "io-finish-first" loss class it opened was therefore
    /// invisible. The [`serve_forever`] seam makes it reachable: a factory
    /// whose server ends the moment it is polled drives the loop through the
    /// case on its very first round.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn gh592_a_finished_server_parks_instead_of_ending_the_io_half() {
        /// The failure-marker window (30 s convention): only ever longer than a
        /// healthy run takes, so it discriminates nothing but a hang.
        const MARKER: Duration = Duration::from_secs(30);

        let port = meclaw_testing::free_port();
        let (events_tx, mut events_rx) = mpsc::channel::<VoiceEvent>(32);
        let (reconfig_tx, reconfig_rx) = mpsc::channel::<VoiceReconfig>(8);
        let (commands_tx, commands_rx) = mpsc::channel::<VoiceReconfig>(8);
        let mut io = VoiceIo::new(
            "127.0.0.1".to_string(),
            port,
            Arc::new(EchoStt::new()),
            None,
            Mode::Auto,
            Duration::from_secs(5),
            Duration::from_secs(5),
            events_tx,
        );
        io.from_handler = Some(commands_rx);

        let join = tokio::spawn(run_io_with(io, reconfig_rx, |_l, _r| async {}));

        // The round begins as any other does, and then the server stops.
        let bound = events_rx.recv().await;
        assert!(
            matches!(bound, Some(VoiceEvent::Bound(_))),
            "the first round binds before it serves; got {bound:?}"
        );
        let ended = events_rx.recv().await;
        let Some(VoiceEvent::BindFailed(why)) = ended else {
            panic!("a server that stopped must be reported like a bind that failed; got {ended:?}")
        };
        assert!(
            why.contains("listener ended") && why.contains(&port.to_string()),
            "the report names the address that stopped answering; got {why:?}"
        );

        // The point of the issue: the half is still there, parked without a
        // listener rather than returned.
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(
            !join.is_finished(),
            "A1′: `run_io` must not return while the handler is still live"
        );

        // And it is still listening to its handler: a later address is served.
        let next = meclaw_testing::free_port();
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        commands_tx
            .send(VoiceReconfig::Rebind {
                bind: "127.0.0.1".to_string(),
                port: next,
                ack: ack_tx,
            })
            .await
            .expect("the parked half still reads its command channel");
        // Under the failure-marker window, not open-ended: a regression here
        // would be a parked half that never answers, and a hung test says less
        // than a failed one.
        let verdict = tokio::time::timeout(MARKER, ack_rx)
            .await
            .expect("the parked half answers its rebind")
            .expect("a verdict");
        assert!(
            verdict.is_ok(),
            "a free address is bindable from the parked state"
        );
        let rebound = tokio::time::timeout(MARKER, events_rx.recv())
            .await
            .expect("the rebind is reported");
        let Some(VoiceEvent::Bound(addr)) = rebound else {
            panic!("the rebind puts the cell back on the air; got {rebound:?}")
        };
        assert!(
            addr.ends_with(&format!(":{next}")),
            "bound to the new address; got {addr:?}"
        );

        // Only the handler going away ends it — either seam closing is that.
        drop(reconfig_tx);
        drop(commands_tx);
        tokio::time::timeout(MARKER, join)
            .await
            .expect("closing the command channels is what ends `run_io`")
            .expect("and it ends without panicking");
    }
}
