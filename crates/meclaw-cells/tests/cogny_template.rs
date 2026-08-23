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
//! 4. **The seam has two lanes (1.1.0, GH #124).** The same assembled errand
//!    reaches `./brain` or `./brain_fast`, decided by `context.consult_class`,
//!    which the ingress edge lifts from the tool name the asking model chose.
//!    Pinned twice: the lookup lane answers on the fast model under its own
//!    length cap, and a fast lane that says "not enough" escalates back into
//!    the seam so the DEEP lane answers instead. A misclassification costs one
//!    extra recall, never a wrong answer.
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
    "brain_fast/config.json",
    "collector/config.json",
    "dispatcher/config.json",
];

/// The one non-`config.json` file the composite ships (GH #124). The length
/// discipline of the lookup lane is the only piece of SYSTEM state this
/// template owns -- identity, instructions and tool schemas stay instance
/// business, and `brevity` is a slot of its own precisely so the instance's
/// `instructions` write never collides with it (one writer per system path).
const COGNY_SEEDS: &[&str] = &["brain_fast/seed/system.jsonl"];

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

/// The two lanes are told apart on the wire by the model id alone -- the one
/// thing a caller cannot fake and the substrate cannot mix up.
const DEEP_MODEL: &str = "gpt-4o-mock-deep";
const FAST_MODEL: &str = "gpt-4o-mock-fast";

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
/// The tool name is the whole class hook of GH #124: the asking model declares
/// `consult_cogny` or `ask_memory`, and the ingress edge turns that choice into
/// `context.consult_class`. A closed value set the model names beats a duration
/// estimate nobody measured, which is why `hop.consult_eta` stays observe-only.
const ASKER: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
args = str(msgs[-1].get("text", "")) if msgs else "{}"
try:
    a = json.loads(args)
except Exception:
    a = {}
tool = str((a or {}).get("tool") or "consult_cogny")
sys.stdout.write(json.dumps({"header": {"route": "consult", "consult_id": "k-9",
                                        "tool_name": tool},
                             "messages": [{"origin": "assistant", "type": "tool_call",
                                           "id": "k-9", "text": args}]}))
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
        // and `col_phase` is cleared because this message comes out of ANOTHER
        // collector's chain and would otherwise arrive mid-assembly.
        //
        // ONE port, TWO edges (GH #124): the tool name the asking model chose
        // is lifted into `context.consult_class` here and nowhere else. The
        // errand itself is identical on both -- the class picks the lane, never
        // the evidence.
        {"from": "./asker", "to": "./cogny/collector",
         "condition": "has(hop.route) && hop.route == 'consult' \
                       && has(hop.tool_name) && hop.tool_name == 'consult_cogny'",
         "modifier": {"set_hop": {"route": "'in_turn'"},
                      "set_context": {"consult_id": "hop.consult_id",
                                      "consult_class": "'consult'", "col_phase": "''"},
                      "restore_ttl": true}},
        {"from": "./asker", "to": "./cogny/collector",
         "condition": "has(hop.route) && hop.route == 'consult' \
                       && has(hop.tool_name) && hop.tool_name == 'ask_memory'",
         "modifier": {"set_hop": {"route": "'in_turn'"},
                      "set_context": {"consult_id": "hop.consult_id",
                                      "consult_class": "'lookup'", "col_phase": "''"},
                      "restore_ttl": true}},
        // ── port 2: the advice goes home on the return lane ──
        {"from": "./cogny/collector", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'answer'",
         "modifier": {"set_hop": {"route": "'in_advice'"},
                      "set_context": {"col_phase": "''"},
                      "restore_ttl": true}},
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
    // `escalate_to_deep` is the core's OWN HANDOFF tool: the fast lane's ticket
    // out is answered on a lane of its own (a fresh turn on the deep lane), so
    // the fan-in must open no expectation for it -- otherwise the round it left
    // behind sits open until the idle exit. A handoff is async by definition
    // (the dispatcher unions the two lists), and it says the second half as
    // well: the turn is over, even though the escalation carries no sentence of
    // its own -- since GH #372 that second half is what keeps a bare async call
    // WITHOUT a handoff mark (a fire-and-forget `remember`) from ending its
    // round in silence.
    std::fs::write(
        root.join(".env"),
        "OPENROUTER_API_KEY=test-key\nDISPATCHER_HANDOFF_TOOLS=escalate_to_deep\n",
    )
    .unwrap();
    write(root, "main/config.json", &main_config());
    write(root, "main/asker/config.json", &{
        code_cell(
            ASKER,
            &["consult"],
            json!({"consult_id": {"type": "string", "required": false},
                   "tool_name": {"type": "string", "required": false}}),
        )
    });
    write(
        root,
        "main/lookup/config.json",
        &code_cell(LOOKUP, &["res"], json!({})),
    );
    copy_cells(root_template, &root.join("main/cogny"));
    // `${ctx.model}` / `${ctx.model_fast}` are INSTANTIATION-side
    // substitutions; a raw copy has to be told which wire to talk to, and here
    // that wire is the mock. The two model ids differ on purpose: they are what
    // the assertions below read the lane off.
    patch(root, "main/cogny/brain/config.json", |v| {
        v["params"]["base_url"] = json!(base_url);
        v["params"]["model"] = json!(DEEP_MODEL);
    });
    patch(root, "main/cogny/brain_fast/config.json", |v| {
        v["params"]["base_url"] = json!(base_url);
        v["params"]["model"] = json!(FAST_MODEL);
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

    // 3. And both of them were the DEEP lane. `consult_cogny` is the class the
    //    two-laned seam of 1.1.0 has to leave exactly where it was.
    for (i, r) in reqs.iter().enumerate() {
        assert_eq!(
            lane_of(r),
            DEEP_MODEL,
            "call {i} of a consult_cogny errand belongs on the thinking lane"
        );
    }

    h.shutdown().await;
}

/// The lookup lane (GH #124), the whole claim in one errand: the SAME ingress,
/// the SAME collector, the SAME assembly -- and a different lane, because the
/// asking model named `ask_memory` instead of `consult_cogny`. One inference,
/// on the fast model, under the length cap the template ships.
///
/// This is what buys the seconds: the two `llm` cells are two mailboxes, so a
/// lookup no longer queues behind whatever the deep lane is writing. A single
/// process cannot show wall-clock queueing without a second consult in flight;
/// what IS provable here, and what the queueing follows from, is that the two
/// classes reach two different cells.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lookup_errand_takes_the_fast_lane_with_its_own_length_cap() {
    let Some(cogny) = shipped_cogny() else {
        return;
    };
    let mock = MockOpenAI::start(vec![canned_chat_completion(
        "you were born in berlin.",
        "stop",
    )])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &cogny, &mock.base_url);
    let (h, mut sink_rx) = boot(&td).await;

    h.send(consult(
        r#"{"tool":"ask_memory","question":"where was i born"}"#,
    ))
    .await;

    // 1. Same port, same correlation, same return lane as a deep consult.
    let advice = recv_bounded(&mut sink_rx).await.expect("the advice");
    assert_eq!(answer_text(&advice), "you were born in berlin.");
    assert_eq!(
        hop_of(&advice, "route"),
        "in_advice",
        "the lookup comes home on the SAME advice port: {:?}",
        advice.headers.hop
    );
    assert_eq!(
        ctx_of(&advice, "consult_id"),
        CONSULT_ID,
        "and under the same correlation: {:?}",
        advice.headers.context
    );
    assert_eq!(
        ctx_of(&advice, "consult_class"),
        "lookup",
        "the class rode along in context, which is what the seam edge reads: {:?}",
        advice.headers.context
    );

    // 2. ONE inference, and it was the fast cell -- not the thinking one.
    let reqs = mock.recorded_requests().await;
    assert_eq!(
        reqs.len(),
        1,
        "a lookup verbalises a bundle: no tool round, one call. Got {}",
        reqs.len()
    );
    assert_eq!(
        lane_of(&reqs[0]),
        FAST_MODEL,
        "the tool name picked the lane; the errand itself was identical"
    );

    // 3. The two halves of H1b, on the wire: the cap and the sentence.
    assert_eq!(
        reqs[0].body.get("max_tokens").and_then(|v| v.as_u64()),
        Some(512),
        "the length cap is the lane's, not the instance's: {:?}",
        reqs[0].body.get("max_tokens")
    );
    let wire = wire_of(&reqs[0]);
    assert!(
        wire.contains("Length discipline of this lane"),
        "the shipped `brevity` seed reached the provider -- a cap without the \
         instruction only truncates: {wire}"
    );

    h.shutdown().await;
}

/// The north-star guard (GH #124): the class picks the lane, so a WRONG class
/// must not produce a wrong answer. The fast lane says "not enough" the only
/// way an `llm` cell can say anything -- as a tool call -- and the split hands
/// that ticket back into the seam with the class flipped. The deep lane then
/// answers the same question, one extra recall later.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fast_lane_that_says_not_enough_escalates_to_the_thinking_lane() {
    let Some(cogny) = shipped_cogny() else {
        return;
    };
    let mock = MockOpenAI::start(vec![
        canned_tool_calls(vec![(
            "e1",
            "escalate_to_deep",
            r#"{"question":"why did the second migration fail"}"#,
        )]),
        canned_chat_completion("because the lock file was not refreshed.", "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &cogny, &mock.base_url);
    let (h, mut sink_rx) = boot(&td).await;

    h.send(consult(
        r#"{"tool":"ask_memory","question":"why did the second migration fail"}"#,
    ))
    .await;

    // 1. The answer that comes home is the DEEP lane's, on the ordinary port.
    let advice = recv_bounded(&mut sink_rx).await.expect("the advice");
    assert_eq!(
        answer_text(&advice),
        "because the lock file was not refreshed.",
        "the escalated question was answered, not the escalation ticket"
    );
    assert_eq!(hop_of(&advice, "route"), "in_advice");
    assert_eq!(
        ctx_of(&advice, "consult_id"),
        CONSULT_ID,
        "the correlation survived the lane change: {:?}",
        advice.headers.context
    );
    assert_eq!(
        ctx_of(&advice, "consult_class"),
        "consult",
        "the escalation edge flipped the class, which is what sent the second \
         assembly down the thinking lane: {:?}",
        advice.headers.context
    );

    // 2. Two calls, one per lane, in that order. This is the whole cost of a
    //    misclassification -- one extra assembly, never a wrong answer.
    let reqs = mock.recorded_requests().await;
    assert_eq!(reqs.len(), 2, "one fast attempt, one deep answer");
    assert_eq!(
        lane_of(&reqs[0]),
        FAST_MODEL,
        "the errand started on the lane its tool name named"
    );
    assert_eq!(
        lane_of(&reqs[1]),
        DEEP_MODEL,
        "and the escalation reached the thinking lane"
    );
    let deep = wire_of(&reqs[1]);
    assert!(
        deep.contains("why did the second migration fail"),
        "the deep lane saw the question, not just the ticket: {deep}"
    );
    assert!(
        !deep.contains("Length discipline of this lane"),
        "and it was NOT given the fast lane's length cap instruction: {deep}"
    );

    h.shutdown().await;
}
