//! Wave voice-cell, strand t3b — the OpenAI-compatible TTS adapter against the
//! hermetic fake. Every test drives the real `OpenAiTts` over a real socket;
//! nothing here stubs the provider itself, so what passes is the wire handling.

use meclaw_cells::voice::contract::{Encoding, ProviderTimeouts, TtsError, TtsProvider};
use meclaw_cells::voice::params::OpenAiTtsParams;
use meclaw_cells::voice::providers::openai_tts::{
    DEFAULT_BASE_URL, DEFAULT_MODEL, DEFAULT_VOICE, OpenAiTts,
};
use meclaw_colony::{ColonyMsg, IoLivenessMark};
use meclaw_core::Path;
use meclaw_testing::mock_openai_tts::{MockOpenAiTts, OpenAiTtsScript};
use std::time::Duration;
use tokio::sync::{mpsc, watch};

/// Params for the fake at `base_url`. Built through serde because `Secret` has
/// no other constructor — which is the point of `Secret`.
fn params(base_url: &str) -> OpenAiTtsParams {
    serde_json::from_value(serde_json::json!({
        "api_key": "test-key",
        "model": "test-model",
        "voice": "test-voice",
        "base_url": base_url,
    }))
    .expect("params parse")
}

/// A provider with tight deadlines, so a stalled fake fails fast.
fn provider(base_url: &str, external_ms: u64, idle_ms: u64) -> OpenAiTts {
    OpenAiTts::new(params(base_url)).with_timeouts(ProviderTimeouts {
        external: Duration::from_millis(external_ms),
        idle: Duration::from_millis(idle_ms),
    })
}

/// Failure-marker timeout: generous, so cargo-parallel load cannot trip it,
/// but finite, so a hung adapter fails as a test instead of as a stuck runner.
const MARKER: Duration = Duration::from_secs(30);

/// Run one synthesis under the failure marker. Every network test goes
/// through here, so a hung adapter fails as a named test rather than as a
/// stuck runner.
async fn synth(
    tts: &OpenAiTts,
    text: &str,
    audio: mpsc::Sender<Vec<u8>>,
    cancel: watch::Receiver<bool>,
    liveness: IoLivenessMark,
) -> Result<(), TtsError> {
    tokio::time::timeout(
        MARKER,
        tts.synthesize(
            tts.output_format(),
            text.to_string(),
            audio,
            cancel,
            liveness,
        ),
    )
    .await
    .expect("synthesize did not finish within the failure marker")
}

/// Drain an audio receiver into one flat buffer of the chunks it saw.
async fn collect(mut rx: mpsc::Receiver<Vec<u8>>) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    while let Ok(Some(chunk)) = tokio::time::timeout(MARKER, rx.recv()).await {
        out.push(chunk);
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn output_format_is_pcm16_mono_24k() {
    let tts = provider("http://127.0.0.1:1", 100, 100);
    let f = tts.output_format();
    assert_eq!(f.encoding, Encoding::PcmS16Le);
    assert_eq!(f.sample_rate, 24_000);
    assert_eq!(f.channels, 1);
    assert_eq!(tts.name(), "openai");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn default_model_is_the_current_tts_model() {
    // R-V14: this file owns the defaults, checked against the vendor model
    // list on 2026-09-05. `params.rs` returns them; that wiring is t1's
    // assertion, so this strand asserts only its own two halves — the values
    // themselves, and that a configured model/voice reaches the wire verbatim
    // rather than being second-guessed by the adapter.
    assert_eq!(DEFAULT_MODEL, "gpt-4o-mini-tts");
    assert_eq!(DEFAULT_VOICE, "alloy");
    assert_eq!(DEFAULT_BASE_URL, "https://api.openai.com");

    // And the params layer hands them out: a config naming only the credential
    // must arrive at exactly these three.
    let bare: OpenAiTtsParams =
        serde_json::from_value(serde_json::json!({"api_key": "k"})).expect("bare params parse");
    assert_eq!(bare.model, DEFAULT_MODEL);
    assert_eq!(bare.voice, DEFAULT_VOICE);
    assert_eq!(bare.base_url, DEFAULT_BASE_URL);

    let mock = MockOpenAiTts::start(OpenAiTtsScript::new())
        .await
        .expect("mock");
    let base = mock.base_url();
    let tts = OpenAiTts::new(
        serde_json::from_value(serde_json::json!({
            "api_key": "test-key",
            "model": DEFAULT_MODEL,
            "voice": DEFAULT_VOICE,
            "base_url": base,
        }))
        .expect("params parse"),
    )
    .with_timeouts(ProviderTimeouts::default());
    let (tx, rx) = mpsc::channel(8);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let drain = tokio::spawn(collect(rx));
    synth(&tts, "x", tx, cancel_rx, IoLivenessMark::disabled())
        .await
        .expect("synthesize");
    drain.await.expect("drain");

    let reqs = mock.requests().await;
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].model(), Some(DEFAULT_MODEL));
    assert_eq!(reqs[0].voice(), Some(DEFAULT_VOICE));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn request_carries_model_voice_pcm() {
    let mock = MockOpenAiTts::start(OpenAiTtsScript::new().chunk(&[1u8, 2]))
        .await
        .expect("mock");
    let tts = provider(&mock.base_url(), 5_000, 5_000);
    let (tx, rx) = mpsc::channel(8);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let drain = tokio::spawn(collect(rx));
    synth(
        &tts,
        "hallo welt",
        tx,
        cancel_rx,
        IoLivenessMark::disabled(),
    )
    .await
    .expect("synthesize");
    drain.await.expect("drain");

    let reqs = mock.requests().await;
    assert_eq!(reqs.len(), 1, "exactly one request per synthesize call");
    assert_eq!(reqs[0].method, "POST");
    assert_eq!(reqs[0].model(), Some("test-model"));
    assert_eq!(reqs[0].voice(), Some("test-voice"));
    assert_eq!(reqs[0].input(), Some("hallo welt"));
    assert_eq!(reqs[0].response_format(), Some("pcm"));
    assert_eq!(
        reqs[0].authorization.as_deref(),
        Some("Bearer test-key"),
        "the key travels as a bearer token, not in the URL"
    );
    // `stream_format` stays unsent: an OpenAI-compatible server would then
    // answer SSE-wrapped base64 on OpenAI and raw bytes elsewhere (R-V10).
    assert!(
        reqs[0].body.get("stream_format").is_none(),
        "stream_format must not be sent: {:?}",
        reqs[0].body
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn base_url_is_used_verbatim() {
    // R-V10: a self-hosted, OpenAI-compatible endpoint behind a path prefix
    // must keep that prefix. No host-keyed special case anywhere.
    let mock = MockOpenAiTts::start(OpenAiTtsScript::new())
        .await
        .expect("mock");
    let prefixed = format!("{}/kokoro/", mock.base_url());
    let tts = provider(&prefixed, 5_000, 5_000);
    let (tx, rx) = mpsc::channel(8);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let drain = tokio::spawn(collect(rx));
    synth(&tts, "x", tx, cancel_rx, IoLivenessMark::disabled())
        .await
        .expect("synthesize");
    drain.await.expect("drain");

    let reqs = mock.requests().await;
    assert_eq!(reqs.len(), 1);
    assert_eq!(
        reqs[0].path, "/kokoro/v1/audio/speech",
        "base_url prefix must survive, one trailing slash absorbed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn body_chunks_forwarded() {
    let scripted: Vec<Vec<u8>> = vec![vec![1, 2, 3, 4], vec![5, 6], vec![7, 8, 9, 10]];
    let mock = MockOpenAiTts::start(
        OpenAiTtsScript::new()
            .with_chunks(scripted.clone())
            .with_chunk_delay(Duration::from_millis(20)),
    )
    .await
    .expect("mock");
    let tts = provider(&mock.base_url(), 5_000, 5_000);
    let (tx, rx) = mpsc::channel(16);
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let drain = tokio::spawn(collect(rx));
    // Constant traffic on the cancel watch that is NOT a cancellation. Each
    // write wakes the `select!`; none of them may cost a byte.
    let noise = tokio::spawn(async move {
        loop {
            if cancel_tx.send(false).is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });
    synth(&tts, "x", tx, cancel_rx, IoLivenessMark::disabled())
        .await
        .expect("synthesize");
    noise.abort();
    let received = drain.await.expect("drain");

    // Chunk boundaries are the server's business, the byte stream is not:
    // assert on the concatenation, and that it arrived in more than one piece.
    let flat: Vec<u8> = received.concat();
    assert_eq!(flat, scripted.concat(), "audio bytes must arrive unaltered");
    assert!(
        received.len() > 1,
        "delayed chunks must arrive as a stream, got {} piece(s)",
        received.len()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_aborts_stream() {
    let mock = MockOpenAiTts::start(
        OpenAiTtsScript::new()
            .with_chunks(vec![vec![1; 8], vec![2; 8], vec![3; 8], vec![4; 8]])
            .with_chunk_delay(Duration::from_millis(150)),
    )
    .await
    .expect("mock");
    let tts = provider(&mock.base_url(), 5_000, 5_000);
    let (tx, mut rx) = mpsc::channel(16);
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let running = synth(&tts, "x", tx, cancel_rx, IoLivenessMark::disabled());

    let flip = tokio::spawn(async move {
        // Cancel once the first chunk is through, well before the last one.
        let _ = tokio::time::timeout(MARKER, rx.recv()).await;
        let _ = cancel_tx.send(true);
        // Keep the receiver alive so the ONLY cancellation signal under test
        // is the watch flag, not a dropped receiver.
        rx
    });
    let err = running
        .await
        .expect_err("cancelled synthesis must not be Ok");
    let _rx = flip.await.expect("flip");
    assert!(
        matches!(err, TtsError::Cancelled),
        "expected Cancelled, got {err:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropped_receiver_cancels() {
    let mock = MockOpenAiTts::start(
        OpenAiTtsScript::new()
            .with_chunks(vec![vec![1; 8], vec![2; 8], vec![3; 8]])
            .with_chunk_delay(Duration::from_millis(100)),
    )
    .await
    .expect("mock");
    let tts = provider(&mock.base_url(), 5_000, 5_000);
    // Capacity 1 and nobody reading: the second chunk blocks on send, and the
    // dropped receiver is what has to end the synthesis.
    let (tx, rx) = mpsc::channel(1);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let running = synth(&tts, "x", tx, cancel_rx, IoLivenessMark::disabled());
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        drop(rx);
    });
    let err = running.await.expect_err("dropped receiver must not be Ok");
    assert!(
        matches!(err, TtsError::Cancelled),
        "expected Cancelled, got {err:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_before_the_first_byte_never_calls_the_endpoint() {
    let mock = MockOpenAiTts::start(OpenAiTtsScript::new().chunk(&[1u8]))
        .await
        .expect("mock");
    let tts = provider(&mock.base_url(), 5_000, 5_000);
    let (tx, _rx) = mpsc::channel(8);
    let (_cancel_tx, cancel_rx) = watch::channel(true);
    let err = synth(&tts, "x", tx, cancel_rx, IoLivenessMark::disabled())
        .await
        .expect_err("already-cancelled synthesis must not be Ok");
    assert!(matches!(err, TtsError::Cancelled), "got {err:?}");
    assert!(
        mock.requests().await.is_empty(),
        "no request may leave for a synthesis that was cancelled before it started"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_error_is_protocol_error() {
    let mock = MockOpenAiTts::start(
        OpenAiTtsScript::new().with_status(500, r#"{"error":{"message":"boom"}}"#),
    )
    .await
    .expect("mock");
    let tts = provider(&mock.base_url(), 5_000, 5_000);
    let (tx, _rx) = mpsc::channel(8);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let err = synth(&tts, "x", tx, cancel_rx, IoLivenessMark::disabled())
        .await
        .expect_err("a 500 must not be Ok");
    match err {
        TtsError::Protocol(detail) => {
            assert!(detail.contains("500"), "status missing: {detail}");
            // Status only: a self-hosted endpoint may echo the request it
            // rejected, and that echo is where a bearer token would ride out.
            assert!(
                !detail.contains("boom"),
                "server body must not reach the error: {detail}"
            );
            assert!(!detail.contains("test-key"), "credential leaked: {detail}");
        }
        other => panic!("expected Protocol, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auth_401() {
    let mock = MockOpenAiTts::start(
        OpenAiTtsScript::new().with_status(401, r#"{"error":{"message":"bad key"}}"#),
    )
    .await
    .expect("mock");
    let tts = provider(&mock.base_url(), 5_000, 5_000);
    let (tx, _rx) = mpsc::channel(8);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let err = synth(&tts, "x", tx, cancel_rx, IoLivenessMark::disabled())
        .await
        .expect_err("a 401 must not be Ok");
    match err {
        TtsError::Auth(detail) => {
            assert!(detail.contains("401"), "status missing: {detail}");
            assert!(
                !detail.contains("bad key"),
                "server body must not reach the error: {detail}"
            );
            assert!(!detail.contains("test-key"), "credential leaked: {detail}");
        }
        other => panic!("expected Auth, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connect_timeout() {
    // The head never comes in time: that is the A-timeout (rule 12), and the
    // provider must report it as Timeout rather than hang on the socket.
    let mock = MockOpenAiTts::start(
        OpenAiTtsScript::new()
            .chunk(&[1u8])
            .with_head_delay(Duration::from_secs(30)),
    )
    .await
    .expect("mock");
    let tts = provider(&mock.base_url(), 150, 5_000);
    let (tx, _rx) = mpsc::channel(8);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let started = std::time::Instant::now();
    let err = synth(&tts, "x", tx, cancel_rx, IoLivenessMark::disabled())
        .await
        .expect_err("a stalled head must not be Ok");
    assert!(matches!(err, TtsError::Timeout), "got {err:?}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the deadline must fire, not the fake's 30s delay: {:?}",
        started.elapsed()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_timeout_between_chunks() {
    // The head arrives, the first chunk arrives, then the stream stalls. The
    // idle deadline is the only thing that can tell that from slow synthesis.
    let mock = MockOpenAiTts::start(
        OpenAiTtsScript::new()
            .with_chunks(vec![vec![1; 4], vec![2; 4]])
            .with_chunk_delay(Duration::from_secs(30)),
    )
    .await
    .expect("mock");
    let tts = provider(&mock.base_url(), 5_000, 200);
    let (tx, rx) = mpsc::channel(8);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let drain = tokio::spawn(collect(rx));
    let started = std::time::Instant::now();
    let err = synth(&tts, "x", tx, cancel_rx, IoLivenessMark::disabled())
        .await
        .expect_err("a stalled stream must not be Ok");
    drain.await.expect("drain");
    assert!(matches!(err, TtsError::Timeout), "got {err:?}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "idle deadline must fire: {:?}",
        started.elapsed()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn truncated_stream_is_a_protocol_error() {
    // The server announced chunked framing and then vanished without the
    // terminating chunk. That is a broken stream, not a finished one — an
    // adapter that returned Ok here would hand the caller half a sentence and
    // call it done.
    let mock = MockOpenAiTts::start(
        OpenAiTtsScript::new()
            .with_chunks(vec![vec![1; 4], vec![2; 4]])
            .with_truncate_after(1),
    )
    .await
    .expect("mock");
    let tts = provider(&mock.base_url(), 5_000, 5_000);
    let (tx, rx) = mpsc::channel(8);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let drain = tokio::spawn(collect(rx));
    let err = synth(&tts, "x", tx, cancel_rx, IoLivenessMark::disabled())
        .await
        .expect_err("a truncated stream must not be Ok");
    let received = drain.await.expect("drain");
    assert!(
        matches!(err, TtsError::Protocol(_)),
        "expected Protocol, got {err:?}"
    );
    assert_eq!(
        received.concat(),
        vec![1u8; 4],
        "what did arrive is still delivered"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn liveness_is_marked_per_chunk() {
    // A real mark, not the disabled one: `mark_success` must fire for the
    // response head and once per chunk, because the whole point of the signal
    // is that a stalled provider stops refreshing it.
    let mock = MockOpenAiTts::start(
        OpenAiTtsScript::new()
            .with_chunks(vec![vec![1; 4], vec![2; 4], vec![3; 4]])
            // Spaced out, so the chunks cannot coalesce into one read and the
            // count below stays an exact statement rather than a lower bound.
            .with_chunk_delay(Duration::from_millis(20)),
    )
    .await
    .expect("mock");
    let tts = provider(&mock.base_url(), 5_000, 5_000);
    // Capacity 64 against 4 expected marks: `mark_success` reports with
    // `try_send`, so a full channel would silently cost a mark.
    let (colony_tx, mut colony_rx) = mpsc::channel(64);
    let mark = IoLivenessMark::new(Path::new("/m/channels/voice"), Some(colony_tx));
    let (tx, rx) = mpsc::channel(16);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let drain = tokio::spawn(collect(rx));
    synth(&tts, "x", tx, cancel_rx, mark)
        .await
        .expect("synthesize");
    let received = drain.await.expect("drain");

    let mut marks = 0usize;
    while let Ok(Some(msg)) =
        tokio::time::timeout(Duration::from_millis(200), colony_rx.recv()).await
    {
        if matches!(msg, ColonyMsg::IoLiveness { at: Some(_), .. }) {
            marks += 1;
        }
    }
    assert_eq!(
        marks,
        1 + received.len(),
        "one mark for the response head plus one per delivered chunk; got {marks} for {} chunk(s)",
        received.len()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_deadline_is_not_refreshed_by_watch_writes() {
    // The regression this guards: an idle deadline built INSIDE the `select!`
    // as `timeout(idle, read)` is dropped and rebuilt whenever another arm
    // wins. A client writing `false` to the cancel watch would then keep a dead
    // stream alive forever. Here the stream stalls after the first chunk while
    // the watch is written repeatedly — the deadline must still fire.
    let mock = MockOpenAiTts::start(
        OpenAiTtsScript::new()
            .with_chunks(vec![vec![1; 4], vec![2; 4]])
            .with_chunk_delay(Duration::from_secs(30)),
    )
    .await
    .expect("mock");
    let tts = provider(&mock.base_url(), 5_000, 300);
    let (tx, rx) = mpsc::channel(16);
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let drain = tokio::spawn(collect(rx));
    let noise = tokio::spawn(async move {
        for _ in 0..40 {
            // Not a cancellation — just traffic on the watch.
            let _ = cancel_tx.send(false);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });
    let started = std::time::Instant::now();
    let err = synth(&tts, "x", tx, cancel_rx, IoLivenessMark::disabled())
        .await
        .expect_err("a stalled stream must not be Ok");
    noise.abort();
    drain.await.expect("drain");
    assert!(matches!(err, TtsError::Timeout), "got {err:?}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the deadline must fire despite watch traffic: {:?}",
        started.elapsed()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn not_found_names_the_route_not_the_host() {
    // The likeliest misconfiguration of an OpenAI-compatible endpoint is a
    // `base_url` pointing at a server without this route. The error says which
    // route was missing; it does not quote the host, which is configuration.
    let mock = MockOpenAiTts::start(OpenAiTtsScript::new().with_status(404, "nope"))
        .await
        .expect("mock");
    let tts = provider(&mock.base_url(), 5_000, 5_000);
    let (tx, _rx) = mpsc::channel(8);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let err = synth(&tts, "x", tx, cancel_rx, IoLivenessMark::disabled())
        .await
        .expect_err("a 404 must not be Ok");
    match err {
        TtsError::Protocol(detail) => {
            assert!(detail.contains("404"), "status missing: {detail}");
            assert!(
                detail.contains("/v1/audio/speech"),
                "route missing: {detail}"
            );
            assert!(!detail.contains("127.0.0.1"), "host leaked: {detail}");
            assert!(!detail.contains("nope"), "server body leaked: {detail}");
        }
        other => panic!("expected Protocol, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropped_watch_sender_is_not_a_cancel() {
    // Orchestrator ruling: a dropped watch SENDER means no cancellation can
    // ever arrive, not that one just did. The synthesis runs to completion.
    // The second half of the claim is that it does so without spinning:
    // `changed()` on a closed channel returns `Err` immediately, so the arm
    // has to retire rather than be re-armed forever.
    let scripted: Vec<Vec<u8>> = vec![vec![1; 4], vec![2; 4]];
    let mock = MockOpenAiTts::start(
        OpenAiTtsScript::new()
            .with_chunks(scripted.clone())
            .with_chunk_delay(Duration::from_millis(20)),
    )
    .await
    .expect("mock");
    let tts = provider(&mock.base_url(), 5_000, 5_000);
    let (tx, rx) = mpsc::channel(16);
    let (cancel_tx, cancel_rx) = watch::channel(false);
    drop(cancel_tx);
    let drain = tokio::spawn(collect(rx));
    synth(&tts, "x", tx, cancel_rx, IoLivenessMark::disabled())
        .await
        .expect("a dropped sender must not cancel the synthesis");
    let received = drain.await.expect("drain");
    assert_eq!(received.concat(), scripted.concat());
}
