//! Welle Live, L2b — a spoken section ends when the model stops talking.
//!
//! A duplex model sends no end-of-speech event at all: it streams audio until
//! it stops. The telephony hive downstream counts one `speak_end` down for
//! every `in_speak` it counted up, so the end has to be produced, and it is
//! produced by the half that has a clock — the connection (OR-L19).
//!
//! The heuristic has two deadlines and this file measures both. The quiet one
//! starts when the model CONFIRMS the append (`appended`) and is pushed out
//! again by every assistant fragment after it; the cap is what a model that
//! never stops talking runs into. Both are held to short numbers here because
//! what is measured is the mechanism, not the shipped 1 500 ms / 8 000 ms.

// harness_fold: #[path = "support/duplex_cell.rs"]
#[allow(unused_imports)]
use crate::duplex_cell; // harness_fold: mod duplex_cell;

use duplex_cell::{boot_fake_timed, duplex_params, header, speak_msg};
use meclaw_cells::voice::contract::{AppendKind, DuplexControl, DuplexEvent, Speaker};
use meclaw_core::serde_json::json;
use std::time::Duration;

/// The quiet deadline of [`quiet_section`].
const QUIET: Duration = Duration::from_millis(300);

/// The quiet deadline: the section ends once the assistant fragments stop.
///
/// Time logic on a paused clock (TV-Z6/TV-Z7): the client sits on the
/// in-process link, the fake model and the lane are channels, so the virtual
/// time from the last fragment to the end is the quiet, exactly -- measured
/// from the first, it would be 400 ms shorter.
#[tokio::test(flavor = "current_thread")]
async fn the_quiet_after_the_last_fragment_ends_the_section() {
    let waited = quiet_section("quiet-1", Duration::ZERO).await;
    assert!(
        waited >= QUIET && waited <= QUIET + TICK,
        "the quiet is measured from the LAST fragment, not from the first: \
         {waited:?} (virtual) against a quiet of {QUIET:?}"
    );
}

/// The timer wheel's resolution: a deadline fires on its millisecond tick.
const TICK: Duration = Duration::from_millis(1);

/// GH #1065: a stall of the test between sending the last fragment and
/// reading its clock must not shorten the quiet it measures.
///
/// A strand gate read 299.25 ms here under full-suite load: the clock was read
/// AFTER `say` returned, and the cell arms its deadline when it reads the
/// event, which can be before the test thread runs again. A 50 ms stall at
/// exactly that point reproduced the red on every run (251.25 ms on the build
/// lane) where the gate needed a preemption by chance. On the paused clock the
/// stall is virtual as well, so the bound is the exact one of the test above.
#[tokio::test(flavor = "current_thread")]
async fn gh1065_a_stall_after_the_last_send_does_not_shorten_the_quiet() {
    let waited = quiet_section("quiet-3", Duration::from_millis(50)).await;
    assert!(
        waited >= QUIET && waited <= QUIET + TICK,
        "the clock of the last fragment must start before its send -- the \
         cell arms the quiet when it reads the event, which a stalled test \
         thread cannot see: {waited:?} (virtual) against a quiet of {QUIET:?}"
    );
}

/// One spoken section ended by the quiet deadline ([`QUIET`]): the model takes
/// the speak up and says three fragments 200 ms apart. Returns how long the
/// end took on the paused clock, measured from the last fragment; `stall`
/// holds the test right after the last send, the way a loaded scheduler does.
///
/// The client connects through the cell's in-process link
/// (`VoiceLinkOpener::open`, `ClientLink::chan`): the same admission and the
/// same `run_connection` as the socket door, with tokio channels where the
/// socket was, so `finish_speak` writes `speak_end` into a channel before it
/// emits on the lane (TV-Z7). The clock is paused right after the boot: the
/// boot binds the shared surface listener (a socket this test never touches)
/// and waits for the mount with a sleep loop; everything after it -- link,
/// session, speak, fragments, quiet, end -- is timers and channels.
async fn quiet_section(tag: &str, stall: Duration) -> Duration {
    let mut live = boot_fake_timed(
        duplex_params(json!({"default_mode": "auto"})),
        QUIET.as_millis() as u64,
        30_000,
    )
    .await;
    tokio::time::pause();
    let (mut client, _hello) = live.link(tag, "auto").await;
    let session = live.session().await;

    live.send(speak_msg(tag, "Tell him the appointment is set."))
        .await;
    let event_id = match live.control().await {
        DuplexControl::Append { event_id, .. } => event_id,
        other => panic!("expected the append, got {other:?}"),
    };
    client.frame_of_type("speak_start").await;

    // The model takes it up, then says three things 200 ms apart — each one
    // inside the 300 ms quiet, so none of them may end the section.
    session
        .say(DuplexEvent::Appended {
            kind: AppendKind::Commentary,
            event_id: event_id.clone(),
            start_ms: 0,
            end_ms: 10,
        })
        .await;
    let mut last_fragment = None;
    for i in 0..3u64 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        // GH #1065: the clock starts BEFORE the send, as the cap's does
        // (GH #992). The cell arms the quiet when it reads the event, at this
        // same virtual instant: nothing moves the paused clock in between.
        last_fragment = Some(tokio::time::Instant::now());
        session
            .say(DuplexEvent::Transcript {
                speaker: Speaker::Assistant,
                delta: format!("Part {i} "),
                start_ms: i * 200,
                end_ms: i * 200 + 150,
            })
            .await;
        if i == 2 {
            tokio::time::sleep(stall).await;
        }
    }

    // The end, on the lane and on the link: both channels, so the paused
    // clock stands while it travels.
    let lane = live.emission("speak_end").await;
    let waited = last_fragment.expect("three fragments were said").elapsed();
    assert_eq!(header(&lane, "speak_id"), Some(&json!(event_id)));
    assert_eq!(header(&lane, "reason"), Some(&json!("done")));
    assert_eq!(header(&lane, "engine"), Some(&json!("duplex")));

    let end = client.frame_of_type("speak_end").await;
    assert_eq!(end["speak_id"], json!(event_id), "got {end}");
    assert_eq!(end["reason"], json!("done"), "got {end}");
    waited
}

/// The cap: a model that says nothing at all still ends the section.
///
/// The `appended` answer never comes here, so the quiet clock never starts —
/// and that is exactly the case the ceiling exists for: without it a hive that
/// holds a hang-up until the sentence is over would wait for the rest of the
/// call.
///
/// Time logic on a paused clock like [`quiet_section`] (TV-Z6/TV-Z7, recheck
/// fix1 B-H1): the client sits on the in-process link, so from the speak to
/// the end only timers and channels run, and the virtual time from the send
/// to `speak_end` is the cap, exactly. GH #992: the clock starts BEFORE the
/// speak is sent -- the cap is armed when the cell takes the speak, never
/// earlier.
#[tokio::test(flavor = "current_thread")]
async fn the_cap_ends_a_section_the_model_never_answers() {
    let waited = cap_section("quiet-2").await;
    assert!(
        waited >= CAP && waited <= CAP + TICK,
        "the cap ends the section on its ceiling, never before: {waited:?} \
         (virtual) against a cap of {CAP:?}"
    );
}

/// One spoken section the model never answers, ended by the cap ([`CAP`]).
/// Returns how long the end took on the paused clock, measured from the
/// speak's send.
async fn cap_section(tag: &str) -> Duration {
    let mut live = boot_fake_timed(
        duplex_params(json!({"default_mode": "auto"})),
        30_000,
        CAP.as_millis() as u64,
    )
    .await;
    tokio::time::pause();
    let (mut client, _hello) = live.link(tag, "auto").await;
    let _session = live.session().await;

    let opened = tokio::time::Instant::now();
    live.send(speak_msg(tag, "Tell him anything.")).await;
    let _ = live.control().await;
    client.frame_of_type("speak_start").await;

    let end = client.frame_of_type("speak_end").await;
    let waited = opened.elapsed();
    assert_eq!(end["reason"], json!("done"), "got {end}");
    waited
}

/// The ceiling of [`the_cap_ends_a_section_the_model_never_answers`].
const CAP: Duration = Duration::from_millis(300);
