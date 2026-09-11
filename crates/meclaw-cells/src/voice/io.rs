//! The I/O half of the `voice` cell: the mount, the session registry, and the
//! loop that keeps both alive for the cell's whole life (wave voice-cell).
//!
//! This half owns the mount the cell is reached under, the WebSocket
//! connections handed to it under that name and the provider sockets those
//! connections drive. It binds nothing: since `voice@2.0.0` a surface cell has
//! no port of its own and is reached at `/<mount>/…` on the colony's one
//! listener (ADR-0031, superseding ADR-0014). It holds no cell state, no
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
//! [`run_io`] is not. It reads the handler's commands and the streams the
//! colony's listener hands over out of the same loop, so a wait for one client
//! is a wait for every other client's first frame — and a command queued behind
//! a client that had stopped reading was simply never seen.
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
//!
//! The count says so on the handler's error lane as well (GH #601): the burst
//! path emits one [`VoiceEvent::ClientTooSlow`] carrying how many queued
//! commands went with the session, so a colony reading its own lanes can tell a
//! caller who hung up from a client that stopped reading.

use meclaw_colony::{HandedConnection, IoLivenessMark, Registration, SurfaceEntry};
use meclaw_core::Path;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{Mutex, mpsc, watch};

use crate::voice::cell::{VoiceEvent, VoiceReconfig};
use crate::voice::contract::{SttProvider, TtsProvider};
use crate::voice::service::{VoiceLinkOpener, mounted_router};
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
    /// Who is connected.
    sessions: Mutex<Registry>,
    /// Resolves when the I/O half is gone; nobody ever sends on it.
    ///
    /// # Why a channel nobody ever sends on
    ///
    /// The substrate ends a long-running cell by **aborting** `run_io` the
    /// moment its handler half returns (`cell_task_long_running`), so a line
    /// after the loop is not a shutdown path — a dropped future is. Everything
    /// that has to happen on the way out therefore hangs off a `Drop`: the
    /// mount off [`MountGuard`], the handed HTTP connections off a `JoinSet`,
    /// and the upgraded sockets off this. An upgraded WebSocket is the one
    /// nothing here can abort — axum's `on_upgrade` runs it on a task of its
    /// own — so it watches this instead and closes itself with `1001`.
    ///
    /// A caller whose socket reads a close frame knows the call is over. One
    /// whose socket merely stops answering holds a line nobody is on, which is
    /// what a respawn must not cost a person on the telephone.
    ///
    /// The arm is polled once per **iteration** of the connection's own
    /// `select!` loop, `biased` and first, so it wins the iteration it is in —
    /// and the close is as prompt as that iteration, no prompter. A connection
    /// parked inside an arm body stays parked: `send_frame` on a socket the
    /// client has stopped reading, or `audio_tx.send` while the recognition
    /// provider is behind, reaches the select point only when that send returns.
    /// That window is GH #593's wedged client, and it is a property of the loop
    /// rather than of this channel.
    ///
    /// `None` in a fixture built by hand: nothing watches, nothing closes.
    shutdown: Option<watch::Receiver<()>>,
}

/// The connection table: who is connected, under which session identity.
#[derive(Default)]
struct Registry {
    /// Who is connected, under which session identity.
    live: HashMap<String, SessionHandle>,
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
        registry
            .live
            .insert(session_id.to_string(), SessionHandle { conn_id, dispatch })
            .map(|old| old.dispatch)
    }

    /// Give up `session_id`, but only if `conn_id` still holds it.
    ///
    /// A displaced connection ends after its successor registered, so an
    /// unconditional remove would delete the live entry. The verdict is also
    /// what decides who reports the `Disconnected`: the task that really left
    /// the table, exactly once.
    async fn release(&self, session_id: &str, conn_id: u64) -> bool {
        let mut registry = self.sessions.lock().await;
        match registry.live.get(session_id) {
            Some(h) if h.conn_id == conn_id => {
                registry.live.remove(session_id);
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
    /// reconnects with the same `?session=` displaces the old connection.
    /// Reporting without asking would hand the **new** connection's session the
    /// failure of the old one's synthesis, or speak for a session that is
    /// already gone.
    async fn holds(&self, session_id: &str, conn_id: u64) -> bool {
        self.sessions
            .lock()
            .await
            .live
            .get(session_id)
            .is_some_and(|h| h.conn_id == conn_id)
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
    /// A command that does not fit is **not dropped in silence**: the burst is
    /// reported as one [`VoiceEvent::ClientTooSlow`] with the number of queued
    /// commands it took with it (GH #601), a `Speak` among them becomes a
    /// failed [`VoiceEvent::SpeakEnded`], and the session is given up with a
    /// `Disconnected` either way. Two full buffers is not backpressure any
    /// more, it is a connection nobody is on.
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
                // The count that decided, before the session is given up on
                // (GH #601): everything the dispatch queue still holds, plus
                // the command that no longer fitted. Read off the channel
                // rather than written as `DISPATCH_QUEUE + 1`, so the number in
                // the message cannot drift from the queue that produced it.
                let dropped = dispatch.max_capacity() + 1;
                self.emit(VoiceEvent::ClientTooSlow {
                    session_id: session_id.to_string(),
                    dropped,
                })
                .await;
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
    /// The only `.await` this leaves in the command loop is [`Self::emit`], and
    /// that one is the handler waiting for itself: the events channel is the
    /// handler's own seam, block-only by the spec. What #593 was about —
    /// waiting for a *client* — is gone from this loop.
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
    }

    /// A watch on this life, for a task nothing here can abort.
    ///
    /// See [`Self::shutdown`] for why the channel exists and why nobody ever
    /// sends on it.
    pub(crate) fn shutdown(&self) -> Option<watch::Receiver<()>> {
        self.shutdown.clone()
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
    /// The name this cell registers on the mount table, from `params.mount`.
    ///
    /// The only door the cell has. Read once, at the top of the life — a name
    /// that moved takes effect on the next one (ruling O-P-2, see
    /// [`crate::voice::params::VoiceParams`]).
    pub mount: String,
    /// Where this cell sits in the tree, for the registry entry.
    ///
    /// The mount table refuses a name another path holds and lets the holder
    /// replace its own entry, which is what a respawn is — so the entry has to
    /// carry the path even though the cell never learns anything from it.
    pub cell_path: Path,
    /// The process's mount table, set by the factory.
    ///
    /// A table of this half's own by default, so a cell built outside a colony
    /// (a fixture, a unit test) registers somewhere harmless instead of nowhere.
    pub surfaces: Arc<meclaw_colony::SurfaceRegistry>,
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
        mount: String,
        stt: Arc<dyn SttProvider>,
        tts: Option<Arc<dyn TtsProvider>>,
        default_mode: Mode,
        external_timeout: Duration,
        idle_timeout: Duration,
        events_tx: mpsc::Sender<VoiceEvent>,
    ) -> Self {
        Self {
            mount,
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
            // Replaced by the factory, which is the only caller that knows it.
            // A half built by hand registers nothing, so nothing reads this.
            cell_path: Path::new(""),
            surfaces: Arc::new(meclaw_colony::SurfaceRegistry::new()),
            from_handler: None,
        }
    }
}

/// Holds a mount for exactly as long as the I/O half that registered it.
///
/// The ordinary end is still the explicit `unregister` when the handler goes
/// away, and this changes nothing about it. What it covers is every OTHER end:
/// a panic in the handler half, the `message_timeout` backstop, an abort. None
/// of them runs that arm, and the entry that stayed behind kept a live
/// [`VoiceLinkOpener`] over shared state nobody serves — a page joining in that
/// window was admitted and then heard nothing.
struct MountGuard {
    /// The table the mount stands in.
    surfaces: Arc<meclaw_colony::SurfaceRegistry>,
    /// The name this life registered.
    mount: String,
    /// The token this life registered under. A spent one removes nothing, which
    /// is what makes a respawn's entry safe from the previous life's guard.
    registration: Registration,
}

impl Drop for MountGuard {
    fn drop(&mut self) {
        // A `Drop` cannot await, and the registry is behind an `Arc`, so the
        // removal is a task of its own. Only on a runtime thread: a guard
        // dropped outside one has no executor to spawn onto, and a process
        // without a runtime has no mount table left to keep tidy either.
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let surfaces = Arc::clone(&self.surfaces);
        let mount = std::mem::take(&mut self.mount);
        let registration = self.registration;
        handle.spawn(async move {
            surfaces.unregister(&mount, &registration).await;
        });
    }
}

/// The I/O loop: register the mount, serve what the listener hands over, and
/// stay up for the cell's whole life.
///
/// **A1′**: this function must not return voluntarily while the cell is live.
/// Only the handler closing one of its two command channels ends it. There is
/// no second round any more and nothing to rebind: the cell owns no socket, and
/// a mount that moved is read by the next life (ruling O-P-2).
pub async fn run_io(mut io: VoiceIo, mut reconfig_rx: mpsc::Receiver<VoiceReconfig>) {
    // Two command seams, one loop. The substrate hands `run_io` its own
    // `reconfig_rx`, but only `handle` is given the matching sender — and this
    // cell issues most of its commands from `handle_event`. So the cell mints
    // its own pair and puts the receiver in `from_handler` (see [`VoiceIo`]).
    // Both are drained here: either one closing means the handler is gone,
    // which is the only thing that ends this function (A1′). A cell built
    // outside a colony leaves `from_handler` empty and speaks on the
    // substrate's channel alone.
    let mut from_handler = io.from_handler.take();
    // The two ends of the way out, and both are LOCALS of this future on
    // purpose: the substrate aborts `run_io` when the handler half returns, so
    // what runs at shutdown is what a dropped future drops, never a line after
    // the loop. `shutdown_tx` closes for the upgraded sockets; `connections`
    // aborts the handed HTTP ones.
    let (shutdown_tx, shutdown_rx) = watch::channel(());
    let mut connections = tokio::task::JoinSet::new();
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
        shutdown: Some(shutdown_rx),
    });
    let mount = io.mount;
    let cell_path = io.cell_path;
    let surfaces = io.surfaces;

    // The mount goes on the table once per life, at the top of it (ADR-0031):
    // whoever has a connection to hand over must find this cell as soon as it
    // exists, and a name taken by another cell is an operator's mistake to read
    // — not a reason to tear the cell down. The cell then serves nobody and
    // says so, and a `params` update naming a free name is served by this same
    // task on the next life.
    //
    // The token comes back with the receiver and is what the unregister at the
    // end of this life must carry: a respawn that registered while this half was
    // still draining holds a newer one, and a spent token removes nothing.
    // It is kept in a [`MountGuard`], so an end that never reaches the ordinary
    // arm — a panic, the backstop, an abort — takes the mount with it as well.
    let entry = SurfaceEntry {
        kind: "voice",
        cell_path,
        // A topic on a display's socket is answered by the same admission the
        // socket door runs, on the same shared state (GH #643).
        links: Some(Arc::new(VoiceLinkOpener {
            shared: shared.clone(),
        })),
    };
    let (mut handoff, registration) = match surfaces.register(&mount, entry).await {
        Ok((rx, held)) => (
            Some(rx),
            Some(MountGuard {
                surfaces: Arc::clone(&surfaces),
                mount: mount.clone(),
                registration: held,
            }),
        ),
        Err(e) => {
            shared.emit(VoiceEvent::MountFailed(e.to_string())).await;
            (None, None)
        }
    };

    // One round, for the whole life: this returns when the handler is gone —
    // and usually not even that, because the substrate aborts this future the
    // moment the handler returns. Both ways out are the same way out, because
    // everything that has to happen is a `Drop`:
    //
    // * `registration` gives the mount back ([`MountGuard`]), so the listener
    //   stops handing streams to a cell that is going;
    // * `connections` is a `JoinSet`, so every handed connection that did not
    //   upgrade — an in-flight `GET /<mount>/info`, an idle keep-alive socket,
    //   the second connection a browser pools — ends with it. Detached, they
    //   outlived the cell: a respawn answered a client out of the previous
    //   life's tables, and the client never learned it had to reconnect.
    //   `axum::serve` took its connections with it when the round dropped it,
    //   and this set is how that survives the move to a handed stream;
    // * `shutdown_tx` closes, and every upgraded socket — which no `JoinSet`
    //   here holds, because axum's `on_upgrade` runs it — reads that and sends
    //   its client a close frame ([`VoiceIoShared::shutdown`]).
    //
    // A line after this call would run on exactly one of the two paths, which
    // is why the three drops below are all there is.
    serve_until_the_handler_goes(
        &shared,
        &mut reconfig_rx,
        &mut from_handler,
        &mut handoff,
        &mount,
        &mut connections,
    )
    .await;
    drop(shutdown_tx);
    drop(connections);
    drop(registration);
}

/// Serve the handler's commands and the streams the listener hands over, until
/// the handler goes away.
///
/// Everything addressed at a session is dispatched here and the loop continues;
/// only the handler going away — either seam closing — ends it.
async fn serve_until_the_handler_goes(
    shared: &Arc<VoiceIoShared>,
    reconfig_rx: &mut mpsc::Receiver<VoiceReconfig>,
    from_handler: &mut Option<mpsc::Receiver<VoiceReconfig>>,
    handoff: &mut Option<mpsc::Receiver<HandedConnection>>,
    mount: &str,
    connections: &mut tokio::task::JoinSet<()>,
) {
    loop {
        let command = tokio::select! {
            // `biased;` so the order is a decision, not a coin toss: the
            // substrate's own seam first. It is a tiebreak rather than a fix —
            // what keeps this loop moving is that no arm below waits on a
            // client (GH #593).
            biased;

            c = reconfig_rx.recv() => c,
            c = recv_opt(from_handler) => c,

            // A connection the colony's listener accepted for this cell's
            // mount. Served on a task of its own, for the whole life of the
            // connection: nothing in this loop ends it, and the life around
            // the loop owns it (see the `JoinSet` in [`run_io`]).
            Some(handed) = recv_opt_handed(handoff) => {
                let router = mounted_router(shared.clone(), mount);
                connections.spawn(crate::handed::serve_handed(handed.stream, router));
                continue;
            }

            // Reaping, so the set does not grow with every request that was
            // ever answered. Guarded, because `join_next` on an empty set is
            // `None` at once and would spin this loop.
            Some(_) = connections.join_next(), if !connections.is_empty() => continue,
        };
        match command {
            // A closed channel is the handler going away, on either seam.
            None => return,
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

/// Receive a handed connection, or wait forever when this cell has no mount.
async fn recv_opt_handed(
    rx: &mut Option<mpsc::Receiver<HandedConnection>>,
) -> Option<HandedConnection> {
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
    use meclaw_colony::HandedConnection;

    /// A mount another cell holds is refused out loud, and the life goes on.
    ///
    /// The one error path of the mount work, and the only assertion about it
    /// used to be a negative one. Two things rot silently without this: the
    /// wrong variant, and an early `return` creeping into the `Err` arm — a
    /// taken name is explicitly not allowed to end a cell that a `params`
    /// update could still put on a free one.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_mount_another_cell_holds_is_refused_and_the_half_stays_alive() {
        const MARKER: Duration = Duration::from_secs(30);
        let surfaces = Arc::new(meclaw_colony::SurfaceRegistry::new());
        // Somebody else is already on `voice`.
        let (_squatter_rx, _squatter) = surfaces
            .register(
                "voice",
                SurfaceEntry {
                    kind: "voice",
                    cell_path: Path::new("/elsewhere/voice"),
                    links: None,
                },
            )
            .await
            .expect("the mount was free");

        let (events_tx, mut events_rx) = mpsc::channel::<VoiceEvent>(32);
        let (reconfig_tx, reconfig_rx) = mpsc::channel::<VoiceReconfig>(8);
        let mut io = VoiceIo::new(
            "voice".to_string(),
            Arc::new(EchoStt::new()),
            None,
            Mode::Auto,
            MARKER,
            MARKER,
            events_tx,
        );
        io.cell_path = Path::new("/v");
        io.surfaces = surfaces.clone();
        let join = tokio::spawn(run_io(io, reconfig_rx));

        // Exactly one refusal, and it names the mount and the holder.
        match tokio::time::timeout(MARKER, events_rx.recv())
            .await
            .expect("the refusal is reported")
            .expect("the channel is open")
        {
            VoiceEvent::MountFailed(detail) => {
                assert!(
                    detail.contains("voice"),
                    "the refusal names the mount; got {detail}"
                );
                assert!(
                    detail.contains("/elsewhere/voice"),
                    "and who holds it; got {detail}"
                );
            }
            other => panic!("expected MountFailed; got {other:?}"),
        }

        // The life goes on, and the receipt is positive rather than a clock:
        // an `ArmReleaseGrace` with no delay is a command only a loop that is
        // still reading its seam can answer, and its answer comes back on the
        // events channel with the token it was given. That is what a later
        // `params` update with a free name needs, and it is what the sleep
        // this replaces could only hope for.
        reconfig_tx
            .send(VoiceReconfig::ArmReleaseGrace {
                session_id: "nobody".to_string(),
                ms: 0,
                token: 7,
            })
            .await
            .expect("the half still takes a command");
        match tokio::time::timeout(MARKER, events_rx.recv())
            .await
            .expect("the parked half answers within the failure marker")
            .expect("the channel is open")
        {
            VoiceEvent::ReleaseGraceExpired { token, .. } => assert_eq!(
                token, 7,
                "A1′: a taken mount must not end the I/O half, and the answer is the \
                 token the command carried"
            ),
            other => panic!("expected the command's own answer; got {other:?}"),
        }
        assert!(
            !join.is_finished(),
            "A1′: a taken mount must not end the I/O half"
        );

        // The squatter still holds the name, and this half never took it.
        let table = surfaces.table().await;
        assert_eq!(table.len(), 1, "one holder, and it is not this cell");
        assert_eq!(table[0].mount, "voice");

        drop(reconfig_tx);
        tokio::time::timeout(MARKER, join)
            .await
            .expect("ends")
            .expect("no panic");
        assert_eq!(
            surfaces.table().await.len(),
            1,
            "a half that never registered removes nothing on its way out"
        );
    }

    /// ADR-0031: the mount is in the table for the whole life of the I/O half,
    /// and a stream handed to it is answered by the cell's own router under the
    /// mount's prefix.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_mounted_voice_cell_registers_on_its_life_and_serves_a_handed_stream() {
        const MARKER: Duration = Duration::from_secs(30);
        let surfaces = Arc::new(meclaw_colony::SurfaceRegistry::new());
        let (events_tx, mut events_rx) = mpsc::channel::<VoiceEvent>(32);
        let (reconfig_tx, reconfig_rx) = mpsc::channel::<VoiceReconfig>(8);
        let mut io = VoiceIo::new(
            "voice".to_string(),
            Arc::new(EchoStt::new()),
            None,
            Mode::Auto,
            MARKER,
            MARKER,
            events_tx,
        );
        io.cell_path = meclaw_core::Path::new("/v");
        io.surfaces = surfaces.clone();
        let join = tokio::spawn(run_io(io, reconfig_rx));
        // The mount is in the table before anything else happens.
        tokio::time::timeout(MARKER, async {
            while surfaces.table().await.is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await
            }
        })
        .await
        .expect("registered");
        assert_eq!(surfaces.table().await[0].mount, "voice");
        // A stream handed in is answered by the cell's router under /voice.
        let l = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = l.local_addr().expect("addr");
        let handoff = surfaces.take_handoff("voice").await.expect("mounted");
        tokio::spawn(async move {
            let (s, p) = l.accept().await.expect("accept");
            handoff
                .send(HandedConnection { stream: s, peer: p })
                .await
                .expect("handed");
        });
        let info: meclaw_core::serde_json::Value =
            reqwest::get(format!("http://{addr}/voice/info"))
                .await
                .expect("get")
                .json()
                .await
                .expect("json");
        assert_eq!(info["protocol"], "meclaw-voice/1");
        assert!(
            events_rx.try_recv().is_err(),
            "a mount that was free is reported by nothing at all"
        );
        drop(reconfig_tx);
        tokio::time::timeout(MARKER, join)
            .await
            .expect("ends")
            .expect("no panic");
        // Waited out rather than read once: the mount leaves on the
        // [`MountGuard`]'s `Drop`, which cannot await and therefore hands the
        // removal to a task of its own. That is the shape every way out shares
        // — the substrate ABORTS this future rather than letting a line after
        // the loop run — and it is one scheduler turn away from here.
        tokio::time::timeout(MARKER, async {
            while !surfaces.table().await.is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await
            }
        })
        .await
        .expect("the mount leaves with the half");
    }

    /// A handed stream does not outlive the life that was serving it.
    ///
    /// The defect this pins: each handed connection was served on a DETACHED
    /// `tokio::spawn`, so a socket that never upgraded — an idle keep-alive
    /// connection, a browser's second pooled one, an `/info` poller — kept its
    /// task and its clone of this life's shared state after `run_io` returned.
    /// The client was never told to reconnect, and a respawn answered it out
    /// of the tables of a cell that no longer existed. `axum::serve` took its
    /// connections with it when the round dropped it; the `JoinSet` in
    /// [`run_io`] is how that survives the move to a handed stream.
    ///
    /// The receipt is the socket itself: a read that returns zero bytes is the
    /// close, and it has to arrive inside the failure marker rather than at
    /// the end of the process.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_handed_connection_is_closed_when_the_handler_goes() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        const MARKER: Duration = Duration::from_secs(30);
        let surfaces = Arc::new(meclaw_colony::SurfaceRegistry::new());
        let (events_tx, _events_rx) = mpsc::channel::<VoiceEvent>(32);
        let (reconfig_tx, reconfig_rx) = mpsc::channel::<VoiceReconfig>(8);
        let mut io = VoiceIo::new(
            "voice".to_string(),
            Arc::new(EchoStt::new()),
            None,
            Mode::Auto,
            MARKER,
            MARKER,
            events_tx,
        );
        io.cell_path = meclaw_core::Path::new("/v");
        io.surfaces = surfaces.clone();
        let join = tokio::spawn(run_io(io, reconfig_rx));
        tokio::time::timeout(MARKER, async {
            while surfaces.table().await.is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await
            }
        })
        .await
        .expect("registered");

        // One stream, handed over the way the colony's listener hands it.
        let l = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = l.local_addr().expect("addr");
        let handoff = surfaces.take_handoff("voice").await.expect("mounted");
        let feeder = tokio::spawn(async move {
            while let Ok((stream, peer)) = l.accept().await {
                if handoff
                    .send(HandedConnection { stream, peer })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        // A KEEP-ALIVE request: no `Connection: close`, so the connection is
        // the cell's to end and not the client's. Reading the answer proves the
        // stream really reached the cell's router.
        let mut client = tokio::net::TcpStream::connect(addr).await.expect("connect");
        client
            .write_all(b"GET /voice/info HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .expect("write the request");
        let mut answer = [0u8; 512];
        let read = tokio::time::timeout(MARKER, client.read(&mut answer))
            .await
            .expect("the mount answers within the failure marker")
            .expect("read");
        assert!(
            String::from_utf8_lossy(&answer[..read]).starts_with("HTTP/1.1 200 OK"),
            "the handed stream was served: {:?}",
            String::from_utf8_lossy(&answer[..read])
        );

        // An UPGRADED socket beside it. This one no `JoinSet` here holds —
        // axum's `on_upgrade` runs it on a task of its own — so it is the half
        // of the promise the set cannot keep, and the watch is.
        let (mut ws, _) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/voice/ws?session=held"))
                .await
                .expect("the mount answers a websocket");
        let _hello = tokio::time::timeout(MARKER, futures_util::StreamExt::next(&mut ws))
            .await
            .expect("hello arrives")
            .expect("a frame")
            .expect("not an error");

        // The handler goes. Both connections are this life's, so both end with
        // it — and the substrate would ABORT this future rather than let a line
        // after the loop run, which is why nothing here relies on one.
        drop(reconfig_tx);
        tokio::time::timeout(MARKER, join)
            .await
            .expect("run_io returns")
            .expect("no panic");

        // The upgraded socket is told, not merely dropped: a caller that reads
        // a close frame knows the call is over.
        let closed_ws = tokio::time::timeout(MARKER, async {
            loop {
                match futures_util::StreamExt::next(&mut ws).await {
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Close(f))) => return Some(f),
                    Some(Ok(_)) => continue,
                    Some(Err(_)) | None => return None,
                }
            }
        })
        .await
        .expect("the websocket hears about it inside the failure marker");
        let frame = closed_ws.expect("a close FRAME, not a socket that merely stopped answering");
        assert_eq!(
            frame.map(|f| u16::from(f.code)),
            Some(1001),
            "the code is the one the ordinary end of this half uses"
        );
        let mut rest = Vec::new();
        let closed = tokio::time::timeout(MARKER, client.read_to_end(&mut rest))
            .await
            .expect("the socket closes inside the failure marker")
            .expect("read");
        assert_eq!(
            closed, 0,
            "a connection of a life that is over must not stay open; got {rest:?}"
        );
        feeder.abort();
    }

    /// And the same on the path the substrate actually takes: an ABORT.
    ///
    /// `cell_task_long_running` aborts `run_io` the moment the handler half
    /// returns, so a line after the loop is not a shutdown path — a dropped
    /// future is. This is the same promise as the test above, proven on the way
    /// out that leaves no code of ours running at all.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_aborted_io_half_closes_the_sockets_it_was_serving() {
        use futures_util::StreamExt;

        const MARKER: Duration = Duration::from_secs(30);
        let surfaces = Arc::new(meclaw_colony::SurfaceRegistry::new());
        let (events_tx, _events_rx) = mpsc::channel::<VoiceEvent>(32);
        let (_reconfig_tx, reconfig_rx) = mpsc::channel::<VoiceReconfig>(8);
        let mut io = VoiceIo::new(
            "voice".to_string(),
            Arc::new(EchoStt::new()),
            None,
            Mode::Auto,
            MARKER,
            MARKER,
            events_tx,
        );
        io.cell_path = meclaw_core::Path::new("/v");
        io.surfaces = surfaces.clone();
        let join = tokio::spawn(run_io(io, reconfig_rx));
        tokio::time::timeout(MARKER, async {
            while surfaces.table().await.is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await
            }
        })
        .await
        .expect("registered");

        let l = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = l.local_addr().expect("addr");
        let handoff = surfaces.take_handoff("voice").await.expect("mounted");
        let feeder = tokio::spawn(async move {
            while let Ok((stream, peer)) = l.accept().await {
                if handoff
                    .send(HandedConnection { stream, peer })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        let (mut ws, _) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/voice/ws?session=aborted"))
                .await
                .expect("the mount answers a websocket");
        let _hello = tokio::time::timeout(MARKER, ws.next())
            .await
            .expect("hello arrives")
            .expect("a frame")
            .expect("not an error");

        join.abort();

        let frame = tokio::time::timeout(MARKER, async {
            loop {
                match ws.next().await {
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Close(f))) => return Some(f),
                    Some(Ok(_)) => continue,
                    Some(Err(_)) | None => return None,
                }
            }
        })
        .await
        .expect("the socket hears about the abort inside the failure marker")
        .expect("a close FRAME, not a socket that merely stopped answering");
        assert_eq!(frame.map(|f| u16::from(f.code)), Some(1001));
        feeder.abort();
    }

    /// An ABORTED I/O half takes its mount with it too (GH #639).
    ///
    /// The ordinary end unregisters on `Round::Done`. Every other end does not
    /// reach that arm: a panic in the handler half, the `message_timeout`
    /// backstop, any abort. The entry then stood in the table with a live
    /// opener over dead shared state — a page joining in that window was
    /// ADMITTED, got its `hello` and heard nothing after it, while a stream
    /// handed to the mount met a receiver nobody holds. A cell whose task dies
    /// stops answering the instant it dies, and the asymmetry is the defect: the
    /// mount kept saying yes.
    ///
    /// Aborting the task is the cheapest way to state "the future was dropped
    /// without that arm running", and it covers all three causes at once.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_aborted_io_half_takes_its_mount_off_the_table() {
        const MARKER: Duration = Duration::from_secs(30);
        let surfaces = Arc::new(meclaw_colony::SurfaceRegistry::new());
        let (events_tx, _events_rx) = mpsc::channel::<VoiceEvent>(32);
        let (_reconfig_tx, reconfig_rx) = mpsc::channel::<VoiceReconfig>(8);
        let mut io = VoiceIo::new(
            "voice".to_string(),
            Arc::new(EchoStt::new()),
            None,
            Mode::Auto,
            MARKER,
            MARKER,
            events_tx,
        );
        io.cell_path = meclaw_core::Path::new("/v");
        io.surfaces = surfaces.clone();
        let join = tokio::spawn(run_io(io, reconfig_rx));
        tokio::time::timeout(MARKER, async {
            while surfaces.table().await.is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await
            }
        })
        .await
        .expect("the mount is registered");

        // No `Round::Done`, no shutdown channel closed: the future is simply
        // gone.
        join.abort();
        tokio::time::timeout(MARKER, async {
            while !surfaces.table().await.is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await
            }
        })
        .await
        .expect("the mount leaves with the aborted half, and does not outlive it");
    }

    /// GH #601 — a client that stops taking what is queued for it loses the
    /// connection, and the cell says so instead of writing a log line.
    ///
    /// The colony side of this cell is backpressure and stays that way
    /// (`voice_t5_behaviour::backpressure_loses_nothing`). The socket side is
    /// not: past [`DISPATCH_QUEUE`] commands behind an already full connection
    /// channel the session is given up on, by count and without a clock. The
    /// ruling of 2026-09-09 on GH #601 is that this verdict is a message —
    /// exactly one [`VoiceEvent::ClientTooSlow`], naming the session and how
    /// many queued commands went with it, before the `Disconnected` that ends
    /// the call.
    ///
    /// No clock decides anything here: `external_timeout` is a failure marker,
    /// far above anything a healthy run needs, so a loaded host cannot turn the
    /// count path into the clock path and make this test measure the wrong one.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn gh601_a_client_that_stops_reading_is_dropped_by_count_and_reported() {
        /// The failure-marker window (30 s convention), not a budget.
        const MARKER: Duration = Duration::from_secs(30);
        /// Two full buffers plus the burst behind them, with room to spare:
        /// the give-up needs `DISPATCH_QUEUE` commands queued behind a full
        /// connection channel, and nothing after it is sent at all.
        const FLOOD: usize = 4 * DISPATCH_QUEUE;

        let (events_tx, mut events_rx) = mpsc::channel::<VoiceEvent>(32);
        let shared = Arc::new(VoiceIoShared {
            stt: Arc::new(EchoStt::new()),
            tts: None,
            default_mode: Mode::Auto,
            external_timeout: MARKER,
            idle_timeout: MARKER,
            audio_out_frame_ms: 20,
            speak_plain: false,
            release_grace_ms: 1500,
            events_tx,
            liveness: meclaw_colony::io_liveness::IoLivenessMark::disabled(),
            sessions: Mutex::new(Registry::default()),
            shutdown: None,
        });

        // A connection nobody reads: the receiver is held and never polled, so
        // the connection channel fills, `deliver` parks on it, and the dispatch
        // queue behind it is what runs over.
        let (conn_id, to_conn, _never_read) = new_connection_slot();
        let dispatch = spawn_delivery(Arc::clone(&shared), "call-1".to_string(), conn_id, to_conn);
        assert!(
            shared.claim("call-1", conn_id, dispatch).await.is_none(),
            "the session was free"
        );

        for _ in 0..FLOOD {
            shared
                .send_to(
                    "call-1",
                    ToConnection::Frame(ServerFrame::Mode { mode: Mode::Auto }),
                )
                .await;
        }

        let first = tokio::time::timeout(MARKER, events_rx.recv())
            .await
            .expect("the give-up is reported without waiting for a clock")
            .expect("the handler seam is open");
        let VoiceEvent::ClientTooSlow {
            session_id,
            dropped,
        } = first
        else {
            panic!("the count path must report itself before it drops the session; got {first:?}")
        };
        assert_eq!(session_id, "call-1", "the report names the call it ended");
        assert_eq!(
            dropped,
            DISPATCH_QUEUE + 1,
            "the number in the report is the queue that decided, plus the command that no \
             longer fitted"
        );

        let second = tokio::time::timeout(MARKER, events_rx.recv())
            .await
            .expect("and the session leaves the table")
            .expect("the handler seam is open");
        assert!(
            matches!(&second, VoiceEvent::Disconnected { session_id } if session_id == "call-1"),
            "the disconnect follows the reason, not the other way round; got {second:?}"
        );

        // Exactly one report: everything after the give-up finds no session and
        // says nothing, because the call it belonged to is over.
        assert!(
            !shared.holds("call-1", conn_id).await,
            "the session is gone from the registry"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
        let mut extra = Vec::new();
        while let Ok(ev) = events_rx.try_recv() {
            extra.push(format!("{ev:?}"));
        }
        assert!(extra.is_empty(), "one client, one verdict: {extra:?}");
    }
}
