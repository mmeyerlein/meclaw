//! D48 (Welle live-2, LC1) — a duplex session asks for its seed and gets it.
//!
//! With `seed_wait_ms` set, every session of a call that opens -- at the ring
//! (GH #1055), at the media connection, at a renewal (GH #896) -- leaves a
//! `seed_ask` lane with `reason` `start` or `renew`, and the colony's answer on
//! `in_seed` reaches THAT session as `DuplexControl::Seed`. Without the knob
//! nothing asks. What the provider does with the seed (wait, `session.input`,
//! start without it after the wait) is pinned at the adapter
//! (`voice_duplex_gpt_live_seed.rs` and the unit tests of `gpt_live.rs`).

// harness_fold: #[path = "support/duplex_cell.rs"]
#[allow(unused_imports)]
use crate::duplex_cell; // harness_fold: mod duplex_cell;

use meclaw_cells::voice::contract::{AppendKind, DuplexControl, DuplexEvent, SeedMessage};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use std::time::Duration;

/// A line fact as the switch posts it.
fn line_fact(route: &str, call: &str) -> Message {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("route".into(), json!(route));
    hop.insert("call_uuid".into(), json!(call));
    MessageBuilder::new(Path::new("/voice"))
        .hop(hop)
        .body(Body::Inline(json!({"messages": []})))
        .build()
}

/// The colony's answer to the `seed_ask` of session `renewal_n` (`0` = the
/// call's first session), with the number given back on `hop.renewal_n`.
fn seed_msg(call: &str, renewal_n: u32, messages: Value) -> Message {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("route".into(), json!("in_seed"));
    hop.insert("renewal_n".into(), json!(renewal_n));
    let mut context = meclaw_core::serde_json::Map::new();
    context.insert("call_id".into(), json!(call));
    MessageBuilder::new(Path::new("/voice"))
        .hop(hop)
        .context(context)
        .body(Body::Inline(
            json!({"messages": messages, "est_tokens": 99_999}),
        ))
        .build()
}

/// The next `Seed` any session was told, and which session it reached.
async fn next_seed(live: &mut duplex_cell::Live) -> (usize, Vec<SeedMessage>) {
    loop {
        let (at, control) =
            tokio::time::timeout(duplex_cell::marker(), live.tagged_controls.recv())
                .await
                .expect("HANG: no seed reached a session")
                .expect("the fake is running");
        if let DuplexControl::Seed { messages } = control {
            return (at, messages);
        }
    }
}

/// Routes of every emission up to and including the first `until`.
async fn routes_until(live: &mut duplex_cell::Live, until: &str) -> Vec<String> {
    let mut seen = Vec::new();
    loop {
        let e = tokio::time::timeout(duplex_cell::marker(), live.emissions.recv())
            .await
            .unwrap_or_else(|_| panic!("HANG: no `{until}`; saw {seen:?}"))
            .expect("the handler is running");
        let route = duplex_cell::emission_route(&e.content)
            .unwrap_or_default()
            .to_string();
        seen.push(route.clone());
        if route == until {
            return seen;
        }
    }
}

/// Test 7: a session opened while the line rings asks for its seed at the
/// ring -- before any media connection, so before its `session.start` -- and
/// the seed reaches the parked session.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gpt_live_ringing_open_asks_seed() {
    let raw = duplex_cell::duplex_params(json!({"default_mode": "auto"}));
    let mut live = duplex_cell::boot_seeded(raw, 10_000, 300, 60_000, 400).await;

    live.send(line_fact("call_ringing", "ring-1")).await;
    let ask = live.emission("seed_ask").await;
    assert_eq!(duplex_cell::header(&ask, "reason"), Some(&json!("start")));
    assert_eq!(duplex_cell::header(&ask, "renewal_n"), Some(&json!(0)));
    assert_eq!(duplex_cell::header(&ask, "call_id"), Some(&json!("ring-1")));
    assert_eq!(duplex_cell::header(&ask, "engine"), Some(&json!("duplex")));

    live.send(seed_msg(
        "ring-1",
        0,
        json!([
            {"role": "developer", "text": "Today is Tuesday."},
            {"role": "user", "text": "Hallo Egon"},
            {"role": "assistant", "text": "Hallo!"},
            {"role": "system", "text": "not a role the model takes"}
        ]),
    ))
    .await;
    let (at, messages) = next_seed(&mut live).await;
    assert_eq!(at, 1, "the parked session of the ring");
    assert_eq!(
        messages,
        vec![
            SeedMessage {
                role: "developer".into(),
                text: "Today is Tuesday.".into()
            },
            SeedMessage {
                role: "user".into(),
                text: "Hallo Egon".into()
            },
            SeedMessage {
                role: "assistant".into(),
                text: "Hallo!".into()
            },
        ],
        "order and roles kept, a role the model does not take left out"
    );

    // The media connection adopts the session and asks nothing twice: the
    // next ask of this call is the renewal's, not a second `start`.
    let (_client, hello) = live
        .connect("session=ring-1&sample_rate=16000&mode=auto")
        .await;
    assert_eq!(hello["duplex"], true, "got {hello}");
    let next = live.emission("seed_ask").await;
    assert_eq!(duplex_cell::header(&next, "reason"), Some(&json!("renew")));
    assert_eq!(duplex_cell::header(&next, "renewal_n"), Some(&json!(1)));
}

/// Test 4: a renewal asks again with `reason: renew` and its number, its seed
/// reaches the RENEWAL (session 2) and not the session in charge, and the
/// `renewed` lane goes out as it always did. Every seed names the session it
/// answers (`hop.renewal_n`), so no assertion here depends on WHEN the answer
/// comes: the start's seed may arrive before or after the renewal opened, it
/// reaches session 1 either way (review M1/M2, no real-time window).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gpt_live_renew_asks_seed_again() {
    let raw = duplex_cell::duplex_params(json!({"default_mode": "auto"}));
    let mut live = duplex_cell::boot_seeded(raw, 0, 300, 60_000, 400).await;
    let (_client, _hello) = live.connect("session=renew-s&mode=auto").await;

    let ask = live.emission("seed_ask").await;
    assert_eq!(duplex_cell::header(&ask, "reason"), Some(&json!("start")));
    assert_eq!(duplex_cell::header(&ask, "renewal_n"), Some(&json!(0)));
    live.send(seed_msg(
        "renew-s",
        0,
        json!([{"role": "user", "text": "first"}]),
    ))
    .await;
    assert_eq!(next_seed(&mut live).await.0, 1);

    let ask = live.emission("seed_ask").await;
    assert_eq!(duplex_cell::header(&ask, "reason"), Some(&json!("renew")));
    assert_eq!(duplex_cell::header(&ask, "renewal_n"), Some(&json!(1)));
    live.send(seed_msg(
        "renew-s",
        1,
        json!([{"origin": "assistant", "type": "text", "text": "so far"}]),
    ))
    .await;
    let (at, messages) = next_seed(&mut live).await;
    assert_eq!(at, 2, "the renewal asked, the renewal gets it");
    assert_eq!(
        messages,
        vec![SeedMessage {
            role: "assistant".into(),
            text: "so far".into()
        }],
        "`origin` stands in for `role`, the universal body's key"
    );
    let renewed = live.emission("renewed").await;
    assert_eq!(duplex_cell::header(&renewed, "renewal_n"), Some(&json!(1)));
}

/// Review M1 (1): the start's answer comes only after the renewal is due and
/// has asked. It still reaches the session that asked for it -- the first --
/// and the renewal gets its own seed, not the conversation of the call's
/// first second. Ordered by events only: both asks are seen before either
/// answer is sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gpt_live_start_seed_after_a_due_renewal_reaches_the_start() {
    let raw = duplex_cell::duplex_params(json!({"default_mode": "auto"}));
    let mut live = duplex_cell::boot_seeded(raw, 0, 300, 60_000, 400).await;
    let (_client, _hello) = live.connect("session=late-start&mode=auto").await;
    let start = live.emission("seed_ask").await;
    assert_eq!(duplex_cell::header(&start, "renewal_n"), Some(&json!(0)));
    let renew = live.emission("seed_ask").await;
    assert_eq!(duplex_cell::header(&renew, "renewal_n"), Some(&json!(1)));

    live.send(seed_msg(
        "late-start",
        0,
        json!([{"role": "user", "text": "the start's"}]),
    ))
    .await;
    let (at, messages) = next_seed(&mut live).await;
    assert_eq!(
        at, 1,
        "the start asked, the start gets it -- not the renewal"
    );
    assert_eq!(messages[0].text, "the start's");

    live.send(seed_msg(
        "late-start",
        1,
        json!([{"role": "user", "text": "the renewal's"}]),
    ))
    .await;
    let (at, messages) = next_seed(&mut live).await;
    assert_eq!(at, 2);
    assert_eq!(messages[0].text, "the renewal's");
}

/// Review M1 (2): renewal 1 has taken over and renewal 2 is open when the
/// late answer to renewal 1 comes. It is stale and dropped; renewal 2 opens
/// on its own answer, which is the first seed it is told.
///
/// Ordered by events only (review R1 of the re-check): with a grace of 60 s
/// no renewal takes over on its own inside the test. Renewal 1 takes over at
/// its handover (`hop.renewal_n` 1, GH #896), and renewal 2 stays open for the
/// rest of the test, so `fresh` always finds it. A short grace let renewal 2
/// take over about 200 ms after its ask and turned `fresh` stale as well.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gpt_live_a_late_seed_for_renewal_n_skips_the_open_renewal_n_plus_1() {
    let raw = duplex_cell::duplex_params(json!({"default_mode": "auto"}));
    let mut live = duplex_cell::boot_seeded(raw, 0, 300, 60_000, 400).await;
    let (_client, _hello) = live.connect("session=late-renew&mode=auto").await;
    for n in 0..=1 {
        let ask = live.emission("seed_ask").await;
        assert_eq!(duplex_cell::header(&ask, "renewal_n"), Some(&json!(n)));
    }
    live.send(duplex_cell::handover_msg(
        "late-renew",
        "The story so far.",
        1,
    ))
    .await;
    // The handover reaching renewal 1 is the takeover's event (GH #896).
    loop {
        let (at, control) =
            tokio::time::timeout(duplex_cell::marker(), live.tagged_controls.recv())
                .await
                .expect("HANG: the handover reaches no session")
                .expect("the fake is running");
        assert!(
            !matches!(control, DuplexControl::Seed { .. }),
            "no seed was sent yet, none may reach a session: {control:?}"
        );
        if let DuplexControl::Append {
            kind: AppendKind::Thinking,
            ..
        } = control
        {
            assert_eq!(at, 2, "the handover is renewal 1's");
            break;
        }
    }
    let ask = live.emission("seed_ask").await;
    assert_eq!(
        duplex_cell::header(&ask, "renewal_n"),
        Some(&json!(2)),
        "renewal 1 in charge opens renewal 2"
    );

    live.send(seed_msg(
        "late-renew",
        1,
        json!([{"role": "user", "text": "stale"}]),
    ))
    .await;
    live.send(seed_msg(
        "late-renew",
        2,
        json!([{"role": "user", "text": "fresh"}]),
    ))
    .await;
    let (at, messages) = next_seed(&mut live).await;
    assert_eq!(at, 3, "renewal 2 is the call's third session");
    assert_eq!(
        messages[0].text, "fresh",
        "the late answer to renewal 1 never reaches renewal 2"
    );
}

/// An answer that does not say which session it answers is refused by name:
/// without the number the connection cannot tell a start from a renewal.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gpt_live_in_seed_without_its_number_is_refused() {
    let raw = duplex_cell::duplex_params(json!({"default_mode": "auto"}));
    let mut live = duplex_cell::boot_seeded(raw, 0, 0, 5_000, 400).await;
    let (_client, _hello) = live.connect("session=no-n&mode=auto").await;
    let mut msg = seed_msg("no-n", 0, json!([{"role": "user", "text": "x"}]));
    msg.headers.hop.remove("renewal_n");
    live.send(msg).await;
    let err = live.emission("error").await;
    assert_eq!(
        duplex_cell::header(&err, "error_code"),
        Some(&json!("missing_renewal_n"))
    );
}

/// Review K5: a seed for a call no session holds -- neither parked at the
/// ring nor held by a connection -- is `unknown_session` on the error lane.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gpt_live_in_seed_for_a_call_nobody_holds_is_unknown_session() {
    let raw = duplex_cell::duplex_params(json!({"default_mode": "auto"}));
    let mut live = duplex_cell::boot_seeded(raw, 0, 0, 5_000, 400).await;
    live.send(seed_msg(
        "nobody",
        0,
        json!([{"role": "user", "text": "x"}]),
    ))
    .await;
    let err = live.emission("error").await;
    assert_eq!(
        duplex_cell::header(&err, "error_code"),
        Some(&json!("unknown_session"))
    );
    assert_eq!(duplex_cell::header(&err, "call_id"), Some(&json!("nobody")));
}

/// Review K7: a delegation the first session opened, answered after a
/// SEEDED renewal took over, is spoken on the renewal without its handle --
/// the seed changes nothing about the delegation table (GH #896).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gpt_live_a_delegation_survives_a_seeded_renewal_as_a_late_fact() {
    let raw = duplex_cell::duplex_params(json!({"default_mode": "auto"}));
    let mut live = duplex_cell::boot_seeded(raw, 0, 300, 60_000, 400).await;
    let (_client, _hello) = live.connect("session=dlg-s&mode=auto").await;
    let first = live.session().await;
    // Both asks first (`emission` passes over other lanes), then the
    // delegation of session 1, which is still in charge (grace 60 s).
    let ask = live.emission("seed_ask").await;
    assert_eq!(duplex_cell::header(&ask, "reason"), Some(&json!("start")));
    let ask = live.emission("seed_ask").await;
    assert_eq!(duplex_cell::header(&ask, "reason"), Some(&json!("renew")));
    first
        .say(DuplexEvent::DelegationCreated {
            delegation_id: "dlg-old".to_string(),
            offset_ms: 100,
        })
        .await;
    let _delegation = live.emission("delegation").await;
    live.send(seed_msg(
        "dlg-s",
        1,
        json!([{"role": "user", "text": "so far"}]),
    ))
    .await;
    assert_eq!(next_seed(&mut live).await.0, 2);
    live.send(duplex_cell::handover_msg("dlg-s", "The story so far.", 1))
        .await;
    // The handover reaching the renewal is the takeover's event (GH #896).
    loop {
        let (at, control) =
            tokio::time::timeout(duplex_cell::marker(), live.tagged_controls.recv())
                .await
                .expect("HANG: the handover reaches no session")
                .expect("the fake is running");
        if let DuplexControl::Append {
            kind: AppendKind::Thinking,
            ..
        } = control
        {
            assert_eq!(at, 2, "the handover is the renewal's");
            break;
        }
    }
    live.send(duplex_cell::advise_msg(
        "dlg-s",
        "fact",
        "It is sunny.",
        Some("dlg-old"),
    ))
    .await;
    loop {
        let (at, control) =
            tokio::time::timeout(duplex_cell::marker(), live.tagged_controls.recv())
                .await
                .expect("HANG: the answer reaches no session")
                .expect("the fake is running");
        if let DuplexControl::Append {
            kind: AppendKind::Commentary,
            delegation_id,
            content,
            ..
        } = control
        {
            assert_eq!(at, 2, "the answer reaches the session in charge");
            assert_eq!(delegation_id, None, "the handle belonged to session 1");
            assert_eq!(content, "It is sunny.");
            return;
        }
    }
}

/// Test 3 (the lane half): without `seed_wait_ms` nothing asks -- a renewal
/// goes out on `renewed` with no `seed_ask` before it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gpt_live_seed_off_asks_nothing() {
    let raw = duplex_cell::duplex_params(json!({"default_mode": "auto"}));
    let mut live = duplex_cell::boot_seeded(raw, 0, 300, 60_000, 0).await;
    let (_client, _hello) = live.connect("session=quiet-s&mode=auto").await;
    let seen = routes_until(&mut live, "renewed").await;
    assert!(
        !seen.iter().any(|r| r == "seed_ask"),
        "no seed_ask without seed_wait_ms: {seen:?}"
    );
    // The fake was never told a seed either.
    let told = live.controls_for(Duration::from_millis(50)).await;
    assert!(
        !told.iter().any(|c| matches!(c, DuplexControl::Seed { .. })),
        "{told:?}"
    );
}

/// A seed for a cascade is refused by name; one that names no call as well.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gpt_live_in_seed_is_refused_where_it_cannot_land() {
    let mut cascade = duplex_cell::boot_cascade(duplex_cell::cascade_params()).await;
    cascade
        .send(seed_msg("c-1", 0, json!([{"role": "user", "text": "x"}])))
        .await;
    let err = cascade.emission("error").await;
    assert_eq!(
        duplex_cell::header(&err, "error_code"),
        Some(&json!("wrong_engine"))
    );

    let raw = duplex_cell::duplex_params(json!({"default_mode": "auto"}));
    let mut live = duplex_cell::boot_seeded(raw, 0, 0, 5_000, 400).await;
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("route".into(), json!("in_seed"));
    let nameless = MessageBuilder::new(Path::new("/voice"))
        .hop(hop)
        .body(Body::Inline(json!({"messages": []})))
        .build();
    live.send(nameless).await;
    let err = live.emission("error").await;
    assert_eq!(
        duplex_cell::header(&err, "error_code"),
        Some(&json!("missing_session"))
    );
}
