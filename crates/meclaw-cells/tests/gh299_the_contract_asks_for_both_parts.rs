//! Wave 5 -- the annotation is an OBLIGATION with two parts (GitHub #299).
//!
//! This file replaces `f9_inline_contract.rs`. That lock held the shipped inline
//! contract against the batched extractor's prompt, in BOTH directions: a
//! discipline the batch lane stated had to be in the inline block, and a
//! discipline the inline block stated had to still be in the batch prompt.
//! Per-turn extraction (GitHub #298) removed the batch lane, so that premise has
//! no second side left -- the prompt it compared against is gone, not weakened.
//! Its disciplines were not dropped with it: five of them are asserted below
//! against the one surface that still carries them, and the sixth ("an empty
//! facts list is a correct answer") was RETRACTED on purpose and is asserted
//! ABSENT here, because the obligation says the same thing from the other end.
//!
//! What is left is one direction, and it is the shape half of #299. The
//! behavioural half -- that a front model actually annotates every turn -- is the
//! harness (wave 6); this file pins only that the block SAYS what the ingress can
//! read, which is the half a prompt rewrite can lose silently.
//!
//! The block is the whole of what the memory tells a model about a turn, it is
//! carried on every single call, and its length is paid for per call. So the
//! assertions are two-sided: the six things that must be in it, the three that
//! were taken out, and a length bound so the prohibition sprawl the rewrite cut
//! (3,302 characters to 1,573) cannot grow back unnoticed.
//!
//! Everything here reads the SHIPPED files -- the contract, `predicate-core.json`
//! and the real `params.script_inline` of `extract-glue` -- so nothing costs
//! anything and nothing is a copy.

use std::io::Write;
use std::process::{Command, Stdio};

const GLUE_CONFIG: &str = "../../templates/memory-hive/extract-glue/config.json";
const INLINE_CONTRACT: &str = "../../templates/memory-hive/inline-contract.md";
const CORE_LIST: &str = "../../templates/memory-hive/predicate-core.json";

/// The block's length bound, in characters.
///
/// A bound rather than a measurement: the rewrite left 1,573 characters and the
/// wording will keep moving while the harness tunes it, so what is pinned is the
/// ceiling under which that tuning has to stay. Every character in here is
/// re-read by the provider on every turn of every conversation.
const LENGTH_BOUND: usize = 1_600;

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
/// python3 **on stdin** instead of in argv (GH #279: a single argv string is
/// capped at 128 KiB and the shipped scripts have grown to within a few KB of
/// that line).
fn run_script_on_stdin(script: &str, stdin_doc: &[u8]) -> std::process::Output {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        serde_json::to_string(script).unwrap(),
        serde_json::to_string(&String::from_utf8_lossy(stdin_doc).to_string()).unwrap(),
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
    let out = run_script_on_stdin(&glue_script(), &meclaw_testing::code_stdin_bytes(&doc));
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

/// The shipped contract file, whole.
fn inline_contract() -> String {
    std::fs::read_to_string(INLINE_CONTRACT).unwrap_or_else(|e| {
        panic!(
            "the hive ships no inline extraction contract ({INLINE_CONTRACT}): {e}. \
             A discipline that lives only in each consumer's persona is a discipline \
             nothing can pin -- GitHub #53."
        )
    })
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

/// Every run of whitespace collapsed to one space.
///
/// The lock is on the WORDING, never on the wrapping: the block is wrapped to a
/// column and a rule that had to survive a re-wrap intact would be a rule nobody
/// dares reformat. What this still catches is the thing that matters -- a
/// paraphrase, which is where one discipline starts to mean two.
fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The first JSON value that starts at `needle` inside the block.
///
/// The forms in the block are wrapped across lines to fit the column, so they are
/// read with a streaming parser rather than by line: what is asserted is that the
/// bytes a model is shown ARE parseable JSON, which is the whole reason to show a
/// form instead of describing one.
fn json_form(block: &str, needle: &str) -> serde_json::Value {
    let at = block
        .find(needle)
        .unwrap_or_else(|| panic!("the block shows no form starting {needle:?}:\n{block}"));
    serde_json::Deserializer::from_str(&block[at..])
        .into_iter::<serde_json::Value>()
        .next()
        .expect("a value follows")
        .unwrap_or_else(|e| {
            panic!(
                "the form starting {needle:?} is not JSON ({e}):\n{}",
                &block[at..]
            )
        })
}

/// The predicates of one cardinality group, as `predicate-core.json` declares them.
fn core_group(kind: &str) -> Vec<String> {
    let raw = std::fs::read_to_string(CORE_LIST).unwrap_or_else(|e| {
        panic!(
            "the hive ships no curated core vocabulary ({CORE_LIST}): {e}. \
             It is the authority the block below copies."
        )
    });
    let list: serde_json::Value = serde_json::from_str(&raw).expect("core list json");
    let mut out: Vec<String> = list["predicates"]
        .as_array()
        .expect("predicates array")
        .iter()
        .filter(|p| p["cardinality"] == kind)
        .map(|p| p["predicate"].as_str().expect("predicate").to_string())
        .collect();
    out.sort();
    out
}

/// The predicates of one cardinality group, as the BLOCK lists them.
///
/// The list runs on past its own line, so it is read as a token run rather than
/// as a line: everything after the group's colon that still looks like a
/// predicate key, up to and including the token the next sentence is glued to.
fn block_group(block: &str, kind: &str) -> Vec<String> {
    let flat = flat(block);
    let marker = format!("{kind} (");
    let at = flat
        .find(&marker)
        .unwrap_or_else(|| panic!("the block names no {kind:?} group:\n{flat}"));
    let (_, tail) = flat[at..]
        .split_once("): ")
        .unwrap_or_else(|| panic!("the {kind:?} group opens no list:\n{flat}"));
    let mut out = Vec::new();
    for token in tail.split(',') {
        let token = token.trim();
        let key: String = token
            .chars()
            .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_')
            .collect();
        if key.is_empty() {
            break;
        }
        let ended = key.len() != token.len();
        out.push(key);
        if ended {
            // The last predicate of the group carries the next sentence behind it.
            break;
        }
    }
    out.sort();
    out
}

/// The conversation the block below is written in.
const SESSION: &str = "s-gh299";

/// The room, and who was present. Not `["*"]` -- a universal set would let the
/// case pass against a write path with no gate at all (#244).
const CHANNEL: &str = "c-gh299";
const AUDIENCE: &str = r#"["member:user","agent:assistant"]"#;

/// One annotation block as the port edge delivers it, with the provenance the
/// gate requires next to it.
fn annotation(payload: &str) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "inline", "mem_phase": "inline",
                        "session_id": SESSION,
                        "audience_set": AUDIENCE, "channel": CHANNEL},
            "hop": {}
        },
        "messages": [{"origin": "user", "type": "text", "text": payload}]
    })
}

fn store_ops(msgs: &[serde_json::Value]) -> Vec<serde_json::Value> {
    msgs.iter()
        .filter(|m| m["header"]["route"] == "xstore")
        .map(|m| {
            let text = m["messages"][0]["text"].as_str().expect("op text");
            serde_json::from_str::<serde_json::Value>(text).expect("op args")
        })
        .collect()
}

#[test]
fn the_block_obliges_an_annotation_on_every_turn() {
    // (a) The whole change of #299. The old block PERMITTED an empty answer; this
    // one requires one, because a turn nobody annotated is now a turn nobody
    // extracts -- there is no second lane behind it that would have read the turn
    // anyway.
    let block = flat(&contract_block());
    assert!(
        block.contains("Annotate EVERY turn"),
        "the block states the obligation, not a permission:\n{block}"
    );
    assert!(
        block.contains("call `remember`"),
        "and names the call that discharges it:\n{block}"
    );
    assert!(
        block.contains("AFTER your answer"),
        "in the order that makes the answer readable -- a model that emits its \
         structured field before its reasoning answers from nothing:\n{block}"
    );
    assert!(
        block.contains("skipping the call is a fault"),
        "and says what an absent call IS, or the obligation reads as a preference:\n{block}"
    );
}

#[test]
fn the_block_names_both_parts_of_the_annotation() {
    // (b) The annotation has two halves and they are not one thing: `facts` is
    // what was worth remembering, `topic` is where the conversation stands. A
    // block that named only the first would leave the topics table with no writer
    // and the close pass with no thread to read.
    let block = flat(&contract_block());
    assert!(
        block.contains("both parts"),
        "the block says the annotation HAS two parts:\n{block}"
    );
    assert!(
        block.contains("`facts`, this turn's delta of world state"),
        "and what the first one is:\n{block}"
    );
    assert!(
        block.contains("`topic`, where the conversation stands"),
        "and what the second one is -- a topic is not a fact: it has no subject \
         and no predicate, and minting it as a claim would open an axis about the \
         conversation next to the axes about the world:\n{block}"
    );
}

#[test]
fn the_nothing_form_is_the_one_the_ingress_actually_parses() {
    // (c) The form the block SHOWS for a turn that carried nothing has to be the
    // form the lane reads as a verdict rather than as a broken block -- otherwise
    // the obligation produces rejects. Read as JSON first (a form a model copies
    // that does not parse is worse than no form), then run through the real
    // script.
    let block = contract_block();
    let form = json_form(&block, "{\"nothing_new\"");
    assert_eq!(
        form["nothing_new"], true,
        "the empty answer says so explicitly -- `is_explicit_nothing` reads the \
         flag as either true or absent: {form}"
    );
    assert_eq!(
        form["facts"].as_array().map(|f| f.len()),
        Some(0),
        "with zero facts: {form}"
    );
    assert!(
        form["topic"]["movement"].is_string(),
        "and the second part anyway -- a turn that carried no world state still \
         moved the conversation somewhere: {form}"
    );

    // The same bytes down the real lane. `episode_id` is added here and ONLY
    // here: the ingress resolves the turn itself (the block names none, by
    // design), and naming it keeps this case to the single hop that measures the
    // verdict rather than re-measuring the binding round trip.
    let mut bound = form.clone();
    bound["episode_id"] = serde_json::json!("e-gh299");
    let msgs = emit(annotation(&bound.to_string()));
    assert!(
        msgs.iter().all(|m| m["header"]["route"] != "reject"),
        "the form the block shows is not a refusal: {msgs:?}"
    );
    let ops = store_ops(&msgs);
    assert_eq!(
        ops.len(),
        1,
        "an explicit nothing writes the coverage op and nothing else: {msgs:?}"
    );
    assert_eq!(ops[0]["table"], "pending_extraction");
    assert_eq!(
        ops[0]["set"]["status"], "nothing",
        "and the turn is booked as annotated-and-empty, not as never looked at: {ops:?}"
    );
}

#[test]
fn the_shown_movements_are_the_ones_the_script_honours() {
    // (d) The `topic.movement` enum in the block against the branches in the real
    // `params.script_inline`. A fourth value in the block would be a value the
    // lane silently ignores; a missing one would be a lane nothing can reach.
    let block = contract_block();
    let form = json_form(&block, "{\"facts\":");
    let shown: Vec<&str> = form["topic"]["movement"]
        .as_str()
        .expect("the block shows the movement alternatives")
        .split('|')
        .collect();
    assert_eq!(
        shown,
        vec!["start", "continue", "end"],
        "the three movements, in the order the block explains them"
    );

    let script = glue_script();
    let mut branches: Vec<String> = Vec::new();
    let mut rest = script.as_str();
    while let Some(at) = rest.find("movement == \"") {
        let tail = &rest[at + "movement == \"".len()..];
        let (value, after) = tail.split_once('"').expect("closed literal");
        branches.push(value.to_string());
        rest = after;
    }
    branches.sort();
    assert_eq!(
        branches,
        vec!["end".to_string(), "start".to_string()],
        "`start` and `end` are the two the lane WRITES for; anything else in the \
         script is a movement the block never offered"
    );
    // `continue` is the deliberate fall-through: most turns continue the thread
    // they are in, and a row per turn would make the topics table a second
    // episode log. It is in the block because a model needs a value to send, not
    // because it writes anything.
    assert!(
        shown.contains(&"continue"),
        "the ordinary case still needs a name a model can send"
    );
}

#[test]
fn the_shown_fact_shape_is_the_one_the_validator_accepts() {
    // (d), the other half: the fact form. Every key the block shows has to be a
    // key `validate()` reads -- a shown key nobody reads is a token paid for on
    // every turn for nothing -- and the `fact_kind` alternatives have to be the
    // tuple the validator checks against, or a well-formed fact is dropped
    // individually and silently.
    let block = contract_block();
    let form = json_form(&block, "{\"facts\":");
    let fact = &form["facts"][0];
    for key in ["subject", "predicate", "claim", "fact_kind", "valid_from"] {
        assert!(
            fact.get(key).is_some(),
            "the fact form shows {key:?}: {form}"
        );
    }

    let script = glue_script();
    let kinds_line = script
        .lines()
        .find(|l| l.starts_with("KINDS = "))
        .expect("the validator declares the kinds it accepts");
    let kinds: Vec<&str> = kinds_line.split('"').skip(1).step_by(2).collect();
    let shown: Vec<&str> = fact["fact_kind"]
        .as_str()
        .expect("the block shows the kind alternatives")
        .split('|')
        .collect();
    assert_eq!(
        shown, kinds,
        "the kinds the block offers are the kinds `validate()` accepts -- a fact \
         on any other one is dropped, on its own, without a word"
    );
}

#[test]
fn the_block_carries_the_speech_act_rule() {
    // GitHub #67's living half. `plans_to_beat`, `wants_to_move_to` and
    // `hopes_to_visit` are not relations, they are sentences ABOUT relations this
    // memory already has, and a value that lands on one of them is invisible to
    // every question about the matter it updates (the currency question groups by
    // `(canonical_subject, canonical_predicate)`).
    //
    // The rule cannot be repaired afterwards: a `plans_to_*` key cannot be aliased
    // into subject-matter form generically, because the matter varies per
    // statement while an alias is per predicate. It is prevention, it used to be
    // carried by the batch prompt alone, and with that prompt gone this block is
    // the only place a model is ever told.
    let block = flat(&contract_block());
    assert!(
        block.contains("A PREDICATE NAMES THE SUBJECT MATTER, NEVER THE SPEECH ACT"),
        "the block states the subject-matter rule (#67):\n{block}"
    );
    assert!(
        block.contains("fact_kind: foresight"),
        "and where the intention goes instead -- the marker the tier-0 foresight \
         leg has always filtered on, so a plan and the fact it is about share an \
         axis:\n{block}"
    );
}

#[test]
fn the_block_carries_the_core_list_split_by_cardinality() {
    // (e) The drift lock `predicate-core.json` needs. That file is the authority
    // and the block is its copy; a persona cannot import a JSON file at prompt
    // time, so this comparison is the only thing that keeps the two from
    // diverging silently. The cardinality split travels with it because it is
    // what the chain arithmetic is derived from (ruling Q4): `single` replaces,
    // `multi` enumerates, and a relation on the wrong side of that line either
    // loses values or keeps dead ones.
    let block = contract_block();
    assert_eq!(
        block_group(&block, "single"),
        core_group("single"),
        "the block's single-valued group drifted from predicate-core.json"
    );
    assert_eq!(
        block_group(&block, "multi"),
        core_group("multi"),
        "the block's multivalued group drifted from predicate-core.json"
    );
    assert!(
        block.contains("snake_case"),
        "and the style rule itself is stated, not only shown by example:\n{block}"
    );
}

#[test]
fn the_three_retracted_sentences_are_gone() {
    // (f) The removals, asserted as removals. Each of the three was in the block
    // before this wave and each was taken out for a reason that survives being
    // read back:
    //
    // * "an empty facts list is a correct answer" -- a PERMISSION, replaced by the
    //   obligation, which says the same thing from the end that can be checked.
    //   Permission was never the problem.
    // * "CANDIDATE, not a verdict" -- it told the model that something better
    //   informed would redo its work. Nothing does, since #298. A model that
    //   believes it is writing a first draft writes like one.
    // * `valid_until` as an EMITTED field -- a validity taken from the range a
    //   question asks about closes the fact on arrival: invisible to the as-of
    //   leg, visible to keyword and semantic. Measured in a running colony.
    let block = flat(&contract_block());
    for gone in [
        "an empty facts list is a correct answer",
        "CANDIDATE, not a verdict",
        "valid_until",
    ] {
        assert!(
            !block.contains(gone),
            "the block carries {gone:?} again -- it was retracted, not mislaid:\n{block}"
        );
    }
    // The same rule one field over, and for the same reason it was never in the
    // tool schema: an `episodes.id` is a uuid the hive mints while the answer is
    // being generated, so a model that names one names somebody else's turn.
    assert!(
        !block.contains("episode_id"),
        "the block asks for no turn id -- the ingress resolves the turn:\n{block}"
    );
}

#[test]
fn the_block_stays_under_its_length_bound() {
    // (g) A bound, not a measurement. This block was the longest tool description
    // the agent carried and it is carried on EVERY turn, so its length is paid for
    // once per call, forever. The rewrite cut it from 3,302 characters to 1,573 by
    // dropping the run of prohibition paragraphs; without a ceiling the next
    // careful addition and the one after it put them back one sentence at a time.
    let block = contract_block();
    let len = block.chars().count();
    assert!(
        len < LENGTH_BOUND,
        "the contract block is {len} characters, over its {LENGTH_BOUND} bound. \
         Cut something before adding something -- what is in here is what the \
         ingress can actually read, and nothing else."
    );
}

#[test]
fn the_contract_file_points_at_its_consumers() {
    // The one discipline of `f9_inline_contract.rs` that is not about the block's
    // wording, kept because it is the reason the file stays current: an authority
    // file that does not say who reads it becomes a file nobody updates. Same rule
    // `predicate-core.json` follows.
    let raw = inline_contract();
    assert!(
        raw.contains("extract-glue"),
        "the contract names the lane that validates what it asks for"
    );
    assert!(
        raw.contains("gh299_the_contract_asks_for_both_parts.rs"),
        "and the drift lock that holds it -- this file, since the two-lane lock \
         it replaces lost its second lane with the batch prompt (#298)"
    );
}
