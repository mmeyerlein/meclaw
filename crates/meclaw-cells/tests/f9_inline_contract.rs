//! 0.3.x follow-up F9 -- the inline ingress extracts world state, exactly as the
//! batch lane does (GitHub #53).
//!
//! The batch lane's prompt was sharpened until it extracted WORLD STATE only:
//! never conversation metadata, never an echo of an assistant turn. The inline
//! contract -- the block a front-line persona carries so it can emit its facts in
//! the answering turn -- never got that discipline. It forbade smalltalk,
//! follow-up questions and the model's own opinions, and said nothing about the
//! model's own ANSWER.
//!
//! Measured in a production colony: two history questions about a preference
//! axis ("which editors did I favour in the last 10 days", "which between the
//! 5th and the 8th"). The front model answered correctly out of the delivered
//! bundle -- and then minted its own answers as facts, on a fresh predicate
//! spelling, each with a `valid_until` derived from the question's date range.
//! Three damages at once: a fifth spelling on an axis that already had four; a
//! validity that closes the fact on arrival, so the as-of leg cannot see it while
//! keyword and semantic still can; and a feedback loop, because every history
//! answer becomes material for the next recall about the same axis.
//!
//! So the hive SHIPS the contract instead of leaving each persona to invent one,
//! the way it ships `predicate-core.json` instead of leaving each extractor to
//! invent a vocabulary. This file is that file's drift lock, in both directions:
//! a discipline the batch lane states must be in the inline contract, and a
//! discipline the inline contract states must still be in the batch prompt.
//! Neither lane can quietly lose one.

use std::io::Write;
use std::process::{Command, Stdio};

const GLUE_CONFIG: &str = "../../templates/memory-hive/extract-glue/config.json";
const INLINE_CONTRACT: &str = "../../templates/memory-hive/inline-contract.md";

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty string --
/// the same substitution the colony performs when it instantiates the template.
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

fn glue_script() -> String {
    let raw = std::fs::read_to_string(GLUE_CONFIG).expect("extract-glue config");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
    )
}

/// Run a shipped script over a real stdin document, handing the script to
/// python3 **on stdin** instead of in argv.
///
/// A single argv string is capped at 128 KiB (`MAX_ARG_STRLEN`) and the shipped
/// scripts have grown to within a few KB of that line, so `python3 -c <whole
/// script>` is a harness that breaks on size rather than on behaviour (GH #279,
/// precedent 89a522e4). stdin carries the program, so the document rides inside
/// it and is put under `sys.stdin` before the script runs. From there the script
/// executes exactly as `python3 -c` ran it: same `__main__` globals, same
/// stdout, same exit status.
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
    // Dropped, not merely borrowed: python reads until EOF.
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    child.wait_with_output().expect("wait")
}

/// Run the real script with a real stdin document and return the emitted messages.
fn emit(doc: serde_json::Value) -> Vec<serde_json::Value> {
    let out = run_script_on_stdin(
        &glue_script(),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    assert!(
        out.status.success(),
        "extract-glue exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// The instructions the batched extractor receives for a batch with no axis hint
/// and no replacement window -- the plain contract, which is where the shared
/// discipline lives.
fn batch_instructions() -> String {
    let items = serde_json::json!([
        {"episode_id": "e1", "sender": "user", "content": "My favorite editor is Helix."}
    ]);
    let scratch = serde_json::json!([
        {"key": "b1", "kind": "batch", "payload": items.to_string()},
        {"key": "b1", "kind": "vocab", "payload": "{}"}
    ]);
    let doc = serde_json::json!({
        "header": {
            "context": {"store_origin": "extract", "mem_phase": "prompt", "batch_id": "b1"},
            "hop": {"operation": "select", "rows_affected": 1}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                      "text": scratch.to_string()}]
    });
    let msgs = emit(doc);
    let ask = msgs
        .iter()
        .find(|m| m["header"]["route"] == "extract")
        .expect("the prompt phase asks the extractor");
    ask["system"]["instructions"]["text"]
        .as_str()
        .expect("instructions text")
        .to_string()
}

/// The shipped inline contract, whole.
fn inline_contract() -> String {
    std::fs::read_to_string(INLINE_CONTRACT).unwrap_or_else(|e| {
        panic!(
            "the hive ships no inline extraction contract ({INLINE_CONTRACT}): {e}. \
             A discipline that lives only in each consumer's persona is a discipline \
             nothing can pin -- GitHub #53."
        )
    })
}

/// Every run of whitespace collapsed to one space.
///
/// The lock is on the WORDING, never on the wrapping: both texts are prose
/// wrapped to a column, and a rule that has to survive a re-wrap intact would be
/// a rule nobody dares reformat. What this still catches is the thing that
/// matters -- a paraphrase, which is where one discipline starts to mean two.
fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The block a persona actually carries: the fenced `text` section of the file.
/// Prose ABOUT a rule is not the rule, so the assertions below read the block and
/// never the page around it.
fn contract_block() -> String {
    let raw = inline_contract();
    let (_, tail) = raw
        .split_once("```text\n")
        .expect("the contract file carries the persona block in a ```text fence");
    let (block, _) = tail
        .split_once("\n```")
        .expect("the persona block's fence is closed");
    block.to_string()
}

/// One discipline of the extraction contract, and where each lane states it.
///
/// The five are the ones GitHub #53 names. `batch` is the phrase the batched
/// prompt uses, `inline` the phrase the shipped contract uses -- two phrasings on
/// purpose, because the batch lane reads a list of finished turns while the front
/// model is standing in the middle of one, and a rule about "the answer you are
/// about to give" cannot be written the same way for both.
struct Discipline {
    id: &'static str,
    batch: &'static str,
    inline: &'static str,
}

const DISCIPLINES: &[Discipline] = &[
    Discipline {
        id: "world state only, never that the turn happened",
        batch: "Extract what a turn is ABOUT, never that the turn happened.",
        inline: "Extract what a turn is ABOUT, never that the turn happened.",
    },
    Discipline {
        id: "the assistant's own answer is not a fact",
        batch: "is an earlier answer of your own",
        inline: "YOUR OWN ANSWER IS NOT A FACT",
    },
    Discipline {
        id: "a question is not a fact",
        batch: "the question IS the episode",
        inline: "A QUESTION IS NOT A FACT",
    },
    Discipline {
        id: "restating stored knowledge mints nothing",
        batch: "Restating a fact the user already gave you is not new world state",
        inline: "RESTATING STORED KNOWLEDGE MINTS NOTHING",
    },
    Discipline {
        id: "an empty facts list is a correct extraction",
        batch: "an empty facts list is a correct answer",
        inline: "an empty facts list is a correct answer",
    },
];

#[test]
fn the_hive_ships_the_inline_extraction_contract() {
    // The premise of the whole package. `predicate-core.json` is shipped because
    // a vocabulary each extractor invents for itself is a vocabulary nothing can
    // hold to account; the extraction DISCIPLINE is the same kind of thing one
    // dimension over, and it was the half every consumer had to guess.
    let block = contract_block();
    assert!(
        block.contains("\"facts\""),
        "the block declares the payload shape it asks for:\n{block}"
    );
    assert!(
        block.contains("fact_kind"),
        "including fact_kind, which the validator requires:\n{block}"
    );
}

#[test]
fn the_inline_contract_carries_every_discipline_of_the_batch_lane() {
    // The issue itself: the batch prompt was sharpened, the inline contract was
    // not, and the two ingresses write into ONE table.
    let block = flat(&contract_block());
    for d in DISCIPLINES {
        assert!(
            block.contains(d.inline),
            "the inline contract does not state '{}' (looked for {:?}):\n{block}",
            d.id,
            d.inline
        );
    }
}

#[test]
fn the_batch_lane_still_carries_every_discipline_it_lends() {
    // The lock's other direction. Two prompts stating one discipline drift apart
    // silently unless something reads both, and the ingress that keeps the rule
    // is not always the one that loses it.
    let instructions = flat(&batch_instructions());
    for d in DISCIPLINES {
        assert!(
            instructions.contains(d.batch),
            "the batch prompt no longer states '{}' (looked for {:?}):\n{instructions}",
            d.id,
            d.batch
        );
    }
}

#[test]
fn the_sentence_both_lanes_can_share_is_shared_byte_for_byte() {
    // Where the two lanes CAN say the same thing, they say it in the same bytes.
    // A paraphrase is where a discipline starts to mean two things.
    let block = flat(&contract_block());
    let instructions = flat(&batch_instructions());
    for shared in [
        "Extract what a turn is ABOUT, never that the turn happened.",
        "an empty facts list is a correct answer",
    ] {
        assert!(
            instructions.contains(shared),
            "the batch prompt lost the shared sentence {shared:?}"
        );
        assert!(
            block.contains(shared),
            "the inline contract lost the shared sentence {shared:?}"
        );
    }
}

#[test]
fn the_contract_forbids_a_validity_derived_from_a_question() {
    // Damage 2 of the measured defect, and the one that is invisible from the
    // outside: the minted `valid_until` came from the RANGE the question asked
    // about, so the fact was closed the moment it landed -- the as-of temporal leg
    // never saw it while keyword and semantic still did. One statement, visible to
    // some legs and invisible to others, is worse than a duplicate.
    let block = contract_block();
    assert!(
        block.contains("valid_until"),
        "the contract has to name the column it forbids deriving:\n{block}"
    );
    assert!(
        block.contains("closed on arrival"),
        "and say what a validity taken from a question's range does:\n{block}"
    );
}

#[test]
fn the_contract_makes_the_block_name_the_turn_it_speaks_for() {
    // The seam with GitHub #52: the ingress takes the queue rows of the episodes a
    // block NAMES out of the queue, and an empty block is a verdict only if it
    // says which turn it is a verdict about. A contract that never mentions
    // `episode_id` produces blocks that cover nothing.
    let block = contract_block();
    assert!(
        block.contains("episode_id"),
        "the block names the turn it speaks for:\n{block}"
    );
}

#[test]
fn the_contract_file_points_at_its_consumers() {
    // Same rule `predicate-core.json` follows: an authority file that does not say
    // who reads it becomes a file nobody updates.
    let raw = inline_contract();
    assert!(
        raw.contains("extract-glue"),
        "the contract names the lane that validates what it asks for"
    );
    assert!(
        raw.contains("f9_inline_contract.rs"),
        "and the drift lock that holds it to the batch prompt"
    );
}
