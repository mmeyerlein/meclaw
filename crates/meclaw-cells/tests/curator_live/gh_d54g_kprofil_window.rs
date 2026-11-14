//! D54g kprofil (GH #1507, contract points 1, 3, 5 and 6) -- the start window
//! and the handover of a renewal follow the limits the ask names.
//!
//! WHY this file exists: a target that holds its own session reports no
//! package and no window profile, so the curator sized its start window by
//! one knob of the instance (`seed_budget`), estimated it with its own rate
//! (bytes / 3) whatever rate the asking cell counts with, and sized the
//! handover of a renewal by the window of the wall (half of `input_soft`:
//! hundreds of thousands of characters for a session that takes a few
//! thousand tokens). The ask for a start window now names the limits of its
//! target in `hop.limits` -- `{model, start_budget, feed_budget,
//! chars_per_token, profile}` -- and the curator holds them per handle
//! `live` until the session ends:
//!
//! * the start window is cut to `limits.start_budget` where the ask names it,
//!   else to `seed_budget` as before; the oldest turns go first, the fact
//!   line never;
//! * the estimate uses `limits.chars_per_token` where named;
//! * the handover block of a renewal is at most half of `start_budget`, in
//!   characters at the target's rate, not half of the wall's `input_soft`;
//! * the end of the session (`in_live_end`) forgets the limits;
//! * a turn whose head says `cut: true` (a sentence the end of the session
//!   tore off) is no said line in a start window;
//! * an ask without `limits` gets the window it got before.
//!
//! Nothing here names a role, a channel, a provider or a product: any cell
//! that holds a session of its own may ask this way.
//!
//! The hive in one process (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_live.rs"]
#[allow(unused_imports)]
use crate::curator_live; // harness_fold: mod curator_live;

use curator_hive::*;
use curator_live::*;
use meclaw_core::serde_json::{Value, json};

/// The share of a window a handover block takes (`HANDOVER_SHARE` of
/// `curator/handover`).
const HANDOVER_SHARE: f64 = 0.5;

/// The limits a target names in its ask: `start_budget` tokens for the start
/// window, counted at `rate` characters a token, its profile as it is.
fn limits(start_budget: i64, rate: i64, profile: Value) -> Value {
    json!({"model": "test/own-session", "start_budget": start_budget, "feed_budget": 500,
           "chars_per_token": rate, "profile": profile})
}

/// The ask for a start window of the handle `live`, `limits` in the hop as
/// the asking cell writes them (`None`: the ask of before). Returns the one
/// seed.
fn ask(h: &mut Hive, live: &str, reason: &str, limits: Option<Value>) -> Msg {
    h.out.clear();
    let mut hop = json!({"session_id": VOICE_SESSION, "reason": reason, "renewal_n": 0,
                         "live": live});
    if let Some(l) = limits {
        hop["limits"] = l;
    }
    h.lane(
        "in_seed_ask",
        json!({"audience_set": ROUND_E, "live": live}),
        hop,
        json!({"messages": []}),
    );
    let seeds = h.routed("seed");
    assert_eq!(seeds.len(), 1, "one seed: {:?} {:?}", h.out, h.stderr);
    seeds[0].clone()
}

/// `n` exchanges of session `s1` on the wall, a row of 299 characters each
/// (about a hundred tokens at three characters a token, fifty at six).
fn talk(h: &mut Hive, n: i64) {
    for i in 0..n {
        let words = format!("{i} {}", "the roses by the old garden wall ".repeat(9));
        h.row(
            backdated(50 - 2 * i),
            "s1",
            &format!("t{i}"),
            "user",
            &user(&words),
            1,
        );
        h.row(
            backdated(49 - 2 * i),
            "s1",
            &format!("t{i}"),
            "assistant",
            &said(&words),
            1,
        );
    }
}

/// The window's turns: every message of the seed that is not a line of the
/// curator's own (role `developer`).
fn turns_of(seed: &Msg) -> usize {
    seed.messages()
        .iter()
        .filter(|m| m["role"] != json!("developer"))
        .count()
}

/// The seed's own estimate of its size.
fn est_of(seed: &Msg) -> i64 {
    seed.body
        .get("est_tokens")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("the seed names est_tokens: {:?}", seed.body))
}

/// The rows of the ledger's `state` that hold the limits of the handle
/// `live`: a row keyed by the handle whose value names a start budget.
fn limits_held(h: &Hive, live: &str) -> Vec<String> {
    h.rows(&format!(
        "SELECT key FROM state WHERE key LIKE '%{live}%' AND value LIKE '%start_budget%' \
         ORDER BY key"
    ))
    .iter()
    .filter_map(|r| r[0].as_str().map(str::to_string))
    .collect()
}

/// The window of the wall this hive serves, as an answer's stamp leaves it
/// in the ledger: large, so half of it is far above any block of these pins.
fn wall_window(h: &Hive) {
    h.db.execute(
        "INSERT OR REPLACE INTO state (key, value) VALUES ('input_soft', '250000')",
        [],
    )
    .expect("the state row is written");
}

/// The renewal of the running session `live` (`in_renewed`): the block the
/// new session is told, without its head line.
fn handover_block(h: &mut Hive, live: &str, n: u32) -> String {
    h.out.clear();
    h.lane(
        "in_renewed",
        json!({"session_id": "s1", "live": live, "channel": "test",
               "audience_set": ROUND_E}),
        json!({"live": live, "session_id": "s1", "renewal_n": n}),
        json!({"messages": []}),
    );
    let told = h.routed("sidecar");
    assert_eq!(told.len(), 1, "one block: {:?} {:?}", h.out, h.stderr);
    assert_eq!(
        told[0].hop["section"],
        json!("context"),
        "{:?}",
        told[0].hop
    );
    assert_eq!(told[0].hop["renewal_n"], json!(n), "{:?}", told[0].hop);
    let text = told[0].body["payload"]
        .as_str()
        .unwrap_or_else(|| panic!("the block is words: {:?}", told[0].body))
        .to_string();
    // The head line says it is a renewal; the block follows on the next line.
    text.split_once('\n')
        .map(|(_, block)| block.to_string())
        .unwrap_or_default()
}

/// The end of the running session `live` (`in_live_end`), answered once.
fn end_session(h: &mut Hive, live: &str) {
    h.out.clear();
    h.lane(
        "in_live_end",
        json!({"live": live, "audience_set": ROUND_E}),
        json!({"live": live, "session_id": live, "final": true, "seconds": 60}),
        json!({"messages": []}),
    );
    assert_eq!(
        h.routed("live_facts").len(),
        1,
        "the end is answered: {:?} {:?}",
        h.out,
        h.stderr
    );
}

#[test]
fn gh_d54g_kprofil_the_start_window_is_cut_to_the_budget_the_ask_names() {
    if !shipped() {
        return;
    }
    // The instance's knob as shipped (8192) carries the six exchanges; the
    // ask names 150 tokens, and at most one row of about a hundred fits. The
    // profile names no `closes_turn`: a target with limits needs none.
    let mut h = live_hive(&[]);
    clear_of_midnight();
    talk(&mut h, 6);
    let seed = ask(&mut h, LIVE, "start", Some(limits(150, 3, json!({}))));
    assert!(
        turns_of(&seed) <= 1,
        "start_budget 150 holds at most one row of about a hundred tokens: {} {:?}",
        turns_of(&seed),
        h.stderr
    );
    assert!(
        first_line(&seed).starts_with("Facts:"),
        "the fact line never goes: {:?}",
        seed.messages()
    );
    assert!(
        est_of(&seed) <= 150,
        "the window is inside the budget the ask names: {}",
        est_of(&seed)
    );

    // Review 1 I1: a budget of 0 is a budget -- the asking cell names it when
    // its fixed texts fill its window. It is the smallest window, not the
    // knob's: no turn at all, the fact line as ever.
    let mut h = live_hive(&[]);
    talk(&mut h, 6);
    let seed = ask(&mut h, LIVE, "start", Some(limits(0, 3, json!({}))));
    assert_eq!(
        turns_of(&seed),
        0,
        "start_budget 0 carries no turn: {:?}",
        h.stderr
    );
    assert!(
        first_line(&seed).starts_with("Facts:"),
        "the fact line never goes, at a budget of 0 either: {:?}",
        seed.messages()
    );
    assert_eq!(
        limits_held(&h, LIVE).len(),
        1,
        "the limits are held as the ask named them"
    );

    // The ask's budget wins over the knob in both directions: a knob of 150
    // and an ask that names 8192 carries the six exchanges.
    let mut h = live_hive(&[("history", "seed_budget", json!(150))]);
    talk(&mut h, 6);
    let seed = ask(&mut h, LIVE, "start", Some(limits(8192, 3, json!({}))));
    assert!(
        turns_of(&seed) >= 6,
        "the budget of the ask, not the knob, sizes the window: {} {:?}",
        turns_of(&seed),
        h.stderr
    );
}

#[test]
fn gh_d54g_kprofil_an_ask_without_limits_gets_the_window_of_today() {
    if !shipped() {
        return;
    }
    // A hive that never heard of limits: the window of today.
    let mut plain = live_hive(&[]);
    clear_of_midnight();
    talk(&mut plain, 6);
    let today = ask(&mut plain, "call-live-8", "start", None);
    assert!(
        turns_of(&today) >= 6,
        "{} {:?}",
        turns_of(&today),
        plain.stderr
    );

    // A hive in which one handle named limits: that handle's window is cut,
    // and another handle that names none gets the window of today -- the
    // same messages after the fact line (which carries the moment).
    let mut h = live_hive(&[]);
    talk(&mut h, 6);
    let cut = ask(&mut h, LIVE, "start", Some(limits(150, 3, json!({}))));
    assert!(
        turns_of(&cut) <= 1,
        "the handle that names limits is cut by them: {} {:?}",
        turns_of(&cut),
        h.stderr
    );
    let other = ask(&mut h, "call-live-8", "start", None);
    assert_eq!(
        other.messages()[1..],
        today.messages()[1..],
        "an ask without limits is sized by the knob, whatever another handle named"
    );
    assert_eq!(
        limits_held(&h, "call-live-8"),
        Vec::<String>::new(),
        "nothing is held for a handle that named nothing"
    );
}

#[test]
fn gh_d54g_kprofil_the_estimate_uses_the_rate_of_the_asking_target() {
    if !shipped() {
        return;
    }
    // Twelve rows of 299 characters and a budget of 450 tokens: at three
    // characters a token a row is a hundred tokens and four fit beside the
    // fact line; at six it is fifty and at least seven fit.
    let window = |rate: i64| {
        let mut h = live_hive(&[]);
        clear_of_midnight();
        talk(&mut h, 6);
        let seed = ask(&mut h, LIVE, "start", Some(limits(450, rate, json!({}))));
        (turns_of(&seed), est_of(&seed))
    };
    let (at_three, est_three) = window(3);
    let (at_six, est_six) = window(6);
    assert!(
        at_three <= 4,
        "three characters a token: four rows of a hundred tokens at most, got {at_three}"
    );
    assert!(
        at_six >= 7,
        "six characters a token: the same rows cost fifty tokens, got {at_six}"
    );
    assert!(
        est_three <= 450 && est_six <= 450,
        "the seed's own estimate is the one it was cut by: {est_three} {est_six}"
    );

    // Review 1 M1: a rate with a fraction is counted as the asking cell
    // counts it -- in thousandths of a character, whole numbers. A text of
    // 300 characters at 2.4 a token is 125 tokens; a division in floating
    // point made it 126, one more for every text that divides evenly.
    let even = format!("{}abcdef", "abcde ".repeat(49));
    assert_eq!(even.chars().count(), 300, "the fixture divides by 2.4");
    let mut h = live_hive(&[]);
    talk(&mut h, 2);
    h.row(backdated(5), "s1", "t-even", "user", &user(&even), 1);
    let seed = ask(
        &mut h,
        LIVE,
        "start",
        Some(
            json!({"model": "test/own-session", "start_budget": 8192, "feed_budget": 500,
                    "chars_per_token": 2.4, "profile": {}}),
        ),
    );
    let texts: Vec<String> = seed
        .messages()
        .iter()
        .map(|m| m["text"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(
        texts.iter().any(|t| t.chars().count() == 300),
        "the row of 300 characters stands in the window as written: {texts:?}"
    );
    let by_the_cell: usize = texts
        .iter()
        .map(|t| (t.chars().count() * 1_000).div_ceil(2_400))
        .sum();
    assert_eq!(
        usize::try_from(est_of(&seed)).ok(),
        Some(by_the_cell),
        "the estimate at 2.4 characters a token is the asking cell's own: {texts:?}"
    );
}

#[test]
fn gh_d54g_kprofil_the_handover_of_a_renewal_fits_the_start_budget() {
    if !shipped() {
        return;
    }
    // 400 tokens at three characters a token: the block may take 600.
    let budget_chars = (HANDOVER_SHARE * 400.0 * 3.0) as usize;

    // Without limits the block is sized by the wall's window: the six
    // exchanges stand in it, far over 600 characters.
    let mut plain = live_hive(&[]);
    clear_of_midnight();
    talk(&mut plain, 6);
    wall_window(&plain);
    ask(&mut plain, LIVE, "start", None);
    let whole = handover_block(&mut plain, LIVE, 1);
    assert!(
        whole.chars().count() > 3 * budget_chars,
        "the wall's window carries the exchanges whole: {} characters",
        whole.chars().count()
    );

    // The target named its start budget: the block is at most half of it.
    let mut h = live_hive(&[]);
    talk(&mut h, 6);
    wall_window(&h);
    ask(&mut h, LIVE, "start", Some(limits(400, 3, json!({}))));
    let block = handover_block(&mut h, LIVE, 1);
    assert!(
        block.chars().count() <= budget_chars,
        "the handover fits half the start budget ({budget_chars} characters), not half \
         the wall's window: {} characters {:?}",
        block.chars().count(),
        h.stderr
    );
    assert!(
        !block.trim().is_empty(),
        "the block is cut, not dropped: its build and order stay"
    );
    // OR-d54g.kprofil.13: for a target with its own budget the block keeps
    // whole units only -- what does not fit is left out and said so, no line
    // is cut with the mark of a cut text.
    assert!(
        !block.contains("...[cut:"),
        "no unit of the block is cut inside: {block:?}"
    );
    assert!(
        block.contains("the roses by the old garden wall"),
        "a whole turn stands in the block: {block:?}"
    );
    for line in block.lines().filter(|l| l.contains("the roses")) {
        assert!(
            line.trim_end()
                .ends_with("the roses by the old garden wall"),
            "a turn stands whole, to its last word: {line:?}"
        );
    }
    // A budget under one turn: 150 tokens are 225 characters for the block,
    // a turn has 305. Before, the newest turn went out as its head with the
    // mark of a cut text; now it is left out whole and the block says so.
    let mut h = live_hive(&[]);
    talk(&mut h, 6);
    wall_window(&h);
    ask(&mut h, LIVE, "start", Some(limits(150, 3, json!({}))));
    let small = handover_block(&mut h, LIVE, 1);
    assert!(
        small.chars().count() <= 225,
        "the block fits half of 150 tokens: {} {small:?}",
        small.chars().count()
    );
    assert!(
        !small.contains("...[cut:") && !small.contains("the roses"),
        "a turn that does not fit whole is not in the block, not in part either: {small:?}"
    );
    assert!(
        small.contains("...[dropped:"),
        "what was left out is said: {small:?} {:?}",
        h.stderr
    );
    // Without limits the cut of before stays: the wall's window is the
    // bound, and a block inside it carries no mark at all.
    assert!(
        !whole.contains("...[cut:") && !whole.contains("...[dropped:"),
        "the block of a target without limits is whole, as before: {whole:?}"
    );

    // Review 1 I1: a start budget of 0 is named -- the block is the smallest
    // one, its head line alone, not half the wall's window.
    let mut h = live_hive(&[]);
    talk(&mut h, 6);
    wall_window(&h);
    ask(&mut h, LIVE, "start", Some(limits(0, 3, json!({}))));
    let none = handover_block(&mut h, LIVE, 1);
    assert!(
        none.trim().is_empty(),
        "start_budget 0 leaves the head line alone: {none:?} {:?}",
        h.stderr
    );
}

#[test]
fn gh_d54g_kprofil_the_limits_are_forgotten_when_the_session_ends() {
    if !shipped() {
        return;
    }
    let budget_chars = (HANDOVER_SHARE * 400.0 * 3.0) as usize;
    let mut h = live_hive(&[]);
    clear_of_midnight();
    talk(&mut h, 6);
    wall_window(&h);
    ask(&mut h, LIVE, "start", Some(limits(400, 3, json!({}))));
    assert_eq!(
        limits_held(&h, LIVE).len(),
        1,
        "the limits are held in the hive's ledger, one row for the handle: {:?} {:?}",
        state_keys(&h, ""),
        h.stderr
    );

    end_session(&mut h, LIVE);
    assert_eq!(
        limits_held(&h, LIVE),
        Vec::<String>::new(),
        "the end of the session forgets the limits"
    );
    // What a later renewal under the same handle is sized by is the wall's
    // window again.
    let block = handover_block(&mut h, LIVE, 1);
    assert!(
        block.chars().count() > budget_chars,
        "no start budget of an ended session sizes a block: {} characters",
        block.chars().count()
    );
}

/// One turn of the running session through the lanes: the person's words on
/// `in_curate`, the answer on the tap. `cut`: the head of the turn says the
/// end of the session tore the answer off (`cut: true`, on the seam's hop and
/// context and on the answer's hop).
fn live_turn(h: &mut Hive, turn: &str, words: &str, reply: &str, cut: bool) {
    h.out.clear();
    let mut hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                         "path": "talk", "needs": "", "families": "", "live": LIVE});
    let mut ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                         "channel": "test", "audience_set": ROUND_E, "path": "talk",
                         "live": LIVE});
    if cut {
        hop["cut"] = json!(true);
        ctx["cut"] = json!(true);
    }
    h.lane("in_curate", ctx, hop, json!({"messages": [user(words)]}));
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "{:?} {:?}", h.out, h.stderr);
    h.out.clear();
    let head = if cut { json!({"cut": true}) } else { json!({}) };
    h.tap(&calls[0], "stop", head, json!([said(reply)]));
}

/// The texts of a seed's messages of `role`.
fn said_by(seed: &Msg, role: &str) -> Vec<String> {
    seed.messages()
        .iter()
        .filter(|m| m["role"] == json!(role))
        .map(|m| m["text"].as_str().unwrap_or("").to_string())
        .collect()
}

#[test]
fn gh_d54g_kprofil_a_cut_turn_is_no_said_line_in_a_start_window() {
    if !shipped() {
        return;
    }
    const ASKED: &str = "What did the gardener say about the roses?";
    const WHOLE: &str = "He will come on Friday and bring new soil.";
    const TORN: &str = "He will come on Friday and bring";

    // The control: a whole answer is a said line of the next start window.
    let mut whole = live_hive(&[]);
    clear_of_midnight();
    ask(&mut whole, LIVE, "start", None);
    live_turn(&mut whole, "t1", ASKED, WHOLE, false);
    let seed = ask(&mut whole, LIVE, "renew", None);
    assert!(
        said_by(&seed, "assistant")
            .iter()
            .any(|t| t.contains(WHOLE)),
        "a whole answer is a said line of the window: {:?} {:?}",
        seed.messages(),
        whole.stderr
    );

    // The same turn with `cut: true`: the person's words stand, the torn
    // sentence is nothing the companion said.
    let mut h = live_hive(&[]);
    ask(&mut h, LIVE, "start", None);
    live_turn(&mut h, "t1", ASKED, TORN, true);
    let seed = ask(&mut h, LIVE, "renew", None);
    assert!(
        said_by(&seed, "user").iter().any(|t| t.contains(ASKED)),
        "the person's words of the turn stand: {:?} {:?}",
        seed.messages(),
        h.stderr
    );
    assert!(
        !said_by(&seed, "assistant").iter().any(|t| t.contains(TORN)),
        "a cut turn is no said line of a start window: {:?}",
        seed.messages()
    );
}

/// The hive of the open-note pin (OR-d56.K.7): `note_once` "1" on the three
/// cells that read it, a main target that gives notes a share of the window,
/// and `over` params on top.
fn note_hive(over: &[(&str, &str, Value)]) -> Hive {
    let mut all: Vec<(&str, &str, Value)> = ["intake", "policy", "history"]
        .into_iter()
        .map(|cell| (cell, "note_once", json!("1")))
        .collect();
    all.extend(over.iter().cloned());
    let mut h = Hive::with(&all).reporting(json!({"notes_share": 0.02}));
    h.check_emits = true;
    h
}

/// The notes on `in_note`, for session `s1`.
fn post_notes(h: &mut Hive, notes: Value) {
    h.lane(
        "in_note",
        json!({"session_id": "s1"}),
        json!({}),
        json!({"notes": notes}),
    );
}

/// The `seed` of the running session `live` for `reason`; the receipts it
/// sent stay in `out`.
fn seed_of(h: &mut Hive, live: &str, reason: &str, renewal_n: u64) -> Msg {
    h.out.clear();
    h.lane(
        "in_seed_ask",
        json!({"live": live}),
        json!({"live": live, "session_id": live, "reason": reason,
               "renewal_n": renewal_n}),
        json!({"messages": []}),
    );
    let seeds = h.routed("seed");
    assert_eq!(seeds.len(), 1, "one seed: {:?} {:?}", h.out, h.stderr);
    seeds[0].clone()
}

/// Where the line `text` stands among the seed's messages, and the index of
/// the one `Notes:` line.
fn line_and_notes(seed: &Msg, text: &str) -> (usize, usize) {
    let texts: Vec<String> = seed
        .messages()
        .iter()
        .map(|m| m["text"].as_str().unwrap_or("").to_string())
        .collect();
    let at: Vec<usize> = (0..texts.len())
        .filter(|&i| texts[i].contains(text))
        .collect();
    assert_eq!(at.len(), 1, "the note stands once: {texts:?}");
    let notes: Vec<usize> = (0..texts.len())
        .filter(|&i| texts[i].starts_with("Notes: "))
        .collect();
    assert_eq!(notes.len(), 1, "one line of the other notes: {texts:?}");
    (at[0], notes[0])
}

/// OR-d56.K.7 (GH #1507): an open `once` note with a `receipt` -- a concern
/// its sender wants asked after -- stood as one item of the one `Notes:` line,
/// behind the long notes of the persona, and the model passed it by (R043
/// probe t7, befund2 § 1 (1) c). It now stands as a line of its own, under
/// the head the set names (`seed_once_head` of `./history`), in front of the
/// other notes; the renewal of the same session carries that line on without
/// a second receipt (contract GH #1477 unchanged). A start window without
/// such a note is the window of before, byte for byte.
#[test]
fn gh_d54g_kprofil_an_open_once_note_stands_in_its_own_line_of_the_start_window() {
    if !shipped() {
        return;
    }
    const PERSONA: &str = "Stay warm, curious and brief; speak as a friend would.";
    const OTHER: &str = "The daughter visits on Sunday.";
    const CONCERN: &str = "Ada felt dizzy yesterday; if it fits, ask how she is now.";

    let head = cell_config("history")["params"]["seed_once_head"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(
        !head.trim().is_empty(),
        "./history ships a head for the line of an open note"
    );

    // The start window of the call: the open note in a line of its own,
    // under the head, before the line of the other notes; one receipt.
    let mut h = note_hive(&[]);
    post_notes(
        &mut h,
        json!([
            {"id": "p1", "source": "probe", "text": PERSONA, "scope": "session"},
            {"id": "p2", "source": "probe", "text": OTHER, "scope": "session"},
            {"id": "n1", "source": "probe", "text": CONCERN, "scope": "once",
             "receipt": "r-1"}
        ]),
    );
    let s = seed_of(&mut h, "call-9", "start", 0);
    let (own, notes) = line_and_notes(&s, CONCERN);
    let m = &s.messages()[own];
    assert_eq!(m["role"], json!("developer"), "{m}");
    assert_eq!(
        m["text"],
        json!(format!("{head}{CONCERN}")),
        "the line is the head and the note, nothing else: {:?}",
        s.messages()
    );
    assert!(
        own < notes,
        "the open note stands before the other notes: {:?}",
        s.messages()
    );
    let line = s.messages()[notes]["text"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(
        line.contains(PERSONA) && line.contains(OTHER),
        "the other notes stay in their line: {line}"
    );
    assert_eq!(
        h.routed("note_receipt").len(),
        1,
        "one receipt: {:?} {:?}",
        h.out,
        h.stderr
    );

    // The renewal of the same session carries the line on, no second receipt.
    let r = seed_of(&mut h, "call-9", "renew", 1);
    let (own_r, notes_r) = line_and_notes(&r, CONCERN);
    assert_eq!(
        r.messages()[own_r]["text"],
        json!(format!("{head}{CONCERN}")),
        "{:?}",
        r.messages()
    );
    assert!(own_r < notes_r, "{:?}", r.messages());
    assert!(
        h.routed("note_receipt").is_empty(),
        "a renewal answers nothing: {:?}",
        h.out
    );

    // Without an open `once` note the start window is the one of before:
    // the same as with the head off ("" -- the one `Notes:` line of today),
    // byte for byte. Both on one fixed instant (`now_utc`), so the fact
    // line's clock is no difference.
    let plain = json!([
        {"id": "p1", "source": "probe", "text": PERSONA, "scope": "session"},
        {"id": "p2", "source": "probe", "text": OTHER, "scope": "session"}
    ]);
    let now = ("history", "now_utc", json!("2026-11-13T09:00:00Z"));
    let mut shipped_head = note_hive(std::slice::from_ref(&now));
    post_notes(&mut shipped_head, plain.clone());
    let a = seed_of(&mut shipped_head, "call-9", "start", 0);
    let mut head_off = note_hive(&[now, ("history", "seed_once_head", json!(""))]);
    post_notes(&mut head_off, plain);
    let b = seed_of(&mut head_off, "call-9", "start", 0);
    assert_eq!(
        canonical(&Value::Object(a.body.clone())),
        canonical(&Value::Object(b.body.clone())),
        "a start window without an open note is the window of before"
    );
    assert_eq!(a.hop, b.hop);
    assert!(
        a.messages()
            .iter()
            .any(|m| m["text"] == json!(format!("Notes: {PERSONA}; {OTHER}"))),
        "the one line of today: {:?}",
        a.messages()
    );

    // The head is the set's to switch (DISPATCH-BAU § 6), as an override of
    // the instance: its own head stands in front of the open note, and ""
    // puts the note back into the one `Notes:` line of before.
    let open = json!([
        {"id": "p1", "source": "probe", "text": PERSONA, "scope": "session"},
        {"id": "n1", "source": "probe", "text": CONCERN, "scope": "once",
         "receipt": "r-1"}
    ]);
    let mut own_head = note_hive(&[("history", "seed_once_head", json!("Ask after: "))]);
    post_notes(&mut own_head, open.clone());
    let o = seed_of(&mut own_head, "call-9", "start", 0);
    let (own_o, notes_o) = line_and_notes(&o, CONCERN);
    assert_eq!(
        o.messages()[own_o]["text"],
        json!(format!("Ask after: {CONCERN}")),
        "the instance's head stands in front of the note: {:?}",
        o.messages()
    );
    assert!(own_o < notes_o, "{:?}", o.messages());
    let mut no_head = note_hive(&[("history", "seed_once_head", json!(""))]);
    post_notes(&mut no_head, open);
    let n = seed_of(&mut no_head, "call-9", "start", 0);
    let (at_n, notes_n) = line_and_notes(&n, CONCERN);
    assert_eq!(
        at_n,
        notes_n,
        "with the head off the note is one item of the `Notes:` line: {:?}",
        n.messages()
    );
}

/// The line `./history` writes when the knob of the instance is overruled by
/// the ask (OR-d54g.kprofil.18 (1)), without the numbers.
const OVERRULE: &str = "cuts this start window";

/// OR-d54g.kprofil.18 (1), the N4 finding of the independent D58 review: the
/// `start_budget` an ask names wins over the instance's `seed_budget`
/// (contract point 3) -- and did so silently, so an instance that set the
/// knob never learned it no longer sizes the window. The rule stays; the
/// overrule is now said on stderr with both numbers. An instance on the
/// shipped knob, or an ask that names no budget, says nothing.
#[test]
fn gh_d54g_kprofil_a_set_seed_budget_the_limits_overrule_is_said() {
    if !shipped() {
        return;
    }
    // The knob set at the instance (an override, not the template), the ask
    // names its own budget: one line with both numbers.
    let mut h = live_hive(&[("history", "seed_budget", json!(6000))]);
    clear_of_midnight();
    talk(&mut h, 2);
    let seed = ask(&mut h, LIVE, "start", Some(limits(9000, 3, json!({}))));
    assert!(turns_of(&seed) >= 2, "{} {:?}", turns_of(&seed), h.stderr);
    let said: Vec<&String> = h.stderr.iter().filter(|e| e.contains(OVERRULE)).collect();
    assert_eq!(said.len(), 1, "the overrule is said once: {:?}", h.stderr);
    assert!(
        said[0].contains(
            "curator/history: seed_budget 6000 set, the ask's start_budget 9000 cuts this start window"
        ),
        "the line names the knob and the ask's budget: {:?}",
        said[0]
    );

    // Counter-checks: the knob as shipped, the same ask -- no line; the knob
    // set, an ask without limits -- no line (the knob sizes the window).
    let mut h = live_hive(&[]);
    talk(&mut h, 2);
    ask(&mut h, LIVE, "start", Some(limits(9000, 3, json!({}))));
    assert!(
        !h.stderr.iter().any(|e| e.contains(OVERRULE)),
        "a knob left as shipped is overruled by design, nothing to say: {:?}",
        h.stderr
    );
    let mut h = live_hive(&[("history", "seed_budget", json!(6000))]);
    talk(&mut h, 2);
    ask(&mut h, LIVE, "start", None);
    assert!(
        !h.stderr.iter().any(|e| e.contains(OVERRULE)),
        "an ask without limits overrules nothing: {:?}",
        h.stderr
    );
}
