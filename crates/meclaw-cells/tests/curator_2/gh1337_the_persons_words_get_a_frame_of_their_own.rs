//! GH #1337 -- the person's words get a frame of their own when anything else
//! stands before them in their element.
//!
//! The curator puts other blocks into the element of the person's newest
//! words: the notes block (GH #1303) opens it, a recall that would open the
//! window is folded into it (GH #1333). Each of those blocks carries a frame,
//! the person's words carried only their short id. Measured in a lab request:
//! a small model read the words as one more note ("no message from you") in 3
//! of 6 calls; the same request with the words framed as the person's own was
//! answered right in 6 of 6. So the words get one fixed frame, between their id
//! and the words, whenever something other than the session boundary of the
//! words themselves (GH #1335) stands before them -- structure only, never a
//! reading of the words. An element of an earlier round never says "now".
//! Filter: `binary_id(meclaw-cells::curator_2) & test(/^gh1337/)`.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The frame of the person's words in the element of the running round.
const NOW: &str = "[the person, now -- their own words to you]";
/// The frame of the person's words in an element of an earlier round.
const PAST: &str = "[the person -- their own words]";
const NOTES_OPEN: &str = "[notes for this turn -- from care;";
const RECALL_FRAME: &str = "[memory, from before this turn -- not said now]";
const RECALL_ID: &str = "call_recall_0123456789abcdef";
const BRIEF_ID: &str = "call_brief_0123456789abcdef";
const CAKE: &str = "I baked a plum cake.";

fn talky() -> Hive {
    Hive::with(&[("policy", "role", json!("talky"))])
}

fn ambient_round(words: &str) -> Value {
    json!([
        user(words),
        tool_call(RECALL_ID, "memory_recall"),
        tool_result(RECALL_ID, "WHAT WAS SAID\n  user: \"hello\""),
        tool_call(BRIEF_ID, "affinity_brief"),
        tool_result(BRIEF_ID, "who: the person"),
    ])
}

fn care_note(h: &mut Hive) {
    h.lane(
        "in_note",
        json!({"session_id": "s1"}),
        json!({}),
        json!({"notes": [{"id": "n1", "source": "care", "text": "Stay with the topic.",
                          "scope": "session"}]}),
    );
}

/// The id and frame the person's `words` close their element with.
fn framed(words: &str, frame: &str) -> String {
    format!("[#{}] {frame}\n{words}", short_id(&user(words)))
}

fn last_text(c: &Msg) -> String {
    texts(c).last().cloned().unwrap_or_default()
}

/// Case a: a recall folded into the turn that opens the window.
#[test]
fn gh1337_a_folded_recall_frames_the_persons_words() {
    if !shipped() {
        return;
    }
    let mut h = talky();
    let c = h.curate("s1", "t1", 0, ambient_round(CAKE), mode("Be brief."));
    let text = last_text(&c);
    assert!(
        text.contains(RECALL_FRAME),
        "the recall is folded: {text:?}"
    );
    assert!(
        text.ends_with(&format!("\n\n{}", framed(CAKE, NOW))),
        "the words close the turn behind their own frame: {text:?}"
    );
    assert_eq!(text.matches(NOW).count(), 1, "{text:?}");
}

/// Case a, later turns: the folded turn of an earlier round never says
/// "now", and from then on it stands byte for byte the same.
#[test]
fn gh1337_an_earlier_folded_turn_never_says_now_and_stays() {
    if !shipped() {
        return;
    }
    let mut h = talky();
    let c1 = h.curate("s1", "t1", 0, ambient_round(CAKE), mode("Be brief."));
    h.tap(&c1, "stop", json!({}), json!([said("Lovely.")]));
    let c2 = h.curate("s1", "t2", 0, json!([user("And now?")]), mode("Be brief."));
    let first = texts(&c2)[0].clone();
    assert!(
        first.ends_with(&format!("\n\n{}", framed(CAKE, PAST))),
        "an earlier round frames the words without now: {first:?}"
    );
    assert!(!first.contains(NOW), "{first:?}");
    // The newest words have nothing before them: no frame.
    assert_eq!(
        last_text(&c2),
        format!("[#{}] And now?", short_id(&user("And now?")))
    );
    h.tap(&c2, "stop", json!({}), json!([said("Still here.")]));
    let c3 = h.curate("s1", "t3", 0, json!([user("Bye.")]), mode("Be brief."));
    assert_eq!(
        c2.messages()[..3],
        c3.messages()[..3],
        "the older elements stay byte for byte"
    );
}

/// Case b: the notes block opens the turn.
#[test]
fn gh1337_the_notes_block_frames_the_persons_words() {
    if !shipped() {
        return;
    }
    let mut h = talky();
    turn(&mut h, "s1", "t1", "a", "b", json!({}));
    care_note(&mut h);
    let c = h.curate("s1", "t2", 0, json!([user("c")]), mode("Be brief."));
    let text = last_text(&c);
    assert!(text.starts_with(NOTES_OPEN), "{text:?}");
    assert_eq!(
        text,
        format!(
            "[notes for this turn -- from care; guidance, not something the person said]\n\
             - Stay with the topic.\n\n{}",
            framed("c", NOW)
        )
    );
}

/// Case c: notes, a folded recall and the session boundary in one element
/// -- exactly one frame of the person, right before the words.
#[test]
fn gh1337_notes_recall_and_session_bound_give_one_frame() {
    if !shipped() {
        return;
    }
    let mut h = talky();
    // A row of an earlier session the window does not render (a result
    // without its call): the wall holds two sessions, so the running one
    // opens with its boundary, and nothing stands before the round -- the
    // recall goes into the turn.
    h.row(
        backdated(30),
        "s0",
        "t0",
        "recall",
        &tool_result("call_recall_00000000000000aa", "WHAT WAS SAID"),
        0,
    );
    care_note(&mut h);
    let c = h.curate("s1", "t1", 0, ambient_round(CAKE), mode("Be brief."));
    let text = last_text(&c);
    assert!(text.starts_with(NOTES_OPEN), "{text:?}");
    assert!(text.contains("[this session, since "), "{text:?}");
    assert!(text.contains(RECALL_FRAME), "{text:?}");
    assert_eq!(text.matches(NOW).count(), 1, "one frame: {text:?}");
    assert!(
        text.ends_with(&format!("\n\n{}", framed(CAKE, NOW))),
        "{text:?}"
    );
}

/// Case d: an element with only the person's words -- or with the bare
/// session boundary of the words -- stays as it was.
#[test]
fn gh1337_words_alone_or_with_their_session_bound_stay_bare() {
    if !shipped() {
        return;
    }
    let mut h = talky();
    turn(&mut h, "s1", "t1", "a", "b", json!({}));
    let c = h.curate("s1", "t2", 0, json!([user("c")]), mode("Be brief."));
    assert_eq!(last_text(&c), format!("[#{}] c", short_id(&user("c"))));
    // A new session: the boundary alone frames the words.
    let c2 = h.curate("s2", "t1", 0, json!([user("d")]), mode("Be brief."));
    let text = last_text(&c2);
    let id = format!("[#{}] d", short_id(&user("d")));
    assert!(
        text.starts_with("[this session, since ") && text.ends_with(&format!("]\n{id}")),
        "{text:?}"
    );
    assert!(!text.contains("[the person"), "{text:?}");
    for t in texts(&c2) {
        assert!(!t.contains("[the person"), "{t:?}");
    }
}

/// Case e: without short ids the frame stands right before the words.
#[test]
fn gh1337_without_short_ids_the_frame_opens_the_words() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "short_ids", json!("0")),
    ]);
    let c = h.curate("s1", "t1", 0, ambient_round(CAKE), mode("Be brief."));
    let text = last_text(&c);
    assert!(text.contains(RECALL_FRAME), "{text:?}");
    assert!(
        text.ends_with(&format!("\n\n{NOW}\n{CAKE}")),
        "the frame opens the words, no id: {text:?}"
    );
    assert!(!text.contains("[#"), "{text:?}");
}

/// The full notes block of `care_note`, as it opens the person's element.
const CARE_BLOCK: &str = "[notes for this turn -- from care; guidance, not something the person \
                          said]\n- Stay with the topic.";
const WEATHER: &str = "how is the weather in Potsdam today?";

/// A `simple` round on `in_curate` (the quick way); `needs` `tool` makes it
/// the tool round, with the bridge beside the required call (GH #1260).
fn simple(h: &mut Hive, needs: &str, messages: Value) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "phase": "",
                     "path": "simple", "families": "web", "needs": needs});
    let ctx = json!({"session_id": "s1", "turn_id": "t1", "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "simple"});
    let body = json!({"messages": messages,
                      "system": {"instructions": {"mode": {"text": "Be brief."}}}});
    h.lane("in_curate", ctx, hop, body);
}

/// Review M-3/I-1: the quick way takes its own notes share, 0 (README,
/// `notes_share`): with a live note its window -- and the bridge of its tool
/// round, built from the same items -- shows the person's words as `render`
/// left them, no block and so no frame. The brain way of the same hive shows
/// the note behind its block, framed: the note was live.
#[test]
fn gh1337_the_quick_way_and_its_bridge_carry_the_bare_turn() {
    if !shipped() {
        return;
    }
    let bare = format!("[#{}] {WEATHER}", short_id(&user(WEATHER)));
    let mut h = talky();
    care_note(&mut h);
    simple(&mut h, "", json!([user(WEATHER)]));
    let quick = h.routed("quick");
    assert_eq!(quick.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(last_text(&quick[0]), bare);
    // The tool round: the required call and the bridge beside it.
    let mut h = talky();
    care_note(&mut h);
    simple(&mut h, "tool", json!([user(WEATHER)]));
    let quick = h.routed("quick");
    assert_eq!(
        quick.len(),
        2,
        "call and bridge: {:?} {:?}",
        h.out,
        h.stderr
    );
    let bridge = quick
        .iter()
        .find(|m| m.hop.contains_key("bridge"))
        .expect("the bridge");
    let call = quick
        .iter()
        .find(|m| !m.hop.contains_key("bridge"))
        .expect("the required call");
    assert_eq!(last_text(call), bare);
    assert_eq!(texts(bridge), vec![bare.clone()], "the person's turn alone");
    // The same note on the brain way: block, then the framed words.
    let c = h.curate("s1", "t2", 0, json!([user("c")]), mode("Be brief."));
    assert_eq!(
        last_text(&c),
        format!("{CARE_BLOCK}\n\n{}", framed("c", NOW))
    );
}

/// The tool rounds of one brain turn show the notes of its first call
/// (GH #1298): the framed element stays byte for byte, a later note
/// included, and the first call's messages open the next one.
#[test]
fn gh1337_the_tool_rounds_of_a_turn_keep_the_framed_words() {
    if !shipped() {
        return;
    }
    let mut h = talky();
    turn(&mut h, "s1", "t1", "a", "b", json!({}));
    care_note(&mut h);
    let c0 = h.curate("s1", "t2", 0, json!([user("q2")]), mode("Be brief."));
    let first = last_text(&c0);
    assert_eq!(first, format!("{CARE_BLOCK}\n\n{}", framed("q2", NOW)));
    h.tap(
        &c0,
        "tool_calls",
        json!({}),
        json!([tool_call("c1", "look")]),
    );
    h.lane(
        "in_note",
        json!({"session_id": "s1"}),
        json!({}),
        json!({"notes": [{"id": "n2", "source": "care", "text": "A later hint.",
                          "scope": "session"}]}),
    );
    let c1 = h.curate(
        "s1",
        "t2",
        1,
        json!([
            user("q2"),
            tool_call("c1", "look"),
            tool_result("c1", "rain")
        ]),
        mode("Be brief."),
    );
    let n = c0.messages().len();
    assert_eq!(
        c1.messages()[..n],
        c0.messages()[..],
        "the first call opens the second byte for byte"
    );
    assert_eq!(texts(&c1)[n - 1], first);
}

/// Review M-1: a person's text the rebuild turned into its one-line pointer
/// is no words -- no frame stands before it.
#[test]
fn gh1337_a_released_text_gets_no_frame() {
    if !shipped() {
        return;
    }
    let mut h = talky();
    let cake = user(CAKE);
    let cid = short_id(&cake);
    let c1 = h.curate("s1", "t1", 0, ambient_round(CAKE), mode("x"));
    h.tap(&c1, "stop", json!({}), json!([said("Lovely.")]));
    let c2 = h.curate("s1", "t2", 0, json!([user("next")]), mode("x"));
    h.tap(
        &c2,
        "stop",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
        json!([said("done")]),
    );
    h.section(&c2, "window", json!({"release": [format!("#{cid}")]}));
    let c3 = h.curate("s1", "t3", 0, json!([user("third")]), mode("x"));
    h.tap(
        &c3,
        "stop",
        json!({"cache_expires_at": "2099-01-01T00:01:00Z"}),
        json!([said("three")]),
    );
    h.fire(&last_add(&h));
    let c4 = h.curate("s1", "t4", 0, json!([user("fourth")]), mode("x"));
    let first = texts(&c4)[0].clone();
    let pointer = format!("[#{cid} released \u{2014} history_read(\"#{cid}\")]");
    assert!(
        first.contains(RECALL_FRAME),
        "the recall is folded: {first:?}"
    );
    assert!(
        first.ends_with(&format!("\n\n{pointer}")),
        "the pointer closes the element bare: {first:?}"
    );
}
