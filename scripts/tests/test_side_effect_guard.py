"""The side-effect guard (`scripts/tests/side_effect_guard.py`, wave dev-tempo W7).

Pins that a test module's `setUpModule` -- through `store_isolation.isolate()`
-- leaves no real `systemctl`, `systemd-run`, `ssh`, `poweroff` ... on the
`PATH`, that the fakes fail loudly, that subprocesses inherit them, and that
no test code calls these tools by an absolute path.

Every check first asserts that the name RESOLVES into the guard's directory
and only then executes it, by that resolved path and against a fake target
(`side-effect-guard-probe.*`): while the guard is missing the test is red
without having run anything real.
"""

import os
import pathlib
import re
import shutil
import signal
import subprocess
import sys
import unittest

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO))

from scripts.tests import kit_time, side_effect_guard, store_isolation  # noqa: E402


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()


def tearDownModule():  # noqa: N802 -- unittest's name
    store_isolation.restore()


PROBE_ARGS = {
    "telinit": ["0"],
    # re-check ZC M4: every own process outside the session, runlevel 0,
    # the holders of a file, signals by name
    "killall5": ["-15"],
    "init": ["0"],
    "fuser": ["-k", "side-effect-guard-probe.lock"],
    "skill": ["-TERM", "side-effect-guard-probe"],
    "systemd-inhibit": ["--what=shutdown", "true"],
    "machinectl": ["poweroff", "side-effect-guard-probe"],
    "systemctl": ["--user", "stop", "side-effect-guard-probe.service"],
    "systemd-run": ["--user", "--unit", "side-effect-guard-probe", "/bin/true"],
    "ssh": ["side-effect-guard-probe.invalid", "true"],
    "scp": ["side-effect-guard-probe.txt", "side-effect-guard-probe.invalid:/tmp/"],
    "poweroff": [],
    "reboot": [],
    "shutdown": ["-h", "now"],
    "halt": [],
    "pkill": ["-f", "side-effect-guard-probe"],
    "killall": ["side-effect-guard-probe"],
    "sudo": ["-n", "systemctl", "--user", "stop", "side-effect-guard-probe.service"],
    "doas": ["systemctl", "--user", "stop", "side-effect-guard-probe.service"],
    "pkexec": ["systemctl", "stop", "side-effect-guard-probe.service"],
    "busctl": ["--user", "call", "org.freedesktop.systemd1", "/org/freedesktop/systemd1",
               "org.freedesktop.systemd1.Manager", "StopUnit", "ss",
               "side-effect-guard-probe.service", "replace"],
    # Z7 / W7 re-check: logind, the BMC, signals by pid, the raw bus by CLI
    "loginctl": ["terminate-user", "side-effect-guard-probe"],
    "ipmitool": ["-H", "side-effect-guard-probe.invalid", "chassis", "power", "off"],
    "kill": ["-TERM", "999999999"],
    "dbus-send": ["--session", "--dest=org.freedesktop.systemd1", "/org/freedesktop/systemd1",
                  "org.freedesktop.systemd1.Manager.StopUnit",
                  "string:side-effect-guard-probe.service", "string:replace"],
    "gdbus": ["call", "--session", "--dest", "org.freedesktop.systemd1", "--object-path",
              "/org/freedesktop/systemd1", "--method", "org.freedesktop.systemd1.Manager.StopUnit",
              "side-effect-guard-probe.service", "replace"],
}

# Test code that names one of the tools by an absolute path bypasses PATH.
# Re-check W7 N2: also the pipeline, the shell scripts, the runners, and
# `/usr/local/bin` -- the code the tests drive, not only the tests.
_ABS_TOOL = re.compile(r"/(?:usr/(?:local/)?)?s?bin/(?:%s)\b" % "|".join(
    re.escape(n) for n in side_effect_guard.BLOCKED))
# Z7: the WHOLE scripts/tests tree, not only its top level (e28 incident).
_SCANNED = ("scripts/tests/**/*.py", "scripts/tests/**/*.sh",
            "workshop/evals/*/tests/*.py", "workshop/evals/*/tests/*.sh",
            "scripts/pipeline/*.py", "scripts/pipeline/*.sh", "scripts/*.sh",
            "workshop/evals/*/run_*.py")


class Guard(unittest.TestCase):
    def test_the_guard_installs_when_loaded_by_file_location(self):
        # The scenario suites under workshop/evals/** load this module by file
        # location: no package, `scripts/tests` not on sys.path (`-I` keeps
        # the probe's own directory and PYTHONPATH off it as well).
        probe = ("import importlib.util, os, shutil\n"
                 "spec = importlib.util.spec_from_file_location('side_effect_guard', %r)\n"
                 "mod = importlib.util.module_from_spec(spec)\n"
                 "spec.loader.exec_module(mod)\n"
                 "tmp = mod.install()\n"
                 "try:\n"
                 "    print(all(shutil.which(n) == os.path.join(tmp, n) for n in mod.BLOCKED))\n"
                 "finally:\n"
                 "    mod.remove()\n") % str(REPO / "scripts" / "tests" / "side_effect_guard.py")
        res = subprocess.run([sys.executable, "-I", "-c", probe], cwd="/",
                             capture_output=True, text=True, check=False)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual("True", res.stdout.strip())

    def resolved(self, name):
        """The tool `name` as PATH resolves it -- asserted to be the fake."""
        guard = side_effect_guard.current_dir()
        self.assertIsNotNone(guard, "setUpModule installed no side-effect guard")
        path = shutil.which(name)
        self.assertIsNotNone(path, name)
        self.assertEqual(os.path.dirname(path), guard,
                         f"{name} resolves to {path}, not to the guard's fake")
        return path

    def test_every_blocked_tool_resolves_to_its_fake(self):
        self.assertEqual(set(PROBE_ARGS), set(side_effect_guard.BLOCKED))
        for name in side_effect_guard.BLOCKED:
            self.resolved(name)

    def test_a_fake_fails_loudly_and_acts_on_nothing(self):
        for name, args in PROBE_ARGS.items():
            path = self.resolved(name)
            res = subprocess.run([path] + args, capture_output=True, text=True,
                                 timeout=kit_time.HANG)
            self.assertEqual(side_effect_guard.BLOCKED_RC, res.returncode, name)
            self.assertIn(side_effect_guard.MARK, res.stderr)
            self.assertIn(f"'{name}'", res.stderr)

    def test_a_shell_subprocess_inherits_the_guard(self):
        self.resolved("systemctl")
        self.resolved("ssh")
        res = subprocess.run(
            ["sh", "-c", "systemctl --user is-active side-effect-guard-probe.service;"
                         " echo rc=$?; ssh side-effect-guard-probe.invalid true; echo rc=$?"],
            capture_output=True, text=True, timeout=kit_time.HANG)
        self.assertEqual(["rc=97", "rc=97"], res.stdout.split(), res.stderr)

    def test_install_and_remove_nest_and_restore_path(self):
        before = os.environ["PATH"]
        inner = side_effect_guard.install()
        try:
            self.assertTrue(os.environ["PATH"].startswith(inner + os.pathsep))
            self.assertEqual(os.path.dirname(shutil.which("systemctl")), inner)
        finally:
            side_effect_guard.remove()
        self.assertEqual(before, os.environ["PATH"])
        self.assertFalse(os.path.exists(inner))

    def test_privilege_and_bus_tools_are_blocked(self):
        for name in ("sudo", "doas", "pkexec", "busctl", "loginctl", "ipmitool", "kill",
                     "dbus-send", "gdbus"):
            self.assertIn(name, side_effect_guard.BLOCKED)

    def test_an_env_without_path_still_resolves_to_the_fakes(self):
        """Z7: `env={...}` without PATH made the child fall back to
        /bin:/usr/bin and reach the real binary. `command -v` only names the
        tool the child would run; nothing is executed."""
        guard = side_effect_guard.current_dir()
        self.assertIsNotNone(guard)
        for env in ({}, {"HOME": "/nonexistent"}):
            res = subprocess.run(["sh", "-c", "command -v systemctl; command -v loginctl"],
                                 env=env, capture_output=True, text=True, timeout=kit_time.HANG)
            self.assertEqual([os.path.join(guard, "systemctl"), os.path.join(guard, "loginctl")],
                             res.stdout.split(), (env, res.stderr))

    def test_an_env_with_its_own_path_is_left_alone(self):
        # Built, not a literal: test_store_isolation forbids fixed search paths.
        own = os.pathsep.join([os.path.dirname(shutil.which("sh")), os.path.join(str(REPO), "own")])
        res = subprocess.run(["sh", "-c", 'echo "$PATH"'], env={"PATH": own},
                             capture_output=True, text=True, timeout=kit_time.HANG)
        self.assertEqual(own, res.stdout.strip())

    def test_the_scan_reaches_nested_test_files(self):
        self.assertIn("scripts/tests/**/*.py", _SCANNED)
        modules = {p.relative_to(REPO).as_posix() for p in REPO.glob("scripts/tests/**/test_*.py")}
        self.assertIn("scripts/tests/test_side_effect_guard.py", modules)

    def test_the_absolute_path_pattern_covers_usr_local_bin(self):
        for line in ("/usr/local/bin/systemctl --user stop x",
                     "/usr/bin/sudo -n true", "/bin/busctl --user"):
            self.assertIsNotNone(_ABS_TOOL.search(line), line)

    def test_the_scan_reaches_pipeline_scripts_and_runners(self):
        scanned = {p.relative_to(REPO).as_posix()
                   for pattern in _SCANNED for p in REPO.glob(pattern)}
        for rel in ("scripts/pipeline/pool.py", "scripts/strand.sh", "scripts/gate.sh",
                    "workshop/evals/scenarios/run_scenarios.py",
                    # M0 (#1120): the measurement pipeline and its tests
                    "scripts/pipeline/measure.py", "scripts/tests/test_pipeline_measure.py"):
            self.assertIn(rel, scanned)

    def test_every_test_module_installs_the_guard(self):
        """Every Python test module puts the fakes on PATH before its tests
        run: through `store_isolation.isolate()` (which installs the guard)
        or, for the runner tests outside `scripts/`, the guard directly.
        A new module -- like M0's `test_pipeline_measure` -- that does
        neither fails here, not on a live unit."""
        modules = sorted(p for pattern in ("scripts/tests/**/test_*.py",
                                           "workshop/evals/*/tests/test_*.py")
                         for p in REPO.glob(pattern))
        rels = {p.relative_to(REPO).as_posix() for p in modules}
        self.assertIn("scripts/tests/test_pipeline_measure.py", rels)
        unguarded = []
        for path in modules:
            text = path.read_text(errors="replace")
            if "store_isolation.isolate()" not in text and "side_effect_guard" not in text:
                unguarded.append(path.relative_to(REPO).as_posix())
        self.assertEqual([], unguarded)

    def test_no_test_code_calls_a_blocked_tool_by_absolute_path(self):
        # this test and the guard module name the paths they refuse
        skip = {pathlib.Path(__file__).resolve(),
                pathlib.Path(side_effect_guard.__file__).resolve()}
        hits = []
        for pattern in _SCANNED:
            for path in sorted(REPO.glob(pattern)):
                if path.resolve() in skip:
                    continue
                for no, line in enumerate(path.read_text(errors="replace").splitlines(), 1):
                    if _ABS_TOOL.search(line):
                        hits.append(f"{path.relative_to(REPO)}:{no}: {line.strip()}")
        self.assertEqual([], hits)


# Review ZC I1: the shell builtin `kill` never goes through PATH, so test
# scripts must not use it at all; signals from Python go through the patched
# `os.kill`/`os.killpg` instead.
_KILL_BUILTIN = re.compile(
    r"(?:^|[;&|()`\"'{!]"
    r"|\b(?:then|do|else|command|builtin|if|elif|while|until|eval|time))\s*kill\s+"
    r"(?:-[A-Za-z0-9]+\s+)*[$%\d\"'{`-]")
_SCANNED_TESTS = ("scripts/tests/**/*.py", "scripts/tests/**/*.sh",
                  "workshop/evals/*/tests/*.py", "workshop/evals/*/tests/*.sh")


class Signals(unittest.TestCase):
    def _dead_pid(self):
        child = subprocess.Popen(["true"])
        child.wait(timeout=kit_time.HANG)
        return child.pid

    def _orphan(self):
        """A live pid that is not our descendant (reparented after its shell exits)."""
        out = subprocess.run(["sh", "-c", "sleep 30 >/dev/null 2>&1 & echo $!"],
                             capture_output=True, text=True, timeout=kit_time.HANG, check=True)
        pid = int(out.stdout.strip())
        self.addCleanup(self._reap_orphan, pid)
        return pid

    @staticmethod
    def _reap_orphan(pid):
        try:
            side_effect_guard._OS_KILL(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass

    def test_signal_zero_is_a_pure_probe_and_reaches_the_kernel(self):
        # Fix-Runde gate-selftest: ports._live_claim reads PermissionError on
        # signal 0 as "alive, other user"; the probe must stay truthful.
        with self.assertRaises(ProcessLookupError):
            os.kill(self._dead_pid(), 0)
        os.kill(os.getppid(), 0)
        os.kill(self._orphan(), 0)
        os.kill(0, 0)
        os.killpg(os.getpgrp(), 0)
        with self.assertRaises(ProcessLookupError):
            os.killpg(self._dead_pid(), 0)

    def test_os_kill_on_a_foreign_pid_is_refused(self):
        orphan = self._orphan()
        with self.assertRaises(PermissionError) as cm:
            os.kill(orphan, signal.SIGTERM)
        self.assertIn(side_effect_guard.MARK, str(cm.exception))
        os.kill(orphan, 0)  # still alive: the guard sent nothing

    def test_os_killpg_on_the_runners_own_group_is_refused(self):
        # SIGCONT: harmless should the guard ever regress, still a real signal
        with self.assertRaises(PermissionError) as cm:
            os.killpg(os.getpgrp(), signal.SIGCONT)
        self.assertIn(side_effect_guard.MARK, str(cm.exception))

    def test_os_kill_with_pid_zero_or_minus_one_is_refused(self):
        for pid in (0, -1, -os.getpgrp()):
            with self.assertRaises(PermissionError, msg=pid):
                os.kill(pid, signal.SIGCONT)

    def test_signals_to_own_children_and_their_groups_pass(self):
        child = subprocess.Popen(["sleep", "30"])
        try:
            os.kill(child.pid, signal.SIGTERM)
            self.assertEqual(-signal.SIGTERM, child.wait(timeout=kit_time.HANG))
        finally:
            child.kill()
            child.wait(timeout=kit_time.HANG)
        group = subprocess.Popen(["sleep", "30"], start_new_session=True)
        try:
            os.killpg(os.getpgid(group.pid), signal.SIGTERM)
            self.assertEqual(-signal.SIGTERM, group.wait(timeout=kit_time.HANG))
        finally:
            group.kill()
            group.wait(timeout=kit_time.HANG)

    def test_an_absolute_path_to_a_blocked_tool_hits_the_fake(self):
        res = subprocess.run(["/usr/bin/systemctl", "--version"],
                             capture_output=True, text=True, timeout=kit_time.HANG)
        self.assertEqual(side_effect_guard.BLOCKED_RC, res.returncode)
        self.assertIn(side_effect_guard.MARK, res.stderr)

    def test_the_kill_builtin_pattern(self):
        for bad in ('sh -c "kill -9 $pid"', "kill %1", "true; kill -TERM 42",
                    "if x; then kill $p; fi",
                    # re-check ZC M1: the idiomatic quoted forms and the prefixes
                    'kill "$pid"', 'f"kill {pid}"', '"kill " + str(pid)', "kill `cat f`",
                    "command kill $p", "builtin kill -9 $p",
                    # re-check 8d26c0ec4 M: the builtin in a condition, negated, eval'd, timed
                    'if kill -TERM "$pid"; then', "while kill -0 $p; do", "until kill -0 $p",
                    "elif kill $p; then", "! kill -0 $p", "eval kill $p", "time kill $p",
                    # re-check 2686990c8 M: a case branch
                    '*) kill "$pid" ;;', "stop) kill -TERM $p ;;"):
            self.assertRegex(bad, _KILL_BUILTIN)
        for ok in ("pkill -f x", "killall x", "os.kill(pid, 0)", "kill_switch = 1",
                   "# we never kill the colony", "the iftime kill_switch", "evaluate(kill_switch)"):
            self.assertNotRegex(ok, _KILL_BUILTIN)

    def test_no_test_script_uses_the_kill_builtin(self):
        skip = {pathlib.Path(__file__).resolve()}
        hits = []
        for pattern in _SCANNED_TESTS:
            for path in sorted(REPO.glob(pattern)):
                if path.resolve() in skip:
                    continue
                for no, line in enumerate(path.read_text(errors="replace").splitlines(), 1):
                    if _KILL_BUILTIN.search(line):
                        hits.append(f"{path.relative_to(REPO)}:{no}: {line.strip()}")
        self.assertEqual([], hits)


    def test_no_test_reaches_the_live_colony_port(self):
        """Review Minor R1: HTTP to the live colony (:7824) never comes from a
        test -- a host or address glued to the port is a hit, a bare number
        (a refused port in a range check) is not."""
        for hit in ("http://127.0.0.1:7824/v1", "localhost:7824", "[::1]:7824"):
            self.assertRegex(hit, _LIVE_PORT)
        self.assertNotRegex("for bad in (31050, 7824):", _LIVE_PORT)
        hits = []
        for pattern in _SCANNED_TESTS:
            for path in sorted(REPO.glob(pattern)):
                for no, line in enumerate(path.read_text(errors="replace").splitlines(), 1):
                    if _LIVE_PORT.search(line) and path.resolve() != pathlib.Path(__file__).resolve():
                        hits.append(f"{path.relative_to(REPO)}:{no}: {line.strip()}")
        self.assertEqual([], hits)


_LIVE_PORT = re.compile(r"[\w\]]:7824\b")


if __name__ == "__main__":
    unittest.main()
