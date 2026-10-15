"""Tests for `scripts/pipeline/gate.py` -- `pipeline.py gate` and `pipeline.py test`.

Every case runs the gate in-process against a throw-away repository and a
`local` lane under the test's temporary directory: the plan is patched with
synthetic stations (`claims`/`tree-rules` are cacheable, `clippy` needs the
cargo lock and so the lane), the executor is the real one copied into the
throw-away repository, or a small wrapper around it where a case needs a
stream the executor does not produce on its own.
"""
import copy
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
import types
import unittest
from unittest import mock

from scripts.tests.test_lane_fingerprint import digest
from scripts.pipeline import gate, gitenv, lane, receipt, schedule, store
from scripts.tests import fake_ssh, kit_time, kit_tree, store_isolation

REPO = pathlib.Path(__file__).resolve().parents[2]

GIT_ENV = {
    "GIT_AUTHOR_NAME": "gate test",
    "GIT_AUTHOR_EMAIL": "gate@example.invalid",
    "GIT_COMMITTER_NAME": "gate test",
    "GIT_COMMITTER_EMAIL": "gate@example.invalid",
    "GIT_CONFIG_GLOBAL": os.devnull,
    "GIT_CONFIG_SYSTEM": os.devnull,
}

# An ssh that never reaches anything: an ssh lane stays `off` (made by
# `fake_ssh.install_inert` in `GateCase.setUp`: exit 255 before any command).

# The executor, then a scope stop: the stream says ABORTED, the child dies by SIGTERM.
SCOPE_STOPPED = """import io, json, os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pipeline import executor
d = json.loads(sys.stdin.read())
buf = io.StringIO()
executor.run(d, buf, sys.stderr)
for line in buf.getvalue().splitlines():
    ev = json.loads(line)
    if ev.get("event") == "pass_ended":
        ev["data"]["verdict"] = "ABORTED"
    print(json.dumps(ev), flush=True)
os._exit(143)
"""

# A host that goes away mid-run: two events, no `pass_ended`, ssh's 255.
LOST = """import io, json, os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pipeline import executor
d = json.loads(sys.stdin.read())
buf = io.StringIO()
executor.run(d, buf, sys.stderr)
for line in buf.getvalue().splitlines()[:2]:
    print(line, flush=True)
os._exit(255)
"""

# Z13: a kern that dies at import (the kit lacked a module): no event, no
# workspace on the lane, exit 1.
DIES_AT_IMPORT = """import sys
sys.stdin.read()
sys.stderr.write("ModuleNotFoundError: No module named 'precheck'\\n")
sys.exit(1)
"""


# GR-i: a test's verdict must not hang on this host's disk. The executors run
# in the kit's Python (`lane.PYTHON`, `-I`): no sitecustomize on PYTHONPATH
# reaches them (re-check tv-bn M7m M2, M7p), so instead of faking a full disk
# every GateCase sets the local floors above any disk (re-check tv-bn M7p M2):
# each floor decision of each executor sees the worst case, as with the full
# disk before. A test that needs a floor that lets the pass run sets it to 0.
FLOOR_ABOVE_ANY_DISK_G = 10 ** 9


# GR-5: the first station ends, then the host goes away -- its GATE line is out already.
ONE_STATION = """import io, json, os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pipeline import executor
d = json.loads(sys.stdin.read())
buf = io.StringIO()
executor.run(d, buf, sys.stderr)
for line in buf.getvalue().splitlines():
    print(line, flush=True)
    if json.loads(line).get("event") == "station_ended":
        os._exit(255)
"""


# GR-iii: a watcher aborted the first station, then the host went away -- the
# stream carries the evidence, no `pass_ended`.
ABORT_EVIDENCE = """import io, json, os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pipeline import executor
d = json.loads(sys.stdin.read())
buf = io.StringIO()
executor.run(d, buf, sys.stderr)
for line in buf.getvalue().splitlines():
    ev = json.loads(line)
    if ev.get("event") == "station_ended":
        ev["data"].update(verdict="ABORTED", reason="aborted_deadline")
        print(json.dumps(ev), flush=True)
        os._exit(255)
    print(line, flush=True)
"""


def git(repo, *args):
    # An inherited GIT_DIR/GIT_WORK_TREE wins over `-C`: GateCase.make_repo's
    # `git init` wrote core.worktree into the caller's shared config and its
    # commit "first" landed on the caller's branch (pd-1155). Never inherited.
    env = gitenv.without_repo_env(os.environ)
    return subprocess.run(["git", "-C", str(repo)] + list(args), check=True,
                          capture_output=True, text=True, env=env).stdout


def station(name, cmd="true", cargo=0, inputs=None):
    st = {
        "name": name, "scope": "scope of %s" % name, "cargo": cargo,
        "stage": ["S", "I", "R"],
        "needs": {"cpu": 1, "colony": 0, "browser_chromium": 0, "browser_webkit": 0,
                  "export": 0, "cargo": cargo},
        "inputs": list(inputs if inputs is not None else ["tree"]),
        "cmds": [{"argv": ["sh", "-c", cmd], "cwd": ""}],
        "env": "none", "reason": ["test"], "timeout_min": 1,
        "after": [], "when": {"outcome": "success"}, "run": True, "keep_ws": True,
    }
    return st


class GateCase(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        tmp = self.tmp = pathlib.Path(self._tmp.name)
        self.bin = tmp / "bin"
        self.bin.mkdir()
        fake_ssh.install_inert(self.bin, "an unreachable host: exit 255 before any command",
                               stderr="ssh: unreachable (test)", rc=255)
        store_isolation.write_lane_fakes(self.bin)
        # The tool versions the cache keys carry (re-check tv-bn M7r A-M1: no
        # version, no key). Under the fake HOME rustup's shims answer nothing; these
        # answer `--version` (and `cargo nextest --version`) and hand every other
        # call to the next one on PATH.
        for tool, said in (("rustc", "rustc 0.0.0-fixture"), ("cargo", "cargo 0.0.0-fixture")):
            fake = self.bin / tool
            fake.write_text("#!/bin/sh\ncase \"$*\" in\n  --version) echo '%s' ;;\n"
                            "  'nextest --version') echo 'cargo-nextest 0.0.0-fixture' ;;\n"
                            "  *) PATH=$(printf %%s \"$PATH\" | sed 's|%s:||g') exec %s \"$@\" ;;\n"
                            "esac\n" % (said, self.bin, tool))
            fake.chmod(0o755)
        self.home = tmp / "home"
        self.home.mkdir()
        self.lane_root = tmp / "lane-root"
        self.lanes_file = tmp / "lanes"
        # W1: a local lane is class egon (guarded); a gate there needs
        # MECLAW_LANE_EGON_GATE_OK, the watchdog's journal is a no-op here.
        self.lanes_file.write_text("lane-l  local  %s  class=egon  threads=4  watch=fake.service\n"
                                   % self.lane_root)
        env = dict(GIT_ENV)
        env.update({"PATH": "%s:%s" % (self.bin, os.environ["PATH"]),
                    "HOME": str(self.home),
                    "MECLAW_LANES_FILE": str(self.lanes_file),
                    "MECLAW_PIPELINE_DB": str(tmp / "store.db"),
                    "MECLAW_GATE_LOCK": str(tmp / "cargo.lock"),
                    "MECLAW_PIPELINE_NO_SCOPE": "1",
                    "MECLAW_LANE_EGON_GATE_OK": "test", "MECLAW_LANE_WATCH_CMD": "true",
                    "MECLAW_LANE_FLOOR_G": "0",
                    # GR-i (re-check tv-bn M7p M2): no GateCase judges this host's disk.
                    "MECLAW_GATE_MIN_FREE_G": str(FLOOR_ABOVE_ANY_DISK_G),
                    "MECLAW_GATE_WATCH_FREE_G": str(FLOOR_ABOVE_ANY_DISK_G)})
        patcher = mock.patch.dict(os.environ, env)
        patcher.start()
        self.addCleanup(patcher.stop)
        # The queue merge (`merge.run_gate`) runs this very suite inside its
        # gate with MECLAW_GATE_STRAND_RUN/_WAVE/_NARROW/_CACHE_FROM and
        # MECLAW_ALLOW_BUMP set: none of them may reach a case that does not
        # set them itself (welle-tb tb-zexp, #1164: the merge gate planned
        # narrowed because the queue named a strand run).
        for var in ("MECLAW_GATE_ARCHIVE", "XDG_STATE_HOME", "GH277_WRITE_GOLDEN", "MECLAW_HYGIENE",
                    "MECLAW_GH935_BLESS", "CARGO_TARGET_DIR", "MECLAW_GATE_MODE",
                    "MECLAW_GATE_STRAND_RUN", "MECLAW_GATE_WAVE", "MECLAW_GATE_NARROW",
                    "MECLAW_GATE_CACHE_FROM", "MECLAW_ALLOW_BUMP"):
            os.environ.pop(var, None)
        self.repo = self.make_repo(tmp / "repo")
        cwd = os.getcwd()
        os.chdir(self.repo)
        self.addCleanup(os.chdir, cwd)
        self.rc_file = tmp / "rc-tree-rules"
        sys.modules.pop(gate.PRE_MODULE, None)

    def make_repo(self, repo):
        (repo / "scripts" / "pipeline").mkdir(parents=True)
        shutil.copy(REPO / "scripts" / "pipeline.py", repo / "scripts" / "pipeline.py")
        # the whole kit, its scripts too: the executor's fingerprint is
        # `lane_fingerprint.sh` of the tree it runs in (re-check tv-bn M7q M1)
        for src in (REPO / "scripts" / "pipeline").iterdir():
            if src.is_file():
                shutil.copy(src, repo / "scripts" / "pipeline" / src.name)
        shutil.copy(REPO / "scripts" / "gate_plan.py", repo / "scripts" / "gate_plan.py")
        # `pipeline.py` loads every command module; W4's `pre` imports the precheck.
        shutil.copy(REPO / "scripts" / "precheck.py", repo / "scripts" / "precheck.py")
        (repo / "README.md").write_text("first\n")
        (repo / "Cargo.toml").write_text("[workspace]\nmembers = []\n")
        (repo / "Cargo.lock").write_text("version = 3\n")
        (repo / ".gitignore").write_text("target/\n__pycache__/\n")
        git(repo.parent, "init", "-q", str(repo))
        git(repo, "add", ".")
        git(repo, "commit", "-q", "-m", "first")
        git(repo, "branch", "-M", "master")
        git(repo, "checkout", "-q", "-b", "welle-x/t1")
        return repo

    def rev(self):
        return git(self.repo, "rev-parse", "HEAD").strip()

    def plan(self, *stations):
        stations = list(stations)

        def build_plan(repo_root, mode, base, files=None, only=None, calibration=None, **kw):
            rev = git(repo_root, "rev-parse", "HEAD").strip()
            return dict({"mode": mode, "rev": rev, "base": base or rev, "repo": "meclaw",
                         "classes": [], "crates": [], "stations": copy.deepcopy(stations),
                         "plan_reason": {s["name"]: s["reason"] for s in stations},
                         "critical_path_s": 1.0, "critical_path": [s["name"] for s in stations]},
                        **copy.deepcopy(getattr(self, "plan_extra", {})))

        patcher = mock.patch.object(gate.plan, "build_plan", side_effect=build_plan)
        patcher.start()
        self.addCleanup(patcher.stop)

    def python_plan(self):
        self.plan(station("claims"),
                  station("tree-rules", 'exit "$(cat %s 2>/dev/null || echo 0)"' % self.rc_file))

    def kern_plan(self):
        self.plan(station("clippy", cargo=1, inputs=[]), station("claims"))

    def seed_metadata(self):
        path = pathlib.Path(gate.metadata_path(str(self.repo)))
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps({"packages": [], "workspace_members": [],
                                    "workspace_root": str(self.repo),
                                    "target_directory": str(self.repo / "target")}))

    def gate(self, *argv):
        out, err = io.StringIO(), io.StringIO()
        rc = gate.run_gate(list(argv), out, err)
        return rc, out.getvalue(), err.getvalue()

    def journaled(self):
        """The receipt of the pass the core journaled (`pass_planned`) -- from the store alone."""
        db = store.open()
        runs = [r[0] for r in db.execute("SELECT run FROM journal WHERE event = 'pass_planned'")]
        self.assertEqual(len(runs), 1, runs)
        return receipt.build(db, runs[0])

    @staticmethod
    def verdicts(out):
        return {m.group(1): m.group(2) for m in
                re.finditer(r"^GATE (\S+) \[.*\] \d+s (\S+)", out, re.M)}


class TestGateLocal(GateCase):
    def test_every_executor_here_sees_a_floor_above_its_disk(self):
        """GR-i under the kit's Python (re-check tv-bn M7p M2): the floors every
        GateCase sets reach a child in the kit's Python as its local budget --
        entry and watch floor -- and both lie above the disk that child sees."""
        from scripts.pipeline import lane
        code = ("import json, shutil, sys; sys.path.insert(0, 'scripts');"
                "from pipeline import gate;"
                "print(json.dumps([gate._local_budget([{'name': 'build'}]),"
                " shutil.disk_usage('.').total]))")
        got = subprocess.run(lane.python_argv("-c", code), cwd=REPO, capture_output=True,
                             text=True, timeout=kit_time.HANG)
        self.assertEqual(0, got.returncode, got.stderr)
        budget, total = json.loads(got.stdout)
        self.assertEqual((float(FLOOR_ABOVE_ANY_DISK_G),) * 2,
                         (budget["need_free_g"], budget["free_min_g"]))
        self.assertGreater(FLOOR_ABOVE_ANY_DISK_G, total / 2 ** 30)

    def test_python_only_plan_runs_locally_without_lease(self):
        self.python_plan()
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test")
        self.assertEqual(rc, 0, out + err)
        self.assertEqual(self.verdicts(out), {"claims": "GREEN", "tree-rules": "GREEN"})
        self.assertRegex(out, r"(?m)^GATE-LANE local crit=\d+ lane_s=\d+ rounds=1$")
        self.assertFalse(self.lane_root.exists(), "a python-only plan never touches a lane")
        db = store.open()
        self.assertEqual(db.execute("SELECT count(*) FROM leases").fetchone()[0], 0)
        legacy = self.repo / "target" / "gate" / self.repo.name / ("strand-%s.json" % self.rev())
        self.assertTrue(legacy.is_file())

    def test_local_cargo_below_the_disk_floor_is_aborted_by_hygiene(self):
        # Delta A I-neu-1: a local cargo pass keeps legacy's floor
        # (MECLAW_GATE_MIN_FREE_G; its default 60 G is pinned in-process
        # below) -- here a floor above any disk (GR-i).
        self.kern_plan()
        os.environ["MECLAW_GATE_MIN_FREE_G"] = str(FLOOR_ABOVE_ANY_DISK_G)
        rc, out, err = self.gate("strand", "--lane", "--base", self.rev(), "--no-pre", "--reason", "test")
        self.assertEqual(rc, 2, out + err)
        self.assertEqual(self.verdicts(out), {"clippy": "ABORTED", "claims": "ABORTED"})
        db = store.open()
        checked = [json.loads(r[0]) for r in
                   db.execute("SELECT data FROM journal WHERE event = 'hygiene_checked'")]
        self.assertEqual([(c["verdict"], c["reserved_g"]) for c in checked],
                         [("ABORTED", float(FLOOR_ABOVE_ANY_DISK_G))])
        # The floor is the operator's knob, as in legacy.
        os.environ["MECLAW_GATE_MIN_FREE_G"] = "0"
        rc, out, err = self.gate("strand", "--lane", "--base", self.rev(), "--no-pre", "--reason", "test")
        self.assertEqual(rc, 0, out + err)
        self.assertEqual(self.verdicts(out), {"clippy": "GREEN", "claims": "GREEN"})

    def test_the_plan_line_of_a_local_pass_says_its_cargo(self):
        # Z14: a local pass with a cargo station printed a fixed `cargo=0`.
        self.kern_plan()
        os.environ["MECLAW_GATE_MIN_FREE_G"] = "0"
        rc, out, err = self.gate("strand", "--lane", "--base", self.rev(), "--no-pre", "--reason", "test")
        self.assertEqual(rc, 0, out + err)
        self.assertRegex(out, r"(?m)^GATE-PLAN strand \S+ .* cargo=1 lane=local ")
        # Re-check pd-legacy m-3: the local pass prints the floor's NOTE.
        self.assertIn("gate: NOTE MECLAW_GATE_MIN_FREE_G=0: disk guard off\n", out)
        rc, out, err = self.gate("strand", "--plan-only", "--base", self.rev())
        self.assertEqual(rc, 0, out + err)
        self.assertRegex(out, r"(?m)^GATE-PLAN strand \S+ .* cargo=1 lane=\S+ ")

    def test_the_lane_flag_runs_no_cargo_here_while_the_lanes_are_armed(self):
        # Re-check pd-legacy m-2 (#1139 "or refuses"): `--lane` ran the cargo
        # stations on this machine, past the lanes. Armed, it is refused (exit
        # 3) unless MECLAW_LANE_LOCAL_OK names a reason; a plan without cargo
        # passes.
        self.kern_plan()
        os.environ["MECLAW_GATE_MIN_FREE_G"] = "0"
        db = store.open()
        self.assertEqual(0, schedule.lease_init(db, lanes="lane-l",
                                                lanes_file=str(self.lanes_file)).code)
        db.close()
        argv = ("strand", "--lane", "--base", self.rev(), "--no-pre", "--reason", "test")
        rc, out, err = self.gate(*argv)
        self.assertEqual(rc, 3, out + err)
        self.assertIn("gate: lanes active: --lane runs cargo on this machine", err)
        self.assertNotIn("GATE-SUMMARY", out)
        with mock.patch.dict(os.environ, {"MECLAW_LANE_LOCAL_OK": "owner said so"}):
            rc, out, err = self.gate(*argv)
        self.assertEqual(rc, 0, out + err)
        self.assertIn("gate: NOTE lane-local-ok owner said so", out)
        self.python_plan()
        rc, out, err = self.gate(*argv)
        self.assertEqual(rc, 0, out + err)

    def test_local_python_plan_needs_no_disk_floor(self):
        self.python_plan()
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test")
        self.assertEqual(rc, 0, out + err)

    def test_repeat_of_the_same_tree_is_cached(self):
        self.python_plan()
        self.seed_metadata()
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test")
        self.assertEqual(rc, 0, out + err)
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test")
        self.assertEqual(rc, 0, out + err)
        self.assertEqual(self.verdicts(out), {"claims": "CACHED", "tree-rules": "CACHED"})
        self.assertRegex(out, r"(?m)^GATE-PLAN strand \S+ stations=2 cached=2 ")

    def test_after_red_only_red_runs_again(self):
        self.python_plan()
        self.seed_metadata()
        self.rc_file.write_text("1\n")
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test")
        self.assertEqual(rc, 1, out + err)
        self.assertEqual(self.verdicts(out), {"claims": "GREEN", "tree-rules": "RED"})
        self.rc_file.write_text("0\n")
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test")
        self.assertEqual(rc, 0, out + err)
        self.assertEqual(self.verdicts(out), {"claims": "CACHED", "tree-rules": "GREEN"})

    def test_plan_only_prints_the_tsv_and_the_plan_line(self):
        self.python_plan()
        rc, out, err = self.gate("strand", "--plan-only")
        self.assertEqual(rc, 0, out + err)
        lines = out.splitlines()
        self.assertIn("claims\tscope of claims\t0\tsh -c true\t", lines)
        self.assertIn("tests=false", lines)
        self.assertRegex(out, r"(?m)^gate: base = \S+ ")
        self.assertRegex(lines[-1], r"^GATE-PLAN strand [0-9a-f]{7} stations=2 cached=0 ")
        self.assertFalse((self.repo / "target").exists(), "--plan-only runs nothing")

    def test_the_strand_gate_asks_the_narrowing_and_prints_its_notes(self):
        # Review Z-I1 minor: the gate.py wiring of #1138 was untested.
        self.python_plan()
        with mock.patch.dict(os.environ, {"MECLAW_GATE_MODE": ""}):
            self.gate("strand", "--plan-only")
        self.assertTrue(gate.plan.build_plan.call_args.kwargs.get("narrow"))

    def test_the_merge_gate_plans_unnarrowed(self):
        # Review Z-I1 C2: merge and revert run `gate.sh strand` with
        # MECLAW_GATE_MODE=merge -- they plan the floor, never narrowed.
        self.python_plan()
        with mock.patch.dict(os.environ, {"MECLAW_GATE_MODE": "merge", "MECLAW_GATE_NARROW": "1"}):
            rc, out, err = self.gate("strand", "--plan-only")
        self.assertEqual(rc, 0, out + err)
        self.assertFalse(gate.plan.build_plan.call_args.kwargs.get("narrow"))

    def test_the_merge_gate_with_a_strand_run_asks_the_merge_decision(self):
        # welle-tb S1 part B (#1159): the run of the one strand reaches the plan.
        self.python_plan()
        with mock.patch.dict(os.environ, {"MECLAW_GATE_MODE": "merge", "MECLAW_GATE_NARROW": "1",
                                          "MECLAW_GATE_STRAND_RUN": "tr-000000000001"}):
            rc, out, err = self.gate("strand", "--plan-only")
        self.assertEqual(rc, 0, out + err)
        kw = gate.plan.build_plan.call_args.kwargs
        self.assertTrue(kw.get("narrow"))
        self.assertEqual("merge", kw.get("stage"))
        self.assertEqual("tr-000000000001", kw.get("strand_run"))

    def test_the_integration_gate_names_its_wave(self):
        # welle-tb S1 part C (KR-TB-1): the wave start comes from MECLAW_GATE_WAVE.
        self.python_plan()
        with mock.patch.dict(os.environ, {"MECLAW_GATE_MODE": "", "MECLAW_GATE_WAVE": "welle-x"}):
            self.gate("integration", "--plan-only")
        kw = gate.plan.build_plan.call_args.kwargs
        self.assertTrue(kw.get("narrow"))
        self.assertEqual("welle-x", kw.get("wave"))

    def test_the_integration_gate_takes_the_kit_wave(self):
        # re-check H2 M1: no kit path sets MECLAW_GATE_WAVE -- the wave
        # `strand.sh gate` passes as --wave decides; the environment still wins.
        self.python_plan()
        env = {k: v for k, v in os.environ.items() if k != "MECLAW_GATE_WAVE"}
        with mock.patch.dict(os.environ, dict(env, MECLAW_GATE_MODE=""), clear=True):
            self.gate("integration", "--plan-only", "--wave", "welle-k")
        self.assertEqual("welle-k", gate.plan.build_plan.call_args.kwargs.get("wave"))
        with mock.patch.dict(os.environ, {"MECLAW_GATE_MODE": "", "MECLAW_GATE_WAVE": "welle-m"}):
            self.gate("integration", "--plan-only", "--wave", "welle-k")
        self.assertEqual("welle-m", gate.plan.build_plan.call_args.kwargs.get("wave"))

    def test_the_scope_of_the_tests_is_a_line_and_in_the_receipt(self):
        # re-check H2 M1 (b): narrowed or full, with the base sha -- in GATE-PLAN,
        # as `gate: SCOPE`, and in pass_planned (the receipt from the journal).
        scope = {"tests": "narrowed", "stage": "integration", "since": "0123456789abcdef",
                 "wave": "welle-x", "map": "fedcba987654", "why": None}
        self.plan_extra = {"narrowing": scope}
        self.python_plan()
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test")
        self.assertEqual(rc, 0, out + err)
        self.assertRegex(out, r"(?m)^GATE-PLAN .* scope=narrowed since=012345678\b")
        self.assertIn("gate: SCOPE tests=narrowed stage=integration since=012345678 "
                      "wave=welle-x map=fedcba987 why=-", out)
        self.assertEqual(scope, self.journaled()["narrowing"])

    def test_the_receipt_carries_the_testing_rules_ruling(self):
        # R-TV-W21 (c), P1c § 4 "Wert im Receipt": the ruling a pass ran under
        # (`MECLAW_TESTING_RULES_RULING`) is in pass_planned -- the receipt from
        # the journal -- and in the archived receipt; without it, the key is null.
        self.python_plan()
        archive = self.tmp / "archive"
        with mock.patch.dict(os.environ, {"MECLAW_TESTING_RULES_RULING": "OR-TV-2,OR-TV-7"}):
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test",
                                     "--archive", str(archive))
        self.assertEqual(rc, 0, out + err)
        self.assertEqual("OR-TV-2,OR-TV-7", self.journaled()["testing_ruling"])
        got = json.loads((archive / "latest" / "receipt.json").read_text())
        self.assertEqual("OR-TV-2,OR-TV-7", got["testing_ruling"])
        self.assertEqual([], receipt.validate(got))

    def test_a_receipt_without_a_testing_rules_ruling_says_null(self):
        self.python_plan()
        env = dict(os.environ)
        env.pop("MECLAW_TESTING_RULES_RULING", None)
        with mock.patch.dict(os.environ, env, clear=True):
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test")
        self.assertEqual(rc, 0, out + err)
        self.assertIsNone(self.journaled()["testing_ruling"])

    def test_the_merge_gate_without_a_strand_run_says_floor(self):
        self.python_plan()
        with mock.patch.dict(os.environ, {"MECLAW_GATE_MODE": "merge", "MECLAW_GATE_NARROW": "1",
                                          "MECLAW_GATE_STRAND_RUN": ""}):
            rc, out, err = self.gate("strand", "--plan-only")
        self.assertEqual(rc, 0, out + err)
        self.assertIn("gate: SCOPE tests=floor stage=merge since=- wave=- map=- "
                      "why=no strand run named (MECLAW_GATE_STRAND_RUN)", out)
        self.assertRegex(out, r"(?m)^GATE-PLAN .* scope=floor")

    def real_plan_lib(self):
        """The real plan asks `meclaw_test_features` of scripts/cargo-target.sh
        (GH #1208, R-TV-W18) and fails closed without it, so a case on the real
        plan carries the library. Only `--plan-only` cases: in the sandbox the
        library also arms the runner's tree sync."""
        kit_tree.library(self.repo / "scripts", "cargo-target.sh")

    def test_only_reaches_the_real_plan_as_a_list(self):
        # No mocked plan: the real `plan.build_plan` validates every name.
        self.real_plan_lib()
        with mock.patch.object(gate.plan, "build_plan", wraps=gate.plan.build_plan) as bp:
            rc, out, err = self.gate("strand", "--plan-only", "--only", " claims,,tree-rules ")
        self.assertEqual(rc, 0, out + err)
        self.assertEqual(bp.call_args.kwargs.get("only"), ["claims", "tree-rules"])
        self.assertNotIn("unknown station", err)
        self.assertEqual(gate.plan.build_plan(str(self.repo), "strand", None, only=["claims"])["only"],
                         ["claims"])

    def test_the_real_plan_gets_the_cached_metadata(self):
        # B-I2: without `metadata=` every rdeps()/all()/t0 selection has no parts, so
        # `tests` never caches and `build` builds the whole workspace.
        self.seed_metadata()
        self.real_plan_lib()
        seeded = json.loads(pathlib.Path(gate.metadata_path(str(self.repo))).read_text())
        with mock.patch.object(gate.plan, "build_plan", wraps=gate.plan.build_plan) as bp:
            rc, out, err = self.gate("strand", "--plan-only")
        self.assertEqual(rc, 0, out + err)
        self.assertEqual(bp.call_args.kwargs.get("metadata"), seeded)

    def test_only_reaches_the_executor_as_a_list(self):
        self.python_plan()
        with mock.patch.object(gate.lane, "exec_stream", wraps=lane.exec_stream) as ex:
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--only", "tree-rules")
        self.assertEqual(rc, 0, out + err)
        self.assertEqual(ex.call_args.args[1]["+only"], ["tree-rules"])
        self.assertEqual(self.verdicts(out).get("tree-rules"), "GREEN")
        self.assertNotEqual(self.verdicts(out).get("claims"), "GREEN")

    def test_only_hands_the_executor_the_build_its_stations_wait_for(self):
        # Delta B M-neu1: the plan adds `build` to `--only tests`; the
        # executor gets the same set, never a build skipped "not in --only".
        o = types.SimpleNamespace(no_nice=False, fail_fast=False, no_precheck_stop=False,
                                  only=["tests"], resync=False, decide=[], archive=None,
                                  no_pre=False, reason=None)
        tests = dict(station("tests", cargo=1), after=["build"])
        flags = gate._flags(o, "/lock", [station("build", cargo=1), tests, station("claims")])
        self.assertEqual(flags["+only"], ["tests", "build"])
        flags = gate._flags(o, "/lock", [station("claims")])
        self.assertEqual(flags["+only"], ["tests"])
        o.only = None
        self.assertIsNone(gate._flags(o, "/lock", [tests])["+only"])

    def test_plan_only_tsv_says_1_or_0_for_cargo(self):
        self.plan(station("clippy", cargo=True, inputs=[]), station("claims", cargo=False))
        rc, out, err = self.gate("strand", "--plan-only")
        self.assertEqual(rc, 0, out + err)
        self.assertIn("clippy\tscope of clippy\t1\tsh -c true\t", out.splitlines())
        self.assertIn("claims\tscope of claims\t0\tsh -c true\t", out.splitlines())

    def test_plan_line_counts_run_stations_and_fully_cached_parts(self):
        doc = {"mode": "strand", "rev": "a" * 40,
               "stations": [{"name": "claims"}, {"name": "tests"}, {"name": "slow", "run": False},
                            {"name": "half"}]}
        keys = {("claims", ""): {}, ("tests", "a"): {}, ("tests", "b"): {},
                ("half", "a"): {}, ("half", "b"): {}}
        known = [{"station": "claims", "part": ""}, {"station": "tests", "part": "a"},
                 {"station": "tests", "part": "b"}, {"station": "half", "part": "a"}]
        line = gate._plan_line(doc, known, keys, False, "local", None)
        self.assertRegex(line, r"^GATE-PLAN strand aaaaaaa stations=3 cached=2 reason=- ")

    def test_plan_line_counts_the_hits_the_offered_runs_answered(self):
        # GH #1141: the merge offers its strand runs (MECLAW_GATE_CACHE_FROM);
        # the plan line says how many cache hits came from them.
        doc = {"mode": "strand", "rev": "a" * 40, "stations": [{"name": "tests"}]}
        keys = {("tests", "a"): {}, ("tests", "b"): {}, ("tests", "c"): {}}
        known = [{"station": "tests", "part": "a", "from_run": "run-000000000001"},
                 {"station": "tests", "part": "b", "from_run": "run-000000000009"}]
        with mock.patch.dict(os.environ, {"MECLAW_GATE_CACHE_FROM": "run-000000000001,x"}):
            line = gate._plan_line(doc, known, keys, False, "local", None)
        self.assertTrue(line.endswith(" offer=1/2"), line)
        with mock.patch.dict(os.environ, {"MECLAW_GATE_CACHE_FROM": ""}):
            line = gate._plan_line(doc, known, keys, False, "local", None)
        self.assertNotIn("offer=", line)

    def test_plan_line_names_the_archived_plan(self):
        self.python_plan()
        archive = self.tmp / "archive"
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--archive", str(archive))
        self.assertEqual(rc, 0, out + err)
        m = re.search(r"(?m)^GATE-PLAN strand \S+ stations=2 cached=0 reason=(\S+) ", out)
        self.assertTrue(m, out)
        self.assertTrue(m.group(1).startswith(str(archive) + os.sep), m.group(1))
        self.assertEqual(json.loads(pathlib.Path(m.group(1)).read_text())["stations"][0]["name"], "claims")
        self.assertEqual(os.path.realpath(archive / "latest" / "plan.json"), os.path.realpath(m.group(1)))

    def _local_dispatch(self, *argv):
        self.python_plan()
        with mock.patch.object(gate.lane, "exec_stream", wraps=lane.exec_stream) as ex:
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", *argv)
        self.assertEqual(rc, 0, out + err)
        return ex.call_args.args[1]

    def test_local_run_on_an_egon_machine_takes_the_egon_width(self):
        nproc = os.cpu_count() or 2
        d = self._local_dispatch("--no-nice")
        self.assertEqual(d["capacity"]["cpu"], max(1, nproc // 2))
        self.assertEqual(d["+cpu_overbook"], schedule.CPU_OVERBOOK)
        self.assertIs(d["+nice"], True)
        self.assertIn("CARGO_BUILD_JOBS", d["+env"])
        with mock.patch.object(gate.lane, "env_for", side_effect=lane.LaneError(2, "no lane_sync")):
            d = self._local_dispatch()
        cpu = max(1, nproc // 2)
        self.assertEqual({k: d["+env"][k] for k in ("CARGO_BUILD_JOBS", "NEXTEST_TEST_THREADS")},
                         {"CARGO_BUILD_JOBS": str(cpu), "NEXTEST_TEST_THREADS": "4"})

    def test_local_gate_without_lane_keeps_egon_cap(self):
        # #1158 (S0): a pass without a lane is the class egon -- on the Egon
        # machine and elsewhere, `--no-nice` or not; only the prefix follows it.
        from scripts.pipeline import executor
        for argv in ((), ("--no-nice",)):
            with self.subTest(argv=argv):
                d = self._local_dispatch(*argv)
                self.assertEqual(d[executor.RUNNER_CLASS_KEY], "egon")
        self.lanes_file.write_text("lane-a  builder@lane-a.invalid\n")
        d = self._local_dispatch("--no-nice")
        self.assertEqual(d[executor.RUNNER_CLASS_KEY], "egon")
        self.assertIs(d["+nice"], False)

    def test_local_run_elsewhere_takes_every_core(self):
        self.lanes_file.write_text("lane-a  builder@lane-a.invalid\n")
        d = self._local_dispatch()
        self.assertEqual(d["capacity"]["cpu"], os.cpu_count() or 2)
        self.assertEqual(d["+cpu_overbook"], schedule.CPU_OVERBOOK)
        self.assertNotIn("CARGO_BUILD_JOBS", d["+env"])

    def test_every_station_sees_the_gate_mode(self):
        # gate_legacy.sh exported MECLAW_GATE_MODE for every station (slow_tests.py reads it).
        seen = self.tmp / "mode"
        self.plan(station("claims", "printf %%s \"$MECLAW_GATE_MODE\" > %s" % seen))
        os.environ["MECLAW_GATE_MODE"] = "release"
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test")
        self.assertEqual(rc, 0, out + err)
        self.assertEqual(seen.read_text(), "strand")

    def test_local_pass_is_rebuilt_from_the_store(self):
        self.python_plan()
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test")
        self.assertEqual(rc, 0, out + err)
        self.assertNotIn("gate: store", err)
        doc = self.journaled()
        self.assertEqual((doc.get("mode"), doc.get("rev")), ("strand", self.rev()))
        self.assertTrue(doc.get("plan"), doc)
        self.assertEqual(doc.get("verdict"), "GREEN")

    def test_linked_worktree_writes_its_receipt_into_the_main_trees_target(self):
        # make_export.gate_dir_for reads the receipt there (B-I3).
        self.python_plan()
        wt = self.tmp / "wt-x"
        git(self.repo, "worktree", "add", "-q", "-b", "welle-x/t2", str(wt))
        os.chdir(wt)
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test")
        self.assertEqual(rc, 0, out + err)
        rev = git(wt, "rev-parse", "HEAD").strip()
        self.assertTrue((self.repo / "target" / "gate" / "wt-x" / ("strand-%s.json" % rev)).is_file())
        self.assertFalse((wt / "target").exists())

    def test_cargo_target_dir_wins_for_the_receipt(self):
        self.python_plan()
        os.environ["CARGO_TARGET_DIR"] = str(self.tmp / "tgt")
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test")
        self.assertEqual(rc, 0, out + err)
        self.assertTrue((self.tmp / "tgt" / "gate" / self.repo.name / "last-strand.json").is_file())
        self.assertFalse((self.repo / "target" / "gate").exists())

    def test_line_order_gate_then_summary_then_lane(self):
        self.python_plan()
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test")
        self.assertEqual(rc, 0, out + err)
        kinds = [l.split(" ", 1)[0] for l in out.splitlines() if l.startswith("GATE")]
        self.assertEqual(kinds, ["GATE-PLAN", "GATE", "GATE", "GATE-SUMMARY", "GATE-LANE"])

    def test_the_full_rev_is_printed_before_the_summary(self):
        # W9 (GH #1123): the merge queue accepts a verdict only for its
        # candidate, named by the full rev; the summary carries seven characters.
        self.python_plan()
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test")
        self.assertEqual(rc, 0, out + err)
        lines = out.splitlines()
        revs = [l for l in lines if re.match(r"^gate: rev = [0-9a-f]{40}$", l)]
        self.assertEqual(revs, ["gate: rev = %s" % self.rev()], out)
        summary = [i for i, l in enumerate(lines) if l.startswith("GATE-SUMMARY ")]
        self.assertLess(lines.index(revs[0]), summary[-1], out)

    def strand_worktree(self):
        """A linked worktree on a branch with a `/` -- where the stamp applies (W4)."""
        wt = self.tmp / "wt"
        git(self.repo, "worktree", "add", "-q", "-b", "welle-x/t2", str(wt))
        os.chdir(wt)
        return wt

    def stamp(self, wt, verdict="GREEN", age_s=0):
        from scripts.pipeline import pre
        at = (datetime.datetime.now(datetime.timezone.utc)
              - datetime.timedelta(seconds=age_s)).strftime("%Y-%m-%dT%H:%M:%SZ")
        pre.write_stamp(str(wt), {"tree_hash": pre.tree_hash(str(wt)), "at": at,
                                  "verdict": verdict, "lane_half": True, "reasons": []})

    def test_strand_gate_in_a_strand_worktree_needs_a_green_stamp(self):
        """W4's stamp check moved with `gate` into the pipeline (OR-DT-23): no
        stamp, a RED stamp or one older than 24 h refuses before anything runs."""
        self.python_plan()
        wt = self.strand_worktree()
        rc, out, err = self.gate("strand")
        self.assertEqual(rc, 2, out + err)
        self.assertIn("run strand.sh pre first (no stamp)", err)
        self.assertNotIn("GATE-SUMMARY", out)
        self.stamp(wt, verdict="RED")
        rc, out, err = self.gate("strand")
        self.assertEqual(rc, 2, out + err)
        self.assertIn("was RED", err)
        self.stamp(wt, age_s=25 * 3600)
        rc, out, err = self.gate("strand")
        self.assertEqual(rc, 2, out + err)
        self.assertIn("older than 24 h", err)
        self.stamp(wt)
        rc, out, err = self.gate("strand")
        self.assertEqual(rc, 0, out + err)
        self.assertIn("GATE-SUMMARY", out)

    def test_lane_flag_is_no_way_past_the_stamp_in_a_strand_worktree(self):
        # Delta B M-neu2: `--lane --base` from a strand worktree once skipped
        # the stamp silently (and ran cargo here, without a lease).
        self.python_plan()
        self.strand_worktree()
        rc, out, err = self.gate("strand", "--lane", "--base", self.rev())
        self.assertEqual(rc, 2, out + err)
        self.assertIn("run strand.sh pre first (no stamp)", err)

    def test_no_stamp_outside_a_strand_gate_of_a_strand_worktree(self):
        """The main tree (the fixture repo itself) and the other modes check no stamp."""
        self.python_plan()
        rc, out, err = self.gate("strand")
        self.assertEqual(rc, 0, out + err)
        self.strand_worktree()
        rc, out, err = self.gate("integration")
        self.assertEqual(rc, 0, out + err)
        self.assertNotIn("strand.sh pre first", err)

    def test_no_pre_needs_a_reason_and_lands_in_the_receipt(self):
        self.python_plan()
        self.strand_worktree()
        rc, out, err = self.gate("strand", "--no-pre")
        self.assertEqual(rc, 2, out + err)
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "measured elsewhere")
        self.assertEqual(rc, 0, out + err)
        self.assertIn("pre-check skipped: measured elsewhere", err)
        doc = self.journaled()
        self.assertIn({"station": "pre", "decision": "pre_skipped", "reason": "measured elsewhere"},
                      [{k: d.get(k) for k in ("station", "decision", "reason")}
                       for d in doc["owner_decisions"]])

    def test_arguments_are_checked_like_gate_sh(self):
        self.python_plan()
        for argv in (["nope"], ["strand", "--bogus"], ["strand", "--lane"],
                     ["strand", "--decide", "persona-receipt=without:x"],
                     ["integration", "--decide", "claims=without:x"],
                     ["integration", "--decide", "persona-receipt=with:x"],
                     ["integration", "--decide", "persona-receipt=without: "],
                     ["integration", "--decide", "persona-receipt=without:a\tb"],
                     # OR-DT.ZI.decide-or: the reason cites the owner's ruling.
                     ["integration", "--decide", "persona-receipt=without:no ruling id"],
                     ["integration", "--decide", "persona-receipt=without:OR- bare prefix"]):
            rc, out, err = self.gate(*argv)
            self.assertEqual(rc, 2, "%s: %s" % (argv, out + err))

    def test_decide_wants_a_ruling_id_in_the_reason(self):
        # OR-DT.ZI.decide-or (review R3 M1): free text alone is no owner's call.
        err = gate._decide_error("integration", "persona-receipt=without:no time")
        self.assertIn("OR-", err)
        for why in ("OR-DT.ZI.decide-or no fresh run", "per OR-7: ships as is"):
            self.assertEqual("", gate._decide_error("release", "persona-receipt=without:" + why),
                             why)

    def test_pipeline_py_hands_every_word_to_the_gate(self):
        from scripts.pipeline import cli
        with mock.patch.object(sys, "stderr", io.StringIO()) as err:
            self.assertEqual(cli.main(["gate", "strand", "--bogus"]), 2)
        self.assertIn("unknown argument: --bogus", err.getvalue())
        with mock.patch.object(sys, "stdout", io.StringIO()) as out:
            self.assertEqual(cli.main(["gate", "-h"]), 0)
        self.assertIn("usage: pipeline.py gate", out.getvalue())


# An ssh that reaches "the host" by running the command here (the gate cases:
# W1 runs a gate on a build host only -- a local lane is class egon, test only).
# A real ssh carries none of the caller's environment (GH #1209): the names of
# the one rule (fake_ssh.py) reach the lane only as words of the command line.
PASS_SSH_TEMPLATE = """#!/usr/bin/env bash
while [ $# -gt 0 ]; do case "$1" in -o|-l|-p|-i|-E|-F) shift 2 ;; -*) shift ;; *) break ;; esac; done
case "$1" in *lane-l*) ;; *) echo "ssh: unreachable (test)" >&2; exit 255 ;; esac
shift
# @fake-ssh: strip the transported names@
exec bash -c "$*"
"""
PASS_SSH = fake_ssh.script(PASS_SSH_TEMPLATE)

# I1: the lane's `systemd-run --scope` -- drops its options, runs the rest.
PASS_SYSTEMD_RUN = """#!/usr/bin/env bash
while [ $# -gt 0 ]; do case "$1" in -*) shift ;; *) break ;; esac; done
exec "$@"
"""


class StationEnvCase(GateCase):
    """Re-check tv-bn M7t A-M2 (OR-T11-22): the gate's cache lookup of a lane pass
    rebuilt the stations' environment as `PATH + +env`, while the executor on a
    lane of transport `local` inherits the gate's process (`lane.exec_stream`
    starts it without `env=`): a RUSTFLAGS there was built with and never looked up
    with -- CACHED across a change of the build. One rule, `executor.station_env`,
    on both sides; pinned through `_run_lane`."""

    MOLD = "--cfg tokio_unstable -C link-arg=-fuse-ld=mold"

    def spy(self):
        looked, sent = [], []
        real_lookup, real_stream = gate._lookup, gate.lane.exec_stream

        def lookup(*a, **kw):
            # the build part the keys are made with (orchestrator OR-T12-18 A-M3: no default)
            looked.append(dict(a[8] if len(a) > 8 else kw["build"]))
            return real_lookup(*a, **kw)

        def stream(ln, dispatch, *a, **kw):
            sent.append((ln, dispatch))
            return real_stream(ln, dispatch, *a, **kw)

        for name, fake in (("_lookup", lookup),):
            patcher = mock.patch.object(gate, name, side_effect=fake)
            patcher.start()
            self.addCleanup(patcher.stop)
        patcher = mock.patch.object(gate.lane, "exec_stream", side_effect=stream)
        patcher.start()
        self.addCleanup(patcher.stop)
        return looked, sent

    # Two cargo configurations of the same name and place, different content
    # (re-check bundle N delta kit-fp M1): the measure keys the content.
    CARGO_CONFIGS = ('[build]\nrustflags = ["-C", "target-cpu=native"]\n',
                     '[build]\nrustflags = ["-C", "target-cpu=x86-64-v2"]\n')

    def cargo_config_over(self):
        """The directory whose `.cargo/` the pass's measure reads: here (lane of
        transport `local`) the stations' tree is `repo`, measured from its parent."""
        return self.repo.parent

    def write_cargo_config(self, text):
        conf = self.cargo_config_over() / ".cargo"
        conf.mkdir(parents=True, exist_ok=True)
        (conf / "config.toml").write_text(text)

    def plan_only_is_the_run(self, rc=0):
        """Plan == run with a `.cargo/config.toml` over the measured tree, once per
        content of `CARGO_CONFIGS` (re-check bundle N delta M1, N-K2): the key is
        there on both sides and moves with the file's content -- two empty lists, or
        a measure of the file's name only, compared equal and pinned nothing."""
        seen = []
        for text in self.CARGO_CONFIGS:
            self.write_cargo_config(text)
            seen.append(self.plan_and_run(rc))
        for build in seen:
            self.assertRegex(build["cargo_config"] or "", r"^[0-9a-f]{16}$", build)
        self.assertNotEqual(seen[0]["cargo_config"], seen[1]["cargo_config"])
        return seen[-1]

    def plan_and_run(self, rc):
        self.kern_plan()
        looked, _sent = self.spy()
        keys = []
        real = gate.cache.keys_for_plan

        def keys_for_plan(*a, **kw):
            keys.append(sorted((k, v["key"]) for k, v in real(*a, **kw).items()))
            return real(*a, **kw)

        with mock.patch.dict(os.environ, {"RUSTFLAGS": self.MOLD}), \
                mock.patch.object(gate.cache, "keys_for_plan", side_effect=keys_for_plan):
            rc_p, out, err = self.gate("strand", "--plan-only", "--host", "lane-l")
            self.assertEqual(0, rc_p, out + err)
            rc_r, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(rc, rc_r, out + err)
        self.assertEqual(2, len(looked), looked)
        self.assertEqual(looked[0], looked[1])
        self.assertIsNotNone(looked[0]["build_env"])
        self.assertEqual(digest("strand"), looked[0]["build_env"].get("MECLAW_GATE_MODE"))
        if keys:   # a fixture with cargo metadata: the same keys too
            self.assertEqual(2, len(keys))
            self.assertEqual(keys[0], keys[1])
        return looked[0]

    def pass_with_rustflags(self):
        from scripts.pipeline import executor
        self.kern_plan()
        looked, sent = self.spy()
        with mock.patch.dict(os.environ, {"RUSTFLAGS": self.MOLD}):
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
            environ = dict(os.environ)
        self.assertEqual(0, rc, out + err)
        self.assertEqual(1, len(looked), looked)
        ln, dispatch = sent[-1]
        return looked[0], dispatch, ln, environ, executor


class TestPlanOnlyIsTheLocalRun(StationEnvCase):
    """Re-check bundle N kit-fp M3 (N-K3, rest of A-M3): `--plan-only` of a pass that
    runs HERE built the run's `+env` a second time (`_local_width` + the mode in
    `_plan_build` beside `_run_local`), and no pin held the two together -- the
    plan-only pin ran the lane branch only (`--host`; mutant P1, the local branch
    with `{}`, stayed green). One source (`gate._local_plus_env`), pinned through
    both local branches: a plan without cargo, and `--lane`."""

    def local_plan_only_is_the_run(self, *argv):
        looked, _sent = self.spy()
        with mock.patch.dict(os.environ, {"RUSTFLAGS": self.MOLD, "MECLAW_GATE_MIN_FREE_G": "0"}):
            rc_p, out, err = self.gate("strand", "--plan-only", *argv)
            self.assertEqual(0, rc_p, out + err)
            rc_r, out, err = self.gate("strand", "--no-pre", "--reason", "test", *argv)
        self.assertEqual(0, rc_r, out + err)
        self.assertEqual(2, len(looked), looked)
        self.assertEqual(looked[0], looked[1])
        self.assertIsNotNone(looked[0]["build_env"])
        self.assertEqual(digest("strand"), looked[0]["build_env"].get("MECLAW_GATE_MODE"))
        self.assertEqual(digest(self.MOLD), looked[0]["build_env"].get("RUSTFLAGS"))

    def test_a_plan_without_cargo(self):
        self.python_plan()
        self.local_plan_only_is_the_run()

    def test_a_cargo_plan_with_the_lane_flag(self):
        self.kern_plan()
        self.local_plan_only_is_the_run("--lane", "--base", self.rev())


class TestStationEnvironmentLocal(StationEnvCase):
    """OR-T12-5, the seam pin: a gate pass never runs on a lane of transport
    `local` -- `lane.guard` refuses a local lane without class egon (W1), the
    admission a class egon lane in a gate mode (`schedule.EGON_MODES`). The
    fixture is a local class=lane lane (configuration only, no patch of the
    guard): the pass runs to its lookup and is refused right after (exit 5).
    Both sides of the seam are pinned: the lookup takes `station_env` over THIS
    process's environment, `lane.exec_stream` starts a local executor without
    `env=` (it inherits that environment), and the executor's `base_env` is
    `station_env` over what it inherits -- one environment on both sides."""

    def setUp(self):
        super().setUp()
        self.lane_root.mkdir(exist_ok=True)
        self.lanes_file.write_text("lane-l  local  %s  threads=4  class=lane\n" % self.lane_root)

    def test_the_lookup_takes_the_station_env_of_this_process(self):
        from scripts.pipeline import executor
        self.kern_plan()
        looked, sent = self.spy()
        calls, real = [], executor.station_env

        def station_env(plus_env, environ):
            calls.append((dict(plus_env or {}), environ))
            return real(plus_env, environ)

        with mock.patch.object(executor, "station_env", side_effect=station_env), \
                mock.patch.dict(os.environ, {"RUSTFLAGS": self.MOLD}):
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
            self.assertEqual(1, len(calls), calls)
            plus_env, environ = calls[0]
            # the environment the local executor inherits: this process's, by identity
            self.assertIs(os.environ, environ)
            want = executor.build_fingerprint(real(plus_env, os.environ), str(self.repo))
        # the guard refuses after the lookup: nothing travels, no executor starts
        self.assertEqual(5, rc, out + err)
        self.assertIn("a local lane is class egon", err)
        self.assertEqual([], sent)
        self.assertEqual([want], looked)
        self.assertEqual(digest(self.MOLD), want["build_env"].get("RUSTFLAGS"))
        self.assertEqual("mold", want["linker"])
        # the dispatch's `+env` is complete before the lookup (the mode travels in it)
        self.assertEqual("strand", plus_env.get("MECLAW_GATE_MODE"))

    def test_plan_only_keys_with_the_runs_build(self):
        """Orchestrator OR-T12-18 A-M3: `--plan-only` keys with the run's build part."""
        self.plan_only_is_the_run(rc=5)

    def test_a_local_executor_inherits_and_takes_the_same_rule(self):
        from scripts.pipeline import executor
        ln = lane.find_lane("lane-l")
        self.assertEqual("local", ln.transport)
        started = []

        def popen(argv, **kw):
            started.append(kw)
            raise OSError("captured")

        with mock.patch.object(lane.subprocess, "Popen", side_effect=popen):
            with self.assertRaisesRegex(lane.LaneError, r"the executor on lane-l did not start \(captured\)"):
                lane.exec_stream(ln, {"run": "r-seam", "tree": str(self.lane_root)})
        self.assertEqual(1, len(started), started)
        self.assertNotIn("env", started[0])  # inherits this process's environment
        plus = {"MECLAW_GATE_MODE": "strand", "RUSTFLAGS": "-C opt-level=1"}
        d = {"run": "r-seam", "tree": str(self.lane_root), "target": str(self.lane_root),
             "ws": str(self.lane_root), "+env": plus}
        with mock.patch.dict(os.environ, {"RUSTFLAGS": self.MOLD}):
            run = executor._Run(d, io.StringIO(), io.StringIO(), None)
            self.assertEqual(executor.station_env(plus, os.environ), run.base_env)


class TestGateLane(GateCase):
    def setUp(self):
        super().setUp()
        fake_ssh.install(self.bin, PASS_SSH_TEMPLATE)
        self.lanes_file.write_text("lane-l  builder@lane-l.invalid  threads=4  class=lane\n")
        os.environ["MECLAW_LANE_ROOT"] = str(self.lane_root)
        # The suite's disk is full (GR-i): the lane's budget asks for nothing free.
        real = lane.exec_stream

        def exec_stream(ln, dispatch, *a, **kw):
            dispatch = dict(dispatch, budget=dict(dispatch.get("budget") or {},
                                                  need_free_g=0, free_min_g=0, reserve_g=0))
            return real(ln, dispatch, *a, **kw)

        patcher = mock.patch.object(gate.lane, "exec_stream", side_effect=exec_stream)
        patcher.start()
        self.addCleanup(patcher.stop)

    def test_kern_writes_local_legacy_receipt_for_make_export(self):
        self.kern_plan()
        archive = self.tmp / "archive"
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l",
                                 "--archive", str(archive))
        self.assertEqual(rc, 0, out + err)
        self.assertEqual(self.verdicts(out), {"clippy": "GREEN", "claims": "GREEN"})
        # GH #1162: a lane pass names the lane's threads (`vcpu=`, lanes file threads=4).
        self.assertRegex(out, r"(?m)^GATE-LANE lane-l crit=\d+ lane_s=\d+ rounds=1 vcpu=4$")
        gate_dir = self.repo / "target" / "gate" / self.repo.name
        legacy = json.loads((gate_dir / ("strand-%s.json" % self.rev())).read_text())
        self.assertEqual(legacy["verdict"], "GREEN")
        self.assertEqual(sorted(s["name"] for s in legacy["stations"]), ["claims", "clippy"])
        self.assertEqual(json.loads((gate_dir / "last-strand.json").read_text()), legacy)
        latest = archive / "latest"
        for name in ("receipt.json", "summary.txt", "events.jsonl", "plan.json"):
            self.assertTrue((latest / name).is_file(), name)
        self.assertTrue((latest / "logs").is_dir())
        self.assertIn("GATE-SUMMARY", (latest / "summary.txt").read_text())

    def test_lane_pass_dispatches_the_store_runner_class(self):
        # #1158 (S0): the gate's flags (`+nice` from `--no-nice`) never stand in
        # for the runner class; the store's `runners.class` travels as its own key.
        from scripts.pipeline import executor
        self.kern_plan()
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(rc, 0, out + err)
        d = gate.lane.exec_stream.call_args.args[1]
        self.assertEqual((d[executor.RUNNER_CLASS_KEY], d["+nice"]), ("lane", True))
        # `--no-nice` drops the prefix and leaves the class alone.
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l",
                                 "--no-nice")
        self.assertEqual(rc, 0, out + err)
        d = gate.lane.exec_stream.call_args.args[1]
        self.assertEqual((d[executor.RUNNER_CLASS_KEY], d["+nice"]), ("lane", False))
        # The class is the store's row: an egon runner reads `egon`, an unknown one None.
        db = store.open()
        self.assertEqual(gate._runner_class(db, "lane-l"), "lane")
        db.execute("UPDATE runners SET class = 'egon' WHERE name = 'lane-l'")
        self.assertEqual(gate._runner_class(db, "lane-l"), "egon")
        self.assertIsNone(gate._runner_class(db, "lane-gone"))

    def test_lane_pass_without_a_store_class_keeps_egon_cap(self):
        # Fail-closed: a store that cannot name the class leaves the key empty,
        # and the executor reads an empty key as Egon's cap.
        from scripts.pipeline import executor
        self.kern_plan()
        with mock.patch.object(gate, "_runner_class", return_value=None):
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(rc, 0, out + err)
        d = gate.lane.exec_stream.call_args.args[1]
        self.assertIsNone(d.get(executor.RUNNER_CLASS_KEY))
        self.assertTrue(executor.egon_capped(d))

    # #1158 B-1: only an explicit `class=lane` runs the booked width; a line
    # without a class (an ssh line that reaches the Egon machine) or with an
    # unknown one keeps the Egon cap, the unknown one with a NOTE.
    def _class_pass(self, line):
        from scripts.pipeline import executor
        self.lanes_file.write_text(line + "\n")
        self.kern_plan()
        name = line.split()[0]
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", name)
        self.assertEqual(rc, 0, out + err)
        d = gate.lane.exec_stream.call_args.args[1]
        env = {}
        executor._book_threads(env, {"needs": {"cpu": 12}}, egon=executor.egon_capped(d))
        return d, env["CARGO_BUILD_JOBS"], err

    def test_lane_pass_ssh_line_without_class_keeps_egon_cap(self):
        d, jobs, err = self._class_pass("egon-box  builder@lane-l.invalid")
        self.assertNotEqual("lane", d.get("+runner_class"))
        self.assertEqual("4", jobs)
        self.assertIn("gate: NOTE", err)

    def test_lane_pass_unknown_class_keeps_egon_cap_with_a_note(self):
        d, jobs, err = self._class_pass("lane-l  builder@lane-l.invalid  class=Lane")
        self.assertEqual("4", jobs)
        self.assertRegex(err, r"gate: NOTE .*lane-l.*'Lane'.*Egon cap")

    def test_lane_pass_build_host_with_class_lane_runs_nproc(self):
        d, jobs, err = self._class_pass("build02  builder@lane-l.invalid  class=lane")
        self.assertEqual("lane", d.get("+runner_class"))
        self.assertEqual("12", jobs)
        self.assertEqual(os.cpu_count(), d["capacity"]["cpu"])
        self.assertNotIn("Egon cap", err)

    def test_the_lane_pass_prints_the_full_rev_too(self):
        # W9 (GH #1123): same line on the pipeline lane path. `merge --host` does
        # not read it (lane_sync.sh run -> gate.sh --lane -> legacy runner), and
        # the rev printed here is the dispatched one, not the lane's report.
        self.kern_plan()
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(rc, 0, out + err)
        self.assertRegex(out, r"(?m)^gate: rev = %s$" % self.rev())

    def test_a_core_plan_without_cargo_stays_local_despite_host(self):
        # Delta A M-neu-4: the admission put the pass in `wt/<name>` (no
        # station needs cargo there); guard, hold and push follow it. Z17:
        # the fixture's `build` without `needs` was the one plan the routing
        # (`gate._needs_cargo`, name rule) and the admission (`schedule._needs`)
        # read differently; with one rule a core plan without cargo never
        # reaches a lane -- `wt/<name>` is the satellites' (W10 pins), and
        # `tree/` stays untouched either way.
        self.plan(station("shellcheck"), station("claims"))
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(rc, 0, out + err)
        self.assertFalse((self.lane_root / "tree").exists())
        # Review Z-A R2 M3: it never reached the lane -- local, no lane root.
        self.assertFalse(self.lane_root.exists())
        self.assertRegex(out, r"(?m)^GATE-PLAN strand \S+ .* lane=local\b")
        # Z14: the plan line says the pass's cargo, not a fixed 1.
        self.assertRegex(out, r"(?m)^GATE-PLAN strand \S+ .* cargo=0 ")

    def test_a_satellite_plan_without_cargo_goes_to_wt_name_on_the_lane(self):
        # Review Z-A2 R1 M2 (GH #1155): the glue between the admission and the
        # lane -- a satellite pass without cargo is guarded and pushed with
        # cargo=False into `wt/<repo-name>`, `tree/` stays untouched.
        rev = self.rev()
        doc = {"mode": "strand", "rev": rev, "base": rev, "repo": "sat",
               "classes": [], "crates": [], "stations": [station("shellcheck"), station("claims")],
               "plan_reason": {"shellcheck": ["test"], "claims": ["test"]},
               "critical_path_s": 1.0, "critical_path": ["shellcheck", "claims"]}
        guarded, pushed = [], []
        real_guard = gate.lane.guard

        def guard(ln, name, cargo=True, **kw):
            got = real_guard(ln, name, cargo=cargo, **kw)
            guarded.append((name, cargo, got[0]))
            return got

        def push(ln, repo, name, cargo, **kw):
            pushed.append((name, cargo, kw.get("kit")))
            raise gate.lane.LaneError(2, "push stopped by the test")

        patches = [mock.patch.object(gate.satellites, "is_satellite", return_value=True),
                   mock.patch.object(gate, "_satellite_plan", side_effect=lambda *a, **kw: copy.deepcopy(doc)),
                   mock.patch.object(gate, "_satellite_transport",
                                     return_value={"kit": str(self.repo)}),
                   mock.patch.object(gate, "_lane_env", return_value={}),  # no register here
                   mock.patch.object(gate.lane, "guard", side_effect=guard),
                   mock.patch.object(gate.lane, "push", side_effect=push)]
        for p in patches:
            p.start()
            self.addCleanup(p.stop)
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(2, rc, out + err)
        self.assertIn("push stopped by the test", err)
        name = self.repo.name
        self.assertEqual([(name, False, "%s/wt/%s" % (self.lane_root, name))], guarded)
        self.assertEqual([(name, False, str(self.repo))], pushed)
        self.assertFalse((self.lane_root / "tree").exists())
        # Z14 on the lane path: the satellite's plan line says cargo=0 as well.
        self.assertRegex(out, r"(?m)^GATE-PLAN strand \S+ .* cargo=0 ")

    def _satellite_on_a_lane(self, *argv):
        """welle-tb S3 F1: a satellite pass with the push stopped -> (rc, out, err, pushed)."""
        rev = self.rev()
        doc = {"mode": "strand", "rev": rev, "base": rev, "repo": "sat",
               "classes": [], "crates": [], "stations": [station("shellcheck")],
               "plan_reason": {"shellcheck": ["test"]},
               "critical_path_s": 1.0, "critical_path": ["shellcheck"]}
        pushed = []

        def push(ln, repo, name, cargo, **kw):
            pushed.append(ln.name)
            raise gate.lane.LaneError(2, "push stopped by the test")

        local = mock.MagicMock(side_effect=AssertionError("a satellite ran locally"))
        patches = [mock.patch.object(gate.satellites, "is_satellite", return_value=True),
                   mock.patch.object(gate, "_satellite_plan",
                                     side_effect=lambda *a, **kw: copy.deepcopy(doc)),
                   mock.patch.object(gate, "_satellite_transport",
                                     return_value={"kit": str(self.repo)}),
                   mock.patch.object(gate, "_lane_env", return_value={}),  # no register here
                   mock.patch.object(gate.lane, "push", side_effect=push),
                   mock.patch.object(gate, "_run_local", local)]
        for p in patches:
            p.start()
            self.addCleanup(p.stop)
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", *argv)
        local.assert_not_called()
        return rc, out, err, pushed

    def test_a_satellite_pass_without_host_is_admitted_to_a_lane_never_here(self):
        # Re-check tb-s3 h1 M-1 (Koordinator): without --host a satellite ran here,
        # on mm-os01 beside the live colony; the admission books it a lane now.
        rc, out, err, pushed = self._satellite_on_a_lane()
        self.assertEqual(2, rc, out + err)
        self.assertIn("push stopped by the test", err)
        self.assertEqual(["lane-l"], pushed)

    def test_a_satellite_pass_with_the_lane_flag_is_refused(self):
        rc, out, err, pushed = self._satellite_on_a_lane("--lane", "--base", self.rev())
        self.assertEqual(2, rc, out + err)
        self.assertIn("a satellite pass never runs on this machine (--lane)", err)
        self.assertEqual([], pushed)

    def test_a_local_pass_dispatches_its_colonies_resolved_from_one_source(self):
        # Re-check tb-s3 h1 m-2 + M-1: `_run_local` resolves COLONY_REST from
        # `colony_capacity` (the S5 source), at least the widest fixed booking, so
        # builder 4 / memory beside browser 3 are never RED before start here.
        st = station("lab-rest")
        st["needs"]["colony"] = gate.plan.COLONY_REST
        self.plan(st)
        with mock.patch.object(gate.lane, "exec_stream", wraps=lane.exec_stream) as ex:
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test")
        self.assertEqual(rc, 0, out + err)
        d = ex.call_args.args[1]
        nproc = os.cpu_count() or 2
        want = schedule.colony_capacity(gate.satellites.local_cores(), nproc)
        self.assertEqual(want, d["capacity"]["colony"])
        widest = max(int(n.get("colony", 0)) for n in gate.plan.NEEDS.values())
        self.assertGreaterEqual(d["capacity"]["colony"], widest)
        self.assertEqual([gate.plan.resolve_colony(gate.plan.COLONY_REST, want)],
                         [s["needs"]["colony"] for s in d["stations"]])

    def test_a_lane_pass_dispatches_its_colonies_resolved_on_the_lane(self):
        # Re-check tb-s3 h1 m-2: `_run_lane` resolves COLONY_REST with the admitted
        # lane's colonies (`cores=6` -> 6, rest beside browser 3 = 3).
        self.lanes_file.write_text("lane-l  builder@lane-l.invalid  cores=6 class=lane\n")
        st = station("lab-rest")
        st["needs"]["colony"] = gate.plan.COLONY_REST
        self.plan(station("clippy", cargo=1, inputs=[]), st)
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(rc, 0, out + err)
        d = gate.lane.exec_stream.call_args.args[1]
        self.assertEqual(6, d["capacity"]["colony"])
        self.assertEqual(3, {s["name"]: s for s in d["stations"]}["lab-rest"]["needs"]["colony"])

    def test_the_flake_issue_duty_runs_after_the_lane_hold_is_free(self):
        # Z-A (OR-DT.W8.S.1): the gh calls of the issue duty (up to 2 x 60 s
        # per pending test) ran with the lane's build path still held.
        self.kern_plan()
        holds, seen = [], []
        real_hold = gate.lane.hold

        def hold(*a, **kw):
            holds.append(real_hold(*a, **kw))
            return holds[-1]

        def ingest(*a, **kw):
            seen.append([h.proc is None for h in holds])

        with mock.patch.object(gate.lane, "hold", side_effect=hold), \
                mock.patch.object(gate, "_flake_ingest", side_effect=ingest):
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(rc, 0, out + err)
        self.assertEqual([[True]], seen)

    def test_set_pins_that_cannot_travel_abort_before_the_push(self):
        # GH #1143: a RegisterError of the pin derivation closes the pass
        # ABORTED with exit 2, nothing is pushed, the build path is free again.
        self.kern_plan()
        holds = []
        real_hold = gate.lane.hold

        def hold(*a, **kw):
            holds.append(real_hold(*a, **kw))
            return holds[-1]

        refused = gate.satellites.RegisterError("set pins of sat cannot travel:\n  sets/0.9.json: x")
        with mock.patch.object(gate.lane, "hold", side_effect=hold), \
                mock.patch.object(gate, "_satellite_transport", side_effect=refused), \
                mock.patch.object(gate.lane, "push") as push, \
                mock.patch.object(gate.schedule, "close", wraps=schedule.close) as close:
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(2, rc, out + err)
        self.assertIn("gate: set pins of sat cannot travel", err)
        self.assertIn("sets/0.9.json: x", err)
        push.assert_not_called()
        self.assertEqual(1, close.call_count, close.call_args_list)
        self.assertEqual("ABORTED", close.call_args.args[2]["data"]["verdict"])
        self.assertEqual([True], [h.proc is None for h in holds])

    def test_every_lane_station_sees_the_gate_mode(self):
        seen = self.tmp / "mode"
        self.plan(station("clippy", cargo=1, inputs=[]),
                  station("claims", "printf %%s \"$MECLAW_GATE_MODE\" > %s" % seen))
        os.environ["MECLAW_GATE_MODE"] = "release"
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(rc, 0, out + err)
        self.assertEqual(seen.read_text(), "strand")

    def test_a_declared_lane_variable_reaches_the_lane_station(self):
        # welle-tb tb-zcol A: the register's `lane_env` travels in the
        # dispatch's `+env` (Z: SOUL2ME_LAB_MECLAW_BIN never arrived).
        seen = self.tmp / "lane-env"
        self.plan(station("clippy", cargo=1, inputs=[]),
                  station("claims", "printf %%s \"$SOUL2ME_LAB_MECLAW_BIN\" > %s" % seen))
        with mock.patch.object(gate, "_lane_env",
                               return_value={"SOUL2ME_LAB_MECLAW_BIN": "/lane/bin"}) as le:
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(rc, 0, out + err)
        self.assertEqual(seen.read_text(), "/lane/bin")
        self.assertIn("gate: NOTE lane lane-l: passes SOUL2ME_LAB_MECLAW_BIN", out)
        le.assert_called_once()

    def test_a_lane_variable_that_cannot_travel_aborts_before_the_push(self):
        self.kern_plan()
        refused = gate.satellites.RegisterError("lane_env SOUL2ME_LAB_MECLAW_BIN: x")
        with mock.patch.object(gate, "_lane_env", side_effect=refused), \
                mock.patch.object(gate.lane, "push") as push:
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(2, rc, out + err)
        self.assertIn("lane_env SOUL2ME_LAB_MECLAW_BIN: x", err)
        push.assert_not_called()

    def test_lane_pass_is_rebuilt_from_the_store(self):
        self.kern_plan()
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(rc, 0, out + err)
        self.assertNotIn("gate: store", err)
        doc = self.journaled()
        self.assertEqual((doc.get("mode"), doc.get("rev")), ("strand", self.rev()))
        self.assertTrue(doc.get("plan"), doc)

    def test_gate_line_is_out_before_the_stream_dies(self):
        self.kern_plan()
        (self.repo / "scripts" / "pipeline.py").write_text(ONE_STATION)
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(rc, 2, out + err)
        self.assertEqual(len(self.verdicts(out)), 1, out)
        self.assertRegex(out, r"(?m)^GATE (clippy|claims) \[scope of \w+\] \d+s GREEN$")

    def test_exit_from_pass_ended_when_scope_stopped(self):
        self.kern_plan()
        (self.repo / "scripts" / "pipeline.py").write_text(SCOPE_STOPPED)
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(rc, 2, out + err)
        self.assertRegex(out, r"(?m)^GATE-SUMMARY strand \S+ \d+/\d+ \d+s ABORTED$")

    def test_stream_without_pass_ended_closes_lost(self):
        self.kern_plan()
        (self.repo / "scripts" / "pipeline.py").write_text(LOST)
        with mock.patch.object(gate.schedule, "close", wraps=schedule.close) as close:
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(rc, 2, out + err)
        self.assertTrue(close.called)
        self.assertTrue(close.call_args.kwargs.get("lost"), close.call_args)
        self.assertIn("lost", err)

    # Z17 (M-neu-1 of the Z13b re-check): ONE cargo rule for plan-only, the
    # local pass and the lane's admission -- a `build` without `needs` and a
    # legacy plan's `cargo` flag printed different `cargo=` per path.
    def test_one_cargo_predicate_for_plan_only_and_the_lane(self):
        cases = [{"name": "build"}, {"name": "x", "cargo": 1}, {"name": "x", "cargo": 0},
                 {"name": "x", "needs": {"cargo": 1}}, {"name": "x", "needs": {"cpu": 2}},
                 {"name": "build", "needs": {"cargo": 0}}, {"name": "x", "needs": {}, "cargo": 1}]
        for st in cases:
            with self.subTest(st=st):
                self.assertEqual(gate._needs_cargo(st), schedule._needs(st)["cargo"] >= 1)

    # Z13: run-696ff322bd86 died at import on build03 (ModuleNotFoundError
    # after 10 s), the fetch failed, and the admission kept the lane's cpu
    # until its deadline -- two runs were refused `needs cpu 8+8 > 8`.
    def _dead_at_import(self, unit_state, scoped=True, script=None, close_side_effect=None,
                        armed=False):
        self.kern_plan()
        if armed:
            # Review Z-A R2 M2: the lanes armed -- the admission takes the lease.
            db = store.open()
            self.assertEqual(0, schedule.lease_init(db, lanes="lane-l",
                                                    lanes_file=str(self.lanes_file)).code)
            db.close()
        (self.repo / "scripts" / "pipeline.py").write_text(script or DIES_AT_IMPORT)
        if scoped:
            # I1: only a run started under a scope has a unit to ask.
            os.environ.pop("MECLAW_PIPELINE_NO_SCOPE", None)
            (self.bin / "systemd-run").write_text(PASS_SYSTEMD_RUN)
            (self.bin / "systemd-run").chmod(0o755)
        if unit_state is not None:
            (self.bin / "is-active").write_text(unit_state + "\n")
        with mock.patch.object(gate.schedule, "close", wraps=schedule.close,
                               side_effect=close_side_effect) as close:
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(rc, 2, out + err)
        if script is None:
            self.assertIn("could not fetch the run", err)
        calls = (self.bin / "systemctl.calls").read_text() if (self.bin / "systemctl.calls").exists() else ""
        if scoped:
            self.assertRegex(calls, r"--user is-active pipe-run-[0-9a-f]+\.scope")
        else:
            self.assertNotIn("is-active", calls)
        db = store.open()
        live = schedule._live_runs(db, int(time.time()))
        # Z17 (OR-DT.Z13.3): the unit's end is a note written after the
        # close went through -- never a second `lease_released`.
        released = [json.loads(r[0]) for r in db.execute(
            "SELECT data FROM journal WHERE event = 'lease_released'").fetchall()]
        if armed:
            self.assertEqual(["release"], [r["verb"] for r in released], released)
        else:
            self.assertEqual([], released)
        noted = [json.loads(r[0]) for r in db.execute(
            "SELECT data FROM journal WHERE event = 'lease_noted'").fetchall()]
        return close, out, err, live, [n for n in noted if n.get("verb") == "unit_gone"]

    def _closes_aborted(self, state, script=None, reason="fetch_failed_unit_gone"):
        close, out, err, live, released = self._dead_at_import(state, script=script)
        self.assertFalse(close.call_args.kwargs.get("lost"), close.call_args)
        self.assertEqual("ABORTED", close.call_args.args[2]["data"]["verdict"])
        self.assertEqual(reason, close.call_args.args[2]["data"]["reason"])
        self.assertRegex(out, r"(?m)^GATE-SUMMARY strand \S+ \d+/\d+ \d+s ABORTED$")
        self.assertIn(reason, err)
        self.assertNotIn("keeps its lease", err)
        self.assertEqual([], live)
        self.assertEqual(1, len(released), released)
        self.assertEqual(("lane-l", reason, state),
                         (released[0]["resource"], released[0]["detail"], released[0]["unit_state"]))

    def test_a_failed_fetch_with_the_unit_inactive_closes_the_lease_aborted(self):
        self._closes_aborted("inactive")

    # Z17 (OR-DT.Z13.4): the fetch went through but the run never ended
    # (OOM, a crash after `ws/`): a gone unit frees the lease at once too.
    def test_a_fetched_run_without_pass_ended_and_its_unit_gone_closes_aborted(self):
        self._closes_aborted("inactive", script=LOST, reason="unit_gone")

    def test_a_fetched_run_without_pass_ended_and_its_unit_active_keeps_the_lease(self):
        close, out, err, live, released = self._dead_at_import("active", script=LOST)
        self.assertTrue(close.call_args.kwargs.get("lost"), close.call_args)
        self.assertEqual(1, len(live), live)
        self.assertEqual([], released)

    # Z17 (OR-DT.Z13.3): a close the store refused writes no note of a release.
    def test_a_refused_close_writes_no_unit_note(self):
        close, out, err, live, released = self._dead_at_import(
            "inactive", close_side_effect=sqlite3.OperationalError("database is locked"))
        self.assertIn("gate: store:", err)
        self.assertEqual([], released)
        # Review Z-A R2 M1: the refused close leaves the run live -- no "free".
        self.assertEqual(1, len(live), live)
        self.assertNotIn("the lease is free", err)
        self.assertIn("the store refused the close, the lease stays until its deadline", err)

    def test_a_lost_unit_note_is_named(self):
        real = gate.events.emit

        def emit(db, event, *a, **kw):
            if event == "lease_noted" and (a[1] if len(a) > 1 else {}).get("verb") == "unit_gone":
                raise sqlite3.OperationalError("database is locked")
            return real(db, event, *a, **kw)

        with mock.patch.object(gate.events, "emit", side_effect=emit), \
                mock.patch.object(gate.store, "WRITE_BACKOFF_S", 0):
            close, out, err, live, released = self._dead_at_import("inactive")
        self.assertIn("gate: store: LOST unit note", err)
        self.assertIn("gate: ABORTED -- the store lost 1 record(s) (unit note)", out)
        self.assertEqual([], released)

    def test_an_armed_lease_is_released_once_and_noted_once_when_the_unit_is_gone(self):
        close, out, err, live, noted = self._dead_at_import("inactive", script=LOST, armed=True)
        self.assertEqual([], live)
        self.assertEqual(1, len(noted), noted)
        self.assertIn("the lease is free", err)

    def test_a_failed_fetch_with_the_unit_failed_closes_the_lease_aborted(self):
        self._closes_aborted("failed")

    def test_a_failed_fetch_with_the_unit_unknown_closes_the_lease_aborted(self):
        self._closes_aborted("unknown")

    def _keeps_lease(self, state, scoped=True):
        close, out, err, live, released = self._dead_at_import(state, scoped)
        self.assertTrue(close.call_args.kwargs.get("lost"), close.call_args)
        self.assertIn("keeps its lease", err)
        self.assertEqual(1, len(live), live)
        self.assertEqual([], released)
        return err

    def test_a_failed_fetch_with_the_unit_active_keeps_the_lease(self):
        self._keeps_lease("active")

    # M3: a unit on its way up, down or through a reload still runs something.
    def test_a_failed_fetch_with_the_unit_activating_keeps_the_lease(self):
        self._keeps_lease("activating")

    def test_a_failed_fetch_with_the_unit_deactivating_keeps_the_lease(self):
        self._keeps_lease("deactivating")

    def test_a_failed_fetch_with_the_unit_reloading_keeps_the_lease(self):
        self._keeps_lease("reloading")

    def test_a_failed_fetch_of_a_run_without_a_scope_keeps_the_lease(self):
        # I1: `is-active` says `inactive` for a unit that never existed --
        # a run started without a scope may still be alive; no probe at all.
        err = self._keeps_lease("inactive", scoped=False)
        self.assertIn("started without a scope", err)

    def test_a_failed_fetch_without_a_probe_answer_keeps_the_lease(self):
        close, out, err, live, released = self._dead_at_import(None)
        self.assertTrue(close.call_args.kwargs.get("lost"), close.call_args)
        self.assertIn("keeps its lease", err)
        self.assertEqual(1, len(live), live)
        self.assertEqual([], released)

    def _raising_run(self, exc, where):
        """A lane run whose `exec_stream` (or whose stream) raises `exc`: the
        lease is closed lost on the way out, the exception goes on."""
        self.kern_plan()

        class _Out:
            def __iter__(self):
                raise exc

            def close(self):
                pass

        fake = types.SimpleNamespace(stdout=_Out(), wait=lambda: 0)
        side = {"side_effect": exc} if where == "exec" else {"return_value": fake}
        with mock.patch.object(gate.lane, "exec_stream", **side), \
                mock.patch.object(gate.schedule, "close", wraps=schedule.close) as close:
            with self.assertRaises(type(exc)):
                self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(close.call_count, 1, close.call_args_list)
        self.assertTrue(close.call_args.kwargs.get("lost"), close.call_args)

    def test_exec_stream_raising_closes_the_lease_lost(self):
        self._raising_run(RuntimeError("boom"), "exec")

    def test_exec_stream_interrupted_closes_the_lease_lost(self):
        self._raising_run(KeyboardInterrupt(), "exec")

    def test_stream_raising_closes_the_lease_lost(self):
        self._raising_run(RuntimeError("boom"), "stream")

    def test_stream_interrupted_closes_the_lease_lost(self):
        self._raising_run(KeyboardInterrupt(), "stream")

    # welle-tb S3 (#1161): a record the store refused ended GREEN with exit 0
    # (run-7b40b01d16cb lost its flake register that way). Fail-closed: a
    # bounded retry, then ABORTED with the lost record named -- never RED.
    # Re-check tb-s3-store m-1: the name said "never red"; the claim is ABORTED, exit 2.
    def test_a_locked_store_ends_the_pass_aborted_never_red(self):
        self.kern_plan()
        self.seed_metadata()
        locked = sqlite3.OperationalError("database is locked")
        with mock.patch.object(gate.schedule, "close", side_effect=locked) as close, \
                mock.patch.object(gate.cache, "record", side_effect=locked) as record, \
                mock.patch.object(gate.cache, "_tools", return_value={}), \
                mock.patch.object(gate.store, "WRITE_BACKOFF_S", 0):
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(rc, 2, out + err)
        self.assertRegex(out, r"(?m)^GATE-SUMMARY strand \S+ \d+/\d+ \d+s ABORTED$")
        self.assertEqual(close.call_count, gate.store.WRITE_ATTEMPTS, close.call_args_list)
        # M-3: the cache record comes last and never after a lost record.
        self.assertEqual(record.call_count, 0)
        self.assertIn("gate: store: LOST close -- StoreWriteError: database is locked", err)
        self.assertIn("gate: ABORTED -- the store lost 1 record(s) (close)", out)
        self.assertIn("run the gate again", out)  # m-2: the advice for a busy store
        self.assertNotIn("Traceback", err)

    def test_a_locked_cache_record_alone_ends_the_pass_aborted(self):
        self.kern_plan()
        self.seed_metadata()
        locked = sqlite3.OperationalError("database is locked")
        with mock.patch.object(gate.cache, "record", side_effect=locked) as record, \
                mock.patch.object(gate.cache, "_tools", return_value={}), \
                mock.patch.object(gate.store, "WRITE_BACKOFF_S", 0):
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(rc, 2, out + err)
        self.assertEqual(record.call_count, gate.store.WRITE_ATTEMPTS)
        self.assertIn("gate: store: LOST cache record", err)
        self.assertRegex(out, r"(?m)^GATE-SUMMARY strand \S+ \d+/\d+ \d+s ABORTED$")
        self.assertEqual("ABORTED", self.journaled()["verdict"])

    # Re-check tb-s3-store m-2: a code error is no busy store -- named by its
    # class, and no "run the gate again" (a second run fails the same way).
    def test_a_code_error_in_a_record_is_named_and_not_retried_by_advice(self):
        self.kern_plan()
        with mock.patch.object(gate.flakes, "ingest_run", side_effect=KeyError("station")), \
                mock.patch.object(gate.store, "WRITE_BACKOFF_S", 0):
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(rc, 2, out + err)
        self.assertIn("gate: store: LOST flake register -- KeyError: 'station'", err)
        self.assertRegex(out, r"gate: ABORTED -- the store lost \d+ record\(s\) \(flake register")
        self.assertNotIn("run the gate again", out)
        self.assertIn("KeyError", out)

    # Re-check tb-s3-store M-2: journal `pass_ended` and `pipeline receipt`
    # stayed GREEN for a pass that ended ABORTED (probe P4); retro counted it.
    def _reads_aborted(self, *argv, target, attr):
        locked = sqlite3.OperationalError("database is locked")
        with mock.patch.object(target, attr, side_effect=locked), \
                mock.patch.object(gate.store, "WRITE_BACKOFF_S", 0):
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", *argv)
        self.assertEqual(rc, 2, out + err)
        doc = self.journaled()
        self.assertEqual("ABORTED", doc["verdict"], doc)
        from scripts.pipeline import retro
        db = store.open()
        got = retro.passes(db, "2000-01-01T00:00:00Z", "2999-01-01T00:00:00Z")
        self.assertEqual(["ABORTED"], [p["verdict"] for p in got], got)
        rows = db.execute("SELECT run, data FROM journal WHERE event='process_finding'").fetchall()
        self.assertEqual(1, len(rows), rows)
        self.assertEqual("store_record_lost", json.loads(rows[0][1])["rule"])
        self.assertEqual(doc["run"], rows[0][0])

    def test_a_lost_flake_record_reads_aborted_in_receipt_and_retro(self):
        self.kern_plan()
        self._reads_aborted("--host", "lane-l", target=gate.flakes, attr="ingest_run")

    def test_a_lost_issue_duty_reads_aborted_in_receipt_and_retro(self):
        self.kern_plan()
        self._reads_aborted("--host", "lane-l", target=gate, attr="_flake_ingest")

    def test_a_lost_record_of_a_local_pass_reads_aborted_in_receipt_and_retro(self):
        self.python_plan()
        self._reads_aborted(target=gate, attr="_flake_ingest")

    # Re-check tb-s3-store2 m-n3: the finding is best effort; with the store
    # still locked it went too, and receipt/retro read the pass GREEN again.
    # The gate's ledger (the archive root, else beside the store) is the
    # fallback every reader of a pass verdict reads.
    def _finding_lost_reads_aborted(self, *argv):
        locked = sqlite3.OperationalError("database is locked")
        real = gate.store.retry_write

        def refuse_finding(fn, *args, **kwargs):
            if len(args) > 1 and args[1] == "process_finding":
                raise gate.store.StoreWriteError("database is locked")
            return real(fn, *args, **kwargs)

        with mock.patch.object(gate.flakes, "ingest_run", side_effect=locked), \
                mock.patch.object(gate.store, "retry_write", side_effect=refuse_finding), \
                mock.patch.object(gate.store, "WRITE_BACKOFF_S", 0):
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test",
                                     "--host", "lane-l", *argv)
        self.assertEqual(rc, 2, out + err)
        self.assertIn("gate: store: LOST finding store_record_lost", err)
        db = store.open()
        self.assertEqual([], db.execute(
            "SELECT seq FROM journal WHERE event='process_finding'").fetchall())
        doc = self.journaled()
        self.assertEqual("ABORTED", doc["verdict"], doc)
        from scripts.pipeline import retro
        got = retro.passes(db, "2000-01-01T00:00:00Z", "2999-01-01T00:00:00Z")
        self.assertEqual(["ABORTED"], [p["verdict"] for p in got], got)
        return doc

    def test_a_lost_finding_falls_back_to_the_ledger_beside_the_store(self):
        self.kern_plan()
        self._finding_lost_reads_aborted()

    def test_a_lost_finding_falls_back_to_the_ledger_in_the_archive(self):
        self.kern_plan()
        archive = self.home / "archive"
        doc = self._finding_lost_reads_aborted("--log-dir", str(archive))
        lines = (archive / "store-lost.jsonl").read_text().splitlines()
        self.assertEqual([doc["run"]], [json.loads(ln)["run"] for ln in lines])

    # Re-check tb-s3-store M-3: a pass that does not count left cache rows;
    # the advised re-run came back CACHED and never healed the register.
    def _no_cache_after(self, *argv, target, attr):
        locked = sqlite3.OperationalError("database is locked")
        with mock.patch.object(target, attr, side_effect=locked), \
                mock.patch.object(gate, "_record") as rec, \
                mock.patch.object(gate.store, "WRITE_BACKOFF_S", 0):
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", *argv)
        self.assertEqual(rc, 2, out + err)
        self.assertEqual(rec.call_count, 0)

    def test_a_lost_flake_register_leaves_no_cache_rows(self):
        self.kern_plan()
        self._no_cache_after("--host", "lane-l", target=gate.flakes, attr="ingest_run")

    def test_a_lost_issue_duty_leaves_no_cache_rows(self):
        self.kern_plan()
        self._no_cache_after("--host", "lane-l", target=gate, attr="_flake_ingest")

    def test_a_lost_flake_record_of_a_local_pass_leaves_no_cache_rows(self):
        self.python_plan()
        self._no_cache_after(target=gate, attr="_flake_ingest")

    # Re-check tb-s3-store m-3: after the first lost stream line every further
    # line cost 3 x busy_timeout again; once the pass is ABORTED, one try each.
    def test_after_the_first_lost_stream_line_each_line_gets_one_try(self):
        self.python_plan()
        texts = []

        def ingest(db, text):
            texts.append(text)
            raise sqlite3.OperationalError("database is locked")

        with mock.patch.object(gate.events, "ingest", side_effect=ingest), \
                mock.patch.object(gate.store, "WRITE_BACKOFF_S", 0):
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test")
        self.assertEqual(rc, 2, out + err)
        lines = list(dict.fromkeys(texts))
        self.assertGreater(len(lines), 1, texts)
        self.assertEqual([gate.store.WRITE_ATTEMPTS] + [1] * (len(lines) - 1),
                         [texts.count(t) for t in lines])

    def _lost_record(self, target, attr):
        self.kern_plan()
        locked = sqlite3.OperationalError("database is locked")
        with mock.patch.object(target, attr, side_effect=locked), \
                mock.patch.object(gate.store, "WRITE_BACKOFF_S", 0), \
                mock.patch.object(gate.schedule, "close", wraps=schedule.close) as close:
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(rc, 2, out + err)
        self.assertRegex(out, r"(?m)^GATE-SUMMARY strand \S+ \d+/\d+ \d+s ABORTED$")
        self.assertIn("gate: store: LOST", err)
        self.assertNotIn("Traceback", err)
        # The store's verdict follows: the close carries ABORTED, not the stream's.
        self.assertEqual(close.call_count, 1, close.call_args_list)
        self.assertEqual("ABORTED", close.call_args.args[2]["data"]["verdict"])
        return err

    def test_a_lost_flake_record_ends_the_pass_aborted(self):
        err = self._lost_record(gate.flakes, "ingest_run")
        self.assertIn("gate: store: LOST flake register", err)

    def test_a_lost_stream_line_ends_the_pass_aborted(self):
        err = self._lost_record(gate.schedule, "ingest")
        self.assertIn("gate: store: LOST stream line", err)

    def test_a_lost_journal_head_aborts_before_anything_runs(self):
        self.kern_plan()
        real = gate.events.emit

        def emit(db, event, *a, **kw):
            if event == "pass_planned":
                raise sqlite3.OperationalError("database is locked")
            return real(db, event, *a, **kw)

        with mock.patch.object(gate.events, "emit", side_effect=emit), \
                mock.patch.object(gate.store, "WRITE_BACKOFF_S", 0), \
                mock.patch.object(gate.lane, "exec_stream") as exec_stream, \
                mock.patch.object(gate.lane, "guard", wraps=gate.lane.guard) as guard, \
                mock.patch.object(gate.lane, "push", wraps=gate.lane.push) as push, \
                mock.patch.object(gate.schedule, "close", wraps=schedule.close) as close:
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(rc, 2, out + err)
        self.assertIn("gate: store: LOST journal pass_planned", err)
        self.assertIn("gate: ABORTED -- the store lost 1 record(s)", out)
        self.assertFalse(exec_stream.called)
        # Re-check tb-s3-store m-5: nothing travels after a lost journal head.
        self.assertFalse(guard.called)
        self.assertFalse(push.called)
        self.assertEqual(close.call_count, 1, close.call_args_list)
        self.assertEqual("ABORTED", close.call_args.args[2]["data"]["verdict"])

    def test_a_lost_flake_issue_duty_ends_the_pass_aborted(self):
        self.kern_plan()
        locked = sqlite3.OperationalError("database is locked")
        with mock.patch.object(gate, "_flake_ingest", side_effect=locked), \
                mock.patch.object(gate.store, "WRITE_BACKOFF_S", 0):
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(rc, 2, out + err)
        self.assertIn("gate: store: LOST flake issue duty", err)
        self.assertRegex(out, r"(?m)^GATE-SUMMARY strand \S+ \d+/\d+ \d+s ABORTED$")

    def test_a_lost_record_of_a_local_pass_ends_aborted(self):
        self.python_plan()
        locked = sqlite3.OperationalError("database is locked")
        with mock.patch.object(gate.events, "ingest", side_effect=locked), \
                mock.patch.object(gate.store, "WRITE_BACKOFF_S", 0):
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test")
        self.assertEqual(rc, 2, out + err)
        self.assertRegex(out, r"(?m)^GATE-SUMMARY strand \S+ \d+/\d+ \d+s ABORTED$")
        self.assertIn("gate: store: LOST stream line", err)

    def test_lost_stream_still_archives_what_came(self):
        self.kern_plan()
        (self.repo / "scripts" / "pipeline.py").write_text(ONE_STATION)
        archive = self.tmp / "archive"
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l",
                                 "--archive", str(archive))
        self.assertEqual(rc, 2, out + err)
        self.assertIn("lost", err)
        got = json.loads((archive / "latest" / "receipt.json").read_text())
        received = set(self.verdicts(out))
        self.assertTrue(received)
        self.assertLessEqual(received, {s["name"] for s in got["stations"]})
        self.assertTrue((archive / "latest" / "logs").is_dir())

    def test_stream_with_abort_evidence_ends_aborted(self):
        self.kern_plan()
        (self.repo / "scripts" / "pipeline.py").write_text(ABORT_EVIDENCE)
        archive = self.tmp / "archive"
        with mock.patch.object(gate.schedule, "close", wraps=schedule.close) as close:
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l",
                                     "--archive", str(archive))
        self.assertEqual(rc, 2, out + err)
        self.assertRegex(out, r"(?m)^GATE-SUMMARY strand \S+ \d+/\d+ \d+s ABORTED$")
        self.assertRegex(out, r"(?m)^GATE-LANE lane-l ")
        self.assertFalse(close.call_args.kwargs.get("lost"), close.call_args)
        self.assertEqual(close.call_args.args[2]["data"]["verdict"], "ABORTED")
        self.assertTrue((archive / "latest" / "receipt.json").is_file())
        gate_dir = self.repo / "target" / "gate" / self.repo.name
        self.assertEqual(json.loads((gate_dir / "last-strand.json").read_text())["verdict"], "ABORTED")

    def test_ctrl_c_asks_the_lane_to_abort_and_reads_on(self):
        self.kern_plan()
        real, calls = gate._Pass.stream, []

        def stream(p, proc, ingest=None):
            calls.append(1)
            if len(calls) == 1:
                raise KeyboardInterrupt
            return real(p, proc, ingest)

        with mock.patch.object(gate._Pass, "stream", autospec=True, side_effect=stream), \
                mock.patch.object(gate.lane, "abort", return_value=0) as abort:
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(rc, 0, out + err)
        run = gate.lane.exec_stream.call_args.args[1]["run"]
        self.assertEqual(abort.call_args.args[2], run)
        self.assertIn("interrupted", err)

    def test_deadline_timer_asks_for_the_abort(self):
        why, fired = [], threading.Event()

        def abort(reason):
            why.append(reason)
            fired.set()

        p = types.SimpleNamespace(err=io.StringIO(),
                                  stream=lambda proc, ingest=None: 0 if fired.wait(kit_time.HANG) else 1)
        self.assertEqual(gate._stream_to_end(p, None, None, abort, "2000-01-01T00:00:00Z"), 0)
        self.assertEqual(why, ["deadline"])

    def test_lane_abort_reaches_the_executor_cli(self):
        ln = lane.Lane(name="lane-x", ssh="local", root=str(self.tmp / "lr"), transport="local")
        self.assertEqual(lane.abort(ln, str(self.repo), "run-0123456789ab", str(self.tmp / "ws")), 1)

    def test_lane_guard_reserves_the_admissions_whole_need_beyond_the_floor(self):
        # Delta A M-neu-1: W2 clears to the lane's floor plus what the guard
        # passes; the executor asks reserve + the live runs' + expected -- so
        # the guard passes that need less the floor, not `expected_g` alone.
        self.kern_plan()
        os.environ["MECLAW_LANE_FLOOR_G"] = "20"
        seen, adms = [], []
        real_admit = gate.schedule.admit

        def admit(*a, **kw):
            adm = real_admit(*a, **kw)
            adms.append(adm)
            return adm

        def guard(ln, name, **kw):
            seen.append(kw.get("expected_g"))
            raise lane.LaneError(2, "stop here")
        with mock.patch.object(gate.schedule, "admit", side_effect=admit), \
                mock.patch.object(gate.lane, "guard", side_effect=guard):
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(rc, 2, out + err)
        need = adms[0].dispatch["budget"]["need_free_g"]
        self.assertGreater(need - 20, adms[0].expected_g)
        self.assertEqual(seen, [int(need - 20)])
        # A floor above the need never passes less than the run's own size.
        self.assertEqual(gate._guard_expected_g(adms[0], {"MECLAW_LANE_FLOOR_G": "999999"}),
                         adms[0].expected_g)

    def test_lane_guards_refuse_before_anything_travels(self):
        # W1 (`lane_sync.sh guard`): a local lane without class egon (5),
        # less than the floor free (2) -- the lease closes ABORTED and the
        # build path stays untouched.
        self.kern_plan()
        self.lane_root.mkdir()
        self.lanes_file.write_text("lane-l  local  %s  threads=4\n" % self.lane_root)
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(rc, 5, out + err)
        self.assertIn("lane lane-l refused the run:\n  refused -- a local lane is class egon", err)
        self.assertFalse((self.lane_root / "tree").exists())
        self.lanes_file.write_text("lane-l  builder@lane-l.invalid\n")
        os.environ["MECLAW_LANE_FLOOR_G"] = "999999"
        # W2: the hygiene runs in front of the floor and refuses first (its
        # exit 6 is a refusal of the lane, never a RED verdict).
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(rc, 2, out + err)
        self.assertIn("ABORTED the run before the gate", err)
        self.assertNotIn("GATE-SUMMARY", out)
        # The hygiene reserves the admission's `expected_g`, set explicitly --
        # never the value the gate's own environment carries.
        os.environ["MECLAW_LANE_EXPECTED_G"] = "77"
        self.addCleanup(os.environ.pop, "MECLAW_LANE_EXPECTED_G", None)
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(rc, 2, out + err)
        self.assertRegex(err, r'"reserved_g": \d+\.0,')
        self.assertNotIn('"reserved_g": 77', err)
        self.assertFalse((self.lane_root / "tree").exists())
        # Without the hygiene the floor itself refuses.
        os.environ["MECLAW_HYGIENE"] = "0"
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-l")
        self.assertEqual(rc, 2, out + err)
        self.assertIn("floor 999999G: clear old generations (W2)", err)
        self.assertFalse((self.lane_root / "tree").exists())

    def test_queued_exits_3(self):
        self.lanes_file.write_text("lane-a  builder@lane-a.invalid\n")
        self.kern_plan()
        rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-a")
        self.assertEqual(rc, 3, out + err)
        self.assertNotIn("GATE-SUMMARY", out)

    def test_wait_admission_ends_on_a_lane_that_is_off(self):
        # Review Z-A2 I1: `_sync` runs once per gate, so no round of the wait
        # turns `off` to `idle` -- the wait ends at once with exit 3.
        self.lanes_file.write_text("lane-a  builder@lane-a.invalid\n")
        self.kern_plan()
        got = []
        th = threading.Thread(target=lambda: got.append(self.gate(
            "strand", "--no-pre", "--reason", "test", "--host", "lane-a", "--wait-admission")),
            daemon=True)
        th.start()
        th.join(kit_time.HANG)   # TV-Z3: the hang marker, never a literal (re-check M7d M3)
        self.assertFalse(th.is_alive(), "gate --wait-admission waits on a lane that is off")
        rc, out, err = got[0]
        self.assertEqual(rc, 3, out + err)
        self.assertIn("lane-a is off", err)

    def test_a_store_error_at_the_admission_is_its_own_exit(self):
        # Review Z-A2 R2 M2: a locked store was a traceback with exit 1, RED's code.
        self.lanes_file.write_text("lane-a  builder@lane-a.invalid\n")
        self.kern_plan()
        with mock.patch.object(schedule, "admit",
                               side_effect=sqlite3.OperationalError("database is locked")):
            rc, out, err = self.gate("strand", "--no-pre", "--reason", "test", "--host", "lane-a")
        self.assertEqual(rc, gate.STORE_EXIT, out + err)
        self.assertEqual(6, gate.STORE_EXIT)
        self.assertIn("gate: store: database is locked -- no admission", err)


# The tier on the lane: records its words and its environment, prints a canned
# nextest log, exits with `FAKE_TIER_RC`.
FAKE_TIER = """#!/bin/sh
printf '%s\\n' "$*" >"$FAKE_TIER_RECORD"
printf 'lane=%s threads=%s target=%s\\n' "$MECLAW_TIER_LANE" "$NEXTEST_TEST_THREADS" \\
    "$CARGO_TARGET_DIR" >>"$FAKE_TIER_RECORD"
cat "$FAKE_TIER_LOG"
exit "${FAKE_TIER_RC:-0}"
"""

GREEN_LOG = """    Starting 3 tests across 1 binary
        PASS [   0.010s] meclaw-cells gh123::a
------------
     Summary [   0.030s] 3 tests run: 3 passed, 0 skipped
"""

STRESS_LOG = """  Stress test [   1.0s] iteration 1/3: 4 tests run: 4 passed, 0 skipped
        FAIL [   0.1s] [2/3] (3/4) meclaw-cells gh123::flaky
  Stress test [   1.0s] iteration 2/3: 4 tests run: 3 passed, 1 failed, 0 skipped
  Stress test [   1.0s] iteration 3/3: 4 tests run: 4 passed, 0 skipped
     Summary [   3.0s] 3/3 stress run iterations: 2 passed, 1 failed
"""


class TestTestCmd(GateCase):
    """`pipeline.py test` -- the successor of `strand.sh test` (`cmd_test_legacy`)."""

    def setUp(self):
        super().setUp()
        tier = self.repo / "scripts" / "test-tier.sh"
        tier.write_text(FAKE_TIER)
        tier.chmod(0o755)
        bericht = self.repo / "plans" / "welle-x-2026-10-08" / "berichte" / "t1.md"
        bericht.parent.mkdir(parents=True)
        bericht.write_text("t1\n")
        git(self.repo, "add", ".")
        git(self.repo, "commit", "-q", "-m", "tier")
        self.record = self.tmp / "tier-record"
        self.tier_log = self.tmp / "tier-log"
        self.tier_log.write_text(GREEN_LOG)
        os.environ.update({"FAKE_TIER_RECORD": str(self.record),
                           "FAKE_TIER_LOG": str(self.tier_log), "FAKE_TIER_RC": "0"})
        os.environ.pop("MECLAW_STRAND_TOKEN_SKIP", None)

    def hold(self, lane_name="lane-l"):
        db = store.open()
        self.assertEqual(schedule.lease_init(db, lanes=lane_name,
                                             lanes_file=str(self.lanes_file)).code, 0)
        self.assertEqual(schedule.lease_take(db, "welle-x/t1", lane=lane_name).code, 0)

    def run_test(self, *argv):
        out, err = io.StringIO(), io.StringIO()
        rc = gate.run_test(list(argv), out, err)
        return rc, out.getvalue(), err.getvalue()

    def test_test_cmd_prints_test_line_byte_compatible(self):
        self.hold()
        rc, out, err = self.run_test("binary(~gh123)")
        self.assertEqual(rc, 0, out + err)
        self.assertRegex(out, r"(?m)^TEST \[binary\(~gh123\)\] 3/3 \d+s GREEN$")
        self.assertRegex(out, r"(?m)^strand: lane lane-l \(local\) rev [0-9a-f]{8} overlay \S+$")
        words, env = self.record.read_text().splitlines()
        self.assertEqual(words, "filter binary(~gh123) --no-fail-fast")
        self.assertEqual(env, "lane=1 threads=4 target=%s/target" % self.lane_root)
        logs = list((self.repo / "plans" / "welle-x-2026-10-08" / "receipts" / "t1")
                    .glob("test-*/test.log"))
        self.assertEqual(len(logs), 1, logs)
        self.assertRegex(logs[0].read_text(), r"TEST \[binary\(~gh123\)\] 3/3 \d+s GREEN\n$")

    def test_test_cmd_names_a_browsers_opt_out_in_its_verdict(self):
        """Re-check tv-bn M7c M1: the opt-out (MECLAW_REQUIRE_BROWSERS other
        than 1) stood only as a NOTE in the raw log; a browser driver without
        an engine SKIPs green, and the verdict line read like a full proof.
        The TEST line -- on the console and closing the log -- names it."""
        self.hold()
        with mock.patch.dict(os.environ, {"MECLAW_REQUIRE_BROWSERS": "0"}):
            rc, out, err = self.run_test("binary(~gh123)")
        self.assertEqual(rc, 0, out + err)
        line = r"TEST \[binary\(~gh123\)\] 3/3 \d+s GREEN \(browsers opt-out: MECLAW_REQUIRE_BROWSERS='0'\)"
        self.assertRegex(out, r"(?m)^%s$" % line)
        logs = list((self.repo / "plans" / "welle-x-2026-10-08" / "receipts" / "t1")
                    .glob("test-*/test.log"))
        self.assertRegex(logs[0].read_text(), r"%s\n$" % line)
        with mock.patch.dict(os.environ, {}):
            os.environ.pop("MECLAW_REQUIRE_BROWSERS", None)
            rc, out, err = self.run_test("binary(~gh123)")
        self.assertEqual(rc, 0, out + err)
        self.assertNotIn("opt-out", out)

    def test_test_cmd_names_its_own_log(self):
        """Kit find tv-s1-legacy-a2 (Welle Testing, fix6): the caller had no way to
        tell its run's log -- flake_triage counted the 20-line tail of a red run
        and guessed the newest log of the strand for a green one (a parallel or
        earlier run). The verb names the log of THIS run before it starts."""
        self.hold()
        rc, out, err = self.run_test("binary(~gh123)")
        self.assertEqual(rc, 0, out + err)
        logs = list((self.repo / "plans" / "welle-x-2026-10-08" / "receipts" / "t1")
                    .glob("test-*/test.log"))
        self.assertEqual(len(logs), 1, logs)
        self.assertIn("strand: test log %s\n" % logs[0], out)

    def test_test_cmd_runs_under_the_second_lease_of_holder(self):
        """Kit find (j) (tv-s1-rust D2, Welle Testing): `pre`/`gate`/`token` take
        `--holder <n>` (GH #1146), `test` did not -- `--holder sat` went to
        nextest and `sat` became the filterset. The test runs on the lane of
        the lease `<strand>:<n>`, never on the strand's first one."""
        db = store.open()
        self.assertEqual(schedule.lease_init(db, lanes="lane-l",
                                             lanes_file=str(self.lanes_file)).code, 0)
        self.assertEqual(schedule.lease_take(db, "welle-x/t1:sat", lane="lane-l").code, 0)
        rc, out, err = self.run_test("binary(~gh123)", "--holder", "sat")
        self.assertEqual(rc, 0, out + err)
        self.assertRegex(out, r"(?m)^strand: lane lane-l \(local\)")
        words, _env = self.record.read_text().splitlines()
        self.assertEqual(words, "filter binary(~gh123) --no-fail-fast")
        # without --holder the strand's own (first) lease is asked -- it holds none
        rc, out, err = self.run_test("binary(~gh123)")
        self.assertEqual(rc, 3, out + err)

    def test_test_cmd_refuses_the_old_strand_colon_form(self):
        """Kit find tv-br M5 (Welle Testing, M7d/M7d2): `--strand tv-br:m2` --
        the name of the second lease -- went whole into the push refspec
        (`<sha>:refs/heads/lane/tv-br:m2`), whose colon splits src:dst, and
        the push failed. The second lease is `--holder <n>` (GH #1146); the
        old form is refused with the new one in the message, before a lane
        is asked or anything travels -- never read as a holder in silence."""
        db = store.open()
        self.assertEqual(schedule.lease_init(db, lanes="lane-l",
                                             lanes_file=str(self.lanes_file)).code, 0)
        self.assertEqual(schedule.lease_take(db, "welle-x/t1:m2", lane="lane-l").code, 0)
        for argv in (("--strand", "t1:m2"), ("--strand", "t1:m2", "--holder", "m2")):
            with self.subTest(argv=argv):
                rc, out, err = self.run_test("binary(~gh123)", *argv)
                self.assertEqual(rc, 2, out + err)
                self.assertIn("--strand t1 --holder m2", err)
                self.assertFalse(self.record.exists())
        refs = subprocess.run(["git", "--git-dir", str(self.lane_root / "meclaw.git"), "for-each-ref",
                               "--format=%(refname)", "refs/heads/lane/"], capture_output=True,
                              text=True).stdout.split()
        self.assertEqual([], refs)

    def test_a_name_git_cannot_take_never_reaches_a_lane(self):
        """The class of the tv-br M5 find: every place a strand name becomes a
        ref, a path or a unit refuses what a ref cannot carry (`:` `~` `^`
        `?` `*` `[` `\\`, `@{`, a leading `-`, a `.lock` end), before ssh."""
        ln = lane.find_lane("lane-l")
        for bad in ("t1:m2", "t1~1", "t1^", "a?b", "a*b", "a[b", "a\\b", "a@{1}", "-x", "t1.lock"):
            with self.subTest(bad=bad):
                with self.assertRaisesRegex(lane.LaneError, "not a tree name"):
                    lane.push(ln, str(self.repo), bad, True)
                res = subprocess.run([lane.LANE_SYNC, "push", ln.ssh, bad], cwd=self.repo,
                                     env=lane.sync_env(ln), capture_output=True, text=True,
                                     timeout=kit_time.HANG)
                self.assertEqual(2, res.returncode, res.stdout + res.stderr)
                self.assertIn("not a strand name", res.stderr)

    def test_test_cmd_refuses_a_holder_that_is_no_suffix(self):
        for bad in (("--holder", "a/b"), ("--holder",)):
            with self.subTest(argv=bad):
                rc, out, err = self.run_test("binary(~gh123)", *bad)
                self.assertEqual(rc, 2, out + err)
                self.assertIn("--holder", err)
                self.assertFalse(self.record.exists())

    def test_test_cmd_without_held_lane_exits_3(self):
        rc, out, err = self.run_test("binary(~gh123)")
        self.assertEqual(rc, 3, out + err)
        self.assertIn("test runs on a lane", err)
        self.assertFalse(self.record.exists())

    def test_test_cmd_host_without_token_exits_3_unless_skip(self):
        rc, out, err = self.run_test("binary(~gh123)", "--host", "lane-l")
        self.assertEqual(rc, 3, out + err)
        self.assertIn("binds to a held token", err)
        os.environ["MECLAW_STRAND_TOKEN_SKIP"] = "test"
        rc, out, err = self.run_test("binary(~gh123)", "--host", "lane-l", "--", "--nocapture")
        self.assertEqual(rc, 0, out + err)
        self.assertEqual(self.record.read_text().splitlines()[0],
                         "filter binary(~gh123) --nocapture --no-fail-fast")

    def test_test_cmd_on_a_local_lane_without_class_egon_is_refused(self):
        self.lanes_file.write_text("lane-l  local  %s  threads=4\n" % self.lane_root)
        self.hold()
        rc, out, err = self.run_test("binary(~gh123)")
        self.assertEqual(rc, 2, out + err)
        self.assertIn("lane lane-l refused the test:\n  refused -- a local lane is class egon", err)
        self.assertFalse(self.record.exists())
        self.assertNotIn("TEST [", out)

    def test_test_cmd_waits_for_no_token_skip_on_a_held_build_path(self):
        # W1 hand-over: a skip of the token does not bypass the lane-side
        # flock of the build path -- the push would rewrite a running build.
        os.environ["MECLAW_STRAND_TOKEN_SKIP"] = "parallel"
        ln = lane.find_lane("lane-l")
        with lane.hold(ln, str(self.lane_root / "tree")):
            rc, out, err = self.run_test("binary(~gh123)", "--host", "lane-l")
        self.assertEqual(rc, 2, out + err)
        self.assertIn("is in use", err)
        self.assertFalse(self.record.exists())
        rc, out, err = self.run_test("binary(~gh123)", "--host", "lane-l")
        self.assertEqual(rc, 0, out + err)

    def test_test_cmd_with_one_package_marks_the_build_partial(self):
        self.hold()
        rc, out, err = self.run_test("binary(~gh123)", "-p", "meclaw-cells")
        self.assertEqual(rc, 0, out + err)
        self.assertTrue((self.lane_root / "target" / ".gate-partial").is_file())
        rc, out, err = self.run_test("binary(~gh123)")
        self.assertEqual(rc, 0, out + err)
        self.assertFalse((self.lane_root / "target" / ".gate-partial").exists())

    def test_test_cmd_lost_ssh_exits_2(self):
        self.hold()
        self.tier_log.write_text("    Starting 3 tests across 1 binary\n")
        os.environ["FAKE_TIER_RC"] = "255"
        rc, out, err = self.run_test("binary(~gh123)")
        self.assertEqual(rc, 2, out + err)
        self.assertIn("lost during the test", err)
        self.assertNotIn("TEST [", out)

    def test_test_cmd_refused_exits_6(self):
        self.hold()
        self.tier_log.write_text("error: no tests matched the filterset\n")
        os.environ["FAKE_TIER_RC"] = "4"
        rc, out, err = self.run_test("binary(~nothing)")
        self.assertEqual(rc, 6, out + err)
        self.assertRegex(out, r"(?m)^TEST \[binary\(~nothing\)\] 0/0 \d+s RED \(no tests matched\)$")

    def test_test_cmd_stress_series_is_graded_by_its_iterations(self):
        self.hold()
        self.tier_log.write_text(STRESS_LOG)
        rc, out, err = self.run_test("binary(~gh123)", "--stress-count", "3")
        self.assertEqual(rc, 100, out + err)
        self.assertRegex(out, r"(?m)^TEST \[binary\(~gh123\)\] 11/12 \d+s RED \(1/3 iterations red\)$")
        self.assertIn("  FAIL meclaw-cells gh123::flaky", out.splitlines())


class TestMetadataKey(unittest.TestCase):
    """Delta B I-neu1: the shared `cargo metadata` cache is keyed by every
    manifest and every auto-discovered cargo target -- a new tests/*.rs is a
    new key, so the plan never drops its binary as unknown to a stale file."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.repo = pathlib.Path(self._tmp.name)
        (self.repo / "Cargo.toml").write_text('[workspace]\nmembers = ["crates/c"]\n')
        (self.repo / "Cargo.lock").write_text("version = 3\n")
        (self.repo / "crates" / "c" / "src").mkdir(parents=True)
        (self.repo / "crates" / "c" / "Cargo.toml").write_text('[package]\nname = "c"\n')
        (self.repo / "crates" / "c" / "src" / "lib.rs").write_text("\n")
        (self.repo / "crates" / "c" / "tests").mkdir()
        (self.repo / "crates" / "c" / "tests" / "old.rs").write_text("\n")

    def key(self):
        return gate.metadata_path(str(self.repo))

    def test_a_new_test_target_is_a_new_key(self):
        before = self.key()
        (self.repo / "crates" / "c" / "tests" / "new_one.rs").write_text("\n")
        self.assertNotEqual(before, self.key())

    def test_every_discovered_target_kind_moves_the_key(self):
        c = self.repo / "crates" / "c"
        for rel in ("tests/dir_test/main.rs", "src/bin/tool.rs", "src/bin/two/main.rs",
                    "examples/ex.rs", "benches/b.rs"):
            before = self.key()
            path = c / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("\n")
            self.assertNotEqual(before, self.key(), rel)

    def test_a_member_manifest_moves_the_key(self):
        before = self.key()
        (self.repo / "crates" / "c" / "Cargo.toml").write_text(
            '[package]\nname = "c"\n[[test]]\nname = "x"\npath = "t/x.rs"\n')
        self.assertNotEqual(before, self.key())

    def test_a_source_edit_keeps_the_key(self):
        before = self.key()
        (self.repo / "crates" / "c" / "src" / "lib.rs").write_text("pub fn f() {}\n")
        (self.repo / "crates" / "c" / "tests" / "old.rs").write_text("#[test] fn t() {}\n")
        self.assertEqual(before, self.key())

    def test_fetched_metadata_lands_under_the_plan_time_key(self):
        # Z1: the key is computed when the plan is made; a tests/*.rs that
        # appears while the run is on the lane must not move the fetched file
        # to a key the plan never looked up (the new binary would fall out).
        planned = self.key()
        (self.repo / "crates" / "c" / "tests" / "during_run.rs").write_text("\n")

        def fake_rsync(ln, argv):
            pathlib.Path(argv[-1]).write_text('{"packages": []}')
            return subprocess.CompletedProcess(argv, 0, "", "")

        ln = lane.Lane(name="x", ssh="local", root=str(self.repo), transport="local")
        with mock.patch.object(gate.lane, "_rsync", fake_rsync), \
                mock.patch.dict(os.environ, {"XDG_STATE_HOME": str(self.repo / "state")}):
            planned = self.key()
            (self.repo / "crates" / "c" / "tests" / "during_run2.rs").write_text("\n")
            gate._fetch_metadata(ln, "/ws", planned)
            self.assertTrue(os.path.exists(planned))
            self.assertFalse(os.path.exists(self.key()))

    def test_two_fetches_of_one_key_never_share_a_temp_file(self):
        # Review Zb R1 M3: runs with the same key (other worktrees too) wrote
        # one `.fetch` file; a lost race threw out of the finished run.
        seen = []

        def fake_rsync(ln, argv):
            seen.append(argv[-1])
            if len(seen) == 1:  # a second run takes the file away meanwhile
                return subprocess.CompletedProcess(argv, 1, "", "")
            pathlib.Path(argv[-1]).write_text('{"packages": []}')
            return subprocess.CompletedProcess(argv, 0, "", "")

        ln = lane.Lane(name="x", ssh="local", root=str(self.repo), transport="local")
        with mock.patch.object(gate.lane, "_rsync", fake_rsync), \
                mock.patch.dict(os.environ, {"XDG_STATE_HOME": str(self.repo / "state")}):
            planned = self.key()
            gate._fetch_metadata(ln, "/ws", planned)
            gate._fetch_metadata(ln, "/ws", planned)
            with mock.patch.object(gate.os, "replace", side_effect=FileNotFoundError):
                gate._fetch_metadata(ln, "/ws", planned)  # never raises
        self.assertNotEqual(seen[0], planned + ".fetch")
        self.assertTrue(os.path.exists(planned))

    def test_without_metadata_a_named_binary_is_built_never_dropped(self):
        parts, build = gate.plan.select_parts({"binaries": ["c::new_one"]}, None)
        self.assertEqual(build["tests"], [["c", "new_one"]])


class TestLocalDiskFloor(unittest.TestCase):
    """Z3: the local floor of 60 G is the entry check; the watcher during the
    run holds the lanes' running floor, so a cold build that starts with
    61-75 G free and eats its expected size is no ABORTED pass."""

    def test_the_watcher_holds_less_than_the_entry_floor(self):
        with mock.patch.dict(os.environ, {}, clear=False):
            os.environ.pop("MECLAW_GATE_MIN_FREE_G", None)
            os.environ.pop("MECLAW_GATE_WATCH_FREE_G", None)
            b = gate._local_budget([{"name": "build"}])
        self.assertEqual(b["need_free_g"], 60.0)
        self.assertEqual(b["free_min_g"], float(schedule.DEFAULT_BUDGET["free_min_g"]))
        self.assertLess(b["free_min_g"], b["need_free_g"])

    def test_the_watch_floor_follows_its_own_variable_never_above_entry(self):
        with mock.patch.dict(os.environ, {"MECLAW_GATE_MIN_FREE_G": "20",
                                          "MECLAW_GATE_WATCH_FREE_G": "45"}):
            b = gate._local_budget([{"name": "build"}])
        self.assertEqual(b["free_min_g"], 20.0)


    def test_the_watch_floor_cannot_be_switched_off(self):
        # Review Zb M2: 0, a negative value or nan silenced the watcher.
        for raw in ("0", "-5", "nan", "inf"):
            with self.subTest(raw=raw), mock.patch.dict(
                    os.environ, {"MECLAW_GATE_WATCH_FREE_G": raw}):
                os.environ.pop("MECLAW_GATE_MIN_FREE_G", None)
                b = gate._local_budget([{"name": "build"}])
            self.assertGreaterEqual(b["free_min_g"], gate.WATCH_MIN_G)
            self.assertLessEqual(b["free_min_g"], b["need_free_g"])

    def test_a_negative_entry_floor_is_rejected(self):
        # GH #1154 item 5: MECLAW_GATE_MIN_FREE_G=-5 left the floor negative
        # and the watcher at min(10, -5) -- the disk guard silently off. A
        # negative floor is no value: the default holds, like for nan.
        with mock.patch.dict(os.environ, {"MECLAW_GATE_MIN_FREE_G": "-5"}):
            os.environ.pop("MECLAW_GATE_WATCH_FREE_G", None)
            b = gate._local_budget([{"name": "build"}])
        self.assertEqual(b["need_free_g"], 60.0)
        self.assertEqual(b["reserve_g"], 60.0)
        self.assertGreaterEqual(b["free_min_g"], gate.WATCH_MIN_G)

    def test_a_rejected_or_off_entry_floor_says_so(self):
        # GH #1154 re-check m-2: a value that is dropped or that switches the
        # guard off leaves a NOTE line; a valid floor and a plan without cargo
        # leave none.
        cases = {"-5": "MECLAW_GATE_MIN_FREE_G=-5 ignored, floor 60G",
                 "nan": "MECLAW_GATE_MIN_FREE_G=nan ignored, floor 60G",
                 "0": "MECLAW_GATE_MIN_FREE_G=0: disk guard off"}
        for raw, want in cases.items():
            with self.subTest(raw=raw), mock.patch.dict(
                    os.environ, {"MECLAW_GATE_MIN_FREE_G": raw}):
                self.assertEqual(gate._floor_notes([{"name": "build"}]), [want])
        with mock.patch.dict(os.environ, {"MECLAW_GATE_MIN_FREE_G": "60"}):
            self.assertEqual(gate._floor_notes([{"name": "build"}]), [])
        with mock.patch.dict(os.environ, {"MECLAW_GATE_MIN_FREE_G": "-5"}):
            self.assertEqual(gate._floor_notes([]), [])


class TestArchiveReasonPath(unittest.TestCase):
    """Z10: GATE-PLAN's `reason=` names the archive before the run; when the
    name collides (a second pass in the same second on the same rev) the
    archive lands in `<rid>-N` -- the reason must name that very directory."""

    def test_reason_names_the_directory_the_archive_lands_in(self):
        with tempfile.TemporaryDirectory() as tmp:
            o = types.SimpleNamespace(archive=tmp, mode="strand")
            doc = {"rev": "abcdef1234", "stations": []}
            p = gate._Pass(o, tmp, doc, io.StringIO(), io.StringIO())
            os.makedirs(os.path.join(tmp, p.rid))  # the collision
            reason = p.plan_path()
            p.archive({"verdict": "GREEN"}, os.path.join(tmp, "nodest"), ["GATE-SUMMARY x"])
            self.assertTrue(os.path.isfile(os.path.join(os.path.dirname(reason), "plan.json")), reason)
            self.assertTrue(os.path.isfile(os.path.join(os.path.dirname(reason), "receipt.json")))


class TestArchiveNotes(unittest.TestCase):
    """Review Z-I1: the narrowing's NOTE stands in summary.txt too, not only on stdout."""

    def test_the_plan_notes_close_the_summary(self):
        with tempfile.TemporaryDirectory() as tmp:
            o = types.SimpleNamespace(archive=tmp, mode="strand")
            doc = {"rev": "abcdef1234", "stations": [],
                   "notes": ["narrowing: off (MECLAW_GATE_NARROW=0) -- resolver floor"]}
            p = gate._Pass(o, tmp, doc, io.StringIO(), io.StringIO())
            p.archive({"verdict": "GREEN"}, os.path.join(tmp, "nodest"),
                      ["GATE-SUMMARY strand abcdef1 1/1 1s GREEN"])
            with open(os.path.join(tmp, "latest", "summary.txt"), encoding="utf-8") as fh:
                lines = fh.read().splitlines()
            self.assertEqual(["GATE-SUMMARY strand abcdef1 1/1 1s GREEN",
                              "gate: NOTE narrowing: off (MECLAW_GATE_NARROW=0) "
                              "-- resolver floor"], lines)


    def test_the_scope_line_opens_the_notes_of_the_summary(self):
        """re-check m-N5: `gate: SCOPE` stands in summary.txt, before the NOTEs."""
        with tempfile.TemporaryDirectory() as tmp:
            o = types.SimpleNamespace(archive=tmp, mode="strand")
            doc = {"rev": "abcdef1234", "stations": [], "notes": ["narrowing: integration x"],
                   "narrowing": {"tests": "full-suite", "stage": "integration", "since": None,
                                 "wave": "welle-x", "map": None, "why": "narrowing: x"}}
            p = gate._Pass(o, tmp, doc, io.StringIO(), io.StringIO())
            p.archive({"verdict": "GREEN"}, os.path.join(tmp, "nodest"),
                      ["GATE-SUMMARY strand abcdef1 1/1 1s GREEN"])
            with open(os.path.join(tmp, "latest", "summary.txt"), encoding="utf-8") as fh:
                lines = fh.read().splitlines()
            self.assertEqual(["GATE-SUMMARY strand abcdef1 1/1 1s GREEN",
                              "gate: SCOPE tests=full-suite stage=integration since=- "
                              "wave=welle-x map=- why=narrowing: x",
                              "gate: NOTE narrowing: integration x"], lines)


class TestModeFlag(unittest.TestCase):
    def test_gate_accepts_the_mode_flag_of_the_wrapper(self):
        import io
        o = gate.parse(["--mode", "strand", "--plan-only"], io.StringIO())
        self.assertEqual((o.mode, o.plan_only), ("strand", True))
        self.assertEqual(gate.parse(["--mode"], io.StringIO()), 2)

    def test_gate_accepts_a_holder_suffix(self):
        # GH #1146: a second lease of one strand (`<strand>:<n>`).
        import io
        self.assertEqual("sat", gate.parse(["strand", "--holder", "sat"], io.StringIO()).holder)
        self.assertEqual(gate.parse(["strand", "--holder", "a/b"], io.StringIO()), 2)
        self.assertEqual("welle-x/a:sat", gate.holder_key("/nowhere", "welle-x/a", None, "sat"))

    def test_gate_refuses_the_old_strand_colon_form(self):
        # Kit find tv-br M5 (M7d2): `--strand <s>:<n>` was the second lease by
        # accident (the lease key, the archive `receipts/<s>:<n>`); it is
        # `--strand <s> --holder <n>` and the old form says so.
        import io
        err = io.StringIO()
        self.assertEqual(gate.parse(["strand", "--strand", "a:sat"], err), 2)
        self.assertIn("--strand a --holder sat", err.getvalue())
        self.assertEqual("a", gate.parse(["strand", "--strand", "a"], io.StringIO()).strand)

    def test_gate_accepts_wait_admission(self):
        # GH #1145: block on admission instead of exit 3.
        import io
        self.assertTrue(gate.parse(["strand", "--wait-admission"], io.StringIO()).wait_admission)
        self.assertFalse(gate.parse(["strand"], io.StringIO()).wait_admission)


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()



class TestStationEnvironmentRemote(StationEnvCase):
    def setUp(self):
        super().setUp()
        fake_ssh.install(self.bin, PASS_SSH_TEMPLATE)
        self.lanes_file.write_text("lane-l  builder@lane-l.invalid  threads=4  class=lane\n")
        os.environ["MECLAW_LANE_ROOT"] = str(self.lane_root)
        real = gate.lane.exec_stream

        def exec_stream(ln, dispatch, *a, **kw):
            dispatch = dict(dispatch, budget=dict(dispatch.get("budget") or {},
                                                  need_free_g=0, free_min_g=0, reserve_g=0))
            return real(ln, dispatch, *a, **kw)

        patcher = mock.patch.object(gate.lane, "exec_stream", side_effect=exec_stream)
        patcher.start()
        self.addCleanup(patcher.stop)

    def test_a_host_lane_looks_up_without_the_gates_build_environment(self):
        """On a host the executor runs under `env -i` of REMOTE_BASE: the gate's
        RUSTFLAGS never reaches a station there, so never the lookup either. The
        build part is measured THERE in the executor's command form (orchestrator
        OR-T12-18 A-M1/A-M3: the whole environment, the host's own names included,
        which this side cannot know) -- the fake ssh runs it here under `env -i`."""
        # a cargo configuration over the host's tree (re-check bundle N delta M1, N-K2):
        # the measure there reads it, and its content (mutant MH1, a scan from the
        # fresh directory, compared two empty lists)
        first, second = self.CARGO_CONFIGS
        conf = self.lane_root / ".cargo" / "config.toml"
        conf.parent.mkdir(parents=True, exist_ok=True)
        conf.write_text(first)
        build, dispatch, ln, environ, executor = self.pass_with_rustflags()
        self.assertEqual("ssh", ln.transport)
        self.assertEqual(executor.stations_build(ln, dispatch["+env"], str(self.repo), dispatch["tree"]),
                         build)
        self.assertRegex(build["cargo_config"] or "", r"^[0-9a-f]{16}$")
        conf.write_text(second)
        moved = executor.stations_build(ln, dispatch["+env"], str(self.repo), dispatch["tree"])
        self.assertRegex(moved["cargo_config"] or "", r"^[0-9a-f]{16}$")
        self.assertNotEqual(build["cargo_config"], moved["cargo_config"])
        self.assertEqual(dict(build, cargo_config=None), dict(moved, cargo_config=None))
        self.assertNotIn("RUSTFLAGS", build["build_env"])
        self.assertEqual("rust-lld", build["linker"])
        # the dispatch's words are in it, this process's session is not
        self.assertEqual(digest("strand"), build["build_env"].get("MECLAW_GATE_MODE"))
        for name in [n for n in environ if n not in gate.lane.remote_base() and n not in dispatch["+env"]]:
            self.assertNotIn(name, build["build_env"], name)
        # the host's own names are (the fake host is this machine): its base under `env -i`
        # and the PATH line of the executor's command (`lane.REMOTE_PATH`)
        base = [n for n in gate.lane.remote_base() if n in environ and n not in ("HOME", "PATH")
                and n not in ("XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS")]
        self.assertTrue(base, "no REMOTE_BASE name set in this process")
        for name in base:
            self.assertEqual(digest(environ[name]), build["build_env"].get(name), name)
        self.assertEqual(digest("%s/.cargo/bin:%s" % (environ["HOME"], environ["PATH"])),
                         build["build_env"].get("PATH"))

    def test_plan_only_keys_with_the_runs_build(self):
        """Orchestrator OR-T12-18 A-M3 (re-check tv-bn M7w A-M3): `--plan-only` looked
        up in a third environment (this process's, without the lane's words and the
        mode). Now it measures the run's: the same build part, the same keys.
        Re-check bundle N delta kit-fp M1: the cargo configuration sits at the lane's
        root (`cargo_config_over`), so it keys only where the measure starts from the
        admitted tree (`gate._lane_tree` = `schedule.admission_tree`; mutant MH2, a
        foreign tree, stayed green with the empty list on both sides)."""
        self.plan_only_is_the_run()

    def cargo_config_over(self):
        """On a host the measure starts from the parent of the dispatch's tree, a
        tree under the lane's root -- never `repo`'s parent (this side's tree)."""
        return self.lane_root


if __name__ == "__main__":
    unittest.main()
