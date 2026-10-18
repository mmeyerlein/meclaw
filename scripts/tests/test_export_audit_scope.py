"""GH #1199 (v2-r3-mergegate, part 2): the strand gate's export audit judges its diff.

Profiled on 16.10. (strand form, `--keep-going --skip-cargo --worktree
--no-gate-receipt`): 162 s, of which R15 97 s -- almost all of it the
resolver's own test module (`scripts.tests.test_gate_plan`) in the public tree
-- and R2b/R2c/R2d 26 s over every exported test of the tree.

Two narrowings, strand only; integration and release keep the whole audit:
- R2b/R2c/R2d judge one file at a time against rules that live in
  `make_export.py`. The strand passes the exported tests of its diff
  (`--diff-paths`); a diff at the rules themselves judges the whole tree.
- R15's resolver module is what `export-tree-tests --kit` (gate-selftest in the
  export tree) runs on a kit diff; on any other diff the strand plans no kit
  station at all. `--no-public-resolver` drops it, visibly in the report.

The pins: a finding in a touched file stays red, an untouched file is not
read again, and the resolver module does not run under the switch.
"""

import inspect
import io
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parent.parent
sys.path.insert(0, str(REPO))

from scripts import gate_plan as gp  # noqa: E402
from scripts.tests import private_tree, store_isolation  # noqa: E402

MAKE_EXPORT = "plans/export-fixtures/make_export.py"

# One file, three defects of the per-file rules: a private template (R2b), a
# root that never travels (R2c), a docs/ file the export does not carry (R2d).
DEFECTS = (
    "fn tpl() {\n"
    '    let _ = std::fs::read("../../templates/zz-private/template.json");\n'
    "}\n"
    "fn work() {\n"
    '    let _dir = repo("workshop");\n'
    "}\n"
    "fn spec() -> String {\n"
    "    let doc_path = concat!(\n"
    '        env!("CARGO_MANIFEST_DIR"),\n'
    '        "/../../docs/zz-not-shipped.md"\n'
    "    );\n"
    '    std::fs::read_to_string(doc_path).expect("readable")\n'
    "}\n"
)
TOUCHED = "crates/zz-fixture/tests/touched_test.rs"
UNTOUCHED = "crates/zz-fixture/tests/untouched_test.rs"


def _load_make_export():
    import importlib.util
    spec = importlib.util.spec_from_file_location("make_export_scope", REPO / MAKE_EXPORT)
    mx = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mx)
    return mx


def _git(root, *args):
    return subprocess.run(
        ["git", "-c", "user.name=fixture", "-c", "user.email=fixture@example.invalid",
         "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null",
         "-c", "init.defaultBranch=master", *args],
        cwd=root, check=True, capture_output=True, text=True).stdout.strip()


@private_tree.private(MAKE_EXPORT)
class PerFileRulesJudgeTheDiff(unittest.TestCase):
    """A tree with two exported tests carrying the same three defects."""

    @classmethod
    def setUpClass(cls):
        cls.mx = _load_make_export()
        cls.tmp = tempfile.TemporaryDirectory(prefix="export-audit-scope-")
        root = cls.tmp.name
        _git(root, "init", "-q")
        for rel in (TOUCHED, UNTOUCHED):
            path = pathlib.Path(root, rel)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(DEFECTS, encoding="utf-8")
        _git(root, "add", "-A")
        cls.tree = _git(root, "write-tree")

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def judge(self, scope):
        """R2b/R2c/R2d over the fixture tree: (reds, report, paths read)."""
        mx = self.mx
        read = []
        real_git = mx.git

        def spy(*args, **kwargs):
            if args and args[0] == "show":
                read.append(args[1].split(":", 1)[1])
            return real_git(*args, **kwargs)

        with mock.patch.object(mx, "REPO", pathlib.Path(self.tmp.name)), \
                mock.patch.object(mx, "KEEP_GOING", True), \
                mock.patch.object(mx, "REDS", []), \
                mock.patch.object(mx, "SCOPE", scope), \
                mock.patch.object(mx, "git", spy):
            report = []
            mx.receipt_test_refs(self.tree, report)
            return list(mx.REDS), report, read

    def test_the_fixture_files_are_exported_tests(self):
        self.assertTrue(self.mx._is_exported_test(TOUCHED))
        self.assertTrue(self.mx._is_exported_test(UNTOUCHED))

    def test_the_whole_tree_finds_both_files_under_each_rule(self):
        reds, _, _ = self.judge(None)
        for rule in ("R2b", "R2c", "R2d"):
            rows = [r for r in reds if r.startswith(rule)]
            self.assertEqual(len(rows), 1, (rule, reds))
            self.assertIn(TOUCHED, rows[0], rule)
            self.assertIn(UNTOUCHED, rows[0], rule)

    def test_a_finding_in_a_touched_file_stays_red(self):
        reds, _, _ = self.judge({TOUCHED})
        for rule in ("R2b", "R2c", "R2d"):
            rows = [r for r in reds if r.startswith(rule)]
            self.assertEqual(len(rows), 1, (rule, reds))
            self.assertIn(TOUCHED, rows[0], rule)

    def test_an_untouched_file_is_neither_read_nor_judged(self):
        reds, _, read = self.judge({TOUCHED})
        self.assertNotIn(UNTOUCHED, read)
        self.assertIn(TOUCHED, read)
        self.assertFalse([r for r in reds if UNTOUCHED in r], reds)

    def test_the_report_says_the_rules_were_narrowed(self):
        _, report, _ = self.judge({TOUCHED})
        self.assertTrue([r for r in report if "--diff-paths" in r and "1 " in r], report)


@private_tree.private(MAKE_EXPORT)
class TheScopeOfADiff(unittest.TestCase):
    """`audit_scope`: the diff's paths, or the whole tree."""

    @classmethod
    def setUpClass(cls):
        cls.mx = _load_make_export()

    def test_no_diff_paths_judges_the_whole_tree(self):
        self.assertIsNone(self.mx.audit_scope(None))

    def test_the_diff_paths_are_the_scope(self):
        self.assertEqual(self.mx.audit_scope([TOUCHED]), {TOUCHED})
        self.assertEqual(self.mx.audit_scope([]), set())

    def test_a_diff_at_the_rules_judges_the_whole_tree(self):
        """The rules (PUBLIC_TEMPLATES, ALLOWED_HITS, DOCS_MAP,
        FORBIDDEN_PREFIX) live in make_export.py: a moved rule can turn any
        untouched file red."""
        self.assertIsNone(self.mx.audit_scope([TOUCHED, MAKE_EXPORT]))

    def test_the_build_path_refuses_the_narrowing(self):
        """A narrowed audit is a strand verdict; an export reads the whole tree."""
        for argv in (["--diff-paths", TOUCHED], ["--no-public-resolver"]):
            err = io.StringIO()
            with mock.patch.object(sys, "argv", ["make_export.py"] + argv), \
                    mock.patch("sys.stderr", new=err), \
                    self.assertRaisesRegex(SystemExit, r"^2$"):
                self.mx.main()
            self.assertIn("nur mit --keep-going", err.getvalue(), argv)


SCRIPTS = (
    ("scripts/check_claims.py", "claims OK"),
    ("scripts/check_adr_anchors.py", "ADR anchor gate OK"),
    ("scripts/check_roadmap_anchors.py", "roadmap anchors OK"),
)


@private_tree.private(MAKE_EXPORT)
class TheResolverModuleUnderTheSwitch(unittest.TestCase):
    """R15 in a stub export tree: each gate script prints its summary, the
    resolver's test module leaves a marker when it runs."""

    @classmethod
    def setUpClass(cls):
        cls.mx = _load_make_export()

    def run_r15(self, resolver):
        mx = self.mx
        marker = tempfile.mktemp(prefix="r15-resolver-ran-")
        self.addCleanup(lambda: pathlib.Path(marker).unlink(missing_ok=True))

        def stub_tree(_tree, dest):
            for rel, summary in SCRIPTS:
                path = pathlib.Path(dest, rel)
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(f"print({summary!r})\n", encoding="utf-8")
            pkg = pathlib.Path(dest, "scripts", "tests")
            pkg.mkdir(parents=True, exist_ok=True)
            pathlib.Path(dest, "scripts", "__init__.py").write_text("", encoding="utf-8")
            (pkg / "__init__.py").write_text("", encoding="utf-8")
            (pkg / "test_gate_plan.py").write_text(
                "import unittest\n"
                "class Ran(unittest.TestCase):\n"
                "    def test_ran(self):\n"
                f"        open({marker!r}, 'w').close()\n", encoding="utf-8")

        with mock.patch.object(mx, "extract_tree", stub_tree), \
                mock.patch.object(mx, "KEEP_GOING", True), \
                mock.patch.object(mx, "REDS", []):
            report = []
            mx.receipt_public_gates("stub", report, resolver=resolver)
            return report, list(mx.REDS), os.path.exists(marker)

    def test_by_default_the_resolver_module_runs_in_the_public_tree(self):
        report, reds, ran = self.run_r15(True)
        self.assertEqual(reds, [])
        self.assertTrue(ran)

    def test_the_switch_skips_the_resolver_module_only(self):
        report, reds, ran = self.run_r15(False)
        self.assertEqual(reds, [])
        self.assertFalse(ran)
        for label in ("spec-claims", "ADR-Anker", "Roadmap-Anker"):
            self.assertTrue([r for r in report if label in r and "OK" in r], (label, report))
        self.assertTrue([r for r in report if "Resolver" in r and "--no-public-resolver" in r],
                        report)


@private_tree.private(MAKE_EXPORT)
class TheStrandPlanNarrowsTheAudit(unittest.TestCase):
    """gate_plan: S passes the switch and the exported tests of its diff;
    integration and release keep the whole audit."""

    TEST = "crates/meclaw-cells/tests/misc_2/gh1199_a_row_is_replaced_in_one_step.rs"

    def audit(self, paths, mode):
        st = {s.name: s for s in gp.plan(paths, mode, repo=None)}
        return st["export-audit"].cmds[0]

    def test_the_strand_passes_its_exported_tests(self):
        cmd = self.audit([self.TEST, "templates/operator/template.json",
                          "scripts/gate_plan.py"], "strand")
        self.assertIn("--no-public-resolver", cmd)
        self.assertEqual(cmd[cmd.index("--diff-paths") + 1:], [self.TEST])

    def test_a_template_only_diff_judges_no_test_file(self):
        cmd = self.audit(["templates/operator/template.json"], "strand")
        self.assertEqual(cmd[-1], "--diff-paths")

    def test_a_diff_at_the_rules_passes_them(self):
        cmd = self.audit([MAKE_EXPORT], "strand")
        self.assertIn(MAKE_EXPORT, cmd[cmd.index("--diff-paths") + 1:])

    def test_integration_and_release_keep_the_whole_audit(self):
        for mode in ("integration", "release"):
            cmd = self.audit([self.TEST], mode)
            self.assertNotIn("--diff-paths", cmd, mode)
            self.assertNotIn("--no-public-resolver", cmd, mode)

    def test_a_test_or_template_diff_keeps_the_resolver(self):
        """Re-check I1: `test_gate_plan` holds real-tree tests over the crates'
        test units and the templates (a new flat `*_demo.rs` moves
        SCENARIO_UNITS_PINNED). Without a kit station in the plan the resolver
        module runs nowhere else, so the strand audit keeps it."""
        for path in ("crates/meclaw-cells/tests/zz_probe_demo.rs",
                     self.TEST, "templates/operator/template.json"):
            st = {s.name: s for s in gp.plan([path], "strand", repo=None)}
            self.assertNotIn("export-tree-tests", st, path)
            self.assertNotIn("--no-public-resolver", st["export-audit"].cmds[0], path)

    def test_the_switch_only_rides_on_a_kit_station_of_the_same_plan(self):
        """The class of I1: a narrowing switch names the station that carries
        what it drops, and that station stands in the same plan."""
        diffs = ([self.TEST], ["templates/operator/template.json"], [MAKE_EXPORT],
                 ["scripts/gate_plan.py", self.TEST],
                 ["scripts/gate_plan.py", "templates/operator/template.json"],
                 ["crates/meclaw-cells/tests/zz_probe_demo.rs", "docs/x.md"])
        for paths in diffs:
            st = {s.name: s for s in gp.plan(paths, "strand", repo=None)}
            kit = [c for c in getattr(st.get("export-tree-tests"), "cmds", [])
                   if "--kit" in c]
            self.assertEqual("--no-public-resolver" in st["export-audit"].cmds[0],
                             bool(kit), paths)


class _StrandPlanFixture:
    """A strand plan over a git fixture and the fail-closed audit it must carry."""

    META = {"packages": [], "workspace_members": []}

    def plan(self, base):
        from scripts.pipeline import plan as pl
        doc = pl.build_plan(self.root, "strand", base, metadata=self.META,
                            resolver=lambda r, m, f: gp.to_json(f, m, r, explain=True))
        st = {s["name"]: s for s in doc["stations"]}
        return st, doc.get("notes") or []

    def assert_whole_audit(self, st, notes):
        self.assertIn("export-audit", sorted(st))
        argv = st["export-audit"]["cmds"][0]["argv"]
        self.assertEqual(argv, gp.strand_audit_cmd(None, kit=False))
        self.assertNotIn("--diff-paths", argv)
        self.assertNotIn("--no-public-resolver", argv)
        self.assertTrue([n for n in notes if "diff base unresolved" in n], notes)


class AnUnreadableDiffAuditsTheWholeTree(_StrandPlanFixture, unittest.TestCase):
    """Re-check I2: `git diff <base> HEAD` that fails is no empty diff. Before
    the fix `changed_files` read its stdout regardless of the exit code, so an
    unknown base planned from 0 paths and `--diff-paths` judged nothing."""

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = os.path.join(tmp.name, "tree")
        os.makedirs(self.root)
        _git(self.root, "init", "-q")
        pathlib.Path(self.root, "README.md").write_text("one\n", encoding="utf-8")
        _git(self.root, "add", "-A")
        _git(self.root, "commit", "-q", "-m", "base")
        self.base = _git(self.root, "rev-parse", "HEAD")
        pathlib.Path(self.root, "README.md").write_text("two\n", encoding="utf-8")
        _git(self.root, "commit", "-q", "-am", "strand: a README diff plans no audit")

    def test_a_readable_diff_plans_no_audit_for_a_readme(self):
        st, notes = self.plan(self.base)
        self.assertNotIn("export-audit", sorted(st))
        self.assertFalse([n for n in notes if "diff base unresolved" in n], notes)

    def test_an_unknown_base_audits_the_whole_tree(self):
        st, notes = self.plan("1" * 40)
        self.assert_whole_audit(st, notes)

    def test_a_failing_git_diff_audits_the_whole_tree(self):
        """Fault injection: the base is a commit, `git diff` fails anyway."""
        from scripts.pipeline import plan as pl
        real = pl._git

        def broken(root, *args, check=True):
            if args[:1] == ("diff",):
                return subprocess.CompletedProcess(["git", *args], 128, "",
                                                   "fatal: injected\n")
            return real(root, *args, check=check)
        with mock.patch.object(pl, "_git", side_effect=broken):
            st, notes = self.plan(self.base)
        self.assert_whole_audit(st, notes)


class AMissingMergeBaseAuditsTheWholeTree(_StrandPlanFixture, unittest.TestCase):
    """Re-check I3: a strand without a merge base (no `master` ref, a foreign
    history, `git merge-base` rc != 0) is no `HEAD~1` diff. Before the fix
    `resolve_base` fell back to `HEAD~1` silently: strand commit 1 (an
    exported test) was never audited because commit 2 touched a README only."""

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = os.path.join(tmp.name, "tree")
        os.makedirs(self.root)
        _git(self.root, "init", "-q", "-b", "main")   # no master ref
        pathlib.Path(self.root, "README.md").write_text("one\n", encoding="utf-8")
        _git(self.root, "add", "-A")
        _git(self.root, "commit", "-q", "-m", "base")
        tests = pathlib.Path(self.root, "crates/meclaw-cells/tests")
        tests.mkdir(parents=True)
        (tests / "zz_probe_demo.rs").write_text("#[test]\nfn t() {}\n", encoding="utf-8")
        _git(self.root, "add", "-A")
        _git(self.root, "commit", "-q", "-m", "strand 1: an exported test")
        pathlib.Path(self.root, "README.md").write_text("two\n", encoding="utf-8")
        _git(self.root, "commit", "-q", "-am", "strand 2: a README")

    def test_no_master_ref_audits_the_whole_tree(self):
        from scripts.pipeline import plan as pl
        st, notes = self.plan(None)
        self.assert_whole_audit(st, notes)
        self.assertEqual(pl.resolve_base(self.root, "strand", None), ("", False))

    def test_the_gate_header_names_the_missing_base(self):
        """GH #1199 (vor-batch, I3 class): without a merge base the header said
        `base = full tree` -- a whole-tree pass it is not; it names the gap."""
        from scripts.pipeline import gate as gt
        out = io.StringIO()
        gt._header({"base": ""}, self.root, "strand", out)
        self.assertRegex(out.getvalue(), r"^gate: base = none \(git merge-base master HEAD: "
                                         r"rc=\d+ [^)]*\) \(\d+ files?\)$")
        self.assertNotIn("full tree", out.getvalue())

    def test_a_failing_merge_base_audits_the_whole_tree(self):
        """Fault injection: a `master` ref exists, `git merge-base` fails anyway."""
        from scripts.pipeline import plan as pl
        _git(self.root, "branch", "master", "HEAD~2")
        real = pl._git

        def broken(root, *args, check=True):
            if args[:1] == ("merge-base",):
                return subprocess.CompletedProcess(["git", *args], 128, "",
                                                   "fatal: injected\n")
            return real(root, *args, check=check)
        with mock.patch.object(pl, "_git", side_effect=broken):
            st, notes = self.plan(None)
        self.assert_whole_audit(st, notes)

    def test_a_merge_base_plans_from_it(self):
        """Control: with `master` at the base the strand diff names commit 1."""
        from scripts.pipeline import plan as pl
        _git(self.root, "branch", "master", "HEAD~2")
        self.assertEqual(pl.resolve_base(self.root, "strand", None)[0],
                         _git(self.root, "rev-parse", "HEAD~2"))
        st, notes = self.plan(None)
        self.assertFalse([n for n in notes if "diff base unresolved" in n], notes)
        self.assertIn("crates/meclaw-cells/tests/zz_probe_demo.rs",
                      pl.changed_files(self.root, "strand", None))

    def test_the_last_commit_on_master_stays_a_head1_diff(self):
        """The deliberate case: on master itself (merge base == HEAD, clean
        tree) the base is HEAD~1 -- a determined base, no fail-closed NOTE."""
        from scripts.pipeline import plan as pl
        _git(self.root, "branch", "master", "HEAD")
        self.assertEqual(pl.resolve_base(self.root, "strand", None)[0],
                         _git(self.root, "rev-parse", "HEAD~1"))
        st, notes = self.plan(None)
        self.assertFalse([n for n in notes if "diff base unresolved" in n], notes)


class AnUnclearBaseRunsTheWholeStage(unittest.TestCase):
    """Re-check I4: without a clear diff base (no merge base, or `git diff`
    rc != 0) only the export audit ran in full; every other station planned
    from the dirty tree -- clippy, fmt, precheck and export-selftest were
    missing and the crate test of strand commit 1 was neither built nor run.
    The rule of `register.build_plan` holds here too: every station of the
    stage, with a NOTE that says why.

    Fixture workspace: strand commit 1 = the crate test `gh3_llm.rs`, strand
    commit 2 = a README."""

    PART = "meclaw-cells::gh3_llm"

    def setUp(self):
        from scripts.tests.test_pipeline_cache import fixture_metadata, fixture_tree
        self.root = fixture_tree(self)
        self.meta = fixture_metadata(self.root)
        self.master = _git(self.root, "rev-parse", "HEAD")
        _git(self.root, "checkout", "-q", "-b", "strand")
        test = pathlib.Path(self.root, "crates/meclaw-cells/tests/gh3_llm.rs")
        test.write_text(test.read_text(encoding="utf-8") + "\n// strand 1\n", encoding="utf-8")
        _git(self.root, "commit", "-q", "-am", "strand 1: a crate test")
        pathlib.Path(self.root, "README.md").write_text("strand 2\n", encoding="utf-8")
        _git(self.root, "add", "-A")
        _git(self.root, "commit", "-q", "-m", "strand 2: a README")

    def plan(self, base=None, files=None, narrow=False):
        from scripts.pipeline import plan as pl
        doc = pl.build_plan(self.root, "strand", base, files=files, metadata=self.meta,
                            narrow=narrow,
                            resolver=lambda r, m, f: gp.to_json(f, m, r, explain=True))
        return {s["name"]: s for s in doc["stations"]}, doc.get("notes") or []

    def whole_stage(self):
        """The stations the strand stage plans over every tracked path."""
        return sorted(self.plan(files=_git(self.root, "ls-files").splitlines())[0])

    def assert_whole_stage(self, st, notes, why):
        self.assertEqual(self.whole_stage(), sorted(st))
        for name in ("precheck", "fmt", "clippy", "export-selftest", "export-audit"):
            self.assertIn(name, st)
        self.assertIn(self.PART, st["tests"]["parts"])
        mine = [n for n in notes if n.startswith("diff base unresolved")]
        self.assertEqual(1, len(mine), notes)
        self.assertIn(why, mine[0])
        self.assertIn("every station of the stage runs", mine[0])
        # the whole tree never travels on an argv (ARG_MAX): precheck lists it itself
        argv = st["precheck"]["cmds"][0]["argv"]
        self.assertEqual(["bash", "-c"], argv[:2])
        self.assertIn("git ls-files", argv[2])
        self.assertIn("scripts/precheck.py --files-from -", argv[2])

    @staticmethod
    def failing(verb):
        from scripts.pipeline import plan as pl
        real = pl._git

        def broken(root, *args, check=True):
            if args[:1] == (verb,):
                return subprocess.CompletedProcess(["git", *args], 128, "",
                                                   "fatal: injected\n")
            return real(root, *args, check=check)
        return mock.patch.object(pl, "_git", side_effect=broken)

    def test_a_merge_base_plans_the_strand_diff(self):
        """Control: with `master` the plan names commit 1's test and no NOTE."""
        st, notes = self.plan()
        self.assertIn(self.PART, st["tests"]["parts"])
        self.assertIn("clippy", st)
        self.assertFalse([n for n in notes if n.startswith("diff base unresolved")], notes)
        self.assertLess(len(st), len(self.whole_stage()))

    def test_a_failing_merge_base_runs_every_station(self):
        with self.failing("merge-base"):
            st, notes = self.plan()
        self.assert_whole_stage(st, notes, "git merge-base master HEAD: rc=128")

    def test_a_failing_git_diff_runs_every_station(self):
        with self.failing("diff"):
            st, notes = self.plan(self.master)
        self.assert_whole_stage(st, notes, "git diff %s HEAD: rc=128" % self.master[:12])

    def test_an_unclear_base_never_narrows(self):
        """No base, no narrowing: the gate's run switch stays off."""
        from scripts.pipeline import narrow as narrow_mod
        with self.failing("merge-base"), \
                mock.patch.object(narrow_mod, "apply", side_effect=AssertionError("narrowed")):
            st, notes = self.plan(narrow=True)
        self.assertIn(self.PART, st["tests"]["parts"])


# GH #1199 (v2-r3-rest, release 0.65.0): R8 -- german_scan over the exported
# templates -- ran only in the FULL audit (`gate release`, the extracted tree).
# KM/KP brought 20 German strings into templates/memory-hive through strand and
# merge gates; the release pre-run was the first red. The dry audit now runs
# R8 with the reach of the full run: every root of `make_export.R8_ROOTS`,
# `--strict` and `--strings --strict`, the declarations of the full run.
GERMAN_SCAN = "plans/p5.5-fixtures/german_scan.py"
R8_TOUCHED = "templates/zz-r8/touched/config.json"
R8_UNTOUCHED = "templates/zz-r8/untouched/script.py"
R8_CLEAN = "templates/zz-r8/clean/config.json"
# One German line per further root of the full run, (path, line).
R8_ROOTS_RED = {
    "crates": ("crates/zz-r8/src/lib.rs", 2),
    "tests": ("tests/zz-r8/README.md", 3),
    "examples": ("examples/zz-r8/main.rs", 2),
    ".gitignore": (".gitignore", 1),
}
R8_STRINGS = "crates/zz-r8/tests/strings.rs"
R8_RUNTIME = "crates/zz-r8/src/ui.rs"
R8_FILES = {
    R8_TOUCHED: ('{\n  "type": "code",\n  "describe": "The cell answers once per turn.",\n'
                 '  "note": "Hier wird die Uhr nicht gelesen"\n}\n'),
    R8_UNTOUCHED: "# Das ist ein deutscher Kommentar\nprint(1)\n",
    R8_CLEAN: '{\n  "describe": "English only."\n}\n',
    "crates/zz-r8/src/lib.rs": "pub fn f() {}\n// Hier wird die Uhr nicht gelesen\n",
    "tests/zz-r8/README.md": "# Fixture\n\nDas ist ein deutscher Kommentar\n",
    "examples/zz-r8/main.rs": "fn main() {\n    // Das ist ein deutscher Kommentar\n}\n",
    ".gitignore": "# Das ist ein deutscher Kommentar\ntarget/\n",
    # a German literal in test context (red) and one in runtime code (counted
    # in the full run's class 1, not red): `--strings --strict`
    R8_STRINGS: '#[test]\nfn t() {\n    assert!(true, "Die Uhr wird nicht gelesen");\n}\n',
    R8_RUNTIME: 'pub const T: &str = "Die Uhr wird nicht gelesen";\n',
}


def _r8_rows(reds, head):
    """(path, line) of the findings in the red whose head starts with `head`."""
    rows = set()
    for r in reds:
        if not r.startswith(head):
            continue
        for ln in r.splitlines()[1:]:
            path, line, _ = ln.strip().split(":", 2)
            rows.add((path, int(line)))
    return rows


def _r8_declarations(mx):
    """Two declared templates/ paths of the REAL table: one whose blocks the
    fixture carries (exempt), one whose file stands without them (dead)."""
    paths = sorted({p for p, _ in mx._german_scan_module().DECLARED_DATA_BLOCKS
                    if p.startswith("templates/") and p.endswith(".json")})
    return paths[0], paths[1]


@private_tree.private(MAKE_EXPORT, GERMAN_SCAN)
class R8RunsDryOverTheTemplates(unittest.TestCase):
    """`receipt_r8_dry` over a fixture tree: German template files,
    a clean one, a declared one and a dead declaration."""

    @classmethod
    def setUpClass(cls):
        cls.mx = _load_make_export()
        cls.declared, cls.dead = _r8_declarations(cls.mx)
        blocks = [b for p, b in cls.mx._german_scan_module().DECLARED_DATA_BLOCKS
                  if p == cls.declared]
        files = dict(R8_FILES)
        files[cls.declared] = "".join(b + "\n" for b in blocks)
        files[cls.dead] = '{\n  "describe": "English only."\n}\n'
        cls.tmp = tempfile.TemporaryDirectory(prefix="export-audit-r8-")
        root = cls.tmp.name
        _git(root, "init", "-q")
        for rel, text in files.items():
            path = pathlib.Path(root, rel)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text, encoding="utf-8")
        _git(root, "add", "-A")
        cls.tree = _git(root, "write-tree")
        cls.paths = sorted(files)

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def judge(self, scope):
        """R8 dry over the fixture tree: (reds, report, paths read)."""
        mx = self.mx
        read = []
        real_texts = mx.r8_texts

        def spy(tree, judged):
            read.extend(judged)
            return real_texts(tree, judged)

        with mock.patch.object(mx, "REPO", pathlib.Path(self.tmp.name)), \
                mock.patch.object(mx, "KEEP_GOING", True), \
                mock.patch.object(mx, "REDS", []), \
                mock.patch.object(mx, "R8_SCOPE", scope), \
                mock.patch.object(mx, "r8_texts", spy):
            report = []
            mx.receipt_r8_dry(self.tree, self.paths, report)
            return list(mx.REDS), report, read

    def test_a_german_line_in_a_touched_template_is_red_with_file_and_line(self):
        reds, _, _ = self.judge({R8_TOUCHED})
        rows = [r for r in reds if r.startswith("R8")]
        self.assertEqual(len(rows), 1, reds)
        self.assertIn(R8_TOUCHED + ":4:", rows[0])
        self.assertIn("Hier wird die Uhr nicht gelesen", rows[0])

    def test_the_whole_subset_finds_every_german_template(self):
        reds, _, _ = self.judge(None)
        hits = "\n".join(r for r in reds if r.startswith("R8"))
        self.assertIn(R8_TOUCHED + ":4:", hits)
        self.assertIn(R8_UNTOUCHED + ":1:", hits)
        self.assertNotIn(R8_CLEAN, hits)

    def test_an_untouched_template_is_neither_read_nor_judged(self):
        reds, _, read = self.judge({R8_TOUCHED})
        self.assertNotIn(R8_UNTOUCHED, read)
        self.assertIn(R8_TOUCHED, read)
        self.assertFalse([r for r in reds if R8_UNTOUCHED in r], reds)

    def test_a_diff_without_templates_judges_no_file(self):
        reds, report, read = self.judge(set())
        self.assertEqual(reds, [])
        self.assertEqual(read, [])
        self.assertTrue([r for r in report if r.lstrip().startswith("R8") and "0 " in r], report)

    def test_the_declarations_of_the_full_run_exempt_their_blocks(self):
        reds, report, _ = self.judge({self.declared})
        self.assertEqual(reds, [])
        self.assertTrue([r for r in report if "R8" in r and self.declared in r], report)

    def test_a_dead_declaration_in_a_judged_file_is_red(self):
        reds, _, _ = self.judge({self.dead})
        rows = [r for r in reds if r.startswith("R8")]
        self.assertEqual(len(rows), 1, reds)
        self.assertIn(self.dead, rows[0])
        self.assertIn("DECLARED_DATA_BLOCKS", rows[0])

    def test_a_german_line_under_every_root_of_the_full_run_is_red_with_file_and_line(self):
        for root, (path, line) in R8_ROOTS_RED.items():
            with self.subTest(root=root):
                reds, _, read = self.judge({path})
                self.assertEqual(_r8_rows(reds, "R8 (dry) german_scan --strict"),
                                 {(path, line)}, reds)
                self.assertEqual(read, [path])

    def test_a_german_literal_in_test_context_is_red_with_file_and_line(self):
        reds, _, _ = self.judge({R8_STRINGS})
        self.assertEqual(_r8_rows(reds, "R8 (dry) german_scan --strings"),
                         {(R8_STRINGS, 3)}, reds)
        self.assertEqual(_r8_rows(reds, "R8 (dry) german_scan --strict"), set(), reds)

    def test_a_german_runtime_literal_is_counted_not_red_like_the_full_run(self):
        reds, report, _ = self.judge({R8_RUNTIME})
        self.assertEqual(reds, [])
        self.assertTrue([r for r in report if "1 Laufzeit-Literal" in r], report)

    def test_the_dry_findings_are_the_findings_of_the_full_run(self):
        """Differential pin: german_scan.py itself, run like the full audit
        (`--strict` and `--strings --strict` over `GERMAN_SCAN_ROOTS` =
        `R8_ROOTS` in a checkout), names the same file:line as the dry stage
        over the whole subset -- the same reach, both modes."""
        env = dict(os.environ, GERMAN_SCAN_ROOTS=":".join(self.mx.R8_ROOTS))
        scan = str(pathlib.Path(REPO, GERMAN_SCAN))

        def run(*mode):
            return subprocess.run([sys.executable, scan, "-C", self.tmp.name, *mode],
                                  capture_output=True, text=True, env=env)

        comments = run("--strict", "--lines")
        full_hits, path = set(), None
        for ln in comments.stdout.splitlines():
            if ln.startswith("=== "):
                path = ln[4:]
            elif path and ln[:5].strip().isdigit() and ln[5:7] == ": ":
                full_hits.add((path, int(ln[:5])))
        strings = run("--strings", "--strict")
        part = strings.stdout.split("--- Klasse 2 (Test-Kontext) ---", 1)[1]
        full_strings = {(r.strip().split(":", 2)[0], int(r.strip().split(":", 2)[1]))
                        for r in part.split("\n\n", 1)[0].splitlines() if r.strip()}
        reds, _, _ = self.judge(None)
        self.assertEqual(_r8_rows(reds, "R8 (dry) german_scan --strict"), full_hits)
        self.assertEqual(_r8_rows(reds, "R8 (dry) german_scan --strings"), full_strings)
        self.assertEqual(comments.returncode, 1, comments.stdout)
        self.assertEqual(strings.returncode, 1, strings.stdout)
        self.assertIn((R8_STRINGS, 3), full_strings)
        self.assertTrue({p for p, _ in R8_ROOTS_RED.values()} <= {p for p, _ in full_hits})

    def test_a_frozen_corridor_is_found_in_the_judged_text_not_the_cwd(self):
        """`corridor_ranges` read the corridor file relative to the cwd; the
        dry stage judges a blob from any cwd. The corridor lines of the text
        handed in stay the declared rest, whatever lies on disk."""
        gs = self.mx._german_scan_module()
        path, marker = gs.FROZEN_CORRIDORS[0]
        text = f"// head\n{marker}x: u8) {{\n    // Hier wird die Uhr nicht gelesen\n}}\n"
        r = gs.classify_file(path, text)
        self.assertEqual(r["hits"], [])
        self.assertEqual([n for n, _ in r["corridor"]], [3])


@private_tree.private(MAKE_EXPORT, GERMAN_SCAN)
class TheFastWordCheckIsTheRegex(unittest.TestCase):
    """`german_scan.has_german_word` answers `GERMAN_RE.search` (v2-r3-rest):
    on ASCII text by a word set -- the alternation cost 15 of the 17 s of R8
    over the export subset --, on other text by the regex itself."""

    LINES = ("Der Wert", "DER", "xder", "der_x", "der1", "1der", "_der", "der.", "(der)",
             "der-x", "Der_Wert", "Fehler!", "is it", "English only.", "", "  // nicht",
             "naechste Zeile", "für", "Straße der", "\u017fein", "caf\u00e9 der", "\tden\n",
             "a|b", "KÖNNEN", "\u212aann")

    def test_it_answers_the_regex_on_every_line(self):
        gs = _load_make_export()._german_scan_module()
        lines = list(self.LINES)
        for w in sorted(gs.GERMAN_WORD_SET):
            lines += [w, w.upper(), w.title(), "x" + w, w + "_", w + "9", "(" + w + ")",
                      "a " + w + " b"]
        for line in lines:
            self.assertEqual(gs.has_german_word(line), gs.GERMAN_RE.search(line) is not None,
                             repr(line))


@private_tree.private(MAKE_EXPORT, GERMAN_SCAN)
class TheScopeOfR8(unittest.TestCase):
    """`r8_scope`: the diff's files under R8_ROOTS, or the whole subset."""

    @classmethod
    def setUpClass(cls):
        cls.mx = _load_make_export()

    def test_no_template_paths_judges_the_whole_subset(self):
        self.assertIsNone(self.mx.r8_scope(None))

    def test_the_template_paths_are_the_scope(self):
        self.assertEqual(self.mx.r8_scope([R8_TOUCHED]), {R8_TOUCHED})
        self.assertEqual(self.mx.r8_scope([]), set())

    def test_every_root_of_the_full_run_is_in_scope_and_nothing_else(self):
        paths = [p for p, _ in R8_ROOTS_RED.values()] + [R8_STRINGS, "docs/x.md",
                                                         "scripts/x.py", "crates/x/src/a.sql"]
        self.assertEqual(self.mx.r8_scope(paths),
                         {p for p, _ in R8_ROOTS_RED.values()} | {R8_STRINGS})

    def test_the_roots_are_the_ones_the_full_run_scans(self):
        """One list: the full run hands `R8_ROOTS` to german_scan, derived from
        what travels (TREE_DIRS), not typed twice."""
        mx = self.mx
        self.assertEqual(mx.R8_ROOTS[:len(mx.TREE_DIRS)], tuple(mx.TREE_DIRS))
        self.assertEqual(set(mx.R8_ROOTS) - set(mx.TREE_DIRS), {"templates", ".gitignore"})
        self.assertIn('GERMAN_SCAN_ROOTS=":".join(R8_ROOTS)',
                      inspect.getsource(mx.receipt_on_worktree))

    def test_a_diff_at_what_decides_r8_judges_the_whole_subset(self):
        """make_export.py decides which templates travel (PUBLIC_TEMPLATES),
        german_scan.py carries the declarations: a change at either can turn
        any untouched template red."""
        for rules in (MAKE_EXPORT, GERMAN_SCAN):
            self.assertIsNone(self.mx.r8_scope([R8_TOUCHED, rules]), rules)

    def test_the_build_path_refuses_the_narrowing(self):
        err = io.StringIO()
        with mock.patch.object(sys, "argv", ["make_export.py", "--r8-paths", R8_TOUCHED]), \
                mock.patch("sys.stderr", new=err), \
                self.assertRaisesRegex(SystemExit, r"^2$"):
            self.mx.main()
        self.assertIn("nur mit --keep-going", err.getvalue())


@private_tree.private(MAKE_EXPORT, GERMAN_SCAN)
class TheDryAuditRunsR8(unittest.TestCase):
    """`main()` in the dry shape over a fixture repo: the stages that need the
    real tree are stubbed, the R8 stage is not. The strand's audit is red with
    file and line; the full run leaves R8 to the extracted tree."""

    @classmethod
    def setUpClass(cls):
        cls.mx = _load_make_export()
        cls.tmp = tempfile.TemporaryDirectory(prefix="export-audit-r8-main-")
        root = cls.tmp.name
        _git(root, "init", "-q")
        for rel, text in R8_FILES.items():
            path = pathlib.Path(root, rel)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text, encoding="utf-8")
        _git(root, "add", "-A")
        _git(root, "commit", "-q", "-m", "fixture")

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def run_main(self, argv):
        mx = self.mx

        def index_of(rev, index, _report):
            mx.git("read-tree", rev, index=index)
            return mx.git("ls-files", index=index).split()

        def nothing(*_args, **_kwargs):
            return None

        out, err = io.StringIO(), io.StringIO()
        with mock.patch.object(mx, "REPO", pathlib.Path(self.tmp.name)), \
                mock.patch.object(mx, "build_index", index_of), \
                mock.patch.object(mx, "receipt", nothing), \
                mock.patch.object(mx, "receipt_every_template_has_a_verdict", nothing), \
                mock.patch.object(mx, "receipt_on_worktree", nothing), \
                mock.patch.object(sys, "argv", ["make_export.py"] + argv), \
                mock.patch("sys.stdout", new=out), mock.patch("sys.stderr", new=err):
            rc = mx.main()
        return rc, out.getvalue(), err.getvalue()

    DRY = ["--keep-going", "--skip-cargo", "--rev", "HEAD", "--parent", "HEAD",
           "--no-gate-receipt", "--no-public-gates"]

    def test_the_strand_audit_over_a_german_template_is_red_with_file_and_line(self):
        rc, _, err = self.run_main(self.DRY + ["--r8-paths", R8_TOUCHED])
        self.assertEqual(rc, 1, err)
        self.assertIn(R8_TOUCHED + ":4:", err)
        self.assertNotIn(R8_UNTOUCHED, err)

    def test_the_integration_shape_judges_every_exported_template(self):
        rc, _, err = self.run_main(self.DRY)
        self.assertEqual(rc, 1, err)
        self.assertIn(R8_TOUCHED + ":4:", err)
        self.assertIn(R8_UNTOUCHED + ":1:", err)

    def test_the_strand_audit_over_a_german_line_under_any_root_is_red_with_file_and_line(self):
        for root, (path, line) in R8_ROOTS_RED.items():
            with self.subTest(root=root):
                rc, _, err = self.run_main(self.DRY + ["--r8-paths", path])
                self.assertEqual(rc, 1, err)
                self.assertIn(f"{path}:{line}:", err)
                self.assertNotIn(R8_UNTOUCHED, err)

    def test_the_strand_audit_over_a_german_test_literal_is_red_with_file_and_line(self):
        rc, _, err = self.run_main(self.DRY + ["--r8-paths", R8_STRINGS])
        self.assertEqual(rc, 1, err)
        self.assertIn(f"{R8_STRINGS}:3:", err)
        self.assertIn("--strings", err)

    def test_a_diff_without_templates_stays_green(self):
        rc, out, err = self.run_main(self.DRY + ["--r8-paths"])
        self.assertEqual(rc, 0, err)
        self.assertTrue([r for r in out.splitlines() if r.lstrip().startswith("R8")], out)

    def test_a_read_failure_aborts_the_stage_red(self):
        """fail-closed (GH #1199): a dry R8 that cannot read the judged blobs
        is red with its stage named, never green over files it did not see
        (the gap of 0.65.0). The clean template alone is green."""
        mx = self.mx

        def boom(*_args, **_kwargs):
            raise mx.Failure("injected: git cat-file --batch -> 128")

        with mock.patch.object(mx, "r8_texts", boom):
            rc, _, err = self.run_main(self.DRY + ["--r8-paths", R8_CLEAN])
        self.assertEqual(rc, 1, err)
        self.assertIn("[Stufe receipt_r8_dry abgebrochen] injected: git cat-file", err)

    def test_a_load_failure_of_german_scan_aborts_the_stage_red(self):
        """fail-closed (GH #1199): without german_scan's rules there is no R8
        verdict -- the stage is red, never skipped. `r8_scans` keeps
        german_scan's `SCAN_EXT` in `_R8_EXT`: with it held, the load fails
        inside the stage; with it empty, `r8_scope` fails first and the
        failure leaves `main` (exit 1) -- both pinned, neither depends on what
        an earlier test left in the cache."""
        mx = self.mx
        scan_ext = mx._german_scan_module().SCAN_EXT

        def boom():
            raise mx.Failure(f"injected: german_scan.py does not load: {GERMAN_SCAN}")

        with mock.patch.object(mx, "_R8_EXT", [scan_ext]), \
                mock.patch.object(mx, "_german_scan_module", boom):
            rc, _, err = self.run_main(self.DRY + ["--r8-paths", R8_CLEAN])
        self.assertEqual(rc, 1, err)
        self.assertIn("[Stufe receipt_r8_dry abgebrochen] injected: german_scan.py", err)
        with mock.patch.object(mx, "_R8_EXT", []), \
                mock.patch.object(mx, "_german_scan_module", boom), \
                self.assertRaisesRegex(mx.Failure, "injected: german_scan.py"):
            self.run_main(self.DRY + ["--r8-paths", R8_CLEAN])

    def test_the_full_audit_leaves_r8_to_the_extracted_tree(self):
        """Without --skip-cargo R8 runs in `receipt_on_worktree` over the
        export tree (stubbed here); the dry stage does not run twice."""
        full = [a for a in self.DRY if a != "--skip-cargo"]
        rc, out, err = self.run_main(full)
        self.assertEqual(rc, 0, err)
        self.assertNotIn("R8 (dry)", out)


@private_tree.private(MAKE_EXPORT, GERMAN_SCAN)
class TheStrandPlanHandsR8ItsFiles(unittest.TestCase):
    """gate_plan: the strand audit names its diff's files under R8_ROOTS
    (`--r8-paths`, before `--diff-paths`); integration runs R8 dry over
    the whole subset, release in the extracted tree."""

    TEST = TheStrandPlanNarrowsTheAudit.TEST
    TPL = "templates/operator/template.json"

    def audit(self, paths, mode):
        st = {s.name: s for s in gp.plan(paths, mode, repo=None)}
        return st["export-audit"].cmds[0]

    def r8_paths(self, cmd):
        start = cmd.index("--r8-paths") + 1
        end = cmd.index("--diff-paths") if "--diff-paths" in cmd else len(cmd)
        return cmd[start:end]

    def test_a_template_diff_hands_its_templates_to_r8(self):
        # v2-r3-rest: the exported test is an R8 file as well (crates/)
        cmd = self.audit([self.TEST, self.TPL, "docs/x.md"], "strand")
        self.assertEqual(self.r8_paths(cmd), [self.TEST, self.TPL])
        self.assertEqual(cmd[cmd.index("--diff-paths") + 1:], [self.TEST])

    def test_a_diff_without_r8_files_plans_no_r8_file(self):
        cmd = self.audit([self.TPL.replace(".json", ".png"), self.TEST.replace(".rs", ".sql")],
                         "strand")
        self.assertEqual(self.r8_paths(cmd), [])

    def test_a_diff_under_any_root_hands_its_files_to_r8_alone(self):
        """A crate source, root tests/, examples/, .gitignore: R8 reads them,
        nothing else in the strand audit does -- the station is planned for
        R8, judges no test file and drops the resolver module, which never
        ran on such a diff (it judges exported tests, templates, make_export)."""
        for root, (path, _) in R8_ROOTS_RED.items():
            with self.subTest(root=root):
                cmd = self.audit([path, "docs/x.md"], "strand")
                self.assertEqual(self.r8_paths(cmd), [path])
                self.assertEqual(cmd[cmd.index("--diff-paths") + 1:], [])
                self.assertIn("--no-public-resolver", cmd)

    def test_a_scripts_or_docs_diff_still_plans_no_audit(self):
        st = {s.name for s in gp.plan(["docs/x.md", "scripts/x.py", "README.md"],
                                      "strand", repo=None)}
        self.assertNotIn("export-audit", st)

    def test_the_reason_of_an_r8_only_diff_names_r8(self):
        path = R8_ROOTS_RED["crates"][0]
        rows = {r["name"]: r for r in gp.to_json([path], "strand", None,
                                                  explain=True)["stations"]}
        self.assertEqual(rows["export-audit"]["reason"], ["r8: " + path])

    def test_a_diff_at_the_declarations_plans_the_audit_over_every_template(self):
        cmd = self.audit([GERMAN_SCAN], "strand")
        self.assertIn(GERMAN_SCAN, self.r8_paths(cmd))
        self.assertIsNone(_load_make_export().r8_scope(self.r8_paths(cmd)))

    def test_a_failing_r8_rule_fails_the_plan(self):
        """fail-closed (GH #1199): when make_export's R8 rule cannot answer,
        the plan fails -- it never drops the R8 file (and with it the
        audit) silently."""
        def boom(_path):
            raise RuntimeError("injected: r8_scans")

        path = R8_ROOTS_RED["crates"][0]
        with mock.patch.object(gp._export_module(), "r8_scans", boom), \
                self.assertRaisesRegex(RuntimeError, "injected: r8_scans"):
            gp.plan([path, "docs/x.md"], "strand", repo=None)

    def test_a_make_export_that_does_not_load_fails_the_plan(self):
        """fail-closed (GH #1199): a make_export.py that does not load is no
        `None` (that means: no plans/ in this tree) -- the plan fails."""
        def boom():
            raise SyntaxError("injected: make_export.py")

        path = R8_ROOTS_RED["crates"][0]
        with mock.patch.object(gp, "_export_module", boom), \
                self.assertRaisesRegex(SyntaxError, "injected: make_export.py"):
            gp.plan([path, "docs/x.md"], "strand", repo=None)

    def test_integration_runs_r8_dry_over_the_whole_subset(self):
        cmd = self.audit([self.TPL], "integration")
        self.assertIn("--skip-cargo", cmd)
        self.assertNotIn("--r8-paths", cmd)

    def test_release_runs_r8_in_the_extracted_tree(self):
        cmd = self.audit([self.TPL], "release")
        self.assertNotIn("--skip-cargo", cmd)
        self.assertNotIn("--r8-paths", cmd)


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


if __name__ == "__main__":
    unittest.main()
