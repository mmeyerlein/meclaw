"""The build in two phases and the order by measured durations (GH #1162, Weg d + A3).

Weg d: `browser:display` needs only its seven lock binaries, about 12 s of
warm build on build04, but waited for the whole `build` (192 s in A2). Phase 1
`build:browser` builds exactly those targets with the same `-p` set as the
main build -- resolver 3 unifies features per cargo call, a narrower set gives
other unit hashes and builds the crates twice (build04,
`run-9e5307de0bc4/logs/build.log`). `browser:display` starts right after it and
waits for no precheck: a red precheck ends it as it ends `build`.

A3: the executor ranked by `timeout_min`, because no plan set `calib_s`; release
A2 stood at 788.2 s instead of 678.7 s in the V6 sim. The kern now gives each
station the median of its last green runs in the same mode on the same lane
class; the timeout stays the fallback.
"""
import json
import os
import pathlib
import sys
import unittest
from unittest import mock

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "scripts"))
sys.path.insert(0, str(REPO))

from scripts.pipeline import executor, gate, schedule  # noqa: E402
from scripts.tests import kit_time, store_isolation  # noqa: E402
from scripts.tests.test_pipeline_executor import station as ex_station  # noqa: E402
from scripts.tests import fake_ssh  # noqa: E402
from scripts.tests.test_pipeline_gate_cli import PASS_SSH_TEMPLATE, GateCase  # noqa: E402
from scripts.tests.test_pipeline_ir_path import IRExecutorCase, ir_plan, positions  # noqa: E402
from scripts.tests.test_pipeline_schedule import FakeLane, T0, ScheduleCase, ev, make_plan  # noqa: E402
from scripts.tests.test_pipeline_schedule import station as sched_station  # noqa: E402

NO_RUN = ["nextest", "run", "--no-run"]


def without_tests(argv):
    """`argv` without its `--test <name>` pairs: the selection that sets the features."""
    out, skip = [], False
    for a in argv:
        if skip:
            skip = False
        elif a == "--test":
            skip = True
        else:
            out.append(a)
    return out


def named_tests(argvs):
    return [a[i + 1] for a in argvs for i, x in enumerate(a) if x == "--test"]


class PhaseOne(IRExecutorCase):
    def test_phase_one_builds_exactly_the_browser_targets(self):
        for mode in ("integration", "release"):
            rows = ir_plan(self, mode)
            self.assertIn("build:browser", list(rows), mode)
            early, browser = rows["build:browser"], rows["browser:display"]
            locks = sorted(p.split("::", 1)[1] for p in browser["parts"])
            self.assertTrue(locks, mode)
            argvs = [c["argv"][1:] for c in early["cmds"]]
            self.assertEqual(locks, sorted(named_tests(argvs)), mode)
            self.assertFalse([a for a in argvs if a[:1] == ["build"]],
                             "phase 1 builds no meclaw binary (%s)" % mode)
            self.assertTrue(all(a[:3] == NO_RUN for a in argvs), argvs)
            self.assertEqual(["build:browser"], browser["after"], mode)
            self.assertIn("build:browser", rows["build"]["after"], mode)
            order = list(rows)
            self.assertLess(order.index("build:browser"), order.index("build"), mode)
            self.assertEqual(early["needs"], rows["build"]["needs"], mode)

    def test_phase_one_selects_like_the_main_build(self):
        # The plan: every phase-1 call is a phase-2 call with fewer `--test`s.
        for mode in ("integration", "release"):
            rows = ir_plan(self, mode)
            main = [without_tests(c["argv"]) for c in rows["build"]["cmds"]]
            for c in rows["build:browser"]["cmds"]:
                self.assertIn(without_tests(c["argv"]), main, mode)
        # The pure half, in each shape the main build can take.
        shapes = [
            ({"packages": ["a", "c1", "z"], "extra": [["c1", "L1"], ["c1", "L2"]]},
             [NO_RUN + ["-p", "a", "-p", "c1", "-p", "z", "--test", "L1", "--test", "L2"]]),
            ({"tests": [["c1", "t1"]], "extra": [["c1", "L1"]]},
             [NO_RUN + ["-p", "c1", "--test", "L1"]]),
            ({"workspace": True, "extra": [["c1", "L1"]]},
             [NO_RUN + ["--workspace", "--test", "L1"]]),
            # S6 groups (#1166, re-check tb-s6 "Kollision S4"): phase 2 builds c1
            # and c2 in ONE call, so phase 1 keeps that call's `-p` set -- a lone
            # `-p c1` were a third feature variant and every crate built twice.
            ({"tests": [["c1", "t1"], ["c2", "t2"]], "extra": [["c1", "L1"]],
              "test_targets": {"c1": ["t1", "L1"], "c2": ["t2"]}},
             [NO_RUN + ["-p", "c1", "-p", "c2", "--test", "L1"]]),
            # c2's test targets unknown: it builds alone in phase 2, c1 too.
            ({"tests": [["c1", "t1"], ["c2", "t2"]], "extra": [["c1", "L1"]],
              "test_targets": {"c1": ["t1", "L1"]}},
             [NO_RUN + ["-p", "c1", "--test", "L1"]]),
        ]
        for build, want in shapes:
            phase1 = executor.build_argvs(dict(build, restrict=build["extra"]), set())
            self.assertEqual(want, phase1, build)
            phase2 = [without_tests(a) for a in executor.build_argvs(build, set())]
            for a in phase1:
                self.assertIn(without_tests(a), phase2, build)
        # A cached lock is not built in phase 1 either.
        self.assertEqual([], executor.build_argvs(
            {"tests": [["c1", "t1"]], "extra": [["c1", "L1"]], "restrict": [["c1", "L1"]]},
            {"c1::L1"}))

    def test_no_lock_is_built_twice(self):
        # A binary of a crate the package call builds is not named once more.
        self.assertEqual([NO_RUN + ["-p", "c1"]],
                         executor.build_argvs({"packages": ["c1"], "extra": [["c1", "L1"]]},
                                              set()))
        for mode in ("integration", "release"):
            rows = ir_plan(self, mode)
            locks = [p.split("::", 1)[1] for p in rows["browser:display"]["parts"]]
            phase2 = [c["argv"][1:] for c in rows["build"]["cmds"]]
            pkgs = {a[i + 1] for a in phase2 if "--test" not in a
                    for i, x in enumerate(a) if x == "-p"}
            if "meclaw-cells" in pkgs or any("--workspace" in a for a in phase2):
                self.assertEqual([], [t for t in named_tests(phase2) if t in locks], mode)
            phase1 = [c["argv"][1:] for c in rows["build:browser"]["cmds"]]
            self.assertEqual(sorted(locks), sorted(named_tests(phase1)), mode)

    def test_phase_two_keeps_the_browser_binaries(self):
        # The sheet command of `browser:display` starts after phase 2 rewrote
        # binaries.json: its binaries stay in it.
        self.write_exe(self.bin / "cargo", FAKE_CARGO_BY_TEST)
        build = {"meclaw_bin": False, "workspace": False, "packages": [],
                 "tests": [["c1", "t1"]], "lib_bins": [], "extra": [["c2", "L1"]]}
        sts = [ex_station("build:browser", None, needs={"cpu": 4, "cargo": 1},
                          build=dict(build, restrict=[["c2", "L1"]])),
               ex_station("build", None, after=["build:browser"], needs={"cpu": 4, "cargo": 1},
                          when="any", build=dict(build, extra=[])),
               ex_station("browser:display", "true", after=["build:browser"], needs={"cpu": 4})]
        rc, evs = self.run_within(self.lane_dispatch(sts))
        self.assertEqual(0, rc, self.err_text)
        doc = json.loads((self.ws / "reuse" / "binaries.json").read_text())
        self.assertEqual({"c1::t1", "c2::L1"}, set(doc["rust-binaries"]))


# A fake `cargo` whose `nextest list` names the binaries of its `--test`s.
FAKE_CARGO_BY_TEST = """#!/bin/sh
printf 'cargo %s\\n' "$*" >> "$CALLS"
case "$1 $2" in
  "nextest list")
    ids="" prev="" crate=""
    for a in "$@"; do
      [ "$prev" = "-p" ] && crate="$a"
      if [ "$prev" = "--test" ]; then
        ids="$ids${ids:+, }\\"$crate::$a\\": {\\"binary-id\\": \\"$crate::$a\\", \\"binary-path\\": \\"/t/$a\\"}"
      fi
      prev="$a"
    done
    printf '{"rust-build-meta": {"target-directory": "/t", "base-output-directories": ["debug"], '
    printf '"non-test-binaries": {}, "build-script-out-dirs": {}, "linked-paths": []}, '
    printf '"rust-binaries": {%s}}\\n' "$ids" ;;
  "metadata --format-version") printf '{"packages": [], "workspace_root": "%s"}\\n' "$PWD" ;;
esac
exit 0
"""


class PrecheckBesideBrowser(IRExecutorCase):
    def test_red_precheck_stops_the_browser_started_beside_it(self):
        rows = ir_plan(self, "integration")
        self.assertNotIn("precheck", rows["browser:display"]["after"])
        self.assertNotIn("precheck", rows["build:browser"]["after"])
        self.fake_cargo()
        self.write_exe(self.bin / "node", "#!/bin/sh\nexit 0\n")   # the browser is not short
        (self.tree / "workshop" / "tools" / "node_modules" / "playwright").mkdir(parents=True)
        sts = [ex_station("precheck", "sleep 1.5; exit 1"),
               ex_station("build:browser", None, needs={"cpu": 4, "cargo": 1},
                          build={"tests": [], "extra": [["c1", "t1"]],
                                 "restrict": [["c1", "t1"]]}),
               # unstopped, the browser holds the run past the hang marker
               ex_station("browser:display", "sleep 600", after=["build:browser"],
                          needs={"cpu": 4})]
        rc, evs = self.run_within(self.lane_dispatch(sts), timeout=kit_time.HANG)
        self.assertEqual(1, rc, self.err_text)
        ended = self.ended(evs)
        pos = positions(evs)
        self.assertLess(pos[("station_started", "browser:display")],
                        pos[("station_ended", "precheck")], "the browser waited for precheck")
        self.assertEqual(("SKIP", "precheck red"),
                         (ended["browser:display"]["verdict"],
                          ended["browser:display"].get("reason")))
        pe = [e for e in evs if e["event"] == "pass_ended"][-1]["data"]
        self.assertEqual("RED", pe["verdict"])


class PhaseTwoAfterABreak(IRExecutorCase):
    """OR-TB.S4.20: phase 2 `build` hangs on phase 1 with `when: any`. The executor
    skips a station only after a RED/ABORTED predecessor, so a `success` edge would
    turn a broken phase 1 into a SKIP of `build` -- and `tests` would run on without
    a build. The plan lock (`test_pipeline_plan`) pins the field; these pin why."""

    def _verdicts(self, when):
        sts = [ex_station("build:browser", "exit 1"),
               ex_station("build", "exit 1", after=["build:browser"], when=when),
               ex_station("tests", "true", after=["build"])]
        _rc, evs = self.run_within(self.lane_dispatch(sts), timeout=kit_time.HANG)
        return {n: e["verdict"] for n, e in self.ended(evs).items()}

    def test_a_broken_phase_one_never_lets_a_consumer_run_without_a_build(self):
        got = self._verdicts("any")
        self.assertEqual(("RED", "RED", "SKIP"),
                         (got["build:browser"], got["build"], got["tests"]), got)

    def test_a_success_edge_would_let_the_tests_run_on_without_a_build(self):
        got = self._verdicts("success")
        self.assertEqual(("SKIP", "GREEN"), (got["build"], got["tests"]), got)


# --- A3: the order by measured durations -----------------------------------------------

class MeasuredDurations(ScheduleCase):
    def history(self, lane, mode, secs, verdict="GREEN"):
        plan = make_plan(*[sched_station(n, cpu=1, timeout=10) for n in secs])
        adm = self.admit(plan, mode=mode, lane_pref=lane, strand="welle-x/%s" % lane)
        self.assertIsInstance(adm, schedule.Admission)
        for name, s in secs.items():
            schedule.ingest(self.db, adm, ev("station_ended", {
                "station": name, "verdict": verdict, "secs": s, "exit": 0}, adm.run))
        pe = json.loads(ev("pass_ended", {"verdict": verdict, "secs": 50}, adm.run))
        schedule.close(self.db, adm, pe, now=T0 + 3600)

    def test_measured_durations_are_the_green_median_of_the_mode_on_the_lane_class(self):
        schedule.sync_runners(self.db, [FakeLane("build04", threads=24),
                                        FakeLane("build05", threads=24),
                                        FakeLane("build01", threads=12)],
                              states={"build04": "idle", "build05": "idle", "build01": "idle"})
        for s in (10.0, 30.0, 11.0):
            self.history("build04", "integration", {"a": s, "b": 100.0})
        self.history("build05", "integration", {"a": 12.0})             # same class
        self.history("build04", "integration", {"a": 999.0}, "RED")      # not green
        self.history("build04", "strand", {"a": 777.0})                  # other mode
        self.history("build01", "integration", {"a": 500.0, "c": 5.0})  # other class
        got = schedule.measured_secs(self.db, "meclaw", "integration", "build04", now=T0)
        self.assertEqual({"a": 11.5, "b": 100.0}, got)
        self.assertEqual({"a": 500.0, "c": 5.0},
                         schedule.measured_secs(self.db, "meclaw", "integration", "build01",
                                                now=T0))
        # Stale: beyond the window nothing is taken, the timeout stands.
        self.assertEqual({}, schedule.measured_secs(
            self.db, "meclaw", "integration", "build04",
            now=T0 + schedule.MEASURED_MAX_AGE_S + 7200))
        # An unknown lane has no class: no measurement.
        self.assertEqual({}, schedule.measured_secs(self.db, "meclaw", "integration", "nope",
                                                    now=T0))

    def test_a_cap_below_three_times_the_green_p95_is_raised(self):
        # TV-K5 (b): the cap of a station is at least 3x the P95 of its last 30 green
        # runs on the lane's class; a cap below that is a finding and is raised
        # (never lowered, never above 120 min). 20 runs of 60..1200 s: P95 = 1140 s.
        schedule.sync_runners(self.db, [FakeLane("build04", threads=24),
                                        FakeLane("build01", threads=12)],
                              states={"build04": "idle", "build01": "idle"})
        for k in range(1, 21):
            self.history("build04", "integration", {"a": 60.0 * k, "b": 1.0})
        self.history("build01", "integration", {"a": 99999.0})          # other class
        p95 = schedule.measured_p95(self.db, "meclaw", "integration", "build04")
        self.assertEqual({"a": 1140.0, "b": 1.0}, p95)
        sts = [{"name": "a", "timeout_min": 10}, {"name": "b", "timeout_min": 10},
               {"name": "c", "timeout_min": 10}]
        out, raised = schedule.with_hang_caps(sts, p95)
        self.assertEqual([57, 10, 10], [s["timeout_min"] for s in out])
        self.assertEqual([("a", 10, 57)], raised)
        self.assertEqual(10, sts[0]["timeout_min"])                      # input untouched
        self.assertEqual([], schedule.clamped_caps(out, p95))
        out, _ = schedule.with_hang_caps([{"name": "a", "timeout_min": 10}], {"a": 99999.0})
        self.assertEqual(120, out[0]["timeout_min"])
        # Re-check tv-p0 (B-Offen): a cap the 120-min clamp holds below 3x the P95
        # is a finding with its own NOTE line, never silent.
        self.assertEqual([("a", 120, 5000)], schedule.clamped_caps(out, {"a": 99999.0}))
        self.assertEqual(
            ["gate: NOTE cap a 10 -> 57 min (3x the P95 of its last 30 green runs, TV-K5)",
             "gate: NOTE cap a clamped at 120 min, below 3x the P95 of its last 30 green runs "
             "(5000 min) -- a finding (TV-K5)"],
            schedule.hang_cap_notes([("a", 10, 57)], [("a", 120, 5000)]))

    def lanes24(self):
        schedule.sync_runners(self.db, [FakeLane("build04", threads=24),
                                        FakeLane("build05", threads=24)],
                              states={"build04": "idle", "build05": "idle"})

    def test_a_measurement_is_finite_and_positive_or_none(self):
        # GH #1162 hA-m-1: NaN, inf, 0 or a negative duration measures nothing -- the
        # station stands as if it had no history (prior, else timeout).
        self.lanes24()
        self.history("build04", "integration", {"a": 10.0, "nan": float("nan"),
                                                "inf": float("inf"), "neg": -3.0, "zero": 0.0})
        self.history("build04", "integration", {"a": float("nan")})
        self.assertEqual({"a": 10.0}, schedule.measured_secs(
            self.db, "meclaw", "integration", "build04", now=T0))

    def test_a_row_that_is_no_json_object_is_skipped_not_fatal(self):
        # GH #1162 hA-m-4: a list or a string where an object belongs ended the whole
        # pass ABORTED (AttributeError); now the row measures nothing.
        self.lanes24()
        self.history("build04", "integration", {"a": 10.0, "b": 20.0, "c": 30.0})
        self.history("build05", "integration", {"d": 40.0})
        by_name = "SELECT id FROM items WHERE json_extract(doc, '$.station.name') = ?"
        self.db.execute("UPDATE task_runs SET results = '[1]' WHERE item IN (%s)" % by_name,
                        ("b",))
        self.db.execute("UPDATE items SET doc = '\"x\"' WHERE id IN (%s)" % by_name, ("c",))
        self.db.execute("UPDATE items SET doc = '[]' WHERE id IN (SELECT parent FROM items "
                        "WHERE json_extract(doc, '$.station.name') = ?)", ("d",))
        self.assertEqual({"a": 10.0}, schedule.measured_secs(
            self.db, "meclaw", "integration", "build04", now=T0))

    def test_rows_beyond_the_window_are_not_read(self):
        # GH #1162 hA-m-4: the window is a SQL filter, not a scan of every green run.
        self.lanes24()
        for s in (10.0, 11.0, 12.0):
            self.history("build04", "integration", {"a": s})
        with mock.patch.object(schedule.store, "parse_at",
                               wraps=schedule.store.parse_at) as parsed:
            self.assertEqual({}, schedule.measured_secs(
                self.db, "meclaw", "integration", "build04",
                now=T0 + schedule.MEASURED_MAX_AGE_S + 7200))
        self.assertEqual(0, parsed.call_count)
        self.assertEqual({"a": 11.0}, schedule.measured_secs(
            self.db, "meclaw", "integration", "build04", now=T0))


class MeasuredOrder(IRExecutorCase):
    def order(self, stations):
        rc, evs = self.run_within(self.lane_dispatch(stations))
        self.assertEqual(0, rc, self.err_text)
        return [e["data"]["station"] for e in evs if e["event"] == "station_started"]

    def test_executor_orders_by_measurement_with_data_else_by_timeout(self):
        # Two lane-wide stations, one at a time (24 + 24 > 24 x 1.5).
        def sts():
            return [ex_station("long_timeout", "sleep 0.2", needs={"cpu": 24}, timeout_min=60),
                    ex_station("short_timeout", "sleep 0.2", needs={"cpu": 24}, timeout_min=5)]
        self.assertEqual(["long_timeout", "short_timeout"], self.order(sts()))
        measured = schedule.with_measured(sts(), {"long_timeout": 1.0, "short_timeout": 100.0})
        self.assertEqual({"long_timeout": 1.0, "short_timeout": 100.0},
                         {s["name"]: s["calib_s"] for s in measured})
        self.assertEqual(["short_timeout", "long_timeout"], self.order(measured))
        # Without a measurement the station keeps its timeout.
        part = schedule.with_measured(sts(), {"short_timeout": 100.0})
        self.assertNotIn("calib_s", part[0])

    def test_a_station_without_a_measurement_ranks_by_its_prior_else_by_its_timeout(self):
        # GH #1162 OR-TB.S4.38: a declared prior (`prior_s`, plan.PRIOR_S) stands in for
        # the missing measurement; without one the timeout stays the fallback; a
        # measurement beats the prior.
        def sts(prior=None):
            long_ = ex_station("long_timeout", "sleep 0.2", needs={"cpu": 24}, timeout_min=60)
            if prior is not None:
                long_["prior_s"] = prior
            return [long_,
                    ex_station("short_timeout", "sleep 0.2", needs={"cpu": 24}, timeout_min=5)]
        self.assertEqual(["long_timeout", "short_timeout"], self.order(sts()))
        self.assertEqual(["short_timeout", "long_timeout"], self.order(sts(prior=1.0)))
        measured = schedule.with_measured(sts(prior=1.0), {"long_timeout": 1000.0})
        self.assertEqual(["long_timeout", "short_timeout"], self.order(measured))

    def test_the_reservation_estimate_is_the_measurement_else_the_prior_else_the_timeout(self):
        self.assertEqual(7.0, executor._estimate({"calib_s": 7.0, "prior_s": 273.3,
                                                  "timeout_min": 30}))
        self.assertEqual(273.3, executor._estimate({"prior_s": 273.3, "timeout_min": 30}))
        self.assertEqual(1800.0, executor._estimate({"timeout_min": 30}))
        self.assertIsNone(executor._estimate({}))

    BAD = (float("nan"), float("inf"), -1.0, 0.0, "x")

    def test_an_estimate_is_finite_and_positive_else_the_next_one_stands(self):
        # GH #1162 hA-m-1 + OR-TB.S4.38: a measurement or a prior that is no finite
        # positive number is as if it were not there.
        for bad in self.BAD:
            with self.subTest(bad=bad):
                self.assertEqual(273.3, executor._estimate({"calib_s": bad, "prior_s": 273.3,
                                                            "timeout_min": 30}))
                self.assertEqual(1800.0, executor._estimate({"calib_s": bad, "prior_s": bad,
                                                             "timeout_min": 30}))
                self.assertIsNone(executor._estimate({"calib_s": bad, "prior_s": bad}))

    def test_a_bad_measurement_or_prior_ranks_by_the_timeout(self):
        # -1 ranked the long station last, inf the short one first.
        for field in ("calib_s", "prior_s"):
            with self.subTest(field=field):
                long_ = ex_station("long_timeout", "sleep 0.2", needs={"cpu": 24},
                                   timeout_min=60)
                short = ex_station("short_timeout", "sleep 0.2", needs={"cpu": 24},
                                   timeout_min=5)
                long_[field], short[field] = -1.0, float("inf")
                self.assertEqual(["long_timeout", "short_timeout"], self.order([long_, short]))

    def test_with_measured_takes_only_a_finite_positive_measurement(self):
        sts = [ex_station(n, "true") for n in ("nan", "inf", "neg", "zero", "ok")]
        got = schedule.with_measured(sts, {"nan": float("nan"), "inf": float("inf"),
                                           "neg": -2.0, "zero": 0.0, "ok": 4.0})
        self.assertEqual({"ok": 4.0}, {s["name"]: s["calib_s"] for s in got if "calib_s" in s})


class MeasuredOnTheLane(GateCase):
    def setUp(self):
        super().setUp()
        fake_ssh.install(self.bin, PASS_SSH_TEMPLATE)
        self.lanes_file.write_text("lane-l  builder@lane-l.invalid  threads=4\n")
        os.environ["MECLAW_LANE_ROOT"] = str(self.lane_root)
        real = gate.lane.exec_stream
        self.sent = []

        def exec_stream(ln, dispatch, *a, **kw):
            self.sent.append(dispatch)
            dispatch = dict(dispatch, budget=dict(dispatch.get("budget") or {},
                                                  need_free_g=0, free_min_g=0, reserve_g=0))
            return real(ln, dispatch, *a, **kw)

        patcher = mock.patch.object(gate.lane, "exec_stream", side_effect=exec_stream)
        patcher.start()
        self.addCleanup(patcher.stop)

    def test_the_kern_sends_the_measured_durations(self):
        self.kern_plan()
        with mock.patch.object(gate.schedule, "measured_secs",
                               return_value={"clippy": 7.0}) as measured:
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(rc, 0, out + err)
        self.assertEqual(1, len(self.sent))
        args = measured.call_args
        self.assertEqual(("meclaw", "strand", "lane-l"), tuple(args[0][1:4]))
        calib = {s["name"]: s.get("calib_s") for s in self.sent[0]["stations"]}
        self.assertEqual({"clippy": 7.0, "claims": None}, calib)


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


if __name__ == "__main__":
    unittest.main()
