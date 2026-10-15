//! GH #1040 — the recall bundle and its query are sized by the asker's model
//! package, and the member's self-dossier is ranked by the question.
//!
//! Measured in a benchmark run of the curator (KD2 § 2/§ 3): the tier-1 bundle
//! was 2000 tokens -- about 2 % of the window the answering model could use --
//! and six facts with the legacy subject `user` stood in all 120 bundles of the
//! run, a third of every bundle, while facts that answered the question were in
//! the store and not in the bundle.
//!
//! The locks:
//!   * a large package grows the bundle to its share of `input_soft`, and the
//!     request's own fan asks the store that deep;
//!   * without a package (or with one too small to matter) every limit keeps its
//!     old value, and a knob set to a number wins over the package;
//!   * the self-dossier never displaces a better-ranked element, a dossier row
//!     the question matches competes in the fused order, and the rest still
//!     backfills a bundle nothing else filled;
//!   * the curator hands the package over on its ask, and the question it builds
//!     never grows past what the memory hive's query guard keeps.
//!
//! Every probe runs the shipped `params.script_inline` (never a copy). No
//! colony, no store, no model.

use meclaw_core::serde_json::{self, Value, json};

const RECALL_CONFIG: &str = "../../templates/memory-hive/recall/config.json";
const PUSH_CONFIG: &str = "../../templates/curator/push/config.json";
const MEMBER_CONFIG: &str = "../../templates/member/config.json";
const HIVE_CONFIG: &str = "../../templates/memory-hive/config.json";

/// Load a shipped script against a stub stdin carrying `params`, swallow the
/// `park()` exit at its end, then run `probe` in the same globals.
fn probe(config: &str, params: Value, probe: &str) -> String {
    let script = meclaw_testing::shipped_script(config);
    let program = format!(
        concat!(
            "import sys, io\n",
            "_real = {}\n",
            "_sink, _out = io.StringIO(), sys.stdout\n",
            "sys.stdout = _sink\n",
            "try:\n",
            "    exec(compile(_real, 'cell', 'exec'), globals())\n",
            "except SystemExit:\n",
            "    pass\n",
            "finally:\n",
            "    sys.stdout = _out\n",
            "{}"
        ),
        serde_json::to_string(&script).unwrap(),
        probe
    );
    let stdin = json!({"envelope": {}, "body": {}, "params": params}).to_string();
    let out = meclaw_testing::run_shipped_script(&program, &stdin);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The shipped recall cell on one real request, as the member's door delivers
/// it; returns the limit of every fan call by id.
fn fan_limits(input_soft: &str, params: Value) -> Value {
    let doc = json!({
        "header": {"context": {"recall_id": "", "mem_phase": "", "memory_tier": "1",
                               "recall_query": "What are my sons called?",
                               "recall_input_soft": input_soft,
                               "audience_now": "[\"member:alex\"]", "channel": "tg:private"},
                   "hop": {"route": "in_query", "phase": "recall"}},
        "messages": [],
        "params": params
    });
    let script = meclaw_testing::shipped_script(RECALL_CONFIG);
    let stdin = meclaw_testing::code_stdin(&doc).to_string();
    let out = meclaw_testing::run_shipped_script(&script, &stdin);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("stdout is json");
    let msgs = match v {
        Value::Array(a) => a,
        other => vec![other],
    };
    let fan = msgs
        .iter()
        .find(|m| m["header"]["phase"] == "t1-fan")
        .unwrap_or_else(|| panic!("no t1-fan bundle in {msgs:?}"));
    let mut limits = serde_json::Map::new();
    for t in fan["messages"].as_array().expect("messages") {
        if t["type"] != "tool_call" {
            continue;
        }
        let args: Value = serde_json::from_str(t["text"].as_str().expect("text")).expect("args");
        limits.insert(
            t["id"].as_str().expect("id").to_string(),
            args["limit"].clone(),
        );
    }
    Value::Object(limits)
}

// ═══════════════════════════════════════════════ 1. the package sizes the bundle

#[test]
fn a_large_package_grows_the_bundle_to_its_share() {
    // luna: input_soft 250k; 5 % of it is 12.5k tokens, 6.25 times the old 2000.
    assert_eq!(
        probe(
            RECALL_CONFIG,
            json!({}),
            "L = package_limits(250000)\n\
             print(L['tier1_tokens'], L['tier1_topk'], L['tier1_leg_limit'], \
             L['bundle_episode_budget'], L['query_safe_chars'], \
             L['query_max_chars'], L['query_tokens'], L['tier1_self_limit'])"
        ),
        "12500 125 125 38 400 500 48 125"
    );
}

#[test]
fn the_request_asks_the_store_as_deep_as_its_package_allows() {
    let big = fan_limits("250000", json!({}));
    assert_eq!(big["r-fan-kw-ep"], 125, "{big}");
    assert_eq!(big["r-fan-kw-fact"], 125, "{big}");
    // Review M3: the store hands the dossier newest first and only `limit`
    // rows of it; `self_by_query` can rank only what came back. A row the
    // question matches on recency place 21 reaches the self leg only when the
    // page grows with the package like every other leg.
    assert_eq!(big["r-fan-self"], 125, "{big}");
    let none = fan_limits("", json!({}));
    assert_eq!(none["r-fan-kw-ep"], 20, "{none}");
    assert_eq!(none["r-fan-kw-fact"], 20, "{none}");
    assert_eq!(none["r-fan-self"], 20, "{none}");
    let fixed = fan_limits("250000", json!({"tier1_self_limit": 20}));
    assert_eq!(fixed["r-fan-self"], 20, "a number wins: {fixed}");
}

#[test]
fn without_a_package_every_limit_keeps_its_old_value() {
    let old = "2000 20 20 6 200 250 24 20";
    for soft in ["None", "''", "'x'", "0", "-5", "30000"] {
        assert_eq!(
            probe(
                RECALL_CONFIG,
                json!({}),
                &format!(
                    "L = package_limits({soft})\n\
                     print(L['tier1_tokens'], L['tier1_topk'], L['tier1_leg_limit'], \
                     L['bundle_episode_budget'], \
                     L['query_safe_chars'], L['query_max_chars'], L['query_tokens'], \
                     L['tier1_self_limit'])"
                )
            ),
            old,
            "input_soft {soft}: an ask without a (large enough) package is sized as before"
        );
    }
    // And the module itself, on a request that names no package and with no
    // fallback window (`input_soft_fallback` 0).
    let module = "print(T1_TOKENS, TOPK, LEG_LIMIT, EPISODE_BUDGET, SEM_FETCH, \
                  QUERY_SAFE_CHARS, QUERY_MAX_CHARS, QUERY_TOKENS, SELF_LIMIT, TOKEN_BUDGET)";
    assert_eq!(
        probe(RECALL_CONFIG, json!({"input_soft_fallback": 0}), module),
        "2000 20 20 6 40 200 250 24 20 1200"
    );
    // GH #1085 (R-IG-1): with a fallback window of 237500 tokens the two
    // BUNDLES take their share of it (5 %: 11875, tier 0 by the same factor:
    // 7125) instead of the fixed 2000 and 1200; what selects (the counts, the
    // query guard the curator mirrors) keeps its old value.
    assert_eq!(
        probe(
            RECALL_CONFIG,
            json!({"input_soft_fallback": 237500}),
            module
        ),
        "11875 20 20 6 40 200 250 24 20 7125"
    );
    // OR-IG-9: the shipped fallback is the smallest chat row (an ask without a
    // window has no born model the memory knows); its 5 % is under the old
    // floor, so the shipped module keeps the old sizes.
    assert_eq!(
        probe(RECALL_CONFIG, json!({}), module),
        "2000 20 20 6 40 200 250 24 20 1200"
    );
}

#[test]
fn a_knob_set_to_a_number_wins_over_the_package() {
    assert_eq!(
        probe(
            RECALL_CONFIG,
            json!({"tier1_tokens": 3000, "tier1_topk": "40"}),
            "L = package_limits(250000)\nprint(L['tier1_tokens'], L['tier1_topk'], L['tier1_leg_limit'], \
             L['query_safe_chars'])"
        ),
        // Review N2: a bundle ceiling also bounds what is counted into it --
        // the count limits grow by the ceiling's factor (3000 / 2000),
        // not by the package's. The query guard keeps the package's growth:
        // the curator's question is sized by the package alone (mirror lock
        // below), and a guard below it would only cut the person's words.
        "3000 40 30 400"
    );
    let fixed = fan_limits("250000", json!({"tier1_leg_limit": 20}));
    assert_eq!(fixed["r-fan-kw-ep"], 20, "{fixed}");
}

// ═════════════════════════════════════ 2. the dossier is ranked by the question

/// Twenty facts a query leg found, six dossier rows the question does not
/// match. The bundle is full of better-ranked elements: no dossier row sits in
/// it. Before #1040 six of the twenty seats were reserved for the dossier.
#[test]
fn the_dossier_never_displaces_a_better_ranked_element() {
    let out = probe(
        RECALL_CONFIG,
        json!({}),
        r#"
mkf = lambda i, q=None: dict({"kind": "fact", "id": i}, **({} if q is None else {"q": q}))
KW = [mkf("f-kw-%02d" % i) for i in range(20)]
SELF = [mkf("s-%d" % i, 0) for i in range(6)]
ranked, _, _, _ = fuse_rank({"keyword": KW, "self": SELF}, fusion_weights({}), {})
print(len(ranked), len([k for k in ranked if k[1].startswith("s-")]))
"#,
    );
    assert_eq!(out, "20 0");
}

#[test]
fn a_dossier_row_the_question_matches_competes_in_the_fused_order() {
    // The self leg hands the matching row first (`self_by_query`); it fuses at
    // self rank 1 and is seated like any query hit -- ahead of the keyword tail,
    // never ahead of the keyword hit with the same rank and an earlier leg.
    let out = probe(
        RECALL_CONFIG,
        json!({}),
        r#"
rows = [{"id": "s-coffee", "claim": "buy coffee filters"},
        {"id": "s-cake", "claim": "call the bakery about the cake"},
        {"id": "s-son", "claim": "Henrik Falkner is the member's son", "predicate": "has_child"}]
order = self_by_query(rows, "Who is my son?")
print([r["id"] for r, q in order], [q for r, q in order])
mkf = lambda i, q=None: dict({"kind": "fact", "id": i}, **({} if q is None else {"q": q}))
KW = [mkf("f-kw-%02d" % i) for i in range(20)]
SELF = [mkf(r["id"], q) for r, q in order]
ranked, _, _, _ = fuse_rank({"keyword": KW, "self": SELF}, fusion_weights({}), {})
ids = [k[1] for k in ranked]
print(len(ids), ids.index("s-son"), "s-coffee" in ids, "f-kw-19" in ids)
"#,
    );
    assert_eq!(
        out, "['s-son', 's-coffee', 's-cake'] [1, 0, 0]\n20 1 False False",
        "the son row is ranked by the question and seated second; the errands are not seated"
    );
}

#[test]
fn a_bundle_nothing_else_filled_is_still_answered_by_the_dossier() {
    let out = probe(
        RECALL_CONFIG,
        json!({}),
        r#"
mkf = lambda i, q=None: dict({"kind": "fact", "id": i}, **({} if q is None else {"q": q}))
KW = [mkf("f-kw-%02d" % i) for i in range(10)]
SELF = [mkf("s-%d" % i, 0) for i in range(6)]
ranked, scores, legs, _ = fuse_rank({"keyword": KW, "self": SELF}, fusion_weights({}), {})
ids = [k[1] for k in ranked]
print(len(ids), ids[-6:] == ["s-%d" % i for i in range(6)], legs[("fact", "s-0")])
"#,
    );
    assert_eq!(out, "16 True ['self']");
}

#[test]
fn the_reserved_floor_is_an_override_and_the_legacy_spelling_is_read() {
    let out = probe(
        RECALL_CONFIG,
        json!({"tier1_self_budget": 6}),
        r#"
mkf = lambda i, q=None: dict({"kind": "fact", "id": i}, **({} if q is None else {"q": q}))
KW = [mkf("f-kw-%02d" % i) for i in range(20)]
SELF = [mkf("s-%d" % i) for i in range(6)]
ranked, _, _, _ = fuse_rank({"keyword": KW, "self": SELF}, fusion_weights({}), {})
print(len([k for k in ranked if k[1].startswith("s-")]))
"#,
    );
    assert_eq!(out, "6", "a number above 0 is the old reserved floor");
    assert_eq!(
        probe(
            RECALL_CONFIG,
            json!({}),
            "print('[%s]' % SELF_LEGACY_SUBJECT, SELF_BUDGET, 'user' in self_names())"
        ),
        "[user] 0 True",
        "review B1: rows written before the person-name subject stay the asker's"
    );
}

/// The KD2 case with the legacy spelling read (review B1): six `user` errands
/// the question does not touch and twenty keyword hits. With the shipped
/// params no errand takes a seat -- what displaced the answer in KD2 § 2 were
/// the six reserved seats and the recency order, not the spelling.
#[test]
fn legacy_user_rows_the_question_misses_take_no_seat() {
    let out = probe(
        RECALL_CONFIG,
        json!({}),
        r#"
rows = [{"id": "s-%d" % i, "canonical_subject": "user", "claim": c, "predicate": "todo"}
        for i, c in enumerate(["buy coffee filters", "call the bakery", "water the plants",
                               "renew the parking permit", "return the library book",
                               "book the dentist appointment"])]
order = self_by_query(rows, "When did I visit Lisbon?")
mkf = lambda i, q=None: dict({"kind": "fact", "id": i}, **({} if q is None else {"q": q}))
KW = [mkf("f-kw-%02d" % i) for i in range(20)]
SELF = [mkf(r["id"], q) for r, q in order]
ranked, _, _, _ = fuse_rank({"keyword": KW, "self": SELF}, fusion_weights({}), {})
print(sum(q for _, q in order), len(ranked), len([k for k in ranked if k[1].startswith("s-")]))
"#,
    );
    assert_eq!(out, "0 20 0");
}

/// Review N5: the curator puts `[topic: ...; mentioned: ...]` in front of the
/// person's words. Those words were picked from earlier rounds; a dossier row
/// that shares one of them is not what the person asked about.
#[test]
fn the_curators_prefix_does_not_rank_the_dossier() {
    let out = probe(
        RECALL_CONFIG,
        json!({}),
        r#"
rows = [{"id": "s-garden", "claim": "plans the garden party with Henrik"},
        {"id": "s-son", "claim": "Henrik Falkner is the member's son", "predicate": "has_child"}]
order = self_by_query(rows, "[topic: garden party; mentioned: Henrik] Who is my son?")
print([(r["id"], q) for r, q in order])
"#,
    );
    assert_eq!(out, "[('s-son', 1), ('s-garden', 0)]");
}

/// Review N1: the curator sizes its question by a mirror of the hive's
/// `package_limits` (two hives, no shared code). The three numbers stay in step.
#[test]
fn the_curators_mirror_of_the_package_rule_is_the_hives() {
    let push = probe(
        PUSH_CONFIG,
        json!({}),
        "print(QUERY_SHARE, QUERY_BASE_TOKENS, QUERY_GROWTH_MAX)",
    );
    let hive = probe(
        RECALL_CONFIG,
        json!({}),
        "print(BUNDLE_SHARE, _PKG_FALLBACK['tier1_tokens'], QUERY_GROWTH_MAX)",
    );
    assert_eq!(push, hive);
}

// ═══════════════════════════════════════════ 3. the curator hands the package on

#[test]
fn the_curators_question_never_outgrows_the_memory_guard() {
    let soft = [0, 1000, 40_000, 80_000, 100_000, 250_000, 1_000_000];
    let push = probe(
        PUSH_CONFIG,
        json!({}),
        &format!(
            "print(' '.join(str(query_budget(200, s)) for s in {soft:?}), \
             query_budget(200, ''), query_budget(200, 250000, True), \
             query_budget(0, 250000))"
        ),
    );
    let guard = probe(
        RECALL_CONFIG,
        json!({}),
        &format!("print(' '.join(str(package_limits(s)['query_safe_chars']) for s in {soft:?}))"),
    );
    let (budgets, rest) = push.split_at(push.match_indices(' ').nth(soft.len() - 1).unwrap().0);
    assert_eq!(budgets, guard, "the push grows exactly as the guard grows");
    assert_eq!(
        rest.trim(),
        "200 200 0",
        "no package / a fixed budget / push off: as set"
    );
    assert_eq!(guard, "200 200 200 400 400 400 400");
}

/// The key reaches the recall cell (the member promotes it from the hop) and
/// never leaves the memory hive (every outbound edge that scrubs the recall
/// keys scrubs it too, GH #823).
#[test]
fn the_package_rides_into_the_hive_and_never_out() {
    let member: Value =
        serde_json::from_str(&std::fs::read_to_string(MEMBER_CONFIG).unwrap()).unwrap();
    let mut promoted = 0;
    for e in member["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
    {
        let sc = &e["modifier"]["set_context"];
        // The objects hive's own memory question carries no model (gh951).
        if sc.get("recall_window_to").is_some() && sc.get("objects_subject").is_none() {
            assert!(
                sc.get("recall_input_soft").is_some(),
                "an edge that promotes the recall keys promotes the package too: {e}"
            );
            promoted += 1;
        }
    }
    assert_eq!(promoted, 2);
    let hive: Value = serde_json::from_str(&std::fs::read_to_string(HIVE_CONFIG).unwrap()).unwrap();
    let mut scrubbed = 0;
    for e in hive["params"]["graph"]["edges"].as_array().expect("edges") {
        if let Some(dc) = e["modifier"]["delete_context"].as_array()
            && dc.iter().any(|k| k == "recall_query")
        {
            assert!(
                dc.iter().any(|k| k == "recall_input_soft"),
                "an edge that scrubs the recall keys scrubs the package too: {e}"
            );
            scrubbed += 1;
        }
    }
    assert!(scrubbed >= 18, "{scrubbed}");
}
