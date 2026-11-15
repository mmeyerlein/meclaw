//! GH #1530: a sign of life for a Slack turn -- a reaction on the person's
//! message while the turn runs.
//!
//! The Events API has no bot typing, and `assistant.threads.setStatus` only
//! works in assistant threads, so the Slack connector shows a running turn the
//! way a person in the channel would: a reaction (`reactions.add`) on the
//! message it is working on, taken away (`reactions.remove`) once the turn is
//! answered. Nothing is posted into the conversation for it. Off by default:
//! a set switches it on per instance with `params.typing_reaction` (the emoji
//! name); `params.typing_max_ms` is the ceiling, as for Telegram.
//!
//! What this file pins (the keeper of `proxy::typing`, as for Telegram,
//! GH #515 / #1504):
//!
//! 1. the incoming turn sets the reaction on the person's own message (its
//!    `ts`, not the thread's), and the first message of the turn without
//!    `hop.interim` takes it away, after its post;
//! 2. an interim leaves it standing: the keeper still reaches its own ceiling
//!    behind an interim;
//! 3. the ceiling takes it away when no answer ever comes;
//! 4. an answer marked `hop.repeat` (GH #1505: nothing is posted) takes it
//!    away like a posted answer;
//! 5. a reaction call that hangs or is refused never holds the answer up and
//!    is no failure of the turn;
//! 6. the shipped template leaves it off, and a set's instance override
//!    passes the door and switches it on;
//! 7. an add that failed at its own timeout may still have gone through, so
//!    the end of the turn takes the reaction away all the same;
//! 8. a restart between the two ends of a turn leaves no reaction behind: the
//!    connector remembers each reaction in its own `cell.db` and the next
//!    start takes every remembered one away;
//! 9. a row goes only once `reactions.remove` is confirmed (OR-d54g.acktyp.2):
//!    the keeper reports the removal on the cell's report lane and the handler,
//!    which owns the `cell.db`, forgets the row. A removal that fails at the
//!    turn's end keeps its row and is tried again (after Slack's `Retry-After`
//!    when it names one); one that fails at the start keeps its row for the
//!    next start;
//! 10. with the bot token from the vault -- the shipped form -- the start's
//!     removal runs once the sealed token arrived, and its report reaches the
//!     handler over the I/O half.
//!
//! Every claim is an event, none a window (as `gh_d54g_ack_typing.rs`): the
//! Web API is the test's own and hands each request over with its connection
//! open, so the test decides when the connector's next step may happen.

// harness_fold: #[path = "support/gh1059_grants.rs"]
#[allow(unused_imports)]
use crate::gh1059_grants; // harness_fold: mod gh1059_grants;

use gh1059_grants::{delivery, message, next_request};
use meclaw_cells::proxy::slack::cell::SlackCell;
use meclaw_cells::proxy::slack::client::SlackClient;
use meclaw_cells::proxy::slack::db::setup_slack_schema;
use meclaw_cells::proxy::slack::io::{SlackEvent, SlackInbound};
use meclaw_cells::proxy::slack::params::SlackParams;
use meclaw_cells::proxy::slack::wire::{SlackEventKind, SlackUserEvent};
use meclaw_colony::mutation::subtree::{check_override_params, parse_subtree};
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::serde_json::{Map, Value};
use meclaw_core::{
    Body, CellEmission, Message, MessageBuilder, OriginSink, OutputSink, Path, Uuid,
    serde_json::json,
};
use meclaw_testing::deadline;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

/// The emoji the tests switch on. Any name; the connector passes it through.
const REACTION: &str = "hourglass_flowing_sand";

/// The thread the person writes in, and the person's own message inside it.
/// Distinct on purpose: the reaction belongs on the message, not on the thread.
const THREAD_TS: &str = "1700000000.000100";
const MESSAGE_TS: &str = "1700000000.000200";

// One channel per test: the keeper's log lines name the chat, and the
// recorder is shared by the whole binary.
const ANSWERED_CHANNEL: &str = "C0ACKTYP01";
const INTERIM_CHANNEL: &str = "C0ACKTYP02";
const CEILING_CHANNEL: &str = "C0ACKTYP03";
const REPEAT_CHANNEL: &str = "C0ACKTYP04";
const HANGING_CHANNEL: &str = "C0ACKTYP05";
const REFUSED_CHANNEL: &str = "C0ACKTYP06";
const OFF_CHANNEL: &str = "C0ACKTYP07";
const OVERRIDE_CHANNEL: &str = "C0ACKTYP08";
const LATE_CHANNEL: &str = "C0ACKTYP09";
const RESTART_CHANNEL: &str = "C0ACKTYP10";
const RETRIED_CHANNEL: &str = "C0ACKTYP11";
const START_FAILED_CHANNEL: &str = "C0ACKTYP12";
const VAULT_CHANNEL: &str = "C0ACKTYP13";

fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Whether the Slack template is in this tree. `slack-agent` does not travel
/// with the public export, and every test here reads its shipped `proxy`
/// params, so in a tree without it each test returns before it starts (the
/// GH #49 presence guard) instead of judging something that is not there.
fn slack_template_in_tree() -> bool {
    let present = core_root().join("templates/slack-agent").exists();
    if !present {
        eprintln!("skipped: templates/slack-agent is not in this tree");
    }
    present
}

/// The shipped `proxy` cell of the Slack template.
fn shipped_proxy_config() -> Value {
    let p = core_root().join("templates/slack-agent/proxy/config.json");
    let raw = std::fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("no such template file {}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The shipped params pointed at the test's Web API: literal tokens instead
/// of the grants (an empty grant id is none), every operation timeout the
/// product wait so a request the test holds is held, never cut short, and
/// `extra` laid over them the way a set's `override_params` does.
fn shipped_params_against(addr: std::net::SocketAddr, extra: &[(&str, Value)]) -> Value {
    let mut params = shipped_proxy_config()["params"].clone();
    let wait = deadline::product_wait_ms();
    params["app_token_grant_id"] = json!("");
    params["bot_token_grant_id"] = json!("");
    params["app_token"] = json!("xapp-test");
    params["bot_token"] = json!("xoxb-test");
    params["emit_to"] = json!("/agent");
    params["base_url"] = json!(format!("http://{addr}"));
    params["connect_timeout_ms"] = json!(wait);
    params["send_timeout_ms"] = json!(wait);
    for (k, v) in extra {
        params[*k] = v.clone();
    }
    params
}

fn cell_from(params: &Value) -> SlackCell {
    let parsed = SlackParams::parse(params).expect("params parse");
    let client = SlackClient::new(&parsed).expect("client");
    SlackCell::new(&parsed, client)
}

/// A cell with the reaction switched on and the given ceiling.
fn cell_on(addr: std::net::SocketAddr, typing_max_ms: u64) -> SlackCell {
    cell_from(&shipped_params_against(
        addr,
        &[
            ("typing_reaction", json!(REACTION)),
            ("typing_max_ms", json!(typing_max_ms)),
        ],
    ))
}

fn db() -> DbConn {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    setup_slack_schema(&conn).expect("schema");
    DbConn::wrap(conn, None)
}

fn out_sink(tx: mpsc::Sender<CellEmission>) -> OutputSink {
    OutputSink::new(
        tx,
        Path::new("/slack"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        meclaw_core::Headers::new(),
        None,
    )
}

/// The person's message: a mention inside a thread the bot already owns, so
/// its own `ts` differs from the thread's.
fn person_writes(envelope_id: &str, channel: &str) -> SlackEvent {
    SlackInbound {
        envelope_id: envelope_id.to_string(),
        event: SlackUserEvent {
            kind: SlackEventKind::Mention,
            channel: channel.to_string(),
            channel_type: Some("channel".to_string()),
            user: Some("U_PERSON".to_string()),
            text: "<@U0BOT> look that up for me".to_string(),
            ts: MESSAGE_TS.to_string(),
            thread_ts: Some(THREAD_TS.to_string()),
            event_ts: Some(MESSAGE_TS.replace('.', "")),
        },
    }
    .into()
}

/// A finished message of the turn as it reaches the connector; `hop` marks
/// it `interim` or `repeat`.
fn outgoing(channel: &str, text: &str, mark: Option<&str>) -> Message {
    let mut context = Map::new();
    context.insert("chat_id".into(), json!(format!("{channel}:{THREAD_TS}")));
    let mut hop: Map<String, Value> = Map::new();
    hop.insert("route".into(), json!("answer"));
    if let Some(m) = mark {
        hop.insert(m.into(), json!("1"));
    }
    MessageBuilder::new(Path::new("/slack"))
        .reply_to(Path::new("/sender"))
        .context(context)
        .hop(hop)
        .body(Body::Inline(json!({
            "messages": [{ "origin": "assistant", "type": "text", "text": text }]
        })))
        .build()
}

/// One request the test's Web API took. Its connection stays open until the
/// test answers it.
struct Taken {
    path: String,
    /// The `Authorization` header, as sent.
    authorization: Option<String>,
    body: Value,
    stream: TcpStream,
}

impl Taken {
    /// Answers with `body` (Slack answers HTTP 200 also for its refusals),
    /// and the connection closes behind it. A client that already gave the
    /// request up is no failure here.
    async fn answer_with(mut self, body: Value) {
        let body = body.to_string();
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        let _ = self.stream.write_all(head.as_bytes()).await;
        let _ = self.stream.write_all(body.as_bytes()).await;
        let _ = self.stream.shutdown().await;
    }

    async fn ok(self) {
        self.answer_with(json!({"ok": true})).await;
    }

    /// Answers HTTP 429 the way Slack rate-limits a method: `Retry-After`
    /// names the seconds to wait.
    async fn rate_limited(mut self, retry_after_secs: u64) {
        let body = json!({"ok": false, "error": "ratelimited"}).to_string();
        let head = format!(
            "HTTP/1.1 429 Too Many Requests\r\nretry-after: {retry_after_secs}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        let _ = self.stream.write_all(head.as_bytes()).await;
        let _ = self.stream.write_all(body.as_bytes()).await;
        let _ = self.stream.shutdown().await;
    }

    /// Resolves once the client gave the unanswered request up: its end of
    /// the connection closed (EOF or reset). The request stays here, so the
    /// test can still answer it afterwards -- the call "went through late".
    async fn given_up(&mut self) {
        let mut b = [0u8; 64];
        loop {
            match self.stream.read(&mut b).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
        }
    }

    /// The Web API method this request called.
    fn method(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or_default()
    }

    /// The request is `reactions.<verb>` of [`REACTION`] on the person's
    /// message in `channel`.
    fn assert_reaction(&self, verb: &str, channel: &str, claim: &str) {
        assert_eq!(
            self.method(),
            format!("reactions.{verb}"),
            "{claim}: expected reactions.{verb}, got {}",
            self.path
        );
        assert_eq!(self.body["channel"], channel, "{claim}");
        assert_eq!(
            self.body["timestamp"], MESSAGE_TS,
            "{claim}: the reaction belongs on the person's message, not on the thread"
        );
        assert_eq!(self.body["name"], REACTION, "{claim}");
    }

    /// The request is a `chat.postMessage` of `text` into the thread.
    fn assert_post(&self, channel: &str, text: &str, claim: &str) {
        assert_eq!(
            self.method(),
            "chat.postMessage",
            "{claim}: expected chat.postMessage, got {}",
            self.path
        );
        assert_eq!(self.body["channel"], channel, "{claim}");
        assert_eq!(self.body["thread_ts"], THREAD_TS, "{claim}");
        assert_eq!(self.body["text"], text, "{claim}");
    }
}

/// Reads one HTTP/1.1 request (head and `content-length` body) off `s`.
async fn take(mut s: TcpStream) -> Option<Taken> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    let end = loop {
        let n = s.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..end]).to_string();
    let path = head.split_whitespace().nth(1)?.to_string();
    let len = head
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            if k.trim().eq_ignore_ascii_case("content-length") {
                v.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);
    let authorization = head.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case("authorization")
            .then(|| v.trim().to_string())
    });
    while buf.len() < end + len {
        let n = s.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let body = serde_json::from_slice(&buf[end..end + len]).unwrap_or(Value::Null);
    Some(Taken {
        path,
        authorization,
        body,
        stream: s,
    })
}

/// The test's own Web API: every request arrives on the channel, in the order
/// the client sent it, and waits there for the test.
async fn web_api() -> (std::net::SocketAddr, mpsc::UnboundedReceiver<Taken>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok((s, _)) = listener.accept().await {
            let tx = tx.clone();
            tokio::spawn(async move {
                if let Some(t) = take(s).await {
                    let _ = tx.send(t);
                }
            });
        }
    });
    (addr, rx)
}

/// The next request the Web API took, under the hang mark.
async fn next(api: &mut mpsc::UnboundedReceiver<Taken>, what: &str) -> Taken {
    deadline::hang(what, api.recv())
        .await
        .unwrap_or_else(|| panic!("{what}: the Web API ended"))
}

/// The incoming turn: handled, emitted once, and the sink drained.
async fn incoming(cell: &mut SlackCell, db: &mut DbConn, envelope_id: &str, channel: &str) {
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let origin = OriginSink::new(tx, Path::new("/slack"), 64);
    cell.handle_event(person_writes(envelope_id, channel), &origin, db)
        .await;
    assert!(rx.try_recv().is_ok(), "the person's turn is emitted");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_ack_typing_slack_a_reaction_stands_from_the_turn_to_its_answer() {
    if !slack_template_in_tree() {
        return;
    }
    let (addr, mut api) = web_api().await;
    let mut cell = cell_on(addr, deadline::product_wait_ms());
    let mut db = db();
    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
    let sink = out_sink(out_tx);
    let (rc_tx, _rc_rx) = mpsc::channel(8);

    // 1. The incoming turn sets the reaction on the person's message.
    incoming(&mut cell, &mut db, "e-answered", ANSWERED_CHANNEL).await;
    let add = next(
        &mut api,
        "GH #1530: the incoming turn sets a reaction on the person's message (today the \
         connector shows nothing while the turn runs)",
    )
    .await;
    add.assert_reaction("add", ANSWERED_CHANNEL, "the reaction of the incoming turn");
    add.ok().await;

    // 2. The answer goes out, and after it the reaction goes away.
    let answer = outgoing(ANSWERED_CHANNEL, "here is what I found", None);
    tokio::join!(cell.handle(answer, &sink, &mut db, &rc_tx), async {
        let post = next(&mut api, "the answer's chat.postMessage reaches the API").await;
        post.assert_post(ANSWERED_CHANNEL, "here is what I found", "the answer");
        post.ok().await;
    });
    let remove = next(
        &mut api,
        "GH #1530: the first message without interim takes the reaction away",
    )
    .await;
    remove.assert_reaction("remove", ANSWERED_CHANNEL, "the end of the turn");
    remove.ok().await;
    assert!(out_rx.try_recv().is_err(), "a posted answer emits nothing");
    // Marker on the same path, zero wait: the removal is the keeper's last
    // step, so nothing follows it.
    if let Ok(extra) = api.try_recv() {
        panic!(
            "GH #1530: no request may follow the removal (saw {})",
            extra.path
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_ack_typing_slack_an_interim_leaves_it_standing() {
    if !slack_template_in_tree() {
        return;
    }
    let log = meclaw_testing::log_recorder::global();
    let (addr, mut api) = web_api().await;
    // The ceiling is far below any round trip: once the reaction call came
    // back, the keeper's verdict is "ceiling", on any host -- unless the
    // interim already ended it.
    let mut cell = cell_on(addr, 1);
    let mut db = db();
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let sink = out_sink(out_tx);
    let (rc_tx, _rc_rx) = mpsc::channel(8);

    // 1. The incoming turn sets the reaction; the call is held, so the
    // keeper waits exactly there while the interim goes out.
    incoming(&mut cell, &mut db, "e-interim", INTERIM_CHANNEL).await;
    let add = next(
        &mut api,
        "GH #1530: the incoming turn sets a reaction on the person's message",
    )
    .await;
    add.assert_reaction("add", INTERIM_CHANNEL, "the reaction of the incoming turn");

    // 2. The interim goes out. It is not the answer.
    let interim = outgoing(INTERIM_CHANNEL, "one moment, checking", Some("interim"));
    tokio::join!(cell.handle(interim, &sink, &mut db, &rc_tx), async {
        let post = next(&mut api, "the interim's chat.postMessage reaches the API").await;
        post.assert_post(INTERIM_CHANNEL, "one moment, checking", "the interim");
        post.ok().await;
    });

    // 3. The held call comes back. The keeper still stands and ends at its
    // own ceiling -- a keeper the interim had ended never reaches that
    // verdict -- and the reaction goes away with it.
    add.ok().await;
    let stop = meclaw_testing::wait::until_some(
        "GH #1530: behind an interim the keeper still stands and ends at its own ceiling (a \
         keeper the interim ended never reaches that verdict)",
        || {
            let lines = log.lines(
                tracing::Level::DEBUG,
                &[INTERIM_CHANNEL, "no answer within max_total"],
            );
            async move { lines.into_iter().next() }
        },
    )
    .await;
    let remove = next(&mut api, "the ceiling takes the reaction away").await;
    remove.assert_reaction("remove", INTERIM_CHANNEL, &format!("the ceiling: {stop}"));
    remove.ok().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_ack_typing_slack_the_ceiling_takes_it_away() {
    if !slack_template_in_tree() {
        return;
    }
    let (addr, mut api) = web_api().await;
    let mut cell = cell_on(addr, 1);
    let mut db = db();

    // 1. The incoming turn sets the reaction.
    incoming(&mut cell, &mut db, "e-ceiling", CEILING_CHANNEL).await;
    let add = next(
        &mut api,
        "GH #1530: the incoming turn sets a reaction on the person's message",
    )
    .await;
    add.assert_reaction("add", CEILING_CHANNEL, "the reaction of the incoming turn");
    add.ok().await;

    // 2. No answer ever comes; the ceiling takes the reaction away.
    let remove = next(
        &mut api,
        "GH #1530: without an answer the reaction goes away at typing_max_ms",
    )
    .await;
    remove.assert_reaction("remove", CEILING_CHANNEL, "the ceiling");
    remove.ok().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_ack_typing_slack_a_repeat_takes_it_away() {
    if !slack_template_in_tree() {
        return;
    }
    let (addr, mut api) = web_api().await;
    let mut cell = cell_on(addr, deadline::product_wait_ms());
    let mut db = db();
    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
    let sink = out_sink(out_tx);
    let (rc_tx, _rc_rx) = mpsc::channel(8);

    incoming(&mut cell, &mut db, "e-repeat", REPEAT_CHANNEL).await;
    let add = next(
        &mut api,
        "GH #1530: the incoming turn sets a reaction on the person's message",
    )
    .await;
    add.assert_reaction("add", REPEAT_CHANNEL, "the reaction of the incoming turn");
    add.ok().await;

    // The answer is a repeat of a text the turn already posted: nothing is
    // posted, and it is still the answer of the turn.
    let answer = outgoing(REPEAT_CHANNEL, "here is what I found", Some("repeat"));
    tokio::join!(cell.handle(answer, &sink, &mut db, &rc_tx), async {
        let remove = next(
            &mut api,
            "GH #1530: an answer marked repeat takes the reaction away like a posted one",
        )
        .await;
        remove.assert_reaction("remove", REPEAT_CHANNEL, "the end of the turn");
        remove.ok().await;
    });
    assert!(out_rx.try_recv().is_err(), "a repeat emits nothing");
    if let Ok(extra) = api.try_recv() {
        panic!(
            "GH #1530: a repeat posts nothing and nothing follows the removal (saw {})",
            extra.path
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_ack_typing_slack_a_hanging_reaction_call_does_not_hold_the_answer() {
    if !slack_template_in_tree() {
        return;
    }
    let (addr, mut api) = web_api().await;
    let mut cell = cell_on(addr, deadline::product_wait_ms());
    let mut db = db();
    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
    let sink = out_sink(out_tx);
    let (rc_tx, _rc_rx) = mpsc::channel(8);

    // The reaction call is taken and never answered for the rest of the test.
    incoming(&mut cell, &mut db, "e-hanging", HANGING_CHANNEL).await;
    let held = next(
        &mut api,
        "GH #1530: the incoming turn sets a reaction on the person's message",
    )
    .await;
    held.assert_reaction("add", HANGING_CHANNEL, "the reaction of the incoming turn");

    // The answer goes out while that call hangs, and the turn ends. Whatever
    // else the connector asks for meanwhile is answered.
    let answer = outgoing(HANGING_CHANNEL, "here is what I found", None);
    let handled = cell.handle(answer, &sink, &mut db, &rc_tx);
    tokio::pin!(handled);
    let mut posted = false;
    deadline::hang(
        "GH #1530: a hanging reaction call never holds the answer up",
        async {
            loop {
                tokio::select! {
                    () = &mut handled => break,
                    Some(t) = api.recv() => {
                        if t.method() == "chat.postMessage" {
                            t.assert_post(HANGING_CHANNEL, "here is what I found", "the answer");
                            posted = true;
                        }
                        t.ok().await;
                    }
                }
            }
        },
    )
    .await;
    assert!(
        posted,
        "GH #1530: the answer is posted while the reaction call hangs"
    );
    assert!(
        out_rx.try_recv().is_err(),
        "a hanging reaction call is no failure of the answer"
    );
    drop(held);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_ack_typing_slack_a_refused_reaction_call_is_no_failure_of_the_answer() {
    if !slack_template_in_tree() {
        return;
    }
    let (addr, mut api) = web_api().await;
    let mut cell = cell_on(addr, deadline::product_wait_ms());
    let mut db = db();
    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
    let sink = out_sink(out_tx);
    let (rc_tx, _rc_rx) = mpsc::channel(8);

    // The workspace did not grant `reactions:write`.
    incoming(&mut cell, &mut db, "e-refused", REFUSED_CHANNEL).await;
    let add = next(
        &mut api,
        "GH #1530: the incoming turn sets a reaction on the person's message",
    )
    .await;
    add.assert_reaction("add", REFUSED_CHANNEL, "the reaction of the incoming turn");
    add.answer_with(json!({"ok": false, "error": "missing_scope", "needed": "reactions:write"}))
        .await;

    // The answer goes out as ever; a refused removal is no failure either.
    let answer = outgoing(REFUSED_CHANNEL, "here is what I found", None);
    tokio::join!(cell.handle(answer, &sink, &mut db, &rc_tx), async {
        let post = next(&mut api, "the answer's chat.postMessage reaches the API").await;
        post.assert_post(REFUSED_CHANNEL, "here is what I found", "the answer");
        post.ok().await;
    });
    let remove = next(
        &mut api,
        "GH #1530: the turn's end still takes the reaction away after a refused add",
    )
    .await;
    remove.assert_reaction("remove", REFUSED_CHANNEL, "the end of the turn");
    remove
        .answer_with(json!({"ok": false, "error": "no_reaction"}))
        .await;
    assert!(
        out_rx.try_recv().is_err(),
        "GH #1530: a refused reaction call is logged, never a send_failed of the answer"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_ack_typing_slack_the_shipped_template_leaves_it_off() {
    if !slack_template_in_tree() {
        return;
    }
    let shipped = shipped_proxy_config();
    assert_eq!(
        shipped["params"]["typing_reaction"],
        json!(""),
        "templates/slack-agent/proxy/config.json declares typing_reaction, shipped \"\" (off)"
    );
    let setting = &shipped["contract"]["settings"]["typing_reaction"];
    assert_eq!(
        setting["default"],
        json!(""),
        "the setting's default is the param"
    );
    assert!(
        setting["description"]
            .as_str()
            .is_some_and(|d| d.contains("reactions:write")),
        "the setting names the scope it needs: {setting}"
    );

    // A whole turn with the shipped values: the answer is the only call.
    let (addr, mut api) = web_api().await;
    let mut cell = cell_from(&shipped_params_against(addr, &[]));
    let mut db = db();
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let sink = out_sink(out_tx);
    let (rc_tx, _rc_rx) = mpsc::channel(8);
    incoming(&mut cell, &mut db, "e-off", OFF_CHANNEL).await;
    let answer = outgoing(OFF_CHANNEL, "here is what I found", None);
    tokio::join!(cell.handle(answer, &sink, &mut db, &rc_tx), async {
        let post = next(
            &mut api,
            "with the shipped values the answer is the first and only call",
        )
        .await;
        post.assert_post(OFF_CHANNEL, "here is what I found", "the answer");
        post.ok().await;
    });
    if let Ok(extra) = api.try_recv() {
        panic!(
            "GH #1530: off by default -- no reaction call (saw {})",
            extra.path
        );
    }
    assert_eq!(
        remembered(&mut db).await,
        0,
        "GH #1530: off by default -- the turn leaves no row in typing_reactions"
    );
}

fn shipped_registry() -> TemplatesRegistry {
    let scanned = scan_templates_dir(&core_root().join("templates")).unwrap_or_default();
    TemplatesRegistry::from_entries(
        scanned
            .into_iter()
            .map(|s| TemplateEntry {
                template_id: format!("scan-{}", s.name),
                name: s.name,
                version: s.version,
                filesystem_path: s.filesystem_path,
            })
            .collect(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_ack_typing_slack_an_instance_override_switches_it_on() {
    if !slack_template_in_tree() {
        return;
    }
    // The door: a set's override of the two params passes.
    let parsed = parse_subtree(
        &core_root().join("templates/slack-agent"),
        &shipped_registry(),
    )
    .unwrap_or_else(|e| panic!("the shipped slack template parses: {e:?}"));
    let proxy = parsed
        .cells
        .into_iter()
        .find(|c| c.rel_path == "proxy")
        .expect("the slack template has a proxy cell");
    let over = json!({"typing_reaction": REACTION, "typing_max_ms": 600_000});
    if let Err(why) = check_override_params(&proxy, Some("proxy"), "slack-agent", &over) {
        panic!("GH #1530: a set's {over} on slack-agent/proxy must pass the door: {why:?}");
    }

    // And the override is taken: the turn shows the reaction.
    let (addr, mut api) = web_api().await;
    let mut cell = cell_from(&shipped_params_against(
        addr,
        &[("typing_reaction", json!(REACTION))],
    ));
    let mut db = db();
    incoming(&mut cell, &mut db, "e-override", OVERRIDE_CHANNEL).await;
    let add = next(
        &mut api,
        "GH #1530: the instance override switches the reaction on",
    )
    .await;
    add.assert_reaction("add", OVERRIDE_CHANNEL, "the overridden instance");
    add.ok().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_ack_typing_slack_a_late_add_is_removed_at_the_end() {
    if !slack_template_in_tree() {
        return;
    }
    let (addr, mut api) = web_api().await;
    // The client's operation timeout is a tenth of the test's hang mark: the
    // add the test holds runs into it long before any mark of the test fires,
    // and every other call of the turn reaches the API well within it.
    let mut cell = cell_from(&shipped_params_against(
        addr,
        &[
            ("typing_reaction", json!(REACTION)),
            ("typing_max_ms", json!(deadline::product_wait_ms())),
            ("send_timeout_ms", json!(deadline::hang_ms() / 10)),
        ],
    ));
    let mut db = db();
    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
    let sink = out_sink(out_tx);
    let (rc_tx, _rc_rx) = mpsc::channel(8);

    // 1. The add is taken and held until the client gives it up at its own
    // timeout -- the connector never learns whether it was set.
    incoming(&mut cell, &mut db, "e-late", LATE_CHANNEL).await;
    let mut add = next(
        &mut api,
        "GH #1530: the incoming turn sets a reaction on the person's message",
    )
    .await;
    add.assert_reaction("add", LATE_CHANNEL, "the reaction of the incoming turn");
    deadline::hang(
        "the client gives the held reactions.add up at its operation timeout",
        add.given_up(),
    )
    .await;
    // 2. Slack still carries it out: the reaction stands after all.
    add.ok().await;

    // 3. The answer (a repeat, so no post is in the way) ends the turn, and
    // the end takes the reaction away although the add was never confirmed.
    let answer = outgoing(LATE_CHANNEL, "here is what I found", Some("repeat"));
    tokio::join!(cell.handle(answer, &sink, &mut db, &rc_tx), async {
        let remove = next(
            &mut api,
            "GH #1530: an add that failed at its timeout may have gone through -- the end of \
             the turn takes the reaction away all the same",
        )
        .await;
        remove.assert_reaction("remove", LATE_CHANNEL, "the end of the turn");
        // Nothing to take away is the goal reached, no failure.
        remove
            .answer_with(json!({"ok": false, "error": "no_reaction"}))
            .await;
    });
    assert!(
        out_rx.try_recv().is_err(),
        "GH #1530: an unconfirmed add is no failure of the answer"
    );
}

/// The reactions the connector remembers in its `cell.db`.
async fn remembered(db: &mut DbConn) -> i64 {
    db.call(|c| c.query_row("SELECT count(*) FROM typing_reactions", [], |r| r.get(0)))
        .await
        .unwrap_or_else(|e: rusqlite::Error| {
            panic!("GH #1530: cell.db remembers the reactions: {e}")
        })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_ack_typing_slack_a_restart_removes_the_reaction_left_behind() {
    if !slack_template_in_tree() {
        return;
    }
    let (addr, mut api) = web_api().await;
    // One cell.db across the restart, as on disk.
    let mut db = db();

    // 1. The turn sets its reaction ...
    let mut before = cell_on(addr, deadline::product_wait_ms());
    incoming(&mut before, &mut db, "e-restart", RESTART_CHANNEL).await;
    let add = next(
        &mut api,
        "GH #1530: the incoming turn sets a reaction on the person's message",
    )
    .await;
    add.assert_reaction("add", RESTART_CHANNEL, "the reaction of the incoming turn");
    add.ok().await;

    // 2. ... and the cell goes down before the answer: its keeper (and with
    // it the ceiling) is gone, nothing in RAM will ever take it away.
    drop(before);

    // 3. The next start takes the remembered reaction away and forgets it.
    let mut after = cell_on(addr, deadline::product_wait_ms());
    let mut reports = reports_of(&mut after);
    start(&mut after, &mut db).await;
    let remove = next(
        &mut api,
        "GH #1530: the start after a restart takes away the reaction the last run left \
         standing (today it stands for good)",
    )
    .await;
    remove.assert_reaction("remove", RESTART_CHANNEL, "the start after the restart");
    remove.ok().await;
    settle(&mut after, &mut db, &mut reports, RESTART_CHANNEL).await;
    assert_eq!(
        remembered(&mut db).await,
        0,
        "GH #1530: the start forgets every reaction it took away"
    );
}

/// The handler's end of the cell's report lane, as the I/O half would hold
/// it: the keeper reports each confirmed removal there.
fn reports_of(cell: &mut SlackCell) -> mpsc::UnboundedReceiver<SlackEvent> {
    cell.take_reaction_reports()
        .expect("the report lane is the cell's until the I/O half takes it")
}

/// `on_start`, as after a (re)spawn.
async fn start(cell: &mut SlackCell, db: &mut DbConn) {
    let (tx, _rx) = mpsc::channel::<CellEmission>(8);
    let origin = OriginSink::new(tx, Path::new("/slack"), 64);
    cell.on_start(&origin, db).await;
}

/// OR-d54g.acktyp.2: the keeper reports the confirmed removal on the lane,
/// and the handler -- the task that owns the `cell.db` -- takes the report
/// like any event of its I/O half.
async fn settle(
    cell: &mut SlackCell,
    db: &mut DbConn,
    reports: &mut mpsc::UnboundedReceiver<SlackEvent>,
    channel: &str,
) {
    let report = deadline::hang(
        "OR-d54g.acktyp.2: the confirmed removal is reported to the task that owns the cell.db",
        reports.recv(),
    )
    .await
    .expect("the report lane is open while the cell lives");
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let origin = OriginSink::new(tx, Path::new("/slack"), 64);
    cell.handle_event(report, &origin, db).await;
    assert!(
        rx.try_recv().is_err(),
        "a report of a removal in {channel} emits nothing"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_ack_typing_slack_a_failed_remove_keeps_its_row_and_is_retried() {
    if !slack_template_in_tree() {
        return;
    }
    let (addr, mut api) = web_api().await;
    // The gap between two tries is the product wait: a retry that came
    // before the test's hang mark followed Slack's `Retry-After`, not it.
    let mut cell = cell_from(&shipped_params_against(
        addr,
        &[
            ("typing_reaction", json!(REACTION)),
            ("typing_max_ms", json!(deadline::product_wait_ms())),
            ("typing_remove_retry_ms", json!(deadline::product_wait_ms())),
        ],
    ));
    let mut reports = reports_of(&mut cell);
    let mut db = db();
    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
    let sink = out_sink(out_tx);
    let (rc_tx, _rc_rx) = mpsc::channel(8);

    incoming(&mut cell, &mut db, "e-retried", RETRIED_CHANNEL).await;
    let add = next(
        &mut api,
        "GH #1530: the incoming turn sets a reaction on the person's message",
    )
    .await;
    add.assert_reaction("add", RETRIED_CHANNEL, "the reaction of the incoming turn");
    add.ok().await;

    // 1. The answer (a repeat: no post in the way) ends the turn; Slack
    // rate-limits the removal and names when to try again.
    let answer = outgoing(RETRIED_CHANNEL, "here is what I found", Some("repeat"));
    tokio::join!(cell.handle(answer, &sink, &mut db, &rc_tx), async {
        let remove = next(&mut api, "the end of the turn takes the reaction away").await;
        remove.assert_reaction("remove", RETRIED_CHANNEL, "the end of the turn");
        remove.rate_limited(0).await;
    });
    assert_eq!(
        remembered(&mut db).await,
        1,
        "OR-d54g.acktyp.2: a removal that failed keeps its row (today the turn's end forgets \
         it, and a reaction that stands is no longer known to anyone)"
    );

    // 2. The keeper tries again after `Retry-After`; the row is still there.
    let retry = next(
        &mut api,
        "OR-d54g.acktyp.2: a failed reactions.remove is tried again after Slack's Retry-After \
         (today it is given up after one try)",
    )
    .await;
    retry.assert_reaction("remove", RETRIED_CHANNEL, "the retry of the removal");
    assert_eq!(
        remembered(&mut db).await,
        1,
        "OR-d54g.acktyp.2: the row stands until the removal is confirmed"
    );
    retry.ok().await;

    // 3. Confirmed: reported, and the handler forgets the row.
    settle(&mut cell, &mut db, &mut reports, RETRIED_CHANNEL).await;
    assert_eq!(
        remembered(&mut db).await,
        0,
        "OR-d54g.acktyp.2: the confirmed removal forgets the row"
    );
    assert!(
        out_rx.try_recv().is_err(),
        "a failed removal is no failure of the answer"
    );
    if let Ok(extra) = api.try_recv() {
        panic!(
            "OR-d54g.acktyp.2: nothing follows the confirmed removal (saw {})",
            extra.path
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_ack_typing_slack_a_failed_remove_at_start_keeps_its_row() {
    if !slack_template_in_tree() {
        return;
    }
    let (addr, mut api) = web_api().await;
    let mut db = db();

    // 1. A turn sets its reaction, and the cell goes down before the answer.
    let mut before = cell_on(addr, deadline::product_wait_ms());
    incoming(&mut before, &mut db, "e-start-failed", START_FAILED_CHANNEL).await;
    let add = next(
        &mut api,
        "GH #1530: the incoming turn sets a reaction on the person's message",
    )
    .await;
    add.assert_reaction(
        "add",
        START_FAILED_CHANNEL,
        "the reaction of the incoming turn",
    );
    add.ok().await;
    drop(before);

    // 2. The next start tries to take it away, and Slack fails the call; no
    // further try in this run (`typing_remove_retries` 0), the cell goes down.
    let mut failing = cell_from(&shipped_params_against(
        addr,
        &[
            ("typing_reaction", json!(REACTION)),
            ("typing_remove_retries", json!(0)),
        ],
    ));
    start(&mut failing, &mut db).await;
    let remove = next(&mut api, "the start takes the remembered reaction away").await;
    remove.assert_reaction("remove", START_FAILED_CHANNEL, "the start");
    remove
        .answer_with(json!({"ok": false, "error": "internal_error"}))
        .await;
    assert_eq!(
        remembered(&mut db).await,
        1,
        "OR-d54g.acktyp.2: a removal that failed at the start keeps its row (today the start \
         forgets every row before its removal)"
    );
    drop(failing);

    // 3. The start after that takes it away again -- the row was still there.
    let mut next_start = cell_on(addr, deadline::product_wait_ms());
    let mut reports = reports_of(&mut next_start);
    start(&mut next_start, &mut db).await;
    let again = next(
        &mut api,
        "OR-d54g.acktyp.2: the next start takes away the reaction whose removal failed",
    )
    .await;
    again.assert_reaction("remove", START_FAILED_CHANNEL, "the next start");
    again.ok().await;
    settle(&mut next_start, &mut db, &mut reports, START_FAILED_CHANNEL).await;
    assert_eq!(
        remembered(&mut db).await,
        0,
        "OR-d54g.acktyp.2: the confirmed removal forgets the row"
    );
}

/// The vault grants of the shipped template, as a test names them.
const APP_GRANT: &str = "grant:slack-app@acktyp/proxy";
const BOT_GRANT: &str = "grant:slack-bot@acktyp/proxy";

/// The rows of `typing_reactions` in the `cell.db` at `path`, read beside the
/// running cell (a second connection to the same file).
fn rows_on_disk(path: &std::path::Path) -> i64 {
    let c = rusqlite::Connection::open(path).expect("open the cell.db beside the cell");
    c.query_row("SELECT count(*) FROM typing_reactions", [], |r| r.get(0))
        .expect("count typing_reactions")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_ack_typing_slack_the_start_sweep_runs_with_tokens_from_the_vault() {
    if !slack_template_in_tree() {
        return;
    }
    use meclaw_cells::proxy::factory::ProxyCellFactory;
    use meclaw_colony::CellFactory;
    use std::sync::Arc;

    let (addr, mut api) = web_api().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let cell_dir = dir.path().join("proxy");
    let cell_db = cell_dir.join("cell.db");

    // 1. What the last run left: one remembered reaction in the cell's own
    // database, before the cell is born.
    {
        let c = meclaw_colony::persist::cell_db::open_or_create_cell_db(&cell_db)
            .expect("the cell's database");
        setup_slack_schema(&c).expect("schema");
        meclaw_cells::proxy::slack::db::remember_reaction(
            &c,
            &format!("{VAULT_CHANNEL}:{THREAD_TS}"),
            &meclaw_cells::proxy::slack::db::RememberedReaction {
                channel: VAULT_CHANNEL.into(),
                ts: MESSAGE_TS.into(),
                name: REACTION.into(),
            },
            0,
        )
        .expect("remember");
    }

    // 2. The shipped form: both tokens from the vault, the whole cell (handler
    // and I/O half) spawned by the factory.
    let mut params = shipped_proxy_config()["params"].clone();
    params["app_token_grant_id"] = json!(APP_GRANT);
    params["bot_token_grant_id"] = json!(BOT_GRANT);
    params["emit_to"] = json!("/agent");
    params["base_url"] = json!(format!("http://{addr}"));
    params["connect_timeout_ms"] = json!(deadline::product_wait_ms());
    params["send_timeout_ms"] = json!(deadline::product_wait_ms());
    params["credential_wait_ms"] = json!(deadline::product_wait_ms());
    let (out_tx, mut out) = mpsc::channel::<CellEmission>(64);
    let (inbox_tx, _inbox) = mpsc::channel(64);
    let factory = Arc::new(ProxyCellFactory::new(Arc::new(
        meclaw_colony::SurfaceRegistry::new(),
    )));
    let spawned = factory
        .spawn_cell(
            Path::new("/slack"),
            params,
            out_tx,
            cell_dir,
            meclaw_colony::ContractView::default(),
            inbox_tx,
            None,
            0,
            None,
            None,
            1000,
        )
        .expect("the slack proxy spawns");
    let meclaw_colony::SpawnedCellKind::Active { sender, join, .. } = spawned else {
        unreachable!("a proxy is spawned active");
    };

    // 3. The bot token arrives sealed (the app token never does: the socket
    // stays shut, so the Web API sees only what the handler sends).
    let asked = next_request(&mut out, Some(BOT_GRANT)).await;
    sender
        .send(message(
            "/slack",
            delivery(BOT_GRANT, &asked, "stub-vault-bot"),
        ))
        .await
        .expect("the cell takes the sealed token");

    // 4. The first call of the run is the removal of the remembered reaction,
    // with the vaulted token.
    let remove = next(
        &mut api,
        "GH #1530 review I-2: with the bot token from the vault, the start takes away the \
         reaction the last run left standing once the sealed token arrived",
    )
    .await;
    remove.assert_reaction("remove", VAULT_CHANNEL, "the start with the vaulted token");
    assert_eq!(
        remove.authorization.as_deref(),
        Some("Bearer stub-vault-bot"),
        "the removal carries the token the vault delivered"
    );
    remove.ok().await;

    // 5. Confirmed: the report travels over the I/O half to the handler,
    // which forgets the row.
    meclaw_testing::wait::until_some(
        "OR-d54g.acktyp.2: the confirmed removal reaches the handler over the I/O half and the \
         row goes",
        || {
            let n = rows_on_disk(&cell_db);
            async move { (n == 0).then_some(()) }
        },
    )
    .await;
    drop(sender);
    join.abort();
}
