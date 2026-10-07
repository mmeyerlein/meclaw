#!/usr/bin/env python3
"""The slow lock: no test may use more than a third of its nextest budget.

    python3 scripts/slow_tests.py [--junit <path>] [--config <nextest.toml>]
                                  [--profile <name>] [--fraction <f>]

nextest kills a test after `slow-timeout.period x terminate-after`. A test that
needs a large share of that budget on a quiet machine is killed on a busy one:
the scenario test of the display hive took 213 s of its 240 s at 12 threads and
was killed at 24 (GH #1046) -- the budget was right, the test had grown into it
unseen, because nothing reads the times of green tests. This script reads them.

It reads the JUnit file nextest writes for the profile (`[profile.<p>.junit]`,
`<target>/nextest/<p>/junit.xml`) and the profile's budgets from
`.config/nextest.toml` -- the profile's own `slow-timeout`, overridden per test
by the first matching `[[profile.<p>.overrides]]` and then the default profile's
overrides, the order nextest itself applies. Every test above `fraction` of its
budget (default 1/3) is named with its seconds and the limit; exit 1 if any.
A test without a kill (`terminate-after` unset) has no budget and is not judged.

A known slow test is a debt, not an exception: `.config/slow-debt.toml` lists it
with the issue that pays it and the measurement behind it, the same discipline as
a retry entry of the nextest quarantine. Every run prints each debt it meets
(`SLOW-DEBT`), so it is never silent; an entry without an issue is red; an entry
whose test came in under the mark is printed as `SLOW-DEBT PAID?` -- the issue
may be done, the line should go.

Exit 0 clean, 1 a test over the mark, 2 no JUnit file, a filter this reader
does not understand or a debt without an issue -- all RED, never a silent pass:
a lock that cannot read its input has not looked.

The profile defaults to `MECLAW_TIER_PROFILE`, the variable `scripts/test-tier.sh`
reads, else `default`; the file is looked for in nextest's store
(`[store] dir`, default `target/nextest` under the workspace root).
"""

import argparse
import fnmatch
import os
import re
import sys
import tomllib
import xml.etree.ElementTree as ET

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# nextest's own default when a profile sets no slow-timeout at all.
DEFAULT_PERIOD = 60.0


class FilterError(ValueError):
    """A filterset outside the subset this reader understands."""


def seconds(text):
    """`30s`, `2m`, `1m 30s`, `500ms` -> seconds (humantime, as nextest reads it)."""
    total, rest = 0.0, text.replace(" ", "")
    for num, unit in re.findall(r"(\d+(?:\.\d+)?)(ms|s|m|h)", rest):
        total += float(num) * {"ms": 0.001, "s": 1, "m": 60, "h": 3600}[unit]
    if not re.fullmatch(r"(\d+(?:\.\d+)?(ms|s|m|h))+", rest):
        raise ValueError("no duration: %r" % text)
    return total


def budget_of(slow):
    """A `slow-timeout` value -> `(period, terminate_after or None)`."""
    if isinstance(slow, str):
        return seconds(slow), None
    return seconds(slow.get("period", "60s")), slow.get("terminate-after")


# --- the filterset subset of .config/nextest.toml --------------------------------
# binary_id(...), test(...), `&`/`and`, `+`/`|`/`or`, `not`, parentheses. Matchers:
# `=x` equal, `~x` contains, `/re/` regex, `#glob` glob; bare is a glob for
# binary_id and `contains` for test (nextest's defaults).

TOKEN = re.compile(r"\s*(binary_id|test)\(([^)]*)\)|\s*(\(|\)|&|\+|\||and\b|or\b|not\b)")


def _tokens(expr):
    pos, out = 0, []
    while pos < len(expr):
        if expr[pos:].strip() == "":
            break
        m = TOKEN.match(expr, pos)
        if not m:
            raise FilterError("cannot read the filter at %r: %s" % (expr[pos:pos + 30], expr))
        out.append(("fn", m.group(1), m.group(2).strip()) if m.group(1)
                   else ("op", m.group(3)))
        pos = m.end()
    return out


def _matcher(kind, raw):
    if raw.startswith("="):
        return lambda s: s == raw[1:]
    if raw.startswith("~"):
        return lambda s: raw[1:] in s
    if len(raw) >= 2 and raw.startswith("/") and raw.endswith("/"):
        rx = re.compile(raw[1:-1])
        return lambda s: rx.search(s) is not None
    if raw.startswith("#"):
        return lambda s: fnmatch.fnmatchcase(s, raw[1:])
    if kind == "binary_id":
        return lambda s: fnmatch.fnmatchcase(s, raw)
    return lambda s: raw in s


def compile_filter(expr):
    """A filterset -> `f(binary_id, test_name) -> bool`."""
    toks = _tokens(expr)
    pos = 0

    def peek():
        return toks[pos] if pos < len(toks) else None

    def take():
        nonlocal pos
        pos += 1
        return toks[pos - 1]

    def atom():
        t = take() if peek() else None
        if t is None:
            raise FilterError("the filter ends early: %s" % expr)
        if t == ("op", "not"):
            inner = atom()
            return lambda b, n: not inner(b, n)
        if t == ("op", "("):
            inner = union()
            if take() != ("op", ")"):
                raise FilterError("unbalanced parentheses: %s" % expr)
            return inner
        if t[0] == "fn":
            m = _matcher(t[1], t[2])
            return (lambda b, n: m(b)) if t[1] == "binary_id" else (lambda b, n: m(n))
        raise FilterError("unexpected %r in %s" % (t[1], expr))

    def inter():
        left = atom()
        while peek() in (("op", "&"), ("op", "and")):
            take()
            right, prev = atom(), left
            left = (lambda p, r: lambda b, n: p(b, n) and r(b, n))(prev, right)
        return left

    def union():
        left = inter()
        while peek() in (("op", "+"), ("op", "|"), ("op", "or")):
            take()
            right, prev = inter(), left
            left = (lambda p, r: lambda b, n: p(b, n) or r(b, n))(prev, right)
        return left

    out = union()
    if peek() is not None:
        raise FilterError("trailing %r in %s" % (peek()[1], expr))
    return out


# --- budgets ------------------------------------------------------------------------

def budgets(config, profile):
    """`f(binary_id, test_name) -> (period, terminate_after or None)` for a profile."""
    profiles = config.get("profile", {})
    own, base = profiles.get(profile, {}), profiles.get("default", {})
    slow = own.get("slow-timeout", base.get("slow-timeout"))
    fallback = budget_of(slow) if slow is not None else (DEFAULT_PERIOD, None)
    rules = []
    chain = [own] if profile == "default" else [own, base]
    for prof in chain:
        for ov in prof.get("overrides", []):
            if "slow-timeout" in ov:
                rules.append((compile_filter(ov["filter"]), budget_of(ov["slow-timeout"])))

    def lookup(binary_id, test):
        for match, budget in rules:
            if match(binary_id, test):
                return budget
        return fallback
    return lookup


def cases(junit):
    """`(binary_id, test_name, seconds)` for every test case of a nextest JUnit file."""
    for tc in ET.parse(junit).getroot().iter("testcase"):
        yield tc.get("classname", ""), tc.get("name", ""), float(tc.get("time") or 0)


def over_the_mark(junit, config, profile, fraction):
    lookup = budgets(config, profile)
    found = []
    for binary_id, test, secs in cases(junit):
        period, after = lookup(binary_id, test)
        if not after:
            continue
        limit = period * after * fraction
        if secs > limit:
            found.append((binary_id, test, secs, limit, period, after))
    return found


def debts(path):
    """`{"<binary_id> <test>": (issue, measured)}` from the debt file (none: empty)."""
    if not os.path.isfile(path):
        return {}
    with open(path, "rb") as f:
        rows = tomllib.load(f).get("debt", [])
    out = {}
    for row in rows:
        if not row.get("test") or not isinstance(row.get("issue"), int):
            raise FilterError("a debt needs `test` and an `issue` number: %r" % row)
        out[row["test"]] = (row["issue"], row.get("measured", ""))
    return out


def junit_path(config, profile):
    """Where nextest writes the profile's JUnit file: its STORE, not cargo's target.

    The store is `[store] dir`, default `target/nextest` under the workspace root
    -- also when `CARGO_TARGET_DIR` points elsewhere, as it does in a linked
    worktree and on a build lane (measured on the first gate of GH #1046: the
    file sat in `<tree>/target/nextest/default/`, not in the shared target)."""
    store = config.get("store", {}).get("dir", os.path.join("target", "nextest"))
    junit = config.get("profile", {}).get(profile, {}).get("junit", {}).get("path", "junit.xml")
    return os.path.join(ROOT, store, profile, junit)


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--profile", default=os.environ.get("MECLAW_TIER_PROFILE") or "default")
    ap.add_argument("--config", default=os.path.join(ROOT, ".config", "nextest.toml"))
    ap.add_argument("--junit", help="default: <root>/target/nextest/<profile>/junit.xml")
    ap.add_argument("--fraction", type=float, default=1 / 3)
    ap.add_argument("--debt", default=os.path.join(ROOT, ".config", "slow-debt.toml"))
    ap.add_argument("--print-junit-path", action="store_true",
                    help="print where the profile's JUnit file lands and exit "
                         "(`test-tier.sh` deletes it there before each run)")
    args = ap.parse_args(argv)

    with open(args.config, "rb") as f:
        config = tomllib.load(f)
    junit = args.junit or junit_path(config, args.profile)
    if args.print_junit_path:
        print(junit)
        return 0
    if not os.path.isfile(junit):
        print("SLOW-LOCK RED: no JUnit file at %s -- the profile `%s` needs "
              "`[profile.%s.junit]` in .config/nextest.toml and a nextest run before "
              "this station" % (junit, args.profile, args.profile))
        print("SLOW-VERDICT broken: no JUnit file")
        return 2
    try:
        found = over_the_mark(junit, config, args.profile, args.fraction)
        owed = debts(args.debt)
    except FilterError as e:
        print("SLOW-LOCK RED: %s" % e)
        print("SLOW-VERDICT broken: %s" % str(e).split(":")[0])
        return 2
    ran = {"%s %s" % (b, t) for b, t, _ in cases(junit)}
    binaries = {key.split(" ", 1)[0] for key in ran}
    red = 0
    paid_by = set()
    for binary_id, test, secs, limit, period, after in sorted(found, key=lambda x: -x[2]):
        key = "%s %s" % (binary_id, test)
        what = "%.1fs > %.0fs (1/%g of %gs x %d, profile %s)" % (
            secs, limit, round(1 / args.fraction, 3), period, after, args.profile)
        if key in owed:
            print("SLOW-DEBT %s %s -- GH #%d, measured %s"
                  % (key, what, owed[key][0], owed[key][1]))
            paid_by.add(owed[key][0])
            continue
        red += 1
        print("SLOW-LOCK %s %s" % (key, what))
    over = {"%s %s" % (b, t) for b, t, *_ in found}
    for key, (issue, _) in sorted(owed.items()):
        if key in ran and key not in over:
            print("SLOW-DEBT PAID? %s came in under the mark -- if GH #%d is done, "
                  "remove its line from %s" % (key, issue, os.path.relpath(args.debt, ROOT)))
    # A debt whose binary ran but whose test did not: renamed or deleted, and
    # its line would stay forever, silent (review M1 of GH #1046). Only in the
    # passes -- a strand's filter may run a binary's other tests and not this
    # one; a binary that did not run at all says nothing either way.
    unknown = 0
    if os.environ.get("MECLAW_GATE_MODE") in ("integration", "release"):
        for key, (issue, _) in sorted(owed.items()):
            if key not in ran and key.split(" ", 1)[0] in binaries:
                unknown += 1
                print("SLOW-DEBT UNKNOWN %s -- GH #%d: its binary ran, the test did "
                      "not; renamed or gone -- fix or remove its line in %s"
                      % (key, issue, os.path.relpath(args.debt, ROOT)))
    print("SLOW-LOCK %d of %d tests over the mark, %d of them owed (%s)"
          % (red, len(ran), len(found) - red, junit))
    # The last line is the reason of the GATE line (`scripts/gate.sh`), so the
    # debt stands in the summary and not only in this log (review I4).
    n_owed = len(found) - red
    verdict = "%d over 1/3, %s" % (red, "%d owed (%s)" % (
        n_owed, ", ".join("GH #%d" % i for i in sorted(paid_by))) if n_owed else "none owed")
    if unknown:
        verdict += ", %d unknown debt" % unknown
    print("SLOW-VERDICT %s" % verdict)
    return 1 if red or unknown else 0


if __name__ == "__main__":
    sys.exit(main())
