//! GH #1055 — the live session of a call opens while the line rings, and the
//! model is ducked while the caller talks over it.
//!
//! Measured on a telephone line (five calls, quiet window): pickup to first
//! syllable 1.84 s p50, because the session opened only once the media fork
//! connected after the pickup; and the companion fell silent 0.12-1.52 s after
//! the caller cut in, because what the caller went on hearing was the model
//! itself, paced in real time, until it yielded on its own.
//!
//! The locks: a `call_ringing` opens the session before any connection, and the
//! connection naming that call adopts it with what the model already said; a
//! `call_ended` or the deadline closes an early session nobody connected to; a
//! cell without the knob opens nothing early; neither lane answers anything;
//! and the model's audio turns to silence once the caller has talked over it
//! for the onset, and comes back after the release.

#[path = "support/duplex_cell.rs"]
mod duplex_cell;

use meclaw_cells::voice::duck::DuckParams;
use meclaw_core::serde_json::json;
use meclaw_core::{Body, Message, MessageBuilder, Path};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::time::Instant;

/// A line fact as the switch posts it: `hop.route` and `hop.call_uuid`, an
/// empty body.
fn line_fact(route: &str, call: &str) -> Message {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("route".into(), json!(route));
    hop.insert("call_uuid".into(), json!(call));
    MessageBuilder::new(Path::new("/voice"))
        .hop(hop)
        .body(Body::Inline(json!({"messages": []})))
        .build()
}

fn off() -> DuckParams {
    DuckParams::default()
}

/// 20 ms at 16 kHz of one constant sample.
fn frame(v: i16) -> Vec<u8> {
    (0..320).flat_map(|_| v.to_le_bytes()).collect()
}

/// The next binary frame the client gets, skipping text frames.
async fn next_audio(client: &mut meclaw_testing::voice_client::VoiceClient) -> Vec<u8> {
    loop {
        let frame = client
            .next_frame(duplex_cell::MARKER)
            .await
            .expect("a frame arrives within the failure marker");
        if let Some(audio) = frame.as_audio() {
            return audio.to_vec();
        }
    }
}

/// No emission and no dead letter within `window`.
async fn says_nothing(live: &mut duplex_cell::Live, window: Duration) {
    tokio::select! {
        e = live.emissions.recv() => panic!("a line fact answers nothing, got {:?}", e.map(|e| e.content)),
        d = live.dead_letters.recv() => panic!("a line fact is never a dead letter (got one: {})", d.is_some()),
        () = tokio::time::sleep(window) => {}
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1055_a_ringing_call_is_adopted_with_what_the_model_already_said() {
    let raw = duplex_cell::duplex_params(json!({"default_mode": "auto", "audio_out_frame_ms": 20}));
    let mut live = duplex_cell::boot_edge(raw, 5_000, off()).await;

    live.send(line_fact("call_ringing", "call-1")).await;
    // The session opens on the ring, before anybody connected.
    let session = live.session().await;
    assert_eq!(live.starts.load(Ordering::SeqCst), 1);
    // The model greets into the ring: two chunks wait for the caller.
    session
        .audio_out
        .send(vec![7u8; 640])
        .await
        .expect("the early session takes audio");
    session
        .audio_out
        .send(vec![8u8; 640])
        .await
        .expect("the early session takes audio");
    says_nothing(&mut live, Duration::from_millis(200)).await;

    let (mut client, hello) = live
        .connect("session=call-1&sample_rate=16000&mode=auto")
        .await;
    assert_eq!(hello["duplex"], true, "got {hello}");
    assert_eq!(
        next_audio(&mut client).await,
        vec![7u8; 640],
        "the greeting is there at once"
    );
    assert_eq!(next_audio(&mut client).await, vec![8u8; 640]);
    // And the same session goes on: no second one was opened for the call.
    session
        .audio_out
        .send(vec![9u8; 640])
        .await
        .expect("still the call's session");
    assert_eq!(next_audio(&mut client).await, vec![9u8; 640]);
    assert_eq!(live.starts.load(Ordering::SeqCst), 1, "one call, one model");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1055_a_call_that_ends_while_ringing_closes_its_early_session() {
    let raw = duplex_cell::duplex_params(json!({"default_mode": "auto"}));
    let mut live = duplex_cell::boot_edge(raw, 60_000, off()).await;

    live.send(line_fact("call_ringing", "call-2")).await;
    let _session = live.session().await;
    live.send(line_fact("call_ended", "call-2")).await;
    let ended = tokio::time::timeout(duplex_cell::MARKER, live.ended.recv())
        .await
        .expect("the early session is closed when the call ends, not at the deadline");
    assert_eq!(ended, Some(1));
    says_nothing(&mut live, Duration::from_millis(200)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1055_an_early_session_nobody_connects_to_ends_at_its_deadline() {
    let raw = duplex_cell::duplex_params(json!({"default_mode": "auto"}));
    let mut live = duplex_cell::boot_edge(raw, 200, off()).await;

    live.send(line_fact("call_ringing", "call-3")).await;
    let _session = live.session().await;
    let ended = tokio::time::timeout(duplex_cell::MARKER, live.ended.recv())
        .await
        .expect("the deadline closes the early session");
    assert_eq!(ended, Some(1));
    // A connection that comes after the deadline opens a fresh one.
    let (_client, _hello) = live
        .connect("session=call-3&sample_rate=16000&mode=auto")
        .await;
    let _fresh = live.session().await;
    assert_eq!(live.starts.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1055_without_the_knob_a_ring_opens_nothing_and_answers_nothing() {
    let raw = duplex_cell::duplex_params(json!({"default_mode": "auto"}));
    let mut live = duplex_cell::boot_edge(raw, 0, off()).await;

    live.send(line_fact("call_ringing", "call-4")).await;
    live.send(line_fact("call_ended", "call-4")).await;
    says_nothing(&mut live, Duration::from_millis(300)).await;
    assert_eq!(
        live.starts.load(Ordering::SeqCst),
        0,
        "shipped behaviour: the hello opens it"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1055_the_model_is_ducked_while_the_caller_talks_over_it() {
    let raw = duplex_cell::duplex_params(json!({"default_mode": "auto", "audio_out_frame_ms": 20}));
    let duck = DuckParams {
        onset_ms: 60,
        release_ms: 100,
        level_dbfs: -35.0,
        echo_margin_db: meclaw_cells::voice::duck::DEFAULT_BARGE_ECHO_MARGIN_DB,
    };
    let mut live = duplex_cell::boot_edge(raw, 0, duck).await;
    let (mut client, _hello) = live
        .connect("session=call-5&sample_rate=16000&mode=auto")
        .await;
    let session = live.session().await;

    // The model speaks; the caller hears it.
    session.audio_out.send(frame(8_000)).await.expect("audio");
    assert_eq!(next_audio(&mut client).await, frame(8_000));

    // The caller talks over it for 60 ms ...
    for _ in 0..3 {
        client
            .send_audio(&frame(8_000))
            .await
            .expect("the socket takes audio");
    }
    // ... which the model is fed (the duck never touches the way in) ...
    for _ in 0..3 {
        let fed = tokio::time::timeout(duplex_cell::MARKER, live.audio.recv())
            .await
            .expect("the caller reaches the model")
            .expect("open");
        assert_eq!(fed, frame(8_000));
    }
    // ... and the model's next chunk reaches the caller as silence.
    session.audio_out.send(frame(8_000)).await.expect("audio");
    assert_eq!(
        next_audio(&mut client).await,
        vec![0u8; 640],
        "ducked: silence, same length"
    );

    // 100 ms of quiet from the caller: the model is heard again.
    for _ in 0..5 {
        client
            .send_audio(&frame(10))
            .await
            .expect("the socket takes audio");
    }
    for _ in 0..5 {
        let _ = tokio::time::timeout(duplex_cell::MARKER, live.audio.recv()).await;
    }
    session.audio_out.send(frame(8_000)).await.expect("audio");
    assert_eq!(next_audio(&mut client).await, frame(8_000), "released");
}

/// 100 ms at 16 kHz of one constant sample: the chunk a live model streams.
fn chunk(v: i16) -> Vec<u8> {
    (0..1_600).flat_map(|_| v.to_le_bytes()).collect()
}

/// The greeting's sample, and the one of a later word.
const GREET: i16 = 8_000;
const LATER: i16 = 12_000;

/// GH #1055 R1: a live model streams a continuous channel, silence included,
/// in real time — 100 ms chunks (S0 a1/a2). Here it says nothing for a second,
/// greets for a second, and goes on streaming silence while the line rings
/// for five; then the caller picks up.
///
/// What the caller must get: the greeting first and at once (not the second
/// of silence before it), exactly once, and after it a line in real time
/// again — not the whole ring queued in front of every later word. And the
/// model's loop must never wait on a channel nobody reads while it rings: a
/// blocked provider loop stops its keepalive and its socket reads.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1055_a_model_streaming_silence_through_the_ring_greets_once_and_in_real_time() {
    let raw = duplex_cell::duplex_params(json!({"default_mode": "auto", "audio_out_frame_ms": 20}));
    let mut live = duplex_cell::boot_edge(raw, 15_000, off()).await;

    live.send(line_fact("call_ringing", "call-r1")).await;
    let session = live.session().await;

    // The model: 10 chunks of silence, 10 of greeting, silence on, one later
    // word 4.5 s after the pickup (chunk 95).
    let sent = Arc::new(AtomicU64::new(0));
    let out = session.audio_out.clone();
    let counter = Arc::clone(&sent);
    let model = tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        for n in 0..110u64 {
            tick.tick().await;
            let c = match n {
                10..20 => chunk(GREET),
                95 => chunk(LATER),
                _ => chunk(0),
            };
            if out.send(c).await.is_err() {
                return;
            }
            counter.store(n + 1, Ordering::SeqCst);
        }
    });

    // The line rings for five seconds.
    tokio::time::sleep(Duration::from_millis(5_000)).await;
    let by_pickup = sent.load(Ordering::SeqCst);
    assert!(
        by_pickup >= 45,
        "the model streamed on through the ring without waiting, sent {by_pickup} of 50 chunks"
    );
    // While it rang the model heard a line: silence, at the wire's 20 ms.
    let mut fed = 0usize;
    while let Ok(f) = live.audio.try_recv() {
        assert!(
            f.iter().all(|b| *b == 0),
            "only silence is fed while it rings"
        );
        fed += 1;
    }
    assert!(
        fed >= 100,
        "the ring feeds the model silence, got {fed} frames"
    );

    let picked_up = Instant::now();
    let (mut client, hello) = live
        .connect("session=call-r1&sample_rate=16000&mode=auto")
        .await;
    assert_eq!(hello["duplex"], true, "got {hello}");

    // The first frame the caller hears is the greeting's first.
    assert_eq!(
        next_audio(&mut client).await,
        frame(GREET),
        "the greeting first, not the silence before it"
    );
    let mut greeting_frames = 1usize;
    let mut greeting_over = false;
    let mut heard_ms = 20u64;
    loop {
        let f = next_audio(&mut client).await;
        if f == frame(LATER) {
            break;
        }
        if f == frame(GREET) {
            assert!(!greeting_over, "a second greeting reached the caller");
            greeting_frames += 1;
        } else {
            assert!(f.iter().all(|b| *b == 0), "only the greeting and silence");
            greeting_over = true;
        }
        heard_ms += (f.len() / 2) as u64 * 1_000 / 16_000;
    }
    let elapsed = picked_up.elapsed().as_millis() as u64;
    assert_eq!(greeting_frames, 50, "exactly one greeting, whole");
    // Real time again: what reached the caller before the later word is not
    // more than the time since the pickup (plus a frame or two of slack).
    assert!(
        heard_ms <= elapsed + 300,
        "a backlog survives the pickup: {heard_ms} ms of audio in {elapsed} ms"
    );
    assert_eq!(live.starts.load(Ordering::SeqCst), 1, "one call, one model");
    model.abort();
}

/// GH #1055 R6: a media connection at another format than the early session
/// opens a fresh one; the caller hears that one's greeting and only that, and
/// the early session ends.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1055_a_connection_at_another_format_opens_a_fresh_session_and_hears_one_greeting() {
    let raw = duplex_cell::duplex_params(json!({"default_mode": "auto", "audio_out_frame_ms": 20}));
    let mut live = duplex_cell::boot_edge(raw, 15_000, off()).await;

    live.send(line_fact("call_ringing", "call-r6")).await;
    let early = live.session().await;
    for _ in 0..3 {
        early
            .audio_out
            .send(chunk(GREET))
            .await
            .expect("the early session greets into the ring");
    }
    says_nothing(&mut live, Duration::from_millis(200)).await;

    let (mut client, _hello) = live
        .connect("session=call-r6&sample_rate=8000&mode=auto")
        .await;
    let fresh = live.session().await;
    assert_eq!(live.starts.load(Ordering::SeqCst), 2, "a fresh session");
    let ended = tokio::time::timeout(duplex_cell::MARKER, live.ended.recv())
        .await
        .expect("the early session ends");
    assert_eq!(ended, Some(1), "the early one, not the fresh one");

    // 100 ms at 8 kHz of the fresh session's greeting.
    let greet8: Vec<u8> = (0..800).flat_map(|_| LATER.to_le_bytes()).collect();
    fresh
        .audio_out
        .send(greet8)
        .await
        .expect("the fresh session greets");
    let frame8: Vec<u8> = (0..160).flat_map(|_| LATER.to_le_bytes()).collect();
    for _ in 0..5 {
        assert_eq!(
            next_audio(&mut client).await,
            frame8,
            "the fresh session's greeting, never the early one's"
        );
    }
    let quiet_until = Instant::now() + Duration::from_millis(300);
    while let Ok(Ok(f)) =
        tokio::time::timeout_at(quiet_until, client.next_frame(duplex_cell::MARKER)).await
    {
        if let Some(a) = f.as_audio() {
            panic!("a second greeting reached the caller: {} bytes", a.len());
        }
    }
}

/// GH #1055 R5: a second ring of the same call (`180` then `183`), and a ring
/// of a call that is already connected, open no provider session: a session
/// opened only to be thrown away is a paid handshake for nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1055_a_second_ring_of_the_same_call_opens_no_second_session() {
    let raw = duplex_cell::duplex_params(json!({"default_mode": "auto"}));
    let mut live = duplex_cell::boot_edge(raw, 15_000, off()).await;

    live.send(line_fact("call_ringing", "call-r5")).await;
    let _early = live.session().await;
    live.send(line_fact("call_ringing", "call-r5")).await;
    says_nothing(&mut live, Duration::from_millis(300)).await;
    assert_eq!(
        live.starts.load(Ordering::SeqCst),
        1,
        "the parked call rings again"
    );

    let (_client, _hello) = live
        .connect("session=call-r5&sample_rate=16000&mode=auto")
        .await;
    live.send(line_fact("call_ringing", "call-r5")).await;
    says_nothing(&mut live, Duration::from_millis(300)).await;
    assert_eq!(
        live.starts.load(Ordering::SeqCst),
        1,
        "the connected call rings again"
    );
}
