//! Welle Live, zell-uhr (GH #798) — what a REAL `gpt-live-1` socket sends on a
//! line where nobody speaks, counted by WebSocket frame type. Ignored by
//! default.
//!
//! This is the measurement `provider_idle_timeout_ms` was missing. The tree
//! assumed a live session is never quiet for long, and the cell's own runs
//! showed no session frame at all inside 45 s of silence — but they were
//! counted at the ADAPTER, which drops `Ping`, `Pong` and `Binary` into a
//! `continue` before anything sees them. So the one question that decides
//! whether a keepalive helps was still open: does the far side speak the
//! transport when it has nothing else to say, and does it answer a ping of
//! ours?
//!
//! The test therefore does not use the adapter. It opens the socket itself,
//! starts a session, feeds nothing but silence, sends its own ping on the
//! shipped `keepalive_ms` and counts every incoming frame by type with the
//! millisecond it arrived.
//!
//! It also SHUTS the model's ear once the session is open, and that did not
//! produce a quiet line either. Both runs of 2026-09-22 look the same: with the
//! ear open, 896 session frames in 95 s, output audio from 1 179 ms on, no gap
//! longer than 604 ms; with it shut, 891 frames, output audio from 780 ms on,
//! no gap longer than 478 ms. This endpoint streamed output audio for the whole
//! watch whatever it was told, so neither run reproduced the line GH #798
//! measured — where a session fed silence saw no session frame inside 45 s —
//! and neither run says anything about it. The mute stays because it is the
//! honest attempt, and what it measured is recorded here rather than tidied
//! away.
//!
//! What the runs DO settle is the ground the keepalive stands on, and that was
//! the open question: nine pings, nine pongs, in both runs, and not one ping
//! the far side sent unasked. Those two runs were flown on a ten-second period,
//! which is what the default was that day; [`KEEPALIVE`] has followed the
//! default down to eight seconds since, so a fresh run counts eleven and not
//! nine. What was measured is the answering, not the count.
//!
//! Printed at the end:
//!
//! * how many frames of each type arrived,
//! * the longest gap between two SESSION frames — the number a deadline
//!   counting session frames alone would have fired on,
//! * the longest gap between two frames of ANY type,
//! * whether any ping arrived unasked, and how many of our own came back.
//!
//! Run it explicitly, with the credential in the ENVIRONMENT and nowhere else:
//!
//! ```text
//! OPENAI_API_KEY=$(grep '^OPENAI_API_KEY=' ~/.env | cut -d= -f2-) \
//!   cargo test -p meclaw-cells --test gpt_live_live_frames -- --ignored --nocapture
//! ```
//!
//! Nothing here prints, logs or asserts on the value; a missing key is a panic
//! naming the variable, which is a name and not a secret. Budget: one session
//! of about a hundred seconds.
//!
//! A second ignored test, `a_greeting_session_recorded` (wave w1, strand
//! liveaufnahme, GH #1610), opens a session with the frames of
//! `fixtures/live/greeting_opening_gpt_live_1_1.json`, lets the model greet and notes every
//! server event. With `MECLAW_LIVE_RECORD=<path>` set it writes them as a
//! recording (`meclaw_testing::mock_gpt_live::LiveRecorder`: a header line,
//! then one line per event with its gap, audio as a duration only) that
//! `LiveScript::from_recording` replays in the mock; without it nothing is
//! written. Budget: one session of about fifteen seconds.

use futures_util::{SinkExt, StreamExt};
use meclaw_cells::voice::providers::gpt_live::{KEEPALIVE_PAYLOAD, session_url};
use meclaw_testing::mock_gpt_live::{LiveRecorder, RecordingHeader};
use serde_json::{Value as JsonValue, json};
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{Bytes, Utf8Bytes};

/// How long the quiet session is watched. The deadline it is measured against
/// is 30 000 ms, so ninety seconds is three of those windows.
const WATCH: Duration = Duration::from_secs(95);

/// How often the test pings, the shipped `keepalive_ms`
/// (`meclaw_cells::voice::params::DEFAULT_KEEPALIVE_MS`). Not imported from
/// there on purpose: this file is a measurement of the far side, and it should
/// change when somebody decides it changes, not when a default moves.
const KEEPALIVE: Duration = Duration::from_secs(8);

/// One 20 ms frame of silence at 24 kHz PCM16 mono.
const SILENCE_FRAME: usize = 960;

/// The deadline this measurement is about.
const IDLE: Duration = Duration::from_secs(30);

/// What arrived, and when.
#[derive(Default)]
struct Tally {
    /// One `(kind, ms since the session opened)` per incoming frame.
    seen: Vec<(&'static str, u128)>,
}

impl Tally {
    fn note(&mut self, kind: &'static str, at_ms: u128) {
        self.seen.push((kind, at_ms));
    }

    fn count(&self, kind: &str) -> usize {
        self.seen.iter().filter(|(k, _)| *k == kind).count()
    }

    /// The longest gap between two frames of the kinds named, counting from the
    /// session's start and to the end of the watch.
    fn worst_gap_ms(&self, kinds: &[&str], watched_ms: u128) -> u128 {
        let mut last = 0u128;
        let mut worst = 0u128;
        for (kind, at) in &self.seen {
            if kinds.contains(kind) {
                worst = worst.max(at.saturating_sub(last));
                last = *at;
            }
        }
        worst.max(watched_ms.saturating_sub(last))
    }
}

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

/// LIVE — a silent session, every incoming frame counted by type.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live: needs a real OPENAI_API_KEY and spends session seconds"]
async fn a_silent_session_counted_by_frame_type() {
    let url = session_url("wss://api.openai.com");
    let mut request = url
        .as_str()
        .into_client_request()
        .expect("the session url is a websocket url");
    // The credential exists only inside this header value.
    let header = format!("Bearer {}", api_key())
        .parse()
        .expect("the authorization header is built");
    request.headers_mut().insert("authorization", header);

    let (ws, _response) = tokio::time::timeout(
        Duration::from_secs(20),
        tokio_tungstenite::connect_async(request),
    )
    .await
    .expect("the handshake answered inside 20 s")
    .expect("the handshake succeeded");
    let (mut write, mut read) = ws.split();

    let start = json!({
        "type": "session.start",
        "event_id": "start",
        "session": {
            "model": "gpt-live-1",
            "instructions": "You are a test device. Speak only when you are \
                             spoken to.",
            "audio": {
                "format": { "type": "audio/pcm", "rate": 24000 },
                "output": { "voice": "marin" },
            },
            "delegation": { "type": "client" },
        }
    });
    write
        .send(WsMessage::Text(Utf8Bytes::from(start.to_string())))
        .await
        .expect("session.start goes out");

    // Shut the ear. A live model answers what it hears, and silence is
    // something it hears; without this the line is not quiet at all (see the
    // module note). `session.input_audio.mute` is the same frame the adapter
    // sends for `hold`.
    write
        .send(WsMessage::Text(Utf8Bytes::from(
            json!({ "type": "session.input_audio.mute", "event_id": "mute" }).to_string(),
        )))
        .await
        .expect("the mute goes out");

    let opened = Instant::now();
    let mut tally = Tally::default();
    let mut own_pings = 0usize;
    let mut own_pongs = 0usize;
    let mut foreign_pings = 0usize;
    let mut session_kinds: Vec<String> = Vec::new();

    // A quiet caller on a real line still sends audio: 20 ms of silence, over
    // and over. That is what the far side hears, and it is the case #798 is
    // about -- somebody is on the phone and nobody is talking.
    let (silence_tx, mut silence_rx) = tokio::sync::mpsc::channel::<()>(1);
    let feeder = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(20)).await;
            if silence_tx.send(()).await.is_err() {
                return;
            }
        }
    });

    let mut keepalive =
        tokio::time::interval_at(tokio::time::Instant::now() + KEEPALIVE, KEEPALIVE);
    while opened.elapsed() < WATCH {
        tokio::select! {
            _ = silence_rx.recv() => {
                // Real PCM silence, base64 of 960 zero bytes, built once per
                // send so nothing here is a shortcut the endpoint could refuse.
                let frame = json!({
                    "type": "session.input_audio.append",
                    "audio": b64(&vec![0u8; SILENCE_FRAME]),
                });
                if write
                    .send(WsMessage::Text(Utf8Bytes::from(frame.to_string())))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            _ = keepalive.tick() => {
                own_pings += 1;
                if write
                    .send(WsMessage::Ping(Bytes::from_static(KEEPALIVE_PAYLOAD)))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            incoming = read.next() => {
                let at = opened.elapsed().as_millis();
                match incoming {
                    None => {
                        println!("LIVE +{at} ms the socket ended");
                        break;
                    }
                    Some(Err(e)) => {
                        println!("LIVE +{at} ms the socket tore: {e}");
                        break;
                    }
                    Some(Ok(WsMessage::Text(text))) => {
                        tally.note("text", at);
                        if let Ok(v) = serde_json::from_str::<JsonValue>(&text)
                            && let Some(kind) = v.get("type").and_then(JsonValue::as_str)
                            && !session_kinds.iter().any(|k| k == kind)
                        {
                            session_kinds.push(kind.to_string());
                            println!("LIVE +{at} ms first `{kind}`");
                        }
                    }
                    Some(Ok(WsMessage::Ping(_))) => {
                        foreign_pings += 1;
                        tally.note("ping", at);
                        println!("LIVE +{at} ms an unasked ping arrived");
                    }
                    Some(Ok(WsMessage::Pong(payload))) => {
                        tally.note("pong", at);
                        if payload.as_ref() == KEEPALIVE_PAYLOAD {
                            own_pongs += 1;
                        }
                    }
                    Some(Ok(WsMessage::Binary(_))) => tally.note("binary", at),
                    Some(Ok(WsMessage::Frame(_))) => tally.note("frame", at),
                    Some(Ok(WsMessage::Close(_))) => {
                        tally.note("close", at);
                        println!("LIVE +{at} ms the far side closed");
                        break;
                    }
                }
            }
        }
    }
    feeder.abort();
    let watched = opened.elapsed().as_millis();

    println!("LIVE watched {watched} ms of a session nobody spoke on");
    println!(
        "LIVE frames: text={} ping={} pong={} binary={} close={}",
        tally.count("text"),
        tally.count("ping"),
        tally.count("pong"),
        tally.count("binary"),
        tally.count("close"),
    );
    println!("LIVE session frame types seen: {session_kinds:?}");
    println!(
        "LIVE worst gap between two SESSION frames: {} ms (the deadline is {} ms)",
        tally.worst_gap_ms(&["text"], watched),
        IDLE.as_millis(),
    );
    println!(
        "LIVE worst gap between two frames of ANY type: {} ms",
        tally.worst_gap_ms(&["text", "ping", "pong", "binary", "frame"], watched),
    );
    println!("LIVE pings sent by us: {own_pings}, pongs that came back: {own_pongs}");
    println!("LIVE pings the far side sent unasked: {foreign_pings}");

    // The one thing this measurement has to establish, and the only assertion:
    // the keepalive of R-L7 rests on the far side answering a ping. If it does
    // not, the fix is built on sand and this line says so.
    assert!(
        own_pongs > 0,
        "the endpoint answered none of our {own_pings} pings -- the keepalive cannot \
         carry the idle deadline"
    );
    let _ = write.send(WsMessage::Close(None)).await;
}

/// Standard-alphabet base64 with padding. The tree's own encoder lives in
/// `meclaw-testing`, which is a dev-dependency here; this is four lines and
/// keeps the test from reaching across a crate for them.
fn b64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(A[(n >> 18) as usize & 63] as char);
        out.push(A[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            A[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            A[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// The frames the recorded session opens with -- shared with the replay test,
/// which is what makes a recording's header comparable with today's. Read at
/// run time, not included: every test of this binary is `#[ignore]`d, so no
/// diff plans it, and a compile-time include would tie the fixture to a binary
/// the gate never builds for it. The replay test includes the same file.
const OPENING_PATH: &str = "tests/fixtures/live/greeting_opening_gpt_live_1_1.json";

/// The variable that names the file a recording goes to. Unset, nothing is
/// written.
const RECORD_ENV: &str = "MECLAW_LIVE_RECORD";

/// How long the greeting is listened to before the session is closed: the
/// measured greeting of OR-L51 took under a second to start and a sentence to
/// end. A watch, not a deadline anything fails on.
const GREETING_WATCH: Duration = Duration::from_secs(10);

/// How long the far side gets to answer `session.close`.
const CLOSE_GRACE: Duration = Duration::from_secs(5);

/// LIVE -- one greeting session, every server event noted; written as a
/// recording when `MECLAW_LIVE_RECORD` names a file.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live: needs a real OPENAI_API_KEY and spends session seconds"]
async fn a_greeting_session_recorded() {
    let opening_text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(OPENING_PATH),
    )
    .expect("the opening fixture is readable");
    let opening: Vec<JsonValue> = serde_json::from_str(&opening_text).expect("the opening parses");
    let header = RecordingHeader::for_opening(&opening, &session_url(""));
    let mut request = session_url("wss://api.openai.com")
        .as_str()
        .into_client_request()
        .expect("the session url is a websocket url");
    let headers = request.headers_mut();
    // The credential exists only inside this header value.
    headers.insert(
        "authorization",
        format!("Bearer {}", api_key())
            .parse()
            .expect("the authorization header is built"),
    );
    // Every model call names its app (GH #1251).
    headers.insert(
        "x-title",
        "meclaw-lab live-recording".parse().expect("x-title"),
    );
    headers.insert(
        "http-referer",
        "https://meclaw.ai".parse().expect("http-referer"),
    );

    let (ws, _response) = tokio::time::timeout(
        Duration::from_secs(20),
        tokio_tungstenite::connect_async(request),
    )
    .await
    .expect("the handshake answered inside 20 s")
    .expect("the handshake succeeded");
    let (mut write, mut read) = ws.split();
    let (start, after_start) = opening.split_first().expect("the opening starts a session");
    write
        .send(WsMessage::Text(Utf8Bytes::from(start.to_string())))
        .await
        .expect("session.start goes out");
    let mut recorder = LiveRecorder::opened_now(header);

    let mut deadline = tokio::time::Instant::now() + GREETING_WATCH;
    let mut closing = false;
    let mut error = None;
    // A caller on a line sends audio even when quiet, and the model needs it:
    // the first recording run (23.11.2026) fed nothing, heard 94 chunks of
    // output audio with no transcript, and its greeting append ended in
    // `context_injection_incomplete` at the close. With 20 ms of silence on
    // the line the second run greeted, transcript and all.
    let mut feed = tokio::time::interval(Duration::from_millis(20));
    loop {
        let next = tokio::select! {
            _ = feed.tick(), if !closing => {
                let frame = json!({
                    "type": "session.input_audio.append",
                    "audio": b64(&[0u8; SILENCE_FRAME]),
                });
                if write
                    .send(WsMessage::Text(Utf8Bytes::from(frame.to_string())))
                    .await
                    .is_err()
                {
                    break;
                }
                continue;
            }
            next = tokio::time::timeout_at(deadline, read.next()) => next,
        };
        let incoming = match next {
            Ok(incoming) => incoming,
            Err(_) if !closing => {
                closing = true;
                deadline = tokio::time::Instant::now() + CLOSE_GRACE;
                let close = json!({ "type": "session.close", "event_id": "close" });
                if write
                    .send(WsMessage::Text(Utf8Bytes::from(close.to_string())))
                    .await
                    .is_err()
                {
                    break;
                }
                continue;
            }
            Err(_) => break,
        };
        let text = match incoming {
            Some(Ok(WsMessage::Text(text))) => text,
            Some(Ok(WsMessage::Close(_))) | Some(Err(_)) | None => break,
            Some(Ok(_)) => continue,
        };
        let Ok(event) = serde_json::from_str::<JsonValue>(&text) else {
            continue;
        };
        recorder.note(&event, tokio::time::Instant::now());
        match event.get("type").and_then(JsonValue::as_str) {
            Some("session.started") => {
                for frame in after_start {
                    write
                        .send(WsMessage::Text(Utf8Bytes::from(frame.to_string())))
                        .await
                        .expect("the opening goes out");
                }
            }
            Some("session.closed") => break,
            // Abort at the first error: a paid run is never retried here.
            Some("error") => {
                error = Some(event.clone());
                break;
            }
            _ => {}
        }
    }

    let kinds: Vec<&str> = recorder.events().iter().map(|e| e.kind.as_str()).collect();
    println!("LIVE recorded {} events: {kinds:?}", kinds.len());
    assert!(
        error.is_none(),
        "the session answered with an error: {error:?}"
    );
    assert!(
        kinds.contains(&"session.output_audio.delta"),
        "the model greeted: {kinds:?}"
    );
    if let Ok(path) = std::env::var(RECORD_ENV) {
        std::fs::write(&path, recorder.to_jsonl()).expect("the recording is written");
        println!("LIVE recording written to {path}");
    }
}
