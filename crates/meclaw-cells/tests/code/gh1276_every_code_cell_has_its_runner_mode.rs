//! GH #1276 -- every shipped `code` cell has its runner mode in writing.
//!
//! `templates/RUNNER-MODES.md` holds one row per `code` cell under
//! `templates/`: its path, its mode, where it sits on a turn (`passes`) and,
//! for `cold` and `resident`, the reason. The rule (`docs/cell-types.md`
//! § `code`, which mode): `warm` is the default, `resident` only for a cell
//! that caches its own durable state, `cold` only with a written reason. This
//! lock holds the table against every `templates/**/config.json` whose
//! `cell.type` is `code`:
//!
//! * rows and cells are one to one (a new cell without a row is red, a row
//!   whose cell is gone is red, in the private tree also when its whole
//!   template is gone; only in the public clone -- it ships a subset of the
//!   library -- a row of a template the tree does not hold is not judged);
//! * the row's mode is the config's `params.runner_mode` (absent reads `cold`,
//!   the parser's default);
//! * `passes` is one of `turn|tool|session|after|rare` (the `?` a new row
//!   gets from the inventory tool is red until it is placed);
//! * `cold` and `resident` carry a reason, and so does a `warm` row that
//!   passes `rare` (GH #1294: a rarely called cell runs cold);
//! * the file names no template version (`name@x.y.z`).

// harness_fold: #[path = "support/gh1276_runner_modes.rs"]
#[allow(unused_imports)]
use crate::runner_modes; // harness_fold: mod runner_modes;

use runner_modes::{
    MODES, PASSES, code_cells, inventory_text, judges_the_row, names_a_version, needs_a_reason,
    parse, private_tree, templates,
};
use std::collections::BTreeMap;

#[test]
fn every_shipped_code_cell_has_one_row_with_its_mode() {
    let root = templates();
    let mut cells = BTreeMap::new();
    code_cells(&root, &root, &mut cells);
    assert!(
        cells.len() > 100,
        "the template tree holds its code cells ({} found)",
        cells.len()
    );
    let text = inventory_text();
    let (rows, mut errors) = parse(&text);
    let private = private_tree();
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for row in &rows {
        let cell = row.cell.as_str();
        if let Some(first) = seen.insert(cell, row.line) {
            errors.push(format!(
                "{cell}: two rows (lines {first} and {}) -- rule: one row per cell",
                row.line
            ));
        }
        if !MODES.contains(&row.mode.as_str()) {
            errors.push(format!(
                "{cell}: mode `{}` is not one of cold|warm|resident",
                row.mode
            ));
        }
        // The public tree ships a subset of the library: there a row whose
        // template is not in the tree has nothing to be held against. In the
        // private tree it is a row of a template that is gone.
        let template = cell.split('/').next().unwrap_or(cell);
        if !judges_the_row(root.join(template).is_dir(), private) {
            continue;
        }
        match cells.get(cell) {
            None => errors.push(format!(
                "{cell}: a row without a shipped code cell -- rule: rows and cells are one to one"
            )),
            Some(found) if *found != row.mode => errors.push(format!(
                "{cell}: expected `{}` (the row), found `{found}` (params.runner_mode) -- rule: \
                 the row's mode is the config's",
                row.mode
            )),
            Some(_) => {}
        }
        if !PASSES.contains(&row.passes.as_str()) {
            errors.push(format!(
                "{cell}: passes `{}` is not one of turn|tool|session|after|rare -- rule: every \
                 row is placed",
                row.passes
            ));
        }
        if needs_a_reason(&row.mode, &row.passes) && row.reason.is_empty() {
            errors.push(format!(
                "{cell}: `{}` on passes `{}` without a reason -- rule: warm is the default \
                 for a cell a turn passes, cold for a rare one, anything else is said in writing",
                row.mode, row.passes
            ));
        }
    }
    for (cell, mode) in &cells {
        if !seen.contains_key(cell.as_str()) {
            errors.push(format!(
                "{cell}: a shipped code cell (`{mode}`) without a row -- rule: rows and cells \
                 are one to one"
            ));
        }
    }
    if let Some(v) = names_a_version(&text) {
        errors.push(format!(
            "the inventory names a template version `{v}` (gh408)"
        ));
    }
    assert!(
        errors.is_empty(),
        "templates/RUNNER-MODES.md breaks the runner-mode lock (GH #1276), {} finding(s):\n{}",
        errors.len(),
        errors.join("\n")
    );
}

/// A row whose whole template is gone (deleted, renamed) is red in the
/// private tree; only the public clone, which ships a subset, skips it.
#[test]
fn a_row_of_a_missing_template_is_judged_in_the_private_tree() {
    assert!(judges_the_row(true, true));
    assert!(judges_the_row(true, false));
    assert!(
        judges_the_row(false, true),
        "private tree: a row of a missing template is a row without a shipped code cell"
    );
    assert!(
        !judges_the_row(false, false),
        "public clone: a private template's row is not judged"
    );
}

#[test]
fn the_parser_reads_a_blank_reason_and_refuses_a_pipe() {
    let (rows, broken) = parse(
        "| cell | mode | passes | reason |\n|---|---|---|---|\n| `a/b` | warm | turn | |\n\
         | `c` | cold | rare | a | b |\n",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].cell, "a/b");
    assert_eq!(rows[0].reason, "");
    assert_eq!(broken.len(), 1, "a pipe in a reason is a broken row");
    assert_eq!(
        names_a_version("see talky@1.2.3 here"),
        Some("talky@1.2.3".into())
    );
    assert_eq!(names_a_version("a mail@example or x@1.2"), None);
}

/// GH #1294: a rare cell runs cold unless its row says why it stays warm; a
/// cell a turn passes runs warm without a word.
#[test]
fn a_warm_row_of_a_rare_cell_needs_its_reason() {
    assert!(needs_a_reason("warm", "rare"));
    assert!(needs_a_reason("cold", "turn"));
    assert!(needs_a_reason("resident", "after"));
    for passes in ["turn", "tool", "session", "after"] {
        assert!(
            !needs_a_reason("warm", passes),
            "warm on {passes} is the default"
        );
    }
    assert!(
        needs_a_reason("cold", "rare"),
        "cold keeps its reason on every pass"
    );
}
