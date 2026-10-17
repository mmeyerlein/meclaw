"""Unit tests for the gate-station resolver (`scripts/gate_plan.py`).

The resolver is the ONE place that decides which gate station runs for which
diff, so every rule in its table gets a test here. Fixtures are path lists;
the rules that grep test sources build a tiny throw-away repo instead of
reading the real tree, so the expectations stay stable.
"""

import collections
import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile
import types
import unittest
from unittest import mock
from scripts.tests import private_tree, store_isolation

HERE = pathlib.Path(__file__).resolve().parent
SCRIPTS = HERE.parent
REPO = SCRIPTS.parent
sys.path.insert(0, str(SCRIPTS))

import gate_plan as gp  # noqa: E402
from scripts.tests.test_pipeline_executor import ExecutorCase  # noqa: E402
from scripts.tests.test_pipeline_executor import station as ex_station  # noqa: E402


def mini_repo(case):
    """A throw-away tree with the four test sources the grep rules need.

    It has no scenario-class target, so a `tests` scope over it subtracts
    nothing (`gate_plan.scenario_for`, GH #934).
    """
    tmp = tempfile.TemporaryDirectory()
    case.addCleanup(tmp.cleanup)
    root = pathlib.Path(tmp.name)
    tests = root / "crates" / "meclaw-cells" / "tests"
    tests.mkdir(parents=True)
    (root / "crates" / "meclaw-cells" / "Cargo.toml").write_text(
        '[package]\nname = "meclaw-cells"\nversion = "0.0.0"\n', encoding="utf-8")
    (tests / "gh1_uses_hive.rs").write_text(
        'let p = root.join("templates/memory-hive");\n', encoding="utf-8")
    (tests / "gh2_all.rs").write_text(
        'for t in shipped_templates(&root) {}\n', encoding="utf-8")
    (tests / "mock_openai.rs").write_text(
        'pub fn server() -> String { String::new() }\n', encoding="utf-8")
    (tests / "gh3_llm.rs").write_text(
        'mod mock_openai;\nfn main() {}\n', encoding="utf-8")
    (tests / "gh4_example.rs").write_text(
        'let p = "examples/organism/grow-1.json";\n', encoding="utf-8")
    return str(root)


def reader_repo(case, sources):
    """A throw-away tree whose `crates/meclaw-cells/tests/` holds `sources`."""
    tmp = tempfile.TemporaryDirectory()
    case.addCleanup(tmp.cleanup)
    root = pathlib.Path(tmp.name)
    tests = root / "crates" / "meclaw-cells" / "tests"
    tests.mkdir(parents=True)
    (root / "crates" / "meclaw-cells" / "Cargo.toml").write_text(
        '[package]\nname = "meclaw-cells"\n', encoding="utf-8")
    for stem, text in sources.items():
        (tests / (stem + ".rs")).write_text(text, encoding="utf-8")
    return str(root)


def by_name(stations):
    return {s.name: s for s in stations}


def ignored_cmd(stations):
    """The `browser:display` call that runs the `#[ignore]`d browser locks: since
    GH #1161 both engine parts do, over the same union of locks -- the first part's
    filter names every one of them."""
    cmds = [c for c in stations["browser:display"].cmds if "--run-ignored" in c]
    assert len(cmds) == 2 and cmds == stations["browser:display"].cmds, cmds
    return cmds[0]


def release_moved():
    """The browser units a release moves into `browser:display`: every one of the
    real tree outside BROWSER_LOCKS (those are lock terms of the union already).
    Read from `browser_units`, never from `_browser_reach`/`browser_moved`, so a
    release reach that loses a unit cannot move the expectation along with it
    (GH #1161 x #1162: `tests` is `all()` in a release and gives every unit up)."""
    return sorted(t for t, (c, u, _x) in gp.browser_units().items()
                  if not gp._browser_lock_term(c, u))


# The two browser units that walk `workshop/tools` for the drivers (744 every
# `.mjs`, gh1209_no_stage every `.mjs`/`.js`, at any depth): a driver of the
# class `display_browser` is such a file, so its diff plans them in
# `browser:display` beside the locks (stapel c2, OR-V2.v2-r3-planbrowser.2).
DRIVER_WALKERS = ["binary_id(=meclaw-cells::744_a_late_patch_does_not_take_the_drawing_back)",
                  "binary_id(=meclaw-cells::gh1209_no_stage_draws_a_filter_browser)"]



# tb-s10 A2 (re-check tb-s4-st-pin L-2): the scenario class spelled out -- 8 terms,
# 57 units of the tree (KR-TB-1 d) -- so a term that joins or leaves
# `gate_plan.SCENARIO_TERMS` (and with it the t1 exception of test-tier.sh)
# turns the release partition red instead of moving along with it.
SCENARIO_TERMS_PINNED = (
    "binary(/_demo$/)", "binary(/_demo_/)", "binary(/e2e/)",
    "binary(/^workshop_scenario$/)", "binary(/^slack_live$/)",
    "binary(/^gpt_live_live$/)",
    "binary(/^harness_real_cli_smoke$/)", "binary(/^audit_14_/)",
)
SCENARIO_UNITS_PINNED = 57
# The scenario terms whose binary never reaches the public clone (make_export.py
# BLOCKLIST): term -> (its source file, its units in the pin). A public tree
# expects the pin without them; the full tree expects all of it.
SCENARIO_PRIVATE_ONLY = {
    "binary(/^slack_live$/)": ("crates/meclaw-cells/tests/slack_live.rs", 1),
}


# Z8: the seven browser locks, spelled out -- a lock that drops out of
# BROWSER_LOCKS or of the browser station's selection turns a test red.
BROWSER_LOCK_IDS = sorted([
    "meclaw-cells::710_the_sheet_holds_in_both_engines_browser",
    "meclaw-cells::710_the_colony_holds_in_both_engines_browser",
    "meclaw-cells::gh867_a_display_runs_under_a_strict_csp_browser",
    "meclaw-cells::gh961_motion_by_target_state_browser",
    "meclaw-cells::gh1002_slow_3g_connects_browser",
    "meclaw-cells::gh1003_the_shell_marks_orientation_browser",
    "meclaw-cells::gh1028_a_new_window_keeps_the_others_in_place_browser",
])

class Classify(unittest.TestCase):
    def test_docs_only_has_no_cargo_station(self):
        """... but only while no test READS the file (rule 8)."""
        repo = reader_repo(self, {"unrelated": 'fn main() { let _ = 1; }\n'})
        st = gp.plan(["docs/memory.en.md", "README.md"], "strand", repo=repo)
        # `export-selftest` is pure Python: both paths ship (README.md is a
        # ROOT_FILES row, docs/ a DOCS_MAP source), V2-A13. It is planned from
        # `make_export.py` (the resolver's `_export_rules`), which never
        # travels: the export tree rightly plans no such station (GH #1199,
        # release 0.64.2, export-audit R15). Only that part depends on the
        # private tree -- "docs-only plans no cargo" holds in both (RC-KX M1).
        private = {"export-selftest"} if private_tree.is_private() else set()
        self.assertEqual(
            {s.name for s in st},
            {"precheck", "roadmap-anchors", "adr-anchors", "claims", "tree-rules",
             "corpus-committed", "corpus"} | private)
        self.assertFalse(any(s.cargo for s in st))

    def test_a_doc_its_test_reads_is_not_docs_only(self):
        """The counter-example: a test that opens the file is its drift lock."""
        repo = reader_repo(self, {
            "reads_the_doc": 'let text = repo("docs/x.md").read_to_string();\n',
            "unrelated": 'fn main() { let _ = 1; }\n'})
        st = by_name(gp.plan(["docs/x.md"], "strand", repo=repo))
        self.assertIn("tests", st)
        self.assertEqual(
            st["tests"].scope,
            "binary_id(=meclaw-cells::reads_the_doc)")
        self.assertTrue(st["tests"].cargo)

    def test_the_slow_lock_follows_the_tests_station(self):
        """GH #1046: wherever nextest runs, the slow lock reads its times right
        after it -- and nowhere else, and not in ci, where `tests` does not run."""
        names = [s.name for s in gp.plan(["crates/meclaw-cells/src/lib.rs"],
                                         "strand", repo=None)]
        self.assertIn("tests", names)
        self.assertEqual(names[names.index("tests") + 1], "slow-tests")
        st = by_name(gp.plan(["crates/meclaw-cells/src/lib.rs"], "strand", repo=None))
        self.assertEqual(st["slow-tests"].cmds, [["python3", "scripts/slow_tests.py"]])
        self.assertTrue(st["slow-tests"].run)
        ci = by_name(gp.plan(["crates/meclaw-cells/src/lib.rs"], "ci", repo=None))
        self.assertFalse(ci["slow-tests"].run)
        docs = by_name(gp.plan(["README.md"], "strand", repo=None))
        self.assertEqual("tests" in docs, "slow-tests" in docs)

    def test_the_station_table_names_the_slow_lock(self):
        """The docstring of `gate_plan.py` is THE station table (AGENTS.md,
        Gates und Tiers); a station missing there is a station nobody looks up
        (review I2 of GH #1046)."""
        rows = [ln for ln in gp.__doc__.splitlines()
                if ln.startswith("    slow-tests ")]
        self.assertEqual(len(rows), 1, "no `slow-tests` row in the station table")
        self.assertIn("right after `tests`", rows[0])

    def test_rule_8_covers_every_path_outside_crates(self):
        """Workflows, plans fixtures and prose all have readers."""
        repo = reader_repo(self, {
            "ci_lock": 'let y = repo(".github/workflows/ci.yml");\n',
            "corridor_lock": ('let f = "plans/phase-13.5-hive-transit-fixtures/'
                              'expected_route_body.txt";\n'),
            "contributing": 'assert!(doc.contains("CONTRIBUTING.md"));\n',
            "quiet": 'fn main() {}\n'})
        for path, stem in ((".github/workflows/ci.yml", "ci_lock"),
                           ("plans/phase-13.5-hive-transit-fixtures/"
                            "expected_route_body.txt", "corridor_lock"),
                           ("CONTRIBUTING.md", "contributing")):
            self.assertEqual(
                gp.test_filter([path], "ci", repo=repo),
                "binary_id(=meclaw-cells::%s)" % stem, path)

    def test_an_english_doc_also_selects_readers_of_its_public_name(self):
        """The export maps `docs/X.en.md` onto `docs/X.md`; tests cite both."""
        repo = reader_repo(self, {
            "cites_public": 'let p = "docs/config.md";\n',
            "cites_source": 'let p = "docs/config.en.md";\n'})
        self.assertEqual(
            gp.test_filter(["docs/config.en.md"], "ci", repo=repo),
            "binary_id(=meclaw-cells::cites_public) "
            "+ binary_id(=meclaw-cells::cites_source)")

    def test_rust_src_in_colony_runs_rdeps_minus_scenario(self):
        st = by_name(gp.plan(["crates/meclaw-colony/src/route.rs"], "strand", repo=None))
        # tb-s1c F1: the tests that walk `crates` or `crates/meclaw-colony/src`
        # read this `.rs` too -- rule 8 runtime, fail-closed wider than the bare
        # rdeps of before; the exact set, so an over-selection shows as well
        walkers = [
            # walks `crates` (text lock of the per-turn lane)
            "(binary_id(=meclaw-cells::misc_5) & test(/^gh523_the_per_turn_lane_is_not_a_batch::/))",
            # walks `crates/meclaw-colony/src`
            "(binary_id(=meclaw-cells::web_1) & test(/^canvy2_position_migration::/))",
            # walks `crates` (corpus claims)
            "(binary_id(=meclaw-colony::gh_1) & test(/^gh230_corpus_constraint_claims::/))",
            # walks `crates/*/src` (GH #1209: no word of the test stage's filter
            # switch in product code) -- a static read, kept out of the browser
            # units, so a Rust diff does not book the browser station for it
            "(binary_id(=meclaw-cells::misc_2) & test(/^gh1209_the_product_carries_no_test_switch::/))",
            # `"/egon/../.."` (a URL probe) lands on `crates` from the crate
            # base (tb-s1c-b): an over-selection, allowed
            "binary_id(=meclaw-cells::web_2)",
            # walks `crates` (GH #867: every HTML page a crate serves has its lock)
            "binary_id(=meclaw-cells::gh867_no_surface_page_carries_unlisted_inline_script)",
            # lists `crates` for every crate's `tests` (GH #1209: no reader takes
            # one page of the log for the whole) and reads `meclaw-colony/src/colony.rs`:
            # for `route.rs` an over-selection, allowed
            "binary_id(=meclaw-cells::809_a_restart_reads_once_and_draws_the_same_screen)",
        ]
        self.assertEqual(sorted(walkers),
                         sorted(gp._tests_reading_at_runtime(None, "crates/meclaw-colony/src/route.rs")))
        # GH #1161 (re-check M-3): `tests` subtracts the browser units too, and
        # `rdeps(meclaw-colony)` reaches them (meclaw-cells), so they run booked
        # in `browser:display`
        # a walker that is a browser unit (GH #1209) runs booked there, not in `tests`
        browser = [w for w in walkers if w.endswith("_browser)")]
        self.assertEqual(st["tests"].scope, "%s - (%s)" % (
            " + ".join(sorted([w for w in walkers if w not in browser]
                              + ["rdeps(meclaw-colony)"])), gp.tests_minus_for(None)))
        for w in browser:
            self.assertIn(w[len("binary_id(=meclaw-cells::"):-1], " ".join(
                [st["browser:display"].scope] + [str(c) for c in st["browser:display"].cmds]))
        self.assertTrue(gp.tests_minus_for(None).startswith(gp.scenario_for(None) + " + "))
        self.assertIn("browser:display", st)
        self.assertIn("-p", st["clippy"].cmds[0])
        self.assertIn("meclaw-colony", st["clippy"].cmds[0])
        self.assertIn("unwrap-budget", st)
        self.assertNotIn("corridor", st)
        self.assertNotIn("scenarios:memory", st)

    def test_colony_rs_adds_corridor(self):
        names = {s.name for s in gp.plan(
            ["crates/meclaw-colony/src/colony.rs"], "strand", repo=None)}
        self.assertIn("corridor", names)

    def test_dot_directories_survive_normalisation(self):
        """`.github/` and `.cargo/` keep their leading dot -- only `./` is noise."""
        self.assertIn("corridor", gp.classify([".github/fixtures/expected_route_body.txt"]))
        self.assertIn("workspace", gp.classify([".cargo/config.toml"]))
        self.assertIn("workspace", gp.classify(["./Cargo.lock"]))
        self.assertIn("ci", gp.classify([".github/workflows/ci.yml"]))

    def test_test_helper_dirs_count_as_source(self):
        cls = gp.classify(["crates/meclaw-cells/tests/common/mod.rs"])
        self.assertIn("rust_src", cls)
        self.assertNotIn("rust_test", cls)

    def test_single_test_file_runs_its_binary_and_sharers(self):
        repo = mini_repo(self)
        st = by_name(gp.plan(
            ["crates/meclaw-cells/tests/mock_openai.rs"], "strand", repo=repo))
        self.assertEqual(
            st["tests"].scope,
            "binary_id(=meclaw-cells::gh3_llm) + binary_id(=meclaw-cells::mock_openai)")

    def _harness_repo(self):
        """A folded harness `misc` (scripts/harness_fold.py) beside a flat file."""
        repo = mini_repo(self)
        h = pathlib.Path(repo) / "crates" / "meclaw-cells" / "tests" / "misc"
        h.mkdir()
        (h / "main.rs").write_text(
            '#[path = "../support/road.rs"]\npub mod road;\npub mod gh5_folded;\n',
            encoding="utf-8")
        (h / "gh5_folded.rs").write_text(
            '#[test]\nfn reads() { let p = "docs/config.md"; }\n', encoding="utf-8")
        sup = h.parent / "support"
        sup.mkdir()
        (sup / "road.rs").write_text("pub fn walk() {}\n", encoding="utf-8")
        return repo

    def test_gate_plan_a_folded_test_file_runs_its_module_only(self):
        """Rule 3 on `tests/<harness>/<stem>.rs`: the harness binary, narrowed to
        the module the file became -- `<stem>::` is the prefix of its tests."""
        repo = self._harness_repo()
        st = by_name(gp.plan(
            ["crates/meclaw-cells/tests/misc/gh5_folded.rs"], "strand", repo=repo))
        self.assertEqual(st["tests"].scope,
                         "(binary_id(=meclaw-cells::misc) & test(/^gh5_folded::/))")

    def test_gate_plan_a_harness_main_runs_the_whole_harness(self):
        repo = self._harness_repo()
        st = by_name(gp.plan(
            ["crates/meclaw-cells/tests/misc/main.rs"], "strand", repo=repo))
        self.assertEqual(st["tests"].scope, "binary_id(=meclaw-cells::misc)")

    def test_gate_plan_a_path_read_inside_a_harness_names_the_module(self):
        """Rule 8 reads the folded file and names its module, not a ghost
        `binary_id(=meclaw-cells::gh5_folded)` nextest would refuse."""
        repo = self._harness_repo()
        self.assertEqual(gp._tests_reading_path(repo, "docs/config.md"),
                         {"(binary_id(=meclaw-cells::misc) & test(/^gh5_folded::/))"})

    def test_gate_plan_a_shared_support_file_runs_the_harness_that_paths_it(self):
        repo = self._harness_repo()
        self.assertIn("binary_id(=meclaw-cells::misc)",
                      gp._sharers(repo, "meclaw-cells", "road"))

    def test_gate_plan_a_folded_helper_runs_its_whole_harness(self):
        """A folded stem other stems of the harness use (`use crate::helper;`,
        `super::helper`) is a helper: editing it changes its users, whose test
        names do not start `helper::` -- so the whole harness runs (W5 delta C1:
        `llm/mock_responses.rs` ran 0 tests and none of its 4 users)."""
        repo = self._harness_repo()
        h = pathlib.Path(repo) / "crates" / "meclaw-cells" / "tests" / "misc"
        (h / "main.rs").write_text(
            '#[path = "../support/road.rs"]\npub mod road;\npub mod gh5_folded;\n'
            'pub mod helper;\npub mod gh6_user;\npub mod gh7_super;\npub mod tests_only;\n',
            encoding="utf-8")
        (h / "helper.rs").write_text("pub fn go() {}\n#[test]\nfn own() {}\n", encoding="utf-8")
        (h / "gh6_user.rs").write_text(
            "#[allow(unused_imports)] use crate::helper;\n#[test]\nfn u() { helper::go(); }\n",
            encoding="utf-8")
        (h / "gh7_super.rs").write_text("use super::tests_only::x;\n#[test]\nfn s() {}\n",
                                        encoding="utf-8")
        (h / "tests_only.rs").write_text("pub fn x() {}\n#[test]\nfn t() {}\n", encoding="utf-8")
        whole = "binary_id(=meclaw-cells::misc)"
        for stem in ("helper", "tests_only"):
            st = by_name(gp.plan(
                ["crates/meclaw-cells/tests/misc/%s.rs" % stem], "strand", repo=repo))
            self.assertEqual(st["tests"].scope, whole, stem)
        # a stem nobody uses stays narrowed to its module
        st = by_name(gp.plan(["crates/meclaw-cells/tests/misc/gh6_user.rs"], "strand", repo=repo))
        self.assertEqual(st["tests"].scope,
                         "(binary_id(=meclaw-cells::misc) & test(/^gh6_user::/))")

    def test_gate_plan_a_helper_pathed_in_from_a_foreign_harness_runs_that_harness(self):
        """A folded file another harness `#[path]`s in (`../<h>/<stem>.rs`) is
        compiled into THAT binary too. Rule 3 asked `_sharers` about the
        rewritten stem `<h>/main` and lost them (re-check N-C1, regression
        against 44b05caac). The three shapes of the real tree."""
        repo = mini_repo(self)
        tests = pathlib.Path(repo) / "crates" / "meclaw-cells" / "tests"
        layout = {
            "llm": ('pub mod mock_responses;\n#[path = "../misc_8/mock_oauth.rs"]\n'
                    'pub mod mock_oauth;\n', {"mock_responses": "pub fn r() {}\n"}),
            "misc_7": ('#[path = "../llm/mock_responses.rs"]\npub mod mock_responses;\n', {}),
            "misc_8": ("pub mod mock_oauth;\n", {"mock_oauth": "pub fn o() {}\n"}),
            "round_2": ("pub mod support_fitness;\n", {"support_fitness": "pub fn f() {}\n"}),
            "round_1": ('#[path = "../round_2/support_fitness.rs"]\npub mod support_fitness;\n', {}),
            "web_1": ('#[path = "../round_2/support_fitness.rs"]\npub mod support_fitness;\n', {}),
            "misc_2": ('#[path = "../round_2/support_fitness.rs"]\npub mod support_fitness;\n', {}),
        }
        for harness, (main, files) in layout.items():
            (tests / harness).mkdir(parents=True)
            (tests / harness / "main.rs").write_text(main, encoding="utf-8")
            for stem, text in files.items():
                (tests / harness / (stem + ".rs")).write_text(text, encoding="utf-8")
        cases = {
            "llm/mock_responses.rs": ["misc_7"],
            "misc_8/mock_oauth.rs": ["llm"],
            "round_2/support_fitness.rs": ["round_1", "web_1", "misc_2"],
        }
        for rel, users in cases.items():
            st = by_name(gp.plan(["crates/meclaw-cells/tests/" + rel], "strand", repo=repo))
            for user in users:
                self.assertIn("binary_id(=meclaw-cells::%s)" % user, st["tests"].scope, rel)

    def test_gate_plan_a_folded_stem_without_tests_runs_its_whole_harness(self):
        """A stem that runs no test of its own cannot be classified as a test
        module -- fail closed on the harness instead of a term over 0 tests."""
        repo = self._harness_repo()
        h = pathlib.Path(repo) / "crates" / "meclaw-cells" / "tests" / "misc"
        (h / "main.rs").write_text(
            '#[path = "../support/road.rs"]\npub mod road;\npub mod gh5_folded;\npub mod quiet;\n',
            encoding="utf-8")
        (h / "quiet.rs").write_text("pub fn q() {}\n", encoding="utf-8")
        st = by_name(gp.plan(["crates/meclaw-cells/tests/misc/quiet.rs"], "strand", repo=repo))
        self.assertEqual(st["tests"].scope, "binary_id(=meclaw-cells::misc)")

    def test_gate_plan_a_nested_mod_of_the_stem_runs_its_whole_harness(self):
        """`mod stem;` inside `mod a { … }` is `a::stem` -- a `stem::` term
        would select 0 tests in silence (OR-TB.S1.19, tb-s1 m3): only a
        top-level declaration folds, anything else runs the harness."""
        repo = self._harness_repo()
        h = pathlib.Path(repo) / "crates" / "meclaw-cells" / "tests" / "misc"
        (h / "main.rs").write_text(
            '#[path = "../support/road.rs"]\npub mod road;\n'
            'pub mod a {\n    pub mod gh5_folded;\n}\n', encoding="utf-8")
        st = by_name(gp.plan(["crates/meclaw-cells/tests/misc/gh5_folded.rs"],
                             "strand", repo=repo))
        self.assertEqual(st["tests"].scope, "binary_id(=meclaw-cells::misc)")

    def test_folded_unit_doubts_every_shape_but_a_plain_top_level_mod(self):
        """Fail closed (`<h>/main`) on a `#[cfg]`-gated, `#[path]`-redirected,
        commented-out or macro-wrapped `mod stem;` and on an unlexable
        `main.rs`; a lint attribute, a doc comment or `pub(crate)` still fold."""
        whole, folded = "misc/main", "misc/gh5_folded"
        cases = {
            '#[cfg(feature = "x")]\npub mod gh5_folded;\n': whole,
            '#[cfg_attr(unix, path = "u.rs")]\nmod gh5_folded;\n': whole,
            '#[path = "../elsewhere.rs"]\npub mod gh5_folded;\n': whole,
            '#[allow(dead_code)]\n#[cfg(test)]\npub mod gh5_folded;\n': whole,
            'pub mod gh5_folded;\n#[cfg(any())]\nmod gh5_folded;\n': whole,
            '/*\nmod gh5_folded;\n*/\n': whole,
            'macro_rules! m { () => {\nmod gh5_folded;\n} }\n': whole,
            'pub mod a {\n    use x::y;\n    mod gh5_folded;\n}\n': whole,
            'm! { x; mod gh5_folded; }\n': whole,
            'pub mod gh5_folded;\n"open\n': whole,
            'pub mod gh5_folded_two;\n': whole,
            '#[allow(clippy::duplicate_mod)]\npub mod gh5_folded;\n': folded,
            '/// the module\npub(crate) mod gh5_folded;\n': folded,
            '#![allow(unused)]\nuse x::y;\nmod gh5_folded;\n': folded,
            'pub mod road { pub fn f() {} }\npub mod gh5_folded;\n': folded,
        }
        for text, want in cases.items():
            self.assertEqual(gp._folded_unit(text, "misc", "gh5_folded"), want, text)

    def test_gate_plan_test_select_names_the_harness_binaries(self):
        """The pipeline builds only `binaries` (`pipeline/plan.py` -> `--test`);
        a narrowed harness term must name its harness there (W5 delta I1)."""
        repo = self._harness_repo()
        sel = gp.test_select(["crates/meclaw-cells/tests/misc/gh5_folded.rs",
                              "crates/meclaw-cells/tests/gh3_llm.rs"], "strand", repo=repo)
        self.assertEqual(sel["binaries"], ["meclaw-cells::gh3_llm", "meclaw-cells::misc"])

    def test_gate_plan_a_folded_file_exists_as_a_target(self):
        repo = self._harness_repo()
        self.assertTrue(gp._test_binary_exists(repo, "meclaw-cells", "misc/gh5_folded"))
        self.assertTrue(gp._test_binary_exists(repo, "meclaw-cells", "misc/main"))
        self.assertFalse(gp._test_binary_exists(repo, "meclaw-cells", "misc/gone"))
        self.assertIn("misc", gp._test_targets(repo))

    def test_a_deleted_test_file_emits_no_binary_id(self):
        """nextest HARD-ERRORS on a `binary_id(=..)` that matches no binary.

        `git diff --name-only` lists deletions, so a strand that removed a test
        file named a path with no binary behind it; the resulting filterset
        failed to parse and took the whole `tests` station down with it.
        """
        repo = mini_repo(self)
        st = by_name(gp.plan(
            ["crates/meclaw-cells/tests/gone_with_the_wave.rs"], "strand", repo=repo))
        self.assertNotIn("tests", st)
        # The path is still classified -- it just has nothing left to run.
        self.assertIn("fmt", st)

    def test_a_deleted_test_file_does_not_take_its_neighbours_with_it(self):
        repo = mini_repo(self)
        st = by_name(gp.plan(
            ["crates/meclaw-cells/tests/gone_with_the_wave.rs",
             "crates/meclaw-cells/tests/gh2_all.rs"], "strand", repo=repo))
        self.assertEqual(
            st["tests"].scope,
            "binary_id(=meclaw-cells::gh2_all)")

    def test_a_directory_test_target_counts_through_its_main_rs(self):
        repo = mini_repo(self)
        suite = pathlib.Path(repo) / "crates" / "meclaw-cells" / "tests" / "suite"
        suite.mkdir()
        (suite / "main.rs").write_text("fn main() {}\n", encoding="utf-8")
        (suite / "part.rs").write_text("// a module of the suite\n", encoding="utf-8")
        st = by_name(gp.plan(
            ["crates/meclaw-cells/tests/suite/part.rs"], "strand", repo=repo))
        self.assertEqual(
            st["tests"].scope,
            "binary_id(=meclaw-cells::suite)")

    def test_a_directory_without_a_main_rs_is_no_binary(self):
        repo = mini_repo(self)
        (pathlib.Path(repo) / "crates" / "meclaw-cells" / "tests" / "leftover").mkdir()
        st = by_name(gp.plan(
            ["crates/meclaw-cells/tests/leftover/notes.rs"], "strand", repo=repo))
        self.assertNotIn("tests", st)

    def test_template_diff_selects_referencing_tests_and_memory_suite(self):
        repo = mini_repo(self)
        st = by_name(gp.plan(
            ["templates/memory-hive/store/config.json"], "strand", repo=repo))
        self.assertEqual(
            st["tests"].scope,
            "binary_id(=meclaw-cells::gh1_uses_hive) + binary_id(=meclaw-cells::gh2_all)")
        self.assertIn("scenarios:memory", st)
        self.assertIn("recall-harness", st)
        self.assertIn("catalogue", st)
        self.assertEqual(len(st["corpus"].cmds), 3)
        self.assertEqual(st["corpus"].scope, "regenerate+check+librarian")
        self.assertNotIn("scenarios:builder", st)
        self.assertNotIn("fmt", st)
        self.assertNotIn("clippy", st)

    def test_other_template_selects_builder_suite(self):
        repo = mini_repo(self)
        st = by_name(gp.plan(["templates/assistant/config.json"], "strand", repo=repo))
        self.assertIn("scenarios:builder", st)
        self.assertNotIn("scenarios:memory", st)
        self.assertNotIn("recall-harness", st)
        self.assertEqual(
            st["tests"].scope,
            "binary_id(=meclaw-cells::gh2_all)")

    def test_display_scenarios_are_a_station_that_travels(self):
        """The pins of the one normative display document (development-rules § 10).

        The driver and its scenario file live under `templates/`, which is a public
        export root, so unlike the three `workshop/` suites this station also runs in
        the published tree -- it is NOT in CI_EXCLUDED. Its own class exists so that a
        diff which touches only the driver still plans it.
        """
        scen = "templates/display/compose/scenarios/scenarios.json"
        self.assertIn("display_scenarios", gp.classify([scen]))
        # It carries `template` too: the path lies under templates/.
        self.assertIn("template", gp.classify([scen]))

        st = by_name(gp.plan([scen], "strand", repo=None))
        self.assertIn("scenarios:display", st)
        self.assertFalse(st["scenarios:display"].cargo)
        # GH #1161: the four booked threads are the runner's workers.
        self.assertEqual(
            st["scenarios:display"].cmds,
            [["python3", "templates/display/compose/scenarios/"
              "run_display_scenarios.py", "--workers", "4"],
             ["python3", "scripts/display_sync.py", "--check-source"]])
        # The number stands in every GATE line and in every receipt.
        self.assertEqual(st["scenarios:display"].scope, "116 scenarios")

        # A diff on the driver alone plans it as well.
        driver = "templates/display/compose/scenarios/run_display_scenarios.py"
        self.assertIn("scenarios:display",
                      {s.name for s in gp.plan([driver], "strand", repo=None)})

        # I, R and C plan it for EVERY diff -- the driver and its scenarios travel
        # under templates/, so ci is the one place where the document is checked
        # against the tree that ships. Two seconds; nothing about that is worth
        # making conditional.
        for mode in ("integration", "release", "ci"):
            self.assertIn("scenarios:display",
                          {s.name for s in gp.plan(["docs/x.md"], mode, repo=None)},
                          mode)
        self.assertIn("scenarios:display",
                      {s.name for s in gp.plan([scen], "ci", repo=None)})
        self.assertNotIn("scenarios:display", gp.CI_EXCLUDED)

        order = gp.STATION_ORDER
        self.assertLess(order.index("scenarios:builder"),
                        order.index("scenarios:display"))
        self.assertLess(order.index("scenarios:display"),
                        order.index("recall-harness"))

    def test_display_lab_is_its_own_cheap_station(self):
        """The measuring library owes a contract, and a contract owes a station.

        `workshop/tools/display-lab/` is the one copy of the tools that read a
        running screen (befund `04-struktur.md` section 8). Its contract --
        inventory, the head with both traps, the mandatory `--port` -- is a
        unittest, and a unittest nothing plans is not a lock.
        """
        tool = "workshop/tools/display-lab/staterow.py"
        test = "scripts/tests/test_display_lab.py"
        self.assertIn("display_lab", gp.classify([tool]))
        self.assertIn("display_lab", gp.classify([test]))
        self.assertNotIn("display_lab", gp.classify(["workshop/tools/judge_eval.py"]))

        st = by_name(gp.plan([tool], "strand", repo=None))
        self.assertIn("display-lab", st)
        self.assertFalse(st["display-lab"].cargo, "a python unittest builds nothing")
        self.assertEqual(
            st["display-lab"].cmds,
            [["python3", "-m", "unittest", "scripts.tests.test_display_lab"]])

        # A diff that touches neither does not plan it.
        self.assertNotIn("display-lab",
                         {s.name for s in gp.plan(["docs/x.md"], "strand", repo=None)})
        # ci never plans it: workshop/ does not travel, so the library is not
        # in the tree the published mirror gates.
        self.assertIn("display-lab", gp.CI_EXCLUDED)
        self.assertNotIn("display-lab",
                         {s.name for s in gp.plan([tool], "ci", repo=None)})

        order = gp.STATION_ORDER
        self.assertLess(order.index("gate-selftest"), order.index("display-lab"))
        self.assertLess(order.index("display-lab"), order.index("fmt"))

    def test_llm_conformance_is_its_own_cheap_station(self):
        """The provider conformance tool owes a contract, and a contract owes a station.

        `workshop/tools/llm-conformance/` probes a model at its real endpoint
        (wave Haertung 5, GH #998); its offline tests run it against a fake
        provider. The cell's `SAMPLING_PARAMS` is mirrored in the tool, so a
        change to `translate.rs` plans the drift lock as well.
        """
        tool = "workshop/tools/llm-conformance/conformance.py"
        test = "scripts/tests/test_llm_conformance.py"
        cell = "crates/meclaw-cells/src/llm/translate.rs"
        for path in (tool, test, cell):
            self.assertIn("llm_conformance", gp.classify([path]), path)
        self.assertNotIn("llm_conformance",
                         gp.classify(["workshop/tools/judge_eval.py"]))

        st = by_name(gp.plan([tool], "strand", repo=None))
        self.assertIn("llm-conformance", st)
        self.assertFalse(st["llm-conformance"].cargo, "a python unittest builds nothing")
        self.assertEqual(
            st["llm-conformance"].cmds,
            [["python3", "-m", "unittest", "scripts.tests.test_llm_conformance"]])

        self.assertNotIn("llm-conformance",
                         {s.name for s in gp.plan(["docs/x.md"], "strand", repo=None)})
        self.assertIn("llm-conformance",
                      {s.name for s in gp.plan(["docs/x.md"], "integration", repo=None)})
        # ci never plans it: workshop/ does not travel.
        self.assertIn("llm-conformance", gp.CI_EXCLUDED)
        self.assertNotIn("llm-conformance",
                         {s.name for s in gp.plan([tool], "ci", repo=None)})

        order = gp.STATION_ORDER
        self.assertLess(order.index("display-lab"), order.index("llm-conformance"))
        self.assertLess(order.index("llm-conformance"), order.index("fmt"))

    def test_the_guide_harness_is_its_own_cheap_station(self):
        """The conversation guide's self-test pins its own wiring (GH #881).

        The fixture edge, the local engine and the rules tail its arms carry
        are checked by `run_guide.py --self-test` -- without a colony and
        without a cent. It stood red for a wave because no station planned it,
        and a self-test nothing plans is not a lock.
        """
        paths = ["workshop/evals/conversation-guide/run_guide.py",
                 "workshop/fixtures/positive/conversation-guide/setup.json",
                 "templates/memory-hive/inline-contract.md",
                 "workshop/evals/engine.py",
                 "workshop/tools/display-lab/fact_yield.py",
                 # L-7 (tb-s10): the #871 replay's unittest and the splitter
                 # its tool imports. Joined, not one literal: the published
                 # tree does not carry the replay test (export R2c).
                 "/".join(("scripts", "tests", "test_replay_871.py")),
                 "workshop/tools/display-lab/sidecar_turns.py",
                 # tv-bn M7b17: the workshop's log pager and its pin (joined
                 # for the same reason).
                 "workshop/evals/colony_log.py",
                 "/".join(("scripts", "tests", "test_colony_log.py"))]
        for path in paths:
            with self.subTest(path=path):
                self.assertIn("evals_guide", gp.classify([path]))
        self.assertNotIn("evals_guide", gp.classify(["workshop/tools/judge_eval.py"]))
        # A file may carry two classes: the fact counter is the library's too.
        self.assertIn("display_lab",
                      gp.classify(["workshop/tools/display-lab/fact_yield.py"]))

        cmd = [["python3", "workshop/evals/conversation-guide/run_guide.py", "--self-test"],
               ["python3", "-m", "unittest", "scripts.tests.test_replay_871"],
               ["python3", "-m", "unittest", "scripts.tests.test_colony_log"]]
        for path in paths:
            with self.subTest(planned_by=path):
                st = by_name(gp.plan([path], "strand", repo=None))
                self.assertIn("guide-selftest", st)
                self.assertFalse(st["guide-selftest"].cargo,
                                 "a self-test without a colony builds nothing")
                self.assertEqual(cmd, st["guide-selftest"].cmds)

        self.assertNotIn("guide-selftest",
                         {s.name for s in gp.plan(["docs/x.md"], "strand", repo=None)})
        # ci never plans it: workshop/ does not travel.
        self.assertIn("guide-selftest", gp.CI_EXCLUDED)
        self.assertNotIn("guide-selftest",
                         {s.name for s in gp.plan([paths[0]], "ci", repo=None)})

        order = gp.STATION_ORDER
        self.assertLess(order.index("display-lab"), order.index("guide-selftest"))
        self.assertLess(order.index("guide-selftest"), order.index("fmt"))

    def test_the_library_pulls_nothing_but_its_own_station(self):
        """A measuring tool is not a layout driver.

        `display_browser` is `workshop/tools/display-*`, and the library lies
        under that prefix -- so every file of it used to drag the browser locks
        and the Chromium/WebKit run behind a five-second unittest. The longer
        prefix wins, and a `.sh` of the library still carries `shell` because
        shellcheck is the thing that reads it.
        """
        tool = "workshop/tools/display-lab/runline.sh"
        self.assertEqual({"display_lab", "shell"}, gp.classify([tool]))
        self.assertEqual({"display_lab"},
                         gp.classify(["workshop/tools/display-lab/staterow.py"]))
        # the driver itself keeps its class
        self.assertIn("display_browser",
                      gp.classify(["workshop/tools/display-browser-lab.mjs"]))

        planned = {s.name for s in gp.plan([tool], "strand", repo=None)}
        self.assertIn("display-lab", planned)
        self.assertNotIn("browser:display", planned)

    def test_shellcheck_reads_the_library(self):
        """The class plans the station; the globs decide what it looks at.

        `runline.sh` carries `shell`, so the station runs -- but `SHELL_GLOBS`
        named three directories and `workshop/` was not among them, so the file
        that carries a `# shellcheck` directive was never read by shellcheck.
        """
        self.assertIn("workshop/tools/display-lab/*.sh", gp.SHELL_GLOBS)
        # ci runs on the published mirror, which has no workshop/ and no plans/
        self.assertEqual(("scripts/*.sh", ".github/gates/*.sh", "scripts/pipeline/*.sh"),
                         gp.SHELL_GLOBS[:gp.SHELL_GLOBS_PUBLIC])

    def test_every_class_is_in_the_register(self):
        """The docstring's CLASSES list is the only place the classes are written
        down, so a class missing from it does not exist for the next reader."""
        register = gp.__doc__.split("STATIONS")[0]
        for name in sorted(gp.classify(["workshop/tools/display-lab/runline.sh"])
                           | gp.classify(["scripts/gate_plan.py"])):
            with self.subTest(cls=name):
                self.assertIn("    %s " % name, register)

    def test_display_browser_is_a_station_that_stays_home(self):
        """The B-proofs of display-hive.md § 5-9, measured in Chromium and WebKit.

        Two shapes, one station name, like `export-audit`: the SHEET half needs no
        colony and takes about thirty seconds, so integration runs it; the COLONY half
        is six boots, six pages and six sets of gestures -- four to six minutes -- and
        only `release` pays for that. The scope column says which halves ran.

        Unlike `scenarios:display` this station is in CI_EXCLUDED: its driver lives in
        `workshop/tools/`, and `workshop/` is a FORBIDDEN_PREFIX of the export. In the
        published tree the driver is simply not there, the Rust locks would skip, and a
        station that can only ever skip is noise in the ci plan.
        """
        driver = "workshop/tools/display-layout-browser.mjs"
        lock = ("crates/meclaw-cells/tests/"
                "710_the_sheet_holds_in_both_engines_browser.rs")
        self.assertIn("display_browser", gp.classify([driver]))
        self.assertIn("display_browser", gp.classify([lock]))
        # The driver lies under workshop/, which carries no template class.
        self.assertNotIn("template", gp.classify([driver]))

        # A diff on the driver alone plans it in a strand, and it builds -- with
        # the two units that walk the drivers beside the locks (stapel c2).
        st = by_name(gp.plan([driver], "strand", repo=None))
        self.assertIn("browser:display", st)
        self.assertTrue(st["browser:display"].cargo)
        self.assertEqual(st["browser:display"].scope, "sheet")
        self.assertEqual(st["browser:display"].cmds,
                         gp.browser_cmds(colony=False, moved=DRIVER_WALKERS))
        for cmd in st["browser:display"].cmds:
            for term in DRIVER_WALKERS:
                self.assertIn(term, cmd[2])

        # So does a diff on the display template itself -- the sheet is what it measures.
        self.assertIn("browser:display",
                      {s.name for s in gp.plan(
                          ["templates/display/compose/display-dna.css"],
                          "strand", repo=None)})
        # And a diff that touches neither does not.
        self.assertNotIn("browser:display",
                         {s.name for s in gp.plan(["docs/x.md"], "strand", repo=None)})

        # Integration AND release: the colony half beside the sheet half, with
        # --run-ignored on the ignored lock. A proof only the release night reaches
        # is a proof nobody reads (GH #746), so the pass that declares a wave done
        # runs it too.
        both = gp.browser_cmds(colony=True)
        self.assertIn(gp.COLONY_LOCK, both[0][2])
        # A release runs `all()` and so reaches every browser unit (GH #1162): the
        # units outside the locks join the same union -- exactly that set.
        moved = release_moved()
        self.assertTrue(moved)
        self.assertEqual(moved, gp.browser_moved(["docs/x.md"], "release", repo=None))
        for mode in ("integration", "release"):
            st_ir = by_name(gp.plan(["docs/x.md"], mode, repo=None))
            self.assertEqual(st_ir["browser:display"].scope, "sheet+colony", mode)
            # Spelled out, never `gp.browser_cmds(...)` (tb-s10 A2, re-check
            # tb-s4-st-pin L-1): an expectation built by the function under test
            # moves with it -- a `browser_cmds` that drops a moved unit left this
            # assert green.
            union = " + ".join([gp.COLONY_LOCK] + list(gp.IGNORED_LOCKS)
                               + [gp.SHEET_LOCK]
                               + (moved if mode == "release" else []))
            self.assertEqual(st_ir["browser:display"].cmds, [
                ["scripts/test-tier.sh", "filter", "(%s) - test(/webkit/)" % union,
                 "--run-ignored", "all"],
                ["scripts/test-tier.sh", "filter", "(%s) & test(/webkit/)" % union,
                 "--run-ignored", "all"]], mode)
            if mode == "integration":
                self.assertEqual(both, st_ir["browser:display"].cmds)

        # A strand keeps the colony half out: six boots are a fifth of a whole pass.
        self.assertNotIn("710_the_colony_holds", str(st["browser:display"].cmds))

        # ci never plans it: workshop/ does not travel.
        self.assertIn("browser:display", gp.CI_EXCLUDED)
        self.assertNotIn("browser:display",
                         {s.name for s in gp.plan([driver], "ci", repo=None)})

        order = gp.STATION_ORDER
        self.assertLess(order.index("scenarios:display"),
                        order.index("browser:display"))
        self.assertLess(order.index("browser:display"),
                        order.index("recall-harness"))

    def test_browser_station_has_two_engine_parts(self):
        """GH #1161: the station runs TWO nextest calls side by side, one per engine.

        W7 made it two calls one after the other -- the ignored locks over the union
        of their binaries, then the sheet -- and every WebKit test held the whole call
        (469.6 s in the integration pass of 2026-10-10, over the 466.3 s target alone).
        Now one union of the locks AND the sheet, split by engine: the Chromium part is
        the union minus `test(/webkit/)`, the WebKit part the union and it, both with
        `--run-ignored all`. Every test is in exactly one part, and all WebKit tests are
        in the one part that runs them one at a time, so two WebKit pages never meet
        (GH #1048, the reason W7 kept the calls serial: review W7 I1, OR-DT.W7.3). The
        station carries `parallel` by cmd with one nextest profile per part. The TSV
        keeps one row per command. A release adds every browser unit outside the
        locks to the union (its `tests` is `all()` and gives them up, GH #1162);
        a strand on a driver the two units that walk the drivers (stapel c2)."""
        moved = release_moved()
        self.assertTrue(moved)
        for mode, colony in (("strand", False), ("integration", True),
                             ("release", True)):
            with self.subTest(mode=mode):
                diff = (["workshop/tools/display-layout-browser.mjs"]
                        if mode == "strand" else ["docs/x.md"])
                st = by_name(gp.plan(diff, mode, repo=None))
                cmds = st["browser:display"].cmds
                self.assertEqual(len(cmds), 2, cmds)
                union = " + ".join(([gp.COLONY_LOCK] if colony else [])
                                   + list(gp.IGNORED_LOCKS) + [gp.SHEET_LOCK]
                                   + (moved if mode == "release" else [])
                                   + (DRIVER_WALKERS if mode == "strand" else []))
                chromium, webkit = cmds
                self.assertEqual(chromium, ["scripts/test-tier.sh", "filter",
                                            "(%s) - test(/webkit/)" % union,
                                            "--run-ignored", "all"])
                self.assertEqual(webkit, ["scripts/test-tier.sh", "filter",
                                          "(%s) & test(/webkit/)" % union,
                                          "--run-ignored", "all"])
                self.assertEqual({"by": "cmd",
                                  "profiles": ["browser-chromium", "browser-webkit"]},
                                 st["browser:display"].parallel)

                doc = gp.to_json(diff, mode)
                rows = {r["name"]: r for r in doc["stations"]}
                self.assertEqual(rows["browser:display"]["cmds"], cmds)
                self.assertEqual([n for n, r in rows.items() if "parallel" in r],
                                 ["browser:display"])

                tsv = [ln.split("\t") for ln in gp.to_tsv(diff, mode).splitlines()]
                self.assertEqual(len([r for r in tsv if r[0] == "browser:display"]), 2)
                self.assertTrue(all(len(r) == 5 for r in tsv))

        # Every browser lock of BROWSER_LOCKS is a term of the union (the colony
        # half in I/R), the sheet among them.
        named = set(gp.IGNORED_LOCKS) | {gp.COLONY_LOCK, gp.SHEET_LOCK}
        for path in gp.BROWSER_LOCKS:
            stem = path.rsplit("/", 1)[1][:-len(".rs")]
            self.assertTrue(any(t[len("binary(/"):-len("/)")] in stem for t in named),
                            stem)

    def test_the_csp_lock_rides_the_browser_station(self):
        """GH #867: the CSP lock is a browser lock and runs where browser locks run.

        It is `#[ignore]`d in the tree so the `tests` station -- which builds every
        binary of a changed crate -- never drives a browser inside its own budget (the
        audio drivers tripped there under parallel load, GH #763). So the station has to
        name it, with `--run-ignored`, in every mode that plans the station at all: a
        lock nothing runs is a lock nobody reads. And its driver is a
        `workshop/tools/display-*` file, so a diff on the driver alone plans it.
        """
        lock = ("crates/meclaw-cells/tests/"
                "gh867_a_display_runs_under_a_strict_csp_browser.rs")
        driver = "workshop/tools/display-csp-browser.mjs"
        self.assertIn(lock, gp.BROWSER_LOCKS)
        self.assertIn("display_browser", gp.classify([lock]))
        self.assertIn("display_browser", gp.classify([driver]))
        self.assertIn(gp.CSP_LOCK, gp.IGNORED_LOCKS)
        self.assertIn("gh867_a_display_runs_under_a_strict_csp", gp.CSP_LOCK)
        for diff, mode in (([driver], "strand"), ([lock], "strand"),
                           (["templates/display/csp.json"], "strand"),
                           (["docs/x.md"], "integration"), (["docs/x.md"], "release")):
            with self.subTest(diff=diff, mode=mode):
                st = by_name(gp.plan(diff, mode, repo=None))
                self.assertIn(gp.CSP_LOCK, ignored_cmd(st)[2])
        # ci never plans the station: workshop/ does not travel.
        self.assertNotIn("browser:display",
                         {s.name for s in gp.plan([lock], "ci", repo=None)})

    def test_the_slow3g_lock_rides_the_browser_station(self):
        """GH #1002: the slow-link lock is a browser lock and runs where browser locks run.

        Like the CSP and the motion lock: `#[ignore]`d in the tree, named by the
        station with `--run-ignored` in every mode that plans it, and planned by a diff
        on its own driver -- a `workshop/tools/display-*` file, not the measuring
        library under `display-lab/`, which must not pull the station.
        """
        lock = ("crates/meclaw-cells/tests/"
                "gh1002_slow_3g_connects_browser.rs")
        driver = "workshop/tools/display-slow3g-browser.mjs"
        self.assertIn(lock, gp.BROWSER_LOCKS)
        self.assertIn("display_browser", gp.classify([lock]))
        self.assertIn("display_browser", gp.classify([driver]))
        self.assertIn(gp.SLOW3G_LOCK, gp.IGNORED_LOCKS)
        self.assertIn("gh1002_slow_3g_connects", gp.SLOW3G_LOCK)
        for diff, mode in (([driver], "strand"), ([lock], "strand"),
                           (["docs/x.md"], "integration"), (["docs/x.md"], "release")):
            with self.subTest(diff=diff, mode=mode):
                st = by_name(gp.plan(diff, mode, repo=None))
                self.assertIn(gp.SLOW3G_LOCK, ignored_cmd(st)[2])
        self.assertNotIn("browser:display",
                         {s.name for s in gp.plan([lock], "ci", repo=None)})

    def test_the_orientation_lock_rides_the_browser_station(self):
        """GH #1003: the orientation lock is a browser lock and runs where browser
        locks run; a diff on its driver or on the lock plans the station."""
        lock = ("crates/meclaw-cells/tests/"
                "gh1003_the_shell_marks_orientation_browser.rs")
        driver = "workshop/tools/display-orientation-browser.mjs"
        self.assertIn(lock, gp.BROWSER_LOCKS)
        self.assertIn("display_browser", gp.classify([lock]))
        self.assertIn("display_browser", gp.classify([driver]))
        self.assertIn(gp.ORIENTATION_LOCK, gp.IGNORED_LOCKS)
        self.assertIn("gh1003_the_shell_marks_orientation", gp.ORIENTATION_LOCK)
        for diff, mode in (([driver], "strand"), ([lock], "strand"),
                           (["docs/x.md"], "integration"), (["docs/x.md"], "release")):
            with self.subTest(diff=diff, mode=mode):
                st = by_name(gp.plan(diff, mode, repo=None))
                self.assertIn(gp.ORIENTATION_LOCK, ignored_cmd(st)[2])
        self.assertNotIn("browser:display",
                         {s.name for s in gp.plan([lock], "ci", repo=None)})

    def test_the_keep_lock_rides_the_browser_station(self):
        # GH #1028: a new window ahead of others leaves their DOM nodes in place.
        lock = ("crates/meclaw-cells/tests/"
                "gh1028_a_new_window_keeps_the_others_in_place_browser.rs")
        driver = "workshop/tools/display-keep-browser.mjs"
        self.assertIn(lock, gp.BROWSER_LOCKS)
        self.assertIn("display_browser", gp.classify([lock]))
        self.assertIn("display_browser", gp.classify([driver]))
        self.assertIn(gp.KEEP_LOCK, gp.IGNORED_LOCKS)
        for diff, mode in (([driver], "strand"), ([lock], "strand"),
                           (["docs/x.md"], "integration"), (["docs/x.md"], "release")):
            with self.subTest(diff=diff, mode=mode):
                st = by_name(gp.plan(diff, mode, repo=None))
                self.assertIn(gp.KEEP_LOCK, ignored_cmd(st)[2])
        self.assertNotIn("browser:display",
                         {s.name for s in gp.plan([lock], "ci", repo=None)})

    def test_the_motion_lock_rides_the_browser_station(self):
        """GH #961: the motion lock is a browser lock and runs where browser locks run.

        Like the CSP lock: `#[ignore]`d in the tree, named by the station with
        `--run-ignored` in every mode that plans it, and planned by a diff on its own
        driver. One boot and three pages per engine, so a strand pays for it.
        """
        lock = ("crates/meclaw-cells/tests/"
                "gh961_motion_by_target_state_browser.rs")
        driver = "workshop/tools/display-motion-browser.mjs"
        self.assertIn(lock, gp.BROWSER_LOCKS)
        self.assertIn("display_browser", gp.classify([lock]))
        self.assertIn("display_browser", gp.classify([driver]))
        self.assertIn(gp.MOTION_LOCK, gp.IGNORED_LOCKS)
        self.assertIn("gh961_motion_by_target_state", gp.MOTION_LOCK)
        for diff, mode in (([driver], "strand"), ([lock], "strand"),
                           (["templates/display/compose/display-dna.css"], "strand"),
                           (["docs/x.md"], "integration"), (["docs/x.md"], "release")):
            with self.subTest(diff=diff, mode=mode):
                st = by_name(gp.plan(diff, mode, repo=None))
                self.assertIn(gp.MOTION_LOCK, ignored_cmd(st)[2])
        self.assertNotIn("browser:display",
                         {s.name for s in gp.plan([lock], "ci", repo=None)})

    def test_example_diff_selects_referencing_tests(self):
        repo = mini_repo(self)
        st = by_name(gp.plan(["examples/organism/grow-1.json"], "strand", repo=repo))
        self.assertEqual(
            st["tests"].scope,
            "binary_id(=meclaw-cells::gh4_example)")
        self.assertIn("scenarios:builder", st)

    def test_workspace_class_runs_all(self):
        st = by_name(gp.plan(["Cargo.lock"], "strand", repo=None))
        # GH #1161 (re-check M-3): minus the scenario class AND every browser unit
        self.assertEqual(st["tests"].scope, "all() - (%s)" % gp.tests_minus_for(None))
        self.assertIn("browser:display", st)
        self.assertIn("--workspace", st["clippy"].cmds[0])
        self.assertIn("deny", st)

    def test_a_root_manifest_or_toolchain_diff_runs_the_workspace(self):
        """Re-check tb-s6c h2 L7: since tb-s6c the root `Cargo.toml` holds the
        feature set of every shared dependency ([workspace.dependencies]), yet
        dropping it from the `workspace` class stayed green in 495 tests -- a
        root-manifest diff would have planned no cargo station at all."""
        for path in ("Cargo.toml", "rust-toolchain.toml", ".cargo/config.toml"):
            with self.subTest(path=path):
                self.assertIn("workspace", gp.classify([path]))
                st = by_name(gp.plan([path], "strand", repo=None))
                # GH #1161 (re-check M-3): minus the scenario class AND every
                # browser unit, which run booked in `browser:display`
                self.assertEqual(st["tests"].scope, "all() - (%s)" % gp.tests_minus_for(None))
                self.assertIn("browser:display", st)
                self.assertIn("--workspace", st["clippy"].cmds[0])
                self.assertIn("deny", st)

    def test_last_run_json_is_ignored(self):
        noisy = gp.plan(list(gp.IGNORED), "strand", repo=None)
        empty = gp.plan([], "strand", repo=None)
        self.assertEqual([s.name for s in noisy], [s.name for s in empty])
        self.assertEqual(gp.classify(list(gp.IGNORED)), set())

    def test_empty_diff_floor_is_t0(self):
        """The floor is the TIER, not the equivalent filterset.

        `test-tier.sh t0` passes `--lib --bins`, so nextest builds the unit
        targets only. `filter 'kind(lib) + kind(bin)'` selects the same tests
        but compiles all 650+ test binaries first -- minutes of rustc for the
        cheapest station in the table.
        """
        self.assertEqual(gp.test_filter([], "strand", repo=None), gp.T0_FLOOR)
        st = by_name(gp.plan([], "strand", repo=None))
        self.assertEqual(st["tests"].scope, "t0 floor")
        self.assertEqual(st["tests"].cmds, [["scripts/test-tier.sh", "t0"]])
        self.assertTrue(st["tests"].cargo)

    def test_a_real_filterset_still_goes_through_filter(self):
        st = by_name(gp.plan(["crates/meclaw-colony/src/route.rs"], "strand",
                             repo=None))
        self.assertEqual(st["tests"].cmds[0][:2], ["scripts/test-tier.sh", "filter"])

    def test_no_rule_matches_means_no_tests_station(self):
        """A path nobody reads and no crate holds plans no tests at all."""
        repo = reader_repo(self, {"quiet": 'fn main() {}\n'})
        st = by_name(gp.plan([".github/workflows/ci.yml"], "strand", repo=repo))
        self.assertNotIn("tests", st)

    def test_integration_always_runs_the_unconditional_stations(self):
        st = by_name(gp.plan(["crates/meclaw-core/src/lib.rs"], "integration", repo=None))
        for name in ("scenarios:builder", "scenarios:display", "doctests",
                     "catalogue"):
            self.assertIn(name, st)
        self.assertIn("--workspace", st["clippy"].cmds[0])

    def test_integration_skips_the_memory_suites_and_deny_without_a_trigger(self):
        """R-P2: 43 runs of the three, 16 317 s, no finding (wave P, finding 04 § 3.4).

        A diff that names neither the memory hive, nor a recall source, nor the
        workspace cannot make any of them speak, so the pass pays 385 s per run
        for an answer it already knows.
        """
        st = by_name(gp.plan(["docs/x.md"], "integration", repo=None))
        for name in ("scenarios:memory", "recall-harness", "deny"):
            self.assertNotIn(name, st)

    def test_integration_runs_the_memory_suites_when_the_diff_asks(self):
        st = by_name(gp.plan(["workshop/evals/scenarios/cases/a.json"],
                             "integration", repo=None))
        self.assertIn("scenarios:memory", st)
        self.assertIn("recall-harness", st)

    def test_scenarios_memory_only_templates_when_the_template_is_the_only_reason(self):
        """W7 item 7: a diff on `templates/memory-hive/**` alone plans only the
        cases that name that template (`run_scenarios.py --only-templates`), in
        `strand` and `integration`. A diff on the suite itself (`evals_memory`)
        and the release run every case."""
        base = ["python3", "workshop/evals/scenarios/run_scenarios.py"]
        narrowed = base + ["--only-templates", "memory-hive"]
        hive = "templates/memory-hive/store/config.json"
        case = "workshop/evals/scenarios/cases/a.json"
        for diff, mode, want in (
                ([hive], "strand", narrowed),
                ([hive], "integration", narrowed),
                ([hive], "release", base),
                ([case], "strand", base),
                ([case], "integration", base),
                ([hive, case], "strand", base),
                ([hive, case], "integration", base),
                (["docs/x.md"], "release", base)):
            with self.subTest(diff=diff, mode=mode):
                st = by_name(gp.plan(diff, mode, repo=None))
                self.assertEqual(st["scenarios:memory"].cmds[-1], want)

    def test_scenario_runner_unit_tests_ride_with_a_suite_diff(self):
        """W7: the runners' own unit tests (workers, slots, case selection)
        run first in their station when the suite itself changed, and only
        then; a template-only diff plans the runner alone."""
        mem_ut = ["python3", "-m", "unittest", "discover", "-s",
                  "workshop/evals/scenarios/tests"]
        bld_ut = ["python3", "-m", "unittest", "discover", "-s", "tests", "-t", "."]
        st = by_name(gp.plan(["workshop/evals/scenarios/run_scenarios.py"],
                             "strand", repo=None))
        self.assertEqual(st["scenarios:memory"].cmds[0], mem_ut)
        self.assertEqual(len(st["scenarios:memory"].cmds), 2)
        st = by_name(gp.plan(
            ["workshop/evals/builder-scenarios/run_builder_scenarios.py"],
            "strand", repo=None))
        self.assertEqual(st["scenarios:builder"].cmds[0], bld_ut)
        self.assertEqual(st["scenarios:builder"].cwds,
                         ["workshop/evals/builder-scenarios"] * 2)
        st = by_name(gp.plan(["templates/memory-hive/store/config.json"],
                             "strand", repo=None))
        self.assertNotIn(mem_ut, st["scenarios:memory"].cmds)

    def test_integration_runs_only_the_recall_harness_for_a_recall_source(self):
        """The recall lane is the one trigger `recall-harness` owns alone.

        Before R-P2 the condition was dead weight -- the station ran in every
        integration pass anyway. Now it is the only thing that still asks about
        a change to the recall lane outside a release, so it needs its own
        pin (OR-P.rulings.2: the two memory stations do NOT share this
        trigger, because a source under `crates/meclaw-cells/src/` does not
        move the scenario cases).
        """
        st = by_name(gp.plan(["crates/meclaw-cells/src/recall.rs"],
                             "integration", repo=None))
        self.assertIn("recall-harness", st)
        self.assertNotIn("scenarios:memory", st)

    def test_the_recall_harness_code_plans_the_recall_harness_alone(self):
        """The harness' own scripts are checked by `answer_cases.py` (GH #878).

        Before #878 `judge.py`, `score.py` and `run_eval.py` classified into no
        class at all: a diff on them planned no station that reads them. The
        class `evals_recall` plans `recall-harness` with both commands and NOT
        `scenarios:memory` (46 cases, ~390 s, which never reads the harness).
        """
        self.assertIn("    evals_recall ", gp.__doc__.split("STATIONS")[0])
        for name in ("judge", "score", "run_eval", "answer_cases"):
            path = "workshop/evals/p5-longmemeval/%s.py" % name
            self.assertIn("evals_recall", gp.classify([path]), path)
            for mode in ("strand", "integration"):
                st = by_name(gp.plan([path], mode, repo=None))
                self.assertIn("recall-harness", st, (path, mode))
                self.assertEqual(st["recall-harness"].cmds,
                                 [["python3", "recall_cases.py"],
                                  ["python3", "answer_cases.py"],
                                  ["python3", "local_path_cases.py"],
                                  ["python3", "pipeline_cases.py"]], (path, mode))
                self.assertNotIn("scenarios:memory", st, (path, mode))

    def test_a_harness_readme_or_tool_is_not_harness_code(self):
        """Negative control: the class stops at the harness' own `*.py`.

        `*` crosses no `/`, so a file under `tools/` stays `evals_memory` only,
        and the README plans no `recall-harness` in a strand gate.
        """
        st = by_name(gp.plan(["workshop/evals/p5-longmemeval/README.md"],
                             "strand", repo=None))
        self.assertNotIn("recall-harness", st)
        cls = gp.classify(["workshop/evals/p5-longmemeval/tools/receipts.py"])
        self.assertIn("evals_memory", cls)
        self.assertNotIn("evals_recall", cls)

    def test_the_eval_engine_plans_the_local_path_cases(self):
        """The shared eval engine is recall-harness code too (GH #879).

        `workshop/evals/engine.py` sits one level above the harness, so the
        `evals_recall` glob alone does not see it; a change to it changes how
        every harness call is built. It plans `recall-harness`, whose third
        command is the offline local-path self-check, and NOT
        `scenarios:memory`.
        """
        path = "workshop/evals/engine.py"
        self.assertIn("evals_recall", gp.classify([path]))
        st = by_name(gp.plan([path], "strand", repo=None))
        self.assertIn("recall-harness", st)
        cmds = st["recall-harness"].cmds
        self.assertIn(["python3", "local_path_cases.py"], cmds)
        self.assertEqual(st["recall-harness"].cwds[cmds.index(["python3", "local_path_cases.py"])],
                         "workshop/evals/p5-longmemeval")
        self.assertNotIn("scenarios:memory", st)

    def test_the_pipeline_harness_plans_the_pipeline_cases(self):
        """The ingest pipeline of the harness has its own offline check (GH #880).

        The annotator, the runner, the self-check itself and the probe setup
        the runner instantiates each plan `recall-harness`, whose fourth
        command is `pipeline_cases.py`, run beside the scripts it imports.
        """
        for path in ("workshop/evals/p5-longmemeval/annotate.py",
                     "workshop/evals/p5-longmemeval/run_eval.py",
                     "workshop/evals/p5-longmemeval/pipeline_cases.py",
                     "workshop/fixtures/positive/memory-hive-probe/p5-setup.json"):
            st = by_name(gp.plan([path], "strand", repo=None))
            self.assertIn("recall-harness", st, path)
            cmds = st["recall-harness"].cmds
            self.assertIn(["python3", "pipeline_cases.py"], cmds, path)
            self.assertEqual(st["recall-harness"].cwds[cmds.index(["python3", "pipeline_cases.py"])],
                             "workshop/evals/p5-longmemeval", path)

    def test_integration_runs_deny_for_the_workspace_class(self):
        st = by_name(gp.plan(["Cargo.lock"], "integration", repo=None))
        self.assertIn("deny", st)

    def test_release_runs_all_three_for_any_diff(self):
        """The release keeps them unconditional -- it ships the tree."""
        st = by_name(gp.plan(["docs/x.md"], "release", repo=None))
        for name in ("scenarios:memory", "recall-harness", "deny"):
            self.assertIn(name, st)

    def test_release_runs_the_full_suite_for_any_diff(self):
        """KR-TB-1 (d), GH #1162 OR-TB.S4.39: the release runs the full suite by
        rule. It used to plan `tests` from its export diff like any pass -- full
        only because that diff happened to be wide: a one-file patch release
        planned `rdeps(<crate>)`, a docs-only one no `tests` at all, an empty one
        the t0 floor. The scenario class stays subtracted (KR-TB-1 d), and so
        does every browser unit (rule 11, GH #1161): `all()` reaches each one,
        and `browser:display` runs it, booked, in its engine part.

        Proved on the real tree, unit by unit: `tests`, `browser:display` and
        the scenario class split every test unit of `all()` -- none in no part
        (a browser unit lost from both stations is red), none in two (a browser
        unit in `tests` and the station is red). The unit tests of the crates
        (`kind(lib)`, `kind(bin)`) stay in `tests`."""
        full = "all() - (%s)" % gp.tests_minus_for(None)
        moved = release_moved()
        self.assertTrue(moved)
        for files in (["crates/meclaw-cells/src/lib.rs"], ["docs/x.md"], []):
            sel = gp.test_select(files, "release", repo=None)
            self.assertIsNotNone(sel, files)
            self.assertEqual((True, False, [], []),
                             (sel["all"], sel["floor"], sel["binaries"], sel["rdeps"]), files)
            self.assertEqual(full, sel["filter"], files)
            self.assertEqual(full, gp.test_filter(files, "release", repo=None), files)
            self.assertEqual(["mode: release -- full suite (KR-TB-1 d)"], sel["why"], files)
            st = by_name(gp.plan(files, "release", repo=None))
            self.assertEqual([["scripts/test-tier.sh", "filter", full]], st["tests"].cmds, files)
            self.assertEqual(moved, gp.browser_moved(files, "release", repo=None), files)
            self.assertEqual(gp.browser_cmds(True, moved=moved), st["browser:display"].cmds, files)
            self.assert_release_splits_all(st, files)
        # The other modes keep planning from the diff.
        for mode in ("strand", "integration"):
            sel = gp.test_select(["crates/meclaw-cells/src/lib.rs"], mode, repo=None)
            self.assertFalse(sel["all"], mode)

    def assert_release_splits_all(self, st, files):
        """Every test unit of the real tree is in exactly one of `tests`, the
        union of `browser:display` and the scenario class, read from the planned
        filters themselves. `hits` reads three term forms; a term in any other
        form is red before a unit is counted, on both sides (fail-closed)."""
        tests = st["tests"].cmds[0][2]
        self.assertTrue(tests.startswith("all() - (") and tests.endswith(")"), tests)
        cut = tests[len("all() - ("):-1].split(" + ")
        chromium, webkit = (c[2] for c in st["browser:display"].cmds)
        tail = ") - %s" % gp.WEBKIT_TESTS
        self.assertTrue(chromium.startswith("(") and chromium.endswith(tail), chromium)
        union = chromium[1:-len(tail)]
        self.assertEqual("(%s) & %s" % (union, gp.WEBKIT_TESTS), webkit)   # one union, split
        union = union.split(" + ")
        # The scenario part as a literal (tb-s10 A2, re-check tb-s4-st-pin L-2):
        # `_scenario_terms` is the production list, so a class that grows or
        # loses a term moved this expectation along with it.
        scenario = list(SCENARIO_TERMS_PINNED)
        units_pinned = SCENARIO_UNITS_PINNED
        # Presence guard, named term by named term (tb-zexp A): the public clone
        # ships without the private-only scenario binaries (make_export.py
        # BLOCKLIST), and `_scenario_terms` drops their operators there on
        # purpose (GH #934). The integration pass on c6dcafa66 measured the
        # export tree red on exactly this literal. The full tree keeps the
        # whole pin; only a file that is really absent takes its term and its
        # units out of the expectation -- never a term the resolver lost.
        for term, (rel, units) in SCENARIO_PRIVATE_ONLY.items():
            if not (REPO / rel).exists():
                scenario.remove(term)
                units_pinned -= units
        self.assertEqual(scenario, gp._scenario_terms(None))

        def hits(terms, crate, unit):
            binary = unit.split("/")[0]
            for t in terms:
                m = gp._ONE_BINARY_RE.match(t)
                if m and re.search(m.group(1), binary):
                    return True
                if t in (gp._binary_id(crate, unit), "binary_id(=%s::%s)" % (crate, binary)):
                    return True
            return False

        units = gp._test_sources(None)
        self.assertGreater(len(units), 100)
        # `hits` knows a binary regex, a whole binary and the folded-module
        # form of `_binary_id`. Any other form reads as "takes nothing out",
        # so on the `tests` side its unit would count as running there while
        # it runs nowhere (re-check pin m-1: `test(/^blob_ref_serde::/)` cut
        # on both sides stayed green). Check every term first.
        known = set()
        for crate, unit, _t in units:
            known.update((gp._binary_id(crate, unit),
                          "binary_id(=%s::%s)" % (crate, unit.split("/")[0])))
        unknown = [t for t in cut + union + scenario
                   if not gp._ONE_BINARY_RE.match(t) and t not in known]
        self.assertEqual([], unknown, ("a term form the partition cannot read", files))
        gaps, twice, browser_seen, scenario_seen = [], [], 0, 0
        for crate, unit, _t in units:
            where = [name for name, inside in (
                ("tests", not hits(cut, crate, unit)),
                ("browser:display", hits(union, crate, unit)),
                ("scenario class", hits(scenario, crate, unit))) if inside]
            if not where:
                gaps.append((crate, unit))
            elif len(where) > 1:
                twice.append((crate, unit, where))
            browser_seen += where == ["browser:display"]
            scenario_seen += where == ["scenario class"]
        self.assertEqual([], gaps, ("in no part", files))
        self.assertEqual([], twice, ("in two parts", files))
        self.assertEqual(len(gp.browser_terms()), browser_seen, files)
        self.assertEqual(units_pinned, scenario_seen, files)
        crates = {c for c, _u, _t in units}
        for crate in crates:   # the lib/bin unit tests: no term takes them out
            self.assertFalse(hits(cut, crate, crate) or hits(union, crate, crate), crate)

    def test_release_checks_every_workspace_member_alone(self):
        """Re-check M-1 (#1166): the build station merges `-p` sets, so no crate is
        built in its own feature variant any more (S6-DIAG: colony with cells
        gets tokio `process`, reqwest `rustls-tls`). `crate-isolation` checks
        each member with ITS features: one call per member, the list read
        from the workspace manifest, never written down here."""
        import tomllib
        root = gp.REPO_ROOT
        with open(os.path.join(root, "Cargo.toml"), "rb") as fh:
            members = tomllib.load(fh)["workspace"]["members"]
        names = []
        for m in members:
            with open(os.path.join(root, m, "Cargo.toml"), "rb") as fh:
                names.append(tomllib.load(fh)["package"]["name"])
        self.assertTrue(names)
        st = by_name(gp.plan(["docs/x.md"], "release", repo=None))
        self.assertIn("crate-isolation", st)
        iso = st["crate-isolation"]
        self.assertTrue(iso.cargo)
        self.assertEqual([["cargo", "check", "-p", n, "--all-targets"] for n in names],
                         iso.cmds)
        self.assertEqual(len(names), len({tuple(c) for c in iso.cmds}))

    def test_crate_isolation_is_release_only(self):
        for mode in ("strand", "integration"):
            st = by_name(gp.plan(["Cargo.lock", "crates/meclaw-colony/src/lib.rs"],
                                 mode, repo=None))
            self.assertNotIn("crate-isolation", st, mode)

    def test_crate_isolation_checks_every_member_alone_after_the_feature_unity(self):
        """GH #1168: the shared dependencies come from `[workspace.dependencies]`,
        so a merged `-p` set no longer hides a member's missing feature -- the one
        place left that does is a dependency only OTHER packages switch on. The
        release still checks each member alone, one call per member, with its
        tests (`--all-targets`), so such a member stays red there."""
        import tomllib
        root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
        with open(os.path.join(root, "Cargo.toml"), "rb") as fh:
            ws = tomllib.load(fh)["workspace"]
        names = []
        for member in ws["members"]:
            with open(os.path.join(root, member, "Cargo.toml"), "rb") as fh:
                names.append(tomllib.load(fh)["package"]["name"])
        self.assertTrue(ws.get("dependencies"), "the unification moved out of the root manifest")
        self.assertEqual([["cargo", "check", "-p", n, "--all-targets"] for n in names],
                         gp.crate_isolation_cmds(root))

    def test_an_unknown_member_list_plans_a_red_check(self):
        """fail-closed: no manifest, no members, or a member without a package
        name is ONE command that exits non-zero and names the cause."""
        cases = {"no manifest": None,
                 "empty": '[workspace]\nmembers = []\n',
                 "nameless": '[workspace]\nmembers = ["crates/a"]\n',
                 "broken": '[workspace\n'}
        for label, text in cases.items():
            with tempfile.TemporaryDirectory() as repo:
                if text is not None:
                    with open(os.path.join(repo, "Cargo.toml"), "w") as fh:
                        fh.write(text)
                os.makedirs(os.path.join(repo, "crates", "a"))
                with open(os.path.join(repo, "crates", "a", "Cargo.toml"), "w") as fh:
                    fh.write("[package]\nversion = \"0.1.0\"\n")
                cmds = gp.crate_isolation_cmds(repo)
                self.assertEqual(1, len(cmds), label)
                self.assertNotEqual("cargo", cmds[0][0], label)
                r = subprocess.run(cmds[0], capture_output=True, text=True)
                self.assertNotEqual(0, r.returncode, label)
                self.assertIn("crate-isolation", r.stderr, label)
                # re-check m-5: the cause is a `path:line` the RED line picks up
                where = "crates/a/Cargo.toml:1: " if label == "nameless" else "Cargo.toml:1: "
                self.assertIn("crate-isolation: " + where, r.stderr, label)

    # --- re-check m-4 (#1166): the member list is cargo's, or the check is refused.
    PKG = '[package]\nname = "%s"\nversion = "0.1.0"\n'

    def _ws(self, files):
        """A workspace at <tmp>/ws from {path relative to ws: text}; the path of ws."""
        tmp = tempfile.mkdtemp(prefix="ws-model-")
        self.addCleanup(__import__("shutil").rmtree, tmp, True)
        repo = os.path.join(tmp, "ws")
        for rel, text in files.items():
            path = os.path.normpath(os.path.join(repo, rel))
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w", encoding="utf-8") as fh:
                fh.write(text)
        return repo

    def test_the_member_model_follows_cargos_workspace_rules(self):
        """Every case is one rule of cargo's `Workspace::find_members` /
        `find_path_deps` / `is_excluded` (cargo/core/workspace.rs): cargo is
        never run here, so the model is pinned to the rules themselves."""
        P = self.PKG
        cases = {
            # find_members: the root manifest's own [package] is a member.
            "root package": ({
                "Cargo.toml": P % "top" + '\n[workspace]\nmembers = ["crates/a"]\n',
                "crates/a/Cargo.toml": P % "a"}, ["a", "top"]),
            # members_paths: a glob expands to its directories; exclude drops a match.
            "glob and exclude": ({
                "Cargo.toml": '[workspace]\nmembers = ["crates/*"]\nexclude = ["crates/x"]\n',
                "crates/a/Cargo.toml": P % "a", "crates/b/Cargo.toml": P % "b",
                "crates/x/Cargo.toml": P % "x", "crates/notes.md": "a file, not a member\n"},
                ["a", "b"]),
            # find_path_deps: a path dependency under the root joins, from every
            # dependency table, inherited, renamed, optional and transitive.
            "implicit path members": ({
                "Cargo.toml": '[workspace]\nmembers = ["crates/a"]\n\n'
                              '[workspace.dependencies]\nc = { path = "libs/c" }\n',
                "crates/a/Cargo.toml": P % "a"
                + '\n[dependencies]\nb = { path = "../b" }\n'
                + 'renamed = { package = "f", path = "../f", optional = true }\n'
                + '\n[dev-dependencies]\nc = { workspace = true }\n'
                + '\n[target.\'cfg(unix)\'.build-dependencies]\n'
                + 'd = { path = "../../libs/d", version = "0.1" }\n',
                "crates/b/Cargo.toml": P % "b" + '\n[dependencies]\ne = { path = "../e" }\n',
                "crates/e/Cargo.toml": P % "e", "crates/f/Cargo.toml": P % "f",
                "libs/c/Cargo.toml": P % "c", "libs/d/Cargo.toml": P % "d"},
                ["a", "b", "c", "d", "e", "f"]),
            # is_excluded: an excluded path dependency stays out ...
            "excluded path dependency": ({
                "Cargo.toml": '[workspace]\nmembers = ["crates/a"]\nexclude = ["vendor"]\n',
                "crates/a/Cargo.toml": P % "a" + '\n[dependencies]\nv = { path = "../../vendor/v" }\n',
                "vendor/v/Cargo.toml": P % "v"}, ["a"]),
            # ... and a member listed by name under an excluded path stays in.
            "explicit member beats exclude": ({
                "Cargo.toml": '[workspace]\nmembers = ["vendor/v", "crates/a"]\nexclude = ["vendor"]\n',
                "crates/a/Cargo.toml": P % "a", "vendor/v/Cargo.toml": P % "v"}, ["a", "v"]),
            # find_path_deps: a path dependency outside the root is no member.
            "path dependency outside the root": ({
                "Cargo.toml": '[workspace]\nmembers = ["crates/a"]\n',
                "crates/a/Cargo.toml": P % "a" + '\n[dependencies]\no = { path = "../../../out" }\n',
                "../out/Cargo.toml": P % "o"}, ["a"]),
            # no `members` key: the root package and its path dependencies.
            "root package only": ({
                "Cargo.toml": P % "top" + '\n[workspace]\n\n[dependencies]\nl = { path = "l" }\n',
                "l/Cargo.toml": P % "l"}, ["l", "top"]),
        }
        for label, (files, want) in cases.items():
            repo = self._ws(files)
            names, why = gp.workspace_members(repo)
            self.assertIsNone(why, label)
            self.assertEqual(want, sorted(names), label)
            scope, cmds = gp.crate_isolation(repo)
            self.assertEqual("%d members, own features" % len(want), scope, label)
            self.assertEqual(sorted(["cargo", "check", "-p", n, "--all-targets"] for n in want),
                             sorted(cmds), label)

    def test_a_workspace_the_model_cannot_resolve_is_refused(self):
        """fail-closed (m-4/m-5): what cargo would reject, or what the model does
        not cover, is ONE red command whose message starts with the manifest as
        `path:1:` -- the RED line's first finding -- and names the crate."""
        from scripts.pipeline import finding
        P, root = self.PKG, '[workspace]\nmembers = ["crates/a"]\n'
        dep = P % "a" + '\n[dependencies]\nb = { path = "../b" }\n'
        cases = {
            "glob dir without manifest": ({
                "Cargo.toml": '[workspace]\nmembers = ["crates/*"]\n',
                "crates/a/Cargo.toml": P % "a", "crates/empty/README": "x\n"},
                "crates/empty/Cargo.toml:1: "),
            "glob matches nothing": ({
                "Cargo.toml": '[workspace]\nmembers = ["crates/*"]\n', "README": "x\n"},
                "Cargo.toml:1: "),
            "member without manifest": ({"Cargo.toml": root, "crates/a/README": "x\n"},
                                        "crates/a/Cargo.toml:1: "),
            "path dependency without manifest": ({"Cargo.toml": root, "crates/a/Cargo.toml": dep},
                                                 "crates/b/Cargo.toml:1: "),
            "two packages, one name": ({
                "Cargo.toml": root, "crates/a/Cargo.toml": dep, "crates/b/Cargo.toml": P % "a"},
                "crates/b/Cargo.toml:1: "),
            "member outside the root": ({
                "Cargo.toml": '[workspace]\nmembers = ["../out"]\n', "../out/Cargo.toml": P % "o"},
                "../out/Cargo.toml:1: "),
            "nested workspace": ({
                "Cargo.toml": root, "crates/a/Cargo.toml": P % "a" + "\n[workspace]\n"},
                "crates/a/Cargo.toml:1: "),
            "member names another root": ({
                "Cargo.toml": root,
                "crates/a/Cargo.toml": '[package]\nname = "a"\nversion = "0.1.0"\nworkspace = "../x"\n'},
                "crates/a/Cargo.toml:1: "),
            "patch with a path": ({
                "Cargo.toml": root + '\n[patch.crates-io]\nz = { path = "vendor/z" }\n',
                "crates/a/Cargo.toml": P % "a", "vendor/z/Cargo.toml": P % "z"},
                "Cargo.toml:1: "),
            "outside dependency claims this root": ({
                "Cargo.toml": root,
                "crates/a/Cargo.toml": P % "a" + '\n[dependencies]\no = { path = "../../../out" }\n',
                "../out/Cargo.toml": '[package]\nname = "o"\nversion = "0.1.0"\nworkspace = "../ws"\n'},
                "../out/Cargo.toml:1: "),
            "members not a list of paths": ({"Cargo.toml": '[workspace]\nmembers = "crates/a"\n',
                                             "crates/a/Cargo.toml": P % "a"}, "Cargo.toml:1: "),
        }
        for label, (files, where) in cases.items():
            repo = self._ws(files)
            names, why = gp.workspace_members(repo)
            self.assertEqual([], names, label)
            self.assertTrue(why and why.startswith(where), "%s: %r" % (label, why))
            scope, cmds = gp.crate_isolation(repo)
            self.assertEqual("refused", scope, label)
            self.assertEqual(1, len(cmds), label)
            r = subprocess.run(cmds[0], capture_output=True, text=True)
            self.assertNotEqual(0, r.returncode, label)
            self.assertIn("crate-isolation: " + where, r.stderr, label)
            log = os.path.join(repo, "station.log")
            with open(log, "w", encoding="utf-8") as fh:
                fh.write("$ %s\n%s" % (" ".join(cmds[0][:2]), r.stderr))
            self.assertEqual(where[:-2], finding.first_finding(log), label)
        # the nameless member and the duplicate name name their crate
        repo = self._ws({"Cargo.toml": root, "crates/a/Cargo.toml": dep,
                         "crates/b/Cargo.toml": P % "a"})
        self.assertIn("`a`", gp.workspace_members(repo)[1])

    def test_a_refused_member_list_plans_a_refused_scope(self):
        """m-5: the station's scope says `refused`, not `1 members`."""
        refusal = (["python3", "-c", "import sys; sys.exit('x')"],)
        from unittest import mock
        with mock.patch.object(gp, "crate_isolation",
                                        return_value=("refused", list(refusal))):
            st = by_name(gp.plan(["docs/x.md"], "release", repo=None))
        self.assertEqual("refused", st["crate-isolation"].scope)
        self.assertEqual(list(refusal), st["crate-isolation"].cmds)

    def test_the_member_model_misses_no_package_of_this_tree(self):
        """m-4 guard on the real tree: every tracked package whose workspace root
        is this repo's root (cargo's `find_root`: the nearest manifest upwards
        with a [workspace] table, or the one its `package.workspace` names) is
        a member the model resolves -- and nothing else is."""
        import tomllib
        root = str(gp.REPO_ROOT)
        env = {k: v for k, v in os.environ.items()
               if k not in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR")}
        r = subprocess.run(["git", "-C", root, "ls-files", "--", "*Cargo.toml"],
                           capture_output=True, text=True, env=env)
        if r.returncode != 0:
            self.skipTest("no git checkout: %s" % r.stderr.strip())
        loaded = {}

        def load(d):
            if d not in loaded:
                try:
                    with open(os.path.join(d, "Cargo.toml"), "rb") as fh:
                        loaded[d] = tomllib.load(fh)
                except OSError:
                    loaded[d] = None
            return loaded[d]

        def ws_root(d):
            pkg = (load(d) or {}).get("package") or {}
            if "workspace" in pkg:
                return os.path.normpath(os.path.join(d, pkg["workspace"]))
            while True:
                if "workspace" in (load(d) or {}):
                    return d
                up = os.path.dirname(d)
                if up == d:
                    return None
                d = up
        want = []
        for rel in r.stdout.split():
            d = os.path.dirname(os.path.join(root, rel))
            name = ((load(d) or {}).get("package") or {}).get("name")
            if name and ws_root(d) == root:
                want.append(name)
        names, why = gp.workspace_members(root)
        self.assertIsNone(why)
        self.assertTrue(want)
        self.assertEqual(sorted(want), sorted(names))

    def test_release_ends_with_export_audit_and_has_advisories(self):
        st = gp.plan(["docs/x.md"], "release", repo=None)
        self.assertEqual(st[-1].name, "export-audit")
        self.assertIn("{receipt}", st[-1].cmds[0])
        self.assertNotIn("--skip-cargo", st[-1].cmds[0])
        self.assertEqual(st[-1].scope, "R1-R17")
        self.assertIn("deny-advisories", by_name(st))

    def test_release_runs_the_catalogue_tests_in_the_export_tree(self):
        """GH #1101: a count over the templates that ship is judged where they ship."""
        st = gp.plan(["docs/x.md"], "release", repo=None)
        names = [s.name for s in st]
        self.assertEqual(names[-2:], ["export-tree-tests", "export-audit"])
        ett = by_name(st)["export-tree-tests"]
        self.assertTrue(ett.cargo)
        # GH #1199 (RC-KX I1): `--worktree`, the tree the cache key describes
        self.assertEqual(ett.cmds, [["python3", "plans/export-fixtures/export_tree_tests.py",
                                     "--worktree"],
                                    # GH #1199: and the public CI's kit stations there
                                    ["python3", "plans/export-fixtures/export_tree_tests.py",
                                     "--worktree", "--kit"]])

    def test_a_template_diff_plans_export_tree_tests_in_release_only_never_ci(self):
        """Release always; S only on a kit input (`KitExportTreeSelection`),
        which a template is not; I never, C never."""
        for mode in ("strand", "integration", "ci"):
            names = [s.name for s in gp.plan(["templates/x/config.json"], mode, repo=None)]
            self.assertNotIn("export-tree-tests", names, mode)
        self.assertIn("export-tree-tests", gp.CI_EXCLUDED)

    def test_export_selftest_runs_the_lock_of_export_tree_tests(self):
        st = by_name(gp.plan(["docs/x.md"], "integration", repo=None))
        self.assertIn(["python3", "plans/export-fixtures/test_export_tree_tests.py"],
                      st["export-selftest"].cmds)

    def test_integration_ends_with_the_dry_export_audit(self):
        """The cheap export rules belong in the pass that declares a wave done.

        R2b (dead template references in tests), R5 (name/domain patterns) and
        R10 (relative links) cost seconds and need no cargo. Until the wave that
        shipped v0.30.0 they ran in `release` only, so they surfaced an hour
        after the integration pass had called the wave finished.
        """
        st = gp.plan(["docs/x.md"], "integration", repo=None)
        self.assertEqual(st[-1].name, "export-audit")
        self.assertEqual(st[-1].scope, "R1-R17 dry")
        self.assertIn("--skip-cargo", st[-1].cmds[0])
        self.assertIn("{receipt}", st[-1].cmds[0])
        self.assertEqual(st[-1].cmds[0][-4:-2], ["--rev", "HEAD"])

    def test_the_dry_audit_builds_nothing(self):
        """`--skip-cargo` skips the R8/R9/R12-class work, so cargo:0.

        A cargo:1 station takes the run's build lock and the nice/ionice/
        build-width wrapper. The dry audit compiles nothing, so claiming the
        lock would put every parallel strand in a queue for a Python run.
        """
        st = by_name(gp.plan(["docs/x.md"], "integration", repo=None))
        self.assertFalse(st["export-audit"].cargo)

    def test_the_two_audits_differ_by_two_named_flags_and_nothing_else(self):
        """One station, two shapes -- and the difference is exactly two flags.

        `--skip-cargo` (the dry run builds nothing) and `--rev HEAD` (the pass
        judges the tree its own receipt was written over; `make_export.py`
        would otherwise default to `master`, which is a different commit on
        every wave branch). Anything ELSE drifting apart -- a different script,
        a different receipt flag -- would mean the integration pass audits
        something other than what the release pass audits, which is the whole
        point of running it early.
        """
        dry = gp.plan(["docs/x.md"], "integration", repo=None)[-1].cmds[0]
        full = gp.plan(["docs/x.md"], "release", repo=None)[-1].cmds[0]
        extra = ("--skip-cargo", "--rev", "HEAD")
        self.assertEqual([a for a in dry if a not in extra], full)

    def test_the_release_audit_keeps_the_master_default(self):
        """An export is of master, whatever happens to be checked out."""
        full = gp.plan(["docs/x.md"], "release", repo=None)[-1].cmds[0]
        self.assertNotIn("--rev", full)

    def test_integration_plans_the_export_selftest(self):
        """Seconds of pure Python, and it self-tests the audit's own rules."""
        self.assertIn("export-selftest",
                      by_name(gp.plan(["docs/x.md"], "integration", repo=None)))

    def test_ci_mode_uses_check_flags_and_plans_tests_without_running(self):
        st = by_name(gp.plan(["Cargo.lock"], "ci", repo=None))
        self.assertIn("--check", st["roadmap-anchors"].cmds[0])
        self.assertIn("--check", st["adr-anchors"].cmds[0])
        self.assertIn("--check", st["claims"].cmds[0])
        self.assertTrue(st["tests"].cargo)
        self.assertFalse(st["tests"].run)
        self.assertNotIn("- (", st["tests"].scope)
        self.assertIn("corridor", st)
        self.assertIn("unwrap-budget", st)

    def test_ci_never_plans_corpus(self):
        """`workshop/` does not travel to the public tree (GH #234).

        Not even when the diff touches a corpus source: the seed builder is
        not in the tree CI runs on, so the station would be red on arrival.
        """
        for paths in ([], ["README.md"], ["templates/assistant/template.json"],
                      ["docs/config.en.md", "crates/meclaw-core/src/lib.rs"]):
            names = {s.name for s in gp.plan(paths, "ci", repo=None)}
            self.assertNotIn("corpus", names, paths)
        # ... while every other mode still plans it.
        for mode in ("strand", "integration", "release"):
            names = {s.name for s in gp.plan(["README.md"], mode, repo=None)}
            self.assertIn("corpus", names, mode)

    def test_a_bare_name_is_not_a_template_reference(self):
        """`"assistant"` as an instance name is not a reference to a template.

        `assistant`, `member`, `talky` and `display` are cell and directory
        names all over the example trees. Counting the bare string made a
        template diff pull a third of the suite.
        """
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = pathlib.Path(tmp.name)
        tests = root / "crates" / "meclaw-cells" / "tests"
        tests.mkdir(parents=True)
        (root / "crates" / "meclaw-cells" / "Cargo.toml").write_text(
            '[package]\nname = "meclaw-cells"\n', encoding="utf-8")
        # Only an instance name: a cell called `assistant` in a grown tree.
        # (`join("assistant")` is deliberately NOT used here -- the ruling
        # counts that shape as a reference, see the report.)
        (tests / "instance_only.rs").write_text(
            'fn main() {\n'
            '    let cell = root.join("main/assistant/config.json");\n'
            '    assert_eq!(name, "assistant");\n'
            '}\n', encoding="utf-8")
        # A real declaration naming the template.
        (tests / "declares_it.rs").write_text(
            'fn main() {\n'
            '    let m = r#"{"template": "assistant@1.0.0"}"#;\n'
            '}\n', encoding="utf-8")
        # And the other reference shapes, each on its own.
        (tests / "path_into_it.rs").write_text(
            'let p = root.join("templates/assistant");\n', encoding="utf-8")
        (tests / "versioned.rs").write_text(
            'let r = "assistant@2.0.0";\n', encoding="utf-8")

        sel = gp.test_filter(["templates/assistant/template.json"], "ci",
                             repo=str(root))
        self.assertEqual(sel,
                         "binary_id(=meclaw-cells::declares_it) "
                         "+ binary_id(=meclaw-cells::path_into_it) "
                         "+ binary_id(=meclaw-cells::versioned)")
        self.assertNotIn("instance_only", sel)

    def test_a_template_reached_through_a_helper_is_a_reference(self):
        """`shipped("assistant/config.json")` reads templates/assistant -- GH #713.

        A test that reads a shipped template usually spells the path:
        `root.join("templates/assistant")`. Two locks do not. They resolve the
        catalogue root ONCE -- `fn templates_root() -> PathBuf` over
        `CARGO_MANIFEST_DIR/../../templates` -- and join a runtime value onto
        it, so the name arrives as a plain argument
        (`shipped("assistant/config.json")`, `shipped("assistant", FILES)`) and
        the literal `templates/assistant` appears nowhere in the file. Neither
        rule 4 nor rule 8 saw them, and a strand that changed
        `templates/assistant/config.json` left both unrun.

        The role literal is the reason the bare name alone cannot count:
        `{"origin": "assistant"}` is in a third of the suite. The name is read
        as a template only where it is HANDED to something -- after `(`, `,`
        or `[` -- or carries a path separator.
        """
        root = pathlib.Path(reader_repo(self, {
            # The gh529 shape: a relative path into the template.
            "helper_rel": (
                'fn templates_root() -> std::path::PathBuf {\n'
                '    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))\n'
                '        .join("../../templates")\n'
                '}\n'
                'fn shipped(rel: &str) -> std::path::PathBuf {\n'
                '    templates_root().join(rel)\n'
                '}\n'
                'fn main() { let _ = shipped("assistant/config.json"); }\n'),
            # The gh561 shape: the bare name as an argument.
            "helper_name": (
                'fn templates_root() -> std::path::PathBuf {\n'
                '    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))\n'
                '        .join("../../templates")\n'
                '}\n'
                'fn shipped(name: &str, files: &[&str]) -> std::path::PathBuf {\n'
                '    templates_root().join(name)\n'
                '}\n'
                'fn main() { let _ = shipped("assistant", &["config.json"]); }\n'),
            # The same helper, but the only `assistant` is an LLM role.
            "role_only": (
                'fn templates_root() -> std::path::PathBuf {\n'
                '    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))\n'
                '        .join("../../templates")\n'
                '}\n'
                'fn shipped(rel: &str) -> std::path::PathBuf {\n'
                '    templates_root().join(rel)\n'
                '}\n'
                'fn main() {\n'
                '    let _ = shipped("talky/config.json");\n'
                '    let m = r#"{"origin": "assistant", "type": "text"}"#;\n'
                '}\n'),
            # A SYNTHETIC library under a temp dir is not the catalogue: the
            # test builds the templates it reads, so a shipped template that
            # changes says nothing about it.
            "fixture_library": (
                'fn register(root: &std::path::Path, name: &str) {\n'
                '    let dir = root.join("templates").join(name);\n'
                '    std::fs::create_dir_all(&dir).unwrap();\n'
                '}\n'
                'fn main() { register(td.path(), "assistant"); }\n'),
        }))
        sel = gp.test_filter(["templates/assistant/config.json"], "ci",
                             repo=str(root))
        self.assertEqual(sel,
                         "binary_id(=meclaw-cells::helper_name) "
                         "+ binary_id(=meclaw-cells::helper_rel)")
        # ... and the helper that reads `talky` travels for `talky`.
        talky = gp.test_filter(["templates/talky/template.json"], "ci",
                               repo=str(root))
        self.assertEqual(talky, "binary_id(=meclaw-cells::role_only)")

    def test_a_literal_path_into_a_template_needs_no_runtime_join(self):
        """`templates_root().join("memory-hive/embed/config.json")` -- GH #713.

        The same helper, the same shipped root, only the name arrives as a
        literal WITH a suffix instead of as a variable. `join("<name>")` alone
        is a reference (rule 4), `join("<name>/...")` was not, and no rule read
        the diff path either: `gh204_the_shipped_embedding_generation_agrees`
        opens `templates/memory-hive/embed/config.json` and fell out of that
        file's diff. What counts is that the file names the shipped root at
        all -- how it spells the path below it is not the resolver's business.
        """
        root = pathlib.Path(reader_repo(self, {
            "literal_suffix": (
                'fn templates_root() -> std::path::PathBuf {\n'
                '    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))\n'
                '        .join("../../templates")\n'
                '}\n'
                'fn main() {\n'
                '    let _ = std::fs::read_to_string(\n'
                '        templates_root().join("memory-hive/embed/config.json"));\n'
                '}\n'),
        }))
        sel = gp.test_filter(["templates/memory-hive/embed/config.json"], "ci",
                             repo=str(root))
        self.assertEqual(sel, "binary_id(=meclaw-cells::literal_suffix)")
        # ... and it says nothing about a template it does not name.
        self.assertIsNone(
            gp.test_filter(["templates/talky/config.json"], "ci", repo=str(root)))

    def test_a_sweep_of_the_shipped_root_is_catalogue_wide(self):
        """A walk of the catalogue counts for every template -- GH #713.

        `_is_catalogue_wide` knew one spelling of the root, `templates_root()`
        or a bare `"templates"` segment. Two real sweeps spell it neither way:
        `store_wake_failure_modes` assigns
        `PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")` and
        hands it to its own `walk()`, and `gh494_no_interior_marker_leaves_a
        _hive` calls its root helper `templates_dir()`. Both read EVERY shipped
        `config.json` and were selected for no template diff at all.
        """
        root = pathlib.Path(reader_repo(self, {
            # The root as a literal, walked by a local recursive helper.
            "sweep_literal": (
                'fn walk(dir: &std::path::Path, out: &mut Vec<String>) {\n'
                '    for e in std::fs::read_dir(dir).unwrap() {}\n'
                '}\n'
                'fn main() {\n'
                '    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))\n'
                '        .join("../../templates");\n'
                '    let mut out = Vec::new();\n'
                '    walk(&root, &mut out);\n'
                '}\n'),
            # The root behind a helper under another name.
            "sweep_helper": (
                'fn templates_dir() -> std::path::PathBuf {\n'
                '    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../templates")\n'
                '}\n'
                'fn main() {\n'
                '    let root = templates_dir();\n'
                '    let entries = std::fs::read_dir(&root).unwrap();\n'
                '}\n'),
            # A recursive helper over a directory PARAMETER, in a file that
            # builds its own tree: not the catalogue.
            "own_tree": (
                'fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {\n'
                '    for e in std::fs::read_dir(src).unwrap() {}\n'
                '}\n'
                'fn main() { copy_cells(td.path(), &root.join("templates")); }\n'),
        }))
        for text in ("sweep_literal", "sweep_helper"):
            src = {stem: t for _c, stem, t in gp._test_sources(str(root))}[text]
            self.assertTrue(gp._is_catalogue_wide(src), text)
        # Any template diff travels with both sweeps, and with nothing else.
        for name in ("talky", "memory-hive", "clock"):
            sel = gp.test_filter(["templates/%s/config.json" % name], "ci",
                                 repo=str(root))
            self.assertEqual(sel,
                             "binary_id(=meclaw-cells::sweep_helper) "
                             "+ binary_id(=meclaw-cells::sweep_literal)", name)

    def test_the_shipped_assistant_config_selects_the_locks_that_read_it(self):
        """The regression of GH #713, pinned against the real tree.

        `gh529_the_menu_merges_every_answerers_declarations` and
        `gh561_the_pack_rides_a_v_lane` both open
        `templates/assistant/config.json` through a helper. The wave-H strand
        gate that changed that file planned 246 binaries and neither of them;
        both went red on the integration branch afterwards.
        """
        # template -> the locks its config.json must run.
        wanted = {
            "assistant": ("gh529_the_menu_merges_every_answerers_declarations",
                          "gh561_the_pack_rides_a_v_lane"),
            # A literal path below the shipped root, and the two catalogue
            # sweeps -- every template's config.json is one of their inputs.
            "memory-hive": ("gh204_the_shipped_embedding_generation_agrees",
                            "store_wake_failure_modes",
                            "gh494_no_interior_marker_leaves_a_hive"),
            "clock": ("store_wake_failure_modes",
                      "gh494_no_interior_marker_leaves_a_hive"),
        }
        present = {stem for _c, stem, _t in gp._test_sources(None)}
        for template, locks in wanted.items():
            sel = gp.test_filter(["templates/%s/config.json" % template], "strand")
            for lock in locks:
                if lock not in present:
                    continue          # renamed or gone: nothing left to pin
                self.assertIn("binary_id(=meclaw-cells::%s)" % lock, sel,
                              "%s: %s" % (template, lock))

    def test_a_format_string_template_path_counts_for_every_template(self):
        """`format!("templates/{dir}")` names a template only at runtime.

        Two real files build their paths that way -- gh455_the_two_templates_ship
        and gh482_the_catalogue_says_no_by_name. Reading them as "no template
        reference" dropped them from every template diff: under-selection.
        """
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = pathlib.Path(tmp.name)
        tests = root / "crates" / "meclaw-cells" / "tests"
        tests.mkdir(parents=True)
        (root / "crates" / "meclaw-cells" / "Cargo.toml").write_text(
            '[package]\nname = "meclaw-cells"\n', encoding="utf-8")
        (tests / "fmt_path.rs").write_text(
            'fn main() {\n'
            '    for dir in ["a", "b"] {\n'
            '        let p = repo(&format!("templates/{dir}/template.json"));\n'
            '    }\n'
            '}\n', encoding="utf-8")
        (tests / "fmt_panic.rs").write_text(
            'let t = reg.get(name)\n'
            '    .unwrap_or_else(|| panic!("templates/{name} declares no contract"));\n',
            encoding="utf-8")
        (tests / "unrelated.rs").write_text(
            'fn main() { let _ = format!("cells/{dir}/cell.db"); }\n', encoding="utf-8")

        srcs = {stem: text for _c, stem, text in gp._test_sources(str(root))}
        self.assertTrue(gp._is_catalogue_wide(srcs["fmt_path"]))
        self.assertTrue(gp._is_catalogue_wide(srcs["fmt_panic"]))
        self.assertFalse(gp._is_catalogue_wide(srcs["unrelated"]))

        # Both travel for a template neither of them names.
        sel = gp.test_filter(["templates/clock/template.json"], "ci", repo=str(root))
        self.assertEqual(sel,
                         "binary_id(=meclaw-cells::fmt_panic) "
                         "+ binary_id(=meclaw-cells::fmt_path)")

    def test_ci_plans_only_stations_that_exist_in_the_published_tree(self):
        """ci runs on the export mirror: no workshop/, no plans/, deny is its own job."""
        paths = ["templates/memory-hive/store/config.json", "Cargo.lock",
                 "crates/meclaw-colony/src/colony.rs", "plans/export-fixtures/x.py",
                 "README.md"]
        names = {s.name for s in gp.plan(paths, "ci", repo=None)}
        for absent in gp.CI_EXCLUDED:
            self.assertNotIn(absent, names, absent)
        # ... while release, which plans every one of them, proves the
        # exclusion is the mode and not a gap in the diff.
        released = {s.name for s in gp.plan(paths, "release", repo=None)}
        self.assertEqual(gp.CI_EXCLUDED - released, set())
        # What ci DOES plan is still the cheap half plus the cargo work.
        for present in ("roadmap-anchors", "tree-rules", "fmt", "clippy",
                        "unwrap-budget", "corridor", "tests", "shellcheck"):
            self.assertIn(present, names, present)

    def test_export_audit_is_cargo_work(self):
        """make_export.py runs `cargo check --workspace --all-targets` inside.

        A cold build in a fresh target directory, so it needs the runner's
        nice/ionice/flock and build-width hygiene like any other cargo station.
        """
        st = by_name(gp.plan(["docs/x.md"], "release", repo=None))
        self.assertTrue(st["export-audit"].cargo)

    def test_a_station_with_a_cwd_carries_a_cwd_relative_argv(self):
        """The runner `cd`s into `cwd`; a repo-relative path misses from there.

        Measured 2026-09-04: `python3 workshop/evals/builder-scenarios/
        run_builder_scenarios.py` with that same directory as cwd resolved to
        `.../builder-scenarios/workshop/evals/builder-scenarios/...` and the
        station died on "No such file".
        """
        st = by_name(gp.plan(["docs/x.md"], "release", repo=None))
        for name in ("scenarios:builder", "recall-harness"):
            for cmd, cwd in zip(st[name].cmds, st[name].cwds):
                self.assertTrue(cwd, name)
                for arg in cmd[1:]:
                    self.assertFalse(
                        arg.startswith(cwd + "/"),
                        "%s: %r is relative to the repo root, not to %r"
                        % (name, arg, cwd))
        self.assertEqual(st["scenarios:builder"].cmds,
                         [["python3", "run_builder_scenarios.py"]])
        self.assertEqual(st["recall-harness"].cmds,
                         [["python3", "recall_cases.py"],
                          ["python3", "answer_cases.py"],
                          ["python3", "local_path_cases.py"],
                          ["python3", "pipeline_cases.py"]])
        self.assertEqual(st["recall-harness"].cwds,
                         ["workshop/evals/p5-longmemeval/tools",
                          "workshop/evals/p5-longmemeval",
                          "workshop/evals/p5-longmemeval",
                          "workshop/evals/p5-longmemeval"])

    def test_every_station_argv_resolves_from_its_working_directory(self):
        """Whatever the cwd, the script the station names has to be there.

        `workshop/` and `plans/` do not travel with an export, so in the
        published tree those stations' scripts are legitimately absent and are
        passed over. In the private tree nothing may be passed over -- that is
        the second assertion, and it is what keeps the skip from hiding a
        genuinely broken path.
        """
        import os
        absent_trees = [d for d in ("workshop", "plans")
                        if not os.path.isdir(os.path.join(str(REPO), d))]
        passed_over = []
        for mode in ("strand", "integration", "release"):
            for st in gp.plan(["docs/x.md", "Cargo.lock"], mode, repo=None):
                for cmd, cwd in zip(st.cmds, st.cwds):
                    for arg in cmd[1:]:
                        if not arg.endswith((".py", ".sh")):
                            continue
                        rel = os.path.join(cwd or "", arg)
                        if rel.split("/")[0] in absent_trees:
                            passed_over.append(rel)
                            continue
                        path = os.path.join(str(REPO), rel)
                        self.assertTrue(os.path.isfile(path),
                                        "%s [%s]: no such file: %s"
                                        % (st.name, mode, path))
        if not absent_trees:
            self.assertEqual([], passed_over,
                             "the private tree has workshop/ and plans/; "
                             "nothing may be passed over here")

    def test_unwrap_budget_is_cargo_work(self):
        """It is a Python script that shells out to `cargo clippy --workspace`."""
        st = by_name(gp.plan(["crates/meclaw-core/src/lib.rs"], "strand", repo=None))
        self.assertTrue(st["unwrap-budget"].cargo)

    def test_corpus_checks_both_librarian_products(self):
        """The old R11 stage ran two checks; one of them is not the gate."""
        seed = "workshop/tools/build_librarian_seed.py"
        lib = "workshop/tools/build_librarian.py"

        touched = by_name(gp.plan(["README.md"], "strand", repo=None))["corpus"]
        self.assertEqual(touched.scope, "regenerate+check+librarian")
        self.assertEqual(touched.cmds, [
            ["python3", seed],
            ["python3", seed, "--check"],
            ["python3", lib, "--check"]])

        untouched = by_name(gp.plan(["docs/nothing.en.md"], "strand", repo=None))["corpus"]
        self.assertEqual(untouched.scope, "check+librarian")
        self.assertEqual(untouched.cmds, [
            ["python3", seed, "--check"],
            ["python3", lib, "--check"]])

        # Never regenerated, never checked in ci -- `workshop/` does not travel.
        self.assertNotIn("corpus",
                         {s.name for s in gp.plan(["README.md"], "ci", repo=None)})

    def test_the_committed_corpus_is_checked_before_anything_regenerates_it(self):
        """GH #596: `corpus` grades the file it has just written.

        So the committed corpus -- the one that actually travels -- needs a
        station of its own, and it has to be ORDERED before the regenerate.
        """
        seed = "workshop/tools/build_librarian_seed.py"
        for mode in ("strand", "integration", "release"):
            st = by_name(gp.plan(["crates/meclaw-core/src/lib.rs"], mode, repo=None))
            self.assertIn("corpus-committed", st, mode)
            self.assertEqual([["python3", seed, "--check"]],
                             st["corpus-committed"].cmds, mode)
            self.assertFalse(st["corpus-committed"].cargo, mode)
        order = gp.STATION_ORDER
        self.assertLess(order.index("corpus-committed"), order.index("corpus"))
        # ci has no `workshop/` at all -- neither half may be planned there.
        self.assertNotIn("corpus-committed",
                         {s.name for s in gp.plan(["README.md"], "ci", repo=None)})

    def test_gate_selftest_runs_every_script_test(self):
        """One command, every module -- every script under `scripts/` that
        carries its own tests is run here, or its tests are a habit."""
        st = by_name(gp.plan(["scripts/gate_plan.py"], "strand", repo=None))
        self.assertEqual(st["gate-selftest"].cmds, [[
            "/usr/bin/python3", "-I", "scripts/pipeline/selftest_shard.py", "unittest",
            "scripts.tests.test_gate_plan", "scripts.tests.test_gate_sh",
            "scripts.tests.test_strand_sh",
            "scripts.tests.test_pipeline_hygiene",
            "scripts.tests.test_wave_receipt",
            "scripts.tests.test_lane_fingerprint",
            "scripts.tests.test_lane_browser_deps",
            "scripts.tests.test_wave_retro",
            "scripts.tests.test_git_merge_display_sync",
            "scripts.tests.test_precheck",
            "scripts.tests.test_precheck_testing_bans",
            "scripts.tests.test_pipeline_locks",
            "scripts.tests.test_harness_fold",
            "scripts.tests.test_pipeline_pre",
            "scripts.tests.test_pipeline_cascade_dry",
            "scripts.tests.test_display_sync",
            "scripts.tests.test_nextest_quarantine",
            "scripts.tests.test_slow_tests",
            "scripts.tests.test_flake_triage",
            "scripts.tests.test_clock_bans",
            "scripts.tests.test_roadmap_anchors",
            "scripts.tests.test_required_pins",
            "scripts.tests.test_pipeline_store",
            "scripts.tests.test_pipeline_events",
            "scripts.tests.test_pipeline_receipt",
            "scripts.tests.test_pipeline_executor",
            "scripts.tests.test_pipeline_schedule",
            "scripts.tests.test_pipeline_lanes_cli",
            "scripts.tests.test_pipeline_plan",
            "scripts.tests.test_pipeline_cache",
            "scripts.tests.test_pipeline_cache_reads",
            "scripts.tests.test_pipeline_lane",
            "scripts.tests.test_fake_ssh",
            "scripts.tests.test_lane_engstelle",
            "scripts.tests.test_pipeline_gate_cli",
            "scripts.tests.test_pipeline_metrics",
            "scripts.tests.test_pipeline_backfill",
            "scripts.tests.test_pipeline_grafana",
            "scripts.tests.test_pipeline_retro",
            "scripts.tests.test_pipeline_night",
            "scripts.tests.test_pipeline_orga_export",
            "scripts.tests.test_wave_close",
            "scripts.tests.test_wave_live",
            "scripts.tests.test_pipeline_register",
            "scripts.tests.test_pipeline_unit_runner",
            "scripts.tests.test_store_isolation",
            "scripts.tests.test_kit_time",
            "scripts.tests.test_pipeline_measure",
            "scripts.tests.test_pipeline_ports",
            "scripts.tests.test_pipeline_lab_width",
            "scripts.tests.test_browser_required",
            "scripts.tests.test_gate_plan_browser_units",
            "scripts.tests.test_pipeline_pool",
            "scripts.tests.test_nextest_groups",
            "scripts.tests.test_display_scenarios_parallel",
            "scripts.tests.test_side_effect_guard",
            "scripts.tests.test_pipeline_merge",
            "scripts.tests.test_pipeline_versions",
            "scripts.tests.test_pipeline_revert",
            "scripts.tests.test_pipeline_flakes",
            "scripts.tests.test_pipeline_graph",
            "scripts.tests.test_pipeline_impact",
            "scripts.tests.test_pipeline_coverage",
            "scripts.tests.test_pipeline_questions",
            "scripts.tests.test_pipeline_finding",
            "scripts.tests.test_unittest_shape",
            "scripts.tests.test_pipeline_receipt_bytes",
            "scripts.tests.test_pipeline_narrow",
            "scripts.tests.test_pipeline_spawn_closure",
            "scripts.tests.test_pipeline_browser_cover",
            "scripts.tests.test_pipeline_browser_plan",
            "scripts.tests.test_pipeline_full_pass",
            "scripts.tests.test_pipeline_lanes_release",
            "scripts.tests.test_pipeline_selftest_shard",
            "scripts.tests.test_meclaw_bin_owner",
            "scripts.tests.test_private_tree",
            "scripts.tests.test_pipeline_ir_path",
            "scripts.tests.test_pipeline_two_phase_build",
            "scripts.tests.test_pipeline_turnaround",
            "scripts.tests.test_stop_watch",
            "scripts.tests.test_pipeline_progress",
            "scripts.tests.test_pipeline_merge_gate_parity",
            "scripts.tests.test_export_audit_scope"]])

    def test_every_runner_test_module_is_in_gate_selftest(self):
        """GH #1162: the runner's own tests (`scripts/tests/test_pipeline_*.py`)
        run in `gate-selftest` or nowhere -- no other station plans them. Two
        modules of the I/R path (`_ir_path`, `_two_phase_build`) stood in no
        station: the kit ran them, no gate did, and the export scan (seeded by
        this very list) never saw them either."""
        st = by_name(gp.plan(["scripts/gate_plan.py"], "strand", repo=None))
        listed = set(st["gate-selftest"].cmds[0])
        mods = sorted("scripts.tests." + p.stem
                      for p in (REPO / "scripts" / "tests").glob("test_pipeline_*.py"))
        self.assertTrue(mods)
        self.assertEqual([], [m for m in mods if m not in listed])

    def test_every_script_test_module_is_run_by_a_station_of_its_own_diff(self):
        """L-7 (tb-s10): a module under `scripts/tests/` that no station of
        its own diff runs is a habit, not a lock. One module stood in no
        station in any mode -- the kit ran it, no gate did. The check reads
        the tree, so a new module is covered the day it lands. A path as an
        argument (`precheck --files`) is not a run; the module name is."""
        missing = []
        for path in sorted((REPO / "scripts" / "tests").glob("test_*.py")):
            rel = path.relative_to(REPO).as_posix()
            mod = "scripts.tests." + path.stem
            argv = {tok for st in gp.plan([rel], "strand", repo=None)
                    for cmd in st.cmds for tok in cmd}
            if not any(t == mod or t.startswith(mod + ".") for t in argv):
                missing.append(rel)
        self.assertEqual([], missing)

    def test_a_binary_owner_diff_plans_the_lock_in_precheck_not_gate_selftest(self):
        """GH #1168 (tb-s6b, tempo correction): the manifests, the binary
        sources and the two recipes decide which crate owns `meclaw`. Their
        lock is `bin-owner-lock` in `scripts/pipeline/locks.py` and runs in
        `precheck` (milliseconds); planning `gate-selftest` (~326 s) for it
        made every manifest diff, the merge's version bump included, pay the
        whole selftest. The test module stays in the selftest list, so a kit
        diff still runs it like every kit test."""
        for path in ("Cargo.toml", "crates/meclaw-cli/Cargo.toml",
                     ".github/workflows/release.yml", "scripts/install.sh",
                     "crates/meclaw-cli/src/main.rs",
                     "crates/meclaw-cells/src/bin/meclaw.rs",
                     "crates/meclaw-cells/src/bin/meclaw/main.rs"):
            for mode in ("strand", "integration"):
                with self.subTest(path=path, mode=mode):
                    self.assertNotIn("bin_owner", gp.classify([path]))
                    st = by_name(gp.plan([path], mode, repo=None))
                    self.assertNotIn("gate-selftest", st)
                    self.assertIn("precheck", st)
                    self.assertIn(path, st["precheck"].cmds[0])

    def test_the_pre_run_and_its_locks_are_gate_infrastructure(self):
        """`pre.py` and `locks.py` are what `precheck` and `strand.sh pre`
        grade with: a diff that touches only them plans their tests."""
        for path in ("scripts/pipeline/pre.py", "scripts/pipeline/locks.py"):
            self.assertIn("gate_infra", gp.classify([path]))

    def test_the_flake_triage_is_gate_infrastructure(self):
        """Re-check of GH #1180: a diff only at `flake_triage.py` plans its
        self-test (`classify` read `set()` before)."""
        self.assertIn("gate_infra", gp.classify(["scripts/flake_triage.py"]))
        st = by_name(gp.plan(["scripts/flake_triage.py"], "strand", repo=None))
        self.assertIn("scripts.tests.test_flake_triage", st["gate-selftest"].cmds[0])

    def test_the_quarantine_config_is_gate_infrastructure(self):
        """`.config/nextest.toml` decides which test may be retried, and
        `test_nextest_quarantine.py` is its drift lock: a filterset that
        reaches no test parses cleanly and leaves the station green over a
        debt nobody pays (wave P review C1, GH #763)."""
        path = ".config/nextest.toml"
        self.assertIn("gate_infra", gp.classify([path]))
        self.assertIn("gate-selftest",
                      {s.name for s in gp.plan([path], "strand", repo=None)})

    def test_the_source_mark_travels_with_the_display_template(self):
        """The mark `SOURCE` is the strand half of the drift lock, so a diff
        that touches it must plan the lock that reads it -- and so must any
        other diff in the display template. Without the `template` class the
        mark would be a file nothing runs (wave P review M4)."""
        mark = "templates/display/compose/scenarios/SOURCE"
        self.assertIn("template", gp.classify([mark]))
        for path in (mark, "templates/display/compose/config.json"):
            expr = gp.test_filter([path], "integration", repo=None) or ""
            self.assertIn(
                "binary_id(=meclaw-cells::710_the_scenarios_run_against_the_curator)",
                expr, path)

    def test_scenarios_display_also_checks_the_source_mark(self):
        """The cheap half of the drift question, in the station that runs in
        I, R and C always: does the mark still describe the living tree? It is
        a NOTE, never a verdict -- the Rust lock is what goes red."""
        st = by_name(gp.plan(["README.md"], "integration", repo=None))
        self.assertIn(["python3", "scripts/display_sync.py", "--check-source"],
                      st["scenarios:display"].cmds)

    def test_the_retro_library_is_gate_infrastructure(self):
        """`scripts/retro/**` and `scripts/wave_retro.py` carry the numbers a
        wave is judged by, and `gate-selftest` runs their test. Without a
        class the station is planned only for whoever edits the test file
        itself -- the habit OR-P.retro.3 was written against."""
        for path in ("scripts/wave_retro.py", "scripts/retro/metrics.py",
                     "scripts/retro/thresholds.json"):
            self.assertIn("gate_infra", gp.classify([path]), path)
            names = {s.name for s in gp.plan([path], "strand", repo=None)}
            self.assertIn("gate-selftest", names, path)

    def test_the_strand_kit_plans_the_gate_selftest(self):
        """`scripts/strand.sh` and `scripts/wave_receipt.py` carry their own
        tests in `gate-selftest`, and a diff that touches only them planned
        `shellcheck` and nothing else -- an under-selection, which
        development-rules section 7 does not allow (V-kit section 1.6)."""
        for path in ("scripts/strand.sh", "scripts/wave_receipt.py",
                     "scripts/pipeline/hygiene.py"):
            self.assertIn("gate_infra", gp.classify([path]), path)
            names = {s.name for s in gp.plan([path], "strand", repo=None)}
            self.assertIn("gate-selftest", names, path)

    def test_the_pipeline_plans_the_gate_selftest(self):
        """Since W3 (#1117) `gate.sh` is a shell around `gate_legacy.sh`
        and `pipeline.py` + `scripts/pipeline/`; a diff that touches only one
        of them has to plan the station their tests run in, as `gate.sh`
        and `strand.sh` do."""
        for path in ("scripts/gate_legacy.sh", "scripts/pipeline.py",
                     "scripts/pipeline/cli.py", "scripts/pipeline/gate.py"):
            self.assertIn("gate_infra", gp.classify([path]), path)
            names = {s.name for s in gp.plan([path], "strand", repo=None)}
            self.assertIn("gate-selftest", names, path)

    def test_gate_infrastructure_under_github_gates_has_a_class(self):
        """The unwrap ratchet and the byte gates are gates, not stray files."""
        for path in (".github/gates/unwrap_budget.py",
                     ".github/gates/unwrap_budget.txt"):
            self.assertIn("unwrap_infra", gp.classify([path]), path)
            names = {s.name for s in gp.plan([path], "strand", repo=None)}
            self.assertIn("unwrap-budget", names, path)
        gates = ".github/gates/corridor_byte_gates.sh"
        self.assertIn("corridor", gp.classify([gates]))
        self.assertIn("shell", gp.classify([gates]))
        names = {s.name for s in gp.plan([gates], "strand", repo=None)}
        self.assertIn("corridor", names)
        self.assertIn("shellcheck", names)

    def test_catalogue_enumeration_needs_real_template_iteration(self):
        """`read_dir` on a directory PARAMETER is not a catalogue enumeration.

        The old rule was "`read_dir(` anywhere AND `templates` anywhere in the
        file", which called 140 of 763 integration tests catalogue-wide --
        almost all of them only because they carry a private
        `fn copy_cells(src, dst)` helper.
        """
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = pathlib.Path(tmp.name)
        tests = root / "crates" / "meclaw-cells" / "tests"
        tests.mkdir(parents=True)
        (root / "crates" / "meclaw-cells" / "Cargo.toml").write_text(
            '[package]\nname = "meclaw-cells"\n', encoding="utf-8")

        # (b) one line carries read_dir together with a catalogue token.
        (tests / "cat_line.rs").write_text(
            'fn main() {\n'
            '    for e in std::fs::read_dir(root.join("templates")).unwrap() {}\n'
            '}\n', encoding="utf-8")
        # (c) the two-statement form, inside the window.
        (tests / "cat_window.rs").write_text(
            'fn main() {\n'
            '    let dir = templates_root();\n'
            '    let mut names = Vec::new();\n'
            '    for e in std::fs::read_dir(dir).unwrap() { names.push(e); }\n'
            '}\n', encoding="utf-8")
        # (e) the walker is its own function, called with the catalogue root.
        (tests / "cat_indirect.rs").write_text(
            'fn copy_tree(src: &Path, dst: &Path) {\n'
            '    for e in std::fs::read_dir(src).unwrap() { let _ = e; }\n'
            '}\n'
            'fn setup(root: &Path) {\n'
            '    copy_tree(&repo("templates"), &root.join("templates"));\n'
            '}\n', encoding="utf-8")
        # ... but not when that root is narrowed to a single template.
        (tests / "one_template.rs").write_text(
            'fn copy_tree(src: &Path, dst: &Path) {\n'
            '    for e in std::fs::read_dir(src).unwrap() { let _ = e; }\n'
            '}\n'
            'fn setup(root: &Path) {\n'
            '    copy_tree(&templates_root().join("talky"), &root.join("main"));\n'
            '}\n', encoding="utf-8")
        # Neither: a recursive helper over a directory PARAMETER, pointed at
        # a cell.db tree, in a file that names one template in passing.
        (tests / "not_cat.rs").write_text(
            'fn walk(dir: &Path) {\n'
            '    for e in std::fs::read_dir(dir).unwrap() { walk(&e.path()); }\n'
            '}\n'
            'fn check(root: &Path) {\n'
            '    walk(&root.join("cells").join("cell.db"));\n'
            '    let _ = r#"{"template": "assistant@1"}"#;\n'
            '}\n', encoding="utf-8")
        # A walk plus a DISTANT `templates_root()` IS catalogue-wide, and that
        # is deliberate: it is the shape of gh196_shipped_hive_ports,
        # gh202_shipped_drain_requirements and gh204_declared_defaults_match_
        # the_inline, which bind the root to a variable and hand it to a helper
        # 25 lines further down. Reading it as a non-match would under-select.
        (tests / "far_root.rs").write_text(
            'fn shipped() -> Vec<String> {\n'
            '    let root = templates_root();\n'
            '    let mut out = Vec::new();\n'
            '    for _pad in 0..4 { out.push(String::new()); }\n'
            '    collect(&root, &mut out);\n'
            '    out\n'
            '}\n'
            'fn collect(dir: &Path, out: &mut Vec<String>) {\n'
            '    for e in std::fs::read_dir(dir).unwrap() { let _ = e; }\n'
            '}\n', encoding="utf-8")

        srcs = {stem: text for _c, stem, text in gp._test_sources(str(root))}
        for stem in ("cat_line", "cat_window", "cat_indirect", "far_root"):
            self.assertTrue(gp._is_catalogue_wide(srcs[stem]), stem)
        for stem in ("one_template", "not_cat"):
            self.assertFalse(gp._is_catalogue_wide(srcs[stem]), stem)

        # A template nobody names selects the enumerators and nothing else.
        self.assertEqual(
            gp.test_filter(["templates/memory-hive/config.json"], "ci", repo=str(root)),
            "binary_id(=meclaw-cells::cat_indirect) "
            "+ binary_id(=meclaw-cells::cat_line) "
            "+ binary_id(=meclaw-cells::cat_window) "
            "+ binary_id(=meclaw-cells::far_root)")
        # `not_cat.rs` still travels for the template it names itself, and
        # `one_template.rs` for the one it copies -- no under-selection.
        assistant = gp.test_filter(["templates/assistant/config.json"], "ci",
                                   repo=str(root))
        self.assertIn("binary_id(=meclaw-cells::not_cat)", assistant)
        talky = gp.test_filter(["templates/talky/config.json"], "ci", repo=str(root))
        self.assertIn("binary_id(=meclaw-cells::one_template)", talky)

    def test_rule_7_integration_selects_exactly_like_strand(self):
        """Rule 7: integration reuses rules 1-5 unchanged -- only the always-stations
        differ. The release does not: it runs the full suite by rule
        (`test_select`, KR-TB-1 d, GH #1162 OR-TB.S4.39)."""
        repo = mini_repo(self)
        for paths in (["crates/meclaw-colony/src/route.rs"],
                      ["templates/memory-hive/store/config.json"],
                      ["crates/meclaw-cells/tests/mock_openai.rs"],
                      ["Cargo.lock"]):
            strand = gp.test_filter(paths, "strand", repo=repo)
            self.assertEqual(gp.test_filter(paths, "integration", repo=repo), strand, paths)
            self.assertTrue(gp.test_filter(paths, "release", repo=repo).startswith("all()"),
                            paths)

    def test_crate_name_mismatch_is_an_error(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = pathlib.Path(tmp.name)
        (root / "crates" / "meclaw-cells" / "src").mkdir(parents=True)
        (root / "crates" / "meclaw-cells" / "Cargo.toml").write_text(
            '[package]\nname = "not-meclaw-cells"\n', encoding="utf-8")
        with self.assertRaises(gp.GatePlanError):
            gp.crates_of(["crates/meclaw-cells/src/lib.rs"], repo=str(root))

    def test_corpus_sources_mirror_the_generator(self):
        """Every source the seed generator reads must trigger the corpus station.

        `CORPUS_SOURCES` is a second copy of a list that lives, canonically, in
        `workshop/tools/build_librarian_seed.py`: the globs that generator
        walks ARE the sources of the committed corpus. A copy ages silently --
        a new section in the generator, and the resolver stops selecting the
        station for the very diff that changes the corpus. So instead of three
        spot checks, this reads the generator as text and demands coverage for
        every path literal it globs.

        Text, not import: the generator lives under `workshop/`, which never
        travels, and importing it would need its whole dependency surface. Two
        shapes carry every source it has -- `glob.glob(os.path.join(CORE, X))`
        and the `add_*(rows, os.path.join(CORE, X), ...)` calls -- plus the
        `SPEC_DOCS` pairs, whose FIRST element is the file that is read (the
        second is the public name the chunk cites, which nothing reads).

        The one shape that carries no `CORE` literal is the recursive
        `config.json` sweep inside a template directory; it is asserted
        separately, because dropping it would silently un-cover every
        `templates/**/config.json`.

        In the published tree there is no `workshop/`, so this skips -- which
        is what R15 of the export observes before the CI does.
        """
        gen = REPO / "workshop" / "tools" / "build_librarian_seed.py"
        private_tree.require("workshop/tools/build_librarian_seed.py")
        text = gen.read_text(encoding="utf-8")

        wanted = re.compile(r'^(?:docs/|templates/|examples/|workshop/|README\.md$)')
        globbed = set()

        # 1. glob.glob(os.path.join(CORE, "<literal>"[, recursive=True]))
        for lit, rest in re.findall(
                r'glob\.glob\(\s*os\.path\.join\(\s*CORE\s*,\s*"([^"]+)"\s*\)([^)]*)',
                text):
            if wanted.match(lit):
                globbed.add((lit, "recursive=True" in rest))

        # 2. add_markdown / add_json_blob straight onto a CORE path.
        for lit in re.findall(
                r'add_\w+\(\s*rows\s*,\s*os\.path\.join\(\s*CORE\s*,\s*"([^"]+)"',
                text):
            if wanted.match(lit):
                globbed.add((lit, False))

        # 3. The docs tuple: SPEC_DOCS = [("docs/x.en.md", "docs/x.md"), ...].
        block = re.search(r'SPEC_DOCS\s*=\s*\[(.*?)\n\]', text, re.S)
        self.assertIsNotNone(block, "SPEC_DOCS is no longer a bracketed list")
        for lit in re.findall(r'\(\s*"([^"]+)"\s*,', block.group(1)):
            if wanted.match(lit):
                globbed.add((lit, False))

        self.assertTrue(globbed, "read no source literal out of the generator")

        def covered(glob_str, recursive):
            for src in gp.CORPUS_SOURCES:
                if src == glob_str:
                    return True
                # `**` and `*` are the same reach only where the generator
                # actually passed recursive=True.
                if recursive and src.replace("**", "*") == glob_str.replace("**", "*"):
                    return True
            return False

        missing = sorted(g for g, rec in globbed if not covered(g, rec))
        self.assertEqual(
            missing, [],
            "build_librarian_seed.py reads sources CORPUS_SOURCES does not "
            "cover: %s" % missing)

        # The template-local recursive sweep (`os.path.join(dirpath, "**",
        # "config.json")`), which carries no CORE literal to extract.
        if re.search(r'os\.path\.join\(\s*dirpath\s*,\s*"\*\*"\s*,\s*"config\.json"',
                     text):
            self.assertTrue(
                covered("templates/**/config.json", True),
                "the generator sweeps config.json under a template, and "
                "CORPUS_SOURCES does not cover templates/**/config.json")


class PersonaStations(unittest.TestCase):
    """The persona gate (ADR-0029, GH #882): two FREE stations over a committed
    measurement. The paid or long run is an artefact, never a station."""

    PERSONA = "workshop/evals/persona"

    def test_a_prompt_literal_plans_the_receipt_station(self):
        """A prompt edit re-plans the receipt, not the offline cases."""
        for p in ("templates/memory-hive/close-glue/config.json",   # INSTRUCTIONS
                  "templates/memory-hive/schemas/config.json",      # MEMORY_RULES
                  "templates/collector/assemble/config.json"):      # SIDECAR_PREAMBLE
            with self.subTest(path=p):
                self.assertIn("persona_source", gp.classify([p]))
                st = by_name(gp.plan([p], "strand", repo=None))
                self.assertIn("persona-receipt", st)
                self.assertNotIn("persona-cases", st)

    def test_an_eval_file_plans_both_stations(self):
        p = self.PERSONA + "/scenarios/memory/B6.json"
        self.assertIn("evals_persona", gp.classify([p]))
        st = by_name(gp.plan([p], "strand", repo=None))
        self.assertIn("persona-cases", st)
        self.assertIn("persona-receipt", st)

    def test_the_passes_plan_both_for_any_diff(self):
        for mode in ("integration", "release"):
            with self.subTest(mode=mode):
                st = by_name(gp.plan(["docs/x.md"], mode, repo=None))
                for name in ("persona-cases", "persona-receipt"):
                    self.assertIn(name, st)
                    self.assertFalse(st[name].cargo)

    def test_ci_plans_neither(self):
        """workshop/ does not travel; ci runs on the published tree."""
        paths = [self.PERSONA + "/scenarios/memory/B6.json",
                 self.PERSONA + "/last_run.json",
                 "templates/memory-hive/close-glue/config.json"]
        names = {s.name for s in gp.plan(paths, "ci", repo=None)}
        for name in ("persona-cases", "persona-receipt"):
            self.assertNotIn(name, names)
            self.assertIn(name, gp.CI_EXCLUDED)

    def test_neither_persona_station_builds(self):
        paths = [self.PERSONA + "/fingerprint.py", "templates/talky/config.json"]
        for mode in ("strand", "integration", "release"):
            for s in gp.plan(paths, mode, repo=None):
                if s.name.startswith("persona-"):
                    self.assertIs(s.cargo, False, (mode, s.name))

    def test_a_committed_report_still_classifies(self):
        """L-12: the committed report is the measurement, not a run artefact."""
        p = self.PERSONA + "/last_run.json"
        self.assertNotIn(p, gp.IGNORED)
        self.assertIn("evals_persona", gp.classify([p]))
        self.assertIn("persona-receipt", by_name(gp.plan([p], "strand", repo=None)))

    def test_the_hive_is_matched_whole(self):
        for cell in ("extract-glue", "embed", "writer", "tool", "recall", "dream-glue"):
            p = "templates/memory-hive/%s/config.json" % cell
            with self.subTest(path=p):
                self.assertIn("persona_source", gp.classify([p]))
        self.assertIn("persona_source", gp.classify(["templates/memory-hive/config.json"]))

    def test_the_curator_is_matched_whole(self):
        """GH #888/#889: the curator shapes the window and carries the summarizer's prompt."""
        for p in ("templates/curator/config.json",
                  "templates/curator/intake/config.json",
                  "templates/curator/summarizer/config.json",
                  "templates/curator/policy/config.json",
                  "templates/curator/ledger/seed/state.jsonl",
                  "templates/curator/template.json"):
            with self.subTest(path=p):
                self.assertIn("persona_source", gp.classify([p]))

    def test_the_surfaces_added_since_the_design_are_sources(self):
        for p in ("templates/talky/schemas/config.json",
                  "templates/cogny/schemas/config.json",
                  "templates/affinity/brief/config.json",
                  "templates/llm-registry/store/seed/models.jsonl",
                  "templates/llm-registry/translate/config.json",
                  "templates/meclaw-os/config.json",
                  "templates/talky/config.json",
                  "templates/cogny/config.json",
                  "workshop/evals/engine.py"):
            with self.subTest(path=p):
                self.assertIn("persona_source", gp.classify([p]))
        self.assertNotIn("evals_persona", gp.classify(["workshop/evals/engine.py"]))

    def test_the_ref_markers_of_the_measured_composites_are_sources(self):
        """A ref marker's override moves the menu and the sidecar switch (OR-GT-S9)."""
        for p in ("templates/talky/collector/config.json",
                  "templates/talky/session-keeper/config.json",
                  "templates/cogny/collector/config.json",
                  "templates/collector/window/config.json"):
            with self.subTest(path=p):
                self.assertIn("persona_source", gp.classify([p]))
        for p in ("templates/dispatcher/config.json", "templates/door/config.json"):
            with self.subTest(path=p):
                self.assertNotIn("persona_source", gp.classify([p]))

    def test_no_pattern_uses_braces_or_classes(self):
        """`_glob_re` knows `**`, `*` and `?` only; a brace would match nothing."""
        for g in gp.PERSONA_SOURCES:
            for ch in "{}[":
                self.assertNotIn(ch, g, g)

    def test_every_persona_source_pattern_matches_a_file(self):
        """Private tree only: templates/egon and templates/slack-agent do not travel."""
        private_tree.require("workshop")
        import os
        files = []
        for dirpath, _dirs, names in os.walk(str(REPO / "templates")):
            for n in names:
                files.append(os.path.relpath(os.path.join(dirpath, n),
                                             str(REPO)).replace(os.sep, "/"))
        files += [p for p in ("workshop/evals/engine.py",) if (REPO / p).is_file()]
        dead = [g for g in gp.PERSONA_SOURCES
                if not any(gp._matches(f, g) for f in files)]
        self.assertEqual([], dead, "PERSONA_SOURCES patterns that match no file")

    def test_the_stations_run_their_scripts_from_the_suite(self):
        st = by_name(gp.plan(["docs/x.md"], "integration", repo=None))
        self.assertEqual([["python3", "fingerprint.py", "--check"]],
                         st["persona-receipt"].cmds)
        self.assertEqual([["python3", "persona_cases.py", "--self-test"],
                          ["python3", "persona_cases.py"]],
                         st["persona-cases"].cmds)
        for name in ("persona-cases", "persona-receipt"):
            for cwd in st[name].cwds:
                self.assertEqual(self.PERSONA, cwd, name)

    def test_the_persona_stations_run_before_the_export_stations(self):
        names = [s.name for s in gp.plan(["docs/x.md"], "release", repo=None)]
        order = ["recall-harness", "deny-advisories", "persona-cases",
                 "persona-receipt", "export-selftest", "export-audit"]
        idx = [names.index(n) for n in order]
        self.assertEqual(sorted(idx), idx, names)

    def test_the_persona_names_are_in_the_register(self):
        register, stations = gp.__doc__.split("STATIONS", 1)
        self.assertIn("    persona_source ", register)
        self.assertIn("    evals_persona ", register)
        self.assertIn("    persona-cases ", stations)
        self.assertIn("    persona-receipt ", stations)

    def test_nested_scenarios_are_counted(self):
        """N may put the scenarios in block sub-folders."""
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = pathlib.Path(tmp.name)
        for rel in ("scenarios/memory/a.json", "scenarios/timing/b.json"):
            f = root / self.PERSONA / rel
            f.parent.mkdir(parents=True, exist_ok=True)
            f.write_text("{}", encoding="utf-8")
        st = by_name(gp.plan([self.PERSONA + "/x.py"], "strand", repo=str(root)))
        self.assertEqual("2 cases", st["persona-cases"].scope)


class Precheck(unittest.TestCase):
    """The cheap form station: first in the plan, no cargo, not in ci.

    Measured over the waves H2/H3/G0: four of twenty-three red strand runs
    (5 771 s) were pure form, and fifty of a hundred and eleven review
    findings were. None of it needs a build, so none of it may wait for the
    lock.
    """

    def test_every_mode_but_ci_plans_it_first(self):
        for mode in ("strand", "integration", "release"):
            st = gp.plan(["docs/x.md"], mode, repo=None)
            self.assertEqual("precheck", st[0].name, mode)
            self.assertEqual("form", st[0].scope, mode)
            self.assertFalse(st[0].cargo, mode)
        self.assertEqual("precheck", gp.STATION_ORDER[0])

    def test_it_carries_the_diff_on_its_argv(self):
        """The runner runs every station with stdin closed, so `--files` it is."""
        paths = ["crates/meclaw-core/src/lib.rs", "docs/x.md"]
        st = by_name(gp.plan(paths, "strand", repo=None))
        self.assertEqual([["python3", "scripts/precheck.py", "--files"] + sorted(paths)],
                         st["precheck"].cmds)

    def test_an_empty_diff_plans_no_precheck(self):
        self.assertNotIn("precheck", {s.name for s in gp.plan([], "strand", repo=None)})

    def test_ci_never_plans_it(self):
        """The published tree has no `plans/`, and ci's diff is often the whole tree.

        Three of the ten checks read the export's own lists and would fall
        silent there; `fmt` is the `fmt` station's job in that mode anyway;
        and a full-tree diff offers files nobody changed, which turns the two
        artefact notes into a fixture of every workflow log.
        """
        self.assertIn("precheck", gp.CI_EXCLUDED)
        self.assertNotIn("precheck",
                         {s.name for s in gp.plan(["docs/x.md"], "ci", repo=None)})

    def test_the_gate_selftest_runs_its_tests_too(self):
        st = by_name(gp.plan(["scripts/precheck.py"], "strand", repo=None))
        self.assertIn("scripts.tests.test_precheck", st["gate-selftest"].cmds[0])
        self.assertIn("gate_infra", gp.classify(["scripts/precheck.py"]))


class RuntimeReadsInTheTree(unittest.TestCase):
    """Rule 8 runtime (GH #1138) against the real tree: the W6 replay case R18."""

    SEED = "(binary_id(=meclaw-cells::misc_8) & test(/^librarian_seed_corpus::/))"

    def setUp(self):
        private_tree.require("crates/meclaw-cells/tests/misc_8/librarian_seed_corpus.rs")

    def test_a_spec_page_the_seed_generator_reads_runs_the_seed_lock(self):
        """`docs/cell-types.en.md` is named by no test -- the seed test reads it
        through `build_librarian_seed.py` (CORPUS_SOURCES)."""
        terms = gp.test_terms(["docs/cell-types.en.md"], str(REPO))
        self.assertIn(self.SEED, terms)
        self.assertIn("rule:8 runtime script:workshop/tools/build_librarian_seed.py "
                      "file:docs/cell-types.en.md", terms[self.SEED])

    def test_a_crate_fixture_is_never_planned_empty(self):
        """Before #1138 a diff to a crate's test fixture planned no `tests` at all."""
        sel = gp.test_select(["crates/meclaw-cells/tests/fixtures/canvy_arranged_colony.json"],
                             "strand", str(REPO))
        self.assertIsNotNone(sel)

    def test_a_crate_fixture_runs_the_tests_that_walk_crates(self):
        """tb-s1c (#1177): two tests walk the bare `crates` -- the text lock of
        gh523 and the corpus claims of gh230. Before the fix a repo-relative
        literal under crates/ counted only with three `/`, so none of the 83
        json fixtures under `crates/*/tests/` planned either walker."""
        fixture = "crates/meclaw-cells/tests/fixtures/canvy_arranged_colony.json"
        walkers = [(c, u) for c, u in gp._runtime_index(str(REPO)).get("crates", ())]
        self.assertIn(("meclaw-cells", "misc_5/gh523_the_per_turn_lane_is_not_a_batch"), walkers)
        have = gp.test_terms([fixture], str(REPO))
        for crate, unit in walkers:
            with self.subTest(unit=unit):
                self.assertEqual([], uncovered(REPO, have, gp._unit_users(str(REPO), crate, unit)))

    # `git ls-files` of the private tree: the export audit runs this module
    # in a published tree without a repository (R15, exit 1). Marker `plans/`
    # as in IncludeTargetsStayInReach (GH #1168).
    @private_tree.private()
    def test_a_file_only_the_crates_walkers_reach_stays_unread(self):
        """tb-s1c M-1: gh523 and gh230 walk the bare `crates`, so every crate
        fixture has them as planned readers -- but gh523 reads json|rs|manifest
        and gh230 skips tests/. A file no test of its own names stays `unread`
        (rdeps of its crate, KR-TB-1 (c)): the HARNESS.toml files, 7 at
        2964faed3, 0 at 7ffa835e4."""
        files = subprocess.run(["git", "-C", str(REPO), "ls-files", "--",
                                "crates/*/tests/HARNESS.toml"], capture_output=True,
                               text=True, check=True).stdout.splitlines()
        self.assertTrue(files)
        walkers = {t for c, u in gp._runtime_index(str(REPO)).get("crates", ())
                   for t in gp._unit_users(str(REPO), c, u)}
        for rel in files:
            with self.subTest(rel):
                crate = rel.split("/")[1]
                self.assertEqual("rule:8 runtime unread file:%s" % rel,
                                 gp._runtime_readers(str(REPO), rel).get((None, crate)))
                terms = gp.test_terms([rel], str(REPO))
                if "all()" in terms:
                    continue   # meclaw-testing: rule 1 plans the whole suite
                self.assertIn("rule:8 runtime unread file:%s" % rel,
                              terms.get("rdeps(%s)" % crate, ()))
                self.assertLessEqual(walkers, set(terms))


class ShortCratePathWalkers(unittest.TestCase):
    """Rule 8 runtime, repo-relative directories under crates/ of any depth
    (tb-s1c, #1177): a test that walks `crates`, `crates/<c>` or
    `crates/<c>/tests` reads every fixture below it, from any crate. The
    crate-relative bare `tests` stays out (every binary's home)."""

    def repo(self):
        root = pathlib.Path(reader_repo(self, {
            "walks_crates": 'fn t() { for d in ["templates", "examples", "crates", "tests"] '
                            '{ walk(&repo(d)); } }\n#[test]\nfn x() {}\n',
            "walks_cli": 'fn t() { walk(&repo("crates/meclaw-cli")); }\n#[test]\nfn x() {}\n',
            "walks_cli_tests": 'fn t() { walk(&repo("crates/meclaw-cli/tests")); }\n'
                               '#[test]\nfn x() {}\n',
            "walks_own_tests": 'fn t() { walk(&dir("tests")); }\n#[test]\nfn x() {}\n'}))
        for d in ("templates", "examples", "tests"):
            (root / d).mkdir()
        cli = root / "crates" / "meclaw-cli"
        (cli / "tests" / "data").mkdir(parents=True)
        (cli / "Cargo.toml").write_text('[package]\nname = "meclaw-cli"\n', encoding="utf-8")
        (cli / "tests" / "data" / "x.json").write_text("{}\n", encoding="utf-8")
        return str(root)

    def test_every_walker_of_a_directory_under_crates_is_planned(self):
        repo = self.repo()
        terms = gp.test_terms(["crates/meclaw-cli/tests/data/x.json"], repo)
        for stem, lit in (("walks_crates", "crates"), ("walks_cli", "crates/meclaw-cli"),
                          ("walks_cli_tests", "crates/meclaw-cli/tests")):
            with self.subTest(stem):
                term = gp._binary_id("meclaw-cells", stem)
                self.assertIn(term, terms)
                self.assertEqual(["rule:8 runtime dir:%s file:crates/meclaw-cli/tests/data/x.json"
                                  % lit], terms[term])
        # another crate's bare `tests` is its own tests/, not meclaw-cli's
        self.assertNotIn(gp._binary_id("meclaw-cells", "walks_own_tests"), terms)

    def test_a_walker_plans_along_but_never_makes_the_file_read(self):
        """tb-s1c M-1: a walk of `crates`, `crates/<c>` or `crates/<c>/tests`
        says nothing of the one file -- `x.json` has no reader of its own, so
        rdeps(meclaw-cli) stays beside the walkers (`unread`, fail-closed)."""
        terms = gp.test_terms(["crates/meclaw-cli/tests/data/x.json"], self.repo())
        self.assertEqual(["rule:8 runtime unread file:crates/meclaw-cli/tests/data/x.json"],
                         terms.get("rdeps(meclaw-cli)"))


# Rule 10 (welle-tb S1b, follow-up of the re-check): a file inside a crate that is
# neither under src/ nor under tests/ (nor build.rs / Cargo.toml) carried NO class
# and planned no `tests`, no fmt, no clippy. The real case is
# `crates/meclaw-core/schemas/ubf-body.json`, compiled into meclaw-core by
# `include_str!`. One path per shape a crate can hold outside src/ and tests/.
CRATE_FILES = {
    "include_str schema": "crates/meclaw-core/schemas/ubf-body.json",
    "example": "crates/meclaw-cli/examples/phase_1_demo.rs",
    "bench": "crates/meclaw-core/benches/route.rs",
    "crate readme (doc include)": "crates/meclaw-core/README.md",
    "sql migration": "crates/meclaw-core/migrations/0001_init.sql",
    "templates inside a crate": "crates/meclaw-core/templates/body/config.json",
    "toml beside Cargo.toml": "crates/meclaw-core/config.toml",
    "build.rs input": "crates/meclaw-core/build/gen.in",
}


class CrateFilesOutsideSrcAndTests(unittest.TestCase):
    """A crate file no rule can assign to a test binary plans the crate's tests
    and those of its reverse dependencies -- `rdeps(<c>)`, like a Rust source."""

    def test_every_crate_file_shape_carries_the_class(self):
        for shape, rel in CRATE_FILES.items():
            self.assertIn("crate_file", gp._classes_of_path(rel), shape)
        for rel in ("crates/meclaw-core/src/schema.rs", "crates/meclaw-core/build.rs",
                    "crates/meclaw-core/Cargo.toml", "crates/meclaw-core/tests/HARNESS.toml",
                    "crates/meclaw-cells/src/web/shell.js"):
            self.assertNotIn("crate_file", gp._classes_of_path(rel), rel)

    def test_a_crate_file_plans_rdeps_of_its_crate(self):
        for shape, rel in CRATE_FILES.items():
            crate = rel.split("/")[1]
            terms = gp.test_terms([rel], str(REPO))
            want = "rdeps(%s)" % crate
            self.assertIn(want, terms, (shape, terms))
            self.assertIn("rule:10 crate-file:%s" % rel, terms[want], shape)

    def test_a_crate_file_plans_the_cargo_stations_of_a_rust_source(self):
        for shape, rel in CRATE_FILES.items():
            crate = rel.split("/")[1]
            st = by_name(gp.plan([rel], "strand", repo=str(REPO)))
            for name in ("fmt", "clippy", "unwrap-budget", "tests", "slow-tests"):
                self.assertIn(name, st, (shape, name))
            self.assertIn("rdeps(%s)" % crate, st["tests"].scope, shape)
            self.assertIn("-p", st["clippy"].cmds[0], shape)
            self.assertIn(crate, st["clippy"].cmds[0], shape)
            ir = by_name(gp.plan([rel], "integration", repo=str(REPO)))
            self.assertIn("doctests", ir, shape)
            self.assertIn("rdeps(%s)" % crate, ir["tests"].scope, shape)

    def test_clippy_lints_what_the_test_builds_compile(self):
        """R-TV-W18 (GH #1208): clippy carries `meclaw-colony/test-hooks` exactly
        where the test builds do (the one rule in scripts/cargo-target.sh), so
        the code behind the feature is linted; a selection without meclaw-colony
        gets nothing (cargo refuses the flag there). The unwrap budget stays the
        product's view, without the feature."""
        flag = ["--features", "meclaw-colony/test-hooks"]
        col = by_name(gp.plan(["crates/meclaw-colony/src/lib.rs"], "strand", repo=str(REPO)))
        self.assertEqual([["cargo", "clippy", "-p", "meclaw-colony", "--all-targets"] + flag
                          + ["--", "-D", "warnings"]], col["clippy"].cmds)
        core = by_name(gp.plan(["crates/meclaw-core/src/lib.rs"], "strand", repo=str(REPO)))
        self.assertEqual([["cargo", "clippy", "-p", "meclaw-core", "--all-targets", "--",
                           "-D", "warnings"]], core["clippy"].cmds)
        self.assertEqual([["python3", ".github/gates/unwrap_budget.py"]],
                         col["unwrap-budget"].cmds)

    def test_the_cargo_argv_of_a_crate_file_is_that_of_a_rust_source(self):
        """Re-check S1b m-7: the whole argv, strand and integration, not just `-p <c>`.
        An example or a bench is linted only with `--all-targets`."""
        for shape, rel in CRATE_FILES.items():
            crate = rel.split("/")[1]
            src = "crates/%s/src/lib.rs" % crate
            st = by_name(gp.plan([rel], "strand", repo=str(REPO)))
            hooks = gp.test_hook_flags(str(REPO), ["-p", crate])
            self.assertEqual([["cargo", "clippy", "-p", crate, "--all-targets"] + hooks
                              + ["--", "-D", "warnings"]], st["clippy"].cmds, shape)
            self.assertEqual(by_name(gp.plan([src], "strand", repo=str(REPO)))["clippy"].cmds,
                             st["clippy"].cmds, shape)
            ir = by_name(gp.plan([rel], "integration", repo=str(REPO)))
            self.assertEqual([["cargo", "clippy", "--workspace", "--all-targets", "--features",
                               "meclaw-colony/test-hooks", "--", "-D", "warnings"]],
                             ir["clippy"].cmds, shape)
            self.assertEqual([["cargo", "test", "--workspace", "--doc", "--features",
                               "meclaw-colony/test-hooks"]], ir["doctests"].cmds, shape)
            irs = by_name(gp.plan([src], "integration", repo=str(REPO)))
            self.assertEqual(irs["clippy"].cmds, ir["clippy"].cmds, shape)
            self.assertEqual(irs["doctests"].cmds, ir["doctests"].cmds, shape)

    def test_explain_names_the_crate_file_for_every_station_it_plans(self):
        rel = CRATE_FILES["include_str schema"]
        doc = gp.to_json([rel], "strand", str(REPO), explain=True)
        # re-check S1b m-6: without this the loop below is vacuously green on a
        # tree that plans none of the four stations for the file
        self.assertLessEqual({"fmt", "clippy", "unwrap-budget", "tests"},
                             {s["name"] for s in doc["stations"]})
        for st in doc["stations"]:
            if st["name"] in ("fmt", "clippy", "unwrap-budget", "tests"):
                self.assertIn("crate_file: %s" % rel, st["reason"], st["name"])


# M-6 (welle-tb S1b, re-check of cfbbcc266; was m-9, OR-TB.S1b.22): rule 10 plans a
# file by the crate directory it lies in. An include macro whose target lies outside
# crates/<c>/ and outside every crate <c> depends on compiles into <c>, but a diff to
# that target plans no test of <c> -- silently, not even a NOTE. M-7 (re-check
# 1a6429420): the same holds inside reach -- a fixture under crates/<d>/tests/ is
# planned by the tests that name it (rule 8), so the guard asks `test_terms`.
_INCLUDE = re.compile(r"\binclude(?:_str|_bytes)?!\s*\(\s*")


def include_targets(repo):
    """`[(source, target)]` of every include macro in a tracked crate file; the
    target relative to the repository, None for an argument that is no plain
    string literal (the guard cannot follow it: doubt, red)."""
    out = []
    for rel in subprocess.run(["git", "-C", str(repo), "ls-files", "--", "crates/*.rs"],
                              capture_output=True, text=True, check=True).stdout.split():
        text = (pathlib.Path(repo) / rel).read_text(encoding="utf-8", errors="replace")
        for m in _INCLUDE.finditer(text):
            lit = re.match(r'"([^"\\]*)"\s*[,)]', text[m.end():])
            out.append((rel, os.path.normpath(os.path.join(os.path.dirname(rel), lit.group(1)))
                        if lit else None))
    return out


def path_deps(repo, crate):
    """The crate directories `crates/<crate>/Cargo.toml` names as path dependencies
    (none for a directory without one)."""
    import tomllib
    toml = pathlib.Path(repo) / "crates" / crate / "Cargo.toml"
    if not toml.is_file():
        return set()
    doc = tomllib.loads(toml.read_text())
    tables = [doc.get(k) or {} for k in ("dependencies", "dev-dependencies", "build-dependencies")]
    tables += [t.get(k) or {} for t in (doc.get("target") or {}).values()
               for k in ("dependencies", "dev-dependencies", "build-dependencies")]
    return {os.path.basename(os.path.normpath(v["path"])) for t in tables for v in t.values()
            if isinstance(v, dict) and "path" in v}


def _reach(repo, crate):
    """`crate` and every crate it reaches over path dependencies: the X whose
    `rdeps(X)` runs `crate`."""
    seen, todo = set(), [crate]
    while todo:
        c = todo.pop()
        if c not in seen:
            seen.add(c)
            todo.extend(path_deps(repo, c))
    return seen


def _term_crate(term):
    """The crate a `rdeps(c)` or `binary_id(=c::..)` term runs in, else None."""
    m = re.fullmatch(r"rdeps\(([^)]+)\)|\(?binary_id\(=([^:)]+)::.*", term)
    return m and (m.group(1) or m.group(2))


def covers(repo, have, want):
    """Does the planned term `have` run every test the term `want` runs?
    `all()` covers all; `rdeps(X)` covers a `rdeps(c)` or `binary_id(=c::..)`
    whose crate c reaches X; `binary_id(=b)` covers `(binary_id(=b) & test(..))`;
    a term covers itself. Anything else is not covered (fail-closed)."""
    if have == "all()" or have == want:
        return True
    m = re.fullmatch(r"rdeps\(([^)]+)\)", have)
    if m:
        crate = _term_crate(want)
        return bool(crate) and m.group(1) in _reach(repo, crate)
    m = re.fullmatch(r"binary_id\(=[^)]+\)", have)
    return bool(m) and want.startswith("(%s & test(" % have) and want.endswith("))")


def code_terms(terms):
    """The terms of a `test_terms` plan that run the planned file as CODE: the
    ones a rule other than 4, 5 and 8 brought in (1 all, 2 rdeps, 3 the
    binary, its harness, its sharers, 10 a crate file). A rule-4/5/8 term
    reads the file as text -- since tb-s1c every walker of `crates` reads
    every `.rs` (gh523, gh230) -- and a diff to what the file includes or
    reads owes its users, not its readers."""
    return {t for t, reasons in terms.items()
            if any(not r.startswith(("rule:4 ", "rule:5 ", "rule:8 ")) for r in reasons)}


def uncovered(repo, have, want):
    """The terms of `want` no term of `have` covers."""
    return sorted(w for w in want if not any(covers(repo, h, w) for h in have))


def unplanned_includes(repo, pairs):
    """The `source -> target` lines of `pairs` whose target diff does not run
    the source: `test_terms([target])` does not cover `test_terms([source])`
    term by term (`covers`). A source that plans nothing, or a target that is
    no plain string literal, is doubt and red."""
    bad = []
    for src, target in pairs:
        want = code_terms(gp.test_terms([src], str(repo)))
        if not target or not want or uncovered(repo, gp.test_terms([target], str(repo)), want):
            bad.append("%s -> %s" % (src, target or "<no string literal>"))
    return bad


def _unit_reader_files(repo, crate, unit):
    """The FILES of a `_test_sources` unit: `tests/<stem>.rs`, `tests/<h>/<m>.rs`;
    for `<h>/main` its main.rs and every module main.rs does not declare at its
    top level -- `_folded_unit` names those `<h>/main` as well, so the unit is
    no file (tb-s1c m-16). The guard plans each file (rule 3), not the unit."""
    base = "crates/%s/tests/" % crate
    if not unit.endswith("/main") or gp.is_helper(str(repo), crate, unit):
        return [base + unit + ".rs"]
    harness = unit[:-len("/main")]
    hdir = pathlib.Path(repo) / base / harness
    main = (hdir / "main.rs").read_text(encoding="utf-8", errors="replace")
    return sorted(base + harness + "/" + f.name for f in hdir.glob("*.rs")
                  if f.name == "main.rs" or gp._folded_unit(main, harness, f.stem) == unit)


def reader_pairs(repo, files):
    """`[(reader, read)]` of rule 8 in full over the tracked `files` (tb-s1c
    m-17): a FILE of a test unit (`_unit_reader_files`) and every file it reads.

      literal  a file outside crates/ (or a crate file) the reader text names
               (`_tests_reading_path`, both `_path_literals` spellings);
      runtime  below a directory the unit walks (`runtime_roots`) EVERY file
               the walk admits (`runtime_dir_applies`, per unit), not the first one; a
               walk written repo-relative (`crates`, `crates/<c>/src`) reads
               every crate file below, `.rs` and src/ included (gh523 reads
               `.rs`/`.json`/`.manifest` below `crates`, gh230 every `.rs`); a
               crate-relative one (`"voice"` -> `crates/<c>/tests/voice`) only
               the non-`.rs` files of tests/ -- a bare cell name is no walk; a
               crate file named whole (a `#[path]` target too);
      script   every file a SCRIPT_READS glob matches, for a unit that names
               the script in its reader text;
      dotdot   a literal with `..`, normalised (`_resolve_dotdot`): a file
               outside crates/ named whole that way, and for a literal no base
               resolves EVERY tracked file (tb-s1c-b).
    A file outside crates/ named whole is rule 8 literal (a prose mention
    reads nothing, `_reader_text`)."""
    tracked = sorted(set(files))
    known = set(tracked)
    idx = gp._runtime_index(str(repo))
    sources = gp.reader_sources(str(repo))   # the helpers without main.rs too (F4)
    out = set()

    def add(crate, unit, read):
        for reader in _unit_reader_files(repo, crate, unit):
            out.add((reader, read))

    def fixture(f):
        return f.split("/")[2:3] == ["tests"] and not f.endswith(".rs")

    spelled = gp.runtime_spelled(str(repo))
    for crate, unit in sorted(gp.runtime_any(str(repo))):
        for f in tracked:   # a `..` literal no base resolves reads every path
            add(crate, unit, f)
    targets = known | {a for f in tracked for a in gp.runtime_ancestors(f)}
    for (crate, unit), paths in gp.runtime_roots(str(repo), targets).items():
        for p in paths:
            as_written = (crate, unit) in idx.get(p, ())
            if p in known:
                # a file outside crates/ named whole is rule 8 literal -- unless
                # the reader spelled it with `..` (tb-s1c-b), the literal rule
                # then misses it and the runtime rule plans it
                if p.startswith("crates/") or (crate, unit) in spelled.get(p, ()):
                    add(crate, unit, p)
                continue
            for f in tracked:
                if f.startswith(p + "/") and gp.runtime_dir_applies(p, f, unit) and (
                        not f.startswith("crates/") or as_written or fixture(f)):
                    add(crate, unit, f)
    named = [f for f in tracked
             if not f.startswith("crates/") or "crate_file" in gp._classes_of_path(f)]
    # a literal contains its first segment (and the `/` after it): an exact
    # prefilter, so each source tests only the files of the tops it names
    by_top = collections.defaultdict(list)
    for f in named:
        by_top[f.split("/", 1)[0] + ("/" if "/" in f else "")].append((f, gp._path_literals(f)))
    for crate, unit, text in sources:
        rtext = None
        for top, entries in by_top.items():
            if top not in text:
                continue
            for f, lits in entries:
                if any(lit in text for lit in lits):
                    rtext = gp.reader_text(text) if rtext is None else rtext
                    if any(lit in rtext for lit in lits):
                        add(crate, unit, f)
    for script, globs in gp.SCRIPT_READS:
        users = [(c, u) for c, u, text in sources
                 if script in text and script in gp.reader_text(text)]
        for f in tracked if users else ():
            if any(gp._matches(f, g) for g in globs):
                for c, u in users:
                    add(c, u, f)
    return sorted(out)


def template_reader_pairs(repo, files):
    """`[(reader, read)]` of rules 4/5 over the tracked `files`: a file of a
    test unit that references a template (an example) and the first tracked
    file of it. One file per name is exact: rules 4/5 plan the same terms for
    every file of the name (a file-specific term comes from rule 8)."""
    out = set()
    for top, names, find in (("templates", gp._templates_of(files), gp.template_readers),
                             ("examples", gp._examples_of(files), gp.example_readers)):
        for name in sorted(names):
            read = min(f for f in files if f.startswith("%s/%s/" % (top, name)))
            for crate, unit in find(str(repo), name):
                for reader in _unit_reader_files(repo, crate, unit):
                    out.add((reader, read))
    return sorted(out)


def unplanned_readers(repo, pairs):
    """The `reader -> read` lines whose read diff does not plan all the
    reader's own diff runs as code (`code_terms`, `covers` term by term): a
    helper reads for its harness, a shared file for its sharers. No pair is skipped (tb-s1c m-17):
    a read whose diff does not plan the reader at all is the widest miss --
    gh523 walks `crates`, `runtime_roots` knew it, the plan did not, and the
    old skip ("not planned as this unit's read") kept it silent. A reader
    whose own diff plans nothing (rule 9: no test) has nothing to miss."""
    bad, plans = [], {}

    def plan(path):
        # once per path: `setdefault(path, test_terms(..))` planned it on every
        # pair (tb-s1c: 10 501 template pairs, 470 s -> one plan per path)
        if path not in plans:
            plans[path] = gp.test_terms([path], str(repo))
        return plans[path]

    for reader, read in pairs:
        want = set()
        for f in helper_includer_units(repo, reader):
            want |= code_terms(plan(f))
        if uncovered(repo, plan(read), want or code_terms(plan(reader))):
            bad.append("%s -> %s" % (reader, read))
    return bad


def helper_includer_units(repo, reader):
    """The unit FILES that compile the helper file `reader` in, through other
    helpers (`gp.includers`); [] for a unit file or a helper nobody includes.
    A helper (no binary) reads for those files: the guard wants what THEIR
    diffs run as code (rule 3, independent of `_helper_users`), not what the
    helper's own diff plans -- rule 2 plans `rdeps` for all of tests/support/."""
    files = gp.test_files(str(repo))[0]
    if reader not in files or not files[reader][2]:
        return []
    inc, seen, todo, out = gp.includers(str(repo)), {reader}, [reader], set()
    while todo:
        for f in inc.get(todo.pop(), ()):
            if f not in seen:
                seen.add(f)
                (todo.append(f) if files[f][2] else out.add(f))
    return sorted(out)


def family_repo(case):
    """The include/reader family of M-7/M-8 on a throw-away tree: meclaw-cells
    with a fixture its flat test `gh1_reads` names and includes, a folded
    harness `h` (helper `helper`, test modules `t1`/`t2`), a flat `gh_s` that
    the harness `h2` `#[path]`s in, and meclaw-cli, a path dependent."""
    root = pathlib.Path(reader_repo(case, {
        "gh1_reads": 'const X: &str = include_str!("fixtures/x.jsonl");\n#[test]\nfn t() {}\n',
        "gh_s": 'const Z: &str = include_str!("fixtures/z.json");\n#[test]\nfn t() {}\n'}))
    cells = root / "crates" / "meclaw-cells"
    cli = root / "crates" / "meclaw-cli"
    texts = {
        cells / "src" / "lib.rs": 'const X: &str = include_str!("../tests/fixtures/x.jsonl");\n',
        cells / "schemas" / "s.json": "{}\n",
        cells / "tests" / "h" / "main.rs": "mod helper;\nmod t1;\nmod t2;\nmod t3;\nmod t4;\n"
                                           '#[path = "deep.rs"]\nmod deep_mod;\n',
        cells / "tests" / "h" / "deep.rs": 'const D: &str = "docs/deep";\n#[test]\nfn t() {}\n',
        cells / "tests" / "h3" / "main.rs": '#[path = "../h/deep.rs"]\nmod deep;\n',
        cells / "tests" / "hs" / "mod.rs": "pub fn s() {}\n",
        cells / "tests" / "gh_p.rs": '#[path = "hs/mod.rs"]\nmod hs;\n#[test]\nfn t() {}\n',
        cells / "tests" / "gh_w.rs":
            'fn r() { walk(&repo().join("crates")); }\n#[test]\nfn t() {}\n',
        cells / "tests" / "h" / "helper.rs":
            'pub const Y: &str = include_str!("../fixtures/y.json");\n'
            'pub fn guide() -> &\'static str { "docs/guide" }\n'
            'pub fn tpl() -> &\'static str { "templates/tpl" }\n'
            'pub fn ex() -> &\'static str { "examples/ex" }\n'
            'pub fn seed() -> &\'static str { "workshop/tools/build_librarian_seed.py" }\n',
        cells / "tests" / "h" / "t4.rs":
            'const T: &str = "templates/tpl";\nconst E: &str = "examples/ex";\n#[test]\nfn t() {}\n',
        cells / "tests" / "h" / "t1.rs": "use crate::helper;\n#[test]\nfn t() { helper::guide(); }\n",
        cells / "tests" / "h" / "t2.rs":
            'const W: &str = include_str!("../fixtures/w.json");\n#[test]\nfn t() {}\n',
        cells / "tests" / "h" / "t3.rs": 'const D: &str = "docs/other";\n#[test]\nfn t() {}\n',
        cells / "tests" / "h2" / "main.rs": '#[path = "../gh_s.rs"]\nmod s;\n',
        cli / "Cargo.toml": '[package]\nname = "meclaw-cli"\n[dependencies]\n'
                            'meclaw-cells = { path = "../meclaw-cells" }\n',
        cli / "src" / "lib.rs": 'const S: &str = include_str!("../../meclaw-cells/schemas/s.json");\n',
        cli / "src" / "main.rs": 'const S: &str = include_str!("../../meclaw-cells/schemas/s.json");\n',
        cli / "tests" / "uses_fixture.rs":
            'const X: &str = include_str!("../../meclaw-cells/tests/fixtures/x.jsonl");\n'
            '#[test]\nfn t() {}\n',
        root / "docs" / "guide" / "a.md": "a\n",
        root / "docs" / "other" / "b.md": "b\n",
        root / "docs" / "deep" / "c.md": "c\n",
        root / "docs" / "config.en.md": "c\n",
        root / "templates" / "tpl" / "template.json": "{}\n",
        root / "examples" / "ex" / "README.md": "e\n",
    }
    for name in ("x.jsonl", "y.json", "w.json", "z.json"):
        texts[cells / "tests" / "fixtures" / name] = "{}\n"
    for path, text in texts.items():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
    return root


class IncludeTargetsStayInReach(unittest.TestCase):
    """A diff to an include target runs everything a diff to the includer runs:
    `test_terms([target])` covers `test_terms([includer])` term by term
    (`covers`) -- the same planning function on both sides, so helpers,
    sharers, flat and folded test files, lib and bin sources fall under one
    rule (M-8). The same for rule 8 runtime: a path a test unit reads plans,
    with that unit, all its file's own diff plans."""

    # The two real-tree halves walk `git ls-files` of the private tree, the
    # one the strand gate narrows in. The export audit runs this module in a
    # published tree without a repository, where both died at the first git
    # call (R15, exit 1). The marker is `plans/`, as for the gh466 reader: in
    # the private tree a missed includer must still go red.
    @private_tree.private()
    def test_every_include_target_is_planned_with_its_includer(self):
        found = include_targets(REPO)
        # not vacuous (re-check S1b m-6): the schema include rule 10 was made for
        self.assertIn(("crates/meclaw-core/src/schema.rs",
                       "crates/meclaw-core/schemas/ubf-body.json"), found)
        self.assertEqual([], unplanned_includes(REPO, found))

    def test_the_family_is_red_exactly_where_the_plan_misses_the_includer(self):
        """M-7/M-8 pins on a throw-away tree. Before the rule-8 fix (a reader
        stood only for its own unit, emulated here) red: P4 src/ beside a
        named fixture, P5 another crate's test, P6 a folded helper (the
        fixture planned `h & helper`, the helper plans the whole `h`), P7 a
        `#[path]`-shared file (planned `gh_s`, not `h2`), P8 a helper reading
        a directory at runtime, P10/P11 a helper referencing a template / an
        example (rules 4/5, tb-s1c), P12 a module its harness main.rs does not
        declare at the top level (`_test_sources` names it `h/main`, rule 3
        plans the FILE with its sharer `h3`; m-16), P15 a helper naming a
        SCRIPT_READS generator. With it P6-P8, P10-P12, P15 are planned and
        green; P4/P5 stay red (an include beside a named fixture from outside
        the reader, none in the tree). Red on 7ffa835e4 in both, green after
        tb-s1c's second round: P13 a `#[path]` target in a directory without
        main.rs (rule 3 planned nothing: no binary, sharers never asked), P14
        a test walking `crates` and a src file of a crate it does not depend
        on (rule 8 runtime skipped every crate file but tests/ fixtures).
        Green in both: a flat and a folded test that include (or read, or
        reference) what they name, a lib and a bin source including a rule-10
        file of a path dependency."""
        repo = family_repo(self)
        cells, cli = "crates/meclaw-cells/", "crates/meclaw-cli/"
        fx = cells + "tests/fixtures/"
        includes = {
            "P4": (True, True, (cells + "src/lib.rs", fx + "x.jsonl")),
            "P5": (True, True, (cli + "tests/uses_fixture.rs", fx + "x.jsonl")),
            "P6": (True, False, (cells + "tests/h/helper.rs", fx + "y.json")),
            "P7": (True, False, (cells + "tests/gh_s.rs", fx + "z.json")),
            "flat": (False, False, (cells + "tests/gh1_reads.rs", fx + "x.jsonl")),
            "folded": (False, False, (cells + "tests/h/t2.rs", fx + "w.json")),
            "lib": (False, False, (cli + "src/lib.rs", cells + "schemas/s.json")),
            "bin": (False, False, (cli + "src/main.rs", cells + "schemas/s.json")),
        }
        readers = {"P8": (True, False, (cells + "tests/h/helper.rs", "docs/guide/a.md")),
                   "reader": (False, False, (cells + "tests/h/t3.rs", "docs/other/b.md")),
                   "P10": (True, False, (cells + "tests/h/helper.rs", "templates/tpl/template.json")),
                   "P11": (True, False, (cells + "tests/h/helper.rs", "examples/ex/README.md")),
                   "tpl": (False, False, (cells + "tests/h/t4.rs", "templates/tpl/template.json")),
                   "ex": (False, False, (cells + "tests/h/t4.rs", "examples/ex/README.md")),
                   "P12": (True, False, (cells + "tests/h/deep.rs", "docs/deep/c.md")),
                   "P13": (False, False, (cells + "tests/gh_p.rs", cells + "tests/hs/mod.rs")),
                   "P14": (False, False, (cells + "tests/gh_w.rs", cli + "src/lib.rs")),
                   "P15": (True, False, (cells + "tests/h/helper.rs", "docs/config.en.md"))}
        files = [str(p.relative_to(repo)) for p in repo.rglob("*") if p.is_file()]
        own_unit = lambda _repo, crate, unit: {gp._binary_id(crate, unit)}
        for fixed in (False, True):
            with mock.patch.object(gp, "_unit_users", gp._unit_users if fixed else own_unit):
                pairs = reader_pairs(repo, files) + template_reader_pairs(repo, files)
                for name, (red_before, red_after, pair) in includes.items():
                    with self.subTest(name, fixed=fixed):
                        self.assertEqual(red_after if fixed else red_before,
                                         bool(unplanned_includes(repo, [pair])))
                for name, (red_before, red_after, pair) in readers.items():
                    with self.subTest(name, fixed=fixed):
                        self.assertIn(pair, pairs)
                        self.assertEqual(red_after if fixed else red_before,
                                         bool(unplanned_readers(repo, [pair])))


class RuleEightReadersInTheTree(unittest.TestCase):
    """Rule 8 in full against the real tree (tb-s1c m-17, #1177): every file a
    test unit names, walks or runs a generator over plans, with the unit,
    all the reader FILE's own diff plans -- every file below a walked
    directory, no pair skipped. On 7ffa835e4: 6 826 of 15 961 pairs red
    (gh230/gh523 walk `crates` and read `.rs` everywhere; `h5_support/mod.rs`
    and `gh927/*.rs` planned nothing). Its own class: ~110 s, every one of
    ~3 000 read files planned once; a sample would miss the one file that
    plans nothing."""

    # Successor of test_every_runtime_read_is_planned_with_its_reader, so it
    # carries its published-tree skip (GH #1168): `git ls-files` of the
    # private tree, no repository in the export audit (R15, exit 1).
    @private_tree.private()
    def test_every_rule_8_read_is_planned_with_its_reader(self):
        files = subprocess.run(["git", "-C", str(REPO), "ls-files"], capture_output=True,
                               text=True, check=True).stdout.splitlines()
        pairs = reader_pairs(REPO, files)
        self.assertGreater(len(pairs), 10000)   # not vacuous
        cells = "crates/meclaw-cells/tests/"
        for pair in ((cells + "misc_5/gh523_the_per_turn_lane_is_not_a_batch.rs",
                      "crates/meclaw/src/main.rs"),   # the binary since GH #1168
                     (cells + "misc_8/main.rs", cells + "h5_support/mod.rs")):
            self.assertIn(pair, pairs)
        self.assertEqual([], unplanned_readers(REPO, pairs))


class TemplateReadersInTheTree(unittest.TestCase):
    """Rules 4/5 against the real tree (tb-s1c, #1177): a diff to a template
    (an example) runs, with every unit that references it, all that unit's
    own diff runs -- a helper references for its harness, a shared file for
    its sharers. Before the fix 2 215 of 10 501 pairs ran only the
    referencing module (98 readers, 24 binaries). Its own class because it is
    the slow one: every one of ~65 names and ~700 readers planned once
    (measured in the report); a sample would miss exactly the rare helper."""

    # `git ls-files` of the private tree, as RuleEightReadersInTheTree.
    @private_tree.private()
    def test_every_template_and_example_reader_is_planned_with_its_users(self):
        files = subprocess.run(["git", "-C", str(REPO), "ls-files"], capture_output=True,
                               text=True, check=True).stdout.splitlines()
        pairs = template_reader_pairs(REPO, files)
        self.assertGreater(len(pairs), 1000)   # not vacuous
        self.assertEqual([], unplanned_readers(REPO, pairs))


def dotdot_repo(case):
    """Readers that spell a path with `..` (E1-E5) and readers that are
    helpers in a directory without main.rs (H1-H3), on a throw-away tree."""
    root = pathlib.Path(reader_repo(case, {
        "gh_named": 'const N: &str = "tests/fixtures/data.json";\n#[test]\nfn t() {}\n',
        "gh_dd": 'const A: &str = "tests/fixtures/sub/../data.json";\n'
                 'const B: &str = "docs/sub/../dd.md";\n#[test]\nfn t() {}\n',
        "gh_top": 'const C: &str = "a/../../../../../../x";\n#[test]\nfn t() {}\n',
        "gh_lnk": 'const L: &str = "tests/lnk/../moved.json";\n#[test]\nfn t() {}\n',
        "gh_root": 'const R: &str = "sub/../../..";\n#[test]\nfn t() {}\n',
        "gh_m": "mod mchain;\n#[test]\nfn t() {}\n",
        "gh_ok": 'const O: &str = "tests/fixtures/data.json";\n#[test]\nfn t() {}\n'}))
    tests = root / "crates" / "meclaw-cells" / "tests"
    texts = {
        tests / "fixtures" / "data.json": "{}\n",
        tests / "fixtures" / "moved.json": "{}\n",
        tests / "x" / "y.json": "{}\n",
        tests / "h" / "main.rs": "mod t5;\n",
        tests / "h" / "t5.rs": 'const U: &str = "sub/../../x/y.json";\n#[test]\nfn t() {}\n',
        tests / "h9" / "main.rs": '#[path = "../support/road.rs"]\nmod road;\n#[test]\nfn t() {}\n',
        tests / "support" / "road.rs": '#[path = "warm.rs"]\nmod warm;\n'
                                       'pub fn d() -> &\'static str { "docs/road/a.md" }\n',
        tests / "support" / "warm.rs": 'pub fn w() -> &\'static str { "docs/warm/w.md" }\n',
        tests / "lonely" / "x.rs": 'pub fn d() -> &\'static str { "docs/lonely/b.md" }\n',
        tests / "mchain" / "mod.rs": "pub mod deep;\n",
        tests / "mchain" / "deep.rs": 'pub fn d() -> &\'static str { "docs/chain/c.md" }\n',
        root / "docs" / "dd.md": "d\n",
        root / "docs" / "road" / "a.md": "a\n",
        root / "docs" / "lonely" / "b.md": "b\n",
        root / "docs" / "chain" / "c.md": "c\n",
        root / "docs" / "warm" / "w.md": "w\n",
        root / "docs" / "other.md": "o\n",
    }
    for path, text in texts.items():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
    (tests / "lnk").symlink_to(tests / "x", target_is_directory=True)
    return root


class DotDotReaders(unittest.TestCase):
    """tb-s1c-b (#1177): rule 8 normalises `..` in the reader's spelling.

    E1/E2 a reader spells the path with an interior `..` (`sub/../data.json`):
    lexically the same file, before unplanned. E3 a literal that climbs above
    its own start (`sub/../../x/y.json` from `tests/h/`) lands per rule-8 base
    (crate, its tests/, the reader file's directory). E4 a climb above every
    base and E5 a `..` behind a symlink resolve nowhere: the reader reads every
    path (fail-closed, all()). E6 lands exactly on the repository root from the
    crate base (`sub/../../..` from `tests/`) while the deeper bases give `crates`:
    the root is every path, not only `crates` (tb-s1c-d, re-check m-1).
    Red before the fix: E1-E5."""

    def test_dotdot_readers_are_planned(self):
        repo = dotdot_repo(self)
        cells = "crates/meclaw-cells/tests/"
        plan = lambda path: set(gp.test_terms([path], str(repo)))
        for name, path, want in (
                ("E1", cells + "fixtures/data.json", "binary_id(=meclaw-cells::gh_dd)"),
                ("E2", "docs/dd.md", "binary_id(=meclaw-cells::gh_dd)"),
                ("E3", cells + "x/y.json", "(binary_id(=meclaw-cells::h) & test(/^t5::/))"),
                ("E4", "docs/other.md", "binary_id(=meclaw-cells::gh_top)"),
                ("E5", "docs/other.md", "binary_id(=meclaw-cells::gh_lnk)"),
                ("E6", "docs/other.md", "binary_id(=meclaw-cells::gh_root)")):
            with self.subTest(name):
                self.assertIn(want, plan(path))
        # resolvable spellings stay narrow: `gh_dd` reads no other doc
        self.assertNotIn("binary_id(=meclaw-cells::gh_dd)", plan("docs/other.md"))
        self.assertNotIn("binary_id(=meclaw-cells::gh_ok)", plan("docs/other.md"))

    def test_the_guard_pairs_the_dotdot_reads_and_finds_them_planned(self):
        repo = dotdot_repo(self)
        files = [str(p.relative_to(repo)) for p in repo.rglob("*") if p.is_file()]
        pairs = reader_pairs(repo, files)
        cells = "crates/meclaw-cells/tests/"
        for pair in ((cells + "gh_dd.rs", "docs/dd.md"),
                     (cells + "gh_dd.rs", cells + "fixtures/data.json"),
                     (cells + "h/t5.rs", cells + "x/y.json"),
                     (cells + "gh_top.rs", "docs/other.md"),
                     (cells + "gh_lnk.rs", "docs/other.md"),
                     (cells + "gh_root.rs", "docs/other.md")):
            with self.subTest(pair):
                self.assertIn(pair, pairs)
        self.assertEqual([], unplanned_readers(repo, pairs))


def reference_runtime_tables(repo):
    """The rule-8 runtime index as built before tb-s1c-d: one `_bare_root`
    per literal with the text before and after it sliced off -- the oracle of
    `RuntimeIndexParity`."""
    root = str(repo)

    def bare_root(text, m):
        before = text[:m.start()].rstrip()[-1:]
        after = text[m.end():].lstrip()[:1]
        if before == "(" and after == ")":
            return True
        if before not in ("[", ",") or after not in ("]", ","):
            return False
        lo, hi = text.rfind("[", 0, m.start()), text.find("]", m.end())
        if lo < 0 or hi < 0 or hi - lo > 400 or any(c in text[lo:hi] for c in "{};"):
            return False
        words = re.findall(r'"([^"]*)"', text[lo:hi])
        return bool(words) and all(os.path.isdir(os.path.join(root, w)) for w in words)

    lits = collections.defaultdict(set)
    spelled = collections.defaultdict(set)
    anywhere = set()
    for crate, unit, text in gp._reader_sources(root):
        for m in gp._STRING_LITERAL_RE.finditer(text):
            lit = gp._dir_literal(m.group(1))
            if not lit or gp._PROBE_RE.match(text, m.end()):
                continue
            if ".." in lit.split("/"):
                keys = gp._resolve_dotdot(lit, crate, unit, root)
                if keys is None:
                    anywhere.add((crate, unit))
                for key in keys or ():
                    lits[key].add((crate, unit))
                    spelled[key].add((crate, unit))
                continue
            if "/" not in lit and not bare_root(text, m):
                continue
            lits[lit].add((crate, unit))
    return lits, spelled, anywhere


class RuntimeIndexParity(unittest.TestCase):
    """tb-s1c-d (#1177): the rule-8 runtime index is built in every plan --
    twice per gate, in the runner and in the resolver subprocess -- and its
    bare-word check sliced the rest of the text per literal (205 545 literals,
    0.8 s a build, +32 % planning time on a crate diff). The fast build must
    give the very same tables as the old one, on the real tree and on the
    edge cases of the context check."""

    def assertSameTables(self, repo):
        gp._RUNTIME_INDEX.pop(str(repo), None)
        got = gp._runtime_tables(str(repo))
        want = reference_runtime_tables(repo)
        for name, a, b in zip(("index", "spelled", "anywhere"), got, want):
            self.assertEqual(dict(b) if name != "anywhere" else b,
                             dict(a) if name != "anywhere" else a, name)

    def test_the_real_tree_builds_the_same_tables(self):
        self.assertSameTables(REPO)

    def test_the_context_edge_cases_build_the_same_tables(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        repo = pathlib.Path(tmp.name)
        for d in ("docs", "plans", "templates", "crates/c/tests"):
            (repo / d).mkdir(parents=True)
        nbsp, em = "\u00a0", "\u2003"
        texts = [
            'f("docs")', 'f( "docs" )', 'f(\n\t"docs"\n)', 'f(%s"docs"%s)' % (nbsp, em),
            '"docs")', 'f("docs"', '"docs"', '("docs"', '"docs" )',
            '["docs", "plans"]', '[ "docs" ,\n "templates" ]', '["docs", "nodir"]',
            '[{"docs", "plans"}]', '["docs"; "plans"]', '["docs","plans"', '"docs", "plans"]',
            '[' + ' ' * 401 + '"docs"]', '["docs"' + ' ' * 401 + ']',
            'g("a""docs")', 'g("x" "docs")', 'g(""docs")', 'x"docs")',
            'root.join("docs").is_dir()', 'root.join("docs") . exists ( )',
            'f("./docs/")', 'f(".")', 'f("..")', 'f("../docs")', 'f("docs/x.md")',
            'f("sub/../docs/x.md")', 'f("../../../../x")', '"plans"' + nbsp,
        ]
        sources = [("c", "u%d" % i, t) for i, t in enumerate(texts)]
        sources.append(("c", "all", "\n".join(texts)))
        with mock.patch.object(gp, "_reader_sources", lambda repo: sources):
            self.assertSameTables(repo)
            docs = {u for c, u in gp._runtime_tables(str(repo))[0]["docs"]}
        # call arguments in any whitespace, `./`/`../` spellings, lists of directories
        self.assertEqual({"u0", "u1", "u2", "u3", "u9", "u10", "u24", "u27", "all"}, docs)


class RuntimeIndexOncePerGate(unittest.TestCase):
    """tb-s10 w6 (#1173, A1 11 L-8): a gate built the rule-8 runtime index
    twice -- in the resolver subprocess (`plan.default_resolver`) and again in
    the runner (graph, impact). It is built once and handed over through a
    file keyed on the code, the tree, HEAD and the source texts; a key that
    does not match builds anew (fail-closed)."""

    ENV = "GATE_PLAN_RULE8_INDEX"

    def counting_builds(self):
        """Count index builds of this process: a build asks `_tree_links`
        once, from `_runtime_tables` itself."""
        calls = []
        real = gp._tree_links

        def counted(root):
            if sys._getframe(1).f_code.co_name == "_runtime_tables":
                calls.append(root)
            return real(root)
        patcher = mock.patch.object(gp, "_tree_links", counted)
        patcher.start()
        self.addCleanup(patcher.stop)
        return calls

    def test_the_resolver_subprocess_hands_its_index_to_the_runner(self):
        from pipeline import plan as pipeline_plan  # noqa: PLC0415
        env = {k: v for k, v in os.environ.items() if k != self.ENV}
        with mock.patch.dict(os.environ, env, clear=True):
            pipeline_plan.default_resolver(str(REPO), "strand", ["docs/cells.en.md"])
            path = os.environ.get(self.ENV)
            gp._RUNTIME_INDEX.pop(str(REPO), None)
            builds = self.counting_builds()
            got = gp._runtime_tables(str(REPO))
            # the subprocess built it (and wrote the file), the runner did not
            self.assertTrue(path and os.path.exists(path), path)
            self.assertEqual([], builds)
        gp._RUNTIME_INDEX.pop(str(REPO), None)
        with mock.patch.dict(os.environ, env, clear=True):
            want = gp._runtime_tables(str(REPO))
        self.assertEqual(1, len(builds))
        for a, b in zip(got, want):
            self.assertEqual(dict(b) if isinstance(b, dict) else b,
                             dict(a) if isinstance(a, dict) else a)

    def test_a_key_that_does_not_match_builds_anew(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        repo = pathlib.Path(tmp.name)
        (repo / "docs").mkdir()
        (repo / "plans").mkdir()
        path = str(repo / "rule8.index")
        sources = [("c", "u", 'v(["docs", "plans"])')]
        builds = self.counting_builds()
        root = str(repo)
        both = {"docs", "plans"}
        with mock.patch.dict(os.environ, {self.ENV: path}), \
                mock.patch.object(gp, "_reader_sources", lambda r: list(sources)):
            gp._RUNTIME_INDEX.pop(root, None)
            self.assertEqual(both, set(gp._runtime_tables(root)[0]))
            self.assertEqual(1, len(builds))
            gp._RUNTIME_INDEX.pop(root, None)   # another process, same tree: loads
            self.assertEqual(both, set(gp._runtime_tables(root)[0]))
            self.assertEqual(1, len(builds))
            sources[0] = ("c", "u", 'v(["docs", "plans"], "x/y")')   # the text moved
            gp._RUNTIME_INDEX.pop(root, None)
            self.assertEqual(both | {"x/y"}, set(gp._runtime_tables(root)[0]))
            self.assertEqual(2, len(builds))
            (repo / "plans").rmdir()   # a directory the list entries asked about is gone
            gp._RUNTIME_INDEX.pop(root, None)
            self.assertEqual({"x/y"}, set(gp._runtime_tables(root)[0]))
            self.assertEqual(3, len(builds))
            with open(path, "wb") as fh:   # a torn file
                fh.write(b"\x80garbage")
            gp._RUNTIME_INDEX.pop(root, None)
            self.assertEqual({"x/y"}, set(gp._runtime_tables(root)[0]))
            self.assertEqual(4, len(builds))

    # Re-check Post-xhigh-8 (recheck-tb-s10-a, line 6 M): the key parts tree
    # (`realpath(root)`), HEAD (`_head_stamp`) and code (`_CODE_DIGEST`) had no
    # pin -- dropping them stayed green, and an index of tree A served tree B:
    # a unit fell silently out of B's plan. Each part is pinned on its own.

    def link_tree(self, link):
        """A tree whose `gh_f` reads `docs/lnk/x.md`; `docs/lnk -> far` is
        committed when `link`, else the tree has no link."""
        rd = 'const J: &str = "docs/lnk/x.md";\n#[test]\nfn t() {}\n'
        root = pathlib.Path(reader_repo(self, {"gh_f": rd}))
        (root / "docs" / "far").mkdir(parents=True)
        (root / "docs" / "far" / "x.md").write_text("x\n", encoding="utf-8")
        if link:
            (root / "docs" / "lnk").symlink_to("far")
        SymlinkedBases.git(root, "init", "-q")
        SymlinkedBases.git(root, "add", "-A")
        commit(SymlinkedBases.git, root)
        for memo in (gp._RUNTIME_INDEX, gp._TREE_LINKS):
            self.addCleanup(memo.pop, str(root), None)
        return root

    def shared_index(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        return mock.patch.dict(os.environ, {self.ENV: os.path.join(tmp.name, "rule8.index")})

    UNIT = "binary_id(=meclaw-cells::gh_f)"

    def plans(self, root):
        gp._RUNTIME_INDEX.pop(str(root), None)   # another process
        return self.UNIT in set(gp.test_terms(["docs/far/x.md"], str(root)))

    def test_an_index_of_another_tree_does_not_serve_this_one(self):
        a, b = self.link_tree(False), self.link_tree(True)
        # the same HEAD stamp in both: only the tree tells them apart
        same = mock.patch.object(gp, "_head_stamp", lambda root: ("same",))
        with self.shared_index(), same:
            self.assertFalse(self.plans(a))   # A wrote the file, without the link
            self.assertTrue(self.plans(b), "tree B planned from tree A's index")

    def test_an_index_of_another_head_does_not_serve_this_tree(self):
        root = self.link_tree(False)
        with self.shared_index():
            self.assertFalse(self.plans(root))
            (root / "docs" / "lnk").symlink_to("far")
            SymlinkedBases.git(root, "add", "-A")
            commit(SymlinkedBases.git, root)
            self.assertTrue(self.plans(root), "the new HEAD planned from the old HEAD's index")

    def test_an_index_of_other_code_builds_anew(self):
        root = self.link_tree(True)
        builds = self.counting_builds()
        with self.shared_index():
            with mock.patch.object(gp, "_CODE_DIGEST", ["0" * 64]):
                self.assertTrue(self.plans(root))
            self.assertEqual(1, len(builds))
            self.assertTrue(self.plans(root))
            self.assertEqual(2, len(builds), "this code loaded an index of other code")
            self.assertTrue(self.plans(root))   # control: the same code loads
            self.assertEqual(2, len(builds))


class FoldedUnitParity(unittest.TestCase):
    """tb-s1c-d (#1177): `_folded_unit` reads one set of folded modules per
    main.rs (`_folded_mods`) instead of one lexer pass per module. The answer
    must be that of the lexer per module (the S4 `_folded_unit` of 426a343f5,
    copied below as the oracle). tb-s10 A1 (re-check tb-s4-st-h1 m-h1-1, L-9):
    the pins asked the regex `_declares_mod` that no planner called any more;
    they ask the production path now, and `_declares_mod` is gone."""

    @staticmethod
    def per_module(main_text, mod):
        """426a343f5 `_folded_unit`, one lexer pass for `mod` alone."""
        code = gp._rust_code(main_text)
        if code is None:
            return False
        hits, depth = [], 0
        for i, t in enumerate(code):
            if t in gp._BRACKETS:
                depth += 1
            elif t in (")", "]", "}"):
                depth -= 1
                if depth < 0:
                    return False
            elif depth == 0 and t == "mod" and code[i + 1:i + 3] == [mod, ";"]:
                hits.append(i)
        if depth or not hits:
            return False
        try:
            return all(gp._plain_mod_item(code, i) for i in hits)
        except gp.crate_root.Doubt:
            return False

    def assert_parity(self, text, mods, where=""):
        for mod in mods:
            want = "h/" + mod if self.per_module(text, mod) else "h/main"
            self.assertEqual(want, gp._folded_unit(text, "h", mod), "%s %r" % (where, mod))

    def test_declares_mod_is_gone(self):
        # L-9: no production caller since c39f1d5e0 -- dead code with live pins
        self.assertFalse(hasattr(gp, "_declares_mod"))
        self.assertFalse(hasattr(gp, "_DECLARED_MODS"))

    def test_every_module_of_the_real_tree_folds_as_the_lexer_per_module_says(self):
        asked = 0
        for main in sorted(REPO.glob("crates/*/tests/*/main.rs")):
            text = main.read_text(encoding="utf-8", errors="replace")
            mods = {p.stem for p in main.parent.glob("*.rs")}
            mods |= set(re.findall(r"mod\s+([^\s;{]+)", text))
            asked += len(mods)
            self.assert_parity(text, sorted(mods), main)
        self.assertGreater(asked, 1000)

    def test_the_declaration_edge_cases_fold_as_the_lexer_per_module_says(self):
        text = ("mod a;\npub(crate) mod b;\n  pub(in crate::x) mod c ;\nmod d\n;\nmod\ne;\n"
                "mod f_g;\n// mod h;\n#[cfg(test)] mod i;\nmod j {}\n#[allow(x)] mod k;\n"
                "pubmod n;\npub  mod o;\nmod r#p;\n\tmod q;\nmod a;\n#[path = \"x.rs\"] mod s;\n"
                "mod t;\n#[cfg(x)] mod t;\n")   # t: one declaration not plain -> no fold
        mods = ("a", "b", "c", "d", "e", "f", "f_g", "g", "h", "i", "j", "k", "n", "o",
                "p", "r#p", "q", "s", "t", "z")
        self.assert_parity(text, mods)
        self.assertEqual({"a", "b", "c", "d", "e", "f_g", "k", "o", "r#p", "q"},
                         set(gp._folded_mods(text)))

    def test_a_bracket_of_the_wrong_kind_before_a_mod_is_doubt_no_fold(self):
        # L-h1-2 (F2): balanced depth, mismatched kinds -- `_opening` doubts at
        # `pub(] [)`; the Doubt branch must not fold (not compilable Rust)
        text = "mod a;\npub(] [) mod b;\n"
        self.assert_parity(text, ("a", "b"))
        self.assertEqual({"a"}, set(gp._folded_mods(text)))

    def test_a_bracket_left_open_at_the_end_folds_nothing(self):
        # L-h1-2 (F3): depth != 0 at the end of main.rs -- no module folds
        text = "mod a;\nfn f() {\n"
        self.assert_parity(text, ("a",))
        self.assertEqual(frozenset(), gp._folded_mods(text))
        self.assertEqual(frozenset(), gp._folded_mods("mod a;\n}\n"))

    def test_the_memo_of_folded_modules_stays_bounded(self):
        # L-7 (re-check tb-s1c): a process-wide memo per main.rs text without a
        # bound grew with every distinct text a long-lived process saw
        for n in range(gp._FOLDED_MODS_MAX + 50):
            gp._folded_mods("mod m%d;\n" % n)
            self.assertLessEqual(len(gp._FOLDED_MODS), gp._FOLDED_MODS_MAX)
        self.assertEqual({"m7"}, set(gp._folded_mods("mod m7;\n")))


class HelperReaders(unittest.TestCase):
    """tb-s1c-c (#1177, F4): a helper in a directory without main.rs reads for
    the files that compile it in. H1 a `#[path]`-included helper for its
    harness, H2 a helper nobody includes for its crate (`rdeps`, fail-closed),
    H3 a helper behind a `mod` chain (`mod mchain;` -> `pub mod deep;`) for the
    flat test at the chain's head, H4 a helper only another helper `#[path]`s
    in (`support/warm_python.rs`) for that helper's includers. Red before the
    fix: H1-H4 (rule 8 never read their text)."""

    def test_helpers_without_main_read_for_their_includers(self):
        repo = dotdot_repo(self)
        plan = lambda path: set(gp.test_terms([path], str(repo)))
        for name, path, want in (
                ("H1", "docs/road/a.md", "binary_id(=meclaw-cells::h9)"),
                ("H2", "docs/lonely/b.md", "rdeps(meclaw-cells)"),
                ("H3", "docs/chain/c.md", "binary_id(=meclaw-cells::gh_m)"),
                ("H4", "docs/warm/w.md", "binary_id(=meclaw-cells::h9)")):
            with self.subTest(name):
                self.assertIn(want, plan(path))
        # a resolvable helper stays narrow: its read runs its includer, not the crate
        self.assertNotIn("rdeps(meclaw-cells)", plan("docs/road/a.md"))

    def test_the_includer_index_reads_each_test_file_once(self):
        """Re-check tb-s10 A1 13 d-4: `_includers` built a dead `texts` map and
        read every non-helper file a second time (`_read_text`) after
        `_test_sources` had read it; a harness `main.rs` was read three times
        (unit text, fold check of `_test_sources`, fold check of
        `_test_files`). From a cold cache each `.rs` is opened exactly once."""
        repo = str(dotdot_repo(self))
        real_open, reads = open, collections.Counter()

        def counting_open(path, *args, **kwargs):
            p = os.fspath(path)
            if p.endswith(".rs") and p.startswith(repo):
                reads[os.path.relpath(p, repo)] += 1
            return real_open(path, *args, **kwargs)

        with mock.patch("builtins.open", counting_open):
            gp.includers(repo)
        files = gp._test_files(repo)[0]
        self.assertEqual(set(files), set(reads))
        self.assertEqual({}, {f: n for f, n in reads.items() if n != 1})

    def test_the_guard_pairs_the_helper_files_and_finds_them_planned(self):
        repo = dotdot_repo(self)
        files = [str(p.relative_to(repo)) for p in repo.rglob("*") if p.is_file()]
        pairs = reader_pairs(repo, files)
        cells = "crates/meclaw-cells/tests/"
        for pair in ((cells + "support/road.rs", "docs/road/a.md"),
                     (cells + "lonely/x.rs", "docs/lonely/b.md"),
                     (cells + "mchain/deep.rs", "docs/chain/c.md"),
                     (cells + "support/warm.rs", "docs/warm/w.md")):
            with self.subTest(pair):
                self.assertIn(pair, pairs)
        self.assertEqual([], unplanned_readers(repo, pairs))


class SymlinkedBases(unittest.TestCase):
    """GH #1183 (re-check tb-s1c L-2, L-4): a symlinked directory of the tree.

    S1 `_includers` resolved `#[path]` lexically: `../lnkd/s2.rs` from `hs/`
    with `lnkd` -> `far/deep` is `far/deep/s2.rs` for the compiler,
    `tests/lnkd/s2.rs` for `normpath` (a file the walk never lists) -- the
    helper kept only its other includer `hother`, and a read of the helper
    ran `hother` alone (fail-open). S2 the `..` check of
    `_resolve_dotdot` looked for the link only at the root, the crate, its
    tests/ and the reader's directory: `jl/../j.json`, joined by the test to
    `tests/fixtures` where `jl` is a link, read like a plain `j.json`. Red
    before the fix: S1, S2.

    Re-check tb-s1e h2: L-a the `#[path]` target is itself a file link
    (`far/link.rs` -> `deep/s2.rs`), and `_real_key` never looked at the
    last segment -- `hf` was missing from the includers of `far/deep/s2.rs`.
    L-c a prefix that normalises to `../lnk` (`a/../../lnk/../j.json`):
    `_linked_anywhere` answered False, not fail-closed; L-c2 climbs twice
    (`a/../../../lnk` is `../../lnk`, re-check tb-s1e m-1: stripping one
    `../` stayed green). m-1 pins the link in
    the middle of a `#[path]` (E `../lnk2/deep/s2.rs`), the link behind a
    plain segment (G `a/jl/../j.json`) and before one (B `jl/b/../j.json`).
    m-2 the links come from the git index, not a walk of the tree (0.34 s
    in the main tree, 118k untracked entries under workshop/): U an
    untracked link counts in no git tree, and a tree outside git keeps the
    walk (N). Red before the fix: L-a, L-c, U."""

    @staticmethod
    def git(root, *args):
        # the repository variables of the caller would name another tree (pd-1155)
        env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
        subprocess.run(["git", "-C", str(root)] + list(args), check=True, env=env,
                       capture_output=True)

    def repo(self, git=True):
        rd = 'const J: &str = "%s";\n#[test]\nfn t() {}\n'
        root = pathlib.Path(reader_repo(self, {
            "gh_j": rd % "jl/../j.json", "gh_g": rd % "a/jl/../j.json",
            "gh_b": rd % "jl/b/../j.json", "gh_c": rd % "a/../../lnk/../j.json",
            "gh_d": rd % "a/../../../lnk/../j.json", "gh_u": rd % "ul/../j.json"}))
        tests = root / "crates" / "meclaw-cells" / "tests"
        inc = '#[path = "%s"]\nmod s2;\n#[test]\nfn t() {}\n'
        for path, text in {
                tests / "hs" / "main.rs": inc % "../lnkd/s2.rs",
                tests / "hm" / "main.rs": inc % "../lnk2/deep/s2.rs",
                tests / "hf" / "main.rs": inc % "../far/link.rs",
                tests / "hother" / "main.rs": inc % "../far/deep/s2.rs",
                tests / "far" / "deep" / "s2.rs":
                    'pub fn d() -> &\'static str { "docs/far/s.md" }\n',
                tests / "fixtures" / "x" / "j.json": "{}\n",
                root / "docs" / "far" / "s.md": "s\n",
                root / "docs" / "other.md": "o\n"}.items():
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text, encoding="utf-8")
        (tests / "fixtures" / "q" / "a").mkdir(parents=True)
        for link, target in (("lnkd", "far/deep"), ("lnk2", "far"), ("far/link.rs", "deep/s2.rs"),
                             ("fixtures/jl", "x/y"), ("fixtures/q/a/jl", "../../x"),
                             ("fixtures/lnk", "x")):
            (tests / link).symlink_to(target)
        if git:
            self.git(root, "init", "-q")
            self.git(root, "add", "-A")
            commit(self.git, root)   # the links of HEAD count (re-check tb-s1e L-f)
        (tests / "fixtures" / "ul").symlink_to("x")   # never added
        return root

    def test_symlinked_directories_plan_fail_closed(self):
        repo = self.repo()
        plan = lambda path: set(gp.test_terms([path], str(repo)))
        for name, path, want in (
                ("S1", "docs/far/s.md", "binary_id(=meclaw-cells::hs)"),
                ("S2", "docs/other.md", "binary_id(=meclaw-cells::gh_j)"),
                ("G", "docs/other.md", "binary_id(=meclaw-cells::gh_g)"),
                ("B", "docs/other.md", "binary_id(=meclaw-cells::gh_b)"),
                ("L-c", "docs/other.md", "binary_id(=meclaw-cells::gh_c)"),
                # re-check tb-s1e m-1: two climbs (`../../lnk`), every `../` goes
                ("L-c2", "docs/other.md", "binary_id(=meclaw-cells::gh_d)")):
            with self.subTest(name):
                self.assertIn(want, plan(path))
        # the other includer stays, and no link widens a plain read
        self.assertIn("binary_id(=meclaw-cells::hother)", plan("docs/far/s.md"))
        self.assertNotIn("binary_id(=meclaw-cells::hother)", plan("docs/other.md"))

    def test_every_includer_through_a_link_compiles_the_target(self):
        """`far/deep/s2.rs` is compiled into all four binaries; `hf` and `hm`
        reach it only through a link (the file itself, a middle directory)."""
        inc = gp.includers(str(self.repo())).get("crates/meclaw-cells/tests/far/deep/s2.rs", ())
        for name, unit in (("S1", "hs"), ("E", "hm"), ("L-a", "hf"), ("plain", "hother")):
            with self.subTest(name):
                self.assertIn("crates/meclaw-cells/tests/%s/main.rs" % unit, inc)

    def test_a_mod_declaration_through_a_link_keeps_its_includer(self):
        """Re-check tb-s10 A1 9 L-b: `mod sub;` in `hmod/main.rs` where
        `hmod/sub` -> `../shared`. The compiler opens `shared/mod.rs`; the
        `mod` branch of `_includers` only named `hmod/sub/mod.rs` (a file the
        walk never lists), so `shared/mod.rs` lost `hmod` (fail-open)."""
        repo = self.repo()
        tests = repo / "crates" / "meclaw-cells" / "tests"
        (tests / "hmod").mkdir()
        (tests / "hmod" / "main.rs").write_text("mod sub;\n#[test]\nfn t() {}\n",
                                                 encoding="utf-8")
        (tests / "shared").mkdir()
        (tests / "shared" / "mod.rs").write_text("pub fn s() {}\n", encoding="utf-8")
        (tests / "hmod" / "sub").symlink_to("../shared")
        self.git(repo, "add", "-A")
        inc = gp.includers(str(repo)).get("crates/meclaw-cells/tests/shared/mod.rs", ())
        self.assertIn("crates/meclaw-cells/tests/hmod/main.rs", inc)

    def test_the_links_are_those_of_the_index(self):
        plan = lambda repo: set(gp.test_terms(["docs/other.md"], str(repo)))
        with self.subTest("U"):
            # untracked: the commit has no such link, so its reader reads `j.json`
            self.assertNotIn("binary_id(=meclaw-cells::gh_u)", plan(self.repo()))
        with self.subTest("N"):
            # outside git the walk of the tree stays: both links count
            got = plan(self.repo(git=False))
            self.assertIn("binary_id(=meclaw-cells::gh_u)", got)
            self.assertIn("binary_id(=meclaw-cells::gh_j)", got)


def commit(git, root):
    git(root, "-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false",
        "commit", "-q", "--no-verify", "--allow-empty", "-m", "c")


class TreeLinksFollowHead(unittest.TestCase):
    """Re-check tb-s1e L-f: the links of the tree are those of HEAD (the plan
    is `rev..HEAD`), not of the git index -- a staged, uncommitted link does
    not count, one dropped with `git rm --cached` still does. L-e: the memo
    of `_tree_links` follows HEAD -- a link committed after the first call is
    seen by the second (a long-lived process planning twice)."""

    git = staticmethod(SymlinkedBases.git)

    def repo(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = pathlib.Path(tmp.name)
        for d in ("x", "y", "z"):
            (root / d).mkdir()
            (root / d / "f").write_text("f\n", encoding="utf-8")
        (root / "a").symlink_to("x")
        self.git(root, "init", "-q")
        self.git(root, "add", "-A")
        commit(self.git, root)
        self.addCleanup(gp._TREE_LINKS.pop, str(root), None)
        return root

    def test_a_staged_link_does_not_count(self):
        root = self.repo()
        (root / "b").symlink_to("y")
        self.git(root, "add", "b")
        self.assertEqual(gp._tree_links(str(root)), frozenset({"a"}))

    def test_a_link_dropped_from_the_index_still_counts(self):
        root = self.repo()
        self.git(root, "rm", "-q", "--cached", "a")
        self.assertEqual(gp._tree_links(str(root)), frozenset({"a"}))

    def test_a_link_committed_after_the_first_call_is_seen(self):
        root = self.repo()
        self.assertEqual(gp._tree_links(str(root)), frozenset({"a"}))
        (root / "b").symlink_to("y")
        self.git(root, "add", "b")
        commit(self.git, root)
        self.assertEqual(gp._tree_links(str(root)), frozenset({"a", "b"}))
        # a moved HEAD drops it again (detached, same branch file untouched)
        self.git(root, "checkout", "-q", "--detach", "HEAD~1")
        self.assertEqual(gp._tree_links(str(root)), frozenset({"a"}))


class LiteralThroughALink(unittest.TestCase):
    """Re-check tb-s1e h2 L-d: a rule-8 literal without `..` that passes a
    symlinked directory of the tree (`docs/lnk/x.md`, `docs/lnk -> far`)
    stayed lexical -- a diff at the link's target `docs/far/x.md` planned
    nothing. It reads under the target's path too, as a whole file and as a
    directory; a link only staged is none (L-f)."""

    def plan(self, path, staged=False):
        rd = 'const J: &str = "%s";\n#[test]\nfn t() {}\n'
        root = pathlib.Path(reader_repo(self, {
            "gh_f": rd % "docs/lnk/x.md", "gh_d": rd % "docs/lnk",
            "gh_s": rd % "docs/stg/x.md"}))
        (root / "docs" / "far").mkdir(parents=True)
        for name in ("x.md", "y.md"):
            (root / "docs" / "far" / name).write_text("x\n", encoding="utf-8")
        (root / "docs" / "lnk").symlink_to("far")
        SymlinkedBases.git(root, "init", "-q")
        SymlinkedBases.git(root, "add", "-A")
        commit(SymlinkedBases.git, root)
        (root / "docs" / "stg").symlink_to("far")
        SymlinkedBases.git(root, "add", "-A")
        return set(gp.test_terms([path], str(root)))

    def test_a_file_literal_through_a_link_reads_the_target(self):
        self.assertIn("binary_id(=meclaw-cells::gh_f)", self.plan("docs/far/x.md"))

    def test_a_directory_literal_through_a_link_reads_below_the_target(self):
        self.assertIn("binary_id(=meclaw-cells::gh_d)", self.plan("docs/far/y.md"))

    def test_a_staged_link_resolves_nothing(self):
        got = self.plan("docs/far/x.md")
        self.assertNotIn("binary_id(=meclaw-cells::gh_s)", got)
        # control: the other file is no read of the file literal
        self.assertNotIn("binary_id(=meclaw-cells::gh_f)", self.plan("docs/far/y.md"))


class BareRootList(unittest.TestCase):
    """Re-check tb-s1c m-1: a list of tree directories reads them
    (`["docs", "plans"]`), but `_bare_root` refuses a bracket that holds
    `{`, `}` or `;` -- `[x; "docs", "plans"]` is no list of paths. Nothing
    pinned that refusal; a pin red when it is dropped. Re-check tb-s1e m-2:
    the braces too (only `;` was pinned, dropping `{`/`}` stayed green)."""

    def plan(self, text):
        root = pathlib.Path(reader_repo(self, {"gh_r": text + "\n#[test]\nfn t() {}\n"}))
        for d in ("docs", "plans"):
            (root / d).mkdir()
            (root / d / "n.md").write_text("n\n", encoding="utf-8")
        return set(gp.test_terms(["plans/n.md"], str(root)))

    def test_a_bracket_with_a_semicolon_is_no_list_of_paths(self):
        unit = "binary_id(=meclaw-cells::gh_r)"
        self.assertNotIn(unit, self.plan('let a = [x; "docs", "plans"];'))
        # control: the same words as a list read plans/
        self.assertIn(unit, self.plan('let a = ["docs", "plans"];'))

    def test_a_bracket_with_a_brace_is_no_list_of_paths(self):
        unit = "binary_id(=meclaw-cells::gh_r)"
        for name, text in (("before", 'let a = [{ "docs" }, "plans"];'),
                           ("after", 'let a = ["docs", "plans", { 1 }];')):
            with self.subTest(name):
                self.assertNotIn(unit, self.plan(text))


class Cli(unittest.TestCase):
    script = str(SCRIPTS / "gate_plan.py")

    def run_cli(self, *args, stdin=None):
        return subprocess.run(
            [sys.executable, self.script, *args], input=stdin,
            capture_output=True, text=True, cwd=str(REPO), check=True)

    def test_tsv_and_json_cli(self):
        r = self.run_cli("--mode", "strand", "--files",
                         "crates/meclaw-colony/src/lib.rs", "--format", "tsv")
        rows = [line.split("\t") for line in r.stdout.rstrip("\n").splitlines()]
        self.assertTrue(rows)
        for row in rows:
            self.assertEqual(len(row), 5)
        self.assertIn("tests", [row[0] for row in rows])

        r = self.run_cli("--mode", "strand", "--files",
                         "crates/meclaw-colony/src/lib.rs", "--format", "json")
        doc = json.loads(r.stdout)
        self.assertEqual(doc["mode"], "strand")
        self.assertIn("rust_src", doc["classes"])
        self.assertIn("meclaw-colony", doc["crates"])
        names = [s["name"] for s in doc["stations"]]
        self.assertIn("tests", names)
        for s in doc["stations"]:
            self.assertTrue(("cmd" in s) != ("cmds" in s))

    def test_tsv_has_five_columns_on_every_row(self):
        """Five columns are the contract: a shell reader names five variables."""
        for mode in ("strand", "integration", "release", "ci"):
            r = self.run_cli("--mode", mode, "--files",
                             "crates/meclaw-colony/src/colony.rs",
                             "templates/memory-hive/config.json", "Cargo.lock",
                             "--format", "tsv")
            rows = r.stdout.rstrip("\n").splitlines()
            self.assertTrue(rows, mode)
            for row in rows:
                self.assertEqual(len(row.split("\t")), 5, (mode, row))

    def test_missing_files_from_is_a_usage_error(self):
        r = subprocess.run(
            [sys.executable, self.script, "--mode", "strand",
             "--files-from", str(REPO / "no-such-file.txt")],
            capture_output=True, text=True, cwd=str(REPO))
        self.assertEqual(r.returncode, 2)
        self.assertIn("--files-from", r.stderr)
        self.assertNotIn("Traceback", r.stderr)

    def test_files_from_stdin(self):
        r = self.run_cli("--mode", "strand", "--files-from", "-",
                         "--format", "json", stdin="docs/a.md\ndocs/b.md\n")
        doc = json.loads(r.stdout)
        self.assertEqual(doc["crates"], [])
        self.assertIn("docs", doc["classes"])

    def test_cwd_column_is_filled_for_the_builder_suite(self):
        r = self.run_cli("--mode", "release", "--files",
                         "docs/a.md", "--format", "tsv")
        rows = [line.split("\t") for line in r.stdout.rstrip("\n").splitlines()]
        by_row = {row[0]: row for row in rows}
        cwds = {row[0]: row[4] for row in rows}
        self.assertEqual(cwds["scenarios:builder"], "workshop/evals/builder-scenarios")
        self.assertEqual(cwds["scenarios:memory"], "")
        # ... and the argv on that row is relative to the cwd, not to the root.
        self.assertEqual(by_row["scenarios:builder"][3],
                         "python3 run_builder_scenarios.py")

    def test_print_scenario(self):
        r = self.run_cli("--print", "scenario")
        self.assertEqual(r.stdout.strip(), gp.scenario_for(None))

    def test_print_ignored(self):
        """`scripts/gate.sh` subtracts these before it calls a tree dirty."""
        r = self.run_cli("--print", "ignored")
        self.assertEqual(r.stdout.split(), list(gp.IGNORED))

    def test_print_persona_sources(self):
        """One semantics for the class and the list: every line classifies."""
        r = self.run_cli("--print", "persona-sources")
        lines = r.stdout.splitlines()
        self.assertTrue(lines)
        self.assertEqual(sorted(set(lines)), lines)
        self.assertIn("templates/memory-hive/inline-contract.md", lines)
        for line in lines:
            with self.subTest(path=line):
                self.assertNotIn("*", line)
                self.assertTrue((REPO / line).is_file())
                self.assertIn("persona_source", gp.classify([line]))

    def test_print_persona_sources_reads_the_named_tree(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = pathlib.Path(tmp.name)
        want = ["templates/memory-hive/x/config.json", "templates/talky/config.json"]
        for rel in want + ["templates/door/config.json"]:
            f = root / rel
            f.parent.mkdir(parents=True, exist_ok=True)
            f.write_text("{}", encoding="utf-8")
        r = self.run_cli("--print", "persona-sources", "--repo", str(root))
        self.assertEqual(want, r.stdout.splitlines())


class LiveBinariesAreScenarioTest(unittest.TestCase):
    """A `*_live.rs` test talks to a paid third party -- it never rides a diff.

    `slack_live` was named in the scenario class by hand, and the next live
    file (`gpt_live_live`, wave Live) would have been selected by any diff that
    touched its crate: a `cargo test` that opens a billed session because a
    comment moved. The rule is the suffix, so this reads the tree rather than a
    second list.
    """

    def live_binaries(self):
        return sorted(
            path.stem
            for path in (REPO / "crates").glob("*/tests/*_live.rs")
        )

    def test_there_is_at_least_one(self):
        """Without this the test below passes by measuring nothing."""
        self.assertTrue(self.live_binaries(), "no *_live.rs in crates/*/tests/")

    def test_every_live_binary_is_in_the_scenario_class(self):
        for name in self.live_binaries():
            with self.subTest(binary=name):
                self.assertIn("binary(/^%s$/)" % name, gp.SCENARIO)


IGNORED_ONLY = (
    '//! Measurement, run with `scripts/test-tier.sh filter ... --run-ignored all`.\n'
    'const READS: &str = "scripts/test-tier.sh";\n'
    '#[tokio::test(flavor = "multi_thread", worker_threads = 4)]\n'
    '#[ignore = "benchmark -- run explicitly"]\n'
    'async fn bench() {}\n')


class IgnoredOnlyBinariesAreNoTarget(unittest.TestCase):
    """GH #997: a binary whose every test is `#[ignore]` runs 0 tests.

    Rule 8 picked `gh850_bench_normal_path` for a diff to
    `scripts/test-tier.sh` (its doc comment names the path); its one test is a
    `#[ignore]` benchmark, so the `tests` station ran 0 tests -- nextest exit 4
    on master, `no tests matched` (exit 6) since K1. Such a binary is never the
    target of ANY plan rule. (Since #1160 a prose comment is no reference;
    the fixtures name the path in a literal.)
    """

    def test_an_ignored_only_reader_is_not_selected(self):
        repo = reader_repo(self, {"bench": IGNORED_ONLY})
        st = by_name(gp.plan(["scripts/test-tier.sh"], "strand", repo=repo))
        self.assertNotIn("tests", st)

    def test_a_reader_with_one_ignored_and_one_normal_test_stays(self):
        repo = reader_repo(self, {
            "bench": IGNORED_ONLY,
            "mixed": ('const READS: &str = "scripts/test-tier.sh";\n'
                      '#[test]\n#[ignore]\nfn slow() {}\n'
                      '#[test]\nfn fast() {}\n')})
        self.assertEqual(
            gp.test_filter(["scripts/test-tier.sh"], "ci", repo=repo),
            "binary_id(=meclaw-cells::mixed)")

    def test_rule_3_drops_a_changed_ignored_only_test_file_too(self):
        """Central, not rule 8 only: the binary runs 0 tests whoever picks it."""
        repo = reader_repo(self, {"bench": IGNORED_ONLY,
                                  "normal": "#[test]\nfn t() {}\n"})
        self.assertEqual(
            gp.test_filter(["crates/meclaw-cells/tests/bench.rs",
                            "crates/meclaw-cells/tests/normal.rs"], "ci", repo=repo),
            "binary_id(=meclaw-cells::normal)")

    def test_doubtful_shapes_stay_targets(self):
        """In doubt the binary stays: cfg_attr, macros, modules, no attribute."""
        doubtful = {
            "cfg_attr": ('const READS: &str = "scripts/test-tier.sh";\n'
                         '#[test]\n#[cfg_attr(not(feature = "x"), ignore)]\nfn t() {}\n'),
            "macro": ('const READS: &str = "scripts/test-tier.sh";\n#[test]\n#[ignore]\nfn a() {}\n'
                      'macro_rules! gen { () => { #[test] fn b() {} } }\ngen!();\n'),
            "module": ('const READS: &str = "scripts/test-tier.sh";\nmod more;\n'
                       '#[test]\n#[ignore]\nfn a() {}\n'),
            "commented_ignore": ('const READS: &str = "scripts/test-tier.sh";\n'
                                 '#[test]\n// #[ignore]\nfn a() {}\n'),
            "harness_false": 'fn main() { let _ = "scripts/test-tier.sh"; }\n',
        }
        repo = reader_repo(self, doubtful)
        self.assertEqual(
            gp.test_filter(["scripts/test-tier.sh"], "ci", repo=repo),
            " + ".join("binary_id(=meclaw-cells::%s)" % s for s in sorted(doubtful)))

    def test_an_expression_macro_in_a_test_body_is_no_doubt(self):
        """`tokio::select! { .. }` is an expression, it never makes a test
        item: an ignored-only binary that uses it still runs 0 tests. Taken as
        doubt, `gh1004_viewport_event_measure` (one `#[ignore]` test, a
        `select!` in its body, `scripts/strand.sh` in its doc) stayed the one
        target of the `tests` station for every `strand.sh` diff, and nextest
        ended RED on "no tests to run" (GH #1053 strand gate, 2026-10-07)."""
        repo = reader_repo(self, {
            "measure": ('const READS: &str = "scripts/test-tier.sh";\n'
                        '#[tokio::test]\n#[ignore = "wall clock"]\nasync fn m() {\n'
                        '    tokio::select! {\n        _ = a => {}\n    }\n'
                        '    futures::join! { a, b };\n}\n'),
            "normal": "const READS: &str = \"scripts/test-tier.sh\";\n#[test]\nfn t() {}\n"})
        self.assertEqual(
            gp.test_filter(["scripts/test-tier.sh"], "ci", repo=repo),
            "binary_id(=meclaw-cells::normal)")

    def test_the_real_tree_does_not_select_the_viewport_measure(self):
        expr = gp.test_filter(["scripts/strand.sh"], "ci") or ""
        self.assertNotIn("gh1004_viewport_event_measure", expr)

    def test_the_counter_parser(self):
        self.assertEqual(gp._test_counts(IGNORED_ONLY), (1, 0))
        self.assertEqual(gp._test_counts(
            '#[test]\n#[ignore]\nfn a() {}\n#[tokio::test]\nasync fn b() {}\n'
            'const S: &str = "#[test]";\n/* #[test] fn c() {} */\n'), (2, 1))

    def test_a_string_continued_over_a_line_keeps_the_count(self):
        """GH #1188: `"a \\<newline> b"` -- an escape must take the newline
        of a line continuation too. The regex lexer's `\\.` did not: the
        string failed to match, its closing quote opened a phantom one and
        the count flipped from there (gh703 browser.rs:90, gh643, gh766,
        710_sheet, ...; it read (2, 2)). The token lexer
        (`crate_root._STR`, welle-tb S4) reads one literal; with `\\.`
        there it doubts the file and the count is (0, 0)."""
        self.assertEqual(gp._test_counts(
            'const S: &str = "a \\\n    b";\n#[test]\n#[ignore]\nfn a() {}\n'
            'const T: &str = "x";\n#[test]\nfn b() {}\n#[test]\nfn c() {}\n'), (3, 2))

    def test_the_real_tree_no_longer_selects_the_bench(self):
        """The measured case: `scripts/test-tier.sh` must not pick gh850."""
        expr = gp.test_filter(["scripts/test-tier.sh"], "ci") or ""
        self.assertNotIn("gh850_bench_normal_path", expr)


def module_repo(case, binaries, modules):
    """`reader_repo` plus out-of-line module files under `tests/` (any depth)."""
    repo = reader_repo(case, binaries)
    tests = pathlib.Path(repo) / "crates" / "meclaw-cells" / "tests"
    for rel, text in modules.items():
        (tests / rel).parent.mkdir(parents=True, exist_ok=True)
        (tests / rel).write_text(text, encoding="utf-8")
    return repo


class OutOfLineModulesAreReadOneLevelDeep(unittest.TestCase):
    """K1 fix round 2: `mod x;` is resolved, not taken as doubt on sight.

    The browser locks gh867/gh961 are `#[ignore]`d and pull in
    `#[path = "support/display_colony.rs"] mod display_colony;`. Taken as
    doubt, they stayed targets of the `tests` station and ran 0 tests there --
    RED on every diff that touches only their driver `.mjs` or the lock
    itself; the station `browser:display` runs them with `--run-ignored all`.
    The module is read one level deep (`tests/<path>`, `tests/x.rs`,
    `tests/x/mod.rs`); doubt only when it has test attributes or hidden shapes
    itself, or cannot be found.
    """

    LOCK = ('const READS: &str = "scripts/test-tier.sh";\n'
            '#[path = "support/helper.rs"]\nmod helper;\n'
            '#[tokio::test]\n#[ignore = "browser lock"]\nasync fn lock() {}\n')

    def selected(self, binaries, modules):
        repo = module_repo(self, binaries, modules)
        return gp.test_filter(["scripts/test-tier.sh"], "ci", repo=repo)

    def test_a_path_module_without_tests_drops_the_lock(self):
        self.assertIsNone(self.selected(
            {"lock": self.LOCK}, {"support/helper.rs": "pub fn boot() {}\n"}))

    def test_plain_module_files_resolve_too(self):
        body = 'const READS: &str = "scripts/test-tier.sh";\nmod %s;\n#[test]\n#[ignore]\nfn a() {}\n'
        self.assertIsNone(self.selected(
            {"flat": body % "flat_help", "nested": body % "nested_help"},
            {"flat_help.rs": "pub fn x() {}\n",
             "nested_help/mod.rs": "pub fn y() {}\n"}))

    def test_a_module_with_its_own_test_keeps_the_binary(self):
        self.assertEqual(
            self.selected({"lock": self.LOCK},
                          {"support/helper.rs": "#[test]\nfn inner() {}\n"}),
            "binary_id(=meclaw-cells::lock)")

    def test_a_module_with_a_hidden_shape_keeps_the_binary(self):
        self.assertEqual(
            self.selected({"lock": self.LOCK},
                          {"support/helper.rs": "macro_rules! m { () => {} }\n"}),
            "binary_id(=meclaw-cells::lock)")

    def test_a_missing_module_keeps_the_binary(self):
        self.assertEqual(self.selected({"lock": self.LOCK}, {}),
                         "binary_id(=meclaw-cells::lock)")

    def test_a_module_declared_inside_an_inline_module_keeps_the_binary(self):
        """K1 fix round 3: `mod a { mod b; }` loads `tests/a/b.rs`, not
        `tests/b.rs`. Resolved from the root, the testless `b.rs` dropped a
        binary whose `a/b.rs` runs a test -- a nested `mod` is doubt."""
        body = ('const READS: &str = "scripts/test-tier.sh";\n#[test] #[ignore] fn slow(){}\n'
                'mod a { mod b; }\n')
        self.assertEqual(
            self.selected({"nest": body},
                          {"b.rs": "pub fn x() {}\n",
                           "a/b.rs": "#[test]\nfn runs() {}\n"}),
            "binary_id(=meclaw-cells::nest)")

    def test_braces_in_comments_and_literals_do_not_nest(self):
        """Depth is read on code without comments and literals."""
        body = ('const READS: &str = "scripts/test-tier.sh"; // { {\n/* { */ const S: &str = "{";\n'
                "const C: char = '{';\n#[test]\n#[ignore]\nfn a() {}\n"
                'mod flat_help;\n')
        self.assertIsNone(self.selected(
            {"flat": body}, {"flat_help.rs": "pub fn x() {}\n"}))

    def test_a_cfg_attr_path_keeps_the_binary(self):
        """`#[cfg_attr(unix, path = "y.rs")] mod x;` may load `y.rs` -- doubt."""
        body = ('const READS: &str = "scripts/test-tier.sh";\n#[test]\n#[ignore]\nfn a() {}\n'
                '#[cfg_attr(unix, path = "y.rs")]\nmod x;\n')
        self.assertEqual(
            self.selected({"alt": body},
                          {"x.rs": "pub fn x() {}\n",
                           "y.rs": "#[test]\nfn runs() {}\n"}),
            "binary_id(=meclaw-cells::alt)")

    def test_a_nested_block_comment_keeps_the_binary(self):
        """K1 fix round 3 close: `/* x /* y */ } */` nests in Rust. The
        non-greedy blanking leaves ` } */`, the stray `}` closed `mod a` early
        and `mod z;` resolved to the testless `tests/z.rs` -- doubt."""
        body = ('const READS: &str = "scripts/test-tier.sh";\n#[test]\n#[ignore]\nfn a() {}\n'
                'mod a {\n/* x /* y */ } */\nmod z;\n}\n')
        self.assertEqual(
            self.selected({"nested": body},
                          {"z.rs": "pub fn x() {}\n",
                           "a/z.rs": "#[test]\nfn runs() {}\n"}),
            "binary_id(=meclaw-cells::nested)")

    def test_only_a_driver_mjs_plans_the_shot_walk_alone_in_tests(self):
        """The measured case: gh961's driver alone ran 0 tests in `tests`. Fix
        round stapel I1 (OR-V2.v2-r3-planbrowser.9): 708 reads every
        top-level `.mjs` (`every_driver_keeps_the_answer_of_an_evidence_shot`),
        so `tests` carries exactly that binary -- never an empty selection."""
        for mjs in ("display-motion-browser.mjs", "display-csp-browser.mjs"):
            st = by_name(gp.plan(["workshop/tools/" + mjs], "strand"))
            self.assertEqual("binary_id(=meclaw-cells::708_the_client_draws_the_tap_before_the_pass)",
                             st["tests"].scope.split(" - ", 1)[0], mjs)
            self.assertIn("browser:display", st, mjs)

    def test_only_the_gh961_lock_has_browser_display_but_no_tests_station(self):
        st = by_name(gp.plan(
            ["crates/meclaw-cells/tests/gh961_motion_by_target_state_browser.rs"],
            "strand"))
        # tb-s1c: the lock's own binary still runs no test (rule 9) -- but the
        # text locks that walk `crates` read every `.rs` (gh523, gh230), so
        # `tests` now carries exactly them, never the gh961 binary
        self.assertIn("browser:display", st)
        self.assertIn("tests", st)
        self.assertNotIn("gh961_motion_by_target_state_browser)", st["tests"].scope)
        self.assertIn("gh523_the_per_turn_lane_is_not_a_batch", st["tests"].scope)


class UnknownTestShapesAreDoubt(unittest.TestCase):
    """K1 fix round 2: a test form the counter does not know is no proof of 0.

    Any attribute whose last path segment contains `test` (`#[wasm_bindgen_test]`,
    `#[serial_test::test]`), and `quickcheck`/`proptest`, counts as a runnable
    test; a macro invocation `name! {` at any indentation is doubt.
    """

    IGNORED = 'const READS: &str = "scripts/test-tier.sh";\n#[test]\n#[ignore]\nfn slow() {}\n'

    def test_unknown_attributes_count_as_runnable_tests(self):
        self.assertEqual(gp._test_counts("#[quickcheck]\nfn p(x: u8) -> bool { true }\n"),
                         (1, 1))
        self.assertEqual(gp._test_counts("#[wasm_bindgen_test]\nfn w() {}\n"), (1, 1))
        self.assertEqual(gp._test_counts("#[proptest]\nfn q(x: u8) {}\n"), (1, 1))
        self.assertEqual(gp._test_counts("#[cfg(test)]\nmod m {}\n#[path = \"x_test.rs\"]\n"),
                         (0, 0))

    def test_such_binaries_stay_targets(self):
        shapes = {
            "quickcheck": self.IGNORED + "#[quickcheck]\nfn p(x: u8) -> bool { true }\n",
            "wasm": self.IGNORED + "#[wasm_bindgen_test]\nfn w() {}\n",
            "indented_macro": self.IGNORED + "mod props {\n    quickcheck! {\n"
                              "        fn p(x: u8) -> bool { true }\n    }\n}\n",
        }
        repo = reader_repo(self, shapes)
        self.assertEqual(
            gp.test_filter(["scripts/test-tier.sh"], "ci", repo=repo),
            " + ".join("binary_id(=meclaw-cells::%s)" % s for s in sorted(shapes)))


# --- rule 9 reads Rust through `crate_root.tokens` (welle-tb S4, GH #1162) ---
#
# `gate_plan._rust_code` was a regex lexer: a string with a `\\`-newline
# continuation (`"a \\<newline>b"`) did not match its string pattern, the
# closing quote opened a "string" of its own, and from there on code read as
# literal and literal as code. `meclaw-cells/tests/misc_1/cogny_template.rs`
# (4 tests) counted 0. Every reader of that lexer could then lose a `#[test]`,
# a `mod x;` or a macro: rule 9 (`_runs_no_test`) dropped a binary that runs
# tests, and `impact.folded_form` took a folded term for a module without one.
# The cases below put the phase loss in front of exactly the shape each reader
# needs. A file the token lexer doubts (an open comment or literal, a
# character it does not know) is kept: never "no test".

# A string the regex lexer cannot close: from its last quote on, the phase is
# inverted up to the next `"` (that stretch reads as one literal) -- `Y`'s
# literal is where it ends. Everything after it must hold no further quote.
LEXER_DRIFT = 'const DRIFT: &str = "a \\\nb";\n'
LEXER_DRIFT_END = 'const Y: &str = "y";\n'
LEXER_IGNORED = "#[test]\n#[ignore]\nfn slow() {}\n"
# A comment left open: the token lexer cannot tell where code resumes. (A
# non-ASCII identifier was the doubt here until tb-s10 A1 point 20 -- valid
# Rust the lexer reads now.)
LEXER_DOUBT = "/* a comment left open\n"


def lexer_swallowed(code):
    """`code` where the regex lexer reads it as the inside of a literal."""
    return LEXER_DRIFT + code + LEXER_DRIFT_END


def lexer_tree(case, binaries, modules=None, crate="meclaw-cells"):
    """A throw-away repo with `crates/<crate>/tests/<stem>.rs` and modules."""
    tmp = tempfile.TemporaryDirectory()
    case.addCleanup(tmp.cleanup)
    root = pathlib.Path(tmp.name)
    tests = root / "crates" / crate / "tests"
    tests.mkdir(parents=True)
    (root / "crates" / crate / "Cargo.toml").write_text(
        '[package]\nname = "%s"\n' % crate, encoding="utf-8")
    for stem, text in binaries.items():
        (tests / (stem + ".rs")).write_text(text, encoding="utf-8")
    for rel, text in (modules or {}).items():
        (tests / rel).parent.mkdir(parents=True, exist_ok=True)
        (tests / rel).write_text(text, encoding="utf-8")
    return str(root), str(tests)


class TokenLexerCountsTheMeasuredFile(unittest.TestCase):

    def test_cogny_template_counts_four_tests(self):
        """The tb-s1 finding: the regex lexer counted 0 of its 4 tests."""
        path = REPO / "crates/meclaw-cells/tests/misc_1/cogny_template.rs"
        self.assertEqual(gp._test_counts(path.read_text(encoding="utf-8")), (4, 4))


class TokenLexerNoReaderLosesAShape(unittest.TestCase):
    """One case per reader that failed open: each drops a binary that runs tests."""

    def test_test_counts_sees_the_runnable_test(self):
        """`_test_counts` in `_runs_no_test`: the runnable test was swallowed,
        the ignored one stayed -- `(1, 0)`, and the binary was dropped."""
        text = lexer_swallowed("#[test]\nfn fast() {}\n") + LEXER_IGNORED
        self.assertEqual(gp._test_counts(text), (2, 1))
        self.assertFalse(gp._runs_no_test(text))

    def test_the_hidden_shape_check_sees_the_item_macro(self):
        """`_runs_no_test`, hidden shapes: a test-making item macro past the
        broken string was gone and the ignored-only binary dropped."""
        text = lexer_swallowed("gen_tests!();\n") + LEXER_IGNORED
        self.assertFalse(gp._runs_no_test(text))

    def test_the_hidden_shape_check_sees_macro_rules(self):
        text = lexer_swallowed("macro_rules! m { () => {} }\nm!();\n") + LEXER_IGNORED
        self.assertFalse(gp._runs_no_test(text))

    def test_the_module_check_sees_the_module(self):
        """`_runs_no_test`, out-of-line modules: `mod cases;` was gone, so
        the binary read as module-free and was dropped although `cases.rs`
        runs a test."""
        _root, tests = lexer_tree(self, {}, {"cases.rs": "#[test]\nfn runs() {}\n"})
        text = lexer_swallowed("mod cases;\n") + LEXER_IGNORED
        self.assertFalse(gp._runs_no_test(text))
        self.assertFalse(gp._runs_no_test(text, tests))

    def test_module_texts_reads_every_module(self):
        """`_module_texts`: a `//` inside a literal became a comment and ate
        the `mod b;` behind it; only the testless `a.rs` was read."""
        _root, tests = lexer_tree(self, {}, {"a.rs": "pub fn x() {}\n",
                                             "b.rs": "#[test]\nfn runs() {}\n"})
        text = ("mod a;\n" + LEXER_DRIFT + 'const U: &str = "http://x"; mod b;\n' + LEXER_IGNORED)
        self.assertFalse(gp._runs_no_test(text, tests))

    def test_the_module_loop_sees_the_module_test(self):
        """`_runs_no_test`, module loop: the module's own test was swallowed."""
        _root, tests = lexer_tree(
            self, {}, {"m.rs": lexer_swallowed("#[test]\nfn runs() {}\n")})
        self.assertFalse(gp._runs_no_test("mod m;\n" + LEXER_IGNORED, tests))

    def test_rule_9_keeps_the_binary_in_every_plan(self):
        """The drop runs through `_drop_binaries_without_tests`, the one call
        of `test_terms`, `test_select` (`test_filter`) and the graph check
        `impact.checked_graph_terms`."""
        # rule 8 reads code and literals, no prose comment (#1160)
        text = ('const READS: &str = "scripts/test-tier.sh";\n'
                + lexer_swallowed("#[test]\nfn fast() {}\n") + LEXER_IGNORED)
        repo, _tests = lexer_tree(self, {"mixed": text})
        term = "binary_id(=meclaw-cells::mixed)"
        gp._NO_TEST_CACHE.clear()
        self.addCleanup(gp._NO_TEST_CACHE.clear)
        self.assertEqual(gp._drop_binaries_without_tests(repo, {term}), {term})
        self.assertEqual(gp.test_filter(["scripts/test-tier.sh"], "ci", repo=repo), term)
        self.assertIn(term, gp.test_terms(["scripts/test-tier.sh"], repo))

    def test_folded_form_takes_rdeps_for_a_module_it_cannot_read(self):
        """`impact.folded_form` counts with `_test_counts` when the token
        count is None. The regex lexer read the inside of a literal as code
        and found a `#[test]` there: the folded term of a module with no test
        replaced rdeps. In doubt the module hits no test -- rdeps."""
        module = ("pub " + LEXER_DOUBT + "pub " + LEXER_DRIFT
                  + 'pub const B: &str = "#[test] fn fake() {}";\n')
        repo, _tests = lexer_tree(self, {}, {
            "misc_9/main.rs": "mod helper;\nmod other;\n",
            "misc_9/helper.rs": module,
            "misc_9/other.rs": "#[test]\nfn t() {}\n"})
        from pipeline import impact as pipeline_impact  # noqa: PLC0415
        term, harness, why = pipeline_impact.folded_form(repo, "meclaw-cells::helper")
        self.assertIsNone(term)
        self.assertEqual(harness, "misc_9")
        self.assertIn("hits no test", why)


class TokenLexerDoubtKeepsTheBinary(unittest.TestCase):
    """`crate_root.Doubt` ends fail-closed in every reader: more tests, never 0."""

    def test_a_binary_the_lexer_doubts_runs(self):
        self.assertFalse(gp._runs_no_test(LEXER_IGNORED + LEXER_DOUBT))
        self.assertFalse(gp._runs_no_test(LEXER_IGNORED + "/* open comment\n"))

    def test_a_module_the_lexer_doubts_keeps_the_binary(self):
        _root, tests = lexer_tree(self, {}, {"m.rs": "pub " + LEXER_DOUBT})
        self.assertFalse(gp._runs_no_test("mod m;\n" + LEXER_IGNORED, tests))

    def test_counts_in_doubt_are_zero_and_read_as_no_proof(self):
        """`(0, 0)`: the one value both readers of the count take fail-closed
        -- `_runs_no_test` keeps a binary with 0 tests, `folded_form` plans
        rdeps for a module with 0."""
        self.assertEqual(gp._test_counts("#[test]\nfn t() {}\n" + LEXER_DOUBT), (0, 0))

    def test_the_plan_keeps_a_doubted_ignored_only_binary(self):
        # rule 8 reads code and literals, no prose comment (#1160)
        text = 'const READS: &str = "scripts/test-tier.sh";\n' + LEXER_IGNORED + LEXER_DOUBT
        repo, _tests = lexer_tree(self, {"odd": text})
        gp._NO_TEST_CACHE.clear()
        self.addCleanup(gp._NO_TEST_CACHE.clear)
        self.assertEqual(gp.test_filter(["scripts/test-tier.sh"], "ci", repo=repo),
                         "binary_id(=meclaw-cells::odd)")

    def test_a_surplus_closing_bracket_keeps_an_ignored_only_binary(self):
        """M-hC-1: the counts see one ignored test, only `_hidden_tests` meets
        the stray `}` -- that Doubt keeps the binary as well."""
        self.assertFalse(gp._runs_no_test(LEXER_IGNORED + "}\n"))

    def test_a_bracket_left_open_at_the_end_keeps_an_ignored_only_binary(self):
        """m-hC-1: a file that ends inside an open bracket is no proof of 0 tests."""
        self.assertFalse(gp._runs_no_test("#[test]\n#[ignore]\nfn slow() {\n"))
        self.assertFalse(gp._runs_no_test(LEXER_IGNORED + "mod m {\n"))


def collide_tree(case, main_rs):
    """B-hC-1: harness `misc` with a runnable `other` and an ignored-only
    `zz_live` that `main.rs` declares in a form that does not fold -- both
    `main.rs` and `zz_live.rs` are the unit `misc/main`."""
    repo, tests = lexer_tree(case, {}, {
        "misc/main.rs": main_rs,
        "misc/other.rs": "#[test]\nfn runs() {}\n",
        "misc/zz_live.rs": LEXER_IGNORED,
    }, crate="c1")
    for cache in (gp._SOURCE_CACHE, gp._NO_TEST_CACHE):
        cache.clear()
        case.addCleanup(cache.clear)
    return repo


class RuleNineReadsEveryFileOfAUnit(unittest.TestCase):
    """B-hC-1: rule 9 kept one text per term, the last file of a unit won.
    Every file of the unit is read now: the term goes only when none runs a test."""

    WHOLE = "binary_id(=c1::misc)"
    FORMS = {
        "rustfmt cfg": 'pub mod other;\n#[cfg(feature = "live")]\npub mod zz_live;\n',
        "one-line path (tb-s1)": 'pub mod other;\n#[path = "zz_live.rs"] pub mod zz_live;\n',
    }

    def test_the_harness_binary_stays_when_a_later_file_of_its_unit_runs_no_test(self):
        for form, main_rs in self.FORMS.items():
            with self.subTest(form):
                repo = collide_tree(self, main_rs)
                self.assertEqual("misc/main", gp._folded_unit(main_rs, "misc", "zz_live"))
                self.assertEqual({self.WHOLE},
                                 gp._drop_binaries_without_tests(repo, {self.WHOLE}))

    def test_a_diff_to_the_harness_main_plans_the_harness(self):
        for form, main_rs in self.FORMS.items():
            with self.subTest(form):
                repo = collide_tree(self, main_rs)
                self.assertEqual(self.WHOLE, gp.test_filter(["crates/c1/tests/misc/main.rs"],
                                                            "strand", repo=repo))

    def test_a_unit_whose_every_file_runs_no_test_still_goes(self):
        repo, _tests = lexer_tree(self, {"live": LEXER_IGNORED}, crate="c1")
        for cache in (gp._SOURCE_CACHE, gp._NO_TEST_CACHE):
            cache.clear()
            self.addCleanup(cache.clear)
        self.assertEqual(set(), gp._drop_binaries_without_tests(
            repo, {"binary_id(=c1::live)"}))


class TokenLexerReadersStayExact(unittest.TestCase):
    """What rule 9 must still drop (GH #997) and what it reads as shapes."""

    def test_an_ignored_only_binary_after_a_continued_string_is_dropped(self):
        self.assertTrue(gp._runs_no_test(LEXER_DRIFT + LEXER_IGNORED))

    def test_an_item_macro_inside_an_inline_module_is_a_hidden_shape(self):
        self.assertFalse(gp._runs_no_test(LEXER_IGNORED + "mod props {\n    gen_tests!();\n}\n"))

    def test_statement_macros_in_a_body_are_no_shape(self):
        self.assertTrue(gp._runs_no_test(
            "#[test]\n#[ignore]\nfn slow() {\n    assert_eq!(1, 1);\n    println!();\n}\n"))

    def test_a_negative_impl_is_no_macro(self):
        self.assertTrue(gp._runs_no_test(LEXER_IGNORED + "struct S;\nimpl !Send for S {}\n"))

    def test_a_leading_path_separator_still_names_a_test(self):
        self.assertEqual(gp._test_counts("#[::tokio::test]\nasync fn t() {}\n"), (1, 1))

    def test_a_path_module_with_two_path_attributes_is_doubt(self):
        _root, tests = lexer_tree(self, {}, {"x.rs": "pub fn x() {}\n",
                                             "y.rs": "#[test]\nfn runs() {}\n"})
        text = LEXER_IGNORED + '#[path = "x.rs"]\n#[path = "y.rs"]\nmod m;\n'
        self.assertFalse(gp._runs_no_test(text, tests))

    def test_a_visible_path_module_without_tests_drops_the_binary(self):
        _root, tests = lexer_tree(self, {}, {"support/h.rs": "pub fn x() {}\n"})
        text = LEXER_IGNORED + '#[cfg(test)]\n#[path = "support/h.rs"]\npub(crate) mod h;\n'
        self.assertTrue(gp._runs_no_test(text, tests))


def scenario_repo(case, stems):
    """A throw-away tree whose test targets are exactly `stems` (plus `gh2_all`)."""
    tmp = tempfile.TemporaryDirectory()
    case.addCleanup(tmp.cleanup)
    root = pathlib.Path(tmp.name)
    tests = root / "crates" / "meclaw-cells" / "tests"
    tests.mkdir(parents=True)
    (root / "crates" / "meclaw-cells" / "Cargo.toml").write_text(
        '[package]\nname = "meclaw-cells"\n', encoding="utf-8")
    for stem in list(stems) + ["gh2_all"]:
        (tests / (stem + ".rs")).write_text("fn main() {}\n", encoding="utf-8")
    return str(root)


class ScenarioFilterFollowsTheTree(unittest.TestCase):
    """GH #934: a scenario operator for a binary the tree does not have.

    nextest refuses a filterset whose `binary(...)` operator matches nothing
    ("operator didn't match any binary IDs"), and that takes the whole `tests`
    station down. A public clone has no `slack_live.rs`, so the subtraction
    `- (... binary(/^slack_live$/) ...)` broke every `tests` station there. The
    resolver now names only the operators that match a test target in the tree
    -- every entry of the list, not only `slack_live`.
    """

    def test_a_tree_without_slack_live_plans_no_operator_for_it(self):
        repo = scenario_repo(self, ["phase_7_demo"])
        expr = gp.scenario_for(repo)
        self.assertNotIn("slack_live", expr)
        self.assertIn("binary(/_demo$/)", expr)
        self.assertNotIn("binary(/e2e/)", expr)

    def test_a_tree_with_slack_live_plans_it(self):
        repo = scenario_repo(self, ["slack_live", "proxy_edge_e2e"])
        expr = gp.scenario_for(repo)
        self.assertIn("binary(/^slack_live$/)", expr)
        self.assertIn("binary(/e2e/)", expr)
        self.assertNotIn("_demo", expr)

    def test_a_directory_target_counts(self):
        repo = scenario_repo(self, [])
        d = pathlib.Path(repo) / "crates" / "meclaw-cells" / "tests" / "workshop_scenario"
        d.mkdir()
        (d / "main.rs").write_text("fn main() {}\n", encoding="utf-8")
        self.assertEqual(gp.scenario_for(repo), "binary(/^workshop_scenario$/)")

    def test_the_real_tree_keeps_the_whole_class(self):
        """Every operator of the class matches a target of the full tree.

        Presence guard: the public clone ships without `slack_live.rs`, and
        there the resolver drops that operator on purpose (the two tests
        above lock both directions on throw-away trees).
        """
        private_tree.require("crates/meclaw-cells/tests/slack_live.rs")
        self.assertEqual(gp.scenario_for(None), gp.SCENARIO)

    def test_the_tests_scope_subtracts_only_what_exists(self):
        repo = scenario_repo(self, ["slack_live"])
        st = by_name(gp.plan(["crates/meclaw-cells/tests/gh2_all.rs"], "strand", repo=repo))
        self.assertEqual(st["tests"].scope,
                         "binary_id(=meclaw-cells::gh2_all) - (binary(/^slack_live$/))")

    def test_no_scenario_target_means_no_subtraction(self):
        repo = scenario_repo(self, [])
        st = by_name(gp.plan(["crates/meclaw-cells/tests/gh2_all.rs"], "strand", repo=repo))
        self.assertEqual(st["tests"].scope, "binary_id(=meclaw-cells::gh2_all)")

    def test_print_scenario_reads_the_named_tree(self):
        repo = scenario_repo(self, ["slack_live"])
        r = subprocess.run([sys.executable, str(SCRIPTS / "gate_plan.py"),
                            "--print", "scenario", "--repo", repo],
                           capture_output=True, text=True, check=True)
        self.assertEqual(r.stdout.strip(), "binary(/^slack_live$/)")

    def test_print_scenario_of_an_empty_class_is_a_valid_filterset(self):
        """`test-tier.sh` builds `all() - ( <this> )`; an empty string is no filterset."""
        repo = scenario_repo(self, [])
        r = subprocess.run([sys.executable, str(SCRIPTS / "gate_plan.py"),
                            "--print", "scenario", "--repo", repo],
                           capture_output=True, text=True, check=True)
        self.assertEqual(r.stdout.strip(), "none()")


class EnvStationsAreMarked(unittest.TestCase):
    """GH #934: the stations whose runner reads `<repo>/.env`.

    A build host never gets the owner's `.env`; it gets a secret-free one
    (`--print lane-env`, GH #942) and runs these stations there like any other.
    The mark lives here, beside the stations, and nowhere else.
    """

    def test_the_marked_stations(self):
        self.assertEqual(set(gp.ENV_STATIONS),
                         {"scenarios:memory", "scenarios:builder", "guide-selftest"})

    def test_every_marked_station_is_a_station(self):
        for name in gp.ENV_STATIONS:
            with self.subTest(station=name):
                self.assertIn(name, gp.STATION_ORDER)

    def test_print_env_stations(self):
        r = subprocess.run([sys.executable, str(SCRIPTS / "gate_plan.py"),
                            "--print", "env-stations"],
                           capture_output=True, text=True, check=True)
        self.assertEqual(r.stdout.split(), list(gp.ENV_STATIONS))

    def test_json_rows_carry_the_mark(self):
        """Table: a release over a docs diff plans all three, display unmarked."""
        out = gp.to_json(["docs/a.md"], "release", None)
        env = {row["name"]: row.get("env", False) for row in out["stations"]}
        for name in ("scenarios:memory", "scenarios:builder", "guide-selftest"):
            with self.subTest(station=name):
                self.assertIs(env[name], True)
        self.assertIs(env["scenarios:display"], False)
        self.assertIs(env.get("tests", False), False)

    def test_a_strand_over_a_script_diff_plans_no_env_station(self):
        """The normal case of a lane: nothing to run locally afterwards."""
        out = gp.to_json(["scripts/strand.sh"], "strand", None)
        self.assertFalse([r["name"] for r in out["stations"] if r.get("env")])

    # --- GH #942: a lane runs EVERY station. The owner's machine runs no cargo
    # while a wave builds on the lanes (a lab colony died at load 15 on
    # 2026-10-02), so nothing is left over to run here after a lane: the env
    # stations get a secret-free `.env` there, `display-lab` finds `node` there
    # and `export-audit` gets the `github-main` ref pushed along.

    def test_lane_local_is_empty(self):
        self.assertEqual({}, dict(gp.LANE_LOCAL))
        r = subprocess.run([sys.executable, str(SCRIPTS / "gate_plan.py"),
                            "--print", "lane-local"],
                           capture_output=True, text=True, check=True)
        self.assertEqual("", r.stdout)

    def test_lane_env_carries_no_secret_value(self):
        """`--print lane-env`: the allowlist with its constants, read from no file.

        The generator runs in a tree whose `.env` is unreadable: a generator that
        opened it would crash, and one that copied a value would show it. A
        provider key NAME may stand in the table -- a colony with an `llm` cell
        resolves it at boot, even in the keyless class (GH #942) -- but only with
        the constant non-secret placeholder `lane-stub`."""
        with tempfile.TemporaryDirectory() as tmp:
            env_file = pathlib.Path(tmp) / ".env"
            env_file.write_text("OPENROUTER_API_KEY=never\nMODEL_EXTRACTOR=never\n")
            env_file.chmod(0)
            r = subprocess.run([sys.executable, str(SCRIPTS / "gate_plan.py"),
                                "--print", "lane-env", "--repo", tmp],
                               cwd=tmp, capture_output=True, text=True)
        self.assertEqual(0, r.returncode, r.stderr)
        self.assertNotIn("never", r.stdout)
        pairs = [l.split("=", 1) for l in r.stdout.splitlines()]
        self.assertTrue(pairs)
        self.assertEqual(dict(pairs), dict(gp.LANE_ENV))
        for name, value in pairs:
            with self.subTest(name=name):
                self.assertIn(value, ("lane-stub", "low", "8"))
                if re.search(r"KEY|TOKEN|SECRET|PASS", name):
                    self.assertEqual("lane-stub", value)

    def test_lane_env_covers_what_the_suites_read(self):
        """The allowlist IS the contract: every name the scenario runners copy
        out of `.env` and a boot resolves (L-0, GH #942), and not one name more."""
        self.assertEqual(set(gp.LANE_ENV), {
            "MODEL_EXTRACTOR", "MODEL_DIALECTIC", "MODEL_DREAMER", "MODEL_JUDGE",
            "MODEL_CLOSER", "MEMORY_EMBED_MODEL", "MEMORY_EMBED_DIM",
            "MEMORY_REASONING_EXTRACT", "MEMORY_REASONING_DIALECTIC",
            "MEMORY_REASONING_DREAM", "MEMORY_REASONING_JUDGE",
            "MEMORY_REASONING_CLOSE", "MODEL_BUILDER", "OPENROUTER_API_KEY"})


class Explain(unittest.TestCase):
    """`--explain` (W3, GH #1117): why each station is planned, and the
    structured selection the `tests` filterset is rendered from.

    Additive only: without the flag the JSON keeps its legacy keys, and the
    TSV form never changes.
    """

    # `parallel` is a plan key (V0 section 4, rendered since W7), not an explain key:
    # `browser:display` is the first station to carry it (GH #1161).
    LEGACY_KEYS = {"name", "scope", "cargo", "cmd", "cwd", "cmds", "cwds", "run", "env",
                   "parallel"}

    def test_json_without_explain_keeps_the_legacy_keys(self):
        repo = mini_repo(self)
        for files, mode in ((["crates/meclaw-cells/src/lib.rs"], "strand"),
                            (["docs/a.md"], "integration"),
                            (["templates/display/x.json"], "release"),
                            ([], "ci")):
            plain = gp.to_json(files, mode, repo)
            explained = gp.to_json(files, mode, repo, explain=True)
            for row in plain["stations"]:
                self.assertLessEqual(set(row), self.LEGACY_KEYS, row["name"])
            stripped = {k: v for k, v in explained.items()}
            stripped["stations"] = [
                {k: v for k, v in row.items() if k not in ("reason", "select")}
                for row in explained["stations"]]
            self.assertEqual(plain, stripped, (files, mode))

    def test_explain_names_class_and_path(self):
        repo = mini_repo(self)
        out = gp.to_json(["crates/meclaw-cells/src/lib.rs"], "strand", repo, explain=True)
        rows = {r["name"]: r for r in out["stations"]}
        self.assertIn("rust_src: crates/meclaw-cells/src/lib.rs", rows["clippy"]["reason"])
        self.assertEqual(rows["roadmap-anchors"]["reason"], ["always"])
        self.assertEqual(rows["precheck"]["reason"], ["diff: crates/meclaw-cells/src/lib.rs"])
        self.assertIn("rust_src: crates/meclaw-cells/src/lib.rs", rows["tests"]["reason"])

    def test_explain_names_the_mode_for_mode_stations(self):
        repo = mini_repo(self)
        out = gp.to_json(["docs/a.md"], "integration", repo, explain=True)
        rows = {r["name"]: r for r in out["stations"]}
        self.assertEqual(rows["export-audit"]["reason"], ["mode: integration"])
        self.assertIn("mode: integration", rows["catalogue"]["reason"])

    def test_tests_select_is_structured(self):
        repo = mini_repo(self)
        out = gp.to_json(["crates/meclaw-cells/tests/mock_openai.rs"], "strand", repo,
                         explain=True)
        tests = {r["name"]: r for r in out["stations"]}["tests"]
        sel = tests["select"]
        self.assertEqual(sel["binaries"], ["meclaw-cells::gh3_llm", "meclaw-cells::mock_openai"])
        self.assertEqual(sel["rdeps"], [])
        self.assertFalse(sel["all"])
        self.assertFalse(sel["floor"])
        self.assertEqual(sel["extra"], [])
        self.assertEqual(sel["filter"], tests["scope"])
        self.assertIn("rust_test: crates/meclaw-cells/tests/mock_openai.rs", tests["reason"])

    def test_select_rdeps_all_and_floor(self):
        repo = mini_repo(self)
        sel = gp.test_select(["crates/meclaw-cells/src/lib.rs"], "strand", repo)
        self.assertEqual(sel["rdeps"], ["meclaw-cells"])
        self.assertEqual(sel["filter"], gp.test_filter(["crates/meclaw-cells/src/lib.rs"],
                                                       "strand", repo))
        sel = gp.test_select(["Cargo.lock"], "strand", repo)
        self.assertTrue(sel["all"])
        self.assertEqual(sel["filter"], "all()")
        sel = gp.test_select([], "strand", repo)
        self.assertTrue(sel["floor"])
        self.assertEqual(sel["filter"], gp.T0_FLOOR)
        self.assertIsNone(gp.test_select(["docs/nothing-reads-me.md"], "strand", repo))

    def test_select_reports_the_rule_8_reader(self):
        repo = mini_repo(self)
        out = gp.to_json(["examples/organism/grow-1.json"], "strand", repo, explain=True)
        tests = {r["name"]: r for r in out["stations"]}["tests"]
        self.assertEqual(tests["select"]["binaries"], ["meclaw-cells::gh4_example"])
        self.assertTrue(any(r.endswith(": examples/organism/grow-1.json")
                            for r in tests["reason"]), tests["reason"])

    def test_terms_and_select_are_views_on_one_rule_source(self):
        """W6 x W3: `test_terms` (reasons per term) and `test_select` (the
        structured selection) read the same `_term_rules` -- the keys of one
        are exactly the terms of the other, for every rule."""
        repo = mini_repo(self)
        # W6 delta M7 (#1133): a shared test module included by a second binary
        pathlib.Path(repo, "crates", "meclaw-cells", "tests", "gh5_llm_too.rs").write_text(
            "mod mock_openai;\nfn main() {}\n", encoding="utf-8")
        diffs = [["crates/meclaw-cells/src/lib.rs"], ["Cargo.lock"],
                 ["examples/organism/grow-1.json"], ["templates/display/x.json"],
                 ["docs/nothing-reads-me.md"],
                 ["crates/meclaw-cells/src/lib.rs", "examples/organism/grow-1.json"],
                 ["crates/meclaw-cells/tests/mock_openai.rs"]]
        shared = gp.test_select(["crates/meclaw-cells/tests/mock_openai.rs"], "strand", repo)
        # both includers, and the flat file itself (cargo builds every tests/*.rs)
        self.assertEqual(["meclaw-cells::gh3_llm", "meclaw-cells::gh5_llm_too",
                          "meclaw-cells::mock_openai"], sorted(shared["binaries"]))
        for files, mode in [(f, m) for f in diffs for m in ("ci", "strand")]:
            with self.subTest(files=files, mode=mode):
                terms = gp.test_terms(files, repo)
                sel = gp.test_select(files, mode, repo)
                if sel is None:
                    self.assertEqual({}, terms)
                    continue
                self.assertEqual(" + ".join(sorted(terms)), sel["filter"])
                self.assertTrue(all(r.startswith("rule:") for rs in terms.values() for r in rs))
                self.assertEqual(len(sel["why"]), len(set(sel["why"])), sel["why"])

    def test_cli_terms_prints_the_reason_per_term(self):
        repo = mini_repo(self)
        res = subprocess.run(
            [sys.executable, str(SCRIPTS / "gate_plan.py"), "--mode", "strand", "--repo", str(repo),
             "--files", "crates/meclaw-cells/src/lib.rs", "--terms"],
            capture_output=True, text=True)
        self.assertEqual(0, res.returncode, res.stderr)
        self.assertEqual({"rdeps(meclaw-cells)": ["rule:2 file:crates/meclaw-cells/src/lib.rs"]},
                         json.loads(res.stdout))

    def test_browser_select_lists_the_lock_binaries(self):
        repo = mini_repo(self)
        out = gp.to_json(["templates/display/x.json"], "integration", repo, explain=True)
        browser = {r["name"]: r for r in out["stations"]}["browser:display"]
        extra = browser["select"]["extra"]
        # Z8 (W3 delta3 M1): ALL seven locks by name, not two of them
        self.assertEqual(extra, BROWSER_LOCK_IDS)
        self.assertEqual(browser["select"]["binaries"], [])

    def test_browser_select_without_the_colony_half_lists_six_locks(self):
        st = types.SimpleNamespace(cmds=gp.browser_cmds(colony=False))
        extra = gp._browser_select(st)["extra"]
        self.assertEqual(extra, [i for i in BROWSER_LOCK_IDS if "710_the_colony_" not in i])
        self.assertEqual(6, len(extra))

    def test_every_planned_station_has_a_reason(self):
        """The trigger table is honest: a station the resolver planned in
        strand mode for one path names that path -- or says `always`."""
        repo = mini_repo(self)
        samples = [
            "crates/meclaw-cells/src/lib.rs", "crates/meclaw-cells/tests/gh1_uses_hive.rs",
            "crates/meclaw-cells/src/recall/x.rs", "Cargo.lock", ".config/nextest.toml",
            ".github/gates/unwrap_budget.txt", ".github/fixtures/a.txt",
            "templates/README.md", "templates/memory-hive/a.md", "templates/display/a.json",
            "templates/display/compose/scenarios/s.json", "templates/builder/a.json",
            "examples/organism/grow-1.json", "examples/meclaw-os/a.json", "docs/a.md",
            "README.md", "workshop/evals/scenarios/a.json", "workshop/evals/engine.py",
            "workshop/evals/p5-longmemeval/judge.py", "workshop/evals/builder-scenarios/a.py",
            "workshop/evals/conversation-guide/a.py", "workshop/tools/display-lab/a.py",
            "workshop/tools/display-layout-browser.mjs", "workshop/tools/llm-conformance/a.py",
            "workshop/evals/persona/a.py", "scripts/strand.sh", "scripts/tests/test_x.py",
            "plans/export-fixtures/a.py", ".github/workflows/ci.yml", "plans/x/plan.md",
        ]
        for path in samples:
            out = gp.to_json([path], "strand", repo, explain=True)
            for row in out["stations"]:
                reason = row["reason"]
                self.assertTrue(reason, (path, row["name"]))
                if reason == ["always"] or row["name"] == "slow-tests":
                    continue
                self.assertTrue(any(r.endswith(": " + path) for r in reason),
                                (path, row["name"], reason))

    def test_tsv_is_unchanged_by_explain(self):
        r = subprocess.run(
            [sys.executable, str(SCRIPTS / "gate_plan.py"), "--mode", "strand",
             "--files", "crates/meclaw-colony/src/lib.rs", "--format", "tsv", "--explain"],
            capture_output=True, text=True, cwd=str(REPO), check=True)
        plain = subprocess.run(
            [sys.executable, str(SCRIPTS / "gate_plan.py"), "--mode", "strand",
             "--files", "crates/meclaw-colony/src/lib.rs", "--format", "tsv"],
            capture_output=True, text=True, cwd=str(REPO), check=True)
        self.assertEqual(r.stdout, plain.stdout)

    def test_cli_explain_adds_reason_and_select(self):
        r = subprocess.run(
            [sys.executable, str(SCRIPTS / "gate_plan.py"), "--mode", "strand",
             "--files", "crates/meclaw-colony/src/lib.rs", "--format", "json", "--explain"],
            capture_output=True, text=True, cwd=str(REPO), check=True)
        doc = json.loads(r.stdout)
        rows = {s["name"]: s for s in doc["stations"]}
        for row in doc["stations"]:
            self.assertIsInstance(row["reason"], list)
        self.assertIn("rdeps", rows["tests"]["select"])


# #1160 (tb-s2, rule 8 without prose comments): `gh617`/`gh623` named
# `scripts/gate_plan.py` only in a `//!` paragraph and became its readers.
P = "scripts/test-tier.sh"
TESTFN = "#[test]\nfn t() {}\n"


class ProseCommentsReadNothing(unittest.TestCase):
    """Rule 8 scans code and literals -- never a comment of any form, its
    code blocks included (r1-kit Fix-Runde 1b, V2-R1-1).

    In doubt (an unterminated literal or comment) the text counts as it is."""

    def readers(self, sources, path=P):
        repo = reader_repo(self, sources)
        self.addCleanup(gp._SOURCE_CACHE.clear)
        return sorted(gp._tests_reading_path(repo, path))

    def test_a_prose_comment_alone_makes_no_reader(self):
        for name, text in {
            "inner_doc": "//! deliberately not named `*_e2e`: `%s` subtracts\n" % P,
            "outer_doc": "/// see %s\n" % P,
            "line": "// run with %s\n" % P,
            "trailing": "fn f() {} // %s\n" % P,
            "block": "/* %s */\n" % P,
            "block_doc": "/** the gate %s\n * plans it */\n" % P,
            "block_inner_doc": "/*!\n * %s\n */\n" % P,
            "nested_block": "/* outer /* inner */ still %s */\n" % P,
            "fence_in_prose_then_closed": ("//! ```text\n//! x\n//! ```\n"
                                           "//! later %s\n" % P),
            "plain_comment_fence": "// ```\n// %s\n// ```\n" % P,
            "char_quote": "fn f() { let q = '\"'; } // %s\n" % P,
            "lifetime": "fn f<'a>(x: &'a str) {} // %s\n" % P,
            "an_item_ends_the_fence": "/// ```\nfn f() {}\n/// %s\n" % P,
        }.items():
            with self.subTest(name):
                self.assertEqual(self.readers({"r": text + TESTFN}), [], name)

    def test_code_and_literals_still_read(self):
        for name, text in {
            "string": 'const R: &str = "%s";\n' % P,
            "raw": 'const R: &str = r"%s";\n' % P,
            "raw_hashes": 'const R: &str = r#"say "%s""#;\n' % P,
            "byte": 'const R: &[u8] = b"%s";\n' % P,
            "include": 'const R: &str = include_str!("../../../%s");\n' % P,
            "attr_doc": '#[doc = "%s"]\nfn f() {}\n' % P,
            "after_a_comment": '// prose\nconst R: &str = "%s";\n' % P,
            "after_a_block": '/* prose */ const R: &str = "%s";\n' % P,
        }.items():
            with self.subTest(name):
                self.assertEqual(self.readers({"r": text + TESTFN}),
                                 ["binary_id(=meclaw-cells::r)"], name)

    def test_an_ignore_reason_alone_makes_no_reader(self):
        """Re-check r1-kit M5: the reason of `#[ignore = "..."]` says why a
        test is skipped -- prose, it opens nothing. gh867/gh961/gh1028 named
        `scripts/gate_plan.py` there and planned browser:display on every
        resolver diff."""
        for name, text in {
            "reason": '#[ignore = "browser lock: (%s BROWSER_LOCKS)"]\n' % P,
            "tight": '#[ignore="%s"]\n' % P,
            "raw": '#[ignore = r#"%s"#]\n' % P,
            "spread": '#[\n    ignore =\n        "%s"\n]\n' % P,
        }.items():
            with self.subTest(name):
                self.assertEqual(self.readers({"r": text + TESTFN}), [], name)

    def test_a_literal_beside_an_ignore_reason_still_reads(self):
        for name, text in {
            "binding": 'fn f() { let ignore = "%s"; }\n' % P,
            "after_the_reason": '#[ignore = "slow"]\nconst R: &str = "%s";\n' % P,
            "bare_ignore": '#[ignore]\nconst R: &str = "%s";\n' % P,
            "both": '#[ignore = "see %s"]\nconst R: &str = "%s";\n' % (P, P),
        }.items():
            with self.subTest(name):
                self.assertEqual(self.readers({"r": text + TESTFN}),
                                 ["binary_id(=meclaw-cells::r)"], name)

    def test_a_failure_message_alone_makes_no_reader(self):
        """Re-check r1-kit M5 (same class): the message of an assert, panic or
        expect says what went wrong -- prose. A reader opens the file in code."""
        for name, text in {
            "assert": 'fn f() { assert!(x, "see %s", y); }\n' % P,
            "assert_lines": 'fn f() {\n    assert!(\n        a == b,\n        "{v}: see %s \\\n         § 4"\n    );\n}\n' % P,
            "assert_eq": 'fn f() { assert_eq!(a, b, "drifted (%s)"); }\n' % P,
            "assert_ne": 'fn f() { assert_ne!(g(a, b), c, "%s"); }\n' % P,
            "debug_assert": 'fn f() { debug_assert!(h(1, (2, 3)), "%s"); }\n' % P,
            "panic": 'fn f() { panic!("see %s"); }\n' % P,
            "panic_closure": 'fn f() { x.unwrap_or_else(|| panic!("{rel}: %s")); }\n' % P,
            "panic_raw": 'fn f() { panic!(r#"see "%s""#); }\n' % P,
            "expect": 'fn f() { x.expect("rules in %s"); }\n' % P,
            "unreachable": "fn f() { unreachable!(\"%s\") }\n" % P,
            "char_paren": "fn f() { let c = '('; assert!(x, \"%s\"); }\n" % P,
        }.items():
            with self.subTest(name):
                self.assertEqual(self.readers({"r": text + TESTFN}), [], name)

    def test_a_path_in_the_code_of_an_assert_still_reads(self):
        for name, text in {
            "read_in_cond": 'fn f() { assert_eq!(std::fs::read_to_string("%s").unwrap(), x); }\n' % P,
            "exists_in_cond": 'fn f() { assert!(exists("%s"), "missing"); }\n' % P,
            "read_in_args": 'fn f() { assert!(x, "{}", read("%s")); }\n' % P,
            "eq_value": 'fn f() { assert_eq!(a, "%s"); }\n' % P,
            "panic_arg": 'fn f() { panic!("{}", "%s"); }\n' % P,
            "concat": 'fn f() { assert!(x, concat!("a ", "%s")); }\n' % P,
            "binding_and_message": 'const R: &str = "%s";\nfn f() { assert!(x, "see %s"); }\n' % (P, P),
            "free_expect": 'fn f() { expect("%s"); }\n' % P,
            "message_then_code": 'fn f() { panic!("a" + "%s"); }\n' % P,
        }.items():
            with self.subTest(name):
                self.assertEqual(self.readers({"r": text + TESTFN}),
                                 ["binary_id(=meclaw-cells::r)"], name)

    def test_the_real_assert_prose_reads_no_rules_page(self):
        """The 9 measured messages: they name docs/development-rules.md only
        in an assert/panic message."""
        gp._SOURCE_CACHE.clear()
        self.addCleanup(gp._SOURCE_CACHE.clear)
        terms = gp._tests_reading_path(str(REPO), "docs/development-rules.md")
        got = " ".join(terms)
        for stem in ("709_an_app_takes_its_view_down", "gh823_no_recall_key",
                     "gh302_assistant_wires", "gh458_the_pack_slots",
                     "gh488_the_agent_record", "gh527_the_member_writes",
                     "a3_spec_claims_gate_drift"):
            with self.subTest(stem):
                self.assertNotIn(stem, got)
        # gh458/gh488 pulled their harness misc_4 in whole
        self.assertNotIn("binary_id(=meclaw-cells::misc_4)", terms)

    def test_the_real_browser_tests_read_no_resolver(self):
        """The measured case of M5: the three browser tests name the resolver
        only in their ignore reasons."""
        gp._SOURCE_CACHE.clear()
        self.addCleanup(gp._SOURCE_CACHE.clear)
        got = gp._tests_reading_path(str(REPO), "scripts/gate_plan.py")
        self.assertEqual([], [t for t in got if t.endswith("_browser)")], got)

    def test_a_code_block_in_a_doc_comment_reads_nothing(self):
        """A test file under `crates/*/tests/` is no library: rustdoc never runs
        its doc-comment code, nextest neither -- a path there is prose like any
        other comment (V2-R1-1)."""
        for name, text in {
            "outer": "/// ```\n/// let p = \"%s\";\n/// ```\nfn f() {}\n" % P,
            "inner": "//! Prose.\n//!\n//! ```rust\n//! std::fs::read(\"%s\");\n//! ```\n" % P,
            "tilde": "/// ~~~\n/// %s\n/// ~~~\nfn f() {}\n" % P,
            "unclosed": "/// ```\n/// %s\nfn f() {}\n" % P,
            "block": "/**\n * ```\n * let p = \"%s\";\n * ```\n */\nfn f() {}\n" % P,
            "block_inner": "/*!\n```\n%s\n```\n*/\n" % P,
            "indented": "/// Prose.\n///\n///     let p = \"%s\";\nfn f() {}\n" % P,
            # re-check tb-s2 Zusatz-Check m-1: only an item ends an open fence
            "over_plain": "/// ```\n// plain\n/// let p = \"%s\";\n/// ```\nfn f() {}\n" % P,
            "over_attr": "/// ```\n#[cfg(any(a, b))]\n/// %s\n/// ```\nfn f() {}\n" % P,
            "over_block_doc": "/** ``` */\n/// %s\n/// ```\nfn f() {}\n" % P,
        }.items():
            with self.subTest(name):
                self.assertEqual(self.readers({"r": text + TESTFN}), [], name)

    def test_a_comment_path_that_code_also_opens_reads(self):
        """The same path in a `//`, `///` or `/* */` comment AND in code that
        opens or embeds it: the code reads (V2-R1-1 control)."""
        comments = {"line": "// %s\n" % P, "doc": "/// %s\nfn g() {}\n" % P,
                    "block": "/* %s */\n" % P}
        codes = {"read": 'fn h() { let _ = std::fs::read_to_string("%s"); }\n' % P,
                 "include": 'const R: &str = include_str!("../../../%s");\n' % P}
        for c, comment in comments.items():
            for k, code in codes.items():
                with self.subTest(comment=c, code=k):
                    self.assertEqual(self.readers({"r": comment + code + TESTFN}),
                                     ["binary_id(=meclaw-cells::r)"])

    def test_a_double_slash_inside_a_literal_is_no_comment(self):
        for name, text in {
            "url": 'const U: &str = "http://h/%s";\n' % P,
            "url_then_path": 'const U: &str = "http://h"; const R: &str = "%s";\n' % P,
            "raw": 'const U: &str = r"// %s";\n' % P,
            "raw_hashes": 'const U: &str = r#"a "quoted" // %s"#;\n' % P,
            "block_open": 'const U: &str = "/* not a comment"; const R: &str = "%s";\n' % P,
            "escaped_quote": 'const U: &str = "a \\" // b %s";\n' % P,
            "char_slash": "const C: char = '/'; const D: char = '/'; const R: &str = \"%s\";\n" % P,
            "multiline": 'const U: &str = "first\n// %s\n";\n' % P,
            # re-check tb-s2 Zusatz-Check B-2: raw C strings kept the lexer in step
            "raw_c_quote": 'const C: &CStr = cr#"a"b"#;\nconst U: &str = "http://h/%s";\n' % P,
            "raw_c_backslash": 'const C: &CStr = cr"C:\\";\nconst U: &str = "http://h/%s";\n' % P,
        }.items():
            with self.subTest(name):
                self.assertEqual(self.readers({"r": text + TESTFN}),
                                 ["binary_id(=meclaw-cells::r)"], name)

    def test_a_nested_block_comment_never_eats_a_literal(self):
        """`/* a /* b */ "x */` is still one comment; the literal after it reads."""
        text = '/* a /* b */ say "hi */\nconst R: &str = "http://h/%s";\n' % P
        self.assertEqual(self.readers({"r": text + TESTFN}),
                         ["binary_id(=meclaw-cells::r)"])

    def test_doubt_counts_the_text_as_it_is(self):
        for name, text in {
            "open_string": '// %s\nconst R: &str = "never closed;\n' % P,
            "open_block": "/* %s\n" % P,
            "open_raw": '// %s\nconst R: &str = r#"never closed";\n' % P,
        }.items():
            with self.subTest(name):
                self.assertEqual(self.readers({"r": text + TESTFN}),
                                 ["binary_id(=meclaw-cells::r)"], name)

    def test_the_script_shape_skips_prose_too(self):
        """Rule 8 runtime, script shape: a test that names the generator only
        in prose reads none of its sources."""
        script, globs = gp.SCRIPT_READS[0]
        src = gp.CORPUS_SOURCES[0]
        repo = reader_repo(self, {
            "prose": "//! regenerate with %s\n" % script + TESTFN,
            "code": 'const G: &str = "%s";\n' % script + TESTFN})
        self.addCleanup(gp._SOURCE_CACHE.clear)
        got = gp._runtime_readers(repo, src)
        self.assertIn(("meclaw-cells", "code"), got)
        self.assertNotIn(("meclaw-cells", "prose"), got)

    def test_reader_text_keeps_offsets_and_lines(self):
        text = '// a %s\nlet s = "x"; /* b */ let t = 1;\n' % P
        got = gp._reader_text(text)
        self.assertEqual(len(got), len(text))
        self.assertEqual(got.count("\n"), text.count("\n"))
        self.assertNotIn(P, got)
        self.assertIn('let s = "x";', got)
        self.assertIn("let t = 1;", got)

    def test_a_lexer_exception_counts_the_whole_text(self):
        # Re-check tb-s2 Zusatz-Check m-2: no fallback -- in doubt all counts.
        text = "// %s m-2\n" % P
        with mock.patch.object(gp, "_reader_lex", side_effect=RuntimeError("boom")):
            self.assertEqual(text, gp._reader_text(text))
        gp._READER_CACHE.pop(text, None)

    def test_prose_and_a_walk_of_an_ancestor_make_no_literal_reader(self):
        """V2-R1-1 (replaces re-check tb-s2 B-1): a comment names nothing, also
        in a unit that walks an ancestor directory. The walk is the runtime
        rule's: gh408 walks templates/ and reads templates/README.md by the
        directory rule, its `//!` paragraph plays no part; gh482/gh496 name
        `docs/development-rules.md` only in comments (their `"docs"` is a
        table name) and read it not at all."""
        walk = 'fn w() { let _ = std::fs::read_dir(root().join("%s")); }\n'
        self.assertEqual(self.readers({"w": "//! see %s\n" % P + walk % "scripts" + TESTFN,
                                       "o": "//! see %s\n" % P + walk % "docs" + TESTFN}), [])
        gp._SOURCE_CACHE.clear()
        self.addCleanup(gp._SOURCE_CACHE.clear)
        rt = gp._tests_reading_at_runtime(str(REPO), "templates/README.md")
        self.assertIn("(binary_id(=meclaw-cells::misc_3) & "
                      "test(/^gh408_prose_names_a_template_without_a_version::/))", rt)
        lit = gp._tests_reading_path(str(REPO), "docs/development-rules.md")
        self.assertFalse([t for t in lit if "gh482" in t or "gh496" in t], lit)

    def test_a_walk_of_the_catalogue_reads_its_top_level_files(self):
        """A top-level file of templates/ belongs to no template, so rules 4/5
        do not plan it: a test that enumerates the catalogue reads it (V2-R1-1,
        the B-1 shape without prose). A path into one template stays rule 4's,
        examples/ keeps its contract (no reader by a walk)."""
        repo = reader_repo(self, {
            "wide": 'fn w() { for e in std::fs::read_dir(root().join("templates")) {} }\n' + TESTFN,
            "one": 'fn w() { let _ = root().join("templates").join("talky"); }\n' + TESTFN,
            "ex": 'fn w() { for e in std::fs::read_dir(root().join("examples")) {} }\n' + TESTFN})
        for d in ("templates/talky", "examples/organism"):
            (pathlib.Path(repo) / d).mkdir(parents=True)
        self.addCleanup(gp._SOURCE_CACHE.clear)
        got = gp._tests_reading_at_runtime(repo, "templates/README.md")
        self.assertEqual({"binary_id(=meclaw-cells::wide)": "rule:8 runtime catalogue "
                          "file:templates/README.md"}, got)
        self.assertEqual(gp._tests_reading_at_runtime(repo, "examples/README.md"), {})
        self.assertEqual(gp._tests_reading_at_runtime(repo, "templates/talky/system.md"), {})

    def test_prose_and_a_walk_in_any_spelling_read_nothing(self):
        """V2-R1-1 (replaces re-check tb-s2 m-N9): prose next to a walk spelled
        as glob, format!, concat!(env!) or a constant makes no literal reader;
        the path named in code does."""
        path = "docs/sub/page.md"
        prose = "//! see %s\n" % path
        walks = {
            "glob": 'fn w() { let _ = glob::glob("docs/**/*.md"); }\n',
            "fmt_sub": 'fn w(x: &str) { let _ = std::fs::read_dir(format!("docs/sub/{}", x)); }\n',
            "fmt_lead": 'fn w(r: &str, x: &str) { let _ = std::fs::read(format!("{}/docs/{}", r, x)); }\n',
            "fmt_named": 'fn w(r: &str) { let _ = std::fs::read_dir(format!("{r}/docs")); }\n',
            "concat": 'const R: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs");\n',
            "const": 'const D: &str = "docs";\nfn w() { let _ = std::fs::read_dir(root().join(D)); }\n',
            "trailing_slash": 'const D: &str = "docs/";\n',
        }
        controls = {
            "file": 'fn w() { let _ = std::fs::read("docs/other.md"); }\n',
            "other_dir": 'fn w() { let _ = glob::glob("scripts/**/*.py"); }\n',
            "prose_only": "",
            "prose_walk": '// a walk in prose: glob::glob("docs/**/*.md")\n',
        }
        code = {"named": 'fn w() { let _ = std::fs::read_to_string("%s"); }\n' % path}
        sources = {n: prose + t + TESTFN for n, t in {**walks, **controls, **code}.items()}
        self.assertEqual(self.readers(sources, path), ["binary_id(=meclaw-cells::named)"])

    def test_the_real_gh617_and_gh623_read_no_resolver(self):
        """The measured case: neither reads `scripts/gate_plan.py`."""
        gp._SOURCE_CACHE.clear()
        self.addCleanup(gp._SOURCE_CACHE.clear)
        got = gp._tests_reading_path(str(REPO), "scripts/gate_plan.py")
        # gh623 moved with the binary to crates/meclaw/tests/misc (GH #1168),
        # so the harness that could read it whole is meclaw::misc now as well.
        self.assertFalse([t for t in got if "gh617" in t or "gh623" in t
                          or t in ("binary_id(=meclaw-cli::misc/main)",
                                   "binary_id(=meclaw::misc/main)")], got)

    # The one real reader (`const RESOLVER`) is a private-tree test: the
    # export keeps it back because it names paths that never ship, so in the
    # published tree there is no reader to find and this half failed there
    # (export audit R15, exit 1). The marker is `plans/`, not the file
    # itself: in the private tree a deleted reader must still go red.
    @private_tree.private()
    def test_the_real_resolver_reader_still_reads_it(self):
        gp._SOURCE_CACHE.clear()
        self.addCleanup(gp._SOURCE_CACHE.clear)
        got = gp._tests_reading_path(str(REPO), "scripts/gate_plan.py")
        self.assertIn("binary_id(=meclaw-cells::gh466_the_seed_gate_is_wired)", got)


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


class SlowTestsAreANoteInEveryMode(ExecutorCase):
    """TV-K5 (a), ruling K6 2026-10-11, ADR-0050: `slow-tests` names a test over
    a third of its nextest budget -- a time, and a time moves with the host's
    load. It is a perf signal and a finding of the wave, never a verdict on the
    commit: NOTE in every mode. A broken lock (exit 2: no JUnit file, a filter
    it cannot read, a debt without an issue) is a form error: RED in every mode."""

    def end(self, mode, rc, verdict):
        st = ex_station("slow-tests", 'echo "SLOW-VERDICT %s"; exit %d' % (verdict, rc))
        rc_run, evs = self.run_exec(self.dispatch([st], mode=mode,
                                                  ws=str(self.root / "ws" / mode / str(rc))))
        return rc_run, self.ended(evs)["slow-tests"]

    def test_a_slow_test_is_a_note_in_every_mode(self):
        for mode in ("strand", "integration", "release"):
            with self.subTest(mode=mode):
                rc, end = self.end(mode, 1, "2 over 1/3, none owed")
                self.assertEqual("NOTE", end["verdict"], end)
                self.assertTrue(end["reason"].startswith("2 over 1/3, none owed; "), end)
                self.assertIn("a finding of the wave", end["reason"])
                self.assertEqual(0, rc)
        rows = [ln for ln in gp.__doc__.splitlines() if ln.startswith("    slow-tests ")]
        self.assertEqual(1, len(rows))
        block = gp.__doc__.split(rows[0], 1)[1].split("\n    doctests ", 1)[0]
        self.assertIn("NOTE in every mode", " ".join((rows[0] + block).split()))

    def test_a_slow_debt_without_an_issue_stays_red_in_every_mode(self):
        for mode in ("strand", "integration", "release"):
            with self.subTest(mode=mode):
                rc, end = self.end(mode, 2, "broken: no JUnit file")
                self.assertEqual("RED", end["verdict"], end)
                self.assertTrue(end["reason"].startswith("broken: no JUnit file"), end)
                self.assertEqual(1, rc)


class TokenLexerReadsUnicodeIdentifiers(unittest.TestCase):
    """tb-s10 A1 point 20 (re-check tb-s4-st-h2 L-b, tb-s4-st-pin L-3): Rust
    takes Unicode identifiers (XID_Start/XID_Continue), the token lexer and
    `_IDENT_RE` took ASCII only and doubted every file with one -- valid Rust
    read as "unknown character". The doubt pins hung on that gap; they use a
    doubt by design now (a comment left open), and a character no identifier
    holds stays doubt."""

    def test_a_non_ascii_identifier_is_one_token(self):
        toks = gp.crate_root.tokens("fn h\u00ebper<'\u00e4>() { let \u03c0 = 1; }\n")
        self.assertIn("h\u00ebper", toks)
        self.assertIn("'\u00e4", toks)
        self.assertIn("\u03c0", toks)

    def test_a_test_with_a_non_ascii_name_is_counted(self):
        self.assertEqual((2, 1), gp._test_counts(
            "#[test]\nfn dr\u00e4ws() {}\n#[test]\n#[ignore]\nfn \u03c0() {}\n"))

    def test_a_non_ascii_item_macro_is_a_hidden_shape(self):
        # `_IDENT_RE` in `_hidden_tests`: `gen_\u03c0!();` may make tests
        self.assertTrue(gp._hidden_tests(gp._rust_code("gen_\u03c0!();\n")))
        self.assertFalse(gp._runs_no_test(LEXER_IGNORED + "gen_\u03c0!();\n"))

    def test_a_character_no_identifier_holds_stays_doubt(self):
        self.assertIsNone(gp._rust_code("fn a() { \u20ac }\n"))
        # `\u00b2` is alphanumeric to Python's `\w`, no XID_Continue to Rust
        self.assertIsNone(gp._rust_code("fn a\u00b2() {}\n"))
        self.assertIsNone(gp._rust_code("fn a<'b\u00b2>() {}\n"))


class TestBansInventoryIsAPassStation(ExecutorCase):
    """VORGABEN § 5.4 (tv-p0): the testing-bans inventory counts the whole tree
    in a pass -- a NOTE with the count, RED when a ban's count rose over the
    baseline. A strand's added lines are graded in `precheck`, never here."""

    def end(self, mode, rc):
        st = ex_station("test-bans-inventory",
                        'echo "TEST-BANS-INVENTORY 7 statements"; exit %d' % rc)
        rc_run, evs = self.run_exec(self.dispatch([st], mode=mode,
                                                  ws=str(self.root / "ws" / mode / str(rc))))
        return rc_run, self.ended(evs)["test-bans-inventory"]

    def test_the_count_is_a_note_and_a_rise_is_red(self):
        rc, end = self.end("integration", 0)
        self.assertEqual(("NOTE", "7 statements"), (end["verdict"], end["reason"]), end)
        self.assertEqual(0, rc)
        rc, end = self.end("integration", 1)
        self.assertEqual("RED", end["verdict"], end)

    def test_it_is_planned_in_the_passes_only(self):
        for mode, planned in (("strand", False), ("integration", True), ("release", True)):
            with self.subTest(mode=mode):
                st = by_name(gp.plan(["scripts/gate_plan.py"], mode, repo=None))
                self.assertEqual(planned, "test-bans-inventory" in st)


def _load_private(rel, name):
    """A module of plans/ in this tree, loaded by path (the export tree has none)."""
    import importlib.util
    spec = importlib.util.spec_from_file_location(name, REPO / rel)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


class ExportSelftestSelection(unittest.TestCase):
    """V2-A13 (GH #1199): every path the export self-tests read from the real
    tree plans `export-selftest` -- in the strand gate and in the merge gate,
    which plans with the same `gate_plan.plan` from its `gate_base` (the strand
    tip when the strand receipt covers it, master otherwise).

    The self-tests read far beyond scripts/: real-file pins, the MUST tests,
    `make_export.build_index("HEAD")` over ROOT_FILES, DOCS_MAP, the public
    templates and R16. So the rule is by construction: every path outside
    `make_export.FORBIDDEN_PREFIX` (what never ships) plans the station, plus
    the private inputs the self-tests load (the fixtures, `german_scan.py`, the
    seed generator of R16).

    v2-fix-e added `scripts/pipeline/progress.py` and its test without a
    `ROOT_FILES` row; strand gate and merge gate were green, and master
    e0b24bdc4 was red in `test_drift_gate.py`. Its diff touched nothing under
    `plans/export-fixtures/`, the only path that planned the station.
    """

    # `git diff --name-only --no-renames e0b24bdc4^1 e0b24bdc4`, the merge of v2-fix-e.
    V2_FIX_E = [
        "CHANGELOG.md", "plans/PREAMBLE.md",
        "plans/welle-validierung-2-2026-10-14/RESTLISTE.md",
        "plans/welle-validierung-2-2026-10-14/berichte/v2-fix-e.md",
        "plans/welle-validierung-2-2026-10-14/straenge/v2-fix-e.json",
        "scripts/flake_triage.py", "scripts/gate_plan.py", "scripts/pipeline/coverage.py",
        "scripts/pipeline/gate.py", "scripts/pipeline/lane.py", "scripts/pipeline/progress.py",
        "scripts/pipeline/schedule.py", "scripts/pipeline/selftest_durations.json",
        "scripts/strand.sh", "scripts/tests/test_gate_plan.py",
        "scripts/tests/test_pipeline_progress.py", "scripts/tests/test_pipeline_schedule.py",
        "scripts/tests/test_strand_sh.py",
    ]

    def planned(self, paths, mode="strand"):
        return by_name(gp.plan(paths, mode, repo=None))

    # GH #1199: a shipping path plans the station through `_export_rules`,
    # i.e. `make_export.py`, which never travels -- private tree only.
    @private_tree.private(gp.MAKE_EXPORT)
    def test_the_v2_fix_e_diff_plans_the_export_selftest(self):
        self.assertIn("export-selftest", self.planned(self.V2_FIX_E))

    @private_tree.private(gp.MAKE_EXPORT)
    def test_a_new_scripts_file_alone_plans_the_drift_fixtures(self):
        """A new module and its test, nothing else: the ROOT_FILES scan of
        `test_drift_gate.py` is what turns a missing row red."""
        st = self.planned(["scripts/pipeline/brand_new.py", "scripts/tests/test_brand_new.py"])
        self.assertIn("export-selftest", st)
        self.assertEqual(st["export-selftest"].cmds[0],
                         ["python3", "plans/export-fixtures/test_drift_gate.py"])

    @private_tree.private(gp.MAKE_EXPORT)
    def test_a_changed_import_in_a_kept_scripts_file_plans_it_too(self):
        """A new `import` in an exported file widens the closure the scan
        checks; the plan sees paths, not statuses, so every scripts/ path counts."""
        for path in ("scripts/pipeline/gate.py", "scripts/precheck.py",
                     "scripts/tests/test_strand_sh.py"):
            self.assertIn("export-selftest", self.planned([path]), path)

    def test_the_string_scan_the_self_test_loads_plans_it(self):
        self.assertIn("export-selftest",
                      self.planned(["plans/p5.5-fixtures/german_scan.py"]))

    @private_tree.private("plans/p5.5-fixtures/german_scan.py", "plans/export-fixtures/make_export.py")
    def test_a_moved_exemption_path_plans_it(self):
        """`test_drift_gate.py` checks that each declared string exemption
        names one tracked file (harness-free). A rename or deletion of that
        file lists its old path in the diff; a twin in another harness lists
        a new path with the same fold key."""
        gs = _load_private("plans/p5.5-fixtures/german_scan.py", "german_scan_pin")
        key = next(p for p in sorted(p for p, _ in gs.DECLARED_STRING_EXEMPTIONS)
                   if gs.fold_key(p) != p)  # one inside a test harness
        parts = key.split("/")
        twin = "/".join(parts[:-2] + ["zz_other_harness", parts[-1]])
        for path in (key, twin):
            self.assertEqual(gs.fold_key(path), gs.fold_key(key))
            self.assertIn("export-selftest", self.planned([path]), path)

    # The two cases the re-check measured on 7b0684ff6: a rename of a file the
    # drift fixtures pin by path (`FileNotFoundError` in `test_drift_gate.py`)
    # and a rename of a DOCS_MAP source (`build_index` raises in
    # `test_export_tree_tests.py`). Strand plan without the station, station red.
    GH230 = "crates/meclaw-colony/tests/gh_1/gh230_corpus_constraint_claims.rs"

    @private_tree.private("plans/export-fixtures/make_export.py")
    def test_a_renamed_pinned_crate_test_plans_it(self):
        """Pin 1 of I-1: `git mv` lists the old and the new path."""
        for path in (self.GH230, "crates/meclaw-colony/tests/gh_1/gh230_renamed_probe.rs"):
            self.assertIn("export-selftest", self.planned([path]), path)

    @private_tree.private("plans/export-fixtures/make_export.py")
    def test_a_renamed_docs_map_source_plans_it(self):
        """Pin 2 of I-1: a DOCS_MAP source `build_index` resolves in HEAD."""
        for path in ("docs/costs.en.md", "docs/costs-renamed.en.md"):
            self.assertIn("export-selftest", self.planned([path]), path)

    @private_tree.private("plans/export-fixtures/make_export.py")
    def test_every_shipping_root_plans_it(self):
        """One path per kind of input: ROOT_FILES outside scripts/ (.github,
        .config, .cargo, root files), the TREE_DIRS, a public template, the
        librarian seed of R16, and a path nothing lists yet."""
        for path in (".github/workflows/ci.yml", ".config/nextest.toml", ".cargo/config.toml",
                     "Cargo.toml", "README.md", "templates/README.md",
                     "crates/meclaw-cells/tests/misc_7/zz_unrelated_test.rs",
                     "tests/zz_probe.rs", "examples/zz_probe.rs",
                     "templates/builder-librarian/store/seed/docs.jsonl",
                     "zz_new_root_file.txt"):
            self.assertIn("export-selftest", self.planned([path]), path)

    @private_tree.private("plans/export-fixtures/make_export.py")
    def test_the_seed_generator_of_r16_plans_it(self):
        """`build_index` reads R16 from `workshop/tools/build_librarian_seed.py`,
        a private path under FORBIDDEN_PREFIX."""
        self.assertIn("export-selftest",
                      self.planned(["workshop/tools/build_librarian_seed.py"]))

    @private_tree.private("plans/export-fixtures/make_export.py")
    def test_the_never_shipping_prefixes_come_from_make_export(self):
        """One list: the planner reads FORBIDDEN_PREFIX from `make_export.py`
        of its own tree, it does not copy it."""
        mx = _load_private("plans/export-fixtures/make_export.py", "make_export_pin")
        rules = gp._export_rules()
        self.assertIsNotNone(rules)
        self.assertEqual(mx.FORBIDDEN_PREFIX.pattern, rules[0].pattern)
        self.assertEqual("workshop/tools/build_librarian_seed.py", rules[1])

    def test_a_never_shipping_path_does_not_plan_it(self):
        """Counter-pin: plans/, ideas/, archive/ and workshop/ never ship and
        none of these is read by the self-tests."""
        for path in ("plans/welle-validierung-2-2026-10-14/berichte/zz_probe.md",
                     "ideas/zz_probe.md", "archive/zz_probe.md",
                     "workshop/evals/persona/zz_probe.md"):
            self.assertNotIn("export-selftest", self.planned([path]), path)

    def test_ci_still_drops_it(self):
        """The published tree carries no plans/: ci plans what it holds."""
        self.assertNotIn("export-selftest", self.planned(self.V2_FIX_E, mode="ci"))


class KitExportTreeSelection(unittest.TestCase):
    """GH #1199 (release 0.64.2, run 4): four kit self-tests of v2-fix-drift
    read `make_export.py` through the resolver and were red in the export tree
    only. The strand gate never ran the kit stations there -- `export-tree-tests`
    was release-only -- so the class reached the release. A diff at a kit input
    (`gate_infra`: the kit and its tests; `make_export.py`: what travels) now
    plans the station's cargo-free kit half in S, over the gated tree."""

    KIT = ["python3", "plans/export-fixtures/export_tree_tests.py", "--worktree", "--kit"]

    def test_the_strand_station_judges_the_gated_tree_not_head(self):
        """RC-KX I1: the strand gate gates the working tree before the commit
        and the cache keys the station on that tree; `--rev HEAD` judged the
        base without the diff and the cache took the GREEN over. No command of
        the station in any mode names a revision."""
        for mode in ("strand", "release"):
            st = by_name(gp.plan(["scripts/tests/test_zz_probe.py"], mode, repo=None))
            for cmd in st["export-tree-tests"].cmds:
                self.assertIn("--worktree", cmd, mode)
                self.assertNotIn("--rev", cmd, mode)

    def test_a_gate_infra_or_export_rules_diff_plans_it_too(self):
        """RC-KX class probe: a diff at `scripts/gate_plan.py` alone turned two
        unchanged kit tests red in the export tree only."""
        for path in ("scripts/gate_plan.py", "scripts/pipeline/cache.py", "scripts/strand.sh",
                     ".config/nextest.toml", gp.MAKE_EXPORT):
            st = by_name(gp.plan([path], "strand", repo=None))
            self.assertIn("export-tree-tests", st, path)
            self.assertEqual(st["export-tree-tests"].cmds, [self.KIT], path)

    def test_integration_and_ci_do_not_plan_it(self):
        """OR-V2.v2-fix-kitexp.1: the integration pass follows merged strands
        that ran the station in S and at the merge; release runs it in full;
        the public CI IS the export tree."""
        paths = ["scripts/tests/test_zz_probe.py", "scripts/gate_plan.py", gp.MAKE_EXPORT]
        for mode in ("integration", "ci"):
            self.assertNotIn("export-tree-tests", by_name(gp.plan(paths, mode, repo=None)), mode)

    def test_a_kit_test_diff_plans_the_kit_half_in_the_export_tree(self):
        st = by_name(gp.plan(["scripts/tests/test_zz_probe.py"], "strand", repo=None))
        self.assertIn("export-tree-tests", st)
        self.assertEqual(st["export-tree-tests"].cmds, [self.KIT])
        self.assertFalse(st["export-tree-tests"].cargo)

    def test_a_diff_without_a_kit_input_does_not_plan_it(self):
        for path in ("docs/x.md", "templates/x/config.json", "scripts/check_adr_anchors.py",
                     "plans/export-fixtures/test_drift_gate.py"):
            self.assertNotIn("export-tree-tests",
                             by_name(gp.plan([path], "strand", repo=None)), path)

    def test_the_reason_names_the_kit_test(self):
        """`--explain`: the station names the path that planned it."""
        out = gp.to_json(["scripts/tests/test_zz_probe.py"], "strand", None, explain=True)
        rows = {r["name"]: r for r in out["stations"]}
        self.assertEqual(rows["export-tree-tests"]["reason"],
                         ["gate_infra: scripts/tests/test_zz_probe.py"])
        out = gp.to_json([gp.MAKE_EXPORT], "strand", None, explain=True)
        rows = {r["name"]: r for r in out["stations"]}
        self.assertEqual(rows["export-tree-tests"]["reason"], ["make_export: " + gp.MAKE_EXPORT])



# The trigger is `make_export._is_exported_test` (loaded, never copied), and
# `make_export.py` does not travel -- private tree only (GH #1199).
@private_tree.private(gp.MAKE_EXPORT)
class StrandExportAuditSelection(unittest.TestCase):
    """V2-A18 (GH #1199, release 0.64.2 run 5): two g855 tests with a bare
    `.join("../../templates").join(name)` reached master through strand and
    merge gates, and R2b first saw them in the release -- `export-audit` ran
    in I/R only. A diff at what R2b/R2c/R2e judge (exported test sources,
    `templates/**`) or at the rules themselves (`make_export.py`) now plans
    the DRY audit in S, over the gated tree, without the rules that read an
    I/R receipt (R11/R12/R14/R17 -- a strand gate has none)."""

    AUDIT = ["python3", "plans/export-fixtures/make_export.py", "--keep-going",
             "--skip-cargo", "--worktree", "--no-gate-receipt", "--parent", "HEAD"]
    TRIGGERS = ("crates/meclaw-cells/tests/misc_2/gh1199_a_row_is_replaced_in_one_step.rs",
                "crates/meclaw-cells/tests/common/mod.rs",
                "crates/meclaw-core/src/blob_test.rs",
                "templates/operator/template.json",
                "templates/README.md",
                gp.MAKE_EXPORT)
    QUIET = ("scripts/gate_plan.py", "scripts/tests/test_zz_probe.py", "docs/x.md",
             "docs/x.en.md", "README.md", "crates/meclaw-cells/src/lib.rs",
             "plans/export-fixtures/test_drift_gate.py")

    def test_a_test_or_template_diff_plans_the_dry_audit_in_strand(self):
        for path in self.TRIGGERS:
            st = by_name(gp.plan([path], "strand", repo=None))
            self.assertIn("export-audit", st, path)
            # GH #1199 (v2-r3-mergegate): narrowed to the diff's exported tests
            # (test_export_audit_scope); a template judges no test file.
            judged = [] if path.startswith("templates/") else [path]
            # re-check I1: the resolver module drops out only beside the kit
            # station that runs it (make_export.py plans it, a test does not)
            kit = ["--no-public-resolver"] if "export-tree-tests" in st else []
            self.assertEqual(path == gp.MAKE_EXPORT, bool(kit), path)
            self.assertEqual(st["export-audit"].cmds,
                             [self.AUDIT + kit + ["--diff-paths"] + judged],
                             path)
            self.assertFalse(st["export-audit"].cargo, path)

    def test_a_scripts_or_docs_only_diff_does_not_plan_it(self):
        for path in self.QUIET:
            self.assertNotIn("export-audit", by_name(gp.plan([path], "strand", repo=None)),
                             path)

    def test_the_strand_audit_reads_no_gate_receipt(self):
        """Without an I/R receipt R11/R12/R14/R17 are red on every strand
        (v2-fix-r2b, standalone dry run): the switch drops exactly them, and
        no `{receipt}` placeholder asks the runner for one."""
        cmd = by_name(gp.plan([self.TRIGGERS[0]], "strand", repo=None))["export-audit"].cmds[0]
        self.assertIn("--no-gate-receipt", cmd)
        self.assertNotIn("{receipt}", cmd)
        self.assertNotIn("--gate-receipt", cmd)

    def test_the_strand_audit_judges_the_gated_tree_not_head(self):
        """RC-KX I1: the strand gate gates the working tree before the commit;
        `--rev HEAD` would judge the base without the diff."""
        cmd = by_name(gp.plan([self.TRIGGERS[0]], "strand", repo=None))["export-audit"].cmds[0]
        self.assertIn("--worktree", cmd)
        self.assertNotIn("--rev", cmd)

    def test_the_strand_audit_needs_no_github_main(self):
        """RC-XA I1: under the default `--parent github-main` make_export
        compares the local ref with `ls-remote origin main` and aborts on drift.
        That ref is moved by hand after an export, so a stale or missing one
        turned every strand gate with a test diff red, unrelated to the diff.
        `--keep-going` builds no commit and never reads the parent: HEAD."""
        cmd = by_name(gp.plan([self.TRIGGERS[0]], "strand", repo=None))["export-audit"].cmds[0]
        self.assertIn("--parent", cmd)
        self.assertEqual(cmd[cmd.index("--parent") + 1], "HEAD")

    def test_the_trigger_is_the_scope_of_the_export_rules(self):
        """One list, loaded: R2b/R2c judge `make_export._is_exported_test`."""
        mx = gp._export_module()
        for path in self.TRIGGERS[:3] + ("crates/meclaw-cells/src/lib.rs",
                                         "scripts/tests/test_zz_probe.py"):
            self.assertEqual(gp._feeds_the_strand_audit(path), mx._is_exported_test(path), path)

    def test_integration_and_release_keep_their_shapes(self):
        for mode, scope in (("integration", "R1-R17 dry"), ("release", "R1-R17")):
            st = by_name(gp.plan([self.TRIGGERS[0]], mode, repo=None))
            self.assertEqual(st["export-audit"].scope, scope, mode)
            self.assertIn("{receipt}", st["export-audit"].cmds[0], mode)
            self.assertNotIn("--no-gate-receipt", st["export-audit"].cmds[0], mode)
            # The parent check against origin/main stays where it is wanted.
            self.assertNotIn("--parent", st["export-audit"].cmds[0], mode)

    def test_ci_never_plans_it(self):
        self.assertNotIn("export-audit", by_name(gp.plan(list(self.TRIGGERS), "ci", repo=None)))

    def test_the_reason_names_the_path(self):
        out = gp.to_json([self.TRIGGERS[0]], "strand", None, explain=True)
        rows = {r["name"]: r for r in out["stations"]}
        self.assertEqual(rows["export-audit"]["reason"], ["export_test: " + self.TRIGGERS[0]])
        out = gp.to_json(["templates/operator/template.json"], "strand", None, explain=True)
        rows = {r["name"]: r for r in out["stations"]}
        self.assertEqual(rows["export-audit"]["reason"],
                         ["template: templates/operator/template.json"])

    def test_a_file_directly_under_templates_names_itself_in_the_reason(self):
        """RC-XA M3: `template` starts at three segments, so a file directly
        under `templates/` (not README.md) planned the station with an empty
        reason."""
        path = "templates/x.json"
        out = gp.to_json([path], "strand", None, explain=True)
        rows = {r["name"]: r for r in out["stations"]}
        self.assertEqual(rows["export-audit"]["reason"], ["template_root: " + path])


if __name__ == "__main__":
    unittest.main()
