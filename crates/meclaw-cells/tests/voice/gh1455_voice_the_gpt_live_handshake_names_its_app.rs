//! GH #1455 -- the `gpt_live` session handshake names its app.
//!
//! Every LLM request carries the app attribution (`X-Title` + `HTTP-Referer`,
//! GH #1225 for the llm cells). The live model's session socket opened with
//! the credential alone, and `GptLiveParams` refused both keys, so a set could
//! not name the colony on the phone either.
//!
//! Two locks: the duplex block takes `x_title` and `http_referer` and the
//! upgrade request carries them as headers; without them no header goes out
//! (unset means absent, as on the llm lane).
//!
//! Round 2 (OR-38-120): the shipped template's `duplex` is null and the sets
//! switch it on through `override_params` without the two keys, so no
//! delivered instance carried the attribution. The template now names both
//! at the top of `params` (env default, AGENTS.md rule 15); a gpt_live block
//! inherits them, and a block that names its own keeps them.

use meclaw_cells::voice::contract::{DuplexEvent, DuplexProvider, DuplexSession, ProviderTimeouts};
use meclaw_cells::voice::params::{DuplexParams, GptLiveParams, VoiceParams};
use meclaw_cells::voice::providers::gpt_live::GptLiveDuplex;
use meclaw_colony::IoLivenessMark;
use meclaw_testing::mock_gpt_live::{LiveAction, LiveScript, MockGptLive};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::sync::mpsc;

/// Failure-marker timeout: generous on purpose, it never discriminates timing.
const FAILURE_TIMEOUT: Duration = Duration::from_secs(30);

/// The attribution headers the fake saw on the upgrade of one session opened
/// with `extra` merged into the minimal params.
async fn handshake_attribution(extra: Value) -> Vec<(String, String)> {
    handshake_with(|base_url| {
        let mut raw = json!({
            "api_key": "test-key",
            "base_url": base_url,
            "instructions": "Be Egon.",
        });
        for (k, v) in extra.as_object().expect("extra is an object") {
            raw[k] = v.clone();
        }
        serde_json::from_value(raw).expect("params parse")
    })
    .await
}

/// The same, for a whole voice params document: `top` merged into the cell's
/// params, `block` into its gpt_live duplex block, parsed the way a spawn
/// parses it.
async fn cell_handshake_attribution(top: Value, block: Value) -> Vec<(String, String)> {
    handshake_with(|base_url| {
        let mut duplex = json!({
            "provider": "gpt_live",
            "api_key": "test-key",
            "base_url": base_url,
            "instructions": "Be Egon.",
        });
        for (k, v) in block.as_object().expect("block is an object") {
            duplex[k] = v.clone();
        }
        let mut doc = json!({ "mount": "voice", "duplex": duplex });
        for (k, v) in top.as_object().expect("top is an object") {
            doc[k] = v.clone();
        }
        let parsed = VoiceParams::parse(&doc).expect("voice params parse");
        match parsed.duplex {
            Some(DuplexParams::GptLive(p)) => p,
            other => panic!("a gpt_live block: {other:?}"),
        }
    })
    .await
}

async fn handshake_with(build: impl FnOnce(String) -> GptLiveParams) -> Vec<(String, String)> {
    let mock = MockGptLive::start(LiveScript {
        session_id: "sess_app_1".to_string(),
        actions: vec![LiveAction::Delay(Duration::from_secs(60))],
        ..LiveScript::default()
    })
    .await
    .expect("bind fake");
    let params = build(mock.base_url());

    let (_audio_in_tx, audio_in_rx) = mpsc::channel(32);
    let (audio_out_tx, _audio_out_rx) = mpsc::channel(32);
    let (events_tx, mut events_rx) = mpsc::channel(64);
    let (_control_tx, control_rx) = mpsc::channel(16);
    let provider = GptLiveDuplex::new(params).with_timeouts(ProviderTimeouts {
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
    let join = tokio::spawn(future);
    let first = tokio::time::timeout(FAILURE_TIMEOUT, events_rx.recv())
        .await
        .expect("timed out waiting for a duplex event")
        .expect("event channel closed early");
    assert!(
        matches!(first, DuplexEvent::Started { .. }),
        "the session opened: {first:?}"
    );
    assert!(
        mock.authorization_is_bearer().await,
        "the credential still rides as Bearer"
    );
    let seen = mock.attribution().await;
    join.abort();
    seen
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1455_the_handshake_carries_the_app_attribution() {
    let seen = handshake_attribution(json!({
        "x_title": "soul2me c-test",
        "http_referer": "https://soul2.me",
    }))
    .await;
    assert_eq!(
        seen,
        vec![
            ("http-referer".to_string(), "https://soul2.me".to_string()),
            ("x-title".to_string(), "soul2me c-test".to_string()),
        ],
        "the live model's request names its app like every llm request (GH #1225)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1455_unset_attribution_sends_no_header() {
    let seen = handshake_attribution(json!({})).await;
    assert!(
        seen.is_empty(),
        "unset means absent, no empty-header noise: {seen:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1455_voice_the_block_inherits_the_cells_attribution() {
    let seen = cell_handshake_attribution(
        json!({ "x_title": "soul2me c-test", "http_referer": "https://soul2.me" }),
        json!({}),
    )
    .await;
    assert_eq!(
        seen,
        vec![
            ("http-referer".to_string(), "https://soul2.me".to_string()),
            ("x-title".to_string(), "soul2me c-test".to_string()),
        ],
        "a set that switches duplex on without the keys still names its app (OR-38-120)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1455_voice_the_blocks_own_attribution_wins() {
    let seen = cell_handshake_attribution(
        json!({ "x_title": "MeClaw", "http_referer": "https://meclaw.ai" }),
        json!({ "x_title": "soul2me c-line", "http_referer": "https://soul2.me" }),
    )
    .await;
    assert_eq!(
        seen,
        vec![
            ("http-referer".to_string(), "https://soul2.me".to_string()),
            ("x-title".to_string(), "soul2me c-line".to_string()),
        ],
        "the duplex block that names its own attribution keeps it"
    );
}
