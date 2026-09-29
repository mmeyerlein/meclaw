//! GH #902: the pure half of the workspace cell `ws` of `templates/file-space`,
//! held to tables. `split_patch` cuts a multi-file unified diff by the line
//! counts of its hunks, `created_text` reads the content of a new file,
//! `commit_plan` decides what a commit does (conflict, tombstoned, base_moved, behind, or
//! a plan in lock order), `recover_action` decides what the next visitor does
//! with a commit it met through a lock, and `to_ms`/`stamp_ms` read its
//! deadline. They are plain functions of the cell's `script_inline` --
//! `script_inline` knows no library (OR-FH-G1) -- so this lock loads them the
//! way the other file-space tables do (the AST loader of
//! `support/file_space_hive.rs`) and pins their docstrings as behavior: the
//! commit protocol rests on them, and a regression here is a lost or a
//! half-visible commit, not a crash.

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{Value, json};

/// A deadline in ms since the epoch, and the same instant as the cell's stamp.
const D: u64 = 1759140000123;
const ISO: &str = "2025-09-29T10:00:00.123000Z";

/// A probe that names what `split_patch` raises on each input, `ok` if nothing.
const CATCH: &str = r#"(exec("""
def _catch(d):
    try:
        split_patch(d)
        return 'ok'
    except Exception as e:
        return type(e).__name__
""", globals()), [_catch(d) for d in ARGS])[1]"#;

/// One call of `probe` over every input of the table, each answer held to its
/// row.
fn check(probe: &str, table: &[(&str, Value, Value)]) {
    let args: Vec<Value> = table.iter().map(|(_, arg, _)| arg.clone()).collect();
    let got = pure("ws", probe, json!(args));
    let got = got.as_array().expect("the probe answers a list");
    assert_eq!(got.len(), table.len(), "one answer per row: {got:?}");
    for ((label, arg, want), got) in table.iter().zip(got) {
        assert_eq!(got, want, "{label}: input {arg}");
    }
}

/// `split_patch`: one entry per file in input order, the end of a hunk
/// is its counts, headers normalised to absolute paths.
#[test]
fn split_patch_cuts_a_diff_by_its_line_counts() {
    if !shipped() {
        return;
    }
    let table: Vec<(&str, Value, Value)> = vec![
        (
            "a modify with one hunk, git prefixes dropped",
            json!(concat!(
                "--- a/docs/x.md\n",
                "+++ b/docs/x.md\n",
                "@@ -1,2 +1,2 @@\n",
                " a\n",
                "-b\n",
                "+c\n"
            )),
            json!([
                {
                    "path": "/docs/x.md",
                    "kind": "modify",
                    "hunks": [
                        concat!(
                            "@@ -1,2 +1,2 @@\n",
                            " a\n",
                            "-b\n",
                            "+c\n"
                        ),
                    ],
                },
            ]),
        ),
        (
            "two hunks of one file",
            json!(concat!(
                "--- /x.md\n",
                "+++ /x.md\n",
                "@@ -1 +1 @@\n",
                "-a\n",
                "+A\n",
                "@@ -5,1 +5,1 @@\n",
                "-e\n",
                "+E\n"
            )),
            json!([
                {
                    "path": "/x.md",
                    "kind": "modify",
                    "hunks": [
                        concat!(
                            "@@ -1 +1 @@\n",
                            "-a\n",
                            "+A\n"
                        ),
                        concat!(
                            "@@ -5,1 +5,1 @@\n",
                            "-e\n",
                            "+E\n"
                        ),
                    ],
                },
            ]),
        ),
        (
            "two files keep the input order",
            json!(concat!(
                "--- /b.md\n",
                "+++ /b.md\n",
                "@@ -1 +1 @@\n",
                "-b\n",
                "+B\n",
                "--- /a.md\n",
                "+++ /a.md\n",
                "@@ -1 +1 @@\n",
                "-a\n",
                "+A\n"
            )),
            json!([
                {
                    "path": "/b.md",
                    "kind": "modify",
                    "hunks": [
                        concat!(
                            "@@ -1 +1 @@\n",
                            "-b\n",
                            "+B\n"
                        ),
                    ],
                },
                {
                    "path": "/a.md",
                    "kind": "modify",
                    "hunks": [
                        concat!(
                            "@@ -1 +1 @@\n",
                            "-a\n",
                            "+A\n"
                        ),
                    ],
                },
            ]),
        ),
        (
            "--- /dev/null is a create",
            json!(concat!(
                "--- /dev/null\n",
                "+++ b/new.md\n",
                "@@ -0,0 +1,2 @@\n",
                "+one\n",
                "+two\n"
            )),
            json!([
                {
                    "path": "/new.md",
                    "kind": "create",
                    "hunks": [
                        concat!(
                            "@@ -0,0 +1,2 @@\n",
                            "+one\n",
                            "+two\n"
                        ),
                    ],
                },
            ]),
        ),
        (
            "+++ /dev/null is a remove",
            json!(concat!(
                "--- a/old.md\n",
                "+++ /dev/null\n",
                "@@ -1,1 +0,0 @@\n",
                "-gone\n"
            )),
            json!([
                {
                    "path": "/old.md",
                    "kind": "remove",
                    "hunks": [
                        concat!(
                            "@@ -1,1 +0,0 @@\n",
                            "-gone\n"
                        ),
                    ],
                },
            ]),
        ),
        (
            "a remove needs no hunk",
            json!(concat!("--- a/old.md\n", "+++ /dev/null\n")),
            json!([{"path": "/old.md", "kind": "remove", "hunks": []}]),
        ),
        (
            "a path without prefix or leading slash becomes absolute",
            json!(concat!(
                "--- docs/y.md\n",
                "+++ docs/y.md\n",
                "@@ -1 +1 @@\n",
                "-a\n",
                "+b\n"
            )),
            json!([
                {
                    "path": "/docs/y.md",
                    "kind": "modify",
                    "hunks": [
                        concat!(
                            "@@ -1 +1 @@\n",
                            "-a\n",
                            "+b\n"
                        ),
                    ],
                },
            ]),
        ),
        (
            "a timestamp after a tab leaves the header",
            json!(concat!(
                "--- /t.md\t2026-09-29 10:00:00.000000000 +0200\n",
                "+++ /t.md\t2026-09-29 10:01:00.000000000 +0200\n",
                "@@ -1 +1 @@\n",
                "-a\n",
                "+b\n"
            )),
            json!([
                {
                    "path": "/t.md",
                    "kind": "modify",
                    "hunks": [
                        concat!(
                            "@@ -1 +1 @@\n",
                            "-a\n",
                            "+b\n"
                        ),
                    ],
                },
            ]),
        ),
        (
            "diff --git and index lines are skipped",
            json!(concat!(
                "diff --git a/g.md b/g.md\n",
                "index 1234567..89abcde 100644\n",
                "--- a/g.md\n",
                "+++ b/g.md\n",
                "@@ -1 +1 @@\n",
                "-a\n",
                "+b\n"
            )),
            json!([
                {
                    "path": "/g.md",
                    "kind": "modify",
                    "hunks": [
                        concat!(
                            "@@ -1 +1 @@\n",
                            "-a\n",
                            "+b\n"
                        ),
                    ],
                },
            ]),
        ),
        (
            "a removed '-- x' and an added '++ x' read like a header but stay in the hunk",
            json!(concat!(
                "--- /d.md\n",
                "+++ /d.md\n",
                "@@ -1,2 +1,2 @@\n",
                "--- x\n",
                "+++ x\n",
                " keep\n"
            )),
            json!([
                {
                    "path": "/d.md",
                    "kind": "modify",
                    "hunks": [
                        concat!(
                            "@@ -1,2 +1,2 @@\n",
                            "--- x\n",
                            "+++ x\n",
                            " keep\n"
                        ),
                    ],
                },
            ]),
        ),
        (
            "a no-newline marker stays in the hunk, inside and at its end",
            json!(concat!(
                "--- /n.md\n",
                "+++ /n.md\n",
                "@@ -1 +1 @@\n",
                "-a\n",
                "\\ No newline at end of file\n",
                "+b\n",
                "\\ No newline at end of file\n"
            )),
            json!([
                {
                    "path": "/n.md",
                    "kind": "modify",
                    "hunks": [
                        concat!(
                            "@@ -1 +1 @@\n",
                            "-a\n",
                            "\\ No newline at end of file\n",
                            "+b\n",
                            "\\ No newline at end of file\n"
                        ),
                    ],
                },
            ]),
        ),
        (
            "a bare empty line counts as an empty context line",
            json!(concat!(
                "--- /e.md\n",
                "+++ /e.md\n",
                "@@ -1,3 +1,3 @@\n",
                " a\n",
                "\n",
                "-c\n",
                "+C\n"
            )),
            json!([
                {
                    "path": "/e.md",
                    "kind": "modify",
                    "hunks": [
                        concat!(
                            "@@ -1,3 +1,3 @@\n",
                            " a\n",
                            " \n",
                            "-c\n",
                            "+C\n"
                        ),
                    ],
                },
            ]),
        ),
    ];
    check("[split_patch(d) for d in ARGS]", &table);
}

/// `split_patch` raises ValueError on anything that is not a clean diff;
/// the probe names the exception so a crash of another kind shows.
#[test]
fn split_patch_refuses_anything_but_a_clean_diff() {
    if !shipped() {
        return;
    }
    let table: Vec<(&str, Value, Value)> = vec![
        (
            "control: a clean diff passes",
            json!(concat!(
                "--- /x.md\n",
                "+++ /x.md\n",
                "@@ -1 +1 @@\n",
                "-a\n",
                "+b\n"
            )),
            json!("ok"),
        ),
        (
            "a rename is not a patch",
            json!(concat!(
                "--- a/x.md\n",
                "+++ b/y.md\n",
                "@@ -1 +1 @@\n",
                "-a\n",
                "+b\n"
            )),
            json!("ValueError"),
        ),
        (
            "a hunk before any header",
            json!(concat!("@@ -1 +1 @@\n", "-a\n", "+b\n")),
            json!("ValueError"),
        ),
        (
            "a hunk shorter than its header",
            json!(concat!(
                "--- /x.md\n",
                "+++ /x.md\n",
                "@@ -1,2 +1,2 @@\n",
                "-a\n",
                "+b\n"
            )),
            json!("ValueError"),
        ),
        (
            "a hunk longer than its header",
            json!(concat!(
                "--- /x.md\n",
                "+++ /x.md\n",
                "@@ -1,1 +1,1 @@\n",
                "-a\n",
                "-b\n",
                "+c\n"
            )),
            json!("ValueError"),
        ),
        (
            "a line left over after a full hunk",
            json!(concat!(
                "--- /x.md\n",
                "+++ /x.md\n",
                "@@ -1 +1 @@\n",
                "-a\n",
                "+b\n",
                "+c\n"
            )),
            json!("ValueError"),
        ),
        (
            "a create hunk that removes lines",
            json!(concat!(
                "--- /dev/null\n",
                "+++ /n.md\n",
                "@@ -1,1 +1,1 @@\n",
                "-a\n",
                "+b\n"
            )),
            json!("ValueError"),
        ),
        (
            "a remove hunk that adds lines",
            json!(concat!(
                "--- /x.md\n",
                "+++ /dev/null\n",
                "@@ -1 +1 @@\n",
                "-a\n",
                "+b\n"
            )),
            json!("ValueError"),
        ),
        (
            "the same file twice",
            json!(concat!(
                "--- /x.md\n",
                "+++ /x.md\n",
                "@@ -1 +1 @@\n",
                "-a\n",
                "+b\n",
                "--- /x.md\n",
                "+++ /x.md\n",
                "@@ -3 +3 @@\n",
                "-c\n",
                "+d\n"
            )),
            json!("ValueError"),
        ),
        (
            "/dev/null on both sides",
            json!(concat!("--- /dev/null\n", "+++ /dev/null\n")),
            json!("ValueError"),
        ),
        (
            "a modify without a hunk",
            json!(concat!("--- /x.md\n", "+++ /x.md\n")),
            json!("ValueError"),
        ),
        (
            "a line outside any diff",
            json!("hello\n"),
            json!("ValueError"),
        ),
    ];
    check(CATCH, &table);
}

/// `created_text`: the `+` lines; a no-newline marker drops the last line end.
#[test]
fn created_text_is_the_added_lines_of_a_new_file() {
    if !shipped() {
        return;
    }
    let table: Vec<(&str, Value, Value)> = vec![
        (
            "the + lines of a create",
            json!(concat!(
                "--- /dev/null\n",
                "+++ /n.md\n",
                "@@ -0,0 +1,2 @@\n",
                "+one\n",
                "+two\n"
            )),
            json!(concat!("one\n", "two\n")),
        ),
        (
            "a no-newline marker drops the last line end",
            json!(concat!(
                "--- /dev/null\n",
                "+++ /n.md\n",
                "@@ -0,0 +1,2 @@\n",
                "+one\n",
                "+two\n",
                "\\ No newline at end of file\n"
            )),
            json!("one\ntwo"),
        ),
        (
            "an added empty line stays",
            json!(concat!(
                "--- /dev/null\n",
                "+++ /n.md\n",
                "@@ -0,0 +1,3 @@\n",
                "+a\n",
                "+\n",
                "+b\n"
            )),
            json!(concat!("a\n", "\n", "b\n")),
        ),
    ];
    check("[created_text(split_patch(d)[0]) for d in ARGS]", &table);
}

/// `commit_plan`: conflict > tombstoned > base_moved > behind > plan, sorted by
/// file id (the lock order); files with nothing to commit fall out.
#[test]
fn commit_plan_ranks_conflict_tombstoned_moved_behind_plan() {
    if !shipped() {
        return;
    }
    let table: Vec<(&str, Value, Value)> = vec![
        (
            "clean: head is base, the working version commits",
            json!({
                "rows": [{"file": "fh-a", "state": "touched", "base": "v1", "working": "v2"}],
                "heads": {"fh-a": {"head": "v1"}},
            }),
            json!({"plan": [{"file": "fh-a", "from": "v1", "to": "v2", "kind": "modify"}]}),
        ),
        (
            "the plan is sorted by file id, the lock order",
            json!({
                "rows": [
                    {"file": "fh-c", "state": "touched", "base": "c1", "working": "c2"},
                    {"file": "fh-a", "state": "touched", "base": "a1", "working": "a2"},
                    {"file": "fh-b", "state": "touched", "base": "b1", "working": "b2"},
                ],
                "heads": {"fh-a": {"head": "a1"}, "fh-b": {"head": "b1"}, "fh-c": {"head": "c1"}},
            }),
            json!({
                "plan": [
                    {"file": "fh-a", "from": "a1", "to": "a2", "kind": "modify"},
                    {"file": "fh-b", "from": "b1", "to": "b2", "kind": "modify"},
                    {"file": "fh-c", "from": "c1", "to": "c2", "kind": "modify"},
                ],
            }),
        ),
        (
            "behind: the head moved past the base",
            json!({
                "rows": [{"file": "fh-a", "state": "touched", "base": "v1", "working": "v2"}],
                "heads": {"fh-a": {"head": "v3"}},
            }),
            json!({"behind": ["fh-a"]}),
        ),
        (
            "conflict: only the conflicts, even beside a file behind",
            json!({
                "rows": [
                    {"file": "fh-b", "state": "conflict", "base": "v1", "working": "v2"},
                    {"file": "fh-a", "state": "touched", "base": "v1", "working": "v2"},
                ],
                "heads": {"fh-a": {"head": "v0"}, "fh-b": {"head": "v1"}},
            }),
            json!({"conflict": ["fh-b"]}),
        ),
        (
            "conflict outranks tombstoned",
            json!({
                "rows": [
                    {"file": "fh-b", "state": "conflict", "base": "v1", "working": "v2"},
                    {"file": "fh-a", "state": "touched", "base": "v1", "working": "v2"},
                ],
                "heads": {
                    "fh-a": {"head": "v1", "tomb": "2026-09-29T10:00:00.000000Z"},
                    "fh-b": {"head": "v1"},
                },
            }),
            json!({"conflict": ["fh-b"]}),
        ),
        (
            "tombstoned: the main line removed a touched file",
            json!({
                "rows": [{"file": "fh-a", "state": "touched", "base": "v1", "working": "v2"}],
                "heads": {"fh-a": {"head": "v1", "tomb": "2026-09-29T10:00:00.000000Z"}},
            }),
            json!({"tombstoned": ["fh-a"]}),
        ),
        (
            "tombstoned outranks behind",
            json!({
                "rows": [
                    {"file": "fh-a", "state": "touched", "base": "v1", "working": "v2"},
                    {"file": "fh-b", "state": "touched", "base": "v1", "working": "v2"},
                ],
                "heads": {
                    "fh-a": {"head": "v1", "tomb": "2026-09-29T10:00:00.000000Z"},
                    "fh-b": {"head": "v0"},
                },
            }),
            json!({"tombstoned": ["fh-a"]}),
        ),
        (
            "behind outranks a clean plan",
            json!({
                "rows": [
                    {"file": "fh-a", "state": "touched", "base": "v1", "working": "v2"},
                    {"file": "fh-b", "state": "touched", "base": "v1", "working": "v2"},
                ],
                "heads": {"fh-a": {"head": "v1"}, "fh-b": {"head": "v0"}},
            }),
            json!({"behind": ["fh-b"]}),
        ),
        (
            "created: from nothing to the working version",
            json!({
                "rows": [{"file": "fh-n", "state": "created", "base": "", "working": "v9"}],
                "heads": {},
            }),
            json!({"plan": [{"file": "fh-n", "from": "", "to": "v9", "kind": "create"}]}),
        ),
        (
            "removed: from the head to nothing",
            json!({
                "rows": [{"file": "fh-a", "state": "removed", "base": "v1", "working": ""}],
                "heads": {"fh-a": {"head": "v1"}},
            }),
            json!({"plan": [{"file": "fh-a", "from": "v1", "to": "", "kind": "remove"}]}),
        ),
        (
            "removed, but the main line changed it since: base_moved",
            json!({
                "rows": [{"file": "fh-a", "state": "removed", "base": "v1", "working": ""}],
                "heads": {"fh-a": {"head": "v2"}},
            }),
            json!({"base_moved": ["fh-a"]}),
        ),
        (
            "base_moved outranks behind",
            json!({
                "rows": [
                    {"file": "fh-a", "state": "removed", "base": "v1", "working": ""},
                    {"file": "fh-b", "state": "touched", "base": "b1", "working": "b2"},
                ],
                "heads": {"fh-a": {"head": "v2"}, "fh-b": {"head": "b0"}},
            }),
            json!({"base_moved": ["fh-a"]}),
        ),
        (
            "tombstoned outranks base_moved",
            json!({
                "rows": [
                    {"file": "fh-a", "state": "removed", "base": "v1", "working": ""},
                    {"file": "fh-b", "state": "touched", "base": "b1", "working": "b2"},
                ],
                "heads": {
                    "fh-a": {"head": "v2"},
                    "fh-b": {"head": "b1", "tomb": "2026-09-29T10:00:00.000000Z"},
                },
            }),
            json!({"tombstoned": ["fh-b"]}),
        ),
        (
            "removed, born here: falls out",
            json!({
                "rows": [{"file": "fh-n", "state": "removed", "base": "", "working": "v9"}],
                "heads": {},
            }),
            json!({"plan": []}),
        ),
        (
            "removed, already buried by the main line: falls out",
            json!({
                "rows": [{"file": "fh-a", "state": "removed", "base": "v1", "working": ""}],
                "heads": {"fh-a": {"head": "v1", "tomb": "2026-09-29T10:00:00.000000Z"}},
            }),
            json!({"plan": []}),
        ),
        (
            "touched, working is base: nothing to commit",
            json!({
                "rows": [{"file": "fh-a", "state": "touched", "base": "v1", "working": "v1"}],
                "heads": {"fh-a": {"head": "v1"}},
            }),
            json!({"plan": []}),
        ),
        (
            "no rows: an empty plan",
            json!({"rows": [], "heads": {}}),
            json!({"plan": []}),
        ),
        (
            "create, modify and remove in one plan, sorted",
            json!({
                "rows": [
                    {"file": "fh-c", "state": "removed", "base": "c1", "working": ""},
                    {"file": "fh-a", "state": "created", "base": "", "working": "a1"},
                    {"file": "fh-b", "state": "touched", "base": "b1", "working": "b2"},
                ],
                "heads": {"fh-b": {"head": "b1"}, "fh-c": {"head": "c1"}},
            }),
            json!({
                "plan": [
                    {"file": "fh-a", "from": "", "to": "a1", "kind": "create"},
                    {"file": "fh-b", "from": "b1", "to": "b2", "kind": "modify"},
                    {"file": "fh-c", "from": "c1", "to": "", "kind": "remove"},
                ],
            }),
        ),
    ];
    check("[commit_plan(a['rows'], a['heads']) for a in ARGS]", &table);
}

/// `recover_action`: committed finishes, aborted aborts, prepared aborts
/// strictly after its deadline (int, digit string or ISO; unreadable has
/// passed), anything else waits.
#[test]
fn recover_action_aborts_only_strictly_after_the_deadline() {
    if !shipped() {
        return;
    }
    let table: Vec<(&str, Value, Value)> = vec![
        (
            "committed: finish",
            json!({"row": {"id": "c-1", "state": "committed", "deadline": D}, "now": D + 5}),
            json!("finish"),
        ),
        (
            "committed before its deadline: still finish",
            json!({"row": {"id": "c-1", "state": "committed", "deadline": D}, "now": D - 5}),
            json!("finish"),
        ),
        (
            "aborted: abort",
            json!({"row": {"id": "c-1", "state": "aborted", "deadline": D}, "now": D - 5}),
            json!("abort"),
        ),
        (
            "prepared, int deadline, one ms late: abort",
            json!({"row": {"id": "c-1", "state": "prepared", "deadline": D}, "now": D + 1}),
            json!("abort"),
        ),
        (
            "prepared, int deadline, exactly on it: wait",
            json!({"row": {"id": "c-1", "state": "prepared", "deadline": D}, "now": D}),
            json!("wait"),
        ),
        (
            "prepared, int deadline, one ms early: wait",
            json!({"row": {"id": "c-1", "state": "prepared", "deadline": D}, "now": D - 1}),
            json!("wait"),
        ),
        (
            "prepared, digit-string deadline, one ms late: abort",
            json!({
                "row": {"id": "c-1", "state": "prepared", "deadline": D.to_string()},
                "now": D + 1,
            }),
            json!("abort"),
        ),
        (
            "prepared, digit-string deadline, exactly on it: wait",
            json!({"row": {"id": "c-1", "state": "prepared", "deadline": D.to_string()}, "now": D}),
            json!("wait"),
        ),
        (
            "prepared, ISO deadline, one ms late: abort",
            json!({"row": {"id": "c-1", "state": "prepared", "deadline": ISO}, "now": D + 1}),
            json!("abort"),
        ),
        (
            "prepared, ISO deadline, exactly on it: wait",
            json!({"row": {"id": "c-1", "state": "prepared", "deadline": ISO}, "now": D}),
            json!("wait"),
        ),
        (
            "prepared, ISO deadline, one ms early: wait",
            json!({"row": {"id": "c-1", "state": "prepared", "deadline": ISO}, "now": D - 1}),
            json!("wait"),
        ),
        (
            "prepared, unreadable deadline: passed, abort",
            json!({"row": {"id": "c-1", "state": "prepared", "deadline": "soon"}, "now": D}),
            json!("abort"),
        ),
        (
            "prepared, no deadline: passed, abort",
            json!({"row": {"id": "c-1", "state": "prepared"}, "now": D}),
            json!("abort"),
        ),
        (
            "prepared, a bool deadline: unreadable, abort",
            json!({"row": {"id": "c-1", "state": "prepared", "deadline": true}, "now": D}),
            json!("abort"),
        ),
        (
            "an unknown state: wait",
            json!({"row": {"id": "c-1", "state": "weird", "deadline": D}, "now": D + 1}),
            json!("wait"),
        ),
        (
            "an empty state: wait",
            json!({"row": {"id": "c-1", "state": "", "deadline": D}, "now": D + 1}),
            json!("wait"),
        ),
        (
            "no row at all: wait",
            json!({"row": null, "now": D + 1}),
            json!("wait"),
        ),
    ];
    check("[recover_action(a['row'], a['now']) for a in ARGS]", &table);
}

/// `stamp_ms` writes the cell's ISO stamp; `to_ms` reads it, a digit string
/// and the ISO text back to the same ms.
#[test]
fn stamp_ms_and_to_ms_round_trip_one_instant() {
    if !shipped() {
        return;
    }
    let table: Vec<(&str, Value, Value)> = vec![(
        "stamp_ms and to_ms round-trip one instant",
        json!([D, ISO]),
        json!([ISO, D, D, D]),
    )];
    let probe = r#"[[stamp_ms(x), to_ms(stamp_ms(x)), to_ms(str(x)), to_ms(i)] for x, i in ARGS]"#;
    check(probe, &table);
}

/// OR-FH-85: `line` is append-only. A visitor of a recovery that lost the
/// claim on a file (`rows_affected` 0 on `r-cl:<file>`) after it had written
/// its own `line` row (`r-l:<file>|<seq>`) leaves that row standing -- the
/// last stage issues no store op on `line`, and nothing for the lost file:
/// no `in_derive`, not counted as finished. The won file is finished once.
#[test]
fn a_lost_claim_leaves_its_line_row_standing() {
    if !shipped() {
        return;
    }
    let plan = json!([
        {"file": "fh-0000000000a1", "from": "a".repeat(64), "to": "b".repeat(64), "kind": "modify"},
        {"file": "fh-0000000000a2", "from": "c".repeat(64), "to": "d".repeat(64), "kind": "modify"}
    ]);
    let lg = json!({
        "r-c": {"payload": [{"commit": "c-race", "ws": "ws-1", "state": "committed",
                             "plan": plan.to_string(), "note": "",
                             "deadline": "2026-01-01T00:00:00.000000Z"}],
                "rows_affected": 0, "error_code": ""},
        "r-l:fh-0000000000a1|5000": {"payload": [], "rows_affected": 1, "error_code": ""},
        "r-l:fh-0000000000a2|6000": {"payload": [], "rows_affected": 1, "error_code": ""},
        "r-cl:fh-0000000000a1": {"payload": [], "rows_affected": 0, "error_code": ""},
        "r-cl:fh-0000000000a2": {"payload": [], "rows_affected": 1, "error_code": ""}
    });
    let got = pure(
        "ws",
        "(lambda r: {'line_ops': [o for o in r[0] if o[1].get('table') == 'line'], \
          'ops': len(r[0]), 'next': r[1], \
          'derive': [m['file'] for m in r[2]], 'done': [p['file'] for p in r[3]]}) \
         (rec_step('r3', 'c-race', ARGS))",
        lg,
    );
    assert_eq!(
        got,
        json!({"line_ops": [], "ops": 0, "next": null,
               "derive": ["fh-0000000000a2"], "done": ["fh-0000000000a2"]}),
        "the loser's line row stands (OR-FH-85)"
    );
}

/// OR-FH-95 (final review V I-1): the lock of a commit is the same CAS as
/// W's main-line swing -- `lock=''`, `head=from` AND `tomb=''`. W's `remove`
/// sets only `tomb` and leaves `head`, so a lock without `tomb=''` taken
/// after a `remove` that landed between reading the heads and locking would
/// still hit one row: the commit point passes, step 4 moves `head` on a
/// grave, the answer is `ok` and the change is silently gone. With `tomb=''`
/// the lock misses and the commit aborts as `busy`. The race is out of reach
/// of the sequential pump, so the ops are held here as a pure function.
#[test]
fn a_commit_locks_only_a_living_unmoved_file() {
    if !shipped() {
        return;
    }
    let a = "a".repeat(64);
    let b = "b".repeat(64);
    let c = "c".repeat(64);
    let table: Vec<(&str, Value, Value)> = vec![(
        "modify, create and remove each lock on lock='', head=from, tomb=''",
        json!({"C": "c-1", "plan": [
            {"file": "fh-0000000000a1", "from": a, "to": b, "kind": "modify"},
            {"file": "fh-0000000000a2", "from": "", "to": c, "kind": "create"},
            {"file": "fh-0000000000a3", "from": c, "to": "", "kind": "remove"}
        ]}),
        json!([
            ["k:fh-0000000000a1", {"operation": "update", "table": "files",
                                   "set": {"lock": "c-1"},
                                   "where": {"file": "fh-0000000000a1", "lock": "",
                                             "head": a, "tomb": ""}}],
            ["k:fh-0000000000a2", {"operation": "update", "table": "files",
                                   "set": {"lock": "c-1"},
                                   "where": {"file": "fh-0000000000a2", "lock": "",
                                             "head": "", "tomb": ""}}],
            ["k:fh-0000000000a3", {"operation": "update", "table": "files",
                                   "set": {"lock": "c-1"},
                                   "where": {"file": "fh-0000000000a3", "lock": "",
                                             "head": c, "tomb": ""}}]
        ]),
    )];
    check(
        "[[list(o) for o in lock_ops(a['C'], a['plan'])] for a in ARGS]",
        &table,
    );
}
