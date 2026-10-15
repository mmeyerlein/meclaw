//! GH #435 — the broker ships the one row a submission needs, switched off.
//!
//! `templates/access/README.md` promises: *a fresh instance grants nothing:
//! every seeded policy row ships `enabled: 0`*. That promise was never
//! measured, and a promise nobody measures is a promise that drifts. The first
//! test here is that sentence, turned into an assertion over the file that
//! actually ships.
//!
//! The second is the row this lane adds. `submit` asks the broker whether a
//! manifest may go to the mutation door, and until now no shipped rule
//! mentioned `colony.mutate` at all — so the broker answered
//! `capability_unknown` and a broker-checked `submit` was unusable out of the
//! box. The row closes that, and it closes it **disabled**: what ships is the
//! shape of the answer, never the answer itself.
//!
//! The delegation is the interesting half. R-AC-1 says the requester comes off
//! the EDGE and never out of a body — so the requester of a submission is the
//! `submit` hive, not the person who wrote it. The person rides `subject`, the
//! axis the broker already has for exactly this case. The rule therefore reads:
//! *`/os/operator/submit` may `colony.mutate` ON BEHALF OF anyone, under
//! `/os/orgs`* — and the delegation stands visibly in a row instead of
//! implicitly in a script.
//!
//! Since GH #556 the `submit` hive is an occupant of the `operator` hive rather
//! than of the OS shell, so the path the shell's `./operator -> ./access` edge
//! promotes — and therefore the path every shipped row names as its requester —
//! is `/os/operator/submit`. The delegation itself is unchanged: what moved is
//! where the submitter STANDS.
//!
//! **R2b guard (GH #49 form).** `access` is PRIVATE — it does not travel with
//! the export, so in the public clone these tests skip.

use meclaw_core::serde_json::Value;

const SEED: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/access/store/seed/policy.jsonl"
);

const STORE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/access/store/config.json"
);

/// The data rows of the shipped seed, or `None` where the template does not
/// ship. Line 1 is the `{"schema": {…}}` header the store cross-checks the
/// declared columns against — it is not a row and carries no `enabled`.
fn seeded_rows() -> Option<Vec<Value>> {
    let text = std::fs::read_to_string(SEED).ok()?;
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header: Value =
        meclaw_core::serde_json::from_str(lines.next().expect("a seed file is never empty"))
            .expect("line 1 is json");
    assert!(
        header.get("schema").is_some_and(Value::is_object),
        "line 1 of a seed file is the schema header, not a data row"
    );
    Some(
        lines
            .map(|l| meclaw_core::serde_json::from_str(l).expect("a data line is json"))
            .collect(),
    )
}

/// The two rules a colony cannot start without, and nothing else.
///
/// Until 2026-08-28 this file asserted that EVERY seeded row ships disabled, and
/// the promise behind it was "a fresh instance grants nothing". Ruling
/// R-Policy-Default retires that promise for exactly two rows and keeps it for
/// every other: a freshly instantiated OS has to be able to build, and its
/// brains have to be able to register for their own identity, or the first
/// mutation of every colony is an operator step and a shipped agent is silent
/// until somebody remembers an `UPDATE policy`.
///
/// What replaces the old promise is narrower and therefore worth more: the
/// on-set is a NAMED LIST, not a count. A third rule shipping enabled is a
/// colony authorising something nobody asked it to, and it goes red here.
const ENABLED_BY_DEFAULT: [&str; 2] = ["colony.mutate.default", "affinity.subscribe.default"];

#[test]
fn only_the_two_rules_a_colony_cannot_start_without_ship_enabled() {
    let Some(rows) = seeded_rows() else {
        return;
    };
    assert!(
        !rows.is_empty(),
        "the README's promise is about the rows that ship — with none, it is vacuous"
    );
    let mut on: Vec<String> = Vec::new();
    for row in &rows {
        let id = row["rule_id"].as_str().unwrap_or_default().to_string();
        match row["enabled"].as_i64() {
            Some(0) => {}
            Some(1) => on.push(id),
            other => panic!(
                "rule {} declares enabled={other:?} — the column is 0 or 1",
                row["rule_id"]
            ),
        }
    }
    on.sort();
    let mut want: Vec<String> = ENABLED_BY_DEFAULT.iter().map(|s| s.to_string()).collect();
    want.sort();
    assert_eq!(
        on, want,
        "the seed grants something other than the two defaults a colony cannot start \
         without (R-Policy-Default, 2026-08-28)"
    );
}

/// The line the default set does not cross.
///
/// `colony.mutate` lets a manifest reach the door; `code.author` is the separate
/// question of whether that manifest may bring executable behaviour with it, and
/// it stays off. A fresh colony may build, and it may still not author code
/// nobody reviewed — asserted rather than implied, because "the two defaults are
/// narrow" is only true while this row is on the other side of the line.
#[test]
fn code_author_is_not_granted_by_default() {
    let Some(rows) = seeded_rows() else {
        return;
    };
    let row = rows
        .iter()
        .find(|r| r["rule_id"] == meclaw_core::serde_json::json!("code.author.default"))
        .expect("the seed ships a code.author row");
    assert_eq!(
        row["enabled"],
        meclaw_core::serde_json::json!(0),
        "code.author.default ships switched ON — the default set stops at building and \
         subscribing, and authoring code is the thing on the other side of that line"
    );
}

#[test]
fn every_seeded_row_fills_every_declared_column() {
    let Some(rows) = seeded_rows() else {
        return;
    };
    let store: Value =
        meclaw_core::serde_json::from_str(&std::fs::read_to_string(STORE).expect("store config"))
            .expect("json");
    let cols = store["params"]["schema"]["policy"]
        .as_object()
        .expect("the policy table is declared");
    for row in &rows {
        for col in cols.keys() {
            assert!(
                row.get(col).is_some(),
                "rule {} names no `{col}` — the loader binds every declared column, so a \
                 missing key is a silent NULL rather than an error",
                row["rule_id"]
            );
        }
    }
}

#[test]
fn the_submission_rule_delegates_by_subject() {
    let Some(rows) = seeded_rows() else {
        return;
    };
    let row = rows
        .iter()
        .find(|r| r["rule_id"] == meclaw_core::serde_json::json!("colony.mutate.default"))
        .expect("the seed carries the submission rule");
    assert_eq!(
        row["capability"],
        meclaw_core::serde_json::json!("colony.mutate")
    );
    // R-AC-1 intact: the requester is the submit hive, promoted by the edge.
    // The identity on whose behalf it asks travels as `subject`. Since GH #556
    // that hive stands inside the front door, so the path the shell's
    // `./operator -> ./access` edge promotes is `/os/operator/submit`.
    assert_eq!(
        row["requester"],
        meclaw_core::serde_json::json!("/os/operator/submit")
    );
    assert_eq!(row["subject"], meclaw_core::serde_json::json!("*"));
    assert_eq!(
        row["scope_match"]["scope_prefix"],
        meclaw_core::serde_json::json!("/os/orgs")
    );
    assert_eq!(row["verdict"], meclaw_core::serde_json::json!("allow"));
    assert_eq!(
        row["max_ttl_ms"],
        meclaw_core::serde_json::json!(0),
        "a checked verdict has no instrument, so it has no lifetime either"
    );
    assert_eq!(row["cred_ref"], meclaw_core::serde_json::json!(""));
}
