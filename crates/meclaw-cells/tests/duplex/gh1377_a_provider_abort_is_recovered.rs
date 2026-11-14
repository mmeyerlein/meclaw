//! GH #1377 -- a provider that ends the session in charge mid-call.
//!
//! **The promise.** A live model may end its session on its own while the
//! caller is still on the line: measured on the phone, `reason=content` 4 to
//! 11 s after a sentence its moderation reads as a crisis. With
//! `recover_max` 0 (the default) the call ends as it always did: the close,
//! no error, no second session. With `recover_max` set the connection opens a
//! fresh session of the same call in place of the dead one -- a renewal
//! (GH #896) that asks for its seed with `reason: recover` and takes the
//! call at once, because nobody is left to hand over from -- at most
//! `recover_max` times per call; the next abort is `duplex_failed` on the
//! error lane, exactly once, which the telephony edge answers with its
//! failure announcement and a hang-up. What the cell ends itself -- a
//! renewal's retiring session, the call the client hangs up -- is no abort
//! and spends nothing.
//!
//! **How it is measured.** At the seam, with the renewing fake of
//! `tests/support/duplex_cell.rs`: the test plays the provider's close
//! (`Closed` with its reason, then the provider task ends) and reads the
//! handler's emissions and the client's frames. Event-driven: the call's one
//! `final` usage, emitted right before the call is let go, is the marker
//! after which nothing more of the call can come, so absence is read up to
//! it and never over a window. No claim here is about time.

// harness_fold: #[path = "support/duplex_cell.rs"]
#[allow(unused_imports)]
use crate::duplex_cell; // harness_fold: mod duplex_cell;

use duplex_cell::{FakeSession, Live, boot_recovering, duplex_params, emission_route, header};
use meclaw_cells::voice::contract::{DuplexControl, DuplexEvent};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use std::sync::atomic::Ordering;

/// A seed wait the fake never runs into: it does not wait for seeds.
const SEED_WAIT_MS: u64 = 400;

/// The params of every call here: the usage lane is the end marker.
fn params() -> Value {
    duplex_params(json!({"default_mode": "auto", "emit_usage": true}))
}

/// The provider ends the session on its own, the way the real adapter does:
/// its close with the provider's reason and meter, then the session is over.
async fn provider_ends(mut session: FakeSession, reason: &str, usage_seconds: f64) {
    session
        .say(DuplexEvent::Closed {
            reason: reason.to_string(),
            usage_seconds,
        })
        .await;
    session.stop().await;
    drop(session);
}

/// The colony's answer to the `seed_ask` of session `renewal_n`.
fn seed_msg(call: &str, renewal_n: u32, text: &str) -> Message {
    let mut hop = Map::new();
    hop.insert("route".into(), json!("in_seed"));
    hop.insert("renewal_n".into(), json!(renewal_n));
    let mut context = Map::new();
    context.insert("call_id".into(), json!(call));
    MessageBuilder::new(Path::new("/voice"))
        .hop(hop)
        .context(context)
        .body(Body::Inline(
            json!({"messages": [{"role": "user", "text": text}]}),
        ))
        .build()
}

/// An `in_advise` `renew` the colony asks for (D48).
fn renew_msg(call: &str) -> Message {
    let mut hop = Map::new();
    hop.insert("route".into(), json!("in_advise"));
    hop.insert("section".into(), json!("renew"));
    let mut context = Map::new();
    context.insert("call_id".into(), json!(call));
    MessageBuilder::new(Path::new("/voice"))
        .hop(hop)
        .context(context)
        .body(Body::Inline(
            json!({"messages": [{"origin": "assistant", "type": "text", "text": ""}]}),
        ))
        .build()
}

/// Every emission up to and including the call's `final` usage.
async fn until_final(live: &mut Live) -> Vec<Value> {
    let mut seen = Vec::new();
    loop {
        let e = tokio::time::timeout(duplex_cell::marker(), live.emissions.recv())
            .await
            .unwrap_or_else(|_| {
                let routes: Vec<_> = seen
                    .iter()
                    .map(|e: &Value| emission_route(e).unwrap_or_default().to_string())
                    .collect();
                panic!("HANG: no final usage; saw {routes:?}")
            })
            .expect("the handler is running")
            .content;
        let last = emission_route(&e) == Some("usage") && header(&e, "final") == Some(&json!(true));
        seen.push(e);
        if last {
            return seen;
        }
    }
}

/// The `error_code`s among `emissions`.
fn error_codes(emissions: &[Value]) -> Vec<String> {
    emissions
        .iter()
        .filter(|e| emission_route(e) == Some("error"))
        .filter_map(|e| header(e, "error_code").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// The `reason`s of every `seed_ask` among `emissions`.
fn seed_reasons(emissions: &[Value]) -> Vec<String> {
    emissions
        .iter()
        .filter(|e| emission_route(e) == Some("seed_ask"))
        .filter_map(|e| header(e, "reason").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// The next `Seed` any session was told, and which session it reached.
async fn next_seed(live: &mut Live) -> usize {
    loop {
        let (at, control) =
            tokio::time::timeout(duplex_cell::marker(), live.tagged_controls.recv())
                .await
                .expect("HANG: no seed reached a session")
                .expect("the fake is running");
        if let DuplexControl::Seed { .. } = control {
            return at;
        }
    }
}

/// The close code the client reads.
async fn close_code(client: &mut meclaw_testing::voice_client::VoiceClient) -> u16 {
    let frames = client
        .collect_until(|f| f.as_close().is_some(), duplex_cell::marker())
        .await;
    frames
        .iter()
        .find_map(|(_, f)| f.as_close())
        .unwrap_or_else(|| panic!("the client is told the call is over: {frames:?}"))
}

/// (a) The shipped default: the abort ends the call as it always did -- an
/// ordinary close, no word on the error lane, and no second session.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recover_max_zero_ends_the_call_as_before() {
    let mut live = boot_recovering(params(), 0, 0, SEED_WAIT_MS).await;
    let (mut client, _hello) = live.connect("session=rec-0&mode=auto").await;
    let first = live.session().await;

    provider_ends(first, "content", 3.0).await;
    assert_eq!(close_code(&mut client).await, 1000, "an ordinary end");
    let seen = until_final(&mut live).await;
    assert_eq!(error_codes(&seen), Vec::<String>::new(), "no error at all");
    assert_eq!(seed_reasons(&seen), vec!["start".to_string()]);
    assert_eq!(live.starts.load(Ordering::SeqCst), 1, "one session, ever");
}

/// (b) One abort with one attempt: exactly one new session, opened as a
/// renewal, which asks for its seed with `reason: recover`, gets it, starts
/// as the `renewed` lane and carries the call on -- the caller's audio reaches
/// it. The ledger counts both sessions, and a client that hangs up later ends
/// the call without an error.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_abort_is_recovered_with_a_fresh_seed() {
    let mut live = boot_recovering(params(), 1, 0, SEED_WAIT_MS).await;
    let (mut client, _hello) = live.connect("session=rec-1&mode=auto").await;
    let ask = live.emission("seed_ask").await;
    assert_eq!(header(&ask, "reason"), Some(&json!("start")));
    let first = live.session().await;

    provider_ends(first, "content", 3.0).await;
    let ask = live.emission("seed_ask").await;
    assert_eq!(header(&ask, "reason"), Some(&json!("recover")), "{ask}");
    assert_eq!(header(&ask, "renewal_n"), Some(&json!(1)), "{ask}");
    assert_eq!(header(&ask, "call_id"), Some(&json!("rec-1")), "{ask}");
    let mut second = live.session().await;
    assert_eq!(
        (second.index, second.renewed),
        (2, true),
        "the recovery opens as a renewal, which a greeting provider does not greet"
    );
    live.send(seed_msg("rec-1", 1, "what was said")).await;
    assert_eq!(next_seed(&mut live).await, 2, "the fresh seed reaches it");
    let renewed = live.emission("renewed").await;
    assert_eq!(header(&renewed, "renewal_n"), Some(&json!(1)), "{renewed}");

    // The call goes on: the caller's audio reaches the new session.
    let frame = vec![7u8; duplex_cell::FRAME_BYTES];
    client.send_audio(&frame).await.expect("the socket is open");
    loop {
        let (at, bytes) = tokio::time::timeout(duplex_cell::marker(), live.tagged_audio.recv())
            .await
            .expect("HANG: the caller's audio reached no session")
            .expect("the fake is running");
        if bytes == frame {
            assert_eq!(at, 2, "the recovered session is in charge");
            break;
        }
    }

    // The client hangs up: the end of the call, not an abort.
    drop(client);
    second
        .say(DuplexEvent::Closed {
            reason: "close_requested".to_string(),
            usage_seconds: 5.0,
        })
        .await;
    second.finish();
    let seen = until_final(&mut live).await;
    assert_eq!(error_codes(&seen), Vec::<String>::new(), "{seen:?}");
    let last = seen.last().expect("the final");
    assert_eq!(
        header(last, "seconds"),
        Some(&json!(8.0)),
        "the dead session's close counts with the new one's: {last}"
    );
    assert_eq!(header(last, "complete"), Some(&json!(true)), "{last}");
    assert_eq!(live.starts.load(Ordering::SeqCst), 2, "one recovery");
}

/// (c) Two aborts with one attempt: the second is `duplex_failed`, exactly
/// once, and the client reads `1011` -- the edge plays its announcement and
/// hangs up instead of leaving the caller on a dead line.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_abort_past_recover_max_is_duplex_failed_once() {
    let mut live = boot_recovering(params(), 1, 0, SEED_WAIT_MS).await;
    let (mut client, _hello) = live.connect("session=rec-2&mode=auto").await;
    let first = live.session().await;
    provider_ends(first, "content", 2.0).await;
    let second = live.session().await;
    let renewed = live.emission("renewed").await;
    assert_eq!(header(&renewed, "renewal_n"), Some(&json!(1)), "{renewed}");

    provider_ends(second, "content", 4.0).await;
    assert_eq!(close_code(&mut client).await, 1011, "a failed call");
    let seen = until_final(&mut live).await;
    assert_eq!(
        error_codes(&seen),
        vec!["duplex_failed".to_string()],
        "one word on the error lane: {seen:?}"
    );
    let failed = seen
        .iter()
        .find(|e| emission_route(e) == Some("error"))
        .expect("the error");
    assert_eq!(header(failed, "call_id"), Some(&json!("rec-2")), "{failed}");
    let last = seen.last().expect("the final");
    assert_eq!(header(last, "seconds"), Some(&json!(6.0)), "{last}");
    assert_eq!(live.starts.load(Ordering::SeqCst), 2, "no third session");
}

/// (d) What the cell ends itself spends nothing: a renewal the colony asked
/// for retires the session it replaces, and that session's close is no
/// abort -- no `recover` seed, no extra session. The one attempt is still
/// there for the provider abort that follows.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_own_renewal_spends_no_attempt() {
    let mut live = boot_recovering(params(), 1, 0, SEED_WAIT_MS).await;
    let (_client, _hello) = live.connect("session=rec-3&mode=auto").await;
    let mut first = live.session().await;

    live.send(renew_msg("rec-3")).await;
    let ask = live.emission("seed_ask").await;
    assert_eq!(header(&ask, "reason"), Some(&json!("start")));
    let ask = live.emission("seed_ask").await;
    assert_eq!(header(&ask, "reason"), Some(&json!("renew")), "{ask}");
    let second = live.session().await;
    let renewed = live.emission("renewed").await;
    assert_eq!(header(&renewed, "renewal_n"), Some(&json!(1)));
    live.send(duplex_cell::handover_msg("rec-3", "Handover.", 1))
        .await;
    let ended = tokio::time::timeout(duplex_cell::marker(), live.ended.recv())
        .await
        .expect("HANG: the old session's audio ends after the takeover")
        .expect("the fake is running");
    assert_eq!(ended, 1, "the first session retires");
    first
        .say(DuplexEvent::Closed {
            reason: "close_requested".to_string(),
            usage_seconds: 1.0,
        })
        .await;
    first.finish();
    drop(first);

    // The provider now ends the session in charge: the attempt is unspent.
    provider_ends(second, "content", 2.0).await;
    let ask = live.emission("seed_ask").await;
    assert_eq!(header(&ask, "reason"), Some(&json!("recover")), "{ask}");
    assert_eq!(
        header(&ask, "renewal_n"),
        Some(&json!(2)),
        "the recovery is the call's next session: {ask}"
    );
    let _third = live.session().await;
    assert_eq!(live.starts.load(Ordering::SeqCst), 3);
}

/// Every emission up to and including the running `usage` that says
/// `seconds` -- the marker that the connection has read what came before it.
async fn until_usage(live: &mut Live, seconds: f64) -> Vec<Value> {
    let mut seen = Vec::new();
    loop {
        let e = tokio::time::timeout(duplex_cell::marker(), live.emissions.recv())
            .await
            .unwrap_or_else(|_| panic!("HANG: no usage of {seconds} s; saw {seen:?}"))
            .expect("the handler is running")
            .content;
        let last =
            emission_route(&e) == Some("usage") && header(&e, "seconds") == Some(&json!(seconds));
        seen.push(e);
        if last {
            return seen;
        }
    }
}

/// (e) The abort comes while a renewal is opening (OR-live-2.fx-recover.2):
/// that renewal never takes over -- it retires, its way in is shut, and its
/// close still counts -- and the recovery takes the call's next number past
/// it. What the retiring renewal still says never reaches the caller's turn
/// (review M-2): it never spoke on the line. A `renew` the colony asked for
/// while the renewal was opening is not opened on top of the recovery
/// (review M-1): the recovery's seed already carries the call as it stands.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_abort_while_a_renewal_opens_retires_it_and_counts_it() {
    let mut live = boot_recovering(params(), 1, 0, SEED_WAIT_MS).await;
    let (client, _hello) = live.connect("session=rec-4&mode=auto").await;
    let first = live.session().await;

    live.send(renew_msg("rec-4")).await;
    let ask = live.emission("seed_ask").await;
    assert_eq!(header(&ask, "reason"), Some(&json!("start")));
    let ask = live.emission("seed_ask").await;
    assert_eq!(header(&ask, "reason"), Some(&json!("renew")), "{ask}");
    let mut second = live.session().await;
    let renewed = live.emission("renewed").await;
    assert_eq!(header(&renewed, "renewal_n"), Some(&json!(1)), "{renewed}");
    // Asked again while the renewal is open and started: it would open the
    // next one right after a takeover.
    live.send(renew_msg("rec-4")).await;
    // A seed behind it travels the same way to the connection: once it
    // reached the renewal, the connection has read the ask.
    live.send(seed_msg("rec-4", 1, "what was said")).await;
    assert_eq!(
        next_seed(&mut live).await,
        2,
        "the seed reaches the renewal"
    );

    provider_ends(first, "content", 2.0).await;
    let ask = live.emission("seed_ask").await;
    assert_eq!(header(&ask, "reason"), Some(&json!("recover")), "{ask}");
    assert_eq!(
        header(&ask, "renewal_n"),
        Some(&json!(2)),
        "the recovery is numbered past the renewal that was opening: {ask}"
    );
    let mut third = live.session().await;
    assert_eq!((third.index, third.renewed), (3, true));
    loop {
        let ended = tokio::time::timeout(duplex_cell::marker(), live.ended.recv())
            .await
            .expect("HANG: the opening renewal's way in is not shut")
            .expect("the fake is running");
        if ended == 2 {
            break;
        }
    }

    // The retiring renewal's last words and its close.
    second
        .say(DuplexEvent::Transcript {
            speaker: meclaw_cells::voice::contract::Speaker::User,
            delta: "Ghost words.".to_string(),
            start_ms: 100,
            end_ms: 400,
        })
        .await;
    second
        .say(DuplexEvent::Closed {
            reason: "close_requested".to_string(),
            usage_seconds: 1.0,
        })
        .await;
    second.finish();
    let mut seen = until_usage(&mut live, 3.0).await;

    drop(client);
    third
        .say(DuplexEvent::Closed {
            reason: "close_requested".to_string(),
            usage_seconds: 4.0,
        })
        .await;
    third.finish();
    seen.extend(until_final(&mut live).await);
    assert_eq!(error_codes(&seen), Vec::<String>::new(), "{seen:?}");
    assert!(
        !seen.iter().any(|e| e.to_string().contains("Ghost words.")),
        "the retiring renewal never spoke on the line: {seen:?}"
    );
    assert_eq!(
        seed_reasons(&seen),
        Vec::<String>::new(),
        "no renewal opens on top of the recovery: {seen:?}"
    );
    let last = seen.last().expect("the final");
    assert_eq!(
        header(last, "seconds"),
        Some(&json!(7.0)),
        "the dead session, the retired renewal and the recovery all count: {last}"
    );
    assert_eq!(header(last, "complete"), Some(&json!(true)), "{last}");
    assert_eq!(live.starts.load(Ordering::SeqCst), 3, "{seen:?}");
}

/// (f) The client hangs up while an attempt is still there: the provider's
/// close that follows is the end of the call, not an abort -- no `recover`
/// seed, no second session, no error.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hang_up_with_an_attempt_left_is_no_abort() {
    let mut live = boot_recovering(params(), 1, 0, SEED_WAIT_MS).await;
    let (client, _hello) = live.connect("session=rec-5&mode=auto").await;
    let ask = live.emission("seed_ask").await;
    assert_eq!(header(&ask, "reason"), Some(&json!("start")));
    let mut first = live.session().await;

    drop(client);
    // The connection has left the call once it shuts the session's way in:
    // what the provider says after that is its answer to the end.
    let ended = tokio::time::timeout(duplex_cell::marker(), live.ended.recv())
        .await
        .expect("HANG: the session's way in is not shut after the hang-up")
        .expect("the fake is running");
    assert_eq!(ended, 1);
    first
        .say(DuplexEvent::Closed {
            reason: "content".to_string(),
            usage_seconds: 2.0,
        })
        .await;
    first.finish();
    let seen = until_final(&mut live).await;
    assert_eq!(error_codes(&seen), Vec::<String>::new(), "{seen:?}");
    assert_eq!(seed_reasons(&seen), Vec::<String>::new(), "{seen:?}");
    assert_eq!(live.starts.load(Ordering::SeqCst), 1, "no second session");
}

/// The next append any session was told, and which session it reached.
async fn next_append(live: &mut Live) -> (usize, Option<String>, String) {
    loop {
        let (at, control) =
            tokio::time::timeout(duplex_cell::marker(), live.tagged_controls.recv())
                .await
                .expect("HANG: no append reached a session")
                .expect("the fake is running");
        if let DuplexControl::Append {
            delegation_id,
            content,
            ..
        } = control
        {
            return (at, delegation_id, content);
        }
    }
}

/// POC gpt-live-1 round 3 (A79, A84): a recovery is a replacement session.
/// An answer to a delegation the DEAD session opened is routed to the new
/// one without its handle -- the provider refuses an id it never issued --
/// while a delegation the recovered session opened keeps its handle.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_answer_for_the_dead_sessions_delegation_reaches_the_recovery_without_its_handle() {
    let mut live = boot_recovering(params(), 1, 0, SEED_WAIT_MS).await;
    let (_client, _hello) = live.connect("session=rec-d&mode=auto").await;
    let first = live.session().await;
    first
        .say(DuplexEvent::DelegationCreated {
            delegation_id: "dlg-old".to_string(),
            offset_ms: 100,
        })
        .await;
    let delegation = live.emission("delegation").await;
    assert_eq!(
        header(&delegation, "delegation_id"),
        Some(&json!("dlg-old"))
    );

    provider_ends(first, "content", 3.0).await;
    let ask = live.emission("seed_ask").await;
    assert_eq!(header(&ask, "reason"), Some(&json!("recover")), "{ask}");
    let second = live.session().await;
    live.send(seed_msg("rec-d", 1, "what was said")).await;
    assert_eq!(next_seed(&mut live).await, 2);
    let _renewed = live.emission("renewed").await;

    live.send(duplex_cell::advise_msg(
        "rec-d",
        "fact",
        "It is sunny.",
        Some("dlg-old"),
    ))
    .await;
    assert_eq!(
        next_append(&mut live).await,
        (2, None, "It is sunny.".to_string()),
        "the old handle is gone with its session; the answer is a plain fact"
    );

    second
        .say(DuplexEvent::DelegationCreated {
            delegation_id: "dlg-new".to_string(),
            offset_ms: 50,
        })
        .await;
    let _ = live.emission("delegation").await;
    live.send(duplex_cell::advise_msg(
        "rec-d",
        "fact",
        "It is raining.",
        Some("dlg-new"),
    ))
    .await;
    assert_eq!(
        next_append(&mut live).await,
        (2, Some("dlg-new".to_string()), "It is raining.".to_string())
    );
}
