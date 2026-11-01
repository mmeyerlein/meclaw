"""`scripts/pipeline/nextest_shards.py`: the public CI's test shards by run time.

Fail-closed: the shard filters together select every binary exactly once --
the binaries the table knows and every binary it does not -- and the matrix of
`.github/workflows/ci.yml` has exactly as many jobs as the table has shards.
"""

import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile
import textwrap
import unittest

from scripts.tests import kit_time, store_isolation

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
REPO = SCRIPTS.parent
sys.path.insert(0, str(SCRIPTS))

from pipeline import lane, nextest_shards  # noqa: E402

TOOL = str(SCRIPTS / "pipeline" / "nextest_shards.py")
WORKFLOW = REPO / ".github" / "workflows" / "ci.yml"


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()


def tearDownModule():  # noqa: N802 -- unittest's name
    store_isolation.restore()


_TERM = re.compile(r"binary_id\(=([^)]+)\)")


def selects(expr, binary):
    """Whether `expr` -- the form `filters` writes: `binary_id(=a) | ... [| not (...)]`
    or `none()` -- selects `binary`. A form it does not know fails the test."""
    if expr == "none()":
        return False
    head, sep, tail = expr.partition(" | not (")
    if sep and head.startswith("not ("):
        raise AssertionError("unexpected form: %s" % expr)
    if expr.startswith("not ("):
        head, tail = "", expr[len("not ("):]
    pos = _TERM.findall(head)
    rest = _TERM.sub("", head).replace("|", "").strip()
    if rest:
        raise AssertionError("unexpected tokens %r in %s" % (rest, expr))
    if binary in pos:
        return True
    if tail:
        if not tail.endswith(")"):
            raise AssertionError("unbalanced: %s" % expr)
        return binary not in _TERM.findall(tail[:-1])
    return False


class TestAssignment(unittest.TestCase):
    TABLE = {"a::x": 100.0, "a::y": 60.0, "b": 50.0, "c::z": 10.0, "d": 5.0}

    def test_every_binary_lands_in_exactly_one_shard_and_an_unknown_one_too(self):
        for n in (1, 2, 3, 8):
            exprs = nextest_shards.filters(self.TABLE, n)
            self.assertEqual(n, len(exprs))
            for binary in list(self.TABLE) + ["new::bin", "meclaw-cli::bin/meclaw"]:
                hits = [k for k, e in enumerate(exprs) if selects(e, binary)]
                self.assertEqual(1, len(hits), (n, binary, exprs))

    def test_longest_first_onto_the_lightest_shard_ties_to_the_lower_one(self):
        self.assertEqual([["a::x"], ["a::y", "d"], ["b", "c::z"]],
                         nextest_shards.assign(self.TABLE, 3))
        # c::z joins b (50 < 60), d the tie of 60 at the lower shard; the unknown
        # binaries go to the smallest sum (b+c::z=60 < a::y+d=65).
        self.assertEqual(2, nextest_shards.unknown_shard(self.TABLE, 3))
        self.assertIn("not (", nextest_shards.filters(self.TABLE, 3)[2])

    def test_a_shard_without_a_binary_selects_nothing(self):
        exprs = nextest_shards.filters({"a": 1.0}, 3)
        self.assertEqual("binary_id(=a)", exprs[0])
        self.assertTrue(exprs[1].startswith("not ("), exprs)
        self.assertEqual("none()", exprs[2])

    def test_a_name_the_filterset_cannot_carry_is_refused(self):
        with self.assertRaisesRegex(ValueError, "cannot stand in a filterset"):
            nextest_shards.filters({"a)b": 1.0}, 2)


class TestTheCheckedInTable(unittest.TestCase):
    def setUp(self):
        self.table = nextest_shards.load(nextest_shards.DURATIONS)

    def test_the_table_is_balanced_within_twenty_percent(self):
        self.assertGreater(len(self.table), nextest_shards.SHARDS)
        sums = [sum(self.table[b] for b in s)
                for s in nextest_shards.assign(self.table, nextest_shards.SHARDS)]
        mean = sum(sums) / len(sums)
        for k, s in enumerate(sums, 1):
            self.assertLessEqual(abs(s - mean), 0.2 * mean, (k, sums))

    def test_the_cli_filter_is_the_function_and_refuses_another_count(self):
        n = nextest_shards.SHARDS
        out = subprocess.run(lane.python_argv(TOOL, "filter", "1/%d" % n), cwd=REPO,
                             capture_output=True, text=True, timeout=kit_time.HANG)
        self.assertEqual(0, out.returncode, out.stderr)
        self.assertEqual(nextest_shards.filters(self.table, n)[0], out.stdout.strip())
        for bad in ("%d/%d" % (n + 1, n), "1/%d" % (n + 1), "0/%d" % n, "x"):
            out = subprocess.run(lane.python_argv(TOOL, "filter", bad), cwd=REPO,
                                 capture_output=True, text=True, timeout=kit_time.HANG)
            self.assertEqual(2, out.returncode, (bad, out.stdout))


class TestDurationsFromJunit(unittest.TestCase):
    JUNIT = textwrap.dedent("""\
        <?xml version="1.0" encoding="UTF-8"?>
        <testsuites name="nextest-run">
          <testsuite name="%s" tests="2">
            <testcase name="t1" time="1.25"/>
            <testcase name="t2" time="2.00"/>
          </testsuite>
          <testsuite name="b" tests="1"><testcase name="t" time="0.5"/></testsuite>
        </testsuites>
        """)

    def test_a_binary_split_over_two_files_sums_up(self):
        with tempfile.TemporaryDirectory() as d:
            paths = []
            for k, name in enumerate(("a::x", "a::x")):
                p = os.path.join(d, "junit-%d.xml" % k)
                pathlib.Path(p).write_text(self.JUNIT % name)
                paths.append(p)
            out = os.path.join(d, "table.json")
            res = subprocess.run(lane.python_argv(TOOL, "durations", "--out", out, *paths),
                                 cwd=REPO, capture_output=True, text=True,
                                 timeout=kit_time.HANG)
            self.assertEqual(0, res.returncode, res.stderr)
            self.assertEqual({"a::x": 6.5, "b": 1.0}, json.loads(pathlib.Path(out).read_text()))


class TestTheWorkflowMatchesTheTable(unittest.TestCase):
    """The matrix lengths of ci.yml are the shard counts of the kit (lock)."""

    def setUp(self):
        try:
            import yaml
        except ImportError:  # no PyYAML for this interpreter
            self.skipTest("PyYAML is not importable")
        self.jobs = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))["jobs"]

    def test_the_test_matrix_has_one_job_per_shard(self):
        job = self.jobs["test"]
        n = nextest_shards.SHARDS
        self.assertEqual(list(range(1, n + 1)), job["strategy"]["matrix"]["shard"])
        steps = [s for s in job["steps"] if "MECLAW_TIER_SHARD" in (s.get("env") or {})]
        self.assertEqual(1, len(steps))
        self.assertEqual("${{ matrix.shard }}/%d" % n, steps[0]["env"]["MECLAW_TIER_SHARD"])
        self.assertIn("scripts/test-tier.sh t2", steps[0]["run"])

    def test_the_selftest_matrix_has_one_job_per_part(self):
        job = self.jobs["selftest"]
        parts = job["strategy"]["matrix"]["part"]
        n = len(parts)
        self.assertGreaterEqual(n, 2)
        self.assertEqual(list(range(1, n + 1)), parts)
        steps = [s for s in job["steps"] if "MECLAW_SELFTEST_PART" in (s.get("env") or {})]
        self.assertEqual(1, len(steps))
        self.assertEqual("${{ matrix.part }}/%d" % n, steps[0]["env"]["MECLAW_SELFTEST_PART"])
        self.assertIn("scripts/gate.sh ci --only gate-selftest", steps[0]["run"])
        # The gate job leaves the station to these jobs, for the same count.
        self.assertEqual(str(n), str(self.jobs["gate"]["env"]["MECLAW_CI_SELFTEST_PARTS"]))


class TestTheTierAndsTheShard(unittest.TestCase):
    def test_t2_with_a_shard_runs_the_shards_filter_and_only_it(self):
        n = nextest_shards.SHARDS
        env = {k: v for k, v in os.environ.items()
               if k not in ("MECLAW_TIER_REUSE", "MECLAW_TIER_LANE", "MECLAW_TIER_EXCLUDE")}
        env.update({"CI": "1", "MECLAW_TIER_DRY": "1", "MECLAW_TIER_SHARD": "2/%d" % n})
        out = subprocess.run([str(SCRIPTS / "test-tier.sh"), "t2"], cwd=REPO, env=env,
                             capture_output=True, text=True, timeout=2 * kit_time.HANG)
        self.assertEqual(0, out.returncode, out.stdout + out.stderr)
        want = "(all()) & (%s)" % nextest_shards.filters(
            nextest_shards.load(nextest_shards.DURATIONS), n)[1]
        self.assertIn("=== filter: %s\n" % want, out.stdout)
        env["MECLAW_TIER_SHARD"] = "9/%d" % n if n < 9 else "x"
        out = subprocess.run([str(SCRIPTS / "test-tier.sh"), "t2"], cwd=REPO, env=env,
                             capture_output=True, text=True, timeout=2 * kit_time.HANG)
        self.assertEqual(2, out.returncode, out.stdout + out.stderr)


if __name__ == "__main__":
    unittest.main()
