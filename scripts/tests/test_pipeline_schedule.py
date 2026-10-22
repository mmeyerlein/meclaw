"""Tests for `scripts/pipeline/schedule.py`: runners, admission, rounds, ingest.

Everything runs in-process against a throw-away store; lanes are dataclass
fakes with the attributes of `lane.Lane`, plans are hand-written dicts with
`needs`/`after`/`timeout_min` (the shape of `plan.build_plan`).
"""
import dataclasses
import json
import os
import re
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import types
import unittest
import unittest.mock
from typing import Optional

from scripts.pipeline import events, plan as plan_mod, schedule, store
from scripts.tests import kit_time, store_isolation

T0 = 1_800_000_000
BASE = "b" * 40
REV = "a" * 40


@dataclasses.dataclass
class FakeLane:
    name: str
    ssh: str = "builder@lane.example.invalid"
    bmc: Optional[str] = None
    root: str = "/srv/target"
    transport: str = "ssh"
    cls: str = "lane"
    threads: Optional[int] = None
    jobs: Optional[int] = None
    slots: Optional[int] = None
    cores: int = 0


def station(name, cpu=1, cargo=0, after=(), timeout=10, when=None, argv=None, **needs):
    st = {"name": name, "scope": name, "cargo": cargo, "stage": ["S", "I", "R"],
          "needs": dict({"cpu": cpu, "cargo": cargo}, **needs), "inputs": [],
          "cmds": [{"argv": argv or ["true", name], "cwd": ""}], "env": "none",
          "reason": [], "timeout_min": timeout, "after": list(after), "run": True,
          "keep_ws": True}
    if when:
        st["when"] = {"outcome": when}
    return st


def make_plan(*stations, base=BASE):
    return {"mode": "strand", "rev": REV, "base": base, "stations": list(stations),
            "plan_reason": {s["name"]: ["test"] for s in stations}}


def cargo_plan(base=BASE):
    return make_plan(
        station("precheck", argv=["sh", "-c", "exit 0"]),
        station("build", cpu=8, cargo=1, after=["precheck"], timeout=20),
        station("tests", cpu=8, after=["build"], timeout=30),
        station("export-audit", after=["precheck", "build", "tests"], when="any", timeout=5),
        base=base)


def light_plan(cpu=1, base=BASE):
    return make_plan(station("unittests", cpu=cpu, timeout=10), base=base)


def ev(event, data, run, kind="status"):
    data = dict(data, run=run)
    # W0a validates every journal line against V0 section 3; a hand-made line
    # carries the required fields this test does not look at as empty values.
    for field in events.EVENTS[event].required:
        data.setdefault(field, "" if field == "scope" else None)
    return json.dumps({"event": event, "kind": kind, "at": "2027-01-15T08:00:00Z",
                       "run": run, "data": data})


class ScheduleCase(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.db = store.open(os.path.join(self._tmp.name, "pipeline.db"))
        # A FakeLane's host does not exist: its lane tree holds no process. Without
        # this the lane tree probe of kit find (k) would ssh to .example.invalid,
        # get no answer and keep every stale lane lease (fail-closed).
        self.addCleanup(setattr, schedule, "_lane_tree_probe", schedule._lane_tree_probe)
        schedule._lane_tree_probe = lambda ln: []
        # nothing lives there, so a forced release has nothing to end (GH #1228)
        self.addCleanup(setattr, schedule, "_lane_tree_kill", getattr(schedule, "_lane_tree_kill", None))
        schedule._lane_tree_kill = lambda ln, pids: []
        # K14: the fresh browser measure of an admission refusal never reaches a lane
        # here (a FakeLane's host does not exist): no answer, unless a test says one
        self.addCleanup(setattr, schedule, "_browser_remeasure", schedule._browser_remeasure)
        schedule._browser_remeasure = lambda ln: None
        self.addCleanup(schedule._FRESH_BROWSERS.clear)
        schedule._FRESH_BROWSERS.clear()

    def tearDown(self):
        self.db.close()
        self._tmp.cleanup()

    def lanes(self, *names, state="idle", threads=24, **kw):
        lanes = [FakeLane(n, threads=threads, **kw) for n in names]
        schedule.sync_runners(self.db, lanes, states={n: state for n in names})
        return lanes

    def admit(self, plan, strand="welle-x/a", mode="strand", minutes=0, wave="welle-x", **kw):
        return schedule.admit(self.db, plan, repo="meclaw", mode=mode, strand=strand, wave=wave,
                              base=plan["base"], lane_pref=kw.pop("lane_pref", None),
                              now=T0 + 60 * minutes, wait=False, **kw)

    def warm(self, lane, gib=25):
        self.db.execute(
            "INSERT INTO workspaces (id, runner, kind, path, state, bytes, created_at, used_at) "
            "VALUES (?, ?, 'target', '/srv/target/target', 'warm', ?, ?, ?)",
            ("ws-%s" % lane, lane, gib * (1 << 30), "2027-01-15T07:00:00Z", "2027-01-15T07:00:00Z"))

    def item(self, item_id):
        return self.db.execute("SELECT * FROM items WHERE id = ?", (item_id,)).fetchone()

    def wp(self, adm):
        return self.db.execute("SELECT * FROM items WHERE key = ?", (adm.workpackage,)).fetchone()

    def runner(self, name):
        return self.db.execute("SELECT * FROM runners WHERE name = ?", (name,)).fetchone()

    def task_run(self, tr):
        return self.db.execute("SELECT * FROM task_runs WHERE id = ?", (tr,)).fetchone()

    def journal(self, event):
        return [json.loads(r["data"]) for r in self.db.execute(
            "SELECT data FROM journal WHERE event = ? ORDER BY seq", (event,))]

    def finish(self, adm, verdicts, pass_verdict="GREEN", lost=False):
        for name, verdict in verdicts.items():
            schedule.ingest(self.db, adm, ev("station_ended", {
                "station": name, "verdict": verdict, "secs": 12.5, "exit": 0}, adm.run))
        pe = json.loads(ev("pass_ended", {"verdict": pass_verdict, "secs": 50}, adm.run))
        schedule.close(self.db, adm, pe, lost=lost, now=T0 + 3600)


class TestRunners(ScheduleCase):
    def test_runner_colony_capacity_follows_the_real_cores(self):
        """welle-tb S5: one colony per real core (`cores=`), no fixed 8; the core's own
        fixed bookings (scenarios:memory, 8 colonies) stay admissible on a smaller lane."""
        widest = max(int(n.get("colony", 0)) for n in plan_mod.NEEDS.values())
        self.assertEqual(12, schedule.colony_capacity(12, 24))
        self.assertEqual(max(6, widest), schedule.colony_capacity(6, 12))
        self.assertEqual(max(16, widest), schedule.colony_capacity(0, 32))
        schedule.sync_runners(self.db, [FakeLane("b4", threads=24, cores=12)])
        self.assertEqual(12, json.loads(self.runner("b4")["capacity"])["colony"])

    def test_sync_runners_registers_capacity_class_and_state(self):
        schedule.sync_runners(self.db, [
            FakeLane("north", threads=24),
            FakeLane("mm", ssh="local", transport="local", cls="egon", threads=4)])
        north, mm = self.runner("north"), self.runner("mm")
        self.assertEqual("lane:north", north["party"])
        self.assertEqual("off", north["state"])
        self.assertEqual(300, north["boot_s"])
        self.assertEqual("lane", north["class"])
        cap = json.loads(north["capacity"])
        self.assertEqual(list(schedule.CAPACITY_KEYS), sorted(cap, key=schedule.CAPACITY_KEYS.index))
        self.assertEqual({"cpu": 24, "colony": schedule.colony_capacity(None, None),
                          "browser_chromium": 1, "browser_webkit": 1, "export": 1, "cargo": 1}, cap)
        self.assertEqual("egon", mm["class"])
        self.assertEqual("idle", mm["state"])
        # half the cores, never over the executor's Egon cap -- on any host (tb-s4
        # fix round: the pin read this host's count, build02 nproc 12 booked 6, ran 4)
        for n, want in ((4, 2), (6, 3), (8, 4), (12, 4), (24, 4)):
            with self.subTest(nproc=n), \
                    unittest.mock.patch.object(schedule.cpus, "threads", return_value=n):
                schedule.sync_runners(self.db, [
                    FakeLane("mm", ssh="local", transport="local", cls="egon", threads=4)])
                self.assertEqual(want, json.loads(self.runner("mm")["capacity"])["cpu"])

    def test_sync_runners_without_threads_or_fingerprint_does_not_assume_one_cpu(self):
        # GR-ii (pre-acceptance build03): a lane line without `threads=` and a
        # store without its fingerprint registered cpu 1, and the first run on
        # the lane ran every station one after another.
        schedule.sync_runners(self.db, [FakeLane("north")])
        self.assertEqual(schedule.cpus.threads(), json.loads(self.runner("north")["capacity"])["cpu"])

    def test_sync_runners_takes_vcpu_from_the_fingerprint_and_drains_lost_lanes(self):
        self.lanes("north", "south", threads=None)
        self.db.execute("UPDATE runners SET fingerprint = ? WHERE name = 'north'",
                        (json.dumps({"vcpu": 32}),))
        schedule.sync_runners(self.db, [FakeLane("north")])
        self.assertEqual(32, json.loads(self.runner("north")["capacity"])["cpu"])
        self.assertEqual("idle", self.runner("north")["state"], "a known state stays")
        self.assertEqual("drained", self.runner("south")["state"])


class TestAdmission(ScheduleCase):
    def test_a_lane_fallback_of_integration_or_release_is_a_note(self):
        # tb-s10-b P7 (m-RC-1 rest): with the widest lane unreachable or drained,
        # integration/release take a narrower one -- before, that showed only as
        # `lane=` in GATE-PLAN; the admission now carries its own NOTE line.
        schedule.sync_runners(self.db, [FakeLane("build01", threads=12),
                                        FakeLane("build04", threads=24),
                                        FakeLane("build05", threads=24)],
                              states={"build01": "idle", "build04": "unreachable",
                                      "build05": "drained"})
        adm = self.admit(cargo_plan(), mode="release", strand="welle-x/r")
        self.assertIsInstance(adm, schedule.Admission, getattr(adm, "reason", adm))
        self.assertEqual("build01", adm.lane)
        self.assertEqual(["lane-fallback release takes build01 (12 threads), widest "
                          "build04 (24 threads) is unreachable, build05 (24 threads) "
                          "is drained"], adm.notes)
        # no note: a strand pass, an explicit lane, the widest lane itself
        self.finish(adm, {"build": "GREEN", "tests": "GREEN"})
        strand = self.admit(cargo_plan(), strand="welle-x/s")
        self.assertEqual(("build01", []), (strand.lane, strand.notes))
        self.finish(strand, {"build": "GREEN", "tests": "GREEN"})
        pinned = self.admit(cargo_plan(), mode="integration", strand="welle-x/i",
                            lane_pref="build01")
        self.assertEqual(("build01", []), (pinned.lane, pinned.notes))
        self.finish(pinned, {"build": "GREEN", "tests": "GREEN"})
        schedule.sync_runners(self.db, [FakeLane("build06", threads=24)],
                              states={"build06": "idle"})
        wide = self.admit(cargo_plan(), mode="integration", strand="welle-x/j")
        self.assertEqual(("build06", []), (wide.lane, wide.notes))

    def test_schedule_prefers_warm_lane(self):
        self.lanes("north", "south")
        self.warm("south")
        adm = self.admit(cargo_plan())
        self.assertIsInstance(adm, schedule.Admission)
        self.assertEqual("south", adm.lane)

    def test_schedule_takes_the_first_free_lane_in_host_order(self):
        self.lanes("north", "south")
        self.assertEqual("north", self.admit(cargo_plan()).lane)
        self.assertEqual("south", self.admit(cargo_plan(), strand="welle-x/b").lane)

    def test_schedule_reserves_disk_and_evicts_first(self):
        self.lanes("north")
        self.warm("north", gib=25)
        adm = self.admit(cargo_plan())
        self.assertEqual(25, adm.expected_g)
        self.assertEqual(25, adm.dispatch["expected_g"])
        budget = adm.dispatch["budget"]
        for key in ("free_min_g", "reserve_g", "max_trees", "export_g", "ws_g", "bench_g", "pool_g"):
            self.assertIn(key, budget)
        # the executor evicts until this much is free, BEFORE the first build
        self.assertEqual(budget["reserve_g"] + 25, budget["need_free_g"])
        self.assertEqual([], adm.dispatch["live_runs"])
        # a second run on the same lane carries the first one's reservation
        second = self.admit(light_plan(), strand="welle-x/b")
        self.assertEqual("north", second.lane)
        self.assertEqual([adm.run], second.dispatch["live_runs"])
        self.assertEqual(25, second.dispatch["budget"]["reserved_live_g"])
        self.assertEqual(schedule.WS_EXPECTED_G, second.expected_g)
        self.assertEqual(budget["reserve_g"] + 25 + schedule.WS_EXPECTED_G,
                         second.dispatch["budget"]["need_free_g"])

    def test_schedule_uses_the_default_reservation_without_a_known_target(self):
        self.lanes("north")
        adm = self.admit(cargo_plan())
        self.assertEqual(schedule.DEFAULT_EXPECTED_G, adm.expected_g)

    def test_schedule_deadline_is_the_critical_path_of_the_timeouts(self):
        self.lanes("north")
        adm = self.admit(cargo_plan())
        # precheck 10 -> build 20 -> tests 30 -> export-audit 5 = 65 min, plus the
        # fixed overhead of push, lock wait and tree sync outside the stations
        over = schedule.DEADLINE_OVERHEAD_S
        self.assertGreaterEqual(over, 900, "the tree sync alone may take 900 s")
        self.assertEqual(schedule._iso(T0 + 65 * 60 + over), adm.deadline_at)
        self.assertEqual(65 * 60 + over + 1800, adm.ttl_s)
        self.assertEqual(adm.deadline_at, adm.dispatch["deadline_at"])

    def test_schedule_deadline_takes_the_executors_default_timeout(self):
        # A station without `timeout_min` runs up to 30 min in the executor; the
        # deadline must not be `now` then (review A M5).
        self.lanes("north")
        plan = light_plan()
        del plan["stations"][0]["timeout_min"]
        adm = self.admit(plan)
        self.assertEqual(schedule._iso(T0 + 30 * 60 + schedule.DEADLINE_OVERHEAD_S),
                         adm.deadline_at)

    def test_schedule_two_runs_one_lane_only_within_capacity(self):
        self.lanes("north", threads=24)
        a = self.admit(light_plan(cpu=8), strand="welle-x/a")
        self.assertEqual(24, a.capacity["cpu"])
        b = self.admit(light_plan(cpu=8), strand="welle-x/b")
        self.assertEqual("north", b.lane)
        self.assertEqual(16, b.capacity["cpu"], "static split: capacity minus what is held")
        c = self.admit(light_plan(cpu=16), strand="welle-x/c")
        self.assertIsInstance(c, schedule.Queued)
        self.assertIn("cpu", c.reason)
        self.assertEqual(3, c.code)
        self.finish(a, {"unittests": "GREEN"})
        # b is still live (before its deadline + grace), a is closed
        c = self.admit(light_plan(cpu=16), strand="welle-x/c", minutes=5)
        self.assertIsInstance(c, schedule.Admission)
        self.assertEqual(16, c.capacity["cpu"])

    def test_admission_books_the_selftest_shards_against_the_lane(self):
        # `CPU_SHARDS` (-2) counted as 0 in the demand.
        self.lanes("north", threads=24)
        with unittest.mock.patch.dict(os.environ, {plan_mod.SELFTEST_SHARDS_ENV: ""}):
            a = self.admit(make_plan(station("gate-selftest", cpu=plan_mod.CPU_SHARDS)),
                           strand="welle-x/a")
            self.assertEqual(24, a.capacity["cpu"])
            b = self.admit(light_plan(cpu=1), strand="welle-x/b")
        booked = plan_mod.resolve_cpu(plan_mod.CPU_SHARDS, 24)
        self.assertGreaterEqual(booked, 4)
        self.assertEqual(24 - booked, b.capacity["cpu"], "gate-selftest holds its shards")

    def test_admission_books_cpu_lane_against_the_lane(self):
        # W3 #1117: `CPU_LANE` (0) counted as 0 in the demand, so two heavy runs were
        # admitted side by side on one lane.
        self.lanes("north", threads=24)
        a = self.admit(make_plan(station("tests", cpu=plan_mod.CPU_LANE)), strand="welle-x/a")
        self.assertEqual("north", a.lane)
        b = self.admit(make_plan(station("clippy", cpu=plan_mod.CPU_LANE)), strand="welle-x/b")
        self.assertIsInstance(b, schedule.Queued, "the lane is fully booked by `tests`")
        self.assertIn("cpu", b.reason)

    def test_clippy_first_then_tests_queues_too(self):
        # the reverse order: a run that took the lane for clippy keeps `tests` out
        self.lanes("north", threads=24)
        a = self.admit(make_plan(station("clippy", cpu=plan_mod.CPU_LANE)), strand="welle-x/a")
        self.assertEqual("north", a.lane)
        b = self.admit(make_plan(station("tests", cpu=plan_mod.CPU_LANE)), strand="welle-x/b")
        self.assertIsInstance(b, schedule.Queued)
        self.assertIn("cpu", b.reason)
        c = self.admit(light_plan(cpu=1), strand="welle-x/c")
        self.assertIsInstance(c, schedule.Queued, "not even one thread is left")

    def test_a_four_thread_lane_books_four_for_cpu_lane(self):
        self.assertEqual(4, schedule._demand_on([station("build", cpu=plan_mod.CPU_LANE)], 4)["cpu"])
        self.lanes("box4", threads=4)
        self.admit(make_plan(station("build", cpu=plan_mod.CPU_LANE, cargo=1)), strand="welle-x/a")
        b = self.admit(light_plan(cpu=1), strand="welle-x/b")
        self.assertIsInstance(b, schedule.Queued)
        self.assertIn("cpu", b.reason)

    def test_the_egon_lane_never_books_more_than_four_on_eight_cores(self):
        # mm-os01: class egon, nproc 8, next to e28 -- its capacity is half the cores
        egon = FakeLane("mm-os01", cls="egon", threads=4, jobs=4)
        cap = schedule._runner_cpu(egon, {"vcpu": 8})
        self.assertEqual(4, cap)
        rows = [station(n, cpu=v.get("cpu", 1)) for n, v in plan_mod.NEEDS.items()]
        rows += [station("export-audit", cpu=v["cpu"]) for v in plan_mod.EXPORT_AUDIT_NEEDS.values()]
        for st in rows:
            with self.subTest(station=st["name"]):
                self.assertLessEqual(schedule._demand_on([st], cap)["cpu"], 4)
        self.assertEqual(4, schedule._demand_on(rows, cap)["cpu"])

    def test_a_cpu_that_is_no_thread_count_is_refused(self):
        # These booked 0 cpu and ran beside anything
        self.lanes("north", threads=24)
        bad = {"negative": -3, "none": None, "text": "4", "bool": True}
        for label, cpu in bad.items():
            with self.subTest(form=label):
                with self.assertRaises(schedule.ScheduleError) as cm:
                    self.admit(make_plan(station("unittests", cpu=cpu)), strand="welle-x/" + label)
                self.assertEqual(2, cm.exception.code)
                self.assertIn("needs.cpu", str(cm.exception))
        missing = station("unittests")
        del missing["needs"]["cpu"]
        with self.assertRaisesRegex(schedule.ScheduleError, "needs.cpu"):
            self.admit(make_plan(missing), strand="welle-x/missing")
        legacy = station("unittests")
        del legacy["needs"]
        with self.assertRaisesRegex(schedule.ScheduleError, "no needs"):
            self.admit(make_plan(legacy), strand="welle-x/legacy")
        ok = self.admit(light_plan(cpu=30), strand="welle-x/ok")
        self.assertEqual("north", ok.lane, "a plain count is capped at the lane")
        self.assertIsInstance(self.admit(light_plan(cpu=1), strand="welle-x/after"),
                              schedule.Queued)

    def test_schedule_never_two_runs_in_one_tree(self):
        self.lanes("north")
        a = self.admit(cargo_plan(), strand="welle-x/a")
        self.assertEqual("tree", a.tree)
        self.assertTrue(a.cargo)
        b = self.admit(cargo_plan(), strand="welle-x/b")
        self.assertIsInstance(b, schedule.Queued, "two cargo runs in the build path tree/")
        self.assertIn("tree", b.reason)
        c = self.admit(light_plan(), strand="welle-x/c")
        self.assertEqual("north", c.lane)
        self.assertEqual("wt/welle-x-c", c.tree)
        d = self.admit(light_plan(), strand="welle-x/d", tree_name="welle-x-c")
        self.assertIsInstance(d, schedule.Queued, "a push would move the tree under a live run")
        self.assertIn("wt/welle-x-c", d.reason)

    def test_schedule_a_live_workpackage_is_not_run_twice(self):
        self.lanes("north", "south")
        self.admit(light_plan())
        again = self.admit(light_plan())
        self.assertIsInstance(again, schedule.Queued)
        self.assertIn("live run", again.reason)

    def test_schedule_an_orphaned_active_run_expires_after_its_deadline(self):
        # Review A C1: the kern died between admit and close (Ctrl-C, a tool
        # timeout, an exception). The workpackage stays `active` -- until its
        # deadline plus the grace; then the next admission closes it as lost.
        self.lanes("north")
        adm = self.admit(cargo_plan())
        deadline = schedule._epoch(adm.deadline_at)
        before = (deadline - T0) // 60
        self.assertIsInstance(self.admit(cargo_plan(), minutes=before), schedule.Queued)
        other = self.admit(cargo_plan(), strand="welle-x/b", minutes=before)
        self.assertIsInstance(other, schedule.Queued, "tree/ is still the orphan's")
        after = (deadline + schedule.ORPHAN_GRACE_S - T0) // 60 + 1
        other = self.admit(cargo_plan(), strand="welle-x/b", minutes=after)
        self.assertIsInstance(other, schedule.Admission, "the orphan no longer holds tree/")
        self.assertEqual(("stopped", "lost"), (self.wp(adm)["state"], self.wp(adm)["reason"]))
        tr = self.task_run(adm.task_runs["build"])
        self.assertEqual(("failed", "lost", 1), (tr["state"], tr["reason"], tr["lost"]))
        again = self.admit(cargo_plan(), strand="welle-x/a", minutes=after)
        self.assertIsInstance(again, schedule.Queued, "now welle-x/b holds tree/")
        self.assertNotIn("live run", again.reason, "the orphan is no live run any more")

    def test_schedule_requests_power_when_no_lane_is_idle(self):
        self.lanes("north", "south", state="off")
        q = self.admit(cargo_plan())
        self.assertIsInstance(q, schedule.Queued)
        self.assertEqual("north", q.power_requested)
        rows = self.journal("lane_power_requested")
        self.assertEqual(1, len(rows))
        self.assertEqual("north", rows[0]["lane"])
        self.assertEqual(300, rows[0]["eta_s"])
        self.assertTrue(rows[0]["reason"])
        # it boots from the request on (T0 lies ahead of the clock that wrote
        # updated_at; without this the lane was long past its boot time)
        self.db.execute("UPDATE runners SET state = 'booting', updated_at = ? WHERE name = 'north'",
                        (schedule._iso(T0),))
        q = self.admit(cargo_plan())
        self.assertIsNone(q.power_requested, "a booting lane is not asked for twice")
        self.assertIn("booting", q.reason)
        self.assertEqual(1, len(self.journal("lane_power_requested")))

    @unittest.mock.patch.dict(os.environ, {"OMP_NUM_THREADS": "8"})
    @unittest.mock.patch.object(schedule.cpus, "threads", return_value=8)
    def test_schedule_egon_lane_only_for_test_and_measure(self, _threads):
        # mm-os01 (nproc 8) patched: the egon width is half the cores, never over 4, so
        # the host's own count would decide the pin (tb-s4 fix round, red at nproc 6)
        schedule.sync_runners(self.db, [FakeLane("mm", ssh="local", transport="local", cls="egon")])
        q = self.admit(light_plan(), mode="strand")
        self.assertIsInstance(q, schedule.Queued)
        self.assertIsNone(q.power_requested)
        self.assertIn("egon", q.reason)
        adm = self.admit(light_plan(), mode="test")
        self.assertEqual("mm", adm.lane)
        self.assertTrue(adm.dispatch["+nice"])
        self.assertEqual("4", adm.dispatch["+env"]["NEXTEST_TEST_THREADS"])
        self.assertEqual("4", adm.dispatch["+env"]["CARGO_BUILD_JOBS"])

    def test_schedule_unknown_lane_pref_is_an_error(self):
        self.lanes("north")
        with self.assertRaises(schedule.ScheduleError) as cm:
            self.admit(light_plan(), lane_pref="nowhere")
        self.assertEqual(2, cm.exception.code)
        self.assertEqual("north", self.admit(light_plan(), lane_pref="north").lane)

    def test_schedule_held_lease_wins_and_admission_takes_a_free_one(self):
        lanes_file = os.path.join(self._tmp.name, "lanes")
        with open(lanes_file, "w") as fh:
            fh.write("north builder@north.example.invalid\nsouth builder@south.example.invalid\n")
        self.lanes("north", "south")
        self.assertEqual(0, schedule.lease_init(self.db, lanes="north,south", lanes_file=lanes_file,
                                                now=T0).code)
        res = schedule.lease_take(self.db, "welle-x/a", lane="south", now=T0)
        self.assertEqual((0, "south"), (res.code, res.lane))
        self.warm("north")
        adm = self.admit(cargo_plan(), strand="welle-x/a")
        self.assertEqual("south", adm.lane, "the held lease beats the warm tree")
        self.assertFalse(adm.lease_taken)
        lease = self.db.execute("SELECT * FROM leases WHERE resource = 'lane:south'").fetchone()
        self.assertEqual(adm.run, lease["run"])
        self.assertGreaterEqual(schedule._epoch(lease["taken_at"]) + lease["ttl_s"], T0 + adm.ttl_s)
        other = self.admit(cargo_plan(), strand="welle-x/b")
        self.assertEqual("north", other.lane)
        self.assertTrue(other.lease_taken)
        self.assertEqual("welle-x/b", self.db.execute(
            "SELECT holder FROM leases WHERE resource = 'lane:north'").fetchone()["holder"])
        third = self.admit(cargo_plan(), strand="welle-x/c")
        self.assertIsInstance(third, schedule.Queued)
        self.assertIn("#1 in the queue", third.reason)
        self.finish(other, {"precheck": "GREEN", "build": "GREEN", "tests": "GREEN",
                            "export-audit": "GREEN"})
        self.assertIsNone(self.db.execute(
            "SELECT holder FROM leases WHERE resource = 'lane:north'").fetchone(),
            "a lease the admission took goes with the run")


class TestLeaseKitFinds(ScheduleCase):
    """Kit finds of 09.10. (Z-A2): a live run's lease, the lane `take` picks, a second holder."""

    def arm(self, *names):
        lanes_file = os.path.join(self._tmp.name, "lanes")
        with open(lanes_file, "w") as fh:
            fh.write("".join("%s builder@%s.example.invalid\n" % (n, n) for n in names))
        self.lanes(*names)
        self.assertEqual(0, schedule.lease_init(self.db, lanes=",".join(names),
                                                lanes_file=lanes_file, now=T0).code)

    def test_gh1190_an_armed_pool_admits_only_its_own_lanes(self):
        # GH #1190: with the pool armed, `_warm_first` walked every registered
        # runner and the lease check only looked at pool lanes -- a host-file
        # lane kept OUT of the pool was admitted without a lease, warm-first
        # even preferred it (seen twice on 2026-10-10).
        self.arm("north")
        self.lanes("north", "west")
        self.warm("west")
        adm = self.admit(cargo_plan(), strand="welle-x/a")
        self.assertEqual("north", adm.lane, "a lane outside the armed pool is no default")
        self.assertTrue(adm.lease_taken)
        second = self.admit(cargo_plan(), strand="welle-x/b")
        self.assertIsInstance(second, schedule.Queued, "the pool is full: queue, never west")
        # an explicit --host still takes the lane outside the pool
        third = self.admit(cargo_plan(), strand="welle-x/c", lane_pref="west")
        self.assertEqual("west", third.lane)

    def test_gh1190_the_widest_lane_is_the_widest_in_the_pool(self):
        # Mini-delta review (Important): `widest` still counted every runner --
        # a wider lane outside the pool sent every pool lane to `narrow_out`
        # and integration was refused for good ("waits for the widest lane
        # (48 threads: none)").
        self.arm("north")
        schedule.sync_runners(self.db, [FakeLane("north", threads=24), FakeLane("west", threads=48)],
                              states={"north": "idle", "west": "idle"})
        adm = self.admit(cargo_plan(), strand="welle-x/a", mode="integration")
        self.assertEqual("north", getattr(adm, "lane", None), adm)

    def test_release_refuses_the_lease_of_a_live_run(self):
        # (c): a second run of the same strand released the token while the
        # first run still built on the lane (workpackage active).
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a", lane="north", now=T0).code)
        adm = self.admit(cargo_plan(), strand="welle-x/a")
        self.assertEqual("north", adm.lane)
        res = schedule.lease_release(self.db, "welle-x/a", now=T0 + 60, branch="welle-x/a")
        self.assertEqual(3, res.code, res.err)
        self.assertIn("live run %s" % adm.run, res.err)
        self.assertEqual(["welle-x/a"], [r["holder"] for r in self.db.execute(
            "SELECT holder FROM leases")])
        res = schedule.lease_release(self.db, "welle-x/a", now=T0 + 60, branch="welle-x/a",
                                     force=True)
        self.assertEqual(0, res.code, res.err)
        self.assertEqual([], self.db.execute("SELECT holder FROM leases").fetchall())
        # Review Z-A2 R3: the journal names the forced release as such.
        self.assertEqual("release --force", self.journal("lease_released")[-1]["verb"])

    def test_the_own_holder_release_is_no_release_by(self):
        # Review Z-A2 R2 M5: `welle-x/a:sat` released from the strand's own tree.
        self.arm("north", "south")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a:sat", lane="south", now=T0).code)
        res = schedule.lease_release(self.db, "welle-x/a:sat", now=T0 + 60, branch="welle-x/a")
        self.assertEqual(0, res.code, res.err)
        self.assertEqual("release", self.journal("lease_released")[-1]["verb"])
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a:sat", lane="south", now=T0).code)
        schedule.lease_release(self.db, "welle-x/a:sat", now=T0 + 60, branch="welle-x/b")
        self.assertEqual("release-by", self.journal("lease_released")[-1]["verb"])

    def test_the_admission_close_of_a_holder_lease_is_no_release_by(self):
        # Re-check Z-A M-a: the admission's close sets the branch to the holder
        # key `<strand>:<n>` itself (schedule.py close path), `_by` compared
        # only the strand part and journalled `release-by`.
        ctx = types.SimpleNamespace(branch="welle-x/a:sat")
        self.assertEqual("", schedule._by(ctx, "welle-x/a:sat"))
        ctx.branch = "welle-x/a"
        self.assertEqual("", schedule._by(ctx, "welle-x/a:sat"))
        ctx.branch = "welle-x/b:sat"
        self.assertIn("released by", schedule._by(ctx, "welle-x/a:sat"))

    def test_an_empty_holder_is_refused(self):
        # Review Z-A2 R2 M4: `--holder ""` acted on the main lease, silently.
        self.arm("north")
        env = {"MECLAW_PIPELINE_DB": os.path.join(self._tmp.name, "pipeline.db"),
               "MECLAW_LANES_FILE": os.path.join(self._tmp.name, "lanes"),
               "MECLAW_STRAND_TOKENS": os.path.join(self._tmp.name, "tok")}
        for verb in ("take", "release", "lane"):
            code, _out, err = schedule.run_lanes([verb, "--strand", "welle-x/a", "--holder", ""],
                                                 env=env, cwd=self._tmp.name)
            self.assertEqual(2, code, (verb, err))
            self.assertIn("--holder takes a short suffix", err)

    def test_take_without_a_lane_picks_like_the_admission(self):
        # (a): `take --wait` without --lane took the first lane of `init`
        # (build02) although the admission would pick the warm one.
        self.arm("north", "south")
        self.warm("south")
        res = schedule.lease_take(self.db, "welle-x/a", now=T0)
        self.assertEqual((0, "south"), (res.code, res.lane))
        res = schedule.lease_take(self.db, "welle-x/b", now=T0)
        self.assertEqual((0, "north"), (res.code, res.lane))

    def test_admit_waiting_blocks_until_the_lease_ends_without_a_clock(self):
        # GH #1145: `gate --wait-admission` -- the admission waits on the pool's
        # wake file (a lease end wakes it), never on a retry clock: the round's
        # watchdog is 300 s here, the release must wake it at once.
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/b", lane="north", now=T0).code)
        path = os.path.join(self._tmp.name, "pipeline.db")
        got, said = [], []

        def wait():
            db = store.open(path)
            try:
                got.append(schedule.admit_waiting(
                    db, cargo_plan(), repo="meclaw", mode="strand", strand="welle-x/a",
                    wave="welle-x", base=BASE, lane_pref=None, now=None, secs=300,
                    on_queued=said.append))
            finally:
                db.close()
        th = threading.Thread(target=wait, daemon=True)
        th.start()
        kit_time.until("the admission queues", lambda: said, state=lambda: got)
        self.assertEqual(1, len(said), "the queue line once")
        self.assertIn("gate --wait-admission blocks here", said[0].reason)
        self.assertTrue(th.is_alive(), "admit_waiting returned while the lane was leased")
        res = schedule.run_lanes(["release", "--strand", "welle-x/b"],
                                 env={"MECLAW_PIPELINE_DB": path,
                                      "MECLAW_LANES_FILE": os.path.join(self._tmp.name, "lanes"),
                                      "MECLAW_STRAND_TOKENS": os.path.join(self._tmp.name, "tok")},
                                 cwd=self._tmp.name)
        self.assertEqual(0, res[0], res)
        # Only the release can end the wait before its 300-s watchdog.
        th.join(kit_time.HANG)
        self.assertFalse(th.is_alive(), "HANG: the release did not wake the admission after "
                         "%.0f s" % kit_time.HANG)
        self.assertEqual("north", got[0].lane)
        self.assertTrue(got[0].lease_taken)

    def test_a_second_holder_of_one_strand_is_admitted_on_another_lane(self):
        # GH #1146: the admission of `welle-x/a:sat` is its own lease and its
        # own workpackage -- the core run of `welle-x/a` does not queue it.
        self.arm("north", "south")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a", lane="north", now=T0).code)
        core = self.admit(cargo_plan(), strand="welle-x/a")
        sat = self.admit(cargo_plan(), strand="welle-x/a:sat")
        self.assertEqual(("north", "south"), (core.lane, sat.lane))
        self.assertTrue(sat.lease_taken)

    def test_a_pass_without_cargo_goes_to_its_workspace_tree_on_the_lane(self):
        # Review Z-A R2 M3: a satellite's pass (no cargo) admitted on a lane
        # runs in `wt/<name>`, never in the shared `tree`.
        self.lanes("north")
        adm = self.admit(light_plan(), strand="welle-x/a", lane_pref="north", tree_name="soul2me")
        self.assertEqual(("north", "wt/soul2me", False), (adm.lane, adm.tree, adm.cargo))

    def test_release_of_a_dead_pass_frees_the_lease_after_deadline_and_grace(self):
        # Review Z-A2 R3 M4 (GH #1155 (4)): a pass that never closed holds its
        # lease until deadline + ORPHAN_GRACE_S; from then on `release` frees it
        # without --force.
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a", lane="north", now=T0).code)
        adm = self.admit(cargo_plan(), strand="welle-x/a")
        dead = schedule._epoch(adm.deadline_at) + schedule.ORPHAN_GRACE_S
        res = schedule.lease_release(self.db, "welle-x/a", now=dead - 1, branch="welle-x/a")
        self.assertEqual(3, res.code, res.err)
        res = schedule.lease_release(self.db, "welle-x/a", now=dead, branch="welle-x/a")
        self.assertEqual(0, res.code, res.err)
        self.assertEqual([], self.db.execute("SELECT holder FROM leases").fetchall())
        self.assertEqual("release", self.journal("lease_released")[-1]["verb"])

    def test_release_after_the_close_frees_the_lease(self):
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a", lane="north", now=T0).code)
        adm = self.admit(cargo_plan(), strand="welle-x/a")
        self.finish(adm, {"precheck": "GREEN", "build": "GREEN", "tests": "GREEN",
                          "export-audit": "GREEN"})
        res = schedule.lease_release(self.db, "welle-x/a", now=T0 + 4000, branch="welle-x/a")
        self.assertEqual(0, res.code, res.err)


class TestLeaseKitFindsWave(TestLeaseKitFinds):
    """Kit finds of welle-testing 2026-10-11 (R-TV-W21): head-of-line, idle lease."""

    def queue(self):
        return [(r["holder"], r["want"]) for r in self.db.execute(
            "SELECT holder, want FROM lease_entries WHERE resource IS NULL ORDER BY seq")]

    def test_a_host_gate_in_the_queue_keeps_its_lane_wish(self):
        # Fund 21:11: a gate with `--host north` queued with `want` empty;
        # `_lanes_left` handed it the first lane to come free (south), it never
        # took it, and every waiter behind it got nothing -- south stood idle.
        self.arm("north", "south")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/x", lane="north", now=T0).code)
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/y", lane="south", now=T0).code)
        queued = self.admit(cargo_plan(), strand="welle-x/b", lane_pref="north")
        self.assertIsInstance(queued, schedule.Queued)
        self.assertEqual([("welle-x/b", "north")], self.queue(), "the queue entry names the --host lane")
        self.assertEqual(0, schedule.lease_release(self.db, "welle-x/y", now=T0 + 60,
                                                   branch="welle-x/y").code)
        nxt = self.admit(cargo_plan(), strand="welle-x/c", minutes=2)
        self.assertEqual("south", getattr(nxt, "lane", None), nxt)

    def test_a_host_gate_queues_for_its_lane_while_another_is_free(self):
        # The same gate, south free: it waits for north with a queue place
        # (FIFO for north), and south stays free for the next strand.
        self.arm("north", "south")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/x", lane="north", now=T0).code)
        self.assertIsInstance(self.admit(cargo_plan(), strand="welle-x/b", lane_pref="north"),
                              schedule.Queued)
        self.assertEqual([("welle-x/b", "north")], self.queue())
        nxt = self.admit(cargo_plan(), strand="welle-x/c", minutes=1)
        self.assertEqual("south", getattr(nxt, "lane", None), nxt)

    def test_an_old_queue_entry_without_its_wish_learns_it_on_the_next_round(self):
        # A queue place written before the fix (want empty) heals itself: the
        # gate's next admission round writes its --host lane into the entry.
        self.arm("north", "south")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/x", lane="north", now=T0).code)
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/y", lane="south", now=T0).code)
        self.assertEqual(3, schedule.lease_take(self.db, "welle-x/b", now=T0).code)
        self.assertEqual([("welle-x/b", None)], self.queue())
        self.admit(cargo_plan(), strand="welle-x/b", lane_pref="north", minutes=1)
        self.assertEqual([("welle-x/b", "north")], self.queue())

    def test_an_idle_lease_goes_back_to_a_waiter_after_the_short_ttl(self):
        # Fund 03:1x: a lease without a run and without a live holder (the
        # builder ended without `token release`) kept build03 101 min while a
        # waiter for it stood behind -- the hold TTL (90 min) was the only way out.
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a", lane="north", now=T0).code)
        idle = schedule.LEASE_IDLE_MIN * 60
        res = schedule.lease_take(self.db, "welle-x/b", lane="north", now=T0 + idle)
        self.assertEqual(3, res.code, "within the short TTL the lease stays: %s" % res.err)
        res = schedule.lease_take(self.db, "welle-x/b", lane="north", now=T0 + idle + 60)
        self.assertEqual(0, res.code, res.err)
        self.assertIn("reclaimed the cargo token of welle-x/a", res.err)
        self.assertIn("no run and no live process", res.err)

    def test_an_idle_lease_goes_back_on_the_first_call_of_a_gate(self):
        # The caller's own wish counts: a `gate --host north` (or one for any
        # lane) reclaims the idle lease on its first round, unqueued.
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a", lane="north", now=T0).code)
        adm = self.admit(cargo_plan(), strand="welle-x/b", lane_pref="north",
                         minutes=schedule.LEASE_IDLE_MIN + 1)
        self.assertEqual("north", getattr(adm, "lane", None), adm)

    def test_an_idle_lease_taken_back_for_the_caller_names_the_caller(self):
        # V2-P1196-2: a `token take` for any lane with an empty queue took back
        # the idle lease (the caller's own wish counts, R-TV-W21 b) and said
        # "with a waiter for it" -- there was none in the queue.
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a", lane="north", now=T0).code)
        later = T0 + (schedule.LEASE_IDLE_MIN + 1) * 60
        res = schedule.lease_take(self.db, "welle-x/c", now=later)
        self.assertEqual(0, res.code, res.err)
        self.assertEqual({"welle-x/c": "north"}, self.holders())
        self.assertIn("reclaimed the cargo token of welle-x/a", res.err)
        self.assertIn("wanted by the caller welle-x/c, no waiter in the queue", res.err)
        self.assertNotIn("with a waiter", res.err)

    def test_an_idle_lease_taken_back_for_a_queued_waiter_names_the_waiter(self):
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a", lane="north", now=T0).code)
        self.assertEqual(3, schedule.lease_take(self.db, "welle-x/b", lane="north", now=T0 + 60).code)
        later = T0 + (schedule.LEASE_IDLE_MIN + 1) * 60
        res = schedule.lease_take(self.db, "welle-x/c", now=later)
        self.assertIn("reclaimed the cargo token of welle-x/a", res.err)
        self.assertIn("with a waiter for it (welle-x/b)", res.err)
        self.assertNotIn("wanted by the caller", res.err)

    def _gate_on_held_lease(self, run_min):
        """welle-x/a holds north, its gate runs `run_min` minutes on the lease and
        closes; returns the close time. The admission refreshes `seen` once."""
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a", lane="north", now=T0).code)
        adm = self.admit(cargo_plan(), strand="welle-x/a", minutes=1)
        self.assertEqual("north", getattr(adm, "lane", None), adm)
        self.assertFalse(adm.lease_taken, "the run rides the held lease")
        end = T0 + 60 + run_min * 60
        pe = json.loads(ev("pass_ended", {"verdict": "RED", "secs": run_min * 60}, adm.run))
        schedule.close(self.db, adm, pe, now=end)
        return end

    def test_k9_the_end_of_a_long_run_is_a_sign_of_life_of_its_lease(self):
        # K9 (welle-validierung-2, 14.10. 17:06): the 16-min gate of v2-p1082 on
        # its held build02 lease ended 17:06:10Z; three seconds later a take of
        # v2-p1078 reclaimed the lease as "idle 16 min ... no run" -- `seen`
        # dated from the admission, the run's end did not count.
        end = self._gate_on_held_lease(run_min=16)
        res = schedule.lease_take(self.db, "welle-x/b", lane="north", now=end + 3)
        self.assertEqual(3, res.code, res.err)
        self.assertNotIn("reclaimed", res.err)
        self.assertEqual({"welle-x/a": "north"}, self.holders())

    def test_k9_the_idle_reason_after_a_long_run_counts_from_its_end(self):
        # The reason of a later reclaim names the minutes since the run ended,
        # not since the admission (it said "idle 16 min" 3 s after the run).
        end = self._gate_on_held_lease(run_min=16)
        later = end + (schedule.LEASE_IDLE_MIN + 1) * 60
        res = schedule.lease_take(self.db, "welle-x/b", lane="north", now=later)
        self.assertEqual(0, res.code, res.err)
        self.assertIn("reclaimed the cargo token of welle-x/a", res.err)
        self.assertIn("idle %d min (over %d) wanted by the caller welle-x/b, no waiter in the "
                      "queue, no run and no live process"
                      % (schedule.LEASE_IDLE_MIN + 1, schedule.LEASE_IDLE_MIN), res.err)

    def test_v2e2_who_counts_the_last_station_of_a_pass_that_ended(self):
        # GH #1199 V2-E-2 (welle-validierung-2, 15.10. 03:06): after a 35-min
        # gate on its held lease, `token who` said "no progress since take" --
        # the stations were read only through the lease's `run`, which the end
        # of the pass clears. The last station of the ended pass still counts.
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a", lane="north", now=T0).code)
        adm = self.admit(cargo_plan(), strand="welle-x/a", minutes=1)
        self.assertFalse(adm.lease_taken, "the run rides the held lease")
        store.journal_add(self.db, schedule._iso(T0 + 600), None, adm.run, "status", "station_ended",
                          "GATE tests", {"station": "tests", "verdict": "RED", "secs": 500, "exit": 1})
        pe = json.loads(ev("pass_ended", {"verdict": "RED", "secs": 600}, adm.run))
        schedule.close(self.db, adm, pe, now=T0 + 660)
        out = schedule.lease_who(self.db, now=T0 + 900).out
        line = next(ln for ln in out.splitlines() if ln.startswith("  welle-x/a "))
        self.assertIn("progress 5 min ago", line, out)

    def test_the_callers_own_idle_lease_is_never_reclaimed(self):
        # The holder's own call proves it alive, a waiter for its lane or not.
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a", lane="north", now=T0).code)
        self.assertEqual(3, schedule.lease_take(self.db, "welle-x/b", lane="north", now=T0 + 60).code)
        later = T0 + (schedule.LEASE_IDLE_MIN + 5) * 60
        res = schedule.lease_take(self.db, "welle-x/a", lane="north", now=later)
        self.assertEqual(0, res.code, res.err)
        self.assertIn("already held by welle-x/a", res.err)

    def test_a_lease_taken_with_an_explicit_ttl_is_held_on_purpose(self):
        # `take --ttl` holds the lane on purpose (lanes CLI
        # test_take_with_a_ttl_holds_longer): no idle reclaim, its hold TTL rules.
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a", lane="north", ttl=180,
                                                now=T0).code)
        later = T0 + (schedule.LEASE_IDLE_MIN + 5) * 60
        self.assertEqual(3, schedule.lease_take(self.db, "welle-x/b", lane="north", now=later).code)
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/b", lane="north",
                                                now=T0 + 181 * 60).code)

    def test_a_held_lease_survives_a_rewrite_by_an_older_core(self):
        # Re-Check tv-kit H2: an older core's `_save` rewrites `lease_entries`
        # with its fixed column list, `hold_min` comes back NULL, and the lease
        # held on purpose went back after 15 idle minutes. Mixed cores share one
        # store (PREAMBLE section 5); the UPDATE is what such a rewrite leaves.
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a", lane="north", ttl=180,
                                                now=T0).code)
        self.db.execute("UPDATE lease_entries SET hold_min = NULL")
        later = T0 + (schedule.LEASE_IDLE_MIN + 5) * 60
        res = schedule.lease_take(self.db, "welle-x/b", lane="north", now=later)
        self.assertEqual(3, res.code, res.err)
        self.assertEqual("north", self.holders().get("welle-x/a"))

    def test_an_old_store_without_hold_min_gains_it_and_keeps_its_holders(self):
        # Re-Check tv-kit H3: a store from before the column (no `hold_min`)
        # gets it on the next call; both holders stay.
        self.arm("north", "south")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a", lane="north", now=T0).code)
        self.db.execute("ALTER TABLE lease_entries DROP COLUMN hold_min")
        res = schedule.lease_take(self.db, "welle-x/b", lane="south", ttl=120, now=T0 + 60)
        self.assertEqual(0, res.code, res.err)
        self.assertIn("hold_min", {r[1] for r in self.db.execute("PRAGMA table_info(lease_entries)")})
        self.assertEqual({"welle-x/a": "north", "welle-x/b": "south"}, self.holders())

    def test_an_idle_lease_nobody_waits_for_keeps_its_hold_ttl(self):
        self.arm("north", "south")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a", lane="north", now=T0).code)
        later = T0 + (schedule.LEASE_IDLE_MIN + 5) * 60
        res = schedule.lease_take(self.db, "welle-x/b", lane="south", now=later)
        self.assertEqual(0, res.code, res.err)
        self.assertEqual({"welle-x/a", "welle-x/b"}, {r["holder"] for r in self.db.execute(
            "SELECT holder FROM leases")}, "a waiter for another lane reclaims nothing")

    def holders(self):
        return {r["holder"]: r["resource"][len("lane:"):] for r in self.db.execute(
            "SELECT holder, resource FROM lease_entries WHERE resource IS NOT NULL")}

    def test_a_take_without_a_lane_reclaims_nothing_while_a_lane_is_free(self):
        # Re-Check tv-kit M1: `_wanted` counted a caller without a wish as demand,
        # so every `token take` without `--lane` took back the idle warm lane of a
        # builder between two test-tier runs although south stood free.
        self.arm("north", "south")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a", lane="north", now=T0).code)
        later = T0 + (schedule.LEASE_IDLE_MIN + 1) * 60
        res = schedule.lease_take(self.db, "welle-x/c", now=later)
        self.assertEqual(0, res.code, res.err)
        self.assertNotIn("reclaimed", res.err)
        self.assertEqual({"welle-x/a": "north", "welle-x/c": "south"}, self.holders())

    def test_an_admission_without_a_host_reclaims_nothing_while_a_lane_is_free(self):
        self.arm("north", "south")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a", lane="north", now=T0).code)
        adm = self.admit(cargo_plan(), strand="welle-x/c", minutes=schedule.LEASE_IDLE_MIN + 1)
        self.assertEqual("south", getattr(adm, "lane", None), adm)
        self.assertEqual("north", self.holders().get("welle-x/a"), "the idle warm lane stays")

    def test_a_waiter_with_a_free_lane_offered_reclaims_nothing(self):
        # A queued waiter without a wish that already has south left for it is
        # no demand on north either.
        self.arm("north", "south")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a", lane="north", now=T0).code)
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/x", lane="south", now=T0).code)
        self.assertEqual(3, schedule.lease_take(self.db, "welle-x/w", now=T0 + 60).code)
        self.assertEqual(0, schedule.lease_release(self.db, "welle-x/x", now=T0 + 120,
                                                   branch="welle-x/x").code)
        later = T0 + (schedule.LEASE_IDLE_MIN + 1) * 60
        res = schedule.lease_take(self.db, "welle-x/d", lane="south", now=later)
        self.assertEqual(3, res.code, res.err)
        self.assertNotIn("reclaimed", res.err)
        self.assertEqual("north", self.holders().get("welle-x/a"))

    def test_a_take_without_a_lane_reclaims_when_no_lane_is_free(self):
        # The other side of M1: with every lane held, a wish for any lane is demand.
        self.arm("north", "south")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a", lane="north", now=T0).code)
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/x", lane="south", ttl=180,
                                                now=T0).code)
        later = T0 + (schedule.LEASE_IDLE_MIN + 1) * 60
        res = schedule.lease_take(self.db, "welle-x/c", now=later)
        self.assertEqual(0, res.code, res.err)
        self.assertIn("reclaimed the cargo token of welle-x/a", res.err)
        self.assertEqual({"welle-x/c": "north", "welle-x/x": "south"}, self.holders())

    def test_an_idle_lease_is_never_reclaimed_under_a_live_process(self):
        # fail-closed: a registered live pid (`token check --pid`) keeps it.
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a", lane="north", now=T0).code)
        self.assertEqual(0, schedule.lease_check(self.db, "welle-x/a", pid=os.getpid(),
                                                 caller=1, now=T0).code)
        later = T0 + (schedule.LEASE_IDLE_MIN + 5) * 60
        self.assertEqual(3, schedule.lease_take(self.db, "welle-x/b", lane="north", now=later).code)

    def test_an_idle_lease_is_never_reclaimed_under_a_live_run(self):
        # fail-closed: the holder's admission is still active on the lane.
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a", lane="north", now=T0).code)
        adm = self.admit(cargo_plan(), strand="welle-x/a")
        self.assertEqual("north", adm.lane)
        later = T0 + (schedule.LEASE_IDLE_MIN + 5) * 60
        self.assertEqual(3, schedule.lease_take(self.db, "welle-x/b", lane="north", now=later).code)


class TestStaleButBusy(TestLeaseKitFinds):
    """Kit find (k) of welle-testing 2026-10-11 (15:3x, R-TV-W43): a builder ran
    cargo-mutants by hand over ssh in the lane tree of build04 under its lease;
    the lease watcher saw no runner run (pid -), reclaimed the lease as stale and
    gave build04 to another strand, whose `pre` built the mutated files. Before
    a stale lane lease moves, the lane tree is probed for live processes; a live
    one (or a probe without an answer) keeps the lease and notes `stale-but-busy`."""

    def setUp(self):
        super().setUp()
        self.probed = []
        self.answer = []

        def probe(ln):
            self.probed.append((ln.name, ln.root))
            return self.answer
        self.addCleanup(setattr, schedule, "_lane_tree_probe", schedule._lane_tree_probe)
        schedule._lane_tree_probe = probe

    def idle_lease(self):
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a", lane="north", now=T0).code)
        return T0 + (schedule.LEASE_IDLE_MIN + 1) * 60

    def busy_notes(self):
        return [d for d in self.journal("lease_noted") if d["verb"] == "stale-but-busy"]

    def holders(self):
        return {r["holder"]: r["resource"][len("lane:"):] for r in self.db.execute(
            "SELECT holder, resource FROM lease_entries WHERE resource IS NOT NULL")}

    def test_a_live_process_in_the_lane_tree_keeps_the_stale_lease(self):
        later = self.idle_lease()
        self.answer = [4242]
        res = schedule.lease_take(self.db, "welle-x/b", lane="north", now=later)
        self.assertEqual(3, res.code, res.err)
        self.assertNotIn("reclaimed", res.err)
        self.assertIn("stale-but-busy", res.err)
        self.assertIn("4242", res.err)
        self.assertEqual({"welle-x/a": "north"}, self.holders())
        self.assertEqual([("north", "/srv/target")], self.probed, "the probe looks at the lane root")
        notes = self.busy_notes()
        self.assertEqual(1, len(notes), notes)
        self.assertEqual("welle-x/a", notes[0]["holder"])
        self.assertEqual("lane:north", notes[0]["resource"])
        self.assertIn("4242", notes[0]["detail"])

    def test_a_probe_without_an_answer_keeps_the_stale_lease(self):
        # fail-closed: an unreachable lane may still build.
        later = self.idle_lease()
        self.answer = None
        res = schedule.lease_take(self.db, "welle-x/b", lane="north", now=later)
        self.assertEqual(3, res.code, res.err)
        self.assertEqual({"welle-x/a": "north"}, self.holders())
        self.assertIn("no answer", self.busy_notes()[0]["detail"])

    def test_an_empty_lane_tree_lets_the_stale_lease_go(self):
        later = self.idle_lease()
        res = schedule.lease_take(self.db, "welle-x/b", lane="north", now=later)
        self.assertEqual(0, res.code, res.err)
        self.assertIn("reclaimed the cargo token of welle-x/a", res.err)
        self.assertEqual({"welle-x/b": "north"}, self.holders())
        self.assertEqual([], self.busy_notes())

    def test_a_busy_lane_tree_is_noted_once_while_it_stays_busy(self):
        later = self.idle_lease()
        self.answer = [4242]
        for n in range(3):
            self.assertEqual(3, schedule.lease_take(self.db, "welle-x/b", lane="north",
                                                    now=later + 60 * n).code)
        self.assertEqual(1, len(self.busy_notes()), "one note per busy spell, not per call")
        self.answer = []
        res = schedule.lease_take(self.db, "welle-x/b", lane="north", now=later + 300)
        self.assertEqual(0, res.code, res.err)

    def test_an_admission_does_not_take_over_a_busy_lane_tree(self):
        self.idle_lease()
        self.answer = [4242]
        adm = self.admit(cargo_plan(), strand="welle-x/b", lane_pref="north",
                         minutes=schedule.LEASE_IDLE_MIN + 1)
        self.assertIsInstance(adm, schedule.Queued, adm)
        self.assertEqual({"welle-x/a": "north"}, self.holders())
        self.assertEqual(1, len(self.busy_notes()))
        self.answer = []
        adm = self.admit(cargo_plan(), strand="welle-x/b", lane_pref="north",
                         minutes=schedule.LEASE_IDLE_MIN + 2)
        self.assertEqual("north", getattr(adm, "lane", None), adm)

    def test_the_real_probe_finds_a_hand_run_in_a_local_lane_tree(self):
        # The probe of `lane.py` itself (no fake): a process whose cwd is in
        # the lane tree -- the hand-run of the find -- keeps the lease.
        schedule._lane_tree_probe = None
        root = os.path.join(self._tmp.name, "lane-root")
        tree = os.path.join(root, "tree")
        os.makedirs(tree)
        schedule.sync_runners(self.db, [FakeLane("north", ssh="local", transport="local",
                                                 root=root, threads=24)],
                              states={"north": "idle"})
        lanes_file = os.path.join(self._tmp.name, "lanes")
        with open(lanes_file, "w") as fh:
            fh.write("north local %s\n" % root)
        self.assertEqual(0, schedule.lease_init(self.db, lanes="north", lanes_file=lanes_file,
                                                now=T0).code)
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a", lane="north", now=T0).code)
        later = T0 + (schedule.LEASE_IDLE_MIN + 1) * 60
        hand = subprocess.Popen([sys.executable, "-c", "import sys; sys.stdin.read()"],
                                cwd=tree, stdin=subprocess.PIPE)
        try:
            res = schedule.lease_take(self.db, "welle-x/b", lane="north", now=later)
            self.assertEqual(3, res.code, res.err)
            self.assertIn(str(hand.pid), res.err)
        finally:
            hand.stdin.close()
            hand.wait()
        res = schedule.lease_take(self.db, "welle-x/b", lane="north", now=later + 60)
        self.assertEqual(0, res.code, res.err)

    # Re-check fix10 M1: the hand way out freed a live hand run without --force.
    def busy_spell(self, answer):
        later = self.idle_lease()
        self.answer = answer
        res = schedule.lease_take(self.db, "welle-x/b", lane="north", now=later)
        self.assertEqual(3, res.code, res.err)
        return later, res

    def test_a_live_pid_names_no_hand_release(self):
        # The pid case never needs a hand release: the next take moves the
        # lease once the tree is idle. The old hint sent the next agent to
        # 'token release --strand A' while 4242 still mutated the tree.
        _, res = self.busy_spell([4242])
        self.assertIn("moves by itself at the next take once the tree is idle", res.err)
        self.assertNotIn("'token release --strand", res.err)

    def test_a_probe_without_an_answer_names_the_hand_check(self):
        _, res = self.busy_spell(None)
        self.assertIn("check the lane by hand, then 'token release --force --strand welle-x/a'",
                      res.err)

    def test_release_of_a_stale_but_busy_lease_needs_force(self):
        later, _ = self.busy_spell([4242])
        res = schedule.lease_release(self.db, "welle-x/a", now=later + 60, branch="welle-x/b")
        self.assertEqual(3, res.code, res.err)
        self.assertIn("4242", res.err)
        self.assertIn(schedule._iso(later), res.err, "the release names when the probe saw it")
        self.assertEqual({"welle-x/a": "north"}, self.holders())
        self.assertEqual([], self.journal("lease_released"))
        res = schedule.lease_release(self.db, "welle-x/a", now=later + 120, branch="welle-x/b",
                                     force=True)
        self.assertEqual(0, res.code, res.err)
        self.assertEqual({}, self.holders())
        self.assertEqual(["release-by --force"],
                         [d["verb"] for d in self.journal("lease_released")])

    def test_own_release_after_a_probe_without_an_answer_needs_force(self):
        later, _ = self.busy_spell(None)
        res = schedule.lease_release(self.db, "welle-x/a", now=later + 60, branch="welle-x/a")
        self.assertEqual(3, res.code, res.err)
        self.assertIn("no answer", res.err)
        res = schedule.lease_release(self.db, "welle-x/a", now=later + 60, branch="welle-x/a",
                                     force=True)
        self.assertEqual(0, res.code, res.err)
        self.assertEqual(["release --force"], [d["verb"] for d in self.journal("lease_released")])

    def test_a_forced_release_over_the_note_warns_and_journals_why(self):
        # re-check tv-kit fix11 H2: `--force` printed the plain release line and
        # journaled `held 17 min` -- not to be told from a force over a run.
        later, _ = self.busy_spell([4242])
        res = schedule.lease_release(self.db, "welle-x/a", now=later + 60, branch="welle-x/b",
                                     force=True)
        self.assertEqual(0, res.code, res.err)
        self.assertIn("warning: released welle-x/a over stale-but-busy", res.err)
        self.assertIn("4242", res.err)
        (rel,) = self.journal("lease_released")
        self.assertIn("over stale-but-busy since %s" % schedule._iso(later), rel["detail"])
        self.assertIn("pid 4242 live in the lane tree", rel["detail"])

    # Re-check fix12 M1 (decision k3 narrowed): `seen` says the holder is back,
    # not that the hand run left the tree -- a check/take/admission after the
    # note lifted the lock and the next release freed the lane blindly under
    # pid 4242 (probes P1/P2/P5). Now nothing lifts the note: a release probes
    # the lane tree while a note since the lease's take speaks.
    def test_a_take_of_the_holder_after_the_note_keeps_the_lock_over_a_live_pid(self):
        later, _ = self.busy_spell([4242])
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a", lane="north",
                                                now=later + 60).code)
        self.probed.clear()
        res = schedule.lease_release(self.db, "welle-x/a", now=later + 120, branch="welle-x/a")
        self.assertEqual(3, res.code, res.err)
        self.assertIn("pid 4242 live in the lane tree", res.err)
        self.assertIn(schedule._iso(later + 120), res.err, "the release names when it probed")
        self.assertNotIn("ends the spell", res.err)
        self.assertEqual([("north", "/srv/target")], self.probed, "the release probes the tree")
        self.assertEqual({"welle-x/a": "north"}, self.holders())
        self.assertEqual([], self.journal("lease_released"))

    def test_a_check_of_the_holder_after_the_note_keeps_the_lock_without_an_answer(self):
        later, _ = self.busy_spell(None)
        self.assertEqual(0, schedule.lease_check(self.db, "welle-x/a", caller=1,
                                                 now=later + 60).code)
        res = schedule.lease_release(self.db, "welle-x/a", now=later + 120, branch="welle-x/a")
        self.assertEqual(3, res.code, res.err)
        self.assertIn("no answer", res.err)
        self.assertIn(schedule._iso(later + 120), res.err)
        self.assertIn("check the lane by hand, then release --force", res.err)
        self.assertEqual({"welle-x/a": "north"}, self.holders())

    def test_a_strangers_check_lifts_nothing(self):
        # P2/P5: `check` takes any holder name from any branch; it set `seen`
        # and the stranger's `release-by` freed the lane under the hand run.
        later, _ = self.busy_spell([4242])
        self.assertEqual(0, schedule.lease_check(self.db, "welle-x/a", caller=1,
                                                 now=later + 60, branch="welle-x/b").code)
        res = schedule.lease_release(self.db, "welle-x/a", now=later + 120, branch="welle-x/b")
        self.assertEqual(3, res.code, res.err)
        self.assertIn("4242", res.err)
        self.assertEqual({"welle-x/a": "north"}, self.holders())
        self.assertEqual([], self.journal("lease_released"))

    def test_an_admission_of_the_holder_after_the_note_keeps_the_lock(self):
        self.busy_spell([4242])
        self.answer = []
        adm = self.admit(cargo_plan(), strand="welle-x/a", lane_pref="north",
                         minutes=schedule.LEASE_IDLE_MIN + 2)
        self.assertEqual("north", getattr(adm, "lane", None), adm)
        self.finish(adm, {})
        self.answer = [4242]
        res = schedule.lease_release(self.db, "welle-x/a", now=T0 + 3660, branch="welle-x/a")
        self.assertEqual(3, res.code, res.err)
        self.assertEqual({"welle-x/a": "north"}, self.holders())

    def test_a_release_over_the_note_frees_the_lane_once_the_tree_probes_empty(self):
        # the narrowed k3: no --force needed, no 15 min wait, once the tree is idle
        later, _ = self.busy_spell([4242])
        self.answer = []
        res = schedule.lease_release(self.db, "welle-x/a", now=later + 60, branch="welle-x/b")
        self.assertEqual(0, res.code, res.err)
        self.assertEqual({}, self.holders())
        (rel,) = self.journal("lease_released")
        self.assertEqual("release-by", rel["verb"])
        self.assertIn("tree probed empty", rel["detail"])
        self.assertNotIn("warning", res.err)

    def test_a_forced_release_over_a_live_pid_after_the_holders_take_is_journaled(self):
        # --force frees and journals as before -- also after the holder's return
        later, _ = self.busy_spell([4242])
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a", lane="north",
                                                now=later + 60).code)
        self.probed.clear()
        res = schedule.lease_release(self.db, "welle-x/a", now=later + 120, branch="welle-x/a",
                                     force=True)
        self.assertEqual(0, res.code, res.err)
        # GH #1228 (OR-V2.v2-r3-lease.1): --force probes the tree to end what
        # lives there -- the hand's word frees the lease, the kit ends the run
        self.assertEqual([("north", "/srv/target")], self.probed)
        self.assertIn("ended pid 4242 in the lane tree of lane north", res.err)
        self.assertIn("warning: released welle-x/a over stale-but-busy", res.err)
        (rel,) = self.journal("lease_released")
        self.assertEqual("release --force", rel["verb"])
        self.assertIn("over stale-but-busy since %s" % schedule._iso(later), rel["detail"])

    def test_a_check_in_the_notes_own_second_does_not_end_the_spell(self):
        # re-check fix12 H2: `seen == at` keeps the spell (fail-closed, no
        # second note for the same hand run); `>=` in `_busy_spell` noted anew.
        later, _ = self.busy_spell([4242])
        self.assertEqual(0, schedule.lease_check(self.db, "welle-x/a", caller=1,
                                                 now=later).code)
        again = later + (schedule.LEASE_IDLE_MIN + 1) * 60
        self.assertEqual(3, schedule.lease_take(self.db, "welle-x/b", lane="north",
                                                now=again).code)
        self.assertEqual(1, len(self.busy_notes()), self.busy_notes())

    def test_a_busy_spell_after_the_holders_return_is_noted_again(self):
        # one note per busy spell: the holder's return ends a spell, so the
        # next probe that finds the same pid notes it anew (the release names
        # the fresh spell); the lock itself never hung on it (fix12 M1).
        later, _ = self.busy_spell([4242])
        self.assertEqual(0, schedule.lease_check(self.db, "welle-x/a", caller=1,
                                                 now=later + 60).code)
        again = later + 60 + (schedule.LEASE_IDLE_MIN + 1) * 60
        res = schedule.lease_take(self.db, "welle-x/b", lane="north", now=again)
        self.assertEqual(3, res.code, res.err)
        self.assertEqual(2, len(self.busy_notes()), self.busy_notes())
        res = schedule.lease_release(self.db, "welle-x/a", now=again + 60, branch="welle-x/b")
        self.assertEqual(3, res.code, res.err)
        self.assertEqual({"welle-x/a": "north"}, self.holders())

    def test_the_note_still_locks_without_a_sign_of_the_holder(self):
        # no check, no take since the note: the own release probes, finds
        # 4242, stays exit 3; --force frees it (the stranger's side is pinned above)
        later, _ = self.busy_spell([4242])
        res = schedule.lease_release(self.db, "welle-x/a", now=later + 600, branch="welle-x/a")
        self.assertEqual(3, res.code, res.err)
        res = schedule.lease_release(self.db, "welle-x/a", now=later + 660, branch="welle-x/a",
                                     force=True)
        self.assertEqual(0, res.code, res.err)
        self.assertEqual(["release --force"], [d["verb"] for d in self.journal("lease_released")])

    # Re-check fix10 M2: a lane without a runner row was reclaimed unprobed.
    def arm_without_runner_row(self):
        lanes_file = os.path.join(self._tmp.name, "lanes")
        with open(lanes_file, "w") as fh:
            fh.write("north builder@north.example.invalid\n")
        self.assertEqual(0, schedule.lease_init(self.db, lanes="north", lanes_file=lanes_file,
                                                now=T0).code)
        self.assertIsNone(self.db.execute("SELECT 1 FROM runners WHERE name = 'north'").fetchone())
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a", lane="north", now=T0).code)
        return T0 + (schedule.LEASE_IDLE_MIN + 1) * 60

    def kept_without_runner_row(self, answer):
        later = self.arm_without_runner_row()
        self.answer = answer
        res = schedule.lease_take(self.db, "welle-x/b", lane="north", now=later)
        self.assertEqual(3, res.code, res.err)
        self.assertEqual({"welle-x/a": "north"}, self.holders())
        self.assertEqual(["north"], [n for n, _ in self.probed], "the hook is asked first")

    def test_a_lane_without_a_runner_row_and_a_live_pid_keeps_the_lease(self):
        self.kept_without_runner_row([4242])

    def test_a_lane_without_a_runner_row_and_no_answer_keeps_the_lease(self):
        self.kept_without_runner_row(None)

    def test_production_probes_a_lane_without_a_runner_row_from_the_host_file(self):
        schedule._lane_tree_probe = None
        later = self.arm_without_runner_row()
        root = os.path.join(self._tmp.name, "lane-root")
        tree = os.path.join(root, "tree")
        os.makedirs(tree)
        hosts = os.path.join(self._tmp.name, "lanes-local")
        with open(hosts, "w") as fh:
            fh.write("north local %s\n" % root)
        hand = subprocess.Popen([sys.executable, "-c", "import sys; sys.stdin.read()"],
                                cwd=tree, stdin=subprocess.PIPE)
        try:
            with unittest.mock.patch.dict(os.environ, {"MECLAW_LANES_FILE": hosts}):
                res = schedule.lease_take(self.db, "welle-x/b", lane="north", now=later)
            self.assertEqual(3, res.code, res.err)
            self.assertIn(str(hand.pid), res.err)
        finally:
            hand.stdin.close()
            hand.wait()
        self.assertEqual({"welle-x/a": "north"}, self.holders())

    def test_production_keeps_the_lease_of_a_lane_known_nowhere(self):
        schedule._lane_tree_probe = None
        later = self.arm_without_runner_row()
        empty = os.path.join(self._tmp.name, "lanes-empty")
        open(empty, "w").close()
        with unittest.mock.patch.dict(os.environ, {"MECLAW_LANES_FILE": empty}):
            res = schedule.lease_take(self.db, "welle-x/b", lane="north", now=later)
        self.assertEqual(3, res.code, res.err)
        self.assertEqual({"welle-x/a": "north"}, self.holders())
        self.assertIn("no answer", self.busy_notes()[0]["detail"])

    def test_a_host_file_that_is_no_utf8_keeps_the_lease(self):
        # re-check tv-kit fix11 H3: a UnicodeDecodeError (a ValueError, no
        # OSError) left `_lane_tree_procs` and broke the verb before its
        # transaction; it is no answer like any unreadable host file.
        schedule._lane_tree_probe = None
        later = self.arm_without_runner_row()
        broken = os.path.join(self._tmp.name, "lanes-broken")
        with open(broken, "wb") as fh:
            fh.write(b"north builder@north.example.invalid /srv/\xff\xfe\n")
        with unittest.mock.patch.dict(os.environ, {"MECLAW_LANES_FILE": broken}):
            res = schedule.lease_take(self.db, "welle-x/b", lane="north", now=later)
        self.assertEqual(3, res.code, res.err)
        self.assertEqual({"welle-x/a": "north"}, self.holders())
        self.assertIn("no answer", self.busy_notes()[0]["detail"])

    # Re-check fix10 M3: the fail-closed paths of k1, one pin each.
    def test_an_invalid_lane_root_keeps_the_stale_lease(self):
        schedule._lane_tree_probe = None
        self.arm("north")
        schedule.sync_runners(self.db, [FakeLane("north", root="/", threads=24)],
                              states={"north": "idle"})
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a", lane="north", now=T0).code)
        later = T0 + (schedule.LEASE_IDLE_MIN + 1) * 60
        res = schedule.lease_take(self.db, "welle-x/b", lane="north", now=later)
        self.assertEqual(3, res.code, res.err)
        self.assertEqual({"welle-x/a": "north"}, self.holders())
        self.assertIn("no answer", self.busy_notes()[0]["detail"])

    def test_a_lease_stale_after_the_probe_stays_unprobed(self):
        # A lease that turns stale between the probe and `BEGIN IMMEDIATE`
        # finds no entry in `ctx.lane_procs` and stays (the next call probes it).
        later = self.idle_lease()
        with unittest.mock.patch.object(schedule, "_probe_stale_lanes", lambda *a, **k: {}):
            res = schedule.lease_take(self.db, "welle-x/b", lane="north", now=later)
        self.assertEqual(3, res.code, res.err)
        self.assertEqual({"welle-x/a": "north"}, self.holders())
        self.assertEqual([], self.probed)
        self.assertNotIn("reclaimed", res.err)

    def test_a_merge_of_the_holder_after_the_note_keeps_the_lock(self):
        # re-check tv-kit fix13 M1 (probe Q1): `strand.sh merge` journals
        # lease_taken/lease_released of `tree:master@<repo>` under the strand's own
        # name; the note read the holder's last lease line over every resource,
        # so the merge line hid it and the release freed the lane unprobed.
        from scripts.pipeline import merge
        later, _ = self.busy_spell([4242])
        at = schedule._iso(later + 60)
        self.assertTrue(merge.take_tree(self.db, "meclaw-core", "welle-x/a", at=at))
        self.assertTrue(merge.release_tree(self.db, "meclaw-core", "welle-x/a", at=at))
        self.probed.clear()
        res = schedule.lease_release(self.db, "welle-x/a", now=later + 120, branch="welle-x/a")
        self.assertEqual(3, res.code, res.err)
        self.assertIn("pid 4242 live in the lane tree", res.err)
        self.assertEqual([("north", "/srv/target")], self.probed, "the release probes the tree")
        self.assertEqual({"welle-x/a": "north"}, self.holders())
        self.assertEqual([merge.resource("meclaw-core")],
                         [d["resource"] for d in self.journal("lease_released")])

    def test_a_unit_gone_note_of_the_holder_keeps_the_lock(self):
        # the class of fix13 M1: gate.py `_note_unit_gone` writes a lease_noted
        # under the strand's name with the bare lane name as resource, and the
        # kit's own `refused`/`skip`/`wait` lines carry no resource -- each hid
        # the note the same way the merge line did.
        later, _ = self.busy_spell([4242])
        for n, (resource, verb) in enumerate([("north", "unit_gone"), (None, "refused")]):
            events.emit(self.db, "lease_noted", "note",
                        {"resource": resource, "holder": "welle-x/a", "run": "r1",
                         "ttl_s": None, "verb": verb, "detail": "-"},
                        at=schedule._iso(later + 60 + n))
            res = schedule.lease_release(self.db, "welle-x/a", now=later + 120 + n,
                                         branch="welle-x/a")
            self.assertEqual(3, res.code, (verb, res.err))
            self.assertIn("pid 4242 live in the lane tree", res.err)
            self.assertEqual({"welle-x/a": "north"}, self.holders())

    def test_a_probe_that_raises_keeps_the_lock_as_no_answer(self):
        # re-check tv-kit fix13 H1 (probe Q2): an OSError of the probe went
        # through `lease_release` as a traceback (exit 1, no line); it is no answer.
        later, _ = self.busy_spell([4242])
        with unittest.mock.patch.object(schedule, "_lane_tree_procs",
                                        side_effect=OSError("boom")):
            res = schedule.lease_release(self.db, "welle-x/a", now=later + 120,
                                         branch="welle-x/a")
        self.assertEqual(3, res.code, res.err)
        self.assertIn("the lane tree probe got no answer", res.err)
        # re-check tv-kit fix14 H2: the exception leaves a trace -- a lasting
        # programming error of the probe no longer reads like a network blip.
        self.assertIn("got no answer (OSError: boom)", res.err)
        self.assertEqual({"welle-x/a": "north"}, self.holders())

    def test_a_take_probe_that_raises_keeps_the_stale_lease(self):
        # the same pattern in `_probe_stale_lanes` (fix13 H1): no traceback,
        # the stale lease stays as stale-but-busy without an answer.
        later = self.idle_lease()
        with unittest.mock.patch.object(schedule, "_lane_tree_procs",
                                        side_effect=OSError("boom")):
            res = schedule.lease_take(self.db, "welle-x/b", lane="north", now=later)
        self.assertEqual(3, res.code, res.err)
        self.assertEqual({"welle-x/a": "north"}, self.holders())
        self.assertIn("no answer (OSError: boom)", self.busy_notes()[0]["detail"])

    def test_a_note_after_the_release_probe_asks_for_a_second_release(self):
        # re-check tv-kit fix13 H2: a note between `_release_probe` and
        # `BEGIN IMMEDIATE` has no probe yet; "no answer ... --force" sent the
        # hand to --force although a second release would probe.
        later, _ = self.busy_spell([4242])
        with unittest.mock.patch.object(schedule, "_release_probe", lambda *a, **k: ({}, {})):
            res = schedule.lease_release(self.db, "welle-x/a", now=later + 120,
                                         branch="welle-x/a")
        self.assertEqual(3, res.code, res.err)
        self.assertIn("the note came after the probe -- release again", res.err)
        self.assertNotIn("--force", res.err)
        # re-check tv-kit fix14 H1: no probe ran, so the line says when the note
        # was seen, never "probed at".
        self.assertIn("noted after the probe at ", res.err)
        self.assertNotIn("probed at", res.err)
        self.assertEqual({"welle-x/a": "north"}, self.holders())


class TestReleaseEndsTheLaneRun(ScheduleCase):
    """GH #1228: a lane run outlived its released lease -- a test run on build03
    went on after its agent was stopped and the lease freed (15.10. 21:36Z), a
    `strand.sh test` on build01 ran eight minutes while `token who` showed the
    lane free. The end of a lane lease now waits for the lane process tree: a
    release finds a live process there (or a live process the holder entered)
    and keeps the lease (exit 3); `release --force` ends the tree, then frees.
    The run here is a real process in a real local lane tree (no fake probe)."""

    def setUp(self):
        super().setUp()
        schedule._lane_tree_probe = None
        self.addCleanup(setattr, schedule, "_lane_tree_kill", schedule._lane_tree_kill)
        schedule._lane_tree_kill = None
        root = os.path.join(self._tmp.name, "lane-root")
        self.tree = os.path.join(root, "tree")
        os.makedirs(self.tree)
        schedule.sync_runners(self.db, [FakeLane("north", ssh="local", transport="local",
                                                 root=root, threads=24)],
                              states={"north": "idle"})
        lanes_file = os.path.join(self._tmp.name, "lanes")
        with open(lanes_file, "w") as fh:
            fh.write("north local %s\n" % root)
        self.assertEqual(0, schedule.lease_init(self.db, lanes="north", lanes_file=lanes_file,
                                                now=T0).code)
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a", lane="north", now=T0).code)

    def lane_run(self, cwd=None):
        """A fake lane run: a process that lives in `cwd` (the lane tree) until it is ended."""
        run = subprocess.Popen([sys.executable, "-c", "import sys; sys.stdin.read()"],
                               cwd=cwd or self.tree, stdin=subprocess.PIPE)

        def end():
            if run.poll() is None:
                run.kill()
            run.stdin.close()
            run.wait()
        self.addCleanup(end)
        return run

    def held(self):
        return {r["holder"]: r["resource"] for r in self.db.execute(
            "SELECT holder, resource FROM lease_entries WHERE resource IS NOT NULL")}

    def test_a_release_waits_while_a_run_lives_in_the_lane_tree(self):
        run = self.lane_run()
        res = schedule.lease_release(self.db, "welle-x/a", now=T0 + 60)
        self.assertEqual(3, res.code, res.err)
        self.assertIn("pid %d" % run.pid, res.err)
        self.assertIn("--force ends it", res.err)
        self.assertEqual({"welle-x/a": "lane:north"}, self.held())
        self.assertIsNone(run.poll(), "a release without --force never ends the run")
        run.stdin.close()
        run.wait()
        res = schedule.lease_release(self.db, "welle-x/a", now=T0 + 120)
        self.assertEqual(0, res.code, res.err)
        self.assertEqual({}, self.held())

    def test_release_force_ends_the_run_in_the_lane_tree(self):
        run = self.lane_run()
        res = schedule.lease_release(self.db, "welle-x/a", now=T0 + 60, force=True)
        self.assertEqual(0, res.code, res.err)
        self.assertEqual({}, self.held())
        kit_time.until("the forced release ends the run", lambda: run.poll() is not None)
        self.assertIn("ended pid %d in the lane tree" % run.pid, res.err)
        detail = [d["detail"] for d in self.journal("lease_released")]
        self.assertEqual(1, len(detail), detail)
        self.assertIn("ended pid %d" % run.pid, detail[0])

    def test_a_live_process_the_holder_entered_keeps_the_lease(self):
        # `strand.sh test` enters its own pid at the lease (`check --pid`); its
        # lane side may not be in the tree yet -- the local driver alone counts.
        driver = self.lane_run(cwd=self._tmp.name)
        self.assertEqual(0, schedule.lease_check(self.db, "welle-x/a", pid=driver.pid,
                                                 caller=os.getpid(), now=T0 + 30).code)
        res = schedule.lease_release(self.db, "welle-x/a", now=T0 + 60)
        self.assertEqual(3, res.code, res.err)
        self.assertIn("pid %d" % driver.pid, res.err)
        self.assertEqual({"welle-x/a": "lane:north"}, self.held())

    def test_a_probe_without_an_answer_keeps_the_lease(self):
        # Error path (fault injection): the probe raises. Fail-closed (ruling
        # test-cc15, re-check I1): without an answer nobody knows whether a lane
        # run still writes in the tree -- the lease stays, exit 3, and the line
        # names the way out (a powered-off lane is freed with --force).
        def broken(ln):
            raise RuntimeError("probe down")
        schedule._lane_tree_probe = broken
        res = schedule.lease_release(self.db, "welle-x/a", now=T0 + 60)
        self.assertEqual(3, res.code, res.err)
        self.assertIn("lane tree probe got no answer (RuntimeError: probe down)", res.err)
        self.assertIn("lane does not answer -- `release --force` frees it", res.err)
        self.assertEqual({"welle-x/a": "lane:north"}, self.held())
        self.assertEqual([], self.journal("lease_released"))

    def test_release_force_never_ends_an_entered_driver_outside_the_tree(self):
        # Re-check I2: `--force` ends the pids its probe found in the lane tree
        # of this lease -- never a local pid the holder entered (`check --pid`,
        # the driver of `strand.sh test`) nor any process outside the tree.
        run = self.lane_run()
        driver = self.lane_run(cwd=self._tmp.name)
        self.assertEqual(0, schedule.lease_check(self.db, "welle-x/a", pid=driver.pid,
                                                 caller=os.getpid(), now=T0 + 30).code)
        res = schedule.lease_release(self.db, "welle-x/a", now=T0 + 60, force=True)
        self.assertEqual(0, res.code, res.err)
        self.assertEqual({}, self.held())
        kit_time.until("the forced release ends the tree run", lambda: run.poll() is not None)
        self.assertIn("ended pid %d in the lane tree" % run.pid, res.err)
        self.assertNotIn("pid %d" % driver.pid, res.err)
        self.assertIsNone(driver.poll(), "the entered driver outside the tree lives on")

    def test_release_force_kills_only_while_the_probed_lease_stands(self):
        # Re-check M1: the probe and the kill run before `BEGIN IMMEDIATE`. A
        # lease that changed under the probe (taken anew: another take time)
        # is not the lease whose tree was probed -- nothing is ended.
        killed = []

        def probe(ln):
            self.db.execute("UPDATE lease_entries SET since = ? WHERE holder = ?",
                            ("2027-01-15T09:00:00Z", "welle-x/a"))
            return [4242]
        schedule._lane_tree_probe = probe
        schedule._lane_tree_kill = lambda ln, pids: killed.append(pids) or []
        res = schedule.lease_release(self.db, "welle-x/a", now=T0 + 60, force=True)
        self.assertEqual([], killed, "a kill under a lease the probe did not see")
        # Re-check 518f70bfb m1: `--force` frees it under the run, as documented --
        # but says so: no probe saw this lease's tree, a lane run may still write.
        self.assertEqual(0, res.code, res.err)
        self.assertEqual({}, self.held())
        self.assertIn("warning: released welle-x/a without a probe of the tree of lane north "
                      "(the lease changed after the probe)", res.err)
        detail = [d["detail"] for d in self.journal("lease_released")]
        self.assertEqual(1, len(detail), detail)
        self.assertIn("no probe of lane north (the lease changed after the probe)", detail[0])

    def test_a_release_whose_lease_changed_after_the_probe_keeps_it(self):
        # Re-check 518f70bfb m1 (the class of #1228): the probe runs before
        # `BEGIN IMMEDIATE`; a lease that moved to another lane in between had
        # no probe of its tree -- no probe is no proof the lane run ended. Exit 3,
        # "release again", the lease stays; the next release probes that tree.
        probed = []

        def probe(ln):
            probed.append(ln)
            if len(probed) == 1:
                self.db.execute("UPDATE lease_entries SET resource = 'lane:south' "
                                "WHERE holder = 'welle-x/a'")
                self.db.commit()
            return []
        schedule._lane_tree_probe = probe
        res = schedule.lease_release(self.db, "welle-x/a", now=T0 + 60)
        self.assertEqual(3, res.code, res.err)
        self.assertIn("welle-x/a holds lane south: the lease changed after the probe", res.err)
        self.assertIn("no probe of this tree ran yet -- release again", res.err)
        self.assertNotIn("probed at", res.err)
        self.assertEqual({"welle-x/a": "lane:south"}, self.held())
        self.assertEqual([], self.journal("lease_released"))
        res = schedule.lease_release(self.db, "welle-x/a", now=T0 + 120)
        self.assertEqual(0, res.code, res.err)
        self.assertEqual(2, len(probed), "the second release probes the tree of the moved lease")
        self.assertEqual({}, self.held())

    def test_a_forced_release_without_a_probe_answer_frees_and_says_so(self):
        def broken(ln):
            raise RuntimeError("probe down")
        schedule._lane_tree_probe = broken
        res = schedule.lease_release(self.db, "welle-x/a", now=T0 + 60, force=True)
        self.assertEqual(0, res.code, res.err)
        self.assertIn("lane tree probe got no answer", res.err)
        self.assertEqual({}, self.held())


class TestMergeGateHolder(TestLeaseKitFindsWave):
    """Kit find (h) of welle-testing 2026-10-11 (ledger 03:35): the merge gate in
    the detached merge tree ran without a holder -- refused on its own strand's
    lane, and its refusal left a waiter without holder or tree that was offered
    a lane it never took (build04 idle ~20 min, seven waiters behind it)."""

    def test_a_gate_without_a_holder_leaves_no_waiter(self):
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/x", lane="north", now=T0).code)
        queued = self.admit(cargo_plan(), strand="", wave="", lane_pref="north")
        self.assertIsInstance(queued, schedule.Queued)
        self.assertEqual([], self.queue(), "an anonymous waiter can never take its offer")

    def test_a_merge_gate_refused_leaves_no_waiter(self):
        # The merge does not come back for an offer: no queue place, held or not.
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/x", lane="north", now=T0).code)
        queued = self.admit(cargo_plan(), strand="welle-x/b", lane_pref="north", queue=False)
        self.assertIsInstance(queued, schedule.Queued)
        self.assertEqual([], self.queue())

    def test_a_merge_gate_without_a_queue_place_keeps_a_place_it_had(self):
        # The strand's own `take --wait` place is not the gate's to drop.
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/x", lane="north", now=T0).code)
        self.assertEqual(3, schedule.lease_take(self.db, "welle-x/b", lane="north", now=T0).code)
        self.admit(cargo_plan(), strand="welle-x/b", lane_pref="north", queue=False)
        self.assertEqual([("welle-x/b", "north")], self.queue())

    def test_a_merge_gate_runs_on_the_lane_its_strand_holds(self):
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/a", lane="north", now=T0).code)
        adm = self.admit(cargo_plan(), strand="welle-x/a", lane_pref="north", queue=False)
        self.assertEqual("north", getattr(adm, "lane", None), adm)

    def test_a_batch_merge_gate_runs_on_the_lane_a_member_holds(self):
        # The batch holder `<wave>/S1+S2` speaks for its members (merge `_members`).
        self.arm("north", "south")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/x", lane="north", now=T0).code)
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/b", lane="south", now=T0).code)
        adm = self.admit(cargo_plan(), strand="welle-x/a+b", lane_pref="south", queue=False)
        self.assertEqual("south", getattr(adm, "lane", None), adm)
        self.assertEqual([], self.queue())

    def test_the_end_of_a_batch_run_clears_the_run_on_the_member_lease(self):
        """Re-check fix4 H1: the batch ran on member b's lease (`run` set there),
        but the run end looked up `welle-x/a+b` and left b's `run` pointing at
        a finished run. The lease itself stays b's."""
        self.arm("north", "south")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/b", lane="south", now=T0).code)
        adm = self.admit(cargo_plan(), strand="welle-x/a+b", lane_pref="south", queue=False)
        self.assertEqual("south", getattr(adm, "lane", None), adm)
        member = lambda: next(h for h in schedule._load(self.db)["holders"] if h["strand"] == "welle-x/b")  # noqa: E731
        self.assertEqual(adm.run, member()["run"])
        self.finish(adm, {})
        self.assertIsNone(member()["run"])
        self.assertEqual("south", member()["lane"])


class TestMergeGoesFirst(TestLeaseKitFindsWave):
    """K14 (16.10., zu #1228): a waiting merge candidate (`MECLAW_GATE_MODE=merge`,
    `merge --wait` -> `gate --wait-admission`) goes ahead of every waiting strand,
    pre and test run; FIFO within each class; held leases are never displaced;
    at most `MERGE_STREAK_MAX` merges in a row while strands wait."""

    def offered(self):
        return [r["holder"] for r in self.db.execute(
            "SELECT holder FROM lease_entries WHERE resource IS NULL AND offered IS NOT NULL "
            "ORDER BY seq")]

    def wait_trees(self):
        """Each waiter's place tree (`lease_entries.tree`, "" for none)."""
        return {r["holder"]: r["tree"] or "" for r in self.db.execute(
            "SELECT holder, tree FROM lease_entries WHERE resource IS NULL")}

    def strands_wait(self, *names, minutes=0, tree=""):
        for n in names:
            self.assertEqual(3, schedule.lease_take(self.db, n, lane="north", tree=tree,
                                                    now=T0 + 60 * minutes).code)

    def release_tree(self, *strands):
        """A caller's tree whose strand files declare `strands` release strands
        (`"release": true`, lease re-check m2) -- the name alone is no class."""
        tree = os.path.join(self._tmp.name, "rt")
        d = os.path.join(tree, "plans", "welle-x-2026-10-16", "straenge")
        os.makedirs(d, exist_ok=True)
        for s in strands:
            with open(os.path.join(d, s + ".json"), "w") as fh:
                json.dump({"strand": s, "release": True}, fh)
        return tree

    def merge_waits(self, name, minutes=1):
        queued = self.admit(cargo_plan(), strand=name, lane_pref="north", minutes=minutes,
                            merge=True)
        self.assertIsInstance(queued, schedule.Queued)

    def free_north(self, holder, minutes):
        if isinstance(holder, schedule.Admission):    # a merge run ends: its close frees the lane
            pe = json.loads(ev("pass_ended", {"verdict": "GREEN", "secs": 50}, holder.run))
            schedule.close(self.db, holder, pe, now=T0 + 60 * minutes)
            return
        self.assertEqual(0, schedule.lease_release(self.db, holder, now=T0 + 60 * minutes,
                                                   branch=holder).code)

    def test_a_merge_gets_the_next_offer_before_three_waiting_strands(self):
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/x", lane="north", now=T0).code)
        self.strands_wait("welle-x/s1", "welle-x/s2", "welle-x/s3")
        self.merge_waits("welle-x/m")
        self.assertEqual(["welle-x/m", "welle-x/s1", "welle-x/s2", "welle-x/s3"],
                         [h for h, _ in self.queue()], "FIFO within the strands, the merge first")
        self.assertEqual(["welle-x/x"], [h["strand"] for h in schedule._load(self.db)["holders"]],
                         "the held lease is not displaced")
        self.free_north("welle-x/x", 2)
        self.assertEqual(["welle-x/m"], self.offered())
        self.assertEqual(3, schedule.lease_take(self.db, "welle-x/s1", lane="north",
                                                now=T0 + 180).code)
        adm = self.admit(cargo_plan(), strand="welle-x/m", lane_pref="north", minutes=3, merge=True)
        self.assertEqual("north", getattr(adm, "lane", None), adm)

    def test_strands_get_a_lane_after_merge_streak_max_merges_in_a_row(self):
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/x", lane="north", now=T0).code)
        self.strands_wait("welle-x/s1", "welle-x/s2")
        holder, minute = "welle-x/x", 1
        for i in range(schedule.MERGE_STREAK_MAX):
            merge = "welle-x/m%d" % i
            self.merge_waits(merge, minutes=minute)
            self.free_north(holder, minute + 1)
            self.assertEqual([merge], self.offered())
            adm = self.admit(cargo_plan(), strand=merge, lane_pref="north", minutes=minute + 1,
                             merge=True)
            self.assertEqual("north", getattr(adm, "lane", None), adm)
            holder, minute = adm, minute + 2
        self.merge_waits("welle-x/m-next", minutes=minute)
        self.assertEqual(["welle-x/s1", "welle-x/m-next", "welle-x/s2"],
                         [h for h, _ in self.queue()], "after the streak the oldest strand leads")
        self.free_north(holder, minute + 1)
        self.assertEqual(["welle-x/s1"], self.offered())
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/s1", lane="north",
                                                now=T0 + 60 * (minute + 1)).code)
        self.assertEqual(["welle-x/m-next", "welle-x/s2"], [h for h, _ in self.queue()],
                         "a strand served ends the streak: merges lead again")

    def test_the_release_wave_waits_between_the_merges_and_the_program(self):
        # K14 (16.10.): merge > release wave > the rest, FIFO within each class.
        # A release strand is `rel*` by name (`rel-3c`, `rel3`, `rel-0643-10`),
        # with or without the wave prefix and a holder suffix `:n` -- and declared
        # as one by its strand file in the caller's tree (lease re-check m2).
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/x", lane="north", now=T0).code)
        rt = self.release_tree("rel-3c", "rel3")
        self.strands_wait("welle-x/s1", "welle-x/relay", tree=rt)
        self.strands_wait("welle-x/rel-3c", "welle-x/s2", "welle-x/rel3:2", minutes=1, tree=rt)
        self.merge_waits("welle-x/m", minutes=2)
        self.assertEqual(["welle-x/m", "welle-x/rel-3c", "welle-x/rel3:2", "welle-x/s1",
                          "welle-x/relay", "welle-x/s2"], [h for h, _ in self.queue()])
        self.free_north("welle-x/x", 3)
        self.assertEqual(["welle-x/m"], self.offered())
        adm = self.admit(cargo_plan(), strand="welle-x/m", lane_pref="north", minutes=3, merge=True)
        self.assertEqual("north", getattr(adm, "lane", None), adm)
        self.free_north(adm, 4)
        self.assertEqual(["welle-x/rel-3c"], self.offered(), "the release before the program")
        # the streak cap still serves the oldest non-merge first -- the release
        self.assertEqual(
            ["rel-a", "m1", "s1"],
            [w["strand"] for w in schedule._queue_order(
                [{"strand": "s1"}, {"strand": "m1", "merge": True},
                 {"strand": "rel-a", "release": True}],
                schedule.MERGE_STREAK_MAX)])

    def test_a_program_strand_named_rel_gets_no_release_lead(self):
        # Lease re-check m2: the name alone made `rel-notes` (a program strand)
        # a release strand. Only a declared one leads the program: its strand
        # file says `"release": true`, or the pass is a release gate.
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/x", lane="north", now=T0).code)
        rt = self.release_tree("rel-3c")
        self.strands_wait("welle-x/s1", "welle-x/rel-notes", tree=rt)
        self.strands_wait("welle-x/rel-3c", minutes=1, tree=rt)
        self.assertEqual(["welle-x/rel-3c", "welle-x/s1", "welle-x/rel-notes"],
                         [h for h, _ in self.queue()])
        self.assertTrue(schedule._release_class("welle-x/rel-0643-10", "", "release"),
                        "a release gate is the release path without a strand file")
        self.assertFalse(schedule._release_class("welle-x/s1", "", "release"),
                         "the name convention stays a condition")

    def test_a_stream_of_merges_and_releases_lets_the_program_through(self):
        # Lease re-check m2: the release class has its own cap. Merges and
        # releases granted in a row while a program strand waits count together;
        # after `RELEASE_STREAK_MAX` of them the oldest program strand goes first once.
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/x", lane="north", now=T0).code)
        rt = self.release_tree(*["rel-%d" % i for i in range(schedule.RELEASE_STREAK_MAX + 1)])
        self.strands_wait("welle-x/s1", tree=rt)
        holder, minute = "welle-x/x", 1
        for i in range(schedule.RELEASE_STREAK_MAX):
            name = ("welle-x/m%d" if i % 2 == 0 else "welle-x/rel-%d") % i
            if i % 2 == 0:
                self.merge_waits(name, minutes=minute)
            else:
                self.strands_wait(name, minutes=minute, tree=rt)
            self.free_north(holder, minute + 1)
            self.assertEqual([name], self.offered())
            if i % 2 == 0:
                holder = self.admit(cargo_plan(), strand=name, lane_pref="north",
                                    minutes=minute + 1, merge=True)
                self.assertEqual("north", getattr(holder, "lane", None), holder)
            else:
                self.assertEqual(0, schedule.lease_take(self.db, name, lane="north", tree=rt,
                                                        now=T0 + 60 * (minute + 1)).code)
                holder = name
            minute += 2
        self.merge_waits("welle-x/m-next", minutes=minute)
        self.strands_wait("welle-x/rel-%d" % schedule.RELEASE_STREAK_MAX, minutes=minute, tree=rt)
        self.assertEqual(["welle-x/s1", "welle-x/m-next",
                          "welle-x/rel-%d" % schedule.RELEASE_STREAK_MAX],
                         [h for h, _ in self.queue()], "after the cap the program strand leads")
        self.free_north(holder, minute + 1)
        self.assertEqual(["welle-x/s1"], self.offered())
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/s1", lane="north", tree=rt,
                                                now=T0 + 60 * (minute + 1)).code)
        self.assertEqual(["welle-x/m-next", "welle-x/rel-%d" % schedule.RELEASE_STREAK_MAX],
                         [h for h, _ in self.queue()], "a program strand served ends the streak")
        # Re-check Kette I1 (a): the program grant set the count to zero -- a
        # program strand queued after it waits behind the merge and the release
        # again (without the reset every program strand would lead from now on).
        self.strands_wait("welle-x/s2", minutes=minute + 2, tree=rt)
        self.assertEqual(["welle-x/m-next", "welle-x/rel-%d" % schedule.RELEASE_STREAK_MAX,
                          "welle-x/s2"], [h for h, _ in self.queue()],
                         "after the reset the merge and the release lead the program again")

    def test_a_release_gate_waits_ahead_of_the_program(self):
        # Re-check Kette I1 (b): the release class on the admission path. A
        # `gate release` (its mode) and the gate of a declared release strand
        # (its tree: `--wait-admission` hands it as `wait_tree`, a gate without
        # it as `release_tree`, Minor 2) queue ahead of a program strand that
        # waited first; a program strand's gate stays program.
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/x", lane="north", now=T0).code)
        rt = self.release_tree("rel-3c", "rel-4")
        self.strands_wait("welle-x/s1", tree=rt)
        for minute, (name, kw) in enumerate((
                ("welle-x/s2", {"wait_tree": rt}),
                ("welle-x/rel-0643-10", {"mode": "release"}),
                ("welle-x/rel-3c", {"wait_tree": rt}),
                ("welle-x/rel-4", {"release_tree": rt})), start=1):
            queued = self.admit(cargo_plan(), strand=name, lane_pref="north", minutes=minute, **kw)
            self.assertIsInstance(queued, schedule.Queued, name)
        self.assertEqual(["welle-x/rel-0643-10", "welle-x/rel-3c", "welle-x/rel-4",
                          "welle-x/s1", "welle-x/s2"], [h for h, _ in self.queue()])
        # Re-check Kette FR2 m4: the class tree is no place tree -- `tree` keeps
        # a waiter's place while the tree lives (`_stale_wait`), a gate without
        # the wait hands its tree for the class alone.
        self.assertEqual({"welle-x/rel-3c": rt, "welle-x/rel-4": ""},
                         {h: t for h, t in self.wait_trees().items()
                          if h in ("welle-x/rel-3c", "welle-x/rel-4")})

    def test_a_take_of_a_second_holder_keeps_the_release_class(self):
        # Re-check Kette Minor 2: `lanes take --holder <n>` from the strand's own
        # tree hands the tree for the class (`release_tree`) -- a declared
        # release strand's `S:n` waits as a release, not as program.
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/x", lane="north", now=T0).code)
        rt = self.release_tree("rel-3c")
        self.strands_wait("welle-x/s1", tree=rt)
        self.assertEqual(3, schedule.lease_take(self.db, "welle-x/rel-3c:2", lane="north",
                                                release_tree=rt, now=T0 + 60).code)
        self.assertEqual(["welle-x/rel-3c:2", "welle-x/s1"], [h for h, _ in self.queue()])
        # Re-check Kette FR2 m4: the class tree keeps no place -- only `tree` does
        self.assertEqual("", self.wait_trees()["welle-x/rel-3c:2"])

    def test_a_release_gate_on_a_lane_offered_to_the_program_is_admitted_at_once(self):
        # Re-check Kette FR2 m1: a caller not yet queued was placed by a probe
        # without its release class (the merge class had it, K14) -- a release
        # gate on a free lane offered to a program strand queued as #1 and left
        # the lane idle until the next round. Placed by its class: admitted.
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/x", lane="north", now=T0).code)
        self.strands_wait("welle-x/s1")
        self.free_north("welle-x/x", 1)
        self.assertEqual(["welle-x/s1"], self.offered())
        adm = self.admit(cargo_plan(), strand="welle-x/rel-0643-10", mode="release",
                         lane_pref="north", minutes=2)
        self.assertEqual("north", getattr(adm, "lane", None), adm)
        self.assertEqual([("welle-x/s1", "north")], self.queue())

    def test_a_release_take_on_a_lane_offered_to_the_program_is_granted_at_once(self):
        # Re-check Kette FR2 m1: `take` counted every waiter ahead of a caller
        # not yet queued -- a declared release strand's take on a free lane
        # offered to a program strand got exit 3 (#1, offered). Its place is
        # where its class puts it, the order `_queue_order` keeps.
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/x", lane="north", now=T0).code)
        rt = self.release_tree("rel-3c")
        self.strands_wait("welle-x/s1", tree=rt)
        self.free_north("welle-x/x", 1)
        self.assertEqual(["welle-x/s1"], self.offered())
        res = schedule.lease_take(self.db, "welle-x/rel-3c", lane="north", tree=rt, now=T0 + 120)
        self.assertEqual(0, res.code, res.err)
        self.assertEqual(["welle-x/rel-3c"],
                         [h["strand"] for h in schedule._load(self.db)["holders"]])
        self.assertEqual([("welle-x/s1", "north")], self.queue())

    def test_release_takes_granted_at_once_count_in_the_release_streak(self):
        # Re-check stack I1 (take path): since FR2 m1 a release caller not yet
        # queued is placed by its class and granted at once -- with no queue
        # entry `_grant` read no class, counted the grant as program and reset
        # the streak: the waiting program strand starved behind the releases.
        # The caller hands its class; after `RELEASE_STREAK_MAX` releases the
        # program strand gets the lane.
        self.arm("north")
        cap = schedule.RELEASE_STREAK_MAX
        rt = self.release_tree(*["rel-%d" % i for i in range(cap + 1)])
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/x", lane="north", now=T0).code)
        self.strands_wait("welle-x/s1", tree=rt)
        holder, minute = "welle-x/x", 1
        for i in range(cap):
            self.free_north(holder, minute)
            self.assertEqual(["welle-x/s1"], self.offered(), "round %d" % i)
            holder = "welle-x/rel-%d" % i
            res = schedule.lease_take(self.db, holder, lane="north", tree=rt,
                                      now=T0 + 60 * minute)
            self.assertEqual(0, res.code, "round %d: %s" % (i, res.err))
            self.assertEqual(i + 1, schedule._load(self.db)["release_streak"],
                             "round %d: a release granted at once counts" % i)
            minute += 2
        self.free_north(holder, minute)
        self.assertEqual(3, schedule.lease_take(self.db, "welle-x/rel-%d" % cap, lane="north",
                                                tree=rt, now=T0 + 60 * minute).code,
                         "after the cap the next release waits")
        self.assertEqual(["welle-x/s1"], self.offered())
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/s1", lane="north", tree=rt,
                                                now=T0 + 60 * minute).code)
        self.assertEqual(0, schedule._load(self.db)["release_streak"],
                         "a program strand served ends the streak")

    def test_release_takes_without_a_waiting_program_count_no_release_streak(self):
        # Re-check stack fr m1: the streak counts releases in a row only while
        # the program waits (lease re-check m2). Counted without a waiter, a
        # program strand arriving after `RELEASE_STREAK_MAX` releases went
        # ahead of the next release at once -- the cap is about starving, and
        # nobody starved.
        self.arm("north")
        cap = schedule.RELEASE_STREAK_MAX
        rt = self.release_tree(*["rel-%d" % i for i in range(cap + 1)])
        minute = 0
        for i in range(cap):
            holder = "welle-x/rel-%d" % i
            res = schedule.lease_take(self.db, holder, lane="north", tree=rt,
                                      now=T0 + 60 * minute)
            self.assertEqual(0, res.code, "round %d: %s" % (i, res.err))
            self.assertEqual(0, schedule._load(self.db)["release_streak"],
                             "round %d: no program waits, no streak" % i)
            minute += 1
            self.free_north(holder, minute)
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/x", lane="north",
                                                now=T0 + 60 * minute).code)
        self.strands_wait("welle-x/s1", minutes=minute + 1, tree=rt)
        self.free_north("welle-x/x", minute + 2)
        res = schedule.lease_take(self.db, "welle-x/rel-%d" % cap, lane="north", tree=rt,
                                  now=T0 + 60 * (minute + 2))
        self.assertEqual(0, res.code, "the next release still goes ahead: %s" % res.err)
        self.assertEqual(1, schedule._load(self.db)["release_streak"],
                         "the first release while the program waits")

    def test_release_gates_admitted_at_once_count_in_the_release_streak(self):
        # Re-check stack I1 (gate release path): the same for the admission of
        # a `gate release` not yet queued -- `_admit` hands its class to the grant.
        self.arm("north")
        cap = schedule.RELEASE_STREAK_MAX
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/x", lane="north", now=T0).code)
        self.strands_wait("welle-x/s1")
        holder, minute = "welle-x/x", 1
        for i in range(cap):
            self.free_north(holder, minute)
            self.assertEqual(["welle-x/s1"], self.offered(), "round %d" % i)
            holder = self.admit(cargo_plan(), strand="welle-x/rel-0643-%d" % (10 + i),
                                mode="release", lane_pref="north", minutes=minute)
            self.assertEqual("north", getattr(holder, "lane", None), "round %d: %s" % (i, holder))
            self.assertEqual(i + 1, schedule._load(self.db)["release_streak"],
                             "round %d: a release gate admitted at once counts" % i)
            minute += 2
        self.free_north(holder, minute)
        queued = self.admit(cargo_plan(), strand="welle-x/rel-0643-%d" % (10 + cap),
                            mode="release", lane_pref="north", minutes=minute)
        self.assertIsInstance(queued, schedule.Queued, "after the cap the next release gate waits")
        self.assertEqual(["welle-x/s1"], self.offered())
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/s1", lane="north",
                                                now=T0 + 60 * minute).code)
        self.assertEqual(0, schedule._load(self.db)["release_streak"],
                         "a program strand served ends the streak")

    def test_the_strand_file_of_another_wave_is_no_release_class(self):
        # Re-check Kette Minor 2 (side note): `plans/<wave>*` also matched a wave
        # `<wave>X-...` with a strand of the same name; the strand file counts in
        # `plans/<wave>/` or `plans/<wave>-*/` only.
        tree = os.path.join(self._tmp.name, "other")
        for wave_dir, release in (("welle-xy-2026-10-16", True), ("welle-x-2026-10-16", False)):
            d = os.path.join(tree, "plans", wave_dir, "straenge")
            os.makedirs(d, exist_ok=True)
            with open(os.path.join(d, "rel-3c.json"), "w") as fh:
                json.dump({"strand": "rel-3c", "release": release}, fh)
        self.assertFalse(schedule._release_class("welle-x/rel-3c", tree),
                         "the strand file of wave welle-xy declares nothing for welle-x")
        self.assertTrue(schedule._release_class("welle-xy/rel-3c", tree))
        for wave_dir in ("welle-x", "welle-x-2026-10-17"):
            d = os.path.join(tree, "plans", wave_dir, "straenge")
            os.makedirs(d, exist_ok=True)
            with open(os.path.join(d, "rel-5.json"), "w") as fh:
                json.dump({"strand": "rel-5", "release": True}, fh)
            with self.subTest(wave_dir=wave_dir):
                self.assertTrue(schedule._release_class("welle-x/rel-5", tree))
                os.remove(os.path.join(d, "rel-5.json"))
        # Re-check Kette FR2 m3: `plans/<wave>-*/` also matched the wave
        # `<wave>-2-<date>` (the repo has e.g. `welle-x-2026-10-10/` beside
        # `welle-x-2-2026-10-14/`) -- the kit's rule decides: the wave
        # is the directory without `-YYYY-MM-DD` (`strand.sh` `wave_name`).
        second = os.path.join(self._tmp.name, "second")
        d = os.path.join(second, "plans", "welle-x-2-2026-10-16", "straenge")
        os.makedirs(d)
        with open(os.path.join(d, "rel-3c.json"), "w") as fh:
            json.dump({"strand": "rel-3c", "release": True}, fh)
        with self.subTest(wave_dir="welle-x-2-2026-10-16"):
            self.assertFalse(schedule._release_class("welle-x/rel-3c", second),
                             "the strand file of wave welle-x-2 declares nothing for welle-x")
            self.assertTrue(schedule._release_class("welle-x-2/rel-3c", second))


class TestLeaseLiveRunClass(ScheduleCase):
    """Re-check fix5 M1, the class: a lease that carries a live run (its `run`,
    or any run on its lane) is never taken over, freed or reclaimed -- except
    with `--force`. Each pin is one site that wrote, freed or took a lease."""

    arm = TestLeaseKitFinds.arm

    def member(self, key="welle-x/b"):
        return next(h for h in schedule._load(self.db)["holders"] if h["strand"] == key)

    def own_run(self, key="welle-x/b", lane="south", tree=""):
        self.arm("north", "south")
        self.assertEqual(0, schedule.lease_take(self.db, key, lane=lane, tree=tree, now=T0).code)
        own = self.admit(light_plan(), strand=key, lane_pref=lane, queue=False)
        self.assertEqual(lane, getattr(own, "lane", None), own)
        self.assertEqual(own.run, self.member(key)["run"])
        return own

    def test_a_batch_never_takes_over_the_member_lease_of_a_live_run(self):
        # admission (`mine["run"] = run`): the batch overwrote b's live run.
        own = self.own_run()
        bat = self.admit(light_plan(), strand="welle-x/a+b", lane_pref="south", queue=False)
        self.assertIsInstance(bat, schedule.Queued, bat)
        self.assertIn("in use by run %s" % own.run, bat.reason)
        self.assertEqual(own.run, self.member()["run"])
        self.assertEqual(3, schedule.lease_release(self.db, "welle-x/b", now=T0).code)
        self.assertEqual("south", self.member()["lane"], "b keeps its lane under its live run")

    def test_a_second_pass_of_the_holder_waits_for_its_live_run(self):
        # admission, the same holder: a second light pass (another base) overwrote the run.
        own = self.own_run()
        second = self.admit(light_plan(base="other"), strand="welle-x/b", lane_pref="south",
                            queue=False)
        self.assertIsInstance(second, schedule.Queued, second)
        self.assertEqual(own.run, self.member()["run"])

    def test_release_refuses_a_lease_whose_lane_carries_a_live_run(self):
        # `lease_release` looked at the `run` pointer only, `_prune` at run OR lane.
        own = self.own_run()
        st = schedule._load(self.db)
        next(h for h in st["holders"] if h["strand"] == "welle-x/b")["run"] = None
        schedule._save(self.db, st, schedule._Ctx(self.db, T0, None))
        self.assertEqual(3, schedule.lease_release(self.db, "welle-x/b", now=T0).code)
        self.assertEqual(0, schedule.lease_release(self.db, "welle-x/b", now=T0, force=True).code)
        self.assertIsNotNone(own.run)

    def test_a_gone_worktree_does_not_reclaim_a_lease_with_a_live_run(self):
        # `_stale` "its worktree is gone" ignored the live run on the lease.
        tree = os.path.join(self._tmp.name, "wt-b")
        os.mkdir(tree)
        self.own_run(tree=tree)
        os.rmdir(tree)
        self.assertEqual(3, schedule.lease_take(self.db, "welle-x/c", lane="south", now=T0).code)
        self.assertEqual("south", self.member()["lane"])

    def test_a_short_hold_ttl_does_not_reclaim_a_lease_with_a_live_run(self):
        # `_stale` "last seen N min ago": `take --ttl 1` shortened the TTL under the run.
        own = self.own_run()
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/b", lane="south", ttl=1,
                                                now=T0).code)
        self.assertIn(own.run, [r["run"] for r in schedule._live_runs(self.db, T0 + 180)])
        self.assertEqual(3, schedule.lease_take(self.db, "welle-x/c", lane="south",
                                                now=T0 + 180).code)
        self.assertEqual(own.run, self.member()["run"])


class TestArmOverALiveRun(ScheduleCase):
    """Re-check fix6 M1, the class at its root: a run admitted while the pool was
    off has no holder, so arming the pool over it (`init`, the migration of the
    old token file) handed its lane to the next `take` or admission (probe P1:
    init 0, `take welle-x/c --lane south` 0 under run-A). Arming refuses while a
    run is live on a lane it arms -- `--force` stays the way through."""

    def lanes_file(self, *names):
        path = os.path.join(self._tmp.name, "lanes")
        with open(path, "w") as fh:
            fh.write("".join("%s builder@%s.example.invalid\n" % (n, n) for n in names))
        return path

    def unarmed_run(self, lane="south"):
        self.lanes("north", "south")
        own = self.admit(light_plan(), strand="welle-x/b", lane_pref=lane, queue=False)
        self.assertEqual(lane, getattr(own, "lane", None), own)
        return own

    def old_file(self, holders=(), lanes=("north", "south"), max_=2):
        path = os.path.join(self._tmp.name, "tokens")
        with open(path, "w") as fh:
            json.dump({"max": max_, "ttl_min": 45, "armed": "2027-01-15T07:00:00Z",
                       "holders": list(holders), "waiting": [],
                       "lanes": list(lanes) or None}, fh)
        return path

    def test_init_refuses_to_arm_lanes_over_a_live_run_without_a_lease(self):
        own = self.unarmed_run()
        res = schedule.lease_init(self.db, lanes="north,south",
                                  lanes_file=self.lanes_file("north", "south"), now=T0 + 60)
        self.assertEqual(2, res.code, res.err)
        self.assertIn("run %s" % own.run, res.err)
        self.assertIn("live on lane south", res.err)
        self.assertIn("--force", res.err)
        self.assertIsNone(schedule._load(self.db))
        self.assertEqual(2, schedule.lease_take(self.db, "welle-x/c", lane="south",
                                                now=T0 + 60).code)

    def test_init_arms_lanes_beside_a_live_run_it_does_not_arm(self):
        self.unarmed_run(lane="north")
        self.assertEqual(0, schedule.lease_init(self.db, lanes="south",
                                                lanes_file=self.lanes_file("north", "south"),
                                                now=T0 + 60).code)

    def test_init_with_max_refuses_over_any_live_run(self):
        own = self.unarmed_run()
        res = schedule.lease_init(self.db, max=2, now=T0 + 60)
        self.assertEqual(2, res.code, res.err)
        self.assertIn("run %s" % own.run, res.err)
        self.assertIsNone(schedule._load(self.db))

    def test_init_force_arms_over_the_live_run_and_says_so(self):
        self.unarmed_run()
        self.assertEqual(0, schedule.lease_init(self.db, lanes="north,south", force=True,
                                                lanes_file=self.lanes_file("north", "south"),
                                                now=T0 + 60).code)
        self.assertEqual(["north", "south"], schedule._load(self.db)["lanes"])
        self.assertIn("init --force", [e.get("verb") for e in self.journal("lease_noted")])

    def test_migration_refuses_to_arm_over_a_live_run_on_a_free_lane(self):
        own = self.unarmed_run()
        res = schedule.lease_migrate(self.db, self.old_file(),
                                     lanes_file=self.lanes_file("north", "south"), now=T0 + 60)
        self.assertEqual(2, res.code, res.err)
        self.assertIn("run %s" % own.run, res.err)
        self.assertFalse(schedule._armed(self.db))

    def test_migration_with_max_refuses_over_any_live_run(self):
        self.unarmed_run()
        res = schedule.lease_migrate(self.db, self.old_file(lanes=()), now=T0 + 60)
        self.assertEqual(2, res.code, res.err)
        self.assertFalse(schedule._armed(self.db))

    def test_migration_refuses_over_a_live_run_whatever_holder_the_old_file_names(self):
        """Re-check fix7 M2: the migration let any carried-over holder of the run's
        lane through -- a FOREIGN one too (probe: run-A of welle-x/b live on south,
        the old file names welle-x/c on south: migrate 0, then `check welle-x/c
        --lane south` 0, so c's `strand.sh test` built on south beside run-A). The
        old file cannot say whose run it is (the run's key is its pass, not its
        strand); the migration refuses like `init`, whoever holds the lane. Way
        out: the run's close, or `token init --force`."""
        own = self.unarmed_run()
        for strand in ("welle-x/c", "welle-x/b"):
            with self.subTest(holder=strand):
                holder = {"strand": strand, "since": T0 - 600, "seen": T0 - 60, "pids": [],
                          "tree": "", "lane": "south"}
                res = schedule.lease_migrate(self.db, self.old_file(holders=[holder]),
                                             lanes_file=self.lanes_file("north", "south"),
                                             now=T0 + 60)
                self.assertEqual(2, res.code, res.err)
                self.assertIn("run %s" % own.run, res.err)
                self.assertIn("live on lane south", res.err)
                self.assertIn("token init --force", res.err)
                self.assertFalse(schedule._armed(self.db))
                self.assertIsNone(schedule._load(self.db))


class TestAdmitWaitingEnds(ScheduleCase):
    """Review Z-A2 I1 (GH #1145): `admit_waiting` blocks only on causes an event
    or a watchdog round can resolve; a cause no round resolves ends the wait at
    once, and the whole wait has a limit."""

    arm = TestLeaseKitFinds.arm

    def wait(self, env=None, **kw):
        path = os.path.join(self._tmp.name, "pipeline.db")
        got = []

        def run():
            db = store.open(path)
            try:
                got.append(schedule.admit_waiting(
                    db, kw.pop("plan", cargo_plan()), repo="meclaw", mode=kw.pop("mode", "strand"),
                    strand=kw.pop("strand", "welle-x/a"), wave="welle-x", base=BASE,
                    lane_pref=kw.pop("lane_pref", None), now=None, **kw))
            except Exception as exc:  # noqa: BLE001 -- the test reads it
                got.append(exc)
            finally:
                db.close()
        old = {k: os.environ.get(k) for k in (env or {})}
        os.environ.update(env or {})
        try:
            th = threading.Thread(target=run, daemon=True)
            th.start()
            th.join(kit_time.HANG)
        finally:
            for k, v in old.items():
                if v is None:
                    os.environ.pop(k, None)
                else:
                    os.environ[k] = v
        self.assertFalse(th.is_alive(), "admit_waiting still waits on a cause no round resolves")
        return got[0]

    def test_a_lane_that_is_off_ends_the_wait(self):
        self.lanes("north", state="off")
        q = self.wait(secs=0.05)
        self.assertIsInstance(q, schedule.Queued)
        self.assertTrue(q.final)
        self.assertIn("north is off", q.reason)

    def test_an_egon_lane_for_a_strand_pass_ends_the_wait(self):
        schedule.sync_runners(self.db, [FakeLane("mm", ssh="local", transport="local", cls="egon")])
        q = self.wait(secs=0.05, lane_pref="mm", plan=light_plan())
        self.assertTrue(q.final)
        self.assertIn("egon lane mm runs only", q.reason)

    def test_no_registered_lane_ends_the_wait(self):
        q = self.wait(secs=0.05)
        self.assertTrue(q.final)
        self.assertEqual("no lane is registered", q.reason)

    def test_the_wait_has_a_limit_and_leaves_the_queue(self):
        # A lease another strand holds resolves on its release -- but not past
        # MECLAW_ADMISSION_WAIT_SECS; the waiter's queue place goes with it.
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/b", lane="north", now=T0).code)
        q = self.wait(env={"MECLAW_ADMISSION_WAIT_SECS": "0.3"}, secs=0.05,
                      wait_tree=self._tmp.name)
        self.assertTrue(q.final)
        self.assertIn("no admission within", q.reason)
        self.assertEqual([], [w["strand"] for w in schedule._load(self.db)["waiting"]])

    def test_a_waiter_is_queued_with_its_tree(self):
        # Review Z-A2 R2 M3: with its tree the waiter keeps its place while it lives.
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/b", lane="north", now=T0).code)
        q = schedule.admit(self.db, cargo_plan(), repo="meclaw", mode="strand", strand="welle-x/a",
                           wave="welle-x", base=BASE, lane_pref=None, now=T0, wait=True,
                           wait_tree=self._tmp.name)
        self.assertIsInstance(q, schedule.Queued)
        self.assertEqual([self._tmp.name], [w["tree"] for w in schedule._load(self.db)["waiting"]])

    def test_a_lane_booting_past_its_boot_time_ends_the_wait(self):
        # GH #1155 (3): nothing ends `booting` inside a wait (`_sync` turns
        # only `off` idle), so past its boot time it is a final reason.
        self.lanes("north", state="booting")
        self.db.execute("UPDATE runners SET updated_at = ?",
                        (schedule._iso(time.time() - schedule.BOOT_S - 60),))
        self.db.commit()
        q = self.wait(secs=0.05)
        self.assertIsInstance(q, schedule.Queued)
        self.assertTrue(q.final, q.reason)
        self.assertIn("north has been booting for more than", q.reason)

    def test_a_lane_within_its_boot_time_keeps_the_waiter(self):
        self.lanes("north", state="booting")
        self.db.execute("UPDATE runners SET updated_at = ?", (schedule._iso(T0 - 60),))
        self.db.commit()
        q = self.admit(cargo_plan())
        self.assertIsInstance(q, schedule.Queued)
        self.assertFalse(q.final, q.reason)
        self.assertIn("north is booting", q.reason)

    def test_an_expired_booting_lane_does_not_end_the_wait_of_one_in_its_window(self):
        # Re-check pd-1155 m-2: the first booting lane alone decided; past its
        # boot time it ended the wait although the second was still booting.
        self.lanes("north", "south", state="booting")
        self.db.execute("UPDATE runners SET updated_at = ? WHERE name = 'north'",
                        (schedule._iso(T0 - schedule.BOOT_S - 60),))
        self.db.execute("UPDATE runners SET updated_at = ? WHERE name = 'south'",
                        (schedule._iso(T0 - 60),))
        self.db.commit()
        q = self.admit(cargo_plan())
        self.assertIsInstance(q, schedule.Queued)
        self.assertFalse(q.final, q.reason)
        self.assertIn("south is booting", q.reason)
        self.assertIn("north has been booting for more than", q.reason)

    def test_an_expired_booting_lane_still_asks_power_for_an_off_one(self):
        # Re-check pd-1155 m-2: an expired `booting` went before `off`, so the
        # off lane was never asked for power.
        schedule.sync_runners(self.db, [FakeLane("north", threads=24), FakeLane("south", threads=24)],
                              states={"north": "booting", "south": "off"})
        self.db.execute("UPDATE runners SET updated_at = ? WHERE name = 'north'",
                        (schedule._iso(T0 - schedule.BOOT_S - 60),))
        self.db.commit()
        q = self.admit(cargo_plan())
        self.assertIsInstance(q, schedule.Queued)
        self.assertEqual("south", q.power_requested, q.reason)
        self.assertIn("north has been booting for more than", q.reason)

    def test_a_demand_over_the_lane_capacity_ends_the_wait(self):
        # GH #1155 (3): a pass no lane can ever hold (held 0) is no fit to wait for.
        # welle-tb S3 F1: the lane's colonies come from `colony_capacity` (no fixed 8),
        # so the demand is one over the lane's own count, at cores= 6, 8, 12 and 24 --
        # never this host's: colony 9 fitted build04 (24 vCPU, 12 colonies) and the
        # pin was red there (tb-s1e gate).
        widest = max(int(n.get("colony", 0)) for n in plan_mod.NEEDS.values())
        for cores in (6, 8, 12, 24):
            with self.subTest(cores=cores):
                self.lanes("north", cores=cores)
                cap = schedule.colony_capacity(cores, None)
                self.assertEqual(max(cores, widest), cap)
                q = self.wait(secs=0.05, plan=make_plan(station("e2e", colony=cap + 1)))
                self.assertIsInstance(q, schedule.Queued, q)
                self.assertTrue(q.final, q.reason)
                self.assertIn("lane north can never take the pass: needs colony %d > %d"
                              % (cap + 1, cap), q.reason)

    # tm-orch4 (16.10.): `token take --wait` died on `database is locked` with 18
    # waiters -- busy_timeout 5 s and the bounded retries were all a waiter had.
    def locked(self, n, real, before=None):
        """A stand-in for `real` that the store refuses `n` times (locked), then `real`."""
        calls = []

        def fake(*args, **kw):
            calls.append(1)
            if len(calls) <= n:
                if before:
                    before(*args)
                raise sqlite3.OperationalError("database is locked")
            return real(*args, **kw)
        return fake, calls

    def lanes_env(self, **extra):
        return dict(os.environ, MECLAW_PIPELINE_DB=os.path.join(self._tmp.name, "pipeline.db"),
                    MECLAW_LANES_FILE=os.path.join(self._tmp.name, "lanes"),
                    MECLAW_STRAND_TOKENS=os.path.join(self._tmp.name, "tok"), **extra)

    def test_a_locked_store_never_ends_the_admission_wait(self):
        self.arm("north")
        # the refusal leaves the transaction open (a COMMIT refused as busy): the
        # next round must not join it and lose its writes on close
        fake, calls = self.locked(4, schedule.admit,
                                  before=lambda db, *_: db.execute("BEGIN IMMEDIATE"))
        with unittest.mock.patch.object(schedule, "admit", side_effect=fake), \
                unittest.mock.patch.object(schedule, "LOCKED_PAUSE_S", (0.001, 0.002)):
            adm = self.wait()
        self.assertEqual(5, len(calls))
        self.assertEqual("north", getattr(adm, "lane", None), adm)
        other = store.open(os.path.join(self._tmp.name, "pipeline.db"))
        try:
            self.assertEqual(["welle-x/a"], [h["strand"] for h in schedule._load(other)["holders"]],
                             "the admission's lease is committed")
        finally:
            other.close()

    def test_a_store_locked_beyond_the_wait_limit_ends_the_wait_loud(self):
        self.arm("north")
        fake, calls = self.locked(10 ** 9, schedule.admit)
        with unittest.mock.patch.object(schedule, "admit", side_effect=fake), \
                unittest.mock.patch.object(schedule, "LOCKED_PAUSE_S", (0.001, 0.002)):
            exc = self.wait(env={"MECLAW_ADMISSION_WAIT_SECS": "0.2"})
        self.assertIsInstance(exc, sqlite3.OperationalError, exc)
        self.assertGreater(len(calls), 1, "the waiter outlasts a locked store up to the limit")

    def test_a_locked_store_never_ends_take_wait(self):
        self.arm("north")
        fake, calls = self.locked(4, schedule._run_once)
        with unittest.mock.patch.object(schedule, "_run_once", side_effect=fake), \
                unittest.mock.patch.object(schedule, "LOCKED_PAUSE_S", (0.001, 0.002)):
            code, out, err = schedule.run_lanes(
                ["take", "--wait", "--strand", "welle-x/a", "--lane", "north"],
                env=self.lanes_env(), cwd=self._tmp.name)
        self.assertEqual(0, code, (out, err))
        self.assertEqual(5, len(calls))
        self.assertEqual(["welle-x/a"], [h["strand"] for h in schedule._load(self.db)["holders"]])

    def test_a_take_without_wait_still_fails_on_a_locked_store(self):
        self.arm("north")
        fake, calls = self.locked(1, schedule._run_once)
        with unittest.mock.patch.object(schedule, "_run_once", side_effect=fake), \
                self.assertRaisesRegex(sqlite3.OperationalError, "database is locked"):
            schedule.run_lanes(["take", "--strand", "welle-x/a", "--lane", "north"],
                               env=self.lanes_env(), cwd=self._tmp.name)
        self.assertEqual(1, len(calls))

    def test_take_wait_refuses_nonsense_wait_seconds(self):
        # GH #1155 (2): `take --wait` read MECLAW_TOKEN_WAIT_SECS unchecked.
        env = dict(os.environ, MECLAW_PIPELINE_DB=os.path.join(self._tmp.name, "pipeline.db"))
        for raw in ("abc", "-1", "0", "nan"):
            code, out, err = schedule.run_lanes(
                ["take", "--wait", "--strand", "welle-x/a"],
                env=dict(env, MECLAW_TOKEN_WAIT_SECS=raw), cwd=self._tmp.name)
            self.assertEqual(2, code, (raw, out, err))
            self.assertIn("MECLAW_TOKEN_WAIT_SECS=%s" % raw, err)

    def test_nonsense_wait_seconds_are_refused(self):
        self.arm("north")
        self.assertEqual(0, schedule.lease_take(self.db, "welle-x/b", lane="north", now=T0).code)
        for name in ("MECLAW_TOKEN_WAIT_SECS", "MECLAW_ADMISSION_WAIT_SECS"):
            for raw in ("0", "abc", "nan", "-1", "inf"):
                exc = self.wait(env={name: raw})
                self.assertIsInstance(exc, schedule.ScheduleError, (name, raw, exc))
                self.assertEqual(2, exc.code)
                self.assertIn("%s=%s" % (name, raw), str(exc))


class TestBrowserLanes(ScheduleCase):
    """GH #1161, OR-TB.S3.17: `browser:display` goes only to a lane whose engines
    start. build02 could not load libGL.so.1 for WebKit, every WebKit proof SKIPped,
    and the station read GREEN; the lane's fingerprint says so now, per engine."""

    wait = TestAdmitWaitingEnds.wait
    WEBKIT_DOWN = {"chromium": "ok",
                   "webkit": "libraries not found: libGL.so.1 (webkit-2359 minibrowser-wpe)"}

    def browser_plan(self):
        return make_plan(station("browser:display", cpu=4, colony=3,
                                 browser_chromium=1, browser_webkit=1))

    def probed(self, name, browsers):
        self.db.execute("UPDATE runners SET fingerprint = ? WHERE name = ?",
                        (json.dumps({"vcpu": 24, "browsers": browsers}), name))

    def test_a_lane_whose_webkit_does_not_start_serves_no_webkit(self):
        lanes = self.lanes("north", "south")
        self.probed("north", self.WEBKIT_DOWN)
        self.probed("south", {"chromium": "ok", "webkit": "ok"})
        schedule.sync_runners(self.db, lanes)
        cap = json.loads(self.runner("north")["capacity"])
        self.assertEqual((1, 0), (cap["browser_chromium"], cap["browser_webkit"]))
        self.assertEqual("south", self.admit(self.browser_plan()).lane)

    def test_the_dispatch_names_the_lane_root(self):
        """GH #1263: the executor finds the lane's core tools for a tree without its own
        under the root the admission names -- never a guess from the tree's path."""
        self.lanes("north", root="/srv/lane-n/")
        self.probed("north", {"chromium": "ok", "webkit": "ok"})
        adm = self.admit(self.browser_plan())
        self.assertEqual("/srv/lane-n", adm.dispatch["+lane_root"])
        self.assertTrue(adm.dispatch["tree"].startswith("/srv/lane-n/"), adm.dispatch["tree"])

    def test_no_lane_that_starts_webkit_refuses_with_the_reason(self):
        lanes = self.lanes("north")
        self.probed("north", self.WEBKIT_DOWN)
        schedule.sync_runners(self.db, lanes)
        q = self.wait(secs=0.05, plan=self.browser_plan())
        self.assertIsInstance(q, schedule.Queued, q)
        self.assertTrue(q.final, q.reason)
        self.assertIn("browser_webkit 1 > 0", q.reason)
        self.assertIn("libGL.so.1", q.reason)

    def foreign_browsers(self):
        """The real `lane_fingerprint.sh --browsers` over a fake satellite tree without
        `workshop/tools` (the orga-hive tree of 16.10.): every engine refused."""
        tree = os.path.join(self._tmp.name, "orga-tree")
        os.makedirs(os.path.join(tree, "docs"))
        script = os.path.join(os.path.dirname(schedule.__file__), "lane_fingerprint.sh")
        res = subprocess.run(["sh", script, "--browsers", tree], check=True,
                             capture_output=True, text=True, timeout=kit_time.HANG)
        said = json.loads(res.stdout)
        self.assertEqual({"chromium", "webkit"}, set(schedule.browser_refusals({"browsers": said})))
        return said

    def run_on(self, repo, fingerprint, plan=None):
        plan = plan or light_plan()
        adm = schedule.admit(self.db, plan, repo=repo, mode="test", strand="welle-x/a",
                             wave="welle-x", base=plan["base"], lane_pref=None, now=T0, wait=False)
        self.assertNotIsInstance(adm, schedule.Queued, adm)
        schedule.ingest(self.db, adm, ev("lane_fingerprint", dict(fingerprint, vcpu=24), adm.run,
                                         kind="note"))
        self.finish(adm, {"unittests": "GREEN"})
        return adm

    def test_a_foreign_tree_never_refuses_the_browsers_of_the_core(self):
        """#1199 K13 (16.10.): an orga-hive pass (a tree without `workshop/tools`) ran on
        build04 after a meclaw pass; its fingerprint said "no playwright in
        workshop/tools/node_modules (npm ci there)", the register took it and the
        admission refused every meclaw browser pass there ("can never take the pass").
        Two trees: the meclaw tree's probe said ok, the satellite's comes from the
        real script. The meclaw browser pass is admitted after the satellite's run."""
        lanes = self.lanes("north")
        self.run_on("meclaw", {"playwright": "1.63.0", "browsers": {"chromium": "ok", "webkit": "ok"}})
        foreign = self.foreign_browsers()
        self.run_on("orga", {"playwright": None, "browsers": foreign})
        schedule.sync_runners(self.db, lanes)  # as every `lanes status` and admission round
        cap = json.loads(self.runner("north")["capacity"])
        self.assertEqual((1, 1), (cap["browser_chromium"], cap["browser_webkit"]))
        adm = self.admit(self.browser_plan())
        self.assertNotIsInstance(adm, schedule.Queued, adm)
        self.assertEqual("north", adm.lane)
        # the core's run expects its own tree, the satellite's its own (`register_view`)
        self.assertEqual("1.63.0", adm.dispatch["fingerprint_expected"]["playwright"])
        self.assertNotIn("repos", adm.dispatch["fingerprint_expected"])

    def test_a_satellite_run_expects_its_own_tree_fields(self):
        # the class of K13: every field the tree decides (TREE_FIELDS), not only browsers
        self.lanes("north")
        self.run_on("meclaw", {"rustc": "rustc 1.98.0", "playwright": "1.63.0", "kernel": "6.8"})
        self.run_on("orga", {"rustc": "rustc 1.90.0", "playwright": None, "kernel": "6.9"})
        reg = json.loads(self.runner("north")["fingerprint"])
        self.assertEqual(("rustc 1.98.0", "1.63.0", "6.9"),
                         (reg["rustc"], reg["playwright"], reg["kernel"]))
        sat = self.run_on("orga", {"rustc": "rustc 1.90.0", "playwright": None, "kernel": "6.9"})
        want = sat.dispatch["fingerprint_expected"]
        self.assertEqual(("rustc 1.90.0", None, "6.9"),
                         (want["rustc"], want["playwright"], want["kernel"]))
        core = self.admit(light_plan())
        self.assertEqual("rustc 1.98.0", core.dispatch["fingerprint_expected"]["rustc"])

    def test_a_run_that_checked_no_browsers_keeps_the_lanes_value(self):
        """#1199 K14 (16.10.: all four lanes, five hand fixes): a docs-only meclaw run
        and a satellite run without a browser station wrote the browsers of their tree
        over the lane's. Their fingerprint carries no `browsers` now (the executor
        probes only for a run that books an engine), and the register keeps what the
        last check said: "ok" stays ok, and the browser pass is admitted."""
        lanes = self.lanes("north")
        ok = {"chromium": "ok", "webkit": "ok"}
        self.run_on("meclaw", {"playwright": "1.63.0", "browsers": ok})
        self.run_on("orga", {"playwright": None, "browsers": self.WEBKIT_DOWN})
        self.run_on("meclaw", {"playwright": "1.63.0"})          # docs-only
        self.run_on("orga", {"playwright": None})                 # satellite, docs-only
        reg = json.loads(self.runner("north")["fingerprint"])
        self.assertEqual(ok, reg["browsers"])
        self.assertEqual(self.WEBKIT_DOWN, reg["repos"]["orga"]["browsers"])
        schedule.sync_runners(self.db, lanes)
        adm = self.admit(self.browser_plan())
        self.assertNotIsInstance(adm, schedule.Queued, adm)
        self.assertEqual("north", adm.lane)

    def test_a_stale_browser_refusal_is_measured_once_before_it_refuses(self):
        """#1199 K14: a refusal "browsers refused" read a register value that a run
        had written before the lane was provisioned. The admission measures the lane
        once afresh before it refuses for good: a fresh "ok" admits the pass, a fresh
        refusal refuses with its own reason -- and the next round measures no more."""
        lanes = self.lanes("north")
        self.probed("north", self.WEBKIT_DOWN)
        schedule.sync_runners(self.db, lanes)
        said = [{"vcpu": 24, "browsers": {"chromium": "ok", "webkit": "ok"}}]
        asked = []

        def fresh(ln):
            asked.append(ln.name)
            return said[0]

        self.addCleanup(setattr, schedule, "_browser_remeasure", schedule._browser_remeasure)
        schedule._browser_remeasure = fresh
        adm = self.admit(self.browser_plan())
        self.assertNotIsInstance(adm, schedule.Queued, adm)
        self.assertEqual(("north", ["north"]), (adm.lane, asked))
        cap = json.loads(self.runner("north")["capacity"])
        self.assertEqual((1, 1), (cap["browser_chromium"], cap["browser_webkit"]))
        self.finish(adm, {"browser:display": "GREEN"})
        # still refused after the fresh measure: the refusal stands, measured once
        self.probed("north", self.WEBKIT_DOWN)
        schedule.sync_runners(self.db, lanes)
        said[0] = {"vcpu": 24, "browsers": dict(self.WEBKIT_DOWN, webkit="still no libGL.so.1")}
        q = self.wait(secs=0.05, plan=self.browser_plan())
        self.assertIsInstance(q, schedule.Queued, q)
        self.assertTrue(q.final, q.reason)
        self.assertIn("still no libGL.so.1", q.reason)
        self.assertEqual(["north", "north"], asked)
        q = self.admit(self.browser_plan())     # the register is the fresh answer now
        self.assertIsInstance(q, schedule.Queued, q)
        self.assertEqual(["north", "north"], asked)

    def test_tree_fields_cover_the_build_part(self):
        from scripts.pipeline import executor
        self.assertLessEqual(set(executor.BUILD_PART), set(schedule.TREE_FIELDS))
        self.assertLessEqual(set(schedule.TREE_FIELDS), set(executor.FINGERPRINT_FIELDS))

    def test_a_lane_not_yet_probed_keeps_its_engines(self):
        # Before its first run with the probe a lane is not refused: the executor
        # probes before the start, and a booked engine that does not start is red there.
        self.lanes("north")
        cap = json.loads(self.runner("north")["capacity"])
        self.assertEqual((1, 1), (cap["browser_chromium"], cap["browser_webkit"]))


class TestColonyAdmission(ScheduleCase):
    """welle-tb S3 F1 (re-check tb-s3 h1 M-1, m-2): the admission books colonies from
    the lane's ONE source (`cores=` -> `colony_capacity`), and a satellite pass never
    lands on the owner's machine."""

    def sat_admit(self, plan, lane_pref=None, mode="test"):
        return schedule.admit(self.db, plan, repo="orga", mode=mode, strand="welle-x/a",
                              wave="welle-x", base=plan["base"], lane_pref=lane_pref,
                              now=T0, wait=False)

    def test_memory_books_the_rest_beside_browser_on_a_six_colony_lane(self):
        # m-2: `_demand_on(..., cap.colony)` resolves COLONY_REST to 6 - 3 = 3 there;
        # unresolved (-1) a second pass of builder 4 would slip in beside it (3+4 > 6).
        schedule.sync_runners(self.db, [FakeLane("north", threads=12, cores=6)],
                              states={"north": "idle"})
        self.assertEqual(6, json.loads(self.runner("north")["capacity"])["colony"])
        first = self.admit(make_plan(station("scenarios:memory", cpu=4,
                                             colony=plan_mod.COLONY_REST)))
        self.assertEqual("north", first.lane)
        live = schedule._live_runs(self.db, schedule._now(T0))
        self.assertEqual([3], [r["demand"]["colony"] for r in live])
        second = self.admit(make_plan(station("scenarios:builder", cpu=2, colony=4)),
                            strand="welle-x/b")
        self.assertIsInstance(second, schedule.Queued, second)
        self.assertIn("colony 3+4 > 6", second.reason)

    def test_an_egon_lane_never_takes_a_satellite_pass(self):
        # M-1 (Koordinator): satellites never run on mm-os01; the admission books a lane.
        schedule.sync_runners(self.db, [FakeLane("north", threads=12, cores=6),
                                        FakeLane("mm", transport="local", cls="egon")],
                              states={"north": "idle", "mm": "idle"})
        sat = make_plan(station("unit", cpu=4, colony=3))
        q = self.sat_admit(sat, lane_pref="mm")
        self.assertIsInstance(q, schedule.Queued, q)
        self.assertIn("egon lane mm never takes a satellite pass (orga)", q.reason)
        self.assertEqual("north", self.sat_admit(sat).lane)


class TestRounds(ScheduleCase):
    def test_schedule_reuses_workpackage_for_round_two(self):
        self.lanes("north")
        first = self.admit(cargo_plan())
        self.assertRegex(first.workpackage, r"^P-\d+$")
        self.assertEqual(1, first.round)
        self.finish(first, {"precheck": "GREEN", "build": "GREEN", "tests": "RED",
                            "export-audit": "GREEN"}, pass_verdict="RED")
        self.assertEqual(("finished", "red"), (self.wp(first)["state"], self.wp(first)["reason"]))
        second = self.admit(cargo_plan(), minutes=70)
        self.assertEqual(first.workpackage, second.workpackage)
        self.assertEqual(2, second.round)
        self.assertEqual(first.tasks, second.tasks, "the same tasks, a new attempt each")
        self.assertEqual(("active", "round"), (self.wp(second)["state"], self.wp(second)["reason"]))
        runs = self.db.execute("SELECT attempt, round FROM task_runs WHERE item = ? ORDER BY attempt",
                               (second.tasks["tests"],)).fetchall()
        self.assertEqual([(1, 1), (2, 2)], [tuple(r) for r in runs])
        self.assertEqual("in_progress", self.item(second.tasks["tests"])["state"])
        other = self.admit(cargo_plan(base="c" * 40), minutes=71, strand="welle-x/a")
        self.assertIsInstance(other, schedule.Queued, "same tree, still live")
        self.finish(second, {"precheck": "GREEN", "build": "GREEN", "tests": "GREEN",
                             "export-audit": "GREEN"})
        other = self.admit(cargo_plan(base="c" * 40), minutes=140)
        self.assertNotEqual(first.workpackage, other.workpackage, "a new base is a new pass")
        self.assertEqual(1, other.round)

    def test_schedule_a_stale_round_never_closes_the_next_one(self):
        # Delta A M-neu-2: a kern that outlived its lost round closes late --
        # the workpackage is the next round's now and stays active.
        self.lanes("north")
        first = self.admit(cargo_plan())
        schedule.close(self.db, first, None, lost=True, now=T0 + 60)
        second = self.admit(cargo_plan(), minutes=300)
        self.assertIsInstance(second, schedule.Admission, getattr(second, "reason", ""))
        self.assertEqual(first.workpackage, second.workpackage)
        self.finish(first, {"precheck": "GREEN", "build": "GREEN", "tests": "GREEN",
                            "export-audit": "GREEN"})
        self.assertEqual(("active", "round"), (self.wp(second)["state"], self.wp(second)["reason"]))
        schedule.close(self.db, first, None, lost=True, now=T0 + 3700)
        self.assertEqual(("active", "round"), (self.wp(second)["state"], self.wp(second)["reason"]))
        self.assertEqual("busy", self.runner("north")["state"])

    def test_schedule_task_doc_exec_has_exactly_orgas_seven_keys(self):
        self.lanes("north")
        adm = self.admit(cargo_plan())
        self.assertEqual({"precheck", "build", "tests", "export-audit"}, set(adm.tasks))
        wp = self.wp(adm)
        self.assertEqual("workpackage", wp["type"])
        doc = json.loads(wp["doc"])
        for key in ("repo", "mode", "stage", "strand", "wave", "rev", "base", "tree", "dirty",
                    "lane", "archive", "plan", "owner_decisions", "plan_reason"):
            self.assertIn(key, doc)
        self.assertEqual(("meclaw", "strand", "strand", "welle-x/a", BASE),
                         (doc["repo"], doc["mode"], doc["stage"], doc["strand"], doc["base"]))
        self.assertEqual(["precheck", "build", "tests", "export-audit"], doc["plan"])
        for name, item_id in adm.tasks.items():
            task = self.item(item_id)
            self.assertEqual("task", task["type"])
            self.assertRegex(task["key"], r"^T-\d+$")
            self.assertEqual(wp["id"], task["parent"])
            tdoc = json.loads(task["doc"])
            self.assertEqual(set(schedule.EXEC_KEYS), set(tdoc["exec"]))
            self.assertEqual(7, len(tdoc["exec"]))
            self.assertEqual(0, tdoc["exec"]["retries"])
            self.assertEqual(["log"], tdoc["exec"]["evidence"])
            self.assertEqual("north", tdoc["exec"]["runner"])
            self.assertEqual(name, tdoc["station"]["name"])
            self.assertEqual(tdoc["station"]["cmds"][0]["argv"], tdoc["exec"]["test_argv"])
            self.assertEqual("/srv/target/ws/%s" % adm.run, tdoc["exec"]["workspace"])
            self.assertEqual("/srv/target/tree", tdoc["exec"]["root"])
            tr = self.task_run(adm.task_runs[name])
            self.assertRegex(tr["id"], r"^tr-[0-9a-f]{12}$")
            self.assertEqual(("dispatched", 1, 1, "north"),
                             (tr["state"], tr["attempt"], tr["round"], tr["runner"]))
        self.assertRegex(adm.run, r"^run-[0-9a-f]{12}$")
        edge = self.db.execute("SELECT data FROM edges WHERE src = ? AND dst = ? AND kind = 'depends_on'",
                               (adm.tasks["tests"], adm.tasks["build"])).fetchone()
        self.assertEqual({"dep": "fs", "lag_min": 0}, json.loads(edge["data"]))
        edge = self.db.execute("SELECT data FROM edges WHERE src = ? AND dst = ? AND kind = 'depends_on'",
                               (adm.tasks["export-audit"], adm.tasks["tests"])).fetchone()
        self.assertEqual({"dep": "fs", "lag_min": 0, "when": {"outcome": "any"}}, json.loads(edge["data"]))
        dispatched = self.journal("run_dispatched")
        self.assertEqual(4, len(dispatched))
        for row in dispatched:
            for key in ("run", "runner", "tree", "attempt", "round", "deadline_at", "ws"):
                self.assertIn(key, row)
        self.assertEqual("busy", self.runner("north")["state"])


class TestIngest(ScheduleCase):
    VERDICTS = {
        "g": ("GREEN", None, "done", None, "done"),
        "c": ("CACHED", None, "done", None, "done"),
        "f": ("FLAKY", None, "failed", "flaky", "waiting"),   # a finding (VG TV-F1)
        "fl": ("FLAKY", None, "done", None, "done"),          # every test legacy (VG TV-F4)
        "n": ("NOTE", None, "done", None, "done"),
        "s": ("SKIP", "precheck red", "done", None, "done"),
        "r": ("RED", None, "failed", "red", "waiting"),
        "a": ("ASK", None, "failed", "ask", "waiting"),
        "ab": ("ABORTED", "aborted_disk", "failed", "aborted_disk", "waiting"),
        "hy": ("ABORTED", "hygiene", "failed", "hygiene", "waiting"),
        "to": ("RED", "timeout", "timed_out", "timeout", "waiting"),
        # The executor's reason since #1150 names the log.
        "tl": ("RED", "timeout -- log logs/tl.log", "timed_out", "timeout", "waiting"),
        # TV-K5: a cap is a hang marker, its reason `HANG: station <n> exceeded <m> min`.
        "hg": ("RED", "HANG: station hg exceeded 20 min -- log logs/hg.log", "timed_out",
               "timeout", "waiting"),
    }

    def test_schedule_maps_verdicts_to_task_run_states(self):
        self.lanes("north")
        adm = self.admit(make_plan(*[station(n) for n in self.VERDICTS]))
        for name, (verdict, reason, _s, _r, _t) in self.VERDICTS.items():
            data = {"station": name, "verdict": verdict, "secs": 3.0, "exit": 0}
            if reason:
                data["reason"] = reason
            if name == "fl":
                data["legacy"] = True
            out = schedule.ingest(self.db, adm, ev("station_ended", data, adm.run))
            self.assertEqual("station_ended", out["event"])
        for name, (verdict, _reason, state, reason, task_state) in self.VERDICTS.items():
            tr = self.task_run(adm.task_runs[name])
            self.assertEqual((state, reason, verdict), (tr["state"], tr["reason"], tr["verdict"]), name)
            self.assertEqual(task_state, self.item(adm.tasks[name])["state"], name)
        rows = self.db.execute("SELECT * FROM journal WHERE run = ? AND event = 'station_ended'",
                               (adm.run,)).fetchall()
        self.assertEqual(len(self.VERDICTS), len(rows))
        self.assertEqual(adm.tasks["r"], [r for r in rows if json.loads(r["data"])["station"] == "r"][0]["item"])

    def test_schedule_test_results_and_cache_hits_become_result_rows(self):
        self.lanes("north")
        tests = station("tests")
        tests["nextest"] = {"profile": "default", "filter": "all()"}
        adm = self.admit(make_plan(tests, station("fmt")))
        schedule.ingest(self.db, adm, ev("station_started", {"station": "tests", "needs": {}, "cmds": 1},
                                         adm.run))
        schedule.ingest(self.db, adm, ev("test_result", {
            "station": "tests", "part": "meclaw-cells::gh1", "verdict": "RED", "secs": 2.0,
            "tests": {"total": 3, "passed": 2, "failed": 1, "flaky": 0, "skipped": 0},
            "failed_names": ["the_case"]}, adm.run, kind="note"))
        schedule.ingest(self.db, adm, ev("cache_hit", {
            "station": "fmt", "part": "", "key": "k" * 64, "from_run": "run-000000000001"},
            adm.run, kind="note"))
        rows = {r["call"]: r for r in self.db.execute("SELECT * FROM task_run_results")}
        self.assertEqual({"tests/meclaw-cells::gh1", "fmt/"}, set(rows))
        red = rows["tests/meclaw-cells::gh1"]
        self.assertEqual(adm.task_runs["tests"], red["run"])
        self.assertEqual("nextest-binary", red["tool_name"])
        entry = json.loads(red["entry"])
        self.assertEqual(("meclaw-cells::gh1", "RED", ["the_case"]),
                         (entry["part"], entry["verdict"], entry["failed_names"]))
        cached = json.loads(rows["fmt/"]["entry"])
        self.assertEqual(("CACHED", "run-000000000001"), (cached["verdict"], cached["cache_from"]))
        self.assertEqual("run-000000000001", self.task_run(adm.task_runs["fmt"])["cache_from"])
        self.assertIsNotNone(self.task_run(adm.task_runs["tests"])["started_at"])

    def test_schedule_fingerprint_lands_on_the_runner(self):
        self.lanes("north")
        adm = self.admit(light_plan())
        schedule.ingest(self.db, adm, ev("lane_fingerprint", {"vcpu": 48, "rustc": "1.90"},
                                         adm.run, kind="note"))
        self.assertEqual(48, json.loads(self.runner("north")["fingerprint"])["vcpu"])

    def test_a_fingerprint_the_lane_did_not_answer_keeps_the_register(self):
        """Re-check tv-bn M7r A-M1 (fail-open): a run whose lane_fingerprint.sh did not
        answer (`unanswered`) overwrote the register with nulls; the next run keyed its
        cache over null tools and null == null raised no doubt. The register keeps
        what the lane last said."""
        self.lanes("north")
        adm = self.admit(light_plan())
        schedule.ingest(self.db, adm, ev("lane_fingerprint", {"vcpu": 48, "rustc": "1.90"},
                                         adm.run, kind="note"))
        schedule.ingest(self.db, adm, ev("lane_fingerprint",
                                         {"vcpu": None, "rustc": None, "unanswered": "exit 3"},
                                         adm.run, kind="note"))
        self.assertEqual("1.90", json.loads(self.runner("north")["fingerprint"])["rustc"])


class TestClose(ScheduleCase):
    def test_schedule_close_writes_calibration_and_frees_the_runner(self):
        self.lanes("north")
        adm = self.admit(cargo_plan())
        self.finish(adm, {"precheck": "GREEN", "build": "GREEN", "tests": "CACHED",
                          "export-audit": "GREEN"})
        self.assertEqual(("finished", "green"), (self.wp(adm)["state"], self.wp(adm)["reason"]))
        cal = {r["key"]: r for r in self.db.execute("SELECT * FROM calibration")}
        self.assertEqual({"precheck@meclaw", "build@meclaw", "export-audit@meclaw"}, set(cal),
                         "a CACHED station took no time worth calibrating")
        self.assertEqual(12.5, cal["build@meclaw"]["actual_s"])
        self.assertEqual("north", cal["build@meclaw"]["executing"])
        self.assertEqual(adm.tasks["build"], cal["build@meclaw"]["item"])
        self.assertEqual("idle", self.runner("north")["state"])

    def test_schedule_lost_run_keeps_lease_and_lane(self):
        lanes_file = os.path.join(self._tmp.name, "lanes")
        with open(lanes_file, "w") as fh:
            fh.write("north builder@north.example.invalid\n")
        self.lanes("north")
        schedule.lease_init(self.db, lanes="north", lanes_file=lanes_file, now=T0)
        adm = self.admit(cargo_plan())
        self.assertTrue(adm.lease_taken)
        schedule.ingest(self.db, adm, ev("station_ended", {"station": "precheck", "verdict": "GREEN",
                                                           "secs": 1.0, "exit": 0}, adm.run))
        schedule.close(self.db, adm, None, lost=True, now=T0 + 600)
        self.assertEqual(("stopped", "lost"), (self.wp(adm)["state"], self.wp(adm)["reason"]))
        lease = self.db.execute("SELECT * FROM leases WHERE resource = 'lane:north'").fetchone()
        self.assertEqual(("welle-x/a", adm.run), (lease["holder"], lease["run"]))
        tr = self.task_run(adm.task_runs["build"])
        self.assertEqual(("failed", "lost", 1), (tr["state"], tr["reason"], tr["lost"]))
        self.assertEqual("done", self.task_run(adm.task_runs["precheck"])["state"])
        self.assertEqual("busy", self.runner("north")["state"])
        again = self.admit(cargo_plan(), minutes=20)
        self.assertIsInstance(again, schedule.Queued, "the lost run may still build in tree/")


class TestNeedsField(unittest.TestCase):
    def test_schedule_demand_reads_needs_not_the_legacy_cargo_flag(self):
        st = {"name": "tests", "cargo": True, "needs": {"cpu": 8, "cargo": 0}}
        self.assertEqual(schedule._needs(st)["cargo"], 0)
        self.assertEqual(schedule._needs({"name": "old", "cargo": 1})["cargo"], 1)


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


class TreeKey(unittest.TestCase):
    """GH #1290: `strand.sh new` puts a strand at `$MECLAW_WORKTREE_ROOT/<repo>/<wave>/<name>`
    when the variable is set; there the directory name repeats across waves, while the kit
    keyed a tree by it (`<target>/gate/<tree>` in the one shared `target/`, the lane tree
    `wt/<tree>` with its lock and ref). `schedule.tree_key` is the one rule; the shell mirror
    in gate_legacy.sh is pinned on the same rows (test_gate_sh.TreeKeyMirror)."""

    ROOT = "/srv/wt-root"
    ROWS = [
        ("/x/meclaw-core-wt-s/", ROOT, "meclaw-core-wt-s"),
        ("/x/meclaw-core-wt-s", None, "meclaw-core-wt-s"),
        ("/x/meclaw-core", ROOT, "meclaw-core"),
        (ROOT + "/meclaw-core/wave-a/fix", ROOT, "wave-a--fix"),
        (ROOT + "/meclaw-core/wave-a/fix/", ROOT + "/", "wave-a--fix"),
        (ROOT + "/meclaw-core/wave-b/fix", ROOT, "wave-b--fix"),
        (ROOT + "/meclaw-core/wave-a/fix", None, "fix"),
        (ROOT + "/meclaw-core/wave-a/fix", "", "fix"),
        (ROOT + "/meclaw-core/loose", ROOT, "loose"),
        (ROOT + "-other/meclaw-core/wave-a/fix", ROOT, "fix"),
        ("fix", ROOT, "fix"),
    ]

    def test_every_row_of_the_table(self):
        for path, root, want in self.ROWS:
            env = {} if root is None else {schedule.WORKTREE_ROOT_ENV: root}
            with self.subTest(path=path, root=root):
                self.assertEqual(want, schedule.tree_key(path, env=env))

    def test_the_lane_tree_name_takes_the_key(self):
        tree = self.ROOT + "/meclaw-core/wave-a/pass2"
        with unittest.mock.patch.dict(os.environ, {schedule.WORKTREE_ROOT_ENV: self.ROOT}):
            self.assertEqual("wave-a--pass2", schedule.lane_tree_name("meclaw", tree))
            self.assertEqual("orga@wave-a--pass2",
                             schedule.lane_tree_name("orga", self.ROOT + "/orga/wave-a/pass2"))
        with unittest.mock.patch.dict(os.environ, {}):
            os.environ.pop(schedule.WORKTREE_ROOT_ENV, None)
            self.assertEqual("pass2", schedule.lane_tree_name("meclaw", tree))

    def test_the_gate_folder_of_a_tree_is_keyed(self):
        env = {schedule.WORKTREE_ROOT_ENV: self.ROOT}
        self.assertEqual("/t/gate/wave-a--fix",
                         schedule.tree_gate_dir("/t", self.ROOT + "/meclaw-core/wave-a/fix", env=env))
        self.assertEqual("/t/gate/meclaw-core", schedule.tree_gate_dir("/t", "/x/meclaw-core/", env=env))


if __name__ == "__main__":
    unittest.main()
