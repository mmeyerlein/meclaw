"""GH #1151: no module-level `def test_*` in a tree unittest runs.

`unittest` collects `TestCase` methods only and silently ignores a
module-level `test_*` function, so a pytest-style test written by a builder
never runs and never goes red (measured: a deliberately failing
`def test_x(): assert False` left `gate-selftest` GREEN 8/8). A helper that
happens to start with `test_` is renamed instead of exempted -- the rule has no
allow list. Each finding is `path:line`.
"""
import ast
import pathlib
import tempfile
import unittest
from scripts.tests import store_isolation

REPO = pathlib.Path(__file__).resolve().parents[2]
# The trees the gate runs with `python3 -m unittest` (scripts/gate_plan.py).
TREES = ("scripts/tests", "workshop/evals/scenarios/tests", "workshop/evals/builder-scenarios/tests")


def module_level_tests(path, root=REPO):
    """`path:line name` of every module-level function unittest would skip."""
    tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    rel = path.relative_to(root)
    return ["%s:%d %s" % (rel, node.lineno, node.name) for node in tree.body
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
            and node.name.startswith("test_")]

def _is_main(node):
    return isinstance(node, ast.If) and "__main__" in ast.dump(node.test)


def classes_after_main(path, root=REPO):
    """`path:line name` of a class defined after `if __name__ == "__main__":` --
    run as a script the module calls `unittest.main()` before it exists
    (review Z-I1 R3 M4)."""
    tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    rel, seen = path.relative_to(root), False
    out = []
    for node in tree.body:
        seen = seen or _is_main(node)
        if seen and isinstance(node, ast.ClassDef):
            out.append("%s:%d %s" % (rel, node.lineno, node.name))
    return out


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()


def tearDownModule():  # noqa: N802 -- unittest's name
    store_isolation.restore()


class TestUnittestShape(unittest.TestCase):
    def test_no_module_level_test_functions(self):
        found = []
        for tree in TREES:
            for path in sorted((REPO / tree).rglob("test*.py")):
                found += module_level_tests(path)
        self.assertEqual([], found, "module-level test functions unittest never runs "
                         "(move them into a TestCase or rename the helper):\n  " + "\n  ".join(found))

    def test_no_class_after_the_main_block(self):
        found = []
        for tree in TREES:
            for path in sorted((REPO / tree).rglob("test*.py")):
                found += classes_after_main(path)
        self.assertEqual([], found, "classes after the main block:\n  " + "\n  ".join(found))

    def test_the_rule_sees_a_class_after_main(self):
        with tempfile.TemporaryDirectory() as d:
            probe = pathlib.Path(d) / "test_probe.py"
            probe.write_text("import unittest\nif __name__ == '__main__':\n    unittest.main()\n"
                             "class Late(unittest.TestCase):\n    pass\n", encoding="utf-8")
            self.assertEqual(["test_probe.py:4 Late"], classes_after_main(probe, pathlib.Path(d)))

    def test_the_rule_sees_a_pytest_style_test(self):
        with tempfile.TemporaryDirectory() as d:
            probe = pathlib.Path(d) / "test_probe.py"
            probe.write_text("import unittest\n\n\ndef test_red():\n    assert False\n", encoding="utf-8")
            self.assertEqual(["test_probe.py:4 test_red"], module_level_tests(probe, pathlib.Path(d)))

if __name__ == "__main__":
    unittest.main()
