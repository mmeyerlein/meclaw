"""Tests for `scripts/wave_receipt.py` -- the strand table of a wave receipt.

The receipt of a wave repeated what the reports already said: of the 2 640
eight-grams in one receipt only two were new text, while 25 of its 34 SHAs and
10 of its 14 issue numbers stood in a report as well
(`befund/04-struktur.md` section 9.4). So the table is GENERATED from the
header blocks of the reports and replaced between two markers -- never a line
by hand.
"""

import os
import pathlib
import re
import shutil
import subprocess
import tempfile
import unittest

REPO = pathlib.Path(__file__).resolve().parents[2]
SCRIPT = REPO / "scripts" / "wave_receipt.py"

WAVE_DIR = "welle-x-2026-09-19"

GIT_ENV = {
    "GIT_AUTHOR_NAME": "receipt test",
    "GIT_AUTHOR_EMAIL": "receipt@example.invalid",
    "GIT_COMMITTER_NAME": "receipt test",
    "GIT_COMMITTER_EMAIL": "receipt@example.invalid",
    "GIT_CONFIG_GLOBAL": os.devnull,
    "GIT_CONFIG_SYSTEM": os.devnull,
}

HEAD = """---
strang: %s
branch: %s
issues: %s
basis: a85bc777
gate: "%s"
commits: [%s]
---

# Strang %s

## Änderungen

- irgendetwas
"""

RECEIPT = """# Welle X -- Receipt

Prosa davor.

## 3. Stränge

<!-- strands:begin -->
veraltet, von Hand
<!-- strands:end -->

## 4. Danach

Prosa danach.
"""


def _git(repo, *args):
    env = dict(os.environ)
    env.update(GIT_ENV)
    return subprocess.run(["git", "-C", str(repo)] + list(args),
                          env=env, check=True, capture_output=True, text=True)


class WaveReceiptTestCase(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.repo = pathlib.Path(self._tmp.name) / "repo"
        (self.repo / "scripts").mkdir(parents=True)
        shutil.copy(SCRIPT, self.repo / "scripts" / SCRIPT.name)
        (self.repo / "scripts" / SCRIPT.name).chmod(0o755)
        self.wave = self.repo / "plans" / WAVE_DIR
        (self.wave / "berichte").mkdir(parents=True)
        (self.repo / "README.md").write_text("x\n")
        _git(self.repo, "init", "-q")
        _git(self.repo, "add", "scripts", "README.md")
        _git(self.repo, "commit", "-q", "-m", "first")
        _git(self.repo, "branch", "-M", "master")

    def report(self, name, issues, gate, commits, branch=None):
        (self.wave / "berichte" / ("%s.md" % name)).write_text(
            HEAD % (name, branch or "welle-x/%s" % name, issues, gate,
                    commits, name))

    def receipt(self, text=RECEIPT):
        path = self.wave / "receipt.md"
        path.write_text(text)
        return path

    def run_tool(self, *args):
        env = dict(os.environ)
        env.update(GIT_ENV)
        return subprocess.run(
            ["python3", str(self.repo / "scripts" / SCRIPT.name), WAVE_DIR]
            + list(args),
            cwd=str(self.repo), env=env, capture_output=True, text=True)


class TestTable(WaveReceiptTestCase):
    def test_two_reports_become_two_rows_between_the_markers(self):
        self.report("kit", "[#756, #755]",
                    "GATE-SUMMARY strand aaaaaaa 7/7 12s GREEN", "fb22b543")
        self.report("gate", "[#757]",
                    "GATE-SUMMARY strand bbbbbbb 9/9 30s GREEN",
                    "c790000d, 1bf0ad51")
        path = self.receipt()
        res = self.run_tool()
        self.assertEqual(0, res.returncode, res.stderr)
        text = path.read_text()
        self.assertIn("Prosa davor.", text)
        self.assertIn("Prosa danach.", text)
        self.assertNotIn("veraltet, von Hand", text)
        body = re.search(r"<!-- strands:begin -->\n(.*?)\n<!-- strands:end -->",
                         text, re.S).group(1)
        self.assertIn("| Strang | Issues | Gate | Commits | Merge |", body)
        rows = [ln for ln in body.splitlines() if ln.startswith("| ")
                and "---" not in ln and "Strang |" not in ln]
        self.assertEqual(2, len(rows), body)
        self.assertTrue(any("kit" in r and "#756, #755" in r
                            and "7/7" in r and "fb22b543" in r for r in rows),
                        body)
        self.assertTrue(any("gate" in r and "1bf0ad51" in r for r in rows),
                        body)

    def test_replacement_is_idempotent(self):
        self.report("kit", "[#756]",
                    "GATE-SUMMARY strand aaaaaaa 7/7 12s GREEN", "fb22b543")
        path = self.receipt()
        self.assertEqual(0, self.run_tool().returncode)
        once = path.read_text()
        self.assertEqual(0, self.run_tool().returncode)
        self.assertEqual(once, path.read_text())

    def test_a_receipt_without_markers_gets_the_section_appended(self):
        self.report("kit", "[#756]",
                    "GATE-SUMMARY strand aaaaaaa 7/7 12s GREEN", "fb22b543")
        path = self.receipt("# Welle X -- Receipt\n\nNur Prosa.\n")
        self.assertEqual(0, self.run_tool().returncode, )
        text = path.read_text()
        self.assertIn("Nur Prosa.", text)
        self.assertIn("<!-- strands:begin -->", text)
        self.assertIn("<!-- strands:end -->", text)
        self.assertEqual(0, self.run_tool().returncode)
        self.assertEqual(text, path.read_text())

    def test_the_merge_column_comes_from_the_merge_commits(self):
        self.report("kit", "[#756]",
                    "GATE-SUMMARY strand aaaaaaa 7/7 12s GREEN", "fb22b543")
        _git(self.repo, "checkout", "-q", "-b", "welle-x/kit")
        (self.repo / "kit.txt").write_text("k\n")
        _git(self.repo, "add", "kit.txt")
        _git(self.repo, "commit", "-q", "-m", "kit")
        _git(self.repo, "checkout", "-q", "master")
        _git(self.repo, "merge", "--no-ff", "-q", "welle-x/kit",
             "-m", "Merge branch 'welle-x/kit'")
        merge = _git(self.repo, "rev-parse", "--short=8", "HEAD").stdout.strip()
        path = self.receipt()
        self.assertEqual(0, self.run_tool().returncode)
        self.assertIn(merge, path.read_text())

    def test_a_longer_strand_name_does_not_take_the_shorter_ones_merge(self):
        """`welle-x/g1` is a substring of `welle-x/g12` -- wave G had thirteen
        strands and `g1` was credited with `g12`'s merge commit."""
        self.report("g1", "[#766]",
                    "GATE-SUMMARY strand aaaaaaa 7/7 12s GREEN", "fb22b543")
        self.report("g12", "[#766]",
                    "GATE-SUMMARY strand bbbbbbb 7/7 12s GREEN", "c790000d")
        _git(self.repo, "checkout", "-q", "-b", "welle-x/g12")
        (self.repo / "g12.txt").write_text("g\n")
        _git(self.repo, "add", "g12.txt")
        _git(self.repo, "commit", "-q", "-m", "g12")
        _git(self.repo, "checkout", "-q", "master")
        _git(self.repo, "merge", "--no-ff", "-q", "welle-x/g12",
             "-m", "welle-x: Merge welle-x/g12 -- the cap (#766)")
        merge = _git(self.repo, "rev-parse", "--short=8", "HEAD").stdout.strip()
        path = self.receipt()
        self.assertEqual(0, self.run_tool().returncode)
        rows = {r.split("|")[1].strip(): r
                for r in path.read_text().splitlines() if r.startswith("| g")}
        self.assertIn(merge, rows["g12"])
        self.assertNotIn(merge, rows["g1"])
        self.assertTrue(rows["g1"].rstrip().endswith("| -- |"), rows["g1"])

    def test_a_gate_line_with_pipes_does_not_break_the_row(self):
        """A repo without cargo reports several suites in one gate line; an
        unescaped pipe there turns five columns into nine."""
        self.report("app", "[#16]",
                    "test_app.sh 700 | test_ui.sh 1 failure | install ok",
                    "ad0effb")
        path = self.receipt()
        self.assertEqual(0, self.run_tool().returncode)
        row = [r for r in path.read_text().splitlines()
               if r.startswith("| app ")][0]
        cells = re.split(r"(?<!\\)\|", row.strip().strip("|"))
        self.assertEqual(5, len(cells), row)
        self.assertIn("test_ui.sh", row)

    def test_a_report_without_a_header_is_reported_and_skipped(self):
        self.report("kit", "[#756]",
                    "GATE-SUMMARY strand aaaaaaa 7/7 12s GREEN", "fb22b543")
        (self.wave / "berichte" / "lose.md").write_text("# kein Kopfblock\n")
        path = self.receipt()
        res = self.run_tool()
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertIn("lose.md", res.stderr)
        self.assertNotIn("lose", path.read_text())


if __name__ == "__main__":
    unittest.main()
