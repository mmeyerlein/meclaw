//! GH #925 -- a model's history serves a round only what that round was
//! present for.
//!
//! Since GH #893 a model reads its own wall back (`history_search`,
//! `history_read`, `history_outline`, served inside its curator). The wall of
//! a surface holds every conversation of every channel it ever served, and
//! before GH #925 a call saw all of it: what a person said with two others in
//! the room could be read back to a round with a third person present. Now
//! every row of `wall`, `calls`, `marks`, `summaries` and `pins` carries the
//! audience of the round that caused it (`audience_set`, canonical: a sorted
//! JSON array without duplicates or whitespace), and a reader hands a row to a
//! round iff the row has one and it names `*` or holds the round as a subset
//! -- the affinity rule. A row from before the rule has no audience and
//! reaches no round (OR-BD-5); a round that declares none is refused with
//! `missing_audience`. A config pin cannot see any of this: it holds only when
//! the round's `context.audience_set` survives the whole road -- session
//! keeper, collector, curator, brain, splitter, dispatcher and every ledger
//! round trip inside the curator (OR-BD.A.1) -- so this file boots the SHIPPED
//! `talky` and `cogny`, drives real turns through them (nothing of the new
//! wall is sown) and measures at the receivers:
//!
//! 1. **A narrower round reads a wider one's words.** Session one runs in the
//!    round {e,a,b}; session two in {e,a} searches the wall and finds session
//!    one's words -- in the tool result the brain's next request carries.
//! 2. **A wider round reads nothing it was absent for.** Session three in
//!    {e,a,b,c} finds nothing of sessions one and two, the count says zero,
//!    and its outline names neither session. No request of session three
//!    carries a word of them, the window included.
//! 3. **A disjoint round reads nothing.** Session four in {e,b} reads session
//!    one ({e,a,b} holds it) and nothing session two ({e,a}) wrote -- not by a
//!    search and not by the id of its block, which reads `not_found`.
//! 4. **A round without an audience is refused.** Session five declares none:
//!    the call's `tool_result` reaches the collector with `hop.error_code`
//!    `missing_audience` (the message log, at the receiver) and the model
//!    reads the refusal and nothing of the wall.
//! 5. **The core's round leaves its marks under that round.** At a `cogny`
//!    curator: a history call that fails (`not_found`), a final answer whose
//!    sidecar block carries `correction`, and a question back
//!    (`ask_requester`) leave exactly one `tool_error`, one `correction` and
//!    one `ask` mark, each with the round's canonical audience, the consult's
//!    `session_id`, the `turn_id` of the round it happened in (the failed read
//!    and the correction share consult one's, the question back has consult
//!    two's) and an `at` inside this run -- read out of the ledger file of
//!    this test's own colony.
//! 6. **A pin needs a round.** `in_pin` without one writes no `pins` row; on
//!    its trace the curator shows exactly the one delivery into `./intake`,
//!    nothing leaves a cell of it and nothing is dead-lettered -- the refusal
//!    happened at `./intake`. The same pin with a round writes exactly one
//!    row, carrying that round.
//! 7. **A pin reaches only the rounds its audience holds.** A text pinned in
//!    {e,b} stands in the window of a new {e,b} session -- the brain's request
//!    carries it -- and no request of a following {e,a} session in another
//!    channel carries a word of it or its id.
//! 8. **A closed session hands the memory only what its round was present
//!    for.** One channel, one generation: turn one in {e,a} opens it (the
//!    session keeper records {e,a} as the generation's round), turn two in
//!    {e,b} and turn three without a round join it -- read out of the ledger,
//!    the session holds rows of {e,a}, of {e,b} and without an audience. A
//!    forced sweep closes it, and the `write` batch that leaves the talky
//!    carries in `messages[]` exactly turn one's question and answer, in
//!    `rounds` exactly turn one's tool round and not turn two's beside it
//!    (review R2-I-5), no word of turns two and three anywhere, and the
//!    counts of what it carries (`./writer`, review I-3).
//!
//! And across 1-4: a row from before the rule -- sown into this colony's own
//! ledger without an audience (OR-BD-19 precedent), and found on the wall as
//! sown before any probe looks for it -- is no hit, no block and no word in
//! any request, whatever the round.
//!
//! Every row this file writes names the four test participants `member:e`,
//! `member:a`, `member:b`, `member:c`; the rounds are stamped unsorted and with
//! spaces, the way a channel may stamp them, so the canonical form in the
//! ledger is the curator's own work.
//!
//! Free of a real provider by construction: each brain talks to a local stub,
//! every other `llm` cell (the curators' summarizers) to a closed local port;
//! every timer is moved out of the run's way. Guarded like every
//! template-reading test (GH #49).

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, NEVER_CRON, override_params_on_disk};
use mock_openai::{MockOpenAI, OpenAiRequestSnapshot, canned_chat_completion, canned_tool_calls};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// The failure-marker convention of this repo, not a timing discriminator.
const DEADLINE: Duration = Duration::from_secs(30);
/// How long a negative probe waits for what must NOT happen, once the positive
/// signal it rides on arrived (the `gh893` probe length).
const SETTLE: Duration = Duration::from_secs(2);
/// An endpoint nothing listens on: a call there fails at once and costs nothing.
const CLOSED_PORT: &str = "http://127.0.0.1:9/v1";
const BRAIN_MODEL: &str = "brain-stub";
const HISTORY_TOOLS: [&str; 3] = ["history_search", "history_read", "history_outline"];

// ─────────────────────────────────────────────────────────────── the rounds

/// The rounds as a channel stamps them: unsorted, with spaces.
const ROUND_EAB: &str = r#"["member:e", "member:a", "member:b"]"#;
const ROUND_EA: &str = r#"["member:e", "member:a"]"#;
const ROUND_EABC: &str = r#"["member:e", "member:a", "member:b", "member:c"]"#;
const ROUND_EB: &str = r#"["member:e", "member:b"]"#;

/// What the rows say. Every sentence a round may or may not see carries the
/// one token the search asks for, so a search that crossed a gate would show
/// it; the other turns and every reply carry none.
const OLD_SAID: &str = "cw925 zero: the lantern hangs under the stairs.";
const ONE_SAID: &str = "cw925 one: the orchard gate opens with the word quince.";
const TWO_SAID: &str = "cw925 two: the boat shed key sits behind the kettle.";
const THREE_ASKS: &str = "Session three asks what was noted before.";
const FOUR_ASKS: &str = "Session four asks what was noted before.";
const FIVE_ASKS: &str = "Session five asks what was noted before.";
const SEARCH: &str = r#"{"query": "cw925", "mode": "exact"}"#;

/// The row from before the rule: its own session, no audience.
const OLD_SESSION: &str = "s-925-old";
const OLD_TURN: &str = "t-925-old";

const CALL_TWO: &str = "call-925-two-search";
const CALL_THREE_SEARCH: &str = "call-925-three-search";
const CALL_THREE_OUTLINE: &str = "call-925-three-outline";
const CALL_FOUR_SEARCH: &str = "call-925-four-search";
const CALL_FOUR_READ: &str = "call-925-four-read";
const CALL_FOUR_READ_TWO: &str = "call-925-four-read-two";
const CALL_FIVE: &str = "call-925-five-search";

/// One talky session: its channel (the session id is `<channel>-<time>`,
/// `session-keeper/stamp`), its round, what the person says, what the model
/// answers last and how many provider calls it takes.
struct Session {
    channel: &'static str,
    round: Option<&'static str>,
    says: &'static str,
    reply: &'static str,
    calls: usize,
}

const SESSIONS: [Session; 5] = [
    Session {
        channel: "chat:925-one",
        round: Some(ROUND_EAB),
        says: ONE_SAID,
        reply: "Noted for the three of you.",
        calls: 1,
    },
    Session {
        channel: "chat:925-two",
        round: Some(ROUND_EA),
        says: TWO_SAID,
        reply: "Noted for the two of you.",
        calls: 2,
    },
    Session {
        channel: "chat:925-three",
        round: Some(ROUND_EABC),
        says: THREE_ASKS,
        reply: "Nothing is noted for this room.",
        calls: 3,
    },
    Session {
        channel: "chat:925-four",
        round: Some(ROUND_EB),
        says: FOUR_ASKS,
        reply: "One thing is noted for this room.",
        calls: 4,
    },
    Session {
        channel: "chat:925-none",
        round: None,
        says: FIVE_ASKS,
        reply: "I cannot look that up here.",
        calls: 2,
    },
];

// ────────────────────────────────────────────────────────────── the core's round

/// The core's round and conversation (case 5).
const CORE_ROUND: &str = r#"["member:e", "member:a"]"#;
const CORE_SESSION: &str = "s-925-core";
const CONSULT_ONE: &str = "k-925-one";
const CONSULT_TWO: &str = "k-925-two";
const QUESTION_ONE: &str = r#"{"question":"what was the plan again?"}"#;
const QUESTION_TWO: &str = r#"{"question":"and what should it cost?"}"#;
const CORE_READ: &str = "call-925-core-read";
/// A block id no wall of this run holds: the read fails `not_found`.
const UNKNOWN_ID: &str = r##"{"id": "#fedcba987654"}"##;
const CORE_ASK: &str = "call-925-core-ask";
const ASK_ARGS: &str = r#"{"question": "Which city is meant?"}"#;
const CORE_SAYS: &str = "The plan stands as ordered.";
/// The final answer with a sidecar block that carries `correction` (the
/// talky splitter's grammar: one JSON object between the fences).
const CORE_CORRECTED: &str = "The plan stands as ordered.\n\n```sidecar\n\
     {\"correction\": \"Name the city before the dates.\"}\n```";

// ──────────────────────────────────────────────────────────────── the pin

const PIN_ROUND: &str = r#"["member:e", "member:a"]"#;
const PIN_SOURCE: &str = "probe";
const PIN_TEXT: &str = "cw925 pin: the door code changes on Monday.";

/// Case 7: a text pinned in {e,b}. Every word of five letters or more is one
/// no shipped template says, so a request that carries one carries the pin.
const PIN_EB_TEXT: &str = "cw925 pin: the violet kayak lies on gravel by the heron.";
const SEVEN_EB: &str = "chat:925-seven-eb";
const SEVEN_EB_ASKS: &str = "Session seven asks what stands pinned here.";
const SEVEN_EB_REPLY: &str = "One pin stands for this room.";
const SEVEN_EA: &str = "chat:925-seven-ea";
const SEVEN_EA_ASKS: &str = "Session eight asks what stands pinned here.";
const SEVEN_EA_REPLY: &str = "No pin stands for this room.";

// ─────────────────────────────────────────────────────────────── the close

/// Case 8: one channel, one generation, three rounds. Every sentence carries
/// the turn it belongs to, so a batch that carried any part of turn two or
/// three would show its marker.
const CLOSE_CHANNEL: &str = "chat:925-close";
const CLOSE_ONE_SAYS: &str = "cw925 close one: the copper kettle sings at dawn.";
const CLOSE_ONE_REPLY: &str = "cw925 close one reply: the kettle is noted for the two of you.";
const CLOSE_TWO_SAYS: &str = "cw925 close two: the green ladder leans on the barn.";
const CLOSE_TWO_REPLY: &str = "cw925 close two reply: the ladder is noted for this room.";
const CLOSE_THREE_SAYS: &str = "cw925 close three: the rusty anchor rests in the yard.";
const CLOSE_THREE_REPLY: &str = "cw925 close three reply: the anchor is noted without a room.";
/// What only turns two and three said.
const CLOSE_LATER: [&str; 2] = ["cw925 close two", "cw925 close three"];
/// The tool rounds of turns one ({e,a}) and two ({e,b}) of case 8: the raw
/// rows the batch's `rounds` are gated over like its said turns (review
/// R2-I-5).
const CLOSE_ONE_CALL: &str = "call-925-close-one-outline";
const CLOSE_TWO_CALL: &str = "call-925-close-two-outline";

// ════════════════════════════════════════════════════════════ the shipped tree

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// Every template this file boots. One missing = skipped (GH #49).
fn shipped() -> bool {
    [
        "talky",
        "cogny",
        "collector",
        "curator",
        "dispatcher",
        "session-keeper",
    ]
    .iter()
    .all(|t| repo(&format!("templates/{t}/config.json")).is_file())
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} does not parse: {e}", p.display()))
}

fn write_json(p: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
    std::fs::write(
        p,
        meclaw_core::serde_json::to_string_pretty(v).expect("serialise"),
    )
    .expect("write");
}

fn map(v: Value) -> Map<String, Value> {
    v.as_object().cloned().expect("an object")
}

/// The shipped template, copied the way instantiation lays it out (the
/// `gh893` copy): a `cell.type: "ref"` directory is replaced by the referenced
/// template's tree and the marker's `override_params` are applied to the cells
/// they name (GH #140, GH #277).
fn copy_resolved(src: &std::path::Path, dst: &std::path::Path, depth: usize) {
    assert!(
        depth < 8,
        "template ref chain does not terminate at {}",
        src.display()
    );
    let marker = src.join("config.json");
    if marker.is_file() {
        let cfg = read_json(&marker);
        if cfg["cell"]["type"] == "ref" {
            let reference = cfg["cell"]["template"]
                .as_str()
                .unwrap_or_else(|| panic!("{}: a ref names a template", marker.display()));
            let name = reference.split('@').next().unwrap_or_default();
            let target = repo("templates").join(name);
            assert!(
                target.join("config.json").is_file(),
                "{}: `{reference}` resolves to no template in this tree",
                marker.display()
            );
            copy_resolved(&target, dst, depth + 1);
            if let Some(over) = cfg["override_params"].as_object() {
                for (cell, params) in over {
                    override_params_on_disk(&dst.join(cell), params);
                }
            }
            return;
        }
    }
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("readable") {
        let entry = entry.expect("entry");
        let from = entry.path();
        let name = entry.file_name();
        if from.is_dir() {
            copy_resolved(&from, &dst.join(name), depth);
        } else if name == "config.json"
            || (src.file_name().is_some_and(|d| d == "seed")
                && std::path::Path::new(&name)
                    .extension()
                    .is_some_and(|e| e == "jsonl"))
        {
            std::fs::copy(&from, dst.join(name)).expect("copy");
        }
    }
}

/// Every `config.json` below `dir`, seeds excluded.
fn configs_under(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("readable") {
        let p = entry.expect("entry").path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n != "seed") {
                configs_under(&p, out);
            }
        } else if p.file_name().is_some_and(|n| n == "config.json") {
            out.push(p);
        }
    }
}

/// Every timer of the tree, out of the run's way (the sweep of `gh893`): a
/// cron moves to a date no run reaches, a `${uuid7:*}` schedule id becomes a
/// fixed one.
fn quiet_timers(main: &std::path::Path) {
    let mut files = Vec::new();
    configs_under(main, &mut files);
    let mut n: u64 = 0;
    for f in files {
        let mut cfg = read_json(&f);
        if cfg["cell"]["type"] != "timer" {
            continue;
        }
        let Some(schedules) = cfg["params"]["schedules"].as_array_mut() else {
            continue;
        };
        for s in schedules.iter_mut() {
            n += 1;
            if s["schedule_id"]
                .as_str()
                .is_some_and(|id| id.contains("${"))
            {
                s["schedule_id"] =
                    json!(format!("0190a3f2-0000-7000-8000-{:012x}", 0x0925_0000 + n));
            }
            if s.get("cron").is_some() {
                s["cron"] = json!(NEVER_CRON);
            }
        }
        write_json(&f, &cfg);
    }
}

/// Every `llm` cell of the tree at a local endpoint: the brains `brains`
/// names at their stubs, everything else at the closed port.
fn point_llms(main: &std::path::Path, brains: &BTreeMap<&str, String>) {
    let mut files = Vec::new();
    configs_under(main, &mut files);
    for f in files {
        let mut cfg = read_json(&f);
        if cfg["cell"]["type"] != "llm" {
            continue;
        }
        let rel = f
            .parent()
            .expect("a cell directory")
            .strip_prefix(main)
            .expect("under main")
            .to_string_lossy()
            .replace('\\', "/");
        let url = brains
            .get(rel.as_str())
            .cloned()
            .unwrap_or_else(|| CLOSED_PORT.to_string());
        cfg["params"]["base_url"] = json!(url);
        cfg["params"]["model"] = json!(BRAIN_MODEL);
        cfg["params"]["api_key"] = json!("sk-test");
        write_json(&f, &cfg);
    }
}

/// The run's own environment file: every `${VAR}` the tree names bound to a
/// dummy, so nothing reads the host's.
fn write_env(root: &std::path::Path, main: &std::path::Path) {
    let mut files = Vec::new();
    configs_under(main, &mut files);
    let mut vars: BTreeMap<String, String> = BTreeMap::new();
    for f in files {
        let raw = std::fs::read_to_string(&f).unwrap_or_default();
        let mut rest = raw.as_str();
        while let Some(start) = rest.find("${") {
            rest = &rest[start + 2..];
            let Some(end) = rest.find('}') else { break };
            let name = &rest[..end];
            if !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            {
                vars.insert(name.to_string(), format!("dummy-{name}"));
            }
            rest = &rest[end + 1..];
        }
    }
    let body: String = vars.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    std::fs::write(root.join(".env"), body).expect("write the env file");
}

/// The lanes a template declares at its own path (an `at` lane docks
/// elsewhere, ADR-0020).
fn rim_emits(template: &str) -> Vec<String> {
    read_json(&repo(&format!("templates/{template}/config.json")))["params"]["contract"]["emits"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|l| match &l["at"] {
            Value::Null => true,
            Value::String(s) => s.is_empty(),
            Value::Array(a) => a.is_empty(),
            _ => false,
        })
        .filter_map(|l| l["route"].as_str().map(str::to_string))
        .collect()
}

/// One drain per rim lane of `composite`: `answer` to its sink, `ask` (the
/// core's question back) to its own port, `tool` to the probe, everything
/// else to the park.
fn rim(composite: &str) -> Vec<Value> {
    let path = format!("./{composite}");
    let mut edges = vec![json!({"from": ".", "to": path.clone(),
                                "condition": "has(hop.route) && hop.route == 'mutation_committed'"})];
    for lane in rim_emits(composite) {
        let to = match lane.as_str() {
            "answer" => format!("/sink_{composite}"),
            "ask" => "/ask_port".to_string(),
            "tool" => "/tool_port".to_string(),
            _ => "/park".to_string(),
        };
        edges.push(json!({"from": path.clone(), "to": to,
                          "condition": format!("has(hop.route) && hop.route == '{lane}'")}));
    }
    edges
}

/// The shipped `composites` under one parent, their brains at `brains`.
fn build(td: &tempfile::TempDir, composites: &[&str], brains: &BTreeMap<&str, String>) {
    let root = td.path();
    let main = root.join("main");
    let mut edges = Vec::new();
    for c in composites {
        copy_resolved(&repo(&format!("templates/{c}")), &main.join(c), 0);
        edges.extend(rim(c));
    }
    // GH #553: the menu is asked for on the mutation receipt, and the boot is
    // the first receipt (ruling O-0904-2).
    std::fs::write(
        root.join("colony.json"),
        r#"{"schema_version": 1, "mutation_receipts": {"to": "/"}}"#,
    )
    .expect("colony.json");
    write_json(
        &main.join("config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );
    quiet_timers(&main);
    point_llms(&main, brains);
    write_env(root, &main);
}

fn sha256_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The canonical JSON of the block of a person's words: the element as
/// `./intake` stores them.
fn user_body(text: &str) -> String {
    let el = json!({"origin": "user", "type": "text", "text": text});
    meclaw_core::serde_json::to_string(&el).expect("canonical: sorted keys, no whitespace")
}

/// The old row's block.
fn old_body() -> String {
    user_body(OLD_SAID)
}

/// The row from before the rule, sown as seed files of the copied ledger (the
/// `gh893` sowing): the block under the sha256 of its body and one wall row
/// WITHOUT `audience_set` -- the column stays NULL, as it is for every row
/// written before GH #925. The header of each file is the shipped schema of
/// its table.
fn sow_old(ledger: &std::path::Path) {
    let schema =
        read_json(&repo("templates/curator/ledger/config.json"))["params"]["schema"].clone();
    let body = old_body();
    let hash = sha256_hex(&body);
    let when = "2026-09-20T10:00:00.000000Z";
    let block = json!({"hash": hash, "kind": "user", "chars": OLD_SAID.chars().count(),
                       "body": body, "first_seen": when});
    let row = json!({"seq": 1_000_i64, "session_id": OLD_SESSION, "turn_id": OLD_TURN,
                     "iter": 0, "kind": "user", "hash": hash, "nth": 0, "final": 1,
                     "episode_idx": 0, "at": when});
    for (table, line) in [("blocks", block), ("wall", row)] {
        let header = json!({"schema": schema[table].clone()});
        std::fs::write(
            ledger.join("seed").join(format!("{table}.jsonl")),
            format!("{header}\n{line}\n"),
        )
        .expect("write a seed file");
    }
}

// ═══════════════════════════════════════════════════════════════ the run

struct Ports {
    talky: mpsc::Receiver<Message>,
    cogny: mpsc::Receiver<Message>,
    ask: mpsc::Receiver<Message>,
    /// Held, not dropped: a capture whose receiver is gone turns every
    /// delivery into a send error.
    _tool: mpsc::Receiver<Message>,
    /// Every other rim lane, `write` among them (case 8).
    park: mpsc::Receiver<Message>,
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, Ports) {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("timer".to_string(), Arc::new(TimerCellFactory)),
            ("llm".to_string(), Arc::new(LlmCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let mut rx = BTreeMap::new();
    for (path, room) in [
        ("/sink_talky", 64),
        ("/sink_cogny", 64),
        ("/ask_port", 64),
        ("/tool_port", 64),
        ("/park", 1024),
    ] {
        let (tx, r) = mpsc::channel::<Message>(room);
        h.spawn(Path::new(path), move || CaptureCell::new(tx.clone()))
            .await;
        rx.insert(path, r);
    }
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the shipped composites with their curators must boot");
    let mut take = |p: &str| rx.remove(p).expect("a capture port");
    let ports = Ports {
        talky: take("/sink_talky"),
        cogny: take("/sink_cogny"),
        ask: take("/ask_port"),
        _tool: take("/tool_port"),
        park: take("/park"),
    };
    (h, ports)
}

/// The ledger file of a composite's curator in THIS test's colony.
fn ledger_db(root: &std::path::Path, composite: &str) -> std::path::PathBuf {
    root.join(format!("main/{composite}/curator/ledger/cell.db"))
}

/// The rows of `sql` over a cell's own `cell.db`, every column as text (NULL
/// is `None`). Read-only and never created here: the store seeds its tables
/// only on a FRESH birth, and a poll that created the file before the ledger
/// woke turned a sown wall into a resumed, empty one (the `gh893` lesson).
fn query(db: &std::path::Path, sql: &str) -> Vec<Vec<Option<String>>> {
    use rusqlite::types::Value as SqlValue;
    if !db.is_file() {
        return Vec::new();
    }
    let Ok(conn) =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return Vec::new();
    };
    let Ok(mut st) = conn.prepare(sql) else {
        return Vec::new();
    };
    let n = st.column_count();
    let out: Vec<Vec<Option<String>>> = st
        .query_map([], |r| {
            Ok((0..n)
                .map(|i| match r.get::<_, SqlValue>(i) {
                    Ok(SqlValue::Text(s)) => Some(s),
                    Ok(SqlValue::Integer(v)) => Some(v.to_string()),
                    Ok(SqlValue::Real(v)) => Some(v.to_string()),
                    _ => None,
                })
                .collect::<Vec<Option<String>>>())
        })
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default();
    out
}

/// The tool names the curator's ledger holds as the collector's menu -- one
/// slot per declaration or the whole family in one block (the `gh893` reader).
fn ledger_tools(root: &std::path::Path, composite: &str) -> Vec<String> {
    let mut names = Vec::new();
    for r in query(
        &ledger_db(root, composite),
        "SELECT s.path, b.body FROM slots s LEFT JOIN blocks b ON b.hash = s.hash \
         WHERE s.path = 'tools' OR s.path LIKE 'tools.%'",
    ) {
        let path = r[0].clone().unwrap_or_default();
        if let Some(rest) = path.strip_prefix("tools.") {
            names.push(rest.split('.').next().unwrap_or(rest).to_string());
        } else if let Some(Value::Object(tree)) = r[1]
            .as_deref()
            .and_then(|b| meclaw_core::serde_json::from_str::<Value>(b).ok())
        {
            names.extend(tree.keys().filter(|k| !k.starts_with('$')).cloned());
        }
    }
    names.sort();
    names.dedup();
    names
}

/// Poll until the history tools stand in the curator's menu -- the boot
/// receipt asks, the curator answers (OR-KY-G1).
async fn await_menu(root: &std::path::Path, composite: &str) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let names = ledger_tools(root, composite);
        if HISTORY_TOOLS.iter().all(|t| names.iter().any(|n| n == t)) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{composite}: the curator's menu never offered the history tools. It holds \
             {names:?}. Dead letters: {:#?}",
            dead_letters(root)
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// One delivery as the colony logged it.
#[derive(Debug)]
struct Logged {
    from: String,
    to: String,
    trace: String,
    hop: Value,
}

fn message_log(root: &std::path::Path) -> Vec<Logged> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare("SELECT from_path, to_path, trace_id, headers FROM message_log ORDER BY rowid")
        .expect("message_log");
    let out: Vec<Logged> = st
        .query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                r.get::<_, Option<String>>(3)?.unwrap_or_default(),
            ))
        })
        .expect("query")
        .filter_map(Result::ok)
        .map(|(from, to, trace, headers)| {
            let h: Value = meclaw_core::serde_json::from_str(&headers).unwrap_or(Value::Null);
            Logged {
                from,
                to,
                trace,
                hop: h["hop"].clone(),
            }
        })
        .collect();
    out
}

/// `(error_code, sender, target, hop.route)` of one dead letter.
type DeadLetter = (String, String, String, String);

/// Every dead letter of the run -- said in every failure message, so a lost
/// message names itself.
fn dead_letters(root: &std::path::Path) -> Vec<DeadLetter> {
    dead_letter_rows(root)
        .into_iter()
        .map(|(_, dl)| dl)
        .collect()
}

/// The dead letters of one trace (review m-1: a refusal is no dead letter).
fn dead_letters_on(root: &std::path::Path, trace: &str) -> Vec<DeadLetter> {
    dead_letter_rows(root)
        .into_iter()
        .filter(|(t, _)| t == trace)
        .map(|(_, dl)| dl)
        .collect()
}

/// `(trace_id, dead letter)` of every dead letter of the run.
fn dead_letter_rows(root: &std::path::Path) -> Vec<(String, DeadLetter)> {
    let Ok(conn) = rusqlite::Connection::open(root.join("colony.db")) else {
        return Vec::new();
    };
    let Ok(mut st) = conn.prepare(
        "SELECT trace_id, error_code, sender_path, resolved_target, message_json \
         FROM dead_letters ORDER BY id",
    ) else {
        return Vec::new();
    };
    let out: Vec<(String, DeadLetter)> = st
        .query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                r.get::<_, Option<String>>(4)?.unwrap_or_default(),
            ))
        })
        .map(|rows| {
            rows.filter_map(Result::ok)
                .map(|(trace, code, sender, target, msg)| {
                    let m: Value = meclaw_core::serde_json::from_str(&msg).unwrap_or(Value::Null);
                    let route = m["headers"]["hop"]["route"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    (trace, (code, sender, target, route))
                })
                .collect()
        })
        .unwrap_or_default();
    out
}

fn said(m: &Message) -> String {
    let Body::Inline(v) = &m.body else {
        return String::new();
    };
    v["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|t| t["text"].as_str().map(str::to_string))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The next message on `rx` that says `needle`; everything before it is kept
/// for the failure message.
async fn answer_saying(
    rx: &mut mpsc::Receiver<Message>,
    root: &std::path::Path,
    needle: &str,
    what: &str,
) -> Message {
    let deadline = Instant::now() + DEADLINE;
    let mut seen = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some(m)) => {
                if said(&m).contains(needle) {
                    return m;
                }
                seen.push(said(&m));
            }
            _ => panic!(
                "{what}: no answer saying {needle:?} within {DEADLINE:?}. Answers so far: \
                 {seen:#?}. Dead letters: {:#?}",
                dead_letters(root)
            ),
        }
    }
}

/// A round's audience in the ledger's canonical form: a sorted JSON array of
/// distinct strings, no whitespace.
fn canonical(round: &str) -> String {
    let set: BTreeSet<String> = meclaw_core::serde_json::from_str::<Vec<String>>(round)
        .expect("a round is a JSON array of strings")
        .into_iter()
        .collect();
    meclaw_core::serde_json::to_string(&set.into_iter().collect::<Vec<_>>()).expect("serialise")
}

/// The whole wire of one provider request -- the window, the system, every
/// tool result -- as one string to look for words in.
fn wire(req: &OpenAiRequestSnapshot) -> String {
    meclaw_core::serde_json::to_string(req.messages().expect("wire messages")).unwrap_or_default()
}

/// The wire of one request without the model's own call under `call_id` and
/// the result answering it. A read by id names the id there because the model
/// asked for it -- in its arguments, and in a refusal that says which id it
/// did not find; anywhere else the id would have come through a gate.
fn wire_without_call(req: &OpenAiRequestSnapshot, call_id: &str) -> String {
    let kept: Vec<Value> = req
        .messages()
        .expect("wire messages")
        .iter()
        .filter(|m| m["tool_call_id"] != call_id)
        .map(|m| {
            let mut m = m.clone();
            if let Some(calls) = m.get_mut("tool_calls").and_then(Value::as_array_mut) {
                calls.retain(|c| c["id"] != call_id);
            }
            m
        })
        .collect();
    meclaw_core::serde_json::to_string(&kept).unwrap_or_default()
}

/// The content of the `tool` message answering `call_id`, in the first of
/// `reqs` that carries one.
fn tool_text(reqs: &[OpenAiRequestSnapshot], call_id: &str) -> String {
    reqs.iter()
        .filter_map(|r| r.messages())
        .flat_map(|w| w.iter())
        .find(|m| m["role"] == "tool" && m["tool_call_id"] == call_id)
        .map(|m| m["content"].as_str().unwrap_or_default().to_string())
        .unwrap_or_else(|| {
            panic!(
                "no tool result under `{call_id}` reached the brain; the requests: {:#?}",
                reqs.iter().map(wire).collect::<Vec<_>>()
            )
        })
}

/// The curator's answer inside a tool result: a talky window shows it under
/// its short id (`[#<12 hex>] `, GH #892, OR-KY.T.2), another role bare.
fn answered(text: &str) -> Value {
    let bare = match text.strip_prefix("[#") {
        Some(rest) => rest.find("] ").map_or(text, |i| &rest[i + 2..]),
        None => text,
    };
    meclaw_core::serde_json::from_str(bare)
        .unwrap_or_else(|e| panic!("the tool result is the curator's JSON ({e}): {text}"))
}

fn hits(result: &Value) -> Vec<Value> {
    result["hits"].as_array().cloned().unwrap_or_default()
}

/// Whether a search result holds a hit whose excerpt carries `words`.
fn has_hit(result: &Value, words: &str) -> bool {
    hits(result)
        .iter()
        .any(|h| h["excerpt"].as_str().is_some_and(|e| e.contains(words)))
}

/// A person's turn at the talky's door on `channel`, in `round` -- or in a
/// channel that declares none.
fn talky_turn(channel: &str, round: Option<&str>, text: &str) -> Message {
    let mut ctx = json!({"channel": channel});
    if let Some(r) = round {
        ctx["audience_set"] = json!(r);
    }
    MessageBuilder::new(Path::new("/talky"))
        .hop(map(json!({"route": "in_turn"})))
        .context(map(ctx))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(400)
        .build()
}

/// A consult at cogny's door in the shape a talky's dispatcher sends it, with
/// the round it carries on the real road (no edge on the way deletes
/// `audience_set`).
fn consult(consult_id: &str, question: &str) -> Message {
    MessageBuilder::new(Path::new("/cogny"))
        .hop(map(json!({"route": "in_turn", "consult_id": consult_id,
                        "session_id": CORE_SESSION, "tool_name": "consult_cogny"})))
        .context(map(
            json!({"consult_id": consult_id, "session_id": CORE_SESSION,
                            "col_phase": "", "audience_set": CORE_ROUND}),
        ))
        .body(Body::Inline(json!({"messages": [
            {"origin": "assistant", "type": "tool_call", "id": consult_id, "text": question}]})))
        .ttl(400)
        .build()
}

/// Another hive's pin of `text` at the talky's door (GH #892), in `round` or
/// in none.
fn pin(round: Option<&str>, text: &str) -> Message {
    let mut ctx = json!({});
    if let Some(r) = round {
        ctx["audience_set"] = json!(r);
    }
    MessageBuilder::new(Path::new("/talky"))
        .hop(map(json!({"route": "in_pin"})))
        .context(map(ctx))
        .body(Body::Inline(json!({"messages": [],
                                  "pins": [{"text": text, "source": PIN_SOURCE}]})))
        .ttl(400)
        .build()
}

/// The operator-forced sweep at the talky's door (`in_sweep`), contextless
/// like the night's firing: whatever round its close carries comes off the
/// generation row (the `gh273` sweep).
fn sweep() -> Message {
    MessageBuilder::new(Path::new("/talky"))
        .hop(map(json!({"route": "in_sweep"})))
        .body(Body::Inline(json!({"messages": []})))
        .ttl(400)
        .build()
}

// ═══════════════════════════════════════════════════════════════ the locks

/// Cases 1-4 and the row from before the rule: five sessions of one talky,
/// each in its own round, one after the other on one wall.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_round_reads_back_only_what_it_was_present_for() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let old = sha256_hex(&old_body());
    let read_old = json!({"id": format!("#{}", &old[..12])}).to_string();
    // Session two's words by the id of their block (review m-3); confirmed on
    // the wall below before the read's answer counts.
    let two = sha256_hex(&user_body(TWO_SAID));
    let read_two = json!({"id": format!("#{}", &two[..12])}).to_string();
    let brain = MockOpenAI::start(vec![
        // one: the words are said, nothing is looked up
        canned_chat_completion(SESSIONS[0].reply, "stop"),
        // two: one search
        canned_tool_calls(vec![(CALL_TWO, "history_search", SEARCH)]),
        canned_chat_completion(SESSIONS[1].reply, "stop"),
        // three: a search and an outline
        canned_tool_calls(vec![(CALL_THREE_SEARCH, "history_search", SEARCH)]),
        canned_tool_calls(vec![(CALL_THREE_OUTLINE, "history_outline", "{}")]),
        canned_chat_completion(SESSIONS[2].reply, "stop"),
        // four: a search, a read of the row from before the rule and a read
        // of session two's words by their id
        canned_tool_calls(vec![(CALL_FOUR_SEARCH, "history_search", SEARCH)]),
        canned_tool_calls(vec![(CALL_FOUR_READ, "history_read", read_old.as_str())]),
        canned_tool_calls(vec![(
            CALL_FOUR_READ_TWO,
            "history_read",
            read_two.as_str(),
        )]),
        canned_chat_completion(SESSIONS[3].reply, "stop"),
        // five: a search without a round
        canned_tool_calls(vec![(CALL_FIVE, "history_search", SEARCH)]),
        canned_chat_completion(SESSIONS[4].reply, "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().expect("tempdir");
    let root = td.path().to_path_buf();
    build(
        &td,
        &["talky"],
        &BTreeMap::from([("talky/brain", brain.base_url.clone())]),
    );
    sow_old(&root.join("main/talky/curator/ledger"));
    let (h, mut ports) = boot(&td).await;
    await_menu(&root, "talky").await;

    // The sessions one after the other; after each, how many provider calls
    // the brain has had, so every request is known by its session.
    let mut upto = Vec::new();
    for s in &SESSIONS {
        h.send(talky_turn(s.channel, s.round, s.says)).await;
        answer_saying(&mut ports.talky, &root, s.reply, s.channel).await;
        upto.push(brain.recorded_requests().await.len());
    }
    // Case 4 at the receiver: the refusal as the collector gets it. The log is
    // written behind the delivery, so it is polled.
    let deadline = Instant::now() + DEADLINE;
    let refused = loop {
        let found: Vec<Value> = message_log(&root)
            .into_iter()
            .filter(|r| r.to.starts_with("/talky/collector"))
            .map(|r| r.hop)
            .filter(|hop| hop["route"] == "in_tool" && hop["tool_call_id"] == CALL_FIVE)
            .collect();
        if !found.is_empty() || Instant::now() >= deadline {
            break found;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let wall = query(
        &ledger_db(&root, "talky"),
        "SELECT w.session_id, w.audience_set, b.body, w.hash FROM wall w \
         LEFT JOIN blocks b ON b.hash = w.hash ORDER BY w.seq",
    );
    let dead = dead_letters(&root);
    let reqs = brain.recorded_requests().await;
    h.shutdown().await;

    // Every request by its session.
    let mut from = 0;
    let mut of_session = Vec::new();
    for (s, &to) in SESSIONS.iter().zip(&upto) {
        assert_eq!(
            to - from,
            s.calls,
            "{}: {} provider call(s) expected, the brain had {upto:?} after each session. \
             Dead letters: {dead:#?}",
            s.channel,
            s.calls
        );
        of_session.push(&reqs[from..to]);
        from = to;
    }

    // The write side: every session's words stand on the wall under its
    // round, canonical -- and under `[]` when the round declared none
    // (PP-BD-12, GH #932: NULL is left to rows from before the rule).
    let mut hash_of = BTreeMap::new();
    for s in &SESSIONS {
        let prefix = format!("{}-", s.channel);
        let rows: Vec<&Vec<Option<String>>> = wall
            .iter()
            .filter(|r| r[0].as_deref().is_some_and(|sid| sid.starts_with(&prefix)))
            .filter(|r| r[2].as_deref().is_some_and(|b| b.contains(s.says)))
            .collect();
        assert!(
            !rows.is_empty(),
            "{}: the person's words are not on the wall: {wall:#?}",
            s.channel
        );
        let want = Some(s.round.map_or_else(|| "[]".to_string(), canonical));
        for r in &rows {
            assert_eq!(
                r[1], want,
                "{}: a row carries the audience of the round that caused it, canonical \
                 (GH #925): {r:?}",
                s.channel
            );
        }
        hash_of.insert(s.channel, rows[0][3].clone().unwrap_or_default());
    }
    let short = |channel: &str| hash_of[channel].chars().take(12).collect::<String>();
    let (one_id, two_id) = (short(SESSIONS[0].channel), short(SESSIONS[1].channel));

    // The probes below find the row from before the rule nowhere; first it is
    // found where it was sown -- its own session, its block, no audience -- so
    // they look for a row that is there (review m-2).
    assert!(
        wall.iter().any(|r| r[0].as_deref() == Some(OLD_SESSION)
            && r[1].is_none()
            && r[3].as_deref() == Some(old.as_str())
            && r[2].as_deref().is_some_and(|b| b.contains(OLD_SAID))),
        "the row from before the rule is not on the wall as sown (`{OLD_SESSION}`, no \
         audience, #{}): {wall:#?}",
        &old[..12]
    );
    // And the block session four reads by its id is session two's, in {e,a}.
    let two_prefix = format!("{}-", SESSIONS[1].channel);
    assert!(
        wall.iter().any(|r| r[0]
            .as_deref()
            .is_some_and(|sid| sid.starts_with(&two_prefix))
            && r[1].as_deref() == Some(canonical(ROUND_EA).as_str())
            && r[3].as_deref() == Some(two.as_str())),
        "session two's words do not stand on the wall under #{} in {{e,a}} -- the id \
         session four reads: {wall:#?}",
        &two[..12]
    );

    // The row from before the rule reaches no request of any round.
    for (s, rs) in SESSIONS.iter().zip(&of_session) {
        for r in rs.iter() {
            assert!(
                !wire(r).contains(OLD_SAID),
                "{}: a row without an audience reached a round (OR-BD-5): {}",
                s.channel,
                wire(r)
            );
        }
    }

    // 1. {e,a} reads what {e,a,b} said, under session one's id.
    let text = tool_text(of_session[1], CALL_TWO);
    let result = answered(&text);
    assert_eq!(result["tool"], "history_search", "{text}");
    assert!(
        hits(&result).iter().any(|h| {
            h["excerpt"].as_str().is_some_and(|e| e.contains(ONE_SAID))
                && h["session_id"]
                    .as_str()
                    .is_some_and(|sid| sid.starts_with(&format!("{}-", SESSIONS[0].channel)))
        }),
        "a round reads the words of a wider round it is a subset of: {text}"
    );
    assert!(
        !has_hit(&result, OLD_SAID) && !text.contains(&old[..12]),
        "the row from before the rule is a hit: {text}"
    );
    assert_eq!(
        result["total_hits"].as_u64(),
        Some(hits(&result).len() as u64),
        "the count names only the hits the round may see: {text}"
    );

    // 2. {e,a,b,c} reads nothing it was absent for: no hit, no count, no id,
    //    no session in the outline, no word in any request.
    let text = tool_text(of_session[2], CALL_THREE_SEARCH);
    let result = answered(&text);
    assert!(
        hits(&result).is_empty() && result["total_hits"].as_u64() == Some(0),
        "a wider round found rows it was absent for: {text}"
    );
    let text = tool_text(of_session[2], CALL_THREE_OUTLINE);
    let outline = answered(&text);
    let listed: Vec<String> = outline["sessions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|s| s["session_id"].as_str().unwrap_or_default().to_string())
        .collect();
    let three = format!("{}-", SESSIONS[2].channel);
    assert!(
        !listed.is_empty() && listed.iter().all(|sid| sid.starts_with(&three)),
        "the outline of {{e,a,b,c}} names its own session and no other (not session one, \
         not session two, not `{OLD_SESSION}`): {text}"
    );
    assert_eq!(
        outline["omitted_sessions"].as_u64(),
        Some(0),
        "a hidden session is no omitted one either: {text}"
    );
    for r in of_session[2].iter() {
        let w = wire(r);
        assert!(
            !w.contains(ONE_SAID) && !w.contains(TWO_SAID),
            "a request of {{e,a,b,c}} carries words of a round it was absent for: {w}"
        );
        assert!(
            !w.contains(&one_id) && !w.contains(&two_id),
            "a request of {{e,a,b,c}} carries the id of a block it may not see: {w}"
        );
    }

    // 3. {e,b} reads {e,a,b} (it holds the round) and nothing {e,a} wrote --
    //    not by a search, not by its id.
    let text = tool_text(of_session[3], CALL_FOUR_SEARCH);
    let result = answered(&text);
    assert!(
        has_hit(&result, ONE_SAID),
        "{{e,b}} reads session one's words -- {{e,a,b}} holds the round: {text}"
    );
    assert!(
        !has_hit(&result, TWO_SAID) && !has_hit(&result, OLD_SAID),
        "{{e,b}} found a row of {{e,a}} or one from before the rule: {text}"
    );
    assert_eq!(
        result["total_hits"].as_u64(),
        Some(hits(&result).len() as u64),
        "the count names only the hits the round may see: {text}"
    );
    let text = tool_text(of_session[3], CALL_FOUR_READ);
    let result = answered(&text);
    assert_eq!(
        result["error"], "not_found",
        "the row from before the rule is read by its id as if the wall never held it: {text}"
    );
    assert!(!text.contains(OLD_SAID), "{text}");
    let text = tool_text(of_session[3], CALL_FOUR_READ_TWO);
    let result = answered(&text);
    assert_eq!(
        result["error"], "not_found",
        "{{e,b}} reads a row of {{e,a}} by its id as if the wall never held it \
         (review m-3): {text}"
    );
    assert!(!text.contains(TWO_SAID), "{text}");
    for r in of_session[3].iter() {
        let w = wire(r);
        assert!(
            !w.contains(TWO_SAID),
            "a request of {{e,b}} carries what {{e,a}} wrote: {w}"
        );
        // The id stands in the model's own read of it and in the refusal
        // naming it; nowhere else.
        let w = wire_without_call(r, CALL_FOUR_READ_TWO);
        assert!(
            !w.contains(&two_id) && !w.contains(&two[..12]),
            "a request of {{e,b}} carries the id of a block of {{e,a}}: {w}"
        );
    }

    // 4. No round: refused at the curator, the refusal reaches the collector
    //    under the call id, the model reads it and nothing of the wall.
    assert!(
        !refused.is_empty(),
        "no tool result under `{CALL_FIVE}` reached the collector -- a refusal is an \
         answer too. Dead letters: {dead:#?}"
    );
    for hop in &refused {
        assert_eq!(
            hop["error_code"], "missing_audience",
            "a call in a round without an audience is refused: {hop:#?}"
        );
    }
    let text = tool_text(of_session[4], CALL_FIVE);
    assert!(
        text.contains("missing_audience"),
        "the model reads the refusal: {text}"
    );
    for r in of_session[4].iter() {
        let w = wire(r);
        assert!(
            !w.contains(ONE_SAID) && !w.contains(TWO_SAID),
            "a round without an audience sees words of another session (OR-BD-4): {w}"
        );
    }
}

/// Case 5: the core's round leaves one `tool_error`, one `correction` and one
/// `ask` mark, each under the round's audience.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_cores_marks_carry_the_round_they_were_made_in() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let core = MockOpenAI::start(vec![
        // Consult one: a read that fails, then the answer with a correction.
        canned_tool_calls(vec![(CORE_READ, "history_read", UNKNOWN_ID)]),
        canned_chat_completion(CORE_CORRECTED, "stop"),
        // Consult two: the core asks back -- a handoff, the round ends there.
        canned_tool_calls(vec![(CORE_ASK, "ask_requester", ASK_ARGS)]),
        // Never asked for; answered harmlessly should a fourth call come (the
        // stub repeats its last answer), so the count below says it.
        canned_chat_completion("Waiting for the answer.", "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().expect("tempdir");
    let root = td.path().to_path_buf();
    build(
        &td,
        &["cogny"],
        &BTreeMap::from([("cogny/brain", core.base_url.clone())]),
    );
    let (h, mut ports) = boot(&td).await;
    await_menu(&root, "cogny").await;

    // No mark of this run can be older than this.
    let started = chrono::Utc::now();
    h.send(consult(CONSULT_ONE, QUESTION_ONE)).await;
    answer_saying(&mut ports.cogny, &root, CORE_SAYS, "consult one").await;
    h.send(consult(CONSULT_TWO, QUESTION_TWO)).await;
    let asked = tokio::time::timeout(DEADLINE, ports.ask.recv())
        .await
        .ok()
        .flatten();
    assert!(
        asked.is_some(),
        "the core's question back never left it on `ask`. Dead letters: {:#?}",
        dead_letters(&root)
    );

    // The marks are written behind the tap and the next round: polled, then
    // read once more after the probe length, so a second copy would show.
    let db = ledger_db(&root, "cogny");
    let sql = "SELECT kind, value, audience_set, session_id, turn_id, at FROM marks \
               WHERE kind IN ('tool_error', 'ask', 'correction') ORDER BY seq";
    let deadline = Instant::now() + DEADLINE;
    loop {
        let kinds: BTreeSet<String> = query(&db, sql)
            .into_iter()
            .filter_map(|r| r[0].clone())
            .collect();
        if kinds.len() == 3 || Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(SETTLE).await;
    let marks = query(&db, sql);
    let ended = chrono::Utc::now();
    let reqs = core.recorded_requests().await;
    let dead = dead_letters(&root);
    h.shutdown().await;

    assert_eq!(
        reqs.len(),
        3,
        "the read, the answer after it, the question back -- nothing after a handoff. \
         Dead letters: {dead:#?}"
    );
    // At the receiver first: the failure the mark counts is the one the model read.
    let text = tool_text(&reqs[1..2], CORE_READ);
    assert_eq!(
        answered(&text)["error"],
        "not_found",
        "the read of an unknown id fails: {text}"
    );

    // Where and when each mark says it was made (review M-11). A consult
    // names its session on hop and context and no turn, so the collector
    // keeps the context's `session_id`, mints the round's turn (an `in_turn`
    // without a `turn_id` gets a uuid; `defer_turns` "0" gives every consult
    // a round of its own) and stamps both on `curate`. `./intake` takes them
    // off that hop on `in_curate` -- the `tool_error` of the read's result.
    // `./policy` hands the same hop on as `brain`, and the edge
    // `./curator -> ./brain` sets `context.session_id`/`context.turn_id` from
    // it, which `./intake` reads first on the tap -- the `correction` and the
    // `ask`. So every mark names CORE_SESSION, the failed read and the
    // correction name consult one's turn, the question back consult two's,
    // and `at` is the curator's clock (RFC 3339) at a moment of this run.
    let round = canonical(CORE_ROUND);
    let mut turns: BTreeMap<&str, String> = BTreeMap::new();
    for (kind, value) in [
        ("tool_error", "history_read:not_found"),
        ("correction", ""),
        ("ask", "ask_requester"),
    ] {
        let of_kind: Vec<&Vec<Option<String>>> = marks
            .iter()
            .filter(|r| r[0].as_deref() == Some(kind))
            .collect();
        assert_eq!(
            of_kind.len(),
            1,
            "exactly one `{kind}` mark for the one event (GH #925): {marks:#?}"
        );
        assert_eq!(
            of_kind[0][1].as_deref().unwrap_or_default(),
            value,
            "the `{kind}` mark's value: {marks:#?}"
        );
        assert_eq!(
            of_kind[0][2].as_deref(),
            Some(round.as_str()),
            "the `{kind}` mark carries the round it was made in, canonical: {marks:#?}"
        );
        assert_eq!(
            of_kind[0][3].as_deref(),
            Some(CORE_SESSION),
            "the `{kind}` mark names the consult's session: {marks:#?}"
        );
        let turn = of_kind[0][4].clone().unwrap_or_default();
        assert!(
            !turn.is_empty(),
            "the `{kind}` mark names the turn it was made in: {marks:#?}"
        );
        let at = of_kind[0][5].as_deref().unwrap_or_default();
        let when = chrono::DateTime::parse_from_rfc3339(at)
            .unwrap_or_else(|e| {
                panic!("the `{kind}` mark's `at` is no RFC 3339 time ({e}): {marks:#?}")
            })
            .with_timezone(&chrono::Utc);
        assert!(
            started <= when && when <= ended,
            "the `{kind}` mark says a moment of this run ({started} .. {ended}): {marks:#?}"
        );
        turns.insert(kind, turn);
    }
    assert_eq!(
        turns["tool_error"], turns["correction"],
        "the failed read and the correction are consult one's round, one turn: {marks:#?}"
    );
    assert_ne!(
        turns["ask"], turns["tool_error"],
        "the question back is consult two's round, a turn of its own: {marks:#?}"
    );
}

/// Case 6: a pin without a round is refused whole at `./intake` -- no row, no
/// emission of the curator, no dead letter; the same pin with a round is kept
/// once, under that round.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pin_without_a_round_is_refused_and_with_one_kept_under_it() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    let root = td.path().to_path_buf();
    // No brain is asked here: every `llm` cell talks to the closed port.
    build(&td, &["talky"], &BTreeMap::new());
    let (h, _ports) = boot(&td).await;
    await_menu(&root, "talky").await;
    let db = ledger_db(&root, "talky");
    let pins_sql = "SELECT p.source, p.audience_set, b.body FROM pins p \
                    LEFT JOIN blocks b ON b.hash = p.hash";

    // Without a round. The substrate logs every routing step once, with the
    // sender as `from`: a cell's emission as `/talky/curator/<cell>`, a hive's
    // transit as the hive. So on this pin's trace the curator shows exactly
    // one row -- the hive `/talky/curator` handing the pin to `./intake` --
    // and a second one would be an emission of a cell of it or a delivery
    // elsewhere (review m-1). That one row also says the refusal happened at
    // `./intake`, which the pin reached.
    let refused = pin(None, PIN_TEXT);
    let refused_trace = refused.trace_id.to_string();
    h.send(refused).await;
    tokio::time::sleep(SETTLE).await;
    let log = message_log(&root);
    let reached = log.iter().filter(|r| r.trace == refused_trace).count();
    let at_curator: Vec<&Logged> = log
        .iter()
        .filter(|r| r.trace == refused_trace && r.from.starts_with("/talky/curator"))
        .collect();
    let refused_dead = dead_letters_on(&root, &refused_trace);
    let pins_refused = query(&db, pins_sql);

    // The same pin with a round.
    let kept = pin(Some(PIN_ROUND), PIN_TEXT);
    let kept_trace = kept.trace_id.to_string();
    h.send(kept).await;
    let deadline = Instant::now() + DEADLINE;
    while query(&db, pins_sql).is_empty() {
        assert!(
            Instant::now() < deadline,
            "the pin with a round did not reach the ledger within {DEADLINE:?}. Dead \
             letters: {:#?}",
            dead_letters(&root)
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(SETTLE).await;
    let pins_kept = query(&db, pins_sql);
    let kept_emitted = message_log(&root)
        .iter()
        .filter(|r| r.trace == kept_trace && r.from.starts_with("/talky/curator/"))
        .count();
    let dead = dead_letters(&root);
    h.shutdown().await;

    assert!(
        reached > 0,
        "the pin without a round never entered the colony -- the probe below would say \
         nothing. Dead letters: {dead:#?}"
    );
    assert!(
        pins_refused.is_empty(),
        "a pin without a round was written (GH #925: `missing_audience`, nothing \
         written): {pins_refused:#?}"
    );
    assert_eq!(
        at_curator.len(),
        1,
        "on the trace of a pin it refuses the curator shows one row, its delivery into \
         `./intake` -- the refusal is a stderr line, no message: {at_curator:#?}"
    );
    assert_eq!(
        (
            at_curator[0].from.as_str(),
            at_curator[0].to.as_str(),
            at_curator[0].hop["route"].as_str()
        ),
        ("/talky/curator", "/talky/curator/intake", Some("in_pin")),
        "the one row is the hive handing the pin to `./intake`: {at_curator:#?}"
    );
    assert!(
        refused_dead.is_empty(),
        "a pin the curator refuses ends in no dead letter: {refused_dead:#?}"
    );
    assert!(
        kept_emitted > 0,
        "the pin with a round left no curator message on its trace -- the probe that \
         found none for the refused pin cannot see a written one either"
    );
    assert_eq!(
        pins_kept.len(),
        1,
        "exactly one `pins` row, the one with a round: {pins_kept:#?}"
    );
    let row = &pins_kept[0];
    assert_eq!(row[0].as_deref(), Some(PIN_SOURCE), "{row:?}");
    assert_eq!(
        row[1].as_deref(),
        Some(canonical(PIN_ROUND).as_str()),
        "the pin carries the round it was pinned in, canonical: {row:?}"
    );
    assert!(
        row[2].as_deref().is_some_and(|b| b.contains(PIN_TEXT)),
        "the row's block is the pinned text: {row:?}"
    );
}

/// Case 7: a pin reaches only the rounds its audience holds. Another hive
/// pins a text in {e,b} through the door of case 6; a new session in {e,b}
/// then finds it in its window -- the brain's request carries the text and its
/// id -- and a following session in {e,a}, in another channel, finds neither:
/// no request of it carries a word of the pin (five letters or more) or its
/// id. The window reads the pins hive-wide (`./policy` `w-pins`, of any
/// session) and gates them by the round alone, so a new session is exactly
/// where a pin of another round would show. The model's own pins (`source`
/// `model`, written by a `./policy` rebuild) need a clock stroke this lock
/// keeps quiet; `curator_policy.rs` locks that road.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pin_reaches_only_the_rounds_its_audience_holds() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let brain = MockOpenAI::start(vec![
        canned_chat_completion(SEVEN_EB_REPLY, "stop"),
        canned_chat_completion(SEVEN_EA_REPLY, "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().expect("tempdir");
    let root = td.path().to_path_buf();
    build(
        &td,
        &["talky"],
        &BTreeMap::from([("talky/brain", brain.base_url.clone())]),
    );
    let (h, mut ports) = boot(&td).await;
    await_menu(&root, "talky").await;
    let db = ledger_db(&root, "talky");

    // The pin first, and the sessions only once it stands in the ledger: a
    // window read before it could not show it.
    h.send(pin(Some(ROUND_EB), PIN_EB_TEXT)).await;
    let deadline = Instant::now() + DEADLINE;
    let pins = loop {
        let rows = query(&db, "SELECT audience_set FROM pins");
        if !rows.is_empty() {
            break rows;
        }
        assert!(
            Instant::now() < deadline,
            "the pin in {{e,b}} did not reach the ledger within {DEADLINE:?}. Dead letters: \
             {:#?}",
            dead_letters(&root)
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    h.send(talky_turn(SEVEN_EB, Some(ROUND_EB), SEVEN_EB_ASKS))
        .await;
    answer_saying(&mut ports.talky, &root, SEVEN_EB_REPLY, SEVEN_EB).await;
    let after_eb = brain.recorded_requests().await.len();
    h.send(talky_turn(SEVEN_EA, Some(ROUND_EA), SEVEN_EA_ASKS))
        .await;
    answer_saying(&mut ports.talky, &root, SEVEN_EA_REPLY, SEVEN_EA).await;
    let reqs = brain.recorded_requests().await;
    let dead = dead_letters(&root);
    h.shutdown().await;

    assert_eq!(
        pins,
        vec![vec![Some(canonical(ROUND_EB))]],
        "one pin, under the round it was pinned in"
    );
    assert_eq!(
        (after_eb, reqs.len()),
        (1, 2),
        "one provider call per session. Dead letters: {dead:#?}"
    );
    // The pin's id: its block, the pinned element as `./intake` hashes it,
    // shown by the first twelve hex digits.
    let element = json!({"type": "pin", "source": PIN_SOURCE, "text": PIN_EB_TEXT});
    let block = meclaw_core::serde_json::to_string(&element).expect("canonical: sorted keys");
    let id = sha256_hex(&block)[..12].to_string();

    // At the receiver, the positive side first: {e,b}'s window holds the pin.
    let w = wire(&reqs[0]);
    assert!(
        w.contains(PIN_EB_TEXT) && w.contains(&id),
        "the window of a new {{e,b}} session shows the pin its round holds (#{id}): {w}"
    );
    // And {e,a}'s holds nothing of it.
    let words: Vec<String> = PIN_EB_TEXT
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| w.len() >= 5)
        .map(str::to_lowercase)
        .collect();
    assert!(!words.is_empty(), "the pin has words to look for");
    for r in &reqs[after_eb..] {
        let w = wire(r).to_lowercase();
        for word in &words {
            assert!(
                !w.contains(word.as_str()),
                "a request of {{e,a}} carries `{word}` of a text pinned in {{e,b}}: {w}"
            );
        }
        assert!(
            !w.contains(&id),
            "a request of {{e,a}} carries the id of a pin it may not see: {w}"
        );
    }
}

/// Case 8: a closed session hands the memory only what its round was present
/// for. One channel, one generation: turn one in {e,a} opens it, and the
/// session keeper records {e,a} as the generation's round -- the close round
/// the curator's `./writer` gates by. Turn two in {e,b} and turn three without
/// a round join the open generation (the stamp records a round only when it
/// opens one), so the session's wall holds rows of {e,a}, of {e,b} and without
/// an audience; the ledger is read to show the three are there. Turns one and
/// two each run a tool round, so the batch's `rounds` -- the raw rows -- are
/// gated where there is something to gate: turn one's are in it, turn two's
/// not (review R2-I-5; without them the check over `rounds` ran over nothing).
/// A forced sweep closes the session, and the `write` batch that leaves the
/// talky -- at the park, where the rim drains it -- carries turn one and
/// nothing else.
/// Turn three's rows stand for the rows from before the rule: the session id
/// is minted when turn one opens the generation, so a row sown before the
/// boot could not name it without sowing the generation row too, and that
/// would set by hand the very round under test; NULL is NULL to the gate.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_closed_session_hands_the_memory_only_what_its_round_was_present_for() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let brain = MockOpenAI::start(vec![
        canned_tool_calls(vec![(CLOSE_ONE_CALL, "history_outline", "{}")]),
        canned_chat_completion(CLOSE_ONE_REPLY, "stop"),
        canned_tool_calls(vec![(CLOSE_TWO_CALL, "history_outline", "{}")]),
        canned_chat_completion(CLOSE_TWO_REPLY, "stop"),
        canned_chat_completion(CLOSE_THREE_REPLY, "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().expect("tempdir");
    let root = td.path().to_path_buf();
    build(
        &td,
        &["talky"],
        &BTreeMap::from([("talky/brain", brain.base_url.clone())]),
    );
    // Every open generation is a candidate the moment the sweep runs (the
    // `gh273` patch of the copied keeper).
    let close = root.join("main/talky/session-keeper/close/config.json");
    let mut cfg = read_json(&close);
    cfg["params"]["idle_ms"] = json!(0);
    write_json(&close, &cfg);
    let (h, mut ports) = boot(&td).await;
    await_menu(&root, "talky").await;

    for (round, says, reply) in [
        (Some(ROUND_EA), CLOSE_ONE_SAYS, CLOSE_ONE_REPLY),
        (Some(ROUND_EB), CLOSE_TWO_SAYS, CLOSE_TWO_REPLY),
        (None, CLOSE_THREE_SAYS, CLOSE_THREE_REPLY),
    ] {
        h.send(talky_turn(CLOSE_CHANNEL, round, says)).await;
        answer_saying(&mut ports.talky, &root, reply, CLOSE_CHANNEL).await;
    }

    // The ledger first, polled until turn three's answer stands on the wall
    // (the tap writes it behind the delivery): the close reads a wall that
    // holds all three turns.
    let db = ledger_db(&root, "talky");
    let prefix = format!("{CLOSE_CHANNEL}-");
    let deadline = Instant::now() + DEADLINE;
    let wall = loop {
        let rows: Vec<Vec<Option<String>>> = query(
            &db,
            "SELECT w.session_id, w.kind, w.final, w.audience_set, b.body, w.hash \
             FROM wall w LEFT JOIN blocks b ON b.hash = w.hash ORDER BY w.seq",
        )
        .into_iter()
        .filter(|r| r[0].as_deref().is_some_and(|sid| sid.starts_with(&prefix)))
        .collect();
        if rows.iter().any(|r| {
            r[4].as_deref()
                .is_some_and(|b| b.contains(CLOSE_THREE_REPLY))
        }) {
            break rows;
        }
        assert!(
            Instant::now() < deadline,
            "turn three's answer did not reach the wall within {DEADLINE:?}: {rows:#?}. \
             Dead letters: {:#?}",
            dead_letters(&root)
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    h.send(sweep()).await;
    let deadline = Instant::now() + DEADLINE;
    let mut parked = Vec::new();
    let batch = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, ports.park.recv()).await {
            Ok(Some(m)) => {
                let route = m.headers.hop.get("route").and_then(Value::as_str);
                if route == Some("write") {
                    break m;
                }
                parked.push(route.unwrap_or_default().to_string());
            }
            _ => panic!(
                "no `write` batch left the talky within {DEADLINE:?} of the sweep. Parked \
                 so far: {parked:?}. Dead letters: {:#?}",
                dead_letters(&root)
            ),
        }
    };
    let reqs = brain.recorded_requests().await;
    let dead = dead_letters(&root);
    h.shutdown().await;

    assert_eq!(
        reqs.len(),
        5,
        "two provider calls for each turn with a tool round, one for turn three. \
         Dead letters: {dead:#?}"
    );

    // The session, as the ledger holds it: ONE generation of the channel, and
    // in it every turn's words under the round they were said in -- {e,a},
    // {e,b} and none.
    let sessions: BTreeSet<String> = wall.iter().filter_map(|r| r[0].clone()).collect();
    assert_eq!(
        sessions.len(),
        1,
        "the three turns are one generation of the channel: {wall:#?}"
    );
    let session = sessions.into_iter().next().unwrap_or_default();
    let (ea, eb) = (canonical(ROUND_EA), canonical(ROUND_EB));
    for (words, want) in [
        (CLOSE_ONE_SAYS, Some(ea.as_str())),
        (CLOSE_ONE_REPLY, Some(ea.as_str())),
        (CLOSE_TWO_SAYS, Some(eb.as_str())),
        (CLOSE_TWO_REPLY, Some(eb.as_str())),
        // PP-BD-12 (GH #932): a round-less turn's rows carry `[]`, no longer
        // NULL -- NULL is left to rows from before the rule, which no reader
        // takes; `[]` is what the round-less reads of the session find.
        (CLOSE_THREE_SAYS, Some("[]")),
        (CLOSE_THREE_REPLY, Some("[]")),
    ] {
        let rows: Vec<&Vec<Option<String>>> = wall
            .iter()
            .filter(|r| r[4].as_deref().is_some_and(|b| b.contains(words)))
            .collect();
        assert!(!rows.is_empty(), "`{words}` is not on the wall: {wall:#?}");
        for r in rows {
            assert_eq!(
                r[3].as_deref(),
                want,
                "`{words}` stands under the round it was said in: {r:?}"
            );
        }
    }
    let audiences: BTreeSet<Option<String>> = wall.iter().map(|r| r[3].clone()).collect();
    assert_eq!(
        audiences,
        BTreeSet::from([Some(ea.clone()), Some(eb.clone()), Some("[]".to_string())]),
        "the session holds rows of {{e,a}}, of {{e,b}} and of no round (`[]`, \
         PP-BD-12): {wall:#?}"
    );
    // What the close round may hand on, counted off the ledger the way the
    // writer counts: a person's turn or a final answer is a said turn, every
    // other row a raw one.
    let of_ea: Vec<&Vec<Option<String>>> = wall
        .iter()
        .filter(|r| r[3].as_deref() == Some(ea.as_str()))
        .collect();
    let said_ea = of_ea
        .iter()
        .filter(|r| match r[1].as_deref().unwrap_or_default() {
            "user" | "peer" => true,
            "assistant" => r[2].as_deref() == Some("1"),
            _ => false,
        })
        .count();
    let ea_hashes: BTreeSet<String> = of_ea.iter().filter_map(|r| r[5].clone()).collect();

    // At the receiver: the batch carries turn one's words and nothing else.
    let Body::Inline(body) = &batch.body else {
        panic!("the batch is an inline body: {batch:?}");
    };
    let turns: Vec<(String, String)> = body["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|t| {
            (
                t["origin"].as_str().unwrap_or_default().to_string(),
                t["text"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(
        turns,
        vec![
            ("user".to_string(), CLOSE_ONE_SAYS.to_string()),
            ("assistant".to_string(), CLOSE_ONE_REPLY.to_string()),
        ],
        "the closed session hands the memory exactly what its round {{e,a}} was present \
         for: turn one's question and answer, nothing of {{e,b}} and nothing without an \
         audience (GH #925, review I-3): {body:#}"
    );
    let whole = body.to_string();
    for later in CLOSE_LATER {
        assert!(
            !whole.contains(later),
            "the batch of the {{e,a}} close carries `{later}`: {body:#}"
        );
    }
    let raw = body["rounds"].as_array().cloned().unwrap_or_default();
    // The check below must run over something (review R2-I-5): turn one's
    // tool round is in the batch, so a writer that gated `messages[]` and not
    // `rounds` would hand turn two's round on beside it and fail here.
    assert!(
        !raw.is_empty() && whole.contains(CLOSE_ONE_CALL),
        "the batch carries turn one's tool round: {body:#}"
    );
    let eb_raw: Vec<&Vec<Option<String>>> = wall
        .iter()
        .filter(|r| {
            r[3].as_deref() == Some(eb.as_str())
                && r[4].as_deref().is_some_and(|b| b.contains(CLOSE_TWO_CALL))
        })
        .collect();
    assert!(
        !eb_raw.is_empty(),
        "turn two's tool round stands on the wall under {{e,b}}: {wall:#?}"
    );
    assert!(
        !whole.contains(CLOSE_TWO_CALL),
        "the batch of the {{e,a}} close carries turn two's tool round: {body:#}"
    );
    let mut checked = 0;
    for r in &raw {
        assert!(
            r["hash"].as_str().is_some_and(|h| ea_hashes.contains(h)),
            "a raw row of the batch is no row of {{e,a}}: {r:#}"
        );
        checked += 1;
    }
    assert!(checked > 0, "the gate over `rounds` ran over nothing");
    for r in &eb_raw {
        let h = r[5].as_deref().unwrap_or_default();
        assert!(
            !raw.iter().any(|x| x["hash"].as_str() == Some(h)),
            "a raw row of {{e,b}} is in the batch: {h}"
        );
    }
    let hop = |key: &str| {
        batch
            .headers
            .hop
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    assert_eq!(
        hop("session_id"),
        session,
        "the batch names the session it closes"
    );
    assert_eq!(
        (hop("turn_count"), hop("round_count")),
        (said_ea.to_string(), (of_ea.len() - said_ea).to_string()),
        "the counts name what the batch carries, the {{e,a}} rows of the ledger: {body:#}"
    );
    assert_eq!(
        said_ea, 2,
        "turn one is one question and one answer: {wall:#?}"
    );
    assert_eq!(
        raw.len(),
        of_ea.len() - said_ea,
        "every raw row of {{e,a}} and no other: {body:#}"
    );
    // And it leaves under the round it was gated for, off the generation row.
    assert_eq!(
        batch
            .headers
            .context
            .get("audience_set")
            .and_then(Value::as_str)
            .map(canonical),
        Some(ea),
        "the batch leaves under the generation's round: {:?}",
        batch.headers.context
    );
}
