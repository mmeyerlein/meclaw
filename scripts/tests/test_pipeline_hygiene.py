"""Tests for the lane hygiene `scripts/pipeline/hygiene.py` (wave dev-tempo W2).

Every case runs against a throw-away lane root in a temp directory, with fake
`systemctl`, `ss`, `systemd-run`, `ssh` and `loginctl` on PATH (they log
their arguments and answer from files) and an injected statvfs/clock -- never
against a real lane, a real scope or a real power switch.

What is pinned here is the CONTRACT the executor (W3) and the strand kit lean
on: the precheck aborts below the reserve before any build, dead workspaces
are collected and every move is one `workspace_changed` event, the watchdog
emits BEFORE it stops the scope it lives in, nothing is ever killed by name,
and nothing at all is removed while a legacy gate holds the host cargo lock.
"""

import errno
import fcntl
import importlib.util
import io
import json
import os
import pathlib
import shlex
import shutil
import sqlite3
import stat
import subprocess
import tempfile
import time
import unittest
from unittest import mock
from scripts.tests import fake_ssh, kit_time, store_isolation

REPO = pathlib.Path(__file__).resolve().parents[2]
HYGIENE_PY = REPO / "scripts" / "pipeline" / "hygiene.py"

_spec = importlib.util.spec_from_file_location("pipeline_hygiene", HYGIENE_PY)
hygiene = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(hygiene)

GIB = hygiene.GIB

# gate-selftest loads every module into one process: no fixture event of this
# module may reach the metrics target of the caller's environment (W0b).
_METRICS_URL = mock.patch.dict(os.environ, {"MECLAW_PIPELINE_METRICS_URL": ""})


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()
    _METRICS_URL.start()


def tearDownModule():  # noqa: N802 -- unittest's name
    _METRICS_URL.stop()
    store_isolation.restore()

FAKE_SYSTEMCTL = """#!/bin/sh
echo "systemctl $*" >> "$HYG_FAKE/calls.log"
case "$2" in
  list-units) cat "$HYG_FAKE/units" 2>/dev/null
    [ -f "$HYG_FAKE/list_rc" ] && exit "$(cat "$HYG_FAKE/list_rc")" ;;
  stop)
    shift 2
    for u in "$@"; do
      if [ -f "$HYG_FAKE/units" ]; then
        grep -v "^$u " "$HYG_FAKE/units" > "$HYG_FAKE/units.tmp"
        mv "$HYG_FAKE/units.tmp" "$HYG_FAKE/units"
      fi
    done ;;
  show)
    [ -f "$HYG_FAKE/show_rc" ] && exit "$(cat "$HYG_FAKE/show_rc")"
    for last; do :; done
    cat "$HYG_FAKE/cg_$last" 2>/dev/null ;;
esac
exit 0
"""

FAKE_LOGGER = """#!/bin/sh
echo "%(name)s $*" >> "$HYG_FAKE/calls.log"
%(body)s
exit 0
"""


class FakeDisk:
    """Free space = capacity minus the sidecar sizes of what still exists."""

    def __init__(self, cap_g, slash_g=100.0):
        self.cap_g = cap_g
        self.slash_g = slash_g
        self.alloc = []

    def add(self, path, g):
        os.makedirs(path, exist_ok=True)
        with open(os.path.join(path, ".hygiene-bytes"), "w") as fh:
            fh.write(str(int(g * GIB)))
        self.alloc.append((path, g))

    def __call__(self, path):
        if path == "/":
            return self.slash_g
        used = sum(g for p, g in self.alloc if os.path.exists(p))
        return self.cap_g - used


class HygieneTestBase(unittest.TestCase):

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="hyg-test-")
        self.root = os.path.join(self.tmp, "lane")
        os.makedirs(self.root)
        self.fake = os.path.join(self.tmp, "fake")
        self.bin = os.path.join(self.tmp, "bin")
        os.makedirs(self.fake)
        os.makedirs(self.bin)
        self._write_bin("systemctl", FAKE_SYSTEMCTL)
        self._write_bin("ss", FAKE_LOGGER % {
            "name": "ss", "body": 'cat "$HYG_FAKE/ss" 2>/dev/null'})
        self._write_bin("systemd-run", FAKE_LOGGER % {
            "name": "systemd-run", "body": ""})
        fake_ssh.install_inert(self.bin, "a logger: prints a canned answer, never its arguments",
                               log=os.path.join(self.fake, "calls.log"), reply=os.path.join(self.fake, "ssh_out"),
                               rc_file=os.path.join(self.fake, "ssh_rc"), rc=0)
        self._write_bin("loginctl", FAKE_LOGGER % {
            "name": "loginctl", "body": "echo yes"})
        self._write_bin("holder", FAKE_LOGGER % {
            "name": "holder", "body": 'cat "$HYG_FAKE/holder" 2>/dev/null'})
        self._old_env = {k: os.environ.get(k) for k in
                         ("PATH", "HYG_FAKE", "MECLAW_GATE_LANE_MIN_FREE_G",
                          "MECLAW_IDLE_OFF_H")}
        os.environ["PATH"] = self.bin + os.pathsep + os.environ["PATH"]
        os.environ["HYG_FAKE"] = self.fake
        os.environ.pop("MECLAW_GATE_LANE_MIN_FREE_G", None)
        os.environ.pop("MECLAW_IDLE_OFF_H", None)
        self.proc = os.path.join(self.tmp, "proc")
        self.cg = os.path.join(self.tmp, "cgroup")
        os.makedirs(self.proc)
        os.makedirs(self.cg)
        with open(os.path.join(self.proc, "meminfo"), "w") as fh:
            fh.write("MemTotal:       16777216 kB\n")  # 16 G
        self.disk = FakeDisk(500.0)
        self.out = io.StringIO()
        self.err = io.StringIO()
        self.lock = os.path.join(self.tmp, "cargo.lock")
        self.ctx = self.make_ctx()

    def tearDown(self):
        for k, v in self._old_env.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v
        shutil.rmtree(self.tmp, ignore_errors=True)

    def make_ctx(self, **kw):
        args = dict(root=self.root, runner="lane-a", free_g=self.disk,
                    loadavg=lambda: 0.5,
                    out=self.out, err=self.err, proc_root=self.proc,
                    cgroup_root=self.cg, lock_path=self.lock, nproc=24,
                    state_file=os.path.join(self.tmp, "state.json"))
        args.update(kw)
        return hygiene.Ctx(**args)

    def _write_bin(self, name, body):
        p = os.path.join(self.bin, name)
        with open(p, "w") as fh:
            fh.write(body)
        os.chmod(p, os.stat(p).st_mode | stat.S_IEXEC)

    # -- helpers -----------------------------------------------------------
    def events(self, name=None):
        evs = [json.loads(line) for line in self.out.getvalue().splitlines()
               if line.startswith("{")]
        return [e for e in evs if name is None or e["event"] == name]

    def calls(self):
        p = os.path.join(self.fake, "calls.log")
        if not os.path.exists(p):
            return []
        with open(p) as fh:
            # an ssh call names the script the host runs (the narrow point
            # wraps it in `exec env -i ... bash -c <script>`, OR-T10-2)
            return fake_ssh.unwrap_remote(fh.read()).splitlines()

    def set_units(self, lines):
        with open(os.path.join(self.fake, "units"), "w") as fh:
            fh.write("".join(line + "\n" for line in lines))

    def mkdir_aged(self, rel, age_s):
        p = os.path.join(self.root, rel)
        os.makedirs(p, exist_ok=True)
        t = time.time() - age_s
        os.utime(p, (t, t))
        return p

    def make_target(self, rel, g, age_s, tree=None):
        p = os.path.join(self.root, rel)
        self.disk.add(p, g)
        if tree:
            with open(os.path.join(p, ".gate-tree"), "w") as fh:
                fh.write(tree + "\n")
        t = time.time() - age_s
        gt = os.path.join(p, ".gate-tree")
        if os.path.exists(gt):
            os.utime(gt, (t, t))
        os.utime(p, (t, t))
        return p

    def hold_lock(self):
        fd = os.open(self.lock, os.O_RDWR | os.O_CREAT, 0o644)
        fcntl.flock(fd, fcntl.LOCK_EX)
        self.addCleanup(os.close, fd)
        return fd


class TestCheck(HygieneTestBase):

    def test_hygiene_check_aborts_below_reserve(self):
        self.disk.cap_g = 70.0
        rc = hygiene.main(["check", "--root", self.root, "--expected-g", "35"],
                          ctx=self.ctx)
        self.assertEqual(rc, 2)
        ev = self.events("hygiene_checked")
        self.assertEqual(len(ev), 1)
        data = ev[0]["data"]
        self.assertEqual(ev[0]["kind"], "finding")
        self.assertEqual(data["verdict"], "aborted")
        self.assertEqual(data["reserved_g"], 35.0)
        for key in ("run", "runner", "free_g", "reserved_g", "leftovers",
                    "zombies", "ports_busy", "verdict"):
            self.assertIn(key, data)
        self.assertTrue(any(r.startswith("disk:") for r in data["reasons"]))
        # Enough space: ok, exit 0, kind note.
        self.out.truncate(0)
        self.out.seek(0)
        self.disk.cap_g = 200.0
        rc = hygiene.main(["check", "--root", self.root, "--expected-g", "35"],
                          ctx=self.ctx)
        self.assertEqual(rc, 0)
        self.assertEqual(self.events("hygiene_checked")[0]["data"]["verdict"],
                         "ok")

    def test_a_negative_lane_floor_is_ignored_with_a_note(self):
        # GH #1154 re-check m-1/m-2: a negative MECLAW_GATE_LANE_MIN_FREE_G
        # made the reserve negative -- the disk guard silently off. It is no
        # value: the default holds, and the check says so.
        os.environ["MECLAW_GATE_LANE_MIN_FREE_G"] = "-5"
        self.assertEqual(hygiene.settings(None)["reserve_g"],
                         hygiene.DEFAULT_RESERVE_G)
        data = hygiene.check(self.ctx, None)
        self.assertIn("MECLAW_GATE_LANE_MIN_FREE_G=-5 ignored, floor 60G",
                      data["notes"])
        self.assertIn("NOTE MECLAW_GATE_LANE_MIN_FREE_G=-5 ignored", self.err.getvalue())

    def test_a_zero_lane_floor_says_the_disk_guard_is_off(self):
        os.environ["MECLAW_GATE_LANE_MIN_FREE_G"] = "0"
        self.assertEqual(hygiene.settings(None)["reserve_g"], 0.0)
        data = hygiene.check(self.ctx, None)
        self.assertIn("MECLAW_GATE_LANE_MIN_FREE_G=0: disk guard off", data["notes"])

    def test_a_valid_lane_floor_leaves_no_note(self):
        os.environ["MECLAW_GATE_LANE_MIN_FREE_G"] = "20"
        self.assertEqual(hygiene.settings(None)["reserve_g"], 20.0)
        data = hygiene.check(self.ctx, None)
        self.assertFalse([n for n in data["notes"] if "MIN_FREE_G" in n])

    def test_hygiene_check_collects_stale_ws_only_older_than_1h(self):
        old = self.mkdir_aged("ws/run-000000000old", 7200)
        young = self.mkdir_aged("ws/run-0000000young", 60)
        live = self.mkdir_aged("ws/run-00000000live", 7200)
        data = hygiene.check(self.ctx, {"run": "run-00000000live",
                                        "live_runs": ["run-00000000live"]})
        self.assertFalse(os.path.exists(old))
        self.assertTrue(os.path.exists(young))
        self.assertTrue(os.path.exists(live))
        self.assertEqual(data["verdict"], "collected")
        self.assertEqual(data["leftovers"], 1)
        self.assertIn("young-ws run-0000000young", data["notes"])
        ch = self.events("workspace_changed")
        self.assertEqual(len(ch), 1)
        self.assertEqual(ch[0]["data"]["to"], "collected")
        self.assertEqual(ch[0]["data"]["kind"], "ws")

    def test_hygiene_check_stops_zombie_scopes(self):
        self.set_units([
            "pipe-run-aaaaaaaaaaaa.scope loaded active running live",
            "pipe-run-bbbbbbbbbbbb.scope loaded active running zombie"])
        data = hygiene.check(self.ctx, {"run": "run-aaaaaaaaaaaa",
                                        "live_runs": ["run-aaaaaaaaaaaa"]})
        self.assertEqual(data["zombies"], 1)
        stops = [c for c in self.calls() if " stop " in c]
        self.assertEqual(stops,
                         ["systemctl --user stop pipe-run-bbbbbbbbbbbb.scope"])

    def test_check_without_dispatch_never_stops_scopes(self):
        self.set_units(["pipe-run-cccccccccccc.scope loaded active running x"])
        self.mkdir_aged("ws/run-cccccccccccc", 7200)
        data = hygiene.check(self.ctx)
        self.assertEqual(data["zombies"], 1)
        self.assertIn("zombie-unverified pipe-run-cccccccccccc.scope",
                      data["notes"])
        self.assertFalse([c for c in self.calls() if " stop " in c])
        # Its ws belongs to a scope that is still active: not a leftover.
        self.assertTrue(os.path.isdir(os.path.join(self.root, "ws",
                                                   "run-cccccccccccc")))

    def test_hygiene_never_kills_stray_colony_only_notes(self):
        with open(os.path.join(self.fake, "ss"), "w") as fh:
            fh.write('LISTEN 0 4096 127.0.0.1:7931 0.0.0.0:* '
                     'users:(("meclaw",pid=4242,fd=9))\n'
                     'LISTEN 0 4096 0.0.0.0:31005 0.0.0.0:* '
                     'users:(("meclaw",pid=4343,fd=9))\n'
                     'LISTEN 0 128 0.0.0.0:22 0.0.0.0:* '
                     'users:(("sshd",pid=1,fd=3))\n')
        for pid, cg in ((4242, "0::/user.slice/app.slice/lab.service\n"),
                        (4343, "0::/user.slice/app.slice/"
                               "pipe-run-aaaaaaaaaaaa.scope\n")):
            os.makedirs(os.path.join(self.proc, str(pid)))
            with open(os.path.join(self.proc, str(pid), "cgroup"), "w") as fh:
                fh.write(cg)
        data = hygiene.check(self.ctx, {"live_runs": ["run-aaaaaaaaaaaa"]})
        self.assertEqual(data["ports_busy"], 2)
        self.assertIn("stray-colony 7931", data["notes"])
        self.assertNotIn("stray-colony 31005", data["notes"])
        for c in self.calls():
            self.assertNotIn("kill", c)
            self.assertNotIn(" stop ", c)

    def test_hygiene_measures_no_fingerprint_and_judges_no_drift(self):
        """Re-check tv-bn M7q M1 (OR-T11-22, one fact one source): the lane's
        fingerprint has one producer, `lane_fingerprint.sh`; the executor judges a
        drift against the register. The check measures none of it -- no kernel,
        no Python of whatever `python3` started it -- and writes no `lane-drift`
        however far the register is from this host; linger-off reads the register."""
        data = hygiene.check(self.ctx, {"fingerprint_expected": {
            "kernel": "0.0.0-elsewhere", "vcpu": 999, "python": "Python 0.0.0",
            "target_fs": "ext2/ext3", "linger": "yes"}})
        self.assertNotIn("drift", data)
        self.assertEqual([], [n for n in data["notes"] if n.startswith("lane-drift")])
        self.assertNotIn("linger-off", data["notes"])
        self.assertEqual([], [c for c in self.calls() if c.startswith("loginctl")])
        self.assertEqual(data["verdict"], "ok")
        for off in ("no", False):
            with self.subTest(linger=off):
                data = hygiene.check(self.ctx, {"fingerprint_expected": {"linger": off}})
                self.assertIn("linger-off", data["notes"])

    def test_hygiene_check_with_fakes_counts_its_work_and_books_its_clock(self):
        """The check's cost is its work, not the host's seconds (VG TV-K3): one
        listing of the run scopes, one stop per stale scope (never the live
        run's), one port query -- no process per directory, no fingerprint (M7q M1).
        `secs` is the injected clock's reading, exact (TV-K1)."""
        for i in range(30):
            self.mkdir_aged("ws/run-%012d" % i, 7200)
        self.set_units(["pipe-run-%012d.scope loaded active running x" % i
                        for i in range(20)])
        ticks = iter([5.0, 6.25])
        ctx = self.make_ctx(monotonic=lambda: next(ticks))
        rc = hygiene.main(["check", "--root", self.root, "--live-run",
                           "run-000000000001"], ctx=ctx)
        self.assertEqual(rc, 0)
        calls = self.calls()
        stops = [c for c in calls if c.startswith("systemctl --user stop ")]
        self.assertEqual(1, sum(c.startswith("systemctl --user list-units ") for c in calls), calls)
        self.assertEqual(19, len(stops), calls)
        self.assertNotIn("systemctl --user stop pipe-run-000000000001.scope", stops)
        self.assertEqual(1 + 19 + 1, len(calls), calls)
        fp_line = self.events("hygiene_checked")[0]["data"]
        self.assertEqual(1.25, fp_line["secs"])

    def test_the_stray_metric_counts_only_strays_past_24h(self):
        # W2 re-check: the alert `Lane stray target older than 24 h` fired at
        # the first sample of a fresh stray -- the metric counts what the
        # finding counts (> 24 h), so one sample of it is already the alarm.
        old = self.mkdir_aged("kb-target/debug", 2 * 86400)
        os.makedirs(os.path.join(old, "deps"))
        os.utime(old, (time.time() - 2 * 86400,) * 2)
        self.mkdir_aged("zm4-measure/debug", 600)
        self.assertEqual(1, hygiene.status(self.ctx)["stray_targets"])

    def test_stray_target_is_finding_after_24h(self):
        old = self.mkdir_aged("kb-target/debug", 2 * 86400)
        os.makedirs(os.path.join(old, "deps"))
        os.utime(old, (time.time() - 2 * 86400,) * 2)
        self.mkdir_aged("zm4-measure/debug", 600)
        self.mkdir_aged("target/debug", 3 * 86400)
        self.mkdir_aged("bench/x/debug", 3 * 86400)
        data = hygiene.check(self.ctx)
        strays = [n for n in data["notes"] if n.startswith("stray-target")]
        self.assertEqual(len(strays), 2)
        self.assertTrue(any("kb-target" in n for n in strays))
        self.assertTrue(any("zm4-measure" in n and n.endswith("?")
                            for n in strays))
        self.assertEqual(len(data["findings"]), 1)
        self.assertIn("kb-target", data["findings"][0])
        self.assertEqual(self.events("hygiene_checked")[0]["kind"], "finding")

    def test_pool_units_are_never_zombies(self):
        self.set_units([
            "pipe-pool-display.service loaded active running pool",
            "pipe-pool-display.timer loaded active waiting pool",
            "pipe-run-dddddddddddd.scope loaded active running zombie"])
        data = hygiene.check(self.ctx, {"live_runs": []})
        self.assertEqual(data["zombies"], 1)
        stops = [c for c in self.calls() if " stop " in c]
        self.assertEqual(stops,
                         ["systemctl --user stop pipe-run-dddddddddddd.scope"])

    def test_check_local_only_owner_disks(self):
        self.disk.slash_g = 30.0
        data = hygiene.check(self.ctx, local=True)
        self.assertEqual(data["verdict"], "aborted")
        self.assertFalse(self.calls())  # no lane parts: no systemctl, no ss


class TestEvictCollect(HygieneTestBase):

    def test_evict_lru_never_own_tree_or_main_pass_during_pass(self):
        own = self.make_target("target", 30, 9000)
        main = self.make_target("slot/1/target", 30, 8000, tree="main-pass")
        other = self.make_target("slot/2/target", 30, 100)
        self.disk.cap_g = 150.0  # free 60, need 60 + 35
        n = hygiene.evict(self.ctx, {"target": own,
                                     "live_runs": ["run-eeeeeeeeeeee"]},
                          need_g=35)
        self.assertEqual(n, 1)
        self.assertTrue(os.path.exists(own))
        self.assertTrue(os.path.exists(main))
        self.assertFalse(os.path.exists(other))
        ch = self.events("workspace_changed")
        self.assertEqual([(c["data"]["from"], c["data"]["to"],
                           c["data"]["bytes"]) for c in ch],
                         [("warm", "evicted", 30 * GIB)])
        # No live run: main-pass is fair game again.
        n = hygiene.evict(self.ctx, {"target": own, "live_runs": []},
                          need_g=35)
        self.assertEqual(n, 1)
        self.assertFalse(os.path.exists(main))

    def test_evict_skips_a_neighbour_slot_whose_build_path_lock_is_held(self):
        # GH #1140: two slots on one lane. Slot 1 is the least recently used
        # target, but a gate holds its build path lock (`slot/1/tree.lock`,
        # lane.hold) -- the check of slot 2 must not wipe it from under it.
        busy = self.make_target("slot/1/target", 30, 9000)
        idle = self.make_target("slot/3/target", 30, 5000)
        own = self.make_target("slot/2/target", 30, 100)
        fd = os.open(os.path.join(self.root, "slot", "1", "tree.lock"),
                     os.O_RDWR | os.O_CREAT, 0o644)
        fcntl.flock(fd, fcntl.LOCK_EX)
        self.addCleanup(os.close, fd)
        self.disk.cap_g = 150.0  # free 60, need 60 + 35
        notes = []
        n = hygiene.evict(self.ctx, {"target": own, "live_runs": []},
                          need_g=35, notes=notes)
        self.assertEqual(n, 1)
        self.assertTrue(os.path.exists(busy))
        self.assertTrue(os.path.exists(own))
        self.assertFalse(os.path.exists(idle))
        self.assertIn("busy-slot-protected slot/1/target", notes)

    @unittest.skipIf(os.geteuid() == 0, "root opens a chmod 000 file")
    def test_evict_keeps_a_neighbour_whose_build_path_lock_cannot_be_probed(self):
        # Review Zb I3: a tree.lock the probe cannot open (chmod 000) is no
        # free lock -- not checkable counts as held, the target stays.
        busy = self.make_target("slot/1/target", 30, 9000)
        own = self.make_target("slot/2/target", 30, 100)
        lock = os.path.join(self.root, "slot", "1", "tree.lock")
        with open(lock, "w"):
            pass
        os.chmod(lock, 0)
        self.addCleanup(lambda: os.path.exists(lock) and os.chmod(lock, 0o644))
        self.disk.cap_g = 150.0
        notes = []
        hygiene.evict(self.ctx, {"target": own, "live_runs": []},
                      need_g=35, notes=notes)
        self.assertTrue(os.path.exists(busy))
        self.assertIn("busy-slot-protected slot/1/target", notes)

    def test_evict_holds_the_build_path_lock_over_the_removal(self):
        # Review Zb R1 M4: no gap between probe and rmtree -- a gate asking
        # for the hold while the target is wiped gets busy.
        idle = self.make_target("slot/3/target", 30, 5000)
        own = self.make_target("slot/2/target", 30, 100)
        lock = os.path.join(self.root, "slot", "3", "tree.lock")
        seen = []
        real = hygiene._remove

        def remove(ctx, path):
            fd = os.open(lock, os.O_RDONLY | os.O_CREAT, 0o644)
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                seen.append("free")
            except OSError:
                seen.append("held")
            finally:
                os.close(fd)
            real(ctx, path)

        self.disk.cap_g = 150.0
        with mock.patch.object(hygiene, "_remove", remove):
            n = hygiene.evict(self.ctx, {"target": own, "live_runs": []}, need_g=35)
        self.assertEqual(n, 1)
        self.assertFalse(os.path.exists(idle))
        self.assertEqual(seen, ["held"])

    def test_lock_held_fails_closed_but_a_missing_lock_is_free(self):
        missing = os.path.join(self.root, "no", "tree.lock")
        self.assertFalse(hygiene.lock_held(missing))
        lock = os.path.join(self.root, "tree.lock")
        with open(lock, "w"):
            pass
        with mock.patch.object(hygiene.fcntl, "flock",
                               side_effect=OSError(errno.EIO, "io")):
            self.assertTrue(hygiene.lock_held(lock))
        with mock.patch.object(hygiene.os, "open",
                               side_effect=OSError(errno.EIO, "io")):
            self.assertTrue(hygiene.lock_held(lock))

    def test_lock_held_counts_a_directory_as_held(self):
        # GH #1154 item 3: flock on a directory fd succeeds, so the probe took
        # a directory in place of the lock file for a free lock. Not a lock
        # file -> not checkable -> held (fail closed, review Zb I3).
        lock = os.path.join(self.root, "tree.lock")
        os.mkdir(lock)
        self.assertTrue(hygiene.lock_held(lock))

    def test_build_path_lock_does_not_follow_a_dangling_symlink(self):
        # GH #1154 item 4: O_CREAT followed a dangling tree.lock symlink and
        # created its target. The lock is not taken (slot protected) and
        # nothing is created behind the link.
        own = self.make_target("slot/1/target", 30, 100)
        lock = os.path.join(self.root, "slot", "1", "tree.lock")
        behind = os.path.join(self.root, "elsewhere.lock")
        os.symlink(behind, lock)
        self.assertIsNone(hygiene._take_build_path_lock(own))
        self.assertFalse(os.path.lexists(behind))
        self.assertTrue(os.path.islink(lock))

    def test_evict_until_reserve_and_max_trees(self):
        paths = [self.make_target("slot/%d/target" % i, 10, 1000 * (10 - i))
                 for i in range(6)]
        # Plenty of space: only the tree quota applies -> 4 warm remain.
        n = hygiene.evict(self.ctx, need_g=0)
        self.assertEqual(n, 2)
        self.assertEqual([os.path.exists(p) for p in paths],
                         [False, False, True, True, True, True])
        # Short on space: LRU continues until reserve + need are free.
        self.disk.cap_g = 60 + 5 + 40 - 15  # free 50 with 4 trees (40 G)
        n = hygiene.evict(self.ctx, need_g=5)
        self.assertEqual(n, 2)
        self.assertGreaterEqual(self.disk(self.root), 65)
        self.assertEqual([os.path.exists(p) for p in paths[2:]],
                         [False, False, True, True])

    def test_the_coverage_target_is_a_warm_layout_element(self):
        # W6 review I1: the instrumented build of `lane_sync.sh coverage`
        # lives in root/coverage/target -- a layout element the hygiene
        # counts as warm (LRU-evictable, protected as own), never a stray.
        cov = self.make_target("coverage/target", 30, 9000)
        os.makedirs(os.path.join(self.root, "coverage", "debug"))
        self.assertIn(cov, hygiene.warm_targets(self.ctx))
        self.assertIn("coverage", hygiene.LAYOUT_TARGET_DIRS)
        self.assertEqual(hygiene.stray_targets(self.ctx), [])
        self.disk.cap_g = 80.0  # free 50, need 60 + 5
        n = hygiene.evict(self.ctx, need_g=5, own_target=cov)
        self.assertEqual(n, 0)
        self.assertTrue(os.path.exists(cov))
        n = hygiene.evict(self.ctx, need_g=5)
        self.assertEqual(n, 1)
        self.assertFalse(os.path.exists(cov))

    def test_check_keeps_the_lane_target_it_is_told_to_keep(self):
        # W2 review C1: since W1 the build path is root/tree, the run's
        # target is root/target -- a check short on room must not wipe it.
        tree = os.path.join(self.root, "tree")
        tgt = self.make_target("target", 40, 9000)
        other = self.make_target("slot/1/target", 5, 100)
        self.disk.cap_g = 130.0  # free 85 < 60 + 35
        rc = hygiene.main(["check", "--root", self.root, "--reserve-g", "60",
                           "--expected-g", "35", "--own-tree", tree,
                           "--own-target", tgt], ctx=self.ctx)
        self.assertEqual(rc, 2)
        self.assertTrue(os.path.exists(tgt))
        self.assertFalse(os.path.exists(other))

    def test_check_aborts_only_below_the_one_floor(self):
        # W2 review M1: the lane's floor (MECLAW_LANE_FLOOR_G) is the one
        # line that refuses; reserve + expected is only how far it clears.
        tree = os.path.join(self.root, "tree")
        tgt = self.make_target("target", 40, 9000)
        argv = ["check", "--root", self.root, "--reserve-g", "60",
                "--expected-g", "35", "--abort-below-g", "60",
                "--own-tree", tree, "--own-target", tgt]
        self.disk.cap_g = 110.0  # free 70: short of 95, above the floor
        self.assertEqual(hygiene.main(argv, ctx=self.ctx), 0)
        self.assertTrue(os.path.exists(tgt))
        self.disk.cap_g = 90.0  # free 50: below the floor
        self.assertEqual(hygiene.main(argv, ctx=self.ctx), 2)
        self.assertTrue(os.path.exists(tgt))

    def test_check_evicts_lru_when_short(self):
        self.make_target("slot/1/target", 50, 5000)
        self.disk.cap_g = 120.0  # free 70 < 60 + 35
        data = hygiene.check(self.ctx, {"live_runs": [], "expected_g": 35})
        self.assertEqual(data["verdict"], "collected")
        self.assertGreaterEqual(data["free_g"], 95)

    def test_collect_is_idempotent(self):
        self.mkdir_aged("ws/run-ffffffffffff", 7200)
        self.set_units(["pipe-run-ffffffffffff.scope loaded active running z"])
        hygiene.bench_new(self.ctx, "old", 0.001)
        meta = os.path.join(self.root, "bench", "old", ".hygiene.json")
        with open(meta, "w") as fh:
            json.dump({"created_at": "2026-01-01T00:00:00Z", "ttl_s": 60}, fh)
        for i in range(3):
            self.make_target("export/%016x" % i, 20, 1000 * (5 - i))
        self.out.truncate(0)
        self.out.seek(0)
        n1 = hygiene.collect(self.ctx, {"live_runs": []})
        first = self.events("workspace_changed")
        self.assertEqual(n1, len(first))
        kinds = sorted(e["data"]["kind"] for e in first)
        self.assertEqual(kinds, ["bench", "export", "ws"])
        self.out.truncate(0)
        self.out.seek(0)
        stops_before = len([c for c in self.calls() if " stop " in c])
        n2 = hygiene.collect(self.ctx, {"live_runs": []})
        self.assertEqual(n2, 0)
        self.assertEqual(self.events(), [])
        self.assertEqual(len([c for c in self.calls() if " stop " in c]),
                         stops_before)

    def test_evict_and_collect_skip_while_legacy_lock_held(self):
        t = self.make_target("slot/1/target", 30, 5000)
        ws = self.mkdir_aged("ws/run-111111111111", 7200)
        self.disk.cap_g = 80.0
        self.hold_lock()
        notes = []
        self.assertEqual(hygiene.evict(self.ctx, need_g=35, notes=notes), 0)
        self.assertEqual(hygiene.collect(self.ctx, {"live_runs": []},
                                         notes=notes), 0)
        self.assertIn("legacy-run", notes)
        self.assertTrue(os.path.exists(t))
        self.assertTrue(os.path.exists(ws))
        data = hygiene.check(self.ctx, {"live_runs": [], "expected_g": 35})
        self.assertEqual(data["verdict"], "aborted")
        self.assertIn("legacy-run", data["notes"])
        self.assertTrue(os.path.exists(t))
        self.assertTrue(os.path.exists(ws))
        self.assertEqual(self.events("workspace_changed"), [])

    def test_legacy_lock_probe_never_holds(self):
        with open(self.lock, "w"):
            pass
        self.assertFalse(hygiene.legacy_lock_held(self.ctx))
        fd = os.open(self.lock, os.O_RDWR)
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)  # still free
        finally:
            os.close(fd)


class TestKits(HygieneTestBase):
    """GH #1147 (KF5): `<root>/kit/` holds two keyings -- the core's pipeline kit
    by git sha8 (`<sha8>/` plus the marker `<sha8>.ok`, lane.push_kit) and the
    hygiene copy by content hash (`<hash8>/hygiene.py`, lane_sync.sh `hygiene
    --kit`) that a timer on the host calls hours later. collect removes what is
    old and referenced by no unit and no process; a failed reference check
    removes nothing."""

    OLD = 3 * 24 * 3600

    def make_kit(self, name, keying, age_s):
        base = os.path.join(self.root, "kit")
        path = os.path.join(base, name)
        if keying == "rev":
            os.makedirs(os.path.join(path, "scripts", "pipeline"))
            with open(os.path.join(path, "scripts", "pipeline.py"), "w") as fh:
                fh.write("x\n")
            with open(os.path.join(base, name + ".ok"), "w"):
                pass
        elif keying == "hygiene":
            os.makedirs(path)
            with open(os.path.join(path, "hygiene.py"), "w") as fh:
                fh.write("x\n")
        else:  # partial: a push that never wrote its marker
            os.makedirs(os.path.join(path, "scripts"))
        t = time.time() - age_s
        for d, dirs, files in os.walk(path):
            for f in dirs + files:
                os.utime(os.path.join(d, f), (t, t))
        os.utime(path, (t, t))
        marker = os.path.join(base, name + ".ok")
        if os.path.exists(marker):
            os.utime(marker, (t, t))
        return path

    def kit_names(self):
        return sorted(os.listdir(os.path.join(self.root, "kit")))

    def test_collect_removes_old_unreferenced_kits_of_both_keyings(self):
        self.make_kit("aaaaaaaa", "rev", self.OLD)
        self.make_kit("bbbbbbbb", "hygiene", self.OLD)
        self.make_kit("dddddddd", "rev", 600)
        n = hygiene.collect(self.ctx, {"live_runs": []})
        self.assertEqual(n, 2)
        self.assertEqual(self.kit_names(), ["dddddddd", "dddddddd.ok"])
        evs = [e["data"] for e in self.events("workspace_changed")]
        self.assertEqual(sorted((e["kind"], os.path.basename(e["path"]), e["to"]) for e in evs),
                         [("kit", "aaaaaaaa", "collected"), ("kit", "bbbbbbbb", "collected")])
        self.assertEqual(hygiene.collect(self.ctx, {"live_runs": []}), 0)

    def test_a_partial_kit_ages_by_its_ctime_too(self):
        """Review pd-1147 M-1 (b): `rsync -a` sets the kit directory's mtime
        to the core tree's, so between the copy and the marker a fresh push
        looks old by mtime. rsync cannot set ctime back: a partial kit ages by
        the later of both. (os.utime in make_kit leaves the ctime at now --
        the same trace rsync leaves.)"""
        self.make_kit("cccccccc", "partial", self.OLD)
        rows = {k["name"]: k for k in hygiene.kits(self.ctx)}
        self.assertEqual((rows["cccccccc"]["keying"], rows["cccccccc"]["verdict"],
                          rows["cccccccc"]["why"]), ("partial", "keep", "young"))
        self.assertEqual(hygiene.collect(self.ctx, {"live_runs": []}), 0)
        later = self.make_ctx(clock=lambda: time.time() + hygiene.KIT_KEEP_AGE_S + 60)
        self.assertEqual(hygiene.collect(later, {"live_runs": []}), 1)
        self.assertEqual(self.kit_names(), [])

    def test_a_kit_a_timer_calls_stays(self):
        kept = self.make_kit("bbbbbbbb", "hygiene", self.OLD)
        self.make_kit("eeeeeeee", "hygiene", self.OLD)
        self.set_units(["pipe-bench-x.timer loaded active waiting bench x",
                        "pipe-bench-x.service loaded inactive dead bench x"])
        with open(os.path.join(self.fake, "cg_pipe-bench-x.service"), "w") as fh:
            fh.write("{ path=/usr/bin/python3 ; argv[]=python3 %s/hygiene.py collect --bench x"
                     " --root %s ; ignore_errors=no }\n" % (kept, self.root))
        rows = {k["name"]: k for k in hygiene.kits(self.ctx)}
        self.assertEqual((rows["bbbbbbbb"]["verdict"], rows["bbbbbbbb"]["why"]),
                         ("keep", "unit pipe-bench-x.service"))
        self.assertEqual(rows["eeeeeeee"]["verdict"], "remove")
        hygiene.collect(self.ctx, {"live_runs": []})
        self.assertEqual(self.kit_names(), ["bbbbbbbb"])

    def test_a_kit_a_running_process_uses_stays(self):
        kept = self.make_kit("aaaaaaaa", "rev", self.OLD)
        os.makedirs(os.path.join(self.proc, "4242"))
        with open(os.path.join(self.proc, "4242", "cmdline"), "w") as fh:
            fh.write("python3\0%s/scripts/pipeline/unit_runner.py\0--list\0" % kept)
        rows = {k["name"]: k for k in hygiene.kits(self.ctx)}
        self.assertEqual((rows["aaaaaaaa"]["verdict"], rows["aaaaaaaa"]["why"]), ("keep", "pid 4242"))
        hygiene.collect(self.ctx, {"live_runs": []})
        self.assertEqual(self.kit_names(), ["aaaaaaaa", "aaaaaaaa.ok"])

    def test_a_failed_reference_check_removes_nothing(self):
        self.make_kit("aaaaaaaa", "rev", self.OLD)
        self.make_kit("bbbbbbbb", "hygiene", self.OLD)
        with open(os.path.join(self.fake, "list_rc"), "w") as fh:
            fh.write("1")
        notes = []
        self.assertEqual(hygiene.collect_kits(self.ctx, notes), 0)
        self.assertIn("kit-refcheck-failed", notes)
        self.assertEqual(self.kit_names(), ["aaaaaaaa", "aaaaaaaa.ok", "bbbbbbbb"])

    def test_a_failed_unit_show_removes_nothing(self):
        """Review pd-1147 M-2: `systemctl show` failing for one service leaves
        its ExecStart unread -- the reference check fails as a whole."""
        self.make_kit("aaaaaaaa", "rev", self.OLD)
        self.set_units(["pipe-bench-x.service loaded inactive dead bench x"])
        with open(os.path.join(self.fake, "show_rc"), "w") as fh:
            fh.write("1")
        notes = []
        self.assertEqual(hygiene.collect_kits(self.ctx, notes), 0)
        self.assertIn("kit-refcheck-failed", notes)
        self.assertEqual(self.kit_names(), ["aaaaaaaa", "aaaaaaaa.ok"])

    def test_a_kit_symlink_stays_and_only_its_marker_goes(self):
        """Review pd-1147 M-2: a kit name that is a symlink is never followed
        into a removal; its marker is an orphan and goes."""
        elsewhere = os.path.join(self.tmp, "elsewhere")
        os.makedirs(os.path.join(elsewhere, "scripts"))
        with open(os.path.join(elsewhere, "scripts", "pipeline.py"), "w") as fh:
            fh.write("x\n")
        base = os.path.join(self.root, "kit")
        os.makedirs(base)
        os.symlink(elsewhere, os.path.join(base, "aaaaaaaa"))
        marker = os.path.join(base, "aaaaaaaa.ok")
        with open(marker, "w"):
            pass
        t = time.time() - self.OLD
        os.utime(marker, (t, t))
        os.utime(elsewhere, (t, t))
        rows = [(k["name"], k["keying"], k["verdict"], k["why"]) for k in hygiene.kits(self.ctx)]
        self.assertEqual(rows, [("aaaaaaaa", "marker", "remove", "orphan-marker")])
        self.assertEqual(hygiene.collect_kits(self.ctx, []), 1)
        self.assertEqual(self.kit_names(), ["aaaaaaaa"])
        self.assertTrue(os.path.islink(os.path.join(base, "aaaaaaaa")))
        self.assertTrue(os.path.isfile(os.path.join(elsewhere, "scripts", "pipeline.py")))

    def test_the_marker_goes_first_and_an_orphan_marker_goes_at_once(self):
        """lane.push_kit trusts `<sha8>.ok` alone: a marker left without its
        directory would skip the next push of a kit that is not there."""
        self.make_kit("aaaaaaaa", "rev", self.OLD)
        with open(os.path.join(self.root, "kit", "ffffffff.ok"), "w"):
            pass  # young, and its directory is gone
        order = []
        real = hygiene._remove
        with mock.patch.object(hygiene, "_remove",
                               side_effect=lambda ctx, p: (order.append(os.path.basename(p)),
                                                           real(ctx, p))):
            hygiene.collect_kits(self.ctx, [])
        self.assertEqual(order[:2], ["aaaaaaaa.ok", "aaaaaaaa"])
        self.assertIn("ffffffff.ok", order)
        self.assertEqual(self.kit_names(), [])

    def test_dry_run_lists_and_removes_nothing(self):
        self.make_kit("aaaaaaaa", "rev", self.OLD)
        self.make_kit("bbbbbbbb", "hygiene", 600)
        ctx = self.make_ctx(dry_run=True)
        self.assertEqual([(k["name"], k["keying"], k["verdict"], k["why"]) for k in hygiene.kits(ctx)],
                         [("aaaaaaaa", "rev", "remove", "old"), ("bbbbbbbb", "hygiene", "keep", "young")])
        hygiene.collect(ctx, {"live_runs": []})
        self.assertEqual(self.kit_names(), ["aaaaaaaa", "aaaaaaaa.ok", "bbbbbbbb"])
        self.assertEqual(self.events("workspace_changed"), [])

    def test_kits_command_prints_one_line_per_kit(self):
        self.make_kit("aaaaaaaa", "rev", self.OLD)
        out = io.StringIO()
        ctx = self.make_ctx(out=out)
        self.assertEqual(hygiene.main(["kits", "--root", self.root, "--runner", "lane-a"], ctx=ctx), 0)
        rows = [json.loads(line) for line in out.getvalue().splitlines()]
        self.assertEqual([(r["name"], r["verdict"]) for r in rows], [("aaaaaaaa", "remove")])
        self.assertTrue(os.path.isdir(os.path.join(self.root, "kit", "aaaaaaaa")))

    def test_collected_kit_event_reaches_the_store(self):
        self.assertIn("kit", hygiene_store_kinds())


def hygiene_store_kinds():
    import sys
    sys.path.insert(0, str(REPO))
    from scripts.pipeline import store
    return store.WORKSPACE_KINDS


class TestWatch(HygieneTestBase):

    def _scope(self, pids, scope="pipe-run-222222222222.scope"):
        with open(os.path.join(self.fake, "cg_" + scope), "w") as fh:
            fh.write("/user.slice/app.slice/%s\n" % scope)
        d = os.path.join(self.cg, "user.slice", "app.slice", scope)
        os.makedirs(d, exist_ok=True)
        with open(os.path.join(d, "cgroup.procs"), "w") as fh:
            fh.write("".join("%d\n" % p for p in pids))
        return scope

    def _proc(self, pid, rss_kb=1000, ticks=0):
        d = os.path.join(self.proc, str(pid))
        os.makedirs(d, exist_ok=True)
        with open(os.path.join(d, "status"), "w") as fh:
            fh.write("Name:\tcargo\nVmRSS:\t%d kB\n" % rss_kb)
        fields = ["S"] + ["0"] * 10 + [str(ticks), "0"] + ["0"] * 30
        with open(os.path.join(d, "stat"), "w") as fh:
            fh.write("%d (cargo build) %s\n" % (pid, " ".join(fields)))

    def _ordered_ctx(self, order, clock=None):
        class Out(io.StringIO):
            def write(inner, s):
                if s.strip():
                    order.append("emit:" + json.loads(s)["event"])
                return io.StringIO.write(inner, s)
        self.out = Out()
        return self.make_ctx(out=self.out, clock=clock)

    def test_watch_fires_on_disk_and_stops_scope(self):
        order = []
        ctx = self._ordered_ctx(order)
        scope = self._scope([10])
        self._proc(10)
        self.disk.cap_g = 50.0
        w = hygiene.Watcher(ctx, "run-222222222222", scope, reserve_g=60,
                            on_abort=lambda k: order.append("abort:" + k),
                            stop_scope=lambda s: order.append("stop:" + s))
        self.assertEqual(w.tick(), "disk")
        self.assertEqual(order, ["emit:watchdog_fired", "abort:disk",
                                 "stop:" + scope])
        data = self.events("watchdog_fired")[0]["data"]
        self.assertEqual((data["kind"], data["action"], data["threshold"]),
                         ("disk", "aborted", 60.0))
        # Standalone `watch`: watchdog_fired, pass_ended ABORTED, then stop.
        self.out = io.StringIO()
        ctx = self.make_ctx()
        rc = hygiene.main(["watch", "--run", "run-222222222222", "--scope",
                           scope, "--root", self.root, "--interval", "0.01"],
                          ctx=ctx)
        self.assertEqual(rc, 2)
        names = [e["event"] for e in self.events()]
        self.assertEqual(names, ["watchdog_fired", "pass_ended"])
        pe = self.events("pass_ended")[0]["data"]
        self.assertEqual(pe["verdict"], "ABORTED")
        for key in ("run", "secs", "lane_seconds", "critical_path_s",
                    "rounds", "cache_hits", "stations"):
            self.assertIn(key, pe)
        self.assertIn("systemctl --user stop " + scope, self.calls())

    def test_watch_fires_on_deadline(self):
        now = [1_000_000.0]
        ctx = self.make_ctx(clock=lambda: now[0])
        scope = self._scope([11])
        self._proc(11)
        stopped = []
        w = hygiene.Watcher(ctx, "run-333333333333", scope,
                            deadline_at=hygiene.iso(now[0] + kit_time.virtual(60)),
                            stop_scope=stopped.append)
        self.assertIsNone(w.tick())
        now[0] += 61
        self.assertEqual(w.tick(), "deadline")
        self.assertEqual(stopped, [scope])
        self.assertEqual(self.events("watchdog_fired")[0]["data"]["kind"],
                         "deadline")

    def test_watch_fires_on_runaway_rss(self):
        scope = self._scope([12, 13])
        self._proc(12, rss_kb=1024)
        self._proc(13, rss_kb=9 * 1024 * 1024)  # 9 G of 16 G > 50 %
        stopped = []
        w = hygiene.Watcher(self.ctx, "run-444444444444", scope,
                            stop_scope=stopped.append)
        self.assertEqual(w.tick(), "process")
        data = self.events("watchdog_fired")[0]["data"]
        self.assertEqual(data["kind"], "process")
        self.assertEqual(data["threshold"], 8.0)
        self.assertEqual(stopped, [scope])

    def test_watch_fires_on_cpu_runaway_without_touch(self):
        now = [1_000_000.0]
        ctx = self.make_ctx(clock=lambda: now[0], nproc=4)
        scope = self._scope([14])
        ticks = [0]
        self._proc(14, ticks=0)
        stopped = []
        w = hygiene.Watcher(ctx, "run-555555555555", scope, clk_tck=100,
                            stop_scope=stopped.append)
        self.assertIsNone(w.tick())
        for step in range(40):  # 30 s ticks at 4 full cores
            now[0] += 30
            ticks[0] += 30 * 100 * 4
            self._proc(14, ticks=ticks[0])
            if step == 5:
                w.touch()  # the executor saw progress: clock restarts
            res = w.tick()
            if res:
                break
        self.assertEqual(res, "process")
        # Fired only > 600 s after the touch at step 5 (t = 180 s).
        self.assertGreater(now[0] - 1_000_000.0 - 180, 600)

    def test_watch_scope_gone_ends_quietly(self):
        w = hygiene.Watcher(self.ctx, "run-666666666666",
                            "pipe-run-666666666666.scope")
        self.assertEqual(w.tick(), "gone")
        self.assertEqual(self.events(), [])


class TestWorkspacesStore(HygieneTestBase):

    def store(self):
        return hygiene._store(os.path.join(self.tmp, "pipeline.db"))

    def test_workspace_transitions_are_events(self):
        hygiene.bench_new(self.ctx, "cmp", 2)
        hygiene.collect_bench(self.ctx, "cmp")
        self.make_target("slot/1/target", 10, 100)
        for i in range(5):
            self.make_target("slot/%d/target" % (i + 2), 10, 50)
        hygiene.evict(self.ctx, need_g=0)
        self.mkdir_aged("ws/run-777777777777", 7200)
        hygiene.collect(self.ctx, {"live_runs": []})
        lines = self.out.getvalue().splitlines()
        events = self.events("workspace_changed")
        self.assertEqual(len(events), len(lines))
        st = self.store()
        self.assertEqual(hygiene.apply_events(st, lines), len(lines))
        rows = st.journal_rows()
        self.assertEqual(len(rows), len(events))
        self.assertEqual([r["event"] for r in rows],
                         ["workspace_changed"] * len(rows))
        bench_id = "lane-a:bench:bench/cmp"
        ws = st.workspace_get(bench_id)
        self.assertEqual(ws["state"], "collected")
        self.assertEqual(ws["ttl_s"], 7200)
        self.assertIsNotNone(ws["created_at"])
        self.assertIsNotNone(ws["collected_at"])
        self.assertEqual(st.workspace_get("lane-a:target:slot/1/target")
                         ["state"], "evicted")
        self.assertEqual(st.workspace_get("lane-a:ws:ws/run-777777777777")
                         ["state"], "collected")
        with self.assertRaises(sqlite3.DatabaseError):
            st.conn.execute("UPDATE journal SET kind = 'x'")
        with self.assertRaises(sqlite3.DatabaseError):
            st.conn.execute("DELETE FROM journal")

    def test_one_event_is_one_transaction(self):
        # Review M3: the workspace row and the journal row commit together
        # (one store.tx), and the event is validated once.
        from unittest import mock
        hygiene.bench_new(self.ctx, "cmp", 2)
        obj = self.events("workspace_changed")[0]
        wid = obj["data"]["id"]
        st = self.store()
        self.addCleanup(st.conn.close)
        w0, ev = hygiene._core()
        real = ev.validate
        seen = []

        def counted(*a):
            seen.append(a[0])
            return real(*a)

        def broken(*a, **kw):
            raise RuntimeError("journal write failed")

        with mock.patch.object(ev, "validate", counted), \
                mock.patch.object(w0, "journal_add", broken):
            with self.assertRaises(RuntimeError):
                st.ingest(obj)
        self.assertIsNone(st.workspace_get(wid))
        self.assertEqual(st.journal_rows(), [])
        self.assertEqual(seen, ["workspace_changed"])
        with mock.patch.object(ev, "validate", counted):
            st.ingest(obj)
        self.assertEqual(st.workspace_get(wid)["state"], obj["data"]["to"])
        self.assertEqual(len(st.journal_rows()), 1)
        self.assertEqual(seen, ["workspace_changed"] * 2)

    def test_the_store_is_the_w0a_store(self):
        """No stub schema any more: `_store` opens W0a's store.py (schema
        version, append-only journal), and events go through events.py."""
        self.assertFalse(hasattr(hygiene, "_STUB_SCHEMA"))
        st = self.store()
        self.addCleanup(st.conn.close)
        w0, _ = hygiene._core()
        self.assertEqual(w0.check(st.conn)["schema"], w0.SCHEMA_VERSION)
        line = json.dumps({"event": "workspace_changed", "kind": "note",
                           "at": "2026-10-08T20:00:00Z", "run": None,
                           "data": {"runner": "lane-a"}})
        with self.assertRaises(ValueError):  # events.EventError: fields missing
            hygiene.apply_events(st, [line])
        self.assertEqual(st.journal_rows(), [])

    def test_the_store_needs_the_core_next_to_hygiene(self):
        """Piped onto a lane there is no store.py: `_store` refuses instead
        of creating a stub file at the shared path."""
        orig = hygiene._script_path
        hygiene._script_path = lambda: None
        try:
            with self.assertRaises(RuntimeError):
                hygiene._store(os.path.join(self.tmp, "nope.db"))
        finally:
            hygiene._script_path = orig
        self.assertFalse(os.path.exists(os.path.join(self.tmp, "nope.db")))

    def test_bench_new_sets_ttl_timer_and_row(self):
        rc = hygiene.main(["bench", "new", "cmp-a", "--ttl", "0.05",
                           "--root", self.root], ctx=self.ctx)
        self.assertEqual(rc, 0)
        lines = self.out.getvalue().splitlines()
        path = os.path.join(self.root, "bench", "cmp-a")
        self.assertEqual(lines[-1], path)
        self.assertTrue(os.path.isdir(path))
        with open(os.path.join(path, ".hygiene.json")) as fh:
            meta = json.load(fh)
        self.assertEqual(meta["ttl_s"], 180)
        runs = [c for c in self.calls() if c.startswith("systemd-run")]
        self.assertEqual(len(runs), 1)
        self.assertIn("--on-active=180s", runs[0])
        self.assertIn("--unit=pipe-bench-cmp-a", runs[0])
        self.assertIn("collect --bench cmp-a --root " + self.root, runs[0])
        st = self.store()
        hygiene.apply_events(st, lines)
        row = st.workspace_get("lane-a:bench:bench/cmp-a")
        self.assertEqual((row["kind"], row["state"], row["ttl_s"]),
                         ("bench", "in_use", 180))
        # `bench list` shows the rest TTL; check's TTL sweep collects it.
        self.out = io.StringIO()
        ctx = self.make_ctx(clock=lambda: time.time() + 200)
        hygiene.main(["bench", "list", "--root", self.root], ctx=ctx)
        row = json.loads(self.out.getvalue().splitlines()[0])
        self.assertEqual(row["name"], "cmp-a")
        self.assertLess(row["rest_s"], 0)
        data = hygiene.check(ctx, {"live_runs": []})
        self.assertEqual(data["verdict"], "collected")
        self.assertFalse(os.path.exists(path))

    def test_bench_new_without_its_timer_fails_loudly(self):
        """Re-check tv-bn M7p H3: systemd-run fails -> NOTE and rc 2, not a silent rc 0."""
        self._write_bin("systemd-run", FAKE_LOGGER.replace("exit 0", "echo nope >&2; exit 1") % {
            "name": "systemd-run", "body": ""})
        rc = hygiene.main(["bench", "new", "cmp-b", "--ttl", "0.05",
                           "--root", self.root], ctx=self.ctx)
        self.assertEqual(rc, 2)
        self.assertEqual(self.out.getvalue().splitlines()[-1], os.path.join(self.root, "bench", "cmp-b"))
        self.assertIn("NOTE systemd-run failed (nope)", self.err.getvalue())

    def test_bench_new_piped_has_no_script_path(self):
        orig = hygiene._script_path
        hygiene._script_path = lambda: None
        try:
            path = hygiene.bench_new(self.ctx, "piped", 1)
        finally:
            hygiene._script_path = orig
        runs = [c for c in self.calls() if c.startswith("systemd-run")]
        self.assertTrue(runs[0].endswith("rm -rf -- " + path))
        self.assertIn("NOTE no script path", self.err.getvalue())

    def test_status_line_has_pipe_lane_fields(self):
        self.make_target("target", 10, 100)
        deps = os.path.join(self.root, "target", "debug", "deps")
        os.makedirs(deps)
        for h in ("0123456789abcdef", "fedcba9876543210"):
            with open(os.path.join(deps, "foo-%s" % h), "wb") as fh:
                fh.write(b"x" * 10)
        self.mkdir_aged("ws/run-888888888888", 10)
        self.mkdir_aged("kb-target/debug", 2 * 86400)  # past 24 h: counts (W2 re-check)
        rc = hygiene.main(["status", "--root", self.root], ctx=self.ctx)
        self.assertEqual(rc, 0)
        line = json.loads(self.out.getvalue())
        for key in ("runner", "state", "free_g", "trees", "ws", "bench",
                    "load1", "zombies", "reserve_hits", "target_g",
                    "generations", "sccache_g", "stray_targets"):
            self.assertIn(key, line)
        self.assertEqual((line["trees"], line["ws"], line["generations"],
                          line["stray_targets"], line["state"]),
                         (1, 1, 2, 1, "idle"))
        self.out = io.StringIO()
        ctx = self.make_ctx(clock=lambda: 1_700_000_000.0)
        hygiene.main(["status", "--root", self.root, "--influx"], ctx=ctx)
        influx = self.out.getvalue().strip()
        self.assertTrue(influx.startswith("pipe_lane,runner=lane-a,"
                                          "state=idle "))
        self.assertTrue(influx.endswith(" 1700000000000000000"))
        for f in ("free_g=", "trees=", "ws=", "bench=", "load1=",
                  "zombies=", "reserve_hits=", "stray_targets="):
            self.assertIn(f, influx)

    def test_idle_off_timer_rearms_on_pass_ended_and_skips_held_lease(self):
        lanes = os.path.join(self.tmp, "lanes")
        with open(lanes, "w") as fh:
            fh.write("lane-a user@lane-a.example bmc-a threads=8\n"
                     "box user@box.example class=egon\n"
                     "here local\n")
        ended = {"event": "pass_ended", "kind": "status",
                 "data": {"verdict": "GREEN"}}
        for _ in range(2):
            self.assertEqual(hygiene.idle_rearm_on(
                self.ctx, ended, "lane-a", 2, lanes_file=lanes), 0)
        self.assertIsNone(hygiene.idle_rearm_on(
            self.ctx, {"event": "station_ended"}, "lane-a", 2,
            lanes_file=lanes))
        calls = [c for c in self.calls()
                 if c.startswith(("systemctl", "systemd-run"))]
        self.assertEqual(len(calls), 4)
        for i in (0, 2):  # re-arm = stop the old timer first, then start
            self.assertEqual(calls[i], "systemctl --user stop "
                             "pipe-idle-lane-a.timer pipe-idle-lane-a.service")
            self.assertIn("--on-active=7200s", calls[i + 1])
            self.assertIn("--unit=pipe-idle-lane-a", calls[i + 1])
            self.assertIn("idle-off fire --lane lane-a", calls[i + 1])
        self.assertEqual(hygiene.idle_off_arm(self.ctx, "box", 2, lanes), 2)
        self.assertEqual(hygiene.idle_off_arm(self.ctx, "here", 2, lanes), 2)
        os.environ["MECLAW_IDLE_OFF_H"] = "0"
        self.assertIsNone(hygiene.idle_rearm_on(self.ctx, ended, "lane-a",
                                                lanes_file=lanes))

        st = self.store()
        st.conn.execute("INSERT INTO leases (resource, holder, run, taken_at,"
                        " ttl_s) VALUES ('lane:lane-a', 'strand-x', NULL, ?,"
                        " 3600)", (hygiene.iso(time.time()),))
        st.conn.commit()
        self.assertEqual(hygiene.idle_off_fire(self.ctx, "lane-a", lanes,
                                               store=st), 0)
        self.assertFalse([c for c in self.calls() if c.startswith("ssh")])
        self.assertIn("lease lane:lane-a held", self.err.getvalue())
        # Lease expired, but the token holder names the lane: still skip.
        st.conn.execute("DELETE FROM leases")
        st.conn.commit()
        with open(os.path.join(self.fake, "holder"), "w") as fh:
            fh.write("holder: strand-x on lane-a since 10:00\n")
        self.assertEqual(hygiene.idle_off_fire(
            self.ctx, "lane-a", lanes, holder_cmd=["holder"], store=st), 0)
        self.assertFalse([c for c in self.calls() if c.startswith("ssh")])
        # Nobody holds it and the lane's probe says idle: power off via
        # sudo, runners.state = off.
        os.unlink(os.path.join(self.fake, "holder"))
        with open(os.path.join(self.fake, "ssh_out"), "w") as fh:
            fh.write(hygiene.IDLE_PROBE_OK + "\n")
        self.assertEqual(hygiene.idle_off_fire(
            self.ctx, "lane-a", lanes, holder_cmd=["holder"], store=st), 0)
        self.assertEqual([c for c in self.calls() if c.startswith("ssh")
                          and c.endswith("poweroff")],
                         ["ssh user@lane-a.example sudo -n systemctl "
                          "poweroff"])
        self.assertEqual(st.runner_state("lane-a"), "off")
        self.assertEqual(self.events(), [])  # no event outside V0


class TestIdleOffFire(HygieneTestBase):
    """`idle-off fire` (R-DT-8) powers a lane off only when every probe says
    it is idle; anything it cannot confirm keeps the host on."""

    def setUp(self):
        super().setUp()
        self.lanes = os.path.join(self.tmp, "lanes")
        with open(self.lanes, "w") as fh:
            fh.write("lane-a user@lane-a.example bmc-a threads=8\n")
        self.st = hygiene._store(os.path.join(self.tmp, "pipeline.db"))
        self.addCleanup(self.st.conn.close)

    def poweroffs(self):
        return [c for c in self.calls()
                if c.startswith("ssh") and c.endswith("poweroff")]

    def test_a_failing_holder_command_counts_as_held(self):
        self._write_bin("holder", FAKE_LOGGER.replace("exit 0", "exit 3") % {
            "name": "holder", "body": ""})
        for holder in (["holder"], ["no-such-holder-binary"]):
            with self.subTest(holder=holder):
                self.assertEqual(hygiene.idle_off_fire(
                    self.ctx, "lane-a", self.lanes, holder_cmd=holder,
                    store=self.st), 0)
                self.assertEqual(self.poweroffs(), [])
                self.assertIn("holder command failed", self.err.getvalue())
        self.assertIsNone(self.st.runner_state("lane-a"))

    def write(self, name, text):
        with open(os.path.join(self.fake, name), "w") as fh:
            fh.write(text)

    def rearms(self):
        return [c for c in self.calls() if c.startswith("systemd-run")
                and "idle-off arm --lane lane-a --after 2" in c]

    def test_the_lane_probe_must_say_idle_or_the_host_stays_on(self):
        # Busy (lock, unit or cargo), a probe without its OK word, a dead
        # ssh (timeout, unreachable): skip, keep the host on, arm again.
        for out, rc in (("busy: gate lock held\n", "1"), ("", "0"),
                        ("", "255")):
            with self.subTest(out=out, rc=rc):
                self.write("ssh_out", out)
                self.write("ssh_rc", rc)
                n = len(self.rearms())
                self.assertEqual(hygiene.idle_off_fire(
                    self.ctx, "lane-a", self.lanes, after_h=2,
                    store=self.st), 0)
                self.assertEqual(self.poweroffs(), [])
                self.assertIn("idle-off skip: lane probe", self.err.getvalue())
                self.assertEqual(len(self.rearms()), n + 1)
        self.assertIsNone(self.st.runner_state("lane-a"))
        probe = [c for c in self.calls() if c.startswith("ssh")][0]
        self.assertIn("BatchMode=yes", probe)
        self.assertIn("ConnectTimeout=", probe)
        self.assertIn("user@lane-a.example", probe)
        for word in ("pgrep", "pkill", "killall"):
            self.assertNotIn(word, probe)
        # Idle: then and only then the power-off.
        self.write("ssh_out", hygiene.IDLE_PROBE_OK + "\n")
        self.write("ssh_rc", "0")
        self.assertEqual(hygiene.idle_off_fire(
            self.ctx, "lane-a", self.lanes, after_h=2, store=self.st), 0)
        self.assertEqual(len(self.poweroffs()), 1)
        self.assertEqual(self.st.runner_state("lane-a"), "off")

    def test_every_skip_arms_the_timer_again(self):
        self.st.conn.execute(
            "INSERT INTO leases (resource, holder, run, taken_at, ttl_s) "
            "VALUES ('lane:lane-a', 'strand-x', NULL, ?, 3600)",
            (hygiene.iso(time.time()),))
        self.st.conn.commit()
        self.assertEqual(hygiene.idle_off_fire(
            self.ctx, "lane-a", self.lanes, after_h=2, store=self.st), 0)
        [rearm] = self.rearms()
        # Not the timer's own unit: that one is still running this fire.
        self.assertIn("--unit=pipe-idle-rearm-lane-a", rearm)
        self.assertIn("--lanes-file " + self.lanes, rearm)
        self.assertEqual(self.poweroffs(), [])

    def test_the_timer_carries_the_callers_store(self):
        # systemd-run passes no environment: a dev store (BUILD-PREAMBLE 1)
        # set by the caller goes with --setenv, the shared one stays implicit.
        old = os.environ.pop("MECLAW_PIPELINE_DB", None)
        self.addCleanup(lambda: os.environ.__setitem__(
            "MECLAW_PIPELINE_DB", old) if old is not None else
            os.environ.pop("MECLAW_PIPELINE_DB", None))
        self.assertEqual(hygiene.idle_off_arm(self.ctx, "lane-a", 2,
                                              self.lanes), 0)
        self.assertNotIn("--setenv", " ".join(self.calls()))
        dev = os.path.join(self.tmp, "pipeline-dev.db")
        os.environ["MECLAW_PIPELINE_DB"] = dev
        self.assertEqual(hygiene.idle_off_arm(self.ctx, "lane-a", 2,
                                              self.lanes), 0)
        hygiene.idle_rearm_later(self.ctx, "lane-a", 2, self.lanes)
        runs = [c for c in self.calls() if c.startswith("systemd-run")]
        self.assertEqual(len(runs), 3)
        for run in runs[1:]:
            self.assertIn("--setenv=MECLAW_PIPELINE_DB=" + dev, run)

    def test_the_fire_command_line_parses_with_dashed_commands(self):
        # A power or holder command with its own options (`ssh -o ...`)
        # must reach the fire intact; argparse took them for its own and the
        # timer died at start, without a word (probe 20:16).
        import argparse
        power = ["ssh", "-o", "BatchMode=yes", "u@h", "sudo", "-n", "true"]
        holder = ["env", "-i", "who", "--now"]
        parser = argparse.ArgumentParser()
        hygiene._build(parser.add_subparsers(dest="cmd"))
        args = parser.parse_args(
            ["idle-off", "fire", "--lane", "lane-a", "--after", "2"]
            + hygiene._idle_opts(self.lanes, holder, power))
        self.assertEqual(args.lanes_file, self.lanes)
        for got, want in ((args.power_cmd, power), (args.holder_cmd, holder)):
            argv = shlex.split(got[0]) if len(got) == 1 else got
            self.assertEqual(argv, want)

    def run_probe(self, units="", ps="", lock_held=False):
        """The probe script itself, run here with a fake systemctl/ps."""
        self.write("units", units)
        self._write_bin("ps", '#!/bin/sh\necho "$*" >"$HYG_FAKE/ps.argv"\n'
                        'cat "$HYG_FAKE/ps"\n')
        self.write("ps", ps)
        fd = None
        if lock_held:
            fd = os.open(self.lock, os.O_RDWR | os.O_CREAT, 0o644)
            fcntl.flock(fd, fcntl.LOCK_EX)
        try:
            cp = subprocess.run(["sh", "-c", hygiene.IDLE_PROBE_SH,
                                 "idle-probe", self.lock],
                                capture_output=True, text=True, timeout=kit_time.HANG)
        finally:
            if fd is not None:
                os.close(fd)
        return cp.returncode, cp.stdout

    def test_the_probe_script_sees_lock_units_and_cargo(self):
        # Realistic `ps -o args=` lines (review I1): gate.sh and test-tier.sh
        # start with `#!/usr/bin/env bash`, so their comm is always "bash";
        # only the command line names them. The probe's own command line
        # (the script text on sh -c) must not count as a build.
        own = "sh -c %s idle-probe %s" % (hygiene.IDLE_PROBE_SH, self.lock)
        rc, out = self.run_probe(
            ps="bash\nsshd: u@notty\nvim notes-gate.sh.orig\n"
               "bash -c %s\n%s\n" % (own, own))
        self.assertEqual((rc, out.strip()), (0, hygiene.IDLE_PROBE_OK))
        self.assertIn("args=", (pathlib.Path(self.fake) / "ps.argv").read_text())
        for kw in ({"lock_held": True},
                   {"units": "pipe-run-0000000000a1.scope loaded active\n"},
                   {"units": "pipe-pool-0.service loaded active\n"},
                   {"ps": "bash\n/home/u/.cargo/bin/cargo build --tests\n"},
                   {"ps": "/home/u/.rustup/toolchains/s/bin/rustc --crate-name x\n"},
                   {"ps": "/home/u/.cargo/bin/cargo-nextest nextest run\n"},
                   {"ps": "bash\nbash scripts/gate.sh strand --lane\n"},
                   {"ps": "bash /srv/lane/tree/scripts/test-tier.sh filter x\n"},
                   {"ps": "bash gate.sh\n"}):
            with self.subTest(kw=kw):
                rc, out = self.run_probe(**kw)
                self.assertNotEqual(rc, 0)
                self.assertNotIn(hygiene.IDLE_PROBE_OK, out)
                self.assertIn("busy", out)


    def test_the_probe_sees_a_held_build_path(self):
        # Delta C M-neu-1: a push+run holds `<tree>.lock` (lane.hold, pre,
        # a bare push) before any pipe-run scope or cargo process exists.
        root = os.path.join(self.fake, "lane-root")
        os.makedirs(os.path.join(root, "slot", "1"))
        self.write("units", "")
        self._write_bin("ps", '#!/bin/sh\ncat "$HYG_FAKE/ps"\n')
        self.write("ps", "")
        for rel in ("tree.lock", "slot/1/tree.lock"):
            lock = os.path.join(root, rel)
            fd = os.open(lock, os.O_RDWR | os.O_CREAT, 0o644)
            try:
                fcntl.flock(fd, fcntl.LOCK_EX)
                cp = subprocess.run(["sh", "-c", hygiene.IDLE_PROBE_SH, "idle-probe", self.lock, root],
                                    capture_output=True, text=True, timeout=kit_time.HANG)
            finally:
                os.close(fd)
            with self.subTest(lock=rel):
                self.assertNotEqual(cp.returncode, 0)
                self.assertIn("busy: build path lock", cp.stdout)
        cp = subprocess.run(["sh", "-c", hygiene.IDLE_PROBE_SH, "idle-probe", self.lock, root],
                            capture_output=True, text=True, timeout=kit_time.HANG)
        self.assertEqual((cp.returncode, cp.stdout.strip()), (0, hygiene.IDLE_PROBE_OK))


class TestGenerations(HygieneTestBase):

    def _unit(self, target, stem, h, age_s, size=1000, kind="lib"):
        debug = os.path.join(target, "debug")
        t = time.time() - age_s
        paths = []
        deps = os.path.join(debug, "deps")
        os.makedirs(deps, exist_ok=True)
        if kind == "lib":
            names = ["lib%s-%s.rlib" % (stem, h), "%s-%s.d" % (stem, h)]
        else:
            names = ["%s-%s" % (stem, h), "%s-%s.d" % (stem, h)]
        for n in names:
            p = os.path.join(deps, n)
            with open(p, "wb") as fh:
                fh.write(b"x" * size)
            paths.append(p)
        fp = os.path.join(debug, ".fingerprint", "%s-%s" % (stem, h))
        os.makedirs(fp, exist_ok=True)
        with open(os.path.join(fp, "lib-%s" % stem), "w") as fh:
            fh.write(h)
        paths.append(fp)
        for p in paths:
            os.utime(p, (t, t))
        return paths

    def test_generation_evict_keeps_current_hashes_and_quota(self):
        target = os.path.join(self.root, "target")
        # Three generations of one test binary plus the current lib of the
        # same crate: the heuristic keeps the newest hash per stem AND shape,
        # so the lib (other shape, other hash) survives next to the binary.
        old = self._unit(target, "foo", "1111111111111111", 3000, kind="test")
        mid = self._unit(target, "foo", "2222222222222222", 2000, kind="test")
        new = self._unit(target, "foo", "3333333333333333", 1000, kind="test")
        lib = self._unit(target, "foo", "4444444444444444", 1000, kind="lib")
        res = hygiene.generations(self.ctx, target=target)
        self.assertEqual(res[0]["groups"], 2)
        for p in old + mid:
            self.assertFalse(os.path.exists(p), p)
        for p in new + lib:
            self.assertTrue(os.path.exists(p), p)
        ch = self.events("workspace_changed")[0]["data"]
        self.assertEqual((ch["kind"], ch["from"], ch["to"]),
                         ("target", "warm", "warm"))
        self.assertIn("evicted_g", ch)
        self.assertEqual(ch["groups"], 2)

        # Exact keep set from a cargo JSON stream, quota respected.
        target2 = os.path.join(self.root, "slot", "1", "target")
        a = self._unit(target2, "bar", "aaaaaaaaaaaaaaaa", 3000)
        b = self._unit(target2, "bar", "bbbbbbbbbbbbbbbb", 2000)
        c = self._unit(target2, "bar", "cccccccccccccccc", 1000)
        stream = os.path.join(self.tmp, "cargo.json")
        with open(stream, "w") as fh:
            fh.write(json.dumps({
                "reason": "compiler-artifact", "fresh": True,
                "filenames": [os.path.join(
                    target2, "debug/deps/libbar-bbbbbbbbbbbbbbbb.rlib")]})
                + "\nnot json\n")
        # Quota fits two generations: only the oldest non-kept goes.
        size_one = sum(os.path.getsize(p) for p in a if os.path.isfile(p)) + \
            sum(os.path.getsize(os.path.join(a[-1], f))
                for f in os.listdir(a[-1]))
        res = hygiene.generations(self.ctx, target=target2, keep_from=stream,
                                  generation_g=2 * size_one / GIB,
                                  quota_factor=1.0)
        self.assertEqual(res[0]["groups"], 2)  # lib + its .d/fingerprint
        self.assertFalse(any(os.path.exists(p) for p in a))
        self.assertTrue(all(os.path.exists(p) for p in b + c))
        # Default quota (kept set x 1.2): the newest non-kept goes as well.
        hygiene.generations(self.ctx, target=target2, keep_from=stream)
        self.assertTrue(all(os.path.exists(p) for p in b))
        self.assertFalse(any(os.path.exists(p) for p in c))
        # Idempotent.
        self.out = io.StringIO()
        self.ctx.out = self.out
        res = hygiene.generations(self.ctx, target=target2, keep_from=stream)
        self.assertEqual(res[0]["groups"], 0)
        self.assertEqual(self.events(), [])

    def test_generation_evict_never_touches_wt_bare_sccache(self):
        keep = []
        for rel in ("wt/strand-x/target", "meclaw.git/target", "sccache"):
            base = os.path.join(self.root, rel)
            keep += self._unit(base, "foo", "1111111111111111", 9000)
            keep += self._unit(base, "foo", "2222222222222222", 100)
        target = os.path.join(self.root, "target")
        self._unit(target, "foo", "1111111111111111", 9000)
        self._unit(target, "foo", "2222222222222222", 100)
        res = hygiene.generations(self.ctx)  # default: lane targets
        self.assertEqual([r["target"] for r in res], [target])
        for rel in ("wt/strand-x/target", "meclaw.git/target", "sccache"):
            res = hygiene.generations(
                self.ctx, target=os.path.join(self.root, rel))
            self.assertEqual(res, [])
        for p in keep:
            self.assertTrue(os.path.exists(p), p)
        # Outside the four debug dirs nothing is touched either.
        stray = os.path.join(target, "debug", "foo-1111111111111111")
        with open(stray, "w") as fh:
            fh.write("uplifted")
        os.utime(stray, (0, 0))
        hygiene.generations(self.ctx, target=target, generation_g=0)
        self.assertTrue(os.path.exists(stray))


class TestCli(HygieneTestBase):

    def test_register_adds_hygiene_command(self):
        import argparse
        parser = argparse.ArgumentParser()
        sub = parser.add_subparsers(dest="cmd")
        hygiene.register(sub)
        args = parser.parse_args(["hygiene", "check", "--root", self.root,
                                  "--reserve-g", "40", "--expected-g", "35",
                                  "--own-tree", "/x", "--live-run", "run-a",
                                  "run-b", "--dry-run", "--runner", "lane-a"])
        self.assertEqual((args.hygiene_cmd, args.reserve_g, args.live_run),
                         ("check", 40.0, ["run-a", "run-b"]))

    def test_piped_stdin_runs_without_file(self):
        import subprocess
        import sys
        with open(HYGIENE_PY) as fh:
            src = fh.read()
        cp = subprocess.run([sys.executable, "-", "status", "--root",
                             self.root, "--runner", "lane-a"], input=src,
                            capture_output=True, text=True, timeout=kit_time.HANG)
        self.assertEqual(cp.returncode, 0, cp.stderr)
        self.assertEqual(json.loads(cp.stdout)["runner"], "lane-a")


    def test_the_hygiene_python_is_the_kits_one_python(self):
        """Re-check tv-bn M7o M1: hygiene.py runs alone on a host and cannot
        import pipeline.lane there, so it names the kit's Python itself; this
        pin keeps the two the same."""
        from scripts.pipeline import lane
        self.assertEqual(lane.PYTHON, hygiene.PYTHON)

    def test_a_hygiene_copy_alone_arms_its_bench_timer(self):
        """Re-check tv-bn M7o M1: `lane_sync.sh hygiene --kit` puts hygiene.py
        alone on a host (`<hash8>/hygiene.py`); `bench new` built its timer's
        argv through pipeline.lane and broke there with ModuleNotFoundError,
        the bench stood without its TTL timer."""
        from scripts.pipeline import lane
        alone = os.path.join(self.tmp, "kit", "0123abcd", "hygiene.py")
        os.makedirs(os.path.dirname(alone))
        shutil.copy(HYGIENE_PY, alone)
        cp = subprocess.run(lane.python_argv(alone, "bench", "new", "solo", "--ttl", "1",
                                             "--root", self.root),
                            cwd=os.path.dirname(alone), capture_output=True, text=True,
                            timeout=kit_time.HANG)
        self.assertEqual(cp.returncode, 0, cp.stderr)
        runs = [c for c in self.calls() if c.startswith("systemd-run")]
        self.assertEqual(1, len(runs), self.calls())
        self.assertIn("--unit=pipe-bench-solo", runs[0])
        self.assertIn(" ".join(list(lane.PYTHON) + [alone, "collect", "--bench", "solo"]), runs[0])


class TestRootFromLanesFile(unittest.TestCase):
    """Z6: without `--root` and `MECLAW_LANE_ROOT` the hygiene takes the root
    of this runner's `local` line in the host file -- the root lane.py uses
    for the same lane -- and `/srv/target` only when the file names none."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.lanes = os.path.join(self._tmp.name, "lanes")
        self.root = os.path.join(self._tmp.name, "laneroot")

    def ctx(self, text, **env):
        with open(self.lanes, "w", encoding="utf-8") as fh:
            fh.write(text)
        base = {"MECLAW_LANES_FILE": self.lanes, "MECLAW_LANE_NAME": "lx"}
        base.update(env)
        with mock.patch.dict(os.environ, base):
            os.environ.pop("MECLAW_LANE_ROOT", None)
            return hygiene.Ctx(clock=lambda: 0)

    def test_the_local_line_of_this_runner_names_the_root(self):
        self.assertEqual(self.ctx("lx local %s class=egon\n" % self.root).root, self.root)

    def test_another_runner_or_an_ssh_line_keeps_the_default(self):
        self.assertEqual(self.ctx("other local %s\nlx build9 bmc9\n" % self.root).root,
                         hygiene.DEFAULT_ROOT)

    def test_the_environment_still_wins(self):
        with open(self.lanes, "w", encoding="utf-8") as fh:
            fh.write("lx local %s\n" % self.root)
        with mock.patch.dict(os.environ, {"MECLAW_LANES_FILE": self.lanes, "MECLAW_LANE_NAME": "lx",
                                          "MECLAW_LANE_ROOT": "/x/y"}):
            self.assertEqual(hygiene.Ctx(clock=lambda: 0).root, "/x/y")



# --- the one CPU source of the kit (GH #1199, re-check A-I1) ------------------------

# Where a CPU count may come from. `--kit` (export_tree_tests.ci_runner) emulates
# the public runner by the affinity (`taskset`) and a sysroot (MECLAW_FP_SYSROOT);
# `os.cpu_count()` counts the machine and /sys or /proc read the host, so any
# other site computed host width under `--kit` and runner width in the public CI.
# hygiene.py is the one module that counts CPUs without `cpus` (it runs alone on
# a lane), so the rule lives beside its tests. The allow list is narrow and
# named, `(file, function)`:
CPU_COUNT_HOME = {("scripts/pipeline/cpus.py", "threads")}
# the sysroot reader of the kit; hygiene.py runs alone on a lane (ssh stdin, no
# sibling to import) and counts its affinity inline
TOPOLOGY_READERS = {("scripts/pipeline/cpus.py", None)}
# (and `ci_runner`, which picks the CPUs of its `taskset` pin from the affinity)
AFFINITY_READERS = {("scripts/pipeline/cpus.py", None),
                    ("scripts/pipeline/hygiene.py", "_affinity_threads"),
                    ("plans/export-fixtures/export_tree_tests.py", "ci_runner")}
# writers of a FAKE topology, never a reading: the runner's sysroot of `--kit`
TOPOLOGY_WRITERS = {("plans/export-fixtures/export_tree_tests.py", "runner_sysroot")}
TOPOLOGY_WORDS = ("cpuinfo", "thread_siblings", "devices/system/cpu")
TOPOLOGY_PIECES = {"devices", "system", "cpu"}


def kit_python_files(root):
    """The kit's Python: scripts/**/*.py without fixtures, .github/**/*.py and
    the export station's runner (plans/export-fixtures/export_tree_tests.py)."""
    root = pathlib.Path(root)
    out = [p for p in sorted((root / "scripts").rglob("*.py")) if "fixtures" not in p.parts]
    out += sorted((root / ".github").rglob("*.py"))
    ett = root / "plans" / "export-fixtures" / "export_tree_tests.py"
    if ett.is_file():
        out.append(ett)
    return out


def _functions_of(tree):
    """`{id(node): innermost function name}` for every node of `tree`."""
    import ast
    owner = {}

    def visit(node, name):
        for child in ast.iter_child_nodes(node):
            inner = child.name if isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef)) else name
            owner[id(child)] = inner
            visit(child, inner)
    visit(tree, None)
    return owner


def _allowed(allow, rel, fn):
    return (rel, fn) in allow or (rel, None) in allow


def cpu_source_findings(root):
    """`path:line why` of every kit site that reads a CPU count past `cpus.py`:
    an `os.cpu_count()` call outside `cpus.threads`; in scripts/ outside its
    tests and in export_tree_tests.py a topology path (`cpuinfo`,
    `thread_siblings`, `devices/system/cpu` or its join pieces) outside the
    reader, and an affinity read outside the named ones. A `mock.patch(
    "os.cpu_count", ...)` string is no call, a docstring no path."""
    import ast
    root = pathlib.Path(root)
    found = []
    for path in kit_python_files(root):
        rel = path.relative_to(root).as_posix()
        try:
            tree = ast.parse(path.read_text(encoding="utf-8"), filename=rel)
        except (OSError, SyntaxError, UnicodeDecodeError) as exc:
            found.append("%s:0 unreadable (%s)" % (rel, exc))
            continue
        owner = _functions_of(tree)
        prose = {id(n.value) for n in ast.walk(tree) if isinstance(n, ast.Expr)}
        files_rule = rel.startswith("scripts/") and not rel.startswith("scripts/tests/") \
            or rel == "plans/export-fixtures/export_tree_tests.py"
        for node in ast.walk(tree):
            fn = owner.get(id(node))
            if isinstance(node, ast.Call):
                name = getattr(node.func, "attr", getattr(node.func, "id", None))
                if name == "cpu_count" and not _allowed(CPU_COUNT_HOME, rel, fn):
                    found.append("%s:%d cpu_count() outside cpus.threads" % (rel, node.lineno))
                if files_rule and name == "sched_getaffinity" \
                        and not _allowed(AFFINITY_READERS, rel, fn):
                    found.append("%s:%d affinity read outside cpus.threads" % (rel, node.lineno))
                pieces = {a.value for a in node.args
                          if isinstance(a, ast.Constant) and isinstance(a.value, str)}
                if files_rule and TOPOLOGY_PIECES <= pieces and not (
                        _allowed(TOPOLOGY_READERS, rel, fn) or _allowed(TOPOLOGY_WRITERS, rel, fn)):
                    found.append("%s:%d topology path joined outside cpus.py" % (rel, node.lineno))
            elif files_rule and isinstance(node, ast.Constant) and isinstance(node.value, str) \
                    and id(node) not in prose and any(w in node.value for w in TOPOLOGY_WORDS) \
                    and not (_allowed(TOPOLOGY_READERS, rel, fn)
                             or _allowed(TOPOLOGY_WRITERS, rel, fn)):
                found.append("%s:%d topology path %r outside cpus.py" % (rel, node.lineno,
                                                                       node.value[:40]))
    return found


class KitCpuSource(unittest.TestCase):
    """GH #1199 (re-check A-I1): `--kit` pins the runner's CPUs by affinity and a
    sysroot, so every CPU count of the kit comes from `pipeline.cpus` -- `threads()`
    (the affinity) and `physical_cores()` (the sysroot's topology). A second source
    computes host width under `--kit` and runner width in the public CI."""

    def test_the_kit_counts_cpus_in_one_place(self):
        self.assertEqual([], cpu_source_findings(REPO))

    def test_the_rule_sees_every_other_source(self):
        with tempfile.TemporaryDirectory() as tmp:
            pkg = pathlib.Path(tmp, "scripts", "pipeline")
            tests = pathlib.Path(tmp, "scripts", "tests")
            pkg.mkdir(parents=True)
            tests.mkdir()
            (pkg / "cpus.py").write_text(
                "import os\ndef threads():\n    return os.cpu_count()\n"
                "def peek():\n    return os.cpu_count()\n"
                "ROOT = os.path.join('/', 'sys', 'devices', 'system', 'cpu')\n")
            (pkg / "wide.py").write_text(
                "'''Reads /proc/cpuinfo.'''\n"
                "import os\nN = os.cpu_count()\nA = len(os.sched_getaffinity(0))\n"
                "P = '/proc/cpuinfo'\nQ = os.path.join('/', 'sys', 'devices', 'system', 'cpu')\n")
            (tests / "test_x.py").write_text(
                "import os\nfrom unittest import mock\nP = mock.patch('os.cpu_count')\n"
                "T = 'sys/devices/system/cpu/cpu0/topology/thread_siblings_list'\n"
                "N = os.cpu_count()\n")
            self.assertEqual(["scripts/pipeline/cpus.py:5 cpu_count() outside cpus.threads",
                              "scripts/pipeline/wide.py:3 cpu_count() outside cpus.threads",
                              "scripts/pipeline/wide.py:4 affinity read outside cpus.threads",
                              "scripts/pipeline/wide.py:5 topology path '/proc/cpuinfo' outside "
                              "cpus.py",
                              "scripts/pipeline/wide.py:6 topology path joined outside cpus.py",
                              "scripts/tests/test_x.py:5 cpu_count() outside cpus.threads"],
                             sorted(cpu_source_findings(tmp)))


if __name__ == "__main__":
    unittest.main()
