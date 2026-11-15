//! Wave d58-3, strand `schnell` (GH #1529, contract point 1) -- a fact that
//! arrives while the cache is warm lands on top of the window; the next
//! rebuild sorts it into the system part.
//! OR-583.stapel.8, GH #1333: the element stands directly before the person's
//! words of the running turn (at the end when the turn holds none), and a call
//! without a sent prefix carries none.
//!
//! WHY this file exists: the owner's line for the curator (11.11.): "when
//! facts or tools come in dynamically and the cache is warm, it puts these
//! things on top of the context window, and only at a cache miss does it sort
//! them into the system part". Since GH #1517 (d) a slot the curator writes
//! itself that arrives after the target's window was first cut on a state of
//! the plan is only held back to the next change of plan: the system part
//! stays byte for byte, but the model does not see the new fact until that
//! change comes (measured: the slot `instructions.hygiene` came home between
//! the first and the second call of a freshly reset session). Now the
//! curator shows such a fact at once, as one element directly before the
//! person's words of the running turn -- behind everything the cached prefix
//! of the turns before holds, while the words stay the last message before
//! the answer (GH #1333; measured in member_road gh953/gh954: with the element
//! behind the words the model answered a line of the curator) -- and the system
//! part stays the same tree; the next rebuild (here: the change of plan the
//! fact ordered, mark `catalog`) moves it into the system part, and the
//! element on top is gone. A call without a sent prefix -- the first after the
//! boot -- holds nothing back: what it has stands in its system part.
//!
//! A fact here is a slot of the system part whose row names the curator as
//! its `owner` -- the one column that tells such a slot from a contract of
//! another owner (GH #1517 (d)); the rule runs on that column and the kind
//! of block, never on a slot's name.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

/// The path of the fact's slot and its text.
const FACT: &str = "facts.today";
const FACT_TEXT: &str = "The person's daughter Lena visits on Sunday afternoon.";

/// The block of the fact's leaf and the slot row naming it, booked for the
/// curator as its owner (the form `./push` books its own slot in).
fn the_curator_writes_the_fact(h: &Hive) {
    the_curator_writes(h, FACT, FACT_TEXT);
}

/// The block of the leaf `path` saying `text`; its hash.
fn the_block(h: &Hive, path: &str, text: &str) -> String {
    let body = canonical(&json!({"path": path, "text": text}));
    let hash = sha256_hex(&body);
    h.db.execute(
        "INSERT INTO blocks (hash, kind, chars, body, first_seen) VALUES (?1, 'system', ?2, ?3, 'x')",
        rusqlite::params![hash, text.len() as i64, body],
    )
    .expect("the slot's block");
    hash
}

/// A slot of the curator's own at `path` saying `text`.
fn the_curator_writes(h: &Hive, path: &str, text: &str) {
    let hash = the_block(h, path, text);
    h.db.execute(
        "INSERT INTO slots (path, hash, owner, at) VALUES (?1, ?2, 'curator', 'x')",
        [path, hash.as_str()],
    )
    .expect("the slot's row");
}

/// The curator's slot at `path` changed to say `text` (its row names the
/// new block).
fn the_curator_changes(h: &Hive, path: &str, text: &str) {
    let hash = the_block(h, path, text);
    let n =
        h.db.execute(
            "UPDATE slots SET hash = ?2 WHERE path = ?1",
            [path, hash.as_str()],
        )
        .expect("the slot's row");
    assert_eq!(n, 1, "check of the setup: the slot's row stood");
}

/// The messages of `call`'s window that carry `text`.
fn messages_with(h: &Hive, call: &Msg, text: &str) -> Vec<usize> {
    window(h, call)
        .messages
        .iter()
        .enumerate()
        .filter(|(_, m)| sj::to_string(m).unwrap_or_default().contains(text))
        .map(|(i, _)| i)
        .collect()
}

/// The place of the element on top in `call`'s window (OR-583.stapel.8, GH
/// #1333): directly before the person's words of the running turn (`words`),
/// which stay the last message; the last place when the window holds none.
fn on_top_place(h: &Hive, call: &Msg, words: &str) -> usize {
    let n = window(h, call).messages.len();
    match messages_with(h, call, words).last() {
        Some(&i) => {
            assert_eq!(
                i + 1,
                n,
                "the person's words are the last message before the answer (GH #1333): {:?}",
                window(h, call).messages
            );
            i.saturating_sub(1)
        }
        None => n.saturating_sub(1),
    }
}

/// The window the model holds for `call`, put together from its snapshot.
fn window(h: &Hive, call: &Msg) -> SentWindow {
    let sent = sent_of(h, call)
        .unwrap_or_else(|| panic!("check of the setup: a snapshot of {:?}", call.hop));
    sent_window(h, &sent)
}

/// Whether the system tree the model holds for `call` holds the fact.
fn fact_in_system(h: &Hive, call: &Msg) -> bool {
    window(h, call).leaves.iter().any(|(path, _)| path == FACT)
}

/// The messages of `call`'s window that carry the fact's text.
fn fact_messages(h: &Hive, call: &Msg) -> Vec<usize> {
    window(h, call)
        .messages
        .iter()
        .enumerate()
        .filter(|(_, m)| sj::to_string(m).unwrap_or_default().contains(FACT_TEXT))
        .map(|(i, _)| i)
        .collect()
}

/// A conversation on the form: the first turn goes out before the fact, the
/// curator writes the fact, the second turn goes on the same state of the
/// plan (no rebuild between them).
fn warm_session() -> (Talk, Msg, Msg) {
    let mut t = Talk::new(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
    ]);
    t.h.target = Some(MAIN_TARGET.to_string());
    let c1 = t.turn("t1", "Good morning.", "Good morning to you.");
    assert!(
        !fact_in_system(&t.h, &c1) && fact_messages(&t.h, &c1).is_empty(),
        "check of the setup: the first call went out before the fact"
    );
    the_curator_writes_the_fact(&t.h);
    let c2 = t.turn("t2", "What is on this week?", "Let me think.");
    assert_eq!(
        cut_epoch(&t.h, &c1),
        cut_epoch(&t.h, &c2),
        "check of the setup: no rebuild lay between the two calls"
    );
    (t, c1, c2)
}

/// Pin: the fact that came while the cache was warm stands directly before
/// the person's words of the second call (OR-583.stapel.8, GH #1333) -- the
/// model sees it at once --
/// while the system part is the tree of the first call (the watch counts no
/// prefix break, the fact is no leaf of it).
#[test]
fn gh_d583_schnell_a_fact_in_a_warm_session_lands_on_top() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let (t, _, c2) = warm_session();
    assert!(
        t.breaks().is_empty(),
        "the second call begins with the window of the first: {:?}",
        t.breaks()
    );
    assert!(
        !fact_in_system(&t.h, &c2),
        "the system part stays byte for byte: the fact is no leaf of it"
    );
    let w = window(&t.h, &c2);
    let on_top = fact_messages(&t.h, &c2);
    assert_eq!(
        on_top,
        vec![on_top_place(&t.h, &c2, "What is on this week?")],
        "the fact stands once, directly before the person's words: {:?}",
        w.messages
    );
    assert!(
        sj::to_string(&c2.messages())
            .unwrap_or_default()
            .contains(FACT_TEXT),
        "the call itself carries the fact to the model: {:?}",
        c2.messages()
    );
}

/// Pin (OR-583.stapel.8 rule 1): a call without a sent prefix carries no
/// element on top -- the first call after the boot holds nothing back, the
/// fact that is there stands in its system part, and the person's words are
/// the last message (GH #1333).
#[test]
fn gh_d583_stapel_the_first_call_after_the_boot_carries_no_element_on_top() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::new(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
    ]);
    t.h.target = Some(MAIN_TARGET.to_string());
    the_curator_writes_the_fact(&t.h);
    let c1 = t.turn("t1", "Good morning.", "Good morning to you.");
    let w = window(&t.h, &c1);
    assert!(
        fact_in_system(&t.h, &c1),
        "the fact stands in the system part of the first call: {:?}",
        w.leaves
    );
    assert!(
        messages_with(&t.h, &c1, "new since this window began").is_empty()
            && fact_messages(&t.h, &c1).is_empty(),
        "the first call after the boot carries no element on top: {:?}",
        w.messages
    );
    assert_eq!(
        messages_with(&t.h, &c1, "Good morning.").last().copied(),
        Some(w.messages.len().saturating_sub(1)),
        "the person's words are the last message (GH #1333): {:?}",
        w.messages
    );
}

/// Pin: the next rebuild sorts the fact into the system part -- after the
/// change of plan the fact ordered, the next call holds it as a leaf and no
/// element at the end carries it any more; the watch counts no break.
#[test]
fn gh_d583_schnell_the_next_rebuild_moves_it_into_the_system() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let (mut t, _, c2) = warm_session();
    assert_eq!(
        fact_messages(&t.h, &c2).len(),
        1,
        "before the rebuild the fact stands on top of the window: {:?}",
        window(&t.h, &c2).messages
    );
    let order =
        t.h.clock_order_for(ROUND_E)
            .unwrap_or_else(|| panic!("an order at the clock: {:?}", t.h.clock));
    assert_eq!(
        order.body["emit_body"]["reason"],
        json!("catalog"),
        "the fact orders the change of plan: {:?}",
        order.body
    );
    t.h.fire(&order);
    let c3 = t.turn("t3", "And on Sunday?", "Lena comes.");
    assert_ne!(
        cut_epoch(&t.h, &c3),
        cut_epoch(&t.h, &c2),
        "check of the setup: the next turn is cut on the new state of the plan"
    );
    assert!(
        fact_in_system(&t.h, &c3),
        "after the rebuild the fact is a leaf of the system part"
    );
    assert!(
        fact_messages(&t.h, &c3).is_empty(),
        "after the rebuild no element at the end carries the fact: {:?}",
        window(&t.h, &c3).messages
    );
}

/// Pin (review M-1 of the strand, contract point 1): a fact that CHANGED
/// while the cache was warm (its slot stood at the first call) goes on top
/// with its new text, and the system part keeps the block it was cut with.
#[test]
fn gh_d583_schnell_a_changed_fact_shows_its_new_text_on_top() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    const NEW_TEXT: &str = "The person's daughter Lena visits on Saturday instead.";
    let mut t = Talk::new(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
    ]);
    t.h.target = Some(MAIN_TARGET.to_string());
    the_curator_writes_the_fact(&t.h);
    let c1 = t.turn("t1", "Good morning.", "Good morning to you.");
    let held = |h: &Hive, c: &Msg| {
        window(h, c)
            .leaves
            .iter()
            .find(|(path, _)| path == FACT)
            .map(|(_, text)| text.clone())
            .unwrap_or_default()
    };
    assert!(
        held(&t.h, &c1).contains(FACT_TEXT),
        "check of the setup: the first call holds the fact as it stood"
    );
    the_curator_changes(&t.h, FACT, NEW_TEXT);
    let c2 = t.turn("t2", "What is on this week?", "Let me think.");
    assert_eq!(
        cut_epoch(&t.h, &c1),
        cut_epoch(&t.h, &c2),
        "check of the setup: no rebuild lay between the two calls"
    );
    assert!(t.breaks().is_empty(), "no prefix break: {:?}", t.breaks());
    assert!(
        held(&t.h, &c2).contains(FACT_TEXT) && !held(&t.h, &c2).contains(NEW_TEXT),
        "the system part keeps the block it was cut with: {}",
        held(&t.h, &c2)
    );
    let w = window(&t.h, &c2);
    assert_eq!(
        messages_with(&t.h, &c2, NEW_TEXT),
        vec![on_top_place(&t.h, &c2, "What is on this week?")],
        "the new text stands once, directly before the person's words: {:?}",
        w.messages
    );
}

/// Pin (review I-2 of the strand): on top goes only what the system part of
/// THIS target would carry after the rebuild. A target whose profile keeps
/// its own instruction slot alone (`instructions.only`) never carries
/// another slot of the family `instructions` -- one of the curator's own
/// that arrives while the cache is warm is not shown on top either, and the
/// system part stays the same.
#[test]
fn gh_d583_schnell_a_slot_the_target_leaves_out_is_not_shown_on_top() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    const HYGIENE: &str = "instructions.hygiene";
    const HYGIENE_TEXT: &str = "Never read out a number twice in one answer.";
    let mut t = Talk::new(&[("policy", "snapshot_window", json!("1"))]).reporting(json!({
        "instructions": {"slot": "own", "text": "Speak plainly.", "only": true}
    }));
    let c1 = t.turn("t1", "Good morning.", "Good morning to you.");
    assert!(
        window(&t.h, &c1)
            .leaves
            .iter()
            .any(|(path, _)| path == "instructions.own"),
        "check of the setup: the target's own slot stands in its system part: {:?}",
        window(&t.h, &c1).leaves
    );
    the_curator_writes(&t.h, HYGIENE, HYGIENE_TEXT);
    let c2 = t.turn("t2", "What is on this week?", "Let me think.");
    assert_eq!(
        cut_epoch(&t.h, &c1),
        cut_epoch(&t.h, &c2),
        "check of the setup: no rebuild lay between the two calls"
    );
    assert!(t.breaks().is_empty(), "no prefix break: {:?}", t.breaks());
    assert_eq!(
        window(&t.h, &c2).leaves,
        window(&t.h, &c1).leaves,
        "the system part stays the same"
    );
    assert!(
        messages_with(&t.h, &c2, HYGIENE_TEXT).is_empty(),
        "a slot the target's system part leaves out is not shown on top: {:?}",
        window(&t.h, &c2).messages
    );
}

/// Pin (review I-3 of the strand, GH #1298): the element on top is fixed
/// for a turn. A fact that arrives between the first call of a turn and its
/// tool round does not come in before the round's `tool_call` -- the part a
/// signed thinking is bound to: the second call begins byte for byte with
/// the first, and the fact stands on top from the next turn on.
#[test]
fn gh_d583_schnell_a_fact_inside_a_turn_waits_for_the_next_turn() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::new(&[("policy", "snapshot_window", json!("1"))]);
    t.h.target = Some(MAIN_TARGET.to_string());
    t.turn("t1", "Good morning.", "Good morning to you.");
    let id = "call_read_1";
    let a1 = t.call("t2", 0, json!([user("Read the file, please.")]));
    t.answer(&a1, "tool_calls", json!([tool_call(id, "read_file")]));
    the_curator_writes_the_fact(&t.h);
    let a2 = t.call(
        "t2",
        1,
        json!([tool_call(id, "read_file"), tool_result(id, "file 1: abc")]),
    );
    t.answer(&a2, "stop", json!([said("All read.")]));
    assert_eq!(
        cut_epoch(&t.h, &a1),
        cut_epoch(&t.h, &a2),
        "check of the setup: no rebuild lay between the two calls"
    );
    let (m1, m2) = (a1.messages(), a2.messages());
    assert!(
        m2.len() > m1.len() && m2[..m1.len()] == m1[..],
        "the tool round begins byte for byte with the turn's first call: {m1:?} / {m2:?}"
    );
    assert!(
        fact_messages(&t.h, &a2).is_empty(),
        "the fact does not come in inside the turn: {:?}",
        window(&t.h, &a2).messages
    );
    assert!(t.breaks().is_empty(), "no prefix break: {:?}", t.breaks());
    let a3 = t.turn("t3", "And on Sunday?", "Lena comes.");
    let w = window(&t.h, &a3);
    assert_eq!(
        fact_messages(&t.h, &a3),
        vec![on_top_place(&t.h, &a3, "And on Sunday?")],
        "the next turn shows the fact on top, directly before the person's words: {:?}",
        w.messages
    );
}

/// Pin (re-check I-B of fix round 1, review I-3 (b)): the element on top
/// that the turn's FIRST call showed stays in the turn's tool round, at the
/// same place: the call before the `tool_call` is what a signed thinking is
/// bound to, so the tool round begins byte for byte with the first call --
/// element included -- and a fact that came in between waits for the next
/// turn.
#[test]
fn gh_d583_schnell_a_tool_round_keeps_the_element_of_its_turn() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    const LATER: &str = "facts.later";
    const LATER_TEXT: &str = "The person's son calls on Monday.";
    let mut t = Talk::new(&[("policy", "snapshot_window", json!("1"))]);
    t.h.target = Some(MAIN_TARGET.to_string());
    t.turn("t1", "Good morning.", "Good morning to you.");
    the_curator_writes_the_fact(&t.h);
    let id = "call_read_1";
    let a1 = t.call("t2", 0, json!([user("Read the file, please.")]));
    assert_eq!(
        fact_messages(&t.h, &a1),
        vec![on_top_place(&t.h, &a1, "Read the file, please.")],
        "check of the setup: the turn's first call shows the fact on top: {:?}",
        window(&t.h, &a1).messages
    );
    t.answer(&a1, "tool_calls", json!([tool_call(id, "read_file")]));
    the_curator_writes(&t.h, LATER, LATER_TEXT);
    let a2 = t.call(
        "t2",
        1,
        json!([tool_call(id, "read_file"), tool_result(id, "file 1: abc")]),
    );
    t.answer(&a2, "stop", json!([said("All read.")]));
    assert_eq!(
        cut_epoch(&t.h, &a1),
        cut_epoch(&t.h, &a2),
        "check of the setup: no rebuild lay between the two calls"
    );
    let (m1, m2) = (a1.messages(), a2.messages());
    assert!(
        m2.len() > m1.len() && m2[..m1.len()] == m1[..],
        "the tool round begins byte for byte with the first call, its element included: {m1:?} / {m2:?}"
    );
    assert_eq!(
        fact_messages(&t.h, &a2).len(),
        1,
        "the element of the turn stands once in the tool round: {:?}",
        window(&t.h, &a2).messages
    );
    assert!(
        messages_with(&t.h, &a2, LATER_TEXT).is_empty(),
        "the fact that came in between waits for the next turn"
    );
    assert!(t.breaks().is_empty(), "no prefix break: {:?}", t.breaks());
}

/// The cuts of `what` a call's hop names (`hop.cuts`).
fn hop_cuts_of(call: &Msg, what: &str) -> Vec<Value> {
    call.hop
        .get("cuts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|c| c["what"] == what)
        .collect()
}

/// The tool round of turn `t2` after its first call showed the fact on top,
/// with the store refusing every leg in `refused` twice (read, read again).
fn a_tool_round_reading_without(refused: &str) -> (Talk, Msg, Msg) {
    clear_of_midnight();
    let mut t = Talk::new(&[("policy", "snapshot_window", json!("1"))]);
    t.h.target = Some(MAIN_TARGET.to_string());
    t.turn("t1", "Good morning.", "Good morning to you.");
    the_curator_writes_the_fact(&t.h);
    let id = "call_read_1";
    let a1 = t.call("t2", 0, json!([user("Read the file, please.")]));
    assert_eq!(
        fact_messages(&t.h, &a1).len(),
        1,
        "check of the setup: the turn's first call shows the fact on top: {:?}",
        window(&t.h, &a1).messages
    );
    t.answer(&a1, "tool_calls", json!([tool_call(id, "read_file")]));
    t.h.ledger_may_refuse = true;
    t.h.refuse_legs = vec![
        (refused.to_string(), "store_error".to_string()),
        (refused.to_string(), "store_error".to_string()),
    ];
    let a2 = t.call(
        "t2",
        1,
        json!([tool_call(id, "read_file"), tool_result(id, "file 1: abc")]),
    );
    assert!(
        t.h.refuse_legs.is_empty(),
        "check of the setup: the store refused {refused} twice: {:?}",
        t.h.refuse_legs
    );
    (t, a1, a2)
}

/// Pin (wave d58-3, stapel minor m-A of the re-check of fix round 2): the
/// session's record of the element (`w-top`) refused twice. The call does
/// not go on silently with an empty record: it names the gap on its hop
/// (`cuts`, what `top`, with the store's code and the leg) and on stderr,
/// and the tool round still shows the element of its turn -- before, it
/// showed none and nobody said why.
#[test]
fn gh_d583_schnell_a_refused_read_of_the_element_is_said_and_keeps_it() {
    if !shipped() {
        return;
    }
    let (t, _a1, a2) = a_tool_round_reading_without("w-top");
    let gap = hop_cuts_of(&a2, "top");
    assert_eq!(
        gap.len(),
        1,
        "the hop names the gap: {:?}",
        a2.hop.get("cuts")
    );
    assert_eq!(gap[0]["error_code"], "store_error", "{gap:?}");
    assert_eq!(gap[0]["legs"], json!(["w-top"]), "{gap:?}");
    assert!(
        t.h.stderr
            .iter()
            .any(|l| l.contains("top") && l.contains("w-top")),
        "stderr names the gap: {:?}",
        t.h.stderr
    );
    assert_eq!(
        fact_messages(&t.h, &a2).len(),
        1,
        "the tool round still shows the element of its turn: {:?}",
        window(&t.h, &a2).messages
    );
}

/// Pin (m-A, the second case): the parked turn of the read (`b-slimq`)
/// lost -- the read phase then reads the records of no session. The call
/// names the gap of the element's record as well and keeps the element.
#[test]
fn gh_d583_schnell_a_lost_parked_turn_says_the_gap_of_the_element() {
    if !shipped() {
        return;
    }
    let (_t, _a1, a2) = a_tool_round_reading_without("b-slimq");
    let gap = hop_cuts_of(&a2, "top");
    assert_eq!(
        gap.len(),
        1,
        "the hop names the gap: {:?}",
        a2.hop.get("cuts")
    );
    assert_eq!(gap[0]["legs"], json!(["b-slimq"]), "{gap:?}");
}

/// The calls the function `tree_rules` of the shipped policy makes, by name,
/// and the calls the cut around it makes outside `tree_rules`. The cut is
/// the nearest enclosing `def`, or -- where `tree_rules` stands in a
/// top-level block of the script (an `if`) -- that top-level statement.
const CALLS_OF_THE_CUT: &str = r#"
import ast, json, sys
src = json.load(sys.stdin)["src"]
tree = ast.parse(src)
parent = {}
for n in ast.walk(tree):
    for c in ast.iter_child_nodes(n):
        parent[c] = n
def names(node, skip=None):
    out = set()
    todo = [node]
    while todo:
        n = todo.pop()
        if n is skip:
            continue
        if isinstance(n, ast.Call) and isinstance(n.func, ast.Name):
            out.add(n.func.id)
        todo.extend(ast.iter_child_nodes(n))
    return out
rules = [n for n in ast.walk(tree) if isinstance(n, ast.FunctionDef) and n.name == "tree_rules"]
inner = names(rules[0]) if rules else set()
outer = set()
if rules:
    cut = parent.get(rules[0])
    while cut is not None and not isinstance(cut, ast.FunctionDef) and not isinstance(parent.get(cut), ast.Module):
        cut = parent.get(cut)
    if cut is not None and not isinstance(cut, ast.Module):
        outer = names(cut, skip=rules[0])
print(json.dumps({"found": bool(rules), "inner": sorted(inner), "outer": sorted(outer)}))
"#;

/// Pin (re-check m-1 of fix round 1): the rules of this target's tree that
/// decide what goes on top (`tree_rules`) are the cut's own -- every step it
/// takes is a function the cut itself calls on the leaves it sends, none is
/// a copy of the cut's code. A copy drifts silently: a step the cut changes
/// would leave the check of what goes on top on the old rules.
#[test]
fn gh_d583_schnell_the_tree_rules_are_the_cuts_own() {
    if !shipped() {
        return;
    }
    let doc = json!({"src": script_of("policy")});
    let out = run_python(CALLS_OF_THE_CUT, &doc.to_string());
    assert!(
        out.status.success(),
        "the policy does not parse: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let got: meclaw_core::serde_json::Value =
        sj::from_slice(&out.stdout).expect("the calls as JSON");
    assert_eq!(
        got["found"],
        json!(true),
        "check of the setup: the policy has `tree_rules`"
    );
    let list = |k: &str| -> Vec<String> {
        got[k]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let (inner, outer) = (list("inner"), list("outer"));
    // What only reads or sorts a list is no rule of the tree.
    const NO_RULE: [&str; 6] = ["any", "sorted", "list", "len", "set", "tuple"];
    for name in inner.iter().filter(|n| !NO_RULE.contains(&n.as_str())) {
        assert!(
            outer.contains(name),
            "`tree_rules` calls `{name}`, which the cut itself never calls -- a copy of a step, not the cut's own: {inner:?}"
        );
    }
    for copied in ["without_slots", "identity_slots", "foreign_slots"] {
        assert!(
            !inner.iter().any(|n| n == copied),
            "`tree_rules` takes the step `{copied}` itself instead of the cut's own step: {inner:?}"
        );
    }
}
