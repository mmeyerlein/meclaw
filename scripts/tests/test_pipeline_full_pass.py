"""A full pass after odd effects (owner's ruling 10.10., option 2; welle-tb #1165).

A narrowed pass -- a strand on the coverage map, a merge after
`narrow.merge_decision`, an integration after `narrow.integration_decision`
-- that shows an unexplained red, a FLAKY station, a coverage map that does
not fit or a cache doubt marks its wave in the store's journal. The next pass
of that wave plans full (an integration the full suite, a strand or merge
without narrowing) until a GREEN full pass clears the mark; `--full` plans a
full pass by hand.
"""

import json
import os
import pathlib
import sqlite3
import sys
import tempfile
import unittest
from unittest import mock

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS))
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import gate_plan  # noqa: E402
from pipeline import events as plain_events  # noqa: E402
from pipeline import full_pass as plain_full  # noqa: E402
from scripts.pipeline import cache, full_pass, gate, store  # noqa: E402

# every tool a cache key carries, with a version (re-check tv-bn M7r A-M1: a
# fingerprint without one is no proof, `gate._cache_doubt`)
TOOLS = {k: "x" for k in cache.KEY_TOOLS}
from scripts.tests import store_isolation  # noqa: E402
from scripts.tests.test_pipeline_gate_cli import GateCase, git as cli_git, station  # noqa: E402
from test_pipeline_narrow import StageBase, git  # noqa: E402
from scripts.pipeline import merge  # noqa: E402
from scripts.tests.test_pipeline_merge import MergeCase  # noqa: E402

_METRICS_URL = mock.patch.dict(os.environ, {"MECLAW_PIPELINE_METRICS_URL": ""})


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()
    _METRICS_URL.start()


def tearDownModule():  # noqa: N802 -- unittest's name
    _METRICS_URL.stop()
    store_isolation.restore()


ON_MAP = "narrowing: on, map abcdef012 -- 1 of 4 binaries"
NARROWED = {"mode": "strand", "notes": [ON_MAP]}
GREEN = [{"name": "claims", "verdict": "GREEN"}, {"name": "tests", "verdict": "GREEN"}]
# The map_fits / narrow texts of narrow.py, as a narrowed pass prints them.
MISFITS = (
    "narrowing: coverage map abcdef012 names 1 binaries HEAD does not have (alpha::gone) "
    "-- floor + graph, rdeps kept",
    "narrowing: coverage map abcdef012 is not in this repository -- floor + graph, rdeps kept",
    "narrowing: tests moved since coverage map abcdef012 (D crates/a/tests/x.rs) -- floor + "
    "graph, rdeps kept",
    "narrowing: stage:merge narrowed parent=abc strand-run=tr-1 -- the map does not fit, the "
    "merge stays unnarrowed",
)


class StoreCase(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.db = store.open(pathlib.Path(tmp.name) / "pipeline.db")
        self.addCleanup(self.db.close)

    def after(self, doc, stations, verdict="GREEN", doubt=None, wave="welle-x", run="tr-1",
              strand=None):
        kw = {"strand": strand} if strand else {}
        return full_pass.after_pass(self.db, wave, doc, stations, verdict, run, doubt, **kw)

    def why(self, wave="welle-x"):
        mark = full_pass.pending(self.db, wave)
        return mark and mark["why"]


class TriggerTest(StoreCase):
    def test_unexplained_red_marks_the_wave(self):
        red = [{"name": "tests", "verdict": "RED", "reason": "unexplained red: exit 101"}]
        lines = self.after(NARROWED, red, "RED")
        self.assertIn("unexplained red", self.why() or "", lines)
        self.assertIn("tests", self.why())
        self.assertTrue(any("next pass of welle-x plans full" in n for n in lines), lines)

    def test_an_explained_red_marks_nothing(self):
        red = [{"name": "tests", "verdict": "RED", "reason": "2 tests failed"}]
        self.assertEqual([], self.after(NARROWED, red, "RED"))
        self.assertIsNone(full_pass.pending(self.db, "welle-x"))

    def test_flaky_marks_the_wave(self):
        flaky = [{"name": "claims", "verdict": "GREEN"},
                 {"name": "tests", "verdict": "FLAKY", "reason": "issue pending"}]
        self.after(NARROWED, flaky, "GREEN")
        self.assertIn("FLAKY", self.why() or "")
        self.assertIn("tests", self.why())

    def test_a_coverage_map_that_does_not_fit_marks_the_wave(self):
        for i, note in enumerate(MISFITS):
            wave = "welle-m%d" % i
            self.after({"mode": "strand", "notes": [note]}, GREEN, wave=wave)
            self.assertIn("map mismatch", self.why(wave) or "", note)

    def test_a_cache_doubt_marks_the_wave_when_the_pass_served_cached(self):
        cached = [{"name": "claims", "verdict": "CACHED"}, {"name": "tests", "verdict": "GREEN"}]
        self.after(NARROWED, cached, doubt="tools changed: rustc")
        self.assertIn("cache doubt", self.why() or "")
        self.assertIn("rustc", self.why())
        # no CACHED station: nothing of the verdict came from the doubted keys
        self.after(NARROWED, GREEN, doubt="tools changed: rustc", wave="welle-y")
        self.assertIsNone(full_pass.pending(self.db, "welle-y"))

    def test_a_quiet_narrowed_pass_marks_nothing(self):
        self.assertEqual([], self.after(NARROWED, GREEN))
        self.assertIsNone(full_pass.pending(self.db, "welle-x"))

    def test_an_unnarrowed_pass_marks_nothing(self):
        flaky = [{"name": "tests", "verdict": "FLAKY"}]
        for notes in ([], ["narrowing: off (MECLAW_GATE_NARROW=0) -- resolver floor"],
                      ["narrowing: merge stays unnarrowed -- no strand run"]):
            self.assertEqual([], self.after({"mode": "strand", "notes": notes}, flaky))
        self.assertIsNone(full_pass.pending(self.db, "welle-x"))

    def test_no_wave_no_mark_and_the_note_says_so(self):
        flaky = [{"name": "tests", "verdict": "FLAKY"}]
        lines = self.after(NARROWED, flaky, wave=None)
        self.assertTrue(any("no wave" in n for n in lines), lines)
        self.assertEqual(0, self.db.execute(
            "SELECT count(*) FROM journal WHERE event LIKE 'full_pass%'").fetchone()[0])


class ClearTest(StoreCase):
    def setUp(self):
        super().setUp()
        full_pass.mark(self.db, "welle-x", "FLAKY tests", "tr-0")

    def test_a_green_full_pass_clears_the_mark_by_an_event(self):
        doc = {"mode": "strand", "full_pass": full_pass.pending(self.db, "welle-x")}
        lines = self.after(doc, GREEN, "GREEN", run="tr-2")
        self.assertIsNone(full_pass.pending(self.db, "welle-x"), lines)
        events = [r[0] for r in self.db.execute(
            "SELECT event FROM journal WHERE event LIKE 'full_pass%' ORDER BY seq")]
        self.assertEqual(["full_pass_marked", "full_pass_cleared"], events)  # nothing deleted

    def test_a_red_full_pass_leaves_the_mark(self):
        doc = {"mode": "strand", "full_pass": full_pass.pending(self.db, "welle-x")}
        self.after(doc, [{"name": "tests", "verdict": "RED", "reason": "unexplained red"}], "RED")
        self.assertEqual("FLAKY tests", self.why())

    def test_an_integration_on_the_full_suite_and_a_release_clear(self):
        full = {"mode": "integration", "stations": [
            {"name": "tests", "select": {"all": True, "filter": "all()"}}]}
        self.after(full, GREEN)
        self.assertIsNone(full_pass.pending(self.db, "welle-x"))
        full_pass.mark(self.db, "welle-x", "FLAKY tests", "tr-3")
        self.after({"mode": "release"}, GREEN)
        self.assertIsNone(full_pass.pending(self.db, "welle-x"))

    def test_a_narrowed_green_pass_leaves_the_mark(self):
        self.after(NARROWED, GREEN)
        self.assertEqual("FLAKY tests", self.why())


class ScopeTest(StoreCase):
    """Re-check S1b (M1): a mark is cleared only by a GREEN full pass at least
    as wide as the pass that set it. A strand or merge planned full runs the
    resolver floor of ITS diff, never the integration's suite."""

    INTEGRATION = {"mode": "integration", "notes": [ON_MAP],
                   "stations": [{"name": "tests", "select": {"all": False}}]}

    def test_a_green_strand_pass_never_clears_the_mark_of_an_integration(self):
        lines = self.after(self.INTEGRATION, [{"name": "tests", "verdict": "FLAKY"}])
        self.assertEqual("FLAKY tests", self.why(), lines)
        for doc in ({"mode": "strand", "full_pass": full_pass.pending(self.db, "welle-x")},
                    {"mode": "strand", "stage": "merge",
                     "full_pass": full_pass.pending(self.db, "welle-x")}):
            lines = self.after(doc, GREEN, "GREEN", run="tr-2")
            self.assertEqual("FLAKY tests", self.why(), lines)   # the mark stands
            self.assertTrue(any("stands" in n for n in lines), lines)
        # every pass of the wave still plans full while it stands
        self.assertEqual("FLAKY tests", full_pass.for_plan(False, "welle-x", self.db)["why"])
        full = {"mode": "integration", "full_pass": full_pass.pending(self.db, "welle-x")}
        self.after(full, GREEN, "GREEN", run="tr-3")
        self.assertIsNone(full_pass.pending(self.db, "welle-x"))

    def test_a_green_strand_pass_clears_a_strand_mark_but_not_an_older_integration_mark(self):
        self.after(self.INTEGRATION, [{"name": "tests", "verdict": "FLAKY"}])
        self.after(NARROWED, [{"name": "tests", "verdict": "RED",
                               "reason": "unexplained red: exit 101"}], "RED")
        self.assertEqual("unexplained red in tests", self.why())
        self.after({"mode": "strand", "full_pass": {"why": "x"}}, GREEN, "GREEN", run="tr-4")
        self.assertEqual("FLAKY tests", self.why())     # the integration's mark is back on top
        self.after({"mode": "release"}, GREEN, "GREEN", run="tr-5")
        self.assertIsNone(full_pass.pending(self.db, "welle-x"))

    def test_the_mark_is_per_wave_and_the_newest_event_wins(self):
        self.assertIsNone(full_pass.pending(self.db, "welle-other"))
        full_pass.clear(self.db, "welle-x", "tr-4")
        full_pass.mark(self.db, "welle-x", "map mismatch: x", "tr-5")
        self.assertEqual("map mismatch: x", self.why())
        self.assertEqual("tr-5", full_pass.pending(self.db, "welle-x")["run"])


class StrandMarkTest(StoreCase):
    """Re-check S1b m-4: a strand's mark binds that strand and every
    integration, never the other strands of the wave; a GREEN full pass of
    strand B never clears the mark of strand A."""

    FLAKY = [{"name": "tests", "verdict": "FLAKY"}]

    def test_a_mark_of_strand_a_binds_a_and_the_integration_not_b(self):
        lines = self.after(NARROWED, self.FLAKY, strand="a")
        self.assertEqual("FLAKY tests", full_pass.pending(self.db, "welle-x", "a")["why"], lines)
        self.assertEqual("a", full_pass.pending(self.db, "welle-x", "a")["strand"])
        self.assertIsNone(full_pass.pending(self.db, "welle-x", "b"))
        self.assertIsNone(full_pass.for_plan(False, "welle-x", self.db, strand="b"))
        self.assertEqual("FLAKY tests", self.why())      # an integration sees every mark
        self.assertEqual("FLAKY tests", full_pass.for_plan(False, "welle-x", self.db)["why"])

    def test_a_green_full_pass_of_strand_b_never_clears_the_mark_of_strand_a(self):
        self.after(NARROWED, self.FLAKY, strand="a")
        self.after(NARROWED, [{"name": "tests", "verdict": "RED",
                               "reason": "unexplained red: exit 101"}], "RED", run="tr-2",
                   strand="b")
        b = full_pass.pending(self.db, "welle-x", "b")
        lines = self.after({"mode": "strand", "full_pass": b}, GREEN, "GREEN", run="tr-3",
                           strand="b")
        self.assertIn("is cleared", " ".join(lines))
        self.assertIsNone(full_pass.pending(self.db, "welle-x", "b"))
        self.assertEqual("FLAKY tests", full_pass.pending(self.db, "welle-x", "a")["why"])
        self.assertEqual("FLAKY tests", self.why())
        a = full_pass.pending(self.db, "welle-x", "a")
        self.after({"mode": "strand", "full_pass": a}, GREEN, "GREEN", run="tr-4", strand="a")
        self.assertIsNone(self.why())

    def test_a_pass_without_a_strand_name_sees_every_mark_and_clears_only_its_own(self):
        """Fail-closed: a merge gate runs detached, without a strand name."""
        self.after(NARROWED, self.FLAKY, strand="a")
        mark = full_pass.pending(self.db, "welle-x")
        lines = self.after({"mode": "strand", "stage": "merge", "full_pass": mark}, GREEN,
                           "GREEN", run="tr-2")
        self.assertTrue(any("stands" in n and "strand a" in n for n in lines), lines)
        self.assertEqual("FLAKY tests", full_pass.pending(self.db, "welle-x", "a")["why"])

    def test_a_mark_without_a_strand_name_binds_every_strand(self):
        """A mark without a `strand` field (before m-4, or a pass without a
        name) binds the whole wave, and any strand's GREEN full pass clears it."""
        full_pass.mark(self.db, "welle-x", "FLAKY tests", "tr-0")
        self.assertEqual("FLAKY tests", full_pass.pending(self.db, "welle-x", "b")["why"])
        self.assertEqual("FLAKY tests", full_pass.pending(self.db, "welle-x", "a")["why"])
        lines = self.after({"mode": "strand", "full_pass": {"why": "x"}}, GREEN, "GREEN",
                           run="tr-1", strand="b")
        self.assertIsNone(self.why(), lines)

    def test_the_strand_of_a_lease_holder(self):
        self.assertEqual("tb-s1b", full_pass.strand_of("welle-tempo-beweis/tb-s1b"))
        self.assertEqual("s", full_pass.strand_of("welle-b/s:2"))
        self.assertIsNone(full_pass.strand_of(""))
        self.assertIsNone(full_pass.strand_of("t1"))


class NoStoreTest(unittest.TestCase):
    """Re-check S1b m-5: a missing store holds no mark; a store that cannot be
    read may hold one, so the plan goes full (fail-closed)."""

    def test_a_missing_store_has_no_mark(self):
        with tempfile.TemporaryDirectory() as tmp, \
                mock.patch.dict(os.environ, {"MECLAW_PIPELINE_DB": tmp + "/none.db"}):
            self.assertIsNone(full_pass.for_plan(False, "welle-x"))

    def test_an_unreadable_store_plans_full(self):
        import sqlite3
        for err in (sqlite3.OperationalError("unable to open database file"),
                    PermissionError("denied")):
            with mock.patch.object(store, "open_ro", side_effect=err):
                got = full_pass.for_plan(False, "welle-x")
            self.assertIsNotNone(got, err)
            self.assertIn("unknown", got["why"])
        with tempfile.TemporaryDirectory() as tmp:
            bad = pathlib.Path(tmp) / "pipeline.db"
            bad.write_text("not a database")
            with mock.patch.dict(os.environ, {"MECLAW_PIPELINE_DB": str(bad)}):
                self.assertIn("unknown", full_pass.for_plan(False, "welle-x")["why"])


class WaveTest(unittest.TestCase):
    def test_the_wave_of_a_pass(self):
        self.assertEqual("welle-tempo-beweis", full_pass.wave_of("welle-tempo-beweis/tb-s1b"))
        # A wave directory of no real wave: a real one makes this file a reader
        # of every report of that wave (turnaround rule 8 python) and costs
        # each of its strands the receipt reuse.
        self.assertEqual("welle-demo", full_pass.wave_of("", "welle-demo-2026-01-02"))
        self.assertEqual("welle-a", full_pass.wave_of("welle-b/s", "welle-a"))
        self.assertEqual("welle-b", full_pass.wave_of("welle-b/s:2", None, ""))
        self.assertIsNone(full_pass.wave_of(""))
        self.assertIsNone(full_pass.wave_of("t1", None))


class PlanTest(StageBase):
    """`plan.build_plan(..., full=)`: the integration plans the full suite, a
    strand and a merge plan without narrowing."""

    def full(self):
        plain_full.mark(self.db, "welle-x", "unexplained red in tests", "tr-0")
        return plain_full.for_plan(False, "welle-x", self.db)

    def test_a_mark_plans_the_next_integration_on_the_full_suite(self):
        old = self.edit_store("// before the wave\n")
        self.cov = dict(self.cov, rev=old)
        plain_events.emit(self.db, "wave_started", "status", {"wave": "welle-x", "start": old})
        self.edit_store("// in the wave\n")
        doc, tests = self.build("integration", self.map_rev, wave="welle-x")
        self.assertFalse(tests["select"]["all"], doc.get("notes"))   # narrowed without a mark
        doc, tests = self.build("integration", self.map_rev, wave="welle-x", full=self.full())
        self.assert_full_suite(doc, tests, "full pass: unexplained red in tests")
        self.assertEqual("unexplained red in tests", doc["full_pass"]["why"])
        self.assertTrue(doc["full_pass"]["since"])
        # stacked on tb-s1 (re-check H2 M1): a full integration names its scope too
        self.assertEqual(("full-suite", "integration", "welle-x", "full pass (#1165)"),
                         tuple(doc["narrowing"][k] for k in ("tests", "stage", "wave", "why")))

    def test_a_mark_plans_the_next_strand_unnarrowed(self):
        self.edit_store("// strand\n")
        doc, tests = self.build("strand", self.map_rev)
        self.assertNotIn("alpha", tests["select"]["rdeps"], doc.get("notes"))  # narrowed
        doc, tests = self.build("strand", self.map_rev, full=self.full())
        self.assert_unnarrowed(doc, tests, "full pass: unexplained red in tests")
        self.assertIn("full_pass", doc)

    def test_a_mark_plans_the_next_merge_unnarrowed(self):
        git(self.root, "checkout", "-q", "-b", "welle-x/s", self.map_rev)
        gated = self.edit_store("// strand\n")
        git(self.root, "checkout", "-q", "master")
        run = self.gate_run(gated)
        base = git(self.root, "rev-parse", "master")
        git(self.root, "merge", "-q", "--no-ff", "-m", "merge", "welle-x/s")
        gate_plan._SOURCE_CACHE.clear()
        doc, tests = self.build("merge", base, strand_run=run)
        self.assertNotIn("alpha", tests["select"]["rdeps"], doc.get("notes"))  # narrowed
        doc, tests = self.build("merge", base, strand_run=run, full=self.full())
        self.assert_unnarrowed(doc, tests, "full pass: unexplained red in tests")


class GateFullTest(GateCase):
    """The gate's wiring: `--full`, the mark into the plan, the receipt
    field, the mark and the clear after the pass (branch `welle-x/t1`)."""

    def setUp(self):
        # The wave of these cases is the branch's (`welle-x`). A strand gate
        # exports MECLAW_GATE_WAVE and its gate-selftest station inherits it;
        # `gate._full_wave` prefers it, so four cases failed in every strand
        # gate that selected this module (#1186). The case owns its env;
        # GateCase's patch of os.environ restores it.
        super().setUp()
        os.environ.pop("MECLAW_GATE_WAVE", None)

    # The four cases #1186 broke under a strand gate's environment.
    WAVE_CASES = ("test_a_mark_of_another_strand_never_reaches_the_plan",
                  "test_a_mark_reaches_the_next_plan",
                  "test_a_green_full_pass_clears_and_its_receipt_carries_the_field",
                  "test_a_narrowed_pass_with_a_misfit_map_marks_its_wave")

    def test_m_n13_a_stale_gate_wave_in_the_env_never_reaches_the_cases(self):
        # Re-check m-N13: the setUp above was pinned only when the run itself
        # carried MECLAW_GATE_WAVE (a strand gate); a plain run stayed green
        # without it. Here the case sets a stale wave itself and runs the four
        # cases under it, so the pin needs no outer variable.
        for name in self.WAVE_CASES:
            with self.subTest(case=name):
                result = unittest.TestResult()
                with mock.patch.dict(os.environ, {"MECLAW_GATE_WAVE": "welle-stale"}):
                    type(self)(name).run(result)
                self.assertEqual(1, result.testsRun)
                self.assertEqual([], [t for _, t in result.failures + result.errors])

    def full_plan(self, notes=()):
        stations = [station("claims"),
                    station("tree-rules", 'exit "$(cat %s 2>/dev/null || echo 0)"' % self.rc_file)]

        def build_plan(repo_root, mode, base, files=None, only=None, calibration=None,
                       full=None, **kw):
            rev = cli_git(repo_root, "rev-parse", "HEAD").strip()
            doc = {"mode": mode, "rev": rev, "base": base or rev, "repo": "meclaw",
                   "classes": [], "crates": [], "stations": [dict(s) for s in stations],
                   "plan_reason": {s["name"]: s["reason"] for s in stations},
                   "critical_path_s": 1.0, "critical_path": [s["name"] for s in stations],
                   "notes": list(notes)}
            if full:
                doc["full_pass"] = dict(full)
                doc["notes"] = ["full pass: %s -- no narrowing" % full["why"]]
            return doc

        patcher = mock.patch.object(gate.plan, "build_plan", side_effect=build_plan)
        patcher.start()
        self.addCleanup(patcher.stop)

    def mark(self, why="FLAKY tests", strand=None):
        db = store.open()
        try:
            full_pass.mark(db, "welle-x", why, "tr-0", **({"strand": strand} if strand else {}))
        finally:
            db.close()

    def pending(self):
        db = store.open()
        try:
            return full_pass.pending(db, "welle-x")
        finally:
            db.close()

    def test_full_is_a_switch_of_the_gate(self):
        o = gate.parse(["strand", "--full"], None)
        self.assertTrue(o.full)
        self.assertFalse(gate.parse(["strand"], None).full)
        self.assertIn("--full", gate.USAGE)

    def test_full_plans_a_full_pass(self):
        self.full_plan()
        rc, out, err = self.gate("strand", "--plan-only", "--full")
        self.assertEqual(rc, 0, out + err)
        self.assertEqual("--full", gate.plan.build_plan.call_args.kwargs["full"]["why"])
        self.assertIn("gate: NOTE full pass: --full", out)

    def test_a_mark_reaches_the_next_plan(self):
        self.full_plan()
        rc, out, err = self.gate("strand", "--plan-only")
        self.assertIsNone(gate.plan.build_plan.call_args.kwargs.get("full"))
        self.mark()
        rc, out, err = self.gate("strand", "--plan-only")
        self.assertEqual(rc, 0, out + err)
        self.assertEqual("FLAKY tests", gate.plan.build_plan.call_args.kwargs["full"]["why"])
        self.assertIn("gate: NOTE full pass: FLAKY tests", out)
        self.assertIsNotNone(self.pending(), "a plan never clears")

    def test_a_mark_of_another_strand_never_reaches_the_plan(self):
        """Re-check S1b m-4: branch `welle-x/t1` is strand t1."""
        self.full_plan()
        self.mark(strand="t2")
        rc, out, err = self.gate("strand", "--plan-only")
        self.assertEqual(rc, 0, out + err)
        self.assertIsNone(gate.plan.build_plan.call_args.kwargs.get("full"))
        self.mark("FLAKY clippy", strand="t1")
        rc, out, err = self.gate("strand", "--plan-only")
        self.assertEqual("FLAKY clippy", gate.plan.build_plan.call_args.kwargs["full"]["why"])
        rc, out, err = self.gate("integration", "--plan-only")
        self.assertEqual("FLAKY clippy", gate.plan.build_plan.call_args.kwargs["full"]["why"])

    def test_a_green_full_pass_clears_and_its_receipt_carries_the_field(self):
        self.full_plan()
        self.mark()
        archive = self.tmp / "archive"
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test",
                                 "--archive", str(archive))
        self.assertEqual(rc, 0, out + err)
        self.assertIsNone(self.pending(), out + err)
        self.assertEqual("FLAKY tests", self.journaled().get("full_pass", {}).get("why"))
        doc = json.loads((archive / "latest" / "receipt.json").read_text())
        self.assertEqual("FLAKY tests", doc.get("full_pass", {}).get("why"))

    def test_a_red_full_pass_leaves_the_mark(self):
        self.full_plan()
        self.mark()
        self.rc_file.write_text("1")
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test")
        self.assertEqual(rc, 1, out + err)
        self.assertEqual("FLAKY tests", (self.pending() or {}).get("why"))

    def test_a_narrowed_pass_with_a_misfit_map_marks_its_wave(self):
        self.full_plan(notes=[MISFITS[0]])
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test")
        self.assertEqual(rc, 0, out + err)
        self.assertIn("map mismatch", (self.pending() or {}).get("why", ""), out + err)
        self.assertIn("gate: NOTE full pass: the next pass of welle-x plans full", out)

    def test_record_names_a_cache_doubt(self):
        keys = {("claims", ""): {"key": "k", "inputs": []}}
        self.assertIsNone(gate._record(store.open(), "tr-1", "local", keys,
                                       dict(TOOLS, rustc="1"), dict(TOOLS, rustc="1"), []))
        doubt = gate._record(store.open(), "tr-1", "local", keys, dict(TOOLS, rustc="1"),
                             dict(TOOLS, rustc="2"), [])
        self.assertIn("rustc", doubt or "")

    def test_the_cache_doubt_is_the_doubt_of_record(self):
        keys = {("claims", ""): {"key": "k", "inputs": []}}
        self.assertIsNone(gate._cache_doubt(keys, dict(TOOLS, rustc="1"), dict(TOOLS, rustc="1")))
        self.assertIsNone(gate._cache_doubt({}, dict(TOOLS, rustc="1"), dict(TOOLS, rustc="2")))
        self.assertEqual(gate._record(store.open(), "tr-1", "local", keys, dict(TOOLS, rustc="1"),
                                      dict(TOOLS, rustc="2"), []),
                         gate._cache_doubt(keys, dict(TOOLS, rustc="1"), dict(TOOLS, rustc="2")))

    # Restack on master 5f7920b02 (tb-s3-store2, #1161): the cache stays the
    # last record of a pass (re-check tb-s3-store M-3), so the full-pass mark
    # runs before it -- a lost mark leaves no cache row behind.
    # m-RS-1: `finish` books a `--cache off` pass with a CACHED station RED (a
    # defect); the mark follows that final verdict, not the stream's GREEN. The
    # defect stands in at its detection (a leaky lookup needs cache metadata,
    # `test_pipeline_ir_path` pins that one).
    def test_a_cache_off_defect_leaves_the_mark(self):
        self.full_plan()
        self.mark()
        with mock.patch.object(gate, "cache_off_defects", return_value=["claims"]):
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--cache", "off")
        self.assertEqual(rc, 1, out + err)
        self.assertIn("DEFECT -- cache=off, yet CACHED", err)
        self.assertEqual("FLAKY tests", (self.pending() or {}).get("why"), out + err)

    def test_the_cache_record_comes_after_the_full_pass_mark(self):
        self.full_plan()
        locked = sqlite3.OperationalError("database is locked")
        with mock.patch.object(gate.full_pass, "after_pass", side_effect=locked), \
                mock.patch.object(gate, "_record") as record, \
                mock.patch.object(gate.store, "WRITE_BACKOFF_S", 0):
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test")
        self.assertEqual(rc, 2, out + err)
        self.assertIn("gate: store: LOST full pass mark", err)
        self.assertEqual(0, record.call_count, out + err)

    # A pass that lost a record ends ABORTED and does not count (#1161): its
    # GREEN stations never clear a full-pass mark.
    def test_a_pass_with_a_lost_record_never_clears_the_mark(self):
        self.full_plan()
        self.mark()
        locked = sqlite3.OperationalError("database is locked")
        with mock.patch.object(gate.flakes, "ingest_run", side_effect=locked), \
                mock.patch.object(gate.store, "WRITE_BACKOFF_S", 0):
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test")
        self.assertEqual(rc, 2, out + err)
        self.assertIn("gate: store: LOST flake register", err)
        self.assertEqual("FLAKY tests", (self.pending() or {}).get("why"), out + err)


class MergeWaveTest(MergeCase):
    """A merge gate runs detached: it names its wave (`MECLAW_GATE_WAVE`) so
    its mark and its full pass find the wave of its strands."""

    def test_the_merge_gate_names_the_wave_of_its_strands(self):
        self.a_strand()
        rc, _, err = self.f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        self.assertEqual("welle-test-2026-01-01", self.f.gate_calls()[0].get("wave"))
        seen = {}

        def fake_run(argv, cwd=None, env=None, **_kw):
            seen.update(env or {})
            return merge.subprocess.CompletedProcess(
                argv, 0, "GATE-SUMMARY strand abcdef123456 1/1 1s GREEN\n", "")
        with mock.patch.dict(os.environ, {"MECLAW_PIPELINE_FAKE_GATE": "",
                                          "MECLAW_GATE_WAVE": "welle-stale",
                                          "MECLAW_MERGE_GATE_CMD": "true"}), \
                mock.patch.object(merge.subprocess, "run", side_effect=fake_run):
            for ctx, want in (({"wave": "welle-test-2026-01-01"}, "welle-test-2026-01-01"),
                              ({}, None)):
                seen.clear()
                try:
                    merge.run_gate("/nonexistent", dict(ctx, rev="abcdef123456", base="b",
                                                        label="S1", strands=["S1"]))
                except merge.Refused:
                    pass
                self.assertEqual(want, seen.get("MECLAW_GATE_WAVE"), ctx)

    def test_the_merge_gate_carries_the_testing_rules_ruling(self):
        # A merge of a testing-rules change gates GREEN only with the owner
        # ruling's register id: `run_gate` hands the caller's
        # MECLAW_TESTING_RULES_RULING to gate.sh (lane_sync.sh `lane_env`
        # forwards it to the lane), and adds none of its own.
        seen = {}

        def fake_run(argv, cwd=None, env=None, **_kw):
            seen.update(env or {})
            return merge.subprocess.CompletedProcess(
                argv, 0, "GATE-SUMMARY strand abcdef123456 1/1 1s GREEN\n", "")
        ctx = dict(rev="abcdef123456", base="b", label="S1", strands=["S1"])
        os.makedirs(os.path.join(merge.state_dir(), "merge"), exist_ok=True)
        for given in ("OR-TV-7", None):
            seen.clear()
            env = {"MECLAW_PIPELINE_FAKE_GATE": "", "MECLAW_MERGE_GATE_CMD": "true"}
            with mock.patch.dict(os.environ, env), \
                    mock.patch.object(merge.subprocess, "run", side_effect=fake_run):
                os.environ.pop("MECLAW_TESTING_RULES_RULING", None)
                if given:
                    os.environ["MECLAW_TESTING_RULES_RULING"] = given
                try:
                    merge.run_gate("/nonexistent", dict(ctx))
                except merge.Refused:
                    pass
            self.assertEqual(given, seen.get("MECLAW_TESTING_RULES_RULING"), given)


if __name__ == "__main__":
    unittest.main()
