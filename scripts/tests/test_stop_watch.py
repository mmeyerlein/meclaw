"""Tests for `scripts/stop_watch.py` -- the stop watch beside a long run (GH #1075).

The rule (plans/PREAMBLE.md section 6.9): every run longer than an hour writes
one line per unit, and a watcher decides after each line, deterministically,
whether the target is still reachable and whether the running rate is
statistically below it. These tests pin both verdicts, the margin where it must
NOT stop, that the signal reaches the remembered run's own process group and
nothing else, that a verdict on the last unit sends no signal, and that a broken
history line is loud.
"""

import contextlib
import io
import json
import os
import pathlib
import signal
import subprocess
import sys
import tempfile
import unittest
from unittest import mock
from scripts.tests import kit_time, store_isolation

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO))

from scripts import gate_plan, stop_watch  # noqa: E402

SCRIPT = "scripts/stop_watch.py"
# A hang marker, never a verdict (kit_time, TV-Z1).
HANG = kit_time.HANG


def armed(pid):
    """True once the watcher `pid` blocks on its follow path: an inotify fd with
    a watch on it (`inotify wd:` in its fdinfo), or the `tail -F` child of the
    fallback. Read from /proc -- an event of the watcher, not a guessed delay."""
    fdinfo = pathlib.Path("/proc/%d/fdinfo" % pid)
    try:
        for entry in fdinfo.iterdir():
            if "inotify wd:" in entry.read_text(encoding="utf-8", errors="replace"):
                return True
        children = pathlib.Path("/proc/%d/task/%d/children" % (pid, pid))
        return bool(children.read_text(encoding="utf-8").split())
    except OSError:
        return False


def alive(pid):
    """True while `pid` exists and is not a zombie (read from /proc)."""
    try:
        stat = pathlib.Path("/proc/%d/stat" % pid).read_text(encoding="utf-8")
    except OSError:
        return False
    return stat.rsplit(")", 1)[1].split()[0] != "Z"


def child_of(pid):
    """The first child PID of `pid`, or None (read from /proc)."""
    path = pathlib.Path("/proc/%d/task/%d/children" % (pid, pid))
    try:
        kids = path.read_text(encoding="utf-8").split()
    except OSError:
        return None
    return int(kids[0]) if kids else None


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()


def tearDownModule():  # noqa: N802 -- unittest's name
    store_isolation.restore()


def feed(watch, rows):
    """Feed `(class, ok)` rows; return the first non-None verdict and its index."""
    for i, (cls, ok) in enumerate(rows, 1):
        verdict = watch.add({"unit": "u%d" % i, "class": cls,
                             "result": "ok" if ok else "fail"})
        if verdict is not None:
            return verdict, i
    return None, len(rows)


class TestVerdicts(unittest.TestCase):
    def test_unreachable_after_k_units(self):
        # 10 units, target 0.85: two failures in class a cap it at 8/10 = 0.80
        # even if all eight remaining units land in a and pass.
        watch = stop_watch.Watch(total=10, target=0.85, min_units=20)
        verdict, at = feed(watch, [("a", True), ("a", False), ("a", False)])
        self.assertEqual(at, 3)
        self.assertEqual(verdict.kind, "unreachable")
        self.assertEqual(verdict.cls, "a")

    def test_one_failure_is_still_reachable(self):
        watch = stop_watch.Watch(total=10, target=0.85, min_units=20)
        verdict, _ = feed(watch, [("a", False), ("a", True)])
        self.assertIsNone(verdict)

    def test_wilson_abort_from_min_units(self):
        # 12/19 is below target but under --min-units: no verdict. The 20th
        # unit makes 13/20, whose 95 % Wilson upper bound is 0.819 < 0.85.
        watch = stop_watch.Watch(total=1000, target=0.85, min_units=20)
        rows = [("a", True)] * 12 + [("a", False)] * 7
        verdict, _ = feed(watch, rows)
        self.assertIsNone(verdict)
        verdict = watch.add({"unit": "u20", "class": "a", "result": "ok"})
        self.assertIsNotNone(verdict)
        self.assertEqual(verdict.kind, "below")
        self.assertAlmostEqual(stop_watch.wilson_upper(13, 20), 0.8188, places=3)

    def test_no_abort_just_above_target(self):
        # 87/100 against 0.85, total far away: neither rule may fire.
        watch = stop_watch.Watch(total=1000, target=0.85, min_units=20)
        rows = ([("a", True)] * 87 + [("a", False)] * 13)
        verdict, _ = feed(watch, rows)
        self.assertIsNone(verdict)

    def test_classes_are_judged_separately(self):
        # Class a's clean record does not lift class b: after five a passes
        # and three b failures, b can still reach (0 + 22) / 25 = 0.88 if every
        # remaining unit is a passing b; a fourth b failure caps it at 0.84.
        watch = stop_watch.Watch(total=30, target=0.85, min_units=20)
        verdict, _ = feed(watch, [("a", True)] * 5 + [("b", False)] * 3)
        self.assertIsNone(verdict)
        verdict = watch.add({"unit": "x", "class": "b", "result": "fail"})
        self.assertEqual((verdict.kind, verdict.cls), ("unreachable", "b"))

    def test_broken_lines_are_loud(self):
        for raw, why in (("not json", r"^line 1: not JSON"),
                         ('{"unit": "u1", "class": "a"}', r"^line 1: result must be ok\|fail"),
                         ('{"unit": "u1", "class": "a", "result": "maybe"}',
                          r"^line 1: result must be ok\|fail"),
                         ('["u1", "a", "ok"]', r"^line 1: not a JSON object")):
            with self.subTest(raw=raw):
                with self.assertRaisesRegex(stop_watch.HistoryError, why):
                    stop_watch.parse_line(raw, 1)

    def test_duplicate_unit_and_overrun_are_loud(self):
        watch = stop_watch.Watch(total=1, target=0.5, min_units=20)
        watch.add({"unit": "u1", "class": "a", "result": "ok"})
        with self.assertRaisesRegex(stop_watch.HistoryError, r"^unit 'u1' appears twice$"):
            watch.add({"unit": "u1", "class": "a", "result": "ok"})
        with self.assertRaisesRegex(stop_watch.HistoryError, r"^more than --total 1 units$"):
            watch.add({"unit": "u2", "class": "a", "result": "ok"})

    def test_gate_plans_the_selftest_for_the_script(self):
        self.assertIn("gate_infra", gate_plan.classify([SCRIPT]))


class TestTarget(unittest.TestCase):
    def test_refuses_pids_that_reach_more_than_one_process(self):
        for pid in (0, 1, -5, os.getpid()):
            with self.subTest(pid=pid):
                with self.assertRaisesRegex(ValueError, r"^refusing --pid %d: " % pid):
                    stop_watch.Target(pid)

    def test_refuses_a_pid_that_does_not_lead_its_own_group(self):
        # A run in the caller's group: signalling its group would reach the
        # caller (and this test) as well, so the watcher refuses it up front.
        foreign = subprocess.Popen(["sleep", "60"])
        try:
            with self.assertRaisesRegex(ValueError, r"^refusing --pid %d: not the leader "
                                        r"of its own process group" % foreign.pid):
                stop_watch.Target(foreign.pid)
            self.assertIsNone(foreign.poll())
        finally:
            foreign.kill()
            foreign.wait()

    def test_reused_pid_is_never_signalled(self):
        foreign = subprocess.Popen(["sleep", "60"], start_new_session=True)
        try:
            target = stop_watch.Target(foreign.pid)
            target.start = "not-the-recorded-start"  # as after a PID reuse
            # No signal leaves the watcher: both send paths are recorded, so
            # "nothing sent" is read, not waited for.
            with mock.patch.object(stop_watch.signal, "pidfd_send_signal", create=True) as pidfd, \
                    mock.patch.object(stop_watch.os, "kill") as kill, \
                    mock.patch.object(stop_watch.os, "killpg") as killpg:
                self.assertFalse(target.terminate())
            self.assertEqual([], pidfd.call_args_list + kill.call_args_list
                             + killpg.call_args_list)
            self.assertIsNone(foreign.poll())
        finally:
            foreign.kill()
            foreign.wait()


class TestEndToEnd(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = pathlib.Path(self.tmp.name)
        self.history = self.dir / "history.jsonl"
        self.report = self.dir / "report.txt"
        self.procs = []

    def tearDown(self):
        for p in self.procs:
            if p.poll() is None:
                p.kill()
            p.wait()
        self.tmp.cleanup()

    def spawn(self, argv, **kw):
        p = subprocess.Popen(argv, **kw)
        self.procs.append(p)
        return p

    def watcher(self, pid, total, env=None, target="0.85"):
        return self.spawn([sys.executable, str(REPO / SCRIPT),
                           "--history", str(self.history), "--total", str(total),
                           "--target", target, "--pid", str(pid),
                           "--report", str(self.report)],
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                          text=True, env=env)

    def append(self, *rows):
        with open(self.history, "a", encoding="utf-8") as fh:
            for row in rows:
                fh.write((row if isinstance(row, str) else json.dumps(row)) + "\n")

    def _abort_hits_only_the_target(self, env=None):
        run = self.spawn(["sleep", "120"], start_new_session=True)
        foreign = self.spawn(["sleep", "120"], start_new_session=True)
        w = self.watcher(run.pid, 10, env=env)
        # The lines arrive while the watcher follows (the path under test).
        kit_time.until("the watcher follows the history", lambda: armed(w.pid) or w.poll() is not None)
        self.append({"unit": "u1", "class": "a", "result": "fail"})
        self.append({"unit": "u2", "class": "a", "result": "fail"})
        out, err = w.communicate(timeout=HANG)
        self.assertEqual(w.returncode, 1, err)
        self.assertEqual(run.wait(timeout=HANG), -signal.SIGTERM)
        self.assertIsNone(foreign.poll())
        text = self.report.read_text(encoding="utf-8")
        self.assertIn("ABORT unreachable", text)
        self.assertRegex(text, r"(?m)^a\s+2\s+0\s+0\.000\s")

    def test_abort_signals_the_remembered_pid_only(self):
        self._abort_hits_only_the_target()

    def test_abort_without_inotify_follows_like_tail(self):
        env = dict(os.environ, MECLAW_STOP_WATCH_NO_INOTIFY="1")
        self._abort_hits_only_the_target(env=env)

    def test_abort_reaches_every_process_of_the_run_group(self):
        # The run is a chain: a shell that waits on its working child. The
        # stop must end the child too, not leave it running as an orphan.
        run = self.spawn(["sh", "-c", "sleep 120; true"], start_new_session=True)
        child = kit_time.until("the run chain starts its child", lambda: child_of(run.pid))
        w = self.watcher(run.pid, 10)
        kit_time.until("the watcher follows the history", lambda: armed(w.pid) or w.poll() is not None)
        self.append({"unit": "u1", "class": "a", "result": "fail"},
                    {"unit": "u2", "class": "a", "result": "fail"})
        _, err = w.communicate(timeout=HANG)
        self.assertEqual(w.returncode, 1, err)
        self.assertEqual(run.wait(timeout=HANG), -signal.SIGTERM)
        kit_time.until("the child of the run chain ends", lambda: not alive(child))

    def test_a_verdict_on_the_last_unit_sends_no_signal(self):
        # total 3, target 0.5: after ok, fail the best case is 2/3; the third
        # unit (fail) makes 1/3 -- a verdict, but the run has produced every
        # unit, so a SIGTERM would only hit its wrap-up.
        run = self.spawn(["sleep", "120"], start_new_session=True)
        self.append({"unit": "u1", "class": "a", "result": "ok"},
                    {"unit": "u2", "class": "a", "result": "fail"},
                    {"unit": "u3", "class": "a", "result": "fail"})
        out = io.StringIO()
        with mock.patch.object(stop_watch.Target, "terminate", return_value=True) as term, \
                contextlib.redirect_stdout(out):
            code = stop_watch.main(["--history", str(self.history), "--total", "3",
                                    "--target", "0.5", "--pid", str(run.pid),
                                    "--report", str(self.report)])
        self.assertEqual([], term.call_args_list)
        self.assertEqual(code, stop_watch.EXIT_ABORT)
        text = self.report.read_text(encoding="utf-8")
        self.assertIn("ABORT unreachable", text)
        self.assertIn("(not signalled: the run has produced all --total units)", text)
        self.assertIsNone(run.poll())

    def test_broken_line_exits_loud_and_spares_the_run(self):
        run = self.spawn(["sleep", "120"], start_new_session=True)
        self.append({"unit": "u1", "class": "a", "result": "ok"}, "garbage")
        w = self.watcher(run.pid, 10)
        _, err = w.communicate(timeout=HANG)
        self.assertEqual(w.returncode, 2)
        self.assertIn("line 2", err)
        self.assertIsNone(run.poll())
        self.assertIn("ERROR", self.report.read_text(encoding="utf-8"))

    def test_complete_run_exits_zero(self):
        run = self.spawn(["sleep", "120"], start_new_session=True)
        w = self.watcher(run.pid, 3)
        self.append(*({"unit": "u%d" % i, "class": "a", "result": "ok"}
                      for i in range(3)))
        _, err = w.communicate(timeout=HANG)
        self.assertEqual(w.returncode, 0, err)
        self.assertIsNone(run.poll())
        self.assertIn("COMPLETE", self.report.read_text(encoding="utf-8"))

    def test_target_gone_ends_the_watch(self):
        run = self.spawn(["sleep", "120"], start_new_session=True)
        w = self.watcher(run.pid, 10)
        self.append({"unit": "u1", "class": "a", "result": "ok"})
        # The run ends while the watcher follows it (pidfd or tail --pid).
        kit_time.until("the watcher follows the history", lambda: armed(w.pid) or w.poll() is not None)
        run.terminate()
        run.wait()
        _, err = w.communicate(timeout=HANG)
        self.assertEqual(w.returncode, 3, err)
        self.assertIn("TARGET GONE", self.report.read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
