//! 0.3.x follow-up F8 -- a turn the inline ingress covered is never extracted a
//! second time (GitHub #52).
//!
//! The hive has two extraction ingresses by design: the front model emits its
//! facts in the answering turn (inline), and the batched extractor picks up the
//! turns of models that emit none. Both pass one validator, one insert path and
//! one `(episode_id, claim_hash)` dedup.
//!
//! What was missing is the coordination. The write lane enqueues EVERY turn, so
//! a turn already handled inline sat in `pending_extraction` waiting to be
//! extracted again. Measured in a production colony: an inline fact six seconds
//! after the message, and the same episode extracted again 33 minutes later --
//! a second fact about the same content on a second predicate spelling. The
//! dedup cannot catch that: it compares claim bytes, and two models never phrase
//! one claim identically, so the duplicate lands on a new axis and the chain
//! arithmetic runs on the wrong collective.
//!
//! The fix is a mark, not a filter: the inline ingress closes the queue row of
//! every episode its block covers, with its own status so the books say WHO
//! handled the turn. An EMPTY facts block covers its episode too -- an empty
//! list is the front model's verdict that nothing was memorable, not an absence
//! of one.
//!
//! Since GH #607 the block arrives in TWO shapes and this file measures both.
//! The one the helpers use by default is the `memory` SECTION of the single
//! fenced block the front model appends to its answer, delivered by the splitter
//! as `body.payload` with `hop.section == "memory"`; the older shape -- the
//! fence's own text written into the first turn -- is `legacy_inline`, and
//! `the_legacy_block_in_a_turn_and_the_sidecar_section_are_one_ingress` proves
//! the lane cannot tell them apart once it holds the annotation. Everything else
//! in this file is about coverage and is written against the shape that ships.
//!
//! Everything here runs the REAL `params.script_inline` of `extract-glue`
//! against injected replies, so nothing costs anything.

use std::io::Write;
use std::process::{Command, Stdio};

const GLUE_CONFIG: &str = "../../templates/memory-hive/extract-glue/config.json";

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

/// The room these blocks are written in. One room for the whole file: nothing
/// here is about a second channel.
const CHANNEL: &str = "c-f8";

/// Who was present when the front model wrote the block: the person whose turn
/// it answers and the agent that answered. That is the whole cast of an inline
/// extraction -- the block is emitted IN the answering turn, so the round of
/// that turn is its audience, and it is exactly the round the facts here are
/// about (`subject: "user"`). Not `["*"]`: a universal set would make every
/// case below pass against a write path with no gate at all.
const AUDIENCE: &str = r#"["member:user","agent:assistant"]"#;

/// One inline block as the port edge delivers it, in the shape it has had since
/// GH #607: `store_origin`/`mem_phase` in the context, the provenance the gate
/// requires next to them (#244 -- `audience_set` and `channel` are promoted by
/// the edge that carries a turn into the hive, the same way a real colony's port
/// edge promotes them), and the annotation itself as the body's `payload`.
///
/// That is the `memory` SECTION of the one fenced block the front model appends
/// to its answer. The splitter cuts the fence into one message per section and
/// hands each one over on route `sidecar` with `hop.section` naming the key; the
/// member's own edge restamps the route to `in_remember` on the way into the
/// hive and leaves the section alone, which is exactly what arrives here. The
/// section object IS the annotation this lane has always validated -- one level
/// down inside the fence, identical once unwrapped.
fn inline(payload: &str) -> serde_json::Value {
    let annotation: serde_json::Value =
        serde_json::from_str(payload).expect("the block under test is json");
    serde_json::json!({
        "header": {
            "context": {"store_origin": "inline", "mem_phase": "inline",
                        "audience_set": AUDIENCE, "channel": CHANNEL},
            "hop": {"route": "in_remember", "section": "memory"}
        },
        "messages": [],
        "section": "memory",
        "payload": annotation
    })
}

/// The SAME block in the shape this lane read until GH #607: the fence's own
/// text, written into the first turn of `messages[]`, with no `payload` and no
/// section anywhere.
///
/// It is still delivered, and it still has to work. The rebuild of the fence is
/// two-phased -- a colony wired to an older surface, a replay of a message
/// written last week, an operator probe -- and a lane that read only the new
/// shape would refuse every one of them as "not JSON" while the answer went out
/// unaffected and nothing in the books said the memory was never written.
fn legacy_inline(payload: &str) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "inline", "mem_phase": "inline",
                        "audience_set": AUDIENCE, "channel": CHANNEL},
            "hop": {}
        },
        "messages": [{"origin": "user", "type": "text", "text": payload}]
    })
}

/// A uuid v4 in its canonical spelling, `8-4-4-4-12` hex.
fn is_uuid(s: &str) -> bool {
    let dashes = [8usize, 13, 18, 23];
    s.len() == 36
        && s.char_indices().all(|(i, c)| {
            if dashes.contains(&i) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

/// The head of an RFC-3339 instant this century, `20dd-dd-ddT`.
fn is_instant_head(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 11
        && &b[..2] == b"20"
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && [2, 3, 5, 6, 8, 9].iter().all(|&i| b[i].is_ascii_digit())
}

/// Every uuid and every instant replaced by a fixed word, so one block run
/// through two deliveries can be compared as a DOCUMENT. Without it the batch
/// key, the scratch row id and the ingest instant differ on every run and no two
/// emissions are ever equal.
fn stable(msgs: &[serde_json::Value]) -> String {
    let s = serde_json::to_string(msgs).expect("messages are json");
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < s.len() {
        if i + 36 <= s.len() && s.is_char_boundary(i + 36) && is_uuid(&s[i..i + 36]) {
            out.push_str("UUID");
            i += 36;
            continue;
        }
        if i + 11 <= s.len() && s.is_char_boundary(i + 11) && is_instant_head(&s[i..i + 11]) {
            out.push_str("WHEN");
            // an instant is always inside a json string, so the closing quote
            // is the end of it
            i += s[i..].find('"').expect("an instant closes its string");
            continue;
        }
        let c = s[i..].chars().next().expect("a char at a boundary");
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// GH #607 -- the two shapes are ONE ingress, and the proof is that the same
/// block produces the same document through both of them.
///
/// This is the whole of the two-phase rebuild in one assertion. It is not "the
/// new form works" and "the old form works" side by side: those two could drift
/// apart on a coverage status, a topic op or a staged payload and both tests
/// would stay green. What must hold is that the lane cannot tell which shape it
/// was handed once it has the annotation, and the only way to say that is to run
/// one block twice and compare everything that leaves.
#[test]
fn the_legacy_block_in_a_turn_and_the_sidecar_section_are_one_ingress() {
    let block = serde_json::json!({
        "episode_id": "e-both",
        "facts": [{"episode_id": "e-both", "subject": "user", "predicate": "favorite_color",
                   "claim": "Blau", "fact_kind": "world", "confidence": 90}],
        "topic": {"movement": "open", "name": "colours"}
    })
    .to_string();

    let new_shape = emit(inline(&block));
    let old_shape = emit(legacy_inline(&block));

    assert!(
        queue_op(&new_shape).is_some(),
        "the sidecar section is read at all: {new_shape:?}"
    );
    assert_eq!(
        stable(&new_shape),
        stable(&old_shape),
        "one annotation, two deliveries, and the lane emitted two different \
         things. The `memory` section of a sidecar block IS the block the older \
         form carried as text -- one level down inside the fence and already \
         parsed -- so everything after the read is the same validator, the same \
         gates and the same ops. A difference here means the rebuild is not \
         two-phased and a colony mid-flip writes two different memories"
    );
}

fn args_of(msg: &serde_json::Value) -> serde_json::Value {
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    serde_json::from_str(text).expect("op args")
}

/// The queue op this block emitted, or `None` if it emitted none.
fn queue_op(msgs: &[serde_json::Value]) -> Option<serde_json::Value> {
    msgs.iter()
        .filter(|m| m["header"]["route"] == "xstore")
        .map(args_of)
        .find(|a| a["table"] == "pending_extraction")
}

/// The episode ids a queue op takes out of the queue, sorted.
fn covered(op: &serde_json::Value) -> Vec<String> {
    let mut out: Vec<String> = op["where"]["episode_id"]["in"]
        .as_array()
        .unwrap_or_else(|| panic!("the coverage op filters on an episode_id list, got {op}"))
        .iter()
        .map(|v| v.as_str().expect("episode id").to_string())
        .collect();
    out.sort();
    out
}

/// The payload the lane staged for the dedup phase, or `None`.
fn staged(msgs: &[serde_json::Value]) -> Option<serde_json::Value> {
    msgs.iter()
        .filter(|m| m["header"]["route"] == "xstore")
        .map(args_of)
        .find(|a| a["table"] == "scratch" && a["row"]["kind"] == "payload")
}

fn one_fact(episode: &str) -> String {
    serde_json::json!({
        "episode_id": episode,
        "facts": [{"episode_id": episode, "subject": "user", "predicate": "favorite_color",
                   "claim": "Blau", "fact_kind": "world", "confidence": 90}]
    })
    .to_string()
}

#[test]
fn an_inline_block_takes_the_turn_it_covered_out_of_the_queue() {
    // The mark, and the whole of #52: the batch lane reads `status = 'pending'`
    // everywhere -- the gate, the claim guard, the flush -- so a row that leaves
    // that status is a row no batch can ever offer the extractor again.
    let msgs = emit(inline(&one_fact("e1")));
    let op = queue_op(&msgs).expect("the inline ingress closes the queue row it covered");
    assert_eq!(op["operation"], "update");
    assert_eq!(covered(&op), vec!["e1".to_string()]);
    // and the facts still travel: coverage is an extra op, never a replacement
    // for the ingress the block came in through.
    assert!(
        staged(&msgs).is_some(),
        "the validated payload is still staged: {msgs:?}"
    );
}

#[test]
fn the_mark_says_who_handled_the_turn_and_guards_on_the_row_it_claims() {
    // Two properties of one op. The status is the lane's OWN, because a queue row
    // that says `done` cannot tell an operator whether a model was paid for it --
    // and because it is what lets a scenario assert the skip instead of guessing
    // it from an absence. The guard is `status = 'pending'`, because a row a batch
    // has already claimed is being extracted right now: marking it would rewrite
    // the books of work that is happening rather than prevent work that is not.
    let msgs = emit(inline(&one_fact("e1")));
    let op = queue_op(&msgs).expect("coverage op");
    assert_eq!(
        op["set"]["status"], "inline",
        "the queue row names the lane that handled it"
    );
    assert_eq!(
        op["where"]["status"], "pending",
        "a claimed row is left to the batch that owns it"
    );
}

#[test]
fn an_empty_facts_block_is_a_verdict_and_covers_its_turn() {
    // The half the issue is explicit about: an empty list is the front model
    // saying "nothing here", and re-asking a second model the same question is
    // exactly the duplicate this package exists to stop. So the turn is covered
    // -- and covered with `nothing` rather than `inline` (wave 5, #298).
    //
    // The verdict used to leave through the `reject` port as well, on the
    // grounds that there was nothing to insert. It no longer does, and the
    // reason is the one #298 is about: a considered silence that travels on the
    // refusal lane is indistinguishable from a refusal. Same route, same absence
    // of a write -- so nothing downstream could tell "a model read this turn and
    // found nothing" from "this block was garbage", and the queue could not tell
    // an un-annotated turn (`pending`, the exception worth looking at) from an
    // annotated-as-empty one. The answer is the emission now; only a block this
    // lane genuinely cannot read still refuses (see
    // `a_block_that_is_not_json_covers_nothing` below).
    let payload = serde_json::json!({"episode_id": "e7", "facts": []}).to_string();
    let msgs = emit(inline(&payload));
    let op = queue_op(&msgs).expect("an empty block still covers its episode");
    assert_eq!(covered(&op), vec!["e7".to_string()]);
    assert_eq!(
        op["set"]["status"], "nothing",
        "an annotated-as-empty turn is its own status, not the one a block with content writes"
    );
    assert!(
        !msgs.iter().any(|m| m["header"]["route"] == "reject"),
        "an answer is not a refusal: {msgs:?}"
    );
    assert!(
        staged(&msgs).is_none(),
        "and nothing is staged for the fact lane"
    );
}

#[test]
fn a_block_that_is_not_json_covers_nothing() {
    // The direction that must stay safe. Garbage is not a verdict about a turn:
    // it names no episode, so it proves nothing about one, and a turn nobody
    // extracted must stay in the queue. Zero store writes, exactly as the lane's
    // contract has always said.
    //
    // Delivered in the OLDER shape, and that is the only shape it can have
    // (GH #607): a sidecar section that is not an object never leaves the
    // splitter -- it is dropped there with `hop.sidecar_dropped` -- so the one
    // way unreadable bytes still reach this ingress is as the text of a turn.
    let msgs = emit(legacy_inline("not json at all"));
    assert!(
        queue_op(&msgs).is_none(),
        "garbage covers no episode: {msgs:?}"
    );
    assert!(
        msgs.iter().all(|m| m["header"]["route"] == "reject"),
        "garbage leaves through the reject port and writes nothing: {msgs:?}"
    );
}

#[test]
fn a_block_without_an_episode_covers_nothing() {
    // Same rule one step in: a payload whose facts name no turn cannot say which
    // turn was handled. The facts are still valid and still travel -- only the
    // queue is left alone.
    let payload = serde_json::json!({
        "facts": [{"subject": "user", "predicate": "favorite_color", "claim": "Blau",
                   "fact_kind": "world", "confidence": 90}]
    })
    .to_string();
    let msgs = emit(inline(&payload));
    assert!(
        queue_op(&msgs).is_none(),
        "no episode named, no coverage claimed: {msgs:?}"
    );
    assert!(staged(&msgs).is_some(), "the facts still travel");
}

#[test]
fn every_turn_the_block_names_is_covered_at_once() {
    // A front model answering after several turns emits ONE block for all of
    // them, and the ids sit on the facts rather than on the payload. Covering
    // only the payload's own id would leave the others in the queue and buy the
    // duplicate for exactly the turns that carried the most.
    let payload = serde_json::json!({
        "episode_id": "e1",
        "facts": [
            {"episode_id": "e2", "subject": "user", "predicate": "lives_in",
             "claim": "Elvese", "fact_kind": "world", "confidence": 90},
            {"episode_id": "e3", "subject": "user", "predicate": "diet",
             "claim": "isst ketogen", "fact_kind": "world", "confidence": 85}
        ]
    })
    .to_string();
    let msgs = emit(inline(&payload));
    let op = queue_op(&msgs).expect("coverage op");
    assert_eq!(
        covered(&op),
        vec!["e1".to_string(), "e2".to_string(), "e3".to_string()],
        "the payload's own turn and every turn its facts name"
    );
}
