"""GH #1571: cpu_floor.py refuses a binary that a signal ends on the floor CPU."""
import os
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from scripts.tests import kit_time, store_isolation

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "cpu_floor.py"

# A stand-in for qemu-x86_64: drops `-cpu <model>` and runs the rest.
QEMU = "#!/bin/sh\nshift 2\nexec \"$@\"\n"
# The 0.89.2 asset on a host without AVX2: killed by SIGILL on its first run.
SIGILL = "#!/usr/bin/env python3\nimport os, signal\nos.kill(os.getpid(), signal.SIGILL)\n"
# A qemu that fails by itself (unknown CPU model, loader error) and never runs the binary.
BROKEN_QEMU = "#!/bin/sh\necho 'qemu: unable to find CPU model' >&2\nexit 1\n"
# A binary that answers: `--apply` of an empty manifest exits 1, which is green.
ANSWERS = "#!/usr/bin/env python3\nimport sys\nsys.exit(1 if '--apply' in sys.argv else 0)\n"


def plant(d, name, text):
    p = Path(d) / name
    p.write_text(text)
    p.chmod(p.stat().st_mode | stat.S_IXUSR)
    return str(p)


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()


def tearDownModule():  # noqa: N802 -- unittest's name
    store_isolation.restore()


class CpuFloor(unittest.TestCase):
    def run_floor(self, body):
        with tempfile.TemporaryDirectory() as d:
            qemu, binary = plant(d, "qemu", QEMU), plant(d, "meclaw", body)
            return subprocess.run([sys.executable, "-I", str(SCRIPT), binary, "--qemu", qemu],
                                  capture_output=True, text=True, timeout=kit_time.HANG)

    def test_a_binary_killed_by_sigill_is_red(self):
        res = self.run_floor(SIGILL)
        self.assertEqual(1, res.returncode, res.stdout + res.stderr)
        self.assertIn("CPU-FLOOR apply cpu=Nehalem rc=-4 (SIGILL) RED", res.stdout)
        self.assertIn("CPU-FLOOR-SUMMARY meclaw RED", res.stdout)

    def test_a_binary_that_answers_is_green_whatever_its_exit_code(self):
        res = self.run_floor(ANSWERS)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertEqual([
            "CPU-FLOOR apply cpu=Nehalem rc=1 GREEN",
            "CPU-FLOOR validate cpu=Nehalem rc=0 GREEN",
            "CPU-FLOOR-SUMMARY meclaw GREEN",
        ], res.stdout.strip().splitlines())

    def test_a_qemu_that_never_runs_the_binary_is_red(self):
        # fail closed: `--validate` of examples/hello has to exit 0 on the floor
        with tempfile.TemporaryDirectory() as d:
            qemu, binary = plant(d, "qemu", BROKEN_QEMU), plant(d, "meclaw", ANSWERS)
            res = subprocess.run([sys.executable, "-I", str(SCRIPT), binary, "--qemu", qemu],
                                 capture_output=True, text=True, timeout=kit_time.HANG)
        self.assertEqual(1, res.returncode, res.stdout + res.stderr)
        self.assertIn("CPU-FLOOR validate cpu=Nehalem rc=1 RED", res.stdout)
        self.assertIn("CPU-FLOOR-SUMMARY meclaw RED", res.stdout)

    def test_without_qemu_it_refuses_instead_of_passing(self):
        with tempfile.TemporaryDirectory() as d:
            binary = plant(d, "meclaw", ANSWERS)
            res = subprocess.run([sys.executable, "-I", str(SCRIPT), binary,
                                  "--qemu", os.path.join(d, "no-such-qemu")],
                                 capture_output=True, text=True, timeout=kit_time.HANG)
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)


if __name__ == "__main__":
    unittest.main()
