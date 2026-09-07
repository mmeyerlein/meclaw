//! Wave voice-cell, strand t3a: the Cartesia `TtsProvider` against the
//! hermetic fake (`meclaw_testing::mock_cartesia`). Nothing here talks to
//! Cartesia — the fake speaks the documented wire, so these tests exercise the
//! real adapter and only `base_url` differs from production.

use meclaw_cells::voice::contract::{ProviderTimeouts, TtsError, TtsProvider};
use meclaw_cells::voice::params::CartesiaParams;
use meclaw_cells::voice::providers::cartesia::CartesiaTts;
use meclaw_colony::ColonyMsg;
use meclaw_colony::io_liveness::IoLivenessMark;
use meclaw_core::Path;
use meclaw_core::serde_json::json;
use meclaw_testing::mock_cartesia::{CartesiaScript, MockCartesia};
use std::time::Duration;
use tokio::sync::{mpsc, watch};

/// Params pointing at the fake. Everything else stays at its default, so a
/// changed default shows up here rather than hiding behind a test override.
fn params(base_url: &str) -> CartesiaParams {
    meclaw_core::serde_json::from_value(json!({
        "api_key": "test-key",
        "voice": "test-voice",
        // A dated snapshot rather than the default, so the assertion proves the
        // value travels rather than agreeing with a constant by accident.
        "model": "sonic-3.6-2026-08-27",
        "base_url": base_url,
    }))
    .expect("cartesia params")
}

/// The two channel pairs a `synthesize` call is wired to: audio out, cancel in.
type Wiring = (
    mpsc::Sender<Vec<u8>>,
    mpsc::Receiver<Vec<u8>>,
    watch::Sender<bool>,
    watch::Receiver<bool>,
);

/// Everything a `synthesize` call needs, minus the provider.
fn wiring() -> Wiring {
    let (audio_tx, audio_rx) = mpsc::channel::<Vec<u8>>(8);
    let (cancel_tx, cancel_rx) = watch::channel(false);
    (audio_tx, audio_rx, cancel_tx, cancel_rx)
}

/// The 30 s failure-marker convention around a synthesis: a test that hangs
/// forever reports nothing, a test that fails names what it waited for.
async fn finish(
    task: tokio::task::JoinHandle<Result<(), TtsError>>,
    what: &str,
) -> Result<(), TtsError> {
    tokio::time::timeout(Duration::from_secs(30), task)
        .await
        .unwrap_or_else(|_| panic!("{what} did not finish within 30 s"))
        .expect("join")
}

/// The 30 s failure-marker convention around a channel read: a test that hangs
/// forever reports nothing, a test that fails names what it waited for.
async fn recv_chunk(rx: &mut mpsc::Receiver<Vec<u8>>, what: &str) -> Vec<u8> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .unwrap_or_else(|_| panic!("no {what} within 30 s"))
        .unwrap_or_else(|| panic!("the audio channel closed before {what}"))
}

/// Wait until the fake has actually read a cancel frame.
///
/// `write.send(...).await` returning `Ok` only means the bytes left this side;
/// the fake's reader task observes them a scheduling moment later. Asserting
/// straight after `synthesize` returned therefore passes on an idle machine and
/// fails under a loaded `nextest` run — measured, not theorised. The deadline is
/// a failure marker (30 s convention), not a timing discriminator.
async fn wait_for_cancel(mock: &MockCartesia) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if mock.cancelled().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the fake never saw a cancel frame within 30 s");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn request_carries_model_voice_format_and_rate() {
    let mock = MockCartesia::start(CartesiaScript::new().chunk(b"\x01\x02"))
        .await
        .expect("mock");
    let tts = CartesiaTts::new(params(&mock.base_url()));
    let (audio_tx, mut audio_rx, _cancel_tx, cancel_rx) = wiring();
    let drain = tokio::spawn(async move { while audio_rx.recv().await.is_some() {} });

    tokio::time::timeout(
        Duration::from_secs(30),
        tts.synthesize(
            "hallo welt".to_string(),
            audio_tx,
            cancel_rx,
            IoLivenessMark::disabled(),
        ),
    )
    .await
    .expect("the synthesis must finish within 30 s")
    .expect("synthesis");
    drain.await.expect("drain");

    let reqs = mock.received_requests().await;
    assert_eq!(reqs.len(), 1, "one request per synthesize");
    let req = &reqs[0];
    assert_eq!(req["transcript"], "hallo welt");
    // R-V5: the voice is the param, never a literal in the adapter.
    assert_eq!(req["voice"]["mode"], "id");
    assert_eq!(req["voice"]["id"], "test-voice");
    // R-V4/R-V14: the model is a param. Whatever it says reaches the wire
    // verbatim — the adapter has no opinion of its own.
    assert_eq!(req["model_id"], "sonic-3.6-2026-08-27");
    assert_eq!(req["output_format"]["container"], "raw");
    assert_eq!(req["output_format"]["encoding"], "pcm_s16le");
    assert_eq!(req["output_format"]["sample_rate"], 24000);
    assert_eq!(req["language"], "de");
    assert_eq!(req["continue"], false);
    assert!(
        req["context_id"].as_str().is_some_and(|c| !c.is_empty()),
        "every request carries a context id, so a cancel can name it"
    );

    // Credentials go in headers, never in the URL: a transport error can then
    // never carry the key.
    let headers = mock.headers().await;
    assert_eq!(
        headers[0].get("x-api-key").map(String::as_str),
        Some("test-key")
    );
    assert!(
        headers[0].contains_key("cartesia-version"),
        "the API version pins how the wire is shaped"
    );
    assert_eq!(
        mock.queries().await,
        vec![String::new()],
        "no credential and no version in the query string"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chunks_arrive_in_order_and_decoded() {
    let mock = MockCartesia::start(
        CartesiaScript::new()
            .chunk(b"\x01\x02")
            .chunk(b"\x03\x04\x05")
            .chunk(&[0xFF, 0xFE]),
    )
    .await
    .expect("mock");
    let tts = CartesiaTts::new(params(&mock.base_url()));
    let (audio_tx, mut audio_rx, _cancel_tx, cancel_rx) = wiring();
    let collect = tokio::spawn(async move {
        let mut all = Vec::new();
        while let Some(chunk) = audio_rx.recv().await {
            all.push(chunk);
        }
        all
    });

    tokio::time::timeout(
        Duration::from_secs(30),
        tts.synthesize(
            "hallo".to_string(),
            audio_tx,
            cancel_rx,
            IoLivenessMark::disabled(),
        ),
    )
    .await
    .expect("the synthesis must finish within 30 s")
    .expect("synthesis");
    let chunks = collect.await.expect("collect");

    assert_eq!(
        chunks,
        vec![vec![0x01, 0x02], vec![0x03, 0x04, 0x05], vec![0xFF, 0xFE],],
        "the Base64 payloads decode to exactly the scripted bytes, in order"
    );
    assert_eq!(mock.connections().await, 1, "one socket per synthesize");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_stops_stream_and_sends_cancel() {
    let mock = MockCartesia::start(
        CartesiaScript::new()
            .chunk(b"\x01")
            .chunk(b"\x02")
            .chunk(b"\x03")
            // A delay, so the cancel meets a stream that is still running
            // rather than racing an already finished one.
            .with_chunk_delay(Duration::from_millis(80)),
    )
    .await
    .expect("mock");
    let tts = CartesiaTts::new(params(&mock.base_url()));
    let (audio_tx, mut audio_rx, cancel_tx, cancel_rx) = wiring();

    let task = tokio::spawn(async move {
        tts.synthesize(
            "hallo".to_string(),
            audio_tx,
            cancel_rx,
            IoLivenessMark::disabled(),
        )
        .await
    });

    assert_eq!(
        recv_chunk(&mut audio_rx, "the first chunk").await,
        vec![0x01]
    );
    cancel_tx.send(true).expect("cancel");

    let verdict = finish(task, "the cancelled synthesis").await;
    assert!(
        matches!(verdict, Err(TtsError::Cancelled)),
        "a cancelled synthesis reports Cancelled, got {verdict:?}"
    );
    // Nothing may follow a cancel: the sender is gone with the future, so the
    // channel is closed, and it must be closed EMPTY.
    assert!(
        tokio::time::timeout(Duration::from_secs(30), audio_rx.recv())
            .await
            .expect("the audio channel must close within 30 s")
            .is_none(),
        "no chunk may reach the caller after the cancel"
    );
    wait_for_cancel(&mock).await;
    let cancels = mock.cancels().await;
    let reqs = mock.received_requests().await;
    assert_eq!(
        cancels[0], reqs[0]["context_id"],
        "the cancel names the context it cancels"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropped_receiver_cancels() {
    let mock = MockCartesia::start(
        CartesiaScript::new()
            .chunk(b"\x01")
            .chunk(b"\x02")
            .chunk(b"\x03")
            .with_chunk_delay(Duration::from_millis(80)),
    )
    .await
    .expect("mock");
    let tts = CartesiaTts::new(params(&mock.base_url()));
    let (audio_tx, mut audio_rx, _cancel_tx, cancel_rx) = wiring();

    let task = tokio::spawn(async move {
        tts.synthesize(
            "hallo".to_string(),
            audio_tx,
            cancel_rx,
            IoLivenessMark::disabled(),
        )
        .await
    });

    assert_eq!(
        recv_chunk(&mut audio_rx, "the first chunk").await,
        vec![0x01]
    );
    // Nobody wants the audio any more — the connection task went away.
    drop(audio_rx);

    let verdict = finish(task, "the abandoned synthesis").await;
    assert!(
        matches!(verdict, Err(TtsError::Cancelled)),
        "dropping the receiver ends the synthesis as cancelled, got {verdict:?}"
    );
    // And the provider hears about it, rather than just being abandoned.
    wait_for_cancel(&mock).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn error_frame_is_protocol_error() {
    let mock = MockCartesia::start(CartesiaScript::new().chunk(b"\x01").failing_after(1))
        .await
        .expect("mock");
    let tts = CartesiaTts::new(params(&mock.base_url()));
    let (audio_tx, mut audio_rx, _cancel_tx, cancel_rx) = wiring();
    let drain = tokio::spawn(async move { while audio_rx.recv().await.is_some() {} });

    let verdict = tokio::time::timeout(
        Duration::from_secs(30),
        tts.synthesize(
            "hallo".to_string(),
            audio_tx,
            cancel_rx,
            IoLivenessMark::disabled(),
        ),
    )
    .await
    .expect("the synthesis must end within 30 s");
    drain.await.expect("drain");

    match verdict {
        Err(TtsError::Protocol(detail)) => {
            assert!(
                detail.contains("internal_error"),
                "the error frame's code reaches the caller: {detail}"
            );
            assert!(
                !detail.contains("test-key"),
                "no error ever carries the credential"
            );
        }
        other => panic!("an error frame must be a protocol error, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connect_timeout_is_a_timeout() {
    let mock =
        MockCartesia::start(CartesiaScript::new().with_accept_delay(Duration::from_secs(30)))
            .await
            .expect("mock");
    let tts = CartesiaTts::new(params(&mock.base_url())).with_timeouts(ProviderTimeouts {
        external: Duration::from_millis(150),
        idle: Duration::from_secs(30),
    });
    let (audio_tx, _audio_rx, _cancel_tx, cancel_rx) = wiring();

    let verdict = tokio::time::timeout(
        Duration::from_secs(30),
        tts.synthesize(
            "hallo".to_string(),
            audio_tx,
            cancel_rx,
            IoLivenessMark::disabled(),
        ),
    )
    .await
    .expect("the synthesis must end within 30 s");
    assert!(
        matches!(verdict, Err(TtsError::Timeout)),
        "a handshake that never answers hits the A-timeout, got {verdict:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refused_key_is_an_auth_error() {
    let mock = MockCartesia::start(CartesiaScript::new().refusing_with_status(401))
        .await
        .expect("mock");
    let tts = CartesiaTts::new(params(&mock.base_url()));
    let (audio_tx, _audio_rx, _cancel_tx, cancel_rx) = wiring();

    let verdict = tokio::time::timeout(
        Duration::from_secs(30),
        tts.synthesize(
            "hallo".to_string(),
            audio_tx,
            cancel_rx,
            IoLivenessMark::disabled(),
        ),
    )
    .await
    .expect("the synthesis must end within 30 s");
    match verdict {
        Err(TtsError::Auth(detail)) => assert!(
            !detail.contains("test-key"),
            "an auth error names the status, never the key: {detail}"
        ),
        other => panic!("a 401 on the upgrade is an auth error, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_provider_that_goes_quiet_hits_the_idle_deadline() {
    // One chunk, then silence with the socket still open — the only shape that
    // an operation timeout cannot catch, because nothing is pending.
    let mock = MockCartesia::start(
        CartesiaScript::new()
            .chunk(b"\x01")
            .chunk(b"\x02")
            .going_silent_after(1),
    )
    .await
    .expect("mock");
    let tts = CartesiaTts::new(params(&mock.base_url())).with_timeouts(ProviderTimeouts {
        external: Duration::from_secs(30),
        idle: Duration::from_millis(200),
    });
    let (audio_tx, mut audio_rx, _cancel_tx, cancel_rx) = wiring();
    let task = tokio::spawn(async move {
        tts.synthesize(
            "hallo".to_string(),
            audio_tx,
            cancel_rx,
            IoLivenessMark::disabled(),
        )
        .await
    });

    assert_eq!(
        recv_chunk(&mut audio_rx, "the first chunk").await,
        vec![0x01]
    );
    let verdict = finish(task, "the synthesis on a silent provider").await;
    assert!(
        matches!(verdict, Err(TtsError::Timeout)),
        "a provider that stops talking is a timeout, not a stall: {verdict:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_chunk_marks_liveness() {
    let mock = MockCartesia::start(CartesiaScript::new().chunk(b"\x01").chunk(b"\x02"))
        .await
        .expect("mock");
    let tts = CartesiaTts::new(params(&mock.base_url()));
    let (audio_tx, mut audio_rx, _cancel_tx, cancel_rx) = wiring();
    let drain = tokio::spawn(async move { while audio_rx.recv().await.is_some() {} });
    let (colony_tx, mut colony_rx) = mpsc::channel::<ColonyMsg>(16);
    let liveness = IoLivenessMark::new(Path::new("/channels/voice"), Some(colony_tx));

    tokio::time::timeout(
        Duration::from_secs(30),
        tts.synthesize("hallo".to_string(), audio_tx, cancel_rx, liveness),
    )
    .await
    .expect("the synthesis must finish within 30 s")
    .expect("synthesis");
    drain.await.expect("drain");

    let mut marks = 0usize;
    while let Ok(msg) = colony_rx.try_recv() {
        match msg {
            ColonyMsg::IoLiveness { path, at } => {
                assert_eq!(path.as_str(), "/channels/voice");
                assert!(at.is_some(), "a provider round trip carries its time");
                marks += 1;
            }
            _ => panic!("only liveness marks are expected on this channel"),
        }
    }
    // One for the completed handshake, one per chunk.
    assert_eq!(
        marks, 3,
        "the handshake and each of the two chunks are round trips"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dead_endpoint_is_a_connect_error_with_a_reason() {
    // Nothing listens on this port: the failure is neither HTTP nor a timeout,
    // so it must arrive as Connect with the transport's own words. The key
    // travels in a header, so no URL — and no credential — can reach the text.
    let port = meclaw_testing::free_port();
    let tts = CartesiaTts::new(params(&format!("ws://127.0.0.1:{port}")));
    let (audio_tx, _audio_rx, _cancel_tx, cancel_rx) = wiring();

    let verdict = tokio::time::timeout(
        Duration::from_secs(30),
        tts.synthesize(
            "hallo".to_string(),
            audio_tx,
            cancel_rx,
            IoLivenessMark::disabled(),
        ),
    )
    .await
    .expect("the synthesis must end within 30 s");
    match verdict {
        Err(TtsError::Connect(detail)) => {
            assert!(
                detail.len() > "cartesia connect: ".len(),
                "the transport's reason must survive into the message: {detail}"
            );
            assert!(
                !detail.contains("test-key"),
                "never the credential: {detail}"
            );
        }
        other => panic!("an unreachable endpoint is a connect error, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropped_cancel_sender_is_not_a_cancel() {
    // The handler may drop its end of the cancel channel while a synthesis is
    // perfectly healthy. `watch::Receiver::changed()` then reports `Err`, and
    // reading that as "cancelled" would cut every turn short the moment the
    // sender goes out of scope. It is not a cancel: only a `true` value or a
    // gone audio receiver is.
    let mock = MockCartesia::start(
        CartesiaScript::new()
            .chunk(b"\x01")
            .chunk(b"\x02")
            .chunk(b"\x03")
            .with_chunk_delay(Duration::from_millis(20)),
    )
    .await
    .expect("mock");
    let tts = CartesiaTts::new(params(&mock.base_url()));
    let (audio_tx, mut audio_rx, cancel_tx, cancel_rx) = wiring();

    let task = tokio::spawn(async move {
        tts.synthesize(
            "hallo".to_string(),
            audio_tx,
            cancel_rx,
            IoLivenessMark::disabled(),
        )
        .await
    });
    // Drop the sender while the stream is still running.
    drop(cancel_tx);

    let mut chunks = Vec::new();
    while let Some(chunk) = tokio::time::timeout(Duration::from_secs(30), audio_rx.recv())
        .await
        .expect("the audio channel must settle within 30 s")
    {
        chunks.push(chunk);
    }
    let verdict = finish(task, "the synthesis with no cancel sender left").await;
    assert!(
        verdict.is_ok(),
        "a dropped cancel sender must not end the synthesis: {verdict:?}"
    );
    assert_eq!(
        chunks,
        vec![vec![0x01], vec![0x02], vec![0x03]],
        "and every chunk still arrives"
    );
    assert!(
        !mock.cancelled().await,
        "no cancel frame may be sent for a synthesis that was never cancelled"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_provider_that_never_answers_hits_the_operation_deadline() {
    // Until the first frame the deadline is the A-timeout (`external`), not the
    // idle deadline: the request is a bounded operation, and an answer that
    // never starts is an elapsed operation. `idle` is deliberately long here,
    // so only the `external` phase can end this.
    let mock = MockCartesia::start(CartesiaScript::new().chunk(b"\x01").going_silent_after(0))
        .await
        .expect("mock");
    let tts = CartesiaTts::new(params(&mock.base_url())).with_timeouts(ProviderTimeouts {
        external: Duration::from_millis(200),
        idle: Duration::from_secs(300),
    });
    let (audio_tx, _audio_rx, _cancel_tx, cancel_rx) = wiring();

    let verdict = tokio::time::timeout(
        Duration::from_secs(30),
        tts.synthesize(
            "hallo".to_string(),
            audio_tx,
            cancel_rx,
            IoLivenessMark::disabled(),
        ),
    )
    .await
    .expect("the operation deadline must end this well within 30 s");
    assert!(
        matches!(verdict, Err(TtsError::Timeout)),
        "no first frame within `external` is a timeout, got {verdict:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_error_frame_with_401_is_an_auth_error() {
    // The wire has two ways to say "your key is wrong": a refused upgrade and
    // an `error` frame on an established socket. Both must land as `Auth`.
    let mock = MockCartesia::start(
        CartesiaScript::new()
            .chunk(b"\x01")
            .failing_after(0)
            .with_error_status(401),
    )
    .await
    .expect("mock");
    let tts = CartesiaTts::new(params(&mock.base_url()));
    let (audio_tx, _audio_rx, _cancel_tx, cancel_rx) = wiring();

    let verdict = tokio::time::timeout(
        Duration::from_secs(30),
        tts.synthesize(
            "hallo".to_string(),
            audio_tx,
            cancel_rx,
            IoLivenessMark::disabled(),
        ),
    )
    .await
    .expect("the synthesis must end within 30 s");
    match verdict {
        Err(TtsError::Auth(detail)) => {
            assert!(
                !detail.contains("test-key"),
                "never the credential: {detail}"
            );
        }
        other => panic!("a 401 error frame is an auth error, got {other:?}"),
    }
}
