"""`run_display_scenarios.py --workers K`: stages stay serial, a stage's scenarios
run side by side, and what the runner prints is the serial run's output exactly.

The full scenario file takes over a minute serially, so the order is pinned on a
small synthetic stage set (scenarios that take different times, so a worker pool
finishes them out of order), plus one short real run over a few picked ids.
"""

import contextlib
import importlib.util
import io
import os
import pathlib
import sys
import tempfile
import time
import unittest

REPO = pathlib.Path(__file__).resolve().parents[2]
RUNNER = REPO / "templates" / "display" / "compose" / "scenarios" / "run_display_scenarios.py"

_spec = importlib.util.spec_from_file_location("run_display_scenarios", RUNNER)
rds = importlib.util.module_from_spec(_spec)
# Registered by name, as an import would: a worker's answer is matched to the
# module's own function by name.
sys.modules["run_display_scenarios"] = rds
_spec.loader.exec_module(rds)

from scripts.tests import store_isolation  # noqa: E402


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()


def tearDownModule():  # noqa: N802 -- unittest's name
    store_isolation.restore()

PIDS = None  # a file the synthetic runner appends its pid to


def _scenarios(n):
    return [{"id": "T-%02d" % i, "title": "synthetic %d" % i} for i in range(n)]


def _runner(sc):
    with open(PIDS, "a") as f:
        f.write("%d\n" % os.getpid())
    i = int(sc["id"][2:])
    # Early scenarios are the slow ones: a pool hands them back last.
    time.sleep(0.02 * (12 - i) if i < 12 else 0)
    if i % 5 == 3:
        return None                      # not applicable
    if i % 4 == 1:
        rds.AGE_ONLY.append((sc["id"], ["age"]))
    if i % 7 == 2:
        return ["broken %s" % sc["id"]]
    return []


def _single_case(i):
    def fn():
        time.sleep(0.01 * (4 - i))
        return [] if i != 2 else ["case %d failed" % i]
    return fn


def _run_stages(workers):
    rds.AGE_ONLY.clear()
    out = io.StringIO()
    rc = 0
    with contextlib.redirect_stdout(out):
        scs = _scenarios(16)
        rc |= rds.stage("ONE", scs, set(), {"T-06": "colony only"}, _runner, workers=workers)
        for sid, first in rds.AGE_ONLY:
            print("ONE %s age %s" % (sid, ",".join(first)))
        rc |= rds.stage("TWO", scs, {"T-01", "T-02", "T-04", "T-09"}, {}, _runner,
                        workers=workers)
        rc |= rds.single("THREE", [("case %d" % i, _single_case(i)) for i in range(5)],
                         workers=workers)
    return rc, out.getvalue(), list(rds.AGE_ONLY)


class DisplayScenariosParallelTest(unittest.TestCase):
    def setUp(self):
        global PIDS
        fd, PIDS = tempfile.mkstemp(prefix="rds-pids-")
        os.close(fd)

    def tearDown(self):
        os.unlink(PIDS)
        rds.AGE_ONLY.clear()

    def pids(self):
        with open(PIDS) as f:
            return {int(line) for line in f if line.strip()}

    def test_display_scenarios_stage_order_kept_with_workers(self):
        rc1, serial, age1 = _run_stages(1)
        self.assertEqual(self.pids(), {os.getpid()})
        open(PIDS, "w").close()
        rc4, parallel, age4 = _run_stages(4)
        self.assertEqual(parallel, serial)
        self.assertEqual(rc4, rc1)
        self.assertEqual(age4, age1)
        self.assertEqual(rc1, 1)
        lines = serial.splitlines()
        # Stage order kept, scenario order kept inside each stage.
        self.assertEqual([ln for ln in lines if ln.startswith(("ONE ", "TWO ", "THREE "))
                          and "/" in ln], ["ONE 10/12", "TWO 2/4", "THREE 4/5"])
        self.assertIn("SKIP T-06 synthetic 6 -- colony only: colony only", lines)
        ids = [ln.split()[2] for ln in lines if ln.startswith(("PASS ONE", "FAIL ONE"))]
        self.assertEqual(ids, sorted(ids))
        # The workers really were other processes.
        pids = self.pids()
        self.assertNotIn(os.getpid(), pids)
        self.assertGreater(len(pids), 1)

    def test_real_scenarios_same_output_with_workers(self):
        argv = ["--pure-only", "--snapshot-only"]
        with open(RUNNER.parent / "scenarios.json") as f:
            ids = [sc["id"] for sc in __import__("json").load(f)["scenarios"]][:6]

        def run(extra):
            rds.AGE_ONLY.clear()
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                rc = rds.main(argv + extra + ids)
            return rc, out.getvalue()

        serial = run([])
        self.assertEqual(run(["--workers", "3"]), serial)
        self.assertEqual(run(["--workers=3"]), serial)
        self.assertIn("PURE 6/6", serial[1])


if __name__ == "__main__":
    unittest.main()
