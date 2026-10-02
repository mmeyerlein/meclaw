//! GH #951 -- a roundless question sees only roundless rows.
//!
//! `covers(row, q)` decides whether an object learned in round `row` may be
//! shown to a question asked in round `q`: nobody in `q` may be missing from
//! `row`. Read as a plain subset, the EMPTY round (`"[]"`, a question asked
//! with nobody named) is a subset of every round, so a roundless question
//! saw every object of every round -- in the object hive's `gate` and `tools`
//! and in the graph space's `query` alike. Measured red before the fix:
//! `covers('["agent:a","member:p"]', '[]')` was `true` in all three cells.
//! The rule now is the one the curator and the memory already keep: a
//! roundless question sees a row only if the row is roundless itself or
//! carries `*`.
//!
//! Pure: the shipped `script_inline` of each cell, loaded without its lanes.
//! Guarded like every template-reading test (GH #49).

#[path = "support/graph_space_pure.rs"]
mod pure;

use meclaw_core::serde_json::json;

const CELLS: [&str; 3] = [
    "templates/objects/gate/config.json",
    "templates/objects/tools/config.json",
    "templates/graph-space/query/config.json",
];

#[test]
fn a_roundless_question_sees_only_roundless_rows() {
    if !pure::shipped() {
        return;
    }
    let cases = json!([
        // [row, question, visible]
        ["[\"agent:a\",\"member:p\"]", "[]", false],
        ["[\"agent:a\",\"member:p\"]", "[\"member:p\"]", true],
        [
            "[\"agent:a\",\"member:p\"]",
            "[\"member:p\",\"member:q\"]",
            false
        ],
        ["[]", "[]", true],
        ["[\"*\"]", "[]", true],
        ["[]", "[\"member:p\"]", false],
        ["[\"agent:a\",\"member:p\"]", "", false],
    ]);
    for rel in CELLS {
        let got = pure::pure_at(
            rel,
            "",
            "[[c[0], c[1], covers(c[0], c[1])] for c in ARGS]",
            cases.clone(),
        );
        for (want, row) in cases
            .as_array()
            .unwrap()
            .iter()
            .zip(got.as_array().unwrap())
        {
            assert_eq!(
                row[2], want[2],
                "{rel}: covers({}, {}) -- a roundless question must not see a row of a round (GH #951)",
                want[0], want[1]
            );
        }
    }
}
