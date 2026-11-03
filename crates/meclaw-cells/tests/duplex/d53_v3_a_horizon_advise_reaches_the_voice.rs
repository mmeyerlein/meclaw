//! #1400 -- the curator's horizon reaches the live model as ONE instruction,
//! never into running speech.
//!
//! WHY this file exists: the curator tells a duplex call how long the answer of
//! a tool round or a delegation will take (`in_advise` section `horizon`, body
//! `seconds`/`way`/`family`, no words). The words are the set's:
//! `duplex.gpt_live.horizon_text` with `{seconds}` filled in, appended as an
//! instruction (the `correction` channel). On a quiet line it goes at once;
//! otherwise the connection holds it to the line's first quiet moment -- no
//! spoken section open and the model's OWN speech (its audio) `spoken_quiet_ms`
//! past, the takeover's rule (o8.6) -- because an append into running speech
//! cut the model off and left it silent (plan d53 T2 § V3, FU-0). A newer held
//! horizon of the same session replaces the older one, which is never sent. `hop.at_turn_end` is
//! not the rule (o8.5): it holds to the caller's NEXT turn, and the horizon is
//! made right after one closed. An empty `horizon_text` (the default) drops the
//! section with a warning and no refusal: the cell stays what it was.

// harness_fold: #[path = "support/duplex_cell.rs"]
#[allow(unused_imports)]
use crate::duplex_cell; // harness_fold: mod duplex_cell;

use duplex_cell::{CAP_MS, Live, boot_fake, boot_fake_timed, duplex_params, header};
use meclaw_cells::voice::contract::{AppendKind, DuplexControl};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};

const HORIZON_TEXT: &str = "The answer takes about {seconds} seconds; keep the talk going.";

/// The quiet the connection waits for after the model's last speech: long
/// enough that a horizon sent right after a chunk cannot outrun it, short
/// enough to wait for.
const QUIET_MS: u64 = 3_000;

/// One 20 ms frame of the model's audio at 16 kHz.
fn model_audio() -> Vec<u8> {
    (0..320).flat_map(|_| 8_000i16.to_le_bytes()).collect()
}

/// The next audio frame the caller gets, skipping text frames.
async fn next_audio(client: &mut meclaw_testing::voice_client::VoiceClient) -> Vec<u8> {
    loop {
        let frame = client
            .next_frame(duplex_cell::marker())
            .await
            .expect("HANG: the caller gets its next frame");
        if let Some(audio) = frame.as_audio() {
            return audio.to_vec();
        }
    }
}

fn params(horizon_text: &str) -> Value {
    duplex_params(json!({
        "default_mode": "auto",
        "duplex": {
            "provider": "gpt_live",
            "api_key": "test-key",
            "instructions": "A test line.",
            "sample_rate": 16_000,
            "tick_ms": 100,
            "turn_gap_ms": 300,
            "horizon_text": horizon_text,
        },
    }))
}

/// The curator's horizon as the set hands it on: no words, the numbers only.
fn horizon(call: &str, seconds: u64) -> Message {
    let mut context = Map::new();
    context.insert("call_id".into(), json!(call));
    let mut hop = Map::new();
    hop.insert("route".into(), json!("in_advise"));
    hop.insert("section".into(), json!("horizon"));
    MessageBuilder::new(Path::new("/voice"))
        .context(context)
        .hop(hop)
        .body(Body::Inline(json!({
            "messages": [], "section": "horizon", "seconds": seconds,
            "way": "quick", "family": "web"
        })))
        .build()
}

fn marker(call: &str, section: &str, text: &str) -> Message {
    duplex_cell::advise_msg(call, section, text, None)
}

/// Every append up to and including the one carrying `content`.
async fn appends_until(live: &mut Live, content: &str) -> Vec<(AppendKind, String)> {
    let mut out = Vec::new();
    loop {
        let control = tokio::time::timeout(duplex_cell::marker(), live.controls.recv())
            .await
            .unwrap_or_else(|_| panic!("HANG: `{content}` never reached the model; saw {out:?}"))
            .expect("the fake is running");
        if let DuplexControl::Append {
            kind, content: c, ..
        } = control
        {
            let done = c == content;
            out.push((kind, c));
            if done {
                return out;
            }
        }
    }
}

/// Pin 5: while a spoken section is open (the bridge) held, and sent at the
/// first quiet moment after it ends as ONE instruction -- the newer horizon,
/// never both.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_horizon_advise_becomes_one_corrected_instruction_at_the_turn_boundary() {
    // The section ends only by the caller's cancel: the fake sends no audio,
    // so no quiet of the connection's heuristic closes it, and the cap is far.
    let mut live = boot_fake_timed(params(HORIZON_TEXT), QUIET_MS, CAP_MS * 8).await;
    let (mut client, _hello) = live.connect("session=hz-1&mode=auto").await;
    let _session = live.session().await;

    live.send(marker("hz-1", "fact", "One moment, I am looking."))
        .await;
    let bridge = appends_until(&mut live, "One moment, I am looking.").await;
    assert_eq!(bridge.last().map(|(k, _)| *k), Some(AppendKind::Commentary));
    client
        .next_frame_of_type("speak_start", duplex_cell::marker())
        .await
        .expect("the bridge's section opens");

    live.send(horizon("hz-1", 4)).await;
    live.send(horizon("hz-1", 7)).await;
    live.send(marker("hz-1", "context", "Marker.")).await;
    let before = appends_until(&mut live, "Marker.").await;
    assert_eq!(
        before,
        vec![(AppendKind::Thinking, "Marker.".to_string())],
        "held while the model speaks"
    );

    client.cancel().await.expect("the caller cuts in");
    live.emission("speak_end").await;
    let released = appends_until(
        &mut live,
        "The answer takes about 7 seconds; keep the talk going.",
    )
    .await;
    let said: Vec<_> = released
        .iter()
        .filter(|(_, c)| c.contains("seconds"))
        .cloned()
        .collect();
    assert_eq!(
        said,
        vec![(
            AppendKind::Instructions,
            "The answer takes about 7 seconds; keep the talk going.".to_string()
        )],
        "one instruction after the section's end, the newer horizon: {released:?}"
    );
}

/// Pin 5, the model's own speech (o8.6): no section the cell opened, but the
/// model talks (its audio reaches the caller) -- the horizon waits for the
/// first quiet moment and goes then, not into the running speech.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_horizon_waits_while_the_model_speaks_on_its_own() {
    let mut live = boot_fake_timed(params(HORIZON_TEXT), QUIET_MS, CAP_MS * 8).await;
    let (mut client, _hello) = live.connect("session=hz-4&mode=auto").await;
    let session = live.session().await;

    // The model speaks; the caller hears it, so the line has heard it.
    session.audio_out.send(model_audio()).await.expect("audio");
    assert_eq!(next_audio(&mut client).await, model_audio());

    live.send(horizon("hz-4", 5)).await;
    live.send(marker("hz-4", "context", "Marker.")).await;
    let before = appends_until(&mut live, "Marker.").await;
    assert_eq!(
        before,
        vec![(AppendKind::Thinking, "Marker.".to_string())],
        "held while the model speaks"
    );
    let after = appends_until(
        &mut live,
        "The answer takes about 5 seconds; keep the talk going.",
    )
    .await;
    assert_eq!(
        after,
        vec![(
            AppendKind::Instructions,
            "The answer takes about 5 seconds; keep the talk going.".to_string()
        )],
        "one instruction at the quiet moment"
    );
}

/// Pin 5, the other half: on a quiet line the horizon goes at once -- nothing
/// to wait for, and the caller's next turn would come after the answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_horizon_on_a_quiet_line_goes_at_once() {
    let mut live = boot_fake(params(HORIZON_TEXT)).await;
    let (_client, _hello) = live.connect("session=hz-3&mode=auto").await;
    let _session = live.session().await;

    live.send(horizon("hz-3", 4)).await;
    live.send(marker("hz-3", "context", "Marker.")).await;
    let seen = appends_until(&mut live, "Marker.").await;
    assert_eq!(
        seen,
        vec![
            (
                AppendKind::Instructions,
                "The answer takes about 4 seconds; keep the talk going.".to_string()
            ),
            (AppendKind::Thinking, "Marker.".to_string()),
        ],
        "sent at once, before anything after it"
    );
}

/// Pin 6: no `horizon_text` (the default) -- the section is dropped, nothing
/// reaches the model and nothing is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_empty_horizon_text_ignores_the_section() {
    let mut live = boot_fake(params("")).await;
    let (_client, _hello) = live.connect("session=hz-2&mode=auto").await;
    let _session = live.session().await;

    live.send(horizon("hz-2", 4)).await;
    live.send(marker("hz-2", "context", "Marker.")).await;
    let seen = appends_until(&mut live, "Marker.").await;
    assert_eq!(
        seen,
        vec![(AppendKind::Thinking, "Marker.".to_string())],
        "nothing of the horizon reaches the model"
    );
    // The first refusal after the horizon is the one a bogus section earns:
    // the horizon itself was refused nothing.
    live.send(marker("hz-2", "opinion", "I like that.")).await;
    let refusal = live.emission("error").await;
    assert_eq!(header(&refusal, "error_code"), Some(&json!("bad_section")));
    let detail = refusal["meta"]["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("opinion"), "got {refusal}");
}
