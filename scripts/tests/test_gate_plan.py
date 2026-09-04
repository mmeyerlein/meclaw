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
    """A throw-away tree with the four test sources the grep rules need."""
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
            {"roadmap-anchors", "adr-anchors", "claims", "tree-rules", "corpus"})
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
            "binary_id(=meclaw-cells::reads_the_doc) - (%s)" % gp.SCENARIO)
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
        self.assertEqual(st["tests"].scope, "rdeps(meclaw-colony) - (%s)" % gp.SCENARIO)
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
            "binary_id(=meclaw-cells::gh3_llm) + binary_id(=meclaw-cells::mock_openai)"
            " - (%s)" % gp.SCENARIO)

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
            "binary_id(=meclaw-cells::gh2_all) - (%s)" % gp.SCENARIO)

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
            "binary_id(=meclaw-cells::suite) - (%s)" % gp.SCENARIO)

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
            "binary_id(=meclaw-cells::gh1_uses_hive) + binary_id(=meclaw-cells::gh2_all)"
            " - (%s)" % gp.SCENARIO)
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
            "binary_id(=meclaw-cells::gh2_all) - (%s)" % gp.SCENARIO)

    def test_example_diff_selects_referencing_tests(self):
        repo = mini_repo(self)
        st = by_name(gp.plan(["examples/organism/grow-1.json"], "strand", repo=repo))
        self.assertEqual(
            st["tests"].scope,
            "binary_id(=meclaw-cells::gh4_example) - (%s)" % gp.SCENARIO)
        self.assertIn("scenarios:builder", st)

    def test_workspace_class_runs_all(self):
        st = by_name(gp.plan(["Cargo.lock"], "strand", repo=None))
        self.assertEqual(st["tests"].scope, "all() - (%s)" % gp.SCENARIO)
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

    def test_integration_always_runs_scenarios_doctests_deny(self):
        st = by_name(gp.plan(["crates/meclaw-core/src/lib.rs"], "integration", repo=None))
        for name in ("scenarios:memory", "scenarios:builder", "recall-harness",
                     "doctests", "deny", "catalogue"):
            self.assertIn(name, st)
        self.assertIn("--workspace", st["clippy"].cmds[0])

    def test_release_ends_with_export_audit_and_has_advisories(self):
        st = gp.plan(["docs/x.md"], "release", repo=None)
        self.assertEqual(st[-1].name, "export-audit")
        self.assertIn("{receipt}", st[-1].cmds[0])
        self.assertIn("deny-advisories", by_name(st))

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
        st = by_name(gp.plan(["docs/x.md"], "integration", repo=None))
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
                         [["python3", "recall_cases.py"]])

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

    def test_gate_selftest_runs_resolver_and_runner_tests(self):
        """One command, both modules -- the runner's tests arrive with it."""
        st = by_name(gp.plan(["scripts/gate_plan.py"], "strand", repo=None))
        self.assertEqual(st["gate-selftest"].cmds, [[
            "python3", "-m", "unittest",
            "scripts.tests.test_gate_plan", "scripts.tests.test_gate_sh"]])

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
        r = self.run_cli("--mode", "integration", "--files",
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
        self.assertEqual(r.stdout.strip(), gp.SCENARIO)

    def test_print_ignored(self):
        """`scripts/gate.sh` subtracts these before it calls a tree dirty."""
        r = self.run_cli("--print", "ignored")
        self.assertEqual(r.stdout.split(), list(gp.IGNORED))


if __name__ == "__main__":
    unittest.main()
