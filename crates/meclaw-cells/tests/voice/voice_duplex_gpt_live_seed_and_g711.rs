//! D48 (Welle live-2, LC1) — the `gpt_live` adapter opens with a seed and
//! speaks G.711.
//!
//! The seed (`session.input`) and the format names are the OpenAI guides'
//! (live-conversations: one `message` per entry, one text part, `input_text`
//! for developer/user, `output_text` for assistant; WebSocket audio formats:
//! `audio/pcmu` / `audio/pcma` at 8000). Against the hermetic fake: these are
//! socket tests and carry no time verdict -- how long the opening waits is
//! pinned on the paused clock in the unit tests of `providers/gpt_live.rs`.

// harness_fold: #[path = "support/gh1059_grants.rs"]
#[allow(unused_imports)]
use crate::gh1059_grants; // harness_fold: mod gh1059_grants;

use gh1059_grants::LogCapture;
use meclaw_cells::voice::contract::{
    DuplexControl, DuplexError, DuplexEvent, DuplexProvider, DuplexSession, ProviderTimeouts,
    SeedMessage,
};
use meclaw_cells::voice::g711;
use meclaw_cells::voice::params::{GptLiveParams, VoiceParams};
use meclaw_cells::voice::providers::gpt_live::GptLiveDuplex;
use meclaw_colony::IoLivenessMark;
use meclaw_testing::mock_gpt_live::{LiveAction, LiveScript, MockGptLive};
use serde_json::{Value as JsonValue, json};
use std::time::Duration;
use tokio::sync::mpsc;

/// Failure marker, never a timing verdict.
const FAILURE_TIMEOUT: Duration = Duration::from_secs(30);

struct Session {
    audio_in: mpsc::Sender<Vec<u8>>,
    audio_out: mpsc::Receiver<Vec<u8>>,
    events: mpsc::Receiver<DuplexEvent>,
    control: mpsc::Sender<DuplexControl>,
    join: tokio::task::JoinHandle<Result<(), DuplexError>>,
}

fn params(base_url: &str, extra: JsonValue) -> GptLiveParams {
    let mut raw = json!({
        "api_key": "test-key",
        "base_url": base_url,
        "instructions": "Be Egon.",
    });
    if let (Some(obj), Some(more)) = (raw.as_object_mut(), extra.as_object()) {
        for (k, v) in more {
            obj.insert(k.clone(), v.clone());
        }
    }
    serde_json::from_value(raw).expect("params parse")
}

fn start(provider: GptLiveDuplex) -> Session {
    let (audio_in_tx, audio_in_rx) = mpsc::channel(32);
    let (audio_out_tx, audio_out_rx) = mpsc::channel(32);
    let (events_tx, events_rx) = mpsc::channel(64);
    let (control_tx, control_rx) = mpsc::channel(16);
    let provider = provider.with_timeouts(ProviderTimeouts {
        external: Duration::from_millis(5_000),
        idle: Duration::from_millis(30_000),
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

async fn started(events: &mut mpsc::Receiver<DuplexEvent>) {
    loop {
        let e = tokio::time::timeout(FAILURE_TIMEOUT, events.recv())
            .await
            .expect("HANG: no event")
            .expect("events open");
        if matches!(e, DuplexEvent::Started { .. }) {
            return;
        }
    }
}

async fn start_frame(mock: &MockGptLive) -> JsonValue {
    tokio::time::timeout(FAILURE_TIMEOUT, async {
        loop {
            if let Some(f) = mock.session_start().await {
                return f;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("HANG: no session.start reached the fake")
}

fn waiting_script(actions: Vec<LiveAction>) -> LiveScript {
    let mut actions = actions;
    actions.push(LiveAction::Delay(Duration::from_secs(60)));
    LiveScript {
        session_id: "sess_seed".to_string(),
        actions,
        ..LiveScript::default()
    }
}

/// Test 1: the seed that arrives while the opening waits rides in
/// `session.input`, every message in order with its role and its part type.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gpt_live_start_frame_carries_seed_input() {
    let logs = LogCapture::global();
    let mock = MockGptLive::start(waiting_script(vec![]))
        .await
        .expect("bind fake");
    let mut session = start(GptLiveDuplex::new(params(
        &mock.base_url(),
        json!({"seed_wait_ms": 20_000}),
    )));
    session
        .control
        .send(DuplexControl::Seed {
            messages: vec![
                SeedMessage {
                    role: "developer".into(),
                    text: "Today is Tuesday.".into(),
                },
                SeedMessage {
                    role: "user".into(),
                    text: "Hallo".into(),
                },
                SeedMessage {
                    role: "assistant".into(),
                    text: "Hello again!".into(),
                },
            ],
        })
        .await
        .expect("the opening takes its seed");
    started(&mut session.events).await;
    let frame = start_frame(&mock).await;
    assert_eq!(
        frame["session"]["input"],
        json!([
            {"type": "message", "role": "developer",
             "content": [{"type": "input_text", "text": "Today is Tuesday."}]},
            {"type": "message", "role": "user",
             "content": [{"type": "input_text", "text": "Hallo"}]},
            {"type": "message", "role": "assistant",
             "content": [{"type": "output_text", "text": "Hello again!"}]},
        ])
    );
    assert_eq!(
        frame["session"]["instructions"], "Be Egon.",
        "the seed does not replace the instructions"
    );
    let used = logs.containing("seed_used");
    assert!(
        used.iter().any(|l| l.text().contains("messages=3")),
        "`seed_used` names the count: {:?}",
        logs.containing("gpt_live")
    );
    // Holds only because nextest runs every test in its own process:
    // `LogCapture::global()` collects per process, so under `cargo test` it
    // would also see the `seed_late` lines of neighbouring tests.
    assert!(logs.containing("seed_late").is_empty());
    session.join.abort();
}

/// Test 2: no seed within the wait -- the session starts without `input`, and
/// the greeting still follows as commentary. What the caller said meanwhile
/// is held and reaches the model after the start, not lost.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gpt_live_starts_without_seed_after_wait() {
    let logs = LogCapture::global();
    let mock = MockGptLive::start(waiting_script(vec![LiveAction::ExpectAppend {
        kind: "commentary",
        contains: "Greet",
    }]))
    .await
    .expect("bind fake");
    let mut session = start(GptLiveDuplex::new(params(
        &mock.base_url(),
        json!({"seed_wait_ms": 400, "greeting": "Greet the caller."}),
    )));
    session
        .audio_in
        .send(vec![1u8; 640])
        .await
        .expect("audio is taken while the opening waits");
    started(&mut session.events).await;
    let frame = start_frame(&mock).await;
    assert!(
        frame["session"].get("input").is_none(),
        "no seed, no input: {frame}"
    );
    tokio::time::timeout(FAILURE_TIMEOUT, async {
        while mock.appends().await.is_empty() || mock.received_audio().await.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("HANG: greeting and held audio reach the model");
    assert_eq!(mock.appends().await[0]["type"], "session.commentary.append");
    assert_eq!(mock.received_audio().await[0], vec![1u8; 640]);
    // The proof that it WAITED: only the deadline arm of the wait ends in
    // `seed_late`; a session that did not wait would log nothing of the seed.
    assert!(
        !logs.containing("seed_late").is_empty(),
        "the wait ran out: {:?}",
        logs.containing("gpt_live")
    );
    // Holds only because nextest runs every test in its own process:
    // `LogCapture::global()` collects per process, so under `cargo test` it
    // would also see the `seed_used` lines of neighbouring tests.
    assert!(logs.containing("seed_used").is_empty());
    session.join.abort();
}

/// Review K3: a seed over the budget leaves its oldest message, the start
/// frame carries the rest, and the log line counts what left.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gpt_live_seed_over_budget_logs_seed_trimmed() {
    let logs = LogCapture::global();
    let mock = MockGptLive::start(waiting_script(vec![]))
        .await
        .expect("bind fake");
    let mut session = start(GptLiveDuplex::new(params(
        &mock.base_url(),
        json!({"seed_wait_ms": 20_000}),
    )));
    session
        .control
        .send(DuplexControl::Seed {
            messages: vec![
                SeedMessage {
                    role: "user".into(),
                    text: "x".repeat(40_000),
                },
                SeedMessage {
                    role: "assistant".into(),
                    text: "kept".into(),
                },
            ],
        })
        .await
        .expect("the opening takes its seed");
    started(&mut session.events).await;
    let frame = start_frame(&mock).await;
    assert_eq!(
        frame["session"]["input"].as_array().map(Vec::len),
        Some(1),
        "{}",
        frame["session"]["input"]
    );
    assert!(
        logs.containing("seed_trimmed")
            .iter()
            .any(|l| l.text().contains("seed_trimmed=1")),
        "{:?}",
        logs.containing("gpt_live")
    );
    session.join.abort();
}

/// Review K1: a call that ends while its session waits for the seed costs no
/// session -- no `session.start` reaches the provider, no greeting is said.
/// Both ways out: a close, and a caller who leaves.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gpt_live_an_end_during_the_seed_wait_starts_nothing() {
    for by_close in [true, false] {
        let mock = MockGptLive::start(waiting_script(vec![]))
            .await
            .expect("bind fake");
        let session = start(GptLiveDuplex::new(params(
            &mock.base_url(),
            json!({"seed_wait_ms": 20_000, "greeting": "Greet the caller."}),
        )));
        let Session {
            audio_in,
            control,
            join,
            ..
        } = session;
        if by_close {
            control
                .send(DuplexControl::Close)
                .await
                .expect("the waiting session reads its control");
        } else {
            drop(audio_in);
        }
        let ended = tokio::time::timeout(FAILURE_TIMEOUT, join)
            .await
            .expect("HANG: the waiting session does not end")
            .expect("the session task does not panic");
        assert!(ended.is_ok(), "an end is no error: {ended:?}");
        assert!(
            mock.session_start().await.is_none(),
            "no session.start for a call that ended (close: {by_close})"
        );
        assert!(mock.appends().await.is_empty(), "no greeting");
    }
}

/// Test 3: without `seed_wait_ms` the opening frame is byte for byte today's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gpt_live_seed_default_is_byte_identical() {
    let mock = MockGptLive::start(waiting_script(vec![]))
        .await
        .expect("bind fake");
    let mut session = start(GptLiveDuplex::new(params(&mock.base_url(), json!({}))));
    started(&mut session.events).await;
    let frame = start_frame(&mock).await;
    let today = json!({
        "type": "session.start",
        "event_id": "start",
        "session": {
            "model": "gpt-live-1",
            "instructions": "Be Egon.",
            "audio": {
                "format": {"type": "audio/pcm", "rate": 16000},
                "output": {"voice": "marin"},
            },
            "delegation": {"type": "client"},
        }
    });
    assert_eq!(frame.to_string(), today.to_string());
    session.join.abort();
}

/// Test 5: L16 8 kHz from the client is μ-law on the wire (half the bytes,
/// the table's reference points), and μ-law from the model reaches the client
/// as L16 that compands back to the very same bytes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gpt_live_g711_ulaw_companded_both_ways() {
    // Every μ-law code but 0x7F (the second zero, which comes back as 0xFF).
    let from_model: Vec<u8> = (0u8..=255).filter(|b| *b != 0x7F).collect();
    let mock = MockGptLive::start(waiting_script(vec![
        LiveAction::RequireAudioBytes(3),
        LiveAction::SendAudio(from_model.clone()),
    ]))
    .await
    .expect("bind fake");
    let provider = GptLiveDuplex::new(params(
        &mock.base_url(),
        json!({"audio_format": "g711_ulaw"}),
    ));
    let declared = provider.format();
    assert_eq!(
        (declared.sample_rate, declared.frame_bytes()),
        (8000, 2),
        "the client keeps L16, at 8000"
    );
    let mut session = start(provider);
    started(&mut session.events).await;
    assert_eq!(
        start_frame(&mock).await["session"]["audio"]["format"],
        json!({"type": "audio/pcmu", "rate": 8000})
    );

    let pcm: Vec<u8> = [0i16, i16::MAX, i16::MIN]
        .iter()
        .flat_map(|s| s.to_le_bytes())
        .collect();
    session.audio_in.send(pcm).await.expect("audio taken");
    let back = tokio::time::timeout(FAILURE_TIMEOUT, session.audio_out.recv())
        .await
        .expect("HANG: the model's audio reaches the client")
        .expect("audio_out open");
    assert_eq!(
        mock.received_audio().await[0],
        vec![0xFF, 0x80, 0x00],
        "one byte per sample, the table's silence and full scales"
    );
    assert_eq!(back.len(), from_model.len() * 2, "L16 towards the client");
    assert_eq!(
        g711::compress(g711::Law::Mu, &back),
        from_model,
        "the round trip is byte-identical"
    );
    session.join.abort();
}

/// Test 6: a G.711 block that also names a rate is refused at birth; without
/// one it parses, and A-law opens as `audio/pcma`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gpt_live_g711_rejects_sample_rate() {
    let doc = |duplex: JsonValue| {
        json!({
            "mount": "voice",
            "duplex": duplex,
        })
    };
    let err = VoiceParams::parse(&doc(json!({
        "provider": "gpt_live", "api_key": "k", "instructions": "Be Egon.",
        "audio_format": "g711_alaw", "sample_rate": 16000,
    })))
    .expect_err("a G.711 block naming a rate is refused");
    assert!(err.contains("audio_format_rate_conflict"), "{err}");
    VoiceParams::parse(&doc(json!({
        "provider": "gpt_live", "api_key": "k", "instructions": "Be Egon.",
        "audio_format": "g711_alaw",
    })))
    .expect("G.711 without a rate parses");
    // `null` is the way out of a base that names a rate (`override_params`):
    // it reads as "not named", and G.711 is reachable (review K6).
    let parsed = VoiceParams::parse(&doc(json!({
        "provider": "gpt_live", "api_key": "k", "instructions": "Be Egon.",
        "audio_format": "g711_ulaw", "sample_rate": null,
    })))
    .expect("a null rate reads as not named");
    assert!(parsed.duplex.is_some());
    let err = VoiceParams::parse(&doc(json!({
        "provider": "gpt_live", "api_key": "k", "instructions": "Be Egon.",
        "audio_format": "opus",
    })))
    .expect_err("an unknown format is refused");
    // serde names the value it refused and the three it knows.
    assert!(
        err.contains("`opus`") && err.contains("`g711_alaw`"),
        "{err}"
    );

    let mock = MockGptLive::start(waiting_script(vec![]))
        .await
        .expect("bind fake");
    let mut session = start(GptLiveDuplex::new(params(
        &mock.base_url(),
        json!({"audio_format": "g711_alaw"}),
    )));
    started(&mut session.events).await;
    assert_eq!(
        start_frame(&mock).await["session"]["audio"]["format"],
        json!({"type": "audio/pcma", "rate": 8000})
    );
    session.join.abort();
}

/// Review K7: the admission a connection runs (`negotiate`) takes a
/// client at L16 8000 on a G.711 instance and refuses one at 16000 -- the
/// session runs at 8000 by definition, whatever rate the block would default.
#[test]
fn gpt_live_g711_admits_a_client_at_8000_only() {
    for law in ["g711_ulaw", "g711_alaw"] {
        let provider = GptLiveDuplex::new(params("ws://127.0.0.1:9", json!({"audio_format": law})));
        assert_eq!(
            provider.negotiate(8000).map(|f| f.sample_rate),
            Some(8000),
            "{law}"
        );
        assert!(provider.negotiate(16000).is_none(), "{law}");
    }
}
