//! GH #514 — the shell is a scope, and until `access@2.4.2` no shipped row
//! reached it.
//!
//! WHAT WAS MEASURED
//! =================
//! Every row `templates/access/store/seed/policy.jsonl` shipped carried
//! `scope_match.scope_prefix: "/os/orgs"`. On a colony grown from
//! `examples/meclaw-os/seed-ref`, the SAME declaration submitted twice through
//! the front door and differing only in its scope root answered:
//!
//! ```text
//! scope "/os"       -> submit/gate: requester_not_permitted, corpus unchanged
//! scope "/os/orgs"  -> allowed -> door committed -> corpus 613 -> 614
//! ```
//!
//! Registering a template class writes to `/colony/templates`, which belongs to
//! no organisation, so `/os` is the scope root a composer writes for it — and
//! `/os/orgs` only happened to work because the prefix matched. The shipped
//! policy and the shipped topology disagreed about where authoring happens, and
//! the disagreement was invisible until somebody submitted at `/os` and read a
//! refusal that names the REQUESTER rather than the scope.
//!
//! THE RULING, AND WHAT THIS FILE PINS
//! ===================================
//! Marcus, 2026-08-29, option (b): a SECOND shipped row for the shell,
//! delivered `enabled: 0`, switched on with a `seed_rows` manifest. Not a
//! widened prefix on the existing row — a colony that grants the shell has
//! granted every shell-level topology change with it, and that is an operator's
//! decision rather than a seed's.
//!
//! * the row ships, with the shape and the priority the README states;
//! * PRECEDENCE: `/os` is a PATH prefix and therefore the SUPERSET of
//!   `/os/orgs`, so the pair is kept apart by `priority` DESC and first-match —
//!   `colony.mutate.default` at 100 still answers for an organisation, the
//!   shell row at 90 only ever answers for a scope the narrow one cannot reach;
//! * the manifest in the README goes through the REAL `seed_rows` door onto a
//!   REAL `cell.db`, and what the broker reads afterwards is one enabled rule;
//! * over the shipped `policy` script, `/os` is `scope_mismatch` as shipped and
//!   `allowed` once the row is on — and `/oscar` is refused either way;
//! * over the shipped `submit/gate`, a `/os` declaration asks over `/os`, a
//!   denied verdict becomes `requester_not_permitted` with nothing on `mutate`,
//!   and an allowed one reaches the door;
//! * and the second question a template registration asks is still asked:
//!   `code.author.default` is scoped `/os/orgs` and ships off, so switching
//!   only the row above on moves a shell-scoped `add_templates` from
//!   `requester_not_permitted` to `code_author_denied`. A different refusal,
//!   not a green light.
//!
//! A drift lock (development rules § 2d): the README paragraph that promises
//! all of this is grepped here AND the manifest inside it is executed, so the
//! prose cannot outlive its mechanism.
//!
//! WHAT IT IS NOT
//! ==============
//! It boots no colony. `gh469` and `gh474` pin which address a draft reaches
//! and both say in as many words that whether the BROKER then permits the
//! submission is the policy's decision and a different file. This is that file:
//! the verdict, measured over the shipped rows, the shipped door and the
//! shipped scripts.
//!
//! **R2b guard (GH #49 form).** `access` is PRIVATE — it does not travel with
//! the export, so in the public clone these tests skip.

use meclaw_core::serde_json::{Map, Value, json};
use meclaw_testing::code_wire::{emit_all, run_shipped_script, shipped_script};
use std::collections::BTreeMap;

const SEED: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/access/store/seed/policy.jsonl"
);
const README: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/access/README.md"
);
const STORE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/access/store/config.json"
);
const POLICY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/access/policy/config.json"
);
const GATE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/submit/gate/config.json"
);

/// The row this issue adds, and the one it stands beside.
const SHELL: &str = "colony.mutate.shell";
const DEFAULT: &str = "colony.mutate.default";

/// What the shell's own edge promotes onto every question the submitter puts —
/// a literal, and the same one whichever door the submission came in at.
const REQUESTER: &str = "/os/submit";
/// What the front door's own emission is stamped with, and therefore what the
/// gate hands the broker as `subject`.
const FRONT: &str = "/os/operator/submit";

// ────────────────────────────────────────────────────────────── the shipped seed

/// The data rows of the shipped seed, or `None` where the template does not
/// ship. Line 1 is the `{"schema": {…}}` header, not a row.
fn seeded_rows() -> Option<Vec<Value>> {
    let text = std::fs::read_to_string(SEED).ok()?;
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header: Value = meclaw_core::serde_json::from_str(lines.next()?).expect("line 1 is json");
    assert!(
        header.get("schema").is_some_and(Value::is_object),
        "line 1 of a seed file is the schema header"
    );
    Some(
        lines
            .map(|l| meclaw_core::serde_json::from_str(l).expect("a data line is json"))
            .collect(),
    )
}

fn row_named(rows: &[Value], rule_id: &str) -> Value {
    rows.iter()
        .find(|r| r["rule_id"] == json!(rule_id))
        .unwrap_or_else(|| panic!("the seed ships no {rule_id} row"))
        .clone()
}

#[test]
fn the_shell_row_ships_and_ships_switched_off() {
    let Some(rows) = seeded_rows() else {
        return;
    };
    let shell = row_named(&rows, SHELL);
    let default = row_named(&rows, DEFAULT);

    assert_eq!(shell["enabled"], json!(0), "the shell row must ship OFF");
    assert_eq!(shell["capability"], json!("colony.mutate"));
    assert_eq!(
        shell["requester"],
        json!(REQUESTER),
        "R-AC-1: the requester of a submission is the submit hive, not the person"
    );
    assert_eq!(shell["subject"], json!("*"));
    assert_eq!(shell["verdict"], json!("allow"));
    assert_eq!(shell["scope_match"]["scope_prefix"], json!("/os"));
    assert_eq!(shell["scope_match"]["actions"], json!(["apply"]));

    // Precedence is a NUMBER here, and the number is the whole mechanism: `/os`
    // permits everything `/os/orgs` permits, so without a lower priority the
    // shell row would be the one that answers for an organisation too, and the
    // narrow row would become decoration.
    let (shell_prio, default_prio) = (
        shell["priority"].as_i64().expect("a priority"),
        default["priority"].as_i64().expect("a priority"),
    );
    assert!(
        shell_prio < default_prio,
        "the shell row ({shell_prio}) must be examined AFTER {DEFAULT} ({default_prio}) \
         — rules are read in priority DESC and the FIRST match wins, and `/os` is the \
         superset of `/os/orgs`"
    );
}

// ───────────────────────────────────────────────────── the README, and its manifest

fn readme() -> String {
    std::fs::read_to_string(README).expect("the access README ships")
}

/// The first fenced `json` block under a named heading.
fn json_block_under(text: &str, heading: &str) -> Value {
    let start = text
        .find(heading)
        .unwrap_or_else(|| panic!("the README carries no heading {heading:?}"));
    let rest = &text[start..];
    let open = rest
        .find("```json")
        .expect("a json block under the heading");
    let after = &rest[open + "```json".len()..];
    let close = after.find("```").expect("the block is closed");
    meclaw_core::serde_json::from_str(after[..close].trim()).expect("the block is json")
}

/// The section this issue's promise lives in. Both halves of the drift lock
/// (§ 2d) hang off this name: the sentence is grepped and the manifest under it
/// is executed.
const SECTION: &str = "#### Enabling the shell for the front";

#[test]
fn the_readme_promises_what_this_file_measures() {
    if seeded_rows().is_none() {
        return;
    }
    let text = readme();
    for sentence in [
        SECTION,
        "`/os` is the superset, and precedence is what keeps the pair apart.",
        "It cannot go through the front door it opens.",
        "**`seed_rows` inserts and never updates.**",
        "**A template registration asks a SECOND question.**",
    ] {
        assert!(
            text.contains(sentence),
            "the README must carry the promise this test pins: {sentence:?}"
        );
    }
}

/// The manifest is the shipped row with ONE decision flipped.
///
/// Every column the broker READS has to be identical, or the row that lands is
/// a different rule wearing the same id — and the README would be documenting a
/// switch for something else. `enabled` is the flip, and `note` is the one
/// column nothing compares and everything reads, so it is allowed to differ.
#[test]
fn the_readme_manifest_is_the_shipped_row_with_one_column_flipped() {
    let Some(rows) = seeded_rows() else {
        return;
    };
    let decls = json_block_under(&readme(), SECTION);
    let decl = &decls[0];
    assert_eq!(decl["scope"], json!("/os"));
    let entry = &decl["diff"]["seed_rows"][0];
    assert_eq!(entry["target"], json!("./access/store"));
    assert_eq!(entry["table"], json!("policy"));

    let manifest_row = entry["rows"][0].clone();
    let shipped = row_named(&rows, SHELL);
    for column in [
        "rule_id",
        "capability",
        "requester",
        "subject",
        "scope_match",
        "verdict",
        "max_ttl_ms",
        "constraints",
        "cred_ref",
        "priority",
    ] {
        assert_eq!(
            manifest_row[column], shipped[column],
            "the README manifest and the shipped row disagree about {column} — the switch \
             would land a DIFFERENT rule under the same id"
        );
    }
    assert_eq!(shipped["enabled"], json!(0));
    assert_eq!(
        manifest_row["enabled"],
        json!(1),
        "the manifest is the switch: it is the same row, enabled"
    );
    assert!(
        manifest_row["note"].as_str().is_some_and(|n| !n.is_empty()),
        "the twin carries its own note — a row nobody can explain is a row nobody dares \
         switch off again"
    );
}

// ──────────────────────────────────────────────────── the real `seed_rows` door

/// The store's declared column→type map for one table, read the way
/// `seed_rows::resolve_entries` reads it.
fn declared_columns(table: &str) -> BTreeMap<String, String> {
    let cfg: Value =
        meclaw_core::serde_json::from_str(&std::fs::read_to_string(STORE).expect("store config"))
            .expect("json");
    cfg["params"]["schema"][table]
        .as_object()
        .expect("the table is declared")
        .iter()
        .map(|(c, t)| (c.clone(), t.as_str().unwrap_or("text").to_string()))
        .collect()
}

fn resolved(
    dir: &std::path::Path,
    rows: Vec<Map<String, Value>>,
) -> meclaw_colony::mutation::seed_rows::ResolvedSeedRows {
    meclaw_colony::mutation::seed_rows::ResolvedSeedRows {
        target: meclaw_core::Path::new("/os/access/store"),
        cell_dir: dir.to_path_buf(),
        table: "policy".to_string(),
        columns: declared_columns("policy"),
        rows,
    }
}

fn as_object(v: &Value) -> Map<String, Value> {
    v.as_object().expect("a row is an object").clone()
}

/// Read back what the broker's `select` would read: the enabled rules for one
/// capability, in `priority` DESC — and with the `json` columns as the TEXT the
/// store keeps, which is the shape the script's `as_obj` expects.
fn rules_for(db: &std::path::Path, capability: &str) -> Vec<Value> {
    let conn = rusqlite::Connection::open(db.join("cell.db")).expect("the cell.db stands");
    let mut stmt = conn
        .prepare(
            "SELECT rule_id, requester, capability, subject, scope_match, verdict, \
             max_ttl_ms, constraints, priority, cred_ref FROM policy \
             WHERE enabled = 1 AND capability IN (?1, '*') ORDER BY priority DESC",
        )
        .expect("the select the broker makes");
    stmt.query_map([capability], |r| {
        Ok(json!({
            "rule_id": r.get::<_, String>(0)?,
            "requester": r.get::<_, String>(1)?,
            "capability": r.get::<_, String>(2)?,
            "subject": r.get::<_, String>(3)?,
            "scope_match": r.get::<_, String>(4)?,
            "verdict": r.get::<_, String>(5)?,
            "max_ttl_ms": r.get::<_, i64>(6)?,
            "constraints": r.get::<_, String>(7)?,
            "priority": r.get::<_, i64>(8)?,
            "cred_ref": r.get::<_, String>(9)?,
        }))
    })
    .expect("query")
    .collect::<Result<Vec<_>, _>>()
    .expect("rows")
}

/// A `cell.db` holding the shipped seed, written by the real door.
fn seeded_store(dir: &std::path::Path) -> Vec<Value> {
    let rows = seeded_rows().expect("guarded by the caller");
    let decl = resolved(dir, rows.iter().map(as_object).collect());
    let applied = meclaw_colony::mutation::seed_rows::apply_entries(std::slice::from_ref(&decl))
        .expect("the shipped seed satisfies the store's own schema");
    assert_eq!(applied[0].inserted, rows.len());
    rows
}

#[test]
fn the_switch_lands_through_the_mutation_door_and_re_applying_is_a_no_op() {
    if seeded_rows().is_none() {
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    seeded_store(td.path());

    // As shipped, the broker's own select finds ONE rule for `colony.mutate`.
    let before = rules_for(td.path(), "colony.mutate");
    assert_eq!(before.len(), 1, "as shipped: {before:?}");
    assert_eq!(before[0]["rule_id"], json!(DEFAULT));

    // The manifest out of the README, through the shape check the door runs on
    // the raw diff and then through the write.
    let decls = json_block_under(&readme(), SECTION);
    let parsed = meclaw_colony::mutation::seed_rows::parse_entries(&decls[0]["diff"])
        .expect("the README manifest is a legal seed_rows diff");
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].table, "policy");
    let entry = resolved(td.path(), parsed[0].rows.clone());
    let applied = meclaw_colony::mutation::seed_rows::apply_entries(std::slice::from_ref(&entry))
        .expect("the twin satisfies the same schema");
    assert_eq!(applied[0].inserted, 1);

    // `seed_rows` INSERTS: the disabled original stays, and what the broker
    // reads is the enabled twin — exactly one rule, and it is the shell's.
    let after = rules_for(td.path(), "colony.mutate");
    assert_eq!(
        after.len(),
        2,
        "both rules are now on, and precedence is what tells them apart: {after:?}"
    );
    assert_eq!(
        after
            .iter()
            .map(|r| r["rule_id"].clone())
            .collect::<Vec<_>>(),
        vec![json!(DEFAULT), json!(SHELL)],
        "read order is priority DESC, so the narrow row is examined first"
    );

    // Applying the same manifest twice is a no-op — the property that makes a
    // build script re-runnable, and the reason the operation is idempotent by
    // DECLARATION rather than by key.
    let again = meclaw_colony::mutation::seed_rows::apply_entries(std::slice::from_ref(&entry))
        .expect("a second apply");
    assert_eq!(again[0].inserted, 0);
    assert_eq!(again[0].already_present, 1);
}

// ─────────────────────────────────────────────────────────── the shipped verdict

/// One check-only verdict out of the shipped `policy` script, driven in the
/// phase the store's answer arrives in.
///
/// Returns `(status, reason_code, rule_id)` — the rule the audit row names, so
/// the precedence claim is read off the broker rather than off the fixture.
fn verdict(rules: &[Value], capability: &str, scope: &str) -> (String, String, String) {
    let carry = json!({
        "call_id": "q1", "requester": REQUESTER, "capability": capability,
        "subject": FRONT, "resource": {"scope": scope, "actions": ["apply"]},
        "purpose": "", "ttl_ms": 0, "check_only": true
    });
    let out = emit_all(
        &shipped_script(POLICY),
        &json!({
            "target": "/os/access",
            "header": { "hop": { "operation": "select" },
                        "context": { "access_origin": "policy", "ac_phase": "rules",
                                     "ac_carry": carry.to_string() } },
            "ttl": 64,
            "messages": [{ "origin": "tool", "type": "tool_result", "id": "x",
                           "text": Value::Array(rules.to_vec()).to_string() }],
            "params": {}
        }),
    );
    let grant = out
        .iter()
        .find(|m| m["header"]["route"] == json!("grant"))
        .unwrap_or_else(|| panic!("no verdict came back: {out:?}"));
    let answer: Value =
        meclaw_core::serde_json::from_str(grant["messages"][0]["text"].as_str().expect("text"))
            .expect("the answer is json");
    let rule = out
        .iter()
        .filter(|m| m["header"]["route"] == json!("astore"))
        .filter_map(|m| {
            let args: Value =
                meclaw_core::serde_json::from_str(m["messages"][0]["text"].as_str().expect("text"))
                    .ok()?;
            args["row"]["detail"]["rule_id"]
                .as_str()
                .map(str::to_string)
        })
        .next_back()
        .unwrap_or_default();
    (
        answer["status"].as_str().unwrap_or_default().to_string(),
        answer["reason_code"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        rule,
    )
}

#[test]
fn the_shell_scope_is_refused_as_shipped_and_permitted_once_the_row_is_on() {
    if seeded_rows().is_none() {
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    seeded_store(td.path());
    let shipped = rules_for(td.path(), "colony.mutate");

    // The defect, over the shipped rows: the front door has no rule to answer a
    // shell-scoped declaration with, and the refusal names the SCOPE rather
    // than saying the capability is unknown — a closed door, not a wall.
    assert_eq!(
        verdict(&shipped, "colony.mutate", "/os"),
        (
            "denied".to_string(),
            "scope_mismatch".to_string(),
            String::new()
        )
    );

    let decls = json_block_under(&readme(), SECTION);
    let parsed =
        meclaw_colony::mutation::seed_rows::parse_entries(&decls[0]["diff"]).expect("diff");
    meclaw_colony::mutation::seed_rows::apply_entries(&[resolved(
        td.path(),
        parsed[0].rows.clone(),
    )])
    .expect("the switch");
    let on = rules_for(td.path(), "colony.mutate");

    // Switched on, the shell answers — and it is the shell row that answers.
    assert_eq!(
        verdict(&on, "colony.mutate", "/os"),
        ("allowed".to_string(), String::new(), SHELL.to_string())
    );

    // And the narrow row keeps its ground. `/os` is the SUPERSET, so without
    // the priority split this line would name the shell row and every audit
    // entry about an organisation would stop naming the rule that meant it.
    for scope in ["/os/orgs", "/os/orgs/acme", "/os/orgs/acme/members/alex"] {
        assert_eq!(
            verdict(&on, "colony.mutate", scope),
            ("allowed".to_string(), String::new(), DEFAULT.to_string()),
            "an organisation is still answered by {DEFAULT} ({scope})"
        );
    }

    // A PATH prefix, not a string prefix — before and after.
    for rules in [&shipped, &on] {
        assert_eq!(
            verdict(rules, "colony.mutate", "/oscar").1,
            "scope_mismatch",
            "`/os` must not permit `/oscar`"
        );
    }

    // The widening the row buys, named rather than papered over: switched on,
    // `colony.mutate` reaches the broker's own address and the submitter's.
    // This is why the row ships off.
    for inside in ["/os/access", "/os/submit"] {
        assert_eq!(verdict(&shipped, "colony.mutate", inside).0, "denied");
        assert_eq!(
            verdict(&on, "colony.mutate", inside).0,
            "allowed",
            "switching the shell on grants every shell-level scope with it ({inside})"
        );
    }
}

#[test]
fn a_shell_scoped_registration_still_asks_code_author() {
    if seeded_rows().is_none() {
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    seeded_store(td.path());
    let decls = json_block_under(&readme(), SECTION);
    let parsed =
        meclaw_colony::mutation::seed_rows::parse_entries(&decls[0]["diff"]).expect("diff");
    meclaw_colony::mutation::seed_rows::apply_entries(&[resolved(
        td.path(),
        parsed[0].rows.clone(),
    )])
    .expect("the switch");

    // `code.author.default` is scoped `/os/orgs` and ships OFF, so the enabled
    // set for that capability is empty and the answer is `capability_unknown`:
    // a missing rule is a denial rather than a silence. The row above moves a
    // shell-scoped `add_templates` from `requester_not_permitted` to
    // `code_author_denied` — a different refusal, not a green light.
    let authors = rules_for(td.path(), "code.author");
    assert!(
        authors.is_empty(),
        "code.author must grant nothing on a fresh tree: {authors:?}"
    );
    assert_eq!(
        verdict(&authors, "code.author", "/os"),
        (
            "denied".to_string(),
            "capability_unknown".to_string(),
            String::new()
        )
    );
}

// ──────────────────────────────────────────────────────────── the front door

fn digest_of(decls: &Value) -> String {
    let program = concat!(
        "import sys, json, hashlib\n",
        "d = json.load(sys.stdin)\n",
        "c = json.dumps(d, sort_keys=True, separators=(',', ':'), ensure_ascii=False)\n",
        "sys.stdout.write(hashlib.sha256(c.encode('utf-8')).hexdigest())\n"
    );
    String::from_utf8(run_shipped_script(program, &decls.to_string()).stdout).expect("hex")
}

fn args_of(msg: &Value) -> Value {
    meclaw_core::serde_json::from_str(msg["messages"][0]["text"].as_str().expect("a tool_call"))
        .expect("the args are json")
}

/// A topology declaration standing on the SHELL. No script, no `add_templates`,
/// no subscribe edge — so it asks exactly one capability question, which is
/// what keeps this measurement about the scope and nothing else.
fn a_shell_declaration() -> Value {
    json!([{
        "scope": "/os", "ctx": {},
        "diff": { "add_nodes": [{ "name": "./scratch", "template": "terminal" }] }
    }])
}

fn submit(decls: &Value) -> Vec<Value> {
    emit_all(
        &shipped_script(GATE),
        &json!({
            "target": "/os/submit",
            "reply_to": FRONT,
            "header": { "hop": { "route": "in_apply", "manifest_sha256": digest_of(decls),
                                 "tool_call_id": "op:c1" }, "context": {} },
            "ttl": 64,
            "manifest": decls,
            "messages": [],
            "params": {}
        }),
    )
}

fn verdict_back(capability: &str, status: &str, sha: &str) -> Vec<Value> {
    emit_all(
        &shipped_script(GATE),
        &json!({
            "target": "/os/submit",
            "header": { "hop": { "route": "in_verdict" },
                        "context": { "sub_ask": "1", "sub_sha": sha } },
            "ttl": 64,
            "messages": [{ "origin": "tool", "type": "tool_result", "id": "q1",
                "text": json!({ "status": status, "capability": capability,
                                "reason_code": "" }).to_string() }],
            "params": {}
        }),
    )
}

/// The store's answer to the un-parking `select`: the manifest comes back off
/// the ROW, which is the only place it could have waited.
fn unpark(phase: &str, decls: &Value) -> Vec<Value> {
    let sha = digest_of(decls);
    let rows = json!([{ "id": "p1", "manifest": decls, "requester": FRONT,
                        "tool_call_id": "op:c1", "manifest_sha256": sha }]);
    emit_all(
        &shipped_script(GATE),
        &json!({
            "target": "/os/submit",
            "header": {
                "hop": { "operation": "select", "rows_affected": 1 },
                "context": { "sub_origin": "gate", "sub_phase": phase,
                             "sub_carry": "{\"status\":\"allowed\"}" }
            },
            "ttl": 64,
            "messages": [{ "origin": "tool", "type": "tool_result", "id": "x",
                           "text": rows.to_string() }],
            "params": {}
        }),
    )
}

#[test]
fn the_front_asks_over_the_shell_and_the_answer_decides_the_door() {
    if seeded_rows().is_none() {
        return;
    }
    let decls = a_shell_declaration();
    let out = submit(&decls);
    let ask = out
        .iter()
        .find(|m| m["header"]["route"] == json!("ask"))
        .unwrap_or_else(|| panic!("no question was put: {out:?}"));
    let q = args_of(ask);
    assert_eq!(q["capability"], json!("colony.mutate"));
    assert_eq!(q["check_only"], json!(true));
    assert_eq!(
        q["resource"]["scope"],
        json!("/os"),
        "the question is asked over the manifest's scope ROOT, which for a shell-level \
         declaration is the shell"
    );
    assert_eq!(
        q["subject"],
        json!(FRONT),
        "R-AC-1: the identity the substrate stamped travels as `subject`. Everything that \
         passes the front carries THIS path, whoever initiated it — which is why `which \
         door` is not an axis a policy row can compare on"
    );

    let sha = digest_of(&decls);

    // The refusal the issue measured, in the string the template has always
    // used — and nothing reaches the door.
    let denied = verdict_back("colony.mutate", "denied", &sha);
    assert!(
        denied
            .iter()
            .all(|m| m["header"]["route"] != json!("mutate")),
        "a denied submission never reaches the mutation door: {denied:?}"
    );
    let receipt = denied
        .iter()
        .find(|m| m["header"]["route"] == json!("receipt"))
        .expect("a refusal is still an answer");
    assert_eq!(
        receipt["header"]["error_code"],
        json!("requester_not_permitted")
    );

    // And with the row switched on, the same declaration goes through. The
    // allowed verdict does not submit by itself — the manifest waits parked,
    // because the broker's answer REPLACED the body it travelled in, so the
    // gate reads it back off its own store first.
    let allowed = verdict_back("colony.mutate", "allowed", &sha);
    assert_eq!(
        allowed[0]["header"]["route"],
        json!("sstore"),
        "an allowed verdict un-parks the manifest: {allowed:?}"
    );
    let out = unpark("parked", &decls);
    assert_eq!(
        out.len(),
        3,
        "forget the park, remember the flight, submit: {out:?}"
    );
    assert_eq!(
        out[2]["header"]["route"],
        json!("mutate"),
        "an allowed shell-scoped submission reaches the door"
    );
    assert_eq!(
        out[2]["manifest"][0]["scope"],
        json!("/os"),
        "and what reaches it is the declaration that was refused a moment ago"
    );
}
