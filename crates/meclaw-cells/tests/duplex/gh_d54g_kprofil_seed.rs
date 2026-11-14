//! D54g kprofil (GH #1507, contract points 1 and 2) -- the ask for a start
//! window names the limits of its target, and the seed is trimmed by params.
//!
//! WHY this file exists: a cell that holds a session of its own reports no
//! package, so whoever builds its start window could not know how large it
//! may be. The cell cut every seed at a fixed 8192 tokens and 128 messages
//! and counted characters / 4, while the instructions it opens the session
//! with were never counted at all -- the provider takes 16 384 tokens of
//! instructions and 8192 tokens of start input, and R-58-28 asks for the
//! whole start window to stay under the former. Now:
//!
//! * `seed_ask` carries `hop.limits` beside `reason` and `renewal_n`:
//!   `{model, start_budget, feed_budget, chars_per_token, profile}`;
//! * `start_budget` is the smaller of `seed_max_tokens` (shipped 8192) and
//!   the provider's limit for the instructions minus the estimated size of
//!   `instructions` and `greeting`;
//! * `feed_budget` is `advise_max_tokens`, `chars_per_token` the rate the
//!   cell estimates with, `profile` the param `window_profile` as it is;
//! * the adapter trims a seed by `seed_max_tokens` and `seed_max_messages`,
//!   estimated at `chars_per_token`, not by constants and characters / 4.
//!
//! The ask is measured at the seam with the fake provider of
//! `tests/support/duplex_cell.rs`, the trim at the adapter against the
//! hermetic fake socket; no model, no time verdict.

// harness_fold: #[path = "support/duplex_cell.rs"]
#[allow(unused_imports)]
use crate::duplex_cell; // harness_fold: mod duplex_cell;

use meclaw_cells::voice::contract::{
    DuplexControl, DuplexError, DuplexEvent, DuplexProvider, DuplexSession, ProviderTimeouts,
    SeedMessage,
};
use meclaw_cells::voice::params::GptLiveParams;
use meclaw_cells::voice::providers::gpt_live::GptLiveDuplex;
use meclaw_colony::IoLivenessMark;
use meclaw_core::serde_json::{self as sj, Value, json};
use meclaw_testing::mock_gpt_live::{LiveAction, LiveScript, MockGptLive};
use std::time::Duration;
use tokio::sync::mpsc;

/// The provider's limit for the instructions of a session, in tokens.
const INSTRUCTIONS_MAX_TOKENS: u64 = 16_384;

/// The shipped ceiling of a seed, in tokens.
const SEED_MAX_TOKENS: u64 = 8_192;

/// How long the opening waits for its seed in these pins.
const SEED_WAIT_MS: u64 = 400;

/// A grace no pin here reaches.
const NO_GRACE_MS: u64 = 60_000;

/// Tokens of `text` at `rate` characters a token, rounded up.
fn est(text: &str, rate: u64) -> u64 {
    (text.chars().count() as u64).div_ceil(rate)
}

/// The params of a duplex cell whose live block carries `extra`.
fn live_params(extra: Value) -> Value {
    let mut block = json!({
        "provider": "gpt_live",
        "api_key": "test-key",
        "model": "test-live-model",
        "instructions": "A test line.",
        "sample_rate": 16_000,
    });
    if let (Some(obj), Some(more)) = (block.as_object_mut(), extra.as_object()) {
        for (k, v) in more {
            obj.insert(k.clone(), v.clone());
        }
    }
    duplex_cell::duplex_params(json!({"default_mode": "auto", "duplex": block}))
}

/// The `limits` of the `seed_ask` a call of a cell with `extra` in its live
/// block opens with.
async fn limits_of(extra: Value, call: &str) -> Value {
    let mut live =
        duplex_cell::boot_seeded(live_params(extra), 0, 0, NO_GRACE_MS, SEED_WAIT_MS).await;
    let (_client, hello) = live
        .connect(&format!("session={call}&sample_rate=16000&mode=auto"))
        .await;
    assert_eq!(hello["duplex"], true, "got {hello}");
    let ask = live.emission("seed_ask").await;
    assert_eq!(duplex_cell::header(&ask, "reason"), Some(&json!("start")));
    assert_eq!(duplex_cell::header(&ask, "renewal_n"), Some(&json!(0)));
    duplex_cell::header(&ask, "limits")
        .cloned()
        .unwrap_or_else(|| panic!("the ask names the limits of its target: {ask}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_kprofil_the_start_ask_names_the_limits_of_its_target() {
    // Every knob set: the ask says what the block says.
    let limits = limits_of(
        json!({
            "seed_max_tokens": 4000,
            "advise_max_tokens": 300,
            "chars_per_token": 4,
            "window_profile": {"feed_own": "context"},
        }),
        "limits-1",
    )
    .await;
    assert_eq!(limits["model"], json!("test-live-model"), "{limits}");
    assert_eq!(limits["start_budget"], json!(4000), "{limits}");
    assert_eq!(limits["feed_budget"], json!(300), "{limits}");
    assert_eq!(limits["chars_per_token"].as_f64(), Some(4.0), "{limits}");
    assert_eq!(
        limits["profile"],
        json!({"feed_own": "context"}),
        "the profile goes out as the param holds it, unread: {limits}"
    );
    let mut keys: Vec<&str> = limits
        .as_object()
        .map(|o| o.keys().map(String::as_str).collect())
        .unwrap_or_default();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "chars_per_token",
            "feed_budget",
            "model",
            "profile",
            "start_budget"
        ],
        "five fields and no other"
    );

    // No knob set: the shipped values -- the seed's ceiling as it was, the
    // provider's bound of one append, an empty profile, a rate above zero.
    let shipped = limits_of(json!({}), "limits-2").await;
    assert_eq!(shipped["start_budget"], json!(SEED_MAX_TOKENS), "{shipped}");
    assert_eq!(shipped["feed_budget"], json!(500), "{shipped}");
    assert_eq!(shipped["profile"], json!({}), "{shipped}");
    // Review 1 M4: the shipped rate is the block's own default, the rate the
    // appends of the cell count with -- read off the params, not a second
    // number in this pin.
    let shipped_rate = adapter_params("http://127.0.0.1:1", json!({})).chars_per_token;
    assert!(shipped_rate > 0.0, "the shipped rate is above zero");
    assert_eq!(
        shipped["chars_per_token"].as_f64(),
        Some(shipped_rate),
        "the ask names the rate the block ships with: {shipped}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_kprofil_the_start_budget_leaves_room_for_the_instructions() {
    // 40 000 characters of instructions and 300 of greeting at four
    // characters a token: 10 000 and 75 tokens of the 16 384 are taken.
    let instructions = "word ".repeat(8_000);
    let greeting = "hello there ".repeat(25);
    let room = INSTRUCTIONS_MAX_TOKENS - est(&instructions, 4) - est(&greeting, 4);
    assert!(
        room < SEED_MAX_TOKENS,
        "the fixture is over the seed ceiling"
    );

    let limits = limits_of(
        json!({"chars_per_token": 4, "instructions": instructions, "greeting": greeting}),
        "room-1",
    )
    .await;
    assert_eq!(
        limits["start_budget"],
        json!(room),
        "what the instructions and the greeting leave of the provider's limit: {limits}"
    );
    let budget = limits["start_budget"].as_u64().unwrap_or(u64::MAX);
    assert!(
        budget + est(&instructions, 4) + est(&greeting, 4) <= INSTRUCTIONS_MAX_TOKENS,
        "the whole start window stays under the provider's limit"
    );

    // A ceiling below the room wins: the smaller of the two.
    let limits = limits_of(
        json!({"chars_per_token": 4, "instructions": instructions, "greeting": greeting,
               "seed_max_tokens": 1000}),
        "room-2",
    )
    .await;
    assert_eq!(limits["start_budget"], json!(1000), "{limits}");
}

// --- the adapter against the hermetic fake socket ---

/// Failure marker, never a timing verdict.
const FAILURE_TIMEOUT: Duration = Duration::from_secs(30);

struct Session {
    events: mpsc::Receiver<DuplexEvent>,
    control: mpsc::Sender<DuplexControl>,
    _audio_in: mpsc::Sender<Vec<u8>>,
    _audio_out: mpsc::Receiver<Vec<u8>>,
    _join: tokio::task::JoinHandle<Result<(), DuplexError>>,
}

fn adapter_params(base_url: &str, extra: Value) -> GptLiveParams {
    let mut raw = json!({
        "api_key": "test-key",
        "base_url": base_url,
        "instructions": "A test line.",
        "seed_wait_ms": 20_000,
    });
    if let (Some(obj), Some(more)) = (raw.as_object_mut(), extra.as_object()) {
        for (k, v) in more {
            obj.insert(k.clone(), v.clone());
        }
    }
    sj::from_value(raw).expect("the params of the live block parse")
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
        events: events_rx,
        control: control_tx,
        _audio_in: audio_in_tx,
        _audio_out: audio_out_rx,
        _join: tokio::spawn(future),
    }
}

/// The texts of the start input a session with `extra` in its params opens
/// with, given `seed`.
async fn start_input(extra: Value, seed: &[SeedMessage]) -> Vec<String> {
    let script = LiveScript {
        session_id: "sess_limits".to_string(),
        actions: vec![LiveAction::Delay(Duration::from_secs(60))],
        ..LiveScript::default()
    };
    let mock = MockGptLive::start(script).await.expect("bind fake");
    let mut session = start(GptLiveDuplex::new(adapter_params(&mock.base_url(), extra)));
    session
        .control
        .send(DuplexControl::Seed {
            messages: seed.to_vec(),
        })
        .await
        .expect("the opening takes its seed");
    loop {
        let e = tokio::time::timeout(FAILURE_TIMEOUT, session.events.recv())
            .await
            .expect("HANG: no event")
            .expect("events open");
        if matches!(e, DuplexEvent::Started { .. }) {
            break;
        }
    }
    let frame = tokio::time::timeout(FAILURE_TIMEOUT, async {
        loop {
            if let Some(f) = mock.session_start().await {
                return f;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("HANG: no session.start reached the fake");
    frame["session"]["input"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|m| m["content"][0]["text"].as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_kprofil_the_seed_is_trimmed_by_the_params() {
    // Six messages of 80 characters: 20 tokens each at four characters a
    // token, 40 at two.
    let seed: Vec<SeedMessage> = (0..6)
        .map(|i| SeedMessage {
            role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
            text: format!("{i}{}", "x".repeat(79)),
        })
        .collect();
    let newest = |n: usize| -> Vec<String> {
        seed[seed.len() - n..]
            .iter()
            .map(|m| m.text.clone())
            .collect()
    };

    // The count of messages is a param.
    let kept = start_input(json!({"chars_per_token": 4, "seed_max_messages": 3}), &seed).await;
    assert_eq!(
        kept,
        newest(3),
        "seed_max_messages 3 keeps the newest three"
    );

    // The tokens are a param.
    let kept = start_input(json!({"chars_per_token": 4, "seed_max_tokens": 40}), &seed).await;
    assert_eq!(kept, newest(2), "seed_max_tokens 40 keeps two of 20 tokens");

    // The estimate uses the rate of the block, not characters / 4.
    let kept = start_input(json!({"chars_per_token": 2, "seed_max_tokens": 40}), &seed).await;
    assert_eq!(kept, newest(1), "at two characters a token one of 40 fits");

    // OR-d54g.kprofil.14: the cut is the start budget the ask names, not the
    // ceiling alone. 65 376 characters of instructions at four a token take
    // 16 344 of the provider's 16 384 tokens; 40 are left, so two messages of
    // 20 stay although `seed_max_tokens` ships far above the seed.
    let long = "x".repeat(65_376);
    let kept = start_input(json!({"chars_per_token": 4, "instructions": long}), &seed).await;
    assert_eq!(
        kept,
        newest(2),
        "the instructions leave 40 tokens: the seed is cut to them"
    );

    // No knob set: the shipped ceilings carry the six.
    let kept = start_input(json!({}), &seed).await;
    assert_eq!(
        kept,
        newest(6),
        "the shipped ceilings are far above the seed"
    );
}
