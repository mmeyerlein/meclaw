"""Guard: no script test may reach the machine's pipeline store.

Every `test_*.py` next to this file must call `store_isolation.isolate()` in
its `setUpModule` and `store_isolation.restore()` in its `tearDownModule`.
The rule is syntactic on purpose: which module reaches the store through a
subprocess (`strand.sh pre` -> `pipeline pre` -> `pass_ended`) is not visible
from its source, and the leak of wave dev-tempo W11 came from exactly such a
module. A module without the calls fails here, before it can write a single
row.
"""

import ast
import fcntl
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import unittest

from scripts.tests import kit_time, side_effect_guard, store_isolation

HERE = pathlib.Path(__file__).resolve().parent
# A literal search path ("/usr/bin:/bin") instead of one built from PATH.
FIXED_SEARCH_PATH = re.compile(r"(^|:)/(usr/)?(local/)?s?bin(:|$)")


def _calls(func, attr):
    """True when `func`'s body calls `store_isolation.<attr>()`."""
    for node in ast.walk(func):
        if (isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute)
                and node.func.attr == attr
                and isinstance(node.func.value, ast.Name)
                and node.func.value.id == "store_isolation"):
            return True
    return False


def _hooks(path):
    tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    found = {}
    for node in tree.body:
        if isinstance(node, ast.FunctionDef) and node.name in ("setUpModule", "tearDownModule"):
            found[node.name] = node
    return found


def _fixed_search_paths(tree):
    """Line numbers of string literals in `tree` that are a fixed search path."""
    return [node.lineno for node in ast.walk(tree)
            if isinstance(node, ast.Constant) and isinstance(node.value, str)
            and "\n" not in node.value and FIXED_SEARCH_PATH.search(node.value)]


def _test_modules(root=HERE):
    """Every test module under `root`, nested ones included (R1 M4), fixtures not."""
    return sorted(p for p in root.rglob("test_*.py")
                  if "fixtures" not in p.relative_to(root).parts)


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


class StoreIsolationGuard(unittest.TestCase):

    def test_every_test_module_isolates_the_pipeline_store(self):
        missing = []
        for path in _test_modules():
            hooks = _hooks(path)
            setup, teardown = hooks.get("setUpModule"), hooks.get("tearDownModule")
            if setup is None or not _calls(setup, "isolate"):
                missing.append(f"{path.name}: setUpModule without store_isolation.isolate()")
            if teardown is None or not _calls(teardown, "restore"):
                missing.append(f"{path.name}: tearDownModule without store_isolation.restore()")
        self.assertEqual(missing, [], "\n".join(missing))

    def test_isolate_points_away_from_the_default_store(self):
        before = os.environ.get(store_isolation.DB_ENV)
        path = store_isolation.isolate()
        try:
            self.assertNotEqual(path, store_isolation.DEFAULT_STORE)
            self.assertEqual(os.environ[store_isolation.DB_ENV], str(path))
            self.assertEqual(os.environ[store_isolation.NIGHT_ENV], "0")
            self.assertTrue(path.parent.is_dir())
        finally:
            store_isolation.restore()
        self.assertEqual(os.environ.get(store_isolation.DB_ENV), before)
        self.assertFalse(path.parent.exists())

    def test_isolate_drops_the_outbound_metric_and_grafana_targets(self):
        names = ("MECLAW_PIPELINE_METRICS_URL", "MECLAW_GRAFANA_URL", "GRAFANA_ADMIN_PASS")
        before = {n: os.environ.get(n) for n in names}
        for n in names:
            os.environ[n] = "http://example.invalid/" + n
        try:
            store_isolation.isolate()
            try:
                for n in names:
                    self.assertNotIn(n, os.environ, n)
            finally:
                store_isolation.restore()
            for n in names:
                self.assertEqual(os.environ.get(n), "http://example.invalid/" + n)
        finally:
            for n, old in before.items():
                if old is None:
                    os.environ.pop(n, None)
                else:
                    os.environ[n] = old

    def test_isolate_hands_no_lock_of_an_outer_gate_run(self):
        """The self-test station inherits the outer run's lane lock path.

        The engine hands every station `MECLAW_GATE_LOCK` and takes that
        lock before its first cargo station; a self-test started earlier
        whose `gate.sh` subprocess flocks the inherited path waits until the
        run ends, and the run waits for the self-test (W3 gate build02,
        gate-selftest timeout 1200 s after 170 tests).
        """
        names = ("MECLAW_GATE_LOCK", "MECLAW_CARGO_LOCK_HELD", "MECLAW_STAGE_LOCK")
        before = {n: os.environ.get(n) for n in names}
        with tempfile.TemporaryDirectory() as outer:
            outer_lock = os.path.join(outer, "outer-cargo.lock")
            os.environ["MECLAW_GATE_LOCK"] = outer_lock
            os.environ["MECLAW_CARGO_LOCK_HELD"] = "1"
            os.environ["MECLAW_STAGE_LOCK"] = os.path.join(outer, "stage.lock")
            try:
                with open(outer_lock, "a") as held:
                    fcntl.flock(held, fcntl.LOCK_EX)
                    path = store_isolation.isolate()
                    try:
                        self.assertIsNone(os.environ.get("MECLAW_CARGO_LOCK_HELD"))
                        self.assertIsNone(os.environ.get("MECLAW_STAGE_LOCK"))
                        self.assertEqual(pathlib.Path(os.environ["MECLAW_GATE_LOCK"]).parent,
                                         path.parent)
                        probe = subprocess.run(
                            ["flock", "-w", str(int(kit_time.HANG)), os.environ["MECLAW_GATE_LOCK"], "true"],
                            stdin=subprocess.DEVNULL, capture_output=True, timeout=2 * kit_time.HANG)
                        self.assertEqual(probe.returncode, 0, probe.stderr)
                    finally:
                        store_isolation.restore()
                self.assertEqual(os.environ.get("MECLAW_GATE_LOCK"), outer_lock)
                self.assertEqual(os.environ.get("MECLAW_CARGO_LOCK_HELD"), "1")
            finally:
                for n, old in before.items():
                    if old is None:
                        os.environ.pop(n, None)
                    else:
                        os.environ[n] = old

    def test_isolate_puts_fakes_for_systemd_remote_and_power_in_front_of_path(self):
        """The lane tests run the lane side HERE through a fake `ssh`.

        Its hygiene listed the host's real `pipe-run-*` scopes (W3, 123 calls
        of `systemctl --user list-units` in one self-test run) and stops the
        ones its own throw-away store does not know -- on a lane inside a
        gate run, the outer run's own scopes.
        """
        store_isolation.isolate()
        try:
            for name in ("systemctl", "systemd-run", "ssh", "sudo", "poweroff"):
                found = shutil.which(name)
                self.assertIsNotNone(found, name)
                with open(found, "rb") as fh:
                    head = fh.read(4096)
                self.assertTrue(b"is blocked in tests" in head, "%s -> %s" % (name, found))
            probe = subprocess.run([shutil.which("systemctl"), "--user", "stop",
                                    "pipe-run-000000000000.scope"],
                                   stdin=subprocess.DEVNULL, capture_output=True,
                                   timeout=kit_time.HANG)
            self.assertEqual(probe.returncode, 97)
        finally:
            store_isolation.restore()

    def test_systemctl_inside_an_isolated_module_is_the_trap(self):
        """Pin of c268dac6c: from inside an isolated module no test reaches systemd.

        `setUpModule` isolated this module and nothing here installs the trap
        again: the first `systemctl` on PATH is the guard's fake, and a call
        ends with its exit code and its mark instead of listing or stopping
        a unit.
        """
        trap = side_effect_guard.current_dir()
        self.assertIsNotNone(trap, "store_isolation.isolate() installed no trap")
        self.assertEqual(trap, os.path.dirname(shutil.which("systemctl")))
        probe = subprocess.run(["systemctl", "--user", "list-units", "pipe-run-*"],
                               stdin=subprocess.DEVNULL, capture_output=True,
                               text=True, timeout=kit_time.HANG)
        self.assertEqual(side_effect_guard.BLOCKED_RC, probe.returncode)
        self.assertIn("%s: real 'systemctl' is blocked" % side_effect_guard.MARK, probe.stderr)

    def test_no_test_hands_a_subprocess_a_fixed_search_path(self):
        """A literal PATH such as "/usr/bin:/bin" drops the trap's directory.

        The subprocess then finds the machine's `systemctl` again. A test
        that wants a minimal PATH takes `store_isolation.trapped_path()`.
        """
        found = []
        for path in _test_modules():
            tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
            found += ["%s:%d" % (path.name, line) for line in _fixed_search_paths(tree)]
        self.assertEqual([], found, "\n".join(found))

    def test_the_lints_reach_nested_test_modules(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            for rel in ("test_top.py", "sub/test_nested.py", "fixtures/test_data.py"):
                (root / rel).parent.mkdir(parents=True, exist_ok=True)
                (root / rel).write_text("")
            self.assertEqual(["sub/test_nested.py", "test_top.py"],
                             sorted(p.relative_to(root).as_posix() for p in _test_modules(root)))

    def test_the_search_path_check_sees_a_fixed_path(self):
        fixed = os.pathsep.join(os.path.join("/", d, "bin") for d in ("usr", ""))  # no literal
        self.assertEqual([1], _fixed_search_paths(ast.parse("env = {'PATH': %r}" % fixed)))
        self.assertEqual([], _fixed_search_paths(ast.parse("env = {'PATH': trapped_path()}")))
        self.assertEqual([], _fixed_search_paths(ast.parse("x = '#!/usr/bin/env bash'")))
        self.assertTrue(store_isolation.trapped_path().startswith(
            side_effect_guard.current_dir() + os.pathsep))

    def test_the_guard_sees_a_module_without_isolation(self):
        tree = ast.parse("def setUpModule():\n    pass\n")
        self.assertFalse(_calls(tree.body[0], "isolate"))
        tree = ast.parse("def setUpModule():\n    store_isolation.isolate()\n")
        self.assertTrue(_calls(tree.body[0], "isolate"))


class TestNoInheritedRepository(unittest.TestCase):
    """pd-1155 kit find: a suite started with GIT_DIR/GIT_WORK_TREE in its
    environment ran every test repository's `git init`/`config`/`commit`
    against that repository -- core.worktree and user.name landed in the
    shared config of meclaw-core. The package drops them on import."""

    def test_the_package_drops_the_callers_repository(self):
        env = dict(os.environ, GIT_DIR="/elsewhere/.git", GIT_WORK_TREE="/elsewhere",
                   GIT_INDEX_FILE="/elsewhere/.git/index", GIT_COMMON_DIR="/elsewhere/.git")
        code = ("import os, scripts.tests; print(' '.join(os.environ.get(n, 'unset') for n in "
                "('GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE', 'GIT_COMMON_DIR')))")
        res = subprocess.run(["python3", "-c", code], cwd=str(HERE.parent.parent), env=env,
                             capture_output=True, text=True, timeout=2 * kit_time.HANG)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual("unset unset unset unset", res.stdout.strip())

    def test_the_one_list_covers_every_repository_variable_of_git(self):
        # Re-check pd-legacy m-1: one list (gitenv), and a newer git that adds
        # a variable turns this red instead of leaving a copy behind.
        from scripts.pipeline import gitenv
        res = subprocess.run(["git", "rev-parse", "--local-env-vars"], capture_output=True,
                             text=True, check=True, cwd=tempfile.gettempdir())
        self.assertLessEqual(set(res.stdout.split()), set(gitenv.GIT_REPO_VARS))

    def test_a_suite_run_inside_a_foreign_repository_leaves_it_alone(self):
        # Re-check pd-legacy m-5: the leak end to end -- the pre tests that
        # wrote core.worktree, user.name and a commit "first" before pd-1155,
        # started with GIT_DIR/GIT_WORK_TREE on a real throwaway repository.
        env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
        env.update(GIT_AUTHOR_NAME="o", GIT_AUTHOR_EMAIL="o@example.invalid",
                   GIT_COMMITTER_NAME="o", GIT_COMMITTER_EMAIL="o@example.invalid",
                   GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1")
        with tempfile.TemporaryDirectory() as tmp:
            r, wt = os.path.join(tmp, "R"), os.path.join(tmp, "R-wt")

            def git(*args):
                return subprocess.run(["git"] + list(args), check=True, capture_output=True,
                                      text=True, env=env).stdout.strip()

            git("init", "-q", "-b", "master", r)
            git("-C", r, "commit", "-q", "--allow-empty", "-m", "0")
            git("-C", r, "worktree", "add", "-q", "-b", "strand", wt)

            def state():
                return (git("config", "--file", os.path.join(r, ".git", "config"), "--list"),
                        git("-C", r, "rev-parse", "master", "strand"),
                        git("-C", wt, "rev-parse", "HEAD"))

            before = state()
            leak = dict(env, GIT_DIR=git("-C", wt, "rev-parse", "--absolute-git-dir"),
                        GIT_WORK_TREE=wt)
            res = subprocess.run([sys.executable, "-m", "unittest",
                                  "scripts.tests.test_pipeline_pre.LocalHalf"],
                                 cwd=str(HERE.parent.parent), env=leak, capture_output=True,
                                 text=True, timeout=10 * kit_time.HANG)
            self.assertEqual(before, state(), "the suite wrote into the caller's repository")
            self.assertEqual(0, res.returncode, res.stderr[-2000:])


if __name__ == "__main__":
    unittest.main()
