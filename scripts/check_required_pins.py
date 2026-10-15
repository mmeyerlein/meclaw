#!/usr/bin/env python3
"""Gate for the required pins (`.config/required-pins.toml`, GH #1199).

A required pin is a test that must run whenever a file is touched. The table
is the one place the mapping file -> test is written down; the resolver
(`scripts/gate_plan.py`, rule 12 of `test_filter`) plans the pinned tests for
every touched path a row's `paths` glob matches, whatever the coverage map
says. The prose that asks for a pin -- AGENTS.md, docs/development-rules.md --
names it by a marker `(required-pin: <id>)` and carries no copy of the
mapping. This script holds the three together:

    (a) every marker id has a row, and every row has a marker
        (no orphan on either side);
    (b) every pinned test exists: the file is a test unit of the tree, and
        the named function is a test in that file;
    and every row is well formed: an id, a `paths` glob list, a `tests`
    list and a `why` sentence, the id used once.

Each finding is one line `path:line: message`; exit 0 clean, 1 on a finding.

TWO TREE SHAPES. AGENTS.md and docs/development-rules.md do not travel to the
published tree, the table, the resolver and `crates/` do. Where neither
document exists, half (a) is SKIPPED and said so by name; half (b) runs in
both trees.

Usage:  python3 scripts/check_required_pins.py [--check] [--repo DIR]
        (`--check` is accepted for symmetry with the other anchor gates)
"""

import argparse
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import gate_plan  # noqa: E402

TABLE = gate_plan.REQUIRED_PINS
MARKER_DOCS = ("AGENTS.md", "docs/development-rules.md")
MARKER_RE = re.compile(r"\(required-pin: ([a-z0-9][a-z0-9-]*)\)")
ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]*$")
TEST_ATTR_RE = re.compile(r"^\s*#\[(?:[A-Za-z_][A-Za-z0-9_]*::)*test\b")


def _read(path):
    try:
        with open(path, encoding="utf-8") as fh:
            return fh.read()
    except OSError:
        return None


def markers(root):
    """`({id: [(doc, line)]}, docs read)` over the marker documents present."""
    found, read = {}, []
    for doc in MARKER_DOCS:
        text = _read(os.path.join(root, doc))
        if text is None:
            continue
        read.append(doc)
        for n, line in enumerate(text.splitlines(), 1):
            for m in MARKER_RE.finditer(line):
                found.setdefault(m.group(1), []).append((doc, n))
    return found, read


def _row_line(table_text, pid):
    """The line of `id = "<pid>"` in the table, else 1."""
    for n, line in enumerate((table_text or "").splitlines(), 1):
        if re.match(r'^\s*id\s*=\s*"%s"\s*$' % re.escape(str(pid)), line):
            return n
    return 1


def _fn_is_test(text, name):
    """`(found, is_test)`: does `fn <name>` stand in `text`, and does an
    attribute right above it (doc comments and attributes in between) name
    `test` -- `#[test]`, `#[tokio::test(...)]`?"""
    lines = text.splitlines()
    fn_re = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+%s\s*[<(]" % re.escape(name))
    for i, line in enumerate(lines):
        if not fn_re.match(line):
            continue
        j = i - 1
        while j >= 0:
            above = lines[j].strip()
            if TEST_ATTR_RE.match(above):
                return True, True
            if not (above.startswith("#[") or above.startswith("//") or above.endswith(")]")):
                break
            j -= 1
        return True, False
    return False, False


def check_test(root, test, where):
    """Findings of one `[[pin.tests]]` entry (half b)."""
    if not isinstance(test, dict) or not isinstance(test.get("file"), str):
        return ["%s: a test entry needs `file`" % where]
    rel, name = test["file"], test.get("fn")
    if gate_plan.required_pin_term(test, root) is None:
        return ["%s: %s%s is no test unit of the tree (crates/<c>/tests/...)"
                % (where, rel, " fn %s" % name if name else "")]
    text = _read(os.path.join(root, rel)) or ""
    if name is None:
        if not any(TEST_ATTR_RE.match(line) for line in text.splitlines()):
            return ["%s: %s holds no test" % (where, rel)]
        return []
    found, is_test = _fn_is_test(text, name)
    if not found:
        return ["%s: %s has no fn %s" % (where, rel, name)]
    if not is_test:
        return ["%s: fn %s in %s is no test" % (where, name, rel)]
    return []


def check(root):
    """`(findings, notes)` for the tree at `root`."""
    findings, notes = [], []
    table_text = _read(os.path.join(root, TABLE))
    if table_text is None:
        return ["%s:1: the table is missing" % TABLE], notes
    try:
        rows = gate_plan.required_pins(root)
    except gate_plan.GatePlanError as exc:
        return ["%s:1: %s" % (TABLE, exc)], notes
    ids = {}
    for row in rows:
        pid = row.get("id")
        line = _row_line(table_text, pid)
        where = "%s:%d" % (TABLE, line)
        if not (isinstance(pid, str) and ID_RE.match(pid)):
            findings.append("%s: `id` must be lowercase words joined by hyphens (got %r)"
                            % (where, pid))
            continue
        if pid in ids:
            findings.append("%s: id %s is used twice (first at line %d)" % (where, pid, ids[pid]))
        ids.setdefault(pid, line)
        paths = row.get("paths")
        if not (isinstance(paths, list) and paths and all(isinstance(p, str) and p for p in paths)):
            findings.append("%s: pin %s needs `paths`, a non-empty list of globs" % (where, pid))
        why = row.get("why")
        if not (isinstance(why, str) and why.strip()):
            findings.append("%s: pin %s needs `why`, one sentence" % (where, pid))
        tests = row.get("tests")
        if not (isinstance(tests, list) and tests):
            findings.append("%s: pin %s needs `tests`, at least one [[pin.tests]]" % (where, pid))
            continue
        for test in tests:
            findings.extend(check_test(root, test, "%s: pin %s" % (where, pid)))

    found, read = markers(root)
    if not read:
        notes.append("required-pins: skip the marker half -- %s absent (published tree)"
                     % ", ".join(MARKER_DOCS))
        return findings, notes
    for pid, places in sorted(found.items()):
        if pid not in ids:
            for doc, n in places:
                findings.append("%s:%d: marker (required-pin: %s) has no row in %s"
                                % (doc, n, pid, TABLE))
    for pid, line in sorted(ids.items()):
        if pid not in found:
            findings.append("%s:%d: pin %s has no marker (required-pin: %s) in %s"
                            % (TABLE, line, pid, pid, " / ".join(read)))
    return findings, notes


def main(argv=None):
    ap = argparse.ArgumentParser(prog="check_required_pins.py",
                                 description="Hold the required pins, their markers and the tree together.")
    ap.add_argument("--check", action="store_true", help="accepted for symmetry; the check is all it does")
    ap.add_argument("--repo", metavar="DIR", default=gate_plan.REPO_ROOT,
                    help="the tree to check (default: this repo)")
    args = ap.parse_args(argv)
    findings, notes = check(args.repo)
    for line in notes + findings:
        print(line)
    if findings:
        print("required-pins: %d finding(s)" % len(findings))
        return 1
    print("required-pins: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
