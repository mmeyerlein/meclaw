"""Tests of the merge queue (`scripts/pipeline/merge.py`, W9, GH #1123).

Every case builds a main tree in a temporary directory: a git repository on
`master` with three templates (`c` pins `a`), a scaffold, an example, docs
with a full version and a shorthand, CHANGELOG and ROADMAP -- and, per
strand, a branch `welle-test/<S>` in its own worktree with a pre stamp, a
strand file, a report and a strand receipt under `plans/<wave>/`. The gate is
the fake one (`MECLAW_PIPELINE_FAKE_GATE`), which logs its context; `gh` is a
script that records its argv. The other two W9 modules (versions, revert)
reuse this fixture.
"""

import argparse
import datetime
import io
import json
import os
import pathlib
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS))

from pipeline import merge, pre, store  # noqa: E402
from scripts.tests import kit_tree, store_isolation  # noqa: E402

WAVE = "welle-test-2026-01-01"
SCAFFOLD = "workshop/evals/builder-scenarios/scaffolds/x/main/os/config.json"
EXAMPLE = "examples/demo.json"
RUN_DIR = "20260101T000000Z-%s"

# gate-selftest loads every module into one process: no fixture event of this
# module may reach the metrics target of the caller's environment.
_METRICS_URL = mock.patch.dict(os.environ, {"MECLAW_PIPELINE_METRICS_URL": ""})


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()
    _METRICS_URL.start()


def tearDownModule():  # noqa: N802 -- unittest's name
    _METRICS_URL.stop()
    store_isolation.restore()


def write(root, rel, text):
    path = os.path.join(root, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(text)


def read(root, rel):
    with open(os.path.join(root, rel), encoding="utf-8") as fh:
        return fh.read()


def git(root, *args):
    res = subprocess.run(["git", "-C", root] + list(args), check=True,
                         capture_output=True, text=True)
    return res.stdout.strip()


def template_json(name, version, **extra):
    doc = {"name": name, "version": version}
    doc.update(extra)
    return json.dumps(doc, indent=2) + "\n"


def version_of(root, d):
    return json.loads(read(root, "templates/%s/template.json" % d))["version"]


TREE = {
    ".gitignore": "plans/\n",
    "templates/a/template.json": template_json("a", "1.0.0"),
    "templates/a/README.md": "Template a.\n",
    "templates/b/template.json": template_json("b", "2.0.0"),
    "templates/c/template.json": template_json("c", "0.3.0", uses=["a@1.0.0"]),
    SCAFFOLD: json.dumps({"template": "a@1.0.0"}, indent=2) + "\n",
    EXAMPLE: json.dumps({"cells": [{"template": "a@1.0.0"}, {"template": "b@2.0.0"}]},
                        indent=2) + "\n",
    "docs/x.md": "# X\n\nThe shell runs a@1.0.0 today.\nSince a@1 the ledger is read by message.\n",
    "docs/superpowers/plans/old.md": "Measured on a@1.0.0.\n",
    "workshop/evals/run.md": "Measured at a@1.0.0.\n",
    "docs/other.md": "Other.\n",
    "CHANGELOG.md": "# Changelog\n\n## [Unreleased]\n\n## [0.1.0]\n\n- First.\n",
    "ROADMAP.md": "# Roadmap\n\n## Now\n\n- Strand one (#101)\n- Something else (#999)\n"
                  "\n## Next\n\n- Later (#101)\n",
}


class Fixture:
    """The main tree, its strands and the environment of one test."""

    def __init__(self, gate="green", plans_tracked=False):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = os.path.realpath(self._tmp.name)
        self.root = os.path.join(self.tmp, "main")
        self.state = os.path.join(self.tmp, "state")
        self.gate_log = os.path.join(self.tmp, "gate.jsonl")
        self.gh_log = os.path.join(self.tmp, "gh.jsonl")
        gh = os.path.join(self.tmp, "gh")
        write(self.tmp, "gh", "#!%s\nimport json, sys\nwith open(%r, 'a') as fh:\n"
                              "    fh.write(json.dumps(sys.argv[1:]) + '\\n')\n"
              % (sys.executable, self.gh_log))
        os.chmod(gh, 0o755)
        self._env = mock.patch.dict(os.environ, {
            "GIT_AUTHOR_NAME": "test", "GIT_AUTHOR_EMAIL": "test@example.invalid",
            "GIT_COMMITTER_NAME": "test", "GIT_COMMITTER_EMAIL": "test@example.invalid",
            "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1",
            "XDG_STATE_HOME": self.state,
            "MECLAW_PIPELINE_DB": os.path.join(self.tmp, "p.db"),
            "MECLAW_PIPELINE_FAKE_GATE": gate,
            "MECLAW_PIPELINE_FAKE_GATE_LOG": self.gate_log,
            "MECLAW_MERGE_CORPUS": "skip", "MECLAW_GH": gh,
            "MECLAW_SATELLITE_REPOS": "", "MECLAW_MERGE_GATE_CMD": "",
            # GH #1167 (f): the owner every commit of `master..tip` must carry
            "MECLAW_OWNER": "test <test@example.invalid>",
        })
        self._env.start()
        os.makedirs(self.root)
        git(self.root, "init", "-q", "-b", "master")
        for rel, text in TREE.items():
            write(self.root, rel, "" if plans_tracked and rel == ".gitignore" else text)
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", "init")
        self.first = self.head()
        self.wdir = os.path.join(self.root, "plans", WAVE)
        self.db = store.open(os.path.join(self.tmp, "p.db"))

    def close(self):
        store.close(self.db)
        self._env.stop()
        self._tmp.cleanup()

    # --- git -------------------------------------------------------------------

    def head(self, ref="master"):
        return git(self.root, "rev-parse", ref)

    def parents(self, ref="master"):
        return git(self.root, "rev-list", "--parents", "-n", "1", ref).split()[1:]

    def subject(self, ref="master"):
        return git(self.root, "log", "-1", "--format=%s", ref)

    def body(self, ref="master"):
        return git(self.root, "log", "-1", "--format=%B", ref)

    def master_commit(self, files, msg="foreign"):
        for rel, text in files.items():
            write(self.root, rel, text)
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", msg)
        return self.head()

    # --- strands ---------------------------------------------------------------

    def strand(self, name, issue, files, kind=None, title=None, base="master", commits=1,
               receipt=True, verdict="GREEN", report=True, review=True, stamp=True,
               in_branch=False, extra=None):
        """A strand branch with `commits` commits over `files`; returns its record.

        `in_branch` commits the strand file and the report on the branch (a
        tracked `plans/`) instead of leaving them in the main tree."""
        wt = os.path.join(self.tmp, "wt-%s" % name)
        branch = "welle-test/%s" % name
        git(self.root, "worktree", "add", "-q", "-b", branch, wt, base)
        own = []
        for n in range(commits):
            for rel, text in files.items():
                write(wt, rel, text if n == 0 else text + "round %d\n" % n)
            git(wt, "add", "-A")
            git(wt, "commit", "-q", "-m", "%s: step %d" % (name, n + 1))
            own.append(git(wt, "rev-parse", "HEAD"))
        tip = own[-1]
        wdir = os.path.join(wt, "plans", WAVE) if in_branch else self.wdir
        write(wdir, "straenge/%s.json" % name, json.dumps(dict({
            "strand": name, "issue": issue, "title": title or "Change %s" % name,
            "kind": kind or {}}, **(extra or {}))) + "\n")
        if report:
            self.report(name, branch, issue, [c[:8] for c in own], review=review, wdir=wdir)
        if in_branch:
            git(wt, "add", "-A")
            git(wt, "commit", "-q", "-m", "%s: report" % name)
            tip = git(wt, "rev-parse", "HEAD")
        if stamp:
            self.stamp(wt)
        if receipt:
            self.receipt(name, RUN_DIR % tip[:8], "strand", tip, verdict)
        return {"wt": wt, "branch": branch, "tip": tip, "commits": own}

    def stamp(self, wt):
        pre.write_stamp(wt, {"tree_hash": pre.tree_hash(wt), "verdict": "GREEN",
                             "at": datetime.datetime.now(datetime.timezone.utc)
                             .strftime("%Y-%m-%dT%H:%M:%SZ"),
                             "lane_half": False, "reasons": []})

    def report(self, name, branch, issue, commits, review=True, wdir=None):
        text = ("---\nstrand: %s\nbranch: %s\nissues: [#%s]\nbasis: %s\ngate: GREEN\n"
                "commits: [%s]\n---\n\n# Report %s\n\nDone.\n\nOutsourced findings: none\n"
                % (name, branch, issue, self.first[:8], ", ".join(commits), name))
        if review:
            text += "\n## Review\n\nreviews/S1.md: Verdikt ja.\n"
        write(wdir or self.wdir, "berichte/%s.md" % name, text)

    def receipt(self, name, run, mode, rev, verdict):
        d = os.path.join(self.wdir, "receipts", name, run)
        write(d, "last-%s.json" % mode, json.dumps({"mode": mode, "rev": rev,
                                                     "verdict": verdict}) + "\n")
        write(d, "summary.txt", "GATE-SUMMARY %s %s 3/3 10s %s\n" % (mode, rev[:7], verdict))
        return d

    # --- the operation ---------------------------------------------------------

    def merge(self, names, **kw):
        out, err = io.StringIO(), io.StringIO()
        rc = merge.merge(names, start=self.root, wave=WAVE, db=self.db, out_=out, err=err, **kw)
        return rc, out.getvalue(), err.getvalue()

    def events(self, name):
        return [json.loads(r["data"]) for r in self.db.execute(
            "SELECT data FROM journal WHERE event=? ORDER BY seq", (name,))]

    def state_of(self, strand):
        row = self.db.execute("SELECT state FROM merges WHERE strand=? ORDER BY at DESC, rowid DESC"
                              " LIMIT 1", (strand,)).fetchone()
        return row["state"] if row else None

    def lease(self):
        return store.lease_who(self.db, merge.resource("meclaw"))

    def gate_calls(self):
        try:
            with open(self.gate_log, encoding="utf-8") as fh:
                return [json.loads(line) for line in fh if line.strip()]
        except OSError:
            return []

    def gh_calls(self):
        try:
            with open(self.gh_log, encoding="utf-8") as fh:
                return [json.loads(line) for line in fh if line.strip()]
        except OSError:
            return []


class MergeCase(unittest.TestCase):
    gate = "green"
    plans_tracked = False

    def setUp(self):
        self.f = Fixture(self.gate, self.plans_tracked)
        self.addCleanup(self.f.close)

    def a_strand(self, name="S1", issue=101, **kw):
        return self.f.strand(name, issue, {"templates/a/foo.txt": "foo\n"}, **kw)


class LeaseTest(MergeCase):

    def test_merge_takes_tree_lease_and_queues_second(self):
        f = self.f
        self.a_strand()
        f.strand("S2", 102, {"templates/b/bar.txt": "bar\n"})
        seen = {}
        real = merge.run_gate

        def gate(tree, ctx):
            seen["lease"] = f.lease()
            seen["second"] = f.merge(["S2"])
            seen["s2_state"] = f.state_of("S2")
            return real(tree, ctx)

        with mock.patch.object(merge, "run_gate", side_effect=gate):
            rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        self.assertEqual(seen["lease"]["holder"], "S1")
        self.assertEqual(seen["second"][0], merge.QUEUED_EXIT, seen["second"])
        self.assertEqual(seen["s2_state"], "queued")
        self.assertEqual([e["strand"] for e in f.events("merge_queued")], ["S2"])
        taken = f.events("lease_taken")
        self.assertEqual(taken[0], {"resource": "tree:master@meclaw", "holder": "S1",
                                    "run": None, "ttl_s": merge.TTL_S})
        # The release offers the lease to the waiting merge, no poll wakes it.
        self.assertEqual([e["holder"] for e in f.events("lease_offered")], ["S2"])
        self.assertEqual(f.lease()["offered_to"], "S2")
        self.assertEqual(f.lease()["holder"], "")
        # Somebody else cannot jump the offer.
        self.assertFalse(merge.take_tree(f.db, "meclaw", "S9"))
        rc, _, err = f.merge(["S2"])
        self.assertEqual(rc, 0, err)
        self.assertEqual(f.state_of("S2"), "merged")
        self.assertIsNone(f.lease())
        # The queued row was reused, not duplicated.
        self.assertEqual(f.db.execute("SELECT COUNT(*) FROM merges WHERE strand='S2'")
                         .fetchone()[0], 1)


    def test_free_tree_is_never_blocked_by_a_queue_nobody_wakes(self):
        f = self.f
        self.a_strand()
        base = f.head()
        # Two merges that both queued themselves before either took the tree
        # (parallel callers): no lease row, so no release will ever offer.
        merge._new_merge(f.db, "meclaw", "S9", base, "queued", "2026-01-01T00:00:00Z")
        rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        self.assertEqual(f.state_of("S1"), "merged")

    def test_lapsed_offer_frees_the_tree(self):
        f = self.f
        self.a_strand()
        base = f.head()
        merge._new_merge(f.db, "meclaw", "S9", base, "queued", "2026-01-01T00:00:00Z")
        self.offer("S9")
        self.assertFalse(merge.take_tree(f.db, "meclaw", "S1", at="2026-01-01T00:05:00Z"))
        rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, 0, err)

    def offer(self, strand, at="2026-01-01T00:00:00Z"):
        with store.tx(self.f.db):
            self.f.db.execute("INSERT OR REPLACE INTO leases VALUES (?,?,?,?,?,?)",
                              (merge.resource("meclaw"), "", None, at, merge.OFFER_TTL_S,
                               strand))

    def test_release_offers_with_short_ttl(self):
        f = self.f
        self.a_strand()
        merge._new_merge(f.db, "meclaw", "S9", f.head(), "queued", "2026-01-01T00:00:00Z")
        self.assertTrue(merge.take_tree(f.db, "meclaw", "S1"))
        self.assertTrue(merge.release_tree(f.db, "meclaw", "S1"))
        self.assertEqual(f.lease()["offered_to"], "S9")
        self.assertEqual(f.lease()["ttl_s"], merge.OFFER_TTL_S)
        self.assertLess(merge.OFFER_TTL_S, merge.TTL_S)

    def test_lapsed_offer_abandons_the_queued_row(self):
        f = self.f
        merge._new_merge(f.db, "meclaw", "S9", f.head(), "queued", "2026-01-01T00:00:00Z")
        self.offer("S9")
        late = "2026-01-01T00:%02d:01Z" % (merge.OFFER_TTL_S // 60)
        self.assertTrue(merge.take_tree(f.db, "meclaw", "S1", at=late))
        self.assertEqual(f.state_of("S9"), "abandoned")
        self.assertEqual([e["strand"] for e in f.events("merge_abandoned")], ["S9"])
        # The next release offers nothing to the abandoned row.
        self.assertTrue(merge.release_tree(f.db, "meclaw", "S1"))
        self.assertIsNone(f.lease())

    def test_dequeue_drops_the_row_and_passes_the_offer_on(self):
        f = self.f
        self.a_strand()
        merge._new_merge(f.db, "meclaw", "S9", f.head(), "queued", "2026-01-01T00:00:00Z")
        merge._new_merge(f.db, "meclaw", "S8", f.head(), "queued", "2026-01-01T00:00:01Z")
        self.offer("S9")
        self.assertEqual(merge.dequeue(f.db, "meclaw", "S9"), 1)
        self.assertEqual(f.state_of("S9"), "abandoned")
        self.assertEqual(f.lease()["offered_to"], "S8")
        self.assertEqual(merge.dequeue(f.db, "meclaw", "S8"), 1)
        self.assertIsNone(f.lease())
        self.assertEqual(merge.dequeue(f.db, "meclaw", "S8"), 0)
        rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, 0, err)

    def test_dequeue_on_the_command_line(self):
        f = self.f
        merge._new_merge(f.db, "meclaw", "S9", f.head(), "queued", "2026-01-01T00:00:00Z")
        parser = argparse.ArgumentParser()
        merge.register(parser.add_subparsers())
        args = parser.parse_args(["merge", "--strand", "S9", "--dequeue",
                                  "--db", os.path.join(f.tmp, "p.db")])
        with mock.patch("sys.stdout", new=io.StringIO()) as out:
            self.assertEqual(args.func(args), 0)
        self.assertIn("S9", out.getvalue())
        self.assertEqual(f.state_of("S9"), "abandoned")


    def test_merge_refuses_fast_forward_when_lease_was_lost(self):
        f = self.f
        self.a_strand()
        base = f.head()
        real = merge.run_gate

        def gate(tree, ctx):
            # The lease lapsed and another merge took the tree during the gate.
            with store.tx(f.db):
                f.db.execute("UPDATE leases SET holder='S9' WHERE resource=?",
                             (merge.resource("meclaw"),))
            return real(tree, ctx)

        with mock.patch.object(merge, "run_gate", side_effect=gate):
            rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, merge.REFUSE_EXIT)
        self.assertIn("lease_lost", err)
        self.assertEqual(f.head(), base)
        self.assertEqual(f.lease()["holder"], "S9")

    def test_hold_tree_renews_the_lease(self):
        f = self.f
        self.assertTrue(merge.take_tree(f.db, "meclaw", "S1", at="2026-01-01T00:00:00Z"))
        merge.hold_tree(f.db, "meclaw", "S1", at="2026-01-01T01:00:00Z")
        self.assertEqual(f.lease()["taken_at"], "2026-01-01T01:00:00Z")
        with self.assertRaises(merge.Refused):
            merge.hold_tree(f.db, "meclaw", "S1", at="2026-01-01T03:00:01Z")

    def test_sigterm_during_the_gate_releases_the_lease(self):
        f = self.f
        self.a_strand()
        base = f.head()

        def gate(tree, ctx):
            os.kill(os.getpid(), signal.SIGTERM)
            raise AssertionError("SIGTERM did not end the run")

        before = signal.getsignal(signal.SIGTERM)
        with mock.patch.object(merge, "run_gate", side_effect=gate):
            with self.assertRaises(SystemExit):
                f.merge(["S1"])
        self.assertIsNone(f.lease())
        self.assertEqual(f.head(), base)
        self.assertIs(signal.getsignal(signal.SIGTERM), before)

    def test_second_merge_of_the_same_strand_is_refused(self):
        f = self.f
        self.a_strand()
        f.strand("S2", 102, {"templates/b/bar.txt": "bar\n"})
        seen = {}
        real = merge.run_gate

        def gate(tree, ctx):
            seen["second"] = f.merge(["S1"])
            seen["batch"] = f.merge(["S1", "S2"])
            seen["lease"] = f.lease()
            return real(tree, ctx)

        with mock.patch.object(merge, "run_gate", side_effect=gate):
            rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        self.assertEqual(seen["second"][0], merge.REFUSE_EXIT, seen["second"])
        self.assertIn("already_merging", seen["second"][2])
        self.assertIn("already_merging", seen["batch"][2])
        # The refused call did not release the running merge's lease.
        self.assertEqual(seen["lease"]["holder"], "S1")
        self.assertEqual([e["strand"] for e in f.events("merge_done")], ["S1"])
        self.assertIsNone(f.lease())

    def test_second_merge_before_gating_leaves_the_running_row_alone(self):
        # N2: before 'gating' the running merge's row is still 'queued'; the
        # second call must not take it over and journal it as refused.
        f = self.f
        self.a_strand()
        seen = {}
        real = merge.preconditions

        def pre(s, tip, no_pre):
            seen["second"] = f.merge(["S1"])
            return real(s, tip, no_pre)

        with mock.patch.object(merge, "preconditions", side_effect=pre):
            rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        self.assertEqual(seen["second"][0], merge.REFUSE_EXIT, seen["second"])
        self.assertIn("already_merging", seen["second"][2])
        done = [e["merge"] for e in f.events("merge_done")]
        self.assertEqual(len(done), 1, done)
        refused = [e["merge"] for e in f.events("merge_refused")]
        self.assertNotIn(done[0], refused)
        states = [r["state"] for r in f.db.execute(
            "SELECT state FROM merges WHERE id=?", (done[0],))]
        self.assertEqual(states, ["merged"])

    def test_merge_reads_base_under_the_lease(self):
        f = self.f
        s = self.a_strand()
        real = merge.take_tree
        moved = {}

        def take(db, repo, holder, at=None):
            # Another merge fast-forwards master just before this one gets the tree.
            moved["sha"] = f.master_commit({"docs/other.md": "Moved under us.\n"})
            return real(db, repo, holder, at)

        with mock.patch.object(merge, "take_tree", side_effect=take):
            rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        self.assertEqual(f.parents(), [moved["sha"], s["tip"]])


class PreconditionTest(MergeCase):

    def assert_refused(self, reason, strand="S1"):
        base = self.f.head()
        rc, _, err = self.f.merge([strand])
        self.assertEqual(rc, merge.REFUSE_EXIT, err)
        self.assertIn(reason, err)
        self.assertEqual(self.f.state_of(strand), "refused")
        self.assertEqual(self.f.events("merge_refused")[-1]["reason"], reason)
        self.assertIsNone(self.f.lease())
        self.assertEqual(self.f.head(), base)
        self.assertEqual(self.f.gate_calls(), [])

    def test_merge_refuses_without_green_strand_receipt_on_current_base(self):
        s = self.a_strand(receipt=False)
        self.assert_refused("no_strand_receipt")
        self.f.receipt("S1", RUN_DIR % s["tip"][:8], "strand", s["tip"], "RED")
        self.assert_refused("strand_gate_not_green")
        # A green receipt, but the branch moved on after it.
        self.f.receipt("S1", "20260102T000000Z-%s" % s["tip"][:8], "strand", s["tip"], "GREEN")
        write(s["wt"], "templates/a/foo.txt", "later\n")
        git(s["wt"], "commit", "-q", "-am", "S1: after the gate")
        self.f.stamp(s["wt"])
        self.assert_refused("receipt_not_branch_head")

    def test_merge_refuses_stale_pre_stamp(self):
        s = self.a_strand(stamp=False)
        self.assert_refused("pre_stamp")
        self.f.stamp(s["wt"])
        write(s["wt"], "templates/a/untracked.txt", "dirty\n")
        self.assert_refused("pre_stamp")
        rc, _, err = self.f.merge(["S1"], no_pre="test")
        self.assertEqual(rc, 0, err)

    def test_merge_refuses_without_report_and_review(self):
        self.a_strand(report=False)
        self.assert_refused("no_report")
        self.f.report("S1", "welle-test/S1", 101, [], review=False)
        self.assert_refused("no_review")
        self.f.report("S1", "welle-test/S1", 101, [], review=True)
        rc, _, err = self.f.merge(["S1"])
        self.assertEqual(rc, 0, err)


    def test_merge_refuses_a_branch_that_touches_the_testing_register(self):
        # VORGABEN § 5.4: the register is the owner's; a strand never carries it.
        self.f.strand("S1", 101, {".config/testing-rulings.toml": "[[ruling]]\n"})
        self.assert_refused("testing_register")

    def test_merge_refuses_when_the_register_diff_fails(self):
        # Re-check tv-p0 B1: a failing `git diff` read as an empty diff, so the
        # register check passed (fail-open). It is a refusal now.
        self.a_strand()
        real = merge.git

        def failing(cwd, *args, **kw):
            if merge.TESTING_REGISTER in args:
                return subprocess.CompletedProcess(args, 128, "", "fatal: bad revision")
            return real(cwd, *args, **kw)

        with mock.patch.object(merge, "git", failing):
            self.assert_refused("testing_register")

    def set_review(self, body):
        path = os.path.join(self.f.wdir, "berichte", "S1.md")
        with open(path, encoding="utf-8") as fh:
            text = fh.read().split("\n## Review\n", 1)[0]
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(text + "\n## Review\n" + body)

    def test_merge_refuses_an_empty_review_section(self):
        # Review Z-I gap I1: `strand.sh new` writes `## Review` empty; the
        # heading alone is no review.
        self.a_strand()
        self.set_review("\n")
        self.assert_refused("no_review_verdict")
        self.set_review("\n(Verdikt-Zeile folgt)\n")
        self.assert_refused("no_review_verdict")
        # The last verdict line decides: a later `nein` beats an earlier `ja`.
        self.set_review("\nreviews/a.md: Verdikt ja\nreviews/b.md: Verdikt nein (I1)\n")
        self.assert_refused("no_review_verdict")
        self.set_review("\n- Verdikt: ja (reviews-cc-xhigh/recheck.md)\n\n## Offene Punkte\n\nnone\n")
        rc, _, err = self.f.merge(["S1"])
        self.assertEqual(rc, 0, err)

    def set_outsourced(self, line):
        path = os.path.join(self.f.wdir, "berichte", "S1.md")
        with open(path, encoding="utf-8") as fh:
            text = re.sub(r"(?m)^.*Outsourced findings:.*\n", "", fh.read())
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(text.replace("Done.\n", "Done.\n\n%s\n" % line if line else "Done.\n"))

    def test_merge_refuses_a_report_without_the_outsourced_findings_field(self):
        # GH #1196: the strand report says what it moved into issues -- nothing,
        # or each issue with the owner ruling that allows it.
        self.a_strand()
        self.set_outsourced("")
        self.assert_refused("no_outsourced_findings")
        # an issue without an owner-ruling reference, the scaffold's placeholder
        # and a bare reference placeholder are refused as well
        for line in ("Outsourced findings: #120",
                     "Outsourced findings: #120 (investigation)",
                     "Outsourced findings: #120 (owner-ruling)",
                     "Outsourced findings: #120 (owner-ruling <reference>)",
                     "Outsourced findings: <none | #<n> (owner-ruling <reference>)>"):
            self.set_outsourced(line)
            self.assert_refused("no_outsourced_findings")
        self.set_outsourced("Outsourced findings: #120 (owner-ruling ledger R-7), "
                            "#121 (owner-ruling R-8)")
        rc, _, err = self.f.merge(["S1"])
        self.assertEqual(rc, 0, err)

    def test_merge_takes_outsourced_findings_none(self):
        self.a_strand()
        self.set_outsourced("**Outsourced findings:** none")
        rc, _, err = self.f.merge(["S1"])
        self.assertEqual(rc, 0, err)

    def test_k_r3_the_review_refuses_before_anything_that_costs(self):
        # K-R3: the verdict refusal came after the reader rules of the
        # merge preparation (16 min) and the merge gate's plan -- a gate
        # cycle lost. Now nothing that costs runs before it.
        costly = mock.Mock(side_effect=AssertionError("ran before the review check"))
        patches = (mock.patch.object(merge, "plan_only", costly),
                   mock.patch.object(merge, "open_migrations", costly),
                   mock.patch.object(merge.turnaround, "offending", costly),
                   mock.patch.object(merge.turnaround, "code_equal_revs", costly))
        self.a_strand()
        self.set_review("\nVerdikt: nein\n")
        for p in patches:
            p.start()
        try:
            self.assert_refused("no_review_verdict")
            self.f.report("S1", "welle-test/S1", 101, [], review=False)
            self.assert_refused("no_review")
        finally:
            for p in patches:
                p.stop()
        costly.assert_not_called()
        self.f.report("S1", "welle-test/S1", 101, [], review=True)
        rc, _, err = self.f.merge(["S1"])
        self.assertEqual(rc, 0, err)


BANS_TOML = (SCRIPTS.parent / ".config" / "testing-bans.toml").read_text(encoding="utf-8")
LIFT = "crates/x/tests/gh_lift.rs"
# A test file in the old format: a 300 ms absence window (rs-short-deadline and
# rs-neg-window on line 7) -- the remainder gh850 carried into its merge (V2-A6).
WINDOW_RS = ("use std::time::Duration;\nuse tokio::time::timeout;\n\n"
             "#[tokio::test]\nasync fn nothing_arrives() {\n"
             "    let (_tx, mut rx) = tokio::sync::mpsc::channel::<u8>(1);\n"
             "    assert!(timeout(Duration::from_millis(300), rx.recv()).await.is_err());\n}\n")
CLEAN_RS = ("#[tokio::test]\nasync fn nothing_arrives() {\n"
            "    let (tx, mut rx) = tokio::sync::mpsc::channel::<u8>(1);\n"
            "    tx.send(1).await.unwrap();\n    assert_eq!(rx.recv().await, Some(1));\n}\n")
LEGACY_ENTRY = ('[[flaky]]\ntest = "x::gh_lift::nothing_arrives"\nissue = 850\n'
                'cause = "async wait"\n')


class LiftedFileBansTest(MergeCase):
    """V2-A6 (GH #1199, K9): a file a strand claims as lifted (AGENTS.md rule
    11a) or FLAKY-fixed meets the testing bans over the WHOLE file, or the
    merge refuses with the hits -- the precheck sees added lines only, and
    gh850 merged as lifted with a 300 ms window left in an unchanged line."""

    def setUp(self):
        super().setUp()
        # master carries the bans and the old-format file; the strand changes
        # only its first line, so the window stands in an unchanged line.
        self.f.master_commit({".config/testing-bans.toml": BANS_TOML,
                              "crates/x/Cargo.toml": '[package]\nname = "x"\n',
                              LIFT: WINDOW_RS}, msg="old format")

    def assert_lift_refused(self, strand="S1"):
        base = self.f.head()
        rc, _, err = self.f.merge([strand])
        self.assertEqual(rc, merge.REFUSE_EXIT, err)
        self.assertIn("lifted_bans", err)
        self.assertIn("%s:7 rs-short-deadline" % LIFT, err)
        self.assertIn("%s:7 rs-neg-window" % LIFT, err)
        self.assertEqual(self.f.events("merge_refused")[-1]["reason"], "lifted_bans")
        self.assertEqual(self.f.head(), base)
        self.assertEqual(self.f.gate_calls(), [])

    def test_a_lift_named_in_the_strand_file_that_keeps_a_300ms_window_is_refused(self):
        self.f.strand("S1", 101, {LIFT: "// lifted\n" + WINDOW_RS.split("\n", 1)[1]},
                      extra={"lifted": [LIFT]})
        self.assert_lift_refused()

    def test_a_lift_named_in_the_report_head_is_refused_too(self):
        self.f.strand("S1", 101, {LIFT: "// lifted\n" + WINDOW_RS.split("\n", 1)[1]})
        path = os.path.join(self.f.wdir, "berichte", "S1.md")
        with open(path, encoding="utf-8") as fh:
            text = fh.read()
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(text.replace("\ngate: GREEN\n", "\ngate: GREEN\nlifted: [%s]\n" % LIFT, 1))
        self.assert_lift_refused()

    def test_a_flaky_fix_strand_checks_every_test_file_it_changes(self):
        self.f.strand("S1", 101, {LIFT: "// fixed\n" + WINDOW_RS.split("\n", 1)[1]},
                      extra={"flaky_fix": True})
        self.assert_lift_refused()

    def test_a_test_dropped_from_the_legacy_list_makes_its_file_lifted(self):
        # (b) holds without any declaration: the diff drops the entry.
        self.f.master_commit({".config/flaky-legacy.toml": LEGACY_ENTRY}, msg="legacy")
        self.f.strand("S1", 101, {".config/flaky-legacy.toml": "# emptied\n"})
        self.assert_lift_refused()

    def test_a_clean_lift_merges(self):
        self.f.master_commit({".config/flaky-legacy.toml": LEGACY_ENTRY}, msg="legacy")
        self.f.strand("S1", 101, {LIFT: CLEAN_RS, ".config/flaky-legacy.toml": "# emptied\n"},
                      extra={"lifted": [LIFT], "flaky_fix": True})
        rc, _, err = self.f.merge(["S1"])
        self.assertEqual(rc, 0, err)

    def test_a_strand_that_lifts_nothing_is_not_graded_over_whole_files(self):
        # Only lifted files are graded whole; the old window elsewhere stays the
        # precheck's business (added lines).
        self.f.strand("S1", 101, {LIFT: "// touched\n" + WINDOW_RS.split("\n", 1)[1]})
        rc, _, err = self.f.merge(["S1"])
        self.assertEqual(rc, 0, err)

    def test_a_declared_lifted_path_missing_at_the_tip_is_refused(self):
        # M5 of the v2-fix-b re-check: a misspelt `lifted` path fell out
        # without a hit or a note, and the merge went through ungraded.
        typo = "crates/x/tests/gh_lfit.rs"
        self.f.strand("S1", 101, {LIFT: CLEAN_RS}, extra={"lifted": [typo]})
        base = self.f.head()
        rc, _, err = self.f.merge(["S1"])
        self.assertEqual(rc, merge.REFUSE_EXIT, err)
        self.assertIn("lifted_bans", err)
        self.assertIn("%s: declared lifted, missing at the tip" % typo, err)
        self.assertEqual(self.f.head(), base)

    def test_a_declared_lifted_file_the_diff_deletes_is_a_note(self):
        # Deleted in base..tip: nothing to grade, said as a note, no refusal.
        self.f.master_commit({"crates/x/tests/gone.rs": CLEAN_RS}, msg="to delete")
        base = self.f.head()
        s = self.f.strand("S1", 101, {LIFT: CLEAN_RS}, extra={"lifted": ["crates/x/tests/gone.rs"]})
        git(s["wt"], "rm", "-q", "crates/x/tests/gone.rs")
        git(s["wt"], "commit", "-q", "-m", "S1: delete")
        tip = git(s["wt"], "rev-parse", "HEAD")
        found, paths, notes = merge.lifted.check(self.f.root, base, tip,
                                                 {"lifted": ["crates/x/tests/gone.rs"]}, {})
        self.assertEqual((found, paths), ([], []))
        self.assertEqual(notes, ["crates/x/tests/gone.rs: declared lifted, deleted in the diff"])


class LiftedResolutionTest(MergeCase):
    """V2-A6: how `pipeline.lifted` finds the file of a dropped legacy test and
    reads the declaration -- in the fixture tree, never the real one."""

    def setUp(self):
        super().setUp()
        self.rev = self.f.master_commit({
            "crates/x/Cargo.toml": '[package]\nname = "x"\n',
            "crates/x/tests/h/gh_a.rs": "#[test]\nfn same_name() {}\n",
            "crates/x/tests/h/gh_b.rs": "#[test]\nfn same_name() {}\n",
            "crates/x/tests/other.rs": "#[test]\nfn same_name() {}\n",
            "scripts/tests/test_k.py": "def test_y(): pass\n"}, msg="tests")

    def test_a_nextest_name_resolves_to_the_file_of_its_harness_and_stem(self):
        files = merge.lifted.files_of_test
        self.assertEqual(files(self.f.root, self.rev, "x::h gh_a::same_name"),
                         ["crates/x/tests/h/gh_a.rs"])
        self.assertEqual(files(self.f.root, self.rev, "x::other::same_name"),
                         ["crates/x/tests/other.rs"])
        self.assertEqual(files(self.f.root, self.rev, "nope::h::same_name"), [])

    def test_a_python_id_resolves_to_its_module(self):
        files = merge.lifted.files_of_test
        self.assertEqual(files(self.f.root, self.rev, "scripts/tests/test_k.py::test_y"),
                         ["scripts/tests/test_k.py"])
        self.assertEqual(files(self.f.root, self.rev, "test_k.Case.test_y"),
                         ["scripts/tests/test_k.py"])

    def test_the_declaration_reads_strand_file_and_report_head(self):
        declared = merge.lifted.declared
        self.assertEqual(declared({"lifted": ["a.rs"]}, {"lifted": "[b.rs, a.rs]"}),
                         (["a.rs", "b.rs"], False))
        self.assertEqual(declared({}, {"flaky_fix": "true"}), ([], True))
        with self.assertRaisesRegex(ValueError, "lifted"):
            declared({"lifted": "a.rs"}, None)
        with self.assertRaisesRegex(ValueError, "flaky_fix"):
            declared({"flaky_fix": "yes"}, None)


PAUSED_HELPER_RS = (
    "use std::time::Duration;\nuse tokio::time::sleep;\n\n"
    "async fn tick(period: Duration) {\n    sleep(period).await;\n}\n\n"
    "async fn twice(period: Duration) {\n    tick(period).await;\n    tick(period).await;\n}\n\n"
    "#[tokio::test(start_paused = true)]\nasync fn a() {\n    tick(Duration::from_millis(10)).await;\n}\n\n"
    "#[tokio::test(start_paused = true)]\nasync fn b() {\n    twice(Duration::from_millis(10)).await;\n}\n")
BOOT_NAME_RS = (
    "const DB_FILE: &str = \"colony.db\";\n\n"
    "fn boot(db_file: &std::path::Path) {\n"
    "    let db = ColonyDb::open(db_file).expect(\"the colony opens its database\");\n"
    "    start(db);\n}\n\n"
    "#[test]\nfn boots() {\n    let db_file = dir().join(DB_FILE);\n    boot(&db_file);\n}\n")
# I1 of the v2-fix-b re-check (`lifted_precision_leak.py`): every fixture boots
# a colony on `"colony.db"` (line 2); the boot name is a hit whether or not the
# test then reads the database (F2: the boot-name exception is withdrawn).
BOOT_HEAD_RS = ("use meclaw_colony::ColonyDb;\nconst DB_FILE: &str = \"colony.db\";\n"
                "fn boot(_db: ColonyDb) {}\n")
BOOT_ONLY_RS = BOOT_HEAD_RS + (
    "#[tokio::test]\nasync fn boots() {\n    let td = tempfile::tempdir().unwrap();\n"
    "    let db = ColonyDb::open(&td.path().join(DB_FILE)).expect(\"the colony opens its database\");\n"
    "    boot(db);\n}\n")
BOOT_RUSQLITE_RS = BOOT_HEAD_RS + (
    "#[tokio::test]\nasync fn reads() {\n    let td = tempfile::tempdir().unwrap();\n"
    "    let p = td.path().join(DB_FILE);\n    let db = ColonyDb::open(&p).expect(\"boot\");\n"
    "    boot(db);\n    let _c = rusqlite::Connection::open(&p).unwrap();\n}\n")
BOOT_HELPER_READS_RS = BOOT_HEAD_RS + (
    "fn dead(c: &ColonyDb) -> usize {\n    c.read_all_dead_letters().unwrap().len()\n}\n"
    "#[tokio::test]\nasync fn reads_the_dlq_through_a_helper() {\n"
    "    let td = tempfile::tempdir().unwrap();\n"
    "    let db = ColonyDb::open(&td.path().join(DB_FILE)).expect(\"boot\");\n"
    "    assert_eq!(dead(&db), 0);\n    boot(db);\n}\n")
BOOT_OPENOPTIONS_RS = BOOT_HEAD_RS + (
    "use std::io::Read;\n#[tokio::test]\nasync fn reads_the_file() {\n"
    "    let td = tempfile::tempdir().unwrap();\n    let p = td.path().join(DB_FILE);\n"
    "    let db = ColonyDb::open(&p).expect(\"boot\");\n    boot(db);\n"
    "    let mut raw = Vec::new();\n"
    "    std::fs::OpenOptions::new().read(true).open(&p).unwrap().read_to_end(&mut raw).unwrap();\n"
    "    assert!(!raw.is_empty());\n}\n")
BOOT_SQLITE3_RS = BOOT_HEAD_RS + (
    "#[tokio::test]\nasync fn asks_sqlite3() {\n    let td = tempfile::tempdir().unwrap();\n"
    "    let p = td.path().join(DB_FILE);\n    let db = ColonyDb::open(&p).expect(\"boot\");\n"
    "    boot(db);\n    let out = std::process::Command::new(\"sqlite3\").arg(&p)\n"
    "        .arg(\"select count(*) from mailbox_overflow\").output().unwrap();\n"
    "    assert!(out.status.success());\n}\n")


class LiftedRefinementTest(MergeCase):
    """OR-V2.v2-fix-b.G.1: one precision of the whole-file view of
    `pipeline.lifted` -- a helper whose every caller in the file runs paused
    runs on the virtual clock. The `colony.db` file name is a hit in every
    form, a boot included (F2: OR-G.3/F1.1 withdrawn, TV-A3 as the precheck
    judges it)."""

    def grade(self, files):
        files = dict(files, **{".config/testing-bans.toml": BANS_TOML})
        rev = self.f.master_commit(files, msg="graded")
        return merge.lifted.hits(self.f.root, rev, sorted(p for p in files if p.endswith(".rs")))

    def test_a_sleep_in_a_helper_whose_every_caller_runs_paused_is_no_hit(self):
        self.assertEqual(self.grade({LIFT: PAUSED_HELPER_RS}), [])

    def test_a_sleep_in_a_helper_with_one_real_time_caller_stays_a_hit(self):
        text = PAUSED_HELPER_RS + "\n#[tokio::test]\nasync fn c() {\n    twice(Duration::from_millis(10)).await;\n}\n"
        self.assertEqual(self.grade({LIFT: text}), ["%s:5 rs-tokio-sleep" % LIFT])

    def test_a_sleep_in_a_public_helper_stays_a_hit(self):
        text = PAUSED_HELPER_RS.replace("async fn tick", "pub async fn tick")
        self.assertEqual(self.grade({LIFT: text}), ["%s:5 rs-tokio-sleep" % LIFT])

    def test_a_helper_no_one_calls_stays_a_hit(self):
        text = "use tokio::time::sleep;\n\nasync fn idle() {\n    sleep(std::time::Duration::from_millis(5)).await;\n}\n"
        self.assertEqual(self.grade({LIFT: text}), ["%s:4 rs-tokio-sleep" % LIFT])

    def test_a_colony_db_name_that_only_boots_the_colony_is_a_hit(self):
        self.assertEqual(self.grade({LIFT: BOOT_NAME_RS}), ["%s:1 rs-colony-db" % LIFT])

    def test_a_colony_db_name_the_test_also_reads_stays_a_hit(self):
        text = BOOT_NAME_RS + (
            "\n#[test]\nfn reads() {\n"
            "    let conn = rusqlite::Connection::open(dir().join(DB_FILE)).unwrap();\n"
            "    drop(conn);\n}\n")
        self.assertEqual(self.grade({LIFT: text}), ["%s:1 rs-colony-db" % LIFT])

    def test_a_colony_db_the_test_queries_through_colony_db_stays_a_hit(self):
        text = BOOT_NAME_RS.replace("    start(db);\n", "    db.load_overflow();\n")
        self.assertEqual(self.grade({LIFT: text}), ["%s:1 rs-colony-db" % LIFT])

    def test_i1_a_boot_on_the_colony_db_constant_alone_is_a_hit(self):
        self.assertEqual(self.grade({LIFT: BOOT_ONLY_RS}), ["%s:2 rs-colony-db" % LIFT])

    def test_i1_a_rusqlite_reader_beside_the_boot_stays_a_hit(self):
        self.assertEqual(self.grade({LIFT: BOOT_RUSQLITE_RS}), ["%s:2 rs-colony-db" % LIFT])

    def test_i1_a_reader_method_through_a_helper_stays_a_hit(self):
        self.assertEqual(self.grade({LIFT: BOOT_HELPER_READS_RS}), ["%s:2 rs-colony-db" % LIFT])

    def test_i1_an_open_options_reader_of_the_booted_path_stays_a_hit(self):
        self.assertEqual(self.grade({LIFT: BOOT_OPENOPTIONS_RS}), ["%s:2 rs-colony-db" % LIFT])

    def test_i1_a_sqlite3_cli_on_the_booted_path_stays_a_hit(self):
        self.assertEqual(self.grade({LIFT: BOOT_SQLITE3_RS}), ["%s:2 rs-colony-db" % LIFT])

    def test_a_log_name_beside_a_boot_is_a_hit_as_well(self):
        text = BOOT_NAME_RS + "\nconst LOG: &str = \"message_log\";\n"
        self.assertEqual(self.grade({LIFT: text}),
                         ["%s:1 rs-colony-db" % LIFT, "%s:14 rs-colony-db" % LIFT])


class MigrationQueueTest(MergeCase):
    """OR-DT.store-migrations: a strand names its store migrations; two open
    strands never carry the same number, and the smaller number merges first."""

    def migrating(self, name, nums):
        return self.f.strand(name, 100, {"templates/a/%s.txt" % name: "x\n"},
                             extra={"migrations": nums})

    def refused(self, strand, reason):
        rc, _, err = self.f.merge([strand])
        self.assertEqual(rc, merge.REFUSE_EXIT, err)
        self.assertIn(reason, err)
        self.assertEqual(self.f.events("merge_refused")[-1]["reason"], reason)
        self.assertEqual(self.f.gate_calls(), [])

    def test_two_open_strands_with_the_same_number_are_refused(self):
        self.migrating("S1", [4])
        self.migrating("S2", [4, 5])
        self.refused("S1", "migration_collision")
        self.refused("S2", "migration_collision")

    def test_the_smaller_number_merges_first(self):
        self.migrating("S1", [5])
        self.migrating("S2", [4])
        self.a_strand("S3")
        self.refused("S1", "migration_order")
        rc, _, err = self.f.merge(["S3"])
        self.assertEqual(rc, 0, err)
        rc, _, err = self.f.merge(["S2"])
        self.assertEqual(rc, 0, err)
        rc, _, err = self.f.merge(["S1"])
        self.assertEqual(rc, 0, err)

    def test_a_batch_merges_in_number_order(self):
        self.migrating("S1", [5])
        self.migrating("S2", [4])
        rc, _, err = self.f.merge(["S2", "S1"])
        self.assertEqual(rc, 0, err)

    def test_a_broken_migrations_field_is_refused(self):
        self.migrating("S1", ["two"])
        rc, _, err = self.f.merge(["S1"])
        self.assertEqual(rc, merge.REFUSE_EXIT, err)
        self.assertIn("strand_file_invalid", err)

    def test_open_migrations_skips_merged_branches(self):
        self.migrating("S1", [4])
        self.assertEqual(merge.open_migrations(self.f.root, "master"), {"S1": [4]})
        rc, _, err = self.f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        self.assertEqual(merge.open_migrations(self.f.root, "master"), {})


class CorpusTest(unittest.TestCase):
    """M4: a failed corpus regeneration refuses, and the result is re-checked."""

    TOOL = ("import os, sys\n"
            "mark = os.path.join(os.path.dirname(__file__), 'regenerated')\n"
            "if sys.argv[1:] == ['--check']:\n"
            "    sys.exit(0 if %s and os.path.exists(mark) else 1)\n"
            "open(mark, 'w').close()\n"
            "sys.exit(%d)\n")

    def tree(self, check_after, regen_exit):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        git(tmp.name, "init", "-q")
        write(tmp.name, merge.CORPUS_TOOL, self.TOOL % (check_after, regen_exit))
        env = mock.patch.dict(os.environ, {"MECLAW_MERGE_CORPUS": ""})
        env.start()
        self.addCleanup(env.stop)
        return tmp.name

    def test_failed_regeneration_refuses(self):
        with self.assertRaises(merge.Refused) as ctx:
            merge.regenerate_corpus(self.tree("True", 1))
        self.assertEqual(ctx.exception.reason, "corpus_failed")

    def test_regeneration_that_stays_stale_refuses(self):
        with self.assertRaises(merge.Refused) as ctx:
            merge.regenerate_corpus(self.tree("False", 0))
        self.assertEqual(ctx.exception.reason, "corpus_failed")

    def test_good_regeneration_passes(self):
        self.assertEqual(merge.regenerate_corpus(self.tree("True", 0)), [])


class ReceiptVerdictTest(MergeCase):

    def test_merge_refuses_receipt_red_with_green_summary(self):
        f = self.f
        s = self.a_strand(receipt=False)
        d = f.receipt("S1", RUN_DIR % s["tip"][:8], "strand", s["tip"], "RED")
        write(d, "summary.txt", "GATE-SUMMARY strand %s 3/3 10s GREEN\n" % s["tip"][:7])
        rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, merge.REFUSE_EXIT)
        self.assertIn("strand_gate_not_green", err)

    def test_merge_refuses_receipt_green_with_red_summary(self):
        f = self.f
        s = self.a_strand(receipt=False)
        d = f.receipt("S1", RUN_DIR % s["tip"][:8], "strand", s["tip"], "GREEN")
        write(d, "summary.txt", "GATE-SUMMARY strand %s 2/3 10s RED\n" % s["tip"][:7])
        rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, merge.REFUSE_EXIT)
        self.assertIn("strand_gate_not_green", err)


class CandidateTest(MergeCase):

    def test_merge_candidate_is_no_ff_no_commit(self):
        f = self.f
        s = self.a_strand()
        base = f.head()
        real = merge.run_gate
        seen = {}

        def gate(tree, ctx):
            seen["tree"] = tree
            seen["main_head"] = f.head()
            seen["main_status"] = git(f.root, "status", "--porcelain")
            seen["parents"] = git(tree, "rev-list", "--parents", "-n", "1", "HEAD").split()[1:]
            seen["subject"] = git(tree, "log", "-1", "--format=%s")
            seen["cand_status"] = git(tree, "status", "--porcelain")
            return real(tree, ctx)

        with mock.patch.object(merge, "run_gate", side_effect=gate):
            rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        # A detached worktree outside the main tree and outside `.git`.
        self.assertEqual(seen["tree"], os.path.join(f.state, "meclaw", "pipeline", "merge", "S1"))
        self.assertFalse(seen["tree"].startswith(f.root + os.sep))
        # The main tree stays untouched until the fast-forward.
        self.assertEqual(seen["main_head"], base)
        self.assertEqual(seen["main_status"], "")
        # --no-ff: two parents, the second the branch tip; --no-commit: our message.
        self.assertEqual(seen["parents"], [base, s["tip"]])
        self.assertEqual(seen["subject"], "Merge S1 (#101): Change S1")
        self.assertEqual(seen["cand_status"], "")
        self.assertFalse(os.path.exists(seen["tree"]))
        self.assertNotIn(seen["tree"], git(f.root, "worktree", "list"))

    def test_merge_gate_uses_cache_from_strand_run(self):
        f = self.f
        s = self.a_strand()
        base = f.head()
        rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        calls = f.gate_calls()
        self.assertEqual(len(calls), 1)
        self.assertEqual(calls[0]["cache_from"], [RUN_DIR % s["tip"][:8]])
        self.assertEqual(calls[0]["strands"], ["S1"])
        self.assertEqual(calls[0]["base"], base)
        self.assertEqual(calls[0]["rev"], f.head())

    def test_merge_writes_a_since_sentence_for_every_number(self):
        # F5 (gh1038): the strand's number and the cascade's each get one.
        f = self.f
        for d in ("a", "c"):
            path = "templates/%s/template.json" % d
            doc = json.loads(read(f.root, path))
            doc["description"] = {"purpose": "Template %s." % d}
            f.master_commit({path: json.dumps(doc, indent=2) + "\n"}, msg="purpose %s" % d)
        self.a_strand()
        rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        link = "#101"
        purpose = {d: json.loads(read(f.root, "templates/%s/template.json" % d))
                   ["description"]["purpose"] for d in ("a", "c")}
        self.assertEqual(purpose["a"], "Template a. Since 1.0.1 (%s) the template carries this "
                                       "change; it is the third digit." % link)
        self.assertEqual(purpose["c"], "Template c. Since 0.3.1 (%s) the template pins "
                                       "`a@1.0.1`; it is the third digit." % link)

    def candidate_kit(self, tail="", locks_tail=""):
        """The real `versions`/`locks` with `tail` appended to `versions.py` and
        `locks_tail` to `locks.py` -- a strand that changes the merge point's own
        numbering code."""
        src = SCRIPTS / "pipeline"
        return {"scripts/pipeline/__init__.py": "",
                "scripts/pipeline/locks.py":
                    (src / "locks.py").read_text(encoding="utf-8") + locks_tail,
                "scripts/pipeline/versions.py":
                    (src / "versions.py").read_text(encoding="utf-8") + tail}

    def test_the_candidate_is_numbered_by_its_own_cascade(self):
        # GH #1169, queue RED of tb-s8 (gate c58bcf1, 9/11): the strand taught
        # the cascade AND the pin-lock the escaped pin of an inlined script;
        # the merge point numbered the candidate with master's cascade (the
        # kit that runs the merge), the merge gate judged it with the
        # candidate's pin-lock -- `display@2.10.8 not provided -- raise to
        # 2.10.9`. One code numbers and judges: the candidate's own.
        f = self.f
        tail = ("\n_own_cascade = cascade\n\n\n"
                "def cascade(tree, moved, also=None):\n"
                "    with open(os.path.join(tree, 'notes/cascade.log'), 'a') as fh:\n"
                "        fh.write('candidate\\n')\n"
                "    return _own_cascade(tree, moved, also)\n")
        files = dict(self.candidate_kit(tail), **{"templates/a/foo.txt": "foo\n",
                                                  "notes/cascade.log": ""})
        f.strand("S1", 101, files)
        rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        # every round of the cascade is the candidate's (a, then c that pins it)
        log = read(f.root, "notes/cascade.log").splitlines()
        self.assertTrue(log)
        self.assertEqual(set(log), {"candidate"})
        self.assertEqual(version_of(f.root, "a"), "1.0.1")

    def test_a_candidate_that_changes_only_the_pin_lock_is_numbered_by_it(self):
        # GH #1169, recheck m-1: the pin form lives in `locks.py`
        # (`pin_position_re`); a strand that teaches only the pin-lock a new
        # form leaves `versions.py` byte-equal to the running kit's, and the
        # cascade must still read pins with the candidate's form.
        f = self.f
        logdir = tempfile.mkdtemp(prefix="m1-")
        self.addCleanup(shutil.rmtree, logdir, True)
        log_path = os.path.join(logdir, "pin_position_re.log")
        locks_tail = ("\n_own_pin_position_re = pin_position_re\n\n\n"
                      "def pin_position_re(names):\n"
                      "    with open(%r, 'a') as fh:\n"
                      "        fh.write('candidate\\n')\n"
                      "    return _own_pin_position_re(names)\n" % log_path)
        files = dict(self.candidate_kit(locks_tail=locks_tail),
                     **{"templates/a/foo.txt": "foo\n"})
        f.strand("S1", 101, files)
        rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        # the cascade (a moves, c pins it) asked the candidate's pin form
        self.assertTrue(os.path.exists(log_path), "the running kit's pin form numbered it")
        with open(log_path) as fh:
            log = fh.read().splitlines()
        self.assertEqual(set(log), {"candidate"})
        self.assertEqual(version_of(f.root, "a"), "1.0.1")

    def test_a_version_conflict_of_the_candidates_own_code_refuses(self):
        f = self.f
        tail = ("\n\ndef assign(*_args, **_kw):\n"
                "    raise VersionError('the candidate refuses')\n")
        files = dict(self.candidate_kit(tail), **{"templates/a/foo.txt": "foo\n"})
        f.strand("S1", 101, files)
        rc, _, err = f.merge(["S1"])
        self.assertNotEqual(rc, 0)
        self.assertIn("the candidate refuses", err)
        self.assertEqual(f.state_of("S1"), "refused")

    def test_merge_commit_message_names_issues_and_versions(self):
        f = self.f
        self.a_strand(kind={"a": "fix"}, title="Teach a a trick")
        rc, out, err = f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        self.assertEqual(f.subject(), "Merge S1 (#101): Teach a a trick")
        body = f.body()
        self.assertIn("Versions: a 1.0.0 -> 1.0.1 (fix), c 0.3.0 -> 0.3.1 (fix)", body)
        pins = [line for line in body.splitlines() if line.startswith("Pins: ")]
        self.assertEqual(len(pins), 1, body)
        self.assertIn(SCAFFOLD, pins[0])
        self.assertIn(EXAMPLE, pins[0])
        self.assertIn("Versions: a 1.0.0 -> 1.0.1 (fix)", out)
        # The [Unreleased] line, and ROADMAP § Now without the closed issue.
        changelog = read(f.root, "CHANGELOG.md")
        unreleased = changelog.split("## [Unreleased]")[1].split("## [0.1.0]")[0]
        self.assertIn("`a@1.0.1`", unreleased)
        self.assertIn("#101", unreleased)
        roadmap = read(f.root, "ROADMAP.md")
        now, nxt = roadmap.split("## Next")
        self.assertNotIn("#101", now)
        self.assertIn("#999", now)
        self.assertIn("#101", nxt)

    def test_merge_red_discards_candidate_and_frees_lease(self):
        f = self.f
        self.a_strand()
        base = f.head()
        with mock.patch.dict(os.environ, {"MECLAW_PIPELINE_FAKE_GATE": "red"}):
            rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, merge.REFUSE_EXIT)
        self.assertIn("gate_red", err)
        self.assertEqual(f.head(), base)
        self.assertEqual(git(f.root, "status", "--porcelain"), "")
        self.assertFalse(os.path.exists(merge.candidate_path("S1")))
        self.assertNotIn(merge.candidate_path("S1"), git(f.root, "worktree", "list"))
        self.assertIsNone(f.lease())
        self.assertEqual(f.state_of("S1"), "refused")
        refused = f.events("merge_refused")
        self.assertEqual([(e["strand"], e["reason"]) for e in refused], [("S1", "gate_red")])
        self.assertEqual(f.events("merge_done"), [])
        self.assertEqual(f.gh_calls(), [])


class FastForwardTest(MergeCase):

    def test_merge_fast_forwards_main_tree_never_update_ref(self):
        f = self.f
        s = self.a_strand()
        base = f.head()
        rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        sha = f.gate_calls()[0]["rev"]
        self.assertEqual(f.head(), sha)
        self.assertEqual(f.parents(), [base, s["tip"]])
        # The working tree moved with master: nothing reads as a backwards diff.
        self.assertEqual(git(f.root, "status", "--porcelain"), "")
        self.assertEqual(read(f.root, "templates/a/foo.txt"), "foo\n")
        self.assertEqual(version_of(f.root, "a"), "1.0.1")
        self.assertIn("Fast-forward", git(f.root, "reflog", "-1", "--format=%gs", "master"))
        self.assertEqual(f.state_of("S1"), "merged")
        done = f.events("merge_done")
        self.assertEqual([(e["strand"], e["merge_sha"]) for e in done], [("S1", sha)])
        self.assertIsNone(f.lease())
        # The strand's issue was closed with its own commits only.
        closes = [c for c in f.gh_calls() if c[:2] == ["issue", "close"]]
        self.assertEqual([c[2] for c in closes], ["101"])

    def test_merge_stays_merged_when_close_refuses(self):
        f = self.f
        s = self.a_strand()
        # The head block lists a commit of master: `close` refuses, the merge stands.
        f.report("S1", s["branch"], 101, [f.first[:8]])
        rc, _, err = f.merge(["S1"])
        self.assertIn("foreign_commits", err)
        self.assertEqual(f.state_of("S1"), "merged")
        self.assertEqual(f.events("merge_refused"), [])
        self.assertEqual(f.head(), f.gate_calls()[0]["rev"])
        self.assertEqual(rc, 0)
        self.assertIsNone(f.lease())

    def test_merge_refuses_dirty_main_tree(self):
        f = self.f
        self.a_strand()
        base = f.head()
        write(f.root, "templates/a/template.json", template_json("a", "1.0.0", note="local"))
        rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, merge.REFUSE_EXIT)
        self.assertIn("main_tree_dirty", err)
        self.assertEqual(f.head(), base)
        self.assertIsNone(f.lease())
        self.assertEqual(f.state_of("S1"), "refused")
        self.assertIn("local", read(f.root, "templates/a/template.json"))

    def test_merge_commit_runs_the_pre_commit_hook(self):
        f = self.f
        self.a_strand()
        base = f.head()
        hook = os.path.join(f.root, ".git", "hooks", "pre-commit")
        write(f.root, ".git/hooks/pre-commit", "#!/bin/sh\necho guard refused >&2\nexit 1\n")
        os.chmod(hook, 0o755)
        rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, merge.REFUSE_EXIT)
        self.assertIn("guard refused", err)
        self.assertEqual(f.head(), base)

    def test_merge_accepts_strand_gate_on_older_base(self):
        f = self.f
        s = self.a_strand()
        newer = f.master_commit({"docs/other.md": "Other, moved on.\n"})
        rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        self.assertEqual(f.parents(), [newer, s["tip"]])
        self.assertEqual(read(f.root, "docs/other.md"), "Other, moved on.\n")
        self.assertEqual(f.gate_calls()[0]["base"], newer)


class TrackedPlansTest(MergeCase):
    """Strands commit their report and strand file on the branch (I2)."""

    plans_tracked = True

    def test_merge_reads_report_and_strand_file_from_branch_tip(self):
        f = self.f
        s = self.a_strand(in_branch=True, kind={"a": "feature"})
        self.assertFalse(os.path.exists(os.path.join(f.wdir, "berichte", "S1.md")))
        rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        self.assertEqual(f.parents()[1], s["tip"])
        self.assertEqual(version_of(f.root, "a"), "1.1.0")
        self.assertIn("# Report S1", read(f.root, "plans/%s/berichte/S1.md" % WAVE))
        self.assertEqual(git(f.root, "status", "--porcelain", "--untracked-files=no"), "")

    def test_merge_sets_aside_byte_identical_untracked_copy(self):
        f = self.f
        s = self.a_strand(in_branch=True)
        rel = "plans/%s/berichte/S1.md" % WAVE
        same = git(s["wt"], "show", "HEAD:%s" % rel) + "\n"
        write(f.root, rel, same)
        rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        self.assertEqual(read(f.root, rel), same)
        aside = [os.path.join(d, n) for d, _, ns in os.walk(os.path.join(f.state, "meclaw"))
                 for n in ns if n == "S1.md"]
        self.assertEqual(len(aside), 1, aside)

    def test_merge_refuses_differing_untracked_copy(self):
        f = self.f
        self.a_strand(in_branch=True)
        rel = "plans/%s/berichte/S1.md" % WAVE
        write(f.root, rel, "a local draft\n")
        rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, merge.REFUSE_EXIT)
        self.assertIn("main_tree_dirty", err)
        self.assertEqual(read(f.root, rel), "a local draft\n")


class BatchTest(MergeCase):
    gate = "red-if:S2"

    def test_batch_bisects_on_red(self):
        f = self.f
        s1 = self.a_strand()
        s2 = f.strand("S2", 102, {"templates/b/bar.txt": "bar\n"})
        base = f.head()
        rc, _, err = f.merge(["S1", "S2"])
        self.assertEqual(rc, merge.REFUSE_EXIT, err)
        self.assertEqual([c["strands"] for c in f.gate_calls()], [["S1", "S2"], ["S1"], ["S2"]])
        self.assertEqual(f.state_of("S1"), "merged")
        self.assertEqual(f.state_of("S2"), "refused")
        self.assertEqual(f.parents(), [base, s1["tip"]])
        self.assertEqual(subprocess.run(["git", "-C", f.root, "merge-base", "--is-ancestor",
                                         s2["tip"], "master"]).returncode, 1)
        self.assertEqual([(e["strand"], e["reason"]) for e in f.events("merge_refused")],
                         [("S2", "gate_red")])
        self.assertIsNone(f.lease())

    def test_aborted_batch_does_not_bisect(self):
        # GH #1154 item 2: an ABORTED gate judged nothing -- the batch is
        # refused as a whole, no half is gated on its own.
        f = self.f
        self.a_strand()
        f.strand("S2", 102, {"templates/b/bar.txt": "bar\n"})
        base = f.head()
        with mock.patch.dict(os.environ, {"MECLAW_PIPELINE_FAKE_GATE": "aborted"}):
            rc, _, err = f.merge(["S1", "S2"])
        self.assertEqual(rc, merge.REFUSE_EXIT, err)
        self.assertEqual([c["strands"] for c in f.gate_calls()], [["S1", "S2"]])
        self.assertEqual([(e["strand"], e["reason"]) for e in f.events("merge_refused")],
                         [("S1", "gate_aborted"), ("S2", "gate_aborted")])
        self.assertEqual(f.head(), base)

    def test_batch_gate_exit_1_without_summary_does_not_bisect(self):
        # GH #1154 item 1: exit 1 without a GATE-SUMMARY is a crashed gate,
        # not a red verdict -- the batch stops, reported as not judged.
        f = self.f
        self.a_strand()
        f.strand("S2", 102, {"templates/b/bar.txt": "bar\n"})
        base = f.head()
        with mock.patch.dict(os.environ, {"MECLAW_PIPELINE_FAKE_GATE": "",
                                          "MECLAW_MERGE_GATE_CMD": "sh -c 'exit 1'"}):
            rc, _, err = f.merge(["S1", "S2"])
        self.assertEqual(rc, merge.REFUSE_EXIT, err)
        self.assertEqual(len(f.events("merge_gated")), 2)
        refused = f.events("merge_refused")
        self.assertEqual([(e["strand"], e["reason"]) for e in refused],
                         [("S1", "gate_aborted"), ("S2", "gate_aborted")])
        self.assertIn("not judged", refused[0].get("text", "") + err)
        self.assertEqual(f.head(), base)


    def test_every_gate_run_renews_the_lease(self):
        # N3: a bisection runs several gates under one lease; each run must
        # start on a renewed lease, not only the fast-forward at the end.
        f = self.f
        s1 = self.a_strand()
        f.strand("S2", 102, {"templates/b/bar.txt": "bar\n"})
        base = f.head()
        real = merge.run_gate
        shift = datetime.timedelta(seconds=int(merge.TTL_S * 0.6))

        def gate(tree, ctx):
            # Each gate run "takes" 0.6 TTL: two runs without a renewal in
            # between outlive the lease.
            res = merge.resource("meclaw")
            row = f.db.execute("SELECT taken_at FROM leases WHERE resource=?", (res,)).fetchone()
            back = (store.parse_at(row["taken_at"]) - shift).strftime("%Y-%m-%dT%H:%M:%SZ")
            with store.tx(f.db):
                f.db.execute("UPDATE leases SET taken_at=? WHERE resource=?", (back, res))
            return real(tree, ctx)

        with mock.patch.object(merge, "run_gate", side_effect=gate):
            rc, _, err = f.merge(["S1", "S2"])
        self.assertNotIn("lease_lost", err)
        self.assertEqual(rc, merge.REFUSE_EXIT, err)
        self.assertEqual(f.state_of("S1"), "merged")
        self.assertEqual(f.parents(), [base, s1["tip"]])
        self.assertIsNone(f.lease())


class GateRevTest(MergeCase):
    """F1: the merge gate must have gated THE candidate -- fail closed otherwise."""

    def no_fake(self, **extra):
        env = {"MECLAW_PIPELINE_FAKE_GATE": ""}
        env.update(extra)
        return mock.patch.dict(os.environ, env)

    def test_merge_refuses_gate_on_foreign_rev(self):
        f = self.f
        self.a_strand()
        base = f.head()
        cmd = "echo GATE-SUMMARY strand 1a91005 11/11 386s GREEN"
        with self.no_fake(MECLAW_MERGE_GATE_CMD=cmd):
            rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, merge.REFUSE_EXIT)
        self.assertIn("gate_foreign_rev", err)
        self.assertEqual(f.head(), base)
        self.assertEqual(f.state_of("S1"), "refused")
        self.assertEqual([e["reason"] for e in f.events("merge_refused")], ["gate_foreign_rev"])
        self.assertEqual(f.events("merge_done"), [])
        self.assertIsNone(f.lease())

    def test_merge_accepts_gate_on_candidate_rev(self):
        f = self.f
        self.a_strand()
        cmd = ("sh -c 'echo gate: rev = $(git -C {tree} rev-parse HEAD); "
               "echo GATE-SUMMARY strand $(git -C {tree} rev-parse --short=7 HEAD) 1/1 1s GREEN'")
        with self.no_fake(MECLAW_MERGE_GATE_CMD=cmd):
            rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        self.assertEqual(f.state_of("S1"), "merged")

    def test_merge_refuses_short_rev_without_full_rev_line(self):
        f = self.f
        self.a_strand()
        cmd = "sh -c 'echo GATE-SUMMARY strand $(git -C {tree} rev-parse --short=7 HEAD) 1/1 1s GREEN'"
        with self.no_fake(MECLAW_MERGE_GATE_CMD=cmd):
            rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, merge.REFUSE_EXIT)
        self.assertIn("gate_foreign_rev", err)

    def test_check_rev_needs_twelve_characters(self):
        sha = "1a91005c5" + "0" * 31
        for rev in ("1", "1a91005", "1a91005c500"):
            with self.assertRaises(merge.Refused):
                merge._check_rev({"rev": sha}, rev, "test")
        merge._check_rev({"rev": sha}, sha[:12], "test")
        merge._check_rev({"rev": sha}, sha, "test")

    def host_gate(self, rev=None, answer=None):
        """`merge --host lane1` with the engine gate faked; returns (rc, err, calls).
        `answer` = (exit code, stdout) replaces the GREEN gate output."""
        f = self.f
        lanes = os.path.join(f.tmp, "lanes")
        write(f.tmp, "lanes", "lane1 user@host.invalid 10.0.0.9 threads=6\n"
                              "lane2 local /srv/lane2 class=egon\n")
        calls = []
        real = subprocess.run

        def fake(argv, *a, **kw):
            name = os.path.basename(str(argv[0])) if argv else ""
            if name in ("gate.sh", "lane_sync.sh"):
                calls.append({"argv": list(argv), "cwd": kw.get("cwd"),
                              "head": git(kw.get("cwd"), "rev-parse", "HEAD"),
                              "cache_from": (kw.get("env") or {}).get("MECLAW_GATE_CACHE_FROM")})
                if answer is not None:
                    return subprocess.CompletedProcess(argv, answer[0], answer[1], "")
                r = rev or calls[-1]["head"]
                text = "gate: rev = %s\nGATE-SUMMARY strand %s 11/11 30s GREEN\n" % (r, r[:7])
                return subprocess.CompletedProcess(argv, 0, text, "")
            return real(argv, *a, **kw)

        s = self.a_strand()
        # A v2 receipt names the store run the cache rows carry.
        rec = os.path.join(f.wdir, "receipts", "S1", RUN_DIR % s["tip"][:8], "last-strand.json")
        with open(rec, encoding="utf-8") as fh:
            doc = json.load(fh)
        doc["run"] = "run-0000000000s1"
        write(os.path.dirname(rec), "last-strand.json", json.dumps(doc) + "\n")
        with self.no_fake(MECLAW_LANES_FILE=lanes), \
                mock.patch.object(merge.subprocess, "run", side_effect=fake):
            rc, _, err = f.merge(["S1"], host="lane1")
        return rc, err, calls, s

    def test_merge_host_runs_the_engine_gate_on_the_lane(self):
        # GH #1141: the lane merge goes through the engine (`gate.sh strand
        # --host`), which pushes the candidate itself (overlay hash, exit 5 on
        # a foreign tree) and looks up the content-keyed cache the strand run
        # filled -- no `lane_sync.sh run` into the legacy runner without cache.
        f = self.f
        base = f.head()
        rc, err, calls, _ = self.host_gate()
        self.assertEqual(rc, 0, err)
        self.assertEqual(len(calls), 1, calls)
        call = calls[0]
        self.assertTrue(call["argv"][0].endswith(os.path.join("scripts", "gate.sh")), call)
        self.assertEqual(call["argv"][1:], ["strand", "--host", "lane1", "--base", base,
                                            "--strand", "welle-test/S1"])  # kit find (h)
        self.assertEqual(call["head"], f.head())

    def test_merge_host_offers_the_store_run_of_the_strand_receipt(self):
        rc, err, calls, _ = self.host_gate()
        self.assertEqual(rc, 0, err)
        self.assertEqual(calls[0]["cache_from"], "run-0000000000s1")

    def test_merge_host_refuses_when_the_gate_names_another_rev(self):
        f = self.f
        base = f.head()
        rc, err, calls, _ = self.host_gate(rev="1a91005c5" + "0" * 31)
        self.assertEqual(rc, merge.REFUSE_EXIT)
        self.assertIn("gate_foreign_rev", err)
        self.assertEqual(f.head(), base)


    def test_merge_host_without_summary_aborts_unless_the_gate_said_red(self):
        # Review Zb I1: the engine ends without a GATE-SUMMARY on exit 3 (lane
        # waits, also on a lease lock) and exit 5 (build path busy, foreign
        # overlay). That is no red verdict -- `gate_aborted`, never `gate_red`
        # (a red batch bisects). GH #1154: exit 1 without a summary is a
        # crash, not a verdict -- aborted as well. GH #1155: exit 6 (store
        # error at the admission) gated nothing either.
        for code, reason in ((3, "gate_aborted"), (5, "gate_aborted"), (6, "gate_aborted"),
                             (2, "gate_aborted"), (1, "gate_aborted")):
            with self.subTest(code=code):
                self.setUp()
                f = self.f
                base = f.head()
                rc, err, calls, _ = self.host_gate(answer=(code, "gate: queued on lane1\n"))
                self.assertEqual(rc, merge.REFUSE_EXIT, err)
                self.assertEqual(len(calls), 1, calls)
                self.assertEqual([e["reason"] for e in f.events("merge_refused")], [reason])
                self.assertEqual(f.head(), base)


class CloseTest(MergeCase):

    def close_info(self, name="S1"):
        return merge.close_info(self.f.root, os.path.join(self.f.wdir, "berichte", "%s.md" % name),
                                os.path.join(self.f.wdir, "receipts", name), name)

    def test_close_lists_only_strand_commits(self):
        f = self.f
        s = self.a_strand(commits=2)
        f.master_commit({"docs/other.md": "Foreign one.\n"}, "foreign 1")
        f.master_commit({"docs/other.md": "Foreign two.\n"}, "foreign 2")
        own = [c[:8] for c in s["commits"]]
        self.assertEqual(self.close_info()[2], own)
        rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        f.master_commit({"docs/other.md": "Foreign three.\n"}, "foreign 3")
        issues, comment, commits = self.close_info()
        self.assertEqual(issues, ["101"])
        self.assertEqual(commits, own)
        self.assertIn("Commits: %s" % ", ".join(own), comment)
        closes = [c for c in f.gh_calls() if c[:2] == ["issue", "close"]]
        self.assertEqual(len(closes), 1)
        self.assertIn("Commits: %s\n" % ", ".join(own), closes[0][4] + "\n")

    def test_close_picks_latest_strand_receipt(self):
        f = self.f
        s = self.a_strand()
        f.receipt("S1", "20250101T000000Z-%s" % s["tip"][:8], "strand", s["tip"], "RED")
        f.receipt("S1", "20990101T000000Z-ffffffff", "integration", s["tip"], "RED")
        _, comment, _ = self.close_info()
        self.assertIn("Gate: `GATE-SUMMARY strand %s 3/3 10s GREEN`" % s["tip"][:7], comment)
        self.assertNotIn("integration", comment)

    def test_close_refuses_commit_reachable_before_branch_point(self):
        f = self.f
        s = self.a_strand()
        f.report("S1", s["branch"], 101, [f.first[:8]] + [c[:8] for c in s["commits"]])
        with self.assertRaises(merge.Refused) as cm:
            self.close_info()
        self.assertEqual(cm.exception.reason, "foreign_commits")
        self.assertIn(f.first[:8], cm.exception.text)


class MergeQueueZiTest(MergeCase):
    """The first real `strand.sh merge --host` (Z-B) was refused five ways (Z-I 2c)."""

    def test_merge_takes_the_branch_name_and_the_strand_name(self):
        # (1) `--strand welle-test/S1` was `no_wave: 0 waves know ...`.
        f = self.f
        self.a_strand()
        out, err = io.StringIO(), io.StringIO()
        rc = merge.merge(["welle-test/S1"], start=f.root, db=f.db, out_=out, err=err)
        self.assertEqual(rc, 0, err.getvalue())
        self.assertIn("MERGED S1 ", out.getvalue())

    def test_merge_takes_the_branch_name_with_a_wave(self):
        self.a_strand()
        rc, out, err = self.f.merge(["welle-test/S1"])
        self.assertEqual(rc, 0, err)
        self.assertIn("MERGED S1 ", out)

    def test_merge_accepts_the_fix_round_review_heading(self):
        # (2) the reports write `## Fix-Runde Review <sha>`, never a bare `## Review`.
        f = self.f
        s = self.a_strand(report=False)
        f.report("S1", s["branch"], 101, [c[:8] for c in s["commits"]], review=False)
        with open(os.path.join(f.wdir, "berichte", "S1.md"), "a", encoding="utf-8") as fh:
            fh.write("\n## Fix-Runde Review 0123abcd\n\nC0/I0. **Verdikt: JA**\n")
        rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, 0, err)

    def test_merge_refusals_exit_non_zero_on_the_command_line(self):
        # (3) a refusal is never rc 0, on the command line either.
        f = self.f
        self.a_strand(report=False)
        f.report("S1", "welle-test/S1", 101, [], review=False)
        for argv in (["merge", "--strand", "nobody"],
                     ["merge", "--strand", "S1", "--wave", WAVE],
                     ["merge", "--strand", "welle-test/S1"]):
            res = subprocess.run([sys.executable, str(SCRIPTS / "pipeline.py")] + argv,
                                 cwd=f.root, capture_output=True, text=True)
            self.assertEqual(res.returncode, merge.REFUSE_EXIT, (argv, res.stderr))
            self.assertIn("refused (", res.stderr)

    def test_close_leaves_the_wave_issue_open(self):
        # (5) every strand file of the wave carries the wave issue: close skips it.
        f = self.f
        s = self.a_strand()
        write(f.wdir, "straenge/S1.json", json.dumps({"strand": "S1", "issue": [101, 1126],
                                                      "title": "x", "kind": {}}) + "\n")
        write(f.wdir, "straenge/S2.json", json.dumps({"strand": "S2", "issue": 1126,
                                                      "title": "y", "kind": {}}) + "\n")
        f.report("S1", s["branch"], "101, #1126", [c[:8] for c in s["commits"]])
        rc, out, err = f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        closed = [c[2] for c in f.gh_calls() if c[:2] == ["issue", "close"]]
        self.assertEqual(closed, ["101"])
        self.assertIn("#1126", out + err)

    def test_close_issue_false_comments_and_leaves_the_carrier_issue_open(self):
        # V2-A1 (welle-val2): a strand on a carrier issue (the release issue
        # of a version not exported yet) says `"close_issue": false`; the merge
        # comments the issue in English and never closes it.
        f = self.f
        s = self.a_strand()
        write(f.wdir, "straenge/S1.json", json.dumps({"strand": "S1", "issue": 101, "title": "x",
                                                      "kind": {}, "close_issue": False}) + "\n")
        rc, out, err = f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        calls = f.gh_calls()
        self.assertEqual([c for c in calls if c[:2] == ["issue", "close"]], [])
        comments = [c for c in calls if c[:2] == ["issue", "comment"]]
        self.assertEqual([(c[2], c[3]) for c in comments], [("101", "--body")])
        self.assertIn("Gate: `GATE-SUMMARY strand", comments[0][4])
        self.assertIn("#101 left open", out + err)

    def test_close_dry_run_prints_a_comment_for_close_issue_false(self):
        f = self.f
        s = self.a_strand()
        write(f.wdir, "straenge/S1.json", json.dumps({"strand": "S1", "issue": 101, "title": "x",
                                                      "kind": {}, "close_issue": False}) + "\n")
        out = io.StringIO()
        self.assertEqual(merge.close_issues(merge.Strand(f.root, "S1", WAVE), out_=out), 0)
        self.assertIn("gh issue comment 101 --body ", out.getvalue())
        self.assertNotIn("gh issue close", out.getvalue())


class InheritedRepositoryTest(MergeCase):
    """`strand.sh merge` without `cd` (`GIT_DIR`/`GIT_WORK_TREE` of the strand
    tree) ran the candidate's `merge`/`commit` in the strand tree: `git -C`
    loses against `GIT_DIR` (re-check pd-legacy M-1)."""

    def _merge_from(self, argv, s):
        env = dict(os.environ, GIT_DIR=git(s["wt"], "rev-parse", "--absolute-git-dir"),
                   GIT_WORK_TREE=s["wt"])
        return subprocess.run(argv, cwd=self.f.tmp, env=env, capture_output=True, text=True)

    def _assert_landed_in_master(self, res, s, base):
        f = self.f
        self.assertEqual(res.returncode, 0, res.stdout + res.stderr)
        self.assertEqual(f.parents(), [base, s["tip"]])
        self.assertEqual(git(s["wt"], "rev-parse", "HEAD"), s["tip"])
        self.assertEqual(git(s["wt"], "status", "--porcelain"), "")
        self.assertEqual(f.state_of("S1"), "merged")

    def test_pipeline_merge_lands_in_master_not_in_the_callers_tree(self):
        s = self.a_strand()
        base = self.f.head()
        res = self._merge_from([sys.executable, str(SCRIPTS / "pipeline.py"), "merge",
                                "--strand", "S1", "--wave", WAVE], s)
        self._assert_landed_in_master(res, s, base)

    def test_strand_sh_merge_lands_in_master_not_in_the_callers_tree(self):
        s = self.a_strand()
        base = self.f.head()
        res = self._merge_from(["bash", str(SCRIPTS / "strand.sh"), "merge",
                                "--strand", "S1", "--wave", WAVE], s)
        self._assert_landed_in_master(res, s, base)


class StrandShEntryTest(MergeCase):
    """strand.sh itself drops an inherited repository before it dispatches
    (re-check pd-legacy M-N1): the case above stays green on `pipeline.py`'s
    own entry alone, and `new`, `gate ci`, `close` and `report` never pass
    through it. A kit copy with a probe `pipeline.py` sees exactly what
    strand.sh hands on."""

    PROBE = (
        "import json, os, subprocess, sys\n"
        "names = subprocess.run(['git', 'rev-parse', '--local-env-vars'],\n"
        "                       capture_output=True, text=True).stdout.split()\n"
        "seen = {'leaked': sorted(n for n in names if n in os.environ), 'cwd': os.getcwd()}\n"
        "with open(os.environ['STRAND_PROBE_OUT'], 'w') as fh:\n"
        "    json.dump(seen, fh)\n"
        "if seen['leaked']:\n"
        "    sys.exit('probe: strand.sh handed on %s' % seen['leaked'])\n"
        "if os.environ.get('STRAND_PROBE_REAL'):\n"
        "    real = os.environ['STRAND_PROBE_REAL']\n"
        "    os.execv(sys.executable, [sys.executable, real] + sys.argv[1:])\n"
    )

    def setUp(self):
        super().setUp()
        self.kit = os.path.join(self.f.tmp, "kit")
        os.makedirs(self.kit)
        shutil.copy2(SCRIPTS / "strand.sh", os.path.join(self.kit, "strand.sh"))
        kit_tree.seed(self.kit, "strand.sh")
        write(self.kit, "pipeline.py", self.PROBE)
        self.probe_out = os.path.join(self.f.tmp, "probe.json")

    def _strand_sh(self, args, cwd, real=False, **git_env):
        env = dict(os.environ, STRAND_PROBE_OUT=self.probe_out, **git_env)
        if real:
            env["STRAND_PROBE_REAL"] = str(SCRIPTS / "pipeline.py")
        res = subprocess.run(["bash", os.path.join(self.kit, "strand.sh")] + args,
                             cwd=cwd, env=env, capture_output=True, text=True)
        with open(self.probe_out) as fh:
            return res, json.load(fh)

    @staticmethod
    def _repo_state(tree):
        gitdir = git(tree, "rev-parse", "--absolute-git-dir")
        common = os.path.realpath(os.path.join(tree, git(tree, "rev-parse", "--git-common-dir")))
        files = {}
        for path in (os.path.join(gitdir, "HEAD"), os.path.join(common, "config"),
                     os.path.join(gitdir, "config.worktree")):
            if os.path.exists(path):
                with open(path, "rb") as fh:
                    files[path] = fh.read()
        return {"head": git(tree, "rev-parse", "HEAD"),
                "branch": subprocess.run(["git", "-C", tree, "symbolic-ref", "-q", "HEAD"],
                                         capture_output=True, text=True).stdout,
                "index": git(tree, "ls-files", "--stage"), "files": files}

    def test_strand_sh_merge_drops_the_inherited_repository_before_the_dispatch(self):
        s = self.a_strand()
        base = self.f.head()
        before = self._repo_state(s["wt"])
        res, seen = self._strand_sh(
            ["merge", "--strand", "S1", "--wave", WAVE], self.f.tmp, real=True,
            GIT_DIR=git(s["wt"], "rev-parse", "--absolute-git-dir"), GIT_WORK_TREE=s["wt"])
        self.assertEqual(seen["leaked"], [], res.stderr)
        self.assertEqual(os.path.realpath(seen["cwd"]), os.path.realpath(s["wt"]))
        self.assertNotIn("NOTE inherited GIT_DIR", res.stderr)
        self.assertEqual(self._repo_state(s["wt"]), before)
        self.assertEqual(res.returncode, 0, res.stdout + res.stderr)
        self.assertEqual(self.f.parents(), [base, s["tip"]])
        self.assertEqual(self.f.state_of("S1"), "merged")

    def test_a_git_dir_the_working_directory_overrules_is_named(self):
        """m-N2: `GIT_DIR` of the strand tree from inside the main tree -- the
        working directory wins, and that is said, not swallowed."""
        s = self.a_strand()
        before = self._repo_state(s["wt"])
        named = os.path.realpath(git(s["wt"], "rev-parse", "--absolute-git-dir"))
        res, seen = self._strand_sh(["merge", "--strand", "S1", "--wave", WAVE],
                                    self.f.root, GIT_DIR=named)
        self.assertEqual(res.returncode, 0, res.stderr)
        self.assertEqual(seen["leaked"], [])
        self.assertEqual(os.path.realpath(seen["cwd"]), os.path.realpath(self.f.root))
        self.assertIn("strand: NOTE inherited GIT_DIR names %s, working in " % named, res.stderr)
        self.assertEqual(self._repo_state(s["wt"]), before)


class MergeGateUnnarrowedTest(unittest.TestCase):
    """Review Z-I1 C2: the merge (and revert) gate never narrows."""

    def test_the_merge_gate_names_its_holder(self):
        # Kit find (h) (ledger 03:35): in the detached merge tree the gate had no
        # holder ("") -- refused on its own strand's lane, waiter without holder left.
        for host in (None, "build04"):
            ctx = {"rev": "abcdef123456", "base": "b", "label": "S1", "host": host,
                   "holder": "welle-x/S1"}
            with mock.patch.dict(os.environ, {"MECLAW_MERGE_GATE_CMD": ""}), \
                    mock.patch.object(merge, "_lane_target", return_value="x"):
                argv = merge.gate_argv("/t", ctx)
            self.assertEqual("welle-x/S1", argv[argv.index("--strand") + 1], argv)
        with mock.patch.dict(os.environ, {"MECLAW_MERGE_GATE_CMD": "{tree}/g --strand {holder}"}):
            argv = merge.gate_argv("/t", dict(ctx, host=None))
        self.assertEqual(["/t/g", "--strand", "welle-x/S1"], argv)

    def test_the_gate_holder_is_the_strand_lease_or_the_batch(self):
        def strand(name, branch, wdir="/r/plans/welle-x-2026-10-11"):
            return argparse.Namespace(name=name, branch=branch, wdir=wdir)
        self.assertEqual("welle-x/S1", merge.gate_holder([strand("S1", "welle-x/S1")]))
        self.assertEqual("welle-x/S1+S2", merge.gate_holder(
            [strand("S1", "welle-x/S1"), strand("S2", "welle-x/S2")]))
        # a branch without a wave prefix: the wave directory names it
        self.assertEqual("welle-x/S1", merge.gate_holder([strand("S1", "S1")]))
        # Re-check fix4 H3: a branch whose suffix is not the name -- the lease is the branch
        self.assertEqual("welle-x/S1-fix", merge.gate_holder([strand("S1", "welle-x/S1-fix")]))

    def test_the_merge_gate_takes_no_queue_place(self):
        # Kit find (h) (2): the refused merge gate left a waiter that was offered
        # a lane it never took -- the gate in merge mode admits with queue=False.
        from pipeline import gate, schedule
        seen = {}

        def fake_admit(db, doc, **kw):
            seen.update(kw)
            return schedule.Queued("lane build04 is leased")
        o = argparse.Namespace(host="build04", mode="strand", strand="welle-x/S1", wave=None,
                               holder=None, any_lane=False, wait_admission=False)
        with mock.patch.dict(os.environ, {"MECLAW_GATE_MODE": "merge"}), \
                mock.patch.object(gate.lane, "load_lanes", return_value=[]), \
                mock.patch.object(gate, "_sync"), \
                mock.patch.object(gate, "holder_key", return_value="welle-x/S1"), \
                mock.patch.object(gate.schedule, "admit", side_effect=fake_admit):
            rc = gate._run_lane(o, "/t", {"repo": "meclaw"}, {}, None, io.StringIO(),
                                io.StringIO(), meta_path="/t/m")
        self.assertEqual(3, rc)
        self.assertIs(False, seen.get("queue"))

    def test_the_merge_gate_runs_with_the_narrowing_off(self):
        seen = {}

        def fake_run(argv, cwd=None, env=None, **_kw):
            seen.update(env or {})
            return subprocess.CompletedProcess(argv, 0, "GATE-SUMMARY strand abcdef123456 1/1 1s GREEN\n", "")
        state = tempfile.TemporaryDirectory()
        self.addCleanup(state.cleanup)
        os.makedirs(os.path.join(state.name, "meclaw", "pipeline", "merge"))
        with mock.patch.dict(os.environ, {"MECLAW_PIPELINE_FAKE_GATE": "", "MECLAW_GATE_NARROW": "1",
                                          "MECLAW_MERGE_GATE_CMD": "true",
                                          "XDG_STATE_HOME": state.name}), \
                mock.patch.object(merge.subprocess, "run", side_effect=fake_run):
            try:
                merge.run_gate("/nonexistent", {"rev": "abcdef123456", "base": "b", "label": "S1"})
            except merge.Refused:
                pass
        self.assertEqual("0", seen.get("MECLAW_GATE_NARROW"))
        self.assertEqual("merge", seen.get("MECLAW_GATE_MODE"))

    def test_a_single_strand_merge_hands_its_strand_run_to_the_gate(self):
        """welle-tb S1 part B (#1159): ONE strand with its GREEN run -- the gate
        decides the narrowing (`narrow.merge_decision`); a batch, a revert or a
        merge without a run stays off."""
        state = tempfile.TemporaryDirectory()
        self.addCleanup(state.cleanup)
        os.makedirs(os.path.join(state.name, "meclaw", "pipeline", "merge"))

        def gate_env(ctx):
            seen = {}

            def fake_run(argv, cwd=None, env=None, **_kw):
                seen.update(env or {})
                return subprocess.CompletedProcess(
                    argv, 0, "GATE-SUMMARY strand abcdef123456 1/1 1s GREEN\n", "")
            with mock.patch.dict(os.environ, {"MECLAW_PIPELINE_FAKE_GATE": "",
                                              "MECLAW_GATE_STRAND_RUN": "tr-stale",
                                              "MECLAW_GATE_NARROW": "1",
                                              "MECLAW_MERGE_GATE_CMD": "true",
                                              "XDG_STATE_HOME": state.name}), \
                    mock.patch.object(merge.subprocess, "run", side_effect=fake_run):
                try:
                    merge.run_gate("/nonexistent", dict(ctx, rev="abcdef123456", base="b"))
                except merge.Refused:
                    pass
            return seen
        one = gate_env({"label": "S1", "strands": ["S1"], "strand_run": "tr-000000000001"})
        self.assertEqual("tr-000000000001", one.get("MECLAW_GATE_STRAND_RUN"))
        self.assertEqual("1", one.get("MECLAW_GATE_NARROW"))   # the caller's switch holds
        for ctx in ({"label": "B", "strands": ["S1", "S2"], "strand_run": "tr-000000000001"},
                    {"label": "R", "strands": ["S1"]},
                    {"label": "S1", "strands": ["S1"], "strand_run": None}):
            seen = gate_env(ctx)
            self.assertEqual("0", seen.get("MECLAW_GATE_NARROW"), ctx)
            self.assertNotIn("MECLAW_GATE_STRAND_RUN", seen, ctx)


class ReportAfterGateTest(MergeCase):
    """(4) The report commit comes after the strand gate: a diff that only
    touches `plans/<wave>/{berichte,straenge}` between receipt and tip passes."""

    plans_tracked = True

    def report_commit(self, s, rel, text):
        write(s["wt"], rel, text)
        git(s["wt"], "add", "-A")
        git(s["wt"], "commit", "-q", "-m", "S1: report after the gate")
        return git(s["wt"], "rev-parse", "HEAD")

    def test_merge_accepts_a_report_only_commit_after_the_gate(self):
        f = self.f
        s = self.a_strand(in_branch=True)
        rel = "plans/%s/berichte/S1.md" % WAVE
        tip = self.report_commit(s, rel, read(s["wt"], rel) + "\nGate GREEN.\n")
        rc, out, err = f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        self.assertEqual(f.parents()[1], tip)

    def test_merge_refuses_a_code_commit_after_the_gate(self):
        f = self.f
        s = self.a_strand(in_branch=True)
        self.report_commit(s, "templates/a/foo.txt", "later\n")
        rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, merge.REFUSE_EXIT, err)
        self.assertIn("receipt_not_branch_head", err)

    def test_merge_refuses_a_report_commit_of_another_wave(self):
        f = self.f
        s = self.a_strand(in_branch=True)
        self.report_commit(s, "plans/welle-other-2026-01-02/berichte/S1.md", "x\n")
        rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, merge.REFUSE_EXIT, err)
        self.assertIn("receipt_not_branch_head", err)


if __name__ == "__main__":
    unittest.main()
