//! #1407 -- the fact a tool said is not said a second time.
//!
//! WHY this file exists: with `fact_from_tool` the curator sends a listed
//! tool's `summary` as the turn's fact the moment the result is in
//! (`hop.fact_source` "tool"); the second model round still writes its own
//! `fact` into the sidecar 2.8 to 6.7 s later (d53 battery round 1). The
//! voice says the first and drops every later `fact` of that `hop.turn_id`
//! (`fact_deduped`) -- no append, no refusal. Without `fact_source` nothing
//! is deduplicated (as before).

// harness_fold: #[path = "support/duplex_cell.rs"]
#[allow(unused_imports)]
use crate::duplex_cell; // harness_fold: mod duplex_cell;

use duplex_cell::{Live, boot_fake, duplex_params};
use meclaw_cells::voice::contract::{AppendKind, DuplexControl};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};

const TOOL: &str = "Tomorrow in Berlin: cloudy, 4 to 9 degrees.";
const SIDECAR: &str = "Tomorrow it stays cloudy, up to 9 degrees.";

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
        },
    }))
}

/// A `fact` as the splitter (or the curator) hands it on: the section's value
/// under `payload`, the hop keys the test names.
fn fact(call: &str, text: &str, hop_extra: Value) -> Message {
    let mut context = Map::new();
    context.insert("call_id".into(), json!(call));
    let mut hop = Map::new();
    hop.insert("route".into(), json!("in_advise"));
    hop.insert("section".into(), json!("fact"));
    if let Value::Object(extra) = hop_extra {
        hop.extend(extra);
    }
    MessageBuilder::new(Path::new("/voice"))
        .context(context)
        .hop(hop)
        .body(Body::Inline(
            json!({"messages": [], "section": "fact", "payload": text}),
        ))
        .build()
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

/// The tool's fact acts; the sidecar's fact of the same turn is a no-op.
/// Another turn, and facts without a tool source, act as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_sidecar_fact_after_a_tool_fact_is_not_sent_twice() {
    let mut live = boot_fake(params()).await;
    let (_client, _hello) = live.connect("session=tf-1&mode=auto").await;
    let _session = live.session().await;

    live.send(fact(
        "tf-1",
        TOOL,
        json!({"turn_id": "tf-1#2", "fact_source": "tool"}),
    ))
    .await;
    live.send(fact("tf-1", SIDECAR, json!({"turn_id": "tf-1#2"})))
        .await;
    live.send(duplex_cell::advise_msg("tf-1", "context", "Marker.", None))
        .await;
    let seen = appends_until(&mut live, "Marker.").await;
    assert_eq!(
        seen,
        vec![
            (AppendKind::Commentary, TOOL.to_string()),
            (AppendKind::Thinking, "Marker.".to_string()),
        ],
        "the tool's fact is said, the sidecar's of the same turn is not"
    );

    // Without a tool fact before them, two facts of one turn both act, and a
    // fact of another turn acts after a tool fact.
    live.send(fact("tf-1", SIDECAR, json!({"turn_id": "tf-1#3"})))
        .await;
    live.send(fact("tf-1", SIDECAR, json!({"turn_id": "tf-1#3"})))
        .await;
    live.send(fact("tf-1", TOOL, json!({"turn_id": "tf-1#4"})))
        .await;
    live.send(duplex_cell::advise_msg("tf-1", "context", "End.", None))
        .await;
    let later = appends_until(&mut live, "End.").await;
    let spoken = later
        .iter()
        .filter(|(k, _)| *k == AppendKind::Commentary)
        .count();
    assert_eq!(spoken, 3, "no tool source, no dedupe: {later:?}");
}
