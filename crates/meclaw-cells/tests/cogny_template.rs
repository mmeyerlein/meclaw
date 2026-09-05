//! meclaw-os -- the shipped `cogny` template, the agent core (GH #28, R-CG-2).
//!
//! [`talky_cogny_advisor.rs`] proved the advisor CONNECTION with the core wired
//! by hand, because R-CG-2 deliberately shipped no template in wave 5. This file
//! is the other half: the same core, now as `templates/cogny/`, read out
//! of the shipped tree and driven end to end.
//!
//! What is pinned here is exactly what a template has to carry:
//!
//! 1. **The sub-units are named, not copied (GH #277).** `collector` and
//!    `dispatcher` sit here as `cell.type: "ref"` markers with an explicitly
//!    pinned version; editing the standalone template IS the sync.
//! 2. **The core is a tool loop.** A consult errand arrives on the documented
//!    ingress, the brain asks for a tool, the core runs its OWN round, and the
//!    advice leaves on the `answer` lane. Not one internal edge is wired by this
//!    test: the parent draws the ports and one tool lane, the template draws
//!    every edge inside the composite.
//! 3. **The correlation survives.** `consult_id` is promoted to context on the
//!    ingress edge and is still on the message that comes home -- which is the
//!    only reason a talky can tell one consultation from another.
//! 4. **One brain, and the core declares its own errand (4.4.0, GH #528).**
//!    The seam had two lanes until 4.4.0 and the class was a second tool name;
//!    both are gone, because a fast memory question belongs to the surface that
//!    already holds the window. Pinned three ways: the composite is
//!    `collector` + `dispatcher` + `brain` + `schemas` and nothing else,
//!    `ask_memory` / `escalate_to_deep` / `brain_fast` survive in no config of
//!    the template, and an `in_schemas` request comes back on `tool_schemas`
//!    carrying the `consult_cogny` schema with `question` and `context` both
//!    required. The last one is the whole of "whoever is reached declares
//!    themselves": with no owner for that schema a grown caller offers its
//!    model a menu without the one tool the core exists for.
//!
//! Free of a real provider by construction: the brain is the SHIPPED `llm` cell
//! pointed at the mock OpenAI wire, the tool is a `code` cell.
//!
//! **R2b guard (GH #49 form).** A template travels only when `PUBLIC_TEMPLATES`
//! in the export script names it -- `cogny` does since 2026-08-15, but the
//! guard is the mechanism, not the current answer. Every read below is guarded
//! per file by [`shipped_cogny`]; where the template does not ship, these tests
//! skip instead of failing on a dead `templates/` reference. That is what keeps
//! this file honest on both sides of the export.
//!
//! The byte-identity pin over the two sub-unit copies retired with the copies
//! themselves (GH #277): `cogny` references its sub-units now, so there is
//! nothing left to drift. Its successors live in
//! `meclaw-colony/tests/gh277_composite_instantiation_is_byte_identical.rs` --
//! `cogny_instantiates_the_same_tree_as_its_golden_manifest` proves the
//! resolved tree is the byte tree the copies produced, and
//! `a_cell_inside_talky_is_stamped_with_its_own_template_and_names_talky_above_it`
//! proves the other half of the same mechanism: a cell that came through a ref
//! records which template it really is an instance of.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_openai::{MockOpenAI, canned_chat_completion, canned_tool_calls};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

// ───────────────────────────────────────────────────────────── the shipped tree

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// Every file the composite is made of. The list is the guard AND the
/// inventory: a cell that silently disappears from the template makes these
/// tests skip rather than pass.
///
/// Since GH #277 `collector/config.json` and `dispatcher/config.json` are `ref`
/// markers, not cells -- the sub-units' own trees live in `templates/collector/`
/// and `templates/dispatcher/` and are pulled in at instantiation time. That is
/// why `collector/assemble` and `collector/window` are no longer listed here:
/// they are not files of THIS template any more.
const COGNY_FILES: &[&str] = &[
    "config.json",
    "brain/config.json",
    "collector/config.json",
    "schemas/config.json",
    "dispatcher/config.json",
];

/// The composite ships no seed at all since 4.4.0 (GH #528). The one it used to
/// ship was `brain_fast/seed/system.jsonl` -- the lookup lane's length
/// discipline -- and it went with the lane. Identity, instructions and the tool
/// menu are instance business, and the menu is asked for rather than seeded
/// since 4.3.0.
const COGNY_SEEDS: &[&str] = &[];

/// The template root, or `None` where it does not ship.
///
/// Guarded per file (the documented R2b exception form, GH #49): a private
/// template is absent in the public clone, and a test that reads it must skip
/// there instead of turning the export red. Nothing else in this file touches
/// `templates/`.
fn shipped_cogny() -> Option<std::path::PathBuf> {
    let root = templates_root().join("cogny");
    for rel in COGNY_FILES.iter().chain(COGNY_SEEDS) {
        if !root.join(rel).exists() {
            return None;
        }
    }
    Some(root)
}

/// The shipped template, copied cell by cell: `config.json` files and the
/// seeds next to them travel, which is exactly what instantiation copies (a
/// recursive directory copy, `docs/meclaw-overview.md` § Instanziierungs-Flow).
/// So the tree under test IS the template and nothing else.
fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    let src = &resolve_template_ref(src);
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let name = entry.file_name();
        if from.is_dir() {
            copy_cells(&from, &dst.join(name));
        } else if name == "config.json"
            || src.file_name().is_some_and(|d| d == "seed")
                && std::path::Path::new(&name)
                    .extension()
                    .is_some_and(|e| e == "jsonl")
        {
            std::fs::copy(&from, dst.join(name)).unwrap();
        }
    }
}

/// GH #277: a directory whose `config.json` declares `cell.type: "ref"` is a
/// REFERENCE, not a cell -- the referenced template's tree belongs in its
/// place. `cogny` names its two sub-units that way, so a tree copied straight
/// off the library follows the same hop the substrate's staging path follows.
fn resolve_template_ref(dir: &std::path::Path) -> std::path::PathBuf {
    let mut dir = dir.to_path_buf();
    for _ in 0..8 {
        let Ok(raw) = std::fs::read_to_string(dir.join("config.json")) else {
            return dir;
        };
        let Ok(v) = meclaw_core::serde_json::from_str::<Value>(&raw) else {
            return dir;
        };
        if v["cell"]["type"] != "ref" {
            return dir;
        }
        let reference = v["cell"]["template"]
            .as_str()
            .expect("a ref cell names a template");
        let name = reference.split('@').next().unwrap_or_default();
        dir = templates_root().join(name);
    }
    panic!("template ref chain does not terminate at {}", dir.display());
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

fn patch(root: &std::path::Path, rel: &str, f: impl FnOnce(&mut Value)) {
    let p = root.join(rel);
    let mut v: Value = meclaw_core::serde_json::from_str(&std::fs::read_to_string(&p).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    f(&mut v);
    std::fs::write(&p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

/// The correlation of the one consultation this file runs.
const CONSULT_ID: &str = "k-9";

/// The one brain's model id on the wire -- the one thing a caller cannot fake
/// and the substrate cannot mix up.
const DEEP_MODEL: &str = "gpt-4o-mock-deep";

/// The session the errand belongs to. `in_turn` DEMANDS it in context since
/// 4.4.0 (GH #528): a core whose memory tool asks about sessions has to be able
/// to say which one.
const SESSION_ID: &str = "s-1";

// ────────────────────────────────────────────────────────── the test-only cells

fn code_cell(script: &str, routes: &[&str], extra_hop: Value) -> Value {
    let mut hop = json!({});
    if !routes.is_empty() {
        hop["route"] = json!({"type": "string", "values": routes, "required": false});
    }
    if let Some(extra) = extra_hop.as_object() {
        for (k, v) in extra {
            hop[k] = v.clone();
        }
    }
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 10000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": hop
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in around the shipped cogny template.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// The asking side, reduced to what the core actually sees: a talky's
/// dispatcher emits a consult call as a `tool_call` turn whose text is the raw
/// arguments, plus the correlation AND the tool name on the hop. Nothing of the
/// talky's own machinery matters here -- that half is pinned in
/// `talky_cogny_advisor.rs`.
///
/// There is ONE errand name since 4.4.0 (GH #528). `ask_memory` picked the
/// lookup lane and both are gone, so the stand-in emits `consult_cogny` and
/// nothing else.
///
/// It also emits `session_id` on the HOP, which a real talky does not: over
/// there the session keeper puts the key in CONTEXT on the first edge of every
/// turn, and the documented ingress edge re-states it from there
/// (`"session_id": "context.session_id"`). This stand-in has no keeper and no
/// turn chain in front of it, so the same requirement is met from the hop --
/// same key, same lane contract, one cell fewer in a test about the core.
const ASKER: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
args = str(msgs[-1].get("text", "")) if msgs else "{}"
sys.stdout.write(json.dumps({"header": {"route": "consult", "consult_id": "k-9",
                                        "session_id": "s-1",
                                        "tool_name": "consult_cogny"},
                             "messages": [{"origin": "assistant", "type": "tool_call",
                                           "id": "k-9", "text": args}]}))
"#;

/// The declaration asker (GH #528): a cell that wants to know what this core's
/// errand looks like, asking exactly the way a collector's menu tick asks a
/// tools hive.
const MENU_ASKER: &str = r#"
import sys, json
sys.stdout.write(json.dumps({"header": {"route": "ask"},
                             "tools": ["*"], "messages": []}))
"#;

/// The core's own tool -- an ordinary, synchronous one, wired on the one lane
/// that is genuinely per-instance.
const LOOKUP: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
c = msgs[0] if msgs else {}
try:
    a = json.loads(c.get("text") or "{}")
except Exception:
    a = {}
sys.stdout.write(json.dumps({"header": {"route": "res"},
                             "messages": [{"origin": "tool", "type": "tool_result",
                                           "id": c.get("id", ""),
                                           "text": "21C and sunny (%s)" % str(a.get("q", ""))}]}))
"#;

// ─────────────────────────────────────────────────────────────── the topology

/// The ports around the composite -- all four of them, and every one is a
/// literal copy of what `templates/cogny/README.md` documents. The
/// template itself draws no edge that appears here.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        // ── port 1: the errand enters, exactly as the talky's dispatcher sends it ──
        // `consult_id` becomes context because the hop decays at the next cell,
        // `session_id` because the `in_turn` lane declares it (GH #528), and
        // `col_phase` is cleared because this message comes out of ANOTHER
        // collector's chain and would otherwise arrive mid-assembly.
        //
        // ONE edge since 4.4.0. The second one carried `ask_memory` and set
        // `context.consult_class`; the class, the lane and the name are gone.
        {"from": "./asker", "to": "./cogny/collector",
         "condition": "has(hop.route) && hop.route == 'consult' \
                       && has(hop.tool_name) && hop.tool_name == 'consult_cogny'",
         "modifier": {"set_hop": {"route": "'in_turn'"},
                      "set_context": {"consult_id": "hop.consult_id",
                                      "session_id": "hop.session_id", "col_phase": "''"},
                      "restore_ttl": true}},
        // ── port 2: the advice goes home on the return lane ──
        {"from": "./cogny/collector", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'answer'",
         "modifier": {"set_hop": {"route": "'in_advice'"},
                      "set_context": {"col_phase": "''"},
                      "restore_ttl": true}},
        // ── port 3+4: the declaration pair (GH #528). It enters at the HIVE
        //    PATH, not at a cell: proving the door is half of what the pin is
        //    for, because the composite is sealed and `./schemas` is not an
        //    address a caller may name.
        {"from": "./menu-asker", "to": "./cogny",
         "condition": "has(hop.route) && hop.route == 'ask'",
         "modifier": {"set_hop": {"route": "'in_schemas'"}}},
        {"from": "./cogny", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'tool_schemas'"},
        // ── the one per-instance lane: which cell answers to `lookup` ──
        {"from": "./cogny/dispatcher", "to": "./lookup",
         "condition": "has(hop.tool_name) && hop.tool_name == 'lookup'"},
        {"from": "./lookup", "to": "./cogny/collector",
         "condition": "has(hop.route) && hop.route == 'res'",
         "modifier": {"set_hop": {"route": "'in_tool'"}}}
    ]}}})
}

fn build_tree(td: &tempfile::TempDir, root_template: &std::path::Path, base_url: &str) {
    let root = td.path();
    // No handoff tool of the core's own since 4.4.0 (GH #528): `escalate_to_deep`
    // was the only one and it went with the lookup lane. `consult_cogny` is a
    // handoff on the ASKING side, which is not this tree.
    std::fs::write(root.join(".env"), "OPENROUTER_API_KEY=test-key\n").unwrap();
    write(root, "main/config.json", &main_config());
    write(root, "main/asker/config.json", &{
        code_cell(
            ASKER,
            &["consult"],
            json!({"consult_id": {"type": "string", "required": false},
                   "session_id": {"type": "string", "required": false},
                   "tool_name": {"type": "string", "required": false}}),
        )
    });
    write(
        root,
        "main/menu-asker/config.json",
        &code_cell(MENU_ASKER, &["ask"], json!({})),
    );
    write(
        root,
        "main/lookup/config.json",
        &code_cell(LOOKUP, &["res"], json!({})),
    );
    copy_cells(root_template, &root.join("main/cogny"));
    // `${ctx.model}` is an INSTANTIATION-side substitution; a raw copy has to be
    // told which wire to talk to, and here that wire is the mock.
    patch(root, "main/cogny/brain/config.json", |v| {
        v["params"]["base_url"] = json!(base_url);
        v["params"]["model"] = json!(DEEP_MODEL);
    });
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
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
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    (h, sink_rx)
}

fn consult(arguments: &str) -> Message {
    MessageBuilder::new(Path::new("/asker"))
        .body(Body::Inline(
            json!({"messages": [{"origin": "assistant", "type": "text", "text": arguments}]}),
        ))
        .ttl(400)
        .build()
}

fn body_of(m: &Message) -> &Value {
    match &m.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("inline expected"),
    }
}

fn hop_of(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn ctx_of(m: &Message, key: &str) -> String {
    m.headers
        .context
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn answer_text(m: &Message) -> String {
    body_of(m)["messages"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

/// The whole wire conversation of one provider call, as one string.
fn wire_of(req: &mock_openai::OpenAiRequestSnapshot) -> String {
    meclaw_core::serde_json::to_string(req.messages().expect("wire messages")).unwrap_or_default()
}

/// Which lane made this call. The model id is the one thing the two `llm`
/// cells cannot share.
fn lane_of(req: &mock_openai::OpenAiRequestSnapshot) -> &str {
    req.model().unwrap_or("<no model>")
}

// ═══════════════════════════════════════════════════════════════════════ pins

fn collect_configs(
    root: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<std::path::PathBuf>,
) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let p = entry.path();
        if p.is_dir() {
            collect_configs(root, &p, out);
        } else if entry.file_name() == "config.json" {
            out.push(p.strip_prefix(root).unwrap().to_path_buf());
        }
    }
}

/// R-CG-2 named three units and no keeper, no summarizer, no proxy. A template
/// that grew one of them would still pass the round below -- it would just stop
/// being the agent core. So the inventory is pinned as a set, not as a floor.
#[test]
fn the_core_carries_the_tool_loop_and_nothing_else() {
    let Some(cogny) = shipped_cogny() else {
        return;
    };
    let mut found = Vec::new();
    collect_configs(&cogny, &cogny, &mut found);
    let mut found: Vec<String> = found
        .iter()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    found.sort();
    let mut want: Vec<String> = COGNY_FILES.iter().map(|s| s.to_string()).collect();
    want.sort();
    assert_eq!(
        found, want,
        "cogny is collector + dispatcher + brain (R-CG-2): no keeper, no summarizer, \
         no proxy -- the core has no channel, no sessions and no night"
    );
}

/// The whole claim of `cogny` in one consultation: the errand enters on the
/// documented ingress, the core runs its OWN tool round, and the advice leaves
/// on the return lane under the `consult_id` it was handed. Three edges came
/// from this test; the six that made the round came from the template.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_consult_runs_the_cores_own_tool_round_and_answers_on_the_return_lane() {
    let Some(cogny) = shipped_cogny() else {
        return;
    };
    let mock = MockOpenAI::start(vec![
        canned_tool_calls(vec![("c1", "lookup", r#"{"q":"weather in berlin"}"#)]),
        canned_chat_completion("it is 21C and sunny in berlin.", "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &cogny, &mock.base_url);
    let (h, mut sink_rx) = boot(&td).await;

    h.send(consult(r#"{"question":"weather in berlin"}"#)).await;

    // 1. The advice comes home -- on the return lane, with its correlation.
    let advice = recv_bounded(&mut sink_rx).await.expect("the advice");
    assert_eq!(answer_text(&advice), "it is 21C and sunny in berlin.");
    assert_eq!(
        hop_of(&advice, "route"),
        "in_advice",
        "the return lane is the port edge's business, and it is the talky's fan-in: {:?}",
        advice.headers.hop
    );
    assert_eq!(
        ctx_of(&advice, "consult_id"),
        CONSULT_ID,
        "the correlation survived the core's whole chain: {:?}",
        advice.headers.context
    );
    assert_eq!(
        hop_of(&advice, "iter"),
        "1",
        "and it took a tool round to get there -- the seam re-entered once: {:?}",
        advice.headers.hop
    );

    // 2. Two inferences, and the second one saw the core's own tool result.
    let reqs = mock.recorded_requests().await;
    assert_eq!(
        reqs.len(),
        2,
        "one to ask for the tool, one to answer with it: {} calls",
        reqs.len()
    );
    let first = wire_of(&reqs[0]);
    assert!(
        first.contains("weather in berlin"),
        "the errand was filed as the turn -- the talky IS the core's user: {first}"
    );
    let second = wire_of(&reqs[1]);
    assert!(
        second.contains("21C and sunny (weather in berlin)"),
        "the core ran its OWN tool round and the result re-entered the seam: {second}"
    );

    // 3. Both calls were the one brain. There is no second lane to reach since
    //    4.4.0, and the model id is what proves the seam did not grow one.
    for (i, r) in reqs.iter().enumerate() {
        assert_eq!(
            lane_of(r),
            DEEP_MODEL,
            "call {i} of a consult errand belongs on the core's one brain"
        );
    }

    // 4. And the session came with it. `in_turn` DEMANDS `session_id` in
    //    context (GH #528) because the core's memory tool asks about sessions,
    //    and it has to survive the whole chain the same way `consult_id` does.
    assert_eq!(
        ctx_of(&advice, "session_id"),
        SESSION_ID,
        "the errand's session survived the core's chain: {:?}",
        advice.headers.context
    );

    h.shutdown().await;
}

/// The core declares its OWN errand (GH #528). An `in_schemas` request enters
/// at the HIVE PATH -- `./schemas` is not an address a caller may name, the
/// composite is sealed -- and the answer comes back on `tool_schemas` in the
/// shape a tools hive answers in, so the asking collector can cut two menus
/// together without knowing which answerer produced which half.
///
/// The whole point of the pin is the OWNERSHIP. Until 4.4.0 the `consult_cogny`
/// schema was typed by hand into every calling brain's `system.tools`; GH #464
/// replaced typed menus with asked-for ones, every caller stopped typing, and
/// the schema had no owner left at all -- so a grown assistant offered its model
/// a menu without the one tool the core exists for.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_core_answers_in_schemas_with_its_own_errand() {
    let Some(cogny) = shipped_cogny() else {
        return;
    };
    let mock = MockOpenAI::start(vec![]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &cogny, &mock.base_url);
    let (h, mut sink_rx) = boot(&td).await;

    h.send(
        MessageBuilder::new(Path::new("/menu-asker"))
            .body(Body::Inline(json!({"messages": []})))
            .ttl(64)
            .build(),
    )
    .await;

    let answer = recv_bounded(&mut sink_rx).await.expect("the declarations");
    assert_eq!(
        hop_of(&answer, "route"),
        "tool_schemas",
        "the declaration lane is the tools hive's own: {:?}",
        answer.headers.hop
    );
    assert_eq!(
        hop_of(&answer, "operation"),
        "schemas",
        "and so is the operation key: {:?}",
        answer.headers.hop
    );

    let schemas = body_of(&answer)["schemas"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        schemas.len(),
        1,
        "this core serves exactly one errand: {schemas:?}"
    );
    let one = &schemas[0];
    assert_eq!(
        one["name"].as_str(),
        Some("consult_cogny"),
        "and it is named the way the ingress edge conditions on it: {one:?}"
    );

    // `question` AND `context`, both required. The second one is the ruling:
    // the asker sends what it knows in full and does not deduplicate it against
    // what it believes the core already has, because the core's curator
    // discards what it does not need and cannot recover what was never sent.
    let required: Vec<String> = one["parameters"]["required"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    assert_eq!(
        required,
        vec!["question".to_string(), "context".to_string()],
        "both are required, and `context` is the half a caller would otherwise \
         filter: {one:?}"
    );
    for optional in ["eta", "consult_id"] {
        assert!(
            one["parameters"]["properties"][optional].is_object(),
            "{optional} is declared and optional: {one:?}"
        );
    }
    // The description is the class boundary and the one place it lives.
    let desc = one["description"]
        .as_str()
        .unwrap_or_default()
        .to_lowercase();
    assert!(
        desc.contains("do not"),
        "the description says what NOT to send here -- that sentence IS the \
         class boundary: {desc}"
    );

    // Provider-neutral: `{name, description, parameters}` and no envelope. The
    // caller is the one that knows its provider, so wrapping is its job.
    assert!(
        one.get("type").is_none() && one.get("function").is_none(),
        "the answer carries no provider envelope: {one:?}"
    );

    h.shutdown().await;
}

/// The lookup lane is gone, and gone means gone from the MECHANISM (GH #528).
///
/// A prose retraction is required and is not enough, so this pin reads neither
/// prose nor the absence of a word: the README and the `because` sentences keep
/// naming `ask_memory`, `escalate_to_deep` and `brain_fast` on purpose, because
/// a promise is retired by an explicit retraction rather than a silent rewrite
/// (`docs/development-rules.md` § 3). What must not survive is a MECHANISM -- a
/// cell directory, an edge, a ctx key -- so every `because` and `description`
/// is stripped out first and the assertion runs on what is left.
#[test]
fn no_lookup_lane_survives_in_the_shipped_mechanism() {
    let Some(cogny) = shipped_cogny() else {
        return;
    };
    assert!(
        !cogny.join("brain_fast").exists(),
        "the fast lane's cell is gone, with its `brevity` seed"
    );

    let mut files = Vec::new();
    collect_configs(&cogny, &cogny, &mut files);
    for rel in &files {
        let mut v: Value =
            meclaw_core::serde_json::from_str(&std::fs::read_to_string(cogny.join(rel)).unwrap())
                .unwrap();
        strip_prose(&mut v);
        let raw = meclaw_core::serde_json::to_string(&v).unwrap();
        for dead in ["ask_memory", "escalate_to_deep", "brain_fast", "model_fast"] {
            assert!(
                !raw.contains(dead),
                "{} still WIRES `{dead}`: one class, one lane, one brain since \
                 4.4.0. A mechanism that outlives its lane is the defect GH #528 \
                 removed -- a word in a retraction is not",
                rel.display()
            );
        }
    }

    // The ctx surface is the other half of the same removal, and it is read as
    // a key rather than as a string: `model_fast` fed `./brain_fast` and there
    // is nothing left to feed.
    let manifest: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(cogny.join("template.json")).unwrap(),
    )
    .unwrap();
    let ctx = &manifest["requires"]["ctx"];
    assert!(
        ctx["model"].is_object() && ctx.get("model_fast").is_none(),
        "one brain takes one model key: {ctx:?}"
    );

    // And the knobs the ruling moved, read off the ref marker rather than off
    // prose: the ambient leg is not this core's, and the memory TOOL is not this
    // core's to switch either.
    let collector: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(cogny.join("collector/config.json")).unwrap(),
    )
    .unwrap();
    assert!(
        collector["override_params"]["assemble"]
            .get("memory_tier")
            .is_none(),
        "the ambient leg is NOT switched on here -- a problem solver asks \
         on purpose: {collector:?}"
    );
    // GH #552: `memory_call_tier` stood here at "1" while this composite served
    // the call itself. It does not any more -- the member's memory hive declares
    // the name and answers it -- and this core reaches it the ordinary way: it
    // declares `["*"]`, so whatever answerers the level wires are asked, the
    // memory among them.
    assert!(
        collector["override_params"]["assemble"]
            .get("memory_call_tier")
            .is_none(),
        "a knob the collector does not have any more: {collector:?}"
    );
    assert_eq!(
        collector["override_params"]["assemble"]["tools"],
        meclaw_core::serde_json::json!(["*"]),
        "and the declared list is what reaches the memory's `schemas` cell: {collector:?}"
    );
}

/// Drop every `because` and `description` subtree, recursively. What is left is
/// the part of a template file a message actually travels through: cells,
/// params, conditions, modifiers, lane names.
fn strip_prose(v: &mut Value) {
    match v {
        Value::Object(map) => {
            map.retain(|k, _| k != "because" && k != "description");
            for (_, child) in map.iter_mut() {
                strip_prose(child);
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                strip_prose(item);
            }
        }
        _ => {}
    }
}
