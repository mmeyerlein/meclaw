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
//! **GH #606 moved the shipper and this file moved with it.** From #525 to #606
//! the collector HELD the text as a literal and wrote it on every turn assembly.
//! That is retracted, not quietly reworded: the rules of a memory sat in a cell
//! that enforces none of them, one composite away — the arrangement GH #552 had
//! already retired for `memory_recall`'s schema — and a contract one cell types
//! can describe exactly one consumer, which is one too few the moment a screen
//! wants a section of the same answer. `templates/memory-hive/schemas` OFFERS the
//! section now, on the same lane that hands out the tool schema, and the
//! collector COMPOSES what it was offered under a preamble of its own.
//!
//! The lesson of #525 survives the move and is what decides where the composed
//! contract lands: not in a seed, which is written once at birth, but in a slot
//! the collector re-derives — on the menu lane, out of the stored offers, on
//! every mutation receipt this hive hears. Same durability class as
//! `system.tools`, one write per change and nothing per turn.
//!
//! What this file pins:
//!
//! 1. **The text the hive offers is the text the hive documents**, byte for
//!    byte — the same text `gh299` pins from the other end.
//! 2. **The menu merge composes it**, on `instructions.sidecar`, with the
//!    collector's preamble in front and the section under its own heading, and
//!    with no `$replace` marker anywhere above the `tools` node — an upsert on
//!    one slot path, not a revocation of a family.
//! 3. **The shipped default asks for nothing**, because what cuts the block back
//!    out is a splitter this cell cannot see.
//! 4. **A GROWN brain carries both.** A real `llm` cell, an identity pack
//!    through the door of GH #488 and then the menu message:
//!    `instructions.reply` and `instructions.sidecar` stand side by side in the
//!    brain's own `cell.db`, and both reach the composed system prompt.
//! 5. **Nobody offering anything is an EMPTY slot, not a silence.** Durable state
//!    is revoked, never merely abandoned — otherwise the last contract written
//!    stands in a brain whose offers are gone, and the model keeps fencing for a
//!    section nothing cuts any more.
//! 6. **The shipped composites ask exactly where the block is cut.** `talky`
//!    routes the sidecar out of its splitter and switches the knob on; `cogny`
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
const SCHEMAS_CONFIG: &str = "../../templates/memory-hive/schemas/config.json";
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

/// The `memory` offer of the SHIPPED declaring cell, obtained by running it —
/// the answer a collector's menu question actually gets back, never a copy of
/// one (GH #606).
fn memory_offer() -> Value {
    let raw = std::fs::read_to_string(SCHEMAS_CONFIG)
        .unwrap_or_else(|e| panic!("the hive ships no declaring cell ({SCHEMAS_CONFIG}): {e}"));
    let cfg: Value = meclaw_core::serde_json::from_str(&raw).expect("schemas config json");
    let script = cfg["params"]["script_inline"]
        .as_str()
        .expect("script_inline");
    let out = run_script_on_stdin(
        script,
        &json!({"body": {"tools": ["*"], "messages": []}}).to_string(),
    );
    assert!(
        out.status.success(),
        "the declaring cell exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let answer: Value = meclaw_core::serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "the answer is not json ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    answer["sidecar"]
        .as_array()
        .and_then(|o| o.iter().find(|o| o["section"] == "memory").cloned())
        .unwrap_or_else(|| panic!("nothing offers the `memory` section: {answer}"))
}

/// The block as it reaches a model: the `instruction` of that offer.
fn shipped_block() -> String {
    let offer = memory_offer();
    offer["instruction"]
        .as_str()
        .unwrap_or_else(|| panic!("the offer carries no instruction: {offer}"))
        .to_string()
}

/// The FRAME of the collector's preamble — the fixed half of the one part of the
/// contract that cell writes. Read out of the shipped `script_inline` source
/// rather than out of a running lane, so the assertion is about what ships.
///
/// The rest of the preamble is composed from the offers (the whole-object shape
/// line and the obligation stated with the section names in it), so it is
/// asserted against a real composition below rather than against a literal.
fn shipped_frame() -> String {
    let src = assemble_script();
    let (_, tail) = src.split_once("SIDECAR_PREAMBLE = (").unwrap_or_else(|| {
        panic!("the collector states no preamble — nothing owns the frame of the block")
    });
    let (body, _) = tail
        .split_once(")\n")
        .expect("the preamble literal is not closed");
    // A parenthesised run of adjacent string literals, joined the way python
    // joins them.
    let mut out = String::new();
    let mut rest = body;
    while let Some(at) = rest.find('"') {
        let tail = &rest[at + 1..];
        let end = tail.find('"').expect("closed literal");
        out.push_str(&tail[..end]);
        rest = &tail[end + 1..];
    }
    out
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

/// One menu ROUND TRIP, driven the way the lane really runs since GH #529: the
/// answer is recorded as one row of the collector's own store, and the write is
/// derived from every stored row. Both halves of the answer travel — `schemas[]`
/// and, since GH #606, `sidecar[]` — and what comes back is the `menu` message.
fn menu_merge(over: &[(&str, &str)], schemas: Value, offers: Value) -> Value {
    let recorded = emit_with(
        over,
        json!({
            "target": "/main/collector",
            "header": {"hop": {"route": "in_menu"}, "context": {}},
            "ttl": 64,
            "messages": [],
            "schemas": schemas,
            "unknown": [],
            "sidecar": offers,
        }),
    );
    let op: Value = meclaw_core::serde_json::from_str(
        recorded
            .first()
            .unwrap_or_else(|| panic!("the answer was not recorded at all"))["messages"]
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
        over,
        json!({
            "target": "/main/collector",
            "header": {"hop": {"route": "cstore", "operation": "bundle"},
                       "context": {"col_phase": "menu-merge"}},
            "ttl": 64,
            "messages": [{"id": "c-menu-all", "type": "tool_result",
                          "text": meclaw_core::serde_json::to_string(
                              &json!([op["row"].clone()])).unwrap()}],
            "results": [{"tool_call_id": "c-menu-all", "operation": "select"}],
        }),
    );
    out.into_iter()
        .next()
        .expect("the menu lane writes one message")
}

/// The offer as the memory hive answers it, in the shape the lane carries.
fn memory_offers() -> Value {
    json!([memory_offer()])
}

/// An OPTIONAL section, in the shape an app offers one — the `display` section
/// of the wave's own contract, which is what the harness measured the block
/// against.
fn a_display_offer() -> Value {
    json!([{"section": "display", "required": false,
            "schema": {"type": "object",
                       "properties": {
                           "kind": {"type": "string",
                                    "enum": ["fact", "list", "table", "text",
                                             "link", "chart"]},
                           "title": {"type": "string", "description": "…"},
                           "data": {"type": "string",
                                    "description": "string | [strings] | {key: value}"},
                           "mode": {"type": "string",
                                    "enum": ["transient", "pinned"]}},
                       "required": ["kind", "title", "data", "mode"]},
            "instruction": "The person is looking at a screen."}])
}

/// One ordinary tool declaration, so a merge has a menu half as well.
fn a_tool() -> Value {
    json!([{"name": "web_search", "description": "search",
            "parameters": {"type": "object", "properties": {}}}])
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
fn the_hive_offers_the_documented_block_byte_for_byte() {
    let documented = documented_block();
    let shipped = shipped_block();
    assert!(
        documented.contains("ANNOTATE EVERY TURN"),
        "the document must still carry the obligation: {documented}"
    );
    assert_eq!(
        shipped, documented,
        "the offer has drifted from templates/memory-hive/inline-contract.md; the \
         document is the authority and a forked prompt is a prompt nobody measured"
    );
    let offer = memory_offer();
    assert_eq!(
        offer["section"], "memory",
        "and it is offered under the key its own ingress is routed on: {offer}"
    );
    assert_eq!(
        offer["required"], true,
        "and as a REQUIRED section — the obligation of GH #299 is a field of the \
         offer and not only a sentence in it: {offer}"
    );
    // The collector's half of the same sentence: the fence the section stopped
    // naming is named ONCE, by the cell that owns the frame.
    let frame = shipped_frame();
    assert!(
        frame.contains("```sidecar"),
        "the frame names the marker `talky/splitter` cuts with, or the block the \
         model writes is cut by nothing: {frame}"
    );
}

// ═══════════════════════════════════ 2.-3. the assembly asks, and only when told

/// Claim 2. The menu merge composes the contract, on its own slot path.
///
/// It used to be the turn assembly, and that sentence is retracted rather than
/// quietly reworded (GH #606): what the collector writes is derived from what it
/// was OFFERED, and the offers arrive on the menu lane. The property #525 needed
/// is untouched — the slot is re-derived, never seeded — and it is now derived on
/// the same occasion, out of the same rows, as `system.tools` beside it.
#[test]
fn the_menu_merge_composes_the_contract_for_the_brain() {
    let msg = menu_merge(&[("sidecar", "1")], a_tool(), memory_offers());
    let sys = &msg["system"];
    let written = sys["instructions"]["sidecar"]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(
        written.starts_with(&shipped_frame()),
        "the frame comes first — a section read before the rule about how many \
         blocks to write is a section a model places by guessing: {msg}"
    );
    assert!(
        written.contains("## memory (required)"),
        "each section stands under its own heading, carrying the word the preamble's \
         obligation is stated with: {written}"
    );
    assert!(
        written.contains(&shipped_block()),
        "and the words are the offering hive's, verbatim — a collector that edited \
         them would be a second author of a contract it does not own: {written}"
    );
    assert_eq!(
        msg["header"]["sidecar_sections"], "memory",
        "and the receipt names what went in: {msg}"
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

/// The two repairs the HARNESS bought (GH #608 measurement, 2026-09-06). The
/// first composition of this contract produced 7 malformed blocks in 52 turns
/// against 0 in the control arm, in two patterns, and neither is about the
/// sections — both are about the frame this cell writes:
///
/// 1. **A model dropped the outer braces** and emitted the section objects side
///    by side. Every section heading shows the INNER shape; nothing showed the
///    OUTER one. So the preamble prints the whole object, with every key of this
///    particular composition in it and the braces the model has to write.
/// 2. **A model let the optional section stand INSTEAD of the required one** on
///    the turns where both applied. "A required section is written on every
///    turn" is true, general, and was read as a rule about sections in the
///    abstract; naming them makes it a rule about THESE, and the clause about
///    the optional one says the failing case out loud.
///
/// Both are asserted against a real two-section composition, because a preamble
/// that named the sections of some other tree would be a preamble nobody
/// measured.
#[test]
fn the_preamble_shows_the_whole_object_and_guards_the_required_section() {
    let mut offers = memory_offers();
    offers.as_array_mut().expect("an array of offers").extend(
        a_display_offer()
            .as_array()
            .expect("an array")
            .iter()
            .cloned(),
    );
    let msg = menu_merge(&[("sidecar", "1")], a_tool(), offers);
    let written = msg["system"]["instructions"]["sidecar"]["text"]
        .as_str()
        .unwrap_or_default();
    let preamble = written
        .split("\n\n## ")
        .next()
        .expect("the preamble stands before the first section");
    assert!(
        preamble.contains(r#"{"memory": {...}, "display": {...}}"#),
        "the preamble shows the WHOLE object — one model wrote the sections side by \
         side without the outer braces, because every heading below shows the inner \
         shape and nothing showed the outer one: {preamble}"
    );
    assert!(
        preamble.contains(r#""memory" is written on EVERY turn"#),
        "and states the obligation with the section's NAME in it, not as a rule about \
         sections in the abstract: {preamble}"
    );
    assert!(
        preamble.contains("an optional section never replaces a required one"),
        "and says the measured failure out loud — one model let `display` stand \
         instead of `memory` on the turns where both applied: {preamble}"
    );
    assert!(
        preamble.chars().count() < 500,
        "while staying under 500 characters: it is re-read by the provider on every \
         turn of every conversation, in front of every section ({} chars): {preamble}",
        preamble.chars().count()
    );
    // Required first, and the shape line says the same thing in the same order:
    // a model that copies the example writes the required key first.
    let memory_at = written
        .find("## memory (required)")
        .expect("the memory heading");
    let display_at = written
        .find("## display (optional)")
        .expect("the display heading");
    assert!(
        memory_at < display_at,
        "required sections stand first, so the obligation is read before its \
         exceptions: {written}"
    );
    assert_eq!(
        msg["header"]["sidecar_sections"], "memory,display",
        "and the receipt names them in that order: {msg}"
    );
}

/// Claim 3. The shipped default is silent, and the reason is the splitter.
#[test]
fn the_shipped_default_asks_for_nothing() {
    let msg = menu_merge(&[], a_tool(), memory_offers());
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
    assert_eq!(
        msg["header"]["sidecar_sections"], "",
        "and the receipt says so rather than naming offers nobody asked for: {msg}"
    );
    // A turn assembly writes none of this either way. The contract travels with
    // the MENU since GH #606, so an assembly that carried one would be a second
    // writer on the same path, racing the merge every round.
    let turn = seam(&[("sidecar", "1")]);
    assert!(
        turn["system"].get("instructions").is_none(),
        "the turn assembly writes no instructions at all — one path, one writer: {turn}"
    );
}

/// Claim 5. Nobody offering anything is an EMPTY slot, not a silence.
///
/// Durable state is revoked, never merely abandoned — the rule the `consult`
/// slot follows one family over. A collector that fell silent here would leave
/// the last contract it wrote standing in a brain whose offers are gone, and the
/// model would keep fencing for a section nothing cuts any more. The empty text
/// contributes nothing to the prompt, which is the pass-through the splitter
/// behind it was before the knob.
#[test]
fn no_offer_writes_an_empty_slot_rather_than_leaving_a_stale_one() {
    let msg = menu_merge(&[("sidecar", "1")], a_tool(), json!([]));
    assert_eq!(
        msg["system"]["instructions"]["sidecar"]["text"], "",
        "the slot is written EMPTY: {msg}"
    );
    assert!(
        msg["system"]["tools"].is_object(),
        "and the menu beside it is untouched by the emptiness of the other half: {msg}"
    );
}

/// The other half of that guard, and the one that costs a tool set if it is
/// wrong: an answerer that offers a SECTION without declaring a tool must not
/// become the occasion on which this collector writes `system.tools`. The
/// `$replace` of GH #464 makes such a write a revocation of everything the model
/// had, on the strength of an answer that declared nothing.
#[test]
fn an_offer_without_declarations_writes_no_menu_at_all() {
    let msg = menu_merge(&[("sidecar", "1")], json!([]), memory_offers());
    assert!(
        msg["system"].get("tools").is_none(),
        "no menu is written from an answer that carried no declaration: {msg}"
    );
    assert!(
        msg["system"]["instructions"]["sidecar"]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("## memory (required)"),
        "and the half that WAS answered still arrives: {msg}"
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

    // 2. The menu message, straight out of the shipped collector: the answer of
    //    a memory hive, merged and composed. This is the write that carries the
    //    contract since GH #606, and it is a write and not an inference — no
    //    turn beside it.
    let menu = menu_merge(&[("sidecar", "1")], a_tool(), memory_offers());
    deliver(
        &mut cell,
        &mut db,
        json!({"system": menu["system"].clone()}),
    )
    .await;

    // 3. And then an ordinary turn, which carries no instructions of its own.
    let mut turn = seam(&[("sidecar", "1")]);
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

/// Claim 6. The `$replace` GH #464 writes sits on `tools` and nowhere above it.
///
/// This is the assertion GH #512 wishes it had had: the marker that deleted the
/// two self-served tool declarations was correct where it stood, and the defect
/// was a seed underneath it. Its position matters more since GH #606, not less —
/// the contract now rides on the SAME message, one family over, so a marker that
/// slipped a level up would delete it on every tick.
#[test]
fn the_menu_tick_replaces_the_tool_subtree_and_nothing_above_it() {
    let msg = menu_merge(&[("sidecar", "1")], a_tool(), memory_offers());
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
        sys["instructions"].get("$replace").is_none(),
        "and one on the family beside it would revoke the charter the `in_pack` lane \
         owns (GH #488), on every tick: {msg}"
    );
    assert!(
        sys["instructions"].get("reply").is_none(),
        "the collector writes the template's promise and never a person's charter: {msg}"
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
/// 'sidecar'`, leaving the cell that emits it. The lane was called `extraction`
/// until `talky@5.1.0`, when the block became generic and the port took the
/// name of the block itself (GH #605).
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
                        .is_some_and(|c| c.contains("hop.route == 'sidecar'"))
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
             `sidecar` edge (GH #379, renamed in GH #605)"
        );
        assert_eq!(
            collector_override("talky", "sidecar"),
            Some(json!("1")),
            "so its collector must ASK for the block — without it the splitter is a pure \
             pass-through, the lane never fires, and the whole memory write path below it \
             is wired and inert (GH #525)"
        );
        assert_eq!(
            collector_override("talky", "inline_extraction"),
            None,
            "under the name it has since GH #606, and under that name only — a knob \
             left standing under its old spelling is a knob nothing reads"
        );
    }
    if shipped("cogny").is_some() {
        assert!(
            !cuts_the_block("cogny"),
            "cogny has no splitter: its answer travels back to the asking agent whole"
        );
        assert_ne!(
            collector_override("cogny", "sidecar"),
            Some(json!("1")),
            "so it must not ask for a fence nobody removes — the advice would reach the \
             front model with a json block stapled to it"
        );
    }
}
