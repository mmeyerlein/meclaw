"""`scripts/release.py` step `roadmap`: every release writes its own line under
`ROADMAP.md` § Shipped (GH #1518).

`docs/development-rules.md` § 1 and § 5b ask every release to bring the roadmap
in line between the CHANGELOG cut and the export, and to run
`scripts/check_roadmap_anchors.py` green. Up to 0.86.1 that step was a sentence
in a prompt, and § Shipped stopped at v0.77.0 while fifteen minor and patch
releases shipped after it. The step now runs in `release.sh prepare`: it takes
the first sentence of the version's CHANGELOG intro, writes it as one line in
the form of the lines already there (newest on top), commits it on the cut in
the release worktree and runs the anchor check. It stands before the release
gate so the gate measures the line; a line under § Shipped and nothing else is
a release record (`turnaround.release_offending`), so a battery measured on the
cut still matches the release head.

The integration cases reuse the throw-away world of `test_release_script.py`
(stubs for every outside tool, no network) and add a ROADMAP.md and the real
anchor check to its cut.

Run: `python3 -m unittest scripts.tests.test_release_roadmap`.
"""
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

from scripts.tests import store_isolation
from scripts.tests.test_release_script import STRAND, VERSION, WDIR, World, git

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
REPO = SCRIPTS.parent
sys.path.insert(0, str(SCRIPTS))
import release  # noqa: E402
from pipeline import turnaround  # noqa: E402

CHECK = SCRIPTS / "check_roadmap_anchors.py"

INTRO = ("A new thing ships, and with it a sentence long enough to wrap: the step takes the first "
         "sentence of the intro verbatim, drops the issue link "
         "([#7](https://github.com/o/r/issues/7)) and keeps `code (with. dots)` whole. "
         "A second sentence stays in the CHANGELOG.")
CHANGELOG = ("# Changelog\n\n## [Unreleased]\n\n## [0.0.2] — 2026-11-01\n\n" + INTRO + "\n\n"
             "### Fixed\n\n- a fix ([#7](https://github.com/o/r/issues/7))\n\n"
             "## [0.0.1] — 2026-10-01\n\nThe first.\n")
# What the step writes for INTRO: the first sentence, the link group gone, the
# first word lower-case as in every line above v0.77.0, wrapped like them.
LINE = ("- v0.0.2: a new thing ships, and with it a sentence long enough to wrap: the step takes "
        "the first sentence of the\n"
        "  intro verbatim, drops the issue link and keeps `code (with. dots)` whole.")

ROADMAP = """# Roadmap

A preamble above the first heading is not read.

## Now

## Next

- Something deliberately not built. *(register: thing)*

## Shipped

One line per release.

- v0.0.1: the first release.

- v0.0.x: the old group.
"""


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


def shipped_bullets(text):
    """The `- v...` lines of § Shipped, in file order."""
    section = text.split("## Shipped", 1)[1]
    return [line for line in section.splitlines() if line.startswith("- v")]


class Case(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = pathlib.Path(self._tmp.name)
        ident = mock.patch.dict(os.environ, {
            "GIT_AUTHOR_NAME": "kit", "GIT_AUTHOR_EMAIL": "kit@example.invalid",
            "GIT_COMMITTER_NAME": "kit", "GIT_COMMITTER_EMAIL": "kit@example.invalid"})
        ident.start()
        self.addCleanup(ident.stop)

    def tearDown(self):
        self._tmp.cleanup()

    def world(self, roadmap=ROADMAP, changelog=CHANGELOG):
        """The release world with a roadmap and the real anchor check on its cut."""
        w = World(self.tmp)
        # the anchor check never asks the tracker here: no network in a kit test
        w.env["MECLAW_ROADMAP_GATE_OFFLINE"] = "1"
        (w.root / "scripts").mkdir(exist_ok=True)
        shutil.copy(CHECK, w.root / "scripts" / "check_roadmap_anchors.py")
        (w.root / "ROADMAP.md").write_text(roadmap)
        (w.root / "CHANGELOG.md").write_text(changelog)
        git(w.root, "add", "-A")
        git(w.root, "commit", "-q", "-m", "Release 0.0.2 with a roadmap")
        return w

    def data(self, w):
        return release.State(w.root / "plans" / WDIR / f"release-{VERSION}.state").data()


class Prepare(Case):
    def test_a_release_writes_exactly_one_line_at_the_top_of_shipped(self):
        w = self.world()
        cut = git(w.root, "rev-parse", "master")
        rc, out = w.run("prepare")
        self.assertEqual(rc, 0, out)
        wt = pathlib.Path(self.data(w)["worktree"])
        text = (wt / "ROADMAP.md").read_text()
        self.assertIn("\n\n" + LINE + "\n- v0.0.1: the first release.\n", text)
        self.assertEqual(shipped_bullets(text), ["- v0.0.2: a new thing ships, and with it a sentence "
                                                 "long enough to wrap: the step takes the first "
                                                 "sentence of the",
                                                 "- v0.0.1: the first release.",
                                                 "- v0.0.x: the old group."])
        self.assertEqual(text.count("- v0.0.2:"), 1)
        # nothing else of the file moved
        self.assertEqual(text.replace(LINE + "\n", ""), ROADMAP)
        # cut -> roadmap commit -> test-count commit; the roadmap commit carries ROADMAP.md alone
        self.assertEqual(git(wt, "rev-parse", "HEAD~2"), cut)
        self.assertEqual(git(wt, "diff", "--name-only", "HEAD~2", "HEAD~1"), "ROADMAP.md")
        # the commit message travels with the public tree: English (review M3)
        self.assertTrue(git(wt, "log", "-1", "--format=%s", "HEAD~1").endswith(
            ": ROADMAP § Shipped line for v0.0.2 from its CHANGELOG section"))

    def test_the_step_stands_after_the_cut_and_before_the_gate_and_the_export(self):
        steps = list(release.PREPARE)
        self.assertIn("roadmap", steps)
        self.assertLess(steps.index("worktree"), steps.index("roadmap"))
        self.assertLess(steps.index("roadmap"), steps.index("gate"))
        self.assertLess(steps.index("roadmap"), steps.index("export"))

    def test_the_release_receipt_and_the_battery_on_the_cut_cover_the_line(self):
        # The gate measures the line (receipt on the roadmap commit), and a
        # line under § Shipped alone is a release record
        # (`turnaround.release_offending`): a battery measured on the cut still
        # matches the release head (`release_matches_battery.py`).
        w = self.world()
        cut = git(w.root, "rev-parse", "master")
        rc, out = w.run("prepare")
        self.assertEqual(rc, 0, out)
        wt = pathlib.Path(self.data(w)["worktree"])
        roadmap_commit = git(wt, "rev-parse", "HEAD~1")
        receipt = w.root / "plans" / WDIR / "receipts" / STRAND / "latest" / "receipt.json"
        self.assertIn(roadmap_commit, receipt.read_text())
        self.assertEqual(turnaround.release_offending(str(w.root), roadmap_commit,
                                                      self.data(w)["release_head"]), {})
        self.assertEqual(turnaround.release_offending(str(w.root), cut, roadmap_commit), {})
        self.assertEqual(turnaround.release_offending(str(w.root), cut,
                                                      self.data(w)["release_head"]), {})

    def test_a_second_run_for_the_same_version_adds_nothing(self):
        w = self.world()
        rc, out = w.run("prepare")
        self.assertEqual(rc, 0, out)
        wt = pathlib.Path(self.data(w)["worktree"])
        head, text = git(wt, "rev-parse", "HEAD"), (wt / "ROADMAP.md").read_text()
        args = release.parse(["prepare", VERSION, "--wave", "w", "--strand", STRAND,
                              "--repo", str(w.root), "--config", str(w.config)])
        with mock.patch.dict(os.environ, w.env, clear=True):
            got = release.Release(args, {}).step_roadmap()
        self.assertEqual(got.get("roadmap"), "present")
        self.assertEqual(git(wt, "rev-parse", "HEAD"), head)
        self.assertEqual((wt / "ROADMAP.md").read_text(), text)

    def test_a_cut_that_already_names_the_version_gets_no_second_line(self):
        hand = ROADMAP.replace("- v0.0.1:", "- v0.0.2: written by hand.\n- v0.0.1:")
        w = self.world(roadmap=hand)
        cut = git(w.root, "rev-parse", "master")
        rc, out = w.run("prepare")
        self.assertEqual(rc, 0, out)
        wt = pathlib.Path(self.data(w)["worktree"])
        self.assertEqual((wt / "ROADMAP.md").read_text(), hand)
        self.assertEqual(git(wt, "rev-parse", "HEAD~1"), cut)

    def test_a_version_bullet_outside_shipped_does_not_count_as_the_line(self):
        # Review M5: only § Shipped says whether the release has its line.
        text = ROADMAP.replace("A preamble above the first heading is not read.\n",
                               "A preamble above the first heading is not read.\n\n"
                               "- v0.0.2: named in the preamble.\n")
        w = self.world(roadmap=text)
        rc, out = w.run("prepare")
        self.assertEqual(rc, 0, out)
        wt = pathlib.Path(self.data(w)["worktree"])
        got = (wt / "ROADMAP.md").read_text()
        self.assertEqual(shipped_bullets(got)[0], LINE.split("\n")[0])
        self.assertEqual(got.replace(LINE + "\n", ""), text)

    def test_the_anchor_check_stays_green_after_the_step(self):
        w = self.world()
        rc, out = w.run("prepare")
        self.assertEqual(rc, 0, out)
        self.assertIn("roadmap anchors OK", out)
        wt = pathlib.Path(self.data(w)["worktree"])
        proc = subprocess.run([sys.executable, "-I", str(wt / "scripts" / "check_roadmap_anchors.py")],
                              capture_output=True, text=True, env=w.env)
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)

    def test_a_red_anchor_check_stops_the_release_before_the_gate(self):
        stray = ROADMAP.replace("## Next\n\n", "## Next\n\nA paragraph that orders nothing.\n\n")
        w = self.world(roadmap=stray)
        rc, out = w.run("prepare")
        self.assertEqual(rc, 1, out)
        self.assertIn("release: roadmap RED", out)
        self.assertNotIn("strand.sh", w.names())
        ends = [(e["step"], e["verdict"]) for e in w.state() if e["event"] == "end"]
        self.assertEqual(ends[-1], ("roadmap", "RED"))

    def test_a_section_without_an_intro_is_red_and_writes_nothing(self):
        bare = CHANGELOG.replace(INTRO + "\n\n", "")
        w = self.world(changelog=bare)
        cut = git(w.root, "rev-parse", "master")
        rc, out = w.run("prepare")
        self.assertEqual(rc, 1, out)
        self.assertIn("no intro paragraph", out)
        wt = pathlib.Path(self.data(w)["worktree"])
        self.assertEqual(git(wt, "rev-parse", "HEAD"), cut)
        self.assertEqual((wt / "ROADMAP.md").read_text(), ROADMAP)


class Line(unittest.TestCase):
    def test_the_line_is_the_first_sentence_in_the_form_of_the_shipped_lines(self):
        section, _ = release.changelog_section(CHANGELOG, "0.0.2")
        self.assertEqual(release.shipped_line(section, "0.0.2"), LINE)

    def test_every_line_stays_inside_the_width_of_the_shipped_lines(self):
        text = (REPO / "ROADMAP.md").read_text()
        widest = max(len(line) for line in text.split("## Shipped", 1)[1].splitlines())
        long_intro = "The " + " ".join(["word"] * 120) + ". Rest."
        line = release.shipped_line(long_intro + "\n", "1.2.3")
        self.assertTrue(line.startswith("- v1.2.3: the word word"), line)
        for row in line.splitlines():
            self.assertLessEqual(len(row), widest)
        for row in line.splitlines()[1:]:
            self.assertTrue(row.startswith("  ") and not row.startswith("   "), row)

    def test_an_identifier_or_acronym_keeps_its_case(self):
        self.assertTrue(release.shipped_line("`curator@2` moves. More.\n", "1.0.0")
                        .startswith("- v1.0.0: `curator@2` moves."))
        self.assertTrue(release.shipped_line("LLM rows move. More.\n", "1.0.0")
                        .startswith("- v1.0.0: LLM rows move."))

    def test_an_issue_link_inside_prose_keeps_its_number(self):
        line = release.shipped_line(
            "A fix ([#1382](https://github.com/o/r/issues/1382), a regression of 0.77.0). Rest.\n", "1.0.0")
        self.assertEqual(line, "- v1.0.0: a fix (#1382, a regression of 0.77.0).")


class Insert(unittest.TestCase):
    def test_the_line_goes_above_the_newest_older_release(self):
        text = ROADMAP.replace("- v0.0.1:", "- v0.0.3: a later one.\n- v0.0.1:")
        got = release.with_shipped_line(text, "0.0.2", "- v0.0.2: between.")
        self.assertIn("- v0.0.3: a later one.\n- v0.0.2: between.\n- v0.0.1:", got)

    def test_a_version_already_there_leaves_the_text_byte_equal(self):
        once = release.with_shipped_line(ROADMAP, "0.0.2", "- v0.0.2: once.")
        self.assertEqual(release.with_shipped_line(once, "0.0.2", "- v0.0.2: twice."), once)

    def test_a_roadmap_without_shipped_is_red(self):
        with self.assertRaisesRegex(release.Red, "no `## Shipped`"):
            release.with_shipped_line("# Roadmap\n\n## Now\n", "0.0.2", "- v0.0.2: x.")

    def test_the_real_roadmap_takes_a_new_release_and_stays_green(self):
        text = (REPO / "ROADMAP.md").read_text()
        got = release.with_shipped_line(text, "99.0.0", "- v99.0.0: a release far ahead.")
        self.assertEqual(shipped_bullets(got)[0], "- v99.0.0: a release far ahead.")
        self.assertEqual(got.replace("- v99.0.0: a release far ahead.\n", "", 1), text)
        with tempfile.TemporaryDirectory() as tmp:
            tree = pathlib.Path(tmp)
            (tree / "scripts").mkdir()
            (tree / "docs").mkdir()
            shutil.copy(CHECK, tree / "scripts" / "check_roadmap_anchors.py")
            register = REPO / "docs" / "defer-register.md"
            if register.is_file():
                shutil.copy(register, tree / "docs" / "defer-register.md")
            (tree / "ROADMAP.md").write_text(got)
            proc = subprocess.run([sys.executable, "-I", str(tree / "scripts" / "check_roadmap_anchors.py"),
                                   "--offline"], capture_output=True, text=True)
            self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
            self.assertIn("roadmap anchors OK", proc.stdout)


if __name__ == "__main__":
    unittest.main()
