//! GH #1225 (KX) -- the finding classes of the curator wave, outside the curator.
//!
//! The class scripts of the wave (`plans/welle-kurator-fix-2026-10-15/klassen/`)
//! found the curator's gaps in other templates too:
//! - a row that must exist once written as a `delete` and an `insert` (two legs
//!   of a bundle that is no transaction: a reader between them misses the row,
//!   a failed insert loses it) -- the collector's legend, file-space's vectors,
//!   its done mark and its hook answer;
//! - an upsert by read: `update` where the row was read, a plain `insert` where
//!   it was not, so two messages in flight both insert -- file-space's
//!   workspace rows and memory-hive's beliefs;
//! - a refused read taken for "no rows" -- the objects gate answered
//!   `not_found` for an object that exists, the push re-pushed what went out.
//!
//! Each site now carries the one-step form (`insert` with `replace` or
//! `if_absent`, form of `misc_2::gh1199_a_row_is_replaced_in_one_step`) or a
//! check of the refused leg that ends the message. Unit class: the shipped
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

fn collapsed(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Checks every (template, form that must be gone, form that must stand).
fn check(sites: &[(&str, &str, &str)]) -> Vec<String> {
    let mut wrong = Vec::new();
    for &(template, gone, stand) in sites {
        let Some(text) = script(template) else {
            continue; // a trimmed tree without this template has nothing to pin
        };
        let text = collapsed(&text);
        if !gone.is_empty() && text.contains(&collapsed(gone)) {
            wrong.push(format!("{template}: still there: {gone}"));
        }
        if !text.contains(&collapsed(stand)) {
            wrong.push(format!("{template}: missing: {stand}"));
        }
    }
    wrong
}

#[test]
fn gh1225_a_row_that_must_exist_once_is_written_in_one_step() {
    let wrong = check(&[
        (
            "collector/assemble",
            r#""joined_at": "%s#%03d" % (now, i)}}))"#,
            r#""joined_at": "%s#%03d" % (now, i)}, "if_absent": {"session_id": session, "ref": str(e.get("ref"))}}))"#,
        ),
        (
            "file-space/derive",
            r#"ops.append(("e-%d" % i, ins("embeddings", {"#,
            r#"ops.append(("e-put", {"operation": "insert", "table": "embeddings", "rows": vecs, "replace": {"file": f, "version": v}}))"#,
        ),
        (
            "file-space/derive",
            r#"("m-del", dele("pending", {"op_id": "done:" + f, "cell": ME})),"#,
            r#""replace": {"op_id": "done:" + f, "cell": ME}}),"#,
        ),
        (
            "file-space/write",
            r#"("clr", dele("pending", {"op_id": pid, "cell": ME + ":hook"})),"#,
            r#""replace": {"op_id": pid, "cell": ME + ":hook"}}),"#,
        ),
    ]);
    assert!(
        wrong.is_empty(),
        "GH #1225 delete+insert:\n{}",
        wrong.join("\n")
    );
}

#[test]
fn gh1225_a_row_read_as_missing_is_claimed_not_inserted_twice() {
    let wrong = check(&[
        (
            "file-space/ws",
            r#"ins("ws_files", {"ws": wsr["ws"], "file": f,"#,
            r#""if_absent": {"ws": wsr["ws"], "file": f}}))"#,
        ),
        // a row a concurrent job made first is a lost swing like a moved one
        (
            "file-space/ws",
            r#"lost = [e for e in mods if e["row"] and affected("#,
            r#"lost = [e for e in mods if affected(lg, "s:" + e["file"]) != 1]"#,
        ),
        (
            "file-space/ws",
            r#"lost = [e for e in rms if e["row"] and affected("#,
            r#"lost = [e for e in rms if affected(lg, "s:" + e["file"]) != 1]"#,
        ),
        (
            "memory-hive/dream-glue",
            "",
            r#""if_absent": {"id": bid}}, "applied"))"#,
        ),
    ]);
    assert!(
        wrong.is_empty(),
        "GH #1225 upsert by read:\n{}",
        wrong.join("\n")
    );
}

#[test]
fn gh1225_a_refused_read_of_the_objects_is_never_no_rows() {
    let refused = r#"def refused(lg):"#;
    let wrong = check(&[
        ("objects/gate", "", refused),
        (
            "objects/gate",
            "",
            r#"if phase == "seen-keys": bad = refused(lg) if bad: park("#,
        ),
        (
            "objects/gate",
            "",
            r#"if phase == "seen-heads": bad = refused(lg) if bad: park("#,
        ),
        (
            "objects/gate",
            "",
            r#"if phase == "edit-head": bad = refused(lg) if bad: emit([answer(c, code="store_error","#,
        ),
        ("objects/push", "", refused),
        (
            "objects/push",
            "",
            r#"if phase == "facts-read": bad = refused(lg) if bad: park("#,
        ),
    ]);
    assert!(
        wrong.is_empty(),
        "GH #1225 error as empty:\n{}",
        wrong.join("\n")
    );
}
