//! D54g oneanswer (GH #1505, OR-54g-13): on a written channel a turn sends at
//! most one bridge and exactly one answer.
//!
//! Measured (finding 16): the quick voice wrote the finished sentence beside a
//! call, the round after the tool result wrote it again, and both went out
//! behind the curator's bridge -- three messages, one of them twice. All three
//! meet in `collector/assemble`, and only the curator's bridge was checked
//! there: a sentence the model says beside its own calls carries no
//! `call_kind` and left without a look at the store.
//!
//! The contract these pins hold:
//! 1. every interim (hop `interim`, with or without `call_kind`) goes through
//!    the bridge gate and leaves only while nothing went out in its turn;
//! 2. every send on `answer` leaves a row of role `said` in the turn's store
//!    (table `round`), which falls with the round;
//! 3. an answer whose text already went out in its turn leaves with the hop
//!    field `repeat` "1" (text unchanged) and still ends the round; a bridge
//!    whose text already went out is held;
//! 4. a turn without a bridge and without a repeat answers as before.
//!
//! The pins drive the shipped script against a store kept in the test
//! (`WindowStore`, `drive`): they measure what leaves on `answer` and what the
//! store holds, so the order of two arrivals is the order they are fed in.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::*;
use serde_json::{Value, json};

const T: &str = "turn-54";
const BRIDGE: &str = "One moment, I am looking.";
const BESIDE: &str = "Let me check the timetable.";
const ANSWER: &str = "The next train to Hamburg leaves at 14:05.";

fn text(m: &str) -> Value {
    json!([{"origin": "assistant", "type": "text", "text": m}])
}

/// A store that holds one open round: its opening row `leg-window` is there,
/// as it is from the turn's opening until the answer drops the round.
fn open_round() -> WindowStore {
    WindowStore::holding(
        "round",
        vec![json!({"turn_id": T, "session_id": SESSION, "iter": 0,
                    "role": "leg-window", "turn": "{}", "fired": 1,
                    "recorded_at": ""})],
    )
}

/// The curator's bridge: an interim of the side call (`call_kind` "side").
fn curator_bridge(m: &str) -> Value {
    lane(
        "in_answer",
        json!({"interim": "1", "finish_reason": "stop"}),
        json!({"turn_id": T, "iter": "0", "path": "simple", "curator_call": "c-9-side",
               "call_kind": "side"}),
        text(m),
    )
}

/// A sentence the model says beside its own calls: interim, no `call_kind`.
fn beside(m: &str) -> Value {
    lane(
        "in_answer",
        json!({"interim": "1", "finish_reason": "tool_calls"}),
        json!({"turn_id": T, "iter": "0", "path": "simple", "curator_call": "c-9"}),
        text(m),
    )
}

/// The turn's answer after a tool result (iteration 1 of a stamped turn).
fn answer_after_tool(m: &str) -> Value {
    lane(
        "in_answer",
        json!({"finish_reason": "stop"}),
        json!({"turn_id": T, "iter": "1", "path": "simple", "curator_call": "c-10"}),
        text(m),
    )
}

/// The first answer of a turn with no tool round behind it.
fn first_answer(m: &str) -> Value {
    lane(
        "in_answer",
        json!({"finish_reason": "stop"}),
        json!({"turn_id": T, "iter": "0", "path": "simple", "curator_call": "c-9"}),
        text(m),
    )
}

/// The same sentence beside the calls of a later iteration.
fn beside_at(m: &str, it: &str) -> Value {
    let mut doc = beside(m);
    doc["header"]["context"]["iter"] = json!(it);
    doc
}

/// The model's calls of iteration `it`: one asynchronous call -- handed over
/// with the turn's answer or not -- and `m` beside it when `m` is not empty.
/// Both end the round on the spot when the model spoke beside them.
fn calls_beside(m: &str, it: &str, handed: bool) -> Value {
    let id = format!("call-{it}");
    let handoff = if handed { id.clone() } else { String::new() };
    let mut turns = Vec::new();
    if !m.is_empty() {
        turns.push(json!({"origin": "assistant", "type": "text", "text": m}));
    }
    turns.push(json!({"origin": "assistant", "type": "tool_call", "id": id,
                      "text": json!({"name": "consult_core", "arguments": {}}).to_string()}));
    lane(
        "in_calls",
        json!({"async_calls": id, "handoff_calls": handoff}),
        json!({"turn_id": T, "iter": it, "path": "simple", "curator_call": "c-10"}),
        Value::Array(turns),
    )
}

fn answers(out: &[Value]) -> Vec<&Value> {
    out.iter()
        .filter(|m| m["header"]["route"].as_str() == Some("answer"))
        .collect()
}

fn texts(out: &[Value]) -> Vec<String> {
    answers(out)
        .iter()
        .map(|m| m["messages"][0]["text"].as_str().unwrap_or("").to_string())
        .collect()
}

/// The rows of role `said` the store holds for the turn.
fn said_rows(store: &WindowStore) -> Vec<Value> {
    store
        .rows("round")
        .into_iter()
        .filter(|r| r["role"] == "said" && r["turn_id"] == T)
        .collect()
}

fn round_is_open(store: &WindowStore) -> bool {
    store
        .rows("round")
        .iter()
        .any(|r| r["role"] == "leg-window" && r["turn_id"] == T)
}

/// Contract point 1: a sentence beside the model's own calls does not leave at
/// once. It reads its round back like the curator's bridge and leaves, marked
/// interim, when nothing went out in its turn yet.
#[test]
fn gh_d54g_oneanswer_a_text_beside_a_call_goes_through_the_bridge_gate() {
    let first = assemble(&[], beside(BESIDE));
    assert!(
        answers(&first).is_empty(),
        "a sentence beside a call left without a look at the store: {first:?}"
    );
    let gate = in_phase(&first, "bridge-gate");
    let kept: Value = serde_json::from_str(&hop_str(gate, "gate")).expect("gate json");
    assert_eq!(
        kept["messages"][0]["text"], BESIDE,
        "the sentence rides beside the op, not in a row"
    );

    let mut store = open_round();
    let out = drive(&mut store, beside(BESIDE));
    let said = answers(&out);
    assert_eq!(said.len(), 1, "the first bridge of a turn is said: {out:?}");
    assert_eq!(said[0]["messages"][0]["text"], BESIDE);
    assert_eq!(hop_str(said[0], "interim"), "1");
    assert_eq!(hop_str(said[0], "turn_id"), T);
}

/// Contract point 1: after one bridge went out, a second interim of the same
/// turn is held -- whichever of the two came first.
#[test]
fn gh_d54g_oneanswer_a_second_bridge_in_one_turn_is_held() {
    for (first, second, order) in [
        (
            curator_bridge(BRIDGE),
            beside(BESIDE),
            "the curator's, then the model's",
        ),
        (
            beside(BESIDE),
            curator_bridge(BRIDGE),
            "the model's, then the curator's",
        ),
    ] {
        let mut store = open_round();
        let a = drive(&mut store, first);
        assert_eq!(
            answers(&a).len(),
            1,
            "{order}: the first bridge is said: {a:?}"
        );
        let b = drive(&mut store, second);
        assert!(
            answers(&b).is_empty(),
            "{order}: a second bridge went out in one turn: {b:?}"
        );
        assert!(
            round_is_open(&store),
            "{order}: a held bridge ends nothing, the round stays open"
        );
    }

    // Re-check 1, I-1: iteration 0 says a bridge beside a tool call, and
    // iteration 1 speaks again beside a call that hands the turn over. That
    // round ends at its calls, before its sentence arrives; the sentence is
    // still the turn's second bridge and is held -- in other words and in the
    // same words. The round is over, and its rows `said` fall behind it.
    for (again, words) in [
        ("Let me ask my core.", "other words"),
        (BESIDE, "the same words"),
    ] {
        let mut store = open_round();
        let a = drive(&mut store, beside(BESIDE));
        assert_eq!(texts(&a), [BESIDE], "{words}: the first bridge: {a:?}");
        let ended = drive(&mut store, calls_beside(again, "1", true));
        assert!(answers(&ended).is_empty(), "{words}: {ended:?}");
        let b = drive(&mut store, beside_at(again, "1"));
        assert!(
            answers(&b).is_empty(),
            "{words}: a second bridge went out beside the call that handed the turn over: {b:?}"
        );
        assert!(
            said_rows(&store).is_empty(),
            "{words}: a row `said` stayed behind the round: {:?}",
            store.rows("round")
        );
    }

    // Re-check 2, I-1: the curator's bridge claims the turn while the round
    // is still open, and before its store reply is handled the round ends at
    // a handed call and the sentence beside it reads the round -- it finds the
    // bridge's claim, not yet its row of a text that went out. The sentence
    // reads once more behind that bridge's word and is held: one bridge in
    // the turn, in other words and in the same words.
    for (side, words) in [(BRIDGE, "other words"), (BESIDE, "the same words")] {
        let mut store = open_round();
        let side_doc = curator_bridge(side);
        let side_ctx = side_doc["header"]["context"].clone();
        let side_gate = assemble(&[], side_doc);
        let side_reply = store.reply(in_phase(&side_gate, "bridge-gate"), &side_ctx);
        let ended = drive(&mut store, calls_beside(BESIDE, "0", true));
        assert!(answers(&ended).is_empty(), "{words}: {ended:?}");
        let sentence = beside(BESIDE);
        let sentence_ctx = sentence["header"]["context"].clone();
        let sentence_gate = assemble(&[], sentence);
        let sentence_reply = store.reply(in_phase(&sentence_gate, "bridge-gate"), &sentence_ctx);
        assert!(
            said_rows(&store).iter().any(|r| r["fired"] == 0
                && r["turn"].as_str().unwrap_or("").contains("\"bridge\"")),
            "{words}: the sentence must read the bridge's open claim: {:?}",
            store.rows("round")
        );
        let a = drive(&mut store, side_reply);
        assert_eq!(texts(&a), [side], "{words}: the curator's bridge: {a:?}");
        let b = drive(&mut store, sentence_reply);
        assert!(
            answers(&b).is_empty(),
            "{words}: the sentence beside the handed call went out behind an open claim: {b:?}"
        );
        assert!(
            said_rows(&store).is_empty(),
            "{words}: a row `said` stayed behind the round: {:?}",
            store.rows("round")
        );
    }

    // Fix round 3, OR .24: the curator's bridge claims the turn while the
    // round is open, but the store refuses its read; before that reply is
    // handled the round ends at a handed call and leaves its row `due`. The
    // bridge takes back its own claim and nothing else, so the sentence
    // beside the handed call still finds its row `due` and goes out -- the
    // person hears one sentence, never none.
    for (side, words) in [(BRIDGE, "other words"), (BESIDE, "the same words")] {
        let mut store = open_round().refusing("c-bridge-gate");
        let side_doc = curator_bridge(side);
        let side_ctx = side_doc["header"]["context"].clone();
        let side_gate = assemble(&[], side_doc);
        let side_reply = store.reply(in_phase(&side_gate, "bridge-gate"), &side_ctx);
        store.heal("c-bridge-gate");
        let ended = drive(&mut store, calls_beside(BESIDE, "0", true));
        assert!(answers(&ended).is_empty(), "{words}: {ended:?}");
        let a = drive(&mut store, side_reply);
        assert!(
            answers(&a).is_empty(),
            "{words}: a bridge whose read was refused was said: {a:?}"
        );
        assert!(
            said_rows(&store)
                .iter()
                .any(|r| r["turn"].as_str().unwrap_or("").contains("\"due\"")),
            "{words}: the refused bridge took the row `due` of the sentence: {:?}",
            store.rows("round")
        );
        let b = drive(&mut store, beside(BESIDE));
        assert_eq!(
            texts(&b),
            [BESIDE],
            "{words}: the sentence beside the handed call: {b:?}"
        );
        assert!(
            said_rows(&store).is_empty(),
            "{words}: a row `said` stayed behind the round: {:?}",
            store.rows("round")
        );
    }

    // Without an earlier bridge the sentence beside the handed call is the
    // turn's one bridge and goes out exactly once; a later bridge of the turn
    // is held. What let it out is a row of the round, not of the departure:
    // nothing of it stays once the sentence has passed (re-check 1, m-2).
    let mut store = open_round();
    let ended = drive(&mut store, calls_beside(BESIDE, "0", true));
    assert!(answers(&ended).is_empty(), "{ended:?}");
    let a = drive(&mut store, beside(BESIDE));
    assert_eq!(
        texts(&a),
        [BESIDE],
        "the sentence beside the handed call: {a:?}"
    );
    let b = drive(&mut store, curator_bridge(BRIDGE));
    assert!(answers(&b).is_empty(), "a later bridge went out: {b:?}");
    let departs: Vec<Value> = store
        .rows("round")
        .into_iter()
        .filter(|r| r["role"] == "depart")
        .collect();
    assert_eq!(departs.len(), 1, "{:?}", store.rows("round"));
    let body: Value =
        serde_json::from_str(departs[0]["turn"].as_str().unwrap_or("null")).expect("depart json");
    assert_eq!(
        body,
        json!({"id": "call-0", "name": "consult_core"}),
        "the departure carries what it carried before GH #1505"
    );
    assert!(
        said_rows(&store).is_empty(),
        "a row `said` outlived the round: {:?}",
        store.rows("round")
    );
}

/// Contract point 2: a bridge that went out leaves one row of role `said`
/// under its turn -- a hash, not the wording --, the answer writes one too,
/// and the bridge's row falls when the answer drops the round.
#[test]
fn gh_d54g_oneanswer_every_send_leaves_a_said_row_that_falls_with_the_round() {
    let mut store = open_round();
    let a = drive(&mut store, curator_bridge(BRIDGE));
    assert_eq!(answers(&a).len(), 1, "the bridge is said: {a:?}");
    let kept = said_rows(&store);
    assert_eq!(
        kept.len(),
        1,
        "a bridge that went out leaves one row `said`: {:?}",
        store.rows("round")
    );
    assert!(
        !kept[0].to_string().contains(BRIDGE),
        "the row keeps a hash of the text, not the wording: {}",
        kept[0]
    );

    let seen = store.ops.len();
    let b = drive(&mut store, answer_after_tool(ANSWER));
    assert_eq!(texts(&b), [ANSWER], "the answer leaves: {b:?}");
    // Review I-1: other words than the bridge's are no repeat. An answer read
    // as one would be sent by no writing channel, and the person would get the
    // bridge and nothing after it.
    assert!(
        answers(&b)[0]["header"].get("repeat").is_none(),
        "an answer with other words than its bridge left as a repeat: {}",
        answers(&b)[0]
    );
    assert!(
        store.ops[seen..].iter().any(|(table, operation, args)| {
            table == "round"
                && operation == "insert"
                && args["row"]["role"] == "said"
                && args["row"]["turn_id"] == T
        }),
        "the answer that went out writes a row `said` too: {:?}",
        &store.ops[seen..]
    );
    assert!(
        !store.rows("round").contains(&kept[0]),
        "the bridge's row falls with the round: {:?}",
        store.rows("round")
    );
    assert!(!round_is_open(&store), "the answer ends the round");
}

/// Contract point 3 (finding 16): the answer's text already went out in this
/// turn, beside a call. The answer still leaves once, so everybody who waits
/// for the turn's end sees it -- marked `repeat`, text unchanged, round ended.
/// Equal means equal after whitespace and case are levelled, nothing else.
#[test]
fn gh_d54g_oneanswer_an_answer_already_sent_closes_the_turn_as_a_repeat() {
    let mut store = open_round();
    let a = drive(&mut store, beside(ANSWER));
    assert_eq!(texts(&a), [ANSWER], "the sentence beside the call: {a:?}");

    let again = "  the NEXT train to Hamburg   leaves at 14:05. ";
    let b = drive(&mut store, answer_after_tool(again));
    let said = answers(&b);
    assert_eq!(said.len(), 1, "the turn closes with one answer: {b:?}");
    assert_eq!(
        said[0]["header"]["repeat"], "1",
        "a text that already went out is marked as a repeat: {}",
        said[0]
    );
    assert_eq!(
        said[0]["messages"][0]["text"], again,
        "the text stays as it came, for readers beside the channel"
    );
    assert!(said[0]["header"].get("interim").is_none(), "{}", said[0]);
    assert_eq!(hop_str(said[0], "turn_id"), T);
    assert!(
        !round_is_open(&store),
        "a repeat ends the round all the same"
    );

    // Review M-3, error injection: the answer reaches the cell before the
    // store's reply to the bridge's gate, so both gates stand in the store's
    // queue, the bridge's first. The store writes the bridge's row and refuses
    // its round read: the bridge does not go out, and its words must not make
    // the answer a repeat -- the person would get neither.
    let mut store = open_round().refusing("c-bridge-gate");
    let (bridge, answer) = race(&mut store, beside(ANSWER), answer_after_tool(ANSWER));
    assert!(
        answers(&bridge).is_empty(),
        "a bridge the store could not check went out: {bridge:?}"
    );
    assert_eq!(texts(&answer), [ANSWER], "the answer leaves: {answer:?}");
    assert!(
        answers(&answer)[0]["header"].get("repeat").is_none(),
        "the words of a bridge that never went out made the answer a repeat: {}",
        answers(&answer)[0]
    );
    assert!(said_rows(&store).is_empty(), "{:?}", store.rows("round"));

    // The same race with a store that says yes: the bridge goes out, and the
    // answer with its words is the repeat -- read after the bridge's row
    // turned from a claim into a text that went out.
    let mut store = open_round();
    let (bridge, answer) = race(&mut store, beside(ANSWER), answer_after_tool(ANSWER));
    assert_eq!(texts(&bridge), [ANSWER], "the bridge goes out: {bridge:?}");
    assert_eq!(
        answers(&answer).len(),
        1,
        "the turn closes with one answer: {answer:?}"
    );
    assert_eq!(
        answers(&answer)[0]["header"]["repeat"],
        "1",
        "{}",
        answers(&answer)[0]
    );
    assert!(
        said_rows(&store).is_empty(),
        "nothing stays behind the round: {:?}",
        store.rows("round")
    );

    // Re-check 1, m-1: the same race, the answer in other words. The bridge
    // goes out, the answer is no repeat, and the bridge's row does not stay
    // behind the round its answer closed.
    let mut store = open_round();
    let (bridge, answer) = race(&mut store, beside(BRIDGE), answer_after_tool(ANSWER));
    assert_eq!(texts(&bridge), [BRIDGE], "the bridge goes out: {bridge:?}");
    assert_eq!(texts(&answer), [ANSWER], "the answer leaves: {answer:?}");
    assert!(
        answers(&answer)[0]["header"].get("repeat").is_none(),
        "{}",
        answers(&answer)[0]
    );
    assert!(
        said_rows(&store).is_empty(),
        "the bridge's row stayed behind the closed round: {:?}",
        store.rows("round")
    );

    // Re-check 1, I-1: iteration 0 said the words as a bridge beside a tool
    // call; iteration 1 says them again beside a call that hands nothing over.
    // That round ends at its calls, and the words come as the turn's answer --
    // the repeat of what already went out, which the round's end must not
    // forget.
    let mut store = open_round();
    let a = drive(&mut store, beside(ANSWER));
    assert_eq!(texts(&a), [ANSWER], "the bridge of iteration 0: {a:?}");
    let ended = drive(&mut store, calls_beside(ANSWER, "1", false));
    assert!(answers(&ended).is_empty(), "{ended:?}");
    let b = drive(&mut store, answer_after_tool(ANSWER));
    assert_eq!(
        answers(&b).len(),
        1,
        "the turn closes with one answer: {b:?}"
    );
    assert_eq!(
        answers(&b)[0]["header"]["repeat"],
        "1",
        "words that went out as a bridge left again as the answer: {}",
        answers(&b)[0]
    );
    assert!(
        said_rows(&store).is_empty(),
        "nothing stays behind the round: {:?}",
        store.rows("round")
    );
}

/// The two gates of one turn in the store's queue, the bridge's first, and
/// the replies handled in that order: what left after the bridge's reply and
/// what left after the answer's.
fn race(store: &mut WindowStore, bridge: Value, answer: Value) -> (Vec<Value>, Vec<Value>) {
    let bridge_ctx = bridge["header"]["context"].clone();
    let answer_ctx = answer["header"]["context"].clone();
    let gb = assemble(&[], bridge);
    let ga = assemble(&[], answer);
    assert_eq!(in_phase(&gb, "bridge-gate")["header"]["route"], "cstore");
    assert_eq!(in_phase(&ga, "answer-gate")["header"]["route"], "cstore");
    let rb = store.reply(in_phase(&gb, "bridge-gate"), &bridge_ctx);
    let ra = store.reply(in_phase(&ga, "answer-gate"), &answer_ctx);
    (drive(store, rb), drive(store, ra))
}

/// Contract point 3: a bridge whose text already went out in its turn is held
/// -- after a bridge with the same words, and after the answer itself.
#[test]
fn gh_d54g_oneanswer_a_bridge_equal_to_a_sent_text_is_held() {
    let mut store = open_round();
    let a = drive(&mut store, curator_bridge(BRIDGE));
    assert_eq!(texts(&a), [BRIDGE], "the first bridge is said: {a:?}");
    let b = drive(&mut store, beside(" one MOMENT, i am   looking. "));
    assert!(
        answers(&b).is_empty(),
        "the same words went out a second time as a bridge: {b:?}"
    );

    let mut store = open_round();
    let a = drive(&mut store, first_answer(ANSWER));
    assert_eq!(texts(&a), [ANSWER], "the answer leaves: {a:?}");
    let b = drive(&mut store, beside(ANSWER));
    assert!(
        answers(&b).is_empty(),
        "the answer's words went out again as a bridge: {b:?}"
    );
    // Review M-2: the held bridge wrote its row into a round that is over and
    // takes it back -- nothing stays behind for the stale sweep.
    assert!(
        said_rows(&store).is_empty(),
        "a held bridge left its row `said` behind: {:?}",
        store.rows("round")
    );
}

/// Contract point 4: a turn without a bridge and without a repeat sends the
/// answer it sent before the fix -- the same message, no `repeat` -- and the
/// one new store write of such a turn is its row `said`.
#[test]
fn gh_d54g_oneanswer_a_turn_without_a_bridge_answers_as_before() {
    let mut store = open_round();
    let out = drive(&mut store, first_answer(ANSWER));
    assert_eq!(
        out,
        vec![json!({
            "header": {"route": "answer", "phase": "", "turn_id": T,
                       "session_id": SESSION, "iter": "0", "round_id": T,
                       "late": "", "finish_reason": "stop", "truncated": ""},
            "messages": text(ANSWER)
        })],
        "the answer of a plain turn is the message it was before"
    );
    assert!(!round_is_open(&store), "the answer ends the round");

    let writes: Vec<&(String, String, Value)> = store
        .ops
        .iter()
        .filter(|(_, operation, _)| operation == "insert" || operation == "update")
        .collect();
    assert!(
        !writes.is_empty()
            && writes.iter().all(|(table, operation, args)| {
                table == "round" && operation == "insert" && args["row"]["role"] == "said"
            }),
        "the one new store write of a plain turn is its row `said`: {writes:?}"
    );

    // Review M-1: a send without text leaves as it did before GH #1505 -- in
    // the call that brought it, the answer with its round's drop behind it,
    // an interim alone. No gate, no `degraded`, no row `said`.
    for (doc, interim) in [
        (first_answer_without_text(), false),
        (beside_without_text(), true),
    ] {
        let out = assemble(&[], doc.clone());
        let mut head = json!({"route": "answer", "phase": "", "turn_id": T,
                              "session_id": SESSION, "iter": "0", "round_id": T,
                              "late": "", "finish_reason": "stop", "truncated": ""});
        if interim {
            head["interim"] = json!("1");
            head["finish_reason"] = json!("tool_calls");
        }
        assert_eq!(
            answers(&out),
            vec![&json!({"header": head, "messages": []})],
            "a send without text is the message it was before: {out:?}"
        );
        let stores: Vec<&Value> = out
            .iter()
            .filter(|m| m["header"]["route"] == "cstore")
            .collect();
        assert_eq!(stores.len(), usize::from(!interim), "{out:?}");
        assert!(
            stores.iter().all(|m| m["header"]["phase"] != "answer-gate"
                && m["header"]["phase"] != "bridge-gate"
                && !m["messages"].to_string().contains("said")),
            "a send without text took a gate or wrote a row `said`: {stores:?}"
        );
        let mut store = open_round();
        let left = drive(&mut store, doc);
        assert!(
            answers(&left)
                .iter()
                .all(|m| m["header"].get("degraded").is_none()),
            "a text that was never there is no text the store lost: {left:?}"
        );
        assert!(said_rows(&store).is_empty(), "{:?}", store.rows("round"));
    }
}

/// The first answer of a turn that says nothing.
fn first_answer_without_text() -> Value {
    let mut doc = first_answer("");
    doc["messages"] = json!([]);
    doc
}

/// An interim beside a call that says nothing.
fn beside_without_text() -> Value {
    let mut doc = beside("");
    doc["messages"] = json!([]);
    doc
}
