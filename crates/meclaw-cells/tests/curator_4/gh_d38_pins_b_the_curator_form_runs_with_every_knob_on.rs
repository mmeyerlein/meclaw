//! GH #1451 (D38 pins b) -- the curator form runs with every knob on.
//!
//! WHY this file exists: a set's `curator_form` "1" turns ten params of the
//! curator on at once (`./intake` `fact_mark_kind`; `./policy`
//! `catalog_ask`, `delegation_park_by_call`, `notes_latest_per_item`,
//! `recall_in_wall`, `state_one_per_subject`, `state_scheduled_on_need`,
//! `wellbeing_line`; `./history` `notes_latest_per_item`, `wellbeing_line`).
//! Every knob had its own pin, none ran with the other nine on. Here every
//! test starts from ONE configuration with all ten "1" (`form_hive`,
//! `form_history`): the overrides pass the door of a birth, the calls run
//! with no traceback and no emission the colony would dead-letter
//! (`check_emits`), and each knob keeps the effect its own pin claims.
//! `state_scheduled_on_need` and the confidence of `wellbeing_line` read
//! what `fact_mark_kind` writes, so their marks come out of the intake of
//! the form here (`intake_mark`), never written by hand.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/history_judge.rs"]
#[allow(unused_imports)]
use crate::history_judge; // harness_fold: mod history_judge;

use curator_hive::*;
use history_judge::{history_params, only, seed_ask};
use meclaw_colony::mutation::subtree::{check_override_params, parse_subtree};
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use meclaw_core::serde_json::{self as sj, Map, Value, json};

/// The ten knobs of `curator_form` (soul2me `CURATOR_FORM_KNOBS`).
const FORM: [(&str, &str); 10] = [
    ("intake", "fact_mark_kind"),
    ("policy", "catalog_ask"),
    ("policy", "delegation_park_by_call"),
    ("policy", "notes_latest_per_item"),
    ("policy", "recall_in_wall"),
    ("policy", "state_one_per_subject"),
    ("policy", "state_scheduled_on_need"),
    ("policy", "wellbeing_line"),
    ("history", "notes_latest_per_item"),
    ("history", "wellbeing_line"),
];

/// The form's knobs of one cell, all "1".
fn form_of(cell: &str) -> Map<String, Value> {
    FORM.iter()
        .filter(|(c, _)| *c == cell)
        .map(|(_, k)| ((*k).to_string(), json!("1")))
        .collect()
}

/// The shipped hive with all ten knobs "1", the talky role and `extra`;
/// every emission checked against its cell's contract.
fn form_hive(extra: &[(&str, &str, Value)]) -> Hive {
    let mut over: Vec<(&str, &str, Value)> = vec![("policy", "role", json!("talky"))];
    over.extend(FORM.iter().map(|(c, k)| (*c, *k, json!("1"))));
    over.extend(extra.iter().cloned());
    let mut h = Hive::with(&over);
    h.check_emits = true;
    h
}

/// The params of `./history` with the form's two knobs "1".
fn form_history() -> (String, Map<String, Value>) {
    let over: Vec<(&str, Value)> = FORM
        .iter()
        .filter(|(c, _)| *c == "history")
        .map(|(_, k)| (*k, json!("1")))
        .collect();
    history_params(&over, &["now_utc"])
}

/// The run went clean: no traceback, nothing the colony would dead-letter.
fn clean(h: &Hive) {
    assert!(
        h.emit_violations.is_empty(),
        "an emission the colony would dead-letter: {:?}",
        h.emit_violations
    );
    assert!(
        !h.stderr.iter().any(|e| e.contains("Traceback")),
        "{:?}",
        h.stderr
    );
}

/// The value of the `fact` mark the form's `./intake` writes for `item`.
fn intake_mark(item: Value) -> String {
    let probe = "fact_marks('memory', {'items': [ARGS]}, 'items')";
    let (got, err) = cell_scope("intake", Value::Object(form_of("intake")), probe, item);
    assert!(!err.contains("Traceback"), "{err}");
    got[0][1]
        .as_str()
        .unwrap_or_else(|| panic!("a mark: {got} {err}"))
        .to_string()
}

fn item(subject: &str, predicate: &str, claim: &str, extra: Value) -> Value {
    let mut v = json!({"subject": subject, "predicate": predicate, "claim": claim,
                       "source": "person", "stance": "asserted", "verified": ["claim"]});
    for (k, x) in obj(extra) {
        v[k] = x;
    }
    v
}

fn texts_of(seed: &Msg, prefix: &str) -> Vec<String> {
    seed.messages()
        .iter()
        .filter_map(|m| m["text"].as_str())
        .filter(|t| t.starts_with(prefix))
        .map(str::to_string)
        .collect()
}

// -- the birth -------------------------------------------------------------

fn shipped_registry(root: &std::path::Path) -> TemplatesRegistry {
    let scanned = scan_templates_dir(&root.join("templates")).unwrap_or_default();
    TemplatesRegistry::from_entries(
        scanned
            .into_iter()
            .map(|s| TemplateEntry {
                template_id: format!("scan-{}", s.name),
                name: s.name,
                version: s.version,
                filesystem_path: s.filesystem_path,
            })
            .collect(),
    )
}

/// Pin: a set's override of all ten knobs passes the door of a birth, one
/// override object per cell as the set renders it.
#[test]
fn the_form_passes_the_door() {
    if !shipped() {
        return;
    }
    let root = repo("");
    let parsed = parse_subtree(&root.join("templates/curator"), &shipped_registry(&root))
        .unwrap_or_else(|e| panic!("the shipped curator template parses: {e:?}"));
    for cell in ["intake", "policy", "history"] {
        let node = parsed
            .cells
            .iter()
            .find(|c| c.rel_path == cell)
            .unwrap_or_else(|| panic!("curator has a {cell} cell"));
        let over = Value::Object(form_of(cell));
        if let Err(why) = check_override_params(node, Some(cell), "curator", &over) {
            panic!("the form's {over} on curator/{cell} must pass the door: {why:?}");
        }
    }
}

// -- ./intake: fact_mark_kind ------------------------------------------------

const SCHEDULED: &str = "Ada wants a reminder at 6 pm to call his mother.";

/// Pin: in the form the intake's mark carries kind, start and confidence.
#[test]
fn the_form_marks_kind_start_and_confidence() {
    if !shipped() {
        return;
    }
    let v: Value = sj::from_str(&intake_mark(item(
        "Ada",
        "reminder_at",
        SCHEDULED,
        json!({"fact_kind": "foresight", "valid_from": "2099-11-07T17:00:00Z",
               "confidence": 0.8}),
    )))
    .expect("the mark value is JSON");
    assert_eq!(v["fact_kind"], json!("foresight"), "{v}");
    assert_eq!(v["valid_from"], json!("2099-11-07T17:00:00Z"), "{v}");
    assert_eq!(v["confidence"], json!(0.8), "{v}");
}

// -- ./policy: state_one_per_subject, state_scheduled_on_need ---------------

const PLAIN: &str = "Elias wants to move to Hamburg.";
const OLD: &str = "Ada will call his mother tonight.";
const NEW: &str = "Ada will call his mother at 8 pm.";

fn state_line(needs: &str) -> String {
    clear_of_midnight();
    let mut h = form_hive(&[("policy", "state_block", json!("1"))]);
    turn(
        &mut h,
        "s1",
        "t1",
        "Remind me at six.",
        "I will.",
        json!({}),
    );
    for (back, it) in [
        (40, item("Elias", "moves_to", PLAIN, json!({}))),
        (30, item("Ada", "calls", OLD, json!({}))),
        (20, item("Ada", "calls", NEW, json!({}))),
        (
            10,
            item(
                "Ada",
                "reminder_at",
                SCHEDULED,
                json!({"fact_kind": "foresight", "valid_from": "2099-11-07T17:00:00Z"}),
            ),
        ),
    ] {
        h.mark(backdated(back), "s1", "t1", "fact", &intake_mark(it));
    }
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": "t2", "iter": "0", "phase": "",
                     "path": "talk", "needs": needs, "families": ""});
    let ctx = json!({"session_id": "s1", "turn_id": "t2", "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "talk"});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("How are you?")], "system": mode("Be brief.")}),
    );
    let b = h.routed("brain");
    assert_eq!(b.len(), 1, "{:?} {:?}", h.out, h.stderr);
    clean(&h);
    texts(&b[0])
        .last()
        .cloned()
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with("- [state -- "))
        .unwrap_or("")
        .to_string()
}

/// Pin: one line per subject, the scheduled line only when the turn asks.
#[test]
fn the_form_keeps_one_line_per_subject_and_the_reminder_for_its_time() {
    if !shipped() {
        return;
    }
    let line = state_line("");
    assert!(line.contains(PLAIN) && line.contains(NEW), "{line}");
    assert!(!line.contains(OLD), "state_one_per_subject: {line}");
    assert!(!line.contains(SCHEDULED), "state_scheduled_on_need: {line}");
    let asked = state_line("memory");
    assert!(asked.contains(SCHEDULED), "a turn that asks: {asked}");
}

// -- ./policy + ./history: notes_latest_per_item, wellbeing_line -----------

const ASKED: &str = "due: Question on T-2 (T-2); kind: hint:question";
const DONE: &str = "due: T-2 was rescheduled to 18:00 (T-2); kind: hint:notify";
const OTHER: &str = "due: T-5 tomorrow (T-5); kind: hint:due";
const CALL: &str = "call-9";

fn notes(h: &mut Hive) {
    for (id, it, text) in [
        ("on-000000000001", "it-2", ASKED),
        ("on-000000000002", "it-2", DONE),
        ("on-000000000003", "it-5", OTHER),
    ] {
        let n = json!({"id": id, "source": "orga", "text": text, "scope": "session",
                       "item": it});
        h.lane(
            "in_note",
            json!({"session_id": "s1"}),
            json!({}),
            json!({"notes": [n]}),
        );
    }
}

/// Pin: the text window carries the latest note of an item only.
#[test]
fn the_form_window_keeps_the_latest_note_of_an_item() {
    if !shipped() {
        return;
    }
    let mut h = form_hive(&[]);
    turn(&mut h, "s1", "t1", "Move it to six.", "Shall I?", json!({}));
    notes(&mut h);
    let c = h.curate(
        "s1",
        "t2",
        0,
        json!([user("Elias moves.")]),
        mode("Be brief."),
    );
    clean(&h);
    let text = texts(&c).last().cloned().unwrap_or_default();
    assert!(text.contains(DONE) && text.contains(OTHER), "{text}");
    assert!(!text.contains(ASKED), "notes_latest_per_item: {text}");
}

fn unwell(h: &mut Hive) {
    let at = backdated(30);
    h.row(at, "text-1", "t1", "user", &user("I feel unwell"), 1);
    h.mark(
        at + chrono::Duration::seconds(1),
        "text-1",
        "t1",
        "fact",
        &intake_mark(item(
            "Ada",
            "has_symptom",
            "Ada feels unwell and slept badly.",
            json!({"confidence": 0.9, "wellbeing": true}),
        )),
    );
}

/// A closed turn of the duplex call `CALL`; the live feeds it sent.
fn closed_turn(h: &mut Hive, turn: &str) -> Vec<Msg> {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": "talk", "needs": "", "families": "",
                     "engine": "duplex", "call_id": CALL});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "talk"});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("and then?")]}),
    );
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "{:?} {:?}", h.out, h.stderr);
    h.out.clear();
    h.tap(&calls[0], "stop", json!({}), json!([said("fine")]));
    clean(h);
    h.routed("live_feed")
}

/// Pin (telephone rim, `./history`): the seed of a call carries the open
/// wellbeing once and the latest note of an item; the call's own turn is not
/// fed again -- the feed of `./policy` reads both knobs as the seed does.
#[test]
fn the_form_seed_carries_the_wellbeing_once_and_the_latest_note() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut h = form_hive(&[("policy", "live_feed", json!("1"))]);
    notes(&mut h);
    unwell(&mut h);
    let p = form_history();
    let seed = only(&seed_ask(&h, &p, CALL), "seed");
    let well = texts_of(&seed, "Wellbeing: ");
    assert_eq!(well.len(), 1, "wellbeing_line: {:?}", seed.messages());
    assert!(well[0].contains("\"state\":\"open\""), "{}", well[0]);
    let n = texts_of(&seed, "Notes: ").concat();
    assert!(n.contains(DONE) && n.contains(OTHER), "{n}");
    assert!(!n.contains(ASKED), "notes_latest_per_item: {n}");
    let fed = closed_turn(&mut h, "t5");
    assert!(fed.is_empty(), "the call already has its facts: {fed:?}");
    let later = only(&seed_ask(&h, &p, "call-10"), "seed");
    assert!(
        texts_of(&later, "Wellbeing: ").is_empty(),
        "asked after once: {:?}",
        later.messages()
    );
}

/// Pin (telephone rim, `./policy`): a wellbeing said after the seed reaches
/// the running call as a live feed, its state carrying the open line.
#[test]
fn the_form_feeds_a_wellbeing_said_after_the_seed() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut h = form_hive(&[("policy", "live_feed", json!("1"))]);
    let p = form_history();
    let seed = only(&seed_ask(&h, &p, CALL), "seed");
    assert!(texts_of(&seed, "Wellbeing: ").is_empty());
    unwell(&mut h);
    let fed = closed_turn(&mut h, "t5");
    assert_eq!(fed.len(), 1, "one feed: {fed:?} {:?}", h.stderr);
    let held = h.state(&format!("seed_facts:{CALL}"));
    let v: Value = sj::from_str(&held).unwrap_or_else(|_| panic!("a state: {held}"));
    assert_eq!(v["wellbeing"]["state"], json!("open"), "{held}");
}

// -- ./policy: delegation_park_by_call ---------------------------------------

/// Pin: the turn path of a call parks beside the delegation that runs under
/// the call's own session -- one answer per delegation.
#[test]
fn the_form_parks_the_turn_beside_its_delegation() {
    if !shipped() {
        return;
    }
    const ASK: &str = "When do I call my mother?";
    const OPEN_UNTIL: u64 = 4_102_444_800_000;
    const PHONE: &str = "phone-2026-11-07T09:06:50Z";
    let mut h = form_hive(&[]);
    let seam = |h: &mut Hive, session: &str, turn: &str, extra: Value, body: Value| {
        h.out.clear();
        let mut hop = json!({"session_id": session, "turn_id": turn, "iter": "0",
                             "phase": "", "path": "simple", "needs": "", "families": "",
                             "engine": "duplex", "call_id": CALL});
        for (k, v) in obj(extra) {
            hop[k] = v;
        }
        let ctx = json!({"session_id": session, "turn_id": turn, "iter": "0",
                         "channel": "test", "audience_set": ROUND_E,
                         "path": hop["path"].clone()});
        h.lane("in_curate", ctx, hop, body);
        clean(h);
        h.routed("quick").len() + h.routed("brain").len()
    };
    let label = format!("{CALL}#1");
    let first = seam(
        &mut h,
        CALL,
        &format!("{label}~{OPEN_UNTIL}~ab12cd34"),
        json!({"path": "", "delegation_id": "item_d1"}),
        json!({"messages": [user(&format!("[delegation item_d1 from the voice channel] {ASK}"))]}),
    );
    assert_eq!(first, 1, "the delegation makes its call: {:?}", h.stderr);
    let second = seam(
        &mut h,
        PHONE,
        &label,
        json!({}),
        json!({"messages": [user(ASK)]}),
    );
    let parked = h
        .rows("SELECT value FROM marks WHERE kind = 'turn_parked'")
        .len();
    assert_eq!(
        (second, parked),
        (0, 1),
        "delegation_park_by_call: one answer"
    );
}

// -- ./policy: recall_in_wall --------------------------------------------------

const RECALL_ID: &str = "call_recall_00000000000pinb";
const SAID: &str = "My sister Anna moved to Hamburg.";

fn recall_pair() -> Value {
    json!([
        user("Tell me about Anna."),
        tool_call(RECALL_ID, "memory_recall"),
        tool_result(RECALL_ID, &format!("WHAT WAS SAID\n  user: \"{SAID}\""))
    ])
}

/// Pin: a recall of an earlier round whose quote stands raw in the wall is
/// sent once, its pair gone whole.
#[test]
fn the_form_sends_a_quote_in_the_wall_once() {
    if !shipped() {
        return;
    }
    let mut h = form_hive(&[]);
    turn(&mut h, "s1", "t1", SAID, "Hamburg is lovely.", json!({}));
    let c2 = h.curate("s1", "t2", 0, recall_pair(), mode("Be brief."));
    h.tap(&c2, "stop", json!({}), json!([said("She likes it there.")]));
    let c = h.curate("s1", "t3", 0, json!([user("And?")]), mode("Be brief."));
    clean(&h);
    let text = sj::to_string(&c.messages()).expect("messages");
    assert_eq!(text.matches(SAID).count(), 1, "recall_in_wall: {text}");
    assert!(!text.contains(RECALL_ID), "{text}");
}

/// Pin (pins a): the guard cuts the quoted round in the very call -- the
/// quote still reaches the model.
#[test]
fn the_form_keeps_the_quote_when_the_guard_cuts_its_source() {
    if !shipped() {
        return;
    }
    let package = json!({"input_soft": 2000, "input_hard": 4000, "cost_in": 10.0,
                         "cost_cached_in": 0.0, "cache_expires_at": "2099-01-01T00:00:00Z",
                         "tokens_prompt": 100});
    clear_of_midnight();
    let mut h = form_hive(&[("policy", "keep_recent", json!(2))]);
    let base = backdated(50);
    h.row(base, "s", "r_said", "user", &user(SAID), 0);
    h.row(
        base + chrono::Duration::milliseconds(10),
        "s",
        "r_said",
        "assistant",
        &said("Hamburg is lovely."),
        1,
    );
    for i in 0..20i64 {
        let at = base + chrono::Duration::milliseconds(100 * (i + 1));
        let text = format!("{i} {}", "x".repeat(1200));
        h.row(at, "s", &format!("r{i}"), "user", &user(&text), 0);
        h.row(
            at + chrono::Duration::milliseconds(10),
            "s",
            &format!("r{i}"),
            "assistant",
            &said(&text),
            1,
        );
    }
    turn(&mut h, "s", "t1", "hello", "hi", package.clone());
    let c2 = h.curate("s", "t2", 0, recall_pair(), mode("Be brief."));
    h.tap(&c2, "stop", package, json!([said("She likes it there.")]));
    let c = h.curate("s", "t3", 0, json!([user("And?")]), mode("Be brief."));
    clean(&h);
    let cuts = c.hop["cuts"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|x| x["what"] == "overflow_cut")
        .count();
    let text = sj::to_string(&c.messages()).expect("messages");
    assert_eq!(cuts, 1, "the guard cut in this call: {text}");
    assert!(!text.contains("Hamburg is lovely."), "{text}");
    assert!(
        text.contains(SAID),
        "recall_in_wall after the guard: {text}"
    );
}

// -- ./policy: catalog_ask -------------------------------------------------------

/// Pin: without a catalogue row the session's first call asks once.
#[test]
fn the_form_asks_for_a_missing_row_once() {
    if !shipped() {
        return;
    }
    let mut h = form_hive(&[]);
    let c1 = h.curate("s1", "t1", 0, json!([user("q1")]), mode("Be brief."));
    let asks = h.routed("catalog_ask");
    assert_eq!(asks.len(), 1, "catalog_ask: {:?} {:?}", h.out, h.stderr);
    assert_eq!(asks[0].hop["catalog_for"], json!("brain"));
    h.tap(&c1, "stop", json!({}), json!([said("a1")]));
    h.curate("s1", "t2", 0, json!([user("q2")]), mode("Be brief."));
    assert!(h.routed("catalog_ask").is_empty(), "{:?}", h.out);
    clean(&h);
}
