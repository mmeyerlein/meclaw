"""Tests of `scripts/pipeline/retro.py` (W11): retro numbers and process findings.

Every test builds its own throw-away store and an anonymous fixture journal
(lanes `lane-a`/`lane-b`, repo `meclaw`, made-up revisions); nothing here reads
the default store, `~/.local/state` or a plan folder.
"""

import argparse
import io
import json
import os
import pathlib
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from datetime import datetime, timedelta, timezone
from scripts.tests import store_isolation

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parent.parent
sys.path.insert(0, str(REPO))

from scripts.pipeline import events, metrics, retro, store  # noqa: E402

URL_ENV = "MECLAW_PIPELINE_METRICS_URL"
_SAVED = {}


def setUpModule():
    store_isolation.isolate()
    _SAVED["url"] = os.environ.pop(URL_ENV, None)


def tearDownModule():
    if _SAVED.get("url") is not None:
        os.environ[URL_ENV] = _SAVED["url"]
    store_isolation.restore()


T0 = datetime(2026, 1, 10, 8, 0, 0, tzinfo=timezone.utc)
FINGERPRINT = {k: None for k in events._FINGERPRINT}
FINGERPRINT.update({"rustc": "1.90.0", "cargo": "1.90.0", "kernel": "6.8.0",
                    "vcpu": 16, "ram_g": 64, "target_free_g": 300})


def at(minutes=0, days=0):
    return (T0 + timedelta(days=days, minutes=minutes)).strftime("%Y-%m-%dT%H:%M:%SZ")


class StoreCase(unittest.TestCase):
    """A fresh store per test; the retro hook is off unless a test turns it on."""

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = pathlib.Path(tmp.name)
        self.db = store.open(self.tmp / "p.db")
        self.addCleanup(store.close, self.db)
        self._hooks = list(events.HOOKS)
        events.HOOKS[:] = [h for h in events.HOOKS if h is not retro.hook]
        self.addCleanup(self._restore_hooks)

    def _restore_hooks(self):
        events.HOOKS[:] = self._hooks

    # --- fixture journal ---------------------------------------------------

    def pass_(self, run, mode, start, end, verdict="GREEN", lane=100.0, crit=None,
              stations=None, repo="meclaw", rev="c1", base="c0", cache_hits=0,
              rounds=1, runner="lane-a", strand=None, lock_wait=None, secs=None):
        planned = {"run": run, "mode": mode, "repo": repo, "rev": rev, "base": base,
                   "stations": [], "reason": {}, "cache_known": 0, "dirty": False}
        if strand:
            planned["strand"] = strand
        if lock_wait is not None:
            planned["lock_wait_secs"] = lock_wait
        events.emit(self.db, "pass_planned", "status", planned, run=run, at=start)
        events.emit(self.db, "run_dispatched", "note",
                    {"run": run, "runner": runner, "tree": None, "attempt": 1, "round": 1,
                     "deadline_at": None, "ws": None}, run=run, at=start)
        if secs is None:
            secs = (store.parse_at(end) - store.parse_at(start)).total_seconds()
        return events.emit(self.db, "pass_ended", "status",
                           {"run": run, "verdict": verdict, "secs": secs,
                            "lane_seconds": lane,
                            "critical_path_s": lane if crit is None else crit,
                            "rounds": rounds, "cache_hits": cache_hits,
                            "stations": stations or {"green": 1}},
                           run=run, at=end)

    def station(self, run, name, secs, when, verdict="GREEN"):
        events.emit(self.db, "station_ended", "status",
                    {"run": run, "station": name, "verdict": verdict, "secs": secs,
                     "exit": 0, "scope": "x"}, run=run, at=when)

    def events_of(self, event):
        return [json.loads(r[0]) for r in self.db.execute(
            "SELECT data FROM journal WHERE event=? ORDER BY seq", (event,))]


# --- public API (B and C import these) -----------------------------------------

class RuleTableTests(StoreCase):
    def test_rules_are_data_with_the_fixed_names(self):
        names = [r["rule"] for r in retro.RULES]
        self.assertEqual(names, ["station_drift", "cache_quota_low", "flake_rate_high",
                                 "coverage_stale", "minimum_stale", "lane_reserve_breached",
                                 "stray_target_old", "turnaround_spread",
                                 "coverage_skipped_no_lane"])
        for rule in retro.RULES:
            self.assertEqual(sorted(rule), ["metric", "proposal", "rule", "threshold", "window"])
            self.assertTrue(rule["proposal"].endswith("."))

    def test_proposal_lines_are_sorted_and_deterministic(self):
        findings = [{"rule": "coverage_stale", "subject": "coverage"},
                    {"rule": "cache_quota_low", "subject": "meclaw/integration"}]
        lines = retro.proposal_lines(findings)
        self.assertEqual(lines, retro.proposal_lines(list(reversed(findings))))
        self.assertEqual(lines, sorted(lines))
        self.assertTrue(lines[0].startswith("proposal: cache_quota_low -> "))
        self.assertTrue(lines[0].endswith("(meclaw/integration)"))


# --- retro numbers ---------------------------------------------------------------

class RetroNumberTests(StoreCase):
    def test_retro_lane_minutes_by_day_and_verdict(self):
        self.pass_("r1", "strand", at(0), at(60), lane=3600)
        self.pass_("r2", "strand", at(70), at(100), verdict="RED", lane=1800)
        self.pass_("r3", "integration", at(0, days=1), at(10, days=1), lane=600,
                   stations={"cached": 5}, cache_hits=5)
        self.pass_("r4", "integration", at(20, days=1), at(140, days=1), lane=7200)
        self.pass_("pre1", "pre", at(5, days=1), at(5, days=1), lane=0.0)
        got = retro.lane_minutes_by_day(self.db, at(0), at(0, days=2))
        self.assertEqual(got, {"2026-01-10": {"green": 60.0, "red": 30.0, "cached": 0.0},
                               "2026-01-11": {"green": 120.0, "red": 0.0, "cached": 10.0}})
        q = retro.quarter(self.db, at(0), at(0, days=2))
        self.assertAlmostEqual(q["Q14"], 130 / 60, places=3)
        # a window before the passes has no day and no Q14
        self.assertIsNone(retro.quarter(self.db, at(0, days=-3), at(0))["Q14"])

    def test_retro_q14_threshold_per_lane_not_sum(self):
        """OR-DT.ORCH.15: the 8 h/day limit holds per lane; the sum stays visible."""
        self.pass_("a1", "strand", at(0), at(300), lane=5 * 3600, runner="lane-a")
        self.pass_("b1", "strand", at(0), at(300), lane=5 * 3600, runner="lane-b")
        self.pass_("b2", "strand", at(0, days=1), at(60, days=1), lane=3600, runner="lane-b")
        got = retro.lane_hours_by_lane_and_day(self.db, at(0), at(0, days=2))
        self.assertEqual(got, {"lane-a": {"2026-01-10": 5.0},
                               "lane-b": {"2026-01-10": 5.0, "2026-01-11": 1.0}})
        self.assertEqual(retro.quarter(self.db, at(0), at(0, days=2))["Q14"], 5.0)
        rows = {r["metric"]: r for r in retro.table(self.db, at(0), at(0, days=2))}
        self.assertEqual(rows["lane_hours_peak_day_per_lane"]["id"], "Q14")
        self.assertEqual(rows["lane_hours_peak_day_per_lane"]["value"], 5.0)
        self.assertEqual(rows["lane_hours_peak_day_sum"]["value"], 10.0)
        # one lane above 8 h on one day breaches, two lanes at 5 h each do not
        self.pass_("a2", "strand", at(0, days=1), at(540, days=1), lane=9 * 3600,
                   runner="lane-a")
        self.assertEqual(retro.quarter(self.db, at(0), at(0, days=2))["Q14"], 9.0)

    def test_retro_flake_share_of_red_passes(self):
        # four red passes: one carries a FLAKY isolation, one failed only on a
        # test the flake table knows, two are real reds
        for n in range(4):
            self.pass_(f"r{n}", "strand", at(10 * n), at(10 * n + 5), verdict="RED")
        self.pass_("g1", "strand", at(50), at(55))
        events.emit(self.db, "flake_isolated", "finding",
                    {"run": "r0", "station": "tests", "test": "bin::a", "count": 5,
                     "green": 4, "verdict": "FLAKY"}, run="r0", at=at(3))
        events.emit(self.db, "test_result", "note",
                    {"run": "r1", "station": "tests", "part": "bin", "verdict": "RED",
                     "secs": 3, "failed_names": ["b"]}, run="r1", at=at(13))
        events.emit(self.db, "test_result", "note",
                    {"run": "r2", "station": "tests", "part": "bin", "verdict": "RED",
                     "secs": 3, "failed_names": ["c"]}, run="r2", at=at(23))
        self.db.execute("INSERT INTO flakes (test, station, repo, first_at, last_at, runs,"
                        " reds, flaky, quarantined_until, issue) VALUES"
                        " ('bin::b', 'tests', 'meclaw', ?, ?, 20, 2, 2, NULL, NULL)",
                        (at(0), at(0)))
        value, reason = retro.flake_share(self.db, at(0), at(60))
        self.assertEqual((value, reason), (50.0, None))
        self.assertEqual(retro.quarter(self.db, at(0), at(60))["Q15"], 50.0)

    # Re-check tb-s3-store2 m-3: clause 2 compared raw `failed_names` with
    # register keys -- a nextest name (`t_x`) never met its key (`cells::t_x`),
    # a Python or scenario name did. One rule now for every kind: the
    # register's own `flakes.test_key`; a scenario case counts under its
    # station key `<station>:case:<id>` (M-1), never under another suite's.
    def _red_on(self, run, station, part, names, minute=0):
        self.pass_(run, "strand", at(minute), at(minute + 5), verdict="RED")
        events.emit(self.db, "test_result", "note",
                    {"run": run, "station": station, "part": part, "verdict": "RED",
                     "secs": 1, "failed_names": names}, run=run, at=at(minute + 3))

    def _flaky(self, *tests):
        for t in tests:
            self.db.execute("INSERT INTO flakes (test, station, repo, first_at, last_at, runs,"
                            " reds, flaky, quarantined_until, issue) VALUES"
                            " (?, 'tests', 'meclaw', ?, ?, 20, 2, 2, NULL, NULL)",
                            (t, at(0), at(0)))

    def test_flake_share_matches_a_nextest_name_by_its_register_key(self):
        self._flaky("cells::t_x")
        self._red_on("r0", "tests", "cells", ["t_x"])
        self.assertEqual(retro.flake_share(self.db, at(0), at(60)), (100.0, None))

    def test_flake_share_matches_a_unittest_line_by_its_register_key(self):
        self._flaky("pkg.mod.Case.test_a")
        self._red_on("r0", "python", "pkg.mod", ["test_a (pkg.mod.Case)"])
        self.assertEqual(retro.flake_share(self.db, at(0), at(60)), (100.0, None))

    def test_flake_share_matches_a_scenario_case_by_its_station_key(self):
        self._flaky("scenarios:memory:case:L1")
        self._red_on("r0", "scenarios:memory", "case:L1", ["case:L1"])
        self.assertEqual(retro.flake_share(self.db, at(0), at(60)), (100.0, None))

    def test_flake_share_never_crosses_suites_on_a_station_key(self):
        self._flaky("scenarios:memory:case:L1")
        self._red_on("r0", "scenarios:builder", "case:L1", ["case:L1"], minute=0)
        self._red_on("r1", "scenarios:builder", "scenarios:builder:case:L1",
                     ["scenarios:builder:case:L1"], minute=10)
        self._red_on("r2", "scenarios:memory", "scenarios:memory:case:L1",
                     ["scenarios:memory:case:L1"], minute=20)
        self.assertEqual(retro.flake_share(self.db, at(0), at(60)), (33.3, None))

    def test_retro_flake_share_is_na_without_flake_data(self):
        self.pass_("r0", "strand", at(0), at(5), verdict="RED")
        value, reason = retro.flake_share(self.db, at(0), at(60))
        self.assertIsNone(value)
        self.assertIn("flake", reason)

    def test_retro_cache_quota_by_mode(self):
        self.pass_("i1", "integration", at(0), at(10), stations={"green": 6, "cached": 4},
                   cache_hits=4)
        self.pass_("i2", "integration", at(20), at(30), stations={"green": 2, "cached": 8},
                   cache_hits=8)
        self.pass_("s1", "strand", at(40), at(50), stations={"green": 9, "red": 1})
        got = retro.cache_quota_by_mode(self.db, at(0), at(60))
        self.assertEqual(got, {"integration": 60.0, "strand": 0.0})
        self.assertEqual(retro.quarter(self.db, at(0), at(60))["Q16"], 60.0)

    def test_retro_parallelism_ratio(self):
        self.pass_("a", "strand", at(0), at(10), lane=1000, crit=300)
        self.pass_("b", "strand", at(20), at(30), lane=1000, crit=500)
        self.pass_("c", "integration", at(40), at(50), lane=400, crit=400)
        self.pass_("p", "pre", at(55), at(55), lane=0.0, crit=0.01)
        self.assertAlmostEqual(retro.parallelism(self.db, at(0), at(60)), 0.5)
        self.assertAlmostEqual(retro.quarter(self.db, at(0), at(60))["Q17"], 0.5)

    def _turnaround_wave(self):
        totals = []
        for n, gate_min in enumerate((10, 15, 20, 25, 100)):
            base = 200 * n
            self.pass_(f"pre{n}", "pre", at(base), at(base), lane=0.0, rev=f"h{n}",
                       secs=2.0)
            self.pass_(f"s{n}", "strand", at(base + 5), at(base + 5 + gate_min),
                       rev=f"c{n}", base=f"h{n}", strand=f"st{n}")
            totals.append((5 + gate_min) * 60)
        events.emit(self.db, "merge_done", "status",
                    {"merge": "m0", "strand": "st0", "base": "h0", "merge_sha": "z0"},
                    at=at(20))
        return totals

    def test_retro_turnaround_p50_p90(self):
        self._turnaround_wave()
        rows = retro.turnarounds(self.db, at(0), at(0, days=1))
        self.assertEqual(len(rows), 5)
        first = rows[0]
        self.assertEqual((first["pre_s"], first["gate_s"], first["merge_s"]),
                         (2.0, 600.0, 300.0))
        self.assertEqual(first["total_s"], 1200.0)
        p50, p90 = retro.turnaround_p50_p90(self.db, at(0), at(0, days=1))
        self.assertEqual((p50, p90), (1500.0, 6300.0))
        table = {r["metric"]: r for r in retro.table(self.db, at(0), at(0, days=1))}
        self.assertEqual(table["turnaround_p50_p90_s"]["value"], [1500.0, 6300.0])

    def test_turnaround_spread_rule(self):
        self._turnaround_wave()
        seqs = retro.evaluate(self.db, now=at(0, days=1))
        found = [f for f in retro.open_findings(self.db) if f["rule"] == "turnaround_spread"]
        self.assertEqual(len(found), 1, seqs)
        self.assertEqual(found[0]["value"], 6300.0)


# --- finding rules ---------------------------------------------------------------

class _CountingDb:
    """A store connection that counts the journal rows a rule pulls."""

    def __init__(self, db):
        self.db, self.rows = db, 0

    def execute(self, sql, args=()):
        cur = self.db.execute(sql, args)
        outer = self

        class _Cur:
            def __iter__(self):
                for row in cur:
                    outer.rows += 1
                    yield row

            def fetchone(self):
                row = cur.fetchone()
                outer.rows += row is not None
                return row

            def fetchall(self):
                return list(iter(self))
        return _Cur()

class FindingRuleTests(StoreCase):
    def _station_series(self, recent_secs):
        for n in range(25):
            run = f"r{n:02d}"
            secs = 100.0 if n < 20 else recent_secs
            self.pass_(run, "strand", at(10 * n), at(10 * n + 5))
            self.station(run, "tests", secs, at(10 * n + 4))

    def test_finding_rule_station_drift_20_percent(self):
        self._station_series(125.0)
        seqs = retro.evaluate(self.db, now=at(0, days=1))
        found = retro.open_findings(self.db)
        self.assertEqual([(f["rule"], f["subject"]) for f in found],
                         [("station_drift", "meclaw/strand/tests")])
        self.assertEqual(found[0]["value"], 25.0)
        self.assertEqual(found[0]["threshold"], 20)
        self.assertEqual(found[0]["since"], at(204))
        self.assertEqual(len(seqs), 1)
        # an open finding is not written twice
        self.assertEqual(retro.evaluate(self.db, now=at(0, days=1)), [])

    def test_station_drift_below_20_percent_is_no_finding(self):
        self._station_series(119.0)
        retro.evaluate(self.db, now=at(0, days=1))
        self.assertEqual(retro.open_findings(self.db), [])

    def test_a_run_scoped_evaluation_judges_only_the_series_the_run_touched(self):
        """#1136: the hook evaluates per ended run; an untouched series that
        drifted is not judged again by an unrelated pass."""
        self._station_series(125.0)
        for n in range(25):   # a second drifting series, in its own runs
            run = f"c{n:02d}"
            self.pass_(run, "strand", at(10 * n), at(10 * n + 5))
            self.station(run, "clippy", 100.0 if n < 20 else 150.0, at(10 * n + 4))
        retro.evaluate(self.db, now=at(0, days=1), run="r24")
        self.assertEqual([f["subject"] for f in retro.open_findings(self.db)],
                         ["meclaw/strand/tests"])
        retro.evaluate(self.db, now=at(0, days=1), run="c24")
        self.assertEqual(sorted(f["subject"] for f in retro.open_findings(self.db)),
                         ["meclaw/strand/clippy", "meclaw/strand/tests"])
        self.assertEqual(retro.open_findings(self.db)[1]["value"], 50.0)

    def test_a_run_scoped_drift_reads_a_bounded_tail_of_the_journal(self):
        """#1136: 300 older green runs of the series cost nothing extra."""
        for n in range(300):
            run = f"o{n:03d}"
            self.pass_(run, "strand", at(n), at(n))
            self.station(run, "tests", 100.0, at(n))
        self._station_series(125.0)
        counting = _CountingDb(self.db)
        retro._rule_station_drift(counting, at(0, days=1), run="r24")
        self.assertLess(counting.rows, 200, counting.rows)
        self.assertEqual([h["subject"] for h in
                          retro._rule_station_drift(self.db, at(0, days=1), run="r24")],
                         ["meclaw/strand/tests"])

    def test_a_run_scoped_cache_quota_looks_at_integration_runs_only(self):
        store.cache_put(self.db, "k1", "tests", "", "GREEN", "i0", "lane-a", [], at=at(0))
        self._integration(3, cached=4, green=6)
        self.pass_("s1", "strand", at(70), at(75))
        self.assertEqual(retro._rule_cache_quota_low(self.db, at(80), run="s1"), [])
        self.assertEqual([(h["subject"], h["value"]) for h in
                          retro._rule_cache_quota_low(self.db, at(80), run="i2")],
                         [("meclaw/integration", 40.0)])

    def _integration(self, n, cached, green):
        for k in range(n):
            self.pass_(f"i{k}", "integration", at(10 * k), at(10 * k + 5),
                       stations={"green": green, "cached": cached}, cache_hits=cached)

    def test_finding_rule_cache_quota_below_50(self):
        store.cache_put(self.db, "k1", "tests", "", "GREEN", "i0", "lane-a", [], at=at(0))
        self._integration(3, cached=4, green=6)
        retro.evaluate(self.db, now=at(60))
        found = retro.open_findings(self.db)
        self.assertEqual([(f["rule"], f["subject"], f["value"]) for f in found],
                         [("cache_quota_low", "meclaw/integration", 40.0)])

    def test_cache_quota_needs_three_runs_and_a_cache(self):
        self._integration(2, cached=0, green=10)
        store.cache_put(self.db, "k1", "tests", "", "GREEN", "i0", "lane-a", [], at=at(0))
        retro.evaluate(self.db, now=at(60))  # two runs only: no finding
        self.assertEqual(retro.open_findings(self.db), [])
        self.pass_("i9", "integration", at(40), at(45), stations={"green": 10})
        retro.evaluate(self.db, now=at(60))
        self.assertEqual(len(retro.open_findings(self.db)), 1)

    def test_cache_quota_is_silent_while_no_cache_exists(self):
        self._integration(3, cached=0, green=10)
        retro.evaluate(self.db, now=at(60))  # no cache rows yet: no data, no finding
        self.assertEqual(retro.open_findings(self.db), [])

    def test_finding_rule_coverage_stale(self):
        cov = {"rev": "c0", "runner": "lane-a", "secs": 60, "tests": 10, "files": 5,
               "edges": 20, "granularity": "module"}
        events.emit(self.db, "coverage_built", "status", cov, at=at(0))
        retro.evaluate(self.db, now=at(0, days=6))
        self.assertEqual(retro.open_findings(self.db), [])
        retro.evaluate(self.db, now=at(0, days=8))
        found = retro.open_findings(self.db)
        self.assertEqual([(f["rule"], f["subject"], f["value"], f["since"]) for f in found],
                         [("coverage_stale", "coverage", 8.0, at(0))])

    def test_evaluate_takes_an_aware_datetime(self):
        """night.py passes `now` as a datetime in local time."""
        cov = {"rev": "c0", "runner": "lane-a", "secs": 60, "tests": 10, "files": 5,
               "edges": 20, "granularity": "module"}
        events.emit(self.db, "coverage_built", "status", cov, at=at(0))
        local = timezone(timedelta(hours=2))
        retro.evaluate(self.db, now=(T0 + timedelta(days=8)).astimezone(local))
        (found,) = retro.open_findings(self.db)
        self.assertEqual((found["value"], found["at"]), (8.0, at(0, days=8)))

    def _fingerprint(self, run, when, **changes):
        events.emit(self.db, "run_dispatched", "note",
                    {"run": run, "runner": "lane-a", "tree": None, "attempt": 1,
                     "round": 1, "deadline_at": None, "ws": None}, run=run, at=when)
        fp = dict(FINGERPRINT, **changes)
        events.emit(self.db, "lane_fingerprint", "note", fp, run=run, at=when)

    def test_finding_rule_minimum_stale_on_fingerprint_change(self):
        self._fingerprint("f1", at(0))
        events.emit(self.db, "minimum_measured", "status",
                    {"stage": "strand", "runner": "lane-a", "seconds": 300,
                     "critical_path": [], "overhead_s": 10, "x": 1.2}, at=at(10))
        self._fingerprint("f2", at(20), target_free_g=120)  # volatile: no finding
        retro.evaluate(self.db, now=at(30))
        self.assertEqual(retro.open_findings(self.db), [])
        self._fingerprint("f3", at(40), rustc="1.91.0")
        retro.evaluate(self.db, now=at(50))
        found = retro.open_findings(self.db)
        self.assertEqual([(f["rule"], f["subject"], f["since"]) for f in found],
                         [("minimum_stale", "strand@lane-a", at(40))])

    def test_lane_reserve_and_stray_target_rules(self):
        for n in range(2):
            events.emit(self.db, "watchdog_fired", "finding",
                        {"run": f"w{n}", "runner": "lane-b", "kind": "disk", "value": 40,
                         "threshold": 60, "action": "aborted"}, run=f"w{n}", at=at(60 * n))
        store.workspace_set(self.db, "ws-1", "lane-a", "target", "/x/target", "closed",
                            at=at(0))
        retro.evaluate(self.db, now=at(0, days=2))
        rules = sorted((f["rule"], f["subject"]) for f in retro.open_findings(self.db))
        self.assertEqual(rules, [("stray_target_old", "lane-a:ws-1")])
        retro.evaluate(self.db, now=at(120))
        rules = sorted((f["rule"], f["subject"]) for f in retro.open_findings(self.db))
        self.assertIn(("lane_reserve_breached", "lane-b"), rules)

    def test_findings_never_block_exit_zero(self):
        self._station_series(150.0)
        broken = dict(retro.RULE_FUNCS)

        def boom(db, now):
            raise RuntimeError("broken rule")

        retro.RULE_FUNCS["coverage_stale"] = boom
        self.addCleanup(retro.RULE_FUNCS.update, broken)
        seqs = retro.evaluate(self.db, now=at(0, days=1))
        self.assertEqual(len(seqs), 1)
        # the hook swallows everything, also a closed connection
        retro.hook(self.db, {"event": "pass_ended", "run": "r00", "at": at(0),
                             "data": {}, "seq": 1})
        dead = store.open(self.tmp / "dead.db")
        dead.close()
        retro.hook(dead, {"event": "pass_ended", "run": "x", "at": at(0), "data": {},
                          "seq": 1})
        self.assertEqual(retro.evaluate(dead), [])
        # `pipeline retro` exits 0 although findings are open
        p = argparse.ArgumentParser()
        sub = p.add_subparsers(dest="cmd", required=True)
        retro.register(sub)
        out = self.tmp / "r.json"
        args = p.parse_args(["retro", "--since", at(0), "--until", at(0, days=1),
                             "--db", str(self.tmp / "p.db"), "--json", str(out)])
        with redirect_stdout(io.StringIO()):
            self.assertEqual(args.func(args), 0)
        data = json.loads(out.read_text())
        self.assertEqual(len(data["open_findings"]), 1)

    def test_finding_resolve_writes_follow_up_row(self):
        self._station_series(150.0)
        retro.evaluate(self.db, now=at(0, days=1))
        (finding,) = retro.open_findings(self.db)
        before = self.db.execute("SELECT count(*) FROM journal").fetchone()[0]
        seq = retro.resolve(self.db, finding["seq"], "station split")
        self.assertGreater(seq, finding["seq"])
        self.assertEqual(self.db.execute("SELECT count(*) FROM journal").fetchone()[0],
                         before + 1)
        (row,) = self.events_of("finding_resolved")
        self.assertEqual(row, {"seq": finding["seq"], "rule": "station_drift",
                               "subject": "meclaw/strand/tests", "note": "station split"})
        self.assertEqual(retro.open_findings(self.db), [])
        # the finding row itself is untouched (append-only)
        self.assertEqual(len(self.events_of("process_finding")), 1)
        with self.assertRaises(ValueError):
            retro.resolve(self.db, finding["seq"], "again")
        with self.assertRaises(ValueError):
            retro.resolve(self.db, 99999, "unknown")
        # the CLI writes the same row and exits 0
        retro.evaluate(self.db, now=at(0, days=1))
        (again,) = retro.open_findings(self.db)
        p = argparse.ArgumentParser()
        sub = p.add_subparsers(dest="cmd", required=True)
        retro.register(sub)
        args = p.parse_args(["finding", "resolve", str(again["seq"]), "--note", "done",
                             "--db", str(self.tmp / "p.db")])
        with redirect_stdout(io.StringIO()):
            self.assertEqual(args.func(args), 0)
        self.assertEqual(retro.open_findings(self.db), [])


# --- hook, window, metrics -------------------------------------------------------

class HookTests(StoreCase):
    def test_hook_evaluates_after_pass_ended_and_pushes_finding_lines(self):
        sent = []
        orig = metrics.post
        metrics.post = lambda target, lines: sent.extend(lines)
        self.addCleanup(setattr, metrics, "post", orig)
        os.environ[URL_ENV] = "http://metrics.invalid/write?db=devtempo"
        self.addCleanup(os.environ.pop, URL_ENV, None)
        events.HOOKS.append(retro.hook)
        for n in range(25):
            run = f"r{n:02d}"
            self.station(run, "tests", 100.0 if n < 20 else 150.0, at(10 * n + 4))
            self.pass_(run, "strand", at(10 * n), at(10 * n + 5))
        # the hook evaluated after the 25th pass_ended and found the drift
        self.assertEqual(len(self.events_of("process_finding")), 1)
        calls = []
        orig_eval = retro.evaluate
        retro.evaluate = lambda db, now=None, run=None: calls.append(run) or []
        self.addCleanup(setattr, retro, "evaluate", orig_eval)
        self.pass_("r99", "strand", at(400), at(405))
        retro.evaluate = orig_eval
        self.assertEqual(calls, ["r99"])   # #1136: the hook scopes to the ended run
        finding = retro.open_findings(self.db)[0]
        retro.resolve(self.db, finding["seq"], "ok")
        opened = [l for l in sent if l.startswith("pipe_finding,") and " open=1" in l]
        closed = [l for l in sent if l.startswith("pipe_finding,") and " open=0" in l]
        self.assertEqual(len(opened), 1)
        self.assertEqual(len(closed), 1)
        self.assertIn("rule=station_drift", opened[0])

    def test_hook_pushes_turnaround_after_the_strand_gate(self):
        sent = []
        orig = metrics.post
        metrics.post = lambda target, lines: sent.extend(lines)
        self.addCleanup(setattr, metrics, "post", orig)
        os.environ[URL_ENV] = "http://metrics.invalid/write?db=devtempo"
        self.addCleanup(os.environ.pop, URL_ENV, None)
        events.HOOKS.append(retro.hook)
        self.pass_("pre0", "pre", at(0), at(0), lane=0.0, rev="h0", secs=2.0)
        self.pass_("s0", "strand", at(5), at(15), rev="c0", base="h0", strand="st0")
        lines = [l for l in sent if l.startswith("pipe_turnaround,")]
        self.assertEqual(len(lines), 1)
        self.assertIn("strand=st0", lines[0])
        self.assertIn("total_s=900", lines[0])

    def test_metric_failure_is_an_event_not_a_crash(self):
        def fail(target, lines):
            raise OSError("down")
        orig = metrics.post
        metrics.post = fail
        self.addCleanup(setattr, metrics, "post", orig)
        os.environ[URL_ENV] = "http://user:secret@metrics.invalid/write"
        self.addCleanup(os.environ.pop, URL_ENV, None)
        events.HOOKS.append(retro.hook)
        cov = {"rev": "c0", "runner": "lane-a", "secs": 60, "tests": 10, "files": 5,
               "edges": 20, "granularity": "module"}
        events.emit(self.db, "coverage_built", "status", cov, at=at(0))
        retro.evaluate(self.db, now=at(0, days=9))
        failed = self.events_of("metrics_failed")
        self.assertTrue(failed)
        self.assertNotIn("secret", json.dumps(failed))

    def test_wave_window_starts_at_the_date_of_the_folder(self):
        since, until = retro.wave_window("wave-x-2026-01-10", self.db)
        self.assertEqual(since, "2026-01-10T00:00:00Z")
        self.assertGreater(until, since)
        with self.assertRaises(ValueError):
            retro.wave_window("wave-without-date", self.db)

    def test_retro_cli_writes_json_and_markdown_with_before_and_after(self):
        self.pass_("old", "strand", at(0, days=-2), at(30, days=-2), lane=1800)
        self.pass_("new", "strand", at(0), at(60), lane=3600, crit=1800)
        p = argparse.ArgumentParser()
        sub = p.add_subparsers(dest="cmd", required=True)
        retro.register(sub)
        js, md = self.tmp / "d.json", self.tmp / "r.md"
        args = p.parse_args(["retro", "wave-x-2026-01-10", "--until", at(0, days=1),
                             "--db", str(self.tmp / "p.db"), "--json", str(js),
                             "--md", str(md)])
        with redirect_stdout(io.StringIO()):
            self.assertEqual(args.func(args), 0)
        data = json.loads(js.read_text())
        self.assertEqual(data["window"]["since"], "2026-01-10T00:00:00Z")
        self.assertEqual(data["quarter"]["after"]["Q14"], 1.0)
        self.assertEqual(data["quarter"]["before"]["Q14"], 0.5)
        text = md.read_text()
        self.assertLessEqual(len(text.splitlines()), 40)
        self.assertIn("| Q14 ", text)
        # the read command never writes into the store
        self.assertEqual(self.events_of("process_finding"), [])


if __name__ == "__main__":
    unittest.main()
