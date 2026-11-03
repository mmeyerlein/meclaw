//! #1404 -- the voice interrupts once per turn, and a waiting horizon never
//! announces an answer that is already being said.
//!
//! WHY this file exists: two senders can interrupt the same turn of a call --
//! the curator on the first crisis partial (`interrupt_on_delta`, early) and
//! the guard at the turn's end (the safety net, unchanged). The FIRST
//! `interrupt` of a `hop.turn_id` in a session acts (an instruction and a
//! spoken commentary, OR-LV-7); every later one of that turn is a no-op with
//! a trace line `interrupt_deduped` -- no append, no refusal. Without a
//! `turn_id` nothing is deduplicated (as before). And a horizon (#1400) that
//! waits for the line's quiet moment lapses when the answer goes out (a new
//! spoken section, or an append that answers a delegation): said after it,
//! it would announce what the caller has already heard (V3 review, N1).

// harness_fold: #[path = "support/duplex_cell.rs"]
#[allow(unused_imports)]
use crate::duplex_cell; // harness_fold: mod duplex_cell;

use duplex_cell::{CAP_MS, Live, boot_fake, boot_fake_timed, duplex_params};
use meclaw_cells::voice::contract::{AppendKind, DuplexControl};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};

const QUESTION: &str = "Are you safe right now?";
const HORIZON_TEXT: &str = "The answer takes about {seconds} seconds; keep the talk going.";
/// The quiet after the model's last speech before the line counts as quiet:
/// short, so a horizon still waiting goes right after the section's end --
/// before the marker that proves it lapsed (the open sections hold it, not
/// this clock).
const QUIET_MS: u64 = 1;

fn params() -> Value {
    duplex_params(json!({
        "default_mode": "auto",
        "duplex": {
            "provider": "gpt_live",
            "api_key": "test-key",
            "instructions": "A test line.",
            "sample_rate": 16_000,
            "tick_ms": 100,
            "turn_gap_ms": 300,
            "horizon_text": HORIZON_TEXT,
        },
    }))
}

/// An `in_advise` as the set hands it on, with the hop keys the test names.
fn advise(call: &str, section: &str, body: Value, hop_extra: Value) -> Message {
    let mut context = Map::new();
    context.insert("call_id".into(), json!(call));
    let mut hop = Map::new();
    hop.insert("route".into(), json!("in_advise"));
    hop.insert("section".into(), json!(section));
    if let Value::Object(extra) = hop_extra {
        hop.extend(extra);
    }
    MessageBuilder::new(Path::new("/voice"))
        .context(context)
        .hop(hop)
        .body(Body::Inline(body))
        .build()
}

fn interrupt(call: &str, turn: Option<&str>) -> Message {
    let hop = turn.map_or(json!({}), |t| json!({"turn_id": t}));
    advise(
        call,
        "interrupt",
        json!({"messages": [{"origin": "assistant", "type": "text", "text": QUESTION}]}),
        hop,
    )
}

fn marker(call: &str) -> Message {
    duplex_cell::advise_msg(call, "context", "Marker.", None)
}

/// Every append up to and including the one carrying `content`.
async fn appends_until(live: &mut Live, content: &str) -> Vec<(AppendKind, String)> {
    let mut out = Vec::new();
    loop {
        let control = tokio::time::timeout(duplex_cell::marker(), live.controls.recv())
            .await
            .unwrap_or_else(|_| panic!("HANG: `{content}` never reached the model; saw {out:?}"))
            .expect("the fake is running");
        if let DuplexControl::Append {
            kind, content: c, ..
        } = control
        {
            let done = c == content;
            out.push((kind, c));
            if done {
                return out;
            }
        }
    }
}

/// Pin 5: two interrupts of the same turn -- the early one and the guard's
/// at the turn's end -- reach the model as ONE instruction and ONE
/// commentary; the second is a no-op, not a refusal.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_interrupt_of_the_same_turn_is_a_no_op() {
    let mut live = boot_fake(params()).await;
    let (_client, _hello) = live.connect("session=ie-1&mode=auto").await;
    let _session = live.session().await;

    live.send(interrupt("ie-1", Some("ie-1#2"))).await;
    live.send(interrupt("ie-1", Some("ie-1#2"))).await;
    live.send(marker("ie-1")).await;
    let seen = appends_until(&mut live, "Marker.").await;
    assert_eq!(
        seen,
        vec![
            (AppendKind::Instructions, QUESTION.to_string()),
            (AppendKind::Commentary, QUESTION.to_string()),
            (AppendKind::Thinking, "Marker.".to_string()),
        ],
        "the first interrupt of the turn acts, the second is a no-op"
    );

    // Another turn interrupts again; without a turn_id nothing is deduplicated.
    live.send(interrupt("ie-1", Some("ie-1#3"))).await;
    live.send(interrupt("ie-1", None)).await;
    live.send(interrupt("ie-1", None)).await;
    live.send(duplex_cell::advise_msg("ie-1", "context", "End.", None))
        .await;
    let later = appends_until(&mut live, "End.").await;
    let questions = later.iter().filter(|(_, c)| c == QUESTION).count();
    assert_eq!(questions, 6, "three interrupts act: {later:?}");
}

/// Pin 6 (N1): a horizon that waits while the model speaks lapses when the
/// answer goes out as a new spoken section -- after it, it would announce an
/// answer the caller has already heard.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_waiting_horizon_lapses_when_the_answer_is_spoken() {
    // The sections end only by the caller's cancel (as V3's pin 5): the fake
    // sends no audio, so no quiet of the connection's heuristic closes them.
    let mut live = boot_fake_timed(params(), QUIET_MS, CAP_MS * 8).await;
    let (mut client, _hello) = live.connect("session=ie-2&mode=auto").await;
    let _session = live.session().await;

    // The model speaks (the bridge); the horizon waits for the quiet moment.
    live.send(duplex_cell::advise_msg(
        "ie-2",
        "fact",
        "One moment, I am looking.",
        None,
    ))
    .await;
    appends_until(&mut live, "One moment, I am looking.").await;
    client
        .next_frame_of_type("speak_start", duplex_cell::marker())
        .await
        .expect("the bridge's section opens");
    live.send(advise(
        "ie-2",
        "horizon",
        json!({"messages": [], "section": "horizon", "seconds": 5,
               "way": "quick", "family": "web"}),
        json!({}),
    ))
    .await;
    // The answer goes out as a new spoken section; the model says it.
    live.send(duplex_cell::advise_msg(
        "ie-2",
        "fact",
        "It is 12 degrees.",
        None,
    ))
    .await;
    let answer = appends_until(&mut live, "It is 12 degrees.").await;
    assert!(
        !answer.iter().any(|(_, c)| c.contains("seconds")),
        "{answer:?}"
    );
    client
        .next_frame_of_type("speak_start", duplex_cell::marker())
        .await
        .expect("the answer's section opens");
    // The answer's section ends: the line is quiet -- the moment the waiting
    // horizon would have gone (`QUIET_MS` after the model's last words).
    client.cancel().await.expect("the caller cuts in");
    loop {
        let end = live.emission("speak_end").await;
        if end["header"]["reason"] == json!("cancelled") {
            break;
        }
    }
    live.send(marker("ie-2")).await;
    let after = appends_until(&mut live, "Marker.").await;
    assert_eq!(
        after,
        vec![(AppendKind::Thinking, "Marker.".to_string())],
        "the horizon lapsed with the answer"
    );
}
