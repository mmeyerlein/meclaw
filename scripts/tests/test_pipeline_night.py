"""Tests for `scripts/pipeline/night.py` -- the night report as a watchdog.

The timer is armed only by a `pass_ended` (never by any other event, never
for a store other than the default one) and kept while it is pending; a night
without runs since the last report writes nothing and journals nothing; red
runs are classified `defect|flake|hygiene|foreign` from journal data only; the
proposals are rule output (`retro.proposal_lines`), sorted and the same for
the same findings. No test ever reaches the real `systemctl`/`systemd-run`:
every call goes to an injected fake runner.
"""

import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile
import types
import unittest
from datetime import datetime, timezone
from unittest import mock
from scripts.tests import store_isolation

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO))

from scripts.pipeline import events, store  # noqa: E402

try:  # Part A builds retro.py in parallel; until it lands, a stand-in keeps the import alive.
    from scripts.pipeline import retro  # noqa: E402
    RETRO_REAL = True
except ImportError:  # pragma: no cover -- only while retro.py is missing
    RETRO_REAL = False
    import scripts.pipeline as _pkg
    retro = types.ModuleType("scripts.pipeline.retro")
    retro.RULES = ()
    retro.evaluate = lambda db, now=None: []
    retro.open_findings = lambda db: []
    retro.proposal_lines = lambda findings: []
    sys.modules["scripts.pipeline.retro"] = retro
    _pkg.retro = retro

from scripts.pipeline import night  # noqa: E402

# No push from this module may reach the metrics or Grafana target of the caller.
_ENV = mock.patch.dict(os.environ, {"MECLAW_PIPELINE_METRICS_URL": "", "MECLAW_GRAFANA_URL": "",
                                    "GRAFANA_ADMIN_PASS": "",
                                    "MECLAW_PIPELINE_NIGHT": "0"})


def setUpModule():
    store_isolation.isolate()
    _ENV.start()


def tearDownModule():
    _ENV.stop()
    store_isolation.restore()


DAY = "2026-10-07"
NOW = "2026-10-08T01:00:00Z"


class FakeRunner:
    """Records every argv; `systemctl is-active` answers `active` once `armed`."""

    def __init__(self, active=False):
        self.calls = []
        self.active = active

    def __call__(self, argv, **kwargs):
        self.calls.append(list(argv))
        if argv[:3] == ["systemctl", "--user", "is-active"]:
            out = "active\n" if self.active else "inactive\n"
            return subprocess.CompletedProcess(argv, 0 if self.active else 3, out, "")
        if argv and argv[0] == "systemd-run":
            self.active = True
        return subprocess.CompletedProcess(argv, 0, "", "")

    def named(self, head):
        return [c for c in self.calls if c and c[0] == head]


def _store(test):
    tmp = tempfile.TemporaryDirectory()
    test.addCleanup(tmp.cleanup)
    db = store.open(pathlib.Path(tmp.name) / "p.db")
    test.addCleanup(db.close)
    return db, pathlib.Path(tmp.name)


def _plan(db, run, at, mode="strand", repo="meclaw"):
    events.emit(db, "pass_planned", "status",
                {"run": run, "mode": mode, "repo": repo, "rev": "r", "base": "b",
                 "stations": [], "reason": {}, "cache_known": 0, "dirty": False},
                run=run, at=at)
    events.emit(db, "run_dispatched", "note",
                {"run": run, "runner": "lane-a", "tree": "t", "attempt": 1, "round": 1,
                 "deadline_at": None, "ws": None}, run=run, at=at)


def _run(db, run, at, verdict, stations, mode="strand", extra=()):
    """One whole run: plan, stations `(name, verdict, secs, exit, fields)`, extras, end."""
    _plan(db, run, at, mode)
    for name, sv, secs, code, fields in stations:
        data = {"run": run, "station": name, "verdict": sv, "secs": secs, "exit": code,
                "scope": "local"}
        data.update(fields)
        events.emit(db, "station_ended", "status", data, run=run, at=at)
    for event, kind, data in extra:
        events.emit(db, event, kind, dict(data, run=run), run=run, at=at)
    counts = {}
    for _, sv, *_ in stations:
        counts[sv.lower()] = counts.get(sv.lower(), 0) + 1
    events.emit(db, "pass_ended", "status",
                {"run": run, "verdict": verdict, "secs": 60, "lane_seconds": 120,
                 "critical_path_s": 50, "rounds": 1, "cache_hits": counts.get("cached", 0),
                 "stations": counts}, run=run, at=at)


def _finding(db, rule, subject, value, threshold, at="2026-10-07T11:00:00Z"):
    return events.emit(db, "process_finding", "finding",
                       {"rule": rule, "subject": subject, "value": value,
                        "threshold": threshold, "since": at}, at=at)


def _journal_count(db):
    return db.execute("SELECT count(*) FROM journal").fetchone()[0]


class TimerTest(unittest.TestCase):
    def test_night_timer_armed_only_after_pass_ended(self):
        db, _ = _store(self)
        fake = FakeRunner()
        with mock.patch.dict(os.environ, {"MECLAW_PIPELINE_NIGHT": "1"}), \
                mock.patch.object(night, "RUNNER", fake), \
                mock.patch.object(events, "HOOKS", [night.hook]):
            _plan(db, "run-a", "2026-10-07T12:00:00Z")
            events.emit(db, "station_ended", "status",
                        {"run": "run-a", "station": "fmt", "verdict": "GREEN", "secs": 1,
                         "exit": 0, "scope": "local"}, run="run-a", at="2026-10-07T12:00:00Z")
            self.assertEqual(fake.calls, [], "only pass_ended may touch the timer")
            events.emit(db, "pass_ended", "status",
                        {"run": "run-a", "verdict": "GREEN", "secs": 1, "lane_seconds": 1,
                         "critical_path_s": 1, "rounds": 1, "cache_hits": 0,
                         "stations": {"green": 1}}, run="run-a", at="2026-10-07T12:00:00Z")
            self.assertEqual(len(fake.named("systemd-run")), 1)
            argv = fake.named("systemd-run")[0]
            self.assertIn("--user", argv)
            self.assertEqual(argv[argv.index("--unit") + 1], "pipe-night")
            cal = [a for a in argv if a.startswith("--on-calendar=")]
            self.assertEqual(len(cal), 1)
            self.assertRegex(cal[0], r"^--on-calendar=\d{4}-\d\d-\d\d 02:00:00$")
            self.assertIn("--timer-property=RemainAfterElapse=no", argv)
            self.assertEqual(argv[-1], "night")
            self.assertFalse(any("cargo" in a for a in argv), "the timer never starts cargo")
            # A second run while the timer is pending keeps it (moves nothing).
            events.emit(db, "pass_ended", "status",
                        {"run": "run-b", "verdict": "RED", "secs": 1, "lane_seconds": 1,
                         "critical_path_s": 1, "rounds": 1, "cache_hits": 0,
                         "stations": {"red": 1}}, run="run-b", at="2026-10-07T13:00:00Z")
            self.assertEqual(len(fake.named("systemd-run")), 1)
            self.assertEqual(len(fake.named("systemctl")), 2)
            self.assertEqual(night.arm(db, runner=FakeRunner(active=True),
                                       store_path=night.home_store()), "kept")

    def test_night_never_arms_outside_the_default_store(self):
        db, tmp = _store(self)
        fake = FakeRunner()
        with mock.patch.dict(os.environ, {"MECLAW_PIPELINE_DB": str(tmp / "p.db")}):
            os.environ.pop("MECLAW_PIPELINE_NIGHT", None)
            self.assertEqual(night.arm(db, runner=fake), "skipped")
        with mock.patch.dict(os.environ, {"MECLAW_PIPELINE_NIGHT": "0"}):
            self.assertEqual(night.arm(db, runner=fake, store_path=night.home_store()),
                             "disabled")
        self.assertEqual(fake.calls, [])
        os.environ.pop("MECLAW_PIPELINE_NIGHT", None)
        self.assertEqual(night.arm(db, runner=fake, store_path=night.home_store()), "armed")
        argv = fake.named("systemd-run")[0]
        self.assertIn("--setenv=MECLAW_PIPELINE_DB=%s" % night.home_store(), argv)

    def test_night_next_0200_is_after_the_local_day_of_the_run(self):
        self.assertEqual(night.next_0200(datetime(2026, 10, 8, 23, 30)), "2026-10-09 02:00:00")
        self.assertEqual(night.next_0200(datetime(2026, 10, 9, 1, 0)), "2026-10-10 02:00:00")
        self.assertEqual(night.next_0200(datetime(2026, 12, 31, 12, 0)), "2027-01-01 02:00:00")

    def test_night_timer_script_prefers_the_main_tree(self):
        with tempfile.TemporaryDirectory() as tmp:
            main, wt = pathlib.Path(tmp, "main"), pathlib.Path(tmp, "wt")
            (main / ".git/worktrees/wt").mkdir(parents=True)
            (main / "scripts/pipeline").mkdir(parents=True)
            wt.mkdir()
            (wt / ".git").write_text("gitdir: %s\n" % (main / ".git/worktrees/wt"))
            self.assertEqual(night.timer_script(wt), wt / "scripts/pipeline.py")
            (main / "scripts/pipeline/night.py").write_text("")
            self.assertEqual(night.timer_script(wt), main / "scripts/pipeline.py")
            self.assertEqual(night.timer_script(main), main / "scripts/pipeline.py")


class ReportTest(unittest.TestCase):
    def setUp(self):
        if not RETRO_REAL:
            return
        for name, value in (("evaluate", lambda db, now=None: []),):
            patcher = mock.patch.object(night.retro, name, value)
            patcher.start()
            self.addCleanup(patcher.stop)

    def test_night_report_classifies_red_reasons(self):
        db, tmp = _store(self)
        at = DAY + "T12:00:00Z"
        _run(db, "run-green", at, "GREEN", [("fmt", "GREEN", 5, 0, {})])
        _run(db, "run-defect", at, "RED", [("tests", "RED", 30, 1, {})])
        _run(db, "run-isolated", at, "RED", [("tests", "RED", 30, 1, {})], extra=[
            ("flake_isolated", "finding",
             {"station": "tests", "test": "a::b", "count": 3, "green": 2, "verdict": "FLAKY"})])
        db.execute("INSERT INTO flakes (test, station, repo, runs, reds, flaky)"
                   " VALUES ('c::d', 'tests', 'meclaw', 10, 1, 1)")
        _run(db, "run-known-flake", at, "RED",
             [("tests", "RED", 30, 1, {"failed_names": ["c::d"]})])
        _run(db, "run-watchdog", at, "ABORTED", [("tests", "ABORTED", 30, 137,
                                                   {"reason": "aborted_disk"})], extra=[
            ("watchdog_fired", "finding", {"runner": "lane-a", "kind": "disk", "value": 3,
                                           "threshold": 20, "action": "aborted"})])
        _run(db, "run-ssh", at, "RED", [("clippy", "RED", 2, 255, {})])
        _plan(db, "run-lost", DAY + "T10:00:00Z")  # planned, never ended: a lost run

        classes = night.classify(db, ["run-defect", "run-isolated", "run-known-flake",
                                      "run-watchdog", "run-ssh"])
        self.assertEqual(classes, {"run-defect": "defect", "run-isolated": "flake",
                                   "run-known-flake": "flake", "run-watchdog": "hygiene",
                                   "run-ssh": "foreign"})
        self.assertEqual([c for c, *_ in night.RED_REASONS],
                         ["hygiene", "foreign", "flake", "defect"])

        before = _journal_count(db)
        path = night.report(db, now=NOW, out_root=tmp / "nacht", runner=FakeRunner(), date=DAY)
        self.assertEqual(path, tmp / "nacht" / (DAY + ".md"))
        text = path.read_text()
        lines = text.splitlines()
        self.assertLessEqual(len(lines), 60)
        self.assertTrue(text.isascii(), "the report is English/ASCII")
        row = [ln for ln in lines if ln.startswith("| " + DAY)]
        self.assertEqual(len(row), 1, text)
        cells = [c.strip() for c in row[0].strip("|").split("|")]
        # date | runs | green | red | lane min | defect | flake | hygiene | foreign | lost
        self.assertEqual(cells, [DAY, "6", "1", "5", "12.0", "1", "2", "1", "2", "1"])
        rows = db.execute("SELECT seq, data FROM journal WHERE event='night_report'").fetchall()
        self.assertEqual(len(rows), 1)
        self.assertIn('"runs": 6', rows[0][1])
        self.assertGreater(_journal_count(db), before)

    # Re-check tb-s3-store2 m-n2: a pass the store lost a record of ended
    # ABORTED (`store_record_lost`); the night report read its stream's GREEN.
    def test_night_report_counts_a_store_lost_pass_red_and_foreign(self):
        db, tmp = _store(self)
        at = DAY + "T12:00:00Z"
        _run(db, "run-green", at, "GREEN", [("fmt", "GREEN", 5, 0, {})])
        _run(db, "run-store-lost", at, "GREEN", [("fmt", "GREEN", 5, 0, {})])
        events.emit(db, "process_finding", "finding",
                    {"rule": events.STORE_LOST_RULE, "subject": "run-store-lost", "value": 1,
                     "threshold": 0, "since": at, "records": ["close"]},
                    run="run-store-lost", at=at)
        self.assertEqual(night.classify(db, ["run-store-lost"]), {"run-store-lost": "foreign"})
        path = night.report(db, now=NOW, out_root=tmp / "nacht", runner=FakeRunner(), date=DAY)
        row = [ln for ln in path.read_text().splitlines() if ln.startswith("| " + DAY)]
        cells = [c.strip() for c in row[0].strip("|").split("|")]
        # date | runs | green | red | lane min | defect | flake | hygiene | foreign | lost
        self.assertEqual(cells, [DAY, "2", "1", "1", "4.0", "0", "0", "0", "1", "0"])

    # Re-check tb-s3-store2 m-3, the night side: `classify` matched raw
    # `failed_names` against register keys, so a nextest flake (`d` against
    # `c::d`) never counted and a legacy scenario line (`case:L1` beside its
    # station) missed its station key. One rule with Q15 (`retro._flake_keys`),
    # and only a row the register counts as flaky (`flaky > 0`): rows exist for
    # every test ever red, so a known defect must stay a defect.
    def test_night_classify_matches_failed_tests_by_the_register_key(self):
        db, _ = _store(self)
        at = DAY + "T12:00:00Z"
        for test, station, reds, flaky in (
                ("c::d", "tests", 1, 1),
                ("scripts.tests.test_x.T.test_y", "python", 1, 1),
                ("scenarios:memory:case:L1", "scenarios:memory", 0, 1),
                ("c::e", "tests", 3, 0)):
            db.execute("INSERT INTO flakes (test, station, repo, runs, reds, flaky)"
                       " VALUES (?, ?, 'meclaw', 10, ?, ?)", (test, station, reds, flaky))

        def red(run, station, part, names):
            _run(db, run, at, "RED", [(station, "RED", 30, 1, {})], extra=[
                ("test_result", "note", {"station": station, "part": part, "verdict": "RED",
                                         "secs": 1.0, "failed_names": names})])

        red("run-nextest-flake", "tests", "c", ["d"])
        red("run-unittest-flake", "python", "scripts.tests.test_x",
            ["scripts.tests.test_x.T.test_y"])
        red("run-scenario-flake", "scenarios:memory", "scenarios:memory:case:L1",
            ["scenarios:memory:case:L1"])
        red("run-legacy-scenario-flake", "scenarios:memory", "case:L1", ["case:L1"])
        red("run-other-suite-L1", "scenarios:builder", "scenarios:builder:case:L1",
            ["scenarios:builder:case:L1"])
        red("run-known-defect", "tests", "c", ["e"])
        red("run-mixed", "tests", "c", ["d", "e"])
        self.assertEqual(night.classify(db, [
            "run-nextest-flake", "run-unittest-flake", "run-scenario-flake",
            "run-legacy-scenario-flake", "run-other-suite-L1", "run-known-defect",
            "run-mixed"]), {
            "run-nextest-flake": "flake", "run-unittest-flake": "flake",
            "run-scenario-flake": "flake", "run-legacy-scenario-flake": "flake",
            "run-other-suite-L1": "defect", "run-known-defect": "defect",
            "run-mixed": "defect"})

    def test_night_report_none_without_runs_since_last_report(self):
        db, tmp = _store(self)
        out = tmp / "nacht"
        before = _journal_count(db)
        self.assertIsNone(night.report(db, now=NOW, out_root=out, runner=FakeRunner(), date=DAY))
        self.assertEqual(_journal_count(db), before)
        self.assertFalse(out.exists())
        _run(db, "run-1", DAY + "T12:00:00Z", "GREEN", [("fmt", "GREEN", 5, 0, {})])
        self.assertIsNotNone(night.report(db, now=NOW, out_root=out, runner=FakeRunner(),
                                          date=DAY))
        before = _journal_count(db)
        self.assertIsNone(night.report(db, now="2026-10-09T01:00:00Z", out_root=out,
                                       runner=FakeRunner(), date="2026-10-08"))
        self.assertIsNone(night.report(db, now=NOW, out_root=out, runner=FakeRunner(), date=DAY))
        self.assertEqual(_journal_count(db), before)
        self.assertEqual(sorted(p.name for p in out.iterdir()), [DAY + ".md"])

    def test_night_coverage_skipped_without_lane_never_powers_one(self):
        db, tmp = _store(self)
        _run(db, "run-1", DAY + "T12:00:00Z", "GREEN", [("fmt", "GREEN", 5, 0, {})])
        events.emit(db, "merge_done", "status", {"merge": "m-1", "strand": "s", "base": "b"},
                    at=DAY + "T12:30:00Z")
        fake = FakeRunner()
        with mock.patch.object(night, "coverage_cli", lambda: None):
            night.report(db, now=NOW, out_root=tmp / "nacht", runner=fake, date=DAY)
        rows = [r[0] for r in db.execute(
            "SELECT data FROM journal WHERE event='process_finding'")]
        self.assertEqual(len(rows), 1)
        self.assertIn('"rule": "coverage_skipped_no_lane"', rows[0])
        self.assertFalse(any("coverage" in a for c in fake.calls for a in c))
        self.assertEqual(db.execute("SELECT count(*) FROM journal WHERE"
                                    " event='lane_power_requested'").fetchone()[0], 0)
        # With W6 present and a lane on, the night starts W6's CLI -- never cargo itself.
        db.execute("INSERT INTO runners (name, state) VALUES ('lane-a', 'on')")
        _run(db, "run-2", "2026-10-08T12:00:00Z", "GREEN", [("fmt", "GREEN", 5, 0, {})])
        fake = FakeRunner()
        with mock.patch.object(night, "coverage_cli", lambda: REPO / "scripts/pipeline.py"):
            night.report(db, now="2026-10-09T01:00:00Z", out_root=tmp / "nacht", runner=fake,
                         date="2026-10-08")
        cov = [c for c in fake.calls if "coverage" in c]
        self.assertEqual(len(cov), 1)
        # The subprocess inherits the night's store, never the default one (review W11 M3),
        # and names the lane that is on (review Z-I gap I2: without it the build always
        # ended in exit 3 coverage_skipped_no_lane).
        self.assertEqual(cov[0][-6:], ["coverage", "build", "--lane", "lane-a",
                                       "--db", str(night.db_path(db))])
        self.assertFalse(any("cargo" in a for c in fake.calls for a in c))
        text = (tmp / "nacht" / "2026-10-08.md").read_text()
        self.assertIn("built on lane lane-a", text)
        self.assertNotIn("build started", text)

    def test_night_coverage_reports_the_build_exit_truthfully(self):
        """Review Z-I gap I2: the night never reports a build it did not get."""
        for rc, err, want in ((3, "coverage: lane lane-a is not up -- skipped\n",
                               "skipped on lane lane-a (exit 3): coverage: lane lane-a is not up"),
                              (2, "coverage: broken map\n",
                               "failed on lane lane-a (exit 2): coverage: broken map")):
            db, tmp = _store(self)
            db.execute("INSERT INTO runners (name, state) VALUES ('lane-a', 'on')")
            # the owner's machine sorts first but is never the build lane
            db.execute("INSERT INTO runners (name, state, transport) VALUES ('a-local', 'idle', 'local')")
            _run(db, "run-1", DAY + "T12:00:00Z", "GREEN", [("fmt", "GREEN", 5, 0, {})])
            events.emit(db, "merge_done", "status", {"merge": "m-1", "strand": "s", "base": "b"},
                        at=DAY + "T12:30:00Z")

            def fake(argv, rc=rc, err=err, **kw):
                return subprocess.CompletedProcess(argv, rc, "", err)
            with mock.patch.object(night, "coverage_cli", lambda: REPO / "scripts/pipeline.py"):
                path = night.report(db, now=NOW, out_root=tmp / "nacht", runner=fake, date=DAY)
            text = path.read_text()
            self.assertIn(want, text)
            self.assertNotIn("build started", text)

    def test_night_coverage_red_is_finding(self):
        """welle-tb S1 C-Beleg: a RED coverage build (red tests) is a finding, never swallowed."""
        from scripts.pipeline import coverage
        err = ("coverage: map abc1234 is RED: 1 red tests: meclaw-cells gh2::broken"
               " -- impact keeps rdeps\n")
        for build_writes in (False, True):
            db, tmp = _store(self)
            db.execute("INSERT INTO runners (name, state) VALUES ('lane-a', 'on')")
            _run(db, "run-1", DAY + "T12:00:00Z", "GREEN", [("fmt", "GREEN", 5, 0, {})])
            events.emit(db, "merge_done", "status", {"merge": "m-1", "strand": "s", "base": "b"},
                        at=DAY + "T12:30:00Z")

            def fake(argv, db=db, build_writes=build_writes, **kw):
                if build_writes:  # the build journals its own finding into the night's store
                    events.emit(db, "process_finding", "finding",
                                {"rule": "coverage_red", "subject": "abc1234", "value": 1,
                                 "threshold": 0, "since": NOW,
                                 "tests": ["meclaw-cells gh2::broken"]})
                return subprocess.CompletedProcess(argv, coverage.RED_EXIT, "", err)
            with mock.patch.object(night, "coverage_cli", lambda: REPO / "scripts/pipeline.py"):
                path = night.report(db, now=NOW, out_root=tmp / "nacht", runner=fake, date=DAY)
            text = path.read_text()
            self.assertIn("RED on lane lane-a (exit %d)" % coverage.RED_EXIT, text)
            self.assertIn("meclaw-cells gh2::broken", text)
            self.assertNotIn("built on lane", text)
            reds = [f for f in retro.open_findings(db) if f.get("rule") == "coverage_red"]
            self.assertEqual(len(reds), 1, (build_writes, reds))  # one finding, never two
            self.assertIn("coverage_red", text)

    def test_night_coverage_red_older_finding_does_not_hide_this_one(self):
        """re-check H2 m3: an older open coverage_red (another rev) never stands
        in for the RED of this night's build."""
        from scripts.pipeline import coverage
        err = "coverage: map abc1234 is RED: 1 red tests: meclaw-cells gh2::broken\n"
        db, tmp = _store(self)
        db.execute("INSERT INTO runners (name, state) VALUES ('lane-a', 'on')")
        _run(db, "run-1", DAY + "T12:00:00Z", "GREEN", [("fmt", "GREEN", 5, 0, {})])
        events.emit(db, "merge_done", "status", {"merge": "m-1", "strand": "s", "base": "b"},
                    at=DAY + "T12:30:00Z")
        events.emit(db, "process_finding", "finding",
                    {"rule": "coverage_red", "subject": "old0000", "value": 1, "threshold": 0,
                     "since": "2026-10-01T00:00:00Z", "tests": ["x old::red"]})

        def fake(argv, **kw):
            return subprocess.CompletedProcess(argv, coverage.RED_EXIT, "", err)
        with mock.patch.object(night, "coverage_cli", lambda: REPO / "scripts/pipeline.py"):
            night.report(db, now=NOW, out_root=tmp / "nacht", runner=fake, date=DAY)
        rows = [json.loads(r[0]) for r in db.execute(
            "SELECT data FROM journal WHERE event='process_finding'")]
        reds = [r for r in rows if r.get("rule") == "coverage_red"]
        self.assertEqual(2, len(reds), reds)
        self.assertEqual("coverage", reds[-1]["subject"])

    def test_night_coverage_names_the_measured_browser_locks(self):
        """welle-tb S1b point 4: the report says how many browser locks the built map
        measured and why the others are not -- an unmeasured lock is no red."""
        import json
        from scripts.pipeline import browser_cover, coverage
        import gate_plan
        lock = sorted(gate_plan.BROWSER_LOCKS)[0]
        field = {l: {"binary": browser_cover.binary_of(l), "measured": l != lock, "tests": 2}
                 for l in gate_plan.BROWSER_LOCKS}
        field[lock]["why"] = "SKIP webkit is not installed"
        built = json.dumps({"rev": "r1", "runner": "lane-a", "secs": 1.0, "tests": 3,
                            "files": 0, "edges": 0, "granularity": "binary"})
        for rc in (0, coverage.RED_EXIT):
            db, tmp = _store(self)
            maps = tmp / "maps"
            maps.mkdir()
            (maps / "coverage-r1.json").write_text(json.dumps(
                {"rev": "r1", "parts": [], browser_cover.FIELD: field}))
            db.execute("INSERT INTO runners (name, state) VALUES ('lane-a', 'on')")
            _run(db, "run-1", DAY + "T12:00:00Z", "GREEN", [("fmt", "GREEN", 5, 0, {})])
            events.emit(db, "merge_done", "status", {"merge": "m-1", "strand": "s", "base": "b"},
                        at=DAY + "T12:30:00Z")

            def fake(argv, rc=rc, **kw):
                return subprocess.CompletedProcess(argv, rc, "coverage: x\n" + built + "\n",
                                                   "coverage: done\n")
            with mock.patch.object(night, "coverage_cli", lambda: REPO / "scripts/pipeline.py"), \
                    mock.patch.dict(os.environ, {"MECLAW_PIPELINE_COVERAGE_DIR": str(maps)}):
                path = night.report(db, now=NOW, out_root=tmp / "nacht", runner=fake, date=DAY)
            text = path.read_text()
            self.assertIn("browser locks measured 6/7 -- not measured: %s (SKIP webkit is not"
                          " installed)" % lock.rsplit("/", 1)[1].removesuffix(".rs"), text)

    def test_night_report_dir_from_the_newest_workpackage_wave(self):
        db, tmp = _store(self)
        self.assertEqual(night.report_dir(db, repo=tmp), tmp / "nacht")
        store.item_create(db, "workpackage", "a", doc={"wave": "welle-x-2026-10-01"},
                          at="2026-10-07T10:00:00Z")
        store.item_create(db, "workpackage", "b", doc={"wave": "welle-y-2026-10-02"},
                          at="2026-10-07T11:00:00Z")
        self.assertEqual(night.report_dir(db, repo=tmp),
                         tmp / "plans" / "welle-y-2026-10-02" / "nacht")
        store.item_create(db, "workpackage", "c", doc={"wave": "../evil"},
                          at="2026-10-07T12:00:00Z")
        self.assertEqual(night.report_dir(db, repo=tmp), tmp / "nacht")

    def test_night_annotation_skipped_without_environment(self):
        self.assertEqual(night.annotate("2026-10-07", "x", 3, 1), "skipped")


def _meta(*test_names):
    targets = [{"kind": ["lib"], "name": "alpha"}] + [{"kind": ["test"], "name": n}
                                                      for n in test_names]
    return {"workspace_members": ["alpha 0.1.0"],
            "packages": [{"id": "alpha 0.1.0", "name": "alpha", "targets": targets}]}


class CoverageBinariesTest(unittest.TestCase):
    """Review Z-I1 (coordinator): the night rebuilds the coverage map when the
    harness or binary list changed -- it can never go stale in silence again."""

    MAP = {"rev": "r", "parts": [{"binary": "alpha", "module": None, "files": {}},
                                 {"binary": "alpha::a1", "module": None, "files": {}}]}

    def test_same_binaries_are_no_change(self):
        self.assertEqual((False, [], []), night.binaries_changed(self.MAP, _meta("a1")))

    def test_a_folded_harness_is_a_change(self):
        self.assertEqual((True, ["alpha::a1"], ["alpha::misc"]),
                         night.binaries_changed(self.MAP, _meta("misc")))

    def test_without_map_or_metadata_nothing_is_claimed(self):
        self.assertEqual((False, [], []), night.binaries_changed(None, _meta("a1")))
        self.assertEqual((False, [], []), night.binaries_changed(self.MAP, None))

    def test_a_changed_binary_list_makes_the_coverage_due(self):
        db, _tmp = _store(self)
        with mock.patch.object(night, "_map_and_metadata",
                               lambda: (self.MAP, _meta("misc"))):
            due, merges, _since = night.coverage_due(db, FakeRunner())
        self.assertTrue(due)
        self.assertEqual(0, merges)
        with mock.patch.object(night, "_map_and_metadata", lambda: (self.MAP, _meta("a1"))):
            self.assertFalse(night.coverage_due(db, FakeRunner())[0])


class ProposalTest(unittest.TestCase):
    def _build(self, order):
        db, tmp = _store(self)
        _run(db, "run-1", DAY + "T12:00:00Z", "GREEN", [("fmt", "GREEN", 5, 0, {})])
        findings = [("station_drift", "tests", 1.4, 1.2), ("flake_rate_high", "a::b", 0.09, 0.05),
                    ("cache_quota_low", "integration", 0.3, 0.5)]
        for i in order:
            _finding(db, *findings[i])
        path = night.report(db, now=NOW, out_root=tmp / "nacht", runner=FakeRunner(), date=DAY)
        lines = path.read_text().splitlines()
        return db, [ln for ln in lines if ln.startswith("- proposal: ")]

    def test_night_proposals_are_deterministic(self):
        self.assertTrue(RETRO_REAL, "retro.py (part A) is missing; this test needs the real "
                                    "retro.proposal_lines")
        db_a, a = self._build([0, 1, 2])
        db_b, b = self._build([2, 0, 1])
        self.assertEqual(a, b)
        self.assertGreaterEqual(len(a), 3)
        self.assertEqual(a, sorted(a))
        expected = ["- " + ln for ln in retro.proposal_lines(retro.open_findings(db_a))]
        self.assertEqual(a, expected)
        for ln in a:
            self.assertRegex(ln, r"^- proposal: [a-z_]+ -> .+ \(.+\)$")


if __name__ == "__main__":
    unittest.main()
