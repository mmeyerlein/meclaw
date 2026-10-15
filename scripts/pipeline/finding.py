"""Where a red station failed: the first finding of its log (GH #1150).

A `GATE <station> ... RED` line said red and nothing else; the cause was only
in the station log. `red_reason()` appends the first finding the log names --
a test name or `path:line` -- and the log path relative to the run archive,
so a reader (or a builder) goes straight to the place. Pure text work on the
log the station already wrote; no station kind is special-cased elsewhere.
"""
import os
import re

# Ordered by how precisely a hit names the place; the first kind found wins.
# Each entry: (regex, how the hit is written: "g1", "g2" or "g1:g2").
_KINDS = (
    # unittest: `FAIL: test_x (scripts.tests.mod.Class.test_x)`
    (re.compile(r"^(?:FAIL|ERROR): \S+ \(([\w.]+)\)"), "g1"),
    # nextest: `        FAIL [   0.123s] crate::bin some::test`; nextest >= 0.9.146 puts a
    # padded progress counter `( 3232/10378)` before the binary -- without the
    # optional group the label read `at ( 3232/10378)` and a counter with more
    # than one pad space never matched (#1164, Z M-INT-F gate.out:294). `(?!\()`:
    # a counter with no name after it is no finding (re-check recheck-tb-zlabel.md).
    (re.compile(r"^\s*(?:FAIL|TIMEOUT|SIGSEGV|SIGABRT|SIGKILL) \[\s*[\d.]+s\] "
                r"(?:\(\s*\d+/\d+\) )?(?!\()(\S+ \S+)"), "g1"),
    # rustc / clippy: `  --> crates/x/src/lib.rs:12:5`
    (re.compile(r"^\s*--> (\S+:\d+(?::\d+)?)"), "g1"),
    # a pin of a self-test: `FAIL some_pin_name   <<< ...`
    (re.compile(r"^\s*FAIL (\w+)\b"), "g1"),
    # a drift check: `CHANGED <path>`, the line from the next `@@ -<line>`
    (re.compile(r"^\s*CHANGED\s+(\S+)"), "hunk"),
    (re.compile(r"^\s*DRIFT\s+(\S+)"), "g1"),
    # a Python traceback: `File "<path>", line <n>`
    (re.compile(r'File "([^"]+)", line (\d+)'), "g1:g2"),
    # anything else that looks like `path:line`
    (re.compile(r"(?<![\w/.-])((?:[\w.-]+/)*[\w.-]+\.(?:rs|py|sh|json|toml|md|ts|js)):(\d+)"), "g1:g2"),
)
_HUNK = re.compile(r"^\s*@@ -(\d+)")


_TB, _RUSTC = 6, 2  # the traceback and the rustc `-->` kinds in _KINDS
_LEVEL = re.compile(r"^(warning|error)(?:\[\w+\])?:")


def first_finding(log_path):
    """`test name` or `path:line` from the log, or '' (streamed; first traceback: innermost
    frame; a warning's `-->` ranks as a bare `path:line` -- review R3 M3)."""
    try:
        fh = open(log_path, encoding="utf-8", errors="replace")
    except OSError:
        return ""
    best, hunk_of = None, None  # (rank, text); the CHANGED path waiting for its hunk
    level, in_tb = "", False
    with fh:
        for line in fh:
            line = line.rstrip("\n")
            lv = _LEVEL.match(line)
            level = lv.group(1) if lv else level
            if line.startswith("Traceback (most recent call last)"):
                in_tb = best is None or best[0] > _TB
            elif in_tb and line and not line[0].isspace():
                in_tb = False
            if hunk_of:
                m = _HUNK.match(line)
                if m:
                    best, hunk_of = (best[0], "%s:%s" % (hunk_of, m.group(1))), None
                    continue
            for rank, (rx, form) in enumerate(_KINDS):
                again = rank == _TB and in_tb and best is not None and best[0] == _TB
                if best is not None and rank >= best[0] and not again:
                    break
                m = rx.search(line)
                if m:
                    if rank == _RUSTC and level == "warning":
                        rank = len(_KINDS) - 1
                        if best is not None and rank >= best[0]:
                            break
                    text = "%s:%s" % (m.group(1), m.group(2)) if form == "g1:g2" else m.group(1)
                    hunk_of = text if form == "hunk" else None
                    best = (rank, text)
                    break
    return best[1] if best else ""


def red_reason(reason, log_path):
    """The reason of a RED line with the first finding and the log path."""
    rel = os.path.join(os.path.basename(os.path.dirname(log_path)), os.path.basename(log_path))
    where = first_finding(log_path)
    parts = [p for p in (reason, ("at %s" % where) if where else "") if p]
    return "%s -- log %s" % ("; ".join(parts), rel) if parts else "log %s" % rel
