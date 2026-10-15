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
import threading
import unittest
from unittest import mock

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS))

from pipeline import merge, pre, store  # noqa: E402
from scripts.tests import kit_time, kit_tree, store_isolation  # noqa: E402

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


class _Said(io.StringIO):
    """An err stream that sets `queued` when the merge says its queue line --
    the event the tests of `--wait` synchronise on (no clock, no poll)."""

    def __init__(self):
        super().__init__()
        self.queued = threading.Event()
        self.waits = []                     # the `merge: waiting` lines, in order
        self._cond = threading.Condition()

    def write(self, text):
        n = super().write(text)
        if "queued" in text:
            self.queued.set()
        with self._cond:
            self.waits += [l for l in text.splitlines() if l.startswith("merge: waiting")]
            self._cond.notify_all()
        return n

    def said(self, n):
        """The first `n` wait lines once they are there (the event, no poll);
        fewer after the hang marker."""
        with self._cond:
            self._cond.wait_for(lambda: len(self.waits) >= n, kit_time.HANG)
            return list(self.waits)


class _Clock:
    """The injected kit clock of a merge (`merge(clock=...)`): RFC 3339 like
    `store.now()`, seven seconds further on every call -- a wait has a length
    without a wall clock."""

    def __init__(self):
        self.t = store.parse_at(store.now())
        self.lock = threading.Lock()

    def __call__(self):
        with self.lock:
            self.t += datetime.timedelta(seconds=7)
            return self.t.strftime("%Y-%m-%dT%H:%M:%SZ")


def merge_receipt(f, name):
    """The merge receipt of strand `name` (the only one of the test)."""
    d = os.path.join(f.wdir, "receipts", "merge-%s" % name)
    runs = sorted(os.listdir(d))
    with open(os.path.join(d, runs[-1], "merge.json"), encoding="utf-8") as fh:
        return json.load(fh)


class MergeWaitTest(MergeCase):
    """V2-P1196-3 (wave validation 2, GH #1199): two merges at once ended in
    exit 3 for the second and a retry by hand -- master moves before the lease
    is free, and a ref move wakes nobody. `merge --wait` blocks on the pool's
    wake file (`schedule._wake_open`, the wait of `gate --wait-admission` and
    `token take --wait`), which the release of the tree lease now rings."""

    def setUp(self):
        super().setUp()
        # Only a wake can end a round: the watchdog of a round is an hour here.
        env = mock.patch.dict(os.environ, {"MECLAW_TOKEN_WAIT_SECS": "3600"})
        env.start()
        self.addCleanup(env.stop)

    def waiting_merge(self, names, **kw):
        """`merge(names, wait=True)` in a thread on its own store connection."""
        got, out_, err = [], io.StringIO(), _Said()
        path = os.path.join(self.f.tmp, "p.db")

        def run():
            db = store.open(path)
            try:
                got.append(merge.merge(names, start=self.f.root, wave=WAVE, db=db, out_=out_,
                                       err=err, wait=True, **kw))
            except Exception as exc:  # noqa: BLE001 -- the test reads it
                got.append(exc)
            finally:
                store.close(db)
        return threading.Thread(target=run, daemon=True), got, out_, err

    def test_a_waiting_merge_lands_after_the_release_without_exit_3(self):
        f = self.f
        self.a_strand()
        s2 = f.strand("S2", 102, {"templates/b/bar.txt": "bar\n"})
        th, got, out2, err2 = self.waiting_merge(["S2"])
        real = merge.run_gate
        seen = {}

        def gate(tree, ctx):
            if ctx["label"] == "S1":
                th.start()
                seen["queued"] = err2.queued.wait(kit_time.HANG)
                seen["alive"] = th.is_alive()
            return real(tree, ctx)

        with mock.patch.object(merge, "run_gate", side_effect=gate):
            rc, _, err = f.merge(["S1"])
            self.assertEqual(rc, 0, err)
            s1_merge = f.head()
            # Only the release of the tree lease can end the wait before its
            # one-hour watchdog.
            th.join(kit_time.HANG)
        self.assertTrue(seen["queued"], "HANG: the second merge never queued")
        self.assertTrue(seen["alive"], "merge --wait returned while S1 held the tree")
        self.assertFalse(th.is_alive(), "HANG: the release did not wake merge --wait after "
                         "%.0f s" % kit_time.HANG)
        self.assertEqual(got, [0], err2.getvalue())
        self.assertIn("MERGED S2", out2.getvalue())
        self.assertEqual(f.state_of("S2"), "merged")
        # The waiter read master anew under the lease: S1's merge is its base.
        self.assertEqual(f.parents(), [s1_merge, s2["tip"]])
        self.assertIsNone(f.lease())
        self.assertEqual([e["strand"] for e in f.events("merge_queued")], ["S2"])

    def test_a_missing_review_verdict_ends_the_wait_at_once(self):
        # A cause no release resolves ends `--wait` as today: exit 1, no wait.
        f = self.f
        self.a_strand(report=False)
        f.report("S1", "welle-test/S1", 101, [], review=False)
        self.assertTrue(merge.take_tree(f.db, "meclaw", "S9"))
        th, got, _, err = self.waiting_merge(["S1"])
        th.start()
        th.join(kit_time.HANG)
        self.assertFalse(th.is_alive(), "merge --wait waits on a missing review")
        self.assertEqual(got, [merge.REFUSE_EXIT], err.getvalue())
        self.assertIn("no_review", err.getvalue())
        self.assertEqual(f.state_of("S1"), "refused")
        self.assertEqual(f.lease()["holder"], "S9")

    def ends_at_once(self, reason):
        """`merge --wait` on S1 (approved review) behind S9's lease: exit 1 with
        `reason` before any wait -- the limit is a minute, a wait would end
        queued (exit 3) only after it (re-check v2-fix-c I1)."""
        f = self.f
        self.assertTrue(merge.take_tree(f.db, "meclaw", "S9"))
        with mock.patch.dict(os.environ, {"MECLAW_ADMISSION_WAIT_SECS": "60",
                                          "MECLAW_TOKEN_WAIT_SECS": "60"}):
            th, got, _, err = self.waiting_merge(["S1"])
            th.start()
            th.join(kit_time.HANG)
        self.assertFalse(th.is_alive(), "HANG: merge --wait waits on %s" % reason)
        self.assertEqual(got, [merge.REFUSE_EXIT], err.getvalue())
        self.assertIn(reason, err.getvalue())
        self.assertNotIn("waiting for its release", err.getvalue())
        self.assertEqual(f.state_of("S1"), "refused")
        self.assertEqual(f.lease()["holder"], "S9")

    def test_a_missing_strand_receipt_ends_the_wait_at_once(self):
        # Re-check v2-fix-c I1: with `Verdikt: ja` the strand receipt was read
        # only under the lease -- rc 3 after the whole wait, not `no_strand_receipt`.
        self.a_strand(receipt=False)
        self.ends_at_once("no_strand_receipt")

    def test_a_red_strand_receipt_ends_the_wait_at_once(self):
        self.a_strand(verdict="RED")
        self.ends_at_once("strand_gate_not_green")

    def test_a_stale_pre_stamp_ends_the_wait_at_once(self):
        s = self.a_strand()
        write(s["wt"], "templates/a/untracked.txt", "dirty\n")
        self.ends_at_once("pre_stamp")

    def test_a_plan_only_branch_without_receipt_still_waits(self):
        # A plan-only branch merges without receipt (GH #1167 d): the lease
        # can change its end, so the pre-check lets it wait as before.
        f = self.f
        self.a_strand(receipt=False)
        self.assertTrue(merge.take_tree(f.db, "meclaw", "S9"))
        with mock.patch.object(merge, "plan_only", return_value=True), \
                mock.patch.dict(os.environ, {"MECLAW_ADMISSION_WAIT_SECS": "0.3",
                                             "MECLAW_TOKEN_WAIT_SECS": "0.05"}):
            th, got, _, err = self.waiting_merge(["S1"])
            th.start()
            th.join(kit_time.HANG)
        self.assertFalse(th.is_alive(), "HANG: merge --wait ignores its limit")
        self.assertEqual(got, [merge.QUEUED_EXIT], err.getvalue())
        self.assertEqual(f.state_of("S1"), "queued")

    def test_a_dequeue_that_passes_the_offer_on_wakes_the_waiter(self):
        # Re-check v2-fix-c M1: `dequeue` hands an offer to the next queued
        # merge; without its ring the waiter sleeps until its watchdog (an
        # hour here) although the lease is offered to it.
        f = self.f
        self.a_strand()
        f.strand("S2", 102, {"templates/b/bar.txt": "bar\n"})
        f.strand("S3", 103, {"templates/c/baz.txt": "baz\n"})
        self.assertTrue(merge.take_tree(f.db, "meclaw", "S9"))
        rc, _, err = f.merge(["S2"])
        self.assertEqual(rc, merge.QUEUED_EXIT, err)
        self.assertTrue(merge.release_tree(f.db, "meclaw", "S9"))
        self.assertEqual(f.lease()["offered_to"], "S2")
        th, got, out3, err3 = self.waiting_merge(["S3"])
        th.start()
        self.assertTrue(err3.queued.wait(kit_time.HANG), "HANG: S3 never queued")
        self.assertEqual(merge.dequeue(f.db, "meclaw", "S2"), 1)
        th.join(kit_time.HANG)
        self.assertFalse(th.is_alive(), "HANG: dequeue's offer did not wake merge --wait")
        self.assertEqual(got, [0], err3.getvalue())
        self.assertIn("MERGED S3", out3.getvalue())
        self.assertEqual(f.state_of("S3"), "merged")

    def test_an_offer_passed_on_without_a_row_of_its_own_rings(self):
        # Re-check f+g minor: `dequeue` rang only when it dropped a row of its
        # own; the offer it passes on (`nxt`) moves the next merge to the head
        # as well -- without a ring that waiter sleeps until its watchdog.
        f = self.f
        self.a_strand()
        f.strand("S2", 102, {"templates/b/bar.txt": "bar\n"})
        f.strand("S3", 103, {"templates/c/baz.txt": "baz\n"})
        self.assertTrue(merge.take_tree(f.db, "meclaw", "S9"))
        for name in ("S2", "S3"):
            rc, _, err = f.merge([name])
            self.assertEqual(rc, merge.QUEUED_EXIT, err)
        self.assertTrue(merge.release_tree(f.db, "meclaw", "S9"))
        self.assertEqual(f.lease()["offered_to"], "S2")
        with f.db:
            f.db.execute("UPDATE merges SET state='abandoned' WHERE strand='S2'")
        with mock.patch.object(merge, "_ring") as ring:
            self.assertEqual(merge.dequeue(f.db, "meclaw", "S2"), 0)
        self.assertEqual(f.lease()["offered_to"], "S3")
        self.assertEqual(ring.call_count, 1)

    def test_the_wait_has_a_limit_and_ends_queued(self):
        # MECLAW_ADMISSION_WAIT_SECS is the hang marker of the wait (as for
        # `gate --wait-admission`): past it the merge ends queued, exit 3.
        f = self.f
        self.a_strand()
        self.assertTrue(merge.take_tree(f.db, "meclaw", "S9"))
        with mock.patch.dict(os.environ, {"MECLAW_ADMISSION_WAIT_SECS": "0.3",
                                          "MECLAW_TOKEN_WAIT_SECS": "0.05"}):
            th, got, _, err = self.waiting_merge(["S1"])
            th.start()
            th.join(kit_time.HANG)
        self.assertFalse(th.is_alive(), "HANG: merge --wait ignores its limit")
        self.assertEqual(got, [merge.QUEUED_EXIT], err.getvalue())
        self.assertIn("MECLAW_ADMISSION_WAIT_SECS", err.getvalue())
        self.assertEqual(f.state_of("S1"), "queued")

    def test_a_waiting_merge_says_its_reason_and_each_new_position(self):
        # V2-A10 (wave validation 2): the merge of v2-fix-b waited 14 min
        # without a word. One line when the wait starts (reason, position),
        # one per change -- the change rings the wake file (`dequeue`, the
        # release, a take), no clock prints a line: the watchdog is an hour.
        f = self.f
        self.a_strand()
        f.strand("S2", 102, {"templates/b/bar.txt": "bar\n"})
        clock = _Clock()
        self.assertTrue(merge.take_tree(f.db, "meclaw", "S9"))
        rc, _, err = f.merge(["S2"])
        self.assertEqual(rc, merge.QUEUED_EXIT, err)
        th, got, out1, err1 = self.waiting_merge(["S1"], clock=clock)
        th.start()
        first = err1.said(1)
        self.assertEqual(first, ["merge: waiting -- the merge lease tree:master@meclaw is held "
                                 "by S9; S1 is #2 in the queue (queued, --wait)"], err1.getvalue())
        self.assertEqual(merge.dequeue(f.db, "meclaw", "S2"), 1)
        second = err1.said(2)
        self.assertEqual(second[1:], ["merge: waiting -- the merge lease tree:master@meclaw is "
                                      "held by S9; S1 is #1 in the queue (queued, --wait)"],
                         err1.getvalue())
        self.assertTrue(merge.release_tree(f.db, "meclaw", "S9"))
        th.join(kit_time.HANG)
        self.assertFalse(th.is_alive(), "HANG: the release did not wake merge --wait")
        self.assertEqual(got, [0], err1.getvalue())
        self.assertEqual(len(err1.waits), 2, err1.getvalue())
        # wait_s: from the queued row to the lease take, both on the kit clock.
        row = f.db.execute("SELECT at FROM merges WHERE strand='S1'").fetchone()
        taken = [r["at"] for r in f.db.execute(
            "SELECT at, data FROM journal WHERE event='lease_taken' ORDER BY seq")
            if json.loads(r["data"])["holder"] == "S1"]
        want = (store.parse_at(taken[0]) - store.parse_at(row["at"])).total_seconds()
        self.assertGreater(want, 0)
        self.assertEqual(merge_receipt(f, "S1")["wait_s"], want)

    def test_a_merge_that_never_waits_has_wait_s_0_and_no_wait_line(self):
        f = self.f
        self.a_strand()
        f.strand("S2", 102, {"templates/b/bar.txt": "bar\n"})
        for name, wait in (("S1", True), ("S2", False)):
            rc, _, err = f.merge([name], wait=wait, clock=_Clock())
            self.assertEqual(rc, 0, err)
            self.assertNotIn("merge: waiting", err)
            self.assertEqual(merge_receipt(f, name)["wait_s"], 0, name)

    def gate_cmd(self, extra):
        return mock.patch.dict(os.environ, {"MECLAW_PIPELINE_FAKE_GATE": "",
                                            "MECLAW_MERGE_GATE_CMD": extra})

    def test_the_candidate_runs_wait_for_its_lane_is_said_and_counted(self):
        # The second wait: the candidate run on its lane behind other runs'
        # cargo lock (the gate's `lock-wait` row) -- said, and part of wait_s.
        f = self.f
        self.a_strand()
        cmd = ("sh -c 'echo gate: rev = $(git -C {tree} rev-parse HEAD); "
               "echo \"GATE lock-wait [42s behind other runs] 42s NOTE\"; "
               "echo GATE-SUMMARY strand $(git -C {tree} rev-parse --short=7 HEAD) 1/1 1s GREEN'")
        with self.gate_cmd(cmd):
            rc, _, err = f.merge(["S1"], clock=_Clock())
        self.assertEqual(rc, 0, err)
        self.assertIn("merge: waited 42s for the lane of the candidate run (behind other "
                      "runs' cargo lock)", err)
        self.assertEqual(merge_receipt(f, "S1")["wait_s"], 42)

    def test_no_lane_for_the_candidate_run_names_the_reason(self):
        # The merge gate takes no queue place (kit find h): queued -> exit 3,
        # the reason went into the gate log only.
        f = self.f
        self.a_strand()
        cmd = ("sh -c 'echo \"gate: queued -- lane build01: busy -- no queue place: a merge "
               "gate does not wait for an offer\" >&2; exit 3'")
        with self.gate_cmd(cmd):
            rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, merge.REFUSE_EXIT, err)
        self.assertIn("no lane free for the candidate run: lane build01: busy", err)

    def test_wait_on_the_command_line(self):
        parser = argparse.ArgumentParser()
        merge.register(parser.add_subparsers())
        for argv, want in ((["merge", "--strand", "S1", "--wait"], True),
                           (["merge", "--strand", "S1"], False)):
            args = parser.parse_args(argv)
            with mock.patch.object(merge, "merge", return_value=0) as call:
                self.assertEqual(args.func(args), 0)
            self.assertEqual(call.call_args.kwargs["wait"], want, argv)


class MergeStepsTest(MergeCase):
    """V2-A10 (h, wave validation 2): the merge of v2-fix-b held tree:master
    826 s (journal seq 116541 lease_taken 00:03:20Z -> 116542 merge_gated
    00:17:06Z) and said nothing in between -- which step ate the time stood
    nowhere. Each step now says one line when it ends, and the merge receipt
    lists them; the kit clock is injected (`_Clock`, +7 s a call), so every
    duration is exact: the lease step reads the clock three times (its start,
    the take, its end), every later one once (its end = the next start)."""

    STEPS = ["lease", "preconditions", "candidate", "gate", "ref-update", "close"]

    def step_lines(self, err):
        return [l for l in err.splitlines() if l.startswith("merge: step ")]

    def test_every_step_says_its_line_in_order_and_the_receipt_lists_them(self):
        f = self.f
        self.a_strand()
        rc, _, err = f.merge(["S1"], clock=_Clock())
        self.assertEqual(rc, 0, err)
        steps = merge_receipt(f, "S1")["steps"]
        self.assertEqual([s["name"] for s in steps], self.STEPS, err)
        self.assertEqual([s["secs"] for s in steps], [14, 7, 7, 7, 7, 7], err)
        self.assertEqual(self.step_lines(err), ["merge: step %s %s %ds" % (
            s["name"], s["start"], s["secs"]) for s in steps])
        for a, b in zip(steps, steps[1:]):
            # contiguous: no second of the merge falls between two steps
            self.assertEqual(store.parse_at(b["start"]) - store.parse_at(a["start"]),
                             datetime.timedelta(seconds=a["secs"]), (a, b))

    def test_a_red_step_is_the_last_line(self):
        f = self.f
        self.a_strand(receipt=False)
        rc, _, err = f.merge(["S1"], clock=_Clock())
        self.assertEqual(rc, merge.REFUSE_EXIT, err)
        lines = self.step_lines(err)
        self.assertEqual([l.split()[2] for l in lines], ["lease", "preconditions"], err)
        self.assertRegex(lines[0], r"^merge: step lease \S+Z 14s$")
        self.assertRegex(lines[1], r"^merge: step preconditions \S+Z 7s RED$")

    def test_a_red_candidate_gate_is_the_last_line(self):
        f = self.f
        self.a_strand()
        with mock.patch.object(merge, "run_gate", return_value={
                "verdict": "RED", "line": "GATE-SUMMARY strand x 0/1 1s RED"}):
            rc, _, err = f.merge(["S1"], clock=_Clock())
        self.assertEqual(rc, merge.REFUSE_EXIT, err)
        self.assertEqual([l.split()[2:3] + l.split()[4:] for l in self.step_lines(err)],
                         [["lease", "14s"], ["preconditions", "7s"], ["candidate", "7s"],
                          ["gate", "7s", "RED"]], err)


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


GOLDEN = "plans/composition-reference/golden/g.manifest"
REF = "templates/g/sub/config.json"

# The fake generator: refuses without its switch (as `write_golden_manifests`
# does), logs each call, and writes the golden from the tree it was handed --
# one line per ref marker, `<rel>\t<sha256>`, like the gh277 manifest.
GENERATOR = """import hashlib, json, os, sys
tree, log = sys.argv[1], sys.argv[2]
with open(log, "a") as fh:
    fh.write(json.dumps({"tree": tree, "argv": sys.argv[3:],
                         "switch": os.environ.get("GH277_WRITE_GOLDEN"),
                         "other": os.environ.get("MECLAW_GH935_BLESS")}) + "\\n")
if os.environ.get("GH277_WRITE_GOLDEN") != "1":
    sys.exit("refusing to re-baseline the goldens without GH277_WRITE_GOLDEN=1")
if os.environ.get("FAKE_GENERATOR_FAILS"):
    sys.exit("the generator broke")
with open(os.path.join(tree, %r), "rb") as fh:
    digest = hashlib.sha256(fh.read()).hexdigest()
with open(os.path.join(tree, %r), "w") as fh:
    fh.write("sub/config.json\\t%%s\\n" %% digest)
""" % (REF, GOLDEN)


def golden_of(root):
    """What the fake generator writes for `root`."""
    import hashlib
    with open(os.path.join(root, REF), "rb") as fh:
        return "sub/config.json\t%s\n" % hashlib.sha256(fh.read()).hexdigest()


def golden_fixture(case):
    """The composite `g` (its `sub/` cell a ref to `a@1.0.0`), its committed
    golden and the fake generator on disk; `case.gen_log` its call log."""
    f = case.f
    case.gen_log = os.path.join(f.tmp, "generator.jsonl")
    write(f.tmp, "generator.py", GENERATOR)
    f.master_commit({
        "templates/g/template.json": template_json("g", "4.0.0"),
        REF: json.dumps({"cell": {"type": "ref", "template": "a@1.0.0"}}, indent=2) + "\n",
        "templates/g/brain/config.json": json.dumps({"cell": {"type": "llm"}}) + "\n",
    }, msg="composite g")
    f.master_commit({GOLDEN: golden_of(f.root)}, msg="golden g")


class GoldenTest(MergeCase):
    """V2-A17 (wave validation 2): the merge point's cascade raised
    `collector@5.1.1 -> 5.1.2` in `templates/{talky,cogny}/collector/config.json`,
    and the hash goldens `plans/composition-reference/golden/*.manifest` did
    not follow -- candidate gate ac1e8c3 17/18 RED in the two byte-identity
    tests. A strand can never write them right (rule 6: numbers only at the
    merge point), so the merge point runs the goldens' own generator on the
    candidate when its cascade moved a file inside a golden's closure, and the
    rewritten goldens go into the merge commit before the gate judges it byte
    for byte.

    Fixture: a composite `g` whose `sub/` cell is a ref to `a@1.0.0` and a
    committed golden `g.manifest`; the generator is a fake behind
    `MECLAW_MERGE_GOLDEN_CMD`."""

    plans_tracked = True

    def setUp(self):
        super().setUp()
        golden_fixture(self)
        self.cmd = "%s %s {tree} %s {switch} {filter}" % (
            sys.executable, os.path.join(self.f.tmp, "generator.py"), self.gen_log)

    def calls(self):
        try:
            with open(self.gen_log, encoding="utf-8") as fh:
                return [json.loads(line) for line in fh if line.strip()]
        except OSError:
            return []

    def merge_with(self, names, **env):
        values = {"MECLAW_MERGE_GOLDEN_CMD": self.cmd, "MECLAW_GH935_BLESS": "1"}
        values.update(env)
        with mock.patch.dict(os.environ, values):
            return self.f.merge(names)

    def test_a_cascade_into_a_golden_closure_rewrites_the_golden_in_the_merge_commit(self):
        f = self.f
        old = read(f.root, GOLDEN)
        base = f.head()
        s = self.a_strand(in_branch=True)
        rc, _, err = self.merge_with(["S1"])
        self.assertEqual(rc, 0, err)
        # the cascade raised the ref pin -- the golden's input moved
        self.assertIn('"a@1.0.1"', read(f.root, REF))
        # ... and the golden followed, in the ONE merge commit the gate judged
        self.assertNotEqual(read(f.root, GOLDEN), old)
        self.assertEqual(read(f.root, GOLDEN), golden_of(f.root))
        self.assertEqual(f.parents(), [base, s["tip"]])
        self.assertIn(GOLDEN, git(f.root, "show", "--name-only", "--format=", "HEAD").split())
        self.assertIn("Goldens: %s rewritten after the cascade (GH277_WRITE_GOLDEN)" % GOLDEN,
                      f.body())
        self.assertEqual([c["rev"] for c in f.gate_calls()], [f.head()])
        # the generator ran once, in the candidate, with its own switch only
        calls = self.calls()
        self.assertEqual(len(calls), 1, calls)
        self.assertEqual(calls[0]["tree"], os.path.join(f.state, "meclaw", "pipeline", "merge",
                                                        "S1"))
        self.assertEqual(calls[0]["switch"], "1")
        self.assertIsNone(calls[0]["other"])
        self.assertEqual(calls[0]["argv"][0], "GH277_WRITE_GOLDEN")
        self.assertIn("write_golden_manifests", calls[0]["argv"][1])
        self.assertIn("merge: goldens rewritten after the cascade: %s" % GOLDEN, err)
        self.assertEqual(merge_receipt(f, "S1")["goldens"], [GOLDEN])

    def test_a_cascade_outside_every_golden_closure_runs_no_generator(self):
        f = self.f
        old = read(f.root, GOLDEN)
        f.strand("S1", 101, {"templates/b/foo.txt": "foo\n"}, in_branch=True)
        rc, _, err = self.merge_with(["S1"])
        self.assertEqual(rc, 0, err)
        self.assertEqual(version_of(f.root, "b"), "2.0.1")
        self.assertEqual(self.calls(), [])
        self.assertEqual(read(f.root, GOLDEN), old)
        self.assertNotIn("Goldens:", f.body())
        self.assertNotIn("goldens", merge_receipt(f, "S1"))

    def test_a_generator_that_writes_the_same_bytes_leaves_the_commit_alone(self):
        # `g` itself is bumped (its template.json moves, a file the manifest
        # drops): the generator runs, writes the same golden, nothing to amend.
        f = self.f
        old = read(f.root, GOLDEN)
        f.strand("S1", 101, {"templates/g/notes.txt": "n\n"}, in_branch=True)
        rc, _, err = self.merge_with(["S1"])
        self.assertEqual(rc, 0, err)
        self.assertEqual(version_of(f.root, "g"), "4.0.1")
        self.assertEqual(len(self.calls()), 1)
        self.assertEqual(read(f.root, GOLDEN), old)
        self.assertNotIn("Goldens:", f.body())
        self.assertEqual(merge_receipt(f, "S1")["goldens"], [])

    def test_a_failing_generator_refuses_the_merge(self):
        f = self.f
        base = f.head()
        self.a_strand(in_branch=True)
        rc, _, err = self.merge_with(["S1"], FAKE_GENERATOR_FAILS="1")
        self.assertEqual(rc, merge.REFUSE_EXIT, err)
        self.assertIn("golden_failed", err)
        self.assertIn("the generator broke", err)
        self.assertEqual(f.head(), base)
        self.assertEqual(f.gate_calls(), [])
        self.assertEqual(f.state_of("S1"), "refused")
        self.assertIsNone(f.lease())

    def test_the_closure_follows_the_refs_of_the_golden_names(self):
        f = self.f
        closure = merge.golden_closure(f.root, merge.GOLDENS[0])
        self.assertEqual(closure, ["templates/a/", "templates/g/"])

    def test_the_default_generator_is_the_lane_test_of_the_candidate(self):
        argv = merge.golden_argv("/c", {"holder": "welle-x/S1", "host": "build03"},
                                 merge.GOLDENS[0])
        # the kit's one Python with -I: the narrow point refuses a bare python3
        # (measured: `lane build02 unreachable ... started without -I`)
        self.assertEqual(argv, [
            "/usr/bin/python3", "-I", "/c/scripts/pipeline.py", "test",
            "--strand", "welle-x/S1", "--host", "build03",
            "test(/(^|::)write_golden_manifests$/)",
            "-p", "meclaw-colony", "--run-ignored", "only"])


# The token check of `pipeline.py test` in front of the fake generator: the
# real `gate._lease_gate` against the fixture's store (MECLAW_PIPELINE_DB) --
# the step a lane run of the generator passes before anything travels --
# then the fake writer (GENERATOR) with the words it gets in GoldenTest.
LEASED_GENERATOR = """import json, os, subprocess, sys
scripts, tree, log, holder, host = sys.argv[1:6]
sys.path.insert(0, scripts)
from pipeline import gate, schedule, store
if os.environ.get("GOLDEN_TEST_WAITER"):
    # a strand queues for a lane while the generator runs (re-check RC-GK2 I1)
    schedule.lease_take(store.open(), os.environ["GOLDEN_TEST_WAITER"], pid=os.getppid())
try:
    lane = gate._lease_gate(store.open(), tree, holder, host, sys.stdout, sys.stderr)
except gate._TestExit as ex:
    sys.stderr.write(str(ex) + "\\n")
    sys.exit(ex.code)
with open(log, "a") as fh:
    fh.write(json.dumps({"lane": lane, "holder": holder}) + "\\n")
sys.exit(subprocess.run([sys.executable, sys.argv[6], tree] + sys.argv[7:]).returncode)
"""


# The merge gate's admission, real: `schedule.admit` as `gate._run_lane` calls
# it (no queue place under MECLAW_GATE_MODE=merge), against the fixture's
# store; queued -> exit 3 without a summary (as the engine), else the run is
# closed GREEN on the lane it got.
ADMIT_GATE = """import json, os, subprocess, sys
scripts, tree, holder, log = sys.argv[1:5]
sys.path.insert(0, scripts)
from pipeline import schedule, store
db = store.open()
st = {"name": "precheck", "scope": "precheck", "cargo": 0, "stage": ["S", "I", "R"],
      "needs": {"cpu": 1, "cargo": 0}, "inputs": [], "cmds": [{"argv": ["true"], "cwd": ""}],
      "env": "none", "reason": [], "timeout_min": 10, "after": [], "run": True, "keep_ws": True}
plan = {"mode": "strand", "rev": "a" * 40, "base": "b" * 40, "stations": [st],
        "plan_reason": {"precheck": ["t"]}}
adm = schedule.admit(db, plan, repo="meclaw", mode="strand", strand=holder,
                     wave=holder.split("/")[0], base="b" * 40, lane_pref=None, now=None,
                     wait=False, queue=os.environ.get("MECLAW_GATE_MODE", "") != "merge")
if isinstance(adm, schedule.Queued):
    with open(log, "a") as fh:
        fh.write(json.dumps({"queued": adm.reason, "holder": holder}) + "\\n")
    sys.stderr.write("gate: queued -- %s\\n" % adm.reason)
    sys.exit(3)
with open(log, "a") as fh:
    fh.write(json.dumps({"lane": adm.lane, "holder": holder}) + "\\n")
schedule.close(db, adm, {"verdict": "GREEN"})
rev = subprocess.run(["git", "-C", tree, "rev-parse", "HEAD"], capture_output=True,
                     text=True).stdout.strip()
print("gate: rev = %s" % rev)
print("GATE-SUMMARY strand %s 1/1 1s GREEN" % rev[:7])
"""


class _Runner:
    """A registered runner of the pool (`schedule.sync_runners`)."""

    def __init__(self, name):
        self.name, self.ssh, self.bmc, self.root = name, "t@%s.invalid" % name, None, "/srv/target"
        self.transport, self.cls, self.threads, self.jobs = "ssh", "lane", 24, None
        self.slots, self.cores = None, 0

class GoldenLaneTest(MergeCase):
    """V2-A17 follow-up (#1199, live merge G-a 12:15): a merge WITHOUT `--host`
    found the golden hits and ran the generator `pipeline.py test --strand
    <holder>` -- whose token check refused it with exit 3 ("test runs on a
    lane -- take one"): the merge holds no lane lease, its gate gets its lane
    from the admission. The generator now runs on a lane the merge point
    leases for it from the same armed pool (`--host`, else a lane the pool
    leaves it; no queue place, like the merge gate) and gives back after the
    run -- never past the token (`MECLAW_STRAND_TOKEN_SKIP`)."""

    plans_tracked = True
    HOLDER = "welle-test/S1"

    def setUp(self):
        super().setUp()
        f = self.f
        from pipeline import schedule
        self.schedule = schedule
        golden_fixture(self)
        write(f.tmp, "lanes", "lane-a test@lane-a.invalid\nlane-b test@lane-b.invalid\n")
        res = schedule.lease_init(f.db, lanes=["lane-a", "lane-b"],
                                  lanes_file=os.path.join(f.tmp, "lanes"))
        self.assertEqual(res.code, 0, res.err)
        self.lane_log = os.path.join(f.tmp, "lanes.jsonl")
        write(f.tmp, "leased.py", LEASED_GENERATOR)
        self.cmd = "%s %s %s {tree} %s {holder} {host} %s %s {switch} {filter}" % (
            sys.executable, os.path.join(f.tmp, "leased.py"), SCRIPTS, self.lane_log,
            os.path.join(f.tmp, "generator.py"), self.gen_log)

    def merge_with(self, names, **kw):
        with mock.patch.dict(os.environ, {"MECLAW_MERGE_GOLDEN_CMD": self.cmd}):
            return self.f.merge(names, **kw)

    def lanes_seen(self):
        try:
            with open(self.lane_log, encoding="utf-8") as fh:
                return [json.loads(line) for line in fh if line.strip()]
        except OSError:
            return []

    def pool(self):
        st = self.schedule._load(self.f.db)
        return ([(h["strand"], h.get("lane")) for h in st["holders"]],
                [w["strand"] for w in st["waiting"]])

    def test_a_merge_without_host_runs_the_generator_on_a_lane_it_leases(self):
        f = self.f
        base = f.head()
        s = self.a_strand(in_branch=True)
        self.assertEqual(self.pool(), ([], []))
        rc, _, err = self.merge_with(["S1"])
        self.assertEqual(rc, 0, err)
        # the token check of the generator found the gate holder's lease
        seen = self.lanes_seen()
        self.assertEqual(len(seen), 1, err)
        self.assertEqual(seen[0]["holder"], self.HOLDER)
        self.assertIn(seen[0]["lane"], ("lane-a", "lane-b"))
        # ... the golden followed in the ONE merge commit
        self.assertEqual(read(f.root, GOLDEN), golden_of(f.root))
        self.assertEqual(f.parents(), [base, s["tip"]])
        self.assertEqual(merge_receipt(f, "S1")["goldens"], [GOLDEN])
        # ... and the lease went back: no holder, no queue place
        self.assertEqual(self.pool(), ([], []))

    def test_a_merge_with_host_runs_the_generator_on_that_lane(self):
        self.a_strand(in_branch=True)
        rc, _, err = self.merge_with(["S1"], host="lane-b")
        self.assertEqual(rc, 0, err)
        self.assertEqual([c["lane"] for c in self.lanes_seen()], ["lane-b"])
        self.assertEqual(self.pool(), ([], []))

    def test_a_lease_the_holder_already_holds_stays_after_the_merge(self):
        self.a_strand(in_branch=True)
        res = self.schedule.lease_take(self.f.db, self.HOLDER, lane="lane-a")
        self.assertEqual(res.code, 0, res.err)
        rc, _, err = self.merge_with(["S1"])
        self.assertEqual(rc, 0, err)
        self.assertEqual([c["lane"] for c in self.lanes_seen()], ["lane-a"])
        self.assertEqual(self.pool(), ([(self.HOLDER, "lane-a")], []))

    def test_no_free_lane_refuses_the_merge_and_takes_no_queue_place(self):
        f = self.f
        base = f.head()
        self.a_strand(in_branch=True)
        for other, ln in (("welle-test/X", "lane-a"), ("welle-test/Y", "lane-b")):
            res = self.schedule.lease_take(f.db, other, lane=ln, pid=os.getpid())
            self.assertEqual(res.code, 0, res.err)
        rc, _, err = self.merge_with(["S1"])
        self.assertEqual(rc, merge.REFUSE_EXIT, err)
        self.assertIn("golden_failed", err)
        self.assertEqual(self.lanes_seen(), [])
        self.assertEqual(f.head(), base)
        self.assertEqual(f.gate_calls(), [])
        self.assertEqual(self.pool(), ([("welle-test/X", "lane-a"), ("welle-test/Y", "lane-b")],
                                       []))

    def admit_gate(self):
        """The merge gate as the engine admits it (ADMIT_GATE), its log."""
        f = self.f
        self.schedule.sync_runners(f.db, [_Runner("lane-a"), _Runner("lane-b")],
                                   states={"lane-a": "idle", "lane-b": "idle"})
        write(f.tmp, "admit_gate.py", ADMIT_GATE)
        self.admit_log = os.path.join(f.tmp, "admit.jsonl")
        return "%s %s %s {tree} {holder} %s" % (
            sys.executable, os.path.join(f.tmp, "admit_gate.py"), SCRIPTS, self.admit_log)

    def admitted(self):
        try:
            with open(self.admit_log, encoding="utf-8") as fh:
                return [json.loads(line) for line in fh if line.strip()]
        except OSError:
            return []

    def test_the_golden_lease_spans_the_merge_gate_while_a_strand_queues(self):
        # Re-check RC-GK2 I1: the lease went back after the generator, a strand
        # that queued meanwhile got the lane offered, and the merge gate (no
        # queue place) ended without a lane -- gate_aborted. The lease now
        # spans the gate: the gate is admitted on it, then it goes back.
        f = self.f
        self.a_strand(in_branch=True)
        res = self.schedule.lease_take(f.db, "welle-test/X", lane="lane-a", pid=os.getpid())
        self.assertEqual(res.code, 0, res.err)
        env = {"MECLAW_MERGE_GOLDEN_CMD": self.cmd, "GOLDEN_TEST_WAITER": "welle-test/W",
               "MECLAW_PIPELINE_FAKE_GATE": "", "MECLAW_MERGE_GATE_CMD": self.admit_gate()}
        with mock.patch.dict(os.environ, env):
            rc, _, err = f.merge(["S1"])
        self.assertEqual([c["lane"] for c in self.lanes_seen()], ["lane-b"])
        self.assertEqual(self.admitted(), [{"lane": "lane-b", "holder": self.HOLDER}], err)
        self.assertEqual(rc, 0, err)
        holders, waiting = self.pool()
        self.assertEqual(holders, [("welle-test/X", "lane-a")])
        self.assertEqual(waiting, ["welle-test/W"])

    def test_a_batch_runs_the_generator_on_the_lane_a_member_holds(self):
        # Re-check RC-GK2 I2: a batch `<wave>/S1+S2` speaks for the lease of a
        # member (schedule `_batch_speaker`) -- the merge gate is admitted on
        # S1's lane, so the generator runs there too, under S1's lease, and
        # takes no second lane (none was free: golden_failed before).
        f = self.f
        self.a_strand(in_branch=True)
        f.strand("S2", 102, {"templates/b/bar.txt": "bar\n"}, in_branch=True)
        for holder, ln in ((self.HOLDER, "lane-a"), ("welle-test/Z", "lane-b")):
            res = self.schedule.lease_take(f.db, holder, lane=ln, pid=os.getpid())
            self.assertEqual(res.code, 0, res.err)
        env = {"MECLAW_MERGE_GOLDEN_CMD": self.cmd, "MECLAW_PIPELINE_FAKE_GATE": "",
               "MECLAW_MERGE_GATE_CMD": self.admit_gate()}
        with mock.patch.dict(os.environ, env):
            rc, _, err = f.merge(["S1", "S2"])
        self.assertEqual(rc, 0, err)
        self.assertEqual(self.lanes_seen(), [{"lane": "lane-a", "holder": self.HOLDER}])
        self.assertEqual(self.admitted(), [{"lane": "lane-a", "holder": "welle-test/S1+S2"}])
        self.assertEqual(self.pool(), ([(self.HOLDER, "lane-a"), ("welle-test/Z", "lane-b")],
                                       []))

    def test_a_failing_generator_gives_the_lane_back(self):
        # Re-check RC-GK2 I3: the release in every exit of the golden step --
        # a generator that fails refuses the merge and leaves no lease.
        f = self.f
        base = f.head()
        self.a_strand(in_branch=True)
        with mock.patch.dict(os.environ, {"MECLAW_MERGE_GOLDEN_CMD": self.cmd,
                                          "FAKE_GENERATOR_FAILS": "1"}):
            rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, merge.REFUSE_EXIT, err)
        self.assertIn("golden_failed", err)
        self.assertEqual(len(self.lanes_seen()), 1, err)
        self.assertEqual(f.head(), base)
        self.assertEqual(f.gate_calls(), [])
        self.assertEqual(self.pool(), ([], []))



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
    touches `turnaround.POST_GATE` of its wave (`plans/<wave>/` berichte/,
    straenge/, RESTLISTE.md, recheck/) between receipt and tip passes."""

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

    # GH #1199 (e): v2-p1082 refused `receipt_not_branch_head` with only
    # plans/<wave>/RESTLISTE.md changed after the gate -- the delta check of
    # Konvergenz A (PREAMBLE § 6.13) writes it there, as it writes recheck/.
    # The ONE post-gate list (`turnaround.POST_GATE`) carries both.
    def test_merge_accepts_a_restliste_commit_after_the_gate(self):
        f = self.f
        s = self.a_strand(in_branch=True)
        tip = self.report_commit(s, "plans/%s/RESTLISTE.md" % WAVE, "# Restliste\n- N1\n")
        rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        self.assertEqual(f.parents()[1], tip)

    def test_merge_accepts_a_recheck_script_after_the_gate(self):
        f = self.f
        s = self.a_strand(in_branch=True)
        tip = self.report_commit(s, "plans/%s/recheck/skripte/S1/klasse.py" % WAVE,
                                 "import sys\nsys.exit(0)\n")
        rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        self.assertEqual(f.parents()[1], tip)

    # GH #1199 (e), Koordinator-3 ruling "100644 on both sides": a re-check
    # script committed 100755 after the gate re-gates. The wave commits them
    # 100644 and runs them with bash/python3 (PREAMBLE § 6.13); `strand.sh pre`
    # says so first (precheck `recheck-mode`). This pin locks the ruling.
    def test_merge_refuses_an_executable_recheck_script_after_the_gate(self):
        f = self.f
        s = self.a_strand(in_branch=True)
        rel = "plans/%s/recheck/skripte/S1/run_all.sh" % WAVE
        write(s["wt"], rel, "#!/usr/bin/env bash\nexit 0\n")
        os.chmod(os.path.join(s["wt"], rel), 0o755)
        git(s["wt"], "add", "-A")
        git(s["wt"], "commit", "-q", "-m", "S1: recheck script after the gate")
        rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, merge.REFUSE_EXIT, err)
        self.assertIn("receipt_not_branch_head", err)
        self.assertIn("mode 100755", err)

    def test_merge_refuses_a_script_commit_after_the_gate(self):
        f = self.f
        s = self.a_strand(in_branch=True)
        self.report_commit(s, "scripts/later.py", "X = 1\n")
        rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, merge.REFUSE_EXIT, err)
        self.assertIn("receipt_not_branch_head", err)

    def test_merge_refuses_a_plans_file_outside_the_post_gate_list(self):
        for rel in ("plans/PREAMBLE.md", "plans/%s/README.md" % WAVE,
                    "plans/%s/RESTLISTE.md.bak" % WAVE):
            with self.subTest(rel=rel):
                self.f = f = Fixture(self.gate, self.plans_tracked)  # a fresh repo per case
                self.addCleanup(f.close)
                s = self.a_strand(in_branch=True)
                self.report_commit(s, rel, "later\n")
                rc, _, err = f.merge(["S1"])
                self.assertEqual(rc, merge.REFUSE_EXIT, err)
                self.assertIn("receipt_not_branch_head", err)
                self.assertIn(rel, err)


if __name__ == "__main__":
    unittest.main()
