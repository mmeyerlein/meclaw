//! D48 / LC2 — two new sections of `in_advise` and one flag on all of them.
//!
//! `interrupt` is the guard stepping in: the text becomes a standing
//! instruction AND, right after it, a commentary with the same words, so the
//! model both changes course and says so (OR-LV-7). `renew` opens a fresh
//! session of the call, which asks for its seed again (`seed_ask` `renew`) —
//! the way facts reach a live model at a turn boundary (OR-LV-6). The flag
//! `hop.at_turn_end: "1"` holds any section until the next turn of the caller
//! closes (OR-LV-9); several `renew` held for the same boundary are ONE
//! renewal.

// harness_fold: #[path = "support/duplex_cell.rs"]
#[allow(unused_imports)]
use crate::duplex_cell; // harness_fold: mod duplex_cell;

use duplex_cell::{
    Live, boot_fake, boot_seeded, duplex_params, emission_route, handover_msg, header,
};
use meclaw_cells::voice::contract::{AppendKind, DuplexControl, DuplexEvent, Speaker};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use std::time::Duration;

/// An `in_advise` with a section, words, an optional delegation and the flag.
fn advise(
    call: &str,
    section: &str,
    text: &str,
    delegation: Option<&str>,
    at_turn_end: bool,
) -> Message {
    let mut context = Map::new();
    context.insert("call_id".into(), json!(call));
    if let Some(id) = delegation {
        context.insert("delegation_id".into(), json!(id));
    }
    let mut hop = Map::new();
    hop.insert("route".into(), json!("in_advise"));
    hop.insert("section".into(), json!(section));
    if at_turn_end {
        hop.insert("at_turn_end".into(), json!("1"));
    }
    MessageBuilder::new(Path::new("/voice"))
        .context(context)
        .hop(hop)
        .body(Body::Inline(json!({
            "messages": [{"origin": "assistant", "type": "text", "text": text}]
        })))
        .build()
}

/// An `in_seed` for a call, answering the `seed_ask` of session `renewal_n`
/// (`0` = the call's first) with the number given back on `hop.renewal_n`.
fn seed(call: &str, renewal_n: u32, text: &str) -> Message {
    let mut context = Map::new();
    context.insert("call_id".into(), json!(call));
    let mut hop = Map::new();
    hop.insert("route".into(), json!("in_seed"));
    hop.insert("renewal_n".into(), json!(renewal_n));
    MessageBuilder::new(Path::new("/voice"))
        .context(context)
        .hop(hop)
        .body(Body::Inline(
            json!({"messages": [{"role": "user", "text": text}]}),
        ))
        .build()
}

/// The caller says one thing on the model's clock.
async fn caller(session: &duplex_cell::FakeSession, text: &str, start_ms: u64, end_ms: u64) {
    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::User,
            delta: text.to_string(),
            start_ms,
            end_ms,
        })
        .await;
}

/// The next seed a session was told, with the session's index.
async fn next_seed(live: &mut Live) -> (usize, String) {
    loop {
        let (at, control) =
            tokio::time::timeout(duplex_cell::marker(), live.tagged_controls.recv())
                .await
                .expect("HANG: no seed reached a session")
                .expect("the fake is running");
        if let DuplexControl::Seed { messages } = control {
            let text = messages.first().map(|m| m.text.clone()).unwrap_or_default();
            return (at, text);
        }
    }
}

/// `interrupt`: an instruction first, then a commentary with the same words,
/// both on the delegation the advice names — the rule `correction` follows.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn advise_interrupt_sends_instructions_then_commentary() {
    let mut live = boot_fake(duplex_params(json!({"default_mode": "auto"}))).await;
    let (mut client, _hello) = live.connect("session=int-1&mode=auto").await;
    let _session = live.session().await;

    let words = "Stop the booking, the caller changed the date.";
    live.send(advise("int-1", "interrupt", words, Some("del-3"), false))
        .await;

    match live.control().await {
        DuplexControl::Append {
            kind,
            delegation_id,
            content,
            ..
        } => {
            assert_eq!(kind, AppendKind::Instructions, "the course changes first");
            assert_eq!(delegation_id.as_deref(), Some("del-3"));
            assert_eq!(content, words);
        }
        other => panic!("expected the instruction, got {other:?}"),
    }
    let event_id = match live.control().await {
        DuplexControl::Append {
            kind,
            delegation_id,
            content,
            event_id,
        } => {
            assert_eq!(kind, AppendKind::Commentary, "then it is said");
            assert_eq!(delegation_id.as_deref(), Some("del-3"));
            assert_eq!(content, words);
            event_id
        }
        other => panic!("expected the commentary, got {other:?}"),
    };
    let start = client
        .next_frame_of_type("speak_start", duplex_cell::marker())
        .await
        .expect("the commentary is a spoken section");
    assert_eq!(start["speak_id"], json!(event_id), "{start}");
}

/// `at_turn_end`: nothing reaches the model while the caller's turn is open;
/// the turn closes, and the section goes — before anything new is said.
#[tokio::test(flavor = "current_thread")]
async fn advise_at_turn_end_waits_for_person_turn_close() {
    let mut live = boot_fake(duplex_params(json!({"default_mode": "auto"}))).await;
    tokio::time::pause();
    let (_client, _hello) = live.link("ate-1", "auto").await;
    let session = live.session().await;

    caller(&session, "What is on today?", 0, 500).await;
    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::Assistant,
            delta: "Let me see.".to_string(),
            start_ms: 550,
            end_ms: 900,
        })
        .await;
    live.send(advise(
        "ate-1",
        "fact",
        "The dentist is at three.",
        None,
        true,
    ))
    .await;

    // The turn is open (900 - 500 < turn_gap_ms 1000, and the session's next
    // tick is a second away on the paused clock): held.
    let open_until = tokio::time::Instant::now() + Duration::from_millis(300);
    let mut early = Vec::new();
    while let Ok(Some(c)) = tokio::time::timeout_at(open_until, live.controls.recv()).await {
        early.push(c);
    }
    assert!(
        early.is_empty(),
        "nothing reaches the model inside the open turn: {early:?}"
    );
    let turn = live.emission("turn").await;
    assert_eq!(header(&turn, "turn_id"), Some(&json!("ate-1#0")));
    match live.control().await {
        DuplexControl::Append { kind, content, .. } => {
            assert_eq!(kind, AppendKind::Commentary);
            assert_eq!(content, "The dentist is at three.");
        }
        other => panic!("expected the held fact, got {other:?}"),
    }
}

/// `renew` at the turn boundary: the seed is asked for again and the new
/// session opens on it; the `renewed` lane goes out as always; a delegation
/// the old session opened is still open and its answer still reaches the
/// model under its handle while the renewal waits (`renew_grace_ms`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn advise_renew_restarts_session_with_fresh_seed() {
    let raw = duplex_params(json!({"default_mode": "auto"}));
    let mut live = boot_seeded(raw, 0, 3_600_000, 60_000, 400).await;
    let (_client, _hello) = live.connect("session=ren-1&mode=auto").await;
    let ask = live.emission("seed_ask").await;
    assert_eq!(header(&ask, "reason"), Some(&json!("start")));
    live.send(seed("ren-1", 0, "first")).await;
    assert_eq!(next_seed(&mut live).await.0, 1);
    let session = live.session().await;

    session
        .say(DuplexEvent::DelegationCreated {
            delegation_id: "del-9".to_string(),
            offset_ms: 100,
        })
        .await;
    live.emission("delegation").await;
    caller(&session, "Book it for Friday.", 200, 700).await;
    live.send(advise("ren-1", "renew", "", None, true)).await;

    live.emission("turn").await;
    let ask = live.emission("seed_ask").await;
    assert_eq!(header(&ask, "reason"), Some(&json!("renew")), "{ask}");
    live.send(seed("ren-1", 1, "fresh")).await;
    let (at, text) = next_seed(&mut live).await;
    assert_eq!(
        (at, text.as_str()),
        (2, "fresh"),
        "the renewal opens on the fresh seed"
    );
    let renewed = live.emission("renewed").await;
    assert_eq!(header(&renewed, "renewal_n"), Some(&json!(1)));

    live.send(advise(
        "ren-1",
        "fact",
        "Friday is booked.",
        Some("del-9"),
        false,
    ))
    .await;
    loop {
        let (at, control) =
            tokio::time::timeout(duplex_cell::marker(), live.tagged_controls.recv())
                .await
                .expect("HANG: the answer never reached a session")
                .expect("the fake is running");
        if let DuplexControl::Append {
            delegation_id,
            content,
            ..
        } = control
        {
            assert_eq!(at, 1, "the old session is in charge until the handover");
            assert_eq!(
                delegation_id.as_deref(),
                Some("del-9"),
                "the handle survived"
            );
            assert_eq!(content, "Friday is booked.");
            break;
        }
    }
}

/// Several `renew` for the same turn boundary are one renewal, and a `renew`
/// while a renewal waits for its seed opens no second one: that seed carries
/// the conversation as it stands. Absence is proved past the point where the
/// second renewal would have opened (review M5): after the takeover, a fresh
/// `renew` opens renewal 2 -- not 3 -- and it is the first `seed_ask` since
/// renewal 1's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn advise_renew_coalesces_to_one_at_the_turn_boundary() {
    let raw = duplex_params(json!({"default_mode": "auto"}));
    let mut live = boot_seeded(raw, 0, 3_600_000, 60_000, 400).await;
    let (_client, _hello) = live.connect("session=ren-2&mode=auto").await;
    live.emission("seed_ask").await;
    live.send(seed("ren-2", 0, "first")).await;
    assert_eq!(next_seed(&mut live).await.0, 1);
    let session = live.session().await;

    caller(&session, "One more thing.", 0, 400).await;
    for _ in 0..3 {
        live.send(advise("ren-2", "renew", "", None, true)).await;
    }
    live.emission("turn").await;
    let ask = live.emission("seed_ask").await;
    assert_eq!(header(&ask, "reason"), Some(&json!("renew")));
    assert_eq!(header(&ask, "renewal_n"), Some(&json!(1)), "{ask}");
    // While renewal 1's seed is still to come: joins it.
    live.send(advise("ren-2", "renew", "", None, false)).await;
    // A marker on the same path proves the connection has taken that renew
    // before the seed is answered.
    live.send(advise("ren-2", "context", "Marker.", None, false))
        .await;
    next_append_with(&mut live, "Marker.").await;
    live.send(seed("ren-2", 1, "fresh")).await;
    assert_eq!(next_seed(&mut live).await, (2, "fresh".to_string()));
    live.emission("renewed").await;

    live.send(handover_msg("ren-2", "summary", 1)).await;
    took_over(&mut live, 1).await;
    live.send(advise("ren-2", "renew", "", None, false)).await;
    let routes = routes_until_seed_ask(&mut live).await;
    let (ask, before) = routes;
    assert_eq!(header(&ask, "renewal_n"), Some(&json!(2)), "{ask}");
    assert!(
        !before.iter().any(|r| r == "seed_ask" || r == "renewed"),
        "one renewal for one boundary: {before:?}"
    );
}

/// Review M1: a `renew` that comes after the opening renewal's seed was
/// answered is a NEW boundary -- the fact of the turn in between is in no seed
/// yet. It opens one more renewal as soon as the open one took over.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn advise_renew_after_the_seed_opens_one_more_renewal() {
    let raw = duplex_params(json!({"default_mode": "auto"}));
    let mut live = boot_seeded(raw, 0, 3_600_000, 60_000, 400).await;
    let (_client, _hello) = live.connect("session=ren-3&mode=auto").await;
    live.emission("seed_ask").await;
    live.send(seed("ren-3", 0, "first")).await;
    assert_eq!(next_seed(&mut live).await.0, 1);
    let session = live.session().await;

    caller(&session, "Book it for Friday.", 0, 400).await;
    live.send(advise("ren-3", "renew", "", None, true)).await;
    live.emission("turn").await;
    let ask = live.emission("seed_ask").await;
    assert_eq!(header(&ask, "renewal_n"), Some(&json!(1)), "{ask}");
    live.send(seed("ren-3", 1, "friday")).await;
    assert_eq!(next_seed(&mut live).await, (2, "friday".to_string()));
    live.emission("renewed").await;

    // The next turn brings a new fact while renewal 1 waits for its handover.
    caller(&session, "No, make it Saturday.", 3000, 3400).await;
    live.send(advise("ren-3", "renew", "", None, true)).await;
    live.emission("turn").await;

    live.send(handover_msg("ren-3", "summary", 1)).await;
    took_over(&mut live, 1).await;
    let ask = live.emission("seed_ask").await;
    assert_eq!(header(&ask, "reason"), Some(&json!("renew")), "{ask}");
    assert_eq!(header(&ask, "renewal_n"), Some(&json!(2)), "{ask}");
    live.send(seed("ren-3", 2, "saturday")).await;
    assert_eq!(
        next_seed(&mut live).await,
        (3, "saturday".to_string()),
        "the fact of the turn in between reaches a session"
    );
}

/// Wait until the control `content` reaches a session (an append).
async fn next_append_with(live: &mut Live, content: &str) -> usize {
    loop {
        let (at, control) =
            tokio::time::timeout(duplex_cell::marker(), live.tagged_controls.recv())
                .await
                .expect("HANG: the marker never reached a session")
                .expect("the fake is running");
        if let DuplexControl::Append { content: c, .. } = control
            && c == content
        {
            return at;
        }
    }
}

/// Wait until session `index` lost the caller's audio: the renewal took over.
async fn took_over(live: &mut Live, index: usize) {
    loop {
        let ended = tokio::time::timeout(duplex_cell::marker(), live.ended.recv())
            .await
            .expect("HANG: no takeover")
            .expect("the fake is running");
        if ended == index {
            return;
        }
    }
}

/// The next `seed_ask`, with the routes of every emission before it.
async fn routes_until_seed_ask(live: &mut Live) -> (Value, Vec<String>) {
    let mut before = Vec::new();
    loop {
        let e = tokio::time::timeout(duplex_cell::marker(), live.emissions.recv())
            .await
            .expect("HANG: no seed_ask")
            .expect("the cell is running");
        let route = emission_route(&e.content).unwrap_or_default().to_string();
        if route == "seed_ask" {
            return (e.content, before);
        }
        before.push(route);
    }
}
