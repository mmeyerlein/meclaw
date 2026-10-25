//! GH #1303 -- notes: guidance another hive hands the curator for the window,
//! standing right before the newest words of the person.
//!
//! A pin (`in_pin`) stands in the system part, changes the cached head and
//! lives until its `until`; a candidate (`in_candidate`) is shown once, on a
//! trigger, as a recall pair. A hint that has to stay near the newest turn
//! for a whole conversation had no place. `in_note` carries
//! `notes[{id, source, text, scope, until?, priority?}]` in the message's
//! round (`context.audience_set`, required): `turn` for the next window,
//! `session` until the close of the generation, `until` until its time or
//! its withdrawal (the same id with text ""). A turn may also carry
//! `context.turn_notes` on its `in_curate` -- rendered the same way, never
//! stored.
//!
//! The block opens the newest message of the person, in that element: the
//! provider's cache mark stands on the block right before the youngest user
//! text (`mark_stable_history`, `crates/meclaw-cells/src/llm/
//! translate_anthropic.rs`), so a note element of its own would carry that
//! mark and move the cached prefix every turn. Within the element the
//! prefix before it stays byte for byte the window without a note, and the
//! system part never sees one.
//!
//! The hive runs in one process (`support/curator_hive.rs`); the rim test at
//! the end walks the shipped edges from the assistant level down to the
//! curator's intake. Guarded like every template-reading test (GH #49).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_colony::cel_eval::{evaluate_condition, parse_condition};
use meclaw_core::serde_json::{Map, Value, json};

const HEAD_PROBE: &str =
    "[notes for this turn -- from probe; guidance, not something the person said]";

/// A talky curator: the role whose preset gives notes their share.
fn talky() -> Hive {
    Hive::with(&[("policy", "role", json!("talky"))])
}

fn note_door(h: &mut Hive, context: Value, notes: Value) {
    h.lane("in_note", context, json!({}), json!({"notes": notes}));
}

fn note(id: &str, scope: &str, text: &str) -> Value {
    json!({"id": id, "source": "probe", "text": text, "scope": scope})
}

fn last_text(c: &Msg) -> String {
    texts(c).last().cloned().unwrap_or_default()
}

/// `(source, note_id, scope, until, used_turn)` of every `notes` row.
fn note_rows(h: &Hive) -> Vec<Vec<Value>> {
    h.rows(
        "SELECT source, note_id, scope, until, used_turn, text FROM notes \
         ORDER BY source, note_id",
    )
}

/// The `notes` marks the window builds left, parsed.
fn note_marks(h: &Hive) -> Vec<Value> {
    h.rows("SELECT value FROM marks WHERE kind = 'notes' ORDER BY seq")
        .into_iter()
        .map(|r| meclaw_core::serde_json::from_str(r[0].as_str().unwrap_or("{}")).unwrap())
        .collect()
}

#[test]
fn gh1303_a_note_sits_right_before_the_newest_turn() {
    if !shipped() {
        return;
    }
    // The same conversation twice, once with a note.
    let mut with = talky();
    let mut without = talky();
    for h in [&mut with, &mut without] {
        turn(h, "s1", "t1", "a", "b", json!({}));
    }
    note_door(
        &mut with,
        json!({"session_id": "s1"}),
        json!([note("n1", "session", "Stay with the person's topic.")]),
    );
    assert_eq!(note_rows(&with).len(), 1, "{:?}", with.stderr);
    let c = with.curate("s1", "t2", 0, json!([user("c")]), mode("Be brief."));
    let plain = without.curate("s1", "t2", 0, json!([user("c")]), mode("Be brief."));
    let (shown, bare) = (texts(&c), texts(&plain));
    assert_eq!(shown.len(), bare.len(), "no element of its own: {shown:?}");
    // Every element before the newest turn is the window without the note,
    // byte for byte -- the cached prefix does not move.
    assert_eq!(
        c.messages()[..shown.len() - 1],
        plain.messages()[..bare.len() - 1]
    );
    assert_eq!(
        last_text(&c),
        format!(
            "{HEAD_PROBE}\n- Stay with the person's topic.\n\n{}",
            last_text(&plain)
        )
    );
    assert!(last_text(&plain).ends_with('c'), "{bare:?}");
}

#[test]
fn gh1303_a_note_never_enters_the_system_block() {
    if !shipped() {
        return;
    }
    let mut h = talky();
    turn(&mut h, "s1", "t1", "a", "b", json!({}));
    note_door(
        &mut h,
        json!({}),
        json!([note("n1", "until", "Speak slowly.")]),
    );
    let c = h.curate("s1", "t2", 0, json!([user("c")]), mode("Be brief."));
    assert!(last_text(&c).contains("Speak slowly."), "{:?}", texts(&c));
    let system = c
        .body
        .get("system")
        .cloned()
        .unwrap_or(json!({}))
        .to_string();
    assert!(!system.contains("Speak slowly."), "{system}");
    assert!(!system.contains("notes for this turn"), "{system}");
    // Nor in the wall: the person's row is the person's words alone.
    let walls = h.rows(
        "SELECT b.body FROM wall w JOIN blocks b ON b.hash = w.hash \
         WHERE b.body LIKE '%Speak slowly%'",
    );
    assert!(walls.is_empty(), "{walls:?}");
}

#[test]
fn gh1303_a_turn_note_lives_one_window() {
    if !shipped() {
        return;
    }
    let mut h = talky();
    turn(&mut h, "s1", "t1", "a", "b", json!({}));
    note_door(
        &mut h,
        json!({}),
        json!([note("n1", "turn", "Ask one question.")]),
    );
    let c = h.curate("s1", "t2", 0, json!([user("c")]), mode("Be brief."));
    assert!(
        last_text(&c).contains("Ask one question."),
        "{:?}",
        texts(&c)
    );
    h.tap(&c, "stop", json!({}), json!([said("d")]));
    let c = h.curate("s1", "t3", 0, json!([user("e")]), mode("Be brief."));
    assert!(
        !texts(&c).iter().any(|t| t.contains("Ask one question.")),
        "{:?}",
        texts(&c)
    );
    let rows = note_rows(&h);
    assert_eq!(
        rows[0][4],
        json!("s1#t2"),
        "used by the turn it was shown in"
    );
}

#[test]
fn gh1303_a_tool_round_keeps_the_notes_of_its_turn() {
    if !shipped() {
        return;
    }
    // GH #1298: a provider that signs its thinking binds it to every block
    // before it, the person's turn included -- a note that came or went
    // between two calls of one turn would make the replay a refused request.
    let mut h = talky();
    turn(&mut h, "s1", "t1", "a", "b", json!({}));
    note_door(
        &mut h,
        json!({}),
        json!([note("n1", "turn", "Ask one question.")]),
    );
    let c0 = h.curate("s1", "t2", 0, json!([user("q2")]), mode("Be brief."));
    let first = last_text(&c0);
    assert!(first.contains("Ask one question."), "{first}");
    h.tap(
        &c0,
        "tool_calls",
        json!({}),
        json!([tool_call("c1", "look")]),
    );
    note_door(
        &mut h,
        json!({}),
        json!([note("n2", "until", "A later hint.")]),
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
    let person: Vec<String> = c1
        .messages()
        .iter()
        .filter(|m| m["origin"] == "user")
        .map(|m| m["text"].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(person.last(), Some(&first), "{person:?}");
    h.tap(&c1, "stop", json!({}), json!([said("done")]));
    // The next turn takes the notes as they stand then.
    let c2 = h.curate("s1", "t3", 0, json!([user("q3")]), mode("Be brief."));
    let now = last_text(&c2);
    assert!(
        now.contains("A later hint.") && !now.contains("Ask one question."),
        "{now}"
    );
}

#[test]
fn gh1303_a_session_note_ends_with_close() {
    if !shipped() {
        return;
    }
    let mut h = talky();
    turn(&mut h, "s1", "t1", "a", "b", json!({}));
    note_door(
        &mut h,
        json!({"session_id": "s1"}),
        json!([note("n1", "session", "Keep it light.")]),
    );
    let c = h.curate("s1", "t2", 0, json!([user("c")]), mode("Be brief."));
    assert!(last_text(&c).contains("Keep it light."));
    h.tap(&c, "stop", json!({}), json!([said("d")]));
    h.lane(
        "in_close",
        json!({"session_id": "s1"}),
        json!({}),
        json!({"messages": []}),
    );
    let rows = note_rows(&h);
    assert_eq!(rows.len(), 1, "ended, never deleted: {rows:?}");
    assert_ne!(rows[0][3], json!(""), "the close ended it: {rows:?}");
    let c = h.curate("s2", "t1", 0, json!([user("e")]), mode("Be brief."));
    assert!(
        !texts(&c).iter().any(|t| t.contains("Keep it light.")),
        "{:?}",
        texts(&c)
    );
}

#[test]
fn gh1303_an_empty_text_withdraws_the_note() {
    if !shipped() {
        return;
    }
    let mut h = talky();
    turn(&mut h, "s1", "t1", "a", "b", json!({}));
    note_door(
        &mut h,
        json!({}),
        json!([note("n1", "until", "Gone soon.")]),
    );
    note_door(&mut h, json!({}), json!([note("n1", "until", "")]));
    let rows = note_rows(&h);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_ne!(rows[0][3], json!(""), "withdrawn: {rows:?}");
    let c = h.curate("s1", "t2", 0, json!([user("c")]), mode("Be brief."));
    assert!(!texts(&c).iter().any(|t| t.contains("notes for this turn")));
}

/// Review C1 M-3: a withdrawal is the same id with `text: ""`, and its scope
/// does not matter (the row ends now, whatever it was) -- so a withdrawal
/// without `scope` must end the note too instead of being refused on stderr.
#[test]
fn gh1303_a_withdrawal_needs_no_scope() {
    if !shipped() {
        return;
    }
    let mut h = talky();
    turn(&mut h, "s1", "t1", "a", "b", json!({}));
    note_door(
        &mut h,
        json!({}),
        json!([note("n1", "session", "Stays a while.")]),
    );
    note_door(
        &mut h,
        json!({}),
        json!([{"id": "n1", "source": "probe", "text": ""}]),
    );
    let rows = note_rows(&h);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_ne!(rows[0][3], json!(""), "withdrawn without a scope: {rows:?}");
    assert_eq!(rows[0][5], json!(""), "the text is gone: {rows:?}");
    let c = h.curate("s1", "t2", 0, json!([user("c")]), mode("Be brief."));
    assert!(!texts(&c).iter().any(|t| t.contains("Stays a while.")));
}

#[test]
fn gh1303_a_note_without_audience_is_refused() {
    if !shipped() {
        return;
    }
    let mut h = talky();
    note_door(
        &mut h,
        json!({"audience_set": null}),
        json!([note("n1", "until", "Nobody's.")]),
    );
    assert!(note_rows(&h).is_empty());
    assert!(
        h.stderr.join("").contains("missing_audience"),
        "{:?}",
        h.stderr
    );
    // A note out of its bounds is refused alone; its neighbour stays.
    note_door(
        &mut h,
        json!({}),
        json!([note("n2", "forever", "x"), note("n3", "turn", "kept")]),
    );
    let rows = note_rows(&h);
    assert_eq!(rows.len(), 1, "{rows:?} {:?}", h.stderr);
    assert_eq!(rows[0][1], json!("n3"));
}

#[test]
fn gh1303_notes_share_drops_lowest_priority_first() {
    if !shipped() {
        return;
    }
    // 100000 tokens x 0.0006 x 3 characters = 180 characters for the block:
    // the head and two lines of 40, not three.
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "notes_share", json!("0.0006")),
    ]);
    turn(&mut h, "s1", "t1", "a", "b", json!({"input_soft": 100000}));
    let line = |c: char| c.to_string().repeat(40);
    note_door(
        &mut h,
        json!({}),
        json!([
            {"id": "low", "source": "probe", "text": line('l'), "scope": "until", "priority": 7},
            {"id": "top", "source": "probe", "text": line('t'), "scope": "until", "priority": 0},
            {"id": "mid", "source": "probe", "text": line('m'), "scope": "until", "priority": 3}
        ]),
    );
    let c = h.curate("s1", "t2", 0, json!([user("c")]), mode("Be brief."));
    let shown = last_text(&c);
    assert!(
        shown.starts_with(&format!(
            "{HEAD_PROBE}\n- {}\n- {}\n\n",
            line('t'),
            line('m')
        )),
        "{shown}"
    );
    assert!(!shown.contains(&line('l')), "{shown}");
    let marks = note_marks(&h);
    let m = marks.last().expect("a notes mark");
    assert_eq!(
        m["notes_dropped"],
        json!([{"id": "low", "source": "probe", "scope": "until"}]),
        "{m}"
    );
    assert_eq!(m["notes_in"].as_array().map(Vec::len), Some(2), "{m}");
    let cut = c.hop["cuts"]
        .as_array()
        .and_then(|cs| cs.iter().find(|x| x["what"] == "notes").cloned());
    assert_eq!(
        cut,
        Some(json!({"what": "notes", "shown": 2, "total": 3, "unit": "notes"}))
    );
}

#[test]
fn gh1303_context_turn_notes_render_like_notes() {
    if !shipped() {
        return;
    }
    let mut h = talky();
    turn(&mut h, "s1", "t1", "a", "b", json!({}));
    h.out.clear();
    h.lane(
        "in_curate",
        json!({"session_id": "s1", "turn_id": "t2", "iter": "0", "channel": "test",
               "turn_notes": [{"id": "r1", "source": "probe", "text": "Answer in one line.",
                               "priority": 0}]}),
        json!({"session_id": "s1", "turn_id": "t2", "iter": "0", "phase": ""}),
        json!({"messages": [user("c")], "system": mode("Be brief.")}),
    );
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "{:?}", h.stderr);
    assert_eq!(
        last_text(&calls[0]).split("\n\n").next(),
        Some(format!("{HEAD_PROBE}\n- Answer in one line.").as_str())
    );
    // Never stored: no row, and the next turn goes without it.
    assert!(note_rows(&h).is_empty());
    h.tap(&calls[0], "stop", json!({}), json!([said("d")]));
    let c = h.curate("s1", "t3", 0, json!([user("e")]), mode("Be brief."));
    assert!(!texts(&c).iter().any(|t| t.contains("Answer in one line.")));
}

#[test]
fn gh1303_without_a_note_the_window_is_todays() {
    if !shipped() {
        return;
    }
    // Default role (`notes_share` 0): a note is kept, never shown, and said.
    let mut h = Hive::new();
    turn(&mut h, "s1", "t1", "a", "b", json!({}));
    note_door(&mut h, json!({}), json!([note("n1", "until", "Hidden.")]));
    let c = h.curate("s1", "t2", 0, json!([user("c")]), mode("Be brief."));
    assert!(
        !texts(&c).iter().any(|t| t.contains("Hidden.")),
        "{:?}",
        texts(&c)
    );
    assert_eq!(
        note_marks(&h).last().map(|m| m["notes_dropped"].clone()),
        Some(json!([{"id": "n1", "source": "probe", "scope": "until"}]))
    );
    // And a window with no note at all has no notes mark and no notes cut.
    let mut h = talky();
    let c = turn(&mut h, "s1", "t1", "a", "b", json!({}));
    assert!(note_marks(&h).is_empty());
    assert!(
        !c.hop
            .get("cuts")
            .is_some_and(|x| x.to_string().contains("\"notes\"")),
        "{:?}",
        c.hop
    );
}

/// The shipped edges of `template` from its own path, for one hop.
fn targets(template: &str, route: &str) -> Vec<String> {
    let cfg = read_json(&repo(&format!("templates/{template}/config.json")));
    let mut hop = Map::new();
    hop.insert("route".into(), json!(route));
    let ctx = Map::new();
    cfg["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .filter(|e| e["from"] == ".")
        .filter(|e| {
            e["condition"].as_str().is_none_or(|c| {
                matches!(
                    evaluate_condition(&parse_condition(c).expect("a condition"), &ctx, &hop),
                    Ok(true)
                )
            })
        })
        .map(|e| e["to"].as_str().unwrap_or("").to_string())
        .collect()
}

fn accepts(template: &str, route: &str) -> bool {
    let cfg = read_json(&repo(&format!("templates/{template}/config.json")));
    cfg["params"]["contract"]["accepts"]
        .as_array()
        .is_some_and(|a| a.iter().any(|x| x["route"] == route))
}

#[test]
fn gh1303_in_note_crosses_the_assistant_rim() {
    if !shipped() {
        return;
    }
    let mut level = targets("assistant", "in_note");
    level.sort();
    assert_eq!(level, ["./cogny", "./talky", "./talky-chat"]);
    for brain in ["talky", "cogny"] {
        assert!(accepts(brain, "in_note"), "{brain} declares the lane");
        assert_eq!(targets(brain, "in_note"), ["./curator"], "{brain}");
    }
    assert!(accepts("assistant", "in_note"));
    assert!(accepts("curator", "in_note"));
    assert_eq!(targets("curator", "in_note"), ["./intake"]);
    // A close reaches the intake too: it ends the session's notes.
    assert!(targets("curator", "in_close").contains(&"./intake".to_string()));
}
