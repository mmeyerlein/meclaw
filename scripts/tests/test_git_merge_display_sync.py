#!/usr/bin/env python3
"""Cases for the `display-sync` git merge driver.

`templates/display/compose/config.json` carries the whole of `compose.py` as a
one-line `params.script_inline` string. Two branches that touch `compose.py` in
two different places merge cleanly there and collide in `config.json` as a single
line nobody can resolve by hand -- it happened in four of five master→strand
re-merges in one day (welle-p befund 03 § 3.9). The driver merges the readable
fields three-way like any other file and writes `script_inline` back out of the
merged `compose.py`, which is the rule `display_sync.py` already owns.

Each case builds a throwaway git repository holding only the files the driver
touches, so nothing here depends on the current state of the template.
"""
import json
import os
import pathlib
import shutil
import subprocess
import tempfile
import unittest

SCRIPTS = pathlib.Path(__file__).resolve().parents[1]
DRIVER = "scripts/git_merge_display_sync.py"
COMPOSE = "templates/display/compose/compose.py"
CONFIG = "templates/display/compose/config.json"

BASE_COMPOSE = '''"""A stand-in for the display template's compose script."""

HEAD = "one"


def components():
    return [{"name": "display-shell"}]


TAIL = "two"
'''


def git(repo, *args, check=True):
    return subprocess.run(["git", "-C", str(repo), *args],
                          capture_output=True, text=True, check=check)


class MergeDriverCase(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.repo = pathlib.Path(self._tmp.name) / "repo"
        self.addCleanup(self._tmp.cleanup)
        (self.repo / "scripts").mkdir(parents=True)
        (self.repo / "templates/display/compose").mkdir(parents=True)
        for name in ("display_sheet_strip.py", "display_sync.py", "git_merge_display_sync.py"):
            shutil.copy2(SCRIPTS / name, self.repo / "scripts" / name)
        (self.repo / ".gitattributes").write_text(
            f"{CONFIG} merge=display-sync\n", encoding="utf-8")
        self.write_pair(BASE_COMPOSE)

        git(self.repo, "init", "-q", "-b", "master")
        git(self.repo, "config", "user.email", "test@example.invalid")
        git(self.repo, "config", "user.name", "test")
        git(self.repo, "config", "merge.display-sync.name", "display template sync")
        git(self.repo, "config", "merge.display-sync.driver",
            f"python3 {DRIVER} %O %A %B")
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-qm", "base")

    # -- fixture helpers ---------------------------------------------------

    def write_pair(self, compose_text):
        """Write `compose.py` and the `config.json` that carries it."""
        (self.repo / COMPOSE).write_text(compose_text, encoding="utf-8")
        cfg = {"type": "web", "params": {"mount": "/display", "script_inline": compose_text}}
        (self.repo / CONFIG).write_text(
            json.dumps(cfg, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")

    def write_config(self, params):
        """A `config.json` with exactly these params, in the driver's spelling."""
        (self.repo / CONFIG).write_text(
            json.dumps({"type": "web", "params": params}, indent=2, ensure_ascii=False)
            + "\n", encoding="utf-8")

    def branch_commit(self, name, compose_text, from_ref="master"):
        git(self.repo, "checkout", "-q", "-b", name, from_ref)
        self.write_pair(compose_text)
        git(self.repo, "commit", "-qam", name)

    def config(self):
        return json.loads((self.repo / CONFIG).read_text(encoding="utf-8"))

    # -- cases -------------------------------------------------------------

    def test_two_branches_edit_compose_and_the_merge_is_clean(self):
        self.branch_commit("ours", BASE_COMPOSE.replace('HEAD = "one"', 'HEAD = "ours"'))
        self.branch_commit("theirs", BASE_COMPOSE.replace('TAIL = "two"', 'TAIL = "theirs"'))
        git(self.repo, "checkout", "-q", "ours")
        merged = git(self.repo, "merge", "--no-edit", "theirs", check=False)
        self.assertEqual(merged.returncode, 0,
                         f"the merge conflicted:\n{merged.stdout}\n{merged.stderr}")

        compose = (self.repo / COMPOSE).read_text(encoding="utf-8")
        self.assertIn('HEAD = "ours"', compose)
        self.assertIn('TAIL = "theirs"', compose)
        self.assertEqual(self.config()["params"]["script_inline"], compose,
                         "script_inline is not the merged compose.py")

    def test_a_field_only_this_side_changed_survives_the_merge(self):
        # The case the review measured: a strand adds a param of its own, master
        # only touches compose.py. Taking the other side wholesale dropped the
        # param without a word -- a clean merge that silently loses data is the
        # most expensive kind of red there is.
        ours = BASE_COMPOSE.replace('HEAD = "one"', 'HEAD = "ours"')
        self.branch_commit("ours", ours)
        self.write_config({"mount": "/display", "max_concurrency": 1,
                           "script_inline": ours})
        git(self.repo, "commit", "-qam", "ours max_concurrency")

        self.branch_commit("theirs", BASE_COMPOSE.replace('TAIL = "two"',
                                                          'TAIL = "theirs"'))
        git(self.repo, "checkout", "-q", "ours")
        merged = git(self.repo, "merge", "--no-edit", "theirs", check=False)
        self.assertEqual(merged.returncode, 0, merged.stdout + merged.stderr)

        params = self.config()["params"]
        self.assertEqual(params.get("max_concurrency"), 1,
                         "the field only this side changed was dropped")
        compose = (self.repo / COMPOSE).read_text(encoding="utf-8")
        self.assertIn('HEAD = "ours"', compose)
        self.assertIn('TAIL = "theirs"', compose)
        self.assertEqual(params["script_inline"], compose)

    def test_a_field_only_the_other_side_changed_comes_across(self):
        ours = BASE_COMPOSE.replace('HEAD = "one"', 'HEAD = "ours"')
        self.branch_commit("ours", ours)

        theirs = BASE_COMPOSE.replace('TAIL = "two"', 'TAIL = "theirs"')
        git(self.repo, "checkout", "-q", "-b", "theirs", "master")
        (self.repo / COMPOSE).write_text(theirs, encoding="utf-8")
        self.write_config({"mount": "/theirs", "script_inline": theirs})
        git(self.repo, "commit", "-qam", "theirs mount")

        git(self.repo, "checkout", "-q", "ours")
        merged = git(self.repo, "merge", "--no-edit", "theirs", check=False)
        self.assertEqual(merged.returncode, 0, merged.stdout + merged.stderr)
        self.assertEqual(self.config()["params"]["mount"], "/theirs")
        self.assertEqual(self.config()["params"]["script_inline"],
                         (self.repo / COMPOSE).read_text(encoding="utf-8"))

    def test_both_sides_on_one_field_is_a_conflict_git_can_show(self):
        # Two sides, one field, two values: nobody but a person can pick. The
        # driver leaves the markers around the readable line and keeps the
        # unreadable one intact, so `git status` says UU and a diff is legible.
        ours = BASE_COMPOSE.replace('HEAD = "one"', 'HEAD = "ours"')
        self.branch_commit("ours", ours)
        self.write_config({"mount": "/ours", "script_inline": ours})
        git(self.repo, "commit", "-qam", "ours mount")

        theirs = BASE_COMPOSE.replace('TAIL = "two"', 'TAIL = "theirs"')
        git(self.repo, "checkout", "-q", "-b", "theirs", "master")
        (self.repo / COMPOSE).write_text(theirs, encoding="utf-8")
        self.write_config({"mount": "/theirs", "script_inline": theirs})
        git(self.repo, "commit", "-qam", "theirs mount")

        git(self.repo, "checkout", "-q", "ours")
        merged = git(self.repo, "merge", "--no-edit", "theirs", check=False)
        self.assertNotEqual(merged.returncode, 0, "the field conflict was swallowed")
        status = git(self.repo, "status", "--porcelain").stdout
        self.assertIn(f"UU {CONFIG}", status)

        text = (self.repo / CONFIG).read_text(encoding="utf-8")
        self.assertIn("<<<<<<<", text)
        self.assertIn("/ours", text)
        self.assertIn("/theirs", text)
        self.assertNotIn("__display_sync_script_inline__", text,
                         "the placeholder stayed in the file instead of compose.py")
        self.assertIn('HEAD = ', text)

    def test_a_conflicting_compose_leaves_the_conflict_standing(self):
        # Both sides changed the same line, so git cannot merge compose.py.
        # Regenerating script_inline from a file full of conflict markers would
        # hide the collision inside a single JSON line -- the driver refuses.
        self.branch_commit("ours", BASE_COMPOSE.replace('HEAD = "one"', 'HEAD = "ours"'))
        self.branch_commit("theirs", BASE_COMPOSE.replace('HEAD = "one"', 'HEAD = "theirs"'))
        git(self.repo, "checkout", "-q", "ours")
        merged = git(self.repo, "merge", "--no-edit", "theirs", check=False)
        self.assertNotEqual(merged.returncode, 0, "the merge should have conflicted")
        status = git(self.repo, "status", "--porcelain").stdout
        self.assertIn(COMPOSE, status)
        self.assertIn(CONFIG, status)

    def test_the_driver_refuses_a_config_that_is_not_json(self):
        script = self.repo / "scripts" / "git_merge_display_sync.py"
        for name, text in (("O", "{}"), ("A", "{}"), ("B", "not json")):
            (self.repo / name).write_text(text, encoding="utf-8")
        out = subprocess.run(["python3", str(script), "O", "A", "B"],
                             cwd=self.repo, capture_output=True, text=True)
        self.assertEqual(out.returncode, 1)


if __name__ == "__main__":
    unittest.main()
