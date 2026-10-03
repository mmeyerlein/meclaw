"""Tests for the source mark of the display template's scenario copies.

`scripts/display_sync.py` keeps the copies under
`templates/display/compose/scenarios/` in step with the living description tree
and writes the mark `SOURCE` beside them: one line, `meclaw-next <sha> <date>`.
The drift lock reads that mark in a strand, so the mark has to be true or
absent -- never a guess. What is pinned here is that contract, against
throw-away git repositories, never against the real description tree.

Measurement behind it: six full strand gates went red on the drift lock in one
evening because three sessions moved the living tree while the gates ran
(`plans/welle-p-2026-09-19/befund/01-zeit.md` section 3.2, 5589 s).
"""

import importlib.util
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest

REPO = pathlib.Path(__file__).resolve().parents[2]
SYNC = REPO / "scripts" / "display_sync.py"

_spec = importlib.util.spec_from_file_location("display_sync", SYNC)
display_sync = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(display_sync)


def git(cwd, *args):
    return subprocess.run(
        ["git", "-C", str(cwd), *args],
        check=True, capture_output=True, text=True,
    ).stdout.strip()


def a_living_tree(root):
    """A throw-away repository shaped like the description tree: one commit
    carrying the three files the template copies."""
    model = root / "23-display" / "model"
    model.mkdir(parents=True)
    (model / "scenarios.json").write_text('{"cases": []}\n', encoding="utf-8")
    (model / "pass.py").write_text("RUNGS = ()\n", encoding="utf-8")
    (model / "run.py").write_text("print('model')\n", encoding="utf-8")
    git(root, "init", "-q", "-b", "main")
    git(root, "config", "user.email", "t@example.invalid")
    git(root, "config", "user.name", "T")
    git(root, "add", "-A")
    git(root, "commit", "-qm", "the model")
    return root / "23-display"


class SourceMark(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        base = pathlib.Path(self.tmp.name)
        self.hive = a_living_tree(base / "hive")
        self.dest = base / "scenarios"
        self.dest.mkdir()
        self.addCleanup(self.tmp.cleanup)

    def test_the_mark_names_the_tree_the_commit_and_its_date(self):
        display_sync.sync_source(self.hive, self.dest)
        mark = (self.dest / "SOURCE").read_text(encoding="utf-8")
        head = git(self.hive, "rev-parse", "HEAD")
        self.assertEqual(mark.count("\n"), 1, "the mark is one line")
        tree, sha, date = mark.split()
        self.assertEqual(tree, "meclaw-next")
        self.assertEqual(sha, head)
        self.assertRegex(date, r"^\d{4}-\d{2}-\d{2}$")

    def test_the_three_files_travel_under_the_names_the_template_uses(self):
        display_sync.sync_source(self.hive, self.dest)
        self.assertEqual(
            (self.dest / "run_model.py").read_text(encoding="utf-8"), "print('model')\n"
        )
        self.assertEqual(
            (self.dest / "scenarios.json").read_text(encoding="utf-8"), '{"cases": []}\n'
        )
        self.assertEqual((self.dest / "pass.py").read_text(encoding="utf-8"), "RUNGS = ()\n")

    def test_the_mark_is_the_commit_that_last_touched_the_model(self):
        """A commit elsewhere in the tree does not move the mark: the lock
        resolves `<sha>:23-display/model/<file>`, so the mark has to name the
        commit those bytes belong to, not the tree's newest one."""
        (self.hive.parent / "README.md").write_text("elsewhere\n", encoding="utf-8")
        git(self.hive, "add", "-A")
        git(self.hive, "commit", "-qm", "something else")
        model_head = git(self.hive, "log", "-1", "--format=%H", "--", "model")
        display_sync.sync_source(self.hive, self.dest)
        self.assertEqual(
            (self.dest / "SOURCE").read_text(encoding="utf-8").split()[1], model_head
        )

    def test_an_uncommitted_model_writes_no_mark_and_copies_nothing(self):
        """The mark promises `git show <sha>` yields these bytes. With the model
        edited but not committed no sha can keep that promise, so the sync
        refuses rather than marking bytes that are not in any commit."""
        (self.hive / "model" / "pass.py").write_text(
            "RUNGS = (1,)\n", encoding="utf-8"
        )
        with self.assertRaises(display_sync.SourceUnclean):
            display_sync.sync_source(self.hive, self.dest)
        self.assertFalse((self.dest / "SOURCE").exists())
        self.assertFalse((self.dest / "pass.py").exists())

    def test_without_the_living_tree_nothing_is_touched(self):
        """A foreign clone has the copies but not their source. It keeps both
        the copies and whatever mark travelled with them."""
        (self.dest / "SOURCE").write_text("meclaw-next deadbeef 2026-09-18\n", encoding="utf-8")
        (self.dest / "pass.py").write_text("the copy\n", encoding="utf-8")
        self.assertIsNone(display_sync.sync_source(pathlib.Path("/nonexistent/hive"), self.dest))
        self.assertEqual(
            (self.dest / "SOURCE").read_text(encoding="utf-8"),
            "meclaw-next deadbeef 2026-09-18\n",
        )
        self.assertEqual((self.dest / "pass.py").read_text(encoding="utf-8"), "the copy\n")


class TheCheapMarkCheck(unittest.TestCase):
    """`display_sync.py --check-source` -- the half of the drift question that
    needs no cargo. It runs in the station `scenarios:display` (I/R/C always)
    and answers "does the mark still describe the living tree?" as a NOTE. The
    verdict stays with the Rust lock, so this never fails: a station that goes
    red on a moving description tree is the defect the strand was built against
    (`plans/welle-p-2026-09-19/befund/01-zeit.md` section 3.2)."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        base = pathlib.Path(self.tmp.name)
        self.hive = a_living_tree(base / "hive")
        self.dest = base / "scenarios"
        self.dest.mkdir()
        self.addCleanup(self.tmp.cleanup)

    def test_copies_that_match_the_mark_are_no_drift(self):
        display_sync.sync_source(self.hive, self.dest)
        self.assertEqual(display_sync.check_source(self.hive, self.dest), [])

    def test_a_changed_copy_is_named_with_the_mark_it_left(self):
        display_sync.sync_source(self.hive, self.dest)
        sha = (self.dest / "SOURCE").read_text(encoding="utf-8").split()[1]
        (self.dest / "pass.py").write_text("RUNGS = (2,)\n", encoding="utf-8")
        drift = display_sync.check_source(self.hive, self.dest)
        self.assertEqual(len(drift), 1)
        self.assertIn("pass.py", drift[0])
        self.assertIn(sha, drift[0])

    def test_a_missing_mark_is_a_finding_not_silence(self):
        """The mark travels with the copies, so its absence is a defect of this
        repository -- not a clone that lags behind."""
        display_sync.sync_source(self.hive, self.dest)
        (self.dest / "SOURCE").unlink()
        drift = display_sync.check_source(self.hive, self.dest)
        self.assertEqual(len(drift), 1)
        self.assertIn("SOURCE", drift[0])

    def test_without_the_living_tree_there_is_nothing_to_compare(self):
        display_sync.sync_source(self.hive, self.dest)
        self.assertEqual(
            display_sync.check_source(pathlib.Path("/nonexistent/hive"), self.dest), [])

    def test_a_tree_that_does_not_know_the_mark_yet_is_not_drift(self):
        """A clone behind the mark has copies from a commit it has not fetched.
        That is a lagging clone, not a description that moved."""
        display_sync.sync_source(self.hive, self.dest)
        (self.dest / "SOURCE").write_text(
            "meclaw-next " + "0" * 40 + " 2026-09-19\n", encoding="utf-8")
        self.assertEqual(display_sync.check_source(self.hive, self.dest), [])

    def test_the_cli_reports_drift_and_still_exits_zero(self):
        display_sync.sync_source(self.hive, self.dest)
        (self.dest / "scenarios.json").write_text('{"cases": [1]}\n', encoding="utf-8")
        done = subprocess.run(
            [sys.executable, str(SYNC), "--check-source", str(self.dest)],
            env={**os.environ, display_sync.HIVE_ENV: str(self.hive)},
            capture_output=True, text=True,
        )
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn("NOTE", done.stdout)
        self.assertIn("scenarios.json", done.stdout)


    def test_an_unknown_argument_syncs_nothing(self):
        """The trap this rule was written against: while `--check-source` was
        unimplemented, calling it fell through to the full sync -- against the
        real template, from a throw-away tree. An argument nobody knows does
        nothing (wave P, fix round 1)."""
        done = subprocess.run(
            [sys.executable, str(SYNC), "--check-sauce"],
            env={**os.environ, display_sync.HIVE_ENV: str(self.hive)},
            capture_output=True, text=True,
        )
        self.assertEqual(done.returncode, 2, done.stdout)
        self.assertIn("usage", done.stderr)


class TheCatalogueCopies(unittest.TestCase):
    """`compose/catalog.json` is the one source of the screen's components (GH #958).

    `display_sync.py` embeds it into `compose.py` as `CATALOG_JSON` and writes a
    block copy into every target of `BLOCK_COPIES`; `--check` is the verdict over
    every copy and exits 1 on drift. Pinned against throw-away copies of the
    template, never by editing the real one."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = pathlib.Path(self.tmp.name) / "compose"
        self.root.mkdir()
        for name in ("compose.py", "display-dna.css", "catalog.json", "config.json"):
            (self.root / name).write_bytes((display_sync.COMPOSE_ROOT / name).read_bytes())
        self.templates = pathlib.Path(self.tmp.name) / "templates"
        (self.templates / "reader").mkdir(parents=True)
        (self.templates / "reader" / "config.json").write_text(
            '{"params": {"other": 1}}\n', encoding="utf-8")
        self.targets = [("reader/config.json", ("params", "catalog"))]

    def catalog(self):
        return json.loads((self.root / "catalog.json").read_text(encoding="utf-8"))

    def test_the_shipped_copies_agree(self):
        done = subprocess.run([sys.executable, str(SYNC), "--check"],
                              capture_output=True, text=True)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)

    def test_the_cli_exits_one_on_drift(self):
        """`--check` over the command line, in a throw-away tree shaped like the
        repository (the script finds its roots from its own path)."""
        tree = pathlib.Path(self.tmp.name) / "repo"
        (tree / "scripts").mkdir(parents=True)
        for name in ("display_sync.py", "display_sheet_strip.py"):
            (tree / "scripts" / name).write_bytes((REPO / "scripts" / name).read_bytes())
        compose = tree / "templates" / "display" / "compose"
        compose.mkdir(parents=True)
        for name in ("compose.py", "display-dna.css", "catalog.json", "config.json"):
            (compose / name).write_bytes((self.root / name).read_bytes())
        # The shipped block-copy targets travel too: `--check` reads every one of them.
        for rel, _keys in display_sync.BLOCK_COPIES:
            (tree / "templates" / rel).parent.mkdir(parents=True, exist_ok=True)
            (tree / "templates" / rel).write_bytes((REPO / "templates" / rel).read_bytes())
        cli = [sys.executable, str(tree / "scripts" / "display_sync.py"), "--check"]
        done = subprocess.run(cli, capture_output=True, text=True)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        cat = self.catalog()
        cat["components"][0]["describe"] = "something else"
        (compose / "catalog.json").write_text(json.dumps(cat), encoding="utf-8")
        done = subprocess.run(cli, capture_output=True, text=True)
        self.assertEqual(done.returncode, 1, done.stdout + done.stderr)
        self.assertIn("DRIFT: compose.py CATALOG_JSON differs from catalog.json", done.stdout)

    def test_an_edited_catalogue_is_drift_and_a_verdict(self):
        cat = self.catalog()
        cat["components"][0]["describe"] = "something else"
        (self.root / "catalog.json").write_text(json.dumps(cat), encoding="utf-8")
        drift = display_sync.check_copies(self.root, targets=[])
        self.assertEqual(len(drift), 1, drift)
        self.assertIn("CATALOG_JSON", drift[0])

    def test_the_embedding_round_trips(self):
        blob = display_sync.catalog_blob((self.root / "catalog.json").read_text(encoding="utf-8"))
        py = display_sync.embed_catalog('A = 1\nCATALOG_JSON = r"""old"""\nB = 2\n', blob)
        self.assertEqual(display_sync.embedded(py, "CATALOG_JSON"), blob)
        self.assertEqual(json.loads(blob), self.catalog())

    def test_the_block_copy_carries_blocks_and_four_fields(self):
        cat = self.catalog()
        blocks = display_sync.block_copy(cat)
        names = [b["name"] for b in blocks]
        self.assertEqual(names, [c["name"] for c in cat["components"] if c.get("block")])
        self.assertIn("display-card", names)
        self.assertNotIn("display-shell", names)
        for block in blocks:
            self.assertEqual(sorted(block), sorted(display_sync.BLOCK_FIELDS), block["name"])

    def test_a_block_target_is_written_and_then_judged(self):
        self.assertEqual(display_sync.check_copies(
            self.root, targets=self.targets, templates=self.templates),
            ["reader/config.json params.catalog differs from the block copy of catalog.json"])
        display_sync.write_block_copies(self.catalog(), self.targets, self.templates)
        cfg = json.loads((self.templates / "reader" / "config.json").read_text(encoding="utf-8"))
        self.assertEqual(cfg["params"]["other"], 1, "the rest of the config is kept")
        self.assertEqual(display_sync.check_copies(
            self.root, targets=self.targets, templates=self.templates), [])

    def test_a_drifted_script_copy_is_named(self):
        cfg = json.loads((self.root / "config.json").read_text(encoding="utf-8"))
        cfg["params"]["script_inline"] += "# hand edit\n"
        (self.root / "config.json").write_text(json.dumps(cfg), encoding="utf-8")
        self.assertEqual(display_sync.check_copies(self.root, targets=[]),
                         ["config.json script_inline differs from compose.py"])

    def test_every_block_target_merges_with_the_driver(self):
        """A block-copy target is a code cell with a one-line script of its own, so
        the merge driver has to be named for it the way it is for the display."""
        attrs = (REPO / ".gitattributes").read_text(encoding="utf-8").splitlines()
        for rel, _keys in display_sync.BLOCK_COPIES + [("display/compose/config.json", ())]:
            self.assertIn("templates/%s merge=display-sync" % rel, attrs, rel)


if __name__ == "__main__":
    unittest.main()
