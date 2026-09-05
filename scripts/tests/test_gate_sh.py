"""Tests for the bash runner `scripts/gate.sh`.

The runner is exercised against a throw-away git repository, never against the
real tree: every station in these tests is a FAKE, planted through the two
environment switches that are part of the runner's interface.

    MECLAW_GATE_PLAN=<file.tsv>   use this ready-made plan instead of calling
                                  the resolver (`scripts/gate_plan.py`)
    MECLAW_GATE_DRY=1             log every command, run none of them

What is pinned here is the CONTRACT, not the station catalogue (that belongs
to `test_gate_plan.py`): the gate line format, the summary line, the receipt
JSON, `--only`, `--plan-only`, and the keep-going / `--fail-fast` split.
"""

import fcntl
import json
import os
import pathlib
import re
import shutil
import subprocess
import tempfile
import time
import unittest

REPO = pathlib.Path(__file__).resolve().parents[2]
GATE_SH = REPO / "scripts" / "gate.sh"
# The runner asks the resolver which run artefacts do not make a tree dirty
# (`--print ignored`), so the throw-away repo needs a copy of it.
GATE_PLAN = REPO / "scripts" / "gate_plan.py"

# name<TAB>scope<TAB>cargo<TAB>shell-quoted argv<TAB>cwd
PLAN_OK_BAD = (
    "ok\tscope-ok\t0\ttrue\t\n"
    "bad\tscope-bad\t0\tfalse\t\n"
    "after\tscope-after\t0\ttrue\t\n"
)
PLAN_WITH_TESTS = (
    "ok\tscope-ok\t0\ttrue\t\n"
    "tests\tall()\t1\tscripts/test-tier.sh filter 'all()'\t\n"
)
PLAN_RECEIPT = (
    "export-audit\tR1-R17\t0\techo '{receipt}'\t\n"
)
# `cat` with nothing on stdin: without </dev/null it reads the command loop's
# here-string and swallows the second command of its own station.
PLAN_STDIN_EATER = (
    "eat\tstdin\t0\tcat\t\n"
    "eat\tstdin\t0\techo second-command-ran\t\n"
)
# The same station name, but not on adjacent rows.
PLAN_SPLIT_STATION = (
    "corpus\tregenerate+check\t0\ttrue\t\n"
    "other\tscope\t0\ttrue\t\n"
    "corpus\tregenerate+check\t0\ttrue\t\n"
)
PLAN_CARGO = (
    "build\tworkspace\t1\ttrue\t\n"
)
# Probe the run-wide cargo lock from a station: `flock -n` exits 1 when the
# lock is already held (by this run) and 0 when it is free. The stations
# around the cargo one answer "is the lock held right now".
_PROBE = ("bash -c 'flock -n \"$MECLAW_GATE_LOCK\" true; echo held=$?'")
PLAN_LOCK_WINDOW = (
    "early\tbefore-cargo\t0\t" + _PROBE + "\t\n"
    "build\tworkspace\t1\ttrue\t\n"
    "late\tafter-cargo\t0\t" + _PROBE + "\t\n"
)
PLAN_TWO_COMMANDS = (
    "corpus\tregenerate+check\t0\ttrue\t\n"
    "corpus\tregenerate+check\t0\ttrue\t\n"
)

GATE_LINE = re.compile(
    r"^GATE (?P<name>\S+) \[(?P<scope>.*)\] (?P<secs>\d+)s "
    r"(?P<verdict>GREEN|RED|SKIP|NOTE)(?: (?P<reason>.*))?$")
SUMMARY_LINE = re.compile(
    r"^GATE-SUMMARY (?P<mode>\S+) (?P<rev>\S+) (?P<green>\d+)/(?P<total>\d+) "
    r"(?P<secs>\d+)s (?P<verdict>GREEN|RED)$")


def _git(repo, *args):
    env = dict(os.environ)
    env.update({
        "GIT_AUTHOR_NAME": "gate test", "GIT_AUTHOR_EMAIL": "gate@example.invalid",
        "GIT_COMMITTER_NAME": "gate test", "GIT_COMMITTER_EMAIL": "gate@example.invalid",
        "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_SYSTEM": os.devnull,
    })
    return subprocess.run(["git", "-C", str(repo)] + list(args),
                          env=env, check=True, capture_output=True, text=True)


def make_repo(root):
    """A two-commit repo with `master` on HEAD and a copy of gate.sh in it.

    It lives in a SUBDIRECTORY of the temp dir: the plan fixtures and the
    --log-dir target must stay outside the repo, or they would show up as
    untracked files and the runner would rightly call the tree dirty.
    """
    repo = pathlib.Path(root) / "repo"
    (repo / "scripts").mkdir(parents=True, exist_ok=True)
    shutil.copy(GATE_SH, repo / "scripts" / "gate.sh")
    (repo / "scripts" / "gate.sh").chmod(0o755)
    shutil.copy(GATE_PLAN, repo / "scripts" / "gate_plan.py")
    (repo / "README.md").write_text("first\n")
    _git(repo, "init", "-q")
    _git(repo, "add", "-A")
    _git(repo, "commit", "-q", "-m", "first")
    (repo / "README.md").write_text("second\n")
    _git(repo, "add", "-A")
    _git(repo, "commit", "-q", "-m", "second")
    # `git init` may hand us `main`; the runner's strand base is `master`.
    _git(repo, "branch", "-M", "master")
    return repo


def run_gate(repo, *args, plan=None, dry=True, extra_env=None, timeout_s=None):
    env = dict(os.environ)
    env.pop("CI", None)
    if plan is not None:
        env["MECLAW_GATE_PLAN"] = str(plan)
    if dry:
        env["MECLAW_GATE_DRY"] = "1"
    else:
        env.pop("MECLAW_GATE_DRY", None)
    # The build-width cap has an "already set wins" rule; a value inherited
    # from the surrounding shell would make these tests say nothing.
    env.pop("CARGO_BUILD_JOBS", None)
    # ... and neither may a target directory inherited from the shell decide
    # where the receipt lands: TestTargetDirectory sets it deliberately.
    env.pop("CARGO_TARGET_DIR", None)
    # Never the real cargo lock: a build running on this host would block the
    # test suite for as long as it takes.
    env.setdefault("MECLAW_GATE_LOCK", str(repo.parent / "cargo.lock.test"))
    # A full disk must not turn the self-test station red over a build that
    # these fake stations never run.
    env.setdefault("MECLAW_GATE_MIN_FREE_G", "0")
    env.update({
        "GIT_AUTHOR_NAME": "gate test", "GIT_AUTHOR_EMAIL": "gate@example.invalid",
        "GIT_COMMITTER_NAME": "gate test", "GIT_COMMITTER_EMAIL": "gate@example.invalid",
        "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_SYSTEM": os.devnull,
    })
    if extra_env:
        env.update(extra_env)
    return subprocess.run([str(repo / "scripts" / "gate.sh")] + list(args),
                          cwd=str(repo), env=env, capture_output=True, text=True,
                          timeout=timeout_s)


def gate_lines(out):
    return [m.groupdict() for m in
            (GATE_LINE.match(ln) for ln in out.splitlines()) if m]


class GateShTestCase(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.repo = make_repo(self._tmp.name)

    def plan_file(self, text, name="plan.tsv"):
        path = pathlib.Path(self._tmp.name) / name
        path.write_text(text)
        return path

    def gate_dir(self, tree=None):
        """`<target>/gate/<tree>` -- the runner namespaces its artefacts per tree.

        `<tree>` is the basename of the worktree the run was started from, so
        two worktrees sharing one target directory do not write the same files.
        """
        return self.repo / "target" / "gate" / (tree or self.repo.name)

    def station_log(self, station, mode="strand", tree=None):
        return self.gate_dir(tree) / "logs" / ("%s-%s.log" % (mode, station))

    def last_receipt(self, mode="strand", tree=None):
        return json.loads(
            (self.gate_dir(tree) / ("last-%s.json" % mode)).read_text())


class TestPlanOnly(GateShTestCase):
    def test_plan_only_lists_stations_and_ends_with_tests_false(self):
        res = run_gate(self.repo, "strand", "--plan-only",
                       plan=self.plan_file(PLAN_OK_BAD))
        self.assertEqual(0, res.returncode, res.stderr)
        lines = res.stdout.strip().splitlines()
        self.assertEqual("tests=false", lines[-1])
        for name in ("ok", "bad", "after"):
            self.assertTrue(any(name in ln for ln in lines[:-1]),
                            "%s missing from plan output: %r" % (name, res.stdout))
        # Nothing ran: no receipt, no logs.
        self.assertFalse((self.repo / "target" / "gate").exists())

    def test_plan_only_reports_tests_true_when_the_tests_station_is_planned(self):
        res = run_gate(self.repo, "strand", "--plan-only",
                       plan=self.plan_file(PLAN_WITH_TESTS))
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual("tests=true", res.stdout.strip().splitlines()[-1])

    def test_plan_only_honours_only(self):
        res = run_gate(self.repo, "strand", "--plan-only", "--only", "ok",
                       plan=self.plan_file(PLAN_OK_BAD))
        self.assertEqual(0, res.returncode, res.stderr)
        body = [ln for ln in res.stdout.strip().splitlines()[:-1]
                if not ln.startswith("gate: ")]
        self.assertTrue(any("ok" in ln for ln in body))
        self.assertFalse(any("bad" in ln for ln in body))


class TestGateLines(GateShTestCase):
    def test_gate_line_format(self):
        res = run_gate(self.repo, "strand", plan=self.plan_file(PLAN_OK_BAD))
        self.assertEqual(0, res.returncode, res.stderr + res.stdout)
        rows = gate_lines(res.stdout)
        self.assertEqual(["ok", "bad", "after"], [r["name"] for r in rows])
        self.assertEqual(["GREEN"] * 3, [r["verdict"] for r in rows])
        self.assertEqual("scope-ok", rows[0]["scope"])

    def test_summary_line_and_exit_code_are_green(self):
        res = run_gate(self.repo, "strand", plan=self.plan_file(PLAN_OK_BAD))
        summaries = [m for m in
                     (SUMMARY_LINE.match(ln) for ln in res.stdout.splitlines()) if m]
        self.assertEqual(1, len(summaries), res.stdout)
        got = summaries[0].groupdict()
        self.assertEqual("strand", got["mode"])
        self.assertEqual("GREEN", got["verdict"])
        self.assertEqual("3", got["green"])
        self.assertEqual("3", got["total"])
        self.assertEqual(0, res.returncode)

    def test_a_station_with_two_commands_yields_one_gate_line(self):
        res = run_gate(self.repo, "strand", plan=self.plan_file(PLAN_TWO_COMMANDS))
        rows = gate_lines(res.stdout)
        self.assertEqual(["corpus"], [r["name"] for r in rows])

    def test_dry_mode_writes_a_log_per_station(self):
        run_gate(self.repo, "strand", plan=self.plan_file(PLAN_OK_BAD))
        log = self.station_log("ok")
        self.assertTrue(log.exists(), "no log at %s" % log)
        self.assertIn("true", log.read_text())


class TestBuildWidth(GateShTestCase):
    """A colony sharing this host is starved by a full-width cold build."""

    def log_of_build_station(self):
        return self.station_log("build").read_text()

    def test_cargo_station_builds_at_half_width(self):
        res = run_gate(self.repo, "strand", plan=self.plan_file(PLAN_CARGO))
        self.assertEqual(0, res.returncode, res.stderr)
        log = self.log_of_build_station()
        found = re.search(r"CARGO_BUILD_JOBS=(\d+)", log)
        self.assertIsNotNone(found, "no build width in the station log: %r" % log)
        self.assertEqual(max(1, (os.cpu_count() or 2) // 2), int(found.group(1)))

    def test_a_preset_build_width_wins(self):
        run_gate(self.repo, "strand", plan=self.plan_file(PLAN_CARGO),
                 extra_env={"CARGO_BUILD_JOBS": "3"})
        self.assertIn("CARGO_BUILD_JOBS=3", self.log_of_build_station())

    def test_no_nice_leaves_the_build_width_alone(self):
        run_gate(self.repo, "strand", "--no-nice", plan=self.plan_file(PLAN_CARGO))
        self.assertIn("CARGO_BUILD_JOBS=<unset>", self.log_of_build_station())

    def test_ci_mode_caps_nothing(self):
        run_gate(self.repo, "ci", "--base", "0" * 40,
                 plan=self.plan_file(PLAN_CARGO))
        log = self.station_log("build", mode="ci").read_text()
        self.assertNotIn("cargo hygiene", log)


class TestReceiptPlaceholder(GateShTestCase):
    """`{receipt}` in a station argv is the runner's job to fill in."""

    def test_receipt_placeholder_is_substituted(self):
        res = run_gate(self.repo, "strand", plan=self.plan_file(PLAN_RECEIPT),
                       dry=False)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        rev = _git(self.repo, "rev-parse", "HEAD").stdout.strip()
        want = str(self.gate_dir() / ("strand-%s.json" % rev))
        log = self.station_log("export-audit").read_text()
        self.assertIn(want, log.splitlines()[-1],
                      "placeholder not substituted: %r" % log)
        # And that path is a receipt that actually exists.
        self.assertTrue(pathlib.Path(want).exists())
        self.assertEqual("strand", json.loads(pathlib.Path(want).read_text())["mode"])


class TestStationStdin(GateShTestCase):
    def test_a_station_command_cannot_eat_the_next_one(self):
        res = run_gate(self.repo, "strand", plan=self.plan_file(PLAN_STDIN_EATER),
                       dry=False)
        log = self.station_log("eat").read_text()
        self.assertIn("$ echo second-command-ran", log,
                      "the first command swallowed the second: %r" % log)
        self.assertIn("second-command-ran\n", log)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)


class TestFolding(GateShTestCase):
    def test_rows_are_folded_by_name_not_only_when_adjacent(self):
        res = run_gate(self.repo, "strand", plan=self.plan_file(PLAN_SPLIT_STATION))
        self.assertEqual(["corpus", "other"], [r["name"] for r in gate_lines(res.stdout)])


class TestReceipt(GateShTestCase):
    def test_receipt_shape(self):
        res = run_gate(self.repo, "strand", plan=self.plan_file(PLAN_OK_BAD))
        rev = _git(self.repo, "rev-parse", "HEAD").stdout.strip()
        receipt = self.gate_dir() / ("strand-%s.json" % rev)
        self.assertTrue(receipt.exists(), res.stdout + res.stderr)
        doc = json.loads(receipt.read_text())
        self.assertEqual(
            {"mode", "rev", "base", "dirty", "lock_wait_secs", "started",
             "finished", "stations", "verdict"},
            set(doc))
        self.assertEqual("strand", doc["mode"])
        self.assertEqual(rev, doc["rev"])
        self.assertEqual("GREEN", doc["verdict"])
        self.assertFalse(doc["dirty"])
        self.assertEqual(["ok", "bad", "after"], [s["name"] for s in doc["stations"]])
        for st in doc["stations"]:
            self.assertEqual({"name", "scope", "secs", "verdict", "log"}, set(st))
            self.assertIsInstance(st["secs"], int)
        self.assertEqual(doc, self.last_receipt())

    def test_finished_is_null_until_the_run_ends(self):
        # Read the receipt FROM INSIDE the run: `first` finishes, the runner
        # rewrites the receipt, `second` prints it. Only the final write may
        # stamp `finished` -- a receipt rewritten after every station would
        # otherwise claim the run ended several times over.
        plan = ("first\tscope\t0\ttrue\t\n"
                "second\tscope\t0\tcat '{receipt}'\t\n")
        res = run_gate(self.repo, "strand", plan=self.plan_file(plan), dry=False)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        mid = json.loads(
            self.station_log("second").read_text().split("\n", 1)[1])
        self.assertEqual(["first"], [s["name"] for s in mid["stations"]])
        self.assertIsNone(mid["finished"], "an unfinished run claimed a finish time")
        # The final write does stamp it.
        doc = self.last_receipt()
        self.assertIsInstance(doc["finished"], str)
        self.assertTrue(doc["finished"])

    def test_dirty_tree_is_recorded(self):
        (self.repo / "README.md").write_text("modified\n")
        run_gate(self.repo, "strand", plan=self.plan_file(PLAN_OK_BAD))
        doc = self.last_receipt()
        self.assertTrue(doc["dirty"])


class TestReceiptDirty(GateShTestCase):
    """`dirty` answers "would an export carry something uncommitted".

    Two subtractions, both measured on the release gate of 2026-09-04: the
    scenario stations rewrite their own `last_run.json` while the run is in
    progress, so a release ended `dirty: true` on its own artefacts and
    `make_export.py` refused. Untracked files never counted for an export
    either -- it takes tracked blobs only.
    """

    def commit_run_artefact(self):
        art = self.repo / "workshop" / "evals" / "scenarios" / "last_run.json"
        art.parent.mkdir(parents=True, exist_ok=True)
        art.write_text('{"runs": 1}\n')
        _git(self.repo, "add", "-A")
        _git(self.repo, "commit", "-q", "-m", "run artefact")
        return art

    def dirty_flag(self):
        run_gate(self.repo, "strand", plan=self.plan_file(PLAN_OK_BAD))
        return self.last_receipt()["dirty"]

    def test_a_rewritten_run_artefact_does_not_make_the_tree_dirty(self):
        art = self.commit_run_artefact()
        art.write_text('{"runs": 2}\n')
        self.assertFalse(self.dirty_flag(),
                         "a station rewriting its own run artefact blocked the "
                         "export")

    def test_a_second_modified_file_does(self):
        art = self.commit_run_artefact()
        art.write_text('{"runs": 2}\n')
        (self.repo / "README.md").write_text("third\n")
        self.assertTrue(self.dirty_flag())

    def test_untracked_files_do_not_make_the_tree_dirty(self):
        (self.repo / "untracked.txt").write_text("x\n")
        self.assertFalse(self.dirty_flag(),
                         "an untracked file is not part of an export")

    def test_the_tree_stamp_still_carries_the_full_list(self):
        # The stamp answers "what must be touched", not "what would travel":
        # a rewritten run artefact belongs in it.
        art = self.commit_run_artefact()
        art.write_text('{"runs": 2}\n')
        (self.repo / "untracked.txt").write_text("x\n")
        run_gate(self.repo, "strand", plan=self.plan_file(PLAN_CARGO), dry=False)
        stamp = (self.repo / "target" / ".gate-tree").read_text().splitlines()
        self.assertIn("workshop/evals/scenarios/last_run.json", stamp[2])
        self.assertIn("untracked.txt", stamp[2])

    def test_log_dir_does_not_warn_when_there_are_no_station_logs(self):
        # Every station SKIPped -> no logs at all. An unmatched glob must not
        # look like a copy failure.
        out = pathlib.Path(self._tmp.name) / "empty-wave"
        res = run_gate(self.repo, "ci", "--base", "0" * 40, "--log-dir", str(out),
                       plan=self.plan_file("tests\tall()\t1\ttrue\t\n"), dry=False)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertNotIn("could not copy", res.stderr)
        self.assertTrue((out / "last-ci.json").exists())

    def test_log_dir_receives_receipt_and_logs(self):
        out = pathlib.Path(self._tmp.name) / "wave-receipts"
        run_gate(self.repo, "strand", "--log-dir", str(out),
                 plan=self.plan_file(PLAN_OK_BAD))
        self.assertTrue((out / "last-strand.json").exists())
        self.assertTrue((out / "logs" / "strand-ok.log").exists())


class TestTreeStamp(GateShTestCase):
    """target/.gate-tree -- the ghost-binary guard across shared worktrees."""

    def stamp(self):
        return self.repo / "target" / ".gate-tree"

    # The four build inputs a full touch owes the workspace: a library source,
    # an integration test source, the member manifest and the root manifest.
    # Before the fix only the second one was touched, so an rlib compiled from
    # `src/` in a since-deleted worktree stayed "fresh" (measured 2026-09-04).
    BUILD_INPUTS = ("crates/x/src/lib.rs", "crates/x/tests/t.rs",
                    "crates/x/Cargo.toml", "Cargo.toml")

    def add_crate(self):
        """Commit a one-member workspace and age every build input of it."""
        (self.repo / "crates" / "x" / "src").mkdir(parents=True)
        (self.repo / "crates" / "x" / "tests").mkdir(parents=True)
        (self.repo / "crates" / "x" / "src" / "lib.rs").write_text("pub fn a() {}\n")
        (self.repo / "crates" / "x" / "tests" / "t.rs").write_text("#[test]\nfn t() {}\n")
        (self.repo / "crates" / "x" / "Cargo.toml").write_text(
            '[package]\nname = "x"\nversion = "0.0.0"\n')
        (self.repo / "Cargo.toml").write_text('[workspace]\nmembers = ["crates/x"]\n')
        _git(self.repo, "add", "-A")
        _git(self.repo, "commit", "-q", "-m", "crate")
        old = time.time() - 3600
        for rel in self.BUILD_INPUTS:
            os.utime(self.repo / rel, (old, old))
        return old

    def assert_all_inputs_touched(self, before):
        for rel in self.BUILD_INPUTS:
            self.assertGreater((self.repo / rel).stat().st_mtime, before,
                               "%s was not touched" % rel)

    def test_no_stamp_full_touches_every_build_input(self):
        before = self.add_crate()
        res = run_gate(self.repo, "strand", plan=self.plan_file(PLAN_CARGO),
                       dry=False)
        rows = {r["name"]: r for r in gate_lines(res.stdout)}
        self.assertEqual("full touch: no stamp, 4 files", rows["tree-sync"]["scope"])
        self.assert_all_inputs_touched(before)

    def test_a_stale_stamp_full_touches_every_build_input(self):
        before = self.add_crate()
        self.stamp().parent.mkdir(parents=True, exist_ok=True)
        self.stamp().write_text("%s\n%s\n\n" % (self.repo, "deadbeef" * 5))
        res = run_gate(self.repo, "strand", plan=self.plan_file(PLAN_CARGO),
                       dry=False)
        rows = {r["name"]: r for r in gate_lines(res.stdout)}
        self.assertEqual("full touch: stale stamp deadbee, 4 files",
                         rows["tree-sync"]["scope"])
        self.assert_all_inputs_touched(before)

    def test_resync_full_touches_every_build_input(self):
        # An up-to-date stamp for THIS tree: without --resync nothing at all
        # would be touched, so the switch is the only thing under test here.
        before = self.add_crate()
        self.stamp().parent.mkdir(parents=True, exist_ok=True)
        self.stamp().write_text("%s\n%s\n\n" % (
            self.repo, _git(self.repo, "rev-parse", "HEAD").stdout.strip()))
        res = run_gate(self.repo, "strand", "--resync",
                       plan=self.plan_file(PLAN_CARGO), dry=False)
        rows = {r["name"]: r for r in gate_lines(res.stdout)}
        self.assertEqual("full touch: --resync, 4 files", rows["tree-sync"]["scope"])
        self.assert_all_inputs_touched(before)

    def test_a_tree_switch_still_touches_only_the_difference(self):
        # The targeted path is deliberately NOT a full touch: the stamp names a
        # commit that still exists, so the diff plus the dirty lists is exact.
        before = self.add_crate()
        self.stamp().parent.mkdir(parents=True, exist_ok=True)
        self.stamp().write_text("/somewhere/else\n%s\n\n" % (
            _git(self.repo, "rev-parse", "HEAD").stdout.strip()))
        res = run_gate(self.repo, "strand", plan=self.plan_file(PLAN_CARGO),
                       dry=False)
        rows = {r["name"]: r for r in gate_lines(res.stdout)}
        self.assertNotIn("full touch", rows["tree-sync"]["scope"], res.stdout)
        self.assertEqual((self.repo / "crates" / "x" / "src" / "lib.rs").stat().st_mtime,
                         before, "the targeted touch went wide")

    def test_a_stale_stamp_sha_forces_a_full_touch(self):
        self.stamp().parent.mkdir(parents=True, exist_ok=True)
        self.stamp().write_text("%s\n%s\n\n" % (self.repo, "deadbeef" * 5))
        res = run_gate(self.repo, "strand", plan=self.plan_file(PLAN_CARGO),
                       dry=False)
        rows = {r["name"]: r for r in gate_lines(res.stdout)}
        self.assertIn("tree-sync", rows)
        self.assertEqual("NOTE", rows["tree-sync"]["verdict"])
        self.assertEqual("full touch: stale stamp deadbee, 0 files",
                         rows["tree-sync"]["scope"])

    def test_a_first_run_full_touches_and_then_stamps(self):
        res = run_gate(self.repo, "strand", plan=self.plan_file(PLAN_CARGO),
                       dry=False)
        rows = {r["name"]: r for r in gate_lines(res.stdout)}
        self.assertEqual("full touch: no stamp, 0 files", rows["tree-sync"]["scope"])
        lines = self.stamp().read_text().splitlines()
        self.assertEqual(str(self.repo), lines[0])
        self.assertEqual(_git(self.repo, "rev-parse", "HEAD").stdout.strip(), lines[1])

    def test_a_red_cargo_station_still_claims_the_tree(self):
        # The stamp answers "who touched target/ last", not "who filled it
        # green". A failed build leaves artefacts behind exactly like a green
        # one, and those are what the next tree would be handed as fresh.
        self.stamp().parent.mkdir(parents=True, exist_ok=True)
        self.stamp().write_text("/somewhere/else\n%s\n\n" % ("cafe" * 10))
        res = run_gate(self.repo, "strand",
                       plan=self.plan_file("build\tworkspace\t1\tfalse\t\n"), dry=False)
        self.assertEqual(1, res.returncode)
        lines = self.stamp().read_text().splitlines()
        self.assertEqual(str(self.repo), lines[0])
        self.assertEqual(_git(self.repo, "rev-parse", "HEAD").stdout.strip(), lines[1])

    def test_the_stamp_is_written_before_the_build_runs(self):
        # `cat` on the stamp as the station command: the stamp must already be
        # there when the first cargo command is dispatched.
        self.stamp().parent.mkdir(parents=True, exist_ok=True)
        run_gate(self.repo, "strand",
                 plan=self.plan_file("build\tworkspace\t1\tcat target/.gate-tree\t\n"),
                 dry=False)
        log = self.station_log("build").read_text()
        self.assertIn(str(self.repo), log, "the stamp was not there yet: %r" % log)


    def test_ci_neither_syncs_the_tree_nor_stamps_it(self):
        # A fresh checkout has an empty target/: there is nothing a ghost
        # binary could have survived in, and a NOTE line in every workflow log
        # is noise. Run for real -- the dry switch would hide the difference.
        res = run_gate(self.repo, "ci", "--base", "0" * 40,
                       plan=self.plan_file(PLAN_CARGO), dry=False)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertEqual(["build"], [r["name"] for r in gate_lines(res.stdout)])
        self.assertFalse(self.stamp().exists(), "ci wrote a tree stamp")


class TestTargetDirectory(GateShTestCase):
    """Every worktree shares ONE target directory.

    A private `target/` per linked worktree costs a full cold build each and
    fills the disk; it also leaves the worktree without the `target/debug/
    meclaw` the scenario suites run against (measured 2026-09-04).
    """

    def add_worktree(self):
        wt = pathlib.Path(self._tmp.name) / "wt"
        _git(self.repo, "worktree", "add", "-q", "-b", "side", str(wt))
        return wt

    def test_the_main_worktree_keeps_its_own_target(self):
        res = run_gate(self.repo, "strand", plan=self.plan_file(PLAN_OK_BAD))
        self.assertIn("gate: target = %s/target" % self.repo, res.stdout)
        self.assertTrue(self.gate_dir().is_dir())

    def test_a_linked_worktree_writes_into_the_main_target(self):
        wt = self.add_worktree()
        res = run_gate(wt, "strand", plan=self.plan_file(PLAN_CARGO), dry=False)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("gate: target = %s/target" % self.repo, res.stdout)
        rev = _git(wt, "rev-parse", "HEAD").stdout.strip()
        self.assertTrue((self.gate_dir("wt") / ("strand-%s.json" % rev)).exists(),
                        res.stdout + res.stderr)
        self.assertTrue((self.repo / "target" / ".gate-tree").exists())
        # ... and NOT into a private one.
        self.assertFalse((wt / "target").exists(), "the worktree built its own target/")
        # The stamp names the worktree that filled it, which is the whole point
        # of sharing one directory.
        self.assertEqual(str(wt),
                         (self.repo / "target" / ".gate-tree").read_text().splitlines()[0])

    def test_the_plan_only_run_says_which_target_it_would_use(self):
        wt = self.add_worktree()
        res = run_gate(wt, "strand", "--plan-only", plan=self.plan_file(PLAN_OK_BAD))
        self.assertIn("gate: target = %s/target" % self.repo, res.stdout)

    def test_an_explicit_cargo_target_dir_wins(self):
        wt = self.add_worktree()
        chosen = pathlib.Path(self._tmp.name) / "elsewhere"
        res = run_gate(wt, "strand", plan=self.plan_file(PLAN_OK_BAD),
                       extra_env={"CARGO_TARGET_DIR": str(chosen)})
        self.assertIn("gate: target = %s" % chosen, res.stdout)
        self.assertTrue((chosen / "gate").is_dir())
        self.assertFalse((self.repo / "target").exists())


class TestReceiptNamespace(GateShTestCase):
    """Two worktrees at the same rev must not overwrite each other's run.

    Every strand of a wave starts at the SAME master commit and, since the
    trees share one target directory, `gate/<mode>-<rev>.json` and
    `gate/logs/<mode>-<station>.log` were literally the same files for all of
    them. Measured 2026-09-04: the main tree's `strand-tests.log` came back
    holding another worktree's compile output and a nextest error that was not
    its own. The basename of the worktree namespaces them apart; the tree STAMP
    stays shared, because it answers a question about the shared directory.
    """

    def add_worktree(self, name="wt", branch="side"):
        wt = pathlib.Path(self._tmp.name) / name
        _git(self.repo, "worktree", "add", "-q", "-b", branch, str(wt))
        return wt

    def test_the_runner_prints_the_directory_its_receipts_go_to(self):
        res = run_gate(self.repo, "strand", "--plan-only",
                       plan=self.plan_file(PLAN_OK_BAD))
        self.assertIn("gate: receipts = %s" % self.gate_dir(), res.stdout)

    def test_two_worktrees_at_the_same_rev_keep_separate_receipts(self):
        wt = self.add_worktree()
        rev = _git(self.repo, "rev-parse", "HEAD").stdout.strip()
        self.assertEqual(rev, _git(wt, "rev-parse", "HEAD").stdout.strip(),
                         "the fixture must put both trees on the same commit")
        main = run_gate(self.repo, "strand", plan=self.plan_file(PLAN_OK_BAD))
        side = run_gate(wt, "strand",
                        plan=self.plan_file(PLAN_TWO_COMMANDS, name="side.tsv"))
        self.assertEqual(0, main.returncode, main.stdout + main.stderr)
        self.assertEqual(0, side.returncode, side.stdout + side.stderr)

        main_receipt = self.gate_dir() / ("strand-%s.json" % rev)
        side_receipt = self.gate_dir("wt") / ("strand-%s.json" % rev)
        self.assertNotEqual(main_receipt, side_receipt)
        self.assertTrue(main_receipt.exists(), main.stdout + main.stderr)
        self.assertTrue(side_receipt.exists(), side.stdout + side.stderr)

        main_doc = json.loads(main_receipt.read_text())
        side_doc = json.loads(side_receipt.read_text())
        # Same rev in both -- that is exactly why the file names collided ...
        self.assertEqual(rev, main_doc["rev"])
        self.assertEqual(rev, side_doc["rev"])
        # ... and each still holds ITS OWN run, not the other one's.
        self.assertEqual(["ok", "bad", "after"],
                         [s["name"] for s in main_doc["stations"]])
        self.assertEqual(["corpus"], [s["name"] for s in side_doc["stations"]])
        self.assertEqual(main_doc, self.last_receipt())
        self.assertEqual(side_doc, self.last_receipt(tree="wt"))

        # The station logs are namespaced with them.
        self.assertIn("true", self.station_log("ok").read_text())
        self.assertTrue(self.station_log("corpus", tree="wt").exists())
        self.assertFalse(self.station_log("ok", tree="wt").exists(),
                         "a worktree wrote into another tree's log directory")

    def test_the_tree_stamp_stays_shared(self):
        # It records which tree filled `target/` last -- one shared question,
        # one shared file. Namespacing it would defeat the ghost-binary guard.
        wt = self.add_worktree()
        run_gate(wt, "strand", plan=self.plan_file(PLAN_CARGO), dry=False)
        self.assertTrue((self.repo / "target" / ".gate-tree").is_file())
        self.assertFalse((self.gate_dir("wt") / ".gate-tree").exists())

    def test_log_dir_copies_from_the_running_trees_namespace(self):
        wt = self.add_worktree()
        out = pathlib.Path(self._tmp.name) / "wave-receipts"
        # The main tree runs first and leaves a receipt and a log behind ...
        run_gate(self.repo, "strand", plan=self.plan_file(PLAN_OK_BAD))
        # ... the copy of the worktree's run must not pick any of it up.
        run_gate(wt, "strand", "--log-dir", str(out),
                 plan=self.plan_file(PLAN_TWO_COMMANDS, name="side.tsv"))
        self.assertTrue((out / "logs" / "strand-corpus.log").exists())
        self.assertFalse((out / "logs" / "strand-ok.log").exists(),
                         "--log-dir copied another worktree's station logs")
        doc = json.loads((out / "last-strand.json").read_text())
        self.assertEqual(["corpus"], [s["name"] for s in doc["stations"]])


class TestOnly(GateShTestCase):
    def test_only_runs_exactly_the_named_stations(self):
        res = run_gate(self.repo, "strand", "--only", "ok,after",
                       plan=self.plan_file(PLAN_OK_BAD))
        self.assertEqual(["ok", "after"], [r["name"] for r in gate_lines(res.stdout)])
        self.assertEqual(0, res.returncode)

    def test_only_with_an_unknown_station_is_an_error(self):
        res = run_gate(self.repo, "strand", "--only", "nope",
                       plan=self.plan_file(PLAN_OK_BAD))
        self.assertNotEqual(0, res.returncode)
        self.assertIn("nope", res.stderr)


class TestRed(GateShTestCase):
    def test_a_red_command_is_red_and_the_run_keeps_going(self):
        res = run_gate(self.repo, "strand", plan=self.plan_file(PLAN_OK_BAD),
                       dry=False)
        rows = gate_lines(res.stdout)
        self.assertEqual(["ok", "bad", "after"], [r["name"] for r in rows],
                         "keep-going is the default: `after` must still run")
        self.assertEqual(["GREEN", "RED", "GREEN"], [r["verdict"] for r in rows])
        self.assertEqual(1, res.returncode)
        summary = [m.groupdict() for m in
                   (SUMMARY_LINE.match(ln) for ln in res.stdout.splitlines()) if m][0]
        self.assertEqual("RED", summary["verdict"])
        self.assertEqual("2", summary["green"])
        self.assertEqual("3", summary["total"])
        self.assertIn("run the gate again as a whole", res.stdout + res.stderr)
        doc = self.last_receipt()
        self.assertEqual("RED", doc["verdict"])
        self.assertEqual("RED", doc["stations"][1]["verdict"])

    def test_fail_fast_stops_after_the_first_red(self):
        res = run_gate(self.repo, "strand", "--fail-fast",
                       plan=self.plan_file(PLAN_OK_BAD), dry=False)
        self.assertEqual(["ok", "bad"], [r["name"] for r in gate_lines(res.stdout)])
        self.assertEqual(1, res.returncode)


class TestCiMode(GateShTestCase):
    def test_ci_skips_the_tests_station_by_name(self):
        res = run_gate(self.repo, "ci", "--base", "0" * 40,
                       plan=self.plan_file(PLAN_WITH_TESTS))
        rows = {r["name"]: r for r in gate_lines(res.stdout)}
        self.assertIn("tests", rows)
        self.assertEqual("SKIP", rows["tests"]["verdict"])
        self.assertEqual("planned-for-shards", rows["tests"]["reason"])
        self.assertEqual("0", rows["tests"]["secs"])
        self.assertEqual(0, res.returncode)

    def test_ci_plan_only_still_reports_tests_true(self):
        res = run_gate(self.repo, "ci", "--base", "0" * 40, "--plan-only",
                       plan=self.plan_file(PLAN_WITH_TESTS))
        self.assertEqual("tests=true", res.stdout.strip().splitlines()[-1])


class TestStationCwd(GateShTestCase):
    """A station's argv is relative to its cwd -- the runner `cd`s in first."""

    def test_a_station_runs_its_command_inside_its_cwd(self):
        sub = self.repo / "sub"
        sub.mkdir()
        (sub / "marker.txt").write_text("i-am-in-sub\n")
        _git(self.repo, "add", "-A")
        _git(self.repo, "commit", "-q", "-m", "sub")
        res = run_gate(self.repo, "strand",
                       plan=self.plan_file("cwd\tsub\t0\tcat marker.txt\tsub\n"),
                       dry=False)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        log = self.station_log("cwd").read_text()
        self.assertIn("i-am-in-sub", log,
                      "the station did not run inside its cwd: %r" % log)
        self.assertEqual("GREEN", gate_lines(res.stdout)[0]["verdict"])


class TestSummaryCounts(GateShTestCase):
    """`green/total` grades JUDGEMENTS: SKIP and NOTE are in neither half."""

    def test_skip_and_note_are_in_neither_half(self):
        # `deny` with no cargo-deny on PATH is the SKIP; the first cargo
        # station on a repo without a stamp produces the tree-sync NOTE.
        # Two GREENs remain, so the summary has to read 2/2.
        plan = ("ok\tscope\t0\ttrue\t\n"
                "deny\tbans\t0\ttrue\t\n"
                "build\tworkspace\t1\ttrue\t\n")
        res = run_gate(self.repo, "strand", plan=self.plan_file(plan), dry=False,
                       extra_env={"PATH": "/usr/bin:/bin"})
        rows = {r["name"]: r for r in gate_lines(res.stdout)}
        self.assertEqual("SKIP", rows["deny"]["verdict"], res.stdout)
        self.assertEqual("NOTE", rows["tree-sync"]["verdict"], res.stdout)
        summary = [m.groupdict() for m in
                   (SUMMARY_LINE.match(ln) for ln in res.stdout.splitlines()) if m][0]
        self.assertEqual("2", summary["green"])
        self.assertEqual("2", summary["total"])
        self.assertEqual("GREEN", summary["verdict"])
        self.assertEqual(0, res.returncode)
        # Every station is still on its own line and in the receipt.
        doc = self.last_receipt()
        self.assertEqual(["ok", "deny", "tree-sync", "build"],
                         [s["name"] for s in doc["stations"]])


class TestCargoLockFd(GateShTestCase):
    """The lock lives on the runner's own fd, never in the station's argv.

    `flock <file> <cmd>` hands the open descriptor to the command and from
    there to everything it leaves behind. Measured 2026-09-04: a test that
    spawned `sleep 300` outlived its runner, kept the inherited lock fd, and
    made the next cargo station wait for it -- 209 s and 263 s for a station
    whose cargo-deny runs in 0.5 s.
    """

    def test_a_cargo_station_does_not_inherit_the_lock_descriptor(self):
        plan = ("build\tworkspace\t1\t"
                "sh -c 'ls -l /proc/$$/fd | grep -c cargo.lock || true'\t\n")
        res = run_gate(self.repo, "strand", plan=self.plan_file(plan), dry=False)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        log = self.station_log("build").read_text()
        counts = [ln for ln in log.splitlines() if ln.strip().isdigit()]
        self.assertTrue(counts, "the station printed no fd count: %r" % log)
        self.assertEqual(["0"], counts,
                         "the station inherited the cargo lock fd: %r" % log)

    def test_two_cargo_stations_run_under_the_one_run_lock(self):
        # The lock is taken once and held to the end, so the second station
        # must not sit in `flock` waiting for a lock its own run holds.
        plan = ("one\tworkspace\t1\ttrue\t\n"
                "two\tworkspace\t1\ttrue\t\n")
        res = run_gate(self.repo, "strand", plan=self.plan_file(plan), dry=False,
                       timeout_s=30)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        rows = {r["name"]: r for r in gate_lines(res.stdout)}
        self.assertEqual("GREEN", rows["one"]["verdict"])
        self.assertEqual("GREEN", rows["two"]["verdict"])

    def test_a_child_that_outlives_its_station_does_not_block_the_next_one(self):
        # The regression itself. `sleep 20` is orphaned by the first station;
        # with the lock in the station's argv it would hold the descriptor and
        # the second station would sit in `flock` for twenty seconds. Its
        # stdout is the station log, not this process's pipe, so nothing here
        # waits on it either way.
        plan = ("one\tworkspace\t1\tsh -c 'sleep 20 & echo spawned'\t\n"
                "two\tworkspace\t1\ttrue\t\n")
        res = run_gate(self.repo, "strand", plan=self.plan_file(plan), dry=False,
                       timeout_s=10)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        names = [r["name"] for r in gate_lines(res.stdout)]
        self.assertEqual(["one", "two"], [n for n in names if n != "tree-sync"])


class TestRunWideCargoLock(GateShTestCase):
    """One shared target/ means one build at a time -- for the WHOLE run.

    Holding the lock only around each cargo command left gaps between the
    stations, and another worktree built the same workspace members from ITS
    sources in one of them; cargo then handed this run its own older sources
    back as fresh and linked against the foreign rlib. Measured 2026-09-04
    (release run 4, five strand gates in parallel): `error[E0063]: missing
    base_path` in `meclaw_core::TransferBounds`, a field that exists in one
    worktree of the wave only. Non-cargo stations that USE target artefacts
    (`scenarios:*` run `target/debug/meclaw`) are inside the same window.
    """

    def lock_path(self):
        return self.repo.parent / "cargo.lock.test"

    def test_the_lock_spans_the_stations_after_the_first_cargo_one(self):
        res = run_gate(self.repo, "strand", plan=self.plan_file(PLAN_LOCK_WINDOW),
                       dry=False, timeout_s=30)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        # Before the first cargo station nothing is locked ...
        self.assertIn("held=0", self.station_log("early").read_text())
        # ... and from there to the end of the run it is.
        self.assertIn("held=1", self.station_log("late").read_text(),
                      "the lock was dropped between two stations")

    def test_the_lock_is_free_again_when_the_run_ends(self):
        res = run_gate(self.repo, "strand", plan=self.plan_file(PLAN_CARGO),
                       dry=False, timeout_s=30)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        free = subprocess.run(["flock", "-n", str(self.lock_path()), "true"])
        self.assertEqual(0, free.returncode,
                         "the run kept the cargo lock after it finished")

    def test_a_run_without_a_cargo_station_never_takes_the_lock(self):
        plan = "probe\tno-cargo\t0\t" + _PROBE + "\t\n"
        res = run_gate(self.repo, "strand", plan=self.plan_file(plan), dry=False)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("held=0", self.station_log("probe").read_text())

    def test_ci_takes_no_lock_at_all(self):
        plan = ("build\tworkspace\t1\ttrue\t\n"
                "probe\tafter-cargo\t0\t" + _PROBE + "\t\n")
        res = run_gate(self.repo, "ci", "--base", "0" * 40,
                       plan=self.plan_file(plan), dry=False)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("held=0",
                      self.station_log("probe", mode="ci").read_text())


class TestTierLockHandoff(GateShTestCase):
    """`scripts/test-tier.sh` must not take a lock the gate already holds.

    The gate now runs the `tests` station like every other cargo station --
    under the run-wide lock. The tier script takes the SAME lock when it is
    run on its own, so the gate hands it `MECLAW_CARGO_LOCK_HELD=1` and the
    script skips its own `flock`. Without that the gate would deadlock against
    its own child. `MECLAW_TIER_DRY=1` prints the nextest argv instead of
    running it, so this proves the LOCKING and never compiles anything.
    """

    def tier_sh(self):
        src = REPO / "scripts" / "test-tier.sh"
        dst = self.repo / "scripts" / "test-tier.sh"
        shutil.copy(src, dst)
        dst.chmod(0o755)
        return dst

    def run_tier(self, held, timeout_s):
        """Run the tier script while THIS process holds the cargo lock."""
        tier = self.tier_sh()
        lock = pathlib.Path(self._tmp.name) / "tier.lock"
        env = dict(os.environ)
        env.pop("CI", None)
        env["MECLAW_GATE_LOCK"] = str(lock)
        env["MECLAW_TIER_DRY"] = "1"
        if held:
            env["MECLAW_CARGO_LOCK_HELD"] = "1"
        else:
            env.pop("MECLAW_CARGO_LOCK_HELD", None)
        with open(lock, "w") as fh:
            fcntl.flock(fh, fcntl.LOCK_EX)
            return subprocess.run([str(tier), "t0"], cwd=str(self.repo), env=env,
                                  capture_output=True, text=True, timeout=timeout_s)

    def test_the_tier_skips_its_own_flock_when_the_caller_holds_it(self):
        res = self.run_tier(held=True, timeout_s=30)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("cargo lock: held by the caller", res.stdout)
        self.assertIn("tier-dry:", res.stdout)

    def test_the_tier_still_takes_the_lock_on_its_own(self):
        # The counter-case, and the reason the skip is not simply "never
        # lock": run outside the gate, the script serialises as it always did.
        with self.assertRaises(subprocess.TimeoutExpired):
            self.run_tier(held=False, timeout_s=5)


class TestOptionValues(GateShTestCase):
    """An option that wants a value says so instead of hanging on an empty one."""

    def test_an_option_without_a_value_is_a_usage_error(self):
        for flag in ("--base", "--only", "--log-dir"):
            res = subprocess.run(
                ["timeout", "5", str(self.repo / "scripts" / "gate.sh"),
                 "strand", flag],
                cwd=str(self.repo), capture_output=True, text=True)
            self.assertEqual(2, res.returncode,
                             "%s: %r / %r" % (flag, res.stdout, res.stderr))
            self.assertIn("%s needs a value" % flag, res.stderr)


class TestPorcelainRenames(GateShTestCase):
    """A staged rename must not travel as one path.

    `git status --porcelain` prints a rename as `R  old -> new` (and quotes
    either half when it holds a space). Cutting the first three characters off
    that line yields the literal string `old -> new`, which then reached the
    resolver and the nextest filterset: `failed to parse filterset`, the tests
    station RED with 0 tests run (measured 2026-09-04). Both halves are real
    paths -- the old one is a deletion the resolver drops by itself, the new
    one is the file to touch and to select.
    """

    def stamp_paths(self):
        """The tree stamp's third line IS the dirty list, comma-joined."""
        text = (self.repo / "target" / ".gate-tree").read_text()
        return text.splitlines()[2].split(",")

    def run_with_a_cargo_station(self):
        res = run_gate(self.repo, "strand", plan=self.plan_file(PLAN_CARGO),
                       dry=False)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        return res

    def test_a_staged_rename_yields_both_paths_and_never_an_arrow(self):
        (self.repo / "a.rs").write_text("fn a() {}\n")
        _git(self.repo, "add", "a.rs")
        _git(self.repo, "commit", "-q", "-m", "a.rs")
        _git(self.repo, "mv", "a.rs", "b.rs")
        # ... and a plainly modified file next to it.
        (self.repo / "README.md").write_text("third\n")
        self.run_with_a_cargo_station()
        paths = self.stamp_paths()
        self.assertIn("b.rs", paths)
        self.assertIn("a.rs", paths)
        self.assertIn("README.md", paths)
        self.assertFalse([p for p in paths if "->" in p],
                         "a rename arrow reached the path list: %r" % (paths,))

    def test_a_path_with_a_space_survives_unquoted(self):
        (self.repo / "two words.rs").write_text("fn t() {}\n")
        _git(self.repo, "add", "two words.rs")
        self.run_with_a_cargo_station()
        paths = self.stamp_paths()
        self.assertIn("two words.rs", paths,
                      "the path came back quoted or mangled: %r" % (paths,))

    def test_a_renamed_path_with_a_space_yields_both_halves(self):
        (self.repo / "old name.rs").write_text("fn o() {}\n")
        _git(self.repo, "add", "old name.rs")
        _git(self.repo, "commit", "-q", "-m", "old name")
        _git(self.repo, "mv", "old name.rs", "new name.rs")
        self.run_with_a_cargo_station()
        paths = self.stamp_paths()
        self.assertIn("new name.rs", paths)
        self.assertIn("old name.rs", paths)

    def test_an_untracked_rename_half_does_not_make_the_tree_dirty(self):
        """The `dirty` field is the tracked half -- a rename IS tracked."""
        (self.repo / "a.rs").write_text("fn a() {}\n")
        _git(self.repo, "add", "a.rs")
        _git(self.repo, "commit", "-q", "-m", "a.rs")
        _git(self.repo, "mv", "a.rs", "b.rs")
        self.run_with_a_cargo_station()
        self.assertTrue(self.last_receipt()["dirty"])


class TestEnvLink(GateShTestCase):
    """The scenario stations read `<repo>/.env`; a linked worktree has none.

    `workshop/evals/scenarios/run_scenarios.py` reads the tree's `.env` and
    copies the keys it needs into every colony it boots. From a linked worktree
    that file does not exist -- `FileNotFoundError .../wt-t7/.env`, the station
    RED with 0 cases (measured 2026-09-04). The runner LINKS it. It never opens
    it.
    """

    def make_env(self):
        # A fixture, not a secret: the runner only ever links this file.
        (self.repo / ".env").write_text("FAKE_KEY=not-a-secret\n")
        (self.repo / ".gitignore").write_text(".env\n")
        _git(self.repo, "add", ".gitignore")
        _git(self.repo, "commit", "-q", "-m", "ignore .env")

    def add_worktree(self, name="wt", branch="side"):
        wt = pathlib.Path(self._tmp.name) / name
        _git(self.repo, "worktree", "add", "-q", "-b", branch, str(wt))
        return wt

    def test_a_linked_worktree_gets_a_symlink_to_the_main_trees_env(self):
        self.make_env()
        wt = self.add_worktree()
        res = run_gate(wt, "strand", "--plan-only", plan=self.plan_file(PLAN_OK_BAD))
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("gate: env = %s/.env (linked)" % self.repo, res.stdout)
        self.assertTrue((wt / ".env").is_symlink(), res.stdout)
        self.assertEqual(str(self.repo / ".env"), os.readlink(str(wt / ".env")))

    def test_a_second_run_does_not_recreate_the_link(self):
        self.make_env()
        wt = self.add_worktree()
        run_gate(wt, "strand", "--plan-only", plan=self.plan_file(PLAN_OK_BAD))
        before = os.lstat(str(wt / ".env"))
        time.sleep(0.02)
        res = run_gate(wt, "strand", "--plan-only", plan=self.plan_file(PLAN_OK_BAD))
        after = os.lstat(str(wt / ".env"))
        self.assertEqual((before.st_ino, before.st_ctime_ns),
                         (after.st_ino, after.st_ctime_ns),
                         "the link was recreated")
        # It still says where the file comes from.
        self.assertIn("gate: env = %s/.env (linked)" % self.repo, res.stdout)

    def test_a_worktree_whose_main_tree_has_no_env_gets_none(self):
        wt = self.add_worktree()
        res = run_gate(wt, "strand", "--plan-only", plan=self.plan_file(PLAN_OK_BAD))
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertNotIn("gate: env =", res.stdout)
        self.assertFalse((wt / ".env").is_symlink())
        self.assertFalse((wt / ".env").exists())

    def test_the_main_worktree_is_never_linked_to_itself(self):
        self.make_env()
        res = run_gate(self.repo, "strand", "--plan-only",
                       plan=self.plan_file(PLAN_OK_BAD))
        self.assertNotIn("gate: env =", res.stdout)
        self.assertFalse((self.repo / ".env").is_symlink())


class TestLockWait(GateShTestCase):
    """The queue in front of the run lock is visible, not hidden in a station.

    A wave measured 3211 s for `fmt` -- almost all of it spent waiting for
    another run's lock. The wait is now its own NOTE line before the first
    cargo station, and its own field in the receipt.
    """

    def lock_path(self):
        return self.repo.parent / "cargo.lock.test"

    def hold_the_lock(self, seconds):
        lock = self.lock_path()
        lock.touch()
        holder = subprocess.Popen(["flock", str(lock), "sleep", str(seconds)])
        self.addCleanup(holder.wait)
        self.addCleanup(holder.terminate)
        for _ in range(500):
            probe = subprocess.run(["flock", "-n", str(lock), "true"])
            if probe.returncode != 0:
                return holder
            time.sleep(0.01)
        self.skipTest("the background holder never took the lock")

    def test_the_wait_is_its_own_note_line_before_the_cargo_station(self):
        self.hold_the_lock(3)
        res = run_gate(self.repo, "strand", plan=self.plan_file(PLAN_CARGO),
                       dry=False, timeout_s=60)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        rows = gate_lines(res.stdout)
        names = [r["name"] for r in rows]
        self.assertIn("lock-wait", names, res.stdout)
        row = rows[names.index("lock-wait")]
        self.assertEqual("NOTE", row["verdict"])
        self.assertGreaterEqual(int(row["secs"]), 2, res.stdout)
        self.assertIn("behind other runs", row["scope"])
        # It comes BEFORE the station it was waiting for ...
        self.assertLess(names.index("lock-wait"), names.index("build"), names)
        # ... and that station's own seconds no longer carry the queue.
        self.assertLessEqual(int(rows[names.index("build")]["secs"]), 1, res.stdout)

    def test_the_wait_is_in_the_receipt_and_counts_as_no_judgement(self):
        self.hold_the_lock(3)
        res = run_gate(self.repo, "strand", plan=self.plan_file(PLAN_CARGO),
                       dry=False, timeout_s=60)
        receipt = self.last_receipt()
        self.assertGreaterEqual(receipt["lock_wait_secs"], 2, res.stdout)
        self.assertIn("lock-wait", [s["name"] for s in receipt["stations"]])
        summary = [m.groupdict() for m in
                   (SUMMARY_LINE.match(ln) for ln in res.stdout.splitlines()) if m]
        self.assertEqual("1", summary[0]["total"], res.stdout)

    def test_a_run_that_never_waits_has_no_line_and_a_zero_field(self):
        res = run_gate(self.repo, "strand", plan=self.plan_file(PLAN_CARGO),
                       dry=False, timeout_s=60)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertNotIn("lock-wait", [r["name"] for r in gate_lines(res.stdout)])
        self.assertEqual(0, self.last_receipt()["lock_wait_secs"])


class TestUsage(GateShTestCase):
    def test_help_mentions_every_flag(self):
        res = run_gate(self.repo, "--help", plan=self.plan_file(PLAN_OK_BAD))
        self.assertEqual(0, res.returncode, res.stderr)
        for flag in ("--base", "--only", "--fail-fast", "--plan-only",
                     "--log-dir", "--no-nice", "--resync"):
            self.assertIn(flag, res.stdout)

    def test_unknown_mode_is_an_error(self):
        res = run_gate(self.repo, "sideways", plan=self.plan_file(PLAN_OK_BAD))
        self.assertEqual(2, res.returncode)
        self.assertIn("sideways", res.stderr)


if __name__ == "__main__":
    unittest.main()
