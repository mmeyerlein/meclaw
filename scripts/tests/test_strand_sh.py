"""Tests for the strand kit `scripts/strand.sh`.

Every case runs against a throw-away git repository with a plans tree of its
own -- never against the real one: `new` creates a worktree and a branch, and
`gate` runs the real gate runner in dry mode.

What is pinned here is the CONTRACT the wave process leans on:

  * `new` makes the worktree, the branch and the report skeleton, and prints a
    dispatch prompt that POINTS at the rule file instead of copying it. The
    measurement behind that: wave H3 repeated 2.8 % of its prompt words where
    the waves before it repeated 15.6 %, because it was the first with a
    preamble (`befund/04-struktur.md` section 7.2).
  * `gate` blocks until the run ends and prints ONE summary line plus the red
    stations -- nobody polls a log. The measurement: a release agent spent
    2 112 `Read` calls re-reading gate logs and task outputs
    (`befund/02-token.md` section 3.4).
  * `report` completes the header block and refuses a red gate.
  * `close` writes the closing comment for the issue, in English, without a
    person's name or a private host.
"""

import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
import uuid

REPO = pathlib.Path(__file__).resolve().parents[2]
STRAND_SH = REPO / "scripts" / "strand.sh"
GATE_SH = REPO / "scripts" / "gate.sh"
TIER_SH = REPO / "scripts" / "test-tier.sh"
GATE_PLAN = REPO / "scripts" / "gate_plan.py"
LANE_SYNC = REPO / "scripts" / "lane_sync.sh"

WAVE_DIR = "welle-x-2026-09-19"
WAVE = "welle-x"

# The four rule sentences that a dispatch prompt must NOT carry any more --
# one probe per rule family of `befund/04-struktur.md` section 7.3.
RULE_WORDS = (".env", "pkill", "flock", "NEXTEST")

GIT_ENV = {
    "GIT_AUTHOR_NAME": "strand test",
    "GIT_AUTHOR_EMAIL": "strand@example.invalid",
    "GIT_COMMITTER_NAME": "strand test",
    "GIT_COMMITTER_EMAIL": "strand@example.invalid",
    "GIT_CONFIG_GLOBAL": os.devnull,
    "GIT_CONFIG_SYSTEM": os.devnull,
}

PLANS_README = """# Plans

## Live

| Path | What it is |
|---|---|
| `%s/` | The wave under test. |
| `welle-old-2026-01-01/` | An older one, and it must not win. |
""" % WAVE_DIR

# A plan part as the waves write them since wave Gate: shell blocks whose
# `#` comments look like headings to a line reader, and a section key with
# brackets. Gate receipt F17: `--section` cut the order at the first comment
# of a code block and read the key as an awk regex.
FENCED_PLAN_FILE = """# The wave plan

```bash
# Strang (R) [retro] is built last -- a comment, not a heading
echo prepare
```

## Strang (R) [retro] -- the retro counts the planning

Contracts:
- the retro counts the planning session.

```bash
# a comment inside the section
scripts/wave_retro.py --check
```

- and the check stays green.

## Strang (S) -- something else

Not this one.
"""

PLAN_FILE = """# The wave plan

### P1 -- kit and report form

Contracts:
- `scripts/strand.sh new` makes the worktree.

### P2 -- something else

Not this one.
"""


def _git(repo, *args):
    env = dict(os.environ)
    env.update(GIT_ENV)
    return subprocess.run(["git", "-C", str(repo)] + list(args),
                          env=env, check=True, capture_output=True, text=True)


def make_repo(root):
    """A one-commit repo with the kit, the gate runner and a plans tree."""
    repo = pathlib.Path(root) / "repo"
    (repo / "scripts" / "tests").mkdir(parents=True, exist_ok=True)
    for src in (STRAND_SH, GATE_SH, TIER_SH, LANE_SYNC):
        shutil.copy(src, repo / "scripts" / src.name)
        (repo / "scripts" / src.name).chmod(0o755)
    shutil.copy(GATE_PLAN, repo / "scripts" / "gate_plan.py")
    # `scripts/gate.sh` SOURCES this one (GH #802) -- without it the runner in
    # the throw-away repo does not start at all.
    shutil.copy(REPO / "scripts" / "cargo-target.sh", repo / "scripts" / "cargo-target.sh")
    plans = repo / "plans"
    (plans / WAVE_DIR / "berichte").mkdir(parents=True, exist_ok=True)
    (plans / "README.md").write_text(PLANS_README)
    (plans / WAVE_DIR / "BUILD-PREAMBLE.md").write_text("# Wave preamble\n")
    (plans / "PREAMBLE.md").write_text("# The rules\n")
    (repo / "README.md").write_text("first\n")
    _git(repo, "init", "-q")
    _git(repo, "add", "scripts", "plans", "README.md")
    _git(repo, "commit", "-q", "-m", "first")
    _git(repo, "branch", "-M", "master")
    return repo


# The host's own token file -- the one a running wave arms. No case may ever
# reach it: the default path is derived from the default cargo lock.
HOST_TOKENS = pathlib.Path("/tmp/meclaw-w26-cargo.tokens")


def kit_env(repo, extra_env=None):
    """The environment of every kit and tier call in this module.

    The token file and the cargo lock are SET, never defaulted: a developer
    shell -- or a wave -- may carry either, and a case that inherited the
    host's armed token file would take a real token. `CI` and
    `MECLAW_CARGO_LOCK_HELD` are the two exemptions of the tier's token check
    and GitHub sets the first one, so both go; the one case that checks the
    exemption puts them back through `extra_env`.
    """
    env = dict(os.environ)
    env.update(GIT_ENV)
    env.pop("CARGO_TARGET_DIR", None)
    for var in ("MECLAW_STRAND_TOKEN_SKIP", "CI", "MECLAW_CARGO_LOCK_HELD",
                "MECLAW_STRAND_NOW", "MECLAW_TIER_DRY", "MECLAW_GATE_LANE_RECEIPT"):
        env.pop(var, None)
    env["MECLAW_STRAND_TOKENS"] = str(repo.parent / "tokens")
    env["MECLAW_GATE_LOCK"] = str(repo.parent / "cargo.lock")
    # ... and so is the host file of the build lanes (GH #934): the owner's
    # real one names real machines.
    env["MECLAW_LANES_FILE"] = str(repo.parent / "lanes")
    env.setdefault("MECLAW_GATE_MIN_FREE_G", "0")
    # The lane floor too (default 60 G): the fake host's target lies in this
    # machine's temp directory, and on a build host that is a tmpfs with less
    # than 60 G free -- the runner wiped the fake target and refused, and the
    # gate-on-a-host cases were red on the lane alone (OR-S3-96).
    env.setdefault("MECLAW_GATE_LANE_MIN_FREE_G", "0")
    if extra_env:
        env.update(extra_env)
    return env


def run_strand(repo, *args, cwd=None, extra_env=None, timeout_s=120):
    return subprocess.run(
        [str(repo / "scripts" / "strand.sh")] + list(args),
        cwd=str(cwd or repo), env=kit_env(repo, extra_env),
        capture_output=True, text=True, timeout=timeout_s)


def run_tier(repo, tree, extra_env=None, timeout_s=120):
    """`scripts/test-tier.sh t0` of a tree in dry mode: it locks, checks and
    prints the nextest argv, and never compiles anything."""
    env = kit_env(repo, extra_env)
    env["MECLAW_TIER_DRY"] = "1"
    return subprocess.run(
        [str(pathlib.Path(tree) / "scripts" / "test-tier.sh"), "t0"],
        cwd=str(tree), env=env, capture_output=True, text=True,
        timeout=timeout_s)


def header_block(path):
    """The YAML head of a report: the lines between the first two `---`."""
    text = pathlib.Path(path).read_text()
    m = re.match(r"^---\n(.*?)\n---\n", text, re.S)
    if not m:
        raise AssertionError("no header block in %s:\n%s" % (path, text[:400]))
    out = {}
    for line in m.group(1).splitlines():
        key, _, value = line.partition(":")
        out[key.strip()] = value.strip()
    return out


class StrandShTestCase(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.repo = make_repo(self._tmp.name)
        # A worktree outlives the test object; git refuses to delete the
        # directory tree under it otherwise.
        self.addCleanup(self._prune)

    def _prune(self):
        subprocess.run(["git", "-C", str(self.repo), "worktree", "prune"],
                       capture_output=True, text=True)

    def worktree(self, name):
        return self.repo.parent / ("%s-wt-%s" % (self.repo.name, name))

    def report(self, name):
        return self.repo / "plans" / WAVE_DIR / "berichte" / ("%s.md" % name)


class TestNew(StrandShTestCase):
    def test_new_makes_worktree_branch_and_report(self):
        res = run_strand(self.repo, "new", "kit", "--issue", "756",
                         "--issue", "755")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertTrue(self.worktree("kit").is_dir(),
                        "worktree missing: %s" % res.stderr)
        branches = _git(self.repo, "branch", "--list", "%s/kit" % WAVE).stdout
        self.assertIn("%s/kit" % WAVE, branches)
        head = header_block(self.report("kit"))
        self.assertEqual("kit", head["strang"])
        self.assertEqual("%s/kit" % WAVE, head["branch"])
        self.assertEqual("[#756, #755]", head["issues"])
        self.assertRegex(head["basis"], r"^[0-9a-f]{7,40}$")
        self.assertIn("gate", head)
        self.assertIn("commits", head)

    def test_new_prompt_points_at_the_rules_and_repeats_none_of_them(self):
        res = run_strand(self.repo, "new", "kit", "--issue", "756")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("plans/PREAMBLE.md", res.stdout)
        self.assertIn("plans/%s/BUILD-PREAMBLE.md" % WAVE_DIR, res.stdout)
        for word in RULE_WORDS:
            self.assertNotIn(word, res.stdout,
                             "the prompt repeats the rule %r:\n%s"
                             % (word, res.stdout))

    def test_new_takes_the_order_from_the_plan_section(self):
        plan = self.repo / "plan.md"
        plan.write_text(PLAN_FILE)
        res = run_strand(self.repo, "new", "kit", "--issue", "756",
                         "--plan", "plan.md")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("`scripts/strand.sh new` makes the worktree.", res.stdout)
        self.assertNotIn("Not this one.", res.stdout)

    def test_new_section_skips_code_comments_and_takes_the_key_as_text(self):
        plan = self.repo / "plan.md"
        plan.write_text(FENCED_PLAN_FILE)
        res = run_strand(self.repo, "new", "retro", "--issue", "891",
                         "--plan", "plan.md", "--section", "Strang (R) [retro]")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("## Strang (R) [retro] -- the retro counts", res.stdout)
        self.assertIn("# a comment inside the section", res.stdout)
        self.assertIn("- and the check stays green.", res.stdout)
        self.assertNotIn("echo prepare", res.stdout)
        self.assertNotIn("Not this one.", res.stdout)

    def test_new_refuses_an_existing_strand(self):
        self.assertEqual(0, run_strand(self.repo, "new", "kit", "--issue",
                                       "756").returncode)
        res = run_strand(self.repo, "new", "kit", "--issue", "756")
        self.assertNotEqual(0, res.returncode)
        self.assertIn("kit", res.stderr)


BUILT_README = """# Plans

## Live

| Path | What it is |
|---|---|
| `%s/` | **Built 2026-09-19:** the wave under test, and it is over. |
""" % WAVE_DIR

# A live table whose top row is an OTHER wave that still exists on disk: the
# branch has to win over it.
OTHER_WAVE_DIR = "welle-y-2026-09-20"
OTHER_README = """# Plans

## Live

| Path | What it is |
|---|---|
| `%s/` | Another wave entirely. |
| `%s/` | The wave under test. |
""" % (OTHER_WAVE_DIR, WAVE_DIR)


# A second wave of the SAME branch prefix, newer by name, on top of the live
# table: the branch alone no longer names one directory.
SAME_PREFIX_WAVE_DIR = "welle-x-2026-09-30"
SAME_PREFIX_README = """# Plans

## Live

| Path | What it is |
|---|---|
| `%s/` | Another wave of the same prefix. |
| `%s/` | The wave under test. |
""" % (SAME_PREFIX_WAVE_DIR, WAVE_DIR)


class TestWaveDetection(StrandShTestCase):
    """Which wave the kit writes into -- never the wrong one in silence.

    The Live table of `plans/README.md` is an archive in order, not a pointer
    at what is running: its top row is the wave built LAST. Taking it means
    writing the report and the gate archive of a running strand into a wave
    that is over.
    """

    def test_new_refuses_a_live_table_whose_newest_wave_is_built(self):
        (self.repo / "plans" / "README.md").write_text(BUILT_README)
        res = run_strand(self.repo, "new", "kit", "--issue", "756")
        self.assertNotEqual(0, res.returncode, res.stdout)
        self.assertIn("--wave", res.stderr)
        self.assertFalse(self.report("kit").exists(),
                         "it wrote into a wave that is over")

    def test_the_branch_wins_over_the_top_row(self):
        res = run_strand(self.repo, "new", "kit", "--issue", "756",
                         "--wave", WAVE_DIR)
        self.assertEqual(0, res.returncode, res.stderr)
        other = self.repo / "plans" / OTHER_WAVE_DIR / "berichte"
        other.mkdir(parents=True)
        (self.repo / "plans" / "README.md").write_text(OTHER_README)
        plan = pathlib.Path(self._tmp.name) / "plan.tsv"
        plan.write_text(PLAN_GREEN)
        res = run_strand(self.repo, "gate", cwd=self.worktree("kit"),
                         extra_env={"MECLAW_GATE_PLAN": str(plan)})
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertTrue(
            (self.repo / "plans" / WAVE_DIR / "receipts" / "kit"
             / "latest" / "summary.txt").is_file(),
            "the gate archive did not land in the branch's wave")
        self.assertFalse(
            (self.repo / "plans" / OTHER_WAVE_DIR / "receipts").exists(),
            "the gate archive landed in the top row's wave")

    def _second_wave_of_the_prefix(self, with_report):
        """A second `plans/<wave>-*` directory beside the wave under test, as
        on the build host on 2026-09-26: `welle-fix-2026-09-12` and
        `welle-fix-2026-09-27` both match the branch prefix `welle-fix`."""
        res = run_strand(self.repo, "new", "kit", "--issue", "756",
                         "--wave", WAVE_DIR)
        self.assertEqual(0, res.returncode, res.stderr)
        other = self.repo / "plans" / SAME_PREFIX_WAVE_DIR / "berichte"
        other.mkdir(parents=True)
        if with_report:
            (other / "kit.md").write_text("# an older strand of the same name\n")
        (self.repo / "plans" / "README.md").write_text(SAME_PREFIX_README)
        plan = pathlib.Path(self._tmp.name) / "plan.tsv"
        plan.write_text(PLAN_GREEN)
        return run_strand(self.repo, "gate", cwd=self.worktree("kit"),
                          extra_env={"MECLAW_GATE_PLAN": str(plan)})

    def test_two_waves_of_one_prefix_the_one_with_the_strands_report_wins(self):
        res = self._second_wave_of_the_prefix(with_report=False)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("strand: wave %s (from the branch)" % WAVE_DIR, res.stderr)
        self.assertTrue(
            (self.repo / "plans" / WAVE_DIR / "receipts" / "kit"
             / "latest" / "summary.txt").is_file(),
            "the gate archive did not land in the wave with the report")
        self.assertFalse(
            (self.repo / "plans" / SAME_PREFIX_WAVE_DIR / "receipts").exists(),
            "the gate archive landed in the other wave of the prefix")

    def test_two_waves_of_one_prefix_that_both_carry_the_report_stay_open(self):
        res = self._second_wave_of_the_prefix(with_report=True)
        self.assertNotIn("(from the branch)", res.stderr)

    def test_every_run_names_the_wave_it_writes_into(self):
        res = run_strand(self.repo, "new", "kit", "--issue", "756")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("strand: wave %s" % WAVE_DIR, res.stderr)


PLAN_GREEN = (
    "ok\tscope-ok\t0\ttrue\t\n"
    "second\tscope-two\t0\ttrue\t\n"
)
PLAN_RED = (
    "ok\tscope-ok\t0\ttrue\t\n"
    "bad\tscope-bad\t0\tfalse\t\n"
)

SUMMARY_LINE = re.compile(
    r"^GATE-SUMMARY (?P<mode>\S+) (?P<rev>\S+) (?P<green>\d+)/(?P<total>\d+) "
    r"(?P<secs>\d+)s (?P<verdict>GREEN|RED|ASK)$")

# A station that ASKS in the passes: the persona-receipt arm of the runner
# turns checker exit 3 (stale) into ASK in integration and release, and the
# run ends with exit 4 and a question block after the summary line.
PLAN_ASK = (
    "persona-receipt\tfingerprint\t0\t"
    "bash -c 'echo \"PERSONA-RECEIPT stale the sources moved\"; exit 3'\t\n"
)

# A stand-in for the runner of the tree: what an ASK run prints, without
# depending on the arm itself. The kit reads the run log, not the runner.
ASK_STUB = """#!/bin/sh
echo 'GATE ok [scope-ok] 0s GREEN'
echo 'GATE persona-receipt [fingerprint] 0s ASK the sources moved'
echo 'GATE-SUMMARY integration deadbeef 1/1 0s ASK'
echo 'gate: ASK -- a question for the owner, not a finding.'
echo '  persona-receipt: the sources moved'
echo '         scripts/gate.sh integration --decide persona-receipt=without:"<reason>"'
exit 4
"""


def ask_stub(tree):
    """Replace the tree's runner by `ASK_STUB`."""
    stub = pathlib.Path(tree) / "scripts" / "gate.sh"
    stub.write_text(ASK_STUB)
    stub.chmod(0o755)


class TestGate(StrandShTestCase):
    def setUp(self):
        super().setUp()
        res = run_strand(self.repo, "new", "kit", "--issue", "756")
        self.assertEqual(0, res.returncode, res.stderr)
        self.tree = self.worktree("kit")

    def plan_file(self, text, name="plan.tsv"):
        path = pathlib.Path(self._tmp.name) / name
        path.write_text(text)
        return path

    def archive_root(self, name="kit"):
        return self.repo / "plans" / WAVE_DIR / "receipts" / name

    def archive(self, name="kit"):
        """The newest run of the strand -- what `latest` points at."""
        return self.archive_root(name) / "latest"

    def runs(self, name="kit"):
        return sorted(d for d in self.archive_root(name).iterdir()
                      if d.is_dir() and not d.is_symlink())

    def run_in_tree(self, plan, *args):
        return run_strand(self.repo, "gate", *args, cwd=self.tree,
                          extra_env={"MECLAW_GATE_PLAN": str(plan)})

    def test_gate_prints_one_summary_line_and_nothing_else(self):
        res = self.run_in_tree(self.plan_file(PLAN_GREEN))
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        lines = [ln for ln in res.stdout.splitlines() if ln.strip()]
        self.assertEqual(1, len(lines),
                         "expected one line, got:\n%s" % res.stdout)
        self.assertTrue(SUMMARY_LINE.match(lines[0]), lines[0])
        self.assertIn("2/2", lines[0])

    def test_gate_archives_receipt_and_logs_next_to_the_wave(self):
        res = self.run_in_tree(self.plan_file(PLAN_GREEN))
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        arc = self.archive()
        self.assertTrue((arc / "last-strand.json").is_file(),
                        "no receipt in %s" % arc)
        self.assertTrue((arc / "logs").is_dir(), "no logs in %s" % arc)
        self.assertTrue((arc / "run.log").is_file(), "no run log in %s" % arc)
        summary = (arc / "summary.txt").read_text().strip()
        self.assertTrue(SUMMARY_LINE.match(summary), summary)

    def test_a_second_run_does_not_overwrite_the_first(self):
        """Every run keeps its own directory.

        The archive was named by strand alone, so a strand that gated twice --
        red, fix, green, or simply a re-run -- wrote `summary.txt`, `run.log`,
        the receipt and every station log of the second run over those of the
        first. That is the material the report is made of; it happened twice in
        one day on 2026-09-21 (GH #802).
        """
        first = self.run_in_tree(self.plan_file(PLAN_RED, name="red.tsv"))
        self.assertEqual(1, first.returncode, first.stdout + first.stderr)
        second = self.run_in_tree(self.plan_file(PLAN_GREEN, name="green.tsv"))
        self.assertEqual(0, second.returncode, second.stdout + second.stderr)

        runs = self.runs()
        self.assertEqual(2, len(runs), "one run overwrote the other: %s" % runs)
        verdicts = sorted((r / "summary.txt").read_text().strip().split()[-1]
                          for r in runs)
        self.assertEqual(["GREEN", "RED"], verdicts)
        for run in runs:
            self.assertTrue((run / "run.log").is_file(), run)
            self.assertTrue((run / "logs").is_dir(), run)

    def test_latest_points_at_the_newest_run(self):
        self.run_in_tree(self.plan_file(PLAN_RED, name="red.tsv"))
        self.run_in_tree(self.plan_file(PLAN_GREEN, name="green.tsv"))
        self.assertIn("GREEN", (self.archive() / "summary.txt").read_text())
        self.assertEqual(self.runs()[-1].resolve(), self.archive().resolve())

    def test_a_run_never_lands_in_a_directory_that_is_already_written(self):
        """Two runs in the same second at the same commit are still two runs."""
        for _ in range(3):
            self.run_in_tree(self.plan_file(PLAN_GREEN))
        self.assertEqual(3, len(self.runs()))

    def test_gate_runs_the_runner_of_the_tree_it_stands_in(self):
        """A strand gates the sources it stands in, whichever kit it called.

        The wave that changes `gate.sh` is exactly the wave in which this
        matters: a strand reaching for the main tree's path would have its own
        sources judged by the main tree's runner.
        """
        marker = pathlib.Path(self._tmp.name) / "tree-gate-ran"
        stub = self.tree / "scripts" / "gate.sh"
        stub.write_text("#!/bin/sh\ntouch %s\n"
                        "echo 'GATE-SUMMARY strand deadbeef 1/1 0s GREEN'\n"
                        % marker)
        stub.chmod(0o755)
        res = run_strand(self.repo, "gate", cwd=self.tree)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertTrue(marker.exists(),
                        "the runner of the main tree ran, not this tree's")

    def test_gate_help_writes_no_archive_and_keeps_latest(self):
        """`--help` is a question, not a run.

        It used to go through to the runner: the kit made a run directory,
        pointed `latest` at it, and the runner printed its help into the run
        log -- a folder without a summary, and a `latest` that `report` could
        no longer read the green gate through (V-kit section 1.4).
        """
        self.assertEqual(0, self.run_in_tree(self.plan_file(PLAN_GREEN)).returncode)
        before = self.runs()
        latest = self.archive().resolve()
        res = run_strand(self.repo, "gate", "--help", cwd=self.tree)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("gate options pass through to scripts/gate.sh", res.stdout)
        self.assertEqual(before, self.runs(), "--help made a run directory")
        self.assertEqual(latest, self.archive().resolve(), "--help moved latest")

    def test_gate_plan_only_prints_the_plan_and_writes_no_archive(self):
        res = self.run_in_tree(self.plan_file(PLAN_GREEN), "--plan-only")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("scope-ok", res.stdout)
        self.assertIn("tests=", res.stdout)
        self.assertFalse(self.archive_root().exists(),
                         "--plan-only archived a run that never ran")

    def test_gate_names_the_red_stations_and_fails(self):
        res = self.run_in_tree(self.plan_file(PLAN_RED))
        self.assertEqual(1, res.returncode, res.stdout + res.stderr)
        lines = [ln for ln in res.stdout.splitlines() if ln.strip()]
        self.assertEqual(2, len(lines),
                         "expected the red station and the summary:\n%s"
                         % res.stdout)
        self.assertTrue(lines[0].startswith("GATE bad "), lines[0])
        self.assertIn(" RED", lines[1])
        # The full run is still on disk -- it just is not in the answer.
        self.assertIn("GATE ok ", (self.archive() / "run.log").read_text())

    def test_gate_passes_an_ask_through_as_four(self):
        """A question for the owner is exit 4, never 3 (no token) and never 0.

        Runs the REAL runner of the tree: the ASK verdict, exit 4 and the
        question block come from its persona-receipt arm.
        """
        (self.tree / "change.txt").write_text("a change\n")
        _git(self.tree, "add", "change.txt")
        _git(self.tree, "commit", "-q", "-m", "a change")
        res = self.run_in_tree(self.plan_file(PLAN_ASK), "integration",
                               "--base", "HEAD~1")
        self.assertEqual(4, res.returncode, res.stdout + res.stderr)
        lines = [ln for ln in res.stdout.splitlines() if ln.strip()]
        self.assertTrue(any(re.match(r"^GATE persona-receipt \[.*\] \d+s ASK", ln)
                            for ln in lines), res.stdout)
        summaries = [ln for ln in lines if SUMMARY_LINE.match(ln)]
        self.assertEqual(1, len(summaries), res.stdout)
        self.assertEqual("ASK", SUMMARY_LINE.match(summaries[0]).group("verdict"))
        self.assertIn('--decide persona-receipt=without:"', res.stdout)

    def test_gate_prints_the_question_block_of_an_ask_run(self):
        """The kit's half of the contract, without the runner's arm: the ASK
        station line, the summary and the question block from the run log."""
        ask_stub(self.tree)
        res = run_strand(self.repo, "gate", "integration", cwd=self.tree)
        self.assertEqual(4, res.returncode, res.stdout + res.stderr)
        lines = [ln for ln in res.stdout.splitlines() if ln.strip()]
        self.assertTrue(lines[0].startswith("GATE persona-receipt "), res.stdout)
        self.assertTrue(SUMMARY_LINE.match(lines[1]), res.stdout)
        self.assertTrue(lines[2].startswith("gate: ASK"), res.stdout)
        self.assertIn('--decide persona-receipt=without:"', res.stdout)
        self.assertNotIn("GATE ok ", res.stdout)


class TestReportAndClose(StrandShTestCase):
    def setUp(self):
        super().setUp()
        res = run_strand(self.repo, "new", "kit", "--issue", "756",
                         "--issue", "755")
        self.assertEqual(0, res.returncode, res.stderr)
        self.tree = self.worktree("kit")

    def plan_file(self, text, name="plan.tsv"):
        path = pathlib.Path(self._tmp.name) / name
        path.write_text(text)
        return path

    def commit(self, rel="scripts/kit_change.txt", text="a change\n"):
        target = self.tree / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)
        _git(self.tree, "add", rel)
        _git(self.tree, "commit", "-q", "-m", "welle-x P1 (#756): eine Änderung")
        return _git(self.tree, "rev-parse", "--short=8", "HEAD").stdout.strip()

    def gate(self, plan_text):
        return run_strand(self.repo, "gate", cwd=self.tree,
                          extra_env={"MECLAW_GATE_PLAN":
                                     str(self.plan_file(plan_text))})

    def test_report_fills_gate_and_commits_from_the_archive(self):
        sha = self.commit()
        self.assertEqual(0, self.gate(PLAN_GREEN).returncode)
        res = run_strand(self.repo, "report", cwd=self.tree)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        head = header_block(self.report("kit"))
        self.assertIn("GATE-SUMMARY", head["gate"])
        self.assertIn("GREEN", head["gate"])
        self.assertEqual("[%s]" % sha, head["commits"])

    def test_report_takes_the_gate_line_of_the_newest_run(self):
        self.commit()
        self.assertEqual(1, self.gate(PLAN_RED).returncode)
        self.assertEqual(0, self.gate(PLAN_GREEN).returncode)
        res = run_strand(self.repo, "report", cwd=self.tree)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("GREEN", header_block(self.report("kit"))["gate"])

    def test_report_still_reads_an_archive_written_before_the_per_run_layout(self):
        """Flat archives of earlier strands are read where they lie."""
        self.commit()
        self.assertEqual(0, self.gate(PLAN_GREEN).returncode)
        root = self.repo / "plans" / WAVE_DIR / "receipts" / "kit"
        run = [d for d in root.iterdir() if d.is_dir() and not d.is_symlink()][0]
        (root / "summary.txt").write_text((run / "summary.txt").read_text())
        (root / "latest").unlink()
        shutil.rmtree(run)
        res = run_strand(self.repo, "report", cwd=self.tree)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("GREEN", header_block(self.report("kit"))["gate"])

    def test_report_refuses_a_red_gate(self):
        self.commit()
        self.assertEqual(1, self.gate(PLAN_RED).returncode)
        res = run_strand(self.repo, "report", cwd=self.tree)
        self.assertEqual(1, res.returncode, res.stdout)
        self.assertIn("RED", res.stderr)

    def test_report_refuses_an_ask_gate(self):
        """ASK is no green: a question for the owner is still open."""
        self.commit()
        ask_stub(self.tree)
        self.assertEqual(4, self.gate(PLAN_GREEN).returncode)
        res = run_strand(self.repo, "report", cwd=self.tree)
        self.assertEqual(1, res.returncode, res.stdout)
        self.assertIn("ASK", res.stderr)
        self.assertIn("not GREEN", res.stderr)

    def test_close_refuses_an_ask_gate(self):
        self.commit()
        self.assertEqual(0, self.gate(PLAN_GREEN).returncode)
        self.assertEqual(0, run_strand(self.repo, "report",
                                       cwd=self.tree).returncode)
        path = self.report("kit")
        text = path.read_text()
        gate = header_block(path)["gate"]
        path.write_text(text.replace(
            "gate: %s" % gate,
            'gate: "GATE-SUMMARY integration abc1234 0/0 1s ASK"', 1))
        self.assertIn("ASK", header_block(path)["gate"])
        res = run_strand(self.repo, "close", cwd=self.tree)
        self.assertNotEqual(0, res.returncode, res.stdout)
        self.assertIn("ASK", res.stderr)
        self.assertNotIn("gh issue close", res.stdout)

    def test_report_refuses_a_header_without_a_gate(self):
        self.commit()
        res = run_strand(self.repo, "report", cwd=self.tree)
        self.assertEqual(1, res.returncode, res.stdout)
        self.assertIn("gate", res.stderr)

    def test_close_prints_an_english_comment_per_issue_with_the_gate_line(self):
        self.commit()
        self.assertEqual(0, self.gate(PLAN_GREEN).returncode)
        self.assertEqual(0, run_strand(self.repo, "report",
                                       cwd=self.tree).returncode)
        # German prose with a name in it -- exactly what must NOT travel.
        path = self.report("kit")
        path.write_text(path.read_text().replace(
            "## Änderungen\n\n(Datei:Zeile, WARUM)",
            "## Änderungen\n\n- `scripts/kit_change.txt` -- weil der Besitzer "
            "Hinrichsen es am Gerät 127.0.0.1:9999 so sah"))
        res = run_strand(self.repo, "close", cwd=self.tree)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("gh issue close 756 --comment", res.stdout)
        self.assertIn("gh issue close 755 --comment", res.stdout)
        self.assertIn("GATE-SUMMARY", res.stdout)
        self.assertIn("scripts/kit_change.txt", res.stdout)
        for leak in ("Hinrichsen", "127.0.0.1", "Besitzer"):
            self.assertNotIn(leak, res.stdout,
                             "the closing comment carries %r:\n%s"
                             % (leak, res.stdout))

    def foreign_master_commit(self, rel="other/foreign.txt"):
        """A commit on master that belongs to ANOTHER strand of the wave."""
        target = self.repo / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text("not this strand\n")
        _git(self.repo, "add", rel)
        _git(self.repo, "commit", "-q", "-m", "welle-x P9 (#999): fremder Strang")
        return _git(self.repo, "rev-parse", "--short=8", "HEAD").stdout.strip()

    def test_report_reads_the_branch_not_the_calling_trees_head(self):
        sha = self.commit()
        self.assertEqual(0, self.gate(PLAN_GREEN).returncode)
        self.foreign_master_commit()
        res = run_strand(self.repo, "report", "--strand", "kit", cwd=self.repo)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        head = header_block(self.report("kit"))
        self.assertEqual("[%s]" % sha, head["commits"])

    def test_report_leaves_out_what_a_merge_of_master_brought_in(self):
        """A fix round merges master first, and that is not the strand's work.

        Without `--first-parent` the header claimed 25 commits for a strand
        that wrote 12: the thirteen the merge carried in belong to the other
        strands of the wave, and a receipt table built from that names the
        wrong author for every one of them.
        """
        mine = self.commit()
        foreign = self.foreign_master_commit()
        _git(self.tree, "merge", "master", "-m", "Merge master in den Strang")
        merge = _git(self.tree, "rev-parse", "--short=8", "HEAD").stdout.strip()
        self.assertEqual(0, self.gate(PLAN_GREEN).returncode)
        res = run_strand(self.repo, "report", cwd=self.tree)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        head = header_block(self.report("kit"))
        self.assertEqual("[%s, %s]" % (mine, merge), head["commits"])
        self.assertNotIn(foreign, head["commits"])

    def test_report_after_a_rebase_takes_the_new_base_and_only_own_commits(self):
        """A strand rebases onto master before its cargo phase (PREAMBLE
        section 5), and the base of its skeleton is then a commit below the
        new master. `basis..tip --first-parent` walked master's first-parent
        line down from there and listed every commit and merge of the other
        strands: eight head blocks of one wave were corrected by hand."""
        self.commit()
        foreign = self.foreign_master_commit()
        _git(self.repo, "checkout", "-q", "-b", "%s/other" % WAVE)
        other = self.foreign_master_commit(rel="other/second.txt")
        _git(self.repo, "checkout", "-q", "master")
        _git(self.repo, "merge", "-q", "--no-ff", "-m",
             "Merge branch '%s/other'" % WAVE, "%s/other" % WAVE)
        tip = _git(self.repo, "rev-parse", "--short=8", "master").stdout.strip()
        _git(self.tree, "rebase", "-q", "master")
        mine = _git(self.tree, "rev-parse", "--short=8", "HEAD").stdout.strip()
        self.assertEqual(0, self.gate(PLAN_GREEN).returncode)
        res = run_strand(self.repo, "report", cwd=self.tree)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        head = header_block(self.report("kit"))
        self.assertEqual(tip, head["basis"])
        self.assertEqual("[%s]" % mine, head["commits"])
        for sha in (foreign, other, tip):
            self.assertNotIn(sha, head["commits"])

    def test_report_after_the_merge_keeps_the_basis(self):
        """Once the branch is in master, its merge-base IS its tip -- the base
        of the head block is the one that still says where it started."""
        sha = self.commit()
        self.assertEqual(0, self.gate(PLAN_GREEN).returncode)
        self.assertEqual(0, run_strand(self.repo, "report",
                                       cwd=self.tree).returncode)
        basis = header_block(self.report("kit"))["basis"]
        _git(self.repo, "merge", "-q", "--no-ff", "-m",
             "Merge branch '%s/kit'" % WAVE, "%s/kit" % WAVE)
        self.foreign_master_commit()
        res = run_strand(self.repo, "report", "--strand", "kit", cwd=self.repo)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        head = header_block(self.report("kit"))
        self.assertEqual(basis, head["basis"])
        self.assertEqual("[%s]" % sha, head["commits"])

    def test_close_reads_the_branch_not_the_calling_trees_head(self):
        self.commit()
        self.assertEqual(0, self.gate(PLAN_GREEN).returncode)
        self.assertEqual(0, run_strand(self.repo, "report",
                                       cwd=self.tree).returncode)
        self.foreign_master_commit()
        res = run_strand(self.repo, "close", "--strand", "kit", cwd=self.repo)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("scripts/kit_change.txt", res.stdout)
        self.assertNotIn("other/foreign.txt", res.stdout)

    def test_close_after_the_merge_falls_back_to_the_merge_commit(self):
        """The orchestrator's normal case: merged, branch gone, tree is main."""
        self.commit()
        self.assertEqual(0, self.gate(PLAN_GREEN).returncode)
        self.assertEqual(0, run_strand(self.repo, "report",
                                       cwd=self.tree).returncode)
        _git(self.repo, "merge", "--no-ff", "-m",
             "Merge branch '%s/kit'" % WAVE, "%s/kit" % WAVE)
        _git(self.repo, "worktree", "remove", "--force", str(self.tree))
        _git(self.repo, "branch", "-D", "%s/kit" % WAVE)
        self.foreign_master_commit()
        res = run_strand(self.repo, "close", "--strand", "kit", cwd=self.repo)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("scripts/kit_change.txt", res.stdout)
        self.assertNotIn("other/foreign.txt", res.stdout)
        self.assertIn("Merged as ", res.stdout)

    def test_close_does_not_run_gh_without_do(self):
        self.commit()
        self.assertEqual(0, self.gate(PLAN_GREEN).returncode)
        self.assertEqual(0, run_strand(self.repo, "report",
                                       cwd=self.tree).returncode)
        marker = pathlib.Path(self._tmp.name) / "gh-was-called"
        bindir = pathlib.Path(self._tmp.name) / "bin"
        bindir.mkdir(exist_ok=True)
        fake = bindir / "gh"
        fake.write_text("#!/bin/sh\ntouch %s\n" % marker)
        fake.chmod(0o755)
        env = {"PATH": "%s:%s" % (bindir, os.environ["PATH"])}
        res = run_strand(self.repo, "close", cwd=self.tree, extra_env=env)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertFalse(marker.exists(), "close ran gh without --do")


REFUSED = "no cargo token for"


class TokenTestCase(StrandShTestCase):
    """A throw-away token file next to a throw-away cargo lock.

    Nothing here reaches the host's file: `kit_env` SETS the path for every
    call, and `test_no_case_touches_the_hosts_token_file` proves it.
    """

    T0 = 1_800_000_000

    def setUp(self):
        super().setUp()
        res = run_strand(self.repo, "new", "kit", "--issue", "861")
        self.assertEqual(0, res.returncode, res.stderr)
        self.tree = self.worktree("kit")

    def token(self, *args, cwd=None, minutes=0, env=None):
        extra = {"MECLAW_STRAND_NOW": str(self.T0 + 60 * minutes)}
        if env:
            extra.update(env)
        return run_strand(self.repo, "token", *args, cwd=cwd, extra_env=extra)

    def arm(self, *args):
        res = self.token("init", *args)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)

    def take(self, strand, minutes=0, cwd=None):
        args = ("take",) if strand is None else ("take", "--strand", strand)
        return self.token(*args, cwd=cwd, minutes=minutes)

    def state(self):
        return json.loads((self.repo.parent / "tokens").read_text())

    def holders(self):
        return [h["strand"] for h in self.state()["holders"]]

    def waiting(self):
        return [w["strand"] for w in self.state()["waiting"]]

    def log(self):
        path = self.repo.parent / "tokens.log"
        return path.read_text() if path.exists() else ""

    def plan_file(self, text=PLAN_GREEN, name="plan.tsv"):
        path = pathlib.Path(self._tmp.name) / name
        path.write_text(text)
        return path

    def gate(self, cwd=None, *args):
        return run_strand(self.repo, "gate", *args, cwd=cwd or self.tree,
                          extra_env={"MECLAW_GATE_PLAN": str(self.plan_file())})


class TestToken(TokenTestCase):
    """The cargo token of a wave (`scripts/strand.sh token`, GH #861).

    Measured in the wave before it: eleven builders queued on the cargo lock
    for 20-90 minutes per single test, and 103 of them woke to a cold cache
    in three hours -- the lock serialises builds, it does not limit how many
    wait. The token is the limit, written down where every tree reads it.
    """

    def test_an_unarmed_host_needs_no_token(self):
        res = self.gate()
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertEqual(1, len([l for l in res.stdout.splitlines() if l.strip()]))
        self.assertNotIn("token", res.stderr)
        res = run_tier(self.repo, self.tree)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertNotIn("token", res.stderr)
        self.assertFalse((self.repo.parent / "tokens").exists())

    def test_take_grants_up_to_max_and_queues_the_rest(self):
        self.arm("--max", "2")
        self.assertEqual(0, self.take("welle-x/a").returncode)
        self.assertEqual(0, self.take("welle-x/b").returncode)
        res = self.take("welle-x/c")
        self.assertEqual(3, res.returncode, res.stderr)
        self.assertIn("all 2 cargo tokens are held", res.stderr)
        self.assertIn("welle-x/c is #1 in the queue", res.stderr)
        self.assertIn("End your turn", res.stderr)
        self.assertEqual(["welle-x/a", "welle-x/b"], self.holders())
        self.assertEqual(["welle-x/c"], self.waiting())

    def test_release_names_the_next_in_the_queue(self):
        self.arm("--max", "1")
        self.take("welle-x/a")
        self.take("welle-x/b", minutes=2)
        res = self.token("release", "--strand", "welle-x/a", minutes=23)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("strand: cargo token released by welle-x/a after 23 min "
                      "-- 0/1 held; next in the queue: welle-x/b (waiting 21 min)",
                      res.stderr)
        self.assertIn("(released by master)", res.stderr)
        self.assertIn("\trelease-by\t", self.log())

    def test_the_queue_is_first_come_first_served(self):
        self.arm("--max", "1")
        self.take("welle-x/a")
        self.assertEqual(3, self.take("welle-x/b").returncode)
        self.assertEqual(3, self.take("welle-x/c").returncode)
        self.token("release", "--strand", "welle-x/a")
        res = self.take("welle-x/c")
        self.assertEqual(3, res.returncode, "a later strand overtook the queue")
        self.assertIn("welle-x/c is #2 in the queue", res.stderr)
        self.assertEqual(0, self.take("welle-x/b").returncode)
        self.assertEqual(["welle-x/b"], self.holders())

    def test_concurrent_takes_never_exceed_max(self):
        """Eight takes at once against three tokens: exactly three win."""
        self.arm("--max", "3")
        env = kit_env(self.repo, {"MECLAW_STRAND_NOW": str(self.T0)})
        procs = [subprocess.Popen(
            [str(self.repo / "scripts" / "strand.sh"), "token", "take",
             "--strand", "welle-x/s%d" % i],
            cwd=str(self.repo), env=env, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE) for i in range(8)]
        codes = sorted(p.wait(timeout=120) for p in procs)
        for p in procs:
            p.stdout.close()
            p.stderr.close()
        self.assertEqual([0, 0, 0, 3, 3, 3, 3, 3], codes)
        self.assertEqual(3, len(self.holders()))
        self.assertEqual(5, len(self.waiting()))

    def test_a_stale_holder_is_reclaimed(self):
        self.arm("--max", "1")
        self.take("welle-x/a")
        res = self.take("welle-x/b", minutes=91)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("reclaimed the cargo token of welle-x/a -- last seen 91 min "
                      "ago, no live process (ttl 90 min)", res.stderr)
        self.assertEqual(["welle-x/b"], self.holders())
        self.assertIn("\tstale\twelle-x/a", self.log())

    def test_a_holder_with_a_live_process_is_not_stale(self):
        """A gate that runs longer than the TTL keeps its token."""
        self.arm("--max", "1")
        self.take("welle-x/a")
        res = self.token("check", "--strand", "welle-x/a", "--pid",
                         str(os.getpid()))
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual(3, self.take("welle-x/b", minutes=120).returncode)
        self.assertEqual(["welle-x/a"], self.holders())

    def test_a_holder_whose_worktree_is_gone_is_stale(self):
        self.arm("--max", "1")
        res = self.take(None, cwd=self.tree)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual(["%s/kit" % WAVE], self.holders())
        _git(self.repo, "worktree", "remove", "--force", str(self.tree))
        res = self.take("welle-x/b")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("its worktree is gone", res.stderr)

    def test_a_dead_waiter_does_not_block_the_queue(self):
        """A waiter nobody wakes any more held place 1 for everybody behind
        it (review rev-1 I3): waiters expire like holders."""
        self.arm("--max", "1")
        self.take("welle-x/a")
        self.assertEqual(3, self.take("welle-x/w").returncode)
        self.token("check", "--strand", "welle-x/a", minutes=94)
        self.token("release", "--strand", "welle-x/a", minutes=95)
        res = self.take("welle-x/c", minutes=95)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual([], self.waiting())
        self.assertIn("\tstale-wait\twelle-x/w", self.log())

    def test_a_waiter_with_a_live_worktree_never_expires(self):
        """A strand that waits in its own worktree is alive as long as the
        tree is: the TTL is for waiters queued from outside (review I1). With
        three tokens for eight strands a wait over 90 min is the normal case,
        and the drop landed at another strand's `take`, unseen by the one
        that lost its place."""
        self.arm("--max", "1")
        self.take("welle-x/a")
        self.token("check", "--strand", "welle-x/a", "--pid", str(os.getpid()))
        res = self.take(None, cwd=self.tree)
        self.assertEqual(3, res.returncode, res.stderr)
        self.assertIn("%s/kit is #1 in the queue" % WAVE, res.stderr)
        self.assertEqual(3, self.take("welle-x/x", minutes=10).returncode)
        res = self.take("welle-x/x", minutes=95)
        self.assertEqual(3, res.returncode, res.stderr)
        self.assertNotIn("dropped", res.stderr)
        self.assertEqual(["%s/kit" % WAVE, "welle-x/x"], self.waiting())
        res = self.token("release", "--strand", "welle-x/a", minutes=96)
        self.assertIn("next in the queue: %s/kit" % WAVE, res.stderr)
        res = self.take(None, cwd=self.tree, minutes=97)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual(["%s/kit" % WAVE], self.holders())

    def test_an_unarmed_host_gets_no_lock_file(self):
        """Unarmed is without a trace: no verb but `init` creates a file."""
        for args in (("take", "--strand", "welle-x/a"),
                     ("release", "--strand", "welle-x/a"),
                     ("check", "--strand", "welle-x/a", "--pid", "0"),
                     ("who",), ("off",)):
            res = self.token(*args)
            self.assertEqual(0, res.returncode, "%s: %s" % (args, res.stderr))
        self.assertEqual([], sorted(p.name for p in self.repo.parent.iterdir()
                                    if p.name.startswith("tokens")))

    def _gate_and_tier(self, first, second):
        """Two heartbeats of one strand, then the first process lives on and
        the second one is gone."""
        self.token("init", "--max", "1", "--force")
        self.assertEqual(0, self.take(None, cwd=self.tree).returncode)
        for pid in (first, second):
            res = self.token("check", "--strand", "%s/kit" % WAVE, "--pid", str(pid))
            self.assertEqual(0, res.returncode, res.stderr)

    def test_a_single_test_beside_a_running_gate_keeps_the_gates_pid(self):
        """A tier started beside a background gate wrote its own pid over the
        gate's; after the tier the entry named a dead process, and a gate
        running on past the TTL lost its token (review M2)."""
        gate = os.getpid()
        for order in ("tier after the gate", "gate after the tier"):
            with self.subTest(order=order):
                tier = subprocess.Popen([sys.executable, "-c",
                                         "import time; time.sleep(120)"])
                try:
                    pair = (gate, tier.pid) if order.startswith("tier") else (tier.pid, gate)
                    self._gate_and_tier(*pair)
                finally:
                    tier.kill()
                    tier.wait()
                res = self.take("welle-x/b", minutes=120)
                self.assertEqual(3, res.returncode, res.stderr)
                self.assertEqual(["%s/kit" % WAVE], self.holders())

    def test_the_heartbeat_after_a_foreign_release_logs_no_refusal(self):
        """The orchestrator released the token while the gate ran; the
        heartbeat after the run is no refusal of anything (review M3)."""
        self.arm()
        self.assertEqual(0, self.take(None, cwd=self.tree).returncode)
        # The one station of this gate is the release, from the side.
        release = "%s token release --strand %s/kit" % (
            self.repo / "scripts" / "strand.sh", WAVE)
        plan = self.plan_file("released\tscope-rel\t0\t%s\t\n" % release,
                              name="release.tsv")
        res = run_strand(self.repo, "gate", cwd=self.tree,
                         extra_env={"MECLAW_GATE_PLAN": str(plan)})
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertEqual([], self.holders())
        self.assertNotIn("\trefused\t", self.log())

    def test_init_refuses_over_waiters_without_force(self):
        """A re-init in the middle of a wave emptied the queue without a word
        (review M6)."""
        self.arm("--max", "1")
        self.take("welle-x/a")
        self.take("welle-x/b")
        self.token("release", "--strand", "welle-x/a")
        res = self.token("init")
        self.assertEqual(2, res.returncode, res.stderr)
        self.assertIn("welle-x/b", res.stderr)
        self.assertIn("--force", res.stderr)
        self.assertEqual(["welle-x/b"], self.waiting())

    def test_off_refuses_over_waiters_without_force(self):
        """`off` dropped the queue without a word -- the counterpart of the
        re-init of review M6 (review fix round 1, m3)."""
        self.arm("--max", "1")
        self.take("welle-x/a")
        self.take("welle-x/b")
        self.token("release", "--strand", "welle-x/a")
        res = self.token("off")
        self.assertEqual(2, res.returncode, res.stderr)
        self.assertIn("welle-x/b waits in the queue", res.stderr)
        self.assertIn("--force", res.stderr)
        self.assertEqual(["welle-x/b"], self.waiting())
        res = self.token("off", "--force")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertFalse((self.repo.parent / "tokens").exists())
        self.assertIn("\toff --force\t", self.log())

    def test_off_names_holders_and_waiters_together(self):
        self.arm("--max", "1")
        self.take("welle-x/a")
        self.take("welle-x/b")
        res = self.token("off")
        self.assertEqual(2, res.returncode, res.stderr)
        self.assertIn("welle-x/a holds a cargo token", res.stderr)
        self.assertIn("welle-x/b waits in the queue", res.stderr)
        self.assertEqual(["welle-x/a"], self.holders())

    def test_who_marks_a_waiter_beyond_the_ttl_and_keeps_it(self):
        """A builder that dies while it waits leaves its tree standing, and
        its entry blocks everybody behind it until `release --strand` (the
        other side of review I1). `who` says so after the TTL -- a hint only,
        the entry stays (review fix round 1, m1)."""
        self.arm("--max", "1")
        self.take("welle-x/a")
        self.token("check", "--strand", "welle-x/a", "--pid", str(os.getpid()))
        self.assertEqual(3, self.take(None, cwd=self.tree).returncode)
        res = self.token("who", minutes=90)
        self.assertNotIn("beyond the ttl", res.stdout)
        res = self.token("who", minutes=95)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("#1 %s/kit  waiting 95 min  waiting beyond the ttl" % WAVE,
                      res.stdout)
        self.assertNotIn("STALE", res.stdout)
        self.assertEqual(["%s/kit" % WAVE], self.waiting())

    def test_a_waiter_whose_worktree_is_gone_leaves_the_queue(self):
        """The one automatic exit of a waiter with a tree (review I1): the
        tree is gone, the entry goes at the next `take`, and the one behind
        it moves up (review fix round 1, m2a)."""
        self.arm("--max", "1")
        self.take("welle-x/a")
        self.token("check", "--strand", "welle-x/a", "--pid", str(os.getpid()))
        self.assertEqual(3, self.take(None, cwd=self.tree).returncode)
        self.assertEqual(3, self.take("welle-x/x", minutes=1).returncode)
        _git(self.repo, "worktree", "remove", "--force", str(self.tree))
        res = self.take("welle-x/x", minutes=300)
        self.assertEqual(3, res.returncode, res.stderr)
        self.assertIn("dropped %s/kit from the queue -- its worktree is gone" % WAVE,
                      res.stderr)
        self.assertIn("welle-x/x is #1 in the queue", res.stderr)
        self.assertEqual(["welle-x/x"], self.waiting())
        self.assertIn("\tstale-wait\t%s/kit" % WAVE, self.log())

    def _old_form(self, holders):
        """A token file of the first form: one `pid` per holder."""
        (self.repo.parent / "tokens").write_text(json.dumps({
            "max": 3, "ttl_min": 90, "armed": "2027-01-15T08:00:00Z",
            "holders": holders, "waiting": []}))

    def test_load_turns_the_old_pid_field_into_the_list(self):
        """The first form of the file kept one `pid` per holder; `load` reads
        it as `pids` (review fix round 1, m2b)."""
        self.arm()
        dead = subprocess.Popen([sys.executable, "-c", "pass"])
        dead.wait()
        old = lambda s, pid: {"strand": s, "since": self.T0, "seen": self.T0,
                              "pid": pid, "tree": ""}
        self._old_form([old("welle-x/live", os.getpid()),
                        old("welle-x/dead", dead.pid),
                        old("welle-x/none", None)])
        res = self.token("check", "--strand", "welle-x/none", minutes=120)
        self.assertEqual(0, res.returncode, res.stderr)
        res = self.take("welle-x/c", minutes=120)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("reclaimed the cargo token of welle-x/dead -- last seen "
                      "120 min ago, no live process", res.stderr)
        by = {h["strand"]: h for h in self.state()["holders"]}
        self.assertEqual(["welle-x/live", "welle-x/none", "welle-x/c"], list(by))
        self.assertEqual([os.getpid()], by["welle-x/live"]["pids"])
        self.assertEqual([], by["welle-x/none"]["pids"])
        self.assertNotIn("pid", by["welle-x/live"])

    def test_load_refuses_a_broken_old_pid_field(self):
        for name, holder in (
                ("a pid that is no number", {"strand": "welle-x/a", "since": 0,
                                             "seen": 0, "pid": "abc"}),
                ("pids that are no list", {"strand": "welle-x/a", "since": 0,
                                           "seen": 0, "pids": 5}),
                ("a holder that is a string", "welle-x/a")):
            with self.subTest(form=name):
                self.arm("--force")
                self._old_form([holder])
                before = (self.repo.parent / "tokens").read_text()
                res = self.take("welle-x/b")
                self.assertEqual(2, res.returncode, res.stderr)
                self.assertIn("is broken", res.stderr)
                self.assertEqual(before, (self.repo.parent / "tokens").read_text())

    def test_release_also_leaves_the_queue(self):
        self.arm("--max", "1")
        self.take("welle-x/a")
        self.take("welle-x/b")
        res = self.token("release", "--strand", "welle-x/b")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual([], self.waiting())
        self.assertEqual(["welle-x/a"], self.holders())

    def test_the_skip_variable_is_logged(self):
        self.arm()
        res = self.token("check", cwd=self.tree,
                         env={"MECLAW_STRAND_TOKEN_SKIP": "a review reruns one test"})
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("strand: token check skipped for %s/kit: a review reruns "
                      "one test" % WAVE, res.stderr)
        self.assertIn("\tskip\t%s/kit" % WAVE, self.log())

    def test_init_refuses_over_holders_without_force(self):
        self.arm()
        self.take("welle-x/a")
        res = self.token("init")
        self.assertEqual(2, res.returncode, res.stderr)
        self.assertEqual(["welle-x/a"], self.holders())
        self.assertEqual(0, self.token("init", "--force").returncode)
        self.assertEqual([], self.holders())

    def test_who_names_holders_the_queue_and_the_last_events(self):
        self.arm("--max", "1")
        self.take("welle-x/a")
        self.take("welle-x/b", minutes=3)
        res = self.token("who", minutes=12)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("welle-x/a", res.stdout)
        self.assertIn("12 min", res.stdout)
        self.assertIn("#1 welle-x/b", res.stdout)
        self.assertIn("wait", res.stdout)

    def test_a_broken_token_file_is_never_rewritten_in_silence(self):
        self.arm()
        (self.repo.parent / "tokens").write_text("{not json")
        res = self.take("welle-x/a")
        self.assertEqual(2, res.returncode, res.stderr)
        self.assertIn(str(self.repo.parent / "tokens"), res.stderr)
        self.assertEqual("{not json", (self.repo.parent / "tokens").read_text())

    def test_off_disarms_and_keeps_the_log(self):
        self.arm()
        self.assertEqual(0, self.token("off").returncode)
        self.assertFalse((self.repo.parent / "tokens").exists())
        self.assertIn("\toff\t", self.log())
        res = self.take("welle-x/a")
        self.assertEqual(0, res.returncode)
        self.assertIn("not armed", res.stderr)

    def test_no_case_touches_the_hosts_token_file(self):
        """Every verb, through the same environment as every other case, and
        not a trace of it in the host's file or log."""
        probe = "hostprobe-%s/probe" % uuid.uuid4().hex[:12]
        self.arm()
        for args in (("take", "--strand", probe),
                     ("check", "--strand", probe, "--pid", "0"),
                     ("who",),
                     ("release", "--strand", probe),
                     ("off",)):
            res = self.token(*args)
            self.assertEqual(0, res.returncode, "%s: %s" % (args, res.stderr))
        self.assertIn(probe, self.log())
        for host in (HOST_TOKENS, pathlib.Path(str(HOST_TOKENS) + ".log")):
            if host.is_file():
                self.assertNotIn(probe, host.read_text(errors="replace"),
                                 "a test reached %s" % host)


LANES_FILE = """# name  ssh target  [bmc]
north  builder@north.example.invalid  bmc-north.example.invalid
south  builder@south.example.invalid
spare  builder@spare.example.invalid
"""


class TestTokenLanes(TokenTestCase):
    """Lanes are named tokens (GH #934): one per build host.

    A strand that takes a lane learns WHICH host its gate runs on; the order
    of `--lanes` is the order of hand-out, so the spare host stands last.
    Everything else -- queue, TTL, exit 3, `release` naming the next -- is the
    token of GH #861, unchanged.
    """

    def setUp(self):
        super().setUp()
        (self.repo.parent / "lanes").write_text(LANES_FILE)

    def test_take_hands_out_lanes_in_the_order_of_init(self):
        self.arm("--lanes", "south,north")
        res = self.take("welle-x/a")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual("lane south", res.stdout.strip())
        res = self.take("welle-x/b")
        self.assertEqual("lane north", res.stdout.strip())
        res = self.take("welle-x/c")
        self.assertEqual(3, res.returncode, res.stderr)
        self.assertIn("welle-x/c is #1 in the queue", res.stderr)
        self.assertEqual(["south", "north"],
                         [h["lane"] for h in self.state()["holders"]])

    def test_a_released_lane_goes_to_the_next(self):
        self.arm("--lanes", "north,south")
        self.take("welle-x/a")
        self.take("welle-x/b")
        self.assertEqual(3, self.take("welle-x/c").returncode)
        res = self.token("release", "--strand", "welle-x/a")
        self.assertIn("next in the queue: welle-x/c", res.stderr)
        res = self.take("welle-x/c")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual("lane north", res.stdout.strip())

    def test_take_again_names_the_held_lane(self):
        self.arm("--lanes", "north")
        self.take("welle-x/a")
        res = self.take("welle-x/a")
        self.assertEqual(0, res.returncode)
        self.assertEqual("lane north", res.stdout.strip())

    def test_who_names_the_lanes(self):
        self.arm("--lanes", "north,south")
        self.take("welle-x/a")
        res = self.token("who")
        self.assertIn("lanes: north south", res.stdout)
        self.assertRegex(res.stdout, r"welle-x/a\s+lane north")

    def test_token_lane_prints_the_held_lane(self):
        self.arm("--lanes", "north,south")
        self.assertEqual("", self.token("lane", "--strand", "welle-x/a").stdout.strip())
        self.take("welle-x/a")
        res = self.token("lane", "--strand", "welle-x/a")
        self.assertEqual(0, res.returncode)
        self.assertEqual("north", res.stdout.strip())

    def test_lanes_and_max_together_refuse(self):
        res = self.token("init", "--lanes", "north", "--max", "2")
        self.assertEqual(2, res.returncode)
        self.assertIn("--lanes", res.stderr)
        self.assertFalse((self.repo.parent / "tokens").exists())

    def test_a_lane_missing_in_the_host_file_refuses(self):
        res = self.token("init", "--lanes", "north,west")
        self.assertEqual(2, res.returncode)
        self.assertIn("west", res.stderr)
        self.assertFalse((self.repo.parent / "tokens").exists())

    def test_without_a_host_file_lanes_refuse(self):
        (self.repo.parent / "lanes").unlink()
        res = self.token("init", "--lanes", "north")
        self.assertEqual(2, res.returncode)
        self.assertIn("lanes", res.stderr)

    def test_a_lane_name_twice_refuses(self):
        res = self.token("init", "--lanes", "north,north")
        self.assertEqual(2, res.returncode)

    def test_max_alone_is_the_old_token(self):
        self.arm("--max", "2")
        res = self.take("welle-x/a")
        self.assertEqual("", res.stdout.strip())
        self.assertNotIn("lane", self.state()["holders"][0])
        self.assertNotIn("lanes", self.state())

    def test_check_with_a_lane_wants_exactly_that_lane(self):
        self.arm("--lanes", "north,south")
        self.take("welle-x/a")
        ok = self.token("check", "--strand", "welle-x/a", "--lane", "north")
        self.assertEqual(0, ok.returncode, ok.stderr)
        bad = self.token("check", "--strand", "welle-x/a", "--lane", "south")
        self.assertEqual(3, bad.returncode)
        self.assertIn("holds lane north, not south", bad.stderr)


# A fake `ssh` for the lane tests: it drops the options, logs the call and
# runs the remote command HERE -- so `git push`, `rsync` and the remote run
# all go through the code paths of a real host, against a directory of this
# test. A host whose name says `unreachable` answers like a dead one.
FAKE_SSH = """#!/usr/bin/env bash
while [ $# -gt 0 ]; do
    case "$1" in
        -o|-p|-l|-i|-E|-F) shift 2 ;;
        -*) shift ;;
        *) break ;;
    esac
done
host="$1"; shift
printf '%s\\t%s\\n' "$host" "$*" >>"$FAKE_SSH_LOG"
case "$host" in
    *unreachable*) echo "ssh: connect to host $host: Connection timed out" >&2; exit 255 ;;
esac
exec bash -c "$*"
"""

HOST_LANES = """north  builder@north.example.invalid
south  builder@south.example.invalid
ghost  builder@unreachable.example.invalid
"""

# A fake `rsync` beside the fake `ssh`: the real one, unless the test asks
# it to drop the overlay (FAKE_RSYNC_DROP: exit 0 and send nothing -- a
# host that silently got a different tree) or to fail on a path
# (FAKE_RSYNC_FAIL=<substring>: the archive does not come back).
FAKE_RSYNC = """#!/usr/bin/env bash
for a in "$@"; do
    case "$a" in
        --files-from=*) [ -n "${FAKE_RSYNC_DROP:-}" ] && exit 0 ;;
    esac
    if [ -n "${FAKE_RSYNC_FAIL:-}" ]; then
        case "$a" in *"$FAKE_RSYNC_FAIL"*) echo "rsync: fake failure" >&2; exit 23 ;; esac
    fi
done
exec %s "$@"
"""

PLAN_HOST = "ok\tscope-ok\t0\ttrue\t\nbuild\tworkspace\t1\ttrue\t\n"
PLAN_HOST_RED = "ok\tscope-ok\t0\ttrue\t\nbad\tscope-bad\t0\tfalse\t\n"
# `scenarios:memory` reads `.env`: skipped on the lane, run here afterwards.
PLAN_HOST_ENV = ("ok\tscope-ok\t0\ttrue\t\n"
                 "scenarios:memory\t1 case\t0\t%s\t\n")
# OR-S3-96: no `node` and no `github-main` ref on a host either -- and the
# audit, run here, reads the receipt and must find the lane's `ok` in it.
PLAN_HOST_LOCAL = ("ok\tscope-ok\t0\ttrue\t\n"
                   "display-lab\tunittest\t0\ttrue\t\n"
                   "export-audit\tR1-R17 dry\t0\tpython3 -c \"import json,sys; "
                   "d=json.load(open(sys.argv[1])); "
                   "v={s['name']: s['verdict'] for s in d['stations']}; "
                   "sys.exit(0 if v.get('ok') == 'GREEN' else 1)\" {receipt}\t\n")


@unittest.skipUnless(shutil.which("rsync"), "the lane overlay needs rsync")
class TestGateHost(TokenTestCase):
    """`strand.sh gate --host <lane>` (GH #934): the gate on a build host.

    The commit travels by `git push` into a bare repository on the host, the
    uncommitted rest (staged or not, against HEAD) by an rsync overlay --
    never a file whose name says secret, never a gitignored one; a tracked
    secret refuses the gate -- and the archive comes back to the wave as if
    the run had been local.
    """

    def setUp(self):
        super().setUp()
        tmp = pathlib.Path(self._tmp.name)
        (self.repo.parent / "lanes").write_text(HOST_LANES)
        self.bin = tmp / "bin"
        self.bin.mkdir()
        (self.bin / "ssh").write_text(FAKE_SSH)
        (self.bin / "ssh").chmod(0o755)
        (self.bin / "rsync").write_text(FAKE_RSYNC % shutil.which("rsync"))
        (self.bin / "rsync").chmod(0o755)
        self.remote = tmp / "remote"
        self.ssh_log = tmp / "ssh.log"
        self.ssh_log.write_text("")
        # The tree: a commit, a change, a new file, a deletion and the files
        # that must never leave this machine.
        t = self.tree
        (t / ".gitignore").write_text("ignored.txt\n")
        (t / "gone.txt").write_text("to be deleted\n")
        (t / "scripts" / "kit_change.txt").write_text("committed\n")
        _git(t, "add", ".gitignore", "gone.txt", "scripts/kit_change.txt")
        _git(t, "commit", "-q", "-m", "a strand commit")
        (t / "README.md").write_text("changed, not committed\n")
        (t / "notes").mkdir()
        (t / "notes" / "new.txt").write_text("untracked\n")
        (t / "gone.txt").unlink()
        (t / ".env").write_text("NEVER=sent\n")
        (t / ".env.local").write_text("NEVER=sent\n")
        (t / "vault.env").write_text("NEVER=sent\n")
        (t / "notes" / "vault-pass.txt").write_text("never sent\n")
        (t / "ignored.txt").write_text("never sent\n")

    def host_gate(self, plan, *args, skip="lane-test", env=None):
        # A gate on a host binds to a held token or names a skip reason
        # (review M3); the cases that arm lanes pass `skip=None`.
        extra = {"MECLAW_GATE_PLAN": str(self.plan_file(plan)),
                 "PATH": "%s:%s" % (self.bin, os.environ["PATH"]),
                 "MECLAW_LANE_ROOT": str(self.remote),
                 "FAKE_SSH_LOG": str(self.ssh_log)}
        if skip:
            extra["MECLAW_STRAND_TOKEN_SKIP"] = skip
        extra.update(env or {})
        return run_strand(self.repo, "gate", *args, cwd=self.tree, extra_env=extra)

    def receipts(self):
        return self.repo / "plans" / WAVE_DIR / "receipts" / "kit"

    def commit(self, files):
        for rel, text in files.items():
            path = self.tree / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
        _git(self.tree, "add", "-f", *files)
        _git(self.tree, "commit", "-q", "-m", "more")

    def wt(self):
        return self.remote / "wt" / "kit"

    def test_the_gate_runs_on_the_host_and_comes_back_green(self):
        res = self.host_gate(PLAN_HOST, "--host", "north")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        lines = [l for l in res.stdout.splitlines() if l.strip()]
        sha = _git(self.tree, "rev-parse", "--short=8", "HEAD").stdout.strip()
        self.assertRegex(lines[0], r"^strand: lane north \(north\.example\.invalid\) rev %s "
                                   r"overlay [0-9a-f]{16}$" % sha)
        self.assertTrue(SUMMARY_LINE.match(lines[-1]), lines[-1])
        self.assertIn("GREEN", lines[-1])
        # The run happened THERE, at full width.
        self.assertIn("(lane)", (self.wt().parent.parent / "runs" / "kit" / "logs"
                                 / "strand-build.log").read_text())

    def test_the_archive_comes_back_to_the_wave(self):
        self.host_gate(PLAN_HOST, "--host", "north")
        latest = self.repo / "plans" / WAVE_DIR / "receipts" / "kit" / "latest"
        self.assertTrue((latest / "run.log").is_file())
        self.assertTrue((latest / "summary.txt").is_file())
        self.assertTrue((latest / "last-strand.json").is_file())
        self.assertTrue((latest / "logs" / "strand-ok.log").is_file())
        receipt = json.loads((latest / "last-strand.json").read_text())
        base = _git(self.tree, "merge-base", "master", "HEAD").stdout.strip()
        self.assertEqual(base, receipt["base"])

    def test_the_host_tree_is_the_local_tree_without_the_secrets(self):
        self.host_gate(PLAN_HOST, "--host", "north")
        wt = self.wt()
        self.assertEqual("committed\n", (wt / "scripts" / "kit_change.txt").read_text())
        self.assertEqual("changed, not committed\n", (wt / "README.md").read_text())
        self.assertEqual("untracked\n", (wt / "notes" / "new.txt").read_text())
        self.assertFalse((wt / "gone.txt").exists())
        for never in (".env", ".env.local", "vault.env", "notes/vault-pass.txt",
                      "ignored.txt"):
            with self.subTest(path=never):
                self.assertFalse((wt / never).exists())
        self.assertNotIn("NEVER=sent", self.ssh_log.read_text())

    def test_a_second_run_takes_the_new_state(self):
        self.host_gate(PLAN_HOST, "--host", "north")
        (self.tree / "notes" / "new.txt").unlink()
        (self.tree / "README.md").write_text("second state\n")
        res = self.host_gate(PLAN_HOST, "--host", "north")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertFalse((self.wt() / "notes" / "new.txt").exists())
        self.assertEqual("second state\n", (self.wt() / "README.md").read_text())

    def test_the_held_lane_is_the_host_without_host(self):
        self.arm("--lanes", "south,north")
        self.assertEqual("lane south", self.take(None, cwd=self.tree).stdout.strip())
        res = self.host_gate(PLAN_HOST, skip=None)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertTrue(res.stdout.startswith("strand: lane south "), res.stdout)

    def test_a_host_other_than_the_held_lane_is_refused(self):
        self.arm("--lanes", "south,north")
        self.take(None, cwd=self.tree)
        res = self.host_gate(PLAN_HOST, "--host", "north", skip=None)
        self.assertEqual(3, res.returncode, res.stdout + res.stderr)
        self.assertIn("holds lane south, not north", res.stderr)
        self.assertFalse((self.repo / "plans" / WAVE_DIR / "receipts" / "kit").exists())

    def test_an_unreachable_host_is_exit_two_without_latest(self):
        self.arm("--lanes", "ghost")
        self.take(None, cwd=self.tree)
        res = self.host_gate(PLAN_HOST, skip=None)
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        self.assertIn("strand: lane ghost unreachable", res.stderr)
        self.assertFalse((self.repo / "plans" / WAVE_DIR / "receipts" / "kit").exists())
        self.assertEqual(["welle-x/kit"], self.holders())

    def test_an_unknown_lane_is_refused(self):
        res = self.host_gate(PLAN_HOST, "--host", "west")
        self.assertEqual(2, res.returncode)
        self.assertIn("west", res.stderr)

    def test_a_red_station_is_passed_through(self):
        res = self.host_gate(PLAN_HOST_RED, "--host", "north")
        self.assertEqual(1, res.returncode, res.stdout + res.stderr)
        self.assertRegex(res.stdout, r"(?m)^GATE bad \[scope-bad\] \d+s RED")
        self.assertRegex(res.stdout, r"(?m)^GATE-SUMMARY .* RED$")

    def test_env_stations_run_here_with_a_second_summary(self):
        res = self.host_gate(PLAN_HOST_ENV % "true", "--host", "north")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        sums = [l for l in res.stdout.splitlines() if l.startswith("GATE-SUMMARY ")]
        self.assertEqual(2, len(sums), res.stdout)
        self.assertTrue(all(l.endswith(" GREEN") for l in sums), sums)
        latest = self.repo / "plans" / WAVE_DIR / "receipts" / "kit" / "latest"
        self.assertIn("SKIP no-env (lane)", (latest / "run.log").read_text())
        self.assertRegex((latest / "run-local.log").read_text(),
                         r"(?m)^GATE scenarios:memory \[1 case\] \d+s GREEN")

    def test_node_and_ref_stations_run_here_and_the_audit_sees_the_lane(self):
        res = self.host_gate(PLAN_HOST_LOCAL, "--host", "north")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        latest = self.repo / "plans" / WAVE_DIR / "receipts" / "kit" / "latest"
        remote = (latest / "run.log").read_text()
        self.assertRegex(remote, r"(?m)^GATE display-lab \[unittest\] 0s SKIP no-node \(lane\)$")
        self.assertRegex(remote, r"(?m)^GATE export-audit \[R1-R17 dry\] 0s SKIP no-ref \(lane\)$")
        local = (latest / "run-local.log").read_text()
        self.assertRegex(local, r"(?m)^GATE display-lab \[unittest\] \d+s GREEN")
        self.assertRegex(local, r"(?m)^GATE export-audit \[R1-R17 dry\] \d+s GREEN")
        self.assertNotRegex(local, r"(?m)^GATE ok ")

    def test_a_red_env_station_here_makes_the_gate_red(self):
        res = self.host_gate(PLAN_HOST_ENV % "false", "--host", "north")
        self.assertEqual(1, res.returncode, res.stdout + res.stderr)
        self.assertRegex(res.stdout, r"(?m)^GATE scenarios:memory \[1 case\] \d+s RED")
        latest = self.repo / "plans" / WAVE_DIR / "receipts" / "kit" / "latest"
        self.assertIn(" RED", (latest / "summary.txt").read_text())

    def test_without_host_and_lane_the_gate_stays_local(self):
        res = self.host_gate(PLAN_HOST)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertNotIn("strand: lane", res.stdout)
        self.assertEqual("", self.ssh_log.read_text())

    # --- fix round 1 (OR-S3-64): the host gates exactly the local tree, and
    # no secret leaves this machine.

    def test_staged_changes_additions_deletions_and_renames_travel(self):
        # Review I1: `ls-files -m/-d` compare against the index, so a staged
        # change stayed old on the host, a staged new file was missing and a
        # `git rm` stayed.
        self.commit({"src/a.txt": "old a\n", "src/b.txt": "b\n", "src/c.txt": "c\n"})
        (self.tree / "src" / "a.txt").write_text("staged a\n")
        (self.tree / "src" / "n.txt").write_text("staged new\n")
        _git(self.tree, "add", "src/a.txt", "src/n.txt")
        _git(self.tree, "rm", "-q", "src/b.txt")
        _git(self.tree, "mv", "src/c.txt", "src/moved.txt")
        res = self.host_gate(PLAN_HOST, "--host", "north")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        wt = self.wt()
        self.assertEqual("staged a\n", (wt / "src" / "a.txt").read_text())
        self.assertEqual("staged new\n", (wt / "src" / "n.txt").read_text())
        self.assertFalse((wt / "src" / "b.txt").exists())
        self.assertFalse((wt / "src" / "c.txt").exists())
        self.assertEqual("c\n", (wt / "src" / "moved.txt").read_text())

    def test_tracked_files_under_vault_and_env_paths_travel(self):
        # Review I2: the exclusion matched any path COMPONENT `vault*`/`.env*`
        # and kept tracked source at its committed state on the host.
        self.commit({"crates/x/src/vault/mod.rs": "old\n",
                     "templates/vault/a.json": "{}\n",
                     "examples/vault-pilot/b.txt": "old\n",
                     ".env.example": "KEY=\n"})
        (self.tree / "crates/x/src/vault/mod.rs").write_text("new\n")
        (self.tree / "examples/vault-pilot/b.txt").write_text("new\n")
        (self.tree / "examples/vault-pilot/c.txt").write_text("untracked\n")
        (self.tree / ".env.example").write_text("KEY=\nOTHER=\n")
        _git(self.tree, "rm", "-q", "templates/vault/a.json")
        res = self.host_gate(PLAN_HOST, "--host", "north")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        wt = self.wt()
        self.assertEqual("new\n", (wt / "crates/x/src/vault/mod.rs").read_text())
        self.assertEqual("new\n", (wt / "examples/vault-pilot/b.txt").read_text())
        self.assertEqual("untracked\n", (wt / "examples/vault-pilot/c.txt").read_text())
        self.assertEqual("KEY=\nOTHER=\n", (wt / ".env.example").read_text())
        self.assertFalse((wt / "templates/vault/a.json").exists())
        for never in (".env", ".env.local", "vault.env", "notes/vault-pass.txt"):
            with self.subTest(path=never):
                self.assertFalse((wt / never).exists())

    def assert_refused(self, res, path):
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        self.assertIn(path, res.stderr)
        self.assertNotIn("GREEN", res.stdout)
        self.assertFalse(self.receipts().exists())
        self.assertNotIn("receive-pack", self.ssh_log.read_text())
        self.assertNotIn("NEVER=sent", self.ssh_log.read_text())

    def test_a_staged_secret_is_refused_with_its_path(self):
        (self.tree / "config").mkdir()
        (self.tree / "config" / ".env.prod").write_text("NEVER=sent\n")
        _git(self.tree, "add", "-f", "config/.env.prod")
        self.assert_refused(self.host_gate(PLAN_HOST, "--host", "north"), "config/.env.prod")

    def test_a_changed_tracked_secret_is_refused_with_its_path(self):
        self.commit({"deploy/vault-pass.txt": "placeholder\n"})
        (self.tree / "deploy" / "vault-pass.txt").write_text("NEVER=sent\n")
        self.assert_refused(self.host_gate(PLAN_HOST, "--host", "north"), "deploy/vault-pass.txt")

    def test_a_committed_secret_is_refused_before_the_push(self):
        # The commit travels by `git push`: a secret in it would leave by
        # that road, not by the overlay.
        self.commit({".cargo/credentials.toml": "NEVER=sent\n"})
        self.assert_refused(self.host_gate(PLAN_HOST, "--host", "north"), ".cargo/credentials.toml")

    def test_the_overlay_hash_is_the_same_on_both_sides(self):
        res = subprocess.run([str(self.repo / "scripts" / "lane_sync.sh"), "digest"],
                             cwd=str(self.tree), env=kit_env(self.repo),
                             capture_output=True, text=True)
        self.assertEqual(0, res.returncode, res.stderr)
        local = res.stdout.strip()
        self.assertRegex(local, r"^[0-9a-f]{16}$")
        gate = self.host_gate(PLAN_HOST, "--host", "north")
        self.assertEqual(0, gate.returncode, gate.stdout + gate.stderr)
        self.assertTrue(gate.stdout.splitlines()[0].endswith(" overlay " + local), gate.stdout)

    def test_a_host_tree_that_differs_is_refused(self):
        res = self.host_gate(PLAN_HOST, "--host", "north", env={"FAKE_RSYNC_DROP": "1"})
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        self.assertIn("overlay", res.stderr)
        self.assertNotIn("GATE-SUMMARY", res.stdout)
        self.assertFalse((self.receipts() / "latest").exists())

    def test_a_receipt_that_does_not_come_back_is_not_green(self):
        # Review M5: the acceptance wants receipt and logs HERE.
        res = self.host_gate(PLAN_HOST, "--host", "north", env={"FAKE_RSYNC_FAIL": "/runs/kit"})
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        self.assertIn("did not come back", res.stderr)
        self.assertFalse((self.receipts() / "latest" / "summary.txt").exists())

    def test_a_host_gate_without_a_held_token_names_a_reason(self):
        # Review M3: unarmed, a gate on a host bound to nothing.
        res = self.host_gate(PLAN_HOST, "--host", "north", skip=None)
        self.assertEqual(3, res.returncode, res.stdout + res.stderr)
        self.assertIn("MECLAW_STRAND_TOKEN_SKIP", res.stderr)
        self.assertEqual("", self.ssh_log.read_text())

    def test_a_held_anonymous_token_binds_a_host_gate(self):
        self.arm("--max", "1")
        self.take(None, cwd=self.tree)
        res = self.host_gate(PLAN_HOST, "--host", "north", skip=None)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)

    def test_the_exclusion_list_is_pinned(self):
        res = subprocess.run([str(self.repo / "scripts" / "lane_sync.sh"), "files"],
                             cwd=str(self.tree), env=kit_env(self.repo),
                             capture_output=True, text=True)
        self.assertEqual(0, res.returncode, res.stderr)
        sent = sorted(res.stdout.split("\0")[:-1])
        self.assertEqual(["README.md", "notes/new.txt"], sent)


class TestLanesStatus(TokenTestCase):
    def test_one_line_per_lane(self):
        tmp = pathlib.Path(self._tmp.name)
        (self.repo.parent / "lanes").write_text(HOST_LANES)
        b = tmp / "bin"; b.mkdir()
        (b / "ssh").write_text(FAKE_SSH); (b / "ssh").chmod(0o755)
        (tmp / "remote").mkdir()
        res = run_strand(self.repo, "lanes", "status", extra_env={
            "PATH": "%s:%s" % (b, os.environ["PATH"]),
            "MECLAW_LANE_ROOT": str(tmp / "remote"),
            "FAKE_SSH_LOG": str(tmp / "ssh.log")})
        self.assertEqual(0, res.returncode, res.stderr)
        lines = res.stdout.splitlines()
        self.assertEqual(3, len(lines), res.stdout)
        self.assertRegex(lines[0], r"^north\s+reachable\s+free \d+G\s+bare none$")
        self.assertRegex(lines[2], r"^ghost\s+unreachable$")


class TestTokenGuards(TokenTestCase):
    """Where the token is checked: `strand.sh gate` and `test-tier.sh`."""

    def test_gate_refuses_without_a_token_and_writes_no_archive(self):
        self.arm()
        res = self.gate()
        self.assertEqual(3, res.returncode, res.stdout + res.stderr)
        self.assertIn(REFUSED, res.stderr)
        self.assertIn("scripts/strand.sh token take", res.stderr)
        self.assertEqual("", res.stdout)
        self.assertFalse((self.repo / "plans" / WAVE_DIR / "receipts" / "kit").exists())

    def test_gate_runs_with_a_token_and_prints_one_summary_line(self):
        self.arm()
        self.assertEqual(0, self.take(None, cwd=self.tree).returncode)
        res = self.gate()
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        lines = [l for l in res.stdout.splitlines() if l.strip()]
        self.assertEqual(1, len(lines), res.stdout)
        self.assertTrue(SUMMARY_LINE.match(lines[0]), lines[0])
        # The heartbeat after the run: seen is fresh, and the pid of the
        # finished gate is gone from the entry.
        self.assertEqual([], self.state()["holders"][0]["pids"])

    def test_tier_refuses_without_a_token(self):
        self.arm()
        res = run_tier(self.repo, self.tree)
        self.assertEqual(3, res.returncode, res.stdout + res.stderr)
        self.assertIn(REFUSED, res.stderr)
        self.assertNotIn("tier-dry:", res.stdout)

    def test_tier_inside_the_gate_needs_no_token(self):
        self.arm()
        for var in ("MECLAW_CARGO_LOCK_HELD", "CI"):
            with self.subTest(var=var):
                res = run_tier(self.repo, self.tree, {var: "1"})
                self.assertEqual(0, res.returncode, res.stdout + res.stderr)
                self.assertIn("tier-dry:", res.stdout)
                self.assertNotIn(REFUSED, res.stderr)

    def test_a_broken_token_file_is_exit_2_at_the_gate_and_the_tier(self):
        """A broken file is a call to look at it, not a missing token: the
        gate passes the exit of `token check` on like the tier (review M4)."""
        self.arm()
        self.assertEqual(0, self.take(None, cwd=self.tree).returncode)
        (self.repo.parent / "tokens").write_text("{not json")
        res = self.gate()
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        self.assertIn("is broken", res.stderr)
        self.assertFalse((self.repo / "plans" / WAVE_DIR / "receipts" / "kit").exists())
        res = run_tier(self.repo, self.tree)
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)

    def test_the_main_tree_needs_no_token(self):
        self.arm()
        res = self.gate(self.repo, "--strand", "kit")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        res = run_tier(self.repo, self.repo)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("tier-dry:", res.stdout)

    def test_report_releases_the_token(self):
        self.arm()
        self.assertEqual(0, self.take(None, cwd=self.tree).returncode)
        target = self.tree / "scripts" / "kit_change.txt"
        target.write_text("a change\n")
        _git(self.tree, "add", "scripts/kit_change.txt")
        _git(self.tree, "commit", "-q", "-m", "welle-x kit (#861): eine Änderung")
        self.assertEqual(0, self.gate().returncode)
        res = run_strand(self.repo, "report", cwd=self.tree)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("strand: cargo token released by %s/kit" % WAVE, res.stderr)
        self.assertEqual([], self.holders())


class TestPreamble(unittest.TestCase):
    """`plans/PREAMBLE.md` is now the ONLY place the machine rules stand.

    The dispatch prompt points at it and repeats nothing, so a rule that is
    not in this file is gone from the next wave. These are the three that were
    only in the wave preambles they replaced -- pinned here so a rewrite of
    the file cannot drop them in silence. `plans/` never travels with the
    export, so in a public clone the case skips.
    """

    PREAMBLE = REPO / "plans" / "PREAMBLE.md"

    # Rule -> a phrase that has to survive any rewrite of the file.
    RULES = {
        "per Manifest bauen": ("Manifest", "Hand-Edit"),
        "die Welle raeumt ihre Issues": ("eigenen Issues",),
        "Runner-Hygiene last_run.json": ("last_run.json",),
        "Unter-Agenten im Vordergrund": ("run_in_background: false",),
        "Token per Kit": ("strand.sh token",),
    }

    def setUp(self):
        if not self.PREAMBLE.is_file():
            self.skipTest("plans/ is not part of the public tree")
        self.text = self.PREAMBLE.read_text()

    def test_every_machine_rule_stands_in_the_canonical_file(self):
        for rule, phrases in self.RULES.items():
            for phrase in phrases:
                self.assertTrue(phrase in self.text,
                                "%s: the rule %r is gone -- no line says %r"
                                % (self.PREAMBLE.name, rule, phrase))


class TestPublicHygiene(unittest.TestCase):
    """The kit travels in the public export (`ROOT_FILES`), tests included.

    Two rules of `plans/PREAMBLE.md` section 1 apply to every line of it, and
    neither is covered by an export audit: R5 knows name and domain patterns
    but no address pattern, and staging a whole tree at once is forbidden by
    `docs/development-rules.md` section 6, which nothing checks yet.
    """

    FILES = (STRAND_SH, LANE_SYNC,
             REPO / "scripts" / "wave_receipt.py",
             pathlib.Path(__file__).resolve(),
             REPO / "scripts" / "tests" / "test_wave_receipt.py")

    # `127.0.0.1` and `0.0.0.0` are the two addresses a public file may name;
    # everything else is somebody's machine.
    ADDRESS = re.compile(r"\b(?:\d{1,3}\.){3}\d{1,3}\b")
    ALLOWED = {"127.0.0.1", "0.0.0.0"}
    ADD_ALL = re.compile(r"add[\"',\s]+-A")

    def test_no_private_address_travels_with_the_kit(self):
        for path in self.FILES:
            found = {a for a in self.ADDRESS.findall(path.read_text())
                     if a not in self.ALLOWED}
            self.assertEqual(set(), found, "%s names %s" % (path.name, found))

    def test_the_retro_writes_english_decimal_points(self):
        """`scripts/` is English, numbers included: `2.2 %`, not `2,2 %`
        (review M5)."""
        path = REPO / "scripts" / "retro" / "cache.py"
        found = re.findall(r"\b\d+,\d+ ?%", path.read_text())
        self.assertEqual([], found, "%s writes %s" % (path.name, found))

    def test_the_kit_never_stages_the_whole_tree(self):
        for path in self.FILES:
            self.assertIsNone(self.ADD_ALL.search(path.read_text()),
                              "%s stages the whole tree" % path.name)


if __name__ == "__main__":
    unittest.main()
