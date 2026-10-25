"""Frozen template versions in the kit (`scripts/pipeline/locks.py`, `versions.py`).

A frozen version is a directory `templates/<name>@<x.y.z>/` whose
`template.json` declares exactly that name and version: the scanner registers
it beside the living `templates/<name>/` (name AND version are unique, GH
#664), so a breaking bump can leave the old number shipped for the pinners
that still need it (`figure` keeps pinning `simple-agent@1.1.4` while
`simple-agent` moves to 2.0.0). The kit took one directory per name until
this module: `versions.assign` overwrote the living directory with the frozen
one in `by_name`, the cascade raised every pin of the frozen number, the
pin-lock called it "not provided", and the version-lock forbade the strand the
number change that keeps the scan free of `DuplicateVersion`.

Every case builds a small tree: `a` living at 2.0.0 beside `a@1.1.3` frozen,
`fig` pinning `a@1.1.3`.
"""

import contextlib
import importlib.util
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
REPO = SCRIPTS.parent
sys.path.insert(0, str(SCRIPTS))

from pipeline import locks, versions  # noqa: E402
from scripts.tests import private_tree, store_isolation  # noqa: E402

_METRICS_URL = mock.patch.dict(os.environ, {"MECLAW_PIPELINE_METRICS_URL": ""})
_GIT_ENV = mock.patch.dict(os.environ, {
    "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@example.invalid",
    "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@example.invalid",
    "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1"})


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()
    _METRICS_URL.start()
    _GIT_ENV.start()


def tearDownModule():  # noqa: N802 -- unittest's name
    _GIT_ENV.stop()
    _METRICS_URL.stop()
    store_isolation.restore()


def write(root, rel, text):
    path = os.path.join(root, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(text)


def read(root, rel):
    with open(os.path.join(root, rel), encoding="utf-8") as fh:
        return fh.read()


def tj(name, version, **extra):
    doc = {"name": name, "version": version}
    doc.update(extra)
    return json.dumps(doc, indent=2) + "\n"


def git(root, *args):
    return subprocess.run(["git", "-C", root] + list(args), check=True,
                          capture_output=True, text=True).stdout.strip()


def commit(root, msg="base"):
    git(root, "add", "-A")
    git(root, "commit", "-q", "-m", msg)
    return git(root, "rev-parse", "HEAD")


FIG_PIN = "templates/fig/brain/config.json"
SCAFFOLD = "workshop/evals/builder-scenarios/scaffolds/s/main/config.json"


class Frozen(unittest.TestCase):
    """`a` 2.0.0 living, `a@1.1.3` frozen, `fig` pins the frozen number."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = os.path.realpath(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)

    def tree(self, frozen=True, living="2.0.0"):
        write(self.root, "templates/a/template.json", tj("a", living))
        write(self.root, "templates/b/template.json", tj("b", "1.0.0"))
        if frozen:
            write(self.root, "templates/a@1.1.3/template.json", tj("a", "1.1.3"))
            write(self.root, "templates/a@1.1.3/brain/config.json",
                  json.dumps({"template": "b@1.0.0"}) + "\n")
        write(self.root, "templates/fig/template.json", tj("fig", "1.0.4"))
        write(self.root, FIG_PIN, json.dumps({"template": "a@1.1.3"}) + "\n")

    def base_then_break(self, frozen=True):
        """Base: `a` living at 1.1.3, pinned by fig. Candidate: `a` written to
        2.0.0 by the strand, the old number frozen beside it."""
        self.tree(frozen=False, living="1.1.3")
        base = (git(self.root, "init", "-q", "-b", "master"), commit(self.root))[1]
        write(self.root, "templates/a/template.json", tj("a", "2.0.0"))
        if frozen:
            write(self.root, "templates/a@1.1.3/template.json", tj("a", "1.1.3"))
        return base


class RuleOneFrozenIsNamedAndDeclared(Frozen):

    def test_frozen_templates_names_the_declared_versions(self):
        self.tree()
        # a directory named like a frozen one whose template.json says another
        # number is no frozen version (the frozen-template-lock says so)
        write(self.root, "templates/a@9.9.9/template.json", tj("a", "1.0.0"))
        write(self.root, "templates/b@0.9.0/template.json", tj("b", "0.9.0"))
        self.assertEqual({"a": {"1.1.3"}, "b": {"0.9.0"}}, locks.frozen_templates(self.root))
        self.assertEqual(("a", "1.1.3"), locks.frozen_dir("a@1.1.3"))
        self.assertIsNone(locks.frozen_dir("a"))


class RuleTwoOnlyLivingDirectoriesNumber(Frozen):

    def test_frozen_dir_is_never_the_version_of_its_name(self):
        self.tree()
        self.assertEqual({"a": "2.0.0", "b": "1.0.0", "fig": "1.0.4"},
                         locks.template_versions(self.root))
        dirs = versions.template_dirs(self.root)
        self.assertEqual({"a": "a", "b": "b", "fig": "fig"}, dirs)
        self.assertEqual(set(), versions.touched_templates(
            ["templates/a@1.1.3/brain/config.json"], dirs))


class RuleThreeAFrozenPinIsProvided(Frozen):

    def test_pin_lock_takes_a_pin_on_a_frozen_version(self):
        self.tree()
        write(self.root, SCAFFOLD, json.dumps({"template": "a@1.1.3"}) + "\n")
        write(self.root, "docs/x.md", "The figure runs a@1.1.3 today.\n")
        found = locks.check_pin_lock(["docs/x.md"], self.root)
        self.assertEqual([], [locks.render(f) for f in found])
        self.assertEqual([], locks.check_scenario_pin_lock([], self.root))
        # a number neither living nor frozen stays RED
        write(self.root, FIG_PIN, json.dumps({"template": "a@1.1.2"}) + "\n")
        self.assertEqual(["RED"], [f.verdict for f in locks.check_pin_lock([], self.root)])


    def test_a_stale_pin_inside_a_frozen_version_is_raised_like_any(self):
        # OR-agent-2.A1.2: a dependency pin of a frozen copy follows the living
        # dependency (the cascade raises it at the merge), so the lock says so
        self.tree()
        write(self.root, "templates/b/template.json", tj("b", "1.0.1"))
        found = locks.check_pin_lock([], self.root)
        self.assertEqual([("RED", "templates/a@1.1.3/brain/config.json")],
                         [(f.verdict, f.path) for f in found])
        self.assertEqual("raise to 1.0.1", found[0].fix)


class RuleFourAFrozenOldNumberIsNotLeftBehind(Frozen):

    def test_net_and_left_pins_spare_a_frozen_old_number(self):
        base = self.base_then_break(frozen=False)
        diff = ["templates/a/template.json"]
        # control: without the frozen directory the fig pin is left behind
        self.assertEqual([FIG_PIN], [f.path for f in locks.left_behind(diff, self.root, base)])
        write(self.root, "templates/a@1.1.3/template.json", tj("a", "1.1.3"))
        self.assertEqual([], locks.left_behind(diff + ["templates/a@1.1.3/template.json"],
                                               self.root, base))
        write(self.root, "templates/fig/notes.txt", "pins a@1.1.3\n")
        self.assertEqual([], versions.left_pins(self.root, {"a": {"1.1.3"}}))

    def test_net_reads_the_dependency_pins_of_a_frozen_copy(self):
        self.tree()
        git(self.root, "init", "-q", "-b", "master")
        base = commit(self.root)
        write(self.root, "templates/b/template.json", tj("b", "1.0.1"))
        found = locks.left_behind(["templates/b/template.json"], self.root, base)
        self.assertEqual(["templates/a@1.1.3/brain/config.json"], [f.path for f in found])


class RuleFiveTheCascadeKeepsFrozenPins(Frozen):

    def test_cascade_never_raises_a_frozen_pin(self):
        self.tree()
        write(self.root, "docs/x.md", "The figure runs a@1.1.3.\n")
        write(self.root, "crates/c/tests/t.rs", 'const PIN: &str = "a@1.1.3";\n')
        write(self.root, "templates/README.md",
              "| [`a`](a/) | 1.1.3 | living |\n| [`a`](a@1.1.3/) | 1.1.3 | frozen |\n")
        raised = versions.cascade(self.root, {"a": ("1.1.3", "2.0.0")})
        self.assertEqual([("templates/README.md", 1)], [(p["path"], p["line"]) for p in raised])
        self.assertIn('"a@1.1.3"', read(self.root, FIG_PIN))
        self.assertIn("a@1.1.3", read(self.root, "docs/x.md"))
        self.assertIn('"a@1.1.3"', read(self.root, "crates/c/tests/t.rs"))
        self.assertEqual("| [`a`](a/) | 2.0.0 | living |\n| [`a`](a@1.1.3/) | 1.1.3 | frozen |\n",
                         read(self.root, "templates/README.md"))

    def test_cascade_raises_only_dependency_pins_in_a_frozen_directory(self):
        # OR-agent-2.A1.2: figure -> simple-agent@1.1.4 -> curator@1.15.3 must keep
        # resolving when curator moves; the frozen copy's own name and its prose
        # stay, and the copy gets no number
        self.tree()
        rel = "templates/a@1.1.3/brain/config.json"
        write(self.root, rel, json.dumps({"template": "b@1.0.0", "self": "a@1.0.0"}) + "\n")
        write(self.root, "templates/a@1.1.3/README.md", "Pins b@1.0.0.\n")
        raised = versions.cascade(self.root, {"b": ("1.0.0", "1.0.1"), "a": ("1.1.3", "2.0.0")})
        self.assertEqual([rel], [p["path"] for p in raised if p["path"].startswith("templates/a@")])
        self.assertIn('"b@1.0.1"', read(self.root, rel))
        self.assertIn('"a@1.0.0"', read(self.root, rel))
        self.assertEqual("Pins b@1.0.0.\n", read(self.root, "templates/a@1.1.3/README.md"))

    def test_assign_raises_a_frozen_dependency_pin_without_numbering(self):
        self.tree()
        git(self.root, "init", "-q", "-b", "master")
        base = commit(self.root)
        write(self.root, "templates/b/README.md", "changed\n")
        res = versions.assign(self.root, base, ["templates/b/README.md"], {})
        self.assertEqual({"b": {"from": "1.0.0", "to": "1.0.1", "kind": "fix"}}, res["versions"])
        self.assertEqual(["templates/a@1.1.3/brain/config.json"], [p["path"] for p in res["pins"]])
        self.assertIn('"a@1.1.3"', read(self.root, FIG_PIN))
        self.assertEqual("1.1.3", json.loads(read(self.root, "templates/a@1.1.3/template.json"))
                         ["version"])
        # the merge's candidate passes the immutability lock
        diff = git(self.root, "diff", "--name-only", base).split()
        self.assertEqual([], locks.check_frozen_template_lock(diff, self.root, {"base": base}))

    def test_assign_breaks_the_living_one_and_keeps_the_frozen_pin(self):
        base = self.base_then_break()
        changed = ["templates/a/template.json", "templates/a@1.1.3/template.json"]
        res = versions.assign(self.root, base, changed, {"a": "break"})
        self.assertEqual({"a": {"from": "1.1.3", "to": "2.0.0", "kind": "break"}},
                         res["versions"])
        self.assertEqual([], res["pins"])
        self.assertIn('"a@1.1.3"', read(self.root, FIG_PIN))
        self.assertEqual("1.1.3", json.loads(read(self.root, "templates/a@1.1.3/template.json"))
                         ["version"])


    def test_the_dry_cascade_takes_a_frozen_old_number(self):
        from pipeline import cascade_dry
        self.tree()
        write(self.root, "templates/a@1.1.3/README.md", "# a@1.1.3\n")
        found = cascade_dry.leftovers(self.root, {"a": ("1.1.3", "2.0.0")},
                                      versions.template_dirs(self.root))
        self.assertEqual([], [locks.render(f) for f in found])


class RuleSixTheVersionLockKnowsTheFrozenOldNumber(Frozen):

    def test_version_lock_is_a_note_when_the_old_number_is_frozen(self):
        base = self.base_then_break()
        ctx = {"base": base, "mode": "strand"}
        found = locks.check_version_lock(["templates/a/template.json"], self.root, ctx)
        self.assertEqual(["NOTE"], [f.verdict for f in found])
        self.assertIn("templates/a@1.1.3/", found[0].text)
        self.assertIn("merge point", found[0].text)

    def test_version_lock_stays_red_without_the_frozen_directory(self):
        base = self.base_then_break(frozen=False)
        found = locks.check_version_lock(["templates/a/template.json"], self.root,
                                         {"base": base, "mode": "strand"})
        self.assertEqual(["RED"], [f.verdict for f in found])


class RuleSevenFrozenIsImmutable(Frozen):

    def lock(self, diff, base=None):
        return locks.check_frozen_template_lock(diff, self.root, {"base": base})

    def test_frozen_template_lock_is_registered(self):
        self.assertIn("frozen-template-lock", dict(locks.LOCKS))

    def test_a_file_the_base_had_is_never_changed_or_deleted(self):
        self.tree()
        git(self.root, "init", "-q", "-b", "master")
        base = commit(self.root)
        self.assertEqual([], self.lock([], base))
        rel = "templates/a@1.1.3/brain/config.json"
        write(self.root, rel, json.dumps({"template": "b@1.0.0", "x": 1}) + "\n")
        found = self.lock([rel], base)
        self.assertEqual([("RED", rel)], [(f.verdict, f.path) for f in found])
        os.remove(os.path.join(self.root, rel))
        self.assertEqual(["RED"], [f.verdict for f in self.lock([rel], base)])

    def test_only_a_pin_raised_to_a_shipped_version_may_change(self):
        self.tree()
        git(self.root, "init", "-q", "-b", "master")
        base = commit(self.root)
        rel = "templates/a@1.1.3/brain/config.json"
        write(self.root, "templates/b/template.json", tj("b", "1.0.1"))
        write(self.root, rel, json.dumps({"template": "b@1.0.1"}) + "\n")
        self.assertEqual([], self.lock([rel], base))
        # a version the tree does not ship
        write(self.root, rel, json.dumps({"template": "b@1.0.2"}) + "\n")
        self.assertEqual(["RED"], [f.verdict for f in self.lock([rel], base)])
        # a pin and other bytes in the same line
        write(self.root, rel, json.dumps({"template": "b@1.0.1", "y": 2}) + "\n")
        self.assertEqual(["RED"], [f.verdict for f in self.lock([rel], base)])
        # a line more
        write(self.root, rel, json.dumps({"template": "b@1.0.1"}) + "\n\n")
        self.assertEqual(["RED"], [f.verdict for f in self.lock([rel], base)])

    def test_the_own_name_pin_of_a_frozen_copy_never_moves(self):
        self.tree()
        rel = "templates/a@1.1.3/brain/config.json"
        write(self.root, rel, json.dumps({"self": "a@1.0.0"}) + "\n")
        git(self.root, "init", "-q", "-b", "master")
        base = commit(self.root)
        write(self.root, rel, json.dumps({"self": "a@2.0.0"}) + "\n")
        self.assertEqual(["RED"], [f.verdict for f in self.lock([rel], base)])

    def test_a_new_file_and_a_new_frozen_directory_are_free(self):
        self.tree(frozen=False)
        git(self.root, "init", "-q", "-b", "master")
        base = commit(self.root)
        self.tree()
        new = ["templates/a@1.1.3/template.json", "templates/a@1.1.3/brain/config.json"]
        self.assertEqual([], self.lock(new, base))

    def test_a_frozen_directory_must_declare_its_name(self):
        self.tree()
        write(self.root, "templates/a@1.2.0/template.json", tj("a", "1.1.3"))
        write(self.root, "templates/x@1.0.0/template.json", tj("y", "1.0.0"))
        write(self.root, "templates/a@next/template.json", tj("a", "1.0.0"))
        found = self.lock([])
        self.assertEqual(["templates/a@1.2.0/template.json", "templates/a@next/template.json",
                          "templates/x@1.0.0/template.json"], sorted(f.path for f in found))
        self.assertEqual({"RED"}, {f.verdict for f in found})


class RuleEightTheCatalogueSkipsFrozen(Frozen):

    def test_check_catalogue_skips_a_frozen_directory(self):
        self.tree()
        write(self.root, "templates/README.md",
              "| [`a`](a/) | 2.0.0 | x |\n| [`fig`](fig/) | 1.0.4 | pins `a@1.1.3` |\n")
        res = subprocess.run([sys.executable, "-I", str(SCRIPTS / "check_catalogue.py")],
                             cwd=self.root, capture_output=True, text=True)
        self.assertEqual((0, "catalogue ok"), (res.returncode, res.stdout.strip()), res.stdout)


def _load_make_export():
    spec = importlib.util.spec_from_file_location(
        "make_export_frozen", REPO / "plans" / "export-fixtures" / "make_export.py")
    mod = importlib.util.module_from_spec(spec)
    with contextlib.redirect_stdout(io.StringIO()):
        spec.loader.exec_module(mod)
    return mod


@private_tree.private("plans/export-fixtures/make_export.py")
class RuleEightTheExportShipsFrozenPublicVersions(unittest.TestCase):

    @classmethod
    def setUpClass(cls):
        cls.mx = _load_make_export()

    def test_a_frozen_public_directory_is_public(self):
        mx = self.mx
        self.assertFalse(mx.is_forbidden("templates/simple-agent@1.1.4/template.json"))
        self.assertTrue(mx.is_forbidden("templates/simple-agentx@1.1.3/template.json"))

    def test_the_export_copies_the_frozen_public_directories(self):
        mx = self.mx
        listing = "templates/simple-agent\ntemplates/simple-agent@1.1.4\ntemplates/nope@1.0.0\n"
        with mock.patch.object(mx, "git", lambda *a, **k: listing):
            self.assertEqual(["simple-agent@1.1.4"], mx.frozen_public_templates("HEAD"))

    def test_a_frozen_directory_takes_the_verdict_of_its_name(self):
        mx = self.mx
        with tempfile.TemporaryDirectory() as tmp:
            for d in ("a", "a@1.1.3", "p"):
                os.makedirs(os.path.join(tmp, "templates", d))
            report = []
            with mock.patch.object(mx, "REPO", pathlib.Path(tmp)), \
                    mock.patch.object(mx, "PUBLIC_TEMPLATES", ["a"]), \
                    mock.patch.object(mx, "PRIVATE_TEMPLATES", [("p", "staging", "x" * 90)]), \
                    mock.patch.object(mx, "KEEP_GOING", True), \
                    mock.patch.object(mx, "REDS", []):
                mx.receipt_every_template_has_a_verdict(report)
                self.assertEqual([], mx.REDS)
            self.assertEqual(1, len(report))

    def test_a_frozen_copy_carries_the_allowed_hits_of_its_name(self):
        mx = self.mx
        # The line comes from the shipped file, so the test itself names nobody (R5).
        src = (REPO / "templates" / "simple-agent" / "template.json").read_text(encoding="utf-8")
        line = next(x for x in src.splitlines() if '"author"' in x)
        self.assertIsNotNone(mx._is_allowed("templates/simple-agent/template.json", line))
        self.assertIsNotNone(mx._is_allowed("templates/simple-agent@1.1.4/template.json", line))


if __name__ == "__main__":
    unittest.main()
