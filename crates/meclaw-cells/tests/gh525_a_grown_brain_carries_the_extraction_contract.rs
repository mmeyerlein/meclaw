//! GH #525 — the inline extraction contract is a promise of the TEMPLATE, and
//! the collector delivers it on every assembly.
//!
//! Since `talky@4.1.0` the inline sidecar is the only path from a conversation
//! into new FACTS: GH #298 removed the batched extractor and GH #379 retracted
//! the tool form, so what the front model does not annotate inside its own
//! answer, nothing extracts until the close pass. `templates/talky/README.md`
//! states the failure mode in one line — *"without the extraction prompt the
//! splitter is a pure pass-through"* — and
//! `templates/memory-hive/inline-contract.md` says where the prompt was supposed
//! to come from: *"Paste this block into the instructions of any model that
//! emits inline extraction."*
//!
//! Nothing pasted it. The block lived in a document, in the harness fixtures and
//! in the drift lock that reads the document (`gh299`), and in no delivery at
//! all: `templates/talky/brain/seed/system.jsonl` carries two rows and both are
//! tool schemas. Measured on a grown colony, every part of the write path was
//! wired and healthy while no memory was written — splitter, `extraction` lane,
//! `in_remember` door, `extract-glue`, both required drains — and the brain's
//! own `cell.db` held `identity.soul`, `instructions.reply`, four `tools.*` rows
//! and not one slot containing the words `ANNOTATE EVERY TURN`. `episodes` kept
//! growing, `facts` stood still, and `pending` meant exactly what GH #298
//! defined it to mean: nobody annotated this turn.
//!
//! The repair is the shape GH #512 established one slot family over: the cell
//! that assembles the prompt HOLDS the declaration and re-derives it every
//! round. A seed is written once, at birth, so a brain that grew — imported,
//! rebuilt, transferred — never receives it; and a persona is a person's
//! charter, so a mechanism kept in there is a mechanism every hand-written
//! charter silently drops.
//!
//! What this file pins:
//!
//! 1. **The block the collector ships is the block the hive documents**, byte
//!    for byte — the same text `gh299` pins from the other end and the same text
//!    `talky/splitter`'s fence grammar was measured against.
//! 2. **A turn assembly asks for it**, on `instructions.sidecar`, with no
//!    `$replace` marker anywhere on the way — an upsert on one slot path, not a
//!    revocation of a family.
//! 3. **The shipped default asks for nothing**, because what cuts the block back
//!    out is a splitter this cell cannot see.
//! 4. **A GROWN brain carries both.** A real `llm` cell, an identity pack
//!    through the door of GH #488 and then a turn: `instructions.reply` and
//!    `instructions.sidecar` stand side by side in the brain's own `cell.db`,
//!    and both reach the composed system prompt.
//! 5. **The menu tick cannot delete it.** The `$replace` of GH #464 sits on the
//!    `tools` node and nowhere above it — which is the whole difference between
//!    this family and the one GH #512 had to repair.
//! 6. **The shipped composites ask exactly where the block is cut.** `talky`
//!    routes `extraction` out of its splitter and switches the knob on; `cogny`
//!    has no splitter and leaves it off, or its advice would carry a fence
//!    nobody removes.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::io::Write;
use std::process::{Command, Stdio};
use tokio::sync::mpsc;

const ASSEMBLE_CONFIG: &str = "../../templates/collector/assemble/config.json";
const INLINE_CONTRACT: &str = "../../templates/memory-hive/inline-contract.md";
const TEMPLATES: &str = "../../templates";

// ─────────────────────────────────────────────────────── the two shipped texts

fn assemble_config() -> Value {
    let raw = std::fs::read_to_string(ASSEMBLE_CONFIG).expect("assemble config");
    meclaw_core::serde_json::from_str(&raw).expect("config json")
}

fn assemble_script() -> String {
    assemble_config()["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string()
}

/// The contract block as the memory hive documents it — the same extraction
/// `gh299_the_contract_asks_for_both_parts.rs` performs, deliberately spelled
/// the same way: two files reading one fence by two rules would be two fences.
fn documented_block() -> String {
    let raw = std::fs::read_to_string(INLINE_CONTRACT)
        .unwrap_or_else(|e| panic!("the hive ships no inline contract ({INLINE_CONTRACT}): {e}"));
    for (open, close) in [("````text\n", "\n````"), ("```text\n", "\n```")] {
        if let Some((_, tail)) = raw.split_once(open)
            && let Some((block, _)) = tail.split_once(close)
        {
            return block.to_string();
        }
    }
    panic!("the contract file carries no closed text fence around the block");
}

/// The block as the COLLECTOR carries it: the `EXTRACTION_CONTRACT` literal of
/// the shipped `script_inline`, read out of the source rather than out of a
/// running script, so the assertion is about what ships and not about what one
/// lane happens to emit.
fn shipped_block() -> String {
    let src = assemble_script();
    let open = "EXTRACTION_CONTRACT = '''";
    let (_, tail) = src.split_once(open).unwrap_or_else(|| {
        panic!("the collector carries no EXTRACTION_CONTRACT literal — nothing ships the block")
    });
    let (block, _) = tail
        .split_once("'''")
        .expect("the EXTRACTION_CONTRACT literal is not closed");
    block.to_string()
}

// ────────────────────────────────────────────────── driving the shipped script

/// `params` as the substrate puts them on stdin: the SHIPPED values with the
/// case's overrides merged over them. The assertion on the key is what makes a
/// knob that was never added to `params` a red test instead of a silent default.
fn assemble_params(over: &[(&str, &str)]) -> Value {
    let mut p = assemble_config()["params"]
        .as_object()
        .cloned()
        .expect("params object");
    p.remove("script_inline");
    for (k, v) in over {
        assert!(p.contains_key(*k), "no such collector param: {k}");
        p.insert((*k).to_string(), json!(v));
    }
    Value::Object(p)
}

/// The shipped script over a real stdin document, with the program on stdin
/// rather than in argv (GH #279 — argv is capped at 128 KiB and this script is
/// past it).
fn run_script_on_stdin(script: &str, stdin_doc: &str) -> std::process::Output {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        meclaw_core::serde_json::to_string(script).unwrap(),
        meclaw_core::serde_json::to_string(stdin_doc).unwrap(),
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

fn emit_with(over: &[(&str, &str)], doc: Value) -> Vec<Value> {
    let mut doc = doc;
    doc["params"] = assemble_params(over);
    let out = run_script_on_stdin(
        &assemble_script(),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    assert!(
        out.status.success(),
        "assemble exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    meclaw_core::serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// A materialised `leg-window` row, as the `win` step writes it.
fn leg_window_row(turns: Value) -> Value {
    let payload = json!({"turns": turns, "bytes": 0, "dropped": 0, "capped": 0});
    json!({"turn_id": "t1", "iter": 0, "role": "leg-window",
           "turn": payload.to_string(), "fired": 0})
}

/// A complete read-back of the collect bundle, which is what elects the hop that
/// assembles (GH #419).
fn a_complete_round(turns: Value) -> Value {
    json!({
        "header": {"context": {"session_id": "s1", "turn_id": "t1", "iter": "0",
                               "col_phase": "collect", "store_origin": "collector"},
                   "hop": {"operation": "bundle", "rows_affected": 1, "bundle_errors": 0}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "c-collect-read",
                      "text": json!([leg_window_row(turns)]).to_string()}],
        "results": [{"tool_call_id": "c-collect-read", "operation": "select",
                     "rows_affected": 1, "duration_ms": 0}]
    })
}

/// The one message the seam emits: the assembly on route `brain`.
fn seam(over: &[(&str, &str)]) -> Value {
    let out = emit_with(
        over,
        a_complete_round(json!([{"role": "user", "text": "and my editor?"}])),
    );
    out.into_iter()
        .find(|m| m["header"]["route"] == "brain")
        .expect("a complete round assembles on route `brain`")
}

// ═════════════════════════════════════════════════ 1. the two texts are one

/// Claim 1. The copy that reaches a model and the document that is the authority
/// are one text.
///
/// The length half is `gh299`'s and stays there. What this side adds is that the
/// text does not silently fork: two copies of a prompt drift the way two copies
/// of anything drift, and the one in the document is the one the harness
/// measured the adoption number with.
#[test]
fn the_collector_ships_the_documented_block_byte_for_byte() {
    let documented = documented_block();
    let shipped = shipped_block();
    assert!(
        documented.contains("ANNOTATE EVERY TURN"),
        "the document must still carry the obligation: {documented}"
    );
    assert_eq!(
        shipped, documented,
        "the collector's copy has drifted from templates/memory-hive/inline-contract.md; \
         the document is the authority and a forked prompt is a prompt nobody measured"
    );
    assert!(
        shipped.contains("opens with ```memory"),
        "and it must name the marker `talky/splitter` cuts with, or the block the model \
         writes is cut by nothing: {shipped}"
    );
}

// ═══════════════════════════════════ 2.-3. the assembly asks, and only when told

/// Claim 2. The turn assembly writes the contract, on its own slot path.
#[test]
fn a_turn_assembly_asks_the_brain_for_the_annotation() {
    let msg = seam(&[("inline_extraction", "1")]);
    let sys = &msg["system"];
    assert_eq!(
        sys["instructions"]["sidecar"]["text"]
            .as_str()
            .unwrap_or_default(),
        shipped_block(),
        "the annotation is asked for on EVERY assembly, because a brain that grew was \
         never handed a seed: {msg}"
    );
    // The NAME, and it is load-bearing: `concat_system_prompt` walks a family's
    // leaves alphabetically, so the leaf name is what puts the block after the
    // charter instead of in front of it. `extraction` — the lane's own name —
    // would have sorted before `reply`.
    assert!(
        "sidecar" > "reply",
        "the slot name has to sort after the charter's, or the block arrives in front of \
         the instructions it is written to follow"
    );
    // The upsert half, and it is the whole reason this may share a family with a
    // person's charter: `system.*` is written per slot path, so a path that is
    // not sent is a path that is not TOUCHED. A marker here would revoke
    // `instructions.reply` — the charter the `in_pack` lane owns since GH #488 —
    // on every single turn, which is the GH #512 defect with the roles swapped.
    assert!(
        sys.get("$replace").is_none(),
        "a marker at the root would revoke every writer's slot in the brain: {msg}"
    );
    assert!(
        sys["instructions"].get("$replace").is_none(),
        "and one on the family would revoke the charter beside it: {msg}"
    );
    assert!(
        sys["instructions"].get("reply").is_none(),
        "the collector writes the template's promise and never a person's charter: {msg}"
    );
}

/// Claim 3. The shipped default is silent, and the reason is the splitter.
#[test]
fn the_shipped_default_asks_for_nothing() {
    let msg = seam(&[]);
    assert!(
        msg["system"].get("instructions").is_none(),
        "what takes the block back out of the answer is a splitter between the brain and \
         the dispatcher, and this cell cannot see whether one stands behind it — asking \
         with nothing cutting leaves a json block in the reader's face: {msg}"
    );
    // And the other direction of the same sentence: switched off is not a
    // half-write. Nothing under the family, not an empty node.
    let sys = msg["system"].as_object().expect("a system tree");
    assert!(
        !sys.keys().any(|k| k.starts_with("instructions")),
        "off means no path at all: {msg}"
    );
}

// ══════════════════════════════════════ 4. a GROWN brain carries both

const CHARTER: &str =
    "You are this person's assistant. Answer briefly, in the language they wrote in.";

/// A real `llm` cell with the families this test is about, and nothing else in
/// `system_writable`: a slot smuggled in under a fifth family would be refused
/// rather than silently accepted.
fn brain(td: &tempfile::TempDir, base_url: &str) -> (LlmCell, DbConn) {
    let slots = json!(["identity", "instructions", "tools", "memory", "consult"]);
    let params = LlmParams::parse(&json!({
        "provider": "openai", "model": "gpt-x", "api_key": "sk-test",
        "base_url": format!("{base_url}/v1"),
        "system_order": slots.clone(),
        "system_writable": slots,
    }))
    .expect("params must parse");
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    (
        LlmCell::new(params, reqwest::Client::builder().build().unwrap()),
        DbConn::wrap(conn, None),
    )
}

/// Deliver one body into the cell exactly as the colony would.
async fn deliver(cell: &mut LlmCell, db: &mut DbConn, body: Value) {
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/brain"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );
    let msg = MessageBuilder::new(Path::new("/brain"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(body))
        .build();
    cell.handle(msg, &sink, db).await;
    drop(sink);
    while rx.recv().await.is_some() {}
}

async fn slot_paths(db: &mut DbConn) -> Vec<String> {
    db.call(|conn| -> rusqlite::Result<Vec<String>> {
        let mut stmt = conn.prepare("SELECT slot_path FROM system ORDER BY slot_path")?;
        stmt.query_map([], |r| r.get::<_, String>(0))?.collect()
    })
    .await
    .expect("cell.db is readable")
}

/// The `system` message of the request the provider actually received.
async fn composed_system_prompt(mock: &MockOpenAI) -> String {
    let reqs = mock.recorded_requests().await;
    let req = reqs
        .first()
        .expect("the brain must have called the provider");
    req.messages()
        .expect("an OpenAI request has messages[]")
        .iter()
        .filter(|m| m.get("role").and_then(|v| v.as_str()) == Some("system"))
        .filter_map(|m| m.get("content").and_then(|v| v.as_str()))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Claim 4 — the drift lock this issue exists for. A brain whose birth is behind
/// it: an identity pack arrives through the door of GH #488 and writes the
/// charter, then a turn arrives from the collector. Both slots stand, and both
/// are read.
///
/// The order is the honest one. The pack is what a grown colony does FIRST —
/// it is how a rebuilt agent gets its charter back — and it is the write that
/// would have deleted a seeded slot had it carried a marker on the family.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_grown_brain_carries_the_contract_after_an_identity_pack() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let td = tempfile::TempDir::new().unwrap();
    let (mut cell, mut db) = brain(&td, &mock.base_url);

    // 1. The charter, exactly as `affinity/brief` renders it and the `in_pack`
    //    lane hands it on: the slots and NO turn beside them, so the update
    //    costs a write and not an inference (GH #263).
    deliver(
        &mut cell,
        &mut db,
        json!({"system": {"instructions": {"reply": {"text": CHARTER}}}}),
    )
    .await;
    assert_eq!(
        slot_paths(&mut db).await,
        vec!["instructions.reply".to_string()],
        "the pack lands as the charter and nothing else"
    );

    // 2. The turn, straight out of the shipped collector.
    let mut turn = seam(&[("inline_extraction", "1")]);
    turn["messages"] = json!([{"origin": "user", "type": "text", "text": "and my editor?"}]);
    deliver(&mut cell, &mut db, turn).await;

    let paths = slot_paths(&mut db).await;
    assert!(
        paths.contains(&"instructions.reply".to_string()),
        "the charter must survive the turn — a collector that revoked it every round \
         would answer as the vendor's default assistant (GH #488): {paths:?}"
    );
    assert!(
        paths.contains(&"instructions.sidecar".to_string()),
        "and a brain that GREW must end up carrying the contract, which is the whole \
         defect: a seed is written once, at birth (GH #512): {paths:?}"
    );

    let prompt = composed_system_prompt(&mock).await;
    assert!(
        prompt.contains(CHARTER),
        "the charter has to reach the prompt, not just the cell.db: {prompt}"
    );
    assert!(
        prompt.contains("ANNOTATE EVERY TURN"),
        "and so does the contract — a slot the model never sees asks nobody for \
         anything: {prompt}"
    );
    // And in the order the contract itself asks for. `concat_system_prompt`
    // walks the leaves of a family alphabetically, so this is decided by the
    // leaf NAME and by nothing else — which is why the slot is `sidecar` and
    // not `extraction`, the name of its own lane.
    assert!(
        prompt.find(CHARTER) < prompt.find("ANNOTATE EVERY TURN"),
        "the block belongs AFTER the answer's instructions: a model that produces its \
         structured field before its reasoning answers from nothing, and the shipped \
         contract says so in its own first line: {prompt}"
    );
}

// ═══════════════════════════════════ 5. the menu tick cannot reach the family

/// Claim 5. The `$replace` GH #464 writes sits on `tools` and nowhere above it.
///
/// This is the assertion GH #512 wishes it had had: the marker that deleted the
/// two self-served tool declarations was correct where it stood, and the defect
/// was a seed underneath it. One family over the same marker would delete the
/// contract on every tick, so its position is pinned rather than assumed.
#[test]
fn the_menu_tick_replaces_the_tool_subtree_and_nothing_above_it() {
    // Since GH #529 the menu is a union over ANSWERERS, so the lane is two steps:
    // the answer is recorded as one row of the collector's own store, and the
    // write is derived from every stored row. What this claim is about — WHERE
    // the `$replace` marker sits — lives on the second step, so the round trip
    // is driven here rather than worked around.
    let recorded = emit_with(
        &[("inline_extraction", "1")],
        json!({
            "target": "/main/collector",
            "header": {"hop": {"route": "in_menu"}, "context": {}},
            "ttl": 64,
            "messages": [],
            "schemas": [{"name": "web_search", "description": "search",
                         "parameters": {"type": "object", "properties": {}}}],
            "unknown": [],
        }),
    );
    let row: Value = meclaw_core::serde_json::from_str(
        recorded[0]["messages"]
            .as_array()
            .expect("a store bundle")
            .iter()
            .find(|m| m["id"] == json!("c-menu-put"))
            .expect("the bundle records this answerer's submenu")["text"]
            .as_str()
            .expect("a tool_call text"),
    )
    .expect("the op is json");
    let out = emit_with(
        &[("inline_extraction", "1")],
        json!({
            "target": "/main/collector",
            "header": {"hop": {"route": "cstore", "operation": "bundle"},
                       "context": {"col_phase": "menu-merge"}},
            "ttl": 64,
            "messages": [{"id": "c-menu-all", "type": "tool_result",
                          "text": meclaw_core::serde_json::to_string(
                              &json!([row["row"].clone()])).unwrap()}],
            "results": [{"tool_call_id": "c-menu-all", "operation": "select"}],
        }),
    );
    let msg = out.first().expect("the menu lane writes one message");
    let sys = &msg["system"];
    assert_eq!(
        sys["tools"]["$replace"],
        json!(true),
        "the derived menu IS the whole subtree: {msg}"
    );
    assert!(
        sys.get("$replace").is_none(),
        "a marker at the root would revoke every other family on every tick: {msg}"
    );
    assert!(
        sys.get("instructions").is_none(),
        "and the menu lane writes no instructions at all — the contract travels with the \
         TURN, where the brain reads it: {msg}"
    );
}

// ══════════════════════ 6. the composites ask exactly where the block is cut

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn shipped(name: &str) -> Option<std::path::PathBuf> {
    let root = std::path::PathBuf::from(TEMPLATES).join(name);
    root.join("config.json").exists().then_some(root)
}

/// Does this composite CUT the block back out of the answer? The splitter's
/// signature is its own: an edge whose condition names `hop.route ==
/// 'extraction'`, leaving the cell that emits it.
fn cuts_the_block(composite: &str) -> bool {
    let cfg = read_json(
        &std::path::PathBuf::from(TEMPLATES)
            .join(composite)
            .join("config.json"),
    );
    cfg["params"]["graph"]["edges"]
        .as_array()
        .map(|edges| {
            edges.iter().any(|e| {
                e["from"] == json!("./splitter")
                    && e["condition"]
                        .as_str()
                        .is_some_and(|c| c.contains("hop.route == 'extraction'"))
            })
        })
        .unwrap_or(false)
}

fn collector_override(composite: &str, key: &str) -> Option<Value> {
    let p = std::path::PathBuf::from(TEMPLATES)
        .join(composite)
        .join("collector/config.json");
    if !p.exists() {
        return None;
    }
    read_json(&p)["override_params"]["assemble"]
        .get(key)
        .filter(|v| !v.is_null())
        .cloned()
}

/// Claim 6. The declaration and the edge are one statement, in both directions.
#[test]
fn the_shipped_composites_ask_exactly_where_the_block_is_cut() {
    if shipped("talky").is_some() {
        assert!(
            cuts_the_block("talky"),
            "talky is the composite the sidecar was built for; its splitter carries the \
             `extraction` edge (GH #379)"
        );
        assert_eq!(
            collector_override("talky", "inline_extraction"),
            Some(json!("1")),
            "so its collector must ASK for the block — without it the splitter is a pure \
             pass-through, the `extraction` lane never fires, and the whole memory write \
             path below it is wired and inert (GH #525)"
        );
    }
    if shipped("cogny").is_some() {
        assert!(
            !cuts_the_block("cogny"),
            "cogny has no splitter: its answer travels back to the asking agent whole"
        );
        assert_ne!(
            collector_override("cogny", "inline_extraction"),
            Some(json!("1")),
            "so it must not ask for a fence nobody removes — the advice would reach the \
             front model with a json block stapled to it"
        );
    }
}
