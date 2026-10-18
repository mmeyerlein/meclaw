"""Locks for scripts/harness_fold.py on a fixture crate -- no cargo, no git."""

import os
import pathlib
import re
import shutil
import sys
import tempfile
import unittest
from unittest import mock

SCRIPTS = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))

import harness_fold as hf  # noqa: E402
from scripts.tests import private_tree, store_isolation  # noqa: E402


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()


def tearDownModule():  # noqa: N802 -- unittest's name
    store_isolation.restore()

FILES = {
    "alpha_one.rs": "//! Uses the helper.\nmod common;\n\n#[test]\nfn one() { common::help(); }\n",
    "alpha_two.rs": "#[test]\nfn two() {}\n\n#[tokio::test]\nasync fn three() {}\n",
    "beta_one.rs": "mod mock_server;\n\n#[test]\nfn uses_mock() { mock_server::start(); }\n",
    "mock_server.rs": "#![allow(dead_code)]\npub fn start() {}\n\n#[test]\nfn mock_alone() {}\n",
    "gamma_path.rs": "#[path = \"elsewhere.rs\"]\nmod elsewhere;\n\n#[test]\nfn g() {}\n",
    "delta_demo.rs": "#[test]\nfn demo() {}\n",
    "common/mod.rs": "#![allow(dead_code)]\npub fn help() {}\n",
    "HARNESS.toml": 'max_files = 60\nkeep = []\n[[rule]]\nmatch = "^alpha_"\nharness = "alpha"\n',
}


def _tests_attr_count(tests_dir):
    rx = re.compile(r"#\[(?:tokio::)?test\b")
    n = 0
    for p in pathlib.Path(tests_dir).rglob("*.rs"):
        n += len(rx.findall(p.read_text()))
    return n


def _snapshot(tests_dir):
    out = {}
    for p in sorted(pathlib.Path(tests_dir).rglob("*")):
        if p.is_file():
            out[str(p.relative_to(tests_dir))] = p.read_bytes()
    return out


class Fixture(unittest.TestCase):
    def setUp(self):
        self.repo = tempfile.mkdtemp(prefix="harness-fold-")
        self.addCleanup(shutil.rmtree, self.repo)
        self.tests = os.path.join(self.repo, "crates", "fx", "tests")
        os.makedirs(os.path.join(self.tests, "common"))
        with open(os.path.join(self.repo, "crates", "fx", "Cargo.toml"), "w") as fh:
            fh.write('[package]\nname = "fx"\nversion = "0.0.0"\n')
        for name, text in FILES.items():
            self.write(name, text)

    def write(self, name, text):
        with open(os.path.join(self.tests, name), "w") as fh:
            fh.write(text)

    def run_fold(self, mode):
        return hf.main(["--crate", "fx", "--repo", self.repo, f"--{mode}", "--no-git"])

    def plan(self):
        return hf.Crate(self.repo, "fx").plan()


class TestHarnessFold(Fixture):
    def test_fold_plan_assigns_every_file_once(self):
        plan = self.plan()
        self.assertEqual(sorted(plan), sorted(
            n[:-3] for n in FILES if n.endswith(".rs") and "/" not in n))
        self.assertEqual(plan["alpha_one"], ("alpha", None))
        self.assertEqual(plan["alpha_two"], ("alpha", None))
        self.assertEqual(plan["beta_one"][0], "misc")

    def test_fold_apply_keeps_file_bytes(self):
        self.run_fold("apply")
        for stem, h in (("alpha_two", "alpha"), ("mock_server", "misc"), ("beta_one", "misc"),
                        ("alpha_one", "alpha")):
            # the fold's own attribute line is the one line it adds (rustfmt's place for it)
            got = [ln for ln in pathlib.Path(self.tests, h, stem + ".rs").read_text().split("\n")
                   if ln.strip() != "#[allow(unused_imports)]"]
            want = FILES[stem + ".rs"].split("\n")
            changed = [(a, b) for a, b in zip(want, got) if a != b]
            self.assertEqual(len(got), len(want))
            self.assertLessEqual(len(changed), 1, stem)
            for old, new in changed:
                self.assertRegex(old, r"^mod \w+;$")
                self.assertRegex(new, r"^(#\[allow\(unused_imports\)\] )?use crate::")

    def test_fold_unfold_roundtrip_is_identity(self):
        before = _snapshot(self.tests)
        self.run_fold("apply")
        self.assertNotEqual(_snapshot(self.tests), before)
        self.run_fold("apply")  # idempotent
        self.run_fold("unfold")
        self.assertEqual(_snapshot(self.tests), before)

    def test_fold_keeps_browser_and_scenario_binaries_separate(self):
        self.write("epsilon_browser.rs", "#[test]\nfn b() {}\n")
        self.write("zeta_e2e_flow.rs", "#[test]\nfn z() {}\n")
        browser = "crates/fx/tests/epsilon_browser.rs"
        real = hf._gate_plan_keeps
        hf._gate_plan_keeps = lambda: (real()[0], frozenset({browser}))
        self.addCleanup(setattr, hf, "_gate_plan_keeps", real)
        plan = self.plan()
        self.assertEqual(plan["delta_demo"], ("keep", "scenario"))
        self.assertEqual(plan["zeta_e2e_flow"], ("keep", "scenario"))
        self.assertEqual(plan["epsilon_browser"], ("keep", "browser"))

    def test_fold_blocks_path_and_include_cases(self):
        self.write("eta_include.rs", 'include!("gen.rs");\n#[test]\nfn e() {}\n')
        self.write("theta_bytes.rs", 'const B: &[u8] = include_bytes!("blob.bin");\n')
        self.write("iota_main.rs", "fn main() {}\n")
        self.write("kappa_ignored.rs", "#[test]\n#[ignore]\nfn k() {}\n")
        self.write("lambda_exec.rs", "fn f() { std::env::current_exe().unwrap(); }\n")
        plan = self.plan()
        self.assertEqual(plan["gamma_path"], ("keep", "block:path"))
        self.assertEqual(plan["eta_include"], ("keep", "block:include"))
        self.assertEqual(plan["theta_bytes"], ("keep", "block:relative_include"))
        self.assertEqual(plan["iota_main"], ("keep", "block:fn_main"))
        self.assertEqual(plan["kappa_ignored"], ("keep", "block:ignore_proof"))
        self.assertEqual(plan["lambda_exec"], ("keep", "block:self_exec"))

    def test_fold_check_is_lock(self):
        self.assertEqual(self.run_fold("check"), 1)  # flat tree, plan folds
        self.run_fold("apply")
        self.assertEqual(self.run_fold("check"), 0)
        self.write("alpha_three.rs", "#[test]\nfn t() {}\n")  # a new flat file
        self.assertEqual(self.run_fold("check"), 1)
        main = pathlib.Path(self.tests, "alpha", "main.rs")
        self.run_fold("apply")
        main.write_text(main.read_text().replace("pub mod alpha_two;\n", ""))
        self.assertEqual(self.run_fold("check"), 1)

    def test_fold_shares_common_once_via_use_crate(self):
        self.write("alpha_four.rs", "mod common;\n#[test]\nfn four() { common::help(); }\n")
        self.run_fold("apply")
        main = pathlib.Path(self.tests, "alpha", "main.rs").read_text()
        self.assertEqual(main.count("pub mod common;"), 1)
        self.assertIn('#[path = "../common/mod.rs"]\npub mod common;', main)
        self.assertNotIn("#[path", main.replace('#[path = "../common/mod.rs"]', ""))
        for stem in ("alpha_one", "alpha_four"):
            text = pathlib.Path(self.tests, "alpha", stem + ".rs").read_text()
            self.assertIn("use crate::common; // harness_fold: mod common;", text)
            self.assertNotRegex(text, r"(?m)^mod common;")
        self.assertTrue(os.path.isfile(os.path.join(self.tests, "common", "mod.rs")))

    def test_fold_blocks_crate_paths_and_crate_attributes(self):
        self.write("mu_crate.rs", "use crate::x;\n")
        self.write("nu_attr.rs", "#![recursion_limit = \"256\"]\n#[test]\nfn n() {}\n")
        self.write("xi_feature.rs", "#![feature(test)]\n")
        self.write("omicron_macro.rs", "#[macro_export]\nmacro_rules! m { () => {} }\n")
        self.write("pi_allow.rs", "#![allow(clippy::all)]\n#![cfg(feature = \"x\")]\n#[test]\nfn p() {}\n")
        plan = self.plan()
        self.assertEqual(plan["mu_crate"], ("keep", "block:crate_path"))
        self.assertEqual(plan["nu_attr"], ("keep", "block:crate_attr"))
        self.assertEqual(plan["xi_feature"], ("keep", "block:crate_attr"))
        self.assertEqual(plan["omicron_macro"], ("keep", "block:macro_export"))
        self.assertEqual(plan["pi_allow"][0], "misc")  # module-level attributes fold

    def test_fold_keeps_sharers_together(self):
        plan = self.plan()
        self.assertEqual(plan["beta_one"][0], plan["mock_server"][0])
        self.run_fold("apply")
        h = plan["beta_one"][0]
        main = pathlib.Path(self.tests, h, "main.rs").read_text()
        self.assertEqual(main.count("pub mod mock_server;"), 1)
        beta = pathlib.Path(self.tests, h, "beta_one.rs").read_text()
        self.assertIn("use crate::mock_server; // harness_fold: mod mock_server;", beta)
        # a sharer whose partner must stay its own binary stays too
        self.write("rho_uses_gamma.rs", "mod gamma_path;\n")
        self.run_fold("unfold")
        plan = self.plan()
        self.assertEqual(plan["rho_uses_gamma"], ("keep", "sharer_of:gamma_path"))

    def _support(self, name, text="#![allow(dead_code)]\npub fn go() {}\n"):
        os.makedirs(os.path.join(self.tests, "support"), exist_ok=True)
        self.write(os.path.join("support", name), text)

    def test_fold_path_helper_is_declared_once_in_main(self):
        self._support("road.rs")
        path_mod = '#[path = "support/road.rs"]\nmod road;\n'
        self.write("sigma_one.rs", path_mod + "#[test]\nfn s1() { road::go(); }\n")
        self.write("sigma_two.rs", "//! two\n" + path_mod + "#[test]\nfn s2() { road::go(); }\n")
        self.write("HARNESS.toml", FILES["HARNESS.toml"] + '[[rule]]\nmatch = "^sigma_"\nharness = "sigma"\n')
        before = _snapshot(self.tests)
        plan = self.plan()
        self.assertEqual(plan["sigma_one"], ("sigma", None))
        self.assertEqual(plan["sigma_two"], ("sigma", None))
        self.run_fold("apply")
        main = pathlib.Path(self.tests, "sigma", "main.rs").read_text()
        self.assertEqual(main.count('#[path = "../support/road.rs"]\npub mod road;'), 1)
        one = pathlib.Path(self.tests, "sigma", "sigma_one.rs").read_text()
        self.assertIn('// harness_fold: #[path = "support/road.rs"]\n', one)
        self.assertIn("use crate::road; // harness_fold: mod road;", one)
        self.assertEqual(len([ln for ln in one.split("\n") if ln.strip() != "#[allow(unused_imports)]"]), len((path_mod + "#[test]\nfn s1() { road::go(); }\n").split("\n")))
        self.assertEqual(self.run_fold("check"), 0)
        self.run_fold("unfold")
        self.assertEqual(_snapshot(self.tests), before)

    def test_fold_splits_a_harness_when_a_module_name_means_two_files(self):
        self._support("road.rs")
        self._support("road2.rs")
        self.write("tau_a.rs", '#[path = "support/road.rs"]\nmod road;\n#[test]\nfn a() {}\n')
        self.write("tau_b.rs", '#[path = "support/road2.rs"]\nmod road;\n#[test]\nfn b() {}\n')
        self.write("tau_c.rs", "#[test]\nfn c() {}\n")
        self.write("HARNESS.toml", FILES["HARNESS.toml"] + '[[rule]]\nmatch = "^tau_"\nharness = "tau"\n')
        plan = self.plan()
        self.assertNotEqual(plan["tau_a"][0], plan["tau_b"][0])
        self.assertTrue(all(plan[s][0].startswith("tau_") for s in ("tau_a", "tau_b", "tau_c")), plan)
        self.run_fold("apply")
        self.assertEqual(self.run_fold("check"), 0)

    def test_fold_path_to_a_member_test_file_uses_the_member(self):
        self.write("upsilon.rs", '#[path = "mock_server.rs"]\nmod mock_server;\n#[test]\nfn u() {}\n')
        plan = self.plan()
        self.assertEqual(plan["upsilon"][0], plan["mock_server"][0])
        self.run_fold("apply")
        main = pathlib.Path(self.tests, plan["upsilon"][0], "main.rs").read_text()
        self.assertEqual(main.count("pub mod mock_server;"), 1)
        self.assertNotIn('"../mock_server.rs"', main)

    def test_fold_stem_not_ident_stays_its_own_binary(self):
        self.write("710_digit_first.rs", "#[test]\nfn d() {}\n")
        plan = self.plan()
        self.assertEqual(plan["710_digit_first"], ("keep", "block:stem_not_ident"))
        self.run_fold("apply")
        self.assertTrue(os.path.isfile(os.path.join(self.tests, "710_digit_first.rs")))
        self.assertEqual(self.run_fold("check"), 0)

    def test_fold_path_to_a_member_of_another_harness_points_to_its_new_place(self):
        self.write("omega_mock.rs", "#![allow(dead_code)]\npub fn m() {}\n#[test]\nfn om() {}\n")
        self.write("psi_inc.rs", '#[path = "omega_mock.rs"]\nmod omega_mock;\n#[test]\nfn p() {}\n')
        self.write("psi_alias.rs", '#[path = "omega_mock.rs"]\nmod om;\n#[test]\nfn q() {}\n')
        self.write("HARNESS.toml", FILES["HARNESS.toml"] + '[[rule]]\nmatch = "^omega_"\nharness = "omega"\n'
                   '[[rule]]\nmatch = "^psi_"\nharness = "psi"\n')
        before = _snapshot(self.tests)
        plan = self.plan()
        self.assertEqual(plan["omega_mock"], ("omega", None))
        self.assertEqual(plan["psi_inc"], ("psi", None))
        self.run_fold("apply")
        main = pathlib.Path(self.tests, "psi", "main.rs").read_text()
        self.assertIn('#[path = "../omega/omega_mock.rs"]\npub mod omega_mock;', main)
        self.assertIn('#[path = "../omega/omega_mock.rs"]\npub mod om;', main)
        self.assertNotIn('"../omega_mock.rs"', main)
        self.assertEqual(self.plan(), plan)
        self.assertEqual(self.run_fold("check"), 0)
        self.run_fold("unfold")
        self.assertEqual(_snapshot(self.tests), before)

    def test_fold_path_target_of_a_kept_file_stays_flat(self):
        self.write("omega_mock.rs", "#![allow(dead_code)]\npub fn m() {}\n#[test]\nfn om() {}\n")
        self.write("rho_demo.rs", '#[path = "omega_mock.rs"]\nmod omega_mock;\n#[test]\nfn r() {}\n')
        self.write("psi_inc.rs", '#[path = "omega_mock.rs"]\nmod omega_mock;\n#[test]\nfn p() {}\n')
        self.write("HARNESS.toml", 'max_files = 60\nkeep = ["rho_demo"]\n[[rule]]\nmatch = "^omega_"\n'
                   'harness = "omega"\n[[rule]]\nmatch = "^psi_"\nharness = "psi"\n')
        plan = self.plan()
        self.assertEqual(plan["omega_mock"], ("keep", "path_target_of:rho_demo"))
        self.assertEqual(plan["psi_inc"], ("psi", None))
        self.run_fold("apply")
        self.assertTrue(os.path.isfile(os.path.join(self.tests, "omega_mock.rs")))
        main = pathlib.Path(self.tests, "psi", "main.rs").read_text()
        self.assertIn('#[path = "../omega_mock.rs"]\npub mod omega_mock;', main)
        self.assertEqual(self.run_fold("check"), 0)

    def test_fold_path_target_brings_its_own_path_modules_along(self):
        self.write("kappa_leaf.rs", "#![allow(dead_code)]\npub fn leaf() {}\n")
        self.write("kappa_sup.rs", '#![allow(dead_code)]\n#[path = "kappa_leaf.rs"]\nmod kappa_leaf;\n'
                   "pub fn s() { kappa_leaf::leaf(); }\n")
        self.write("lambda_x.rs", '#[path = "kappa_sup.rs"]\nmod support;\n#[test]\nfn x() { support::s(); }\n')
        self.write("HARNESS.toml", FILES["HARNESS.toml"] + '[[rule]]\nmatch = "^kappa_"\nharness = "kappa"\n'
                   '[[rule]]\nmatch = "^lambda_"\nharness = "lambda"\n')
        plan = self.plan()
        self.assertEqual(plan["kappa_sup"], ("kappa", None))
        self.assertEqual(plan["lambda_x"], ("lambda", None))
        self.run_fold("apply")
        main = pathlib.Path(self.tests, "lambda", "main.rs").read_text()
        self.assertIn('#[path = "../kappa/kappa_sup.rs"]\npub mod support;', main)
        self.assertIn('#[path = "../kappa/kappa_leaf.rs"]\npub mod kappa_leaf;', main)
        self.assertEqual(self.run_fold("check"), 0)

    def test_fold_kept_path_target_keeps_its_own_path_modules(self):
        # a kept file is not rewritten: its nested `mod` stays nested, a root copy is clippy::duplicate_mod
        self.write("omega_leaf.rs", "#![allow(dead_code)]\npub fn leaf() {}\n")
        self.write("omega_sup.rs", '#![allow(dead_code)]\n#[path = "omega_leaf.rs"]\nmod omega_leaf;\n'
                   "pub fn s() { omega_leaf::leaf(); }\n#[test]\nfn os() {}\n")
        self.write("rho_demo.rs", '#[path = "omega_sup.rs"]\nmod omega_sup;\n#[test]\nfn r() {}\n')
        self.write("psi_inc.rs", '#[path = "omega_sup.rs"]\nmod omega_sup;\n#[test]\nfn p() { omega_sup::s(); }\n')
        self.write("HARNESS.toml", 'max_files = 60\nkeep = ["rho_demo"]\n[[rule]]\nmatch = "^psi_"\nharness = "psi"\n')
        self.assertEqual(self.plan()["omega_sup"][0], "keep")
        self.run_fold("apply")
        main = pathlib.Path(self.tests, "psi", "main.rs").read_text()
        self.assertIn('#[path = "../omega_sup.rs"]\npub mod omega_sup;', main)
        self.assertNotIn("omega_leaf", main)
        self.assertEqual(self.run_fold("check"), 0)

    def test_fold_helper_nested_in_another_helper_is_loaded_once(self):
        # two files of one harness load support/leaf.rs once directly, once inside support/road.rs:
        # main.rs re-exports the nested module (clippy::duplicate_mod otherwise); a private nested
        # module cannot be re-exported, so that root copy carries a targeted allow
        self._support("leaf.rs")
        self._support("hid.rs")
        self._support("road.rs", '#![allow(dead_code)]\n#[path = "leaf.rs"]\npub mod leaf;\n'
                      '#[path = "hid.rs"]\nmod hid;\npub fn go() { leaf::go(); hid::go(); }\n')
        self.write("sigma_one.rs", '#[path = "support/leaf.rs"]\nmod leaf;\n#[path = "support/hid.rs"]\nmod hid;\n'
                   "#[test]\nfn s1() { leaf::go(); hid::go(); }\n")
        self.write("sigma_two.rs", '#[path = "support/road.rs"]\nmod road;\n#[test]\nfn s2() { road::go(); }\n')
        self.write("HARNESS.toml", FILES["HARNESS.toml"] + '[[rule]]\nmatch = "^sigma_"\nharness = "sigma"\n')
        before = _snapshot(self.tests)
        self.run_fold("apply")
        main = pathlib.Path(self.tests, "sigma", "main.rs").read_text()
        self.assertIn("pub use road::leaf;", main)
        self.assertNotIn('#[path = "../support/leaf.rs"]', main)
        self.assertIn('#[path = "../support/road.rs"]\npub mod road;', main)
        self.assertIn('#[allow(clippy::duplicate_mod)]\n#[path = "../support/hid.rs"]\npub mod hid;', main)
        self.assertEqual(main.count("clippy::duplicate_mod"), 1)
        self.assertEqual(self.run_fold("check"), 0)
        self.run_fold("unfold")
        self.assertEqual(_snapshot(self.tests), before)

    def test_fold_keeps_binaries_a_nextest_override_names(self):
        os.makedirs(os.path.join(self.repo, ".config"))
        with open(os.path.join(self.repo, ".config", "nextest.toml"), "w") as fh:
            fh.write('[[profile.ci.overrides]]\nfilter = "binary_id(=fx::alpha_two) + binary_id(/_slow$/)"\n'
                     'slow-timeout = "60s"\n')
        self.write("beta_slow.rs", "#[test]\nfn s() {}\n")
        plan = self.plan()
        self.assertEqual(plan["alpha_two"], ("keep", "nextest_override"))
        self.assertEqual(plan["beta_slow"], ("keep", "nextest_override"))
        self.assertEqual(plan["alpha_one"], ("alpha", None))

    def test_fold_keeps_files_the_export_never_ships(self):
        """A file the export blocklists leaves the public tree; folded, its
        harness `main.rs` would still say `pub mod <stem>;` there and the
        public build breaks. So it stays its own binary."""
        os.makedirs(os.path.join(self.repo, "plans", "export-fixtures"))
        with open(os.path.join(self.repo, "plans", "export-fixtures", "make_export.py"), "w") as fh:
            fh.write('BLOCKLIST = {\n    "crates/fx/tests/somewhere/alpha_two.rs":\n'
                     '        "private " "drift lock",\n    "docs/x.md": "y",\n}\n')
        plan = self.plan()
        self.assertEqual(plan["alpha_two"], ("keep", "export_private"))
        self.assertEqual(plan["alpha_one"], ("alpha", None))

    def test_fold_keeps_files_a_public_template_readme_links(self):
        """A public template README links `../../crates/<c>/tests/<stem>.rs`;
        folded, the link is dead in the export (R10, W5 delta B-C1) and
        templates/ stays untouched (OR-DT.W5.doc) -- so the file stays flat.
        A non-public template's link keeps nothing."""
        os.makedirs(os.path.join(self.repo, "plans", "export-fixtures"))
        with open(os.path.join(self.repo, "plans", "export-fixtures", "make_export.py"), "w") as fh:
            fh.write('PUBLIC_TEMPLATES = [\n    "pub-tpl",\n]\n')
        for tpl, stem in (("pub-tpl", "alpha_two"), ("private-tpl", "alpha_one")):
            os.makedirs(os.path.join(self.repo, "templates", tpl))
            with open(os.path.join(self.repo, "templates", tpl, "README.md"), "w") as fh:
                fh.write(f"See [the lock](../../crates/fx/tests/{stem}.rs).\n")
        plan = self.plan()
        self.assertEqual(plan["alpha_two"], ("keep", "doc_linked"))
        self.assertEqual(plan["alpha_one"], ("alpha", None))

    def test_fold_keeps_readme_linked_files_in_the_export_tree(self):
        """The export tree ships no `plans/` and exactly the public templates
        (GH #1267: 13 linked files folded into `misc` there, its parts went
        from eight to ten and the nextest overrides of `misc_<n>` named no test
        in the public CI). Without the export script every template README is
        public -- the plan of the export tree is the plan of the source tree."""
        os.makedirs(os.path.join(self.repo, "templates", "any-tpl"))
        with open(os.path.join(self.repo, "templates", "any-tpl", "README.md"), "w") as fh:
            fh.write("See [the lock](../../crates/fx/tests/alpha_two.rs).\n")
        plan = self.plan()
        self.assertEqual(plan["alpha_two"], ("keep", "doc_linked"))
        self.assertEqual(plan["alpha_one"], ("alpha", None))

    def test_fold_harness_name_must_not_match_a_nextest_override(self):
        """A harness named into a `binary_id(/re/)` override would inherit its
        timeouts and retries (W5 delta M5)."""
        os.makedirs(os.path.join(self.repo, ".config"))
        with open(os.path.join(self.repo, ".config", "nextest.toml"), "w") as fh:
            fh.write('[[profile.ci.overrides]]\nfilter = "binary_id(/::alpha$/)"\nslow-timeout = "60s"\n')
        with self.assertRaises(SystemExit):
            self.plan()

    def test_fold_writes_rustfmt_clean_files_in_a_workspace(self):
        """The strand gate's fmt station rejected 300+ folded files: the fold
        wrote `#[allow(unused_imports)] use crate::x;` on one line and main.rs in
        ASCII order (W5 delta, PRE fmt RED). In a workspace the fold formats what
        it writes, `--check` compares formatted, and unfold still gives the flat
        file back."""
        self._fold_is_rustfmt_clean(hide_rustfmt=False)

    def test_fold_writes_rustfmt_clean_files_without_rustfmt_on_path(self):
        """#1134: the fold's own text is already what rustfmt writes; a fold
        on a host without rustfmt leaves no fmt finding behind (rustfmt only
        judges the result here)."""
        self._fold_is_rustfmt_clean(hide_rustfmt=True)

    def _fold_is_rustfmt_clean(self, hide_rustfmt):
        if shutil.which("rustfmt") is None:
            self.skipTest("no rustfmt to judge the result")
        import subprocess
        with open(os.path.join(self.repo, "Cargo.toml"), "w") as fh:
            fh.write('[workspace]\nmembers = ["crates/fx"]\n[workspace.package]\nedition = "2024"\n')
        self.write("alpha_one.rs", "//! Uses the helper.\nmod common;\n\n#[test]\nfn one() {\n    common::help();\n}\n")
        self.write("alpha_two.rs", "#[test]\nfn two() {}\n\n#[tokio::test]\nasync fn three() {}\n")
        self.write("alpha_10.rs", "#[test]\nfn ten() {}\n")
        self.write("alpha_9.rs", "#[test]\nfn nine() {}\n")
        for p in pathlib.Path(self.tests).rglob("*.rs"):  # the tree starts formatted, as cargo fmt keeps it
            r = subprocess.run(["rustfmt", "--edition", "2024"], input=p.read_text(),
                               capture_output=True, text=True)
            if r.returncode == 0 and r.stdout:
                p.write_text(r.stdout)
        before = _snapshot(self.tests)
        with mock.patch.dict(os.environ, {"PATH": ""} if hide_rustfmt else {}):
            self.run_fold("apply")
            self.assertEqual(self.run_fold("check"), 0)
        files = [str(p) for p in pathlib.Path(self.tests, "alpha").glob("*.rs")]
        r = subprocess.run(["rustfmt", "--edition", "2024", "--check", *files],
                           capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stdout[-2000:])
        self.run_fold("unfold")
        self.assertEqual(_snapshot(self.tests), before)

    def test_fold_path_mod_whose_target_is_missing_stays_blocked(self):
        self.assertEqual(self.plan()["gamma_path"], ("keep", "block:path"))
        self.write("phi_escape.rs", '#[path = "../src/lib.rs"]\nmod lib;\n')
        self.assertEqual(self.plan()["phi_escape"], ("keep", "block:path"))

    def test_fold_rule_uses_routes_a_file_by_the_helper_it_declares(self):
        self._support("hive.rs")
        self.write("chi_x.rs", '#[path = "support/hive.rs"]\nmod hive;\n#[test]\nfn x() {}\n')
        self.write("HARNESS.toml", 'max_files = 60\nkeep = []\n[[rule]]\nuses = "^support/hive\\\\.rs$"\n'
                   'harness = "hive"\n[[rule]]\nmatch = "^alpha_"\nharness = "alpha"\n')
        plan = self.plan()
        self.assertEqual(plan["chi_x"], ("hive", None))
        self.assertEqual(plan["alpha_one"], ("alpha", None))  # common/mod.rs does not match

    def test_nextest_list_count_unchanged_after_fold(self):
        before = _tests_attr_count(self.tests)
        self.run_fold("apply")
        self.assertEqual(_tests_attr_count(self.tests), before)
        self.run_fold("unfold")
        self.assertEqual(_tests_attr_count(self.tests), before)


class DocPathMentions(unittest.TestCase):
    """Test paths named in code spans of templates/ and docs/ (W5 re-check N3,
    beside R10, which sees links only)."""

    def test_doc_mentions_a_dead_name_is_found_a_live_or_frozen_historical_one_is_not(self):
        repo = pathlib.Path(tempfile.mkdtemp(prefix="hf-mentions-"))
        self.addCleanup(shutil.rmtree, repo, ignore_errors=True)
        (repo / "crates" / "fx" / "tests" / "misc").mkdir(parents=True)
        (repo / "crates" / "fx" / "tests" / "misc" / "gh5_a.rs").write_text("")
        (repo / "templates" / "t").mkdir(parents=True)
        (repo / "templates" / "t" / "README.md").write_text(
            "Pinned by `crates/fx/tests/misc/gh5_a.rs` and `crates/fx/tests/gh5_a.rs`;\n"
            "also `../../crates/fx/tests/misc/gh5_a.rs` and `../../crates/fx/tests/gh6.rs`.\n")
        (repo / "docs" / "superpowers" / "plans").mkdir(parents=True)
        (repo / "docs" / "superpowers" / "plans" / "old.md").write_text(
            "`crates/fx/tests/gone.rs`, again `crates/fx/tests/gone.rs`, `crates/fx/tests/new.rs`\n")
        (repo / "docs" / "superpowers" / "specs").mkdir()
        (repo / "docs" / "superpowers" / "specs" / "old.md").write_text("`crates/fx/tests/gone.rs`\n")
        (repo / "docs" / "a.md").write_text("`crates/fx/tests/gone.rs` and `tests/<h>/main.rs`\n")
        # The historical docs are excused by a FROZEN list (doc, path) -> count, not
        # by their directory (W5 re-check, Koordinator 09.10.): a mention beyond
        # the frozen count, a new path or an unlisted doc there is RED.
        frozen = {("docs/superpowers/plans/old.md", "crates/fx/tests/gone.rs"): 1}
        self.assertEqual(hf.dead_path_mentions(str(repo), frozen=frozen), [
            ("templates/t/README.md", "crates/fx/tests/gh5_a.rs"),
            ("templates/t/README.md", "../../crates/fx/tests/gh6.rs"),
            ("docs/a.md", "crates/fx/tests/gone.rs"),
            ("docs/superpowers/plans/old.md", "crates/fx/tests/gone.rs"),
            ("docs/superpowers/plans/old.md", "crates/fx/tests/new.rs"),
            ("docs/superpowers/specs/old.md", "crates/fx/tests/gone.rs"),
        ])

    def test_non_md_text_files_are_read_whole_and_binary_ones_are_skipped(self):
        # W5 OR-DT.W5.tpl rest: 29 dead mentions sat in config.json, template.json,
        # docs.jsonl and canvy .js/.py, where no code span marks a path.
        repo = pathlib.Path(tempfile.mkdtemp(prefix="hf-mentions-"))
        self.addCleanup(shutil.rmtree, repo, ignore_errors=True)
        (repo / "crates" / "fx" / "tests" / "misc").mkdir(parents=True)
        (repo / "crates" / "fx" / "tests" / "misc" / "gh5_a.rs").write_text("")
        (repo / "templates" / "t" / "seed").mkdir(parents=True)
        (repo / "templates" / "t" / "config.json").write_text(
            '{"note": "pinned by crates/fx/tests/gh5_a.rs and crates/fx/tests/misc/gh5_a.rs"}\n')
        (repo / "templates" / "t" / "seed" / "docs.jsonl").write_text(
            '{"text": "see crates/fx/tests/gone.rs"}\n')
        (repo / "templates" / "t" / "layout.js").write_text(
            "// pinned by ../../crates/fx/tests/gh6.rs\n")
        (repo / "templates" / "t" / "layout.py").write_text(
            "# pinned by ../../crates/fx/tests/misc/gh5_a.rs\n")
        (repo / "templates" / "t" / "icon.png").write_bytes(
            b"\x89PNG\xff\xfe crates/fx/tests/gone.rs")
        (repo / "docs").mkdir()
        (repo / "docs" / "tool.py").write_text("# crates/fx/tests/gone.rs\n")
        self.assertEqual(sorted(hf.dead_path_mentions(str(repo), frozen={})), [
            ("docs/tool.py", "crates/fx/tests/gone.rs"),
            ("templates/t/config.json", "crates/fx/tests/gh5_a.rs"),
            ("templates/t/layout.js", "../../crates/fx/tests/gh6.rs"),
            ("templates/t/seed/docs.jsonl", "crates/fx/tests/gone.rs"),
        ])

    def test_the_frozen_historical_list_holds_todays_124_mentions_and_nothing_else(self):
        self.assertEqual(sum(hf.FROZEN_HISTORICAL.values()), 124)
        self.assertTrue(all(doc.startswith(hf.HISTORICAL_DOCS) and n > 0
                            for (doc, _), n in hf.FROZEN_HISTORICAL.items()))

    def test_stale_frozen_names_entries_beyond_the_dead_count(self):
        """W5 re-check 4 M1: an entry whose path lives again or whose
        mention went away would later excuse a NEW dead mention silently."""
        import tempfile
        with tempfile.TemporaryDirectory() as tmp:
            doc = pathlib.Path(tmp, "docs/superpowers/plans/old.md")
            doc.parent.mkdir(parents=True)
            doc.write_text("Pinned by `crates/fx/tests/gone.rs`.\n")
            rel, path = "docs/superpowers/plans/old.md", "crates/fx/tests/gone.rs"
            self.assertEqual([], hf.stale_frozen(tmp, {(rel, path): 1}))
            self.assertEqual([(rel, path, 2, 1)], hf.stale_frozen(tmp, {(rel, path): 2}))
            self.assertEqual([("docs/superpowers/plans/gone.md", path, 1, 0)],
                             hf.stale_frozen(tmp, {("docs/superpowers/plans/gone.md", path): 1}))
            live = pathlib.Path(tmp, path)
            live.parent.mkdir(parents=True)
            live.write_text("#[test]\nfn t() {}\n")
            self.assertEqual([(rel, path, 1, 0)], hf.stale_frozen(tmp, {(rel, path): 1}))

    @private_tree.private()   # GH #1199: the historical docs never travel
    def test_the_frozen_historical_list_is_not_stale_in_the_real_tree(self):
        repo = SCRIPTS.parent
        self.assertEqual([], hf.stale_frozen(str(repo)))

    @private_tree.private()   # GH #1199: the export blocklists tests the READMEs name
    def test_doc_mentions_the_real_tree_names_no_dead_test_path(self):
        repo = SCRIPTS.parent
        dead = hf.dead_path_mentions(str(repo))
        self.assertEqual(dead, [], "%d dead, first: %r" % (len(dead), dead[:5]))


if __name__ == "__main__":
    unittest.main()
