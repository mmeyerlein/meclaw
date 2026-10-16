"""GH #1199: the one mechanism for kit self-tests that read the private tree.

`private_tree.require` skips only where the export definition is absent (the
export tree) and fails, never skips, where it is present (the private tree).
These cases pin both halves on throw-away trees, and on the real tree that the
marker is the export script and never travels.
"""
import importlib.util
import pathlib
import re
import tempfile
import unittest

from scripts.tests import private_tree, store_isolation


def tree(*files):
    root = pathlib.Path(tempfile.mkdtemp(prefix="private-tree-"))
    for rel in files:
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("x\n", encoding="utf-8")
    return root


class Require(unittest.TestCase):

    def test_the_private_tree_fails_a_missing_input_instead_of_skipping(self):
        root = tree(private_tree.MARKER)
        # A SkipTest would pass through assertRaisesRegex(AssertionError) and
        # book this pin as skipped -- exactly the quiet it guards against.
        try:
            with self.assertRaisesRegex(AssertionError, r"private-tree input missing .*: AGENTS\.md"):
                private_tree.require("AGENTS.md", repo=root)
        except unittest.SkipTest as exc:
            self.fail("require() skipped in the private tree instead of failing: %s" % exc)

    def test_the_export_tree_skips_a_missing_input_and_names_it(self):
        root = tree("scripts/x.py")
        with self.assertRaisesRegex(unittest.SkipTest, r"private tree only .*: docs/development-rules\.md"):
            private_tree.require("scripts/x.py", "docs/development-rules.md", repo=root)

    def test_a_present_input_runs_in_both_trees(self):
        for root in (tree("AGENTS.md"), tree("AGENTS.md", private_tree.MARKER)):
            self.assertIsNone(private_tree.require("AGENTS.md", repo=root))

    def test_no_path_means_the_whole_private_tree(self):
        self.assertIsNone(private_tree.require(repo=tree(private_tree.MARKER)))
        with self.assertRaisesRegex(unittest.SkipTest, r"private tree only \(GH #1199\)"):
            private_tree.require(repo=tree("scripts/x.py"))


class Decorator(unittest.TestCase):

    def test_a_decorated_class_checks_inside_each_test_never_at_import(self):
        seen = []

        @private_tree.private("no/such/path-1199")
        class Probe(unittest.TestCase):
            def test_reads(self):
                seen.append("ran")

        result = unittest.TestResult()
        Probe("test_reads").run(result)
        self.assertEqual([], seen)
        if private_tree.is_private():
            self.assertEqual(1, len(result.failures), result.failures)
            self.assertEqual([], result.skipped)
        else:
            self.assertEqual(1, len(result.skipped), result.skipped)

    def test_a_decorated_class_checks_before_its_set_up_class(self):
        """A class whose setUpClass loads the private input (an import from
        workshop/, the export script) dies in setUpClass in the export tree
        unless the check runs first -- an error, not the named skip."""
        seen = []

        @private_tree.private("no/such/path-1199")
        class Probe(unittest.TestCase):
            @classmethod
            def setUpClass(cls):
                seen.append("set up")

            def test_reads(self):
                seen.append("ran")

        result = unittest.TestResult()
        unittest.TestSuite([Probe("test_reads")]).run(result)
        self.assertEqual([], seen)
        if private_tree.is_private():
            self.assertEqual(1, len(result.errors), result.errors)
            self.assertIn("private-tree input missing", result.errors[0][1])
            self.assertEqual([], result.skipped)
        else:
            self.assertEqual(1, len(result.skipped), result.skipped)
            self.assertEqual([], result.errors)


# A skip call of a kit self-test, and a path predicate of its own within two
# lines of it: the second mechanism beside `require` (re-check v2-fix-x A-M2).
SKIP_CALL = re.compile(r"\bskip(Test|Unless|If)\(|\bSkipTest\(")
PATH_PREDICATE = re.compile(
    r"\.(is_dir|exists|is_file)\(\)|\btravels\(\)|os\.path\.(isdir|exists|isfile)\(")


def adhoc_path_skips(tests):
    """`<file>:<line>` of every skip in `tests/test_*.py` that decides on a path
    predicate of its own. `/proc` is a fact of the host, not of the tree."""
    found = []
    for path in sorted(pathlib.Path(tests).glob("test_*.py")):
        lines = path.read_text(encoding="utf-8").splitlines()
        for i, line in enumerate(lines):
            if not SKIP_CALL.search(line) or line.lstrip().startswith("#"):
                continue
            window = "\n".join(lines[max(0, i - 2):i + 2])
            if PATH_PREDICATE.search(window) and "/proc" not in window:
                found.append("%s:%d" % (path.name, i + 1))
    return found


class OneMechanism(unittest.TestCase):
    """GH #1199 A-M2: a test that skips on its own path check goes quiet in the
    private tree when the file moves; `require` fails there instead."""

    def test_no_kit_self_test_skips_on_a_path_check_of_its_own(self):
        self.assertEqual([], adhoc_path_skips(pathlib.Path(__file__).parent))

    def test_the_scan_sees_each_form_of_a_path_skip(self):
        root = pathlib.Path(tempfile.mkdtemp(prefix="private-tree-scan-"))
        # the forms spelled apart, so this file does not carry them itself
        skip, unless = "self.skip" + "Test", "@unittest.skip" + "Unless"
        raise_skip = "raise unittest.Skip" + "Test"
        (root / "test_forms.py").write_text("\n".join((
            "    if not (REPO / 'plans').is_dir():",
            "        %s('x')" % skip,
            "%s(travels(), 'x')" % unless,
            "    if not os.path.isfile(p): %s('x')" % skip,
            "", "", "",
            "    if not open('/proc/pressure/cpu').read(): %s('x')" % skip,
            "    private_tree.require('plans/x.md')",
            "", "", "",
            "def setUpModule():",
            "    if not TOOLS.is_dir():",
            "        %s('x')" % raise_skip, "")), encoding="utf-8")
        self.assertEqual(["test_forms.py:2", "test_forms.py:3", "test_forms.py:4",
                          "test_forms.py:15"],
                         adhoc_path_skips(root))


class TheRealTree(unittest.TestCase):

    def test_the_marker_is_the_export_script_and_never_travels(self):
        """In the private tree the marker is the export audit, and the audit's
        own R2 rule forbids it -- so the export tree never carries it and the
        private tree always does. In the export tree there is nothing to read."""
        private_tree.require(private_tree.MARKER)
        path = private_tree.REPO / private_tree.MARKER
        spec = importlib.util.spec_from_file_location("make_export_1199", path)
        me = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(me)
        self.assertTrue(me.is_forbidden(private_tree.MARKER))
        self.assertNotIn(private_tree.MARKER, me.ROOT_FILES)
        self.assertTrue(private_tree.is_private())


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


if __name__ == "__main__":
    unittest.main()
