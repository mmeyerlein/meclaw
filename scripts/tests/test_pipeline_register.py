"""Tests of the satellite station register (`scripts/pipeline/register.py`, W10, GH #1124).

Every case builds a small satellite repository in a temporary directory: a
`pipeline/stations.json`, a main branch and a strand branch with a diff. The
transport cases (`kit_push_spec`, `mirrors_spec`, `assets_spec`) run their
rsync argv lists against a local destination and read what arrived.
"""

import contextlib
import io
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

from scripts.tests import kit_time, store_isolation

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS))

from pipeline import register  # noqa: E402
from pipeline import cli  # noqa: E402


# gate-selftest loads every module into one process: no fixture event of this
# module may reach the metrics target of the caller's environment.
_METRICS_URL = mock.patch.dict(os.environ, {"MECLAW_PIPELINE_METRICS_URL": ""})


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()
    _METRICS_URL.start()


def tearDownModule():  # noqa: N802 -- unittest's name
    _METRICS_URL.stop()
    store_isolation.restore()

# The field set of W3's `plan.build_plan` (scripts/pipeline/plan.py on the
# W3 branch), written down here on purpose: the plan-form cases hold the
# register against this copy, not against plan.py (W3Hooks imports plan.py).
W3_PLAN_FIELDS = {"mode", "rev", "base", "repo", "classes", "crates", "stations",
                  "plan_reason", "critical_path_s", "critical_path"}
W3_STATION_FIELDS = {"name", "scope", "cargo", "cmds", "env", "reason", "stage", "needs",
                     "inputs", "timeout_min", "run", "keep_ws", "after", "when"}
W3_STATION_OPTIONAL = {"parallel"}
W3_NEEDS_KEYS = {"cpu", "colony", "browser_chromium", "browser_webkit", "export", "cargo"}
W3_STAGE_LETTERS = {"S", "I", "R", "C"}


def write(root, rel, text):
    path = os.path.join(root, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(text)


def git(root, *args):
    return subprocess.run(["git", "-C", root] + list(args), check=True,
                          capture_output=True, text=True).stdout.strip()


def station(name, inputs, stage=("strand", "integration", "release"), **extra):
    st = {"name": name, "scope": "all", "stage": list(stage), "cargo": 0,
          "cmds": [{"argv": ["{python}", "-c", "print('%s')" % name], "cwd": "."}],
          "needs": {"cpu": 1}, "inputs": list(inputs), "after": [], "timeout_min": 5}
    st.update(extra)
    return st


def registry(stations, repo="coding"):
    return {"schema": "meclaw.pipeline.stations/1", "repo": repo, "stations": stations}


UNIT = station(
    "unit", ["scripts/**", "tests/**"],
    cmds=[{"argv": ["{python}", "{pipeline}/unit_runner.py", "--root", "{root}", "--by", "file",
                    "--workers", "{workers}", "--include", "tests/test_*.py",
                    "--slots", "{slots}"], "cwd": "{root}"}],
    parallel={"by": "file", "workers": 4, "slot_env": "CODING_LAB_SLOT"},
    needs={"colony": 1})
DOCS = station("docs", ["docs/**"], stage=("strand", "integration"))
SETS = station("set", ["sets/*.json"], after=["unit"])
LINT = station("lint", [], always=True)
RELEASE_ONLY = station("release-check", ["CHANGELOG.md"], stage=("release",))


class Satellite(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = os.path.join(self._tmp.name, "coding")
        os.makedirs(self.root)
        write(self.root, "pipeline/stations.json",
              json.dumps(registry([UNIT, DOCS, SETS, LINT, RELEASE_ONLY]), indent=2) + "\n")
        write(self.root, "scripts/a.py", "A = 1\n")
        write(self.root, "docs/a.md", "# a\n")
        git(self.root, "init", "-q", "-b", "main")
        git(self.root, "config", "user.email", "t@example.invalid")
        git(self.root, "config", "user.name", "t")
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", "base")
        self.main = git(self.root, "rev-parse", "HEAD")
        git(self.root, "checkout", "-q", "-b", "strand")

    def tearDown(self):
        self._tmp.cleanup()

    def commit(self, rel, text):
        write(self.root, rel, text)
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", "change " + rel)


class Resolve(Satellite):
    def test_register_resolves_globs_against_diff(self):
        reg = register.load_register(self.root)
        picked = register.resolve(reg, ["scripts/a.py"], "strand")
        self.assertEqual(["unit", "lint"], [s["name"] for s in picked])
        self.assertEqual(["glob:scripts/**: scripts/a.py"], picked[0]["reason"])
        picked = register.resolve(reg, ["docs/a.md", "sets/0.2.json"], "strand")
        self.assertEqual(["docs", "set", "lint"], [s["name"] for s in picked])
        # the diff of a real branch decides: base = merge-base with main
        self.commit("docs/b.md", "# b\n")
        plan = register.build_plan(self.root, "strand", None)
        self.assertEqual(self.main, plan["base"])
        self.assertEqual(["docs", "lint"], [s["name"] for s in plan["stations"]])
        self.assertEqual(["glob:docs/**: docs/b.md"], plan["plan_reason"]["docs"])
        # the dirty tree counts, like meclaw's gate
        write(self.root, "tests/test_x.py", "")
        plan = register.build_plan(self.root, "strand", None)
        self.assertIn("unit", [s["name"] for s in plan["stations"]])
        # the command line reads the file list from stdin
        out = io.StringIO()
        with contextlib.redirect_stdout(out), \
                mock.patch("sys.stdin", io.StringIO("sets/0.2.json\n")):
            rc = cli.main(["register", "plan", "--repo", self.root, "--files-from", "-"])
        self.assertEqual(0, rc)
        doc = json.loads(out.getvalue())
        self.assertEqual(["set", "lint"], [s["name"] for s in doc["stations"]])
        self.assertEqual("coding", doc["repo"])

    def test_a_committed_rename_lists_both_sides(self):
        # Re-check tb-s2 h1 m-N2: without --no-renames only the target was
        # listed -- a station matching only the source was never planned.
        git(self.root, "mv", "scripts/a.py", "docs/a_moved.py")
        git(self.root, "commit", "-q", "-m", "move")
        self.assertEqual(["docs/a_moved.py", "scripts/a.py"],
                         register.changed_files(self.root, self.main))
        plan = register.build_plan(self.root, "strand", None)
        self.assertIn("unit", [s["name"] for s in plan["stations"]])

    def test_register_always_stations_and_release_all(self):
        reg = register.load_register(self.root)
        picked = register.resolve(reg, [], "strand")
        self.assertEqual(["lint"], [s["name"] for s in picked])
        self.assertEqual(["always"], picked[0]["reason"])
        picked = register.resolve(reg, [], "release")
        self.assertEqual(["unit", "set", "lint", "release-check"], [s["name"] for s in picked])
        self.assertTrue(all(s["reason"] == ["release: all stations"] for s in picked))
        picked = register.resolve(reg, [], "strand", all_stations=True)
        self.assertEqual(["unit", "docs", "set", "lint"], [s["name"] for s in picked])
        # release-only stations never run in a strand, even when the diff hits them
        picked = register.resolve(reg, ["CHANGELOG.md"], "integration")
        self.assertEqual(["lint"], [s["name"] for s in picked])
        plan = register.build_plan(self.root, "release", None)
        self.assertEqual(["unit", "set", "lint", "release-check"],
                         [s["name"] for s in plan["stations"]])
        self.assertEqual(["unit"], next(s for s in plan["stations"]
                                        if s["name"] == "set")["after"])

    def test_register_rejects_bad_schema_and_repo(self):
        for bad in ({"schema": "x/1", "repo": "coding", "stations": []},
                    {"schema": "meclaw.pipeline.stations/1", "repo": "meclaw", "stations": []},
                    registry([station("a", []), station("a", [])]),
                    registry([station("a", [], stage=("nightly",))]),
                    registry([station("a", [], parallel={"by": "row", "workers": 2})]),
                    registry([station("a", [], after=["ghost"])])):
            write(self.root, "pipeline/stations.json", json.dumps(bad))
            with self.assertRaises(register.RegisterError, msg=json.dumps(bad)):
                register.load_register(self.root)

    def test_register_is_satellite(self):
        self.assertTrue(register.is_satellite(self.root))
        write(self.root, "scripts/gate_plan.py", "")
        self.assertFalse(register.is_satellite(self.root))
        self.assertFalse(register.is_satellite(self._tmp.name))


class LaneEnv(Satellite):
    """welle-tb tb-zcol A: the kit hands the lane only the variables a
    satellite's register declares (`lane_env`), by exact name, fail-closed --
    in Z SOUL2ME_LAB_MECLAW_BIN never reached the lab (only the golden
    switches travelled) and the set's release binary ran instead."""

    def declare(self, names):
        reg = registry([UNIT, DOCS, SETS, LINT, RELEASE_ONLY])
        reg["lane_env"] = names
        write(self.root, "pipeline/stations.json", json.dumps(reg))

    def test_lane_env_carries_declared_names_that_are_set_here(self):
        self.declare(["SOUL2ME_LAB_MECLAW_BIN", "CODING_LAB_MODE"])
        reg = register.load_register(self.root)
        environ = {"SOUL2ME_LAB_MECLAW_BIN": "/srv/target/bench/x/meclaw", "HOME": "/h",
                   "OTHER": "y"}
        self.assertEqual({"SOUL2ME_LAB_MECLAW_BIN": "/srv/target/bench/x/meclaw"},
                         register.lane_env(reg, environ))

    def test_lane_env_without_declaration_carries_nothing(self):
        reg = register.load_register(self.root)
        self.assertEqual({}, register.lane_env(reg, {"SOUL2ME_LAB_MECLAW_BIN": "/x"}))

    def test_lane_env_refuses_secret_kit_and_malformed_names(self):
        for bad in ("nope", ["lower_case"], ["OPENAI_API_KEY"], ["GH_TOKEN"],
                    ["VAULT_PASSWORD"], ["MY_SECRET_DIR"], ["MECLAW_GATE_MODE"],
                    ["CARGO_TARGET_DIR"], ["PATH"], ["LD_PRELOAD"], ["X=1"], [3],
                    ["A_B", "A_B"]):
            self.declare(bad)
            with self.assertRaises(register.RegisterError, msg=json.dumps(bad)):
                register.load_register(self.root)

    def test_lane_env_refuses_every_secret_spelling(self):
        # Re-Check M1 (post-xhigh-8): the filter matches substrings, case
        # blind -- these all slipped through the old part-wise filter.
        for bad in ("OPENAI_APIKEY", "API_KEYS", "AWS_ACCESSKEY", "PRIVATEKEY",
                    "GPG_PASSPHRASE", "DBPASS", "GITHUB_OAUTH", "GH_PAT", "JWT",
                    "BEARER", "SIGNING_SALT", "DB_CREDS", "TLS_CERT_FILE",
                    "MYTOKENS", "X_SECRETS",
                    # PRIVATE alone, no KEY beside it and not as a `_` part
                    "MYPRIVATEDIR"):
            self.declare([bad])
            with self.assertRaises(register.RegisterError, msg=bad):
                register.load_register(self.root)
        for word in ("key", "Token", "jwt"):
            self.assertTrue(register.reads_like_secret("my_%s_x" % word), word)
        self.assertFalse(register.reads_like_secret("SOUL2ME_LAB_MECLAW_BIN"))

    def test_lane_env_refuses_a_value_that_cannot_travel(self):
        self.declare(["SOUL2ME_LAB_MECLAW_BIN"])
        reg = register.load_register(self.root)
        for value in ("/a\n/b", "/a\x00b"):
            with self.assertRaises(register.RegisterError):
                register.lane_env(reg, {"SOUL2ME_LAB_MECLAW_BIN": value})

    def test_gate_lane_env_is_empty_for_the_core_and_read_from_the_register(self):
        from pipeline import gate
        self.declare(["SOUL2ME_LAB_MECLAW_BIN"])
        env = {"SOUL2ME_LAB_MECLAW_BIN": "/lane/bin"}
        self.assertEqual({}, gate._lane_env(self.root, False, env))
        self.assertEqual(env, gate._lane_env(self.root, True, env))


class PlanForm(Satellite):
    def test_register_plan_form_matches_plan_py(self):
        plan = register.build_plan(self.root, "release", None, pipeline_dir="/kit/scripts/pipeline",
                                   python="/usr/bin/python3", slots=["7930", "7931"])
        self.assertEqual(W3_PLAN_FIELDS, set(plan))
        self.assertEqual("release", plan["mode"])
        self.assertEqual(git(self.root, "rev-parse", "HEAD"), plan["rev"])
        self.assertEqual("coding", plan["repo"])
        self.assertEqual([], plan["classes"])
        self.assertEqual([], plan["crates"])
        self.assertIsInstance(plan["critical_path_s"], float)
        self.assertEqual(["unit", "set"], plan["critical_path"])
        self.assertEqual(600.0, plan["critical_path_s"])
        for st in plan["stations"]:
            self.assertTrue(W3_STATION_FIELDS <= set(st) <= W3_STATION_FIELDS | W3_STATION_OPTIONAL,
                            sorted(set(st) ^ W3_STATION_FIELDS))
            self.assertEqual(W3_NEEDS_KEYS, set(st["needs"]))
            self.assertTrue(set(st["stage"]) <= W3_STAGE_LETTERS)
            self.assertEqual("none", st["env"])
            self.assertEqual({"outcome": "success"}, st["when"])
            self.assertIs(True, st["run"])
            self.assertIs(False, st["keep_ws"])
            for cmd in st["cmds"]:
                self.assertEqual({"argv", "cwd"}, set(cmd))
                self.assertNotIn("{", " ".join(cmd["argv"]) + cmd["cwd"])
            self.assertEqual(plan["plan_reason"][st["name"]], st["reason"])
        unit = plan["stations"][0]
        self.assertEqual(["S", "I", "R"], unit["stage"])
        self.assertEqual(["glob:scripts/**", "glob:tests/**", "tool:python"], unit["inputs"])
        # two slots cap the four workers at two, and cpu follows the workers
        self.assertEqual({"by": "file", "workers": 2, "slot_env": "CODING_LAB_SLOT"},
                         unit["parallel"])
        self.assertEqual({"cpu": 2, "colony": 1, "browser_chromium": 0, "browser_webkit": 0,
                          "export": 0, "cargo": 0}, unit["needs"])
        argv = unit["cmds"][0]["argv"]
        self.assertEqual(["/usr/bin/python3", "/kit/scripts/pipeline/unit_runner.py", "--root",
                          os.path.abspath(self.root)], argv[:4])
        self.assertEqual("2", argv[argv.index("--workers") + 1])
        self.assertEqual("7930,7931", argv[argv.index("--slots") + 1])
        self.assertEqual(os.path.abspath(self.root), unit["cmds"][0]["cwd"])
        # default placeholders: this interpreter and this core's package
        plan = register.build_plan(self.root, "release", None, **LANE)
        argv = plan["stations"][0]["cmds"][0]["argv"]
        self.assertEqual([sys.executable,
                          os.path.join(os.path.dirname(register.__file__), "unit_runner.py")],
                         argv[:2])
        self.assertEqual("0,1,2,3", argv[argv.index("--slots") + 1])


class SetOrder(unittest.TestCase):
    """W10 part B (F1): `set` rebuilds build/lib (rmtree + rename) while a colony
    station copies from it -- coding's lab-boot and apply went RED on a lane."""

    def plan(self, stations):
        with tempfile.TemporaryDirectory() as tmp:
            root = os.path.join(tmp, "coding")
            write(root, "pipeline/stations.json", json.dumps(registry(stations)) + "\n")
            plan = register.build_plan(root, "release", None, files=[])
        return {s["name"]: s for s in plan["stations"]}

    def test_register_colony_station_runs_after_set(self):
        by = self.plan([station("unit", ["x/**"], needs={"colony": 0}),
                        station("set", ["sets/**"]),
                        station("apply", ["x/**"], needs={"colony": 1}),
                        station("lab", ["x/**"], needs={"colony": 2})])
        self.assertEqual([], by["unit"]["after"])
        self.assertEqual(["set"], by["apply"]["after"])
        # G1: two colony stations without slots run in register order
        self.assertEqual(["set", "apply"], by["lab"]["after"])
        self.assertEqual([], by["set"]["after"])

    def test_register_station_before_set_keeps_its_order(self):
        # set already waits for it: serial either way, and no cycle
        by = self.plan([station("probe", ["x/**"], needs={"colony": 1}),
                        station("set", ["sets/**"], after=["probe"]),
                        station("lab", ["x/**"], needs={"colony": 2}, after=["probe"])])
        self.assertEqual([], by["probe"]["after"])
        self.assertEqual(["probe", "set"], by["lab"]["after"])

    def test_register_no_set_station_adds_nothing(self):
        by = self.plan([station("lab", ["x/**"], needs={"colony": 2})])
        self.assertEqual([], by["lab"]["after"])


class LibField(unittest.TestCase):
    """GH #1142 (1)+(2): the role towards build/lib is an explicit field
    (`lib: build|read|none`) instead of the name `set` plus needs.colony; a
    station that reads build/lib without booting a colony (coding's unit)
    declares `read` and gets the colony order."""

    def plan(self, stations):
        with tempfile.TemporaryDirectory() as tmp:
            root = os.path.join(tmp, "coding")
            write(root, "pipeline/stations.json", json.dumps(registry(stations)) + "\n")
            plan = register.build_plan(root, "release", None, files=[])
        return {s["name"]: s for s in plan["stations"]}

    def test_register_lib_field_names_builder_and_readers(self):
        by = self.plan([station("unit", ["x/**"], needs={"colony": 0}, lib="read"),
                        station("resolve", ["sets/**"], lib="build"),
                        station("apply", ["x/**"], needs={"colony": 1}),
                        station("probe", ["x/**"], needs={"colony": 1}, lib="none")])
        self.assertEqual(["resolve"], by["unit"]["after"])
        self.assertEqual(["resolve", "unit"], by["apply"]["after"])
        self.assertEqual([], by["probe"]["after"])
        self.assertEqual([], by["resolve"]["after"])

    def test_register_lib_none_takes_set_out_of_the_builder_role(self):
        by = self.plan([station("set", ["sets/**"], lib="none"),
                        station("apply", ["x/**"], needs={"colony": 1})])
        self.assertEqual([], by["apply"]["after"])

    def test_register_rejects_an_unknown_lib_role(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = os.path.join(tmp, "coding")
            write(root, "pipeline/stations.json",
                  json.dumps(registry([station("a", ["x/**"], lib="write")])) + "\n")
            with self.assertRaises(register.RegisterError):
                register.load_register(root)


class OnlyLikeTheCore(unittest.TestCase):
    """GH #1142 (3), KF2: `gate --repo ... --only` with W3's semantics -- an
    unknown name refuses, the others stay in the plan with `run: false`, and
    the critical path is the one of the stations that run."""

    def plan(self, only):
        stations = [station("a", ["x/**"], timeout_min=1),
                    station("b", ["x/**"], timeout_min=10, after=["a"])]
        with tempfile.TemporaryDirectory() as tmp:
            root = os.path.join(tmp, "coding")
            write(root, "pipeline/stations.json", json.dumps(registry(stations)) + "\n")
            return register.build_plan(root, "release", None, files=[], only=only)

    def test_register_only_marks_the_rest_run_false(self):
        doc = self.plan(["a"])
        by = {s["name"]: s for s in doc["stations"]}
        self.assertEqual(["a", "b"], sorted(by))
        self.assertIs(by["b"]["run"], False)
        self.assertNotEqual(False, by["a"].get("run"))
        self.assertEqual(["a"], doc["critical_path"])
        self.assertEqual(60.0, doc["critical_path_s"])
        self.assertEqual(["a"], doc["only"])

    def test_register_only_refuses_an_unknown_station(self):
        with self.assertRaises(register.RegisterError):
            self.plan(["nope"])


def slot_station(name, workers, colony, **extra):
    return station(name, ["x/**"], needs={"cpu": workers, "colony": colony},
                   cmds=[{"argv": ["{python}", "run.py", "--slots", "{slots}"], "cwd": "."}],
                   parallel={"by": "module", "workers": workers, "slot_env": "X_SLOT"}, **extra)


# The lane these plans are made for. Without a pool the lane's physical cores
# are the pool (GH #1163); the cases took this host's `lscpu` and were red on the
# public runner, 4 vCPU on 2 cores (GH #1199).
LANE = {"lane_cpu": 16, "lane_cores": 8}


def _plan_of(stations, slots=None):
    with tempfile.TemporaryDirectory() as tmp:
        root = os.path.join(tmp, "coding")
        write(root, "pipeline/stations.json", json.dumps(registry(stations)) + "\n")
        plan = register.build_plan(root, "release", None, files=[], slots=slots, **LANE)
    return {s["name"]: s for s in plan["stations"]}


def _slots_of(st):
    return st["cmds"][0]["argv"][-1].split(",")


class LibBuilderOrder(unittest.TestCase):
    """W10 part B (G1): every colony station resolves the set's library and
    rebuilds build/lib (rmtree + rename); coding's apply and lab-boot ran side
    by side after `set` and apply read a half-removed lib (g3 GREEN, g4 RED)."""

    def test_register_slot_colony_station_runs_after_the_oneshot_colony_station(self):
        by = _plan_of([station("set", ["sets/**"]),
                       station("apply", ["x/**"], needs={"colony": 1}),
                       slot_station("lab", 2, 2)])
        self.assertEqual(["set"], by["apply"]["after"])
        self.assertEqual(["set", "apply"], by["lab"]["after"])

    def test_register_oneshot_colony_stations_run_in_register_order(self):
        by = _plan_of([station("probe", ["x/**"], needs={"colony": 1}),
                       station("apply", ["x/**"], needs={"colony": 1})])
        self.assertEqual([], by["probe"]["after"])
        self.assertEqual(["probe"], by["apply"]["after"])

    def test_register_oneshot_that_waits_for_the_slot_station_keeps_its_order(self):
        by = _plan_of([slot_station("lab", 2, 2),
                       station("apply", ["x/**"], needs={"colony": 1}, after=["lab"])])
        self.assertEqual([], by["lab"]["after"])
        self.assertEqual(["lab"], by["apply"]["after"])

    def test_register_station_without_colony_adds_nothing(self):
        by = _plan_of([station("unit", ["x/**"], needs={"colony": 0}),
                       slot_station("lab", 2, 2)])
        self.assertEqual([], by["lab"]["after"])


class SlotRanges(unittest.TestCase):
    """W10 part B (G2): soul2me's lab and lab-ops both got the slots from 0 and
    the colony pool (4 + 2 <= 8) let them run side by side: `slot 0 is locked`."""

    def test_register_slot_stations_get_disjoint_ranges_without_a_pool(self):
        by = _plan_of([slot_station("lab", 4, 4), slot_station("lab-ops", 1, 2)])
        self.assertEqual(["0", "1", "2", "3"], _slots_of(by["lab"]))
        # GH #1163: a station is handed every colony it books (lab-ops: one worker, two colonies)
        self.assertEqual(["4", "5"], _slots_of(by["lab-ops"]))
        self.assertEqual([], by["lab-ops"]["after"])

    def test_register_station_gets_a_slot_for_every_colony_it_books(self):
        """GH #1163 (welle-tb S5): orga's peer-pair modules run on one worker that holds two colonies at
        once; the register handed one slot per worker, so the second booked colony had no name and the
        pair could only be derived by convention ((n, n+4) on a fixed pool of eight). The station now
        gets one slot per booked colony, at least one per worker -- disjoint from its neighbours."""
        by = _plan_of([slot_station("lab", 6, 6), slot_station("lab-pair", 1, 2)])
        self.assertEqual([str(i) for i in range(6)], _slots_of(by["lab"]))
        self.assertEqual(["6", "7"], _slots_of(by["lab-pair"]))
        self.assertEqual([], by["lab-pair"]["after"])
        by = _plan_of([slot_station("lab-pair", 1, 2), slot_station("lab", 2, 2)],
                      slots=["10", "11", "12", "13"])
        self.assertEqual(["10", "11"], _slots_of(by["lab-pair"]))
        self.assertEqual(["12", "13"], _slots_of(by["lab"]))
        self.assertEqual([], by["lab"]["after"])
        # the workers stay the workers: never more than the station names
        argv = by["lab-pair"]["cmds"][0]["argv"]
        self.assertEqual("1", str(by["lab-pair"]["needs"]["cpu"]))
        self.assertEqual("10,11", argv[-1])

    def test_register_slot_stations_get_disjoint_ranges_from_the_pool(self):
        by = _plan_of([slot_station("lab", 2, 2), slot_station("lab-ops", 1, 1)],
                      slots=["10", "11", "12", "13"])
        self.assertEqual(["10", "11"], _slots_of(by["lab"]))
        self.assertEqual(["12"], _slots_of(by["lab-ops"]))
        self.assertEqual([], by["lab-ops"]["after"])

    def test_register_exhausted_pool_wraps_and_waits(self):
        by = _plan_of([slot_station("lab", 2, 2), slot_station("lab-ops", 2, 2)],
                      slots=["0", "1", "2"])
        self.assertEqual(["0", "1"], _slots_of(by["lab"]))
        self.assertEqual(["0", "1"], _slots_of(by["lab-ops"]))
        self.assertEqual(["lab"], by["lab-ops"]["after"])


# --- the colony density (GH #1163, welle-tb S5) -----------------------------------

def density_station(name, cpw=None, **extra):
    par = {"by": "module", "workers": "density", "slot_env": "X_SLOT"}
    if cpw is not None:
        par["colonies_per_worker"] = cpw
    return station(name, ["x/**"], needs={},
                   cmds=[{"argv": ["{python}", "run.py", "--workers", "{workers}",
                                   "--slots", "{slots}"], "cwd": "."}],
                   parallel=par, **extra)


def _density_plan(stations, lane_cpu=12, environ=None, slots=None, workers=None, lane_cores=6,
                  colony_slots=None):
    reg = registry(stations)
    if colony_slots is not None:
        reg["colony_slots"] = colony_slots
    with tempfile.TemporaryDirectory() as tmp:
        root = os.path.join(tmp, "orga")
        write(root, "pipeline/stations.json", json.dumps(reg) + "\n")
        return register.build_plan(root, "release", None, files=[], slots=slots,
                                   workers=workers, lane_cpu=lane_cpu, lane_cores=lane_cores,
                                   environ={} if environ is None else environ)


def _stations(plan):
    return {s["name"]: s for s in plan["stations"]}


def cores_station(name, **extra):
    return station(name, ["x/**"], needs={},
                   cmds=[{"argv": ["{python}", "run.py", "--workers", "{workers}"], "cwd": "."}],
                   parallel={"by": "file", "workers": "cores"}, **extra)


LSCPU_BUILD03 = "# Core,Socket\n" + "".join("%d,0\n" % (i % 6) for i in range(12))
LSCPU_BUILD04 = "# Core,Socket\n" + "".join("%d,%d\n" % (i % 6, (i // 6) % 2) for i in range(24))


class ColonyDensity(unittest.TestCase):
    """GH #1163 (welle-tb S5): build03 (6 cores + HT, nproc 12) ran eight colonies
    beside eight unit workers -- soul2me lab 9/15 RED (peer_book acks missing),
    orga's lab sum +35 %. Hardware ruling 10.10.: the density follows the PHYSICAL cores
    (one colony per real core; unknown cores count as nproc / 2), a unit
    station never runs beside a full lab, and the plan names cores and density."""

    def test_register_counts_physical_cores(self):
        self.assertEqual(6, register.count_cores(LSCPU_BUILD03))
        self.assertEqual(12, register.count_cores(LSCPU_BUILD04))
        for bad in ("", "# Core,Socket\n", "no cpu here\n", None):
            self.assertIsNone(register.count_cores(bad))

    def test_register_density_is_one_colony_per_real_core(self):
        """welle-tb S5 (owner's hardware ruling): CPU-bound parallelism counts real
        cores -- the density is the lane's real cores, one colony each (build01-03:
        6 real cores of 12 vCPU, build04: 12 of 24; no fixed cap behind the cores)."""
        self.assertEqual(1, register.CORES_PER_COLONY)
        self.assertEqual((6, "lane cores 6, one colony per core"),
                         register.colony_density(12, environ={}, lane_cores=6))
        self.assertEqual((12, "lane cores 12, one colony per core"),
                         register.colony_density(24, environ={}, lane_cores=12))
        self.assertEqual(1, register.colony_density(2, environ={}, lane_cores=1)[0])
        # cores unknown: nproc / 2
        self.assertEqual((6, "lane cores 6 (unknown: nproc 12 / 2), one colony per core"),
                         register.colony_density(12, environ={}))

    def test_register_density_is_capped_only_by_the_pool(self):
        """The old cap of 8 colonies (W3 start value, OR-DT-11, never measured) is gone:
        only a slot pool -- the CLI's `--slots` or the satellite's `colony_slots` (its
        lab ports) -- caps the density."""
        self.assertFalse(hasattr(register, "COLONY_CAPACITY"))
        self.assertEqual(32, register.colony_density(64, environ={}, lane_cores=32)[0])
        self.assertEqual(3, register.colony_density(24, environ={}, pool=["a", "b", "c"],
                                                    lane_cores=12)[0])
        self.assertEqual((8, "MECLAW_COLONY_DENSITY=8"),
                         register.colony_density(12, environ={register.DENSITY_ENV: "8"},
                                                 lane_cores=6))

    def test_register_build04_runs_twelve_colonies_unless_the_satellite_has_fewer_slots(self):
        """build04 (cores=12) runs 12 colonies; a satellite whose lab owns fewer port
        slots (`colony_slots`, orga and soul2me: 8) caps it there, and says so."""
        def slots_of(st):
            argv = st["cmds"][0]["argv"]
            return argv[argv.index("--slots") + 1]
        st = _stations(_density_plan([density_station("lab")], lane_cpu=24, lane_cores=12))["lab"]
        self.assertEqual((12, 12), (st["parallel"]["workers"], st["needs"]["colony"]))
        self.assertEqual(",".join(str(i) for i in range(12)), slots_of(st))
        plan = _density_plan([density_station("lab")], lane_cpu=24, lane_cores=12, colony_slots=8)
        st = _stations(plan)["lab"]
        self.assertEqual((8, 8), (st["parallel"]["workers"], st["needs"]["colony"]))
        self.assertEqual("0,1,2,3,4,5,6,7", slots_of(st))
        self.assertIn("capped at 8 colony slots", " ".join(plan.get("notes") or []))
        for bad in (0, -1, "8", 2.5, True):
            with self.assertRaises(register.RegisterError):
                _density_plan([density_station("lab")], lane_cpu=24, lane_cores=12, colony_slots=bad)

    def test_register_density_env_overrides_and_fails_closed(self):
        env = {register.DENSITY_ENV: "6"}
        self.assertEqual("MECLAW_COLONY_DENSITY", register.DENSITY_ENV)
        self.assertEqual((6, "MECLAW_COLONY_DENSITY=6"),
                         register.colony_density(12, environ=env, lane_cores=6))
        for bad in ("x", "0", "-2", "2.5", ""):
            with self.assertRaises(register.RegisterError) as ctx:
                register.colony_density(12, environ={register.DENSITY_ENV: bad})
            self.assertIn("MECLAW_COLONY_DENSITY", str(ctx.exception))

    def test_register_density_station_holds_the_lane(self):
        lab = _stations(_density_plan([density_station("lab")]))["lab"]
        self.assertEqual((6, 6, 12), (lab["parallel"]["workers"], lab["needs"]["colony"],
                                      lab["needs"]["cpu"]))
        self.assertEqual([str(i) for i in range(6)], _slots_of(lab))
        self.assertEqual("6", lab["cmds"][0]["argv"][3])

    def test_register_density_station_with_two_colonies_per_worker(self):
        pair = _stations(_density_plan([density_station("lab-pair", cpw=2)],
                                       slots=["10", "11", "12", "13", "14"]))["lab-pair"]
        # D 6 capped at the pool's 5 holds two pairs; their share of the lane: 12 * 4 / 5
        self.assertEqual((2, 4, 10), (pair["parallel"]["workers"], pair["needs"]["colony"],
                                      pair["needs"]["cpu"]))
        self.assertEqual(["10", "11", "12", "13"], _slots_of(pair))
        pair = _stations(_density_plan([density_station("lab-pair", cpw=2)], lane_cpu=24,
                                       lane_cores=12))["lab-pair"]
        self.assertEqual((6, 12, 24), (pair["parallel"]["workers"], pair["needs"]["colony"],
                                      pair["needs"]["cpu"]))

    def test_register_slots_stay_in_the_lanes_colonies_without_a_pool(self):
        # build04 (12 cores, D 12): lab 0..11, lab-pair's twelve would run past 11 --
        # it starts at the head again and waits for lab, never slots 12+
        two = [density_station("lab"), density_station("lab-pair", cpw=2)]
        by = _stations(_density_plan(two, lane_cpu=24, lane_cores=12))
        self.assertEqual([str(i) for i in range(12)], _slots_of(by["lab"]))
        self.assertEqual([str(i) for i in range(12)], _slots_of(by["lab-pair"]))
        self.assertIn("lab", by["lab-pair"]["after"])
        # build03 (6 cores, D 6): the same, inside 0..5 (two density stations
        # never share the budget -- a satellite that wants that runs one
        # station with `unit_runner.py --pair`)
        by = _stations(_density_plan(two))
        self.assertEqual([str(i) for i in range(6)], _slots_of(by["lab"]))
        self.assertEqual([str(i) for i in range(6)], _slots_of(by["lab-pair"]))
        self.assertIn("lab", by["lab-pair"]["after"])
        by = _stations(_density_plan([density_station("lab"), density_station("lab-pair", cpw=2)],
                                     environ={register.DENSITY_ENV: "8"}))
        self.assertTrue(all(0 <= int(x) < 8 for s in by.values() for x in _slots_of(s)))

    def test_register_unit_station_never_runs_beside_a_density_station(self):
        unit = station("unit", ["x/**"], needs={"cpu": 8})
        by = _stations(_density_plan([unit, density_station("lab")]))
        # the executor runs two stations side by side only while their cpu fits the lane
        self.assertGreater(by["unit"]["needs"]["cpu"] + by["lab"]["needs"]["cpu"], 12)
        by = _stations(_density_plan([cores_station("unit"), density_station("lab")]))
        self.assertGreater(by["unit"]["needs"]["cpu"] + by["lab"]["needs"]["cpu"], 12)

    def test_register_cores_station_runs_one_worker_per_physical_core(self):
        unit = _stations(_density_plan([cores_station("unit")]))["unit"]
        self.assertEqual((6, 6), (unit["parallel"]["workers"], unit["needs"]["cpu"]))
        self.assertEqual("6", unit["cmds"][0]["argv"][3])
        unit = _stations(_density_plan([cores_station("unit")], lane_cpu=24, lane_cores=None))["unit"]
        self.assertEqual(12, unit["parallel"]["workers"])      # unknown: nproc / 2
        unit = _stations(_density_plan([cores_station("unit")],
                                       environ={register.UNIT_WORKERS_ENV: "4"}))["unit"]
        self.assertEqual((4, 4), (unit["parallel"]["workers"], unit["needs"]["cpu"]))
        for bad in ("x", "0", ""):
            with self.assertRaises(register.RegisterError) as ctx:
                _density_plan([cores_station("unit")], environ={register.UNIT_WORKERS_ENV: bad})
            self.assertIn("MECLAW_UNIT_WORKERS", str(ctx.exception))
        for row in (dict(cores_station("unit"), needs={"cpu": 8}),
                    dict(cores_station("unit"), parallel={"by": "file", "workers": "cores",
                                                          "colonies_per_worker": 2})):
            with self.assertRaises(register.RegisterError):
                _density_plan([row])
        # a slot pool caps only the stations that take slots: a cores station keeps its width
        unit = _stations(_density_plan([cores_station("unit")], lane_cpu=24, lane_cores=12,
                                       slots=[str(i) for i in range(8)]))["unit"]
        self.assertEqual((12, "12"), (unit["parallel"]["workers"], unit["cmds"][0]["argv"][3]))
        # a cores station keeps the colonies it books
        unit = _stations(_density_plan([dict(cores_station("unit"), needs={"colony": 3})]))["unit"]
        self.assertEqual(3, unit["needs"]["colony"])

    def test_register_density_follows_the_env(self):
        by = _stations(_density_plan([density_station("lab")],
                                     environ={register.DENSITY_ENV: "6"}))
        self.assertEqual((6, 6, 12), (by["lab"]["parallel"]["workers"],
                                      by["lab"]["needs"]["colony"], by["lab"]["needs"]["cpu"]))
        with self.assertRaises(register.RegisterError):
            _density_plan([density_station("lab")], environ={register.DENSITY_ENV: "x"})

    def test_register_cli_workers_stay_first(self):
        by = _stations(_density_plan([density_station("lab", cpw=2)], workers=1))
        self.assertEqual((1, 2, 4), (by["lab"]["parallel"]["workers"],
                                     by["lab"]["needs"]["colony"], by["lab"]["needs"]["cpu"]))
        self.assertEqual(["0", "1"], _slots_of(by["lab"]))

    def test_register_plan_names_the_density(self):
        plan = _density_plan([cores_station("unit"), density_station("lab"),
                              density_station("lab-pair", cpw=2)])
        self.assertIn("colony density 6 (lane cores 6, one colony per core), nproc 12; "
                      "unit 6 workers (cores); lab 6 workers x 1 colony, cpu 12; "
                      "lab-pair 3 workers x 2 colonies, cpu 12",
                      plan["notes"])
        plan = _density_plan([density_station("lab")], environ={register.DENSITY_ENV: "6"})
        self.assertIn("colony density 6 (MECLAW_COLONY_DENSITY=6), nproc 12; "
                      "lab 6 workers x 1 colony, cpu 12", plan["notes"])
        plan = _density_plan([slot_station("lab", 2, 2)])
        self.assertFalse([n for n in plan.get("notes") or [] if n.startswith("colony density")])

    def test_register_density_station_does_not_carry_its_needs(self):
        for needs in ({"colony": 4}, {"cpu": 12}):
            with self.assertRaises(register.RegisterError) as ctx:
                _density_plan([dict(density_station("lab"), needs=needs)])
            self.assertIn("density", str(ctx.exception))
        for par in ({"by": "module", "workers": 2, "colonies_per_worker": 2},
                    {"by": "module", "workers": "density", "colonies_per_worker": 0},
                    {"by": "module", "workers": "dense"}):
            with self.assertRaises(register.RegisterError):
                _density_plan([dict(density_station("lab"), parallel=par)])


class DensityThroughTheGate(Satellite):
    """GH #1163: the gate hands the lane's cpu and cores to the satellite plan and says the density."""

    def setUp(self):
        super().setUp()
        write(self.root, "pipeline/stations.json",
              json.dumps(registry([UNIT, density_station("lab")]), indent=2) + "\n")
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", "density")

    def test_gate_satellite_plan_takes_the_lane_cpu_and_cores(self):
        from pipeline import gate
        err = io.StringIO()
        o = gate.parse(["release", "--repo", self.root], err)
        with mock.patch.dict(os.environ, {register.DENSITY_ENV: ""}):
            os.environ.pop(register.DENSITY_ENV)
            doc = gate._satellite_plan(o, self.root, lane_cpu=24, lane_cores=12)
        lab = {s["name"]: s for s in doc["stations"]}["lab"]
        self.assertEqual((12, 12, 24), (lab["parallel"]["workers"], lab["needs"]["colony"],
                                        lab["needs"]["cpu"]))
        self.assertIn("colony density 12 (lane cores 12, one colony per core), "
                      "nproc 24; lab 12 workers x 1 colony, cpu 24",
                      doc["notes"])

    def test_gate_satellite_lane_plan_takes_the_declared_cores(self):
        """welle-tb S5: a lane's real cores come from its host-file line (`cores=`),
        never from `lscpu` on the lane -- build03 is a VM of 1 socket x 12 cores x
        1 thread and counted 12 real cores on a 6-core host (diagnosis d6)."""
        from pipeline import gate, lane
        err = io.StringIO()
        o = gate.parse(["release", "--repo", self.root], err)
        with mock.patch.dict(os.environ, {register.DENSITY_ENV: ""}):
            os.environ.pop(register.DENSITY_ENV)
            doc = gate._satellite_lane_plan(o, self.root, lane.Lane(name="box", ssh="b@x", cores=6),
                                            lane_cpu=12)
            lab = {s["name"]: s for s in doc["stations"]}["lab"]
            self.assertEqual(6, lab["parallel"]["workers"])
            self.assertIn("colony density 6 (lane cores 6, one colony per core), nproc 12; "
                          "lab 6 workers x 1 colony, cpu 12", doc["notes"])
            self.assertFalse([n for n in doc["notes"] if "cores=" in n], doc["notes"])
            # no `cores=`: nproc / 2, and the run log says so
            doc = gate._satellite_lane_plan(o, self.root, lane.Lane(name="box", ssh="b@x"),
                                            lane_cpu=12)
        self.assertEqual(6, {s["name"]: s for s in doc["stations"]}["lab"]["parallel"]["workers"])
        self.assertIn("the host file declares no cores= for lane box: cores = nproc 12 / 2",
                      doc["notes"])
        self.assertFalse(hasattr(lane, "physical_cores"))


# --- the satellites' forms (meclaw#1124) -----------------------------------------

def _runner(*args):
    return ["{python}", "{pipeline}/unit_runner.py", "--root", "{root}"] + list(args)


SAT_STATIONS = [
    station("unit", ["tests/**"], cmds=[{"argv": ["env", "ORGA_KIT_LAB=0"] + _runner(
        "--by", "file", "--workers", "{workers}", "--include", "tests/*/test_*.py",
        "--exclude", "tests/lab/**", "--exclude", "tests/*/fixtures/**", "--exclude", "tests/lab_*/**",
        "--exclude", "tests/portal/**", "--python", "{python}"), "cwd": "."}], parallel={"by": "file", "workers": 8}),
    station("script", ["tests/**"], cmds=[{"argv": _runner(
        "--by", "file", "--run", "script", "--workers", "{workers}", "--include", "tests/test_*.py",
        "--include", "tests/portal/test_*.py", "--python", "{root}/apps/portal/.venv/bin/python"),
        "cwd": "."}], parallel={"by": "file", "workers": 2}),
    station("lab", ["tests/**"], cmds=[
        {"argv": ["{python}", "-c", "print('boot')", "--slots", "{slots}"], "cwd": "."},
        {"argv": ["env", "SOUL2ME_LAB_SLOTS={slots}"] + _runner(
            "--by", "dir", "--top", "unit", "--workers", "{workers}", "--include", "tests/lab_*",
            "--exclude", "tests/lab_ops", "--slots", "{slots}", "--slot-env", "SOUL2ME_LAB_SLOT"),
         "cwd": "."}],
        parallel={"by": "dir", "workers": 4, "slot_env": "SOUL2ME_LAB_SLOT"}),
    station("lab-module", ["tests/**"], cmds=[{"argv": _runner(
        "--by", "module", "--workers", "{workers}", "--include", "tests/lab/test_*.py",
        "--slots", "{slots}", "--slot-env", "ORGA_LAB_WORKER_SLOT"), "cwd": "."}],
        parallel={"by": "module", "workers": 4, "slot_env": "ORGA_LAB_WORKER_SLOT"}),
]


def _listed(cmd, root):
    """`unit_runner --list` with the plan's argv (the `env K=V` prefix kept)."""
    proc = subprocess.run(cmd["argv"] + ["--list"], cwd=os.path.join(root, cmd["cwd"]),
                          capture_output=True, text=True, timeout=2 * kit_time.HANG)
    return proc.returncode, proc.stdout.strip().splitlines()


class SatelliteForms(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = os.path.join(self._tmp.name, "orga")
        write(self.root, "pipeline/stations.json",
              json.dumps(registry(SAT_STATIONS, repo="orga"), indent=2) + "\n")
        for rel in ("tests/a/test_1.py", "tests/a/test_2.py", "tests/a/fixtures/test_f.py",
                    "tests/test_top.py", "tests/portal/test_p.py", "tests/lab/test_l1.py",
                    "tests/lab/test_l2.py", "tests/lab_x/test_x.py", "tests/lab_x/_setup.py",
                    "tests/lab_y/test_y.py", "tests/lab_ops/test_o.py"):
            write(self.root, rel, "raise SystemExit('must not run')\n")

    def tearDown(self):
        self._tmp.cleanup()

    def test_register_satellite_argv_lists_the_old_selection(self):
        plan = register.build_plan(self.root, "release", None, files=[],
                                   slots=["7930", "7931", "7932", "7933"])
        by = {s["name"]: s for s in plan["stations"]}
        self.assertEqual(["unit", "script", "lab", "lab-module"], [s["name"] for s in plan["stations"]])
        for st in plan["stations"]:
            self.assertIn("tool:python", st["inputs"], st["name"])
            for cmd in st["cmds"]:
                self.assertNotIn("{", " ".join(cmd["argv"]) + cmd["cwd"], st["name"])
        # cmds keep their order; every placeholder is resolved, inside a K=V too
        lab = by["lab"]["cmds"]
        self.assertEqual("-c", lab[0]["argv"][1])
        self.assertEqual(["--slots", "7930,7931,7932,7933"], lab[0]["argv"][-2:])
        self.assertEqual(["env", "SOUL2ME_LAB_SLOTS=7930,7931,7932,7933"], lab[1]["argv"][:2])
        script = by["script"]["cmds"][0]["argv"]
        self.assertEqual(os.path.join(os.path.abspath(self.root), "apps/portal/.venv/bin/python"),
                         script[script.index("--python") + 1])
        self.assertEqual((0, ["UNIT tests/a/test_1.py", "UNIT tests/a/test_2.py", "UNIT-LIST 2"]),
                         _listed(by["unit"]["cmds"][0], self.root))
        self.assertEqual((0, ["UNIT tests/portal/test_p.py", "UNIT tests/test_top.py", "UNIT-LIST 2"]),
                         _listed(by["script"]["cmds"][0], self.root))
        self.assertEqual((0, ["UNIT tests/lab_x", "UNIT tests/lab_y", "UNIT-LIST 2"]),
                         _listed(lab[1], self.root))
        self.assertEqual((0, ["UNIT tests/lab/test_l1.py", "UNIT tests/lab/test_l2.py", "UNIT-LIST 2"]),
                         _listed(by["lab-module"]["cmds"][0], self.root))

    def test_register_unknown_placeholder_is_an_error(self):
        bad = station("bad", ["x/**"], cmds=[{"argv": ["{python}", "{repo_root}/x.py"], "cwd": "."}])
        write(self.root, "pipeline/stations.json", json.dumps(registry([bad], repo="orga")) + "\n")
        with self.assertRaises(register.RegisterError) as cm:
            register.load_register(self.root)
        self.assertIn("{repo_root}", str(cm.exception))


# --- transport ------------------------------------------------------------------

SECRETS = (".env", ".env.local", "vault.env", "vault-pass.txt", "provider.env", ".netrc",
           ".git-credentials", "sub/.env", "sub/provider.env")
HARMLESS = (".env.example", "vault/notes.md", ".env.d/readme.txt", "environment.md")


def _walk(root):
    out = []
    for top, dirs, files in os.walk(root):
        for f in files:
            out.append(os.path.relpath(os.path.join(top, f), root))
    return sorted(out)


def _run(argv):
    res = subprocess.run(argv, capture_output=True, text=True)
    if res.returncode != 0:
        raise AssertionError("%s: %s" % (argv, res.stderr))


@unittest.skipUnless(shutil.which("rsync"), "rsync not installed")
class Transport(unittest.TestCase):

    def test_lane_display_lab_lands_where_the_satellites_look(self):
        # W10 part B, G3: soul2me's lab-ops folds screens through display-lab's
        # patches.mjs at ~/projeks/MeClaw/meclaw-core/...; on build03 it was not there
        (argv,) = register.display_lab_spec("/x/core", target="lane@build03")
        # the checked local rsync of a remote way (OR-T10-25), absolute
        from pipeline import lane
        self.assertEqual([lane.rsync_program(), "-aR"], argv[:2])
        self.assertIn("/x/core/./workshop/tools/display-lab/patches.mjs", argv)
        self.assertEqual("lane@build03:projeks/MeClaw/meclaw-core/", argv[-1])

    def test_lane_display_lab_creates_a_missing_home_path(self):
        # W10 part B, I1: a fresh lane has no ~/projeks/MeClaw yet; plain rsync
        # then exits 11 and the push turns the whole lane red
        (argv,) = register.display_lab_spec("/x/core", target="lane@build09")
        self.assertIn("--mkpath", argv[:argv.index("/x/core/./workshop/tools/display-lab/patches.mjs")])
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = self._tmp.name

    def tearDown(self):
        self._tmp.cleanup()

    def test_lane_push_mirrors_and_assets_never_secrets(self):
        for name in SECRETS:
            self.assertTrue(register.secret_name(name), name)
        for name in HARMLESS:
            self.assertFalse(register.secret_name(name), name)
        repos = []
        for name in ("meclaw-core", "voice2vision", "orga-hive"):
            repo = os.path.join(self.tmp, "src", name)
            os.makedirs(repo)
            git(repo, "init", "-q", "-b", "main")
            git(repo, "config", "user.email", "t@example.invalid")
            git(repo, "config", "user.name", "t")
            write(repo, "README.md", name + "\n")
            git(repo, "add", "-A")
            git(repo, "commit", "-q", "-m", "base")
            git(repo, "tag", name + "-1.0.0")
            git(repo, "branch", "side")
            for secret in SECRETS:
                write(repo, ".git/" + secret, "TOKEN=x\n")
            write(repo, ".env", "TOKEN=x\n")
            repos.append(repo)
        dest = os.path.join(self.tmp, "lane", "mirrors")
        specs = register.mirrors_spec(repos, dest_root=dest)
        self.assertEqual(3, len(specs))
        for argv in specs:
            self.assertEqual("rsync", argv[0])
            self.assertTrue(argv[-2].endswith("/.git/"), argv)
            os.makedirs(os.path.dirname(argv[-1].rstrip("/")), exist_ok=True)
            _run(argv)
        got = _walk(dest)
        self.assertEqual([], [p for p in got if register.secret_name(p)])
        for name in ("meclaw-core", "voice2vision", "orga-hive"):
            mirror = os.path.join(dest, name)
            self.assertTrue(os.path.isdir(os.path.join(mirror, ".git")), mirror)
            # all refs travel: the tag and the side branch resolve in the mirror
            self.assertTrue(git(mirror, "rev-parse", "--verify", name + "-1.0.0^{commit}"))
            self.assertTrue(git(mirror, "rev-parse", "--verify", "side"))
        # the release-asset cache: every version, never a secret beside it
        cache = os.path.join(self.tmp, "cache", "soul2me", "meclaw")
        write(cache, "0.63.0/meclaw", "binary\n")
        write(cache, "0.63.0/templates.tgz", "tgz\n")
        write(cache, "0.63.0/.env", "TOKEN=x\n")
        write(cache, "0.62.1/meclaw", "old\n")
        write(cache, "provider.env", "TOKEN=x\n")
        adest = os.path.join(self.tmp, "lane", "cache")
        specs = register.assets_spec(cache, dest_root=adest)
        self.assertEqual(1, len(specs))
        os.makedirs(adest, exist_ok=True)
        for argv in specs:
            _run(argv)
        got = _walk(adest)
        self.assertEqual(["soul2me/meclaw/0.62.1/meclaw", "soul2me/meclaw/0.63.0/meclaw",
                          "soul2me/meclaw/0.63.0/templates.tgz"], got)
        one = register.assets_spec(cache, dest_root=adest, versions=["0.63.0"])
        self.assertEqual(1, len(one))
        self.assertTrue(one[0][-2].endswith("/0.63.0/"), one[0])
        # a remote target only prefixes the destination
        remote = register.mirrors_spec(repos[:1], target="lane-a")
        self.assertTrue(remote[0][-1].startswith("lane-a:/srv/target/mirrors/"))
        for argv in register.mirrors_spec(repos) + register.assets_spec(cache):
            for pattern in ("--exclude=.env", "--exclude=.env.*", "--exclude=vault.env",
                            "--exclude=vault-pass*", "--exclude=provider.env",
                            "--exclude=.netrc", "--exclude=.git-credentials"):
                self.assertIn(pattern, argv)

    def test_lane_push_kit_once_per_core_rev(self):
        core = os.path.join(self.tmp, "core")
        write(core, "scripts/pipeline.py", "# starter\n")
        write(core, "scripts/pipeline/__init__.py", "")
        write(core, "scripts/pipeline/register.py", "# register\n")
        write(core, "scripts/pipeline/receipt-v2.schema.json", "{}\n")
        # Joined, not one literal: the real tree carries this .pyc as soon as
        # the module is imported, and precheck's root-files check reads every
        # whole "scripts/..." literal of a test as a path the test reads.
        write(core, os.path.join("scripts", "pipeline", "__pycache__",
                                 "register.cpython-312.pyc"), "x")
        write(core, "scripts/pipeline/.env", "TOKEN=x\n")
        write(core, "scripts/gate.sh", "# not the kit\n")
        write(core, "crates/x/src/lib.rs", "")
        rev = "0123456789abcdef0123456789abcdef01234567"
        root = os.path.join(self.tmp, "lane", "kit")
        kit = register.kit_push_spec(core, rev, dest_root=root)
        self.assertEqual("01234567", kit.sha8)
        self.assertEqual(rev, kit.rev)
        self.assertEqual(os.path.join(root, "01234567") + "/", kit.dest)
        self.assertEqual(["scripts/pipeline.py", "scripts/pipeline/__init__.py",
                          "scripts/pipeline/receipt-v2.schema.json",
                          "scripts/pipeline/register.py"], kit.files)
        self.assertFalse(kit.already_there(""))
        listing = "\n".join(os.listdir(root)) if os.path.isdir(root) else ""
        self.assertFalse(kit.already_there(listing))
        for argv in kit.steps():
            _run(argv)
        self.assertEqual(kit.files, _walk(kit.dest))
        listing = "\n".join(sorted(os.listdir(root)))
        self.assertTrue(kit.already_there(listing))
        # a half-copied kit (no marker) is not there
        self.assertFalse(kit.already_there("01234567\n"))
        # once per core rev: another rev is a new push, the same rev is none
        other = register.kit_push_spec(core, "fedcba98" + rev[8:], dest_root=root)
        self.assertFalse(other.already_there(listing))
        self.assertTrue(register.kit_push_spec(core, rev, dest_root=root).already_there(listing))
        remote = register.kit_push_spec(core, rev, target="lane-a")
        self.assertEqual("lane-a:/srv/target/kit/01234567/", remote.rsync_argv()[-1])
        self.assertEqual(["ls", "-1", "/srv/target/kit"], remote.probe_argv())


_PATH_LOADERS = ("spec_from_file_location", "run_path", "SourceFileLoader")


def _loader_literals(tree):
    """`.py` file names spelled as literals in a function that loads a module
    by path (`importlib.util.spec_from_file_location` and kin)."""
    import ast
    found = []
    for fn in ast.walk(tree):
        if not isinstance(fn, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        calls = {getattr(n.func, "attr", getattr(n.func, "id", None))
                 for n in ast.walk(fn) if isinstance(n, ast.Call)}
        if calls & set(_PATH_LOADERS):
            found += [n.value for n in ast.walk(fn) if isinstance(n, ast.Constant)
                      and isinstance(n.value, str) and n.value.endswith(".py")
                      and " " not in n.value]
    return found


def _repo_imports(root, rel):
    """Independent oracle: the repository modules under `scripts/` that the
    file `rel` imports -- absolute imports (`scripts/` is on sys.path),
    level-1 relative imports and modules loaded by path from a literal file
    name next to the file or one directory up (W10 M-a)."""
    import ast
    with open(os.path.join(root, rel), encoding="utf-8") as fh:
        tree = ast.parse(fh.read(), filename=rel)
    here = os.path.dirname(rel)
    names = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            names += [a.name for a in node.names]
        elif isinstance(node, ast.ImportFrom) and node.level in (0, 1):
            base = node.module or ""
            if node.level == 1:
                base = ".".join(filter(None, [here.replace("/", "."), base]))
            if base:
                names.append(base)
                names += ["%s.%s" % (base, a.name) for a in node.names]
    found = set()
    for name in names:
        parts = name.split(".")
        if parts[0] != "scripts":
            parts = ["scripts"] + parts
        base = os.path.join(*parts)
        for cand in (base + ".py", os.path.join(base, "__init__.py")):
            if os.path.isfile(os.path.join(root, cand)):
                found.add(cand.replace(os.sep, "/"))
    for lit in _loader_literals(tree):
        for cand in (os.path.join(here, lit), os.path.join(os.path.dirname(here), lit)):
            cand = os.path.normpath(cand).replace(os.sep, "/")
            if cand.startswith("scripts/") and os.path.isfile(os.path.join(root, cand)):
                found.add(cand)
                break
    return found


class KitImports(unittest.TestCase):
    """The kit on the lane carries every repository module the pipeline imports
    (W10 part B: `pipeline/pre.py` imports `precheck`, the kit lacked it and
    the satellite gate on the lane stopped after 10 s with rc 2). No ssh; the
    push case runs rsync locally into a temp directory."""

    def test_the_oracle_sees_relative_imports_and_path_loaders(self):
        """W10 M-a (#1135): level-1 relative imports and a module loaded by
        `spec_from_file_location` from a literal file name count as imports."""
        with tempfile.TemporaryDirectory() as tmp:
            pkg = os.path.join(tmp, "scripts", "pkg")
            os.makedirs(pkg)
            for rel in ("pkg/__init__.py", "pkg/a.py", "pkg/b.py", "pkg/c.py", "side.py",
                        "unused.py"):
                open(os.path.join(tmp, "scripts", rel), "w").close()
            with open(os.path.join(pkg, "main.py"), "w") as fh:
                fh.write("from . import a\nfrom .b import thing\nimport importlib.util\n"
                         "def load():\n    p = os.path.join(os.path.dirname(HERE), 'side.py')\n"
                         "    return importlib.util.spec_from_file_location('s', p)\n"
                         "NOT_LOADED = 'unused.py'\n")
            self.assertEqual({"scripts/pkg/__init__.py", "scripts/pkg/a.py",
                              "scripts/pkg/b.py", "scripts/side.py"},
                             _repo_imports(tmp, "scripts/pkg/main.py"))

    def test_kit_carries_every_transitive_repo_import_of_the_pipeline(self):
        root = str(SCRIPTS.parent)
        kit = register.kit_push_spec(root, "0" * 40, dest_root=os.path.join("kit-dest"))
        start = [f for f in kit.files if f.endswith(".py")
                 and (f == "scripts/pipeline.py" or f.startswith("scripts/pipeline/"))]
        self.assertIn(os.path.join("scripts", "pipeline", "pre.py").replace(os.sep, "/"), start)
        seen, todo = set(), list(start)
        while todo:
            rel = todo.pop()
            if rel in seen:
                continue
            seen.add(rel)
            todo += sorted(_repo_imports(root, rel) - seen)
        missing = sorted(seen - set(kit.files))
        self.assertEqual([], missing, "imported by the pipeline, missing in the kit")
        self.assertIn(os.path.join("scripts", "precheck.py").replace(os.sep, "/"), kit.files)

    def test_the_kit_alone_starts_the_executor(self):
        # cli.py builds its parser from every module, so `exec --help` loads
        # the whole import closure: the kit, copied alone, must carry it.
        root = str(SCRIPTS.parent)
        kit = register.kit_push_spec(root, "0" * 40, dest_root=os.path.join("kit-dest"))
        with tempfile.TemporaryDirectory() as tmp:
            for rel in kit.files:
                dest = os.path.join(tmp, rel)
                os.makedirs(os.path.dirname(dest), exist_ok=True)
                shutil.copyfile(os.path.join(root, rel), dest)
            res = subprocess.run(
                [sys.executable, "-I", os.path.join(tmp, "scripts", "pipeline.py"), "exec", "--help"],
                cwd=tmp, capture_output=True, text=True, timeout=2 * kit_time.HANG)
        self.assertEqual(0, res.returncode, res.stderr[-2000:])

    def test_the_kit_pushed_through_push_kit_starts_the_executor(self):
        # Re-check W10 I1: kit_files listed precheck/gate_plan/display_sheet_strip,
        # but push_kit copied through a fixed rsync filter (`scripts/pipeline/***`
        # only) and all three were missing on the lane. This case runs the real
        # transport: lane.push_kit on a local lane, rsync into a temp directory.
        from pipeline import lane as lane_mod
        if shutil.which("rsync") is None:
            self.skipTest("rsync not installed")
        with tempfile.TemporaryDirectory() as tmp:
            core = os.path.join(tmp, "core")
            shutil.copytree(str(SCRIPTS), os.path.join(core, "scripts"),
                            ignore=shutil.ignore_patterns("__pycache__", "*.pyc"))
            git(core, "init", "-q", "-b", "master")
            git(core, "config", "user.email", "t@example.invalid")
            git(core, "config", "user.name", "t")
            git(core, "add", "-A")
            git(core, "commit", "-q", "-m", "base")
            ln = lane_mod.Lane(name="lane-a", ssh="", root=os.path.join(tmp, "lane"),
                               transport="local")
            kit = lane_mod.push_kit(ln, core)
            for rel in ("scripts/precheck.py", "scripts/gate_plan.py"):
                self.assertTrue(os.path.isfile(os.path.join(kit, rel)), rel + " not pushed")
            self.assertFalse(os.path.exists(os.path.join(kit, "scripts", "gate.sh")),
                             "the kit carries only its own files")
            res = subprocess.run(
                [sys.executable, "-I", os.path.join(kit, "scripts", "pipeline.py"), "exec", "--help"],
                cwd=tmp, capture_output=True, text=True, timeout=2 * kit_time.HANG)
        self.assertEqual(0, res.returncode, res.stderr[-2000:])


class W3Hooks(Satellite):
    """W10's hooks into W3's executor side (merge slot): lane verbs, the
    shared secret rule, gate --repo and one critical path."""

    def lane(self, root):
        from pipeline import lane
        return lane, lane.Lane(name="lane-a", ssh="", root=root, transport="local")

    def core(self):
        core = os.path.join(self._tmp.name, "core")
        write(core, "scripts/pipeline.py", "# starter\n")
        write(core, "scripts/pipeline/__init__.py", "")
        write(core, "scripts/pipeline/register.py", "# register\n")
        write(core, "scripts/gate.sh", "# not the kit\n")
        git(core, "init", "-q", "-b", "master")
        git(core, "config", "user.email", "t@example.invalid")
        git(core, "config", "user.name", "t")
        git(core, "add", "-A")
        git(core, "commit", "-q", "-m", "base")
        git(core, "tag", "core-1.0.0")
        return core

    def test_register_critical_path_is_plan_py(self):
        from pipeline import plan
        self.assertIs(register.critical_path, plan.critical_path)

    def test_lane_secret_name_is_the_register_rule(self):
        lane, _ = self.lane(self._tmp.name)
        for name in list(SECRETS) + ["apps/portal/provider.env"]:
            self.assertTrue(lane.secret_name(name), name)
        for name in HARMLESS:
            self.assertFalse(lane.secret_name(name), name)

    def test_lane_push_kit_refuses_a_deleted_kit_file(self):
        # W10 re-check M-b: the dirty check saw edits but not deletions.
        core = self.core()
        lane, ln = self.lane(os.path.join(self._tmp.name, "lane"))
        os.remove(os.path.join(core, "scripts", "pipeline", "__init__.py"))
        with self.assertRaises(lane.LaneError) as cm:
            lane.push_kit(ln, core)
        self.assertEqual(5, cm.exception.code)
        self.assertIn("__init__.py", str(cm.exception))

    def test_lane_push_kit_refuses_a_deleted_imported_module_outside_the_kit_roots(self):
        # Review Zb R2 M3: `pipeline/register.py` imports `precheck`; deleting
        # the tracked scripts/precheck.py left it out of the kit silently.
        core = self.core()
        write(core, "scripts/precheck.py", "# imported\n")
        write(core, "scripts/pipeline/register.py", "import precheck\n")
        write(core, "scripts/unrelated.py", "# not imported\n")
        git(core, "add", "-A")
        git(core, "commit", "-q", "-m", "imports")
        lane, ln = self.lane(os.path.join(self._tmp.name, "lane"))
        os.remove(os.path.join(core, "scripts", "unrelated.py"))
        lane.push_kit(ln, core)  # a deleted module nobody imports is no kit file
        os.remove(os.path.join(core, "scripts", "precheck.py"))
        with self.assertRaises(lane.LaneError) as cm:
            lane.push_kit(ln, core)
        self.assertEqual(5, cm.exception.code)
        self.assertIn("precheck.py", str(cm.exception))

    def test_lane_push_kit_mirrors_assets_through_the_lane_verbs(self):
        core = self.core()
        root = os.path.join(self._tmp.name, "lane")
        lane, ln = self.lane(root)
        rev = git(core, "rev-parse", "HEAD")
        kit = lane.push_kit(ln, core)
        self.assertEqual(os.path.join(root, "kit", rev[:8]), kit)
        self.assertEqual(["scripts/pipeline.py", "scripts/pipeline/__init__.py",
                          "scripts/pipeline/register.py"], _walk(kit))
        self.assertTrue(os.path.isfile(os.path.join(root, "kit", rev[:8] + ".ok")))
        # once per core revision: the second push copies nothing
        with mock.patch.object(lane, "_rsync", side_effect=AssertionError("copied twice")):
            self.assertEqual(kit, lane.push_kit(ln, core))
        # review pd-1147 M-1 (a): the reuse refreshes the marker in the same
        # remote command as the listing, so a lane hygiene sweep (KF5, 24 h
        # keep) never ages out a kit that push_kit has just handed out.
        marker = os.path.join(root, "kit", rev[:8] + ".ok")
        os.utime(marker, (1_000_000_000, 1_000_000_000))
        with mock.patch.object(lane, "_rsync", side_effect=AssertionError("copied twice")):
            self.assertEqual(kit, lane.push_kit(ln, core))
        self.assertGreater(os.stat(marker).st_mtime, time.time() - 3600)
        # a kit built from uncommitted files would lie about its revision
        write(core, "scripts/pipeline/register.py", "# changed\n")
        with self.assertRaises(lane.LaneError) as cm:
            lane.push_kit(ln, core)
        self.assertEqual(5, cm.exception.code)
        git(core, "checkout", "-q", "--", ".")
        mirrors = lane.push_mirrors(ln, [core])
        self.assertEqual([os.path.join(root, "mirrors", "core")], mirrors)
        self.assertEqual(rev, git(mirrors[0], "rev-parse", "core-1.0.0^{commit}"))
        cache = os.path.join(self._tmp.name, "cache", "soul2me", "meclaw")
        write(cache, "0.63.0/meclaw", "binary\n")
        write(cache, "0.63.0/.env", "TOKEN=x\n")
        # a local lane shares this home: nothing to copy
        self.assertIsNone(lane.push_assets(ln, cache))
        dest = os.path.join(root, "cache")
        self.assertEqual(dest, lane.push_assets(ln, cache, dest_root=dest))
        self.assertEqual(["soul2me/meclaw/0.63.0/meclaw"], _walk(dest))

    def test_gate_repo_plans_a_satellite_from_its_register(self):
        from pipeline import gate
        err = io.StringIO()
        o = gate.parse(["strand", "--repo", self.root, "--slots", "7930,7931"], err)
        self.assertEqual((self.root, "7930,7931"), (o.repo, o.slots))
        self.commit("scripts/a.py", "A = 2\n")
        out = io.StringIO()
        with _METRICS_URL, contextlib.chdir(self._tmp.name):
            rc = gate.run_gate(["strand", "--plan-only", "--repo", self.root, "--base", self.main],
                               out, err)
        self.assertEqual(0, rc, err.getvalue())
        self.assertIn("unit", out.getvalue())
        self.assertNotIn("docs", [l.split()[1] for l in out.getvalue().splitlines()
                                   if l.startswith("GATE ") and len(l.split()) > 1])

    def test_register_plan_runs_from_another_root(self):
        plan = register.build_plan(self.root, "release", None, pipeline_dir="/kit/scripts/pipeline",
                                   python="python3", run_root="/srv/target/wt/coding")
        argv = [w for st in plan["stations"] for c in st["cmds"] for w in c["argv"]]
        self.assertFalse([w for w in argv if self.root in w], argv)


def _repo(path, tag=None):
    os.makedirs(path, exist_ok=True)
    git(path, "init", "-q", "-b", "main")
    git(path, "config", "user.email", "t@example.invalid")
    git(path, "config", "user.name", "t")
    write(path, "README.md", os.path.basename(path) + "\n")
    git(path, "add", "-A")
    git(path, "commit", "-q", "-m", "base")
    if tag:
        git(path, "tag", tag)
    return git(path, "rev-parse", "HEAD")


class PinFixture(unittest.TestCase):
    """A home with a core, two source repositories, a release-asset cache and
    a soul2me-like satellite whose sets pin all of them (GH #1143)."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.home = os.path.join(self._tmp.name, "home")
        self.core = os.path.join(self.home, "projeks", "MeClaw", "meclaw-core")
        self.core_sha = _repo(self.core)
        self.orga_sha = _repo(os.path.join(self.home, "projeks", "orga-hive"), tag="orga-1.0.0")
        self.v2v_sha = _repo(os.path.join(self.home, "projeks", "voice2vision"))
        cache = os.path.join(self.home, ".cache", "soul2me", "meclaw")
        write(cache, "0.62.1/meclaw", "old\n")
        write(cache, "0.63.0/meclaw", "binary\n")
        write(cache, "0.55.0/meclaw", "never pinned\n")
        self.sat = os.path.join(self.home, "projeks", "soul2me")
        write(self.sat, "pipeline/stations.json",
              json.dumps(registry([station("set", ["sets/**"])], repo="soul2me")))
        self.set("0.20", {"meclaw_release": "0.62.1",
                          "meclaw_core": "%s (meclaw-core, pins)" % self.core_sha[:9],
                          "sources": {"orga": {"repo": "~/projeks/orga-hive", "tag": "orga-1.0.0",
                                               "templates": ["orga"]}}})
        self.set("0.21", {"meclaw_release": "0.63.0", "meclaw_core": self.core_sha[:9],
                          "sources": {"orga": {"repo": "~/projeks/orga-hive",
                                               "commit": self.orga_sha[:7]},
                                      "voice2vision": {"repo": "~/projeks/voice2vision",
                                                       "commit": self.v2v_sha[:7], "dir": "apps"}}})
        # an old set built from the core alone names no release asset
        self.set("0.4", {"meclaw_core": self.core_sha[:9]})

    def tearDown(self):
        self._tmp.cleanup()

    def set(self, version, doc, root=None):
        doc = dict({"set": "s-" + version, "version": version}, **doc)
        write(root or self.sat, "sets/%s.json" % version, json.dumps(doc, indent=2) + "\n")

    def pins(self, root=None):
        return register.set_pins(root or self.sat, self.core, home=self.home)


class SetPins(PinFixture):
    """GH #1143: the kit step derives the release-asset cache and the source
    mirrors (meclaw-core included) a satellite's lane needs from its set pins
    (`sets/<v>.json`) -- on build03 both were pushed by hand (W10 part B,
    F4/S1). A pin that cannot travel is loud: no lane run on a guess."""

    def test_set_pins_derive_the_asset_cache_and_the_mirrors(self):
        got = self.pins()
        self.assertEqual("soul2me", got.repo)
        self.assertEqual(os.path.join(self.home, ".cache", "soul2me", "meclaw"), got.cache)
        # exactly the pinned releases, not the whole cache
        self.assertEqual(["0.62.1", "0.63.0"], got.versions)
        mirrors = {m["rel"]: m for m in got.mirrors}
        self.assertEqual(["projeks/MeClaw/meclaw-core", "projeks/orga-hive", "projeks/voice2vision"],
                         sorted(mirrors))
        self.assertEqual([self.core_sha], mirrors["projeks/MeClaw/meclaw-core"]["revs"])
        self.assertEqual([self.orga_sha], mirrors["projeks/orga-hive"]["revs"])
        self.assertEqual([self.v2v_sha], mirrors["projeks/voice2vision"]["revs"])
        self.assertEqual(os.path.join(self.core, ".git"),
                         mirrors["projeks/MeClaw/meclaw-core"]["git_dir"])
        # the core pin of a worktree is the main checkout's repository
        wt = os.path.join(self.home, "projeks", "MeClaw", "meclaw-core-wt-x")
        git(self.core, "worktree", "add", "-q", wt)
        got = register.set_pins(self.sat, wt, home=self.home)
        self.assertIn("projeks/MeClaw/meclaw-core", [m["rel"] for m in got.mirrors])

    def test_set_pins_read_the_satellites_version_key_and_cache_name(self):
        # coding and orga-hive pin `meclaw`, soul2me `meclaw_release`; orga's
        # cache is ~/.cache/orga-hive (its build_set.py), not ~/.cache/orga
        for repo, cache in (("coding", "coding"), ("orga", "orga-hive")):
            root = os.path.join(self.home, "projeks", "sat-" + repo)
            write(root, "pipeline/stations.json", json.dumps(registry([], repo=repo)))
            write(self.home, ".cache/%s/meclaw/0.62.1/meclaw" % cache, "b\n")
            self.set("0.2", {"meclaw": "0.62.1"}, root=root)
            got = self.pins(root)
            self.assertEqual(os.path.join(self.home, ".cache", cache, "meclaw"), got.cache)
            self.assertEqual((["0.62.1"], []), (got.versions, got.mirrors))

    def test_set_pins_without_sets_carry_nothing(self):
        shutil.rmtree(os.path.join(self.sat, "sets"))
        got = self.pins()
        self.assertEqual(([], []), (got.versions, got.mirrors))

    def test_set_pins_fail_closed_on_every_pin_that_cannot_travel(self):
        cases = {
            "0.30": ("{not json", "not JSON"),
            "0.31": ({"meclaw_release": "0.63"}, "not x.y.z"),
            "0.32": ({"meclaw_release": "0.99.0"}, "0.99.0"),
            "0.33": ({"meclaw_core": "fab (no commit)"}, "names no commit"),
            "0.34": ({"meclaw_core": "deadbeef1"}, "deadbeef1"),
            "0.35": ({"sources": {"orga": {"repo": "~/projeks/gone", "tag": "x"}}}, "projeks/gone"),
            "0.36": ({"sources": {"orga": {"repo": "~/projeks/orga-hive", "tag": "orga-9"}}},
                     "orga-9"),
            "0.37": ({"sources": {"orga": {"repo": "~/projeks/orga-hive"}}}, "neither tag nor commit"),
            "0.38": ({"sources": {"orga": "~/projeks/orga-hive"}}, "not an object"),
            "0.39": ({"sources": ["orga"]}, "sources"),
            "0.40": ({"meclaw": "0.62.1", "meclaw_release": "0.63.0"}, "disagree"),
        }
        outside = os.path.join(self._tmp.name, "elsewhere")
        _repo(outside)
        cases["0.41"] = ({"sources": {"x": {"repo": outside, "commit": "HEAD"}}}, "not below")
        write(self.home, ".cache/soul2me/meclaw/0.61.0/download/.keep-dir", "")
        os.remove(os.path.join(self.home, ".cache/soul2me/meclaw/0.61.0/download/.keep-dir"))
        cases["0.42"] = ({"meclaw_release": "0.61.0"}, "0.61.0")  # an empty download/ is no asset
        for version, (doc, needle) in cases.items():
            with self.subTest(version=version):
                tmp = tempfile.mkdtemp(dir=self._tmp.name)
                sat = os.path.join(tmp, "sat")
                write(sat, "pipeline/stations.json",
                      json.dumps(registry([], repo="soul2me")))
                if isinstance(doc, str):
                    write(sat, "sets/%s.json" % version, doc)
                else:
                    self.set(version, doc, root=sat)
                with self.assertRaises(register.RegisterError) as cm:
                    register.set_pins(sat, self.core, home=self.home)
                self.assertIn("sets/%s.json" % version, str(cm.exception))
                self.assertIn(needle, str(cm.exception))
        # every broken pin of the tree is named at once, not the first only
        self.set("0.30", {"meclaw_release": "0.99.0"})
        self.set("0.31", {"meclaw_core": "deadbeef1"})
        with self.assertRaises(register.RegisterError) as cm:
            self.pins()
        self.assertIn("sets/0.30.json", str(cm.exception))
        self.assertIn("sets/0.31.json", str(cm.exception))

    def test_register_pins_cli_prints_the_transport_or_refuses(self):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            rc = cli.main(["register", "pins", "--repo", self.sat, "--core", self.core,
                           "--home", self.home])
        self.assertEqual(0, rc, err.getvalue())
        doc = json.loads(out.getvalue())
        self.assertEqual(["0.62.1", "0.63.0"], doc["versions"])
        self.assertEqual(3, len(doc["mirrors"]))
        self.set("0.22", {"meclaw_release": "0.99.0"})
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            rc = cli.main(["register", "pins", "--repo", self.sat, "--core", self.core,
                           "--home", self.home])
        self.assertEqual(2, rc)
        self.assertIn("0.99.0", err.getvalue())

    def test_set_pins_name_an_unhashable_release_as_a_finding(self):
        # a list or object as release value is one finding among all others
        # (exit 2), not a TypeError that hides the rest
        self.set("0.22", {"meclaw_release": ["0.63.0"]})
        self.set("0.23", {"meclaw": {"v": "0.63.0"}, "meclaw_release": "0.63.0"})
        self.set("0.24", {"meclaw_release": "0.99.0"})
        with self.assertRaises(register.RegisterError) as cm:
            self.pins()
        msg = str(cm.exception)
        self.assertIn("sets/0.22.json: meclaw release ['0.63.0'] is not x.y.z", msg)
        self.assertIn("sets/0.23.json: meclaw and meclaw_release disagree", msg)
        self.assertIn("sets/0.24.json", msg)
        err = io.StringIO()
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(err):
            rc = cli.main(["register", "pins", "--repo", self.sat, "--core", self.core,
                           "--home", self.home])
        self.assertEqual(2, rc)
        self.assertIn("sets/0.22.json", err.getvalue())

    def test_set_pins_check_every_revision_a_source_names(self):
        # tag plus commit (and coding's `source_commits.<n>`): every named
        # revision must be here and all must agree -- the tag alone is not
        # the pin
        orga = os.path.join(self.home, "projeks", "orga-hive")
        write(orga, "next.md", "x\n")
        git(orga, "add", "-A")
        git(orga, "commit", "-q", "-m", "next")
        src = {"repo": "~/projeks/orga-hive", "tag": "orga-1.0.0"}
        self.set("0.22", {"sources": {"orga": dict(src, commit=self.orga_sha[:9])},
                          "source_commits": {"orga": self.orga_sha}})
        got = {m["rel"]: m for m in self.pins().mirrors}
        self.assertEqual([self.orga_sha], got["projeks/orga-hive"]["revs"])
        cases = {
            "0.22": ({"sources": {"orga": dict(src, commit="deadbeef1")}}, "deadbeef1"),
            "0.23": ({"sources": {"orga": src}, "source_commits": {"orga": "deadbeef2"}},
                     "deadbeef2"),
            "0.24": ({"sources": {"orga": dict(src, commit="HEAD")}}, "disagree"),
            "0.25": ({"sources": {"orga": src}, "source_commits": {"orga": "HEAD"}}, "disagree"),
            "0.26": ({"sources": {"orga": src}, "source_commits": ["orga"]}, "source_commits"),
        }
        for version, (doc, needle) in cases.items():
            with self.subTest(version=version):
                self.set("0.22", {"meclaw_release": "0.63.0"})
                self.set(version, doc)
                with self.assertRaises(register.RegisterError) as cm:
                    self.pins()
                self.assertIn("sets/%s.json sources.orga" % version
                              if version != "0.26" else "sets/0.26.json", str(cm.exception))
                self.assertIn(needle, str(cm.exception))
                os.remove(os.path.join(self.sat, "sets", "%s.json" % version))

    def test_set_pins_need_a_register(self):
        os.remove(os.path.join(self.sat, "pipeline", "stations.json"))
        with self.assertRaises(register.RegisterError):
            self.pins()


@unittest.skipUnless(shutil.which("rsync"), "rsync not installed")
class PinPush(PinFixture):
    """The lane verb: the pinned assets and mirrors below the lane user's
    home, additive (nothing there is deleted), verified there."""

    def lane(self):
        from pipeline import lane
        root = os.path.join(self._tmp.name, "lane")
        return lane, lane.Lane(name="lane-a", ssh="", root=root, transport="local")

    def test_lane_push_pins_lands_where_build_set_looks(self):
        lane, ln = self.lane()
        for secret in SECRETS:
            write(self.core, ".git/" + secret, "TOKEN=x\n")
        write(self.home, ".cache/soul2me/meclaw/0.63.0/.env", "TOKEN=x\n")
        wt = os.path.join(self.home, "projeks", "MeClaw", "meclaw-core-wt-x")
        git(self.core, "worktree", "add", "-q", wt)
        there = os.path.join(self._tmp.name, "lanehome")
        # something the lane holds already stays: the push is additive
        write(there, "projeks/orga-hive/.git/lane-only", "keep\n")
        write(there, ".cache/soul2me/meclaw/0.55.0/meclaw", "keep\n")
        got = lane.push_pins(ln, self.pins(), home_root=there)
        self.assertEqual(["0.62.1", "0.63.0"], got["versions"])
        self.assertEqual(["projeks/MeClaw/meclaw-core", "projeks/orga-hive", "projeks/voice2vision"],
                         got["mirrors"])
        cache = _walk(os.path.join(there, ".cache"))
        self.assertEqual(["soul2me/meclaw/0.55.0/meclaw", "soul2me/meclaw/0.62.1/meclaw",
                          "soul2me/meclaw/0.63.0/meclaw"], cache)
        self.assertTrue(os.path.isfile(os.path.join(there, "projeks/orga-hive/.git/lane-only")))
        core = os.path.join(there, "projeks", "MeClaw", "meclaw-core")
        self.assertEqual(self.core_sha, git(core, "rev-parse", self.core_sha[:9] + "^{commit}"))
        self.assertEqual(self.orga_sha, git(os.path.join(there, "projeks", "orga-hive"),
                                            "rev-parse", "orga-1.0.0^{commit}"))
        # this machine's worktree bookkeeping and its secrets stay here
        self.assertFalse(os.path.exists(os.path.join(core, ".git", "worktrees")))
        self.assertEqual([], [p for p in _walk(there) if register.secret_name(p)])

    def test_lane_push_pins_on_a_local_lane_shares_the_home(self):
        lane, ln = self.lane()
        with mock.patch.object(lane, "_rsync", side_effect=AssertionError("copied")):
            self.assertIsNone(lane.push_pins(ln, self.pins()))

    def test_lane_push_pins_refuses_a_lane_without_the_pinned_revision(self):
        lane, ln = self.lane()
        pins = self.pins()
        pins.mirrors[0]["revs"].append("0" * 40)
        with self.assertRaises(lane.LaneError) as cm:
            lane.push_pins(ln, pins, home_root=os.path.join(self._tmp.name, "lanehome"))
        self.assertEqual(2, cm.exception.code)
        self.assertIn("0" * 12, str(cm.exception))

    def test_lane_push_carries_the_pins_with_the_kit(self):
        lane, ln = self.lane()
        pins = self.pins()
        git(self.sat, "init", "-q", "-b", "main")
        git(self.sat, "config", "user.email", "t@example.invalid")
        git(self.sat, "config", "user.name", "t")
        git(self.sat, "add", "-A")
        git(self.sat, "commit", "-q", "-m", "base")
        with mock.patch.object(lane, "push_pins", return_value={"versions": ["x"]}) as pp:
            got = lane.push(ln, self.sat, "soul2me", False, pins=pins)
        pp.assert_called_once_with(ln, pins)
        self.assertEqual({"versions": ["x"]}, got["pins"])

    def test_gate_derives_the_pins_before_anything_travels(self):
        from pipeline import gate
        self.assertEqual({}, gate._satellite_transport(self._tmp.name, False))
        got = gate._satellite_transport(self.sat, True, core_root=self.core, home=self.home)
        self.assertEqual(self.core, got["kit"])
        self.assertEqual(["0.62.1", "0.63.0"], got["pins"].versions)
        self.set("0.22", {"meclaw_release": "0.99.0"})
        with self.assertRaises(register.RegisterError):
            gate._satellite_transport(self.sat, True, core_root=self.core, home=self.home)


if __name__ == "__main__":
    unittest.main()
