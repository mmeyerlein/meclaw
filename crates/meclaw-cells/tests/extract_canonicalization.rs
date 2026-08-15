//! 0.2.0 P1 -- the extraction lane mints CANONICAL predicates (rulings Q1, Q2, Q4).
//!
//! The measured defect: every turn opened its own axis. `favorite editor`,
//! `Lieblingseditor` and `favorite_editor` are three axes for one relation, so the
//! version chain never fires (5 percent chain-fire rate) and a knowledge update
//! reads as two unrelated facts.
//!
//! Three guarantees are pinned here, all against the REAL `params.script_inline`
//! of `extract-glue` (P5 pattern -- never a copy):
//!
//! 1. The lane READS before it prompts: the predicates this memory already uses
//!    are fetched from the store and handed to the extractor. The read sits on the
//!    asynchronous extraction lane; the write hot path stays round-trip free.
//! 2. The prompt carries the curated core list WITH its cardinality split, byte
//!    for byte the one in `predicate-core.json` -- that file is the authority and
//!    this is its drift lock (the script cannot import it at runtime).
//! 3. The prompt carries the entity-fidelity rule: predicates are translated into
//!    canonical English, subjects/objects/proper names never are.

use std::io::Write;
use std::process::{Command, Stdio};

const GLUE_CONFIG: &str = "../../templates/memory-hive/extract-glue/config.json";
const CORE_LIST: &str = "../../templates/memory-hive/predicate-core.json";

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

fn core_list() -> serde_json::Value {
    let raw = std::fs::read_to_string(CORE_LIST).expect("predicate-core.json");
    serde_json::from_str(&raw).expect("core list json")
}

/// Predicates of one cardinality group, as the authority file declares them.
fn core_group(kind: &str) -> Vec<String> {
    let list = core_list();
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

/// Run the real script with a real stdin document and return the emitted messages.
fn emit(doc: serde_json::Value) -> Vec<serde_json::Value> {
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(glue_script())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(&meclaw_testing::code_stdin_bytes(&doc))
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait");
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

/// One store reply as the edge delivers it: `mem_phase`/`batch_id` in the context,
/// `operation` in the hop, the projected rows as the first message's text.
fn store_reply(phase: &str, operation: &str, rows: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "extract", "mem_phase": phase, "batch_id": "b1"},
            "hop": {"operation": operation, "rows_affected": 1}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r", "text": rows.to_string()}]
    })
}

fn args_of(msg: &serde_json::Value) -> serde_json::Value {
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    serde_json::from_str(text).expect("op args")
}

/// The op that parks one scratch kind. Since W4 (#13, ruling Q2 option C) the
/// `vocab` phase emits TWO of them -- the axis hint this file is about and the
/// replacement window, which is pinned in `w4_extract_replaces.rs` -- so the
/// vocabulary assertions name their row instead of indexing into the emit list.
fn parked_op(msgs: &[serde_json::Value], kind: &str) -> serde_json::Value {
    msgs.iter()
        .map(args_of)
        .find(|a| a["table"] == "scratch" && a["row"]["kind"] == kind)
        .unwrap_or_else(|| panic!("no scratch row of kind {kind} was parked"))
}

fn claimed_rows() -> serde_json::Value {
    serde_json::json!([
        {"id": "q1", "episode_id": "e1", "sender": "user",
         "content": "Meine Lieblingsfarbe ist Blau.", "status": "claimed"}
    ])
}

/// The full instructions text the extractor would receive for a batch that meets
/// `vocab` as the memory's existing axes.
fn instructions_for(vocab: serde_json::Value) -> String {
    let items = serde_json::json!([
        {"episode_id": "e1", "sender": "user", "content": "Meine Lieblingsfarbe ist Blau."}
    ]);
    let scratch = serde_json::json!([
        {"key": "b1", "kind": "batch", "payload": items.to_string()},
        {"key": "b1", "kind": "vocab", "payload": vocab.to_string()}
    ]);
    let msgs = emit(store_reply("prompt", "select", scratch));
    assert_eq!(msgs.len(), 1, "the prompt phase emits exactly one message");
    assert_eq!(msgs[0]["header"]["route"], "extract");
    msgs[0]["system"]["instructions"]["text"]
        .as_str()
        .expect("instructions text")
        .to_string()
}

/// The `  <kind>: a, b, c` line of the rendered core list.
fn rendered_group(instructions: &str, kind: &str) -> Vec<String> {
    let needle = format!("{kind}:");
    let line = instructions
        .lines()
        .find(|l| l.trim_start().starts_with(&needle))
        .unwrap_or_else(|| panic!("no '{kind}:' line in the instructions:\n{instructions}"));
    let (_, tail) = line.split_once(':').expect("colon");
    let mut out: Vec<String> = tail
        .split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    out.sort();
    out
}

#[test]
fn the_batch_is_parked_and_the_lane_asks_for_the_existing_predicates() {
    // A code cell holds no state, so the batch has to survive the extra store
    // round trips -- parked in `scratch` exactly like the dedup lane parks its
    // payload (DEC-009). Without the park the vocabulary read would eat the batch.
    let msgs = emit(store_reply("batch", "select", claimed_rows()));
    assert_eq!(msgs.len(), 1, "one op, not a fan-out");
    assert_eq!(msgs[0]["header"]["route"], "xstore");
    assert_eq!(msgs[0]["header"]["phase"], "vocab-fetch");
    let args = args_of(&msgs[0]);
    assert_eq!(args["operation"], "insert");
    assert_eq!(args["table"], "scratch");
    assert_eq!(args["row"]["kind"], "batch");
    assert_eq!(args["row"]["key"], "b1");
    let parked: serde_json::Value =
        serde_json::from_str(args["row"]["payload"].as_str().expect("payload")).expect("items");
    assert_eq!(parked[0]["content"], "Meine Lieblingsfarbe ist Blau.");
}

#[test]
fn the_vocabulary_is_read_from_the_facts_table() {
    // Q1: the extractor receives the subject's existing predicates. The read is
    // legal HERE and only here -- the extraction lane is asynchronous, the write
    // hot path never grows a round trip.
    //
    // 0.2.0 P5: it reads the CANONICAL columns. The GC writes aliases at night;
    // a vocabulary read on the written spellings would show the extractor the old
    // ones the very next morning and it would keep minting them -- the nightly
    // merge and the daily mint would work against each other. The written columns
    // travel along as the fallback for rows from before the migration.
    //
    // GH #68: the hint has its own read since the vocabulary leg was paged, and it
    // is the LAST of the leg's three -- so this asks for it where it is emitted,
    // behind the window's page.
    let msgs = emit(store_reply("window-page", "select", serde_json::json!([])));
    let args = msgs
        .iter()
        .map(args_of)
        .find(|a| a["operation"] == "select")
        .expect("the axis hint is read");
    assert_eq!(args["table"], "facts");
    let cols: Vec<&str> = args["columns"]
        .as_array()
        .expect("columns")
        .iter()
        .map(|c| c.as_str().expect("column"))
        .collect();
    for needed in [
        "canonical_subject",
        "canonical_predicate",
        "subject",
        "predicate",
    ] {
        assert!(
            cols.contains(&needed),
            "the vocabulary leg needs {needed}, got {cols:?}"
        );
    }
}

#[test]
fn the_vocabulary_is_grouped_per_subject_and_deduplicated() {
    let rows = serde_json::json!([
        {"subject": "user", "canonical_subject": "user",
         "predicate": "favorite_color", "canonical_predicate": "favorite_color"},
        {"subject": "user", "canonical_subject": "user",
         "predicate": "favorite_color", "canonical_predicate": "favorite_color"},
        {"subject": "user", "canonical_subject": "user",
         "predicate": "lives_in", "canonical_predicate": "lives_in"},
        {"subject": "cat:nala", "canonical_subject": "cat:nala",
         "predicate": "has_pet", "canonical_predicate": "has_pet"},
        {"subject": "", "canonical_subject": "", "predicate": "dropped",
         "canonical_predicate": "dropped"}
    ]);
    let msgs = emit(store_reply("vocab", "select", rows));
    let args = parked_op(&msgs, "vocab");
    let vocab: serde_json::Value =
        serde_json::from_str(args["row"]["payload"].as_str().expect("payload")).expect("vocab");
    assert_eq!(
        vocab["user"],
        serde_json::json!(["favorite_color", "lives_in"]),
        "one entry per axis, sorted -- a duplicate would spend prompt budget twice"
    );
    assert_eq!(vocab["cat:nala"], serde_json::json!(["has_pet"]));
    assert!(
        vocab.get("").is_none(),
        "a row without a subject carries no axis"
    );
}

#[test]
fn the_vocabulary_shows_the_identity_the_gc_settled_on() {
    // 0.2.0 P5, the point of the change: two rows written under two spellings of
    // one relation and one entity, already merged by a nightly judgement. The
    // extractor must be shown ONE axis under the settled names -- being shown the
    // originals is how a merged axis drifts apart again the next morning.
    let rows = serde_json::json!([
        {"subject": "user:u1", "canonical_subject": "user",
         "predicate": "Lieblingseditor", "canonical_predicate": "favorite_editor"},
        {"subject": "user", "canonical_subject": "user",
         "predicate": "favorite editor", "canonical_predicate": "favorite_editor"},
        // a row from before the migration keeps its written axis rather than
        // disappearing into an empty bucket (the fallback of axis_key/subject_key)
        {"subject": "cat:nala", "predicate": "has_pet"}
    ]);
    let msgs = emit(store_reply("vocab", "select", rows));
    let args = parked_op(&msgs, "vocab");
    let vocab: serde_json::Value =
        serde_json::from_str(args["row"]["payload"].as_str().expect("payload")).expect("vocab");
    assert_eq!(vocab["user"], serde_json::json!(["favorite_editor"]));
    assert!(
        vocab.get("user:u1").is_none(),
        "the merged spelling is not a second subject any more"
    );
    assert_eq!(vocab["cat:nala"], serde_json::json!(["has_pet"]));
}

#[test]
fn the_prompt_carries_the_core_list_split_by_cardinality() {
    // The drift lock: `predicate-core.json` is the authority, the literal in the
    // script is its copy. A script cannot import at runtime, so this comparison
    // is the only thing that keeps the two from diverging silently -- and the
    // cardinality split is what P2 derives the chain rules from (ruling Q4).
    let text = instructions_for(serde_json::json!({}));
    assert_eq!(
        rendered_group(&text, "single"),
        core_group("single"),
        "the prompt's single-valued group drifted from predicate-core.json"
    );
    assert_eq!(
        rendered_group(&text, "multi"),
        core_group("multi"),
        "the prompt's multivalued group drifted from predicate-core.json"
    );
    assert!(
        text.contains("snake_case"),
        "the style rule itself has to be in the prompt, not only its examples"
    );
}

#[test]
fn the_prompt_names_the_axes_this_memory_already_uses() {
    // The core list alone cannot canonicalise a predicate it does not contain.
    // Reuse of what is already there is the other half of Q1, and the deliberately
    // unlisted `brews_coffee_with` is the discriminator: it can only come from the
    // store.
    let text = instructions_for(serde_json::json!({
        "user": ["brews_coffee_with", "favorite_color"]
    }));
    assert!(
        text.contains("brews_coffee_with"),
        "an existing axis of the subject is missing from the prompt:\n{text}"
    );
    assert!(
        text.contains("user"),
        "the axes are listed per subject, so the subject has to be named:\n{text}"
    );
}

#[test]
fn an_empty_memory_prompts_with_the_core_list_alone() {
    // First mint: no axis exists yet, and the prompt must not claim otherwise.
    let text = instructions_for(serde_json::json!({}));
    assert!(text.contains("favorite_color"));
    assert!(
        !text.contains("brews_coffee_with"),
        "nothing invented for an empty store"
    );
}

#[test]
fn the_prompt_forbids_translating_entities() {
    // Q2 entity-fidelity rule: the small closed class of relation patterns becomes
    // canonical English, everything a turn NAMES stays byte-faithful. A model that
    // "corrects" an unfamiliar village into a familiar one destroys the fact.
    let text = instructions_for(serde_json::json!({}));
    let lower = text.to_lowercase();
    for needle in ["never translate", "verbatim", "spell"] {
        assert!(
            lower.contains(needle),
            "the entity-fidelity rule is missing {needle:?}:\n{text}"
        );
    }
}

#[test]
fn the_batch_items_reach_the_extractor_unchanged() {
    // The park-and-reread detour must not touch the payload: the extractor sees
    // the same item list the old one-hop lane handed it.
    let items = serde_json::json!([
        {"episode_id": "e1", "sender": "user", "content": "I come from Elvese."}
    ]);
    let scratch = serde_json::json!([
        {"key": "b1", "kind": "batch", "payload": items.to_string()},
        {"key": "b1", "kind": "vocab", "payload": "{}"}
    ]);
    let msgs = emit(store_reply("prompt", "select", scratch));
    let sent: serde_json::Value =
        serde_json::from_str(msgs[0]["messages"][0]["text"].as_str().expect("items text"))
            .expect("items");
    assert_eq!(sent, items);
    assert_eq!(msgs[0]["header"]["batch_id"], "b1");
}

#[test]
fn an_incomplete_meeting_point_parks_instead_of_closing_the_batch() {
    // The detour buys a failure mode the one-hop lane did not have: the parked
    // batch could be missing from the meeting point. Closing there would mark
    // turns 'done' that were never extracted although they are still on disk --
    // so the lane parks, the rows stay 'claimed', and `flush_reclaim` recovers
    // them. Same rule as the apply phase (DEC-009).
    let scratch = serde_json::json!([{"key": "b1", "kind": "vocab", "payload": "{}"}]);
    let msgs = emit(store_reply("prompt", "select", scratch));
    assert!(
        msgs.is_empty(),
        "a batch that lost its items must not be closed and must not be prompted: {msgs:?}"
    );
}

#[test]
fn the_o6_discipline_survives_the_new_prompt() {
    // P15 O-6 was bought with two red runs. The canonicalisation block is added to
    // that prompt, never in place of it.
    let text = instructions_for(serde_json::json!({}));
    assert!(text.contains("WORLD STATE"));
    assert!(text.contains("asked_about"));
    assert!(text.contains("fact_kind"));
}
