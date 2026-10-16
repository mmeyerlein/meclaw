"""GH #1199 (kit find V2-A8, welle-validierung-2): long kit runs report progress
unbuffered, refuse loudly before the start, and show their liveness in `token who`.

Measured 14.10.: a builder ran 17:45-21:40 without a result. `flake_triage.py`,
started without `-I`, refused at the lane's narrow point with one stderr line
the background wrapper never kept (its log stayed empty, exit 2), and a
running series printed nothing until its end (stdout block-buffered into a
file). Nobody saw it for four hours: `token who` showed a held lane.

Every pin decides on an event (a line in a file after pass 1, a refusal line,
a store row) -- the kit clock is injected, nothing sleeps.
"""
import io
import os
import pathlib
import re
import subprocess
import sys
import tempfile
import threading
import types
import unittest
from unittest import mock

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO))
sys.path.insert(0, str(REPO / "scripts"))
import flake_triage as ft  # noqa: E402
from scripts.pipeline import coverage, gate, lane, progress, schedule, store  # noqa: E402
from scripts.tests import kit_time, store_isolation  # noqa: E402
from scripts.tests.kit_time import FakeClock  # noqa: E402


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()


def tearDownModule():  # noqa: N802 -- unittest's name
    store_isolation.restore()


T0 = 1_800_000_000
PASS_LINE = r"(?m)^\d\d:\d\d:\d\d pass %d/%d done=%d red=%d$"
UNITTEST_OK = "test_x (m.T.test_x) ... ok\n\n" + "-" * 70 + "\nRan 1 test in 0.001s\n\nOK\n"


def _held(holder, run=None, now=T0):
    """An armed token pool in the isolated store with `holder` holding one token."""
    db = store.open()
    if schedule.lease_who(db, now=now).out.startswith("cargo tokens are not armed"):
        assert schedule.lease_init(db, max=4, now=now).code == 0
    assert schedule.lease_take(db, holder, run=run, now=now).code == 0
    return db


class FreshStore(unittest.TestCase):
    """Each test its own store (the module's isolated one is shared by its
    tests, and a pool of four tokens fills up across them)."""

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        env = mock.patch.dict(os.environ, {"MECLAW_PIPELINE_DB": str(pathlib.Path(tmp.name) / "p.db")})
        env.start()
        self.addCleanup(env.stop)


def _holder_line(db, holder, now):
    out = schedule.lease_who(db, now=now).out
    return next(ln for ln in out.splitlines() if ln.startswith("  " + holder + " ")), out


class ProgressLineInTheLog(FreshStore):
    """(1) A long run writes one line per pass into its log at once."""

    def test_the_python_series_writes_pass_one_into_the_log_before_pass_two_runs(self):
        with tempfile.TemporaryDirectory() as tmp:
            log = pathlib.Path(tmp) / "run.log"
            seen = []

            def run(argv, hang_s):
                seen.append(log.read_text())   # the log as a reader sees it now
                return subprocess.CompletedProcess(argv, 0, "", UNITTEST_OK)
            # a local series in any interpreter (re-check v2-fix-e M4: no lane, no -I)
            with open(log, "w") as fh, mock.patch("sys.stdout", fh), mock.patch("sys.stderr", fh), \
                    mock.patch.object(ft, "isolated", return_value=False), \
                    mock.patch.object(ft, "_run_unittest", run):
                rc = ft.main(["--unittest", "--host", "local", "--runs", "2", "--no-shuffle", "m.T.test_x"])
            self.assertEqual(0, rc, log.read_text())
            self.assertEqual(2, len(seen))
            self.assertNotRegex(seen[0], PASS_LINE % (1, 2, 1, 0))
            self.assertRegex(seen[1], PASS_LINE % (1, 2, 1, 0))
            self.assertRegex(log.read_text(), PASS_LINE % (2, 2, 2, 0))

    def test_a_local_python_series_runs_without_dash_i(self):
        """Re-check v2-fix-e M4: `--host local` reaches no lane, so no narrow
        point -- the -I refusal is for lane runs only."""
        out, err = io.StringIO(), io.StringIO()
        with mock.patch.object(ft, "isolated", return_value=False), \
                mock.patch.object(ft, "_run_unittest",
                                  lambda argv, hang_s: subprocess.CompletedProcess(argv, 0, "", UNITTEST_OK)), \
                mock.patch("sys.stdout", out), mock.patch("sys.stderr", err):
            rc = ft.main(["--unittest", "--host", "local", "--runs", "1", "--no-shuffle", "m.T.test_x"])
        self.assertEqual(0, rc, err.getvalue())
        self.assertNotIn("REFUSED", out.getvalue() + err.getvalue())

    def _nextest_series(self, out, err, child):
        """`flake_triage.py <bin> <test> --runs 2` (nextest class) with `child`
        standing in for `strand.sh test`: it gets the keywords of the real call."""
        with mock.patch.object(ft, "isolated", return_value=True), \
                mock.patch.object(ft.subprocess, "run", child), \
                mock.patch("sys.stdout", out), mock.patch("sys.stderr", err):
            return ft.main(["meclaw-cells::misc_1", "t", "--host", "local", "--runs", "2"])

    def test_the_nextest_series_relays_each_pass_line_into_a_stdout_only_log_while_it_runs(self):
        """Re-check v2-fix-e I1(a): `strand.sh test` writes its pass lines on
        its stdout and stderr; flake_triage kept the child's stdout captured to
        the end, so `flake_triage.py ... > log` (stdout only, V2-A8's wrapper)
        held no pass line while the series ran. The child here behaves like the
        real one: each line onto the stdout flake_triage hands it (a pipe it
        reads only at the end, or a stream it relays) and onto the inherited stderr."""
        with tempfile.TemporaryDirectory() as tmp:
            log = pathlib.Path(tmp) / "run.log"
            relayed, seen = threading.Event(), []

            class Spy:   # the log's stream: a flush after a pass-1 line is the event
                def __init__(self, fh):
                    self._fh = fh

                def write(self, text):
                    self._fh.write(text)

                def flush(self):
                    self._fh.flush()
                    if "pass 1/2" in log.read_text():
                        relayed.set()

                def __getattr__(self, name):
                    return getattr(self._fh, name)

            def child(argv, stdout=None, **k):
                captured = []

                def say(text):
                    print(text, file=sys.stderr, flush=True)       # the inherited stderr
                    if isinstance(stdout, int) and stdout >= 0:      # a stream flake_triage reads now
                        os.write(stdout, (text + "\n").encode())
                    else:                                            # a pipe read at the end
                        captured.append(text + "\n")
                say("12:00:00 pass 1/2 done=1 red=0")
                if isinstance(stdout, int) and stdout >= 0:
                    relayed.wait(kit_time.HANG)
                seen.append(log.read_text())          # the log as a reader sees it before pass 2
                say("12:00:01 pass 2/2 done=2 red=0")
                return subprocess.CompletedProcess(argv, 0, "".join(captured) or None, None)
            with open(log, "w") as fh, open(os.devnull, "w") as devnull:
                self._nextest_series(Spy(fh), devnull, child)
            self.assertEqual(1, len(seen))
            self.assertRegex(seen[0], PASS_LINE % (1, 2, 1, 0))
            self.assertRegex(log.read_text(), PASS_LINE % (2, 2, 2, 0))

    def test_one_log_for_both_streams_holds_each_pass_line_once(self):
        """`> log 2>&1`: the child's stderr copy is in the log already, the
        relay adds none."""
        with tempfile.TemporaryDirectory() as tmp:
            log = pathlib.Path(tmp) / "run.log"

            def child(argv, stdout=None, **k):
                print("12:00:00 pass 1/1 done=1 red=0", file=sys.stderr, flush=True)
                text = "12:00:00 pass 1/1 done=1 red=0\nstrand: no test ran\n"   # the run's last line
                if isinstance(stdout, int) and stdout >= 0:
                    os.write(stdout, text.encode())
                    return subprocess.CompletedProcess(argv, 0, None, None)
                return subprocess.CompletedProcess(argv, 0, text, None)
            with open(log, "w") as fh:
                self._nextest_series(fh, fh, child)
            self.assertEqual(1, len(re.findall(PASS_LINE % (1, 1, 1, 0), log.read_text())), log.read_text())

    def test_a_refusal_of_strand_test_reaches_a_stdout_only_log(self):
        """The child's `pipeline test: REFUSED` line travels like a pass line."""
        refusal = "pipeline test: REFUSED (exit 3) -- strand: no lane held"
        with tempfile.TemporaryDirectory() as tmp:
            log = pathlib.Path(tmp) / "run.log"

            def child(argv, stdout=None, **k):
                print(refusal, file=sys.stderr, flush=True)
                if isinstance(stdout, int) and stdout >= 0:
                    os.write(stdout, (refusal + "\n").encode())
                    return subprocess.CompletedProcess(argv, 3, None, None)
                return subprocess.CompletedProcess(argv, 3, refusal + "\n", None)
            with open(log, "w") as fh, open(os.devnull, "w") as devnull:
                rc = self._nextest_series(fh, devnull, child)
            self.assertNotEqual(0, rc)
            self.assertIn(refusal, log.read_text())

    def test_strand_test_relays_each_stress_iteration_while_the_lane_runs(self):
        """`strand.sh test` (gate._test): the lane's output goes line by line
        into test.log, and each stress iteration is a pass line on the
        caller's stream before the next iteration arrives."""
        with tempfile.TemporaryDirectory() as tmp:
            fake = pathlib.Path(tmp) / "lane_sync.sh"
            fake.write_text("#!/bin/sh\n"
                            "echo 'Stress test [ 1.0s] iteration 1/2: 1 test run: 1 passed'\n"
                            "echo 'Stress test [ 1.0s] iteration 2/2: 1 test run: 0 passed, 1 failed'\n")
            fake.chmod(0o755)
            log = pathlib.Path(tmp) / "test.log"
            out = io.StringIO()
            prog = gate.TestProgress(out, None, 2, clock=FakeClock().time)
            seen = []

            def on_line(text):
                seen.append((out.getvalue(), log.read_text()))
                prog.line(text)
            ln = types.SimpleNamespace(name="lane-a", root="/r", cls="", threads=0, jobs=0, slots=0,
                                       opts={}, ssh="u@lane-a.invalid")
            prog.start()
            with mock.patch.object(lane, "LANE_SYNC", str(fake)):
                rc = lane.run_test(ln, "tree", "expr", [], str(log), on_line=on_line)
            self.assertEqual(0, rc)
            self.assertRegex(seen[0][0], PASS_LINE % (0, 2, 0, 0))
            self.assertIn("iteration 1/2", seen[1][1])        # the log holds line 1 already
            self.assertRegex(seen[1][0], PASS_LINE % (1, 2, 1, 0))
            self.assertRegex(out.getvalue(), PASS_LINE % (2, 2, 2, 1))

    def test_the_coverage_build_says_its_pass_started_before_the_lane_runs(self):
        with tempfile.TemporaryDirectory() as tmp:
            hosts = pathlib.Path(tmp) / "lanes"
            hosts.write_text("lane-a user@lane-a.invalid\n")
            log = pathlib.Path(tmp) / "cov.log"
            seen = []

            def fake_run(cmd, **kw):
                if cmd[0] not in ("scripts/strand.sh", "timeout"):   # git of the lease key
                    return subprocess.CompletedProcess(cmd, 0, "", "")
                seen.append((cmd[0], log.read_text()))
                return subprocess.CompletedProcess(cmd, 0 if len(seen) == 1 else 9)
            args = types.SimpleNamespace(op="build", lane="lane-a", strand="welle-x/c",
                                         out_dir=str(pathlib.Path(tmp) / "out"), db=None, scope="full")
            with open(log, "w") as fh, mock.patch.dict(os.environ, {"MECLAW_LANES_FILE": str(hosts)}), \
                    mock.patch.object(coverage, "lane_up", return_value=True), \
                    mock.patch.object(coverage.subprocess, "run", side_effect=fake_run), \
                    mock.patch("sys.stdout", fh), mock.patch("sys.stderr", fh):
                coverage._cmd_coverage(args)
            # token check, the lane run, the `seen` check-in after the run (v2-fix-b, K9)
            self.assertEqual(["scripts/strand.sh", "timeout", "scripts/strand.sh"],
                             [c for c, _ in seen])
            self.assertRegex(seen[1][1], PASS_LINE % (0, 1, 0, 0))


class LoudRefusal(FreshStore):
    """(2) A refusal before the run: exit != 0, one line on stderr AND in the log."""

    def test_flake_triage_without_dash_i_refuses_on_stderr_in_the_log_and_at_the_lease(self):
        db = _held("welle-x/ft-a")
        with tempfile.TemporaryDirectory() as tmp:
            log, table = pathlib.Path(tmp) / "run.log", pathlib.Path(tmp) / "table.md"
            with open(log, "w") as fh:
                # no -I: the start V2-A8 measured (`python3 scripts/flake_triage.py ...`)
                r = subprocess.run([sys.executable, str(REPO / "scripts" / "flake_triage.py"),
                                    "--unittest", "--host", "lane-nowhere", "--runs", "1",
                                    "--wave", "welle-x", "--strand", "ft-a", "--out", str(table),
                                    "m.T.test_x"],
                                   cwd=str(REPO), stdout=fh, stderr=subprocess.PIPE, text=True,
                                   env=dict(os.environ), timeout=kit_time.HANG)
            self.assertNotEqual(0, r.returncode)
            self.assertRegex(r.stderr, r"flake_triage: REFUSED -- .*-I")
            self.assertRegex(log.read_text(), r"flake_triage: REFUSED -- .*-I")
            self.assertRegex(table.read_text(), r"flake_triage: REFUSED -- .*-I")
        line, out = _holder_line(db, "welle-x/ft-a", None)
        self.assertIn("REFUSED", line, out)

    def test_a_lane_error_before_the_start_is_a_loud_refusal_not_a_traceback(self):
        out, err = io.StringIO(), io.StringIO()
        with mock.patch.object(ft, "lane_target", side_effect=lane.LaneError(2, "no lane file")), \
                mock.patch.object(ft, "isolated", return_value=True), \
                mock.patch("sys.stdout", out), mock.patch("sys.stderr", err):
            rc = ft.main(["--unittest", "--host", "lane-b", "--runs", "1", "--strand", "s", "m.T"])
        self.assertEqual(2, rc)
        for stream in (out, err):
            self.assertIn("flake_triage: REFUSED -- no lane file", stream.getvalue())

    def _both(self, argv, **patches):
        """`flake_triage.main(argv)` in the kit's Python: (exit, stdout, stderr)."""
        out, err = io.StringIO(), io.StringIO()
        with mock.patch.object(ft, "isolated", return_value=True), \
                mock.patch.object(ft.subprocess, "run", side_effect=AssertionError("ran something")), \
                mock.patch("sys.stdout", out), mock.patch("sys.stderr", err):
            rc = ft.main(argv)
        return rc, out.getvalue(), err.getvalue()

    def test_a_python_series_with_a_nextest_log_refuses_on_both_streams(self):
        """Re-check v2-fix-e M1: `--unittest --log` said so on stderr only."""
        rc, out, err = self._both(["--unittest", "--log", "x.log", "--host", "local", "m.T.test_x"])
        self.assertEqual(2, rc)
        for text in (out, err):
            self.assertIn("flake_triage: REFUSED -- --log reads nextest logs", text)

    def test_an_odd_test_list_is_a_refusal_on_both_streams_not_a_bare_exit(self):
        """Re-check v2-fix-e M1: `pairs()` ended the run as SystemExit -- one
        stderr line, no exit code a caller could tell from argparse."""
        rc, out, err = self._both(["meclaw-cells::misc_1", "--host", "local", "--runs", "1"])
        self.assertEqual(2, rc)
        for text in (out, err):
            self.assertIn("flake_triage: REFUSED -- tests come as pairs <binary_id> <name>", text)

    def test_a_store_error_before_the_lease_check_is_a_refusal_not_a_traceback(self):
        """Re-check v2-fix-e M1: `lease_missing` opened the store unguarded --
        a store that cannot be read ended the run as a traceback."""
        with tempfile.TemporaryDirectory() as tmp:
            bad = pathlib.Path(tmp) / "p.db"
            bad.write_bytes(b"no sqlite store " * 64)
            with mock.patch.dict(os.environ, {"MECLAW_PIPELINE_DB": str(bad)}):
                rc, out, err = self._both(["meclaw-cells::misc_1", "t", "--host", "lane-b", "--runs", "1",
                                           "--wave", "welle-x", "--strand", "ft-s"])
        self.assertEqual(2, rc)
        for text in (out, err):
            self.assertRegex(text, r"flake_triage: REFUSED -- pipeline store .*p\.db.*: ")

    def test_strand_test_without_a_lane_refuses_on_both_streams_and_at_the_lease(self):
        db = _held("welle-x/st-a")
        out, err = io.StringIO(), io.StringIO()
        # the holder is read from the cwd's repository: this tree, whatever the runner's cwd
        with mock.patch.object(schedule, "lease_lane", return_value=schedule.LeaseResult(0)), \
                mock.patch.object(gate.os, "getcwd", return_value=str(REPO)):
            rc = gate.run_test(["expr", "--wave", "welle-x", "--strand", "st-a"], out, err)
        self.assertNotEqual(0, rc)
        for stream in (out, err):
            self.assertRegex(stream.getvalue(), r"pipeline test: REFUSED \(exit %d\) -- strand: " % rc)
        line, whole = _holder_line(db, "welle-x/st-a", None)
        self.assertIn("REFUSED", line, whole)

    def test_a_refused_coverage_build_says_so_on_both_streams(self):
        with tempfile.TemporaryDirectory() as tmp:
            hosts = pathlib.Path(tmp) / "lanes"
            hosts.write_text("mm local\n")
            out, err = io.StringIO(), io.StringIO()
            args = types.SimpleNamespace(op="build", lane="mm", strand="welle-x/c",
                                         out_dir=str(pathlib.Path(tmp) / "out"), db=None, scope="full")
            with mock.patch.dict(os.environ, {"MECLAW_LANES_FILE": str(hosts)}), \
                    mock.patch("sys.stdout", out), mock.patch("sys.stderr", err):
                self.assertEqual(5, coverage._cmd_coverage(args))
        for stream in (out, err):
            self.assertIn("coverage build: REFUSED", stream.getvalue())

    def test_a_gate_refused_before_its_pass_says_so_in_the_log(self):
        def refuse(argv, out, err):
            print("gate: queued -- no lane free (waiting for admission)", file=err)
            return 3
        out, err = io.StringIO(), io.StringIO()
        with mock.patch.object(gate, "run_gate", refuse), mock.patch("sys.stdout", out), \
                mock.patch("sys.stderr", err):
            self.assertEqual(3, gate._cmd_gate(types.SimpleNamespace(argv=["strand"])))
        self.assertIn("gate: REFUSED (exit 3) -- gate: queued -- no lane free", out.getvalue())


class LivenessInTokenWho(FreshStore):
    """(3) `token who` shows each holder's last progress, read on demand."""

    def test_who_shows_progress_minutes_and_no_progress_for_a_silent_holder(self):
        db = _held("welle-x/lv-a")
        _held("welle-x/lv-b")
        clock = FakeClock(wall=T0 + 120)
        self.assertTrue(progress.beat("welle-x/lv-a", "12:00:00 pass 1/3 done=1 red=0", now=clock.time()))
        line_a, out = _holder_line(db, "welle-x/lv-a", T0 + 300)
        line_b, _ = _holder_line(db, "welle-x/lv-b", T0 + 300)
        self.assertIn("progress 3 min ago", line_a, out)
        self.assertIn("no progress since take", line_b, out)

    def test_a_beat_of_an_earlier_lease_does_not_count_for_the_new_one(self):
        db = _held("welle-x/lv-c")
        self.assertTrue(progress.beat("welle-x/lv-c", "pass", now=T0 + 60))
        self.assertEqual(0, schedule.lease_release(db, "welle-x/lv-c", now=T0 + 120).code)
        _held("welle-x/lv-c", now=T0 + 180)
        line, out = _holder_line(db, "welle-x/lv-c", T0 + 240)
        self.assertIn("no progress since take", line, out)

    def test_a_gate_pass_counts_its_stations_from_the_journal(self):
        """The gate prints `GATE <station>` per station already (flushed) and
        journals station_ended under its run, the run the lease names."""
        db = _held("welle-x/lv-g", run="run-lv-g")
        store.journal_add(db, schedule._iso(T0 + 240), None, "run-lv-g", "status", "station_ended",
                          "GATE x", {"station": "x", "verdict": "GREEN", "secs": 1, "exit": 0})
        line, out = _holder_line(db, "welle-x/lv-g", T0 + 300)
        self.assertIn("progress 1 min ago", line, out)

    def test_a_series_without_a_pass_shows_no_pass_not_no(self):
        """Re-check v2-fix-e M2 (a): the state is the whole word, not its first half."""
        db = _held("welle-x/lv-n")
        progress.refuse("flake_triage", "no test ran", out=io.StringIO(), err=io.StringIO(),
                        holder="welle-x/lv-n", now=T0 + 60, code=1, word="NO PASS")
        line, out = _holder_line(db, "welle-x/lv-n", T0 + 180)
        self.assertIn("NO PASS 2 min ago: flake_triage: NO PASS -- no test ran", line, out)
        _held("welle-x/lv-m")
        progress.refuse("pipeline test", "strand: no lane", out=io.StringIO(), err=io.StringIO(),
                        holder="welle-x/lv-m", now=T0 + 60, word="REFUSED (exit 3)")
        line, out = _holder_line(db, "welle-x/lv-m", T0 + 180)
        self.assertIn("REFUSED 2 min ago: pipeline test: REFUSED (exit 3) -- strand: no lane", line, out)

    def test_a_batch_holder_notes_its_progress_at_the_member_lease(self):
        """Re-check v2-fix-e M2 (b): `merge --batch` runs as `<wave>/S1+S2` on the
        lease of a member (`schedule._batch_speaker`); its beat lands there."""
        db = _held("welle-x/bt-a")
        self.assertTrue(progress.beat("welle-x/bt-a+bt-b", "GATE fmt 1s GREEN", now=T0 + 60))
        line, out = _holder_line(db, "welle-x/bt-a", T0 + 180)
        self.assertIn("progress 2 min ago", line, out)

    def test_a_batch_holder_with_two_member_leases_notes_at_the_lane_it_runs_on(self):
        """Re-check RC-K3 M4: both members of `<wave>/S1+S2` hold a lane; without
        the lane wish the beat landed at the first member, whatever lane the run
        was on. The caller names its lane, as the merge path does."""
        db = store.open()
        with tempfile.TemporaryDirectory() as tmp:
            lanes_file = pathlib.Path(tmp) / "lanes"
            lanes_file.write_text("north b@north.example.invalid\nsouth b@south.example.invalid\n")
            self.assertEqual(0, schedule.lease_init(db, lanes="north,south", lanes_file=str(lanes_file),
                                                    now=T0).code)
        self.assertEqual("north", schedule.lease_take(db, "welle-x/bt-a", lane="north", now=T0).lane)
        self.assertEqual("south", schedule.lease_take(db, "welle-x/bt-b", lane="south", now=T0).lane)
        self.assertTrue(progress.beat("welle-x/bt-a+bt-b", "GATE fmt 1s GREEN", now=T0 + 60, lane="south"))
        line, out = _holder_line(db, "welle-x/bt-b", T0 + 180)
        self.assertIn("progress 2 min ago", line, out)
        line, out = _holder_line(db, "welle-x/bt-a", T0 + 180)
        self.assertIn("no progress since take", line, out)
        self.assertEqual(3, progress.refuse("pipeline test", "strand: no lane", holder="welle-x/bt-a+bt-b",
                                            db=db, now=T0 + 120, code=3, lane="north"))
        line, out = _holder_line(db, "welle-x/bt-a", T0 + 180)
        self.assertIn("REFUSED 1 min ago", line, out)

    def test_a_beat_without_a_lease_or_a_store_writes_nothing(self):
        self.assertFalse(progress.beat("welle-x/nobody", "pass", now=T0))
        self.assertFalse(progress.beat("", "pass", now=T0))
        with tempfile.TemporaryDirectory() as tmp:
            missing = pathlib.Path(tmp) / "none.db"
            self.assertFalse(progress.beat("welle-x/lv-a", "pass", now=T0, path=missing))
            self.assertFalse(missing.exists())



class LeaseNotFound(FreshStore):
    """V2-A8b (re-check test-cc2, 14.10.): `flake_triage.py --runs 5 ... --strand
    v2-fix-b` from a tree on another branch found no lease, ran 0 passes and
    ended without a word. A lane run without the lease of its holder, and a
    series where no test ran in any pass, end with exit != 0 and one line on
    stderr and in the log that names the holder looked for and the branch."""

    def _run(self, argv, completed=None):
        calls, out, err = [], io.StringIO(), io.StringIO()

        def run(cmd, **k):
            calls.append(cmd)
            return completed or subprocess.CompletedProcess(cmd, 0, "", "")
        with mock.patch.object(ft, "lane_target", return_value="lane@test"), \
                mock.patch.object(ft, "isolated", return_value=True), \
                mock.patch.object(ft.subprocess, "run", run), \
                mock.patch("sys.stdout", out), mock.patch("sys.stderr", err):
            rc = ft.main(argv)
        return rc, out.getvalue(), err.getvalue(), calls

    def _says(self, text, word, holder):
        self.assertRegex(text, r"flake_triage: %s -- .*holder %s.*branch %s"
                         % (word, re.escape(repr(holder)), re.escape(repr(ft.head_branch() or ""))))

    def test_a_lane_run_whose_holder_holds_no_lease_refuses_before_anything_runs(self):
        db = _held("welle-x/other")
        for mode in (["--unittest"], []):
            rc, out, err, calls = self._run(mode + ["--host", "lane-b", "--runs", "5", "--wave", "welle-x",
                                                    "--strand", "ft-b"] + (["m.T.test_x"] if mode else
                                                                           ["meclaw-cells::misc_1", "t"]))
            self.assertEqual(2, rc, mode)
            self.assertEqual([], calls, mode)        # no token check, no push, no run
            for text in (out, err):
                self._says(text, "REFUSED", "welle-x/ft-b")
        line, whole = _holder_line(db, "welle-x/other", None)
        self.assertNotIn("REFUSED", line, whole)     # the lease of another holder is untouched

    def test_a_series_that_ran_no_test_in_any_pass_says_so_with_holder_and_branch(self):
        _held("welle-x/ft-c")
        rc, out, err, _ = self._run(["--host", "lane-b", "--runs", "5", "--wave", "welle-x", "--strand", "ft-c",
                                     "meclaw-cells::misc_1", "t"],
                                    subprocess.CompletedProcess([], 0, "strand: no test ran\n", ""))
        self.assertNotEqual(0, rc)
        for text in (out, err):
            self._says(text, "NO PASS", "welle-x/ft-c")


if __name__ == "__main__":
    unittest.main()
