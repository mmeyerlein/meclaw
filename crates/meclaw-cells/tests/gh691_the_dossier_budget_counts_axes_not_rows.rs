//! GH #691 — the dossier budget counts AXES, not rows.
//!
//! Measured on a live hive (a first-person question about the asker's family, trace
//! `01a09af4`): the `self` leg ranked the asker's own facts by recency, and the
//! six dossier seats (`tier1_self_budget`) went to self ranks 1-6 —
//! `has_car, drives, drives, trip_plan_created, drives, favorite_color`. Three
//! of the six were the same car on the axis `(user, drives)`. The fact that
//! carried the answer, `has_sons_named`, stood on self rank 7 and was cut; no
//! query leg had found it (keyword: the German question and the English claim
//! stem apart; semantic: see `gh691_duplicate_episodes_fold_in_the_query_legs`),
//! and the temporal leg has no vote in point mode.
//!
//! The rule pinned here: a dossier seat is spent per `(subject, predicate)` axis.
//! Rows of one axis take one seat in the first pass and queue behind every other
//! axis; a `CORE_MULTI` axis (`has_child`: one row per child) ENUMERATES and is
//! never folded. Without an axis map the cut is exactly what it was. And a
//! dossier row is a fact no OTHER VOTING leg nominated — in point mode that is
//! bit-identical to before (the temporal leg does not vote there), in window
//! mode a fact the window found is seated with the query hits.
//!
//! Every probe runs the shipped `params.script_inline` (never a copy) and calls
//! `fuse_rank` directly with synthetic leg lists — the pattern of
//! `p15_recall_fusion_budget.rs`. No colony, no store, no model.

use meclaw_core::serde_json;

const RECALL_CONFIG: &str = "../../templates/memory-hive/recall/config.json";

/// Load the shipped script against a stub stdin, swallow the `park()` exit at its
/// end, then run `probe` in the same globals.
fn run_probe(probe: &str) -> String {
    let script = meclaw_testing::shipped_script(RECALL_CONFIG);
    let program = format!(
        concat!(
            "import sys, io\n",
            "_real = {}\n",
            "_sink, _out = io.StringIO(), sys.stdout\n",
            "sys.stdout = _sink\n",
            "try:\n",
            "    exec(compile(_real, 'recall', 'exec'), globals())\n",
            "except SystemExit:\n",
            "    pass\n",
            "finally:\n",
            "    sys.stdout = _out\n",
            "{}"
        ),
        serde_json::to_string(&script).unwrap(),
        probe
    );
    let out = meclaw_testing::run_shipped_script(
        &program,
        r#"{"envelope": {}, "body": {}, "params": {}}"#,
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The competition every case below runs against: twelve facts and twelve
/// episodes the QUESTION found. That is enough to fill the bundle without the
/// dossier, so the budget bites and nothing is backfilled — the measured turn,
/// where `used == TOPK` and no slot fell back to the dossier.
const COMPETITION: &str = r#"
mkf = lambda i: {"kind": "fact", "id": i}
mke = lambda i: {"kind": "episode", "id": i}
KW = [mkf("f-kw-%02d" % i) for i in range(12)] + [mke("e-%02d" % i) for i in range(12)]
POINT = fusion_weights({})
def seated(ranked, prefix):
    return [k[1] for k in ranked if k[1].startswith(prefix)]
"#;

/// The measured self ranks 1-7, plus one more single axis behind them.
const MEASURED: &str = r#"
SELF = ["s-car", "s-drives-1", "s-drives-2", "s-trip", "s-drives-3", "s-color",
        "s-sons", "s-project"]
AXIS = {"s-car": ["user", "has_car"], "s-drives-1": ["user", "drives"],
        "s-drives-2": ["user", "drives"], "s-trip": ["user", "trip_plan_created"],
        "s-drives-3": ["user", "drives"], "s-color": ["user", "favorite_color"],
        "s-sons": ["user", "has_sons_named"], "s-project": ["user", "project_context"]}
"#;

#[test]
fn three_copies_of_one_axis_take_one_dossier_seat() {
    let probe = format!(
        "{COMPETITION}{MEASURED}{}",
        r#"
ranked, _, _, _ = fuse_rank({"keyword": KW, "self": [mkf(i) for i in SELF]}, POINT, AXIS)
mine = seated(ranked, "s-")
print("s-sons" in mine, len([i for i in mine if i.startswith("s-drives")]), len(mine))
"#
    );
    assert_eq!(
        run_probe(&probe),
        "True 1 6",
        "the carrying fact on self rank 7 is seated, the three rows of one axis \
         take one seat, and the dossier still gets exactly its budget"
    );
}

#[test]
fn a_multi_axis_keeps_every_row_as_its_own_answer() {
    // `has_child` is CORE_MULTI: two children are two answers, not two copies.
    // Folding the axis would push the second child behind every other axis and
    // out of the budget — the row on self rank 6 is where that shows.
    let probe = format!(
        "{COMPETITION}{}",
        r#"
SELF = ["s-car", "s-drives", "s-child-elias", "s-trip", "s-color", "s-child-leroy",
        "s-project"]
AXIS = {"s-car": ["user", "has_car"], "s-drives": ["user", "drives"],
        "s-child-elias": ["user", "has_child"], "s-trip": ["user", "trip_plan_created"],
        "s-color": ["user", "favorite_color"], "s-child-leroy": ["user", "has_child"],
        "s-project": ["user", "project_context"]}
ranked, _, _, _ = fuse_rank({"keyword": KW, "self": [mkf(i) for i in SELF]}, POINT, AXIS)
mine = seated(ranked, "s-")
print("s-child-elias" in mine, "s-child-leroy" in mine, len(mine))
"#
    );
    assert_eq!(run_probe(&probe), "True True 6");
}

#[test]
fn without_an_axis_map_the_dossier_is_cut_as_before() {
    // No axis map is the shape every caller had before GH #691: the budget is
    // spent in rank order, row by row, and the measured cut happens exactly as
    // it was measured.
    let probe = format!(
        "{COMPETITION}{MEASURED}{}",
        r#"
ranked, _, _, _ = fuse_rank({"keyword": KW, "self": [mkf(i) for i in SELF]}, POINT)
mine = seated(ranked, "s-")
print("s-sons" in mine, len([i for i in mine if i.startswith("s-drives")]), mine)
"#
    );
    assert_eq!(
        run_probe(&probe),
        "False 3 ['s-car', 's-drives-1', 's-drives-2', 's-trip', 's-drives-3', 's-color']"
    );
}

/// Ten self rows on ten different axes; `s-09` (self rank 10) is ALSO a hit of
/// the temporal leg.
const TEN_AXES: &str = r#"
SELF = ["s-%02d" % i for i in range(10)]
AXIS = {i: ["user", "p%s" % i[-2:]] for i in SELF}
TEMPORAL = [mkf("s-09")]
"#;

#[test]
fn in_window_mode_a_fact_the_window_found_is_not_a_dossier_row() {
    // In window mode the temporal leg VOTES: the window's own ordering is the
    // answer (ruling O-2). A fact it found is a query hit, seated by the query's
    // competition — it must not spend one of the six dossier seats, which then
    // go to six self-only rows.
    let probe = format!(
        "{COMPETITION}{TEN_AXES}{}",
        r#"
ranked, _, hit_legs, _ = fuse_rank({"keyword": KW, "self": [mkf(i) for i in SELF],
                                    "temporal": TEMPORAL}, W, AXIS)
mine = seated(ranked, "s-")
print("s-09" in mine, len([i for i in mine if i != "s-09"]))
"#
    );
    assert_eq!(run_probe(&probe), "True 6");
}

#[test]
fn in_point_mode_the_silent_temporal_leg_changes_nothing() {
    // The #536 DOSSIER FLOOD guard: in point mode the temporal leg is recency and
    // carries no vote, and it overlaps the self leg on almost every row of a
    // member hive. Reading it as "found by another leg" would turn every
    // dossier row into an ordinary fact and the flood would be back. The rule is
    // "no other VOTING leg", so with the temporal list present or absent the
    // bundle is the same.
    let probe = format!(
        "{COMPETITION}{TEN_AXES}{}",
        r#"
with_t, _, _, _ = fuse_rank({"keyword": KW, "self": [mkf(i) for i in SELF],
                             "temporal": TEMPORAL}, POINT, AXIS)
without_t, _, _, _ = fuse_rank({"keyword": KW, "self": [mkf(i) for i in SELF]}, POINT, AXIS)
print("s-09" in seated(with_t, "s-"), with_t == without_t)
"#
    );
    assert_eq!(run_probe(&probe), "False True");
}
