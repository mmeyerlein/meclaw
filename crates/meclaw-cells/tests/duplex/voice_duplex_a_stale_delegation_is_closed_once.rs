//! Welle Live, zell-uhr (#793, R-L9) — a delegation nobody answers is closed
//! by the cell.
//!
//! `session.delegation.created` hands work to this colony and the provider then
//! waits. Measured over four delegations of one 305 s session (S0,
//! `a-pacing.json`): three of them produced ONE holding sentence of 1.0-2.8 s
//! and then 55-58 s of silence, and no provider-side timeout appeared within
//! the longest observation of 59.4 s. The caller is the one who pays for that,
//! and they pay by hearing nothing.
//!
//! The first line against it is the `advise` prompt (talky answers every
//! delegation, even empty-handed). The second is here, and it is a deadline:
//! a delegation open longer than `params.duplex.delegation_grace_ms` is closed
//! with a `Commentary` append on ITS `delegation_id` carrying
//! `params.duplex.delegation_fallback`. That is the same shape a `fact`
//! travels in — measured taken up and spoken, S0 b4 and proof B-2 — so the
//! caller hears the model say, in its own words, that it cannot look this up
//! right now.
//!
//! Exactly once. A deadline that fired every tick would append the same
//! sentence for the rest of the call.

// harness_fold: #[path = "support/duplex_cell.rs"]
#[allow(unused_imports)]
use crate::duplex_cell; // harness_fold: mod duplex_cell;

use duplex_cell::{advise_msg, boot_fake, duplex_params};
use meclaw_cells::voice::contract::{AppendKind, DuplexControl, DuplexEvent};
use meclaw_core::serde_json::json;
use std::time::Duration;

/// The fallback sentence this fixture configures, so the assertion names the
/// param rather than a string that happens to match the shipped default.
const FALLBACK: &str = "Das kann ich gerade nicht nachsehen.";

/// A duplex params document with the two deadlines held short.
///
/// `gpt_live` rather than `echo`, because the three numbers of this strand live
/// with that adapter exactly as `turn_gap_ms` and `close_grace_ms` do; the
/// object that actually RUNS is still the fixture's fake (see `duplex_params`).
fn ticking(grace_ms: u64) -> meclaw_core::serde_json::Value {
    duplex_params(json!({
        "default_mode": "auto",
        "duplex": {
            "provider": "gpt_live",
            "api_key": "test-key",
            "instructions": "Be Egon.",
            "sample_rate": 16_000,
            "tick_ms": 100,
            "delegation_grace_ms": grace_ms,
            "delegation_fallback": FALLBACK,
        },
    }))
}

/// Past the deadline the cell closes the delegation itself — once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stale_delegation_is_closed_once() {
    let mut live = boot_fake(ticking(300)).await;
    let (_client, _hello) = live.connect("session=stale-1&mode=auto").await;
    let session = live.session().await;

    session
        .say(DuplexEvent::DelegationCreated {
            delegation_id: "dlg-stale".to_string(),
            offset_ms: 0,
        })
        .await;
    // The lane went out; nothing in this colony answers it.
    let _ = live.emission("delegation").await;

    match live.control().await {
        DuplexControl::Append {
            kind,
            delegation_id,
            content,
            ..
        } => {
            assert_eq!(
                kind,
                AppendKind::Commentary,
                "the fallback is said out loud, like every `fact`"
            );
            assert_eq!(
                delegation_id.as_deref(),
                Some("dlg-stale"),
                "it closes THAT delegation, not the session in general"
            );
            assert_eq!(content, FALLBACK, "the operator's sentence, verbatim");
        }
        other => panic!("the deadline appends a commentary, got {other:?}"),
    }

    // And the delegation is gone from the list: the next ticks find nothing.
    let after = live.controls_for(Duration::from_millis(800)).await;
    assert!(
        after.is_empty(),
        "a closed delegation is closed once, not once per tick: {after:?}"
    );
}

/// A delegation answered inside the grace is never given the fallback.
///
/// The load-bearing half: without it the lock above also passes against a cell
/// that appends the fallback to every delegation the moment it arrives.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_answered_delegation_never_hears_the_fallback() {
    let mut live = boot_fake(ticking(5_000)).await;
    let (_client, _hello) = live.connect("session=stale-2&mode=auto").await;
    let session = live.session().await;

    session
        .say(DuplexEvent::DelegationCreated {
            delegation_id: "dlg-answered".to_string(),
            offset_ms: 0,
        })
        .await;
    let _ = live.emission("delegation").await;

    live.send(advise_msg(
        "stale-2",
        "fact",
        "The appointment is on Thursday at a quarter past three.",
        Some("dlg-answered"),
    ))
    .await;

    let appends = live.controls_for(Duration::from_millis(1_200)).await;
    let contents: Vec<String> = appends
        .iter()
        .filter_map(|c| match c {
            DuplexControl::Append { content, .. } => Some(content.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        contents,
        vec!["The appointment is on Thursday at a quarter past three.".to_string()],
        "the answer, and nothing the deadline added beside it"
    );
}

/// The fixture of round 2 of POC gpt-live-1: `delegation_superseded` named.
fn superseding(grace_ms: u64, mode: &str) -> meclaw_core::serde_json::Value {
    duplex_params(json!({
        "default_mode": "auto",
        "duplex": {
            "provider": "gpt_live",
            "api_key": "test-key",
            "instructions": "Be brief.",
            "sample_rate": 16_000,
            "tick_ms": 100,
            "delegation_grace_ms": grace_ms,
            "delegation_fallback": FALLBACK,
            "delegation_superseded": mode,
        },
    }))
}

/// Two delegations, the older first: X then Y.
/// The client and the session come back so they live as long as the test.
async fn two_delegations(
    live: &mut duplex_cell::Live,
    call: &str,
) -> (
    meclaw_testing::voice_client::VoiceClient,
    duplex_cell::FakeSession,
) {
    let (client, _hello) = live.connect(&format!("session={call}&mode=auto")).await;
    let session = live.session().await;
    for (id, offset_ms) in [("dlg-x", 0), ("dlg-y", 50)] {
        session
            .say(DuplexEvent::DelegationCreated {
                delegation_id: id.to_string(),
                offset_ms,
            })
            .await;
        let _ = live.emission("delegation").await;
    }
    (client, session)
}

/// The next append the model was told, whatever else it was told before it.
async fn next_append(live: &mut duplex_cell::Live) -> (AppendKind, Option<String>, String) {
    loop {
        if let DuplexControl::Append {
            kind,
            delegation_id,
            content,
            ..
        } = live.control().await
        {
            return (kind, delegation_id, content);
        }
    }
}

/// A grace no run of a test reaches: one hour. With it a deadline passes when
/// the test moves the call's clock ([`pass_the_grace`]) and at no other time,
/// so no verdict here depends on how fast the machine is.
const GRACE_MS: u64 = 3_600_000;

/// Move the call's clock past the grace of every delegation open so far.
///
/// The call's clock is the model's: every stamped event of the session sets
/// it, and the tick reads it. An `appended` answer for an event nobody sent
/// is such a stamp and nothing else -- the cell only logs it. Two hours in,
/// every delegation opened near zero is past [`GRACE_MS`], and the next tick
/// closes them.
async fn pass_the_grace(session: &duplex_cell::FakeSession) {
    session
        .say(DuplexEvent::Appended {
            kind: AppendKind::Thinking,
            event_id: "the-clock-moves".to_string(),
            start_ms: 2 * GRACE_MS,
            end_ms: 2 * GRACE_MS,
        })
        .await;
}

/// The text of the marker [`appends_up_to_mark`] sends.
const MARK: &str = "marker: nothing came before this";

/// Every append the model was told before a marker sent now.
///
/// The marker is a `context` advise without a delegation: thinking, said by
/// nobody. The cell reads its mailbox in order and hands every append to the
/// one connection in order, so whatever the messages before the marker caused
/// has reached the model before the marker does -- and what is not in this
/// list was not caused by them. No window of time decides anything here.
async fn appends_up_to_mark(
    live: &mut duplex_cell::Live,
    call: &str,
) -> Vec<(AppendKind, Option<String>, String)> {
    live.send(advise_msg(call, "context", MARK, None)).await;
    let mark = (AppendKind::Thinking, None, MARK.to_string());
    let mut before = Vec::new();
    loop {
        let got = next_append(live).await;
        if got == mark {
            return before;
        }
        before.push(got);
    }
}

/// Wave d54g, strand deleg (contract 6 a and b): an answer that names its
/// delegation closes exactly that delegation whichever lane it comes by. Over
/// `in_speak` the append carries the delegation, a second answer for it is
/// not said, and the deadline finds nothing left to close.
///
/// The deadline is what makes the cell act, not what this test judges by,
/// and it passes when the test says so: the grace is an hour, and the call's
/// clock is moved past it by an event ([`pass_the_grace`]). A second
/// delegation opens after the answer with the same offset; both deadlines
/// pass in the same tick, and the cell closes stale delegations in the order
/// they opened. So the first append after the answer is the SECOND
/// delegation's fallback exactly when the answered one got none.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_answer_over_in_speak_closes_its_delegation() {
    let mut live = boot_fake(ticking(GRACE_MS)).await;
    let (_client, _hello) = live.connect("session=spk-1&mode=auto").await;
    let session = live.session().await;
    session
        .say(DuplexEvent::DelegationCreated {
            delegation_id: "dlg-spoken".to_string(),
            offset_ms: 0,
        })
        .await;
    let _ = live.emission("delegation").await;

    let answer = |text: &str| {
        let mut speak = duplex_cell::speak_msg("spk-1", text);
        speak
            .headers
            .context
            .insert("delegation_id".into(), json!("dlg-spoken"));
        speak
    };
    live.send(answer("Thursday at a quarter past three.")).await;
    live.send(answer("Thursday, once more.")).await;
    assert_eq!(
        appends_up_to_mark(&mut live, "spk-1").await,
        vec![(
            AppendKind::Commentary,
            Some("dlg-spoken".to_string()),
            "Thursday at a quarter past three.".to_string()
        )],
        "the answer carries its delegation, and a second one for it is not said"
    );

    session
        .say(DuplexEvent::DelegationCreated {
            delegation_id: "dlg-open".to_string(),
            offset_ms: 0,
        })
        .await;
    let _ = live.emission("delegation").await;
    pass_the_grace(&session).await;
    assert_eq!(
        next_append(&mut live).await,
        (
            AppendKind::Commentary,
            Some("dlg-open".to_string()),
            FALLBACK.to_string()
        ),
        "the deadline closes the delegation nobody answered, and only that one"
    );
    assert_eq!(
        appends_up_to_mark(&mut live, "spk-1").await,
        Vec::<(AppendKind, Option<String>, String)>::new(),
        "no fallback for the delegation `in_speak` answered"
    );
}

/// POC gpt-live-1 round 2 (A94, A81): with `delegation_superseded` "think",
/// the answer to X after Y opened goes in as thinking, not speech; Y's own
/// answer is spoken as always.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_superseded_answer_is_thought_not_said() {
    let mut live = boot_fake(superseding(GRACE_MS, "think")).await;
    let _held = two_delegations(&mut live, "sup-1").await;

    live.send(advise_msg(
        "sup-1",
        "fact",
        "Friday at nine.",
        Some("dlg-x"),
    ))
    .await;
    live.send(advise_msg(
        "sup-1",
        "fact",
        "Thursday at ten.",
        Some("dlg-y"),
    ))
    .await;
    let got = appends_up_to_mark(&mut live, "sup-1").await;
    assert_eq!(
        got,
        vec![
            (
                AppendKind::Thinking,
                Some("dlg-x".to_string()),
                "Friday at nine.".to_string()
            ),
            (
                AppendKind::Commentary,
                Some("dlg-y".to_string()),
                "Thursday at ten.".to_string()
            ),
        ]
    );
}

/// The default is the cell it was: the same answer is spoken.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn by_default_a_superseded_answer_is_still_said() {
    let mut live = boot_fake(superseding(GRACE_MS, "speak")).await;
    let _held = two_delegations(&mut live, "sup-2").await;

    live.send(advise_msg(
        "sup-2",
        "fact",
        "Friday at nine.",
        Some("dlg-x"),
    ))
    .await;
    let got = appends_up_to_mark(&mut live, "sup-2").await;
    assert_eq!(
        got,
        vec![(
            AppendKind::Commentary,
            Some("dlg-x".to_string()),
            "Friday at nine.".to_string()
        )]
    );
}

/// An `in_speak` that names the superseded delegation is thought, too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_superseded_speak_is_thought_not_said() {
    let mut live = boot_fake(superseding(GRACE_MS, "think")).await;
    let _held = two_delegations(&mut live, "sup-3").await;

    let mut speak = duplex_cell::speak_msg("sup-3", "Friday at nine.");
    speak
        .headers
        .context
        .insert("delegation_id".into(), json!("dlg-x"));
    live.send(speak).await;
    let got = appends_up_to_mark(&mut live, "sup-3").await;
    assert_eq!(
        got,
        vec![(
            AppendKind::Thinking,
            Some("dlg-x".to_string()),
            "Friday at nine.".to_string()
        )]
    );
}

/// The deadline of a superseded delegation says nothing; the newer one's
/// fallback falls as always.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_superseded_delegation_gets_no_fallback() {
    let mut live = boot_fake(superseding(GRACE_MS, "think")).await;
    let (_client, session) = two_delegations(&mut live, "sup-4").await;
    pass_the_grace(&session).await;

    // Both deadlines pass in the same tick. The cell closes stale delegations
    // in the order they opened, and X opened first: Y's fallback as the
    // FIRST append means X's fell out.
    assert_eq!(
        next_append(&mut live).await,
        (
            AppendKind::Commentary,
            Some("dlg-y".to_string()),
            FALLBACK.to_string()
        ),
        "the newer delegation's fallback, with nothing said for the older one before it"
    );
    assert_eq!(
        appends_up_to_mark(&mut live, "sup-4").await,
        Vec::<(AppendKind, Option<String>, String)>::new(),
        "and nothing after it"
    );
}

/// The fixture of round 3 of POC gpt-live-1: the round-2 block plus `extra`.
fn round_three(
    grace_ms: u64,
    extra: meclaw_core::serde_json::Value,
) -> meclaw_core::serde_json::Value {
    let mut duplex = json!({
        "provider": "gpt_live",
        "api_key": "test-key",
        "instructions": "Be brief.",
        "sample_rate": 16_000,
        "tick_ms": 100,
        "delegation_grace_ms": grace_ms,
        "delegation_fallback": FALLBACK,
    });
    for (k, v) in extra.as_object().expect("an object") {
        duplex[k] = v.clone();
    }
    duplex_params(json!({"default_mode": "auto", "duplex": duplex}))
}

/// POC gpt-live-1 round 3 (A94, A45): with `delegation_superseded` "drop",
/// overtaking X emits ONE `delegation_cancel` naming X, its operation and
/// the newer Y; X's answer then reaches the model neither as speech nor as
/// thinking -- by `in_advise` or by `in_speak` -- while Y's is spoken.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_superseded_answer_is_dropped_and_its_work_cancelled() {
    let mut live = boot_fake(round_three(
        GRACE_MS,
        json!({"delegation_superseded": "drop"}),
    ))
    .await;
    let _held = two_delegations(&mut live, "sup-5").await;
    let cancel = live.emission("delegation_cancel").await;
    for (key, want) in [
        ("delegation_id", json!("dlg-x")),
        ("superseded_by", json!("dlg-y")),
        ("call_id", json!("sup-5")),
        ("operation_id", json!("sup-5:dlg-x:1")),
    ] {
        assert_eq!(
            duplex_cell::header(&cancel, key),
            Some(&want),
            "{key}: {cancel}"
        );
    }
    assert_eq!(cancel["messages"], json!([]), "{cancel}");

    live.send(advise_msg(
        "sup-5",
        "fact",
        "Friday at nine.",
        Some("dlg-x"),
    ))
    .await;
    let mut speak = duplex_cell::speak_msg("sup-5", "Friday, once more.");
    speak
        .headers
        .context
        .insert("delegation_id".into(), json!("dlg-x"));
    live.send(speak).await;
    live.send(advise_msg(
        "sup-5",
        "fact",
        "Thursday at ten.",
        Some("dlg-y"),
    ))
    .await;
    let got = appends_up_to_mark(&mut live, "sup-5").await;
    assert_eq!(
        got,
        vec![(
            AppendKind::Commentary,
            Some("dlg-y".to_string()),
            "Thursday at ten.".to_string()
        )],
        "the outdated answer is ignored, the current one is said"
    );
}

/// Without "drop" no `delegation_cancel` leaves: every delegation carries its
/// own `operation_id` and `task_revision` (A95), and that is all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_delegation_carries_its_operation_and_revision() {
    let mut live = boot_fake(round_three(GRACE_MS, json!({}))).await;
    let (_client, _hello) = live.connect("session=rev-1&mode=auto").await;
    let session = live.session().await;
    for (id, offset_ms) in [("dlg-a", 0), ("dlg-b", 50), ("dlg-c", 90)] {
        session
            .say(DuplexEvent::DelegationCreated {
                delegation_id: id.to_string(),
                offset_ms,
            })
            .await;
    }
    let mut routes = Vec::new();
    let mut delegations = Vec::new();
    while delegations.len() < 3 {
        let e = tokio::time::timeout(duplex_cell::marker(), live.emissions.recv())
            .await
            .expect("HANG: three delegations")
            .expect("the handler runs")
            .content;
        let route = duplex_cell::emission_route(&e)
            .unwrap_or_default()
            .to_string();
        if route == "delegation" {
            delegations.push(e.clone());
        }
        routes.push(route);
    }
    assert!(
        !routes.iter().any(|r| r == "delegation_cancel"),
        "no cancel unless asked: {routes:?}"
    );
    for (n, (e, id)) in delegations
        .iter()
        .zip(["dlg-a", "dlg-b", "dlg-c"])
        .enumerate()
    {
        let revision = n + 1;
        assert_eq!(
            duplex_cell::header(e, "task_revision"),
            Some(&json!(revision)),
            "{e}"
        );
        assert_eq!(
            duplex_cell::header(e, "operation_id"),
            Some(&json!(format!("rev-1:{id}:{revision}"))),
            "{e}"
        );
    }
}

/// "drop" closes the deadline of a superseded delegation without a word, as
/// "think" does; the newer one's fallback falls.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dropped_delegation_gets_no_fallback() {
    let mut live = boot_fake(round_three(
        GRACE_MS,
        json!({"delegation_superseded": "drop"}),
    ))
    .await;
    let (_client, session) = two_delegations(&mut live, "sup-6").await;
    pass_the_grace(&session).await;

    // Both deadlines pass in the same tick. The cell closes stale delegations
    // in the order they opened, and X opened first: Y's fallback as the
    // FIRST append means X's fell out.
    assert_eq!(
        next_append(&mut live).await,
        (
            AppendKind::Commentary,
            Some("dlg-y".to_string()),
            FALLBACK.to_string()
        ),
        "the newer delegation's fallback, with nothing said for the older one before it"
    );
    assert_eq!(
        appends_up_to_mark(&mut live, "sup-6").await,
        Vec::<(AppendKind, Option<String>, String)>::new(),
        "and nothing after it"
    );
}

/// The failed answer of an open delegation: `hop.error_code` and the
/// delegation on the context, words in the body the model must never hear.
fn failed_answer(call: &str, delegation_id: &str, speak: bool) -> meclaw_core::Message {
    let mut msg = if speak {
        let mut m = duplex_cell::speak_msg(call, "Error: upstream 500 at /backend");
        m.headers
            .context
            .insert("delegation_id".into(), json!(delegation_id));
        m
    } else {
        advise_msg(
            call,
            "fact",
            "Error: upstream 500 at /backend",
            Some(delegation_id),
        )
    };
    msg.headers
        .hop
        .insert("error_code".into(), json!("llm_upstream"));
    msg
}

/// POC gpt-live-1 round 3 (A86/A87): with `advise_error_fallback`, a failed
/// answer closes its delegation AT ONCE with the fallback sentence on that
/// delegation -- not after the grace -- and its body never reaches the model;
/// a second failed answer of the same delegation says nothing more.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_answer_says_the_fallback_at_once() {
    for (call, speak) in [("err-1", false), ("err-2", true)] {
        let mut live = boot_fake(round_three(
            GRACE_MS,
            json!({"advise_error_fallback": true}),
        ))
        .await;
        let (_client, _hello) = live.connect(&format!("session={call}&mode=auto")).await;
        let session = live.session().await;
        session
            .say(DuplexEvent::DelegationCreated {
                delegation_id: "dlg-e".to_string(),
                offset_ms: 0,
            })
            .await;
        let _ = live.emission("delegation").await;
        live.send(failed_answer(call, "dlg-e", speak)).await;
        live.send(failed_answer(call, "dlg-e", speak)).await;
        // At once: both are in front of the marker, an hour before the grace.
        let got = appends_up_to_mark(&mut live, call).await;
        assert_eq!(
            got,
            vec![(
                AppendKind::Commentary,
                Some("dlg-e".to_string()),
                FALLBACK.to_string()
            )],
            "{call}: the fallback once, the error text never"
        );
    }
}

/// Without the param a failed answer travels as it always did.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn by_default_a_failed_answer_is_said_as_written() {
    let mut live = boot_fake(round_three(GRACE_MS, json!({}))).await;
    let (_client, _hello) = live.connect("session=err-3&mode=auto").await;
    let session = live.session().await;
    session
        .say(DuplexEvent::DelegationCreated {
            delegation_id: "dlg-e".to_string(),
            offset_ms: 0,
        })
        .await;
    let _ = live.emission("delegation").await;
    live.send(failed_answer("err-3", "dlg-e", false)).await;
    let got = appends_up_to_mark(&mut live, "err-3").await;
    assert_eq!(
        got,
        vec![(
            AppendKind::Commentary,
            Some("dlg-e".to_string()),
            "Error: upstream 500 at /backend".to_string()
        )]
    );
}
