"""The serial rest of integration and release, off the critical path (GH #1162).

The integration pass 7b1e0e5 (build01, 12 threads) took 2092 s: 1527 s of
`tests` and 565 s serial around it -- precheck 34 s before `build`, clippy
102 s after `tests` (its booking of the whole lane never fit beside them),
export-audit 88 s after every station. Release added the export tree's own
build (274.9 s) and the full audit (149.8 s) after the suite. These tests pin
the cure and that it stays fail-closed:

- clippy books four threads and starts as soon as `build` frees the target
  lock, beside `tests`; the overbooked cpu of the lane still caps every start;
- precheck runs beside `build`; a red precheck stops the build and every
  station that would judge over it, and the pass is RED as before;
- export-audit waits for the four stations whose receipt verdicts it reads,
  not for every station; the pass still sums every verdict;
- integration and release wait for the widest lane (M0 minima are 24-thread
  values) unless `--any-lane` or `--host`;
- `--cache off` looks nothing up, records as always, says so in GATE-PLAN,
  GATE-LANE and the receipt, and a CACHED station in such a pass is a defect;
- the long tests of the suite start first (nextest `priority`);
- a test nextest retried to green lands in the flake register.
"""

import json
import os
import pathlib
import re
import sys
import tempfile
import threading
import tomllib
import unittest
from unittest import mock

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "scripts"))

import harness_fold  # noqa: E402
import slow_tests  # noqa: E402
from scripts.pipeline import events, flakes, gate, plan as pl, schedule, store  # noqa: E402
from scripts.tests import kit_time, store_isolation  # noqa: E402
from scripts.tests.test_pipeline_cache import fixture_metadata, fixture_tree  # noqa: E402
from scripts.tests.test_pipeline_executor import ExecutorCase  # noqa: E402
from scripts.tests.test_pipeline_executor import station as ex_station  # noqa: E402
from scripts.tests.test_pipeline_flakes import AT, BIN, StoreCase  # noqa: E402
from scripts.tests import fake_ssh  # noqa: E402
from scripts.tests.test_pipeline_gate_cli import PASS_SSH_TEMPLATE, GateCase  # noqa: E402
from scripts.tests.test_pipeline_plan import resolver as plan_resolver  # noqa: E402
from scripts.tests.test_pipeline_schedule import FakeLane, ScheduleCase  # noqa: E402
from scripts.tests.test_pipeline_schedule import make_plan  # noqa: E402
from scripts.tests.test_pipeline_schedule import station as sched_station  # noqa: E402

WIDE, NARROW = 24, 12
# A diff over one crate and the docs: integration and release plan build,
# tests, clippy, the export stations and the four receipt stations.
IR_FILES = ["docs/guide.md", "crates/meclaw-core/src/lib.rs"]


def ir_plan(case, mode, files=IR_FILES):
    """`{name: station}` of a real plan over the fixture workspace."""
    root = fixture_tree(case)
    doc = pl.build_plan(root, mode, "HEAD", files=files, resolver=plan_resolver,
                        metadata=fixture_metadata(root))
    return {s["name"]: s for s in doc["stations"]}


def positions(evs):
    return {(e["event"], e["data"].get("station")): i for i, e in enumerate(evs)}


class IRExecutorCase(ExecutorCase):
    def run_within(self, d, timeout=kit_time.HANG):
        box = {}
        t = threading.Thread(target=lambda: box.update(res=self.run_exec(d)), daemon=True)
        t.start()
        t.join(timeout)
        self.assertFalse(t.is_alive(), "HANG: the run after %.0f s" % timeout)
        return box["res"]

    def lane_dispatch(self, stations, cpu=WIDE):
        return self.dispatch(stations, mode="integration",
                             capacity={"cpu": cpu, "colony": 8, "browser_chromium": 1,
                                       "browser_webkit": 1, "export": 1, "cargo": 1},
                             **{"+cpu_overbook": schedule.CPU_OVERBOOK})


class ClippyBesideTests(IRExecutorCase):
    def test_integration_clippy_starts_after_build_not_after_tests(self):
        # The plan: clippy has no edge on `tests` and books the side width.
        for mode in ("integration", "release"):
            rows = ir_plan(self, mode)
            self.assertNotIn("tests", rows["clippy"]["after"], mode)
            self.assertEqual(pl.resolve_cpu(rows["clippy"]["needs"]["cpu"], WIDE), 4, mode)
        # The run on the 24-thread lane: `tests` 24 + `browser:display` 4 fill
        # 28 of 36 (24 x 1.5); clippy at the plan's booking fits beside them.
        clippy_cpu = pl.resolve_cpu(pl.NEEDS["clippy"]["cpu"], WIDE)
        sts = [ex_station("build", "sleep 0.3", needs={"cpu": WIDE, "cargo": 1}),
               ex_station("tests", "sleep 2", after=["build"], needs={"cpu": WIDE}),
               ex_station("browser:display", "sleep 2", after=["build"], needs={"cpu": 4}),
               ex_station("clippy", "sleep 0.3", needs={"cpu": clippy_cpu, "cargo": 1})]
        rc, evs = self.run_within(self.lane_dispatch(sts))
        self.assertEqual(0, rc, self.err_text)
        pos = positions(evs)
        self.assertLess(pos[("station_ended", "build")], pos[("station_started", "clippy")],
                        "one cargo station at a time: clippy after build")
        self.assertLess(pos[("station_started", "clippy")], pos[("station_ended", "tests")],
                        "clippy waited behind tests")

    def test_admission_stays_fail_closed_with_parallel_clippy(self):
        # In the run: the overbooked cpu caps every start, clippy beside tests or not.
        cap = WIDE * schedule.CPU_OVERBOOK
        needs = {"build": {"cpu": WIDE, "cargo": 1}, "tests": {"cpu": WIDE},
                 "browser:display": {"cpu": 4},
                 "clippy": {"cpu": pl.resolve_cpu(pl.NEEDS["clippy"]["cpu"], WIDE), "cargo": 1},
                 "unwrap-budget": {"cpu": 2, "cargo": 1},
                 "doctests": {"cpu": pl.resolve_cpu(pl.NEEDS["doctests"]["cpu"], WIDE), "cargo": 1}}
        after = {"tests": ["build"], "browser:display": ["build"]}
        sts = [ex_station(n, "sleep %s" % ("1.5" if n in ("tests", "browser:display") else "0.2"),
                          after=after.get(n, ()), needs=nd) for n, nd in needs.items()]
        rc, evs = self.run_within(self.lane_dispatch(sts))
        self.assertEqual(0, rc, self.err_text)
        used, peak, running = 0, 0, set()
        for e in evs:
            name = e["data"].get("station")
            if e["event"] == "station_started":
                running.add(name)
                used += needs[name]["cpu"]
                peak = max(peak, used)
            elif e["event"] == "station_ended" and name in running:
                running.discard(name)
                used -= needs[name]["cpu"]
        self.assertLessEqual(peak, cap, "the run booked more cpu than the lane overbooks")
        pos = positions(evs)
        self.assertLess(pos[("station_started", "clippy")], pos[("station_ended", "tests")],
                        "clippy waited behind tests")
        self.assertLess(pos[("station_ended", "tests")], pos[("station_started", "doctests")],
                        "doctests books the lane: never beside tests")
        # At the admission: the pass books the whole lane, a second cargo pass waits.
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        db = store.open(os.path.join(tmp.name, "pipeline.db"))
        self.addCleanup(db.close)
        schedule.sync_runners(db, [FakeLane("build04", threads=WIDE)], states={"build04": "idle"})
        plan = make_plan(sched_station("build", cpu=WIDE, cargo=1),
                         sched_station("tests", cpu=WIDE, after=["build"]),
                         sched_station("clippy", cpu=needs["clippy"]["cpu"], cargo=1))
        plan["mode"] = "integration"
        kw = dict(repo="meclaw", mode="integration", wave="welle-x", base=plan["base"],
                  lane_pref=None, now=1_800_000_000, wait=False)
        first = schedule.admit(db, plan, strand="welle-x/a", **kw)
        self.assertIsInstance(first, schedule.Admission)
        second = schedule.admit(db, plan, strand="welle-x/b", **kw)
        self.assertIsInstance(second, schedule.Queued)
        self.assertIn("one cargo run in the build path", second.reason)
        self.assertFalse(second.final, "a busy lane is an open cause")


class PrecheckBesideBuild(IRExecutorCase):
    def test_precheck_parallel_red_aborts_pass(self):
        rows = ir_plan(self, "integration")
        self.assertNotIn("precheck", rows["build"]["after"], "build starts beside precheck")
        for name in ("tests", "clippy", "doctests"):
            self.assertIn("precheck", rows[name]["after"], name)
        sts = [ex_station("precheck", "sleep 1; exit 1"),
               # A build the red precheck does not stop holds the run past
               # the hang marker: the stop is the event, not a duration.
               ex_station("build", "sleep 600", needs={"cpu": 8, "cargo": 1}),
               ex_station("tests", "true", after=["precheck", "build"], needs={"cpu": 8}),
               ex_station("export-tree-tests", "sleep 600", needs={"cpu": 4, "export": 1}),
               ex_station("docs-links", "true")]
        rc, evs = self.run_within(self.lane_dispatch(sts), timeout=kit_time.HANG)
        self.assertEqual(1, rc, self.err_text)
        ended = self.ended(evs)
        self.assertEqual("RED", ended["precheck"]["verdict"])
        for name in ("build", "export-tree-tests", "tests"):
            self.assertEqual(("SKIP", "precheck red"),
                             (ended[name]["verdict"], ended[name].get("reason")), name)
        self.assertEqual("GREEN", ended["docs-links"]["verdict"])
        pe = [e for e in evs if e["event"] == "pass_ended"][-1]["data"]
        self.assertEqual("RED", pe["verdict"])


class AuditOffTheCriticalPath(IRExecutorCase):
    def test_export_audit_off_critical_path_keeps_verdict(self):
        for mode in ("integration", "release"):
            rows = ir_plan(self, mode)
            audit = rows["export-audit"]
            reads = set(pl.AUDIT_READS) & set(rows)
            self.assertTrue(reads, mode)
            self.assertLessEqual(reads, set(audit["after"]), mode)
            for name in ("build", "tests", "clippy", "doctests"):
                self.assertNotIn(name, audit["after"], "%s: audit waits for %s" % (mode, name))
            self.assertEqual({"outcome": "any"}, audit["when"], mode)
        release = ir_plan(self, "release")
        for name in ("export-tree-tests", "export-audit"):
            self.assertEqual(0, release[name]["needs"]["cargo"], "own target: " + name)
            self.assertEqual(1, release[name]["needs"]["export"], name)
        # The run: the audit ends before `tests`, `tests` goes red after it,
        # and the pass is RED all the same.
        sts = [ex_station("corpus", "true"),
               ex_station("tests", "sleep 1.5; exit 1", needs={"cpu": 8}),
               ex_station("export-audit", "true", after=["corpus"], when="any",
                          needs={"export": 1})]
        rc, evs = self.run_within(self.lane_dispatch(sts))
        self.assertEqual(1, rc, self.err_text)
        pos = positions(evs)
        self.assertLess(pos[("station_ended", "export-audit")], pos[("station_ended", "tests")])
        ended = self.ended(evs)
        self.assertEqual(("GREEN", "RED"),
                         (ended["export-audit"]["verdict"], ended["tests"]["verdict"]))
        pe = [e for e in evs if e["event"] == "pass_ended"][-1]["data"]
        self.assertEqual("RED", pe["verdict"])


class WidestLane(ScheduleCase):
    def wide_and_narrow(self):
        schedule.sync_runners(self.db, [FakeLane("build01", threads=NARROW),
                                        FakeLane("build04", threads=WIDE)],
                              states={"build01": "idle", "build04": "idle"})
        self.warm("build01")  # warm-first alone would take the narrow lane

    def cargo_plan(self, mode):
        plan = make_plan(sched_station("build", cpu=8, cargo=1, timeout=20),
                         sched_station("tests", cpu=8, after=["build"], timeout=30))
        plan["mode"] = mode
        return plan

    def test_integration_prefers_widest_lane(self):
        self.wide_and_narrow()
        for mode, strand in (("integration", "welle-x/i"), ("release", "welle-x/r")):
            adm = self.admit(self.cargo_plan(mode), mode=mode, strand=strand)
            self.assertIsInstance(adm, schedule.Admission, mode)
            self.assertEqual("build04", adm.lane, mode)
            self.finish(adm, {"build": "GREEN", "tests": "GREEN"})
        self.assertEqual("build01", self.admit(self.cargo_plan("strand")).lane,
                         "strand keeps warm-first")
        self.assertEqual(WIDE, schedule.lane_vcpu(self.db, "build04"))
        self.assertEqual(0, schedule.lane_vcpu(self.db, "nowhere"))

    def test_an_unreachable_lane_is_not_the_widest(self):
        # GH #1162 hA-m-5: `_admit` and `_warm_first` skip an unreachable lane like a
        # drained one; `widest` counted it, so I/R waited for a lane it cannot take.
        rows = [{"name": "w", "state": "unreachable", "class": "build",
                 "capacity": '{"cpu": %d}' % WIDE},
                {"name": "n", "state": "idle", "class": "build",
                 "capacity": '{"cpu": %d}' % NARROW}]
        self.assertEqual((NARROW, ["n"]), schedule.widest(rows))
        schedule.sync_runners(self.db, [FakeLane("build01", threads=NARROW),
                                        FakeLane("build04", threads=WIDE)],
                              states={"build01": "idle", "build04": "unreachable"})
        adm = self.admit(self.cargo_plan("release"), mode="release", strand="welle-x/r")
        self.assertIsInstance(adm, schedule.Admission, getattr(adm, "reason", adm))
        self.assertEqual("build01", adm.lane)

    def test_a_busy_fallback_names_the_unreachable_lane_not_a_narrower_one(self):
        # GH #1162 m-RC-1: with the 24-thread lane unreachable and the 12-thread one
        # busy, the wait message listed the unreachable lane as "not build04 --
        # --any-lane takes a narrower one": build04 is wider, and --any-lane does
        # not reach it. It is named for what it is; no narrower lane, no switch.
        schedule.sync_runners(self.db, [FakeLane("build01", threads=NARROW),
                                        FakeLane("build04", threads=WIDE)],
                              states={"build01": "idle", "build04": "unreachable"})
        busy = self.admit(self.cargo_plan("release"), mode="release", strand="welle-x/a")
        self.assertEqual("build01", busy.lane)
        waits = self.admit(self.cargo_plan("release"), mode="release", strand="welle-x/b")
        self.assertIsInstance(waits, schedule.Queued)
        self.assertFalse(waits.final, "a busy lane is an open cause: --wait-admission waits")
        self.assertIn("lane build04 is unreachable", waits.reason)
        self.assertIn("lane build01", waits.reason)
        self.assertNotIn("not build04", waits.reason)
        self.assertNotIn("--any-lane", waits.reason)

    def test_release_waits_for_widest_lane_unless_switch(self):
        self.wide_and_narrow()
        busy = self.admit(self.cargo_plan("release"), mode="release", strand="welle-x/a")
        self.assertEqual("build04", busy.lane)
        waits = self.admit(self.cargo_plan("release"), mode="release", strand="welle-x/b")
        self.assertIsInstance(waits, schedule.Queued)
        self.assertFalse(waits.final, "a busy widest lane is an open cause: --wait-admission waits")
        self.assertIn("waits for the widest lane", waits.reason)
        self.assertIn("--any-lane", waits.reason)
        switch = self.admit(self.cargo_plan("release"), mode="release", strand="welle-x/b",
                            any_lane=True)
        self.assertIsInstance(switch, schedule.Admission)
        self.assertEqual("build01", switch.lane)
        self.finish(switch, {"build": "GREEN", "tests": "GREEN"})
        host = self.admit(self.cargo_plan("integration"), mode="integration",
                          strand="welle-x/c", lane_pref="build01")
        self.assertEqual("build01", host.lane, "--host names the lane")


class CacheOffLocal(GateCase):
    def test_cache_off_never_looks_up(self):
        self.python_plan()
        self.seed_metadata()
        no_lookup = mock.patch.object(gate.cache, "lookup",
                                      side_effect=AssertionError("cache=off looked up"))
        with no_lookup:
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--cache", "off")
        self.assertEqual(rc, 0, out + err)
        self.assertEqual(self.verdicts(out), {"claims": "GREEN", "tree-rules": "GREEN"})
        self.assertRegex(out, r"(?m)^GATE-PLAN strand \S+ stations=2 cached=0 .* cache=off$")
        # Recorded under the keys a pass with the cache finds (the key never
        # sees the switch) ...
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test")
        self.assertEqual(rc, 0, out + err)
        self.assertEqual(self.verdicts(out), {"claims": "CACHED", "tree-rules": "CACHED"})
        self.assertNotIn("cache=off", out)
        # ... and the environment form runs everything again.
        with no_lookup, mock.patch.dict(os.environ, {gate.CACHE_ENV: "off"}):
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test")
        self.assertEqual(rc, 0, out + err)
        self.assertEqual(self.verdicts(out), {"claims": "GREEN", "tree-rules": "GREEN"})

    def test_cache_off_run_with_cached_station_is_defect(self):
        self.assertEqual(["a"], gate.cache_off_defects(
            {"stations": [{"name": "a", "verdict": "CACHED"}, {"name": "b", "verdict": "GREEN"}]}))
        self.assertEqual([], gate.cache_off_defects({}))
        self.python_plan()
        self.seed_metadata()
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test")
        self.assertEqual(rc, 0, out + err)
        real = gate._lookup

        def leaky(db, doc, repo, metadata, fp, lane_name, mode, use_cache, build):
            return real(db, doc, repo, metadata, fp, lane_name, mode, "on", build)  # ignores the switch

        with mock.patch.object(gate, "_lookup", side_effect=leaky):
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--cache", "off")
        self.assertEqual(rc, 1, out + err)
        self.assertEqual(self.verdicts(out), {"claims": "CACHED", "tree-rules": "CACHED"})
        self.assertRegex(out, r"(?m)^GATE-SUMMARY strand \S+ \d+/\d+ \d+s RED$")
        self.assertIn("DEFECT", err)
        self.assertIn("claims tree-rules", err)


class CacheOffLane(GateCase):
    def setUp(self):
        super().setUp()
        fake_ssh.install(self.bin, PASS_SSH_TEMPLATE)
        self.lanes_file.write_text("lane-l  builder@lane-l.invalid  threads=4\n")
        os.environ["MECLAW_LANE_ROOT"] = str(self.lane_root)
        real = gate.lane.exec_stream

        def exec_stream(ln, dispatch, *a, **kw):
            dispatch = dict(dispatch, budget=dict(dispatch.get("budget") or {},
                                                  need_free_g=0, free_min_g=0, reserve_g=0))
            return real(ln, dispatch, *a, **kw)

        patcher = mock.patch.object(gate.lane, "exec_stream", side_effect=exec_stream)
        patcher.start()
        self.addCleanup(patcher.stop)
        # the lane's register as `lanes fingerprint` writes it: every key tool with a
        # version (re-check tv-bn M7r A-M1: an empty register keys nothing)
        keyed = gate.keyed_fingerprint
        tools = {k: "fixture" for k in gate.cache.KEY_TOOLS}
        patcher = mock.patch.object(gate, "keyed_fingerprint",
                                    side_effect=lambda fp, env: dict(tools, **keyed(fp, env)))
        patcher.start()
        self.addCleanup(patcher.stop)

    def test_the_lane_path_looks_up_only_with_the_cache(self):
        """A2 point 23 (L-h1-1): the lane path hands `--cache` to `_lookup` on its own
        line (`gate._run_lane`); without the switch there a `--cache off` pass on a
        lane serves every known key CACHED. The store answers every key as known."""
        self.kern_plan()
        self.seed_metadata()
        calls = []

        def every_key_known(db, keys, repo, mode, lane_name, now):
            calls.append(lane_name)
            return sorted(({"station": k[0], "part": k[1], "key": v["key"], "from_run": "r0"}
                           for k, v in keys.items()), key=lambda k: (k["station"], k["part"]))

        known = mock.patch.object(gate.cache, "lookup", side_effect=every_key_known)
        with known:
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(rc, 0, out + err)
        self.assertEqual(["lane-l"], calls)
        self.assertEqual({"clippy": "CACHED", "claims": "CACHED"}, self.verdicts(out))
        self.assertRegex(out, r"(?m)^GATE-PLAN strand \S+ stations=2 cached=2 .*lane=lane-l")
        calls.clear()
        with known:
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l",
                                     "--cache", "off")
        self.assertEqual(rc, 0, out + err)
        self.assertEqual([], calls, "cache=off looked up on the lane path")
        self.assertEqual({"clippy": "GREEN", "claims": "GREEN"}, self.verdicts(out))
        self.assertRegex(out, r"(?m)^GATE-PLAN strand \S+ stations=2 cached=0 .*lane=lane-l .*cache=off$")

    def test_cache_off_marks_plan_and_receipt(self):
        self.kern_plan()
        archive = self.tmp / "archive"
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l",
                                 "--archive", str(archive), "--cache", "off")
        self.assertEqual(rc, 0, out + err)
        self.assertRegex(out, r"(?m)^GATE-PLAN strand \S+ .* lane=lane-l .* cache=off$")
        lane_line = r"GATE-LANE lane-l crit=\d+ lane_s=\d+ rounds=1 vcpu=4 cache=off"
        self.assertRegex(out, r"(?m)^%s$" % lane_line)
        self.assertRegex((archive / "latest" / "summary.txt").read_text(), lane_line)
        db = store.open()
        planned = [json.loads(r[0]) for r in db.execute(
            "SELECT data FROM journal WHERE event = 'pass_planned'")]
        self.assertEqual(["off"], [p.get("cache") for p in planned])
        rc, out, err = self.gate("strand", "--plan-only", "--cache", "off")
        self.assertEqual(rc, 0, out + err)
        self.assertRegex(out.splitlines()[-1], r"^GATE-PLAN strand .* cache=off$")
        rc, out, err = self.gate("strand", "--plan-only", "--cache", "bogus")
        self.assertEqual(rc, 2, out + err)
        self.assertIn("--cache takes on|off", err)


class FlakeIssuesReachTheDispatch(GateCase):
    """Re-check Post-xhigh-8 (recheck-tb-s10-b M1): the register's issues
    (`flakes.dispatch_issues`) ride in the dispatch's `flake_issues` on both
    paths, `gate._run_local` and `gate._run_lane` -- the executor names them on
    a FLAKY station. Dropping either line stayed green."""

    ISSUES = {"example-cells::wobbles": 1180}

    def capture(self, lane_budget=False):
        """The `flake_issues` of every dispatch handed to `lane.exec_stream`."""
        seen = []
        real = gate.lane.exec_stream

        def exec_stream(ln, dispatch, *a, **kw):
            seen.append(dispatch.get("flake_issues"))
            if lane_budget:
                dispatch = dict(dispatch, budget=dict(dispatch.get("budget") or {},
                                                      need_free_g=0, free_min_g=0, reserve_g=0))
            return real(ln, dispatch, *a, **kw)

        for patcher in (mock.patch.object(gate.lane, "exec_stream", side_effect=exec_stream),
                        mock.patch.object(gate.flakes, "dispatch_issues",
                                          side_effect=lambda db: dict(self.ISSUES))):
            patcher.start()
            self.addCleanup(patcher.stop)
        return seen

    def test_the_local_path_carries_the_register_issues(self):
        self.python_plan()
        seen = self.capture()
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test")
        self.assertEqual(rc, 0, out + err)
        self.assertEqual([self.ISSUES], seen)

    def test_the_lane_path_carries_the_register_issues(self):
        fake_ssh.install(self.bin, PASS_SSH_TEMPLATE)
        self.lanes_file.write_text("lane-l  builder@lane-l.invalid  threads=4\n")
        os.environ["MECLAW_LANE_ROOT"] = str(self.lane_root)
        self.kern_plan()
        seen = self.capture(lane_budget=True)
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(rc, 0, out + err)
        self.assertEqual([self.ISSUES], seen)


# --- long tests first (nextest `priority`) -------------------------------------------------

# M0 A4 j24 on build04 (lanes/dt-m0/res-build04/logs/A4junit-j24-1.junit.xml):
# 11094 tests, these 47 above 30 s, 2009.2 s together. Names as nextest ran them
# then, before the harness fold (`<crate>::<stem>`, `<test>`), with seconds.
LONG_TESTS = [
    ("meclaw-cells::gh906_a_repo_round_trips_through_the_hive", "a_repo_round_trips_through_the_hive", 69.7),
    ("meclaw-cells::gh974_a_person_round_leaves_no_dead_letter", "a_person_round_leaves_only_declared_dead_letters", 62.1),
    ("meclaw-cells::gh975_a_projection_job_fits_the_colony_ttl", "a_projection_job_of_six_files_fits_ttl_64", 56.5),
    ("meclaw-cells::gh974_a_person_round_leaves_no_dead_letter", "the_ways_own_messages_never_dead_letter", 54.9),
    ("meclaw-cells::gh954_two_first_turns_open_one_generation", "two_first_turns_open_one_generation_runs_9_to_11", 53.8),
    ("meclaw-cells::gh954_two_first_turns_open_one_generation", "two_first_turns_open_one_generation_runs_3_to_5", 53.4),
    ("meclaw-cells::gh954_two_first_turns_open_one_generation", "two_first_turns_open_one_generation_runs_15_to_17", 53.2),
    ("meclaw-cells::gh954_two_first_turns_open_one_generation", "two_first_turns_open_one_generation_runs_0_to_2", 53.2),
    ("meclaw-cells::gh954_two_first_turns_open_one_generation", "two_first_turns_open_one_generation_runs_6_to_8", 52.8),
    ("meclaw-cells::gh954_two_first_turns_open_one_generation", "two_first_turns_open_one_generation_runs_18_to_20", 52.3),
    ("meclaw-cells::gh954_two_first_turns_open_one_generation", "two_first_turns_open_one_generation_runs_12_to_14", 51.9),
    ("meclaw-cells::gh953_a_round_change_closes_after_the_last_answer", "a_round_change_waits_for_a_late_last_answer_runs_5_to_9", 50.3),
    ("meclaw-cells::gh953_a_round_change_closes_after_the_last_answer", "a_round_change_waits_for_a_late_last_answer_runs_15_to_19", 50.2),
    ("meclaw-cells::gh953_a_round_change_closes_after_the_last_answer", "a_round_change_waits_for_a_late_last_answer_runs_0_to_4", 50.0),
    ("meclaw-cells::gh953_a_round_change_closes_after_the_last_answer", "a_round_change_waits_for_a_late_last_answer_runs_10_to_14", 49.5),
    ("meclaw-cells::gh953_a_round_change_closes_after_the_last_answer", "a_round_change_closes_after_the_last_answer_runs_15_to_19", 48.8),
    ("meclaw-cells::gh953_a_round_change_closes_after_the_last_answer", "a_round_change_closes_after_the_last_answer_runs_5_to_9", 48.7),
    ("meclaw-cells::gh953_a_round_change_closes_after_the_last_answer", "a_round_change_closes_after_the_last_answer_runs_0_to_4", 48.7),
    ("meclaw-cells::gh953_a_round_change_closes_after_the_last_answer", "a_round_change_closes_after_the_last_answer_runs_10_to_14", 48.3),
    ("meclaw-cells::gh905_a_workspace_is_materialized_and_adopted", "a_workspace_is_materialized_and_adopted", 45.6),
    ("meclaw-cells::gh951_an_alias_pushes_the_brief", "an_alias_pushes_the_brief", 44.9),
    ("meclaw-cells::gh980_the_writer_drives_its_projection", "export_after_pull_commits_nothing", 39.2),
    ("meclaw-cells::gh953_a_round_change_closes_after_the_last_answer", "a_double_message_without_ids_closes_after_the_answer_to_the_last_turn_runs_0_to_2", 39.2),
    ("meclaw-cells::gh953_a_round_change_closes_after_the_last_answer", "a_double_message_without_ids_closes_after_the_answer_to_the_last_turn_runs_3_to_5", 39.2),
    ("meclaw-cells::gh945_the_graph_answers_without_waking_the_files", "the_graph_answers_across_files_and_wakes_none", 39.0),
    ("meclaw::gh1061_the_organism_example_starts_and_its_member_gets_the_key", "gh1061_the_organism_example_starts_and_its_member_gets_the_key", 38.1),
    ("meclaw-cells::gh995_every_run_tool_result_names_its_tool", "gh995_an_observer_hears_every_holder_by_name", 37.5),
    ("meclaw-cells::gh980_the_writer_drives_its_projection", "six_file_export_fits_ttl_64", 37.3),
    ("meclaw-cells::gh945_a_rename_breaks_and_reports_the_edge", "a_renamed_function_breaks_its_callers_and_a_removed_file_its_links", 37.3),
    ("meclaw-cells::gh995_every_run_tool_result_names_its_tool", "gh995_every_run_tool_result_names_its_tool", 37.2),
    ("meclaw-cells::gh995_every_run_tool_result_names_its_tool", "gh995_the_body_text_is_the_result", 37.0),
    ("meclaw-cells::gh995_every_run_tool_result_names_its_tool", "gh995_a_derive_job_behind_a_run_keeps_its_budget", 36.8),
    ("meclaw-cells::710_the_sheet_holds_in_both_engines_browser", "the_sheet_holds_in_webkit_hold", 35.7),
    ("meclaw-cells::gh929_every_budget_segment_fits_its_reserve", "s5_without_its_restoring_edges_a_file_turn_does_not_fit", 35.5),
    ("meclaw-cells::gh302_the_stack_grows_from_templates", "d_a_second_channel_is_one_instantiation_in_the_member", 35.1),
    ("meclaw-cells::gh855_a_grown_assistant_is_a_subscriber", "a_generation_and_a_member_grown_in_the_shell_become_subscribers", 34.4),
    ("meclaw-cells::gh980_the_writer_drives_its_projection", "exec_cannot_steer_the_view", 34.4),
    ("meclaw-cells::gh933_the_close_pass_never_crosses_a_round", "round_eb_files_at_the_source_orders_0_to_2", 32.4),
    ("meclaw-cells::gh933_the_close_pass_never_crosses_a_round", "round_eb_files_at_the_source_orders_3_to_5", 32.4),
    ("meclaw-cells::710_the_sheet_holds_in_both_engines_browser", "the_sheet_holds_in_chromium_hold", 32.2),
    ("meclaw-cells::gh422_the_manifest_grows_the_same_stack", "a_manifest_grows_the_same_stack_as_the_five_declarations", 32.2),
    ("meclaw-cells::curator_cells", "a_failed_summary_keeps_the_old_window", 32.2),
    ("meclaw-cells::gh974_a_person_round_leaves_no_dead_letter", "growing_a_level_leaves_only_declared_dead_letters", 31.9),
    ("meclaw-cells::gh933_the_close_pass_never_crosses_a_round", "round_ea_files_at_the_source_orders_3_to_5", 31.3),
    ("meclaw-cells::gh422_the_manifest_grows_the_same_stack", "the_manifest_is_resumable_from_where_it_stopped", 31.3),
    ("meclaw-cells::gh933_the_close_pass_never_crosses_a_round", "round_ea_files_at_the_source_orders_0_to_2", 31.1),
    ("meclaw-cells::gh466_a_recipe_grown_level_boots", "the_rendered_levels_grow_the_tree_the_declarations_grow", 30.6),
]
PRIORITY_TERM = re.compile(r"^binary_id\(=([^)]+)\) & \(?(test\(=[^)]+\)(?: \+ test\(=[^)]+\))*)\)?$")


class Placement:
    """Where the harness table puts a test (`harness_fold.Crate.plan`, GH #1119):
    a kept stem stays `<crate>::<stem>` with its name, a folded one runs in
    `<crate>::<harness>` as `<stem>::<test>`."""

    def __init__(self):
        self.crates = {}
        for d in sorted((REPO / "crates").iterdir()):
            cargo = d / "Cargo.toml"
            if (d / "tests").is_dir() and cargo.is_file():
                m = re.search(r'(?m)^name\s*=\s*"([^"]+)"', cargo.read_text())
                self.crates[m.group(1) if m else d.name] = d.name
        self._plans = {}

    def crate(self, name):
        if name not in self._plans:
            c = harness_fold.Crate(str(REPO), self.crates[name])
            self._plans[name] = (c.plan(), c.files)
        return self._plans[name]

    def folded(self, old_binary, test):
        crate, stem = old_binary.split("::", 1)
        where = self.crate(crate)[0][stem][0]
        if where == "keep":
            return old_binary, test
        return "%s::%s" % (crate, where), "%s::%s" % (stem, test)

    def source(self, binary, test):
        """The source text a test of `binary` named `test` comes from, or None."""
        crate, harness = binary.split("::", 1)
        if crate not in self.crates:
            return None
        plan, files = self.crate(crate)
        stem = test.split("::", 1)[0] if "::" in test else harness
        where = (plan.get(stem) or (None,))[0]
        if where is None or where != ("keep" if stem == harness else harness):
            return None
        return files[stem][1]


def priority_overrides(config):
    return [ov for ov in config["profile"]["default"].get("overrides", []) if "priority" in ov]


def priority_of(config, profile, binary, test):
    """The `priority` nextest gives a test in `profile`: the first matching
    override that sets it -- the profile's own first, then `default`'s."""
    profiles = config.get("profile", {})
    chain = [profiles.get(profile, {})] + ([profiles["default"]] if profile != "default" else [])
    for prof in chain:
        for ov in prof.get("overrides", []):
            if "priority" in ov and slow_tests.compile_filter(ov["filter"])(binary, test):
                return ov["priority"]
    return 0


def group_in(config, profile, binary, test):
    """The ONE test group nextest puts a test into in `profile`: the first
    matching override that sets `test-group` -- the profile's own, then `default`'s."""
    profiles = config.get("profile", {})
    chain = [profiles.get(profile, {})] + ([profiles["default"]] if profile != "default" else [])
    for prof in chain:
        for ov in prof.get("overrides", []):
            if "test-group" in ov and slow_tests.compile_filter(ov["filter"])(binary, test):
                return ov["test-group"]
    return None


# Z-INT-W § 2 (GH #1164): `priority` alone let all 47 start together at the
# head of the station; on build01/12 the twelve slots held twelve of them, and
# each ran 1.5-2.3x slower (gh974 62 s -> 135 s, 12 over the 80 s mark of
# slow-tests). Before S4 the 47 were spread over the run: 2009 s of them in a
# 470 s station on build04 is 4.3 at a time on average, and 0 went over the
# mark in every full run of the wave. The group keeps them first but never
# more than that many at once.
LONG_FIRST_WIDTH = 4
# Each long test takes two slots (GH #1164: gh906 over the mark at the station head).
LONG_FIRST_THREADS = 2
# The two WebKit/Chromium sheet tests stay in the group an earlier override
# gives them (first match wins, `scripts/tests/test_nextest_groups.py`).
LONG_ELSEWHERE = {"710_the_sheet_holds_in_both_engines_browser"}


class LongTestsFirst(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.config = tomllib.loads((REPO / ".config" / "nextest.toml").read_text())
        cls.place = Placement()

    def test_long_tests_get_priority_override(self):
        self.assertEqual(47, len(LONG_TESTS))
        self.assertAlmostEqual(2009.2, sum(s for _, _, s in LONG_TESTS), delta=0.2)  # junit rounding
        for profile in ("default", "ci"):
            for old, name, _ in LONG_TESTS:
                binary, test = self.place.folded(old, name)
                # The rule: the measured seconds of the slowest of its source file.
                want = min(100, round(max(s for o, _, s in LONG_TESTS if o == old)))
                with self.subTest(profile=profile, test="%s %s" % (binary, test)):
                    self.assertEqual(want, priority_of(self.config, profile, binary, test))
        # The same seconds, the same order: the longest goes first.
        by_prio = sorted(LONG_TESTS, key=lambda r: -priority_of(
            self.config, "default", *self.place.folded(r[0], r[1])))
        self.assertEqual("gh906_a_repo_round_trips_through_the_hive",
                         by_prio[0][0].split("::", 1)[1])
        # A short test of the same binary keeps the default.
        self.assertEqual(0, priority_of(self.config, "default", "meclaw-cells::round_2",
                                        "gh906_a_repo_round_trips_through_the_hive::nothing_else"))
        # Priority and nothing else: groups and leashes stay where they are.
        for ov in priority_overrides(self.config):
            self.assertEqual({"filter", "priority"}, set(ov), ov["filter"])

    def test_long_tests_share_one_narrow_group(self):
        self.assertEqual(LONG_FIRST_WIDTH, self.config["test-groups"]["long-first"]["max-threads"])
        for profile in ("default", "ci"):
            for old, name, _ in LONG_TESTS:
                binary, test = self.place.folded(old, name)
                stem = old.split("::", 1)[1]
                with self.subTest(profile=profile, test="%s %s" % (binary, test)):
                    group = group_in(self.config, profile, binary, test)
                    if stem in LONG_ELSEWHERE:
                        self.assertIn(group, ("colony-boot", "webkit"))
                    else:
                        self.assertEqual("long-first", group)
        # Only the measured 47: a short test of the same binary and stem runs free.
        self.assertIsNone(group_in(self.config, "default", "meclaw-cells::member_road",
                                   "gh974_a_person_round_leaves_no_dead_letter::nothing_else"))
        # The group sets two slots and nothing else: no leash, no retry, no priority of its own.
        for ov in self.config["profile"]["default"]["overrides"]:
            if ov.get("test-group") == "long-first":
                self.assertEqual({"filter", "test-group", "threads-required"}, set(ov), ov["filter"])
                self.assertEqual(LONG_FIRST_THREADS, ov["threads-required"])

    def test_priority_overrides_name_existing_tests(self):
        overrides = priority_overrides(self.config)
        self.assertTrue(overrides)
        named = set()
        for ov in overrides:
            m = PRIORITY_TERM.match(ov["filter"])
            self.assertIsNotNone(m, "form `binary_id(=B) & (test(=N) + ...)`: " + ov["filter"])
            binary = m.group(1)
            for test in re.findall(r"test\(=([^)]+)\)", m.group(2)):
                with self.subTest(binary=binary, test=test):
                    text = self.place.source(binary, test)
                    self.assertIsNotNone(text, "no source places %s in %s" % (test, binary))
                    self.assertRegex(text, r"\b%s\b" % re.escape(test.rsplit("::", 1)[-1]))
                    named.add((binary, test))
        want = {self.place.folded(old, name) for old, name, _ in LONG_TESTS}
        self.assertEqual(want, named, "the priority overrides name exactly the measured 47")
        # The reader bites: the name before the fold is no name.
        self.assertIsNone(self.place.source("meclaw-cells::gh906_a_repo_round_trips_through_the_hive",
                                            "a_repo_round_trips_through_the_hive"))


# --- the flake register (Q15) ------------------------------------------------------------

JUNIT_RETRIED = """<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="2" failures="0" errors="0">
  <testsuite name="%(bin)s" tests="2" failures="0" errors="0" disabled="0">
    <testcase name="holds" classname="%(bin)s" time="0.1"/>
    <testcase name="wobbles" classname="%(bin)s" time="0.4">
      <flakyFailure type="test failure" message="first try">boom</flakyFailure>
    </testcase>
  </testsuite>
</testsuites>
"""


class FlakeRegister(StoreCase):
    def test_junit_flaky_lands_in_flake_register(self):
        junit = self.root / "junit.xml"
        junit.write_text(JUNIT_RETRIED % {"bin": BIN})
        run, other = "run-0000000000f1", "run-0000000000f2"
        self.pass_planned(run, AT)
        for body in flakes.junit_parts(str(junit)):
            events.emit(self.db, "test_result", "note", dict(body, run=run, station="tests"),
                        run=run, at=AT)
        elsewhere = "example-cells::another_binary"
        self.result_row(other, elsewhere, flaky=["wobbles_too"])
        self.assertIsNone(self.row(f"{BIN}::wobbles"))
        flakes.ingest_run(self.db, run, {}, at=AT)
        row = self.row(f"{BIN}::wobbles")
        self.assertIsNotNone(row, "a test nextest retried to green is missing from the register")
        self.assertEqual((1, 0, 1), (row["runs"], row["reds"], row["flaky"]))
        self.assertIsNone(self.row(f"{BIN}::holds"), "a green test is no flake")
        self.assertIsNone(self.row(f"{elsewhere}::wobbles_too"),
                          "another run's line waits for its own ingest")
        flakes.ingest_run(self.db, other, {}, at=AT)
        self.assertEqual(1, self.row(f"{elsewhere}::wobbles_too")["flaky"])


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


if __name__ == "__main__":
    unittest.main()
