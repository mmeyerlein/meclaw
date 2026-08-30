//! GH #468 — two `getUpdates` consumers on one bot token, and the one that
//! loses says so.
//!
//! Telegram permits exactly ONE long-poll consumer per token and answers every
//! other one with `409 Conflict`. The connector classified that as an ordinary
//! `Transient`: exponential backoff, one `tracing::debug!` line, no further
//! trace. Two pollers on one token therefore stole each other's updates in
//! silence, and the only symptom anybody saw was a bot answering every other
//! message — the exact failure a switchover produces when the old poller is
//! still running.
//!
//! # What is fixed, and what deliberately is not
//!
//! The RECOVERY is unchanged, and that is a decision rather than an oversight: a
//! conflict backs off like a transient, because the other consumer may stop —
//! and a switchover is precisely the case where it does. Falling into the
//! 5-minute `Permanent` sleep would turn a 200 ms handover window into a
//! 5-minute outage.
//!
//! What changes is that the condition has a NAME. `TelegramError::Conflict` is
//! its own variant, and `run_io` logs it at `warn` with
//! `error_code = "conflict_other_poller"` instead of swallowing it at DEBUG.
//!
//! It is deliberately not an emission. The poll lane answers no message, so a
//! receipt would have to be a source emission carrying `hop.error_code` — a
//! fifth failure code every level holding a connector would owe a drain for,
//! repeated on every backoff tick, for a condition only an operator can fix.
//!
//! # Why the classification is what is pinned
//!
//! The workspace has no `tracing-subscriber` in its dev-dependencies and adding
//! one is a `Cargo.toml` change (`AGENTS.md`, hard rule 6), so a test cannot
//! read the log line back. What it CAN read is the thing the log line is derived
//! from — and a `Conflict` that never reaches `run_io`'s arm cannot produce the
//! warning either. So the pin is the variant, on both calls that can meet a 409,
//! plus the loop behaviour the arm is responsible for: the refused poller keeps
//! polling instead of dying or going quiet for five minutes.

use meclaw_cells::proxy::io::{ProxyEvent, ProxyReconfig, RunIoConfig, run_io};
use meclaw_cells::proxy::telegram::{TelegramClient, TelegramError};
use meclaw_testing::mock_http::{
    CapturedRequest, MockResponse, RequestValidator, start_mock_server_capturing,
    start_mock_server_capturing_with_validator,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;

/// A `409 Conflict` the way Telegram sends it. `MockResponse` ships no
/// constructor for it — 409 is not a generic HTTP-mock concern, it is this
/// template's one interesting status — so the test builds it here rather than
/// widening shared infrastructure for a single caller.
fn conflict() -> MockResponse {
    MockResponse {
        status: 409,
        body: br#"{"ok":false,"error_code":409,"description":"Conflict: terminated by other getUpdates request"}"#
            .to_vec(),
        content_type: "application/json".into(),
        delay: None,
    }
}

/// One update, so a winner can be told from a loser.
const ONE_UPDATE: &[u8] = br#"{"ok":true,"result":[
    {"update_id":1,"message":{"message_id":1,"chat":{"id":100},"from":{"id":200},"text":"mine"}}
]}"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_409_on_get_updates_is_its_own_classification() {
    let (addr, _join, _cap) = start_mock_server_capturing(vec![conflict()]).await;
    let client = TelegramClient::new(&format!("http://{addr}"), "TOKEN").unwrap();

    let err = client
        .get_updates(0, 1, Duration::from_millis(2000))
        .await
        .expect_err("409 must not parse as a successful poll");

    match err {
        TelegramError::Conflict(reason) => {
            // The reason has to name the cause, not only the number: an
            // operator reading one line must learn that somebody else holds
            // the token, which is the fix.
            assert!(
                reason.contains("409") && reason.contains("token"),
                "the conflict must name the status AND the cause: {reason}"
            );
        }
        other => panic!(
            "a 409 is a conflict, not a nameless transient — got {other:?}. \
             Falling back into `Transient` is the silence GH #468 removed."
        ),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_409_on_send_message_is_its_own_classification() {
    let (addr, _join, _cap) = start_mock_server_capturing(vec![conflict()]).await;
    let client = TelegramClient::new(&format!("http://{addr}"), "TOKEN").unwrap();

    let err = client
        .send_message(100, "hi", Duration::from_millis(2000))
        .await
        .expect_err("409 must not read as a delivered message");

    assert!(
        matches!(err, TelegramError::Conflict(_)),
        "the inbound side classifies a 409 the same way — the topology still \
         sees `send_failed`, but its detail now names the conflict: {err:?}"
    );
}

/// Two pollers, one token, one fake upstream — and the fake behaves the way the
/// real one does: the first consumer to ask gets the update, everybody after it
/// is refused with 409.
///
/// Three measurements, and the second is the one that matters:
///
/// 1. **Exactly one update is delivered**, across both loops together. That is
///    the failure the issue describes — two consumers do not both get the
///    traffic, one of them is simply refused.
/// 2. **The refused loop keeps polling.** The fake sees far more requests than
///    the number of successful polls, which is only true if the conflict is
///    treated as recoverable. A `Permanent` classification would sleep 5
///    minutes and the request count would stop at two.
/// 3. **Neither loop dies.** Both join cleanly when their reconfig channel is
///    dropped.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_pollers_on_one_token_and_the_refused_one_keeps_going() {
    // The first `getUpdates` is served the update; every later one is refused,
    // exactly as Telegram refuses a second consumer.
    let served = Arc::new(AtomicUsize::new(0));
    let served_for_validator = served.clone();
    let validator: RequestValidator = Arc::new(move |req: &CapturedRequest| {
        if !req.path.contains("getUpdates") {
            return None;
        }
        if served_for_validator.fetch_add(1, Ordering::SeqCst) == 0 {
            None // the winner: fall through to the canned 200
        } else {
            Some(conflict())
        }
    });
    let (addr, _join, cap) = start_mock_server_capturing_with_validator(
        vec![MockResponse::ok_json(ONE_UPDATE)],
        Some(validator),
    )
    .await;

    // Two loops, one token — the whole point.
    let base = format!("http://{addr}");
    let mut senders = Vec::new();
    let mut joins = Vec::new();
    let mut receivers = Vec::new();
    for _ in 0..2 {
        let client = TelegramClient::new(&base, "ONE-TOKEN").unwrap();
        let (events_tx, events_rx) = mpsc::channel::<ProxyEvent>(64);
        let (rc_tx, rc_rx) = mpsc::channel::<ProxyReconfig>(8);
        let cfg = RunIoConfig {
            client,
            initial_offset: 0,
            long_poll_request_secs: 0,
            long_poll_timeout_ms: 500,
            liveness: meclaw_colony::IoLivenessMark::disabled(),
        };
        joins.push(tokio::spawn(run_io(cfg, events_tx, rc_rx)));
        senders.push(rc_tx);
        receivers.push(events_rx);
    }

    // Long enough for the transient ladder to fire several times (0 s, 1 s,
    // 2 s, 4 s) and far short of the 5-minute permanent sleep — that gap is the
    // discriminator, so it is wide on purpose.
    tokio::time::sleep(Duration::from_secs(6)).await;

    let mut delivered = 0usize;
    for rx in receivers.iter_mut() {
        while rx.try_recv().is_ok() {
            delivered += 1;
        }
    }
    assert_eq!(
        delivered, 1,
        "one token means one consumer: the update goes to whoever asked first, \
         and the other poller gets nothing (that is the update theft GH #468 \
         names). Delivered {delivered}."
    );

    let requests = cap.lock().await.len();
    assert!(
        requests >= 4,
        "a conflict must recover like a transient — the refused poller has to \
         keep asking, because the other consumer may stop and a switchover is \
         exactly that case. A `Permanent` classification would have slept 5 \
         minutes and left 2 requests here; found {requests}."
    );

    drop(senders);
    for join in joins {
        tokio::time::timeout(Duration::from_secs(30), join)
            .await
            .expect("a conflicted poll loop must still shut down cleanly")
            .expect("the loop must not panic on a 409");
    }
}

/// The drift lock for the sentence this change put on a public template surface
/// (`docs/development-rules.md` § 2d): grep the prose AND assert the mechanism.
///
/// The README of `telegram-connector` now says two things a reader will act on —
/// that a conflict is named rather than swallowed, and that it is a LOG line and
/// not a fifth `hop.error_code`. Both halves are asserted here, because a prose
/// promise nothing reads is the defect class that rule exists for.
#[test]
fn the_readme_says_what_a_conflict_does_and_the_code_agrees() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates/telegram-connector/README.md");
    if !path.is_file() {
        return; // GH #49: a tree without the template is skipped, never judged.
    }
    let readme = std::fs::read_to_string(&path).expect("the connector README is readable");

    assert!(
        readme.contains("conflict_other_poller"),
        "{}: the README must name the code an operator will grep for",
        path.display()
    );
    assert!(
        readme.contains("log line, not an emission"),
        "{}: the README must say that a conflict is a log line rather than a \
         fifth failure code — a reader who wires a drain for one will wait \
         forever",
        path.display()
    );

    // The mechanism half of the lock, on both claims at once: the source of the
    // warning is the `Conflict` variant, and the emitted-error set is untouched.
    let io = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/proxy/io.rs"),
    )
    .expect("the proxy io module is readable");
    assert!(
        io.contains("TelegramError::Conflict") && io.contains("conflict_other_poller"),
        "the poll loop must classify a conflict on its own arm and log it under \
         the code the README promises — without that arm the README's sentence \
         is a wish"
    );
    let emit = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/proxy/cell.rs"),
    )
    .expect("the proxy cell module is readable");
    assert!(
        !emit.contains("conflict_other_poller"),
        "a conflict must NOT reach the emission path: the README promises the \
         four inbound codes are still the whole set, and a fifth one would put \
         an undrained lane into every topology holding a connector"
    );
}
