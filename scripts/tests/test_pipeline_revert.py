"""Tests of the revert operation (`scripts/pipeline/revert.py`, W9).

Fixture of `test_pipeline_merge`: a strand changes template `a` and is merged
(`a` 1.0.0 -> 1.0.1, its pinner `c` follows); the revert takes the merge back
through the same door -- lease, candidate, numbers, gate, fast-forward.
"""

import io
import json
import os
import pathlib
import sys
import unittest
from unittest import mock

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS))

from pipeline import merge, revert, store  # noqa: E402

from . import test_pipeline_merge as fx  # noqa: E402
from scripts.tests import store_isolation  # noqa: E402

# gate-selftest loads every module into one process: no fixture event of this
# module may reach the metrics target of the caller's environment.
_METRICS_URL = mock.patch.dict(os.environ, {"MECLAW_PIPELINE_METRICS_URL": ""})


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()
    _METRICS_URL.start()


def tearDownModule():  # noqa: N802 -- unittest's name
    _METRICS_URL.stop()
    store_isolation.restore()


class RevertCase(unittest.TestCase):

    def setUp(self):
        self.f = fx.Fixture()
        self.addCleanup(self.f.close)
        self.f.strand("S1", 101, {"templates/a/foo.txt": "foo\n"})
        rc, _, err = self.f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        self.merged = self.f.head()
        self.assertEqual(fx.version_of(self.f.root, "a"), "1.0.1")

    def revert(self, target="S1", reason="test", **kw):
        out, err = io.StringIO(), io.StringIO()
        rc = revert.revert(target, reason, start=self.f.root, wave=fx.WAVE, db=self.f.db,
                           out_=out, err=err, **kw)
        return rc, out.getvalue(), err.getvalue()


class RevertTest(RevertCase):

    def test_revert_bumps_third_digit_and_reopens_issue(self):
        f = self.f
        rc, out, err = self.revert()
        self.assertEqual(rc, 0, err)
        root = f.root
        # A new number on top of master's, never the old one back.
        self.assertEqual(fx.version_of(root, "a"), "1.0.2")
        self.assertEqual(fx.version_of(root, "c"), "0.3.2")
        self.assertFalse(os.path.exists(os.path.join(root, "templates/a/foo.txt")))
        self.assertEqual(json.loads(fx.read(root, fx.SCAFFOLD))["template"], "a@1.0.2")
        self.assertEqual(json.loads(fx.read(root, "templates/c/template.json"))["uses"],
                         ["a@1.0.2"])
        self.assertIn("runs a@1.0.2 today", fx.read(root, "docs/x.md"))
        self.assertEqual(f.subject(), "Revert S1 (test)")
        self.assertIn("Versions: a 1.0.1 -> 1.0.2 (fix), c 0.3.1 -> 0.3.2 (fix)", f.body())
        self.assertEqual(f.parents(), [self.merged])
        self.assertEqual(fx.git(root, "status", "--porcelain"), "")
        self.assertEqual(f.state_of("S1"), "reverted")
        self.assertIsNone(f.lease())
        reopen = [c for c in f.gh_calls() if c[:2] == ["issue", "reopen"]]
        self.assertEqual(len(reopen), 1)
        self.assertEqual(reopen[0][2:4], ["101", "--comment"])
        self.assertIn("test", reopen[0][4])
        self.assertIn(self.merged[:9], reopen[0][4])

    def test_revert_writes_receipt_and_event(self):
        f = self.f
        rc, out, err = self.revert(target=self.merged[:12])
        self.assertEqual(rc, 0, err)
        sha = f.head()
        archive = os.path.join(f.wdir, "receipts", "revert-S1")
        runs = os.listdir(archive)
        self.assertEqual(len(runs), 1)
        self.assertTrue(runs[0].endswith("-" + sha[:8]), runs)
        d = os.path.join(archive, runs[0])
        doc = json.loads(fx.read(d, "revert.json"))
        self.assertEqual((doc["strand"], doc["reverted"], doc["revert_sha"], doc["reason"]),
                         ("S1", self.merged, sha, "test"))
        self.assertEqual(doc["gate"]["verdict"], "GREEN")
        self.assertIn("REVERT S1 %s" % sha[:9], fx.read(d, "summary.txt"))
        done = f.events("revert_done")
        self.assertEqual(len(done), 1)
        self.assertEqual((done[0]["strand"], done[0]["merge_sha"], done[0]["base"]),
                         ("S1", sha, self.merged))
        self.assertIn("REVERTED S1", out)
        # The revert's candidate went through the gate like a merge.
        self.assertEqual(f.gate_calls()[-1]["label"], "revert-S1")
        self.assertFalse(os.path.exists(merge.candidate_path("revert-S1")))

    def test_revert_red_keeps_master_and_the_merge(self):
        f = self.f
        with mock.patch.dict(os.environ, {"MECLAW_PIPELINE_FAKE_GATE": "red"}):
            rc, _, err = self.revert()
        self.assertEqual(rc, merge.REFUSE_EXIT)
        self.assertIn("gate_red", err)
        self.assertEqual(f.head(), self.merged)
        self.assertEqual(f.state_of("S1"), "merged")
        self.assertIsNone(f.lease())
        self.assertEqual(f.events("revert_done"), [])
        self.assertEqual([c for c in f.gh_calls() if c[:2] == ["issue", "reopen"]], [])


class ReentrantRevertTest(RevertCase):

    def test_second_revert_of_the_same_strand_is_refused(self):
        f = self.f
        self.assertTrue(merge.take_tree(f.db, "meclaw", "revert:S1"))
        rc, _, err = self.revert()
        self.assertEqual(rc, merge.REFUSE_EXIT)
        self.assertIn("already_merging", err)
        self.assertEqual(store.lease_who(f.db, merge.resource("meclaw"))["holder"], "revert:S1")
        self.assertEqual(f.head(), self.merged)


class HookRevertTest(RevertCase):

    def test_revert_commit_runs_the_pre_commit_hook(self):
        f = self.f
        hook = os.path.join(f.root, ".git", "hooks", "pre-commit")
        fx.write(f.root, ".git/hooks/pre-commit", "#!/bin/sh\necho guard refused >&2\nexit 1\n")
        os.chmod(hook, 0o755)
        rc, _, err = self.revert()
        self.assertEqual(rc, merge.REFUSE_EXIT)
        self.assertIn("guard refused", err)
        self.assertEqual(f.head(), self.merged)


class OlderMergeTest(RevertCase):
    """F2: a later merge moved the version, pin and CHANGELOG lines of S1."""

    def later_merge(self, files):
        self.f.strand("S2", 102, files)
        rc, _, err = self.f.merge(["S2"])
        self.assertEqual(rc, 0, err)
        self.assertEqual(fx.version_of(self.f.root, "a"), "1.0.2")

    def test_revert_older_merge_renumbers_over_later_bumps(self):
        f = self.f
        self.later_merge({"templates/a/bar.txt": "bar\n"})
        later = f.head()
        rc, out, err = self.revert()
        self.assertEqual(rc, 0, err)
        root = f.root
        # S1's content is gone, S2's stays; the numbers go on from master's.
        self.assertFalse(os.path.exists(os.path.join(root, "templates/a/foo.txt")))
        self.assertEqual(fx.read(root, "templates/a/bar.txt"), "bar\n")
        self.assertEqual(fx.version_of(root, "a"), "1.0.3")
        self.assertEqual(fx.version_of(root, "c"), "0.3.3")
        self.assertEqual(json.loads(fx.read(root, fx.SCAFFOLD))["template"], "a@1.0.3")
        self.assertEqual(json.loads(fx.read(root, "templates/c/template.json"))["uses"],
                         ["a@1.0.3"])
        self.assertIn("runs a@1.0.3 today", fx.read(root, "docs/x.md"))
        changelog = fx.read(root, "CHANGELOG.md")
        self.assertNotIn("<<<<<<<", changelog)
        self.assertIn("#102", changelog)
        self.assertIn("revert", changelog)
        self.assertEqual(f.parents(), [later])
        self.assertEqual(fx.git(root, "status", "--porcelain"), "")
        self.assertEqual(f.state_of("S1"), "reverted")
        self.assertEqual(f.state_of("S2"), "merged")

    def test_revert_older_merge_refuses_content_conflict_with_guidance(self):
        f = self.f
        self.later_merge({"templates/a/foo.txt": "foo changed\n"})
        later = f.head()
        rc, _, err = self.revert()
        self.assertEqual(rc, merge.REFUSE_EXIT)
        self.assertIn("revert_conflict", err)
        self.assertIn("templates/a/foo.txt", err)
        self.assertIn("newest first", err)
        self.assertEqual(f.head(), later)
        self.assertEqual(f.state_of("S1"), "merged")
        self.assertIsNone(f.lease())
        self.assertFalse(os.path.exists(merge.candidate_path("revert-S1")))


class ChangelogProseRevertTest(unittest.TestCase):
    """I1: the reverted merge's own CHANGELOG lines go, a later merge's stay."""

    def setUp(self):
        self.f = fx.Fixture()
        self.addCleanup(self.f.close)

    def with_line(self, line):
        text = fx.read(self.f.root, "CHANGELOG.md")
        head, sep, rest = text.partition("\n## [0.1.0]")
        return head.rstrip("\n") + "\n" + line + "\n" + sep + rest

    def test_revert_older_merge_drops_its_changelog_prose(self):
        f = self.f
        f.strand("S1", 101, {"templates/a/foo.txt": "foo\n",
                             "CHANGELOG.md": self.with_line("- Feature S1 (#101).")})
        rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        own = [l for l in fx.read(f.root, "CHANGELOG.md").splitlines()
               if l.startswith("- **Template versions")]
        self.assertEqual(len(own), 1)
        f.strand("S2", 102, {"templates/b/bar.txt": "bar\n",
                             "CHANGELOG.md": self.with_line("- Feature S2 (#102).")})
        rc, _, err = f.merge(["S2"])
        self.assertEqual(rc, 0, err)
        out, err = io.StringIO(), io.StringIO()
        rc = revert.revert("S1", "test", start=f.root, wave=fx.WAVE, db=f.db, out_=out,
                           err=err)
        self.assertEqual(rc, 0, err.getvalue())
        changelog = fx.read(f.root, "CHANGELOG.md")
        self.assertNotIn("<<<<<<<", changelog)
        self.assertNotIn("Feature S1", changelog)
        self.assertNotIn(own[0], changelog)
        self.assertIn("- Feature S2 (#102).", changelog)
        self.assertIn("revert", changelog)


    def test_revert_older_merge_drops_its_own_heading_once_empty(self):
        # N1: the merge's own `### ` heading goes with its entries; a heading
        # that still carries an entry of someone else stays.
        f = self.f
        f.strand("S1", 101, {"templates/a/foo.txt": "foo\n",
                             "CHANGELOG.md": self.with_line("### Fixed\n\n- Fix S1 (#101).")})
        rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        f.strand("S2", 102, {"templates/b/bar.txt": "bar\n",
                             "CHANGELOG.md": self.with_line("### Added\n\n- Feature S2 (#102).")})
        rc, _, err = f.merge(["S2"])
        self.assertEqual(rc, 0, err)
        out, err = io.StringIO(), io.StringIO()
        rc = revert.revert("S1", "test", start=f.root, wave=fx.WAVE, db=f.db, out_=out,
                           err=err)
        self.assertEqual(rc, 0, err.getvalue())
        changelog = fx.read(f.root, "CHANGELOG.md")
        self.assertNotIn("<<<<<<<", changelog)
        self.assertNotIn("Fix S1", changelog)
        self.assertNotIn("### Fixed", changelog)
        self.assertIn("### Added\n\n- Feature S2 (#102).", changelog)


    def test_revert_older_merge_keeps_its_heading_over_a_foreign_subsection(self):
        # Review M1: only `#`..`###` ends a section. A foreign `#### `
        # subsection under the merge's own `### ` heading is content, so the
        # heading stays and the subsection keeps its parent.
        f = self.f
        f.strand("S1", 101, {"templates/a/foo.txt": "foo\n",
                             "CHANGELOG.md": self.with_line("### Fixed\n\n- Fix S1 (#101).")})
        rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        f.strand("S2", 102, {"templates/b/bar.txt": "bar\n",
                             "CHANGELOG.md": self.with_line("#### Display\n\n- Fix S2 (#102).")})
        rc, _, err = f.merge(["S2"])
        self.assertEqual(rc, 0, err)
        out, err = io.StringIO(), io.StringIO()
        rc = revert.revert("S1", "test", start=f.root, wave=fx.WAVE, db=f.db, out_=out,
                           err=err)
        self.assertEqual(rc, 0, err.getvalue())
        changelog = fx.read(f.root, "CHANGELOG.md")
        self.assertNotIn("<<<<<<<", changelog)
        self.assertNotIn("Fix S1", changelog)
        self.assertIn("### Fixed", changelog)
        self.assertLess(changelog.index("### Fixed"), changelog.index("#### Display"))
        self.assertIn("#### Display\n\n- Fix S2 (#102).", changelog)

class HistoryRevertTest(unittest.TestCase):
    """F5 after F2: the `Since` sentences of a later merge sit on the same
    purpose line as the older merge's -- bookkeeping, the revert takes it."""

    def setUp(self):
        self.f = fx.Fixture()
        self.addCleanup(self.f.close)
        path = "templates/a/template.json"
        doc = json.loads(fx.read(self.f.root, path))
        doc["description"] = {"purpose": "Template a."}
        self.f.master_commit({path: json.dumps(doc, indent=2) + "\n"}, msg="purpose")
        for name, issue, files in (("S1", 101, {"templates/a/foo.txt": "foo\n"}),
                                   ("S2", 102, {"templates/a/bar.txt": "bar\n"})):
            self.f.strand(name, issue, files)
            rc, _, err = self.f.merge([name])
            self.assertEqual(rc, 0, err)

    def test_revert_older_merge_keeps_the_history_and_writes_its_own(self):
        f = self.f
        later = f.head()
        rc = revert.revert("S1", "test", start=f.root, wave=fx.WAVE, db=f.db,
                           out_=io.StringIO(), err=io.StringIO())
        self.assertEqual(rc, 0)
        self.assertEqual(f.parents(), [later])
        p = json.loads(fx.read(f.root, "templates/a/template.json"))["description"]["purpose"]
        self.assertIn("Since 1.0.1 (#101)", p)
        self.assertIn("Since 1.0.2 (#102)", p)
        self.assertTrue(p.endswith("Since 1.0.3 (revert of #101) the template takes this "
                                   "change back; it is the third digit."), p)
        self.assertFalse(os.path.exists(os.path.join(f.root, "templates/a/foo.txt")))
        self.assertEqual(fx.read(f.root, "templates/a/bar.txt"), "bar\n")

    def test_bookkeeping_only_takes_appended_since_sentences(self):
        f = self.f
        block = ('<<<<<<< HEAD\n  "purpose": "T. Since 1.0.1 (x) a. Since 1.0.2 (y) b."\n'
                 '=======\n  "purpose": "T."\n>>>>>>> parent\n')
        fx.write(f.tmp, "c1.json", block)
        self.assertTrue(revert.bookkeeping_only(f.tmp, ["c1.json"]))
        # Content before the sentences differs: no bookkeeping.
        fx.write(f.tmp, "c2.json", block.replace('"T."', '"U."'))
        self.assertFalse(revert.bookkeeping_only(f.tmp, ["c2.json"]))


class CorpusRevertTest(unittest.TestCase):
    """ORCH.18: the generated corpus is regenerated by the version step, never
    reverted -- a later merge that moved the same chunks (blocks of unequal
    length) is cascade, not content."""

    CORPUS = merge.CORPUS[0]

    def setUp(self):
        self.f = fx.Fixture()
        self.addCleanup(self.f.close)
        self.f.master_commit({self.CORPUS: "head\nv1\ntail\n"}, msg="corpus")
        for name, issue, files in (
                ("S1", 101, {"templates/a/foo.txt": "foo\n", self.CORPUS: "head\nS1-a\nS1-b\ntail\n"}),
                ("S2", 102, {"templates/a/bar.txt": "bar\n", self.CORPUS: "head\nS2\ntail\n"})):
            self.f.strand(name, issue, files)
            rc, _, err = self.f.merge([name])
            self.assertEqual(rc, 0, err)

    def test_bookkeeping_only_takes_the_corpus(self):
        f = self.f
        fx.write(f.tmp, self.CORPUS, "<<<<<<< HEAD\nS2\n=======\nv1\nmore\n>>>>>>> parent\n")
        self.assertTrue(revert.bookkeeping_only(f.tmp, [self.CORPUS]))

    def test_revert_older_merge_keeps_master_corpus_for_the_regeneration(self):
        f = self.f
        later = f.head()
        rc = revert.revert("S1", "test", start=f.root, wave=fx.WAVE, db=f.db,
                           out_=io.StringIO(), err=io.StringIO())
        self.assertEqual(rc, 0)
        self.assertEqual(f.parents(), [later])
        self.assertEqual(fx.read(f.root, self.CORPUS), "head\nS2\ntail\n")
        self.assertFalse(os.path.exists(os.path.join(f.root, "templates/a/foo.txt")))
        self.assertEqual(fx.read(f.root, "templates/a/bar.txt"), "bar\n")


class RerereRevertTest(unittest.TestCase):
    """OR-TM-11: `take_back` decides from its own conflict -- a recorded rerere
    resolution of the same conflict never hides the corpus conflict of the
    revert (the kit's git calls run with `rerere.enabled=false`)."""

    CORPUS = merge.CORPUS[0]

    def test_a_recorded_resolution_never_hides_the_corpus_conflict_of_a_revert(self):
        root = fx.rerere_init(self)
        fx.write(root, self.CORPUS, "head\nv1\ntail\n")
        fx.git(root, "add", "-A")
        fx.git(root, "commit", "-qm", "corpus")
        fx.git(root, "checkout", "-qb", "s1")
        fx.write(root, self.CORPUS, "head\nS1\ntail\n")
        fx.git(root, "commit", "-qam", "s1")
        fx.git(root, "checkout", "-q", "master")
        fx.git(root, "merge", "-q", "--no-ff", "-m", "merge s1", "s1")
        merged = fx.git(root, "rev-parse", "HEAD")
        fx.write(root, self.CORPUS, "head\nS2\ntail\n")
        fx.git(root, "commit", "-qam", "a later merge moves the corpus")
        fx.rerere_record(self, root, ["revert", "-m", "1", "--no-commit", merged], self.CORPUS,
                         "OLD-RESOLUTION\n")
        revert.take_back(root, merged)
        self.assertEqual(fx.read(root, self.CORPUS), "head\nS2\ntail\n")
        self.assertEqual(fx.git(root, "diff", "--name-only", "--diff-filter=U"), "")
        self.assertTrue(os.path.exists(os.path.join(root, ".git", "REVERT_HEAD")))


if __name__ == "__main__":
    unittest.main()
