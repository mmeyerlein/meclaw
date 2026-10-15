"""Required pins (GH #1199): a test that must run whenever a file is touched.

AUDIT-PRE14-001 (`cell_task_long_running_propagates_handler_task_panic_to_outer_join`)
did not run in the strand gate of v2-p1082 although `cell_task.rs` was in the
diff: the resolver planned `rdeps(meclaw-colony)`, and the coverage narrowing
replaced it by the parts the map saw -- the pin was not among them. The
mapping file -> test now lives in ONE table, `.config/required-pins.toml`; the
resolver plans its tests for every touched path a `paths` glob matches, and
the narrowing keeps them (they are floor terms, never `rdeps`).

`scripts/check_required_pins.py` holds the table and the prose together:
every marker `(required-pin: <id>)` in AGENTS.md / docs/development-rules.md
has its row and every row its marker, and every pinned test exists.
"""

import contextlib
import importlib.util
import io
import os
import pathlib
import sys
import tempfile
import unittest

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS))
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import gate_plan  # noqa: E402
from pipeline import narrow  # noqa: E402
from scripts.tests import store_isolation  # noqa: E402
from test_pipeline_impact import COVERAGE, NOW, Base  # noqa: E402
from test_pipeline_graph import FILES  # noqa: E402
from test_pipeline_narrow import filter_terms, git, metadata  # noqa: E402
import test_pipeline_impact  # noqa: E402

REPO = SCRIPTS.parent
CHECK = SCRIPTS / "check_required_pins.py"

CELL_TASK = "crates/meclaw-colony/src/cell_task.rs"
COLONY = "crates/meclaw-colony/src/colony.rs"
AUDIT_PRE14_001 = ("(binary_id(=meclaw-colony::phase) & test(=phase_10a_cell_task_lr::"
                   "cell_task_long_running_propagates_handler_task_panic_to_outer_join))")
HOT_PATH = "(binary_id(=meclaw-colony::misc) & test(/^colony_hot_path_panic_free::/))"


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()
    test_pipeline_impact._METRICS_URL.start()


def tearDownModule():  # noqa: N802 -- unittest's name
    test_pipeline_impact._METRICS_URL.stop()
    store_isolation.restore()


def load_check():
    spec = importlib.util.spec_from_file_location("check_required_pins", CHECK)
    module = importlib.util.module_from_spec(spec)
    written, sys.dont_write_bytecode = sys.dont_write_bytecode, True
    try:
        spec.loader.exec_module(module)
    finally:
        sys.dont_write_bytecode = written
    return module


class ThePlanCarriesTheRequiredPinsTest(unittest.TestCase):
    """The real tree: the two pins AGENTS.md names, planned by the resolver."""

    def select(self, paths):
        return gate_plan.test_select(paths, "strand", str(REPO))

    def test_a_cell_task_diff_plans_audit_pre14_001(self):
        sel = self.select([CELL_TASK])
        self.assertIn(AUDIT_PRE14_001, filter_terms(sel["filter"]))
        self.assertIn("meclaw-colony::phase", sel["binaries"])
        self.assertIn("required-pin:audit-pre14-001: %s" % CELL_TASK, sel["why"])

    def test_a_colony_diff_plans_the_hot_path_lock(self):
        sel = self.select([COLONY])
        self.assertIn(HOT_PATH, filter_terms(sel["filter"]))
        self.assertIn("meclaw-colony::misc", sel["binaries"])
        self.assertIn("required-pin:colony-hot-path-panic-free: %s" % COLONY, sel["why"])

    def test_a_diff_off_the_pinned_files_plans_no_pin(self):
        sel = self.select(["crates/meclaw-colony/src/drain.rs"])
        terms = filter_terms(sel["filter"])
        self.assertNotIn(AUDIT_PRE14_001, terms)
        self.assertNotIn(HOT_PATH, terms)
        self.assertEqual([w for w in sel["why"] if w.startswith("required-pin:")], [])

    def test_a_diff_on_the_table_plans_every_pin(self):
        terms = filter_terms(self.select([".config/required-pins.toml"])["filter"])
        self.assertIn(AUDIT_PRE14_001, terms)
        self.assertIn(HOT_PATH, terms)

    def test_the_terms_view_names_the_pin_as_the_reason(self):
        terms = gate_plan.test_terms([CELL_TASK], str(REPO))
        self.assertIn("required-pin:audit-pre14-001 file:%s" % CELL_TASK,
                      terms.get(AUDIT_PRE14_001, []))

    def test_a_pin_term_reaches_no_browser_unit(self):
        # An unknown term shape reaches every browser unit (fail-closed) and
        # would plan `browser:display` for a pin diff; the pin form is known.
        self.assertFalse(gate_plan._selects_unit(
            AUDIT_PRE14_001, "meclaw-cells", "710_the_sheet_holds_in_both_engines_browser", None))
        self.assertFalse(gate_plan._selects_unit(
            AUDIT_PRE14_001, "meclaw-colony", "misc/colony_hot_path_panic_free", None))
        self.assertTrue(gate_plan._selects_unit(
            AUDIT_PRE14_001, "meclaw-colony", "phase/phase_10a_cell_task_lr", None))


PIN_TOML = """\
[[pin]]
id = "store-pin"
paths = ["crates/alpha/src/store.rs"]
why = "The template test must run whenever the store moves."

[[pin.tests]]
file = "crates/alpha/tests/a2_template.rs"
fn = "t"
"""
PIN_TERM = "(binary_id(=alpha::a2_template) & test(=t))"
CHANGED = ["crates/alpha/src/store.rs"]


class Narrowed(Base):
    def setUp(self):
        super().setUp()
        git(self.root, "init", "-q", "-b", "master")
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", "map rev")
        self.cov = dict(COVERAGE, rev=git(self.root, "rev-parse", "HEAD"))

    def run_narrow(self):
        floor = gate_plan.test_select(CHANGED, "strand", self.root)
        sel, _notes = narrow.narrow(floor, CHANGED, self.root, self.doc, self.cov, None,
                                    now=NOW, metadata=metadata(self.root))
        self.assertNotIn("alpha", sel["rdeps"])   # the map did replace rdeps(alpha)
        return sel


class TheNarrowingDropsAnUnpinnedTestTest(Narrowed):
    """The hole of v2-p1082 in the synthetic tree: without the table the map's
    parts replace rdeps(alpha), and a2_template is gone."""

    def test_without_the_table_the_narrowing_drops_the_test(self):
        self.assertNotIn("alpha::a2_template", self.run_narrow()["binaries"])


class TheNarrowingKeepsTheRequiredPinTest(Narrowed):
    files = dict(FILES, **{".config/required-pins.toml": PIN_TOML})

    def test_a_pin_survives_the_coverage_narrowing(self):
        sel = self.run_narrow()
        self.assertIn(PIN_TERM, filter_terms(sel["filter"]))
        self.assertIn("alpha::a2_template", sel["binaries"])


AGENTS = "# Rules\n\n- A rule with its pin (required-pin: p-one).\n"
TABLE = """\
[[pin]]
id = "p-one"
paths = ["crates/x/src/a.rs"]
why = "The pinned test guards a.rs."

[[pin.tests]]
file = "crates/x/tests/h/m.rs"
fn = "pinned"
"""
TREE = {
    "crates/x/tests/h/main.rs": "pub mod m;\n",
    "crates/x/tests/h/m.rs": "/// The pin.\n#[test]\nfn pinned() {}\n\nfn helper() {}\n",
    "AGENTS.md": AGENTS,
    "docs/development-rules.md": "# Development rules\n",
    ".config/required-pins.toml": TABLE,
}


class CheckRequiredPinsTest(unittest.TestCase):
    def setUp(self):
        self.mod = load_check()
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = pathlib.Path(self._tmp.name)

    def tree(self, drop=()):
        for rel, text in TREE.items():
            if rel in drop:
                continue
            p = self.root / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(text, encoding="utf-8")

    def run_check(self, root=None):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = self.mod.main(["--repo", str(root or self.root)])
        return code, out.getvalue()

    def test_a_clean_tree_is_green(self):
        self.tree()
        code, out = self.run_check()
        self.assertEqual(code, 0, out)

    def test_a_marker_without_a_row_is_red(self):
        self.tree(drop=("AGENTS.md",))
        (self.root / "AGENTS.md").write_text(AGENTS + "- Another (required-pin: p-two).\n",
                                             encoding="utf-8")
        code, out = self.run_check()
        self.assertEqual(code, 1, out)
        self.assertIn("AGENTS.md:4:", out)
        self.assertIn("p-two", out)

    def test_a_row_without_a_marker_is_red(self):
        self.tree()
        (self.root / "AGENTS.md").write_text("# Rules\n", encoding="utf-8")
        code, out = self.run_check()
        self.assertEqual(code, 1, out)
        self.assertIn(".config/required-pins.toml:2:", out)
        self.assertIn("p-one", out)

    def test_a_test_that_does_not_exist_is_red(self):
        self.tree()
        (self.root / ".config/required-pins.toml").write_text(
            TABLE.replace('fn = "pinned"', 'fn = "gone"'), encoding="utf-8")
        code, out = self.run_check()
        self.assertEqual(code, 1, out)
        self.assertIn("crates/x/tests/h/m.rs", out)
        self.assertIn("gone", out)

    def test_a_function_that_is_no_test_is_red(self):
        self.tree()
        (self.root / ".config/required-pins.toml").write_text(
            TABLE.replace('fn = "pinned"', 'fn = "helper"'), encoding="utf-8")
        code, out = self.run_check()
        self.assertEqual(code, 1, out)
        self.assertIn("helper", out)

    def test_a_file_that_is_no_test_target_is_red(self):
        self.tree()
        (self.root / ".config/required-pins.toml").write_text(
            TABLE.replace("crates/x/tests/h/m.rs", "crates/x/tests/h/gone.rs"), encoding="utf-8")
        code, out = self.run_check()
        self.assertEqual(code, 1, out)
        self.assertIn("crates/x/tests/h/gone.rs", out)

    def test_a_row_without_why_or_paths_is_red(self):
        self.tree()
        (self.root / ".config/required-pins.toml").write_text(
            TABLE.replace('paths = ["crates/x/src/a.rs"]\n', "").replace(
                'why = "The pinned test guards a.rs."\n', ""), encoding="utf-8")
        code, out = self.run_check()
        self.assertEqual(code, 1, out)
        self.assertIn("paths", out)
        self.assertIn("why", out)

    def test_the_public_tree_skips_the_marker_half_by_name(self):
        # AGENTS.md and docs/development-rules.md do not travel; the tests do.
        self.tree(drop=("AGENTS.md", "docs/development-rules.md"))
        code, out = self.run_check()
        self.assertEqual(code, 0, out)
        self.assertIn("skip", out)

    def test_the_real_tree_is_green(self):
        code, out = self.run_check(REPO)
        self.assertEqual(code, 0, out)


if __name__ == "__main__":
    unittest.main()
