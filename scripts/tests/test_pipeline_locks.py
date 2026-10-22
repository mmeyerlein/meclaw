"""Tests of the cheap locks (`scripts/pipeline/locks.py`, W4, GH #1118).

Every case builds a small tree in a temporary directory: a template with
its `template.json`, a scaffold that pins it, a set file -- and, where the
lock reads a base revision, a git repository with two commits.
"""

import argparse
import contextlib
import io
import json
import os
import shutil
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest import mock
from scripts.tests import private_tree, store_isolation

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS))

from pipeline import locks, versions  # noqa: E402
import precheck  # noqa: E402

SCAFFOLD = "workshop/evals/builder-scenarios/scaffolds/intake-min/main/os/config.json"


@contextlib.contextmanager
def spawned():
    """The argv of every `subprocess.run` inside the block (the work a check does)."""
    procs = []
    real = subprocess.run

    def run(argv, *a, **kw):
        procs.append([str(x) for x in argv])
        return real(argv, *a, **kw)

    with mock.patch.object(subprocess, "run", run):
        yield procs


# gate-selftest loads every module into one process: no fixture event of this
# module may reach the metrics target of the caller's environment.
_METRICS_URL = mock.patch.dict(os.environ, {"MECLAW_PIPELINE_METRICS_URL": ""})


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()
    _METRICS_URL.start()


def tearDownModule():  # noqa: N802 -- unittest's name
    _METRICS_URL.stop()
    store_isolation.restore()


def write(root, rel, text):
    path = os.path.join(root, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(text)


def template(root, name, version):
    write(root, "templates/%s/template.json" % name,
          json.dumps({"name": name, "version": version}, indent=2) + "\n")


def git(root, *args):
    subprocess.run(["git", "-C", root] + list(args), check=True,
                   capture_output=True, text=True)


def init_repo(root):
    git(root, "init", "-q", "-b", "master")
    git(root, "config", "user.email", "t@example.invalid")
    git(root, "config", "user.name", "t")
    git(root, "add", "-A")
    git(root, "commit", "-q", "-m", "base")
    return subprocess.run(["git", "-C", root, "rev-parse", "HEAD"], check=True,
                          capture_output=True, text=True).stdout.strip()


class Tree(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = self._tmp.name
        template(self.root, "meclaw-os", "2.2.44")

    def tearDown(self):
        self._tmp.cleanup()


class PinLock(Tree):
    def test_pin_lock_finds_unprovided_pin_with_line(self):
        write(self.root, "examples/demo/main/config.json",
              '{\n  "name": "demo",\n  "template": "meclaw-os@2.2.43"\n}\n')
        found = locks.check_pin_lock(["templates/meclaw-os/template.json"], self.root)
        self.assertEqual(1, len(found))
        f = found[0]
        self.assertEqual(("RED", "pin-lock", "examples/demo/main/config.json", 3),
                         (f.verdict, f.check, f.path, f.line))
        self.assertEqual("examples/demo/main/config.json:3 meclaw-os@2.2.43 not provided "
                         "-- raise to 2.2.44", locks.render(f))

    def test_pin_lock_is_green_on_a_provided_pin(self):
        # GH #1108: the counterpart of the orphan -- a scaffold and an example
        # pinning the version the template ships at raise nothing.
        write(self.root, "examples/demo/main/config.json",
              '{\n  "name": "demo",\n  "template": "meclaw-os@2.2.44"\n}\n')
        write(self.root, "workshop/scaffold/config.json",
              '{"cells": [{"template": "meclaw-os@2.2.44"}]}\n')
        self.assertEqual([], locks.check_pin_lock([], self.root))

    def test_pin_lock_accepts_since_sentence_shorthand_as_note(self):
        write(self.root, "docs/x.md", "Since `meclaw-os@2.2.40` the door is shut.\n")
        found = locks.check_pin_lock(["docs/x.md"], self.root)
        self.assertEqual(["NOTE"], [f.verdict for f in found])
        self.assertEqual(("docs/x.md", 1), (found[0].path, found[0].line))
        # against a base revision only the lines the diff adds are read:
        # the history a page already names is not this diff's finding
        base = init_repo(self.root)
        write(self.root, "docs/x.md", "Since `meclaw-os@2.2.40` the door is shut.\n\n"
              "Since `meclaw-os@2.2.41` it stays shut.\n")
        found = locks.check_pin_lock(["docs/x.md"], self.root, {"base": base})
        self.assertEqual([("docs/x.md", 3)], [(f.path, f.line) for f in found])

    def test_pins_check_lists_pins_to_raise(self):
        write(self.root, SCAFFOLD, '{\n  "template": "meclaw-os@2.2.43"\n}\n')
        write(self.root, "templates/x/main/config.json", '{"template": "meclaw-os@2.2.44"}\n')
        found = locks.pins_to_raise(self.root)
        self.assertEqual([(SCAFFOLD, 2)], [(f.path, f.line) for f in found])
        # the CLI form `pins --check` as `register` hangs it into a parser
        # (pipeline/cli.py after W0a): one PIN line per pin, exit 1 while one is left
        ap = argparse.ArgumentParser()
        locks.register(ap.add_subparsers())
        args = ap.parse_args(["pins", "--check", "--repo", self.root])
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            rc = args.func(args)
        lines = buf.getvalue().splitlines()
        self.assertEqual(1, rc)
        self.assertEqual("PIN %s:2 meclaw-os@2.2.43 not provided -- raise to 2.2.44" % SCAFFOLD,
                         lines[0])
        self.assertEqual("PINS 1 to raise", lines[-1])

    def test_fixture_0810_pin_after_bump_red_with_files_only(self):
        # The 08.10. case: the bump moved meclaw-os, a scaffold kept the old pin.
        # Its budget is work (VG TV-K3): the pin lock reads files and spawns
        # nothing; the whole precheck spawns git and nothing else (no cargo).
        write(self.root, SCAFFOLD, '{\n  "template": "meclaw-os@2.2.43"\n}\n')
        with spawned() as procs:
            self.assertEqual("RED", locks.check_scenario_pin_lock(
                ["templates/meclaw-os/template.json"], self.root)[0].verdict)
        self.assertEqual([], procs, "the pin lock spawned a process")
        with spawned() as procs:
            found = precheck.run(["templates/meclaw-os/template.json"], repo=self.root)
        self.assertIn("RED", [f.level for f in found if f.check == "scenario-pin-lock"])
        self.assertTrue(procs)
        self.assertEqual({"git"}, {p[0] for p in procs}, procs)


    def test_pin_lock_sees_an_escaped_pin_in_an_inlined_script(self):
        # GH #1169: a script inlined as a JSON string (`script_inline`) carries
        # its pins one level deeper, as `\"name@x.y.z\"`; the merge point's
        # cascade and this lock both missed it, the recipe kept rendering the
        # screen the bump had just retired (template_missing in five tests).
        write(self.root, "templates/x/recipes/config.json",
              '{\n  "script_inline": "S = {\\"screen\\": \\"meclaw-os@2.2.43\\"}"\n}\n')
        found = locks.check_pin_lock([], self.root)
        self.assertEqual([("RED", "templates/x/recipes/config.json", 2)],
                         [(f.verdict, f.path, f.line) for f in found])
        self.assertEqual("templates/x/recipes/config.json:2 meclaw-os@2.2.43 not provided "
                         "-- raise to 2.2.44", locks.render(found[0]))

    def test_pin_lock_net_finds_a_pin_position_the_bump_left_behind_in_any_quoting(self):
        # GH #1169, the net: once the diff numbers a template anew against its
        # base (the merge point's candidate), a pin POSITION `"<name>@<old>"` in
        # a file class the cascade owns is RED at any escape depth -- the tb-s8
        # case, a recipe whose inlined script pins the screen escaped, among
        # them; a Rust test's `json!` pin and a Markdown example too (B-2).
        template(self.root, "meclaw-os", "2.2.43")
        base = init_repo(self.root)
        template(self.root, "meclaw-os", "2.2.44")
        recipe = "templates/builder/recipes/config.json"
        write(self.root, recipe,
              '{\n  "script_inline": "S = {\\"screen\\": \\"meclaw-os@2.2.43\\"}",\n'
              '  "js": "const SCREEN = \'meclaw-os@2.2.43\';",\n'
              '  "deeper": "x = \\"y = \\\\\\"meclaw-os@2.2.43\\\\\\"\\"",\n'
              '  "meclaw-os@2.2.43": "a key"\n}\n')
        write(self.root, "workshop/evals/builder-scenarios/cases/c.json",
              '{"template": "meclaw-os@2.2.43"}\n')
        write(self.root, "crates/c/tests/grow.rs",
              'let m = json!({"template": "meclaw-os@2.2.43"});\n')
        write(self.root, "docs/how.md", '```json\n{"template": "meclaw-os@2.2.43"}\n```\n')
        diff = ["templates/meclaw-os/template.json"]
        found = locks.check_pin_lock(diff, self.root, {"base": base})
        # `'…'` (line 3) is no position: neither the cascade nor the net reads it
        self.assertEqual([("RED", "crates/c/tests/grow.rs", 1), ("RED", "docs/how.md", 2),
                          ("RED", recipe, 2), ("RED", recipe, 4), ("RED", recipe, 5),
                          ("RED", "workshop/evals/builder-scenarios/cases/c.json", 1)],
                         sorted((f.verdict, f.path, f.line) for f in found))
        net = sorted(locks.render(f) for f in locks.left_behind(diff, self.root, base))
        self.assertEqual(6, len(net))
        self.assertEqual("crates/c/tests/grow.rs:1 meclaw-os@2.2.43 left behind by the bump to "
                         "2.2.44 -- raise it to 2.2.44", net[0])
        # without a bump in the diff, or without a base, the net is silent
        self.assertEqual([], locks.left_behind([], self.root, base))
        self.assertEqual([], locks.left_behind(diff, self.root, None))

    def test_pin_lock_net_leaves_prose_fixtures_and_the_seed_alone(self):
        # B-1 (tb-s8 re-check): the net reads pin positions only. The old number
        # in a sentence -- inside a JSON string, a code span, a Rust message,
        # Markdown --, in a fixture outside the cascade's classes, in the seed
        # corpus and in the frozen plan archive is no position: green.
        template(self.root, "meclaw-os", "2.2.43")
        base = init_repo(self.root)
        template(self.root, "meclaw-os", "2.2.44")
        write(self.root, "templates/x/config.json", json.dumps({
            "template": "meclaw-os@2.2.44",
            "because": "Derived from meclaw-os@2.2.43\".",
            "use_when": "the template pins `meclaw-os@2.2.43`; no lane moved",
            "note": "one add_nodes with template 'meclaw-os@2.2.43', plus an edge"}, indent=2))
        write(self.root, "templates/x/README.md", "# `meclaw-os@2.2.43`\nRuns on meclaw-os@2.2.43.\n")
        write(self.root, "templates/builder-librarian/store/seed/docs.jsonl",
              '{"text": "\\"meclaw-os@2.2.43\\" pins it"}\n')
        write(self.root, "crates/c/tests/fixtures/edges.json",
              '{"template": "meclaw-os@2.2.43"}\n')
        write(self.root, "crates/c/tests/a.rs",
              "assert!(ok, \"2.5.27 only pins `meclaw-os@2.2.43`\");\n"
              ".expect(\"register meclaw-os@2.2.43\");\n")
        write(self.root, "tests/t.json", '{"template": "meclaw-os@2.2.43"}\n')
        write(self.root, "docs/superpowers/plans/old.json", '{"t": "meclaw-os@2.2.43"}\n')
        write(self.root, "docs/superpowers/plans/old.md", '"meclaw-os@2.2.43"\n')
        write(self.root, "workshop/evals/run.md", '"meclaw-os@2.2.43"\n')
        write(self.root, "templates/x/s.py", "SCREEN = 'meclaw-os@2.2.43'\n")
        diff = ["templates/meclaw-os/template.json"]
        self.assertEqual([], locks.left_behind(diff, self.root, base))
        # (the frozen archive's JSON is the stale-pin scan's own business, not the net's)
        self.assertEqual([], [f for f in locks.check_pin_lock(diff, self.root, {"base": base})
                              if f.verdict == "RED" and not f.path.startswith(locks.FROZEN)])

    def test_the_net_reads_the_classes_the_cascade_raises(self):
        # B-2: a class the cascade raises and the net skips is fail-open there.
        from pipeline import versions
        self.assertEqual({(".json", frozenset(versions.SURFACES)),
                          (".rs", frozenset(versions.CODE_SURFACES)),
                          (".md", frozenset(versions.PROSE_SURFACES))},
                         {(suffix, frozenset(tops)) for suffix, tops in locks.NET_CLASSES})
        self.assertEqual(versions.FROZEN, locks.FROZEN)


class PinReadersReadTheTreesOwnFiles(Tree):
    """GH #1199 (K19 17.10.): on build04 pin-lock read four ignored
    `workshop/workspace/builder-scenarios/*` an older run had left in the lane tree
    (`git clean` without -x keeps them) and called `member@2.5.33 not provided` after
    the bump to 2.6.0. A pin reader reads what git lists, an ignored rest stays where
    it is and is no pin of the tree."""

    def setUp(self):
        super().setUp()
        write(self.root, ".gitignore", "workshop/workspace/\nsets/local-*.json\n")
        write(self.root, "workshop/evals/kept.json", '{"template": "meclaw-os@2.2.44"}\n')
        init_repo(self.root)
        for rel in ("workshop/workspace/builder-scenarios/s1/config.json", "sets/local-x.json"):
            write(self.root, rel, '{"template": "meclaw-os@2.2.33"}\n')
        # an ignored template directory is no template of the tree either: it carries the
        # tree's name with an older version -- read, it would win (sorted, the last one)
        # and turn the tree's own pin red (re-check I1, mutant RC8)
        write(self.root, "templates/stale/template.json",
              json.dumps({"name": "meclaw-os", "version": "2.2.33"}, indent=2) + "\n")
        write(self.root, ".git/info/exclude", "templates/stale/\n")

    def test_an_ignored_rest_with_a_foreign_pin_is_green_and_stays(self):
        self.assertEqual([], locks.check_pin_lock([], self.root))
        self.assertEqual([], locks.check_scenario_pin_lock([], self.root))
        self.assertEqual({"meclaw-os": "2.2.44"}, locks.template_versions(self.root))
        self.assertEqual({"meclaw-os": "meclaw-os"}, versions.template_dirs(self.root))
        self.assertTrue(os.path.isfile(os.path.join(
            self.root, "workshop/workspace/builder-scenarios/s1/config.json")))

    def test_a_tracked_or_untracked_stale_pin_stays_red(self):
        write(self.root, "workshop/evals/kept.json", '{"template": "meclaw-os@2.2.43"}\n')
        write(self.root, "sets/new.json", '{"template": "meclaw-os@2.2.42"}\n')
        found = locks.check_pin_lock([], self.root)
        self.assertEqual([("RED", "sets/new.json"), ("RED", "workshop/evals/kept.json")],
                         sorted((f.verdict, f.path) for f in found))

    def test_the_cascade_leaves_an_ignored_rest_as_it_is(self):
        # re-check I1, mutant RC1: the cascade raises what git lists, an ignored rest
        # with the old pin stays as it is
        rest = "workshop/workspace/builder-scenarios/s2/config.json"
        write(self.root, rest, '{"template": "meclaw-os@2.2.44"}\n')
        changed = versions.cascade(self.root, {"meclaw-os": ("2.2.44", "2.2.45")})
        self.assertEqual(["workshop/evals/kept.json"], sorted({c["path"] for c in changed}))
        with open(os.path.join(self.root, "workshop/evals/kept.json"), encoding="utf-8") as fh:
            self.assertIn("meclaw-os@2.2.45", fh.read())
        with open(os.path.join(self.root, rest), encoding="utf-8") as fh:
            self.assertEqual('{"template": "meclaw-os@2.2.44"}\n', fh.read())

    def test_a_git_tree_git_cannot_list_is_red_never_a_walk(self):
        # re-check M5: a git error in a tree WITH .git (safe.directory, the 30-s
        # timeout) is RED with its reason -- no quiet fall back to the walk
        with mock.patch.object(locks, "_git", return_value=None):
            for check in (locks.check_pin_lock, locks.check_scenario_pin_lock,
                          locks.check_set_drift_lock, locks.check_pin_drift_lock,
                          locks.check_out_link_lock):
                with self.subTest(check=check.__name__):
                    found = check([], self.root)
                    self.assertEqual(["RED"], [f.verdict for f in found])
                    self.assertRegex(found[0].text, "git cannot list the tree")
                    self.assertIn("ls-files", found[0].fix)
            with self.assertRaisesRegex(locks.Unlisted, "git cannot list the tree"):
                versions.template_dirs(self.root)
        self.assertTrue(os.path.isfile(os.path.join(
            self.root, "workshop/workspace/builder-scenarios/s1/config.json")))

    def test_a_tree_without_git_is_walked_with_no_process(self):
        # no git repository at all (cascade_dry's throwaway copy, an unpacked export,
        # a fixture): the walk reads everything, and no process runs (VG TV-K3)
        other = os.path.join(self.root, "copy")
        shutil.copytree(self.root, other, ignore=shutil.ignore_patterns(".git", "copy"))
        with spawned() as procs:
            rels = locks.listed(other, "workshop")
            dirs = locks.template_dirs(other)
        self.assertEqual([], procs)
        self.assertIn("workshop/workspace/builder-scenarios/s1/config.json", rels)
        self.assertEqual(["meclaw-os", "stale"], dirs)


class ScenarioPinLock(Tree):
    def test_scenario_pin_lock_finds_stale_scaffold(self):
        write(self.root, SCAFFOLD, '{\n  "template": "meclaw-os@2.2.43"\n}\n')
        write(self.root, "workshop/evals/builder-scenarios/cases/A1.json",
              '{"pins": ["meclaw-os@2.2.44"]}\n')
        found = locks.check_scenario_pin_lock([], self.root)
        self.assertEqual([("RED", SCAFFOLD, 2)], [(f.verdict, f.path, f.line) for f in found])
        # the scenario tree is not graded twice: pin-lock leaves it alone
        self.assertEqual([], locks.check_pin_lock([], self.root))


class VersionLock(Tree):
    def _bumped(self):
        base = init_repo(self.root)
        template(self.root, "meclaw-os", "2.2.45")
        return base

    def test_version_lock_rejects_bump_in_strand(self):
        base = self._bumped()
        found = locks.check_version_lock(["templates/meclaw-os/template.json"], self.root,
                                         {"base": base, "mode": "strand"})
        self.assertEqual(1, len(found))
        self.assertEqual(("RED", "templates/meclaw-os/template.json", 3),
                         (found[0].verdict, found[0].path, found[0].line))
        self.assertIn("versions are assigned at the merge point", found[0].text)
        # passes carry their bumps on purpose
        self.assertEqual([], locks.check_version_lock(
            ["templates/meclaw-os/template.json"], self.root, {"base": base, "mode": "release"}))

    def test_version_lock_allow_bump_records_reason(self):
        base = self._bumped()
        found = locks.check_version_lock(["templates/meclaw-os/template.json"], self.root,
                                         {"base": base, "mode": "pre",
                                          "allow_bump": "merge point R7"})
        self.assertEqual(["NOTE"], [f.verdict for f in found])
        self.assertIn("merge point R7", found[0].text)

    def test_version_lock_reads_workspace_version(self):
        write(self.root, "Cargo.toml", '[workspace]\nmembers = []\n\n[workspace.package]\n'
              'version = "0.63.0"\nedition = "2021"\n')
        base = init_repo(self.root)
        write(self.root, "Cargo.toml", '[workspace]\nmembers = []\n\n[workspace.package]\n'
              'version = "0.63.1"\nedition = "2021"\n')
        found = locks.check_version_lock(["Cargo.toml"], self.root, {"base": base})
        self.assertEqual([("RED", "Cargo.toml", 5)], [(f.verdict, f.path, f.line) for f in found])


class TestLineLock(Tree):
    def test_test_line_lock_notes_promise_without_test(self):
        found = locks.check_test_line_lock(["docs/stability.en.md", "CHANGELOG.md"], self.root)
        self.assertEqual(["NOTE", "NOTE"], [f.verdict for f in found])
        self.assertTrue(all("no test moved with the promise" in f.text for f in found))
        self.assertEqual([], locks.check_test_line_lock(
            ["CHANGELOG.md", "crates/meclaw-cells/tests/gh1_x.rs"], self.root))

    def test_a_template_scenario_driver_is_a_test(self):
        # A display scenario driver pins template promises through test 710 (GH #1169).
        self.assertEqual([], locks.check_test_line_lock(
            ["CHANGELOG.md",
             "templates/display/compose/scenarios/run_display_scenarios.py"], self.root))

    def test_a_template_scenario_module_is_no_test(self):
        # Only a driver `run_*.py` runs scenarios; the model it drives is no test.
        found = locks.check_test_line_lock(
            ["CHANGELOG.md", "templates/display/compose/scenarios/pass.py"], self.root)
        self.assertEqual(["NOTE"], [f.verdict for f in found])


class SetDriftLock(Tree):
    def test_set_drift_lock_reads_local_pins(self):
        write(self.root, "sets/0.21.json",
              '{\n  "core": "0.63.0",\n  "local_pins": {\n    "meclaw-os": "2.2.43"\n  }\n}\n')
        found = locks.check_set_drift_lock([], self.root)
        self.assertEqual([("RED", "sets/0.21.json", 4)],
                         [(f.verdict, f.path, f.line) for f in found])
        self.assertEqual([], locks.check_set_drift_lock(
            [], self.root, {"core_versions": {"meclaw-os": "2.2.43"}}))


class PinDriftLock(Tree):
    """`pin-drift-lock` (W10): a set's sources against the source repository."""

    def source(self, home):
        src = os.path.join(home, "src-orga")
        template(src, "orga", "0.11.1")
        template(src, "orga-link", "1.1.0")
        init_repo(src)
        git(src, "tag", "orga-0.11.1")
        tagged = subprocess.run(["git", "-C", src, "rev-parse", "HEAD"], check=True,
                                capture_output=True, text=True).stdout.strip()
        template(src, "orga", "0.12.0")
        git(src, "commit", "-qam", "bump")
        return tagged

    def set_file(self, source, local_pins, commits=None, name="0.2"):
        doc = {"set": "coding-" + name, "sources": {"orga": source},
               "local_pins": local_pins}
        if commits is not None:
            doc["source_commits"] = commits
        write(self.root, "sets/%s.json" % name, json.dumps(doc, indent=2) + "\n")

    def test_pin_drift_lock_reports_tag_and_commit_forms(self):
        home = os.path.join(self.root, "home")
        tagged = self.source(home)
        with mock.patch.dict(os.environ, {"HOME": home}):
            # in step with the tag: nothing to say
            self.set_file({"repo": "~/src-orga", "tag": "orga-0.11.1",
                           "templates": ["orga", "orga-link"]},
                          {"orga": "0.11.1", "orga-link": "1.1.0"}, {"orga": tagged})
            self.assertEqual([], locks.check_pin_drift_lock([], self.root))
            # a local pin moved by hand: RED with path, line and the tag form
            self.set_file({"repo": "~/src-orga", "tag": "orga-0.11.1",
                           "templates": ["orga", "orga-link"]},
                          {"orga": "0.11.2", "orga-link": "1.1.0"}, {"orga": tagged})
            # the budget is work (VG TV-K3): one rev-parse of the tag, one
            # `git show` per template of the source -- no fetch, no clone
            with spawned() as procs:
                found = locks.check_pin_drift_lock([], self.root)
            self.assertEqual([["git", "rev-parse"], ["git", "show"], ["git", "show"]],
                             [[p[0], p[3]] for p in procs], procs)
            self.assertEqual([("RED", "pin-drift-lock", "sets/0.2.json")],
                             [(f.verdict, f.check, f.path) for f in found])
            self.assertIn("orga@0.11.2", found[0].text)
            self.assertIn("tag orga-0.11.1 ships 0.11.1", found[0].text)
            self.assertEqual(14, found[0].line)
            # the commit form: a commit whose template moved on, and a stale source_commits
            self.set_file({"repo": "~/src-orga", "commit": "HEAD", "templates": ["orga"]},
                          {"orga": "0.11.1"}, {"orga": tagged})
            found = locks.check_pin_drift_lock([], self.root)
            self.assertEqual(["RED", "RED"], [f.verdict for f in found])
            self.assertIn("commit HEAD resolves to", found[0].text)
            self.assertIn("commit HEAD ships 0.12.0", found[1].text)
            # a source with its templates under another directory (`dir`)
            self.set_file({"repo": "~/src-orga", "commit": tagged, "dir": "apps",
                           "templates": ["orga"]}, {"orga": "0.11.1"})
            found = locks.check_pin_drift_lock([], self.root)
            self.assertEqual(["RED"], [f.verdict for f in found])
            self.assertIn("has no apps/orga/template.json", found[0].text)
            # fail-closed (review I1, OR-DT.ORCH.19): a pin that cannot be
            # checked is RED with the fetch hint, always -- a lane in the
            # context does not grade it down
            lane = {"lane": "lane-a"}
            self.assertEqual(["RED"], [f.verdict for f in
                                       locks.check_pin_drift_lock([], self.root, lane)])
            self.set_file({"repo": "~/nowhere", "tag": "orga-0.11.1", "templates": ["orga"]},
                          {"orga": "0.11.1"})
            found = locks.check_pin_drift_lock([], self.root)
            self.assertEqual(["RED"], [f.verdict for f in found])
            self.assertIn("fetch", found[0].fix)
            self.assertEqual(["RED"], [f.verdict for f in
                                       locks.check_pin_drift_lock([], self.root, lane)])
            self.set_file({"repo": "~/src-orga", "tag": "orga-9.9.9", "templates": ["orga"]},
                          {"orga": "0.11.1"})
            found = locks.check_pin_drift_lock([], self.root, {"mode": "pre"})
            self.assertEqual(["RED"], [f.verdict for f in found])
            self.assertIn("orga-9.9.9", found[0].text)
            self.assertIn("fetch", found[0].fix)
            self.assertEqual(["RED"], [f.verdict for f in
                                       locks.check_pin_drift_lock([], self.root, lane)])
            # a git error or timeout reads like an unknown revision: RED, not skipped
            self.set_file({"repo": "~/src-orga", "tag": "orga-0.11.1", "templates": ["orga"]},
                          {"orga": "0.11.1"}, {"orga": tagged})
            with mock.patch.object(locks, "_git", return_value=None):
                found = locks.check_pin_drift_lock([], self.root)
            self.assertEqual(["RED"], [f.verdict for f in found])
            # a source that is not an object is RED, lane or not
            self.set_file("~/src-orga", {"orga": "0.11.1"})
            for ctx in (None, lane):
                found = locks.check_pin_drift_lock([], self.root, ctx)
                self.assertEqual(["RED"], [f.verdict for f in found])
                self.assertIn("source orga is not an object", found[0].text)
        # pre runs it beside the other locks
        self.assertIn(("pin-drift-lock", locks.check_pin_drift_lock), locks.LOCKS)

    def test_merge_base_falls_back_to_main_for_satellites(self):
        write(self.root, "a.txt", "a\n")
        subprocess.run(["git", "-C", self.root, "init", "-q", "-b", "main"], check=True)
        git(self.root, "config", "user.email", "t@example.invalid")
        git(self.root, "config", "user.name", "t")
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", "base")
        base = subprocess.run(["git", "-C", self.root, "rev-parse", "HEAD"], check=True,
                              capture_output=True, text=True).stdout.strip()
        git(self.root, "checkout", "-q", "-b", "strand")
        write(self.root, "b.txt", "b\n")
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", "change")
        self.assertEqual(base, locks.merge_base(self.root))
        self.assertIsNone(locks.merge_base(self.root, "master"))

    def test_merge_base_knows_origin_like_register_main_branch(self):
        # a satellite clone that only has origin/main (no local main): the
        # same order as register.main_branch -- local first, then origin
        write(self.root, "a.txt", "a\n")
        subprocess.run(["git", "-C", self.root, "init", "-q", "-b", "main"], check=True)
        git(self.root, "config", "user.email", "t@example.invalid")
        git(self.root, "config", "user.name", "t")
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", "base")
        base = subprocess.run(["git", "-C", self.root, "rev-parse", "HEAD"], check=True,
                              capture_output=True, text=True).stdout.strip()
        git(self.root, "update-ref", "refs/remotes/origin/main", base)
        git(self.root, "checkout", "-q", "-b", "strand")
        git(self.root, "branch", "-q", "-D", "main")
        write(self.root, "b.txt", "b\n")
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", "change")
        self.assertEqual(base, locks.merge_base(self.root))


class DiffSize(Tree):
    def test_diff_size_note_ignores_moves_and_generated_files(self):
        write(self.root, "a/big.txt", "".join("line %d\n" % i for i in range(600)))
        base = init_repo(self.root)
        os.makedirs(os.path.join(self.root, "b"))
        git(self.root, "mv", "a/big.txt", "b/big.txt")
        write(self.root, "templates/x/store/seed/docs.jsonl",
              "".join('{"n": %d}\n' % i for i in range(900)))
        git(self.root, "add", "-A")
        self.assertEqual([], locks.check_diff_size([], self.root, {"base": base}))
        write(self.root, "c.txt", "".join("x%d\n" % i for i in range(401)))
        found = locks.check_diff_size([], self.root, {"base": base})
        self.assertEqual(["NOTE"], [f.verdict for f in found])
        self.assertIn("large diff 401 lines", found[0].text)


class SameLockTwoCallers(Tree):
    def test_precheck_station_uses_same_locks(self):
        init_repo(self.root)
        write(self.root, "examples/demo/main/config.json", '{"template": "meclaw-os@2.2.43"}\n')
        lock_line = locks.render(locks.check_pin_lock([], self.root)[0])
        station = [f for f in precheck.run(["examples/demo/main/config.json"], repo=self.root)
                   if f.check == "pin-lock"]
        self.assertEqual(["RED"], [f.level for f in station])
        self.assertEqual(lock_line, station[0].message)
        from pipeline import pre
        lines = []
        pre.run_pre(self.root, base=None, out=lines.append)
        self.assertIn("PRE pin-lock 0s RED " + lock_line, lines)

    def test_precheck_lane_context_is_explicit(self):
        # OR-DT.ORCH.19: no environment variable grades an unchecked pin
        # down. The executor inherits the caller's environment, so a stray
        # `MECLAW_GATE_LANE=1` would otherwise turn a RED into a NOTE.
        init_repo(self.root)
        write(self.root, "sets/0.2.json", json.dumps(
            {"set": "coding-0.2", "local_pins": {"orga": "0.11.1"},
             "sources": {"orga": {"repo": "~/nowhere", "tag": "orga-0.11.1",
                                  "templates": ["orga"]}}}, indent=2) + "\n")
        env = {k: v for k, v in os.environ.items() if k != "MECLAW_GATE_LANE"}
        for lane in (None, "1"):
            if lane:
                env["MECLAW_GATE_LANE"] = lane
            with mock.patch.dict(os.environ, env, clear=True):
                station = [f for f in precheck.run(["sets/0.2.json"], repo=self.root)
                           if f.check == "pin-drift-lock"]
            self.assertEqual(["RED"], [f.level for f in station])

class TheHarnessFoldLock(unittest.TestCase):
    """`harness_fold --check` as a lock of `pre` and `precheck` (W5 delta I2):
    a new flat file or a drifted fold is RED, a tree that is the plan is quiet."""

    def _tree(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        tests = pathlib.Path(tmp.name, "crates", "fx", "tests")
        tests.mkdir(parents=True)
        pathlib.Path(tmp.name, "crates", "fx", "Cargo.toml").write_text(
            '[package]\nname = "fx"\nversion = "0.0.0"\n')
        (tests / "HARNESS.toml").write_text(
            'max_files = 60\nkeep = []\n[[rule]]\nmatch = "^alpha_"\nharness = "alpha"\n')
        (tests / "alpha_one.rs").write_text("#[test]\nfn one() {}\n")
        (tests / "alpha_two.rs").write_text("#[test]\nfn two() {}\n")
        return tmp.name, tests

    def test_harness_fold_lock_is_red_on_a_tree_that_is_not_the_plan(self):
        import harness_fold
        tree, tests = self._tree()
        diff = ["crates/fx/tests/alpha_one.rs"]
        self.assertTrue(any(f.verdict == "RED" and f.check == "harness-fold-lock"
                            for f in locks.check_harness_fold_lock(diff, tree)))
        harness_fold.main(["--crate", "fx", "--repo", tree, "--apply", "--no-git"])
        self.assertEqual(locks.check_harness_fold_lock(diff, tree), [])
        (tests / "alpha_three.rs").write_text("#[test]\nfn t() {}\n")  # a new flat file
        red = locks.check_harness_fold_lock(["crates/fx/tests/alpha_three.rs"], tree)
        self.assertTrue(red and all(f.verdict == "RED" for f in red))
        self.assertIn("harness-fold-lock", [n for n, _ in locks.LOCKS])

    def test_harness_fold_lock_runs_when_the_fold_tables_in_gate_plan_move(self):
        """#1134: the fold reads its scenario and browser keeps from gate_plan.py."""
        tree, _ = self._tree()
        self.assertTrue(any(f.check == "harness-fold-lock"
                            for f in locks.check_harness_fold_lock(["scripts/gate_plan.py"], tree)))

    def test_harness_fold_lock_is_silent_without_a_test_path(self):
        tree, _ = self._tree()
        self.assertEqual(locks.check_harness_fold_lock(["docs/x.md"], tree), [])


class TheDeadPathLock(Tree):
    """`harness_fold.dead_path_mentions` as a lock of `pre` and `precheck`
    (W5 re-check R3-C1): it runs on EVERY diff -- a doc names a test path, the
    path moves in a diff that never touches the doc -- and a dead mention is RED."""

    def test_dead_path_lock_runs_in_pre_and_precheck_on_any_diff(self):
        init_repo(self.root)
        write(self.root, "templates/meclaw-os/README.md",
              "Pinned by `crates/fx/tests/gone.rs`.\n")
        self.assertIn("dead-path-lock", [n for n, _ in locks.LOCKS])
        red = locks.check_dead_path_lock(["scripts/x.py"], self.root)
        self.assertEqual(["RED"], [f.verdict for f in red])
        self.assertIn("crates/fx/tests/gone.rs", locks.render(red[0]))
        station = [f for f in precheck.run(["scripts/x.py"], repo=self.root)
                   if f.check == "dead-path-lock"]
        self.assertEqual(["RED"], [f.level for f in station])
        from pipeline import pre
        lines = []
        pre.run_pre(self.root, base=None, out=lines.append)
        self.assertTrue(any(ln.startswith("PRE dead-path-lock ") and " RED " in ln
                            for ln in lines), lines)

    def test_dead_path_lock_sees_a_template_config_json(self):
        write(self.root, "templates/meclaw-os/config.json",
              '{"_pinned_by": "crates/fx/tests/gone.rs"}\n')
        red = locks.check_dead_path_lock([], self.root)
        self.assertEqual([("RED", "templates/meclaw-os/config.json")],
                         [(f.verdict, f.path) for f in red])

    def test_dead_path_lock_is_quiet_on_a_live_mention(self):
        write(self.root, "crates/fx/tests/here.rs", "#[test]\nfn t() {}\n")
        write(self.root, "templates/meclaw-os/README.md",
              "Pinned by `crates/fx/tests/here.rs`.\n")
        self.assertEqual(locks.check_dead_path_lock([], self.root), [])

    def test_a_stale_frozen_entry_is_red_on_a_docs_only_diff(self):
        """R3 M1: the doc of a frozen entry lost its dead mention -- RED in the lock."""
        from scripts import harness_fold
        (doc, path), n = next(iter(harness_fold.FROZEN_HISTORICAL.items()))
        write(self.root, "crates/fx/tests/here.rs", "#[test]\nfn t() {}\n")
        write(self.root, doc, "No mention left.\n")
        red = locks.check_dead_path_lock([doc], self.root)
        self.assertEqual(["RED"], [f.verdict for f in red])
        self.assertIn("FROZEN_HISTORICAL excuses %d mention(s) of %s" % (n, path),
                      locks.render(red[0]))

    def test_dead_path_lock_is_red_in_a_core_tree_without_templates_and_docs(self):
        """W5 re-check 4 M2: nothing to scan in a tree with crates/ is no green."""
        write(self.root, "crates/fx/tests/here.rs", "#[test]\nfn t() {}\n")
        for top in ("templates", "docs"):
            shutil.rmtree(os.path.join(self.root, top), ignore_errors=True)
        red = locks.check_dead_path_lock([], self.root)
        self.assertEqual(["RED"], [f.verdict for f in red])
        self.assertIn("neither templates/ nor docs/", locks.render(red[0]))

    def test_dead_path_lock_stays_quiet_in_a_tree_without_crates(self):
        write(self.root, "README.md", "a satellite\n")
        for top in ("templates", "docs", "crates"):
            shutil.rmtree(os.path.join(self.root, top), ignore_errors=True)
        self.assertEqual(locks.check_dead_path_lock([], self.root), [])

    def test_a_consumer_tree_without_crates_resolves_against_the_kits_core(self):
        """GH #1291: a consumer repository's frozen derived templates name the
        core's pin tests; without crates/ of its own they resolve against the
        meclaw-core tree the kit comes from, and only a path dead there too is RED."""
        kit = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(locks.__file__))))
        live = next(p for p in sorted(pathlib.Path(kit, "crates").glob("*/tests/*/*.rs")))
        live = live.relative_to(kit).as_posix()
        shutil.rmtree(os.path.join(self.root, "crates"), ignore_errors=True)
        write(self.root, "templates/local/t/README.md",
              "Pinned by `%s` and `crates/fx/tests/gone.rs`.\n" % live)
        red = locks.check_dead_path_lock([], self.root)
        self.assertEqual([("RED", "templates/local/t/README.md")], [(f.verdict, f.path) for f in red])
        self.assertIn("crates/fx/tests/gone.rs", locks.render(red[0]))

    def test_dead_path_lock_is_red_on_an_unreadable_directory(self):
        """W5 re-check 4 M2: rglob skipped a chmod-000 directory silently."""
        if os.geteuid() == 0:
            self.skipTest("root reads every directory")
        write(self.root, "crates/fx/tests/here.rs", "#[test]\nfn t() {}\n")
        write(self.root, "docs/sub/a.md", "Pinned by `crates/fx/tests/gone.rs`.\n")
        sub = os.path.join(self.root, "docs", "sub")
        os.chmod(sub, 0)
        try:
            red = locks.check_dead_path_lock([], self.root)
        finally:
            os.chmod(sub, 0o755)
        self.assertEqual(["RED"], [f.verdict for f in red])
        self.assertIn("cannot be read", locks.render(red[0]))

    def test_dead_path_lock_names_a_dangling_symlink_not_a_permission(self):
        """Re-check Z-D R1 M1: a dangling symlink got the hint "make the
        directory readable"; the hint names the link and its missing target."""
        write(self.root, "crates/fx/tests/here.rs", "#[test]\nfn t() {}\n")
        write(self.root, "docs/a.md", "Pinned by `crates/fx/tests/here.rs`.\n")
        os.symlink(os.path.join(self.root, "nowhere.md"), os.path.join(self.root, "docs", "b.md"))
        red = locks.check_dead_path_lock([], self.root)
        self.assertEqual(["RED"], [f.verdict for f in red])
        text = locks.render(red[0])
        self.assertIn("dangling symlink", text)
        self.assertNotIn("make the directory readable", text)

    def test_dead_path_lock_counts_what_it_could_not_read(self):
        """A file that is not UTF-8 is not read; the lock says so in a NOTE
        with the count instead of passing it over in silence (review Z-D M1)."""
        path = pathlib.Path(self.root, "templates/meclaw-os/blob.bin")
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"\xff\xfe crates/fx/tests/gone.rs")
        found = locks.check_dead_path_lock([], self.root)
        self.assertEqual([("NOTE", "templates/meclaw-os/blob.bin")],
                         [(f.verdict, f.path) for f in found])
        self.assertIn("1 file not UTF-8", locks.render(found[0]))


class TheOutLinkLock(Tree):
    """OR-TB.S10.Z.4 fail closed (re-check post-xhigh-8, M2): a symlink outside
    `plans/` that resolves out of the tree is RED in `pre` and `precheck`, on any
    diff. `pre.tree_hash` hashes a link as `link:<target>`, so content behind it
    moved no stamp; `turnaround.stamp_aliases` only saw it when a report changed."""

    def setUp(self):
        super().setUp()
        self._out = tempfile.TemporaryDirectory()
        self.out = self._out.name
        write(self.out, "vendored.txt", "outside\n")

    def tearDown(self):
        self._out.cleanup()
        super().tearDown()

    def link(self, rel, to):
        path = os.path.join(self.root, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        os.symlink(to, path)

    def found(self):
        return [(f.verdict, f.path) for f in locks.check_out_link_lock([], self.root)]

    def test_a_link_leaving_the_tree_outside_plans_is_red_in_pre_and_precheck(self):
        self.link("templates/a/vendored", self.out)
        init_repo(self.root)
        write(self.root, "docs/x.md", "# x\n")
        station = [f for f in precheck.run(["docs/x.md"], repo=self.root)
                   if f.check == "out-link-lock"]
        self.assertEqual(["RED"], [f.level for f in station])
        self.assertIn("templates/a/vendored is a symlink out of the tree", station[0].message)
        from pipeline import pre
        lines = []
        doc = pre.run_pre(self.root, base=None, out=lines.append)
        self.assertEqual("RED", doc["verdict"])
        self.assertEqual(1, sum(1 for l in lines
                                if l.startswith("PRE out-link-lock ") and " RED templates/a/vendored "
                                in l), lines)

    def test_links_under_plans_leaving_the_tree_stay_allowed(self):
        # master: four links under plans/welle-loop-2026-10-03/ into the coding repo
        self.link("plans/welle-loop/berichte", self.out)
        self.link("plans/welle-loop/ledger.md", os.path.join(self.out, "vendored.txt"))
        init_repo(self.root)
        self.assertEqual([], self.found())
        from pipeline import pre
        lines = []
        pre.run_pre(self.root, base=None, out=lines.append)
        self.assertIn("PRE out-link-lock 0s GREEN", [l.split(" -- ")[0] for l in lines])

    def test_a_link_inside_the_tree_is_no_finding(self):
        self.link("docs/alias.md", "../templates/meclaw-os/template.json")
        self.link("docs/dir", "../templates")
        init_repo(self.root)
        self.assertEqual([], self.found())

    def test_a_chain_out_through_plans_is_red_where_it_starts(self):
        self.link("plans/w/out", self.out)
        self.link("docs/hop", "../plans/w/out")
        init_repo(self.root)
        self.assertEqual([("RED", "docs/hop")], self.found())

    def test_a_prefix_that_only_looks_like_plans_is_no_exemption(self):
        self.link("plansx/out", self.out)
        self.link("docs/plans/out", self.out)
        init_repo(self.root)
        self.assertEqual([("RED", "docs/plans/out"), ("RED", "plansx/out")], self.found())

    def test_an_untracked_and_a_dangling_link_count_an_ignored_one_does_not(self):
        init_repo(self.root)
        write(self.root, ".gitignore", ".env\n")
        self.link(".env", os.path.join(self.out, "main.env"))  # the gate's linked .env
        self.link("templates/new", self.out)
        self.link("templates/gone", os.path.join(self.out, "absent"))
        self.assertEqual([("RED", "templates/gone"), ("RED", "templates/new")], self.found())

    def test_without_git_the_walk_fails_closed(self):
        self.link("templates/a/vendored", self.out)
        self.assertEqual([("RED", "templates/a/vendored")], self.found())

    def test_the_real_tree_holds_no_out_link_outside_plans(self):
        real = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
        self.assertEqual([], [locks.render(f) for f in locks.check_out_link_lock([], real)])


class TheFeatureUnityLock(unittest.TestCase):
    """One feature set per shared third-party dependency (tb-s6c, GH #1168):
    no member declares a shared dependency with features of its own, so the
    measured `-p` sets that hold the CLI resolve like the workspace (cargo
    tree). Building the station's two sets of the S6 case on a workspace-warm
    target took 141.4 s with member-local lists and 45.6 s with one set per
    dependency (`plans/welle-tempo-beweis-2026-10-10/berichte/tb-s6c.md`)."""

    ROOT = ('[workspace]\nresolver = "3"\nmembers = ["crates/a", "crates/b"]\n'
            '[workspace.package]\nversion = "0.1.0"\n')

    def _tree(self, root=None, a="", b="", ws_deps=""):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        text = (self.ROOT if root is None else root)
        if ws_deps:
            text += "[workspace.dependencies]\n" + ws_deps
        write(tmp.name, "Cargo.toml", text)
        for name, deps in (("a", a), ("b", b)):
            write(tmp.name, "crates/%s/Cargo.toml" % name,
                  '[package]\nname = "%s"\nversion.workspace = true\n%s' % (name, deps))
        return tmp.name

    def _red(self, tree, diff=("Cargo.toml",)):
        found = locks.check_feature_unity_lock(list(diff), tree)
        self.assertTrue(found, "expected a finding")
        self.assertTrue(all(f.verdict == "RED" and f.check == "feature-unity-lock"
                            for f in found), [locks.render(f) for f in found])
        return [locks.render(f) for f in found]

    # tb-s10 point 18 (re-check tb-s6c L-9): a crate ONE member declares with
    # features of its own, reachable in Cargo.lock from a shared dependency,
    # forces a second variant of it in every `-p` set without that member.
    LOCK = ('version = 4\n[[package]]\nname = "s"\nversion = "1.0.0"\n'
            'dependencies = ["c"]\n[[package]]\nname = "c"\nversion = "1.0.0"\n')
    SHARED = '[workspace.dependencies]\ns = "1"\n'

    def _transitive(self, a_extra):
        tree = self._tree(root=self.ROOT + self.SHARED,
                          a="[dependencies]\ns = { workspace = true }\n" + a_extra,
                          b="[dependencies]\ns = { workspace = true }\n")
        write(tree, "Cargo.lock", self.LOCK)
        return tree

    # Ruling OR-TV-1/OR-TV-4 (VG TV-W1): tokio `test-util` is the one
    # member-local feature the lock allows, and only where no release build
    # resolves it -- [dev-dependencies], or the dev-only crate meclaw-testing.
    TOKIO = '[workspace.dependencies]\ntokio = { version = "1", features = ["rt"] }\n'

    def _tokio(self, a_deps, a_name="a"):
        tree = self._tree(root=self.ROOT + self.TOKIO, a=a_deps,
                          b="[dependencies]\ntokio = { workspace = true }\n")
        if a_name != "a":
            write(tree, "crates/a/Cargo.toml", '[package]\nname = "%s"\nversion.workspace = true\n%s'
                  % (a_name, a_deps))
        return tree

    def test_tokio_test_util_in_dev_deps_is_allowed(self):
        tree = self._tokio('[dev-dependencies]\n'
                           'tokio = { workspace = true, features = ["test-util"] }\n')
        self.assertEqual(locks.check_feature_unity_lock(["crates/a/Cargo.toml"], tree), [])

    def test_tokio_test_util_in_normal_deps_is_red(self):
        deps = '[dependencies]\ntokio = { workspace = true, features = ["test-util"] }\n'
        found = self._red(self._tokio(deps), diff=("crates/a/Cargo.toml",))
        self.assertTrue(any("tokio" in f for f in found), found)
        # the one exception: meclaw-testing is never a runtime dependency (AGENTS rule 6)
        self.assertEqual(locks.check_feature_unity_lock(
            ["crates/a/Cargo.toml"], self._tokio(deps, a_name="meclaw-testing")), [])

    def test_another_tokio_feature_in_dev_deps_stays_red(self):
        for feats in ('["fs"]', '["test-util", "fs"]'):
            tree = self._tokio('[dev-dependencies]\n'
                               'tokio = { workspace = true, features = %s }\n' % feats)
            self._red(tree, diff=("crates/a/Cargo.toml",))
        tree = self._tokio('[dev-dependencies]\n'
                           'tokio = { workspace = true, features = ["test-util"], '
                           'default-features = false }\n')
        self._red(tree, diff=("crates/a/Cargo.toml",))
        tree = self._tree(root=self.ROOT + '[workspace.dependencies]\nserde = "1"\n',
                          a='[dev-dependencies]\nserde = { workspace = true, features = ["test-util"] }\n',
                          b="[dependencies]\nserde = { workspace = true }\n")
        self._red(tree, diff=("crates/a/Cargo.toml",))

    def test_a_single_declaration_with_features_under_a_shared_crate_is_red(self):
        tree = self._transitive('c = { version = "1", features = ["x"] }\n')
        found = locks.check_feature_unity_lock(["crates/a/Cargo.toml"], tree)
        self.assertEqual([(f.verdict, f.path) for f in found], [("RED", "crates/a/Cargo.toml")])
        self.assertIn("c", found[0].text)
        self.assertIn("s", found[0].text)

    def test_a_feature_entry_on_a_transitive_crate_is_red(self):
        tree = self._transitive('c = "1"\n[features]\nfx = ["c/x"]\n')
        found = locks.check_feature_unity_lock(["crates/a/Cargo.toml"], tree)
        self.assertEqual([f.verdict for f in found], ["RED"])

    def test_a_single_declaration_without_features_or_off_the_graph_stays_green(self):
        self.assertEqual(locks.check_feature_unity_lock(
            ["crates/a/Cargo.toml"], self._transitive('c = "1"\n')), [])
        tree = self._transitive('d = { version = "1", features = ["x"] }\n')
        self.assertEqual(locks.check_feature_unity_lock(["crates/a/Cargo.toml"], tree), [])

    def test_feature_unity_lock_is_registered(self):
        self.assertIn("feature-unity-lock", [n for n, _ in locks.LOCKS])

    def test_two_members_with_different_features_are_red(self):
        tree = self._tree(
            a='[dependencies]\ntokio = { version = "1", features = ["sync"] }\n',
            b='[dependencies]\ntokio = { version = "1", features = ["process"] }\n')
        lines = self._red(tree)
        self.assertTrue(any("tokio" in l and "crates/a/Cargo.toml" in l for l in lines), lines)
        self.assertTrue(any("crates/b/Cargo.toml" in l for l in lines), lines)

    def test_two_members_with_equal_local_declarations_are_red_too(self):
        """Equal today is no lock: a shared dependency lives in [workspace.dependencies]."""
        tree = self._tree(a='[dependencies]\nserde_json = "1"\n',
                          b='[dev-dependencies]\nserde_json = "1"\n')
        self.assertTrue(any("serde_json" in l for l in self._red(tree)))

    def test_one_member_dev_and_normal_with_different_features_is_red(self):
        tree = self._tree(
            a='[dependencies]\nfu = { version = "0.3", features = ["std"] }\n'
              '[dev-dependencies]\nfu = { version = "0.3", features = ["io"] }\n')
        self.assertTrue(any("fu" in l for l in self._red(tree)))

    def test_target_tables_count(self):
        tree = self._tree(
            a='[target."cfg(unix)".dependencies]\nlibc = { version = "0.2", features = ["extra_traits"] }\n',
            b='[dependencies]\nlibc = "0.2"\n')
        self.assertTrue(any("libc" in l for l in self._red(tree)))

    def test_workspace_inherited_shared_dependency_is_quiet(self):
        tree = self._tree(
            ws_deps='tokio = { version = "1", features = ["sync", "process"] }\n'
                    'helper = { path = "crates/b" }\n',
            a='[dependencies]\ntokio = { workspace = true }\nb = { path = "../b" }\n'
              '[dev-dependencies]\ntokio.workspace = true\n',
            b='[dependencies]\ntokio = { workspace = true, optional = true }\nonly-b = "2"\n')
        self.assertEqual(locks.check_feature_unity_lock(["crates/a/Cargo.toml"], tree), [])

    def test_member_features_on_top_of_the_workspace_are_red(self):
        tree = self._tree(ws_deps='tokio = { version = "1", features = ["sync"] }\n',
                          a='[dependencies]\ntokio = { workspace = true, features = ["fs"] }\n',
                          b='[dependencies]\ntokio = { workspace = true }\n')
        self.assertTrue(any("tokio" in l and "crates/a/Cargo.toml" in l for l in self._red(tree)))

    def test_member_default_features_override_is_red(self):
        tree = self._tree(ws_deps='hyper = { version = "1", default-features = false }\n',
                          a='[dependencies]\nhyper = { workspace = true, default-features = true }\n',
                          b='[dependencies]\nhyper = { workspace = true }\n')
        self.assertTrue(any("hyper" in l for l in self._red(tree)))

    def test_a_workspace_dependency_declared_locally_is_red(self):
        """One user only, but the workspace declares it: a local copy forks the set."""
        tree = self._tree(ws_deps='uuid = { version = "1", features = ["v7"] }\n',
                          a='[dependencies]\nuuid = { version = "1", features = ["v4"] }\n')
        self.assertTrue(any("uuid" in l for l in self._red(tree)))

    def test_inherited_name_missing_in_the_workspace_is_red(self):
        tree = self._tree(a='[dependencies]\ntokio = { workspace = true }\n')
        self.assertTrue(any("tokio" in l for l in self._red(tree)))

    def test_path_dependencies_of_members_are_exempt(self):
        tree = self._tree(a='[dependencies]\nb = { path = "../b" }\n',
                          b='[dev-dependencies]\nb2 = { path = "../b2", features = ["x"] }\n')
        write(tree, "crates/a/Cargo.toml",
              '[package]\nname = "a"\n[dependencies]\nshared = { path = "../s" }\n')
        write(tree, "crates/b/Cargo.toml",
              '[package]\nname = "b"\n[dependencies]\nshared = { path = "../s", features = ["y"] }\n')
        self.assertEqual(locks.check_feature_unity_lock(["Cargo.toml"], tree), [])

    def test_unreadable_member_manifest_is_red(self):
        tree = self._tree(a='[dependencies]\ntokio = { version = "1"\n')  # broken TOML
        self.assertTrue(any("crates/a/Cargo.toml" in l for l in self._red(tree)))

    def test_missing_member_manifest_is_red(self):
        tree = self._tree()
        os.remove(os.path.join(tree, "crates", "b", "Cargo.toml"))
        self.assertTrue(any("crates/b/Cargo.toml" in l for l in self._red(tree)))

    def test_root_without_members_is_red(self):
        tree = self._tree(root='[workspace]\nresolver = "3"\n')
        self.assertTrue(any("members" in l for l in self._red(tree)))

    def test_unreadable_root_manifest_is_red(self):
        tree = self._tree(root='[workspace\n')
        self.assertTrue(any("Cargo.toml" in l for l in self._red(tree)))

    def test_glob_members_are_red_rather_than_unread(self):
        tree = self._tree(root='[workspace]\nmembers = ["crates/*"]\n')
        self.assertTrue(any("crates/*" in l for l in self._red(tree)))

    def test_silent_without_a_manifest_in_the_diff(self):
        tree = self._tree(
            a='[dependencies]\ntokio = { version = "1", features = ["sync"] }\n',
            b='[dependencies]\ntokio = { version = "1", features = ["process"] }\n')
        self.assertEqual(locks.check_feature_unity_lock(["docs/x.md"], tree), [])
        self.assertTrue(locks.check_feature_unity_lock(["crates/b/Cargo.toml"], tree))

    def test_a_tree_without_a_workspace_is_silent(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.assertEqual(locks.check_feature_unity_lock(["Cargo.toml"], tmp.name), [])

    def test_precheck_station_carries_the_lock(self):
        tree = self._tree(
            a='[dependencies]\ntokio = { version = "1", features = ["sync"] }\n',
            b='[dependencies]\ntokio = { version = "1" }\n')
        station = [f for f in precheck.run(["Cargo.toml"], repo=tree)
                   if f.check == "feature-unity-lock"]
        self.assertTrue(station and all(f.level == "RED" for f in station))

    # Re-check h1 L-1: the lock keyed on the dependency NAME, so `package =`
    # carried a second feature set of a shared crate past it (mutations M6/M6b
    # green). The key is the crate a declaration names.
    def test_a_renamed_local_copy_of_a_shared_dependency_is_red(self):
        tree = self._tree(
            ws_deps='tokio = { version = "1", features = ["sync"] }\n',
            a='[dependencies]\ntokio = { workspace = true }\n'
              'tokio_full = { package = "tokio", version = "1", features = ["full"] }\n',
            b='[dependencies]\ntokio = { workspace = true }\n')
        lines = self._red(tree, diff=("crates/a/Cargo.toml",))
        self.assertTrue(any("tokio_full" in l and "package tokio" in l
                            and "crates/a/Cargo.toml" in l for l in lines), lines)

    def test_renamed_copies_in_two_members_are_red(self):
        tree = self._tree(
            a='[dependencies]\ntk1 = { package = "tokio", version = "1", features = ["sync"] }\n',
            b='[dependencies]\ntk2 = { package = "tokio", version = "1" }\n')
        lines = self._red(tree)
        self.assertTrue(any("tk1" in l and "crates/a/Cargo.toml" in l for l in lines), lines)
        self.assertTrue(any("tk2" in l and "crates/b/Cargo.toml" in l for l in lines), lines)

    def test_the_workspace_declaring_one_crate_under_two_names_is_red(self):
        tree = self._tree(
            ws_deps='tokio = { version = "1", features = ["sync"] }\n'
                    'tk = { package = "tokio", version = "1", features = ["full"] }\n',
            a='[dependencies]\ntokio = { workspace = true }\n',
            b='[dependencies]\ntk = { workspace = true }\n')
        lines = self._red(tree)
        self.assertTrue(any(l.startswith("Cargo.toml") and "tk" in l and "tokio" in l
                            for l in lines), lines)

    def test_a_renamed_workspace_dependency_inherited_everywhere_is_quiet(self):
        tree = self._tree(
            ws_deps='tk = { package = "tokio", version = "1", features = ["sync"] }\n',
            a='[dependencies]\ntk = { workspace = true }\n',
            b='[dev-dependencies]\ntk = { workspace = true }\n')
        self.assertEqual(locks.check_feature_unity_lock(["Cargo.toml"], tree), [])

    # Re-check h1 L-2: `dep/feature` in a member's [features] and the dependency
    # tables of a root [package] switch features on just like a member list, and
    # the lock read neither (mutations M7/M8 green).
    def test_a_member_feature_switching_on_a_shared_dependency_feature_is_red(self):
        for entry in ("tokio/test-util", "tokio?/test-util", "tk/test-util"):
            with self.subTest(entry=entry):
                tree = self._tree(
                    ws_deps='tokio = { version = "1", features = ["sync"] }\n'
                            'tk = { package = "tokio", version = "1", features = ["sync"] }\n'
                    if entry.startswith("tk/") else
                    'tokio = { version = "1", features = ["sync"] }\n',
                    a='[dependencies]\n%s = { workspace = true }\n'
                      '[features]\ndefault = ["%s"]\n' % (entry.split("/")[0].rstrip("?"), entry),
                    b='[dependencies]\ntokio = { workspace = true }\n')
                lines = self._red(tree, diff=("crates/a/Cargo.toml",))
                self.assertTrue(any("[features] default" in l and entry in l
                                    and "crates/a/Cargo.toml" in l for l in lines), lines)

    def test_member_features_on_own_path_or_unshared_dependencies_are_quiet(self):
        tree = self._tree(
            ws_deps='tokio = { version = "1", features = ["sync"] }\n',
            a='[dependencies]\ntokio = { workspace = true }\nb = { path = "../b" }\n'
              'only-a = { version = "2", optional = true }\n'
              '[features]\ndefault = ["hooks"]\nhooks = []\n'
              'x = ["b/y", "dep:only-a", "only-a/z"]\n',
            b='[dependencies]\ntokio = { workspace = true }\n[features]\ny = []\n')
        self.assertEqual(locks.check_feature_unity_lock(["Cargo.toml"], tree), [])

    def test_a_root_package_is_read_like_a_member(self):
        root = self.ROOT + '[package]\nname = "root"\nversion = "0.1.0"\n'
        for tail in ('[dependencies]\ntokio = { version = "1", features = ["full"] }\n',
                     '[dependencies]\ntokio = { workspace = true }\n'
                     '[features]\ndefault = ["tokio/full"]\n'):
            with self.subTest(tail=tail):
                tree = self._tree(root=root + tail,
                                  ws_deps='tokio = { version = "1", features = ["sync"] }\n',
                                  a='[dependencies]\ntokio = { workspace = true }\n')
                lines = self._red(tree)
                self.assertTrue(any(l.startswith("Cargo.toml") and "tokio" in l for l in lines),
                                lines)

    def test_a_root_package_that_inherits_is_quiet(self):
        root = self.ROOT + ('[package]\nname = "root"\nversion = "0.1.0"\n'
                            '[dependencies]\ntokio = { workspace = true }\n')
        tree = self._tree(root=root, ws_deps='tokio = { version = "1", features = ["sync"] }\n',
                          a='[dependencies]\ntokio = { workspace = true }\n')
        self.assertEqual(locks.check_feature_unity_lock(["Cargo.toml"], tree), [])

    def test_this_repository_has_one_feature_set_per_shared_dependency(self):
        """The real tree, every run: the S6 measurement holds only while it is quiet."""
        repo = str(SCRIPTS.parent)
        private_tree.require("crates/meclaw-colony/Cargo.toml", repo=repo)
        found = locks.check_feature_unity_lock(["Cargo.toml"], repo)
        self.assertEqual([], [locks.render(f) for f in found])


if __name__ == "__main__":
    unittest.main()
