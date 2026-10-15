"""`plans/README.md` section Live knows the running wave (welle-validierung-2 3b, GH #1199).

Section Live was an archive in order: its top row was the wave built LAST, the
row of the running wave came only at its close. In the main tree every kit call
then needed `--wave`. `pipeline wave open <dir> --title <sentence>` writes the
row `**Running since <date>:**` on top and commits it; `wave close` turns it
into `**Built <date>:**` (the orchestrator describes the result afterwards) and
writes a new Built row for a wave opened before this command existed.
"""

import os
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))

from scripts.pipeline import wave  # noqa: E402
from scripts.tests import store_isolation  # noqa: E402


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()


def tearDownModule():  # noqa: N802 -- unittest's name
    store_isolation.restore()


WAVE = "welle-z-2026-10-14"
OLD = "welle-y-2026-10-10"

README = """# Plans

Intro.

## Live

| Path | What it is |
|---|---|
| `%s/` | **Built 2026-10-12:** Wave Y, done. |

## The archive rule

Text.
""" % OLD

GIT_ENV = {
    "GIT_AUTHOR_NAME": "wave test",
    "GIT_AUTHOR_EMAIL": "wave@example.invalid",
    "GIT_COMMITTER_NAME": "wave test",
    "GIT_COMMITTER_EMAIL": "wave@example.invalid",
    "GIT_CONFIG_GLOBAL": os.devnull,
    "GIT_CONFIG_SYSTEM": os.devnull,
}


def _git(root, *args):
    return subprocess.run(["git", "-C", str(root), *args], capture_output=True, text=True,
                          check=True, env={**os.environ, **GIT_ENV}).stdout.strip()


def live_lines(text):
    return [ln for ln in text.splitlines() if ln.startswith("| `")]


class LiveRowsTest(unittest.TestCase):
    def test_open_writes_the_running_row_on_top_of_section_live(self):
        out = wave.live_open(README, WAVE, "Wave Z, the test wave.", "2026-10-14")
        self.assertEqual(
            ["| `%s/` | **Running since 2026-10-14:** Wave Z, the test wave. |" % WAVE,
             "| `%s/` | **Built 2026-10-12:** Wave Y, done. |" % OLD],
            live_lines(out))
        self.assertIn("## The archive rule\n\nText.\n", out)

    def test_open_refuses_a_wave_the_table_names(self):
        once = wave.live_open(README, WAVE, "Wave Z.", "2026-10-14")
        with self.assertRaisesRegex(ValueError, "already"):
            wave.live_open(once, WAVE, "Wave Z.", "2026-10-15")
        with self.assertRaisesRegex(ValueError, "already"):
            wave.live_open(README, OLD, "Wave Y again.", "2026-10-15")

    def test_open_refuses_a_title_that_breaks_the_table(self):
        for title in ("", "  ", "a | b", "two\nlines"):
            with self.assertRaisesRegex(ValueError, "one non-empty line", msg=repr(title)):
                wave.live_open(README, WAVE, title, "2026-10-14")

    def test_running_names_every_running_row(self):
        one = wave.live_open(README, WAVE, "Wave Z.", "2026-10-14")
        self.assertEqual([WAVE], wave.live_running(one))
        two = wave.live_open(one, "welle-q-2026-10-14", "Wave Q.", "2026-10-14")
        self.assertEqual(["welle-q-2026-10-14", WAVE], wave.live_running(two))
        self.assertEqual([], wave.live_running(README))

    def test_close_turns_the_running_row_built(self):
        text = wave.live_open(README, WAVE, "Wave Z, the test wave.", "2026-10-14")
        out, what = wave.live_close(text, WAVE, "2026-10-20")
        self.assertEqual("built", what)
        self.assertEqual("| `%s/` | **Built 2026-10-20:** Wave Z, the test wave. |" % WAVE,
                         live_lines(out)[0])
        self.assertEqual([], wave.live_running(out))

    def test_close_without_a_running_row_writes_a_new_built_row(self):
        out, what = wave.live_close(README, WAVE, "2026-10-20")
        self.assertEqual("new", what)
        rows = live_lines(out)
        self.assertEqual(2, len(rows), rows)
        self.assertTrue(rows[0].startswith("| `%s/` | **Built 2026-10-20:** " % WAVE), rows)

    def test_close_leaves_a_built_row_as_it_is(self):
        out, what = wave.live_close(README, OLD, "2026-10-20")
        self.assertEqual("kept", what)
        self.assertEqual(README, out)


class OpenCommandTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.tmp.name)
        (self.root / "plans" / WAVE).mkdir(parents=True)
        (self.root / "plans" / "README.md").write_text(README, encoding="utf-8")
        _git(self.root, "init", "-q", "-b", "master")
        _git(self.root, "add", "plans")
        _git(self.root, "commit", "-q", "-m", "first")
        self.lines = []
        self.env = mock.patch.dict(os.environ, GIT_ENV)
        self.env.start()

    def tearDown(self):
        self.env.stop()
        self.tmp.cleanup()

    def test_open_commits_the_row_on_the_current_branch(self):
        rc = wave.open_wave(WAVE, "Wave Z.", root=self.root, out=self.lines.append,
                            today="2026-10-14")
        self.assertEqual(0, rc, self.lines)
        self.assertEqual("", _git(self.root, "status", "--porcelain"))
        self.assertEqual(["plans/README.md"],
                         _git(self.root, "show", "--name-only", "--format=", "HEAD").split())
        self.assertEqual([WAVE], wave.live_running(
            (self.root / "plans" / "README.md").read_text(encoding="utf-8")))
        # c23 M-6: the commit names its wave first, as every wave-level commit
        # does (`welle-live: Zeile in plans/README.md`); no strand, no issue yet
        self.assertEqual("%s: plans/README.md § Live, läuft seit 2026-10-14" % WAVE,
                         _git(self.root, "log", "-1", "--format=%s"))

    def test_open_refuses_a_wave_without_its_directory(self):
        rc = wave.open_wave("welle-none-2026-10-14", "Nope.", root=self.root,
                            out=self.lines.append, today="2026-10-14")
        self.assertEqual(2, rc)
        self.assertEqual("first", _git(self.root, "log", "-1", "--format=%s"))

    def test_open_refuses_a_readme_with_changes_of_its_own(self):
        p = self.root / "plans" / "README.md"
        p.write_text(README + "\nscratch\n", encoding="utf-8")
        rc = wave.open_wave(WAVE, "Wave Z.", root=self.root, out=self.lines.append,
                            today="2026-10-14")
        self.assertEqual(2, rc)
        self.assertEqual("first", _git(self.root, "log", "-1", "--format=%s"))


if __name__ == "__main__":
    unittest.main()
