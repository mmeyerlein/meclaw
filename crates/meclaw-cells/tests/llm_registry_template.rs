//! meclaw-os -- the shipped `llm-registry@1` template: the catalog behind the
//! tier names, plus a write hand that moves them ON CALL (V8 spec § 3, ruling L6
//! of 2026-08-15).
//!
//! What is pinned here is what the template PROMISES, in the order the README
//! promises it:
//!
//! 1. **The inventory, and the absence of a clock.** Three cells and a hive,
//!    no `timer`, no `schedules`, no cron string anywhere -- and a booted tree
//!    that says nothing at all until it is asked. v1 is the catalog plus an
//!    on-call hand; the closed loop is the target picture (GH #130), not
//!    this release, and an absence that is not pinned arrives by accident.
//! 2. **Resolution is deterministic.** The same request over the same catalog
//!    resolves to the same model twice, because the rank ends on a unique
//!    column -- and each round leaves one `resolutions` line.
//! 3. **A refusal is not a guess.** Requirements nothing satisfies come back
//!    `resolved: false` with `model_id: ""` -- no nearest match, no default --
//!    and a named tier whose model cannot serve the request is refused rather
//!    than silently replaced.
//! 4. **The hand moves on call, and reaches exactly the right cells.** One
//!    remap command supersedes the tier row and pushes `{system:{},
//!    params:{model}}` at every UNPINNED subscriber of that tier. The push is
//!    proved on the WIRE: the receivers are real `llm` cells against a mock
//!    provider, the params message triggers no provider call at all (accepted
//!    and silent -- the 202 form), and the next inference of each cell shows
//!    which ones actually moved.
//! 5. **`incidents` is a journal.** An incident row changes no other table,
//!    does not move the tier index, does not alter the next resolution, and
//!    emits nothing. That is the ruling: no automation crept in.
//!
//! Free of a paid call by construction: the only provider on the wire is the
//! in-process mock, and the registry itself holds no model.
//!
//! **R2b guard (GH #49 form).** `llm-registry` was PRIVATE until 2.1.0 and is
//! in `PUBLIC_TEMPLATES` since 2.2.0 (GH #855: `meclaw-os@1.9.0` refs it).
//! Every read below is still guarded per file by [`shipped_registry`], so a
//! tree without the template skips instead of failing on a dead `templates/`
//! reference -- the form `affinity_template.rs` carries.
//!
//! 6. **Since 2.2.0 (GH #855): the precedence, the package and the view.**
//!    Targeted > global > tier > start value, one deterministic function; a
//!    push carries the whole package and `$reset` for the rest, only when it
//!    changed; `show` names model, rank, reason and since per cell; a grown
//!    generation's announcement makes subscribers.
//! 7. **Since 2.3.0 (GH #858): one llm cell, and it is not on the resolution
//!    path.** `translate` answers a prose requirement at most once per change
//!    (`gh858_prose_in_a_model_out_once.rs` pins how); here only the shape is
//!    pinned -- it is the one llm cell, nothing pushes into it, and its model is
//!    a start value. The tests below write their OWN catalogue seed (the
//!    placeholder rows of 2.2.0) over the shipped one, because the shipped seed
//!    carries a hosted provider's real, dated price list, and a price moving
//!    there must not move an assertion here.

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
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

// ───────────────────────────────────────────────────────────── the shipped tree

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// Every cell the hive is made of. The list is the guard AND the inventory: a
/// cell that silently appears or disappears is caught by the set comparison in
/// [`the_hive_carries_three_cells_and_no_clock`].
const REGISTRY_FILES: &[&str] = &[
    "config.json",
    "store/config.json",
    "select/config.json",
    "hand/config.json",
    "translate/config.json",
];

const REGISTRY_SEEDS: &[&str] = &["store/seed/models.jsonl", "store/seed/tiers.jsonl"];

/// The template root, or `None` where it does not ship (the documented R2b
/// exception form, GH #49).
fn shipped_registry() -> Option<std::path::PathBuf> {
    let root = templates_root().join("llm-registry");
    for rel in REGISTRY_FILES.iter().chain(REGISTRY_SEEDS) {
        if !root.join(rel).exists() {
            return None;
        }
    }
    Some(root)
}

/// The shipped template, copied cell by cell: `config.json` files and the seeds
/// next to them travel, which is what instantiation copies.
fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
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

fn read_json(p: &std::path::Path) -> Value {
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(p).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

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

// ────────────────────────────────────────────────────────── the test-only cells

/// The asking side of the read port: one request in, the documented `tool_call`
/// turn out, and WHO is asking declared on the hop -- which the port edge then
/// promotes to `context.asker`. `select` never reads an asker out of a body.
const ASKER: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
raw = str(msgs[-1].get("text", "{}")) if msgs else "{}"
sys.stdout.write(json.dumps({
    "header": {"route": "select", "asker": "member:alex"},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "s1",
                  "text": raw}]}))
"#;

/// The commanding side of the hand port. The actor rides the hop so the port
/// edge can make it edge truth; `ACTOR` in the raw request switches it off, so
/// the refusal path has a way in.
const OPERATOR: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
raw = str(msgs[-1].get("text", "{}")) if msgs else "{}"
try:
    cmd = json.loads(raw)
except Exception:
    cmd = {}
actor = "" if cmd.pop("_no_actor", False) else "member:alex"
sys.stdout.write(json.dumps({
    "header": {"route": "command", "actor": actor},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "c1",
                  "text": json.dumps(cmd)}]}))
"#;

/// The maintenance cell at the far end of the BOOT-GRAPH edge into `./store`
/// (GH #310). It is not a port and not a bypass: `params.ports` is empty, so no
/// runtime mutation could ever draw this edge (`hive_port_boundary`) -- but the
/// bootstrap is deliberately outside that check, and the birth topology of the
/// parent is where `models`, `subscribers` and `incidents` come from, because no
/// lane of the hive writes them. The test uses the same edge to read.
const ADMIN: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
raw = str(msgs[-1].get("text", "{}")) if msgs else "{}"
sys.stdout.write(json.dumps({
    "header": {"route": "astore"},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "a1",
                  "text": raw}]}))
"#;

/// GH #855 -- the stand-in for a grown generation's announcement edge. It turns
/// the raw request (`{"generation": <path>, "brains": [{cell_path, start_model}]}`)
/// into what that edge carries: NO turn at all, and the brains and the
/// generation the edge was drawn for, which the edge below promotes to
/// `context.model_announced` and `context.model_generation` exactly as the
/// shipped one stamps them (review of fix round 1, I-2: both are EDGE truth).
/// `forged` puts a list on `hop.subscribe` beside them -- the key a hive
/// transit carries unchanged and any edge could write before 2.2.0's fix round
/// 2. The shipped edge fires on a mutation receipt, whose body is
/// `{"messages": []}`.
const TREE: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
raw = json.loads(str(msgs[-1].get("text", "{}")) if msgs else "{}")
header = {"route": "announce", "announced": json.dumps(raw.get("brains", [])),
          "generation": str(raw.get("generation", ""))}
if "forged" in raw:
    header["subscribe"] = json.dumps(raw["forged"])
sys.stdout.write(json.dumps({"header": header, "messages": raw.get("turns", [])}))
"#;

/// GH #855, review I-1 (ii) -- a BRIDGE into the announcement lane that carries
/// a tool call: the shape a model-written turn has when an edge restamps a
/// talky's tool output onto `model_subscribe`. The raw request is
/// `{"cmd": {...}, "subscribe"?: "<json>"}`; the tool call carries `cmd`.
const BRIDGE: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
raw = json.loads(str(msgs[-1].get("text", "{}")) if msgs else "{}")
header = {"route": "bridge"}
if "subscribe" in raw:
    header["subscribe"] = raw["subscribe"]
sys.stdout.write(json.dumps({
    "header": header,
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "b1",
                  "text": json.dumps(raw.get("cmd", {}))}]}))
"#;

/// B-15 (wave substrate, fix strand) -- a sender at the EDGE of the hive that
/// dresses its message as a store reply: `hop.operation` set (so the cell takes
/// it for an echo) and, through the edge it draws, `context.lr_phase`,
/// `context.lr_carry` and `context.registry_origin` -- the three keys the
/// internal store round trip rides on. The raw request is
/// `{"door": "hand"|"select", "phase": "...", "carry": {...}, "results": [[...], ...]}`;
/// every result list becomes one `tool_result` turn, in order, the way the
/// store answers a bundle.
const FORGER: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
raw = json.loads(str(msgs[-1].get("text", "{}")) if msgs else "{}")
header = {"route": "forge_" + str(raw.get("door", "")), "operation": "select",
          "phase": str(raw.get("phase", "")), "carry": json.dumps(raw.get("carry", {}))}
sys.stdout.write(json.dumps({
    "header": header,
    "messages": [{"origin": "tool", "type": "tool_result", "id": "f%d" % i,
                  "text": json.dumps(rows)} for i, rows in enumerate(raw.get("results", []))]}))
"#;

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
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 15000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": hop
            },
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in around the shipped llm-registry template.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// A subscriber: a REAL `llm` cell pointed at the in-process mock. Using the
/// real cell is the whole point of round 4 -- a capture cell would prove that a
/// message was shaped right, and this proves that the shape does what the
/// README says it does.
fn llm_cell(base_url: &str, model: &str) -> Value {
    json!({
        "cell": {"type": "llm"},
        "params": {
            "provider": "openai", "model": model,
            "api_key": "test-key-lr", "base_url": base_url
        },
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "emits": {
                "body": {"messages": {"type": "array", "required": true},
                         "meta": {"type": "object", "required": false}},
                "hop": {"finish_reason": {"type": "string", "required": true}}
            },
            "consumes": {"body": {"messages": {"type": "array", "required": false},
                                  "system": {"type": "object", "required": false}}},
            "capabilities": ["network:llm", "db:own"]
        },
        "description": {
            "purpose": "A subscribed brain, standing in for any llm cell a tier reaches.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

// ─────────────────────────────────────────────────────────────── the topology

/// The ports around the hive -- every one a literal copy of what
/// `templates/llm-registry/README.md` documents. The template draws no edge
/// that appears here.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        // ── in_select: the asker's identity becomes EDGE truth, and only here ──
        {"from": "./asker", "to": "./llm_registry/select",
         "condition": "has(hop.route) && hop.route == 'select'",
         "modifier": {"set_context": {"asker": "hop.asker"}}},
        // ── out_select ──
        {"from": "./llm_registry/select", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'answer'"},
        // ── in_hand: likewise, and a command without one is refused ──
        {"from": "./operator", "to": "./llm_registry/hand",
         "condition": "has(hop.route) && hop.route == 'command'",
         "modifier": {"set_context": {"actor": "hop.actor"}}},
        // ── out_ack ──
        {"from": "./llm_registry/hand", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'ack'"},
        // ── out_update: ONE edge per subscriber, by hand. That is the honest
        //    cost of a substrate in which cells cannot be enumerated. ──
        {"from": "./llm_registry/hand", "to": "./sub_a",
         "condition": "has(hop.route) && hop.route == 'update' && hop.subscriber == '/sub_a'"},
        {"from": "./llm_registry/hand", "to": "./sub_b",
         "condition": "has(hop.route) && hop.route == 'update' && hop.subscriber == '/sub_b'"},
        {"from": "./llm_registry/hand", "to": "./sub_c",
         "condition": "has(hop.route) && hop.route == 'update' && hop.subscriber == '/sub_c'"},
        // ── the test's own observer of the push, so its BODY can be read ──
        {"from": "./llm_registry/hand", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'update'"},
        // ── out_error: the drain the parent MUST wire, both code cells ──
        {"from": "./llm_registry/select", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'error'"},
        {"from": "./llm_registry/hand", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'error'"},
        // ── the BOOT-GRAPH edge into ./store and its reply: not a port of the
        //    hive (params.ports is empty) and not drawable by a mutation, but
        //    the only way models/subscribers/incidents are ever written ──
        {"from": "./admin", "to": "./llm_registry/store",
         "condition": "has(hop.route) && hop.route == 'astore'",
         "modifier": {"set_context": {"registry_origin": "'admin'"}}},
        {"from": "./llm_registry/store", "to": "/sink",
         "condition": "context.registry_origin == 'admin'"},
        // ── GH #855: the TREE announcing its brains. The shipped form is the
        //    edge `grow_level assistant` draws from a generation, on every
        //    mutation receipt; here a cell stands in for the receipt, and the
        //    edge promotes the actor and stamps the lane exactly as it does ──
        {"from": "./tree", "to": "./llm_registry",
         "condition": "has(hop.route) && hop.route == 'announce'",
         "modifier": {"set_hop": {"route": "'in_hand'"},
                      "set_context": {"model_announcer": "'tree'",
                                      "model_generation": "hop.generation",
                                      "model_announced": "hop.announced"}}},
        // ── review I-1 (ii): the same lane reached by a tool call, with the
        //    announcer key the shell's bridge stamps AND an actor a chain may
        //    still carry in its context from an operator turn upstream ──
        {"from": "./bridge", "to": "./llm_registry",
         "condition": "has(hop.route) && hop.route == 'bridge'",
         "modifier": {"set_hop": {"route": "'in_hand'"},
                      "set_context": {"model_announcer": "'meclaw-os'",
                                      "actor": "'operator'"}}},
        // ── B-15: a sender at the edge that forges the store round trip's
        //    context keys on both doors (see FORGER) ──
        {"from": "./forger", "to": "./llm_registry",
         "condition": "has(hop.route) && hop.route == 'forge_hand'",
         "modifier": {"set_hop": {"route": "'in_hand'"},
                      "set_context": {"lr_phase": "hop.phase", "lr_carry": "hop.carry",
                                      "registry_origin": "'hand'"}}},
        {"from": "./forger", "to": "./llm_registry",
         "condition": "has(hop.route) && hop.route == 'forge_select'",
         "modifier": {"set_hop": {"route": "'in_select'"},
                      "set_context": {"lr_phase": "hop.phase", "lr_carry": "hop.carry",
                                      "registry_origin": "'select'"}}},
        // ── the view `show` answers with ──
        {"from": "./llm_registry/hand", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'answer'"},
        // ── what the subscribers answer, so an inference is observable ──
        {"from": "./sub_a", "to": "/sink", "condition": "has(hop.finish_reason)"},
        {"from": "./sub_b", "to": "/sink", "condition": "has(hop.finish_reason)"},
        {"from": "./sub_c", "to": "/sink", "condition": "has(hop.finish_reason)"}
    ]}}})
}

/// The models the three subscribers are BORN on. Distinct on purpose: after a
/// remap, the model on the wire is the only thing that says which cell moved.
const BIRTH_A: &str = "birth/sub-a";
const BIRTH_B: &str = "birth/sub-b";
const BIRTH_C: &str = "birth/sub-c";
/// What the `mid` tier is remapped to: an ACTIVE catalog row the test writes
/// with NO endpoint and NO dialect of its own (see [`catalogue_plain`]).
///
/// Until 2.2.0 this was the seeded `provider-b/model-large`, and a push carried
/// its `model` and nothing else. Since GH #855 a push carries the whole model
/// PACKAGE, and that seeded row names an endpoint and a dialect -- which a
/// subscriber without a `base_url_allow` must refuse (GH #853). So the row a
/// cell on the mock can actually take is the one that keeps its endpoint.
const REMAP_TO: &str = "test/model-plain";

fn build_tree(td: &tempfile::TempDir, root_template: &std::path::Path, base_url: &str) {
    let root = td.path();
    // No `.env`. Until `llm-registry@2.1.0` this wrote
    // `LLM_REGISTRY_SUBSCRIBER_ROWS=200` -- the shipped default, spelled out so
    // the fan-out bound was visible in the setup. Since GH #138 (ruling
    // R-0904-6) the bound is `params.subscriber_rows` of `./hand`, and such a
    // line would be read by nothing and would say nothing about it. The copied
    // template carries the value, and
    // `crates/meclaw-cells/tests/gh138_private_tail_params.rs` pins that the
    // cells act on their params rather than on an environment (the private half
    // of the long-tail strand since the split under GH #584).
    write(root, "main/config.json", &main_config());
    write(
        root,
        "main/asker/config.json",
        &code_cell(
            ASKER,
            &["select"],
            json!({"asker": {"type": "string", "required": false}}),
        ),
    );
    write(
        root,
        "main/operator/config.json",
        &code_cell(
            OPERATOR,
            &["command"],
            json!({"actor": {"type": "string", "required": false}}),
        ),
    );
    write(
        root,
        "main/admin/config.json",
        &code_cell(ADMIN, &["astore"], json!({})),
    );
    write(
        root,
        "main/tree/config.json",
        &code_cell(
            TREE,
            &["announce"],
            json!({"subscribe": {"type": "string", "required": false},
                   "announced": {"type": "string", "required": false},
                   "generation": {"type": "string", "required": false}}),
        ),
    );
    write(
        root,
        "main/bridge/config.json",
        &code_cell(
            BRIDGE,
            &["bridge"],
            json!({"subscribe": {"type": "string", "required": false}}),
        ),
    );
    write(
        root,
        "main/forger/config.json",
        &code_cell(
            FORGER,
            &["forge_hand", "forge_select"],
            json!({"operation": {"type": "string", "required": false},
                   "phase": {"type": "string", "required": false},
                   "carry": {"type": "string", "required": false}}),
        ),
    );
    write(root, "main/sub_a/config.json", &llm_cell(base_url, BIRTH_A));
    write(root, "main/sub_b/config.json", &llm_cell(base_url, BIRTH_B));
    write(root, "main/sub_c/config.json", &llm_cell(base_url, BIRTH_C));
    copy_cells(root_template, &root.join("main/llm_registry"));
    write_placeholder_seed(&root.join("main/llm_registry/store/seed"));
    // GH #858: the translator is pointed at the mock, so even a question these
    // tests never ask could not leave the process.
    let tp = root.join("main/llm_registry/translate/config.json");
    let mut translate = read_json(&tp);
    translate["params"]["base_url"] = json!(base_url);
    translate["params"]["model"] = json!("test/translator");
    std::fs::write(
        &tp,
        meclaw_core::serde_json::to_string_pretty(&translate).unwrap(),
    )
    .unwrap();
}

/// The catalogue these tests resolve against: the generic rows 2.2.0 shipped,
/// kept as a fixture since 2.3.0 put a real, dated price list in the seed.
/// Two rows are awkward on purpose: the retired one is the cheapest, so a
/// `select` that ignored `status` would pick it; the local one costs zero, so
/// cost is never the only rank key.
fn write_placeholder_seed(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap();
    let gw = "https://gateway.example/api/v1";
    let models = [
        json!({"schema": {"model_id": "text", "provider": "text", "base_url": "text",
            "wire_dialect": "text", "context_window": "int", "cost_in": "int",
            "cost_out": "int", "caps": "json", "traits": "json", "status": "text",
            "note": "text", "package": "json", "prompt": "text", "strengths": "text"}}),
        json!({"model_id": "provider-a/model-small", "provider": "gateway", "base_url": gw,
            "wire_dialect": "chat_completions", "context_window": 128000, "cost_in": 15,
            "cost_out": 60, "caps": {"json": true, "reasoning": false, "tools": true,
            "vision": false}, "traits": {}, "status": "active", "note": "TEST FIXTURE",
            "package": {}, "prompt": "", "strengths": ""}),
        json!({"model_id": "provider-a/model-mid", "provider": "gateway", "base_url": gw,
            "wire_dialect": "chat_completions", "context_window": 200000, "cost_in": 100,
            "cost_out": 400, "caps": {"json": true, "reasoning": false, "tools": true,
            "vision": true}, "traits": {}, "status": "active", "note": "TEST FIXTURE",
            "package": {}, "prompt": "", "strengths": ""}),
        json!({"model_id": "provider-b/model-large", "provider": "gateway", "base_url": gw,
            "wire_dialect": "responses", "context_window": 400000, "cost_in": 500,
            "cost_out": 2000, "caps": {"json": true, "reasoning": true, "tools": true,
            "vision": true}, "traits": {}, "status": "active", "note": "TEST FIXTURE",
            "package": {"max_tokens": 4096, "reasoning_effort": "medium"},
            "prompt": "EXAMPLE -- the lines this model needs.", "strengths": ""}),
        json!({"model_id": "local/model-onprem", "provider": "local",
            "base_url": "http://localhost:8000/v1", "wire_dialect": "chat_completions",
            "context_window": 32000, "cost_in": 0, "cost_out": 0, "caps": {"json": true,
            "reasoning": false, "tools": true, "vision": false}, "traits": {},
            "status": "active", "note": "TEST FIXTURE", "package": {}, "prompt": "",
            "strengths": ""}),
        json!({"model_id": "provider-b/model-legacy", "provider": "gateway", "base_url": gw,
            "wire_dialect": "chat_completions", "context_window": 8000, "cost_in": 5,
            "cost_out": 20, "caps": {"json": false, "reasoning": false, "tools": false,
            "vision": false}, "traits": {}, "status": "retired", "note": "TEST FIXTURE",
            "package": {}, "prompt": "", "strengths": ""}),
    ];
    let tiers = [
        json!({"schema": {"tier": "text", "model_id": "text", "since": "text",
                          "decided_by": "text", "active": "int"}}),
        json!({"tier": "light", "model_id": "provider-a/model-small",
               "since": "2026-01-01T00:00:00Z", "decided_by": "seed", "active": 1}),
        json!({"tier": "mid", "model_id": "provider-a/model-mid",
               "since": "2026-01-01T00:00:00Z", "decided_by": "seed", "active": 1}),
        json!({"tier": "strong", "model_id": "provider-b/model-large",
               "since": "2026-01-01T00:00:00Z", "decided_by": "seed", "active": 1}),
        json!({"tier": "local", "model_id": "local/model-onprem",
               "since": "2026-01-01T00:00:00Z", "decided_by": "seed", "active": 1}),
    ];
    let lines = |rows: &[Value]| {
        let mut out = String::new();
        for r in rows {
            out.push_str(&r.to_string());
            out.push('\n');
        }
        out
    };
    std::fs::write(dir.join("models.jsonl"), lines(&models)).unwrap();
    std::fs::write(dir.join("tiers.jsonl"), lines(&tiers)).unwrap();
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

fn to(cell: &str, text: &str) -> Message {
    MessageBuilder::new(Path::new(cell))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
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

fn turn_text(m: &Message) -> String {
    body_of(m)["messages"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn turn_json(m: &Message) -> Value {
    meclaw_core::serde_json::from_str(&turn_text(m)).unwrap_or(Value::Null)
}

async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

/// The next message matching `want`, skipping whatever else the sink collects.
async fn recv_matching(
    rx: &mut mpsc::Receiver<Message>,
    label: &str,
    want: impl Fn(&Message) -> bool,
) -> Message {
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..24 {
        let m = recv_bounded(rx).await.unwrap_or_else(|| {
            panic!("nothing more arrived while waiting for {label}; saw {seen:?}");
        });
        if want(&m) {
            return m;
        }
        seen.push(format!("{:?}: {}", m.headers.hop, turn_text(&m)));
    }
    panic!("{label} never arrived; saw {seen:?}");
}

async fn recv_route(rx: &mut mpsc::Receiver<Message>, route: &str) -> Message {
    let owned = route.to_string();
    recv_matching(rx, route, move |m| hop_of(m, "route") == owned).await
}

/// One store op over the boot-graph edge into `./store`, returned as its rows.
async fn admin(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>, op: Value) -> Value {
    h.send(to(
        "/admin",
        &meclaw_core::serde_json::to_string(&op).unwrap(),
    ))
    .await;
    let m = recv_matching(rx, "admin answer", |m| !hop_of(m, "operation").is_empty()).await;
    turn_json(&m)
}

/// One lookup through the read port, returned as the resolution payload.
async fn resolve(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>, req: Value) -> Value {
    h.send(to(
        "/asker",
        &meclaw_core::serde_json::to_string(&req).unwrap(),
    ))
    .await;
    turn_json(&recv_route(rx, "answer").await)
}

/// One inference on a subscriber, answered as the model that served it. This is
/// the only place the wire says which cell a remap actually moved.
async fn inference_model(
    h: &ColonyHandle,
    rx: &mut mpsc::Receiver<Message>,
    cell: &str,
    mock: &MockOpenAI,
) -> String {
    let before = mock.recorded_requests().await.len();
    h.send(to(cell, "ping")).await;
    let _ = recv_matching(rx, "an inference answer", |m| {
        !hop_of(m, "finish_reason").is_empty()
    })
    .await;
    let snaps = mock.recorded_requests().await;
    assert_eq!(
        snaps.len(),
        before + 1,
        "exactly one provider call per inference"
    );
    snaps[before].model().unwrap_or_default().to_string()
}

fn now_iso() -> &'static str {
    "2026-08-15T00:00:00Z"
}

/// The three subscriber rows: two on `mid`, one of them pinned, and one on
/// `light`. Written over the boot-graph edge, because NO lane of this hive
/// writes `subscribers` -- `select` and `hand` only read it, and the seed does
/// not carry it either (GH #310). This is the one path there is.
async fn wire_subscribers(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>) {
    for (path, tier, pinned) in [
        ("/sub_a", "mid", 0),
        ("/sub_b", "mid", 1),
        ("/sub_c", "light", 0),
    ] {
        admin(
            h,
            rx,
            json!({"operation": "insert", "table": "subscribers",
                   "row": {"cell_path": path, "tier": tier, "pinned": pinned,
                           "wired_at": now_iso()}}),
        )
        .await;
    }
}

/// GH #855 -- a catalogue row written over the boot-graph edge, the way an
/// operator maintains `models`. No endpoint and no dialect: a package that
/// names neither keeps the cell's own (and `$reset`s any a previous package
/// set), which is what lets a subscriber on the in-process mock take it.
async fn catalogue_plain(
    h: &ColonyHandle,
    rx: &mut mpsc::Receiver<Message>,
    model_id: &str,
    package: Value,
    prompt: &str,
) {
    admin(
        h,
        rx,
        json!({"operation": "insert", "table": "models",
               "row": {"model_id": model_id, "provider": "gateway", "base_url": "",
                       "wire_dialect": "", "context_window": 32000, "cost_in": 1,
                       "cost_out": 1, "caps": {"tools": true}, "traits": {},
                       "status": "active", "note": "TEST ROW", "package": package,
                       "prompt": prompt}}),
    )
    .await;
}

/// One command through the hand port, answered as (pushes, ack payload): every
/// `update` the sink saw before the acknowledgement, which the hand emits AFTER
/// its pushes, so the list is complete when the ack arrives.
async fn command(
    h: &ColonyHandle,
    rx: &mut mpsc::Receiver<Message>,
    cmd: Value,
) -> (Vec<Message>, Value) {
    h.send(to(
        "/operator",
        &meclaw_core::serde_json::to_string(&cmd).unwrap(),
    ))
    .await;
    let mut pushes = Vec::new();
    for _ in 0..24 {
        let m = recv_bounded(rx)
            .await
            .unwrap_or_else(|| panic!("no ack for {cmd}; pushes so far {}", pushes.len()));
        match hop_of(&m, "route").as_str() {
            "update" => pushes.push(m),
            "ack" => return (pushes, turn_json(&m)),
            _ => {}
        }
    }
    panic!("no ack for {cmd}");
}

/// The `show` view, as its list of cells keyed by path.
async fn show(
    h: &ColonyHandle,
    rx: &mut mpsc::Receiver<Message>,
) -> std::collections::BTreeMap<String, Value> {
    h.send(to("/operator", r#"{"op":"show"}"#)).await;
    let m = recv_route(rx, "answer").await;
    let v = turn_json(&m);
    // The view says what was SENT and says that it is only that: a brain that
    // refused a push keeps its previous params, and the registry cannot know.
    assert!(
        v["view"].as_str().is_some_and(|t| t.contains("last sent")),
        "show must say it reports what was sent, not what runs: {v}"
    );
    v["cells"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|c| (c["cell_path"].as_str().unwrap_or_default().to_string(), c))
        .collect()
}

fn pushed_to(pushes: &[Message]) -> Vec<(String, String, String)> {
    let mut v: Vec<(String, String, String)> = pushes
        .iter()
        .map(|m| {
            (
                hop_of(m, "subscriber"),
                hop_of(m, "rank"),
                body_of(m)["params"]["model"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    v.sort();
    v
}

/// The package keys a push resets, as a sorted list.
fn resets(m: &Message) -> Vec<String> {
    let mut v: Vec<String> = body_of(m)["params"]["$reset"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|k| k.as_str().map(str::to_string))
        .collect();
    v.sort();
    v
}

/// The package keys of the llm cell, as the hand script spells them. The Rust
/// constant is the authority; `gh855_an_override_reaches_the_brain_through_its_door.rs`
/// holds the script's literal, this copy and the constant together.
const PACKAGE_KEYS: &[&str] = &[
    "model",
    "base_url",
    "wire_dialect",
    "reasoning_effort",
    "reasoning_wire",
    "reasoning",
    "thinking_budget",
    "max_tokens",
    "temperature",
    "external_timeout_ms",
    "provider_extra",
    "model_prompt",
];

// ═══════════════════════════════════════════════════════════════════════ pins

/// Three cells and a hive, and NO clock. Pinned as a set, because the shape is
/// the ruling: v1 is the catalog plus a write hand on call. A `timer` here, a
/// `schedules` block, or a cron string in any config would be the control loop
/// arriving by accident -- and the loop is GH #130's business, not v1's.
#[test]
fn the_hive_carries_four_cells_and_no_clock() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let mut found = Vec::new();
    collect_configs(&root, &root, &mut found);
    let mut found: Vec<String> = found
        .iter()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    found.sort();
    let mut want: Vec<String> = REGISTRY_FILES.iter().map(|s| s.to_string()).collect();
    want.sort();
    assert_eq!(
        found, want,
        "llm-registry is store + select + hand + translate: no probe, no clock"
    );
    for rel in REGISTRY_FILES {
        let cfg = read_json(&root.join(rel));
        let ty = cfg["cell"]["type"].as_str().unwrap_or_default().to_string();
        // GH #858 (OR-SN-49): exactly one llm cell, the translator, and it is
        // not on the resolution path -- its model is a START VALUE out of the
        // environment, never a context key the registry could fill, and no
        // edge of the hive carries a push into it.
        if ty == "llm" {
            assert_eq!(
                *rel, "translate/config.json",
                "{rel} is an llm cell -- the registry that repairs a model holds only its translator"
            );
            let model = cfg["params"]["model"].as_str().unwrap_or_default();
            assert!(
                model.starts_with("${LLM_REGISTRY_TRANSLATOR_MODEL:-"),
                "the translator's model is a start value, never resolved here: {model}"
            );
        }
        assert_ne!(
            ty, "timer",
            "{rel} is a timer -- v1 has no tick, and that is the ruling"
        );
        let raw = std::fs::read_to_string(root.join(rel)).unwrap();
        assert!(
            !raw.contains("schedules"),
            "{rel} declares schedules -- nothing in v1 fires on its own"
        );
        assert!(
            !raw.contains("cron"),
            "{rel} names a cron -- nothing in v1 fires on its own"
        );
    }
    let hive = read_json(&root.join("config.json"));
    let into_translate: Vec<&Value> = hive["params"]["graph"]["edges"]
        .as_array()
        .map(|e| e.iter().filter(|e| e["to"] == "./translate").collect())
        .unwrap_or_default();
    assert_eq!(into_translate.len(), 1, "{into_translate:?}");
    assert_eq!(into_translate[0]["from"], "./hand");
    assert_eq!(
        into_translate[0]["condition"], "has(hop.route) && hop.route == 'translate'",
        "only a question reaches the translator, never an `update`"
    );
}

/// The other half of the same ruling, at runtime: a booted registry with a full
/// catalog and three wired subscribers says NOTHING until somebody asks. No
/// probe tick, no health sweep, no unsolicited push.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_booted_registry_emits_nothing_on_its_own() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let mock = MockOpenAI::start(vec![]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, &format!("{}/v1", mock.base_url));
    let (h, mut rx) = boot(&td).await;

    wire_subscribers(&h, &mut rx).await;

    // Long enough that any plausible tick would have fired -- and the shortest
    // cron this substrate can express is one second.
    let quiet = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
    assert!(
        quiet.is_err(),
        "an idle registry must emit nothing at all, got {:?}",
        quiet.map(|m| m.map(|m| m.headers.hop.clone()))
    );
    assert!(
        mock.recorded_requests().await.is_empty(),
        "and it must reach no provider: this hive holds no model"
    );

    h.shutdown().await;
}

/// The read port, resolved twice. The claim is not that some model comes back
/// -- it is that the SAME one comes back, because the rank ends on `model_id`
/// and no two active rows share it. And each round leaves its journal line.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_request_resolves_the_same_way_twice_and_is_journalled() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let mock = MockOpenAI::start(vec![]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, &format!("{}/v1", mock.base_url));
    let (h, mut rx) = boot(&td).await;

    // 1. A tier is an INDEX LOOKUP: the answer is the row the index points at,
    //    with the catalog facts that make it usable without a second question.
    let by_tier = resolve(&h, &mut rx, json!({"tier": "mid"})).await;
    assert_eq!(by_tier["resolved"].as_bool(), Some(true), "{by_tier}");
    assert_eq!(by_tier["model_id"].as_str(), Some("provider-a/model-mid"));
    assert_eq!(by_tier["reason_code"].as_str(), Some("tier_active"));
    assert_eq!(by_tier["provider"].as_str(), Some("gateway"));
    assert_eq!(by_tier["wire_dialect"].as_str(), Some("chat_completions"));
    assert_eq!(
        by_tier["cost_out"].as_i64(),
        Some(400),
        "the price rides along as a CENT INTEGER, which is why it can be ordered"
    );

    // 2. Without a tier it is a ranked search -- and the rank is part of the
    //    answer, so a caller can see WHY this row won.
    let ranked = resolve(
        &h,
        &mut rx,
        json!({"capability": ["tools", "vision"], "max_cost": 500}),
    )
    .await;
    assert_eq!(ranked["resolved"].as_bool(), Some(true), "{ranked}");
    assert_eq!(
        ranked["model_id"].as_str(),
        Some("provider-a/model-mid"),
        "the cheapest ACTIVE model that has both capabilities: {ranked}"
    );
    assert_eq!(ranked["reason_code"].as_str(), Some("ranked"));
    assert_eq!(
        ranked["rank"].as_str(),
        Some("cost_out asc, cost_in asc, context_window desc, model_id asc")
    );

    // 3. The same request again. This is the determinism claim, and it is the
    //    reason `select` holds no model: a comparison repeats, a judgement does
    //    not.
    let again = resolve(
        &h,
        &mut rx,
        json!({"capability": ["tools", "vision"], "max_cost": 500}),
    )
    .await;
    assert_eq!(
        again["model_id"], ranked["model_id"],
        "the same request over the same catalog must resolve identically"
    );
    assert_eq!(again["reason_code"], ranked["reason_code"]);

    // 4. The retired row is the CHEAPEST in the seed, and it was never in the
    //    running: `status` is a filter, not a hint.
    let cheapest = resolve(&h, &mut rx, json!({})).await;
    assert_eq!(cheapest["resolved"].as_bool(), Some(true), "{cheapest}");
    assert_ne!(
        cheapest["model_id"].as_str(),
        Some("provider-b/model-legacy"),
        "a retired model must never be resolved, however cheap it is"
    );

    // 5. Four lookups, four journal lines -- and the asker on each of them is
    //    the one the EDGE promoted, not one a body claimed.
    let rows = admin(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "resolutions",
               "columns": ["tier", "model_id", "cell_path", "reason"],
               "order_by": [{"col": "at", "dir": "asc"}], "limit": 50}),
    )
    .await;
    let rows = rows.as_array().cloned().unwrap_or_default();
    assert_eq!(rows.len(), 4, "one line per lookup: {rows:?}");
    assert!(
        rows.iter()
            .all(|r| r["cell_path"].as_str() == Some("member:alex")),
        "the journal names the asker the edge wrote: {rows:?}"
    );
    assert_eq!(rows[0]["reason"].as_str(), Some("tier_active"));
    assert_eq!(rows[1]["reason"].as_str(), Some("ranked"));

    h.shutdown().await;
}

/// The refusal, in both of its shapes. Nothing satisfiable comes back with a
/// reason code and NO model id; and a named tier whose model cannot serve the
/// request is refused rather than quietly swapped for one that can -- which
/// would move a cell off the tier its operator chose.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unsatisfiable_requirements_are_refused_and_never_guessed() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let mock = MockOpenAI::start(vec![]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, &format!("{}/v1", mock.base_url));
    let (h, mut rx) = boot(&td).await;

    // 1. A budget nothing meets. Not the nearest model, not the cheapest, not a
    //    default -- an empty model id.
    let broke = resolve(&h, &mut rx, json!({"capability": "vision", "max_cost": 1})).await;
    assert_eq!(broke["resolved"].as_bool(), Some(false), "{broke}");
    assert_eq!(broke["reason_code"].as_str(), Some("no_candidate"));
    assert_eq!(
        broke["model_id"].as_str(),
        Some(""),
        "a refusal names NO model at all: {broke}"
    );
    assert!(
        broke["considered"].as_i64().unwrap_or(0) > 0,
        "and it looked -- the refusal is a result, not an empty catalog: {broke}"
    );

    // 2. A tier that exists, and a requirement its model does not meet. The
    //    catalog HAS a model with vision under this ceiling; it is not offered.
    let below = resolve(
        &h,
        &mut rx,
        json!({"tier": "light", "capability": "vision"}),
    )
    .await;
    assert_eq!(below["resolved"].as_bool(), Some(false), "{below}");
    assert_eq!(
        below["reason_code"].as_str(),
        Some("tier_below_requirement"),
        "a named tier is a lookup, not a search: {below}"
    );
    assert_eq!(below["model_id"].as_str(), Some(""));
    assert_eq!(
        below["detail"].as_str(),
        Some("capability_missing"),
        "and the refusal says WHICH requirement failed: {below}"
    );

    // 3. A tier nobody has decided about.
    let unknown = resolve(&h, &mut rx, json!({"tier": "does-not-exist"})).await;
    assert_eq!(unknown["resolved"].as_bool(), Some(false), "{unknown}");
    assert_eq!(unknown["reason_code"].as_str(), Some("unknown_tier"));

    // 4. Refusals are journalled too. A log that only records successes cannot
    //    answer "why did nothing get a model last Tuesday".
    let rows = admin(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "resolutions",
               "columns": ["model_id", "reason"], "limit": 50}),
    )
    .await;
    let rows = rows.as_array().cloned().unwrap_or_default();
    assert_eq!(rows.len(), 3, "three refusals, three lines: {rows:?}");
    assert!(
        rows.iter().all(|r| r["model_id"].as_str() == Some("")),
        "a refused line carries no model either: {rows:?}"
    );

    h.shutdown().await;
}

/// The write hand, on call. One command, and the wire shows all three halves of
/// the promise: the message form an `llm` cell accepts in silence, the tier
/// index superseded, and exactly the unpinned subscribers of that tier moved.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_remap_command_pushes_params_to_exactly_the_unpinned_subscribers() {
    let Some(root) = shipped_registry() else {
        return;
    };
    // Three inferences, one per subscriber, and not one more: the params push
    // itself must reach no provider.
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("a", "stop"),
        canned_chat_completion("b", "stop"),
        canned_chat_completion("c", "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, &format!("{}/v1", mock.base_url));
    let (h, mut rx) = boot(&td).await;
    wire_subscribers(&h, &mut rx).await;
    catalogue_plain(&h, &mut rx, REMAP_TO, json!({}), "").await;

    h.send(to(
        "/operator",
        &meclaw_core::serde_json::to_string(&json!({
            "op": "remap", "tier": "mid", "model_id": REMAP_TO,
            "reason": "provider-a degraded"
        }))
        .unwrap(),
    ))
    .await;

    // 1. The push, as the observer sees it: exactly ONE update message, and it
    //    is addressed at the unpinned mid subscriber. The pinned one and the
    //    light one produce no message at all.
    let pushed = recv_route(&mut rx, "update").await;
    assert_eq!(
        hop_of(&pushed, "subscriber"),
        "/sub_a",
        "the push names its subscriber, which is how the parent's edge finds \
         the llm cell: {:?}",
        pushed.headers.hop
    );
    assert_eq!(hop_of(&pushed, "tier"), "mid");
    assert_eq!(hop_of(&pushed, "model_id"), REMAP_TO);

    // 2. The FORM (GH #855). An EMPTY `system` slot, a `params` overlay and no
    //    `messages[]` -- the params-only body an llm cell persists and answers
    //    with nothing (accepted, silent, free). The empty slot is what makes it
    //    a valid body at all (ubf-body.json wants `system`, `messages` or
    //    `attachments`). The overlay carries the whole package: this row names
    //    only a model, so every other package key is RESET, and no key of an
    //    earlier package can survive the move.
    let body = body_of(&pushed);
    assert_eq!(
        body["params"]["model"].as_str(),
        Some(REMAP_TO),
        "the overlay carries the model: {body}"
    );
    let mut rest: Vec<String> = PACKAGE_KEYS
        .iter()
        .filter(|k| **k != "model")
        .map(|k| k.to_string())
        .collect();
    rest.sort();
    assert_eq!(
        resets(&pushed),
        rest,
        "every key the package does not set is reset: {body}"
    );
    assert!(
        body["system"].is_object() && body["system"].as_object().is_some_and(|o| o.is_empty()),
        "the system slot is present and EMPTY: {body}"
    );
    assert!(
        body.get("messages").is_none(),
        "no messages slot -- a turn here would cost a provider call: {body}"
    );
    assert!(
        !body["params"]
            .as_object()
            .is_some_and(|o| o.contains_key("provider") || o.contains_key("api_key")),
        "and the overlay never touches the immutable auth dimension: {body}"
    );

    // 3. The acknowledgement, which is emitted AFTER the pushes -- so by the
    //    time it reaches this sink, every update is already in its mailbox.
    let ack = turn_json(&recv_route(&mut rx, "ack").await);
    assert_eq!(ack["outcome"].as_str(), Some("accepted"), "{ack}");
    assert_eq!(ack["pushed"].as_i64(), Some(1), "{ack}");
    assert_eq!(
        ack["skipped_pinned"].as_i64(),
        Some(1),
        "the pinned subscriber is counted, not silently dropped: {ack}"
    );
    assert_eq!(
        ack["decided_by"].as_str(),
        Some("member:alex"),
        "the decider is edge truth: {ack}"
    );

    // 4. Nothing reached a provider. The whole remap cost zero tokens.
    let calls: Vec<String> = mock
        .recorded_requests()
        .await
        .iter()
        .map(|r| r.model().unwrap_or_default().to_string())
        .collect();
    assert!(
        calls.is_empty(),
        "a params push must not trigger inference: {calls:?}"
    );

    // 5. What each cell RUNS now, read off the wire one inference at a time.
    assert_eq!(
        inference_model(&h, &mut rx, "/sub_a", &mock).await,
        REMAP_TO,
        "the unpinned mid subscriber moved"
    );
    assert_eq!(
        inference_model(&h, &mut rx, "/sub_b", &mock).await,
        BIRTH_B,
        "the PINNED mid subscriber did not: a pin is a decision written down"
    );
    assert_eq!(
        inference_model(&h, &mut rx, "/sub_c", &mock).await,
        BIRTH_C,
        "and another tier is another question entirely"
    );

    // 6. The index moved by supersede, not by overwrite: what `mid` used to
    //    mean is still readable.
    let tiers = admin(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "tiers",
               "columns": ["tier", "model_id", "active", "decided_by"],
               "where": {"tier": "mid"},
               "order_by": [{"col": "active", "dir": "desc"}], "limit": 10}),
    )
    .await;
    let tiers = tiers.as_array().cloned().unwrap_or_default();
    assert_eq!(tiers.len(), 2, "the old row is still there: {tiers:?}");
    assert_eq!(tiers[0]["model_id"].as_str(), Some(REMAP_TO));
    assert_eq!(tiers[0]["active"].as_i64(), Some(1));
    assert_eq!(tiers[0]["decided_by"].as_str(), Some("member:alex"));
    assert_eq!(tiers[1]["model_id"].as_str(), Some("provider-a/model-mid"));
    assert_eq!(tiers[1]["active"].as_i64(), Some(0));

    // 7. And the journal separates the two outcomes by name.
    let rows = admin(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "resolutions",
               "columns": ["cell_path", "model_id", "reason"],
               "where": {"reason": {"in": ["hand_remap", "hand_skipped_pinned"]}},
               "order_by": [{"col": "cell_path", "dir": "asc"}], "limit": 20}),
    )
    .await;
    let rows = rows.as_array().cloned().unwrap_or_default();
    assert_eq!(rows.len(), 2, "one line per mid subscriber: {rows:?}");
    assert_eq!(rows[0]["cell_path"].as_str(), Some("/sub_a"));
    assert_eq!(rows[0]["reason"].as_str(), Some("hand_remap"));
    assert_eq!(rows[1]["cell_path"].as_str(), Some("/sub_b"));
    assert_eq!(rows[1]["reason"].as_str(), Some("hand_skipped_pinned"));

    h.shutdown().await;
}

/// The two ways a remap is refused, and both leave the world untouched. A
/// command with no actor is the interesting one: a remap changes what other
/// cells run, so it needs an identity an edge wrote.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_remap_without_an_actor_or_onto_an_unknown_model_is_refused() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let mock = MockOpenAI::start(vec![]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, &format!("{}/v1", mock.base_url));
    let (h, mut rx) = boot(&td).await;
    wire_subscribers(&h, &mut rx).await;

    // 1. No actor on the edge.
    h.send(to(
        "/operator",
        &meclaw_core::serde_json::to_string(&json!({
            "op": "remap", "tier": "mid", "model_id": REMAP_TO, "_no_actor": true
        }))
        .unwrap(),
    ))
    .await;
    let ack = turn_json(&recv_route(&mut rx, "ack").await);
    assert_eq!(ack["outcome"].as_str(), Some("rejected"), "{ack}");
    assert_eq!(ack["reason_code"].as_str(), Some("no_actor"), "{ack}");

    // 2. A model the catalog does not carry as active -- the retired one is the
    //    sharper case: the row EXISTS, and it is still not a legal target.
    h.send(to(
        "/operator",
        &meclaw_core::serde_json::to_string(&json!({
            "op": "remap", "tier": "mid", "model_id": "provider-b/model-legacy"
        }))
        .unwrap(),
    ))
    .await;
    let ack = turn_json(&recv_route(&mut rx, "ack").await);
    assert_eq!(ack["outcome"].as_str(), Some("rejected"), "{ack}");
    assert_eq!(ack["reason_code"].as_str(), Some("unknown_model"), "{ack}");

    // 3. The index never moved, and nothing was pushed at anybody.
    let tiers = admin(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "tiers",
               "columns": ["model_id", "active"], "where": {"tier": "mid"},
               "limit": 10}),
    )
    .await;
    let tiers = tiers.as_array().cloned().unwrap_or_default();
    assert_eq!(tiers.len(), 1, "a refusal writes no index row: {tiers:?}");
    assert_eq!(tiers[0]["model_id"].as_str(), Some("provider-a/model-mid"));
    assert_eq!(tiers[0]["active"].as_i64(), Some(1));
    assert_eq!(
        inference_model(&h, &mut rx, "/sub_a", &mock).await,
        BIRTH_A,
        "and the subscriber is still on the model it was born with"
    );

    h.shutdown().await;
}

/// `incidents` is a journal and nothing more. This is the ruling of 2026-08-15
/// pinned as behaviour: a row lands, and the world does not move. The day
/// somebody wires an incident rule, this test is what notices.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_incident_row_changes_nothing_and_triggers_nothing() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let mock = MockOpenAI::start(vec![canned_chat_completion("a", "stop")]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, &format!("{}/v1", mock.base_url));
    let (h, mut rx) = boot(&td).await;
    wire_subscribers(&h, &mut rx).await;

    let before = resolve(&h, &mut rx, json!({"tier": "mid"})).await;
    assert_eq!(before["model_id"].as_str(), Some("provider-a/model-mid"));

    // The worst incident the schema can express, against the very model the
    // `mid` tier points at.
    admin(
        &h,
        &mut rx,
        json!({"operation": "insert", "table": "incidents",
               "row": {"id": "inc-1", "model_id": "provider-a/model-mid",
                       "kind": "outage", "at": now_iso(),
                       "detail": {"note": "provider returned 503 for ten minutes"}}}),
    )
    .await;

    // 1. Nothing leaves the hive. No auto-remap, no push, no alert.
    let quiet = tokio::time::timeout(Duration::from_secs(4), rx.recv()).await;
    assert!(
        quiet.is_err(),
        "an incident row must trigger NOTHING in v1, got {:?}",
        quiet.map(|m| m.map(|m| m.headers.hop.clone()))
    );

    // 2. The tier index is untouched -- still one active row, still the model
    //    the incident was filed against.
    let tiers = admin(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "tiers",
               "columns": ["model_id", "active"], "where": {"tier": "mid"},
               "limit": 10}),
    )
    .await;
    let tiers = tiers.as_array().cloned().unwrap_or_default();
    assert_eq!(tiers.len(), 1, "{tiers:?}");
    assert_eq!(tiers[0]["model_id"].as_str(), Some("provider-a/model-mid"));
    assert_eq!(tiers[0]["active"].as_i64(), Some(1));

    // 3. And `models` is untouched too: an incident does not degrade a status.
    //    Deciding that a model is degraded is a human act, and it looks like an
    //    UPDATE somebody made on purpose.
    let models = admin(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "models",
               "columns": ["status"], "where": {"model_id": "provider-a/model-mid"},
               "limit": 5}),
    )
    .await;
    assert_eq!(models[0]["status"].as_str(), Some("active"), "{models}");

    // 4. The next lookup answers exactly as the first one did.
    let after = resolve(&h, &mut rx, json!({"tier": "mid"})).await;
    assert_eq!(
        after["model_id"], before["model_id"],
        "an incident is a line in a log, not an input to the resolver"
    );
    assert_eq!(after["reason_code"], before["reason_code"]);

    // 5. And the subscriber never heard a thing.
    assert_eq!(inference_model(&h, &mut rx, "/sub_a", &mock).await, BIRTH_A);

    h.shutdown().await;
}

/// GH #310 — the catalog store's write surface has two halves, this template
/// shipped neither, and exactly ONE of them can be closed here.
///
/// `contract.write_surface` (GH #260) bounds the `import` of the `transfer`
/// body slot, which the SUBSTRATE answers before `handle()` is ever reached. An
/// absent key means `open`, and `open` bounds nothing — `meclaw_colony`'s
/// `an_open_write_surface_bounds_no_import_at_all` is the negative pin. Without
/// it an `import` writes `models` rows in bulk, from any sender, straight past
/// the comparison that `select` IS: the rank literal, the `status` column and
/// the refusal. It is declared.
///
/// `params.write_surface` (GH #132) bounds the ops the store's own `handle()`
/// runs, and it is deliberately NOT declared — which is the second half of what
/// this test pins, because an omission that is not asserted reads as an
/// oversight. The reason is a property of the template: **no cell in this hive
/// ever writes `models`, `subscribers` or `incidents`**. `select` and `hand`
/// only select from those three; what they write is `tiers` and `resolutions`.
/// The three operator tables are maintained over a parent's BOOT-graph edge
/// straight into `./store` (the bootstrap is deliberately outside the port
/// seal's scope, `mutation::port_boundary`), and that sender lies outside the
/// hive by construction — [`build_tree`]'s own `./admin -> ./llm_registry/store`
/// edge is exactly it, and it is how `subscribers` and `incidents` get their
/// rows in the tests below. A cell-level seal would not tighten the boundary; it
/// would leave a freshly instantiated registry with no way to ever name a
/// subscriber.
///
/// The two sweeps below are what make this revisable rather than a standing
/// excuse: the day a cell in here writes `incidents` (the hand writes
/// `subscribers` since GH #855 and `models` since GH #858), the first goes red and the seal becomes possible — and the day
/// nothing in here writes `tiers`/`resolutions` any more, the second goes red
/// and the hive has stopped being its own writer at all.
#[test]
fn the_catalog_store_bounds_the_import_and_says_why_the_cell_surface_stays_open() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let store = read_json(&root.join("store/config.json"));
    assert_eq!(
        store["contract"]["write_surface"], "internal",
        "GH #260: without the substrate half an import writes catalog rows in \
         bulk past the comparison this hive is built on"
    );
    assert!(
        store["params"].get("write_surface").is_none(),
        "GH #132 stays open here on purpose: incidents have no writer inside \
         the hive, and a sealed handle() would leave the boot-edge maintenance \
         of the catalogue and the field log with no way in"
    );

    // Half one: every touch of an operator table inside the hive is a read.
    let mut reads = 0usize;
    // Half two: the hive DOES write its own two tables — which is why the
    // contract half above is true rather than merely harmless.
    let mut writes = 0usize;
    for rel in ["select/config.json", "hand/config.json"] {
        let cfg = read_json(&root.join(rel));
        let script = cfg["params"]["script_inline"].as_str().unwrap_or_default();
        for line in script.lines() {
            // GH #855: `subscribers` left this list. The hand writes it now --
            // `subscribe`, and the announcement a grown generation sends -- and
            // that still does not make the cell-level seal possible, because
            // `incidents` keeps its only writer OUTSIDE the hive. GH #858:
            // `models` left it too (`model_upsert`, `model_retire`), and the
            // seal is still not possible for the same reason -- `incidents`,
            // and a catalogue kept over the boot edge, which stays legal.
            for table in ["incidents"] {
                if line.contains(&format!("table=\"{table}\"")) {
                    assert!(
                        line.contains("\"select\""),
                        "{rel} touches `{table}` with something other than a \
                         select -- this hive now HAS an internal writer for an \
                         operator table, so params.write_surface can and should \
                         be sealed: {line}"
                    );
                    reads += 1;
                }
            }
            // The sweep above has nothing to read once no cell touches
            // `incidents` at all, so the anti-vacuity count also takes the
            // catalogue reads every op resolves against.
            if line.contains("table=\"models\"") && line.contains("\"select\"") {
                reads += 1;
            }
            for table in ["tiers", "resolutions", "subscribers", "overrides", "models"] {
                if line.contains(&format!("table=\"{table}\""))
                    && (line.contains("\"insert\"") || line.contains("\"update\""))
                {
                    writes += 1;
                }
            }
        }
    }
    assert!(reads >= 1, "the select-only sweep found nothing to check");
    assert!(
        writes >= 4,
        "no cell in this hive writes tiers or resolutions any more -- the \
         registry has stopped being its own writer"
    );
}

/// GH #310, the same boundary proved at runtime rather than at the declaration:
/// a `transfer` `import` addressed straight at the catalog store writes no row.
///
/// This is the half that makes the omission load-bearing. The slot is answered
/// by the SUBSTRATE in `cell_task`, before the `consumes` gate and before
/// `handle()` — so it walks past everything this hive is: past `select`'s rank
/// literal, past `hand`'s single op, past the actor the edge has to promote. And
/// it writes in BULK. The message below carries no sender at all, which the rule
/// treats as outside (fail-closed), and it plants two rows that matter more than
/// they look: a `subscribers` row decides who a remap reaches, and an
/// `incidents` row is the field record a human reads before moving a tier.
/// With `contract.write_surface` absent both land; with `"internal"` they are
/// refused with `write_denied` before the first row.
///
/// The evidence is the store's own content, read back over the boot-graph edge:
/// a refused import is invisible in every other way, because the reply to a
/// source message carries no `registry_origin` and therefore matches no out-edge
/// of the store.
///
/// RED receipt, taken by dropping the declaration from the shipped template and
/// running this test: `an import from outside the scope planted a subscriber --
/// the row that decides who a remap reaches: [{"cell_path":"/smuggled",
/// "tier":"strong"}]`. So this pin fails for the reason it names, and not
/// because a shape assertion happened to read `Null`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_transfer_import_from_outside_plants_no_subscriber_and_no_incident() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let mock = MockOpenAI::start(vec![]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, &format!("{}/v1", mock.base_url));
    let (h, mut rx) = boot(&td).await;

    let count = |v: &Value| v.as_array().map(|a| a.len()).unwrap_or(0);
    let read_subs = json!({"operation": "select", "table": "subscribers",
                           "columns": ["cell_path", "tier"], "limit": 50});
    let read_inc = json!({"operation": "select", "table": "incidents",
                          "columns": ["id", "model_id"], "limit": 50});
    let subs_before = admin(&h, &mut rx, read_subs.clone()).await;
    let inc_before = admin(&h, &mut rx, read_inc.clone()).await;

    for transfer in [
        json!({
            "operation": "import", "table": "subscribers", "key": ["cell_path"],
            "schema": {"cell_path": "text", "tier": "text", "pinned": "int",
                       "wired_at": "text"},
            "rows": [{"cell_path": "/smuggled", "tier": "strong", "pinned": 0,
                      "wired_at": "2026-08-15T00:00:00Z"}]
        }),
        json!({
            "operation": "import", "table": "incidents", "key": ["id"],
            "schema": {"id": "text", "model_id": "text", "kind": "text",
                       "at": "text", "detail": "json"},
            "rows": [{"id": "smuggled", "model_id": "provider-a/model-mid",
                      "kind": "outage", "at": "2026-08-15T00:00:00Z",
                      "detail": {"note": "planted"}}]
        }),
    ] {
        h.send(
            MessageBuilder::new(Path::new("/llm_registry/store"))
                .body(Body::Inline(json!({ "transfer": transfer })))
                .ttl(400)
                .build(),
        )
        .await;
    }
    // The import travels ONE hop; the read below travels two. The wait is the
    // discriminator, not the ordering: without it a green result would only mean
    // the import had not arrived yet.
    tokio::time::sleep(Duration::from_millis(600)).await;

    let subs_after = admin(&h, &mut rx, read_subs).await;
    let inc_after = admin(&h, &mut rx, read_inc).await;
    assert_eq!(
        count(&subs_after),
        count(&subs_before),
        "an import from outside the scope planted a subscriber -- the row that \
         decides who a remap reaches: {subs_after}"
    );
    assert!(
        subs_after
            .as_array()
            .is_none_or(|a| a.iter().all(|r| r["cell_path"] != "/smuggled")),
        "the planted subscriber is in the table: {subs_after}"
    );
    assert_eq!(
        count(&inc_after),
        count(&inc_before),
        "an import from outside the scope planted an incident: {inc_after}"
    );
    assert!(
        inc_after
            .as_array()
            .is_none_or(|a| a.iter().all(|r| r["id"] != "smuggled")),
        "the planted incident is in the field log: {inc_after}"
    );

    h.shutdown().await;
}

// ═════════════════════════════════════════ GH #855 -- the one way to operate models

/// The precedence, walked rule by rule on three subscribers, and read off the
/// WIRE of the push each time: targeted > global > tier > start value.
///
/// `/sub_a` runs the tier `t1`, `/sub_b` runs it too but is PINNED, `/sub_c`
/// runs no tier and only its start value. Every step names exactly which cells
/// were pushed, with which rank and which model, and a step that changes
/// nothing a cell would run pushes nothing at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_precedence_is_targeted_then_global_then_tier_then_start() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let mock = MockOpenAI::start(vec![]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, &format!("{}/v1", mock.base_url));
    let (h, mut rx) = boot(&td).await;
    catalogue_plain(&h, &mut rx, "m/a", json!({}), "").await;
    catalogue_plain(&h, &mut rx, "m/b", json!({"temperature": 0.1}), "").await;
    catalogue_plain(
        &h,
        &mut rx,
        "m/c",
        json!({"max_tokens": 1234, "reasoning_effort": "low", "no_such_key": 1}),
        "P-C",
    )
    .await;

    // 0. Three subscribers. Nothing resolves past the start value yet (`t1` has
    //    no row), so nothing is pushed: a cell already runs its start value.
    for (path, tier, pinned, start) in [
        ("/sub_a", "t1", 0, BIRTH_A),
        ("/sub_b", "t1", 1, BIRTH_B),
        ("/sub_c", "", 0, BIRTH_C),
    ] {
        let (pushes, ack) = command(
            &h,
            &mut rx,
            json!({"op": "subscribe", "cell_path": path, "start_model": start,
                   "tier": tier, "pinned": pinned}),
        )
        .await;
        assert_eq!(ack["outcome"].as_str(), Some("accepted"), "{ack}");
        assert!(pushes.is_empty(), "a start value is never pushed: {path}");
    }

    // 1. The tier gets a model: the unpinned tier subscriber moves, the pinned
    //    one is journalled as skipped, the start-value cell is not concerned.
    let (pushes, ack) = command(
        &h,
        &mut rx,
        json!({"op": "remap", "tier": "t1", "model_id": "m/a"}),
    )
    .await;
    assert_eq!(
        pushed_to(&pushes),
        vec![("/sub_a".into(), "tier".into(), "m/a".into())],
        "{ack}"
    );
    assert_eq!(ack["skipped_pinned"].as_i64(), Some(1), "{ack}");

    // 2. Global: "m/a everywhere -> m/b". It replaces the BASE model, whatever
    //    put it there; the pinned subscriber is passed over again.
    let (pushes, ack) = command(
        &h,
        &mut rx,
        json!({"op": "override_set", "scope": "global", "match": "m/a", "model_id": "m/b"}),
    )
    .await;
    assert!(ack["id"].as_str().is_some_and(|i| !i.is_empty()), "{ack}");
    assert_eq!(
        pushed_to(&pushes),
        vec![("/sub_a".into(), "global".into(), "m/b".into())],
        "{ack}"
    );
    assert_eq!(
        body_of(&pushes[0])["params"]["temperature"].as_f64(),
        Some(0.1),
        "the package's own parameters ride with the model: {}",
        body_of(&pushes[0])
    );

    // 3. Targeted, at the PINNED cell: a targeted replacement is the operator's
    //    statement about exactly this cell, so a pin does not stop it. The push
    //    is the whole package -- model, prompt block, the row's parameters -- and
    //    `$reset` for every package key the row does not set.
    let (pushes, ack) = command(
        &h,
        &mut rx,
        json!({"op": "override_set", "scope": "target", "match": "/sub_b", "model_id": "m/c"}),
    )
    .await;
    let target_id = ack["id"].as_str().unwrap_or_default().to_string();
    assert_eq!(
        pushed_to(&pushes),
        vec![("/sub_b".into(), "target".into(), "m/c".into())],
        "{ack}"
    );
    let p = &body_of(&pushes[0])["params"];
    assert_eq!(p["model_prompt"].as_str(), Some("P-C"), "{p}");
    assert_eq!(p["max_tokens"].as_i64(), Some(1234), "{p}");
    assert_eq!(p["reasoning_effort"].as_str(), Some("low"), "{p}");
    assert!(
        p.get("no_such_key").is_none(),
        "a key that is not a package key never reaches a cell: {p}"
    );
    let mut want: Vec<String> = PACKAGE_KEYS
        .iter()
        .filter(|k| !["model", "model_prompt", "max_tokens", "reasoning_effort"].contains(k))
        .map(|k| k.to_string())
        .collect();
    want.sort();
    assert_eq!(resets(&pushes[0]), want, "{p}");

    // 4. The view per cell: model, rank, reason -- one row per subscriber.
    let view = show(&h, &mut rx).await;
    assert_eq!(view["/sub_a"]["model_id"], "m/b", "{view:?}");
    assert_eq!(view["/sub_a"]["rank"], "global");
    assert_eq!(view["/sub_a"]["reason"], "override_global");
    assert_eq!(view["/sub_b"]["model_id"], "m/c");
    assert_eq!(view["/sub_b"]["rank"], "target");
    assert_eq!(view["/sub_b"]["pinned"], 1);
    assert_eq!(view["/sub_c"]["model_id"], BIRTH_C);
    assert_eq!(view["/sub_c"]["rank"], "start");
    assert!(
        view.values()
            .all(|c| !c["since"].as_str().unwrap_or_default().is_empty()),
        "every cell says since when: {view:?}"
    );

    // 5. The same replacement again changes no package: nothing is pushed.
    let (pushes, _) = command(
        &h,
        &mut rx,
        json!({"op": "override_set", "scope": "global", "match": "m/a", "model_id": "m/b"}),
    )
    .await;
    assert!(
        pushes.is_empty(),
        "a push happens only when the package changed"
    );

    // 6. The targeted one cleared: the pinned cell falls back to its START
    //    value, which is a `$reset` of every package key and nothing else.
    let (pushes, _) = command(
        &h,
        &mut rx,
        json!({"op": "override_clear", "id": target_id}),
    )
    .await;
    assert_eq!(
        pushed_to(&pushes),
        vec![("/sub_b".into(), "start".into(), "".into())]
    );
    let mut all: Vec<String> = PACKAGE_KEYS.iter().map(|k| k.to_string()).collect();
    all.sort();
    assert_eq!(resets(&pushes[0]), all);
    assert_eq!(
        body_of(&pushes[0])["params"].as_object().map(|o| o.len()),
        Some(1),
        "a fallback is the reset alone: {}",
        body_of(&pushes[0])
    );

    // 7. The global one cleared -- the ACTIVE row, which since step 5 is the
    //    newer one: a replacement set again supersedes, it never overwrites.
    //    Back onto the tier.
    let overrides = admin(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "overrides",
               "columns": ["id", "active"], "where": {"scope": "global", "active": 1},
               "limit": 5}),
    )
    .await;
    let live = overrides[0]["id"].as_str().unwrap_or_default().to_string();
    let (pushes, _) = command(&h, &mut rx, json!({"op": "override_clear", "id": live})).await;
    assert_eq!(
        pushed_to(&pushes),
        vec![("/sub_a".into(), "tier".into(), "m/a".into())]
    );

    // 8. Nothing is ever deleted: both replacements are still readable.
    let all_rows = admin(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "overrides",
               "columns": ["id", "active"], "limit": 10}),
    )
    .await;
    assert_eq!(all_rows.as_array().map(Vec::len), Some(3), "{all_rows}");
    assert!(
        all_rows
            .as_array()
            .is_some_and(|a| a.iter().all(|r| r["active"] == 0)),
        "{all_rows}"
    );

    h.shutdown().await;
}

/// The refusals of the new ops. Each is acknowledged by name and changes
/// nothing: a replacement onto a model the catalogue does not carry, a
/// targeted one that reaches no subscriber, an empty match, a clear of an id
/// nobody set, a reset of a path that is no subscriber.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_override_the_registry_cannot_stand_behind_is_refused_by_name() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let mock = MockOpenAI::start(vec![]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, &format!("{}/v1", mock.base_url));
    let (h, mut rx) = boot(&td).await;
    catalogue_plain(&h, &mut rx, "m/a", json!({}), "").await;
    let _ = command(
        &h,
        &mut rx,
        json!({"op": "subscribe", "cell_path": "/sub_a", "start_model": BIRTH_A}),
    )
    .await;

    for (cmd, code) in [
        (
            json!({"op": "override_set", "scope": "global", "match": BIRTH_A,
                   "model_id": "provider-b/model-legacy"}),
            "unknown_model",
        ),
        (
            json!({"op": "override_set", "scope": "target", "match": "/nobody",
                   "model_id": "m/a"}),
            "no_subscriber",
        ),
        (
            json!({"op": "override_set", "scope": "target", "match": " ",
                   "model_id": "m/a"}),
            "incomplete_command",
        ),
        (
            json!({"op": "override_set", "scope": "everywhere", "match": BIRTH_A,
                   "model_id": "m/a"}),
            "incomplete_command",
        ),
        (
            json!({"op": "override_clear", "id": "nope"}),
            "unknown_override",
        ),
        (
            json!({"op": "reset", "cell_path": "/nobody"}),
            "no_subscriber",
        ),
        (
            json!({"op": "subscribe", "cell_path": "/sub_x"}),
            "incomplete_command",
        ),
        (
            json!({"op": "override_set", "scope": "global", "match": BIRTH_A,
                   "model_id": "m/a", "_no_actor": true}),
            "no_actor",
        ),
    ] {
        let (pushes, ack) = command(&h, &mut rx, cmd.clone()).await;
        assert_eq!(ack["outcome"].as_str(), Some("rejected"), "{cmd}: {ack}");
        assert_eq!(ack["reason_code"].as_str(), Some(code), "{cmd}: {ack}");
        assert!(pushes.is_empty(), "{cmd}: a refusal pushes nothing");
    }
    let rows = admin(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "overrides", "columns": ["id"], "limit": 5}),
    )
    .await;
    assert_eq!(
        rows.as_array().map(Vec::len),
        Some(0),
        "no refusal wrote a replacement: {rows}"
    );
    let view = show(&h, &mut rx).await;
    assert_eq!(view["/sub_a"]["rank"], "start", "{view:?}");

    h.shutdown().await;
}

/// `reset` puts ONE cell back on what it resolves to without its own targeted
/// replacements -- and only its own: a prefix replacement that also covers a
/// sibling stays, because a reset of one cell is not a statement about another.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reset_clears_the_targeted_replacements_of_one_cell() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let mock = MockOpenAI::start(vec![]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, &format!("{}/v1", mock.base_url));
    let (h, mut rx) = boot(&td).await;
    catalogue_plain(&h, &mut rx, "m/a", json!({}), "").await;
    catalogue_plain(&h, &mut rx, "m/b", json!({}), "").await;
    for (path, start) in [("/g/talky/brain", BIRTH_A), ("/g/cogny/brain", BIRTH_B)] {
        let _ = command(
            &h,
            &mut rx,
            json!({"op": "subscribe", "cell_path": path, "start_model": start}),
        )
        .await;
    }
    // A prefix reaches both cells of the generation ...
    let (pushes, _) = command(
        &h,
        &mut rx,
        json!({"op": "override_set", "scope": "target", "match": "/g", "model_id": "m/a"}),
    )
    .await;
    assert_eq!(pushes.len(), 2, "a prefix reaches every cell under it");
    // ... and the longer prefix wins for the one it names.
    let (pushes, _) = command(
        &h,
        &mut rx,
        json!({"op": "override_set", "scope": "target", "match": "/g/talky/brain",
               "model_id": "m/b"}),
    )
    .await;
    assert_eq!(
        pushed_to(&pushes),
        vec![("/g/talky/brain".into(), "target".into(), "m/b".into())]
    );
    let (pushes, ack) = command(
        &h,
        &mut rx,
        json!({"op": "reset", "cell_path": "/g/talky/brain"}),
    )
    .await;
    assert_eq!(ack["cleared"].as_i64(), Some(1), "{ack}");
    assert_eq!(
        pushed_to(&pushes),
        vec![("/g/talky/brain".into(), "target".into(), "m/a".into())],
        "the generation-wide replacement still holds for it"
    );
    h.shutdown().await;
}

/// The tree announces its brains (GH #855): a message with NO turn, the
/// brains on `hop.subscribe`, over an edge that stamps the announcer and the
/// generation it was drawn for as context. It makes a subscriber of each new
/// one, answers nobody, and a second identical announcement -- the next
/// mutation receipt -- changes nothing, journals nothing and writes no second
/// row. A brain announced while a global replacement covers its start value is
/// pushed at once, because the edge that carries the push exists already: the
/// announcement is fired BY that tree, after the commit that grew it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_generation_announces_its_brains_and_becomes_a_subscriber() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let mock = MockOpenAI::start(vec![]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, &format!("{}/v1", mock.base_url));
    let (h, mut rx) = boot(&td).await;
    catalogue_plain(&h, &mut rx, "m/b", json!({}), "").await;
    // A global replacement for the start value of one of the two brains. With
    // no subscriber yet it reaches nobody.
    let (pushes, _) = command(
        &h,
        &mut rx,
        json!({"op": "override_set", "scope": "global", "match": BIRTH_B, "model_id": "m/b"}),
    )
    .await;
    assert!(pushes.is_empty());

    let announcement = json!({"generation": "/g", "brains": [
        {"cell_path": "/g/talky/brain", "start_model": BIRTH_A},
        {"cell_path": "/g/cogny/brain", "start_model": BIRTH_B}
    ]});
    h.send(to("/tree", &announcement.to_string())).await;
    let pushed = recv_route(&mut rx, "update").await;
    assert_eq!(hop_of(&pushed, "subscriber"), "/g/cogny/brain");
    assert_eq!(hop_of(&pushed, "rank"), "global");
    let quiet = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await;
    assert!(
        quiet.is_err(),
        "an announcement is answered by nobody and pushes only what moved: {:?}",
        quiet.map(|m| m.map(|m| m.headers.hop.clone()))
    );
    let view = show(&h, &mut rx).await;
    assert_eq!(view.len(), 2, "{view:?}");
    assert_eq!(view["/g/talky/brain"]["rank"], "start");
    assert_eq!(view["/g/talky/brain"]["start_model"], BIRTH_A);
    assert_eq!(view["/g/cogny/brain"]["model_id"], "m/b");

    // The next receipt says the same thing again: nothing moves, nothing is
    // journalled twice, and no second row appears (review I-2).
    h.send(to("/tree", &announcement.to_string())).await;
    let quiet = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await;
    assert!(quiet.is_err(), "a known brain announced again is nothing");
    let rows = admin(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "resolutions",
               "columns": ["cell_path", "reason"],
               "where": {"reason": "hand_subscribed"}, "limit": 10}),
    )
    .await;
    assert_eq!(rows.as_array().map(Vec::len), Some(2), "{rows}");
    assert_eq!(subscriber_paths(&h, &mut rx).await.len(), 2);

    h.shutdown().await;
}

/// Every subscriber row, as its path -- duplicates included, which is the point.
async fn subscriber_paths(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>) -> Vec<String> {
    let rows = admin(
        h,
        rx,
        json!({"operation": "select", "table": "subscribers", "columns": ["cell_path"],
               "order_by": [{"col": "cell_path", "dir": "asc"}], "limit": 1000}),
    )
    .await;
    rows.as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|r| r["cell_path"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The journal lines with this reason, polled until `want` of them are there.
async fn journal_until(
    h: &ColonyHandle,
    rx: &mut mpsc::Receiver<Message>,
    reason: &str,
    want: usize,
) -> Vec<Value> {
    let mut got = Vec::new();
    for _ in 0..60 {
        let rows = admin(
            h,
            rx,
            json!({"operation": "select", "table": "resolutions",
                   "columns": ["cell_path", "reason", "model_id"],
                   "where": {"reason": reason}, "limit": 100}),
        )
        .await;
        got = rows.as_array().cloned().unwrap_or_default();
        if got.len() >= want {
            return got;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("the journal never carried {want} line(s) `{reason}`: {got:?}");
}

/// The view, polled until `path` is in it.
async fn view_with(
    h: &ColonyHandle,
    rx: &mut mpsc::Receiver<Message>,
    path: &str,
) -> std::collections::BTreeMap<String, Value> {
    for _ in 0..60 {
        let view = show(h, rx).await;
        if view.contains_key(path) {
            return view;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("{path} never became a subscriber");
}

/// Review I-1 (iii): an announcement speaks only for the generation its edge
/// was drawn for. The builder stamps that generation into
/// `context.model_generation` -- edge truth, and the submit gate lets exactly
/// that form through -- so a brain outside it is neither made a subscriber nor
/// has its start value rewritten; the generation's own road may correct it,
/// because that road IS what the brain was born on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_announcement_speaks_only_for_its_own_generation() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let mock = MockOpenAI::start(vec![]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, &format!("{}/v1", mock.base_url));
    let (h, mut rx) = boot(&td).await;
    let tree = |generation: &str, brains: Value| {
        json!({"generation": generation, "brains": brains}).to_string()
    };

    h.send(to(
        "/tree",
        &tree(
            "/g",
            json!([{"cell_path": "/g/talky/brain", "start_model": BIRTH_A}]),
        ),
    ))
    .await;
    let _ = view_with(&h, &mut rx, "/g/talky/brain").await;

    // Another generation's road names the first one's brain with another start
    // value, beside a brain of its own.
    h.send(to(
        "/tree",
        &tree(
            "/h",
            json!([{"cell_path": "/g/talky/brain", "start_model": "forged/model"},
                   {"cell_path": "/h/talky/brain", "start_model": BIRTH_C}]),
        ),
    ))
    .await;
    let view = view_with(&h, &mut rx, "/h/talky/brain").await;
    assert_eq!(
        view["/g/talky/brain"]["start_model"], BIRTH_A,
        "a foreign generation never rewrites a start value: {view:?}"
    );
    // Segments, never a string prefix: `/g` does not cover `/g2`.
    h.send(to(
        "/tree",
        &tree(
            "/g",
            json!([{"cell_path": "/g2/talky/brain", "start_model": "x"},
                   {"cell_path": "/g/cogny/brain", "start_model": BIRTH_B}]),
        ),
    ))
    .await;
    let view = view_with(&h, &mut rx, "/g/cogny/brain").await;
    assert!(!view.contains_key("/g2/talky/brain"), "{view:?}");
    let outside = journal_until(&h, &mut rx, "hand_announce_outside_generation", 2).await;
    assert_eq!(outside.len(), 2, "{outside:?}");

    // The generation's own road corrects its own brain.
    h.send(to(
        "/tree",
        &tree(
            "/g",
            json!([{"cell_path": "/g/talky/brain", "start_model": BIRTH_B}]),
        ),
    ))
    .await;
    let mut corrected = false;
    for _ in 0..60 {
        if show(&h, &mut rx).await["/g/talky/brain"]["start_model"] == BIRTH_B {
            corrected = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(corrected, "the generation's own road may correct its brain");

    // No generation on the edge: nothing at all.
    h.send(to(
        "/tree",
        &tree(
            "",
            json!([{"cell_path": "/z/talky/brain", "start_model": "x"}]),
        ),
    ))
    .await;
    let _ = journal_until(&h, &mut rx, "no_generation", 1).await;
    assert!(!show(&h, &mut rx).await.contains_key("/z/talky/brain"));
    assert_eq!(subscriber_paths(&h, &mut rx).await.len(), 3);

    h.shutdown().await;
}

/// GH #858, review rev-L2b2 m-2: an announcement that states NO start value
/// never clears one the registry holds. `meclaw-os` announces its own judge
/// with an empty start value -- it substitutes nothing in its own values
/// (`gh302`), and a token written there is not bound when the shell is grown
/// by a mutation (measured in fix round 1: the row read
/// `${ARGUS_JUDGE_MODEL:-…}` literally). So the start value an operator states
/// with `subscribe` must survive every later mutation receipt, or `show` would
/// print an empty `model_id` again at the next commit. An announcement that
/// does name a start value still corrects it: that one is what the tree knows.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_announcement_with_no_start_value_keeps_the_one_the_registry_holds() {
    let Some(root) = shipped_registry() else {
        return;
    };
    // The need is asked once; the translator's answer is a catalogue row, and
    // the spare answers only keep a second question from failing the mock.
    let mock = MockOpenAI::start(
        (0..6)
            .map(|_| {
                canned_chat_completion(
                    &json!({"model_id": "provider-a/model-mid", "reason": "a careful judge"})
                        .to_string(),
                    "stop",
                )
            })
            .collect(),
    )
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, &format!("{}/v1", mock.base_url));
    let (h, mut rx) = boot(&td).await;
    const JUDGE: &str = "/g/argus/judge";
    const NEED: &str = "Judges a proposed change to the colony; careful reasoning over speed \
                        and price.";
    let announce = |start: &str, beside: &str| {
        json!({"generation": "/g", "brains": [
            {"cell_path": JUDGE, "start_model": start, "requirement": NEED},
            {"cell_path": beside, "start_model": BIRTH_B}
        ]})
        .to_string()
    };

    // The boot's receipt: the judge becomes a subscriber with no start value.
    h.send(to("/tree", &announce("", "/g/a/brain"))).await;
    let view = view_with(&h, &mut rx, "/g/a/brain").await;
    assert_eq!(view[JUDGE]["start_model"], "", "{view:?}");

    // The operator states the value the judge is born on.
    let _ = command(
        &h,
        &mut rx,
        json!({"op": "subscribe", "cell_path": JUDGE, "start_model": BIRTH_A,
               "requirement": NEED}),
    )
    .await;
    let mut stated = false;
    for _ in 0..60 {
        if show(&h, &mut rx).await[JUDGE]["start_model"] == BIRTH_A {
            stated = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(stated, "the operator's subscribe states the start value");

    // The next receipt announces the judge with no start value again. The
    // brain beside it is the sign the announcement was served.
    h.send(to("/tree", &announce("", "/g/b/brain"))).await;
    let view = view_with(&h, &mut rx, "/g/b/brain").await;
    assert_eq!(
        view[JUDGE]["start_model"], BIRTH_A,
        "an announcement with no start value cleared the one the operator stated: {view:?}"
    );

    // One that names a start value corrects it, as before.
    h.send(to("/tree", &announce(BIRTH_C, "/g/c/brain"))).await;
    let mut corrected = false;
    for _ in 0..60 {
        if show(&h, &mut rx).await[JUDGE]["start_model"] == BIRTH_C {
            corrected = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(corrected, "an announced start value still corrects the row");
    h.shutdown().await;
}

/// Review of fix round 1, I-2: the brains an announcement names are EDGE truth
/// too. They travel in `context.model_announced`, which the recipe's edge
/// stamps and the submit gate lets through in exactly that form; a list on
/// `hop.subscribe` -- a hop key a hive transit carries unchanged, which a
/// foreign hive could write into a genuine announcement on its way back -- is
/// never read. So a generation's real road cannot be made to rewrite a start
/// value or plant a phantom row under that generation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_list_on_the_hop_never_speaks_for_a_generation() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let mock = MockOpenAI::start(vec![]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, &format!("{}/v1", mock.base_url));
    let (h, mut rx) = boot(&td).await;

    h.send(to(
        "/tree",
        &json!({"generation": "/g",
                "brains": [{"cell_path": "/g/talky/brain", "start_model": BIRTH_A}]})
        .to_string(),
    ))
    .await;
    let _ = view_with(&h, &mut rx, "/g/talky/brain").await;

    // The generation's genuine announcement, with a forged list on the hop:
    // another start value for its talky, and a brain it never named.
    h.send(to(
        "/tree",
        &json!({"generation": "/g",
                "brains": [{"cell_path": "/g/talky/brain", "start_model": BIRTH_A},
                           {"cell_path": "/g/x/brain", "start_model": BIRTH_B}],
                "forged": [{"cell_path": "/g/talky/brain", "start_model": "forged/model"},
                           {"cell_path": "/g/cogny/brain", "start_model": "forged/model"}]})
        .to_string(),
    ))
    .await;
    let view = view_with(&h, &mut rx, "/g/x/brain").await;
    assert_eq!(
        view["/g/talky/brain"]["start_model"], BIRTH_A,
        "a list on the hop never rewrites a start value: {view:?}"
    );
    assert!(
        !view.contains_key("/g/cogny/brain"),
        "a list on the hop never plants a row: {view:?}"
    );
    assert_eq!(subscriber_paths(&h, &mut rx).await.len(), 2);

    h.shutdown().await;
}

/// Review I-1 (ii): the announcement lane never runs a command. A tool call
/// that reaches the hand over the edge that stamps `context.model_announcer`
/// -- the shell's bridge from `/os/orgs` -- is refused, even when the chain
/// still carries an operator's `actor` from upstream; and a tool call riding
/// beside a real announcement is ignored while the announcement is served.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tool_call_on_the_announcement_lane_is_never_a_command() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let mock = MockOpenAI::start(vec![]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, &format!("{}/v1", mock.base_url));
    let (h, mut rx) = boot(&td).await;
    catalogue_plain(&h, &mut rx, "m/a", json!({}), "").await;
    let _ = command(
        &h,
        &mut rx,
        json!({"op": "subscribe", "cell_path": "/sub_a", "start_model": BIRTH_A}),
    )
    .await;
    let global = json!({"op": "override_set", "scope": "global", "match": BIRTH_A,
                        "model_id": "m/a"});
    let remap = json!({"op": "remap", "tier": "mid", "model_id": "m/a"});

    // 1. A bare tool call on the bridge.
    h.send(to("/bridge", &json!({"cmd": global}).to_string()))
        .await;
    h.send(to("/bridge", &json!({"cmd": remap}).to_string()))
        .await;
    let _ = journal_until(&h, &mut rx, "announcer_carries_no_announcement", 2).await;
    // 2. The same call with a list on `hop.subscribe` beside it: an
    //    announcement travels in the context the recipe's edge stamps, never
    //    on a hop key (fix round 2), so this one carries none -- refused as an
    //    announcement, never run.
    h.send(to(
        "/bridge",
        &json!({"cmd": global,
                "subscribe": json!([{"cell_path": "/sub_a", "start_model": "x"}]).to_string()})
        .to_string(),
    ))
    .await;
    let _ = journal_until(&h, &mut rx, "announcer_carries_no_announcement", 3).await;
    // 3. A tool call beside a REAL announcement: the announcement is served,
    //    the call is not.
    h.send(to(
        "/tree",
        &json!({"generation": "/g",
                "brains": [{"cell_path": "/g/talky/brain", "start_model": BIRTH_A}],
                "turns": [{"origin": "assistant", "type": "tool_call", "id": "t1",
                           "text": global.to_string()}]})
        .to_string(),
    ))
    .await;
    let _ = view_with(&h, &mut rx, "/g/talky/brain").await;

    let overrides = admin(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "overrides", "columns": ["id"], "limit": 5}),
    )
    .await;
    assert_eq!(
        overrides.as_array().map(Vec::len),
        Some(0),
        "no replacement came in through the announcement lane: {overrides}"
    );
    let tiers = admin(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "tiers", "columns": ["model_id"],
               "where": {"tier": "mid", "active": 1}, "limit": 5}),
    )
    .await;
    assert!(
        tiers
            .as_array()
            .is_some_and(|t| t.iter().all(|r| r["model_id"] != "m/a")),
        "no tier moved through the announcement lane: {tiers}"
    );
    let view = show(&h, &mut rx).await;
    assert_eq!(view["/sub_a"]["rank"], "start", "{view:?}");
    assert_eq!(view["/sub_a"]["start_model"], BIRTH_A, "{view:?}");

    h.shutdown().await;
}

/// Review I-2: `subscriber_rows` bounds the WORK of one hop, not the number of
/// subscribers. An op that meets the bound pages on until the read comes back
/// short, so a tier move or a replacement reaches every subscriber; a
/// subscription reads its own paths exactly, so a subscriber past the bound is
/// found and never inserted twice; the view says when it was cut. And `show`,
/// which changes nothing, needs no actor (review M-4).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_op_reaches_every_subscriber_past_the_page_bound() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let mock = MockOpenAI::start(vec![]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, &format!("{}/v1", mock.base_url));
    let hand = td.path().join("main/llm_registry/hand/config.json");
    let mut cfg = read_json(&hand);
    cfg["params"]["subscriber_rows"] = json!(2);
    std::fs::write(
        &hand,
        meclaw_core::serde_json::to_string_pretty(&cfg).unwrap(),
    )
    .unwrap();
    let (h, mut rx) = boot(&td).await;
    catalogue_plain(&h, &mut rx, "m/a", json!({}), "").await;
    catalogue_plain(&h, &mut rx, "m/b", json!({}), "").await;
    let paths: Vec<String> = (0..5).map(|i| format!("/p/{i}/brain")).collect();
    for p in &paths {
        let (_, ack) = command(
            &h,
            &mut rx,
            json!({"op": "subscribe", "cell_path": p, "start_model": BIRTH_A, "tier": "t5"}),
        )
        .await;
        assert_eq!(ack["outcome"], "accepted", "{ack}");
    }
    // Past the bound, subscribing again is an update of the row it finds.
    let (_, ack) = command(
        &h,
        &mut rx,
        json!({"op": "subscribe", "cell_path": paths[4], "start_model": BIRTH_A, "tier": "t5"}),
    )
    .await;
    assert_eq!(ack["outcome"], "accepted", "{ack}");
    assert_eq!(
        subscriber_paths(&h, &mut rx).await,
        paths,
        "one row per subscriber, however far past the page bound it sits"
    );

    let (pushes, ack) = command(
        &h,
        &mut rx,
        json!({"op": "remap", "tier": "t5", "model_id": "m/a"}),
    )
    .await;
    assert_eq!(
        pushes.len(),
        5,
        "a remap reaches every subscriber of the tier: {ack}"
    );
    assert_eq!(ack["pushed"].as_i64(), Some(5), "{ack}");
    assert_eq!(ack["subscribers"].as_i64(), Some(5), "{ack}");

    let (pushes, ack) = command(
        &h,
        &mut rx,
        json!({"op": "override_set", "scope": "global", "match": "m/a", "model_id": "m/b"}),
    )
    .await;
    assert_eq!(
        pushes.len(),
        5,
        "a replacement reaches every subscriber: {ack}"
    );
    let id = ack["id"].as_str().unwrap_or_default().to_string();
    let (pushes, _) = command(&h, &mut rx, json!({"op": "override_clear", "id": id})).await;
    assert_eq!(pushes.len(), 5, "and so does clearing it");
    assert!(
        pushed_to(&pushes).iter().all(|(_, _, m)| m == "m/a"),
        "back on the tier: {:?}",
        pushed_to(&pushes)
    );

    // The view is one page and says so -- asked without an actor.
    h.send(to("/operator", r#"{"op":"show","_no_actor":true}"#))
        .await;
    let v = turn_json(&recv_route(&mut rx, "answer").await);
    assert_eq!(v["cells"].as_array().map(Vec::len), Some(2), "{v}");
    assert_eq!(v["truncated"], true, "{v}");

    h.shutdown().await;
}

/// Review of fix round 1, m-2: `show` with a `cell_path` narrows to exactly
/// that path and the paths below it. The read used to be `cell_path >= <path>`,
/// and `-` sorts before `/`: siblings like `<path>-1/...` filled the page
/// before `<path>/...` came, so the view answered nothing and `truncated`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn show_with_a_path_narrows_to_that_path_and_below() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let mock = MockOpenAI::start(vec![]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, &format!("{}/v1", mock.base_url));
    let hand = td.path().join("main/llm_registry/hand/config.json");
    let mut cfg = read_json(&hand);
    cfg["params"]["subscriber_rows"] = json!(2);
    std::fs::write(
        &hand,
        meclaw_core::serde_json::to_string_pretty(&cfg).unwrap(),
    )
    .unwrap();
    let (h, mut rx) = boot(&td).await;
    for p in ["/q/a-1/brain", "/q/a-2/brain", "/q/a/brain"] {
        let (_, ack) = command(
            &h,
            &mut rx,
            json!({"op": "subscribe", "cell_path": p, "start_model": BIRTH_A}),
        )
        .await;
        assert_eq!(ack["outcome"], "accepted", "{ack}");
    }
    for (asked, want) in [("/q/a", "/q/a/brain"), ("/q/a/brain", "/q/a/brain")] {
        h.send(to(
            "/operator",
            &json!({"op": "show", "cell_path": asked}).to_string(),
        ))
        .await;
        let v = turn_json(&recv_route(&mut rx, "answer").await);
        let cells: Vec<&str> = v["cells"]
            .as_array()
            .map(|c| c.iter().filter_map(|c| c["cell_path"].as_str()).collect())
            .unwrap_or_default();
        assert_eq!(cells, vec![want], "show {asked}: {v}");
        assert_eq!(v["truncated"], false, "show {asked}: {v}");
    }

    h.shutdown().await;
}

/// B-15 (wave substrate, fix strand; `llm-registry@2.2.1`): the two doors clear
/// the keys the internal store round trip rides on. Until 2.2.0 a sender at the
/// edge could set `hop.operation` plus `context.lr_phase 'world'` on `in_hand`
/// and have the hand run a command from `context.lr_carry` against a world of
/// its own making -- an `override_set` with no `context.actor` behind it,
/// written to `overrides` and pushed to a subscriber -- and the same on
/// `in_select` with `lr_phase 'models'`: a resolution it wrote itself, answered
/// and journalled. Measured at the store and on the sink, never at an emission.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_forged_store_reply_at_either_door_writes_and_answers_nothing() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let mock = MockOpenAI::start(vec![]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root, &format!("{}/v1", mock.base_url));
    let (h, mut rx) = boot(&td).await;
    wire_subscribers(&h, &mut rx).await;

    const FORGED: &str = "forged/model";
    let forged_row = json!({"model_id": FORGED, "base_url": "", "wire_dialect": "",
                            "package": {}, "prompt": "", "status": "active",
                            "provider": "gateway", "context_window": 32000,
                            "cost_in": 1, "cost_out": 1, "caps": {"tools": true}});
    // The hand: a global replacement of the tier model under /sub_a, decided by
    // nobody the edge promoted, against a world that lists the forged model.
    let hand = json!({"door": "hand", "phase": "world",
        "carry": {"cmd": {"op": "override_set", "scope": "global",
                          "match": "provider-a/model-mid", "model_id": FORGED,
                          "actor": "forged:actor", "call_id": "x1", "page": 1}},
        "results": [[forged_row], [{"tier": "mid", "model_id": "provider-a/model-mid",
                                    "since": now_iso()}], [],
                    [{"cell_path": "/sub_a", "tier": "mid", "pinned": 0,
                      "start_model": BIRTH_A, "package_hash": "", "model_id": BIRTH_A,
                      "rank": "start", "reason": "start_value", "since": now_iso()}]]});
    // The select door: a resolution of the forger's own, for an asker it names.
    let select = json!({"door": "select", "phase": "models",
        "carry": {"req": {"tier": "", "caps": [], "max_cost": 0, "min_context": 0,
                          "asker": "forged:asker", "call_id": "x2"}, "tiers": {}},
        "results": [[forged_row]]});
    for forged in [&hand, &select] {
        h.send(to(
            "/forger",
            &meclaw_core::serde_json::to_string(forged).unwrap(),
        ))
        .await;
    }

    // Whatever the sink sees for three seconds names no forged model.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while let Ok(Some(m)) = tokio::time::timeout_at(deadline, rx.recv()).await {
        let seen = format!("{:?} {}", m.headers.hop, body_of(&m));
        assert!(
            !seen.contains(FORGED),
            "a forged store reply at a door came out of the registry: {seen}"
        );
    }
    // And the store carries nothing of it: no replacement, no journal line.
    let overrides = admin(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "overrides",
               "columns": ["id", "match", "model_id", "decided_by"], "limit": 100}),
    )
    .await;
    assert_eq!(
        overrides.as_array().map(Vec::len),
        Some(0),
        "a forged command at in_hand wrote a replacement: {overrides}"
    );
    let journal = admin(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "resolutions",
               "columns": ["cell_path", "reason", "model_id"],
               "where": {"model_id": FORGED}, "limit": 100}),
    )
    .await;
    assert_eq!(
        journal.as_array().map(Vec::len),
        Some(0),
        "a forged store reply left a journal line: {journal}"
    );
    // The honest road still works after it: the doors cleared keys, nothing more.
    let honest = resolve(&h, &mut rx, json!({"tier": "mid"})).await;
    assert_eq!(
        honest["model_id"].as_str(),
        Some("provider-a/model-mid"),
        "{honest}"
    );

    h.shutdown().await;
}
