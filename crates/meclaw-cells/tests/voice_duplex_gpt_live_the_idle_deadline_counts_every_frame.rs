//! Welle Live, zell-uhr (#798, R-L7) — the idle deadline counts FRAMES, not
//! meters.
//!
//! `provider_idle_timeout_ms` was justified in the tree by a sentence that has
//! since been measured false: "the model streams audio and its meter arrives
//! every 15 s". On a session that hears only silence the meter arrives at no
//! point inside 45 s (GH #798, four runs). Had the deadline really been resting
//! on the meter, every quiet line would have been cut at 30 s.
//!
//! It never was — the reset sits on the arrival of any SESSION frame, before
//! the event is even mapped — and this file is the lock that says so, because a
//! promise nothing tests is a comment. A model that sends nothing but output
//! audio, and not one `session.usage.updated`, holds the deadline off for as
//! long as it keeps sending.
//!
//! What ends the session when it stops is the subject of the file next door,
//! `..._a_quiet_socket_is_held_by_its_own_keepalive`: the adapter asks the
//! socket itself, and a line where nobody speaks is only cut once the answers
//! stop. This lock is about the frames alone, so it switches that question OFF
//! — `keepalive_ms` is set past the whole run — and measures what the session
//! frames do on their own.
//!
//! The one incoming frame that deliberately does NOT count is a transport
//! keepalive somebody else sent (`Ping`): a proxy pinging a dead upstream is
//! not a live session, and that exclusion is older than this strand and stays
//! (OR-L.zell-uhr.1).

use meclaw_cells::voice::contract::{
    DuplexControl, DuplexError, DuplexEvent, DuplexProvider, DuplexSession, ProviderTimeouts,
};
use meclaw_cells::voice::params::GptLiveParams;
use meclaw_cells::voice::providers::gpt_live::GptLiveDuplex;
use meclaw_colony::IoLivenessMark;
use meclaw_testing::mock_gpt_live::{LiveAction, LiveScript, MockGptLive};
use serde_json::json;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Failure-marker timeout: generous on purpose, it never discriminates timing.
const FAILURE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the socket may stay quiet.
const IDLE: Duration = Duration::from_millis(400);

/// How often the model sends one chunk of audio — comfortably inside [`IDLE`].
const CHUNK_EVERY: Duration = Duration::from_millis(100);

/// How many chunks it sends. Twenty at 100 ms is five idle windows: long enough
/// that a deadline resting on anything other than these frames would have
/// fired several times over.
const CHUNKS: usize = 20;

fn params(base_url: &str) -> GptLiveParams {
    serde_json::from_value(json!({
        "api_key": "test-key",
        "base_url": base_url,
        "instructions": "Be Egon.",
        // Longer than this whole test: the adapter's own keepalive holds a
        // quiet socket open on purpose, and what is measured here is what the
        // SESSION frames do without it.
        "keepalive_ms": 60_000,
    }))
    .expect("params parse")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn voice_duplex_gpt_live_the_idle_deadline_counts_every_frame() {
    // Output audio and NOTHING else: no meter, no transcript, no ack. The
    // shape of the quiet line #798 measured, with the model's own voice on it.
    let mut actions = Vec::new();
    for _ in 0..CHUNKS {
        actions.push(LiveAction::Delay(CHUNK_EVERY));
        actions.push(LiveAction::SendAudio(vec![1u8, 2, 3, 4]));
    }
    // Then it stops talking and says nothing more. What ends the session from
    // here on is the deadline.
    let mock = MockGptLive::start(LiveScript {
        session_id: "sess_frames".to_string(),
        actions,
        ..LiveScript::default()
    })
    .await
    .expect("bind fake");

    // Held, never written: the caller is on the line and silent, so nothing
    // of OURS can be what keeps the session alive. Dropping it would close
    // the call instead.
    let (_audio_in_tx, audio_in_rx) = mpsc::channel::<Vec<u8>>(32);
    let (audio_out_tx, mut audio_out_rx) = mpsc::channel(32);
    let (events_tx, mut events_rx) = mpsc::channel::<DuplexEvent>(64);
    let (_control_tx, control_rx) = mpsc::channel::<DuplexControl>(16);
    let provider = GptLiveDuplex::new(params(&mock.base_url())).with_timeouts(ProviderTimeouts {
        external: Duration::from_millis(5_000),
        idle: IDLE,
    });
    let future = provider.run_session(
        provider.format(),
        DuplexSession {
            audio_in: audio_in_rx,
            audio_out: audio_out_tx,
            events: events_tx,
            control: control_rx,
        },
        IoLivenessMark::disabled(),
    );
    let began = Instant::now();
    let join = tokio::spawn(future);

    // The model's audio has to be taken, or a full channel stalls the adapter
    // and the test would measure the test's own backpressure.
    let drain = tokio::spawn(async move {
        let mut chunks = 0usize;
        while audio_out_rx.recv().await.is_some() {
            chunks += 1;
        }
        chunks
    });

    // The caller is holding the line and saying nothing at all: `audio_in`
    // stays open and empty, so no frame of OURS can be what keeps the session
    // alive.
    assert!(
        matches!(
            tokio::time::timeout(FAILURE_TIMEOUT, events_rx.recv())
                .await
                .expect("timed out waiting for `Started`"),
            Some(DuplexEvent::Started { .. })
        ),
        "the session opened"
    );

    let mut meters = 0usize;
    let mut saw_close = false;
    while let Ok(Some(event)) = tokio::time::timeout(FAILURE_TIMEOUT, events_rx.recv()).await {
        match event {
            DuplexEvent::Usage { .. } => meters += 1,
            DuplexEvent::Closed { .. } => saw_close = true,
            _ => {}
        }
    }
    let verdict = tokio::time::timeout(FAILURE_TIMEOUT, join)
        .await
        .expect("the idle deadline ended the session, not the test's marker")
        .expect("the session task did not panic");
    let took = began.elapsed();

    assert_eq!(
        meters, 0,
        "the model sent no meter at all — that is the premise of #798"
    );
    assert!(
        !saw_close,
        "a session cut by the idle deadline reports no orderly close"
    );
    let Err(DuplexError::Closed(detail)) = &verdict else {
        panic!("a socket that finally went quiet is `Closed`, not {verdict:?}");
    };
    assert!(
        detail.contains("idle"),
        "the reason names the guard that fired: {detail}"
    );

    // The load-bearing number. The audio ran for CHUNKS * CHUNK_EVERY = 2 s,
    // five times the idle window, and the session outlived all of it: what held
    // the deadline off was the frames, one at a time.
    let audio_ran = CHUNK_EVERY * u32::try_from(CHUNKS).expect("CHUNKS fits a u32");
    assert!(
        took > audio_ran,
        "the session lived through every chunk before the deadline took it: \
         {took:?} against {audio_ran:?}"
    );
    let chunks = tokio::time::timeout(FAILURE_TIMEOUT, drain)
        .await
        .expect("the drain stopped when the adapter let go")
        .expect("the drain did not panic");
    assert_eq!(
        chunks, CHUNKS,
        "and every one of those frames really arrived"
    );
}
