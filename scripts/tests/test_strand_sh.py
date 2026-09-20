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

import os
import pathlib
import re
import shutil
import subprocess
import tempfile
import unittest

REPO = pathlib.Path(__file__).resolve().parents[2]
STRAND_SH = REPO / "scripts" / "strand.sh"
GATE_SH = REPO / "scripts" / "gate.sh"
GATE_PLAN = REPO / "scripts" / "gate_plan.py"

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
    for src in (STRAND_SH, GATE_SH):
        shutil.copy(src, repo / "scripts" / src.name)
        (repo / "scripts" / src.name).chmod(0o755)
    shutil.copy(GATE_PLAN, repo / "scripts" / "gate_plan.py")
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


def run_strand(repo, *args, cwd=None, extra_env=None, timeout_s=120):
    env = dict(os.environ)
    env.update(GIT_ENV)
    env.pop("CARGO_TARGET_DIR", None)
    env.setdefault("MECLAW_GATE_LOCK", str(repo.parent / "cargo.lock.test"))
    env.setdefault("MECLAW_GATE_MIN_FREE_G", "0")
    if extra_env:
        env.update(extra_env)
    return subprocess.run(
        [str(repo / "scripts" / "strand.sh")] + list(args),
        cwd=str(cwd or repo), env=env, capture_output=True, text=True,
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
             / "summary.txt").is_file(),
            "the gate archive did not land in the branch's wave")
        self.assertFalse(
            (self.repo / "plans" / OTHER_WAVE_DIR / "receipts").exists(),
            "the gate archive landed in the top row's wave")

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
    r"(?P<secs>\d+)s (?P<verdict>GREEN|RED)$")


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

    def archive(self, name="kit"):
        return self.repo / "plans" / WAVE_DIR / "receipts" / name

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

    def test_report_refuses_a_red_gate(self):
        self.commit()
        self.assertEqual(1, self.gate(PLAN_RED).returncode)
        res = run_strand(self.repo, "report", cwd=self.tree)
        self.assertEqual(1, res.returncode, res.stdout)
        self.assertIn("RED", res.stderr)

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

    FILES = (STRAND_SH,
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

    def test_the_kit_never_stages_the_whole_tree(self):
        for path in self.FILES:
            self.assertIsNone(self.ADD_ALL.search(path.read_text()),
                              "%s stages the whole tree" % path.name)


if __name__ == "__main__":
    unittest.main()
