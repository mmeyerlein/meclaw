"""Unit tests for the gate-station resolver (`scripts/gate_plan.py`).

The resolver is the ONE place that decides which gate station runs for which
diff, so every rule in its table gets a test here. Fixtures are path lists;
the rules that grep test sources build a tiny throw-away repo instead of
reading the real tree, so the expectations stay stable.
"""

import json
import pathlib
import re
import subprocess
import sys
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
SCRIPTS = HERE.parent
REPO = SCRIPTS.parent
sys.path.insert(0, str(SCRIPTS))

import gate_plan as gp  # noqa: E402


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


class Classify(unittest.TestCase):
    def test_docs_only_has_no_cargo_station(self):
        """... but only while no test READS the file (rule 8)."""
        repo = reader_repo(self, {"unrelated": 'fn main() { let _ = 1; }\n'})
        st = gp.plan(["docs/memory.en.md", "README.md"], "strand", repo=repo)
        self.assertEqual(
            {s.name for s in st},
            {"precheck", "roadmap-anchors", "adr-anchors", "claims", "tree-rules",
             "corpus-committed", "corpus"})
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
        self.assertEqual(st["tests"].scope,
                         "rdeps(meclaw-colony) - (%s)" % gp.scenario_for(None))
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
        self.assertEqual(
            st["scenarios:display"].cmds,
            [["python3", "templates/display/compose/scenarios/"
              "run_display_scenarios.py"],
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
                 "workshop/tools/display-lab/fact_yield.py"]
        for path in paths:
            with self.subTest(path=path):
                self.assertIn("evals_guide", gp.classify([path]))
        self.assertNotIn("evals_guide", gp.classify(["workshop/tools/judge_eval.py"]))
        # A file may carry two classes: the fact counter is the library's too.
        self.assertIn("display_lab",
                      gp.classify(["workshop/tools/display-lab/fact_yield.py"]))

        cmd = [["python3", "workshop/evals/conversation-guide/run_guide.py", "--self-test"]]
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
        self.assertEqual(("scripts/*.sh", ".github/gates/*.sh"), gp.SHELL_GLOBS[:2])

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

        # A diff on the driver alone plans it in a strand, and it builds.
        st = by_name(gp.plan([driver], "strand", repo=None))
        self.assertIn("browser:display", st)
        self.assertTrue(st["browser:display"].cargo)
        self.assertEqual(st["browser:display"].scope, "sheet")
        self.assertEqual(
            st["browser:display"].cmds,
            [["scripts/test-tier.sh", "filter", "binary(/710_the_sheet_holds/)"],
             gp.CSP_LOCK_CMD, gp.MOTION_LOCK_CMD, gp.SLOW3G_LOCK_CMD,
             gp.ORIENTATION_LOCK_CMD, gp.KEEP_LOCK_CMD])

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
        both = [["scripts/test-tier.sh", "filter", "binary(/710_the_sheet_holds/)"],
                ["scripts/test-tier.sh", "filter", "binary(/710_the_colony_holds/)",
                 "--run-ignored", "all"],
                gp.CSP_LOCK_CMD, gp.MOTION_LOCK_CMD, gp.SLOW3G_LOCK_CMD,
                gp.ORIENTATION_LOCK_CMD, gp.KEEP_LOCK_CMD]
        for mode in ("integration", "release"):
            st_ir = by_name(gp.plan(["docs/x.md"], mode, repo=None))
            self.assertEqual(st_ir["browser:display"].scope, "sheet+colony", mode)
            self.assertEqual(st_ir["browser:display"].cmds, both, mode)

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
        self.assertEqual(gp.CSP_LOCK_CMD[-2:], ["--run-ignored", "all"])
        self.assertIn("gh867_a_display_runs_under_a_strict_csp", gp.CSP_LOCK_CMD[2])
        for diff, mode in (([driver], "strand"), ([lock], "strand"),
                           (["templates/display/csp.json"], "strand"),
                           (["docs/x.md"], "integration"), (["docs/x.md"], "release")):
            with self.subTest(diff=diff, mode=mode):
                st = by_name(gp.plan(diff, mode, repo=None))
                self.assertIn(gp.CSP_LOCK_CMD, st["browser:display"].cmds)
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
        self.assertEqual(gp.SLOW3G_LOCK_CMD[-2:], ["--run-ignored", "all"])
        self.assertIn("gh1002_slow_3g_connects", gp.SLOW3G_LOCK_CMD[2])
        for diff, mode in (([driver], "strand"), ([lock], "strand"),
                           (["docs/x.md"], "integration"), (["docs/x.md"], "release")):
            with self.subTest(diff=diff, mode=mode):
                st = by_name(gp.plan(diff, mode, repo=None))
                self.assertIn(gp.SLOW3G_LOCK_CMD, st["browser:display"].cmds)
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
        self.assertEqual(gp.ORIENTATION_LOCK_CMD[-2:], ["--run-ignored", "all"])
        self.assertIn("gh1003_the_shell_marks_orientation", gp.ORIENTATION_LOCK_CMD[2])
        for diff, mode in (([driver], "strand"), ([lock], "strand"),
                           (["docs/x.md"], "integration"), (["docs/x.md"], "release")):
            with self.subTest(diff=diff, mode=mode):
                st = by_name(gp.plan(diff, mode, repo=None))
                self.assertIn(gp.ORIENTATION_LOCK_CMD, st["browser:display"].cmds)
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
        self.assertEqual(gp.KEEP_LOCK_CMD[-2:], ["--run-ignored", "all"])
        for diff, mode in (([driver], "strand"), ([lock], "strand"),
                           (["docs/x.md"], "integration"), (["docs/x.md"], "release")):
            with self.subTest(diff=diff, mode=mode):
                st = by_name(gp.plan(diff, mode, repo=None))
                self.assertIn(gp.KEEP_LOCK_CMD, st["browser:display"].cmds)
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
        self.assertEqual(gp.MOTION_LOCK_CMD[-2:], ["--run-ignored", "all"])
        self.assertIn("gh961_motion_by_target_state", gp.MOTION_LOCK_CMD[2])
        for diff, mode in (([driver], "strand"), ([lock], "strand"),
                           (["templates/display/compose/display-dna.css"], "strand"),
                           (["docs/x.md"], "integration"), (["docs/x.md"], "release")):
            with self.subTest(diff=diff, mode=mode):
                st = by_name(gp.plan(diff, mode, repo=None))
                self.assertIn(gp.MOTION_LOCK_CMD, st["browser:display"].cmds)
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
        self.assertEqual(st["tests"].scope, "all() - (%s)" % gp.scenario_for(None))
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

    def test_release_ends_with_export_audit_and_has_advisories(self):
        st = gp.plan(["docs/x.md"], "release", repo=None)
        self.assertEqual(st[-1].name, "export-audit")
        self.assertIn("{receipt}", st[-1].cmds[0])
        self.assertNotIn("--skip-cargo", st[-1].cmds[0])
        self.assertEqual(st[-1].scope, "R1-R17")
        self.assertIn("deny-advisories", by_name(st))

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
            "python3", "-m", "unittest",
            "scripts.tests.test_gate_plan", "scripts.tests.test_gate_sh",
            "scripts.tests.test_strand_sh",
            "scripts.tests.test_wave_receipt",
            "scripts.tests.test_wave_retro",
            "scripts.tests.test_git_merge_display_sync",
            "scripts.tests.test_precheck",
            "scripts.tests.test_display_sync",
            "scripts.tests.test_nextest_quarantine",
            "scripts.tests.test_roadmap_anchors"]])

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
        for path in ("scripts/strand.sh", "scripts/wave_receipt.py"):
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
        """Rule 7: I/R reuse rules 1-5 unchanged -- only the always-stations differ."""
        repo = mini_repo(self)
        for paths in (["crates/meclaw-colony/src/route.rs"],
                      ["templates/memory-hive/store/config.json"],
                      ["crates/meclaw-cells/tests/mock_openai.rs"],
                      ["Cargo.lock"]):
            strand = gp.test_filter(paths, "strand", repo=repo)
            for mode in ("integration", "release"):
                self.assertEqual(gp.test_filter(paths, mode, repo=repo), strand, paths)

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
        if not gen.is_file():
            self.skipTest("generator not in this tree")
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
        if not (REPO / "workshop").is_dir():
            self.skipTest("published tree: workshop/ does not travel")
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
    '#[tokio::test(flavor = "multi_thread", worker_threads = 4)]\n'
    '#[ignore = "benchmark -- run explicitly"]\n'
    'async fn bench() {}\n')


class IgnoredOnlyBinariesAreNoTarget(unittest.TestCase):
    """GH #997: a binary whose every test is `#[ignore]` runs 0 tests.

    Rule 8 picked `gh850_bench_normal_path` for a diff to
    `scripts/test-tier.sh` (its doc comment names the path); its one test is a
    `#[ignore]` benchmark, so the `tests` station ran 0 tests -- nextest exit 4
    on master, `no tests matched` (exit 6) since K1. Such a binary is never the
    target of ANY plan rule; a comment naming the path is still a reference.
    """

    def test_an_ignored_only_reader_is_not_selected(self):
        repo = reader_repo(self, {"bench": IGNORED_ONLY})
        st = by_name(gp.plan(["scripts/test-tier.sh"], "strand", repo=repo))
        self.assertNotIn("tests", st)

    def test_a_reader_with_one_ignored_and_one_normal_test_stays(self):
        repo = reader_repo(self, {
            "bench": IGNORED_ONLY,
            "mixed": ('// reads scripts/test-tier.sh\n'
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
            "cfg_attr": ('// scripts/test-tier.sh\n'
                         '#[test]\n#[cfg_attr(not(feature = "x"), ignore)]\nfn t() {}\n'),
            "macro": ('// scripts/test-tier.sh\n#[test]\n#[ignore]\nfn a() {}\n'
                      'macro_rules! gen { () => { #[test] fn b() {} } }\ngen!();\n'),
            "module": ('// scripts/test-tier.sh\nmod more;\n'
                       '#[test]\n#[ignore]\nfn a() {}\n'),
            "commented_ignore": ('// scripts/test-tier.sh\n'
                                 '#[test]\n// #[ignore]\nfn a() {}\n'),
            "harness_false": 'fn main() { let _ = "scripts/test-tier.sh"; }\n',
        }
        repo = reader_repo(self, doubtful)
        self.assertEqual(
            gp.test_filter(["scripts/test-tier.sh"], "ci", repo=repo),
            " + ".join("binary_id(=meclaw-cells::%s)" % s for s in sorted(doubtful)))

    def test_the_counter_parser(self):
        self.assertEqual(gp._test_counts(IGNORED_ONLY), (1, 0))
        self.assertEqual(gp._test_counts(
            '#[test]\n#[ignore]\nfn a() {}\n#[tokio::test]\nasync fn b() {}\n'
            'const S: &str = "#[test]";\n/* #[test] fn c() {} */\n'), (2, 1))

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

    LOCK = ('// reads scripts/test-tier.sh\n'
            '#[path = "support/helper.rs"]\nmod helper;\n'
            '#[tokio::test]\n#[ignore = "browser lock"]\nasync fn lock() {}\n')

    def selected(self, binaries, modules):
        repo = module_repo(self, binaries, modules)
        return gp.test_filter(["scripts/test-tier.sh"], "ci", repo=repo)

    def test_a_path_module_without_tests_drops_the_lock(self):
        self.assertIsNone(self.selected(
            {"lock": self.LOCK}, {"support/helper.rs": "pub fn boot() {}\n"}))

    def test_plain_module_files_resolve_too(self):
        body = '// scripts/test-tier.sh\nmod %s;\n#[test]\n#[ignore]\nfn a() {}\n'
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
        body = ('// scripts/test-tier.sh\n#[test] #[ignore] fn slow(){}\n'
                'mod a { mod b; }\n')
        self.assertEqual(
            self.selected({"nest": body},
                          {"b.rs": "pub fn x() {}\n",
                           "a/b.rs": "#[test]\nfn runs() {}\n"}),
            "binary_id(=meclaw-cells::nest)")

    def test_braces_in_comments_and_literals_do_not_nest(self):
        """Depth is read on code without comments and literals."""
        body = ('// scripts/test-tier.sh { {\n/* { */ const S: &str = "{";\n'
                "const C: char = '{';\n#[test]\n#[ignore]\nfn a() {}\n"
                'mod flat_help;\n')
        self.assertIsNone(self.selected(
            {"flat": body}, {"flat_help.rs": "pub fn x() {}\n"}))

    def test_a_cfg_attr_path_keeps_the_binary(self):
        """`#[cfg_attr(unix, path = "y.rs")] mod x;` may load `y.rs` -- doubt."""
        body = ('// scripts/test-tier.sh\n#[test]\n#[ignore]\nfn a() {}\n'
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
        body = ('// scripts/test-tier.sh\n#[test]\n#[ignore]\nfn a() {}\n'
                'mod a {\n/* x /* y */ } */\nmod z;\n}\n')
        self.assertEqual(
            self.selected({"nested": body},
                          {"z.rs": "pub fn x() {}\n",
                           "a/z.rs": "#[test]\nfn runs() {}\n"}),
            "binary_id(=meclaw-cells::nested)")

    def test_only_a_driver_mjs_has_no_tests_station(self):
        """The measured case: gh961's driver alone ran 0 tests in `tests`."""
        for mjs in ("display-motion-browser.mjs", "display-csp-browser.mjs"):
            st = by_name(gp.plan(["workshop/tools/" + mjs], "strand"))
            self.assertNotIn("tests", st, mjs)
            self.assertIn("browser:display", st, mjs)

    def test_only_the_gh961_lock_has_browser_display_but_no_tests_station(self):
        st = by_name(gp.plan(
            ["crates/meclaw-cells/tests/gh961_motion_by_target_state_browser.rs"],
            "strand"))
        self.assertNotIn("tests", st)
        self.assertIn("browser:display", st)


class UnknownTestShapesAreDoubt(unittest.TestCase):
    """K1 fix round 2: a test form the counter does not know is no proof of 0.

    Any attribute whose last path segment contains `test` (`#[wasm_bindgen_test]`,
    `#[serial_test::test]`), and `quickcheck`/`proptest`, counts as a runnable
    test; a macro invocation `name! {` at any indentation is doubt.
    """

    IGNORED = '// scripts/test-tier.sh\n#[test]\n#[ignore]\nfn slow() {}\n'

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
        if not (REPO / "crates" / "meclaw-cells" / "tests" / "slack_live.rs").exists():
            self.skipTest("slack_live.rs not in this tree (public clone)")
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

if __name__ == "__main__":
    unittest.main()
