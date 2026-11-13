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
FIELDS_FILE = "target/script-inline-conflicts/templates/curator/policy/fields.json"

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
             "python3 scripts/git_merge_script_inline.py %O %A %B %P",
             # OR-58-146: the internal merge of two merge bases has its own mode
             "merge.script-inline.recursive script-inline-inner",
             "merge.script-inline-inner.name cell script_inline merge of merge bases",
             "merge.script-inline-inner.driver "
             "python3 scripts/git_merge_script_inline.py --inner %O %A %B %P"])
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


    # -- more than one merge base (OR-58-146) ---------------------------------

    def merge_commit(self, branch, other, cfg):
        """`branch` merges `other` and settles the config at `cfg` by hand,
        the way a person resolves a conflict and commits."""
        git(self.repo, "checkout", "-q", branch)
        git(self.repo, "merge", "--no-edit", "--no-commit", other, check=False)
        self.write(cfg)
        git(self.repo, "add", CONFIG)
        git(self.repo, "commit", "-qm", f"{branch} merges {other}", "--no-edit")

    def test_a_field_only_one_side_changed_survives_a_criss_cross_merge(self):
        # The followup merge of d58-2 (6cdf3754a) had three merge bases. git
        # first merges the bases into a virtual one with the same driver; a
        # field conflict there kept the first base's value, so in the real merge
        # a field only the other side had changed looked changed on both sides
        # and this side's value stayed in a valid config -- one `git add` and the
        # other side's change was gone (befund-mergedrv, repro-merge.log 15-18).
        #
        # Two keys, so that whichever base git calls "ours" inside, one of them
        # would get a base value nobody ended at: both bases change `route` and
        # `note` to two values, both criss-cross merges settle both keys back at
        # the start value, then only the other side changes them again.
        def side(route, note, extra=None):
            return cell(BASE_SCRIPT, {"route": route, "note": note, **(extra or {})})

        self.branch_commit("cc-base", side("start", "start"))
        self.branch_commit("p1", side("p", "p"), "cc-base")
        self.branch_commit("q1", side("q", "q"), "cc-base")
        git(self.repo, "branch", "-q", "ours", "p1")
        git(self.repo, "branch", "-q", "theirs", "q1")
        self.merge_commit("ours", "q1", side("start", "start"))
        self.merge_commit("theirs", "p1", side("start", "start"))
        bases = git(self.repo, "merge-base", "--all", "ours", "theirs").stdout.split()
        self.assertEqual(len(bases), 2, "the history is no criss-cross")

        # Only the other side changes the two keys; this side changes a third
        # one, so the driver has work on both sides.
        git(self.repo, "checkout", "-q", "theirs")
        self.write(side("note_receipt", "theirs"))
        git(self.repo, "commit", "-qam", "theirs changes route and note")
        git(self.repo, "checkout", "-q", "ours")
        self.write(side("start", "start", {"keep_recent": 3}))
        git(self.repo, "commit", "-qam", "ours adds a param")

        # Vorgabe Nr. 3: the other side's value, or a visible conflict -- never
        # this side's value silently. Here the bases changed both keys two
        # ways, so the virtual base marks them and the merge conflicts.
        merged = self.merge()
        said = merged.stdout + merged.stderr
        cfg = self.config()
        self.assertEqual(cfg["params"]["keep_recent"], 3)
        if merged.returncode == 0:
            self.assertEqual(cfg["params"]["route"], "note_receipt",
                             "the other side's change of a key only it changed was lost")
            self.assertEqual(cfg["params"]["note"], "theirs")
            self.assertEqual(self.status(), "", "the merge left something behind")
        else:
            self.assert_visible_field_conflict(said, {"params.route": "note_receipt",
                                                      "params.note": "theirs"})

    def assert_visible_field_conflict(self, said, theirs):
        """The merge stopped with each key of `theirs` in `fields.json` (with
        the other side's value) and in the message, the config unmerged."""
        fields_file = self.repo / FIELDS_FILE
        self.assertTrue(fields_file.is_file(), f"no fields.json:\n{said}")
        table = json.loads(fields_file.read_bytes())
        for path, value in theirs.items():
            self.assertIn(path, table, f"{path} is not in fields.json:\n{said}")
            self.assertEqual(table[path]["theirs"], value)
            self.assertIn(f"    {path}: ", said)
        self.assertIn(f"UU {CONFIG}", self.status(), "the config is not left unmerged")

    def criss_cross(self, p_cfg, q_cfg, settle_ours, settle_theirs):
        """Two merge bases p1 and q1 off cc-base; `ours` merges q1 and settles
        at `settle_ours`, `theirs` merges p1 and settles at `settle_theirs`."""
        self.branch_commit("p1", p_cfg, "cc-base")
        self.branch_commit("q1", q_cfg, "cc-base")
        git(self.repo, "branch", "-q", "ours", "p1")
        git(self.repo, "branch", "-q", "theirs", "q1")
        self.merge_commit("ours", "q1", settle_ours)
        self.merge_commit("theirs", "p1", settle_theirs)
        bases = git(self.repo, "merge-base", "--all", "ours", "theirs").stdout.split()
        self.assertEqual(len(bases), 2, "the history is no criss-cross")

    def commit_on(self, branch, cfg):
        git(self.repo, "checkout", "-q", branch)
        self.write(cfg)
        git(self.repo, "commit", "-qam", f"{branch} moves on")

    def test_a_key_the_criss_cross_merges_settled_two_ways_conflicts(self):
        # Review close-kit I1, probes 6 and 9: the two criss-cross merges settle
        # `k` two ways, one of them back at the old value 0, the other at 2;
        # nobody touches `k` afterwards. That is a real disagreement: with the
        # older base's value in the virtual base the merge took the side that
        # had moved away from it, exit 0, the other side's decision gone.
        def side(k, extra=None):
            return cell(BASE_SCRIPT, {"k": k, **(extra or {})})

        for ours_k, theirs_k in ((0, 2), (2, 0)):
            with self.subTest(ours=ours_k, theirs=theirs_k):
                self.setUp()
                self.branch_commit("cc-base", side(0))
                self.criss_cross(side(1), side(2), side(ours_k), side(theirs_k))
                self.commit_on("theirs", side(theirs_k, {"y": 1}))
                self.commit_on("ours", side(ours_k, {"x": 1}))
                merged = self.merge()
                said = merged.stdout + merged.stderr
                self.assertNotEqual(merged.returncode, 0,
                                    f"a key settled two ways merged silently:\n{said}")
                self.assert_visible_field_conflict(said, {"params.k": theirs_k})
                table = json.loads((self.repo / FIELDS_FILE).read_bytes())
                self.assertEqual(table["params.k"]["ours"], ours_k)
                self.assertEqual(self.config()["params"]["x"], 1)

    def test_a_script_line_the_criss_cross_merges_settled_two_ways_conflicts(self):
        # Review close-kit I1, probe 7: the same with a script line. The bases
        # set `HEAD` two ways; ours settles back at the old line, theirs keeps
        # its own; afterwards each side edits another line.
        def head(value, tail="two"):
            return cell(BASE_SCRIPT.replace('HEAD = "one"', f'HEAD = "{value}"')
                        .replace('TAIL = "two"', f'TAIL = "{tail}"'))

        git(self.repo, "branch", "-q", "cc-base", "master")    # master is head("one")
        self.criss_cross(head("p"), head("q"), head("one"), head("q"))
        self.commit_on("theirs", head("q", "theirs"))
        self.commit_on("ours", cell(BASE_SCRIPT.replace(
            "def run(message):", "def run(message):  # ours")))
        merged = self.merge()
        said = merged.stdout + merged.stderr
        self.assertNotEqual(merged.returncode, 0,
                            f"a script line settled two ways merged silently:\n{said}")
        waiting = (self.repo / CONFLICT_FILE).read_text(encoding="utf-8")
        self.assertIn('HEAD = "q"', waiting)
        self.assertIn("<<<<<<< ", waiting)
        self.assertIn(f"the script of {CONFIG} conflicted", said)
        self.assertIn(f"UU {CONFIG}", self.status())

    def test_an_internal_merge_favours_no_side(self):
        # `--inner` is the driver git runs for the virtual base: a key changed
        # two ways (also deleted against changed, added two ways) gets a mark
        # value that equals neither side and is the same whichever side comes
        # first, a script hunk changed two ways gets merge-file's marks.
        base = cell(BASE_SCRIPT, {"both": "b", "mine": "b", "other": "b", "gone": "b",
                                  "limits": {"soft": 1, "hard": 2}})
        ours = cell(BASE_SCRIPT.replace('HEAD = "one"', 'HEAD = "ours"')
                    .replace('TAIL = "two"', 'TAIL = "ours"'),
                    {"both": "o", "mine": "o", "other": "b", "limits": {"soft": 5, "hard": 2},
                     "added": "o"})
        theirs = cell(BASE_SCRIPT.replace('HEAD = "one"', 'HEAD = "theirs"'),
                      {"both": "t", "mine": "b", "other": "t", "gone": "t",
                       "limits": {"soft": 7, "hard": 3}, "added": "t"})
        files = {}
        for name, cfg in (("base", base), ("ours", ours), ("theirs", theirs)):
            files[name] = self.repo / f"{name}.json"
            files[name].write_bytes(tree_form(cfg))
        results = []
        for first, second in (("ours", "theirs"), ("theirs", "ours")):
            out = self.repo / "out.json"
            out.write_bytes(files[first].read_bytes())
            done = driver(self.repo, "--inner", str(files["base"]), str(out),
                          str(files[second]), CONFIG)
            self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
            results.append(json.loads(out.read_bytes()))
        scripts = [r["params"].pop("script_inline") for r in results]
        self.assertEqual(results[0], results[1], "the internal merge favours a side")

        params = results[0]["params"]
        self.assertEqual(params["mine"], "o")             # one side only: that side
        self.assertEqual(params["other"], "t")
        self.assertEqual(params["limits"]["hard"], 3)
        for key, values in (("both", ("o", "t")), ("gone", ("t",)),
                            ("added", ("o", "t"))):
            mark = params[key]
            self.assertIsInstance(mark, str)
            self.assertTrue(mark.startswith("\x00"), f"{key} carries no mark: {mark!r}")
            for value in values:
                self.assertIn(json.dumps(value), mark)
            self.assertNotIn(mark, {"b", "o", "t"})
        self.assertTrue(params["limits"]["soft"].startswith("\x00"))
        for script in scripts:
            # the hunk both changed carries both lines between marks, the hunk
            # only ours changed is merged
            self.assertIn('HEAD = "ours"', script)
            self.assertIn('HEAD = "theirs"', script)
            self.assertIn("<" * 41 + " ", script)
            self.assertIn('TAIL = "ours"', script)
            self.assertNotIn('TAIL = "two"', script)
        # Nothing waits under target/: an internal merge is no conflict of a person.
        self.assertFalse((self.repo / "target").exists())

    def test_an_internal_merge_with_a_base_without_a_script_marks_it(self):
        # Review close-kit m1: a merge base without `script_inline` (from before
        # the cell carried one) ended the internal merge with exit 1 and `%A`
        # left at one base -- that base's state kept silently. Now the bases
        # merge key by key, the script one key among them.
        base = {"cell": "code", "params": {"runner": "python"}}
        ours = cell(BASE_SCRIPT.replace('HEAD = "one"', 'HEAD = "ours"'), {"k": 1})
        theirs = {"cell": "code", "params": {"runner": "python", "k": 2}}
        for first, second, label in ((ours, theirs, "script on ours"),
                                     (theirs, ours, "script on theirs")):
            with self.subTest(label):
                files = []
                for name, cfg in (("base", base), ("out", first), ("other", second)):
                    files.append(self.repo / f"{name}.json")
                    files[-1].write_bytes(tree_form(cfg))
                done = driver(self.repo, "--inner", *map(str, files), CONFIG)
                self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
                params = json.loads(files[1].read_bytes())["params"]
                self.assertEqual(params["runner"], "python")
                self.assertTrue(params["k"].startswith("\x00"), params)
                # added on one side only: that side's script
                self.assertEqual(params["script_inline"], ours["params"]["script_inline"])
        # A side that is no JSON object: the virtual base becomes a mark text,
        # never one base's bytes.
        out = self.repo / "out.json"
        out.write_bytes(tree_form(ours))
        (self.repo / "other.json").write_bytes(b"{ not json")
        done = driver(self.repo, "--inner", str(self.repo / "base.json"), str(out),
                      str(self.repo / "other.json"), CONFIG)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertTrue(out.read_bytes().startswith(b"\x00"), out.read_bytes()[:80])
        self.assertFalse((self.repo / "target").exists())

    def test_an_internal_merge_with_scripts_the_bases_added_two_ways_marks_the_hunk(self):
        # Re-check close-kit m1': a script both bases added two ways on a base
        # without one became a mark value with a NUL, and the real merge's
        # `git merge-file` refused that base as binary -- even with both sides
        # agreeing. Now the scripts merge as text (no script = the empty
        # text): merge-file's marks, no NUL in the script.
        base = {"cell": "code", "params": {"runner": "python"}}
        ours = cell(BASE_SCRIPT.replace('HEAD = "one"', 'HEAD = "ours"'))
        theirs = cell(BASE_SCRIPT.replace('HEAD = "one"', 'HEAD = "theirs"'))
        files = []
        for name, cfg in (("base", base), ("out", ours), ("other", theirs)):
            files.append(self.repo / f"{name}.json")
            files[-1].write_bytes(tree_form(cfg))
        done = driver(self.repo, "--inner", *map(str, files), CONFIG)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        script = json.loads(files[1].read_bytes())["params"]["script_inline"]
        self.assertNotIn("\x00", script)
        self.assertIn("<" * 41 + " ", script)
        self.assertIn('HEAD = "ours"', script)
        self.assertIn('HEAD = "theirs"', script)

    def test_an_internal_merge_reads_an_empty_base_as_an_add_add(self):
        # Re-check close-kit m1': git hands an add/add of the merge bases over
        # with an empty `%O`; read as "no JSON object" the virtual base became
        # a mark text and the real merge stopped at the whole file. Now `{}`:
        # key by key, the scripts as text.
        ours = cell(BASE_SCRIPT, {"k": 1, "same": "s"})
        theirs = cell(BASE_SCRIPT, {"k": 2, "same": "s"})
        files = [self.repo / "base.json", self.repo / "out.json", self.repo / "other.json"]
        files[0].write_bytes(b"")
        files[1].write_bytes(tree_form(ours))
        files[2].write_bytes(tree_form(theirs))
        done = driver(self.repo, "--inner", *map(str, files), CONFIG)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        params = json.loads(files[1].read_bytes())["params"]
        self.assertEqual(params["script_inline"], BASE_SCRIPT)
        self.assertEqual(params["same"], "s")
        self.assertTrue(params["k"].startswith("\x00"), params)

    def test_an_internal_merge_whose_scripts_merge_file_refuses_gives_a_text_mark(self):
        # Re-check close-kit m1': with three or more merge bases an input of
        # the second internal merge can carry a mark already, and merge-file
        # refuses it; the internal merge ended with exit 1 and `%A` left at
        # one input. Now the virtual base's script is a plain text mark that
        # no side equals.
        base = cell(BASE_SCRIPT)
        ours = cell(BASE_SCRIPT + "\x00 a mark of an earlier internal merge\n", {"k": 1})
        theirs = cell(BASE_SCRIPT.replace('HEAD = "one"', 'HEAD = "theirs"'), {"k": 1})
        files = []
        for name, cfg in (("base", base), ("out", ours), ("other", theirs)):
            files.append(self.repo / f"{name}.json")
            files[-1].write_bytes(tree_form(cfg))
        done = driver(self.repo, "--inner", *map(str, files), CONFIG)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        params = json.loads(files[1].read_bytes())["params"]
        self.assertNotIn("\x00", params["script_inline"])
        self.assertNotIn(params["script_inline"], {BASE_SCRIPT, ours["params"]["script_inline"],
                                                    theirs["params"]["script_inline"]})
        self.assertEqual(params["k"], 1)

    def test_a_script_the_merge_bases_added_two_ways_merges_in_the_real_merge(self):
        # Re-check close-kit m1' (test gap): the m1 path through a real
        # criss-cross merge with git. cc-base has the config without a script,
        # the two merge bases add two different ones, both sides settle at
        # the same script and then change other keys: the real merge takes
        # the agreed script, exit 0. Settled two ways, it conflicts visibly.
        def bare(extra=None):
            return {"cell": "code", "params": {"runner": "python", **(extra or {})}}

        def head(value, extra=None):
            return cell(BASE_SCRIPT.replace('HEAD = "one"', f'HEAD = "{value}"'),
                        {"runner": "python", **(extra or {})})

        for agree in (True, False):
            with self.subTest(agree=agree):
                self.setUp()
                self.branch_commit("cc-base", bare())
                self.criss_cross(head("p"), head("q"), head("s"),
                                 head("s" if agree else "t"))
                self.commit_on("theirs", head("s" if agree else "t", {"y": 1}))
                self.commit_on("ours", head("s", {"x": 1}))
                merged = self.merge()
                said = merged.stdout + merged.stderr
                self.assertNotIn("Cannot merge binary files", said)
                if agree:
                    self.assertEqual(merged.returncode, 0, said)
                    params = self.config()["params"]
                    self.assertIn('HEAD = "s"', params["script_inline"])
                    self.assertEqual((params["x"], params["y"]), (1, 1))
                    self.assertEqual(self.status(), "", said)
                else:
                    self.assertNotEqual(merged.returncode, 0, said)
                    waiting = (self.repo / CONFLICT_FILE).read_text(encoding="utf-8")
                    self.assertIn('HEAD = "t"', waiting)
                    self.assertIn(f"UU {CONFIG}", self.status())

    def test_a_config_both_merge_bases_added_merges_in_the_real_merge(self):
        # Re-check close-kit m1', probe A1: cc-base has no such file, the two
        # merge bases add it two ways (an add/add, `%O` empty in the internal
        # merge), both sides settle at the same config: exit 0, that config.
        git(self.repo, "checkout", "-q", "-b", "cc-base", "master")
        git(self.repo, "rm", "-q", CONFIG)
        git(self.repo, "commit", "-qm", "cc-base without the config")

        def side(k, extra=None):
            return cell(BASE_SCRIPT, {"k": k, **(extra or {})})

        for name, cfg in (("p1", side(1)), ("q1", side(2))):
            git(self.repo, "checkout", "-q", "-b", name, "cc-base")
            self.write(cfg)
            git(self.repo, "add", CONFIG)       # a new file: `commit -a` skips it
            git(self.repo, "commit", "-qm", name)
        git(self.repo, "branch", "-q", "ours", "p1")
        git(self.repo, "branch", "-q", "theirs", "q1")
        self.merge_commit("ours", "q1", side(3))
        self.merge_commit("theirs", "p1", side(3))
        bases = git(self.repo, "merge-base", "--all", "ours", "theirs").stdout.split()
        self.assertEqual(len(bases), 2, "the history is no criss-cross")
        self.commit_on("theirs", side(3, {"y": 1}))
        self.commit_on("ours", side(3, {"x": 1}))
        merged = self.merge()
        said = merged.stdout + merged.stderr
        self.assertEqual(merged.returncode, 0, said)
        params = self.config()["params"]
        self.assertEqual((params["k"], params["x"], params["y"]), (3, 1, 1))
        self.assertEqual(self.status(), "", said)

    def test_a_field_conflict_leaves_a_file_and_blocks_resolve(self):
        # A field conflict of the final merge used to be one line on stderr and
        # a valid config with this side's value -- `git add` took it silently.
        # Now it leaves a file with base, ours and theirs per key, the message
        # names all three, and `--resolve` refuses while the file is open.
        long = "y" * 100
        ours = cell(BASE_SCRIPT.replace('HEAD = "one"', 'HEAD = "ours"'),
                    {"keep_recent": 5, "long": long + "ours"}, description="Ours says so.")
        theirs = cell(BASE_SCRIPT.replace('HEAD = "one"', 'HEAD = "theirs"'),
                      {"keep_recent": 7, "long": long + "theirs"},
                      description="Theirs says " + "x" * 200)
        self.branch_commit("key-base", cell(BASE_SCRIPT, {"keep_recent": 3,
                                                          "long": long + "base"}))
        self.branch_commit("ours", ours, "key-base")
        self.branch_commit("theirs", theirs, "key-base")
        merged = self.merge()
        said = merged.stdout + merged.stderr
        self.assertNotEqual(merged.returncode, 0)

        fields = self.repo / FIELDS_FILE
        self.assertTrue(fields.is_file(), f"no file for the field conflict:\n{said}")
        waiting = json.loads(fields.read_text(encoding="utf-8"))
        self.assertEqual(waiting["params.keep_recent"],
                         {"base": 3, "ours": 5, "theirs": 7})
        self.assertEqual(waiting["description"]["theirs"], "Theirs says " + "x" * 200)
        # The message names all three values, the long one shortened.
        self.assertIn("params.keep_recent: base 3, ours 5, theirs 7", said)
        self.assertIn("description: base \"The policy cell.\", ours \"Ours says so.\"", said)
        self.assertNotIn("x" * 200, said)
        # Three long values alike for their first 100 characters: each is
        # shown from shortly before the first difference.
        tail = "y" * 20
        self.assertIn(f'params.long: base ...{tail}base", ours ...{tail}ours", '
                      f'theirs ...{tail}theirs"', said)
        self.assertIn(FIELDS_FILE, said)

        # The script conflicted too: resolving it is refused while the fields wait.
        conflict = self.repo / CONFLICT_FILE
        text = conflict.read_text(encoding="utf-8")
        block = '<<<<<<< ours\nHEAD = "ours"\n=======\nHEAD = "theirs"\n>>>>>>> theirs\n'
        conflict.write_text(text.replace(block, 'HEAD = "both"\n'), encoding="utf-8")
        standing = self.raw()
        refused = driver(self.repo, "--resolve", CONFIG, CONFLICT_FILE)
        self.assertEqual(refused.returncode, 1, refused.stdout + refused.stderr)
        self.assertIn(FIELDS_FILE, refused.stderr)
        self.assertEqual(self.raw(), standing)
        self.assertTrue(conflict.is_file())

        # Fields set by hand, the file removed: the way back is open again.
        fields.unlink()
        done = driver(self.repo, "--resolve", CONFIG, CONFLICT_FILE)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertEqual(self.config()["params"]["script_inline"],
                         BASE_SCRIPT.replace('HEAD = "one"', 'HEAD = "both"'))

    def test_resolve_refuses_a_lone_separator_line(self):
        # mergedrv M1: with the two marks deleted by hand and the separator
        # forgotten, `--resolve` wrote `=======` into the script and removed
        # the conflict file -- a broken script and no evidence left.
        merged = self.same_line_conflict()
        self.assertNotEqual(merged.returncode, 0)
        conflict = self.repo / CONFLICT_FILE
        text = conflict.read_text(encoding="utf-8")
        half = text.replace("<<<<<<< ours\n", "").replace(">>>>>>> theirs\n", "")
        self.assertIn("\n=======\n", half)
        conflict.write_text(half, encoding="utf-8")
        standing = self.raw()
        refused = driver(self.repo, "--resolve", CONFIG, CONFLICT_FILE)
        self.assertEqual(refused.returncode, 1, refused.stdout + refused.stderr)
        self.assertIn("=======", refused.stderr)
        self.assertEqual(self.raw(), standing)
        self.assertTrue(conflict.is_file())

    # -- the way back from elsewhere, and its spelling (mergedrv M2, M3) ------

    def resolve_the_head_conflict(self, resolution='HEAD = "ours and theirs"\n'):
        conflict = self.repo / CONFLICT_FILE
        text = conflict.read_text(encoding="utf-8")
        block = '<<<<<<< ours\nHEAD = "ours"\n=======\nHEAD = "theirs"\n>>>>>>> theirs\n'
        self.assertIn(block, text)
        conflict.write_text(text.replace(block, resolution), encoding="utf-8")
        return conflict

    def test_resolve_from_another_directory_removes_the_conflict_file(self):
        # mergedrv M2: the conflict directory was taken relative to the working
        # directory, so `--resolve` with absolute paths from elsewhere wrote the
        # script back and left the file under target/.
        self.assertNotEqual(self.same_line_conflict().returncode, 0)
        conflict = self.resolve_the_head_conflict()
        done = subprocess.run(["python3", str(self.repo / DRIVER), "--resolve",
                               str(self.repo / CONFIG), str(conflict)],
                              cwd=self._tmp.name, env={**os.environ, **GIT_ENV},
                              capture_output=True, text=True)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertEqual(self.config()["params"]["script_inline"],
                         BASE_SCRIPT.replace('HEAD = "one"', 'HEAD = "ours and theirs"'))
        self.assertFalse(conflict.exists(), "the conflict file stayed behind")

    def test_resolve_keeps_the_spelling_of_the_three_sides(self):
        # mergedrv M3: all three sides spell characters outside ASCII as
        # escapes (like curator/intake), this side no longer carries one. The
        # spelling came from the standing config alone, so a resolved `ä` went
        # in as it is -- a clean merge would have escaped it.
        base = BASE_SCRIPT + 'NOTE = "ä"\n'
        self.branch_commit("esc-base", cell(base), ensure_ascii=True)
        self.branch_commit("ours", cell(base.replace('HEAD = "one"', 'HEAD = "ours"')
                                        .replace('NOTE = "ä"\n', "")),
                           "esc-base", ensure_ascii=True)
        self.branch_commit("theirs", cell(base.replace('HEAD = "one"', 'HEAD = "theirs"')),
                           "esc-base", ensure_ascii=True)
        self.assertNotEqual(self.merge().returncode, 0)
        self.assertTrue(self.raw().isascii())
        self.resolve_the_head_conflict('HEAD = "ä"\n')
        done = driver(self.repo, "--resolve", CONFIG, CONFLICT_FILE)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertTrue(self.raw().isascii(), "the resolved script left the escape spelling")
        self.assertEqual(self.config()["params"]["script_inline"],
                         BASE_SCRIPT.replace('HEAD = "one"', 'HEAD = "ä"'))

    # -- the driver called directly over three files (mergedrv M4-M6) ---------

    def run_driver(self, base, ours, theirs, env=None):
        """The driver over three sides, as git calls it: (process, result)."""
        files = []
        for name, side in (("base", base), ("ours", ours), ("theirs", theirs)):
            path = self.repo / f"{name}.json"
            path.write_bytes(side if isinstance(side, bytes) else tree_form(side))
            files.append(str(path))
        done = subprocess.run(["python3", DRIVER, *files, CONFIG], cwd=self.repo,
                              env=env or {**os.environ, **GIT_ENV},
                              capture_output=True, text=True)
        return done, (self.repo / "ours.json").read_bytes()

    def test_a_key_with_a_dot_is_named_unambiguously(self):
        # mergedrv M4 c: `{"a.b": ...}` and `{"a": {"b": ...}}` both read `a.b`.
        done, _ = self.run_driver(cell(BASE_SCRIPT, {"a.b": 1}), cell(BASE_SCRIPT, {"a.b": 2}),
                                  cell(BASE_SCRIPT, {"a.b": 3}))
        self.assertEqual(done.returncode, 1, done.stderr)
        self.assertIn('params["a.b"]: base 1, ours 2, theirs 3', done.stderr)

    def test_a_side_that_is_no_config_is_named_and_left_alone(self):
        # mergedrv M4 b + M6: invalid JSON, or a config without `script_inline`,
        # on one side -- the message says which side and what to do, the file
        # stays as git left it.
        good = cell(BASE_SCRIPT)
        for side, broken in (("theirs", b"{ not json"),
                             ("base", tree_form({"cell": "code", "params": {}}))):
            with self.subTest(side=side):
                sides = {"base": good, "ours": good, "theirs": good, side: broken}
                done, result = self.run_driver(sides["base"], sides["ours"], sides["theirs"])
                self.assertEqual(done.returncode, 1, done.stderr)
                self.assertIn(f"the {side} side of {CONFIG} is not a config with a script",
                              done.stderr)
                self.assertIn(f"git show :1:{CONFIG}", done.stderr)
                self.assertEqual(result, tree_form(good))

    def test_a_failing_merge_file_is_one_line_not_a_traceback(self):
        # mergedrv M5: `git merge-file` itself failing (killed, or a broken git)
        # ended as a Python traceback; the result was right, the message not.
        fake = self.repo / "fakebin"
        fake.mkdir()
        (fake / "git").write_text("#!/bin/sh\nkill -9 $$\n", encoding="utf-8")
        (fake / "git").chmod(0o755)
        ours = cell(BASE_SCRIPT.replace('HEAD = "one"', 'HEAD = "ours"'))
        env = {**os.environ, **GIT_ENV, "PATH": f"{fake}:{os.environ['PATH']}"}
        done, result = self.run_driver(cell(BASE_SCRIPT), ours, cell(BASE_SCRIPT), env)
        self.assertEqual(done.returncode, 1, done.stderr)
        self.assertNotIn("Traceback", done.stderr)
        self.assertIn("git merge-file failed", done.stderr)
        self.assertEqual(result, tree_form(ours))

    def test_the_edge_cases_of_the_key_merge(self):
        # mergedrv M6: behaviour after OR-58-61 no test held.
        obj = {"soft": 1, "hard": 2}
        base = cell(BASE_SCRIPT, {"gone": 1, "same": 1, "shape": obj, "fight": obj})
        ours = cell(BASE_SCRIPT, {"same": 2, "shape": 5, "fight": 5})
        theirs = cell(BASE_SCRIPT, {"same": 2, "shape": obj,
                                    "fight": {"soft": 9, "hard": 2}})
        done, result = self.run_driver(base, ours, theirs)
        got = json.loads(result)["params"]
        self.assertNotIn("gone", got)                    # deleted on both sides
        self.assertEqual(got["same"], 2)                 # the same change twice
        self.assertEqual(got["shape"], 5)                # an object made a value, one side
        # an object made a value here, changed inside there: a conflict, ours kept
        self.assertEqual(got["fight"], 5)
        self.assertEqual(done.returncode, 1, done.stderr)
        self.assertIn('params.fight: base {"soft": 1, "hard": 2}, ours 5, '
                      'theirs {"soft": 9, "hard": 2}', done.stderr)
        for path in ("params.gone", "params.same", "params.shape"):
            self.assertNotIn(path, done.stderr)


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


if __name__ == "__main__":
    unittest.main()
