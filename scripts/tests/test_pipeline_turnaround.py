"""Tests of `gate a strand once` (`scripts/pipeline/turnaround.py`, GH #1167).

(a)/(h) code tree, not commit: commits after the receipt and after the pre
stamp that touch only gate-neutral paths keep both valid; any other file --
the corridor fixtures and the ADRs under `plans/` included -- does not.
(b) the merge gate plans from the strand tip when the strand receipt covers it.
(c) the queue names every missing precondition in one pass.
(d) a plan-only branch merges without gate and without report.
(e) pre reports main-tree files that would stop the fast-forward.
(f) pre reports commits in `master..HEAD` not by the owner.

The merge cases reuse the fixture of `test_pipeline_merge` (a throw-away main
tree, strand worktrees, the fake gate)."""

import datetime
import io
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS))

from pipeline import merge, pre, turnaround  # noqa: E402
from scripts.tests import store_isolation  # noqa: E402
from scripts.tests import test_pipeline_merge as tm  # noqa: E402

REPO_ROOT = SCRIPTS.parent

WAVE = tm.WAVE
WAVE_PREFIX = WAVE.rsplit("-", 3)[0]   # the branch prefix of the wave
LEDGER = "plans/%s/ledger.md" % WAVE
REPORT = "plans/%s/berichte/S1.md" % WAVE
NOTES = "plans/%s/berichte/zz-notes.md" % WAVE
STRAENGE = "plans/%s/straenge/S1-notes.md" % WAVE
CORRIDOR = "plans/phase-13.5-hive-transit-fixtures/expected_route_body.txt"
ADR = "plans/adr/0001-something.md"
OWNER = "test <test@example.invalid>"


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()
    tm.setUpModule()


def tearDownModule():  # noqa: N802 -- unittest's name
    tm.tearDownModule()
    store_isolation.restore()


def commit(wt, files, msg="later"):
    for rel, text in files.items():
        tm.write(wt, rel, text)
    tm.git(wt, "add", "-A")
    tm.git(wt, "commit", "-q", "-m", msg)
    return tm.git(wt, "rev-parse", "HEAD")


class Case(tm.MergeCase):
    plans_tracked = True

    def setUp(self):
        super().setUp()
        env = mock.patch.dict(os.environ, {turnaround.OWNER_ENV: OWNER})
        env.start()
        self.addCleanup(env.stop)

    def strand(self, name="S1", issue=101, **kw):
        kw.setdefault("in_branch", True)
        return self.f.strand(name, issue, {"templates/a/foo.txt": "foo\n"}, **kw)

    def stamp(self, wt):
        """A stamp as `run_pre` writes it now: with HEAD and the overlay."""
        paths = pre.overlay_paths(wt)
        pre.write_stamp(wt, {"tree_hash": pre.tree_hash(wt), "verdict": "GREEN",
                             "at": datetime.datetime.now(datetime.timezone.utc)
                             .strftime("%Y-%m-%dT%H:%M:%SZ"),
                             "lane_half": False, "reasons": [],
                             "head": tm.git(wt, "rev-parse", "HEAD"),
                             "overlay": turnaround.overlay_digests(wt, paths)})

    def refused(self, strand="S1"):
        rc, _, err = self.f.merge([strand])
        self.assertEqual(rc, merge.REFUSE_EXIT, err)
        return err


# --- (a) / (h) --------------------------------------------------------------------

class CodeTreeTest(Case):

    def bump(self, wt, rel=REPORT, msg="later"):
        """A commit appending a line to `rel` (the strand report by default)."""
        old = tm.read(wt, rel) if os.path.exists(os.path.join(wt, rel)) else ""
        return commit(wt, {rel: old + "note\n"}, msg)

    def test_a_review_commit_after_the_receipt_keeps_it(self):
        s = self.strand()
        self.stamp(s["wt"])
        commit(s["wt"], {REPORT: tm.read(s["wt"], REPORT) + "\nVerdikt: ja (recheck)\n",
                         STRAENGE: "strand file\n"})
        self.stamp(s["wt"])
        rc, out, err = self.f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        self.assertIn("MERGED S1", out)

    def test_r7_ruling_a_file_new_since_the_gate_keeps_the_receipt(self):
        # Koordinator-3, ruling on "100644 on both sides": absent on the gated
        # side and a regular 100644 file in the tip is a new report -- reuse.
        s = self.strand()
        self.stamp(s["wt"])
        for rel in (NOTES, STRAENGE):
            self.assertEqual(tm.git(s["wt"], "ls-tree", "HEAD", "--", rel), "", rel)
        commit(s["wt"], {NOTES: "new\n", STRAENGE: "new\n"}, "new files")
        self.stamp(s["wt"])
        rc, out, err = self.f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        self.assertIn("MERGED S1", out)

    def test_r7_ruling_a_file_deleted_since_the_gate_regates(self):
        # Same ruling, narrower half: a file the gated rev holds and the tip
        # lacks is a deletion -- the receipt does not cover it.
        for rel in (NOTES, STRAENGE):
            with self.subTest(rel=rel):
                self.f = tm.Fixture(self.gate, self.plans_tracked)  # a fresh repo per case
                self.addCleanup(self.f.close)
                s = self.f.strand("S1", 101, {"templates/a/foo.txt": "foo\n"},
                                  in_branch=True, receipt=False)
                gated = commit(s["wt"], {rel: "old\n"}, "a file")
                self.f.receipt("S1", tm.RUN_DIR % gated[:8], "strand", gated, "GREEN")
                self.stamp(s["wt"])
                os.remove(os.path.join(s["wt"], rel))
                commit(s["wt"], {}, "drop it")
                self.stamp(s["wt"])
                err = self.refused()
                self.assertIn("receipt_not_branch_head", err)
                self.assertIn("%s (absent in the tip" % rel, err)

    def test_r7_bookkeeping_outside_berichte_and_straenge_regates(self):
        # Koordinator-3, fix round 7: the receipt survives berichte/ and
        # straenge/ of the strand's own wave only -- ledger, receipts and
        # README re-gate.
        for rel in (LEDGER, "plans/%s/receipts/S1/x/summary.txt" % WAVE,
                    "plans/%s/README.md" % WAVE, "plans/%s/messvertrag.md" % WAVE,
                    "plans/welle-other-2026-01-02/berichte/S1.md"):
            with self.subTest(rel=rel):
                self.f = tm.Fixture(self.gate, self.plans_tracked)  # a fresh repo per case
                self.addCleanup(self.f.close)
                s = self.strand()
                self.stamp(s["wt"])
                self.bump(s["wt"], rel)
                self.stamp(s["wt"])
                err = self.refused()
                self.assertIn("receipt_not_branch_head", err)
                self.assertIn("%s (outside plans/%s/{%s})" % (
                    rel, WAVE, ",".join(turnaround.POST_GATE)), err)

    def test_post_gate_is_one_list_for_receipt_and_stamp(self):
        # GH #1199 (e): `reuse_path` (merge receipt) and `allowlisted` (pre
        # stamp, plan-only) read the ONE list `POST_GATE` -- RESTLISTE.md and
        # recheck/ of Konvergenz A on both, nothing else of plans/.
        for rel in (REPORT, "plans/%s/straenge/S1.json" % WAVE,
                    "plans/%s/RESTLISTE.md" % WAVE,
                    "plans/%s/recheck/skripte/S1/run_all.sh" % WAVE):
            with self.subTest(rel=rel):
                self.assertIsNone(turnaround.reuse_path(rel, WAVE))
                self.assertTrue(turnaround.allowlisted(rel, WAVE))
        # Every wave carries RESTLISTE.md and run_all.sh: a script naming the
        # bare name (this module's `POST_GATE`) reads no wave's file; the path does.
        for rel in ("plans/%s/RESTLISTE.md" % WAVE,
                    "plans/%s/recheck/skripte/S1/run_all.sh" % WAVE):
            with self.subTest(needle=rel):
                needles = turnaround._needles(rel)
                self.assertNotIn((rel.rsplit("/", 1)[-1], False), needles)
                self.assertIn((rel, False), needles)
        for rel in ("plans/PREAMBLE.md", LEDGER, "plans/%s/README.md" % WAVE,
                    "plans/%s/RESTLISTE.md/x" % WAVE, "plans/%s/recheck" % WAVE,
                    "plans/%s/sub/RESTLISTE.md" % WAVE, "scripts/pipeline/merge.py",
                    "plans/welle-other-2026-01-02/RESTLISTE.md"):
            with self.subTest(rel=rel):
                self.assertIsNotNone(turnaround.reuse_path(rel, WAVE))
                self.assertFalse(turnaround.allowlisted(rel, WAVE))

    def test_r7_a_dot_dot_component_is_never_reused(self):
        for rel in ("plans/%s/berichte/../ledger.md" % WAVE,
                    "plans/%s/berichte/./S1.md" % WAVE,
                    "plans/%s/straenge/../../../crates/x/src/lib.rs" % WAVE):
            self.assertIn("'..'", turnaround.reuse_path(rel, WAVE) or "", rel)
        self.assertIsNone(turnaround.reuse_path(REPORT, WAVE))
        self.assertIsNotNone(turnaround.reuse_path(REPORT, None))

    def test_r7_a_dot_dot_entry_in_the_diff_regates(self):
        # A tree entry named `..` (git mktree takes it) under berichte/: the
        # allowlist prefix alone would pass it.
        s = self.strand()
        gated = tm.git(s["wt"], "rev-parse", "HEAD")

        def mktree(text):
            return subprocess.run(["git", "-C", self.f.root, "mktree"], input=text,
                                  capture_output=True, text=True, check=True).stdout.strip()
        blob = subprocess.run(["git", "-C", self.f.root, "hash-object", "-w", "--stdin"],
                              input="x\n", capture_output=True, text=True,
                              check=True).stdout.strip()
        dotdot = mktree("100644 blob %s\tx.md\n" % blob)
        tree = tm.git(self.f.root, "rev-parse", gated + "^{tree}")
        # Rebuild the tree bottom-up with plans/<wave>/berichte/.. added.
        ber = tm.git(self.f.root, "rev-parse", "%s:plans/%s/berichte" % (gated, WAVE))
        listing = tm.git(self.f.root, "ls-tree", ber)
        ber2 = mktree(listing + "\n040000 tree %s\t..\n" % dotdot)
        w = tm.git(self.f.root, "rev-parse", "%s:plans/%s" % (gated, WAVE))
        lines = [l for l in tm.git(self.f.root, "ls-tree", w).split("\n")
                 if not l.endswith("\tberichte")]
        w2 = mktree("\n".join(lines + ["040000 tree %s\tberichte" % ber2]) + "\n")
        pl = tm.git(self.f.root, "rev-parse", "%s:plans" % gated)
        lines = [l for l in tm.git(self.f.root, "ls-tree", pl).split("\n")
                 if not l.endswith("\t" + WAVE)]
        pl2 = mktree("\n".join(lines + ["040000 tree %s\t%s" % (w2, WAVE)]) + "\n")
        lines = [l for l in tm.git(self.f.root, "ls-tree", tree).split("\n")
                 if not l.endswith("\tplans")]
        top = mktree("\n".join(lines + ["040000 tree %s\tplans" % pl2]) + "\n")
        tip = subprocess.run(["git", "-C", self.f.root, "commit-tree", top, "-p", gated,
                              "-m", "dotdot"], capture_output=True, text=True,
                             check=True).stdout.strip()
        ok, bad = turnaround.code_equal_revs(self.f.root, gated, tip, WAVE)
        self.assertFalse(ok)
        self.assertEqual(["plans/%s/berichte/../x.md" % WAVE], sorted(bad))
        self.assertIn("'..'", bad["plans/%s/berichte/../x.md" % WAVE])

    def test_a_report_head_rewrite_after_pre_keeps_the_stamp(self):
        s = self.strand()
        self.stamp(s["wt"])
        # `strand.sh report` rewrites the head -- dirty first, then committed.
        tm.write(s["wt"], REPORT, tm.read(s["wt"], REPORT).replace("gate: GREEN", "gate: x"))
        ok, line = pre.stamp_status(s["wt"])
        self.assertTrue(ok, line)
        commit(s["wt"], {}, "report")
        ok, line = pre.stamp_status(s["wt"])
        self.assertTrue(ok, line)
        self.assertIn("code tree", line)

    def test_a_code_change_after_pre_breaks_the_stamp(self):
        s = self.strand()
        self.stamp(s["wt"])
        tm.write(s["wt"], "templates/a/foo.txt", "other\n")
        ok, line = pre.stamp_status(s["wt"])
        self.assertFalse(ok)
        self.assertIn("templates/a/foo.txt", line)

    def fresh_strand(self):
        self.f = tm.Fixture(self.gate, self.plans_tracked)  # a fresh repo per case
        self.addCleanup(self.f.close)
        return self.strand()

    def test_s10_w7_a_symlink_alias_onto_a_report_breaks_the_stamp(self):
        # tb-s7 open point: the stamp saw a symlink only as its `link:` digest.
        # A link outside the allowlist onto a report (or onto its directory)
        # keeps that digest while the report behind it changes; a link in the
        # allowlist may change its target; a link may leave the tree. None of
        # them is code-equal (fail closed).
        outside = tempfile.TemporaryDirectory()
        self.addCleanup(outside.cleanup)
        reports = "../../plans/%s/berichte" % WAVE
        cases = (
            # (name, link, target, committed, the change after pre)
            ("file link", "templates/a/alias.md", reports + "/S1.md", True, REPORT),
            ("directory link", "templates/a/reports", reports, True, REPORT),
            ("untracked link", "templates/a/alias.md", reports + "/S1.md", False, REPORT),
            ("link in the allowlist retargeted", NOTES, "S1.md", True, "retarget"),
            ("link leaving the tree", "templates/a/out", outside.name, True, REPORT),
        )
        for name, link, target, committed, change in cases:
            with self.subTest(case=name):
                s = self.fresh_strand()
                wt = s["wt"]
                os.symlink(target, os.path.join(wt, link))
                if committed:
                    commit(wt, {}, "link")
                self.stamp(wt)
                ok, line = pre.stamp_status(wt)
                self.assertTrue(ok, line)
                if change == "retarget":
                    os.remove(os.path.join(wt, link))
                    os.symlink("../../../templates/a/foo.txt", os.path.join(wt, link))
                else:
                    tm.write(wt, change, tm.read(wt, change) + "\nchanged behind the link\n")
                ok, line = pre.stamp_status(wt)
                self.assertFalse(ok, line)
                self.assertIn("symlink", line)

    def test_s10_w7_an_unchanged_tree_with_links_stays_code_equal(self):
        # No false alarm: a link onto a report alone, or a link elsewhere in
        # the wave (a `receipts/<strand>/latest` as on master) or of another
        # wave out of the tree while a report changes, keeps the stamp.
        s = self.strand()
        wt = s["wt"]
        os.symlink("../../plans/%s/berichte/S1.md" % WAVE, os.path.join(wt, "templates/a/alias.md"))
        os.makedirs(os.path.join(wt, "plans", WAVE, "receipts", "S1"))
        os.symlink("20261010T000000Z-00000000",
                   os.path.join(wt, "plans", WAVE, "receipts", "S1", "latest"))
        # and a link of another wave out of the tree, as master's
        # `plans/welle-loop-2026-10-03/berichte` into the coding repo
        outside = tempfile.TemporaryDirectory()
        self.addCleanup(outside.cleanup)
        os.makedirs(os.path.join(wt, "plans", "welle-other"))
        os.symlink(outside.name, os.path.join(wt, "plans", "welle-other", "berichte"))
        commit(wt, {}, "links")
        self.stamp(wt)
        head = tm.git(wt, "rev-parse", "HEAD")
        digests = turnaround.overlay_digests(wt, pre.overlay_paths(wt))
        ok, bad = turnaround.code_equal_stamp(wt, {"head": head, "overlay": digests},
                                              head, digests, WAVE)
        self.assertTrue(ok, bad)
        ok, line = pre.stamp_status(wt)
        self.assertTrue(ok, line)
        os.remove(os.path.join(wt, "templates/a/alias.md"))
        commit(wt, {}, "unlink")
        self.stamp(wt)
        tm.write(wt, REPORT, tm.read(wt, REPORT) + "\nreport only\n")
        ok, line = pre.stamp_status(wt)
        self.assertTrue(ok, line)
        self.assertIn("code tree", line)

    def test_h_a_corridor_fixture_after_the_receipt_breaks_it(self):
        s = self.strand()
        self.stamp(s["wt"])
        commit(s["wt"], {CORRIDOR: "async fn route() {}\n"})
        self.stamp(s["wt"])
        err = self.refused()
        self.assertIn("receipt_not_branch_head", err)
        self.assertIn(CORRIDOR, err)

    def test_h_an_adr_after_the_receipt_breaks_it(self):
        s = self.strand()
        self.stamp(s["wt"])
        commit(s["wt"], {ADR: "# ADR\n"})
        self.stamp(s["wt"])
        err = self.refused()
        self.assertIn("receipt_not_branch_head", err)
        self.assertIn(ADR, err)

    # Auflage B1 (Koordinator-3): a report a committed script or test names
    # re-gates -- `reuse_readers`, `git grep` over the tip and master.

    def test_h_a_neutral_path_a_test_reads_is_code(self):
        tm.write(self.f.root, "crates/x/tests/reads_report.rs",
                 'const P: &str = "%s";\n#[test]\nfn t() {}\n' % REPORT)
        tm.git(self.f.root, "add", "-A")
        tm.git(self.f.root, "commit", "-q", "-m", "a reader")
        s = self.strand()
        self.stamp(s["wt"])
        self.bump(s["wt"])
        self.stamp(s["wt"])
        err = self.refused()
        self.assertIn("receipt_not_branch_head", err)
        self.assertIn("read by crates/x/tests/reads_report.rs", err)

    def test_h_a_python_test_naming_the_report_is_code(self):
        tm.write(self.f.root, "scripts/tests/test_x.py", 'REPORT = "%s"\n' % REPORT)
        tm.git(self.f.root, "add", "-A")
        tm.git(self.f.root, "commit", "-q", "-m", "a python reader")
        s = self.strand()
        self.stamp(s["wt"])
        self.bump(s["wt"])
        self.stamp(s["wt"])
        err = self.refused()
        self.assertIn("receipt_not_branch_head", err)
        self.assertIn("read by scripts/tests/test_x.py", err)

    def test_r7_b1_a_report_test_wave_retro_reads_regates(self):
        # The reader shape of test_wave_retro: `REPO / "plans" / "<wave>"`,
        # no path literal -- the wave directory is the needle.
        tm.write(self.f.root, "scripts/tests/test_wave_retro.py",
                 'import pathlib\nW = pathlib.Path("plans") / "%s"\n' % WAVE)
        tm.git(self.f.root, "add", "-A")
        tm.git(self.f.root, "commit", "-q", "-m", "test_wave_retro")
        s = self.strand()
        self.stamp(s["wt"])
        self.bump(s["wt"])
        self.stamp(s["wt"])
        err = self.refused()
        self.assertIn("receipt_not_branch_head", err)
        self.assertIn("read by scripts/tests/test_wave_retro.py", err)

    def test_r7_b1_the_real_test_wave_retro_is_a_reader_of_its_reports(self):
        # In this repository: test_wave_retro reads the h2 reports (fixed old
        # waves); `reuse_readers` names it for a report of that wave.
        p = "plans/welle-h2-2026-09-18/berichte/x.md"
        self.assertIn(p, turnaround.reuse_readers(str(REPO_ROOT), [p], (("HEAD", "here"),)))
        self.assertIn("scripts/tests/test_wave_retro.py", turnaround.python_readers(
            str(REPO_ROOT), [p], "HEAD", turnaround.PY_READERS + turnaround.REUSE_TESTS))

    def test_r7_a_reader_master_gains_after_the_strand_base_regates(self):
        # Reader half `master` of `reuse_readers` (the tip half: h_n1 below).
        s = self.strand()
        self.stamp(s["wt"])
        self.bump(s["wt"])
        self.stamp(s["wt"])
        self.f.master_commit({"scripts/tests/test_late.py": 'R = "%s"\n' % REPORT})
        err = self.refused()
        self.assertIn("receipt_not_branch_head", err)
        self.assertIn("read by scripts/tests/test_late.py in master", err)

    def test_v2a10_the_per_path_reader_grep_reads_only_the_hit_files(self):
        # V2-A10 (h): after a hit of the one grep over every path, each path
        # was grepped again over every script and test of the rev -- 614
        # greps, 984 s for the 310 re-check files of v2-fix-b (probe
        # 15.10., /tmp/claude-v2fixc-h/probe2.log). A file naming one path
        # is a hit of the grep over all of them: the per-path grep reads the
        # hit files alone (`offending` does it so since K-R1).
        self.f.master_commit({"scripts/tests/test_x.py": 'REPORT = "%s"\n' % REPORT})
        with mock.patch.object(turnaround, "python_readers",
                               wraps=turnaround.python_readers) as pr:
            got = turnaround.reuse_readers(self.f.root, [REPORT, NOTES], (("HEAD", "master"),))
        self.assertEqual(got, {REPORT: "read by scripts/tests/test_x.py in master"})
        per_path = [c for c in pr.call_args_list if len(c.args[1]) == 1]
        self.assertEqual(len(per_path), 2, pr.call_args_list)
        for c in per_path:
            self.assertEqual(tuple(c.args[3]), (":(literal)scripts/tests/test_x.py",), c)

    def test_i_h1_a_reader_name_git_quotes_is_found_by_both_reader_rules(self):
        # Re-check I-h1 (h): `git grep -l` without `-z` quotes a name with
        # non-ASCII, `"` or `\` (`"HEAD:scripts/\303\244.py"`); the per-path
        # grep over `:(literal)<hit>` then matched nothing and the path counted
        # as unread -- fail-open in `reuse_readers` and in `offending` (K-R1).
        readers = {"scripts/ä.py": REPORT, 'scripts/a"b.py': NOTES}
        self.f.master_commit({r: "open(%r)\n" % p for r, p in readers.items()})
        got = turnaround.reuse_readers(self.f.root, [REPORT, NOTES], (("HEAD", "master"),))
        bad = turnaround.offending([REPORT, NOTES], self.f.root, WAVE)
        for reader, path in readers.items():
            self.assertEqual(got.get(path), "read by %s in master" % reader, got)
            self.assertEqual(bad.get(path), "rule 8 python: %s" % reader, bad)
        self.assertEqual(sorted(readers), turnaround.python_readers(
            self.f.root, [REPORT, NOTES], "HEAD"))

    def own_reader_then_report(self, reader, text, extra=None):
        """Re-check B-N1: the strand brings its OWN reader of its report (not
        on master); the receipt covers that rev, a report commit and a fresh
        stamp follow. `extra`: more files of the strand commit."""
        files = {"templates/a/foo.txt": "foo\n", reader: text}
        files.update(extra or {})
        s = self.f.strand("S1", 101, files, in_branch=True)
        self.stamp(s["wt"])
        self.bump(s["wt"])
        self.stamp(s["wt"])
        self.assertFalse(os.path.exists(os.path.join(self.f.root, reader)))
        return s

    def test_h_n1_a_python_reader_the_strand_brings_is_code(self):
        self.own_reader_then_report("scripts/tests/test_x.py", 'REPORT = "%s"\n' % REPORT)
        err = self.refused()
        self.assertIn("receipt_not_branch_head", err)
        self.assertIn("read by scripts/tests/test_x.py in the tip", err)

    def test_h_n1_a_rust_reader_the_strand_brings_is_code(self):
        self.own_reader_then_report("crates/x/tests/reads_report.rs",
                                    'const P: &str = "%s";\n#[test]\nfn t() {}\n' % REPORT)
        err = self.refused()
        self.assertIn("receipt_not_branch_head", err)
        self.assertIn("read by crates/x/tests/reads_report.rs in the tip", err)

    # Re-check tb-s7 B-N2: `git archive` dropped `export-ignore` files. The
    # reuse readers come from `git grep <commit>`, which knows no attributes.

    def test_b_n2_a_reader_export_ignored_by_the_strands_attributes_is_code(self):
        reader = "crates/x/tests/reads_report.rs"
        text = 'const P: &str = "%s";\n#[test]\nfn t() {}\n' % REPORT
        for attrs in ({".gitattributes": "%s export-ignore\n" % reader},
                      {"crates/x/tests/.gitattributes": "*.rs export-ignore\n"}):
            with self.subTest(attrs=sorted(attrs)):
                self.f = tm.Fixture(self.gate, self.plans_tracked)  # a fresh repo per case
                self.addCleanup(self.f.close)
                self.own_reader_then_report(reader, text, extra=attrs)
                err = self.refused()
                self.assertIn("receipt_not_branch_head", err)
                self.assertIn("read by %s" % reader, err)

    def test_b_n2_a_reader_export_ignored_by_info_attributes_is_code(self):
        reader = "crates/x/tests/reads_report.rs"
        common = tm.git(self.f.root, "rev-parse", "--git-common-dir")
        info = os.path.join(self.f.root, common, "info")
        os.makedirs(info, exist_ok=True)
        with open(os.path.join(info, "attributes"), "w", encoding="utf-8") as fh:
            fh.write("crates/*/tests/** export-ignore\n")
        self.own_reader_then_report(reader,
                                    'const P: &str = "%s";\n#[test]\nfn t() {}\n' % REPORT)
        err = self.refused()
        self.assertIn("receipt_not_branch_head", err)
        self.assertIn("read by %s" % reader, err)

    def test_b_n3_a_symlink_reader_whose_target_is_a_crate_source_refuses(self):
        # B-N3: a test linked to `../src/impl_reads.rs`; `git grep` follows no
        # link, it reads every crate file -- the source names the report.
        s = self.f.strand("S1", 101, {
            "templates/a/foo.txt": "foo\n",
            "crates/x/src/impl_reads.rs": 'const P: &str = "%s";\n' % REPORT}, in_branch=True,
            receipt=False)
        os.makedirs(os.path.join(s["wt"], "crates/x/tests"), exist_ok=True)
        os.symlink("../src/impl_reads.rs", os.path.join(s["wt"], "crates/x/tests/reads.rs"))
        tip = commit(s["wt"], {}, "a link")
        self.f.receipt("S1", tm.RUN_DIR % tip[:8], "strand", tip, "GREEN")
        self.stamp(s["wt"])
        self.bump(s["wt"])
        self.stamp(s["wt"])
        err = self.refused()
        self.assertIn("receipt_not_branch_head", err)
        self.assertIn("read by crates/x/src/impl_reads.rs", err)

    def test_m_n6_a_failing_reuse_check_refuses(self):
        # The fail-closed arm of `code_equal_revs`: a git error is no cover.
        for name in ("changed", "reuse_tree", "reuse_readers"):
            with self.subTest(name=name):
                self.f = tm.Fixture(self.gate, self.plans_tracked)  # a fresh repo per case
                self.addCleanup(self.f.close)
                s = self.strand()
                self.stamp(s["wt"])
                self.bump(s["wt"])
                self.stamp(s["wt"])
                with mock.patch.object(turnaround, name, side_effect=ValueError("boom")):
                    err = self.refused()
                self.assertIn("receipt_not_branch_head", err)
                self.assertIn("(boom)", err)

    # Receipt rule by construction (Koordinator-3): every changed path is a
    # regular 100644 file in the tip and a regular 100644 file or absent in
    # the gated rev and master, with directories only above it. Each half is pinned on its own (M-N7).

    def gated_link(self, link, target):
        """A strand whose gated commit carries the symlink `link -> target`."""
        s = self.f.strand("S1", 101, {"templates/a/foo.txt": "foo\n"}, in_branch=True,
                          receipt=False)
        dest = os.path.join(s["wt"], link)
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        os.symlink(target, dest)
        tip = commit(s["wt"], {}, "a link")
        self.f.receipt("S1", tm.RUN_DIR % tip[:8], "strand", tip, "GREEN")
        self.stamp(s["wt"])
        return s

    def test_construction_a_symlink_in_the_diff_since_the_gate_refuses(self):
        # Half `the tip`: the link exists in the tip only.
        s = self.strand()
        self.stamp(s["wt"])
        link = "plans/%s/berichte/alias.md" % WAVE
        os.symlink("S1.md", os.path.join(s["wt"], link))
        commit(s["wt"], {}, "a link")
        self.stamp(s["wt"])
        err = self.refused()
        self.assertIn("receipt_not_branch_head", err)
        self.assertIn("%s (mode 120000 in the tip" % link, err)

    def test_r7_a_symlink_the_strand_drops_since_the_gate_refuses(self):
        # Half `the gated rev`: the link exists in the gated rev only.
        link = "plans/%s/berichte/alias.md" % WAVE
        s = self.gated_link(link, "S1.md")
        os.remove(os.path.join(s["wt"], link))
        commit(s["wt"], {link: "now a file\n"}, "link -> file")
        self.stamp(s["wt"])
        err = self.refused()
        self.assertIn("receipt_not_branch_head", err)
        self.assertIn("%s (mode 120000 in the gated rev" % link, err)

    def test_r7_m_n7_a_symlink_master_gains_after_the_strand_base_refuses(self):
        # Half `master`: the link lands on master after the strand base, on
        # the changed path itself or on a directory above it.
        for link, changed_path in (
                ("plans/%s/berichte/n.md" % WAVE, "plans/%s/berichte/n.md" % WAVE),
                ("plans/%s/berichte/sub" % WAVE, "plans/%s/berichte/sub/n.md" % WAVE)):
            with self.subTest(link=link):
                self.f = tm.Fixture(self.gate, self.plans_tracked)  # a fresh repo per case
                self.addCleanup(self.f.close)
                s = self.strand()
                self.stamp(s["wt"])
                commit(s["wt"], {changed_path: "n\n"})
                self.stamp(s["wt"])
                dest = os.path.join(self.f.root, link)
                os.makedirs(os.path.dirname(dest), exist_ok=True)
                os.symlink(".", dest)
                tm.git(self.f.root, "add", "-A")
                tm.git(self.f.root, "commit", "-q", "-m", "a link on master")
                err = self.refused()
                self.assertIn("receipt_not_branch_head", err)
                self.assertIn("in master", err)
                self.assertIn(changed_path, err)

    def test_construction_a_mode_change_since_the_gate_refuses(self):
        s = self.strand()
        self.stamp(s["wt"])
        tm.git(s["wt"], "update-index", "--chmod=+x", REPORT)
        tm.git(s["wt"], "commit", "-q", "-m", "chmod")
        self.stamp(s["wt"])
        err = self.refused()
        self.assertIn("receipt_not_branch_head", err)
        self.assertIn("%s (mode 100755 in the tip" % REPORT, err)

    def test_construction_an_unrelated_symlink_keeps_the_receipt(self):
        # Only the paths of the diff count: the receipt `latest` links of
        # other waves change nothing.
        dest = os.path.join(self.f.root, "plans/welle-old/receipts/S/latest")
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        os.symlink("20260101T000000Z-abcdef12", dest)
        tm.git(self.f.root, "add", "-A")
        tm.git(self.f.root, "commit", "-q", "-m", "an old receipt link")
        s = self.strand()
        self.stamp(s["wt"])
        self.bump(s["wt"])
        self.stamp(s["wt"])
        rc, out, err = self.f.merge(["S1"])
        self.assertEqual(rc, 0, err)

    def test_m_n7_rule_8_python_matches_a_name_only_at_a_path_boundary(self):
        # The file name counts beside its wave only (GH #1343): each file
        # names the wave by its branch prefix `<wave>/` (`_wave_token`).
        target = "plans/%s/berichte/x.md" % WAVE
        b = 'B = "%s/x"\n' % WAVE_PREFIX
        tm.write(self.f.root, "scripts/tests/test_c.py", b + "# see recheck-x.md and sx.md\n")
        tm.write(self.f.root, "scripts/tests/test_p.py", 'P = "%s"\n' % target)
        tm.write(self.f.root, "scripts/tests/test_n.py", b + 'P = BERICHTE / "x.md"\n')
        tm.write(self.f.root, "scripts/tests/test_s.py", b + "P = 'berichte/x.md'\n")
        got = turnaround.python_readers(self.f.root, target)
        self.assertNotIn("scripts/tests/test_c.py", got)
        for reader in ("scripts/tests/test_p.py", "scripts/tests/test_n.py",
                       "scripts/tests/test_s.py"):
            self.assertIn(reader, got)

    def test_m_n9_a_name_with_regex_characters_is_matched_literally(self):
        # `_bounded` escapes the needle: unescaped, `a+b.md` misses its own
        # reader and `a+b.md` as an ERE takes `aab.md` for it.
        target = "plans/%s/berichte/a+b.md" % WAVE
        b = 'B = "%s/x"\n' % WAVE_PREFIX   # beside its wave (GH #1343)
        tm.write(self.f.root, "scripts/tests/test_r.py", b + 'P = "berichte/a+b.md"\n')
        tm.write(self.f.root, "scripts/tests/test_q.py", b + 'P = "berichte/aab.md"\n')
        got = turnaround.python_readers(self.f.root, target)
        self.assertIn("scripts/tests/test_r.py", got)
        self.assertNotIn("scripts/tests/test_q.py", got)

    def test_k_r2_a_text_naming_another_file_of_the_wave_reads_no_report(self):
        # K-R2: a doc comment naming `plans/<wave>/berichte/<other>.md`
        # (writer.rs :557/:1416) made every report commit of the wave
        # non-neutral -- the wave directory was a needle at any depth.
        specs = turnaround.PY_READERS + turnaround.REUSE_TESTS
        tm.write(self.f.root, "crates/x/src/w.rs",
                 "/// (`plans/%s/berichte/other.md`). Inside\n" % WAVE)
        tm.write(self.f.root, "scripts/tests/test_g.py",
                 'G = "plans/%s/berichte/*.md"\n' % WAVE)
        tm.write(self.f.root, "scripts/tests/test_d.py",
                 'D = "plans/%s/berichte/"\n' % WAVE)
        tm.write(self.f.root, "scripts/tests/test_e.py",
                 'E = "plans/%s/berichte"\n' % WAVE)
        tm.write(self.f.root, "scripts/tests/test_o.py",
                 'O = "plans/%s/ledger.md"\n' % WAVE)
        got = turnaround.python_readers(self.f.root, REPORT, specs=specs)
        self.assertNotIn("crates/x/src/w.rs", got)
        self.assertNotIn("scripts/tests/test_o.py", got)
        for reader in ("scripts/tests/test_g.py", "scripts/tests/test_d.py",
                       "scripts/tests/test_e.py"):
            self.assertIn(reader, got)
        self.assertIn("crates/x/src/w.rs", turnaround.python_readers(
            self.f.root, "plans/%s/berichte/other.md" % WAVE, specs=specs))

    def test_k_r1_offending_greps_once_over_all_paths(self):
        # K-R1: one `git grep --untracked` per diff path cost a merge
        # preparation 16 min (529 plans files); one grep over the list, per
        # path only on a hit and only over the hit files.
        paths = ["plans/%s/berichte/r%d.md" % (WAVE, i) for i in range(12)]
        tm.write(self.f.root, "scripts/tests/test_r5.py",
                 'P = "plans/%s/berichte/r5.md"\n' % WAVE)
        greps, real = [], turnaround._git

        def counting(repo, *args):
            if args and args[0] == "grep":
                greps.append(args)
            return real(repo, *args)
        with mock.patch.object(turnaround, "_git", counting):
            bad = turnaround.offending(paths, self.f.root)
        self.assertEqual(["plans/%s/berichte/r5.md" % WAVE], sorted(bad))
        self.assertIn("rule 8 python: scripts/tests/test_r5.py", bad[paths[5]])
        # One grep over every file, the rest over the hit file alone -- GH
        # #1343 adds the hit-only grep that tells a path from a bare name.
        self.assertEqual(1, len([a for a in greps if not a[-1].startswith(":(literal)")]))
        self.assertEqual(len(paths) + 3, len(greps))   # + the hit-only grep twice
        self.assertTrue(all(a[-1] == ":(literal)scripts/tests/test_r5.py" for a in greps[1:]))
        os.remove(os.path.join(self.f.root, "scripts/tests/test_r5.py"))
        greps.clear()
        with mock.patch.object(turnaround, "_git", counting):
            self.assertEqual({}, turnaround.offending(paths, self.f.root))
        self.assertEqual(1, len(greps))

    def test_h_n1_without_the_strand_worktree_a_reader_at_the_tip_refuses(self):
        s = self.own_reader_then_report("scripts/tests/test_x.py", 'REPORT = "%s"\n' % REPORT)
        tm.git(self.f.root, "worktree", "remove", "--force", s["wt"])
        rc, _, err = self.f.merge(["S1"], no_pre="owner")
        self.assertEqual(rc, merge.REFUSE_EXIT, err)
        self.assertIn("receipt_not_branch_head", err)
        self.assertIn("read by scripts/tests/test_x.py", err)

    def test_m_n3_without_the_strand_worktree_and_no_reader_it_merges(self):
        s = self.own_reader_then_report("scripts/tests/test_x.py", "x = 1\n")
        tm.git(self.f.root, "worktree", "remove", "--force", s["wt"])
        rc, _, err = self.f.merge(["S1"], no_pre="owner")
        self.assertEqual(rc, 0, err)

    def test_h_n1_a_dirty_reader_in_the_strand_worktree_is_still_the_tips(self):
        s = self.own_reader_then_report("scripts/tests/test_x.py", 'REPORT = "%s"\n' % REPORT)
        tm.write(s["wt"], "scripts/tests/test_x.py", "x = 1\n")  # hides the reader, uncommitted
        rc, _, err = self.f.merge(["S1"], no_pre="owner")
        self.assertEqual(rc, merge.REFUSE_EXIT, err)
        self.assertIn("read by scripts/tests/test_x.py", err)

    def test_m_n3_a_reader_hidden_by_assume_unchanged_is_still_the_tips(self):
        # Measured in the re-check: an index flag made the worktree read clean
        # while the disk copy had the reader disarmed -- rc 0.
        for reader, text in (
                ("scripts/tests/test_x.py", 'REPORT = "%s"\n' % REPORT),
                ("crates/x/tests/reads_report.rs",
                 'const P: &str = "%s";\n#[test]\nfn t() {}\n' % REPORT)):
            with self.subTest(reader=reader):
                self.f = tm.Fixture(self.gate, self.plans_tracked)  # a fresh repo per case
                self.addCleanup(self.f.close)
                s = self.own_reader_then_report(reader, text)
                tm.write(s["wt"], reader, "x = 1\n")
                tm.git(s["wt"], "update-index", "--assume-unchanged", reader)
                self.assertEqual("", tm.git(s["wt"], "status", "--porcelain",
                                            "--untracked-files=no"))
                rc, _, err = self.f.merge(["S1"], no_pre="owner")
                self.assertEqual(rc, merge.REFUSE_EXIT, err)
                self.assertIn("read by %s" % reader, err)

    def test_m_n3_a_dirty_non_reader_in_the_worktree_is_not_the_tips(self):
        # Only the commit is merged: an uncommitted reader the tip lacks
        # vetoes nothing.
        s = self.own_reader_then_report("scripts/tests/test_x.py", "x = 1\n")
        tm.write(s["wt"], "scripts/tests/test_x.py", 'REPORT = "%s"\n' % REPORT)
        rc, _, err = self.f.merge(["S1"], no_pre="owner")
        self.assertEqual(rc, 0, err)

    def test_h_a_python_reader_joining_the_wave_name_is_code(self):
        # The reader of test_wave_retro: `REPO / "plans" / "<wave>"`, no path literal.
        tm.write(self.f.root, "scripts/tests/test_y.py",
                 'import pathlib\nW = pathlib.Path("plans") / "%s"\n' % WAVE)
        why = turnaround.veto(REPORT, self.f.root)
        self.assertIsNotNone(why)
        self.assertIn("rule 8 python: scripts/tests/test_y.py", why)

    # GH #1343: the kit's own tests build `plans/<test wave>/berichte/S1.md`
    # (this module's REPORT, `os.path.join(wdir, "berichte", "S1.md")`); the
    # bare file name made them readers of every real wave's S1 report -- the
    # merge refused a live wave's S1 `receipt_not_branch_head` over a commit
    # of its report alone.
    FIXTURE = ('W = "welle-fixture-2026-01-01"\nR = "plans/%s/berichte/S1.md" % W\n'
               'import os\nP = os.path.join("tmp", "berichte", "S1.md")\n')

    def test_gh1343_a_strand_named_like_a_fixture_keeps_its_receipt(self):
        self.f.master_commit({"scripts/tests/test_fixture.py": self.FIXTURE,
                              "crates/x/tests/old.rs": "// plans/welle-old-2026-01-01/berichte/S1.md\n"})
        s = self.strand()
        self.stamp(s["wt"])
        self.bump(s["wt"])
        self.stamp(s["wt"])
        rc, out, err = self.f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        self.assertIn("MERGED S1", out)

    def test_gh1343_a_fixture_of_another_wave_reads_no_report_of_this_one(self):
        self.f.master_commit({"scripts/tests/test_fixture.py": self.FIXTURE})
        self.assertEqual({}, turnaround.reuse_readers(self.f.root, [REPORT], (("HEAD", "m"),)))
        self.assertEqual({}, turnaround.offending([REPORT], self.f.root, WAVE))

    def test_gh1343_the_file_name_with_the_wave_named_is_still_a_reader(self):
        # Fail-closed: a file naming the report's name AND the wave -- its
        # directory or the branch prefix `<wave>/` -- reads it.
        prefix = WAVE_PREFIX
        for text in ('W = "%s"\nP = os.path.join("plans", W, "berichte", "S1.md")\n' % WAVE,
                     'B = "%s/S1"\nP = os.path.join(d, "berichte", "S1.md")\n' % prefix):
            with self.subTest(text=text):
                self.f.master_commit({"scripts/tests/test_reader.py": text})
                got = turnaround.reuse_readers(self.f.root, [REPORT], (("HEAD", "m"),))
                self.assertEqual({REPORT: "read by scripts/tests/test_reader.py in m"}, got)
                self.assertIn("rule 8 python: scripts/tests/test_reader.py",
                              turnaround.offending([REPORT], self.f.root, WAVE).get(REPORT, ""))

    def test_h_test_wave_retro_reads_the_h2_and_h3_receipts(self):
        # The concrete reader of the re-check (B1), in this repository: the
        # h2 report is vetoed by rule 8 python, the h3 receipt lies outside
        # the allowlist since fix round 7; test_wave_retro names both.
        p = "plans/welle-h2-2026-09-18/berichte/x.md"
        why = turnaround.veto(p, str(REPO_ROOT))
        self.assertIn("rule 8 python", why or "")
        r = "plans/welle-h3-2026-09-18/receipts/release-0.39.0.md"
        self.assertEqual("outside the allowlist", turnaround.veto(r, str(REPO_ROOT)))
        for q in (p, r):
            self.assertIn("scripts/tests/test_wave_retro.py",
                          turnaround.python_readers(str(REPO_ROOT), q))

    def test_h_a_foreign_wave_report_breaks_the_stamp(self):
        # The stamp keeps the strand's own wave only, like the receipt.
        s = self.strand()
        self.stamp(s["wt"])
        foreign = "plans/welle-other-2026-01-02/berichte/S1.md"
        tm.write(s["wt"], foreign, "x\n")
        ok, line = pre.stamp_status(s["wt"])
        self.assertFalse(ok, line)
        self.assertIn(foreign, line)
        os.remove(os.path.join(s["wt"], foreign))
        tm.write(s["wt"], REPORT, tm.read(s["wt"], REPORT) + "own wave\n")
        ok, line = pre.stamp_status(s["wt"])
        self.assertTrue(ok, line)

    def test_h_a_shellcheck_path_an_allowlist_admits_stays_shell(self):
        # GH #1199 (e): the `shell` exception of `veto` holds only off
        # `gate_plan.SHELL_GLOBS` -- shellcheck reads those. The plan of a new
        # wave named like `plans/meclaw-os/` admits one of them
        # (`plan_allowlisted`); it must keep its `shell` class.
        sh = "plans/meclaw-os/x.sh"
        tm.write(self.f.root, sh, "#!/usr/bin/env bash\nexit 0\n")
        why = turnaround.veto(sh, self.f.root, wave="meclaw-os",
                              allow=turnaround.plan_allowlisted)
        self.assertEqual("classes docs,shell", why)
        # Off SHELL_GLOBS the same file name is docs (the re-check entry).
        rc = "plans/%s/recheck/skripte/S1/run_all.sh" % WAVE
        tm.write(self.f.root, rc, "#!/usr/bin/env bash\nexit 0\n")
        self.assertIsNone(turnaround.veto(rc, self.f.root))

    def test_h_neutral_is_the_allowlist_only(self):
        repo = self.f.root
        for p in (REPORT, STRAENGE):
            self.assertIsNone(turnaround.veto(p, repo), p)
        for p in (LEDGER, "plans/%s/receipts/S1/x/summary.txt" % WAVE,
                  "plans/%s/README.md" % WAVE, CORRIDOR, ADR,
                  "plans/%s/messvertrag.md" % WAVE,
                  "plans/%s/plan-parts/S1.md" % WAVE, "plans/export-fixtures/make_export.py",
                  "plans/README.md", "docs/x.md", "templates/a/foo.txt"):
            self.assertIsNotNone(turnaround.veto(p, repo), p)


# --- (b) --------------------------------------------------------------------------

class GateBaseTest(Case):

    PLAN = ["precheck", "clippy", "tests"]

    def receipt_with_base(self, s, base, doc_extra=None, plan_extra=None, stations=None):
        """A FULL strand receipt as the executor archives it: clean tree, every
        planned station run, no owner decision, a `plan.json` without `--only`."""
        d = self.f.receipt("S1", "20260102T000000Z-%s" % s["tip"][:8], "strand", s["tip"], "GREEN")
        doc = json.loads(tm.read(d, "last-strand.json"))
        doc.update({"base": base, "dirty": False, "plan": list(self.PLAN),
                    "stations": stations if stations is not None else
                    [{"name": n, "verdict": "GREEN"} for n in self.PLAN],
                    "owner_decisions": []})
        doc.update(doc_extra or {})
        tm.write(d, "last-strand.json", json.dumps(doc) + "\n")
        plan = {"mode": "strand", "rev": s["tip"], "base": base,
                "stations": [{"name": n} for n in self.PLAN]}
        plan.update(plan_extra or {})
        tm.write(d, "plan.json", json.dumps(plan) + "\n")
        return d

    def assert_full(self, why):
        rc, _, err = self.f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        call = self.f.gate_calls()[-1]
        self.assertEqual(call["gate_base"], call["base"])
        self.assertTrue(call["gate_reuse"].startswith("full"), call["gate_reuse"])
        self.assertIn(why, call["gate_reuse"])

    def test_b_master_unmoved_gates_the_merge_point_only(self):
        base = self.f.head()
        s = self.strand()
        self.receipt_with_base(s, base)
        rc, _, err = self.f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        call = self.f.gate_calls()[-1]
        self.assertEqual(call["gate_base"], s["tip"])
        self.assertIn("master unmoved", call["gate_reuse"])

    def test_b_master_moved_gates_the_difference(self):
        base = self.f.head()
        s = self.strand()
        self.receipt_with_base(s, base)
        self.f.master_commit({"docs/other.md": "Moved.\n"})
        rc, _, err = self.f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        call = self.f.gate_calls()[-1]
        self.assertEqual(call["gate_base"], s["tip"])
        self.assertIn("master moved", call["gate_reuse"])
        # m3: the planned diff gate_base..candidate carries master's move.
        planned = tm.git(self.f.root, "diff", "--name-only", call["gate_base"], call["rev"]).split()
        self.assertIn("docs/other.md", planned)

    # B2: only a FULL strand gate on a clean tree covers the tip.

    def test_b_a_dirty_receipt_runs_full(self):
        s = self.strand()
        self.receipt_with_base(s, self.f.head(), doc_extra={"dirty": True})
        self.assert_full("dirty")

    def test_b_an_only_receipt_runs_full(self):
        s = self.strand()
        self.receipt_with_base(s, self.f.head(), plan_extra={"only": ["tests"]})
        self.assert_full("--only")

    def test_b_a_no_pre_receipt_runs_full(self):
        s = self.strand()
        self.receipt_with_base(s, self.f.head(), doc_extra={"owner_decisions": [
            {"station": "pre", "decision": "pre_skipped", "reason": "x"}]})
        self.assert_full("owner decision")

    def test_b_a_cached_planned_station_counts_as_run(self):
        # Re-check H1 m-N2: the old `cache` criterion pinned a field no receipt
        # carries; what the cache does to a receipt is CACHED stations, and
        # those are a full strand gate (content key) -- the tip stays the base.
        base = self.f.head()
        s = self.strand()
        self.receipt_with_base(s, base, stations=[
            {"name": n, "verdict": "CACHED" if n == "tests" else "GREEN"} for n in self.PLAN])
        rc, _, err = self.f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        call = self.f.gate_calls()[-1]
        self.assertTrue(call["gate_reuse"].startswith("reuse"), call["gate_reuse"])

    def test_b_a_skipped_planned_station_runs_full(self):
        # m1: a SKIP on the strand lane is no verdict the queue may reuse.
        s = self.strand()
        self.receipt_with_base(s, self.f.head(), stations=[
            {"name": "precheck", "verdict": "GREEN"}, {"name": "clippy", "verdict": "SKIP"},
            {"name": "tests", "verdict": "CACHED"}])
        self.assert_full("clippy SKIP")

    def test_b_a_missing_planned_station_runs_full(self):
        s = self.strand()
        self.receipt_with_base(s, self.f.head(), stations=[
            {"name": "precheck", "verdict": "GREEN"}, {"name": "tests", "verdict": "GREEN"}])
        self.assert_full("clippy missing")

    def test_b_a_receipt_without_coverage_fields_runs_full(self):
        # An older receipt that does not say it was clean and complete.
        s = self.strand()
        d = self.f.receipt("S1", "20260102T000000Z-%s" % s["tip"][:8], "strand", s["tip"], "GREEN")
        doc = json.loads(tm.read(d, "last-strand.json"))
        doc["base"] = self.f.head()
        tm.write(d, "last-strand.json", json.dumps(doc) + "\n")
        self.assert_full("partial")

    def test_b_without_a_receipt_base_the_gate_runs_full(self):
        s = self.strand()
        rc, _, err = self.f.merge(["S1"])
        self.assertEqual(rc, 0, err)
        call = self.f.gate_calls()[-1]
        self.assertEqual(call["gate_base"], call["base"])
        self.assertNotEqual(call["gate_base"], s["tip"])
        self.assertTrue(call["gate_reuse"].startswith("full"))


# --- (c) --------------------------------------------------------------------------

class OnePassTest(Case):

    def test_c_every_missing_precondition_in_one_refusal(self):
        self.strand(report=False, receipt=False, stamp=False, in_branch=False)
        err = self.refused()
        for reason in ("no_report", "no_strand_receipt", "pre_stamp"):
            self.assertIn(reason, err)
        self.assertEqual(self.f.events("merge_refused")[-1]["reason"], "no_report")


# --- (d) --------------------------------------------------------------------------

class PlanOnlyTest(Case):

    def plan_branch(self, files):
        self.f.master_commit({LEDGER: "start\n"})
        wt = os.path.join(self.f.tmp, "wt-plan")
        tm.git(self.f.root, "worktree", "add", "-q", "-b", "welle-test/plan", wt, "master")
        return commit(wt, files, "plan")

    def test_d_a_plan_only_branch_merges_without_gate_and_report(self):
        tip = self.plan_branch({NOTES: "ruling\n", STRAENGE: "strand file\n"})
        rc, out, err = self.f.merge(["plan"])
        self.assertEqual(rc, 0, err)
        self.assertIn("MERGED plan", out)
        self.assertEqual(self.f.gate_calls(), [])
        self.assertEqual(self.f.parents()[1], tip)
        self.assertEqual(tm.read(self.f.root, NOTES), "ruling\n")

    def test_v2a10_a_branch_with_a_code_path_asks_no_reader_rule(self):
        # V2-A10 (h): the merge of v2-fix-b held tree:master 826 s before its
        # gate (journal seq 116541 -> 116542); `plan_only` ran the reader
        # greps of `code_equal_revs` over 310 re-check files (984 s in the
        # probe) although 53 code paths had already ruled plan-only out.
        self.strand()
        st = merge.Strand(self.f.root, "S1", tm.WAVE)
        tip = tm.git(self.f.root, "rev-parse", st.branch)
        with mock.patch.object(turnaround, "python_readers",
                               wraps=turnaround.python_readers) as pr:
            self.assertFalse(merge.plan_only(st, self.f.head(), tip))
        self.assertEqual(pr.call_count, 0, pr.call_args_list)

    def test_r7_a_plan_branch_editing_the_ledger_is_gated(self):
        # Fix round 7: the ledger left the allowlist -- a ledger line takes
        # the normal way (report, review, gate).
        self.plan_branch({LEDGER: "start\nruling\n"})
        err = self.refused("plan")
        self.assertIn("no_report", err)
        self.assertEqual(self.f.gate_calls(), [])

    def test_d_a_plan_only_branch_with_a_negative_review_is_refused(self):
        # Re-check H2 m6: a review that is there and says no is not ignored.
        self.plan_branch({NOTES: "ruling\n",
                          "plans/%s/berichte/plan.md" % WAVE:
                          "---\nstrand: plan\nbranch: welle-test/plan\n---\n\n"
                          "## Review\n\nVerdikt: nein\n"})
        err = self.refused("plan")
        self.assertIn("no_review_verdict", err)
        self.assertEqual(self.f.gate_calls(), [])

    def test_d_a_foreign_commit_on_a_plan_only_branch_is_refused(self):
        # Re-check H2 m7.
        self.f.master_commit({LEDGER: "start\n"})
        wt = os.path.join(self.f.tmp, "wt-plan")
        tm.git(self.f.root, "worktree", "add", "-q", "-b", "welle-test/plan", wt, "master")
        tm.write(wt, NOTES, "ruling\n")
        tm.git(wt, "add", "-A")
        env = dict(os.environ, GIT_AUTHOR_NAME="someone", GIT_AUTHOR_EMAIL="x@y.invalid")
        subprocess.run(["git", "-C", wt, "commit", "-q", "-m", "foreign"], env=env, check=True)
        err = self.refused("plan")
        self.assertIn("foreign_commits", err)

    def test_d_n4_a_plan_only_merge_that_brings_nothing_is_refused(self):
        # Re-check H2 m-N4 (closes OR-TB.S7.9), arm `not paths`: the branch's
        # neutral change is on master already -- the candidate diff is empty.
        tip = self.plan_branch({NOTES: "ruling\n"})
        self.f.master_commit({"docs/other.md": "x\n"})  # master moves on
        tm.git(self.f.root, "cherry-pick", tip)
        err = self.refused("plan")
        self.assertIn("plan_only_not_neutral", err)
        self.assertIn("touches nothing", err)
        self.assertEqual(self.f.gate_calls(), [])

    def test_d_n4_a_candidate_diff_the_reader_rules_veto_is_refused(self):
        # Arm `bad`: the branch passed `plan_only`, the candidate's own diff
        # is vetoed (the second `offending` call is the candidate's).
        self.plan_branch({NOTES: "ruling\n"})
        real, calls = turnaround.offending, []

        def offending(paths, repo, wave=None):
            calls.append(list(paths))
            return real(paths, repo, wave) if len(calls) == 1 else {NOTES: "vetoed"}

        with mock.patch.object(turnaround, "offending", offending):
            err = self.refused("plan")
        self.assertEqual(2, len(calls), calls)
        self.assertIn("plan_only_not_neutral", err)
        self.assertIn("%s (vetoed)" % NOTES, err)
        self.assertEqual(self.f.gate_calls(), [])

    def test_r7_a_plan_branch_a_reader_rule_vetoes_is_gated(self):
        # `plan_only` keeps the reader rules on top of the reuse rule: a
        # plan-only merge has no gate behind it. The reader is untracked in
        # the main tree: only `offending` (rule 8 python, `--untracked`) sees
        # it, `reuse_readers` reads commits.
        self.plan_branch({NOTES: "ruling\n"})
        tm.write(self.f.root, "scripts/tests/test_x.py", 'N = "%s"\n' % NOTES)
        err = self.refused("plan")
        self.assertIn("no_report", err)
        self.assertEqual(self.f.gate_calls(), [])

    def test_d_a_plan_branch_with_a_symlink_is_gated(self):
        # A plan-only merge lands without any gate: the reuse rule's
        # construction holds here too -- a symlink in the diff is no plan-only.
        self.f.master_commit({LEDGER: "start\n"})
        wt = os.path.join(self.f.tmp, "wt-plan")
        tm.git(self.f.root, "worktree", "add", "-q", "-b", "welle-test/plan", wt, "master")
        os.makedirs(os.path.dirname(os.path.join(wt, NOTES)), exist_ok=True)
        os.symlink("../ledger.md", os.path.join(wt, NOTES))
        commit(wt, {}, "plan")
        err = self.refused("plan")
        self.assertIn("no_report", err)
        self.assertEqual(self.f.gate_calls(), [])

    def test_d_a_plan_branch_with_a_plan_part_is_gated(self):
        self.plan_branch({"plans/%s/plan-parts/S9.md" % WAVE: "# part\n"})
        err = self.refused("plan")
        self.assertIn("no_report", err)
        self.assertEqual(self.f.gate_calls(), [])


# --- Fund V-1 (welle-validierung, GH #1203): the plan of a NEW wave ----------------
# 14.10. 00:27, main tree: `strand.sh merge --strand plan --wave
# <the new wave>` refused `no_wave` (the wave directory comes with
# the merge), with an absolute --wave `no_report/no_strand_receipt/pre_stamp`
# (README, ledger, plan-parts, PLAN-*.md lie outside berichte/straenge); the
# plan landed by hand (8fdfa581b). A wave master does not hold at the merge base
# is planned whole on its `<prefix>/plan` branch: every file of plans/<wave>/
# but receipts/, under the reuse rule's construction and the reader rules.
NEW_WAVE = "welle-neu-2026-02-02"
NEW_PLAN = {"plans/%s/README.md" % NEW_WAVE: "# plan\n",
            "plans/%s/ledger.md" % NEW_WAVE: "start\n",
            "plans/%s/PLAN-2026-02-02.md" % NEW_WAVE: "the plan\n",
            "plans/%s/plan-parts/s1.md" % NEW_WAVE: "# part\n"}


class NewWavePlanTest(Case):

    def plan_branch(self, files, branch="welle-neu/plan"):
        wt = os.path.join(self.f.tmp, "wt-new-plan")
        tm.git(self.f.root, "worktree", "add", "-q", "-b", branch, wt, "master")
        return commit(wt, files, "plan")

    def merge_new(self, strand="plan"):
        out, err = io.StringIO(), io.StringIO()
        rc = merge.merge([strand], start=self.f.root, wave=NEW_WAVE, db=self.f.db,
                         out_=out, err=err)
        return rc, out.getvalue(), err.getvalue()

    def refused_new(self, strand="plan"):
        rc, _, err = self.merge_new(strand)
        self.assertEqual(rc, merge.REFUSE_EXIT, err)
        self.assertEqual(self.f.gate_calls(), [])
        return err

    def test_v1_the_plan_of_a_new_wave_lands_plan_only(self):
        tip = self.plan_branch(NEW_PLAN)
        self.assertFalse(os.path.isdir(os.path.join(self.f.root, "plans", NEW_WAVE)))
        rc, out, err = self.merge_new()
        self.assertEqual(rc, 0, err)
        self.assertIn("MERGE-PLAN-ONLY plan", out)
        self.assertEqual(self.f.gate_calls(), [])
        self.assertEqual(self.f.parents()[1], tip)
        for rel, text in NEW_PLAN.items():
            self.assertEqual(tm.read(self.f.root, rel), text)

    def test_v1_the_receipt_reuse_rule_is_unchanged(self):
        for rel in NEW_PLAN:
            self.assertIsNotNone(turnaround.reuse_path(rel, NEW_WAVE), rel)

    def test_v1_a_new_wave_plan_with_receipts_is_gated(self):
        self.plan_branch(dict(NEW_PLAN, **{
            "plans/%s/receipts/S1/summary.txt" % NEW_WAVE: "GATE-SUMMARY\n"}))
        self.assertIn("no_report", self.refused_new())

    def test_v1_a_new_wave_plan_with_a_path_outside_its_wave_is_gated(self):
        self.plan_branch(dict(NEW_PLAN, **{"templates/a/foo.txt": "changed\n"}))
        self.assertIn("no_report", self.refused_new())

    # Re-check I-2: the wave boundary of `plan_path` is the only rule that
    # keeps these two out -- a docs page has the class {docs}, an existing
    # wave's ledger is named by no reader -- so plan-only stays narrow for
    # the waves master holds even when a new wave's plan touches them.
    def test_v1_a_new_wave_plan_editing_an_existing_waves_ledger_is_gated(self):
        self.plan_branch(dict(NEW_PLAN, **{LEDGER: "start\nruling\n"}))
        self.assertIn("no_report", self.refused_new())

    def test_v1_a_new_wave_plan_with_a_docs_page_is_gated(self):
        self.plan_branch(dict(NEW_PLAN, **{"docs/zz-probe-note.md": "probe\n"}))
        self.assertIn("no_report", self.refused_new())

    # Re-check I-1/m-3: the session markers a wave carries -- the build
    # session's DISPATCH-BAU.md, the planning session's HANDOVER-PLANUNG.md
    # (`retro.transcripts.PLANNING`) -- are no file-name needle: a text
    # naming another wave's marker reads no file of this plan.
    def test_v1_a_new_wave_plan_with_its_session_markers_lands_plan_only(self):
        self.f.master_commit({"scripts/tests/test_retro_prompts.py":
                              'BUILD = "Du faehrst plans/welle-t/DISPATCH-BAU.md."\n'
                              'PLAN = "plans/welle-t/HANDOVER-PLANUNG.md zuerst lesen."\n'})
        markers = {"plans/%s/DISPATCH-BAU.md" % NEW_WAVE: "# build\n",
                   "plans/%s/HANDOVER-PLANUNG.md" % NEW_WAVE: "# handover\n"}
        tip = self.plan_branch(dict(NEW_PLAN, **markers))
        rc, out, err = self.merge_new()
        self.assertEqual(rc, 0, err)
        self.assertIn("MERGE-PLAN-ONLY plan", out)
        self.assertEqual(self.f.gate_calls(), [])
        self.assertEqual(self.f.parents()[1], tip)
        for rel, text in markers.items():
            self.assertEqual(tm.read(self.f.root, rel), text)

    def test_v1_a_new_wave_plan_a_reader_names_is_gated(self):
        self.f.master_commit({"scripts/tests/test_reads_plan.py":
                              'P = "plans/%s/PLAN-2026-02-02.md"\n' % NEW_WAVE})
        self.plan_branch(NEW_PLAN)
        self.assertIn("no_report", self.refused_new())

    def test_v1_a_new_wave_plan_with_a_symlink_is_gated(self):
        wt = os.path.join(self.f.tmp, "wt-new-plan")
        tm.git(self.f.root, "worktree", "add", "-q", "-b", "welle-neu/plan", wt, "master")
        for rel, text in NEW_PLAN.items():
            tm.write(wt, rel, text)
        os.symlink("README.md", os.path.join(wt, "plans", NEW_WAVE, "alias.md"))
        commit(wt, {}, "plan")
        self.assertIn("no_report", self.refused_new())

    def test_v1_only_the_plan_strand_plans_a_new_wave_whole(self):
        self.plan_branch(NEW_PLAN, branch="welle-neu/S1")
        self.assertIn("no_report", self.refused_new("S1"))


# --- (e) / (f) ----------------------------------------------------------------------

class PreChecksTest(Case):

    def test_e_a_differing_main_tree_file_is_red(self):
        s = self.strand()
        tm.write(self.f.root, "templates/a/foo.txt", "foo\n")
        verdict, _ = turnaround.main_tree_check(s["wt"], ["templates/a/foo.txt"])
        self.assertEqual(verdict, "GREEN")  # byte-equal: the merge sets it aside
        tm.write(self.f.root, "templates/a/foo.txt", "stale copy\n")
        verdict, why = turnaround.main_tree_check(s["wt"], ["templates/a/foo.txt"])
        self.assertEqual(verdict, "RED")
        self.assertIn("templates/a/foo.txt", why)

    def test_i_h1_a_main_tree_file_git_quotes_is_red(self):
        # Re-check I-h1 (h), the same class: `git status --porcelain` without
        # `-z` quotes `templates/a/\303\244.txt`, the strand's path never
        # matched the status key and the check stayed GREEN.
        s = self.strand()
        rel = "templates/a/ä.txt"
        tm.write(self.f.root, rel, "stale copy\n")
        verdict, why = turnaround.main_tree_check(s["wt"], [rel])
        self.assertEqual(verdict, "RED", why)
        self.assertIn(rel, why)

    def test_f_a_foreign_commit_is_red_with_its_sha(self):
        s = self.strand()
        self.assertEqual(turnaround.owner_check(s["wt"])[0], "GREEN")
        env = dict(os.environ, GIT_AUTHOR_NAME="someone", GIT_AUTHOR_EMAIL="x@y.invalid")
        tm.write(s["wt"], "templates/a/bar.txt", "bar\n")
        tm.git(s["wt"], "add", "-A")
        subprocess.run(["git", "-C", s["wt"], "commit", "-q", "-m", "foreign"], env=env, check=True)
        sha = tm.git(s["wt"], "rev-parse", "HEAD")
        verdict, why = turnaround.owner_check(s["wt"])
        self.assertEqual(verdict, "RED")
        self.assertIn(sha[:9], why)

    def test_e_a_tracked_main_tree_change_is_a_note(self):
        # Re-check H2 m5: often the orchestrator's own and gone by the merge;
        # the fast-forward still refuses it then.
        self.f.master_commit({LEDGER: "start\n"})
        s = self.strand()
        tm.write(self.f.root, LEDGER, "orchestrator edit\n")
        verdict, why = turnaround.main_tree_check(s["wt"], [LEDGER])
        self.assertEqual(verdict, "NOTE")
        self.assertIn(LEDGER, why)

    def test_e_an_unknown_main_tree_is_red(self):
        # Re-check H2 m4: fail closed.
        verdict, _ = turnaround.main_tree_check(os.path.join(self.f.tmp, "no-such-tree"), ["x"])
        self.assertEqual(verdict, "RED")

    def test_f_without_an_owner_identity_is_red(self):
        s = self.strand()
        with mock.patch.dict(os.environ):
            os.environ.pop(turnaround.OWNER_ENV, None)
            verdict, why = turnaround.owner_check(s["wt"])
        self.assertEqual(verdict, "RED")
        self.assertIn("no owner identity", why)

    def test_f_the_repo_identity_is_the_owner(self):
        # Re-check H2 m3: not only `--global` -- includes and a local setting.
        s = self.strand()
        tm.git(s["wt"], "config", "user.name", "test")
        tm.git(s["wt"], "config", "user.email", "test@example.invalid")
        with mock.patch.dict(os.environ):
            os.environ.pop(turnaround.OWNER_ENV, None)
            self.assertEqual(turnaround.owner(s["wt"]), OWNER)
            self.assertEqual(turnaround.owner_check(s["wt"])[0], "GREEN")

    def test_f_a_foreign_committer_alone_is_red(self):
        s = self.strand()
        env = dict(os.environ, GIT_COMMITTER_NAME="someone", GIT_COMMITTER_EMAIL="x@y.invalid")
        tm.write(s["wt"], "templates/a/bar.txt", "bar\n")
        tm.git(s["wt"], "add", "-A")
        subprocess.run(["git", "-C", s["wt"], "commit", "-q", "-m", "foreign"], env=env, check=True)
        verdict, why = turnaround.owner_check(s["wt"])
        self.assertEqual(verdict, "RED")
        self.assertIn("committer someone", why)

    def test_e_f_run_pre_carries_both_checks(self):
        s = self.strand()
        tm.write(self.f.root, "templates/a/foo.txt", "stale copy\n")
        lines = []
        with mock.patch.object(pre, "local_half", return_value=[]), \
                mock.patch.object(pre, "unittest_half", return_value=[]), \
                mock.patch.object(pre, "record_in_store", return_value=[]):
            doc = pre.run_pre(s["wt"], base=self.f.first, out=lines.append)
        checks = {c["check"]: c["verdict"] for c in doc["checks"]}
        self.assertEqual(checks.get("main-tree"), "RED")
        self.assertEqual(checks.get("owner"), "GREEN")
        self.assertEqual(doc["verdict"], "RED")
        self.assertEqual(doc["head"], s["tip"])
        self.assertIn("overlay", doc)


class DefaultBranchTest(unittest.TestCase):
    """K13-1: check (f) took `master..HEAD` as given. orga-hive and soul2me have
    only `main`: `pre` was RED with `git log master..HEAD failed`. The range
    starts at the repo's default branch: local `master`, else local `main`, else
    the name `origin/HEAD` points at; none of them is a clear RED, not a silent
    pass. Fix round 5 (M7): a local branch before `origin/HEAD` -- meclaw-core's
    origin is the export mirror, whose `origin/main` has a history of its own."""

    def setUp(self):
        env = mock.patch.dict(os.environ, {turnaround.OWNER_ENV: OWNER})
        env.start()
        self.addCleanup(env.stop)
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.repo = tmp.name

    def repo_on(self, branch):
        tm.git(self.repo, "init", "-q", "-b", branch)
        tm.git(self.repo, "config", "user.name", "test")
        tm.git(self.repo, "config", "user.email", "test@example.invalid")
        return commit(self.repo, {"README.md": "start\n"}, "start")

    def foreign_commit(self, rel):
        env = dict(os.environ, GIT_AUTHOR_NAME="someone", GIT_AUTHOR_EMAIL="x@y.invalid")
        tm.write(self.repo, rel, "foreign\n")
        tm.git(self.repo, "add", "-A")
        subprocess.run(["git", "-C", self.repo, "commit", "-q", "-m", "foreign"], env=env,
                       check=True)
        return tm.git(self.repo, "rev-parse", "HEAD")

    def test_a_repo_with_only_main_checks_main_dot_dot_head(self):
        self.repo_on("main")
        tm.git(self.repo, "checkout", "-q", "-b", "welle-x/S1")
        commit(self.repo, {"a.txt": "a\n"})
        self.assertEqual(turnaround.owner_check(self.repo), ("GREEN", ""))
        sha = self.foreign_commit("b.txt")
        verdict, why = turnaround.owner_check(self.repo)
        self.assertEqual(verdict, "RED")
        self.assertIn(sha[:9], why)
        self.assertIn("main..HEAD", why)

    def test_a_local_master_wins_over_origin_head_on_main(self):
        # meclaw-core with a set origin/HEAD: local master, origin/main = the export mirror.
        self.repo_on("master")
        tm.git(self.repo, "checkout", "-q", "-b", "mirror")
        mirror = self.foreign_commit("mirror.txt")              # the mirror's own history
        tm.git(self.repo, "update-ref", "refs/remotes/origin/main", mirror)
        tm.git(self.repo, "symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/main")
        tm.git(self.repo, "checkout", "-q", "master")
        tm.git(self.repo, "branch", "-q", "-D", "mirror")
        tm.git(self.repo, "checkout", "-q", "-b", "welle-x/S1")
        commit(self.repo, {"a.txt": "a\n"})
        self.assertEqual(turnaround.default_branch(self.repo), "master")
        self.assertEqual(turnaround.owner_check(self.repo), ("GREEN", ""))

    def test_a_local_main_wins_over_origin_head_on_trunk(self):
        # Re-check FR5 I1: the local half of M7 was pinned for `master` only -- a
        # search of local `master` alone stayed green. No local master, a local
        # main, origin/HEAD -> origin/trunk with a history of its own: `main`.
        self.repo_on("main")
        tm.git(self.repo, "checkout", "-q", "-b", "mirror")
        mirror = self.foreign_commit("mirror.txt")              # the remote's own history
        tm.git(self.repo, "update-ref", "refs/remotes/origin/trunk", mirror)
        tm.git(self.repo, "symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/trunk")
        tm.git(self.repo, "checkout", "-q", "main")
        tm.git(self.repo, "branch", "-q", "-D", "mirror")
        tm.git(self.repo, "checkout", "-q", "-b", "welle-x/S1")
        commit(self.repo, {"a.txt": "a\n"})
        self.assertEqual(turnaround.default_branch(self.repo), "main")
        self.assertEqual(turnaround.owner_check(self.repo), ("GREEN", ""))

    def test_origin_head_names_the_branch_when_no_local_master_or_main(self):
        self.repo_on("trunk")
        trunk = tm.git(self.repo, "rev-parse", "HEAD")
        tm.git(self.repo, "update-ref", "refs/remotes/origin/trunk", trunk)
        tm.git(self.repo, "symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/trunk")
        self.assertEqual(turnaround.default_branch(self.repo), "trunk")
        tm.git(self.repo, "checkout", "-q", "--detach")
        tm.git(self.repo, "branch", "-q", "-D", "trunk")
        self.assertEqual(turnaround.default_branch(self.repo), "origin/trunk")

    def test_master_before_main_without_origin_head(self):
        self.repo_on("main")
        tm.git(self.repo, "branch", "master")
        self.assertEqual(turnaround.default_branch(self.repo), "master")

    def test_no_default_branch_is_a_clear_red(self):
        self.repo_on("trunk")
        self.assertIsNone(turnaround.default_branch(self.repo))
        verdict, why = turnaround.owner_check(self.repo)
        self.assertEqual(verdict, "RED")
        self.assertIn("no default branch", why)
        self.assertIn("master, main, origin/HEAD", why)


# --- (i) --------------------------------------------------------------------------

class BranchWaveTest(unittest.TestCase):
    """GH #1294 (OR-NZ-4): the branch `folge-0101/K` names the wave
    `plans/folge-0101`, a directory without a date suffix -- as `strand.sh`
    resolves it (`branch_wave_dir`); a dated wave of the same prefix loses."""

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.repo = tmp.name
        tm.git(self.repo, "init", "-q", "-b", "folge-0101/K")
        tm.git(self.repo, "config", "user.name", "test")
        tm.git(self.repo, "config", "user.email", "test@example.invalid")
        commit(self.repo, {"README.md": "start\n"}, "start")

    def test_a_wave_without_a_date_is_the_branch_wave(self):
        os.makedirs(os.path.join(self.repo, "plans", "folge-0101", "berichte"))
        self.assertEqual("folge-0101", turnaround.branch_wave(self.repo))
        os.makedirs(os.path.join(self.repo, "plans", "folge-0101-2026-10-01"))
        self.assertEqual("folge-0101", turnaround.branch_wave(self.repo))

    def test_a_dated_wave_resolves_as_before(self):
        os.makedirs(os.path.join(self.repo, "plans", "folge-0101-2026-10-01"))
        self.assertEqual("folge-0101-2026-10-01", turnaround.branch_wave(self.repo))


class PartKeysTest(unittest.TestCase):

    def test_i_the_dispatch_carries_the_plan_part_keys(self):
        from pipeline import cache
        keys = {("tests", "a::x"): {"key": "1" * 64}, ("clippy", ""): {"key": "2" * 64}}
        self.assertEqual({"tests::a::x": "1" * 64}, cache.part_keys(keys))
        self.assertEqual({}, cache.part_keys({}))


if __name__ == "__main__":
    unittest.main()
