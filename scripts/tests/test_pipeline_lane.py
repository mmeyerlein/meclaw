"""Tests for the lane transport `scripts/pipeline/lane.py` (W3, OR-DT-27).

`lane.py` is the Python successor of `scripts/lane_sync.sh` with the same
semantics: the commit travels by `git push` into a bare repository on the
lane, the uncommitted rest (against HEAD) by an overlay, never a file whose
name says secret; a tracked secret refuses the push before any contact, and
both sides hash the overlay. New beside it: one build path `tree/` per lane
for runs with a cargo station (`wt/<tree>` for the rest), the transport
`local` (no ssh), the executor stream and the host file reader.

Every case runs against a throw-away repository and a fake `ssh` on PATH
that runs the remote command HERE, against a directory of this test -- the
pattern of `TestGateHost` in `test_strand_sh.py`. No case reaches a lane.
"""

import contextlib
import dataclasses
import json
import os
import pathlib
import re
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import textwrap
import time
import unittest
from unittest import mock

from scripts.pipeline import lane
from scripts.tests import fake_ssh, kit_time, store_isolation

REPO = pathlib.Path(__file__).resolve().parents[2]
GATE_PLAN = REPO / "scripts" / "gate_plan.py"
LANE_SYNC = REPO / "scripts" / "lane_sync.sh"

# The egon widths a thread pin runs through, never the host's own nproc: the
# pin of 915b26b35 took this machine's count, was green on mm-os01 (8) and red
# on build02 (12) -- tb-s4 gate bcc315e83.
EGON_NPROC = (4, 6, 8, 12, 24)


def host_nproc(n):
    """This process's CPU count patched to `n` (`cpus.threads` under both import
    names of the kit, and the affinity it reads), so no fallback to the real
    host can hide in a pin (GH #1199: the kit counts CPUs in `pipeline.cpus`)."""
    import scripts.pipeline.cpus  # present under its package name
    stack = contextlib.ExitStack()
    for name in ("scripts.pipeline.cpus", "pipeline.cpus"):
        if name in sys.modules:
            stack.enter_context(mock.patch.object(sys.modules[name], "threads", return_value=n))
    stack.enter_context(mock.patch("os.sched_getaffinity", return_value=set(range(n))))
    return stack


GIT_ENV = {
    "GIT_AUTHOR_NAME": "lane test",
    "GIT_AUTHOR_EMAIL": "lane@example.invalid",
    "GIT_COMMITTER_NAME": "lane test",
    "GIT_COMMITTER_EMAIL": "lane@example.invalid",
    "GIT_CONFIG_GLOBAL": os.devnull,
    "GIT_CONFIG_SYSTEM": os.devnull,
}

# A fake `ssh`: drops the options, logs the call and runs the remote command
# HERE. A host whose name says `unreachable` answers like a dead one. A real
# ssh does not carry the caller's environment: the golden switches and a
# secret beside them reach the host only on the command line.
FAKE_SSH_TEMPLATE = """#!/usr/bin/env bash
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
# ssh carries none of the caller's environment: what is not a word of the
# command line never arrives -- the names of the one rule (fake_ssh.py: the
# golden switches, lane_sync.sh VERDICT_ENV, the browser brake, a key).
# @fake-ssh: strip the transported names@
exec bash -c "$*"
"""
FAKE_SSH = fake_ssh.script(FAKE_SSH_TEMPLATE)

# A fake `rsync` beside it: the real one, logged, unless the test asks it to
# drop the overlay (FAKE_RSYNC_DROP) or to fail on a path (FAKE_RSYNC_FAIL).
FAKE_RSYNC = """#!/usr/bin/env bash
printf '%%s\\n' "$*" >>"$FAKE_RSYNC_LOG"
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

# A fake `systemd-run`: logs its words, drops its options, runs the rest.
FAKE_SYSTEMD_RUN = """#!/usr/bin/env bash
printf '%s\\n' "$*" >>"$FAKE_SYSTEMD_LOG"
while [ $# -gt 0 ]; do
    case "$1" in
        -*) shift ;;
        *) break ;;
    esac
done
exec "$@"
"""

# The executor of the fixture tree: echoes what it got as two events, and
# answers `hygiene status` with one JSON line (W2's form).
FAKE_EXECUTOR = """import json, os, sys
if sys.argv[1:3] == ["hygiene", "status"]:
    print(json.dumps({"runner": sys.argv[-1], "state": "idle", "free_g": 373.0,
                      "trees": 1, "ws": 0, "zombies": 0, "generations": 2}))
    sys.exit(0)
d = json.load(sys.stdin)
print(json.dumps({"event": "hygiene_checked", "kind": "status", "run": d["run"],
                  "data": {"argv": sys.argv[1:], "cwd": os.getcwd()}}), flush=True)
print(json.dumps({"event": "pass_ended", "kind": "status", "run": d["run"],
                  "data": {"verdict": "GREEN"}}), flush=True)
"""

RUN = "run-0123456789ab"


def git(repo, *args):
    return subprocess.run(["git", "-C", str(repo)] + list(args), check=True,
                          capture_output=True, text=True).stdout


# A locale that sorts mixed case apart from byte order (re-check tv-bn M7r B-M1).
FOREIGN_LOCALE = "en_US.UTF-8"


def shell_digest(repo):
    """`lane_sync.sh digest` in `repo` under FOREIGN_LOCALE, no LC_ALL=C of the caller."""
    env = {k: v for k, v in os.environ.items() if not k.startswith("LC_") and k != "LANG"}
    env.update(LANG=FOREIGN_LOCALE, LC_ALL=FOREIGN_LOCALE)
    res = subprocess.run(["bash", str(LANE_SYNC), "digest"], cwd=str(repo), env=env,
                         capture_output=True, text=True, timeout=kit_time.HANG)
    if res.returncode != 0:
        raise AssertionError(res.stderr)
    return res.stdout.strip()


class LaneTestCase(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        tmp = self.tmp = pathlib.Path(self._tmp.name)
        self.bin = tmp / "bin"
        self.bin.mkdir()
        fake_ssh.install(self.bin, FAKE_SSH_TEMPLATE)
        self.systemd_log = tmp / "systemd.log"
        # the host's `systemd-run` runs under the narrow point's `env -i`
        # (orchestrator OR-T10-2): its log path is part of its body, no name
        # of this side reaches it
        systemd_run = FAKE_SYSTEMD_RUN.replace('"$FAKE_SYSTEMD_LOG"', shlex.quote(str(self.systemd_log)))
        for name, text in (("rsync", FAKE_RSYNC % shutil.which("rsync")), ("systemd-run", systemd_run)):
            (self.bin / name).write_text(text)
            (self.bin / name).chmod(0o755)
        store_isolation.write_lane_fakes(self.bin)
        self.remote = tmp / "remote"
        self.local_root = tmp / "local-root"
        self.ssh_log = tmp / "ssh.log"
        self.rsync_log = tmp / "rsync.log"
        for log in (self.ssh_log, self.rsync_log, self.systemd_log):
            log.write_text("")
        self.lanes_file = tmp / "lanes"
        self.lanes_file.write_text(
            "# the lanes of this test\n"
            "lane-a  builder@lane-a.invalid  bmc-a  threads=8\n"
            "lane-dead  builder@unreachable.invalid  class=lane\n"
            "lane-l  local  %s  class=egon threads=4 jobs=6\n" % self.local_root)
        env = dict(GIT_ENV)
        env.update({"PATH": "%s:%s" % (self.bin, os.environ["PATH"]),
                    "MECLAW_LANE_ROOT": str(self.remote),
                    "MECLAW_LANES_FILE": str(self.lanes_file),
                    "FAKE_SSH_LOG": str(self.ssh_log),
                    "FAKE_RSYNC_LOG": str(self.rsync_log),
                    "FAKE_SYSTEMD_LOG": str(self.systemd_log)})
        patcher = mock.patch.dict(os.environ, env)
        patcher.start()
        self.addCleanup(patcher.stop)
        # the narrow point checks the local rsync of a remote way like ssh (OR-T10-25)
        fake_ssh.register_tool(self.bin / "rsync")
        for var in ("GH277_WRITE_GOLDEN", "MECLAW_GH935_BLESS", "MECLAW_PIPELINE_NO_SCOPE",
                    "FAKE_RSYNC_DROP", "FAKE_RSYNC_FAIL", "MECLAW_LANE_FLOOR_G", "MECLAW_LANE_WIPE_OK",
                    "MECLAW_LANE_EGON_GATE_OK", "MECLAW_LANE_SLOT"):
            os.environ.pop(var, None)
        self.repo = self.make_repo(tmp / "repo")
        self.a = lane.find_lane("lane-a")
        self.local = lane.find_lane("lane-l")

    def make_repo(self, repo):
        (repo / "scripts" / "pipeline").mkdir(parents=True)
        shutil.copy(GATE_PLAN, repo / "scripts" / "gate_plan.py")
        # gate_plan.py reads Rust through `pipeline.crate_root` (GH #1162)
        for f in ("__init__.py", "crate_root.py"):
            shutil.copy(REPO / "scripts" / "pipeline" / f, repo / "scripts" / "pipeline" / f)
        (repo / "scripts" / "pipeline.py").write_text(FAKE_EXECUTOR)
        (repo / "README.md").write_text("first\n")
        (repo / ".gitignore").write_text("ignored.txt\n")
        (repo / "gone.txt").write_text("to be deleted\n")
        (repo / "scripts" / "kit_change.txt").write_text("committed\n")
        git(repo.parent, "init", "-q", str(repo))
        git(repo, "add", ".")
        git(repo, "commit", "-q", "-m", "first")
        git(repo, "branch", "-M", "master")
        # The working state: a change, a new file, a deletion and the files
        # that must never leave this machine.
        (repo / "README.md").write_text("changed, not committed\n")
        (repo / "notes").mkdir()
        (repo / "notes" / "new.txt").write_text("untracked\n")
        (repo / "gone.txt").unlink()
        (repo / ".env").write_text("NEVER=sent\n")
        (repo / ".env.local").write_text("NEVER=sent\n")
        (repo / "vault.env").write_text("NEVER=sent\n")
        (repo / "notes" / "vault-pass.txt").write_text("never sent\n")
        (repo / "ignored.txt").write_text("never sent\n")
        return repo

    def commit(self, files):
        for rel, text in files.items():
            path = self.repo / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
        git(self.repo, "add", "-f", *files)
        git(self.repo, "commit", "-q", "-m", "more")

    def push(self, cargo=False, name="kit", target=None):
        return lane.push(target or self.a, str(self.repo), name, cargo)

    def wt(self, name="kit"):
        return self.remote / "wt" / name

    def lane_env_text(self):
        return subprocess.run(["python3", str(GATE_PLAN), "--print", "lane-env"],
                              capture_output=True, text=True, check=True).stdout


class TestLanePush(LaneTestCase):
    """The push: same semantics as `lane_sync.sh push`."""

    def test_lane_push_refuses_tracked_secret(self):
        # Staged, changed-tracked and committed: each leaves by one road (the
        # overlay or the push) -- refused with the path, before any contact.
        cases = {
            "config/.env.prod": lambda: (
                (self.repo / "config").mkdir(),
                (self.repo / "config" / ".env.prod").write_text("NEVER=sent\n"),
                git(self.repo, "add", "-f", "config/.env.prod")),
            "deploy/vault-pass.txt": lambda: (
                self.commit({"deploy/vault-pass.txt": "placeholder\n"}),
                (self.repo / "deploy" / "vault-pass.txt").write_text("NEVER=sent\n")),
            ".cargo/credentials.toml": lambda: self.commit(
                {".cargo/credentials.toml": "NEVER=sent\n"}),
        }
        for path, arrange in cases.items():
            with self.subTest(path=path):
                shutil.rmtree(self.repo)
                self.repo = self.make_repo(self.tmp / "repo")
                arrange()
                with self.assertRaises(lane.LaneError) as cm:
                    self.push()
                self.assertEqual(5, cm.exception.code)
                self.assertIn(path, str(cm.exception))
                self.assertEqual("", self.ssh_log.read_text())
                self.assertEqual("", self.rsync_log.read_text())
                self.assertFalse(self.remote.exists())

    def test_lane_overlay_hash_both_sides(self):
        local = lane.digest(str(self.repo))
        self.assertRegex(local, r"^[0-9a-f]{16}$")
        res = self.push()
        self.assertEqual(local, res["overlay_hash"])
        self.assertEqual(git(self.repo, "rev-parse", "HEAD").strip(), res["rev"])
        # ... byte-compatible with the legacy transport, under any locale (re-check
        # tv-bn M7r B-M1: the pin set LC_ALL=C itself, and under en_US the shell
        # sorted the list another way)
        self.assertEqual(local, shell_digest(self.repo))

    def test_lane_env_runs_in_the_kits_isolated_python_on_the_host(self):
        """Re-check tv-bn M7r B-M2 (OR-T11-22): the host wrote its `.env` with a bare
        `python3` from PATH, a second form of the kit's one Python (lane.PYTHON)."""
        self.push()
        sent = [ln for ln in self.ssh_log.read_text().splitlines() if "gate_plan.py" in ln]
        self.assertTrue(sent)
        for line in sent:
            self.assertIn(" ".join(lane.PYTHON) + " scripts/gate_plan.py --print lane-env", line)
            self.assertNotRegex(line, r"(?<![\w/.-])python3 scripts/gate_plan")

    def test_lane_overlay_digest_is_one_under_a_foreign_locale_and_with_a_link(self):
        """Re-check tv-bn M7r B-M1 (OR-T11-27): mixed case sorts apart in en_US and
        in byte order, and the link branch of DIGEST is in the list."""
        locale_sorted = subprocess.run(["sort"], input="a\nB\n", capture_output=True, text=True,
                                       env=dict(os.environ, LC_ALL=FOREIGN_LOCALE)).stdout
        if locale_sorted != "a\nB\n":
            self.skipTest("no %s on this host: it sorts like C" % FOREIGN_LOCALE)
        (self.repo / "Upper.txt").write_text("U\n")
        (self.repo / "lower.txt").write_text("l\n")
        os.symlink("README.md", str(self.repo / "a-link"))
        local = lane.digest(str(self.repo))
        self.assertEqual(local, shell_digest(self.repo))
        # the link branch counts: another target, another digest on both sides
        os.unlink(str(self.repo / "a-link"))
        os.symlink("lower.txt", str(self.repo / "a-link"))
        other = lane.digest(str(self.repo))
        self.assertNotEqual(local, other)
        self.assertEqual(other, shell_digest(self.repo))

    def test_lane_host_tree_that_differs_is_refused(self):
        os.environ["FAKE_RSYNC_DROP"] = "1"
        with self.assertRaises(lane.LaneError) as cm:
            self.push()
        self.assertEqual(5, cm.exception.code)
        self.assertIn("overlay", str(cm.exception))

    def test_lane_host_tree_is_the_local_tree_without_the_secrets(self):
        self.push()
        wt = self.wt()
        self.assertEqual("committed\n", (wt / "scripts" / "kit_change.txt").read_text())
        self.assertEqual("changed, not committed\n", (wt / "README.md").read_text())
        self.assertEqual("untracked\n", (wt / "notes" / "new.txt").read_text())
        self.assertFalse((wt / "gone.txt").exists())
        for never in (".env.local", "vault.env", "notes/vault-pass.txt", "ignored.txt"):
            with self.subTest(path=never):
                self.assertFalse((wt / never).exists())
        self.assertNotIn("NEVER=sent", self.ssh_log.read_text())
        # The `.env` there is the secret-free lane env, never this tree's.
        self.assertEqual(self.lane_env_text(), (wt / ".env").read_text())
        self.assertNotIn("NEVER", (wt / ".env").read_text())

    def test_lane_second_push_takes_the_new_state(self):
        self.push()
        (self.repo / "notes" / "new.txt").unlink()
        (self.repo / "README.md").write_text("second state\n")
        self.push()
        self.assertFalse((self.wt() / "notes" / "new.txt").exists())
        self.assertEqual("second state\n", (self.wt() / "README.md").read_text())

    def test_lane_staged_changes_additions_deletions_and_renames_travel(self):
        self.commit({"src/a.txt": "old a\n", "src/b.txt": "b\n", "src/c.txt": "c\n"})
        (self.repo / "src" / "a.txt").write_text("staged a\n")
        (self.repo / "src" / "n.txt").write_text("staged new\n")
        git(self.repo, "add", "src/a.txt", "src/n.txt")
        git(self.repo, "rm", "-q", "src/b.txt")
        git(self.repo, "mv", "src/c.txt", "src/moved.txt")
        self.push()
        wt = self.wt()
        self.assertEqual("staged a\n", (wt / "src" / "a.txt").read_text())
        self.assertEqual("staged new\n", (wt / "src" / "n.txt").read_text())
        self.assertFalse((wt / "src" / "b.txt").exists())
        self.assertFalse((wt / "src" / "c.txt").exists())
        self.assertEqual("c\n", (wt / "src" / "moved.txt").read_text())

    def test_lane_tracked_files_under_vault_and_env_paths_travel(self):
        self.commit({"crates/x/src/vault/mod.rs": "old\n", "templates/vault/a.json": "{}\n",
                     "examples/vault-pilot/b.txt": "old\n", ".env.example": "KEY=\n"})
        (self.repo / "crates/x/src/vault/mod.rs").write_text("new\n")
        (self.repo / "examples/vault-pilot/b.txt").write_text("new\n")
        (self.repo / "examples/vault-pilot/c.txt").write_text("untracked\n")
        (self.repo / ".env.example").write_text("KEY=\nOTHER=\n")
        git(self.repo, "rm", "-q", "templates/vault/a.json")
        self.push()
        wt = self.wt()
        self.assertEqual("new\n", (wt / "crates/x/src/vault/mod.rs").read_text())
        self.assertEqual("new\n", (wt / "examples/vault-pilot/b.txt").read_text())
        self.assertEqual("untracked\n", (wt / "examples/vault-pilot/c.txt").read_text())
        self.assertEqual("KEY=\nOTHER=\n", (wt / ".env.example").read_text())
        self.assertFalse((wt / "templates/vault/a.json").exists())

    def test_lane_exclusion_list_is_pinned(self):
        # an untracked, unignored provider.env is a secret in both rules (delta M2)
        (self.repo / "notes" / "provider.env").write_text("KEY=x\n")
        self.assertEqual(["README.md", "notes/new.txt"], sorted(lane.overlay_files(str(self.repo))))
        legacy = subprocess.run(["bash", str(LANE_SYNC), "files"], cwd=str(self.repo),
                                capture_output=True, text=True)
        self.assertEqual(sorted(legacy.stdout.split("\0")[:-1]),
                         sorted(lane.overlay_files(str(self.repo))))
        for name, secret in ((".env", True), ("a/.env.prod", True), (".env.example", False),
                             ("vault.env", True), ("x/vault-pass.txt", True), (".netrc", True),
                             (".git-credentials", True), (".cargo/credentials", True),
                             ("a/.cargo/credentials.toml", True), ("src/vault/mod.rs", False),
                             ("templates/.envoy/x", False), ("x/provider.env", True)):
            with self.subTest(name=name):
                self.assertEqual(secret, lane.secret_name(name))

    def test_lane_pushes_the_export_ref_and_master(self):
        res = self.push()
        bare = self.remote / "meclaw.git"
        self.assertEqual(git(self.repo, "rev-parse", "master").strip(),
                         git(bare, "rev-parse", "refs/heads/master").strip())
        self.assertEqual(res["rev"], git(bare, "rev-parse", "refs/heads/lane/kit").strip())
        # without github-main the push still went; with it, the ref travels
        git(self.repo, "branch", "github-main", "master")
        self.push()
        here = git(self.repo, "rev-parse", "github-main").strip()
        self.assertEqual(here, git(bare, "rev-parse", "github-main").strip())
        self.assertEqual(here, git(self.wt(), "rev-parse", "github-main").strip())

    def test_lane_origin_only_plain_https(self):
        for url, want in (("https://user:tok3n@git.example.invalid/x.git", ""),
                          ("git@git.example.invalid:x/y.git", ""),
                          ("https://git.example.invalid/x/y.git",
                           "https://git.example.invalid/x/y.git")):
            with self.subTest(url=url):
                subprocess.run(["git", "-C", str(self.repo), "remote", "remove", "origin"],
                               capture_output=True)
                shutil.rmtree(self.remote, ignore_errors=True)
                self.ssh_log.write_text("")
                git(self.repo, "remote", "add", "origin", url)
                self.push()
                got = subprocess.run(["git", "--git-dir", str(self.remote / "meclaw.git"),
                                      "config", "--get", "remote.origin.url"],
                                     capture_output=True, text=True).stdout.strip()
                self.assertEqual(want, got)
                if not want:
                    self.assertNotIn("git.example.invalid", self.ssh_log.read_text())

    def test_lane_changed_file_is_never_older_than_the_lane_build(self):
        # GH #1071: a file edited while the lane compiled the previous sync
        # must not arrive older than that build.
        readme = self.repo / "README.md"
        old = time.time() - 1000
        os.utime(readme, (old, old))
        self.push()
        built = time.time()
        readme.write_text("edited during the lane build\n")
        os.utime(readme, (old + 10, old + 10))
        self.push()
        there = self.wt() / "README.md"
        self.assertEqual("edited during the lane build\n", there.read_text())
        self.assertGreaterEqual(there.stat().st_mtime, built - 1)

    def test_lane_unchanged_overlay_file_keeps_its_time(self):
        readme = self.repo / "README.md"
        old = time.time() - 1000
        os.utime(readme, (old, old))
        self.push()
        there = self.wt() / "README.md"
        first = there.stat().st_mtime
        self.push()
        self.assertLessEqual(there.stat().st_mtime, first)
        self.assertAlmostEqual(readme.stat().st_mtime, there.stat().st_mtime, delta=1)

    def test_lane_unreachable_host_is_code_2(self):
        with self.assertRaises(lane.LaneError) as cm:
            self.push(target=lane.find_lane("lane-dead"))
        self.assertEqual(2, cm.exception.code)
        self.assertIn("did not answer", str(cm.exception))


class TestLaneTreeOrigin(LaneTestCase):
    """GH #1199 (OR-TM-14): a lane tree belongs to one repository. coding's `pass2`
    overwrote orga's `pass2` on build04 -- the run data of orga were gone."""

    def two_repositories(self):
        orga = self.make_repo(self.tmp / "orga" / "pass2")
        coding = self.make_repo(self.tmp / "coding" / "pass2")
        # one history each: the commits of one are unknown to the other (the fixture's
        # first commits are byte-equal); the overlay README stays each one's
        for repo in (orga, coding):
            (repo / "own.txt").write_text(repo.parent.name + "\n")
            git(repo, "add", "own.txt")
            git(repo, "commit", "-q", "-m", repo.parent.name)
        return orga, coding

    def test_two_repositories_with_one_worktree_name_get_two_lane_trees(self):
        from scripts.pipeline import schedule
        orga, coding = self.two_repositories()
        names = [schedule.lane_tree_name(r, str(p)) for r, p in (("orga", orga), ("coding", coding))]
        self.assertEqual(["orga@pass2", "coding@pass2"], names)
        # the core keeps its path (the plain worktree name)
        self.assertEqual("meclaw-core-wt-s", schedule.lane_tree_name("meclaw", "/x/meclaw-core-wt-s/"))
        self.assertEqual("meclaw-core-wt-s", schedule.lane_tree_name(None, "/x/meclaw-core-wt-s"))
        a = lane.push(self.a, str(orga), names[0], False, origin="orga")
        (pathlib.Path(a["tree"]) / "ignored.txt").write_text("orga run data\n")
        b = lane.push(self.a, str(coding), names[1], False, origin="coding")
        self.assertNotEqual(a["tree"], b["tree"])
        self.assertEqual(git(orga, "rev-parse", "HEAD").strip(), git(pathlib.Path(a["tree"]), "rev-parse", "HEAD").strip())
        self.assertEqual("orga run data\n", (pathlib.Path(a["tree"]) / "ignored.txt").read_text())

    def test_a_foreign_repositorys_tree_under_the_name_is_refused_and_kept(self):
        orga, coding = self.two_repositories()
        a = lane.push(self.a, str(orga), "pass2", False, origin="orga")
        tree = pathlib.Path(a["tree"])
        (tree / "ignored.txt").write_text("orga run data\n")
        orga_rev = git(orga, "rev-parse", "HEAD").strip()
        bare = self.remote / "meclaw.git"
        self.assertIn("receive-pack", self.ssh_log.read_text())  # the log sees a push
        # the same repository again: the tree is its own
        lane.push(self.a, str(orga), "pass2", False, origin="orga")
        # the origin file says orga; a tree from before it (no file): coding knows
        # none of orga's commits -- both refused before anything travels
        origin_file = pathlib.Path(git(tree, "rev-parse", "--absolute-git-dir").strip()) / lane.ORIGIN_FILE
        self.assertEqual("orga\n", origin_file.read_text())
        for case in ("origin file", "no origin file"):
            with self.subTest(case=case):
                if case == "no origin file":
                    origin_file.unlink()
                self.ssh_log.write_text("")
                with self.assertRaisesRegex(lane.LaneError, "never overwritten by another repository") as cm:
                    lane.push(self.a, str(coding), "pass2", False, origin="coding")
                self.assertEqual(5, cm.exception.code)
                self.assertIn("orga" if case == "origin file" else orga_rev[:12], str(cm.exception))
                if case == "no origin file":
                    # re-check M4: the way out of a tree whose commit this repository
                    # dropped -- its origin once by hand, never a deletion
                    self.assertIn('echo coding >"$(git -C %s rev-parse --absolute-git-dir)/%s"'
                                  % (tree, lane.ORIGIN_FILE), str(cm.exception))
                    self.assertIn("hand step", str(cm.exception))
                self.assertEqual(orga_rev, git(tree, "rev-parse", "HEAD").strip())
                self.assertEqual(orga_rev, git(bare, "rev-parse", "refs/heads/lane/pass2").strip())
                self.assertEqual("orga run data\n", (tree / "ignored.txt").read_text())
                self.assertEqual("changed, not committed\n", (tree / "README.md").read_text())
                self.assertNotIn("git-receive-pack", self.ssh_log.read_text())
        # a tree from before the file whose commit this repository knows: its own
        lane.push(self.a, str(orga), "pass2", False, origin="orga")
        self.assertEqual("orga\n", origin_file.read_text())


    def test_a_directory_git_cannot_read_is_refused_and_kept(self):
        # re-check M1: only a path that does not exist is "no tree"; a directory git
        # cannot read (no .git, a .git that points nowhere) never reaches the
        # checkout's `rm -rf`
        orga, _ = self.two_repositories()
        for case in ("plain directory", "dangling .git"):
            with self.subTest(case=case):
                tree = self.remote / "wt" / case.replace(" ", "-")
                tree.mkdir(parents=True)
                (tree / "run.txt").write_text("run data\n")
                if case == "dangling .git":
                    (tree / ".git").write_text("gitdir: %s\n" % (self.tmp / "gone"))
                self.ssh_log.write_text("")
                with self.assertRaisesRegex(lane.LaneError, "git cannot read") as cm:
                    lane.push(self.a, str(orga), tree.name, False, origin="orga")
                self.assertEqual(5, cm.exception.code)
                self.assertEqual("run data\n", (tree / "run.txt").read_text())
                self.assertNotIn("git-receive-pack", self.ssh_log.read_text())

    def test_every_unexpected_probe_answer_is_refused(self):
        # re-check M1: fail closed -- `none`, this repository's `origin`, a known
        # `head` pass; an empty answer, a banner, another word, `head` without a
        # commit refuse with code 5
        orga, _ = self.two_repositories()
        rev = git(orga, "rev-parse", "HEAD").strip()

        def probe(answer):
            def remote(lane_, script, input=b"", fail=None):
                return subprocess.CompletedProcess([], 0, stdout=answer, stderr=b"")
            return mock.patch.object(lane, "_remote", remote)

        for answer in (b"none\n", b"origin orga\n", ("head %s\n" % rev).encode()):
            with self.subTest(answer=answer), probe(answer):
                self.assertIsNone(lane.check_origin(self.a, str(orga), "/r/wt/t", "orga"))
        for answer in (b"", b"\n", b"Welcome to build01\n", b"maybe orga\n", b"head\n",
                       b"head \n", b"none\nnone\n", b"none x\n", b"origin\n"):
            with self.subTest(answer=answer), probe(answer):
                with self.assertRaisesRegex(lane.LaneError, "^refused -- ") as cm:
                    lane.check_origin(self.a, str(orga), "/r/wt/t", "orga")
                self.assertEqual(5, cm.exception.code)


class TestLaneBuildPath(LaneTestCase):
    """W1 point 1: cargo runs in ONE build path `tree/` per lane."""

    def test_lane_cargo_push_uses_the_build_path(self):
        res = self.push(cargo=True)
        root = self.remote
        self.assertEqual(str(root / "tree"), res["tree"])
        self.assertEqual(str(root / "target"), res["target"])
        self.assertEqual(str(root / "ws"), res["ws_root"])
        self.assertEqual("changed, not committed\n", (root / "tree" / "README.md").read_text())
        self.assertEqual(self.lane_env_text(), (root / "tree" / ".env").read_text())
        self.assertFalse((root / "wt" / "kit").exists())

    def test_lane_python_push_stays_in_wt(self):
        res = self.push(cargo=False)
        self.assertEqual(str(self.wt()), res["tree"])
        self.assertEqual(str(self.remote / "target"), res["target"])
        self.assertFalse((self.remote / "tree").exists())

    def test_lane_build_path_is_shared_and_keeps_unchanged_mtimes(self):
        # `git checkout` gives a new time only to the files the commit
        # changed: a file both commits share keeps its time to the nanosecond.
        self.push(cargo=True, name="strand-a")
        kept = self.remote / "tree" / "scripts" / "kit_change.txt"
        before = kept.stat().st_mtime_ns
        self.commit({"src/other.txt": "another strand's commit\n"})
        res = self.push(cargo=True, name="strand-b")
        self.assertEqual(str(self.remote / "tree"), res["tree"])
        self.assertEqual("another strand's commit\n",
                         (self.remote / "tree" / "src" / "other.txt").read_text())
        self.assertEqual(before, kept.stat().st_mtime_ns)
        self.assertFalse((self.remote / "wt" / "strand-b").exists())


    def test_lane_slot_builds_in_its_own_path(self):
        # W1 `build_path`/`target_path`: slot n >= 1 is `slot/<n>/{tree,target}`,
        # a slot the host file does not grant is refused (code 5).
        wide = dataclasses.replace(self.a, slots=2)
        res = lane.push(wide, str(self.repo), "kit", True, slot=1)
        self.assertEqual(str(self.remote / "slot" / "1" / "tree"), res["tree"])
        self.assertEqual(str(self.remote / "slot" / "1" / "target"), res["target"])
        self.assertEqual("changed, not committed\n",
                         (self.remote / "slot" / "1" / "tree" / "README.md").read_text())
        for ln, slot in ((wide, 2), (self.a, 1), (self.a, "x")):
            with self.assertRaises(lane.LaneError) as cm:
                lane.push(ln, str(self.repo), "kit", True, slot=slot)
            self.assertEqual(5, cm.exception.code)
            self.assertIn("slot", str(cm.exception))

    def test_lane_checkout_keeps_files_the_last_run_touched(self):
        # W1: `update-index --refresh` before the forced checkout -- a file
        # the last run touched (tree-sync) keeps its time when its content
        # is the commit's; without it every touched member was rewritten.
        self.push(cargo=True)
        kept = self.remote / "tree" / "scripts" / "kit_change.txt"
        later = kept.stat().st_mtime + 500
        os.utime(kept, (later, later))
        self.push(cargo=True)
        self.assertEqual(later, kept.stat().st_mtime)


class TestLaneBrowserDrivers(LaneTestCase):
    def setUp(self):
        super().setUp()
        self.commit({"workshop/tools/driver.mjs": "// a driver\n"})

    def test_lane_tree_gets_the_browser_drivers_of_the_lane(self):
        seed = self.remote / "wt" / "lab" / "workshop" / "tools" / "node_modules"
        (seed / "playwright").mkdir(parents=True)
        (seed / "playwright" / "index.js").write_text("// the package\n")
        self.assertEqual("linked", self.push()["browser"])
        nm = self.wt() / "workshop" / "tools" / "node_modules"
        self.assertTrue(nm.is_symlink())
        self.assertEqual("// the package\n", (nm / "playwright" / "index.js").read_text())
        # the cache stands on its own: the seed may go, a second push links again
        shutil.rmtree(self.remote / "wt" / "lab")
        self.assertEqual("linked", self.push(cargo=True)["browser"])
        self.assertTrue((self.remote / "tree" / "workshop" / "tools" / "node_modules"
                         / "playwright" / "index.js").is_file())

    def test_lane_seed_never_lands_inside_a_cache_that_stands(self):
        seed = self.remote / "wt" / "lab" / "workshop" / "tools" / "node_modules"
        (seed / "playwright").mkdir(parents=True)
        cache = self.remote / "cache" / "workshop-tools" / "node_modules"
        cache.mkdir(parents=True)
        self.assertEqual("none", self.push()["browser"])
        self.assertEqual([], sorted(p.name for p in cache.iterdir()))
        self.assertEqual([], [p.name for p in cache.parent.iterdir() if p.name != "node_modules"])

    def test_lane_without_browser_drivers_says_none_and_without_tools_na(self):
        self.assertEqual("none", self.push()["browser"])
        git(self.repo, "rm", "-q", "workshop/tools/driver.mjs")
        git(self.repo, "commit", "-q", "-m", "no tools")
        self.assertEqual("n/a", self.push()["browser"])


class TestLaneGoldens(LaneTestCase):
    """GH #1051: the golden switches; what the run rewrote comes back."""

    GOLDEN = "plans/composition-reference/golden/talky.manifest"

    def setUp(self):
        super().setUp()
        self.commit({self.GOLDEN: "the old golden\n"})

    def run_on_lane(self, tree, writes=None):
        tree = pathlib.Path(tree)
        writes = writes if writes is not None else {
            self.GOLDEN: "rewritten on the lane\n",
            "crates/x/tests/fixtures/gh1_new.json": "new golden\n",
            "crates/x/tests/fixtures/gh1 odd\\name.json": "odd name\n",
            "README.md": "the lane's readme\n"}
        for rel, text in writes.items():
            (tree / rel).parent.mkdir(parents=True, exist_ok=True)
            (tree / rel).write_text(text)
        ws = pathlib.Path(self.remote if tree.is_relative_to(self.remote) else self.local_root)
        logs = ws / "ws" / RUN / "logs"
        logs.mkdir(parents=True, exist_ok=True)
        (logs / "tests.log").write_text("a log\n")
        (logs.parent / "receipt.json").write_text("{}\n")
        (logs.parent / "events.jsonl").write_text("{}\n")

    def fetch(self, pushed, target=None):
        return lane.fetch(target or self.a, RUN, str(self.tmp / "back"), pushed=pushed,
                          repo_root=str(self.repo))

    def test_lane_golden_switch_brings_the_goldens_back(self):
        os.environ["GH277_WRITE_GOLDEN"] = "1"
        os.environ["FAKE_SECRET_API_KEY"] = "sk-never-sent"
        self.assertEqual({"GH277_WRITE_GOLDEN": "1"}, lane.golden_env())
        pushed = self.push()
        self.run_on_lane(pushed["tree"])
        with mock.patch("sys.stderr") as err:
            got = self.fetch(pushed)
        said = "".join(c.args[0] for c in err.write.call_args_list)
        self.assertEqual("rewritten on the lane\n", (self.repo / self.GOLDEN).read_text())
        self.assertEqual("new golden\n",
                         (self.repo / "crates/x/tests/fixtures/gh1_new.json").read_text())
        self.assertEqual("odd name\n",
                         (self.repo / "crates/x/tests/fixtures/gh1 odd\\name.json").read_text())
        self.assertIn("golden back: " + self.GOLDEN, said)
        self.assertEqual("changed, not committed\n", (self.repo / "README.md").read_text())
        self.assertNotIn("README.md", got["goldens_back"])
        self.assertEqual([], got["golden_conflicts"])
        self.assertFalse(got["golden_failed"])
        self.assertNotIn("sk-never-sent", self.ssh_log.read_text())
        # the run's files came back too
        back = self.tmp / "back"
        self.assertTrue((back / "receipt.json").is_file())
        self.assertTrue((back / "events.jsonl").is_file())
        self.assertEqual("a log\n", (back / "logs" / "tests.log").read_text())

    def test_lane_bless_switch_goldens_come_back_from_the_build_path(self):
        # W1 trap: the golden road points at `tree/` when the cargo stations ran there.
        os.environ["MECLAW_GH935_BLESS"] = "1"
        pushed = self.push(cargo=True)
        self.run_on_lane(pushed["tree"], {"crates/x/tests/golden/gh935.txt": "blessed\n"})
        with mock.patch("sys.stderr"):
            got = self.fetch(pushed)
        self.assertEqual("blessed\n", (self.repo / "crates/x/tests/golden/gh935.txt").read_text())
        self.assertEqual(["crates/x/tests/golden/gh935.txt"], got["goldens_back"])

    def test_lane_failed_golden_fetch_says_so(self):
        os.environ["GH277_WRITE_GOLDEN"] = "1"
        pushed = self.push()
        self.run_on_lane(pushed["tree"])
        os.environ["FAKE_RSYNC_FAIL"] = str(self.repo) + "/"
        with mock.patch("sys.stderr") as err:
            got = self.fetch(pushed)
        said = "".join(c.args[0] for c in err.write.call_args_list)
        self.assertTrue(got["golden_failed"])
        self.assertIn("golden fetch failed", said)
        self.assertEqual("the old golden\n", (self.repo / self.GOLDEN).read_text())

    def test_lane_golden_changed_here_is_kept_and_named(self):
        self.commit({"plans/composition-reference/golden/other.manifest": "committed\n"})
        other = self.repo / "plans/composition-reference/golden/other.manifest"
        other.write_text("changed here, not committed\n")
        os.environ["GH277_WRITE_GOLDEN"] = "1"
        pushed = self.push()
        self.run_on_lane(pushed["tree"])
        # an edit HERE during the run: the lane's copy never overwrites it
        (self.repo / self.GOLDEN).write_text("edited here during the run\n")
        with mock.patch("sys.stderr") as err:
            got = self.fetch(pushed)
        said = "".join(c.args[0] for c in err.write.call_args_list)
        self.assertEqual("edited here during the run\n", (self.repo / self.GOLDEN).read_text())
        self.assertEqual([self.GOLDEN], got["golden_conflicts"])
        self.assertIn("golden conflict: " + self.GOLDEN, said)
        self.assertIn("crates/x/tests/fixtures/gh1_new.json", got["goldens_back"])
        # changed here, not on the lane: stays, unnamed
        self.assertEqual("changed here, not committed\n", other.read_text())
        self.assertNotIn("other.manifest", said)

    def test_lane_without_a_golden_switch_nothing_comes_back(self):
        pushed = self.push()
        self.assertIsNone(pushed.get("goldens_before"))
        self.run_on_lane(pushed["tree"])
        got = self.fetch(pushed)
        self.assertEqual("the old golden\n", (self.repo / self.GOLDEN).read_text())
        self.assertEqual([], got["goldens_back"])

    def test_lane_fetch_without_the_run_is_code_2(self):
        with self.assertRaises(lane.LaneError) as cm:
            lane.fetch(self.a, "run-ffffffffffff", str(self.tmp / "back"))
        self.assertEqual(2, cm.exception.code)


class TestLaneLocal(LaneTestCase):
    """W1 point 2: the transport `local` -- no ssh, no network."""

    def test_lane_local_transport_runs_without_ssh(self):
        res = self.push(cargo=True, target=self.local)
        root = self.local_root
        self.assertEqual(str(root / "tree"), res["tree"])
        self.assertEqual(str(root / "target"), res["target"])
        self.assertEqual(lane.digest(str(self.repo)), res["overlay_hash"])
        self.assertEqual("changed, not committed\n", (root / "tree" / "README.md").read_text())
        self.assertFalse((root / "tree" / "gone.txt").exists())
        self.assertFalse((root / "tree" / "vault.env").exists())
        self.assertEqual(self.lane_env_text(), (root / "tree" / ".env").read_text())
        # the bare repository borrows this clone's objects
        alt = (root / "meclaw.git" / "objects" / "info" / "alternates").read_text().strip()
        self.assertEqual(str((self.repo / ".git" / "objects").resolve()), alt)
        self.assertEqual("", self.ssh_log.read_text())
        self.assertNotIn("ssh", self.rsync_log.read_text())
        # fetch and status stay local too
        self.run_local()
        lane.fetch(self.local, RUN, str(self.tmp / "back"))
        self.assertTrue((self.tmp / "back" / "receipt.json").is_file())
        self.assertTrue(lane.status(self.local).startswith("reachable"))
        self.assertEqual("", self.ssh_log.read_text())

    def run_local(self):
        ws = self.local_root / "ws" / RUN
        (ws / "logs").mkdir(parents=True)
        (ws / "receipt.json").write_text("{}\n")
        (ws / "events.jsonl").write_text("")

    def test_lane_local_refuses_a_secret_before_any_write(self):
        self.commit({".cargo/credentials.toml": "NEVER=sent\n"})
        with self.assertRaises(lane.LaneError) as cm:
            self.push(cargo=True, target=self.local)
        self.assertEqual(5, cm.exception.code)
        self.assertFalse(self.local_root.exists())

    def test_lane_push_mirrors_returns_the_mirror_a_worktree_lands_in(self):
        # W10 KF1-Rest: a worktree's git dir is the main repository's
        # (`--git-common-dir`), so the mirror lands under that repository's
        # name, not the worktree's; the returned path must be the one rsync
        # wrote, from the same derivation as `register.mirrors_spec`.
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-q", "-m", "wt base")
        wt = self.tmp / (self.repo.name + "-wt-x")
        git(self.repo, "worktree", "add", "-q", str(wt), "-b", "wt-x")
        out = lane.push_mirrors(self.local, [str(wt), str(self.repo)])
        want = str(self.local_root / "mirrors" / self.repo.name)
        self.assertEqual([want, want], out)
        self.assertEqual(git(self.repo, "rev-parse", "wt-x"), git(want, "rev-parse", "wt-x"))

class TestLaneExecStream(LaneTestCase):
    def dispatch(self, tree):
        return {"run": RUN, "repo": "meclaw", "mode": "strand", "tree": tree, "stations": []}

    def read(self, proc):
        with proc.stdout:
            events = [json.loads(line) for line in proc.stdout]
        self.assertEqual(0, proc.wait())
        return events

    def test_lane_exec_stream_ssh_command_line(self):
        tree = self.push(cargo=True)["tree"]
        events = self.read(lane.exec_stream(self.a, self.dispatch(tree)))
        self.assertEqual(["hygiene_checked", "pass_ended"], [e["event"] for e in events])
        self.assertEqual(RUN, events[0]["run"])
        self.assertEqual(["exec", "--dispatch", "-"], events[0]["data"]["argv"])
        call = [l for l in self.ssh_log.read_text().splitlines() if "pipeline.py" in l]
        self.assertEqual(1, len(call), self.ssh_log.read_text())
        self.assertIn("systemd-run --user --scope --unit=pipe-%s --collect %s "
                      "%s/scripts/pipeline.py exec --dispatch -" % (RUN, shlex.join(lane.PYTHON), tree), call[0])
        self.assertIn("--unit=pipe-%s" % RUN, self.systemd_log.read_text())

    def test_lane_exec_stream_local_with_and_without_scope(self):
        tree = self.push(cargo=True, target=self.local)["tree"]
        events = self.read(lane.exec_stream(self.local, self.dispatch(tree)))
        self.assertEqual("pass_ended", events[-1]["event"])
        self.assertIn("--user --scope --unit=pipe-%s --collect %s %s/scripts/pipeline.py"
                      % (RUN, shlex.join(lane.PYTHON), tree), self.systemd_log.read_text())
        self.systemd_log.write_text("")
        os.environ["MECLAW_PIPELINE_NO_SCOPE"] = "1"
        events = self.read(lane.exec_stream(self.local, self.dispatch(tree)))
        self.assertEqual(["exec", "--dispatch", "-"], events[0]["data"]["argv"])
        self.assertEqual(str(pathlib.Path(tree).resolve()),
                         str(pathlib.Path(events[0]["data"]["cwd"]).resolve()))
        self.assertEqual("", self.systemd_log.read_text())
        self.assertEqual("", self.ssh_log.read_text())

    def test_lane_exec_stream_says_whether_it_started_a_scope(self):
        # I1: the gate probes a run's unit only when the run has one.
        tree = self.push(cargo=True)["tree"]
        local = self.push(cargo=True, target=self.local)["tree"]
        for no_scope, want in (("0", True), ("1", False)):
            os.environ["MECLAW_PIPELINE_NO_SCOPE"] = no_scope
            for ln, t in ((self.a, tree), (self.local, local)):
                proc = lane.exec_stream(ln, self.dispatch(t))
                self.read(proc)
                self.assertIs(want, proc.scoped, (ln.name, no_scope))

    def test_lane_exec_stream_local_without_systemd_run_has_no_scope(self):
        # Z17 (M-neu-2 of the Z13b re-check): a local lane without
        # `systemd-run` on PATH starts no scope, and says so.
        local = self.push(cargo=True, target=self.local)["tree"]
        os.environ["MECLAW_PIPELINE_NO_SCOPE"] = "0"
        with mock.patch.object(lane.shutil, "which", return_value=None):
            proc = lane.exec_stream(self.local, self.dispatch(local))
        self.read(proc)
        self.assertIs(False, proc.scoped)
        self.assertEqual("", self.systemd_log.read_text())

    def test_lane_exec_stream_lost_host_ends_without_pass_ended(self):
        dead = lane.find_lane("lane-dead")
        proc = lane.exec_stream(dead, self.dispatch(str(self.remote / "tree")),
                                stderr=subprocess.DEVNULL)
        with proc.stdout:
            self.assertEqual([], list(proc.stdout))
        self.assertEqual(255, proc.wait())


class TestLaneCpuQuota(LaneTestCase):
    """TV-F7 (tv-p1c § 6): `MECLAW_LANE_CPU_QUOTA` runs the lane's tier in a
    `systemd-run --user --scope -p CPUQuota=<n>%`; a bad value, or class egon,
    is a refusal before anything runs."""

    FAKE_TIER = ("#!/bin/sh\necho \"fake-tier: $* brake=${MECLAW_BROWSER_CPU_THROTTLE:-none}\"\n"
                 "echo \"fake-tier-factor: ${MECLAW_TEST_TIME_FACTOR:-unset}\"\n"
                 "echo \"fake-tier-quota-mark: ${MECLAW_TEST_CPU_QUOTA:-unset}\"\nexit 0\n")
    FAKE_SYSTEMD_RUN = ("#!/bin/sh\necho \"fake-systemd-run: $*\"\n"
                        "while [ $# -gt 0 ] && [ \"$1\" != -- ]; do shift; done\nshift\nexec \"$@\"\n")

    def setUp(self):
        super().setUp()
        tier = self.repo / "scripts" / "test-tier.sh"
        tier.write_text(self.FAKE_TIER)
        tier.chmod(0o755)
        (self.bin / "systemd-run").write_text(self.FAKE_SYSTEMD_RUN)
        (self.bin / "systemd-run").chmod(0o755)
        self.push(cargo=True)
        self.log = self.tmp / "test.log"
        os.environ["MECLAW_LANE_FLOOR_G"] = "0"
        self.addCleanup(os.environ.pop, "MECLAW_LANE_CPU_QUOTA", None)
        self.addCleanup(os.environ.pop, "MECLAW_BROWSER_CPU_THROTTLE", None)
        self.addCleanup(os.environ.pop, "MECLAW_TEST_TIME_FACTOR", None)
        # tv-br M10u (re-check R-delta M1): the scope's own cgroup tree, empty
        # unless a test gives the scope an effective quota (`effective_quota`).
        self.cgfs = self.tmp / "cgfs"
        self.cgfs.mkdir()
        # R<->N merge NS: the lane runs its command under `env -i` (REMOTE_BASE,
        # OR-T10-2), so no name of this side reaches the scope -- the fake host's
        # `systemd-run` points its scope at the fake cgroup tree, as the host's
        # own cgroup mount would be.
        (self.bin / "systemd-run").write_text(self.FAKE_SYSTEMD_RUN.replace(
            "#!/bin/sh\n",
            "#!/bin/sh\nexport MECLAW_LANE_CGROUP_FS=%s\n" % shlex.quote(str(self.cgfs)), 1))

    def effective_quota(self, cpu_max):
        """The scope's `cpu.max` as the kernel shows it with the cpu controller
        delegated: the fake `systemd-run` execs in place, so the scope's cgroup
        is this process's own (`0::<path>` of /proc/self/cgroup)."""
        own = ""
        for line in pathlib.Path("/proc/self/cgroup").read_text().splitlines():
            if line.startswith("0::"):
                own = line[3:]
        scope = self.cgfs / own.lstrip("/")
        scope.mkdir(parents=True, exist_ok=True)
        (scope / "cpu.max").write_text(cpu_max + "\n")

    def test_the_time_factor_reaches_the_tier_and_the_log(self):
        """OR-T12-13: `MECLAW_TEST_TIME_FACTOR` crosses the ssh (which carries
        no environment) by exact name, and the run's log names it."""
        self.effective_quota("80000 100000")
        os.environ["MECLAW_LANE_CPU_QUOTA"] = "80"
        os.environ["MECLAW_TEST_TIME_FACTOR"] = "2.5"
        rc = lane.run_test(self.a, "kit", "binary(~x)", [], str(self.log))
        text = self.log.read_text()
        self.assertEqual(0, rc, text)
        self.assertIn("fake-tier-factor: 2.5", text)
        self.assertIn("# time factor: MECLAW_TEST_TIME_FACTOR=2.5", text)

    def test_a_quota_run_sets_the_quota_mark_the_marks_read(self):
        """tv-br M10q-a (re-check R K3): the scope with the quota hands the tier
        `MECLAW_TEST_CPU_QUOTA=<pct>` -- the mark `meclaw_testing::deadline::
        time_factor` reads before it admits a factor other than 1. tv-br M10u
        (re-check R-delta M1): the percent the scope's `cpu.max` enforces."""
        self.effective_quota("80000 100000")
        os.environ["MECLAW_LANE_CPU_QUOTA"] = "80"
        os.environ["MECLAW_TEST_TIME_FACTOR"] = "3"
        rc = lane.run_test(self.a, "kit", "binary(~x)", [], str(self.log))
        text = self.log.read_text()
        self.assertEqual(0, rc, text)
        self.assertIn("fake-tier-quota-mark: 80", text)

    def test_the_quota_mark_is_the_quota_the_scope_enforces_not_the_one_asked(self):
        """tv-br M10u (re-check R-delta M1): the mark comes from the scope's
        `cpu.max`, never from `MECLAW_LANE_CPU_QUOTA` -- asked 80, enforced 50,
        the tier reads 50."""
        self.effective_quota("50000 100000")
        os.environ["MECLAW_LANE_CPU_QUOTA"] = "80"
        rc = lane.run_test(self.a, "kit", "binary(~x)", [], str(self.log))
        text = self.log.read_text()
        self.assertEqual(0, rc, text)
        self.assertIn("fake-tier-quota-mark: 50", text)

    def test_a_scope_without_an_effective_quota_sets_no_quota_mark(self):
        """tv-br M10u (re-check R-delta M1, fail-open before): a lane whose
        user manager has no cpu controller runs the scope unthrottled -- no
        `cpu.max`, or `max` -- and the tier sees no mark."""
        os.environ["MECLAW_LANE_CPU_QUOTA"] = "80"
        for cpu_max in (None, "max 100000"):
            if cpu_max is not None:
                self.effective_quota(cpu_max)
            self.log.write_text("")
            rc = lane.run_test(self.a, "kit", "binary(~x)", [], str(self.log))
            text = self.log.read_text()
            self.assertEqual(0, rc, text)
            self.assertIn("fake-tier-quota-mark: unset", text)

    def test_a_time_factor_in_a_scope_without_an_effective_quota_is_a_refusal(self):
        """tv-br M10u (re-check R-delta M1): a factor other than 1 in a scope
        whose quota does not bite ends the run before the tier starts."""
        os.environ["MECLAW_LANE_CPU_QUOTA"] = "80"
        os.environ["MECLAW_TEST_TIME_FACTOR"] = "3"
        for cpu_max in (None, "max 100000"):
            if cpu_max is not None:
                self.effective_quota(cpu_max)
            self.log.write_text("")
            rc = lane.run_test(self.a, "kit", "binary(~x)", [], str(self.log))
            text = self.log.read_text()
            self.assertNotEqual(0, rc, text)
            self.assertNotIn("fake-tier:", text)
            self.assertIn("no effective cpu quota in the scope", text)

    def test_without_a_quota_no_quota_mark_reaches_the_tier(self):
        os.environ["MECLAW_TEST_CPU_QUOTA"] = "80"  # a stray export never crosses
        self.addCleanup(os.environ.pop, "MECLAW_TEST_CPU_QUOTA", None)
        rc = lane.run_test(self.a, "kit", "binary(~x)", [], str(self.log))
        text = self.log.read_text()
        self.assertEqual(0, rc, text)
        self.assertIn("fake-tier-quota-mark: unset", text)

    def test_a_time_factor_without_a_quota_is_a_refusal(self):
        """tv-br M10o (re-check B-M4): a factor other than 1 without
        `MECLAW_LANE_CPU_QUOTA` ends the run before the tier starts."""
        os.environ["MECLAW_TEST_TIME_FACTOR"] = "3"
        rc = lane.run_test(self.a, "kit", "binary(~x)", [], str(self.log))
        text = self.log.read_text()
        self.assertNotEqual(0, rc, text)
        self.assertNotIn("fake-tier:", text)
        self.assertIn("a factor belongs to a quota run only", text)

    def test_without_a_time_factor_none_reaches_the_tier(self):
        rc = lane.run_test(self.a, "kit", "binary(~x)", [], str(self.log))
        text = self.log.read_text()
        self.assertEqual(0, rc, text)
        self.assertIn("fake-tier-factor: unset", text)
        self.assertNotIn("time factor", text)

    def test_a_cpu_quota_wraps_the_tier_in_a_user_scope(self):
        os.environ["MECLAW_LANE_CPU_QUOTA"] = "80"
        rc = lane.run_test(self.a, "kit", "binary(~x)", [], str(self.log))
        text = self.log.read_text()
        self.assertEqual(0, rc, text)
        # tv-br M10u: the scope runs QUOTA_MARK_SH first, which execs the tier
        self.assertIn("fake-systemd-run: --user --scope -q -p CPUQuota=80% -- bash -c ", text)
        self.assertIn(" quota-mark scripts/test-tier.sh filter binary(~x)", text)
        self.assertIn("fake-tier: filter binary(~x) --no-fail-fast", text)

    def test_without_a_quota_the_tier_runs_bare(self):
        rc = lane.run_test(self.a, "kit", "binary(~x)", [], str(self.log))
        text = self.log.read_text()
        self.assertEqual(0, rc, text)
        self.assertNotIn("fake-systemd-run", text)
        self.assertIn("fake-tier: filter binary(~x)", text)

    def test_a_bad_quota_is_a_refusal(self):
        os.environ["MECLAW_LANE_CPU_QUOTA"] = "80%"
        rc = lane.run_test(self.a, "kit", "binary(~x)", [], str(self.log))
        text = self.log.read_text()
        self.assertEqual(2, rc, text)
        self.assertNotIn("fake-tier:", text)

    # GH #1210 (TV-B7): the Chromium brake of the browser drivers is an
    # environment word; ssh does not carry it, so it travels by name and the
    # log says it did -- the drivers' own THROTTLE line dies in the test's
    # captured stderr.
    def test_the_browser_brake_travels_to_the_tier_and_is_said(self):
        os.environ["MECLAW_BROWSER_CPU_THROTTLE"] = "4"
        rc = lane.run_test(self.a, "kit", "binary(~x)", [], str(self.log))
        text = self.log.read_text()
        self.assertEqual(0, rc, text)
        self.assertIn("brake=4", text)
        self.assertIn("=== browser cpu brake: x4 (MECLAW_BROWSER_CPU_THROTTLE, Chromium CDP)", text)

    def test_without_a_brake_none_travels(self):
        rc = lane.run_test(self.a, "kit", "binary(~x)", [], str(self.log))
        text = self.log.read_text()
        self.assertEqual(0, rc, text)
        self.assertIn("brake=none", text)
        self.assertNotIn("=== browser cpu brake", text)

    def test_a_bad_brake_is_a_refusal(self):
        os.environ["MECLAW_BROWSER_CPU_THROTTLE"] = "x4; true"
        rc = lane.run_test(self.a, "kit", "binary(~x)", [], str(self.log))
        text = self.log.read_text()
        self.assertEqual(2, rc, text)
        self.assertNotIn("fake-tier:", text)

    # Re-check tv-bn-drv2-fix1 § 5: `throttleRate` throws below 1 (`0`, `0.5`); the
    # lane passed them on, and the drivers died on the lane instead of the run
    # being refused before anything ran. Same rule on both sides now.
    def test_a_brake_below_one_is_a_refusal(self):
        for word in ("0", "0.5", "00"):
            with self.subTest(brake=word):
                os.environ["MECLAW_BROWSER_CPU_THROTTLE"] = word
                rc = lane.run_test(self.a, "kit", "binary(~x)", [], str(self.log))
                text = self.log.read_text()
                self.assertEqual(2, rc, text)
                self.assertNotIn("fake-tier:", text)
                self.assertIn("a slowdown factor >= 1", text)


class TestLaneVerdictEnv(LaneTestCase):
    """GH #1210 (welle-tv tv-bn M5): the names that change a verdict travel to
    the lane's tier by exact name (lane_sync.sh VERDICT_ENV, one list for
    `strand.sh test`, the gate's lane environment, flake_triage and measure).
    Measured before: `strand.sh test` with MECLAW_REQUIRE_BROWSERS=1 ran the
    browser drivers on build03 without it -- a driver SKIP (no engine) graded
    GREEN (tv-bn M4)."""

    # The tier of the fixture: says what arrived, then the drivers' rule
    # (gh767_the_page_canvas_draws_and_sends_browser.rs): no engine here, a
    # skip is green -- unless MECLAW_REQUIRE_BROWSERS=1, then it is red.
    FAKE_TIER = ("#!/bin/sh\n"
                 "for n in %s; do eval \"echo arrived $n=\\${$n:-unset}\"; done\n"
                 % " ".join(lane.verdict_env()) +
                 "if [ \"${MECLAW_REQUIRE_BROWSERS:-}\" = 1 ]; then\n"
                 "    echo 'a skip under MECLAW_REQUIRE_BROWSERS=1 is red'; exit 100\n"
                 "fi\n"
                 "echo 'SKIP: no engine on this host'; exit 0\n")
    NAMES = ("MECLAW_ALLOW_BUMP", "MECLAW_GATE_MODE", "MECLAW_REQUIRE_BROWSERS",
             "MECLAW_TESTING_RULES_RULING")

    def setUp(self):
        super().setUp()
        tier = self.repo / "scripts" / "test-tier.sh"
        tier.write_text(self.FAKE_TIER)
        tier.chmod(0o755)
        self.push(cargo=True)
        self.log = self.tmp / "test.log"
        os.environ["MECLAW_LANE_FLOOR_G"] = "0"
        for name in self.NAMES:
            os.environ.pop(name, None)

    def test_the_list_is_the_one_of_lane_sync(self):
        self.assertEqual(self.NAMES, lane.verdict_env())

    def test_each_verdict_name_arrives_on_the_lane(self):
        values = {n: "value of %s" % n for n in lane.verdict_env()}
        values.update({"MECLAW_ALLOW_BUMP": "merge point of x", "MECLAW_GATE_MODE": "strand",
                       "MECLAW_REQUIRE_BROWSERS": "0", "MECLAW_TESTING_RULES_RULING": "OR-TV-8"})
        os.environ.update(values)
        rc = lane.run_test(self.a, "kit", "binary(~x)", [], str(self.log))
        text = self.log.read_text()
        self.assertEqual(0, rc, text)
        for name, value in values.items():
            self.assertIn("arrived %s=%s" % (name, value), text)

    def test_require_browsers_without_an_engine_is_red_not_a_green_skip(self):
        os.environ["MECLAW_REQUIRE_BROWSERS"] = "1"
        rc = lane.run_test(self.a, "kit", "binary(~x)", [], str(self.log))
        text = self.log.read_text()
        self.assertNotEqual(0, rc, text)
        self.assertIn("a skip under MECLAW_REQUIRE_BROWSERS=1 is red", text)
        self.assertNotIn("SKIP: no engine", text)

    def test_a_name_unset_here_travels_nowhere(self):
        # the browser switch aside: a test run sets it itself (below)
        os.environ["MECLAW_REQUIRE_BROWSERS"] = "0"
        rc = lane.run_test(self.a, "kit", "binary(~x)", [], str(self.log))
        text = self.log.read_text()
        self.assertEqual(0, rc, text)
        for name in lane.verdict_env():
            if name != "MECLAW_REQUIRE_BROWSERS":
                self.assertIn("arrived %s=unset" % name, text)

    def test_a_test_run_requires_the_browsers_unless_told_otherwise(self):
        # GH #1209 (re-check tv-bn M7 M4): nobody set the switch outside the
        # gate -- `strand.sh test` (and flake_triage's TV-F7 runs through it)
        # graded a driver SKIP on a lane without the engine GREEN.
        rc = lane.run_test(self.a, "kit", "binary(~x)", [], str(self.log))
        text = self.log.read_text()
        self.assertIn("arrived MECLAW_REQUIRE_BROWSERS=1", text)
        self.assertNotEqual(0, rc, text)
        self.assertIn("a skip under MECLAW_REQUIRE_BROWSERS=1 is red", text)
        self.assertNotIn("SKIP: no engine", text)

    def test_an_opt_out_of_the_browsers_is_a_note(self):
        os.environ["MECLAW_REQUIRE_BROWSERS"] = "0"
        rc = lane.run_test(self.a, "kit", "binary(~x)", [], str(self.log))
        text = self.log.read_text()
        self.assertEqual(0, rc, text)
        self.assertIn("NOTE: MECLAW_REQUIRE_BROWSERS='0' (opt-out) -- a browser driver without an "
                      "engine SKIPs green, no browser proof", text)

    def test_the_gate_lane_environment_carries_the_list(self):
        os.environ["MECLAW_REQUIRE_BROWSERS"] = "1"
        os.environ["MECLAW_TESTING_RULES_RULING"] = "OR-TV-8"
        env = lane.env_for(self.a, "strand", nproc=8)
        self.assertEqual("1", env.get("MECLAW_REQUIRE_BROWSERS"))
        self.assertEqual("OR-TV-8", env.get("MECLAW_TESTING_RULES_RULING"))

    def test_verdict_words_are_quoted_and_only_the_set_names(self):
        words = lane.verdict_env_words({"MECLAW_GATE_MODE": "a b; true", "MECLAW_REQUIRE_BROWSERS": "",
                                        "OTHER_KEY": "x"})
        self.assertEqual(["MECLAW_GATE_MODE='a b; true'"], words)


class TestLaneEgonGuard(LaneTestCase):
    """Re-check pd-legacy M-1: the protection class of the owner's machine on
    `lane_sync.sh test` (nice/ionice, the watched unit, the watchdog that kills
    the run). Its pins ran over `strand.sh test --host` and went with it."""

    FAKE_TIER = ("#!/bin/sh\necho \"fake-tier: $* nice=$(nice)\"\n"
                 "[ -n \"${FAKE_TIER_PID:-}\" ] && echo $$ >\"$FAKE_TIER_PID\"\n"
                 # held: only a kill ends it (re-check tv-bn M7j B-M2: `sleep 600`
                 # against waits of 2 * HANG went hollow from time factor 10)
                 "[ -n \"${FAKE_TIER_HOLD:-}\" ] && exec sleep infinity\n"
                 # FAKE_TIER_AFTER: a file that names a process group; end only once
                 # it exists and that group is gone -- every member ended and
                 # reaped (M7l H6: a `sleep 2` guessed that event and went hollow
                 # under load); past HANG a loud end (M7j B-M1)
                 "if [ -n \"${FAKE_TIER_AFTER:-}\" ]; then n=0\n"
                 "  while [ ! -e \"$FAKE_TIER_AFTER\" ]; do [ $n -lt %(cap)d ] || "
                 "{ echo \"HANG: fake-tier: $FAKE_TIER_AFTER never came\" >&2; exit 9; }; "
                 "sleep 0.05; n=$((n + 1)); done\n"
                 "  g=$(tr -dc 0-9 <\"$FAKE_TIER_AFTER\")\n"
                 "  [ -n \"$g\" ] || { echo \"HANG: fake-tier: $FAKE_TIER_AFTER names no group\" >&2; exit 9; }\n"
                 "  while ps -e -o pgid= | grep -qx \" *$g\"; do [ $n -lt %(cap)d ] || "
                 "{ echo \"HANG: fake-tier: group $g never gone\" >&2; exit 9; }; "
                 "sleep 0.05; n=$((n + 1)); done; fi\n"
                 "[ -n \"${FAKE_TIER_DONE:-}\" ] && : >\"$FAKE_TIER_DONE\"\nexit 0\n"
                 % {"cap": int(kit_time.HANG / 0.05)})

    # The watcher of run_guarded (its script names "watchdog trip") calls
    # setsid only once the run's stop of the watcher has begun -- the run's
    # xtrace line `+ stop_watcher` in its log, written after the tier ended --
    # so the stop lands before the watcher has its group (#1185). A `sleep 1`
    # after the tier's end guessed that order and went hollow when the run was
    # slower than the second (M7l H6). Any other setsid passes straight through.
    FAKE_SETSID = """#!/bin/sh
case "$*" in *"watchdog trip"*)
    echo $$ >"$FAKE_SETSID_PID"
    n=0
    until grep -qE '^[+]+ stop_watcher ' "$FAKE_RUN_LOG"; do
        [ "$n" -lt %(cap)d ] || { echo "HANG: fake-setsid: the run's stop never began" >&2; exit 9; }
        sleep 0.05; n=$((n + 1))
    done ;;
esac
exec %(real)s "$@"
"""

    def setUp(self):
        super().setUp()
        tier = self.repo / "scripts" / "test-tier.sh"
        tier.write_text(self.FAKE_TIER)
        tier.chmod(0o755)
        self.push(cargo=True, target=self.local)
        self.log = self.tmp / "test.log"
        for var in ("MECLAW_LANE_WATCH_CMD", "FAKE_TIER_HOLD", "FAKE_TIER_DONE", "FAKE_SETSID_PID",
                    "FAKE_TIER_PID", "FAKE_TIER_AFTER", "FAKE_WATCH_PID", "FAKE_WATCH_DONE",
                    "MECLAW_LANE_WATCH_HANDSHAKE_S", "MECLAW_LANE_XTRACE", "FAKE_RUN_LOG"):
            os.environ.pop(var, None)
        # The floor is not what these pins test (its own pin is
        # test_lane_guard_floor_fails_closed_and_slots_are_granted): 0 turns it
        # off, so the free space of the host's temp FS cannot refuse the run.
        os.environ["MECLAW_LANE_FLOOR_G"] = "0"

    def watched(self, unit="guarded.service"):
        return dataclasses.replace(self.local, opts=dict(self.local.opts, watch=unit))

    def test_egon_class_needs_a_watched_unit(self):
        rc = lane.run_test(self.watched(""), "kit", "binary(~x)", [], str(self.log))
        self.assertEqual(2, rc)
        self.assertIn("class egon needs the unit it guards", self.log.read_text())
        self.assertNotIn("fake-tier:", self.log.read_text())

    def test_egon_class_runs_the_tier_nice_and_idle(self):
        os.environ["MECLAW_LANE_WATCH_CMD"] = "exec tail -f /dev/null"
        rc = lane.run_test(self.watched(), "kit", "binary(~x)", [], str(self.log))
        text = self.log.read_text()
        self.assertEqual(0, rc, text)
        self.assertIn("# cargo hygiene: class egon -- nice -n 19 ionice -c3", text)
        self.assertIn("fake-tier: filter binary(~x) --no-fail-fast nice=19", text)

    def test_egon_watcher_stopped_before_its_setsid_never_hangs_the_run(self):
        """#1185: the tier ended before the watcher had called setsid(); the
        stop of the watcher's group went nowhere and `wait` hung on the
        journal follower for good (5/240 under load, a gate shard stalled
        1200 s). Here the follower ends by itself after 30 s, so a hang shows
        as a run of 30 s instead of a stuck test and leaves no orphan."""
        self.stop_before_setsid()

    def test_m_n14_without_a_handshake_the_watcher_is_stopped_by_its_pid(self):
        """Re-check m-N14: the pin above meets the handshake late, within the
        default wait; the branch without any handshake (the TERM to the
        watcher's PID) ran in no test -- dropping that TERM stayed green.
        A wait of 0 s takes that branch."""
        os.environ["MECLAW_LANE_WATCH_HANDSHAKE_S"] = "0"
        # The TERM to the PID may land before the fake setsid has run a line.
        self.stop_before_setsid(seen=False)

    def stop_before_setsid(self, seen=True):
        real = shutil.which("setsid")
        self.assertIsNotNone(real)
        fake = self.bin / "setsid"
        fake.write_text(self.FAKE_SETSID % {"real": shlex.quote(real), "cap": int(kit_time.HANG / 0.05)})
        fake.chmod(0o755)
        pid_file = self.tmp / "watcher.pid"
        # A watcher the run never stops holds it past the hang marker: the stop
        # is the event, not the run's seconds.
        # The run's xtrace carries the marker the fake setsid waits on.
        os.environ.update({"MECLAW_LANE_WATCH_CMD": "exec sleep infinity",
                           "FAKE_TIER_DONE": str(self.tmp / "tier.done"),
                           "FAKE_SETSID_PID": str(pid_file),
                           "FAKE_RUN_LOG": str(self.log), "MECLAW_LANE_XTRACE": "1"})
        with self.hang_bounded("the run waits on a watcher it never stopped"):
            rc = lane.run_test(self.watched(), "kit", "binary(~x)", [], str(self.log))
        text = self.log.read_text()
        self.assertEqual(0, rc, text)
        self.assertTrue((self.tmp / "tier.done").exists(), text)
        seen_pid = pid_file.read_text().strip() if pid_file.exists() else ""
        if not seen and not seen_pid:
            return
        self.assertTrue(seen_pid, "the fake setsid never saw the watcher")
        watcher = int(seen_pid)

        def gone():  # the TERM of the group is delivered, its exit is not instant
            try:
                os.killpg(watcher, 0)
            except ProcessLookupError:
                return True
            return False

        def group():
            ps = subprocess.run(["ps", "-e", "-o", "pid=,ppid=,pgid=,stat=,args="],
                                capture_output=True, text=True).stdout.splitlines()
            return [l for l in ps if l.split()[2] == str(watcher)]

        kit_time.until("the watcher's group %d ends with the run" % watcher, gone, state=group)

    @contextlib.contextmanager
    def hang_bounded(self, desc):
        """`lane.subprocess.run` with the hang marker as its timeout: a run that
        misses its stop fails `HANG: <desc>` instead of holding the test."""
        real_run = subprocess.run

        def bounded(*a, **kw):
            try:
                return real_run(*a, timeout=kit_time.HANG, **kw)
            except subprocess.TimeoutExpired:
                tail = self.log.read_text()[-400:] if self.log.exists() else ""
                raise kit_time.Hang("HANG: %s after %.0f s (last: %r)"
                                    % (desc, kit_time.HANG, tail)) from None

        with mock.patch.object(lane.subprocess, "run", bounded):
            yield

    WATCH_NAMES_ITS_GROUP = ('ps -o pgid= -p $$ >"$FAKE_WATCH_DONE.part" && '
                             'exec mv "$FAKE_WATCH_DONE.part" "$FAKE_WATCH_DONE"')

    def test_egon_a_watcher_gone_after_its_handshake_gets_no_pid_kill(self):
        """Re-check latent (b) / ruling 7d (a): after the handshake the watcher
        leads its own group, so the TERM of the group is the whole stop. The
        PID fallback (TERM to the bare pid) fired only when that group was gone,
        i.e. the watcher had ended and the shell had reaped it: a TERM to a
        free PID, or to whatever process got it since. The watcher here ends
        by itself before the tier does; the shell's trace shows each kill."""
        # The watcher names its group in the file and ends; the tier ends once
        # that group is gone (FAKE_TIER_AFTER), so the run's stop meets a
        # watcher already reaped.
        os.environ.update({"MECLAW_LANE_WATCH_CMD": self.WATCH_NAMES_ITS_GROUP,
                           "FAKE_WATCH_DONE": str(self.tmp / "watch.done"),
                           "FAKE_TIER_AFTER": str(self.tmp / "watch.done"),
                           "MECLAW_LANE_XTRACE": "1"})
        rc = lane.run_test(self.watched(), "kit", "binary(~x)", [], str(self.log))
        text = self.log.read_text()
        self.assertEqual(0, rc, text)
        self.assertTrue((self.tmp / "watch.done").exists(), text)
        groups = re.findall(r"^\++ kill -TERM -- -(\d+)$", text, re.M)
        self.assertTrue(groups, "no stop of the watcher's group in the trace:\n" + text)
        hits = [line for line in text.splitlines()
                if re.fullmatch(r"\++ kill -TERM (%s)" % "|".join(groups), line)]
        self.assertEqual([], hits, "a TERM went to the PID of a watcher that had ended")

    def test_egon_an_aborted_run_leaves_no_fifo_tier_or_watcher(self):
        """Re-check latent (a) / ruling 7d (b): run_guarded had no trap; a TERM
        to lane_sync.sh during the tier left the handshake FIFO in TMPDIR, the
        tier and the watcher behind. Now the abort stops both and removes it."""
        tmpdir = self.tmp / "rg-tmp"
        tmpdir.mkdir()
        pids = {"tier": self.tmp / "tier.pid", "watch": self.tmp / "watch.pid"}
        os.environ.update({"TMPDIR": str(tmpdir), "FAKE_TIER_HOLD": "1",
                           "FAKE_TIER_PID": str(pids["tier"]),
                           "FAKE_WATCH_PID": str(pids["watch"]),
                           # a watcher the abort misses outlives the hang marker
                           "MECLAW_LANE_WATCH_CMD": 'echo $$ >"$FAKE_WATCH_PID"; exec sleep infinity'})

        def up():
            return all(p.exists() and p.read_text().strip() for p in pids.values())

        def aborted(argv, **kw):
            proc = subprocess.Popen(argv, **kw)
            try:
                # the event: both PID files written, or the run ended before
                kit_time.until("the guarded run brings its tier and watcher up",
                               lambda: up() or proc.poll() is not None,
                               state=lambda: {k: p.exists() for k, p in pids.items()})
                if not up():
                    self.fail("the guarded run ended (rc %s) before its tier and watcher were up"
                              % proc.returncode)
                proc.terminate()
                try:
                    return subprocess.CompletedProcess(argv, proc.wait(timeout=kit_time.HANG))
                except subprocess.TimeoutExpired:
                    raise kit_time.Hang("HANG: the aborted run after %.0f s" % kit_time.HANG) from None
            finally:
                if proc.poll() is None:
                    proc.kill()
                    proc.wait()

        with mock.patch.object(lane.subprocess, "run", aborted):
            rc = lane.run_test(self.watched(), "kit", "binary(~x)", [], str(self.log))
        text = self.log.read_text()
        self.assertEqual(143, rc, text)
        self.assertEqual([], sorted(p.name for p in tmpdir.iterdir()),
                         "the abort left its temp files (the handshake FIFO)")
        for what, path in pids.items():
            pid = int(path.read_text())

            def gone():  # the TERM is delivered, the exit is not instant
                try:
                    os.kill(pid, 0)
                except ProcessLookupError:
                    return True
                return False

            # both hold `sleep infinity`: only the abort's stop ends them
            kit_time.until("the %s (%d) ends with the aborted run" % (what, pid), gone)

    def test_egon_watchdog_trip_kills_the_run(self):
        os.environ["MECLAW_LANE_WATCH_CMD"] = \
            "echo 'guarded.service: Watchdog timeout (limit 3min)!'"
        os.environ["FAKE_TIER_HOLD"] = "1"   # only the watchdog's kill ends the tier
        with self.hang_bounded("the held tier was not killed"):
            rc = lane.run_test(self.watched(), "kit", "binary(~x)", [], str(self.log), run="r-1")
        text = self.log.read_text()
        self.assertEqual(2, rc, text)
        line = next(l for l in text.splitlines() if "watchdog_fired" in l)
        body = json.loads(line.split("watchdog_fired ", 1)[1])
        self.assertEqual(("egon", "process", "killed", "lane-l", "r-1"),
                         (body["reason"], body["kind"], body["action"], body["runner"],
                          body["run"]))
        self.assertIn("Watchdog timeout", body["value"])


# Kernel wait channels and syscalls of a process blocked in `read -t`: bash
# waits in select/pselect6 (wchan `do_select`), other builds in poll or read.
_READ_WCHANS = ("do_select", "do_sys_poll", "pipe_read", "pipe_wait")
_READ_SYSCALLS = {"x86_64": ("0", "7", "23", "270", "271"), "aarch64": ("63", "72", "73")}


def _blocked_in_read(pid, detail=False):
    """True once `pid` sleeps in a read/select/poll: the event that a shell has
    reached its `read -t` (a fixed sleep guessed it, TV-Z1)."""
    proc = pathlib.Path("/proc/%d" % pid)
    try:
        state = proc.joinpath("stat").read_text().rsplit(")", 1)[1].split()[0]
        wchan = proc.joinpath("wchan").read_text().strip()
        syscall = proc.joinpath("syscall").read_text().split()[0]
    except (OSError, IndexError):
        return (None, None, None) if detail else False
    if detail:
        return state, wchan, syscall
    return state == "S" and (wchan in _READ_WCHANS
                             or syscall in _READ_SYSCALLS.get(os.uname().machine, ()))


class TestLaneGuardAbortAfterStop(unittest.TestCase):
    """Restfenster (i) of tb-s7: a TERM to lane_sync.sh while run_guarded sits in
    `wait "$wp"` -- after the watcher was stopped -- ran `guard_abort`, whose
    `stop_watcher ... 0` found the handshake already read and sent a TERM to the
    watcher's bare PID: the watcher had ended and could be reaped, so the TERM
    hit a free PID or whatever process got it since. The functions run here as
    lane_sync.sh defines them; a shell function stands in for the signal call
    and only prints its arguments."""

    def test_an_abort_after_the_stop_sends_no_second_stop(self):
        text = LANE_SYNC.read_text()
        funcs = "".join(re.findall(r"^(?:stop_watcher|guard_abort)\(\) \{.*?^\}\n", text,
                                   re.M | re.S))
        self.assertEqual(2, funcs.count("() {"), "stop_watcher/guard_abort not found")
        with tempfile.TemporaryDirectory() as tmp:
            script = funcs + textwrap.dedent("""\
                kill() { printf 'signal %s\\n' "$*"; }
                hs="$1/hs"; fired="$1/fired"; tp=""; wp=4242
                mkfifo -m 600 "$hs"; exec {hfd}<>"$hs"; echo ready >&"$hfd"
                stop_watcher "$wp" "$hfd" 1
                echo abort
                guard_abort 143
                """)
            proc = subprocess.run(["bash", "-c", script, "_", tmp], capture_output=True,
                                  text=True, timeout=kit_time.HANG)
        self.assertEqual(143, proc.returncode, proc.stderr)
        before, _, after = proc.stdout.partition("abort\n")
        self.assertEqual("signal -TERM -- -4242\n", before)
        self.assertNotRegex(after, r"(?m)^signal -TERM 4242$",
                            "the abort sent a TERM to the PID of a stopped watcher")
        self.assertEqual("", after)


    def test_an_abort_while_the_stop_waits_for_the_handshake_still_stops(self):
        # Mini-delta review (Important): the flag was set before the wait for
        # the handshake -- a TERM in that wait (the tier ended before the
        # watcher's `ready`) skipped the stop and left the watcher behind.
        text = LANE_SYNC.read_text()
        funcs = "".join(re.findall(r"^(?:stop_watcher|guard_abort)\(\) \{.*?^\}\n", text,
                                   re.M | re.S))
        with tempfile.TemporaryDirectory() as tmp:
            script = funcs + textwrap.dedent("""\
                kill() { printf 'signal %s\\n' "$*"; }
                hs="$1/hs"; fired="$1/fired"; tp=""; wp=4242
                mkfifo -m 600 "$hs"; exec {hfd}<>"$hs"
                trap 'guard_abort 143' TERM
                echo waiting
                stop_watcher "$wp" "$hfd" 20
                echo not-aborted
                """)
            proc = subprocess.Popen(["bash", "-c", script, "_", tmp], stdout=subprocess.PIPE,
                                    stderr=subprocess.PIPE, text=True)
            try:
                self.assertEqual("waiting\n", proc.stdout.readline())
                # the event: the shell blocks in the 20 s read of the handshake
                kit_time.until("the shell waits in the handshake read",
                               lambda: _blocked_in_read(proc.pid),
                               state=lambda: _blocked_in_read(proc.pid, detail=True))
                proc.send_signal(signal.SIGTERM)
                try:
                    out, err = proc.communicate(timeout=kit_time.HANG)
                except subprocess.TimeoutExpired:
                    raise kit_time.Hang("HANG: the aborted stop after %.0f s"
                                        % kit_time.HANG) from None
            finally:
                if proc.poll() is None:
                    proc.kill()
                    proc.wait()
        self.assertEqual(143, proc.returncode, out + err)
        self.assertIn("signal -TERM -- -4242\n", out,
                      "the abort during the handshake wait left the watcher running")


class TestLaneStatusAndFingerprint(LaneTestCase):
    # W1's status line (`lane_sync.sh status`): the inventory columns after
    # fs/free/bare, the fingerprint short form and the state.
    W1_LINE = (r"^reachable  fs \S+  free \d+G  bare %s  deps \d+G  incr \d+G  gen \d+/\d+  "
               r"wt \d+  slots %s  sccache \S+  fp [0-9a-f]{8}  state (idle|busy)$")

    def test_lane_status_reachable_and_unreachable(self):
        self.remote.mkdir()
        self.assertRegex(lane.status(self.a), self.W1_LINE % ("none", 1))
        self.push(cargo=True)
        self.assertRegex(lane.status(self.a), self.W1_LINE % ("1 refs", 1))
        self.assertEqual("unreachable  state off", lane.status(lane.find_lane("lane-dead")))

    def test_lane_status_is_the_line_of_lane_sync(self):
        # One truth: the line is `lane_sync.sh status`, with the lane's
        # settings as `strand.sh` exports them (slots, class, root).
        self.local_root.mkdir()
        self.remote.mkdir()
        wide = dataclasses.replace(self.a, slots=3)
        self.assertRegex(lane.status(wide), self.W1_LINE % ("none", 3))
        env = dict(os.environ, MECLAW_LANE_ROOT=str(self.local_root), MECLAW_LANE_CLASS="egon",
                   MECLAW_LANE_NAME="lane-l")
        res = subprocess.run([str(LANE_SYNC), "status", "local"], env=env, capture_output=True,
                             text=True, stdin=subprocess.DEVNULL)
        volatile = lambda line: re.sub(r"  (free \d+G|state \S+)", "", line.strip())
        self.assertEqual(volatile(res.stdout), volatile(lane.status(self.local)))

    def test_lane_fingerprint_runs_the_script_on_the_lane(self):
        from scripts.pipeline import executor
        doc = dict({k: None for k in executor.FINGERPRINT_FIELDS}, vcpu=24)
        script = self.tmp / "fp.sh"
        body = json.dumps(dict(doc, root="ROOT")).replace('"ROOT"', '"%s"')
        script.write_text("printf '%s\\n' \"$1\"\n" % body)
        fp = lane.fingerprint(self.a, script=str(script))
        self.assertEqual(dict(doc, root=str(self.remote)), fp)
        self.assertIn("sh -s --", self.ssh_log.read_text())
        # re-check tv-bn M7r A-M1: a half answer is no fingerprint (the register keeps its own)
        half = self.tmp / "half.sh"
        half.write_text("printf '%s\\n'\n" % json.dumps({k: v for k, v in doc.items() if k != "python"}))
        with self.assertRaisesRegex(lane.LaneError, "has no python"):
            lane.fingerprint(self.a, script=str(half))
        with self.assertRaises(lane.LaneError) as cm:
            lane.fingerprint(lane.find_lane("lane-dead"), script=str(script))
        self.assertEqual(2, cm.exception.code)


class TestLaneUnitState(LaneTestCase):
    # Z13: a run whose kern died at import (run-696ff322bd86 on build03,
    # ModuleNotFoundError after 10 s) left nothing to fetch; the gate asks the
    # lane whether the run's scope is still there before it keeps the lease.
    RUN = "run-0123456789ab"

    def calls(self):
        path = self.bin / "systemctl.calls"
        return path.read_text().splitlines() if path.exists() else []

    def test_unit_state_asks_is_active_of_the_runs_scope(self):
        for state in ("inactive", "failed", "active"):
            (self.bin / "is-active").write_text(state + "\n")
            self.assertEqual(state, lane.unit_state(self.a, self.RUN))
        self.assertIn("--user is-active pipe-%s.scope" % self.RUN, self.calls())

    def test_unit_state_without_an_answer_is_none(self):
        # The lane fake refuses `is-active` without a state file: no answer.
        self.assertIsNone(lane.unit_state(self.a, self.RUN))

    def test_unit_state_of_an_unreachable_lane_is_none(self):
        (self.bin / "is-active").write_text("inactive\n")
        self.assertIsNone(lane.unit_state(lane.find_lane("lane-dead"), self.RUN))

    def test_unit_state_refuses_a_foreign_run_name(self):
        with self.assertRaises(lane.LaneError):
            lane.unit_state(self.a, "run; systemctl poweroff")
        self.assertEqual([], self.calls())


class TestLaneGuards(LaneTestCase):
    """W1's guards of a run (`lane_sync.sh guard`, shared with `run`)."""

    def refused(self, ln, code, text, **kw):
        with self.assertRaises(lane.LaneError) as cm:
            lane.guard(ln, "kit", **kw)
        self.assertEqual(code, cm.exception.code, str(cm.exception))
        self.assertIn(text, str(cm.exception))

    def test_lane_guard_passes_and_names_the_paths(self):
        os.environ["MECLAW_LANE_FLOOR_G"] = "0"
        self.assertEqual((str(self.remote / "tree"), str(self.remote / "target")),
                         lane.guard(self.a, "kit"))

    def test_lane_guard_local_needs_class_egon_and_egon_runs_no_gate(self):
        os.environ["MECLAW_LANE_FLOOR_G"] = "0"
        plain = dataclasses.replace(self.local, cls="lane")
        self.refused(plain, 5, "a local lane is class egon")
        self.refused(self.local, 2, "runs 'test' only")
        os.environ["MECLAW_LANE_EGON_GATE_OK"] = "test"
        self.addCleanup(os.environ.pop, "MECLAW_LANE_EGON_GATE_OK", None)
        self.assertEqual(str(self.local_root / "tree"), lane.guard(self.local, "kit")[0])

    def test_lane_guard_floor_fails_closed_and_slots_are_granted(self):
        self.remote.mkdir()
        os.environ["MECLAW_LANE_FLOOR_G"] = "999999"
        # W2's hygiene runs first (`lane_sync.sh guard` -> hygiene_before) and
        # refuses the same floor with its own finding (exit 6 -> code 2).
        self.refused(self.a, 2, "ABORTED the run before the gate")
        # Without the hygiene the W1 floor itself fails closed.
        os.environ["MECLAW_HYGIENE"] = "0"
        self.addCleanup(os.environ.pop, "MECLAW_HYGIENE", None)
        self.refused(self.a, 2, "clear old generations (W2)")
        os.environ["MECLAW_LANE_WIPE_OK"] = "test"
        self.addCleanup(os.environ.pop, "MECLAW_LANE_WIPE_OK", None)
        lane.guard(self.a, "kit")
        self.refused(self.a, 5, "slot 1 is not granted", slot=1)

    def test_lane_guard_sets_the_expected_size_of_the_admission(self):
        # The hygiene in front of the floor reserves what THIS run is expected
        # to write (the admission's `expected_g`), never a value the gate's
        # own environment happens to carry.
        self.remote.mkdir()
        os.environ["MECLAW_LANE_FLOOR_G"] = "999999"
        os.environ["MECLAW_LANE_EXPECTED_G"] = "77"
        self.addCleanup(os.environ.pop, "MECLAW_LANE_EXPECTED_G", None)
        with self.assertRaises(lane.LaneError) as cm:
            lane.guard(self.a, "kit", expected_g=7)
        self.assertIn('"reserved_g": 7.0,', str(cm.exception))
        self.assertNotIn('"reserved_g": 77', str(cm.exception))


class TestLaneHold(LaneTestCase):
    """W1 hand-over: push+run on a lane under a lane-side flock on the build
    path, which no token skip bypasses (else a parallel push rewrites the
    sources under a running build -- a false green)."""

    def test_lane_hold_refuses_a_second_holder_of_the_build_path(self):
        os.environ["MECLAW_STRAND_TOKEN_SKIP"] = "parallel"
        self.addCleanup(os.environ.pop, "MECLAW_STRAND_TOKEN_SKIP", None)
        tree = str(self.remote / "tree")
        for ln in (self.a, self.local):
            with self.subTest(transport=ln.transport):
                path = tree if ln is self.a else str(self.local_root / "tree")
                held = lane.hold(ln, path)
                with self.assertRaises(lane.LaneError) as cm:
                    lane.hold(ln, path)
                self.assertEqual(5, cm.exception.code)
                self.assertIn("in use", str(cm.exception))
                held.release()
                with lane.hold(ln, path):
                    pass
                lane.hold(ln, path).release()

    def test_lane_hold_names_its_holder_beside_the_lock(self):
        # Z4: `lane_sync.sh` reads `<lock>.who` for the HOLD nonce -- a hold
        # of the pipeline left the previous holder's nonce standing there.
        for ln in (self.a, self.local):
            with self.subTest(transport=ln.transport):
                path = str(self.remote / "tree") if ln is self.a else str(self.local_root / "tree")
                who = pathlib.Path(path + ".lock.who")
                who.parent.mkdir(parents=True, exist_ok=True)
                who.write_text("stale-nonce")
                with lane.hold(ln, path) as held:
                    self.assertNotEqual("stale-nonce", who.read_text())
                    self.assertEqual(held.nonce, who.read_text())

    def test_lane_hold_of_a_dead_lane_is_code_2(self):
        with self.assertRaises(lane.LaneError) as cm:
            lane.hold(lane.find_lane("lane-dead"), str(self.remote / "tree"))
        self.assertEqual(2, cm.exception.code)


class TestLanePartialBuild(LaneTestCase):
    """GH #1071 after a `test -p`: the run built one package, so the content
    the build path held before is no proof of a build -- the next push gives
    every overlay file the lane's time (the `.gate-partial` mark)."""

    def test_lane_push_after_a_partial_run_freshens_the_overlay(self):
        readme = self.repo / "README.md"
        old = time.time() - 1000
        os.utime(readme, (old, old))
        self.push(cargo=True)
        res = self.push(cargo=True)
        there = self.remote / "tree" / "README.md"
        self.assertAlmostEqual(old, there.stat().st_mtime, delta=1)   # unchanged: carried time
        lane.mark_partial(self.a, res["target"])
        self.assertTrue((self.remote / "target" / ".gate-partial").is_file())
        self.push(cargo=True)
        self.assertGreater(there.stat().st_mtime, old + 500)            # the lane's time
        self.assertFalse((self.remote / "target" / ".gate-partial").exists())
        # the mark is spent: the next push carries the time again
        self.push(cargo=True)
        self.assertAlmostEqual(old, there.stat().st_mtime, delta=1)

    def test_lane_partial_words(self):
        for words, partial in ((["-p", "meclaw-cells"], True), (["--package", "x"], True),
                               (["--package=x"], True), (["-pmeclaw"], True),
                               (["--no-capture"], False), (["--", "-p"], False), ([], False)):
            with self.subTest(words=words):
                self.assertEqual(partial, lane.partial(words))


class TestLaneHostFile(LaneTestCase):
    def setUp(self):
        super().setUp()
        # The merge gate runs these tests with MECLAW_ALLOW_BUMP set
        # (merge.py `run_gate`), and `lane_env` forwards it on purpose
        # (lane_sync.sh: precheck's version-lock reads it on the lane). The
        # expected env below is the one without it; the forwarding has its
        # own test.
        patcher = mock.patch.dict(os.environ)
        patcher.start()
        self.addCleanup(patcher.stop)
        os.environ.pop("MECLAW_ALLOW_BUMP", None)
        # Same for the register id of an owner ruling (testing_bans.py), which
        # a strand or merge gate on a testing-rules change carries.
        os.environ.pop("MECLAW_TESTING_RULES_RULING", None)
        # And every other name of the verdict's list (GH #1210: a gate exports
        # MECLAW_GATE_MODE to its stations, a browser station MECLAW_REQUIRE_BROWSERS).
        for name in ("MECLAW_GATE_MODE", "MECLAW_REQUIRE_BROWSERS"):
            os.environ.pop(name, None)

    def test_lane_load_lanes_format(self):
        lanes = {l.name: l for l in lane.load_lanes()}
        self.assertEqual(["lane-a", "lane-dead", "lane-l"], [l.name for l in lane.load_lanes()])
        a = lanes["lane-a"]
        self.assertEqual(("builder@lane-a.invalid", "bmc-a", str(self.remote), "ssh", "", 8, 0, 1),
                         (a.ssh, a.bmc, a.root, a.transport, a.cls, a.threads, a.jobs, a.slots))
        l = lanes["lane-l"]
        self.assertEqual(("local", "", str(self.local_root), "local", "egon", 4, 6, 1),
                         (l.ssh, l.bmc, l.root, l.transport, l.cls, l.threads, l.jobs, l.slots))
        # an explicit path wins over MECLAW_LANES_FILE; typos cost the word, not the lane
        other = self.tmp / "other-lanes"
        other.write_text("lane-b  b@lane-b.invalid threads=x jobs=0 slots=2 class=odd sccache=1\n"
                         "lonely\n   # nothing\n")
        (b,) = lane.load_lanes(str(other))
        self.assertEqual(("lane-b", 0, 0, 2, "odd", {"sccache": "1"}),
                         (b.name, b.threads, b.jobs, b.slots, b.cls, b.opts))
        self.assertEqual([], lane.load_lanes(str(self.tmp / "missing")))
        # welle-tb S5: the real cores are declared (`cores=`), a typo costs the word
        other.write_text("lane-c  c@lane-c.invalid  cores=6 threads=12\n"
                         "lane-d  d@lane-d.invalid  cores=six\n")
        c, d = lane.load_lanes(str(other))
        self.assertEqual((6, 12, {}), (c.cores, c.threads, c.opts))
        self.assertEqual((0, {}), (d.cores, d.opts))
        self.assertEqual(0, b.cores)
        with self.assertRaises(lane.LaneError):
            lane.find_lane("lane-zz")

    def test_lane_class_has_no_silent_default(self):
        # #1158 B-1: only `class=lane` names a build host; a missing class stays
        # empty and an unknown one keeps its word -- both keep the Egon cap.
        f = self.tmp / "class-lanes"
        f.write_text("a a@a.invalid\nb b@b.invalid class=lane\nc c@c.invalid class=Egon\n"
                     "d local /r class=egon\ne e@e.invalid class=\n")
        self.assertEqual({"a": "", "b": "lane", "c": "Egon", "d": "egon", "e": ""},
                         {ln.name: ln.cls for ln in lane.load_lanes(str(f))})
        self.assertEqual("", lane.Lane(name="x", ssh="x").cls)

    def test_lane_env_for_the_class(self):
        floor = {"MECLAW_GATE_LANE_MIN_FREE_G": "0"}
        # egon: half the cores, never over 4 -- `jobs=6` of the host file no longer
        # lifts nproc 16 over the cap (tb-s4 fix round); a smaller `jobs=` still lowers
        self.assertEqual(dict(floor, MECLAW_LANE_NAME="lane-l", CARGO_INCREMENTAL="1",
                              CARGO_BUILD_JOBS="4", NEXTEST_TEST_THREADS="4"),
                         lane.env_for(self.local, "strand", nproc=16))
        two = lane.env_for(dataclasses.replace(self.local, jobs=2), "strand", nproc=16)
        self.assertEqual(("2", "4"), (two["CARGO_BUILD_JOBS"], two["NEXTEST_TEST_THREADS"]))
        self.assertEqual("4", lane.env_for(self.local, "m0", nproc=8)["CARGO_BUILD_JOBS"])
        self.assertEqual("0", lane.env_for(self.local, "main-pass", nproc=8)["CARGO_INCREMENTAL"])
        self.assertEqual(dict(floor, MECLAW_LANE_NAME="lane-a", NEXTEST_TEST_THREADS="8",
                              CARGO_BUILD_JOBS="8"), lane.env_for(self.a, "integration", nproc=24))
        self.assertEqual("24", lane.env_for(lane.find_lane("lane-dead"), "integration",
                                            nproc=24)["NEXTEST_TEST_THREADS"])

    def test_lane_env_forwards_the_merge_point_bump_allowance(self):
        # The merge gate on a lane gates the numbers the merge point assigned:
        # precheck's version-lock reads MECLAW_ALLOW_BUMP from the run's env,
        # so `lane_env` carries it over -- and only when it is set.
        os.environ["MECLAW_ALLOW_BUMP"] = "merge point of s-x"
        self.assertEqual("merge point of s-x",
                         lane.env_for(self.local, "strand", nproc=16).get("MECLAW_ALLOW_BUMP"))
        self.assertEqual("merge point of s-x",
                         lane.env_for(self.a, "integration", nproc=24).get("MECLAW_ALLOW_BUMP"))
        os.environ.pop("MECLAW_ALLOW_BUMP")
        self.assertNotIn("MECLAW_ALLOW_BUMP", lane.env_for(self.local, "strand", nproc=16))

    def test_lane_env_forwards_the_testing_rules_ruling(self):
        # A change to the testing rules (scripts/testing_bans.py) is GREEN only
        # with the owner ruling's register id: precheck reads
        # MECLAW_TESTING_RULES_RULING from the run's env, so a lane run sees
        # it only when `lane_env` carries it over -- quoted, and only when set
        # (build04 strand gate f81fa08: the started run had it, the lane not).
        os.environ["MECLAW_TESTING_RULES_RULING"] = "OR-TV-7 it's"
        self.assertEqual("OR-TV-7 it's",
                         lane.env_for(self.local, "strand", nproc=16).get("MECLAW_TESTING_RULES_RULING"))
        self.assertEqual("OR-TV-7 it's",
                         lane.env_for(self.a, "integration", nproc=24).get("MECLAW_TESTING_RULES_RULING"))
        os.environ.pop("MECLAW_TESTING_RULES_RULING")
        self.assertNotIn("MECLAW_TESTING_RULES_RULING", lane.env_for(self.local, "strand", nproc=16))

    def test_the_gate_path_runs_a_build_host_at_nproc_and_egon_at_four(self):
        """DIAG dt-zdiag: a build host without `threads=` got no test width
        from `lane_sync.sh env`, so test-tier.sh's egon default of 4 ran the
        tests station on every lane. A build host now runs at its nproc (the
        host's vCPUs, from the fingerprint); class egon stays at 4."""
        box = lane.find_lane("lane-dead")
        for mode in ("strand", "integration", "release", "main-pass"):
            with self.subTest(mode=mode):
                self.assertEqual("24", lane.env_for(box, mode, nproc=24)["NEXTEST_TEST_THREADS"])
                self.assertEqual("4", lane.env_for(self.local, mode, nproc=24)["NEXTEST_TEST_THREADS"])
        # the host expands it on a remote command line (`lane_sync.sh run`)
        env = dict(os.environ, MECLAW_LANE_NAME=box.name, MECLAW_LANE_ROOT=box.root,
                   MECLAW_LANE_THREADS="")
        words = subprocess.run([str(LANE_SYNC), "env", "strand"], env=env, check=True,
                               capture_output=True, text=True).stdout
        self.assertIn("NEXTEST_TEST_THREADS=$(nproc)", shlex.split(words))
        # without a known width the key stays away (test-tier.sh's floor)
        self.assertNotIn("NEXTEST_TEST_THREADS", lane.env_for(box, "strand"))

    def test_no_station_books_fewer_threads_than_it_runs(self):
        """A station that runs at the lane's width (cargo jobs, nextest
        threads) books the whole lane, on a build host at nproc and on egon at the
        capacity admission gives it. GH #1162: `clippy` and the release export
        stations book a side width (CLIPPY_CPU, EXPORT_CPU) and run exactly that
        width, never the lane's. tb-s4 fix round: egon at nproc 4, 6, 8, 12 and 24
        (patched, never the host's own -- build02, nproc 12, booked 6 and ran 4,
        gate bcc315e83)."""
        from scripts.pipeline import executor, plan as plan_mod, schedule
        wide = dict((n, plan_mod.NEEDS[n]["cpu"]) for n in ("build", "tests", "doctests"))
        side = {"clippy": plan_mod.NEEDS["clippy"]["cpu"],
                "export-tree-tests": plan_mod.NEEDS["export-tree-tests"]["cpu"],
                "export-audit (release)": plan_mod.EXPORT_AUDIT_NEEDS["release"]["cpu"]}
        self.assertEqual({"clippy": plan_mod.CLIPPY_CPU, "export-tree-tests": plan_mod.EXPORT_CPU,
                          "export-audit (release)": plan_mod.EXPORT_CPU}, side)
        for ln, vcpu in ((lane.find_lane("lane-dead"), 24), (self.a, 24),
                         *((self.local, n) for n in EGON_NPROC)):
            with self.subTest(lane=ln.name, nproc=vcpu), host_nproc(vcpu):
                env = lane.env_for(ln, "integration", nproc=vcpu)
                width = max(int(env.get("NEXTEST_TEST_THREADS") or vcpu),
                            int(env.get("CARGO_BUILD_JOBS") or vcpu))
                cap = schedule._runner_cpu(ln, {"vcpu": vcpu})
                self.assertLessEqual(width, cap)
                for name, cpu in wide.items():
                    self.assertEqual(cap, plan_mod.resolve_cpu(cpu, cap), name)
                for name, cpu in side.items():
                    booked = plan_mod.resolve_cpu(cpu, cap)
                    self.assertEqual(min(cpu, cap), booked, name)
                    env = {}
                    executor._book_threads(env, {"needs": {"cpu": booked}}, egon=ln.cls == "egon")
                    for key in executor.THREAD_KEYS:
                        self.assertEqual(str(booked), env[key], "%s %s" % (name, key))
                self.assertEqual(cap, plan_mod.resolve_cpu(plan_mod.CPU_LANE, cap))
        # egon: half the cores, never over 4 (mm-os01: nproc 8)
        self.assertEqual([2, 3, 4, 4, 4], [schedule._runner_cpu(self.local, {"vcpu": n})
                                           for n in EGON_NPROC])

    def test_every_station_runs_at_most_the_threads_it_books(self):
        """Re-check Z-I 8591d6fba I-2: no hand list. Every station `build_plan` can
        emit (`build` and the catalogue `gate_plan.STATION_ORDER`, with the needs `build_plan`
        gives it) runs at most the threads it booked -- nextest threads and cargo
        jobs from the executor's station environment, on a build host, a lane with
        `threads=` and on egon (never over 4). `browser:display` booked 4 and ran
        nproc nextest threads; `unwrap-budget` booked 2 and ran clippy at -j nproc.
        Re-check ca3d77311 I-2c: libtest threads too -- `doctests` runs
        `cargo test --doc`, and rustdoc's libtest reads only `RUST_TEST_THREADS`."""
        import importlib.util
        from scripts.pipeline import executor, plan as plan_mod, schedule
        spec = importlib.util.spec_from_file_location("gate_plan_catalogue", GATE_PLAN)
        gp = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(gp)
        # `build` and the stations `split_alone` moves out of a catalogue one
        # (`plan.PARENT`, GH #1191) are the ones build_plan adds
        names = ["build"] + list(gp.STATION_ORDER) + list(plan_mod.PARENT)
        self.assertLessEqual(set(plan_mod.NEEDS), set(names), "a booked station outside the catalogue")
        for ln, vcpu in ((lane.find_lane("lane-dead"), 24), (self.a, 24),
                         *((self.local, n) for n in EGON_NPROC)):
            egon = ln.cls == "egon"
            cap = schedule._runner_cpu(ln, {"vcpu": vcpu})
            for mode in ("strand", "integration", "release"):
                lane_env = lane.env_for(ln, mode, nproc=vcpu)
                for name in names:
                    with self.subTest(lane=ln.name, nproc=vcpu, mode=mode, station=name), \
                            host_nproc(vcpu):
                        needs = plan_mod._needs(name, {"cargo": True}, mode)
                        booked = plan_mod.resolve_cpu(needs["cpu"], cap)
                        run = executor._Run.__new__(executor._Run)
                        run.d = {"+nice": egon}
                        run.base_env = dict(lane_env)
                        run.target, run.lock_file, run.lock_state, run.export_root = \
                            "/nonexistent", "", "absent", ""
                        env = run._env({"name": name, "needs": dict(needs, cpu=booked)})
                        for key in ("NEXTEST_TEST_THREADS", "CARGO_BUILD_JOBS",
                                    "RUST_TEST_THREADS"):
                            # unset = the tool's own default, the host's nproc
                            used = int(env.get(key) or vcpu)
                            self.assertLessEqual(used, booked, key)
                            if egon:
                                self.assertLessEqual(used, 4, "mm-os01: never over 4")

    def test_lane_env_for_is_lane_sync_env_in_its_order(self):
        # One truth: the words of `lane_sync.sh env <mode>` in their order,
        # behind the runner's floor of 0 (`gate_floor_env`, before the width
        # words) -- not for `test`, and not when the wipe is wanted.
        fancy = dataclasses.replace(self.a, opts={"sccache": "1", "linker": "mold"})
        for ln in (self.a, fancy, lane.find_lane("lane-dead")):
            for mode in ("strand", "test", "main-pass", "m0", "integration", "release"):
                with self.subTest(lane=ln.name, opts=ln.opts, mode=mode):
                    env = dict(os.environ, MECLAW_LANE_NAME=ln.name, MECLAW_LANE_ROOT=ln.root,
                               MECLAW_LANE_THREADS=str(ln.threads or ""),
                               MECLAW_LANE_SCCACHE=ln.opts.get("sccache", ""),
                               MECLAW_LANE_LINKER=ln.opts.get("linker", ""))
                    words = subprocess.run([str(LANE_SYNC), "env", mode], env=env, check=True,
                                           capture_output=True, text=True).stdout
                    want = ([] if mode == "test" else ["MECLAW_GATE_LANE_MIN_FREE_G=0"]) + \
                        [w.replace("=$(nproc)", "=24") for w in shlex.split(words)]
                    got = ["%s=%s" % kv for kv in lane.env_for(ln, mode, nproc=24).items()]
                    self.assertEqual(want, got)
        os.environ["MECLAW_LANE_WIPE_OK"] = "test"
        self.addCleanup(os.environ.pop, "MECLAW_LANE_WIPE_OK", None)
        self.assertNotIn("MECLAW_GATE_LANE_MIN_FREE_G", lane.env_for(self.a, "integration"))


class TestWsProcsFailClosed(unittest.TestCase):
    """Re-check tv-kit fix10 M3: `ws_procs` carries the stale-but-busy rule of
    kit find (k) -- every path without a full answer is None ("maybe busy"),
    never [] ("nothing there"). A `ws_procs` "simplified" to [] on an error
    would hand a lane with a live hand run to the next strand. H1/H2/H5 of the
    same re-check: environ, a symlinked root, a non-ASCII digit."""

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = pathlib.Path(tmp.name)
        self.root = self.tmp / "lane-root"
        (self.root / "tree").mkdir(parents=True)
        self.lane = lane.Lane(name="north", ssh="local", transport="local", root=str(self.root))

    def answer(self, argv):
        with mock.patch.object(lane, "_remote_argv", lambda ln, script, long=False: argv):
            return lane.ws_procs(self.lane, str(self.root))

    def test_no_output_and_a_failed_exit_is_no_answer(self):
        self.assertIsNone(self.answer(["sh", "-c", "exit 255"]))

    def test_output_without_the_end_marker_is_no_answer(self):
        self.assertIsNone(self.answer(["sh", "-c", "echo 4242"]))

    def test_a_missing_ssh_binary_is_no_answer(self):
        self.assertIsNone(self.answer([str(self.tmp / "no-such-ssh")]))

    def test_a_probe_past_its_timeout_is_no_answer(self):
        def expired(argv, **kw):
            raise subprocess.TimeoutExpired(argv, kw.get("timeout"))
        with mock.patch.object(lane.subprocess, "run", expired):
            self.assertIsNone(lane.ws_procs(self.lane, str(self.root)))

    def test_the_end_marker_makes_the_list_count_even_on_a_failed_exit(self):
        self.assertEqual([4242], self.answer(
            ["sh", "-c", "echo 4242; echo %s; exit 1" % lane.WS_PROBE_END]))

    def test_a_non_ascii_digit_does_not_crash_the_probe(self):
        # H5: '²'.isdigit() is True and int('²') raises ValueError -- it left
        # `_lane_tree_procs` (which catches LaneError only) and broke the verb.
        self.assertEqual([4242], self.answer(
            ["sh", "-c", "printf '\\302\\262\\n4242\\n%s\\n'" % lane.WS_PROBE_END]))

    def procs(self, root):
        """`ws_procs` of the real probe, which walks every process of this
        host: under the load of a full run (4 shards at nice 19, load 15-17)
        it took up to 19 s (M7l, 20 runs), past a third of its default 30 s,
        and the full run saw one None. The hang marker here is 2 * HANG, and
        no answer is a loud `HANG:`, not a TypeError on None."""
        procs = lane.ws_procs(self.lane, str(root), timeout=2 * kit_time.HANG)
        if procs is None:
            raise kit_time.Hang("HANG: the ws_procs probe of %s gave no answer within %.0f s"
                                % (root, 2 * kit_time.HANG))
        return procs

    def hand_run(self, cwd, env=None):
        proc = subprocess.Popen([sys.executable, "-c", "import sys; sys.stdin.read()"],
                                cwd=str(cwd), stdin=subprocess.PIPE, env=env)

        def stop():
            proc.stdin.close()
            proc.wait()
        self.addCleanup(stop)
        return proc

    def test_ws_kill_signals_only_a_pid_still_in_the_tree(self):
        # Re-check v2-r3-lease M1: between the probe and the kill a pid may
        # end and be reused by a process outside the tree -- `ws_kill` checks
        # each pid against the tree again at the signal, by the probe's rule.
        outside = self.tmp / "src"
        outside.mkdir()
        stranger = self.hand_run(outside)
        inside = self.hand_run(self.root / "tree")
        left = lane.ws_kill(self.lane, str(self.root), [stranger.pid, inside.pid],
                            timeout=2 * kit_time.HANG)
        self.assertIsNotNone(left, "HANG: ws_kill gave no answer")
        kit_time.until("the tree pid ends", lambda: inside.poll() is not None)
        self.assertIsNone(stranger.poll(), "a pid outside the tree was signalled")

    def test_a_target_dir_in_the_environment_is_found(self):
        # H1: the source tree outside the root, only CARGO_TARGET_DIR below it.
        outside = self.tmp / "src"
        outside.mkdir()
        proc = self.hand_run(outside, env=dict(os.environ,
                                               CARGO_TARGET_DIR=str(self.root / "tree" / "target")))
        self.assertIn(proc.pid, self.procs(self.root))

    def test_a_symlinked_root_finds_a_cwd_in_its_target(self):
        # H2: `readlink cwd` is canonical, the root of the host file need not be.
        link = self.tmp / "root-link"
        link.symlink_to(self.root)
        proc = self.hand_run(self.root / "tree")
        self.assertIn(proc.pid, self.procs(link))

    def test_an_empty_root_answers_an_empty_list(self):
        self.assertEqual([], self.procs(self.root))

    def test_a_neighbour_root_with_the_same_prefix_is_not_found(self):
        # re-check tv-kit fix11 H4: the cmdline/environ match was a substring
        # without a boundary -- a local lane root `.../lane1` matched a process
        # of `.../lane10` and held the neighbour lane's lease.
        neighbour = pathlib.Path(str(self.root) + "0")
        (neighbour / "tree").mkdir(parents=True)
        outside = self.tmp / "src"
        outside.mkdir()
        proc = self.hand_run(outside, env=dict(os.environ,
                                               CARGO_TARGET_DIR=str(neighbour / "tree" / "target")))
        self.assertNotIn(proc.pid, self.procs(self.root))
        self.assertIn(proc.pid, self.procs(neighbour))

    def test_the_root_at_the_end_of_a_word_is_found(self):
        # the boundary's other side: `PWD=<root>` and `<root>:<more>` still hold
        outside = self.tmp / "src"
        outside.mkdir()
        for value in (str(self.root), str(self.root) + ":" + str(self.tmp / "other")):
            proc = self.hand_run(outside, env=dict(os.environ, MECLAW_PROBE_SEEN=value))
            self.assertIn(proc.pid, self.procs(self.root), value)


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


class TestLaneSyncVerbs(unittest.TestCase):
    """GH #1156 (OR-PDT.pd-legacy.2): `lane_sync.sh run` started the old
    runner on a build host for `strand.sh gate --host`; that branch is gone,
    and nothing in the kit calls `run` any more. `test` stays: the pipeline's
    `lane.run_test` and `pre.py` call it."""

    def test_run_is_gone_and_test_stays(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = dict(os.environ, MECLAW_LANE_ROOT=tmp)
            env.pop("MECLAW_LANE_CLASS", None)
            res = subprocess.run([str(LANE_SYNC), "run", "local", "s", "strand", "master"],
                                 env=env, capture_output=True, text=True,
                                 stdin=subprocess.DEVNULL, timeout=2 * kit_time.HANG)
            usage = subprocess.run([str(LANE_SYNC)], env=env, capture_output=True, text=True,
                                   stdin=subprocess.DEVNULL, timeout=2 * kit_time.HANG)
        self.assertEqual(2, res.returncode, res.stderr)
        self.assertIn("unknown subcommand: run", res.stderr)
        self.assertNotIn("|run|", usage.stderr)
        self.assertIn("|test|", usage.stderr)
        self.assertNotIn("cmd_run", LANE_SYNC.read_text())


if __name__ == "__main__":
    unittest.main()
