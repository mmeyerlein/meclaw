//! GH #1199 / #1093 -- a row that must exist once is written in ONE step.
//!
//! The registry hand wrote an unknown subscriber row as a `delete` and an
//! `insert`, two legs of a store bundle that is no transaction: a reader
//! between them missed the row (the coverage of a20e7d3fe, #1093), an insert
//! that failed lost it, and a late duplicate reset what was booked on it. The
//! store's `insert` writes such a row in one step now (`if_absent`, `replace`;
//! behaviour pinned in `store::ops::tests::gh1199_*` and, for the hand, in
//! `misc_7::gh855_a_grown_assistant_is_a_subscriber::gh1199_*`).
//!
//! This file pins the class across the shipped templates: every site the
//! class search of the strand found writing a row that must exist once as a
//! delete followed by an insert (`recheck/skripte/v2-fix-g855/delete_insert.py`,
//! which also lists the pairs that are NOT that gap and why) carries the
//! one-step form, and the two-step form is gone. Unit class: the shipped
//! files are read, nothing runs.

use meclaw_core::serde_json::Value;

fn script(template: &str) -> Option<String> {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates")
        .join(template)
        .join("config.json");
    let raw = std::fs::read_to_string(&p).ok()?;
    let v: Value =
        meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    Some(
        v["params"]["script_inline"]
            .as_str()
            .unwrap_or_else(|| panic!("{}: no script_inline", p.display()))
            .to_string(),
    )
}

/// (template, the two-step form that must be gone, the one-step form that
/// must stand). Whitespace is collapsed before matching.
const SITES: [(&str, &str, &str); 8] = [
    (
        "llm-registry/hand",
        r#"writes.append(call("delete", table="subscribers","#,
        r#"writes.append(call("insert", table="subscribers", row=row, if_absent={"cell_path": path}))"#,
    ),
    (
        "llm-registry/hand",
        r#"writes.append(call("delete", table="models","#,
        r#"replace={"model_id": cmd["model_id"]}))"#,
    ),
    (
        "collector/assemble",
        r#"("c-menu-del","#,
        r#""replace": {"answerer": answerer}}),"#,
    ),
    (
        "collector/assemble",
        r#"("c-open-peer-drop","#,
        r#""replace": {"session_id": session, "role": "depart", "correlation": ""}}"#,
    ),
    (
        "collector/assemble",
        r#"("c-open-scope-del","#,
        r#""replace": {"session_id": session}}"#,
    ),
    (
        "memory-hive/dream-glue",
        r#"out.append(store_op({"operation": "delete", "table": "predicate_cardinality","#,
        r#""replace": {"canonical_predicate": predicate}},"#,
    ),
    (
        "submit/gate",
        r#"untouched by the detour through the broker. sys.stdout.write(json.dumps([ store("delete", "written", None, table="submissions","#,
        r#"replace={"id": str(row["id"])}),"#,
    ),
    (
        "graph-space/index",
        r#"("e-del", {"operation": "delete", "table": "edges", "where": {"from_source": S}})]"#,
        r#"ops = one_step(ops, "e-put", "edges", {"from_source": S})"#,
    ),
];

fn collapsed(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn gh1199_every_row_that_must_exist_once_is_written_in_one_step() {
    let mut wrong = Vec::new();
    for (template, two_step, one_step) in SITES {
        let Some(text) = script(template) else {
            continue; // a trimmed tree without this template has nothing to pin
        };
        let text = collapsed(&text);
        if text.contains(&collapsed(two_step)) {
            wrong.push(format!(
                "{template}: still a delete and an insert: {two_step}"
            ));
        }
        if !text.contains(&collapsed(one_step)) {
            wrong.push(format!(
                "{template}: the one-step form is missing: {one_step}"
            ));
        }
    }
    assert!(wrong.is_empty(), "GH #1199:\n{}", wrong.join("\n"));
}

/// The graph-space index folds its nodes and names the same way as its edges
/// (the tombstone path, which only removes, keeps its bare deletes).
#[test]
fn gh1199_the_graph_index_replaces_nodes_and_names_in_one_step() {
    let Some(text) = script("graph-space/index") else {
        return;
    };
    let text = collapsed(&text);
    for form in [
        r#"ops = one_step(ops, "n-put", "nodes", {"source": S})"#,
        r#"ops = one_step(ops, "m-put", "names", {"source": S})"#,
        r#"return rest + [(put_id, {"operation": "insert", "table": table, "rows": rows, "replace": where})]"#,
    ] {
        assert!(text.contains(&collapsed(form)), "missing: {form}");
    }
}
