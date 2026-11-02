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


    CONFIG = textwrap.dedent("""\
        [[profile.ci.overrides]]
        filter = 'binary_id(=a::x) & test(=t1)'
        threads-required = 3

        [[profile.default.overrides]]
        filter = \'\'\'
          (binary_id(=a::x) & (test(=t1) + test(=t2)))
          + binary_id(=b)
          + (binary_id(/regex/) & test(=t))
        \'\'\'
        threads-required = 2
        """)

    def test_a_test_that_reserves_slots_weighs_its_slots(self):
        # CI v0.77.1: shard 3 ran 290 s of wall for 217 s of seconds / 4 -- its
        # long-first tests hold two threads each. Slot-seconds met it (284 s).
        with tempfile.TemporaryDirectory() as d:
            cfg = os.path.join(d, "nextest.toml")
            pathlib.Path(cfg).write_text(self.CONFIG)
            junit = os.path.join(d, "junit.xml")
            pathlib.Path(junit).write_text(self.JUNIT % "a::x")
            out = os.path.join(d, "table.json")
            res = subprocess.run(lane.python_argv(TOOL, "durations", "--out", out,
                                                  "--nextest-config", cfg, junit),
                                 cwd=REPO, capture_output=True, text=True,
                                 timeout=kit_time.HANG)
            self.assertEqual(0, res.returncode, res.stderr)
            # t1: ci first (3 slots), t2: default (2), b: the whole binary (2).
            self.assertEqual({"a::x": 7.8, "b": 1.0},  # 1.25 x 3 + 2.0 x 2, at 0.1 s
                             json.loads(pathlib.Path(out).read_text()))

    def test_union_keeps_a_binary_the_run_did_not_name(self):
        with tempfile.TemporaryDirectory() as d:
            junit = os.path.join(d, "junit.xml")
            pathlib.Path(junit).write_text(self.JUNIT % "a::x")
            old = os.path.join(d, "old.json")
            pathlib.Path(old).write_text(json.dumps({"a::x": 99.0, "gone": 7.0}))
            out = os.path.join(d, "table.json")
            res = subprocess.run(lane.python_argv(TOOL, "durations", "--out", out, "--union", old,
                                                  "--nextest-config", os.devnull, junit),
                                 cwd=REPO, capture_output=True, text=True,
                                 timeout=kit_time.HANG)
            self.assertEqual(0, res.returncode, res.stderr)
            self.assertEqual({"a::x": 3.2, "b": 0.5, "gone": 7.0},
                             json.loads(pathlib.Path(out).read_text()))


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


class TestTheTestJobFreesTheDiskFirst(unittest.TestCase):
    """GH #1391: the `test` job clears the hosted runner's preinstalled
    toolchains before anything builds (lock).

    CI of v0.77.2 (run 41421276845): shard 10 died after 3 min 52 s with "No
    space left on device" inside the build of its test step, while nine shards
    building the identical set of test binaries passed -- every shard builds
    the whole workspace's tests, the shard's filterset only narrows the run.
    The margin was too thin for every shard, so the space is made at the
    job's start, before checkout, the cache restore and the build."""

    CLEANUP = "free disk space"
    # What the job itself needs: the Rust toolchain (cargo, rustup) and Node
    # for the actions. A cleanup that names them could delete them.
    KEPT = ("cargo", "rust", "node")

    def setUp(self):
        try:
            import yaml
        except ImportError:  # no PyYAML for this interpreter
            self.skipTest("PyYAML is not importable")
        self.steps = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))["jobs"]["test"]["steps"]

    def _index(self, pred, what):
        hits = [i for i, s in enumerate(self.steps) if pred(s)]
        self.assertTrue(hits, "no step in job test: %s" % what)
        return hits[0]

    def test_the_cleanup_step_runs_before_the_first_cargo_call(self):
        cleanup = self._index(lambda s: s.get("name") == self.CLEANUP, self.CLEANUP)
        self.assertEqual(0, cleanup, "the cleanup is the job's first step")
        for what, pred in (
                ("checkout", lambda s: str(s.get("uses", "")).startswith("actions/checkout")),
                ("rust-cache", lambda s: str(s.get("uses", "")).startswith("Swatinem/rust-cache")),
                ("cargo", lambda s: "cargo" in str(s.get("run", ""))
                 or "test-tier.sh" in str(s.get("run", "")))):
            self.assertLess(cleanup, self._index(pred, what), what)

    def test_the_cleanup_step_names_no_rust_node_or_cargo_directory(self):
        step = self.steps[self._index(lambda s: s.get("name") == self.CLEANUP, self.CLEANUP)]
        text = str(step.get("run", "")).lower()
        self.assertIn("rm -rf", text)
        for kept in self.KEPT:
            self.assertNotIn(kept, text, "the cleanup names %r, which the job needs" % kept)
        # Nothing under the tool cache but CodeQL: Node lives there as well.
        for line in text.splitlines():
            if "hostedtoolcache" in line:
                self.assertIn("hostedtoolcache/codeql", line, line)

    def test_the_free_disk_is_logged_before_and_after_the_cleanup_and_after_the_build(self):
        step = self.steps[self._index(lambda s: s.get("name") == self.CLEANUP, self.CLEANUP)]
        self.assertGreaterEqual(str(step.get("run", "")).count("df -h /"), 2)
        build = self._index(lambda s: "test-tier.sh" in str(s.get("run", "")), "test-tier.sh")
        after = [s for s in self.steps[build + 1:] if "df -h /" in str(s.get("run", ""))]
        self.assertEqual(1, len(after), "one disk line after the build")
        self.assertEqual("always()", after[0].get("if"), "a red build reports its disk as well")


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
        # CI v0.77.1: a job's shard value reached every script a test starts;
        # the tier reads it once and its nextest child (and every test in it)
        # runs without it.
        self.assertIn("=== tier-env: child env MECLAW_TIER_SHARD=<unset>\n", out.stdout)
        env["MECLAW_TIER_SHARD"] = "9/%d" % n if n < 9 else "x"
        out = subprocess.run([str(SCRIPTS / "test-tier.sh"), "t2"], cwd=REPO, env=env,
                             capture_output=True, text=True, timeout=2 * kit_time.HANG)
        self.assertEqual(2, out.returncode, out.stdout + out.stderr)


if __name__ == "__main__":
    unittest.main()
