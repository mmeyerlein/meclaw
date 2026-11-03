"""GH #1199: the one mechanism for kit self-tests that read the private tree.

`private_tree.require` skips only where the export definition is absent (the
export tree) and fails, never skips, where it is present (the private tree).
These cases pin both halves on throw-away trees, and on the real tree that the
marker is the export script and never travels.
"""
import ast
import importlib.util
import pathlib
import re
import sys
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


# V8 fix round 1 (#1405, review I2): the release's private `gate-selftest`
# keeps only the modules that name `private_tree`
# (`gate_plan.private_tree_selftest_modules`); every other module runs in the
# export tree only. Three modules read `workshop/` or `plans/export-fixtures/`
# with a silent empty fallback (`rglob`/`os.walk` on a missing directory is
# `[]`, an `is_file()` guard drops the file) -- green there, checking nothing,
# and gone from the private run. The scan: a read call (`rglob`, `os.walk`,
# `is_file`, ...; or a module function that reads) on a path built from the
# repo root and a private root (a literal, or a name bound to one).
PRIVATE_ROOT = re.compile(r"^(?:workshop|plans|ideas|archive|AGENTS\.md|docs)(?:/|$)")
REPO_NAMES = {"REPO", "REPO_ROOT"}
READ_CALLS = {"rglob", "glob", "iterdir", "is_file", "is_dir", "exists", "read_text",
              "walk", "listdir", "open"}


def _names(node):
    out = {n.id for n in ast.walk(node) if isinstance(n, ast.Name)}
    out |= {n.attr for n in ast.walk(node) if isinstance(n, ast.Attribute)
            and isinstance(n.value, ast.Name) and n.value.id in ("self", "cls")}
    return out


def _private_literal(node):
    return any(isinstance(n, ast.Constant) and isinstance(n.value, str)
               and PRIVATE_ROOT.match(n.value) for n in ast.walk(node))


def _callee(call):
    f = call.func
    return f.attr if isinstance(f, ast.Attribute) else f.id if isinstance(f, ast.Name) else None


def silent_private_reads(text):
    """Line numbers of `text` where a read call reaches the repo root's private
    trees. Names bound to a private-root literal (constants, loop variables,
    functions that spell one) are followed to a fixpoint."""
    tree = ast.parse(text)
    local = {n.name: n for n in ast.walk(tree)
             if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef))}
    tainted = {name for name, fn in local.items() if _private_literal(fn)}
    grew = True
    while grew:
        grew = False
        for node in ast.walk(tree):
            if isinstance(node, (ast.Assign, ast.AnnAssign)) and node.value is not None:
                source = node.value
                targets = node.targets if isinstance(node, ast.Assign) else [node.target]
            elif isinstance(node, (ast.For, ast.comprehension)):
                source, targets = node.iter, [node.target]
            else:
                continue
            if not (_private_literal(source) or _names(source) & tainted):
                continue
            for name in set().union(*map(_names, targets)) - tainted - REPO_NAMES:
                tainted.add(name)
                grew = True

    def reads(fn):
        return any(isinstance(c, ast.Call) and _callee(c) in READ_CALLS for c in ast.walk(fn))

    out = set()
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        names = _names(node)
        if not (names & REPO_NAMES and (_private_literal(node) or names & tainted)):
            continue
        name = _callee(node)
        if name in READ_CALLS or (name in tainted and name in local and reads(local[name])):
            out.add(node.lineno)
    return sorted(out)


def undeclared_private_reads(repo):
    """`module:line` of every gate-selftest module outside the release's
    private list whose code reads the private tree (`silent_private_reads`)."""
    sys.path.insert(0, str(pathlib.Path(repo) / "scripts"))
    import gate_plan
    stations = {s.name: s for s in gate_plan.plan(["scripts/gate_plan.py"], "integration")}
    modules = stations["gate-selftest"].cmds[0][4:]
    private = set(gate_plan.private_tree_selftest_modules(modules, str(repo)))
    found = []
    for mod in modules:
        if mod in private:
            continue
        path = pathlib.Path(repo, *mod.split(".")).with_suffix(".py")
        found += ["%s:%d" % (mod, n)
                  for n in silent_private_reads(path.read_text(encoding="utf-8"))]
    return found


class NoSilentPrivateRead(unittest.TestCase):
    """V8 fix round 1 (review I2): a module that reads the private tree
    declares it (`private_tree`), so the release keeps it in its private run
    and the export tree skips it by name instead of passing it empty."""

    def test_every_selftest_module_that_reads_the_private_tree_declares_it(self):
        self.assertEqual([], undeclared_private_reads(private_tree.REPO))

    def test_the_scan_sees_a_constant_a_loop_and_a_reading_helper(self):
        text = "\n".join((
            'TREES = ("scripts/tests", "workshop/evals/tests")',
            "def t1():",
            "    for tree in TREES:",
            "        list((REPO / tree).rglob('*.py'))",
            "ROOTS = ('scripts', os.path.join('plans', 'export-fixtures'))",
            "def t2():",
            "    for root in ROOTS:",
            "        os.walk(REPO / root)",
            "def files(root):",
            "    ett = root / 'plans' / 'x.py'",
            "    return [ett] if ett.is_file() else []",
            "def t3():",
            "    files(REPO)",
            "def fixture(root):",
            "    (root / 'plans' / 'w').mkdir(parents=True)",
            "    list((REPO / 'scripts').rglob('*.py'))",
            ""))
        self.assertEqual([4, 8, 13], silent_private_reads(text))


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
