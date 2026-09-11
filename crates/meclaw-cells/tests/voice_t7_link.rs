//! GH #643 — the `voice` cell answers a topic the way it answers a socket.
//!
//! One cell, no colony, and no socket anywhere: the client here is a
//! [`meclaw_colony::Link`] taken out of the mount table, which is exactly what a
//! `web` cell's socket loop takes when a page joins `voice:<call>`. The
//! assertions are the four promises the wire protocol makes, checked over the
//! second door:
//!
//! * `hello` is the first frame, and it names the session the join asked for;
//! * a refusal is the sentence the HTTP door would have answered, with its
//!   status — one admission, two doors;
//! * `hold`, 20 ms frames, `release` produce exactly one `turn`, on the link
//!   **and** on the cell's own lane;
//! * a second link for the same session displaces the first with `4409`, and a
//!   client that stops reading is counted out as `client_too_slow`.
//!
//! The handler half is driven by this file, the way `voice_t4_service.rs` drives
//! it: a `turn` frame on the link is the handler's answer to a recognition
//! event, so nothing shorter than both halves proves it arrives.

use meclaw_cells::voice::cell::{VoiceCell, VoiceEvent, VoiceReconfig};
use meclaw_cells::voice::io::VoiceIo;
use meclaw_cells::voice::params::VoiceParams;
use meclaw_cells::voice::providers::{build_stt, build_tts};
use meclaw_cells::voice::wire::{Mode, ServerFrame};
use meclaw_colony::{DbConn, Link, LinkFrame, LinkRequest, LongRunningCell, SurfaceRegistry};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{CellEmission, OriginSink, OutputSink, Path};
use meclaw_testing::mock_cartesia::{CartesiaScript, MockCartesia};
use meclaw_testing::mock_deepgram::{DeepgramScript, MockDeepgram};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// The repo's failure-marker window (30 s convention), never a budget.
const MARKER: Duration = Duration::from_secs(30);
/// 640 bytes = 20 ms of 16 kHz mono PCM16, the frame length the wire recommends.
const FRAME_BYTES: usize = 640;
/// What the scripted recogniser says once the audio really arrived.
const SAID: &str = "what time is it";
/// Two full buffers and the burst behind them: what it takes to be counted out.
const FLOOD: usize = 260;

/// The cell path this file's instance answers under.
const CELL: &str = "/voice";

/// One mounted `voice` cell, both halves, plus the seams a colony would hold.
struct Live {
    surfaces: Arc<SurfaceRegistry>,
    cell: VoiceCell,
    events_rx: mpsc::Receiver<VoiceEvent>,
    out_rx: mpsc::Receiver<CellEmission>,
    origin: OriginSink,
    db: DbConn,
    reconfig_tx: mpsc::Sender<VoiceReconfig>,
    task: tokio::task::JoinHandle<()>,
    /// Held: dropping it would read as a shutdown to the I/O half.
    _out: OutputSink,
    _stt: MockDeepgram,
    _tts: MockCartesia,
}

impl Drop for Live {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// An in-memory `cell.db` with the one table a `voice` cell owns.
fn cell_db() -> DbConn {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS params (
             key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at INTEGER NOT NULL);",
    )
    .expect("the overlay table");
    DbConn::wrap(conn, None)
}

/// Boot a `voice` cell on its mount — the shape a display's microphone reaches.
async fn boot() -> Live {
    let stt_fake = MockDeepgram::start(
        DeepgramScript::new()
            .require_audio_bytes(FRAME_BYTES)
            .turn_info("EndOfTurn", SAID),
    )
    .await
    .expect("the fake deepgram binds");
    let tts_fake = MockCartesia::start(CartesiaScript::new())
        .await
        .expect("the fake cartesia binds");

    let raw = json!({
        "mount": "voice",
        "stt": {
            "provider": "deepgram",
            "api_key": "fake-key",
            "language": "de",
            "sample_rate": 16000,
            "base_url": stt_fake.base_url()
        },
        "tts": {
            "provider": "cartesia",
            "api_key": "fake-key",
            "voice": "fake-voice",
            "language": "de",
            "sample_rate": 24000,
            "base_url": tts_fake.base_url()
        }
    });
    let params = VoiceParams::parse(&raw).expect("a mounted cell parses");
    let timeouts = meclaw_cells::voice::contract::ProviderTimeouts {
        external: Duration::from_millis(params.external_timeout_ms),
        idle: Duration::from_millis(params.provider_idle_timeout_ms),
    };
    let stt = build_stt(&params.stt, timeouts).expect("the recogniser builds");
    let tts = params
        .tts
        .as_ref()
        .map(|t| build_tts(t, timeouts).expect("the synthesiser builds"));

    let path = Path::new(CELL);
    let surfaces = Arc::new(SurfaceRegistry::new());
    let (events_tx, events_rx) = mpsc::channel::<VoiceEvent>(256);
    let mut io = VoiceIo::new(
        params.mount.clone(),
        stt,
        tts,
        params.default_mode,
        Duration::from_millis(params.external_timeout_ms),
        Duration::from_millis(params.provider_idle_timeout_ms),
        events_tx.clone(),
    );
    io.cell_path = path.clone();
    io.surfaces = Arc::clone(&surfaces);
    let mut cell = VoiceCell::new(path.clone(), io, &params, &raw);
    let split = LongRunningCell::split_io(&mut cell);
    let (reconfig_tx, reconfig_rx) = mpsc::channel::<VoiceReconfig>(64);
    let task = tokio::spawn(<VoiceCell as LongRunningCell>::run_io(
        split,
        events_tx,
        reconfig_rx,
    ));

    let (out_tx, out_rx) = mpsc::channel::<CellEmission>(256);
    let origin = OriginSink::new(out_tx.clone(), path.clone(), 64);
    let out = OutputSink::new(
        out_tx,
        path.clone(),
        meclaw_core::Uuid::now_v7(),
        meclaw_core::Uuid::now_v7(),
        64,
        meclaw_core::Headers::new(),
        None,
    );

    // The mount goes on the table at the top of the life; a link before that is
    // a link to nothing.
    tokio::time::timeout(MARKER, async {
        while surfaces.table().await.is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the cell registers its mount");

    Live {
        surfaces,
        cell,
        events_rx,
        out_rx,
        origin,
        db: cell_db(),
        reconfig_tx,
        task,
        _out: out,
        _stt: stt_fake,
        _tts: tts_fake,
    }
}

impl Live {
    /// Open one link on the mount, as a socket loop would.
    async fn link(&self, req: LinkRequest) -> Link {
        self.surfaces
            .open_link("voice", req)
            .await
            .expect("the mount answers")
            .expect("the client is admitted")
    }

    /// Feed the handler every event it is owed until `want` sees a frame it
    /// accepts, and answer with everything that arrived on the link on the way.
    ///
    /// Both sides in one loop on purpose: the handler's answer to a recognition
    /// event travels down the link, so a test that drained one and then the
    /// other would deadlock on whichever it left waiting.
    async fn frames_until(
        &mut self,
        link: &mut Link,
        want: impl Fn(&Value) -> bool,
    ) -> Vec<LinkFrame> {
        let mut seen = Vec::new();
        let found = tokio::time::timeout(MARKER, async {
            loop {
                tokio::select! {
                    event = self.events_rx.recv() => match event {
                        None => return false,
                        Some(event) => self.cell.handle_event(event, &self.origin, &mut self.db).await,
                    },
                    frame = link.from_cell.recv() => match frame {
                        None => return false,
                        Some(frame) => {
                            let hit = match &frame {
                                LinkFrame::Text(text) => meclaw_core::serde_json::from_str::<Value>(text)
                                    .map(|v| want(&v))
                                    .unwrap_or(false),
                                _ => false,
                            };
                            seen.push(frame);
                            if hit {
                                return true;
                            }
                        }
                    },
                }
            }
        })
        .await
        .unwrap_or(false);
        assert!(found, "the frame never arrived; the link carried {seen:?}");
        seen
    }

    /// The next event the handler is owed, handed to it, described for an
    /// assertion.
    ///
    /// The label rather than the event: `VoiceEvent` is not `Clone` — the
    /// handler takes ownership of what it is given — so what comes back is what
    /// a failure message needs.
    async fn pump_one(&mut self) -> String {
        let event = tokio::time::timeout(MARKER, self.events_rx.recv())
            .await
            .expect("the I/O half reports")
            .expect("the events seam stays open");
        let label = format!("{event:?}");
        self.cell
            .handle_event(event, &self.origin, &mut self.db)
            .await;
        label
    }
}

/// One text frame of a link, as JSON.
fn text_of(frame: &LinkFrame) -> Option<Value> {
    match frame {
        LinkFrame::Text(text) => meclaw_core::serde_json::from_str(text).ok(),
        _ => None,
    }
}

/// The close code a link was ended with, if it was.
async fn wait_for_close(from_cell: &mut mpsc::Receiver<LinkFrame>) -> Option<u16> {
    tokio::time::timeout(MARKER, async {
        while let Some(frame) = from_cell.recv().await {
            if let LinkFrame::Close { code, .. } = frame {
                return Some(code);
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

/// The whole of the second door: `hello`, a refusal, one turn on both sides, and
/// the displacement.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_topic_is_a_door_and_it_answers_like_the_socket() {
    let mut live = boot().await;

    // 1. `hello` first, over a link, naming what the join asked for (O-639-3).
    let mut link = live
        .link(LinkRequest {
            session: Some("c1".into()),
            mode: Some("hold".into()),
            ..Default::default()
        })
        .await;
    let Some(LinkFrame::Text(hello)) = link.from_cell.recv().await else {
        panic!("hello is the first frame")
    };
    let hello: Value = meclaw_core::serde_json::from_str(&hello).expect("hello is JSON");
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["session_id"], "c1");
    assert_eq!(hello["mode"], "hold");
    assert_eq!(
        hello["audio_in"]["sample_rate"], 16000,
        "the declaration is the one the socket door makes: {hello}"
    );

    // 2. A refusal is the 400 text, verbatim — one admission, two doors.
    let refused = live
        .surfaces
        .open_link(
            "voice",
            LinkRequest {
                session: Some("bad id!".into()),
                ..Default::default()
            },
        )
        .await
        .expect("mounted")
        .err()
        .expect("a session identity this cell will not address is refused");
    assert_eq!(refused.status, 400);
    assert!(
        refused.detail.starts_with("session must be"),
        "a page reads the sentence a curl reads: {:?}",
        refused.detail
    );

    // 3. hold, 20 ms frames, release → exactly one turn, on the link and on the
    //    lane. The recogniser only speaks once the audio really arrived, so the
    //    transcript is a receipt for the frames rather than a coincidence.
    link.to_cell
        .send(LinkFrame::Text(r#"{"type":"hold"}"#.into()))
        .await
        .expect("the link takes a frame");
    for _ in 0..10 {
        link.to_cell
            .send(LinkFrame::Binary(vec![0u8; FRAME_BYTES]))
            .await
            .expect("the link takes audio");
    }
    link.to_cell
        .send(LinkFrame::Text(r#"{"type":"release"}"#.into()))
        .await
        .expect("the link takes a frame");

    let seen = live
        .frames_until(&mut link, |v| v["type"] == "turn")
        .await
        .into_iter()
        .filter_map(|f| text_of(&f))
        .collect::<Vec<_>>();
    let turns: Vec<&Value> = seen.iter().filter(|v| v["type"] == "turn").collect();
    assert_eq!(turns.len(), 1, "the take produced a turn: {seen:?}");
    assert_eq!(turns[0]["text"], SAID);
    assert_eq!(
        turns[0]["turn_id"], "c1#1",
        "the turn is counted per session, whatever door it came through"
    );

    // ...and EXACTLY one, which needs something to come after it -- on the RIGHT
    // seam.
    //
    // Waiting for the turn and then counting turns can only ever find the one it
    // waited for: a second one arriving a millisecond later would be read after
    // the assertion, and a duplicate could not make this test red. So a sentinel
    // has to follow the turn, and it has to follow it down the same pipe.
    //
    // **Which seam orders what.** A turn is dispatched by the handler
    // (`cell.rs`'s `handle_event` -> `dispatch` -> `to_io`), and the I/O half reads
    // that as `from_handler`. The substrate's own `reconfig_rx` is a SECOND
    // channel, and `next_round` selects `biased;` with `reconfig_rx` FIRST -- so a
    // sentinel sent there overtakes any turn still queued on `from_handler`, and
    // an assertion behind it would be blind to exactly the duplicate it is looking
    // for. It has to travel `to_io`.
    //
    // The cheapest thing that does: a `mode` control frame from the CLIENT. It
    // arrives as `VoiceEvent::Control`, is handled by the same `handle_event` loop
    // that produced the turn, and its `Action::ToClient(Mode)` is dispatched into
    // the same `to_io` queue -- so it is behind every turn dispatched before it, by
    // construction rather than by timing. A sleep would claim the same thing
    // without proving it.
    link.to_cell
        .send(LinkFrame::Text(
            r#"{"type":"mode","mode":"hold"}"#.to_string(),
        ))
        .await
        .expect("the link takes a frame");
    let after = live
        .frames_until(&mut link, |v| v["type"] == "mode")
        .await
        .into_iter()
        .filter_map(|f| text_of(&f))
        .collect::<Vec<_>>();
    assert!(
        after.iter().all(|v| v["type"] != "turn"),
        "one release is one turn, and nothing behind it: {after:?}"
    );

    // The same turn on the cell's own lane, which is what a colony reads. The
    // sentinel bounds this count too, and for the same reason: the lane emission
    // and the turn frame both leave `handle_event`, so by the time that loop has
    // answered the `mode` frame every lane message it owed is already on the
    // channel.
    let mut lane = Vec::new();
    while let Ok(em) = live.out_rx.try_recv() {
        lane.push(em.content);
    }
    let on_lane: Vec<&Value> = lane
        .iter()
        .filter(|c| c["header"]["route"] == "turn")
        .collect();
    assert_eq!(
        on_lane.len(),
        1,
        "one turn on the link is one turn on the lane: {lane:?}"
    );
    assert_eq!(on_lane[0]["header"]["call_id"], "c1");
    assert_eq!(on_lane[0]["messages"][0]["text"], SAID);

    // 4. A second link for the same session displaces the first with 4409.
    let mut second = live
        .link(LinkRequest {
            session: Some("c1".into()),
            ..Default::default()
        })
        .await;
    assert_eq!(
        wait_for_close(&mut link.from_cell).await,
        Some(4409),
        "a reconnect claims the address it had, and the first is told the code"
    );
    let Some(LinkFrame::Text(hello2)) = second.from_cell.recv().await else {
        panic!("the successor gets its own hello")
    };
    assert!(hello2.contains("\"session_id\":\"c1\""), "{hello2}");
}

/// A page that stops reading is counted out, and says so before it goes.
///
/// The same verdict by the same count as on a socket of its own (GH #601): the
/// link's own queue fills, the connection stops writing, and the dispatch queue
/// behind it is what runs over. No clock decides it — the failure marker is far
/// above anything a healthy run needs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_page_that_stops_reading_is_counted_out() {
    let mut live = boot().await;
    // Never read: the link is opened and its receiving half is held untouched.
    let link = live
        .link(LinkRequest {
            session: Some("slow-1".into()),
            ..Default::default()
        })
        .await;
    let _never_read = link.from_cell;

    // The handler has to know the session before it can be spoken to.
    let connected = live.pump_one().await;
    assert!(
        connected.starts_with("Connected") && connected.contains("slow-1"),
        "the link is reported like any other connection; got {connected}"
    );

    for _ in 0..FLOOD {
        live.reconfig_tx
            .send(VoiceReconfig::ToClient {
                session_id: "slow-1".to_string(),
                frame: ServerFrame::Mode { mode: Mode::Auto },
            })
            .await
            .expect("the I/O half takes a frame order");
    }

    let mut reports = Vec::new();
    let too_slow = tokio::time::timeout(MARKER, async {
        while let Some(event) = live.events_rx.recv().await {
            let hit = matches!(&event, VoiceEvent::ClientTooSlow { session_id, .. } if session_id == "slow-1");
            reports.push(format!("{event:?}"));
            if hit {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    assert!(
        too_slow,
        "a link nobody reads is given up on by count, and reported: {reports:?}"
    );
}
