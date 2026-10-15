"""The browser locks in the coverage map (welle-tb S1b, #1165, owner's ruling 10.10.).

WHY THIS EXISTS
===============
The browser locks (`gate_plan.BROWSER_LOCKS`) are `#[ignore]`d in the tree, so the
map build -- `cargo nextest run --run-ignored default` -- never ran them, and the
map could not say which files a lock executes. Every integration therefore paid
for the colony half and every ignored lock, whatever the wave touched.

The owner's ruling (10.10.): no double or needless work; a full run when strange
effects show up, and at regular intervals. This module is the build half of it:

    cargo_selection()   the cargo targets of every lock (`-p <crate> --test <stem>`)
    nextest_filter()    the same locks as one nextest union (`binary_id(=c::stem) + ...`)
    marker(parts, junit)
                        the map field `browser_locks`: one entry per lock,
                        `measured` true only when the lock's profiles were merged
                        into a part with files AND its junit shows every test of
                        it green, not skipped, with stored output and without a
                        `SKIP ` line
    measured_line(cov), night_line(stdout)
                        `browser locks measured n/7` of a map, and of the map a
                        `coverage build` installed (the night report, point 4)

A lock that could not drive its browser leaves green with one `SKIP ` line
(`.config/nextest.toml`, GH #703/#867: "every way out without a measurement
prints one `SKIP ` line"); its profile then covers the early path only. That is
why the junit of the browser call stores the output of green tests (nextest
profile `coverage-browser`, `store-success-output`) and why a lock without that
output is never measured. Only `coverage aggregate` writes the field; a map
without it, or a lock without `measured: true`, is fail-closed for the planning
rule (`browser_plan.py`, point 2 of the ruling).

`coverage_host.sh` calls this file as a script: `select` and `filter` print the
two forms for the browser call and the main run's exclusion.
"""

import json
import os
import re
import sys
import xml.etree.ElementTree as ET

_SCRIPTS = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if _SCRIPTS not in sys.path:
    sys.path.insert(0, _SCRIPTS)

import gate_plan  # noqa: E402

FIELD = "browser_locks"
_SKIP = re.compile(r"(?m)^SKIP\b.*$")
_RED_TAGS = ("failure", "error", "flakyFailure", "flakyError", "rerunFailure", "rerunError")
# A skipped case measured nothing, stored output or not (re-check S1b m-3).
_SKIPPED_TAGS = ("skipped",)


def _stem(lock):
    return lock.rsplit("/", 1)[1].removesuffix(".rs")


def binary_of(lock):
    """`crates/<c>/tests/<stem>.rs` -> `<c>::<stem>` (the locks are standalone binaries)."""
    return "%s::%s" % (gate_plan._crate_of(lock), _stem(lock))


def cargo_selection(locks=None):
    """`-p <crate> --test <stem> ...` of the locks, grouped by crate, sorted."""
    by_crate = {}
    for lock in sorted(locks or gate_plan.BROWSER_LOCKS):
        by_crate.setdefault(gate_plan._crate_of(lock), []).append(_stem(lock))
    out = []
    for crate in sorted(by_crate):
        out += ["-p", crate]
        for stem in by_crate[crate]:
            out += ["--test", stem]
    return out


def nextest_filter(locks=None):
    """The locks as one nextest union of `binary_id(=...)` terms."""
    return " + ".join("binary_id(=%s)" % binary_of(l)
                      for l in sorted(locks or gate_plan.BROWSER_LOCKS))


def _junit_cases(path):
    """`{binary_id: [(name, red, stdout or None, skipped)]}` of a nextest junit, or None."""
    try:
        root = ET.parse(str(path)).getroot()
    except (OSError, ET.ParseError):
        return None
    out = {}
    for suite in root.iter("testsuite"):
        for case in suite.iter("testcase"):
            bid = suite.get("name") or case.get("classname") or ""
            red = any(case.find(t) is not None for t in _RED_TAGS)
            skipped = any(case.find(t) is not None for t in _SKIPPED_TAGS)
            so = case.find("system-out")
            out.setdefault(bid, []).append(
                (case.get("name") or "", red, so.text if so is not None else None, skipped))
    return out


def marker(parts, junit_path):
    """The map field `browser_locks` (see the module docstring)."""
    cases = _junit_cases(junit_path)
    merged = {p.get("binary") for p in parts or () if p.get("files")}
    out = {}
    for lock in sorted(gate_plan.BROWSER_LOCKS):
        bid = binary_of(lock)
        mine = (cases or {}).get(bid, [])
        why = None
        if cases is None:
            why = "no junit of the browser run"
        elif not mine:
            why = "not run"
        elif any(red for _n, red, _o, _s in mine):
            why = "red: %s" % ", ".join(n for n, red, _o, _s in mine if red)
        elif any(sk for _n, _r, _o, sk in mine):
            why = "skipped: %s" % ", ".join(n for n, _r, _o, sk in mine if sk)
        elif any(not (o or "").strip() for _n, _r, o, _s in mine):
            why = "no stored output: %s" % ", ".join(n for n, _r, o, _s in mine
                                                     if not (o or "").strip())
        else:
            skips = [m.group(0).strip() for _n, _r, o, _s in mine for m in _SKIP.finditer(o)]
            if skips:
                why = "; ".join(skips[:3])
            elif bid not in merged:
                why = "no merged profile"
        entry = {"binary": bid, "measured": why is None, "tests": len(mine)}
        if why:
            entry["why"] = why
        out[lock] = entry
    return out


def _measured(cov):
    """`(n, total, line)`: the locks the map's field measures (point 4, the night report)."""
    field = (cov or {}).get(FIELD)
    total = len(gate_plan.BROWSER_LOCKS)
    if not isinstance(field, dict):
        return 0, total, "browser locks measured 0/%d -- the map has no %s" % (total, FIELD)
    n = sum(1 for l in gate_plan.BROWSER_LOCKS
            if isinstance(field.get(l), dict) and field[l].get("measured") is True)
    open_ = ["%s (%s)" % (_stem(l), (field.get(l) or {}).get("why", "no entry"))
             for l in sorted(gate_plan.BROWSER_LOCKS)
             if not (isinstance(field.get(l), dict) and field[l].get("measured") is True)]
    return n, total, "browser locks measured %d/%d%s" % (
        n, total, (" -- not measured: " + "; ".join(open_)) if open_ else "")


def measured_line(cov):
    """`browser locks measured n/7[ -- not measured: <stem> (<why>); ...]`. An
    unmeasured lock is no red: the integration rule runs it (`browser_plan`)."""
    return _measured(cov)[2]


def night_line(stdout, map_dir=None):
    """`measured_line` of the map a `coverage build` installed, read from the
    `coverage_built` JSON it printed last (the one with `edges`); None when it
    printed none -- no map was installed."""
    from . import coverage as coverage_mod
    for raw in reversed((stdout or "").splitlines()):
        try:
            doc = json.loads(raw)
        except ValueError:
            continue
        if isinstance(doc, dict) and "rev" in doc and "edges" in doc:
            cov, why = coverage_mod.load_map(coverage_mod.map_path(doc["rev"], map_dir))
            return measured_line(cov) if cov is not None else "browser locks: %s" % why
    return None


def probe_report(cov):
    """`(exit, line)` of a `coverage build --scope browser` probe: 0 when every lock
    is measured, else 1 -- the probe never installs or journals its map."""
    n, total, line = _measured(cov)
    return (0 if n == total else 1), "coverage probe: " + line


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    if argv == ["select"]:
        print(" ".join(cargo_selection()))
        return 0
    if argv == ["filter"]:
        print(nextest_filter())
        return 0
    print("usage: browser_cover.py select|filter", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
