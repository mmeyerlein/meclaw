//! The throwaway display colony the five seam locks run against (befund 04 § E.2).
//!
//! Every other display lock in this tree drives `compose.py` as a subprocess and asserts
//! what it says. That covers 105 of the 107 scenarios, because the script hands its whole
//! curator state out in one emission. What it CANNOT show is the five seams around the
//! script: that the clock's order really strikes and comes back, that the judge really
//! answers and its verdict reaches the next pass, that a view really survives the
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
//! store bundles (the apps' rows and the curator's rest row), the patch bundle for the web
//! cell, the orders to the clock, the questions to the judge. Reading it is also how a lock
//! measures that NOTHING was made. Since display 2.7.0 the curator's state is memory
//! (GH #809) and no message carries it: a lock reads the screen at `web`, as the patch
//! hops folded into a tree (`tree`), never inside the cell (OR-D11).
#![allow(dead_code)]

/// The presenter's colony is a CHILD of this module: it reaches the wire the way this
/// module does, and a test does not (re-check tv-bn M7b5 A-M1/B-M1). A test names it as
/// `crate::display_colony::presenter_colony`, never through a `#[path]` of its own.
#[path = "presenter_colony.rs"]
pub mod presenter_colony;

#[path = "browser_run.rs"]
pub mod browser_run;
/// The sealed core (OR-T11-25): `Unsettled`, the settle proof, the one door to the address,
/// the boot key and the sealed decider, std only and private to this module. Its fields and
/// constructors are visible to nothing outside it, its `pub(super)` openers to this module
/// and its children only -- the COMPILER keeps every way around the barrier shut, and the
/// pin `809_a_restart…::the_sealed_core_refuses_every_way_around_it` compiles it with a
/// probe of every such way in every run.
#[path = "sealed.rs"]
mod sealed;
pub use sealed::{Named, Unsettled, Why};

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_cells::voice::VoiceCellFactory;
use meclaw_cells::web::WebCellFactory;
use meclaw_colony::api_dto::{
    MessageLogCursor, MessageLogDto, MessageLogFilter, ReadMessagesReply,
};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, SurfaceRegistry,
    bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mock_http::{CapturedRequest, MockResponse, start_mock_server_capturing};
use meclaw_testing::{surface_listener, wait_for_mount};
use tokio::sync::{mpsc, oneshot};

/// The kit's hang mark as this process scales it (`MECLAW_TEST_TIME_FACTOR`).
///
/// Re-check tv-bn M5 M1 (TV-S3, TV-Z3): every wait of this fixture goes through
/// `meclaw_testing::wait` / `deadline` under this mark and fails with `HANG:`. Five of them
/// kept a std `Instant` plus the unscaled `MARKER` and failed with "did not hold within
/// 30s" (probe of fix round M7: `the probe never holds did not hold within 30s; DLQ []`).
/// Re-check tv-bn M7 M1: the export `MARKER` (an unscaled 30 s) is gone with its last
/// users (808, gh958, gh964, gh867, 710); they wait through the kit as well.
fn hang_mark() -> Duration {
    meclaw_testing::deadline::scaled(meclaw_testing::deadline::HANG)
}

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

/// The owner of the code view a lock puts on the screen: the layout cell of a grown
/// `colony-view` (the owner half of the view's identity).
pub const COLONY_VIEW: &str = "/alex/apps/colony-view/layout";

/// One emission of the shipped `colony-view` layout over a two-cell snapshot, owned by
/// [`COLONY_VIEW`]: the code view the display shows by default (`params.code_views`),
/// whose shell carries its client as an inline script (re-check tv-bn M8, M1). Run from
/// `templates/colony-view/layout/layout.py`, the source `config.json` carries byte for byte.
pub fn colony_view_emission() -> Value {
    use std::io::Write;
    let doc = json!({
        "body": {"graph": {
            "scope": "/os",
            "nodes": [
                {"path": "/os/one", "cell_type": "code"},
                {"path": "/os/two", "cell_type": "store"}
            ],
            "edges": [{"from": "/os/one", "to": "/os/two"}]
        }},
        "envelope": {"target": COLONY_VIEW}
    });
    let mut child = std::process::Command::new("python3")
        .arg(repo("templates/colony-view/layout/layout.py"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("python3 starts");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(doc.to_string().as_bytes())
        .expect("the snapshot reaches the layout");
    let out = child.wait_with_output().expect("the layout ends");
    assert!(
        out.status.success(),
        "the colony-view layout failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    meclaw_core::serde_json::from_slice(&out.stdout).expect("the layout answers one emission")
}

pub fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

pub fn write_json(p: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("dirs");
    std::fs::write(
        p,
        meclaw_core::serde_json::to_string_pretty(v).expect("json"),
    )
    .expect("write");
}

pub fn patch_json(p: &std::path::Path, f: impl FnOnce(&mut Value)) {
    let mut v = read_json(p);
    f(&mut v);
    write_json(p, &v);
}

pub fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
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
# ONE act. Each write is its own pass on the one state in the curator's memory (GH #809),
# so a breath of three writes keeps all three windows.
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
///
/// **No raw door.** The handle, the egress, the judge's captures and the surface registry
/// live in the private `wire`; a test reaches the colony only through the readers below,
/// which settle before they read, and through [`Unsettled`], which opens only with a named
/// [`Why`], and to its address only through [`Colony::url`]/[`Colony::api_url`]/
/// [`Colony::base`] with a named [`Site`].
///
/// The guarantee is the compiler, and it is TESTED: the old `compile_fail` block here never
/// ran (integration-test doctests do not); the pin
/// `809_a_restart…::the_sealed_core_refuses_every_way_around_it` compiles the sealed core
/// with a positive control and every way around it in every run of its binary (OR-T11-25,
/// re-check tv-bn M7b11 A-M1 i/A-M2/A-M3).
pub struct Colony {
    _td: tempfile::TempDir,
    /// Every raw door onto the colony, behind the module boundary.
    w: wire::Wire,
    /// The colony's address: the mount listener and the API port, behind the one door
    /// (fixture readers against a settle proof, users through a named [`Site`]).
    door: sealed::Door,
    _listener: tokio::task::JoinHandle<()>,
    _api: tokio::task::JoinHandle<()>,
    _judge: tokio::task::JoinHandle<()>,
}

/// The event "the message log moved" (`ColonyHandle::subscribe_log_seq`, GH #1208 TV-A3).
#[derive(Clone)]
pub struct LogChanges(tokio::sync::watch::Receiver<u64>);

impl LogChanges {
    /// Until the log writer commits again.
    pub async fn changed(&mut self) {
        self.0
            .changed()
            .await
            .expect("the colony's log writer is alive");
    }

    /// The writer's commit counter as it stands now -- for a hang's last state.
    pub fn at(&self) -> Unsettled<u64> {
        Unsettled::new(*self.0.borrow())
    }
}

/// The one place that holds the colony's raw doors (re-check tv-bn M7b5 A-M1/B-M1).
///
/// Private to the fixture: the rest of this module and `presenter_colony` reach the colony
/// through a [`wire::Reader`], built either SETTLED ([`wire::Wire::settled`]) or wrapped
/// in [`Unsettled`] ([`wire::Wire::unsettled`]); writes and events have their own verbs.
/// There is no generic inbox send: a `ReadMessages` from outside would be a read past the
/// readers.
mod wire {
    use std::sync::Arc;

    use meclaw_colony::api_dto::{DeadLetterDto, MessageLogDto, MessageLogFilter};
    use meclaw_colony::{ColonyMsg, MutationOutcome, SurfaceRegistry};
    use meclaw_core::serde_json::Value;
    use meclaw_core::{Message, Uuid};
    use meclaw_testing::ColonyHandle;
    use meclaw_testing::mock_http::CapturedRequest;
    use tokio::sync::{mpsc, oneshot};

    use super::sealed::{Settled, barrier};
    use super::{SCREEN, Unsettled, calls_of, fold, hang_mark, hop_of};

    /// The raw doors. Every field is private, so only this module touches them.
    pub(super) struct Wire {
        h: ColonyHandle,
        egress: tokio::sync::Mutex<mpsc::Receiver<Message>>,
        judge_asks: Arc<tokio::sync::Mutex<Vec<CapturedRequest>>>,
        surfaces: Arc<SurfaceRegistry>,
    }

    impl Wire {
        pub(super) fn new(
            h: ColonyHandle,
            egress: mpsc::Receiver<Message>,
            judge_asks: Arc<tokio::sync::Mutex<Vec<CapturedRequest>>>,
            surfaces: Arc<SurfaceRegistry>,
        ) -> Self {
            Wire {
                h,
                egress: tokio::sync::Mutex::new(egress),
                judge_asks,
                surfaces,
            }
        }

        /// One message into the colony, sender `/`.
        pub(super) async fn send(&self, msg: Message) {
            self.h.send(msg).await;
        }

        /// The next message that left `/alex` marked: the event itself, `None` once the
        /// egress closed.
        pub(super) async fn next_out(&self) -> Option<Message> {
            self.egress.lock().await.recv().await
        }

        /// The event "the message log moved".
        pub(super) fn log_changes(&self) -> super::LogChanges {
            super::LogChanges(self.h.subscribe_log_seq())
        }

        /// Shut the colony down.
        pub(super) async fn shutdown(self) {
            self.h.shutdown().await;
        }

        /// `ColonyMsg::RescanTemplates`: the template table filled from `root`.
        pub(super) async fn rescan_templates(
            &self,
            root: std::path::PathBuf,
        ) -> Result<(), String> {
            let (ack_tx, ack_rx) = oneshot::channel();
            self.h
                .inbox_tx
                .send(ColonyMsg::RescanTemplates {
                    templates_root: root,
                    ack: ack_tx,
                })
                .await
                .expect("the colony is up");
            ack_rx.await.expect("the scan answers")
        }

        /// `ColonyMsg::Mutation` of `payload`, under a fresh trace; the outcome comes back.
        pub(super) async fn mutate(&self, payload: Value) -> MutationOutcome {
            let (ack_tx, ack_rx) = oneshot::channel();
            self.h
                .inbox_tx
                .send(ColonyMsg::Mutation {
                    payload,
                    reply_to: None,
                    trace_id: Uuid::now_v7(),
                    parent_message_id: Uuid::now_v7(),
                    ack: ack_tx,
                })
                .await
                .expect("the colony is up");
            ack_rx.await.expect("the mutation answers")
        }

        /// The quiescence barrier (`ColonyMsg::Settle`, GH #1208 TV-A2), and on a hang the
        /// colony's last state.
        ///
        /// **Why not `ColonyHandle::quiesce`.** It waits under `deadline::hang`, whose hang
        /// says only "HANG: the colony settles (waited 30 s)" -- no dead letter, no hop. And
        /// not `hang_with` either: its last state is a SYNC closure, and the colony's state
        /// is read by asking the colony (re-check tv-bn M7b5 B-M2). So the barrier is sent
        /// here, and a hang reads the dead letters (not draining) and the newest log rows
        /// asynchronously, each under its own hang mark, before it fails.
        pub(super) async fn settle(&self) -> Settled {
            // The proof is minted only by the barrier itself (`sealed::barrier`, re-check
            // tv-bn M7b12 A-M1): this function holds the hang mark around it, nothing more.
            match tokio::time::timeout(hang_mark(), barrier::settle(&self.h)).await {
                Ok(Some(settled)) => settled,
                Ok(None) => panic!("the colony settles: the colony ended before answering"),
                Err(_) => {
                    let state = self.last_state().await;
                    panic!(
                        "HANG: the colony settles (waited {} s); last state: {state}",
                        hang_mark().as_secs()
                    );
                }
            }
        }

        /// The last state of a colony that did not settle: its dead letters (read, not
        /// drained) and the newest log rows, oldest of them first.
        async fn last_state(&self) -> String {
            const ROWS: usize = 12;
            let reader = Reader(self);
            let dlq = meclaw_testing::deadline::hang(
                "the dead letters answer for the last state",
                reader.read_dead_letters(),
            )
            .await;
            let rows = meclaw_testing::deadline::hang(
                "the message log answers for the last state",
                reader.newest(ROWS),
            )
            .await;
            let lines: Vec<String> = rows
                .iter()
                .map(|r| {
                    let hop = hop_of(r);
                    format!(
                        "{} -> {} route={} error_code={}",
                        r.from_path, r.to_path, hop["route"], hop["error_code"]
                    )
                })
                .collect();
            let dead: Vec<String> = dlq
                .iter()
                .map(|d| {
                    format!(
                        "{} -> {} error_code={} trace={}",
                        d.sender_path, d.resolved_target, d.error_code, d.trace_id
                    )
                })
                .collect();
            // One read of the queue serves at most `DLQ_READ` letters and has no cursor: a
            // full read says so instead of standing for the whole queue (K8 class "a page
            // for the whole", tv-bn M7b16).
            let more = if dlq.len() >= super::DLQ_READ {
                format!(" (one read of {}: more letters stand)", super::DLQ_READ)
            } else {
                String::new()
            };
            format!(
                "DLQ{more} {dead:?}; the last {} log rows: {lines:#?}",
                lines.len()
            )
        }

        /// Settle, then a reader of the colony at rest.
        pub(super) async fn settled(&self) -> Reader<'_> {
            self.settle().await;
            Reader(self)
        }

        /// A reader WITHOUT the barrier, opened only with a named [`super::Why`].
        pub(super) fn unsettled(&self) -> Unsettled<Reader<'_>> {
            Unsettled::new(Reader(self))
        }
    }

    /// The colony's readers. The field is private: only [`Wire`] builds one, settled or
    /// wrapped in [`Unsettled`].
    pub(super) struct Reader<'a>(&'a Wire);

    impl Reader<'_> {
        /// The colony's `message_log` under `to_prefix`, oldest first: the WHOLE of it,
        /// read page by page like [`Reader::whole_log`] (re-check tv-bn M7b12 C-M2 -- one
        /// page of 1000 was a window, and a full page read as "nothing more"), every body
        /// inline ([`super::inline_bodies`], C-M1).
        pub(super) async fn log(&self, to_prefix: Option<&str>) -> Vec<MessageLogDto> {
            let mut rows = self
                .0
                .h
                .read_log(MessageLogFilter {
                    to_path_prefix: to_prefix.map(str::to_string),
                    ..Default::default()
                })
                .await;
            rows.reverse(); // the reply is newest first; a lock reads a colony forwards
            super::inline_bodies(&rows);
            rows
        }

        /// The WHOLE `message_log`, oldest first, read page by page
        /// (`ColonyHandle::read_log`): no page cap cuts it.
        pub(super) async fn whole_log(&self) -> Vec<MessageLogDto> {
            let mut rows = self.0.h.read_log(MessageLogFilter::default()).await;
            rows.reverse(); // newest first on the wire
            super::inline_bodies(&rows);
            rows
        }

        /// The newest `n` rows of the log, oldest of them first.
        async fn newest(&self, n: usize) -> Vec<MessageLogDto> {
            let (ack_tx, ack_rx) = oneshot::channel();
            let asked = self
                .0
                .h
                .inbox_tx
                .send(ColonyMsg::ReadMessages {
                    filter: MessageLogFilter {
                        limit: n,
                        scan_budget: 50_000,
                        ..Default::default()
                    },
                    ack: ack_tx,
                })
                .await;
            if asked.is_err() {
                return Vec::new();
            }
            let mut rows = ack_rx.await.map(|r| r.entries).unwrap_or_default();
            rows.reverse();
            rows
        }

        /// Every row of one trace, oldest first.
        pub(super) async fn trace(&self, trace_id: &str) -> Vec<MessageLogDto> {
            // Paged like `log` (re-check tv-bn M7b12 C-M2, same class): no page cap cuts
            // a long trace.
            let mut rows = self
                .0
                .h
                .read_log(MessageLogFilter {
                    trace_id: Some(trace_id.to_string()),
                    ..Default::default()
                })
                .await;
            rows.reverse(); // newest first on the wire; a hop chain reads forwards
            super::inline_bodies(&rows);
            rows
        }

        /// Every message the screen's `<child>` received so far, oldest first.
        pub(super) async fn to_child(&self, child: &str) -> Vec<MessageLogDto> {
            self.log(Some(&format!("{SCREEN}/{child}"))).await
        }

        /// The store bundles the compose cell sent to `views` so far, as their calls.
        pub(super) async fn store_bundles(&self) -> Vec<Vec<Value>> {
            self.to_child("views")
                .await
                .iter()
                .filter_map(calls_of)
                .collect()
        }

        /// The patch bundles the compose cell sent to `web` so far, as their calls.
        pub(super) async fn patches(&self) -> Vec<Vec<Value>> {
            super::patches_of(&self.to_child("web").await)
        }

        /// The tree the patches to `web` built so far, folded.
        pub(super) async fn tree(&self) -> Value {
            let mut held = meclaw_core::serde_json::Map::new();
            for calls in self.patches().await {
                fold(&mut held, &calls);
            }
            Value::Object(held)
        }

        /// How many store bundles put a row of `(owner, view_id)` into the table so far.
        pub(super) async fn writes_of(&self, owner: &str, view_id: &str) -> usize {
            self.store_bundles()
                .await
                .iter()
                .filter(|calls| {
                    calls.iter().any(|c| {
                        c["operation"] == "insert"
                            && c["row"]["owner"] == owner
                            && c["row"]["view_id"] == view_id
                    })
                })
                .count()
        }

        /// Mark the message log as it stands (`ColonyHandle::log_mark`).
        pub(super) async fn mark_log(&self) -> meclaw_testing::LogMark {
            self.0.h.log_mark().await
        }

        /// The rows matching `filter` committed after `mark`, newest first.
        pub(super) async fn rows_since(
            &self,
            mark: &meclaw_testing::LogMark,
            filter: MessageLogFilter,
        ) -> Vec<MessageLogDto> {
            // A log reader like `log` (re-check tv-bn M7b12 C-M1, sibling found in M7b14):
            // a body held as a blob is red here too, not a row a lock reads past.
            let rows = self.0.h.log_since(mark, filter).await;
            super::inline_bodies(&rows);
            rows
        }

        /// Read AND drain the dead-letter queue (`ColonyMsg::DrainDeadLetters`).
        pub(super) async fn take_dead_letters(&self) -> Vec<meclaw_colony::DeadLetter> {
            self.0.h.drain_dead_letters().await
        }

        /// Read the dead-letter queue without draining it (`ColonyMsg::ReadDeadLetters`).
        pub(super) async fn read_dead_letters(&self) -> Vec<DeadLetterDto> {
            let (ack, rx) = oneshot::channel();
            let asked = self
                .0
                .h
                .inbox_tx
                .send(ColonyMsg::ReadDeadLetters {
                    since: None,
                    error_code: None,
                    limit: super::DLQ_READ,
                    ack,
                })
                .await;
            if asked.is_err() {
                return Vec::new();
            }
            rx.await.map(|r| r.entries).unwrap_or_default()
        }

        /// Every message waiting on the egress, drained.
        pub(super) async fn drain_egress(&self) -> Vec<Message> {
            let mut egress = self.0.egress.lock().await;
            let mut out = Vec::new();
            while let Ok(m) = egress.try_recv() {
                out.push(m);
            }
            out
        }

        /// Every question the judge cell put to its provider, in arrival order.
        pub(super) async fn judge_asks(&self) -> Vec<CapturedRequest> {
            self.0.judge_asks.lock().await.clone()
        }
    }
}

/// One read of the dead-letter queue: the colony's cap (`handle_read_dead_letters` clamps
/// `limit` to 1..=1000); the queue has no cursor, so a full read is a window.
const DLQ_READ: usize = meclaw_testing::DEAD_LETTERS_PAGE;

/// The page `GET /colony/messages` asks for and judges by (re-check tv-bn M7b14 B-M3): the
/// colony's own cap of a `ReadMessages` read (`handle_read_messages` clamps `limit` to
/// 1..=1000), one constant for the request and its guard.
pub(crate) const MESSAGES_PAGE: usize = meclaw_testing::MESSAGES_PAGE;

/// The scan budget of one page of the door: the colony's own cap (`handle_read_messages`
/// clamps `scan_budget` to 1..=50_000); a cut scan is refused by [`next_page`].
const MESSAGES_SCAN: usize = 50_000;

/// What one reply of a `ReadMessages` read with `limit: page` means for a reader of the
/// whole log, as a pure function (re-check tv-bn M7b14 B-M3): `Ok(Some(cursor))` -- read
/// the next page; `Ok(None)` -- this was the last one; `Err` -- the reply stands for a
/// window, never for the whole: a cut scan, or a full page without a cursor.
pub(crate) fn next_page(
    reply: &ReadMessagesReply,
    page: usize,
) -> Result<Option<MessageLogCursor>, &'static str> {
    if reply.scan_truncated {
        return Err("the log outgrew one scan: a count would read a window");
    }
    match &reply.next {
        Some(cursor) => Ok(Some(cursor.clone())),
        None if reply.entries.len() >= page => {
            Err("a full page without a cursor: a count would read a window")
        }
        None => Ok(None),
    }
}

/// The WHOLE `message_log` through `ColonyMsg::ReadMessages`, newest first, page by page
/// on the colony's cursor -- the horizon of `ColonyHandle::read_log`, read by the door
/// `GET /colony/messages` (re-check tv-bn M7b14 B-M3 and the M7b15 find: the colony serves
/// at most [`MESSAGES_PAGE`] rows a read, so one read of a longer log is a window). Every
/// reply is judged by [`next_page`]; a window is an error, never a count.
pub(crate) async fn whole_messages(
    inbox: &mpsc::Sender<ColonyMsg>,
) -> Result<Vec<MessageLogDto>, (axum::http::StatusCode, &'static str)> {
    let mut filter = MessageLogFilter {
        limit: MESSAGES_PAGE,
        scan_budget: MESSAGES_SCAN,
        ..Default::default()
    };
    let mut entries = Vec::new();
    loop {
        let (ack_tx, ack_rx) = oneshot::channel();
        if inbox
            .send(ColonyMsg::ReadMessages {
                filter: filter.clone(),
                ack: ack_tx,
            })
            .await
            .is_err()
        {
            return Err((axum::http::StatusCode::SERVICE_UNAVAILABLE, "colony gone"));
        }
        let Ok(Ok(reply)) = tokio::time::timeout(hang_mark(), ack_rx).await else {
            return Err((
                axum::http::StatusCode::GATEWAY_TIMEOUT,
                "the log did not answer",
            ));
        };
        let next = next_page(&reply, MESSAGES_PAGE);
        entries.extend(reply.entries);
        match next {
            Err(why) => return Err((axum::http::StatusCode::INTERNAL_SERVER_ERROR, why)),
            Ok(Some(cursor)) => filter.before = Some(cursor),
            Ok(None) => return Ok(entries),
        }
    }
}

/// The reply of one `ReadMessages` read, for the door's unit pin in 809 (a test file names
/// the colony's read types only through the fixture).
pub(crate) type PageReply = ReadMessagesReply;

/// Test data for the door's red line (tv-bn M7b16): `rows` messages routed through the
/// colony inbox to a hive marker of their own (`/seed`, a hive hop is a logged hop), so the
/// colony's own writer puts every row into its log; returns their ids once the writer
/// committed them (`quiesce`). A write of test data, never a read: the read under test
/// goes through the colony inbox (tv-bn M7b19: no path or SQL into the colony's database,
/// TV-A3).
pub(crate) async fn seed_messages(h: &ColonyHandle, rows: usize) -> Vec<String> {
    h.add_hive_scope(Path::new("/seed")).await;
    let mut ids = Vec::with_capacity(rows);
    for _ in 0..rows {
        let msg = MessageBuilder::new(Path::new("/seed"))
            .body(Body::Inline(json!({})))
            .ttl(8)
            .build();
        ids.push(msg.id.to_string());
        h.send(msg).await;
    }
    h.quiesce().await;
    ids
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
/// **What it is not.** No auth, no paging for the caller (the door reads every page
/// itself), no filters, no other route. A proof that needs
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
    /// The door reads the WHOLE log, page by page on the colony's cursor, the same horizon
    /// as `ColonyHandle::read_log`. A count of one page saturates: the colony serves at
    /// most [`MESSAGES_PAGE`] rows a read whatever `limit` asks for (`handle_read_messages`
    /// clamps it), and a proof that asks "did the log grow" then reads the same number
    /// before and after -- measured: B-26 failed in WebKit with `log_before: 1000,
    /// log_after: 1000`. The 100_000 this door asked for before was clamped to that page
    /// (re-check tv-bn M7b14 B-M3): [`next_page`] judges every reply.
    async fn get_messages(
        State(inbox): State<mpsc::Sender<ColonyMsg>>,
        axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
    ) -> axum::response::Response {
        let entries = match whole_messages(&inbox).await {
            Ok(entries) => entries,
            Err((status, why)) => return (status, why).into_response(),
        };
        // `?route=hold` counts only the rows whose hop names that route. A proof
        // that asks "did my gesture reach the colony" cannot use the whole log:
        // the clock strikes into it while the browser is looking, and the newest
        // row is a different one a second later whatever the finger did.
        let route = q.get("route").cloned();
        let named = route.as_ref().map(|r| {
            let needle = format!("\"route\":\"{r}\"");
            entries
                .iter()
                .filter(|e| e.headers_json.contains(&needle))
                .count()
        });
        // `?event=tap` counts the GESTURES of that name. A gesture is no route of
        // its own: the `web` cell sends every one on `event` with its name in
        // `body.event.name` (`events_into_pass`), so `?route=tap` or
        // `?route=hold` read 0 whatever the finger did -- measured: the TV-A1
        // marker of B-01/B-14 read `route:tap=0` for 30 s after the tap, in all
        // twenty lines of L1 and L3 (welle-tv tv-bn-layout A').
        // `&for=<oid>` narrows them to the gestures for one window: a TV-A1
        // marker taps a `for` no other tap carries and waits on ITS line, not
        // on any tap count that moved (re-check tv-bn-layout A fix1 H1).
        let event = q.get("event").cloned();
        let window = q.get("for").cloned();
        // `&seq=<n>` narrows them to the ONE tap a driver stamped with that seq
        // (`phx-value-seq`): whether that tap reached the colony at all, which
        // a count of a window's taps cannot say once the log has cut its front.
        let seq = q.get("seq").cloned();
        // One row per gesture: the hop INTO the pass, where a gesture becomes a
        // message (`events_into_pass`). An event the pass sends on travels the
        // hives' `event` lanes as further rows -- measured: B-26's one Enter read
        // `typed` five times while every row was counted (tv-bn M10h).
        let pass = format!("{SCREEN}/compose");
        let gestures = event.as_ref().map(|n| {
            entries
                .iter()
                .filter(|e| e.headers_json.contains("\"route\":\"event\""))
                .filter(|e| e.to_path == pass)
                .filter(|e| body_of(e)["event"]["name"].as_str() == Some(n.as_str()))
                .filter(|e| {
                    window.as_ref().is_none_or(|w| {
                        body_of(e)["event"]["value"]["for"].as_str() == Some(w.as_str())
                    })
                })
                .filter(|e| {
                    seq.as_ref().is_none_or(|n| {
                        let v = &body_of(e)["event"]["value"]["seq"];
                        v.as_str() == Some(n.as_str())
                            || v.as_u64().map(|x| x.to_string()).as_deref() == Some(n.as_str())
                    })
                })
                .count()
        });
        axum::Json(json!({
        "messages": entries.len(),
        "named": named,
        "gestures": gestures,
        // The NEWEST row's id, and it is what a proof should compare. A count
        // saturates -- the log of this colony stops growing at its retention
        // limit, and a proof that asks "did the log grow" then reads the same
        // number before and after (measured: B-26 red in WebKit with
        // `log_before: 1000, log_after: 1000`). A uuid7 only ever moves forward.
        "newest": entries.first().map(|r| r.id.clone())}))
        .into_response()
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
    boot_inner(opts, false, None).await
}

/// [`boot`], plus a `voice` cell with the `echo` provider on the mount the screen's
/// microphone joins (`voice`, the shipped `voice_mount`).
///
/// Its own entry point rather than a field of [`Boot`]: several locks spell `Boot` out
/// field by field, and only a proof that holds the mark in a real browser needs something
/// behind it (GH #867). Echo, because it needs no credential and sends every frame straight
/// back -- the page then has audio to count on both sides of its own socket.
pub async fn boot_with_voice_echo(opts: Boot) -> Colony {
    boot_inner(opts, true, None).await
}

/// [`boot`], plus whatever `grow` writes into the tree before the colony boots (GH #959:
/// the presenter locks put an app hive under `/alex/apps` and rewire the two levels
/// above it). `grow` gets the colony root; the shipped screen is already in place.
///
/// Private (re-check tv-bn M7b9 B-M1): the root is the way to `colony.db` and every
/// `cell.db` past the wire, so only the fixture and its child `presenter_colony` grow a
/// tree; a test that needs its own growth gets a verb here, never the root. It takes the
/// fixture's [`sealed::Key`] (OR-T11-25), so it stays shut even if it were `pub`.
async fn boot_with(_key: sealed::Key, opts: Boot, grow: impl FnOnce(&std::path::Path)) -> Colony {
    boot_inner(opts, false, Some(Box::new(grow))).await
}

/// The tree hook of [`boot_with`].
type Grow<'a> = Box<dyn FnOnce(&std::path::Path) + 'a>;

/// The colony's tree before it boots, as a test may grow it: writes under a path relative
/// to the root, never the root itself (re-check tv-bn M7b9 B-M1 -- the root would be the
/// way to `colony.db` and every `cell.db` once the colony runs).
pub struct Tree<'r>(&'r std::path::Path);

impl Tree<'_> {
    /// The root joined with `rel`, which stays under the root (no `..`, not absolute).
    fn under(&self, rel: &str) -> std::path::PathBuf {
        let rel = std::path::Path::new(rel);
        assert!(
            rel.components()
                .all(|c| matches!(c, std::path::Component::Normal(_))),
            "a tree path stays under the colony root: {}",
            rel.display()
        );
        self.0.join(rel)
    }

    /// [`patch_json`] on the file `rel` under the colony root.
    pub fn patch_json(&self, rel: &str, f: impl FnOnce(&mut Value)) {
        patch_json(&self.under(rel), f);
    }

    /// [`copy_tree`] from `src` to `rel` under the colony root.
    pub fn copy_tree(&self, src: &std::path::Path, rel: &str) {
        copy_tree(src, &self.under(rel));
    }
}

/// [`boot`], plus whatever `grow` writes into the tree before the colony boots, through a
/// [`Tree`] that never hands out the root (GH #964: a setting of the compose cell).
pub async fn boot_growing(opts: Boot, grow: impl FnOnce(&Tree<'_>)) -> Colony {
    boot_with(sealed::Key::new(), opts, move |root| grow(&Tree(root))).await
}

async fn boot_inner(opts: Boot, voice_echo: bool, grow: Option<Grow<'_>>) -> Colony {
    let (judge_addr, judge_join, judge_asks) =
        start_mock_server_capturing(vec![canned(&opts.verdict.to_string())]).await;

    let td = tempfile::TempDir::new().expect("tempdir");
    let root = td.path();
    write_json(&root.join("colony.json"), &json!({"schema_version": 1}));

    // The root: the world's timer keeps `/alex` connected (GH #265), and whatever leaves
    // `/alex` dies at the root onto this module's channel, marked on the way.
    let mut mark = meclaw_core::serde_json::Map::new();
    mark.insert(MARK.to_string(), json!("'1'"));
    let mut edges = vec![
        json!({"from": "./world", "to": "./alex"}),
        json!({"from": "./alex", "to": ".", "modifier": {"set_context": mark}}),
    ];
    if voice_echo {
        // A cell no edge reaches is never spawned, and a voice cell that never spawns
        // registers no mount (measured: `no cell registered the mount "voice"`). The two
        // edges `gh643_*` draws for its voice cell: a lane in, and the way out to the root.
        edges.push(json!({"from": ".", "to": "./voice",
                          "condition": "has(hop.route) && hop.route == 'in_speak'"}));
        edges.push(json!({"from": "./voice", "to": "."}));
    }
    write_json(
        &root.join("main/config.json"),
        &json!({
            "cell": {"type": "hive"},
            "params": {"graph": {"edges": edges}}
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
                // Measured 27.09. with retries off: 0 lost writes in 180 stage rebuilds
                // (120 without browsers, 60 between browser lines), GH #714 closed.
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
    // The display refs `web@2.3.0`, and a ref resolves against the templates table, which
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

    if voice_echo {
        // Outside `/alex`, with only the two edges drawn above (a lane in, the way out to
        // the root) so the cell spawns: the echo provider says nothing on the topology
        // side (no partial, no turn), and a microphone proof only needs the mount.
        write_json(
            &root.join("main/voice/config.json"),
            &json!({
                "cell": {"type": "voice", "timeout": -1},
                "params": {"mount": "voice", "release_grace_ms": 0,
                           "stt": {"provider": "echo"}},
                // The contract the shipped `voice` template declares, minus what a
                // fixture has no use for (the same cut `gh643_*` makes).
                "contract": {
                    "version": "1.0.0", "settings": {},
                    "ingress": {"context": ["session_id"]},
                    "emits": {
                        "body": {"messages": {"type": "array", "required": true}},
                        "hop": {
                            "route": {"type": "string", "required": false},
                            "session_id": {"type": "string", "required": false},
                            "call_id": {"type": "string", "required": false},
                            "turn_id": {"type": "string", "required": false},
                            "platform": {"type": "string", "required": false},
                            "mode": {"type": "string", "required": false},
                            "eager": {"type": "boolean", "required": false},
                            "error_code": {"type": "string", "required": false}
                        }
                    },
                    "consumes": {
                        "body": {"messages": {"type": "array", "required": true}},
                        "context": {
                            "call_id": {"type": "string", "required": false},
                            "session_id": {"type": "string", "required": false}
                        }
                    }
                }
            }),
        );
    }

    if let Some(grow) = grow {
        grow(root);
    }

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
        (
            "voice".to_string(),
            Arc::new(VoiceCellFactory::new(Arc::clone(&surfaces))),
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
    if voice_echo {
        wait_for_mount(&surfaces, "voice").await;
    }
    let (addr, listener) = surface_listener(Arc::clone(&surfaces)).await;
    let (api_port, api) = api_listener(h.inbox_tx.clone()).await;
    // Boot is the last place the handle is held in the open; from here on it is the wire's.
    Colony {
        _td: td,
        w: wire::Wire::new(h, egress, judge_asks, surfaces),
        door: sealed::Door::new(addr.port(), api_port),
        _listener: listener,
        _api: api,
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
    /// Put one window up, owned by `owner`. The trace id of the write comes back.
    pub async fn write_view(&self, owner: &str, view_id: &str, props: Value) -> String {
        self.write_view_ttl(owner, view_id, props, 0).await
    }

    /// `write_view` with a `ttl_ms` on the write (§ 4.34). The trace id of the write comes
    /// back, so a wait that runs dry can name the hop where the write stopped (GH #714).
    pub async fn write_view_ttl(
        &self,
        owner: &str,
        view_id: &str,
        props: Value,
        ttl_ms: i64,
    ) -> String {
        // `pane_id` is the DOM id of the window, and the dock's `data-for` is that id
        // (compose.py, TILE_TEMPLATE). A window written without one has no id in the
        // DOM at all, and a browser proof that follows a tile to its window finds
        // nothing; a real application always writes one.
        let mut props = props;
        if let Some(map) = props.as_object_mut() {
            map.entry("pane_id")
                .or_insert_with(|| json!(format!("pane-{view_id}")));
        }
        let msg = to_screen(
            "in_view",
            owner,
            json!({"view_id": view_id, "region": "main", "kind": "component",
                   "content": window(props), "components": [], "ttl_ms": ttl_ms,
                   "messages": []}),
        );
        let trace = msg.trace_id.to_string();
        self.w.send(msg).await;
        trace
    }

    /// Put one window up and WAIT until the pass has drawn it; the folded tree comes back.
    ///
    /// Read at the receivers (GH #809): the write's store bundle reached `views` (its mark
    /// names this window), the window stands in the tree the `patch` hops built at `web`,
    /// and the colony went quiet -- so the pass that took the write has drawn.
    pub async fn put(&self, owner: &str, view_id: &str, props: Value) -> Value {
        self.settle().await;
        self.put_ttl(owner, view_id, props, 0).await
    }

    /// `put` with a `ttl_ms` on the write (§ 4.34).
    pub async fn put_ttl(&self, owner: &str, view_id: &str, props: Value, ttl_ms: i64) -> Value {
        let before = self.w.settled().await.writes_of(owner, view_id).await;
        let trace = self.write_view_ttl(owner, view_id, props, ttl_ms).await;
        self.drawn_after(owner, view_id, before, &trace).await
    }

    /// Tell the app stand-in to act, and WAIT until the pass has drawn the window
    /// `view_id`. An act may write more than one view (§ 9.1); this waits for one of them.
    pub async fn app_put(&self, cmd: Value, view_id: &str) -> Value {
        let before = self.w.settled().await.writes_of(PROBE, view_id).await;
        let trace = self.app(cmd).await;
        self.drawn_after(PROBE, view_id, before, &trace).await
    }

    /// How many store bundles put a row of `(owner, view_id)` into the table: an `insert`
    /// leg of that row, read where the store took it.
    pub async fn writes_of(&self, owner: &str, view_id: &str) -> usize {
        self.w.settled().await.writes_of(owner, view_id).await
    }

    /// Wait until a write of `(owner, view_id)` beyond the first `before` reached the store
    /// and its window stands in the tree, with the colony quiescent in between.
    ///
    /// Both waits run under the kit's hang mark (`wait::until_some_with`), and a lost write
    /// fails with `HANG:` and its account as the last state ([`Self::lost_write`]).
    /// Re-check tv-bn M7 M2: the caller-chosen window of `try_drawn_after` (3 s for the
    /// lost-write lock) was sleep as synchronisation and is gone; that lock waits for the
    /// command's hop and quiescence and reads the account then.
    ///
    /// Re-check tv-bn M7b5 B-M2: the account of a hang is read WITHOUT the barrier
    /// ([`Self::lost_write_now`]). A settle inside the failure path waited a second hang mark
    /// on the very colony that just did not move, and the account came after 2 x HANG.
    // no-settle: polls for the write's positive events (Why::Watch), the account of a hang is Why::Report; hands back the tree read at rest
    async fn drawn_after(&self, owner: &str, view_id: &str, before: usize, trace: &str) -> Value {
        let oid = self.oid(owner, view_id);
        meclaw_testing::wait::until_some_with(
            &format!("the write of {oid} reaches the store"),
            move || async move {
                let r = self.w.unsettled().because(Why::Watch);
                (r.writes_of(owner, view_id).await > before).then_some(())
            },
            || async {
                self.lost_write_now("the write never reached the store", owner, view_id, trace)
                    .await
                    .because(Why::Report)
            },
        )
        .await;
        self.settle().await;
        let id = oid.as_str();
        meclaw_testing::wait::until_some_with(
            &format!("the window {oid} stands on the screen"),
            move || async move {
                let tree = self.w.unsettled().because(Why::Watch).tree().await;
                tree.get(id).is_some().then_some(())
            },
            || async {
                self.lost_write_now("the window is not in the tree", owner, view_id, trace)
                    .await
                    .because(Why::Report)
            },
        )
        .await;
        // Re-check tv-bn M7b4 A-M1: the tree handed back is read at rest, not at the event.
        self.tree().await
    }

    /// The account of a lost write (GH #714): DLQ, the hop chain of its trace, the last hop,
    /// what the store took and whether the window stands in the tree.
    ///
    /// **Why it names a hop.** GH #714: in 1 of 5 rebuilds (1 of 12 once the stand-in ran
    /// cold) one write of a round never arrived -- no dead letter, every other write of the
    /// round there -- and the wait said only "never reached the store". The owner asked on
    /// 17.09. for the stand-in's own message log of that rebuild. So the account reads the
    /// write's trace back out of the colony's log (every hop of it, `from_path -> to_path`
    /// with its `hop.route`, `ttl` and `hop.error_code`) and says which receiver the last
    /// hop reached, plus the store count and whether the window stands in the tree.
    pub async fn lost_write(&self, why: &str, owner: &str, view_id: &str, trace: &str) -> String {
        let r = self.w.settled().await;
        self.account_of_write(&r, why, owner, view_id, trace).await
    }

    /// [`Self::lost_write`] WITHOUT the barrier, for the failure path of a wait: the colony
    /// that did not move is read as it stands (re-check tv-bn M7b5 B-M2).
    // no-settle: the account of a hang, read inside the failing wait; the text is a report, never a verdict
    async fn lost_write_now(
        &self,
        why: &str,
        owner: &str,
        view_id: &str,
        trace: &str,
    ) -> Unsettled<String> {
        let r = self.w.unsettled().because(Why::Report);
        Unsettled::new(self.account_of_write(&r, why, owner, view_id, trace).await)
    }

    /// The account of GH #714, read through `r` -- settled or not is the caller's.
    async fn account_of_write(
        &self,
        r: &wire::Reader<'_>,
        why: &str,
        owner: &str,
        view_id: &str,
        trace: &str,
    ) -> String {
        let oid = self.oid(owner, view_id);
        let dlq = r.take_dead_letters().await;
        let rows = r.trace(trace).await;
        let chain: Vec<String> = rows
            .iter()
            .map(|r| {
                let hop = hop_of(r);
                format!(
                    "{} -> {} route={} ttl={} error_code={}",
                    r.from_path, r.to_path, hop["route"], r.ttl, hop["error_code"]
                )
            })
            .collect();
        let last = rows.last().map_or_else(
            || format!("none (the trace {trace} has no row in the log)"),
            |r| r.to_path.clone(),
        );
        let writes = r.writes_of(owner, view_id).await;
        let in_tree = r.tree().await.get(&oid).is_some();
        format!(
            "{why}; last hop: {last}; store took {writes} write(s) of {oid}; \
             in the tree: {in_tree}; hops of trace {trace}: {chain:#?}; DLQ {dlq:?}"
        )
    }

    /// The receipts that left the hive, as `(error_code, view_id, detail)`, until `n`
    /// refusals are in hand.
    ///
    /// Re-check tv-bn M7 M1 (TV-S3, TV-Z3, TV-F5): gh958 and gh964 each kept a word-for-word
    /// copy of this wait, polling the egress every 25 ms under a std `Instant` plus the
    /// unscaled `MARKER` ("within 30s"). One copy now, on the egress itself (`recv`, an
    /// event), under the kit's hang mark; a hang names the refusals in hand.
    pub async fn refusals(&mut self, n: usize) -> Vec<(String, String, String)> {
        let (seen_tx, seen_rx) = tokio::sync::watch::channel(Vec::new());
        let w = &self.w;
        let got = async move {
            let mut out: Vec<(String, String, String)> = Vec::new();
            while out.len() < n {
                let Some(m) = w.next_out().await else {
                    panic!(
                        "the egress closed with {} of {n} refusals: {out:?}",
                        out.len()
                    );
                };
                let Some(r) = refusal_of(&m) else {
                    continue;
                };
                out.push(r);
                seen_tx.send_replace(out.clone());
            }
            out
        };
        let mut out = meclaw_testing::deadline::hang_with(
            &format!("{n} refusals leave the hive"),
            got,
            move || seen_rx.borrow().clone(),
        )
        .await;
        // Re-check tv-bn M7b4 A-M1: the n in hand are the event; what the lock judges is
        // every refusal once the colony settled, so one too many is seen, not cut off.
        for m in self.w.settled().await.drain_egress().await {
            if let Some(r) = refusal_of(&m) {
                out.push(r);
            }
        }
        out
    }

    /// Put one window up whose content is a whole component tree, and WAIT until the pass
    /// has drawn it. `put` writes a bare `display-pane` with props; a proof that needs
    /// children in the window (a list, a table, a line of text) names the tree itself.
    pub async fn put_content(&self, owner: &str, view_id: &str, content: Value) -> Value {
        let before = self.w.settled().await.writes_of(owner, view_id).await;
        let msg = to_screen(
            "in_view",
            owner,
            json!({"view_id": view_id, "region": "main", "kind": "component",
                   "content": content, "components": [], "ttl_ms": 0,
                   "messages": []}),
        );
        let trace = msg.trace_id.to_string();
        self.w.send(msg).await;
        self.drawn_after(owner, view_id, before, &trace).await
    }

    /// Put up the view an application's code cell emitted, as it emitted it (`view_id`,
    /// `content` AND `components`), and WAIT until the pass has drawn it. `put_content`
    /// defines no component; a view that brings its own -- a code view like `colony-view`,
    /// whose shell carries its client script -- needs them on the same write.
    pub async fn put_emission(&self, owner: &str, emission: &Value) -> Value {
        let view_id = emission["view_id"]
            .as_str()
            .expect("an emission names its view");
        let before = self.writes_of(owner, view_id).await;
        let msg = to_screen(
            "in_view",
            owner,
            json!({"view_id": view_id, "region": emission["region"], "kind": emission["kind"],
                   "content": emission["content"], "components": emission["components"],
                   "ttl_ms": 0, "messages": []}),
        );
        let trace = msg.trace_id.to_string();
        self.w.send(msg).await;
        self.drawn_after(owner, view_id, before, &trace).await
    }

    /// Take one window down again (§ 4.34).
    pub async fn withdraw(&self, owner: &str, view_id: &str) {
        self.w
            .send(to_screen(
                "in_withdraw",
                owner,
                json!({"view_id": view_id, "messages": []}),
            ))
            .await;
    }

    /// Tell the app stand-in to act. The command travels as an ordinary text turn, so the
    /// probe is a cell of the colony and not a back door of the test. The trace id of the
    /// command comes back: every hop the act causes carries it (GH #714).
    pub async fn app(&self, cmd: Value) -> String {
        let msg = MessageBuilder::new(Path::new(PROBE))
            .reply_to(Path::new(PROBE))
            .body(Body::Inline(json!({"messages": [
                {"origin": "user", "type": "text", "text": cmd.to_string()}
            ]})))
            .ttl(24)
            .build();
        let trace = msg.trace_id.to_string();
        self.w.send(msg).await;
        trace
    }

    /// One message into the colony, sender `/`: what a lock writes that no verb above
    /// names (a resident's answer, a forged lane, a refused block).
    pub async fn send(&self, msg: Message) {
        self.w.send(msg).await;
    }

    /// Fill the template table from `root` (`ColonyMsg::RescanTemplates`); the scan's
    /// outcome comes back.
    pub async fn rescan_templates(&self, root: &std::path::Path) -> Result<(), String> {
        self.w.rescan_templates(root.to_path_buf()).await
    }

    /// Apply one mutation of `diff` under `scope` (`ColonyMsg::Mutation`, a fresh trace);
    /// the outcome comes back.
    pub async fn apply_diff(&self, scope: &str, diff: Value) -> MutationOutcome {
        self.w.mutate(json!({"scope": scope, "diff": diff})).await
    }

    /// The window id of a view in the screen state (§ 2 Id).
    pub fn oid(&self, owner: &str, view_id: &str) -> String {
        format!("view.{}.{}", owner.replace('/', "~"), view_id)
    }

    pub async fn shutdown(self) {
        self.w.shutdown().await;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Reading the colony back
// ─────────────────────────────────────────────────────────────────────────────

impl Colony {
    /// The colony's `message_log`, oldest first -- the `GET /colony/messages` of a running
    /// meclaw. `to_prefix` narrows it to one receiver. Read after the colony settled (every
    /// reader of this fixture is, re-check tv-bn M7b4 A-M1).
    pub async fn log(&self, to_prefix: Option<&str>) -> Vec<MessageLogDto> {
        self.w.settled().await.log(to_prefix).await
    }

    /// Every row of one trace, oldest first (`MessageLogFilter.trace_id`, indexed): the hops
    /// one write caused, whoever handled them.
    pub async fn trace(&self, trace_id: &str) -> Vec<MessageLogDto> {
        self.w.settled().await.trace(trace_id).await
    }

    /// Mark the message log as it stands once the colony settled (`ColonyHandle::log_mark`):
    /// rows committed later are what [`Self::rows_since`] returns. Not `log_mark`: that is
    /// the handle's raw reader, a needle of the 809 pin on every receiver (OR-T11-25 G3).
    pub async fn mark_log(&self) -> meclaw_testing::LogMark {
        self.w.settled().await.mark_log().await
    }

    /// The rows matching `filter` committed after `mark`, newest first, read after the
    /// colony settled (`ColonyHandle::log_since`).
    pub async fn rows_since(
        &self,
        mark: &meclaw_testing::LogMark,
        filter: MessageLogFilter,
    ) -> Vec<MessageLogDto> {
        self.w.settled().await.rows_since(mark, filter).await
    }

    /// How many rows the whole log holds. What Q-07 counts.
    pub async fn log_len(&self) -> usize {
        self.w.settled().await.log(None).await.len()
    }

    /// Every message the screen's `<child>` received, oldest first.
    pub async fn to_child(&self, child: &str) -> Vec<MessageLogDto> {
        self.w.settled().await.to_child(child).await
    }

    /// The store bundles the compose cell sent to `views`, as the list of their calls.
    pub async fn store_bundles(&self) -> Vec<Vec<Value>> {
        self.w.settled().await.store_bundles().await
    }

    /// The tree the display holds, as the `patch` hops to `web` built it: every patch
    /// bundle in log order, folded (GH #809, OR-D11). `{id: {props, parent, ord, component}}`.
    ///
    /// The curator's state is memory since display 2.7.0 and no message carries it, so a
    /// lock reads the screen where it lands -- at `web` -- and never inside the cell.
    pub async fn tree(&self) -> Value {
        self.w.settled().await.tree().await
    }

    /// The tree as the pass that first drew the window `oid` left it: the patch bundles
    /// in log order, folded up to and including the first one that creates `<oid>/c.win`.
    ///
    /// GH #1184: what a window's FIRST pass said (`age` is `fresh` in the pass in which it
    /// appears, § 4.35) is a fact of that pass, not of the moment a lock reads the tree.
    /// `put` returns once the colony was quiet for 300 ms, and the stroke the first pass
    /// orders comes one second after it; under suite load the settle outlasted that second,
    /// the stroke's pass drew `settled`, and the lock read the second pass as the first.
    /// Bound to the bundle that drew the window, the read no longer races the clock.
    pub async fn first_drawn(&self, oid: &str) -> Value {
        let win = format!("{oid}/c.win");
        let mut held = meclaw_core::serde_json::Map::new();
        for calls in self.w.settled().await.patches().await {
            fold(&mut held, &calls);
            let draws = calls
                .iter()
                .any(|c| c["op"] == "object.create" && c["id"] == win.as_str());
            if draws {
                return Value::Object(held);
            }
        }
        panic!("no patch bundle drew the window {win}");
    }

    /// Watch the folded tree until `want` holds, or fail with `HANG:` -- a tight watch for an
    /// event that has to be acted on inside a window, before the colony settles (809: the
    /// kill has to land inside the fresh second; with the settle `wait_tree` ends on, the
    /// stroke came first in 4 of 5 runs at CPUQuota 80, tv-bn M7b6 F7). It hands back
    /// nothing: whatever the caller claims afterwards it reads through a settling reader.
    // no-settle: a tight watch for a positive event; returns nothing
    pub async fn watch_tree(&self, why: &str, want: impl Fn(&Value) -> bool) {
        let want = &want;
        meclaw_testing::wait::until_some_with(
            why,
            move || async move {
                let tree = self.w.unsettled().because(Why::Watch).tree().await;
                want(&tree).then_some(())
            },
            || async {
                let r = self.w.unsettled().because(Why::Report);
                let dlq = r.take_dead_letters().await;
                format!("DLQ {dlq:?}")
            },
        )
        .await
    }

    /// Wait until `want` holds of the folded tree, or fail with `HANG:`, the ids of the tree
    /// read after the mark and the dead letters. What comes back is the tree once the
    /// colony settled after that (re-check tv-bn M7b4 A-M1: a value read at the event is
    /// half a pass).
    // no-settle: polls for a positive event (Why::Watch), its hang text is Why::Report; hands back the tree read at rest
    pub async fn wait_tree(&self, why: &str, want: impl Fn(&Value) -> bool) -> Value {
        let want = &want;
        meclaw_testing::wait::until_some_with(
            why,
            move || async move {
                let tree = self.w.unsettled().because(Why::Watch).tree().await;
                want(&tree).then_some(())
            },
            || async {
                let r = self.w.unsettled().because(Why::Report);
                let tree = r.tree().await;
                let ids: Vec<&String> = tree
                    .as_object()
                    .map(|m| m.keys().collect())
                    .unwrap_or_default();
                let dlq = r.take_dead_letters().await;
                format!("the tree holds {ids:?}; DLQ {dlq:?}")
            },
        )
        .await;
        self.tree().await
    }

    /// Wait until `want` holds of whatever it reads off the colony, or fail with `HANG:`
    /// naming `why` and the dead letters.
    // no-settle: a wait for a positive event; it returns nothing, its closure's reads are the caller's
    pub async fn wait_until(&self, why: &str, want: impl AsyncFn() -> bool) {
        let want = &want;
        meclaw_testing::wait::until_some_with(
            why,
            move || async move { want().await.then_some(()) },
            || async {
                let r = self.w.unsettled().because(Why::Report);
                let dlq = r.take_dead_letters().await;
                format!("DLQ {dlq:?}")
            },
        )
        .await
    }

    /// How many times the compose cell asked `web` for its tree (`hop.route == "read"`):
    /// once per boot, and once after a refused patch (OR-D7).
    pub async fn reads(&self) -> usize {
        self.w
            .settled()
            .await
            .to_child("web")
            .await
            .iter()
            .filter(|row| hop_of(row)["route"] == "read")
            .count()
    }

    /// The boot behind the `n`-th `read` hop at `web` (1-based, as [`Self::reads`] counts),
    /// once it is in the log: what woke the cell -- the message the boot's select answers,
    /// as `(sender -> receiver, route)` -- and the tree `web`
    /// held before it, the patch bundles in log order folded up to that read.
    ///
    /// TV-F2 (tv-bn M7b10): a killed curator boots on whatever reaches it first, and under
    /// load that was a late reply of the pass before the kill (`views`/`web` -> compose), not
    /// the stroke; a restart test asks this which message it was, and a count of strokes
    /// cannot tell a stroke before the kill from one after it.
    pub async fn boot_read(&self, n: usize) -> Option<(String, String, Value)> {
        let settled = self.w.settled().await;
        let mut held = meclaw_core::serde_json::Map::new();
        let mut seen = 0;
        let mut parent = None;
        for row in settled.to_child("web").await {
            let route = hop_of(&row)["route"].clone();
            if route == "read" {
                seen += 1;
                if seen == n {
                    parent = Some(row.parent_message_id.clone().unwrap_or_default());
                    break;
                }
            } else if route == "patch"
                && let Some(calls) = calls_of(&row)
            {
                fold(&mut held, &calls);
            }
        }
        // The boot is one select, then one read (`docs/cell-types.md` § code, GH #809): the
        // read answers the store's reply to the boot's select, and the select answers the
        // message that woke the cell. Walk the parents back to that select.
        let rows: std::collections::HashMap<String, MessageLogDto> = settled
            .log(None)
            .await
            .into_iter()
            .map(|row| (row.id.clone(), row))
            .collect();
        let mut at = parent?;
        let mut woke = None;
        for _ in 0..rows.len() {
            let Some(row) = rows.get(&at) else { break };
            let selects = row.to_path.ends_with("/views")
                && calls_of(row)
                    .is_some_and(|calls| calls.iter().any(|c| c["operation"] == "select"));
            let Some(up) = row.parent_message_id.clone() else {
                break;
            };
            if selects {
                woke = rows.get(&up);
                break;
            }
            at = up;
        }
        let (from, route) = woke.map_or_else(
            || {
                (
                    format!("<no select of a boot above the read {at}>"),
                    String::new(),
                )
            },
            |row| {
                let route = hop_of(row)["route"].as_str().unwrap_or("").to_string();
                (format!("{} -> {}", row.from_path, row.to_path), route)
            },
        );
        Some((from, route, Value::Object(held)))
    }

    /// Every rest row the curator wrote (`owner "display"`, `view_id "screen-rest"`),
    /// oldest first, the content parsed: what outlives the cell's memory (OR-D3).
    pub async fn rest_rows(&self) -> Vec<Value> {
        let mut out = Vec::new();
        for bundle in self.w.settled().await.store_bundles().await {
            for call in bundle {
                if call["operation"] == "insert"
                    && call["row"]["owner"] == "display"
                    && call["row"]["view_id"] == "screen-rest"
                {
                    let text = call["row"]["content"].as_str().unwrap_or("{}");
                    out.push(
                        meclaw_core::serde_json::from_str(text)
                            .expect("the rest row's content is JSON"),
                    );
                }
            }
        }
        out
    }

    /// The last rest row the curator wrote, or `Null` while it wrote none.
    pub async fn rest(&self) -> Value {
        self.settle().await;
        self.rest_rows().await.pop().unwrap_or(Value::Null)
    }

    /// The `headers_json` of every logged message whose receiver starts with `to_prefix`.
    pub async fn headers(&self, to_prefix: &str) -> Vec<String> {
        self.w
            .settled()
            .await
            .log(Some(to_prefix))
            .await
            .into_iter()
            .map(|row| row.headers_json)
            .collect()
    }

    /// Kill the compose cell's child: SIGKILL to every child of THIS process whose command
    /// line is the resident harness (`crates/meclaw-cells/src/code/harness.py`, line 1).
    ///
    /// Found through `/proc/self/task/*/children`, never by name across the host: the
    /// probe is a `cold` cell and runs no harness, so the one match is the curator. The
    /// pool replaces the child with the next message (`tests/gh420_a_resident_child_may_
    /// be_killed_mid_stream.rs`). Returns how many it killed.
    pub async fn kill_compose(&self) -> usize {
        let mut pids = Vec::new();
        for task in std::fs::read_dir("/proc/self/task").expect("/proc/self/task") {
            let children = task.expect("a task").path().join("children");
            // a thread that ended between the listing and the read has no children
            let text = proc_read(
                &children.display().to_string(),
                std::fs::read_to_string(&children),
            )
            .unwrap_or_default();
            pids.extend(
                text.split_whitespace()
                    .filter_map(|p| p.parse::<i32>().ok()),
            );
        }
        pids.sort_unstable();
        pids.dedup();
        let mut killed = 0;
        let mut dead = Vec::new();
        for pid in pids {
            let path = format!("/proc/{pid}/cmdline");
            // a child that ended and was reaped since the listing is no harness
            let cmdline = proc_read(&path, std::fs::read(&path)).unwrap_or_default();
            if !String::from_utf8_lossy(&cmdline).contains("resident runner harness") {
                continue;
            }
            // SAFETY: a plain syscall on a pid that is a child of this very process.
            if unsafe { libc::kill(pid, libc::SIGKILL) } == 0 {
                killed += 1;
                dead.push(pid);
            }
        }
        // Until the kernel has made each a zombie (or it is reaped): a message that meets
        // a DYING child fails with it (`script_failed`, exit -1, measured here), and the
        // test is about the next message meeting a DEAD one, which the pool replaces
        // (`gh420_a_resident_child_may_be_killed_mid_stream.rs` waits for the same).
        // Re-check tv-bn M3 H4: the wait is the kit's (`wait::until_with`, the HANG mark),
        // not a hand-made poll with its own clock; re-check tv-bn M7b9 B-M2: a HANG names
        // the child's last `/proc/<pid>/stat` (its state letter: hung in the kernel, `D`,
        // or never reaped).
        for pid in dead {
            let path = format!("/proc/{pid}/stat");
            meclaw_testing::wait::until_with(
                &format!("the killed child {pid} died"),
                || {
                    let path = path.clone();
                    async move { proc_stat_dead(&path, std::fs::read_to_string(&path)) }
                },
                || format!("{path}: {:?}", std::fs::read_to_string(&path)),
            )
            .await;
        }
        eprintln!("kill_compose: {killed} resident child(ren) killed");
        killed
    }

    /// The patch bundles the compose cell sent to the `web` cell, as their calls.
    pub async fn patches(&self) -> Vec<Vec<Value>> {
        self.w.settled().await.patches().await
    }

    /// The orders that reached the clock, as `(op, schedule_id)`. `op`, `schedule_id` and
    /// `at` are body slots of the `due` emission, not hop keys.
    pub async fn orders(&self) -> Vec<(String, String)> {
        self.w
            .settled()
            .await
            .to_child("clock")
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
        self.w
            .settled()
            .await
            .to_child("compose")
            .await
            .iter()
            .filter(|row| hop_of(row)["route"] == "in_tick")
            .filter_map(|row| Some(hop_of(row)["schedule_id"].as_str()?.to_string()))
            .collect()
    }

    /// The trace of every strike. A timer emits its strike as a source message, so each
    /// strike opens a trace of its own, and whatever its pass sends carries that trace.
    pub async fn strike_traces(&self) -> Vec<String> {
        self.w
            .settled()
            .await
            .to_child("compose")
            .await
            .iter()
            .filter(|row| hop_of(row)["route"] == "in_tick")
            .map(|row| row.trace_id.clone())
            .collect()
    }

    /// The trace of every store bundle, in the order of `store_bundles`.
    pub async fn store_bundle_traces(&self) -> Vec<String> {
        self.w
            .settled()
            .await
            .to_child("views")
            .await
            .iter()
            .filter(|row| calls_of(row).is_some())
            .map(|row| row.trace_id.clone())
            .collect()
    }

    /// Every event that reached the pass, as `(name, value)`. This is the one place a
    /// gesture becomes a message: the `web` cell sends it, the hive's edge carries it to
    /// `compose`. A gesture that has no wire never shows up here.
    pub async fn events_into_pass(&self) -> Vec<(String, Value)> {
        self.w
            .settled()
            .await
            .to_child("compose")
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
        self.w
            .settled()
            .await
            .log(Some(SCREEN))
            .await
            .iter()
            .filter(|row| row.to_path == SCREEN)
            .filter_map(|row| Some(hop_of(row)["route"].as_str()?.to_string()))
            .filter(|route| route.starts_with("in_"))
            .collect()
    }

    /// The `views` table as the store holds it: every bundle the compose cell sent to
    /// `views`, folded in log order (delete by `where`, insert the row) over the empty
    /// table this colony boots with. Read where the store took the calls; since GH #809 no
    /// write asks the store back, so there is no answer to read the table out of.
    pub async fn store_rows(&self) -> Vec<Value> {
        let mut table: Vec<Value> = Vec::new();
        for bundle in self.w.settled().await.store_bundles().await {
            for call in bundle {
                match call["operation"].as_str().unwrap_or("") {
                    "delete" => {
                        let wanted = call["where"].as_object().cloned().unwrap_or_default();
                        table.retain(|row| !wanted.iter().all(|(k, v)| row[k] == *v));
                    }
                    "insert" => table.push(call["row"].clone()),
                    _ => {}
                }
            }
        }
        table
    }

    /// Every receipt the screen sent out of the hive, as `(error_code, view_id)`, drained
    /// once the colony settled.
    pub async fn receipts(&mut self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for m in self.w.settled().await.drain_egress().await {
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

    /// Every message that left the hive since the last drain, drained once the colony
    /// settled: what a lock counts at the member's door.
    pub async fn left_the_hive(&mut self) -> Vec<Message> {
        self.w.settled().await.drain_egress().await
    }

    /// How many questions reached the judge cell.
    pub async fn judge_questions(&self) -> usize {
        self.w.settled().await.to_child("judge").await.len()
    }

    /// Every question the judge cell put to its provider, in arrival order, once the
    /// colony settled.
    pub async fn judge_asked(&self) -> Vec<CapturedRequest> {
        self.w.settled().await.judge_asks().await
    }

    /// The event "the message log moved": a wait on it is a wait on an event, and its
    /// counter is a hang's last state ([`LogChanges::at`]).
    pub fn log_changes(&self) -> LogChanges {
        self.w.log_changes()
    }

    /// Wait until the colony is quiescent: no delivery, mailbox, output or log write of
    /// what was sent before is still in flight (`ColonyMsg::Settle`, GH #1208 TV-A2). A
    /// colony that does not settle within the hang mark fails with `HANG: the colony
    /// settles` and its last state -- the dead letters and the newest log rows (re-check
    /// tv-bn M7b5 B-M2).
    ///
    /// A pass is several hops, and a lock that reads after the first of them reads half a
    /// pass. Re-check tv-bn M7 M2 (TV-S2, TV-A5): this was a window -- "the newest log row
    /// stood for `quiet`" -- and a pause under load longer than the window read as rest.
    /// Quiescence covers what follows from delivered messages; where a timer stroke or an
    /// ingress drives the movement, the lock awaits that event first (`wait_until`).
    ///
    /// It hands back the proof that the colony settled; a test has no use for it (only the
    /// fixture's readers trade it for the address), a fixture reader does.
    pub async fn settle(&self) -> sealed::Settled {
        self.w.settle().await
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The page, and the socket a gesture rides
// ─────────────────────────────────────────────────────────────────────────────

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// One call site of a user that gets something the colony's address opens (OR-T11-25 G2,
/// re-check tv-bn M7b12 A-M2/A-M3, OR-T12-2): a variant per call site, bound to its file
/// AND its line at run time ([`Named::admits`] through `#[track_caller]`), so a second
/// place panics in every process. No site gets the address itself: a driver site gets a
/// driver the fixture starts ([`Colony::drive`]), the voice site the voice files the
/// fixture fetches ([`Colony::voice_files`]). A new place is a new variant here, with its
/// file, its line and its reason -- the `match`es below do not compile without them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Site {
    /// gh767: the tap driver on the monitor.
    Gh767Monitor,
    /// 710 (both engines): the layout driver on the line's exit, with the API.
    B710,
    /// gh961: the motion driver on the monitor, with the API.
    Gh961,
    /// gh1028: the keep driver on the monitor, with the API.
    Gh1028,
    /// gh867 (strict CSP): the CSP driver on the phone.
    Gh867CspPhone,
    /// gh867 (inline scripts): the voice test page and its files, fetched by the fixture.
    Gh867VoiceFiles,
}

impl Site {
    /// Every site, in the order of the enum (the 809 pin checks it against the source).
    pub const ALL: [Site; 6] = [
        Site::Gh767Monitor,
        Site::B710,
        Site::Gh961,
        Site::Gh1028,
        Site::Gh867CspPhone,
        Site::Gh867VoiceFiles,
    ];

    /// The driver a site starts, `None` for the voice files: how it launches, the name of
    /// its script (`workshop/tools/display-<name>-browser.mjs`), and whether it gets the API
    /// (`MECLAW_DISPLAY_API`). The script's path itself stands in the test unit, which names
    /// it in [`Driver::script`]: the gate finds a browser lock by the driver it names, and a
    /// shared module naming one would hide its callers (`browser-station-lock`,
    /// `scripts/gate_plan.py`). [`Colony::drive`] refuses any other script for the site.
    fn driver(self) -> Option<(Launch, &'static str, bool)> {
        match self {
            Site::Gh767Monitor => Some((Launch::Node, "page", false)),
            Site::B710 => Some((Launch::Lab, "layout", true)),
            Site::Gh961 => Some((Launch::Lab, "motion", true)),
            Site::Gh1028 => Some((Launch::Lab, "keep", true)),
            Site::Gh867CspPhone => Some((Launch::Node, "csp", false)),
            Site::Gh867VoiceFiles => None,
        }
    }
}

/// Handed to a browser driver: the page reads the colony, the lock reads the page (OR-M7b7-4).
const TO_THE_DRIVER: &str = "a browser driver the fixture starts: the page reads the colony through its own socket, \
     and the lock reads only what the page reports (OR-M7b7-4)";

impl sealed::NamedSeal for Site {}

impl Named for Site {
    fn file(self) -> &'static str {
        match self {
            Site::Gh767Monitor => "gh767_the_drivers_tap_reaches_the_curator_browser.rs",
            Site::B710 => "710_the_colony_holds_in_both_engines_browser.rs",
            Site::Gh961 => "gh961_motion_by_target_state_browser.rs",
            Site::Gh1028 => "gh1028_a_new_window_keeps_the_others_in_place_browser.rs",
            Site::Gh867CspPhone => "gh867_a_display_runs_under_a_strict_csp_browser.rs",
            Site::Gh867VoiceFiles => "gh867_no_surface_page_carries_unlisted_inline_script.rs",
        }
    }

    fn line(self) -> u32 {
        match self {
            Site::Gh767Monitor => 105,
            Site::B710 => 242,
            Site::Gh961 => 162,
            Site::Gh1028 => 105,
            Site::Gh867CspPhone => 175,
            Site::Gh867VoiceFiles => 373,
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Site::Gh767Monitor | Site::B710 | Site::Gh961 | Site::Gh1028 | Site::Gh867CspPhone => {
                TO_THE_DRIVER
            }
            Site::Gh867VoiceFiles => {
                "the voice test page and its three files as they ship (`/voice`), fetched by the \
                 fixture: static files of the voice cell, no state of the colony"
            }
        }
    }
}

/// How a site's driver starts.
#[derive(Debug, Clone, Copy)]
enum Launch {
    /// `node <script> <url> …`.
    Node,
    /// Through the WebKit laboratory: `sh -c` with `$1` the laboratory (sourced for every
    /// run except a Chromium one), the rest the driver and its arguments.
    Lab,
}

/// The laboratory WebKit runs in on this host (`Launch::Lab`).
const WKENV: &str = "workshop/tools/wkenv.sh";
/// `sh -c` with `$1` the laboratory, the rest the driver and its arguments. Sourced for
/// Chromium too, a host without the laboratory read its `SKIP` line as the Chromium run's
/// own (XB-M review I-1).
const WITH_LAB: &str =
    "case \" $* \" in *\" --engine chromium \"*) ;; *) . \"$1\" ;; esac; shift; exec node \"$@\"";

/// What a test gives the driver of its site: the arguments after the page's URL and the
/// environment. The colony's address is no part of it -- the fixture puts it in, and the
/// started command never comes back out (re-check tv-bn M7b12 A-M3, OR-T12-2).
#[derive(Debug, Default)]
pub struct Driver {
    script: Option<&'static str>,
    args: Vec<std::ffi::OsString>,
    envs: Vec<(std::ffi::OsString, std::ffi::OsString)>,
}

impl Driver {
    /// The site's driver script under the repository, as the test unit names it (the one
    /// [`Colony::drive`] accepts for the site, nothing else).
    pub fn script(mut self, path: &'static str) -> Self {
        self.script = Some(path);
        self
    }

    /// One argument after the URL.
    pub fn arg(mut self, a: impl AsRef<std::ffi::OsStr>) -> Self {
        self.args.push(a.as_ref().to_os_string());
        self
    }

    /// Arguments after the URL.
    pub fn args<I, A>(mut self, all: I) -> Self
    where
        I: IntoIterator<Item = A>,
        A: AsRef<std::ffi::OsStr>,
    {
        self.args
            .extend(all.into_iter().map(|a| a.as_ref().to_os_string()));
        self
    }

    /// One variable of the driver's environment.
    pub fn env(mut self, k: impl AsRef<std::ffi::OsStr>, v: impl AsRef<std::ffi::OsStr>) -> Self {
        self.envs
            .push((k.as_ref().to_os_string(), v.as_ref().to_os_string()));
        self
    }
}

/// A driver the fixture started the command of: it runs, it does not open (the command
/// is private; its arguments carry the address).
pub struct Driving(tokio::process::Command);

impl std::fmt::Debug for Driving {
    /// Not the command: its arguments carry the address.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Driving(..)")
    }
}

impl Driving {
    /// Run the driver to its end (the child is killed when the future is dropped).
    pub async fn output(mut self) -> std::io::Result<std::process::Output> {
        self.0.output().await
    }

    /// Run the driver under the Rust marker of [`browser_run::run_marked`].
    pub async fn run_marked(
        mut self,
        what: &str,
        run: Duration,
        outside: Duration,
    ) -> Result<std::io::Result<std::process::Output>, Box<browser_run::Hung>> {
        browser_run::run_marked(&mut self.0, what, run, outside).await
    }
}

/// The URL of one output under the mount (or of the switch when `screen` is empty, § 6.5).
fn screen_url(ports: sealed::Ports, screen: &str) -> String {
    format!("http://127.0.0.1:{}/{MOUNT}/{screen}", ports.port)
}

/// The voice test page (`""`, i.e. `/voice/`) and the three files beside it.
const VOICE_FILES: [&str; 4] = ["", "test.js", "test.css", "worklet.js"];

/// GET one shipped file until it answers past `503 starting`: (status, body).
async fn get_shipped(url: String) -> (u16, String) {
    let at = url.as_str();
    meclaw_testing::wait::until_some_with(
        &format!("{url} answers past `503 starting`"),
        move || async move {
            let r = reqwest::get(at).await.ok()?;
            if r.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE {
                return None;
            }
            let status = r.status().as_u16();
            Some((status, r.text().await.expect("a body")))
        },
        move || async move {
            match reqwest::get(at).await {
                Ok(r) => format!("it answers {}", r.status()),
                Err(e) => format!("it does not answer: {e}"),
            }
        },
    )
    .await
}

// The bridge from the compile probe to this fixture: the colony's address IS the sealed
// door, and the presenter's decider the sealed one. Change a type, and this does not compile.
const _: fn(&Colony) -> &sealed::Door = |c| &c.door;

impl Colony {
    /// Start the driver of a named call site of a user on one output (§ 6.5: an empty
    /// `screen` is the switch): its script, the page's URL, then `with`'s arguments and
    /// environment, and the API where the site gets it. The site is checked against the
    /// caller's file and line; the address stays in the fixture.
    #[track_caller]
    pub fn drive(&self, at: Site, screen: &str, with: Driver) -> Driving {
        let ports = self.door.hand(at);
        let Some((launch, name, api)) = at.driver() else {
            panic!("{at:?} starts no driver");
        };
        let script = format!("workshop/tools/display-{name}-browser.mjs");
        assert!(
            with.script == Some(script.as_str()),
            "{at:?} drives `{script}`, the caller names {:?}",
            with.script
        );
        let mut cmd = match launch {
            Launch::Node => {
                let mut c = tokio::process::Command::new("node");
                c.arg(repo(&script));
                c
            }
            Launch::Lab => {
                let mut c = tokio::process::Command::new("sh");
                c.arg("-c")
                    .arg(WITH_LAB)
                    .arg("sh")
                    .arg(repo(WKENV))
                    .arg(repo(&script));
                c
            }
        };
        cmd.arg(screen_url(ports, screen))
            .args(with.args)
            .envs(with.envs);
        if api {
            cmd.env(
                "MECLAW_DISPLAY_API",
                format!("http://127.0.0.1:{}", ports.api_port),
            );
        }
        // The node process holds a browser. A dropped future does not take a child with
        // it, so the browser would outlive the test that started it.
        cmd.kill_on_drop(true);
        Driving(cmd)
    }

    /// The voice test page and its three files as they ship (`/voice/`, `test.js`,
    /// `test.css`, `worklet.js`), for a named call site of a user: (file, status, body),
    /// the page first as `""`. The fixture GETs them; the address stays in the fixture.
    #[track_caller]
    pub fn voice_files(
        &self,
        at: Site,
    ) -> impl std::future::Future<Output = Vec<(&'static str, u16, String)>> + use<> {
        let ports = self.door.hand(at);
        assert!(
            at == Site::Gh867VoiceFiles,
            "{at:?} is no site of the voice files"
        );
        async move {
            let mut out = Vec::new();
            for file in VOICE_FILES {
                let (status, body) =
                    get_shipped(format!("http://127.0.0.1:{}/voice/{file}", ports.port)).await;
                out.push((file, status, body));
            }
            out
        }
    }

    /// GET one output until it answers, and hand the markup back.
    pub async fn page(&self, screen: &str) -> String {
        let at_rest = self.settle().await;
        let url = screen_url(self.door.settled(at_rest), screen);
        let at = url.as_str();
        meclaw_testing::wait::until_some_with(
            &format!("{url} serves a page"),
            move || async move {
                match reqwest::get(at).await {
                    Ok(r) if r.status().is_success() => Some(r.text().await.expect("text")),
                    _ => None,
                }
            },
            move || async move {
                match reqwest::get(at).await {
                    Ok(r) => format!("it answers {}", r.status()),
                    Err(e) => format!("it does not answer: {e}"),
                }
            },
        )
        .await
    }

    /// The HTTP status one path under the mount answers with.
    pub async fn status(&self, screen: &str) -> u16 {
        let at_rest = self.settle().await;
        reqwest::get(screen_url(self.door.settled(at_rest), screen))
            .await
            .expect("the listener answers")
            .status()
            .as_u16()
    }

    /// Join the LiveView socket of one output -- the page's OWN socket, the one a gesture
    /// travels on (§ 5.6). What comes back is the socket, ready for `push`.
    pub async fn socket(&self, screen: &str) -> Ws {
        let at_rest = self.settle().await;
        let ports = self.door.settled(at_rest);
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
            ports.port
        ))
        .await
        .expect("the display accepts a websocket");
        let join = json!(["1", "1", topic, "phx_join",
                          {"session": token, "url": screen_url(ports, screen)}]);
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
///
/// Re-check tv-bn M7 H6 (TV-Z3): a hang names how many other frames came meanwhile.
pub async fn next_text(ws: &mut Ws) -> Value {
    let (seen_tx, seen_rx) = tokio::sync::watch::channel(0_usize);
    let text = async move {
        loop {
            let msg = ws
                .next()
                .await
                .expect("the stream stays open")
                .expect("a frame");
            if let tokio_tungstenite::tungstenite::Message::Text(t) = msg {
                return meclaw_core::serde_json::from_str(&t).expect("the frame is JSON");
            }
            seen_tx.send_modify(|n| *n += 1);
        }
    };
    meclaw_testing::deadline::hang_with("the socket sends a text frame", text, move || {
        format!("{} non-text frame(s) came", *seen_rx.borrow())
    })
    .await
}

/// A refusal receipt that left the hive, as `(error_code, view_id, detail)`; `None` for
/// any other message and for a receipt without an error.
fn refusal_of(m: &Message) -> Option<(String, String, String)> {
    if m.headers.hop.get("route").and_then(Value::as_str) != Some("receipt") {
        return None;
    }
    let Body::Inline(body) = &m.body else {
        return None;
    };
    let r = &body["receipt"];
    let code = r["error_code"].as_str().unwrap_or("");
    if code.is_empty() {
        return None;
    }
    Some((
        code.to_string(),
        r["view_id"].as_str().unwrap_or("").to_string(),
        r["detail"].as_str().unwrap_or("").to_string(),
    ))
}

/// The hop header of one logged message.
pub fn hop_of(row: &MessageLogDto) -> Value {
    let headers: Value =
        meclaw_core::serde_json::from_str(&row.headers_json).unwrap_or_else(|_| json!({}));
    headers["hop"].clone()
}

/// Fail closed on a body the log does not hold inline (re-check tv-bn M7b12 C-M1, class
/// M9): a body over `blob_inline_max_bytes` is logged as `body_kind = "blob"` with the
/// blob's id as its payload, and every reader of a body (`body_of`, `calls_of`, a raw
/// `body_payload`) would read nothing there -- an absence claim green on a row it never
/// read. No reader of the fixture resolves a blob, so such a row is red, with its name.
pub fn inline_bodies(rows: &[MessageLogDto]) {
    let blobs: Vec<String> = rows
        .iter()
        .filter(|r| r.body_kind != "inline")
        .map(|r| {
            format!(
                "{} {} -> {} ({})",
                r.id, r.from_path, r.to_path, r.body_kind
            )
        })
        .collect();
    assert!(
        blobs.is_empty(),
        "fail closed: the log holds these bodies as blobs, which no reader of the fixture \
         resolves: {blobs:#?}"
    );
}

/// The patch bundles among `rows` (the rows to `web`), as their calls: a patch row
/// without calls is no patch the tree folds, a patch row held as a blob is red.
pub fn patches_of(rows: &[MessageLogDto]) -> Vec<Vec<Value>> {
    inline_bodies(rows);
    rows.iter()
        .filter(|row| hop_of(row)["route"] == "patch")
        .filter_map(calls_of)
        .collect()
}

/// The inline body of one logged message; a body held as a blob is red
/// ([`inline_bodies`]).
pub fn body_of(row: &MessageLogDto) -> Value {
    inline_bodies(std::slice::from_ref(row));
    row.body_payload
        .as_deref()
        .and_then(|raw| meclaw_core::serde_json::from_str(raw).ok())
        .unwrap_or_else(|| json!({}))
}

/// The `tool_call` turns of one logged message, parsed back into calls; a body held as a
/// blob is red ([`inline_bodies`]).
pub fn calls_of(row: &MessageLogDto) -> Option<Vec<Value>> {
    inline_bodies(std::slice::from_ref(row));
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

/// The `display_request` mark of one logged message, parsed (`Null` without one).
pub fn request_of(row: &MessageLogDto) -> Value {
    let headers: Value =
        meclaw_core::serde_json::from_str(&row.headers_json).unwrap_or_else(|_| json!({}));
    headers["context"]["display_request"]
        .as_str()
        .and_then(|t| meclaw_core::serde_json::from_str(t).ok())
        .unwrap_or(Value::Null)
}

/// One patch bundle folded onto a held tree, the way `web` applies it (`support::apply`
/// over a map): create, update (props merged), move, delete.
pub fn fold(held: &mut meclaw_core::serde_json::Map<String, Value>, calls: &[Value]) {
    for c in calls {
        let Some(id) = c["id"].as_str() else { continue };
        match c["op"].as_str().unwrap_or("") {
            "object.create" => {
                held.insert(
                    id.to_string(),
                    json!({"props": c["props"], "parent": c["parent"], "ord": c["ord"],
                           "component": c["component"]}),
                );
            }
            "object.update" => {
                if let Some(obj) = held.get_mut(id) {
                    if let Some(props) = c["props"].as_object() {
                        for (k, v) in props {
                            obj["props"][k] = v.clone();
                        }
                    }
                    if !c["parent"].is_null() {
                        obj["parent"] = c["parent"].clone();
                    }
                }
            }
            "object.move" => {
                if let Some(obj) = held.get_mut(id) {
                    obj["parent"] = c["parent"].clone();
                    obj["ord"] = c["ord"].clone();
                }
            }
            "object.delete" => {
                held.remove(id);
            }
            _ => {}
        }
    }
}

/// The props one window wears on the screen (`<oid>/c.win`, the window node every app of
/// these locks writes): the app's hints plus the curator's values (`rung`, `age`, `since`,
/// `score`, `acted`, … -- `WINDOW_ATTRS` in compose.py).
pub fn attr(tree: &Value, oid: &str, key: &str) -> Value {
    tree[format!("{oid}/c.win")]["props"][key].clone()
}

/// The props of one window's tile in the dock of the default output.
pub fn tile(tree: &Value, oid: &str) -> Value {
    tree[format!("display.dock/tile.{oid}")]["props"].clone()
}

/// Whether the window stands on the screen: its wrapper is in the tree (§ 4.11).
pub fn present(tree: &Value, oid: &str) -> bool {
    tree.get(oid).is_some()
}

/// One read of a `/proc` file (re-check tv-bn M3 M2): `None` when the task or process
/// is gone (NotFound, or ESRCH from a process reaped between open and read), the
/// content otherwise. Any other error is a red assertion that names the path and the
/// error: an empty read counted as "ended", and the next message then met a dying
/// child (`script_failed`) -- a red that looked like a flake.
pub fn proc_read<T>(path: &str, read: std::io::Result<T>) -> Option<T> {
    match read {
        Ok(content) => Some(content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) if e.raw_os_error() == Some(libc::ESRCH) => None,
        Err(e) => panic!("{path} could not be read ({e}): a /proc read error is no ended process"),
    }
}

/// Whether the process of the `/proc/<pid>/stat` read `read` has died: gone, a zombie
/// (`Z`) or dead (`X`). A read error other than "gone" and a stat without a state
/// field are red assertions (re-check tv-bn M3 M2), never "died".
pub fn proc_stat_dead(path: &str, read: std::io::Result<String>) -> bool {
    let Some(stat) = proc_read(path, read) else {
        return true;
    };
    match stat
        .rsplit(')')
        .next()
        .and_then(|r| r.split_whitespace().next())
    {
        Some("Z") | Some("X") => true,
        Some(_) => false,
        None => panic!("{path} carries no state field: {stat:?}"),
    }
}
