"""The flake register: isolation of red tests, runtime quarantine, issue duty (W8).

A red test is not yet a verdict. The executor isolates every red test of a
station at once (V0 section 3, `flake_isolated`): ten runs of that test alone.
Ten of ten green -> the station ends `FLAKY` (counts as green, V0 section 5);
anything less -> it stays `RED`, the test is a defect. The station's ONE
`station_ended` line comes after the isolations, before `pass_ended`.

Two halves:

* Lane side (pure functions, no store): reading JUnit / unittest output /
  scenario `last_run.json`, the isolation argv per kind, the fail-closed
  verdicts, `isolate_station` (the one hook between a station's end and its
  `station_ended`), `end_station` (the lines to emit), the rendered
  `quarantine.toml` the executor hands to nextest as a tool config file.
* Core side (store): `refresh` builds the `flakes` rows from the journal (a
  window of the last 20 observations per test), `ingest_run` turns every
  `flake_isolated` into a task with a failure guard -- and refuses a FLAKY
  test without a register row (`RegisterError`, the kern ends the pass
  ABORTED), `quarantine` /
  `release_expired` / `dispatch_quarantine` keep the quarantine as data with
  an expiry, `issue_for` files or comments the issue a flake owes -- behind
  four locks (fixed template, private-pattern scan, one issue per test, a
  daily cap), with drafts instead of issues whenever a lock bites or `gh`
  is missing.

Test keys (V0 section 2): nextest `<binary_id>::<name>`, unittest
`<module>.<Class>.<test>`, scenario `<station>:case:<id>` (a bare
`case:<id>` is the legacy form, read as its station key). The quarantine
never edits a tracked file: `.config/nextest.toml` stays the hand-kept list;
the runtime entries live in the store and in `ws/<run>/quarantine.toml` of
one run, and they take nextest keys only.
"""

import json
import os
import pathlib
import re
import sqlite3
import subprocess
import sys
import tomllib
import xml.etree.ElementTree as ET
from datetime import timedelta

from . import events as _events
from . import store as _store

REPO = pathlib.Path(__file__).resolve().parents[2]
DRAFTS_ENV = "MECLAW_FLAKE_DRAFTS"
ISSUES_ENV = "MECLAW_FLAKE_ISSUES"   # "0": the kern runs no issue duty (tests)

WINDOW = 20            # observations per test in the register window
RATE_THRESHOLD = 0.05  # auto quarantine from this flaky/runs rate on (with an issue)
QUARANTINE_DAYS = 14   # the longest a quarantine may run (VG TV-F3)
QUARANTINE_MAX = 8     # active entries at most; the ninth is refused (VG TV-F3)
QUARANTINE_FULL = "quarantine full"
FIX_QUIET = 20         # a measured fix: no red in this many quiet runs ...
FIX_LOAD = 20          # ... and in this many under load (VG TV-F6)
LEGACY_FILE = ".config/flaky-legacy.toml"
OBSERVATION_LABEL = "observation"
ISOLATION_CAP = 5      # every kind: more red tests than this is a defect, not a flake
ISOLATION_COUNT = 10
SLOW_S = 30
MAX_NEW_ISSUES = 5     # new issues per day; above: drafts

TOO_MANY = "too many red for isolation"
UNEXPLAINED = "unexplained red"
# The exit a station ends with when only tests failed: nextest TEST_RUN_FAILED
# (100; 101 is a build failure), unittest and the scenario runner 1.
TEST_FAILURE_EXIT = {"nextest": 100, "unittest": 1, "scenario": 1}
PENDING = "issue pending"
LATER_NOT_RUN = "not isolatable: later commands not run"
BUDGET_SPENT = "not isolatable: isolation budget spent"
TIMER_ACTIONS = ("paused", "relative", "confirm", "keep_real_time")

# The one known flake with a signature (GH #721, `registry lacks` in the
# builder scenarios): the same constants as the executor's BUILDER_FLAKE_*;
# a case red with this signature in ITS OWN error is isolated like any other and
# carries #721 -- the signature elsewhere in the log hands it to no other case.
GH721_STATION = "scenarios:builder"
GH721_SIGNATURE = "registry lacks"
GH721_ISSUE = 721

# --- reading results ----------------------------------------------------------------

_FLAKY_TAGS = {"flakyFailure", "flakyError", "rerunFailure", "rerunError"}


def _suites(path):
    root = ET.parse(path).getroot()
    return [root] if root.tag == "testsuite" else list(root.iter("testsuite"))


def _secs(tc):
    try:
        return float(tc.get("time") or 0)
    except ValueError:
        return 0.0


def failure_kind(tc):
    """How a failed test case failed: `assert`, `abort` (a signal), `timeout` or `error`.

    nextest writes `<failure type="test failure">` for a failing test,
    `type="test abort"` for a signal, `type="test timeout"` for a timeout and
    `<error>` when the test could not run at all."""
    if tc.find("error") is not None:
        return "error"
    kind = (tc.find("failure").get("type") or "").lower()
    if "abort" in kind or "signal" in kind:
        return "abort"
    if "timeout" in kind or "timed out" in kind:
        return "timeout"
    return "assert"


def junit_tests(path):
    """Every test case of a nextest JUnit file: {binary_id, name, outcome, secs}.

    The testsuite name is the binary id; `<failure>`/`<error>` is failed (with
    `failure`: its `failure_kind`), `<skipped>` skipped, a passing case with
    `<flakyFailure>`/`<rerun*>` is flaky (nextest retried it to green)."""
    out = []
    for suite in _suites(path):
        for tc in suite.iter("testcase"):
            tags = {child.tag for child in tc}
            if tags & {"failure", "error"}:
                outcome = "failed"
            elif "skipped" in tags:
                outcome = "skipped"
            elif tags & _FLAKY_TAGS:
                outcome = "flaky"
            else:
                outcome = "passed"
            t = {"binary_id": suite.get("name", ""), "name": tc.get("name", ""),
                 "outcome": outcome, "secs": round(_secs(tc), 3)}
            if outcome == "failed":
                t["failure"] = failure_kind(tc)
            out.append(t)
    return out


def slow_tests(path, threshold_s=SLOW_S):
    """The test cases slower than `threshold_s`, slowest first."""
    slow = [t for t in junit_tests(path) if t["secs"] > threshold_s]
    return sorted(slow, key=lambda t: -t["secs"])


def junit_parts(path):
    """The `test_result` bodies of one JUnit file, one per binary (V0 section 2.2).

    The executor's form plus `flaky_names` (additive): the register needs to
    know WHICH test nextest retried to green, not only how many; and
    `hard_failures` {name: abort|timeout|error} for the failures no isolation
    may turn green."""
    parts = {}
    for t in junit_tests(path):
        p = parts.setdefault(t["binary_id"], {
            "part": t["binary_id"], "secs": 0.0, "failed_names": [], "flaky_names": [],
            "tests": {"total": 0, "passed": 0, "failed": 0, "flaky": 0, "skipped": 0}})
        p["secs"] += t["secs"]
        p["tests"]["total"] += 1
        if t["outcome"] == "failed":
            p["tests"]["failed"] += 1
            p["failed_names"].append(t["name"])
            if t["failure"] != "assert":
                p.setdefault("hard_failures", {})[t["name"]] = t["failure"]
        elif t["outcome"] == "skipped":
            p["tests"]["skipped"] += 1
        else:
            p["tests"]["passed"] += 1
            if t["outcome"] == "flaky":
                p["tests"]["flaky"] += 1
                p["flaky_names"].append(t["name"])
    out = []
    for p in parts.values():
        p["secs"] = round(p["secs"], 3)
        p["verdict"] = "RED" if p["failed_names"] else "GREEN"
        for k in ("failed_names", "flaky_names"):
            if not p[k]:
                del p[k]
        out.append(p)
    return out


_UT_LINE = re.compile(r"^(\w+) \(([\w.]+)\)(?:\s*\.\.\.\s*(.*))?$")
# Python 3.12 names an unexpected success in a block of its own, like a FAIL.
_UT_HEAD = re.compile(r"^(FAIL|ERROR|UNEXPECTED SUCCESS): (\w+) \(([\w.]+)\)")
# Only a trailing ` ... <status>` is one: a docstring line may hold `...` itself
# (`@...`, "waits for x..."), and the first `...` read it as a red status
# (tv-kit D: a green test counted red in 5 of 5 runs). Only the exact status
# words are one: a test's own output that begins with `ok` (`ok, done`, `ok so
# far`) read as green and an unexpected success after it ended exit 0 in the
# triage (re-check tv-kit fix2 M1, probe under Python 3.12.3). The skip reason
# comes as unittest's `repr`: in double quotes when it holds a `'` (fix3 H).
_UT_WORDS = r"(ok|FAIL|ERROR|skipped (?:'.*'|\".*\")|expected failure|unexpected success)\s*$"
_UT_STATUS = re.compile(r"\s\.\.\.\s+" + _UT_WORDS)
# A test's own output after ` ... ` pushes its status to a line of its own.
_UT_ALONE = re.compile(r"^" + _UT_WORDS)


def unittest_id(name, inner):
    """`test_x (mod.Class.test_x)` (3.11+) and `test_x (mod.Class)` -> `mod.Class.test_x`."""
    return inner if inner.endswith("." + name) else f"{inner}.{name}"


def _ut_status(text):
    text = text.strip()
    if text.startswith("ok"):
        return "passed"
    if text.startswith("skipped") or text.startswith("expected failure"):
        return "skipped"
    return "failed"   # FAIL, ERROR, unexpected success


def parse_unittest_output(text):
    """`python3 -m unittest -v` output -> {passed, failed, skipped} lists of test ids.

    A test whose docstring line sits between its name and its `... ok` is
    taken by the next status line."""
    status, pending = {}, None
    for raw in text.splitlines():
        line = raw.rstrip()
        m = _UT_LINE.match(line)
        if m:
            tid = unittest_id(m.group(1), m.group(2))
            if m.group(3) is not None and _UT_ALONE.match(m.group(3).strip()):
                status.setdefault(tid, _ut_status(m.group(3)))
                pending = None
            else:
                pending = tid
            continue
        h = _UT_HEAD.match(line)
        if h:
            status[unittest_id(h.group(2), h.group(3))] = "failed"
            continue
        if pending:
            s = _UT_STATUS.search(line) or _UT_ALONE.match(line.strip())
            if s:
                status.setdefault(pending, _ut_status(s.group(1)))
                pending = None
    out = {"passed": [], "failed": [], "skipped": []}
    for tid, st in status.items():
        out[st].append(tid)
    return out


def _module(tid):
    parts = tid.split(".")
    return ".".join(parts[:-2]) if len(parts) > 2 else parts[0]


_UT_RAN = re.compile(r"^Ran (\d+) tests? in ", re.M)
_UT_FAILED = re.compile(r"^FAILED \(([^)]*)\)", re.M)
# Delta M5: "expected failures=" is no failure
_UT_COUNT = re.compile(r"(?<!expected )\b(failures|errors|unexpected successes)=(\d+)")


def unittest_summary(text):
    """(ran, failed) from the summary of `python3 -m unittest` -- None when cut.

    `failed` = failures + errors + unexpected successes of the `FAILED (...)`
    line, 0 after `OK`. No `Ran N tests` line, or neither `OK` nor `FAILED`
    after it, is a cut output (re-check N2)."""
    ran = _UT_RAN.findall(text or "")
    if not ran:
        return None
    tail = (text or "")[[m for m in _UT_RAN.finditer(text)][-1].end():]
    failed = _UT_FAILED.findall(tail)
    if failed:
        return int(ran[-1]), sum(int(n) for _, n in _UT_COUNT.findall(failed[-1]))
    if re.search(r"^OK\b", tail, re.M):
        return int(ran[-1]), 0
    return None


def unittest_test_results(station, text):
    """The `test_result` bodies of a unittest station, one per module.

    The failed names are checked against the summary line: a cut output (no
    summary) or a summary that counts more failures than the output names
    adds one RED part without names, `unexplained: true` -- `unexplained()`
    then keeps the station RED instead of isolating what was named (N2)."""
    parsed = parse_unittest_output(text)
    summary = unittest_summary(text)
    mods = {}
    for kind in ("passed", "failed", "skipped"):
        for tid in parsed[kind]:
            m = mods.setdefault(_module(tid), {"station": station, "part": _module(tid),
                                               "secs": 0.0, "failed_names": [],
                                               "tests": {"total": 0, "passed": 0, "failed": 0,
                                                         "flaky": 0, "skipped": 0}})
            m["tests"]["total"] += 1
            m["tests"][kind] += 1
            if kind == "failed":
                m["failed_names"].append(tid)
    out = []
    for m in mods.values():
        m["verdict"] = "RED" if m["failed_names"] else "GREEN"
        if not m["failed_names"]:
            del m["failed_names"]
        out.append(m)
    if summary is None or summary[1] > len(parsed["failed"]):
        out.append({"station": station, "part": "unittest:summary", "verdict": "RED",
                    "secs": 0.0, "unexplained": True,
                    "reason": "no summary line" if summary is None else
                    f"summary counts {summary[1]} failed, the output names {len(parsed['failed'])}"})
    return out


def parse_scenario_last_run(doc):
    """A scenario runner's `last_run.json` (path or parsed) -> {case id: GREEN|RED}.

    Two runner forms: `verdict` per case (scenarios) or `pass` (builder
    scenarios); skipped cases are not results."""
    if not isinstance(doc, dict):
        doc = json.loads(pathlib.Path(doc).read_text(encoding="utf-8"))
    out = {}
    for case in doc.get("results") or []:
        cid = str(case.get("id", ""))
        if not cid:
            continue
        if "verdict" in case:
            out[cid] = "GREEN" if case["verdict"] == "GREEN" else "RED"
        else:
            out[cid] = "GREEN" if case.get("pass") is True else "RED"
    return out


def scenario_errors(doc):
    """{case id: error text} of the red cases of a `last_run.json` that carry one."""
    if not isinstance(doc, dict):
        doc = json.loads(pathlib.Path(doc).read_text(encoding="utf-8"))
    return {str(c["id"]): c["error"] for c in doc.get("results") or []
            if c.get("id") and isinstance(c.get("error"), str) and c["error"]}


def scenario_test_results(station, cases, errors=None):
    """The `test_result` bodies of a scenario station, one per case
    (`<station>:case:<id>`, `case_key`).

    A red case carries its own `error` text (additive) when `errors` has one."""
    out = []
    for cid, verdict in cases.items():
        key = case_key(station, cid)
        r = {"station": station, "part": key, "verdict": verdict, "secs": 0.0}
        if verdict != "GREEN":
            r["failed_names"] = [key]
            if (errors or {}).get(cid):
                r["error"] = errors[cid]
        out.append(r)
    return out


def scenario_last_run(path, started_wall):
    """(cases, errors) of the runner's `last_run.json` -- None when it does not
    belong to this run (re-check N3).

    The runners exit 1 on a red case AND on a crash or `SystemExit`, and write
    `last_run.json` only at their end: a file older than the station start is
    the previous run's, a missing or unreadable one says nothing. Both make
    the station's red unexplained, never a list of cases to isolate."""
    try:
        if os.path.getmtime(path) < started_wall - 1:
            return None
        with open(path, encoding="utf-8") as fh:
            doc = json.load(fh)
        cases = parse_scenario_last_run(doc)
    except (OSError, ValueError, TypeError, AttributeError):
        return None
    if not cases:
        return None
    return cases, scenario_errors(doc)


_SCENARIO_RUNNER = re.compile(r"(^|/)run_\w*scenarios\.py$")


def station_kind(station, argv):
    """`nextest|unittest|scenario` of the command that failed, else None (no isolation).

    Read from the Python form of the command (`lane.python_rest`, the one
    reader): a scenario runner is the SCRIPT the interpreter runs, never any
    word of its arguments (re-check tv-bn M7q M2: `precheck.py --files ...
    run_builder_scenarios.py` was a scenario station); `unittest` is among the
    first three words after the interpreter (`-m unittest`,
    `selftest_shard.py unittest`, `hang_dump.py -m unittest`)."""
    if station.get("nextest") is not None:
        return "nextest"
    if _scenario_script(argv) is not None:
        return "scenario"
    from . import lane
    rest = lane.python_rest(argv)
    if rest is not None and "unittest" in rest[:3]:
        return "unittest"
    return None


def _scenario_script(argv):
    """The scenario runner a command runs as its script, else None."""
    from . import lane
    rest = lane.python_rest(argv)
    if rest and _SCENARIO_RUNNER.search(rest[0]):
        return rest[0]
    return None


def scenario_runner_path(argv, cwd):
    """`<cwd>/<dir of the runner>/last_run.json` of a scenario command."""
    return os.path.join(cwd, os.path.dirname(_scenario_script(argv)), "last_run.json")


def scenario_base_argv(argv):
    """The runner command without its case selection (`--only`, `--only-templates`)."""
    out, skip = [], False
    for a in argv:
        if a in ("--only", "--only-templates"):
            skip = True
            continue
        if skip and not a.startswith("-"):
            continue
        skip = False
        out.append(a)
    return out


# --- test keys ----------------------------------------------------------------------

CASE = "case:"


def case_key(station, cid):
    """The register key of a scenario case: `<station>:case:<id>`.

    Re-check tb-s3-store2 M-1 (#1161): two suites share case ids (`L1` is a
    memory AND a builder case), so a bare `case:<id>` merged two tests into
    one register row, one issue and one window."""
    return f"{station}:{CASE}{cid}" if station else f"{CASE}{cid}"


def case_id(key):
    """The case id of a scenario key -- the station form or the legacy `case:<id>`."""
    if ":" + CASE in key:
        return key.rsplit(":" + CASE, 1)[1]
    return key[len(CASE):] if key.startswith(CASE) else key


def legacy_case(key):
    """The legacy register key `case:<id>` of a station key, else None."""
    return CASE + case_id(key) if ":" + CASE in key else None


def qualify(station, key):
    """A legacy `case:<id>` (a journal line of an older kit) -> its station key."""
    if station and isinstance(key, str) and key.startswith(CASE):
        return case_key(station, key[len(CASE):])
    return key


def test_key(part, name):
    """The register key of a test from its `test_result` part and name (V0 section 2)."""
    m = _UT_LINE.match(name)
    if m and m.group(3) is None:
        name = unittest_id(m.group(1), m.group(2))
    if name.startswith("case:"):
        return name
    if part and (name == part or name.startswith(part + ".")):
        return name   # a unittest id already carries its module
    return f"{part}::{name}"


def split_test(key, known_binaries):
    """`<binary_id>::<name>` -> (binary_id, name) by the longest known binary prefix.

    Binary ids are `crate` or `crate::bin` and test names may carry `::`, so
    the split needs the build's binary list; an unknown binary -> None."""
    best = None
    for b in known_binaries:
        if key.startswith(b + "::") and len(key) > len(b) + 2:
            if best is None or len(b) > len(best):
                best = b
    return None if best is None else (best, key[len(best) + 2:])


# --- isolation argv and verdicts ------------------------------------------------------

_UT_SKIPPED = re.compile(r"^(?:OK|FAILED) \([^)]*\bskipped=(\d+)", re.M)


def unittest_ran(text):
    """True when a unittest output ran at least one test and none failed (Delta I2).

    A skipped test is no run evidence: Ran - skipped >= 1."""
    got = unittest_summary(text)
    if got is None or got[1] != 0:
        return False
    tail = text[[m for m in _UT_RAN.finditer(text)][-1].end():]
    skipped = _UT_SKIPPED.findall(tail)
    return got[0] - (int(skipped[-1]) if skipped else 0) >= 1


def runs_ignored(argv):
    """True when a nextest command ran the `#[ignore]` tests (`--run-ignored`)."""
    return any(a == "--run-ignored" or a.startswith("--run-ignored=") for a in argv or ())


_VALUE_FLAGS = ("--profile", "-E", "--filterset", "--retries", "--stress-count",
                "--run-ignored", "-P")


def _strip_flags(argv):
    out, skip = [], False
    for a in argv:
        if skip:
            skip = False
            continue
        if a in _VALUE_FLAGS:
            skip = True
            continue
        if any(a.startswith(f + "=") for f in _VALUE_FLAGS if f.startswith("--")):
            continue
        out.append(a)
    return out


def nextest_isolation_argv(base_argv, binary_id, name, count=ISOLATION_COUNT, ignored=False):
    """The isolation run of ONE test: `--stress-count` in profile `isolate`.

    The binary term is mandatory: same-named tests (`tests::...`) live in many
    binaries. The station's own profile, filter, retries are dropped from the
    base so the isolation is exactly this test, retried never."""
    argv = _strip_flags(list(base_argv)) + ["--profile", "isolate", "--retries", "0"]
    if count and count > 1:
        argv += ["--stress-count", str(count)]
    if ignored:
        argv += ["--run-ignored", "all"]
    return argv + ["-E", f"binary_id(={binary_id}) & test(={name})"]


def isolate_profile_toml():
    """The profile `isolate` (tool config file): the default profile, no retries."""
    return ("[profile.isolate]\n"
            'inherits = "default"\n'
            "retries = 0\n"
            "fail-fast = false\n")


def unittest_base_argv():
    """`python -m unittest` in the kit's one Python, through hang_dump (re-check tv-bn M7m M2)."""
    from . import hang_dump, lane
    return hang_dump.argv(lane.PYTHON, "unittest")


def unittest_isolation_argv(test_id, base_argv=None):
    """One unittest alone: `-k <id>` on the station's command, else the id itself."""
    if base_argv:
        return list(base_argv) + ["-k", test_id]
    return unittest_base_argv() + [test_id]


def scenario_isolation_argv(case, base_argv=("python3", "run_scenarios.py")):
    """One scenario case alone: the runner with `--only <case>`."""
    return list(base_argv) + ["--only", case_id(case)]


_STRESS = re.compile(r"(\d+)/(\d+) stress run iterations: (\d+) passed([^\n]*)")
_ITERATION = re.compile(r"iteration (\d+)/(\d+): (\d+) tests? run: (\d+) passed")


def stress_verdict(rc, output, count=ISOLATION_COUNT):
    """(GREEN|RED, green iterations) of a nextest stress run -- fail-closed.

    Measured on nextest 0.9.146: a stress run whose EARLY iterations failed
    ends with exit 0 (`3/3 stress run iterations: 2 passed, 1 failed`, rc 0),
    so the exit code alone would call a flake green. Green is rc 0 AND the
    summary `N/N stress run iterations: N passed` with nothing after it, for
    N = count; no summary is red. And every iteration must prove that the
    test ran (Delta I2): `iteration i/N: k test(s) run: k passed` with k >= 1
    for each i -- a filter that matched nothing passes zero tests N times."""
    found = _STRESS.findall(output or "")
    if not found:
        return "RED", 0
    done, total, passed, tail = found[-1]
    green = int(passed)
    ran = {int(i) for i, n, k, p in _ITERATION.findall(output or "")
           if int(n) == count and int(k) >= 1 and int(p) == int(k)}
    ok = (rc == 0 and int(done) == int(total) == count and green == count
          and not tail.strip() and ran == set(range(1, count + 1)))
    return ("GREEN" if ok else "RED"), green


def repeat_verdict(rcs, count=ISOLATION_COUNT):
    """(GREEN|RED, green runs) of `count` single runs (Python, scenarios)."""
    green = sum(1 for rc in rcs if rc == 0)
    return ("GREEN" if len(rcs) == count and green == count else "RED"), green


def isolation_candidates(test_results, cap=ISOLATION_CAP, kind="nextest"):
    """The red tests of a station -> (candidates, over_cap).

    `test_results` are the station's `test_result` bodies (V0 section 3); a
    candidate is {test, part, name, error}. The cap holds for every kind (Delta
    I1): ten single runs per red Python test or scenario case cost more than a
    nextest stress run, and many red ones are a defect, not a flake."""
    cands, seen = [], set()
    for r in test_results:
        if r.get("verdict") == "GREEN":
            continue
        for name in r.get("failed_names") or ():
            key = test_key(r.get("part", ""), name)
            if key in seen:
                continue
            seen.add(key)
            cands.append({"test": key, "part": r.get("part", ""), "name": name,
                          "error": r.get("error") or ""})
    return cands, len(cands) > cap


def unexplained(test_results):
    """True when the failed names do not explain the station's red (C1, fail-closed).

    A non-green part without names (a `cmd` part, a cut JUnit or unittest
    output), or one whose `tests.failed` counts more than it names."""
    for r in test_results:
        if r.get("verdict") == "GREEN":
            continue
        names = r.get("failed_names") or ()
        if not names:
            return True
        failed = (r.get("tests") or {}).get("failed")
        if isinstance(failed, int) and failed > len(names):
            return True
    return False


# --- the triage (OR-TV-2, TV-F2/TV-F5) ------------------------------------------------

# The cause classes of a flake (VG TV-F5) plus the triage's own two answers.
CAUSES = ("async wait", "concurrency", "order/state", "resource/load", "time", "network",
          "random", "infrastructure")
CAUSE_T2 = "resource/load|async wait"
CAUSE_T3 = "order/state"
CAUSE_T4 = "time"
CAUSE_NONE = "not reproduced"
DEFECT = "defect"
T2_MODULE_FROM = 200     # more tests than this in the binary: T2 runs the module `<stem>::`
# T4: the injected kit clock (`clock=`) reads this variable -- one day on, then
# across a month's and a year's end (VG TV-F2).
CLOCK_ENV = "MECLAW_TEST_CLOCK_SHIFT"
CLOCK_SHIFTS = ("+24h", "month", "year")
# The kit clock (`scripts/tests/kit_time.py`, tv-s1-kit) reads CLOCK_ENV. A tree
# without a reader would make T4 run nothing and say so (re-check A-M4, the
# `clock_reader=False` branch); the pin `test_the_clock_reader_flag_matches_the_tree`
# keeps flag and tree equal.
CLOCK_READER = True
T4_UNAVAILABLE = "NOTE not available: nothing reads " + CLOCK_ENV
SHUFFLE = pathlib.Path(__file__).resolve().parent / "shuffle.py"

_NX_SUMMARY = re.compile(r"Summary \[[^\]]*\] (\d+) tests? run: (\d+) passed([^\n]*)")


def run_verdict(rc, output):
    """GREEN|RED of ONE nextest run (no stress) -- fail-closed.

    Green is rc 0 AND the summary `N test(s) run: N passed` with N >= 1 and no
    failure after it; a filter that matched nothing proves nothing (I2)."""
    found = _NX_SUMMARY.findall(output or "")
    if not found or rc != 0:
        return "RED"
    ran, passed, tail = found[-1]
    ok = int(ran) >= 1 and int(ran) == int(passed) and not re.search(
        r"\b[1-9]\d* (?:failed|timed out|leaky)", tail)
    return "GREEN" if ok else "RED"


def _binary_size(test_results, part):
    """The number of tests the station's JUnit counted in `part` (0: unknown)."""
    n = 0
    for r in test_results:
        if r.get("part") == part:
            n += sum(v for v in (r.get("tests") or {}).values() if isinstance(v, int))
    return n


def t2_nextest_argv(base_argv, binary_id, name, threads, size):
    """T2: the test's harness binary with the station's thread count; over
    `T2_MODULE_FROM` tests only the module `<stem>::` of the test."""
    argv = _strip_flags(list(base_argv)) + ["--profile", "isolate", "--retries", "0",
                                            "--test-threads", str(int(threads or 1))]
    expr = f"binary_id(={binary_id})"
    if size > T2_MODULE_FROM and "::" in name:
        expr += " & test(/^%s::/)" % re.escape(name.split("::", 1)[0])
    return argv + ["-E", expr]


def t3_unittest_argv(module, seed):
    """T3: the test's module in a random order, the seed on the command line."""
    from . import lane   # the kit's one Python (re-check tv-bn M7m M2)
    return lane.python_argv(SHUFFLE, "--seed", int(seed), module)


def clock_tests(test_results, tree):
    """The red unittest ids whose module injects the kit clock (`clock=`): T4 runs for them."""
    out = set()
    for r in test_results:
        for name in r.get("failed_names") or ():
            path = pathlib.Path(tree) / (_module(name).replace(".", "/") + ".py")
            try:
                if "clock=" in path.read_text(encoding="utf-8", errors="replace"):
                    out.add(name)
            except OSError:
                continue
    return out


def triage_station(station, kind, test_results, run_cmd, quarantine_issues, cap=ISOLATION_CAP,
                   base_argv=None, ignored=(), expired=None, proof=None, threads=None,
                   seed=None, clock_tests=(), clock_reader=None):
    """THE hook between a red station's end and its `station_ended` (OR-TV-2).

    Every red test is triaged instead of run ten times (VG TV-F2): T1 alone
    (nextest: process per test, Python: one test, scenario: one case), T2 the
    harness under the station's load (nextest: its binary with the station's
    `threads`, over 200 tests the module; Python: the module; scenario: the
    station's runner), T3 the module in a random order with the `seed` on its
    command line (Python only -- nextest isolates per process, T2 covers it),
    T4 only for `clock_tests` (an injected kit clock): T1 again with
    `CLOCK_ENV` one day on and across a month's and a year's end -- only with a
    reader of it (`clock_reader`, default `CLOCK_READER`); without one T4 is the
    NOTE `T4_UNAVAILABLE`, no run and no `time` class.

    T1 red -> RED `defect`. T1 green, T2 red -> FLAKY `resource/load|async
    wait`; T2 green, T3 red -> FLAKY `order/state`; T4 red -> FLAKY `time`;
    all green -> FLAKY `not reproduced` -- still a finding: FLAKY counts red
    (TV-F1) unless the test is on the legacy list (`end_station`).

    `run_cmd(argv[, env]) -> (rc, output)`; `proof(test, rc, output)` says a
    run really ran the test (Delta I2); `expired()` is the station's budget,
    asked before every run -- spent, the triage stops RED (Delta I1)."""
    cands, over = isolation_candidates(test_results, cap=cap, kind=kind)
    if not cands:
        return {"verdict": "RED", "isolated": 0, "reason": "nothing to isolate", "events": []}
    if unexplained(test_results):
        return {"verdict": "RED", "isolated": 0, "reason": UNEXPLAINED, "events": []}
    hard = sorted({k for r in test_results for k in (r.get("hard_failures") or {}).values()})
    if hard:  # a crash, a timeout, a test that never ran: a defect, not a flake (I1)
        return {"verdict": "RED", "isolated": 0, "reason": "not isolatable: " + ", ".join(hard),
                "events": []}
    if over:
        return {"verdict": "RED", "isolated": 0, "reason": TOO_MANY, "events": []}
    if seed is None:
        import random
        seed = random.SystemRandom().randrange(1, 2 ** 31)
    evs = []
    spent = expired or (lambda: False)
    base = list(base_argv or [])
    if kind == "scenario" and not base:
        base = ["python3", "run_scenarios.py"]

    class _Spent(Exception):
        pass

    for c in cands:
        runs = {"n": 0, "green": 0}

        def run(argv, env=None, test=c["test"]):
            if spent():
                raise _Spent()
            runs["n"] += 1
            rc, out = run_cmd(argv, env) if env else run_cmd(argv)
            if kind == "nextest":
                ok = run_verdict(rc, out) == "GREEN"
            else:   # no proof of a run is a red run (Delta I2)
                ok = rc == 0 and (proof(test, rc, out) if proof is not None
                                  else kind == "unittest" and unittest_ran(out))
            runs["green"] += 1 if ok else 0
            return ok

        if kind == "nextest":
            ign = ignored is True or c["test"] in ignored
            t1 = nextest_isolation_argv(base, c["part"], c["name"], count=1, ignored=ign)
            t2 = t2_nextest_argv(base, c["part"], c["name"], threads,
                                 _binary_size(test_results, c["part"]))
            if ign:
                t2 += ["--run-ignored", "all"]
            t3 = None
        elif kind == "unittest":
            t1 = unittest_isolation_argv(c["test"], base)
            t2 = unittest_isolation_argv(_module(c["test"]), base)
            t3 = t3_unittest_argv(_module(c["test"]), seed)
        else:
            t1 = scenario_isolation_argv(c["test"], base)
            t2, t3 = list(base), None
        steps, cause = {}, None
        try:
            steps["t1"] = "GREEN" if run(t1) else "RED"
            if steps["t1"] == "GREEN":
                steps["t2"] = "GREEN" if run(t2) else "RED"
                if steps["t2"] == "RED":
                    cause = CAUSE_T2
            if cause is None and steps["t1"] == "GREEN" and t3 is not None:
                steps["t3"] = "GREEN" if run(t3) else "RED"
                if steps["t3"] == "RED":
                    cause = CAUSE_T3
            t4 = cause is None and steps["t1"] == "GREEN" and c["test"] in set(clock_tests or ())
            if t4 and not (CLOCK_READER if clock_reader is None else clock_reader):
                steps["t4"] = T4_UNAVAILABLE
            elif t4:
                for shift in CLOCK_SHIFTS:
                    if not run(t1, {CLOCK_ENV: shift}):
                        steps["t4"] = "RED " + shift
                        cause = CAUSE_T4
                        break
                else:
                    steps["t4"] = "GREEN"
        except _Spent:
            return {"verdict": "RED", "isolated": len(evs), "reason": BUDGET_SPENT,
                    "events": [dict(e, verdict="RED") for e in evs]}
        ev = {"station": station, "test": c["test"], "count": runs["n"], "green": runs["green"],
              "triage": steps}
        if steps["t1"] == "RED":
            ev["verdict"] = "RED"
        else:
            ev.update(verdict="FLAKY", cause=cause or CAUSE_NONE)
        if "t3" in steps:
            ev["seed"] = int(seed)
        gh721 = station == GH721_STATION and GH721_SIGNATURE in c["error"]
        issue = quarantine_issues.get(c["test"]) or (GH721_ISSUE if gh721 else None)
        if issue is not None:
            ev["issue"] = int(issue)
        evs.append(ev)
    reds = [e for e in evs if e["verdict"] == "RED"]
    if reds:
        # #1164: the label names the tests the triage proved red, not the log's first FAIL.
        return {"verdict": "RED", "isolated": len(evs),
                "reason": f"triage T1 red: {DEFECT}, {len(reds)} of {len(evs)}", "events": evs,
                "where": ", ".join(e["test"] for e in reds)}
    causes = ", ".join(dict.fromkeys(e["cause"] for e in evs))
    if any("issue" not in e for e in evs):
        reason = PENDING
    else:
        issues = list(dict.fromkeys(e["issue"] for e in evs))
        reason = "flaky: " + ", ".join(f"GH #{i}" for i in issues)
    return {"verdict": "FLAKY", "isolated": len(evs), "reason": reason,
            "events": evs, "kind": kind, "cause": causes}


# The tail `finding.red_reason` puts on a RED reason: `at <where> -- log <path>`.
_WHERE = re.compile(r"(?:(?:^|; )(at .*) -- )?(log \S+)$")


def _keep_where(reason, old, where=None):
    """`reason` with the location and log tail of `old` (review R3 M2), once.

    `where` (the tests an isolation proved red, #1164) replaces the location of
    `old`: the log's first finding may be a test the isolation found green."""
    at = ("at " + where) if where else None
    m = _WHERE.search(old)
    if not m:
        return "; ".join(p for p in (reason, at) if p) if at else (reason or old)
    if reason.endswith(m.group(0)):
        return reason
    head = "; ".join(p for p in (reason, at or m.group(1)) if p)
    return "%s -- %s" % (head, m.group(2)) if head else m.group(2)


def end_station(ended, result, legacy=()):
    """The lines a red station ends with: its isolations, then ONE `station_ended`.

    `ended` is the `station_ended` body the executor would have written
    (verdict RED); the final line carries the end verdict, `isolated: n` and
    the reason. FLAKY needs the station's exit to be the test-failure code of
    its kind (`TEST_FAILURE_EXIT`), else RED `unexplained red: exit <n>`. A
    station that was not red ends unchanged.

    A FLAKY end carries its `cause` (TV-F5) and counts red (TV-F1) -- except
    when every test of it is in `legacy` (the kern's `legacy_tests`: on
    `.config/flaky-legacy.toml` and unchanged in this diff, TV-F4): then it
    carries `legacy: true` and counts green."""
    if ended.get("verdict") != "RED" or result is None:
        return [("station_ended", dict(ended))]
    final = dict(ended, verdict=result["verdict"], isolated=result["isolated"])
    if result.get("reason"):
        final["reason"] = result["reason"]
    later = result.get("later")
    if (result["verdict"] == "FLAKY"
            and ended.get("exit") != TEST_FAILURE_EXIT.get(result.get("kind"))):
        final.update(verdict="RED", reason=f"{UNEXPLAINED}: exit {ended.get('exit')}")
    elif result["verdict"] == "FLAKY" and later is not None and not later.get("ran"):
        final.update(verdict="RED", reason=LATER_NOT_RUN)
    elif result["verdict"] == "FLAKY" and later is not None and (
            later.get("exit") != 0 or later.get("timed_out")):
        why = "timeout" if later.get("timed_out") else f"exit {later.get('exit')}"
        final.update(verdict="RED", reason=f"isolated flaky, later command red: {why}")
    if final["verdict"] == "RED":
        final["reason"] = _keep_where(final.get("reason") or "", ended.get("reason") or "",
                                      result.get("where"))
    if final["verdict"] == "FLAKY":
        # Delta C1: every command of the station ran.
        final["all_cmds"] = True
        if result.get("cause"):
            final["cause"] = result["cause"]
        tests = [e["test"] for e in result["events"]]
        if tests and set(tests) <= set(legacy or ()):
            final["legacy"] = True
    evs = [dict(e) for e in result["events"]]
    if final["verdict"] == "RED":
        # The station stays RED, so no isolation of it may count as a flake in
        # the register (re-check N4): the runs stay, the verdict follows the end.
        for e in evs:
            e["verdict"] = "RED"
    return [("flake_isolated", e) for e in evs] + [("station_ended", final)]


# --- the runtime quarantine file -------------------------------------------------------

_NEXTEST_NAME = re.compile(r"^[A-Za-z0-9_:]+$")
QUARANTINE_HEAD = (
    "# Runtime quarantine of ONE pipeline run -- written into the run's workspace,\n"
    "# never committed. Every entry is a flake the register measured, with its issue\n"
    "# and its expiry; the hand-kept list stays in .config/nextest.toml.\n")


def quarantine_toml(entries, known_binaries, profiles=("default",)):
    """The overrides of the dispatch's `quarantine` list -> (text, dropped).

    Same grammar as `.config/nextest.toml` (comment `# GH #<n> -- ...`, a
    `binary_id(=...) & test(=...)` filter, `retries = 2`). An entry whose
    binary is not in this build is DROPPED and returned: nextest refuses the
    whole config on a `binary_id(=X)` that matches no binary (measured on
    0.9.146: `operator didn't match any binary IDs`)."""
    blocks, dropped = [], []
    for e in entries:
        split = split_test(e["test"], known_binaries)
        if split is None or not _NEXTEST_NAME.match(split[1]):
            dropped.append(e["test"])
            continue
        binary, name = split
        for prof in profiles:
            blocks.append(
                f"[[profile.{prof}.overrides]]\n"
                f"# GH #{int(e['issue'])} -- runtime quarantine until {e['until']}\n"
                f"filter = 'binary_id(={binary}) & test(={name})'\n"
                "retries = 2\n"
                'flaky-result = "fail"\n')
    return "\n".join([QUARANTINE_HEAD] + blocks), dropped


def write_quarantine(ws, entries, known_binaries, profiles=("default",)):
    """Write `<ws>/quarantine.toml` (isolation profile + quarantine) -> (argv, dropped).

    The argv is what every nextest station of the run gets:
    `--tool-config-file pipeline:<abs>` -- a tool config ranks below the repo
    file, so `.config/nextest.toml` is never edited (a pipeline that changed a
    tracked file from a worktree would have no way to merge it)."""
    text, dropped = quarantine_toml(entries, known_binaries, profiles)
    path = pathlib.Path(ws) / "quarantine.toml"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text + "\n" + isolate_profile_toml(), encoding="utf-8")
    return ["--tool-config-file", f"pipeline:{path.resolve()}"], dropped


# --- core: the register -----------------------------------------------------------------

def _rows(db, event):
    # A list, never a live cursor (welle-tb S3, #1161): a write while the
    # cursor still reads holds a WAL snapshot, and once another gate commits,
    # that write's BEGIN IMMEDIATE fails at once -- `database is locked`, no
    # busy_timeout (run-7b40b01d16cb lost its flake register that way).
    return [(seq, at, run, json.loads(data) if data else {})
            for seq, at, run, data in db.execute(
                "SELECT seq, at, run, data FROM journal WHERE event=? ORDER BY seq",
                (event,)).fetchall()]


def _repo_of(db, run):
    row = db.execute("SELECT data FROM journal WHERE run=? AND event='pass_planned'"
                     " ORDER BY seq LIMIT 1", (run,)).fetchone()
    return json.loads(row[0]).get("repo") if row and row[0] else None


def get(db, test):
    row = db.execute("SELECT * FROM flakes WHERE test=?", (test,)).fetchone()
    return dict(row) if row else None


def rate(row):
    return (row["flaky"] or 0) / row["runs"] if row and row["runs"] else 0.0


def quarantine_applies(test):
    """The runtime quarantine is a nextest filter entry `<binary_id>::<name>`
    (`quarantine_toml`). A unittest or scenario key has no `::` and no entry
    it could become, so it is never quarantined -- a quarantine without effect
    left a status, a dispatch entry dropped as "binary not in this build" and
    an expiry finding (re-check tb-s3-store2 m-2, #1161)."""
    return isinstance(test, str) and "::" in test


# A quarantine is armed only with all four fields: issue, owner, cause class and
# expiry (VG TV-F3). A row armed before migration 4 has no owner and no cause and
# is disarmed by this -- the migration invents no owner (re-check A-M3).
ARMED_SQL = ("quarantined_until > ? AND issue IS NOT NULL AND owner IS NOT NULL"
             " AND owner != '' AND cause IS NOT NULL AND cause != ''")


def armed(row, at):
    """True when `row` holds an armed quarantine at `at` (`ARMED_SQL`)."""
    return bool(row is not None and quarantine_applies(row["test"]) and row["quarantined_until"]
                and row["quarantined_until"] > at and row["issue"] and row["owner"]
                and row["cause"])


def status(row, at):
    if row is None:
        return "unknown"
    if armed(row, at):
        return "quarantined"
    if row["issue"]:
        return "tracked"
    if row["flaky"]:
        return PENDING
    return "red"


def refresh(db, tests=None, at=None):
    """Rebuild the `flakes` rows from the journal; returns the tests written.

    Observation = one `test_result` line whose `part` is the test's binary
    (module, case), over all modes. A nextest station writes its parts on
    every end (JUnit); a unittest or scenario station only when it is red and
    reaches the isolation (`executor._isolate`), so their window counts red
    ends (welle-tb S3, #1161). Per test the window is its last 20
    observations: `reds` = red without a green isolation, `flaky` = red with a
    FLAKY isolation or retried to green by nextest (`flaky_names`), `runs` =
    the window's size. Rows exist only for tests that were ever red or flaky;
    `first_at`/`last_at` are the first and last such observation, over the
    whole history. `quarantined_until` is kept, and so is a known `issue`; an
    empty one takes the issue the last isolation of the test named."""
    iso, issue_of = {}, {}
    for _, _, run, d in _rows(db, "flake_isolated"):
        test = qualify(d.get("station"), d.get("test"))
        # A RED isolation sticks: a later round of the same run that isolates
        # the test green does not wash the defect out (re-check N1).
        if iso.get((run, test)) != "RED":
            iso[(run, test)] = d.get("verdict")
        if d.get("issue") is not None:
            issue_of[test] = str(int(d["issue"]))
    obs, part_of, seen = {}, {}, {}
    for seq, at_, run, d in _rows(db, "test_result"):
        part, failed, retried = _observed(d)
        obs.setdefault(part, []).append((at_, run, set(failed), set(retried)))
        for key in failed + retried:
            part_of[key] = part
            seen.setdefault(key, []).append((at_, run, d.get("station")))
    wanted = set(part_of) if tests is None else set(tests) & set(part_of)
    written = []
    with _store.tx(db):
        for key in sorted(wanted):
            part = part_of[key]
            runs = reds = flaky = 0
            for _, run, failed, retried in obs[part][-WINDOW:]:
                runs += 1
                if key in failed:
                    if iso.get((run, key)) == "FLAKY":
                        flaky += 1
                    else:
                        reds += 1
                elif key in retried:
                    flaky += 1
            first_at, last_run = seen[key][0][0], seen[key][-1]
            db.execute(
                "INSERT INTO flakes (test, station, repo, first_at, last_at, runs, reds, flaky)"
                " VALUES (?,?,?,?,?,?,?,?) ON CONFLICT(test) DO UPDATE SET"
                " station=excluded.station, repo=excluded.repo,"
                " first_at=min(coalesce(flakes.first_at, excluded.first_at), excluded.first_at),"
                " last_at=excluded.last_at, runs=excluded.runs, reds=excluded.reds,"
                " flaky=excluded.flaky",
                (key, last_run[2], _repo_of(db, last_run[1]), first_at, last_run[0],
                 runs, reds, flaky))
            if key in issue_of:   # the issue an isolation named, unless one is known
                db.execute("UPDATE flakes SET issue=? WHERE test=? AND issue IS NULL",
                           (issue_of[key], key))
            if legacy_case(key) is not None:
                _supersede(db, legacy_case(key), key)
            written.append(key)
    return written


def _observed(d):
    """(part, failed keys, retried keys) of one `test_result` body; a legacy
    scenario line (`case:<id>` beside its station) reads as its station key."""
    station = d.get("station")
    part = qualify(station, d.get("part", ""))
    return (part, [test_key(part, qualify(station, n)) for n in d.get("failed_names") or ()],
            [test_key(part, qualify(station, n)) for n in d.get("flaky_names") or ()])


def _supersede(db, legacy, key):
    """Migration of a legacy row `case:<id>` (re-check tb-s3-store2 M-1, #1161).

    The station key takes the legacy row's issue when it has none -- no
    second issue for the same test -- and the legacy row names its successor
    in `evidence`: it stays readable (`flakes show case:<id>`), takes no
    observation any more, and is never silently orphaned."""
    old = get(db, legacy)
    if old is None:
        return
    if old["issue"]:
        db.execute("UPDATE flakes SET issue=? WHERE test=? AND issue IS NULL",
                   (old["issue"], key))
    note = f"superseded by {key}"
    if note not in (old["evidence"] or ""):
        db.execute("UPDATE flakes SET evidence=? WHERE test=?",
                   ("; ".join(x for x in (old["evidence"], note) if x), legacy))


def _plus_days(at, days):
    return (_store.parse_at(at) + timedelta(days=days)).strftime("%Y-%m-%dT%H:%M:%SZ")


def _ensure(db, test):
    db.execute("INSERT OR IGNORE INTO flakes (test, runs, reds, flaky) VALUES (?, 0, 0, 0)",
               (test,))


def register_external(db, test, repo, issue, evidence, at=None):
    """Register a flake seen outside this repo's journal (#1135), e.g. a satellite test.

    The row carries `repo`, the issue in that repo and `evidence` (where the
    red was seen); it has no observations (`runs` 0), so `refresh` never
    rewrites it and the auto quarantine never takes it."""
    if not repo or not evidence or issue is None or int(issue) <= 0:
        raise ValueError("a hand registration needs repo, issue and evidence")
    at = at or _store.now()
    with _store.tx(db):
        _ensure(db, test)
        db.execute("UPDATE flakes SET repo=?, issue=?, evidence=?, first_at=coalesce(first_at, ?),"
                   " last_at=? WHERE test=?", (repo, str(int(issue)), evidence, at, at, test))


class QuarantineFull(ValueError):
    """A ninth active quarantine (VG TV-F3): refused, RED for the caller."""


def armable_cause(cause):
    """A cause class a quarantine may carry: one of `CAUSES` or the T2 answer."""
    return cause in CAUSES or cause == CAUSE_T2


def quarantine(db, test, issue, owner=None, cause=None, days=QUARANTINE_DAYS, at=None):
    """Arm the quarantine of `test` (data in the store, never a commit) -> its expiry.

    Armed only with all four fields (VG TV-F3): an issue (a number, never
    `pending`), an owner, a cause class (`armable_cause`) and an expiry of at
    most `QUARANTINE_DAYS`. At most `QUARANTINE_MAX` entries are active at
    once: a ninth raises `QuarantineFull` -- re-arming an active one is none.
    Arming forgets an earlier measured fix (`record_fix`)."""
    if issue is None or not str(issue).isdigit() or int(issue) <= 0:
        raise ValueError("a quarantine needs an issue (a number, never pending)")
    if not isinstance(owner, str) or not owner.strip():
        raise ValueError("a quarantine needs an owner")
    if not armable_cause(cause):
        raise ValueError("a quarantine needs a cause class, one of: %s"
                         % ", ".join(CAUSES + (CAUSE_T2,)))
    if not isinstance(days, int) or not 1 <= days <= QUARANTINE_DAYS:
        raise ValueError("a quarantine runs 1 to %d days" % QUARANTINE_DAYS)
    if not quarantine_applies(test):
        raise ValueError("a quarantine takes nextest tests only (<binary_id>::<name>)")
    at = at or _store.now()
    until = _plus_days(at, days)
    with _store.tx(db):
        active = db.execute("SELECT COUNT(*) FROM flakes WHERE " + ARMED_SQL + " AND test != ?",
                            (at, test)).fetchone()[0]
        if active >= QUARANTINE_MAX:
            raise QuarantineFull("%s: %d of %d entries are active" % (QUARANTINE_FULL, active,
                                                                     QUARANTINE_MAX))
        _ensure(db, test)
        db.execute("UPDATE flakes SET issue=?, quarantined_until=?, owner=?, cause=?, fixed=NULL"
                   " WHERE test=?", (str(int(issue)), until, owner.strip(), cause, test))
    return until


def record_fix(db, test, quiet, load, evidence="", at=None):
    """A measured fix (VG TV-F6): no red in `quiet` quiet and `load` loaded runs,
    at least `FIX_QUIET` + `FIX_LOAD`. Ends the quarantine and clears its
    expiry duty; fewer runs raise -- a longer wait is never a fix."""
    if int(quiet) < FIX_QUIET or int(load) < FIX_LOAD:
        raise ValueError("a fix is measured: at least %d quiet and %d loaded runs without red"
                         % (FIX_QUIET, FIX_LOAD))
    at = at or _store.now()
    note = "%s quiet=%d load=%d%s" % (at, int(quiet), int(load),
                                      (" " + evidence) if evidence else "")
    with _store.tx(db):
        cur = db.execute("UPDATE flakes SET fixed=?, quarantined_until=min(coalesce("
                         "quarantined_until, ?), ?) WHERE test=?", (note, at, at, test))
    if cur.rowcount != 1:
        raise ValueError(f"{test} is not in the register")
    return note


def release(db, test, at=None):
    """End a quarantine now: `quarantined_until` becomes the present (a past from now on)."""
    at = at or _store.now()
    with _store.tx(db):
        cur = db.execute("UPDATE flakes SET quarantined_until=? WHERE test=?", (at, test))
    return cur.rowcount == 1


def auto_quarantine(db, tests, at, threshold=RATE_THRESHOLD):
    """PROPOSE a quarantine for every test of `tests` at rate >= threshold with an
    issue (VG TV-F3).

    The pipeline never arms one: a proposal is the finding
    `flake_quarantine_proposed`, once per test while it is not quarantined;
    `pipeline flakes quarantine` arms it with issue, owner, cause and expiry.
    Never one with a defect red (`reds` > 0) in its window (C2); never a
    unittest or scenario key (`quarantine_applies`). Returns the proposed tests."""
    done = []
    for test in tests:
        if not quarantine_applies(test):
            continue
        row = get(db, test)
        if (not row or not row["issue"] or (row["reds"] or 0) > 0
                or rate(row) < threshold - 1e-9):
            continue
        if armed(row, at):
            continue
        since = row["quarantined_until"] or ""
        known = db.execute(
            "SELECT 1 FROM journal WHERE event='process_finding'"
            " AND json_extract(data, '$.rule')='flake_quarantine_proposed'"
            " AND json_extract(data, '$.subject')=? AND json_extract(data, '$.since')=?",
            (test, since)).fetchone()
        if not known:
            _events.emit(db, "process_finding", "finding",
                         {"rule": "flake_quarantine_proposed", "subject": test,
                          "value": round(rate(row), 4), "threshold": threshold,
                          "since": since, "issue": row["issue"]}, at=at)
        done.append(test)
    return done


# An expiry is one of an armed quarantine only: a row armed before migration 4
# (gh1001, #1180) has no owner, is never RED at its end (`dispatch_expired`) and
# so writes no `flake_quarantine_expired` finding either (re-check tv-p1c-fix1 H1).
EXPIRED_SQL = ("quarantined_until IS NOT NULL AND quarantined_until <= ?"
               " AND owner IS NOT NULL AND fixed IS NULL")


def release_expired(db, at):
    """Every quarantine that ran out WITHOUT a measured fix: one `process_finding`
    per expiry, and the pass that dispatches it is RED (`dispatch_expired`, VG TV-F3).

    `quarantined_until` stays as a past date (the log is the audit); the
    finding `flake_quarantine_expired` is written once per test and expiry.
    Returns the tests whose expiry is new (the finding's once)."""
    out = []
    for row in db.execute("SELECT * FROM flakes WHERE " + EXPIRED_SQL + " ORDER BY test",
                          (at,)).fetchall():
        row = dict(row)
        if not quarantine_applies(row["test"]):
            continue        # a legacy entry without effect: nothing ran out
        if row.get("fixed"):
            continue        # a measured fix ended it (record_fix)
        known = db.execute(
            "SELECT 1 FROM journal WHERE event='process_finding'"
            " AND json_extract(data, '$.rule')='flake_quarantine_expired'"
            " AND json_extract(data, '$.subject')=? AND json_extract(data, '$.since')=?",
            (row["test"], row["quarantined_until"])).fetchone()
        if known:
            continue
        _events.emit(db, "process_finding", "finding",
                     {"rule": "flake_quarantine_expired", "subject": row["test"],
                      "value": round(rate(row), 4), "threshold": RATE_THRESHOLD,
                      "since": row["quarantined_until"], "issue": row["issue"]}, at=at)
        out.append(row["test"])
    return out


def dispatch_expired(db, at):
    """The dispatch's `quarantine_expired`: tests whose quarantine ran out without
    a measured fix -- the station `quarantine` ends RED on them (VG TV-F3)."""
    return [t for (t,) in db.execute(
        "SELECT test FROM flakes WHERE " + EXPIRED_SQL + " ORDER BY test", (at,))
        if quarantine_applies(t)]


# --- the NOTE station `quarantine` (VG TV-F3) -------------------------------------------------

QUARANTINE_STATION = "quarantine"
# The profiles the run's `quarantine.toml` gives its retry entries: the blocking
# default (where the tests are excluded anyway) and the NOTE station's own.
QUARANTINE_PROFILES = ("default", QUARANTINE_STATION)
TIER_EXCLUDE_ENV = "MECLAW_TIER_EXCLUDE"


def quarantine_filter(entries, known_binaries=None):
    """The nextest filterset of the quarantined tests; an entry whose binary is not
    in `known_binaries` is dropped (nextest refuses the whole filter on it)."""
    terms = []
    for e in entries:
        split = split_test(e["test"], known_binaries or ())
        if split is None or not _NEXTEST_NAME.match(split[1]):
            continue
        terms.append("binary_id(=%s) & test(=%s)" % split)
    return " | ".join("(%s)" % t for t in terms) if len(terms) > 1 else "".join(terms)


def quarantine_station(tests_station, entries, known_binaries=None):
    """The NOTE station `quarantine`: the blocking `tests` station's shape, its
    command the quarantined tests (`scripts/test-tier.sh filter`); the executor
    narrows the filter to the binaries of its build. `known_binaries`: the
    plan's binaries (the `tests` station's `parts`)."""
    st = json.loads(json.dumps(tests_station))
    st["name"] = QUARANTINE_STATION
    st["cmds"] = [{"argv": ["scripts/test-tier.sh", "filter",
                            quarantine_filter(entries, known_binaries)],
                   "cwd": ""}]
    st["reason"] = ["quarantine: %d test(s), NOTE (VG TV-F3)" % len(entries)]
    # its own nextest profile (`.config/nextest.toml`: retries with
    # `flaky-result = "fail"`, its own JUnit file beside the `tests` station's)
    st["nextest"] = dict(st.get("nextest") or {}, profile=QUARANTINE_STATION)
    for key in ("parallel", "parts"):
        st.pop(key, None)
    st["inputs"] = list(st.get("inputs") or [])
    return st


def quarantine_station_verdict(rc, expired):
    """(verdict, reason) of the station `quarantine`: NOTE on red, never RED --
    except a quarantine that ran out without a measured fix (VG TV-F3)."""
    if expired:
        return "RED", "quarantine expired without a measured fix: " + ", ".join(expired)
    if rc == 0:
        return "GREEN", ""
    return "NOTE", "a quarantined test is red (exit %s) -- a finding, not a gate" % rc


# --- the legacy list (VG TV-F4) ----------------------------------------------------------------

def legacy_entries(repo):
    """[{test, issue, cause}] of `.config/flaky-legacy.toml` (`[[flaky]]` tables);
    an entry without an issue number or a cause never counts (fail-closed)."""
    try:
        data = tomllib.loads((pathlib.Path(repo) / LEGACY_FILE).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return []
    out = []
    for e in data.get("flaky") or []:
        if (isinstance(e, dict) and isinstance(e.get("test"), str) and e["test"]
                and isinstance(e.get("issue"), int) and e["issue"] > 0
                and isinstance(e.get("cause"), str) and e["cause"]):
            out.append({"test": e["test"], "issue": e["issue"], "cause": e["cause"]})
    return out


_HUNK = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@", re.M)


def changed_hunks(repo, base):
    """{path: [(first, last)] new-side lines} of `git diff -U0 <base>` (the tree
    against the base, dirty files included); a deleted file maps to None."""
    proc = subprocess.run(["git", "-C", str(repo), "diff", "-U0", "--no-renames", "--no-color",
                           "--no-ext-diff", base], capture_output=True, text=True, check=True)
    out, path, old = {}, None, None
    for line in proc.stdout.splitlines():
        if line.startswith("--- "):
            old = line[6:] if line.startswith("--- a/") else None
        elif line.startswith("+++ "):
            path = line[6:] if line.startswith("+++ b/") else None
            if path is None and old:
                out[old] = None
            elif path is not None:
                out.setdefault(path, [])
        elif path is not None and line.startswith("@@"):
            m = _HUNK.match(line)
            if m:
                first, n = int(m.group(1)), int(m.group(2) or 1)
                out[path].append((first, first + max(n, 1) - 1))
    return out


def _test_fn(test):
    """(name, suffix) a test is spelled under: Rust `fn <last>` (.rs), Python
    `def <last>` (.py), a scenario case its id anywhere (None)."""
    if ":" + CASE in test or test.startswith(CASE):
        return case_id(test), None
    if "::" in test:
        return test.rsplit("::", 1)[1], ".rs"
    return test.rsplit(".", 1)[-1], ".py"


def _attr_top(lines, i, suffix):
    """The first line (0-based) of the attributes (`#[...]`, Rust) or decorators
    (`@...`, Python) right above the function head at `i` -- they belong to the
    test (re-check A-M2). Comments between them are passed over; a multi-line
    attribute is followed up to its start; a blank line ends the walk."""
    if suffix == ".rs":
        start = lambda t: t.startswith("#[")
        comment = lambda t: t.startswith("//")
        stop = lambda t: t.endswith(("{", "}", ";"))
    else:
        start = lambda t: t.startswith("@")
        comment = lambda t: t.startswith("#")
        stop = lambda t: t.endswith(":")
    top = j = i
    while j > 0:
        t = lines[j - 1].strip()
        if not t:
            break
        if start(t):
            top = j = j - 1
            continue
        if comment(t):
            j -= 1
            continue
        m = j - 1          # the inside of a multi-line attribute: find its start
        while m >= 0 and j - m <= 30:
            u = lines[m].strip()
            if not u or stop(u) or comment(u):
                m = -1
                break
            if start(u):
                break
            m -= 1
        if m < 0 or j - m > 30:
            break
        top = j = m
    return top


def fn_spans(text, name, suffix):
    """The 1-based (first, last) line spans of the functions `name` in `text`,
    their attributes/decorators included (`_attr_top`)."""
    lines = text.splitlines()
    spans = []
    if suffix == ".rs":
        head = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?fn\s+%s\s*[<(]"
                          % re.escape(name))
        for i, line in enumerate(lines):
            if not head.match(line):
                continue
            depth, opened, end = 0, False, len(lines)
            for j in range(i, len(lines)):
                for ch in lines[j]:
                    if ch == "{":
                        depth, opened = depth + 1, True
                    elif ch == "}":
                        depth -= 1
                if opened and depth <= 0:
                    end = j + 1
                    break
            spans.append((_attr_top(lines, i, suffix) + 1, end))
    else:
        head = re.compile(r"^(\s*)(?:async\s+)?def\s+%s\s*\(" % re.escape(name))
        for i, line in enumerate(lines):
            m = head.match(line)
            if not m:
                continue
            ind, end = len(m.group(1)), len(lines)
            for j in range(i + 1, len(lines)):
                if lines[j].strip() and len(lines[j]) - len(lines[j].lstrip()) <= ind:
                    end = j
                    break
            spans.append((_attr_top(lines, i, suffix) + 1, end))
    return spans


def _test_changed(repo, test, hunks, base):
    """True when the diff touches the test: a hunk inside its function, the file
    when the function cannot be found (fallback), a deleted file that named it in
    `base` (any other deleted file is passed over -- re-check A-M1)."""
    name, suffix = _test_fn(test)
    for path, ranges in hunks.items():
        if suffix is not None and not path.endswith(suffix):
            continue
        if ranges is None:
            if suffix is None or _named_in_base(repo, base, path, name):
                return True
            continue
        try:
            text = (pathlib.Path(repo) / path).read_text(encoding="utf-8", errors="replace")
        except OSError:
            return True
        if name not in text:
            continue
        if suffix is None:
            return True
        spans = fn_spans(text, name, suffix)
        if not spans:
            return True
        if any(a <= e and s <= b for a, b in ranges for s, e in spans):
            return True
    return False


def _named_in_base(repo, base, path, name):
    """True when `path` named the test in `base` (the deleted file is only there);
    a grep that fails otherwise counts as named (fail-closed)."""
    proc = subprocess.run(["git", "-C", str(repo), "grep", "-q", "-F", name, base, "--", path],
                          capture_output=True)
    return proc.returncode != 1


def legacy_tests(repo, base):
    """The legacy tests that may count green in a pass against `base` (VG TV-F4):
    on `.config/flaky-legacy.toml` with issue and cause, and not new or changed
    in the diff (hunk in the test function, fallback the file). No base, or a
    diff that cannot be read: none (fail-closed)."""
    entries = legacy_entries(repo)
    if not base or not entries:
        return []
    try:
        hunks = changed_hunks(repo, base)
    except (OSError, subprocess.CalledProcessError):
        return []
    return sorted(e["test"] for e in entries if not _test_changed(repo, e["test"], hunks, base))


def dispatch_quarantine(db, at):
    """The dispatch's `quarantine` list (V0 section 6): [{test, issue, until}]."""
    return [{"test": t, "issue": int(i), "until": u} for t, i, u in db.execute(
        "SELECT test, issue, quarantined_until FROM flakes WHERE " + ARMED_SQL
        + " ORDER BY test", (at,)) if quarantine_applies(t)]


def dispatch_issues(db):
    """The dispatch's `flake_issues` `{test: issue}`: EVERY register row with an
    issue, quarantined or not. The station's FLAKY reason names it; without
    this a tracked flake outside the quarantine (a defect red in its window,
    or a unittest/scenario key) read `issue pending` (tb-s8, gh1001 -> #1180)."""
    return {t: int(i) for t, i in db.execute(
        "SELECT test, issue FROM flakes WHERE issue IS NOT NULL ORDER BY test")
        if str(i).isdigit()}


def _run_meta(db, run):
    row = db.execute("SELECT data FROM journal WHERE run=? AND event='run_dispatched'"
                     " ORDER BY seq LIMIT 1", (run,)).fetchone()
    d = json.loads(row[0]) if row and row[0] else {}
    return d.get("runner"), d.get("round") or 1


class RegisterError(Exception):
    """A FLAKY isolation whose test has no register row (welle-tb S3, #1161)."""


def ingest_run(db, run, station_items, at=None, gh=None, drafts=None, max_new=MAX_NEW_ISSUES):
    """Take the `flake_isolated` lines of `run` into the store.

    Per isolation: a task `T-<n>` (parent = the station's workpackage) with
    an edge `depends_on` to the station task, guard `when: {outcome: failure}`
    (Orga's grammar), one `task_runs` row with the verdict FLAKY|RED and the
    task's transition (done | waiting). Then `refresh`, the issue an
    isolation event names, the auto quarantine. With a `gh` runner the issue
    duty runs for every FLAKY test still without an issue. A FLAKY test that
    no `test_result` line of THIS run names raises `RegisterError` before
    any row is made, the quarantine and the issue duty -- a row of an earlier
    run is no observation of this one (re-check tb-s3-store2 m-1), and never
    the silent `not in the register` of `issue_for`. Idempotent per (run, test)."""
    at = at or _store.now()
    runner, rnd = _run_meta(db, run)
    tasks, flaky_tests, tests = [], [], []
    mine = [dict(d, test=qualify(d.get("station"), d["test"]))
            for _, _, r, d in _rows(db, "flake_isolated") if r == run]
    for d in mine:
        test, verdict = d["test"], d["verdict"]
        tests.append(test)
        if verdict == "FLAKY":
            flaky_tests.append(test)
        # One isolation, one transaction (welle-tb S3, #1161): a task whose
        # run row failed made a retry skip the test -- the row stayed lost.
        with _store.tx(db):
            if db.execute("SELECT 1 FROM items WHERE type='task'"
                          " AND json_extract(doc, '$.isolation.run')=?"
                          " AND json_extract(doc, '$.isolation.test')=?", (run, test)).fetchone():
                continue
            st = _store.item_get(db, station_items.get(d["station"])) if station_items.get(
                d["station"]) else None
            doc = {"isolation": {k: d.get(k) for k in ("station", "test", "count", "green",
                                                       "verdict", "issue")}}
            doc["isolation"]["run"] = run
            item = _store.item_create(db, "task", f"isolate {test}"[:200],
                                      parent=st["parent"] if st else None, doc=doc, at=at)
            if st:
                _store.edge_link(db, item["id"], st["id"], "depends_on", "pipeline",
                                 {"dep": "fs", "lag_min": 0, "when": {"outcome": "failure"}}, at=at)
            tr = _store.run_open(db, item["id"], runner, round=rnd, key=item["key"], at=at)
            state = _store.run_close(db, tr, verdict, at=at)
            v = _store.item_transition(db, item["id"], "open", "in_progress", item["version"], at=at)
            _store.item_transition(db, item["id"], "in_progress", _store.TASK_AFTER_RUN[state], v,
                                   reason=verdict.lower(), at=at)
        tasks.append(item["key"])
    # A test nextest retried to green (junit `flakyFailure`, `flaky_names` of a
    # `test_result`) has no isolation line: the station was green. `refresh`
    # counts it flaky, but only for the tests it is asked about -- without
    # this the register never saw it (GH #1162, Q15 of the measurement: an
    # empty register has to be a real 0, not a path that drops retries).
    for _, _, r, d in _rows(db, "test_result"):
        if r == run:
            tests.extend(test_key(d.get("part", ""), n) for n in d.get("flaky_names") or ())
    refresh(db, tests=tests, at=at)
    # Delta M2 (N1 in the issue duty): a test with a RED isolation in this run
    # is a defect of the run, never a flake -- no quarantine, no flake issue.
    red_tests = {d["test"] for d in mine if d["verdict"] == "RED"}
    flaky_tests = [t for t in dict.fromkeys(flaky_tests) if t not in red_tests]
    # Fail-closed (welle-tb S3, #1161): a FLAKY test no `test_result` line of
    # THIS run names was green-judged from names the journal never got -- no
    # quarantine, no issue duty, and the kern ends the pass ABORTED
    # (`gate._Pass.store`). A row of an earlier run or one an isolation's
    # issue would make is no observation (re-check tb-s3-store2 m-1), so the
    # check runs before the issue rows below.
    observed = {k for _, _, r, d in _rows(db, "test_result") if r == run
                for k in _observed(d)[1]}
    missing = [t for t in flaky_tests if t not in observed]
    if missing:
        raise RegisterError(
            "FLAKY test(s) without a register observation: %s -- no test_result line of "
            "run %s names them" % (", ".join(missing), run))
    with _store.tx(db):
        for d in mine:
            if d.get("issue") is not None:
                _ensure(db, d["test"])
                db.execute("UPDATE flakes SET issue=? WHERE test=? AND issue IS NULL",
                           (str(int(d["issue"])), d["test"]))
    proposed = auto_quarantine(db, flaky_tests, at)
    pending = [t for t in flaky_tests if not (get(db, t) or {}).get("issue")]
    issues = []
    if gh is not None:
        for t in pending:
            issues.append(issue_for(db, t, gh=gh, drafts=drafts, max_new=max_new, at=at))
        # VG TV-F9: a flake behind a closed `observation` issue is back -- the
        # issue path reopens that issue (`issue_for`), never a silent row.
        for t in flaky_tests:
            if t not in pending and _observation_closed(db, t, gh):
                issues.append(issue_for(db, t, gh=gh, drafts=drafts, max_new=max_new, at=at))
    return {"tasks": tasks, "proposed": proposed, "pending": pending, "issues": issues}


def _observation_closed(db, test, gh):
    """True when the test's register issue is CLOSED with the label `observation`."""
    row = get(db, test)
    if not row or not row["issue"] or (row["repo"] and row["repo"] != "meclaw"):
        return False
    try:
        got = json.loads(_gh_call(gh, ["gh", "issue", "view", str(int(row["issue"])),
                                       "--json", "state,labels"]))
    except Exception:  # noqa: BLE001 -- no gh: nothing to reopen, the row stays
        return False
    return got.get("state") == "CLOSED" and OBSERVATION_LABEL in {
        (lab or {}).get("name") for lab in got.get("labels") or ()}


# --- the issue duty ------------------------------------------------------------------------

ISSUE_LABEL = "flaky"
ISSUE_TITLE = "flaky: {test}"
ISSUE_BODY = (
    "The pipeline's flake register lists `{test}`.\n"
    "\n"
    "- runs: {runs}\n"
    "- reds: {reds}\n"
    "- flaky: {flaky}\n"
    "- rate: {rate}\n"
    "- isolation: {green}/{count} green\n"
    "- lane: {lane}\n"
    "\n"
    "Window: the last 20 runs of the test. Written by `pipeline flakes issue`;\n"
    "quarantine and expiry live in the pipeline store.\n")
# A unittest or scenario station names its tests only when it ends red, and
# no quarantine applies to them (re-check tb-s3-store2 m-2).
ISSUE_BODY_RED_ENDS = ISSUE_BODY.replace(
    "Window: the last 20 runs of the test. Written by `pipeline flakes issue`;\n"
    "quarantine and expiry live in the pipeline store.\n",
    "Window: the last 20 red ends of the test's station -- a unittest or scenario\n"
    "station names its tests only when it ends red, so the rate is per red end.\n"
    "Written by `pipeline flakes issue`; no quarantine: it takes nextest tests only.\n")
TEST_NAME = re.compile(r"^[A-Za-z0-9_][A-Za-z0-9_.:/-]{0,199}$")
LANE_ID = re.compile(r"^fp-(?:[0-9a-f]{8}|none)$")

# The R5 name patterns are the export audit's own `NAME_PATTERNS`, read at run
# time (ast, never imported) -- no copy here. The audit lives under `plans/`,
# which never travels: without it no issue is filed, only a draft (fail-closed).
R5_AUDIT = REPO / "plans" / "export-fixtures" / "make_export.py"
R5_UNAVAILABLE = "r5 patterns unavailable"
# Beyond names: what must never reach a public issue from a lane.
R5_PRIVATE_PATTERNS = (
    r"(?<![\d.])(?!127\.0\.0\.1(?![\d.]))(?:\d{1,3}\.){3}\d{1,3}(?![\d.])",  # IPv4, not loopback
    r"(?:^|[\s`'\"(=])(?:/home/|/root/|/srv/|/opt/|/Users/|~/)",          # private paths
    r"\b[a-z0-9-]+\.(?:lan|local|internal|home|corp)\b",                  # internal host names
)


def r5_name_patterns():
    """The audit's `NAME_PATTERNS` (first items), or None when absent or unreadable."""
    import ast
    try:
        tree = ast.parse(R5_AUDIT.read_text(encoding="utf-8"))
        for node in tree.body:
            if isinstance(node, ast.Assign) and any(
                    getattr(t, "id", None) == "NAME_PATTERNS" for t in node.targets):
                patterns = tuple(p for p, _ in ast.literal_eval(node.value))
                for p in patterns:
                    re.compile(p)
                return patterns if patterns and all(
                    isinstance(p, str) and p for p in patterns) else None
    except (OSError, SyntaxError, ValueError, TypeError, re.error):
        return None
    return None


def r5_hits(text):
    """The private patterns found in `text` (case-insensitive, as the audit greps).

    Without the audit's name patterns every text is a hit (`R5_UNAVAILABLE`)."""
    names = r5_name_patterns()
    if names is None:
        return [R5_UNAVAILABLE]
    return [p for p in names + R5_PRIVATE_PATTERNS
            if re.search(p, text, re.IGNORECASE | re.MULTILINE)]


def _count(value):
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise ValueError(f"not a count: {value!r}")
    return value


def render_issue(test, runs, reds, flaky, green, count, lane):
    """(title, body) from the fixed template -- a whitelisted name and numbers only."""
    if not isinstance(test, str) or not TEST_NAME.match(test):
        raise ValueError("test name outside the whitelist")
    if not isinstance(lane, str) or not LANE_ID.match(lane):
        raise ValueError("lane id outside the whitelist")
    nums = {k: _count(v) for k, v in (("runs", runs), ("reds", reds), ("flaky", flaky),
                                      ("green", green), ("count", count))}
    r = nums["flaky"] / nums["runs"] if nums["runs"] else 0.0
    body = ISSUE_BODY if quarantine_applies(test) else ISSUE_BODY_RED_ENDS
    return (ISSUE_TITLE.format(test=test),
            body.format(test=test, rate=f"{r:.2f}", lane=lane, **nums))


def _last_isolation(db, test):
    row = db.execute("SELECT run, data FROM journal WHERE event='flake_isolated'"
                     " AND json_extract(data, '$.test')=? ORDER BY seq DESC LIMIT 1",
                     (test,)).fetchone()
    return (row[0], json.loads(row[1])) if row else (None, {})


def lane_short(db, run):
    """`fp-<8hex>` of the lane fingerprint of `run` (a hash, never a host name)."""
    row = db.execute("SELECT data FROM journal WHERE run=? AND event='lane_fingerprint'"
                     " ORDER BY seq DESC LIMIT 1", (run,)).fetchone() if run else None
    if not row or not row[0]:
        return "fp-none"
    import hashlib
    return "fp-" + hashlib.sha256(json.dumps(json.loads(row[0]), sort_keys=True)
                                  .encode()).hexdigest()[:8]


def default_drafts():
    """`$MECLAW_FLAKE_DRAFTS`, else the tree's gate folder `<target>/gate/<tree>/flakes`.

    Never inside the tree (tb-s10 point 19): `plans/flakes/issue-*.md` in a
    strand worktree is an untracked file, moves `pre`'s tree hash and broke
    the GREEN stamp after every FLAKY station with a draft."""
    env = os.environ.get(DRAFTS_ENV)
    if env:
        return pathlib.Path(env)
    from . import gate as _gate  # gate imports this module: late, not at the top
    return pathlib.Path(_gate.target_dir(str(REPO))) / "gate" / REPO.name / "flakes"


def _draft(drafts, test, title, body, reason):
    drafts = pathlib.Path(drafts)
    drafts.mkdir(parents=True, exist_ok=True)
    path = drafts / ("issue-%s.md" % re.sub(r"[^A-Za-z0-9._-]", "_", test))
    path.write_text(f"<!-- draft: {reason} -- not filed; to be filed or dropped by hand -->\n"
                    f"# {title}\n\nlabels: {ISSUE_LABEL}\n\n{body}", encoding="utf-8")
    return {"action": "draft", "test": test, "reason": reason, "draft": str(path)}


def _take_daily(db, at, cap):
    """Compare-and-set on `counters` `flake_issues:<date>`; False at the cap."""
    name = f"flake_issues:{at[:10]}"
    with _store.tx(db):
        row = db.execute("SELECT value FROM counters WHERE name=?", (name,)).fetchone()
        value = row[0] if row else 0
        if value >= cap:
            return False
        if row is None:
            db.execute("INSERT INTO counters VALUES (?, 1)", (name,))
        else:
            cur = db.execute("UPDATE counters SET value=? WHERE name=? AND value=?",
                             (value + 1, name, value))
            if cur.rowcount != 1:
                return False
    return True


def _remember(db, test, issue):
    with _store.tx(db):
        db.execute("UPDATE flakes SET issue=? WHERE test=?", (str(int(issue)), test))


class _GhFailed(Exception):
    pass


def _gh_call(gh, argv):
    rc, out = gh(argv)
    if rc != 0:
        raise _GhFailed(argv[1:3])
    return out


def _issue_number(out):
    m = re.search(r"/issues/(\d+)", out or "")
    if not m:
        raise _GhFailed("no issue number")
    return int(m.group(1))


def _gh(argv):
    """The default runner: the owner's `gh`, 60 s per call -- never under the
    test switch `MECLAW_FLAKE_ISSUES=0` (Delta M3: a second layer before a
    public issue)."""
    if os.environ.get(ISSUES_ENV) == "0":
        return 1, ""
    r = subprocess.run(argv, capture_output=True, text=True, timeout=60, check=False)
    return r.returncode, r.stdout


GH_RUNNER = _gh


def issue_for(db, test, gh=None, drafts=None, max_new=MAX_NEW_ISSUES, dry_run=False, at=None):
    """The issue a flaky test owes: comment on it, or open it -- or a draft.

    Locks, in order: (1) the fixed template with a whitelisted name and
    numbers only; (2) the private-pattern scan of title and body -- the
    audit's name patterns read at run time, none readable -> a draft; (3) one
    issue per test -- the register's issue first, else the exact title; open
    -> comment, closed -> a new issue naming `Previous: #<n>`; (4) a daily cap
    on NEW issues (`counters`). Any bite, a missing `gh` or a failing call
    writes the draft `issue-<test>.md`. A store refusal (`StoreWriteError`,
    `sqlite3.Error`) raises: the kern ends the pass ABORTED, the hand
    `flakes issue` ends with one line and exit 2 (re-check tb-s3-store2 m-n1)."""
    at = at or _store.now()
    gh = gh or GH_RUNNER
    drafts = drafts or default_drafts()
    row = get(db, test)
    if row is None:
        return {"action": "none", "test": test, "reason": "not in the register"}
    if row["repo"] and row["repo"] != "meclaw":
        # The issue number belongs to the satellite; `gh` here speaks to meclaw.
        return {"action": "skip", "test": test, "reason": f"tracked in {row['repo']}"}
    run, iso = _last_isolation(db, test)
    try:
        title, body = render_issue(test, row["runs"] or 0, row["reds"] or 0, row["flaky"] or 0,
                                   int(iso.get("green") or 0), int(iso.get("count") or 0),
                                   lane_short(db, run))
    except ValueError:
        return _draft(drafts, test, f"flaky: {test}", "", "name outside the whitelist")
    hits = r5_hits(title + "\n" + body)
    if hits == [R5_UNAVAILABLE]:
        return _draft(drafts, test, title, body, R5_UNAVAILABLE)
    if hits:
        return _draft(drafts, test, title, body, "r5 scan hit")
    if dry_run:
        return {"action": "dry-run", "test": test, "title": title, "body": body}
    try:
        previous = None
        if row["issue"]:
            known = int(row["issue"])
            got = json.loads(_gh_call(gh, ["gh", "issue", "view", str(known),
                                           "--json", "state,labels"]))
            state = got["state"]
            if state == "OPEN":
                _gh_call(gh, ["gh", "issue", "comment", str(known), "--body", body])
                return {"action": "comment", "test": test, "issue": known}
            if OBSERVATION_LABEL in {(lab or {}).get("name") for lab in got.get("labels") or ()}:
                # VG TV-F9: not reproduced was an observation -- it is back.
                _gh_call(gh, ["gh", "issue", "reopen", str(known), "--comment", body])
                _store.retry_write(_events.emit, db, "process_finding", "finding",
                                   {"rule": "flake_issue_reopened", "subject": test,
                                    "value": round(rate(row), 4), "threshold": RATE_THRESHOLD,
                                    "since": at, "issue": known,
                                    "text": f"reopened #{known}"}, at=at)
                return {"action": "reopen", "test": test, "issue": known}
            previous = known
        else:
            found = json.loads(_gh_call(gh, [
                "gh", "issue", "list", "--state", "all", "--search", f'"{title}" in:title',
                "--json", "number,title,state", "--limit", "50"]))
            same = [i for i in found if i.get("title") == title]
            open_ = sorted(i["number"] for i in same if i.get("state") == "OPEN")
            if open_:
                _gh_call(gh, ["gh", "issue", "comment", str(open_[0]), "--body", body])
                _store.retry_write(_remember, db, test, open_[0])
                return {"action": "comment", "test": test, "issue": open_[0]}
            if same:
                previous = max(i["number"] for i in same)
        if not _store.retry_write(_take_daily, db, at, max_new):
            return _draft(drafts, test, title, body, "daily cap")
        if previous:
            body += f"\nPrevious: #{previous}\n"
        n = _issue_number(_gh_call(gh, ["gh", "issue", "create", "--title", title,
                                        "--body", body, "--label", ISSUE_LABEL]))
        _store.retry_write(_remember, db, test, n)
        return {"action": "create", "test": test, "issue": n}
    except (_store.StoreWriteError, sqlite3.Error):
        # Re-check tb-s3-store M-1 (#1161): a store refusal is no "gh
        # unavailable" -- swallowed, the issue number never reached the
        # register and the pass ended GREEN (probe P3). The bounded retry is
        # in here, around each write; StoreWriteError is no OperationalError,
        # so the gate's outer retry never repeats a `gh issue create`.
        raise
    except Exception:  # noqa: BLE001 -- no gh, no auth, a failed call: a draft, never red
        return _draft(drafts, test, title, body, "gh unavailable")


# --- the pass share (retro Q14, panel 6) ------------------------------------------------------

def pass_share(db):
    """Red passes and passes a flake turned green: {share, passes_red, passes_flaky}.

    A pass with a FLAKY isolation that still ended RED is a red pass, not a
    flaky one (M1)."""
    red = {run for _, _, run, d in _rows(db, "pass_ended") if d.get("verdict") == "RED"}
    flaky = {run for _, _, run, d in _rows(db, "flake_isolated")
             if d.get("verdict") == "FLAKY"} - red
    both = red | flaky
    return {"share": len(flaky) / len(both) if both else 0.0,
            "passes_red": len(red), "passes_flaky": len(flaky)}


# --- CLI -----------------------------------------------------------------------------------

def _open(args, write):
    try:
        return _store.open(args.db) if write else _store.open_ro(args.db)
    except FileNotFoundError as exc:
        print(f"flakes: {exc}", file=sys.stderr)
        return None


def _cmd_flakes(args):
    write = args.flakes_cmd in ("quarantine", "release", "issue", "register", "fixed")
    db = _open(args, write)
    if db is None:
        return 1
    try:
        return _COMMANDS[args.flakes_cmd](db, args)
    finally:
        _store.close(db)


def _list(db, args):
    at = _store.now()
    rows = [dict(r) for r in db.execute("SELECT * FROM flakes ORDER BY test")]
    rows.sort(key=lambda r: (-rate(r), -(r["reds"] or 0), r["test"]))
    for r in rows:
        print(f"{r['test']}  runs={r['runs']} reds={r['reds']} flaky={r['flaky']}"
              f" rate={rate(r):.2f} issue={r['issue'] or '-'}"
              f" until={r['quarantined_until'] or '-'}  {status(r, at)}")
    print(f"FLAKES tests={len(rows)}")
    return 0


def _show(db, args):
    row = get(db, args.test)
    if row is None:
        print(f"flakes: {args.test} is not in the register", file=sys.stderr)
        return 1
    row["rate"] = round(rate(row), 4)
    row["status"] = status(row, _store.now())
    print(json.dumps(row, indent=2, sort_keys=True))
    return 0


def _rate(db, args):
    s = pass_share(db)
    n = db.execute("SELECT count(*), coalesce(sum(flaky > 0), 0) FROM flakes").fetchone()
    print(f"FLAKES share={s['share']:.2f} passes_red={s['passes_red']}"
          f" passes_flaky={s['passes_flaky']} tests={n[0]} flaky_tests={n[1]}")
    return 0


def _quarantine(db, args):
    try:
        until = quarantine(db, args.test, args.issue, owner=args.owner, cause=args.cause,
                           days=args.days)
    except ValueError as exc:   # QuarantineFull among them: RED for the caller
        print(f"flakes: {exc}", file=sys.stderr)
        return 2
    print(f"FLAKES quarantine {args.test} issue=#{args.issue} until={until}")
    return 0


def _fixed(db, args):
    try:
        note = record_fix(db, args.test, args.quiet, args.load, evidence=args.evidence)
    except ValueError as exc:
        print(f"flakes: {exc}", file=sys.stderr)
        return 2
    print(f"FLAKES fixed {args.test} {note}")
    return 0


def _release(db, args):
    if not release(db, args.test):
        print(f"flakes: {args.test} is not in the register", file=sys.stderr)
        return 1
    print(f"FLAKES release {args.test}")
    return 0


def _issue(db, args):
    try:
        res = issue_for(db, args.test, gh=GH_RUNNER, drafts=args.drafts,
                        max_new=args.max_new, dry_run=args.dry_run)
    except (_store.StoreWriteError, sqlite3.Error) as exc:
        print(f"flakes: store: {type(exc).__name__}: {exc}", file=sys.stderr)
        return 2
    print(f"FLAKES issue {args.test} action={res['action']}"
          + (f" issue=#{res['issue']}" if res.get("issue") else "")
          + (f" reason={res['reason']!r}" if res.get("reason") else "")
          + (f" draft={res['draft']}" if res.get("draft") else ""))
    if res["action"] == "dry-run":
        print(res["title"])
        print(res["body"], end="")
    return 0 if res["action"] != "none" else 1


def _register(db, args):
    try:
        register_external(db, args.test, args.repo, args.issue, args.evidence)
    except ValueError as exc:
        print(f"flakes: {exc}", file=sys.stderr)
        return 2
    print(f"FLAKES registered {args.test} repo={args.repo} issue=#{args.issue}")
    return 0


_COMMANDS = {"list": _list, "show": _show, "rate": _rate, "quarantine": _quarantine,
             "release": _release, "issue": _issue, "register": _register, "fixed": _fixed}


def register(sub):
    """`pipeline flakes list|show|rate|quarantine|release|issue|register` (V0 section 9)."""
    p = sub.add_parser("flakes", help="the flake register, quarantine and issue duty (W8)")
    p.add_argument("--db", default=None, help="store path (default $MECLAW_PIPELINE_DB)")
    fs = p.add_subparsers(dest="flakes_cmd", required=True)
    fs.add_parser("list", help="every test that was red or flaky, worst rate first")
    fs.add_parser("show", help="one register row").add_argument("test")
    fs.add_parser("rate", help="flake share of red passes, register size")
    q = fs.add_parser("quarantine", help="quarantine a test under its issue")
    q.add_argument("test")
    q.add_argument("--issue", type=int, required=True)
    q.add_argument("--days", type=int, default=QUARANTINE_DAYS)
    q.add_argument("--owner", required=True, help="who answers for the quarantine")
    q.add_argument("--cause", required=True, help="cause class: " + ", ".join(CAUSES + (CAUSE_T2,)))
    fx = fs.add_parser("fixed", help="record a measured fix (VG TV-F6)")
    fx.add_argument("test")
    fx.add_argument("--quiet", type=int, required=True, help=f"green quiet runs (>= {FIX_QUIET})")
    fx.add_argument("--load", type=int, required=True, help=f"green loaded runs (>= {FIX_LOAD})")
    fx.add_argument("--evidence", default="", help="where the runs are (receipt, log)")
    fs.add_parser("release", help="end a quarantine now").add_argument("test")
    i = fs.add_parser("issue", help="comment on or open the issue a flaky test owes")
    i.add_argument("test")
    i.add_argument("--max-new", type=int, default=MAX_NEW_ISSUES)
    i.add_argument("--drafts", default=None, help=f"draft folder (default ${DRAFTS_ENV})")
    i.add_argument("--dry-run", action="store_true")
    r = fs.add_parser("register", help="register a flake seen outside this journal (#1135)")
    r.add_argument("test")
    r.add_argument("--repo", required=True)
    r.add_argument("--issue", type=int, required=True)
    r.add_argument("--evidence", required=True, help="where the red was seen (file:lines)")
    p.set_defaults(func=_cmd_flakes)
