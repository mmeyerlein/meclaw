//! GH #1016 — **nothing is said into a call that is over.**
//!
//! Measured on a live colony (2026-09-28): the caller hung up, the switch
//! stopped the audio fork, the duplex session closed — and the answers the
//! member was still writing for that call kept arriving at the media half as
//! `in_speak` and `in_advise`. Each one was refused as `unknown_session` ("no
//! live connection holds this session") on the cell's `error` lane, which
//! carried it up the channel and out of the member, where nothing routes a
//! voice error: it ended as a `hive_no_route` dead letter at the colony root,
//! one per answer, named for a routing gap that is not there.
//!
//! The answer is not lost and not an error: it is late. The media half is the
//! only place that knows the call is over -- an edge condition reads the
//! message, not the line -- so the cell closes the session's door when its
//! connection goes, and whatever still arrives for that session is handed to
//! the colony as a dead letter of its own class, `session_ended`, carrying the
//! message itself. Nothing reaches the provider, nothing travels on as an
//! error, and nothing vanishes (a dead letter is the record, never silence).
//!
//! A session id that connects again is a new call and is spoken to again.

#[path = "support/duplex_cell.rs"]
mod duplex_cell;

use duplex_cell::{Live, MARKER, advise_msg, boot_fake, duplex_params, speak_msg};
use meclaw_cells::voice::contract::DuplexControl;
use meclaw_colony::{ColonyMsg, DeadLetterReason};
use meclaw_core::Message;
use meclaw_core::serde_json::json;
use std::time::{Duration, Instant};

/// What became of one message sent after the caller hung up.
#[derive(Debug)]
enum After {
    /// Handed to the colony as a dead letter, with its class and its lane.
    DeadLetter(&'static str, String),
    /// Emitted on the cell's own `error` lane, with its `error_code`.
    Error(String),
}

/// A duplex call that was answered and is over: the client connected, the
/// model's session opened, and the socket closed -- the switch stopping the
/// fork at `call_ended`.
async fn a_call_that_ended(session: &str) -> Live {
    let mut live = boot_fake(duplex_params(json!({}))).await;
    let (client, hello) = live.connect(&format!("session={session}&mode=auto")).await;
    assert_eq!(hello["duplex"], true, "got {hello}");
    let _session = live.session().await;
    drop(client);
    live
}

/// Send what `make` builds until the cell has noticed the connection is gone,
/// and say what became of the first message it treated as such. The socket
/// closes asynchronously, so a message can overtake the `Disconnected`: that
/// one reaches the model (it is a live call to the cell) and the next is sent.
async fn after_the_end(live: &mut Live, make: impl Fn() -> Message) -> After {
    let deadline = Instant::now() + MARKER;
    loop {
        assert!(
            Instant::now() < deadline,
            "the cell never noticed the call ended"
        );
        live.send(make()).await;
        let window = Instant::now() + Duration::from_millis(400);
        while Instant::now() < window {
            tokio::select! {
                dl = live.dead_letters.recv() => {
                    if let Some(ColonyMsg::DeadLetterMessage { message, reason }) = dl {
                        let lane = message
                            .headers
                            .hop
                            .get("route")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        return After::DeadLetter(reason.as_code(), lane);
                    }
                }
                em = live.emissions.recv() => {
                    if let Some(em) = em {
                        let h = &em.content["header"];
                        if h["route"] == "error" {
                            return After::Error(h["error_code"].as_str().unwrap_or_default().to_string());
                        }
                    }
                }
                c = live.controls.recv() => {
                    if let Some(DuplexControl::Append { .. }) = c {
                        break;
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            }
        }
        // Either it reached the model (it overtook the close) or nothing
        // came yet: send the next one.
    }
}

/// The e28 pattern: an answer for the call arrives after the caller hung up.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_late_answer_is_a_session_ended_dead_letter() {
    let mut live = a_call_that_ended("ended-speak").await;
    let got = after_the_end(&mut live, || {
        speak_msg("ended-speak", "Are you still there?")
    })
    .await;
    match got {
        After::DeadLetter(code, lane) => {
            assert_eq!(
                code,
                DeadLetterReason::SessionEnded.as_code(),
                "a late answer is classified as what it is"
            );
            assert_eq!(code, "session_ended", "the canonical string");
            assert_eq!(
                lane, "in_speak",
                "the dead letter IS the message that came late"
            );
        }
        other => panic!(
            "an answer for a call that is over goes to the colony as a dead letter \
             of its own class -- not to the model, not up the channel as an error \
             (measured 2026-09-28: `unknown_session`, then `hive_no_route`): {other:?}"
        ),
    }
}

/// Advice for a call that is over is the same: no append, a dead letter.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_advice_is_a_session_ended_dead_letter() {
    let mut live = a_call_that_ended("ended-advise").await;
    let got = after_the_end(&mut live, || {
        advise_msg(
            "ended-advise",
            "fact",
            "The appointment is on Thursday.",
            Some("d-1"),
        )
    })
    .await;
    match got {
        After::DeadLetter(code, lane) => {
            assert_eq!(code, "session_ended");
            assert_eq!(lane, "in_advise");
        }
        other => panic!("late advice is a session_ended dead letter: {other:?}"),
    }
}

/// A session that was never connected is still refused by name: the class is
/// for a call that WAS on this cell, not for a typo in `context.call_id`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_never_seen_is_still_unknown() {
    let mut live = a_call_that_ended("ended-one").await;
    let got = after_the_end(&mut live, || speak_msg("never-connected", "Hello?")).await;
    match got {
        After::Error(code) => assert_eq!(code, "unknown_session"),
        other => panic!("a session this cell never held stays `unknown_session`: {other:?}"),
    }
}

/// The same session id connecting again is a new call, spoken to again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_that_connects_again_is_spoken_to() {
    let mut live = a_call_that_ended("again").await;
    let first = after_the_end(&mut live, || speak_msg("again", "late")).await;
    assert!(
        matches!(first, After::DeadLetter("session_ended", _)),
        "first the old call is over: {first:?}"
    );
    let (_client, hello) = live.connect("session=again&mode=auto").await;
    assert_eq!(hello["duplex"], true, "got {hello}");
    let _session = live.session().await;
    live.send(speak_msg("again", "Welcome back.")).await;
    let deadline = Instant::now() + MARKER;
    loop {
        assert!(
            Instant::now() < deadline,
            "the new call never heard its answer"
        );
        tokio::select! {
            c = live.controls.recv() => {
                if let Some(DuplexControl::Append { content, .. }) = c {
                    assert!(content.contains("Welcome back"), "{content}");
                    break;
                }
            }
            dl = live.dead_letters.recv() => {
                if let Some(ColonyMsg::DeadLetterMessage { reason, .. }) = dl {
                    panic!("a reconnected session is live again, not {}", reason.as_code());
                }
            }
        }
    }
}
