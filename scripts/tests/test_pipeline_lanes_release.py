"""Tests for `pipeline.py lanes release-run <lane> --run <run>` (`strand.sh lanes release`).

An orphaned run: its driver ended without `schedule.close()`, its task runs
stay `dispatched` and its workpackage `active` until deadline + grace, so the
admission keeps the lane busy. The verb closes such a run fail-closed: only
when no local driver pid of the run lives and the lane shows neither the
run's `pipe-<run>.scope` nor a process in its workspace.

Everything runs in-process against a throw-away store; the lane probes come
from a fake lane module (`schedule._lane_override`), the pid probe is patched
(`schedule._alive`) -- no ssh, no systemctl, no signal.
"""
import dataclasses
import json
import os
import sqlite3
import tempfile
import unittest
from typing import Optional
from unittest import mock

from scripts.pipeline import schedule, store
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
    threads: Optional[int] = 24
    jobs: Optional[int] = None
    slots: Optional[int] = None


class FakeLaneModule:
    """The probes of `lane.py` the verb asks: `unit_state` and `ws_procs`."""

    def __init__(self, names, unit="inactive", procs=()):
        self.lanes = [FakeLane(n) for n in names]
        self.unit, self.procs = unit, list(procs)
        self.asked = []

    def load_lanes(self, path=None):
        return list(self.lanes)

    def unit_state(self, lane, run, timeout=kit_time.HANG):
        self.asked.append(("unit", lane.name, run))
        return self.unit

    def ws_procs(self, lane, ws, timeout=kit_time.HANG):
        self.asked.append(("ws", lane.name, ws))
        return list(self.procs)


def station(name, cpu=1, cargo=0, after=()):
    return {"name": name, "scope": name, "cargo": cargo, "stage": ["S", "I", "R"],
            "needs": {"cpu": cpu, "cargo": cargo}, "inputs": [],
            "cmds": [{"argv": ["true", name], "cwd": ""}], "env": "none", "reason": [],
            "timeout_min": 30, "after": list(after), "run": True, "keep_ws": True}


def cargo_plan():
    stations = [station("precheck"), station("build", cpu=8, cargo=1, after=["precheck"]),
                station("tests", cpu=8, after=["build"]),
                station("slow-tests", cpu=8, after=["build"])]
    return {"mode": "strand", "rev": REV, "base": BASE, "stations": stations,
            "plan_reason": {s["name"]: ["test"] for s in stations}}


class ReleaseRunCase(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.db_path = os.path.join(self._tmp.name, "state", "pipeline.db")
        self.lanes_file = os.path.join(self._tmp.name, "lanes")
        with open(self.lanes_file, "w") as fh:
            fh.write("north builder@north.example.invalid\n")
        self.db = store.open(self.db_path)
        schedule.sync_runners(self.db, [FakeLane("north")], states={"north": "idle"})
        self.fake = FakeLaneModule(["north"])
        self._override = mock.patch.object(schedule, "_lane_override", self.fake)
        self._override.start()

    def tearDown(self):
        self._override.stop()
        self.db.close()
        self._tmp.cleanup()

    def admit(self, strand="welle-x/a", minutes=0):
        plan = cargo_plan()
        return schedule.admit(self.db, plan, repo="meclaw", mode="strand", strand=strand,
                              wave="welle-x", base=plan["base"], lane_pref=None,
                              now=T0 + 60 * minutes, wait=False)

    def orphan(self):
        """A run whose driver died after precheck and build: tests and slow-tests
        stay dispatched, the workpackage active."""
        adm = self.admit()
        for name in ("precheck", "build"):
            schedule.ingest(self.db, adm, json.dumps({
                "event": "station_ended", "kind": "status", "at": "2027-01-15T08:00:00Z",
                "run": adm.run, "data": {"station": name, "verdict": "GREEN", "secs": 3.0,
                                         "exit": 0, "scope": name, "run": adm.run}}))
        return adm

    def release(self, run, lane="north", minutes=5):
        env = {"MECLAW_PIPELINE_DB": self.db_path, "MECLAW_LANES_FILE": self.lanes_file,
               "MECLAW_STRAND_NOW": str(T0 + 60 * minutes)}
        return schedule.run_lanes(["release-run", lane, "--run", run, "--by", "tester"],
                                  env, self._tmp.name)

    def wp(self, adm):
        return self.db.execute("SELECT * FROM items WHERE key = ?", (adm.workpackage,)).fetchone()

    def task_run(self, tr):
        return self.db.execute("SELECT * FROM task_runs WHERE id = ?", (tr,)).fetchone()

    def journal(self, event):
        return [json.loads(r["data"]) for r in self.db.execute(
            "SELECT data FROM journal WHERE event = ? ORDER BY seq", (event,))]

    def assert_untouched(self, adm):
        self.assertEqual(("active",), (self.wp(adm)["state"],))
        for name in ("tests", "slow-tests"):
            self.assertEqual("dispatched", self.task_run(adm.task_runs[name])["state"])
        self.assertEqual([], self.journal("run_closed"))
        self.assertIsInstance(self.admit(strand="welle-x/b", minutes=6), schedule.Queued,
                              "a refused release leaves the lane held")


class TestReleaseRun(ReleaseRunCase):
    def test_an_orphaned_run_is_closed_and_frees_the_lane(self):
        adm = self.orphan()
        self.assertIsInstance(self.admit(strand="welle-x/b", minutes=1), schedule.Queued,
                              "the orphan holds tree/ of the lane")
        with mock.patch.object(schedule, "_alive", lambda pid: False):
            code, out, err = self.release(adm.run)
        self.assertEqual(0, code, err)
        self.assertIn(adm.run, out)
        self.assertEqual(("stopped", "aborted"), (self.wp(adm)["state"], self.wp(adm)["reason"]))
        for name in ("tests", "slow-tests"):
            tr = self.task_run(adm.task_runs[name])
            self.assertEqual(("failed", "orphan"), (tr["state"], tr["reason"]))
        self.assertEqual("done", self.task_run(adm.task_runs["build"])["state"])
        closed = self.journal("run_closed")
        self.assertEqual(1, len(closed))
        self.assertEqual((adm.run, "north", "orphan", "tester"),
                         (closed[0]["run"], closed[0]["runner"], closed[0]["reason"],
                          closed[0]["by"]))
        self.assertEqual(sorted([adm.task_runs["tests"], adm.task_runs["slow-tests"]]),
                         sorted(closed[0]["task_runs"]))
        self.assertIn(("unit", "north", adm.run), self.fake.asked)
        self.assertIn(("ws", "north", adm.ws), self.fake.asked)
        runner = self.db.execute("SELECT state FROM runners WHERE name = 'north'").fetchone()
        self.assertEqual("idle", runner["state"])
        self.assertIsInstance(self.admit(strand="welle-x/b", minutes=6), schedule.Admission,
                              "the lane is free in the admission")
        code, _, err = self.release(adm.run, minutes=7)
        self.assertNotEqual(0, code, "a closed run has nothing left to close")
        self.assertIn("nothing open", err)

    def test_a_live_driver_pid_refuses(self):
        adm = self.orphan()
        doc = json.loads(self.wp(adm)["doc"])
        self.assertEqual(os.getpid(), doc.get("driver_pid"),
                         "the admission records the local driver of the run")
        with mock.patch.object(schedule, "_alive", lambda pid: pid == os.getpid()):
            code, _, err = self.release(adm.run)
        self.assertNotEqual(0, code)
        self.assertIn("driver pid %d" % os.getpid(), err)
        self.assert_untouched(adm)
        self.assertEqual([], self.fake.asked, "a live driver refuses before the lane is asked")

    def test_a_live_lease_pid_refuses(self):
        adm = self.orphan()
        self.db.execute("INSERT INTO lease_entries (pool, holder, resource, since, seen, pids, "
                        "tree, run) VALUES ('cargo', 'welle-x/a', 'lane:north', ?, ?, '[4242]', "
                        "'', ?)", ("2027-01-15T08:00:00Z", "2027-01-15T08:00:00Z", adm.run))
        with mock.patch.object(schedule, "_alive", lambda pid: pid == 4242):
            code, _, err = self.release(adm.run)
        self.assertNotEqual(0, code)
        self.assertIn("driver pid 4242", err)
        self.db.execute("DELETE FROM lease_entries")
        self.assert_untouched(adm)

    def test_an_active_scope_refuses(self):
        adm = self.orphan()
        self.fake.unit = "active"
        with mock.patch.object(schedule, "_alive", lambda pid: False):
            code, _, err = self.release(adm.run)
        self.assertNotEqual(0, code)
        self.assertIn("pipe-%s.scope" % adm.run, err)
        self.assert_untouched(adm)

    def test_a_scope_probe_without_an_answer_refuses(self):
        adm = self.orphan()
        self.fake.unit = None
        with mock.patch.object(schedule, "_alive", lambda pid: False):
            code, _, err = self.release(adm.run)
        self.assertNotEqual(0, code)
        self.assert_untouched(adm)

    def test_a_process_in_the_workspace_refuses(self):
        adm = self.orphan()
        self.fake.procs = [3131]
        with mock.patch.object(schedule, "_alive", lambda pid: False):
            code, _, err = self.release(adm.run)
        self.assertNotEqual(0, code)
        self.assertIn("3131", err)
        self.assert_untouched(adm)

    def test_another_lane_refuses(self):
        adm = self.orphan()
        self.fake.lanes.append(FakeLane("south"))
        with mock.patch.object(schedule, "_alive", lambda pid: False):
            code, _, err = self.release(adm.run, lane="south")
        self.assertNotEqual(0, code)
        self.assertIn("north", err)
        self.assert_untouched(adm)


class TestReleaseLostRun(ReleaseRunCase):
    """#1228 round 3 (16.10.): run-4005045395d9 ended `stopped/lost` on build04 --
    its driver gone, its scope inactive, nothing in its workspace -- and kept the
    lane until its deadline: `_live_runs` counts a lost run, `release-run` said
    "has nothing open" (it knew only `active`), and nothing probed it again.
    A lost run is open: the verb closes it fail-closed, and `take` closes it --
    or an `active` run whose driver died (K13) -- by itself when the lane probe
    finds the run gone (no answer keeps it, as I1)."""

    def setUp(self):
        super().setUp()
        self.assertEqual(0, schedule.lease_init(self.db, lanes="north",
                                                lanes_file=self.lanes_file, now=T0).code)

    def lost(self):
        adm = self.orphan()
        self.assertTrue(adm.lease_taken, "the admission took the lane lease of the run")
        schedule.close(self.db, adm, None, lost=True, now=T0 + 120)
        self.assertEqual(("stopped", "lost"), (self.wp(adm)["state"], self.wp(adm)["reason"]))
        return adm

    def take(self, minutes=6):
        return schedule.lease_take(self.db, "welle-x/b", lane="north", now=T0 + 60 * minutes)

    def holders(self):
        return [r["holder"] for r in self.db.execute("SELECT holder FROM leases")]

    def assert_held(self, adm):
        self.assertEqual(("stopped", "lost"), (self.wp(adm)["state"], self.wp(adm)["reason"]))
        self.assertIn("welle-x/a", self.holders())
        self.assertEqual([], self.journal("run_closed"))

    def test_release_run_closes_a_lost_run_and_frees_its_lease(self):
        adm = self.lost()
        with mock.patch.object(schedule, "_alive", lambda pid: True):
            self.assertEqual(3, self.take().code, "a lost run with a live driver keeps the lane")
        with mock.patch.object(schedule, "_alive", lambda pid: False):
            code, out, err = self.release(adm.run)
        self.assertEqual(0, code, err)
        self.assertNotIn("nothing open", err)
        self.assertEqual(("stopped", "aborted"), (self.wp(adm)["state"], self.wp(adm)["reason"]))
        self.assertNotIn("welle-x/a", self.holders(), "the run's lease goes with its close")
        self.assertIn(("ws", "north", "/srv/target/tree"), self.fake.asked,
                      "the run's tree is probed beside its workspace")
        with mock.patch.object(schedule, "_alive", lambda pid: True):
            res = self.take(minutes=7)
        self.assertEqual(0, res.code, res.err)

    def test_release_run_keeps_a_lost_run_when_the_probe_has_no_answer(self):
        adm = self.lost()
        self.fake.unit = None
        with mock.patch.object(schedule, "_alive", lambda pid: False):
            code, _, err = self.release(adm.run)
        self.assertEqual(2, code, err)
        self.assertIn("no answer", err)
        self.assert_held(adm)

    def test_take_closes_a_lost_run_whose_lane_probes_empty(self):
        adm = self.lost()
        with mock.patch.object(schedule, "_alive", lambda pid: False):
            res = self.take()
        self.assertEqual(0, res.code, res.err)
        self.assertIn(adm.run, res.err)
        self.assertEqual(("stopped", "aborted"), (self.wp(adm)["state"], self.wp(adm)["reason"]))
        closed = self.journal("run_closed")
        self.assertEqual([(adm.run, "lost-probe")], [(c["run"], c["by"]) for c in closed])
        self.assertEqual(["welle-x/b"], self.holders())

    def test_take_keeps_a_lost_run_when_the_probe_has_no_answer(self):
        for unit, procs in ((None, ()), ("active", ()), ("inactive", (3131,))):
            with self.subTest(unit=unit, procs=procs):
                self.tearDown()
                self.setUp()
                adm = self.lost()
                self.fake.unit, self.fake.procs = unit, list(procs)
                with mock.patch.object(schedule, "_alive", lambda pid: False):
                    res = self.take()
                self.assertEqual(3, res.code, res.err)
                self.assert_held(adm)

    def test_take_names_a_probe_fault_and_keeps_the_lost_run(self):
        adm = self.lost()

        def boom(lane, run, timeout=None):
            raise RuntimeError("probe broke")
        with mock.patch.object(schedule, "_alive", lambda pid: False), \
                mock.patch.object(self.fake, "unit_state", boom):
            res = self.take()
        self.assertEqual(3, res.code, res.err)
        self.assertIn("RuntimeError: probe broke", res.err)
        self.assertIn("the close failed", res.err)
        self.assert_held(adm)

    def test_take_keeps_the_lost_run_when_the_no_answer_note_meets_a_locked_store(self):
        # Lease re-check m3: the no-answer note (`events.emit` in the
        # `except ProbeUnanswered`) stood outside the fail-closed net of
        # `_close_dead_runs`. A locked store there ended a `take` without `--wait`
        # (and the golden lease) instead of keeping the run and saying so.
        adm = self.lost()
        self.fake.unit = None
        real = schedule.events.emit

        def emit(db, name, *args, **kw):
            if name == "lease_noted":
                raise sqlite3.OperationalError("database is locked")
            return real(db, name, *args, **kw)
        with mock.patch.object(schedule, "_alive", lambda pid: False), \
                mock.patch.object(schedule.events, "emit", side_effect=emit):
            res = self.take()
        self.assertEqual(3, res.code, res.err)
        self.assertIn("OperationalError: database is locked", res.err)
        self.assertIn("the close failed", res.err)
        self.assert_held(adm)

    def test_take_closes_an_active_run_whose_driver_died(self):
        # driver death (SIGKILL): no `finally` ran, the workpackage stays active
        adm = self.orphan()
        self.assertEqual("active", self.wp(adm)["state"])
        with mock.patch.object(schedule, "_alive", lambda pid: True):
            self.assertEqual(3, self.take().code, "a live driver keeps its run")
        with mock.patch.object(schedule, "_alive", lambda pid: False):
            res = self.take(minutes=7)
        self.assertEqual(0, res.code, res.err)
        self.assertEqual(("stopped", "aborted"), (self.wp(adm)["state"], self.wp(adm)["reason"]))
        self.assertEqual(["welle-x/b"], self.holders())

    def test_take_asks_an_unanswering_lane_once_per_retry_window(self):
        # Re-check v2-r3-lease m3: every take swept a lost run on a dead lane
        # again -- one ssh connect timeout (10 s) per take. The no-answer is
        # remembered per run until `LOST_PROBE_RETRY_S`; the lease stays (fail-closed).
        adm = self.lost()
        retry_min = schedule.LOST_PROBE_RETRY_S // 60
        self.fake.unit = None
        with mock.patch.object(schedule, "_alive", lambda pid: False):
            self.assertEqual(3, self.take(minutes=6).code)
            self.assertEqual(1, len(self.fake.asked), self.fake.asked)
            for minutes in (7, 8, 5 + retry_min):
                self.assertEqual(3, self.take(minutes=minutes).code)
            self.assertEqual(1, len(self.fake.asked), "no answer is not asked again in its window")
            self.assert_held(adm)
            self.fake.unit = "inactive"
            res = self.take(minutes=6 + retry_min)
        self.assertEqual(0, res.code, res.err)
        self.assertEqual(("stopped", "aborted"), (self.wp(adm)["state"], self.wp(adm)["reason"]))

    def test_take_asks_again_a_lane_that_answered(self):
        # the window is for "no answer" only: an answering lane costs no timeout,
        # and a run whose pid just ended is freed at the next take
        adm = self.lost()
        self.fake.procs = [3131]
        with mock.patch.object(schedule, "_alive", lambda pid: False):
            self.assertEqual(3, self.take(minutes=6).code)
            asked = len(self.fake.asked)
            self.fake.procs = []
            res = self.take(minutes=7)
        self.assertEqual(0, res.code, res.err)
        self.assertGreater(len(self.fake.asked), asked)
        self.assertEqual(("stopped", "aborted"), (self.wp(adm)["state"], self.wp(adm)["reason"]))

    def test_take_keeps_a_lost_run_whose_driver_lives(self):
        adm = self.lost()
        with mock.patch.object(schedule, "_alive", lambda pid: True):
            res = self.take()
        self.assertEqual(3, res.code, res.err)
        self.assert_held(adm)
        self.assertEqual([], self.fake.asked, "a live driver keeps the run before the lane is asked")


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()
