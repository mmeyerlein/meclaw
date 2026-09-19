//! The `turn_id` a channel stamps reaches the answer.
//!
//! `display-hive.md` § 2: "a turn is an utterance of the member in a channel, with a
//! `turn_id`; the answer … carries the same `turn_id`, as does every window that arises
//! from the answer". § 8.2 says who mints it: "assigned by the channel on acceptance;
//! the answer and every window from it carry it on". § 4.13 (`step5_chat_closes`) is
//! what SPENDS it: the chat closes when a canvas window carries the `turn_id` of the
//! last turn. None of it can fire while the answer carries a different id than the turn
//! did, and that is what was measured.
//!
//! # The measurement, before a line was edited
//!
//! Two live colonies, both shipped channels — one spoken, one typed — and the same road
//! with the same three losses on each:
//!
//! | hop | what the id was | what happens |
//! |---|---|---|
//! | `turn` channel → `./channels` | the channel's stamp | minted here (`voice/turns.rs:537`, `chat-channel/channel.py:77`) |
//! | `in_turn` → `./firewall/screen` | unchanged | the hive edges carry the hop header |
//! | `pass` `./screen` → `./firewall` | **gone** | `firewall/screen` `verdict()` REBUILDS the header from a fixed key set that has no `turn_id` in it |
//! | `turn` `…/stamp` → `session-keeper` | **gone** | `session-keeper/stamp` builds a fresh `turn` header and copies only the BODY into it |
//! | `cstore` `…/assemble` → `window` | a fresh `uuid4` | `collector/assemble` MINTS one per round |
//! | `answer` → the member's applications | that `uuid4` | the applications read `hop.turn_id or ctx.turn_id` and get the collector's id |
//!
//! The consequence on the screen: the chat window carried no `turn_id` at all while the
//! windows an application had opened carried the collector's. § 4.13 can never fire, and
//! the chat never closes.
//!
//! A fourth loss was found in review and is locked here too: `firewall/warden`, the
//! second producer of the hive's `pass` and `reject` exits. A turn a person RELEASES
//! from a hold used to re-enter the agent with no id at all — so the one turn somebody
//! deliberately let through was the one the collector renamed.
//!
//! # What this file locks, and what it does not
//!
//! The four losses, each against the SHIPPED `script_inline` of the real template, plus
//! the three hive edges that have to carry the id across a store round-trip (the screen,
//! the stamp and the warden all hand their turn to a store and get a reply whose hop is
//! the STORE's, so the id rides the context the way `fw_body`, `keeper_body` and
//! `wd_meta` already do).
//!
//! It does not boot a colony, and it does not reach the window. Every hop between these
//! cells is an edge that copies the hop header through — `templates/talky/config.json`
//! `./session-keeper -> ./collector` uses a PARTIAL `set_hop` (`route` only), and
//! `709_the_typed_turn_takes_the_road_of_every_turn` already locks the member's two
//! `chat` edges — so a colony test here would re-measure wiring that is locked elsewhere
//! and pay a full assistant for it. The window half lives in another repository
//! (`apps/chat/compose`) and is settled by measuring a running colony, not here. What
//! was NOT locked anywhere is the four cells that drop the id, and that is what is here.
//!
//! Guarded like every other template-reading test (GH #49).

use std::io::Write;
use std::process::{Command, Stdio};

const SCREEN: &str = "../../templates/firewall/screen/config.json";
const FIREWALL: &str = "../../templates/firewall/config.json";
const WARDEN: &str = "../../templates/firewall/warden/config.json";
const KEEPER_STAMP: &str = "../../templates/session-keeper/stamp/config.json";
const KEEPER: &str = "../../templates/session-keeper/config.json";
const ASSEMBLE: &str = "../../templates/collector/assemble/config.json";

/// The id a channel stamps. `#` on purpose: BOTH shipped channels put one in
/// (`<session>#<seq>` for voice, `chat#<hex>` for the typed one), and `#` is the one
/// separator the collector's composite ids do NOT use — `|` is (`turn|call|kind|ref`),
/// which is why the adoption below has to refuse it.
const T: &str = "chat#0f1e2d3c4b5a6978";

// ───────────────────────────────────────────────────────────── running a shipped script

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty string — the
/// substitution the colony performs at instantiation.
fn resolve_vars(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail
            .find('}')
            .expect("unterminated ${...} in script_inline");
        if let Some((_, default)) = tail[..end].split_once(":-") {
            out.push_str(default);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn config_of(path: &str) -> serde_json::Value {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{path} does not parse: {e}"))
}

fn script_of(config: &str) -> String {
    resolve_vars(
        config_of(config)["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
    )
}

/// Hand the script to python3 **on stdin** rather than in argv (GH #279: a single argv
/// string is capped at 128 KiB, and `collector/assemble` is well past it).
fn run_script_on_stdin(script: &str, stdin_doc: &str) -> std::process::Output {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        serde_json::to_string(script).unwrap(),
        serde_json::to_string(stdin_doc).unwrap(),
    );
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    child.wait_with_output().expect("wait")
}

fn emit(config: &str, doc: serde_json::Value) -> Vec<serde_json::Value> {
    let out = run_script_on_stdin(
        &script_of(config),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    assert!(
        out.status.success(),
        "{config}: the script exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{config}: output is not JSON ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    match v {
        serde_json::Value::Array(a) => a,
        one => vec![one],
    }
}

/// The one emission on `route`, or a failure that prints everything the cell said —
/// "the id is on the pass lane" is not a claim about whichever message came first.
fn on_route<'a>(msgs: &'a [serde_json::Value], route: &str, lane: &str) -> &'a serde_json::Value {
    msgs.iter()
        .find(|m| m["header"]["route"].as_str() == Some(route))
        .unwrap_or_else(|| {
            panic!(
                "{lane}: nothing left on route `{route}`: {}",
                serde_json::to_string(msgs).expect("serialise")
            )
        })
}

fn turn_id_of(m: &serde_json::Value, lane: &str) -> String {
    m["header"]["turn_id"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "{lane}: the header carries no `turn_id` key at all: {}",
                serde_json::to_string(m).expect("serialise")
            )
        })
        .to_string()
}

/// The `set_context` of the one edge `from -> to` inside a hive's graph.
fn edge_promotions(config: &str, from: &str, to: &str) -> serde_json::Value {
    let cfg = config_of(config);
    let edges = cfg["params"]["graph"]["edges"]
        .as_array()
        .unwrap_or_else(|| panic!("{config}: no graph edges"));
    edges
        .iter()
        .find(|e| e["from"].as_str() == Some(from) && e["to"].as_str() == Some(to))
        .map(|e| e["modifier"]["set_context"].clone())
        .unwrap_or_else(|| panic!("{config}: no edge {from} -> {to}"))
}

// ───────────────────────────────────────────────────────── firewall/screen: the verdict

/// The turn as the member's ingress edge delivers it: the channel's stamp on the hop.
fn screening_doc(text: &str) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"channel": "chat", "user_id": "300850023", "turn_id": T},
            "hop": {"route": "in_turn", "channel": "chat", "user_id": "300850023",
                    "turn_id": T, "recorded_at": "2026-01-02T03:04:05.000000Z"}
        },
        "messages": [{"origin": "user", "type": "text", "text": text}]
    })
}

/// The rules store's answer to the `rate` count, as the return edge delivers it: the hop
/// is the STORE's, so everything the screen still knows about the turn is in the context.
/// `fw_hold` set is the same reply for a turn a hold row matched.
fn rate_reply(hold: &str) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"fw_phase": "rate", "channel": "chat", "user_id": "300850023",
                        "turn_id": T, "fw_hold": hold,
                        "fw_now": "2026-01-02T03:04:05.000000Z",
                        "fw_body": "{\"messages\":[{\"origin\":\"user\",\"type\":\"text\",\"text\":\"Are you there?\"}]}"},
            "hop": {"operation": "select", "rows_affected": 0}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "", "text": "[]"}]
    })
}

#[test]
fn the_screen_passes_the_turn_id_on() {
    let out = emit(SCREEN, rate_reply(""));
    assert_eq!(
        turn_id_of(
            on_route(&out, "pass", "firewall/screen"),
            "firewall/screen pass"
        ),
        T,
        "the pass lane drops the id the channel stamped — § 8.2"
    );
}

#[test]
fn the_screen_refuses_with_the_turn_id_on_it() {
    // The size cap fires AHEAD of the first store hop, so this exit reads the id off the
    // inbound hop and not off a promoted context — both halves of the fallback matter.
    let out = emit(SCREEN, screening_doc(&"x".repeat(17_000)));
    let m = on_route(&out, "reject", "firewall/screen");
    assert_eq!(
        m["header"]["reject_reason"].as_str(),
        Some("oversize"),
        "the fixture no longer trips the size cap: {}",
        serde_json::to_string(&out).expect("serialise")
    );
    assert_eq!(
        turn_id_of(m, "firewall/screen reject"),
        T,
        "a refused turn is still a turn and still has its id"
    );
}

#[test]
fn the_screen_holds_with_the_turn_id_on_it() {
    let out = emit(SCREEN, rate_reply("hold:row-1"));
    assert_eq!(
        turn_id_of(
            on_route(&out, "hold", "firewall/screen"),
            "firewall/screen hold"
        ),
        T,
        "a held turn keeps its id — the release later answers THAT turn"
    );
}

#[test]
fn the_screen_asks_its_store_under_the_turn_id() {
    // The store round-trip is where the id would otherwise die: the reply's hop belongs
    // to the store. `fw_body` and `fw_now` already ride the context across it; the id
    // takes the same road, and the edge below is the half that carries it.
    let out = emit(SCREEN, screening_doc("Are you there?"));
    assert_eq!(
        turn_id_of(
            on_route(&out, "fwstore", "firewall/screen"),
            "firewall/screen fwstore"
        ),
        T,
        "the store op does not name the turn it is about"
    );
}

#[test]
fn a_receipt_about_a_rule_row_still_names_the_turn_it_arrived_on() {
    // `notice()` is the one thing on the `reject` lane that is NOT a verdict: a bodiless
    // receipt about an unreadable rule ROW (GH #506). It carries the same key set for the
    // same reason — a parent edge promoting `hop.turn_id` must not meet a lane that left
    // the key out and skip itself.
    let mut doc = screening_doc("Are you there?");
    doc["header"]["context"]["fw_phase"] = serde_json::json!("rules");
    doc["header"]["hop"] = serde_json::json!({"operation": "select", "rows_affected": 1});
    doc["messages"] = serde_json::json!([{
        "origin": "tool", "type": "tool_result", "id": "",
        // One row with a `kind` outside the closed vocabulary: skipped, and named.
        "text": "[{\"rule_id\": \"r-1\", \"kind\": \"nonsense\", \"field\": \"channel\", \"value\": \"chat\", \"action\": \"deny\"}]"
    }]);
    let out = emit(SCREEN, doc);
    let m = out
        .iter()
        .find(|m| m["header"]["reject_reason"].as_str() == Some("rule_unreadable"))
        .unwrap_or_else(|| {
            panic!(
                "the fixture no longer produces a rule receipt: {}",
                serde_json::to_string(&out).expect("serialise")
            )
        });
    assert_eq!(
        turn_id_of(m, "firewall/screen notice"),
        T,
        "the receipt left the key out — a parent edge promoting it skips the whole edge"
    );
}

#[test]
fn the_firewall_edge_carries_the_turn_id_across_the_store() {
    let set = edge_promotions(FIREWALL, "./screen", "./rules");
    assert_eq!(
        set["turn_id"].as_str(),
        Some("hop.turn_id"),
        "./screen -> ./rules has to promote the id, or the reply comes back without it: {set}"
    );
}

// ────────────────────────────────────────────────────── session-keeper/stamp: the turn

#[test]
fn the_stamp_asks_its_store_under_the_turn_id() {
    let out = emit(
        KEEPER_STAMP,
        serde_json::json!({
            "header": {
                "context": {"channel": "chat", "turn_id": T},
                "hop": {"route": "in_turn", "channel": "chat", "turn_id": T}
            },
            "messages": [{"origin": "user", "type": "text", "text": "Are you there?"}]
        }),
    );
    assert_eq!(
        turn_id_of(
            on_route(&out, "kstore", "session-keeper/stamp"),
            "stamp kstore"
        ),
        T,
        "the session lookup does not name the turn it is about"
    );
}

#[test]
fn the_stamp_hands_the_turn_on_with_its_turn_id() {
    // The lookup came back empty: this turn opens the next generation. The `turn` header
    // is built fresh here — that is exactly where the id was dropped.
    let out = emit(
        KEEPER_STAMP,
        serde_json::json!({
            "header": {
                "context": {"ses_phase": "look", "channel": "chat", "turn_id": T,
                            "keeper_body": "{\"messages\":[{\"origin\":\"user\",\"type\":\"text\",\"text\":\"Are you there?\"}]}"},
                "hop": {"operation": "select", "rows_affected": 0}
            },
            "messages": [{"origin": "tool", "type": "tool_result", "id": "", "text": "[]"}]
        }),
    );
    let m = on_route(&out, "turn", "session-keeper/stamp");
    assert!(
        m["header"]["session_id"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "the fixture no longer opens a generation: {}",
        serde_json::to_string(&out).expect("serialise")
    );
    assert_eq!(
        turn_id_of(m, "session-keeper/stamp turn"),
        T,
        "the stamped turn lost the id the channel gave it — § 8.2"
    );
}

#[test]
fn the_keeper_edge_carries_the_turn_id_across_the_store() {
    let set = edge_promotions(KEEPER, "./stamp", "./sessions");
    assert_eq!(
        set["turn_id"].as_str(),
        Some("hop.turn_id"),
        "./stamp -> ./sessions has to promote the id, or the reply comes back without it: {set}"
    );
}

// ───────────────────────────────────────── firewall/warden: custody keeps the id

/// The hold the screen hands over, as the hive's `./screen -> ./warden` edge delivers it.
fn held_turn() -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"channel": "chat", "user_id": "4711", "turn_id": T},
            "hop": {"route": "hold", "rule_id": "hold:row-1", "reject_reason": "held",
                    "channel": "chat", "user_id": "4711", "turn_id": T}
        },
        "messages": [{"origin": "user", "type": "text", "text": "Are you there?"}]
    })
}

/// The `wd_meta` the warden minted for a parked turn, as its own emission carries it.
fn parked_meta(out: &[serde_json::Value]) -> serde_json::Value {
    let m = on_route(out, "wdstore", "firewall/warden");
    let raw = m["header"]["wd_meta"].as_str().unwrap_or_else(|| {
        panic!(
            "firewall/warden: the store op carries no wd_meta: {}",
            serde_json::to_string(m).expect("serialise")
        )
    });
    serde_json::from_str(raw).expect("wd_meta is json")
}

#[test]
fn the_warden_parks_the_turn_under_its_own_turn_id() {
    let meta = parked_meta(&emit(WARDEN, held_turn()));
    assert_eq!(
        meta["turn_id"].as_str(),
        Some(T),
        "the custody meta forgot the id — and it is the only place the id survives the \
         time a person takes to answer: {meta}"
    );
}

#[test]
fn the_notice_that_a_turn_was_parked_names_the_turn() {
    // The store wrote the row; only now does anybody hear that the turn was parked.
    let meta = parked_meta(&emit(WARDEN, held_turn()));
    let out = emit(
        WARDEN,
        serde_json::json!({
            "header": {
                "context": {"wd_phase": "park", "wd_meta": meta.to_string(),
                            "wd_body": "{\"messages\":[]}"},
                "hop": {"operation": "insert", "rows_affected": 1}
            },
            "messages": []
        }),
    );
    assert_eq!(
        turn_id_of(
            on_route(&out, "hold", "firewall/warden"),
            "warden hold notice"
        ),
        T,
        "the notice names a hold but not the turn it holds"
    );
}

#[test]
fn a_released_turn_re_enters_under_the_id_it_was_spoken_under() {
    // THE case this section exists for. The release lane carries a hold id and a decision
    // and nothing else, so the id has to come back off the stored row — and then out on
    // `pass`, or the collector mints a fresh round id for exactly the turns a person
    // deliberately let through.
    let meta = serde_json::json!({
        "hold_id": "h-1", "state": "released", "turn_id": T,
        "rule_id": "hold:row-1", "reason": "held", "channel": "chat", "user_id": "4711",
        "expires_at": "2126-01-02T03:04:05.000000Z",
        "context": {"channel": "chat", "user_id": "4711", "turn_id": T}
    });
    let out = emit(
        WARDEN,
        serde_json::json!({
            "header": {
                "context": {"wd_phase": "decide", "wd_meta": meta.to_string(),
                            "wd_body": "{\"messages\":[{\"origin\":\"user\",\"type\":\"text\",\"text\":\"Are you there?\"}]}"},
                "hop": {"operation": "update", "rows_affected": 1}
            },
            "messages": []
        }),
    );
    assert_eq!(
        turn_id_of(on_route(&out, "pass", "firewall/warden"), "warden release"),
        T,
        "the released turn left custody without the id it was parked with"
    );
}

#[test]
fn the_warden_edge_hands_the_released_turns_id_back_to_the_context() {
    let cfg = config_of(FIREWALL);
    let edges = cfg["params"]["graph"]["edges"]
        .as_array()
        .expect("graph edges");
    let e = edges
        .iter()
        .find(|e| {
            e["from"].as_str() == Some("./warden")
                && e["to"].as_str() == Some(".")
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains("'pass'"))
        })
        .expect("the warden's pass exit");
    assert_eq!(
        e["modifier"]["set_context"]["turn_id"].as_str(),
        Some("hop.turn_id"),
        "the released turn re-enters with no turn_id in its context: {e}"
    );
}

// ────────────────────────────────────────────────── collector/assemble: the round's id

fn collector_turn(lane: &str, turn_id: Option<&str>) -> serde_json::Value {
    let mut hop = serde_json::json!({"route": lane});
    if let Some(t) = turn_id {
        hop["turn_id"] = serde_json::json!(t);
    }
    serde_json::json!({
        "header": {"context": {"session_id": "chat-2026-01-02T03:04:05.000000Z"}, "hop": hop},
        "messages": [{"origin": "user", "type": "text", "text": "Are you there?"}]
    })
}

#[test]
fn the_collector_opens_the_round_under_the_channels_turn_id() {
    let out = emit(ASSEMBLE, collector_turn("in_turn", Some(T)));
    assert_eq!(
        turn_id_of(
            on_route(&out, "cstore", "collector/assemble"),
            "collector turn-open"
        ),
        T,
        "the collector minted an id of its own over the one the channel stamped — \
         everything downstream, the answer and every window with it, then carries THAT"
    );
}

#[test]
fn the_collector_still_mints_one_for_a_channel_that_stamps_none() {
    // Not every ingress has a channel in front of it. A turn that arrives without an id
    // still needs one, and uuid4 stays the fallback — what changed is only that a
    // stamped id now wins over it.
    let out = emit(ASSEMBLE, collector_turn("in_turn", None));
    let got = turn_id_of(
        on_route(&out, "cstore", "collector/assemble"),
        "collector turn-open",
    );
    assert!(
        !got.is_empty() && got != T,
        "an unstamped turn has to get a fresh id: {got:?}"
    );
}

#[test]
fn an_advisors_return_never_adopts_the_id_on_its_hop() {
    // `in_advice` opens a round too, but its hop belongs to the ADVISOR's answer, not to
    // a channel. Adopting that id would key this round's `turns` and `round` rows on an
    // id another session already owns.
    let out = emit(ASSEMBLE, collector_turn("in_advice", Some(T)));
    let got = turn_id_of(
        on_route(&out, "cstore", "collector/assemble"),
        "collector advice-open",
    );
    assert_ne!(got, T, "the advisor's round took the id off a foreign hop");
}

#[test]
fn a_stamped_id_can_never_collide_with_a_composite_one() {
    // `tr-sel` and `prune-cut` read a COMPOSITE id back apart on `|`
    // (`turn|call|kind|ref`, `prune|<boundary>`). A channel is free to stamp whatever it
    // likes, so an id carrying that separator has to be disarmed on adoption rather than
    // trusted — otherwise one channel's choice of id silently reroutes a tool round.
    let out = emit(
        ASSEMBLE,
        collector_turn("in_turn", Some("weird|id|here|now")),
    );
    let got = turn_id_of(
        on_route(&out, "cstore", "collector/assemble"),
        "collector turn-open",
    );
    assert!(
        !got.contains('|'),
        "an adopted id still carries the composite separator: {got:?}"
    );
    assert!(
        got.starts_with("weird"),
        "the id was replaced rather than disarmed: {got:?}"
    );
}

#[test]
fn a_stamped_id_is_capped_in_length() {
    // The adopted id becomes a store key and a hop header. uuid4 was 36 characters and
    // the question never came up; a foreign string has no bound of its own.
    let long = "x".repeat(4000);
    let out = emit(ASSEMBLE, collector_turn("in_turn", Some(&long)));
    let got = turn_id_of(
        on_route(&out, "cstore", "collector/assemble"),
        "collector turn-open",
    );
    assert!(
        got.len() <= 128,
        "an adopted id went into the store at {} characters",
        got.len()
    );
    assert!(
        got.starts_with('x'),
        "the id was replaced rather than capped: {got:?}"
    );
}

#[test]
fn a_stamped_id_may_not_wear_the_reserved_close_shape() {
    // `CLOSE_ID` is `"close-" + session`: the bookkeeping row of a session's end. An id
    // in that shape would write into it, so it is refused outright rather than disarmed —
    // there is no character to replace, the whole shape is the collision.
    let out = emit(
        ASSEMBLE,
        collector_turn("in_turn", Some("close-chat-2026-01-02T03:04:05.000000Z")),
    );
    let got = turn_id_of(
        on_route(&out, "cstore", "collector/assemble"),
        "collector turn-open",
    );
    assert!(
        !got.starts_with("close-"),
        "a turn opened a round on the session-close row's own key: {got:?}"
    );
}
