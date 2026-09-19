//! The throwaway display colony the five seam locks run against (befund 04 § E.2).
//!
//! Every other display lock in this tree drives `compose.py` as a subprocess and asserts
//! what it says. That covers 105 of the 107 scenarios, because the script hands its whole
//! curator state out in one emission. What it CANNOT show is the five seams around the
//! script: that the clock's order really strikes and comes back, that the judge really
//! answers and its verdict reaches the next pass, that the state row really survives the
//! store's delete+insert, that a mount really serves three outputs, and that an app's write
//! really travels the door. Above all it cannot show the ABSENCE of a message, because a
//! message that is never made is only missing where messages are made.
//!
//! So this module boots a real colony:
//!
//! ```text
//! /world                       timer   (keeps the colony connected)
//! /alex/channels/display       the SHIPPED display hive
//!                                web    mount "display" on the test listener
//!                                views  store
//!                                code   compose (script_inline, the shipped bytes)
//!                                timer  clock
//!                                llm    judge -> a loopback mock, never a real model
//! /alex/apps/probe             a `code` stand-in for the apps (§ 8.8, § 9)
//! ```
//!
//! Minimal on purpose, never `egon-seed`: a throwaway tree gets no real tokens, no
//! Telegram, no member and no memory hive -- none of which decides a sentence of § 3-9.
//!
//! **What a test reads.** The colony's own `message_log` (`ColonyMsg::ReadMessages`) is the
//! `GET /colony/messages` of a running meclaw, and every hop of every pass is in it: the
//! store bundles (and with them the ONE state row of § 3.1), the patch bundle for the web
//! cell, the orders to the clock, the questions to the judge. Reading it is also how a lock
//! measures that NOTHING was made.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_cells::web::WebCellFactory;
use meclaw_colony::api_dto::{MessageLogDto, MessageLogFilter};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, SurfaceRegistry, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mock_http::{CapturedRequest, MockResponse, start_mock_server_capturing};
use meclaw_testing::{surface_listener, wait_for_mount};
use tokio::sync::{mpsc, oneshot};

/// The failure-marker window of this repo: generous, robust under parallel cargo load,
/// never a semantic discriminator (CONTRIBUTING.md, 30 s convention).
pub const MARKER: Duration = Duration::from_secs(30);

/// The name the screen answers to on the colony's one listener. A `web` cell has had no
/// port of its own since `web@2.0.0`; the port in every URL is the LISTENER's.
pub const MOUNT: &str = "display";

/// Where the screen stands. The path is the owner slug of everything it writes itself.
pub const SCREEN: &str = "/alex/channels/display";

/// The app stand-in of § 8.8 / § 9. voice2vision is a foreign repository and does not
/// travel with this one, so the app seam is driven by a `code` cell that emits exactly the
/// writes the description names.
pub const PROBE: &str = "/alex/apps/probe";

/// The context key that lets a message die at the root onto this module's own channel.
const MARK: &str = "screen_out";

/// Every template this module reads, spelled out so the export's R2b check sees the
/// names (GH #9).
const NEEDED: [&str; 2] = ["templates/display", "templates/web"];

// ─────────────────────────────────────────────────────────────────────────────
// Guards and small file helpers
// ─────────────────────────────────────────────────────────────────────────────

pub fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// Whether the template library travels in this tree (it does not in the published one).
pub fn library_ships() -> bool {
    NEEDED
        .iter()
        .all(|rel| repo(rel).join("template.json").is_file())
}

pub fn have_python() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_ok()
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn write_json(p: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("dirs");
    std::fs::write(
        p,
        meclaw_core::serde_json::to_string_pretty(v).expect("json"),
    )
    .expect("write");
}

fn patch_json(p: &std::path::Path, f: impl FnOnce(&mut Value)) {
    let mut v = read_json(p);
    f(&mut v);
    write_json(p, &v);
}

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("dirs");
    for entry in std::fs::read_dir(src).expect("read_dir") {
        let entry = entry.expect("entry");
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).expect("copy");
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The app stand-in
// ─────────────────────────────────────────────────────────────────────────────

/// The `code` cell at `/alex/apps/probe`: the writes of § 8.8 and § 9, and nothing else.
///
/// A test names an act of an app (`chat_turn`, `ambient_boot`, `clock_minute`, …) and this
/// script builds the `in_view` the description prescribes for it -- the window with its
/// hints, `touched` on a new line, `turn_id` on a turn, a minute of the clock WITHOUT
/// `touched`. Keeping that vocabulary in the stand-in rather than in the test is the point:
/// what travels the door is then an app's write, not a test's hand-built row.
const PROBE_SCRIPT: &str = r#"# The apps of the description, as far as a seam lock needs them.
#
# Reads one command out of the body and answers with the `in_view` (or `in_withdraw`) the
# description prescribes: display-hive.md 8.8 for the chat, 9 for ambient.
import json
import sys


def window(props, children=None):
    node = {"component": "display-pane", "props": props, "key": "c.win"}
    if children:
        node["children"] = children
    return node


def view(view_id, props, children=None, ttl_ms=0, ord_=0):
    # `pane_id` is the DOM id of the window, and the dock's `data-for` is that id --
    # it is how the client matches tile and window for the zoom (compose.py, TILE
    # TEMPLATE). A window written WITHOUT one carries no id in the DOM at all, so a
    # browser proof that follows a tile to its window finds nothing; a real
    # application always writes one.
    props = dict(props)
    props.setdefault("pane_id", "pane-" + view_id)
    return {"header": {"route": "in_view"}, "messages": [],
            "view_id": view_id, "region": "main", "ord": ord_, "kind": "component",
            "content": window(props, children), "components": [], "ttl_ms": ttl_ms}


def line(text):
    # `body`, not `text`: that is the prop `display-text` declares (compose.py,
    # TEXT_TEMPLATE). Written under the wrong name the paragraph rendered EMPTY, and
    # every window of this stage was one title tall -- which is why no canvas of a
    # large exit ever overflowed.
    return {"component": "display-text", "props": {"body": text}, "key": "c.line"}


def prose(lines, key="c.prose"):
    """A window with enough in it to outgrow a canvas (§ 6.3, § 5.9).

    More WINDOWS do not make a canvas overflow -- the curator opens what comes over the
    bar, and nine one-line windows fit on a monitor with room to spare. Height does.
    """
    return {"component": "display-panel", "key": key,
            "props": {"title": "Notes", "scroll": False},
            "children": [{"component": "display-text", "key": "p.%d" % i,
                          "props": {"body": t}} for i, t in enumerate(lines)]}


def tile(props):
    """The one line an app says about itself, keyed `tile` -- the dock takes it out.

    `end_at` is an INT epoch: the client writes the remainder into the tile once a
    second, and the server never ticks for it (D-27).
    """
    return {"component": "display-tile", "props": props, "key": "tile"}


def chat_line(role, text, channel, source, at=0):
    """§ 8.4/§ 8.5 (R-26-1): one line, with the CHANNEL it came through and its MOMENT.

    The stage writes what the chat application writes (apps/chat 0.3.2, `chat_line`): the
    moment rides on the CHILD as epoch milliseconds under `at` -- a number, because the
    catalogue declares the prop `int` and only hints on the window travel as text (§ 3.3)
    -- and a line with no moment carries no prop at all. A zero is a time (01.01.1970) and
    a screen handed one draws it; leaving the prop off is how a row from before R-26-1
    renders.

    The text of the time is NOT written here. The markup carries the number and the client
    formats HH:MM in the viewer's zone (§ 8.4), so a stage that stamped a word would
    measure its own clock instead of the screen's.
    """
    props = {"role": role, "text": text, "channel": channel, "source": source}
    if int(at) > 0:
        props["at"] = int(at)
    return {"component": "display-chat-line", "key": "c.%s.%s" % (role, abs(hash(text))),
            "props": props}


def chat(cmd):
    """§ 8.5: the chat window as the app writes it; § 8.8: what it writes per line.

    `lines` builds a real conversation -- a `display-chat` with `display-chat-line`
    children and a `display-input` at the foot -- which is what §§ 7.3/8.4/8.5 are about
    and what a browser proof of them needs to find in the DOM. Without it the window
    carries one plain text node and B-26/B-27/B-28 have nothing to look at.
    """
    props = {"title": "Chat", "layer": "modal", "pinned": True,
             "context": "conversation", "topic": "chat",
             "relevance": cmd.get("relevance") or "0.8",
             "linger": str(cmd.get("linger") or 90000),
             "touched": str(cmd["at"])}
    if cmd.get("turn_id"):
        props["turn_id"] = str(cmd["turn_id"])
    turns = cmd.get("lines")
    if not turns:
        return [view("chat", props, [line(cmd.get("text") or "")])]
    # Every line its own moment, ascending to the window's (R-26-1): the newest line
    # stands at the bottom and is the most recent one, the way a conversation reads. A
    # minute apart, the way the sheet stage of the same proof writes them -- the lines
    # handed to this stage carry no time of their own, so the window's is where theirs
    # comes from.
    said = [(("you" if i % 2 == 0 else "companion"), t)
            for i, t in enumerate(turns)]
    if cmd.get("text"):
        said.append(("companion", {"text": cmd["text"], "channel": "voice",
                                   "source": "voice"}))
    kids = []
    for i, (who, t) in enumerate(said):
        where = t.get("channel") or ("typed" if who == "you" else "voice")
        kids.append(chat_line(who, t.get("text") or "", where,
                              t.get("source") or where,
                              at=t.get("at") or (int(cmd["at"])
                                                 - (len(said) - 1 - i) * 60000)))
    conversation = {"component": "display-chat", "key": "c.chat",
                    "props": {"title": "Conversation"}, "children": kids}
    # § 7.3: ONE field, no form. The window carries it at its foot, and a person types
    # into it -- the event it sends is the app's (`typed`), not the screen's.
    field = {"component": "display-input", "key": "c.input",
             "props": {"placeholder": "Say something", "event": "typed", "for": "chat"}}
    return [view("chat", props, [conversation, field])]


# § 9.1: three views -- clock, weather, timer -- as the app writes them, all three in
# ONE act. The pass reconciles its state with the store's rows before its own event runs
# (OR-H0.9, H1-F4), so a breath of three writes keeps all three windows.
AMBIENT = {
    "clock": {"title": "Clock", "seat": "bottom", "seat_ord": "0", "layer": "canvas",
              "topic": "clock", "context": "ambient", "relevance": "0.4"},
    "weather": {"title": "Weather", "seat": "bottom", "seat_ord": "10", "layer": "canvas",
                "topic": "weather:berlin", "context": "ambient", "relevance": "0.5"},
    # D-27: a timer counts down, and the tile writes the remainder once a second out
    # of `end_at`. Without the epoch the dock has nothing to count.
    "timer": {"title": "Timer", "layer": "canvas", "topic": "timer:egg",
              "context": "ambient", "relevance": "0.6"},
}


def ambient(cmd):
    """The views of § 9.1, with the touch of their newest content.

    Without `view` the app writes all three in one act, the way § 9.1 describes it.
    With `view` it writes exactly that one -- what a case about a single window needs.
    """
    which = cmd.get("view")
    wanted = [str(which)] if which else ["clock", "weather", "timer"]
    out = []
    for name in wanted:
        props = dict(AMBIENT[name])
        props["touched"] = str(cmd["at"])
        if which and cmd.get("state"):
            # § 9.1: a timer that rings says `state: urgent`, together with `touched`.
            props["state"] = str(cmd["state"])
        kids = [line(cmd.get("text") or "")]
        if name == "timer" and cmd.get("end_at"):
            # D-27: the seconds of a running timer are the tile's, out of `end_at`.
            # `value` is not decoration: the slot the client writes the remainder
            # into (`[data-role=remaining]`) exists only where the tile said a
            # value at all (compose.py, TILE_TEMPLATE).
            kids.insert(0, tile({"glyph": "\u23f2", "line": "Timer",
                                 "value": "10:00", "end_at": int(cmd["end_at"])}))
        out.append(view(name, props, kids))
    return out


def main():
    doc = json.load(sys.stdin)
    body = doc.get("body") or {}
    out = []
    for turn in body.get("messages") or []:
        if not isinstance(turn, dict) or turn.get("type") != "text":
            continue
        try:
            cmd = json.loads(turn.get("text") or "")
        except ValueError:
            continue
        if not isinstance(cmd, dict):
            continue
        what = str(cmd.get("do") or "")
        if what == "chat":
            out += chat(cmd)
        elif what == "ambient":
            out += ambient(cmd)
        elif what == "minute":
            # § 9.3: the clock writes its time into a child every minute, WITHOUT
            # `touched`. No touch, no judge -- and the pass has to see exactly that.
            out.append(view("clock", dict(AMBIENT["clock"]),
                            [line(cmd.get("text") or "12:01")]))
        elif what == "card":
            # § 8.3: an app whose window arises from an answer carries that answer's
            # `turn_id` -- the write step 5 reads.
            props = {"title": cmd.get("title") or "Card", "layer": "canvas",
                     "context": cmd.get("context") or "work",
                     "relevance": cmd.get("relevance") or "0.9",
                     "topic": cmd.get("topic") or "card:1",
                     "touched": str(cmd["at"])}
            if cmd.get("turn_id"):
                props["turn_id"] = str(cmd["turn_id"])
            out.append(view(str(cmd.get("view_id") or "card"), props,
                            [line(cmd.get("text") or "")]))
        elif what == "tall":
            # A window with a lot IN it. Not a TALL one: § 5.9 makes the window body
            # the scroller, so sixty paragraphs and one line are the same eighty-odd
            # pixels on the canvas. What this act is for is a window that really has
            # something to scroll INSIDE (B-09), and a dock tile with a topic of its
            # own beside it.
            props = {"title": cmd.get("title") or "Notes", "layer": "canvas",
                     "context": "work",
                     "relevance": cmd.get("relevance") or "0.95",
                     "topic": cmd.get("topic") or "notes:1",
                     "touched": str(cmd["at"])}
            body = ["%s %d -- a paragraph long enough that the body of this window has "
                    "to scroll, which is what § 5.9 asks of it" %
                    (cmd.get("text") or "note", i)
                    for i in range(int(cmd.get("paragraphs") or 60))]
            out.append(view(str(cmd.get("view_id") or "notes"), props,
                            [prose(body)]))
        elif what == "withdraw":
            out.append({"header": {"route": "in_withdraw"}, "messages": [],
                        "view_id": str(cmd.get("view_id") or "")})
    sys.stdout.write(json.dumps(out[0] if len(out) == 1 else out))


main()
"#;

// ─────────────────────────────────────────────────────────────────────────────
// Boot
// ─────────────────────────────────────────────────────────────────────────────

/// The dials a seam lock turns. Everything else is the shipped default.
pub struct Boot {
    /// `params.judge` of the compose cell: `on` wires the loopback judge in.
    pub judge: bool,
    /// § 4.3: the shortest distance between two judge calls, over REAL time here.
    pub judge_min_interval_ms: u64,
    /// § 4.15. Small, so a lock that waits for the fade is a lock and not a nap.
    pub linger_ms: u64,
    /// § 4.15.
    pub fade_ms: u64,
    /// What the loopback judge answers, every time (§ 4.5).
    pub verdict: Value,
}

impl Default for Boot {
    fn default() -> Self {
        Boot {
            judge: false,
            judge_min_interval_ms: 3000,
            linger_ms: 2000,
            fade_ms: 3000,
            verdict: json!({"bar": 0.3, "weights": {}, "windows": []}),
        }
    }
}

/// The three outputs of befund 04 § E.2: a television with nothing to press, a monitor
/// with pointer and keyboard, a phone with audio and touch (§ 6.1, § 6.4).
pub fn screens() -> Value {
    json!({
        "tv": {"display_type": "tv", "viewing_distance_m": 3.0, "inputs": [],
               "dock_default": "shown", "dock_max": 7},
        "monitor": {"display_type": "monitor", "viewing_distance_m": 0.7,
                    "inputs": ["pointer", "keyboard"],
                    "dock_default": "shown", "dock_max": 8},
        "phone": {"display_type": "phone", "viewing_distance_m": 0.35,
                  "inputs": ["audio", "touch"], "dock_default": "hidden", "dock_max": 5}
    })
}

/// A booted colony and every door a lock needs onto it.
pub struct Colony {
    _td: tempfile::TempDir,
    pub h: ColonyHandle,
    pub surfaces: Arc<SurfaceRegistry>,
    /// The one listener in front of the whole colony.
    pub port: u16,
    _listener: tokio::task::JoinHandle<()>,
    /// The two API routes a browser proof needs, on a port of their own.
    pub api_port: u16,
    _api: tokio::task::JoinHandle<()>,
    /// Whatever left `/alex` marked: receipts and events of the screen.
    pub egress: mpsc::Receiver<Message>,
    /// Every question the judge cell put to its provider, in arrival order.
    pub judge_asks: Arc<tokio::sync::Mutex<Vec<CapturedRequest>>>,
    _judge: tokio::task::JoinHandle<()>,
}

/// The two API routes a browser proof needs, on a listener of their own.
///
/// **Why at all.** A browser proof of § 5.5 has to write into the colony while the page
/// stands (a new urgent, a fresh tile, an answer) and to read the `message_log` back (a
/// press says NOTHING to the colony, Q-07). A running meclaw answers both at
/// `POST /messages` and `GET /colony/messages`; a fixture's `surface_listener` is a pure
/// MOUNT listener and answers `404` to everything else. Until this existed, the driver
/// asked the mount port for both and got three 404s per run -- and the proof that reads
/// "nothing changed" was green ON those 404s (measured: `wrote: [{status: 404} x3]` in
/// all six lines).
///
/// **Why a second port and not the mount listener.** A fixture may not carry the HTTP API
/// crate (a cell must not depend on it, GH #381), so this is not the shipped router but
/// the two routes spelled out with the pieces `meclaw-cells` already has. A port of its
/// own keeps it out of the way of the mount listener, which is the thing under test.
///
/// **What it is not.** No auth, no paging, no filters, no other route. A proof that needs
/// more than these two verbs is a proof about the API, and that has its own tests.
async fn api_listener(inbox: mpsc::Sender<ColonyMsg>) -> (u16, tokio::task::JoinHandle<()>) {
    use axum::extract::State;
    use axum::routing::{get, post};

    /// `POST /messages`: `{target, hop, body}` -- the shape the shipped route takes, cut
    /// to what a proof sends.
    async fn post_message(
        State(inbox): State<mpsc::Sender<ColonyMsg>>,
        axum::Json(body): axum::Json<Value>,
    ) -> axum::response::Response {
        let Some(target) = body.get("target").and_then(Value::as_str) else {
            return (axum::http::StatusCode::BAD_REQUEST, "no target").into_response();
        };
        let mut hop = meclaw_core::serde_json::Map::new();
        if let Some(map) = body.get("hop").and_then(Value::as_object) {
            hop.extend(map.clone());
        }
        let payload = body.get("body").cloned().unwrap_or_else(|| json!({}));
        let msg = MessageBuilder::new(Path::new(target))
            .reply_to(Path::new(PROBE))
            .hop(hop)
            .body(Body::Inline(payload))
            .ttl(24)
            .build();
        let id = msg.id.to_string();
        if inbox
            .send(ColonyMsg::Route {
                sender_path: Path::new("/"),
                msg,
            })
            .await
            .is_err()
        {
            return (axum::http::StatusCode::SERVICE_UNAVAILABLE, "colony gone").into_response();
        }
        (
            axum::http::StatusCode::ACCEPTED,
            axum::Json(json!({"id": id})),
        )
            .into_response()
    }

    /// `GET /colony/messages`: how many rows the log holds. A proof counts them; it never
    /// reads one.
    ///
    /// The limit is far above what a run of this fixture produces on purpose. With a
    /// page-sized one the count SATURATES, and a proof that asks "did the log grow"
    /// reads the same number before and after -- measured: B-26 failed in WebKit with
    /// `log_before: 1000, log_after: 1000` once the stage wrote enough.
    async fn get_messages(
        State(inbox): State<mpsc::Sender<ColonyMsg>>,
        axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
    ) -> axum::response::Response {
        let (ack_tx, ack_rx) = oneshot::channel();
        if inbox
            .send(ColonyMsg::ReadMessages {
                filter: MessageLogFilter {
                    limit: 100_000,
                    scan_budget: 500_000,
                    ..Default::default()
                },
                ack: ack_tx,
            })
            .await
            .is_err()
        {
            return (axum::http::StatusCode::SERVICE_UNAVAILABLE, "colony gone").into_response();
        }
        match tokio::time::timeout(MARKER, ack_rx).await {
            Ok(Ok(reply)) => {
                // `?route=hold` counts only the rows whose hop names that route. A proof
                // that asks "did my gesture reach the colony" cannot use the whole log:
                // the clock strikes into it while the browser is looking, and the newest
                // row is a different one a second later whatever the finger did.
                let route = q.get("route").cloned();
                let named = route.as_ref().map(|r| {
                    let needle = format!("\"route\":\"{r}\"");
                    reply
                        .entries
                        .iter()
                        .filter(|e| e.headers_json.contains(&needle))
                        .count()
                });
                axum::Json(json!({
                "messages": reply.entries.len(),
                "named": named,
                // The NEWEST row's id, and it is what a proof should compare. A count
                // saturates -- the log of this colony stops growing at its retention
                // limit, and a proof that asks "did the log grow" then reads the same
                // number before and after (measured: B-26 red in WebKit with
                // `log_before: 1000, log_after: 1000`). A uuid7 only ever moves forward.
                "newest": reply.entries.first().map(|r| r.id.clone()),
                "scan_truncated": reply.scan_truncated}))
                .into_response()
            }
            _ => (
                axum::http::StatusCode::GATEWAY_TIMEOUT,
                "the log did not answer",
            )
                .into_response(),
        }
    }

    use axum::response::IntoResponse;
    let app = axum::Router::new()
        .route("/messages", post(post_message))
        .route("/colony/messages", get(get_messages))
        .with_state(inbox);
    // Same shape as `surface_listener`: a named free port rather than `:0`, asked again
    // when another process won the race (OR-H5.1).
    for _ in 0..8 {
        let port = meclaw_testing::free_port();
        if let Ok(bound) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            let app = app.clone();
            let join = tokio::spawn(async move {
                let _ = axum::serve(bound, app).await;
            });
            return (port, join);
        }
    }
    panic!("no port for the fixture's API listener");
}

/// One canned OpenAI chat completion. The mock repeats its LAST response for ever, so one
/// verdict answers every question the judge ever asks.
fn canned(text: &str) -> MockResponse {
    MockResponse::ok_json(
        json!({
            "id": "verdict-1",
            "object": "chat.completion",
            "created": 1,
            "model": "mock-judge",
            "choices": [{"index": 0, "finish_reason": "stop",
                         "message": {"role": "assistant", "content": text}}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        })
        .to_string()
        .as_bytes(),
    )
}

/// Boot the tree of befund 04 § E.2 and hand back every door onto it.
///
/// The listener takes a free port from the OS rather than a number written down here
/// (OR-H5.1): `surface_listener` already asks for one and retries, so five of these files
/// run beside each other -- and beside a gate, and beside the live colonies of this host --
/// without a port ever being a reason for a red run.
pub async fn boot(opts: Boot) -> Colony {
    let (judge_addr, judge_join, judge_asks) =
        start_mock_server_capturing(vec![canned(&opts.verdict.to_string())]).await;

    let td = tempfile::TempDir::new().expect("tempdir");
    let root = td.path();
    write_json(&root.join("colony.json"), &json!({"schema_version": 1}));

    // The root: the world's timer keeps `/alex` connected (GH #265), and whatever leaves
    // `/alex` dies at the root onto this module's channel, marked on the way.
    let mut mark = meclaw_core::serde_json::Map::new();
    mark.insert(MARK.to_string(), json!("'1'"));
    write_json(
        &root.join("main/config.json"),
        &json!({
            "cell": {"type": "hive"},
            "params": {"graph": {"edges": [
                {"from": "./world", "to": "./alex"},
                {"from": "./alex", "to": ".", "modifier": {"set_context": mark}}
            ]}}
        }),
    );
    write_json(
        &root.join("main/world/config.json"),
        &json!({"cell": {"type": "timer", "timeout": -1},
                "params": {"query_timeout_ms": 5000},
                "contract": {"version": "1.0.0", "settings": {}, "consumes": {}}}),
    );

    // The person's level: the apps write onto the channels, and what the screen answers
    // leaves the level.
    // The three lanes in, the two lanes out (`templates/display/config.json`). Both
    // directions are named: an unconditional way back would make a message that only
    // ARRIVED at the screen travel up again and back down the in-lane -- a loop the TTL
    // ends after twenty passes instead of a door that opens once.
    let three_lanes = "has(hop.route) && (hop.route == 'in_view' || \
                       hop.route == 'in_withdraw' || hop.route == 'in_notice')";
    let two_lanes = "has(hop.route) && (hop.route == 'event' || hop.route == 'receipt')";
    write_json(
        &root.join("main/alex/config.json"),
        &json!({
            "cell": {"type": "hive"},
            "params": {"graph": {"edges": [
                {"from": "./apps", "to": "./channels", "condition": three_lanes},
                {"from": "./channels", "to": ".", "condition": two_lanes}
            ]}}
        }),
    );
    write_json(
        &root.join("main/alex/apps/config.json"),
        &json!({
            "cell": {"type": "hive"},
            "params": {"graph": {"edges": [{"from": "./probe", "to": "."}]}}
        }),
    );
    write_json(
        &root.join("main/alex/apps/probe/config.json"),
        &json!({
            "cell": {"type": "code"},
            "params": {
                "runner": "python3",
                "script_inline": PROBE_SCRIPT,
                // COLD, not warm. GH #714: a warm runner that is recycled between two
                // acts swallowed one act of a rebuild in 1 of 5 runs -- no answer, no
                // dead letter, the waiting lock ran 30 s into nothing. The stand-in gets
                // about a dozen messages per run, so a fresh interpreter each time costs
                // nothing measurable here, and what is under test is the SCREEN.
                "runner_mode": "cold",
                "external_timeout_ms": 20000,
                "sandbox": {"trust": "restricted", "network": "deny",
                            "filesystem": {"runtime": true}}
            },
            "contract": {
                "version": "1.0.0", "settings": {},
                // One act of an app may be several views (§ 9.1: the ambient app writes
                // clock, weather and timer in one breath). A `code` cell whose script
                // answers with a JSON ARRAY needs to say so, or the substrate refuses the
                // second and third message with `multi_send_not_declared` -- measured.
                "multi_send_capable": true,
                "consumes": {"body": {"messages": {"type": "array", "required": true}}},
                "emits": {
                    "body": {"messages": {"type": "array", "required": true}},
                    "hop": {"route": {"type": "string", "required": false}}
                }
            }
        }),
    );
    write_json(
        &root.join("main/alex/channels/config.json"),
        &json!({
            "cell": {"type": "hive"},
            "params": {"graph": {"edges": [
                {"from": ".", "to": "./display", "condition": three_lanes},
                {"from": "./display", "to": ".", "condition": two_lanes}
            ]}}
        }),
    );

    // The SHIPPED screen, with four values changed and nothing else: the mount, the three
    // outputs, the curator's dials and a judge that answers on loopback.
    copy_tree(
        &repo("templates/display"),
        &root.join("main/alex/channels/display"),
    );
    // The display refs `web@2.0.4`, and a ref resolves against the templates table, which
    // is empty until somebody fills it (GH #424).
    copy_tree(&repo("templates/web"), &root.join("templates/web"));
    let screen = root.join("main/alex/channels/display");
    patch_json(&screen.join("web/config.json"), |v| {
        v["override_params"][""]["mount"] = json!(MOUNT)
    });
    let judge_word = if opts.judge { "on" } else { "off" };
    patch_json(&screen.join("compose/config.json"), |v| {
        v["params"]["screens"] = screens();
        v["params"]["default_screen"] = json!("monitor");
        v["params"]["judge"] = json!(judge_word);
        v["params"]["judge_min_interval_ms"] = json!(opts.judge_min_interval_ms);
        v["params"]["linger_ms"] = json!(opts.linger_ms);
        v["params"]["fade_ms"] = json!(opts.fade_ms);
    });
    patch_json(&screen.join("judge/config.json"), |v| {
        v["params"]["model"] = json!("mock-judge");
        v["params"]["api_key"] = json!("fake-key");
        v["params"]["base_url"] = json!(format!("http://{judge_addr}"));
    });

    let surfaces = Arc::new(SurfaceRegistry::new());
    let factories: Vec<(String, Arc<dyn CellFactory>)> = vec![
        (
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        ),
        ("store".to_string(), Arc::new(StoreCellFactory)),
        ("timer".to_string(), Arc::new(TimerCellFactory)),
        ("llm".to_string(), Arc::new(LlmCellFactory)),
        (
            "web".to_string(),
            Arc::new(WebCellFactory::new(Arc::clone(&surfaces))),
        ),
    ];
    let (h, egress) = ColonyHandle::new_with_marked_egress_at(&td, factories.clone(), MARK);
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories {
        registry.insert(name, f);
    }
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: root.join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan sent");
    ack_rx
        .await
        .expect("rescan acked")
        .expect("the template table fills");
    bootstrap_from_filesystem(root, &registry, &h.runtime())
        .await
        .expect("the colony boots");

    wait_for_mount(&surfaces, MOUNT).await;
    let (addr, listener) = surface_listener(Arc::clone(&surfaces)).await;
    let (api_port, api) = api_listener(h.inbox_tx.clone()).await;
    Colony {
        _td: td,
        h,
        surfaces,
        port: addr.port(),
        _listener: listener,
        api_port,
        _api: api,
        egress,
        judge_asks,
        _judge: judge_join,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Writing onto the screen
// ─────────────────────────────────────────────────────────────────────────────

/// One message on a lane of the screen, owned by `owner` (`envelope.reply_to`, § 4.6).
pub fn to_screen(route: &str, owner: &str, body: Value) -> Message {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("route".into(), json!(route));
    MessageBuilder::new(Path::new(SCREEN))
        .reply_to(Path::new(owner))
        .hop(hop)
        .body(Body::Inline(body))
        .ttl(24)
        .build()
}

/// A component window with `props`, the shape every app sends (§ 7.1).
pub fn window(props: Value) -> Value {
    json!({"component": "display-pane", "props": props, "key": "c.win"})
}

impl Colony {
    /// Put one window up, owned by `owner`.
    pub async fn write_view(&self, owner: &str, view_id: &str, props: Value) {
        self.write_view_ttl(owner, view_id, props, 0).await
    }

    pub async fn write_view_ttl(&self, owner: &str, view_id: &str, props: Value, ttl_ms: i64) {
        // `pane_id` is the DOM id of the window, and the dock's `data-for` is that id
        // (compose.py, TILE_TEMPLATE). A window written without one has no id in the
        // DOM at all, and a browser proof that follows a tile to its window finds
        // nothing; a real application always writes one.
        let mut props = props;
        if let Some(map) = props.as_object_mut() {
            map.entry("pane_id")
                .or_insert_with(|| json!(format!("pane-{view_id}")));
        }
        self.h
            .send(to_screen(
                "in_view",
                owner,
                json!({"view_id": view_id, "region": "main", "kind": "component",
                       "content": window(props), "components": [], "ttl_ms": ttl_ms,
                       "messages": []}),
            ))
            .await;
    }

    /// Put one window up and WAIT until the pass has placed it.
    ///
    /// Only the wait, never a pause before it: a pass reconciles its state with the
    /// store's rows before its own event runs (§ 3.1, OR-H0.9, H1-F4), so a write that
    /// reached the store reaches the state row too -- also when a second write, or a
    /// verdict coming back, is in the air at the same moment. The lock that says so is
    /// `707_the_state_is_reconciled_with_the_store.rs`.
    pub async fn put(&self, owner: &str, view_id: &str, props: Value) -> Value {
        self.put_ttl(owner, view_id, props, 0).await
    }

    /// `put` with a `ttl_ms` on the write (§ 4.34).
    pub async fn put_ttl(&self, owner: &str, view_id: &str, props: Value, ttl_ms: i64) -> Value {
        let oid = self.oid(owner, view_id);
        self.write_view_ttl(owner, view_id, props, ttl_ms).await;
        self.wait_state(&format!("the write of {oid} reaches the state row"), |s| {
            s["views"].get(&oid).is_some()
        })
        .await
    }

    /// Tell the app stand-in to act, and WAIT until the pass has placed the window
    /// `view_id`. An act may write more than one view (§ 9.1); this waits for one of them
    /// and leaves the rest to the reconciliation of § 3.1 (H1-F4), which is what
    /// `707_the_state_is_reconciled_with_the_store.rs` pins.
    pub async fn app_put(&self, cmd: Value, view_id: &str) -> Value {
        let oid = self.oid(PROBE, view_id);
        let before = self
            .state()
            .await
            .map(|s| s["views"][&oid]["written_at"].clone())
            .unwrap_or(Value::Null);
        self.app(cmd).await;
        self.wait_state(&format!("the app's write of {oid} reaches the pass"), |s| {
            s["views"].get(&oid).is_some() && s["views"][&oid]["written_at"] != before
        })
        .await
    }

    /// Take one window down again (§ 4.34).
    pub async fn withdraw(&self, owner: &str, view_id: &str) {
        self.h
            .send(to_screen(
                "in_withdraw",
                owner,
                json!({"view_id": view_id, "messages": []}),
            ))
            .await;
    }

    /// Tell the app stand-in to act. The command travels as an ordinary text turn, so the
    /// probe is a cell of the colony and not a back door of the test.
    pub async fn app(&self, cmd: Value) {
        self.h
            .send(
                MessageBuilder::new(Path::new(PROBE))
                    .reply_to(Path::new(PROBE))
                    .body(Body::Inline(json!({"messages": [
                        {"origin": "user", "type": "text", "text": cmd.to_string()}
                    ]})))
                    .ttl(24)
                    .build(),
            )
            .await;
    }

    /// The window id of a view in the screen state (§ 2 Id).
    pub fn oid(&self, owner: &str, view_id: &str) -> String {
        format!("view.{}.{}", owner.replace('/', "~"), view_id)
    }

    pub async fn shutdown(self) {
        self.h.shutdown().await;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Reading the colony back
// ─────────────────────────────────────────────────────────────────────────────

impl Colony {
    /// The colony's `message_log`, oldest first -- the `GET /colony/messages` of a running
    /// meclaw. `to_prefix` narrows it to one receiver.
    pub async fn log(&self, to_prefix: Option<&str>) -> Vec<MessageLogDto> {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.h
            .inbox_tx
            .send(ColonyMsg::ReadMessages {
                filter: MessageLogFilter {
                    to_path_prefix: to_prefix.map(str::to_string),
                    limit: 1000,
                    scan_budget: 50_000,
                    ..Default::default()
                },
                ack: ack_tx,
            })
            .await
            .expect("inbox alive");
        let reply = tokio::time::timeout(MARKER, ack_rx)
            .await
            .expect("the log answers within the failure-marker window")
            .expect("ack delivered");
        assert!(
            !reply.scan_truncated,
            "the message log outgrew the scan budget; the lock would be reading a window"
        );
        let mut rows = reply.entries;
        rows.reverse(); // the reply is newest first; a lock reads a colony forwards
        rows
    }

    /// How many rows the whole log holds. What Q-07 counts.
    pub async fn log_len(&self) -> usize {
        self.log(None).await.len()
    }

    /// Every message the screen's `<child>` received, oldest first.
    pub async fn to_child(&self, child: &str) -> Vec<MessageLogDto> {
        self.log(Some(&format!("{SCREEN}/{child}"))).await
    }

    /// The store bundles the compose cell sent to `views`, as the list of their calls.
    pub async fn store_bundles(&self) -> Vec<Vec<Value>> {
        self.to_child("views")
            .await
            .iter()
            .filter_map(calls_of)
            .collect()
    }

    /// The ONE state row of § 3.1, as it stood after the last pass that wrote it -- the
    /// content parsed. `None` while no pass has written one.
    /// The content of the state row one store call writes, or `None` when the call is
    /// not one. Two spellings since GH #744: the first creation inserts the row, every
    /// later pass updates it under a condition on the version it read.
    fn state_content(call: &Value) -> Option<&Value> {
        match call["operation"].as_str().unwrap_or("") {
            "insert"
                if call["row"]["owner"] == "display"
                    && call["row"]["view_id"] == "screen-state" =>
            {
                Some(&call["row"]["content"])
            }
            "update"
                if call["where"]["owner"] == "display"
                    && call["where"]["view_id"] == "screen-state" =>
            {
                Some(&call["set"]["content"])
            }
            _ => None,
        }
    }

    pub async fn state(&self) -> Option<Value> {
        for bundle in self.store_bundles().await.into_iter().rev() {
            for call in bundle {
                if let Some(content) = Self::state_content(&call) {
                    let text = content.as_str().unwrap_or("{}");
                    return Some(
                        meclaw_core::serde_json::from_str(text)
                            .expect("the state row's content is JSON"),
                    );
                }
            }
        }
        None
    }

    /// Every state row the colony ever wrote, oldest first.
    pub async fn states(&self) -> Vec<Value> {
        let mut out = Vec::new();
        for bundle in self.store_bundles().await {
            for call in bundle {
                if let Some(content) = Self::state_content(&call) {
                    let text = content.as_str().unwrap_or("{}").to_string();
                    out.push(
                        meclaw_core::serde_json::from_str(&text)
                            .expect("the state row's content is JSON"),
                    );
                }
            }
        }
        out
    }

    /// The patch bundles the compose cell sent to the `web` cell, as their calls.
    pub async fn patches(&self) -> Vec<Vec<Value>> {
        self.to_child("web")
            .await
            .iter()
            .filter(|row| hop_of(row)["route"] == "patch")
            .filter_map(calls_of)
            .collect()
    }

    /// The orders that reached the clock, as `(op, schedule_id)`. `op`, `schedule_id` and
    /// `at` are body slots of the `due` emission, not hop keys.
    pub async fn orders(&self) -> Vec<(String, String)> {
        self.to_child("clock")
            .await
            .iter()
            .filter_map(|row| {
                let body = body_of(row);
                let op = body.get("op")?.as_str()?.to_string();
                Some((op, body.get("schedule_id")?.as_str()?.to_string()))
            })
            .collect()
    }

    /// The strikes the clock sent back, as their `schedule_id` (§ 4.34: the moment the
    /// curator ordered, coming back as a pass).
    pub async fn strikes(&self) -> Vec<String> {
        self.to_child("compose")
            .await
            .iter()
            .filter(|row| hop_of(row)["route"] == "in_tick")
            .filter_map(|row| Some(hop_of(row)["schedule_id"].as_str()?.to_string()))
            .collect()
    }

    /// Every event that reached the pass, as `(name, value)`. This is the one place a
    /// gesture becomes a message: the `web` cell sends it, the hive's edge carries it to
    /// `compose`. A gesture that has no wire never shows up here.
    pub async fn events_into_pass(&self) -> Vec<(String, Value)> {
        self.to_child("compose")
            .await
            .iter()
            .filter(|row| hop_of(row)["route"] == "event")
            .map(|row| {
                let event = body_of(row)["event"].clone();
                (
                    event["name"].as_str().unwrap_or("").to_string(),
                    event["value"].clone(),
                )
            })
            .collect()
    }

    /// Every write that arrived at the screen's door, as its `hop.route` -- `in_view`,
    /// `in_withdraw`, `in_notice`. What a lock counts to say "nobody wrote".
    pub async fn writes_taken(&self) -> Vec<String> {
        self.log(Some(SCREEN))
            .await
            .iter()
            .filter(|row| row.to_path == SCREEN)
            .filter_map(|row| Some(hop_of(row)["route"].as_str()?.to_string()))
            .filter(|route| route.starts_with("in_"))
            .collect()
    }

    /// The rows the store handed back the last time it was asked -- leg 0 of a write
    /// bundle, or the select of a tick. This is the table itself, read through the real
    /// store cell: what a lock uses to say a row SURVIVED, or never got there.
    pub async fn store_rows(&self) -> Vec<Value> {
        let from_store = format!("{SCREEN}/views");
        for row in self.to_child("compose").await.iter().rev() {
            if row.from_path != from_store {
                continue;
            }
            let body = body_of(row);
            let Some(first) = body["messages"].as_array().and_then(|m| m.first()) else {
                continue;
            };
            let Some(text) = first["text"].as_str() else {
                continue;
            };
            if let Ok(Value::Array(rows)) = meclaw_core::serde_json::from_str::<Value>(text) {
                return rows;
            }
        }
        Vec::new()
    }

    /// Every receipt the screen sent out of the hive, as `(error_code, view_id)`.
    pub fn receipts(&mut self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        while let Ok(m) = self.egress.try_recv() {
            if m.headers.hop.get("route").and_then(Value::as_str) != Some("receipt") {
                continue;
            }
            let Body::Inline(body) = &m.body else {
                continue;
            };
            out.push((
                body["receipt"]["error_code"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
                body["receipt"]["view_id"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
            ));
        }
        out
    }

    /// How many questions reached the judge cell.
    pub async fn judge_questions(&self) -> usize {
        self.to_child("judge").await.len()
    }

    /// Wait until `want` holds of the state row, or fail with the last one seen.
    pub async fn wait_state(&self, why: &str, want: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + MARKER;
        let mut last = Value::Null;
        loop {
            if let Some(state) = self.state().await {
                if want(&state) {
                    return state;
                }
                last = state;
            }
            if Instant::now() >= deadline {
                let dlq = self.h.drain_dead_letters().await;
                panic!("{why} did not hold within 30s; last state {last}; DLQ {dlq:?}");
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Wait until the colony has settled: nothing new in the log for `quiet`.
    ///
    /// A pass is several hops, and a lock that reads after the first of them reads half a
    /// pass. `quiet` is a settle window and never a semantic discriminator -- what the lock
    /// then asserts is the same whether the colony took 5 ms or 500.
    pub async fn settle(&self, quiet: Duration) {
        let deadline = Instant::now() + MARKER;
        let mut seen = self.log_len().await;
        loop {
            tokio::time::sleep(quiet).await;
            let now = self.log_len().await;
            if now == seen {
                return;
            }
            seen = now;
            assert!(
                Instant::now() < deadline,
                "the colony never went quiet ({seen} rows and still counting)"
            );
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The page, and the socket a gesture rides
// ─────────────────────────────────────────────────────────────────────────────

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

impl Colony {
    /// The URL of one output, or of the switch when `screen` is empty (§ 6.5).
    pub fn url(&self, screen: &str) -> String {
        format!("http://127.0.0.1:{}/{MOUNT}/{screen}", self.port)
    }

    /// Where `POST /messages` and `GET /colony/messages` answer. A DIFFERENT port from
    /// the mount listener on purpose: the mount listener is the thing under test, and a
    /// fixture that answered API routes on it would be measuring its own scaffolding.
    pub fn api_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.api_port)
    }

    /// GET one output until it answers, and hand the markup back.
    pub async fn page(&self, screen: &str) -> String {
        let url = self.url(screen);
        let deadline = Instant::now() + MARKER;
        loop {
            if let Ok(r) = reqwest::get(&url).await
                && r.status().is_success()
            {
                return r.text().await.expect("text");
            }
            assert!(Instant::now() < deadline, "{url} never served a page");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// The HTTP status one path under the mount answers with.
    pub async fn status(&self, screen: &str) -> u16 {
        reqwest::get(self.url(screen))
            .await
            .expect("the listener answers")
            .status()
            .as_u16()
    }

    /// Join the LiveView socket of one output -- the page's OWN socket, the one a gesture
    /// travels on (§ 5.6). What comes back is the socket, ready for `push`.
    pub async fn socket(&self, screen: &str) -> Ws {
        let body = self.page(screen).await;
        let marker = "data-phx-session=\"";
        let at = body
            .find(marker)
            .unwrap_or_else(|| panic!("the {screen} page carries no session token"));
        let start = at + marker.len();
        let end = start + body[start..].find('"').expect("the token is quoted");
        let token = &body[start..end];

        let topic = format!(
            "lv:{}",
            meclaw_surface::session::container_id(&format!("{SCREEN}/web"))
        );
        let (mut ws, _) = tokio_tungstenite::connect_async(format!(
            "ws://127.0.0.1:{}/{MOUNT}/live/websocket",
            self.port
        ))
        .await
        .expect("the display accepts a websocket");
        let join = json!(["1", "1", topic, "phx_join",
                          {"session": token, "url": self.url(screen)}]);
        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            join.to_string().into(),
        ))
        .await
        .expect("join sent");
        let reply = next_text(&mut ws).await;
        assert_eq!(
            reply[4]["status"],
            json!("ok"),
            "the join is answered: {reply}"
        );
        ws
    }
}

/// Push one LiveView event on an open page socket, the way the client's hook does.
pub async fn push(ws: &mut Ws, name: &str, value: Value) {
    let topic = format!(
        "lv:{}",
        meclaw_surface::session::container_id(&format!("{SCREEN}/web"))
    );
    let frame = json!(["1", "9", topic, "event",
                       {"type": "hook", "event": name, "value": value}]);
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        frame.to_string().into(),
    ))
    .await
    .expect("the event reaches the socket");
}

/// The next TEXT frame off a socket, as JSON.
pub async fn next_text(ws: &mut Ws) -> Value {
    let deadline = Instant::now() + MARKER;
    loop {
        let msg = tokio::time::timeout(MARKER, ws.next())
            .await
            .expect("the socket answers within the failure-marker window")
            .expect("the stream stays open")
            .expect("a frame");
        if let tokio_tungstenite::tungstenite::Message::Text(t) = msg {
            return meclaw_core::serde_json::from_str(&t).expect("the frame is JSON");
        }
        assert!(Instant::now() < deadline, "only binary frames came back");
    }
}

/// The hop header of one logged message.
pub fn hop_of(row: &MessageLogDto) -> Value {
    let headers: Value =
        meclaw_core::serde_json::from_str(&row.headers_json).unwrap_or_else(|_| json!({}));
    headers["hop"].clone()
}

/// The inline body of one logged message.
pub fn body_of(row: &MessageLogDto) -> Value {
    row.body_payload
        .as_deref()
        .and_then(|raw| meclaw_core::serde_json::from_str(raw).ok())
        .unwrap_or_else(|| json!({}))
}

/// The `tool_call` turns of one logged message, parsed back into calls.
pub fn calls_of(row: &MessageLogDto) -> Option<Vec<Value>> {
    let body: Value = meclaw_core::serde_json::from_str(row.body_payload.as_deref()?).ok()?;
    let turns = body["messages"].as_array()?;
    let mut out = Vec::new();
    for turn in turns {
        if turn["type"] != "tool_call" {
            continue;
        }
        if let Some(text) = turn["text"].as_str()
            && let Ok(call) = meclaw_core::serde_json::from_str::<Value>(text)
        {
            out.push(call);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// One curator value of one window, out of a state row.
pub fn curator(state: &Value, oid: &str, key: &str) -> Value {
    state["views"][oid]["curator"][key].clone()
}
