"""The kit's ONE narrow point to a lane (GH #1209, re-check tv-bn M7e part B).

* M1: every ssh way of the kit runs through `pipeline/lane.py ssh_argv` /
  `rsh` (Python) or `lane_sync.sh remote`/`remote_long`/`lane_rsync`
  (`ssh_bin`). Measured before: `flake_triage._shell`, `hygiene.idle_probe`
  and the power-off, `register.KitPush.steps`, `coverage.lane_up` and
  `measure.ssh_runner` built their own ssh argv, `run_guarded` ran class
  egon's command unchecked. `ProductionSweep` is the structure pin over the
  production code: a program literal `ssh`/`scp`/`sftp` (or an `rsync`
  without the narrow point's remote shell) outside the narrow point is RED
  unless its line carries `# lane-engstelle-ausnahme: <reason>`.
* M2: the word check is fail-closed -- no left context, a setting builtin
  counts too -- and the environment is built from data (`env_prefix`,
  `env_word`). The eight forms of the re-check and the twelve quoting
  places are mutants here, on the Bash AND the Python side.
* Registry (orchestrator OR-T10-2, re-check M7h A-M1): ALWAYS, an `ssh` on
  PATH that is neither the system's own nor a fake of `fake_ssh.py` with its
  body hash in `lane.FAKE_SSH_REGISTRY` is refused, with its path.
* The word check and `ProductionSweep` are early warnings (OR-T10-2); the
  guarantees are the program check above and the remote environment built
  from data under `env -i` (`test_fake_ssh`, the probe of setting forms).
"""
import ast
import collections
import importlib
import os
import pathlib
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import unittest
import unittest.mock

from scripts import flake_triage
from scripts.pipeline import coverage, hygiene, lane, measure, register
from scripts.tests import fake_ssh, kit_time, kit_tree, store_isolation

REPO = pathlib.Path(__file__).resolve().parents[2]
MARK = "lane-engstelle-ausnahme:"


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


# ---------------------------------------------------------------------------
# the Bash side: lane_sync.sh's functions without its dispatch
# ---------------------------------------------------------------------------

_TMP = tempfile.TemporaryDirectory(prefix="engstelle-")
_FNS = pathlib.Path(_TMP.name) / "lane_sync.sh"


def _fns():
    """lane_sync.sh's functions in a file beside the kit libraries they source."""
    if not _FNS.exists():
        kit_tree.functions(_TMP.name)
    return str(_FNS)


def bash(snippet, *args, env=None):
    """`snippet` with lane_sync.sh's functions loaded; ($1.. = args)."""
    return subprocess.run(["bash", "-c", 'source "$0"; ' + snippet, _fns()] + list(args),
                          capture_output=True, text=True, env=env, timeout=kit_time.HANG)


def bash_refusals(scripts):
    """remote_words_known's exit per script, one bash for all."""
    res = bash('for s in "$@"; do ( remote_words_known "$s" ) 2>/dev/null; echo $?; done', *scripts)
    return [int(x) for x in res.stdout.split()]


def py_refused(script):
    try:
        lane.remote_words_known(script)
    except lane.LaneError:
        return True
    return False


# The eight forms of re-check M7e B-M2 (both checks passed them).
EIGHT_FORMS = {
    "single quote": "bash -c 'FOO=1 true'",
    "double quote": 'env "FOO=a b" true',
    "append": "FOO+=1 true",
    "ansi-c q of a script": "bash -c $'FOO=1\\ntrue'",
    "backtick": "x=`FOO=1 true`",
    "bound option": "systemd-run --setenv=FOO=1 true",
    "shlex.quote word": "env " + shlex.quote("FOO=a b") + " true",
    "shlex.quote script": "bash -c " + shlex.quote("FOO=1 true"),
}
# Further setting forms without `NAME=` at a word start (M7g, same class).
SETTING_FORMS = {
    "index": "FOO[0]=1",
    "default assign": "echo ${FOO:=1}",
    "export bare": "export FOO",
    "declare -x": "declare -x FOO",
    "read": "read -r FOO <x",
    "printf -v": "printf -v FOO %s 1",
    "for": "for FOO in 1; do :; done",
    "readonly second name": "readonly a FOO",
    "mixed prefix": "xFOO=1 true",
    # M7h: a value is skipped, the names beside it are not
    "export after a value": "export MECLAW_GATE_MODE=x FOO",
    "export after a quoted value": "export MECLAW_GATE_MODE='a b' FOO",
    "export quoted name": "export 'FOO'",
    # M7i (re-check M7h A-M2.1, regression of M7h): a nameref's value is a name
    "declare -n": "declare -n r=FOO; r=1",
    "typeset -n": "typeset -n r=FOO; r=1",
    "local -n": "local -n r=FOO",
    "declare -gn": "declare -gn r=FOO; export r=1",
}
CONTROLS = {"plain": "FOO=1 true", "after ;": "true; FOO=1 true", "after &&": "a && FOO=1",
            "subshell": "(FOO=1 true)"}
PASSING = {
    "host PATH": 'export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH" && cd /t',
    "IFS read": "while IFS= read -r -d '' f; do :; done",
    "LC_ALL": "LC_ALL=C sort -z",
    "verdict word": "MECLAW_GATE_MODE=strand true",
    "lane word quoted": "env " + shlex.quote("RUSTFLAGS=--cfg x") + " true",
    "lower case": "foo=1 true",
    "read of a value": 'echo "$FOO" "${FOO:-x}"',
    "comparison": '[ "$X" = y ]',
    "mixed-case option": "systemd-run -p CPUQuota=50% true",
    "ssh option": "ssh -o BatchMode=yes h true",
    # M7h (pre red, seed 384518679): the value of a verdict word is no name --
    # measure's own script under the gate's ruling env was refused for `OR`
    "ruling value": "export MECLAW_TESTING_RULES_RULING=OR-TV-8,OR-TV-9 MECLAW_REQUIRE_BROWSERS=1",
    "ruling value quoted": "export MECLAW_TESTING_RULES_RULING='OR-TV-8,OR-TV-9'",
    "ruling value double-quoted": 'declare -x MECLAW_TESTING_RULES_RULING="OR-TV-8 OR-TV-9"',
}


class WordCheck(unittest.TestCase):
    """M2: fail-closed on both sides, one corpus (parity)."""

    def test_the_eight_forms_and_the_setting_forms_are_refused_on_both_sides(self):
        forms = dict(EIGHT_FORMS, **SETTING_FORMS, **CONTROLS)
        rcs = bash_refusals(list(forms.values()))
        for (label, script), rc in zip(forms.items(), rcs):
            with self.subTest(form=label):
                self.assertEqual(2, rc, "bash passed %r" % script)
                self.assertTrue(py_refused(script), "python passed %r" % script)

    def test_the_kit_forms_pass_on_both_sides(self):
        rcs = bash_refusals(list(PASSING.values()))
        for (label, script), rc in zip(PASSING.items(), rcs):
            with self.subTest(form=label):
                self.assertEqual(0, rc, "bash refused %r" % script)
                self.assertFalse(py_refused(script), "python refused %r" % script)

    def test_the_kit_scripts_pass_and_each_with_a_word_in_front_is_refused(self):
        # The eleven `bash -c $(q "$X")` places of lane_sync.sh (re-check: :572
        # :584 :659 :712 :718 :991 :1077 :1161 :1221 :1327; R<->N merge NS: the
        # quota scope's QUOTA_MARK_SH, tv-br M10u): each one's own
        # script passes, and with an unlisted word in front -- the `$'..'`
        # form `q` writes for a multi-line script -- it is refused.
        text = pathlib.Path(lane.LANE_SYNC).read_text(encoding="utf-8")
        names = re.findall(r'bash -c \$\(q "\$([A-Z_]+)', text)
        self.assertEqual(sorted(["HOLD_SH"] * 3 + ["HASHES", "BROWSER_DEPS", "DIGEST", "GOLDEN_HASH",
                                 "GOLDEN_HASH", "CHECK_ON_HOST", "INVENTORY",
                                 "QUOTA_MARK_SH"]), sorted(names))
        for name in sorted(set(names)):
            with self.subTest(place=name):
                res = bash('remote_words_known "cd /t && bash -c $(q "${!1}")" || exit 7; '
                           'remote_words_known "cd /t && bash -c $(q "FOO=1"$\'\\n\'"${!1}")"', name)
                self.assertEqual(2, res.returncode, res.stderr)
                self.assertIn("lane word FOO", res.stderr)
        for const in ("GOLDEN_HASH", "HASHES", "DIGEST", "BROWSER_DEPS"):
            with self.subTest(python=const):
                script = getattr(lane, const)
                self.assertFalse(py_refused("cd /t && bash -c %s" % lane.q(script)))
                self.assertTrue(py_refused("cd /t && bash -c %s" % lane.q("FOO=1\n" + script)))

    def test_the_mirrored_constants_are_byte_equal(self):
        """Re-check tv-bn M7r B-M1 (OR-T11-27): the scripts both drivers send by
        `bash -c` -- the overlay digest, the content hashes, the golden policy and
        the browser drivers -- are one text on both sides, every branch, every
        separator, every algorithm: read from lane_sync.sh at run time, not by
        their words."""
        for const in ("DIGEST", "HASHES", "GOLDEN_HASH", "BROWSER_DEPS"):
            with self.subTest(const=const):
                res = bash('printf %s "${!1}"', const)
                self.assertEqual(0, res.returncode, res.stderr)
                self.assertEqual(getattr(lane, const), res.stdout)

    def test_measures_script_passes_under_the_gates_ruling_env(self):
        # M7h: a pass with MECLAW_TESTING_RULES_RULING set (the gate's, OR-TV-8
        # form with commas) -- measure.timed_script carries it as a verdict word
        ruling = {"MECLAW_TESTING_RULES_RULING": "OR-TV-8,OR-TV-9,OR-TV-10,OR-TV-2"}
        with unittest.mock.patch.dict(os.environ, ruling):
            script = measure.timed_script("/t", "/tg", "", "true", "/l/x.log")
        self.assertIn("OR-TV-8,OR-TV-9", script)
        self.assertFalse(py_refused(script))
        self.assertEqual([0], bash_refusals([script]))

    def test_the_python_quoting_places_refuse_a_word(self):
        # lane.py exec_stream (" ".join(q(w))), abort (same) and
        # flake_triage.lane_unittest_argv (shlex.join): a word as a quoted
        # argv element never leaves, and no process starts.
        ln = lane.Lane(name="l", ssh="h")
        with unittest.mock.patch.object(lane.subprocess, "Popen") as popen, \
                unittest.mock.patch.object(lane.subprocess, "run") as run:
            with self.assertRaisesRegex(lane.LaneError, "lane word FOO is not in"):
                lane.exec_stream(ln, {"run": "r1", "tree": "/t"}, pipeline="FOO=a b")
            with self.assertRaisesRegex(lane.LaneError, "lane word FOO is not in"):
                lane.abort(ln, "/t", "r1", ws="FOO=a b")
            with self.assertRaisesRegex(lane.LaneError, "lane word FOO is not in"):
                flake_triage.lane_unittest_argv("h", "/t", ["python3", "FOO=a b"], 10, environ={})
            popen.assert_not_called()
            run.assert_not_called()


class StructuredEnv(unittest.TestCase):
    """M2: the narrow point builds the environment from data."""

    def test_env_prefix_quotes_each_value_and_refuses_a_name_outside_the_lists(self):
        self.assertEqual("unset MECLAW_GATE_MODE; export MECLAW_TIER_LANE='a b'; ",
                         lane.env_prefix({"MECLAW_TIER_LANE": "a b"}, ["MECLAW_GATE_MODE"]))
        for bad in ({"FOO": "1"}, {"PATH": "/x"}, {"TMPDIR": "/x"}):
            with self.subTest(env=bad), self.assertRaisesRegex(lane.LaneError, "lane word (FOO|PATH|TMPDIR) is not in"):
                lane.env_prefix(bad)
        with self.assertRaisesRegex(lane.LaneError, "lane word FOO is not in"):
            lane.env_prefix(unset=["FOO"])

    def test_ssh_argv_puts_the_environment_in_front_and_refuses_before_any_process(self):
        argv = lane.ssh_argv("local", "true", env={"MECLAW_TIER_LANE": "1"})
        self.assertEqual(["bash", "-c", "export MECLAW_TIER_LANE=1; true"], argv)
        with self.assertRaisesRegex(lane.LaneError, "lane word FOO is not in"):
            lane.ssh_argv("h", "true", env={"FOO": "x"})
        with self.assertRaisesRegex(TypeError, "env/unset need a command string"):
            lane.ssh_argv("h", ["true"], env={"MECLAW_TIER_LANE": "1"})

    def test_lane_unittest_argv_carries_the_verdict_by_data(self):
        argv = flake_triage.lane_unittest_argv("local", "/t", ["python3", "-V"], 10,
                                               environ={"MECLAW_GATE_MODE": "strand x"})
        script = argv[-1]
        self.assertIn("export MECLAW_GATE_MODE='strand x'", script)
        self.assertIn("MECLAW_REQUIRE_BROWSERS=1", script)
        self.assertTrue(script.startswith("unset "), script)

    def test_env_word_refuses_a_name_outside_the_lists_in_this_shell(self):
        res = bash('w=""; env_word w FOO 1; echo reached')
        self.assertEqual(2, res.returncode)
        self.assertNotIn("reached", res.stdout)
        res = bash('w=""; env_word w PATH /x; echo reached')
        self.assertEqual(2, res.returncode)
        res = bash('w=""; env_word w MECLAW_TIER_LANE "a b"; env_word w CARGO_INCREMENTAL 1; printf %s "$w"')
        self.assertEqual("MECLAW_TIER_LANE=a\\ b CARGO_INCREMENTAL=1 ", res.stdout)

    def test_lane_words_known_refuses_host_own_names_the_narrow_point_allows(self):
        self.assertEqual(2, bash('lane_words_known "PATH=/x"').returncode)
        self.assertEqual(0, bash('remote_words_known "PATH=/x"').returncode)

    def test_lane_words_known_refuses_a_lower_or_mixed_case_word(self):
        """Re-check tv-bn M7h A-M3: the base refused `foo=1`/`Foo=1` among the
        words of `lane_env`/`cmd_test`; M7g's shared scan took only upper-case
        names there. Lower-case names are a host script's own on the remote
        side (OR-M7e-2), never a word of the caller."""
        for word in ("foo=1", "Foo=1", "MECLAW_GATE_MODE=x foo=1 "):
            with self.subTest(word=word):
                self.assertEqual(2, bash('lane_words_known "$1"', word).returncode)
        self.assertEqual(0, bash('lane_words_known "MECLAW_GATE_MODE=x CARGO_INCREMENTAL=1 "').returncode)
        self.assertEqual(0, bash('remote_words_known "foo=1 true"').returncode)


class Routes(unittest.TestCase):
    """M1: each former own way now runs through the narrow point."""

    def test_run_guarded_checks_the_words_before_it_starts_anything(self):
        with tempfile.TemporaryDirectory() as d:
            mark = os.path.join(d, "ran")
            res = bash('run_guarded "FOO=1 touch $1"', mark)
            self.assertEqual(2, res.returncode, res.stderr)
            self.assertIn("lane word FOO", res.stderr)
            self.assertFalse(os.path.exists(mark))

    def test_flake_triage_shell_runs_local_and_lanes_through_the_narrow_point(self):
        self.assertEqual(["bash", "-c", "true"], flake_triage._shell(flake_triage.LOCAL, "true"))
        for target in (flake_triage.LOCAL, "h"):
            with self.subTest(target=target), self.assertRaisesRegex(lane.LaneError, "lane word FOO is not in"):
                flake_triage._shell(target, "FOO=1 true")

    def test_each_route_asks_the_narrow_point(self):
        calls = []
        real = lane.ssh_argv

        def spy(target, command, **kw):
            calls.append(target)
            return real(target, command, **kw)

        cov_lane = importlib.import_module("pipeline.lane")  # coverage's own import path
        with unittest.mock.patch.object(lane, "ssh_argv", side_effect=spy), \
                unittest.mock.patch.object(cov_lane, "ssh_argv", side_effect=spy), \
                unittest.mock.patch.object(hygiene, "_engstelle", return_value=lane):
            ctx = unittest.mock.Mock()
            ctx.run.return_value = (0, "", "")
            hygiene.idle_probe(ctx, "h-idle")
            push = register.KitPush(rev="a" * 40, sha8="a" * 8, core_root="/c", dest_root="/d",
                                    files=[], target="h-reg")
            push.steps()
            with unittest.mock.patch.object(coverage.subprocess, "run") as run:
                run.return_value.returncode = 0
                coverage.lane_up("h-cov")
            with unittest.mock.patch.object(measure.subprocess, "run") as run:
                run.return_value = unittest.mock.Mock(returncode=0, stdout="", stderr="")
                measure.ssh_runner("h-measure")("true", 5)
            flake_triage._shell("h-flake", "true")
        for target in ("h-idle", "h-reg", "h-cov", "h-measure", "h-flake"):
            self.assertIn(target, calls)

    def test_a_word_in_a_route_never_leaves(self):
        with unittest.mock.patch.object(hygiene, "IDLE_PROBE_SH", "FOO=1 true"), \
                unittest.mock.patch.object(hygiene, "_engstelle", return_value=lane):
            ctx = unittest.mock.Mock()
            with self.assertRaisesRegex(lane.LaneError, "lane word FOO is not in"):
                hygiene.idle_probe(ctx, "h")
            ctx.run.assert_not_called()
        push = register.KitPush(rev="a" * 40, sha8="a" * 8, core_root="/c", dest_root="/d",
                                files=[], target="h")
        with unittest.mock.patch.object(register.KitPush, "remote_before", return_value=["FOO=1", "true"]):
            with self.assertRaisesRegex(lane.LaneError, "lane word FOO is not in"):
                push.steps()
        with unittest.mock.patch.object(measure.subprocess, "run") as run:
            with self.assertRaisesRegex(SystemExit, "lane word FOO is not in"):
                measure.ssh_runner("h")("FOO=1 true", 5)
            run.assert_not_called()


# ---------------------------------------------------------------------------
# the registry of fake ssh programs (interface to scripts/tests/fake_ssh.py)
# ---------------------------------------------------------------------------

class Registry(unittest.TestCase):
    """Re-check tv-bn M7h A-M1 (orchestrator OR-T10-2): the narrow point
    checks ALWAYS, without a test-mode switch, and admits by origin, not by
    path: the system's own `ssh` (the realpath of `ssh` in SYSTEM_SSH_DIRS,
    neither the file nor its directory writable here) or a fake of
    `fake_ssh.py` whose registry entry carries its path AND the sha256 of its
    body. No registry = no fake (fail-closed)."""

    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        # the fake comes from the one maker (re-check tv-bn M7e, M7h): what is
        # "unregistered" here is the registry each test hands in, not the file
        self.mark = os.path.join(self.dir.name, "ran")
        self.fake = str(fake_ssh.install_inert(self.dir.name, "engstelle registry pin: logs, runs nothing",
                                               log=self.mark, rc=0))
        self.entry = fake_ssh.entry(self.fake)

    def env(self, registry, where=None):
        env = {"PATH": (where or os.path.dirname(self.fake)) + ":" + store_isolation.trapped_path()}
        if registry is not None:
            env[lane.FAKE_SSH_REGISTRY] = registry
        return env

    def refused(self, env, path=None):
        with self.assertRaisesRegex(lane.LaneError, "unregistered ssh on PATH: ") as cm:
            lane.ssh_program(env)
        self.assertIn(path or self.fake, str(cm.exception))

    def bash_refused(self, env, path=None, snippet="remote h true"):
        res = bash(snippet, env=dict(os.environ, **env) if lane.FAKE_SSH_REGISTRY in env
                   else {k: v for k, v in dict(os.environ, **env).items() if k != lane.FAKE_SSH_REGISTRY})
        self.assertEqual(2, res.returncode, res.stderr)
        self.assertIn("unregistered ssh on PATH: " + (path or self.fake), res.stderr)
        self.assertFalse(os.path.exists(self.mark))

    def test_an_unregistered_fake_is_refused_with_its_path(self):
        self.refused(self.env(""))
        self.refused(self.env("/elsewhere/fake:relative/" + os.path.basename(self.fake)))

    def test_a_path_without_the_hash_of_its_body_is_refused(self):
        self.refused(self.env(self.fake))
        self.refused(self.env(self.fake + "@" + "0" * 64))
        self.bash_refused(self.env(self.fake))

    def test_a_registered_fake_passes(self):
        self.assertEqual(os.path.realpath(self.fake),
                         lane.ssh_program(self.env("/x/ssh@" + "0" * 64 + ":" + self.entry)))

    def test_no_registry_admits_no_fake_on_either_side(self):
        """Was pinned green (no test mode pass): a child with an environment
        from scratch ran a hand fake unchecked."""
        self.refused(self.env(None))
        self.bash_refused(self.env(None))

    def test_a_fake_overwritten_after_registering_is_refused_on_either_side(self):
        with open(self.fake, "w", encoding="utf-8") as fh:
            fh.write("#!/bin/sh\ntouch %s\n" % shlex.quote(self.mark))
        self.refused(self.env(self.entry))
        self.bash_refused(self.env(self.entry))

    def test_an_ssh_in_a_system_dir_this_user_can_write_is_refused_on_either_side(self):
        """A system directory admits only an ssh nobody here can rewrite: the
        maker's body in a writable `SYSTEM_SSH_DIRS` entry is refused (the
        link to another program: `test_fake_ssh`, the sweep's own file)."""
        sysdir = os.path.join(self.dir.name, "sysbin")
        os.makedirs(sysdir)
        own = str(fake_ssh.install_inert(sysdir, "engstelle: a writable system dir", log=self.mark, rc=0))
        with unittest.mock.patch.object(lane, "SYSTEM_SSH_DIRS", (sysdir,)):
            self.refused(self.env("", sysdir), own)
        self.bash_refused(self.env("", sysdir), own, 'SYSTEM_SSH_DIRS=("${PATH%%:*}"); remote h true')

    def test_the_system_ssh_passes_with_and_without_a_registry(self):
        dirs = [d for d in lane.SYSTEM_SSH_DIRS if shutil.which("ssh", path=d)]
        if not dirs:
            self.skipTest("no system ssh here")
        own = os.path.realpath(shutil.which("ssh", path=dirs[0]))
        self.assertEqual(own, lane.ssh_program({"PATH": dirs[0], lane.FAKE_SSH_REGISTRY: ""}))
        self.assertEqual(own, lane.ssh_program({"PATH": dirs[0]}))
        res = bash('ssh_bin; printf %s "$SSH_CHECKED"', env={"PATH": dirs[0] + ":" + store_isolation.trapped_path()})
        self.assertEqual((own, 0), (res.stdout, res.returncode), res.stderr)

    def test_ssh_argv_and_rsh_refuse_an_unregistered_fake(self):
        with unittest.mock.patch.dict(os.environ, self.env("")):
            with self.assertRaisesRegex(lane.LaneError, "unregistered ssh on PATH: "):
                lane.ssh_argv("h", "true")
            with self.assertRaisesRegex(lane.LaneError, "unregistered ssh on PATH: "):
                lane.rsh()
            self.assertEqual(["bash", "-c", "true"], lane.ssh_argv("local", "true"))
        with unittest.mock.patch.dict(os.environ, self.env(self.entry)):
            self.assertEqual(os.path.realpath(self.fake), lane.ssh_argv("h", "true")[0])

    def test_the_bash_side_refuses_an_unregistered_fake_before_it_runs(self):
        self.bash_refused(self.env(""))
        env = dict(os.environ, **self.env(self.entry))
        res = bash('remote h true', env=env)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertTrue(os.path.exists(self.mark))


class CheckedIsRun(unittest.TestCase):
    """Re-check tv-bn M7j A-M1/A-M3/A-M4 (orchestrator OR-T10-2, OR-T10-11):
    what the narrow point checked is what runs -- its absolute path is the
    program word of every way, a hand fake under another PATH or behind an
    exported function never runs, no PATH is no ssh; the Bash check takes its
    tools by absolute path; and the program the host starts for rsync and
    git push runs under `env -i` like every remote command."""

    def setUp(self):
        self.dir = pathlib.Path(tempfile.mkdtemp(prefix="checked-"))
        self.addCleanup(shutil.rmtree, self.dir, True)
        (self.dir / "reg").mkdir()
        (self.dir / "hand").mkdir()
        self.log = self.dir / "fake.log"
        self.fake = os.path.realpath(fake_ssh.install_inert(self.dir / "reg", "checked-is-run: logs, runs nothing",
                                                            log=self.log, rc=0))
        # the hand fake comes from the one maker as well (the sweep of
        # test_fake_ssh allows no other writer); no registry the check sees
        # names it, and it must never run
        self.hand_mark = self.dir / "HAND-RAN"
        fake_ssh.install_inert(self.dir / "hand", "checked-is-run: the hand fake, must not run",
                               log=self.hand_mark, rc=0, env={})
        self.env = {"PATH": str(self.dir / "reg") + ":" + store_isolation.trapped_path(),
                    lane.FAKE_SSH_REGISTRY: fake_ssh.entry(self.fake)}

    def ran(self):
        return self.log.read_text(encoding="utf-8") if self.log.exists() else ""

    def repo(self):
        repo = self.dir / "repo"
        subprocess.run(["git", "init", "-q", str(repo)], check=True, timeout=kit_time.HANG)
        subprocess.run(["git", "-C", str(repo), "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q",
                        "--allow-empty", "-m", "x"], check=True, timeout=kit_time.HANG)
        return repo

    def test_python_runs_the_checked_path_not_the_name(self):
        """P4: checked under this PATH, run under a child's PATH with a hand
        fake -- the hand fake ran (HAND-WRITTEN BODY RAN)."""
        with unittest.mock.patch.dict(os.environ, self.env):
            argv = lane.ssh_argv("h", "true")
            self.assertEqual(self.fake, argv[0])
            self.assertEqual(self.fake, shlex.split(lane.rsh())[0])
        res = subprocess.run(argv, env={"PATH": str(self.dir / "hand")}, capture_output=True, text=True,
                             timeout=kit_time.HANG)
        self.assertEqual(0, res.returncode, (argv[0], self.fake, open(self.fake).read(), res.stderr))
        self.assertFalse(self.hand_mark.exists())
        self.assertIn("ssh ", self.ran())

    def test_no_path_or_no_ssh_is_refused_on_either_side(self):
        """P10: PATH empty -> `ssh` unchecked, the exec found `./ssh`."""
        for env in ({"PATH": ""}, {}, {"PATH": "hand"}, {"PATH": str(self.dir / "nothing")}):
            with self.subTest(env=env):
                with self.assertRaisesRegex(lane.LaneError, "no (PATH to find ssh on|ssh on PATH)"):
                    lane.ssh_program(dict(env, **{lane.FAKE_SSH_REGISTRY: self.env[lane.FAKE_SSH_REGISTRY]}))
                with unittest.mock.patch.dict(os.environ, env, clear=True):
                    with self.assertRaisesRegex(lane.LaneError, "no (PATH to find ssh on|ssh on PATH)"):
                        lane.ssh_argv("h", "true")
                # `env -u PATH`: the side-effect guard hands an env without PATH its trap
                drop = [] if "PATH" in env else ["/usr/bin/env", "-u", "PATH"]
                res = subprocess.run(drop + ["/bin/bash", "-c", 'source "$0"; remote h true', _fns()], cwd=self.dir,
                                     env=dict(env, **{lane.FAKE_SSH_REGISTRY: self.env[lane.FAKE_SSH_REGISTRY]}),
                                     capture_output=True, text=True, timeout=kit_time.HANG)
                self.assertEqual(2, res.returncode, res.stderr)
                self.assertRegex(res.stderr, "no (PATH to find ssh on|ssh on PATH)")
                self.assertFalse(self.hand_mark.exists())

    def test_bash_runs_the_checked_path_not_an_exported_function(self):
        """The name `ssh` called an exported function `ssh` past the check,
        on all four Bash ways."""
        env = dict(os.environ, **self.env)
        env["BASH_FUNC_ssh%%"] = "() { touch %s; }" % shlex.quote(str(self.hand_mark))
        src = self.dir / "src"
        src.write_text("x\n", encoding="utf-8")
        repo = self.repo()
        for snippet in ("remote h true", "remote_long h true", 'lane_rsync h "$1" %LANE%/nowhere',
                        'lane_git_push h /nowhere/bare.git HEAD:refs/heads/x'):
            with self.subTest(snippet=snippet):
                self.log.unlink(missing_ok=True)
                cwd = repo if "git" in snippet else self.dir
                res = subprocess.run(["bash", "-c", 'source "$0"; ' + snippet, _fns(), str(src)], cwd=cwd,
                                     env=env, capture_output=True, text=True, timeout=kit_time.HANG)
                self.assertFalse(self.hand_mark.exists(), res.stderr)
                self.assertIn("ssh ", self.ran(), res.stderr)

    def test_rsync_and_git_push_start_the_host_side_under_env_i(self):
        """P8: rsync and git built the host's command themselves --
        `h rsync --server ...`, `git-receive-pack '...'` -- without `env -i`
        (OR-T10-11: all five ways)."""
        keep = lane.remote_env_i()
        self.assertTrue(keep.startswith("env -i ${HOME+\"HOME=$HOME\"} "), keep)
        res = bash("remote_env_keep")
        self.assertEqual(keep, res.stdout)
        src = self.dir / "src"
        src.write_text("x\n", encoding="utf-8")
        repo = self.repo()
        ssh_lane = type("L", (), {"transport": "ssh", "ssh": "h"})()

        def py_rsync():
            lane._rsync(ssh_lane, [str(src), "h:/nowhere/x"])

        def py_register():
            subprocess.run(["rsync"] + register._rsh("h") + [str(src), "h:/nowhere/x"], capture_output=True,
                           timeout=kit_time.HANG)

        def py_push():
            lane._git_push(ssh_lane, str(repo), "/nowhere/bare.git", ["HEAD:refs/heads/x"])

        def sh(snippet, cwd):
            return lambda: subprocess.run(["bash", "-c", 'source "$0"; ' + snippet, _fns(), str(src)], cwd=cwd,
                                          env=dict(os.environ, **self.env), capture_output=True,
                                          timeout=kit_time.HANG)

        ways = {"lane._rsync": (py_rsync, "rsync --server"), "register._rsh": (py_register, "rsync --server"),
                "lane._git_push": (py_push, "git-receive-pack '/nowhere/bare.git'"),
                "lane_rsync": (sh('lane_rsync h "$1" %LANE%/nowhere/x', self.dir), "rsync --server"),
                "lane_git_push": (sh("lane_git_push h /nowhere/bare.git HEAD:refs/heads/x", repo),
                                  "git-receive-pack '/nowhere/bare.git'")}
        for name, (run, prog) in ways.items():
            with self.subTest(way=name):
                self.log.unlink(missing_ok=True)
                with unittest.mock.patch.dict(os.environ, self.env):
                    run()
                line = self.ran()
                self.assertIn(" h %s%s" % (keep, prog), line)

    def test_rsync_and_git_of_a_remote_way_run_their_checked_path(self):
        """Re-check tv-bn M7l A-H5 (orchestrator OR-T10-25): the local rsync
        and git of a remote way ran by name from PATH -- a shadow there skips
        `--rsync-path`/`--receive-pack` and with them the host's `env -i`.
        Now the narrow point checks them like ssh: an unregistered one is
        refused before it runs, a registered fake runs by its absolute path,
        the system's passes."""
        tools = self.dir / "tools"
        tools.mkdir()
        hand_ran = self.dir / "HAND-TOOL-RAN"
        for name in ("rsync", "git"):
            (tools / name).write_text("#!/bin/sh\necho %s >> %s\nexit 0\n" % (name, shlex.quote(str(hand_ran))),
                                      encoding="utf-8")
            (tools / name).chmod(0o755)
        src = self.dir / "src"
        src.write_text("x\n", encoding="utf-8")
        repo = self.repo()
        ssh_lane = type("L", (), {"transport": "ssh", "ssh": "h"})()
        env = dict(self.env, PATH="%s:%s" % (tools, self.env["PATH"]))
        py_ways = {"lane._rsync": lambda: lane._rsync(ssh_lane, [str(src), "h:/nowhere/x"]),
                   "lane._git_push": lambda: lane._git_push(ssh_lane, str(repo), "/nowhere/bare.git", ["HEAD"]),
                   "register.assets_spec": lambda: register.assets_spec(self.dir / "cache", target="h"),
                   "register.display_lab_spec": lambda: register.display_lab_spec(REPO, target="h"),
                   "register.KitPush": lambda: register.KitPush("r", "s", str(REPO), "/d", ["a"], "h").rsync_argv()}
        sh_ways = {"lane_rsync": ('lane_rsync h "$1" %LANE%/nowhere/x', self.dir, "rsync"),
                   "lane_git_push": ("lane_git_push h /nowhere/bare.git HEAD:refs/heads/x", repo, "git")}
        for name, run in py_ways.items():
            with self.subTest(way=name):
                with unittest.mock.patch.dict(os.environ, env):
                    with self.assertRaisesRegex(lane.LaneError, "unregistered (rsync|git) on PATH: %s/(rsync|git)"
                                                % re.escape(str(tools))):
                        run()
        for name, (snippet, cwd, tool) in sh_ways.items():
            with self.subTest(way=name):
                res = subprocess.run(["bash", "-c", 'source "$0"; ' + snippet, _fns(), str(src)], cwd=cwd,
                                     env=dict(os.environ, **env), capture_output=True, text=True,
                                     timeout=kit_time.HANG)
                self.assertEqual(2, res.returncode, res.stderr)
                self.assertIn("unregistered %s on PATH: %s/%s" % (tool, tools, tool), res.stderr)
        self.assertFalse(hand_ran.exists())
        for name in ("rsync", "git"):
            with self.subTest(registered=name), unittest.mock.patch.dict(os.environ):
                reg = dict(env)
                real = fake_ssh.register_tool(tools / name, reg)
                self.assertEqual(real, lane.checked_program(name, reg))
                res = subprocess.run([lane.LANE_SYNC, "checked", name], env=reg, capture_output=True, text=True,
                                     timeout=kit_time.HANG)
                self.assertEqual((0, real + "\n"), (res.returncode, res.stdout), res.stderr)
                system = os.path.realpath(shutil.which(name, path=store_isolation.MINIMAL_PATH))
                for check in (lambda: lane.checked_program(name, self.env),
                              lambda: subprocess.run([lane.LANE_SYNC, "checked", name], env=self.env,
                                                     capture_output=True, text=True,
                                                     timeout=kit_time.HANG).stdout.strip()):
                    self.assertEqual(system, check())
        both = "%s:%s" % (self.env[lane.FAKE_SSH_REGISTRY], fake_ssh.entry(tools / "rsync"))
        with unittest.mock.patch.dict(os.environ, dict(env, **{lane.FAKE_SSH_REGISTRY: both})):
            self.assertEqual(os.path.realpath(tools / "rsync"), register.display_lab_spec(REPO, target="h")[0][0])
            self.assertEqual("rsync", register.display_lab_spec(REPO)[0][0])


class EntryIsolation(unittest.TestCase):
    """Re-check tv-bn M7l A-M1: the check ran in a process that took the
    caller's code -- an exported `exit`, BASH_ENV, `bash` from PATH, a
    sitecustomize on PYTHONPATH -- and the hand fake ran. Every entry of the
    kit starts its shell privileged (`#!/bin/bash -p`) and its Python
    isolated by absolute path (`/usr/bin/python3 -I`); each form is locked
    on the EXECUTED entry, not on sourced functions."""

    BASH_ENTRIES = ("scripts/lane_sync.sh", "scripts/strand.sh", "scripts/gate.sh")
    PY_ENTRIES = ("scripts/pipeline.py", "scripts/flake_triage.py", "scripts/pipeline/pre.py",
                  "scripts/pipeline/hygiene.py")

    def setUp(self):
        self.dir = pathlib.Path(tempfile.mkdtemp(prefix="entry-"))
        self.addCleanup(shutil.rmtree, self.dir, True)
        (self.dir / "hand").mkdir()
        self.mark = self.dir / "HAND-RAN"
        self.hand = os.path.realpath(fake_ssh.install_inert(self.dir / "hand", "entry isolation: must not run",
                                                            log=self.mark, rc=0, env={}))
        self.home = self.dir / "home"
        self.home.mkdir()
        self.path = "%s:%s" % (self.dir / "hand", store_isolation.trapped_path())

    def test_every_bash_entry_runs_privileged_and_python_isolated(self):
        for rel in self.BASH_ENTRIES:
            with self.subTest(entry=rel):
                text = (REPO / rel).read_text(encoding="utf-8")
                self.assertEqual("#!/bin/bash -p", text.splitlines()[0])
                # its Python: the kit's one spelling, `KIT_PY` (test_every_kit_shell_entry_...)
                self.assertEqual([], self.python_starts(rel))
        for rel in self.PY_ENTRIES:
            with self.subTest(entry=rel):
                self.assertEqual("#!/usr/bin/python3 -I", (REPO / rel).read_text(encoding="utf-8").splitlines()[0])

    # Every kit shell file -- the one scan of KitShellClass (`kit_tree.shell_files`:
    # tracked under scripts/ at any depth, `*.sh` or a shell shebang; re-check
    # bundle N N-K6, OR-T12-18) -- is either a kit entry (scanned: its Python only
    # as `KIT_PY`) or named here with the reason (re-check tv-bn M7t B-M4). A new
    # shell file is classified, never skipped.
    KIT_PY_LIB = "scripts/pipeline/kit_python.sh"
    NOT_KIT_PY = {
        "scripts/gate_legacy.sh": "OR-tv-bn.M7o.1: the public CI runner (`gate.sh ci`); its Python "
                                  "starts are gate_plan.py and stdin heredocs that import the standard "
                                  "library only -- none reaches the narrow point (pinned below)",
        "scripts/publish_docs.sh": "not the kit: publishes the docs site, on no gate or lane path",
        "scripts/start.sh": "not the kit: the colony starter; `need python3` names a tool, starts none",
        "scripts/pipeline/lane_fingerprint.sh": "POSIX sh the lane reads from stdin: nothing to source; "
                                                "its literal is pinned byte-equal to lane.PYTHON below",
    }
    PY_TOKEN = re.compile(r"(?<![\w-])(?:/usr/bin/)?python(?:3(?:\.\d+)?)?(?![\w.-])")

    # The files that take the kit's Python, each with the FULL list of kit
    # libraries it sources itself, in source order (N-K6: equality, not
    # containment -- a library that redefines KIT_PY is a new entry here).
    KIT_PY_USERS = {
        "scripts/cargo-target.sh": ["pipeline/kit_python.sh"],
        "scripts/gate.sh": ["pipeline/kit_python.sh"],
        "scripts/lane_sync.sh": ["pipeline/kit_python.sh"],
        "scripts/strand.sh": ["pipeline/kit_python.sh"],
        "scripts/test-tier.sh": ["pipeline/kit_python.sh", "cargo-target.sh"],
        "scripts/pipeline/coverage_host.sh": ["pipeline/kit_python.sh", "cargo-target.sh"],
        "scripts/pipeline/coverage_lane.sh": ["pipeline/kit_python.sh"],
    }

    @staticmethod
    def shell_files():
        return kit_tree.shell_files(REPO)

    def python_starts(self, rel):
        """The non-comment lines of `rel` that name a Python other than `KIT_PY`."""
        return [(i, line) for i, line in enumerate((REPO / rel).read_text(encoding="utf-8").splitlines(), 1)
                if not line.lstrip().startswith("#") and self.PY_TOKEN.search(line)]

    def test_kit_py_is_lane_python_at_run_time(self):
        """OR-T11-27: the shell spelling of the kit's Python is asked of bash
        and compared with `lane.PYTHON` -- no regex over a literal. A scalar
        KIT_PY in the caller's environment does not survive the source."""
        res = subprocess.run(["/bin/bash", "-p", "-c",
                              '. "$1" && printf "%s\\n" "${KIT_PY[@]}" && kit_py_q',
                              "kit", str(REPO / self.KIT_PY_LIB)],
                             env={"PATH": store_isolation.MINIMAL_PATH, "KIT_PY": "python3"},
                             capture_output=True, text=True, timeout=kit_time.HANG)
        self.assertEqual(0, res.returncode, res.stderr)
        *words, quoted = res.stdout.splitlines()
        self.assertEqual(list(lane.PYTHON), words)
        self.assertEqual(shlex.join(lane.PYTHON), quoted)
        self.assertEqual(list(lane.PYTHON), shlex.split(quoted))

    def test_every_kit_shell_entry_takes_the_kits_python(self):
        files = self.shell_files()
        self.assertIn(self.KIT_PY_LIB, files)
        # no stale reason; publish_docs.sh is not exported (R5: it carries the
        # site's names) -- where it is absent, its reason has nothing to hold
        stale = set(self.NOT_KIT_PY) - set(files)
        if not (REPO / "scripts/publish_docs.sh").exists():
            stale.discard("scripts/publish_docs.sh")
        self.assertEqual([], sorted(stale))
        users = []
        for rel in files:
            if rel in self.NOT_KIT_PY:
                continue
            with self.subTest(entry=rel):
                text = (REPO / rel).read_text(encoding="utf-8")
                starts = self.python_starts(rel)
                if rel == self.KIT_PY_LIB:  # the one definition
                    self.assertEqual(["KIT_PY=(/usr/bin/python3 -I)"], [line for _, line in starts])
                    continue
                self.assertEqual([], starts)
                if re.search(r"KIT_PY\[@\]|kit_py_q", text):
                    users.append(rel)
                    # sourced by the file itself, never inherited from whoever sourced it,
                    # and fail-closed (`|| exit 2`, M7w): a tree without the library stops
                    # before any Python start; the same reading seeds every test tree. The
                    # full list: no further library in between (N-K6). Its own directory
                    # with builtins only is KitShellClass's (`kit_tree.self_by_tool`).
                    self.assertEqual(self.KIT_PY_USERS.get(rel), kit_tree.sourced(rel[len("scripts/"):]))
        # the entries that start Python at all (a vacuous scan would pass anything)
        self.assertEqual(sorted(self.KIT_PY_USERS), sorted(users))

    def test_every_kit_py_user_ends_on_the_lane_python(self):
        """N-K6: each KIT_PY user's own libraries as the file sources them
        (kit_tree, not the list above), in its order from the repository
        (bash follows their sources): KIT_PY is lane.PYTHON after the last
        one -- a library further down that redefines it is red at run time,
        not only by its spelling."""
        for rel in sorted(self.KIT_PY_USERS):
            libs = kit_tree.sourced(rel[len("scripts/"):])
            with self.subTest(entry=rel, libs=libs):
                self.assertIn("pipeline/kit_python.sh", libs)
                res = subprocess.run(["/bin/bash", "-p", "-c",
                                      'for lib; do . "$lib" || exit 2; done; printf "%s\\n" "${KIT_PY[@]}"',
                                      "kit"] + [str(REPO / "scripts" / lib) for lib in libs],
                                     env={"PATH": store_isolation.MINIMAL_PATH, "KIT_PY": "python3"},
                                     capture_output=True, text=True, timeout=kit_time.HANG)
                self.assertEqual(0, res.returncode, res.stderr)
                self.assertEqual(list(lane.PYTHON), res.stdout.splitlines())

    def test_the_exceptions_keep_their_reason(self):
        """The NOT_KIT_PY reasons, held by the files: lane_fingerprint.sh is
        POSIX sh and spells lane.PYTHON byte for byte; gate_legacy.sh starts
        gate_plan.py or a stdin heredoc, standard library only (M7o.1)."""
        fp = (REPO / "scripts/pipeline/lane_fingerprint.sh").read_text(encoding="utf-8")
        self.assertEqual("#!/bin/sh", fp.splitlines()[0])
        code = "\n".join(line for line in fp.splitlines() if not line.lstrip().startswith("#"))
        self.assertGreaterEqual(code.count("python3"), 1)
        self.assertEqual(code.count("python3"), code.count(" ".join(lane.PYTHON) + " "))
        stdlib = set(sys.stdlib_module_names)

        def imports(code):
            return {(n.module or "").split(".")[0] if isinstance(n, ast.ImportFrom) else a.name.split(".")[0]
                    for n in ast.walk(ast.parse(code)) if isinstance(n, (ast.Import, ast.ImportFrom))
                    for a in n.names}

        lines = (REPO / "scripts/gate_legacy.sh").read_text(encoding="utf-8").splitlines()
        starts, i = [], 0
        while i < len(lines):
            line = lines[i]
            here = re.search(r"<<-?'?(\w+)'?", line)
            body_end = i
            if here:
                body_end = next((j for j in range(i + 1, len(lines)) if lines[j].strip() == here.group(1)), i)
            if not line.lstrip().startswith("#"):
                for m in self.PY_TOKEN.finditer(line):
                    rest = line[m.end():].split()
                    starts.append(rest[0] if rest else "")
                    if rest and rest[0] == "-":
                        self.assertLessEqual(imports("\n".join(lines[i + 1:body_end])), stdlib)
            i = body_end + 1
        self.assertEqual({'"$root/scripts/gate_plan.py"', "-"}, set(starts))
        # gate_plan.py and every repository module it imports (transitively): the
        # narrow point (`lane.checked_program`) is in none of them
        reach = self.repo_imports(REPO / "scripts/gate_plan.py")
        self.assertIn(REPO / "scripts/pipeline/crate_root.py", reach)  # the walk is not vacuous
        self.assertNotIn(REPO / "scripts/pipeline/lane.py", reach)
        self.assertEqual([], [str(f) for f in reach if "checked_program" in f.read_text(encoding="utf-8")])

    @staticmethod
    def repo_imports(start):
        """`start` and the repository modules it imports, transitively (scripts/ on sys.path)."""
        scripts, seen, todo = REPO / "scripts", set(), [start]
        while todo:
            f = todo.pop()
            if f in seen:
                continue
            seen.add(f)
            for n in ast.walk(ast.parse(f.read_text(encoding="utf-8"))):
                if isinstance(n, ast.Import):
                    bases, names = [scripts], [a.name for a in n.names]
                elif isinstance(n, ast.ImportFrom):
                    base = f.parent if n.level else scripts
                    mod = (n.module or "").replace(".", "/")
                    bases = [base / mod] if mod else [base]
                    names = [a.name for a in n.names] + ([""] if mod else [])
                else:
                    continue
                for b in bases:
                    for name in names:
                        stem = b / name.replace(".", "/") if name else b
                        for cand in (stem.with_suffix(".py"), stem / "__init__.py"):
                            if cand.is_file():
                                todo.append(cand)
        return seen

    def test_the_executed_lane_sync_takes_no_code_from_the_caller(self):
        """P11/P13/P14 on the executed script: `die` that returns, BASH_ENV
        with `exit()`, the check's globals readonly beforehand."""
        rc_exit = self.dir / "rc-exit"
        rc_exit.write_text("exit() { :; }\n", encoding="utf-8")
        liar = self.dir / "liar"
        liar.mkdir()
        (liar / "stat").write_text("#!/bin/sh\nshift 3\nfor a; do echo 0:755; done\n", encoding="utf-8")
        (liar / "stat").chmod(0o755)
        rc_ro = self.dir / "rc-readonly"
        rc_ro.write_text("readonly SSH_STAT=%s\nreadonly -a SYSTEM_SSH_DIRS=(%s)\n"
                         % (shlex.quote(str(liar / "stat")), shlex.quote(str(self.dir / "hand"))), encoding="utf-8")
        forms = {"exported exit": {"BASH_FUNC_exit%%": "() { :; }"},
                 "exported local": {"BASH_FUNC_local%%": "() { :; }", "ok": "1"},
                 "BASH_ENV exit()": {"BASH_ENV": str(rc_exit)}, "ENV exit()": {"ENV": str(rc_exit)},
                 "BASH_ENV readonly globals": {"BASH_ENV": str(rc_ro)}}
        for name, extra in forms.items():
            for verb in ("status", "checked"):
                with self.subTest(form=name, verb=verb):
                    res = subprocess.run([lane.LANE_SYNC, "status", "h"] if verb == "status"
                                         else [lane.LANE_SYNC, "checked", "ssh"],
                                         env=dict(extra, PATH=self.path, HOME=str(self.home)),
                                         capture_output=True, text=True, timeout=kit_time.HANG, cwd=self.dir)
                    self.assertFalse(self.mark.exists(), res.stdout + res.stderr)
                    if verb == "checked":
                        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
                        self.assertIn("unregistered ssh on PATH: %s" % self.hand, res.stderr)
                    else:
                        self.assertIn("off", res.stdout, res.stderr)

    def test_the_trace_prompt_is_never_the_callers(self):
        """Re-check tv-bn M7m M1: under `MECLAW_LANE_XTRACE=1` bash expanded
        the caller's PS4 before every traced line; `$((ok=1))` set the
        admission inside `tool_bin` and the hand fake ran."""
        made = self.dir / "PS4-RAN"
        forms = {"arith ok": "$((ok=1))", "command": "$(touch %s)" % shlex.quote(str(made)),
                 "arith ok, trace fd": "$((ok=1))"}
        for name, ps4 in forms.items():
            for verb in ("status", "checked"):
                with self.subTest(form=name, verb=verb):
                    env = {"PATH": self.path, "HOME": str(self.home), "MECLAW_LANE_XTRACE": "1", "PS4": ps4}
                    if "fd" in name:
                        env["BASH_XTRACEFD"] = "1"
                    res = subprocess.run([lane.LANE_SYNC, "status", "h"] if verb == "status"
                                         else [lane.LANE_SYNC, "checked", "ssh"],
                                         env=env, capture_output=True, text=True, timeout=kit_time.HANG, cwd=self.dir)
                    self.assertFalse(self.mark.exists(), res.stdout + res.stderr)
                    self.assertFalse(made.exists(), res.stderr)
                    self.assertIn("\n+ ", "\n" + res.stderr)
                    if verb == "checked":
                        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
                        self.assertIn("unregistered ssh on PATH: %s" % self.hand, res.stderr)

    def test_the_check_refuses_in_a_python_that_is_not_isolated(self):
        """Re-check tv-bn M7m (class): `-I` is not inherited by a child the kit
        starts with a bare `python3`, so the lock at the entries alone leaves
        such a child open. The check refuses fail-closed in a process that is
        not isolated, and admits the same program in `lane.PYTHON`."""
        probe = ("import sys; sys.path.insert(0, %r)\nfrom pipeline import lane\n"
                 "try:\n    print(lane.checked_program('sh'))\n"
                 "except lane.LaneError as exc:\n    print('REFUSED', exc)\n" % str(REPO / "scripts"))
        env = {"PATH": store_isolation.trapped_path(), "HOME": str(self.home)}
        bare = subprocess.run(["/usr/bin/python3", "-c", probe], env=env, capture_output=True, text=True,
                              timeout=kit_time.HANG, check=True).stdout
        self.assertIn("REFUSED", bare)
        self.assertIn("isolated Python (/usr/bin/python3 -I)", bare)
        isolated = subprocess.run(lane.python_argv("-c", probe), env=env, capture_output=True, text=True,
                                  timeout=kit_time.HANG, check=True).stdout
        self.assertEqual(os.path.realpath(shutil.which("sh", path=env["PATH"])), isolated.strip())

    def test_a_kit_entry_started_past_its_lock_is_refused(self):
        """Not only a scan of the forms: an entry run by a bare `python3`
        (its shebang lock skipped, as a new unlocked entry would be) with a
        sitecustomize that lies every hash refuses at the narrow point --
        the hand fake never runs."""
        lie = ("import hashlib\nclass _Z:\n    def __init__(self, *a): pass\n    def update(self, *a): pass\n"
               "    def hexdigest(self): return '0' * 64\nhashlib.sha256 = _Z\n")
        site = self.dir / "site"
        site.mkdir()
        (site / "sitecustomize.py").write_text(lie, encoding="utf-8")
        lanes = self.dir / "lanes"
        lanes.write_text("lx  h  /nowhere\n", encoding="utf-8")
        env = {"PATH": self.path, "HOME": str(self.home), lane.FAKE_SSH_REGISTRY: "%s@%s" % (self.hand, "0" * 64),
               "MECLAW_LANES_FILE": str(lanes), "MECLAW_PIPELINE_DB": str(self.dir / "store.db"),
               "PYTHONPATH": str(site)}
        res = subprocess.run(["/usr/bin/python3", str(REPO / "scripts/pipeline.py"), "lanes", "fingerprint", "lx"],
                             env=env, capture_output=True, text=True, timeout=kit_time.HANG, cwd=REPO)
        self.assertIn("isolated Python", res.stdout + res.stderr)
        self.assertFalse(self.mark.exists(), res.stdout + res.stderr)

    def test_every_python_the_kit_starts_is_the_one_python(self):
        """Re-check tv-bn M7m M2: the idle-off units, the night timer, the
        coverage build, the lane dispatch and abort, pre's children, the
        selftest shards and the triage reruns started `python3` or
        `sys.executable` without `-I`. Each start takes `lane.PYTHON`."""
        self.assertEqual(("/usr/bin/python3", "-I"), lane.PYTHON)
        bare = []
        for rel in ("hygiene.py", "night.py", "coverage.py", "lane.py", "pre.py", "selftest_shard.py",
                    "flakes.py", "orga_export.py"):
            tree = ast.parse((REPO / "scripts/pipeline" / rel).read_text(encoding="utf-8"))
            for node in ast.walk(tree):
                if isinstance(node, (ast.List, ast.Tuple)) and node.elts:
                    first = node.elts[0]
                    if len(node.elts) > 1 and isinstance(node.elts[1], ast.Constant) \
                            and node.elts[1].value == "run_scenarios.py":
                        continue   # a station's scenario runner (product, imports beside itself), not the kit
                    if isinstance(first, ast.BoolOp):   # M7p: `[sys.executable or "python3", ...]`
                        first = first.values[0]
                    if (isinstance(first, ast.Constant) and first.value in ("python3", "python")) or (
                            isinstance(first, ast.Attribute) and first.attr == "executable"):
                        bare.append("%s:%d" % (rel, node.lineno))
                elif isinstance(node, ast.Assign) and isinstance(node.value, ast.Attribute) \
                        and node.value.attr == "executable":
                    bare.append("%s:%d" % (rel, node.lineno))
        self.assertEqual([], bare)
        from scripts.pipeline import flakes, hang_dump, night
        self.assertEqual(lane.python_argv("/k/pipeline.py", "night"),
                         night.timer_argv("*-*-* 07:00", "/x.db", "/k/pipeline.py")[-4:])
        self.assertEqual(lane.python_argv(hang_dump.SELF, "-m", "unittest", "m.K.t"),
                         flakes.unittest_isolation_argv("m.K.t"))
        self.assertEqual(list(lane.PYTHON), flakes.t3_unittest_argv("m", 7)[:2])
        # M7p: the triage's own unittest run cut the kit's argv at a fixed width
        self.assertEqual(lane.python_argv(hang_dump.SELF, "-m", "unittest", "-v"),
                         flake_triage.unittest_argv(["m.K.t"])[:len(lane.PYTHON) + 4])
        self.assertEqual(lane.PYTHON, hygiene.PYTHON)

    def test_no_entry_takes_its_shell_from_path(self):
        """P15: `#!/usr/bin/env bash` started the `bash` PATH named."""
        own = self.dir / "own-bash"
        own.mkdir()
        ran = self.dir / "OWN-BASH-RAN"
        (own / "bash").write_text("#!/bin/sh\ntouch %s\nexec /bin/bash \"$@\"\n" % shlex.quote(str(ran)),
                                  encoding="utf-8")
        (own / "bash").chmod(0o755)
        for rel, argv in (("scripts/lane_sync.sh", []), ("scripts/strand.sh", []), ("scripts/gate.sh", ["--help"])):
            with self.subTest(entry=rel):
                subprocess.run([str(REPO / rel)] + argv, env={"PATH": "%s:%s" % (own, store_isolation.trapped_path()),
                                                               "HOME": str(self.home)},
                               capture_output=True, timeout=kit_time.HANG, cwd=self.dir)
                self.assertFalse(ran.exists())

    def test_no_python_entry_takes_code_from_the_caller(self):
        """P17 on the entries: a sitecustomize on PYTHONPATH (or a
        usercustomize in the caller's HOME) that answers every sha256 with
        64 zeros admitted a hand fake whose registry entry says 0...0."""
        lie = ("import hashlib\nclass _Z:\n    def __init__(self, *a): pass\n    def update(self, *a): pass\n"
               "    def hexdigest(self): return '0' * 64\nhashlib.sha256 = _Z\n")
        site = self.dir / "site"
        site.mkdir()
        (site / "sitecustomize.py").write_text(lie, encoding="utf-8")
        user = subprocess.run(["/usr/bin/python3", "-c", "import site; print(site.getusersitepackages())"],
                              env={"HOME": str(self.home)}, capture_output=True, text=True, check=True,
                              timeout=kit_time.HANG).stdout.strip()
        os.makedirs(user)
        (pathlib.Path(user) / "usercustomize.py").write_text(lie, encoding="utf-8")
        lanes = self.dir / "lanes"
        lanes.write_text("lx  h  /nowhere\n", encoding="utf-8")
        base = {"PATH": self.path, "HOME": str(self.home), lane.FAKE_SSH_REGISTRY: "%s@%s" % (self.hand, "0" * 64),
                "MECLAW_LANES_FILE": str(lanes), "MECLAW_PIPELINE_DB": str(self.dir / "store.db")}
        for form, extra in (("PYTHONPATH", {"PYTHONPATH": str(site)}), ("usercustomize", {})):
            for entry in ("scripts/strand.sh", "scripts/pipeline.py"):
                with self.subTest(form=form, entry=entry):
                    res = subprocess.run([str(REPO / entry), "lanes", "fingerprint", "lx"], env=dict(base, **extra),
                                         capture_output=True, text=True, timeout=kit_time.HANG, cwd=REPO)
                    self.assertIn("unregistered ssh on PATH: %s" % self.hand, res.stdout + res.stderr)
                    self.assertFalse(self.mark.exists(), res.stdout + res.stderr)


# ---------------------------------------------------------------------------
# the structure pin over the production code (M1)
# ---------------------------------------------------------------------------

SSH_PROGS = ("ssh", "scp", "sftp")
_PY_PROG_STR = re.compile(r"^(?:ssh|scp|sftp)\s|(?:[;&|`]|\$\()\s*(?:ssh|scp|sftp)\s|-e\s+['\"]?(?:ssh|scp)\b")
_CALL_ARG0 = {"which", "run", "Popen", "call", "check_call", "check_output", "system", "execv", "execvp",
              "execvpe", "execl", "execlp", "spawnv", "spawnvp", "spawnlp", "create_subprocess_exec",
              "create_subprocess_shell"}
# The narrow point itself (file, function) and the names a function using
# rsync must reference to take its remote shell from there.
# Since M7m (OR-T10-25) the Python side names its program only as data
# (`checked_program(name)`, `shutil.which(name, ...)`): no `ssh` program
# literal stands there, so no function of lane.py is excused.
ENGSTELLE = {("scripts/lane_sync.sh", "ssh_bin")}
RSH_NAMES_PY = {"rsh", "_rsh"}
RSH_NAMES_SH = ("ssh_bin",)
_SH_SSH = re.compile(r"(?:^|[\s;&|(`\"'=])(?:ssh|scp|sftp)(?=\s|$|[\"'])")
_SH_RSYNC = re.compile(r"(?:^|[\s;&|(`])rsync(?=\s)")
_SH_FUNC = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*)\(\)\s*\{")


def _marked(lines, first, last):
    return any(re.search(r"#\s*%s\s*\S" % re.escape(MARK), lines[i - 1]) for i in range(first, last + 1))


def py_ssh_builders(path, text):
    """(path, line, function) of every ssh program literal outside the
    narrow point and every rsync literal in a function that does not take
    its remote shell from the narrow point -- unless marked."""
    tree = ast.parse(text)
    lines = text.splitlines()
    docs, parent, func_of = set(), {}, {}
    for node in ast.walk(tree):
        for child in ast.iter_child_nodes(node):
            parent[child] = node
        if isinstance(node, (ast.Module, ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)) and node.body:
            first = node.body[0]
            if isinstance(first, ast.Expr) and isinstance(first.value, ast.Constant):
                docs.add(first.value)

    def func(node):
        while node in parent:
            node = parent[node]
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda)):
                if isinstance(node, ast.Lambda):
                    continue
                return node
        return None

    def names_in(fn):
        out = set()
        for n in ast.walk(fn if fn is not None else tree):
            if isinstance(n, ast.Name):
                out.add(n.id)
            elif isinstance(n, ast.Attribute):
                out.add(n.attr)
        return out

    found = []
    for node in ast.walk(tree):
        if not (isinstance(node, ast.Constant) and isinstance(node.value, str)) or node in docs:
            continue
        val, up = node.value, parent.get(node)
        lead = isinstance(up, (ast.List, ast.Tuple)) and up.elts and up.elts[0] is node
        if isinstance(up, ast.Call) and up.args and up.args[0] is node:
            name = up.func.attr if isinstance(up.func, ast.Attribute) else getattr(up.func, "id", "")
            lead = lead or name in _CALL_ARG0
        hit = (val in SSH_PROGS and lead) or bool(_PY_PROG_STR.search(val))
        fn = func(node)
        fname = fn.name if fn is not None else "<module>"
        if not hit and val == "rsync" and lead:
            hit = not (names_in(fn) & RSH_NAMES_PY)
        if hit and (path, fname) not in ENGSTELLE and \
                not _marked(lines, node.lineno, getattr(node, "end_lineno", node.lineno)):
            found.append((path, node.lineno, fname))
    return found


def sh_ssh_builders(path, text):
    """The same rule over a shell file, line by line (comments skipped)."""
    lines = text.splitlines()
    found, fname, start = [], "<top>", 0
    bodies = {}
    for i, line in enumerate(lines):
        m = _SH_FUNC.match(line)
        if m:
            fname, start = m.group(1), i
        elif line.startswith("}"):
            bodies[(start, i)] = fname
            fname = "<top>"
    def owner(i):
        for (a, b), name in bodies.items():
            if a <= i <= b:
                return name, "\n".join(lines[a:b + 1])
        return "<top>", text
    for i, line in enumerate(lines, 1):
        code = line.split(" #", 1)[0] if not line.lstrip().startswith("#") else ""
        if not code:
            continue
        name, body = owner(i - 1)
        hit = bool(_SH_SSH.search(code))
        if not hit and _SH_RSYNC.search(code):
            hit = not any(n in body for n in RSH_NAMES_SH)
        if hit and (path, name) not in ENGSTELLE and not _marked(lines, i, i):
            found.append((path, i, name))
    return found


def production_files(root=REPO, rev=None):
    """{path: text} of the production scripts (not scripts/tests/), from the
    work tree -- tracked AND untracked, not ignored (re-check tv-bn M7h B-M4)
    -- or from `rev`. An empty set is an AssertionError: a pin that reads
    nothing is green for nothing."""
    ls = ["git", "-C", str(root), "ls-files", "-z", "--cached", "--others", "--exclude-standard"] if rev is None \
        else ["git", "-C", str(root), "ls-tree", "-r", "-z", "--name-only", rev]
    out = subprocess.run(ls + ["--", "scripts"], capture_output=True, check=True,
                         timeout=kit_time.HANG).stdout.decode("utf-8", "surrogateescape").split("\0")
    files = {}
    for path in sorted(set(out)):
        if not path or path.startswith("scripts/tests/") or not path.endswith((".py", ".sh")):
            continue
        if rev is None:
            files[path] = (pathlib.Path(root) / path).read_text(encoding="utf-8")
        else:
            files[path] = subprocess.run(["git", "-C", str(root), "show", "%s:%s" % (rev, path)],
                                         capture_output=True, text=True, check=True, timeout=kit_time.HANG).stdout
    if not files:
        raise AssertionError("no production file under %s/scripts -- the sweep reads nothing" % root)
    return files


def sweep(files):
    found = []
    for path, text in sorted(files.items()):
        found += (py_ssh_builders if path.endswith(".py") else sh_ssh_builders)(path, text)
    return found


def _src(*lines):
    """A mutant source from its lines."""
    return "\n".join(lines) + "\n"


class KitShellClass(unittest.TestCase):
    """Every kit shell file, found -- not listed (re-check tv-bn M7w B-M1/B-M2,
    OR-T12-18): a file that sources a kit library sources it from its OWN
    directory and fail-closed (`. "$<dir>/<library>" || exit 2`), and no file
    finds its own directory with an external `dirname` (off PATH it answered
    the caller's cwd: strand-gate.sh ran the cwd's gate.sh, rc 0). test-tier.sh
    sourced cargo-target.sh open: without it the lane lock was skipped and cargo
    started. `kit_tree` reads the same lines, transitively, so a test tree
    seeded from it has the libraries of the libraries."""

    @staticmethod
    def kit_shell_files():
        return kit_tree.shell_files(REPO)

    def test_every_source_line_is_own_directory_and_fail_closed(self):
        """Every `.`/`source` at a command position (after `&&`, `;`, `then`,
        `{`, ... too, re-check bundle N N-K4): the whole line
        `. "$<v>/<lib>" || exit 2`, `<v>` this file's own directory by
        builtins (`$PWD`, `$HOME`, an inherited name are not), the library
        in the repository. A kit library sourced in a snippet (a string for
        the lane host) is fail-closed the same way."""
        files = self.kit_shell_files()
        seen, excluded = [], set()
        for rel in files:
            text = (REPO / rel).read_text(encoding="utf-8")
            code, _ = kit_tree.split(text)
            for line, whole, var, lib, pos in kit_tree.source_positions(text):
                if (rel, whole) in kit_tree.NOT_SOURCES:
                    excluded.add((rel, whole))
                    continue
                with self.subTest(file=rel, line=line):
                    self.assertIsNotNone(lib, "%s:%d sources `%s`: the whole line "
                                              "`. \"$<dir>/<library>\" || exit 2`" % (rel, line, whole))
                    self.assertTrue(kit_tree.self_located(code, var, pos),
                                    "%s:%d: `$%s` is not this file's own directory there (builtin form)"
                                    % (rel, line, var))
                    self.assertTrue((REPO / "scripts" / lib).is_file(), lib)
                    seen.append((rel, lib))
            for line, cmd, ok in kit_tree.snippet_sources(text, rel):
                with self.subTest(file=rel, line=line, snippet=cmd):
                    self.assertTrue(ok, "%s:%d sources open: `%s`" % (rel, line, cmd))
                    if (rel, cmd) in kit_tree.NOT_SOURCES:
                        excluded.add((rel, cmd))
                    else:
                        seen.append((rel, "snippet"))
            with self.subTest(file=rel, check="self by tool"):
                self.assertEqual([], kit_tree.self_by_tool(text),
                                 "%s finds its own directory with dirname/readlink/realpath" % rel)
        # the named exclusions are each still there (no stale entry: a list, not a rule)
        self.assertEqual({k for k in kit_tree.NOT_SOURCES if not k[0].endswith(".py")}, excluded)
        # not vacuous: the found files and the lines of the re-check are in the scan
        for rel in ("scripts/strand-gate.sh", "scripts/gate_legacy.sh", "scripts/test-tier.sh",
                    "scripts/pipeline/coverage_host.sh", "scripts/start.sh", "scripts/publish_docs.sh"):
            self.assertIn(rel, files)
        for pair in (("scripts/test-tier.sh", "cargo-target.sh"),
                     ("scripts/gate_legacy.sh", "cargo-target.sh"),
                     ("scripts/pipeline/coverage_host.sh", "cargo-target.sh"),
                     ("scripts/cargo-target.sh", "pipeline/kit_python.sh"),
                     ("scripts/lane_sync.sh", "snippet")):
            self.assertIn(pair, seen)

    def test_the_scan_sees_every_form(self):
        """The scan itself, on the forms the re-check found green (bundle N
        M1/M2: m5, m6, m7, m7b, m8a) and on the kit's own form."""
        sh = "#!/usr/bin/env bash\n"
        own = ('case "${BASH_SOURCE[0]}" in */*) here=${BASH_SOURCE[0]%/*}/ ;; *) here=./ ;; esac\n'
               'here=$(cd -- "$here" && pwd) || exit 2\n')

        def verdict(text, rel="scripts/x.sh"):
            code, _ = kit_tree.split(text)
            bad = [w for _, w, v, lib, pos in kit_tree.source_positions(text)
                   if lib is None or not kit_tree.self_located(code, v, pos)]
            bad += [c for _, c, ok in kit_tree.snippet_sources(text, rel) if not ok]
            return bad + [w for _, w in kit_tree.self_by_tool(text)]

        cargo = "$PWD/scripts/cargo-target.sh"
        self.assertEqual([], verdict(sh + own + '. "$here/cargo-target.sh" || exit 2\n'))
        self.assertEqual([], verdict(sh + own + "# `dirname` of $0 is the caller's cwd\n"))
        self.assertEqual([], verdict(sh + own + 'msg="$(printf "%s" "can\'t")"\n'
                                     '. "$here/cargo-target.sh" || exit 2\necho \'x\' "$msg"\n'))
        self.assertEqual([], verdict(sh + "x=$(sh -c '. \"$1\" >/dev/null 2>&1; shift; exec node -e \"$@\"' bp)\n",
                                     "scripts/pipeline/lane_fingerprint.sh"))
        for form_name, form_text in (
                ("m5", sh + 'self=${BASH_SOURCE[0]}\nhere=$(cd -- "$(dirname -- "$self")" && pwd)\n'),
                ("m6", sh + 'SCRIPT=$(readlink -f "$0")\nDIR=$(dirname "$SCRIPT")\n'),
                ("m6-realpath", sh + 'me=$(realpath -- "${BASH_SOURCE[0]}")\n'),
                ("m7", sh + own + '. "$PWD/scripts/cargo-target.sh" || exit 2\n'),
                ("m7b", sh + '. "$HOME/cargo-target.sh" || exit 2\n'),
                ("inherited", sh + '. "$here/cargo-target.sh" || exit 2\n'),
                ("reassigned", sh + own + 'here=$PWD\n. "$here/cargo-target.sh" || exit 2\n'),
                ("m8a", sh + own + '[ -f "$here/cargo-target.sh" ] && . "$here/cargo-target.sh"\n'),
                ("then", sh + own + 'if true; then . "$here/cargo-target.sh" || exit 2; fi\n'),
                ("source", sh + own + 'source "$here/cargo-target.sh" || exit 2\n'),
                ("open", sh + own + '. "$here/cargo-target.sh"\n'),
                ("snippet", sh + "X='cd /t && . scripts/cargo-target.sh && cargo check'\n"),
                # re-check bundle N delta kit-shell M1-M4 (g01-g04, g06-g16, g18)
                ("g01 if", sh + own + 'if . "%s"; then :; fi\n' % cargo),
                ("g01 while", sh + own + 'while . "%s"; do break; done\n' % cargo),
                ("g02 case arm", sh + own + 'case "${1:-}" in *) . "%s" ;; esac\n' % cargo),
                ("g03 command", sh + own + 'command . "%s"\n' % cargo),
                ("g04 builtin", sh + own + 'builtin source "%s"\n' % cargo),
                ("g06 eval dq", sh + 'eval ". \\"\\$PWD/scripts/cargo-target.sh\\""\n'),
                ("g07 bash -c dq", sh + 'bash -c ". scripts/cargo-target.sh; cargo check"\n'),
                ("g08 ssh dq", sh + 'h=x\nssh "$h" ". scripts/cargo-target.sh && cargo check"\n'),
                ("g09 backtick", sh + own + 'x=`. "%s"; echo y`\necho "$x"\n' % cargo),
                ("g10 before own", sh + '. "$here/cargo-target.sh" || exit 2\n' + own),
                ("g11 read", sh + own + 'read -r here <<<"$PWD"\n. "$here/cargo-target.sh" || exit 2\n'),
                ("g11 printf -v", sh + own + 'printf -v here %s "$PWD"\n. "$here/cargo-target.sh" || exit 2\n'),
                ("g11 for", sh + own + 'for here in "$PWD"; do :; done\n. "$here/cargo-target.sh" || exit 2\n'),
                ("g11 default", sh + own + ': "${here:=$PWD}"\n. "$here/cargo-target.sh" || exit 2\n'),
                ("g11 unset", sh + own + 'unset here\n. "$here/cargo-target.sh" || exit 2\n'),
                ("g11 local", sh + 'f() {\n' + own + 'local here\n. "$here/cargo-target.sh" || exit 2\n}\n'),
                ("g12 snippet arg", sh + own + 'bash -c \'. "$1"; cargo check\' kit "$here/cargo-target.sh"\n'),
                ("g12 foreign exclusion", sh + "x=$(sh -c '. \"$1\" >/dev/null 2>&1; shift; exec node -e \"$@\"' bp)\n"),
                ("g13 default", sh + 'self=${BASH_SOURCE[0]:-$0}\nhere=$(cd -- "$(dirname -- "$self")" && pwd) || exit 2\n'),
                ("g14 trim", sh + 'self=${BASH_SOURCE[0]#./}\nhere=$(dirname -- "$self")\n'),
                ("g14 path", sh + 'lib=$0.d\nd=$(dirname "$lib")\n'),
                ("g18 dq apostrophe", sh + own + 'msg="$(printf "%%s" "can\'t")"\n'
                                                '. "%s" || exit 2\necho \'x\' "$msg"\n' % cargo),
                # rest list R-N (re-check bundle N delta2 kit M1, Restliste): the word
                # escaped against an alias, every target, a quoted or split word
                ("n-r1 backslash dot", sh + own + '\\. "%s"\n' % cargo),
                ("n-r1 backslash source", sh + own + '\\source "%s"\n' % cargo),
                ("n-r1 backslash in a snippet", sh + "X='\\. \"$1\"; cargo check'\n"),
                ("n-r2 absolute", sh + '. /etc/os-release\n'),
                ("n-r2 relative", sh + '. ./env\n'),
                ("n-r2 home", sh + 'source ~/.bashrc\n'),
                ("n-r2 bare name", sh + '. env\n'),
                ("n-r2 bare name in a snippet", sh + "X='cd /t && source env; cargo check'\n"),
                ("n-r2 quoted dot", sh + '"." "$1"\n'),
                ("n-r2 split source", sh + 'sour""ce "$1"\n'),
                ("n-r2 single-quoted dot", sh + "'.' \"$1\"\n"),
                ("n-r2 single-quoted part", sh + "so'ur'ce \"$1\"\n"),
                ("n-r2 quoted in a snippet", sh + "X='\"source\" \"$1\" || exit 2'\n")):
            with self.subTest(form=form_name):
                self.assertNotEqual([], verdict(form_text))
        # prose stays prose: a word after a sentence, a word followed by more words
        for prose in ('die "checksum mismatch for ${asset}. Refusing to install."\n',
                      'echo "the source of truth"\n'):
            with self.subTest(prose=prose):
                self.assertEqual([], verdict(sh + prose))

    def test_every_exclusion_binds_its_level_and_its_text(self):
        """A named exclusion takes its own text at its own level, nothing else
        (re-check bundle N delta FN2-d, [NS iii]): the bare command
        `<word> <target>` of an entry, at file level, in a snippet or in a
        Python string, and the entry's text in the other context (a snippet
        entry written straight into the file, a file-level entry run as a
        snippet) are judged with the list exactly as without it -- in the
        entry's own file. Bites: an open `. "$1"` at file level of
        lane_fingerprint.sh, an `f"source {x}"` in check_adr_anchors.py."""
        mark = getattr(kit_tree, "SNIPPET_MARK", None)

        def named(rel, text, use_ns):
            return use_ns and (rel, text) in kit_tree.NOT_SOURCES

        def judged(rel, text, use_ns):
            if rel.endswith(".py"):
                return [c for _, c, ok in kit_tree.python_sources(text) if not (ok or named(rel, c, use_ns))]
            code, _ = kit_tree.split(text)
            bad = [w for _, w, v, lib, pos in kit_tree.source_positions(text)
                   if not named(rel, w, use_ns) and (lib is None or not kit_tree.self_located(code, v, pos))]
            return bad + [c for _, c, ok in kit_tree.snippet_sources(text, rel if use_ns else None) if not ok]

        # the entries, as the tree takes them (the stale check pins that each is taken)
        entries = set()
        for rel in self.kit_shell_files():
            text = (REPO / rel).read_text(encoding="utf-8")
            entries |= {(rel, w) for _, w, _, _, _ in kit_tree.source_positions(text) if named(rel, w, True)}
            entries |= {(rel, c) for _, c, _ in kit_tree.snippet_sources(text, rel) if named(rel, c, True)}
        for rel in subprocess.run(["git", "-C", str(REPO), "ls-files", "-z", "scripts/*.py", "scripts/**/*.py"],
                                  capture_output=True, check=True).stdout.decode().split("\0"):
            if rel and not rel.startswith("scripts/tests/"):
                text = (REPO / rel).read_text(encoding="utf-8")
                entries |= {(rel, c) for _, c, _ in kit_tree.python_sources(text) if named(rel, c, True)}
        self.assertTrue(entries)

        def added(rel, add, ns):
            base = (REPO / rel).read_text(encoding="utf-8")
            base = base if base.endswith("\n") else base + "\n"
            return collections.Counter(judged(rel, base + add, ns)) - collections.Counter(judged(rel, base, ns))

        bites = [("scripts/pipeline/lane_fingerprint.sh", '. "$1" >/dev/null 2>&1; shift; exec node -e "$@"\n'),
                 ("scripts/check_adr_anchors.py", 'x = 1\nPROBE = f"source {x}"\n')]
        probes = list(bites)
        for rel, text in sorted(entries):
            is_py = rel.endswith(".py")
            for _, m in kit_tree.source_hits(text, lambda t, a="", b="": kit_tree._is_path(t, a, b) or (
                    is_py and kit_tree.PY_PLACEHOLDER.search(t) is not None)):
                bare = "%s %s" % (m.group("cmd"), m.group("target"))
                probes.append((rel, "PROBE = %r\n" % bare if is_py else bare + "\n"))
                if not is_py and "'" not in bare:
                    probes.append((rel, "x=$(sh -c '%s' bp)\n" % bare))
            if is_py:
                continue
            if mark and text.endswith(mark):
                probes.append((rel, text[:-len(mark)] + "\n"))
            elif "'" not in text:
                probes.append((rel, "x=$(sh -c '%s' bp)\n" % text))
        for rel, add in probes:
            with self.subTest(file=rel, probe=add):
                raw = added(rel, add, False)
                if (rel, add) in bites:
                    self.assertTrue(raw, "%s: `%s` is no source to the scan" % (rel, add.strip()))
                self.assertEqual(raw, added(rel, add, True),
                                 "%s: `%s` passes as a named exclusion" % (rel, add.strip()))

    def test_the_kit_python_sources_fail_closed(self):
        """The kit's Python side sources the same libraries in `bash -c`
        strings (executor.test_features, the tree sync, gate_plan, pre): the
        same fail-closed form, `.` or `source`, any quoting -- an unreadable
        library is exit 2, never a half-defined shell that answers anyway.
        Each known place is expected by itself (N-K4 c), no count."""
        known = collections.Counter({
            ("scripts/gate_plan.py", '. "$1" || exit 2'): 1,
            ("scripts/pipeline/executor.py", '. "$1" || exit 2'): 1,
            ("scripts/pipeline/executor.py", '. "$1/scripts/cargo-target.sh" || exit 2'): 1,
            ("scripts/pipeline/pre.py", '. "$1" || exit 2'): 1,
        })
        # the scan itself (re-check bundle N delta kit-shell M3: g15, g16, m11c)
        for py_name, src in (("g15 percent", 'CMD = "source %s; cargo check" % "x"\n'),
                             ("g15 named", 'CMD = ". %(lib)s; true" % {"lib": "x"}\n'),
                             ("g15 format", 'CMD = "source {}; true".format("x")\n'),
                             ("g16 f-string", 'lib = "x"\nCMD = f". {lib}; cargo check"\n'),
                             ("open", 'CMD = \'. "$1"; true\'\n'),
                             # rest list R-N: the escaped word, every target, a quoted or split word
                             ("n-r1 backslash", 'CMD = "\\\\. %s; true" % "x"\n'),
                             ("n-r1 backslash source", 'CMD = "\\\\source $X; true"\n'),
                             ("n-r2 absolute", 'CMD = ". /etc/os-release; true"\n'),
                             ("n-r2 home", 'CMD = "source ~/.bashrc; true"\n'),
                             ("n-r2 bare name", 'CMD = "cd /t && . env; true"\n'),
                             ("n-r2 quoted dot", 'CMD = \'"." "$1"; true\'\n'),
                             ("n-r2 split source", 'CMD = \'sour""ce "$1"; true\'\n')):
            with self.subTest(py=py_name):
                self.assertEqual([False], [ok for _, _, ok in kit_tree.python_sources(src)])
        self.assertEqual([True], [ok for _, _, ok in kit_tree.python_sources('CMD = f\'. "{x}" || exit 2; t\'\n')])
        found, excluded = collections.Counter(), set()
        for rel in subprocess.run(["git", "-C", str(REPO), "ls-files", "-z", "scripts/*.py",
                                   "scripts/**/*.py"], capture_output=True, check=True
                                  ).stdout.decode().split("\0"):
            if not rel or rel.startswith("scripts/tests/"):
                continue
            for line, cmd, ok in kit_tree.python_sources((REPO / rel).read_text(encoding="utf-8")):
                if (rel, cmd) in kit_tree.NOT_SOURCES:
                    excluded.add((rel, cmd))
                    continue
                with self.subTest(file=rel, line=line):
                    self.assertTrue(ok, "%s:%d sources open: `%s`" % (rel, line, cmd))
                found[(rel, kit_tree.SNIPPET_SOURCE.match(cmd).group(0) if ok else cmd)] += 1
        self.assertEqual(known, found)
        self.assertEqual({k for k in kit_tree.NOT_SOURCES if k[0].endswith(".py")}, excluded)

    def test_the_seed_is_transitive(self):
        """The libraries of the libraries: every kit shell file's seed closes
        over its own source lines (gate_legacy.sh -> cargo-target.sh ->
        pipeline/kit_python.sh)."""
        for rel in self.kit_shell_files():
            own = rel[len("scripts/"):]
            libs = kit_tree.closure(own)
            for lib in libs:
                with self.subTest(file=rel, lib=lib):
                    self.assertLessEqual(set(kit_tree.sourced(lib)), set(libs) | {own})
        self.assertIn("pipeline/kit_python.sh", kit_tree.closure("gate_legacy.sh"))

    def test_the_lane_check_without_its_library_stops_before_cargo(self):
        """lane_sync.sh's CHECK_ON_HOST in a tree without cargo-target.sh:
        exit 2, the cargo after it never starts (it sourced the library open
        and cwd-relative, re-check bundle N N-K4)."""
        text = pathlib.Path(lane.LANE_SYNC).read_text(encoding="utf-8")
        script = re.search(r"(?m)^CHECK_ON_HOST='([^']*)'", text).group(1)
        with tempfile.TemporaryDirectory() as tmp:
            res = subprocess.run(["/bin/bash", "-p", "-c", script + " && echo CARGO-RAN"], cwd=tmp,
                                 env={"PATH": store_isolation.MINIMAL_PATH, "HOME": tmp},
                                 capture_output=True, text=True, timeout=kit_time.HANG)
        self.assertEqual(2, res.returncode, res.stderr)
        self.assertNotIn("CARGO-RAN", res.stdout)

    def test_test_tier_without_its_target_library_stops_before_cargo(self):
        """test-tier.sh in a tree without cargo-target.sh: exit 2, no cargo
        (the lane lock lives behind that library)."""
        with tempfile.TemporaryDirectory() as tmp:
            scripts = pathlib.Path(tmp) / "scripts"
            scripts.mkdir()
            shutil.copy(REPO / "scripts" / "test-tier.sh", scripts / "test-tier.sh")
            kit_tree.seed(scripts, "test-tier.sh")
            (scripts / "cargo-target.sh").unlink(missing_ok=True)
            fake = pathlib.Path(tmp) / "bin"
            fake.mkdir()
            mark = pathlib.Path(tmp) / "cargo-ran"
            for tool in ("cargo", "cargo-nextest", "flock"):
                (fake / tool).write_text("#!/bin/sh\n: > %s\nexit 0\n" % shlex.quote(str(mark)))
                (fake / tool).chmod(0o755)
            env = {"PATH": "%s:%s" % (fake, store_isolation.trapped_path()), "HOME": tmp}
            res = subprocess.run(["/bin/bash", str(scripts / "test-tier.sh"), "t0"], cwd=tmp,
                                 env=env, capture_output=True, text=True,
                                 timeout=2 * kit_time.HANG)
            self.assertEqual(2, res.returncode, res.stderr)
            self.assertFalse(mark.exists())


class ProductionSweep(unittest.TestCase):
    """An early warning beside the run-time check of `ssh_program`/`ssh_bin`
    (orchestrator OR-T10-2), not the guarantee: the forms it misses
    (re-check tv-bn M7h B-M3) meet the narrow point's check of the program."""

    def test_no_production_code_builds_an_ssh_way_outside_the_narrow_point(self):
        self.assertEqual([], sweep(production_files()))

    def test_the_sweep_reads_untracked_files_and_paths_with_blanks(self):
        """Re-check tv-bn M7h B-M4: `git ls-files` without `--others` missed a
        new production file not yet added, `.split()` cut a path with a blank."""
        with tempfile.TemporaryDirectory() as d:
            root = pathlib.Path(d)
            (root / "scripts").mkdir()
            (root / "scripts" / "tracked.py").write_text("x = 1\n")
            subprocess.run(["git", "-C", d, "init", "-q"], check=True, capture_output=True, timeout=kit_time.HANG)
            subprocess.run(["git", "-C", d, "add", "-A"], check=True, capture_output=True, timeout=kit_time.HANG)
            (root / "scripts" / "new way.py").write_text(_src("import subprocess",
                                                              "def f(h):",
                                                              "    subprocess.run([PROG, h])").replace(
                                                                  "PROG", repr("s" + "sh")))
            files = production_files(root)
            self.assertIn("scripts/new way.py", files)
            self.assertEqual(["scripts/new way.py"], sorted({f[0] for f in sweep(files)}))

    def test_an_empty_scan_set_is_red(self):
        with tempfile.TemporaryDirectory() as d:
            subprocess.run(["git", "-C", d, "init", "-q"], check=True, capture_output=True, timeout=kit_time.HANG)
            with self.assertRaisesRegex(AssertionError, "no production file"):
                production_files(pathlib.Path(d))

    def test_without_the_narrow_point_the_sweep_finds_exactly_its_two_functions(self):
        """Positive control on the real tree: the scan set is the one the pin
        reads, and the sweep sees what it must -- the two narrow points."""
        prod_texts = production_files()
        with unittest.mock.patch.object(sys.modules[__name__], "ENGSTELLE", set()):
            found = {(f[0], f[2]) for f in sweep(prod_texts)}
        self.assertEqual({("scripts/lane_sync.sh", "ssh_bin")}, found)
        # lane.py is read, and an own ssh way in its narrow point is seen
        lane_text = prod_texts["scripts/pipeline/lane.py"]
        self.assertEqual([], py_ssh_builders("scripts/pipeline/lane.py", lane_text))
        mutant = lane_text.replace("    return checked_program(",
                                   '    subprocess.run(["ssh", "h", "true"]); return checked_program(', 1)
        self.assertNotEqual(lane_text, mutant)
        self.assertEqual(["ssh_program"], [f[2] for f in py_ssh_builders("scripts/pipeline/lane.py", mutant)])

    def test_the_sweep_sees_every_form_of_an_own_ssh_way(self):
        py_red = {
            "argv list": _src('import subprocess', 'def f(t):', '    subprocess.run(["ssh", t, "true"])'),
            "tuple": 'def f(t):\n    return ("scp", "a", t)\n',
            "f-string": 'import os\ndef f(t):\n    os.system(f"ssh {t} true")\n',
            "program string": 'def f():\n    return "ssh -o BatchMode=yes"\n',
            "mid-string": 'def f():\n    return "cd /t; ssh h true"\n',
            "rsync -e": 'def f():\n    return "rsync -e ssh a h:b"\n',
            "exec": _src('import os', 'def f():', '    os.execvp("ssh", ["ssh"])'),
            "which": _src('import shutil', 'def f():', '    return shutil.which("ssh")'),
            "module level": _src('SSH = ["ssh", "-o", "BatchMode=yes"]'),
            "rsync without rsh": 'def f(t):\n    return ["rsync", "-a", "x", t + ":y"]\n',
        }
        for label, src in py_red.items():
            with self.subTest(py=label):
                self.assertTrue(py_ssh_builders("scripts/x.py", src), label)
                marked = "\n".join(l + "  # %s test reason" % MARK if ("ssh" in l or "scp" in l or "rsync" in l)
                                   and not l.startswith(("import", "def")) else l for l in src.splitlines())
                self.assertEqual([], py_ssh_builders("scripts/x.py", marked), label)
        py_green = {
            "transport word": _src('def f(l):', '    return l.transport == "ssh"'),
            "key": _src('def f(i):', '    return i.get("ssh") or i["ssh"]'),
            "docstring": 'def f():\n    """ssh -o x h true"""\n',
            "message": 'def f():\n    raise OSError("lost (ssh exit 255)")\n',
            "rsync through rsh": 'def _rsh(t):\n    return []\ndef f(t):\n    return ["rsync"] + _rsh(t)\n',
        }
        for label, src in py_green.items():
            with self.subTest(py_green=label):
                self.assertEqual([], py_ssh_builders("scripts/x.py", src), label)
        sh_red = {
            "ssh": 'f() {\n    ssh -o BatchMode=yes "$1" true\n}\n',
            "pipe": 'f() {\n    echo x | ssh h cat\n}\n',
            "rsync -e": 'f() {\n    rsync -e "ssh -o x" a h:b\n}\n',
            "git": 'f() {\n    GIT_SSH_COMMAND="ssh -o x" git push h:x\n}\n',
            "scp": 'scp a h:b\n',
            "rsync without ssh_bin": 'f() {\n    rsync -a a h:b\n}\n',
        }
        for label, src in sh_red.items():
            with self.subTest(sh=label):
                self.assertTrue(sh_ssh_builders("scripts/x.sh", src), label)
                marked = "\n".join(l + "  # %s test reason" % MARK if ("ssh" in l or "scp" in l or "rsync" in l)
                                   else l for l in src.splitlines())
                self.assertEqual([], sh_ssh_builders("scripts/x.sh", marked), label)
        self.assertEqual([], sh_ssh_builders("scripts/x.sh", 'f() {\n    local ssh; ssh=$(ssh_bin) || exit 2\n'
                                                              '    "$ssh" h true\n    rsync -a a b\n}\n'))
        self.assertEqual([], sh_ssh_builders("scripts/x.sh", '# ssh h true\n'))

    def test_a_marker_needs_a_reason(self):
        src = _src('def f(t):', '    return ["ssh", t]  # ' + MARK)
        self.assertTrue(py_ssh_builders("scripts/x.py", src))


class TestPythonForm(unittest.TestCase):
    """Re-check tv-bn M7p M1: the one reader of a command's Python form."""

    def test_the_words_after_the_interpreter(self):
        _lane = lane
        self.assertEqual(["s.py", "unittest"], _lane.python_rest(_lane.python_argv("s.py", "unittest")))
        self.assertEqual(["-m", "unittest", "m"], _lane.python_rest(["python3", "-m", "unittest", "m"]))
        self.assertEqual(["-m", "x"], _lane.python_rest(["/usr/bin/python3", "-I", "-X", "dev", "-u", "-m", "x"]))
        self.assertEqual(["s.py"], _lane.python_rest(["{python}", "s.py"]))
        self.assertIsNone(_lane.python_rest(["scripts/test-tier.sh", "filter", "x"]))
        self.assertIsNone(_lane.python_rest([]))

    def test_an_env_prefix_is_read_past(self):
        """Re-check tv-bn M7q H1: the `env NAME=value` prefix of a satellite cmd
        (register.py, `env SOUL2ME_LAB_SLOTS={slots} {python} ...`) is read in the
        one reader, not only in register's own `_program`."""
        self.assertEqual(["-m", "unittest"], lane.python_rest(["env", "A=1", "python3", "-m", "unittest"]))
        self.assertEqual(["s.py"], lane.python_rest(["/usr/bin/env", "A=1", "B=", "{python}", "s.py"]))
        self.assertIsNone(lane.python_rest(["env", "A=1", "cargo", "test"]))
        self.assertEqual("cargo", lane.program(["env", "A=1", "cargo", "test"]))
        self.assertEqual("cargo", lane.program(["cargo", "test"]))
        self.assertEqual("", lane.program([]))

    def test_combined_short_options_keep_m_and_c(self):
        """Re-check tv-bn M7q H2: `-Im x` is `-I -m x`, as the interpreter reads it."""
        cases = ((["python3", "-Im", "unittest", "x"], ["-m", "unittest", "x"]),
                 (["python3", "-Imunittest", "x"], ["-m", "unittest", "x"]),
                 (["python3", "-mx"], ["-m", "x"]),
                 (["python3", "-Ic", "print(1)"], ["-c", "print(1)"]),
                 (["python3", "-IXdev", "s.py"], ["s.py"]),
                 (["python3", "-IX", "dev", "s.py"], ["s.py"]),
                 (["python3", "-W", "error", "-uI", "s.py"], ["s.py"]),
                 (["python3", "-I", "-", "a"], ["-", "a"]))
        for argv, want in cases:
            with self.subTest(argv=argv):
                self.assertEqual(want, lane.python_rest(argv))

    def test_every_reader_asks_the_one_reader(self):
        """The station readers name `lane.python_rest`/`is_python_program`, no
        own `startswith("python")` (frühwarnung; the behaviour pins are the real
        station in test_pipeline_selftest_shard)."""
        import re
        for rel in ("pipeline/flakes.py", "pipeline/plan.py", "pipeline/register.py"):
            text = (REPO / "scripts" / rel).read_text(encoding="utf-8")
            self.assertNotRegex(text, re.compile(r"""\.startswith\(\s*["']python"""), rel)


class TestOneSourceNeighbours(unittest.TestCase):
    """Re-check tv-bn M7q, OR-T11-22 "one fact, one source": the neighbour classes
    of M1/M2 the kit derived more than once."""

    def test_the_own_lane_name_has_one_rule(self):
        """The executor took the full host name, pool and hygiene the short one."""
        from scripts.pipeline import pool
        with tempfile.TemporaryDirectory() as root:
            for env, want in (({}, "h"), ({"MECLAW_LANE_NAME": "build02"}, "build02")):
                with self.subTest(env=env), \
                        unittest.mock.patch.dict(os.environ, env, clear=False), \
                        unittest.mock.patch("socket.gethostname", return_value="h.lan.example.org"):
                    if not env:
                        os.environ.pop("MECLAW_LANE_NAME", None)
                    self.assertEqual(want, lane.own_name())
                    self.assertEqual("x", lane.own_name("x"))
                    self.assertEqual(want, pool._runner(None))
                    self.assertEqual(want, hygiene.Ctx(root=root).runner)   # the mirror
        # no other reader derives it (re-check tv-bn M7r B-M3: backfill did): only the
        # rule itself, its mirror hygiene (stdin, no kit), and the schedule, whose
        # `driver_host` is the full host of the driver -- another fact
        readers = sorted(p.name for p in (REPO / "scripts" / "pipeline").glob("*.py")
                         if "gethostname" in p.read_text(encoding="utf-8"))
        self.assertEqual(["hygiene.py", "lane.py", "schedule.py"], readers)

    def test_the_volatile_fingerprint_fields_have_one_source(self):
        """Re-check tv-bn M7r A-H3: retro kept its own copy of the measures."""
        from scripts.pipeline import executor, retro
        self.assertIs(executor.FINGERPRINT_MEASURES, retro.VOLATILE_FINGERPRINT)

    def test_the_ruling_variable_has_one_name(self):
        from scripts import testing_bans
        from scripts.pipeline import gate
        self.assertEqual(testing_bans.RULING_ENV, gate.TESTING_RULING_ENV)
        text = (REPO / "scripts" / "lane_sync.sh").read_text(encoding="utf-8")
        verdict_env = re.search(r"^VERDICT_ENV=\(([^)]*)\)", text, re.M).group(1).split()
        self.assertIn(testing_bans.RULING_ENV, verdict_env)


if __name__ == "__main__":
    unittest.main()

