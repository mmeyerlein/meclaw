//! GH #660 — an upgraded socket ends with the cell, and the next life answers.
//!
//! A `voice` cell serves its socket on a task axum spawned. Nothing in the I/O
//! half holds a handle to that task, so when the half goes away the socket is
//! the one thing that could be left hanging: a client on a line nobody is
//! listening to, with no frame to tell it so. Since GH #654 it is not. The half
//! keeps a watch channel that closes with it, every upgraded connection polls
//! that watch first in its own `select!`, and what the client reads is `1001` —
//! the same code the ordinary end of a connection uses, so a client has one
//! story either way.
//!
//! Three facts, in the order a respawn produces them:
//!
//! (a) a client that connected to the live cell gets `hello`;
//! (b) when the half ends, that socket is closed with `1001` rather than left
//!     to time out somewhere;
//! (c) the next life registers the same mount and accepts a new socket.
//!
//! It pins the sentence in `docs/cell-types.en.md` § `voice`: an upgraded socket
//! is the cell's, and it ends with it.

use meclaw_cells::voice::cell::VoiceReconfig;
use meclaw_cells::voice::io::{VoiceIo, run_io};
use meclaw_cells::voice::providers::echo::EchoStt;
use meclaw_cells::voice::wire::Mode;
use meclaw_colony::SurfaceRegistry;
use meclaw_core::Path;
use meclaw_testing::surface_listener;
use meclaw_testing::voice_client::VoiceClient;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// The repo's failure-marker window (30 s convention), never a budget.
const MARKER: Duration = Duration::from_secs(30);
/// The mount both lives of this file's cell register.
const MOUNT: &str = "voice";
/// The close code the wire protocol reserves for "this half is going away".
const GOING_AWAY: u16 = 1001;

/// One life of the I/O half, behind a listener of its own.
struct Life {
    /// `ws://<listener>/<mount>` — the prefix the socket route hangs off.
    ws_base: String,
    task: tokio::task::JoinHandle<()>,
    listener: tokio::task::JoinHandle<()>,
    /// The seam that ends this life: closing it is what a colony does when the
    /// handler half returns, and the only thing `run_io` ends on.
    reconfig_tx: Option<mpsc::Sender<VoiceReconfig>>,
    /// Held for the length of the life. The events seam closing would be a
    /// second way out, and this test is about the one it names.
    _events_rx: mpsc::Receiver<meclaw_cells::voice::cell::VoiceEvent>,
}

impl Life {
    /// Start `run_io` on `surfaces`, put a listener in front of it and wait for
    /// the mount to be on the table.
    async fn start(surfaces: &Arc<SurfaceRegistry>) -> Self {
        let (events_tx, events_rx) = mpsc::channel(64);
        let (reconfig_tx, reconfig_rx) = mpsc::channel(64);
        let mut io = VoiceIo::new(
            MOUNT.to_string(),
            Arc::new(EchoStt::new()),
            None,
            Mode::Auto,
            Duration::from_secs(5),
            Duration::from_secs(30),
            events_tx,
        );
        io.cell_path = Path::new("/voice");
        io.surfaces = Arc::clone(surfaces);
        let task = tokio::spawn(run_io(io, reconfig_rx));
        let (addr, listener) = surface_listener(Arc::clone(surfaces)).await;
        await_mount(surfaces).await;
        Self {
            ws_base: format!("ws://{addr}/{MOUNT}"),
            task,
            listener,
            reconfig_tx: Some(reconfig_tx),
            _events_rx: events_rx,
        }
    }

    /// End this life the way a colony ends it, and wait for the half to be gone.
    async fn end(&mut self) {
        self.reconfig_tx = None;
        tokio::time::timeout(MARKER, &mut self.task)
            .await
            .expect("the I/O half returns when its command seam closes")
            .expect("and returns rather than panicking");
        self.listener.abort();
    }
}

/// Wait until a cell has put its mount on `surfaces`.
async fn await_mount(surfaces: &Arc<SurfaceRegistry>) {
    tokio::time::timeout(MARKER, async {
        while surfaces.table().await.is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the cell registers its mount within the failure marker");
}

/// Wait until the mount table is empty again — the life before this one gave
/// its name back.
async fn await_no_mount(surfaces: &Arc<SurfaceRegistry>) {
    tokio::time::timeout(MARKER, async {
        while !surfaces.table().await.is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the mount goes back on the table when the life ends");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_upgraded_socket_ends_with_the_cell() {
    let surfaces = Arc::new(SurfaceRegistry::new());
    let mut first = Life::start(&surfaces).await;

    let (mut client, hello) = VoiceClient::connect(&format!("{}/ws?session=one", first.ws_base))
        .await
        .expect("the live cell accepts a socket");
    assert_eq!(
        hello["session_id"], "one",
        "and it is the session asked for"
    );

    first.end().await;

    // The close is the point: a socket that merely stopped answering is a line
    // nobody is on, and the client could not tell that from a quiet call.
    let frames = client
        .collect_until(|f| f.as_close().is_some(), MARKER)
        .await;
    assert_eq!(
        frames.last().and_then(|(_, f)| f.as_close()),
        Some(GOING_AWAY),
        "the socket of a cell that is going away is closed with 1001: {frames:?}"
    );

    // And the name comes back with it, so the next life can take it.
    await_no_mount(&surfaces).await;
    let mut second = Life::start(&surfaces).await;
    let (_client, hello) = VoiceClient::connect(&format!("{}/ws?session=two", second.ws_base))
        .await
        .expect("the next life accepts a socket of its own");
    assert_eq!(
        hello["session_id"], "two",
        "a respawn serves the same mount, and nothing of the old socket is in the way"
    );

    second.end().await;
}
