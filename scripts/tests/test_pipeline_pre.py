"""Tests of `pre`, the pre-run before a strand gate (`scripts/pipeline/pre.py`,
the `pre` subcommand and the stamp check of `scripts/strand.sh gate`; W4,
GH #1118).

The gate cases run the real `strand.sh` in a throw-away repository with a
linked strand worktree and a fake `gate.sh` that only prints a summary line
and writes a receipt -- nothing builds, nothing reaches a lane.
"""

import contextlib
import datetime
import io
import json
import os
import pathlib
import re
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock
from scripts.tests import fake_ssh, kit_time, kit_tree, store_isolation

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS))

from pipeline import cli, events, flakes, gitenv, lane, pre, receipt, store  # noqa: E402

# argv index of the script in a kit Python start (`lane.PYTHON` + script)
SCRIPT_AT = len(lane.PYTHON)

LINE = re.compile(r"^PRE (?P<check>[a-z0-9-]+) (?P<secs>\d+)s (?P<verdict>GREEN|RED|NOTE)( .*)?$")
SUMMARY = re.compile(r"^PRE-SUMMARY [0-9a-f]{8} \d+/\d+ \d+s (GREEN|RED)$")


# gate-selftest loads every module into one process: no fixture event of this
# module may reach the metrics target of the caller's environment.
_METRICS_URL = mock.patch.dict(os.environ, {"MECLAW_PIPELINE_METRICS_URL": ""})


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()
    _METRICS_URL.start()


def tearDownModule():  # noqa: N802 -- unittest's name
    _METRICS_URL.stop()
    store_isolation.restore()

FAKE_GATE = """#!/usr/bin/env bash
mode="$1"
if [ -n "${MECLAW_GATE_ARCHIVE:-}" ]; then
    mkdir -p "$MECLAW_GATE_ARCHIVE"
    printf '{"mode": "%s", "owner_decisions": []}\\n' "$mode" >"$MECLAW_GATE_ARCHIVE/receipt.json"
fi
echo "GATE-SUMMARY $mode 0000000 1/1 0s GREEN"
"""


def write(root, rel, text, mode=None):
    path = os.path.join(root, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(text)
    if mode:
        os.chmod(path, mode)


def git(root, *args):
    # Never the caller's repository: init_repo's `git init`/`config user.name t`
    # went to an inherited GIT_DIR's shared config (pd-1155).
    return subprocess.run(["git", "-C", root] + list(args), check=True,
                          capture_output=True, text=True,
                          env=gitenv.without_repo_env(os.environ)).stdout.strip()


def init_repo(root):
    git(root, "init", "-q", "-b", "master")
    git(root, "config", "user.email", "t@example.invalid")
    git(root, "config", "user.name", "t")
    git(root, "add", "-A")
    git(root, "commit", "-q", "-m", "base")


def pid_ended(pid):
    """True when `pid` is gone or a zombie (`/proc/<pid>/stat` state Z or X): a
    zombie still answers `os.kill(pid, 0)` until its parent reaps it."""
    try:
        with open("/proc/%d/stat" % pid) as fh:
            stat = fh.read()
    except FileNotFoundError:
        return True
    except ProcessLookupError:
        return True
    return stat.rsplit(")", 1)[-1].split()[0] in ("Z", "X")


class Fixture(unittest.TestCase):
    """A mini tree: one template, one crate with one test file, a script test."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        # never the shared store of this machine (BUILD-PREAMBLE 1)
        self._db_env = mock.patch.dict(os.environ, {
            "MECLAW_PIPELINE_DB": os.path.join(self._tmp.name, "pipeline.db"),
            "MECLAW_PIPELINE_METRICS_URL": ""})
        self._db_env.start()
        self.root = os.path.join(self._tmp.name, "repo")
        os.makedirs(self.root)
        write(self.root, "templates/x/template.json", '{"name": "x", "version": "1.0.0"}\n')
        write(self.root, "crates/meclaw-demo/Cargo.toml", '[package]\nname = "meclaw-demo"\n')
        write(self.root, "crates/meclaw-demo/src/lib.rs", "pub fn a() {}\n")
        write(self.root, "crates/meclaw-demo/tests/gh1_case.rs", "#[test]\nfn t() {}\n")
        write(self.root, "docs/a.md", "# a\n")
        init_repo(self.root)

    def tearDown(self):
        self._db_env.stop()
        self._tmp.cleanup()

    def run_pre(self, **kw):
        lines = []
        doc = pre.run_pre(self.root, out=lines.append, **kw)
        return doc, lines


class LocalHalf(Fixture):
    def test_pre_local_half_spawns_git_only_with_fixture_tree(self):
        # The local half's budget is work (VG TV-K3): without a lane it runs
        # git and nothing else -- no cargo, no test process.
        write(self.root, "docs/a.md", "# a\n\nmore\n")
        procs = []
        real = subprocess.run

        def run(argv, *a, **kw):
            procs.append(str(argv[0]))
            return real(argv, *a, **kw)

        with mock.patch.object(subprocess, "run", run):
            doc, lines = self.run_pre()
        self.assertTrue(procs)
        self.assertEqual({"git"}, set(procs), procs)
        self.assertEqual("GREEN", doc["verdict"])
        self.assertFalse(doc["plan"]["lane_half"])
        self.assertTrue(lines[-1].startswith("PRE-SUMMARY "))

    def test_pre_lines_are_machine_readable(self):
        write(self.root, "examples/d/main/config.json", '{\n  "template": "x@0.9.0"\n}\n')
        doc, lines = self.run_pre()
        self.assertEqual("RED", doc["verdict"])
        for line in lines[:-1]:
            self.assertRegex(line, LINE)
        self.assertRegex(lines[-1], SUMMARY)
        reds = [l for l in lines if " RED" in l and l.startswith("PRE ")]
        self.assertEqual(["PRE pin-lock 0s RED examples/d/main/config.json:2 x@0.9.0 not "
                          "provided -- raise to 1.0.0"], reds)
        for red in reds:
            self.assertRegex(red, r" [^ ]+:\d+ ")
        with open(os.path.join(pre.git_dir(self.root), "pre.json"), encoding="utf-8") as fh:
            body = json.load(fh)
        self.assertIn({"check": "pin-lock", "verdict": "RED"},
                      [{"check": c["check"], "verdict": c["verdict"]} for c in body["checks"]])


UT_GREEN = '''import unittest


class T(unittest.TestCase):
    def test_one(self):
        self.assertTrue(True)

    def test_two(self):
        self.assertTrue(True)
'''

UT_RED = '''import unittest


class T(unittest.TestCase):
    def test_red(self):
        self.assertEqual(1, 2)
'''

UT_CRASH = '''import os
import unittest


class T(unittest.TestCase):
    def test_crash(self):
        os._exit(3)
'''

UT_HANG = '''import os
import threading
import time
import unittest


class T(unittest.TestCase):
    def test_hang(self):
        with open(%r, "w") as fh:
            fh.write(str(os.getpid()))
        time.sleep(600)
'''

# Hangs after writing the config of the tree: a key the guard calls drift,
# and a key a parallel git writes (a NOTE).
UT_HANG_CONFIG = UT_HANG.replace("import os\n", "import os\nimport subprocess\n", 1).replace(
    "        time.sleep(600)",
    "        for kv in ([\"core.worktree\", \"/elsewhere\"], [\"branch.x.remote\", \"origin\"]):\n"
    "            subprocess.run([\"git\", \"config\"] + kv, check=True)\n"
    "        time.sleep(600)")
SHARD_OK = "Ran 1 test in 0.001s\n\nOK\nSELFTEST-VERDICT GREEN 1/1 shards\n"
# The tail of a one-shard run (plain `python3 -m unittest` on stderr): the
# runner judges it, an "OK" without "Ran" is RED (re-check tb-s2 h2 M1).
UT_OK = "Ran 1 test in 0.0s\n\nOK\n"


class UnittestHalf(Fixture):
    """The unit tests of `pre` run through the selftest shard (#1160): the same
    modules, side by side, one line, fail-closed, never wider than 4 next to egon."""

    def module(self, name, body):
        write(self.root, "scripts/__init__.py", "")
        write(self.root, "scripts/tests/__init__.py", "")
        write(self.root, "scripts/tests/%s.py" % name, body)
        return "scripts.tests." + name

    def half(self, modules, shards="2"):
        with mock.patch.dict(os.environ, {"MECLAW_SELFTEST_SHARDS": shards}):
            return pre.unittest_half(modules, self.root)

    def test_pre_unittest_runs_sharded_and_reports_one_line(self):
        mods = [self.module("test_fx_a", UT_GREEN), self.module("test_fx_b", UT_GREEN)]
        res = self.half(mods)
        self.assertEqual(["unittest"], [r.check for r in res])
        self.assertEqual("GREEN", res[0].verdict, res[0].text)
        self.assertEqual(" ".join(mods) + " shards=2", res[0].text)
        self.assertRegex(res[0].line(), LINE)
        base = os.path.join(pre.git_dir(self.root), "pre-unittest")
        runs = [os.path.join(base, d) for d in os.listdir(base)]
        self.assertEqual(1, len(runs), runs)
        self.assertTrue(os.path.isfile(os.path.join(runs[0], "shard-1.log")))
        self.assertTrue(os.path.isfile(os.path.join(runs[0], "shard-2.log")))

    def test_pre_unittest_each_run_has_its_own_logs(self):
        # Re-check tb-s2 h2 m4: two runs wrote the same shard-k.log and read it back.
        mods = [self.module("test_fx_a", UT_GREEN), self.module("test_fx_b", UT_GREEN)]
        base = os.path.join(pre.git_dir(self.root), "pre-unittest")
        stale = os.path.join(base, "run-stale")
        os.makedirs(stale)
        os.utime(stale, (time.time() - 2 * 86400,) * 2)
        self.half(mods)
        self.half(mods)
        runs = sorted(d for d in os.listdir(base))
        self.assertEqual(2, len(runs), runs)
        for d in runs:
            self.assertTrue(d.startswith("run-"), d)
            self.assertTrue(os.path.isfile(os.path.join(base, d, "shard-2.log")), d)

    def test_pre_git_dir_is_the_trees_with_an_inherited_git_dir(self):
        # Re-check tb-s2 h2 m4: `git_dir` asked git without cleaning the environment.
        want = pre.git_dir(self.root)
        with mock.patch.dict(os.environ, {"GIT_DIR": "/elsewhere/.git"}):
            self.assertEqual(want, pre.git_dir(self.root))

    def test_pre_unittest_a_hung_test_is_red_and_leaves_no_process(self):
        # Re-check tb-s2 h2 m5: no bound -- a hung test held pre forever.
        pidfile = os.path.join(self.root, "hung.pid")
        hang = UT_HANG % pidfile
        for shards, mods in (("1", [self.module("test_fx_hang", hang)]),
                             ("2", [self.module("test_fx_a", UT_GREEN),
                                    self.module("test_fx_hang2", hang)])):
            if os.path.exists(pidfile):
                os.unlink(pidfile)
            # The bound is the event (RED `timeout after 4s`); a half that
            # misses it holds past the hang marker on the 600-s sleep.
            box = {}
            with mock.patch.object(pre, "unittest_timeout", return_value=4):
                th = threading.Thread(target=lambda: box.update(res=self.half(mods, shards=shards)),
                                      daemon=True)
                th.start()
                th.join(kit_time.HANG)
            self.assertFalse(th.is_alive(), "HANG: the half with a hung test after %.0f s "
                             "(shards=%s)" % (kit_time.HANG, shards))
            res = box["res"]
            self.assertEqual("RED", res[0].verdict, res[0].text)
            self.assertIn("timeout after 4s", res[0].text)
            # Re-check tv-kit fix15 H2: the seed printed before the hang survives it.
            self.assertIsInstance(res[0].seed, int)
            self.assertIn(pre.unittest_rerun(mods, res[0].seed, int(shards)), res[0].text)
            with open(pidfile) as fh:
                pid = int(fh.read())

            def gone():
                try:
                    os.kill(pid, 0)
                except ProcessLookupError:
                    return True
                return False

            try:
                kit_time.until("the hung test %d ends with the half (shards=%s)" % (pid, shards),
                               gone)
            except kit_time.Hang:
                os.kill(pid, 9)
                raise

    def test_pre_unittest_a_hung_test_still_names_the_drift_and_the_note(self):
        # Re-check tb-s2 h1 m-N1 / h2 m-N3: the timeout branch names what the run
        # changed -- untested, `drift, note = "", ""` there stayed green.
        pidfile = os.path.join(self.root, "hung.pid")
        mod = self.module("test_fx_hang_cfg", UT_HANG_CONFIG % pidfile)
        with mock.patch.object(pre, "unittest_timeout", return_value=4):
            res = self.half([mod], shards="1")
        self.assertEqual(["RED", "NOTE"], [r.verdict for r in res], [r.text for r in res])
        self.assertIn("timeout after 4s", res[0].text)
        self.assertIn("+core.worktree=/elsewhere", res[0].text)
        self.assertEqual("repo-config", res[1].check)
        self.assertIn("+branch.x.remote=origin", res[1].text)
        with open(pidfile) as fh:
            pid = int(fh.read())
        # Re-check tv-bn M3 B-H1: the kit's wait (HANG mark, time factor), not a
        # literal 10-s poll; a zombie nobody reaped yet is ended, not alive.
        try:
            kit_time.until("the hung test %d ended" % pid, lambda: pid_ended(pid))
        except kit_time.Hang:
            os.kill(pid, 9)
            raise

    def test_a_zombie_counts_as_ended(self):
        # Re-check tv-bn M3 B-H1: `os.kill(pid, 0)` answers for a zombie too, so a
        # wait on it ran into its deadline under load although the process had ended.
        child = subprocess.Popen(["true"])
        self.addCleanup(child.wait)
        kit_time.until("the unreaped child %d is ended" % child.pid, lambda: pid_ended(child.pid))

    def test_pre_unittest_timeout_comes_from_the_table(self):
        from pipeline import selftest_shard
        known = selftest_shard.load_durations(selftest_shard.DURATIONS)
        mod = "scripts.tests.test_strand_sh"
        self.assertEqual(pre.UNITTEST_TIMEOUT_FLOOR_S + pre.UNITTEST_TIMEOUT_FACTOR * known[mod],
                         pre.unittest_timeout([mod, mod]))
        self.assertEqual(pre.UNITTEST_TIMEOUT_FLOOR_S
                         + pre.UNITTEST_TIMEOUT_FACTOR * pre.UNITTEST_UNKNOWN_S,
                         pre.unittest_timeout(["scripts.tests.test_nowhere"]))

    def test_pre_unittest_shard_red_is_red(self):
        mods = [self.module("test_fx_a", UT_GREEN), self.module("test_fx_red", UT_RED)]
        res = self.half(mods)
        self.assertEqual(1, len(res))
        self.assertEqual("RED", res[0].verdict)
        self.assertIn("shards=2", res[0].text)
        self.assertIn("1/2 shards; shard 2/2 1 failed", res[0].text)
        self.assertRegex(res[0].line(), LINE)
        # Kit find C4 (Welle Testing): the red test and the seed of the order.
        self.assertEqual(["scripts.tests.test_fx_red.T.test_red"], res[0].tests)
        self.assertIsInstance(res[0].seed, int)
        self.assertIn("red scripts.tests.test_fx_red.T.test_red", res[0].line())
        self.assertIn("seed %d" % res[0].seed, res[0].line())
        self.assertEqual(1, res[0].line().count("seed %d" % res[0].seed), res[0].line())
        # Re-check tv-kit fix15 M1: the hint is the command that replays the
        # order -- the runner with seed and width; bare unittest reads neither.
        self.assertIn("-- replay: `MECLAW_SELFTEST_SEED=%d MECLAW_SELFTEST_SHARDS=2 /usr/bin/python3 -I "
                      "scripts/pipeline/selftest_shard.py unittest %s`"
                      % (res[0].seed, " ".join(mods)), res[0].line())
        self.assertNotIn("python3 -m unittest", res[0].line())

    def test_pre_unittest_one_shard_red_names_the_test_and_the_seed(self):
        # One shard: the runner prints no verdict line of its own; pre reads the names itself.
        mods = [self.module("test_fx_red", UT_RED)]
        with mock.patch.dict(os.environ, {"MECLAW_SELFTEST_SEED": "42"}):
            res = self.half(mods, shards="1")
        self.assertEqual("RED", res[0].verdict, res[0].text)
        self.assertEqual(["scripts.tests.test_fx_red.T.test_red"], res[0].tests)
        self.assertEqual(42, res[0].seed)
        self.assertIn("red scripts.tests.test_fx_red.T.test_red; seed 42 -- replay: "
                      "`MECLAW_SELFTEST_SEED=42 MECLAW_SELFTEST_SHARDS=1 /usr/bin/python3 -I "
                      "scripts/pipeline/selftest_shard.py unittest scripts.tests.test_fx_red`",
                      res[0].line())
        self.assertNotIn("python3 -m unittest", res[0].line())

    def test_pre_unittest_without_a_seed_keeps_the_plain_unittest_hint(self):
        # Re-check tv-kit fix15 M1: no seed in the output -- the plain command.
        self.assertEqual("run `/usr/bin/python3 -I scripts/pipeline/hang_dump.py -m unittest a b`",
                         pre.unittest_rerun(["a", "b"], None, 3))
        self.assertEqual("replay: `MECLAW_SELFTEST_SEED=7 MECLAW_SELFTEST_SHARDS=3 /usr/bin/python3 -I "
                         "scripts/pipeline/selftest_shard.py unittest a b`",
                         pre.unittest_rerun(["a", "b"], 7, 3))

    def test_pre_unittest_shard_crash_is_red(self):
        mods = [self.module("test_fx_a", UT_GREEN), self.module("test_fx_crash", UT_CRASH)]
        res = self.half(mods)
        self.assertEqual("RED", res[0].verdict)
        self.assertIn("without a unittest summary", res[0].text)
        # one shard: the runner execs `python3 -m unittest` -- still fail-closed
        res = self.half([mods[1]], shards="1")
        self.assertEqual("RED", res[0].verdict)
        self.assertIn("without a unittest summary", res[0].text)

    def test_pre_unittest_width_egon_is_at_most_four(self):
        seen = []
        real = subprocess.run

        def run(argv, *a, **kw):
            if len(argv) <= SCRIPT_AT or not str(argv[SCRIPT_AT]).endswith("selftest_shard.py"):
                return real(argv, *a, **kw)
            seen.append((argv, kw.get("env") or {}))
            return subprocess.CompletedProcess(argv, 0, SHARD_OK, "")

        with mock.patch.object(pre, "_nproc", return_value=32), \
                mock.patch.object(pre.subprocess, "run", side_effect=run):
            for env, want in (("16", "4"), ("2", "2"), ("", "4"), ("x", "4")):
                seen.clear()
                self.half(["scripts.tests.test_fx_a"], shards=env)
                self.assertEqual(want, seen[0][1].get("MECLAW_SELFTEST_SHARDS"), env)
        with mock.patch.object(pre, "_nproc", return_value=3), \
                mock.patch.object(pre.subprocess, "run", side_effect=run):
            seen.clear()
            self.half(["scripts.tests.test_fx_a"], shards="16")
            self.assertEqual("3", seen[0][1].get("MECLAW_SELFTEST_SHARDS"))
        argv = seen[0][0]
        self.assertEqual(os.path.join(os.path.dirname(os.path.abspath(pre.__file__)),
                                      "selftest_shard.py"), argv[SCRIPT_AT])
        self.assertEqual(list(lane.PYTHON), argv[:SCRIPT_AT])
        self.assertEqual("unittest", argv[SCRIPT_AT + 1])

    def test_pre_unittest_module_set_unchanged(self):
        self.module("test_fx_a", UT_GREEN)
        self.module("test_fx_b", UT_GREEN)
        write(self.root, "scripts/pipeline/tool.py", "X = 1\n")
        write(self.root, "scripts/tests/helper.py", "Y = 1\n")
        real = subprocess.run
        spied = []

        def run(argv, *a, **kw):
            if len(argv) > SCRIPT_AT and str(argv[SCRIPT_AT]).endswith("selftest_shard.py"):
                spied.append(list(argv))
                return subprocess.CompletedProcess(argv, 0, SHARD_OK, "")
            return real(argv, *a, **kw)

        with mock.patch.object(pre.subprocess, "run", side_effect=run):
            doc, _lines = self.run_pre()
        self.assertEqual(["scripts.tests.test_fx_a", "scripts.tests.test_fx_b"],
                         doc["plan"]["unittests"])
        self.assertEqual(1, len(spied))
        self.assertEqual(doc["plan"]["unittests"], spied[0][SCRIPT_AT + 2:])


class Summary(unittest.TestCase):
    def test_summary_counts_no_note_check(self):
        # Review M1 / V0 section 5: a NOTE is a finding to read, not a verdict --
        # it is neither green nor part of the total.
        results = [pre.Result("form", 0, "GREEN"), pre.Result("diff-size", 0, "NOTE", "big"),
                   pre.Result("lane", 0, "NOTE", "no lane"), pre.Result("pin-lock", 0, "RED", "x"),
                   pre.Result("test-line-lock", 0, "NOTE", "a"),
                   pre.Result("test-line-lock", 0, "GREEN")]
        green, total, verdict, line = pre.summarize(results, "ab" * 32, 3.2)
        self.assertEqual((2, 3, "RED"), (green, total, verdict))
        self.assertEqual("PRE-SUMMARY abababab 2/3 3s RED", line)
        green, total, verdict, _ = pre.summarize(results[:3], "ab" * 32, 1)
        self.assertEqual((1, 1, "GREEN"), (green, total, verdict))


class Shellcheck(Fixture):
    def test_shellcheck_red_names_path_and_line(self):
        # Review M3 / plan section 4: every RED line names `path:line`.
        self.assertEqual("scripts/a.sh:3:6 warning: foo is referenced but not assigned [SC2154]",
                         pre.shellcheck_first(
                             "scripts/a.sh:3:6: warning: foo is referenced but not assigned."
                             " [SC2154]\nscripts/a.sh:4:1: warning: other [SC2034]\n"))
        if not shutil.which("shellcheck"):
            self.skipTest("shellcheck not installed")
        write(self.root, "scripts/a.sh", "#!/usr/bin/env bash\nset -u\necho \"$foo\"\n", 0o755)
        doc, lines = self.run_pre()
        red = [l for l in lines if l.startswith("PRE shellcheck")]
        self.assertEqual(1, len(red))
        self.assertRegex(red[0], r"^PRE shellcheck \d+s RED scripts/a\.sh:3:\d+ warning: .*SC2154")


class LaneHalf(Fixture):
    def test_a_browsers_opt_out_is_in_the_lane_result_and_the_summary(self):
        """Re-check tv-bn M7c M1: `pre`'s test-binaries ran through
        `lane_sync.sh test` (browsers required unless opted out); an opt-out
        stood only in the runner's text, the PRE line and PRE-SUMMARY read
        like a full run. Both name it now."""
        note = ("NOTE: MECLAW_REQUIRE_BROWSERS='0' (opt-out) -- a browser driver without an engine "
                "SKIPs green, no browser proof\n")
        cmds = [["lane", "nextest", "-p", "meclaw-cells", "--test", "gh1"]]
        for text, mark in ((note + "ok\n", "(browsers opt-out: MECLAW_REQUIRE_BROWSERS='0')"), ("ok\n", "")):
            res = pre.lane_half(cmds, lambda argv: (0, text))
            with self.subTest(mark=mark):
                self.assertEqual(["GREEN"], [r.verdict for r in res])
                summary = pre.summarize(res, "0123456789", 1.0)[3]
                if mark:
                    self.assertIn(mark, res[0].line())
                    self.assertTrue(summary.endswith("GREEN " + mark), summary)
                else:
                    self.assertNotIn("opt-out", res[0].line() + summary)

    def test_lane_half_builds_the_test_hooks(self):
        # GH #1208: the lane half checks and runs with the test-hook feature
        # the gate's build station gives the same selection.
        write(self.root, "crates/meclaw-colony/Cargo.toml", '[package]\nname = "meclaw-colony"\n')
        write(self.root, "crates/meclaw-colony/src/lib.rs", "pub fn a() {}\n")
        write(self.root, "crates/meclaw-colony/tests/t9.rs", "#[test]\nfn t() {}\n")
        p = pre.plan(["crates/meclaw-colony/src/lib.rs", "crates/meclaw-colony/tests/t9.rs"],
                     self.root)
        feat = ["--features", "meclaw-colony/test-hooks"]
        self.assertEqual(
            [["cargo", "check", "--tests", "-p", "meclaw-colony"] + feat,
             ["cargo", "nextest", "run", "--no-fail-fast", "--no-tests=warn", "-p",
              "meclaw-colony", "--test", "t9"] + feat],
            pre.lane_commands(p, self.root))
        # meclaw-demo knows no meclaw-colony: cargo would refuse the flag
        p = pre.plan(["crates/meclaw-demo/src/lib.rs"], self.root)
        self.assertEqual([["cargo", "check", "--tests", "-p", "meclaw-demo"]],
                         pre.lane_commands(p, self.root))

    def test_pre_selects_lane_half_only_for_rust_diff(self):
        self.assertFalse(pre.plan(["docs/a.md", "scripts/x.py"], self.root)["lane_half"])
        p = pre.plan(["crates/meclaw-demo/src/lib.rs"], self.root)
        self.assertTrue(p["lane_half"])
        self.assertEqual(["meclaw-demo"], p["crates"])
        self.assertEqual([["cargo", "check", "--tests", "-p", "meclaw-demo"]],
                         pre.lane_commands(p))
        # a Rust diff without a lane: the half is skipped with a NOTE, the stamp says so
        write(self.root, "crates/meclaw-demo/src/lib.rs", "pub fn b() {}\n")
        doc, lines = self.run_pre()
        self.assertIn("PRE lane 0s NOTE pre: no lane held, cargo half skipped", lines)
        self.assertFalse(doc["lane_half"])

    def test_pre_selects_lane_half_for_a_crate_file_outside_src_and_tests(self):
        """Rule 10 (welle-tb S1b): an `include_str!` schema, an example or a bench
        is compiled with its crate -- `cargo check --tests` must see the change."""
        for rel in ("crates/meclaw-demo/schemas/body.json",
                    "crates/meclaw-demo/examples/demo.rs",
                    "crates/meclaw-demo/benches/hot.rs"):
            p = pre.plan([rel], self.root)
            self.assertTrue(p["lane_half"], rel)
            self.assertEqual(["meclaw-demo"], p["crates"], rel)

    def test_pre_lane_half_keeps_every_changed_test_target(self):
        """No source heuristic decides which test files run (review W7 I2): a
        file with real tests next to ignored ones and a doc line that mentions
        `#[ignore]` stays a target, and so does an all-ignored browser lock."""
        write(self.root, "crates/meclaw-demo/tests/gh2_browser.rs",
              "#[test]\n#[ignore = \"needs a browser\"]\nfn a_chromium() {}\n"
              "#[tokio::test]\n#[ignore]\nasync fn a_webkit() {}\n")
        write(self.root, "crates/meclaw-demo/tests/gh3_case.rs",
              "//! The slow ones carry `#[ignore]`, see below; `#[ignore]` again.\n"
              "#[test]\nfn t() {}\n#[test]\nfn u() {}\n#[test]\n#[ignore]\nfn slow() {}\n")
        p = pre.plan(["crates/meclaw-demo/tests/gh2_browser.rs",
                      "crates/meclaw-demo/tests/gh3_case.rs"], self.root)
        self.assertEqual([("meclaw-demo", "gh2_browser"), ("meclaw-demo", "gh3_case")],
                         p["test_targets"])
        self.assertNotIn("ignored_only", p)

    def _harness(self):
        write(self.root, "crates/meclaw-demo/tests/misc/main.rs", "mod gh1_a;\nmod gh2_b;\n")
        write(self.root, "crates/meclaw-demo/tests/misc/gh1_a.rs", "#[test]\nfn a() {}\n")
        write(self.root, "crates/meclaw-demo/tests/misc/gh2_b.rs", "#[test]\nfn b() {}\n")

    def _commit(self, msg):
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", msg)
        return git(self.root, "rev-parse", "HEAD")

    def _misc_lane(self, paths, base):
        return pre.lane_commands(pre.plan(paths, self.root, base=base))[1:]

    def test_pre_runs_only_the_changed_module_of_a_folded_harness(self):
        """GH #1130: adding a test folds it into its harness, so the diff names
        `main.rs` (one `mod` line more) and the new module. The lane half ran
        the WHOLE harness binary (misc_2 + misc_3: 646 tests, 45 s of run on
        build03); a main.rs whose change against the base is that `mod` line
        alone touches no other module, so pre builds the binary and runs the
        module with the term of `gate_plan.test_file_unit`. The gate's rule 3
        still runs the whole binary (main.rs is the unit `<h>/main`)."""
        write(self.root, "crates/meclaw-demo/tests/misc/main.rs", "mod gh1_a;\n")
        write(self.root, "crates/meclaw-demo/tests/misc/gh1_a.rs", "#[test]\nfn a() {}\n")
        base = self._commit("harness")
        self._harness()
        paths = ["crates/meclaw-demo/tests/misc/main.rs", "crates/meclaw-demo/tests/misc/gh2_b.rs"]
        p = pre.plan(paths, self.root, base=base)
        self.assertEqual([("meclaw-demo", "misc")], p["test_targets"])
        self.assertEqual(
            [["cargo", "nextest", "run", "--no-fail-fast", "--no-tests=warn",
              "-p", "meclaw-demo", "--test", "misc",
              "-E", "(binary_id(=meclaw-demo::misc) & test(/^gh2_b::/))"]],
            self._misc_lane(paths, base))

    def test_pre_narrows_a_folded_harness_when_the_diff_also_names_a_script(self):
        """Delta PF1: the unittest loop of `plan` bound each path's basename to
        `base`, the merge base `main_rs_folds_only` reads. A diff with a path
        under scripts/ next to a folded harness asked git for `test_x.py:<main.rs>`
        and ran the whole binary; the merge base must survive the loop."""
        write(self.root, "crates/meclaw-demo/tests/misc/main.rs", "mod gh1_a;\n")
        write(self.root, "crates/meclaw-demo/tests/misc/gh1_a.rs", "#[test]\nfn a() {}\n")
        base = self._commit("harness")
        self._harness()
        write(self.root, "scripts/tests/test_x.py", "import unittest\n")
        paths = ["crates/meclaw-demo/tests/misc/main.rs", "crates/meclaw-demo/tests/misc/gh2_b.rs",
                 "scripts/tests/test_x.py"]
        p = pre.plan(paths, self.root, base=base)
        self.assertEqual(["scripts.tests.test_x"], p["unittests"])
        self.assertEqual({"meclaw-demo::misc": ["gh2_b"]}, p["test_modules"])

    def test_pre_runs_the_whole_harness_when_main_rs_changes_more_than_mod_lines(self):
        """Re-check RC-P1 I1: 30 of 44 harness main.rs carry more than `mod`
        lines (`#[path = "../mock_openai.rs"]`, `../support/*.rs`, `pub use`).
        A diff that changes such a line AND a module ran only the module, where
        pre ran the whole harness before and the gate's rule 3 runs it whole:
        the users of the moved helper ran nowhere before the gate. Narrowing
        needs a main.rs that differs from the base in `mod` lines of the named
        modules only; a non-mod line, a new main.rs or no base -> whole."""
        whole = [["cargo", "nextest", "run", "--no-fail-fast", "--no-tests=warn",
                  "-p", "meclaw-demo", "--test", "misc"]]
        write(self.root, "crates/meclaw-demo/tests/misc/main.rs",
              '#[path = "../support/a.rs"]\nmod support;\nmod gh1_a;\n')
        write(self.root, "crates/meclaw-demo/tests/support/a.rs", "pub fn a() {}\n")
        write(self.root, "crates/meclaw-demo/tests/support/b.rs", "pub fn b() {}\n")
        write(self.root, "crates/meclaw-demo/tests/misc/gh1_a.rs", "#[test]\nfn a() {}\n")
        base = self._commit("harness")
        paths = ["crates/meclaw-demo/tests/misc/main.rs", "crates/meclaw-demo/tests/misc/gh2_b.rs"]
        write(self.root, "crates/meclaw-demo/tests/misc/gh2_b.rs", "#[test]\nfn b() {}\n")
        for label, main_rs, b in (
                ("path line moved", '#[path = "../support/b.rs"]\nmod support;\nmod gh1_a;\nmod gh2_b;\n', base),
                ("mod line of a module the diff does not name",
                 '#[path = "../support/a.rs"]\nmod support;\nmod gh2_b;\n', base),
                ("no base", '#[path = "../support/a.rs"]\nmod support;\nmod gh1_a;\nmod gh2_b;\n', None)):
            with self.subTest(label):
                write(self.root, "crates/meclaw-demo/tests/misc/main.rs", main_rs)
                self.assertEqual(whole, self._misc_lane(paths, b))
                self.assertEqual({}, pre.plan(paths, self.root, base=b)["test_modules"])
        # the same harness, only `mod gh2_b;` added: narrowed
        write(self.root, "crates/meclaw-demo/tests/misc/main.rs",
              '#[path = "../support/a.rs"]\nmod support;\nmod gh1_a;\nmod gh2_b;\n')
        self.assertEqual({"meclaw-demo::misc": ["gh2_b"]},
                         pre.plan(paths, self.root, base=base)["test_modules"])

    def test_pre_runs_the_whole_harness_when_main_rs_is_new(self):
        # a new harness: main.rs is not in the base, no line of it is proven a fold
        base = git(self.root, "rev-parse", "HEAD")
        self._harness()
        p = pre.plan(["crates/meclaw-demo/tests/misc/main.rs",
                      "crates/meclaw-demo/tests/misc/gh2_b.rs"], self.root, base=base)
        self.assertEqual({}, p["test_modules"])

    def test_pre_runs_the_whole_binary_when_the_diff_names_a_helper_and_a_module(self):
        """Re-check RC-P1 I2: a helper module runs its harness whole (its users'
        test names do not start `<helper>::`, W5 delta C1). With a test module
        of the same harness in the diff the whole binary must win -- a narrowed
        `-E` for the test module would drop the helper's users."""
        write(self.root, "crates/meclaw-demo/tests/misc/main.rs",
              "mod helper;\nmod gh1_a;\nmod gh2_b;\n")
        write(self.root, "crates/meclaw-demo/tests/misc/helper.rs", "pub fn h() {}\n")
        write(self.root, "crates/meclaw-demo/tests/misc/gh1_a.rs",
              "use crate::helper;\n#[test]\nfn a() { helper::h() }\n")
        write(self.root, "crates/meclaw-demo/tests/misc/gh2_b.rs", "#[test]\nfn b() {}\n")
        p = pre.plan(["crates/meclaw-demo/tests/misc/helper.rs",
                      "crates/meclaw-demo/tests/misc/gh2_b.rs"], self.root)
        self.assertEqual({}, p["test_modules"])
        self.assertEqual([["cargo", "nextest", "run", "--no-fail-fast", "--no-tests=warn",
                           "-p", "meclaw-demo", "--test", "misc"]], pre.lane_commands(p)[1:])

    def test_pre_plans_no_binary_for_a_file_in_a_directory_without_main_rs(self):
        """Re-check RC-P1 M1: `tests/support/*.rs` is shared code, no binary of
        its own; planning it would ask for `--test support`, a binary that does
        not exist (a false red in pre)."""
        write(self.root, "crates/meclaw-demo/tests/support/x.rs", "pub fn x() {}\n")
        p = pre.plan(["crates/meclaw-demo/tests/support/x.rs"], self.root)
        self.assertEqual([], p["test_targets"])
        self.assertEqual({}, p["test_modules"])

    def test_pre_runs_a_changed_module_without_its_main_rs(self):
        # an edited module alone named no target: its tests never ran in pre
        self._harness()
        p = pre.plan(["crates/meclaw-demo/tests/misc/gh1_a.rs"], self.root)
        self.assertEqual([("meclaw-demo", "misc")], p["test_targets"])
        self.assertEqual(["-E", "(binary_id(=meclaw-demo::misc) & test(/^gh1_a::/))"],
                         pre.lane_commands(p)[1][-2:])

    def test_pre_runs_the_whole_harness_for_main_rs_alone_or_an_unfolded_module(self):
        # no module to name: the whole binary, as rule 3 selects it
        self._harness()
        write(self.root, "crates/meclaw-demo/tests/misc/gh3_c.rs", "#[test]\nfn c() {}\n")
        for paths in (["crates/meclaw-demo/tests/misc/main.rs"],
                      ["crates/meclaw-demo/tests/misc/gh3_c.rs"]):
            with self.subTest(paths=paths):
                p = pre.plan(paths, self.root)
                self.assertEqual([("meclaw-demo", "misc")], p["test_targets"])
                self.assertEqual(["cargo", "nextest", "run", "--no-fail-fast", "--no-tests=warn",
                                  "-p", "meclaw-demo", "--test", "misc"],
                                 pre.lane_commands(p)[1])

    def test_pre_narrows_one_binary_and_keeps_its_neighbour_whole(self):
        self._harness()
        p = pre.plan(["crates/meclaw-demo/tests/misc/gh2_b.rs",
                      "crates/meclaw-demo/tests/misc/gh1_a.rs",
                      "crates/meclaw-demo/tests/gh1_case.rs"], self.root)
        self.assertEqual([("meclaw-demo", "gh1_case"), ("meclaw-demo", "misc")],
                         p["test_targets"])
        self.assertEqual(
            ["--test", "gh1_case", "--test", "misc", "-E",
             "binary_id(=meclaw-demo::gh1_case) | (binary_id(=meclaw-demo::misc) & test(/^gh1_a::/))"
             " | (binary_id(=meclaw-demo::misc) & test(/^gh2_b::/))"],
            pre.lane_commands(p)[1][7:])

    def test_the_lane_filter_is_the_narrowed_expression(self):
        """`lane_sync.sh test` takes the filterset as its own word: a second
        `-E` among the options would be OR-ed by nextest and run the whole
        binary again."""
        seen = []

        class Hold:
            def __init__(self, *a, **kw):
                self.stdout = io.StringIO("held tok\n")
                self.stdin, self.stderr = io.StringIO(), io.StringIO()

            def wait(self, timeout=None):
                return 0

        def exec_(argv, env=None, **kw):
            seen.append(argv)
            return subprocess.CompletedProcess(argv, 0, "ok", "")

        term = "(binary_id(=meclaw-demo::misc) & test(/^gh2_b::/))"
        run = pre.ssh_runner("lx", "s1", str(self.root), exec_=exec_, popen=Hold)
        rc, _ = run(["cargo", "nextest", "run", "--no-fail-fast", "-p", "meclaw-demo",
                     "--test", "misc", "-E", term, "--features", "f/x"])
        run.release()
        self.assertEqual(0, rc)
        test = [a for a in seen if a[1] == "test"][0]
        self.assertEqual(term, test[4])
        self.assertNotIn("-E", test[5:])
        self.assertEqual(["--no-fail-fast", "--features", "f/x", "--test", "misc"], test[5:])

    def test_pre_test_binaries_run_with_no_tests_warn(self):
        p = {"lane_half": True, "crates": ["meclaw-demo"],
             "test_targets": [("meclaw-demo", "gh1_case")]}
        run = pre.lane_commands(p)[1]
        self.assertEqual(["cargo", "nextest", "run", "--no-fail-fast", "--no-tests=warn",
                          "-p", "meclaw-demo", "--test", "gh1_case"], run)

    def test_pre_grades_a_run_of_zero_tests_as_note(self):
        """An all-ignored browser lock runs zero tests in pre; test-tier says
        `no tests matched` (exit 6). The gate station runs it with
        `--run-ignored all` -- pre notes it and stays green."""
        def runner(argv):
            if argv[1] == "check":
                return 0, "ok"
            return 6, "test-tier: RED -- no tests matched: binary_id(=meclaw-demo::gh2_browser)\n"
        res = pre.lane_half([["cargo", "check", "--tests", "-p", "meclaw-demo"],
                             ["cargo", "nextest", "run", "--no-fail-fast", "--no-tests=warn",
                              "-p", "meclaw-demo", "--test", "gh2_browser"]], runner)
        self.assertEqual(["GREEN", "NOTE"], [r.verdict for r in res])
        self.assertIn("zero tests ran", res[1].text)

    def test_an_egon_lane_refusing_check_is_a_note(self):
        # W4 re-check: a class egon lane runs 'test' only; its check refusal
        # (exit 2) is no verdict about the tree -- NOTE, never RED.
        def runner(argv):
            return 2, ("lane_sync: a class egon lane runs 'test' only -- no check there "
                       "(MECLAW_LANE_EGON_GATE_OK=<reason> overrides)\n")
        res = pre.lane_half([["cargo", "check", "--tests"],
                             ["cargo", "nextest", "run", "-p", "c", "--test", "t"]], runner)
        self.assertEqual(["NOTE"], [r.verdict for r in res])
        self.assertIn("class egon", res[0].text)

    def test_only_the_egon_refusal_line_turns_check_rc_2_into_a_note(self):
        # Review Zb M1: rc 2 alone (floor refusal, host gone) stays RED; the
        # refusal text counts only as lane_sync's own line, not quoted inside.
        for text in ("lane_sync: free 40G under the floor 60G\n",
                     "ssh: host did not answer\n",
                     "note: a class egon lane runs 'test' only -- quoted\n"):
            with self.subTest(text=text):
                res = pre.lane_half([["cargo", "check", "--tests"]], lambda argv: (2, text))
                self.assertEqual(["RED"], [r.verdict for r in res])

    def test_check_reserves_less_than_a_cold_target(self):
        # W4 delta: the check inherited MECLAW_LANE_EXPECTED_G=35 (a cold
        # target); a cargo check writes rmeta only and reserves less.
        seen = {}

        class Hold:
            def __init__(self, *a, **kw):
                self.stdout = io.StringIO("held tok\n")
                self.stdin, self.stderr = io.StringIO(), io.StringIO()

            def wait(self, timeout=None):
                return 0

        def exec_(argv, env=None, **kw):
            seen.setdefault(argv[1], env)
            return subprocess.CompletedProcess(argv, 0, "ok", "")

        with mock.patch.dict(os.environ, {"MECLAW_LANE_EXPECTED_G": "35"}):
            run = pre.ssh_runner("lx", "s1", str(self.root), exec_=exec_, popen=Hold)
            rc, _ = run(["cargo", "check", "--tests"])
            run.release()
        self.assertEqual(0, rc)
        self.assertEqual(str(pre.CHECK_EXPECTED_G), seen["check"]["MECLAW_LANE_EXPECTED_G"])
        self.assertLess(pre.CHECK_EXPECTED_G, 35)

    def test_pre_keeps_a_failing_test_red(self):
        def runner(argv):
            return (0, "ok") if argv[1] == "check" else (100, "error: test failed\n")
        res = pre.lane_half([["cargo", "check", "--tests"],
                             ["cargo", "nextest", "run", "-p", "c", "--test", "t"]], runner)
        self.assertEqual(["GREEN", "RED"], [r.verdict for r in res])

    def test_pre_lane_half_builds_only_changed_test_targets(self):
        write(self.root, "crates/meclaw-demo/tests/gh1_case.rs", "#[test]\nfn t2() {}\n")
        seen = []

        def fake_cargo(argv):
            seen.append(argv)
            return 0, "ok"
        doc, lines = self.run_pre(runner=fake_cargo)
        self.assertEqual([["cargo", "check", "--tests", "-p", "meclaw-demo"],
                          ["cargo", "nextest", "run", "--no-fail-fast", "--no-tests=warn",
                           "-p", "meclaw-demo", "--test", "gh1_case"]], seen)
        self.assertTrue(all("-E" not in argv for argv in seen))
        self.assertTrue(doc["lane_half"])
        self.assertEqual("GREEN", doc["verdict"])

    def test_fixture_0810_compile_error_in_test_line_red_in_lane_half(self):
        write(self.root, "crates/meclaw-demo/tests/gh1_case.rs", "#[test]\nfn t() { let x: u8 = ; }\n")

        cargo = []

        def fake_cargo(argv):
            cargo.append(argv[1])
            if argv[1] == "check":
                return 101, ("error: expected expression, found `;`\n"
                             " --> crates/meclaw-demo/tests/gh1_case.rs:2:22\n")
            return 0, ""
        doc, lines = self.run_pre(runner=fake_cargo)
        self.assertEqual("RED", doc["verdict"])
        self.assertEqual(["check"], cargo, "the lane half ran on after the red check")
        red = [l for l in lines if l.startswith("PRE cargo-check")]
        self.assertEqual(1, len(red))
        self.assertIn("crates/meclaw-demo/tests/gh1_case.rs:2:22 error: expected expression", red[0])
        # the check failed: no test binary was built after it
        self.assertFalse([l for l in lines if l.startswith("PRE test-binaries")])

    def test_fast_never_runs_the_lane_half(self):
        # Review M2: `--fast` is the local half only, also with a lane target.
        write(self.root, "crates/meclaw-demo/tests/gh1_case.rs", "#[test]\nfn t3() {}\n")
        seen = []

        def fake_cargo(argv):
            seen.append(argv)
            return 0, ""
        doc, lines = self.run_pre(runner=fake_cargo, fast=True)
        self.assertEqual([], seen)
        self.assertFalse(doc["lane_half"])
        self.assertIn("fast: local half only", doc["reasons"])
        self.assertIn("PRE lane 0s NOTE pre: --fast, cargo half skipped", lines)
        made = []
        orig = pre.ssh_runner
        try:
            pre.ssh_runner = lambda *a, **k: made.append(a) or fake_cargo
            with contextlib.redirect_stdout(io.StringIO()):
                rc = pre.main(["--repo", self.root, "--fast", "--lane-target", "u@lane"])
        finally:
            pre.ssh_runner = orig
        self.assertEqual(0, rc)
        self.assertEqual([], made)
        self.assertEqual([], seen)

# A real ssh carries none of the caller's environment (GH #1209): the names of
# the one rule (fake_ssh.py) reach the host only as words of the command line.
FAKE_SSH_TEMPLATE = """#!/usr/bin/env bash
while [ $# -gt 0 ]; do
    case "$1" in
        -o|-p|-l|-i|-E|-F) shift 2 ;;
        -*) shift ;;
        *) break ;;
    esac
done
host="$1"; shift
printf '%s\\t%s\\n' "$host" "$*" >>"$FAKE_SSH_LOG"
# @fake-ssh: strip the transported names@
exec bash -c "$*"
"""
FAKE_SSH = fake_ssh.script(FAKE_SSH_TEMPLATE)


class LaneVerb(Fixture):
    """The cargo check of `pre` through `lane_sync.sh check`, after the REAL push.

    Review I2 (re-check 191deecd8): pre composed its own lane paths
    (`wt/<strand>`, `target/`) while `lane_sync.sh push` checks a cargo run's
    commit out in the lane's build path (`tree/`, or `slot/<n>/tree`). The
    check ran in a stale or missing tree, stamped `.gate-tree` on the wrong
    path, and skipped the floor and the lane's environment. Here a fake `ssh`
    runs the host's side in a temp directory: the push, the checkout and the
    check all take the code paths of a real lane.
    """

    def setUp(self):
        super().setUp()
        # gate_plan.py reads Rust through `pipeline.crate_root` (GH #1162)
        for rel in ("lane_sync.sh", "gate_plan.py",
                    "pipeline/__init__.py", "pipeline/crate_root.py"):
            os.makedirs(os.path.dirname(self._scripts(rel)), exist_ok=True)
            shutil.copy2(SCRIPTS / rel, self._scripts(rel))
        kit_tree.seed(self._scripts(""), "lane_sync.sh")
        kit_tree.library(self._scripts(""), "cargo-target.sh")
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", "kit")
        tmp = self._tmp.name
        self.lane_root = os.path.join(tmp, "lane")
        os.makedirs(self.lane_root)
        self.cargo_log = os.path.join(tmp, "cargo.log")
        os.makedirs(os.path.join(tmp, "bin"), exist_ok=True)
        fake_ssh.install(os.path.join(tmp, "bin"), FAKE_SSH_TEMPLATE)
        write(tmp, "bin/cargo",
              '#!/bin/sh\necho "cargo $* pwd=$PWD target=$CARGO_TARGET_DIR jobs=$CARGO_BUILD_JOBS'
              ' incr=$CARGO_INCREMENTAL rustflags=$RUSTFLAGS" >>%s\n' % self.cargo_log, 0o755)
        self.env = dict(os.environ, HOME=tmp,
                        PATH=os.path.join(tmp, "bin") + os.pathsep + os.environ.get("PATH", ""),
                        MECLAW_LANE_ROOT=self.lane_root, MECLAW_LANE_THREADS="8",
                        MECLAW_LANE_LINKER="mold", MECLAW_LANE_CARGO="1",
                        MECLAW_LANE_FLOOR_G="0", FAKE_SSH_LOG=os.path.join(tmp, "ssh.log"))
        for k in ("MECLAW_LANE_SLOT", "MECLAW_LANE_SLOTS", "MECLAW_LANE_CLASS",
                  "MECLAW_LANE_SCCACHE", "MECLAW_LANE_WIPE_OK"):
            self.env.pop(k, None)
        self.seen = []

    def _scripts(self, rel):
        os.makedirs(os.path.join(self.root, "scripts"), exist_ok=True)
        return os.path.join(self.root, "scripts", rel)

    def exec_(self, argv, env=None, **kw):
        self.seen.append(argv)
        if len(argv) > 1 and argv[1] == "test":
            # the test binaries: `lane_sync.sh test` is W1's, tested there
            return subprocess.CompletedProcess(argv, 0, "", "")
        # pre's own environment is the process's; only its hold travels here
        hold = (env or {}).get("MECLAW_LANE_HOLD")
        return subprocess.run(argv, env=dict(self.env, **({"MECLAW_LANE_HOLD": hold} if hold else {})),
                              **kw)

    def popen(self, argv, **kw):
        return subprocess.Popen(argv, env=self.env, **kw)

    def runner(self):
        run = pre.ssh_runner("u@lane", "w-s1", self.root, exec_=self.exec_, popen=self.popen)
        self.addCleanup(run.release)
        return run

    def hold_tree(self):
        """A gate's `lane.hold` on the build path: `<tree>.lock`, held until cleanup."""
        lock = os.path.join(self.lane_root, "tree.lock")
        holder = subprocess.Popen(["bash", "-c", 'exec 9>>"$1" && flock 9 && echo held && exec cat >/dev/null',
                                   "hold", lock], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        self.addCleanup(holder.stdout.close)
        self.addCleanup(holder.wait)
        self.addCleanup(holder.stdin.close)
        self.assertEqual(b"held\n", holder.stdout.readline())
        return lock

    def test_pre_never_pushes_into_a_build_path_a_gate_holds(self):
        # Delta C I-neu-1: the gate holds `<tree>.lock` from push to fetch;
        # pre takes the same lock (no token skip bypasses it) -- busy is exit 5.
        self.hold_tree()
        write(self.root, "crates/meclaw-demo/src/lib.rs", "pub fn changed() {}\n")
        rc, text = self.runner()(["cargo", "check", "--tests", "-p", "meclaw-demo"])
        self.assertEqual(5, rc, text)
        self.assertIn("in use", text)
        self.assertEqual([], self.seen, "nothing travels under a held build path")
        self.assertFalse(os.path.exists(os.path.join(self.lane_root, "tree")))
        self.assertFalse(os.path.exists(self.cargo_log))

    def test_the_unittest_half_never_inherits_the_callers_repository(self):
        # pd-1155 kit find: `strand.sh pre` with GIT_DIR/GIT_WORK_TREE set (the
        # recipe without cd) handed both to the unittest station -- 121 errors,
        # the test repositories acted on the caller's tree.
        seen = []

        real = subprocess.run

        def run(argv, **kw):
            if argv[0] == "git":    # the config guard's reads
                return real(argv, **kw)
            seen.append(kw.get("env"))
            return subprocess.CompletedProcess(argv, 0, "", UT_OK)

        with mock.patch.dict(os.environ, {"GIT_DIR": "/elsewhere/.git", "GIT_WORK_TREE": "/elsewhere"}), \
                mock.patch.object(pre.subprocess, "run", side_effect=run):
            res = pre.unittest_half(["scripts.tests.test_x"], self.root)
        self.assertEqual("GREEN", res[0].verdict)
        self.assertIsNotNone(seen[0], "the unittest half passes no environment of its own")
        self.assertNotIn("GIT_DIR", seen[0])
        self.assertNotIn("GIT_WORK_TREE", seen[0])
        self.assertEqual(os.environ.get("PATH"), seen[0].get("PATH"))
        for name in ("GIT_INDEX_FILE", "GIT_COMMON_DIR"):
            self.assertIn(name, pre.gitenv.GIT_REPO_VARS)

    def test_a_unittest_half_that_changes_the_repository_config_is_red(self):
        # pd-1155: the guard compares the repository config before and after.
        config = os.path.join(self.root, ".git", "config")
        real = subprocess.run

        def run(argv, **kw):
            if argv[0] == "git":    # the guard's own reads
                return real(argv, **kw)
            with open(config, "a") as fh:
                fh.write("[core]\n\tworktree = /elsewhere\n")
            return subprocess.CompletedProcess(argv, 0, "", UT_OK)

        with mock.patch.object(pre.subprocess, "run", side_effect=run):
            res = pre.unittest_half(["scripts.tests.test_x"], self.root)
        self.assertEqual("RED", res[0].verdict)
        self.assertIn("changed the repository config", res[0].text)
        self.assertIn("+core.worktree=/elsewhere", res[0].text)

    def _half_that_writes(self, text, exc=None):
        config = os.path.join(self.root, ".git", "config")
        real = subprocess.run

        def run(argv, **kw):
            if argv[0] == "git":    # the guard's own reads
                return real(argv, **kw)
            with open(config, "a") as fh:
                fh.write(text)
            if exc:
                raise exc
            return subprocess.CompletedProcess(argv, 0, "", UT_OK)

        with mock.patch.object(pre.subprocess, "run", side_effect=run):
            return pre.unittest_half(["scripts.tests.test_x"], self.root)

    def test_a_branch_key_beside_the_half_is_a_note(self):
        # Re-check pd-legacy m-4: `branch.*`/`remote.*` are written by a
        # parallel `git push -u`; a NOTE with the key, never RED.
        res = self._half_that_writes('[branch "x"]\n\tremote = origin\n')
        self.assertEqual(["GREEN", "NOTE"], [r.verdict for r in res])
        self.assertIn("+branch.x.remote=origin", res[1].text)
        self.assertEqual("GREEN", pre.summarize(res, "0" * 40, 0)[2])

    def test_the_runners_imbalance_note_reaches_pre(self):
        # Re-check tb-s2 h2 m-N8: pre dropped the runner's SELFTEST-NOTE.
        line = ("shard imbalance max/mean 1.61 > 1.3 (73.6/42.5/40.9/44.6 s) -- is "
                "scripts/pipeline/selftest_durations.json stale?")
        real = subprocess.run

        def run(argv, **kw):
            if argv[0] == "git":    # the guard's own reads
                return real(argv, **kw)
            return subprocess.CompletedProcess(
                argv, 0, "SELFTEST-NOTE %s\nSELFTEST-VERDICT GREEN 4/4 shards\n" % line, UT_OK)

        with mock.patch.object(pre.subprocess, "run", side_effect=run):
            res = pre.unittest_half(["scripts.tests.test_x"], self.root)
        self.assertEqual([("unittest", "GREEN"), ("unittest-balance", "NOTE")],
                         [(r.check, r.verdict) for r in res])
        self.assertEqual(line, res[1].text)
        self.assertEqual("GREEN", pre.summarize(res, "0" * 40, 0)[2])

    def test_an_interrupted_half_still_names_the_drift(self):
        # Re-check pd-legacy m-2: no `finally` around the run.
        err = io.StringIO()
        with contextlib.redirect_stderr(err), self.assertRaises(KeyboardInterrupt):
            self._half_that_writes("[core]\n\tworktree = /elsewhere\n", KeyboardInterrupt())
        self.assertIn("PRE unittest RED -- the suite changed the repository config", err.getvalue())
        self.assertIn("+core.worktree=/elsewhere", err.getvalue())

    def test_pre_release_never_waits_unbounded_on_the_hold(self):
        # Z5: release() waited on the hold's shell without a timeout -- a
        # lane that keeps the ssh open kept `strand.sh pre` with it.
        waits, made = [], []

        class Stuck:
            def __init__(self, *a, **kw):
                made.append(self)
                self.kw = kw
                self.pid = 4242
                self.stdin = io.StringIO()
                self.stdout = io.StringIO("held n-1\n")
                self.stderr = io.StringIO()
                self.returncode = None
                self.killed = False

            def wait(self, timeout=None):
                waits.append(timeout)
                if timeout is None and not self.killed:
                    raise AssertionError("release waits unbounded")
                if not self.killed:
                    raise subprocess.TimeoutExpired("hold", timeout)
                return -9

            def kill(self):
                raise AssertionError("release kills the ssh alone, not its process group")

        killpg = []

        def fake_killpg(pid, sig):
            killpg.append((pid, sig))
            made[0].killed = True

        run = pre.ssh_runner("u@lane", "w-s1", self.root, exec_=lambda *a, **kw: None, popen=Stuck)
        # take() is the runner's own hold path (a closure of `run`)
        take = [c.cell_contents for c in run.__closure__ if callable(getattr(c, "cell_contents", None))
                and getattr(c.cell_contents, "__name__", "") == "take"]
        self.assertTrue(take, "ssh_runner has no take()")
        self.assertEqual(0, take[0]()[0])
        err = io.StringIO()
        with contextlib.redirect_stderr(err), mock.patch.object(pre.os, "killpg", fake_killpg):
            run.release()
        self.assertTrue(waits and waits[0] is not None, waits)
        # Review Z-A R3 M1: the hold runs in its own process group (the ssh
        # child goes with the kill), the kill is waited for and said.
        self.assertTrue(made[0].kw.get("start_new_session"), made[0].kw)
        # Review Z-A2 R3 M5: the kill goes to the group, not to the ssh alone.
        self.assertEqual([(4242, pre.signal.SIGKILL)], killpg)
        self.assertEqual(2, len(waits), waits)
        self.assertIn("hold not released in 30 s, killed", err.getvalue())

    def test_pre_holds_the_build_path_from_push_to_its_last_step(self):
        lock = os.path.join(self.lane_root, "tree.lock")
        write(self._tmp.name, "bin/cargo",
              '#!/bin/sh\nflock -n %s true && echo free >>%s || echo held >>%s\n'
              % (lock, self.cargo_log, self.cargo_log), 0o755)
        run = self.runner()
        rc, text = run(["cargo", "check", "--tests", "-p", "meclaw-demo"])
        self.assertEqual(0, rc, text)
        with open(self.cargo_log, encoding="utf-8") as fh:
            self.assertEqual(["held"], fh.read().split())
        run.release()
        self.assertEqual(0, subprocess.run(["flock", "-n", lock, "true"]).returncode)

    def test_a_bare_push_takes_the_lock_and_a_false_claim_is_refused(self):
        sync = os.path.join(self.root, "scripts", "lane_sync.sh")
        res = subprocess.run([sync, "push", "u@lane", "w-s1"], env=self.env, cwd=self.root,
                             capture_output=True, text=True)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual(0, subprocess.run(["flock", "-n", os.path.join(self.lane_root, "tree.lock"),
                                            "true"]).returncode, "the push let go of its lock")
        self.hold_tree()
        for claim in (None, "not-the-holder"):
            env = dict(self.env, **({"MECLAW_LANE_HOLD": claim} if claim else {}))
            res = subprocess.run([sync, "push", "u@lane", "w-s1"], env=env, cwd=self.root,
                                 capture_output=True, text=True)
            self.assertEqual(5, res.returncode, (claim, res.stderr))

    def test_check_runs_in_the_lane_build_path_after_the_real_push(self):
        write(self.root, "crates/meclaw-demo/src/lib.rs", "pub fn a() {}\npub fn b() {}\n")
        run = self.runner()
        rc, text = run(["cargo", "check", "--tests", "-p", "meclaw-demo"])
        self.assertEqual(0, rc, text)
        sync = os.path.join(self.root, "scripts", "lane_sync.sh")
        self.assertEqual([sync, "push", "u@lane", "w-s1"], self.seen[0])
        self.assertEqual([sync, "check", "u@lane", "w-s1", "--tests", "-p", "meclaw-demo"],
                         self.seen[1])
        tree = os.path.join(self.lane_root, "tree")
        target = os.path.join(self.lane_root, "target")
        # the overlay reached the build path the check ran in
        with open(os.path.join(tree, "crates/meclaw-demo/src/lib.rs"), encoding="utf-8") as fh:
            self.assertIn("pub fn b()", fh.read())
        self.assertFalse(os.path.exists(os.path.join(self.lane_root, "wt", "w-s1")))
        with open(os.path.join(target, ".gate-tree"), encoding="utf-8") as fh:
            self.assertEqual(tree, fh.readline().strip())
        with open(self.cargo_log, encoding="utf-8") as fh:
            lines = fh.read().splitlines()
        self.assertEqual(1, len(lines), lines)
        self.assertTrue(lines[0].startswith(
            "cargo check --tests -p meclaw-demo pwd=%s target=%s jobs=8 incr=1 " % (tree, target)),
            lines[0])
        # the lane's environment (lane_env), not a copy of its width: the linker travels
        self.assertIn("-fuse-ld=mold", lines[0])

    def test_check_refuses_below_the_floor(self):
        self.env["MECLAW_LANE_FLOOR_G"] = "999999999"
        rc, text = self.runner()(["cargo", "check", "--tests", "-p", "meclaw-demo"])
        self.assertEqual(2, rc, text)
        self.assertIn("floor", text)
        self.assertFalse(os.path.exists(self.cargo_log))

    def _fake_hygiene(self, rc):
        log = os.path.join(self._tmp.name, "hygiene.log")
        write(self.root, "scripts/pipeline/hygiene.py",
              "import os, sys\n"
              "with open(%r, 'a') as fh:\n"
              "    fh.write(' '.join(sys.argv[1:]) + '\\n')\n"
              "sys.exit(%d)\n" % (log, rc))
        return log

    def test_check_runs_the_lane_hygiene_before_the_floor(self):
        # W2: hygiene first (collect/evict, the run's own target protected),
        # then the floor judges what is left -- the same order as `run`
        log = self._fake_hygiene(0)
        rc, text = self.runner()(["cargo", "check", "--tests", "-p", "meclaw-demo"])
        self.assertEqual(0, rc, text)
        with open(log, encoding="utf-8") as fh:
            calls = fh.read().splitlines()
        self.assertEqual(1, len(calls), calls)
        argv = calls[0].split()
        self.assertEqual("check", argv[0])
        target = os.path.join(self.lane_root, "target")
        self.assertEqual(target, argv[argv.index("--own-target") + 1])
        self.assertEqual(os.path.join(self.lane_root, "tree"), argv[argv.index("--own-tree") + 1])
        self.assertEqual("0", argv[argv.index("--abort-below-g") + 1])
        self.assertTrue(os.path.exists(self.cargo_log))

    def test_check_stops_when_the_hygiene_finds_no_room(self):
        self._fake_hygiene(2)
        rc, text = self.runner()(["cargo", "check", "--tests", "-p", "meclaw-demo"])
        self.assertEqual(6, rc, text)
        self.assertIn("ABORTED", text)
        self.assertFalse(os.path.exists(self.cargo_log))

    def test_test_binaries_keep_no_fail_fast(self):
        rc, text = self.runner()(["cargo", "nextest", "run", "--no-fail-fast", "-p", "meclaw-demo",
                                  "--test", "gh1_case"])
        self.assertEqual(0, rc, text)
        sync = os.path.join(self.root, "scripts", "lane_sync.sh")
        self.assertEqual([sync, "test", "u@lane", "w-s1", "binary_id(=meclaw-demo::gh1_case)",
                          "--no-fail-fast", "--test", "gh1_case"], self.seen[-1])

    def test_no_test_pair_never_makes_an_empty_filterset(self):
        for argv in (["cargo", "nextest", "run", "--no-fail-fast", "-p", "meclaw-demo"],
                     ["cargo", "nextest", "run", "--no-fail-fast", "--test", "gh1_case"]):
            self.seen = []
            run = self.runner()
            rc, text = run(argv)
            run.release()  # lane_half lets go of the build path after its last step
            self.assertNotEqual(0, rc, argv)
            self.assertIn("no (-p, --test) pair", text)
            self.assertFalse([a for a in self.seen if len(a) > 1 and a[1] == "test"], self.seen)


class Stamp(Fixture):
    def test_pre_writes_stamp_with_tree_hash(self):
        write(self.root, "docs/a.md", "# a\n\nchanged\n")
        doc, _ = self.run_pre()
        with open(os.path.join(self.root, ".git", pre.STAMP), encoding="utf-8") as fh:
            stamp = json.load(fh)
        # GH #1167: HEAD and the overlay digests travel for the code-tree check
        self.assertEqual({"tree_hash", "at", "verdict", "lane_half", "reasons", "head",
                          "overlay"}, set(stamp))
        self.assertEqual(pre.tree_hash(self.root), stamp["tree_hash"])
        self.assertEqual("GREEN", stamp["verdict"])
        self.assertTrue(stamp["at"].endswith("Z"))
        # the stamp is outside the tree: writing it does not change the hash
        self.assertEqual(stamp["tree_hash"], pre.tree_hash(self.root))
        ok, line = pre.stamp_status(self.root)
        self.assertTrue(ok, line)
        write(self.root, "docs/a.md", "# a\n\nchanged again\n")
        ok, line = pre.stamp_status(self.root)
        self.assertFalse(ok)
        # GH #1167: the refusal names the code path that changed
        self.assertEqual("gate: run strand.sh pre first (tree changed since %s: docs/a.md "
                         "(outside the allowlist))" % stamp["at"], line)


class Store(Fixture):
    """`pre` is a workpackage `mode: pre` in the W0 store, its metric `pipe_pre`."""

    def db(self):
        return store.open()

    def test_record_pre_is_one_transaction(self):
        # W4 re-check: a break before the last transition left a workpackage
        # `active` without pass_ended; now nothing of the pre-run stays.
        write(self.root, "docs/a.md", "# a\n\nchanged\n")
        real = store.item_transition

        def breaking(db, item, frm, to, *a, **kw):
            if to in ("finished", "stopped"):
                raise sqlite3.OperationalError("simulated break")
            return real(db, item, frm, to, *a, **kw)

        with mock.patch.object(store, "item_transition", breaking):
            self.run_pre(strand="w-tx")
        db = self.db()
        self.assertEqual(0, db.execute("SELECT count(*) FROM items WHERE type='workpackage'").fetchone()[0])

    def test_record_pre_writes_a_pre_workpackage_through_the_store(self):
        write(self.root, "docs/a.md", "# a\n\nchanged\n")
        doc, lines = self.run_pre(strand="w-s1")
        self.assertEqual("GREEN", doc["verdict"])
        self.assertFalse([l for l in lines if l.startswith("PRE store ")], lines)
        db = self.db()
        wps = [dict(r) for r in db.execute("SELECT * FROM items WHERE type='workpackage'")]
        self.assertEqual(1, len(wps))
        wp = store.item_get(db, wps[0]["id"])
        self.assertEqual("finished", wp["state"])
        self.assertEqual("pre", wp["doc"]["mode"])
        self.assertEqual("pre", wp["doc"]["stage"])
        self.assertEqual("w-s1", wp["doc"]["strand"])
        self.assertEqual(doc["tree_hash"], wp["doc"]["tree"])
        tasks = [dict(r) for r in db.execute(
            "SELECT * FROM items WHERE type='task' AND parent=?", (wp["id"],))]
        checks = sorted({c["check"] for c in doc["checks"]})
        self.assertEqual(checks, sorted(t["title"] for t in tasks))
        self.assertEqual({"done"}, {t["state"] for t in tasks})
        runs = db.execute("SELECT count(*) FROM task_runs WHERE item IN (%s)"
                          % ",".join("?" * len(tasks)), [t["id"] for t in tasks]).fetchone()[0]
        self.assertEqual(len(tasks), runs)
        # the journal alone gives the receipt of the pre-run
        rec = receipt.build(db, doc["run"])
        self.assertEqual("pre", rec["mode"])
        self.assertEqual("GREEN", rec["verdict"])
        self.assertEqual(checks, sorted(st["name"] for st in rec["stations"]))

    def test_a_red_unittest_station_keeps_its_tests_and_seed_in_the_store(self):
        # Kit find C4 (Welle Testing): the store held only `station_ended ...
        # verdict=RED`; name and seed went to the console or nowhere.
        write(self.root, "scripts/__init__.py", "")
        write(self.root, "scripts/tests/__init__.py", "")
        write(self.root, "scripts/tests/test_fx_red.py", UT_RED)
        with mock.patch.dict(os.environ, {"MECLAW_SELFTEST_SHARDS": "1",
                                          "MECLAW_SELFTEST_SEED": "42"}):
            res = pre.unittest_half(["scripts.tests.test_fx_red"], self.root)
        doc = {"at": "2026-10-12T10:00:00Z", "base": "HEAD", "tree_hash": "0" * 40,
               "verdict": "RED", "secs": 1.0, "strand": "w-ut",
               "checks": [r.as_json() for r in res]}
        db = self.db()
        run = pre.record_pre(db, doc, self.root)
        ended = [r["data"] for r in events.read_run(db, run) if r["event"] == "station_ended"]
        self.assertEqual(1, len(ended), ended)
        self.assertEqual("RED", ended[0]["verdict"])
        # Re-check tv-kit fix15 H3: a name list, not the receipt's count dict `tests`.
        self.assertEqual(["scripts.tests.test_fx_red.T.test_red"], ended[0]["red_tests"])
        self.assertNotIn("tests", ended[0])
        self.assertEqual(42, ended[0]["seed"])
        self.assertIn("red scripts.tests.test_fx_red.T.test_red", ended[0]["reason"])

    def test_a_red_unittest_station_without_a_name_is_marked_unexplained(self):
        # Re-check tv-kit fix15 H1: no parsable name -- the row says so.
        write(self.root, "scripts/__init__.py", "")
        write(self.root, "scripts/tests/__init__.py", "")
        write(self.root, "scripts/tests/test_fx_crash.py", UT_CRASH)
        with mock.patch.dict(os.environ, {"MECLAW_SELFTEST_SHARDS": "1"}):
            res = pre.unittest_half(["scripts.tests.test_fx_crash"], self.root)
        doc = {"at": "2026-10-12T10:00:00Z", "base": "HEAD", "tree_hash": "0" * 40,
               "verdict": "RED", "secs": 1.0, "strand": "w-ut",
               "checks": [r.as_json() for r in res]}
        db = self.db()
        run = pre.record_pre(db, doc, self.root)
        ended = [r["data"] for r in events.read_run(db, run) if r["event"] == "station_ended"]
        self.assertEqual("RED", ended[0]["verdict"])
        self.assertNotIn("red_tests", ended[0])
        self.assertTrue(ended[0]["reason"].startswith(flakes.UNEXPLAINED), ended[0]["reason"])

    def test_red_pre_stops_the_workpackage(self):
        if not shutil.which("shellcheck"):
            self.skipTest("shellcheck not installed")
        write(self.root, "scripts/a.sh", "#!/usr/bin/env bash\nset -u\necho \"$foo\"\n", 0o755)
        doc, _ = self.run_pre()
        self.assertEqual("RED", doc["verdict"])
        db = self.db()
        wp = dict(db.execute("SELECT * FROM items WHERE type='workpackage'").fetchone())
        self.assertEqual("stopped", wp["state"])
        states = {r["title"]: r["state"] for r in db.execute(
            "SELECT title, state FROM items WHERE type='task' AND parent=?", (wp["id"],))}
        self.assertIn("waiting", states.values())
        self.assertEqual("RED", receipt.build(db, doc["run"])["verdict"])

    def test_store_failure_never_blocks_pre(self):
        with mock.patch.dict(os.environ, {"MECLAW_PIPELINE_DB": self.root}):  # a directory
            doc, lines = self.run_pre()
        self.assertEqual("GREEN", doc["verdict"])
        self.assertTrue([l for l in lines if l.startswith("PRE store 0s NOTE store write failed")],
                        lines)

    def test_pipe_pre_goes_through_the_w0_emitter(self):
        sent = []

        class Emitter:
            @staticmethod
            def url():
                return "http://metrics.invalid/write?db=devtempo"

            @staticmethod
            def line(measurement, tags, fields, at):
                return (measurement, tuple(tags), tuple(fields), at)

            @staticmethod
            def post(target, lines):
                sent.append((target, lines))

        with mock.patch.object(pre, "_emitter", lambda: Emitter):
            doc, lines = self.run_pre(strand="w-s1")
        self.assertEqual(1, len(sent), lines)
        (measurement, tags, fields, at), = sent[0][1]
        self.assertEqual("pipe_pre", measurement)
        self.assertEqual((("repo", "meclaw"), ("strand", "w-s1"), ("verdict", "GREEN")), tags)
        self.assertEqual("secs", fields[0][0])
        self.assertEqual(doc["at"], at)

    def test_failed_push_is_a_metrics_failed_event(self):
        class Emitter:
            url = staticmethod(lambda: "http://user:pw@metrics.invalid/write?db=devtempo")
            line = staticmethod(lambda *a: "pipe_pre x")

            @staticmethod
            def post(target, lines):
                raise OSError("connection refused")

        with mock.patch.object(pre, "_emitter", lambda: Emitter):
            doc, _ = self.run_pre()
        self.assertEqual("GREEN", doc["verdict"])
        rows = [r for r in events.read_run(self.db(), doc["run"])
                if r["event"] == "metrics_failed"]
        self.assertEqual(1, len(rows))
        self.assertEqual("metrics.invalid", rows[0]["data"]["url_host"])
        self.assertNotIn("pw", json.dumps(rows[0]["data"]))

    def test_pipe_pre_goes_through_the_merged_w0_emitter(self):
        # no patched `_emitter`: since the W0b merge it is metrics.py itself
        from pipeline import metrics
        self.assertIs(metrics, pre._emitter())
        sent = []
        with mock.patch.object(metrics, "post", lambda target, lines: sent.append((target, lines))):
            # the fixture keeps the URL empty: nothing is pushed, nothing noted
            _, lines = self.run_pre(strand="w-s1")
            self.assertEqual([], sent)
            self.assertFalse([x for x in lines if x.startswith("PRE metrics ")], lines)
            # a target set: one pipe_pre line from metrics.line, posted (not pushed)
            with mock.patch.dict(os.environ, {metrics.URL_ENV: "http://metrics.invalid/write"}):
                doc, _ = self.run_pre(strand="w-s1")
        # the journal hook of W0b sends pipe_station/pipe_pass of the same run as well
        mine = [(tg, ls) for tg, ls in sent if ls[0].startswith("pipe_pre,")]
        self.assertEqual(1, len(mine), sent)
        target, (line,) = mine[0]
        self.assertEqual("http://metrics.invalid/write", target)
        self.assertEqual(metrics.line("pipe_pre", (("repo", "meclaw"), ("strand", "w-s1"),
                                                   ("verdict", "GREEN")),
                                      (("secs", doc["secs"]),), doc["at"]), line)
        self.assertTrue(line.startswith("pipe_pre,repo=meclaw,strand=w-s1,verdict=GREEN secs="),
                        line)

    def test_no_emitter_is_a_note_never_a_stub(self):
        with mock.patch.object(pre, "_emitter", lambda: None):
            _, lines = self.run_pre()
        self.assertIn("PRE metrics 0s NOTE pipe_pre not sent -- no W0 emitter "
                      "(scripts/pipeline/metrics.py)", lines)

    def test_cli_registers_pre(self):
        self.assertIn("pre", cli.MODULES)
        args = cli.parser().parse_args(["pre", "--repo", self.root, "--tree-hash"])
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.assertEqual(0, args.func(args))
        self.assertEqual(pre.tree_hash(self.root), out.getvalue().strip())


class Gate(unittest.TestCase):
    """`strand.sh gate` in a linked strand worktree with a fake runner."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        base = self._tmp.name
        self.main = os.path.join(base, "core")
        os.makedirs(self.main)
        for rel in ("strand.sh", "precheck.py", "gate_plan.py", "harness_fold.py",
                    # precheck imports the testing bans (tv-p0, #1207)
                    "testing_bans.py",
                    "pipeline/__init__.py", "pipeline/pre.py", "pipeline/locks.py", "pipeline/gitenv.py",
                    # pre runs the dry cascade (GH #1173)
                    "pipeline/cascade_dry.py", "pipeline/versions.py",
                    "pipeline/store.py", "pipeline/events.py", "pipeline/turnaround.py",
                    # gate_plan.py reads Rust through `pipeline.crate_root` (GH #1162)
                    "pipeline/crate_root.py"):
            dst = os.path.join(self.main, "scripts", rel)
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            shutil.copy2(SCRIPTS / rel, dst)
        kit_tree.seed(os.path.join(self.main, "scripts"), "strand.sh")
        write(self.main, "scripts/gate.sh", FAKE_GATE, 0o755)
        write(self.main, "plans/w-2026-01-01/README.md", "# w\n")
        write(self.main, "docs/a.md", "# a\n")
        init_repo(self.main)
        self.wt = os.path.join(base, "core-wt-s1")
        git(self.main, "worktree", "add", "-q", "-b", "w/s1", self.wt)
        self.env = dict(os.environ,
                        MECLAW_STRAND_TOKENS=os.path.join(base, "no-tokens"),
                        MECLAW_LANES_FILE=os.path.join(base, "no-lanes"),
                        MECLAW_PIPELINE_DB=os.path.join(base, "pipeline.db"),
                        MECLAW_PIPELINE_METRICS_URL="",
                        # GH #1167 (f): the owner of `master..HEAD` (init_repo's identity)
                        MECLAW_OWNER="t <t@example.invalid>")
        self.env.pop("MECLAW_STRAND_TOKEN_SKIP", None)

    def tearDown(self):
        self._tmp.cleanup()

    def strand(self, *args, cwd=None):
        return subprocess.run(["bash", os.path.join(cwd or self.wt, "scripts", "strand.sh")]
                              + list(args), cwd=cwd or self.wt, env=self.env,
                              capture_output=True, text=True)

    def test_pre_hands_the_lane_settings_to_the_transport(self):
        # Review I2: pre's lane half calls `lane_sync.sh` itself, so the lane's
        # settings from the host file (class, slots, width, linker ...) and the
        # cargo build path must reach it as they reach a gate's transport.
        with open(self.env["MECLAW_LANES_FILE"], "w", encoding="utf-8") as fh:
            fh.write("north  builder@north.example.invalid  threads=6 linker=mold slots=2\n")
        write(self.wt, "scripts/pipeline/pre.py",
              "import os, sys\n"
              "keys = ('MECLAW_LANE_NAME', 'MECLAW_LANE_THREADS', 'MECLAW_LANE_LINKER',\n"
              "        'MECLAW_LANE_SLOTS', 'MECLAW_LANE_CARGO')\n"
              "print(' '.join('%s=%s' % (k, os.environ.get(k, '')) for k in keys))\n"
              "print(' '.join(sys.argv[1:]))\n")
        res = self.strand("pre", "--host", "north")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        envline, argline = res.stdout.splitlines()[:2]
        self.assertEqual("MECLAW_LANE_NAME=north MECLAW_LANE_THREADS=6 MECLAW_LANE_LINKER=mold "
                         "MECLAW_LANE_SLOTS=2 MECLAW_LANE_CARGO=1", envline)
        self.assertIn("--lane north --lane-target builder@north.example.invalid", argline)

    def test_gate_refuses_without_fresh_stamp(self):
        res = self.strand("gate", "--wave", "w-2026-01-01")
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        self.assertIn("gate: run strand.sh pre first (no stamp)", res.stderr)
        self.assertNotIn("GATE-SUMMARY", res.stdout)
        # a stamp of another tree is no stamp
        self.assertEqual(0, self.strand("pre", "--no-lane").returncode)
        write(self.wt, "docs/a.md", "# a\n\nlater\n")
        res = self.strand("gate", "--wave", "w-2026-01-01")
        self.assertEqual(2, res.returncode)
        # GH #1167: the refusal names the code path that changed
        self.assertRegex(res.stderr, r"gate: run strand\.sh pre first \(tree changed since "
                                     r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ: docs/a\.md ")

    def test_gate_accepts_stamp_within_24h_same_tree(self):
        write(self.wt, "docs/a.md", "# a\n\nnew\n")
        res = self.strand("pre", "--no-lane")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertRegex(res.stdout.splitlines()[-1], SUMMARY)
        res = self.strand("gate", "--wave", "w-2026-01-01")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("GATE-SUMMARY strand", res.stdout)
        # older than 24 h: refused again
        path = os.path.join(pre.git_dir(self.wt), pre.STAMP)
        with open(path, encoding="utf-8") as fh:
            stamp = json.load(fh)
        old = datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(hours=25)
        stamp["at"] = old.strftime("%Y-%m-%dT%H:%M:%SZ")
        with open(path, "w", encoding="utf-8") as fh:
            json.dump(stamp, fh)
        res = self.strand("gate", "--wave", "w-2026-01-01")
        self.assertEqual(2, res.returncode)
        self.assertIn("older than 24 h", res.stderr)

    def commit_in_wt(self, rel, text):
        write(self.wt, rel, text)
        git(self.wt, "add", "-A")
        git(self.wt, "commit", "-q", "-m", "after pre")

    def test_gate_keeps_the_stamp_over_recheck_scripts_after_pre(self):
        # GH #1199 (e): a re-check writes its class scripts after pre; the
        # gate's stamp check reads the ONE post-gate list (`POST_GATE`).
        self.assertEqual(0, self.strand("pre", "--no-lane").returncode)
        rdir = "plans/w-2026-01-01/recheck/skripte/s1/"
        self.commit_in_wt(rdir + "authors_s1.py", "import sys\nsys.exit(0)\n")
        write(self.wt, rdir + "run_all.sh", "#!/bin/sh\nexit 0\n")
        write(self.wt, "plans/w-2026-01-01/RESTLISTE.md", "# Restliste\n")
        res = self.strand("gate", "--wave", "w-2026-01-01")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertNotIn("run strand.sh pre first", res.stderr)
        self.assertIn("GATE-SUMMARY strand", res.stdout)

    def test_gate_refuses_code_after_pre_beside_recheck_scripts(self):
        self.assertEqual(0, self.strand("pre", "--no-lane").returncode)
        self.commit_in_wt("plans/w-2026-01-01/recheck/skripte/s1/authors_s1.py", "X = 1\n")
        for rel in ("scripts/later.py", "plans/PREAMBLE.md"):
            with self.subTest(rel=rel):
                write(self.wt, rel, "later\n")
                res = self.strand("gate", "--wave", "w-2026-01-01")
                self.assertEqual(2, res.returncode, res.stdout + res.stderr)
                self.assertIn("run strand.sh pre first (tree changed since", res.stderr)
                self.assertIn(rel, res.stderr)

    def test_gate_no_pre_requires_reason_and_records_it(self):
        res = self.strand("gate", "--wave", "w-2026-01-01", "--no-pre")
        self.assertEqual(2, res.returncode)
        self.assertIn("--no-pre needs --reason", res.stderr)
        res = self.strand("gate", "--wave", "w-2026-01-01", "--no-pre",
                          "--reason", "lane down, owner said go")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("gate: pre skipped -- lane down, owner said go", res.stdout)
        latest = os.path.join(self.main, "plans", "w-2026-01-01", "receipts", "s1", "latest")
        with open(os.path.join(latest, "receipt.json"), encoding="utf-8") as fh:
            receipt = json.load(fh)
        self.assertEqual("lane down, owner said go", receipt["pre_skipped"])
        self.assertEqual("pre_skipped", receipt["owner_decisions"][-1]["decision"])

    def test_gate_needs_no_stamp_for_passes(self):
        # an integration pass from the strand worktree, and any gate in the main tree
        res = self.strand("gate", "integration", "--wave", "w-2026-01-01")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        git(self.main, "checkout", "-q", "-b", "x/main-pass")
        res = self.strand("gate", "--wave", "w-2026-01-01", "--strand", "m", cwd=self.main)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)


if __name__ == "__main__":
    unittest.main()
