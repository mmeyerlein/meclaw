"""Tests for `scripts/pipeline/flakes.py` -- the flake register, the isolation of
red tests, the runtime quarantine and the issue duty (wave dev-tempo, W8).

Every store here is a TempDir file opened with `store.open(path)` -- never the
shared one. No test talks to GitHub: the issue code takes its `gh` as an
injected runner, and every test hands it a fake that records the argv.

The names of the tests are the contract of the plan part (W8 "Tests"); the
fixtures under `fixtures/flakes/` are anonymized (no hosts, no addresses, no
names).
"""

import ast
import contextlib
import datetime
import hashlib
import importlib
import io
import json
import os
import pathlib
import re
import sqlite3
import subprocess
import sys
import tempfile
import tomllib
import unittest
from unittest import mock

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO))

from scripts.pipeline import (cache, cli, events, executor, flakes, gate, hang_dump,  # noqa: E402
                              metrics, receipt, store)
from scripts.tests import store_isolation  # noqa: E402

FIX = pathlib.Path(__file__).resolve().parent / "fixtures" / "flakes"
AT = "2026-10-08T12:00:00Z"
BIN = "example-cells::gh1_a_window_closes"
# gate-selftest loads every module into one process: no fixture event of this
# module may reach the metrics URL of the caller (test_pipeline_metrics checks it).
_METRICS_URL = mock.patch.dict(os.environ, {"MECLAW_PIPELINE_METRICS_URL": ""})

# The R5 name patterns live in the export audit under `plans/`, which never
# travels: every test here reads a sample audit instead, so the module runs the
# same in the private and the public tree.
SAMPLE_AUDIT = 'NAME_PATTERNS = [\n    (r"forbidden-sample|other-sample", "sample names"),\n]\n'
_AUDIT = {}   # made in setUpModule: gate-selftest imports every module before any test runs


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()   # the store, the side-effect guard, no issue duty
    _METRICS_URL.start()
    _AUDIT["dir"] = tempfile.TemporaryDirectory()
    audit = pathlib.Path(_AUDIT["dir"].name) / "make_export.py"
    audit.write_text(SAMPLE_AUDIT)
    _AUDIT["patch"] = mock.patch.object(flakes, "R5_AUDIT", audit)
    _AUDIT["patch"].start()


def tearDownModule():  # noqa: N802 -- unittest's name
    _AUDIT.pop("patch").stop()
    _METRICS_URL.stop()
    _AUDIT.pop("dir").cleanup()
    store_isolation.restore()


ITERATIONS = "".join(f"  Stress test [   0.001s] iteration {i}/10: 1 test run: 1 passed, 0 skipped\n"
                     for i in range(1, 11))
# The triage (OR-TV-2) reads the single-run summary; the stress summary stays for
# `stress_verdict`, which `scripts/flake_triage.py` still uses.
GREEN_SUMMARY = (ITERATIONS + "     Summary [   0.015s] 10/10 stress run iterations: 10 passed\n"
                 "     Summary [   0.001s] 1 test run: 1 passed, 0 skipped\n")


def ran_yes(test, rc, output):
    """The proof of a run the executor reads off `last_run.json` -- here always there."""
    return True
RED_SUMMARY = ("     Summary [   0.015s] 10/10 stress run iterations: 9 passed, 1 failed\n"
               "     Summary [   0.001s] 1 test run: 0 passed, 1 failed, 0 skipped\n")


def at_n(n):
    """A distinct, ordered RFC 3339 time per observation."""
    return f"2026-10-{1 + n // 24:02d}T{n % 24:02d}:00:00Z"


class FakeRun:
    """The injected `run_cmd(argv) -> (rc, output)`; records every argv."""

    def __init__(self, answer):
        self.answer = answer
        self.calls = []

    def __call__(self, argv):
        self.calls.append(list(argv))
        return self.answer(argv) if callable(self.answer) else self.answer


class FakeGh:
    """The injected `gh` runner: canned answers per verb, every argv recorded."""

    def __init__(self, issues=(), fail=False):
        self.issues = {i["number"]: dict(i) for i in issues}
        self.fail = fail
        self.calls = []
        self.next = 500

    def __call__(self, argv):
        self.calls.append(list(argv))
        if self.fail:
            return 1, ""
        verb = argv[1:3]
        if verb == ["issue", "view"]:
            n = int(argv[3])
            return 0, json.dumps({"state": self.issues[n]["state"],
                                  "labels": self.issues[n].get("labels", [])})
        if verb == ["issue", "list"]:
            return 0, json.dumps(list(self.issues.values()))
        if verb == ["issue", "reopen"]:
            self.issues[int(argv[3])]["state"] = "OPEN"
            return 0, ""
        if verb == ["issue", "comment"]:
            return 0, "https://example.invalid/issues/%s#comment" % argv[3]
        if verb == ["issue", "create"]:
            self.next += 1
            title = argv[argv.index("--title") + 1]
            self.issues[self.next] = {"number": self.next, "title": title, "state": "OPEN"}
            return 0, "https://example.invalid/issues/%d\n" % self.next
        return 1, ""

    def verbs(self):
        return [c[2] for c in self.calls]


class StoreCase(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = pathlib.Path(self.tmp.name)
        self.db = store.open(self.root / "pipeline.db")
        self.addCleanup(store.close, self.db)
        self.drafts = self.root / "drafts"

    # -- journal helpers ----------------------------------------------------
    def pass_planned(self, run, at):
        events.emit(self.db, "pass_planned", "status",
                    {"run": run, "mode": "strand", "repo": "meclaw", "rev": "abc",
                     "base": "abd", "stations": ["tests"], "reason": {}, "cache_known": 0,
                     "dirty": False}, run=run, at=at)

    def result_row(self, run, part, failed=(), flaky=(), at=AT, station="tests"):
        data = {"run": run, "station": station, "part": part,
                "verdict": "RED" if failed else "GREEN", "secs": 1.0}
        if failed:
            data["failed_names"] = list(failed)
        if flaky:
            data["flaky_names"] = list(flaky)
        events.emit(self.db, "test_result", "note", data, run=run, at=at)

    def isolated(self, run, test, verdict, at=AT, issue=None, station="tests"):
        data = {"run": run, "station": station, "test": test, "count": 10,
                "green": 10 if verdict == "FLAKY" else 7, "verdict": verdict}
        if issue is not None:
            data["issue"] = issue
        events.emit(self.db, "flake_isolated", "finding", data, run=run, at=at)

    def row(self, test):
        r = self.db.execute("SELECT * FROM flakes WHERE test=?", (test,)).fetchone()
        return dict(r) if r else None

    def observe(self, n, test_name, outcome, issue=None):
        """Observation n of `BIN::test_name`: pass, red or flaky (red + green isolation)."""
        run = f"run-{n:012x}"
        if outcome == "pass":
            self.result_row(run, BIN, at=at_n(n))
        else:
            self.result_row(run, BIN, failed=[test_name], at=at_n(n))
            self.isolated(run, f"{BIN}::{test_name}",
                          "FLAKY" if outcome == "flaky" else "RED", at=at_n(n), issue=issue)
        return run


# --- the register ---------------------------------------------------------------------

class TheRegister(StoreCase):
    SAT = ("tests.lab_verbund.test_nl_show.ShowLab."
           "test_the_verbund_topic_reaches_the_screen_in_the_member_round")

    def test_a_satellite_flake_registers_with_repo_issue_and_evidence(self):
        """#1135: a flake seen in a satellite repo is registered by hand."""
        flakes.register_external(self.db, self.SAT, repo="soul2me", issue=78,
                                 evidence="lanes/dt-w10/g7.out:160-165", at=at_n(1))
        row = flakes.get(self.db, self.SAT)
        self.assertEqual((row["repo"], row["issue"], row["evidence"]),
                         ("soul2me", "78", "lanes/dt-w10/g7.out:160-165"))
        self.assertEqual(flakes.status(row, at_n(2)), "tracked")
        self.assertEqual(flakes.refresh(self.db, at=at_n(2)), [])
        self.assertEqual(flakes.get(self.db, self.SAT)["repo"], "soul2me")

    def test_a_satellite_registration_needs_repo_issue_and_evidence(self):
        for kw in ({"repo": "", "issue": 78, "evidence": "x"},
                   {"repo": "soul2me", "issue": None, "evidence": "x"},
                   {"repo": "soul2me", "issue": 78, "evidence": ""}):
            with self.assertRaises(ValueError, msg=kw):
                flakes.register_external(self.db, self.SAT, **kw)
        self.assertIsNone(flakes.get(self.db, self.SAT))

    def test_the_issue_duty_never_touches_a_satellite_issue_in_this_repo(self):
        flakes.register_external(self.db, self.SAT, repo="soul2me", issue=78,
                                 evidence="e", at=at_n(1))
        gh = FakeGh(issues=[{"number": 78, "title": "x", "state": "OPEN"}])
        res = flakes.issue_for(self.db, self.SAT, gh=gh, drafts=self.drafts, at=at_n(2))
        self.assertEqual((res["action"], gh.calls), ("skip", []))

    def test_the_cli_registers_a_satellite_flake(self):
        with contextlib.redirect_stdout(io.StringIO()):
            rc = cli.main(["flakes", "--db", str(self.root / "pipeline.db"), "register",
                           self.SAT, "--repo", "soul2me", "--issue", "78", "--evidence", "e"])
        self.assertEqual(rc, 0)
        self.assertEqual(flakes.get(self.db, self.SAT)["evidence"], "e")

    def test_register_counts_runs_reds_flaky_from_junit(self):
        parts = flakes.junit_parts(FIX / "junit-flaky.xml")
        by_part = {p["part"]: p for p in parts}
        self.assertEqual(by_part[BIN]["failed_names"], ["the_window_closes_on_time"])
        self.assertEqual(by_part[BIN]["flaky_names"], ["tests::the_window_reopens"])
        self.assertEqual(by_part[BIN]["tests"],
                         {"total": 3, "passed": 1, "failed": 1, "flaky": 1, "skipped": 1})
        run1, run2 = "run-000000000001", "run-000000000002"
        for p in parts:
            events.emit(self.db, "test_result", "note", dict(p, run=run1, station="tests"),
                        run=run1, at=at_n(1))
        self.isolated(run1, f"{BIN}::the_window_closes_on_time", "RED", at=at_n(1))
        self.result_row(run2, BIN, at=at_n(2))   # the next run is green throughout
        flakes.refresh(self.db, at=at_n(3))

        red = self.row(f"{BIN}::the_window_closes_on_time")
        self.assertEqual((red["runs"], red["reds"], red["flaky"]), (2, 1, 0))
        flaky = self.row(f"{BIN}::tests::the_window_reopens")
        self.assertEqual((flaky["runs"], flaky["reds"], flaky["flaky"]), (2, 0, 1))
        self.assertEqual(flaky["station"], "tests")
        self.assertEqual(flaky["repo"], None)            # no pass_planned in this journal
        self.assertEqual((flaky["first_at"], flaky["last_at"]), (at_n(1), at_n(1)))
        # never red, never flaky -> no row (the register is not a test list)
        self.assertIsNone(self.row("example-cells::tests::a_quick_unit"))
        self.assertIsNone(self.row(f"{BIN}::a_skipped_case"))

    def test_junit_tests_reads_outcomes_and_slow_tests(self):
        tests = {(t["binary_id"], t["name"]): t for t in flakes.junit_tests(FIX / "junit-flaky.xml")}
        self.assertEqual(tests[(BIN, "the_window_closes_on_time")]["outcome"], "failed")
        self.assertEqual(tests[(BIN, "tests::the_window_reopens")]["outcome"], "flaky")
        self.assertEqual(tests[(BIN, "a_skipped_case")]["outcome"], "skipped")
        self.assertEqual(tests[("example-cells", "tests::a_quick_unit")]["outcome"], "passed")
        slow = flakes.slow_tests(FIX / "junit-flaky.xml")
        self.assertEqual([(s["binary_id"], s["name"]) for s in slow],
                         [("example-cells", "tests::a_slow_unit")])

    def test_register_window_is_last_20_runs(self):
        for n in range(5):
            self.observe(n, "a_test", "red")
        for n in range(5, 25):
            self.observe(n, "a_test", "pass")
        flakes.refresh(self.db, at=at_n(30))
        row = self.row(f"{BIN}::a_test")
        self.assertEqual((row["runs"], row["reds"], row["flaky"]), (20, 0, 0))
        self.assertEqual(row["first_at"], at_n(0))   # the history stays, the window moves
        self.assertEqual(row["last_at"], at_n(4))
        self.observe(25, "a_test", "flaky")
        flakes.refresh(self.db, at=at_n(30))
        row = self.row(f"{BIN}::a_test")
        self.assertEqual((row["runs"], row["reds"], row["flaky"]), (20, 0, 1))
        self.assertAlmostEqual(flakes.rate(row), 0.05)


# --- isolation (lane side) -------------------------------------------------------------

def red_result(names, part=BIN, station="tests"):
    return {"station": station, "part": part, "verdict": "RED", "secs": 3.0,
            "failed_names": list(names)}


ENDED = {"station": "tests", "verdict": "RED", "secs": 12.5, "exit": 100, "scope": "1 binary"}


class TheIsolation(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)

    def isolate(self, results, answer, kind="nextest", issues=None, **kw):
        run = FakeRun(answer)
        out = flakes.triage_station("tests", kind, results, run, issues or {},
                                     base_argv=["cargo-nextest", "nextest", "run"], **kw)
        return out, run

    def test_stress_verdict_reads_the_upstream_0_9_146_format(self):
        """#1137 (OR-DT.W8.S.2): the per-iteration and summary lines as
        nextest 0.9.146 prints them (its replay snapshot and site output at
        tag cargo-nextest-0.9.146; `completed` counts 1..N, singular `test`)."""
        lines = []
        for i in range(1, 6):
            lines += [f"Stress test iteration {i}/5 (HH:MM:SS elapsed so far, "
                      f"{6 - i} iterations remaining)",
                      f"        PASS [   0.004s] (1/1) fx::alpha one",
                      f"Stress test [   0.005s] iteration {i}/5: 1 test run: 1 passed, 31 skipped"]
        green = "\n".join(lines + ["     Summary [   0.030s] 5/5 stress run iterations: 5 passed"])
        self.assertEqual(flakes.stress_verdict(0, green, count=5), ("GREEN", 5))
        failed = green.replace("5/5 stress run iterations: 5 passed",
                               "5/5 stress run iterations: 4 passed, 1 failed")
        self.assertEqual(flakes.stress_verdict(0, failed, count=5), ("RED", 4))
        partial = green.replace("iteration 3/5: 1 test run", "iteration 3/5: 0/1 tests run")
        self.assertEqual(flakes.stress_verdict(0, partial, count=5)[0], "RED")

    def test_stress_verdict_reads_a_measured_lane_run(self):
        """#1137: the output of a real `--stress-count 10` run of nextest 0.9.146
        on a build lane (fixture, copied verbatim from the run's test.log)."""
        real = (pathlib.Path(__file__).parent / "fixtures"
                / "nextest-0.9.146-stress-10.txt").read_text()
        self.assertEqual(flakes.stress_verdict(0, real), ("GREEN", 10))
        one_red = real.replace("iteration 7/10: 1 test run: 1 passed",
                               "iteration 7/10: 1 test run: 0 passed, 1 failed")
        self.assertEqual(flakes.stress_verdict(0, one_red)[0], "RED")
        self.assertEqual(flakes.stress_verdict(0, real, count=5)[0], "RED")

    def test_isolation_filter_names_binary_and_test(self):
        argv = flakes.nextest_isolation_argv(
            ["cargo-nextest", "nextest", "run", "--profile", "default", "-E", "all()"],
            "example-cells::gh1_a_window_closes", "tests::the_window_reopens")
        self.assertEqual(argv[:3], ["cargo-nextest", "nextest", "run"])
        self.assertNotIn("default", argv)      # the station's own profile and filter go
        self.assertNotIn("all()", argv)
        self.assertEqual(argv[argv.index("--profile") + 1], "isolate")
        self.assertEqual(argv[argv.index("--retries") + 1], "0")
        self.assertEqual(argv[argv.index("--stress-count") + 1], "10")
        self.assertEqual(argv[argv.index("-E") + 1],
                         "binary_id(=example-cells::gh1_a_window_closes) "
                         "& test(=tests::the_window_reopens)")
        self.assertNotIn("--run-ignored", argv)
        ignored = flakes.nextest_isolation_argv(["cargo-nextest", "nextest", "run"],
                                                BIN, "a", ignored=True)
        self.assertEqual(ignored[ignored.index("--run-ignored") + 1], "all")
        profile = tomllib.loads(flakes.isolate_profile_toml())
        self.assertEqual(profile["profile"]["isolate"]["inherits"], "default")
        self.assertEqual(profile["profile"]["isolate"]["retries"], 0)

    def test_stress_verdict_is_fail_closed(self):
        self.assertEqual(flakes.stress_verdict(0, GREEN_SUMMARY, 10), ("GREEN", 10))
        # measured on nextest 0.9.146: early red iterations still end with rc 0
        self.assertEqual(flakes.stress_verdict(0, RED_SUMMARY, 10), ("RED", 9))
        self.assertEqual(flakes.stress_verdict(100, RED_SUMMARY, 10), ("RED", 9))
        self.assertEqual(flakes.stress_verdict(0, "no summary at all", 10), ("RED", 0))
        self.assertEqual(flakes.stress_verdict(
            0, "Summary [ 0.1s] 3/3 stress run iterations: 3 passed\n", 10), ("RED", 3))
        self.assertEqual(flakes.repeat_verdict([0] * 10), ("GREEN", 10))
        self.assertEqual(flakes.repeat_verdict([0] * 9 + [1]), ("RED", 9))
        self.assertEqual(flakes.repeat_verdict([0] * 9), ("RED", 9))

    def test_isolation_ten_green_marks_station_flaky_before_pass_ended(self):
        out, run = self.isolate([red_result(["a_test"])], (0, GREEN_SUMMARY),
                                issues={f"{BIN}::a_test": 4711})
        self.assertEqual((out["verdict"], out["isolated"]), ("FLAKY", 1))
        ev, = out["events"]
        self.assertEqual(ev, {"station": "tests", "test": f"{BIN}::a_test", "count": 2,
                              "green": 2, "verdict": "FLAKY", "issue": 4711,
                              "cause": flakes.CAUSE_NONE,
                              "triage": {"t1": "GREEN", "t2": "GREEN"}})
        # the executor's order: isolations, then THE station_ended, then pass_ended
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        db = store.open(pathlib.Path(tmp.name) / "p.db")
        self.addCleanup(store.close, db)
        r = "run-0000000000aa"
        for name, data in flakes.end_station(ENDED, out):
            events.emit(db, name, events.EVENTS[name].kinds[0], dict(data, run=r), run=r)
        stations = {"green": 0, "red": 0, "cached": 0, "flaky": 1, "skip": 0, "note": 0,
                    "ask": 0, "aborted": 0}
        events.emit(db, "pass_ended", "status",
                    {"run": r, "verdict": "GREEN", "secs": 13, "lane_seconds": 13,
                     "critical_path_s": 13, "rounds": 1, "cache_hits": 0,
                     "stations": stations}, run=r)
        order = [row["event"] for row in events.read_run(db, r)]
        self.assertEqual(order, ["flake_isolated", "station_ended", "pass_ended"])
        ended = events.read_run(db, r)[1]["data"]
        self.assertEqual((ended["verdict"], ended["isolated"]), ("FLAKY", 1))

    def test_isolation_one_red_keeps_red(self):
        out, _ = self.isolate([red_result(["a_test"])], (100, RED_SUMMARY))
        self.assertEqual(out["verdict"], "RED")
        self.assertEqual(out["events"][0]["verdict"], "RED")
        self.assertEqual(out["events"][0]["green"], 0)   # T1 red: one run, a defect
        # rc 0 with a failed iteration (nextest 0.9.146) is still red
        out, _ = self.isolate([red_result(["a_test"])], (0, RED_SUMMARY))
        self.assertEqual(out["verdict"], "RED")
        # two red tests, one flaky: the station stays red
        answers = {"a_test": (0, GREEN_SUMMARY), "b_test": (0, RED_SUMMARY)}
        out, _ = self.isolate([red_result(["a_test", "b_test"])],
                              lambda argv: answers.get(argv[-1].rsplit("=", 1)[1].rstrip(")"), (0, GREEN_SUMMARY)))
        self.assertEqual(out["verdict"], "RED")
        self.assertEqual([e["verdict"] for e in out["events"]], ["FLAKY", "RED"])

    def test_isolation_cap_five_per_run(self):
        six = [red_result(["t%d" % i for i in range(4)]),
               red_result(["u0", "u1"], part="example-cells")]
        cands, over = flakes.isolation_candidates(six)
        self.assertTrue(over)
        self.assertEqual(len(cands), 6)
        out, run = self.isolate(six, (0, GREEN_SUMMARY))
        self.assertEqual((out["verdict"], out["isolated"]), ("RED", 0))
        self.assertEqual(out["reason"], "too many red for isolation")
        self.assertEqual(run.calls, [])
        self.assertEqual(out["events"], [])
        five = [red_result(["t%d" % i for i in range(5)])]
        out, run = self.isolate(five, (0, GREEN_SUMMARY), issues={
            f"{BIN}::t{i}": 10 + i for i in range(5)})
        self.assertEqual((out["verdict"], out["isolated"], len(run.calls)), ("FLAKY", 5, 10))
        self.assertEqual(out["reason"], "flaky: GH #10, GH #11, GH #12, GH #13, GH #14")

    def test_isolation_emits_one_station_ended(self):
        out, _ = self.isolate([red_result(["a_test", "b_test"])], (0, GREEN_SUMMARY))
        evs = flakes.end_station(ENDED, out)
        names = [n for n, _ in evs]
        self.assertEqual(names, ["flake_isolated", "flake_isolated", "station_ended"])
        final = evs[-1][1]
        self.assertEqual((final["verdict"], final["isolated"]), ("FLAKY", 2))
        self.assertEqual((final["secs"], final["exit"], final["scope"]), (12.5, 100, "1 binary"))
        for name, data in evs:   # every line is a valid V0 event once the run is set
            events.validate(name, events.EVENTS[name].kinds[0], dict(data, run="run-x"))
        # too many red: no isolation, still exactly one station_ended
        many = [red_result(["t%d" % i for i in range(6)])]
        out, _ = self.isolate(many, (0, GREEN_SUMMARY))
        evs = flakes.end_station(ENDED, out)
        self.assertEqual([n for n, _ in evs], ["station_ended"])
        self.assertEqual(evs[0][1]["verdict"], "RED")
        self.assertEqual(evs[0][1]["reason"], "too many red for isolation")

    def test_a_red_after_isolation_keeps_where_and_log(self):
        """Review R3 M2: the end reason keeps the #1150 location and log path."""
        where = "at crate::bin a::b_test -- log logs/strand-tests.log"
        many = [red_result(["t%d" % i for i in range(6)])]
        out, _ = self.isolate(many, (0, GREEN_SUMMARY))
        for old, want in (("exit 100; " + where, "too many red for isolation; " + where),
                          (where, "too many red for isolation; " + where),
                          ("log logs/x.log", "too many red for isolation -- log logs/x.log")):
            final = flakes.end_station(dict(ENDED, reason=old), out)[-1][1]
            self.assertEqual((final["verdict"], final["reason"]), ("RED", want))

    def test_an_isolation_red_names_the_test_that_stayed_red(self):
        """#1164 (Z M-INT-F): the log's first FAIL was gh1001, green 10/10 in the
        isolation; red stayed seed_corpus -- the label named gh1001. The end
        reason names the test the isolation found red, never the first log hit."""
        answers = {"gh1001_cost": (0, GREEN_SUMMARY), "seed_corpus": (100, RED_SUMMARY)}
        out, _ = self.isolate([red_result(["gh1001_cost", "seed_corpus"])],
                              lambda argv: answers.get(argv[-1].rsplit("=", 1)[1].rstrip(")"), (0, GREEN_SUMMARY)))
        old = "at crate::gh1001 gh1001_cost -- log logs/tests.log"
        final = flakes.end_station(dict(ENDED, reason=old), out)[-1][1]
        self.assertEqual((final["verdict"], final["reason"]),
                         ("RED", f"triage T1 red: defect, 1 of 2; at {BIN}::seed_corpus -- log logs/tests.log"))
        # no log tail: the red test is still named
        final = flakes.end_station(ENDED, out)[-1][1]
        self.assertEqual(final["reason"], f"triage T1 red: defect, 1 of 2; at {BIN}::seed_corpus")

    def test_an_isolation_with_two_reds_names_both(self):
        """#1164 re-check: two of three stay red -- both names in the label, the
        green one not."""
        answers = {"a_ok": (0, GREEN_SUMMARY), "b_red": (100, RED_SUMMARY),
                   "c_red": (100, RED_SUMMARY)}
        out, _ = self.isolate([red_result(["a_ok", "b_red", "c_red"])],
                              lambda argv: answers.get(argv[-1].rsplit("=", 1)[1].rstrip(")"), (0, GREEN_SUMMARY)))
        old = "at crate::bin a_ok -- log logs/tests.log"
        final = flakes.end_station(dict(ENDED, reason=old), out)[-1][1]
        self.assertEqual(final["reason"], f"triage T1 red: defect, 2 of 3; at {BIN}::b_red, "
                                          f"{BIN}::c_red -- log logs/tests.log")

    def test_unexplained_red_never_ends_flaky(self):
        """C1: FLAKY only when the names explain the whole red and the exit is a test failure."""
        cmd_part = {"station": "tests", "part": "cmd:1", "verdict": "RED", "secs": 1.0}
        cut = dict(red_result(["a_test"]), tests={"total": 3, "failed": 2})
        for results in ([cmd_part, red_result(["a_test"])], [cut]):
            out, run = self.isolate(results, (0, GREEN_SUMMARY), issues={f"{BIN}::a_test": 7})
            self.assertEqual((out["verdict"], out["reason"]), ("RED", "unexplained red"))
            self.assertEqual(run.calls, [])
            final = flakes.end_station(ENDED, out)[-1][1]
            self.assertEqual((final["verdict"], final["reason"]), ("RED", "unexplained red"))
        # the names explain it, but the station exited with a non-test code (101: build)
        out, _ = self.isolate([red_result(["a_test"])], (0, GREEN_SUMMARY),
                              issues={f"{BIN}::a_test": 7})
        self.assertEqual(out["verdict"], "FLAKY")
        evs = flakes.end_station(dict(ENDED, exit=101), out)
        self.assertEqual(evs[-1][1]["verdict"], "RED")
        self.assertEqual(evs[-1][1]["reason"], "unexplained red: exit 101")
        self.assertEqual(flakes.end_station(ENDED, out)[-1][1]["verdict"], "FLAKY")
        # Python and scenario stations fail their tests with exit 1
        out, _ = self.isolate([red_result(["m.C.test_a"], part="py")], (0, UT_ONE_OK), kind="unittest",
                              issues={"m.C.test_a": 7})
        self.assertEqual(flakes.end_station(dict(ENDED, exit=1), out)[-1][1]["verdict"], "FLAKY")
        self.assertEqual(flakes.end_station(ENDED, out)[-1][1]["verdict"], "RED")

    def test_crash_and_timeout_are_never_isolated(self):
        """I1: the JUnit failure type travels; a signal abort, a timeout or an
        execution error is RED at once -- only an assertion failure is isolated."""
        junit = pathlib.Path(self.tmp.name) / "junit.xml"
        junit.write_text(
            '<testsuites><testsuite name="example-cells::gh1">'
            '<testcase name="t_assert"><failure type="test failure">panicked</failure></testcase>'
            '<testcase name="t_abort"><failure type="test abort">process aborted with signal 11'
            '</failure></testcase>'
            '<testcase name="t_timeout"><failure type="test timeout">timed out</failure>'
            '</testcase>'
            '<testcase name="t_exec"><error type="execution failure">no such file</error>'
            '</testcase>'
            '</testsuite></testsuites>')
        kinds = {t["name"]: t.get("failure") for t in flakes.junit_tests(junit)}
        self.assertEqual(kinds, {"t_assert": "assert", "t_abort": "abort",
                                 "t_timeout": "timeout", "t_exec": "error"})
        part = flakes.junit_parts(junit)[0]
        self.assertEqual(part["hard_failures"],
                         {"t_abort": "abort", "t_timeout": "timeout", "t_exec": "error"})
        out, run = self.isolate([part], (0, GREEN_SUMMARY))
        self.assertEqual((out["verdict"], out["reason"]),
                         ("RED", "not isolatable: abort, error, timeout"))
        self.assertEqual(run.calls, [])
        # an assertion failure alone is isolated as before, and carries no hard_failures
        junit.write_text('<testsuites><testsuite name="example-cells::gh1"><testcase name="t_a">'
                         '<failure type="test failure">panicked</failure></testcase>'
                         '</testsuite></testsuites>')
        part = flakes.junit_parts(junit)[0]
        self.assertNotIn("hard_failures", part)
        out, _ = self.isolate([part], (0, GREEN_SUMMARY))
        self.assertEqual(out["verdict"], "FLAKY")

    def test_flaky_without_issue_notes_pending_never_green_silently(self):
        out, _ = self.isolate([red_result(["a_test"])], (0, GREEN_SUMMARY))
        self.assertEqual(out["verdict"], "FLAKY")
        self.assertEqual(out["reason"], "issue pending")
        self.assertNotIn("issue", out["events"][0])
        final = flakes.end_station(ENDED, out)[-1][1]
        self.assertEqual((final["verdict"], final["reason"]), ("FLAKY", "issue pending"))
        # and the issue duty never turns a missing `gh` into red or an exception
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        db = store.open(pathlib.Path(tmp.name) / "p.db")
        self.addCleanup(store.close, db)
        r = "run-0000000000bb"
        events.emit(db, "test_result", "note", dict(red_result(["a_test"]), run=r), run=r)
        events.emit(db, "flake_isolated", "finding", dict(out["events"][0], run=r), run=r)
        flakes.refresh(db, at=AT)

        def no_gh(argv):
            raise FileNotFoundError("gh")
        res = flakes.issue_for(db, f"{BIN}::a_test", gh=no_gh,
                               drafts=pathlib.Path(tmp.name) / "d", at=AT)
        self.assertEqual(res["action"], "draft")
        self.assertTrue(pathlib.Path(res["draft"]).is_file())
        self.assertEqual(flakes.status(flakes.get(db, f"{BIN}::a_test"), AT), "issue pending")

    def test_a_docstring_with_dots_is_not_the_status(self):
        """tv-kit D: a docstring line under the test name that holds `...`
        itself (`@...`) is no status -- only the trailing ` ... ok|FAIL|ERROR|
        skipped` is. Measured: `TheFlakyCount.test_attributes_and_decorators_
        belong_to_the_test` counted red in 5 of 5 green runs. Nor is a
        test's own output between ` ... ` and its status on the next line
        (`Argv.test_a_case_without_its_runs_is_a_refusal`, 5 of 5)."""
        text = ("test_a (m.K.test_a)\n"
                "Lines with `@...` above the fn ... ok\n"
                "test_b (m.K.test_b)\n"
                "Waits for x... and y ... FAIL\n"
                "test_c (m.K.test_c) ... skipped 'no lane ... here'\n"
                "test_d (m.K.test_d) ... tool: ran 0 of 5 times -- check the id\n"
                "ok\n")
        parsed = flakes.parse_unittest_output(text)
        self.assertEqual(parsed["passed"], ["m.K.test_a", "m.K.test_d"])
        self.assertEqual(parsed["failed"], ["m.K.test_b"])
        self.assertEqual(parsed["skipped"], ["m.K.test_c"])

    def test_an_unexpected_success_is_red_after_an_ok_line(self):
        """Re-check fix2 M1: a test's own output that begins with `ok` read as
        its status (`ok, done` on the name line, `ok so far` on a line of its
        own) -- an unexpected success counted green. Only the exact status
        words are one, and Python 3.12's `UNEXPECTED SUCCESS:` block names the
        test red like a `FAIL:` block (probe output, Python 3.12.3)."""
        text = ("test_ok (m.U.test_ok) ... ok\n"
                "test_skip (m.U.test_skip) ... skipped 'r'\n"
                "test_unexpected (m.U.test_unexpected) ... ok, done\n"
                "unexpected success\n"
                "test_unexpected_own_line (m.U.test_unexpected_own_line) ... x\n"
                "ok so far\n"
                "unexpected success\n\n" + "=" * 70 + "\n"
                "UNEXPECTED SUCCESS: test_unexpected (m.U.test_unexpected)\n"
                "UNEXPECTED SUCCESS: test_unexpected_own_line (m.U.test_unexpected_own_line)\n")
        parsed = flakes.parse_unittest_output(text)
        self.assertEqual(parsed["passed"], ["m.U.test_ok"])
        self.assertEqual(sorted(parsed["failed"]), ["m.U.test_unexpected", "m.U.test_unexpected_own_line"])
        self.assertEqual(parsed["skipped"], ["m.U.test_skip"])
        # the block alone names the test red, too
        self.assertEqual(flakes.parse_unittest_output(
            "UNEXPECTED SUCCESS: test_x (m.U.test_x)\n")["failed"], ["m.U.test_x"])

    def test_a_skip_reason_with_a_quote_is_a_skip(self):
        """Re-check fix3 H: unittest prints the reason with `repr`, so a reason
        holding `'` comes in double quotes -- that line was no status at all."""
        text = ("test_x (m.C.test_x) ... skipped \"it's off\"\n"
                "test_y (m.C.test_y) ... skipped 'off'\n")
        self.assertEqual(flakes.parse_unittest_output(text)["skipped"], ["m.C.test_x", "m.C.test_y"])

    def test_python_station_isolation_uses_module_id(self):
        text = (FIX / "unittest-verbose.txt").read_text()
        parsed = flakes.parse_unittest_output(text)
        self.assertEqual(parsed["failed"], ["test_example.TheThing.test_a_flaky_case",
                                            "test_other.OldStyle.test_an_old_style_case"])
        self.assertEqual(parsed["passed"], ["test_example.TheThing.test_a_green_case"])
        self.assertEqual(parsed["skipped"], ["test_example.TheThing.test_skipped_case"])
        results = flakes.unittest_test_results("gate-selftest", text)
        parts = {r["part"]: r for r in results}
        self.assertEqual(parts["test_other"]["failed_names"],
                         ["test_other.OldStyle.test_an_old_style_case"])
        # the kit's one Python (re-check tv-bn M7m M2): a rerun that reaches
        # the narrow point in its own process runs isolated
        self.assertEqual(flakes.unittest_isolation_argv("test_x.K.test_y"),
                         ["/usr/bin/python3", "-I", hang_dump.SELF, "-m", "unittest", "test_x.K.test_y"])
        self.assertEqual(flakes.unittest_isolation_argv(
            "test_x.K.test_y", ["python3", "-m", "unittest", "test_x"]),
            ["python3", "-m", "unittest", "test_x", "-k", "test_x.K.test_y"])
        run = FakeRun((0, UT_ONE_OK))
        out = flakes.triage_station("gate-selftest", "unittest", results, run, {})
        self.assertEqual(out["verdict"], "FLAKY")
        self.assertEqual(len(run.calls), 6)    # T1, T2, T3 each (OR-TV-2), two under the cap
        self.assertEqual(run.calls[0][-1], "test_example.TheThing.test_a_flaky_case")
        self.assertEqual({e["test"] for e in out["events"]},
                         {"test_example.TheThing.test_a_flaky_case",
                          "test_other.OldStyle.test_an_old_style_case"})
        # six red modules cap a Python station like a nextest one (Delta I1)
        cands, over = flakes.isolation_candidates(
            [red_result([f"m{i}.K.test_a"], part=f"m{i}") for i in range(6)], kind="unittest")
        self.assertEqual((len(cands), over), (6, True))

    def test_scenario_isolation_uses_case_id(self):
        cases = flakes.parse_scenario_last_run(FIX / "last_run.json")
        self.assertEqual(cases, {"C1": "GREEN", "C2": "RED", "D4": "RED"})
        results = flakes.scenario_test_results("scenarios:memory", cases)
        self.assertEqual([r["part"] for r in results], ["scenarios:memory:case:C1",
                                                        "scenarios:memory:case:C2",
                                                        "scenarios:memory:case:D4"])
        self.assertEqual(flakes.scenario_isolation_argv("C2", ["python3", "run.py"]),
                         ["python3", "run.py", "--only", "C2"])
        reds = iter([0, 0, 1])   # C2: T1 and T2 green; D4: T1 red (OR-TV-2)
        run = FakeRun(lambda argv: (next(reds), ""))
        out = flakes.triage_station("scenarios:memory", "scenario", results, run,
                                     {"scenarios:memory:case:C2": 7},
                                     base_argv=["python3", "run.py"],
                                     proof=ran_yes)
        self.assertEqual([c[-1] for c in run.calls], ["C2", "run.py", "D4"])
        self.assertEqual([(e["test"], e["verdict"], e["green"]) for e in out["events"]],
                         [("scenarios:memory:case:C2", "FLAKY", 2),
                          ("scenarios:memory:case:D4", "RED", 0)])
        self.assertEqual(out["verdict"], "RED")

    def test_gh721_signature_is_isolated_like_any_flake(self):
        cases = flakes.parse_scenario_last_run(FIX / "builder_last_run.json")
        self.assertEqual(cases, {"B1": "GREEN", "B4": "RED", "B5": "GREEN"})
        errors = flakes.scenario_errors(FIX / "builder_last_run.json")
        self.assertEqual(errors, {"B4": "registry lacks 'a-template' (have [])"})
        results = flakes.scenario_test_results(flakes.GH721_STATION, cases, errors)
        run = FakeRun((0, ""))
        out = flakes.triage_station(flakes.GH721_STATION, "scenario", results, run, {},
                                     base_argv=["python3", "run_builder.py"], proof=ran_yes)
        self.assertEqual([c[-2:] for c in run.calls],
                         [["--only", "B4"], ["python3", "run_builder.py"]])   # T1, T2
        ev, = out["events"]
        self.assertEqual((ev["test"], ev["verdict"], ev["issue"]),
                         (flakes.case_key(flakes.GH721_STATION, "B4"), "FLAKY", 721))
        self.assertEqual((out["verdict"], out["reason"]), ("FLAKY", "flaky: GH #721"))
        # without the signature the case is a flake like any other: no issue known
        results = flakes.scenario_test_results(flakes.GH721_STATION, cases,
                                               {"B4": "a different failure"})
        out = flakes.triage_station(flakes.GH721_STATION, "scenario", results,
                                     FakeRun((0, "")), {}, base_argv=["python3", "x.py"],
                                     proof=ran_yes)
        self.assertEqual(out["reason"], "issue pending")

    def test_gh721_signature_binds_to_its_own_case(self):
        """M2: the signature in ONE case's error never hands #721 to another red case."""
        cases = {"B4": "RED", "B6": "RED"}
        errors = {"B4": "registry lacks 'a-template' (have [])", "B6": "assertion failed"}
        results = flakes.scenario_test_results(flakes.GH721_STATION, cases, errors)
        out = flakes.triage_station(flakes.GH721_STATION, "scenario", results,
                                     FakeRun((0, "")), {}, base_argv=["python3", "x.py"],
                                     proof=ran_yes)
        issues = {e["test"]: e.get("issue") for e in out["events"]}
        self.assertEqual(issues, {flakes.case_key(flakes.GH721_STATION, "B4"): 721,
                                  flakes.case_key(flakes.GH721_STATION, "B6"): None})
        self.assertEqual((out["verdict"], out["reason"]), ("FLAKY", "issue pending"))


# --- the core: tasks, quarantine ------------------------------------------------------

class TheCore(StoreCase):
    def station_items(self):
        wp = store.item_create(self.db, "workpackage", "strand pass", at=AT)
        st = store.item_create(self.db, "task", "tests", parent=wp["id"], at=AT)
        return wp, st

    def test_isolation_task_has_failure_guard(self):
        wp, st = self.station_items()
        r = "run-0000000000c1"
        events.emit(self.db, "run_dispatched", "note",
                    {"run": r, "runner": "lane-a", "tree": "t", "attempt": 1, "round": 1,
                     "deadline_at": AT, "ws": "ws"}, run=r, at=AT)
        self.result_row(r, BIN, failed=["a_test", "b_test"])
        self.isolated(r, f"{BIN}::a_test", "FLAKY", issue=12)
        self.isolated(r, f"{BIN}::b_test", "RED")
        out = flakes.ingest_run(self.db, r, {"tests": st["id"]}, at=AT)
        self.assertEqual(len(out["tasks"]), 2)
        for key, verdict, state, run_state in (
                (out["tasks"][0], "FLAKY", "waiting", "failed"),   # a finding (OR-TV-2)
                (out["tasks"][1], "RED", "waiting", "failed")):
            item = dict(self.db.execute("SELECT * FROM items WHERE key=?", (key,)).fetchone())
            self.assertTrue(re.fullmatch(r"T-\d+", key))
            self.assertEqual((item["type"], item["parent"], item["state"]),
                             ("task", wp["id"], state))
            edge = self.db.execute("SELECT * FROM edges WHERE src=?", (item["id"],)).fetchone()
            self.assertEqual((edge["dst"], edge["kind"]), (st["id"], "depends_on"))
            self.assertEqual(json.loads(edge["data"]),
                             {"dep": "fs", "lag_min": 0, "when": {"outcome": "failure"}})
            tr = self.db.execute("SELECT * FROM task_runs WHERE item=?",
                                 (item["id"],)).fetchone()
            self.assertEqual((tr["verdict"], tr["state"], tr["runner"]),
                             (verdict, run_state, "lane-a"))
        # a second read of the same run adds nothing
        again = flakes.ingest_run(self.db, r, {"tests": st["id"]}, at=AT)
        self.assertEqual(again["tasks"], [])
        n = self.db.execute("SELECT count(*) FROM items WHERE type='task'").fetchone()[0]
        self.assertEqual(n, 3)
        self.assertEqual(self.row(f"{BIN}::a_test")["issue"], "12")

    def test_quarantine_auto_at_rate_5_percent(self):
        _, st = self.station_items()
        for n in range(19):
            self.observe(n, "a_test", "pass")
            self.observe(100 + n, "b_test", "pass")
        r = self.observe(19, "a_test", "flaky", issue=31)
        out = flakes.ingest_run(self.db, r, {"tests": st["id"]}, at=at_n(20))
        # OR-TV-2 (VG TV-F3): the pipeline proposes, it never arms
        self.assertEqual(out["proposed"], [f"{BIN}::a_test"])
        row = self.row(f"{BIN}::a_test")
        self.assertAlmostEqual(flakes.rate(row), 0.05)
        self.assertIsNone(row["quarantined_until"])
        self.assertEqual(flakes.dispatch_quarantine(self.db, at_n(21)), [])
        flakes.quarantine(self.db, f"{BIN}::a_test", 31, owner="o", cause="time", at=at_n(20))
        self.assertEqual(self.row(f"{BIN}::a_test")["quarantined_until"],
                         "2026-10-15T20:00:00Z")   # + 14 days
        self.assertEqual(flakes.dispatch_quarantine(self.db, at_n(21)),
                         [{"test": f"{BIN}::a_test", "issue": 31,
                           "until": "2026-10-15T20:00:00Z"}])
        # the same rate without an issue: no quarantine, the status says why
        r = self.observe(119, "b_test", "flaky")
        out = flakes.ingest_run(self.db, r, {"tests": st["id"]}, at=at_n(120))
        self.assertEqual(out["proposed"], [])
        self.assertEqual(out["pending"], [f"{BIN}::b_test"])
        self.assertIsNone(self.row(f"{BIN}::b_test")["quarantined_until"])
        self.assertEqual(flakes.status(self.row(f"{BIN}::b_test"), at_n(120)), "issue pending")

    def test_quarantine_never_for_a_test_with_defect_reds(self):
        """C2: a defect red (isolation RED) in the window rules the auto quarantine out."""
        _, st = self.station_items()
        for n in range(16):
            self.observe(n, "a_test", "pass")
        for n in range(16, 19):
            self.observe(n, "a_test", "red", issue=31)
        r = self.observe(19, "a_test", "flaky", issue=31)
        out = flakes.ingest_run(self.db, r, {"tests": st["id"]}, at=at_n(20))
        row = self.row(f"{BIN}::a_test")
        self.assertEqual((row["reds"], row["flaky"]), (3, 1))
        self.assertGreaterEqual(flakes.rate(row), 0.05)
        self.assertEqual(out["proposed"], [])
        self.assertIsNone(row["quarantined_until"])
        self.assertEqual(flakes.auto_quarantine(self.db, [f"{BIN}::a_test"], at_n(20)), [])

    def test_pass_share_counts_only_passes_a_flake_turned_green(self):
        """M1: a FLAKY isolation inside a pass that still ended RED is a red pass."""
        def ended(run, verdict):
            events.emit(self.db, "pass_ended", "status",
                        {"run": run, "verdict": verdict, "secs": 1.0, "lane_seconds": 1.0,
                         "critical_path_s": 1.0, "rounds": 1, "cache_hits": 0,
                         "stations": 1}, run=run, at=AT)
        self.isolated("run-0000000000f1", f"{BIN}::a_test", "FLAKY")
        ended("run-0000000000f1", "GREEN")
        self.isolated("run-0000000000f2", f"{BIN}::a_test", "FLAKY")
        ended("run-0000000000f2", "RED")
        ended("run-0000000000f3", "RED")
        self.assertEqual(flakes.pass_share(self.db),
                         {"share": 1 / 3, "passes_red": 2, "passes_flaky": 1})

    def test_quarantine_below_the_rate_stays_out(self):
        """A flake that left the window of 20 runs no longer buys a quarantine."""
        self.observe(0, "a_test", "flaky", issue=5)
        for n in range(1, 21):
            self.observe(n, "a_test", "pass")
        flakes.refresh(self.db, at=at_n(22))
        row = self.row(f"{BIN}::a_test")
        self.assertEqual((row["runs"], row["flaky"], row["issue"]), (20, 0, "5"))
        self.assertEqual(flakes.auto_quarantine(self.db, [f"{BIN}::a_test"], at_n(22)), [])

    def test_quarantine_release_after_14_days(self):
        flakes.quarantine(self.db, f"{BIN}::a_test", 77, owner="o", cause="time",
                          at="2026-10-01T00:00:00Z")
        until = "2026-10-15T00:00:00Z"
        self.assertEqual(self.row(f"{BIN}::a_test")["quarantined_until"], until)
        self.assertEqual(flakes.release_expired(self.db, "2026-10-14T23:59:59Z"), [])
        self.assertEqual(len(flakes.dispatch_quarantine(self.db, "2026-10-14T23:59:59Z")), 1)
        self.assertEqual(flakes.release_expired(self.db, "2026-10-15T00:00:01Z"),
                         [f"{BIN}::a_test"])
        self.assertEqual(flakes.dispatch_quarantine(self.db, "2026-10-15T00:00:01Z"), [])
        # the date stays, as a past; the hint is a finding in the journal -- once
        self.assertEqual(self.row(f"{BIN}::a_test")["quarantined_until"], until)
        self.assertEqual(flakes.release_expired(self.db, "2026-10-16T00:00:00Z"), [])
        rows = [dict(r) for r in self.db.execute(
            "SELECT * FROM journal WHERE event='process_finding'")]
        self.assertEqual(len(rows), 1)
        data = json.loads(rows[0]["data"])
        self.assertEqual((data["rule"], data["subject"], data["since"]),
                         ("flake_quarantine_expired", f"{BIN}::a_test", until))
        with self.assertRaises(ValueError):
            flakes.quarantine(self.db, f"{BIN}::b_test", None, at=AT)

    def test_quarantine_override_has_issue_and_expiry(self):
        entries = [{"test": f"{BIN}::a_test", "issue": 12, "until": "2026-10-22T20:00:00Z"},
                   {"test": "example-cells::tests::a_unit", "issue": 13,
                    "until": "2026-10-23T00:00:00Z"},
                   {"test": "gone-crate::old_binary::a_test", "issue": 14,
                    "until": "2026-10-23T00:00:00Z"},
                   {"test": "test_x.K.test_y", "issue": 15, "until": "2026-10-23T00:00:00Z"}]
        text, dropped = flakes.quarantine_toml(entries, [BIN, "example-cells"])
        self.assertEqual(dropped, ["gone-crate::old_binary::a_test", "test_x.K.test_y"])
        self.assertIn("# GH #12 -- runtime quarantine until 2026-10-22T20:00:00Z\n"
                      f"filter = 'binary_id(={BIN}) & test(=a_test)'\nretries = 2\n", text)
        self.assertIn("filter = 'binary_id(=example-cells) & test(=tests::a_unit)'", text)
        overrides = tomllib.loads(text)["profile"]["default"]["overrides"]
        self.assertEqual([o["retries"] for o in overrides], [2, 2])
        self.assert_filter_grammar(text)
        two, _ = flakes.quarantine_toml(entries[:1], [BIN], profiles=("default", "ci"))
        self.assertEqual(len(tomllib.loads(two)["profile"]["ci"]["overrides"]), 1)

    def assert_filter_grammar(self, text):
        """The rules of `TheFilterGrammar`, imported, run against the generated text.

        The class reads the repo file in its setUp; a subclass made HERE (not at
        module level, or discovery would run the whole class a second time) reads
        the generated text instead. `::` in a test name is legal for nextest (a
        module path); the house rule against it guards `<crate>::<binary>` in
        `test()`, so the fixture above keeps its nextest names to one segment
        where the rule applies and the grammar check runs on that entry alone."""
        mod = importlib.import_module("scripts.tests.test_nextest_quarantine")
        one = "\n".join(b for b in text.split("\n\n") if "tests::" not in b)

        class Generated(mod.TheFilterGrammar):
            def setUp(inner):
                inner.text = one
                inner.config = tomllib.loads(one)
        rules = ("test_no_test_term_carries_a_binary", "test_every_retry_entry_names_its_binary",
                 "test_every_retry_entry_names_its_issue")
        result = unittest.TestResult()
        for r in rules:  # TestCase.run, not a TestSuite: a suite would run THIS module's
            Generated(r).run(result)  # tearDownModule at its end and drop the patches above
        self.assertTrue(result.wasSuccessful(), result.failures + result.errors)
        self.assertEqual(mod.retry_entries_missing_an_issue(text), [])

    def test_quarantine_is_runtime_tool_config_not_repo_edit(self):
        repo_cfg = REPO / ".config" / "nextest.toml"
        before = hashlib.sha256(repo_cfg.read_bytes()).hexdigest()
        ws = self.root / "ws" / "run-0000000000e1"
        entries = [{"test": f"{BIN}::a_test", "issue": 12, "until": "2026-10-22T20:00:00Z"}]
        args, dropped = flakes.write_quarantine(ws, entries, [BIN])
        path = ws / "quarantine.toml"
        self.assertEqual(args, ["--tool-config-file", f"pipeline:{path.resolve()}"])
        self.assertEqual(dropped, [])
        self.assertTrue(path.is_file())
        self.assertIn("[profile.isolate]", path.read_text())
        self.assertEqual(hashlib.sha256(repo_cfg.read_bytes()).hexdigest(), before)
        # no entry: the file still carries the isolation profile, and no override
        args, _ = flakes.write_quarantine(self.root / "ws2", [], [BIN])
        empty = (self.root / "ws2" / "quarantine.toml").read_text()
        self.assertEqual(args[0], "--tool-config-file")
        self.assertNotIn("overrides", empty)
        self.assertEqual(tomllib.loads(empty)["profile"]["isolate"]["inherits"], "default")

    def test_isolation_reuses_the_one_pipeline_tool_config(self):
        # OR-DT.W8.3: nextest refuses two `--tool-config-file` with the same tool
        # name, so the isolate profile lives in the SAME file as the quarantine
        # (one tool name `pipeline`); the isolation argv keeps the station's flag
        # exactly once and adds no second tool config.
        ws = self.root / "ws" / "run-0000000000e3"
        tool_args, _ = flakes.write_quarantine(ws, [], [BIN])
        station = ["cargo-nextest", "nextest", "run", *tool_args, "--profile", "default"]
        argv = flakes.nextest_isolation_argv(station, BIN, "a_test")
        self.assertEqual(argv.count("--tool-config-file"), 1)
        self.assertEqual(argv[argv.index("--tool-config-file") + 1], tool_args[1])
        self.assertTrue(tool_args[1].startswith("pipeline:"))
        cfg = tomllib.loads((ws / "quarantine.toml").read_text())
        self.assertIn("isolate", cfg["profile"])

    def test_split_test_takes_the_longest_known_binary(self):
        known = ["example-cells", BIN]
        self.assertEqual(flakes.split_test(f"{BIN}::tests::a", known), (BIN, "tests::a"))
        self.assertEqual(flakes.split_test("example-cells::tests::a", known),
                         ("example-cells", "tests::a"))
        self.assertIsNone(flakes.split_test("other::a", known))
        self.assertIsNone(flakes.split_test("case:C2", known))


# --- the issue duty ---------------------------------------------------------------------

class TheIssueDuty(StoreCase):
    def flaky(self, name, issue=None):
        self.n = getattr(self, "n", 200) + 1
        r = self.observe(self.n, name, "flaky", issue=issue)
        events.emit(self.db, "lane_fingerprint", "note",
                    {k: None for k in events._FINGERPRINT} | {"nextest": "0.9.146"},
                    run=r, at=AT)
        flakes.refresh(self.db, at=AT)
        return f"{BIN}::{name}"

    def test_flake_issue_template_has_no_free_text(self):
        title, body = flakes.render_issue("a-crate::a_bin::a_test", runs=20, reds=1, flaky=2,
                                          green=10, count=10, lane="fp-0a1b2c3d")
        self.assertEqual(title, "flaky: a-crate::a_bin::a_test")
        # the template, every hole a closed character class -- no room for prose
        holes = {"test": r"[A-Za-z0-9_.:/-]+", "runs": r"\d+", "reds": r"\d+",
                 "flaky": r"\d+", "rate": r"\d\.\d\d", "green": r"\d+", "count": r"\d+",
                 "lane": r"fp-(?:[0-9a-f]{8}|none)"}
        pattern = re.escape(flakes.ISSUE_BODY)
        for k, v in holes.items():
            pattern = pattern.replace(re.escape("{%s}" % k), "(?:%s)" % v)
        self.assertTrue(re.fullmatch(pattern, body), body)
        self.assertIn("- rate: 0.10\n", body)
        self.assertEqual(flakes.ISSUE_LABEL, "flaky")
        # the renderer takes numbers and the whitelisted name only
        import inspect
        self.assertEqual(list(inspect.signature(flakes.render_issue).parameters),
                         ["test", "runs", "reds", "flaky", "green", "count", "lane"])
        with self.assertRaises(ValueError):
            flakes.render_issue("a test with spaces", 1, 0, 1, 10, 10, "fp-none")
        with self.assertRaises(ValueError):
            flakes.render_issue("a_test", "20; see my notes", 0, 1, 10, 10, "fp-none")
        # through the full path: a name outside the whitelist is a draft, never an issue
        gh = FakeGh()
        self.db.execute("INSERT INTO flakes (test, runs, reds, flaky) VALUES (?, 20, 0, 1)",
                        ("bad name`x`",))
        res = flakes.issue_for(self.db, "bad name`x`", gh=gh, drafts=self.drafts, at=AT)
        self.assertEqual((res["action"], res["reason"]), ("draft", "name outside the whitelist"))
        self.assertEqual(gh.calls, [])

    def test_flake_issue_r5_scan_writes_draft_on_private_pattern(self):
        for text in ("case:192.0.2.10", "see /home/someone/x", "a.lan host", "~/x"):
            self.assertTrue(flakes.r5_hits(text), text)
        for text in ("example-cells::gh1::a_test", "listen on 127.0.0.1", "rate 0.05"):
            self.assertEqual(flakes.r5_hits(text), [], text)
        gh = FakeGh()
        self.db.execute("INSERT INTO flakes (test, runs, reds, flaky) VALUES (?, 20, 0, 1)",
                        ("case:192.0.2.10",))
        res = flakes.issue_for(self.db, "case:192.0.2.10", gh=gh, drafts=self.drafts, at=AT)
        self.assertEqual((res["action"], res["reason"]), ("draft", "r5 scan hit"))
        self.assertEqual(gh.calls, [])
        draft = pathlib.Path(res["draft"])
        self.assertEqual(draft.parent, self.drafts)
        self.assertEqual(draft.name, "issue-case_192.0.2.10.md")
        self.assertIn("flaky: case:192.0.2.10", draft.read_text())

    def test_r5_name_patterns_are_read_from_the_export_audit(self):
        """No copy of the audit's name patterns: they are read at run time (ast, no import)."""
        self.assertFalse(hasattr(flakes, "R5_NAME_PATTERNS"))
        self.assertEqual(flakes.r5_name_patterns(), (r"forbidden-sample|other-sample",))
        self.assertTrue(flakes.r5_hits("see Other-Sample here"))
        self.assertEqual(flakes.r5_hits("example-cells::gh1::a_test"), [])

    def test_flake_issue_without_the_export_audit_writes_draft(self):
        """Fail-closed: no audit (the public tree), no readable NAME_PATTERNS -> a draft."""
        test = self.flaky("a_test")
        for text in (None, "NAME_PATTERNS = make()\n", "x = [\n"):
            audit = self.root / "audit.py"
            if text is None:
                audit = self.root / "missing.py"
            else:
                audit.write_text(text)
            with mock.patch.object(flakes, "R5_AUDIT", audit):
                self.assertIsNone(flakes.r5_name_patterns())
                gh = FakeGh()
                res = flakes.issue_for(self.db, test, gh=gh, drafts=self.drafts, at=AT)
            self.assertEqual((res["action"], res["reason"]),
                             ("draft", "r5 patterns unavailable"), text)
            self.assertEqual(gh.calls, [])

    def test_the_real_export_audit_yields_name_patterns(self):
        """In the private tree the real audit parses; in the public tree it is absent."""
        audit = REPO / "plans" / "export-fixtures" / "make_export.py"
        if not audit.is_file():
            self.skipTest("the export audit is not in this tree")
        with mock.patch.object(flakes, "R5_AUDIT", audit):
            patterns = flakes.r5_name_patterns()
        self.assertTrue(patterns)
        self.assertTrue(all(isinstance(p, str) and p for p in patterns))

    def test_flake_issue_dedup_comments_open_issue(self):
        test = self.flaky("a_test", issue=12)
        gh = FakeGh([{"number": 12, "title": f"flaky: {test}", "state": "OPEN"}])
        res = flakes.issue_for(self.db, test, gh=gh, drafts=self.drafts, at=AT)
        self.assertEqual((res["action"], res["issue"]), ("comment", 12))
        self.assertEqual(gh.verbs(), ["view", "comment"])
        body = gh.calls[-1][gh.calls[-1].index("--body") + 1]
        self.assertRegex(body, r"- lane: fp-[0-9a-f]{8}\n")
        # unknown to the register, found by its exact title: comment, and remember it
        test = self.flaky("b_test")
        gh = FakeGh([{"number": 40, "title": f"flaky: {test} (old)", "state": "OPEN"},
                     {"number": 41, "title": f"flaky: {test}", "state": "OPEN"}])
        res = flakes.issue_for(self.db, test, gh=gh, drafts=self.drafts, at=AT)
        self.assertEqual((res["action"], res["issue"]), ("comment", 41))
        self.assertEqual(self.row(test)["issue"], "41")
        # closed: a new one that names the old
        test = self.flaky("c_test", issue=50)
        gh = FakeGh([{"number": 50, "title": f"flaky: {test}", "state": "CLOSED"}])
        res = flakes.issue_for(self.db, test, gh=gh, drafts=self.drafts, at=AT)
        self.assertEqual((res["action"], res["issue"]), ("create", 501))
        create = gh.calls[-1]
        self.assertTrue(create[create.index("--body") + 1].endswith("Previous: #50\n"))
        self.assertEqual(create[create.index("--label") + 1], "flaky")
        self.assertEqual(self.row(test)["issue"], "501")

    def test_flake_issue_daily_cap_writes_draft(self):
        a, b = self.flaky("a_test"), self.flaky("b_test")
        gh = FakeGh()
        res = flakes.issue_for(self.db, a, gh=gh, drafts=self.drafts, at=AT, max_new=1)
        self.assertEqual(res["action"], "create")
        res = flakes.issue_for(self.db, b, gh=gh, drafts=self.drafts, at=AT, max_new=1)
        self.assertEqual((res["action"], res["reason"]), ("draft", "daily cap"))
        self.assertTrue(pathlib.Path(res["draft"]).is_file())
        self.assertEqual(gh.verbs(), ["list", "create", "list"])
        value = self.db.execute("SELECT value FROM counters WHERE name=?",
                                ("flake_issues:2026-10-08",)).fetchone()[0]
        self.assertEqual(value, 1)
        # the next day has its own cap
        res = flakes.issue_for(self.db, b, gh=gh, drafts=self.drafts,
                               at="2026-10-09T08:00:00Z", max_new=1)
        self.assertEqual(res["action"], "create")

    def test_flake_issue_dry_run_and_gh_failure_never_raise(self):
        test = self.flaky("a_test")
        gh = FakeGh()
        res = flakes.issue_for(self.db, test, gh=gh, drafts=self.drafts, at=AT, dry_run=True)
        self.assertEqual(res["action"], "dry-run")
        self.assertEqual(gh.calls, [])
        res = flakes.issue_for(self.db, test, gh=FakeGh(fail=True), drafts=self.drafts, at=AT)
        self.assertEqual((res["action"], res["reason"]), ("draft", "gh unavailable"))

    # Re-check tb-s3-store M-1 (#1161): `issue_for` swallowed a locked store
    # after `gh issue create` -- draft "gh unavailable", the number never in
    # the register, the pass GREEN (probe P3: 2 creates, issue None).
    def test_a_locked_store_after_the_create_raises_never_a_silent_draft(self):
        test = self.flaky("a_test")
        gh = FakeGh()
        locked = sqlite3.OperationalError("database is locked")
        with mock.patch.object(flakes, "_remember", side_effect=locked) as remember, \
                mock.patch.object(store, "WRITE_BACKOFF_S", 0):
            # The gate's outer retry around the issue duty must not file twice.
            with self.assertRaises(store.StoreWriteError):
                store.retry_write(flakes.issue_for, self.db, test, gh=gh, drafts=self.drafts,
                                  at=AT)
        self.assertEqual(gh.verbs().count("create"), 1, gh.calls)
        self.assertEqual(remember.call_count, store.WRITE_ATTEMPTS)
        self.assertFalse(self.drafts.exists() and list(self.drafts.iterdir()))

    def test_a_locked_daily_counter_raises_before_any_create(self):
        test = self.flaky("a_test")
        gh = FakeGh()
        locked = sqlite3.OperationalError("database is locked")
        with mock.patch.object(flakes, "_take_daily", side_effect=locked) as take, \
                mock.patch.object(store, "WRITE_BACKOFF_S", 0):
            with self.assertRaises(store.StoreWriteError):
                flakes.issue_for(self.db, test, gh=gh, drafts=self.drafts, at=AT)
        self.assertNotIn("create", gh.verbs())
        self.assertEqual(take.call_count, store.WRITE_ATTEMPTS)

    def test_a_briefly_locked_store_still_remembers_the_issue(self):
        test = self.flaky("a_test")
        gh = FakeGh()
        real, calls = flakes._remember, []

        def remember(*a):
            calls.append(a)
            if len(calls) == 1:
                raise sqlite3.OperationalError("database is locked")
            return real(*a)

        with mock.patch.object(flakes, "_remember", side_effect=remember), \
                mock.patch.object(store, "WRITE_BACKOFF_S", 0):
            res = flakes.issue_for(self.db, test, gh=gh, drafts=self.drafts, at=AT)
        self.assertEqual(res["action"], "create", res)
        self.assertEqual(self.row(test)["issue"], str(res["issue"]))
        self.assertEqual(gh.verbs().count("create"), 1, gh.calls)


# --- CLI, metric, timer list ----------------------------------------------------------

class TheSurface(StoreCase):
    def cli(self, *argv):
        out = io.StringIO()
        env = {"MECLAW_PIPELINE_DB": str(self.root / "pipeline.db")}
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()), \
                mock.patch.dict(os.environ, env):
            rc = cli.main(["flakes", *argv])
        return rc, out.getvalue()

    def test_cli_list_show_rate_quarantine_release(self):
        r = "run-0000000000f1"
        self.pass_planned(r, AT)
        self.result_row(r, BIN, failed=["a_test"])
        self.isolated(r, f"{BIN}::a_test", "FLAKY")
        events.emit(self.db, "pass_ended", "status",
                    {"run": r, "verdict": "GREEN", "secs": 1, "lane_seconds": 1,
                     "critical_path_s": 1, "rounds": 1, "cache_hits": 0,
                     "stations": {"flaky": 1}}, run=r, at=AT)
        flakes.refresh(self.db, at=AT)
        rc, out = self.cli("list")
        self.assertEqual(rc, 0)
        self.assertIn(f"{BIN}::a_test", out)
        self.assertIn("issue pending", out)
        rc, out = self.cli("show", f"{BIN}::a_test")
        self.assertEqual((rc, json.loads(out)["flaky"], json.loads(out)["repo"]),
                         (0, 1, "meclaw"))
        rc, out = self.cli("rate")
        self.assertEqual(rc, 0)
        self.assertIn("FLAKES share=1.00 passes_red=0 passes_flaky=1", out)
        rc, out = self.cli("quarantine", f"{BIN}::a_test", "--issue", "12", "--days", "3",
                           "--owner", "test-high-1", "--cause", "time")
        self.assertEqual(rc, 0)
        self.assertEqual(self.row(f"{BIN}::a_test")["issue"], "12")
        self.assertIn("quarantined", self.cli("list")[1])
        rc, _ = self.cli("release", f"{BIN}::a_test")
        self.assertEqual(rc, 0)
        self.assertEqual(flakes.dispatch_quarantine(self.db, store.now()), [])
        rc, _ = self.cli("show", "no::such::test")
        self.assertEqual(rc, 1)
        with mock.patch.object(flakes, "GH_RUNNER", FakeGh()):
            rc, out = self.cli("issue", f"{BIN}::a_test", "--dry-run",
                               "--drafts", str(self.drafts))
        self.assertEqual(rc, 0)
        self.assertIn("dry-run", out)

    def test_pipe_flake_metric_line(self):
        r = "run-0000000000f2"
        self.pass_planned(r, AT)
        self.result_row(r, BIN, failed=["a_test"])
        flakes.refresh(self.db, at=AT)
        with mock.patch.dict(os.environ, {metrics.URL_ENV: ""}):
            self.isolated(r, f"{BIN}::a_test", "FLAKY", at=AT)
        flakes.refresh(self.db, at=AT)
        line, = [x for x in metrics.lines(self.db) if x.startswith("pipe_flake")]
        self.assertEqual(line, f"pipe_flake,repo=meclaw,station=tests,test={BIN}::a_test "
                               "outcome=1,rate=1.0 1791460800000000000")

    def test_timer_list_matches_m0_junit(self):
        """`timer_tests.json` names every test of the JUnit that ran > 30 s, once.

        Form: `{"threshold_s": 30, "tests": [{"test": "<binary_id>::<name>",
        "secs", "action", "reason"?}]}`; `action` from the closed set
        `flakes.TIMER_ACTIONS`; a `keep_real_time` line says why."""
        doc = json.loads((FIX / "timer_tests.json").read_text())
        threshold = doc.get("threshold_s", 30)
        junit = {f"{t['binary_id']}::{t['name']}": t
                 for t in flakes.junit_tests(FIX / "slow-junit.xml")}
        slow = {k for k, t in junit.items() if t["secs"] > threshold}
        listed = [t["test"] for t in doc["tests"]]
        self.assertEqual(sorted(listed), sorted(set(listed)), "a test is listed twice")
        self.assertEqual(slow - set(listed), set(), "a slow test is missing from the list")
        self.assertEqual(set(listed) - set(junit), set(), "a list line names no test case")
        for t in doc["tests"]:
            with self.subTest(test=t["test"]):
                self.assertIn(t["action"], flakes.TIMER_ACTIONS)
                if t["action"] == "keep_real_time":
                    self.assertTrue(t.get("reason", "").strip())



# --- the hook-in after the W3 merge (re-check N1-N4, executor and kern) ---------------

class TheReCheckFixes(StoreCase):
    def test_a_red_isolation_sticks_over_a_later_flaky_round(self):
        """N1: round 1 isolates RED, round 2 of the same run FLAKY -> the test
        counts as red, never as a flake (no auto quarantine off a defect)."""
        run = "run-000000000001"
        self.result_row(run, BIN, failed=["wobbles"], at=at_n(1))
        self.isolated(run, f"{BIN}::wobbles", "RED", at=at_n(1))
        self.isolated(run, f"{BIN}::wobbles", "FLAKY", at=at_n(2), issue=4711)
        flakes.refresh(self.db, at=at_n(3))
        row = self.row(f"{BIN}::wobbles")
        self.assertEqual((row["reds"], row["flaky"]), (1, 0))

    def test_m2_a_red_isolation_in_the_run_owes_no_flake_issue(self):
        """Delta M2: round 1 RED, round 2 FLAKY -> no issue duty, no pending flake."""
        run = "run-000000000002"
        self.result_row(run, BIN, failed=["wobbles"], at=at_n(1))
        self.isolated(run, f"{BIN}::wobbles", "RED", at=at_n(1))
        self.isolated(run, f"{BIN}::wobbles", "FLAKY", at=at_n(2))
        gh = FakeGh()
        got = flakes.ingest_run(self.db, run, {}, at=at_n(3), gh=gh)
        self.assertEqual((got["pending"], got["issues"], gh.calls), ([], [], []))

    def test_m3_the_default_gh_runner_obeys_the_test_switch(self):
        """Delta M3: `MECLAW_FLAKE_ISSUES=0` stops the owner's `gh` itself, not only the kern."""
        def boom(*a, **kw):
            raise AssertionError("gh ran under the test switch")
        with mock.patch.object(flakes.subprocess, "run", boom), \
                mock.patch.dict(os.environ, {flakes.ISSUES_ENV: "0"}):
            rc, _ = flakes.GH_RUNNER(["gh", "issue", "create"])
        self.assertNotEqual(rc, 0)

    def test_m5_expected_failures_do_not_count_as_failed(self):
        text = "Ran 3 tests in 0.1s\n\nFAILED (failures=1, expected failures=2)\n"
        self.assertEqual(flakes.unittest_summary(text), (3, 1))

    def test_unittest_output_without_summary_is_unexplained(self):
        """N2: a cut output (no `Ran N tests`) never isolates what it happened to name."""
        text = "test_a (m.T.test_a) ... FAIL\n"
        results = flakes.unittest_test_results("unittests", text)
        self.assertTrue(flakes.unexplained(results))

    def test_unittest_summary_counting_more_than_named_is_unexplained(self):
        text = ("test_a (m.T.test_a) ... FAIL\n\n"
                "----------------------------------------------------------------------\n"
                "Ran 3 tests in 0.010s\n\nFAILED (failures=1, errors=1)\n")
        self.assertTrue(flakes.unexplained(flakes.unittest_test_results("unittests", text)))
        self.assertEqual(flakes.unittest_summary(text), (3, 2))

    def test_unittest_summary_that_matches_the_names_isolates(self):
        text = ("test_a (m.T.test_a) ... FAIL\ntest_b (m.T.test_b) ... ok\n\n"
                "Ran 2 tests in 0.010s\n\nFAILED (failures=1)\n")
        results = flakes.unittest_test_results("unittests", text)
        self.assertFalse(flakes.unexplained(results))
        self.assertEqual(flakes.unittest_summary("Ran 2 tests in 0.1s\n\nOK\n"), (2, 0))

    def test_a_stale_last_run_is_no_case_list(self):
        """N3: the runners exit 1 on a crash too and write last_run.json at the
        end only -- a file older than the station start is not this run's."""
        path = self.root / "last_run.json"
        path.write_text((FIX / "last_run.json").read_text())
        now = path.stat().st_mtime
        os.utime(path, (now - 600, now - 600))
        self.assertIsNone(flakes.scenario_last_run(str(path), now))
        os.utime(path, (now, now))
        cases, _errors = flakes.scenario_last_run(str(path), now)
        self.assertTrue(cases)
        self.assertIsNone(flakes.scenario_last_run(str(self.root / "missing.json"), now))

    def test_a_downgraded_flaky_station_carries_red_isolations(self):
        """N4: FLAKY with the wrong exit ends RED -- and its isolations say RED,
        so the register does not count a flake off a red station."""
        result = {"verdict": "FLAKY", "isolated": 1, "reason": "flaky: GH #1", "kind": "nextest",
                  "events": [{"station": "tests", "test": f"{BIN}::x", "count": 10,
                              "green": 10, "verdict": "FLAKY", "issue": 1}]}
        lines = flakes.end_station({"station": "tests", "verdict": "RED", "exit": 101}, result)
        self.assertEqual([body["verdict"] for _, body in lines], ["RED", "RED"])
        self.assertEqual(result["events"][0]["verdict"], "FLAKY")   # the input stays as it was

    def test_station_kind_and_scenario_argv(self):
        self.assertEqual(flakes.station_kind({"nextest": {}}, ["scripts/test-tier.sh"]), "nextest")
        self.assertEqual(flakes.station_kind({}, ["python3", "-m", "unittest", "m"]), "unittest")
        from pipeline import lane   # re-check tv-bn M7p M1: the kit's Python is two words
        self.assertEqual(flakes.station_kind({}, lane.python_argv("-m", "unittest", "m")), "unittest")
        self.assertEqual(flakes.station_kind({}, lane.python_argv("s.py", "unittest", "m")), "unittest")
        self.assertIsNone(flakes.station_kind({}, lane.python_argv("s.py", "m")))
        self.assertEqual(flakes.station_kind({}, ["python3", "run_builder_scenarios.py"]),
                         "scenario")
        self.assertIsNone(flakes.station_kind({}, ["sh", "-c", "false"]))
        self.assertEqual(flakes.scenario_base_argv(
            ["python3", "run_scenarios.py", "--only", "T1", "T3", "--workers", "2"]),
            ["python3", "run_scenarios.py", "--workers", "2"])
        self.assertEqual(flakes.scenario_runner_path(
            ["python3", "workshop/evals/scenarios/run_scenarios.py"], "/t"),
            "/t/workshop/evals/scenarios/last_run.json")


FLAKY_MODULE = """import os
import unittest

MARK = os.path.join(os.path.dirname(os.path.abspath(__file__)), "seen-" + __name__)


class T(unittest.TestCase):
    def test_wobbles(self):
        first = not os.path.exists(MARK)
        open(MARK, "a").close()
        self.assertFalse(first and {red_first}, "the first run is red")

    def test_holds(self):
        self.assertTrue(True)
"""


class TheExecutorHook(unittest.TestCase):
    """The executor isolates a red unittest station before its ONE `station_ended`."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="pipe-flake-")
        self.addCleanup(self.tmp.cleanup)
        self.root = pathlib.Path(self.tmp.name)
        self.tree = self.root / "tree"
        self.tree.mkdir()

    def dispatch(self, module):
        st = {"name": "unittests", "scope": "unittests", "cargo": 0, "stage": ["S"],
              "needs": {"cpu": 1, "colony": 0, "browser_chromium": 0, "browser_webkit": 0,
                        "export": 0, "cargo": 0},
              "inputs": [], "cmds": [{"argv": ["python3", "-m", "unittest", "-v", module],
                                      "cwd": ""}],
              "env": "none", "reason": ["test"], "timeout_min": 1, "after": [],
              "when": {"outcome": "success"}, "run": True, "keep_ws": True}
        return {"run": "run-0000f1a4e001", "repo": "meclaw", "mode": "strand", "rev": "a" * 40,
                "base": "b" * 40, "strand": "dt-test", "tree": str(self.tree),
                "target": str(self.root / "target"), "ws": str(self.root / "ws"),
                "stations": [st], "capacity": {"cpu": 4, "colony": 1, "browser_chromium": 1,
                                               "browser_webkit": 1, "export": 1, "cargo": 1},
                "cache_known": [], "live_runs": [], "expected_g": 0, "fingerprint_expected": {},
                "budget": {"free_min_g": 0, "reserve_g": 0}, "quarantine": [], "decide": [],
                # an hour ahead: a year 2999 deadline overflows the watcher's timer
                "deadline_at": (datetime.datetime.now(datetime.timezone.utc)
                                + datetime.timedelta(hours=1)).strftime("%Y-%m-%dT%H:%M:%SZ"),
                "+env": {}, "+nice": False, "+cpu_overbook": 1.0,
                "+export_root": str(self.root / "export"), "+fail_fast": False,
                "+no_precheck_stop": False, "+only": None}

    def run_station(self, module, red_first, always=False):
        text = FLAKY_MODULE.replace("{red_first}", red_first)
        if always:
            text = text.replace("first and True", "True")
        (self.tree / f"{module}.py").write_text(text)
        out, err = io.StringIO(), io.StringIO()
        executor.run(self.dispatch(module), out, err)
        return [json.loads(line) for line in out.getvalue().splitlines() if line.strip()], err

    def test_a_flake_is_isolated_and_the_station_ends_flaky(self):
        evs, err = self.run_station("wobbly_mod", "True")
        names = [e["event"] for e in evs]
        iso = [e["data"] for e in evs if e["event"] == "flake_isolated"]
        ended = [e["data"] for e in evs if e["event"] == "station_ended"]
        self.assertEqual(len(ended), 1, err.getvalue())
        self.assertEqual(ended[0]["verdict"], "FLAKY", ended[0])
        self.assertEqual(ended[0]["isolated"], 1)
        self.assertEqual(ended[0]["reason"], flakes.PENDING)
        self.assertEqual([(i["test"], i["green"], i["verdict"]) for i in iso],
                         [("wobbly_mod.T.test_wobbles", 3, "FLAKY")])   # T1, T2, T3
        self.assertLess(names.index("flake_isolated"), names.index("station_ended"))
        self.assertLess(names.index("station_ended"), names.index("pass_ended"))

    def test_a_defect_stays_red_after_its_isolation(self):
        evs, err = self.run_station("broken_mod", "True", always=True)
        ended = [e["data"] for e in evs if e["event"] == "station_ended"]
        iso = [e["data"] for e in evs if e["event"] == "flake_isolated"]
        self.assertEqual(ended[0]["verdict"], "RED", err.getvalue())
        self.assertEqual(ended[0]["reason"],  # R3 M2: the #1150 tail stays
                         "triage T1 red: defect, 1 of 1; at broken_mod.T.test_wobbles -- log logs/unittests.log")
        self.assertEqual([(i["green"], i["verdict"]) for i in iso], [(0, "RED")])

    def builder_station(self):
        """`scenarios:builder` red with the #721 signature on its first round only."""
        script = self.root / "builder.sh"
        script.write_text("#!/bin/sh\n"
                          "echo x >> \"$0.rounds\"\n"
                          "if [ -e \"$0.ran\" ]; then echo '17 cases, 17 ok'; exit 0; fi\n"
                          "touch \"$0.ran\"\n"
                          "echo \"A1 registry lacks '/os/builder/eyes'\"\n"
                          "exit 1\n")
        script.chmod(0o755)
        d = self.dispatch("unused")
        st = d["stations"][0]
        st.update(name=flakes.GH721_STATION, scope="17 cases",
                  cmds=[{"argv": [str(script)], "cwd": ""}])
        out, err = io.StringIO(), io.StringIO()
        executor.run(d, out, err)
        evs = [json.loads(line) for line in out.getvalue().splitlines() if line.strip()]
        return script, evs, err

    def test_the_builder_signature_gets_triage_not_a_deciding_retry(self):
        """TV-B8/OR-TV-2: a red round of `scenarios:builder` with `registry lacks`
        (GH #721) is a finding -- no second round decides it green. The executor
        ran the commands again and judged THAT exit (fund tv-bn-ci, Welle Testing)."""
        script, evs, err = self.builder_station()
        ended = [e["data"] for e in evs if e["event"] == "station_ended"]
        by_name = {e["station"]: e for e in ended}
        st = by_name[flakes.GH721_STATION]
        self.assertIn(st["verdict"], ("RED", "FLAKY"), (st, err.getvalue()))
        self.assertTrue(executor.ends_red(st), st)
        self.assertNotIn("retry", st.get("scope", ""), st)
        self.assertNotIn(flakes.GH721_STATION + "-retry", by_name, ended)
        # One round of the station's own commands; any further run is the triage's.
        self.assertEqual(1, len(pathlib.Path(str(script) + ".rounds").read_text().split()))


class TheKernHook(StoreCase):
    def test_the_issue_duty_is_off_under_the_test_switch(self):
        calls = []
        with mock.patch.object(flakes, "ingest_run",
                               lambda db, run, items, gh=None: calls.append(gh)):
            with mock.patch.dict(os.environ, {flakes.ISSUES_ENV: "0"}):
                gate._flake_ingest(self.db, "run-1", {})
            with mock.patch.dict(os.environ, {flakes.ISSUES_ENV: "1"}):
                gate._flake_ingest(self.db, "run-1", {})
        self.assertEqual(calls, [None, flakes.GH_RUNNER])

    def test_the_dispatch_carries_the_valid_quarantine(self):
        flakes.quarantine(self.db, f"{BIN}::wobbles", 4711, owner="o", cause="time", at=AT)
        got = gate._flake_quarantine(self.db)
        self.assertEqual([(q["test"], q["issue"]) for q in got], [(f"{BIN}::wobbles", 4711)])

    def test_the_quarantine_file_reaches_the_nextest_station(self):
        ws = self.root / "ws"
        (ws / "reuse").mkdir(parents=True)
        (ws / "reuse" / "binaries.json").write_text(
            json.dumps({"rust-binaries": {BIN: {"binary-id": BIN}}}))
        run = executor._Run.__new__(executor._Run)
        run.d = {"quarantine": [{"test": f"{BIN}::wobbles", "issue": 4711,
                                 "until": "2999-01-01T00:00:00Z"}]}
        run.ws, run.reuse_dir, run.err = str(ws), str(ws / "reuse"), io.StringIO()
        run.cond = __import__("threading").Condition()
        argv = run._quarantine_argv()
        self.assertEqual(argv[0], "--tool-config-file")
        text = (ws / "quarantine.toml").read_text()
        self.assertIn("# GH #4711", text)
        self.assertIn("[profile.isolate]", text)



UT_RED = ("test_holds (m.T.test_holds) ... ok\n"
          "test_wobbles (m.T.test_wobbles) ... FAIL\n\n"
          + "=" * 70 + "\nFAIL: test_wobbles (m.T.test_wobbles)\n" + "-" * 70
          + "\nTraceback (most recent call last):\nAssertionError\n\n" + "-" * 70
          + "\nRan 2 tests in 0.001s\n\nFAILED (failures=1)\n")
UT_ONE_OK = "test_wobbles (m.T.test_wobbles) ... ok\n\n" + "-" * 70 + "\nRan 1 test in 0.001s\n\nOK\n"


class FakeSpawn:
    """`_Run._spawn` replaced: one answer per argv, never a process."""

    def __init__(self, answer):
        self.answer, self.calls = answer, []

    def __call__(self, st, argv, cwd, env, log, t0, capture=None):
        self.calls.append((list(argv), t0))
        rc, text = self.answer(list(argv))
        log.write("$ %s\n%s" % (__import__("shlex").join(argv), text))
        log.flush()
        return rc, False


class TheSlotFixes(unittest.TestCase):
    """Delta check W8 slot: C1 (later commands), I1 (budget), I2 (proof of a run)."""

    setUp = TheExecutorHook.setUp
    dispatch = TheExecutorHook.dispatch

    def two_cmds(self):
        d = self.dispatch("m")
        d["stations"][0]["cmds"].append({"argv": ["python3", "later.py"], "cwd": ""})
        return d

    def run_fake(self, d, answer):
        fake = FakeSpawn(answer)
        out, err = io.StringIO(), io.StringIO()
        with mock.patch.object(executor._Run, "_spawn", new=fake):
            executor.run(d, out, err)
        evs = [json.loads(line) for line in out.getvalue().splitlines() if line.strip()]
        ended = [e["data"] for e in evs if e["event"] == "station_ended"]
        self.assertEqual(len(ended), 1, err.getvalue())
        return ended[0], fake, evs

    @staticmethod
    def answer(later_rc):
        def ans(argv):
            if argv[:2] == ["python3", "later.py"]:
                return later_rc, ""
            if "-k" in argv:
                return 0, UT_ONE_OK
            return 1, UT_RED
        return ans

    def test_c1_a_flaky_first_command_runs_the_later_ones_and_their_red_stays(self):
        ended, fake, _ = self.run_fake(self.two_cmds(), self.answer(1))
        argvs = [c[0] for c in fake.calls]
        self.assertEqual(argvs[-1], ["python3", "later.py"])
        self.assertEqual(sum(1 for a in argvs if "-k" in a), 2)   # T1 and T2 (OR-TV-2)
        self.assertEqual(ended["verdict"], "RED", ended)
        self.assertIn("later command", ended["reason"])
        self.assertNotEqual(ended.get("all_cmds"), True)

    def test_c1_a_flaky_first_command_with_green_later_ones_ends_flaky(self):
        ended, fake, _ = self.run_fake(self.two_cmds(), self.answer(0))
        self.assertEqual([c[0] for c in fake.calls][-1], ["python3", "later.py"])
        self.assertEqual(ended["verdict"], "FLAKY", ended)
        self.assertIs(ended["all_cmds"], True)

    def test_c1_the_later_commands_keep_the_stations_deadline(self):
        # Re-check tb-s7 H2 M-N3 (same pattern as the cache cut, executor.py
        # Delta C1): the later commands ran with `time.monotonic()` as their
        # start, a fresh `timeout_min` after the first command and its
        # isolation. The station has ONE deadline, counted from its start.
        ended, fake, _ = self.run_fake(self.two_cmds(), self.answer(0))
        first_t0 = fake.calls[0][1]
        later = [t0 for argv, t0 in fake.calls if argv[:2] == ["python3", "later.py"]]
        self.assertEqual([first_t0], later, ended)

    def test_c1_a_flaky_without_the_later_commands_is_red(self):
        res = {"verdict": "FLAKY", "isolated": 1, "reason": flakes.PENDING, "kind": "unittest",
               "events": [{"station": "s", "test": "t", "count": 10, "green": 10,
                           "verdict": "FLAKY"}], "later": {"ran": False, "exit": None}}
        lines = flakes.end_station({"station": "s", "verdict": "RED", "exit": 1}, res)
        final = lines[-1][1]
        self.assertEqual(final["verdict"], "RED")
        self.assertEqual(final["reason"], "not isolatable: later commands not run")
        self.assertEqual([e["verdict"] for ev, e in lines if ev == "flake_isolated"], ["RED"])

    def test_c1_a_flaky_station_is_never_cached(self):
        """Delta C1 cached a FLAKY station whose commands all ran; OR-TV-2 (VG TV-F1)
        makes FLAKY a finding -- no cache entry, all commands run or not."""
        got = []
        keys = {("s", ""): {"inputs": []}}
        evs = [{"event": "station_ended", "data": {"station": "s", "verdict": "FLAKY"}}]
        with mock.patch.object(gate.cache, "_tools", return_value={}), \
                mock.patch.object(gate.cache, "record",
                                  lambda db, run, runner, keys, results: got.append(results)):
            gate._record(None, "run-1", "r", keys, {}, {}, evs)
            evs[0]["data"]["all_cmds"] = True
            gate._record(None, "run-1", "r", keys, {}, {}, evs)
        self.assertEqual(got, [])
        evs[0]["data"]["verdict"] = "GREEN"
        with mock.patch.object(gate.cache, "_tools", return_value={}), \
                mock.patch.object(gate.cache, "record",
                                  lambda db, run, runner, keys, results: got.append(results)):
            gate._record(None, "run-1", "r", keys, {}, {}, evs)
        self.assertEqual(got, [[{"station": "s", "part": "", "verdict": "GREEN"}]])

    def test_i1_the_isolation_shares_one_clock(self):
        _, fake, _ = self.run_fake(self.two_cmds(), self.answer(0))
        t0s = {t0 for argv, t0 in fake.calls if "-k" in argv}
        self.assertEqual(len(t0s), 1, "every isolation run counts from the same start")

    def test_i1_a_spent_budget_stops_the_isolation_red(self):
        results = [red_result(["m1.K.test_a"], part="m1"), red_result(["m2.K.test_b"], part="m2")]
        run = FakeRun((0, UT_ONE_OK))
        out = flakes.triage_station("gate-selftest", "unittest", results, run, {},
                                     expired=lambda: len(run.calls) >= 3)
        self.assertEqual(out["verdict"], "RED")
        self.assertEqual(out["reason"], flakes.BUDGET_SPENT)
        self.assertEqual(len(run.calls), 3)
        self.assertTrue(all(e["verdict"] == "RED" for e in out["events"]))

    def test_i1_the_cap_holds_for_every_kind(self):
        reds = [red_result([f"m{i}.K.test_a"], part=f"m{i}") for i in range(6)]
        for kind in ("unittest", "scenario"):
            run = FakeRun((0, UT_ONE_OK))
            out = flakes.triage_station("s", kind, reds, run, {})
            self.assertEqual((out["verdict"], out["reason"], run.calls),
                             ("RED", flakes.TOO_MANY, []), kind)

    def test_i2_a_stress_run_without_a_run_per_iteration_is_red(self):
        summary = "     Summary [   0.015s] 10/10 stress run iterations: 10 passed\n"
        self.assertEqual(flakes.stress_verdict(0, summary), ("RED", 10))
        zero = "".join(
            f"  Stress test [   0.001s] iteration {i}/10: 0 tests run: 0 passed, 0 skipped\n"
            for i in range(1, 11)) + summary
        self.assertEqual(flakes.stress_verdict(0, zero)[0], "RED")
        self.assertEqual(flakes.stress_verdict(0, GREEN_SUMMARY), ("GREEN", 10))

    def test_i2_a_unittest_run_without_a_test_is_red(self):
        results = [red_result(["m1.K.test_a"], part="m1")]
        for text in ("", "\n" + "-" * 70 + "\nRan 0 tests in 0.000s\n\nOK\n"):
            out = flakes.triage_station("s", "unittest", results, FakeRun((0, text)), {})
            self.assertEqual(out["verdict"], "RED", repr(text))
        out = flakes.triage_station("s", "unittest", results, FakeRun((0, UT_ONE_OK)), {})
        self.assertEqual(out["verdict"], "FLAKY")

    def test_slot_a_skipped_unittest_run_is_no_run_evidence(self):
        # a test red in the run and skipped in isolation must not wash to FLAKY:
        # the evidence is Ran - skipped >= 1
        skipped = ("test_a (m1.K.test_a) ... skipped 'resource busy'\n\n" + "-" * 70
                   + "\nRan 1 test in 0.000s\n\nOK (skipped=1)\n")
        self.assertFalse(flakes.unittest_ran(skipped))
        results = [red_result(["m1.K.test_a"], part="m1")]
        out = flakes.triage_station("s", "unittest", results, FakeRun((0, skipped)), {})
        self.assertEqual(out["verdict"], "RED")
        partly = ("test_a (m1.K.test_a) ... ok\ntest_b (m1.K.test_b) ... skipped 'x'\n\n"
                  + "-" * 70 + "\nRan 2 tests in 0.000s\n\nOK (skipped=1)\n")
        self.assertTrue(flakes.unittest_ran(partly))

    def test_i2_a_scenario_run_needs_its_case_in_last_run(self):
        results = flakes.scenario_test_results("scenarios:memory", {"C2": "RED"})
        out = flakes.triage_station("scenarios:memory", "scenario", results, FakeRun((0, "")), {})
        self.assertEqual(out["verdict"], "RED")
        # the executor's proof: a fresh last_run.json naming the case green
        path = pathlib.Path(self.tmp.name) / "last_run.json"
        (pathlib.Path(self.tmp.name) / "run_scenarios.py").write_text("")
        run = executor._Run.__new__(executor._Run)
        run.tree, run.logs_dir, run.d = self.tmp.name, self.tmp.name, {"quarantine": []}
        seen = {}
        last = json.dumps({"results": [{"id": "C2", "verdict": "GREEN"}]})
        path.write_text(json.dumps({"results": [{"id": "C2", "verdict": "RED"}]}))

        def fake_isolate(*a, **kw):
            seen.update(kw)
            seen["run_cmd"] = a[3]
            return None
        with mock.patch.object(run, "_env", return_value={}), \
                mock.patch.object(run, "emit") as emit, \
                mock.patch.object(flakes, "triage_station", side_effect=fake_isolate):
            run._isolate({"name": "scenarios:memory", "timeout_min": 1},
                         (["python3", "run_scenarios.py"], "", {}), str(path), 0, [])
        self.assertEqual([c.args[0] for c in emit.call_args_list], ["test_result"])
        proof = seen["proof"]
        self.assertFalse(proof("scenarios:memory:case:C2", 0, ""), "a stale file is no proof")
        os.utime(path, ns=(path.stat().st_mtime_ns + 10**9,) * 2)
        path.write_text(last)
        os.utime(path, ns=(path.stat().st_mtime_ns + 2 * 10**9,) * 2)
        self.assertTrue(proof("scenarios:memory:case:C2", 0, ""))
        self.assertTrue(proof("case:C2", 0, ""), "the legacy key names the same case")
        self.assertFalse(proof("scenarios:memory:case:C3", 0, ""))

    def test_i2_a_run_ignored_station_isolates_with_the_ignored_tests(self):
        run = executor._Run.__new__(executor._Run)
        run.tree, run.logs_dir, run.reuse_dir = self.tmp.name, self.tmp.name, self.tmp.name
        run.d = {"quarantine": []}
        seen = {}
        argv = ["scripts/test-tier.sh", "filter", "x", "--run-ignored", "all"]
        with mock.patch.object(run, "_env", return_value={}), \
                mock.patch.object(run, "_quarantine_argv", return_value=["--tool-config-file", "q"]), \
                mock.patch.object(flakes, "junit_parts", return_value=[]), \
                mock.patch.object(flakes, "triage_station",
                                  side_effect=lambda *a, **kw: seen.update(kw)):
            run._isolate({"name": "browser:display", "nextest": {}, "timeout_min": 1},
                         (argv, "", {}), "log", 0, ["junit.xml"])
        self.assertIs(seen["ignored"], True)
        self.assertEqual(flakes.nextest_isolation_argv([], BIN, "a", ignored=True)[-3:-2],
                         ["all"])

    def test_m1_an_abort_during_the_isolation_ends_aborted_without_rows(self):
        def spawn(run, st, argv, cwd, env, log, t0, capture=None):
            log.write("$ %s\n" % " ".join(argv))
            if "-k" in argv:
                run.aborted = "deadline"
                return 143, False
            log.write(UT_RED)
            return 1, False
        out, err = io.StringIO(), io.StringIO()
        with mock.patch.object(executor._Run, "_spawn", spawn):
            executor.run(self.dispatch("m"), out, err)
        evs = [json.loads(line) for line in out.getvalue().splitlines() if line.strip()]
        ended = [e["data"] for e in evs if e["event"] == "station_ended"]
        self.assertEqual([e["verdict"] for e in ended], ["ABORTED"], err.getvalue())
        self.assertEqual([e for e in evs if e["event"] == "flake_isolated"], [])


class TheStoreLock(StoreCase):
    """welle-tb S3 (#1161): run-7b40b01d16cb lost its flake register rows to
    `gate: store: database is locked` -- a second writer committed while
    `ingest_run` still read the journal through an open cursor, so its
    `BEGIN IMMEDIATE` failed at once (a stale WAL snapshot; `busy_timeout`
    never waits for that)."""

    def foreign_writer(self):
        """A second connection that commits one journal row: another gate."""
        other = store.open(self.root / "pipeline.db")
        self.addCleanup(store.close, other)
        return lambda: events.emit(other, "lease_noted", "note", {"resource": "lane-x", "holder": "other", "run": "run-other",
                                           "ttl_s": None, "verb": "foreign"}, at=AT)

    def test_a_foreign_commit_during_the_ingest_loses_no_isolation(self):
        for name in ("wobbles", "shakes"):
            self.result_row("run-1", BIN, failed=[name])
            self.isolated("run-1", f"{BIN}::{name}", "FLAKY")
        commit, real = self.foreign_writer(), flakes._rows
        fired = []

        def rows(db, event):
            it = iter(real(db, event))
            first = next(it, None)
            if first is None:
                return
            if not fired:   # the other gate commits while this one still reads
                fired.append(commit())
            yield first
            yield from it

        with mock.patch.object(flakes, "_rows", rows):
            got = flakes.ingest_run(self.db, "run-1", {}, at=AT)
        self.assertEqual(1, len(fired))
        self.assertEqual(2, len(got["tasks"]), got)
        self.assertIsNotNone(self.row(f"{BIN}::wobbles"))
        self.assertIsNotNone(self.row(f"{BIN}::shakes"))

    def test_an_isolation_is_written_whole_or_not_at_all(self):
        # A write that fails halfway left the task without its run: the
        # retry then skipped the test (the task existed) and the run row
        # stayed lost for good.
        self.result_row("run-1", BIN, failed=["wobbles"])
        self.isolated("run-1", f"{BIN}::wobbles", "FLAKY")
        real, calls = store.run_open, []

        def run_open(*a, **kw):
            calls.append(1)
            if len(calls) == 1:
                raise sqlite3.OperationalError("database is locked")
            return real(*a, **kw)

        with mock.patch.object(flakes._store, "run_open", side_effect=run_open):
            with self.assertRaises(sqlite3.OperationalError):
                flakes.ingest_run(self.db, "run-1", {}, at=AT)
            self.assertEqual([], self.isolation_tasks("run-1"))
            got = flakes.ingest_run(self.db, "run-1", {}, at=AT)
        self.assertEqual(1, len(got["tasks"]), got)
        tasks = self.isolation_tasks("run-1")
        self.assertEqual(1, len(tasks), tasks)
        runs = self.db.execute("SELECT state, verdict FROM task_runs WHERE item=?",
                               (tasks[0],)).fetchall()
        self.assertEqual([("failed", "FLAKY")], [tuple(r) for r in runs])   # a finding (OR-TV-2)

    def isolation_tasks(self, run):
        return [r[0] for r in self.db.execute(
            "SELECT id FROM items WHERE type='task' AND json_extract(doc, '$.isolation.run')=?",
            (run,))]

SCENARIO_RUNNER = """import json, os, sys

HERE = os.path.dirname(os.path.abspath(__file__))
MARK = os.path.join(HERE, "seen-C1")
only = sys.argv[sys.argv.index("--only") + 1] if "--only" in sys.argv else None
results = []
for cid in ("C1", "C2"):
    if only is not None and cid != only:
        continue
    red = cid == "C1" and (ALWAYS or not os.path.exists(MARK))
    if cid == "C1":
        open(MARK, "a").close()
    results.append({"id": cid, "verdict": "RED" if red else "GREEN"})
with open(os.path.join(HERE, "last_run.json"), "w") as fh:
    json.dump({"results": results}, fh)
sys.exit(1 if any(r["verdict"] == "RED" for r in results) else 0)
"""


class TheNonNextestRegister(StoreCase):
    """welle-tb S3 (#1161): a unittest or scenario station whose isolation
    judges FLAKY writes its `test_result` lines -- the register and the issue
    duty see its tests like a nextest binary; a FLAKY test without a register
    row ends the ingest with an error, never with a silent `not in the
    register`."""

    dispatch = TheExecutorHook.dispatch

    def setUp(self):
        super().setUp()
        self.tree = self.root / "tree"
        self.tree.mkdir()

    def station(self, name, argv):
        d = self.dispatch("unused")
        d["stations"][0].update(name=name, scope=name, cmds=[{"argv": argv, "cwd": ""}])
        out, err = io.StringIO(), io.StringIO()
        executor.run(d, out, err)
        lines = [json.loads(line) for line in out.getvalue().splitlines() if line.strip()]
        for e in lines:
            events.emit(self.db, e["event"], e["kind"], e["data"], run=e["run"], at=e["at"])
        return d["run"], lines, err

    def unittest_station(self):
        (self.tree / "wobbly_mod.py").write_text(FLAKY_MODULE.replace("{red_first}", "True"))
        return self.station("unittests", ["python3", "-m", "unittest", "-v", "wobbly_mod"])

    def scenario_station(self, always=False):
        (self.tree / "run_scenarios.py").write_text(
            SCENARIO_RUNNER.replace("ALWAYS", "True" if always else "False"))
        return self.station("scenarios:memory", ["python3", "run_scenarios.py"])

    def test_a_flaky_unittest_station_reaches_the_register_and_the_issue_duty(self):
        run, lines, err = self.unittest_station()
        ended = [e["data"] for e in lines if e["event"] == "station_ended"]
        self.assertEqual(ended[0]["verdict"], "FLAKY", err.getvalue())
        res = flakes.ingest_run(self.db, run, {}, gh=FakeGh(), drafts=self.drafts)
        row = self.row("wobbly_mod.T.test_wobbles")
        self.assertIsNotNone(row, "the FLAKY unittest is in the register")
        self.assertEqual((row["station"], row["flaky"], row["reds"]), ("unittests", 1, 0))
        self.assertEqual([i["action"] for i in res["issues"]], ["create"], res)

    def test_a_flaky_scenario_station_reaches_the_register_and_the_issue_duty(self):
        run, lines, err = self.scenario_station()
        ended = [e["data"] for e in lines if e["event"] == "station_ended"]
        self.assertEqual(ended[0]["verdict"], "FLAKY", err.getvalue())
        parts = {e["data"]["part"]: e["data"]["verdict"] for e in lines
                 if e["event"] == "test_result"}
        self.assertEqual(parts, {"scenarios:memory:case:C1": "RED",
                                 "scenarios:memory:case:C2": "GREEN"})
        res = flakes.ingest_run(self.db, run, {}, gh=FakeGh(), drafts=self.drafts)
        row = self.row("scenarios:memory:case:C1")
        self.assertIsNotNone(row, "the FLAKY scenario case is in the register")
        self.assertEqual((row["station"], row["flaky"], row["reds"]), ("scenarios:memory", 1, 0))
        self.assertEqual([i["action"] for i in res["issues"]], ["create"], res)

    def test_a_flaky_scenario_case_without_its_names_is_not_green(self):
        # the journal of a lane before this fix: the isolation, no test_result
        run = "run-0000000000a5"
        self.isolated(run, "case:I5", "FLAKY", station="scenarios:memory")
        gh = FakeGh()
        with self.assertRaises(flakes.RegisterError) as cm:
            flakes.ingest_run(self.db, run, {}, gh=gh, drafts=self.drafts)
        self.assertIn("case:I5", str(cm.exception))
        self.assertEqual(gh.calls, [], "no issue duty on a register with a hole")
        # the kern: a lost register record, the pass ABORTED with the reason
        p = gate._Pass.__new__(gate._Pass)
        p.store_lost, p.err = [], io.StringIO()
        self.assertIsNone(p.store("flake register", flakes.ingest_run, self.db, run, {}))
        self.assertIn("RegisterError", p.lost_line())
        self.assertIn("case:I5", p.err.getvalue())

    def test_the_flake_share_counts_a_scenario_flake(self):
        from scripts.pipeline import retro
        run, _, err = self.scenario_station()
        flakes.ingest_run(self.db, run, {})
        # a later red pass whose only red test is that scenario case
        red_run = "run-0000000000b6"
        at = store.now()
        key = "scenarios:memory:case:C1"
        self.result_row(red_run, key, failed=[key], at=at, station="scenarios:memory")
        events.emit(self.db, "pass_ended", "status",
                    {"run": red_run, "verdict": "RED", "secs": 1.0, "stations": {},
                     "lane_seconds": 1.0, "critical_path_s": 1.0, "rounds": 1,
                     "cache_hits": 0},
                    run=red_run, at=at)
        value, reason = retro.flake_share(self.db, "2000-01-01T00:00:00Z",
                                          "2999-01-01T00:00:00Z")
        self.assertEqual(value, 100.0, (reason, err.getvalue()))


class TheStationKeys(StoreCase):
    """Re-check tb-s3-store2 (#1161), part 4a: a scenario key names its
    station (M-1), the fail-closed check asks for an observation of THIS run
    (m-1), the quarantine takes nextest keys only (m-2), and the hand
    `flakes issue` ends with a line on a store refusal (m-n1)."""

    def scenario_round(self, run, station, cid, verdict, at=AT, issue=None):
        results = flakes.scenario_test_results(station, {cid: "RED"})
        for r in results:
            events.emit(self.db, "test_result", "note", dict(r, run=run), run=run, at=at)
        key, = results[0]["failed_names"]
        self.isolated(run, key, verdict, at=at, issue=issue, station=station)
        return key

    def pass_ended(self, run, at):
        events.emit(self.db, "pass_ended", "status",
                    {"run": run, "verdict": "RED", "secs": 1.0, "stations": {},
                     "lane_seconds": 1.0, "critical_path_s": 1.0, "rounds": 1,
                     "cache_hits": 0}, run=run, at=at)

    # -- M-1 ------------------------------------------------------------------
    def test_the_same_case_id_in_two_scenario_suites_keeps_two_rows(self):
        from scripts.pipeline import retro
        mem = self.scenario_round("run-00000000aa01", "scenarios:memory", "L1", "FLAKY")
        bld = self.scenario_round("run-00000000aa02", "scenarios:builder", "L1", "RED")
        self.assertEqual((mem, bld), ("scenarios:memory:case:L1", "scenarios:builder:case:L1"))
        flakes.ingest_run(self.db, "run-00000000aa01", {})
        flakes.ingest_run(self.db, "run-00000000aa02", {})
        m, b = self.row(mem), self.row(bld)
        self.assertEqual((m["station"], m["runs"], m["flaky"], m["reds"]),
                         ("scenarios:memory", 1, 1, 0))
        self.assertEqual((b["station"], b["runs"], b["flaky"], b["reds"]),
                         ("scenarios:builder", 1, 0, 1))
        self.assertIsNone(self.row("case:L1"), "no row shared by two suites")
        # Q15: a red pass whose only red test is the builder case is a defect
        red, at = "run-00000000aa03", store.now()
        self.result_row(red, bld, failed=[bld], at=at, station="scenarios:builder")
        self.pass_ended(red, at)
        value, reason = retro.flake_share(self.db, "2000-01-01T00:00:00Z",
                                          "2999-01-01T00:00:00Z")
        self.assertEqual(value, 0.0, reason)

    def test_a_station_key_isolates_and_proves_its_case_id(self):
        key = "scenarios:memory:case:C2"
        self.assertEqual(flakes.case_id(key), "C2")
        self.assertEqual(flakes.case_id("case:C2"), "C2")
        self.assertEqual(flakes.scenario_isolation_argv(key, ["python3", "run.py"]),
                         ["python3", "run.py", "--only", "C2"])
        self.assertEqual(flakes.test_key(key, key), key)
        self.assertIsNone(flakes.split_test(key, [BIN]))

    def test_a_legacy_case_row_hands_its_issue_to_the_station_key(self):
        # the machine's register before this fix: `case:I5` under #1157
        flakes.register_external(self.db, "case:I5", "meclaw", 1157, "a lane log", at=AT)
        run = "run-00000000aa04"
        key = self.scenario_round(run, "scenarios:builder", "I5", "FLAKY")
        gh = FakeGh(issues=[{"number": 1157, "title": "flaky: case:I5", "state": "OPEN"}])
        res = flakes.ingest_run(self.db, run, {}, gh=gh, drafts=self.drafts)
        self.assertEqual(self.row(key)["issue"], "1157", "no second issue for the same test")
        self.assertNotIn("create", gh.verbs(), res)
        old = flakes.get(self.db, "case:I5")
        self.assertEqual(old["issue"], "1157", "the legacy row stays readable")
        self.assertIn("superseded by " + key, old["evidence"], "and names its successor")
        self.assertIn("a lane log", old["evidence"])
        # a journal line of an old lane kit (`case:<id>` with its station) lands there too
        run2 = "run-00000000aa05"
        self.result_row(run2, "case:I5", failed=["case:I5"], station="scenarios:builder")
        self.isolated(run2, "case:I5", "FLAKY", station="scenarios:builder")
        flakes.ingest_run(self.db, run2, {})
        self.assertEqual((self.row(key)["runs"], self.row(key)["flaky"]), (2, 2))
        self.assertEqual(self.row("case:I5")["runs"], 0, "the legacy row takes no observation")
        self.assertEqual(self.row("case:I5")["evidence"].count("superseded by"), 1)

    def test_unittest_keys_need_no_station(self):
        """M-1 checked for unittest: the key is the dotted module path; two
        stations could only share one through a module basename two test dirs
        share (a discover whose top level is its start dir)."""
        seen = {}
        for top in ("scripts", "workshop"):
            for p in (REPO / top).rglob("test_*.py"):
                if {"node_modules", "target", "__pycache__"} & set(p.parts):
                    continue
                seen.setdefault(p.name, []).append(str(p.relative_to(REPO)))
        self.assertEqual({k: v for k, v in seen.items() if len(v) > 1}, {})

    # -- m-1 ------------------------------------------------------------------
    def test_a_row_of_an_earlier_run_is_no_observation_of_this_one(self):
        self.result_row("run-00000000ab01", BIN, failed=["t"])
        self.isolated("run-00000000ab01", BIN + "::t", "FLAKY")
        flakes.ingest_run(self.db, "run-00000000ab01", {})
        self.isolated("run-00000000ab02", BIN + "::t", "FLAKY")   # no test_result of ab02
        with self.assertRaises(flakes.RegisterError) as cm:
            flakes.ingest_run(self.db, "run-00000000ab02", {})
        self.assertIn(BIN + "::t", str(cm.exception))
        self.assertEqual(self.row(BIN + "::t")["flaky"], 1)

    def test_an_issue_named_by_the_isolation_is_no_observation(self):
        self.isolated("run-00000000ab03", BIN + "::u", "FLAKY", issue=7)
        with self.assertRaises(flakes.RegisterError):
            flakes.ingest_run(self.db, "run-00000000ab03", {})
        self.assertIsNone(self.row(BIN + "::u"), "the check runs before any row is made")

    # -- m-2 ------------------------------------------------------------------
    def test_the_quarantine_takes_nextest_keys_only(self):
        scen, unit, nxt = "scenarios:memory:case:C1", "wobbly_mod.T.test_w", BIN + "::t"
        for run, key, station, part, name in (
                ("run-00000000ac01", scen, "scenarios:memory", scen, scen),
                ("run-00000000ac02", unit, "unittests", "wobbly_mod", unit),
                ("run-00000000ac03", nxt, "tests", BIN, "t")):
            self.result_row(run, part, failed=[name], station=station)
            self.isolated(run, key, "FLAKY", issue=9, station=station)
            flakes.ingest_run(self.db, run, {})
        self.assertIsNone(self.row(scen)["quarantined_until"])
        self.assertIsNone(self.row(unit)["quarantined_until"])
        self.assertIsNone(self.row(nxt)["quarantined_until"])     # proposed, never armed
        flakes.quarantine(self.db, nxt, 9, owner="o", cause="time", at=AT)
        self.assertIsNotNone(self.row(nxt)["quarantined_until"])
        with self.assertRaises(ValueError):
            flakes.quarantine(self.db, scen, 9, owner="o", cause="time", at=AT)
        # a quarantine set before this fix: no dispatch entry, no "quarantined", no expiry
        with store.tx(self.db):
            self.db.execute("UPDATE flakes SET quarantined_until=? WHERE test=?",
                            ("2999-01-01T00:00:00Z", unit))
            self.db.execute("UPDATE flakes SET quarantined_until=? WHERE test=?",
                            ("2000-01-01T00:00:00Z", scen))
        self.assertEqual([e["test"] for e in flakes.dispatch_quarantine(self.db, AT)], [nxt])
        self.assertEqual(flakes.status(self.row(unit), AT), "tracked")
        self.assertEqual(flakes.release_expired(self.db, AT), [])

    def test_the_issue_text_of_a_red_end_window_says_so(self):
        _, body = flakes.render_issue("scenarios:memory:case:C1", runs=3, reds=0, flaky=3,
                                      green=10, count=10, lane="fp-none")
        self.assertIn("the last 20 red ends", body)
        self.assertNotIn("the last 20 runs", body)
        self.assertIn("no quarantine", body)
        holes = {"test": r"[A-Za-z0-9_.:/-]+", "runs": r"\d+", "reds": r"\d+",
                 "flaky": r"\d+", "rate": r"\d\.\d\d", "green": r"\d+", "count": r"\d+",
                 "lane": r"fp-(?:[0-9a-f]{8}|none)"}
        pattern = re.escape(flakes.ISSUE_BODY_RED_ENDS)
        for k, v in holes.items():
            pattern = pattern.replace(re.escape("{%s}" % k), "(?:%s)" % v)
        self.assertTrue(re.fullmatch(pattern, body), body)
        _, body = flakes.render_issue(BIN + "::t", 20, 0, 1, 10, 10, "fp-none")
        self.assertIn("the last 20 runs", body)

    # -- m-n1 -----------------------------------------------------------------
    def test_the_hand_issue_command_ends_with_a_line_on_a_store_refusal(self):
        import argparse
        args = argparse.Namespace(test=BIN + "::t", drafts=self.drafts, max_new=5,
                                  dry_run=False)
        for exc in (store.StoreWriteError("database is locked"),
                    sqlite3.OperationalError("database is locked")):
            err = io.StringIO()
            with mock.patch.object(flakes, "issue_for", side_effect=exc), \
                    contextlib.redirect_stderr(err):
                rc = flakes._issue(self.db, args)
            self.assertEqual(rc, 2)
            self.assertEqual(err.getvalue(), "flakes: store: %s: database is locked\n"
                             % type(exc).__name__)


class TheDraftsStayOutOfTheTree(unittest.TestCase):
    """tb-s10 point 19: a FLAKY station's issue duty wrote its draft to
    `plans/flakes/issue-*.md` IN the strand worktree; the untracked file moved
    the tree hash and broke the GREEN `pre` stamp. The default folder is the
    tree's gate folder under the shared target (`<target>/gate/<tree>/flakes`)."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.repo = pathlib.Path(os.path.realpath(self._tmp.name)) / "wt"
        self.repo.mkdir()
        env = {"GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1",
               "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@example.invalid",
               "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@example.invalid"}
        self._env = mock.patch.dict(os.environ, env)
        self._env.start()
        os.environ.pop(flakes.DRAFTS_ENV, None)
        os.environ.pop("CARGO_TARGET_DIR", None)
        for argv in (["init", "-q", "-b", "master"], ["commit", "-q", "--allow-empty", "-m", "i"]):
            subprocess.run(["git", "-C", str(self.repo)] + argv, check=True, capture_output=True)
        (self.repo / ".gitignore").write_text("target/\n")
        self._repo = mock.patch.object(flakes, "REPO", self.repo)
        self._repo.start()

    def tearDown(self):
        self._repo.stop()
        self._env.stop()
        self._tmp.cleanup()

    def test_the_default_draft_folder_is_the_trees_gate_folder(self):
        self.assertEqual(flakes.default_drafts(), self.repo / "target" / "gate" / "wt" / "flakes")

    def test_the_pre_stamp_stays_green_after_a_flaky_draft(self):
        from scripts.pipeline import pre
        th = pre.tree_hash(str(self.repo))
        pre.write_stamp(str(self.repo), {"tree_hash": th, "at": pre._now().isoformat(),
                                         "verdict": "GREEN", "lane_half": True, "reasons": []})
        self.assertTrue(pre.stamp_status(str(self.repo))[0])
        res = flakes._draft(flakes.default_drafts(), "bin::a_test", "flaky: bin::a_test", "",
                            "gh unavailable")
        self.assertTrue(os.path.isfile(res["draft"]))
        self.assertEqual(pre.stamp_status(str(self.repo)), (True, mock.ANY))


class TheRegisterIssueReachesTheStation(StoreCase):
    """tb-s10 (tb-s8 finding, gate 4f2bf3c9c): the station wrote `issue pending`
    for gh1001 while the register carried #1180 -- the isolation knew only the
    issues of the runtime QUARANTINE, and gh1001 was out of it (a defect red
    in the window). The dispatch carries every register issue."""

    T = "meclaw-cells::gh1001_web_write_cost::gh1001_write_cost_does_not_grow"

    def test_the_dispatch_carries_a_register_issue_without_quarantine(self):
        flakes.register_external(self.db, self.T, "meclaw", 1180, "tb-s8 gate")
        self.assertEqual(flakes.dispatch_quarantine(self.db, "2026-10-10T00:00:00Z"), [])
        self.assertEqual(flakes.dispatch_issues(self.db), {self.T: 1180})

    def test_the_isolation_names_the_register_issue_not_pending(self):
        # a unittest key: the class the runtime quarantine never takes
        u = "test_mod.Case.test_x"
        d = {"quarantine": [], "flake_issues": {u: 1180}}
        self.assertEqual(executor.isolation_issues(d), {u: 1180})
        red = [{"station": "tests", "part": "test_mod", "verdict": "RED",
                "failed_names": [u], "tests": {"failed": 1}}]
        out = flakes.triage_station("tests", "unittest", red,
                                     lambda argv: (0, "Ran 1 test in 0.01s\n\nOK\n"),
                                     executor.isolation_issues(d))
        self.assertEqual((out["verdict"], out["reason"]), ("FLAKY", "flaky: GH #1180"))
        pending = flakes.triage_station("tests", "unittest", red,
                                         lambda argv: (0, "Ran 1 test in 0.01s\n\nOK\n"), {})
        self.assertEqual(pending["reason"], flakes.PENDING)

    def test_the_quarantine_issue_wins_over_the_register(self):
        d = {"quarantine": [{"test": self.T, "issue": 7, "until": "x"}],
             "flake_issues": {self.T: 1180, "b": "9"}}
        self.assertEqual(executor.isolation_issues(d), {self.T: 7, "b": 9})



# --- OR-TV-2 (welle-testing tv-p1c): FLAKY is a finding, triage, quarantine ------------

TRIAGE_NX_GREEN = "     Summary [   0.010s] 1 test run: 1 passed, 31 skipped\n"
TRIAGE_NX_RED = "     Summary [   0.010s] 1 test run: 0 passed, 1 failed, 31 skipped\n"
TRIAGE_NX_MANY_GREEN = "     Summary [   0.900s] 40 tests run: 40 passed, 0 skipped\n"
TRIAGE_NX_MANY_RED = "     Summary [   0.900s] 40 tests run: 39 passed, 1 failed, 0 skipped\n"
TRIAGE_UT_GREEN = "Ran 3 tests in 0.01s\n\nOK\n"
TRIAGE_UT_RED = "Ran 3 tests in 0.01s\n\nFAILED (failures=1)\n"


def red_part(name="tests::the_window_reopens", total=40, part=BIN):
    return [{"station": "tests", "part": part, "verdict": "RED", "failed_names": [name],
             "tests": {"passed": total - 1, "failed": 1, "skipped": 0}}]


def ut_red(test="scripts.tests.test_x.TheX.test_y"):
    return [{"station": "unittests", "part": "scripts.tests.test_x", "verdict": "RED",
             "failed_names": [test], "tests": {"failed": 1}}]


class Answers:
    """`run_cmd(argv, env=None)` answering by triage step; records (step, argv, env)."""

    def __init__(self, kind, red_steps=(), red_shift=None):
        self.kind, self.red, self.red_shift = kind, set(red_steps), red_shift
        self.calls = []

    def step(self, argv, env):
        if env and env.get(flakes.CLOCK_ENV):
            return "t4"
        if any("shuffle" in a for a in argv):
            return "t3"
        if self.kind == "nextest":
            return "t2" if "--test-threads" in argv else "t1"
        return "t2" if argv[-1].count(".") <= 2 and argv[-2] == "-k" else "t1"

    def __call__(self, argv, env=None):
        s = self.step(argv, env)
        self.calls.append((s, list(argv), dict(env or {})))
        red = s in self.red and (s != "t4" or env.get(flakes.CLOCK_ENV) == self.red_shift)
        if self.kind == "nextest":
            if s == "t2":
                return (100, TRIAGE_NX_MANY_RED) if red else (0, TRIAGE_NX_MANY_GREEN)
            return (100, TRIAGE_NX_RED) if red else (0, TRIAGE_NX_GREEN)
        return (1, TRIAGE_UT_RED) if red else (0, TRIAGE_UT_GREEN)

    def steps(self):
        return [c[0] for c in self.calls]


class TheTriage(unittest.TestCase):
    """TV-F2/TV-F5: T1 alone, T2 the harness under the station's threads, T3 shuffled
    with a logged seed, T4 the injected clock moved -- one cause class per flake."""

    def triage(self, results, answers, kind="nextest", **kw):
        base = (["cargo-nextest", "nextest", "run", "--profile", "t1", "-E", "all()"]
                if kind == "nextest" else ["python3", "-m", "unittest"])
        return flakes.triage_station("tests", kind, results, answers, {}, base_argv=base,
                                     threads=4, seed=4242, **kw)

    def test_triage_t1_red_is_a_defect(self):
        a = Answers("nextest", red_steps={"t1"})
        out = self.triage(red_part(), a)
        self.assertEqual(out["verdict"], "RED")
        self.assertIn(flakes.DEFECT, out["reason"])
        self.assertEqual(a.steps(), ["t1"])           # one run, never ten
        argv = a.calls[0][1]
        self.assertEqual(argv[argv.index("--profile") + 1], "isolate")
        self.assertEqual(argv[argv.index("--retries") + 1], "0")
        self.assertNotIn("--stress-count", argv)
        self.assertEqual(argv[argv.index("-E") + 1],
                         "binary_id(=%s) & test(=tests::the_window_reopens)" % BIN)
        self.assertEqual(out["events"][0]["verdict"], "RED")

    def test_t2_red_only_is_resource_load(self):
        a = Answers("nextest", red_steps={"t2"})
        out = self.triage(red_part(), a)
        self.assertEqual((out["verdict"], out["cause"]), ("FLAKY", flakes.CAUSE_T2))
        self.assertEqual(out["events"][0]["cause"], flakes.CAUSE_T2)
        t2 = [c[1] for c in a.calls if c[0] == "t2"][0]
        self.assertEqual(t2[t2.index("--test-threads") + 1], "4")
        self.assertEqual(t2[t2.index("-E") + 1], "binary_id(=%s)" % BIN)
        # over 200 tests in the binary: only the module `<stem>::` of the test
        a = Answers("nextest", red_steps={"t2"})
        self.triage(red_part(total=500), a)
        t2 = [c[1] for c in a.calls if c[0] == "t2"][0]
        self.assertEqual(t2[t2.index("-E") + 1], "binary_id(=%s) & test(/^tests::/)" % BIN)

    def test_t3_red_only_is_order_state(self):
        a = Answers("unittest", red_steps={"t3"})
        out = self.triage(ut_red(), a, kind="unittest")
        self.assertEqual((out["verdict"], out["cause"]), ("FLAKY", flakes.CAUSE_T3))
        self.assertEqual(a.steps(), ["t1", "t2", "t3"])
        t3 = a.calls[2][1]
        self.assertEqual(t3[t3.index("--seed") + 1], "4242")
        self.assertEqual(t3[-1], "scripts.tests.test_x")
        self.assertEqual(out["events"][0]["seed"], 4242)

    def test_t4_moves_the_injected_clock(self):
        test = "scripts.tests.test_x.TheX.test_y"
        a = Answers("unittest", red_steps={"t4"}, red_shift="year")
        out = self.triage(ut_red(test), a, kind="unittest", clock_tests={test},
                          clock_reader=True)
        self.assertEqual((out["verdict"], out["cause"]), ("FLAKY", flakes.CAUSE_T4))
        shifts = [c[2][flakes.CLOCK_ENV] for c in a.calls if c[0] == "t4"]
        self.assertEqual(shifts, ["+24h", "month", "year"])
        # without an injected clock there is no T4
        a = Answers("unittest", red_steps={"t4"}, red_shift="year")
        out = self.triage(ut_red(test), a, kind="unittest")
        self.assertNotIn("t4", a.steps())
        self.assertEqual(out["cause"], flakes.CAUSE_NONE)

    def test_t4_without_a_clock_reader_is_a_note(self):
        """Re-check A-M4: nothing reads CLOCK_ENV yet -- T4 says so (NOTE), runs
        nothing and gives no time class."""
        test = "scripts.tests.test_x.TheX.test_y"
        a = Answers("unittest", red_steps={"t4"}, red_shift="year")
        out = self.triage(ut_red(test), a, kind="unittest", clock_tests={test},
                          clock_reader=False)
        self.assertNotIn("t4", a.steps())
        self.assertEqual(out["events"][0]["triage"]["t4"], flakes.T4_UNAVAILABLE)
        self.assertTrue(flakes.T4_UNAVAILABLE.startswith("NOTE "))
        self.assertEqual(out["cause"], flakes.CAUSE_NONE)

    def test_the_clock_reader_flag_matches_the_tree(self):
        """`CLOCK_READER` is True exactly when some file outside the triage reads
        CLOCK_ENV (the kit clock of tv-s1-kit flips it)."""
        root = pathlib.Path(flakes.__file__).resolve().parents[2]
        proc = subprocess.run(["git", "-C", str(root), "grep", "-l", "-F", flakes.CLOCK_ENV,
                               "--", "scripts", "crates", ":!*.md",
                               ":!scripts/pipeline/flakes.py",
                               ":!scripts/tests/test_pipeline_flakes.py"],
                              capture_output=True, text=True)
        self.assertIn(proc.returncode, (0, 1), proc.stderr)
        self.assertEqual(flakes.CLOCK_READER, bool(proc.stdout.strip()), proc.stdout)

    def test_all_green_is_not_reproduced_and_still_red(self):
        a = Answers("nextest")
        out = self.triage(red_part(), a)
        self.assertEqual((out["verdict"], out["cause"]), ("FLAKY", flakes.CAUSE_NONE))
        ended = {"station": "tests", "verdict": "RED", "exit": 100, "secs": 1.0,
                 "scope": "s", "reason": "x"}
        final = flakes.end_station(ended, out)[-1][1]
        self.assertEqual((final["verdict"], final["cause"]), ("FLAKY", flakes.CAUSE_NONE))
        self.assertTrue(executor.ends_red(final))

    def test_the_budget_and_the_fail_closed_cases_stay(self):
        self.assertEqual(self.triage([{"verdict": "RED", "part": "cmd:1"}] + red_part(), Answers("nextest"))
                         ["reason"], flakes.UNEXPLAINED)
        out = self.triage(red_part(), Answers("nextest"), expired=lambda: True)
        self.assertEqual((out["verdict"], out["reason"]), ("RED", flakes.BUDGET_SPENT))


class TheFlakyCount(unittest.TestCase):
    """TV-F1/TV-F4: FLAKY counts red; only a legacy test, unchanged in the diff, is green."""

    def doc(self, *stations, verdict="RED"):
        return {"mode": "strand", "rev": "abcdef0", "secs": 1, "verdict": verdict,
                "stations": [dict({"name": "s%d" % i, "scope": "", "secs": 1}, **s)
                             for i, s in enumerate(stations)]}

    def test_a_flaky_station_makes_the_summary_red(self):
        flaky = {"station": "tests", "verdict": "FLAKY", "cause": flakes.CAUSE_NONE}
        self.assertTrue(executor.ends_red(flaky))
        self.assertFalse(executor.ends_red({"verdict": "GREEN"}))
        self.assertEqual(executor.pass_verdict([{"verdict": "GREEN"}, flaky]), "RED")
        self.assertEqual(executor.pass_verdict([{"verdict": "GREEN"}]), "GREEN")
        lines = receipt.gate_lines(self.doc({"verdict": "GREEN"}, {"verdict": "FLAKY"}))
        self.assertIn(" 1/2 ", lines[-1])
        self.assertEqual(receipt.legacy(self.doc({"verdict": "FLAKY"}))["stations"][0]["verdict"],
                         "RED")
        self.assertEqual(store.verdict_state("FLAKY"), ("failed", "flaky"))
        self.assertNotIn("FLAKY", cache.CACHEABLE_VERDICTS)
        self.assertNotIn("FLAKY", store.DONE_VERDICTS)

    def test_a_flaky_legacy_test_counts_green(self):
        repo, key = legacy_repo(self)
        base = git(repo, "rev-parse", "HEAD")
        (repo / "src" / "lib.rs").write_text(LIB_RS + "\nfn unrelated() {}\n")
        git(repo, "commit", "-qam", "elsewhere")
        self.assertEqual(flakes.legacy_tests(repo, base), [key])
        out = {"verdict": "FLAKY", "isolated": 1, "reason": flakes.PENDING, "kind": "nextest",
               "cause": flakes.CAUSE_NONE, "events": [{"station": "tests", "test": key,
                                                         "verdict": "FLAKY"}]}
        ended = {"station": "tests", "verdict": "RED", "exit": 100, "secs": 1.0, "scope": "s"}
        final = flakes.end_station(ended, out, legacy=[key])[-1][1]
        self.assertEqual((final["verdict"], final.get("legacy")), ("FLAKY", True))
        self.assertFalse(executor.ends_red(final))
        self.assertEqual(store.verdict_state("FLAKY", legacy=True), ("done", None))
        lines = receipt.gate_lines(self.doc({"verdict": "FLAKY", "legacy": True}, verdict="GREEN"))
        self.assertIn(" 1/1 ", lines[-1])
        # a FLAKY station with one test off the list stays red
        final = flakes.end_station(ended, out, legacy=["other::t"])[-1][1]
        self.assertNotIn("legacy", final)

    def test_a_changed_legacy_test_counts_red(self):
        repo, key = legacy_repo(self)
        base = git(repo, "rev-parse", "HEAD")
        (repo / "src" / "lib.rs").write_text(LIB_RS.replace("let x = 1;", "let x = 2;"))
        git(repo, "commit", "-qam", "inside the test fn")
        self.assertEqual(flakes.legacy_tests(repo, base), [])
        # no base (a full-scope pass): nothing is provably unchanged
        self.assertEqual(flakes.legacy_tests(repo, ""), [])

    def test_a_deleted_file_never_hides_a_changed_legacy_test(self):
        """Re-check A-M1: a deleted file in the diff is skipped unless it named the
        test in the BASE; it never ends the walk over the other files."""
        repo, key = legacy_repo(self)
        (repo / "src" / "a_old.rs").write_text("fn gone() {}\n")
        (repo / "src" / "b_old.rs").write_text("// the_window_reopens moved from here\n")
        git(repo, "add", "-A")
        git(repo, "commit", "-qm", "two more files")
        base = git(repo, "rev-parse", "HEAD")
        git(repo, "rm", "-q", "src/a_old.rs")
        git(repo, "commit", "-qm", "an unrelated file deleted")
        self.assertEqual(flakes.legacy_tests(repo, base), [key])
        (repo / "src" / "lib.rs").write_text(LIB_RS.replace("let x = 1;", "let x = 2;"))
        git(repo, "commit", "-qam", "inside the test fn, beside the deletion")
        self.assertEqual(flakes.legacy_tests(repo, base), [])
        git(repo, "checkout", "-q", base, "--", "src/lib.rs")
        git(repo, "rm", "-q", "src/b_old.rs")
        git(repo, "commit", "-qam", "a file that named the test in the base deleted")
        self.assertEqual(flakes.legacy_tests(repo, base), [])

    def test_attributes_and_decorators_belong_to_the_test(self):
        """Re-check A-M2: `#[...]` and `@...` lines above the fn are part of its span."""
        rs = LIB_RS.replace("    #[test]\n    fn the_window_reopens",
                            "    #[tokio::test(\n        flavor = \"multi_thread\",\n    )]\n"
                            "    // why it waits\n    fn the_window_reopens")
        self.assertEqual(flakes.fn_spans(rs, "the_window_reopens", ".rs"), [(3, 10)])
        self.assertEqual(flakes.fn_spans(LIB_RS, "another_one", ".rs"), [(9, 12)])
        py = ("class TheX:\n    x = 1\n\n    @mock.patch(\n        \"a.b\",\n    )\n"
              "    @unittest.skip\n    def test_y(self):\n        pass\n")
        self.assertEqual(flakes.fn_spans(py, "test_y", ".py"), [(4, 9)])
        repo, key = legacy_repo(self)
        base = git(repo, "rev-parse", "HEAD")
        (repo / "src" / "lib.rs").write_text(
            LIB_RS.replace("    #[test]\n    fn the_window_reopens",
                           "    #[test]\n    #[ignore]\n    fn the_window_reopens"))
        git(repo, "commit", "-qam", "an attribute of the test fn")
        self.assertEqual(flakes.legacy_tests(repo, base), [])


LIB_RS = """#[cfg(test)]
mod tests {
    #[test]
    fn the_window_reopens() {
        let x = 1;
        assert_eq!(x, x);
    }

    #[test]
    fn another_one() {
        assert!(true);
    }
}
"""


def git(repo, *args):
    return subprocess.run(["git", "-C", str(repo), *args], check=True, capture_output=True,
                          text=True).stdout.strip()


def legacy_repo(case):
    tmp = tempfile.TemporaryDirectory()
    case.addCleanup(tmp.cleanup)
    repo = pathlib.Path(tmp.name)
    git(repo, "init", "-q", "-b", "master")
    git(repo, "config", "user.email", "t@example.invalid")
    git(repo, "config", "user.name", "t")
    (repo / "src").mkdir()
    (repo / "src" / "lib.rs").write_text(LIB_RS)
    (repo / ".config").mkdir()
    key = BIN + "::tests::the_window_reopens"
    (repo / ".config" / "flaky-legacy.toml").write_text(
        '[[flaky]]\ntest = "%s"\nissue = 1180\ncause = "resource/load"\n\n'
        '[[flaky]]\ntest = "%s::tests::no_issue"\ncause = "time"\n' % (key, BIN))
    git(repo, "add", "-A")
    git(repo, "commit", "-qm", "base")
    return repo, key


class TheQuarantineArming(StoreCase):
    """TV-F3/TV-F9: the pipeline proposes, a quarantine is armed only with issue, owner,
    cause and expiry, at most eight; expiry without a measured fix is RED."""

    T = BIN + "::tests::the_window_reopens"

    def arm(self, test=None, **kw):
        args = dict(issue=1180, owner="test-high-1", cause="resource/load", days=14, at=AT)
        args.update(kw)
        return flakes.quarantine(self.db, test or self.T, **args)

    def test_a_quarantine_without_owner_or_cause_is_not_armed(self):
        for bad, why in (({"owner": ""}, "needs an owner"), ({"cause": None}, "needs a cause class"),
                         ({"cause": "bad luck"}, "needs a cause class"),
                         ({"issue": None}, "needs an issue"), ({"days": 15}, "runs 1 to 14 days")):
            with self.assertRaisesRegex(ValueError, why, msg=bad):
                self.arm(**bad)
            self.assertFalse((self.row(self.T) or {}).get("quarantined_until"))
        for n in range(1, 21):   # a 100 % flaky window with an issue: only a proposal
            self.observe(n, "tests::the_window_reopens", "flaky", issue=1180)
        flakes.refresh(self.db)
        self.assertEqual(flakes.auto_quarantine(self.db, [self.T], AT), [self.T])
        self.assertIsNone(self.row(self.T)["quarantined_until"])
        rules = [json.loads(d)["rule"] for (d,) in self.db.execute(
            "SELECT data FROM journal WHERE event='process_finding'")]
        self.assertIn("flake_quarantine_proposed", rules)
        until = self.arm()
        self.assertEqual(until, "2026-10-22T12:00:00Z")
        row = self.row(self.T)
        self.assertEqual((row["owner"], row["cause"], row["issue"]),
                         ("test-high-1", "resource/load", "1180"))

    def test_a_v3_quarantine_without_owner_or_cause_is_disarmed(self):
        """Re-check A-M3: a quarantine armed before migration 4 (no owner, no cause)
        is not armed after it -- the migration invents no owner (TV-F3)."""
        path = self.root / "v3.db"
        db = sqlite3.connect(str(path))
        db.execute("CREATE TABLE schema_version (version TEXT, applied_at TEXT)")
        for _n, sqls in store.MIGRATIONS[:3]:
            for sql in sqls:
                db.execute(sql)
        for n in (1, 2, 3):
            db.execute("INSERT INTO schema_version VALUES (?, ?)",
                       ("meclaw.pipeline.store/%d" % n, AT))
        db.execute("INSERT INTO flakes (test, repo, runs, issue, quarantined_until)"
                   " VALUES (?, 'meclaw', 20, '1180', '2026-10-24T19:32:01Z')", (self.T,))
        db.commit()
        db.close()
        db = store.open(path)
        self.addCleanup(store.close, db)
        self.assertEqual(flakes.dispatch_quarantine(db, AT), [])
        self.assertEqual(flakes.status(flakes.get(db, self.T), AT), "tracked")
        self.assertEqual(flakes.quarantine_filter(flakes.dispatch_quarantine(db, AT)), "")
        # armed with all four fields it is in the dispatch again
        flakes.quarantine(db, self.T, issue=1180, owner="test-high-1", cause="resource/load",
                          days=14, at=AT)
        self.assertEqual([q["test"] for q in flakes.dispatch_quarantine(db, AT)], [self.T])

    def test_quarantine_holds_at_most_eight(self):
        for i in range(8):
            self.arm(BIN + "::tests::t%d" % i)
        with self.assertRaisesRegex(flakes.QuarantineFull, "8 of 8 entries are active") as cm:
            self.arm(BIN + "::tests::t8")
        self.assertIn("quarantine full", str(cm.exception))
        self.arm(BIN + "::tests::t0")   # re-arming one of the eight is no ninth

    def test_a_disarmed_legacy_quarantine_writes_no_expiry_finding(self):
        # re-check tv-p1c-fix1 H1: a row armed before migration 4 (gh1001, #1180, no
        # owner) is never RED at its expiry (`dispatch_expired`), so it writes no
        # `flake_quarantine_expired` finding either -- the same owner filter.
        with store.tx(self.db):
            self.db.execute("INSERT INTO flakes (test, repo, runs, issue, quarantined_until)"
                            " VALUES (?, 'meclaw', 20, '1180', '2026-10-24T19:32:01Z')",
                            (self.T,))
        later = "2026-10-25T00:00:00Z"
        self.assertEqual(flakes.dispatch_expired(self.db, later), [])
        self.assertEqual(flakes.release_expired(self.db, later), [])
        rules = [json.loads(d)["rule"] for (d,) in self.db.execute(
            "SELECT data FROM journal WHERE event='process_finding'")]
        self.assertNotIn("flake_quarantine_expired", rules)

    def test_expiry_without_a_fix_is_red(self):
        self.arm()
        later = "2026-10-23T12:00:00Z"
        self.assertEqual(flakes.release_expired(self.db, later), [self.T])
        self.assertEqual(flakes.dispatch_expired(self.db, later), [self.T])
        self.assertEqual(flakes.quarantine_station_verdict(0, [self.T])[0], "RED")
        with self.assertRaisesRegex(ValueError, "a fix is measured"):
            flakes.record_fix(self.db, self.T, quiet=20, load=19, at=later)
        flakes.record_fix(self.db, self.T, quiet=20, load=20, at=later)
        self.assertEqual(flakes.dispatch_expired(self.db, later), [])

    def test_a_closed_observation_issue_is_reopened(self):
        gh = FakeGh(issues=[{"number": 1180, "title": "flaky: x", "state": "CLOSED",
                             "labels": [{"name": "observation"}]}])
        for n in range(1, 3):
            self.observe(n, "tests::the_window_reopens", "flaky", issue=1180)
        flakes.refresh(self.db)
        got = flakes.issue_for(self.db, self.T, gh=gh, drafts=self.drafts, at=AT)
        self.assertEqual((got["action"], got["issue"]), ("reopen", 1180))
        self.assertIn("reopen", gh.verbs())
        self.assertNotIn("create", gh.verbs())
        texts = [json.loads(d) for (d,) in self.db.execute(
            "SELECT data FROM journal WHERE event='process_finding'")]
        self.assertTrue(any(t["rule"] == "flake_issue_reopened" and t["issue"] == 1180
                            for t in texts))

    def test_a_quarantined_test_runs_in_the_note_station(self):
        tests = {"name": "tests", "nextest": {}, "after": ["build"], "when": {"outcome": "success"},
                 "cmds": [{"argv": ["scripts/test-tier.sh", "t1"], "cwd": ""}], "timeout_min": 30}
        entries = [{"test": self.T, "issue": 1180, "until": "2026-10-22T12:00:00Z"}]
        st = flakes.quarantine_station(tests, entries, {BIN})
        self.assertEqual(st["name"], flakes.QUARANTINE_STATION)
        self.assertEqual(st["after"], ["build"])
        self.assertEqual(st["nextest"]["profile"], "quarantine")
        profile = tomllib.loads((REPO / ".config" / "nextest.toml").read_text())["profile"]
        self.assertEqual((profile["quarantine"]["flaky-result"], profile["quarantine"]["retries"]),
                         ("fail", 0))
        text, _ = flakes.quarantine_toml(entries, {BIN}, profiles=flakes.QUARANTINE_PROFILES)
        quarantine_overrides = tomllib.loads(text)["profile"]["quarantine"]["overrides"]
        self.assertEqual([(o["retries"], o["flaky-result"]) for o in quarantine_overrides],
                         [(2, "fail")])
        expr = "binary_id(=%s) & test(=tests::the_window_reopens)" % BIN
        self.assertEqual(st["cmds"][0]["argv"], ["scripts/test-tier.sh", "filter", expr])
        self.assertEqual(flakes.quarantine_filter(entries, {BIN}), expr)
        self.assertEqual(flakes.quarantine_filter(entries, {"other"}), "")
        self.assertEqual(flakes.quarantine_station_verdict(100, [])[0], "NOTE")
        self.assertEqual(flakes.quarantine_station_verdict(0, [])[0], "GREEN")
        text, _ = flakes.quarantine_toml(entries, {BIN})
        for block in tomllib.loads(text)["profile"]["default"]["overrides"]:
            self.assertEqual(block["flaky-result"], "fail")
        out = subprocess.run(
            [str(REPO / "scripts" / "test-tier.sh"), "filter", "all()"], capture_output=True,
            text=True, env=dict(os.environ, MECLAW_TIER_DRY="1", MECLAW_TIER_EXCLUDE=expr,
                                MECLAW_TIER_REUSE="/nonexistent", MECLAW_TIER_LANE="1"))
        self.assertIn("(all()) - (%s)" % expr, out.stdout + out.stderr)


if __name__ == "__main__":
    unittest.main()
