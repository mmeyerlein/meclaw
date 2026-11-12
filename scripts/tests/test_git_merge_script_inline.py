#!/usr/bin/env python3
"""Cases for the `script-inline` git merge driver.

A code cell's `config.json` carries its whole script as one `params.script_inline`
string -- for `templates/curator/policy/config.json` that is one JSON line of
about 500 000 characters. Two branches that edit the script in two different
places collide there as a single line nobody can resolve by hand, and unlike the
display's `compose.py` these cells have no source file beside the config that
git could merge instead. The driver merges the script as text, the readable
fields like any other file, and on a script conflict writes the marks into a
file under `target/` -- never into the JSON line.

Each merge case builds a throwaway git repository holding only the files the
driver touches. The attributes case and the form case also read the real tree:
the list in `.gitattributes` and the spelling of the committed configs are what
the driver has to agree with.
"""
import json
import os
import pathlib
import shutil
import subprocess
import tempfile
import unittest
from scripts.tests import store_isolation

REPO = pathlib.Path(__file__).resolve().parents[2]
SCRIPTS = REPO / "scripts"
DRIVER = "scripts/git_merge_script_inline.py"
CONFIG = "templates/curator/policy/config.json"
CONFLICT_FILE = "target/script-inline-conflicts/templates/curator/policy/script_inline.py"

# The clone's own settings only: a developer's `merge.conflictStyle` or a
# global attributes file would change what a merge writes.
GIT_ENV = {"GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_SYSTEM": os.devnull}

BASE_SCRIPT = '''"""A stand-in for a cell script."""

HEAD = "one"


def run(message):
    return [{"route": "out", "payload": message}]


TAIL = "two"
'''


def git(repo, *args, check=True):
    return subprocess.run(["git", "-C", str(repo), *args], env={**os.environ, **GIT_ENV},
                          capture_output=True, text=True, check=check)


def driver(repo, *args):
    return subprocess.run(["python3", DRIVER, *args], cwd=repo,
                          env={**os.environ, **GIT_ENV}, capture_output=True, text=True)


def cell(script, params=None, description="The policy cell."):
    """A cell config in the key order the tree's code cells use."""
    return {"cell": "code",
            "params": {"runner": "python", "external_timeout_ms": 30000,
                       **(params or {}), "script_inline": script},
            "contract": {"accepts": ["in"], "emits": ["out"]},
            "description": description}


def tree_form(cfg, ensure_ascii=False):
    """The bytes a config has in the tree, spelled here a second time on purpose:
    the driver's own writer is what the cases judge."""
    return (json.dumps(cfg, indent=2, ensure_ascii=ensure_ascii) + "\n").encode("utf-8")


class ScriptInlineCase(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.repo = pathlib.Path(self._tmp.name) / "repo"
        self.addCleanup(self._tmp.cleanup)
        (self.repo / "scripts").mkdir(parents=True)
        shutil.copy2(SCRIPTS / "git_merge_script_inline.py", self.repo / DRIVER)
        (self.repo / ".gitattributes").write_text(
            f"{CONFIG} merge=script-inline\n", encoding="utf-8")
        (self.repo / ".gitignore").write_text("/target/\n", encoding="utf-8")
        self.write(cell(BASE_SCRIPT))

        git(self.repo, "init", "-q", "-b", "master")
        git(self.repo, "config", "user.email", "test@example.invalid")
        git(self.repo, "config", "user.name", "test")
        installed = driver(self.repo, "--install")
        self.assertEqual(installed.returncode, 0, installed.stdout + installed.stderr)
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-qm", "base")

    # -- fixture helpers ---------------------------------------------------

    def write(self, cfg, ensure_ascii=False):
        path = self.repo / CONFIG
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(tree_form(cfg, ensure_ascii))

    def branch_commit(self, name, cfg, from_ref="master", ensure_ascii=False):
        git(self.repo, "checkout", "-q", "-b", name, from_ref)
        self.write(cfg, ensure_ascii)
        git(self.repo, "commit", "-qam", name)

    def merge(self, ours="ours", theirs="theirs"):
        git(self.repo, "checkout", "-q", ours)
        return git(self.repo, "merge", "--no-edit", theirs, check=False)

    def raw(self):
        return (self.repo / CONFIG).read_bytes()

    def config(self):
        return json.loads(self.raw())

    def status(self):
        return git(self.repo, "status", "--porcelain", "--untracked-files=all").stdout

    def same_line_conflict(self, theirs_params=None):
        """Both sides rewrite the same script line; the merge is left standing."""
        self.branch_commit("ours", cell(BASE_SCRIPT.replace('HEAD = "one"', 'HEAD = "ours"')))
        self.branch_commit("theirs", cell(
            BASE_SCRIPT.replace('HEAD = "one"', 'HEAD = "theirs"'), theirs_params))
        return self.merge()

    # -- cases -------------------------------------------------------------

    def test_two_edits_in_different_places_merge_clean(self):
        self.branch_commit("ours", cell(BASE_SCRIPT.replace('HEAD = "one"', 'HEAD = "ours"')))
        self.branch_commit("theirs", cell(BASE_SCRIPT.replace('TAIL = "two"',
                                                              'TAIL = "theirs"')))
        merged = self.merge()
        self.assertEqual(merged.returncode, 0,
                         f"the merge conflicted:\n{merged.stdout}\n{merged.stderr}")
        script = self.config()["params"]["script_inline"]
        self.assertEqual(script, BASE_SCRIPT.replace('HEAD = "one"', 'HEAD = "ours"')
                         .replace('TAIL = "two"', 'TAIL = "theirs"'))
        self.assertEqual(self.status(), "", "the merge left something behind")
        self.assertFalse((self.repo / CONFLICT_FILE).exists())

    def test_two_edits_of_the_same_line_conflict_with_readable_marks(self):
        merged = self.same_line_conflict()
        self.assertNotEqual(merged.returncode, 0, "the script conflict was swallowed")
        self.assertIn(f"UU {CONFIG}", self.status())

        # The JSON line never carries a mark: the config is still one valid
        # document, with this side's script whole.
        self.assertNotIn(b"<<<<<<<", self.raw())
        self.assertNotIn(b">>>>>>>", self.raw())
        self.assertEqual(self.config()["params"]["script_inline"],
                         BASE_SCRIPT.replace('HEAD = "one"', 'HEAD = "ours"'))

        # The marks stand in a file of their own, one script line per line.
        lines = (self.repo / CONFLICT_FILE).read_text(encoding="utf-8").splitlines()
        start = lines.index("<<<<<<< ours")
        self.assertEqual(lines[start:start + 5],
                         ["<<<<<<< ours", 'HEAD = "ours"', "=======", 'HEAD = "theirs"',
                          ">>>>>>> theirs"])
        self.assertIn('TAIL = "two"', lines)
        # ... outside the template tree: nothing new lies under templates/.
        self.assertEqual([line for line in self.status().splitlines()
                          if not line.startswith("UU ")], [])
        self.assertEqual(sorted(p.name for p in (self.repo / CONFIG).parent.iterdir()),
                         ["config.json"])
        # The way back is named where the person reads it.
        self.assertIn("--resolve", merged.stdout + merged.stderr)
        self.assertIn(CONFLICT_FILE, merged.stdout + merged.stderr)

    def test_a_field_only_one_side_changed_survives(self):
        # A strand adds a param of its own, the other side edits the script and
        # a readable field elsewhere. Taking either side wholesale would drop
        # one of them without a word.
        ours = BASE_SCRIPT.replace('HEAD = "one"', 'HEAD = "ours"')
        theirs = BASE_SCRIPT.replace('TAIL = "two"', 'TAIL = "theirs"')
        self.branch_commit("ours", cell(ours, {"keep_recent": 3}))
        self.branch_commit("theirs", cell(theirs, description="The policy cell, reworded."))
        merged = self.merge()
        self.assertEqual(merged.returncode, 0, merged.stdout + merged.stderr)

        cfg = self.config()
        self.assertEqual(cfg["params"].get("keep_recent"), 3,
                         "the field only this side added was dropped")
        self.assertEqual(cfg["description"], "The policy cell, reworded.",
                         "the field only the other side changed did not come across")
        self.assertEqual(cfg["params"]["script_inline"],
                         ours.replace('TAIL = "two"', 'TAIL = "theirs"'))
        self.assertEqual(list(cfg["params"]),
                         ["runner", "external_timeout_ms", "keep_recent", "script_inline"])

    def test_two_sides_each_add_a_param_and_both_survive(self):
        # Measured on the policy cell: two strands each append one new param
        # at the end of `params`. For a merge by text line that is the same
        # place and a conflict; by key it is two keys nobody shares.
        ours, theirs, expected = cell(BASE_SCRIPT), cell(BASE_SCRIPT), cell(BASE_SCRIPT)
        ours["params"]["probe_x"] = "1"
        theirs["params"]["probe_y"] = {"soft": 2, "hard": [3, 4]}
        self.branch_commit("ours", ours)
        self.branch_commit("theirs", theirs)
        merged = self.merge()
        self.assertEqual(merged.returncode, 0,
                         f"the merge conflicted:\n{merged.stdout}\n{merged.stderr}")
        self.assertEqual(self.status(), "", "the merge left something behind")

        # This side's keys in its order, then the other side's new key; the
        # bytes are the tree's form.
        expected["params"]["probe_x"] = "1"
        expected["params"]["probe_y"] = {"soft": 2, "hard": [3, 4]}
        self.assertEqual(list(self.config()["params"]),
                         ["runner", "external_timeout_ms", "script_inline", "probe_x",
                          "probe_y"])
        self.assertEqual(self.raw(), tree_form(expected))

    def test_the_same_key_changed_on_both_sides_conflicts_with_its_path(self):
        def side(keep_recent, soft, description="The policy cell.", drop=()):
            cfg = cell(BASE_SCRIPT, {"keep_recent": keep_recent, "spare": "x", "idle": "y", "budget": 10,
                                     "limits": {"soft": soft, "hard": 9}},
                       description=description)
            for key in drop:
                del cfg["params"][key]
            return cfg

        self.branch_commit("key-base", side(3, 1))
        # Three collisions: one key with two new values, one key inside a
        # nested object with two new values, and one key this side deletes
        # while the other changes it. Beside them, what does not collide: a key
        # only this side deletes, a key only the other side deletes, a field
        # only the other side changes.
        ours = side(5, 2, drop=("spare", "budget"))
        theirs = side(7, 4, description="The policy cell, reworded.", drop=("idle",))
        theirs["params"]["budget"] = 20
        self.branch_commit("ours", ours, "key-base")
        self.branch_commit("theirs", theirs, "key-base")
        merged = self.merge()
        self.assertNotEqual(merged.returncode, 0, "the conflict of a key was swallowed")
        self.assertIn(f"UU {CONFIG}", self.status())

        # The message names each key by its path.
        said = merged.stdout + merged.stderr
        for path in ("params.keep_recent", "params.limits.soft", "params.budget"):
            self.assertIn(path, said)
        for path in ("params.spare", "params.idle", "params.limits.hard", "description"):
            self.assertNotIn(path, said)

        # The config never carries a mark: it is one valid document in the
        # tree's form, with this side's state at each key that conflicted and
        # everything else merged.
        self.assertNotIn(b"<<<<<<<", self.raw())
        self.assertNotIn(b">>>>>>>", self.raw())
        expected = side(5, 2, description="The policy cell, reworded.",
                        drop=("spare", "idle", "budget"))
        self.assertEqual(self.raw(), tree_form(expected))
        # No script conflicted: nothing waits under target/.
        self.assertFalse((self.repo / CONFLICT_FILE).exists())
        self.assertEqual([line for line in self.status().splitlines()
                          if not line.startswith("UU ")], [])

    def test_the_attributes_list_names_every_cell_with_a_script(self):
        from scripts import git_merge_script_inline as drv

        # The real tree: every config with a script under the driver's roots,
        # found here by a walk of its own, is what `.gitattributes` names.
        found = []
        for root in ("curator", "talky", "collector", "assistant", "objects", "lead"):
            for path in sorted((REPO / "templates" / root).rglob("config.json")):
                params = json.loads(path.read_bytes()).get("params")
                if isinstance(params, dict) and isinstance(params.get("script_inline"), str):
                    found.append(path.relative_to(REPO).as_posix())
        self.assertIn(CONFIG, found)
        want = [f"{rel} merge=script-inline" for rel in sorted(found)]
        listed = subprocess.run(["python3", str(REPO / DRIVER), "--attributes"],
                                capture_output=True, text=True)
        self.assertEqual(listed.returncode, 0, listed.stderr)
        self.assertEqual(listed.stdout.splitlines(), want)
        attrs = (REPO / ".gitattributes").read_text(encoding="utf-8").splitlines()
        self.assertEqual([line for line in attrs if line.endswith(" merge=script-inline")],
                         want, "`.gitattributes` is not the generated list -- run "
                               f"`python3 {DRIVER} --attributes --write`")
        # The display's driver keeps its own lines.
        self.assertIn("templates/display/compose/config.json merge=display-sync", attrs)
        self.assertIn("templates/presenter/stage/config.json merge=display-sync", attrs)

        # A throwaway tree: a root that is missing is no error, and a root that
        # appears later is taken up by itself, nested cells included.
        self.assertFalse((self.repo / "templates/lead").exists())
        before = driver(self.repo, "--attributes")
        self.assertEqual(before.returncode, 0, before.stderr)
        self.assertEqual(before.stdout.splitlines(), [f"{CONFIG} merge=script-inline"])

        for rel, cfg in (("templates/lead/plan/config.json", cell(BASE_SCRIPT)),
                         ("templates/assistant/talky/judge/config.json", cell(BASE_SCRIPT)),
                         # no script: an llm cell is not the driver's business
                         ("templates/lead/model/config.json",
                          {"cell": "llm", "params": {"model": "x"}}),
                         # outside the roots
                         ("templates/display/compose/config.json", cell(BASE_SCRIPT))):
            path = self.repo / rel
            path.parent.mkdir(parents=True)
            path.write_bytes(tree_form(cfg))
        written = driver(self.repo, "--attributes", "--write")
        self.assertEqual(written.returncode, 0, written.stderr)
        attrs = (self.repo / ".gitattributes").read_text(encoding="utf-8").splitlines()
        self.assertEqual([line for line in attrs if line.endswith(" merge=script-inline")],
                         ["templates/assistant/talky/judge/config.json merge=script-inline",
                          f"{CONFIG} merge=script-inline",
                          "templates/lead/plan/config.json merge=script-inline"])
        self.assertEqual(drv.attribute_lines(self.repo),
                         [line for line in attrs if line.endswith(" merge=script-inline")])
        # Written twice it is the same file.
        once = (self.repo / ".gitattributes").read_bytes()
        self.assertEqual(driver(self.repo, "--attributes", "--write").returncode, 0)
        self.assertEqual((self.repo / ".gitattributes").read_bytes(), once)

    def test_a_clean_merge_writes_the_config_back_byte_for_byte(self):
        from scripts import git_merge_script_inline as drv

        # The form is the tree's: every committed config the driver is named
        # for comes out of the driver's writer as the bytes it has on disk.
        named = [line.split(" ", 1)[0] for line in drv.attribute_lines(REPO)]
        self.assertIn(CONFIG, named)
        for rel in named:
            raw = (REPO / rel).read_bytes()
            with self.subTest(config=rel):
                self.assertEqual(drv.render(json.loads(raw), drv.form_of(raw)), raw)

        # Both spellings the tree carries: characters outside ASCII written as
        # they are (the policy cell), and written as escapes (the intake cell).
        base = BASE_SCRIPT.replace("stand-in", "stand-in – with a dash ä")
        ours = base.replace('HEAD = "one"', 'HEAD = "ours ö"')
        theirs = base.replace('TAIL = "two"', 'TAIL = "theirs →"')
        expected = cell(ours.replace('TAIL = "two"', 'TAIL = "theirs →"'),
                        {"keep_recent": 3}, description="A dash – too.")
        for ensure_ascii in (False, True):
            with self.subTest(ensure_ascii=ensure_ascii):
                git(self.repo, "checkout", "-q", "master")
                for name in ("form-base", "ours", "theirs"):
                    git(self.repo, "branch", "-qD", name, check=False)
                self.branch_commit("form-base", cell(base), ensure_ascii=ensure_ascii)
                self.branch_commit("ours", cell(ours, {"keep_recent": 3}), "form-base",
                                   ensure_ascii)
                self.branch_commit("theirs", cell(theirs, description="A dash – too."),
                                   "form-base", ensure_ascii)
                merged = self.merge()
                self.assertEqual(merged.returncode, 0, merged.stdout + merged.stderr)
                self.assertEqual(self.raw(), tree_form(expected, ensure_ascii))
                self.assertEqual(self.raw().isascii(), ensure_ascii)

    def test_a_resolved_script_goes_back_into_the_config(self):
        # The other side also changed a readable field: resolving the script
        # must not cost that field.
        merged = self.same_line_conflict(theirs_params={"keep_recent": 3})
        self.assertNotEqual(merged.returncode, 0)
        conflict = self.repo / CONFLICT_FILE
        standing = self.raw()

        # Marks still in the file: refused, the config untouched.
        refused = driver(self.repo, "--resolve", CONFIG, CONFLICT_FILE)
        self.assertEqual(refused.returncode, 1, refused.stdout + refused.stderr)
        self.assertIn("<<<<<<< ours", refused.stderr)
        self.assertEqual(self.raw(), standing)
        self.assertTrue(conflict.is_file())

        text = conflict.read_text(encoding="utf-8")
        block = '<<<<<<< ours\nHEAD = "ours"\n=======\nHEAD = "theirs"\n>>>>>>> theirs\n'
        self.assertIn(block, text)
        resolved = text.replace(block, 'HEAD = "ours and theirs"\n')
        conflict.write_text(resolved, encoding="utf-8")

        done = driver(self.repo, "--resolve", CONFIG, CONFLICT_FILE)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertEqual(resolved, BASE_SCRIPT.replace('HEAD = "one"',
                                                       'HEAD = "ours and theirs"'))
        # The field only the other side added sits behind our own keys (the
        # field merge keeps our order, then appends what the other side added).
        expected = cell(resolved)
        expected["params"]["keep_recent"] = 3
        self.assertEqual(self.raw(), tree_form(expected))
        self.assertFalse(conflict.exists(), "the conflict file stayed behind")

        git(self.repo, "add", CONFIG)
        git(self.repo, "commit", "-qm", "merge resolved")
        self.assertEqual(self.status(), "")

    def test_install_sets_the_driver_once(self):
        clone = pathlib.Path(self._tmp.name) / "clone"
        (clone / "scripts").mkdir(parents=True)
        shutil.copy2(SCRIPTS / "git_merge_script_inline.py", clone / DRIVER)
        git(clone, "init", "-q", "-b", "master")
        self.assertEqual(git(clone, "config", "--get-regexp", r"^merge\.", check=False).stdout,
                         "")

        first = driver(clone, "--install")
        self.assertEqual(first.returncode, 0, first.stdout + first.stderr)
        self.assertEqual(
            git(clone, "config", "--local", "--get-regexp", r"^merge\.").stdout.splitlines(),
            ["merge.script-inline.name cell script_inline three-way merge",
             "merge.script-inline.driver "
             "python3 scripts/git_merge_script_inline.py %O %A %B %P"])
        self.assertIn("installed", first.stdout)

        # The second run changes nothing: not a byte of the clone's config.
        config = (clone / ".git" / "config").read_bytes()
        second = driver(clone, "--install")
        self.assertEqual(second.returncode, 0, second.stdout + second.stderr)
        self.assertEqual((clone / ".git" / "config").read_bytes(), config)
        self.assertIn("nothing changed", second.stdout)

        # A linked worktree shares the clone's config: installed from there,
        # it is the same two lines and again no change.
        (clone / "README.md").write_text("x\n", encoding="utf-8")
        git(clone, "add", "README.md", DRIVER)
        git(clone, "-c", "user.email=test@example.invalid", "-c", "user.name=test",
            "commit", "-qm", "first")
        tree = pathlib.Path(self._tmp.name) / "tree"
        git(clone, "worktree", "add", "-q", "-b", "strand", str(tree))
        third = driver(tree, "--install")
        self.assertEqual(third.returncode, 0, third.stdout + third.stderr)
        self.assertEqual((clone / ".git" / "config").read_bytes(), config)
        # A path names the clone as well as the working directory does.
        fourth = subprocess.run(["python3", str(clone / DRIVER), "--install", str(tree)],
                                cwd=self._tmp.name, env={**os.environ, **GIT_ENV},
                                capture_output=True, text=True)
        self.assertEqual(fourth.returncode, 0, fourth.stdout + fourth.stderr)
        self.assertEqual((clone / ".git" / "config").read_bytes(), config)


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


if __name__ == "__main__":
    unittest.main()
