//! GH #1038 -- the curator knows what its window costs.
//!
//! Before, `./policy` ordered a rebuild by its own numbers: `compress_at` of
//! the usable window (the model's, at most the role's `quality_cap`). The
//! limits of a model differ per model (GH #1037 stamps them on every answer of
//! the llm cell: `hop.input_soft`, `hop.input_hard`, `hop.cost_in`,
//! `hop.cost_cached_in`, cents per million like the catalogue), and a cached
//! window is not free: at a cached price of 10 % of the input price a 500k
//! window costs on every turn what 50k fresh tokens cost. What is pinned:
//!
//! 1. over `input_soft` the window is compressed (`curate_mark` `soft`), over
//!    `input_hard` hard (`hard`); a package's limits replace the role's own
//!    numbers -- `quality_cap` orders nothing any more;
//! 2. under soft, a window whose cached rest costs more over `amortise_turns`
//!    turns than one rebuild is compressed (`cost`); the worked example
//!    amortises after three turns;
//! 3. a rebuild a package ordered aims at the package's share
//!    (`input_soft` * `rebuild_to` / `compress_at`), not at the usable window;
//!    a `cost` rebuild aims lower, at `input_soft` * `rebuild_to`, so the cost
//!    rule bites with the shipped roles at a cached price of 10 % (KF2 review
//!    B2, OR-HK-25);
//! 4. every turn stamps `window_tokens`, `cached_tokens`, a cost estimate in
//!    cents and its `curate_mark`; without the package's values (old packages)
//!    nothing changes and the mark is `none`.
//! 5. the window is measured, never summed: after a `length` continuation the
//!    llm cell reports the sum of both calls, and the policy's own projection
//!    of the call (estimated like the cell estimates a request) stands; and
//!    `hard` comes before the cell's estimate of the next window reaches
//!    `input_hard` (KF1 review M2/M3); a projection no tap read falls with
//!    the next call of its round (KF2 review N3).
//! 6. a cascade writes a `Since` sentence of its own for every version it
//!    bumps and leaves the pins of the sentences before it as written (KF2
//!    review B1).

#[path = "support/curator_hive.rs"]
mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The talky instance as it ships, plus `more` -- the same overlay
/// `curator_policy.rs` measures with.
fn talky(more: &[(&str, &str, Value)]) -> Hive {
    let marker = read_json(&repo("templates/talky/curator/config.json"));
    let mut over: Vec<(String, String, Value)> = Vec::new();
    for (cell, params) in marker["override_params"]
        .as_object()
        .cloned()
        .unwrap_or_default()
    {
        for (k, v) in params.as_object().cloned().unwrap_or_default() {
            over.push((cell.clone(), k, v));
        }
    }
    let mut all: Vec<(&str, &str, Value)> = over
        .iter()
        .map(|(c, k, v)| (c.as_str(), k.as_str(), v.clone()))
        .collect();
    all.extend(more.iter().cloned());
    Hive::with(&all)
}

/// The limits and prices of a model as the llm cell stamps them (GH #1037).
fn package(soft: i64, hard: i64, cost_in: f64, cost_cached_in: f64) -> Value {
    json!({"input_soft": soft, "input_hard": hard, "cost_in": cost_in,
           "cost_cached_in": cost_cached_in})
}

fn with(mut a: Value, b: Value) -> Value {
    for (k, v) in b.as_object().cloned().unwrap_or_default() {
        a[k] = v;
    }
    a
}

/// The turn's stamp the policy keeps for the round of the call.
fn stamp(h: &Hive) -> Value {
    let raw = h.state_in("curate", ROUND_E);
    meclaw_core::serde_json::from_str(&raw).unwrap_or(Value::Null)
}

fn curate_marks(h: &Hive) -> Vec<Value> {
    h.rows("SELECT value FROM marks WHERE kind = 'curate' ORDER BY seq")
        .into_iter()
        .map(|r| meclaw_core::serde_json::from_str(r[0].as_str().unwrap()).unwrap())
        .collect()
}

// ======================================================= 1. soft and hard

/// A round whose window holds a user turn of `chars` characters: the
/// policy's projection of the call is then at least `chars / 3` tokens (the
/// llm cell's estimate), and a measured `tokens_prompt` below it is the
/// window as the provider counted it.
fn turn_of(h: &mut Hive, chars: usize, reply: &str, usage: Value) -> Msg {
    let call = h.curate(
        "s",
        "t1",
        0,
        json!([user(&"x".repeat(chars))]),
        mode("Be brief."),
    );
    h.tap(&call, "stop", usage, json!([said(reply)]));
    call
}

#[test]
fn a_window_over_input_soft_is_compressed() {
    if !shipped() {
        return;
    }
    let mut h = talky(&[]);
    turn_of(
        &mut h,
        6000,
        "a",
        with(
            json!({"tokens_prompt": 1600, "tokens_cached": 1200}),
            package(1500, 100_000, 10.0, 1.0),
        ),
    );
    let add = last_add(&h);
    assert_eq!(add.body["emit_body"]["reason"], "soft");
    assert_eq!(add.body["emit_body"]["curate_mark"], "soft");
    let s = stamp(&h);
    assert_eq!(s["curate_mark"], "soft");
    assert_eq!(s["window_tokens"], 1600, "the measured prompt: {s}");
    assert_eq!(s["cached_tokens"], 1200);
    assert!(s["window_estimate"].as_i64().unwrap() >= 2000, "{s}");
    // 400 fresh at 10 c/M + 1 200 cached at 1 c/M.
    assert!(
        (s["cost_cents"].as_f64().unwrap() - 0.0052).abs() < 1e-12,
        "{s}"
    );
    let marks = curate_marks(&h);
    assert_eq!(marks.len(), 1, "one stamp per turn in the ledger");
    assert_eq!(marks[0]["curate_mark"], "soft");
    assert_eq!(marks[0]["window_tokens"], 1600);
    // The projection is read once, by its call, and gone after the tap.
    assert!(
        h.rows("SELECT key FROM state WHERE key LIKE 'window_est:%'")
            .is_empty()
    );
}

#[test]
fn a_window_over_input_hard_is_compressed_hard() {
    if !shipped() {
        return;
    }
    let mut h = talky(&[]);
    turn_of(
        &mut h,
        6000,
        "a",
        with(
            json!({"tokens_prompt": 1600}),
            package(1000, 1500, 10.0, 1.0),
        ),
    );
    assert_eq!(last_add(&h).body["emit_body"]["reason"], "hard");
    assert_eq!(stamp(&h)["curate_mark"], "hard");
}

#[test]
fn hard_comes_before_the_cells_estimate_of_the_next_window_reaches_it() {
    if !shipped() {
        return;
    }
    // KF1 review M3: the llm cell refuses on its byte estimate (bytes / 3)
    // over `input_hard`, not on measured tokens. Measured, this window (1 600)
    // is under soft (1 700) and under hard (2 300); estimated, the next one --
    // this call (>= 2 000) and its answer (>= 200) -- reaches 0.9 of hard.
    let mut h = talky(&[]);
    turn_of(
        &mut h,
        6000,
        &"y".repeat(600),
        with(
            json!({"tokens_prompt": 1600}),
            package(1700, 2300, 10.0, 1.0),
        ),
    );
    assert_eq!(last_add(&h).body["emit_body"]["reason"], "hard");
    // With room to spare under hard, nothing is ordered. (A cached price of
    // 0 keeps the cost rule out of it: since OR-HK-25 it would rebuild this
    // window, 0.94 of soft, at 10 %.)
    let mut h = talky(&[]);
    turn_of(
        &mut h,
        6000,
        &"y".repeat(600),
        with(
            json!({"tokens_prompt": 1600}),
            package(1700, 4000, 10.0, 0.0),
        ),
    );
    assert!(h.clock.is_empty(), "{:?}", h.clock);
    assert_eq!(stamp(&h)["curate_mark"], "none");
}

#[test]
fn a_continuations_sum_is_not_the_window() {
    if !shipped() {
        return;
    }
    // KF1 review M2: after a `length` continuation `tokens_prompt` is the sum
    // of both calls (3 200), twice the window. Over soft (2 500) by the sum,
    // under it by what the call held (its projection, ~2 000): no rebuild,
    // and the stamp names the projection. A cached price of 0 keeps the
    // cost rule out of it (OR-HK-25 would rebuild 0.8 of soft at 10 %).
    let mut h = talky(&[]);
    turn_of(
        &mut h,
        6000,
        "a",
        with(
            json!({"tokens_prompt": 3200}),
            package(2500, 100_000, 10.0, 0.0),
        ),
    );
    assert!(h.clock.is_empty(), "{:?}", h.clock);
    let s = stamp(&h);
    assert_eq!(s["curate_mark"], "none");
    assert_eq!(s["window_tokens"], s["window_estimate"], "{s}");
    assert!(s["window_tokens"].as_i64().unwrap() < 2500, "{s}");
}

#[test]
fn a_package_replaces_the_roles_own_numbers() {
    if !shipped() {
        return;
    }
    // 70 000 measured is over talky's `compress_at` of its `quality_cap`
    // (0.5 * 120 000) -- the role's own number, which ordered a rebuild
    // before. The model's soft limit is 250 000: nothing to do.
    let mut h = talky(&[]);
    turn_of(
        &mut h,
        240_000,
        "a",
        with(
            json!({"tokens_prompt": 70000}),
            package(250_000, 1_000_000, 10.0, 1.0),
        ),
    );
    assert!(h.clock.is_empty(), "under soft and cheap: nothing ordered");
    let s = stamp(&h);
    assert_eq!(s["curate_mark"], "none");
    assert_eq!(s["window_tokens"], 70000);
}

#[test]
fn the_estimate_is_the_llm_cells() {
    if !shipped() {
        return;
    }
    // `window.rs` `estimate_prompt_tokens`: bytes of every string and key / 3,
    // rounded up; a number or a bool 4 bytes (`window.rs` `walk`, KF2 review
    // N1); an inline image 1024.
    let (got, _) = policy_scope(
        json!({}),
        "[estimate_tokens({'ab': 'xyz'}), estimate_tokens(['\u{e4}']), \
         estimate_tokens({'u': 'data:image/png;base64,AAAA'}), \
         window_of(3200, 2100), window_of(1600, 2100), window_of(None, 2100), \
         window_of(1600, 0), estimate_tokens([1, True, 2.5, None])]",
        Value::Null,
    );
    assert_eq!(got, json!([2, 1, 1025, 2100, 1600, 2100, 1600, 4]));
}

// ============================================ 2. the price of a cached rest

#[test]
fn under_soft_a_dear_cache_is_compressed_for_cost() {
    if !shipped() {
        return;
    }
    // talky as it ships (`rebuild_to` 0.35): a cost rebuild of a soft limit
    // of 5 000 aims at 1 750. A window of 4 000 keeps 2 250 cached at 1 c/M
    // -- 0.00225 cents a turn, 0.0225 over ten turns -- against one rebuild
    // of 1 750 fresh at 10 c/M = 0.0175 cents: paid back on the eighth turn.
    let usage = with(
        json!({"tokens_prompt": 4000, "tokens_cached": 3900}),
        package(5000, 100_000, 10.0, 1.0),
    );
    let mut h = talky(&[]);
    turn_of(&mut h, 15_000, "a", usage.clone());
    assert_eq!(last_add(&h).body["emit_body"]["reason"], "cost");
    let s = stamp(&h);
    assert_eq!(s["curate_mark"], "cost");
    assert_eq!(
        h.state_in("aim", ROUND_E),
        "1750",
        "input_soft * rebuild_to"
    );
    // 100 fresh at 10 c/M + 3 900 cached at 1 c/M.
    assert!(
        (s["cost_cents"].as_f64().unwrap() - 0.0049).abs() < 1e-12,
        "{s}"
    );
    // Two turns to come do not pay a rebuild back: nothing ordered.
    let mut h = talky(&[("policy", "amortise_turns", json!(2))]);
    turn_of(&mut h, 15_000, "a", usage);
    assert!(h.clock.is_empty(), "{:?}", h.clock);
    assert_eq!(stamp(&h)["curate_mark"], "none");
}

/// KF2 review B2 (OR-HK-25): with the roles AS THEY SHIP and a cached price
/// of 10 %, `cost` fires under soft -- above `2 * rebuild_to` of `input_soft`
/// at ten turns (talky and consult 0.7, coding and research 0.5) -- and a
/// `soft` rebuild still aims at `rebuild_to` / `compress_at` of it. Before,
/// a cost rebuild aimed at the soft share too (talky 0.7 of soft): the
/// cached rest had to cost more than 23 % of the input price, and the rule
/// never fired.
#[test]
fn the_shipped_roles_compress_for_cost_under_soft() {
    if !shipped() {
        return;
    }
    let pkg = "{'input_soft': 100000, 'input_hard': 1000000, 'cost_in': 10, \
               'cost_cached_in': 1}";
    for (role, aim, soft_aim, above, below) in [
        ("talky", 35_000, 70_000, 75_000, 65_000),
        ("consult", 35_000, 70_000, 75_000, 65_000),
        ("coding", 25_000, 62_500, 55_000, 45_000),
        ("research", 25_000, 62_500, 55_000, 45_000),
    ] {
        let (got, err) = policy_scope(
            json!({"role": role}),
            &format!(
                "[curate_mark({above}, {pkg}), curate_mark({below}, {pkg}), \
                 curate_mark(100000, {pkg})]"
            ),
            Value::Null,
        );
        assert_eq!(got[0]["mark"], "cost", "{role} at {above}: {got} {err}");
        assert_eq!(got[0]["rebuild_to"], aim, "{role}: {got}");
        assert_eq!(got[1]["mark"], "none", "{role} at {below}: {got}");
        assert_eq!(got[2]["mark"], "soft", "{role}: {got}");
        assert_eq!(got[2]["rebuild_to"], soft_aim, "{role}: {got}");
    }
    // The role without an aim never rebuilds for cost.
    let (got, _) = policy_scope(
        json!({"role": ""}),
        &format!("curate_mark(90000, {pkg})"),
        Value::Null,
    );
    assert_eq!(got["mark"], "none", "{got}");
}

#[test]
fn the_worked_example_amortises_after_three_turns() {
    if !shipped() {
        return;
    }
    // cost_in 10 c/M (0.10 $/M), cached 10 % of it, a 500k window, rebuilt
    // to 100k: keeping it costs 400k * 1 c/M = 0.4 cents a turn more than the
    // rebuilt one, the rebuild 100k * 10 c/M = 1 cent once -- 1 / 0.4 = 2.5,
    // so the third turn pays it back.
    // A cost rebuild aims at `input_soft` * `rebuild_to` (OR-HK-25): 0.1 of
    // a million is the 100k; any other mark keeps `rebuild_to` /
    // `compress_at` of it (200k).
    let knobs = json!({"compress_at": 0.5, "rebuild_to": 0.1});
    let (got, err) = policy_scope(
        knobs.clone(),
        "amortise_after(500000, 100000, 10, 1)",
        Value::Null,
    );
    assert_eq!(got, json!(3), "{err}");
    for (n, want) in [(2, "none"), (3, "cost"), (10, "cost")] {
        let (got, err) = policy_scope(
            with(knobs.clone(), json!({"amortise_turns": n})),
            "curate_mark(500000, {'input_soft': 1000000, 'input_hard': 2000000, \
             'cost_in': 10, 'cost_cached_in': 1})",
            Value::Null,
        );
        assert_eq!(got["mark"], json!(want), "N = {n}: {got} {err}");
        let aim = if want == "cost" { 100000 } else { 200000 };
        assert_eq!(got["rebuild_to"], json!(aim), "derived from input_soft");
    }
    // The shipped default is ten turns.
    let (got, _) = policy_scope(json!({}), "AMORTISE_TURNS", Value::Null);
    assert_eq!(got, json!(10));
}

// ================================================ 3. the aim of the rebuild

/// A talky with two tool results of ~3 000 characters each and a usable
/// window far above them, rebuilt on a turn whose package says `soft`.
fn under_package_pressure(soft: i64) -> (Hive, String) {
    let mut h = talky(&[
        ("policy", "keep_recent", json!(2)),
        ("policy", "quality_cap", json!(1_000_000)),
        ("policy", "keep_rounds", json!(1)),
    ]);
    let base = chrono::Utc::now() - chrono::Duration::seconds(30);
    let at = |s: i64| base + chrono::Duration::seconds(s);
    let big = format!("{}\nsecond line", "A".repeat(3000));
    h.row(at(1), "s", "r1", "user", &user("look twice"), 0);
    h.row(at(2), "s", "r1", "tool_call", &tool_call("c1", "look"), 0);
    let (_, first) = h.row(at(3), "s", "r1", "tool_result", &tool_result("c1", &big), 0);
    h.row(at(4), "s", "r1", "assistant", &said("seen one"), 1);
    h.row(at(5), "s", "r2", "user", &user("again"), 0);
    h.row(at(6), "s", "r2", "tool_call", &tool_call("c2", "look"), 0);
    h.row(
        at(7),
        "s",
        "r2",
        "tool_result",
        &tool_result("c2", &"B".repeat(3000)),
        0,
    );
    h.row(at(8), "s", "r2", "assistant", &said("seen two"), 1);
    let call = h.curate("s", "r3", 0, json!([user("now")]), mode("x"));
    h.tap(
        &call,
        "stop",
        with(
            json!({"tokens_prompt": soft, "cache_expires_at": "2099-01-01T00:00:00Z"}),
            package(soft, soft * 4, 10.0, 1.0),
        ),
        json!([said("ok")]),
    );
    h.fire(&last_add(&h));
    if !h.summ.is_empty() {
        h.answer("Earlier, briefly.", "stop");
    }
    (h, first)
}

#[test]
fn a_package_rebuild_aims_at_the_share_of_input_soft() {
    if !shipped() {
        return;
    }
    clear_of_midnight(120);
    let r1 = id_of(&tool_result(
        "c1",
        &format!("{}\nsecond line", "A".repeat(3000)),
    ));
    // A soft limit of 2 000 tokens: talky's 0.35 / 0.5 of it is an aim of
    // 1 400 tokens (5 600 characters), and the two tool results alone are
    // ~6 000 -- the older one has to shrink. The usable window (a cap of a
    // million) would have aimed at 350 000 and shrunk nothing.
    let (h, _) = under_package_pressure(2000);
    assert_eq!(h.plan()["shrunk"], json!([r1]));
    // A soft limit of 100 000: an aim of 70 000, room enough.
    let (h, _) = under_package_pressure(100_000);
    assert_eq!(h.plan()["shrunk"], json!([]));
}

fn id_of(el: &Value) -> String {
    short_id(el)
}

// ===================================================== 4. old packages

#[test]
fn without_the_packages_values_nothing_changes() {
    if !shipped() {
        return;
    }
    let mut h = talky(&[]);
    turn(
        &mut h,
        "s",
        "t1",
        "q",
        "a",
        json!({"tokens_prompt": 70000, "tokens_cached": 1000, "context_window": 1000000}),
    );
    // The rule of before: compress_at of the usable window.
    assert_eq!(last_add(&h).body["emit_body"]["reason"], "compress");
    let s = stamp(&h);
    assert_eq!(s["curate_mark"], "none");
    assert_eq!(s["window_tokens"], 70000);
    assert_eq!(s["cached_tokens"], 1000);
    assert_eq!(s["cost_cents"], Value::Null, "no price, no estimate");
    assert!(curate_marks(&h).is_empty(), "no package, no ledger row");
}

// ========================================== 5. a projection nobody tapped

/// KF2 review N3: the policy keeps its projection of a call until the tap
/// reads it. A call no tap answers (a lost answer) left it in `state` for
/// good; the next call of the round now takes it down, and the round's own
/// pointer to it falls with the round.
#[test]
fn a_projection_no_tap_read_falls_with_the_next_call() {
    if !shipped() {
        return;
    }
    let mut h = talky(&[]);
    h.curate("s", "t1", 0, json!([user("first")]), mode("Be brief."));
    let est = |h: &Hive| h.rows("SELECT key FROM state WHERE key LIKE 'window_est:%'");
    assert_eq!(
        est(&h).len(),
        1,
        "the first call's projection waits for its tap"
    );
    // No tap: the next call of the round renders.
    let second = h.curate("s", "t2", 0, json!([user("second")]), mode("Be brief."));
    let left = est(&h);
    assert_eq!(left.len(), 1, "one projection per round: {left:?}");
    assert_eq!(
        left[0][0].as_str().unwrap_or_default(),
        format!(
            "window_est:{}",
            second.hop["curator_call"].as_str().unwrap_or_default()
        ),
        "the projection left is the second call's"
    );
}

// ============================================ 6. the history of the cascade

fn purpose_of(template: &str) -> (String, String) {
    let t = read_json(&repo(&format!("templates/{template}/template.json")));
    (
        t["version"].as_str().unwrap_or_default().to_string(),
        t["description"]["purpose"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

fn sentence<'a>(purpose: &'a str, start: &str) -> &'a str {
    let at = purpose
        .find(start)
        .unwrap_or_else(|| panic!("no sentence starting {start:?}"));
    let rest = &purpose[at..];
    let end = rest.find(". Since ").map(|i| i + 1).unwrap_or(rest.len());
    &rest[..end]
}

/// KF2 review B1: the cascades of KT, KF1 and KF2 (and one before them)
/// moved the pins INSIDE the `Since` sentences of earlier versions to the
/// current ones -- `Since 6.6.2 ... the pin moves to curator@1.9.0`, a
/// version 6.6.2 never pinned -- and wrote no sentence of their own. A
/// `Since` sentence is history: its pins stay as written (the commit that
/// wrote it), and every bumped version gets its own.
#[test]
fn a_cascade_writes_its_own_sentence_and_keeps_the_old_ones() {
    let kept = [
        (
            "talky",
            "Since 6.6.2 (",
            "the pin moves to `curator@1.7.2`;",
        ),
        (
            "cogny",
            "Since 5.8.1 (",
            "the pin moves to `curator@1.7.2`;",
        ),
        (
            "assistant",
            "Since 3.7.2 ",
            "pins `talky@6.6.2` and `cogny@5.8.1`;",
        ),
        (
            "member",
            "Since 2.5.6 ",
            "pins `file-space@1.4.3` and `memory-hive@3.8.4` and derives \
             `./assistants` from `assistant@3.7.2`;",
        ),
        ("org", "Since 2.1.10 ", "names `member@2.5.6`;"),
        ("org", "Since 2.1.12 ", "names `member@2.5.8`;"),
        ("builder", "Since 1.26.4 ", "grows as `display@2.10.3`"),
        (
            "builder",
            "Since 1.26.9 ",
            "`builder-librarian@2.2.30`, whose corpus carries the model status \
             `explicit` of `llm-registry@2.7.0`",
        ),
        (
            "builder",
            "Since 1.26.11 ",
            "pins `display@2.10.4` in its screen recipes",
        ),
        ("builder", "Since 1.26.14 ", "`builder-librarian@2.2.35`,"),
        (
            "meclaw-os",
            "Since 2.2.11 ",
            "pins `argus@1.3.2`, `llm-registry@2.6.1` and `builder@1.26.3` and its \
             derivation names `org@2.1.10`;",
        ),
        (
            "meclaw-os",
            "Since 2.2.17 (",
            "pins `llm-registry@2.7.0` and `builder@1.26.9`;",
        ),
        (
            "meclaw-os",
            "Since 2.2.22 (",
            "pins `builder@1.26.14` and its derivation names `org@2.1.12`;",
        ),
    ];
    for (t, start, pins) in kept {
        let (_, p) = purpose_of(t);
        let s = sentence(&p, start);
        assert!(
            s.contains(pins),
            "{t}: {start:?} keeps {pins:?}, reads {s:?}"
        );
    }
    // Every version the three cascades bumped has its own sentence.
    for (t, versions) in [
        ("talky", ["6.6.3", "6.6.4", "6.6.5"]),
        ("cogny", ["5.8.2", "5.8.3", "5.8.4"]),
        ("assistant", ["3.7.3", "3.7.4", "3.7.5"]),
        ("member", ["2.5.7", "2.5.9", "2.5.10"]),
        ("org", ["2.1.11", "2.1.13", "2.1.14"]),
        ("builder", ["1.26.10", "1.26.15", "1.26.16"]),
        ("meclaw-os", ["2.2.18", "2.2.23", "2.2.24"]),
    ] {
        let (version, p) = purpose_of(t);
        for v in versions {
            assert!(
                p.contains(&format!("Since {v} (")),
                "{t}: no sentence of {v}"
            );
        }
        assert!(
            p.contains(&format!("Since {version} (")),
            "{t}: the shipped version {version} has no sentence of its own"
        );
    }
}
