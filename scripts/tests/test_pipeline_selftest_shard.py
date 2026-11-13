"""`scripts/pipeline/selftest_shard.py`: gate-selftest in shards (GH #1126).

Fail-closed: every module runs exactly once, a red or crashed shard makes the
station red and names its log, shards share no TMPDIR and no state, and one
shard is the plain `python3 -m unittest` of before.
"""

import contextlib
import io
import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile
import textwrap
import types
import unittest
from unittest import mock

from scripts.tests import kit_time, private_tree, store_isolation

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
REPO = SCRIPTS.parent
sys.path.insert(0, str(SCRIPTS))

from pipeline import lane, plan, register, selftest_shard  # noqa: E402

RUNNER = str(SCRIPTS / "pipeline" / "selftest_shard.py")

# gate-selftest loads every module into one process: no fixture event of this
# module may reach the metrics target of the caller's environment.
_METRICS_URL = mock.patch.dict(os.environ, {"MECLAW_PIPELINE_METRICS_URL": ""})


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()
    _METRICS_URL.start()


def tearDownModule():  # noqa: N802 -- unittest's name
    _METRICS_URL.stop()
    store_isolation.restore()


GREEN_CASE = '''import unittest


class T(unittest.TestCase):
    def test_one(self):
        self.assertTrue(True)

    def test_two(self):
        self.assertTrue(True)
'''

RED_CASE = '''import unittest


class T(unittest.TestCase):
    def test_red(self):
        self.assertEqual(1, 2)
'''

CRASH_CASE = '''import os
import unittest


class T(unittest.TestCase):
    def test_crash(self):
        os._exit(3)
'''

# Writes what the shard's process sees of its own environment.
SPY_CASE = '''import json
import os
import tempfile
import unittest

NAMES = %r


class T(unittest.TestCase):
    def test_spy(self):
        seen = {n: os.environ.get(n) for n in NAMES}
        seen["gettempdir"] = tempfile.gettempdir()
        out = os.path.join(os.environ["SPY_OUT"], __name__ + ".json")
        with open(out, "w") as fh:
            json.dump(seen, fh)
'''


# Changes the repository config of the fixture root (pd-1155 kit find).
CONFIG_CASE = '''import subprocess
import unittest


class T(unittest.TestCase):
    def test_config(self):
        subprocess.run(["git", "config", "user.name", "t"], check=True)
'''


# Commits on the branch of the fixture root (re-check pd-legacy m-3), and a
# key a parallel git writes legitimately (m-4).
COMMIT_CASE = CONFIG_CASE.replace('["git", "config", "user.name", "t"]',
                                  '["git", "commit", "-q", "--allow-empty", "-m", "first"]')
BRANCH_KEY_CASE = CONFIG_CASE.replace('"user.name", "t"', '"branch.master.remote", "origin"')
GIT_ENV = {"GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@example.invalid",
           "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@example.invalid",
           "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1"}


class ShardCase(unittest.TestCase):
    """A fixture root with a package `fx` of test modules, and the runner in it."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self._tmp.name)
        (self.root / "fx").mkdir()
        (self.root / "fx" / "__init__.py").write_text("")
        self.logs = self.root / "logs"

    def tearDown(self):
        self._tmp.cleanup()

    def module(self, name, text):
        (self.root / "fx" / (name + ".py")).write_text(text)
        return "fx." + name

    def init_root(self):
        env = dict({k: v for k, v in os.environ.items() if not k.startswith("GIT_")}, **GIT_ENV)
        for argv in (["init", "-q", "-b", "master"], ["commit", "-q", "--allow-empty", "-m", "0"]):
            subprocess.run(["git", "-C", str(self.root)] + argv, check=True, env=env)

    def run_runner(self, args, shards, stdout=subprocess.PIPE, extra=None):
        if isinstance(shards, int) and shards > selftest_shard._nproc():
            self.skipTest("the runner starts at most nproc shards, %d wanted" % shards)
        env = dict(os.environ)
        env.update({plan.SELFTEST_SHARDS_ENV: str(shards),
                    selftest_shard.LOG_DIR_ENV: str(self.logs),
                    selftest_shard.DURATIONS_ENV: str(self.root / "no-table.json")})
        env.update(extra or {})
        return subprocess.run(lane.python_argv(RUNNER, *args), cwd=self.root, env=env,
                              stdout=stdout, stderr=subprocess.STDOUT, text=True,
                              stdin=subprocess.DEVNULL, timeout=4 * kit_time.HANG)


class TestTheRealPrecheckIsNoScenario(unittest.TestCase):
    """Re-check tv-bn M7q M2: a scenario station is one whose SCRIPT is a
    `run_*scenarios.py` (`lane.python_rest(argv)[0]`), never one that names such a
    file among its arguments. `precheck --files ... run_builder_scenarios.py` went
    to the scenario triage, read `last_run.json` beside the changed file and was
    re-run with `--only`. Against the stations gate_plan really plans."""

    CHANGED = ["scripts/gate_plan.py",
               "workshop/evals/builder-scenarios/run_builder_scenarios.py",
               "templates/display/compose/scenarios/run_display_scenarios.py",
               "workshop/evals/scenarios/run_scenarios.py"]

    def test_precheck_is_no_scenario_and_the_runners_are(self):
        import gate_plan
        from pipeline import flakes
        for mode in ("strand", "integration"):
            stations = {s.name: s for s in gate_plan.plan(self.CHANGED, mode, repo=None)}
            with self.subTest(mode=mode):
                pre = stations["precheck"].cmds[0]
                self.assertTrue(any(flakes._SCENARIO_RUNNER.search(a) for a in pre), pre)
                self.assertIsNone(flakes.station_kind({}, pre))
                for name in ("scenarios:builder", "scenarios:display", "scenarios:memory"):
                    runner = [c for c in stations[name].cmds
                              if any(flakes._SCENARIO_RUNNER.search(a) for a in c)][0]
                    self.assertEqual("scenario", flakes.station_kind({}, runner), name)
                    self.assertEqual(os.path.join("/w", os.path.dirname(lane.python_rest(runner)[0]),
                                                  "last_run.json"),
                                     flakes.scenario_runner_path(runner, "/w"))

    def test_an_env_prefixed_command_is_read_like_the_bare_one(self):
        """M7q H1: the station readers see past `env NAME=value`."""
        from pipeline import flakes
        argv = ["env", "SOUL2ME_LAB_SLOTS=4"] + lane.python_argv("-m", "unittest", "m")
        self.assertEqual("unittest", flakes.station_kind({}, argv))
        self.assertIn("tool:python", plan._inputs("s", {"cmds": [argv]}))
        self.assertIn("tool:python", register._inputs({"cmds": [{"argv": argv}]}))


class TestTheRealStationIsRecognised(unittest.TestCase):
    """Re-check tv-bn M7p M1: the readers of a station argv against the station
    gate_plan really plans -- not a fixture argv. `gate-selftest` starts in the kit's
    one Python (`/usr/bin/python3 -I`, two words); the readers took `python…` as the
    first word and `unittest` among the first three."""

    @classmethod
    def setUpClass(cls):
        import gate_plan
        cls.st = {s.name: s for s in gate_plan.plan(["scripts/gate_plan.py"], "strand",
                                                    repo=None)}["gate-selftest"]

    def test_the_station_starts_in_the_kits_python(self):
        self.assertEqual(list(lane.PYTHON), self.st.cmds[0][:len(lane.PYTHON)])

    def test_a_red_selftest_goes_to_the_unittest_triage(self):
        from pipeline import flakes
        argv = self.st.cmds[0]
        # None here: `executor._isolate` returns None, the red station ends as judged,
        # no T1/T3, no FLAKY class, no register entry (OR-TV-2) -- silently.
        self.assertEqual("unittest", flakes.station_kind({}, argv))
        self.assertEqual(argv + ["-k", "m.T.t"], flakes.unittest_isolation_argv("m.T.t", argv))

    def test_the_cache_key_of_the_station_carries_the_python_version(self):
        from pipeline import cache
        row = {"cmds": [list(c) for c in self.st.cmds]}
        inputs = plan._inputs("gate-selftest", row)
        self.assertIn("tool:python", inputs)
        st = {"name": "gate-selftest", "inputs": inputs}
        specs = cache._key_specs(st)
        old, new = ({"python": v} for v in ("3.12.3", "3.13.0"))
        self.assertNotEqual(cache._tools(old, specs), cache._tools(new, specs))


class TestDistribution(unittest.TestCase):
    """(a) every module exactly once, deterministic."""

    def assert_partition(self, shards, modules):
        flat = [m for s in shards for m in s]
        self.assertEqual(sorted(flat), sorted(set(modules)))
        self.assertEqual(len(flat), len(set(flat)), "a module ran twice")

    def test_round_robin_puts_every_module_in_exactly_one_shard(self):
        mods = ["m%02d" % i for i in range(23)]
        for n in (1, 2, 3, 8, 23, 40):
            shards = selftest_shard.distribute(mods, n)
            self.assert_partition(shards, mods)
            self.assertEqual(min(n, len(mods)), len(shards))
            self.assertEqual(shards, selftest_shard.distribute(list(reversed(mods)), n))

    def test_a_duration_table_balances_longest_first_and_keeps_unknown_modules(self):
        mods = ["a", "b", "c", "d", "e"]
        table = {"a": 100.0, "b": 60.0, "c": 50.0, "d": 10.0}
        shards = selftest_shard.distribute(mods, 2, table)
        self.assert_partition(shards, mods)
        # e weighs the median (60): a, b, e, c, d onto the lighter shard.
        self.assertEqual([["a", "c"], ["b", "d", "e"]], shards)

    def test_the_plan_of_the_real_station_names_every_module_once(self):
        import gate_plan
        st = {s.name: s for s in gate_plan.plan(["scripts/gate_plan.py"], "strand", repo=None)}
        argv = st["gate-selftest"].cmds[0]
        # M7p: the station starts the runner in the kit's one Python (`lane.PYTHON`)
        self.assertEqual(["/usr/bin/python3", "-I", "scripts/pipeline/selftest_shard.py", "unittest"], argv[:4])
        mods = argv[4:]
        shards = min(2, selftest_shard._nproc())
        env = dict(os.environ, **{plan.SELFTEST_SHARDS_ENV: str(shards)})
        out = subprocess.run(argv[:2] + [RUNNER, "--plan-only"] + argv[3:], cwd=REPO,
                             env=env, capture_output=True, text=True, timeout=2 * kit_time.HANG)
        self.assertEqual(0, out.returncode, out.stderr)
        lines = out.stdout.splitlines()
        named = [m for ln in lines if ln.startswith("SHARD ") for m in ln.split()[5:]]
        self.assertEqual(sorted(mods), sorted(named))
        self.assertEqual(len(named), len(set(named)))
        self.assertTrue(lines[-1].startswith("SHARD-PLAN shards=%d modules=%d "
                                             % (shards, len(mods))),
                        lines[-1])


class TestPart(unittest.TestCase):
    """The part mode (`MECLAW_SELFTEST_PART=<k>/<n>`): the public CI runs the
    station as n jobs; the parts are disjoint and together every module."""

    def test_the_parts_are_disjoint_and_together_every_unit(self):
        mods = ["m%02d" % i for i in range(23)]
        table = {m: float(i % 7 + 1) for i, m in enumerate(mods[:20])}
        for weights in (None, table):
            for n in (2, 3, 30):
                parts = [selftest_shard.part_units(mods, k, n, weights) for k in range(1, n + 1)]
                flat = [u for p in parts for u in p]
                self.assertEqual(sorted(mods), sorted(flat), (n, weights is None))
                self.assertEqual(len(flat), len(set(flat)), "a unit in two parts")

    def test_a_malformed_part_is_a_usage_error(self):
        self.assertIsNone(selftest_shard.parse_part(None))
        self.assertEqual((2, 3), selftest_shard.parse_part("2/3"))
        for bad in ("", "0/3", "4/3", "1/0", "a/b", "1"):
            with self.assertRaisesRegex(ValueError, "MECLAW_SELFTEST_PART"):
                selftest_shard.parse_part(bad)
        env = dict(os.environ, **{selftest_shard.PART_ENV: "4/3"})
        out = subprocess.run(lane.python_argv(RUNNER, "--plan-only", "unittest", "x"),
                             cwd=REPO, env=env, capture_output=True, text=True,
                             timeout=kit_time.HANG)
        self.assertEqual(2, out.returncode, out.stdout + out.stderr)

    def test_the_parts_of_the_real_station_name_every_module_once(self):
        import gate_plan
        st = {s.name: s for s in gate_plan.plan(["scripts/gate_plan.py"], "ci", repo=None)}
        argv = st["gate-selftest"].cmds[0]
        mods = argv[4:]
        units = []
        for k in (1, 2, 3):
            env = dict(os.environ, **{plan.SELFTEST_SHARDS_ENV: "2",
                                      selftest_shard.PART_ENV: "%d/3" % k})
            out = subprocess.run(argv[:2] + [RUNNER, "--plan-only"] + argv[3:], cwd=REPO,
                                 env=env, capture_output=True, text=True,
                                 timeout=2 * kit_time.HANG)
            self.assertEqual(0, out.returncode, out.stderr)
            lines = out.stdout.splitlines()
            self.assertIn("SHARD-PART %d/3 " % k, out.stdout)
            units += [u for ln in lines if ln.startswith("SHARD ") for u in ln.split()[5:]]
        self.assertEqual(len(units), len(set(units)), "a unit in two parts")
        self.assertEqual(sorted(mods), sorted({selftest_shard.module_of(u) if u not in mods
                                               else u for u in units}))


ORDER_CASE = '''import os
import unittest


class T(unittest.TestCase):
    def test_order(self):
        with open(os.environ["ORDER_OUT"], "a") as fh:
            fh.write(__name__ + "\\n")
'''


class TestShuffle(ShardCase):
    """TV-K4 (tv-p1c § 6): gate-selftest runs its modules in a random order and
    logs the seed (`SELFTEST-SEED <n>`); `MECLAW_SELFTEST_SEED` replays one.
    The shard assignment stays deterministic; only the order inside a shard moves."""

    def order(self, mods, shards, seed):
        out = pathlib.Path(tempfile.mkstemp(dir=self.root, prefix="order-")[1])
        res = self.run_runner(["unittest"] + mods, shards,
                              extra={"ORDER_OUT": str(out), selftest_shard.SEED_ENV: str(seed)})
        self.assertEqual(0, res.returncode, res.stdout)
        self.assertIn("SELFTEST-SEED %d" % seed, res.stdout)
        return res.stdout, out.read_text().split()

    def test_gate_selftest_shuffles_with_a_logged_seed(self):
        mods = [self.module("m%d" % i, ORDER_CASE) for i in range(6)]
        orders = {}
        for seed in range(1, 6):
            _, got = self.order(mods, 1, seed)
            self.assertEqual(sorted(got), sorted(mods))
            orders[seed] = got
        self.assertGreater(len({tuple(o) for o in orders.values()}), 1, orders)
        self.assertEqual(self.order(mods, 1, 3)[1], orders[3])   # the seed replays its order
        # Without a seed the runner draws one and names it.
        res = self.run_runner(["unittest"] + mods, 1, extra={"ORDER_OUT": str(self.root / "x")})
        self.assertRegex(res.stdout, r"SELFTEST-SEED \d+")
        # Two shards: every module once, the seed logged as well.
        _, got = self.order(mods, 2, 7)
        self.assertEqual(sorted(got), sorted(mods))
        # The isolation's `-k` call stays the exact command of before: no shuffle.
        res = self.run_runner(["unittest", "-k", "test_order"] + mods, 1,
                              extra={"ORDER_OUT": str(self.root / "k")})
        self.assertNotIn("SELFTEST-SEED", res.stdout)
        self.assertEqual((self.root / "k").read_text().split(), mods)


class TestVerdict(ShardCase):
    """(b) a red shard is a red station with its log; (e) a crash is never green."""

    def test_a_red_shard_makes_the_station_red_and_names_its_log(self):
        mods = [self.module("a_ok", GREEN_CASE), self.module("b_red", RED_CASE),
                self.module("c_ok", GREEN_CASE)]
        res = self.run_runner(["unittest"] + mods, 3, extra={selftest_shard.SEED_ENV: "11"})
        self.assertEqual(1, res.returncode, res.stdout)
        verdict = [ln for ln in res.stdout.splitlines() if ln.startswith("SELFTEST-VERDICT ")]
        # Kit find C4 (Welle Testing): the line, which the gate stores as the
        # station's reason, names the red tests and the seed of the order.
        self.assertEqual(["SELFTEST-VERDICT RED 2/3 shards; shard 2/3 1 failed -- log shard-2.log"
                          " -- red fx.b_red.T.test_red; seed 11"], verdict)
        log = self.logs / "shard-2.log"
        self.assertIn("FAIL: test_red (fx.b_red.T.test_red)", log.read_text())
        # The station log carries the red shard's output: the finding of #1150.
        self.assertIn("FAIL: test_red (fx.b_red.T.test_red)", res.stdout)
        self.assertIn("FAILED (errors=1)", res.stdout)
        self.assertNotIn("\nOK\n", res.stdout)

    def test_a_suite_that_changes_the_repository_config_is_red(self):
        # pd-1155: test repositories wrote core.worktree and user.name into the
        # shared config of the tree under test; the guard compares before/after.
        subprocess.run(["git", "init", "-q", str(self.root)], check=True,
                       env={k: v for k, v in os.environ.items() if not k.startswith("GIT_")})
        mods = [self.module("a_ok", GREEN_CASE), self.module("b_cfg", CONFIG_CASE)]
        res = self.run_runner(["unittest"] + mods, 2)
        self.assertEqual(1, res.returncode, res.stdout)
        self.assertRegex(res.stdout, r"(?m)^SELFTEST-VERDICT RED .*the suite changed the "
                                     r"repository config .*\+user\.name=t")

    def test_one_shard_that_changes_the_repository_config_is_red(self):
        # Re-check pd-legacy m-2: one shard (and `-k`) was `exec`, unguarded.
        self.init_root()
        res = self.run_runner(["unittest", self.module("b_cfg", CONFIG_CASE)], 1)
        self.assertEqual(1, res.returncode, res.stdout)
        self.assertRegex(res.stdout, r"(?m)^SELFTEST-VERDICT RED .*the suite changed the "
                                     r"repository config .*\+user\.name=t")

    def test_a_suite_that_commits_on_the_branch_is_red(self):
        # Re-check pd-legacy m-3: the leak's third harm, a commit "first".
        self.init_root()
        mods = [self.module("a_ok", GREEN_CASE), self.module("b_commit", COMMIT_CASE)]
        res = self.run_runner(["unittest"] + mods, 2, extra=GIT_ENV)
        self.assertEqual(1, res.returncode, res.stdout)
        self.assertRegex(res.stdout, r"(?m)^SELFTEST-VERDICT RED .*the suite moved HEAD of ")

    def test_a_branch_key_beside_the_run_is_a_note(self):
        # Re-check pd-legacy m-4: `git push -u` of a parallel strand writes
        # `branch.*` into the shared config -- a NOTE with the key, not RED.
        self.init_root()
        mods = [self.module("a_ok", GREEN_CASE), self.module("b_key", BRANCH_KEY_CASE)]
        res = self.run_runner(["unittest"] + mods, 2)
        self.assertEqual(0, res.returncode, res.stdout)
        self.assertIn("SELFTEST-VERDICT GREEN 2/2 shards", res.stdout)
        self.assertRegex(res.stdout, r"(?m)^SELFTEST-NOTE .*\+branch\.master\.remote=origin")

    def test_all_green_shards_are_a_green_station(self):
        mods = [self.module("a_ok", GREEN_CASE), self.module("b_ok", GREEN_CASE)]
        res = self.run_runner(["unittest"] + mods, 2)
        self.assertEqual(0, res.returncode, res.stdout)
        self.assertIn("Ran 4 tests in", res.stdout)
        self.assertIn("SELFTEST-VERDICT GREEN 2/2 shards", res.stdout)

    def test_shard_logs_sit_next_to_the_station_log(self):
        mods = [self.module("a_ok", GREEN_CASE), self.module("b_red", RED_CASE)]
        station_log = self.root / "run" / "strand-gate-selftest.log"
        station_log.parent.mkdir()
        with open(station_log, "w") as fh:
            res = self.run_runner(["unittest"] + mods, 2, stdout=fh)
        self.assertEqual(1, res.returncode)
        side = self.root / "run" / "strand-gate-selftest.shard-2.log"
        self.assertIn("FAIL: test_red", side.read_text())
        self.assertIn("-- log strand-gate-selftest.shard-2.log", station_log.read_text())

    def test_a_crashed_shard_is_red_never_green(self):
        mods = [self.module("a_ok", GREEN_CASE), self.module("b_crash", CRASH_CASE)]
        res = self.run_runner(["unittest"] + mods, 2)
        self.assertEqual(1, res.returncode, res.stdout)
        self.assertIn("shard 2/2 exit 3 without a unittest summary -- log shard-2.log",
                      res.stdout)
        self.assertIn("FAILED (errors=1)", res.stdout)

    def test_judge_is_fail_closed(self):
        ok = "Ran 3 tests in 0.1s\n\nOK\n"
        self.assertEqual("GREEN", selftest_shard.judge(0, ok)[0])
        self.assertEqual("RED", selftest_shard.judge(0, "")[0], "no summary")
        self.assertEqual("RED", selftest_shard.judge(1, ok)[0], "exit != 0 after OK")
        self.assertEqual("RED", selftest_shard.judge(-9, "Ran 3 tests in 0.1s\n")[0], "cut")
        self.assertEqual(("RED", 3, 2),
                         selftest_shard.judge(1, "Ran 3 tests in 0.1s\n\nFAILED "
                                                 "(failures=1, errors=1)\n")[:3])
        # Expected failures are no failures (flakes.unittest_summary).
        self.assertEqual(("RED", 3, 1),
                         selftest_shard.judge(1, "Ran 3 tests in 0.1s\n\nFAILED "
                                                 "(failures=1, expected failures=2)\n")[:3])


class TestIsolation(ShardCase):
    """(c) shards share no TMPDIR and no state path."""

    def test_a_shard_never_inherits_the_log_dir(self):
        # Re-check tb-s2 h2 m4: a runner inside a shard would write into this run's logs.
        # The PATH is built, not a literal: test_store_isolation forbids fixed search paths.
        path = store_isolation.trapped_path()
        env = selftest_shard.shard_env({selftest_shard.LOG_DIR_ENV: "/x", "PATH": path},
                                       str(self.root / "state"))
        self.assertNotIn(selftest_shard.LOG_DIR_ENV, env)
        self.assertEqual(path, env["PATH"])

    def test_the_part_reaches_no_shard(self):
        # CI v0.77.1 (run 41334566186): the selftest jobs ran with
        # MECLAW_SELFTEST_PART=k/3, the shards inherited it, and every kit test
        # that starts this runner (pre, strand.sh, this module) ran only part k
        # of its fixture -- `shards=2 not found ... shards=1`. The part is the
        # top run's, never a child's.
        mod = self.module("s0", SPY_CASE % ([selftest_shard.PART_ENV],))
        out = self.root / "spy"
        out.mkdir()
        res = self.run_runner(["unittest", mod], 1,
                              extra={"SPY_OUT": str(out), selftest_shard.PART_ENV: "1/1"})
        self.assertEqual(0, res.returncode, res.stdout)
        self.assertIn("SHARD-PART 1/1", res.stdout)
        seen = json.loads((out / (mod + ".json")).read_text())
        self.assertIsNone(seen[selftest_shard.PART_ENV], res.stdout)

    def test_shards_share_no_tmpdir_and_no_state(self):
        names = sorted(set(["TMPDIR", "XDG_STATE_HOME"]) | set(selftest_shard.STATE_ENVS))
        mods = [self.module("s%d" % i, SPY_CASE % (names,)) for i in range(4)]
        out = self.root / "spy"
        out.mkdir()
        res = self.run_runner(["unittest"] + mods, 4, extra={"SPY_OUT": str(out)})
        self.assertEqual(0, res.returncode, res.stdout)
        seen = [json.loads((out / (m + ".json")).read_text()) for m in mods]
        for name in names + ["gettempdir"]:
            values = [s[name] for s in seen]
            self.assertTrue(all(values), name)
            self.assertEqual(len(values), len(set(values)), "shared %s: %s" % (name, values))
            self.assertNotIn(os.environ.get(name), values, name)
        for s in seen:
            self.assertFalse(os.path.exists(s["TMPDIR"]), "the shard dirs go after the run")


# --- the class mode (#1160) ---------------------------------------------------
# Every test claims its own marker file with O_EXCL: a second run of the same
# test fails, so a green station proves "each test exactly once".
ONCE_HEAD = """import os
import unittest


def claim(test):
    os.close(os.open(os.path.join(os.environ["ONCE_DIR"], test.id()),
                     os.O_CREAT | os.O_EXCL | os.O_WRONLY))


class Base(unittest.TestCase):
    pass
"""
ONCE_CLASS = """

class %s(Base):
""" + "".join("""
    def test_%d(self):
        claim(self)
""" % i for i in range(3))
FIXTURE_TAIL = """

def setUpModule():
    pass
"""


class HeavyCase(ShardCase):
    """Heavy fixture modules and their duration table -- no tests of its own,
    so a subclass never runs another's tests twice (re-check tb-s2 h2 m-N6)."""

    def heavy(self, name, classes, tail=""):
        return self.module(name, ONCE_HEAD + "".join(ONCE_CLASS % c for c in classes) + tail)

    def table(self, weights):
        path = self.root / "durations.json"
        path.write_text(json.dumps(weights))
        return {selftest_shard.DURATIONS_ENV: str(path), "ONCE_DIR": str(self.once)}

    def setUp(self):
        super().setUp()
        self.once = self.root / "once"
        self.once.mkdir()

    def plan_named(self, out):
        return [u for ln in out.splitlines() if ln.startswith("SHARD ") for u in ln.split()[5:]]


class TestClassMode(HeavyCase):
    """A module heavier than a whole shard runs as its classes (#1160)."""

    def test_shard_by_class_runs_every_test_once(self):
        big = self.heavy("t_big", ["A", "B", "C", "D"])
        small = self.heavy("t_small", ["S"])
        extra = self.table({big: 100.0, small: 1.0})
        plan_out = self.run_runner(["--plan-only", "unittest", big, small], 3, extra=extra)
        named = [u for ln in plan_out.stdout.splitlines() if ln.startswith("SHARD ")
                 for u in ln.split()[5:]]
        self.assertEqual(sorted(["%s.%s" % (big, c) for c in "ABCD"] + [small]), sorted(named))
        self.assertIn("modules=2 tests=15 units=5", plan_out.stdout)
        res = self.run_runner(["unittest", big, small], 3, extra=extra)
        self.assertEqual(0, res.returncode, res.stdout)
        self.assertIn("Ran 15 tests in", res.stdout)
        self.assertIn("SELFTEST-VERDICT GREEN 3/3 shards", res.stdout)
        self.assertEqual(15, len(os.listdir(self.once)))

    def test_shard_by_class_keeps_module_fixture_module_whole(self):
        big = self.heavy("t_load", ["A", "B", "C"], "\n\ndef load_tests(loader, tests, pattern):\n"
                                                    "    return tests\n")
        units, _w, _t = selftest_shard.plan_units([big, "x.y"], 4, {big: 100.0, "x.y": 1.0},
                                                  str(self.root))
        self.assertEqual([big, "x.y"], units)

    def test_shard_by_class_runs_set_up_module_in_every_shard(self):
        """setUpModule runs once per process around the classes -- in each shard."""
        tail = FIXTURE_TAIL.replace("    pass", "    open(os.path.join(os.environ['ONCE_DIR'],"
                                    " 'setup-%d' % os.getpid()), 'w').close()")
        big = self.heavy("t_setup", ["A", "B"], tail)
        extra = self.table({big: 100.0})
        res = self.run_runner(["unittest", big], 2, extra=extra)
        self.assertEqual(0, res.returncode, res.stdout)
        self.assertIn("SELFTEST-VERDICT GREEN 2/2 shards", res.stdout)
        names = os.listdir(self.once)
        self.assertEqual(6, len([n for n in names if not n.startswith("setup-")]))
        self.assertEqual(2, len([n for n in names if n.startswith("setup-")]))

    def test_shard_by_class_keeps_a_module_whole_without_a_table_or_on_one_shard(self):
        big = self.heavy("t_big", ["A", "B"])
        root = str(self.root)
        self.assertEqual([big], selftest_shard.plan_units([big], 4, {}, root)[0])
        self.assertEqual([big], selftest_shard.plan_units([big], 1, {big: 9.0}, root)[0])
        broken = self.module("t_broken", "import no_such_module_anywhere\n")
        self.assertEqual([broken], selftest_shard.plan_units([broken], 4, {broken: 9.0}, root)[0])

    def test_the_table_knows_every_test_module(self):
        # Fund tv-bn-ci (Welle Testing): pre printed `shard imbalance 2.06`, the
        # table was older than eleven modules of scripts/tests/ (among them
        # test_precheck_testing_bans, 91 tests) -- each weighed the median
        # instead of itself. A new module brings its measured entry along.
        table = selftest_shard.load_durations(selftest_shard.DURATIONS)
        mods = {"scripts.tests." + p.stem for p in (REPO / "scripts" / "tests").glob("test_*.py")}
        self.assertEqual([], sorted(mods - set(table)),
                         "modules without an entry in %s" % selftest_shard.DURATIONS)

    def test_the_committed_table_is_in_the_writers_form(self):
        # d58-2 bank re-check 5: entries added by hand stood out of order, so the
        # next `durations --write` moved them -- a diff nobody made. The file is
        # what `write_table` writes, sorted keys, one entry per line.
        with open(selftest_shard.DURATIONS, encoding="utf-8") as fh:
            committed = fh.read()
        table = selftest_shard.load_durations(selftest_shard.DURATIONS)
        self.assertEqual(json.dumps(table, indent=1, sort_keys=True) + "\n", committed)

    def test_the_tree_guards_of_test_gate_plan_are_units_of_their_own(self):
        # tb-s1c (#1177): TemplateReadersInTheTree and RuleEightReadersInTheTree
        # plan every reader of the real tree (~110 s each). Under the old entry
        # (29.5 s, the module without them) the planner kept test_gate_plan
        # whole on one shard; the table names both classes, and the module
        # weighs at least their sum, so each is its own unit.
        mod = "scripts.tests.test_gate_plan"
        slow = ["%s.%s" % (mod, c) for c in ("TemplateReadersInTheTree",
                                              "RuleEightReadersInTheTree")]
        table = selftest_shard.load_durations(selftest_shard.DURATIONS)
        for cls in slow:
            self.assertIn(cls, table)
        self.assertGreater(table[mod], sum(table[c] for c in slow))
        mods = sorted(m for m in table if m.count(".") == 2)
        # The width at which the module outweighs the ideal shard (4 up to the
        # refresh of tv-kit B: at 4 the re-measured module, 193.8 s of 801 s,
        # no longer outweighs the ideal 200 s and rightly stays whole there; pre
        # plans fewer modules, where it is cut). At that width both guards must
        # be units of their own -- the split this pin is about.
        total = sum(table[m] for m in mods)
        shards = next(n for n in range(4, 65) if table[mod] > total / n)
        units = selftest_shard.plan_units(mods, shards, table, str(REPO))[0]
        self.assertLessEqual(set(slow), {u.split("[", 1)[0] for u in units})

    def test_shard_by_class_balances_the_kit_modules(self):
        # Re-check tb-s2 h2 M2: after pd-legacy (TestGateHost gone, test_strand_sh
        # 286 -> 131 tests) the old table kept test_gate_sh whole as the floor.
        # Whatever the classes are: at pre's width every unit fits the ideal
        # shard unless it cannot be cut, and the units load the whole run.
        mods = ["scripts.tests.test_strand_sh", "scripts.tests.test_gate_sh"]
        table = selftest_shard.load_durations(selftest_shard.DURATIONS)
        for mod in mods:
            self.assertIn(mod, table)
        units, weights, tests = selftest_shard.plan_units(mods, 4, table, str(REPO))
        ideal = sum(table[m] for m in mods) / 4
        for mod in mods:
            # Whole only where it fits the ideal shard (GH #1419 M3, OR-d53-4.z2.12):
            # on the table a green gate measured (a 12-thread lane, 4 shards)
            # test_gate_sh weighs 142.4 s of an ideal 201.0 s and rightly runs whole;
            # the pin is about a module heavier than the ideal one kept whole.
            if mod in units:
                self.assertLessEqual(table[mod], ideal,
                                     "%s runs whole: its table entry is stale" % mod)
                continue
            info = selftest_shard.discover(mod, str(REPO))
            self.assertEqual(info["total"], sum(n for u, n in tests.items()
                                               if selftest_shard.module_of(u) == mod))
            for u in (u for u in units if selftest_shard.module_of(u) == mod):
                cls = u.split("[", 1)[0].rsplit(".", 1)[1]
                if weights[u] > ideal:
                    self.assertTrue(info["classes"][cls] == 1 or cls in info["class_fixtures"],
                                    "%s weighs %.1f > ideal %.1f and could be cut"
                                    % (u, weights[u], ideal))
        # the counting proof: the units' names load exactly the whole run, none twice
        names = unit_ids(units)
        self.assertEqual(len(names), len(set(names)))
        whole = whole_run_ids(mods, REPO)
        self.assertEqual(sorted(whole), sorted(whole_run_ids(names, REPO)))
        shards = selftest_shard.distribute(units, 4, weights)
        loads = sorted(sum(weights[u] for u in sh) for sh in shards)
        self.assertAlmostEqual(sum(table[m] for m in mods), sum(loads), delta=0.5)
        # No shard far above the even share: a quarter at most, unless one unit
        # alone is heavier -- it cannot be cut (re-check tb-s2 h2 m-N5; the
        # bound of before, ideal + the heaviest unit, held for any greedy plan).
        bound = max(1.25 * ideal, max(weights[u] for u in units))
        self.assertLessEqual(loads[-1], bound, loads)

    def test_a_class_with_a_class_fixture_is_never_cut(self):
        # Re-check tb-s2 h2 m3: setUpClass would run once per part, side by side.
        fixed = self.module("t_fix", ONCE_HEAD + ONCE_CLASS % "A" + ONCE_CLASS % "B" + """

class C(Base):
    @classmethod
    def setUpClass(cls):
        pass

    def test_0(self):
        claim(self)

    def test_1(self):
        claim(self)

    def test_2(self):
        claim(self)


class D(C):
    def test_3(self):
        claim(self)

    def test_4(self):
        claim(self)


class Fixture:
    @classmethod
    def setUpClass(cls):
        pass


class E(Fixture, Base):
    def test_5(self):
        claim(self)

    def test_6(self):
        claim(self)
""")
        info = selftest_shard.discover(fixed, str(self.root))
        # E: the class fixture comes from a mixin, not a TestCase (re-check tb-s2 h2 m-N2)
        self.assertEqual(["C", "D", "E"], sorted(info["class_fixtures"]))
        table = {fixed: 200.0, fixed + ".A": 70.0, fixed + ".C": 70.0, fixed + ".D": 70.0,
                 fixed + ".E": 70.0}
        units = selftest_shard.plan_units([fixed], 3, table, str(self.root))[0]
        self.assertIn(fixed + ".C", units)
        self.assertIn(fixed + ".D", units)
        self.assertIn(fixed + ".E", units)
        self.assertTrue(any(u.startswith(fixed + ".A[") for u in units), units)
        # a discovery without the word: no class is cut
        info.pop("class_fixtures")
        units = selftest_shard.plan_units([fixed], 3, table, str(self.root),
                                          discover_fn=lambda m, r: info)[0]
        self.assertFalse(any("[" in u for u in units), units)


# --- the method mode (#1160, OR-TB.S2.6) ----------------------------------------
_IDS = r"""
import sys, unittest
def flat(s):
    for t in s:
        if isinstance(t, unittest.TestSuite):
            yield from flat(t)
        else:
            yield t
for t in flat(unittest.TestLoader().loadTestsFromNames(sys.argv[1:])):
    print(t.id())
"""


def whole_run_ids(names, root):
    """The test ids `python -m unittest <names>` loads (a module name: its whole run)."""
    names = [names] if isinstance(names, str) else list(names)
    out = subprocess.run([sys.executable, "-c", _IDS] + names, cwd=root, capture_output=True,
                         text=True, check=True, timeout=4 * kit_time.HANG).stdout
    return out.split()


def unit_ids(units):
    """The unittest names the shards are started with, unit by unit."""
    return selftest_shard.unit_argv(units)


class TestMethodMode(HeavyCase):
    """A class heavier than a whole shard runs as parts of its methods."""

    def test_shard_by_method_splits_a_heavy_class(self):
        big = self.heavy("t_m", ["A", "B"])
        units, weights, tests = selftest_shard.plan_units(
            [big], 3, {big: 100.0, big + ".A": 90.0}, str(self.root))
        parts = [u for u in units if u.startswith(big + ".A[")]
        self.assertGreaterEqual(len(parts), 2, units)
        self.assertIn(big + ".B", units)
        self.assertNotIn(big + ".A", units)
        self.assertEqual(3, sum(tests[u] for u in parts))
        self.assertAlmostEqual(90.0, sum(weights[u] for u in parts), delta=0.01)
        self.assertAlmostEqual(100.0, sum(weights.values()), delta=0.01)
        extra = self.table({big: 100.0, big + ".A": 90.0})
        out = self.run_runner(["--plan-only", "unittest", big], 3, extra=extra).stdout
        self.assertEqual(sorted(units), sorted(self.plan_named(out)))
        self.assertIn("modules=1 tests=6 units=%d" % len(units), out)

    def test_shard_by_method_runs_the_whole_test_set_once(self):
        """Union of the units = the whole run; no test twice (O_EXCL markers), none lost."""
        big = self.heavy("t_m", ["A", "B", "C"])
        table = {big: 100.0, big + ".A": 60.0, big + ".C": 30.0}
        whole = whole_run_ids(big, self.root)
        self.assertEqual(9, len(whole))
        units, _w, _t = selftest_shard.plan_units([big], 4, table, str(self.root))
        self.assertTrue(any("[" in u for u in units), units)
        names = unit_ids(units)
        self.assertEqual(len(names), len(set(names)), names)
        self.assertEqual(sorted(whole), sorted(whole_run_ids(names, self.root)))
        res = self.run_runner(["unittest", big], 4, extra=self.table(table))
        self.assertEqual(0, res.returncode, res.stdout)
        self.assertIn("Ran 9 tests in", res.stdout)
        self.assertEqual(sorted(whole), sorted(os.listdir(self.once)))

    def test_shard_by_method_keeps_a_class_whole_with_load_tests_or_unknown_methods(self):
        root = str(self.root)
        loaded = self.heavy("t_load", ["A", "B"], "\n\ndef load_tests(loader, tests, pattern):\n"
                                                  "    return tests\n")
        table = {loaded: 100.0, loaded + ".A": 90.0}
        self.assertEqual([loaded], selftest_shard.plan_units([loaded], 3, table, root)[0])
        # A class imported from another module: its ids do not name this module.
        other = self.heavy("t_other", ["X"])
        imp = self.module("t_imp", ONCE_HEAD + ONCE_CLASS % "B" + "\nfrom %s import X\n" % other)
        units = selftest_shard.plan_units([imp], 3, {imp: 100.0, imp + ".X": 90.0}, root)[0]
        self.assertEqual(sorted([imp + ".B", imp + ".X"]), sorted(units))
        # No method list at all (a discovery without one): the class stays whole.
        info = {"fixture": False, "classes": {"A": 3, "B": 3}, "total": 6}
        units = selftest_shard.plan_units(["m"], 3, {"m": 100.0, "m.A": 90.0}, root,
                                          discover_fn=lambda m, r: info)[0]
        self.assertEqual(["m.A", "m.B"], units)

    def test_shard_by_method_a_red_part_makes_the_station_red(self):
        red = self.module("t_red", ONCE_HEAD + ONCE_CLASS % "B" + """

class A(Base):
    def test_a(self):
        claim(self)

    def test_b(self):
        self.fail("a red part")

    def test_c(self):
        claim(self)
""")
        extra = self.table({red: 100.0, red + ".A": 90.0})
        out = self.run_runner(["--plan-only", "unittest", red], 3, extra=extra).stdout
        self.assertTrue(any(u.startswith(red + ".A[") for u in self.plan_named(out)), out)
        res = self.run_runner(["unittest", red], 3, extra=extra)
        self.assertEqual(1, res.returncode, res.stdout)
        self.assertIn("1 failed", res.stdout)
        self.assertIn("FAILED (errors=1)", res.stdout)
        self.assertRegex(res.stdout, r"SELFTEST-VERDICT RED \d/3 shards")


GROW_CASE = ONCE_HEAD + ONCE_CLASS % "A" + ONCE_CLASS % "B" + """

GROW = os.path.join(os.environ["ONCE_DIR"], "..", "grow")


def setUpModule():
    open(GROW, "w").close()


if os.path.exists(GROW):
    class C(Base):
        def test_late(self):
            claim(self)
"""
ATEXIT_CASE = GREEN_CASE + """

import atexit, os
if os.environ.get("BREAK_PLAN"):
    atexit.register(lambda: print('{"fixture": false, "classes": ["A", "B"], "total": 2}'))
"""


IMPORT_CONFIG_CASE = ONCE_HEAD + ONCE_CLASS % "A" + ONCE_CLASS % "B" + """

import subprocess
subprocess.run(["git", "config", "core.probeimport", "1"], check=True)
"""
# Writes the config on the import after both shards ran setUpModule: the
# recount's, never the plan's or a shard's.
RECOUNT_CONFIG_CASE = ONCE_HEAD + ONCE_CLASS % "A" + ONCE_CLASS % "B" + """

SETUPS = os.path.join(os.environ["ONCE_DIR"], "..", "setups")
os.makedirs(SETUPS, exist_ok=True)
if len(os.listdir(SETUPS)) >= 2:
    import subprocess
    subprocess.run(["git", "config", "core.proberecount", "1"], check=True)


def setUpModule():
    open(os.path.join(SETUPS, str(os.getpid())), "w").close()
"""


class TestCounted(HeavyCase):
    """Re-check tb-s2 h2 m1/m2/m6: a split run is counted, the plan sees no
    caller repository, and the fail-closed paths are tested, not argued."""

    def test_a_class_added_after_the_plan_is_red(self):
        big = self.module("t_grow", GROW_CASE)
        table = self.table({big: 100.0})
        out = self.run_runner(["--plan-only", "unittest", big], 2, extra=table).stdout
        self.assertIn(big + ".A", self.plan_named(out))
        res = self.run_runner(["unittest", big], 2, extra=table)
        self.assertEqual(1, res.returncode, res.stdout)
        self.assertIn("%s has 7 tests, 6 planned" % big, res.stdout)
        self.assertRegex(res.stdout, r"SELFTEST-VERDICT RED 2/2 shards; .*7 tests, 6 planned")

    def test_recount_names_a_module_that_no_longer_imports(self):
        big = self.heavy("t_m", ["A", "B"])
        units, _w, tests = selftest_shard.plan_units([big], 2, {big: 100.0}, str(self.root))
        self.assertEqual([], selftest_shard.recount(units, tests, str(self.root)))
        (self.root / "fx" / "t_m.py").write_text("import no_such_module_anywhere\n")
        self.assertEqual(["%s has no count of its tests, 6 planned" % big],
                         selftest_shard.recount(units, tests, str(self.root)))

    def test_a_shard_that_runs_fewer_than_planned_is_red(self):
        big = self.heavy("t_m", ["A", "B"])
        real = selftest_shard.run_shard

        def short(argv, env, cwd, log, children):
            rc, text, secs = real(argv, env, cwd, log, children)
            return rc, text.replace("Ran 3 tests", "Ran 2 tests"), secs

        argv = ["unittest", big]
        env = {plan.SELFTEST_SHARDS_ENV: "2", selftest_shard.LOG_DIR_ENV: str(self.logs),
               selftest_shard.DURATIONS_ENV: self.table({big: 100.0})[selftest_shard.DURATIONS_ENV],
               "ONCE_DIR": str(self.once)}
        out = io.StringIO()
        with mock.patch.dict(os.environ, env), mock.patch.object(selftest_shard, "run_shard", short), \
                mock.patch.object(selftest_shard, "_nproc", return_value=2), \
                mock.patch("os.getcwd", return_value=str(self.root)), \
                contextlib.redirect_stdout(out):
            rc = selftest_shard.main(argv)
        self.assertEqual(1, rc, out.getvalue())
        self.assertIn("ran 2 of 3 planned", out.getvalue())

    def test_the_plan_never_sees_the_callers_repository(self):
        big = self.heavy("t_m", ["A", "B"])
        seen = []

        def spy(module, root):
            seen.append(os.environ.get("GIT_DIR"))
            return None

        env = {"GIT_DIR": "/elsewhere/.git", plan.SELFTEST_SHARDS_ENV: "2",
               selftest_shard.DURATIONS_ENV: self.table({big: 100.0})[selftest_shard.DURATIONS_ENV]}
        with mock.patch.dict(os.environ, env), mock.patch.object(selftest_shard, "discover", spy), \
                mock.patch.object(selftest_shard, "_nproc", return_value=2), \
                mock.patch("os.getcwd", return_value=str(self.root)), \
                contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(0, selftest_shard.main(["--plan-only", "unittest", big]))
        self.assertEqual([None], seen)

    def test_a_planner_that_raises_is_red(self):
        mod = self.module("t_plan", ATEXIT_CASE)
        extra = dict(self.table({mod: 100.0}), BREAK_PLAN="1")
        res = self.run_runner(["unittest", mod], 2, extra=extra)
        self.assertNotEqual(0, res.returncode, res.stdout)
        self.assertIn("Traceback", res.stdout)
        self.assertEqual("RED", selftest_shard.judge(res.returncode, res.stdout)[0])

    def test_a_part_without_its_ids_is_red(self):
        big = self.heavy("t_m", ["A", "B"])
        part = selftest_shard.MethodUnit(big + ".A[1/2]")
        res = subprocess.run([sys.executable, "-m", "unittest"] + selftest_shard.unit_argv([part]),
                             cwd=self.root, capture_output=True, text=True, timeout=2 * kit_time.HANG,
                             env=dict(os.environ, ONCE_DIR=str(self.once)))
        self.assertEqual("RED", selftest_shard.judge(res.returncode, res.stdout + res.stderr)[0])

    def test_no_tests_is_red(self):
        # Two modules for two shards: one module is one unit, one shard
        # (re-check tb-s2 h2 m-N4).
        empty = [self.module("t_empty", "import unittest\n"),
                 self.module("t_empty2", "import unittest\n")]
        for shards, mods in ((1, empty[:1]), (2, empty)):
            res = self.run_runner(["unittest"] + mods, shards)
            self.assertNotEqual(0, res.returncode, res.stdout)
            self.assertEqual("RED", selftest_shard.judge(res.returncode, res.stdout)[0])
        self.assertRegex(res.stdout, r"(?m)^SELFTEST-VERDICT RED 0/2 shards")

    def test_a_config_write_by_the_plans_discovery_is_red(self):
        # Re-check tb-s2 h2 M-N1: the snapshot came after the plan, whose
        # discovery imports the heavy module -- GREEN beside a changed config.
        self.init_root()
        big = self.module("t_imp", IMPORT_CONFIG_CASE)
        res = self.run_runner(["unittest", big], 2, extra=self.table({big: 100.0}))
        self.assertIn("SHARD 2/2", res.stdout)
        self.assertEqual(1, res.returncode, res.stdout)
        self.assertRegex(res.stdout, r"(?m)^SELFTEST-VERDICT RED .*\+core\.probeimport=1")

    def test_a_config_write_by_the_recount_is_red(self):
        # Re-check tb-s2 h2 M-N1: the guard ran before the recount, which
        # imports the split module once more.
        self.init_root()
        big = self.module("t_rec", RECOUNT_CONFIG_CASE)
        res = self.run_runner(["unittest", big], 2, extra=self.table({big: 100.0}))
        self.assertIn("SHARD 2/2", res.stdout)
        self.assertEqual(1, res.returncode, res.stdout)
        self.assertRegex(res.stdout, r"(?m)^SELFTEST-VERDICT RED .*\+core\.proberecount=1")

    def test_shard_seconds_far_apart_are_a_note(self):
        # Re-check tb-s2 h2 m-N5: the table drifts from the run unnoticed.
        self.assertEqual("", selftest_shard.imbalance([10.0, 9.0]))
        self.assertEqual("", selftest_shard.imbalance([3.0, 0.1]))  # under the floor
        self.assertEqual("", selftest_shard.imbalance([40.0]))
        self.assertIn("max/mean 1.90", selftest_shard.imbalance([20.0, 1.0]))
        # the A/B run of receipts/tb-s2 (m-N8): stale table vs fresh table
        for secs, note in (([73.6, 42.5, 40.9, 44.6], True), ([53.7, 26.9, 24.7, 28.0], True),
                           ([39.3, 34.6, 42.3, 38.4], False), ([39.1, 32.0, 41.5, 36.2], False)):
            self.assertEqual(note, bool(selftest_shard.imbalance(secs)), secs)
        big = self.heavy("t_m", ["A", "B"])
        real = selftest_shard.run_shard
        for secs, note in (([20.0, 1.0], True), ([10.0, 9.0], False)):
            def timed(argv, env, cwd, log, children, secs=secs):
                rc, text, _s = real(argv, env, cwd, log, children)
                return rc, text, secs.pop()

            env = {plan.SELFTEST_SHARDS_ENV: "2", selftest_shard.LOG_DIR_ENV: str(self.logs),
                   selftest_shard.DURATIONS_ENV:
                       self.table({big: 100.0})[selftest_shard.DURATIONS_ENV],
                   "ONCE_DIR": str(self.once)}
            for name in os.listdir(self.once):
                os.unlink(os.path.join(self.once, name))
            out = io.StringIO()
            with mock.patch.dict(os.environ, env), \
                    mock.patch.object(selftest_shard, "run_shard", timed), \
                    mock.patch.object(selftest_shard, "_nproc", return_value=2), \
                    mock.patch("os.getcwd", return_value=str(self.root)), \
                    contextlib.redirect_stdout(out):
                rc = selftest_shard.main(["unittest", big])
            self.assertEqual(0, rc, out.getvalue())
            self.assertIn("SELFTEST-VERDICT GREEN 2/2 shards", out.getvalue())
            self.assertEqual(note, "SELFTEST-NOTE shard imbalance max/mean" in out.getvalue(),
                             out.getvalue())


class TestThisModule(unittest.TestCase):

    def test_no_test_runs_twice(self):
        # Re-check tb-s2 h2 m-N6: TestCounted inherited the four tests of
        # TestMethodMode and ran them a second time.
        def flatten(suite):
            for t in suite:
                if isinstance(t, unittest.TestSuite):
                    yield from flatten(t)
                else:
                    yield t

        seen = {}
        for t in flatten(unittest.TestLoader().loadTestsFromModule(sys.modules[__name__])):
            seen.setdefault(getattr(type(t), t._testMethodName), []).append(t.id())
        self.assertEqual([], [ids for ids in seen.values() if len(ids) > 1])


class TestOneShard(ShardCase):
    """(d) MECLAW_SELFTEST_SHARDS=1 is the command of before."""

    def test_one_shard_is_plain_unittest(self):
        mods = [self.module("a_ok", GREEN_CASE), self.module("b_red", RED_CASE)]
        old = subprocess.run([sys.executable, "-m", "unittest"] + mods, cwd=self.root,
                             capture_output=True, text=True, timeout=2 * kit_time.HANG)
        new = self.run_runner(["unittest"] + mods, 1)
        self.assertEqual(old.returncode, new.returncode)
        self.assertNotIn("SHARD", new.stdout)
        self.assertNotIn("SELFTEST-VERDICT", new.stdout)
        self.assertIn("Ran 3 tests in", new.stdout)
        self.assertIn("FAILED (failures=1)", new.stdout)

    def test_options_of_the_isolation_run_serially(self):
        mods = [self.module("a_ok", GREEN_CASE), self.module("b_red", RED_CASE)]
        res = self.run_runner(["unittest"] + mods + ["-k", "test_one"], 2)
        self.assertEqual(0, res.returncode, res.stdout)
        self.assertNotIn("SELFTEST-VERDICT", res.stdout)
        self.assertIn("Ran 1 test in", res.stdout)

    def test_the_shard_count_and_its_override(self):
        # GH #1419 L2: max(4, nproc // 4), at most nproc -- build04 (24) 6, the
        # 12-thread lanes 4, the public runner (4) 4
        self.assertEqual(6, plan.selftest_shards(24, {}))
        self.assertEqual(4, plan.selftest_shards(12, {}))
        self.assertEqual(8, plan.selftest_shards(32, {}))
        self.assertEqual(4, plan.selftest_shards(4, {}))
        self.assertEqual(3, plan.selftest_shards(3, {}))
        self.assertEqual(6, plan.resolve_cpu(plan.CPU_SHARDS, 24))
        self.assertEqual(1, plan.selftest_shards(24, {plan.SELFTEST_SHARDS_ENV: "1"}))
        self.assertEqual(12, plan.selftest_shards(4, {plan.SELFTEST_SHARDS_ENV: "12"}))
        for bad in ("0", "-2", "x", "1.5"):
            with self.assertRaises(ValueError, msg=bad):
                plan.selftest_shards(8, {plan.SELFTEST_SHARDS_ENV: bad})
        res = self.run_runner(["unittest", self.module("a_ok", GREEN_CASE)], "zero")
        self.assertEqual(2, res.returncode)


class TestCpuNeed(unittest.TestCase):
    """The scheduler books one thread per shard, never more than the lane has."""

    def test_the_station_books_its_shards(self):
        self.assertEqual(plan.CPU_SHARDS, plan.NEEDS["gate-selftest"]["cpu"])
        with mock.patch.dict(os.environ, {plan.SELFTEST_SHARDS_ENV: ""}):
            # GH #1419 L2: max(4, nproc // 4), at most nproc
            self.assertEqual(6, plan.resolve_cpu(plan.CPU_SHARDS, 24))
            self.assertEqual(2, plan.resolve_cpu(plan.CPU_SHARDS, 2))
        with mock.patch.dict(os.environ, {plan.SELFTEST_SHARDS_ENV: "3"}):
            self.assertEqual(3, plan.resolve_cpu(plan.CPU_SHARDS, 24))
        with mock.patch.dict(os.environ, {plan.SELFTEST_SHARDS_ENV: "16"}):
            # Booked at most nproc; the executor hands the runner these 8 (TestBookedIsStarted).
            self.assertEqual(8, plan.resolve_cpu(plan.CPU_SHARDS, 8))


class TestBookedIsStarted(unittest.TestCase):
    """The shards the runner starts are the threads the lane booked."""

    MODS = ["m%02d" % i for i in range(12)]

    def booked(self, lane_cpu, kern_env):
        doc = {"stations": [{"name": "gate-selftest", "needs": dict(plan.NEEDS["gate-selftest"])}]}
        with mock.patch.dict(os.environ, kern_env):
            return plan.resolve_needs(doc, lane_cpu)["stations"][0]

    def station_env(self, st, lane_env):
        from pipeline import executor
        run = executor._Run.__new__(executor._Run)
        run.base_env = dict(lane_env)
        run.target, run.lock_file, run.lock_state, run.export_root = "/nonexistent", "", "absent", ""
        return run._env(st)

    def started(self, env, nproc):
        import contextlib
        import io
        out = io.StringIO()
        with mock.patch.dict(os.environ, env, clear=True), \
                mock.patch.object(selftest_shard, "_nproc", return_value=nproc), \
                contextlib.redirect_stdout(out):
            rc = selftest_shard.main(["--plan-only", "unittest"] + self.MODS)
        self.assertEqual(0, rc, out.getvalue())
        last = out.getvalue().splitlines()[-1]
        return int(last.split()[1].split("=")[1])

    def assert_started_is_booked(self, lane_cpu, nproc, kern_env, lane_env):
        st = self.booked(lane_cpu, kern_env)
        env = self.station_env(st, lane_env)
        self.assertEqual(str(st["needs"]["cpu"]), env[plan.SELFTEST_SHARDS_ENV])
        self.assertEqual(st["needs"]["cpu"], self.started(env, nproc))
        return st["needs"]["cpu"]

    def test_the_egon_lane_mm_os01_starts_what_it_books(self):
        # threads=4 next to the live Egon on an 8-thread machine: 4 booked, 4 started.
        self.assertEqual(4, self.assert_started_is_booked(4, 8, {plan.SELFTEST_SHARDS_ENV: ""},
                                                          {}))

    def test_an_override_reaches_a_remote_lane(self):
        # The kern books 1; the lane's own environment does not carry the override.
        self.assertEqual(1, self.assert_started_is_booked(
            24, 24, {plan.SELFTEST_SHARDS_ENV: "1"}, {plan.SELFTEST_SHARDS_ENV: "8"}))
        self.assertEqual(1, self.assert_started_is_booked(
            24, 24, {plan.SELFTEST_SHARDS_ENV: "1"}, {}))

    def test_an_invalid_override_books_and_starts_the_default_not_the_lane(self):
        # An invalid override booked the whole lane, and a Unicode digit threw in the
        # executor; now both ends fall back to the booked number and the log says so.
        import contextlib
        import io
        for bad in ("x", "0", "-3", "\u00b2", "\u0663"):
            with self.subTest(bad=bad):
                self.assertEqual(6, self.assert_started_is_booked(
                    24, 24, {plan.SELFTEST_SHARDS_ENV: bad}, {plan.SELFTEST_SHARDS_ENV: bad}))
                env = self.station_env(self.booked(24, {plan.SELFTEST_SHARDS_ENV: bad}),
                                       {plan.SELFTEST_SHARDS_ENV: bad})
                out = io.StringIO()
                with mock.patch.dict(os.environ, env, clear=True), \
                        mock.patch.object(selftest_shard, "_nproc", return_value=24), \
                        contextlib.redirect_stdout(out):
                    selftest_shard.main(["--plan-only", "unittest"] + self.MODS)
                self.assertIn("override %s=%r ignored, booked 6" % (plan.SELFTEST_SHARDS_ENV, bad),
                              out.getvalue())
                with self.assertRaises(ValueError):
                    plan.selftest_shards(24, {plan.SELFTEST_SHARDS_ENV: bad})

    def test_the_runner_never_starts_more_shards_than_threads(self):
        self.assertEqual(2, self.started({plan.SELFTEST_SHARDS_ENV: "16"}, 2))
        self.assertEqual(6, self.started({}, 24), "default: max(4, nproc // 4)")
        self.assertEqual(3, self.started({}, 3))


class TestSignal(unittest.TestCase):
    """SIGTERM/SIGINT on the runner ends every shard, no orphans.

    No real signal: the handler the runner installs is captured and called by a
    patched child, and the children are fakes that record terminate/kill."""

    def test_a_signal_on_the_runner_ends_every_shard(self):
        import contextlib
        import io
        import signal
        import threading
        handlers, children = {}, []

        def capture(signum, handler):
            handlers[signum] = handler
            return signal.SIG_DFL

        class Child:
            def __init__(self, argv, **kw):
                self.argv, self.returncode, self.ended = argv, None, threading.Event()
                self.signals, self.kw, self.pid = [], kw, 40000 + len(children)
                self.dirs = os.listdir(os.environ["TMPDIR"])  # the runner's TMPDIR
                children.append(self)

            def wait(self, timeout=None):
                if self is children[0]:
                    handlers[signal.SIGTERM](signal.SIGTERM, None)
                self.ended.wait(kit_time.HANG)
                return self.returncode

            def poll(self):
                return self.returncode

            def terminate(self):
                self.signals.append("TERM")
                self.returncode = -signal.SIGTERM
                self.ended.set()

            def kill(self):
                self.signals.append("KILL")
                self.returncode = -signal.SIGKILL
                self.ended.set()

        with tempfile.TemporaryDirectory() as tmp:
            env = {plan.SELFTEST_SHARDS_ENV: "3", selftest_shard.LOG_DIR_ENV: tmp,
                   "TMPDIR": tmp, "PATH": os.environ.get("PATH", "")}
            out = io.StringIO()
            with mock.patch.dict(os.environ, env, clear=True), \
                    mock.patch.object(selftest_shard, "_nproc", return_value=3), \
                    mock.patch.object(selftest_shard, "stdout_log", return_value=None), \
                    mock.patch.object(selftest_shard, "_popen", Child), \
                    mock.patch.object(selftest_shard.os, "killpg", side_effect=AssertionError), \
                    mock.patch.object(tempfile, "tempdir", tmp), \
                    mock.patch.object(selftest_shard.signal, "signal", side_effect=capture), \
                    mock.patch.object(selftest_shard.gitenv, "drift",
                                      return_value=("the suite changed X", "")), \
                    contextlib.redirect_stdout(out):
                rc = selftest_shard.main(["unittest", "fx.a", "fx.b", "fx.c"])
            self.assertEqual({signal.SIGTERM, signal.SIGINT} & set(handlers),
                             {signal.SIGTERM, signal.SIGINT})
            self.assertEqual(128 + signal.SIGTERM, rc, out.getvalue())
            self.assertTrue(children)
            for child in children:
                self.assertEqual(["TERM"], child.signals, child.argv)
                self.assertFalse(child.kw.get("start_new_session"),
                                 "a shard stays in the station's group (the executor's stop)")
                self.assertNotIn("preexec_fn", child.kw)
                wrap = selftest_shard.die_with_runner(["x"], os.getpid())[:-1]
                self.assertEqual(wrap, child.argv[:len(wrap)], "the shard dies with the runner")
            # `tempfile.tempdir` is patched: the shard dirs really sat in tmp during the run
            self.assertTrue(any(d.startswith("meclaw-selftest-") for d in children[0].dirs))
            self.assertEqual([], [d for d in os.listdir(tmp) if d.startswith("meclaw-selftest-")],
                             "the shard dirs go after a stop too")
            self.assertIn("SELFTEST-VERDICT RED", out.getvalue())
            # Re-check pd-legacy m-2: a stopped run still names what it changed.
            self.assertIn("every shard ended; the suite changed X", out.getvalue())


class TestChildren(unittest.TestCase):
    """Stop of each shard and a second signal during `stop`."""

    def test_a_second_signal_during_stop_does_not_hang(self):
        import signal
        import threading
        children = selftest_shard.Children()

        def nested():  # the handler runs again while the first `stop` holds the lock
            with children.lock:
                children.stop(signal.SIGINT)

        worker = threading.Thread(target=nested, daemon=True)
        worker.start()
        worker.join(kit_time.HANG)
        self.assertFalse(worker.is_alive(), "the reentrant stop deadlocked")
        self.assertEqual(signal.SIGINT, children.stopped)
        children.close()

    def test_stop_signals_each_shard_never_a_group(self):
        import signal
        started, sent = [], []

        class Proc:
            pid = 41234

            def __init__(self, argv, **kw):
                self.kw = kw
                started.append(self)

            def poll(self):
                return None

            def terminate(self):
                sent.append((self.pid, signal.SIGTERM))

            def kill(self):
                sent.append((self.pid, signal.SIGKILL))

        children = selftest_shard.Children()
        with mock.patch.object(selftest_shard, "_popen", Proc), \
                mock.patch.object(selftest_shard.os, "killpg", side_effect=AssertionError), \
                mock.patch.object(selftest_shard.os, "kill", side_effect=AssertionError):
            children.start(["true"])
            children.stop(signal.SIGTERM)
            children.close()
            self.assertFalse(started[0].kw.get("start_new_session"))
            self.assertEqual([(41234, signal.SIGTERM)], sent)
            children._signal(started, "kill")
            self.assertEqual((41234, signal.SIGKILL), sent[-1])


# The shard holds until it is killed (re-check tv-bn M7j B-M2: `time.sleep(60)`
# against a wait of HANG for its death -- from time factor 2 a shard that
# outlived the runner ended by itself within the wait, and the pin stayed green).
SLOW_CASE = '''import os, threading, unittest
class T(unittest.TestCase):
    def test_slow(self):
        with open(os.path.join(os.environ["SPY_OUT"], "%d.pid" % os.getpid()), "w") as fh:
            fh.write(__name__)
        threading.Event().wait()
'''


class TestRunnerKilled(ShardCase):
    """A SIGKILL on the runner (the executor's after its grace, the OOM
    killer) leaves no shard alive, and the next start removes the dead run's state dir.
    The only signal goes to the runner this test started (its remembered PID)."""

    def _alive(self, pid):
        try:
            with open("/proc/%d/stat" % pid) as fh:
                return fh.read().rsplit(")", 1)[1].split()[0] != "Z"
        except OSError:
            return False

    def test_a_sigkill_on_the_runner_leaves_no_shard_and_no_state_dir(self):
        import signal
        if not os.path.isdir("/proc/self") or selftest_shard._nproc() < 2:
            self.skipTest("needs /proc and two threads")
        spy, tmp = self.root / "spy", self.root / "tmp"
        spy.mkdir()
        tmp.mkdir()
        mods = [self.module("slow_a", SLOW_CASE), self.module("slow_b", SLOW_CASE)]
        env = dict(os.environ, SPY_OUT=str(spy), TMPDIR=str(tmp))
        env.update({plan.SELFTEST_SHARDS_ENV: "2", selftest_shard.LOG_DIR_ENV: str(self.logs),
                    selftest_shard.DURATIONS_ENV: str(self.root / "no-table.json")})
        runner = subprocess.Popen(lane.python_argv(RUNNER, "unittest", *mods), cwd=self.root,
                                  env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                  stdin=subprocess.DEVNULL, start_new_session=True)
        pids = []
        try:
            kit_time.until("both shards started", lambda: len(os.listdir(spy)) == 2, 2 * kit_time.HANG,
                           state=lambda: os.listdir(spy))
            pids = [int(n.split(".")[0]) for n in os.listdir(spy)]
            stale = [d for d in os.listdir(tmp) if d.startswith("meclaw-selftest-")]
            self.assertEqual(1, len(stale), "the run's state dir")
            os.kill(runner.pid, signal.SIGKILL)
            runner.wait(kit_time.HANG)
            kit_time.until("a shard outlived the runner", lambda: not any(map(self._alive, pids)),
                           state=lambda: [p for p in pids if self._alive(p)])
        finally:
            if runner.poll() is None:
                runner.kill()
                runner.wait(kit_time.HANG)
            for pid in pids:  # only a shard of this test, never a stranger
                try:
                    with open("/proc/%d/environ" % pid, "rb") as fh:
                        mine = ("SPY_OUT=%s" % spy).encode() in fh.read()
                except OSError:
                    mine = False
                if mine:  # a red run: the guard refuses a reparented shard, it sleeps out
                    try:
                        os.kill(pid, signal.SIGKILL)
                    except PermissionError:
                        pass
        self.assertEqual(stale, [d for d in os.listdir(tmp) if d.startswith("meclaw-selftest-")])
        green = [self.module("one", GREEN_CASE), self.module("two", GREEN_CASE)]
        proc = self.run_runner(["unittest"] + green, 2, extra={"TMPDIR": str(tmp)})
        self.assertEqual(0, proc.returncode, proc.stdout)
        self.assertEqual([], [d for d in os.listdir(tmp) if d.startswith("meclaw-selftest-")],
                         "the next start removes the dead run's state dir")

    def test_the_runner_grace_ends_before_the_executors(self):
        from pipeline import executor
        self.assertLessEqual(selftest_shard.KILL_GRACE_S * 2, executor.KILL_GRACE_S)


class TestDieWithRunner(unittest.TestCase):
    """A child dies with the runner without `preexec_fn` (fix round 3: a shard started
    from a pool thread with `preexec_fn` can deadlock before exec, Python's documented
    hazard; gate bba60d7 hung 19.5 min there). `setpriv --pdeathsig KILL` sets the
    death signal in the child, a shell refuses the exec when the runner is already gone."""

    # Roots of the kit's Python; every file there is scanned.
    ROOTS = ("scripts", os.path.join("plans", "export-fixtures"))
    CALL = re.compile(r"\bpreexec_fn\s*=")
    THREADS = re.compile(r"^\s*(?:import|from)\s+(?:threading|concurrent)\b", re.M)

    @classmethod
    def thread_reachable(cls, rel, text):
        """A file reaches threads when it imports them, or when it is a module of the
        runner package (`scripts/pipeline/`, loaded by the executor's station threads)."""
        return bool(cls.THREADS.search(text)) or rel.startswith("scripts/pipeline/")

    def test_no_thread_reachable_file_starts_a_child_with_preexec_fn(self):
        hits = []
        for root in self.ROOTS:
            for dirpath, _dirs, files in os.walk(REPO / root):
                for name in files:
                    if not name.endswith(".py"):
                        continue
                    path = os.path.join(dirpath, name)
                    rel = os.path.relpath(path, REPO).replace(os.sep, "/")
                    with open(path, encoding="utf-8", errors="replace") as fh:
                        text = fh.read()
                    if not self.thread_reachable(rel, text):
                        continue
                    for no, line in enumerate(text.splitlines(), 1):
                        if self.CALL.search(line.split("#", 1)[0]):
                            hits.append("%s:%d" % (rel, no))
        self.assertEqual([], hits, "preexec_fn in a file that reaches threads")

    def test_the_private_root_of_the_scan_is_there(self):
        # V8 fix round 1 (review I2): `os.walk` of a missing root yields
        # nothing; plans/export-fixtures never travels, so the scan above
        # checks it only in the private tree -- declared, never silent.
        private_tree.require(*(r for r in self.ROOTS if r != "scripts"))
        for root in self.ROOTS:
            self.assertTrue((REPO / root).is_dir(), root)

    def test_the_runner_package_counts_as_thread_reachable(self):
        self.assertTrue(self.thread_reachable("scripts/pipeline/x.py", ""))
        self.assertTrue(self.thread_reachable("scripts/y.py", "import concurrent.futures\n"))
        self.assertFalse(self.thread_reachable("scripts/y.py", "import os\n"))

    def run_wrapped(self, runner):
        with tempfile.TemporaryDirectory() as tmp:
            marker = os.path.join(tmp, "ran")
            argv = selftest_shard.die_with_runner(
                [sys.executable, "-I", "-c", "import sys; open(sys.argv[1], 'w').close()", marker],
                runner)
            proc = subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True, text=True,
                                  timeout=kit_time.HANG)
            return argv, proc, os.path.exists(marker)

    def test_a_child_of_the_runner_runs_its_program(self):
        argv, proc, ran = self.run_wrapped(os.getpid())
        self.assertEqual(0, proc.returncode, proc.stderr)
        self.assertTrue(ran, argv)

    def test_a_child_whose_runner_is_gone_before_exec_never_runs(self):
        # No skip (fix round 5, I4): without setpriv on Linux `die_with_runner` raises,
        # and this test is red with the tool's name. The child's parent is this process, not the named runner: as if the runner had
        # died between fork and the death signal (the child would be reparented).
        argv, proc, ran = self.run_wrapped(os.getppid())
        self.assertEqual(137, proc.returncode, proc.stderr)
        self.assertFalse(ran, argv)

    def test_on_linux_a_missing_setpriv_is_an_error(self):
        # Fault injection (fix round 5, I4): setpriv is gone on a Linux host. The plain
        # argv would start a child that outlives its runner, silently.
        with mock.patch.object(selftest_shard.sys, "platform", "linux"), \
                mock.patch.object(selftest_shard.shutil, "which", return_value=None):
            with self.assertRaisesRegex(selftest_shard.SetprivMissing, r"^setpriv not found"):
                selftest_shard.die_with_runner(["true"], os.getpid())

    def test_off_linux_the_plain_argv(self):
        with mock.patch.object(selftest_shard.sys, "platform", "darwin"):
            self.assertEqual(["true"], selftest_shard.die_with_runner(["true"], os.getpid()))

    def test_on_linux_a_run_without_setpriv_starts_nothing_and_names_it(self):
        def no_start(*_a, **_kw):
            raise AssertionError("a child started without setpriv")

        err = io.StringIO()
        with mock.patch.object(selftest_shard.sys, "platform", "linux"), \
                mock.patch.object(selftest_shard.shutil, "which", return_value=None), \
                mock.patch.object(selftest_shard, "_popen", no_start), \
                mock.patch.object(selftest_shard.sys, "stderr", err), \
                mock.patch.dict(os.environ):
            rc = selftest_shard.main(["unittest", "test_does_not_exist"])
        self.assertEqual(2, rc)
        self.assertIn("setpriv", err.getvalue())

    def test_every_child_start_wraps_its_argv(self):
        started = []

        class Proc:
            def __init__(self, argv, **kw):
                self.argv, self.kw = argv, kw
                started.append(self)

            def wait(self):
                return 0

        with mock.patch.object(selftest_shard, "_popen", Proc), \
                mock.patch.object(selftest_shard.shutil, "which", return_value="/usr/bin/setpriv"):
            selftest_shard.Children().start(["true"])
            selftest_shard.run_single(["true"])
        for proc in started:
            self.assertNotIn("preexec_fn", proc.kw)
            self.assertEqual(["/usr/bin/setpriv", "--pdeathsig", "KILL", "--"], proc.argv[:4])
            self.assertEqual(["true"], proc.argv[-1:])
            self.assertEqual(str(os.getpid()), proc.argv[-2])


class TestNoInheritedStdin(unittest.TestCase):
    """Fix round 4 v2-r3-kit: no test process of the selftest path reads the runner's
    stdin. A test that leaves a child holding the write end of a pipe (a leftover of
    the test before) must not block the next test's child that reads its stdin: a
    shard got DEVNULL already, `run_single` handed the runner's stdin on. Judged by
    the child's own word (its fd 0 over a fifo), never by time."""

    CHILD = ("import os, sys\n"
             "with open(os.environ['PROBE_FIFO'], 'w') as fh:\n"
             "    fh.write(os.readlink('/proc/self/fd/0') + '\\n')\n"
             "sys.stdin.read()\n")
    SINGLE = ("import sys\n"
              "sys.path.insert(0, %(scripts)r)\n"
              "from pipeline import selftest_shard\n"
              "sys.exit(selftest_shard.run_single([sys.executable, '-c', %(child)r]))\n")
    SHARD = ("import os, sys\n"
             "sys.path.insert(0, %(scripts)r)\n"
             "from pipeline import selftest_shard\n"
             "rc, _text, _secs = selftest_shard.run_shard(\n"
             "    [sys.executable, '-c', %(child)r], dict(os.environ), os.getcwd(), %(log)r,\n"
             "    selftest_shard.Children())\n"
             "sys.exit(rc)\n")

    def fd_zero_of_the_test(self, runner_src):
        """What the test started by `runner_src` reads as fd 0, while the runner's own
        stdin is a pipe whose only writer is a leftover child that lives on."""
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        fifo = os.path.join(tmp.name, "probe")
        os.mkfifo(fifo)
        r, w = os.pipe()
        leftover = subprocess.Popen([sys.executable, "-c", "import sys; sys.stdin.read()"],
                                    stdin=subprocess.PIPE, pass_fds=(w,))
        os.close(w)   # the write end lives in the leftover only
        src = runner_src % {"scripts": str(SCRIPTS), "child": self.CHILD,
                            "log": os.path.join(tmp.name, "shard.log")}
        runner = subprocess.Popen([sys.executable, "-I", "-c", src], stdin=r, cwd=str(REPO),
                                  env=dict(os.environ, PROBE_FIFO=fifo))
        os.close(r)
        # LIFO: the leftover ends first, then the test's child sees EOF and the runner ends
        self.addCleanup(leftover.wait)
        self.addCleanup(runner.wait)
        self.addCleanup(leftover.stdin.close)
        # Event wait with a hang marker (fix round 5, M8): a runner that dies before
        # its child opens no writer, and a blocking open would wait for the station cap.
        fd = os.open(fifo, os.O_RDONLY | os.O_NONBLOCK)
        self.addCleanup(os.close, fd)
        got = bytearray()

        def line():
            try:
                got.extend(os.read(fd, 4096))
            except BlockingIOError:
                pass
            return bytes(got) if b"\n" in got else None

        data = kit_time.until("the test's child writes its fd 0 to the probe fifo", line,
                              state=lambda: ("runner rc", runner.poll(), bytes(got)))
        return data.split(b"\n", 1)[0].decode("utf-8").strip()

    def test_a_single_run_gives_its_test_no_stdin_of_the_runner(self):
        self.assertEqual("/dev/null", self.fd_zero_of_the_test(self.SINGLE))

    def test_a_shard_gives_its_test_no_stdin_of_the_runner(self):
        self.assertEqual("/dev/null", self.fd_zero_of_the_test(self.SHARD))


class TestGateLine(unittest.TestCase):
    """(b) the GATE line of a red gate-selftest names the red shard and its log."""

    def test_the_red_reason_carries_the_shard_line(self):
        from pipeline import executor
        with tempfile.TemporaryDirectory() as tmp:
            log = os.path.join(tmp, "run", "strand-gate-selftest.log")
            os.makedirs(os.path.dirname(log))
            with open(log, "w") as fh:
                fh.write(textwrap.dedent("""\
                    FAIL: test_red (fx.b_red.T.test_red)
                    SELFTEST-VERDICT RED 7/8 shards; shard 3/8 1 failed -- log x.shard-3.log -- red fx.b_red.T.test_red; seed 11
                    """))
            run = executor._Run.__new__(executor._Run)
            run.aborted = None
            run.mode = "strand"
            verdict, reason, _ = run._judge({"name": "gate-selftest"}, 1, False, log, None)
        self.assertEqual("RED", verdict)
        self.assertIn("shard 3/8 1 failed -- log x.shard-3.log -- red fx.b_red.T.test_red; seed 11",
                      reason)
        self.assertIn("at fx.b_red.T.test_red", reason)


# --- GH #1419 L2: the table from the gate's own measurement ----------------------
# relgate-0800 (build04, 4 shards): the table WAS applied -- every one of the 35
# station modules had an entry, LPT planned four shards of ~160 s each -- but its
# numbers were another machine's (647.9 s for the station against 3 643 s of shard
# seconds measured) and not even in proportion: the shards ran 609 / 1163 / 1249 /
# 622 s. The subprocess-heavy kit modules weigh far more under a gate's load.

def _shard_log(path, times, wall_scale=1.25, header="$ python3 -m unittest --durations 0"):
    """A shard log as the runner leaves it: unittest's `--durations 0` block, then
    the summary; returns the shard's wall seconds (the tests' sum x `wall_scale`:
    imports and class fixtures are in the wall, not in the test durations)."""
    lines = [header, "." * len(times), "", "Slowest test durations", "-" * 70]
    for tid, secs in sorted(times.items(), key=lambda kv: (-kv[1], kv[0])):
        lines.append("%-10s %s (%s)" % ("%.3fs" % secs, tid.rsplit(".", 1)[1], tid))
    total = sum(times.values())
    lines += ["", "-" * 70, "Ran %d tests in %.3fs" % (len(times), total), "", "OK"]
    pathlib.Path(path).write_text("\n".join(lines) + "\n")
    return total * wall_scale


def _station_log(path, shards, verdict="GREEN"):
    """The station log: one SHARD line per (wall, shard log) and the verdict."""
    n = len(shards)
    lines = ["$ /usr/bin/python3 -I scripts/pipeline/selftest_shard.py unittest ...",
             "SELFTEST-SEED 1"]
    for k, (wall, log) in enumerate(shards, 1):
        lines.append("SHARD %d/%d %s tests=1 fail=0 %.1fs -- log /srv/target/ws/run-x/logs/%s"
                     % (k, n, "GREEN" if verdict == "GREEN" else "RED", wall,
                        os.path.basename(log)))
    lines += ["-" * 70, "Ran 9 tests in 1.0s", "", "OK" if verdict == "GREEN" else "FAILED (errors=1)",
              "SELFTEST-VERDICT %s %d/%d shards" % (verdict, n if verdict == "GREEN" else n - 1, n)]
    pathlib.Path(path).write_text("\n".join(lines) + "\n")


# The measured weights of a station shaped after relgate-0800 (sum 3 643 s): one
# module of eight classes, four heavy kit modules, twenty small ones. The stale
# table has the same names in the proportions of the old table (the kit modules
# understated 5x) -- on paper balanced, measured not.
_HEAVY = "scripts.tests.test_gate_plan"
_TRUE = {"%s.C%d" % (_HEAVY, i): 110.0 for i in range(8)}
_KIT = {"scripts.tests.test_pipeline_gate_cli": 520.0, "scripts.tests.test_strand_sh": 430.0,
        "scripts.tests.test_pipeline_executor": 380.0, "scripts.tests.test_precheck": 260.0}
_SMALL = {"scripts.tests.test_small_%02d" % i: 58.65 for i in range(20)}


def _true_tests():
    """{test id: seconds}: four tests per class, the weights above."""
    out = {}
    for cls, secs in _TRUE.items():
        for j in range(4):
            out["%s.test_%d" % (cls, j)] = secs / 4
    for mod, secs in list(_KIT.items()) + list(_SMALL.items()):
        for j in range(4):
            out["%s.T.test_%d" % (mod, j)] = secs / 4
    return out


def _fake_discover(mod, root):
    if mod != _HEAVY:
        return {"classes": {"T": 4}, "total": 4, "class_fixtures": [],
                "methods": {"T": ["%s.T.test_%d" % (mod, j) for j in range(4)]}}
    classes = {c.rsplit(".", 1)[1]: 4 for c in _TRUE}
    return {"classes": classes, "total": 32, "class_fixtures": [],
            "methods": {c: ["%s.%s.test_%d" % (_HEAVY, c, j) for j in range(4)] for c in classes}}


class MeasuredDurations(unittest.TestCase):

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.logs = pathlib.Path(self._tmp.name) / "logs"
        self.logs.mkdir()

    def measured_run(self, verdict="GREEN", shards=4):
        """A station log + shard logs: the tests spread round-robin over the shards."""
        tests = sorted(_true_tests().items())
        per = [dict(tests[k::shards]) for k in range(shards)]
        entries = []
        for k, times in enumerate(per, 1):
            log = self.logs / ("gate-selftest.shard-%d.log" % k)
            entries.append((_shard_log(log, times), log))
        station = self.logs / "gate-selftest.log"
        _station_log(station, entries, verdict)
        return station

    def loads(self, table, n):
        true = _true_tests()
        mods = sorted({t.rsplit(".", 2)[0] for t in true})
        units, weights, _ = selftest_shard.plan_units(mods, n, table, "/nowhere",
                                                      discover_fn=_fake_discover)
        shards = selftest_shard.distribute(units, n, weights)
        # the true seconds of each shard: its units' tests
        out = []
        for sh in shards:
            s = 0.0
            for u in sh:
                base = u.split("[", 1)[0]
                names = getattr(u, "names", ()) or ()
                if names:
                    s += sum(true[x] for x in names)
                else:
                    s += sum(v for t, v in true.items() if t.startswith(base + "."))
            out.append(s)
        return out

    def test_selftest_shards_follow_the_measured_durations(self):
        table = selftest_shard.measured_table(str(self.measured_run()))
        self.assertIsNotNone(table)
        # the shard's wall goes onto its tests: the table sums the measured seconds
        total = sum(_true_tests().values()) * 1.25
        mods = [k for k in table if not any(k.startswith(m + ".") for m in table if m != k)]
        self.assertAlmostEqual(total, sum(table[m] for m in mods), delta=1.0)
        self.assertAlmostEqual(_KIT["scripts.tests.test_pipeline_gate_cli"] * 1.25,
                               table["scripts.tests.test_pipeline_gate_cli"], delta=0.5)
        # a module of several classes carries its classes (the class mode reads them)
        self.assertAlmostEqual(110.0 * 1.25, table["%s.C3" % _HEAVY], delta=0.5)
        self.assertNotIn("scripts.tests.test_strand_sh.T", table, "one class: no class entry")
        # six shards (build04, nproc // 4): LPT over the measured table stays within
        # 1.2 x the ideal shard -- Plan: 3 020-3 643 s / 6 = 500-600 s instead of 1 250
        loads = self.loads(table, 6)
        ideal = sum(loads) / 6
        self.assertLessEqual(max(loads), 1.2 * ideal, loads)
        # the stale table (kit modules understated 5x) misses that bound
        stale = dict(table)
        for mod in _KIT:
            stale[mod] = table[mod] / 5
        self.assertGreater(max(self.loads(stale, 6)), 1.2 * ideal)
        # without a table: round-robin over the sorted names, as before
        names = ["m%02d" % i for i in range(9)]
        self.assertEqual([names[k::6] for k in range(6)], selftest_shard.distribute(names, 6))

    def test_a_red_or_unmeasured_run_gives_no_table(self):
        self.assertIsNone(selftest_shard.measured_table(str(self.measured_run("RED"))))
        station = self.measured_run()
        # a shard log without the durations block (an older runner, a crash): no table
        (self.logs / "gate-selftest.shard-2.log").write_text("Ran 3 tests in 1.0s\n\nOK\n")
        self.assertIsNone(selftest_shard.measured_table(str(station)))
        # one shard (the command of before) prints no SHARD line
        one = self.logs / "one.log"
        one.write_text("Ran 3 tests in 1.0s\n\nOK\n")
        self.assertIsNone(selftest_shard.measured_table(str(one)))

    def test_the_merge_keeps_one_scale(self):
        old = {"a.m": 10.0, "a.m.X": 6.0, "a.m.Y": 4.0, "a.other": 2.0, "a.other.Z": 1.0}
        new = selftest_shard.merge_table(old, {"a.m": 50.0, "a.m.X": 30.0, "a.m.W": 20.0})
        # measured entries replace the module's old ones (Y is gone with the class)
        self.assertEqual({"a.m": 50.0, "a.m.X": 30.0, "a.m.W": 20.0},
                         {k: v for k, v in new.items() if k.startswith("a.m")})
        # an unmeasured module keeps its entry on the new scale (50 / 10 = 5x)
        self.assertEqual((10.0, 5.0), (new["a.other"], new["a.other.Z"]))

    def test_the_shards_measure_their_tests(self):
        argv = selftest_shard.shard_argv(["/usr/bin/python3", "-I"], ["fx.a", "fx.b"])
        self.assertEqual(["fx.a", "fx.b"], argv[-2:])
        if sys.version_info >= (3, 12):
            self.assertEqual(["--durations", "0"], argv[-4:-2])
        self.assertIn("unittest", argv)


class MeasuredDurationsCli(ShardCase):
    """The verb `durations --from-log` and a real two-shard run (fixture modules)."""

    def test_a_real_run_measures_and_the_verb_writes_the_table(self):
        if sys.version_info < (3, 12):
            self.skipTest("unittest --durations needs Python 3.12")
        # unittest hides durations under 1 ms: these tests compute for longer
        slow = GREEN_CASE.replace("self.assertTrue(True)",
                                  "self.assertTrue(sum(range(10 ** 6)))")
        mods = [self.module("a_ok", slow), self.module("c_ok", slow)]
        station = self.root / "station.log"
        with open(station, "w") as fh:
            res = self.run_runner(["unittest"] + mods, 2, stdout=fh)
        self.assertEqual(0, res.returncode, station.read_text())
        table = selftest_shard.measured_table(str(station))
        self.assertIsNotNone(table, station.read_text())
        self.assertEqual({"fx.a_ok", "fx.c_ok"}, set(table))
        into = self.root / "old.json"
        into.write_text(json.dumps({"fx.a_ok": 1.0, "fx.gone": 3.0}))
        out = self.root / "new.json"
        got = subprocess.run(lane.python_argv(RUNNER, "durations", "--from-log", str(station),
                                              "--into", str(into), "--write", str(out)),
                             cwd=self.root, capture_output=True, text=True,
                             timeout=kit_time.HANG)
        self.assertEqual(0, got.returncode, got.stderr)
        written = json.loads(out.read_text())
        self.assertEqual({"fx.a_ok", "fx.c_ok", "fx.gone"}, set(written))
        self.assertEqual(table["fx.a_ok"], written["fx.a_ok"])

    def test_a_red_run_writes_nothing(self):
        station = self.root / "station.log"
        _station_log(station, [(1.0, self.root / "s1.log"), (1.0, self.root / "s2.log")], "RED")
        out = self.root / "new.json"
        got = subprocess.run(lane.python_argv(RUNNER, "durations", "--from-log", str(station),
                                              "--write", str(out)),
                             cwd=self.root, capture_output=True, text=True,
                             timeout=kit_time.HANG)
        self.assertEqual(1, got.returncode, got.stderr)
        self.assertFalse(out.exists())
        self.assertIn("nothing written", got.stderr)


class AGreenGateWritesTheSelftestDurationsItMeasured(unittest.TestCase):
    """Test 3 (GH #1419 L2): the gate's archive keeps the table its GREEN
    gate-selftest measured (`<run>/selftest_durations.json`, the committed table
    merged with the measurement); a red run writes nothing."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.tmp = pathlib.Path(self._tmp.name)

    def archive(self, verdict):
        from pipeline import gate
        dest = self.tmp / ("ws-" + verdict)
        logs = dest / "logs"
        logs.mkdir(parents=True)
        times = _true_tests()
        tests = sorted(times.items())
        entries = []
        for k in (1, 2):
            log = logs / ("gate-selftest.shard-%d.log" % k)
            entries.append((_shard_log(log, dict(tests[k - 1::2])), log))
        _station_log(logs / "gate-selftest.log", entries, verdict)
        repo = self.tmp / "repo"
        (repo / "scripts" / "pipeline").mkdir(parents=True, exist_ok=True)
        (repo / "scripts" / "pipeline" / "selftest_durations.json").write_text(
            json.dumps({"scripts.tests.test_unmeasured": 2.0, _HEAVY: 100.0}))
        o = types.SimpleNamespace(archive=str(self.tmp / ("archive-" + verdict)), mode="strand")
        p = gate._Pass(o, str(repo), {"rev": "c" * 40, "stations": []}, io.StringIO(), io.StringIO())
        v2 = {"stations": [{"name": "gate-selftest", "verdict": verdict}], "rev": "c" * 40}
        p.archive(v2, str(dest), ["GATE-SUMMARY strand c 1/1 1s %s" % verdict])
        return pathlib.Path(o.archive) / p.rid, p

    def test_a_green_gate_writes_the_selftest_durations_it_measured(self):
        adir, p = self.archive("GREEN")
        path = adir / "selftest_durations.json"
        self.assertTrue(path.is_file(), p.err.getvalue())
        table = json.loads(path.read_text())
        measured = selftest_shard.measured_table(str(adir / "logs" / "gate-selftest.log"))
        for k, v in measured.items():
            self.assertEqual(v, table[k], k)
        self.assertIn("scripts.tests.test_unmeasured", table, "the committed rest stays")

    def test_a_red_gate_writes_nothing(self):
        adir, _ = self.archive("RED")
        self.assertTrue((adir / "receipt.json").is_file())
        self.assertFalse((adir / "selftest_durations.json").exists())


if __name__ == "__main__":
    unittest.main()
