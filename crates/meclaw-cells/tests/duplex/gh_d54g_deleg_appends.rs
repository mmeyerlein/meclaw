//! Wave d54g, strand deleg -- **an append into a running session never fails
//! silently at the provider's ceiling.**
//!
//! The provider takes about 500 tokens per append and refuses a longer one
//! whole (`invalid_value`, "Context append text must not exceed 500 tokens.").
//! The cell cut at 1 800 characters, which is more than 500 tokens of text
//! with numbers and names in it: three appends of one call were refused, the
//! model never got them, and all the colony saw was a `duplex_warning`
//! without a word about WHICH append it was.
//!
//! The promise, in five pins:
//! - the ceiling is counted in tokens (`advise_max_tokens` times
//!   `chars_per_token`, less a tenth), so an append over it leaves in parts
//!   as shipped, and an instance can move both numbers;
//! - the parts keep their order and end at sentence ends;
//! - the waiting-time instruction (`horizon`) is cut like every append;
//! - a refusal that names the append is reported on the error lane as
//!   `advise_append_refused`, with the section and the length of that append;
//! - a refused answer fails its delegation at once: the fallback sentence is
//!   said now, once.
//!
//! Played against the fixture's fake model inside a real duplex cell
//! (class topology, development rules § 11): every wait is the named hang
//! marker, every claim is about the order of events, none about time.

// harness_fold: #[path = "support/duplex_cell.rs"]
#[allow(unused_imports)]
use crate::duplex_cell; // harness_fold: mod duplex_cell;

use duplex_cell::{Live, advise_msg, boot_fake, duplex_params, header};
use meclaw_cells::voice::contract::{AppendKind, DuplexControl, DuplexEvent};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};

/// The fallback sentence this fixture configures.
const FALLBACK: &str = "I cannot look that up right now.";

/// The provider's own words for an append over its ceiling.
const REFUSAL: &str = "Context append text must not exceed 500 tokens.";

/// The ceiling as shipped: 500 tokens at 2.4 characters each, less a tenth.
const SHIPPED_LIMIT: usize = 1_080;

/// A `gpt_live` params document; `extra` lands in the `duplex` block, the way
/// an instance overrides it. The delegation grace is far beyond any run of
/// these tests, so a fallback that is said here was not said by the deadline.
fn params(extra: Value) -> Value {
    let mut duplex = json!({
        "provider": "gpt_live",
        "api_key": "test-key",
        "instructions": "A test line.",
        "sample_rate": 16_000,
        "tick_ms": 100,
        "delegation_grace_ms": 600_000,
        "delegation_fallback": FALLBACK,
    });
    if let (Some(obj), Some(more)) = (duplex.as_object_mut(), extra.as_object()) {
        for (k, v) in more {
            obj.insert(k.clone(), v.clone());
        }
    }
    duplex_params(json!({"default_mode": "auto", "duplex": duplex}))
}

/// One append the fake model was handed.
#[derive(Debug, Clone, PartialEq)]
struct Sent {
    kind: AppendKind,
    event_id: String,
    delegation_id: Option<String>,
    content: String,
}

/// The next append the model is handed.
async fn next_append(live: &mut Live) -> Sent {
    loop {
        let control = tokio::time::timeout(duplex_cell::marker(), live.controls.recv())
            .await
            .expect("HANG: no append reached the model")
            .expect("the fake is running");
        if let DuplexControl::Append {
            kind,
            event_id,
            delegation_id,
            content,
        } = control
        {
            return Sent {
                kind,
                event_id,
                delegation_id,
                content,
            };
        }
    }
}

/// Every append BEFORE the one carrying `marker`, in the order it came.
async fn appends_before(live: &mut Live, marker: &str) -> Vec<Sent> {
    let mut out = Vec::new();
    loop {
        let sent = next_append(live).await;
        if sent.content == marker {
            return out;
        }
        out.push(sent);
    }
}

/// A `context` advise as the marker that ends a collection: it travels the
/// same road as the appends before it, so what comes first was sent first.
fn marker(call: &str) -> Message {
    advise_msg(call, "context", "Marker.", None)
}

/// The refusal of one append, as the provider adapter hands it on.
fn refusal(event_id: &str) -> DuplexEvent {
    DuplexEvent::Warning {
        detail: REFUSAL.to_string(),
        provider_code: Some("invalid_value".to_string()),
        client_event_id: Some(event_id.to_string()),
    }
}

/// `count` numbered sentences in one paragraph, with numbers and a time in
/// each: the kind of text the refused appends were.
fn sentences(count: usize) -> String {
    (1..=count)
        .map(|n| format!("Entry {n} of the list starts at 9:30 in room 204 and ends at 11:45."))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The curator's horizon: numbers only, no words.
fn horizon(call: &str, seconds: u64) -> Message {
    let mut context = Map::new();
    context.insert("call_id".into(), json!(call));
    let mut hop = Map::new();
    hop.insert("route".into(), json!("in_advise"));
    hop.insert("section".into(), json!("horizon"));
    MessageBuilder::new(Path::new("/voice"))
        .context(context)
        .hop(hop)
        .body(Body::Inline(json!({
            "messages": [], "section": "horizon", "seconds": seconds,
            "way": "quick", "family": "web"
        })))
        .build()
}

/// One call on a cell of its own: `text` as a `fact`, and every append the
/// model was handed for it.
async fn fact_parts(extra: Value, call: &str, text: &str) -> Vec<Sent> {
    let mut live = boot_fake(params(extra)).await;
    let (_client, _hello) = live.connect(&format!("session={call}&mode=auto")).await;
    let _session = live.session().await;
    live.send(advise_msg(call, "fact", text, None)).await;
    live.send(marker(call)).await;
    appends_before(&mut live, "Marker.").await
}

/// Pin 1: as shipped, an append over 1 080 characters leaves in parts, none
/// over the ceiling and nothing lost -- 1 800 characters was the old cut, and
/// the provider refused such appends. And the ceiling is the instance's to
/// move: `advise_max_tokens` and `chars_per_token`, set as an override of
/// the instance's own `duplex` block, cut the same text smaller (the proof
/// that a set can switch the two new params).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_deleg_an_append_over_the_limit_goes_out_in_parts_by_default() {
    let text = sentences(24);
    let chars = text.chars().count();
    assert!(
        chars > SHIPPED_LIMIT && chars <= 1_800,
        "the fixture sits between the new ceiling and the old one: {chars}"
    );

    let shipped = fact_parts(json!({}), "parts-1", &text).await;
    assert!(
        shipped.len() >= 2,
        "over the ceiling is more than one append: {shipped:?}"
    );
    for part in &shipped {
        assert!(
            part.content.chars().count() <= SHIPPED_LIMIT,
            "no part over 500 tokens x 2.4 characters less a tenth: {}",
            part.content.chars().count()
        );
        assert_eq!(part.kind, AppendKind::Commentary, "a fact is said");
    }
    let joined = shipped
        .iter()
        .map(|p| p.content.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(joined, text, "nothing lost, nothing doubled");

    // The instance moves the ceiling: 100 tokens at 2 characters, less a
    // tenth, is 180 characters.
    let moved = fact_parts(
        json!({"advise_max_tokens": 100, "chars_per_token": 2.0}),
        "parts-2",
        &text,
    )
    .await;
    assert!(
        moved.len() > shipped.len(),
        "a lower ceiling is more parts: {} against {}",
        moved.len(),
        shipped.len()
    );
    for part in &moved {
        assert!(
            part.content.chars().count() <= 180,
            "the instance's ceiling holds: {}",
            part.content.chars().count()
        );
    }
}

/// Pin 2: the parts arrive in the order of the text, and each one ends where
/// a sentence ends -- as shipped, without `advise_split_chars`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_deleg_the_parts_keep_their_order_and_cut_at_sentence_ends() {
    let text = sentences(48);
    let parts = fact_parts(json!({}), "order-1", &text).await;
    assert!(parts.len() >= 3, "{} parts", parts.len());
    for part in &parts {
        assert!(
            part.content.ends_with("11:45."),
            "every part ends with a whole sentence: ...{:?}",
            part.content
                .chars()
                .rev()
                .take(24)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<String>()
        );
        assert!(
            part.content.starts_with("Entry "),
            "and begins with one: {:?}",
            part.content.chars().take(24).collect::<String>()
        );
    }
    let joined = parts
        .iter()
        .map(|p| p.content.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(
        joined, text,
        "the order of the text is the order of the parts"
    );
    let ids: std::collections::HashSet<&str> = parts.iter().map(|p| p.event_id.as_str()).collect();
    assert_eq!(
        ids.len(),
        parts.len(),
        "every part has an event id of its own"
    );
}

/// Pin 3: the waiting-time instruction goes through the same cut. On a quiet
/// line a `horizon_text` longer than the instance's ceiling leaves as
/// instructions in parts, in order, with the seconds filled in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_deleg_the_waiting_advise_is_split_like_any_append() {
    let horizon_text = "The answer takes about {seconds} seconds. Keep the talk going until \
                        then. Say that you are looking it up. Do not promise more than that. \
                        Ask nothing new while you wait.";
    let mut live = boot_fake(params(json!({
        "horizon_text": horizon_text,
        "advise_split_chars": 80,
    })))
    .await;
    let (_client, _hello) = live.connect("session=wait-1&mode=auto").await;
    let _session = live.session().await;
    live.send(horizon("wait-1", 4)).await;
    live.send(marker("wait-1")).await;
    let parts = appends_before(&mut live, "Marker.").await;
    assert!(
        parts.len() >= 2,
        "a waiting advise over the ceiling is cut: {parts:?}"
    );
    for part in &parts {
        assert_eq!(part.kind, AppendKind::Instructions, "{part:?}");
        assert!(part.content.chars().count() <= 80, "{part:?}");
        assert!(
            part.content.ends_with('.'),
            "cut at a sentence end: {part:?}"
        );
    }
    let joined = parts
        .iter()
        .map(|p| p.content.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(joined, horizon_text.replace("{seconds}", "4"));
}

/// Pin 4: a refusal that names the append is one emission on the error lane,
/// `advise_append_refused`, with the section the append travelled in, its
/// length, and the provider's code. A warning that names no append this cell
/// remembers is the `duplex_warning` it always was.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_deleg_a_refused_append_is_reported_with_section_and_length() {
    let mut live = boot_fake(params(json!({}))).await;
    let (_client, _hello) = live.connect("session=refused-1&mode=auto").await;
    let session = live.session().await;
    let note = "A note of forty-one characters, no more.";
    live.send(advise_msg("refused-1", "context", note, None))
        .await;
    let sent = next_append(&mut live).await;
    assert_eq!(sent.content, note);

    session.say(refusal(&sent.event_id)).await;
    let reported = live.emission("error").await;
    assert_eq!(
        header(&reported, "error_code"),
        Some(&json!("advise_append_refused")),
        "{reported}"
    );
    assert_eq!(
        reported["meta"],
        json!({
            "detail": REFUSAL,
            "provider_code": "invalid_value",
            "section": "context",
            "chars": note.chars().count(),
        }),
        "section, length and the provider's code, and no delegation where there was none: \
         {reported}"
    );

    session.say(refusal("an-event-of-somebody-else")).await;
    let foreign = live.emission("error").await;
    assert_eq!(
        header(&foreign, "error_code"),
        Some(&json!("duplex_warning")),
        "{foreign}"
    );
    assert_eq!(
        foreign["meta"],
        json!({"detail": REFUSAL, "provider_code": "invalid_value"}),
        "an event this cell did not send is not assigned to an append: {foreign}"
    );
}

/// Pin 5: the refused append was the answer to a delegation -- the delegation
/// has failed, and the fallback sentence is said on it NOW, not after the
/// grace. Once: a refusal of the fallback itself, and a second refusal of the
/// same answer, are reported and say nothing more.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_deleg_a_refused_answer_fails_its_delegation_at_once() {
    let mut live = boot_fake(params(json!({}))).await;
    let (_client, _hello) = live.connect("session=refused-2&mode=auto").await;
    let session = live.session().await;
    session
        .say(DuplexEvent::DelegationCreated {
            delegation_id: "dlg-r".to_string(),
            offset_ms: 0,
        })
        .await;
    let _ = live.emission("delegation").await;
    let answer = "The list has three entries for today.";
    live.send(advise_msg("refused-2", "fact", answer, Some("dlg-r")))
        .await;
    let sent = next_append(&mut live).await;
    assert_eq!(sent.content, answer);
    assert_eq!(sent.delegation_id.as_deref(), Some("dlg-r"));

    session.say(refusal(&sent.event_id)).await;
    let reported = live.emission("error").await;
    assert_eq!(
        header(&reported, "error_code"),
        Some(&json!("advise_append_refused")),
        "{reported}"
    );
    assert_eq!(
        reported["meta"],
        json!({
            "detail": REFUSAL,
            "provider_code": "invalid_value",
            "section": "fact",
            "chars": answer.chars().count(),
            "delegation_id": "dlg-r",
        }),
        "{reported}"
    );
    let fallback = next_append(&mut live).await;
    assert_eq!(
        (
            fallback.kind,
            fallback.delegation_id.as_deref(),
            fallback.content.as_str()
        ),
        (AppendKind::Commentary, Some("dlg-r"), FALLBACK),
        "the fallback on that delegation, at once"
    );

    // The fallback is refused as well, and the answer a second time: both are
    // reported, neither says another sentence. Each report is awaited before
    // the marker is sent, and the cell sends a fallback BEFORE it reports, so
    // whatever a refusal said would stand in front of the marker.
    session.say(refusal(&fallback.event_id)).await;
    let again = live.emission("error").await;
    assert_eq!(
        header(&again, "error_code"),
        Some(&json!("advise_append_refused")),
        "{again}"
    );
    assert_eq!(again["meta"]["chars"], json!(FALLBACK.chars().count()));
    session.say(refusal(&sent.event_id)).await;
    let twice = live.emission("error").await;
    assert_eq!(
        header(&twice, "error_code"),
        Some(&json!("advise_append_refused")),
        "{twice}"
    );
    live.send(marker("refused-2")).await;
    let more = appends_before(&mut live, "Marker.").await;
    assert!(
        more.is_empty(),
        "a failed delegation hears its fallback once: {more:?}"
    );
}
