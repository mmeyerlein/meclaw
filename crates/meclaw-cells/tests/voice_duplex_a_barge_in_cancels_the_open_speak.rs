//! Welle Live, L2b — talking over the model cancels what it was saying.
//!
//! The barge-in path of today, with a new trigger. The telephony hive counts a
//! `speak_end` down for every `in_speak` it counted up and runs `uuid_break`
//! when the reason is `cancelled` — that is the wire that stops a call's audio
//! (contract § 2). Measured in the wave (S0): the model does NOT stop on its
//! own when it is interrupted; it stalls about 2 s, starts again, and audible
//! output goes on for another 4,4–11,0 s after the interruption began. So the
//! cancel is necessary rather than cosmetic.
//!
//! Two triggers, two locks. The client's own `cancel` frame is this strand's
//! and is measured here. The one that comes out of the model's timeline — a
//! user run inside an agent block past `backchannel_max_ms` (OR-L18) — is
//! produced by `live_turns::step`, which strand L3 fills.

#[path = "support/duplex_cell.rs"]
mod duplex_cell;

use duplex_cell::{boot_fake, duplex_params, header, speak_msg};
use meclaw_cells::voice::contract::{DuplexControl, DuplexEvent, Speaker};
use meclaw_core::serde_json::json;
use std::time::Duration;

/// The failure marker of this file.
const MARKER: Duration = Duration::from_secs(30);

/// A `cancel` from the client ends the open section as `cancelled`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancel_ends_the_open_section_as_cancelled() {
    let mut live = boot_fake(duplex_params(json!({"default_mode": "auto"}))).await;
    let (mut client, _hello) = live.connect("session=barge-1&mode=auto").await;
    let _session = live.session().await;

    live.send(speak_msg("barge-1", "The appointment is on Thursday."))
        .await;
    let event_id = match live.control().await {
        DuplexControl::Append { event_id, .. } => event_id,
        other => panic!("expected the append, got {other:?}"),
    };
    client
        .next_frame_of_type("speak_start", MARKER)
        .await
        .expect("the section opens");

    client.cancel().await.expect("the caller cuts in");

    let end = client
        .next_frame_of_type("speak_end", MARKER)
        .await
        .expect("the section ends");
    assert_eq!(end["speak_id"], json!(event_id), "got {end}");
    assert_eq!(
        end["reason"],
        json!("cancelled"),
        "`cancelled` is what the telephony hive turns into a `uuid_break`: \
         {end}"
    );

    let lane = live.emission("speak_end").await;
    assert_eq!(header(&lane, "reason"), Some(&json!("cancelled")));
    assert_eq!(header(&lane, "engine"), Some(&json!("duplex")));
}

/// And the model's own timeline says the same thing.
///
/// The run below is the reference shape of OR-L18: the model is talking, the
/// caller talks over it for two seconds, and that is past any backchannel, so
/// `live_turns::step` hands out one `BargeIn` and the open section is cut.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_long_interjection_cancels_the_open_section() {
    let mut live = boot_fake(duplex_params(json!({"default_mode": "auto"}))).await;
    let (mut client, _hello) = live.connect("session=barge-2&mode=auto").await;
    let session = live.session().await;

    live.send(speak_msg("barge-2", "The appointment is on Thursday."))
        .await;
    let _ = live.control().await;
    client
        .next_frame_of_type("speak_start", MARKER)
        .await
        .expect("the section opens");

    // The model is in the middle of an agent block …
    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::Assistant,
            delta: "The appointment is".to_string(),
            start_ms: 0,
            end_ms: 800,
        })
        .await;
    // … and the caller talks over it for two seconds.
    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::User,
            delta: "No, hang on a second, that's not right".to_string(),
            start_ms: 600,
            end_ms: 2_600,
        })
        .await;

    let end = client
        .next_frame_of_type("speak_end", MARKER)
        .await
        .expect("the section the caller talked over ends");
    assert_eq!(end["reason"], json!("cancelled"), "got {end}");
}
