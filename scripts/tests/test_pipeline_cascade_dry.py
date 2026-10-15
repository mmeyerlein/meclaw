"""Tests of `cascade-dry` (`scripts/pipeline/cascade_dry.py`, GH #1173): `pre`
runs the merge point's version cascade dry, so a pin the cascade cannot raise
and a version literal next to a template path fail before the merge queue.

The tree is the fixture of `test_pipeline_merge` (template `a` 1.0.0, `c`
pins `a@1.0.0`); the strand's diff is the working copy against `master`.
"""

import os
import pathlib
import sys
import unittest
from unittest import mock

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS))

from pipeline import cascade_dry, pre  # noqa: E402

from . import test_pipeline_merge as fx  # noqa: E402
from scripts.tests import store_isolation  # noqa: E402

_METRICS_URL = mock.patch.dict(os.environ, {"MECLAW_PIPELINE_METRICS_URL": ""})

# The escaped recipe pin the queue run on tb-s8's candidate failed 7 tests for.
# Since the tb-s8 merge (c1a69f8a6, `pins --cascade --write` raises escaped
# pins) the cascade raises it: it stays green (tb-s10 Zusammenführer).
RAISED = "templates/b/recipe.json"
RAISED_TEXT = '{"script_inline": "S = {\\"screen\\": \\"a@1.0.0\\"}"}\n'
# A pin in a template file outside the cascade's classes (a script): the
# cascade leaves it, the leftover scan reads every template file.
ESCAPED = "templates/b/render.js"
ESCAPED_TEXT = "const S = {screen: 'a@1.0.0'};\n"
LITERAL = "crates/x/tests/version.rs"
LITERAL_TEXT = ('const T: &str = "templates/a/template.json";\n'
                'fn v() { assert_eq!(version(T), "1.0.0"); }\n')


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()
    _METRICS_URL.start()


def tearDownModule():  # noqa: N802 -- unittest's name
    _METRICS_URL.stop()
    store_isolation.restore()


class CascadeDryCase(unittest.TestCase):

    def setUp(self):
        self.f = fx.Fixture()
        self.addCleanup(self.f.close)
        self.root = self.f.root

    def commit(self, files):
        for rel, text in files.items():
            fx.write(self.root, rel, text)
        fx.git(self.root, "add", "-A")
        fx.git(self.root, "commit", "-q", "-m", "fixture")

    def strand_touches_a(self):
        fx.write(self.root, "templates/a/README.md", "Template a, changed.\n")
        return ["templates/a/README.md"]

    def check(self, paths):
        return cascade_dry.check(paths, self.root, {"base": "master"})


class TheDryCascade(CascadeDryCase):

    def test_an_escaped_pin_the_cascade_cannot_raise_is_red(self):
        self.commit({ESCAPED: ESCAPED_TEXT})
        found, _ = self.check(self.strand_touches_a())
        self.assertEqual([(f.verdict, f.check, f.path, f.line) for f in found],
                         [("RED", "cascade-dry", ESCAPED, 1)])
        self.assertIn("a@1.0.0", found[0].text)
        self.assertIn("a@1.0.1", found[0].text)

    def test_the_escaped_recipe_pin_tb_s8_raises_stays_green(self):
        self.commit({RAISED: RAISED_TEXT})
        found, note = self.check(self.strand_touches_a())
        self.assertEqual(found, [])
        self.assertIsNone(note)

    def test_a_version_literal_next_to_the_template_path_is_red(self):
        self.commit({LITERAL: LITERAL_TEXT})
        found, _ = self.check(self.strand_touches_a())
        self.assertEqual([(f.path, f.line) for f in found], [(LITERAL, 2)])
        self.assertIn('"1.0.0"', found[0].text)

    def test_pins_the_cascade_raises_and_history_stay_green(self):
        self.commit({"crates/x/tests/hist.rs": "// measured on a@1.0.0\n",
                     "templates/b/template.json": fx.template_json(
                         "b", "2.0.0", description={
                             "purpose": "Shows. Since 2.0.0 (#1): pins a@1.0.0."})})
        found, note = self.check(self.strand_touches_a())
        self.assertEqual(found, [])
        self.assertIsNone(note)

    def test_the_tree_is_never_written(self):
        self.commit({ESCAPED: ESCAPED_TEXT})
        paths = self.strand_touches_a()
        self.check(paths)
        self.assertEqual(fx.version_of(self.root, "a"), "1.0.0")
        self.assertEqual(fx.read(self.root, "templates/c/template.json"),
                         fx.TREE["templates/c/template.json"])

    def test_a_diff_without_templates_skips_at_no_cost(self):
        self.commit({ESCAPED: ESCAPED_TEXT})
        fx.write(self.root, "docs/other.md", "Changed.\n")
        with mock.patch.object(cascade_dry.versions, "assign") as assign:
            found, note = self.check(["docs/other.md"])
        assign.assert_not_called()
        self.assertEqual(found, [])
        self.assertIn("skipped", note)

    def test_relevance_is_fail_closed_on_any_templates_path(self):
        self.assertTrue(cascade_dry.relevant(["templates/README.md"]))
        self.assertFalse(cascade_dry.relevant(["docs/x.md", "scripts/a.py"]))


class PreCarriesTheDryCascade(CascadeDryCase):

    def test_pre_local_half_is_red_on_a_pin_the_queue_would_leave(self):
        self.commit({ESCAPED: ESCAPED_TEXT})
        paths = self.strand_touches_a()
        results = pre.local_half(paths, self.root, {"base": "master", "mode": "pre"})
        mine = [r for r in results if r.check == "cascade-dry"]
        self.assertEqual([r.verdict for r in mine], ["RED"])
        self.assertIn(ESCAPED, mine[0].text)

    def test_pre_local_half_names_the_skip(self):
        fx.write(self.root, "docs/other.md", "Changed.\n")
        results = pre.local_half(["docs/other.md"], self.root, {"base": "master", "mode": "pre"})
        mine = [r for r in results if r.check == "cascade-dry"]
        self.assertEqual([(r.verdict, r.text) for r in mine],
                         [("GREEN", "skipped: no templates/ path in the diff")])


if __name__ == "__main__":
    unittest.main()
