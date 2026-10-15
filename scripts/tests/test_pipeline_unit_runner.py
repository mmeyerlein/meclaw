"""Tests of the unit runner (`scripts/pipeline/unit_runner.py`, W10, GH #1124).

Every case writes a small tree of `unittest` files into a temporary
directory and runs the runner as a subprocess -- the way a satellite station
calls it (`{python} {pipeline}/unit_runner.py --root {root} ...`).
"""

import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

from scripts.tests import kit_time, store_isolation

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS))

from pipeline import events  # noqa: E402

RUNNER = str(SCRIPTS / "pipeline" / "unit_runner.py")


# gate-selftest loads every module into one process: no fixture event of this
# module may reach the metrics target of the caller's environment.
_METRICS_URL = mock.patch.dict(os.environ, {"MECLAW_PIPELINE_METRICS_URL": ""})


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()
    _METRICS_URL.start()


def tearDownModule():  # noqa: N802 -- unittest's name
    _METRICS_URL.stop()
    store_isolation.restore()

CASE = '''import os, time, unittest


class T(unittest.TestCase):
{body}
'''


def write(root, rel, text):
    path = os.path.join(root, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(text)


def case(root, rel, *methods):
    body = "".join("    def test_%d(self):\n        %s\n" % (i, m) for i, m in enumerate(methods))
    write(root, rel, CASE.format(body=body))


class Runner(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = self._tmp.name

    def tearDown(self):
        self._tmp.cleanup()

    def run_runner(self, *args, env=None, timeout=4 * kit_time.HANG):
        full = dict(os.environ)
        full.update(env or {})
        try:
            return subprocess.run([sys.executable, RUNNER, "--root", self.root] + list(args),
                                  capture_output=True, text=True, env=full, timeout=timeout)
        except subprocess.TimeoutExpired as exc:
            raise kit_time.Hang("HANG: unit_runner %s after %.0f s (last: %r)"
                                % (" ".join(args), timeout, exc.stdout)) from None


def strip_secs(lines):
    return [re.sub(r" \d+(\.\d+)?s ", " Ns ", l) for l in lines]


class Parallel(Runner):
    def test_unit_runner_parallel_results_deterministic(self):
        # the first file is the slowest: finishing order differs from the sorted order
        case(self.root, "tests/test_a.py", "time.sleep(0.6)", "pass")
        case(self.root, "tests/test_b.py", "time.sleep(0.2)")
        case(self.root, "tests/test_c.py", "self.assertEqual(1, 2)", "pass")
        case(self.root, "tests/test_d.py", "pass")
        case(self.root, "tests/lab/test_e.py", "pass")
        write(self.root, "tests/__init__.py", "")
        write(self.root, "tests/lab/__init__.py", "")
        outs = []
        for _ in range(2):
            res = self.run_runner("--by", "file", "--workers", "3",
                                  "--include", "tests/test_*.py", "--exclude", "tests/lab/**")
            self.assertEqual(1, res.returncode, res.stdout + res.stderr)
            outs.append(strip_secs(res.stdout.strip().splitlines()))
        self.assertEqual(outs[0], outs[1])
        self.assertEqual([
            "UNIT tests/test_a.py Ns GREEN tests=2 fail=0",
            "UNIT tests/test_b.py Ns GREEN tests=1 fail=0",
            "UNIT tests/test_c.py Ns RED tests=2 fail=1",
            "UNIT tests/test_d.py Ns GREEN tests=1 fail=0",
            "UNIT-SUMMARY 3/4 Ns RED",
        ], [l.split(" -- ")[0] for l in outs[0]])
        # by dir: one discover run per directory
        res = self.run_runner("--by", "dir", "--workers", "2", "--include", "tests/**/test_*.py",
                              "--include", "tests/test_*.py")
        lines = strip_secs(res.stdout.strip().splitlines())
        self.assertEqual(["UNIT tests Ns RED tests=6 fail=1", "UNIT tests/lab Ns GREEN tests=1 fail=0",
                          "UNIT-SUMMARY 1/2 Ns RED"], [l.split(" -- ")[0] for l in lines])

    def test_unit_runner_workers_use_distinct_slots(self):
        record = ("open(os.path.join(os.environ['OUT_DIR'], __name__), 'w').write("
                  "'%s %s %s %s %f' % (os.environ['MECLAW_SLOT'], os.environ['MECLAW_WORKER'], "
                  "os.environ['SOUL2ME_LAB_SLOT'], time.time(), 0)); time.sleep(0.4)")
        for n in range(6):
            case(self.root, "tests/test_%d.py" % n, record)
        out = os.path.join(self.root, "out")
        os.makedirs(out)
        res = self.run_runner("--by", "file", "--workers", "3", "--include", "tests/test_*.py",
                              "--slots", "7930,7931,7932", "--slot-env", "SOUL2ME_LAB_SLOT",
                              env={"OUT_DIR": out})
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        seen = {}
        for name in os.listdir(out):
            slot, worker, named, _at, _ = pathlib.Path(out, name).read_text().split()
            self.assertEqual(slot, named)
            seen.setdefault(worker, set()).add(slot)
        # worker i holds slots[i] and nothing else; three workers ran
        self.assertEqual({"0": {"7930"}, "1": {"7931"}, "2": {"7932"}}, seen)
        # fewer slots than workers is a usage error, never a shared slot
        res = self.run_runner("--by", "file", "--workers", "3", "--include", "tests/test_*.py",
                              "--slots", "7930,7931", env={"OUT_DIR": out})
        self.assertEqual(2, res.returncode)
        self.assertIn("slots", res.stderr)


    def test_unit_runner_names_itself_to_each_unit(self):
        """GH #1147 (KF4): a satellite's negative test asks THIS runner for its unit list
        (`--list`); on a lane the runner lives in the kit, so it names its own path."""
        case(self.root, "tests/test_0.py",
             "open(os.environ['OUT_FILE'], 'w').write(os.environ['MECLAW_UNIT_RUNNER'])")
        out = os.path.join(self.root, "seen")
        res = self.run_runner("--by", "file", "--workers", "1", "--include", "tests/test_*.py",
                              env={"OUT_FILE": out, "MECLAW_UNIT_RUNNER": "/elsewhere/unit_runner.py"})
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertEqual(os.path.realpath(RUNNER), pathlib.Path(out).read_text())


class Jsonl(Runner):
    def test_unit_runner_emits_test_result_per_file(self):
        case(self.root, "tests/test_a.py", "pass")
        case(self.root, "tests/test_b.py", "self.fail('no')")
        case(self.root, "tests/test_c.py", "pass")
        path = os.path.join(self.root, "results.jsonl")
        res = self.run_runner("--by", "file", "--workers", "2", "--include", "tests/test_*.py",
                              "--jsonl", path, "--run-id", "run-1", "--station", "unit")
        self.assertEqual(1, res.returncode)
        with open(path, encoding="utf-8") as fh:
            rows = list(events.read_jsonl(fh))
        self.assertEqual(["tests/test_a.py", "tests/test_b.py", "tests/test_c.py"],
                         [r["data"]["part"] for r in rows])
        for row in rows:
            self.assertEqual("test_result", row["event"])
            self.assertEqual("note", row["kind"])
            self.assertEqual("run-1", row["run"])
            events.validate(row["event"], row["kind"], row["data"])
            self.assertEqual("unit", row["data"]["station"])
            self.assertIsInstance(row["data"]["secs"], float)
            self.assertTrue(row["at"].endswith("Z"))
        self.assertEqual(["GREEN", "RED", "GREEN"], [r["data"]["verdict"] for r in rows])
        self.assertEqual(1, rows[1]["data"]["fail"])

    def test_unit_runner_keeps_the_full_log_of_every_unit_beside_the_jsonl(self):
        # W10 kit finding K2: the stream gets the cut, the lane keeps the whole
        # output of each unit under `<jsonl>.d/<unit>.log`.
        case(self.root, "tests/test_a.py", "pass")
        case(self.root, "tests/test_b.py", "self.fail('the reason')")
        path = os.path.join(self.root, "results.jsonl")
        res = self.run_runner("--by", "file", "--workers", "1", "--include", "tests/test_*.py",
                              "--jsonl", path, "--run-id", "run-1", "--station", "unit")
        self.assertEqual(1, res.returncode)
        logs = sorted(os.listdir(path + ".d"))
        self.assertEqual(["tests_test_a.py.log", "tests_test_b.py.log"], logs)
        with open(os.path.join(path + ".d", "tests_test_b.py.log"), encoding="utf-8") as fh:
            self.assertIn("the reason", fh.read())

    def test_unit_runner_timeout_is_red_with_reason(self):
        # The kill is the event: a runner that misses it holds past the marker.
        case(self.root, "tests/test_slow.py", "time.sleep(600)")
        case(self.root, "tests/test_fast.py", "pass")
        res = self.run_runner("--by", "file", "--workers", "2", "--include", "tests/test_*.py",
                              "--timeout-s", "1", timeout=kit_time.HANG)
        self.assertEqual(1, res.returncode)
        lines = res.stdout.strip().splitlines()
        self.assertTrue(lines[0].startswith("UNIT tests/test_fast.py "), lines)
        self.assertIn(" RED ", lines[1])
        self.assertIn("timeout after 1s", lines[1])

    def test_unit_runner_without_units_is_red(self):
        res = self.run_runner("--by", "file", "--workers", "2", "--include", "tests/test_*.py")
        self.assertEqual(1, res.returncode)
        self.assertIn("UNIT-SUMMARY 0/0", res.stdout)

    def test_unit_runner_zero_tests_overall_is_red(self):
        # Re-check W10 I2 (GH #997): a run where no test ran must not report
        # GREEN -- each test-less module exits 5 and stays a GREEN unit, but the
        # run as a whole is RED with a loud reason.
        write(self.root, "tests/test_empty_a.py", "import unittest\n")
        write(self.root, "tests/test_empty_b.py", "import unittest\n")
        res = self.run_runner("--by", "file", "--workers", "2", "--include", "tests/test_*.py")
        self.assertEqual(1, res.returncode, res.stdout + res.stderr)
        self.assertEqual([
            "UNIT tests/test_empty_a.py Ns GREEN tests=0 fail=0",
            "UNIT tests/test_empty_b.py Ns GREEN tests=0 fail=0",
            "UNIT-SUMMARY 2/2 Ns RED -- no test ran in 2 units",
        ], strip_secs(res.stdout.strip().splitlines()))
        # one real test makes the run GREEN; the test-less unit stays GREEN
        case(self.root, "tests/test_real.py", "pass")
        res = self.run_runner("--by", "file", "--workers", "2", "--include", "tests/test_*.py")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("UNIT-SUMMARY 3/3", res.stdout)
        # a script's exit code is its verdict: script mode counts no tests
        res = self.run_runner("--by", "file", "--run", "script", "--include", "tests/test_empty_*.py")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)


class Satellites(Runner):
    """The forms the satellites' registers use (meclaw#1124)."""

    def test_unit_runner_script_mode_runs_each_file(self):
        # soul2me: each file is `<python> <file>`, its exit code is the verdict,
        # the tree is on PYTHONPATH (`from tests.support... import`)
        write(self.root, "tests/__init__.py", "")
        write(self.root, "tests/support/__init__.py", "")
        write(self.root, "tests/support/helper.py", "VALUE = 7\n")
        write(self.root, "tests/a/test_script.py",
              "from tests.support.helper import VALUE\nassert VALUE == 7\nprint('ok')\n")
        write(self.root, "tests/a/test_exit.py", "import sys\nprint('bad')\nsys.exit(3)\n")
        write(self.root, "tests/a/test_main.py", CASE.format(
            body="    def test_0(self):\n        pass\n") + "\nunittest.main()\n")
        res = self.run_runner("--by", "file", "--run", "script", "--workers", "2",
                              "--include", "tests/*/test_*.py", "--exclude", "tests/support/**")
        self.assertEqual(1, res.returncode, res.stdout + res.stderr)
        self.assertEqual([
            "UNIT tests/a/test_exit.py Ns RED tests=0 fail=1 -- bad",
            "UNIT tests/a/test_main.py Ns GREEN tests=1 fail=0",
            "UNIT tests/a/test_script.py Ns GREEN tests=0 fail=0",
            "UNIT-SUMMARY 2/3 Ns RED",
        ], strip_secs(res.stdout.strip().splitlines()))
        # a script is a file: by=dir has no script form
        res = self.run_runner("--by", "dir", "--run", "script", "--include", "tests/*/test_*.py")
        self.assertEqual(2, res.returncode)
        self.assertIn("--run script", res.stderr)

    def test_unit_runner_top_unit_discovers_each_dir_alone(self):
        # soul2me's lab dirs: no __init__.py, neighbour imports (`import _setup`)
        write(self.root, "tests/lab_x/_setup.py", "X = 1\n")
        case(self.root, "tests/lab_x/test_a.py", "import _setup; self.assertEqual(1, _setup.X)", "pass")
        write(self.root, "tests/lab_y/_setup.py", "X = 2\n")
        case(self.root, "tests/lab_y/test_a.py", "import _setup; self.assertEqual(2, _setup.X)")
        case(self.root, "tests/lab_ops/test_c.py", "self.fail('excluded')")
        case(self.root, "tests/test_top.py", "self.fail('not a lab dir')")
        res = self.run_runner("--by", "dir", "--top", "unit", "--workers", "2",
                              "--include", "tests/lab_*", "--exclude", "tests/lab_ops")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertEqual([
            "UNIT tests/lab_x Ns GREEN tests=2 fail=0",
            "UNIT tests/lab_y Ns GREEN tests=1 fail=0",
            "UNIT-SUMMARY 2/2 Ns GREEN",
        ], strip_secs(res.stdout.strip().splitlines()))

    def test_unit_runner_missing_interpreter_is_red_not_a_crash(self):
        # soul2me's unit-portal names a venv a worktree may not have
        case(self.root, "tests/test_a.py", "pass")
        res = self.run_runner("--by", "file", "--include", "tests/test_*.py",
                              "--python", os.path.join(self.root, "no-venv", "bin", "python"))
        self.assertEqual(1, res.returncode, res.stderr)
        lines = res.stdout.strip().splitlines()
        self.assertTrue(lines[0].startswith("UNIT tests/test_a.py "), lines)
        self.assertIn(" RED ", lines[0])
        self.assertIn("cannot start", lines[0])
        self.assertNotIn("Traceback", res.stderr)

    def test_unit_runner_list_prints_units_and_runs_nothing(self):
        marker = "open(os.path.join(os.environ['OUT_DIR'], __name__), 'w').write('ran')"
        for rel in ("tests/a/test_1.py", "tests/a/test_2.py", "tests/b/test_3.py",
                    "tests/lab/test_4.py", "tests/b/fixtures/test_5.py"):
            case(self.root, rel, marker)
        out = os.path.join(self.root, "out")
        os.makedirs(out)
        res = self.run_runner("--by", "file", "--workers", "4", "--include", "tests/*/test_*.py",
                              "--exclude", "tests/lab/**", "--exclude", "tests/*/fixtures/**",
                              "--list", env={"OUT_DIR": out})
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertEqual(["UNIT tests/a/test_1.py", "UNIT tests/a/test_2.py",
                          "UNIT tests/b/test_3.py", "UNIT-LIST 3"],
                         res.stdout.strip().splitlines())
        self.assertEqual([], os.listdir(out))
        res = self.run_runner("--by", "dir", "--top", "unit", "--include", "tests/*",
                              "--exclude", "tests/lab", "--list")
        self.assertEqual(["UNIT tests/a", "UNIT tests/b", "UNIT-LIST 2"],
                         res.stdout.strip().splitlines())
        res = self.run_runner("--by", "file", "--include", "nothing/test_*.py", "--list")
        self.assertEqual(1, res.returncode)
        self.assertIn("UNIT-LIST 0", res.stdout)


from pipeline import unit_runner  # noqa: E402 -- the module under test, in-process


class TestTailKeepsFailures(unittest.TestCase):
    """W10 kit finding K2: the cut of a RED unit's output lost the FAIL and
    `not ok` lines above the last lines; they stay, the rest is the tail."""

    def test_failure_lines_above_the_tail_survive(self):
        lines = ["FAIL: test_a (m.T)", "not ok 3 - b"] + ["noise %d" % i for i in range(100)]
        kept = unit_runner.keep_failures(lines, 5)
        self.assertEqual(["FAIL: test_a (m.T)", "not ok 3 - b"] + lines[-5:], kept)

    def test_the_failure_head_is_capped(self):
        # Review Zb R2 M4: 5000 `not ok` lines went into the stream whole.
        lines = ["not ok %d" % i for i in range(5000)] + ["tail"]
        kept = unit_runner.keep_failures(lines, 1)
        self.assertEqual(unit_runner.FAILURE_HEAD_MAX + 2, len(kept))
        self.assertIn("%d more failure lines" % (5000 - unit_runner.FAILURE_HEAD_MAX), kept[-2])
        self.assertEqual("tail", kept[-1])

    def test_unit_logs_are_capped_and_never_collide(self):
        # Review Zb R2 M4: no size cap; `a/b` and `a_b` wrote one file.
        with tempfile.TemporaryDirectory() as tmp:
            big = "x" * (unit_runner.UNIT_LOG_MAX_B + 100) + "END"
            unit_runner._write_unit_logs(tmp, [{"unit": "a/b", "output": big},
                                               {"unit": "a_b", "output": "two"}])
            names = sorted(os.listdir(tmp))
            self.assertEqual(2, len(names), names)
            with open(os.path.join(tmp, "a_b.log"), encoding="utf-8") as fh:
                first = fh.read()
            self.assertLessEqual(len(first), unit_runner.UNIT_LOG_MAX_B + 200)
            self.assertTrue(first.endswith("END"))

    def test_a_short_output_is_kept_whole(self):
        lines = ["a", "FAIL: x", "b"]
        self.assertEqual(lines, unit_runner.keep_failures(lines, 30))


if __name__ == "__main__":
    unittest.main()
