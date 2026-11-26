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
//! * `start_budget` is the smaller of `seed_max_tokens` (shipped 8192) minus
//!   the estimated size of `greeting` (GH #1628: it is a message of the same
//!   start input) and the provider's limit for the instructions minus the
//!   estimated size of `instructions` and `greeting`;
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
    // GH #1628: the greeting is part of the start input, so it counts there too.
    assert_eq!(
        limits["start_budget"],
        json!(1000 - est(&greeting, 4)),
        "{limits}"
    );
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

fn start(provider: GptLiveDuplex, renewed: bool) -> Session {
    let (audio_in_tx, audio_in_rx) = mpsc::channel(32);
    let (audio_out_tx, audio_out_rx) = mpsc::channel(32);
    let (events_tx, events_rx) = mpsc::channel(64);
    let (control_tx, control_rx) = mpsc::channel(16);
    let provider = provider.with_timeouts(ProviderTimeouts {
        external: Duration::from_millis(5_000),
        idle: Duration::from_millis(30_000),
    });
    let session = DuplexSession {
        audio_in: audio_in_rx,
        audio_out: audio_out_tx,
        events: events_tx,
        control: control_rx,
    };
    let liveness = IoLivenessMark::disabled();
    let future = if renewed {
        provider.run_renewed_session(provider.format(), session, liveness)
    } else {
        provider.run_session(provider.format(), session, liveness)
    };
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
    session_input(extra, seed, false).await
}

/// [`start_input`], of the renewed session of a running call where `renewed`.
async fn session_input(extra: Value, seed: &[SeedMessage], renewed: bool) -> Vec<String> {
    let script = LiveScript {
        session_id: "sess_limits".to_string(),
        actions: vec![LiveAction::Delay(Duration::from_secs(60))],
        ..LiveScript::default()
    };
    let mock = MockGptLive::start(script).await.expect("bind fake");
    let mut session = start(
        GptLiveDuplex::new(adapter_params(&mock.base_url(), extra)),
        renewed,
    );
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

/// One message of a seed: `text`, filled up to `chars` characters.
fn filled(role: &str, text: &str, chars: usize) -> SeedMessage {
    SeedMessage {
        role: role.into(),
        text: format!("{text}{}", "x".repeat(chars - text.len())),
    }
}

/// GH #1654: in front of a greeting the turns of a seed leave (R-W25fix-1), and
/// the cut counts the form that is sent. A seed whose turns alone go past the
/// limit of tokens or of messages keeps every message in front of and behind
/// them: it is not the oldest context that gives way to turns nobody sends.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_cut_of_a_greeted_seed_counts_the_form_it_is_sent_in() {
    // 160 characters at four a token: 40 tokens, 8 152 are left for the seed.
    let greeting = "g".repeat(160);
    let extra = json!({"chars_per_token": 4, "greeting": greeting});
    let facts = filled("developer", "Today is Tuesday.", 80);
    let note = filled("developer", "Say this once:", 80);
    let after = filled("developer", "Be kind.", 48);
    // 20 + 20 + 12 tokens around 110 turns of 81: far past the budget.
    let turns: Vec<SeedMessage> = (0..110)
        .map(|i| filled("user", &format!("{i:04}"), 324))
        .collect();
    let mut seed = vec![facts.clone(), note.clone()];
    seed.extend(turns.iter().cloned());
    seed.push(after.clone());
    let raw: u64 = seed.iter().map(|m| est(&m.text, 4)).sum();
    assert!(
        raw + 40 > SEED_MAX_TOKENS,
        "the seed goes past the window: {raw}"
    );
    assert_eq!(
        start_input(extra.clone(), &seed).await,
        vec![
            facts.text.clone(),
            note.text.clone(),
            after.text.clone(),
            greeting.clone()
        ],
        "the turns leave, the context around them stays whole"
    );

    // 131 messages, of which 127 have room beside the greeting: counted as
    // sent, the one in front of the turns stays.
    let turns: Vec<SeedMessage> = (0..130)
        .map(|i| filled("user", &format!("{i:04}"), 8))
        .collect();
    assert_eq!(
        start_input(extra, &[&[facts.clone()][..], &turns].concat()).await,
        vec![facts.text, greeting],
        "at the limit of messages too"
    );
}

/// GH #1654: a `developer` message between two turns (a delegation, an
/// advice, an event) belongs to the earlier conversation and leaves with the
/// turns, whatever room there is. What stands around the turns leaves only for
/// the room, the oldest first, as without a greeting.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn what_stands_between_the_turns_leaves_with_them() {
    let greeting = "g".repeat(160);
    let extra = |budget: u64| json!({"chars_per_token": 4, "seed_max_tokens": 40 + budget, "greeting": greeting});
    let head = filled("developer", "Be kind.", 8);
    let seed = [
        head.clone(),
        filled("user", "old", 400),
        filled("developer", "(delegation)", 12),
        filled("user", "new", 20),
        filled("assistant", "Yes.", 20),
    ];
    assert_eq!(
        start_input(extra(1_000), &seed).await,
        vec![head.text, greeting.clone()],
        "the delegation leaves with the turns"
    );

    let long = filled("developer", "long", 200);
    let short = filled("developer", "short", 40);
    let turn = filled("user", "hi", 20);
    assert_eq!(
        start_input(extra(12), &[long, short.clone(), turn]).await,
        vec![short.text, greeting.clone()],
        "the turn leaves, then the oldest context for the room"
    );
}

/// GH #1654: 127 messages of a seed have room beside a greeting, counted
/// without the turns that leave.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_turns_of_a_greeted_seed_take_no_message_of_the_input() {
    let greeting = "g".repeat(160);
    let context: Vec<SeedMessage> = (0..127)
        .map(|i| filled("developer", &format!("{i:04}"), 8))
        .collect();
    let turns: Vec<SeedMessage> = (0..10)
        .map(|i| filled("user", &format!("t{i:03}"), 8))
        .collect();
    let mut want: Vec<String> = context.iter().map(|m| m.text.clone()).collect();
    want.push(greeting.clone());
    assert_eq!(
        start_input(
            json!({"chars_per_token": 4, "greeting": greeting}),
            &[&context[..], &turns].concat()
        )
        .await,
        want,
        "127 messages in front of ten turns stay, the greeting is last"
    );
}

/// GH #1654: a renewed session greets nobody, so no turn leaves: a seed at the
/// limit of tokens or of messages goes out as it came, although the block has
/// a greeting.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_renewed_session_keeps_a_seed_at_its_limit() {
    let extra = json!({"chars_per_token": 4, "greeting": "g".repeat(160)});
    for (n, chars) in [(4, 8_152), (128, 8)] {
        let turns: Vec<SeedMessage> = (0..n)
            .map(|i| filled("user", &format!("{i:04}"), chars))
            .collect();
        let want: Vec<String> = turns.iter().map(|m| m.text.clone()).collect();
        let kept = session_input(extra.clone(), &turns, true).await;
        assert!(kept == want, "{n} turns of {chars}: {} kept", kept.len());
    }
}

/// GH #1654: a seed of nothing but turns leaves the greeting alone, never an
/// empty message in front of it; a message behind the turns stays.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_greeted_seed_of_turns_leaves_the_greeting_alone() {
    let greeting = "g".repeat(160);
    let turns: Vec<SeedMessage> = (0..127)
        .map(|i| SeedMessage {
            role: "user".into(),
            text: format!("{i:04}{}", "x".repeat(252)),
        })
        .collect();
    assert_eq!(
        start_input(json!({"chars_per_token": 4, "greeting": greeting}), &turns).await,
        vec![greeting.clone()],
        "no turn and no empty message, the greeting is the input"
    );

    let extra = json!({"chars_per_token": 4, "seed_max_tokens": 50, "greeting": greeting});
    let turn = SeedMessage {
        role: "user".into(),
        text: "x".repeat(20),
    };
    let after = SeedMessage {
        role: "developer".into(),
        text: "Be kind.".into(),
    };
    assert_eq!(
        start_input(extra, &[turn, after]).await,
        vec!["Be kind.".to_string(), greeting.clone()],
        "the turn leaves, the message behind it stays"
    );
}

/// GH #1628: the greeting is the last message of the start input and is never
/// cut; the seed gives way to it, in tokens and in messages (the provider
/// takes 8 192 tokens and 128 messages of input). A seed that does not fit
/// beside it at all leaves the greeting after the configured start.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_seed_gives_way_to_the_greeting_at_the_input_limit() {
    let seed = |role: &str, n: usize, chars: usize| -> Vec<SeedMessage> {
        (0..n)
            .map(|i| SeedMessage {
                role: role.into(),
                text: format!("{i:04}{}", "x".repeat(chars - 4)),
            })
            .collect()
    };
    // 160 characters at four a token: 40 tokens.
    let greeting = "g".repeat(160);

    // Four messages of 2 048 tokens fill the 8 192 exactly; beside the
    // greeting one of them has to leave, and the greeting stays whole.
    // `developer` messages, which a greeting keeps (turns leave, GH #1654).
    let four = seed("developer", 4, 8_192);
    let kept = start_input(json!({"chars_per_token": 4}), &four).await;
    assert_eq!(kept.len(), 4, "without a greeting the seed fills the limit");
    let kept = start_input(json!({"chars_per_token": 4, "greeting": greeting}), &four).await;
    let mut want: Vec<String> = four[1..].iter().map(|m| m.text.clone()).collect();
    want.push(greeting.clone());
    assert_eq!(kept, want, "the oldest leaves, the greeting is last");
    let total: u64 = kept.iter().map(|t| est(t, 4)).sum();
    assert!(total <= SEED_MAX_TOKENS, "the input stays under the limit");

    // 128 messages: one leaves for the greeting.
    let many = seed("developer", 128, 8);
    let kept = start_input(json!({"chars_per_token": 4}), &many).await;
    assert_eq!(kept.len(), 128, "without a greeting 128 messages fit");
    let kept = start_input(json!({"chars_per_token": 4, "greeting": greeting}), &many).await;
    assert_eq!(kept.len(), 128, "127 of the seed and the greeting");
    assert_eq!(kept[0], many[1].text, "the oldest message left");
    assert_eq!(kept.last(), Some(&greeting));

    // Nothing is left for the seed: the configured start and the greeting.
    let kept = start_input(
        json!({
            "chars_per_token": 4,
            "seed_max_tokens": 40,
            "greeting": greeting,
            "start_input": [{"role": "developer", "text": "Be kind."}],
        }),
        &many,
    )
    .await;
    assert_eq!(
        kept,
        vec!["Be kind.".to_string(), greeting.clone()],
        "a budget of 0 drops the seed, never the greeting"
    );
}
