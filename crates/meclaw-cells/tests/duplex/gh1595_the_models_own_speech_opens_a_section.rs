//! GH #1595 -- the model's own speech opens a spoken section of its own.
//!
//! A duplex model speaks on its own too: it answers the caller's audio and it
//! greets. No `advise` with a `speak_id` stands behind that speech, and before
//! this fix the client got audio and no section around it (browser probe d54s
//! takt 3: `played=1440`, `speak_start=0`, `speak_end=0`). Now the first audio
//! no open section covers opens one: `speak_start` with a fresh `speak_id`
//! before the audio, `speak_end` after the same quiet rule an advised section
//! ends by.
//!
//! Such a section exists only on the client's socket. The telephony hive
//! counts each `speak_end` on the lane down against an `in_speak` it issued,
//! and nobody issued one for this speech: the lane stays silent about it.
//!
//! Time logic on a paused clock (TV-Z6/TV-Z7) like
//! `voice_duplex_a_speak_ends_when_the_model_goes_quiet`: the client sits on
//! the in-process link, the fake model and the lane are channels. Nothing here
//! judges a time: the end comes from quiescence, the order from the fake.

// harness_fold: #[path = "support/duplex_cell.rs"]
#[allow(unused_imports)]
use crate::duplex_cell; // harness_fold: mod duplex_cell;

use duplex_cell::{FRAME_BYTES, boot_fake_timed, duplex_params, header, speak_msg};
use meclaw_cells::voice::contract::{AppendKind, DuplexControl, DuplexEvent};
use meclaw_core::serde_json::json;

/// The quiet the sections end by, in ms.
const QUIET_MS: u64 = 300;

/// One 20 ms frame of the model audibly speaking: a square wave at `level`.
fn loud(level: i16) -> Vec<u8> {
    let mut out = Vec::with_capacity(FRAME_BYTES);
    for i in 0..FRAME_BYTES / 2 {
        let sample: i16 = if i % 2 == 0 { level } else { -level };
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

/// Audio without an advise opens a section on the client, ends it on the
/// quiet, and leaves the lane's `speak_end` count to the sections the handler
/// ordered.
#[tokio::test(flavor = "current_thread")]
async fn gh1595_the_models_own_speech_opens_and_ends_a_section() {
    let tag = "own-1";
    let mut live = boot_fake_timed(
        duplex_params(json!({"default_mode": "auto"})),
        QUIET_MS,
        30_000,
    )
    .await;
    tokio::time::pause();
    let (mut client, _hello) = live.link(tag, "auto").await;
    let session = live.session().await;

    // The model speaks on its own: two frames, no advise before them.
    let first = loud(12_000);
    let second = loud(9_000);
    session
        .audio_out
        .send(first.clone())
        .await
        .expect("model audio");
    session
        .audio_out
        .send(second.clone())
        .await
        .expect("model audio");

    // The section opens BEFORE its audio: `frame_of_type` would skip the
    // audio, and then the first frame read after it could not be `first`.
    let start = client.frame_of_type("speak_start").await;
    let own_id = start["speak_id"]
        .as_str()
        .expect("speak_start carries a speak_id")
        .to_string();
    assert!(!own_id.is_empty(), "a fresh speak_id: {start}");
    assert_eq!(
        client.next_audio().await,
        first,
        "the audio follows its section's start"
    );
    assert_eq!(
        client.next_audio().await,
        second,
        "one section covers the whole speech"
    );

    // The model goes quiet; the section ends on the quiet rule.
    let end = client.frame_of_type("speak_end").await;
    assert_eq!(end["speak_id"], json!(own_id), "got {end}");
    assert_eq!(end["reason"], json!("done"), "got {end}");

    // The lane: the first `speak_end` on it is the one of the section the
    // handler ordered, never the connection's own.
    live.send(speak_msg(tag, "Tell him the appointment is set."))
        .await;
    let event_id = match live.control().await {
        DuplexControl::Append { event_id, .. } => event_id,
        other => panic!("expected the append, got {other:?}"),
    };
    let advised = client.frame_of_type("speak_start").await;
    assert_eq!(advised["speak_id"], json!(event_id), "got {advised}");
    session
        .say(DuplexEvent::Appended {
            kind: AppendKind::Commentary,
            event_id: event_id.clone(),
            start_ms: 0,
            end_ms: 10,
        })
        .await;
    let lane = live.emission("speak_end").await;
    assert_eq!(
        header(&lane, "speak_id"),
        Some(&json!(event_id)),
        "the lane carries no speak_end for a section no in_speak opened: {lane}"
    );
    let advised_end = client.frame_of_type("speak_end").await;
    assert_eq!(
        advised_end["speak_id"],
        json!(event_id),
        "got {advised_end}"
    );
}
