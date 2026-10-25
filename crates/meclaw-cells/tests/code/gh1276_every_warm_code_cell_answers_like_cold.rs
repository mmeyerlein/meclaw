//! GH #1276 -- every warm `code` cell answers like cold, proved from data.
//!
//! Warm is the default runner mode of a code cell since GH #1276; a cell stays
//! cold only with a written reason (`templates/RUNNER-MODES.md`). For each
//! cell that became warm here -- and for every warm cell to come -- a fixture
//! under `fixtures/warm/<cell-with-dashes>.json` holds the messages
//! (`{"overlay"?, "probes": [{hop?, context?, body}, ...]}`, `<base>` in the
//! overlay and the probes is a directory of each mode's own), and one test per cell runs them
//! through the engine of GH #1197 (`support/warm_equivalence.rs`): spawned on
//! the production path once warm on ONE child and once cold, the messages one
//! after the other, the masked answers equal. A fixture's second message takes
//! another branch than the first, so a namespace that leaked from message 1
//! would show in message 2.
//!
//! The fixtures come, in this order, from lab colonies with simulated turns
//! (`scripts/capture_code_inputs.py`, ids and free text replaced by
//! placeholders), from the inputs of the template's own tests, or by hand from
//! the template's contract. They never come from a colony of a real person.
//!
//! The completeness lock reads the modes from the shipped `config.json`s, not
//! from the inventory: every warm code cell has its fixture and its test,
//! except the 25 that were warm before (`WARM_BEFORE_1276`, a list that never
//! grows -- a new warm cell brings its own fixture).

// harness_fold: #[path = "support/warm_equivalence.rs"]
#[allow(unused_imports)]
use crate::warm_equivalence; // harness_fold: mod warm_equivalence;

use meclaw_core::serde_json::{self, Value};
use std::collections::BTreeSet;
use warm_equivalence::{Probe, mask, templates, warm_answers_like_cold_at};

/// The code cells that ran warm in 0.69.0, before GH #1276. Seven of them carry
/// their probes in `gh1197_the_warm_code_cells_answer_like_cold`; this list
/// is history and never grows.
const WARM_BEFORE_1276: [&str; 25] = [
    "affinity/brief",
    "affinity/gate",
    "canvy/layout",
    "cogny/splitter",
    "colony-view/layout",
    "daily-digest/void",
    "file-space/guard",
    "file-space/ingest",
    "file-space/read",
    "file-space/tools",
    "file-space/write",
    "file-space/ws",
    "graph-space/index",
    "graph-space/query",
    "memory-hive/recall",
    "npc/splitter",
    "objects/brief",
    "objects/gate",
    "objects/push",
    "objects/source",
    "objects/tools",
    "projection/mat",
    "talky/route",
    "talky/splitter",
    "terminal",
];

fn fixtures() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/code/fixtures/warm")
}

fn fixture_file(cell: &str) -> std::path::PathBuf {
    fixtures().join(format!("{}.json", cell.replace('/', "-")))
}

/// `v` with every `<base>` in a string replaced by `base`.
fn with_base(v: &Value, base: &str) -> Value {
    match v {
        Value::String(s) => Value::String(s.replace("<base>", base)),
        Value::Array(a) => Value::Array(a.iter().map(|x| with_base(x, base)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, x)| (k.clone(), with_base(x, base)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The acceptance of GH #1276 for one cell: its fixture, warm against cold.
async fn answers_like_cold(cell: &str) {
    // A private template does not travel with the public tree (nor does its
    // fixture): there the cell is not shipped, and there is nothing to prove.
    if !shipped_here(cell) {
        return;
    }
    let path = fixture_file(cell);
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    let fx: Value = serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    let probes: Vec<Value> = fx["probes"]
        .as_array()
        .unwrap_or_else(|| panic!("{path:?}: `probes` is a list"))
        .clone();
    let probes_at = |base: &str| -> Vec<Probe> {
        probes
            .iter()
            .map(|p| with_base(p, base))
            .map(|p| Probe {
                hop: p
                    .get("hop")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({})),
                context: p
                    .get("context")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({})),
                body: p
                    .get("body")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({})),
            })
            .collect()
    };
    let overlay = fx.get("overlay").cloned().unwrap_or(serde_json::json!({}));
    warm_answers_like_cold_at(cell, |base| with_base(&overlay, base), probes_at).await;
}

/// Whether `cell` is a template cell of this tree.
fn shipped_here(cell: &str) -> bool {
    templates().join(cell).join("config.json").is_file()
}

/// Every `code` cell below `templates/` with its runner mode (absent reads
/// `cold`, the parser's default), as `(cell, mode)`.
fn shipped_modes() -> Vec<(String, String)> {
    fn walk(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<(String, String)>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .expect("a template dir")
            .map(|e| e.expect("a dir entry").path())
            .collect();
        entries.sort();
        for p in entries {
            if !p.is_dir() {
                continue;
            }
            let cfg = p.join("config.json");
            if cfg.is_file() {
                let raw = std::fs::read_to_string(&cfg).expect("config.json");
                let v: Value =
                    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{cfg:?}: {e}"));
                if v["cell"]["type"] == "code" {
                    let cell = p.strip_prefix(root).expect("below templates/");
                    let mode = v["params"]["runner_mode"].as_str().unwrap_or("cold");
                    out.push((cell.to_string_lossy().into_owned(), mode.to_string()));
                }
            }
            walk(&p, root, out);
        }
    }
    let root = templates();
    let mut out = Vec::new();
    walk(&root, &root, &mut out);
    out
}

macro_rules! warm_cells {
    ($($test:ident: $cell:literal),* $(,)?) => {
        /// The cells with a fixture, one test each (nextest spreads them, and
        /// `slow-tests` sees the single cell).
        const FIXTURED: &[&str] = &[$($cell),*];
        $(
            #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
            async fn $test() {
                answers_like_cold($cell).await;
            }
        )*
    };
}

/// The lock: the warm code cells of the shipped configs, minus the history
/// list, are exactly the cells with a fixture and a test -- and every fixture
/// file belongs to one of them.
#[test]
fn every_warm_code_cell_has_its_fixture() {
    let warm: BTreeSet<String> = shipped_modes()
        .into_iter()
        .filter(|(_, mode)| mode == "warm")
        .map(|(cell, _)| cell)
        .filter(|cell| !WARM_BEFORE_1276.contains(&cell.as_str()))
        .collect();
    let fixtured: BTreeSet<String> = FIXTURED
        .iter()
        .filter(|c| shipped_here(c))
        .map(|c| c.to_string())
        .collect();
    assert_eq!(
        warm, fixtured,
        "every warm code cell (except WARM_BEFORE_1276) has its fixture and its line in warm_cells!"
    );
    for cell in FIXTURED.iter().filter(|c| shipped_here(c)) {
        assert!(
            fixture_file(cell).is_file(),
            "{cell}: its fixture {:?}",
            fixture_file(cell)
        );
    }
    let files: BTreeSet<String> = std::fs::read_dir(fixtures())
        .expect("the fixture dir")
        .map(|e| {
            e.expect("a dir entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let named: BTreeSet<String> = FIXTURED
        .iter()
        .map(|c| format!("{}.json", c.replace('/', "-")))
        .collect();
    let orphans: Vec<_> = files.difference(&named).collect();
    assert!(
        orphans.is_empty(),
        "no fixture without its cell: {orphans:?}"
    );
}

/// The mask takes exactly the fresh shapes: a `uuid4` goes, and so does the
/// `uuid4().hex[:8]` suffix of a minted id and a stamp in seconds; a counter,
/// a short id or a word with dashes stays -- so a leak that writes a count or
/// a name still shows.
#[test]
fn the_mask_takes_the_fresh_shapes_and_nothing_else() {
    assert_eq!(
        mask(
            r#"{"id":"9f1c2a4e-3b7d-4c1e-8a2f-0d9e8c7b6a51","t":"archive-1a2b3c4d","s":1761040000,"n":"2","w":"in-tool-17"}"#
        ),
        r#"{"id":"<uuid>","t":"archive-<hex>","s":<hex>,"n":"2","w":"in-tool-17"}"#
    );
}

warm_cells! {
    access_invoke: "access/invoke",
    affinity_porter: "affinity/porter",
    affinity_push: "affinity/push",
    archive_bridge: "archive-bridge",
    chat_channel: "chat-channel",
    coder_pipeline_coderprep: "coder-pipeline/coderprep",
    coder_pipeline_collector: "coder-pipeline/collector",
    coder_pipeline_dispatch: "coder-pipeline/dispatch",
    coder_pipeline_plannerprep: "coder-pipeline/plannerprep",
    coder_pipeline_revout: "coder-pipeline/revout",
    coder_pipeline_revprep: "coder-pipeline/revprep",
    coder_pipeline_taskarchive: "coder-pipeline/taskarchive",
    cogny_ask: "cogny/ask",
    collector_assemble: "collector/assemble",
    curator_reader: "curator/reader",
    curator_writer: "curator/writer",
    dispatcher: "dispatcher",
    door: "door",
    egon_persona: "egon/persona",
    figure_phone: "figure/phone",
    file_space_derive: "file-space/derive",
    file_space_embed: "file-space/embed",
    file_space_extract: "file-space/extract",
    firewall_porter: "firewall/porter",
    firewall_screen: "firewall/screen",
    firewall_warden: "firewall/warden",
    freeswitch_dial: "freeswitch/dial",
    freeswitch_signal: "freeswitch/signal",
    librarian_index: "librarian/index",
    librarian_query: "librarian/query",
    librarian_tools: "librarian/tools",
    memory_drain_drain: "memory-drain/drain",
    memory_hive_close_glue: "memory-hive/close-glue",
    memory_hive_embed: "memory-hive/embed",
    memory_hive_entity_glue: "memory-hive/entity-glue",
    memory_hive_extract_glue: "memory-hive/extract-glue",
    memory_hive_porter: "memory-hive/porter",
    memory_hive_tool: "memory-hive/tool",
    memory_hive_writer: "memory-hive/writer",
    projection_git: "projection/git",
    projection_run: "projection/run",
    receptionist_greet: "receptionist/greet",
    research_assistant_archive: "research-assistant/archive",
    research_assistant_collector: "research-assistant/collector",
    research_assistant_dispatch: "research-assistant/dispatch",
    research_assistant_prep: "research-assistant/prep",
    retry: "retry",
    session_keeper_close: "session-keeper/close",
    session_keeper_porter: "session-keeper/porter",
    session_keeper_stamp: "session-keeper/stamp",
    simple_agent_1_splitter: "simple-agent@1.1.4/splitter",
    simple_agent_splitter: "simple-agent/splitter",
    simple_agent_tool_gate: "simple-agent/tool_gate",
    slack_agent_persona: "slack-agent/persona",
    summarizer_prep: "summarizer/prep",
    tools_build_apply: "tools/build-apply",
    tools_build_draft: "tools/build-draft",
    tools_unknown: "tools/unknown",
    tools_weather: "tools/weather",
}
