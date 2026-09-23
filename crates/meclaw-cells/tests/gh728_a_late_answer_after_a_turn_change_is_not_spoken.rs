//! GH #728, lock 8 — a late answer after a turn change is not spoken.
//!
//! Ruling point 2, the voice half: "a voice call after a turn change drops it". A
//! straggler (`hop.late == "1"`) whose `hop.turn_id` is no longer the turn the call is
//! in would be a sentence about something the caller stopped talking about — so the
//! cell drops it: no synthesis, no append, no `speak_end`, and no error line either,
//! because nobody did anything wrong. A late answer to the turn that is STILL running
//! is spoken, and so is every answer inside the deadline.
//!
//! "Running" per engine: the cascade's is `"<session>#<turn_seq>"`, the last turn it
//! emitted; a duplex session's is the open turn or the one that closed last.
//!
//! The duplex SIDECAR path is the third case (OR-T13). A delegation's answer arrives as
//! an `in_advise` `fact`, whose header the splitter builds fresh, so it carries no
//! `late` at all. What the cell knows instead is its own deadline: a delegation it
//! already closed with the fallback sentence, and since whose opening another turn has
//! closed, gets no second answer spoken into a conversation that moved on.

#[path = "support/duplex_cell.rs"]
mod duplex_cell;

use duplex_cell::{advise_msg, boot_cascade, boot_fake, duplex_params, emission_route};
use meclaw_cells::voice::contract::{DuplexControl, DuplexEvent, Speaker};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use std::time::{Duration, Instant};

/// An answer for the call, as the member's edge delivers it: `turn_id` and `late`
/// on the hop, exactly where `collector/assemble` puts them.
fn answer(call: &str, turn_id: &str, late: &str, text: &str) -> Message {
    let mut context = meclaw_core::serde_json::Map::new();
    context.insert("call_id".into(), json!(call));
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("route".into(), json!("in_speak"));
    hop.insert("turn_id".into(), json!(turn_id));
    hop.insert("late".into(), json!(late));
    MessageBuilder::new(Path::new("/voice"))
        .context(context)
        .hop(hop)
        .body(Body::Inline(json!({
            "messages": [{"origin": "assistant", "type": "text", "text": text}]
        })))
        .build()
}

/// Every emission route within `window`.
async fn routes_for(live: &mut duplex_cell::Live, window: Duration) -> Vec<String> {
    let deadline = Instant::now() + window;
    let mut out = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return out;
        }
        match tokio::time::timeout(left, live.emissions.recv()).await {
            Ok(Some(e)) => out.push(emission_route(&e.content).unwrap_or_default().to_string()),
            Ok(None) | Err(_) => return out,
        }
    }
}

fn appended(controls: &[DuplexControl]) -> Vec<String> {
    controls
        .iter()
        .filter_map(|c| match c {
            DuplexControl::Append { content, .. } => Some(content.clone()),
            _ => None,
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────── cascade (half duplex)

fn cascade() -> Value {
    json!({"mount": duplex_cell::MOUNT, "stt": {"provider": "echo"}, "emit_speak_end": true})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cascade_drops_a_straggler_for_a_turn_that_is_not_running() {
    let mut live = boot_cascade(cascade()).await;
    let (_client, _hello) = live.connect("session=hd-1&mode=auto").await;
    // No turn has closed: the running one is `hd-1#0`. `hd-1#4` is not it.
    live.send(answer("hd-1", "hd-1#4", "1", "It is 21C in Berlin."))
        .await;
    let routes = routes_for(&mut live, Duration::from_millis(800)).await;
    assert!(
        !routes.iter().any(|r| r == "speak_end" || r == "error"),
        "a straggler after a turn change is neither spoken nor reported: {routes:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cascade_speaks_a_late_answer_to_the_running_turn_and_every_leg() {
    let mut live = boot_cascade(cascade()).await;
    let (_client, _hello) = live.connect("session=hd-2&mode=auto").await;
    live.send(answer("hd-2", "hd-2#0", "1", "Late, but still on topic."))
        .await;
    let first = live.emission("speak_end").await;
    assert!(first.to_string().contains("hd-2"), "{first}");
    live.send(answer("hd-2", "hd-2#4", "0", "Inside the deadline."))
        .await;
    let _ = live.emission("speak_end").await;
}

// ──────────────────────────────────────────────────────────────────────────── duplex

/// User fragments on the model's clock, each a full gap after the one before: every
/// fragment after the first closes the turn in front of it. Waits for those turns.
async fn fragments(
    live: &mut duplex_cell::Live,
    session: &duplex_cell::FakeSession,
    starts: &[u64],
) {
    for at in starts {
        session
            .say(DuplexEvent::Transcript {
                speaker: Speaker::User,
                delta: format!("fragment at {at}"),
                start_ms: *at,
                end_ms: at + 500,
            })
            .await;
    }
    for _ in 1..starts.len() {
        let _ = live.emission("turn").await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_duplex_call_drops_a_straggler_two_turns_on() {
    let mut live = boot_fake(duplex_params(json!({"default_mode": "auto"}))).await;
    let (_client, _hello) = live.connect("session=dx-1&mode=auto").await;
    let session = live.session().await;
    fragments(&mut live, &session, &[0, 5_000, 10_000]).await;
    // Two turns closed: dx-1#2 is the open one (or, should a tick close it on the
    // silent line, the last closed one) — running either way; dx-1#0 is two back.
    live.send(answer("dx-1", "dx-1#0", "1", "Stale.")).await;
    live.send(answer("dx-1", "dx-1#2", "1", "Late, running turn."))
        .await;
    live.send(answer("dx-1", "dx-1#0", "0", "A leg is always said."))
        .await;
    let said = appended(&live.controls_for(Duration::from_millis(1_200)).await);
    assert_eq!(
        said,
        vec![
            "Late, running turn.".to_string(),
            "A leg is always said.".to_string()
        ],
        "only the straggler for a turn two turns back is dropped"
    );
}

/// The sidecar half: a `fact` for a delegation the fallback already closed.
fn ticking(grace_ms: u64) -> Value {
    duplex_params(json!({
        "default_mode": "auto",
        "duplex": {
            "provider": "gpt_live",
            "api_key": "test-key",
            "instructions": "Be Egon.",
            "sample_rate": 16_000,
            "tick_ms": 100,
            "delegation_grace_ms": grace_ms,
            "delegation_fallback": "I cannot look that up right now.",
        },
    }))
}

async fn fallback_closes(
    live: &mut duplex_cell::Live,
    session: &duplex_cell::FakeSession,
    id: &str,
) {
    session
        .say(DuplexEvent::DelegationCreated {
            delegation_id: id.to_string(),
            offset_ms: 0,
        })
        .await;
    let _ = live.emission("delegation").await;
    match live.control().await {
        DuplexControl::Append { delegation_id, .. } => {
            assert_eq!(delegation_id.as_deref(), Some(id), "the fallback closed it");
        }
        other => panic!("expected the fallback append, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fact_for_a_delegation_closed_by_the_fallback_is_dropped_after_a_turn_change() {
    let mut live = boot_fake(ticking(300)).await;
    let (_client, _hello) = live.connect("session=sc-1&mode=auto").await;
    let session = live.session().await;
    fallback_closes(&mut live, &session, "dlg-late").await;
    fragments(&mut live, &session, &[0, 5_000]).await;
    live.send(advise_msg(
        "sc-1",
        "fact",
        "The appointment is on Thursday.",
        Some("dlg-late"),
    ))
    .await;
    let routes = routes_for(&mut live, Duration::from_millis(800)).await;
    let said = appended(&live.controls_for(Duration::from_millis(400)).await);
    assert!(
        said.is_empty(),
        "a moved-on conversation hears nothing: {said:?}"
    );
    assert!(
        !routes.iter().any(|r| r == "error"),
        "and nothing is reported: {routes:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fact_after_the_fallback_but_in_the_same_turn_is_still_said() {
    let mut live = boot_fake(ticking(300)).await;
    let (_client, _hello) = live.connect("session=sc-2&mode=auto").await;
    let session = live.session().await;
    fallback_closes(&mut live, &session, "dlg-same").await;
    live.send(advise_msg(
        "sc-2",
        "fact",
        "The appointment is on Thursday.",
        Some("dlg-same"),
    ))
    .await;
    let said = appended(&live.controls_for(Duration::from_millis(1_000)).await);
    assert_eq!(said, vec!["The appointment is on Thursday.".to_string()]);
}
