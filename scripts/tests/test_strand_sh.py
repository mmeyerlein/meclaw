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

import fcntl
import hashlib
import json
import os
import pathlib
import re
import shlex
import shutil
import signal
import sqlite3
import subprocess
import socket
import time
import sys
import tempfile
import time
import unittest
import uuid
from scripts.pipeline import lane
from scripts.tests import fake_ssh, kit_time, kit_tree, store_isolation

REPO = pathlib.Path(__file__).resolve().parents[2]
STRAND_SH = REPO / "scripts" / "strand.sh"
GATE_SH = REPO / "scripts" / "gate.sh"
TIER_SH = REPO / "scripts" / "test-tier.sh"
GATE_PLAN = REPO / "scripts" / "gate_plan.py"
LANE_SYNC = REPO / "scripts" / "lane_sync.sh"
LANE_FP = REPO / "scripts" / "pipeline" / "lane_fingerprint.sh"
# Since W3 (GH #1117): `gate.sh` is the shell in front of these two, and
# `strand.sh gate|test|token|lanes` delegate to the pipeline core; since
# GH #1156 there is no switch back, only `gate ci` keeps the old runner.
GATE_LEGACY = REPO / "scripts" / "gate_legacy.sh"
PIPELINE_PY = REPO / "scripts" / "pipeline.py"
PIPELINE_PKG = REPO / "scripts" / "pipeline"

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


PRE_STAMP_STUB = """

def stamp_status(repo, now=None):  # test_strand_sh fixture: GREEN, no line
    return True, ""


_pre_main = main


def main(argv=None):  # test_strand_sh fixture: the stamp check passes silently
    args = sys.argv[1:] if argv is None else argv
    return 0 if "--stamp-check" in args else _pre_main(argv)
"""


def make_repo(root):
    """A one-commit repo with the kit, the gate runner and a plans tree."""
    repo = pathlib.Path(root) / "repo"
    (repo / "scripts" / "tests").mkdir(parents=True, exist_ok=True)
    for src in (STRAND_SH, GATE_SH, GATE_LEGACY, TIER_SH, LANE_SYNC):
        shutil.copy(src, repo / "scripts" / src.name)
        (repo / "scripts" / src.name).chmod(0o755)
    shutil.copy(PIPELINE_PY, repo / "scripts" / "pipeline.py")
    shutil.copytree(PIPELINE_PKG, repo / "scripts" / "pipeline",
                    ignore=shutil.ignore_patterns("__pycache__"))
    shutil.copy(GATE_PLAN, repo / "scripts" / "gate_plan.py")
    # `pipeline.py` loads every command module; W4's `pre` imports the precheck.
    shutil.copy(REPO / "scripts" / "precheck.py", repo / "scripts" / "precheck.py")
    # W4's pre stamp guards a strand gate from a strand worktree. The trees
    # here are never pre-checked -- the stamp has its own suites
    # (test_pipeline_pre, test_pipeline_gate_cli); this copy reports it GREEN.
    pre_py = repo / "scripts" / "pipeline" / "pre.py"
    main = '\nif __name__ == "__main__":'
    pre_py.write_text(pre_py.read_text().replace(main, PRE_STAMP_STUB + main, 1))
    # `scripts/gate.sh` SOURCES this one (GH #802) -- without it the runner in
    # the throw-away repo does not start at all.
    kit_tree.library(repo / "scripts", "cargo-target.sh")
    # The lane fingerprint (W1) -- `lane_sync.sh` sends it to the lane.
    (repo / "scripts" / "pipeline").mkdir(exist_ok=True)
    shutil.copy(LANE_FP, repo / "scripts" / "pipeline" / "lane_fingerprint.sh")
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


def install_pipeline(repo):
    """The pipeline core beside the kit of `repo`, UNTRACKED (W9).

    `close`, `merge` and `revert` hand over to `scripts/pipeline.py`, which
    loads the whole package -- `pre.py` with `precheck.py` among it. Committed,
    `pre.py` would reach every strand worktree, and its presence arms the pre
    stamp of `gate`; untracked in the main tree, the kit finds it (the kit is
    called from there) and no strand gate sees it.
    """
    shutil.copy(REPO / "scripts" / "pipeline.py", repo / "scripts" / "pipeline.py")
    shutil.copy(REPO / "scripts" / "precheck.py", repo / "scripts" / "precheck.py")
    for src in (REPO / "scripts" / "pipeline").iterdir():
        if src.suffix in (".py", ".json"):
            shutil.copy(src, repo / "scripts" / "pipeline" / src.name)


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
                "MECLAW_STRAND_NOW", "MECLAW_TIER_DRY", "MECLAW_GATE_LANE_RECEIPT",
                "MECLAW_LANES_CALLER"):
        env.pop(var, None)
    env["MECLAW_STRAND_TOKENS"] = str(repo.parent / "tokens")
    # The store of the pipeline core: SET like the token file, never the
    # host's. No way back to the old path (GH #1156): a switch inherited from
    # a developer shell goes.
    env["MECLAW_PIPELINE_DB"] = str(repo.parent / "pipeline.db")
    env.pop("MECLAW_GATE_LEGACY", None)
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
    # ... and the floor of lane_sync.sh (60 G): on a build host the temp
    # directory of the fake lane has less free (W1 gate on a build host).
    env.setdefault("MECLAW_LANE_FLOOR_G", "0")
    # The idle-off timer of a lane (dev-tempo W2) is a real `systemd-run`
    # timer on this machine: no case arms one unless it says so.
    env.setdefault("MECLAW_IDLE_OFF_H", "0")
    for var in ("MECLAW_HYGIENE", "MECLAW_LANE_EXPECTED_G"):
        env.pop(var, None)
    if extra_env:
        env.update(extra_env)
    # `None` removes a variable.
    return {k: v for k, v in env.items() if v is not None}


def run_strand(repo, *args, cwd=None, extra_env=None, timeout_s=4 * kit_time.HANG):
    return subprocess.run(
        [str(repo / "scripts" / "strand.sh")] + list(args),
        cwd=str(cwd or repo), env=kit_env(repo, extra_env),
        capture_output=True, text=True, timeout=timeout_s)


def run_tier(repo, tree, extra_env=None, timeout_s=4 * kit_time.HANG):
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
        """The report `new` writes: in the strand's worktree (#1148)."""
        return self.worktree(name) / "plans" / WAVE_DIR / "berichte" / ("%s.md" % name)

    def main_report(self, name):
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
        # #1148: report and strand file in the worktree, the main tree untouched.
        self.assertFalse(self.main_report("kit").exists())
        self.assertFalse((self.repo / "plans" / WAVE_DIR / "straenge" / "kit.json").exists())
        self.assertTrue((self.worktree("kit") / "plans" / WAVE_DIR / "straenge"
                         / "kit.json").is_file())
        # Z-I 2c (2): the scaffold carries the section the merge asks for.
        self.assertRegex(self.report("kit").read_text(), r"(?m)^## Review$")
        # GH #1196: the field merge asks for, as a placeholder that does not pass.
        text = self.report("kit").read_text()
        self.assertRegex(text, r"(?m)^Outsourced findings: <")
        from scripts.pipeline import merge
        self.assertFalse(merge.outsourced_findings_ok(text))
        self.assertTrue(merge.outsourced_findings_ok(
            text.replace(re.search(r"(?m)^Outsourced findings: .*$", text).group(0),
                         "Outsourced findings: none")))

    def test_new_prompt_points_at_the_rules_and_repeats_none_of_them(self):
        res = run_strand(self.repo, "new", "kit", "--issue", "756")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("plans/PREAMBLE.md", res.stdout)
        self.assertIn("plans/%s/BUILD-PREAMBLE.md" % WAVE_DIR, res.stdout)
        for word in RULE_WORDS:
            self.assertNotIn(word, res.stdout,
                             "the prompt repeats the rule %r:\n%s"
                             % (word, res.stdout))

    def test_new_prompt_carries_the_reading_budget_line_without_graphify(self):
        """BUILD-PREAMBLE point 12 of the dev-tempo wave: the orientation line
        is printed by the kit; the graphify sentence is not (W6 adds it)."""
        res = run_strand(self.repo, "new", "kit", "--issue", "756")
        self.assertEqual(0, res.returncode, res.stderr)
        lines = [ln for ln in res.stdout.splitlines() if ln.startswith("Orientierung:")]
        self.assertEqual(1, len(lines), res.stdout)
        self.assertTrue(lines[0].startswith("Orientierung: dein Plan-Teil"), lines[0])
        self.assertNotIn("V0-vertraege", res.stdout)
        self.assertIn("nie `grep -r` ueber `crates/` ohne Pfadfilter.", res.stdout)
        self.assertNotIn("graphify", res.stdout)
        # C-M4: the contracts file is named only in a wave that has one
        parts = self.repo / "plans" / WAVE_DIR / "plan-parts"
        parts.mkdir(parents=True, exist_ok=True)
        (parts / "V0-vertraege.md").write_text("# V0\n")
        res = run_strand(self.repo, "new", "kit2", "--issue", "757")
        self.assertEqual(0, res.returncode, res.stderr)
        lines = [ln for ln in res.stdout.splitlines() if ln.startswith("Orientierung:")]
        self.assertIn("`plan-parts/V0-vertraege.md`, dein Plan-Teil", lines[0])

    def test_new_graphify_adds_the_one_question_line(self):
        res = run_strand(self.repo, "new", "kit", "--issue", "756", "--graphify")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("query|path|explain '<question>'", res.stdout)
        plain = run_strand(self.repo, "new", "kit2", "--issue", "756")
        self.assertNotIn("query|path|explain", plain.stdout)

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

    def test_new_writes_the_strand_file_and_keeps_an_existing_one(self):
        """W9: the strand declares the KIND of its change; the merge point
        assigns the numbers (dev rules section 4)."""
        res = run_strand(self.repo, "new", "kit", "--issue", "756",
                         "--issue", "#755")
        self.assertEqual(0, res.returncode, res.stderr)
        sfile = self.worktree("kit") / "plans" / WAVE_DIR / "straenge" / "kit.json"
        self.assertEqual({"strand": "kit", "issue": [756, 755], "title": "",
                          "kind": {}}, json.loads(sfile.read_text()))
        self.assertIn("straenge/kit.json", res.stdout)
        self.assertIn("## Versionsart", self.report("kit").read_text())
        one = self.repo / "plans" / WAVE_DIR / "straenge" / "solo.json"
        one.parent.mkdir(parents=True, exist_ok=True)
        one.write_text('{"strand": "solo", "issue": 1, "title": "kept", '
                       '"kind": {"t": "feature"}}\n')
        res = run_strand(self.repo, "new", "solo", "--issue", "757")
        self.assertEqual(0, res.returncode, res.stderr)
        # The orchestrator's file in the main tree travels into the worktree.
        mine = self.worktree("solo") / "plans" / WAVE_DIR / "straenge" / "solo.json"
        self.assertEqual("kept", json.loads(mine.read_text())["title"])
        self.assertEqual("kept", json.loads(one.read_text())["title"])
        res = run_strand(self.repo, "new", "duo", "--issue", "758")
        self.assertEqual(0, res.returncode, res.stderr)
        duo = self.worktree("duo") / "plans" / WAVE_DIR / "straenge" / "duo.json"
        self.assertEqual(758, json.loads(duo.read_text())["issue"])

    def test_new_refuses_an_existing_strand(self):
        self.assertEqual(0, run_strand(self.repo, "new", "kit", "--issue",
                                       "756").returncode)
        res = run_strand(self.repo, "new", "kit", "--issue", "756")
        self.assertNotEqual(0, res.returncode)
        self.assertIn("kit", res.stderr)

    def _side_commit(self, author):
        _git(self.repo, "checkout", "-q", "-b", "side")
        (self.repo / "side.txt").write_text("side\n")
        _git(self.repo, "add", "side.txt")
        env = dict(kit_env(self.repo), GIT_AUTHOR_NAME=author[0], GIT_AUTHOR_EMAIL=author[1])
        subprocess.run(["git", "-C", str(self.repo), "commit", "-q", "-m", "side"],
                       env=env, check=True, capture_output=True)
        sha = _git(self.repo, "rev-parse", "HEAD").stdout.strip()
        _git(self.repo, "checkout", "-q", "master")
        return sha

    def test_new_refuses_a_base_with_foreign_commits(self):
        # GH #1167 (g): a base other than the master tip carries no commit
        # that is not the owner's -- it would land under the owner's name.
        owner = {"MECLAW_OWNER": "strand test <strand@example.invalid>"}
        sha = self._side_commit(("someone", "x@example.invalid"))
        res = run_strand(self.repo, "new", "kit", "--issue", "756", "--base", "side",
                         extra_env=owner)
        self.assertNotEqual(0, res.returncode, res.stdout)
        self.assertIn(sha[:9], res.stderr)
        self.assertFalse(self.worktree("kit").exists())

    def test_new_takes_an_own_base(self):
        owner = {"MECLAW_OWNER": "strand test <strand@example.invalid>"}
        self._side_commit(("strand test", "strand@example.invalid"))
        res = run_strand(self.repo, "new", "kit", "--issue", "756", "--base", "side",
                         extra_env=owner)
        self.assertEqual(0, res.returncode, res.stderr)


class TestMergeAndRevert(StrandShTestCase):
    """`merge` and `revert` are the pipeline core's (W9): the kit hands the
    arguments through and passes the exit on unchanged."""

    def setUp(self):
        super().setUp()
        install_pipeline(self.repo)

    def test_merge_and_revert_hand_over_to_the_pipeline(self):
        for sub in ("merge", "revert"):
            res = run_strand(self.repo, sub, "--help")
            self.assertEqual(0, res.returncode, res.stderr)
            self.assertIn("usage: pipeline %s" % sub, res.stdout)

    def test_merge_passes_the_usage_exit_through(self):
        res = run_strand(self.repo, "merge")
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        self.assertIn("--strand", res.stderr)

    def test_the_kit_names_merge_and_revert(self):
        res = run_strand(self.repo, "nosuch")
        self.assertEqual(2, res.returncode)
        self.assertIn("merge|revert", res.stderr)
        self.assertIn("strand.sh merge", run_strand(self.repo, "--help").stdout)

    def test_merge_in_a_strand_tree_takes_the_strand_from_the_branch(self):
        """GH #1199 (a): `gate` and `report` read the strand from the branch
        `<wave>/<S>`, `merge` ended with `merge: name the strand` in the very
        same tree (wave validation 2, strand v2-p1078). `--dequeue` is the
        merge's step that touches nothing but the store."""
        self.assertEqual(0, run_strand(self.repo, "new", "kit", "--issue", "756").returncode)
        res = run_strand(self.repo, "merge", "--dequeue", cwd=self.worktree("kit"))
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("DEQUEUED kit ", res.stdout)

    def test_merge_in_the_main_tree_still_asks_for_the_strand(self):
        # The main tree is no strand tree, even on a branch with a slash.
        _git(self.repo, "checkout", "-q", "-b", "welle-x/kit")
        res = run_strand(self.repo, "merge", "--dequeue")
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        self.assertIn("merge: name the strand", res.stderr)


class TestChdirOption(StrandShTestCase):
    """GH #1199 (b), class V2-A4: `-C <dir>` in front of the subcommand acts as
    a `cd <dir>` first, as `git -C` does. An agent shell that resets its cwd
    on every call could reach a worktree only through a wrapper script
    (wave validation 2, strand v2-p1078: one line per use in its log of
    manual steps)."""

    def setUp(self):
        super().setUp()
        install_pipeline(self.repo)
        self.assertEqual(0, run_strand(self.repo, "new", "kit", "--issue", "756").returncode)
        self.tree = self.worktree("kit")
        self.outside = pathlib.Path(self._tmp.name) / "elsewhere"
        self.outside.mkdir()

    def test_the_subcommand_runs_as_if_started_in_the_tree(self):
        res = run_strand(self.repo, "-C", str(self.tree), "merge", "--dequeue", cwd=self.outside)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("DEQUEUED kit ", res.stdout)
        # Without it the same call stands outside any repository.
        res = run_strand(self.repo, "merge", "--dequeue", cwd=self.outside)
        self.assertNotEqual(0, res.returncode, res.stdout)

    def test_a_relative_path_is_taken_from_the_callers_directory(self):
        res = run_strand(self.repo, "-C", self.tree.name, "merge", "--dequeue",
                         cwd=self.tree.parent)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("DEQUEUED kit ", res.stdout)

    def test_help_after_the_option_is_the_help(self):
        res = run_strand(self.repo, "-C", str(self.tree), "--help", cwd=self.outside)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("strand.sh -C <worktree>", res.stdout)

    def test_a_missing_directory_is_a_clear_exit_2(self):
        gone = self.outside / "gone"
        res = run_strand(self.repo, "-C", str(gone), "merge", "--dequeue", cwd=self.outside)
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        self.assertIn("strand: -C: no such directory: %s" % gone, res.stderr)
        self.assertEqual("", res.stdout)
        res = run_strand(self.repo, "-C", cwd=self.outside)
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        self.assertIn("strand: -C needs a value", res.stderr)


class TestPipelineChdirOption(StrandShTestCase):
    """GH #1199 (b): `scripts/pipeline.py -C <dir> <command>` -- the same
    option in front of the pipeline core, which reads its tree from the cwd
    (`pre`, `gate`, `merge`, `revert`, `lanes`)."""

    def pipeline(self, *args, cwd):
        return subprocess.run(["python3", "-I", str(self.repo / "scripts" / "pipeline.py")]
                              + list(args), cwd=str(cwd), env=kit_env(self.repo),
                              capture_output=True, text=True, timeout=4 * kit_time.HANG)

    def setUp(self):
        super().setUp()
        install_pipeline(self.repo)
        self.other = pathlib.Path(self._tmp.name) / "other"
        self.other.mkdir()
        (self.other / "only-here.txt").write_text("a tree of its own\n")
        _git(self.other, "init", "-q")
        _git(self.other, "add", "only-here.txt")
        _git(self.other, "commit", "-q", "-m", "other")

    def test_the_command_runs_as_if_started_in_the_directory(self):
        there = self.pipeline("pre", "--tree-hash", cwd=self.other)
        self.assertEqual(0, there.returncode, there.stderr)
        res = self.pipeline("-C", str(self.other), "pre", "--tree-hash", cwd=self.repo)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual(there.stdout, res.stdout)
        here = self.pipeline("pre", "--tree-hash", cwd=self.repo)
        self.assertNotEqual(here.stdout, res.stdout)

    def test_a_missing_directory_is_a_clear_exit_2(self):
        gone = pathlib.Path(self._tmp.name) / "gone"
        res = self.pipeline("-C", str(gone), "pre", "--tree-hash", cwd=self.repo)
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        self.assertIn("pipeline: -C: no such directory: %s" % gone, res.stderr)
        self.assertEqual("", res.stdout)
        res = self.pipeline("-C", cwd=self.repo)
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        self.assertIn("pipeline: -C needs a directory", res.stderr)


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
        res = run_strand(self.repo, "gate", "ci", cwd=self.worktree("kit"),
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
        return run_strand(self.repo, "gate", "ci", cwd=self.worktree("kit"),
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

    # --- welle-validierung-2 3b (GH #1199): section Live knows the running wave ---

    def _live(self, *rows):
        for d in (OTHER_WAVE_DIR, SAME_PREFIX_WAVE_DIR):
            (self.repo / "plans" / d / "berichte").mkdir(parents=True, exist_ok=True)
        (self.repo / "plans" / "README.md").write_text(
            "# Plans\n\n## Live\n\n| Path | What it is |\n|---|---|\n"
            + "".join("| `%s/` | %s |\n" % r for r in rows) + "\n## Archive\n")

    def test_the_running_row_names_the_wave_below_a_built_top_row(self):
        self._live((OTHER_WAVE_DIR, "**Built 2026-10-12:** over."),
                   (WAVE_DIR, "**Running since 2026-10-14:** the wave under test."))
        res = run_strand(self.repo, "new", "kit", "--issue", "756")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("strand: wave %s (Running row" % WAVE_DIR, res.stderr)
        self.assertTrue(self.report("kit").exists(), res.stderr)

    def test_two_running_rows_stop_and_name_both(self):
        self._live((OTHER_WAVE_DIR, "**Running since 2026-10-14:** one."),
                   (WAVE_DIR, "**Running since 2026-10-13:** the other."))
        res = run_strand(self.repo, "new", "kit", "--issue", "756")
        self.assertNotEqual(0, res.returncode, res.stdout)
        for d in (OTHER_WAVE_DIR, WAVE_DIR, "--wave <directory>"):   # c23 M-4: English
            self.assertIn(d, res.stderr)
        self.assertFalse(self.report("kit").exists(), "it guessed one of two running waves")

    def test_a_row_the_core_does_not_read_as_running_is_none_for_the_kit(self):
        """Re-check v2-kit c23 M-5: one reader of the Running row. A date the
        core's `wave.RUNNING` refuses (it would never turn the row Built) is no
        Running row for `wave_dir` either -- the kit asks `pipeline wave running`."""
        self._live((OTHER_WAVE_DIR, "**Running since 14.10.2026:** a hand-written row."),
                   (WAVE_DIR, "**Running since 2026-10-14:** the wave under test."))
        res = run_strand(self.repo, "new", "kit", "--issue", "756")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("strand: wave %s (Running row" % WAVE_DIR, res.stderr)
        self.assertTrue(self.report("kit").exists(), res.stderr)

    def test_wave_open_commits_the_running_row_the_kit_then_finds(self):
        self._live((OTHER_WAVE_DIR, "**Built 2026-10-12:** over."))
        _git(self.repo, "add", "plans")
        _git(self.repo, "commit", "-q", "-m", "live")
        before = run_strand(self.repo, "new", "kit", "--issue", "756")
        self.assertNotEqual(0, before.returncode, before.stdout)     # the stop of today
        res = run_strand(self.repo, "wave", "open", WAVE_DIR, "--title", "The wave under test.")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertEqual("", _git(self.repo, "status", "--porcelain", "--", "plans/README.md").stdout)
        top = [ln for ln in (self.repo / "plans" / "README.md").read_text().splitlines()
               if ln.startswith("| `")][0]
        self.assertRegex(top, r"^\| `%s/` \| \*\*Running since \d{4}-\d{2}-\d{2}:\*\* "
                              r"The wave under test\. \|$" % re.escape(WAVE_DIR))
        res = run_strand(self.repo, "new", "kit", "--issue", "756")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("strand: wave %s (Running row" % WAVE_DIR, res.stderr)


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
        return run_strand(self.repo, "gate", "ci", *args, cwd=self.tree,
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
        self.assertTrue((arc / "last-ci.json").is_file(),
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
        res = run_strand(self.repo, "gate", "ci", cwd=self.tree)
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
        self.assertIn("gate options pass through to scripts/pipeline.py gate", res.stdout)
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

    def test_gate_prints_the_question_block_of_an_ask_run(self):
        """The kit's half of the contract, without the runner's arm: the ASK
        station line, the summary and the question block from the run log."""
        ask_stub(self.tree)
        res = run_strand(self.repo, "gate", "ci", cwd=self.tree)
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
        install_pipeline(self.repo)
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
        """A gate run whose archive `report` and `close` read as a STRAND run.

        The strand gate is the pipeline's (its archive layout is pinned in
        test_pipeline_gate_cli); since GH #1156 the kit reaches the old runner
        with `ci` only, which writes the same layout with mode `ci`. The run is
        relabelled `strand` -- receipt and summary line -- so these cases keep
        pinning what report and close do with an archive."""
        res = run_strand(self.repo, "gate", "ci", cwd=self.tree,
                         extra_env={"MECLAW_GATE_PLAN": str(self.plan_file(plan_text))})
        root = self.repo / "plans" / WAVE_DIR / "receipts" / "kit"
        run = (root / "latest").resolve() if (root / "latest").exists() else None
        if run is not None and run.is_dir():
            for path in run.glob("*.json"):
                try:
                    doc = json.loads(path.read_text())
                except ValueError:
                    continue
                if isinstance(doc, dict) and doc.get("mode") == "ci":
                    doc["mode"] = "strand"
                    (run / "last-strand.json").write_text(json.dumps(doc))
            summary = run / "summary.txt"
            if summary.exists():
                summary.write_text(summary.read_text().replace("GATE-SUMMARY ci ",
                                                               "GATE-SUMMARY strand "))
        return res

    def test_report_fills_gate_and_commits_from_the_archive(self):
        sha = self.commit()
        self.assertEqual(0, self.gate(PLAN_GREEN).returncode)
        res = run_strand(self.repo, "report", cwd=self.tree)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        head = header_block(self.report("kit"))
        self.assertIn("GATE-SUMMARY", head["gate"])
        self.assertIn("GREEN", head["gate"])
        self.assertEqual("[%s]" % sha, head["commits"])

    def test_report_from_a_strand_tree_fills_the_report_in_that_tree(self):
        # Z-A (E-DT-W0.a): the report lives in the strand's worktree; `report`
        # read the main tree's copy, and strands copied and deleted files in
        # the main tree to feed it (stall 05:06-07:53 on a permission dialog).
        sha = self.commit()
        self.assertEqual(0, self.gate(PLAN_GREEN).returncode)
        # Since #1148 `new` writes it there; the main tree never has one.
        mine = self.tree / "plans" / WAVE_DIR / "berichte" / "kit.md"
        self.assertFalse(self.main_report("kit").exists())
        res = run_strand(self.repo, "report", cwd=self.tree)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        head = header_block(mine)
        self.assertIn("GATE-SUMMARY", head["gate"])
        self.assertEqual("[%s]" % sha, head["commits"])
        self.assertFalse(self.main_report("kit").exists())
        # Review Z-A R3 M2: close reads the same copy (it handed the main
        # tree's path to `merge.py close`, which refused `no_report`).
        res = run_strand(self.repo, "close", cwd=self.tree)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("gh issue close", res.stdout)

    def test_report_from_the_main_tree_fills_the_worktree_copy(self):
        # GH #1167 (e): a stale copy in the main tree never wins over the
        # strand's worktree -- `report` never writes the main tree while the
        # strand has a worktree (its untracked copy stops the fast-forward).
        sha = self.commit()
        self.assertEqual(0, self.gate(PLAN_GREEN).returncode)
        stale = self.main_report("kit")
        stale.parent.mkdir(parents=True, exist_ok=True)
        stale.write_text(self.report("kit").read_text())
        before = stale.read_text()
        res = run_strand(self.repo, "report", "--strand", "kit")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertEqual("[%s]" % sha, header_block(self.report("kit"))["commits"])
        self.assertEqual(before, stale.read_text())

    def test_report_from_another_strands_tree_fills_the_named_strands_copy(self):
        # Re-check H2 m8: a copy of kit's report in ANOTHER strand's tree
        # (a rebase brought it along) is not kit's report.
        sha = self.commit()
        self.assertEqual(0, self.gate(PLAN_GREEN).returncode)
        res = run_strand(self.repo, "new", "other", "--issue", "757")
        self.assertEqual(0, res.returncode, res.stderr)
        foreign = self.worktree("other") / "plans" / WAVE_DIR / "berichte" / "kit.md"
        foreign.write_text(self.report("kit").read_text())
        before = foreign.read_text()
        res = run_strand(self.repo, "report", "--strand", "kit", cwd=self.worktree("other"))
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertEqual("[%s]" % sha, header_block(self.report("kit"))["commits"])
        self.assertEqual(before, foreign.read_text())

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

    def test_report_refuses_without_a_fresh_pre_green_stamp_on_the_head(self):
        # Convergence A (owner, 13.10.): the done-definition's (a) -- pre
        # GREEN on the own head -- is checked by `report`, not trusted. The
        # strand tree here carries the REAL stamp check (the fixture's copy
        # passes every check), committed like any kit update.
        real = (PIPELINE_PKG / "pre.py").read_text()
        (self.tree / "scripts" / "pipeline" / "pre.py").write_text(real)
        _git(self.tree, "add", "scripts/pipeline/pre.py")
        _git(self.tree, "commit", "-q", "-m", "welle-x P1 (#756): the real stamp check")
        self.commit()
        self.assertEqual(0, self.gate(PLAN_GREEN).returncode)
        res = run_strand(self.repo, "report", cwd=self.tree)
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        self.assertIn("no fresh pre-GREEN stamp", res.stderr)
        self.assertIn("no stamp", res.stderr)
        self.assertNotIn("GATE-SUMMARY", header_block(self.report("kit")).get("gate", ""))
        # From the main tree the strand's worktree copy is the one filled:
        # the same check.
        res = run_strand(self.repo, "report", "--strand", "kit")
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        if str(REPO) not in sys.path:
            sys.path.insert(0, str(REPO))
        from scripts.pipeline import pre

        def stamp(verdict="GREEN"):
            pre.write_stamp(str(self.tree), {
                "tree_hash": pre.tree_hash(str(self.tree)), "verdict": verdict,
                "at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                "lane_half": False, "reasons": [],
                "head": _git(self.tree, "rev-parse", "HEAD").stdout.strip()})
        stamp("RED")
        res = run_strand(self.repo, "report", cwd=self.tree)
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        self.assertIn("was RED", res.stderr)
        stamp()
        res = run_strand(self.repo, "report", cwd=self.tree)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("GATE-SUMMARY", header_block(self.report("kit"))["gate"])
        # A code commit after the stamp: the head is no longer the graded one.
        self.commit("scripts/kit_change.txt", "another change\n")
        res = run_strand(self.repo, "report", cwd=self.tree)
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        self.assertIn("tree changed since", res.stderr)

    def lane_summary(self, verdict, with_summary=True):
        """The newest run's summary.txt in the lane gate's layout (W3):
        station lines, GATE-SUMMARY, then GATE-LANE as the LAST line."""
        # Z16: derived from the writer (receipt.summary_lines without its
        # GATE-PLAN head), not typed by hand -- a drifting writer turns it red
        root = str(pathlib.Path(__file__).resolve().parents[2])
        if root not in sys.path:
            sys.path.insert(0, root)
        from scripts.pipeline import receipt
        doc = {"mode": "strand", "rev": "abc1234def", "secs": 9, "verdict": verdict,
               "stations": [{"name": "fmt", "scope": "rust", "secs": 1, "verdict": verdict}],
               "lane": "build02", "critical_path_s": 7, "lane_seconds": 9, "rounds": [{}]}
        lines = receipt.summary_lines(doc)[1:]
        self.assertEqual(lines[-1], "GATE-LANE build02 crit=7 lane_s=9 rounds=1")
        self.assertTrue(lines[1].startswith("GATE-SUMMARY strand abc1234 "), lines)
        if not with_summary:
            lines = [ln for ln in lines if not ln.startswith("GATE-SUMMARY ")]
        latest = (self.repo / "plans" / WAVE_DIR / "receipts" / "kit"
                  / "latest" / "summary.txt")
        latest.write_text("\n".join(lines) + "\n")
        return lines

    def test_report_reads_the_summary_line_before_a_trailing_lane_line(self):
        """Z12: GATE-LANE after GATE-SUMMARY is no verdict."""
        self.commit()
        self.assertEqual(0, self.gate(PLAN_GREEN).returncode)
        lines = self.lane_summary("GREEN")
        res = run_strand(self.repo, "report", cwd=self.tree)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertEqual('"%s"' % lines[1],
                         header_block(self.report("kit"))["gate"])

    def test_report_keeps_a_red_summary_red_behind_a_lane_line(self):
        self.commit()
        self.assertEqual(0, self.gate(PLAN_GREEN).returncode)
        lines = self.lane_summary("RED")
        res = run_strand(self.repo, "report", cwd=self.tree)
        self.assertEqual(1, res.returncode, res.stdout)
        self.assertIn("the gate is RED", res.stderr)
        self.assertEqual('"%s"' % lines[1],
                         header_block(self.report("kit"))["gate"])

    def test_report_refuses_a_summary_file_without_a_summary_line(self):
        """Fail closed: an earlier GREEN header must not survive for close."""
        self.commit()
        self.assertEqual(0, self.gate(PLAN_GREEN).returncode)
        self.assertEqual(0, run_strand(self.repo, "report",
                                       cwd=self.tree).returncode)
        self.assertIn("GREEN", header_block(self.report("kit"))["gate"])
        self.lane_summary("GREEN", with_summary=False)
        res = run_strand(self.repo, "report", cwd=self.tree)
        self.assertEqual(1, res.returncode, res.stdout)
        self.assertIn("no GATE-SUMMARY line", res.stderr)
        gate = header_block(self.report("kit"))["gate"]
        self.assertNotIn("GREEN", gate)
        self.assertIn("no GATE-SUMMARY", gate)
        res = run_strand(self.repo, "close", cwd=self.tree)
        self.assertEqual(1, res.returncode, res.stdout)
        self.assertNotIn("gh issue close", res.stdout)

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
        """Close reads the gate line of the newest STRAND receipt in the
        archive (GH #1107), not the head block: an ASK there closes nothing."""
        self.commit()
        self.assertEqual(0, self.gate(PLAN_GREEN).returncode)
        self.assertEqual(0, run_strand(self.repo, "report",
                                       cwd=self.tree).returncode)
        root = self.repo / "plans" / WAVE_DIR / "receipts" / "kit"
        summary = (root / "latest").resolve() / "summary.txt"
        text = summary.read_text()
        self.assertIn(" GREEN", text)
        summary.write_text(text.replace(" GREEN", " ASK"))
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
        # #1148: the strand commits its report on the branch; the merge
        # brings it into the main tree.
        _git(self.tree, "add", "plans")
        _git(self.tree, "commit", "-q", "-m", "welle-x P1 (#756): Bericht")
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

    def test_close_refuses_a_head_block_with_commits_of_master(self):
        """GH #1107: a head block written before a rebase named commits that
        master already had -- the comment listed fifty commits of other
        strands. Close refuses and asks for `report` again."""
        self.commit()
        self.assertEqual(0, self.gate(PLAN_GREEN).returncode)
        self.assertEqual(0, run_strand(self.repo, "report",
                                       cwd=self.tree).returncode)
        first = _git(self.repo, "rev-list", "--max-parents=0", "--abbrev-commit",
                     "--abbrev=8", "master").stdout.strip()
        path = self.report("kit")
        commits = header_block(path)["commits"]
        path.write_text(path.read_text().replace(
            "commits: %s" % commits, "commits: [%s, %s]" % (first, commits.strip("[]")), 1))
        res = run_strand(self.repo, "close", cwd=self.tree)
        self.assertNotEqual(0, res.returncode, res.stdout)
        self.assertIn("report again", res.stderr)
        self.assertNotIn("gh issue close", res.stdout)

    def test_close_lists_git_commits_when_master_moved_past_the_stored_basis(self):
        """GH #1107: the commit list is `merge-base(master, branch)..branch` at
        close time, never `<stored basis>..branch`. Master moves on (other
        strands merge), the strand rebases, and the head block still names
        the old basis and the pre-rebase SHA -- the comment must name the
        rebased commit only, none of master's."""
        self.commit()
        self.assertEqual(0, self.gate(PLAN_GREEN).returncode)
        self.assertEqual(0, run_strand(self.repo, "report",
                                       cwd=self.tree).returncode)
        foreign = self.foreign_master_commit()
        other = self.foreign_master_commit(rel="other/second.txt")
        _git(self.tree, "rebase", "-q", "master")
        mine = _git(self.tree, "rev-parse", "--short=8", "HEAD").stdout.strip()
        self.assertEqual(0, self.gate(PLAN_GREEN).returncode)
        res = run_strand(self.repo, "close", cwd=self.tree)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("Commits: %s\n" % mine, res.stdout)
        self.assertIn("from %s" % other, res.stdout)
        for sha in (foreign, other):
            self.assertNotIn("Commits: %s" % sha, res.stdout)
        self.assertNotIn("other/foreign.txt", res.stdout)

    def test_close_takes_the_newest_strand_receipt_not_a_newer_release_run(self):
        """GH #1107: a red release dry run archived after the green strand
        gate is the newest FILE, not the newest STRAND receipt -- it must not
        block the close, and its line must not travel into the comment."""
        self.commit()
        self.assertEqual(0, self.gate(PLAN_GREEN).returncode)
        self.assertEqual(0, run_strand(self.repo, "report",
                                       cwd=self.tree).returncode)
        root = self.repo / "plans" / WAVE_DIR / "receipts" / "kit"
        rev = _git(self.tree, "rev-parse", "--short=9", "HEAD").stdout.strip()
        run = root / ("99991231T235959Z-%s" % rev[:8])
        run.mkdir(parents=True)
        (run / "receipt.json").write_text(json.dumps({"mode": "release", "rev": rev}))
        (run / "summary.txt").write_text(
            "GATE-SUMMARY release %s 1/3 5s RED\n" % rev)
        (root / "latest").unlink()
        (root / "latest").symlink_to(run.name)
        res = run_strand(self.repo, "close", cwd=self.tree)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("GATE-SUMMARY strand ", res.stdout)
        self.assertNotIn("GATE-SUMMARY release", res.stdout)

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

    Since GH #1156 every token verb is the store's (`pipeline.py lanes`): the
    cases that pinned the old token file went with it, their semantics are
    the cases of `test_pipeline_lanes_cli.py` (carried over from TestToken,
    TestTokenLanes, TestTokenGuards and TestLanesStatus). What stays here is
    what the kit itself adds on top of the store.
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
        return run_strand(self.repo, "gate", "ci", *args, cwd=cwd or self.tree,
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

    def test_an_unarmed_host_gets_no_lock_file(self):
        """Unarmed is without a trace: no verb but `init` creates a file --
        `take` refuses (exit 2, Z11-M1), every other verb passes."""
        for want, args in ((2, ("take", "--strand", "welle-x/a")),
                           (0, ("release", "--strand", "welle-x/a")),
                           (0, ("check", "--strand", "welle-x/a", "--pid", "0")),
                           (0, ("who",)), (0, ("off",))):
            res = self.token(*args)
            self.assertEqual(want, res.returncode, "%s: %s" % (args, res.stderr))
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

    def utc(self, minutes):
        return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(self.T0 + 60 * minutes))

    def _old_form(self, holders):
        """A token file of the first form: one `pid` per holder."""
        (self.repo.parent / "tokens").write_text(json.dumps({
            "max": 3, "ttl_min": 90, "armed": "2027-01-15T08:00:00Z",
            "holders": holders, "waiting": []}))

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

    def test_pre_on_another_lane_than_the_held_one_refuses_at_once(self):
        # Kit find (a), 09.10.: `take` without --lane held one lane, the chain
        # then ran pre and gate with --host on another -- the gate's admission
        # refused only after pre. The mismatch is loud before pre starts.
        self.arm("--lanes", "north,south")
        res = self.token("take", "--lane", "north", cwd=self.tree)
        self.assertEqual(0, res.returncode, res.stderr)
        res = run_strand(self.repo, "pre", "--host", "south", cwd=self.tree)
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        self.assertIn("holds lane north, not south", res.stderr)

    def test_pre_with_a_holder_checks_the_lane_of_that_lease(self):
        # Review Z-A2 R3 M2: pre knew no --holder, so it compared the second
        # lease's --host with the main lease's lane.
        new = {"MECLAW_GATE_LEGACY": None}    # --holder is the store's (the old path has none)
        self.assertEqual(0, self.token("init", "--lanes", "north,south", env=new).returncode)
        self.assertEqual(0, self.token("take", "--lane", "north", cwd=self.tree, env=new).returncode)
        res = self.token("take", "--lane", "south", "--holder", "sat", cwd=self.tree, env=new)
        self.assertEqual(0, res.returncode, res.stderr)
        res = run_strand(self.repo, "pre", "--host", "north", "--holder", "sat", cwd=self.tree,
                         extra_env=dict(new, MECLAW_STRAND_NOW=str(self.T0)))
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        self.assertIn("holds lane south, not north", res.stderr)

    def test_pre_refuses_an_empty_or_malformed_holder(self):
        # Re-check Z-A M-b: `--holder ""` took the main lease in silence, and
        # `--holder 'bad!'` lost the rc 2 of `token lane` (pre ran without a lane).
        new = {"MECLAW_GATE_LEGACY": None}
        for bad in ("", "bad!", "x" * 33):
            res = run_strand(self.repo, "pre", "--holder", bad, cwd=self.tree,
                             extra_env=dict(new, MECLAW_STRAND_NOW=str(self.T0)))
            self.assertEqual(2, res.returncode, (bad, res.stdout + res.stderr))
            self.assertIn("--holder takes a short suffix", res.stderr)

    def test_token_refuses_the_old_strand_colon_form(self):
        # Kit find tv-br M5 (M7d2): `--strand <s>:<n>` was the key of the second
        # lease by accident -- the same name a `test` pushed as a ref. It is
        # `--strand <s> --holder <n>` (GH #1146); the old form takes nothing.
        new = {"MECLAW_GATE_LEGACY": None}
        self.assertEqual(0, self.token("init", "--lanes", "north,south", env=new).returncode)
        for verb in ("take", "release", "check", "lane"):
            res = self.token(verb, "--strand", "t1:sat", cwd=self.tree, env=new)
            self.assertEqual(2, res.returncode, (verb, res.stdout + res.stderr))
            self.assertIn("--strand t1 --holder sat", res.stderr)
        who = self.token("who", cwd=self.tree, env=new)
        self.assertNotIn("t1", who.stdout)

    # A waiter is an entry in a file: nothing in it takes the lane that comes
    # free, and a builder that ended its turn waited 115 min beside a free
    # lane until the orchestrator dropped it. `take --wait` blocks until the
    # take succeeds -- started in the background, its exit wakes the builder.

    def test_take_wait_passes_other_refusals_through(self):
        self.arm("--lanes", "north")
        res = self.token("take", "--strand", "welle-x/b", "--lane", "nowhere", "--wait")
        self.assertNotEqual(0, res.returncode)
        self.assertNotEqual(3, res.returncode)

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

    def test_a_threads_word_in_the_host_file_does_not_break_the_token(self):
        # `threads=<n>` is a lane's width cap (the readers that predate it
        # look at the first two words only): armed, held and named as ever.
        (self.repo.parent / "lanes").write_text(
            "north  builder@north.example.invalid  bmc-north.example.invalid  threads=12\n"
            "south  builder@south.example.invalid  threads=8\n"
            "spare  builder@spare.example.invalid\n")
        self.arm("--lanes", "north,south,spare")
        res = self.take("welle-x/a")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual("lane north", res.stdout.strip())
        self.assertIn("lanes: north south spare", self.token("who").stdout)

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

    def take_lane(self, strand, lane, minutes=0):
        return self.token("take", "--strand", strand, "--lane", lane, minutes=minutes)

    def test_release_names_the_waiter_the_freed_lane_goes_to(self):
        # N review I-2: the head of the queue waits for a lane that stays held; the
        # waiter behind it without a wish is the one the freed lane goes to, so it is
        # the one `release` names -- naming the head woke a strand that gets nothing.
        self.arm("--lanes", "south,north")
        self.take("welle-x/a")
        self.take("welle-x/b")
        self.assertEqual(3, self.take_lane("welle-x/c", "south").returncode)
        self.assertEqual(3, self.take("welle-x/d").returncode)
        res = self.token("release", "--strand", "welle-x/b")
        self.assertIn("next in the queue: welle-x/d (lane north", res.stderr)
        self.assertNotIn("next in the queue: welle-x/c", res.stderr)
        res = self.take("welle-x/d")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual("lane north", res.stdout.strip())

    def test_release_with_no_waiter_for_the_freed_lane_says_so(self):
        self.arm("--lanes", "south,north")
        self.take("welle-x/a")
        self.take("welle-x/b")
        self.assertEqual(3, self.take_lane("welle-x/c", "south").returncode)
        res = self.token("release", "--strand", "welle-x/b")
        self.assertIn("no waiter takes lane north", res.stderr)
        self.assertNotIn("next in the queue: welle-x/c", res.stderr)

    def _past_ttl_waiter_before_a_free_lane(self):
        """The measured case of GH #1053 (2026-10-07 00:12): lane south free,
        the head of the queue waits in a standing tree far beyond the TTL and
        never takes it, a strand behind it keeps asking."""
        self.arm("--lanes", "south")
        self.take("welle-x/a")
        self.token("check", "--strand", "welle-x/a", "--pid", str(os.getpid()))
        self.assertEqual(3, self.take(None, cwd=self.tree).returncode)
        self.assertEqual(3, self.take("welle-x/x", minutes=1).returncode)
        res = self.token("release", "--strand", "welle-x/a", minutes=100)
        self.assertIn("next in the queue: %s/kit (lane south" % WAVE, res.stderr)

    def test_who_names_the_lane_offered_to_a_waiter(self):
        self._past_ttl_waiter_before_a_free_lane()
        res = self.token("who", minutes=101)
        self.assertIn("#1 %s/kit  waiting 101 min  waiting beyond the ttl  offered south at %s"
                      % (WAVE, time.strftime("%Y-%m-%dT%H:%M:%SZ",
                                             time.gmtime(self.T0 + 6000))), res.stdout)
        self.assertNotIn("offered", res.stdout.split("#2", 1)[1].splitlines()[0])

    def test_a_named_waiter_keeps_its_lane_against_a_later_take(self):
        self.arm("--lanes", "south,north")
        self.take("welle-x/a")
        self.take("welle-x/b")
        self.assertEqual(3, self.take_lane("welle-x/c", "south").returncode)
        self.token("release", "--strand", "welle-x/a")
        # a strand without a wish that came later does not overtake c on south
        res = self.take("welle-x/d")
        self.assertEqual(3, res.returncode, res.stderr)
        res = self.take_lane("welle-x/c", "south")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual("lane south", res.stdout.strip())

    def test_a_named_waiter_does_not_block_another_lane(self):
        self.arm("--lanes", "south,north")
        self.take("welle-x/a")
        self.assertEqual(3, self.take_lane("welle-x/b", "south").returncode)
        # north is free and nobody ahead wants it
        res = self.take("welle-x/c")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual("lane north", res.stdout.strip())

    def test_who_names_the_wanted_lane_of_a_waiter(self):
        self.arm("--lanes", "south")
        self.take("welle-x/a")
        self.take_lane("welle-x/b", "south")
        self.assertRegex(self.token("who").stdout, r"#1 welle-x/b\s+for lane south")

    def test_a_held_other_lane_is_not_swapped(self):
        self.arm("--lanes", "south,north")
        self.take("welle-x/a")
        res = self.take_lane("welle-x/a", "north")
        self.assertEqual(2, res.returncode)
        self.assertIn("holds lane south, not north", res.stderr)

    def test_a_lane_without_lanes_armed_refuses(self):
        self.arm("--max", "2")
        res = self.take_lane("welle-x/a", "north")
        self.assertEqual(2, res.returncode)
        self.assertIn("not armed with lanes", res.stderr)

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
FAKE_SSH_TEMPLATE = """#!/usr/bin/env bash
opts=()
while [ $# -gt 0 ]; do
    case "$1" in
        -o|-p|-l|-i|-E|-F) opts+=("$1" "$2"); shift 2 ;;
        -*) opts+=("$1"); shift ;;
        *) break ;;
    esac
done
host="$1"; shift
printf '%s\\t%s\\n' "$host" "$*" >>"$FAKE_SSH_LOG"
# The options apart (Z5 keep-alive): `<host> TAB <options> TAB <command>`.
# One line per call: the newlines of a multi-line command become spaces.
cmd="$*"
[ -n "${FAKE_SSH_OPTS_LOG:-}" ] && printf '%s\\t%s\\t%s\\n' "$host" "${opts[*]}" "${cmd//$'\\n'/ }" >>"$FAKE_SSH_OPTS_LOG"
case "$host" in
    *unreachable*) echo "ssh: connect to host $host: Connection timed out" >&2; exit 255 ;;
esac
# A real ssh does not carry the caller's environment: the golden switches
# (GH #1051), the verdict's names and a secret beside them reach the host only
# on the command line -- the names of the one rule (fake_ssh.py, GH #1209).
# @fake-ssh: strip the transported names@
# The identity query of `lane_sync.sh coverage` (W6 review I2): the fake host
# runs on this machine, so it answers with FAKE_SSH_IDENTITY when a test gives
# one (a different machine); without it the real identity comes back.
case "$*" in
    *machine-id*) [ -n "${FAKE_SSH_IDENTITY:-}" ] && { printf '%b\\n' "$FAKE_SSH_IDENTITY"; exit 0; } ;;
esac
# A hold whose connection drops (re-check ZC M-neu-1): with FAKE_SSH_HOLD_DROP
# naming a fifo, ssh ends with 255 once a line arrives there; the far side
# lives on until its stdin closes, as a remote holder does.
case "$*" in
    *"exec cat >/dev/null"*) if [ -n "${FAKE_SSH_HOLD_DROP:-}" ]; then
        bash -c "$*" <&0 & read -r _ <"$FAKE_SSH_HOLD_DROP"; exit 255; fi
        # Re-check 2686990c8 M: a hold that lives to EOF and only then ends
        # with 255 -- the `wait` branch of the release, not the reaped one.
        if [ -n "${FAKE_SSH_HOLD_EOF_255:-}" ]; then bash -c "$*" <&0; exit 255; fi ;;
esac
exec bash -c "$*"
"""
FAKE_SSH = fake_ssh.script(FAKE_SSH_TEMPLATE)

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
# GH #942: the stations that read `.env`, need `node` or the `github-main`
# ref run ON the lane like every other -- the lane gets a secret-free `.env`,
# the ref is pushed along -- and nothing runs here afterwards.
PLAN_HOST_ENV = ("ok\tscope-ok\t0\ttrue\t\n"
                 "scenarios:memory\t1 case\t0\t%s\t\n")
# The audit reads the receipt and must find the lane's `ok` in it, and the
# ref must be there: `git rev-parse github-main` in the host's tree.
PLAN_HOST_LOCAL = ("ok\tscope-ok\t0\ttrue\t\n"
                   "display-lab\tunittest\t0\ttrue\t\n"
                   "export-audit\tR1-R17 dry\t0\tpython3 -c \"import json,subprocess,sys; "
                   "d=json.load(open(sys.argv[1])); "
                   "v={s['name']: s['verdict'] for s in d['stations']}; "
                   "subprocess.run(['git', 'rev-parse', '--verify', 'github-main'], check=True); "
                   "sys.exit(0 if v.get('ok') == 'GREEN' else 1)\" {receipt}\t\n")

# A fake `cargo` for `strand.sh test` on a fake host: it prints the summary
# line nextest prints and exits with FAKE_CARGO_RC. FAKE_CARGO_NO_SUMMARY
# leaves the summary out, FAKE_CARGO_ERR goes to stderr (a refused filter).
FAKE_CARGO = """#!/usr/bin/env bash
# every word on its own, a filterset with blanks or `|` stays one (review N6)
printf 'words %s\\n' "$(printf '<%s>' "$@")" >>"$FAKE_CARGO_LOG"
printf 'cargo %s\\n' "$*" >>"$FAKE_CARGO_LOG"
# W1: the environment a lane run hands cargo, and a run that only a kill ends
printf 'jobs %s threads %s incr %s nice %s\\n' "${CARGO_BUILD_JOBS:-}" "${NEXTEST_TEST_THREADS:-}" \
    "${CARGO_INCREMENTAL:-}" "$(nice)" >>"$FAKE_CARGO_LOG.env"
[ -z "${FAKE_CARGO_HOLD:-}" ] || exec tail -f /dev/null
# GH #1051: a golden writer, run in the host's tree. It rewrites a golden and,
# beside it, a tracked file that is no golden -- only the first may come back.
if [ "${GH277_WRITE_GOLDEN:-}" = 1 ]; then
    printf 'rewritten on the lane\\n' >plans/composition-reference/golden/talky.manifest
    printf 'touched by the run\\n' >README.md
    mkdir -p crates/x/tests/fixtures
    printf 'new golden\\n' >crates/x/tests/fixtures/gh1_new.json
    # a name `sha256sum` escapes (review N5): a backslash and a blank
    printf 'odd name\\n' >'crates/x/tests/fixtures/gh1 odd\\name.json'
fi
# the other switch of the allowlist (review N6)
if [ "${MECLAW_GH935_BLESS:-}" = 1 ]; then
    mkdir -p crates/x/tests/golden
    printf 'blessed\\n' >crates/x/tests/golden/gh935.txt
fi
# a golden touched HERE while the lane runs (review N6): FAKE_LOCAL_TOUCH
[ -z "${FAKE_LOCAL_TOUCH:-}" ] || printf 'edited here during the run\\n' >"$FAKE_LOCAL_TOUCH"
printf 'env %s\\n' "${FAKE_SECRET_API_KEY:-unset}" >>"$FAKE_CARGO_LOG"
[ -z "${FAKE_CARGO_ERR:-}" ] || echo "$FAKE_CARGO_ERR" >&2
# GH #1086: a stress series -- FAKE_CARGO_STRESS holds the failed tests of
# each iteration ("0 1 1 1 0"), printed in nextest's own stress shape.
if [ -n "${FAKE_CARGO_STRESS:-}" ]; then
    n=0; for f in $FAKE_CARGO_STRESS; do n=$((n+1)); done
    i=0; ok=0; bad=0
    for f in $FAKE_CARGO_STRESS; do
        i=$((i+1))
        echo " Stress test iteration $i/$n (00:00:00 elapsed so far)"
        if [ "$f" = 0 ]; then
            ok=$((ok+1))
            echo " Stress test [   0.420s] iteration $i/$n: 3 tests run: 3 passed (1 slow), 5 skipped"
        else
            bad=$((bad+1))
            # FAKE_CARGO_STRESS_KIND: the class of red (default `failed`)
            kind="${FAKE_CARGO_STRESS_KIND:-failed}"; mark=FAIL
            [ "$kind" != "timed out" ] || mark=TIMEOUT
            echo "        $mark [   0.020s] [$i/$n] (2/3) meclaw-cells::gh1_x a_case"
            echo " Stress test [   0.420s] iteration $i/$n: 3 tests run: $((3-f)) passed (1 slow), $f $kind, 5 skipped"
        fi
    done
    # nextest leaves the failed count out of a green series (measured 07.10.)
    tail=""; [ "$bad" = 0 ] || tail=", $bad failed"
    # FAKE_CARGO_STRESS_NO_SUMMARY: a lane cut off before nextest's summary
    [ -n "${FAKE_CARGO_STRESS_NO_SUMMARY:-}" ] \\
        || echo "     Summary [   2.100s] ${FAKE_CARGO_STRESS_RAN:-$n}/$n stress run iterations: $ok passed$tail"
    exit "${FAKE_CARGO_RC:-0}"
fi
if [ -z "${FAKE_CARGO_NO_SUMMARY:-}" ]; then
    echo "        PASS [   0.010s] meclaw-cells::gh1_x a_case"
    for f in ${FAKE_CARGO_FAILS:-}; do
        echo "        FAIL [   0.020s] meclaw-cells::gh1_x $f"
        i=0; while [ "$i" -lt "${FAKE_CARGO_TAIL:-0}" ]; do echo "  output line $i of $f"; i=$((i+1)); done
    done
    echo "     Summary [   0.420s] ${FAKE_CARGO_TOTAL:-3} tests run: ${FAKE_CARGO_PASSED:-3} passed, 0 skipped"
fi
exit "${FAKE_CARGO_RC:-0}"
"""

# A fake `df` for `lanes status`: the file system type a build host's target
# directory sits on (tmpfs before the disk was put in, ext4 after).
FAKE_DF = """#!/usr/bin/env bash
echo "Type Avail"
echo "$(cat @FSTYPE@ 2>/dev/null || echo tmpfs) 373G"
"""




class TestLanesStatus(TokenTestCase):
    LEGACY = None   # GH #1156: no old path left; the switch reaches nothing

    def test_one_line_per_lane(self):
        tmp = pathlib.Path(self._tmp.name)
        (self.repo.parent / "lanes").write_text(HOST_LANES)
        b = tmp / "bin"; b.mkdir()
        fake_ssh.install(b, FAKE_SSH_TEMPLATE)
        store_isolation.write_lane_fakes(b)
        (tmp / "remote").mkdir()
        res = run_strand(self.repo, "lanes", "status", extra_env={
            "MECLAW_GATE_LEGACY": self.LEGACY,
            "PATH": "%s:%s" % (b, os.environ["PATH"]),
            "MECLAW_LANE_ROOT": str(tmp / "remote"),
            "FAKE_SSH_LOG": str(tmp / "ssh.log")})
        self.assertEqual(0, res.returncode, res.stderr)
        lines = res.stdout.splitlines()
        self.assertEqual(3, len(lines), res.stdout)
        self.assertRegex(lines[0], r"^north\s+reachable\s+fs \S+\s+free \d+G\s+bare none\s")
        self.assertRegex(lines[2], r"^ghost\s+unreachable\s+state off$")

    def test_lanes_status_of_one_lane_goes_through_the_host_file(self):
        # GH #1144: the disk probe of one lane is `lanes status <lane>` -- the
        # target of the host file, never a hand-made `ssh <alias> df`.
        tmp = pathlib.Path(self._tmp.name)
        (self.repo.parent / "lanes").write_text(HOST_LANES)
        b = tmp / "bin"; b.mkdir()
        fake_ssh.install(b, FAKE_SSH_TEMPLATE)
        store_isolation.write_lane_fakes(b)
        (tmp / "remote").mkdir()
        env = {"MECLAW_GATE_LEGACY": self.LEGACY,
               "PATH": "%s:%s" % (b, os.environ["PATH"]),
               "MECLAW_LANE_ROOT": str(tmp / "remote"),
               "FAKE_SSH_LOG": str(tmp / "ssh.log")}
        res = run_strand(self.repo, "lanes", "status", "north", extra_env=env)
        self.assertEqual(0, res.returncode, res.stderr)
        lines = res.stdout.splitlines()
        self.assertEqual(1, len(lines), res.stdout)
        self.assertRegex(lines[0], r"^north\s+reachable\s+fs \S+\s+free \d+G")
        res = run_strand(self.repo, "lanes", "status", "nope", extra_env=env)
        self.assertEqual(2, res.returncode, res.stdout)
        self.assertIn("nope", res.stderr)
        # Review Zb R2 M6: the one lane named is unreachable -- its space is
        # unknown, no success (the line still says so).
        res = run_strand(self.repo, "lanes", "status", "ghost", extra_env=env)
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        self.assertRegex(res.stdout, r"^ghost\s+unreachable")

    def test_lanes_status_names_the_target_filesystem(self):
        tmp = pathlib.Path(self._tmp.name)
        (self.repo.parent / "lanes").write_text(HOST_LANES)
        b = tmp / "bin"; b.mkdir()
        fake_ssh.install(b, FAKE_SSH_TEMPLATE)
        store_isolation.write_lane_fakes(b)
        fstype_file = tmp / "df-fstype"   # the host's state: a file, the host runs under `env -i`
        (b / "df").write_text(FAKE_DF.replace("@FSTYPE@", shlex.quote(str(fstype_file)))); (b / "df").chmod(0o755)
        (tmp / "remote").mkdir()
        for fstype in ("tmpfs", "ext4"):
            with self.subTest(fs=fstype):
                fstype_file.write_text(fstype)
                res = run_strand(self.repo, "lanes", "status", extra_env={
                    "MECLAW_GATE_LEGACY": self.LEGACY,
                    "PATH": "%s:%s" % (b, os.environ["PATH"]),
                    "MECLAW_LANE_ROOT": str(tmp / "remote"),
                    "FAKE_SSH_LOG": str(tmp / "ssh.log")})
                self.assertEqual(0, res.returncode, res.stderr)
                self.assertRegex(res.stdout.splitlines()[0],
                                 r"^north\s+reachable\s+fs %s\s+free 373G\s+bare none\s" % fstype)


    def test_lanes_status_shows_inventory(self):
        tmp = pathlib.Path(self._tmp.name)
        remote = tmp / "remote"
        deps = remote / "target" / "debug" / "deps"
        deps.mkdir(parents=True)
        for name in ("lane_cells-0123456789abcdef", "lane_cells-fedcba9876543210",
                     "lane_other-00112233445566aa"):
            (deps / name).write_text("#!/bin/sh\n"); (deps / name).chmod(0o755)
        (deps / "liblane_cells-0123456789abcdef.rlib").write_text("lib\n")
        for t in ("a", "b"):
            (remote / "wt" / t).mkdir(parents=True)
        (self.repo.parent / "lanes").write_text(
            HOST_LANES.replace("north  builder@north.example.invalid",
                               "north  builder@north.example.invalid  slots=3")
            + "here  local  %s  class=egon  watch=guarded.service\n" % (tmp / "local"))
        b = tmp / "bin"; b.mkdir()
        fake_ssh.install(b, FAKE_SSH_TEMPLATE)
        store_isolation.write_lane_fakes(b)
        # The machine running the test is not the lane: on build02 its user
        # had no linger, the real `loginctl` said so, and hygiene's tail
        # grew a `linger no` this line never shows (W3 gate 09.10.,
        # 20261009T020350Z). This host's answer stands behind the lane fake.
        host = tmp / "host-bin"; host.mkdir()
        (host / "loginctl").write_text("#!/bin/sh\necho no\n"); (host / "loginctl").chmod(0o755)
        res = run_strand(self.repo, "lanes", "status", extra_env={
            "PATH": "%s:%s:%s" % (b, host, os.environ["PATH"]),
            "MECLAW_LANE_ROOT": str(remote),
            "FAKE_SSH_LOG": str(tmp / "ssh.log")})
        self.assertEqual(0, res.returncode, res.stderr)
        lines = res.stdout.splitlines()
        self.assertRegex(lines[0], r"^north\s+reachable\s+fs \S+\s+free \d+G\s+bare none\s+deps \d+G\s+"
                                   r"incr \d+G\s+gen 3/2\s+wt 2\s+slots 3\s+sccache \S+\s+"
                                   r"fp [0-9a-f]{8}\s+state (idle|busy)"
                                   # W2's hygiene tail, where `hygiene.py` is in the kit
                                   r"(\s+trees \d+\s+generations \d+\s+zombies \d+\s+stray \d+)?$")
        here = next(l for l in lines if l.startswith("here"))
        self.assertRegex(here, r"^here\s+reachable\s.*\sslots 1\s")


class TestLanesStatusPipeline(TestLanesStatus):
    """The same two cases through `pipeline.py lanes status`."""
    LEGACY = None

    def test_lanes_release_reaches_the_pipeline_verb(self):
        # Z-I (#1126): `lanes release` is `pipeline.py lanes release-run`; a
        # run the throw-away store does not know is refused, nothing probed.
        (self.repo.parent / "lanes").write_text(HOST_LANES)
        sqlite3.connect(str(self.repo.parent / "pipeline.db")).close()
        res = run_strand(self.repo, "lanes", "release", "north", "--run", "run-abc123",
                         extra_env={"MECLAW_GATE_LEGACY": self.LEGACY})
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        self.assertIn("no workpackage carries the run run-abc123", res.stderr)

    def test_lanes_browsers_reaches_the_transport_with_the_ssh_target(self):
        # GH #1161: `lanes browsers <lane> ...` is `lane_sync.sh browsers <ssh-target> ...`
        # with the target from the host file; an unknown lane is refused, nothing called.
        (self.repo.parent / "lanes").write_text(HOST_LANES)
        log = self.repo.parent / "sync.log"
        stub = self.repo / "scripts" / "lane_sync.sh"
        stub.write_text('#!/usr/bin/env bash\nprintf "%%s\\n" "$*" >> %s\nexit 7\n' % log)
        stub.chmod(0o755)
        # Re-Check S3 Teil 2 m-2: after every check or install the store learns the
        # lane's fingerprint again (`pipeline.py lanes fingerprint <lane>`), so a lane
        # refused for an engine is bookable once the engine starts
        plog = self.repo.parent / "pipeline.log"
        (self.repo / "scripts" / "pipeline.py").write_text(
            "import sys\nopen(%r, 'a').write(' '.join(sys.argv[1:]) + '\\n')\n" % str(plog))
        res = run_strand(self.repo, "lanes", "browsers", "north", "check", "webkit")
        self.assertEqual(7, res.returncode, res.stdout + res.stderr)
        self.assertEqual(["browsers builder@north.example.invalid check webkit"],
                         log.read_text().splitlines())
        self.assertEqual(["lanes fingerprint north"], plog.read_text().splitlines())
        res = run_strand(self.repo, "lanes", "fingerprint", "north")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertEqual(["lanes fingerprint north"] * 2, plog.read_text().splitlines())
        res = run_strand(self.repo, "lanes", "browsers", "nowhere", "check")
        self.assertNotEqual(0, res.returncode)
        self.assertIn("no lane nowhere", res.stderr)
        self.assertEqual(1, len(log.read_text().splitlines()))

    def test_lanes_browsers_never_installs_on_the_owners_machine(self):
        # Re-check tb-s3 h2 M-1: `install` runs apt as root on the lane. The owner's
        # machine (`class=egon`) and this machine (`local`) are refused before any call,
        # the way idle-off skips them; `check` reads only and stays allowed.
        (self.repo.parent / "lanes").write_text(
            HOST_LANES + "here  local  /srv/x\n"
            "egon  builder@egon.example.invalid  class=egon\n")
        log = self.repo.parent / "sync.log"
        stub = self.repo / "scripts" / "lane_sync.sh"
        stub.write_text('#!/usr/bin/env bash\nprintf "%%s|%%s\\n" "$*" "${MECLAW_LANE_CLASS:-}"'
                        ' >> %s\nexit 0\n' % log)
        stub.chmod(0o755)
        for lane, why in (("here", "transport local"), ("egon", "class egon")):
            with self.subTest(lane=lane):
                res = run_strand(self.repo, "lanes", "browsers", lane, "install", "webkit")
                self.assertNotEqual(0, res.returncode, res.stdout + res.stderr)
                self.assertIn("never installs on %s (%s)" % (lane, why), res.stderr)
                self.assertFalse(log.exists(), "the transport was called")
        res = run_strand(self.repo, "lanes", "browsers", "egon", "check")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        # the class reaches the transport, its own refusal is the second line;
        # the fingerprint after the recipe reads the stations' words from the
        # same transport (`env test`, read only, the class with it: M7s A-M2),
        # and nothing else is called
        self.assertEqual(["browsers builder@egon.example.invalid check|egon", "env test|egon"],
                         log.read_text().splitlines())
        res = run_strand(self.repo, "lanes", "browsers", "north", "install")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertEqual(["browsers builder@north.example.invalid install|", "env test|"],
                         log.read_text().splitlines()[2:])


# A stand-in for `scripts/pipeline/hygiene.py` (W2 of the dev-tempo wave):
# the kit's side of the hygiene is the command line it builds and where the
# script runs, not what the script does -- that has its own tests
# (`test_pipeline_hygiene.py`). The stub logs its argv as one JSON line
# (`argv0` is `-` when the script came over stdin, its path when a timer could
# call it), prints what the kit reads, and exits with FAKE_HYGIENE_RC_<CMD>.
HYGIENE_STUB = r'''import json, os, sys
args = sys.argv[1:]
KNOBS = "@KNOBS@"   # the host's state, set by the test (no name of the caller reaches a host)


def knob(name, default):
    path = os.path.join(KNOBS, name)
    return open(path).read() if os.path.exists(path) else default


with open(os.path.join(KNOBS, "..", "hygiene.log"), "a") as f:
    f.write(json.dumps({"argv0": sys.argv[0], "args": args}) + "\n")
cmd = args[0] if args else ""
if cmd == "status":
    print(knob("FAKE_HYGIENE_STATUS", "{}"))
elif cmd == "bench" and args[1:2] == ["new"]:
    print("bench note on stdout")
    print("/lane/bench/" + args[2])
else:
    print(json.dumps({"event": "hygiene_checked", "kind": "note", "data": {"verdict": "ok"}}))
sys.exit(int(knob("FAKE_HYGIENE_RC_" + cmd.upper().replace("-", "_"), "0")))
'''

HYGIENE_STATUS = json.dumps({
    "runner": "north", "state": "free", "free_g": 373, "trees": 2, "ws": 0,
    "bench": 1, "load1": 0.2, "zombies": 0, "reserve_hits": 0,
    "generations": 1, "sccache_g": 3, "fingerprint_short": "x",
    "linger": False, "stray_targets": 1})


class HygieneTestCase(TokenTestCase):
    """The shell side of the lane hygiene (dev-tempo W2): what the kit and
    the transport hand to `hygiene.py`, and where."""

    def setUp(self):
        super().setUp()
        tmp = pathlib.Path(self._tmp.name)
        (self.repo.parent / "lanes").write_text(HOST_LANES)
        self.bin = tmp / "bin"
        self.bin.mkdir()
        fake_ssh.install(self.bin, FAKE_SSH_TEMPLATE)
        store_isolation.write_lane_fakes(self.bin)
        (self.bin / "rsync").write_text(FAKE_RSYNC % (shutil.which("rsync") or "false"))
        (self.bin / "rsync").chmod(0o755)
        # the narrow point checks the local rsync of a remote way like ssh (OR-T10-25)
        fake_ssh.register_tool(self.bin / "rsync")
        self.remote = tmp / "remote"
        self.remote.mkdir()
        self.ssh_log = tmp / "ssh.log"
        self.ssh_log.write_text("")
        self.hyg_log = tmp / "hygiene.log"
        self.hyg_log.write_text("")
        # the stub runs on the host under the narrow point's `env -i`
        # (orchestrator OR-T10-2): its knobs are files, never the caller's env
        self.knobs = tmp / "knobs"
        self.knobs.mkdir()
        stub = HYGIENE_STUB.replace("@KNOBS@", str(self.knobs))
        # The kit (`strand.sh` of the main tree) and the transport (the
        # strand tree's own `lane_sync.sh`) each read the script beside them.
        for root in (self.repo, self.tree):
            (root / "scripts" / "pipeline").mkdir(parents=True, exist_ok=True)
            (root / "scripts" / "pipeline" / "hygiene.py").write_text(stub)

    def env(self, **extra):
        env = {"PATH": "%s:%s" % (self.bin, os.environ["PATH"]),
               "MECLAW_LANE_ROOT": str(self.remote),
               "FAKE_SSH_LOG": str(self.ssh_log)}
        for old in self.knobs.iterdir():
            old.unlink()
        for name, value in extra.items():
            if name.startswith("FAKE_HYGIENE_"):
                (self.knobs / name).write_text(value)
            else:
                env[name] = value
        return env

    def kit(self, *args, **extra):
        return run_strand(self.repo, *args, extra_env=self.env(**extra))

    def calls(self):
        return [json.loads(l) for l in self.hyg_log.read_text().splitlines()]

    def lanes_with(self, text):
        (self.repo.parent / "lanes").write_text(text)


class TestBench(HygieneTestCase):
    def test_bench_new_on_a_lane_installs_the_kit_copy_and_prints_the_path(self):
        res = self.kit("bench", "new", "cmp", "--ttl", "3", "--lane", "north")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertEqual("/lane/bench/cmp", res.stdout.splitlines()[-1])
        [call] = self.calls()
        self.assertEqual(["bench", "new", "cmp", "--ttl", "3", "--root", str(self.remote)],
                         call["args"])
        # A timer needs a path to call: the script was written under the lane
        # root, named by its content, and run from there.
        # (the stub carries its knob directory in the body, see setUp)
        body = (self.repo / "scripts" / "pipeline" / "hygiene.py").read_text()
        sha = hashlib.sha256(body.encode()).hexdigest()[:8]
        kit = self.remote / "kit" / sha / "hygiene.py"
        self.assertEqual(str(kit), call["argv0"])
        self.assertEqual(body, kit.read_text())
        self.assertIn("builder@north.example.invalid", self.ssh_log.read_text())

    def test_bench_collect_and_list_pipe_the_script(self):
        for args, want in ((("bench", "collect", "cmp", "--lane", "south"),
                            ["bench", "collect", "cmp"]),
                           (("bench", "list", "--lane", "south"), ["bench", "list"])):
            with self.subTest(args=args):
                self.hyg_log.write_text("")
                res = self.kit(*args)
                self.assertEqual(0, res.returncode, res.stdout + res.stderr)
                [call] = self.calls()
                self.assertEqual("-", call["argv0"])
                self.assertEqual(want + ["--root", str(self.remote)], call["args"])
        self.assertFalse((self.remote / "kit").exists())

    def test_bench_without_a_lane_runs_here(self):
        res = self.kit("bench", "list")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        [call] = self.calls()
        self.assertEqual(str(self.repo / "scripts" / "pipeline" / "hygiene.py"), call["argv0"])
        self.assertEqual(["bench", "list", "--root", str(self.remote)], call["args"])
        self.assertEqual("", self.ssh_log.read_text())

    def test_bench_refuses_a_wrong_call(self):
        for args in (("bench", "new", "cmp"),                      # no TTL
                     ("bench", "new", "cmp", "--ttl", "soon"),
                     ("bench", "new", "../x", "--ttl", "1"),
                     ("bench", "collect", "cmp", "--ttl", "1"),
                     ("bench", "collect"),
                     ("bench", "drop", "cmp"),
                     ("bench", "list", "--lane", "nowhere")):
            with self.subTest(args=args):
                res = self.kit(*args)
                self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        self.assertEqual([], self.calls())

    def test_bench_on_an_unreachable_lane_says_so(self):
        res = self.kit("bench", "list", "--lane", "ghost")
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        self.assertIn("lane ghost unreachable", res.stderr)


class TestLanesHygiene(HygieneTestCase):

    def test_lanes_status_keeps_the_old_line_without_a_status_word(self):
        res = self.kit("lanes", "status", FAKE_HYGIENE_STATUS="not json", FAKE_HYGIENE_RC_STATUS="1")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertRegex(res.stdout.splitlines()[0],
                         r"^north\s+reachable\s+fs \S+\s+free \d+G\s+bare none\s+deps .*\s+state \w+$")

    IDLE_LANES = ("north  builder@north.example.invalid  threads=8\n"
                  "home   builder@home.example.invalid  class=egon\n"
                  "box    local\n"
                  "south  builder@south.example.invalid  bmc-s.example.invalid\n")

    def test_idle_off_arms_every_lane_but_egon_and_local(self):
        self.lanes_with(self.IDLE_LANES)
        res = self.kit("lanes", "idle-off", "--after", "1.5")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertRegex(res.stdout, r"(?m)^home\s+skipped \(class egon\)$")
        self.assertRegex(res.stdout, r"(?m)^box\s+skipped \(transport local\)$")
        calls = self.calls()
        self.assertEqual(["north", "south"], [c["args"][c["args"].index("--lane") + 1] for c in calls])
        args = calls[0]["args"]
        self.assertEqual(["idle-off", "arm", "--lane", "north", "--after", "1.5",
                          "--lanes-file", str(self.repo.parent / "lanes")], args[:8])
        self.assertEqual(9, len(args), args)
        self.assertTrue(args[8].startswith("--holder-cmd="), args)
        # It runs HERE, on the owner's machine: no ssh.
        self.assertEqual("", self.ssh_log.read_text())

    def holder(self, lane):
        """The holder command the timer of `lane` runs, run now."""
        self.hyg_log.write_text("")
        res = self.kit("lanes", "idle-off", "--lane", lane)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        [call] = self.calls()
        argv = shlex.split(call["args"][-1][len("--holder-cmd="):])
        # The main tree's kit, the token and host file of the call.
        self.assertIn(str(self.repo / "scripts" / "strand.sh"), argv)
        self.assertIn("MECLAW_STRAND_TOKENS=%s" % (self.repo.parent / "tokens"), argv)
        res = subprocess.run(argv, capture_output=True, text=True, cwd=str(self._tmp.name),
                             env=kit_env(self.repo))
        self.assertEqual(0, res.returncode, res.stderr)
        return res.stdout

    def test_the_holder_check_names_only_a_held_lane(self):
        # Unarmed: nobody holds anything -- the host may go off.
        self.assertEqual("", self.holder("north"))
        # Armed with lanes: `who` lists every lane; only a HELD one counts.
        self.arm("--lanes", "north,south")
        self.assertEqual("", self.holder("north"))
        res = self.token("take", "--lane", "north", cwd=self.tree)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertEqual("held north\n", self.holder("south"))

    def test_a_failing_holder_check_keeps_the_host_on(self):
        self.arm("--lanes", "north,south")
        # the store is the one source (GH #1156): an unreadable one fails `who`
        (self.repo.parent / "pipeline.db").write_text("{ broken")
        self.assertRegex(self.holder("south"), r"^held south \(token who failed")

    def test_the_timer_calls_the_main_trees_script(self):
        # A strand's worktree goes at `strand close`; a timer armed from its
        # kit must not point into it. The main tree's script when it has
        # one, else the kit's own (before the hygiene is merged).
        self.lanes_with(self.IDLE_LANES)
        own = self.tree / "scripts" / "strand.sh"
        self.assertTrue(own.exists())
        for main_has_it, want in ((True, self.repo), (False, self.tree)):
            with self.subTest(main_has_it=main_has_it):
                if not main_has_it:
                    (self.repo / "scripts" / "pipeline" / "hygiene.py").unlink()
                self.hyg_log.write_text("")
                res = subprocess.run([str(own), "lanes", "idle-off", "--lane", "south"],
                                     cwd=str(self.tree), env=kit_env(self.repo, self.env()),
                                     capture_output=True, text=True, timeout=4 * kit_time.HANG)
                self.assertEqual(0, res.returncode, res.stdout + res.stderr)
                [call] = self.calls()
                self.assertEqual(str(want / "scripts" / "pipeline" / "hygiene.py"),
                                 call["argv0"])

    def test_idle_off_for_one_lane_and_its_refusals(self):
        self.lanes_with(self.IDLE_LANES)
        # without `--after`: MECLAW_IDLE_OFF_H when it arms, else 2 h
        res = self.kit("lanes", "idle-off", "--lane", "south")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        res = self.kit("lanes", "idle-off", "--lane", "south", MECLAW_IDLE_OFF_H="3")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertEqual([["--lane", "south", "--after", "2"], ["--lane", "south", "--after", "3"]],
                         [c["args"][2:6] for c in self.calls()])
        self.hyg_log.write_text("")
        for args in (("--lane", "nowhere"), ("--after", "0"), ("--after", "x"), ("--now",)):
            with self.subTest(args=args):
                res = self.kit("lanes", "idle-off", *args)
                self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        res = self.kit("lanes", "idle-off", "--lane", "south", FAKE_HYGIENE_RC_IDLE_OFF="2")
        self.assertEqual(1, res.returncode, res.stdout + res.stderr)
        self.assertRegex(res.stdout, r"^south\s+NOT armed \(exit 2\)$")




# The hygiene on ssh stdin, as `lane_sync.sh hygiene` starts it: in the kit's one
# Python (lane.PYTHON; the shell literal is pinned against it). A bare "python3 -"
# here was a second spelling that the M7s switch to `-I` left behind -- the
# positive asserts went red, the negative ones (refused before any check) went
# vacuous (re-check tv-bn M7t K2).
HYGIENE_STDIN = " ".join(lane.PYTHON) + " - "

# The host part runs on the lane under the narrow point's `env -i` (orchestrator
# OR-T10-2): the caller's environment never reaches it, so where it writes is
# part of its body (@COVLOG@, filled in by the test), never a variable.
COVERAGE_HOST_STUB = r'''#!/usr/bin/env bash
printf '%s\n' "$PWD" "$@" >"@COVLOG@"
echo '{"complete": true}' >"${1%/*}/coverage.json"
'''


@unittest.skipUnless(shutil.which("rsync"), "the lane overlay needs rsync")
class TestLaneCoverage(HygieneTestCase):
    """`lane_sync.sh coverage` (W6 review I1/I2): the instrumented run goes
    through the lane engine -- push, the hygiene check with its own tree and
    target, the fail-closed floor, the host part, the sweep -- in the layout
    element `coverage/{tree,target}`, never on the owner's machine."""

    def setUp(self):
        super().setUp()
        tmp = pathlib.Path(self._tmp.name)
        self.cov_log = tmp / "coverage-host.log"
        host = self.tree / "scripts" / "pipeline" / "coverage_host.sh"
        host.write_text(self.host_stub())
        host.chmod(0o755)
        self.dfbin = tmp / "df-bin"
        self.dfbin.mkdir()

    def host_stub(self, tail="", **paths):
        """The host stub with its paths in the body: @COVLOG@ and any
        @NAME@ given in `paths` (the host's state is files, not the env)."""
        body = COVERAGE_HOST_STUB + tail
        for name, value in dict(paths, COVLOG=self.cov_log).items():
            body = body.replace("@%s@" % name, str(value))
        return body

    def coverage(self, target="builder@north.example.invalid", free_g=999, pre_path=None,
                 out_dir=None, scope=None, **extra):
        (self.dfbin / "df").write_text('#!/usr/bin/env bash\necho "Avail"\necho "%dG"\n' % free_g)
        (self.dfbin / "df").chmod(0o755)
        env = dict(os.environ)
        path = "%s:%s:%s" % (self.dfbin, self.bin, os.environ["PATH"])
        if pre_path:
            path = "%s:%s" % (pre_path, path)
        env.update(self.env(PATH=path,
                            MECLAW_LANE_FLOOR_G="7",
                            MECLAW_LANE_NAME="north", **extra))
        env.setdefault("FAKE_SSH_IDENTITY",
                       "MID=0123456789abcdef0123456789abcdef\\nHOST=north-lane")
        if env["FAKE_SSH_IDENTITY"] == "":
            del env["FAKE_SSH_IDENTITY"]
        return subprocess.run([str(self.tree / "scripts" / "lane_sync.sh"), "coverage",
                               target, "dt-w6", "600"] + ([out_dir] if out_dir else [])
                              + ([scope] if scope else []),
                              cwd=self.tree, env=env,
                              capture_output=True, text=True, timeout=4 * kit_time.HANG)

    def test_the_browser_scope_reaches_the_host_part(self):
        # welle-tb S1b (#1165): the probe of the browser locks alone; `full`
        # keeps the host call as it was (the test above), anything else refused
        out = pathlib.Path(self._tmp.name) / "out"
        res = self.coverage(out_dir=str(out), scope="browser")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        _cwd, *host_args = self.cov_log.read_text().splitlines()
        sha = _git(self.tree, "rev-parse", "HEAD").stdout.strip()
        self.assertEqual([str(self.remote / "coverage" / "target"), "600", sha, "browser"],
                         host_args)
        self.cov_log.unlink()
        bad = self.coverage(out_dir=str(out), scope="nightly")
        self.assertNotEqual(0, bad.returncode)
        self.assertFalse(self.cov_log.exists())

    def test_the_verdict_names_travel_to_the_host_part(self):
        # GH #1210 (tv-bn M5): ssh carries no environment; a coverage run ran its
        # proofs without MECLAW_REQUIRE_BROWSERS -- a driver SKIP (no engine)
        # left a hole in the map instead of a red run. The names of
        # lane_sync.sh VERDICT_ENV travel as words, nothing else.
        # the other names empty: a pre or gate runs this module under its own
        # MECLAW_GATE_MODE / MECLAW_TESTING_RULES_RULING, which would travel too
        blank = {n: "" for n in lane.verdict_env()}
        res = self.coverage(**dict(blank, MECLAW_REQUIRE_BROWSERS="1",
                                   MECLAW_SOME_KEY="never-sent"))
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        line = next(l for l in self.ssh_log.read_text().splitlines() if "coverage_host.sh" in l)
        self.assertIn("MECLAW_REQUIRE_BROWSERS=1 scripts/pipeline/coverage_host.sh", line)
        self.assertNotIn("never-sent", line)

    def test_a_coverage_run_requires_the_browsers_unless_told_otherwise(self):
        # GH #1209 (re-check tv-bn M7 M4): the coverage run had the switch only
        # when its caller set it -- a driver SKIP (no engine) left a hole in the
        # map. Unset, it goes as `1`; an opt-out is kept and named as a NOTE.
        blank = {n: "" for n in lane.verdict_env() if n != "MECLAW_REQUIRE_BROWSERS"}
        saved = os.environ.pop("MECLAW_REQUIRE_BROWSERS", None)
        if saved is not None:
            self.addCleanup(os.environ.__setitem__, "MECLAW_REQUIRE_BROWSERS", saved)
        res = self.coverage(**blank)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        line = next(l for l in self.ssh_log.read_text().splitlines() if "coverage_host.sh" in l)
        self.assertIn("MECLAW_REQUIRE_BROWSERS=1 scripts/pipeline/coverage_host.sh", line)
        self.assertNotIn("opt-out", res.stderr)
        self.ssh_log.unlink()
        res = self.coverage(MECLAW_REQUIRE_BROWSERS="0", **blank)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        line = next(l for l in self.ssh_log.read_text().splitlines() if "coverage_host.sh" in l)
        self.assertIn("MECLAW_REQUIRE_BROWSERS=0 scripts/pipeline/coverage_host.sh", line)

    def test_a_browsers_opt_out_is_in_the_coverage_receipt(self):
        # Re-check tv-bn M7c M1: the opt-out stood only on stderr -- the map
        # (coverage.json, the receipt `pipeline coverage build` keeps and
        # journals) read like a full run, the hole of a SKIPped driver unnamed.
        blank = {n: "" for n in lane.verdict_env() if n != "MECLAW_REQUIRE_BROWSERS"}
        out = pathlib.Path(self._tmp.name) / "out"
        for given, want in (("0", "0"), ("1", None)):
            with self.subTest(given=given):
                res = self.coverage(out_dir=str(out), MECLAW_REQUIRE_BROWSERS=given, **blank)
                self.assertEqual(0, res.returncode, res.stdout + res.stderr)
                cov = json.loads((out / "coverage.json").read_text())
                self.assertEqual(want, cov.get("browsers_opt_out"), cov)

    def test_the_coverage_run_goes_through_check_floor_host_part_and_sweep(self):
        res = self.coverage()
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        lines = self.ssh_log.read_text().splitlines()
        check = next(i for i, l in enumerate(lines) if HYGIENE_STDIN + "check" in l)
        host = next(i for i, l in enumerate(lines) if "coverage_host.sh" in l)
        collect = next(i for i, l in enumerate(lines) if HYGIENE_STDIN + "collect" in l)
        self.assertLess(check, host)
        self.assertLess(host, collect)
        tree = self.remote / "coverage" / "tree"
        target = self.remote / "coverage" / "target"
        args = [c for c in self.calls() if c["argv0"] == "-"][0]["args"]
        self.assertEqual(str(tree), args[args.index("--own-tree") + 1])
        self.assertEqual(str(target), args[args.index("--own-target") + 1])
        cwd, *host_args = self.cov_log.read_text().splitlines()
        self.assertEqual(str(tree), cwd)
        sha = _git(self.tree, "rev-parse", "HEAD").stdout.strip()
        self.assertEqual([str(target), "600", sha], host_args)
        self.assertTrue((self.remote / "coverage" / "coverage.json").is_file())

    def test_below_the_floor_the_coverage_run_never_builds(self):
        res = self.coverage(free_g=1)
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        self.assertFalse(self.cov_log.exists())

    def test_the_owner_machine_and_an_egon_lane_are_refused(self):
        for target, extra in (("local", {}),
                              ("builder@north.example.invalid", {"MECLAW_LANE_CLASS": "egon"})):
            with self.subTest(target=target):
                res = self.coverage(target=target, **extra)
                self.assertEqual(5, res.returncode, res.stdout + res.stderr)
                self.assertIn("refused", res.stderr)
                self.assertEqual("", self.ssh_log.read_text())
                self.assertFalse(self.cov_log.exists())


    def test_a_lane_that_is_this_machine_is_refused_by_identity(self):
        """I2 rest: the label says build lane, the host is this machine -- the
        machine-id or the host name gives it away, exit 5 before any build."""
        here_id = pathlib.Path("/etc/machine-id").read_text().strip() \
            if pathlib.Path("/etc/machine-id").is_file() else ""
        here_name = socket.gethostname()
        cases = {"same machine (real identity)": "",
                 "same host name": "MID=ffffffffffffffffffffffffffffffff\\nHOST="
                                   + here_name.upper(),
                 "unreadable identity": "\\n"}
        if here_id:
            cases["same machine-id"] = "MID=" + here_id + "\\nHOST=somewhere-else"
        for name, ident in cases.items():
            with self.subTest(name):
                self.cov_log.unlink(missing_ok=True)
                self.ssh_log.write_text("")
                res = self.coverage(FAKE_SSH_IDENTITY=ident)
                self.assertEqual(5, res.returncode, res.stdout + res.stderr)
                self.assertIn("refused", res.stderr)
                self.assertFalse(self.cov_log.exists())
                self.assertNotIn(HYGIENE_STDIN + "check", self.ssh_log.read_text())

    def assert_refused_before_any_build(self, res):
        self.assertEqual(5, res.returncode, res.stdout + res.stderr)
        self.assertIn("refused", res.stderr)
        self.assertFalse(self.cov_log.exists())
        self.assertNotIn(HYGIENE_STDIN + "check", self.ssh_log.read_text())

    def test_an_unreadable_own_identity_is_refused(self):
        """W6 review M1 (a): with this side's machine-id or host name empty no
        comparison can prove the target is another machine -- exit 5, never
        open (fail-closed). `cat` and `hostname` are faked through PATH; the
        fake `cat` hands every other file to the next `cat` on PATH."""
        idbin = pathlib.Path(self._tmp.name) / "id-bin"
        idbin.mkdir()
        fake_cat = ('#!/usr/bin/env bash\n'
                    'for a; do [ "$a" = /etc/machine-id ] && exit 0; done\n'
                    'exec "$(type -P -a cat | sed -n 2p)" "$@"\n')
        fake_hostname = '#!/usr/bin/env bash\nexit 0\n'
        for name, fakes in (("empty machine-id", {"cat": fake_cat}),
                            ("empty host name", {"hostname": fake_hostname}),
                            ("both empty", {"cat": fake_cat, "hostname": fake_hostname})):
            with self.subTest(name):
                for f in idbin.iterdir():
                    f.unlink()
                for tool, body in fakes.items():
                    (idbin / tool).write_text(body)
                    (idbin / tool).chmod(0o755)
                self.cov_log.unlink(missing_ok=True)
                self.ssh_log.write_text("")
                res = self.coverage(pre_path=str(idbin))
                self.assert_refused_before_any_build(res)
                self.assertIn("this machine cannot be read", res.stderr)
                # GH #1133 N2: this side refuses before anything reaches the lane.
                self.assertEqual("", self.ssh_log.read_text())

    def test_a_polluted_target_identity_is_refused(self):
        """W6 review M1 (b): the target must answer with exactly one
        non-empty machine-id line and one non-empty host name line. A banner
        from the login shell, two ids, a glued line or a blank id is not a
        proof of another machine -- exit 5."""
        mid = "MID=0123456789abcdef0123456789abcdef"
        cases = {"banner before the id": "Welcome to north\\n" + mid + "\\nHOST=north-lane",
                 "banner glued to the id": "Welcome" + mid + "\\nHOST=north-lane",
                 "two ids": mid + "\\nMID=ffffffffffffffffffffffffffffffff\\nHOST=north-lane",
                 "line after the host name": mid + "\\nHOST=north-lane\\nbye",
                 "empty machine-id": "MID=\\nHOST=north-lane",
                 "empty host name": mid + "\\nHOST=",
                 "blank inside the id": "MID=0123 4567\\nHOST=north-lane",
                 "unprefixed lines": "0123456789abcdef0123456789abcdef\\nnorth-lane"}
        for name, ident in cases.items():
            with self.subTest(name):
                self.cov_log.unlink(missing_ok=True)
                self.ssh_log.write_text("")
                res = self.coverage(FAKE_SSH_IDENTITY=ident)
                self.assert_refused_before_any_build(res)
                # GH #1133 N1: the exact line, naming the target.
                self.assertIn("lane_sync: refused -- the identity of "
                              "builder@north.example.invalid cannot be read", res.stderr)

    def test_the_tree_lock_is_held_from_push_through_the_map_fetch(self):
        """GH #1133 M4: coverage/tree.lock is held over the host part and the
        fetch of the map (as a gate holds from push to fetch), released after."""
        host = self.tree / "scripts" / "pipeline" / "coverage_host.sh"
        host.write_text(self.host_stub('flock -n "$PWD.lock" true && echo free >>"@COVLOG@"'
                                       ' || echo held >>"@COVLOG@"\n'))
        _git(self.tree, "commit", "-qam", "host stub with a lock probe")
        out = pathlib.Path(self._tmp.name) / "map"
        res = self.coverage(out_dir=str(out))
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertEqual("held", self.cov_log.read_text().splitlines()[-1])
        self.assertEqual({"complete": True}, json.loads((out / "coverage.json").read_text()))
        lock = self.remote / "coverage" / "tree.lock"
        self.assertEqual(0, subprocess.run(["flock", "-n", str(lock), "true"]).returncode)

    def test_a_hold_that_ends_with_255_at_eof_fails_the_run_and_says_so(self):
        """Re-check 2686990c8 M: the hold lives until the release closes its
        stdin and only then ends with 255, so bash has not reaped it and
        `wait` reports the 255. The run is red, the warning stands, and the
        hygiene collect still runs after the host part."""
        out = pathlib.Path(self._tmp.name) / "map"
        res = self.coverage(out_dir=str(out), FAKE_SSH_HOLD_EOF_255="1")
        self.assertNotEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertNotEqual(255, res.returncode, res.stdout + res.stderr)
        self.assertIn("ended before the run did", res.stderr)
        lines = self.ssh_log.read_text().splitlines()
        host = next(i for i, l in enumerate(lines) if "coverage_host.sh" in l)
        collect = [i for i, l in enumerate(lines) if HYGIENE_STDIN + "collect" in l]
        self.assertTrue(collect and collect[-1] > host, "hygiene_after did not run after the host part")

    def test_a_hold_that_drops_mid_run_fails_the_run_and_says_so(self):
        """Re-check ZC M-neu-1: bash reaps a coproc that ended early and drops
        its variables; under `set -u` the release must not die on them.
        Re-check 8d26c0ec4 I-1: the map then came back without the tree lock,
        so the run is not green (exit != 0, whether bash reaped the hold or
        `wait` reports its 255); the hygiene collect still runs after it."""
        fifo = pathlib.Path(self._tmp.name) / "hold-drop"
        os.mkfifo(fifo)
        host = self.tree / "scripts" / "pipeline" / "coverage_host.sh"
        host.write_text(self.host_stub('echo drop >"@DROP@"\n', DROP=fifo))
        _git(self.tree, "commit", "-qam", "host stub that drops the hold")
        out = pathlib.Path(self._tmp.name) / "map"
        res = self.coverage(out_dir=str(out), FAKE_SSH_HOLD_DROP=str(fifo))
        self.assertNotEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertNotEqual(255, res.returncode, res.stdout + res.stderr)
        self.assertNotIn("unbound variable", res.stderr)
        self.assertIn("ended before the run did", res.stderr)
        lines = self.ssh_log.read_text().splitlines()
        host = next(i for i, l in enumerate(lines) if "coverage_host.sh" in l)
        collect = [i for i, l in enumerate(lines) if HYGIENE_STDIN + "collect" in l]
        self.assertTrue(collect and collect[-1] > host, "hygiene_after did not run after the host part")

    def test_an_exported_coverage_switch_moves_no_other_verb(self):
        """GH #1133 M3: the coverage layout is the verb's own; a caller's
        MECLAW_LANE_COVERAGE=1 leaves paths/run/test/guard/hold where they were."""
        env = dict(os.environ)
        env.update(self.env(PATH="%s:%s" % (self.bin, os.environ["PATH"]),
                            MECLAW_LANE_NAME="north"))
        plain = subprocess.run([str(self.tree / "scripts" / "lane_sync.sh"), "paths", "dt-w6"],
                               cwd=self.tree, env=env, capture_output=True, text=True,
                               timeout=2 * kit_time.HANG)
        env["MECLAW_LANE_COVERAGE"] = "1"
        res = subprocess.run([str(self.tree / "scripts" / "lane_sync.sh"), "paths", "dt-w6"],
                             cwd=self.tree, env=env, capture_output=True, text=True, timeout=2 * kit_time.HANG)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertEqual(plain.stdout, res.stdout)
        self.assertNotIn("/coverage/", res.stdout)

    def test_an_unreachable_lane_is_exit_2_not_a_refusal(self):
        """GH #1133 M2: ssh rc 255 on the identity probe is an unreachable
        lane (exit 2); only an answer that is no clean identity refuses (5)."""
        res = self.coverage(target="builder@unreachable.example.invalid")
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        self.assertIn("lane_sync: builder@unreachable.example.invalid is unreachable", res.stderr)
        self.assertNotIn("refused", res.stderr)
        self.assertFalse(self.cov_log.exists())

    def test_a_clean_different_identity_goes_on(self):
        """W6 review M1 (c): both sides clean and different -- the run goes on
        to the hygiene check."""
        res = self.coverage(
            FAKE_SSH_IDENTITY="MID=0123456789abcdef0123456789abcdef\\nHOST=north-lane")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertNotIn("refused", res.stderr)
        self.assertIn(HYGIENE_STDIN + "check", self.ssh_log.read_text())

    # Z5 (dev-tempo Z list): the three holds of `lane_sync.sh` -- `hold`, the
    # push lock and the coverage tree lock -- idle on stdin for a whole gate
    # run. They went over `remote()` without `ServerAliveInterval`, the one
    # keep-alive only `remote_long` carried: an idle connection a middlebox
    # drops ends the hold, and the build path is unlocked under a running
    # gate. Each hold carries the keep-alive; the fake ssh logs the options.

    @property
    def opts_log(self):
        return pathlib.Path(self._tmp.name) / "ssh-opts.log"

    def hold_calls(self):
        text = self.opts_log.read_text() if self.opts_log.exists() else ""
        rows = [l.split("\t", 2) for l in text.splitlines()]
        return [(host, opts) for host, opts, cmd in rows if "exec cat >/dev/null" in cmd]

    def lane_sync(self, *args):
        env = dict(os.environ)
        env.update(self.env(PATH="%s:%s" % (self.bin, os.environ["PATH"]),
                            MECLAW_LANE_NAME="north", FAKE_SSH_OPTS_LOG=str(self.opts_log)))
        return subprocess.run([str(self.tree / "scripts" / "lane_sync.sh")] + list(args),
                              cwd=self.tree, env=env, capture_output=True, text=True,
                              stdin=subprocess.DEVNULL, timeout=kit_time.HANG)

    def test_the_hold_verb_carries_the_keep_alive(self):
        res = self.lane_sync("hold", "builder@north.example.invalid", "dt-w6")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertTrue(res.stdout.startswith("held "), res.stdout)
        [(host, opts)] = self.hold_calls()
        self.assertEqual("builder@north.example.invalid", host)
        self.assertIn("ServerAliveInterval=", opts)

    def test_the_push_lock_carries_the_keep_alive(self):
        self.lane_sync("push", "builder@north.example.invalid", "dt-w6")
        calls = self.hold_calls()
        self.assertTrue(calls, "no hold reached the fake ssh")
        for host, opts in calls:
            self.assertIn("ServerAliveInterval=", opts, host)

    def test_the_coverage_hold_carries_the_keep_alive(self):
        res = self.coverage(FAKE_SSH_OPTS_LOG=str(self.opts_log))
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        calls = self.hold_calls()
        self.assertTrue(calls, "no hold reached the fake ssh")
        for host, opts in calls:
            self.assertIn("ServerAliveInterval=", opts, host)


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

    def test_tier_refuses_without_a_token(self):
        self.arm()
        res = run_tier(self.repo, self.tree)
        self.assertEqual(3, res.returncode, res.stdout + res.stderr)
        self.assertIn(REFUSED, res.stderr)
        self.assertNotIn("tier-dry:", res.stdout)

    def test_k9_the_tier_says_seen_after_its_run(self):
        # K9 (welle-validierung-2): a long holder without a closing heartbeat
        # left `seen` at its start, and the idle reclaim (15 min) read a run
        # that had just ended as "idle". The tier checks in with its pid before
        # the run and says `seen` once more after it, as `strand.sh gate` does.
        calls = self.repo.parent / "strand-calls"
        stub = self.tree / "scripts" / "strand.sh"
        stub.write_text("#!/bin/sh\necho \"$*\" >> %s\n" % calls)
        stub.chmod(0o755)
        res = run_tier(self.repo, self.tree)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("tier-dry:", res.stdout)
        lines = calls.read_text().splitlines()
        self.assertEqual(2, len(lines), lines)
        self.assertRegex(lines[0], r"^token check --pid [1-9][0-9]*$")
        self.assertEqual("token check --pid 0", lines[1])

    def test_k9_an_aborted_tier_says_seen_all_the_same(self):
        # Re-check v2-fix-b M4: `seen` after the run sat on the straight path
        # only -- a tier stopped by a signal (^C, kill) left it at its start,
        # and the dead process no longer guarded the lane. The tier is held
        # on the cargo lock with the token checked in, its process group gets
        # the signal (as ^C sends it): the kit hears `seen` exactly once, and
        # the tier still dies of that signal. The signal goes out once `flock`
        # runs -- it says so itself (a wrapper on PATH) -- so the tier waits on
        # a child that dies of it, as it waits on nextest in a real run.
        calls = self.repo.parent / "strand-calls"
        stub = self.tree / "scripts" / "strand.sh"
        stub.write_text("#!/bin/sh\necho \"$*\" >> %s\n" % calls)
        stub.chmod(0o755)
        real_flock = shutil.which("flock")
        self.assertIsNotNone(real_flock)
        wrap = self.repo.parent / "flock-bin"
        wrap.mkdir()
        (wrap / "flock").write_text("#!/bin/sh\necho '=== flock waits'\nexec %s \"$@\"\n"
                                    % shlex.quote(real_flock))
        (wrap / "flock").chmod(0o755)
        env = kit_env(self.repo, {"PATH": "%s:%s" % (wrap, os.environ["PATH"])})
        env["MECLAW_TIER_DRY"] = "1"
        for sig in (signal.SIGTERM, signal.SIGINT):
            with self.subTest(sig=sig.name), open(self.repo.parent / "cargo.lock", "w") as lock:
                calls.unlink(missing_ok=True)
                fcntl.flock(lock, fcntl.LOCK_EX)
                proc = subprocess.Popen([str(self.tree / "scripts" / "test-tier.sh"), "t0"],
                                        cwd=str(self.tree), env=env, stdout=subprocess.PIPE,
                                        stderr=subprocess.PIPE, text=True,
                                        start_new_session=True)
                try:
                    # the token is checked in before `flock` runs; the lock is ours
                    for line in proc.stdout:
                        if line.startswith("=== flock waits"):
                            break
                    else:
                        self.fail("the tier never reached the cargo lock")
                    os.killpg(proc.pid, sig)
                    out, err = proc.communicate(timeout=4 * kit_time.HANG)
                finally:
                    if proc.poll() is None:
                        os.killpg(proc.pid, signal.SIGKILL)
                        proc.communicate()
                    fcntl.flock(lock, fcntl.LOCK_UN)
                self.assertEqual(-sig, proc.returncode, out + err)
                self.assertNotIn("tier-dry:", out)
                lines = calls.read_text().splitlines()
                self.assertEqual(2, len(lines), lines)
                self.assertRegex(lines[0], r"^token check --pid [1-9][0-9]*$")
                self.assertEqual("token check --pid 0", lines[1])

    def test_tier_inside_the_gate_needs_no_token(self):
        self.arm()
        for var in ("MECLAW_CARGO_LOCK_HELD", "CI"):
            with self.subTest(var=var):
                res = run_tier(self.repo, self.tree, {var: "1"})
                self.assertEqual(0, res.returncode, res.stdout + res.stderr)
                self.assertIn("tier-dry:", res.stdout)
                self.assertNotIn(REFUSED, res.stderr)

    # --- GH #942: while the lanes are armed, the owner's machine runs no cargo.
    # A lab colony died at load 15 on 2026-10-02 while strands built there.

    LANES_REFUSED = "lanes active: use scripts/strand.sh test|gate --host <lane>"
    PLAN_CARGO = "ok\tscope-ok\t0\ttrue\t\nclippy\tworkspace\t1\ttrue\t\n"

    def run_gate_sh(self, *args, plan=PLAN_CARGO, env=None):
        extra = {"MECLAW_GATE_PLAN": str(self.plan_file(plan, name="cargo-plan.tsv")),
                 "MECLAW_GATE_DRY": "1"}
        extra.update(env or {})
        return subprocess.run([str(self.tree / "scripts" / "gate.sh")] + list(args),
                              cwd=str(self.tree), env=kit_env(self.repo, extra),
                              capture_output=True, text=True, timeout=4 * kit_time.HANG)

    def test_lanes_mode_on_the_store_refuses_local_cargo(self):
        # C-C1: `lanes init --lanes` arms the store (`lease_pools`) and writes
        # no token file -- the tier on this machine is refused all the same.
        (self.repo.parent / "lanes").write_text(HOST_LANES)
        res = self.token("init", "--lanes", "north,south", env={"MECLAW_GATE_LEGACY": None})
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertFalse((self.repo.parent / "tokens").exists(), "the store writes no token file")
        res = run_tier(self.repo, self.tree)
        self.assertEqual(3, res.returncode, res.stdout + res.stderr)
        self.assertIn(self.LANES_REFUSED, res.stderr)
        self.assertNotIn("tier-dry:", res.stdout)

    # Re-check pd-legacy M-2: the refusal in `gate_legacy.sh` guards the one
    # mode left there, `ci`, outside the CI runner as well. Its pins went with
    # the old path; these drive `gate.sh ci` on a store armed with lanes.
    LANES_CI_NOTE = "lanes armed on this host, cargo runs here under CI"

    def arm_the_store(self):
        (self.repo.parent / "lanes").write_text(HOST_LANES)
        res = self.token("init", "--lanes", "north,south", env={"MECLAW_GATE_LEGACY": None})
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)

    def test_a_local_ci_gate_with_cargo_is_refused_while_the_lanes_are_armed(self):
        self.arm_the_store()
        res = self.run_gate_sh("ci", env={"CI": ""})
        self.assertEqual(3, res.returncode, res.stdout + res.stderr)
        self.assertIn(self.LANES_REFUSED, res.stderr)
        self.assertNotIn("GATE-SUMMARY", res.stdout)
        # A plan without cargo is no business of the lanes.
        res = self.run_gate_sh("ci", plan=PLAN_GREEN, env={"CI": ""})
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)

    def test_the_ci_runner_passes_the_lanes_refusal_with_a_note(self):
        self.arm_the_store()
        res = self.run_gate_sh("ci", env={"CI": "true"})
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertRegex(res.stdout,
                         r"(?m)^GATE lanes-ci \[.*\] 0s NOTE %s$" % re.escape(self.LANES_CI_NOTE))

    def test_the_way_out_of_the_lanes_refusal_names_its_reason(self):
        self.arm_the_store()
        res = self.run_gate_sh("ci", env={"CI": "", "MECLAW_LANE_LOCAL_OK": "owner said so"})
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertRegex(res.stdout, r"(?m)^GATE lane-local-ok \[.*\] 0s NOTE owner said so$")

    def test_a_ci_gate_on_an_unarmed_host_says_nothing_of_lanes(self):
        for ci in ("", "true"):
            with self.subTest(CI=ci):
                res = self.run_gate_sh("ci", env={"CI": ci})
                self.assertEqual(0, res.returncode, res.stdout + res.stderr)
                self.assertNotIn(self.LANES_REFUSED, res.stderr)
                self.assertNotIn("lanes-ci", res.stdout)

    def test_tier_reuse_by_hand_is_no_way_past_the_lanes(self):
        # Delta C M-neu-3: MECLAW_TIER_REUSE set by hand ran test binaries on
        # this machine; only the gate's own reuse (MECLAW_TIER_LANE) passes.
        (self.repo.parent / "lanes").write_text(HOST_LANES)
        self.arm("--lanes", "north,south")
        reuse = str(self.tree / "reuse")
        res = run_tier(self.repo, self.tree, {"MECLAW_TIER_REUSE": reuse})
        self.assertEqual(3, res.returncode, res.stdout + res.stderr)
        self.assertIn(self.LANES_REFUSED, res.stderr)
        res = run_tier(self.repo, self.tree, {"MECLAW_TIER_REUSE": reuse, "MECLAW_TIER_LANE": "1"})
        self.assertNotIn(self.LANES_REFUSED, res.stderr)

    def test_a_store_the_probe_cannot_read_counts_as_armed(self):
        # Delta C M-neu-4: fail-closed -- a broken store or lanes column
        # refuses local cargo; a store without the lease tables does not.
        bad = self.repo.parent / "broken.db"
        bad.write_bytes(b"not a database at all" * 100)
        res = run_tier(self.repo, self.tree, {"MECLAW_PIPELINE_DB": str(bad)})
        self.assertEqual(3, res.returncode, res.stdout + res.stderr)
        self.assertIn(self.LANES_REFUSED, res.stderr)
        odd = self.repo.parent / "odd.db"
        db = sqlite3.connect(str(odd))
        db.execute("CREATE TABLE lease_pools (pool TEXT, lanes TEXT, armed TEXT)")
        db.execute("INSERT INTO lease_pools VALUES ('cargo', '{not json', 'now')")
        db.commit()
        db.close()
        res = run_tier(self.repo, self.tree, {"MECLAW_PIPELINE_DB": str(odd)})
        self.assertEqual(3, res.returncode, res.stdout + res.stderr)
        self.assertIn(self.LANES_REFUSED, res.stderr)
        empty = self.repo.parent / "empty.db"
        sqlite3.connect(str(empty)).close()
        res = run_tier(self.repo, self.tree, {"MECLAW_PIPELINE_DB": str(empty)})
        self.assertNotIn(self.LANES_REFUSED, res.stderr)

    # Review L Minor 1: the exception is the CI runner (`$CI`), like the tier,
    # not the mode `ci` -- a local `gate.sh ci` with a cargo station is local
    # cargo all the same.

    # C1 finding (L Nit): `CI` still passes the lanes refusal -- a CI job runs
    # on its own machine and has no armed token file -- but a `CI=1` typed on
    # an armed host was silent. It says so now: a NOTE row in the receipt of
    # the gate, a NOTE line from the tier.
    LANES_CI_NOTE = "lanes armed on this host, cargo runs here under CI"

    # C1 finding (L M-2): "are the lanes armed" had two writers, one in the
    # runner and one in the tier, word for word. One rule with two writers is
    # not a rule (the GH #802 lesson behind `scripts/cargo-target.sh`), so the
    # probe lives there and both scripts read it. Since W3 (#1117) the runner
    # body is gate_legacy.sh; gate.sh is a shell around it.
    def test_the_lanes_probe_has_one_source(self):
        lib = (REPO / "scripts" / "cargo-target.sh").read_text()
        self.assertTrue(re.search(r"(?m)^meclaw_lanes_active\(\) \{", lib),
                        "scripts/cargo-target.sh defines no meclaw_lanes_active()")
        for script in (GATE_LEGACY, TIER_SH):
            with self.subTest(script=script.name):
                text = script.read_text()
                self.assertFalse(re.search(r"(?m)^\s*lanes_active\(\) \{", text),
                                 "%s defines its own lanes_active()" % script.name)
                self.assertFalse('.get("lanes")' in text,
                                 "%s reads the token file itself" % script.name)
                self.assertTrue(re.search(r"\bmeclaw_lanes_active\b", text),
                                "%s does not ask meclaw_lanes_active" % script.name)

    def test_the_lanes_probe_reads_the_token_file(self):
        tokens = self.repo.parent / "probe.tokens"

        def probe(state):
            if state is None:
                tokens.unlink(missing_ok=True)
            else:
                tokens.write_text(state)
            env = kit_env(self.repo, {"MECLAW_STRAND_TOKENS": str(tokens)})
            return subprocess.run(
                ["bash", "-c", '. "$1"; meclaw_lanes_active "$2"', "probe",
                 str(REPO / "scripts" / "cargo-target.sh"),
                 str(self.repo.parent / "cargo.lock")],
                env=env, capture_output=True, text=True, timeout=kit_time.HANG).returncode

        self.assertEqual(0, probe(json.dumps({"lanes": ["north", "south"]})))
        self.assertEqual(1, probe(json.dumps({"lanes": []})))
        self.assertEqual(1, probe(json.dumps({"max": 2, "holders": []})))
        self.assertEqual(1, probe("{not json"))
        self.assertEqual(1, probe("null"))
        self.assertEqual(1, probe(None))
        # Without `MECLAW_STRAND_TOKENS` the file sits next to the lock.
        (self.repo.parent / "beside.tokens").write_text(json.dumps({"lanes": ["north"]}))
        env = kit_env(self.repo)
        env.pop("MECLAW_STRAND_TOKENS")
        res = subprocess.run(
            ["bash", "-c", '. "$1"; meclaw_lanes_active "$2"', "probe",
             str(REPO / "scripts" / "cargo-target.sh"), str(self.repo.parent / "beside.lock")],
            env=env, capture_output=True, text=True, timeout=kit_time.HANG)
        self.assertEqual(0, res.returncode, res.stderr)

    def test_the_main_tree_needs_no_token(self):
        self.arm()
        res = self.gate(self.repo, "--strand", "kit")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        res = run_tier(self.repo, self.repo)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("tier-dry:", res.stdout)

# A stand-in for `scripts/pipeline.py`: it writes its argv, the caller and the
# store it was handed as one JSON line to `$STUB_RECORD`. As `gate` it opens a
# run directory under `--archive` with a `latest` pointer, as the core does,
# and prints a red run; as anything else it says one word.
PIPELINE_STUB = """import json, os, sys
argv = sys.argv[1:]
with open(os.environ["STUB_RECORD"], "a") as f:
    f.write(json.dumps({"argv": argv,
                        "caller": os.environ.get("MECLAW_LANES_CALLER"),
                        "wave_env": os.environ.get("MECLAW_GATE_WAVE"),
                        "db": os.environ.get("MECLAW_PIPELINE_DB")}) + "\\n")
if argv[:1] == ["gate"] and "--archive" in argv:
    root = argv[argv.index("--archive") + 1]
    run = os.path.join(root, "20261008T000000Z-deadbeef")
    os.makedirs(run, exist_ok=True)
    if not os.path.lexists(os.path.join(root, "latest")):
        os.symlink("20261008T000000Z-deadbeef", os.path.join(root, "latest"))
    print("gate: receipts = somewhere")
    print("GATE precheck [form] 1s GREEN")
    print("GATE tests [unit] 3s RED 1 test failed")
    print("GATE-SUMMARY strand abc1234 1/2 4s RED")
    print("GATE-LANE b1 crit=3 lane_s=4 rounds=1")
    sys.exit(1)
print("stub " + " ".join(argv))
sys.exit(int(os.environ.get("STUB_EXIT") or 0))
"""

# GH #1156: `kit_env` sets no switch any more; kept so the cases read as before.
NO_LEGACY = {"MECLAW_GATE_LEGACY": None}


class TestDelegation(TokenTestCase):
    """`strand.sh gate|test|token|lanes` delegate to the pipeline core (W3, GH #1117).

    The token semantics on the store are pinned in `test_pipeline_lanes_cli.py`;
    here: that the kit reaches the core with the right words, that the output
    and the exit are the core's, that `test-tier.sh` (which asks `strand.sh
    token check`) now asks the store, and that `MECLAW_GATE_LEGACY=1` takes
    no old path any more (GH #1156).
    """

    def tok(self, *args, cwd=None, minutes=0, env=None):
        extra = dict(NO_LEGACY)
        extra.update(env or {})
        return self.token(*args, cwd=cwd, minutes=minutes, env=extra)

    def db(self):
        return self.repo.parent / "pipeline.db"

    def store_rows(self, waiting=False):
        import sqlite3
        db = sqlite3.connect(str(self.db()))
        try:
            return [r[0] for r in db.execute(
                "SELECT holder FROM lease_entries WHERE resource IS %s NULL ORDER BY seq"
                % ("" if waiting else "NOT"))]
        finally:
            db.close()

    def stub(self):
        """The stub in both places the kit looks: next to the kit (token,
        test, lanes) and in the strand's tree (behind its `gate.sh`)."""
        for scripts in (self.repo / "scripts", self.tree / "scripts"):
            (scripts / "pipeline.py").write_text(PIPELINE_STUB)
        record = pathlib.Path(self._tmp.name) / "stub.jsonl"
        return record

    def calls(self, record):
        if not record.exists():
            return []
        return [json.loads(ln) for ln in record.read_text().splitlines()]

    def test_token_runs_on_the_store_word_for_word(self):
        res = self.tok("init", "--max", "1")
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertTrue(self.db().exists())
        self.assertFalse((self.repo.parent / "tokens").exists(), "the old token file")
        res = self.tok("take", cwd=self.tree, minutes=1)
        self.assertEqual(0, res.returncode, res.stderr)
        res = self.tok("take", "--strand", "welle-x/other", minutes=2)
        self.assertEqual(3, res.returncode, res.stderr)
        self.assertIn("welle-x/other is #1 in the queue", res.stderr)
        self.assertEqual(["welle-x/kit"], self.store_rows())
        self.assertEqual(["welle-x/other"], self.store_rows(waiting=True))
        # the kit's words and exit ARE the core's
        kit = self.tok("who", minutes=3)
        direct = subprocess.run(
            ["python3", str(self.repo / "scripts" / "pipeline.py"), "lanes", "who"],
            cwd=str(self.repo), capture_output=True, text=True,
            env=kit_env(self.repo, dict(NO_LEGACY, MECLAW_STRAND_NOW=str(self.T0 + 180))))
        self.assertEqual((direct.returncode, direct.stdout, direct.stderr),
                         (kit.returncode, kit.stdout, kit.stderr))
        self.assertIn("welle-x/kit", kit.stdout)
        res = self.tok("release", cwd=self.tree, minutes=4)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual([], self.store_rows())

    def test_a_strand_with_a_wave_directory_becomes_the_key(self):
        self.assertEqual(0, self.tok("init", "--max", "2").returncode)
        res = self.tok("take", "--strand", "a", "--wave", WAVE_DIR)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual(["%s/a" % WAVE], self.store_rows())
        res = self.tok("take", "--strand", "b", "--wave", "no-such-wave")
        self.assertEqual(2, res.returncode, res.stderr)
        self.assertEqual(["%s/a" % WAVE], self.store_rows())

    def test_a_detached_tree_takes_the_strand_from_strand(self):
        """GH #1199 (f): in a detached tree `token take --strand <S>` refused
        ("needs --wave, a strand branch ..."); the wave comes from the context
        as for `gate` (`wave_dir`), the strand from `--strand`."""
        self.assertEqual(0, self.tok("init", "--max", "2").returncode)
        _git(self.tree, "checkout", "-q", "--detach")
        res = run_strand(self.repo, "-C", str(self.tree), "token", "take", "--strand", "kit",
                         extra_env=dict(NO_LEGACY, MECLAW_STRAND_NOW=str(self.T0)))
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertEqual(["%s/kit" % WAVE], self.store_rows())

    def test_a_detached_tree_without_strand_names_the_way(self):
        """... and without `--strand` every entry that binds a strand ends with
        exit 2 and the way out ("cannot tell the strand from the branch")."""
        self.assertEqual(0, self.tok("init", "--max", "2").returncode)
        _git(self.tree, "checkout", "-q", "--detach")
        for words in (("token", "take"), ("gate",), ("report",)):
            with self.subTest(words=words):
                res = run_strand(self.repo, "-C", str(self.tree), *words, extra_env=NO_LEGACY)
                self.assertEqual(2, res.returncode, res.stdout + res.stderr)
                self.assertIn("detached HEAD: pass --strand <S>", res.stderr)
        self.assertEqual([], self.store_rows())

    def test_the_shell_chain_says_the_texts_of_the_python_chain(self):
        """One chain in two languages (GH #1199 f): `strand_of_branch` and
        `gitenv.strand_of` refuse with the same words."""
        from scripts.pipeline import gitenv
        text = STRAND_SH.read_text()
        for words in (gitenv.DETACHED, gitenv.NO_STRAND):
            self.assertIn('echo "strand: %s" >&2' % words, text)
        self.assertEqual(("kit", "welle-x/kit", ""), tuple(gitenv.strand_of("HEAD", "kit", WAVE_DIR)))
        self.assertEqual(("kit", "welle-x/kit", ""), tuple(gitenv.strand_of("welle-x/kit")))
        self.assertEqual(("a", "welle-x/a", ""), tuple(gitenv.strand_of("welle-x/kit", "a")))
        self.assertEqual(gitenv.DETACHED, gitenv.strand_of("HEAD").why)
        self.assertEqual(gitenv.NO_STRAND, gitenv.strand_of("master").why)

    def test_the_shell_chain_names_the_strand_as_the_python_chain(self):
        """Re-check f+g minor: `--strand <wave>/<S>` -- the Python chain names
        `<S>` (`gitenv.strand_of`), the shell chain passed the value on
        unchanged, so `strand.sh gate|report` archived and read under
        `receipts/<wave>/<S>` where `pipeline.py test` uses `receipts/<S>`."""
        from scripts.pipeline import gitenv
        fn = re.search(r"^strand_of_branch\(\) \{\n.*?^\}\n", STRAND_SH.read_text(),
                       re.S | re.M).group(0)
        for value in ("kit", "welle-x/kit"):
            with self.subTest(value=value):
                res = subprocess.run(["bash", "-c", fn + 'strand_of_branch "$1"', "sh", value],
                                     capture_output=True, text=True, cwd=str(self.tree))
                self.assertEqual(0, res.returncode, res.stderr)
                self.assertEqual(gitenv.strand_of("HEAD", value, WAVE_DIR).name,
                                 res.stdout.strip())

    def test_the_tier_asks_the_store_and_the_skip_variable_counts(self):
        # unarmed: nothing to ask, and nothing is created
        res = run_tier(self.repo, self.tree, extra_env=NO_LEGACY)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertFalse(self.db().exists())
        # armed, the one token held elsewhere: refused with exit 3
        self.assertEqual(0, self.tok("init", "--max", "1").returncode)
        self.assertEqual(0, self.tok("take", "--strand", "welle-x/other").returncode)
        res = run_tier(self.repo, self.tree, extra_env=NO_LEGACY)
        self.assertEqual(3, res.returncode, res.stdout + res.stderr)
        self.assertIn("no cargo token for welle-x/kit", res.stderr)
        res = run_tier(self.repo, self.tree, extra_env=dict(
            NO_LEGACY, MECLAW_STRAND_TOKEN_SKIP="one-off from the orchestrator"))
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("one-off from the orchestrator", res.stderr)

    def test_take_on_an_unarmed_store_refuses_through_the_kit(self):
        # Z11: the kit passes the refusal on -- no "not armed" with exit 0.
        res = self.tok("take", cwd=self.tree)
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        self.assertIn("strand.sh token init --lanes", res.stderr)

    def test_the_caller_and_the_store_travel_to_the_core(self):
        record = self.stub()
        env = {"STUB_RECORD": str(record), "STUB_EXIT": "3"}
        res = self.tok("check", "--pid", "123", "--lane", "b1", cwd=self.tree, env=env)
        self.assertEqual(3, res.returncode, res.stderr)
        self.assertEqual("stub lanes check --pid 123 --lane b1\n", res.stdout)
        (call,) = self.calls(record)
        self.assertEqual(["lanes", "check", "--pid", "123", "--lane", "b1"], call["argv"])
        self.assertRegex(call["caller"], r"^[0-9]+$")
        self.assertNotEqual("123", call["caller"])
        self.assertEqual(str(self.db()), call["db"])

    def test_gate_goes_to_the_core_with_strand_wave_and_archive(self):
        record = self.stub()
        res = run_strand(self.repo, "gate", "--host", "b1", "--no-pre", "--reason", "a b",
                         cwd=self.tree, extra_env=dict(NO_LEGACY, STUB_RECORD=str(record)))
        self.assertEqual(1, res.returncode, res.stderr)
        archive = self.repo / "plans" / WAVE_DIR / "receipts" / "kit"
        (call,) = self.calls(record)
        self.assertEqual(["gate", "--mode", "strand", "--strand", "kit", "--wave", WAVE,
                          "--host", "b1", "--no-pre", "--reason", "a b",
                          "--archive", str(archive)], call["argv"])
        # what the caller sees: each station line as it comes (GH #1199,
        # re-check v2-fix-e I1), then the verdict block -- the red station,
        # the summary, the lane line
        self.assertEqual(["GATE precheck [form] 1s GREEN",
                          "GATE tests [unit] 3s RED 1 test failed",
                          "GATE tests [unit] 3s RED 1 test failed",
                          "GATE-SUMMARY strand abc1234 1/2 4s RED",
                          "GATE-LANE b1 crit=3 lane_s=4 rounds=1"], res.stdout.splitlines())
        latest = archive / "latest"
        self.assertEqual("GATE-SUMMARY strand abc1234 1/2 4s RED\n",
                         (latest / "summary.txt").read_text())
        self.assertIn("gate: receipts = somewhere", (latest / "run.log").read_text())

    def test_gate_plan_only_names_the_wave_like_the_run(self):
        """re-check m-N3/m-N2: `--plan-only` passes the wave as the run does,
        and the integration narrows from the directory with its date."""
        record = self.stub()
        for extra in (("--plan-only",), ("--no-pre", "--reason", "r")):
            run_strand(self.repo, "gate", "integration", *extra, cwd=self.tree,
                       extra_env=dict(NO_LEGACY, STUB_RECORD=str(record), MECLAW_GATE_WAVE=""))
            call = self.calls(record)[-1]
            argv = call["argv"]
            self.assertIn("--wave", argv, (extra, argv))
            self.assertEqual(WAVE, argv[argv.index("--wave") + 1], extra)
            self.assertEqual(WAVE_DIR, call["wave_env"], extra)
        # a wave named by hand still wins
        run_strand(self.repo, "gate", "integration", "--plan-only", cwd=self.tree,
                   extra_env=dict(NO_LEGACY, STUB_RECORD=str(record), MECLAW_GATE_WAVE="welle-h"))
        self.assertEqual("welle-h", self.calls(record)[-1]["wave_env"])

    def test_a_pre_w3_tree_gates_on_its_own_runner(self):
        # C-M1: a tree without `gate_legacy.sh` beside its `gate.sh` is older
        # than W3 -- its `gate.sh` is the old runner, which knows no `--strand`.
        record = self.stub()
        scripts = self.tree / "scripts"
        (scripts / "gate_legacy.sh").unlink()
        (scripts / "gate.sh").write_text(
            "#!/bin/sh\n"
            "for a in \"$@\"; do [ \"$a\" = --strand ] && { echo \"unknown argument\" >&2; exit 2; }; done\n"
            "echo \"GATE-SUMMARY $1 old 1/1 0s GREEN\"\n")
        res = run_strand(self.repo, "gate", cwd=self.tree,
                         extra_env=dict(NO_LEGACY, STUB_RECORD=str(record)))
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("GATE-SUMMARY strand old 1/1 0s GREEN", res.stdout)
        # the old runner asks the store for the token (GH #1156), the core gates nothing
        self.assertEqual([], [c for c in self.calls(record) if c["argv"][:1] != ["lanes"]])

    def test_report_on_an_unarmed_store_says_nothing_about_tokens(self):
        # C-M3: the store exists (an earlier gate or lanes run) and its pool
        # is not armed -- `report` has no token to give back and stays quiet.
        self.assertEqual(0, self.tok("init", "--max", "1").returncode)
        self.assertEqual(0, self.tok("off").returncode)
        self.assertTrue(self.db().exists())
        (self.tree / "scripts" / "kit_change.txt").write_text("a change\n")
        _git(self.tree, "add", "scripts/kit_change.txt")
        _git(self.tree, "commit", "-q", "-m", "welle-x kit (#861): eine Änderung")
        self.assertEqual(0, self.gate().returncode)
        res = run_strand(self.repo, "report", cwd=self.tree, extra_env=NO_LEGACY)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertNotIn("not armed", res.stderr)

    def test_report_passes_a_held_release_on(self):
        # Re-check tv-kit fix11 M1: `report` threw the exit 3 of its `token
        # release` away and ended 0 -- the lane stayed held (a stale-but-busy
        # note, a live run) and nobody saw it. The report is written; the exit
        # says the lease is still there.
        (self.tree / "scripts" / "kit_change.txt").write_text("a change\n")
        _git(self.tree, "add", "scripts/kit_change.txt")
        _git(self.tree, "commit", "-q", "-m", "welle-x kit (#861): eine Änderung")
        self.assertEqual(0, self.gate().returncode)
        self.assertEqual(0, self.tok("init", "--max", "1").returncode)
        record = self.stub()
        res = run_strand(self.repo, "report", cwd=self.tree,
                         extra_env=dict(NO_LEGACY, STUB_RECORD=str(record), STUB_EXIT="3"))
        self.assertEqual(3, res.returncode, res.stdout + res.stderr)
        self.assertIn("header block complete, gate green", res.stdout)
        self.assertEqual([["lanes", "release", "--strand", "welle-x/kit"]],
                         [c["argv"] for c in self.calls(record)])
        self.assertIn("the lease of welle-x/kit stays", res.stderr)

    def test_gate_shows_aborted_stations(self):
        # C-M2: a station a watchdog stopped is shown like a red one.
        (self.tree / "scripts" / "gate.sh").write_text(
            "#!/bin/sh\n"
            "echo 'GATE precheck [form] 1s GREEN'\n"
            "echo 'GATE tests [unit] 3s ABORTED watchdog'\n"
            "echo 'GATE-SUMMARY strand abc1234 1/2 4s ABORTED'\n"
            "exit 2\n")
        res = run_strand(self.repo, "gate", cwd=self.tree, extra_env=NO_LEGACY)
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        # the station lines as they come (GH #1199), then the verdict block:
        # the aborted station is shown like a red one beside the summary
        self.assertEqual(["GATE precheck [form] 1s GREEN",
                          "GATE tests [unit] 3s ABORTED watchdog",
                          "GATE tests [unit] 3s ABORTED watchdog",
                          "GATE-SUMMARY strand abc1234 1/2 4s ABORTED"], res.stdout.splitlines())

    def test_gate_relays_each_station_line_to_a_stdout_only_log_while_the_pass_runs(self):
        """Re-check v2-fix-e I1(b) (GH #1199, kit find V2-A8): the runner's
        output went into a temp log only, so `strand.sh gate > log` (a wrapper
        keeping stdout) held no `GATE <station>` line until the pass ended.
        The stub blocks on a fifo after its first station: the line is in the
        stdout file while the pass still runs."""
        tmp = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, tmp, True)
        fifo, out = tmp / "go", tmp / "gate-stdout.log"
        os.mkfifo(fifo)
        (self.tree / "scripts" / "gate.sh").write_text(
            "#!/bin/sh\n"
            "echo 'GATE precheck [form] 1s GREEN'\n"
            "read go < '%s'\n"
            "echo 'GATE-SUMMARY strand abc1234 1/1 1s GREEN'\n" % fifo)
        with open(out, "w") as fh:
            proc = subprocess.Popen([str(self.repo / "scripts" / "strand.sh"), "gate"], cwd=str(self.tree),
                                    env=kit_env(self.repo, NO_LEGACY), stdout=fh,
                                    stderr=subprocess.DEVNULL)

        def opened():   # the stub's read end is open (or the pass is over)
            try:
                return os.open(fifo, os.O_WRONLY | os.O_NONBLOCK)
            except OSError:
                return -1 if proc.poll() is not None else None

        def release():
            if proc.poll() is None:
                fd = kit_time.until("the gate stub opens its fifo", opened)
                if fd >= 0:
                    os.write(fd, b"go\n")
                    os.close(fd)
            try:
                proc.wait(timeout=kit_time.HANG)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()
        self.addCleanup(release)
        kit_time.until("the station line in the stdout log while the gate blocks",
                       lambda: "GATE precheck [form] 1s GREEN" in out.read_text(),
                       state=out.read_text)
        self.assertIsNone(proc.poll())            # the pass still runs
        release()
        self.assertEqual(0, proc.returncode, out.read_text())
        self.assertEqual(["GATE precheck [form] 1s GREEN", "GATE-SUMMARY strand abc1234 1/1 1s GREEN"],
                         out.read_text().splitlines())

    def test_one_log_for_both_streams_holds_each_station_line_once(self):
        (self.tree / "scripts" / "gate.sh").write_text(
            "#!/bin/sh\n"
            "echo 'GATE precheck [form] 1s GREEN'\n"
            "echo 'GATE-SUMMARY strand abc1234 1/1 1s GREEN'\n")
        res = subprocess.run([str(self.repo / "scripts" / "strand.sh"), "gate"], cwd=str(self.tree),
                             env=kit_env(self.repo, NO_LEGACY), stdout=subprocess.PIPE,
                             stderr=subprocess.STDOUT, text=True, timeout=4 * kit_time.HANG)
        self.assertEqual(0, res.returncode, res.stdout)
        self.assertEqual(1, res.stdout.count("GATE precheck [form] 1s GREEN"), res.stdout)

    def test_a_gate_refused_before_its_pass_says_so_on_both_streams(self):
        """Re-check v2-fix-e I1(b): a run without GATE-SUMMARY (a refusal of
        the runner) went to the caller as `tail ... >&2` only -- the REFUSED
        line never reached a stdout log. Now on both streams, once in one log."""
        refusal = "gate: REFUSED (exit 3) -- gate: queued -- no lane free"
        (self.tree / "scripts" / "gate.sh").write_text(
            "#!/bin/sh\n"
            "echo 'gate: queued -- no lane free' >&2\n"
            "echo '%s'\n"
            "exit 3\n" % refusal)
        res = run_strand(self.repo, "gate", cwd=self.tree, extra_env=NO_LEGACY)
        self.assertEqual(3, res.returncode, res.stdout + res.stderr)
        for text in (res.stdout, res.stderr):
            self.assertIn("strand: the gate wrote no summary line", text)
            self.assertIn(refusal, text)
        one = subprocess.run([str(self.repo / "scripts" / "strand.sh"), "gate"], cwd=str(self.tree),
                             env=kit_env(self.repo, NO_LEGACY), stdout=subprocess.PIPE,
                             stderr=subprocess.STDOUT, text=True, timeout=4 * kit_time.HANG)
        self.assertEqual(3, one.returncode, one.stdout)
        self.assertEqual(1, one.stdout.count(refusal), one.stdout)

    def test_gate_plan_only_and_ci_and_legacy(self):
        record = self.stub()
        env = dict(NO_LEGACY, STUB_RECORD=str(record))
        res = run_strand(self.repo, "gate", "integration", "--plan-only", "--base", "x",
                         cwd=self.tree, extra_env=env)
        self.assertEqual(0, res.returncode, res.stderr)
        # the plan names the wave as the run does (re-check m-N3)
        self.assertEqual(["gate", "--mode", "integration", "--wave", WAVE, "--plan-only",
                          "--base", "x"], self.calls(record)[-1]["argv"])
        # `ci` never reaches the core
        n = len(self.calls(record))
        res = run_strand(self.repo, "gate", "ci", "--plan-only", cwd=self.tree,
                         extra_env=dict(env, MECLAW_GATE_PLAN=str(self.plan_file())))
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual(n, len(self.calls(record)))
        # ... and the old switch no longer keeps a strand gate away from it (GH #1156)
        res = run_strand(self.repo, "gate", "--plan-only", cwd=self.tree,
                         extra_env=dict(env, MECLAW_GATE_LEGACY="1"))
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual(["gate", "--mode", "strand", "--wave", WAVE, "--plan-only"],
                         self.calls(record)[-1]["argv"])

    def test_test_goes_to_the_core_with_every_argument(self):
        record = self.stub()
        args = ["binary(~gh123)", "--host", "b1", "-p", "meclaw-cells", "--", "--nocapture"]
        res = run_strand(self.repo, "test", *args, cwd=self.tree,
                         extra_env=dict(NO_LEGACY, STUB_RECORD=str(record), STUB_EXIT="6"))
        self.assertEqual(6, res.returncode, res.stderr)
        self.assertEqual(["test"] + args, self.calls(record)[-1]["argv"])


    def test_test_in_a_detached_tree_names_the_wave_of_strand(self):
        """GH #1199 (f): `test --strand <name>` in a detached tree reached the
        core without a wave, and the core refused ("cannot tell the wave from
        the branch"); the kit names it from the context, in front of every
        word as given (nextest's own after `--` stay last)."""
        record = self.stub()
        _git(self.tree, "checkout", "-q", "--detach")
        args = ["binary(~gh123)", "--strand", "kit", "--host", "b1", "--", "--nocapture"]
        res = run_strand(self.repo, "-C", str(self.tree), "test", *args,
                         extra_env=dict(NO_LEGACY, STUB_RECORD=str(record)))
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual(["test", "--wave", WAVE_DIR] + args, self.calls(record)[-1]["argv"])

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
        # re-check ZC: OR-DT.store-migrations, the hash runs over the raw text
        "Store-Migrationen nur additiv": ("Migrationen nur additiv", "Rohtext", "kein DROP/RENAME"),
        # Review I1 of Z-D: the chain writes its own PID, the probe proves it.
        "Warte-Kette schreibt ihre PID selbst": ("echo $$ >", "ps -o sid= -p"),
    }

    def setUp(self):
        if not self.PREAMBLE.is_file():
            self.skipTest("plans/ is not part of the public tree")
        self.text = self.PREAMBLE.read_text()

    def test_the_pass_close_frees_only_an_admission_lease(self):
        # Re-check Z-A M-c: on the standard way (`token take`, then gate) the
        # close of the pass only clears the run; a second `release` frees it.
        self.assertNotIn("der Close des Passes gibt sie frei,", self.text)
        self.assertIn("gibt ein erneutes `release` sie frei", self.text)

    def test_every_machine_rule_stands_in_the_canonical_file(self):
        for rule, phrases in self.RULES.items():
            for phrase in phrases:
                self.assertTrue(phrase in self.text,
                                "%s: the rule %r is gone -- no line says %r"
                                % (self.PREAMBLE.name, rule, phrase))

    def test_the_chain_pid_rule_does_not_point_at_the_setsid_child(self):
        """Without job control `setsid bash -c '...' &` does not fork, `$!`
        is already the chain and its child is the first link; with it `$!`
        is the dead starter. "Report the child of setsid" is right in one
        shell and wrong in the other (review Z-D I1)."""
        self.assertNotIn("das Kind des setsid", self.text)

    def test_the_chain_pid_recipe_never_reads_a_stale_pidfile(self):
        """`... &` and then `cat <pidfile>` finds it empty, missing, or with the
        PID of an earlier run (re-check Z-D 714a5f0ba: old PID in 8-14 of 20
        runs), and `tail --pid` returns at once. The recipe removes the file
        first, reads it in the NEXT command, and reads again when it is empty.
        A failed probe is no reason to read again (re-check Z-D f2d534b65):
        the PID in the file is the chain's, the chain has ended or never ran,
        so the log says why."""
        rm = self.text.find("rm -f <pidfile>")
        start = self.text.find("setsid bash -c 'echo $$ > <pidfile>;")
        self.assertTrue(0 <= rm < start, "the pidfile is not removed before the chain starts")
        self.assertIn("im NÄCHSTEN Kommando", self.text)
        self.assertIn("pidfile leer → erneut lesen", self.text)
        self.assertIn("Probe falsch → Log lesen", self.text)
        self.assertNotIn("oder Probe falsch → erneut lesen", self.text)
        self.assertNotIn("2>&1 &`, dann `cat <pidfile>`", self.text)

    def test_the_integration_pass_has_one_entry_that_names_its_strand(self):
        """On master `strand.sh gate integration` without `--strand` dies
        ("cannot tell the strand from the branch -- pass --strand"), and
        `gate.sh integration` is the shell behind the one entry, not the entry
        (re-check Z-D 714a5f0ba M2). Every rule file says the same command."""
        for rel in ("AGENTS.md", "plans/PREAMBLE.md", "docs/development-rules.md"):
            text = " ".join((REPO / rel).read_text(encoding="utf-8").split())
            self.assertNotIn("gate.sh integration", text, rel)
            for m in re.finditer(r"strand\.sh gate integration", text):
                self.assertTrue(text[m.end():m.end() + 40].lstrip(" `").startswith("--strand"),
                                "%s: %r has no --strand" % (rel, text[m.start():m.end() + 40]))


class TestPublicHygiene(unittest.TestCase):
    """The kit travels in the public export (`ROOT_FILES`), tests included.

    Two rules of `plans/PREAMBLE.md` section 1 apply to every line of it, and
    neither is covered by an export audit: R5 knows name and domain patterns
    but no address pattern, and staging a whole tree at once is forbidden by
    `docs/development-rules.md` section 6, which nothing checks yet.
    """

    FILES = (STRAND_SH, LANE_SYNC,
             REPO / "scripts" / "pipeline" / "hygiene.py",
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


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


if __name__ == "__main__":
    unittest.main()
