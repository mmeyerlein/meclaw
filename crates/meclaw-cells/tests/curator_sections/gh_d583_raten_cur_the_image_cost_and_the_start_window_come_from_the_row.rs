//! GH #1532 (wave d58-3, strand raten) -- the curator counts an image at the
//! model's catalog row and cuts a start window as a share of the target's
//! window ("the model dictates, no fixed token limits in the code").
//!
//! Before, `./policy` and `./history` weighed an inline image at a fixed 1024
//! tokens whatever model the window belonged to, while the llm cell counts it
//! at the row's `tokens_per_image` (`window.rs` `estimate_prompt_tokens`);
//! and a start window (`seed`) whose ask named the target's window but no
//! `start_budget` was cut at the fixed `seed_budget` of 8192. Now:
//!
//! 1. the package line's `tokens_per_image` is held with the row
//!    (`catalog:<target>`), only when the line names one;
//! 2. the projection of a call (`window_est:<call>`) counts an image at the
//!    target's row, and the tap's look one window ahead (`hard` at
//!    HARD_HEADROOM of `input_hard`) counts the answer's image the same way;
//! 3. a view and a start window count an image at the image cost their ask
//!    names for its target;
//! 4. an ask that names its target's window (`limits.input_soft`) but no
//!    `start_budget` gets the share `seed_share` of that window; `seed_budget`
//!    stays the fallback without a window and a set's number still stands
//!    (OR-d54g.kprofil.18); a set override of `seed_share` is taken;
//! 5. without a row nothing moves: 1024 an image, 8192 a start window.
//!
//! The hive runs in one process (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;
// harness_fold: #[path = "support/curator_live.rs"]
#[allow(unused_imports)]
use crate::curator_live; // harness_fold: mod curator_live;

use curator_hive::*;
use curator_live::{LIVE, VOICE_SESSION, first_line, live_hive};
use curator_target::{announce, answer, curate_for, held, roomy, the_call};
use meclaw_core::serde_json::{self as sj, Value, json};

/// An inline image as a turn carries it.
const IMAGE: &str = "data:image/png;base64,AAAA";
/// The image cost of the row these pins name, and the core's.
const ROW_IMAGE: i64 = 1600;
const CORE_IMAGE: i64 = 1024;

/// The roomy row of `brain`, with the image cost `image` (None: the line
/// names none, `null` as the llm cell sends it).
fn brain_row(image: Option<i64>) -> Value {
    let mut r = roomy("test/brain");
    r["tokens_per_image"] = image.map_or(Value::Null, |n| json!(n));
    r
}

/// A hive whose target `brain` reported `brain_row(image)`.
fn hive_with(image: Option<i64>) -> Hive {
    let mut h = Hive::new();
    announce(&mut h, "brain", brain_row(image), json!({}), json!([]));
    h
}

/// The projection of `call` (`window_est:<call>`), parsed.
fn est_row_of(h: &Hive, call: &Msg) -> Value {
    let id = call.hop["curator_call"].as_str().unwrap_or_default();
    sj::from_str(&h.state(&format!("window_est:{id}"))).unwrap_or(Value::Null)
}

/// One curate for `brain` whose turn is one inline image.
fn image_call(h: &mut Hive) -> Msg {
    let calls = curate_for(h, Some("brain"), "t1", 0, json!([user(IMAGE)]), json!({}));
    the_call(h, &calls)
}

#[test]
fn gh_d583_raten_cur_the_held_row_keeps_the_lines_image_cost() {
    if !shipped() {
        return;
    }
    let h = hive_with(Some(ROW_IMAGE));
    assert_eq!(
        held(&h, "brain")["tokens_per_image"],
        json!(ROW_IMAGE),
        "the row is held with its image cost: {:?}",
        h.stderr
    );
    // A line without one: the row of before, no key added.
    let h = hive_with(None);
    let row = held(&h, "brain");
    assert!(row.is_object(), "the row is held: {:?}", h.stderr);
    assert!(
        row.get("tokens_per_image").is_none(),
        "a line without an image cost holds the row it always held: {row}"
    );
}

#[test]
fn gh_d583_raten_cur_the_projection_counts_an_image_at_the_targets_row() {
    if !shipped() {
        return;
    }
    let mut with = hive_with(Some(ROW_IMAGE));
    let mut without = hive_with(None);
    let c_with = image_call(&mut with);
    let c_without = image_call(&mut without);
    let e_with = est_row_of(&with, &c_with);
    let e_without = est_row_of(&without, &c_without);
    let est = |e: &Value| e["est"].as_i64().unwrap_or_else(|| panic!("est: {e}"));
    assert_eq!(
        est(&e_with) - est(&e_without),
        ROW_IMAGE - CORE_IMAGE,
        "the one image is counted at the row's cost: {e_with} {e_without}"
    );
    assert_eq!(
        e_with["image"],
        json!(ROW_IMAGE),
        "the projection keeps the cost it counted with: {e_with}"
    );
    assert!(
        e_without.get("image").is_none(),
        "without a row the projection is the row of before: {e_without}"
    );
}

/// The curate stamp the tap of `call` wrote (`curate:<round>`).
fn curate_mark_of(h: &Hive) -> String {
    let v: Value = sj::from_str(&h.state_in("curate", ROUND_E)).unwrap_or(Value::Null);
    v["curate_mark"].as_str().unwrap_or("").to_string()
}

#[test]
fn gh_d583_raten_cur_the_tap_looks_ahead_at_the_rows_image_cost() {
    if !shipped() {
        return;
    }
    // The answer carries one image. The next window is the projection plus
    // the answer: at the row's 1600 an image it reaches HARD_HEADROOM (0.9)
    // of `input_hard`, at the core's 1024 it stays under it.
    let mut without = hive_with(None);
    let c_without = image_call(&mut without);
    let p = est_row_of(&without, &c_without)["est"]
        .as_i64()
        .expect("a projection");
    // Under the line by 300 tokens with the core's image; the row's adds
    // twice 576 (the call's image and the answer's).
    let hard = ((p + CORE_IMAGE + 300) as f64 / 0.9).ceil() as i64;
    let usage = |p: i64| json!({"tokens_prompt": p, "input_soft": hard - 1, "input_hard": hard});
    answer(
        &mut without,
        &c_without,
        "stop",
        usage(p),
        json!([said(IMAGE)]),
    );
    let mut with = hive_with(Some(ROW_IMAGE));
    let c_with = image_call(&mut with);
    let p_with = est_row_of(&with, &c_with)["est"]
        .as_i64()
        .expect("a projection");
    answer(
        &mut with,
        &c_with,
        "stop",
        usage(p_with),
        json!([said(IMAGE)]),
    );
    assert_eq!(
        curate_mark_of(&with),
        "hard",
        "the next window at the row's image cost reaches the line: {:?}",
        with.stderr
    );
    assert_ne!(
        curate_mark_of(&without),
        "hard",
        "at the core's image cost it does not: {:?}",
        without.stderr
    );
    // The answer's own stamp counts as well, without a row.
    let mut stamped = hive_with(None);
    let c = image_call(&mut stamped);
    answer(
        &mut stamped,
        &c,
        "stop",
        json!({"tokens_prompt": p, "input_soft": hard - 1, "input_hard": hard,
               "tokens_per_image": 2 * ROW_IMAGE}),
        json!([said(IMAGE)]),
    );
    assert_eq!(curate_mark_of(&stamped), "hard", "{:?}", stamped.stderr);
}

/// The view of the person's words (no target) of `budget` tokens, the ask
/// naming `image` as its target's image cost. The budget is said both ways
/// the view has been sized: as `budget_tokens` (the number of the ask) and as
/// the whole share (1.0) of the window the ask names (`hop.input_soft`, GH
/// #1526) -- either way `budget` tokens, so the pin holds before and after
/// strand grenzen.
fn view_with(image: Option<i64>, budget: i64) -> Msg {
    let mut h = Hive::new();
    h.row(backdated(30), "s1", "t1", "user", &user(IMAGE), 1);
    let mut ask = json!({"turns": 2, "budget_tokens": budget, "share": 1.0});
    if let Some(n) = image {
        ask["tokens_per_image"] = json!(n);
    }
    h.out.clear();
    h.lane(
        "in_view_ask",
        json!({"session_id": "s1", "turn_id": "t2", "audience_set": ROUND_E}),
        json!({"input_soft": budget}),
        json!({"messages": [], "view_ask": ask}),
    );
    let views = h.routed("view");
    assert_eq!(views.len(), 1, "one view: {:?} {:?}", h.out, h.stderr);
    views[0].clone()
}

#[test]
fn gh_d583_raten_cur_a_view_counts_an_image_at_the_cost_its_ask_names() {
    if !shipped() {
        return;
    }
    let turns = |v: &Msg| v.body["messages"].as_array().map_or(0, Vec::len);
    // 1100 tokens: the image fits at the core's 1024, not at 1600.
    let core = view_with(None, 1100);
    assert_eq!(turns(&core), 1, "{:?}", core.body);
    assert_eq!(core.body["truncated"], json!(false), "{:?}", core.body);
    let row = view_with(Some(ROW_IMAGE), 1100);
    assert_eq!(turns(&row), 0, "{:?}", row.body);
    assert_eq!(row.body["truncated"], json!(true), "{:?}", row.body);
}

/// The ask for a start window, `limits` in the hop. Returns the one seed.
fn seed_ask(h: &mut Hive, limits: Option<Value>) -> Msg {
    h.out.clear();
    let mut hop = json!({"session_id": VOICE_SESSION, "reason": "start", "renewal_n": 0,
                         "live": LIVE});
    if let Some(l) = limits {
        hop["limits"] = l;
    }
    h.lane(
        "in_seed_ask",
        json!({"audience_set": ROUND_E, "live": LIVE}),
        hop,
        json!({"messages": []}),
    );
    let seeds = h.routed("seed");
    assert_eq!(seeds.len(), 1, "one seed: {:?} {:?}", h.out, h.stderr);
    seeds[0].clone()
}

/// `n` exchanges of session `s1`, a row of 299 characters each (about a
/// hundred tokens at three characters a token).
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

/// The seed's turns (every message that is not a line of the curator's own).
fn turns_of(seed: &Msg) -> usize {
    seed.messages()
        .iter()
        .filter(|m| m["role"] != json!("developer"))
        .count()
}

fn est_of(seed: &Msg) -> i64 {
    seed.body
        .get("est_tokens")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("the seed names est_tokens: {:?}", seed.body))
}

/// Limits that name the target's window (`input_soft`) and its rate, no
/// `start_budget`.
fn window_limits(input_soft: i64) -> Value {
    json!({"model": "test/own-session", "chars_per_token": 3, "input_soft": input_soft})
}

#[test]
fn gh_d583_raten_cur_the_start_window_is_a_share_of_the_targets_window() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    // 6000 tokens of window at the shipped share 0.032768: 196 tokens, at
    // most one row of about a hundred beside the fact line.
    let mut h = live_hive(&[]);
    talk(&mut h, 6);
    let seed = seed_ask(&mut h, Some(window_limits(6000)));
    assert!(
        turns_of(&seed) <= 1 && est_of(&seed) <= 196,
        "the start window is the share of the target's window: {} {} {:?}",
        turns_of(&seed),
        est_of(&seed),
        h.stderr
    );
    assert!(
        first_line(&seed).starts_with("Facts:"),
        "{:?}",
        seed.messages()
    );

    // 250 000 tokens (the fallback row's window) at the shipped share is
    // 8192: the start window of an ask without any window, byte for byte.
    // Both seeds stand at one instant (`now_utc`, the test override of the
    // seed's clock): their fact lines named the second each was built in, and
    // the two ran a second apart (build03, test-20261114T112512Z-ad760637).
    let at = json!(backdated(0).to_rfc3339());
    let mut a = live_hive(&[("history", "now_utc", at.clone())]);
    talk(&mut a, 6);
    let by_share = seed_ask(&mut a, Some(window_limits(250_000)));
    let mut b = live_hive(&[("history", "now_utc", at)]);
    talk(&mut b, 6);
    let by_fallback = seed_ask(
        &mut b,
        Some(json!({"model": "test/own-session", "chars_per_token": 3})),
    );
    assert_eq!(turns_of(&by_share), 12, "{:?}", a.stderr);
    assert_eq!(by_share.messages(), by_fallback.messages());
    assert_eq!(est_of(&by_share), est_of(&by_fallback));

    // A `start_budget` the ask names still wins over the share.
    let mut h = live_hive(&[]);
    talk(&mut h, 6);
    let mut l = window_limits(250_000);
    l["start_budget"] = json!(150);
    let seed = seed_ask(&mut h, Some(l));
    assert!(turns_of(&seed) <= 1, "{:?}", seed.messages());
}

#[test]
fn gh_d583_raten_cur_a_set_seed_budget_still_stands_and_no_window_keeps_8192() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    // OR-d54g.kprofil.18: the instance set `seed_budget` -- its number stands
    // until an ask names a budget, also against the share of a window.
    let mut h = live_hive(&[("history", "seed_budget", json!(150))]);
    talk(&mut h, 6);
    let seed = seed_ask(&mut h, Some(window_limits(250_000)));
    assert!(
        turns_of(&seed) <= 1,
        "a set's number stands: {} {:?}",
        turns_of(&seed),
        h.stderr
    );
    // No limits at all: 8192, the window of before.
    let mut h = live_hive(&[]);
    talk(&mut h, 6);
    let seed = seed_ask(&mut h, None);
    assert_eq!(turns_of(&seed), 12, "{:?}", h.stderr);
}

#[test]
fn gh_d583_raten_cur_a_set_override_of_seed_share_is_taken() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    // 600 tokens of window: at the shipped share 19 tokens, no row; at a
    // set's 0.5 300 tokens, two rows of about a hundred.
    let mut h = live_hive(&[]);
    talk(&mut h, 6);
    let shipped_share = seed_ask(&mut h, Some(window_limits(600)));
    assert_eq!(turns_of(&shipped_share), 0, "{:?}", h.stderr);
    let mut h = live_hive(&[("history", "seed_share", json!(0.5))]);
    talk(&mut h, 6);
    let set_share = seed_ask(&mut h, Some(window_limits(600)));
    assert!(
        turns_of(&set_share) >= 1 && est_of(&set_share) <= 300,
        "the set's share sizes the window: {} {} {:?}",
        turns_of(&set_share),
        est_of(&set_share),
        h.stderr
    );
    // The door takes the override: the param is in the shipped config with
    // its setting, the value the script reads.
    let cfg: Value = sj::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../templates/curator/history/config.json"),
        )
        .expect("config"),
    )
    .expect("json");
    assert_eq!(cfg["params"]["seed_share"], json!(0.032768));
    assert_eq!(
        cfg["contract"]["settings"]["seed_share"]["default"],
        json!(0.032768)
    );
}

#[test]
fn gh_d583_raten_cur_a_start_window_counts_an_image_at_the_cost_its_ask_names() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let est = |image: Option<i64>| {
        let mut h = live_hive(&[]);
        h.row(backdated(30), "s1", "t1", "user", &user(IMAGE), 1);
        let mut l = json!({"model": "test/own-session", "chars_per_token": 3});
        if let Some(n) = image {
            l["tokens_per_image"] = json!(n);
        }
        let seed = seed_ask(&mut h, Some(l));
        assert_eq!(turns_of(&seed), 1, "{:?}", h.stderr);
        est_of(&seed)
    };
    // At a named rate an image is still an image: the core's 1024, not its
    // 26 characters over three.
    let core = est(None);
    let row = est(Some(ROW_IMAGE));
    assert_eq!(row - core, ROW_IMAGE - CORE_IMAGE, "{core} {row}");
    assert!(
        core >= CORE_IMAGE,
        "the image counts 1024 at a rate: {core}"
    );
}
