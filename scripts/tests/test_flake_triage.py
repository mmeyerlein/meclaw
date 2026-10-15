"""Selftest of scripts/flake_triage.py: log parsing, static scan, file mapping, argv."""
import contextlib
import io
import os
import pathlib
import sys
import subprocess
import tempfile
import unittest
from unittest import mock

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO))
sys.path.insert(0, str(REPO / "scripts"))
import flake_triage as ft  # noqa: E402
from scripts.tests import fake_ssh, store_isolation  # noqa: E402


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()


def tearDownModule():  # noqa: N802 -- unittest's name
    store_isolation.restore()

CASE_A = ("meclaw-cells::gh1001_web_write_cost", "gh1001_write_cost_does_not_grow")
CASE_B = ("meclaw-cli::misc", "gh121_root_lease::a_hard_killed_daemon")
# The line `gate.py` `_test` appends to every `strand.sh test` log after the run (GREEN only at rc 0).
RUNNER_1 = "TEST [x] 1/1 1s GREEN\n"
RUNNER_2 = "TEST [x] 2/2 21s GREEN (2/2 iterations green)\n"

LOG = """\
        PASS [   2.000s] (1/2) meclaw-cells::gh1001_web_write_cost gh1001_write_cost_does_not_grow
        PASS [   4.000s] (2/2) meclaw-cli::misc gh121_root_lease::a_hard_killed_daemon
        FAIL [   6.000s] (1/2) meclaw-cells::gh1001_web_write_cost gh1001_write_cost_does_not_grow
thread 'gh1001_write_cost_does_not_grow' (2387898) panicked at crates/meclaw-cells/tests/gh1001_web_write_cost.rs:77:5:
        PASS [   3.000s] (2/2) meclaw-cli::misc gh121_root_lease::a_hard_killed_daemon
        PASS [   4.000s] (1/2) meclaw-cells::gh1001_web_write_cost gh1001_write_cost_does_not_grow
"""

SOURCE = """\
fn a() {
    std::thread::sleep(Duration::from_millis(50));
    let t0 = Instant::now();
    tokio::time::timeout(Duration::from_secs(30), fut).await; // the marker convention
    wait_for(Duration::from_secs(5), || x).await;
    page.waitForSelector('#x', { timeout: 10000 });
    let d = t0.elapsed();
    // sleep(1) in a comment does not count
}
"""


class ParseLog(unittest.TestCase):
    def test_counts_runs_reds_durations_and_assert_lines(self):
        st = ft.parse_log(LOG, [CASE_A, CASE_B])
        self.assertEqual(st[CASE_A]["runs"], 3)
        self.assertEqual(st[CASE_A]["red"], 1)
        self.assertEqual(st[CASE_A]["secs"], [2.0, 6.0, 4.0])
        self.assertEqual(st[CASE_A]["asserts"], {"crates/meclaw-cells/tests/gh1001_web_write_cost.rs:77": 1})
        self.assertEqual((st[CASE_B]["runs"], st[CASE_B]["red"]), (2, 0))

    def test_table_has_one_row_per_test(self):
        st = ft.parse_log(LOG, [CASE_A, CASE_B])
        out = ft.table([CASE_A, CASE_B], st, {CASE_A: "none"})
        rows = out.splitlines()
        self.assertEqual(len(rows), 4)
        self.assertIn("| gh1001_write_cost_does_not_grow | 3 | 1 | 0.33 | 4.0 | 6.0 |", rows[2])


class PositiveSummary(unittest.TestCase):
    def test_a_stress_series_is_positive_only_when_it_ran_every_planned_iteration(self):
        """Re-check kit-fix9 H2: `_positive_summary` demands ran == planned
        (`1/2` is a series cut after its first iteration) beside passed == planned.
        Without the first clause a cut `1/2 ... successful` counted two green
        iterations that never ran."""
        self.assertEqual(2, ft._positive_summary("2/2 stress run iterations successful"))
        self.assertEqual(2, ft._positive_summary("2/2 stress run iterations: 2 passed"))
        for summary in ("1/2 stress run iterations successful",
                        "1/2 stress run iterations: 2 passed",
                        "1/2 stress run iterations: 1 passed"):
            with self.subTest(summary=summary):
                self.assertEqual(0, ft._positive_summary(summary))


class Scan(unittest.TestCase):
    def test_marks_wall_clock_constructs_but_not_the_30s_marker(self):
        found = ft.scan(SOURCE)
        self.assertEqual(found["sleep"], [2])
        self.assertEqual(found["instant"], [3])
        self.assertEqual(found["short-timeout"], [5])
        self.assertEqual(found["js-timeout"], [6])
        self.assertEqual(found["elapsed"], [7])
        self.assertIn("sleep×1 (L2)", ft.render_scan(found))


class FileMapping(unittest.TestCase):
    def test_single_file_and_harness_module(self):
        with tempfile.TemporaryDirectory() as d:
            root = pathlib.Path(d)
            (root / "crates/c/tests/misc").mkdir(parents=True)
            (root / "crates/c/tests/one.rs").write_text("")
            (root / "crates/c/tests/misc/mod_a.rs").write_text("")
            self.assertEqual(ft.test_file("c::one", "t", root), root / "crates/c/tests/one.rs")
            self.assertEqual(ft.test_file("c::misc", "mod_a::t", root), root / "crates/c/tests/misc/mod_a.rs")


class Argv(unittest.TestCase):
    def test_reuses_the_pipeline_isolation_flags_in_one_filterset(self):
        argv = ft.strand_test_argv([CASE_A, CASE_B], "build03", 30)
        self.assertEqual(argv[1:2], ["test"])
        self.assertIn("binary_id(=meclaw-cli::misc) & test(=gh121_root_lease::a_hard_killed_daemon)", argv[2])
        self.assertIn(" | ", argv[2])
        tail = argv[argv.index("--host"):]
        self.assertNotIn("--profile", tail)   # test-tier.sh names the profile
        for flag, value in (("--retries", "0"), ("--stress-count", "30")):
            self.assertEqual(tail[tail.index(flag) + 1], value)

    def test_wave_and_strand_reach_strand_sh_test(self):
        # R-TV-W17: beside `plans/welle-testing-2-vorrat` the branch names no single
        # wave, so `strand.sh test` needs `--wave`/`--strand` from the caller.
        argv = ft.strand_test_argv([CASE_A], "build03", 30, wave="w-x", strand="s-y")
        self.assertEqual(argv[argv.index("--wave") + 1], "w-x")
        self.assertEqual(argv[argv.index("--strand") + 1], "s-y")
        self.assertNotIn("--wave", ft.strand_test_argv([CASE_A], "build03", 30))
        seen = []

        def fake_run(argv, **_kw):
            seen.append(argv)
            return mock.Mock(stdout="", stderr="")

        with mock.patch.object(ft.subprocess, "run", fake_run), \
                mock.patch.object(ft, "start_load", return_value=None), \
                mock.patch.object(ft, "check_load", return_value=""), \
                mock.patch.object(ft, "stop_load"), \
                mock.patch("sys.stdout", io.StringIO()), mock.patch("sys.stderr", io.StringIO()):
            ft.main(list(CASE_A) + ["--wave", "w-x", "--strand", "s-y"])
        self.assertEqual(seen[0][seen[0].index("--wave") + 1], "w-x")
        self.assertEqual(seen[0][seen[0].index("--strand") + 1], "s-y")

    def test_every_result_line_is_printed_once(self):
        tail = ft.strand_test_argv([CASE_A], "build03", 30)
        self.assertEqual(tail[tail.index("--status-level") + 1], "pass")
        self.assertEqual(tail[tail.index("--final-status-level") + 1], "none")

    def test_a_case_without_its_runs_is_a_refusal(self):
        stress = ("        PASS [  10.457s] [ 1/2] (1/1) meclaw-cli::misc gh121_root_lease::a_hard_killed_daemon\n"
                  "        PASS [  10.280s] [ 2/2] (1/1) meclaw-cli::misc gh121_root_lease::a_hard_killed_daemon\n")
        stats = ft.parse_log(stress + "     Summary [  27.423s] 2/2 stress run iterations\n" + stress.splitlines()[0],
                             [CASE_A, CASE_B])
        self.assertEqual(stats[CASE_B]["runs"], 2)
        self.assertEqual(ft.short_runs([CASE_A, CASE_B], stats, 2), [CASE_A])
        with tempfile.NamedTemporaryFile("w", suffix=".log", delete=False) as f:
            f.write(stress + "     Summary [  27.423s] 2/2 stress run iterations: 2 passed\n" + RUNNER_2)
        self.assertEqual(ft.main(["--log", f.name, *CASE_A, *CASE_B]), 1)
        self.assertEqual(ft.main(["--log", f.name, *CASE_B]), 0)
        pathlib.Path(f.name).unlink()

    def test_a_test_that_never_ran_is_red_and_says_not_run(self):
        """Kit find (i) (tv-bn-layout B): an `#[ignore]` test without
        `--run-ignored` ran 0 times; its row read `red 0` and the exit was 3,
        which the kit reads as `waits or held` -- fail-open. 0 runs is RED."""
        with tempfile.NamedTemporaryFile("w", suffix=".log", delete=False) as f:
            f.write("     Summary [   0.001s] 0 tests run: 0 passed, 1 skipped\n")
        out, err = io.StringIO(), io.StringIO()
        with mock.patch("sys.stdout", out), mock.patch("sys.stderr", err):
            rc = ft.main(["--log", f.name, *CASE_A])
        pathlib.Path(f.name).unlink()
        self.assertEqual(1, rc, out.getvalue() + err.getvalue())
        self.assertIn("| gh1001_write_cost_does_not_grow | 0 | 0 | NOT RUN |", out.getvalue())
        self.assertIn("--run-ignored", err.getvalue())

    def test_run_ignored_reaches_strand_sh_test(self):
        """Kit find (i): an `#[ignore]` test meant on purpose runs with `--run-ignored`."""
        argv = ft.strand_test_argv([CASE_A], "build03", 30, ignored=True)
        self.assertEqual("all", argv[argv.index("--run-ignored") + 1])
        self.assertNotIn("--run-ignored", ft.strand_test_argv([CASE_A], "build03", 30))
        seen = []
        with mock.patch.object(ft.subprocess, "run",
                               lambda argv, **_kw: seen.append(argv) or mock.Mock(stdout="", stderr="")), \
                mock.patch.object(ft, "start_load", return_value=None), \
                mock.patch.object(ft, "check_load", return_value=""), \
                mock.patch.object(ft, "stop_load"), \
                mock.patch("sys.stdout", io.StringIO()), mock.patch("sys.stderr", io.StringIO()):
            self.assertEqual(1, ft.main(["--run-ignored", *CASE_A]))
        self.assertEqual("all", seen[0][seen[0].index("--run-ignored") + 1])

    def _stress_main(self, rc, text=None, count="2"):
        """A mocked `strand.sh test`: green stress output (or `text`) in the log
        the run names (fix6: the triage reads only that log), then exit `rc`."""
        green = text if text is not None else ("        PASS [  10.457s] [ 1/2] (1/1) meclaw-cli::misc gh121_root_lease::a_hard_killed_daemon\n"
                 "        PASS [  10.280s] [ 2/2] (1/1) meclaw-cli::misc gh121_root_lease::a_hard_killed_daemon\n"
                 "     Summary [  27.423s] 2/2 stress run iterations successful\n")
        with tempfile.NamedTemporaryFile("w", suffix=".log", delete=False) as f:
            f.write(green)
        self.addCleanup(pathlib.Path(f.name).unlink)
        named = "strand: test log %s\n" % f.name
        out, err = io.StringIO(), io.StringIO()
        with mock.patch.object(ft.subprocess, "run",
                               lambda argv, **_kw: mock.Mock(stdout=named, stderr="", returncode=rc)), \
                mock.patch.object(ft, "start_load", return_value=None), \
                mock.patch.object(ft, "check_load", return_value=""), \
                mock.patch.object(ft, "stop_load"), \
                mock.patch("sys.stdout", out), mock.patch("sys.stderr", err):
            code = ft.main([*CASE_B, "--stress-count", count])
        return code, out.getvalue()

    def test_a_green_stress_run_with_a_nonzero_exit_is_red(self):
        """Re-check fix4 M1: the nextest path never read the run's exit code --
        green output ending in a lane `timeout` (124), a dropped ssh (255), a
        `test-tier.sh` error (1) or exit 3 ended exit 0."""
        for rc in (124, 255, 1, 3, -9, -15):
            with self.subTest(rc=rc):
                code, out = self._stress_main(rc)
                self.assertEqual(1, code, out)
                self.assertIn(f"unexplained: run exit {rc}", out)
                self.assertIn("| a_hard_killed_daemon | 2 | 0 | 0.00 |", out)
        code, out = self._stress_main(0)   # the counter-check: exit 0 stays green
        self.assertEqual(0, code, out)
        self.assertNotIn("unexplained", out)

    PASS2 = ("        PASS [  10.457s] [ 1/2] (1/1) meclaw-cli::misc gh121_root_lease::a_hard_killed_daemon\n"
             "        PASS [  10.280s] [ 2/2] (1/1) meclaw-cli::misc gh121_root_lease::a_hard_killed_daemon\n")

    def _log_main(self, text, *extra, cases=CASE_B):
        with tempfile.NamedTemporaryFile("w", suffix=".log", delete=False) as f:
            f.write(text)
        out, err = io.StringIO(), io.StringIO()
        with mock.patch("sys.stdout", out), mock.patch("sys.stderr", err):
            code = ft.main(["--log", f.name, *cases, *extra])
        pathlib.Path(f.name).unlink()
        return code, out.getvalue() + err.getvalue()

    def test_a_log_whose_summary_is_red_is_red(self):
        """Re-check fix5 M2: `--log` had no exit and never read the summary --
        two PASS lines under `1/2 ... failed` ended exit 0. Green needs a
        positive nextest summary; a red one is `unexplained`, exit 1."""
        for summary in ("2/2 stress run iterations: 1 passed, 1 failed",
                        "1/2 stress run iterations failed",
                        "2 tests run: 1 passed, 1 timed out, 0 skipped",
                        "0/0 stress run iterations: 0 passed"):
            with self.subTest(summary=summary):
                code, out = self._log_main(self.PASS2 + "     Summary [  27.423s] %s\n" % summary)
                self.assertEqual(1, code, out)
                self.assertIn("unexplained: log summary `%s`" % summary, out)

    def test_a_cut_log_without_a_summary_is_red(self):
        """Re-check fix5 M2: a log cut by the lane's `timeout` after one PASS line ended exit 0.
        Re-check fix6 H2: the exact old input of `test_a_case_without_its_runs_is_a_refusal`
        (both PASS lines, no summary) is red too -- complete `[k/N]` marks are no summary."""
        for text in (self.PASS2.splitlines(keepends=True)[0], self.PASS2):
            with self.subTest(text=text):
                code, out = self._log_main(text)
                self.assertEqual(1, code, out)
                self.assertIn("unexplained: log without a nextest summary", out)

    RED_STRESS = "     Summary [  27.423s] 2/2 stress run iterations: 1 passed, 1 failed\n"
    GREEN_STRESS = "     Summary [  27.423s] 2/2 stress run iterations: 2 passed\n"
    GREEN_PLAIN = "     Summary [   1.001s] 1 test run: 1 passed, 0 skipped\n"

    def test_a_red_summary_before_a_green_rerun_is_red(self):
        """Re-check fix6 M2: only the LAST summary counted -- a red stress run
        followed by a green rerun in the same log (probe F1) ended exit 0. A red
        first run with a green rerun is FLAKY and never green (TV-F1)."""
        code, out = self._log_main(self.PASS2 + self.RED_STRESS + self.GREEN_PLAIN)
        self.assertEqual(1, code, out)
        self.assertIn("unexplained: log summary `2/2 stress run iterations: 1 passed, 1 failed`",
                      out)

    def test_the_count_comes_from_the_stress_summary_not_a_plain_rerun(self):
        """Re-check fix6 M2 (probe F4): one PASS under `2/2 ... 2 passed` and then
        `1 test run: 1 passed` took 1 as the count from the last summary."""
        code, out = self._log_main(self.PASS2.splitlines(keepends=True)[0]
                                   + self.GREEN_STRESS + self.GREEN_PLAIN + RUNNER_2)
        self.assertEqual(1, code, out)
        self.assertIn("ran 1 of 2 times -- RED, not run", out)

    def test_a_run_with_a_red_summary_before_a_green_one_is_red(self):
        """Re-check fix6 M2 on the run path (probe F7): rc 0, a red summary, then a green one."""
        code, out = self._stress_main(0, text=self.PASS2 + self.RED_STRESS + self.GREEN_PLAIN)
        self.assertEqual(1, code, out)
        self.assertIn("unexplained: run summary", out)

    def test_several_positive_summaries_stay_green(self):
        # the counter-check: a green stress run and a green plain rerun with all runs is green.
        code, out = self._log_main(self.PASS2 + self.GREEN_STRESS + self.GREEN_PLAIN + RUNNER_2)
        self.assertEqual(0, code, out)
        self.assertNotIn("unexplained", out)

    START = (" Nextest run ID 0000 with nextest profile: default\n"
             "    Starting 1 test across 3 binaries (9 tests skipped)\n")
    LATE = "        PASS [   7.000s] [ 1/2] (1/1) meclaw-cli::misc gh121_root_lease::a_hard_killed_daemon\n"

    def test_a_run_after_the_last_summary_is_red(self):
        """Re-check fix7 M3: a log that carries a second run (`--log` on a log a
        rerun was appended to) was green when that run was cut after its first
        result line -- every summary was positive, the cut run had none. nextest
        starts each run with `Starting`; a run started after the last summary
        has no summary of its own and is red, with or without a result line."""
        for tail in (self.START + self.LATE, self.START):
            with self.subTest(tail=tail):
                code, out = self._log_main(self.PASS2 + self.GREEN_STRESS + tail)
                self.assertEqual(1, code, out)
                self.assertIn("unexplained: log a run after the last summary", out)

    def test_a_cut_run_before_a_complete_one_is_red(self):
        # the same class one run earlier: run 1 cut without its summary, run 2 green.
        code, out = self._log_main(self.START + self.LATE + self.START + self.PASS2
                                   + self.GREEN_STRESS)
        self.assertEqual(1, code, out)
        self.assertIn("unexplained: log a run without its summary", out)

    def test_final_status_lines_after_the_summary_stay_green(self):
        """The counter-check (fix J, measured on 1386 real `test.log` of
        plans/*/receipts): 62 green logs print their result lines after the
        summary (nextest's final status, `--success-output final`), 55 of them
        only there -- a result line after the summary is no second run."""
        code, out = self._log_main(self.START + self.GREEN_STRESS + self.PASS2 + RUNNER_2)
        self.assertEqual(0, code, out)
        self.assertNotIn("unexplained", out)
        # the slow test's reprint of a line already seen before the summary
        code, out = self._log_main(self.START + self.PASS2 + self.GREEN_STRESS
                                   + self.PASS2.splitlines(keepends=True)[0] + RUNNER_2)
        self.assertEqual(0, code, out)

    def test_a_log_is_green_only_with_its_positive_summary_and_all_its_runs(self):
        ok = "     Summary [  27.423s] 2/2 stress run iterations: 2 passed\n"
        self.assertEqual(0, self._log_main(self.PASS2 + ok + RUNNER_2)[0])
        plain = ("        PASS [   1.000s] meclaw-cli::misc gh121_root_lease::a_hard_killed_daemon\n"
                 "     Summary [   1.001s] 1 test run: 1 passed, 3 skipped\n")
        self.assertEqual(0, self._log_main(plain + RUNNER_1)[0])
        # the count comes from the runner line (= the summary): 2 iterations, one PASS line is short
        code, out = self._log_main(self.PASS2.splitlines(keepends=True)[0] + ok + RUNNER_2)
        self.assertEqual(1, code, out)
        self.assertIn("ran 1 of 2 times -- RED, not run", out)

    FIXTURE = REPO / "scripts" / "tests" / "fixtures" / "strand-test-stress-20.log"
    FIXTURE_CASES = ("meclaw-colony", "persist::writer::tests::a_lingering_writer_takes_a_queued_op_at_once",
                     "meclaw-colony::gh_1",
                     "gh1208_settle_answers_at_quiescence::settle_never_ends_the_colony")
    NO_RUNNER = "unexplained: log without the runner's closing `TEST ... GREEN` line"

    def test_a_log_without_the_runners_green_line_is_red(self):
        """Re-check fix8 M1: `--log` never read the line `gate.py` `_test`
        appends to `test.log` after the run -- `TEST [..] p/t Ns GREEN` only at
        rc 0, nothing when the runner never finished. On the real 20/20 log:
        P3a without that line, P3b without it plus a cut follow-up run without
        `Starting`, P4 with `RED` after a positive summary -- all three exit 0.
        Green needs exactly one runner line, the last one, GREEN, its n the
        summary's iterations; a station log has no such line and is red here."""
        real = self.FIXTURE.read_text()
        body, final = real.rstrip("\n").rsplit("\n", 1)
        body += "\n"
        cut = "".join(l + "\n" for l in real.splitlines() if "PASS" in l and "[ 1/20]" in l)
        cases = {
            "P3a no runner line": body,
            "P3b no runner line, cut follow-up": body + cut,
            "P4 runner RED": body + final.split(" GREEN")[0] + " RED\n",
            "runner RED stress": body + final.split(" GREEN")[0] + " RED (1/20 iterations red)\n",
            "runner n != summary": body + final.replace("20/20 iterations", "19/19 iterations") + "\n",
            "two runner lines": body + final + "\n" + final + "\n",
            "runner line before the last summary": body + final + "\n"
            + "     Summary [  23.634s] 20/20 stress run iterations: 20 passed\n",
            "station log, plain": ("        PASS [   1.000s] meclaw-cli::misc gh121_root_lease::a_hard_killed_daemon\n"
                                   "     Summary [   1.001s] 1 test run: 1 passed, 3 skipped\n"),
        }
        for name, text in cases.items():
            with self.subTest(case=name):
                code, out = self._log_main(text, cases=self.FIXTURE_CASES if "station" not in name
                                           else CASE_B)
                self.assertEqual(1, code, out)
                self.assertIn(self.NO_RUNNER, out)
        # the counter-check: the real log as the runner closed it stays green, n = 20
        code, out = self._log_main(real, cases=self.FIXTURE_CASES)
        self.assertEqual(0, code, out)
        self.assertNotIn("unexplained", out)
        self.assertIn("| settle_never_ends_the_colony | 20 | 0 |", out)
        # a runner line from before the stress grading (bare GREEN): the summary's 20 counts
        code, out = self._log_main(body + final.split(" GREEN")[0] + " GREEN\n", cases=self.FIXTURE_CASES)
        self.assertEqual(0, code, out)
        self.assertIn("| settle_never_ends_the_colony | 20 | 0 |", out)

    def test_a_single_stress_iteration_is_green(self):
        """Re-check fix8 H1: nextest says `1/1 stress run iteration: 1 passed`
        (singular, real log tb-s10f test-20261010T182038Z) -- every
        `--stress-count 1` run was red although the runner said GREEN."""
        one = ("        PASS [  10.457s] [1/1] (1/1) meclaw-cli::misc gh121_root_lease::a_hard_killed_daemon\n"
               "     Summary [  27.423s] 1/1 stress run iteration: 1 passed\n")
        self.assertEqual((1, ""), ft.nextest_summary(one))
        code, out = self._log_main(one + "TEST [x] 1/1 30s GREEN (1/1 iterations green)\n")
        self.assertEqual(0, code, out)
        code, out = self._stress_main(0, text=one, count="1")
        self.assertEqual(0, code, out)
        self.assertNotIn("unexplained", out)

    def test_a_summary_line_in_test_output_changes_no_verdict(self):
        """Re-check fix8 H2: SUMMARY/START match every line, test output too
        (`success-output = "immediate-final"` prints stdout unfiltered). A
        test printing a positive `Summary` closed the cut run: green. The
        runner's line comes after the run and decides; without it, or RED, red."""
        fake = self.START + self.LATE + "     Summary [   0.100s] 1 test run: 1 passed\n"
        for tail in ("", "TEST [x] 0/0 3s RED\n"):
            with self.subTest(tail=tail):
                code, out = self._log_main(fake + tail)
                self.assertEqual(1, code, out)
                self.assertIn(self.NO_RUNNER, out)

    def test_a_run_without_a_positive_summary_is_red(self):
        """Re-check fix5 M2 on the run path: rc 0 with a red or missing summary
        (nextest 0.9.146 ends a stress run with an early red iteration at rc 0)."""
        for tail in ("", "     Summary [  27.423s] 2/2 stress run iterations: 1 passed, 1 failed\n"):
            with self.subTest(tail=tail):
                code, out = self._stress_main(0, text=self.PASS2 + tail)
                self.assertEqual(1, code, out)
                self.assertIn("unexplained: run ", out)

    def test_zero_runs_are_a_wrong_call(self):
        """Re-check fix5 M2: `--stress-count 0` asked for nothing and `short_runs(.., 0)`
        named no case -- green with zero runs."""
        code, out = self._stress_main(0, count="0")
        self.assertEqual(2, code, out)

    def test_scan_only_says_it_is_no_verdict(self):
        out = io.StringIO()
        with mock.patch("sys.stdout", out):
            self.assertEqual(0, ft.main(["--scan-only", *CASE_B]))
        self.assertIn("scan only: no run, no verdict", out.getvalue())

    def test_python_scan_only_says_it_is_no_verdict(self):
        """Re-check fix6 H1: `--unittest --scan-only` ended exit 0 without the
        line the nextest path prints -- a scan read as a green verdict."""
        out = io.StringIO()
        with mock.patch("sys.stdout", out):
            self.assertEqual(0, ft.main(["--unittest", "--scan-only",
                                         "scripts.tests.test_flake_triage.T.test_green"]))
        self.assertIn("scan only: no run, no verdict", out.getvalue())

    def test_load_command_ends_at_stdin_eof_and_has_a_cap(self):
        cmd = ft.load_command(4, 600)
        self.assertTrue(cmd.startswith("timeout 600 "))
        self.assertIn("seq 4", cmd)
        self.assertIn("cat >/dev/null", cmd)
        self.assertIn("echo LOAD", cmd)

    def test_an_asked_load_that_does_not_run_is_a_refusal(self):
        """Re-check of GH #1180: without an ssh target, or with a dead ssh,
        the run "under load" is no result -- exit 2 before any test runs."""
        ran = []
        saved = (ft.lane_target, ft.subprocess.run)
        try:
            ft.lane_target = lambda lane, lanes_file=None: None
            ft.subprocess.run = lambda *a, **k: ran.append(a) or saved[1](["false"])
            self.assertEqual(ft.main(["--load", "4", *CASE_A]), 2)
            self.assertEqual(ran, [])
            ft.lane_target = lambda lane, lanes_file=None: "nobody@invalid"
            ft.subprocess.run = lambda argv, **k: saved[1](["sh", "-c", "exit 255"], **k)
            self.assertEqual(ft.main(["--load", "4", *CASE_A]), 2)
        finally:
            ft.lane_target, ft.subprocess.run = saved

    def test_a_loop_missing_at_the_start_or_the_end_is_a_refusal(self):
        """Re-check M1: 2 of 3 loops at the start, or one loop dead at the
        end of the run, is exit 2 -- the run is no result under load."""
        class Proc:
            stdout, stdin, returncode = None, io.StringIO(), None
            poll = wait = terminate = lambda self, *a, **k: None
        saved = (ft.lane_target, ft.lane_probe, ft.alive, ft.subprocess.Popen, ft.subprocess.run)
        ran = []
        try:
            ft.lane_target = lambda lane, lanes_file=None: "lane@test"
            ft.lane_probe = lambda target: (0.0, 8)
            ft.subprocess.run = lambda *a, **k: ran.append(a) or ft.subprocess.CompletedProcess(a, 0, "", "")
            for counts, runs in (([2], 0), ([3, 2], 1)):
                Proc.stdout = io.StringIO("LOAD 11 12 13\n")
                ft.subprocess.Popen = lambda *a, **k: Proc()
                ft.alive = lambda target, pids, c=iter(counts): next(c)
                self.assertEqual(ft.main(["--load", "3", *CASE_A]), 2, counts)
                self.assertEqual(len(ran), runs, counts)
        finally:
            ft.lane_target, ft.lane_probe, ft.alive, ft.subprocess.Popen, ft.subprocess.run = saved

    def _throttle_run(self, delegated, extra=()):
        """main() with `--throttle 80 --runs 5` on a fake lane; returns (rc, stdout, runs, loads)."""
        class Proc:
            stdin, returncode = io.StringIO(), None
            poll = wait = terminate = lambda self, *a, **k: None
        saved = (ft.lane_target, ft.lane_probe, ft.alive, ft.cpu_delegated,
                 ft.subprocess.Popen, ft.subprocess.run)
        runs, loads, out = [], [], io.StringIO()
        try:
            ft.lane_target = lambda lane, lanes_file=None: "lane@test"
            ft.lane_probe = lambda target: (0.0, 8)
            ft.alive = lambda target, pids: len(pids)
            ft.cpu_delegated = lambda target: delegated

            def popen(argv, **k):
                loads.append(argv)
                proc = Proc()
                proc.stdout = io.StringIO("LOAD " + " ".join(str(10 + i) for i in range(8)) + "\n")
                return proc
            ft.subprocess.Popen = popen
            ft.subprocess.run = lambda argv, **k: runs.append((argv, k.get("env"))) or \
                ft.subprocess.CompletedProcess(argv, 0, "", "")
            with mock.patch("sys.stdout", out), mock.patch("sys.stderr", io.StringIO()):
                rc = ft.main(["--throttle", "80", "--runs", "5", *extra, *CASE_A])
        finally:
            (ft.lane_target, ft.lane_probe, ft.alive, ft.cpu_delegated,
             ft.subprocess.Popen, ft.subprocess.run) = saved
        return rc, out.getvalue(), runs, loads

    def test_throttle_runs_under_a_cpu_quota_where_the_lane_delegates(self):
        """TV-F7: `--throttle 80` hands the lane's test run `CPUQuota=80%`
        (`MECLAW_LANE_CPU_QUOTA`, lane_sync.sh wraps test-tier.sh in a
        `systemd-run --user --scope`), with `--runs` iterations and no load."""
        rc, out, runs, loads = self._throttle_run(True)
        (argv, env), = runs
        self.assertEqual(env["MECLAW_LANE_CPU_QUOTA"], "80")
        self.assertEqual(argv[argv.index("--stress-count") + 1], "5")
        self.assertEqual(loads, [])
        self.assertIn("throttle: CPUQuota=80% on lane@test", out)

    def test_a_time_factor_travels_with_the_quota_and_is_named(self):
        """OR-T12-13: `--time-factor 3` hands the quota run
        `MECLAW_TEST_TIME_FACTOR=3` and the header (the receipt's table) names it."""
        rc, out, runs, _ = self._throttle_run(True, ("--time-factor", "3"))
        (argv, env), = runs
        self.assertEqual(env["MECLAW_TEST_TIME_FACTOR"], "3")
        self.assertIn("time factor: MECLAW_TEST_TIME_FACTOR=3 (quota run)", out)

    def test_without_a_time_factor_the_quota_run_has_none(self):
        _, out, runs, _ = self._throttle_run(True)
        (argv, env), = runs
        self.assertNotIn("MECLAW_TEST_TIME_FACTOR", env)
        self.assertNotIn("time factor", out)

    def test_an_invalid_or_quota_less_time_factor_is_refused(self):
        # tv-br M10o (re-check B-M3/H1): above MAX_TIME_FACTOR, and `3_0`
        # (Python's 30, Rust's refusal), are refused like garbage.
        for bad in ("0.5", "fast", "inf", "11", "1e9", "3_0"):
            rc, _, runs, _ = self._throttle_run(True, ("--time-factor", bad))
            self.assertEqual((rc, runs), (2, []), bad)
        with self.assertRaisesRegex(ft.LoadError, "belongs to a --throttle run only"):
            ft.time_factor_env("3", 0)

    def test_a_time_factor_without_a_delegated_quota_is_refused(self):
        """tv-br M10q-a (re-check R K3): a lane without a delegated cpu
        controller runs the fallback load, never a quota -- a time factor there
        would stretch every mark without the quota it belongs to (OR-T12-13).
        LoadError (exit 2) before any load starts or test runs; the header never
        claims a "(quota run)"."""
        rc, out, runs, loads = self._throttle_run(False, ("--time-factor", "3"))
        self.assertEqual((rc, runs, loads), (2, [], []))
        self.assertNotIn("quota run", out)

    def test_the_factor_bound_is_the_one_of_the_rust_marks(self):
        """tv-br M10o (B-M3): the bound is read from `deadline.rs`, not copied."""
        self.assertEqual(ft.max_time_factor(), 10.0)
        self.assertEqual(ft.time_factor_env("10", 1), {"MECLAW_TEST_TIME_FACTOR": "10"})

    def test_throttle_falls_back_to_load_with_a_note(self):
        """A lane without a delegated cpu controller cannot hold a quota: the
        probe runs under `--load nproc` busy loops instead, and says so."""
        rc, out, runs, loads = self._throttle_run(False)
        (argv, env), = runs
        self.assertNotIn("MECLAW_LANE_CPU_QUOTA", env or {})
        self.assertEqual(len(loads), 1)
        self.assertIn("seq 8", loads[0][-1])
        self.assertIn("NOTE throttle: cpu controller not delegated on lane@test -- fallback --load 8", out)
        self.assertIn("load: 8 busy loops on lane@test", out)


class RunLog(unittest.TestCase):
    """Kit find tv-s1-legacy-a2 (Welle Testing, fix6), the class: the run path
    counted what the run's output happened to carry. A red run prints only the
    last 20 lines of its log (run 040123Z: "ran 29 of 60", nextest 60/60 with
    59 passed); a green one prints none, and the fallback took the NEWEST log
    of the strand -- a parallel or earlier run (run 044609Z: 61 runs, 1 red,
    nextest 40/40 green). Kit-Fund tv-s1-legacy-b before it: the newest log of
    ANY strand. The run names its own log (`strand: test log <path>`); the
    triage reads that log and no other, and a run that names none is red."""

    FULL = ("        PASS [  10.457s] [ 1/2] (1/1) meclaw-cli::misc gh121_root_lease::a_hard_killed_daemon\n"
            "        FAIL [  10.280s] [ 2/2] (1/1) meclaw-cli::misc gh121_root_lease::a_hard_killed_daemon\n"
            "     Summary [  27.423s] 2/2 stress run iterations: 1 passed, 1 failed\n")
    GREEN = ("        PASS [  10.457s] [ 1/2] (1/1) meclaw-cli::misc gh121_root_lease::a_hard_killed_daemon\n"
             "        PASS [  10.280s] [ 2/2] (1/1) meclaw-cli::misc gh121_root_lease::a_hard_killed_daemon\n"
             "     Summary [  27.423s] 2/2 stress run iterations: 2 passed\n")

    def setUp(self):
        tmp = tempfile.TemporaryDirectory(prefix="ft-log-")
        self.addCleanup(tmp.cleanup)
        self.root = pathlib.Path(tmp.name)

    def log(self, name, text):
        path = self.root / name / "test.log"
        path.parent.mkdir(parents=True)
        path.write_text(text)
        return path

    def main(self, stdout, rc, other=None, cases=CASE_B, count="2", guess=True):
        """`main` on a mocked `strand.sh test` printing `stdout`; `other` is the
        log the old newest-log guess would have handed it (`guess=False`: no
        stand-in, a guess of the code itself would read the tree)."""
        out, err = io.StringIO(), io.StringIO()
        with contextlib.ExitStack() as stack:
            if guess:
                stack.enter_context(mock.patch.object(ft, "newest_test_log", create=True,
                                                      side_effect=lambda since, **kw: other))
            stack.enter_context(mock.patch.object(
                ft.subprocess, "run",
                lambda argv, **_kw: mock.Mock(stdout=stdout, stderr="", returncode=rc)))
            stack.enter_context(mock.patch.object(ft, "start_load", return_value=None))
            stack.enter_context(mock.patch.object(ft, "check_load", return_value=""))
            stack.enter_context(mock.patch.object(ft, "stop_load"))
            stack.enter_context(mock.patch("sys.stdout", out))
            stack.enter_context(mock.patch("sys.stderr", err))
            code = ft.main([*cases, "--stress-count", count, "--wave", "w-x", "--strand", "s-y"])
        return code, out.getvalue() + err.getvalue()

    def test_a_red_run_is_counted_from_its_log_not_from_the_tail_it_prints(self):
        own = self.log("test-1", self.FULL)
        tail = "".join(self.FULL.splitlines(keepends=True)[1:])   # FAIL + summary: one run
        code, out = self.main("strand: lane l (local) rev aaaa overlay x\nstrand: test log %s\n%s"
                              "TEST [x] 1/2 1s RED\n" % (own, tail), 100)
        self.assertEqual(1, code, out)
        self.assertIn("| a_hard_killed_daemon | 2 | 1 |", out)
        self.assertNotIn("ran 1 of 2", out)

    def test_a_green_run_reads_its_own_log_never_a_newer_one(self):
        own = self.log("test-1", self.GREEN)
        newer = self.log("test-2", self.FULL)   # a parallel run of the strand
        code, out = self.main("strand: test log %s\nTEST [x] 2/2 1s GREEN\n" % own, 0, other=newer)
        self.assertEqual(0, code, out)
        self.assertIn("| a_hard_killed_daemon | 2 | 0 |", out)

    def test_the_own_strands_log_is_read_never_a_newer_one_of_another_strand(self):
        """GH #1209 (kit find legacy-b I): the old fallback `newest_test_log`
        took the newest `plans/*/receipts/*/test-*/test.log` of ANY strand --
        a triage verdict could rest on another strand's run. Two strands with a
        test.log in the tree, the newest is the foreign one; the own is read."""
        receipts = self.root / "plans" / "w-x" / "receipts"
        own = receipts / "s-y" / "test-1" / "test.log"
        foreign = receipts / "s-z" / "test-2" / "test.log"
        # fixed stamps far ahead of any run start, the foreign one the newest
        for path, text, stamp in ((own, self.GREEN, 4102444800), (foreign, self.FULL, 4102444900)):
            path.parent.mkdir(parents=True)
            path.write_text(text)
            os.utime(path, (stamp, stamp))
        with mock.patch.object(ft, "REPO", self.root):
            # no result line in the output: the case the old fallback filled
            code, out = self.main("strand: test log %s\n" % own, 0, guess=False)
        self.assertEqual(0, code, out)
        self.assertIn("| a_hard_killed_daemon | 2 | 0 |", out)
        self.assertNotIn("| a_hard_killed_daemon | 2 | 1 |", out)

    def test_a_run_that_names_no_log_is_red_and_reads_none(self):
        stray = self.log("test-1", self.GREEN)
        code, out = self.main("TEST [x] 2/2 1s GREEN\n", 0, other=stray)
        self.assertEqual(1, code, out)
        self.assertIn("unexplained: run names no test log", out)

    def test_a_second_log_line_in_the_output_is_not_the_runs_log(self):
        """Re-check fix7 H1: `strand.sh test` names its log before any output of
        the lane (`gate.py` `_test`); a test output carrying the same line later
        steered the triage to that log (`found[-1]`). The first line is the run's."""
        own = self.log("test-1", self.GREEN)
        other = self.log("test-2", self.FULL)
        code, out = self.main("strand: test log %s\nstrand: test log %s\nTEST [x] 2/2 1s GREEN\n"
                              % (own, other), 0)
        self.assertEqual(0, code, out)
        self.assertIn("| a_hard_killed_daemon | 2 | 0 |", out)

    REAL = REPO / "scripts" / "tests" / "fixtures" / "strand-test-stress-20.log"

    def test_a_real_strand_test_run_counts_every_iteration_of_its_log(self):
        """P1b (Welle Testing, mini run P1, nachtrag test-high-4): a 20/20 green
        stress run showed "ran 0 of 20" -- `RESULT.search(text)` without `re.M`
        found no line in the output of `strand.sh test`, and the newest-log guess
        handed a foreign log. The fixture is that run's real `test.log` (receipt
        test-20261012T053415Z, 27 tests x 20, `=== ` lines and 25 tests' PASS
        lines cut); the output is `strand.sh test`'s own lines (`gate.py` `_test`).
        Every iteration of both tests counts, line by line, from the named log."""
        own = self.log("test-20261012T053415Z-a989e6d1", self.REAL.read_text())
        foreign = self.log("test-x", self.FULL)
        final = self.REAL.read_text().splitlines()[-1]
        stdout = ("strand: wave w-x\n"
                  "strand: lane l (lane.example.invalid) rev a989e6d1 overlay eba7272d6f3ddc1a\n"
                  "strand: test log %s\n%s\n" % (own, final))
        cases = ["meclaw-colony", "persist::writer::tests::a_lingering_writer_takes_a_queued_op_at_once",
                 "meclaw-colony::gh_1",
                 "gh1208_settle_answers_at_quiescence::settle_never_ends_the_colony"]
        code, out = self.main(stdout, 0, other=foreign, cases=cases, count="20")
        self.assertEqual(0, code, out)
        self.assertIn("| a_lingering_writer_takes_a_queued_op_at_once | 20 | 0 |", out)
        self.assertIn("| settle_never_ends_the_colony | 20 | 0 |", out)
        self.assertNotIn("ran 0 of 20", out)

    def test_a_named_log_that_is_gone_is_red(self):
        gone = self.root / "test-9" / "test.log"
        code, out = self.main("strand: test log %s\nTEST [x] 2/2 1s GREEN\n" % gone, 0)
        self.assertEqual(1, code, out)
        self.assertIn("unexplained: run names no test log (%s is gone)" % gone, out)


class StrandOfBranch(unittest.TestCase):
    """Re-check fix7 M1: `strand_of_branch` names the tree `--unittest --host`
    pushes to its lane when no `--strand` is given (`main_unittest`); its pins
    left with `NewestTestLog`, and the mutation "the wave instead of the strand"
    stayed green. A tree without a strand pushes nothing."""

    def setUp(self):
        tmp = tempfile.TemporaryDirectory(prefix="ft-branch-")
        self.addCleanup(tmp.cleanup)
        self.root = pathlib.Path(tmp.name)

    def tree(self, name, head, linked=True, relative=True):
        """A worktree whose HEAD file says `head` (linked: `.git` is a file with `gitdir:`)."""
        wt = self.root / name / "wt"
        wt.mkdir(parents=True)
        if linked:
            gitdir = self.root / name / "main" / ".git" / "worktrees" / "wt"
            gitdir.mkdir(parents=True)
            (wt / ".git").write_text("gitdir: %s\n" % (
                os.path.relpath(gitdir, wt) if relative else gitdir))
        else:
            gitdir = wt / ".git"
            gitdir.mkdir()
        (gitdir / "HEAD").write_text(head + "\n")
        return wt

    def test_a_linked_worktree_names_the_strand_of_its_branch(self):
        for relative in (True, False):
            with self.subTest(relative=relative):
                wt = self.tree("r%d" % relative, "ref: refs/heads/w-1/mine", relative=relative)
                self.assertEqual("mine", ft.strand_of_branch(wt))

    def test_a_tree_without_a_strand_names_none(self):
        self.assertIsNone(ft.strand_of_branch(self.root))   # no .git at all
        for i, (head, linked) in enumerate((("ref: refs/heads/master", False),
                                            ("ref: refs/heads/master", True),
                                            ("0123456789abcdef0123456789abcdef01234567", True))):
            with self.subTest(head=head, linked=linked):
                self.assertIsNone(ft.strand_of_branch(self.tree("t%d" % i, head, linked=linked)))

    def test_unittest_on_a_lane_from_a_detached_tree_names_the_way(self):
        """GH #1199 (f): a detached tree without `--strand` names the way out
        (exit 2); with `--strand` the strand of the call binds and pushes."""
        wt = self.tree("d", "0123456789abcdef0123456789abcdef01234567")
        for words, code in (((), 2), (("--strand", "s-y"), None)):
            pushed = []
            out, err = io.StringIO(), io.StringIO()
            with self.subTest(words=words), mock.patch.object(ft, "REPO", wt), \
                    mock.patch.object(ft, "lane_target", lambda lane, lanes_file=None: "b@lane.example.invalid"), \
                    mock.patch.object(ft, "bind_lane", lambda *a: pushed.append(("bind",) + a)), \
                    mock.patch.object(ft, "push_tree", lambda *a: pushed.append(("push",) + a) or "/x"), \
                    mock.patch.object(ft, "start_load", side_effect=ft.LoadError("stop here")), \
                    mock.patch("sys.stdout", out), mock.patch("sys.stderr", err):
                rc = ft.main(["--unittest", "--host", "lane-x", "--runs", "1"] + list(words) + ["ft_fixture_mod.T"])
                self.assertEqual(2, rc, out.getvalue() + err.getvalue())
                if code:
                    self.assertIn("detached HEAD: pass --strand <S>", err.getvalue())
                    self.assertEqual([], pushed)
                else:
                    self.assertEqual([("bind", "lane-x", None, "s-y", None), ("push", "lane-x", "s-y")], pushed)

    def test_unittest_on_a_lane_without_a_strand_pushes_nothing(self):
        pushed = []
        out, err = io.StringIO(), io.StringIO()
        with mock.patch.object(ft, "REPO", self.root), \
                mock.patch.object(ft, "lane_target", lambda lane, lanes_file=None: "b@lane.example.invalid"), \
                mock.patch.object(ft, "bind_lane", lambda *a: pushed.append(("bind",) + a)), \
                mock.patch.object(ft, "push_tree", lambda *a: pushed.append(("push",) + a) or "/x"), \
                mock.patch.object(ft, "start_load", side_effect=AssertionError("no run")), \
                mock.patch("sys.stdout", out), mock.patch("sys.stderr", err):
            code = ft.main(["--unittest", "--host", "lane-x", "--runs", "1", "ft_fixture_mod.T"])
        self.assertEqual(2, code, out.getvalue() + err.getvalue())
        self.assertIn("the lane's tree needs a strand name: cannot tell the strand from the branch -- pass --strand", err.getvalue())
        self.assertEqual([], pushed)


FIXTURE = """\
import os
import unittest


def _tick():
    path = os.environ["FT_FIXTURE_COUNT"]
    n = int(open(path).read() or 0) if os.path.exists(path) else 0
    open(path, "w").write(str(n + 1))
    return n


class T(unittest.TestCase):
    def test_green(self):
        pass

    def test_flaky(self):
        self.assertEqual(_tick() % 2, 0, "every second run is red")


class U(unittest.TestCase):
    @unittest.expectedFailure
    def test_unexpected(self):
        os.write(2, b"ok, done\\n")


class V(unittest.TestCase):
    def test_exit_after_ok(self):
        import atexit
        atexit.register(os._exit, 7)


class S(unittest.TestCase):
    @unittest.skip("never runs")
    def test_skipped(self):
        pass


class H(unittest.TestCase):
    def test_hang_after_ok(self):
        import atexit
        import threading
        atexit.register(threading.Event().wait)
"""


class PythonClass(unittest.TestCase):
    """The Python class of the triage (tv-kit D): unittest ids from
    scripts/tests or the kit run `--runs` times like a nextest stress run,
    one row per test, T3 in a random order with the seed in the header."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        root = pathlib.Path(self.tmp.name)
        (root / "ft_fixture_mod.py").write_text(FIXTURE)
        self.root = root
        self.env = mock.patch.dict(os.environ, {"FT_FIXTURE_COUNT": str(root / "count")})
        self.env.start()

    def tearDown(self):
        self.env.stop()
        self.tmp.cleanup()

    def _main(self, *args):
        """A local run starts in the kit's Python (`-I`, no PYTHONPATH) from
        the repo root: the fixture's directory stands in for the repo there
        (M7p); a lane run keeps the real repo, its paths go to the lane."""
        out, err = io.StringIO(), io.StringIO()
        repo = self.root if "local" in args else ft.REPO
        with mock.patch("sys.stdout", out), mock.patch("sys.stderr", err), \
                mock.patch.object(ft, "REPO", repo):
            rc = ft.main(list(args))
        return rc, out.getvalue(), err.getvalue()

    def test_a_python_test_runs_n_times_with_one_row_per_test(self):
        rc, out, err = self._main("--unittest", "--host", "local", "--runs", "4", "--seed", "7", "ft_fixture_mod.T")
        self.assertEqual(rc, 1, out + err)
        self.assertIn("| T.test_flaky | 4 | 2 | 0.50 |", out)
        self.assertIn("| T.test_green | 4 | 0 | 0.00 |", out)
        self.assertRegex(out, r"T\.test_flaky \|[^\n]*ft_fixture_mod\.py:17×2")
        self.assertIn("shuffle: seed 7..10", out)
        self.assertIn("seed 8", err)

    def test_a_python_id_that_runs_nothing_is_a_refusal(self):
        rc, out, err = self._main("--unittest", "--host", "local", "--runs", "2", "--no-shuffle", "ft_fixture_mod.T.test_nope")
        self.assertEqual(rc, 1, out + err)   # kit find (i): not run is RED, never exit 3
        # Re-check fix4 M2: the unknown id's ERROR is red on its own; the pin names the not-run cause.
        self.assertIn("ft_fixture_mod.T.test_nope ran 0 of 2 times -- RED, not run", err)

    def test_a_skipped_python_id_is_red_and_says_not_run(self):
        """Re-check fix4 M2: a `@skip` test runs 0 of n times with exit 0 and no
        red line -- only the not-run branch makes it RED."""
        rc, out, err = self._main("--unittest", "--host", "local", "--runs", "2", "--no-shuffle",
                                  "ft_fixture_mod.S.test_skipped")
        self.assertEqual(rc, 1, out + err)
        self.assertIn("| S.test_skipped | 0 | 0 | NOT RUN |", out)
        self.assertIn("ft_fixture_mod.S.test_skipped ran 0 of 2 times -- RED, not run", err)

    def test_an_exit_after_ok_is_red(self):
        """Re-check fix3 M1: the run's exit code was never read -- `os._exit(7)`
        after unittest's `OK` ended exit 0."""
        rc, out, err = self._main("--unittest", "--host", "local", "--runs", "1", "--no-shuffle",
                                  "ft_fixture_mod.V")
        self.assertEqual(rc, 1, out + err)
        self.assertIn("unexplained: 1 of 1 runs red without a named red test", out)

    def test_a_hang_after_ok_is_red(self):
        """Re-check fix3 M1: a run that hangs after `OK` and is killed at the hang
        marker (-9) ended exit 0."""
        rc, out, err = self._main("--unittest", "--host", "local", "--runs", "1", "--no-shuffle",
                                  "--hang-s", "2", "ft_fixture_mod.H")
        self.assertEqual(rc, 1, out + err)
        self.assertIn("unexplained: 1 of 1 runs red without a named red test", out)

    def test_the_python_throttle_wraps_the_run_in_a_cpu_quota_where_delegated(self):
        runs = []
        done = ft.subprocess.CompletedProcess([], 0, "", "")
        with mock.patch.object(ft, "cpu_delegated", lambda target: True), \
                mock.patch.object(ft, "_run_unittest", lambda argv, hang_s: runs.append(argv) or done):
            rc, out, _ = self._main("--unittest", "--host", "local", "--throttle", "80", "--runs", "2", "--no-shuffle",
                                    "ft_fixture_mod.T.test_green")
        self.assertEqual(len(runs), 2)
        self.assertEqual(runs[0][:5], ["systemd-run", "--user", "--scope", "--quiet", "-p"])
        self.assertEqual(runs[0][5], "CPUQuota=80%")
        self.assertIn("throttle: CPUQuota=80% on local", out)

    def test_the_python_throttle_falls_back_to_local_load_with_a_note(self):
        class Proc:
            stdin, returncode = io.StringIO(), None
            poll = wait = terminate = lambda self, *a, **k: None
        loads, real = [], ft.subprocess.Popen

        def popen(argv, **k):
            if argv[:2] != ["bash", "-c"]:  # LOCAL through the narrow point (M7g)
                return real(argv, **k)
            loads.append(argv)
            proc = Proc()
            proc.stdout = io.StringIO("LOAD 11 12\n")
            return proc
        with mock.patch.object(ft, "cpu_delegated", lambda target: False), \
                mock.patch.object(ft, "lane_probe", lambda target: (0.0, 2)), \
                mock.patch.object(ft, "alive", lambda target, pids: len(pids)), \
                mock.patch.object(ft.subprocess, "Popen", popen):
            rc, out, _ = self._main("--unittest", "--host", "local", "--throttle", "80", "--runs", "2", "--no-shuffle",
                                    "ft_fixture_mod.T.test_green")
        self.assertEqual(rc, 0, out)
        self.assertEqual(loads[0][:2], ["bash", "-c"])
        self.assertIn("seq 2", loads[0][-1])
        self.assertIn("NOTE throttle: cpu controller not delegated on local -- fallback --load 2", out)
        self.assertIn("| T.test_green | 2 | 0 | 0.00 |", out)

    def test_a_run_only_its_summary_calls_red_is_red(self):
        """Re-check fix2 M1: a run whose summary counts a failure the output
        names nowhere ended `red 0`, exit 0 -- fail-open. It counts as an
        unexplained red run and the header says so."""
        text = ("test_green (ft_fixture_mod.T.test_green) ... ok\n\n" + "-" * 70 +
                "\nRan 1 test in 0.000s\n\nFAILED (errors=1)\n")
        done = ft.subprocess.CompletedProcess([], 1, text, "")
        with mock.patch.object(ft, "_run_unittest", lambda argv, hang_s: done):
            rc, out, err = self._main("--unittest", "--host", "local", "--runs", "2", "--no-shuffle",
                                      "ft_fixture_mod.T.test_green")
        self.assertEqual(rc, 1, out + err)
        self.assertIn("unexplained: 2 of 2 runs red without a named red test", out)

    def test_an_unexpected_success_is_a_named_red(self):
        """Re-check fix2 M1, the probe: an `@expectedFailure` test that prints
        `ok, done` before unittest's `unexpected success` ended exit 0."""
        rc, out, err = self._main("--unittest", "--host", "local", "--runs", "2", "--no-shuffle",
                                  "ft_fixture_mod.U")
        self.assertEqual(rc, 1, out + err)
        self.assertIn("| U.test_unexpected | 2 | 2 | 1.00 |", out)

    def test_the_python_class_runs_on_the_lane(self):
        """R-TV-W35 (TV-F7 by its words, `on the lane`): the tree goes to the
        lane's `wt/<strand>` (`pipeline/lane.py push`, `cargo=False`) and every run is one
        ssh call there, the quota inside it; the load runs on the lane too."""
        runs, pushed, binds = [], [], []
        done = ft.subprocess.CompletedProcess(
            [], 0, "test_green (ft_fixture_mod.T.test_green) ... ok\n\nRan 1 test in 0.0s\n\nOK\n", "")
        clean = {k: v for k, v in os.environ.items() if k not in ft.LANE_UNITTEST_ENV}
        with mock.patch.dict(os.environ, clean, clear=True), \
                mock.patch.object(ft, "lane_target", lambda lane, lanes_file=None: "lane@test"), \
                mock.patch.object(ft, "cpu_delegated", lambda target: True), \
                mock.patch.object(ft, "push_tree",
                                  lambda host, strand: pushed.append((host, strand)) or "/srv/l/wt/s-y"), \
                mock.patch.object(ft, "bind_lane", lambda host, wave, strand, holder=None: binds.append((host, wave, strand))), \
                mock.patch.object(ft, "_run_unittest", lambda argv, hang_s: runs.append(argv) or done):
            rc, out, err = self._main("--unittest", "--host", "build09", "--strand", "s-y",
                                      "--throttle", "80", "--runs", "2", "--seed", "5",
                                      "ft_fixture_mod.T.test_green")
        self.assertEqual(rc, 0, out + err)
        self.assertEqual(binds, [("build09", None, "s-y")])
        self.assertEqual(pushed, [("build09", "s-y")])
        self.assertEqual(len(runs), 2)
        # one ssh call through the kit's narrow point (`lane.ssh_argv`, M7g); its
        # program is the absolute path the narrow point checked (M7k A1), here
        # the guard's registered ssh fake, never the bare name
        import re
        from scripts.pipeline import lane
        self.assertEqual(runs[0][:6], [lane.ssh_program(), "-o", "BatchMode=yes", "-o", "ConnectTimeout=10",
                                       "lane@test"])
        self.assertTrue(os.path.isabs(runs[0][0]), runs[0][0])
        # Re-check tv-kit fix15 H5: a name unset here is unset on the lane too --
        # the browser switch aside, which the run sets itself (GH #1209); the
        # narrow point builds both from data (orchestrator M7g).
        self.assertRegex(fake_ssh.remote_script(runs[0][-1]), r"^unset MECLAW_ALLOW_BUMP MECLAW_GATE_MODE MECLAW_TESTING_RULES_RULING; "
                                      r"export MECLAW_REQUIRE_BROWSERS=1; "
                                      r"cd /srv/l/wt/s-y && timeout -k 10 \d+ systemd-run --user --scope "
                                     r"--quiet -p CPUQuota=80% -- " + re.escape(" ".join(lane.PYTHON)) +
                                     r" scripts/pipeline/shuffle.py --seed 5 "
                                     r"ft_fixture_mod\.T\.test_green$")
        self.assertIn("throttle: CPUQuota=80% on lane@test", out)
        self.assertIn("host: build09 (lane@test), tree /srv/l/wt/s-y", out)

    def test_a_lane_run_carries_the_env_that_changes_the_verdict(self):
        """Kit find C4 (Welle Testing): `--unittest --host <lane>` ran without
        MECLAW_TESTING_RULES_RULING, so a test the ruling turns green locally was
        red on the lane (20/20, build03). The names of an explicit list travel,
        nothing else of the environment."""
        runs = []
        done = ft.subprocess.CompletedProcess(
            [], 0, "test_green (ft_fixture_mod.T.test_green) ... ok\n\nRan 1 test in 0.0s\n\nOK\n", "")
        env = {"MECLAW_TESTING_RULES_RULING": "OR-TV-8", "MECLAW_GATE_MODE": "strand",
               "MECLAW_ALLOW_BUMP": "x y", "MECLAW_REQUIRE_BROWSERS": "1",
               "MECLAW_SOME_KEY": "never-sent"}
        with mock.patch.dict(os.environ, env), \
                mock.patch.object(ft, "lane_target", lambda lane, lanes_file=None: "lane@test"), \
                mock.patch.object(ft, "push_tree", lambda host, strand: "/srv/l/wt/s-y"), \
                mock.patch.object(ft, "bind_lane", lambda host, wave, strand, holder=None: None), \
                mock.patch.object(ft, "_run_unittest", lambda argv, hang_s: runs.append(argv) or done):
            rc, out, err = self._main("--unittest", "--host", "build09", "--strand", "s-y",
                                      "--runs", "1", "--no-shuffle", "ft_fixture_mod.T.test_green")
        self.assertEqual(rc, 0, out + err)
        cmd = fake_ssh.remote_script(runs[0][-1])
        self.assertRegex(cmd, r"^export MECLAW_ALLOW_BUMP='x y' MECLAW_GATE_MODE=strand "
                              r"MECLAW_REQUIRE_BROWSERS=1 MECLAW_TESTING_RULES_RULING=OR-TV-8; "
                              r"cd /srv/l/wt/s-y && timeout -k 10 \d+ ")
        self.assertNotIn("MECLAW_SOME_KEY", cmd)
        self.assertNotIn("never-sent", cmd)

    def test_a_lane_run_requires_the_browsers_unless_told_otherwise(self):
        """GH #1209 (re-check tv-bn M7 M4): the TV-F7 tool only handed the
        browser switch on and never set it -- a driver on a lane without its
        engine SKIPped green. Unset, the run sets it; an opt-out is kept and
        named in the header."""
        done = ft.subprocess.CompletedProcess(
            [], 0, "test_green (ft_fixture_mod.T.test_green) ... ok\n\nRan 1 test in 0.0s\n\nOK\n", "")
        clean = {k: v for k, v in os.environ.items() if k not in ft.LANE_UNITTEST_ENV}
        for given, word, note in ((None, "MECLAW_REQUIRE_BROWSERS=1;", False),
                                  ("0", "MECLAW_REQUIRE_BROWSERS=0;", True)):
            runs = []
            env = dict(clean, **({} if given is None else {"MECLAW_REQUIRE_BROWSERS": given}))
            with mock.patch.dict(os.environ, env, clear=True), \
                    mock.patch.object(ft, "lane_target", lambda lane, lanes_file=None: "lane@test"), \
                    mock.patch.object(ft, "push_tree", lambda host, strand: "/srv/l/wt/s-y"), \
                    mock.patch.object(ft, "bind_lane", lambda host, wave, strand, holder=None: None), \
                    mock.patch.object(ft, "_run_unittest", lambda argv, hang_s: runs.append(argv) or done):
                rc, out, err = self._main("--unittest", "--host", "build09", "--strand", "s-y",
                                          "--runs", "1", "--no-shuffle", "ft_fixture_mod.T.test_green")
            with self.subTest(given=given):
                self.assertEqual(rc, 0, out + err)
                self.assertIn("export %s cd " % word, fake_ssh.remote_script(runs[0][-1]))
                # the switch is never among the names the lane run drops
                # (re-check tv-bn M7h C-M1: `runs[0][4]` had become an ssh option)
                self.assertNotRegex(fake_ssh.remote_script(runs[0][-1]), r"unset [^;]*\bMECLAW_REQUIRE_BROWSERS\b")
                self.assertEqual(note, "NOTE: MECLAW_REQUIRE_BROWSERS='0' (opt-out)" in out, out)

    def test_the_lane_list_is_the_one_of_lane_sync(self):
        """GH #1210 (tv-bn M5): one list of the verdict's names for every kit
        way that starts tests on a lane -- here a copy of three names lacked
        MECLAW_REQUIRE_BROWSERS, and a driver SKIP graded green on the lane."""
        from scripts.pipeline import lane as lanes
        self.assertEqual(ft.LANE_UNITTEST_ENV, lanes.verdict_env())
        self.assertIn("MECLAW_REQUIRE_BROWSERS", ft.LANE_UNITTEST_ENV)

    def test_each_failed_subtest_keeps_its_error_text(self):
        """Re-check tv-kit fix15 H4: two subtests of one test failed; the second
        overwrote the first."""
        text = ("=" * 70 + "\nFAIL: test_c (m.T.test_c) (i=1)\n" + "-" * 70 + "\n"
                "Traceback (most recent call last):\n  File \"m.py\", line 5, in test_c\n"
                "    self.assertEqual(i, i + 1)\nAssertionError: 1 != 2\n\n"
                + "=" * 70 + "\nFAIL: test_c (m.T.test_c) (i=2)\n" + "-" * 70 + "\n"
                "Traceback (most recent call last):\n  File \"m.py\", line 5, in test_c\n"
                "    self.assertEqual(i, i + 1)\nAssertionError: 2 != 3\n\n"
                + "-" * 70 + "\nRan 1 test in 0.0s\n\nFAILED (failures=2)\n")
        entry = "\n".join(ft.red_run_entry("run 1/1 red", text, ["m.T.test_c"], 1))
        self.assertIn("AssertionError: 1 != 2", entry)
        self.assertIn("AssertionError: 2 != 3", entry)

    def test_a_red_run_keeps_its_error_text_in_the_result(self):
        """Kit find C4 (Welle Testing): a red run left only the assert's place
        in the table and its last line on stderr; the error text is the finding."""
        res = pathlib.Path(self.tmp.name) / "result.md"
        rc, out, err = self._main("--unittest", "--host", "local", "--runs", "2", "--seed", "7",
                                  "--out", str(res), "ft_fixture_mod.T.test_flaky")
        self.assertEqual(rc, 1, out + err)
        self.assertIn("run 2/2 red, seed 8: ft_fixture_mod.T.test_flaky", out)
        self.assertIn("AssertionError: 1 != 0 : every second run is red", out)
        self.assertIn("AssertionError: 1 != 0 : every second run is red", res.read_text())
        # A run red without a named test keeps its own tail.
        rc, out, err = self._main("--unittest", "--host", "local", "--runs", "1", "--no-shuffle",
                                  "ft_fixture_mod.V")
        self.assertEqual(rc, 1, out + err)
        self.assertIn("run 1/1 red, exit 7 without a named red test", out)

    def test_a_lane_run_with_ok_and_exit_124_is_red(self):
        """Re-check fix3 M1: on the lane `timeout` ends a hung run with 124 after
        its `OK` text -- that ended exit 0."""
        done = ft.subprocess.CompletedProcess(
            [], 124, "test_green (ft_fixture_mod.T.test_green) ... ok\n\nRan 1 test in 0.0s\n\nOK\n", "")
        with mock.patch.object(ft, "lane_target", lambda lane, lanes_file=None: "lane@test"), \
                mock.patch.object(ft, "push_tree", lambda host, strand: "/srv/l/wt/s-y"), \
                mock.patch.object(ft, "bind_lane", lambda host, wave, strand, holder=None: None), \
                mock.patch.object(ft, "_run_unittest", lambda argv, hang_s: done):
            rc, out, err = self._main("--unittest", "--host", "build09", "--strand", "s-y",
                                      "--runs", "2", "--no-shuffle", "ft_fixture_mod.T.test_green")
        self.assertEqual(rc, 1, out + err)
        self.assertIn("unexplained: 2 of 2 runs red without a named red test", out)

    def test_the_python_class_binds_to_a_held_token_like_strand_test(self):
        """`Lanes only bound` (DISPATCH-BUENDEL 4): the lane run asks
        `strand.sh token check --lane` first, as `strand.sh test` does for the
        nextest class; MECLAW_STRAND_TOKEN_SKIP passes there, logged. A refusal
        is exit 2, nothing pushed."""
        calls, pushed = [], []

        def run(argv, **kw):
            calls.append(argv)
            return ft.subprocess.CompletedProcess(argv, 3, "", "no lease for tv-x\n")
        with mock.patch.object(ft, "lane_target", lambda lane, lanes_file=None: "lane@test"), \
                mock.patch.object(ft, "push_tree", lambda host, strand: pushed.append(host) or "/x"), \
                mock.patch.object(ft.subprocess, "run", run):
            rc, out, err = self._main("--unittest", "--host", "build09", "--wave", "w-1", "--strand", "tv-x",
                                      "--runs", "1", "ft_fixture_mod.T.test_green")
        self.assertEqual(rc, 2, out + err)
        self.assertEqual(pushed, [])
        self.assertEqual(calls[0][1:], ["token", "check", "--pid", str(os.getpid()), "--lane", "build09",
                                        "--wave", "w-1", "--strand", "tv-x"])
        self.assertTrue(calls[0][0].endswith("scripts/strand.sh"))
        self.assertIn("no lease for tv-x", err)

    def test_shuffle_takes_several_names_and_replays_its_order(self):
        sys.path.insert(0, str(REPO / "scripts" / "pipeline"))
        import shuffle
        argv = ["--seed", "3", "ft_fixture_mod.T.test_green", "ft_fixture_mod.T.test_flaky"]
        orders = []
        for _ in range(2):
            err = io.StringIO()
            with mock.patch("sys.stderr", err):
                shuffle.main(argv)
            orders.append([l.split(" ")[0] for l in err.getvalue().splitlines() if " ... " in l])
        self.assertEqual(len(orders[0]), 2)
        self.assertEqual(orders[0], orders[1])


class OptOutInTheReport(unittest.TestCase):
    def test_the_rust_triage_names_a_browsers_opt_out(self):
        """Re-check tv-bn M7c M1: only the Python class named the opt-out; the
        Rust/TV-F7 report -- the proof that goes into a strand's report --
        said nothing, and a driver SKIP (no engine) read as a green run. The
        run's own log carries the NOTE and the runner's line; the report names
        it, and the runner's line with the mark still counts green."""
        stress = ("        PASS [  10.457s] [ 1/2] (1/1) meclaw-cli::misc gh121_root_lease::a_hard_killed_daemon\n"
                  "        PASS [  10.280s] [ 2/2] (1/1) meclaw-cli::misc gh121_root_lease::a_hard_killed_daemon\n")
        note = ("NOTE: MECLAW_REQUIRE_BROWSERS='0' (opt-out) -- a browser driver without an engine "
                "SKIPs green, no browser proof\n")
        for given, mark in ((note, "(browsers opt-out: MECLAW_REQUIRE_BROWSERS='0')"), ("", "")):
            with tempfile.NamedTemporaryFile("w", suffix=".log", delete=False) as f:
                f.write(given + stress + "     Summary [  27.423s] 2/2 stress run iterations: 2 passed\n"
                        + RUNNER_2.rstrip("\n") + (" " + mark if mark else "") + "\n")
            self.addCleanup(pathlib.Path(f.name).unlink)
            out, err = io.StringIO(), io.StringIO()
            with mock.patch("sys.stdout", out), mock.patch("sys.stderr", err):
                rc = ft.main(["--log", f.name, *CASE_B])
            with self.subTest(mark=mark):
                self.assertEqual(0, rc, out.getvalue() + err.getvalue())
                self.assertNotIn("unexplained", out.getvalue())
                if mark:
                    self.assertIn(mark, out.getvalue().split("\n\n")[0])
                else:
                    self.assertNotIn("opt-out", out.getvalue())


class HolderNotInTheStrand(unittest.TestCase):
    def test_the_old_strand_colon_form_is_refused_and_the_holder_travels(self):
        """Kit find tv-br M5 (M7d/M7d2): `--strand tv-br:m2` (the second lease's
        name) went whole into the push -- its refspec `refs/heads/lane/tv-br:m2`
        split at the colon and the push failed. The second lease is `--holder`
        (GH #1146): the old form is refused before a lane is asked, and the
        holder reaches the token check and `strand.sh test`."""
        err = io.StringIO()
        with mock.patch.object(ft, "bind_lane", side_effect=AssertionError("asked a lane")), \
                mock.patch("sys.stderr", err), self.assertRaisesRegex(SystemExit, "^2$") as cm:
            ft.main(["bin", "t", "--host", "lane-a", "--strand", "tv-br:m2"])
        self.assertEqual(2, cm.exception.code)
        self.assertIn("--strand tv-br --holder m2", err.getvalue())
        argv = ft.strand_test_argv([("b", "t")], "lane-a", 3, "welle-x", "tv-br", holder="m2")
        self.assertEqual(["--strand", "tv-br", "--holder", "m2"],
                         argv[argv.index("--strand"):argv.index("--strand") + 4])
        seen = []
        with mock.patch.object(ft.subprocess, "run",
                               lambda a, **k: seen.append(a) or subprocess.CompletedProcess(a, 0, "", "")):
            ft.bind_lane("lane-a", "welle-x", "tv-br", holder="m2")
        self.assertEqual(["--strand", "tv-br", "--holder", "m2"], seen[0][-4:])


class ChdirOption(unittest.TestCase):
    """GH #1199 (b): `flake_triage.py -C <dir>` acts as a `cd <dir>` first
    (git -C). The run's `strand.sh test` takes the strand and its lease from
    the cwd's branch; an agent shell that resets its cwd per call needed a
    wrapper script for it (wave validation 2, strand v2-p1078)."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.addCleanup(os.chdir, os.getcwd())

    def run_main(self, *argv):
        seen = []

        def fake_run(a, **kw):
            seen.append((a, kw.get("cwd") or os.getcwd()))
            return subprocess.CompletedProcess(a, 0, RUNNER_1, "")

        err = io.StringIO()
        with mock.patch.object(ft.subprocess, "run", fake_run), \
                mock.patch.object(ft, "start_load", return_value=None), \
                mock.patch.object(ft, "check_load", return_value=""), \
                mock.patch.object(ft, "stop_load"), \
                mock.patch("sys.stdout", io.StringIO()), mock.patch("sys.stderr", err):
            try:
                code = ft.main(list(argv))
            except SystemExit as exc:
                code = exc.code
        return code, seen, err.getvalue()

    def test_the_run_starts_in_the_named_directory(self):
        there = pathlib.Path(self._tmp.name) / "there"
        there.mkdir()
        _code, seen, err = self.run_main("-C", str(there), *CASE_A, "--stress-count", "1")
        self.assertTrue(seen, err)
        self.assertEqual("test", seen[0][0][1])
        self.assertEqual(os.path.realpath(there), os.path.realpath(seen[0][1]))

    def test_a_missing_directory_is_a_clear_exit_2(self):
        gone = pathlib.Path(self._tmp.name) / "gone"
        code, seen, err = self.run_main("-C", str(gone), *CASE_A)
        self.assertEqual(2, code, err)
        self.assertIn("flake_triage: -C: no such directory: %s" % gone, err)
        self.assertEqual([], seen)


if __name__ == "__main__":
    unittest.main()
