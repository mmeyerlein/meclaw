//! never-forgets -- `examples/never-forgets` really answers a question about
//! February in March.
//!
//! The example makes one claim, and it is a claim about TIME rather than about
//! retrieval: a turn that happened months ago is still there, it still knows
//! WHEN it happened, and an agent can ask for a date range on purpose -- not
//! because somebody upstream guessed the range for it, but because the model
//! that read the question named it.
//!
//! So this file takes the shipped seed and the shipped `grow.json` verbatim --
//! no inlined copy, no paraphrase -- boots the one, applies the other, and
//! drives the whole story:
//!
//!   1. THREE MONTHS GO IN, model-free. Prepared turns with staggered
//!      `happened_at` enter over the import lane, speak the episode port
//!      directly and land in the episode table, each under the instant it was
//!      said. The only clock this leg touches is `recorded_at`.
//!   2. A QUESTION COMES IN TODAY. The brain (mock wire) answers it with a
//!      `memory_recall` tool call carrying a window over mid-February. The
//!      dispatcher routes it by name, the collector serves it on the recall
//!      port it already owns, the memory answers with the episodes of that
//!      window -- and the tool result the model sees on the SECOND inference
//!      carries the February sentence, with its date, and neither the January
//!      nor the March one.
//!   3. AND THE TURN ITSELF IS ALREADY IN. Per-turn freshness: the question of
//!      step 2 is an episode while its session is still open, so a colony that
//!      is asked twice in one conversation can answer about the first time.
//!
//! Free of a real provider by construction: the ingest leg is `code` and
//! `store` cells end to end, and the only `llm` cell in the tree talks to the
//! mock OpenAI wire. The file spends nothing.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_openai::{MockOpenAI, OpenAiRequestSnapshot, canned_chat_completion, canned_tool_calls};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

// ─────────────────────────────────────────────────────── the shipped example

fn repo_path(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn example_path(rel: &str) -> std::path::PathBuf {
    repo_path("examples/never-forgets").join(rel)
}

/// The templates `grow.json` names, each next to the path this test really
/// reads, in the order the declaration names them. All three are part of the
/// public distribution, so the file runs in the open clone exactly as it does
/// here -- and the paths are spelled out rather than formatted, so the export's
/// R2b check can read the names off them (GH #9: a runtime path a gate cannot
/// see is the whole defect class).
const GROWN_FROM: [(&str, &str); 3] = [
    ("door", "templates/door"),
    ("talky", "templates/talky"),
    ("terminal", "templates/terminal"),
];

/// Three checked-in cells (the import lane plus the memory's two -- the hive
/// marker is a scope, not a cell) and fourteen grown ones: one from `door@1`,
/// twelve from `talky` (the tenth is the sidecar splitter, talky@4.1.0, GH #379;
/// the summarizer's two left with talky@4.3.0, GH #447; the eleventh is the
/// collector's `menu-clock`, collector@3.3.0, GH #464; the twelfth is the
/// keeper's own `porter`, session-keeper@2.1.0, GH #471), one from `terminal@1`.
const CELLS_AFTER_GROW: usize = 17;

/// GH #277: `talky` REFERENCES its three sub-units instead of carrying copies
/// of them, so the library the colony scans has to hold them next to it. They
/// are NOT `grow.json` entries -- the mutation still names `talky` alone, and
/// the registry resolves the rest. The `summarizer` left the set with
/// `talky@4.3.0` (GH #447).
const REFERENCED_SUB_UNITS: [(&str, &str); 3] = [
    ("collector", "templates/collector"),
    ("session-keeper", "templates/session-keeper"),
    ("dispatcher", "templates/dispatcher"),
];

/// The three months of the story live in `past.jsonl`, next to the seed -- the
/// file the README's import loop reads is the file this test replays. These are
/// the marks it is measured by: the February one is the answer, the other two
/// are the reason a window has to be a window and not a search.
const JAN_MARK: &str = "kitchen radiator";
const FEB_MARK: &str = "Danfoss RA-N";
const FEB_INSTANT: &str = "2026-02-14T11:05:00.000Z";
const MAR_MARK: &str = "bike lock";

/// One line of the shipped past: a session, an instant, a speaker, a sentence.
struct PastTurn {
    session: String,
    at: String,
    origin: String,
    text: String,
}

/// The example's own month of history, read the way the README reads it.
fn shipped_past() -> Vec<PastTurn> {
    let raw = std::fs::read_to_string(example_path("past.jsonl")).expect("past.jsonl");
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let v: Value = meclaw_core::serde_json::from_str(l).expect("one json object per line");
            PastTurn {
                session: v["session"].as_str().expect("session").to_string(),
                at: v["at"].as_str().expect("at").to_string(),
                origin: v["origin"].as_str().expect("origin").to_string(),
                text: v["text"].as_str().expect("text").to_string(),
            }
        })
        .collect()
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Copy a directory tree verbatim -- template seeds travel too.
fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

fn patch(p: &std::path::Path, f: impl FnOnce(&mut Value)) {
    let mut v = read_json(p);
    f(&mut v);
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

// ══════════════════════════════════ 1. what is checked in, and what it names

/// The premise, measured rather than asserted in prose: the seed carries the
/// memory and the way into it, and NOT one line of agent. Every cell that
/// talks, sessions, screens or summarises arrives from the library.
#[test]
fn the_seed_carries_the_memory_and_nothing_that_talks() {
    let mut files: Vec<String> = Vec::new();
    fn walk(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).expect("seed dir").flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk(&p, base, out);
            } else {
                out.push(
                    p.strip_prefix(base)
                        .expect("under seed")
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    let seed = example_path("seed");
    walk(&seed, &seed, &mut files);
    files.sort();
    assert_eq!(
        files,
        vec![
            "colony.json".to_string(),
            "main/config.json".to_string(),
            "main/memory/config.json".to_string(),
            "main/memory/episodes/config.json".to_string(),
            "main/memory/keep/config.json".to_string(),
            "main/replay/config.json".to_string(),
        ],
        "the seed grew a file -- either it belongs in a template or the README lies"
    );

    let hive = read_json(&seed.join("main/config.json"));
    assert_eq!(hive["cell"]["type"], json!("hive"));
    assert_eq!(
        hive["params"]["graph"]["edges"],
        json!([]),
        "the root hive draws NO edge: every edge of this colony is in grow.json"
    );

    // The bi-temporal split is a COLUMN, not a convention. If either time
    // disappears from the table, the whole example is a search example.
    let store = read_json(&seed.join("main/memory/episodes/config.json"));
    let cols = &store["params"]["schema"]["episodes"];
    assert_eq!(cols["happened_at"], json!("text"), "when it was said");
    assert_eq!(cols["recorded_at"], json!("text"), "when we learned it");
}

/// `grow.json` names only templates that ship publicly -- otherwise the example
/// is a promise the open clone cannot keep.
#[test]
fn grow_json_only_names_templates_that_ship() {
    let grow = read_json(&example_path("grow.json"));
    let named: Vec<&str> = grow["diff"]["add_nodes"]
        .as_array()
        .expect("add_nodes")
        .iter()
        .map(|n| n["template"].as_str().expect("template"))
        .collect();
    let expected: Vec<&str> = GROWN_FROM.iter().map(|(name, _)| *name).collect();
    assert_eq!(named, expected, "grow.json grew a template: {named:?}");
    for (name, dir) in GROWN_FROM {
        assert!(
            repo_path(dir).join("template.json").is_file(),
            "{name}@1 is missing from the tree this test runs in"
        );
    }

    // The two edges GH #78 is made of. Without the first the model's call is
    // routed nowhere; without the second the collector asks into a void and the
    // round parks until the idle exit. Both are the PARENT's job -- `talky`
    // ships neither, on purpose.
    let edges = grow["diff"]["add_edges"].as_array().expect("add_edges");
    // Since GH #228 sealed `talky`, both ends of that loopback are the hive
    // path: the tool call leaves on the `tool` lane and comes back in on
    // `in_memory_call`. Which cell made the call and which one takes it is no
    // longer anything this example knows.
    assert!(
        edges.iter().any(|e| e["from"] == json!("./talky")
            && e["to"] == json!("./talky")
            && e["modifier"]["set_hop"]["route"] == json!("'in_memory_call'")),
        "the memory tool loopback is not wired"
    );
    let recall = edges
        .iter()
        .find(|e| {
            e["condition"]
                .as_str()
                .is_some_and(|c| c.contains("hop.route == 'recall'"))
        })
        .expect("the recall port is not wired");
    for key in ["recall_window_from", "recall_window_to", "memory_call_id"] {
        assert_eq!(
            recall["modifier"]["set_context"][key],
            json!(format!("hop.{key}")),
            "the recall edge drops {key}, so the window never reaches the memory"
        );
    }
}

/// GH #220: the per-turn lane is named IN the declaration, not by forking the
/// library -- and since GH #298 it is named there rather than switched on there,
/// because the library ships it on.
///
/// `turn_write` is what makes the memory fresh during a session instead of at
/// the nightly close, and the example used to set it by copying the whole
/// template library and editing one `config.json`. Since GH #140
/// `override_params` on a subtree template is addressed by the cell's path
/// inside it, so one key on the `talky` node does the same thing where the
/// reader can see it.
///
/// This is pinned rather than left to the freshness assertion alone because of
/// how GH #203 failed: a documented setup step that writes a key nothing reads
/// brings the example up with NO per-turn writes and says nothing about it. The
/// path `collector/assemble` is the one that must survive here -- `collector`
/// alone is the sub-unit's hive, and a hive reads `graph`/`ports`/
/// `required_drains`/`contract` and would swallow this key in silence (GH #212).
#[test]
fn grow_json_sets_the_per_turn_lane_at_instantiation() {
    let grow = read_json(&example_path("grow.json"));
    let talky = grow["diff"]["add_nodes"]
        .as_array()
        .expect("add_nodes")
        .iter()
        .find(|n| n["template"] == json!("talky"))
        .expect("the talky node");
    assert_eq!(
        talky["override_params"]["collector/assemble"]["turn_write"],
        json!("1"),
        "the per-turn lane left the declaration -- either it moved somewhere a \
         reader can see, or this example is back to a freshness hole of up to a day"
    );

    // Since GH #298 the library ships the lane ON, so the override no longer
    // MAKES the difference -- it declares, where the reader reads, which knob
    // this example depends on. What it must never do is disagree with the
    // shipped value: a `"0"` here would switch the example's own subject off.
    let shipped = read_json(&repo_path("templates/collector/assemble/config.json"));
    assert_eq!(
        shipped["params"]["turn_write"],
        json!("1"),
        "the library stopped shipping the per-turn lane on -- then this example \
         depends on its override again and the README paragraph must say so"
    );
    assert_eq!(
        talky["override_params"]["collector/assemble"]["turn_write"],
        shipped["params"]["turn_write"],
        "the declaration and the library disagree about the example's own lane"
    );
}

/// GH #298, ruling Q11: `memory-drain` left the live stack.
///
/// The decomposer between the talky and the memory is gone from this
/// declaration -- not bypassed, not renamed: no edge of the file names
/// `./drain` any more, and the per-turn route speaks the episode port itself.
/// Since GH #298 the collector emits ONE turn per message on `turn_write`, with
/// `turn_id`/`turn_index`/`happened_at` on the hop, which is exactly the shape
/// the port reads -- so a decomposer in between has nothing left to decompose.
///
/// The modifier is asserted against the IMPORT lane rather than spelled out: the
/// whole subject of this example is that both producers reach the same port in
/// the same shape, and two edges that promote different keys would be two ports
/// wearing one name.
#[test]
fn grow_json_wires_the_per_turn_route_straight_at_the_memory() {
    let grow = read_json(&example_path("grow.json"));
    let edges = grow["diff"]["add_edges"].as_array().expect("add_edges");
    assert!(
        !edges
            .iter()
            .any(|e| e["from"] == json!("./drain") || e["to"] == json!("./drain")),
        "an edge still names ./drain -- the drain is back on the live path"
    );

    let turn_write = edges
        .iter()
        .find(|e| {
            e["condition"]
                .as_str()
                .is_some_and(|c| c.contains("hop.route == 'turn_write'"))
        })
        .expect("the per-turn route is not wired at all");
    assert_eq!(
        turn_write["from"],
        json!("./talky"),
        "the per-turn route no longer leaves the talky"
    );
    assert_eq!(
        turn_write["to"],
        json!("./memory/keep"),
        "the per-turn route ends somewhere other than the episode port"
    );

    let import = edges
        .iter()
        .find(|e| e["from"] == json!("./replay"))
        .expect("the import lane is not wired");
    assert_eq!(
        turn_write["modifier"], import["modifier"],
        "the live lane and the import lane no longer speak the port the same \
         way -- one port with two shapes is two ports"
    );
}

// ══════════════════════════════════════════════════ 2. the colony under test

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![
        (
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        ),
        ("store".to_string(), Arc::new(StoreCellFactory)),
        ("timer".to_string(), Arc::new(TimerCellFactory)),
        ("llm".to_string(), Arc::new(LlmCellFactory)),
    ]
}

/// Never during a test run: the shipped keeper default is the real night, and
/// the shipped menu tick (GH #464) would ask a tools hive this example has not
/// got -- one dead letter every five minutes, against an example whose whole
/// claim is an empty queue.
const NEVER: &str = "0 0 0 1 1 *";

/// The seed as the reader gets it, plus the template library next to it and the
/// `.env` the README asks for. Only the provider endpoint is bent -- towards
/// the mock wire, so the run costs nothing.
fn build_root(td: &tempfile::TempDir, base_url: &str) {
    let root = td.path();
    copy_tree(&example_path("seed"), root);
    for (name, dir) in GROWN_FROM.iter().chain(REFERENCED_SUB_UNITS.iter()) {
        copy_tree(&repo_path(dir), &root.join("templates").join(name));
    }
    patch(&root.join("templates/talky/brain/config.json"), |v| {
        v["params"]["base_url"] = json!(base_url)
    });
    // The per-turn lane is NOT patched here any more (GH #220). It is a
    // collector param, and since GH #140 `override_params` reaches a subtree
    // template's sub-cells by path, so `grow.json` carries
    // `{"collector/assemble": {"turn_write": "1"}}` on the talky node -- the
    // declaration the reader POSTs is the whole setting, and this test applies
    // that file verbatim. `grow_json_sets_the_per_turn_lane_at_instantiation`
    // pins the key; the freshness assertion at the end of the main test is what
    // proves it arrived. `memory_call_tier` needs no setting: `"1"` is the
    // shipped default.
    //
    // The library copy above stays, and not for this: WALKTHROUGH step 2 writes
    // the brain's `system.jsonl` INTO it. A seed is a file in the template's
    // `seed/` directory, read once at spawn -- there is no override_params for
    // it, so the reader who wants the model to see `memory_recall` needs a
    // library he may write to.
    seed_the_recall_tool(root);
    std::fs::write(
        root.join(".env"),
        format!(
            "OPENROUTER_API_KEY=test-key\nMODEL_BRAIN=gpt-4o-mock\n\
             KEEPER_NIGHT_CRON={NEVER}\nMENU_CRON={NEVER}\n"
        ),
    )
    .unwrap();
}

/// WALKTHROUGH Step 2, executed (GH #142).
///
/// The step the example does not work without, and the one this test used to
/// skip: the `memory_recall` lane is wired in `grow.json`, but a wired lane is
/// not a tool the model can see. The schema lives next to the brain, and the
/// `talky` composite ships none on purpose -- identity, instructions and tools
/// are the agent, not the graph.
///
/// Skipping it here is what made this file a green test of a broken example: a
/// canned tool call arrives in canonical form whatever the brain was told, so
/// the missing seed was invisible until a real provider confabulated an answer
/// instead of calling anything. Running the documented step and then asserting
/// the wire (`assert_tool_was_offered`) closes both halves.
fn seed_the_recall_tool(root: &std::path::Path) {
    let dir = root.join("templates/talky/brain/seed");
    std::fs::create_dir_all(&dir).expect("brain seed dir");
    let tool = json!({
        "type": "function",
        "function": {
            "name": "memory_recall",
            "description": "Ask long-term memory about something, optionally restricted to a time range.",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "what to look for"},
                    "window_from": {"type": "string", "description": "ISO-8601 start of the range (optional)"},
                    "window_to": {"type": "string", "description": "ISO-8601 end of the range (optional)"}
                },
                "required": ["query"]
            }
        }
    });
    let instructions = "Today is 2026-08-15. You have a long-term memory you cannot see. \
         When the user asks about something said in the past, call memory_recall FIRST -- \
         never answer from your own recollection. If the question names a month or a period, \
         pass it as window_from/window_to in ISO-8601 UTC. Answer only from what memory \
         returns; if it returns nothing for that window, say so plainly.";
    // The tool leaf holds the provider-native object as a JSON STRING -- the
    // whole `{"type":"function",…}` envelope, not the inner schema. Seed only
    // the inner half and the provider rejects the call.
    let lines = [
        json!({"schema": {"slot_path": "text", "value": "json", "updated_at": "int"}}),
        json!({"slot_path": "instructions.memory",
               "value": {"text": instructions}, "updated_at": 0}),
        json!({"slot_path": "tools.memory_recall",
               "value": {"text": tool.to_string()}, "updated_at": 0}),
    ];
    let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(dir.join("system.jsonl"), body).expect("write the brain seed");
}

async fn boot(td: &tempfile::TempDir) -> ColonyHandle {
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the seed must boot");
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan");
    ack_rx
        .await
        .expect("rescan ack")
        .expect("GH #440: the rescan must not have aborted");
    h
}

/// The shipped declaration, applied verbatim -- the file the reader `curl`s at
/// `/colony/mutations` is the file this test hands the colony.
async fn grow(h: &ColonyHandle) -> meclaw_colony::mutation::MutationOutcome {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: read_json(&example_path("grow.json")),
            reply_to: None,
            trace_id: meclaw_core::Uuid::now_v7(),
            parent_message_id: meclaw_core::Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("send mutation");
    ack_rx.await.expect("mutation ack")
}

async fn registry_paths(h: &ColonyHandle) -> Vec<String> {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 1000,
            ack: ack_tx,
        })
        .await
        .expect("read registry");
    let mut v: Vec<String> = ack_rx
        .await
        .expect("registry ack")
        .entries
        .into_iter()
        .map(|e| e.path)
        .collect();
    v.sort();
    v
}

/// One turn out of the past, on the import lane. The instant it was said rides
/// in the request headers, which land in `context` -- a UBF turn carries
/// origin, type, text and id and NOTHING else, so a timestamp glued to the turn
/// would be refused at the delivery boundary before any cell saw it.
fn past(t: &PastTurn) -> Message {
    let mut ctx = meclaw_core::serde_json::Map::new();
    ctx.insert("session_id".into(), json!(t.session));
    ctx.insert("happened_at".into(), json!(t.at));
    MessageBuilder::new(Path::new("/replay"))
        .body(Body::Inline(
            json!({"messages": [{"origin": t.origin, "type": "text", "text": t.text}]}),
        ))
        .context(ctx)
        .ttl(200)
        .build()
}

/// A turn on the conversation surface, today.
fn turn(channel: &str, text: &str) -> Message {
    let mut ctx = meclaw_core::serde_json::Map::new();
    ctx.insert("channel".into(), json!(channel));
    MessageBuilder::new(Path::new("/door"))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .context(ctx)
        .ttl(200)
        .build()
}

fn texts(m: &Message) -> Vec<String> {
    match &m.body {
        Body::Inline(v) => v["messages"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|t| t["text"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        Body::Blob(_) => Vec::new(),
    }
}

fn rows(db: &std::path::Path, sql: &str) -> Vec<Vec<String>> {
    let Ok(conn) = rusqlite::Connection::open(db) else {
        return vec![];
    };
    let Ok(mut stmt) = conn.prepare(sql) else {
        return vec![];
    };
    let n = stmt.column_count();
    let Ok(mapped) = stmt.query_map([], |r| {
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            out.push(r.get::<_, Option<String>>(i)?.unwrap_or_default());
        }
        Ok(out)
    }) else {
        return vec![];
    };
    mapped.map(|r| r.expect("row")).collect()
}

/// Polls a query until it returns at least `n` rows. 30 s failure marker,
/// robust under cargo parallel load.
fn await_rows(db: &std::path::Path, sql: &str, n: usize) -> Vec<Vec<String>> {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let r = rows(db, sql);
        if r.len() >= n {
            return r;
        }
        if std::time::Instant::now() > deadline {
            panic!("{} of {n} rows after 30s for {sql:?}: {r:?}", r.len());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

const EPISODES: &str = "SELECT happened_at, sender, content FROM episodes \
                        ORDER BY happened_at ASC";

/// The one thing the mock cannot fake (GH #142).
///
/// Everything else in this file is downstream of a canned tool call: the mock
/// emits `memory_recall` in canonical form whatever the brain was told, so the
/// lane can be wired perfectly and green while the model on the other end never
/// learned the tool exists. That is not hypothetical -- it is how this example
/// shipped: the recall lane was wired, `system.tools` was never seeded, and a
/// real provider confabulated the answer instead of calling anything. The pin
/// test stayed green through all of it.
///
/// So the wire is asserted from the other side: whatever the brain ANSWERED, it
/// must have been OFFERED the schema the lane depends on. A missing seed step
/// now turns this file red instead of leaving it a green test of a broken
/// example.
fn assert_tool_was_offered(req: &OpenAiRequestSnapshot, tool: &str) {
    let tools = req.tools().unwrap_or_else(|| {
        panic!("the brain was called with no `tools` at all -- the recall lane depends on {tool:?}")
    });
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t.get("function")?.get("name")?.as_str())
        .collect();
    assert!(
        names.contains(&tool),
        "the brain was never told about {tool:?}; it was offered {names:?}. \
         Wiring the lane is half the job -- the schema has to reach system.tools"
    );
}

// ═══════════════════════════════════════════════ 3. the whole claim in one run

/// Three months in, one question out, and the answer is dated.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_january_sentence_is_still_there_in_march_with_its_date() {
    // The model reads the question and decides the range itself. That decision
    // is the whole point of GH #78 -- before it, nobody who had SEEN the turn
    // was ever the one asking memory.
    let mock = MockOpenAI::start(vec![
        canned_tool_calls(vec![(
            "call-m1",
            "memory_recall",
            r#"{"query":"radiator valve","window_from":"2026-02-01T00:00:00.000Z","window_to":"2026-02-28T23:59:59.000Z"}"#,
        )]),
        canned_chat_completion("A Danfoss RA-N, and it needs a new insert.", "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_root(&td, &mock.base_url);

    let h = boot(&td).await;
    let outcome = grow(&h).await;
    assert!(
        matches!(
            outcome,
            meclaw_colony::mutation::MutationOutcome::Committed { .. }
        ),
        "grow.json was not committed: {outcome:?}"
    );
    let after = registry_paths(&h).await;
    for expected in [
        "/replay",
        "/memory/keep",
        "/memory/episodes",
        "/door",
        "/talky/session-keeper/stamp",
        "/talky/collector/assemble",
        "/talky/dispatcher",
        "/talky/splitter",
        "/talky/brain",
        "/sink",
    ] {
        assert!(
            after.iter().any(|p| p == expected),
            "{expected} did not grow: {after:?}"
        );
    }
    // GH #298, ruling Q11: the drain is not in the live stack any more, so it
    // is not in the inventory either -- neither of its two cells grows here.
    for gone in ["/drain/drain", "/drain/ledger"] {
        assert!(
            !after.iter().any(|p| p == gone),
            "{gone} still grows: the drain is back on the live path: {after:?}"
        );
    }
    assert_eq!(after.len(), CELLS_AFTER_GROW, "{after:?}");

    let db = td.path().join("main/memory/episodes/cell.db");

    // ─────────────────────────────────────────── 1. three months, model-free
    // The shipped past, replayed line by line: three sessions, months apart,
    // because that is what they were -- not one long day.
    let history = shipped_past();
    assert!(
        history.len() > 6,
        "past.jsonl shrank to {} turns",
        history.len()
    );
    for t in &history {
        h.send(past(t)).await;
    }

    let stored = await_rows(&db, EPISODES, history.len());
    let mut expected: Vec<String> = history.iter().map(|t| t.at.clone()).collect();
    expected.sort();
    let times: Vec<String> = stored.iter().map(|r| r[0].clone()).collect();
    assert_eq!(
        times, expected,
        "the import stamped its own clock instead of keeping the event time"
    );
    assert!(
        stored
            .iter()
            .any(|r| r[0] == FEB_INSTANT && r[2].contains(FEB_MARK)),
        "the February sentence is not in the store under its own instant: {stored:?}"
    );

    // ─────────────────────────────────── 2. the question, and what the model saw
    // The probe watches the collector's exits; the example sends them to a
    // terminal, and a terminal that swallows leaves nothing to assert.
    let (probe_tx, mut probe_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/probe"), move || {
        CaptureCell::new(probe_tx.clone())
    })
    .await;
    h.add_edge(
        meclaw_core::Uuid::now_v7(),
        Path::new("/talky/collector/assemble"),
        Path::new("/probe"),
    )
    .await;

    h.send(turn(
        "chat-1",
        "What did the plumber say about the radiator in February?",
    ))
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let mut seen: Vec<String> = Vec::new();
    let answer = loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !left.is_zero(),
            "no answer reached the reply port: {seen:?}"
        );
        let Ok(Some(m)) = tokio::time::timeout(left, probe_rx.recv()).await else {
            panic!("the reply port went quiet: {seen:?}");
        };
        let t = texts(&m);
        seen.push(format!("{:?}", m.headers.hop.get("route")));
        if t.iter()
            .any(|s| s == "A Danfoss RA-N, and it needs a new insert.")
        {
            break m;
        }
    };
    assert_eq!(
        answer.headers.hop.get("route").and_then(|v| v.as_str()),
        Some("answer"),
        "the model's words came home on some other lane: {:?}",
        answer.headers.hop
    );

    // THE CLAIM. The second inference is the one that ran with the memory in
    // it, and this is what the provider was handed: the February sentence,
    // carrying the date it happened -- and neither neighbour.
    let requests = mock.recorded_requests().await;
    assert_eq!(
        requests.len(),
        2,
        "expected two inferences, got {}",
        requests.len()
    );
    let prompt = meclaw_core::serde_json::to_string(
        requests[1]
            .messages()
            .expect("the second prompt has messages"),
    )
    .expect("serialise");
    assert!(
        prompt.contains(FEB_MARK),
        "the model never saw February: {prompt}"
    );
    assert!(
        prompt.contains(FEB_INSTANT),
        "February arrived without its date, so the answer cannot be dated: {prompt}"
    );
    assert!(
        !prompt.contains(JAN_MARK),
        "January leaked into a February window: {prompt}"
    );
    assert!(
        !prompt.contains(MAR_MARK),
        "March leaked into a February window: {prompt}"
    );

    // ────────────────────────────────── 3. and today's turn is already in
    // Per-turn freshness: the question of step 2 is an episode while its
    // session is still open. Without this lane the first row would appear at
    // the night close -- a freshness hole of up to 24 hours.
    let fresh = await_rows(
        &db,
        "SELECT content FROM episodes WHERE content LIKE '%plumber say%'",
        1,
    );
    assert_eq!(
        fresh.len(),
        1,
        "today's turn is not in memory yet: {fresh:?}"
    );

    // GH #142: the answer above came back through a canned tool call. This is
    // the assertion that the model could have MADE that call for real.
    let requests = mock.recorded_requests().await;
    assert_tool_was_offered(&requests[0], "memory_recall");

    let dlq: i64 = rusqlite::Connection::open(td.path().join("colony.db"))
        .expect("colony.db")
        .query_row("SELECT COUNT(*) FROM dead_letters", [], |r| r.get(0))
        .expect("dead_letters count");
    assert_eq!(dlq, 0, "something in this example has nowhere to go");

    h.shutdown().await;
}

/// The counter-question, and the reason the window is not decoration: the SAME
/// store, the SAME query words, a window one month further on -- and the answer
/// is empty rather than "the closest thing I found".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_window_that_holds_nothing_answers_nothing() {
    let mock = MockOpenAI::start(vec![
        canned_tool_calls(vec![(
            "call-m2",
            "memory_recall",
            r#"{"query":"radiator valve","window_from":"2026-04-01T00:00:00.000Z","window_to":"2026-04-30T23:59:59.000Z"}"#,
        )]),
        canned_chat_completion("Nothing in April, no.", "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_root(&td, &mock.base_url);
    let h = boot(&td).await;
    grow(&h).await;

    let db = td.path().join("main/memory/episodes/cell.db");
    let history = shipped_past();
    for t in &history {
        h.send(past(t)).await;
    }
    await_rows(&db, EPISODES, history.len());

    let (probe_tx, mut probe_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/probe"), move || {
        CaptureCell::new(probe_tx.clone())
    })
    .await;
    h.add_edge(
        meclaw_core::Uuid::now_v7(),
        Path::new("/talky/collector/assemble"),
        Path::new("/probe"),
    )
    .await;
    h.send(turn("chat-1", "Anything about the radiator in April?"))
        .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!left.is_zero(), "no answer reached the reply port");
        let Ok(Some(m)) = tokio::time::timeout(left, probe_rx.recv()).await else {
            panic!("the reply port went quiet");
        };
        if texts(&m).iter().any(|s| s == "Nothing in April, no.") {
            break;
        }
    }

    let requests = mock.recorded_requests().await;
    assert_tool_was_offered(&requests[0], "memory_recall");
    let prompt = meclaw_core::serde_json::to_string(
        requests[1]
            .messages()
            .expect("the second prompt has messages"),
    )
    .expect("serialise");
    assert!(
        !prompt.contains(FEB_MARK),
        "a February row answered an April question: {prompt}"
    );
    assert!(
        prompt.contains("nothing was said in this window"),
        "an empty window has to SAY it is empty, or the model guesses: {prompt}"
    );

    h.shutdown().await;
}
