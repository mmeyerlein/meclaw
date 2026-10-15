"""Tests for `scripts/pipeline/wave.py` -- `pipeline wave close <wave>`.

`wave close` is the lock behind "waves clear their own issues": it refuses
while an issue of the wave is open without a ruling or investigation mark,
then runs the retro, prepares the safety bump (never runs it here), checks
the Orga inputs per strand report and writes the Orga export. GitHub is a
fake behind the injected `runner`; every test builds its own tree and store.
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
from unittest import mock
from scripts.tests import kit_tree, store_isolation

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO))

from scripts.pipeline import store, wave  # noqa: E402

_METRICS_URL = mock.patch.dict(os.environ, {"MECLAW_PIPELINE_METRICS_URL": "",
                                                "MECLAW_LANES_FILE": os.devnull})


def setUpModule():
    store_isolation.isolate()
    _METRICS_URL.start()


def tearDownModule():
    _METRICS_URL.stop()
    store_isolation.restore()


WAVE = "welle-x-2026-10-08"
LINE = re.compile(r"^WAVE (\S+) issues=(\d+)/(\d+) retro=(\S+) bump=(\S+) export=(\S+)$")


def report(strand, issues, body):
    return (f"---\nstrand: {strand}\nbranch: x/{strand}\nissues: [{', '.join(issues)}]\n---\n\n"
            f"# Strand {strand}\n\n{body}\n")


def _git(root, *args):
    env = dict(os.environ, GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@t", GIT_COMMITTER_NAME="t",
               GIT_COMMITTER_EMAIL="t@t")
    for k in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR"):
        env.pop(k, None)
    return subprocess.run(["git", "-C", str(root), *args], check=True, capture_output=True,
                          text=True, env=env).stdout.strip()


class FakeGh:
    """`gh issue view <n> --json state,labels,title`, `gh issue list` (the issues
    created since a date, GH #1196) and the retro script, recorded."""

    def __init__(self, issues, created=None, list_rc=0):
        self.issues = issues
        self.created = created or {}      # {n: (state, createdAt, labels)}
        self.list_rc = list_rc
        self.calls = []

    def __call__(self, argv, **kw):
        self.calls.append(list(argv))
        if argv[:1] == ["git"]:
            # the test tree is a real repository: git reads pass through, recorded
            return subprocess.run(argv, check=False, **kw)
        if argv[:3] == ["gh", "issue", "list"]:
            if self.list_rc:
                return subprocess.CompletedProcess(argv, self.list_rc, "", "HTTP 502")
            rows = [{"number": n, "state": st, "createdAt": at, "title": f"issue {n}",
                     "labels": [{"name": name} for name in lbs]}
                    for n, (st, at, lbs) in sorted(self.created.items())]
            return subprocess.CompletedProcess(argv, 0, json.dumps(rows), "")
        if argv[:3] == ["gh", "issue", "view"]:
            n = int(argv[3])
            if n not in self.issues:
                return subprocess.CompletedProcess(argv, 1, "", "not found")
            state, labels = self.issues[n]
            body = {"state": state, "title": f"issue {n}",
                    "labels": [{"name": name} for name in labels]}
            return subprocess.CompletedProcess(argv, 0, json.dumps(body), "")
        return subprocess.CompletedProcess(argv, 0, "retro written\n", "")


class WaveClose(unittest.TestCase):

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.tmp.name)
        self.wdir = self.root / "plans" / WAVE
        (self.wdir / "berichte").mkdir(parents=True)
        (self.wdir / "berichte" / "s1.md").write_text(
            report("s1", ["#11", "#12"], "Orga: E-DT-3 extended."), encoding="utf-8")
        (self.wdir / "berichte" / "s2.md").write_text(
            report("s2", ["#13"], "E-DT: none"), encoding="utf-8")
        (self.wdir / "berichte" / "s2-addendum.md").write_text("# addendum\n", encoding="utf-8")
        (self.wdir / "orga-eingaben.md").write_text(
            "# inputs\n\n- **E-DT-3 — journal kinds.** text. — W0\n", encoding="utf-8")
        self.db_path = self.root / "p.db"
        db = store.open(self.db_path)
        wp = store.item_create(db, "workpackage", "pass", at="2026-10-08T10:00:00Z",
                               doc={"repo": "meclaw", "mode": "strand", "strand": "s1",
                                    "run": "run-1", "plan": ["precheck"]})
        t = store.item_create(db, "task", "precheck", parent=wp["id"], at="2026-10-08T10:00:01Z")
        r = store.run_open(db, t["id"], "lane-a", key=t["key"], at="2026-10-08T10:00:02Z")
        store.run_close(db, r, "GREEN", run_s=1.0, at="2026-10-08T10:00:03Z")
        store.close(db)
        self.lines = []
        self.no_retro = mock.patch.object(wave, "_retro", return_value=None)
        self.no_retro.start()
        # welle-tb S1 C-Beleg: close needs a GREEN coverage map on the end tip (master);
        # the tree is a repository with one commit, the map lives in a private directory
        _git(self.root, "init", "-q", "-b", "master")
        # GH #1196: the balance window opens at the commit time of the start rev
        with mock.patch.dict(os.environ, {"GIT_COMMITTER_DATE": "2026-10-08T06:00:00Z"}):
            _git(self.root, "commit", "-q", "--allow-empty", "-m", "tip")
        self.tip = _git(self.root, "rev-parse", "HEAD")
        # GH #1196: the issue balance needs the wave window; `wave start` journals it.
        self.start_wave("2026-10-08T06:00:00Z")
        self.maps = self.root.parent / (self.root.name + "-coverage")
        self.maps.mkdir()
        self.env = mock.patch.dict(os.environ, {"MECLAW_PIPELINE_COVERAGE_DIR": str(self.maps)})
        self.env.start()
        self.write_map(self.tip, "GREEN")

    def tearDown(self):
        self.env.stop()
        self.no_retro.stop()
        self.tmp.cleanup()
        shutil.rmtree(self.maps, ignore_errors=True)

    def start_wave(self, at, db_path=None):
        from scripts.pipeline import events
        db = store.open(db_path or self.db_path)
        events.emit(db, "wave_started", "status", {"wave": WAVE, "start": self.tip}, at=at)
        store.close(db)

    def write_map(self, rev, verdict):
        from scripts.pipeline import coverage
        parts = [{"binary": "meclaw-cells", "module": "gh2", "files": {"a.rs": 1}}]
        red = ["meclaw-cells gh2::broken"] if verdict == "RED" else []
        exit_ = {"GREEN": 0, "RED": 100, "INCOMPLETE": 124}[verdict]
        cov = coverage.coverage_map(rev, "lane-a", parts, 1.0, 1, run_ok=coverage.run_ok(exit_),
                                    expected_ids=["meclaw-cells"], nextest_exit=exit_,
                                    red_tests=red)
        (self.maps / ("coverage-%s.json" % rev)).write_text(json.dumps(cov), encoding="utf-8")

    def close(self, gh, **kw):
        self.lines = []
        kw.setdefault("dry_run", True)
        return wave.close(WAVE, root=self.root, db_path=self.db_path, runner=gh,
                          out=self.lines.append, now="2026-10-09T00:00:00Z", **kw)

    def wave_line(self):
        hits = [ln for ln in self.lines if ln.startswith("WAVE ")]
        self.assertEqual(1, len(hits), self.lines)
        return hits[0]

    def snapshot(self):
        # SQLite's own -wal/-shm beside the store appear for any reader of a WAL
        # database; they are the store's, not something the command wrote.
        return sorted((str(p), p.stat().st_size) for p in self.root.rglob("*")
                      if p.is_file() and not p.name.endswith(("-wal", "-shm")))

    def test_wave_close_refuses_open_issue_without_ruling_mark(self):
        gh = FakeGh({11: ("CLOSED", []), 12: ("OPEN", ["bug"]), 13: ("CLOSED", [])})
        rc = self.close(gh, issues=(14,))
        self.assertNotEqual(0, rc)
        m = LINE.match(self.wave_line())
        self.assertEqual(("2", "4", "skipped", "skipped", "skipped"), m.groups()[1:])
        self.assertTrue(any("#12" in ln and "OPEN" in ln for ln in self.lines), self.lines)
        self.assertTrue(any("#14" in ln for ln in self.lines), self.lines)   # gh could not read it
        self.assertFalse(any("wave_retro.py" in " ".join(c) for c in gh.calls))
        # A ruling label lets the open issue pass ...
        gh = FakeGh({11: ("CLOSED", []), 12: ("OPEN", ["Ruling"]), 13: ("CLOSED", [])})
        self.assertEqual(0, self.close(gh))
        self.assertEqual("3", LINE.match(self.wave_line()).group(2))
        # ... and so does a mark line in the wave folder.
        gh = FakeGh({11: ("CLOSED", []), 12: ("OPEN", []), 13: ("CLOSED", [])})
        self.assertNotEqual(0, self.close(gh))
        (self.wdir / "rulings.md").write_text("- OPEN #12 investigation: flake measured\n",
                                              encoding="utf-8")
        self.assertEqual(0, self.close(gh))
        # Only the read-only `gh issue view` / `gh issue list` ever touch GitHub.
        for c in gh.calls:
            if c[0] == "gh":
                self.assertIn(c[:3], (["gh", "issue", "view"], ["gh", "issue", "list"]))

    def test_wave_close_line_format(self):
        gh = FakeGh({11: ("CLOSED", []), 12: ("CLOSED", []), 13: ("CLOSED", [])})
        before = self.snapshot()
        self.assertEqual(0, self.close(gh))
        self.assertEqual(f"WAVE {WAVE} issues=3/3 retro=dry bump=prepared export=dry", self.wave_line())
        self.assertEqual(before, self.snapshot())                 # dry-run writes nothing
        self.assertTrue(any(ln.startswith("BUMP prepared:") for ln in self.lines), self.lines)
        self.assertTrue(any(ln == "EDT reports=2/2" for ln in self.lines), self.lines)
        rc = self.close(gh, dry_run=False)
        self.assertEqual(0, rc, self.lines)
        self.assertEqual(f"WAVE {WAVE} issues=3/3 retro=ok bump=prepared export=ok", self.wave_line())
        retro = [c for c in gh.calls if c[-2:] == [str(self.root / "scripts" / "wave_retro.py"), WAVE]]
        self.assertEqual(1, len(retro))
        out = self.wdir / "orga-export"
        self.assertTrue((out / f"{WAVE}.graph.json").is_file())
        self.assertTrue((out / f"{WAVE}.runs.json").is_file())
        # A failing retro script is named, not hidden.
        bad = FakeGh(gh.issues)
        bad_call = bad.__call__

        def failing(argv, **kw):
            res = bad_call(argv, **kw)
            # gh and the balance's git reads (GH #1196) answer; the retro script fails
            return (res if argv[0] in ("gh", "git")
                    else subprocess.CompletedProcess(argv, 3, "", "boom"))
        self.assertNotEqual(0, self.close(failing, dry_run=False))
        self.assertEqual("failed", LINE.match(self.wave_line()).group(4))

    def test_wave_close_refuses_red_end_tip_coverage(self):
        # welle-tb S1 C-Beleg (KR-TB-1 a): the map build on the end tip is the full suite
        # of the wave; red or incomplete there is a finding, never a closed wave.
        gh = FakeGh({11: ("CLOSED", []), 12: ("CLOSED", []), 13: ("CLOSED", [])})
        self.write_map(self.tip, "RED")
        for dry in (True, False):
            gh.calls.clear()
            self.assertEqual(1, self.close(gh, dry_run=dry), self.lines)
            self.assertEqual(f"WAVE {WAVE} issues=3/3 retro=skipped bump=skipped export=skipped",
                             self.wave_line())
            text = "\n".join(self.lines)
            self.assertIn("COVERAGE end-tip=%s verdict=RED" % self.tip[:9], text)
            self.assertIn("meclaw-cells gh2::broken", text)
            # no retro ran: only gh and the balance's read-only git calls (GH #1196)
            self.assertFalse(any(c[0] not in ("gh", "git") for c in gh.calls), gh.calls)
        self.assertFalse((self.wdir / "orga-export").exists())
        # incomplete, missing, or only a map of an older rev: refused as well (fail-closed)
        self.write_map(self.tip, "INCOMPLETE")
        self.assertEqual(1, self.close(gh))
        self.assertIn("verdict=INCOMPLETE", "\n".join(self.lines))
        (self.maps / ("coverage-%s.json" % self.tip)).unlink()
        old = self.tip
        self.write_map(old, "GREEN")
        _git(self.root, "commit", "-q", "--allow-empty", "-m", "merge after the map")
        self.tip = _git(self.root, "rev-parse", "HEAD")
        self.assertEqual(1, self.close(gh))
        self.assertIn("COVERAGE end-tip=%s verdict=MISSING" % self.tip[:9], "\n".join(self.lines))
        # a GREEN map on the end tip lets the close go on
        self.write_map(self.tip, "GREEN")
        self.assertEqual(0, self.close(gh), self.lines)
        self.assertIn("COVERAGE end-tip=%s verdict=GREEN" % self.tip[:9], "\n".join(self.lines))

    def test_wave_close_refuses_a_report_without_orga_inputs(self):
        (self.wdir / "berichte" / "s3.md").write_text(report("s3", [], "nothing about Orga"),
                                                      encoding="utf-8")
        gh = FakeGh({11: ("CLOSED", []), 12: ("CLOSED", []), 13: ("CLOSED", [])})
        self.assertNotEqual(0, self.close(gh))
        self.assertIn("EDT reports=2/3", self.lines)
        self.assertTrue(any(ln.startswith("EDT s3 ") for ln in self.lines), self.lines)

    def test_wave_close_bump_runs_only_with_versions_py_and_confirmation(self):
        gh = FakeGh({11: ("CLOSED", []), 12: ("CLOSED", []), 13: ("CLOSED", [])})
        self.assertEqual(0, self.close(gh, dry_run=False, bump=True, confirm=lambda q: True))
        self.assertIn("bump=prepared", self.wave_line())          # no versions.py in this tree
        self.assertFalse(any("versions.py" in " ".join(c) for c in gh.calls if c[0] != "gh"))

    # welle-validierung-2 3b (GH #1199): close turns the Running row of section Live
    # Built; a wave opened before `wave open` existed gets a new Built row.
    LIVE = ("# Plans\n\n## Live\n\n| Path | What it is |\n|---|---|\n%s"
            "| `welle-w-2026-10-01/` | **Built 2026-10-02:** Wave W. |\n\n## Archive\n")

    def live_close(self, row, dry_run=False):
        readme = self.root / "plans" / "README.md"
        readme.write_text(self.LIVE % row, encoding="utf-8")
        gh = FakeGh({11: ("CLOSED", []), 12: ("CLOSED", []), 13: ("CLOSED", [])})
        self.assertEqual(0, self.close(gh, dry_run=dry_run), self.lines)
        return [ln for ln in readme.read_text(encoding="utf-8").splitlines()
                if ln.startswith("| `")]

    def test_wave_close_turns_the_running_row_built(self):
        rows = self.live_close("| `%s/` | **Running since 2026-10-08:** Wave X. |\n" % WAVE)
        self.assertEqual("| `%s/` | **Built 2026-10-09:** Wave X. |" % WAVE, rows[0])
        self.assertEqual(2, len(rows), rows)
        self.assertTrue(any(ln.startswith("LIVE ") and "describe" in ln for ln in self.lines),
                        self.lines)

    def test_wave_close_without_a_running_row_writes_a_built_row(self):
        rows = self.live_close("")
        self.assertEqual(2, len(rows), rows)
        self.assertTrue(rows[0].startswith("| `%s/` | **Built 2026-10-09:** " % WAVE), rows)

    def test_wave_close_dry_run_leaves_the_running_row(self):
        row = "| `%s/` | **Running since 2026-10-08:** Wave X. |" % WAVE
        rows = self.live_close(row + "\n", dry_run=True)
        self.assertEqual(row, rows[0])
        self.assertTrue(any(ln.startswith("LIVE dry:") for ln in self.lines), self.lines)


    # --- GH #1196: the issue balance of the wave ---------------------------------
    # Issues created in the wave window (from `wave start` / `wave open`) and named
    # in the wave's reports or ledger, minus the closed ones; only an owner ruling
    # (`OPEN #<n> owner-ruling: <reference>`) excuses one the wave opened itself.

    CLOSED_3 = {11: ("CLOSED", []), 12: ("CLOSED", []), 13: ("CLOSED", [])}

    def found(self, n, where="berichte/s1-notes.md"):
        path = self.wdir / where
        path.write_text("# notes\n\nfinding moved to #%d\n" % n, encoding="utf-8")

    def balance_line(self):
        hits = [ln for ln in self.lines if ln.startswith("BALANCE opened=")]
        self.assertEqual(1, len(hits), self.lines)
        return hits[0]

    def test_wave_close_balance_zero_passes(self):
        self.found(20)
        (self.wdir / "ledger.md").write_text("- 10:00 #21 opened and built\n", encoding="utf-8")
        gh = FakeGh(self.CLOSED_3, created={
            20: ("CLOSED", "2026-10-08T09:00:00Z", []),   # opened by the wave, closed
            21: ("CLOSED", "2026-10-08T10:00:00Z", []),   # named in the ledger, closed
            22: ("OPEN", "2026-10-08T11:00:00Z", []),     # not named: another wave's
            11: ("CLOSED", "2026-10-08T07:00:00Z", [])})
        self.assertEqual(0, self.close(gh), self.lines)
        self.assertEqual("BALANCE opened=3 closed=3 excused=0 balance=0 "
                         "since=2026-10-08T06:00:00Z (wave start)", self.balance_line())
        lists = [c for c in gh.calls if c[:3] == ["gh", "issue", "list"]]
        self.assertEqual(1, len(lists), gh.calls)
        self.assertIn("created:>=2026-10-08T06:00:00Z", " ".join(lists[0]))

    def test_wave_close_balance_one_without_ruling_is_refused(self):
        self.found(20)
        gh = FakeGh(self.CLOSED_3, created={20: ("OPEN", "2026-10-08T09:00:00Z", [])})
        for dry in (True, False):
            self.assertEqual(1, self.close(gh, dry_run=dry), self.lines)
            self.assertEqual("BALANCE opened=1 closed=0 excused=0 balance=1 "
                             "since=2026-10-08T06:00:00Z (wave start)", self.balance_line())
            self.assertIn("BALANCE #20 OPEN opened by the wave, no owner ruling: issue 20",
                          self.lines)
            self.assertIn("wave close: refused -- the wave opened more issues than it closed",
                          self.lines)
            self.assertEqual(f"WAVE {WAVE} issues=3/3 retro=skipped bump=skipped export=skipped",
                             self.wave_line())
        self.assertFalse((self.wdir / "orga-export").exists())

    def test_wave_close_balance_one_with_investigation_mark_only_is_refused(self):
        # The existing marks and labels excuse an issue that predates the wave,
        # never one the wave opened itself.
        self.found(20)
        (self.wdir / "rulings.md").write_text(
            "- OPEN #20 investigation: flake measured\n- OPEN #20 ruling: later\n",
            encoding="utf-8")
        gh = FakeGh(self.CLOSED_3, created={
            20: ("OPEN", "2026-10-08T09:00:00Z", ["investigation", "needs-ruling"])})
        self.assertEqual(1, self.close(gh), self.lines)
        self.assertIn("balance=1 ", self.balance_line())
        # a placeholder reference is no owner ruling either
        (self.wdir / "rulings.md").write_text("- OPEN #20 owner-ruling: <reference>\n",
                                              encoding="utf-8")
        self.assertEqual(1, self.close(gh), self.lines)
        self.assertIn("balance=1 ", self.balance_line())

    def test_wave_close_balance_one_with_owner_ruling_passes(self):
        self.found(20)
        (self.wdir / "ledger.md").write_text(
            "- OPEN #20 owner-ruling: ledger 2026-10-08 R-7 (the owner keeps it open)\n",
            encoding="utf-8")
        gh = FakeGh(self.CLOSED_3, created={20: ("OPEN", "2026-10-08T09:00:00Z", [])})
        self.assertEqual(0, self.close(gh), self.lines)
        self.assertEqual("BALANCE opened=1 closed=0 excused=1 balance=0 "
                         "since=2026-10-08T06:00:00Z (wave start)", self.balance_line())
        self.assertTrue(any(ln.startswith("BALANCE #20 OPEN excused (owner-ruling ledger "
                                          "2026-10-08 R-7") for ln in self.lines), self.lines)
        # the owner ruling marks a header issue of the wave as well (today's check)
        gh = FakeGh({11: ("CLOSED", []), 12: ("OPEN", []), 13: ("CLOSED", [])})
        (self.wdir / "ledger.md").write_text("- OPEN #12 owner-ruling: R-8\n", encoding="utf-8")
        self.assertEqual(0, self.close(gh), self.lines)

    def test_wave_close_without_a_wave_start_fails_closed(self):
        db_path = self.root / "fresh.db"
        store.close(store.open(db_path))
        gh = FakeGh(self.CLOSED_3)
        self.lines = []
        rc = wave.close(WAVE, root=self.root, db_path=db_path, runner=gh,
                        out=self.lines.append, now="2026-10-09T00:00:00Z", dry_run=True)
        self.assertEqual(1, rc, self.lines)
        self.assertTrue(any(ln.startswith("BALANCE unknown: no wave start") for ln in self.lines),
                        self.lines)
        # the remedy names a command that exists: `wave start` is pipeline.py's, strand.sh
        # routes only open|close (re-check v2-p1196 M1)
        hint = [ln for ln in self.lines if ln.startswith("BALANCE unknown")][0]
        self.assertTrue(hint.endswith(f"run `scripts/pipeline.py wave start {WAVE} --rev <sha>`"),
                        hint)
        self.assertNotIn("strand.sh wave start", hint)
        self.assertFalse(any(c[:3] == ["gh", "issue", "list"] for c in gh.calls), gh.calls)
        # `wave open` is a start as well: the commit that wrote the Running row
        readme = self.root / "plans" / "README.md"
        readme.write_text(self.LIVE % ("| `%s/` | **Running since 2026-10-08:** X. |\n" % WAVE),
                          encoding="utf-8")
        env = {"GIT_COMMITTER_DATE": "2026-10-08T07:30:00+02:00"}
        with mock.patch.dict(os.environ, env):
            _git(self.root, "add", "plans/README.md")
            _git(self.root, "commit", "-q", "-m", "%s: plans/README.md § Live" % WAVE)
        self.write_map(_git(self.root, "rev-parse", "HEAD"), "GREEN")
        self.lines = []
        rc = wave.close(WAVE, root=self.root, db_path=db_path, runner=gh,
                        out=self.lines.append, now="2026-10-09T00:00:00Z", dry_run=True)
        self.assertEqual(0, rc, self.lines)
        self.assertIn("since=2026-10-08T05:30:00Z (wave open)", self.balance_line())
        # both known: the earlier one opens the window
        self.assertEqual(0, self.close(gh), self.lines)
        self.assertIn("since=2026-10-08T05:30:00Z (wave open)", self.balance_line())

    def test_wave_close_window_opens_at_the_start_rev_not_at_a_late_wave_start(self):
        # Re-check v2-p1196 I1: `wave start --rev <old>` recorded late (the remedy of the
        # fail-closed refusal) must not move the window to "now" -- the start rev's commit
        # time opens it, read through the injected runner.
        db_path = self.root / "late.db"
        self.start_wave("2026-10-09T00:00:00Z", db_path=db_path)
        self.found(20)
        gh = FakeGh(self.CLOSED_3, created={20: ("OPEN", "2026-10-08T09:00:00Z", [])})
        self.lines = []
        rc = wave.close(WAVE, root=self.root, db_path=db_path, runner=gh,
                        out=self.lines.append, now="2026-10-09T00:00:00Z", dry_run=True)
        self.assertEqual(1, rc, self.lines)
        self.assertEqual("BALANCE opened=1 closed=0 excused=0 balance=1 "
                         "since=2026-10-08T06:00:00Z (wave start)", self.balance_line())
        self.assertIn(["git", "-C", str(self.root), "show", "-s", "--format=%cI",
                       self.tip + "^{commit}"], gh.calls)

    def test_wave_close_refuses_when_the_start_rev_is_not_readable(self):
        # The journal time is no fallback: it is the late moment the window must not take.
        db_path = self.root / "gone.db"
        from scripts.pipeline import events
        db = store.open(db_path)
        events.emit(db, "wave_started", "status", {"wave": WAVE, "start": "0" * 40},
                    at="2026-10-08T06:00:00Z")
        store.close(db)
        gh = FakeGh(self.CLOSED_3)
        self.lines = []
        rc = wave.close(WAVE, root=self.root, db_path=db_path, runner=gh,
                        out=self.lines.append, now="2026-10-09T00:00:00Z", dry_run=True)
        self.assertEqual(1, rc, self.lines)
        self.assertTrue(any(ln.startswith("BALANCE unknown: the start rev 000000000")
                            for ln in self.lines), self.lines)
        self.assertFalse(any(c[:3] == ["gh", "issue", "list"] for c in gh.calls), gh.calls)

    def test_wave_close_window_of_a_rebased_wave_open_commit_is_its_author_time(self):
        # `wave open` commits on the current branch; a rebase before the merge moves
        # only the committer time (later). The author time is the moment of the act.
        db_path = self.root / "open-only.db"
        store.close(store.open(db_path))
        readme = self.root / "plans" / "README.md"
        readme.write_text(self.LIVE % ("| `%s/` | **Running since 2026-10-08:** X. |\n" % WAVE),
                          encoding="utf-8")
        env = {"GIT_AUTHOR_DATE": "2026-10-08T07:30:00+02:00",
               "GIT_COMMITTER_DATE": "2026-10-09T12:00:00Z"}
        with mock.patch.dict(os.environ, env):
            _git(self.root, "add", "plans/README.md")
            _git(self.root, "commit", "-q", "-m", "%s: plans/README.md § Live" % WAVE)
        self.write_map(_git(self.root, "rev-parse", "HEAD"), "GREEN")
        self.found(20)
        gh = FakeGh(self.CLOSED_3, created={20: ("OPEN", "2026-10-08T09:00:00Z", [])})
        self.lines = []
        rc = wave.close(WAVE, root=self.root, db_path=db_path, runner=gh,
                        out=self.lines.append, now="2026-10-09T00:00:00Z", dry_run=True)
        self.assertEqual(1, rc, self.lines)
        self.assertEqual("BALANCE opened=1 closed=0 excused=0 balance=1 "
                         "since=2026-10-08T05:30:00Z (wave open)", self.balance_line())

    def test_wave_close_refuses_when_github_cannot_list(self):
        gh = FakeGh(self.CLOSED_3, list_rc=1)
        self.assertEqual(1, self.close(gh), self.lines)
        self.assertTrue(any(ln.startswith("BALANCE unknown: gh issue list exit 1")
                            for ln in self.lines), self.lines)


class StrandKit(unittest.TestCase):

    def test_strand_sh_wave_close_hands_over_to_the_pipeline(self):
        with tempfile.TemporaryDirectory() as tmp:
            kit = pathlib.Path(tmp) / "scripts"
            kit.mkdir()
            (kit / "strand.sh").write_text((REPO / "scripts" / "strand.sh").read_text())
            kit_tree.seed(kit, "strand.sh")
            (kit / "pipeline.py").write_text("import sys\nprint(' '.join(sys.argv[1:]))\n")
            res = subprocess.run(["bash", str(kit / "strand.sh"), "wave", "close", WAVE, "--dry-run"],
                                 capture_output=True, text=True, cwd=tmp)
            self.assertEqual((0, f"wave close {WAVE} --dry-run"), (res.returncode, res.stdout.strip()))
            res = subprocess.run(["bash", str(kit / "strand.sh"), "wave", "open"],
                                 capture_output=True, text=True, cwd=tmp)
            self.assertEqual(2, res.returncode)


if __name__ == "__main__":
    unittest.main()
