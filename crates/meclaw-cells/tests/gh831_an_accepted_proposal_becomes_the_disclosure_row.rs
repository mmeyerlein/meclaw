//! GH #831 — an accepted proposal about a disclosure becomes the disclosure row.
//!
//! Measured at `affinity@3.4.0`: `decide_proposal accepted` marked the judged row
//! `superseded`, appended the verdict, wrote an audit line and acked -- and nothing
//! else. The proposal's `field_path` and `audience` never became a `disclosure` row,
//! so a caller that wanted the release had to repeat both in a second `set_disclosure`,
//! and the README's promise that the member *makes* the release was kept by nobody.
//!
//! What is pinned here, at the gate script itself (the shipped `script_inline`, run
//! over stdin the way a `code` cell runs it):
//!
//! 1. an accepted verdict with an audience writes exactly one `disclosure` row in the
//!    same pass, its `id` names the verdict, and the `ack` and the audit `detail`
//!    carry it;
//! 2. a `rejected` verdict, an accepted verdict without an audience and a `propose`
//!    accepted as it arrives write none;
//! 3. an unknown `mode` and a `field_path` that is not rooted at `aieos.`/`mx.` are
//!    refused by name, and nothing at all is written;
//! 4. the proposed `value` is never applied, and the README says who writes it (the
//!    drift lock, `docs/development-rules.md` § 2d).
//!
//! The end-to-end half -- the row in a real store, and an `in_brief` afterwards that
//! serves the field -- lives in `affinity_template.rs`
//! (`a_directory_audience_is_never_auto_accepted`).

#[path = "support/assemble_cell.rs"]
mod assemble_cell;

use assemble_cell::{repo, run_cell};
use serde_json::{Value, json};

const GATE: &str = "templates/affinity/gate/config.json";
const README: &str = "templates/affinity/README.md";

/// One tool call arriving at the gate, the writer named by the edge.
fn gate(args: Value) -> Vec<Value> {
    run_cell(
        GATE,
        &[],
        json!({
            "messages": [{"origin": "assistant", "type": "tool_call", "id": "c1",
                          "text": args.to_string()}],
            "header": {"hop": {}, "context": {"actor": "member:alex"}}
        }),
    )
    .0
}

/// Every store op the gate emitted, in emission order.
fn store_ops(out: &[Value]) -> Vec<Value> {
    out.iter()
        .filter(|m| m["header"]["route"] == "astore")
        .filter_map(|m| serde_json::from_str(m["messages"][0]["text"].as_str()?).ok())
        .collect()
}

fn op_on(out: &[Value], table: &str, operation: &str) -> Vec<Value> {
    store_ops(out)
        .into_iter()
        .filter(|a| a["table"] == table && a["operation"] == operation)
        .collect()
}

fn ack(out: &[Value]) -> Value {
    let m = out
        .iter()
        .find(|m| m["header"]["route"] == "ack")
        .expect("the gate always acks");
    serde_json::from_str(m["messages"][0]["text"].as_str().expect("ack text")).expect("ack json")
}

fn audit(out: &[Value]) -> Value {
    let rows = op_on(out, "audit", "insert");
    assert_eq!(rows.len(), 1, "exactly one audit row per call: {out:?}");
    rows[0]["row"].clone()
}

/// The verdict of a directory proposal, as the member sends it.
fn verdict(status: &str) -> Value {
    json!({
        "op": "decide_proposal",
        "id": "prop:0123abcd",
        "status": status,
        "source_ref": "mem:ep-1",
        "entity_ref": "entity:alex",
        "field_path": "aieos.interests.favorites.music_genre",
        "value": "jazz",
        "audience": "directory:example"
    })
}

fn with(mut v: Value, key: &str, value: Value) -> Value {
    v[key] = value;
    v
}

// ═══════════════════════════════════════════════════════════════════════ pins

#[test]
fn an_accepted_verdict_with_an_audience_writes_the_disclosure_row() {
    let out = gate(with(
        with(verdict("accepted"), "mode", json!("summarize")),
        "audience_set",
        json!(["member:alex", "directory:example"]),
    ));

    let verdicts = op_on(&out, "proposals", "insert");
    assert_eq!(verdicts.len(), 1, "one verdict row: {out:?}");
    let vid = verdicts[0]["row"]["id"]
        .as_str()
        .expect("verdict id")
        .to_string();
    let hex = vid
        .strip_prefix("prop:")
        .expect("a verdict id is prop:<hex>");

    let rows = op_on(&out, "disclosure", "insert");
    assert_eq!(rows.len(), 1, "exactly one release: {out:?}");
    let row = &rows[0]["row"];
    assert_eq!(row["entity_id"], "entity:alex", "{row}");
    assert_eq!(
        row["field_path"], "aieos.interests.favorites.music_genre",
        "{row}"
    );
    assert_eq!(row["audience"], "directory:example", "{row}");
    assert_eq!(
        row["audience_set"],
        json!(["directory:example", "member:alex"]),
        "the set it was released in, sorted like set_disclosure sorts it: {row}"
    );
    assert_eq!(
        row["mode"], "summarize",
        "the mode comes from the verdict: {row}"
    );
    assert_eq!(
        row["id"],
        json!(format!("disc:{hex}")),
        "the release names the verdict that made it: {row}"
    );
    assert!(
        !row["decided_at"].as_str().unwrap_or_default().is_empty(),
        "{row}"
    );

    let a = ack(&out);
    assert_eq!(a["outcome"], "accepted", "{a}");
    assert_eq!(a["id"], json!(vid), "{a}");
    assert_eq!(a["supersedes"], "prop:0123abcd", "{a}");
    assert_eq!(
        a["disclosure"], row["id"],
        "the ack carries the release: {a}"
    );

    let log = audit(&out);
    assert_eq!(log["outcome"], "ok", "{log}");
    assert_eq!(
        log["detail"]["disclosure"], row["id"],
        "the audit row names the release it wrote: {log}"
    );

    // The single pass keeps its order: the judged row is marked, the verdict
    // lands, and the release it makes comes after it.
    let order: Vec<String> = store_ops(&out)
        .iter()
        .map(|a| {
            format!(
                "{}:{}",
                a["operation"].as_str().unwrap_or(""),
                a["table"].as_str().unwrap_or("")
            )
        })
        .collect();
    assert_eq!(
        order,
        vec![
            "update:proposals",
            "insert:proposals",
            "insert:disclosure",
            "insert:audit"
        ],
        "{out:?}"
    );
}

#[test]
fn a_verdict_without_mode_or_set_releases_to_the_audience_alone_and_shares() {
    let out = gate(verdict("accepted"));
    let rows = op_on(&out, "disclosure", "insert");
    assert_eq!(rows.len(), 1, "{out:?}");
    assert_eq!(rows[0]["row"]["mode"], "share", "default mode: {rows:?}");
    assert_eq!(
        rows[0]["row"]["audience_set"],
        json!(["directory:example"]),
        "default set is the addressee alone: {rows:?}"
    );
}

#[test]
fn a_rejected_verdict_writes_no_row() {
    let out = gate(with(verdict("rejected"), "mode", json!("share")));
    assert_eq!(
        ack(&out)["outcome"],
        "accepted",
        "the verdict itself is written"
    );
    assert_eq!(op_on(&out, "proposals", "insert").len(), 1, "{out:?}");
    assert!(
        op_on(&out, "disclosure", "insert").is_empty(),
        "a rejection releases nothing: {out:?}"
    );
    assert!(ack(&out).get("disclosure").is_none(), "{:?}", ack(&out));
    assert!(audit(&out)["detail"].get("disclosure").is_none());
}

#[test]
fn an_accepted_verdict_without_an_audience_releases_nothing() {
    let mut v = verdict("accepted");
    v.as_object_mut().unwrap().remove("audience");
    let out = gate(v);
    assert_eq!(ack(&out)["outcome"], "accepted", "{out:?}");
    assert!(
        op_on(&out, "disclosure", "insert").is_empty(),
        "an R-AF-1 extension with nobody named is not a release: {out:?}"
    );
}

#[test]
fn an_auto_accepted_proposal_writes_no_row() {
    for auto in [json!(true), Value::Null] {
        let mut p = json!({
            "op": "propose",
            "source_ref": "mem:ep-2",
            "entity_ref": "entity:alex",
            "field_path": "aieos.interests.favorites.food",
            "value": "ramen",
            "audience": "agent:aiden"
        });
        if !auto.is_null() {
            p["auto_accept"] = auto.clone();
        }
        let out = gate(p);
        assert_eq!(
            ack(&out)["status"],
            "accepted",
            "R-AF-1 is unchanged: {out:?}"
        );
        assert!(
            op_on(&out, "disclosure", "insert").is_empty(),
            "a proposal accepted as it arrives never releases (auto_accept={auto}): {out:?}"
        );
    }
}

#[test]
fn an_unknown_mode_is_refused_by_name_and_writes_nothing() {
    let out = gate(with(verdict("accepted"), "mode", json!("publish")));
    let a = ack(&out);
    assert_eq!(a["outcome"], "rejected", "{a}");
    assert_eq!(a["reason_code"], "disclosure_mode_unknown", "{a}");
    let writes: Vec<Value> = store_ops(&out)
        .into_iter()
        .filter(|o| o["table"] != "audit")
        .collect();
    assert!(
        writes.is_empty(),
        "neither the verdict nor the release is half-written: {writes:?}"
    );
    assert_eq!(audit(&out)["reason_code"], "disclosure_mode_unknown");
}

#[test]
fn an_unrooted_field_path_is_refused_by_name() {
    let out = gate(with(
        verdict("accepted"),
        "field_path",
        json!("interests.music"),
    ));
    let a = ack(&out);
    assert_eq!(a["outcome"], "rejected", "{a}");
    assert_eq!(a["reason_code"], "disclosure_field_unrooted", "{a}");
    let writes: Vec<Value> = store_ops(&out)
        .into_iter()
        .filter(|o| o["table"] != "audit")
        .collect();
    assert!(writes.is_empty(), "nothing is written: {writes:?}");

    // A rejection judges no release, so its field path is none of this rule's business.
    let out = gate(with(
        verdict("rejected"),
        "field_path",
        json!("interests.music"),
    ));
    assert_eq!(ack(&out)["outcome"], "accepted", "{out:?}");
}

/// Drift lock (`docs/development-rules.md` § 2d): the README sentence that says who
/// applies a verdict is read, and the mechanism it describes is run.
#[test]
fn the_readme_states_who_applies_a_verdict() {
    let readme = std::fs::read_to_string(repo(README)).expect("README");
    let flat = readme.split_whitespace().collect::<Vec<_>>().join(" ");

    // The table row of the op names both new refusals -- derived from the script,
    // not typed twice.
    let mode_code = ack(&gate(with(verdict("accepted"), "mode", json!("x"))))["reason_code"]
        .as_str()
        .unwrap()
        .to_string();
    let root_code =
        ack(&gate(with(verdict("accepted"), "field_path", json!("x.y"))))["reason_code"]
            .as_str()
            .unwrap()
            .to_string();
    let row = readme
        .lines()
        .find(|l| l.starts_with("| `decide_proposal` |"))
        .expect("the write-ops table has a decide_proposal row");
    for code in [&mode_code, &root_code] {
        assert!(
            row.contains(&format!("`{code}`")),
            "{row} does not name {code}"
        );
    }
    assert!(
        row.contains("`disclosure`"),
        "the row says the verdict writes the release: {row}"
    );

    // The sentence: the verdict is the release, and the value is `upsert_entity`'s.
    let sentence = flat
        .split(". ")
        .find(|s| s.contains("never applies the proposed `value`"))
        .expect("README names what the verdict does not apply");
    assert!(
        sentence.contains("`upsert_entity`"),
        "the README names who writes the value: {sentence}"
    );
    assert!(
        flat.contains("An accepted verdict is the release"),
        "the paragraph heading of the rule is missing"
    );

    // ...and the mechanism: a verdict carrying a value writes no entity.
    let out = gate(verdict("accepted"));
    assert!(
        store_ops(&out).iter().all(|o| o["table"] != "entities"),
        "the verdict applied a value: {out:?}"
    );
    assert_eq!(op_on(&out, "disclosure", "insert").len(), 1, "{out:?}");
}
