//! Wave Live, L9 § 3: LIVE checks against the real `gpt-live-1` endpoint.
//! Ignored by default.
//!
//! These never run in the normal suite -- they need a real credential, real
//! network and real session seconds (billed). Run one explicitly:
//!
//! ```text
//! OPENAI_API_KEY=$(grep '^OPENAI_API_KEY=' <colony>/.env | cut -d= -f2-) \
//!   cargo test -p meclaw-cells --test gpt_live_live -- --ignored --nocapture
//! ```
//!
//! The credential is read from the ENVIRONMENT and never from a file here: the
//! `grep | cut` above is the one sanctioned way to lift a single key out of a
//! `.env`, and it belongs to the caller rather than to the test. Nothing in this
//! file prints, logs or asserts on the value; a missing key is a panic naming
//! the variable, which is a name and not a secret.
//!
//! They drive the PROVIDER directly (`GptLiveDuplex::run_session` with the four
//! channels), not the cell and not a socket of the colony -- what is under test
//! is the adapter against the real wire, and every layer above it has a fake of
//! its own. Budget: three sessions, well under two minutes of session time.

use meclaw_cells::voice::contract::{
    AppendKind, DuplexControl, DuplexError, DuplexEvent, DuplexProvider, DuplexSession,
    ProviderTimeouts, Speaker,
};
use meclaw_cells::voice::params::GptLiveParams;
use meclaw_cells::voice::providers::gpt_live::GptLiveDuplex;
use meclaw_colony::IoLivenessMark;
use serde_json::json;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Failure-marker timeout: generous on purpose, it never discriminates timing.
const FAILURE_TIMEOUT: Duration = Duration::from_secs(40);

/// How long a session is watched before it is let go.
///
/// It is a WATCH, not a deadline anything fails on, and nothing here waits for
/// the running meter. `session.usage.updated` is documented at about every
/// 15 s, and on a session fed nothing but silence it arrived at NO point inside
/// 45 s and twice inside 60 s (four runs, L9, 2026-09-21) -- while the cell's
/// 297 s session carrying real speech had its first inside 12 s. Such a session
/// also left one `session.close` unanswered for 40 s. GH #798 carries the
/// measurement.
///
/// That was an interim retreat (OR-L.L9.1) until #798 was understood. It is
/// settled now and the retreat is permanent: R-L7 took the meter out of the
/// turn machine altogether, so the session's clock is
/// `params.duplex.tick_ms` in the cell and the meter is a reading of the
/// context window. A live test that waited on it would be waiting on a number
/// nothing depends on any more -- the ticks are still PRINTED below, because
/// the arrival pattern is the finding, and the finding is worth one line of
/// output per run.
const WATCH: Duration = Duration::from_secs(12);

/// One 20 ms frame of silence at 24 kHz PCM16 mono -- 480 samples, two bytes.
const SILENCE_FRAME: usize = 960;

/// The credential, from the environment. Never printed, never asserted on.
fn api_key() -> String {
    match std::env::var("OPENAI_API_KEY") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => panic!(
            "OPENAI_API_KEY is not set in the environment -- see the head of this file \
             for the one sanctioned way to lift it out of a .env"
        ),
    }
}

/// The params of a live session: the real endpoint by default, and an
/// instruction that keeps the model quiet unless it is spoken to.
fn params() -> GptLiveParams {
    serde_json::from_value(json!({
        "api_key": api_key(),
        "instructions": "You are a test device. Speak only when you are spoken to. \
                         Answer in one sentence.",
        "sample_rate": 24000,
    }))
    .expect("params parse")
}

/// A running session: the four channel ends, plus the join handle.
struct Session {
    audio_in: mpsc::Sender<Vec<u8>>,
    #[allow(dead_code)]
    audio_out: mpsc::Receiver<Vec<u8>>,
    events: mpsc::Receiver<DuplexEvent>,
    control: mpsc::Sender<DuplexControl>,
    join: tokio::task::JoinHandle<Result<(), DuplexError>>,
}

/// Opens one live session against the real endpoint.
///
/// The A-timeout is 20 s rather than the 5 s default: the handshake crosses the
/// Atlantic here and a fake on loopback is what the default was sized for.
fn start() -> Session {
    let (audio_in_tx, audio_in_rx) = mpsc::channel(64);
    let (audio_out_tx, audio_out_rx) = mpsc::channel(64);
    let (events_tx, events_rx) = mpsc::channel(128);
    let (control_tx, control_rx) = mpsc::channel(16);
    let provider = GptLiveDuplex::new(params()).with_timeouts(ProviderTimeouts {
        external: Duration::from_secs(20),
        idle: Duration::from_secs(60),
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
    Session {
        audio_in: audio_in_tx,
        audio_out: audio_out_rx,
        events: events_rx,
        control: control_tx,
        join: tokio::spawn(future),
    }
}

/// The next event, or a failed test -- never a hang.
async fn next_event(events: &mut mpsc::Receiver<DuplexEvent>) -> DuplexEvent {
    tokio::time::timeout(FAILURE_TIMEOUT, events.recv())
        .await
        .expect("timed out waiting for a duplex event")
        .expect("event channel closed early")
}

/// The session id the provider answered with, once `Started` has arrived.
async fn started(events: &mut mpsc::Receiver<DuplexEvent>) -> String {
    match next_event(events).await {
        DuplexEvent::Started { session_id } => {
            println!("LIVE started: session_id={session_id}");
            session_id
        }
        other => panic!("expected Started first, got {other:?}"),
    }
}

/// Feeds 20 ms silence frames until the sender is dropped -- what a client on a
/// quiet line sends, and what keeps the model's ear open.
fn feed_silence(audio_in: mpsc::Sender<Vec<u8>>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let frame = vec![0u8; SILENCE_FRAME];
        while audio_in.send(frame.clone()).await.is_ok() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
}

/// Ends a session in an orderly way and reports what it cost.
async fn close_and_report(session: Session, what: &str) -> Option<f64> {
    let Session {
        audio_in,
        audio_out,
        mut events,
        control,
        join,
    } = session;
    let _ = control.send(DuplexControl::Close).await;
    let mut final_usage = None;
    while let Ok(Some(event)) = tokio::time::timeout(FAILURE_TIMEOUT, events.recv()).await {
        if let DuplexEvent::Closed {
            reason,
            usage_seconds,
        } = event
        {
            println!("LIVE {what} closed: reason={reason} usage_seconds={usage_seconds}");
            final_usage = Some(usage_seconds);
            break;
        }
    }
    drop(audio_in);
    drop(audio_out);
    drop(control);
    let _ = tokio::time::timeout(FAILURE_TIMEOUT, join).await;
    println!("LIVE {what} final usage_seconds={final_usage:?}");
    final_usage
}

/// LIVE 1 -- a real session opens and carries both directions of the wire.
///
/// Receipt: the provider's own session id, the handshake in ms, and the
/// assistant transcript that answers one append. That is the smallest complete
/// statement about a live socket: the credential was accepted, the session
/// exists under an id the provider owns, and something the client said came
/// back as something the model said.
///
/// It does NOT wait for the running meter, and the `WATCH` constant says why --
/// that wait was measured and it does not end, and since R-L7 nothing in the
/// cell depends on it either. The metered close is LIVE 3.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live: needs a real OPENAI_API_KEY and spends session seconds"]
async fn a_session_starts_and_answers_on_the_wire() {
    let mut session = start();
    let opened = Instant::now();
    let session_id = started(&mut session.events).await;
    println!("LIVE handshake: {} ms", opened.elapsed().as_millis());
    let feeder = feed_silence(session.audio_in.clone());
    session
        .control
        .send(DuplexControl::Append {
            kind: AppendKind::Commentary,
            event_id: "l9-live-wire-1".to_string(),
            delegation_id: None,
            content: "Please count slowly from one to ten.".to_string(),
        })
        .await
        .expect("control channel open");

    let watched = Instant::now();
    let mut spoke = None;
    let mut ticks = Vec::new();
    while watched.elapsed() < WATCH && spoke.is_none() {
        let left = WATCH - watched.elapsed();
        match tokio::time::timeout(left, session.events.recv()).await {
            Ok(Some(DuplexEvent::Usage { seconds, .. })) => ticks.push(seconds),
            Ok(Some(DuplexEvent::Transcript { speaker, delta, .. }))
                if speaker == Speaker::Assistant && !delta.trim().is_empty() =>
            {
                println!(
                    "LIVE +{} ms the model answers: {delta:?}",
                    watched.elapsed().as_millis()
                );
                spoke = Some(delta);
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => break,
        }
    }
    feeder.abort();
    println!("LIVE running meter ticks while watched: {ticks:?}");
    let answer = spoke.unwrap_or_else(|| {
        panic!("session {session_id} never answered the append within {WATCH:?}")
    });
    assert!(
        !answer.trim().is_empty(),
        "an answer that is only whitespace is not an answer"
    );
    // Best effort: the close of a session that heard nothing but silence was
    // measured going unanswered for 40 s (GH #798), and this test does not
    // judge the close -- LIVE 3 does, on a session that was closed at once.
    let _ = session.control.send(DuplexControl::Close).await;
}

/// LIVE 2 -- an append on the commentary channel is taken up and placed.
///
/// Receipt: the `event_id` comes back on an `Appended` with the model's own
/// `start_ms`/`end_ms`. That echo is what the delegation cycle is built on: the
/// colony learns WHERE its advice landed on the model's clock, not merely that
/// the frame was written to a socket.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live: needs a real OPENAI_API_KEY and spends session seconds"]
async fn a_commentary_append_is_acknowledged() {
    let mut session = start();
    started(&mut session.events).await;
    let feeder = feed_silence(session.audio_in.clone());

    let event_id = "l9-live-commentary-1".to_string();
    session
        .control
        .send(DuplexControl::Append {
            kind: AppendKind::Commentary,
            event_id: event_id.clone(),
            delegation_id: None,
            content: "Sag bitte kurz Hallo.".to_string(),
        })
        .await
        .expect("control channel open");

    let sent = Instant::now();
    let mut placed = None;
    while sent.elapsed() < FAILURE_TIMEOUT {
        match next_event(&mut session.events).await {
            DuplexEvent::Appended {
                kind,
                event_id: echoed,
                start_ms,
                end_ms,
            } if echoed == event_id => {
                println!(
                    "LIVE appended after {} ms: kind={kind:?} start_ms={start_ms} end_ms={end_ms}",
                    sent.elapsed().as_millis()
                );
                assert_eq!(AppendKind::Commentary, kind);
                placed = Some((start_ms, end_ms));
                break;
            }
            other => println!("LIVE event: {other:?}"),
        }
    }
    feeder.abort();
    let (start_ms, end_ms) = placed.expect("the append was never acknowledged");
    assert!(
        end_ms >= start_ms,
        "an append ends no earlier than it starts: {start_ms}..{end_ms}"
    );
    let _ = close_and_report(session, "live-2").await;
}

/// LIVE 3 -- an orderly close reports the final meter and the session future
/// returns `Ok`.
///
/// Receipt: `Closed { reason, usage_seconds }` with seconds above zero, and a
/// provider that ended without an error. A close that ends the socket but loses
/// the final reading is how a wave stops knowing what it spent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live: needs a real OPENAI_API_KEY and spends session seconds"]
async fn a_closed_session_reports_final_usage() {
    let mut session = start();
    started(&mut session.events).await;
    let feeder = feed_silence(session.audio_in.clone());
    tokio::time::sleep(Duration::from_secs(3)).await;
    feeder.abort();

    session
        .control
        .send(DuplexControl::Close)
        .await
        .expect("control channel open");

    let mut closed = None;
    while closed.is_none() {
        match next_event(&mut session.events).await {
            DuplexEvent::Closed {
                reason,
                usage_seconds,
            } => closed = Some((reason, usage_seconds)),
            other => println!("LIVE event: {other:?}"),
        }
    }
    let (reason, usage_seconds) = closed.expect("the close reported no final usage");
    println!("LIVE closed: reason={reason} usage_seconds={usage_seconds}");
    assert!(
        usage_seconds > 0.0,
        "a session that ran meters more than zero seconds, got {usage_seconds}"
    );

    drop(session.audio_in);
    drop(session.control);
    let ended = tokio::time::timeout(FAILURE_TIMEOUT, session.join)
        .await
        .expect("the session future never returned")
        .expect("the session task panicked");
    assert!(ended.is_ok(), "an orderly close is not an error: {ended:?}");
}
