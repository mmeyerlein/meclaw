//! D48 / LC2 — the caller's transcript as a speculative stream.
//!
//! The `partial` lane of a duplex session is the stream a curator reads while
//! the caller is still speaking: cumulative (the sentence so far, never the
//! fragment), stamped with the `turn_id` the closed `turn` will carry. Two
//! knobs of `duplex.gpt_live` throttle it per incoming transcript delta
//! (`delta_min_chars`, `delta_min_ms`, both 0 = unthrottled, the behaviour
//! before them), and the last state before a turn closes always leaves. A
//! third, `emit_spoken`, stops the model's side of the stream without the
//! caller's: a telephone line that wants the deltas has no edge for `spoken`.
//!
//! The clock is paused right after the boot (the boot binds the shared surface
//! listener and waits for the mount), and the client connects through the
//! in-process link, so everything after it is timers and channels (TV-Z7).

// harness_fold: #[path = "support/duplex_cell.rs"]
#[allow(unused_imports)]
use crate::duplex_cell; // harness_fold: mod duplex_cell;

use duplex_cell::{FakeSession, Live, boot_fake, duplex_params, emission_route, header};
use meclaw_cells::voice::contract::{DuplexEvent, Speaker};
use meclaw_core::serde_json::{Value, json};
use std::collections::BTreeSet;
use std::time::Duration;

/// A `gpt_live` block the parse accepts, with a literal key that is not one:
/// the fixture runs the fake provider and reads only the knobs of the handler.
fn live_block(extra: Value) -> Value {
    let mut block = json!({
        "provider": "gpt_live",
        "api_key": "test-key",
        "instructions": "A test line."
    });
    if let (Some(obj), Some(more)) = (block.as_object_mut(), extra.as_object()) {
        for (k, v) in more {
            obj.insert(k.clone(), v.clone());
        }
    }
    block
}

/// Boot, pause the clock, open the call on the link.
async fn open(tag: &str, duplex: Value) -> (Live, duplex_cell::LinkClient, FakeSession) {
    let mut live = boot_fake(duplex_params(
        json!({"default_mode": "auto", "duplex": duplex}),
    ))
    .await;
    tokio::time::pause();
    let (client, _hello) = live.link(tag, "auto").await;
    let session = live.session().await;
    (live, client, session)
}

/// Every emission up to and including the next `turn`, in order.
async fn until_turn(live: &mut Live) -> Vec<Value> {
    let mut out = Vec::new();
    loop {
        let e = tokio::time::timeout(duplex_cell::marker(), live.emissions.recv())
            .await
            .unwrap_or_else(|_| {
                let seen: Vec<_> = out
                    .iter()
                    .map(|c: &Value| emission_route(c).unwrap_or_default().to_string())
                    .collect();
                panic!("HANG: no `turn`; saw {seen:?}")
            })
            .expect("the handler is running");
        let is_turn = emission_route(&e.content) == Some("turn");
        out.push(e.content);
        if is_turn {
            return out;
        }
    }
}

/// The lanes of one route, in order.
fn of_route<'a>(emissions: &'a [Value], route: &str) -> Vec<&'a Value> {
    emissions
        .iter()
        .filter(|c| emission_route(c) == Some(route))
        .collect()
}

/// The text of the first message of an emission.
fn text(content: &Value) -> String {
    duplex_cell::message_text(content, 0)
}

/// 30 fragments of 4 characters in 2 s at 12 characters / 250 ms (the
/// telephone set): at most 9 `partial`, each one cumulative, the last one the
/// caller's whole sentence, all on the turn's `turn_id`. Virtual time between
/// the deltas, so both halves of the throttle are wired (review K1).
#[tokio::test(flavor = "current_thread")]
async fn duplex_partial_stream_is_throttled() {
    let (mut live, _client, session) = open(
        "delta-1",
        live_block(json!({"delta_min_chars": 12, "delta_min_ms": 250})),
    )
    .await;

    let step = 2_000 / 30;
    for i in 0..30u64 {
        tokio::time::advance(Duration::from_millis(step)).await;
        session
            .say(DuplexEvent::Transcript {
                speaker: Speaker::User,
                delta: format!("w{i:02} "),
                start_ms: i * step,
                end_ms: i * step + step - 6,
            })
            .await;
    }
    let seen = until_turn(&mut live).await;
    let partials = of_route(&seen, "partial");
    let turn = seen.last().expect("the turn closed");
    let sentence = text(turn);

    assert!(
        partials.len() <= 9 && partials.len() >= 2,
        "a throttled stream, not a flood and not silence: {} partials",
        partials.len()
    );
    for pair in partials.windows(2) {
        assert!(
            text(pair[1]).starts_with(&text(pair[0])) && text(pair[1]).len() > text(pair[0]).len(),
            "every partial is the sentence so far: {:?} then {:?}",
            text(pair[0]),
            text(pair[1])
        );
    }
    assert_eq!(
        text(partials.last().expect("partials")),
        sentence,
        "the last state before the turn closed went out"
    );
    for p in &partials {
        assert_eq!(
            header(p, "turn_id"),
            header(turn, "turn_id"),
            "delta and turn meet on the turn_id"
        );
    }
}

/// The closed turn carries when the caller stopped and when the model's first
/// audio reached the caller, both on the call's timeline, and the `turn_id` of
/// its deltas.
///
/// BAT R1e (C3): with the answer's TRANSCRIPT as the second mark, 12 of 15
/// telephone turns read `t_first_audio_ms == t_person_end_ms` -- a duplex
/// model puts its answer's words right where the caller's end on its own
/// 200 ms raster, and the overlap rule clamped the rest onto them. The mark is
/// now the first audio frame sent to the client: here 700 ms of virtual time
/// after the caller's last fragment arrived, so 900 + 700, while the
/// transcript of that answer still says 1200.
#[tokio::test(flavor = "current_thread")]
async fn duplex_turn_carries_first_audio_time() {
    let (mut live, mut client, session) = open("delta-2", live_block(json!({}))).await;
    let mut partials = Vec::new();

    for (delta, start_ms, end_ms) in [("Is it ", 0, 500), ("raining?", 500, 900)] {
        session
            .say(DuplexEvent::Transcript {
                speaker: Speaker::User,
                delta: delta.to_string(),
                start_ms,
                end_ms,
            })
            .await;
        // The fragment is through the connection (its clock stamped) once
        // the handler has turned it into a partial.
        partials.push(live.emission("partial").await);
    }
    tokio::time::advance(Duration::from_millis(700)).await;
    session
        .audio_out
        .send(speech(9_600))
        .await
        .expect("the connection takes model audio");
    let _ = client.next_audio().await;
    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::Assistant,
            delta: "Not right now.".to_string(),
            start_ms: 1_200,
            end_ms: 1_800,
        })
        .await;

    let seen = until_turn(&mut live).await;
    let turn = seen.last().expect("the turn closed");
    assert_eq!(header(turn, "t_person_end_ms"), Some(&json!(900)), "{turn}");
    assert_eq!(
        header(turn, "t_first_audio_ms"),
        Some(&json!(1_600)),
        "{turn}"
    );
    let person_end = header(turn, "t_person_end_ms").and_then(Value::as_u64);
    let first_audio = header(turn, "t_first_audio_ms").and_then(Value::as_u64);
    assert!(person_end <= first_audio, "{turn}");
    partials.extend(of_route(&seen, "partial").into_iter().cloned());
    assert!(!partials.is_empty());
    for p in &partials {
        assert_eq!(header(p, "turn_id"), header(turn, "turn_id"));
    }
}

/// `len` bytes of PCM16 at a steady 8 000 (about -12 dBFS): speech to the
/// level the gate and the park pump hear by (`duck::first_audible`).
fn speech(len: usize) -> Vec<u8> {
    (0..len / 2).flat_map(|_| 8_000i16.to_le_bytes()).collect()
}

/// The caller's sentence, two fragments ending at 900 on the call's timeline.
async fn caller_asks(live: &mut Live, session: &FakeSession) {
    for (delta, start_ms, end_ms) in [("Is it ", 0, 500), ("raining?", 500, 900)] {
        session
            .say(DuplexEvent::Transcript {
                speaker: Speaker::User,
                delta: delta.to_string(),
                start_ms,
                end_ms,
            })
            .await;
        let _ = live.emission("partial").await;
    }
}

/// Fix-round C-1 (BAT R1e): a live model streams its silence too, in 100 ms
/// chunks (`park_pump`, GH #798), so the first chunk sent after a fragment is
/// silence about one chunk later and the mark read the provider's cadence.
/// The mark waits for the first AUDIBLE chunk: silence at +700 ms, speech at
/// +1 000 ms, so 900 + 1 000.
#[tokio::test(flavor = "current_thread")]
async fn duplex_first_audio_mark_skips_the_silence_before_the_answer() {
    let (mut live, mut client, session) = open("delta-6", live_block(json!({}))).await;
    caller_asks(&mut live, &session).await;
    tokio::time::advance(Duration::from_millis(700)).await;
    session
        .audio_out
        .send(vec![0u8; 9_600])
        .await
        .expect("the connection takes model audio");
    let _ = client.next_audio().await;
    tokio::time::advance(Duration::from_millis(300)).await;
    session
        .audio_out
        .send(speech(9_600))
        .await
        .expect("the connection takes model audio");
    let _ = client.next_audio().await;
    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::Assistant,
            delta: "Not right now.".to_string(),
            start_ms: 1_200,
            end_ms: 1_800,
        })
        .await;

    let seen = until_turn(&mut live).await;
    let turn = seen.last().expect("the turn closed");
    assert_eq!(header(turn, "t_person_end_ms"), Some(&json!(900)), "{turn}");
    assert_eq!(
        header(turn, "t_first_audio_ms"),
        Some(&json!(1_900)),
        "{turn}"
    );
}

/// Fix-round C-1: a turn whose answer is only silence has no first audio --
/// the mark is missing, not the time of a silent chunk.
#[tokio::test(flavor = "current_thread")]
async fn duplex_first_audio_mark_is_missing_when_only_silence_went_out() {
    let (mut live, mut client, session) = open("delta-7", live_block(json!({}))).await;
    caller_asks(&mut live, &session).await;
    tokio::time::advance(Duration::from_millis(700)).await;
    session
        .audio_out
        .send(vec![0u8; 9_600])
        .await
        .expect("the connection takes model audio");
    let _ = client.next_audio().await;
    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::Assistant,
            delta: "Hm.".to_string(),
            start_ms: 1_200,
            end_ms: 1_400,
        })
        .await;
    session
        .audio_out
        .send(vec![0u8; 9_600])
        .await
        .expect("the connection takes model audio");
    let _ = client.next_audio().await;

    let seen = until_turn(&mut live).await;
    let turn = seen.last().expect("the turn closed");
    assert_eq!(header(turn, "t_person_end_ms"), Some(&json!(900)), "{turn}");
    assert_eq!(header(turn, "t_first_audio_ms"), None, "{turn}");
}

/// Without the new knobs every delta is a `partial`, in the header it always
/// had; `emit_spoken: false` stops the model's side and nothing else.
#[tokio::test(flavor = "current_thread")]
async fn duplex_partial_unthrottled_by_default() {
    for (tag, duplex, spoken_expected) in [
        (
            "delta-3",
            json!({"provider": "echo", "sample_rate": 16_000}),
            true,
        ),
        ("delta-4", live_block(json!({})), true),
        ("delta-5", live_block(json!({"emit_spoken": false})), false),
    ] {
        let (mut live, _client, session) = open(tag, duplex).await;
        for i in 0..5u64 {
            session
                .say(DuplexEvent::Transcript {
                    speaker: Speaker::User,
                    delta: format!("w{i} "),
                    start_ms: i * 10,
                    end_ms: i * 10 + 9,
                })
                .await;
        }
        session
            .say(DuplexEvent::Transcript {
                speaker: Speaker::Assistant,
                delta: "Yes.".to_string(),
                start_ms: 100,
                end_ms: 300,
            })
            .await;
        let seen = until_turn(&mut live).await;
        let partials = of_route(&seen, "partial");
        assert_eq!(partials.len(), 5, "{tag}: one partial per delta");
        assert_eq!(text(partials[4]), "w0 w1 w2 w3 w4 ", "{tag}");
        let keys: BTreeSet<&str> = partials[0]["header"]
            .as_object()
            .expect("a header")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            BTreeSet::from([
                "route",
                "session_id",
                "call_id",
                "platform",
                "mode",
                "eager",
                "turn_id",
                "engine"
            ]),
            "{tag}: the partial header is the one it always was"
        );
        assert_eq!(
            !of_route(&seen, "spoken").is_empty(),
            spoken_expected,
            "{tag}: `spoken` follows emit_spoken, falling back to emit_partials"
        );
        // The next boot pauses the clock again.
        drop(live);
        tokio::time::resume();
    }
}
