"""Tests for the colony pool `scripts/pipeline/pool.py` (wave dev-tempo W7).

A pool colony is a transient user unit that outlives one scenario case, so
round 2 or a second gate on the same binary finds it warm instead of booting
again. What is pinned here: the unit and idle-timer names and argv (the idle
timer is a one-shot that every `get` re-arms -- RuntimeMaxSec is only the hard
ceiling), the hit rule (active AND same full binary sha AND healthy), the
reboot on a mismatch, and the `workspace_changed` events whose rows the core
writes into `workspaces` (kind `pool`).

Every test runs against a fake systemd (records argv, answers from a table),
a fake health probe and a fake clock -- never the real `systemd-run`, never a
real wait.
"""

import hashlib
import json
import os
import pathlib
import sys
import tempfile
import unittest
from unittest import mock

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO))

from scripts.pipeline import events, pool, ports, store  # noqa: E402
from scripts.tests import side_effect_guard, store_isolation  # noqa: E402


# Store, night watchdog and outbound metric targets stay away from the
# machine's (scripts/tests/store_isolation.py).
def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()


def tearDownModule():  # noqa: N802 -- unittest's name
    store_isolation.restore()

SHA_A = "abcdef01" + "0" * 56
SHA_A2 = "abcdef01" + "1" * 56  # same 8-char prefix -> same unit name
SHA_B = "12345678" + "0" * 56
ARGV = ["/opt/bin/meclaw", "--root", "/tmp/r", "--port", "31300"]
PORT = ports.port_at(3, "api")


class FakeSystemd:
    """Answers `systemctl --user is-active|stop` and `systemd-run --user`."""

    def __init__(self, start_rc=0):
        self.calls = []
        self.active = set()
        self.start_rc = start_rc

    def run(self, argv):
        self.calls.append(list(argv))
        if argv[:3] == ["systemctl", "--user", "is-active"]:
            return (0, "active\n", "") if argv[3] in self.active else (3, "inactive\n", "")
        if argv[:3] == ["systemctl", "--user", "stop"]:
            unit = argv[3]
            if unit.endswith(".timer"):
                unit = unit[:-len(".timer")]
            if unit not in self.active:
                return 5, "", f"Unit {argv[3]} not loaded."
            self.active.discard(unit)
            return 0, "", ""
        if argv[:2] == ["systemd-run", "--user"]:
            unit = argv[argv.index("--unit") + 1]
            if any(a.startswith("--on-active=") for a in argv):
                self.active.add(unit)
                return 0, "", ""
            if self.start_rc == 0:
                self.active.add(unit)
            return self.start_rc, "", ""
        raise AssertionError(f"unexpected argv {argv}")

    def starts(self):
        return [c for c in self.calls if c[:2] == ["systemd-run", "--user"]
                and not any(a.startswith("--on-active=") for a in c)]


class FakeClock:
    def __init__(self):
        self.t = 1000.0
        self.waits = []

    def clock(self):
        return self.t

    def wait(self, secs):
        self.waits.append(secs)
        self.t += secs


class Base(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = pathlib.Path(self._tmp.name)
        self.state = self.tmp / "pool"
        self.sd = FakeSystemd()
        self.fc = FakeClock()
        self.events = []
        self.healthy = True

    def tearDown(self):
        self._tmp.cleanup()

    def emit(self, event, kind, data):
        events.validate(event, kind, data)  # every emitted event is V0-conformant
        self.events.append((event, kind, dict(data)))

    def health(self, port):
        self.last_health_port = port
        return self.healthy

    def get(self, sha=SHA_A, suite="memory", slot=3, port=PORT, root=None, argv=ARGV, **kw):
        # The constant shas name no real file: the payload "hashes" to the sha asked for.
        kw.setdefault("binary_hash", lambda path: sha)
        return pool.get(sha, suite, slot, argv, str(root or self.tmp / "root"), port,
                        health=self.health, emit=self.emit, systemd=self.sd,
                        state_dir=self.state, runner="lane-x", clock=self.fc.clock,
                        wait=self.fc.wait, **kw)

    def moves(self):
        return [(d["id"], d["from"], d["to"]) for _, _, d in self.events]


class Names(Base):
    def test_pool_unit_name_and_idle_timer(self):
        self.assertEqual(pool.unit_name(SHA_A, "memory", 3), "pipe-pool-abcdef01-memory-3")
        self.assertEqual(pool.idle_unit("pipe-pool-abcdef01-memory-3"),
                         "pipe-pool-abcdef01-memory-3-idle")
        self.assertEqual(pool.IDLE_MIN, 20)
        self.assertEqual(pool.RUNTIME_MAX, "4h")
        for bad in (("abc", "memory", 3), ("zzzzzzzz", "memory", 3), (SHA_A[:40], "memory", 3),
                    (SHA_A, "mem ory", 3), (SHA_A, "", 3), (SHA_A, "a/b", 3),
                    (SHA_A, "memory", 8)):
            with self.assertRaises(ValueError, msg=bad):
                pool.unit_name(*bad)

        col = self.get()
        unit = "pipe-pool-abcdef01-memory-3"
        self.assertEqual(col, pool.PoolColony(unit, PORT, str(self.tmp / "root"), False))
        start = ["systemd-run", "--user", "--unit", unit, "--collect",
                 "-p", "RuntimeMaxSec=4h", *ARGV]
        idle_stop = ["systemctl", "--user", "stop", unit + "-idle.timer"]
        idle_arm = ["systemd-run", "--user", "--unit", unit + "-idle",
                    "--on-active=20min", "systemctl", "--user", "stop", unit]
        self.assertEqual(self.sd.starts(), [start])
        self.assertEqual(self.sd.calls[-2:], [idle_stop, idle_arm])
        # Idle is the one-shot timer; RuntimeMaxSec only rides on the colony unit.
        self.assertFalse(any("RuntimeMaxSec" in a for a in idle_arm))
        self.assertLess(self.sd.calls.index(start), self.sd.calls.index(idle_arm))
        rec = json.loads((self.state / f"{unit}.json").read_text())
        self.assertEqual({k: rec[k] for k in ("sha", "suite", "slot", "port", "root")},
                         {"sha": SHA_A, "suite": "memory", "slot": 3, "port": PORT,
                          "root": str(self.tmp / "root")})
        self.assertTrue(rec["created_at"].endswith("Z"))


class Lifecycle(Base):
    def test_pool_idle_timer_rearms_on_get(self):
        unit = pool.unit_name(SHA_A, "memory", 3)
        idle_stop = ["systemctl", "--user", "stop", unit + "-idle.timer"]
        idle_arm = ["systemd-run", "--user", "--unit", unit + "-idle",
                    "--on-active=20min", "systemctl", "--user", "stop", unit]
        first = self.get()
        n_after_first = len(self.sd.calls)
        second = self.get()
        third = self.get()
        self.assertEqual((first.hit, second.hit, third.hit), (False, True, True))
        # One boot, three arms: every get stops the old one-shot and arms a new one.
        self.assertEqual(len(self.sd.starts()), 1)
        # (A miss also stops the timer while it collects the old unit.)
        self.assertGreaterEqual(self.sd.calls.count(idle_stop), 3)
        self.assertEqual(self.sd.calls.count(idle_arm), 3)
        tail = self.sd.calls[n_after_first:]
        self.assertEqual(tail[-2:], [idle_stop, idle_arm])
        for i, call in enumerate(self.sd.calls):
            if call == idle_arm:
                self.assertEqual(self.sd.calls[i - 1], idle_stop)
        # The first stop of the timer finds nothing loaded (rc 5) -- ignored.
        self.assertIn((unit + "-idle"), self.sd.active)

    def test_pool_hit_needs_health(self):
        self.get()
        self.healthy = False
        with self.assertRaises(pool.PoolError):
            self.get()
        unit = pool.unit_name(SHA_A, "memory", 3)
        # Active but unhealthy: stopped and booted again, which never got healthy.
        self.assertEqual(len(self.sd.starts()), 2)
        self.assertNotIn(unit, self.sd.active)
        self.assertFalse((self.state / f"{unit}.json").exists())

    def test_pool_boots_only_the_binary_whose_sha_keys_the_unit(self):
        """#1132: argv[0] must hash to `binary_sha` before anything boots."""
        binary = self.tmp / "meclaw"
        binary.write_bytes(b"\x7fELF pool payload")
        sha = hashlib.sha256(binary.read_bytes()).hexdigest()
        argv = [str(binary), *ARGV[1:]]
        col = self.get(sha=sha, argv=argv, binary_hash=None)
        self.assertFalse(col.hit)
        self.assertEqual(len(self.sd.starts()), 1)
        other = self.tmp / "other" / "meclaw"   # same basename, other bytes
        other.parent.mkdir()
        other.write_bytes(b"something else")
        for bad in ([str(other), *ARGV[1:]], [str(self.tmp / "missing" / "meclaw"), *ARGV[1:]]):
            with self.assertRaises(pool.PoolError):
                self.get(sha=SHA_B, argv=bad, binary_hash=None)
        self.assertEqual(len(self.sd.starts()), 1)   # no boot for either

    def test_pool_mismatched_sha_boots_new(self):
        unit = pool.unit_name(SHA_A, "memory", 3)
        self.get(sha=SHA_A)
        self.events.clear()
        # Same unit name (same 8-char prefix), different full sha: never a hit.
        col = self.get(sha=SHA_A2)
        self.assertFalse(col.hit)
        self.assertEqual(col.unit, unit)
        self.assertIn(["systemctl", "--user", "stop", unit], self.sd.calls)
        self.assertEqual(len(self.sd.starts()), 2)
        rec = json.loads((self.state / f"{unit}.json").read_text())
        self.assertEqual(rec["sha"], SHA_A2)
        wid = f"lane-x:pool:{unit}"
        self.assertEqual(self.moves(), [(wid, "warm", "collected"),
                                         (wid, "collected", "created"),
                                         (wid, "created", "warm")])
        # Another binary on the same slot port: the old colony holds the port, so
        # it is stopped and collected before the new unit boots.
        self.events.clear()
        col_b = self.get(sha=SHA_B)
        unit_b = pool.unit_name(SHA_B, "memory", 3)
        self.assertEqual(col_b.unit, unit_b)
        self.assertFalse(col_b.hit)
        stop_old = max(i for i, c in enumerate(self.sd.calls)
                       if c == ["systemctl", "--user", "stop", unit])
        start_b = self.sd.calls.index(self.sd.starts()[-1])
        self.assertLess(stop_old, start_b)
        self.assertNotIn(unit, self.sd.active)
        self.assertFalse((self.state / f"{unit}.json").exists())
        self.assertEqual(self.moves(), [(wid, "warm", "collected"),
                                         (f"lane-x:pool:{unit_b}", None, "created"),
                                         (f"lane-x:pool:{unit_b}", "created", "warm")])

    def test_pool_health_wait_is_one_deadline(self):
        answers = iter([False, False, True])
        self.health = lambda port: next(answers)
        col = self.get(timeout_s=30)
        self.assertFalse(col.hit)
        self.assertEqual(len(self.fc.waits), 2)
        self.assertLessEqual(sum(self.fc.waits), 30)

    def test_pool_health_timeout_stops_and_collects(self):
        self.healthy = False
        with self.assertRaises(pool.PoolError):
            self.get(timeout_s=10)
        unit = pool.unit_name(SHA_A, "memory", 3)
        self.assertAlmostEqual(sum(self.fc.waits), 10)
        self.assertNotIn(unit, self.sd.active)
        self.assertFalse((self.state / f"{unit}.json").exists())
        wid = f"lane-x:pool:{unit}"
        self.assertEqual(self.moves(), [(wid, None, "created"), (wid, "created", "collected")])

    def test_pool_start_failure_raises_without_events(self):
        self.sd.start_rc = 1
        with self.assertRaises(pool.PoolError):
            self.get()
        self.assertEqual(self.events, [])

    def test_pool_stop_collects(self):
        col = self.get()
        self.events.clear()
        self.assertTrue(pool.stop(col.unit, self.sd, emit=self.emit,
                                  state_dir=self.state, runner="lane-x"))
        self.assertNotIn(col.unit, self.sd.active)
        self.assertNotIn(col.unit + "-idle", self.sd.active)
        self.assertFalse((self.state / f"{col.unit}.json").exists())
        self.assertEqual(self.moves(), [(f"lane-x:pool:{col.unit}", "warm", "collected")])


    def test_pool_stop_refuses_units_without_the_pool_prefix(self):
        """Review W7 M1: `pool stop --unit` on mm-os01 would stop the live
        colony (`mm-os-e28`) or any other user unit -- only pool units go."""
        for unit in ("mm-os-e28", "mm-os-search-shim", "pipe-poolx-1", "pipe-pool-",
                     "pipe-pool-../../x", "pipe-pool-a b", "x-pipe-pool-1"):
            with self.assertRaises(pool.PoolError, msg=unit):
                pool.stop(unit, self.sd, emit=self.emit, state_dir=self.state)
        self.assertEqual([], self.sd.calls)

    def test_pool_cli_stop_refuses_a_foreign_unit(self):
        import argparse
        import contextlib
        import io
        ap = argparse.ArgumentParser()
        pool.register(ap.add_subparsers())
        args = ap.parse_args(["pool", "stop", "--unit", "mm-os-e28",
                              "--state-dir", str(self.state)])
        buf = io.StringIO()
        # Never the real systemctl, even while this test is red: a broken
        # guard must not stop a real user unit of the test host.
        with mock.patch.object(pool, "Systemd", lambda: self.sd), \
                contextlib.redirect_stdout(buf):
            self.assertEqual(2, args.func(args))
        self.assertIn("not a pool unit", buf.getvalue())
        self.assertEqual([], self.sd.calls)


FOREIGN_UNITS = ("mm-os-e28", "mm-os-e28.service", "mm-os-search-shim", "pipe-poolx-1",
                 "pipe-pool-", "pipe-pool-../../x", "pipe-pool-a b", "x-pipe-pool-1",
                 "pipe-run-0123456789ab", "default.target", "")


class ForeignUnits(Base):
    """Coordinator rule after the e28 incident (09.10. 02:28): the pool refuses
    every unit without its own fixed prefix -- start, stop and status alike --
    BEFORE any systemctl/systemd-run runs, in the API, in the CLI and at the
    real systemd boundary itself."""

    def foreign_argvs(self, unit):
        ok = pool.unit_name(SHA_A, "memory", 3)
        return [
            ["systemctl", "--user", "stop", unit],
            ["systemctl", "--user", "is-active", unit],
            ["systemctl", "--user", "start", unit],
            ["systemctl", "--user", "restart", unit],
            ["systemd-run", "--user", "--unit", unit, "--collect", "/bin/true"],
            # an idle timer of a pool unit that would stop a foreign unit later
            ["systemd-run", "--user", "--unit", ok + "-idle", "--on-active=20min",
             "systemctl", "--user", "stop", unit],
        ]

    def test_guard_refuses_every_verb_on_a_foreign_unit(self):
        guarded = pool.Guarded(self.sd)
        for unit in FOREIGN_UNITS:
            for argv in self.foreign_argvs(unit):
                with self.assertRaises(pool.PoolError, msg=argv):
                    guarded.run(argv)
        self.assertEqual([], self.sd.calls)

    def test_guard_refuses_other_verbs_and_tools_even_on_a_pool_unit(self):
        unit = pool.unit_name(SHA_A, "memory", 3)
        guarded = pool.Guarded(self.sd)
        for argv in (["systemctl", "--user", "start", unit],
                     ["systemctl", "--user", "restart", unit],
                     ["systemctl", "stop", unit],  # the system manager
                     ["systemctl", "--user", "stop", unit, "mm-os-e28"],
                     ["systemctl", "--user", "poweroff"],
                     ["systemd-run", "--unit", unit, "/bin/true"],
                     ["systemd-run", "--user", "/bin/true"],
                     ["ssh", "lane", "systemctl", "--user", "stop", unit],
                     ["poweroff"], []):
            with self.assertRaises(pool.PoolError, msg=argv):
                guarded.run(argv)
        self.assertEqual([], self.sd.calls)

    def systemd_run_escapes(self):
        """Re-check W7 N1: a valid pool `--unit` must not carry anything else
        through -- extra options, a foreign payload, a second `--unit=`."""
        unit = pool.unit_name(SHA_A, "memory", 3)
        head = ["systemd-run", "--user", "--unit", unit]
        return [
            head + ["--collect", "-p", "Conflicts=mm-os-e28.service", *ARGV],
            # an absolute systemctl path: the trap's fake, so no literal search
            # path and no host binary even if it ran (this argv is refused data,
            # never run; test_store_isolation pins the literal)
            head + ["--collect", os.path.join(side_effect_guard.current_dir(),
                                              "systemctl"), "--user",
                    "stop", "mm-os-e28"],
            head + ["--collect", "sh", "-c", "systemctl --user stop mm-os-e28"],
            head + ["--unit=mm-os-e28", "--collect", *ARGV],
            # `--user` alone in the scheduled command, not in front of --unit
            ["systemd-run", "--unit", unit, "--collect", "systemctl", "--user",
             "stop", unit],
            head + ["--collect", "-p", "RuntimeMaxSec=4h", "-p", "RuntimeMaxSec=4h",
                    *ARGV],
            head + ["--collect", "-p", "RuntimeMaxSec=4h", "--wait", *ARGV],
            head + ["--on-active=20min", "systemctl", "--user", "stop", unit,
                    "mm-os-e28"],
            head + ["--on-active=20min", "systemctl", "--user", "stop", "mm-os-e28"],
            head + ["--on-active=20min", "systemctl", "--user", "start", unit],
            head + ["--collect"],
            head + ["--collect", "/opt/bin/meclaw-evil", "--root", "/tmp/r"],
            ["systemd-run", "--user", "--collect", "--unit", unit, *ARGV],
        ]

    def test_systemd_run_refuses_options_and_payloads_beyond_the_pool_form(self):
        guarded = pool.Guarded(self.sd)
        for argv in self.systemd_run_escapes():
            with self.assertRaises(pool.PoolError, msg=argv):
                guarded.run(argv)
        self.assertEqual([], self.sd.calls)

    def test_systemd_run_escapes_never_reach_a_real_subprocess(self):
        never = mock.Mock(side_effect=AssertionError("real subprocess reached"))
        with mock.patch.object(pool.subprocess, "run", never):
            for argv in self.systemd_run_escapes():
                with self.assertRaises(pool.PoolError, msg=argv):
                    pool.Systemd().run(argv)
        never.assert_not_called()

    def test_systemd_run_accepts_exactly_the_two_pool_forms(self):
        unit = pool.unit_name(SHA_A, "memory", 3)
        ok = [["systemd-run", "--user", "--unit", unit, "--collect",
               "-p", "RuntimeMaxSec=4h", *ARGV],
              ["systemd-run", "--user", "--unit", pool.idle_unit(unit),
               "--on-active=20min", "systemctl", "--user", "stop", unit]]
        done = mock.Mock(return_value=mock.Mock(returncode=0, stdout="", stderr=""))
        with mock.patch.object(pool.subprocess, "run", done):
            for argv in ok:
                self.assertEqual(0, pool.Systemd().run(argv)[0], msg=argv)
        self.assertEqual(2, done.call_count)

    def test_status_and_stop_refuse_a_foreign_unit_without_calling_systemd(self):
        for unit in FOREIGN_UNITS:
            with self.assertRaises(pool.PoolError, msg=unit):
                pool.status(unit, self.sd)
            with self.assertRaises(pool.PoolError, msg=unit):
                pool.stop(unit, self.sd, emit=self.emit, state_dir=self.state)
        self.assertEqual([], self.sd.calls)

    def test_status_answers_for_a_pool_unit(self):
        col = self.get()
        self.assertEqual("active", pool.status(col.unit, self.sd))
        pool.stop(col.unit, self.sd, emit=self.emit, state_dir=self.state)
        self.assertEqual("inactive", pool.status(col.unit, self.sd))

    def test_real_systemd_never_spawns_for_a_foreign_unit(self):
        """The guard sits in `Systemd.run` itself: even a caller that skips the
        API (or passes no fake) cannot reach a real `systemctl` with mm-os-e28."""
        never = mock.Mock(side_effect=AssertionError("real subprocess reached"))
        with mock.patch.object(pool.subprocess, "run", never):
            for unit in FOREIGN_UNITS:
                for argv in self.foreign_argvs(unit):
                    with self.assertRaises(pool.PoolError, msg=argv):
                        pool.Systemd().run(argv)
        never.assert_not_called()

    def test_pool_cli_refuses_a_foreign_unit_on_every_action(self):
        import argparse
        import contextlib
        import io
        ap = argparse.ArgumentParser()
        pool.register(ap.add_subparsers())
        for action in ("stop", "status"):
            for unit in ("mm-os-e28", "mm-os-search-shim", "pipe-run-0123456789ab"):
                args = ap.parse_args(["pool", action, "--unit", unit,
                                      "--state-dir", str(self.state)])
                buf = io.StringIO()
                with mock.patch.object(pool, "Systemd", lambda: self.sd), \
                        contextlib.redirect_stdout(buf):
                    self.assertEqual(2, args.func(args), (action, unit))
                self.assertIn("not a pool unit", buf.getvalue())
        self.assertEqual([], self.sd.calls)

    def test_pool_cli_status_of_a_pool_unit(self):
        import argparse
        import contextlib
        import io
        col = self.get()
        ap = argparse.ArgumentParser()
        pool.register(ap.add_subparsers())
        args = ap.parse_args(["pool", "status", "--unit", col.unit,
                              "--state-dir", str(self.state)])
        buf = io.StringIO()
        with mock.patch.object(pool, "Systemd", lambda: self.sd), \
                contextlib.redirect_stdout(buf):
            self.assertEqual(0, args.func(args))
        self.assertIn("active", buf.getvalue())
        self.sd.active.discard(col.unit)
        buf = io.StringIO()
        with mock.patch.object(pool, "Systemd", lambda: self.sd), \
                contextlib.redirect_stdout(buf):
            self.assertEqual(3, args.func(args))
        self.assertIn("inactive", buf.getvalue())


class Rows(Base):
    def test_pool_rows_in_workspaces(self):
        db = store.open(self.tmp / "p.db")
        self.addCleanup(store.close, db)
        col = self.get()
        for _, _, data in self.events:
            pool.apply_workspace_event(db, data)
        wid = f"lane-x:pool:{col.unit}"
        row = dict(db.execute("SELECT * FROM workspaces WHERE id=?", (wid,)).fetchone())
        self.assertEqual((row["runner"], row["kind"], row["path"], row["state"]),
                         ("lane-x", "pool", str(self.tmp / "root"), "warm"))
        self.assertIsNotNone(row["created_at"])
        self.assertEqual(row["ttl_s"], pool.IDLE_MIN * 60)
        self.events.clear()
        pool.stop(col.unit, self.sd, emit=self.emit, state_dir=self.state, runner="lane-x")
        for _, _, data in self.events:
            pool.apply_workspace_event(db, data)
        row = dict(db.execute("SELECT * FROM workspaces WHERE id=?", (wid,)).fetchone())
        self.assertEqual(row["state"], "collected")
        self.assertIsNotNone(row["collected_at"])
        self.assertEqual(db.execute("SELECT count(*) FROM workspaces").fetchone()[0], 1)


class Report(unittest.TestCase):
    def test_pool_report_written_from_env(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp) / "pool.json"
            rep = pool.PoolReport("memory")
            rep.case("c2", pool.PoolColony("u", 1, "/r", True))
            rep.case("c1", pool.PoolColony("u", 1, "/r", False))
            rep.case("c3", None)
            self.assertIsNone(rep.write(environ={}))
            self.assertFalse(path.exists())
            self.assertEqual(rep.write(environ={"MECLAW_POOL_REPORT": str(path)}), str(path))
            self.assertEqual(json.loads(path.read_text()),
                             {"suite": "memory", "hits": 1, "boots": 1,
                              "cases": {"c1": "pool", "c2": "pool", "c3": "fresh"}})


if __name__ == "__main__":
    unittest.main()
