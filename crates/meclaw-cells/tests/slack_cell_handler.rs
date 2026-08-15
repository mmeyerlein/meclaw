//! P12 S16/S17/S20: the Slack handler — dedup, thread ownership, emission and
//! the inbound reply leg.
//!
//! Thread ownership (D-5) is the rule that makes a bot liveable in a shared
//! channel: it answers when addressed, keeps following the thread it opened,
//! and stays silent in everyone else's conversations. Each of those three is a
//! separate test below, and the third one is the important one — a bot that
//! joins threads it was never invited into is the failure mode that gets a bot
//! removed from a workspace.

use meclaw_cells::proxy::slack::cell::SlackCell;
use meclaw_cells::proxy::slack::client::SlackClient;
use meclaw_cells::proxy::slack::db::setup_slack_schema;
use meclaw_cells::proxy::slack::io::SlackInbound;
use meclaw_cells::proxy::slack::params::SlackParams;
use meclaw_cells::proxy::slack::wire::{SlackEventKind, SlackUserEvent};
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::serde_json::Map;
use meclaw_core::{
    Body, CellEmission, Headers, MessageBuilder, OriginSink, OutputSink, Path, Uuid,
    serde_json::{Value, json},
};
use meclaw_testing::mock_slack::MockSlack;
use tokio::sync::mpsc;

fn params_for(base_url: &str) -> SlackParams {
    SlackParams::parse(&json!({
        "app_token": "xapp-x",
        "bot_token": "xoxb-x",
        "emit_to": "/agent",
        "base_url": base_url
    }))
    .expect("params parse")
}

fn cell_for(base_url: &str) -> SlackCell {
    let params = params_for(base_url);
    let client = SlackClient::new(&params).expect("client");
    SlackCell::new(&params, client)
}

fn db() -> DbConn {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    setup_slack_schema(&conn).expect("schema");
    DbConn::wrap(conn, None)
}

fn event(
    kind: SlackEventKind,
    channel: &str,
    channel_type: &str,
    ts: &str,
    thread_ts: Option<&str>,
) -> SlackUserEvent {
    SlackUserEvent {
        kind,
        channel: channel.to_string(),
        channel_type: Some(channel_type.to_string()),
        user: Some("U_HUMAN".to_string()),
        text: "hello there".to_string(),
        ts: ts.to_string(),
        thread_ts: thread_ts.map(String::from),
        event_ts: Some(ts.replace('.', "")),
    }
}

fn inbound(envelope_id: &str, event: SlackUserEvent) -> SlackInbound {
    SlackInbound {
        envelope_id: envelope_id.to_string(),
        event,
    }
}

/// Drains whatever the cell emitted through the origin sink.
async fn emitted(rx: &mut mpsc::Receiver<CellEmission>) -> Vec<CellEmission> {
    let mut out = Vec::new();
    while let Ok(em) = rx.try_recv() {
        out.push(em);
    }
    let _ = &mut out;
    out
}

fn header_of(em: &CellEmission) -> Map<String, Value> {
    em.content
        .get("header")
        .and_then(|h| h.as_object())
        .cloned()
        .unwrap_or_default()
}

// ------------------------------------------------------------- thread rules

/// D-5 line 1: a mention in the channel root OPENS a thread — the reply is
/// addressed to the mention's own ts. This is the only way ownership becomes
/// well-defined at all, and it keeps multi-bot channels readable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mention_in_the_channel_root_opens_a_thread() {
    let server = MockSlack::start().await.expect("fake");
    let mut cell = cell_for(&server.base_url());
    let mut db = db();
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let sink = OriginSink::new(tx, Path::new("/slack"), 64);

    cell.handle_event(
        inbound(
            "e1",
            event(SlackEventKind::Mention, "C1", "channel", "111.000111", None),
        ),
        &sink,
        &mut db,
    )
    .await;

    let ems = emitted(&mut rx).await;
    assert_eq!(ems.len(), 1, "the mention must be emitted");
    let h = header_of(&ems[0]);
    assert_eq!(
        h.get("chat_id").and_then(|v| v.as_str()),
        Some("C1:111.000111"),
        "the composite must name the thread this mention just opened"
    );
    assert_eq!(h.get("platform").and_then(|v| v.as_str()), Some("slack"));
    assert_eq!(
        h.get("slack_thread_ts").and_then(|v| v.as_str()),
        Some("111.000111")
    );
}

/// A mention inside an existing thread stays in that thread rather than
/// starting a nested one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mention_inside_a_thread_stays_in_that_thread() {
    let server = MockSlack::start().await.expect("fake");
    let mut cell = cell_for(&server.base_url());
    let mut db = db();
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let sink = OriginSink::new(tx, Path::new("/slack"), 64);

    cell.handle_event(
        inbound(
            "e1",
            event(
                SlackEventKind::Mention,
                "C1",
                "channel",
                "222.000222",
                Some("100.000100"),
            ),
        ),
        &sink,
        &mut db,
    )
    .await;

    let ems = emitted(&mut rx).await;
    assert_eq!(
        header_of(&ems[0]).get("chat_id").and_then(|v| v.as_str()),
        Some("C1:100.000100"),
        "the reply belongs to the parent thread, not to the mention's own ts"
    );
}

/// A DM has no thread. Inventing one would be wrong and visible to the user.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_direct_message_has_no_thread() {
    let server = MockSlack::start().await.expect("fake");
    let mut cell = cell_for(&server.base_url());
    let mut db = db();
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let sink = OriginSink::new(tx, Path::new("/slack"), 64);

    cell.handle_event(
        inbound(
            "e1",
            event(SlackEventKind::Message, "D1", "im", "333.000333", None),
        ),
        &sink,
        &mut db,
    )
    .await;

    let ems = emitted(&mut rx).await;
    assert_eq!(ems.len(), 1, "a DM is always for us");
    let h = header_of(&ems[0]);
    assert_eq!(h.get("chat_id").and_then(|v| v.as_str()), Some("D1"));
    assert!(
        h.get("slack_thread_ts").is_none(),
        "a DM reply must not carry a thread"
    );
}

/// R5, the anti-jump-in rule. An ordinary channel message that neither mentions
/// the bot nor belongs to a thread it owns is NOT ours. This is what keeps a
/// bot from barging into every conversation in a shared channel.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unaddressed_channel_message_is_ignored() {
    let server = MockSlack::start().await.expect("fake");
    let mut cell = cell_for(&server.base_url());
    let mut db = db();
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let sink = OriginSink::new(tx, Path::new("/slack"), 64);

    cell.handle_event(
        inbound(
            "e1",
            event(SlackEventKind::Message, "C1", "channel", "444.000444", None),
        ),
        &sink,
        &mut db,
    )
    .await;

    assert!(
        emitted(&mut rx).await.is_empty(),
        "an unaddressed channel message must not reach the agent"
    );
}

/// A message in someone else's thread is equally not ours.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_in_a_foreign_thread_is_ignored() {
    let server = MockSlack::start().await.expect("fake");
    let mut cell = cell_for(&server.base_url());
    let mut db = db();
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let sink = OriginSink::new(tx, Path::new("/slack"), 64);

    cell.handle_event(
        inbound(
            "e1",
            event(
                SlackEventKind::Message,
                "C1",
                "channel",
                "555.000555",
                Some("999.000999"),
            ),
        ),
        &sink,
        &mut db,
    )
    .await;

    assert!(
        emitted(&mut rx).await.is_empty(),
        "a thread we never opened is none of our business"
    );
}

/// The positive half of R5: once the bot opened a thread, follow-ups in that
/// thread need no further mention. This is what makes a conversation feel like
/// a conversation instead of a series of commands.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_follow_up_in_our_own_thread_is_processed() {
    let server = MockSlack::start().await.expect("fake");
    let mut cell = cell_for(&server.base_url());
    let mut db = db();
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let sink = OriginSink::new(tx, Path::new("/slack"), 64);

    // The mention opens (and claims) the thread at ts 600.000600.
    cell.handle_event(
        inbound(
            "e1",
            event(SlackEventKind::Mention, "C1", "channel", "600.000600", None),
        ),
        &sink,
        &mut db,
    )
    .await;
    let _ = emitted(&mut rx).await;

    // A later, unmentioned message in that same thread belongs to us.
    cell.handle_event(
        inbound(
            "e2",
            event(
                SlackEventKind::Message,
                "C1",
                "channel",
                "601.000601",
                Some("600.000600"),
            ),
        ),
        &sink,
        &mut db,
    )
    .await;

    let ems = emitted(&mut rx).await;
    assert_eq!(
        ems.len(),
        1,
        "our own thread keeps flowing without a mention"
    );
    assert_eq!(
        header_of(&ems[0]).get("chat_id").and_then(|v| v.as_str()),
        Some("C1:600.000600")
    );
}

/// Slack redelivers un-acked envelopes. Without dedup that redelivery would put
/// the user's message into the agent tree a second time.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_redelivered_envelope_is_emitted_only_once() {
    let server = MockSlack::start().await.expect("fake");
    let mut cell = cell_for(&server.base_url());
    let mut db = db();
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let sink = OriginSink::new(tx, Path::new("/slack"), 64);

    let ev = event(SlackEventKind::Mention, "C1", "channel", "700.000700", None);
    cell.handle_event(inbound("same-envelope", ev.clone()), &sink, &mut db)
        .await;
    cell.handle_event(inbound("same-envelope", ev), &sink, &mut db)
        .await;

    assert_eq!(
        emitted(&mut rx).await.len(),
        1,
        "the redelivery must be swallowed"
    );
}

/// LIVE-DERIVED (2026-08-09): Slack delivers ONE user message TWICE to the
/// mentioned bot — once as `app_mention` and once as `message`, with the SAME
/// `ts` but DIFFERENT envelope ids. Observed against the real API:
///
/// ```text
/// [bot-a] EVENT kind=Mention ts=1786254534.594009 text="<@U0BOTA> ping"
/// [bot-a] EVENT kind=Message ts=1786254534.594009 text="<@U0BOTA> ping"
/// ```
///
/// Envelope dedup does NOT cover this: two envelopes, two ids. What keeps the
/// user's message from entering the agent tree twice is R5 — the message copy
/// carries no `thread_ts` and belongs to no owned thread, so it is not ours.
/// Without that rule every mention would be answered twice.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_message_arriving_as_mention_and_message_emits_once() {
    let server = MockSlack::start().await.expect("fake");
    let mut cell = cell_for(&server.base_url());
    let mut db = db();
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let sink = OriginSink::new(tx, Path::new("/slack"), 64);

    let ts = "1786254534.594009";
    // Arrives first as the mention…
    cell.handle_event(
        inbound(
            "env-mention",
            event(SlackEventKind::Mention, "C1", "channel", ts, None),
        ),
        &sink,
        &mut db,
    )
    .await;
    // …then as the plain channel message, same ts, different envelope.
    cell.handle_event(
        inbound(
            "env-message",
            event(SlackEventKind::Message, "C1", "channel", ts, None),
        ),
        &sink,
        &mut db,
    )
    .await;

    let ems = emitted(&mut rx).await;
    assert_eq!(
        ems.len(),
        1,
        "one user message must produce one emission, not two: {ems:?}"
    );
    assert_eq!(
        header_of(&ems[0]).get("chat_id").and_then(|v| v.as_str()),
        Some(format!("C1:{ts}").as_str())
    );
}

/// Same pair, opposite arrival order. Slack promises no ordering between the
/// two deliveries, so the message copy may well land first — and it must still
/// be the mention that produces the single emission.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_duplicate_pair_emits_once_regardless_of_arrival_order() {
    let server = MockSlack::start().await.expect("fake");
    let mut cell = cell_for(&server.base_url());
    let mut db = db();
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let sink = OriginSink::new(tx, Path::new("/slack"), 64);

    let ts = "1786254540.867009";
    cell.handle_event(
        inbound(
            "env-message",
            event(SlackEventKind::Message, "C1", "channel", ts, None),
        ),
        &sink,
        &mut db,
    )
    .await;
    cell.handle_event(
        inbound(
            "env-mention",
            event(SlackEventKind::Mention, "C1", "channel", ts, None),
        ),
        &sink,
        &mut db,
    )
    .await;

    let ems = emitted(&mut rx).await;
    assert_eq!(ems.len(), 1, "still exactly one emission: {ems:?}");
    assert_eq!(
        header_of(&ems[0]).get("chat_id").and_then(|v| v.as_str()),
        Some(format!("C1:{ts}").as_str()),
        "the mention is the copy that must win — it is the one that opens the thread"
    );
}

/// The other half of the live finding: a bot only ever receives `app_mention`
/// for ITSELF, but `message.channels` for EVERYTHING. So the mention addressed
/// to another bot arrives here as a plain message and must be ignored.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mention_addressed_to_another_bot_arrives_as_message_and_is_ignored() {
    let server = MockSlack::start().await.expect("fake");
    let mut cell = cell_for(&server.base_url());
    let mut db = db();
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let sink = OriginSink::new(tx, Path::new("/slack"), 64);

    // Text mentions a DIFFERENT bot; we only see the message copy.
    let mut ev = event(
        SlackEventKind::Message,
        "C1",
        "channel",
        "1786254540.867009",
        None,
    );
    ev.text = "<@U0BP252GQRF> ping".to_string();
    cell.handle_event(inbound("env-other", ev), &sink, &mut db)
        .await;

    assert!(
        emitted(&mut rx).await.is_empty(),
        "another bot's mention is not our business — Slack routes app_mention, \
         we must not second-guess it by scanning text"
    );
}

// -------------------------------------------------------------- inbound leg

fn context_with(chat_id: &str) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("chat_id".into(), json!(chat_id));
    m
}

fn out_sink(tx: mpsc::Sender<CellEmission>) -> OutputSink {
    OutputSink::new(
        tx,
        Path::new("/slack"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        Headers::new(),
        None,
    )
}

/// The reply leg: the composite chat_id is decoded back into channel plus
/// thread, and the post lands in that thread.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_reply_is_posted_into_the_addressed_thread() {
    let server = MockSlack::start().await.expect("fake");
    let mut cell = cell_for(&server.base_url());
    let mut db = db();
    let (tx, _rx) = mpsc::channel::<CellEmission>(8);
    let sink = out_sink(tx);
    let (rc_tx, _rc_rx) = mpsc::channel(8);

    let msg = MessageBuilder::new(Path::new("/slack"))
        .reply_to(Path::new("/sender"))
        .context(context_with("C1:100.000100"))
        .body(Body::Inline(json!({
            "messages": [
                { "origin": "user", "type": "text", "text": "ignored" },
                { "origin": "assistant", "type": "text", "text": "the answer" }
            ]
        })))
        .build();

    cell.handle(msg, &sink, &mut db, &rc_tx).await;

    let posts = server.posts().await;
    assert_eq!(posts.len(), 1, "the reply must be posted");
    assert_eq!(posts[0].channel(), Some("C1"));
    assert_eq!(posts[0].text(), Some("the answer"));
    assert_eq!(posts[0].thread_ts(), Some("100.000100"));
}

/// A DM chat_id has no thread part, and the post must not invent one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reply_to_a_dm_carries_no_thread() {
    let server = MockSlack::start().await.expect("fake");
    let mut cell = cell_for(&server.base_url());
    let mut db = db();
    let (tx, _rx) = mpsc::channel::<CellEmission>(8);
    let sink = out_sink(tx);
    let (rc_tx, _rc_rx) = mpsc::channel(8);

    let msg = MessageBuilder::new(Path::new("/slack"))
        .reply_to(Path::new("/sender"))
        .context(context_with("D1"))
        .body(Body::Inline(json!({
            "messages": [{ "origin": "assistant", "type": "text", "text": "dm answer" }]
        })))
        .build();

    cell.handle(msg, &sink, &mut db, &rc_tx).await;
    let posts = server.posts().await;
    assert_eq!(posts[0].channel(), Some("D1"));
    assert!(posts[0].thread_ts().is_none());
}

/// Missing chat_id is the fail-loud case for a forgotten promotion edge. It
/// must produce a visible error rather than a silent drop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_missing_chat_id_emits_a_loud_error() {
    let server = MockSlack::start().await.expect("fake");
    let mut cell = cell_for(&server.base_url());
    let mut db = db();
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let sink = out_sink(tx);
    let (rc_tx, _rc_rx) = mpsc::channel(8);

    let msg = MessageBuilder::new(Path::new("/slack"))
        .reply_to(Path::new("/sender"))
        .body(Body::Inline(json!({
            "messages": [{ "origin": "assistant", "type": "text", "text": "x" }]
        })))
        .build();

    cell.handle(msg, &sink, &mut db, &rc_tx).await;

    let ems = emitted(&mut rx).await;
    assert_eq!(ems.len(), 1, "the failure must be announced, not swallowed");
    assert_eq!(
        header_of(&ems[0])
            .get("error_code")
            .and_then(|v| v.as_str()),
        Some("missing_chat_id")
    );
    assert!(
        server.posts().await.is_empty(),
        "nothing may be posted without an address"
    );
}

/// Pure-sink discipline: a successful post emits nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_successful_post_emits_nothing() {
    let server = MockSlack::start().await.expect("fake");
    let mut cell = cell_for(&server.base_url());
    let mut db = db();
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let sink = out_sink(tx);
    let (rc_tx, _rc_rx) = mpsc::channel(8);

    let msg = MessageBuilder::new(Path::new("/slack"))
        .reply_to(Path::new("/sender"))
        .context(context_with("C1"))
        .body(Body::Inline(json!({
            "messages": [{ "origin": "assistant", "type": "text", "text": "ok" }]
        })))
        .build();

    cell.handle(msg, &sink, &mut db, &rc_tx).await;
    assert!(
        emitted(&mut rx).await.is_empty(),
        "a pure sink stays silent on success"
    );
}
