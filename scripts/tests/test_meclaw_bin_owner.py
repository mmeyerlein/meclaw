"""GH #1168: one `meclaw` binary target, and the release names its crate.

Before #1168 both `crates/meclaw` and `crates/meclaw-cli` declared
`[[bin]] name = "meclaw"`. Both linked to `target/debug/meclaw`, so which one a
test started through `CARGO_BIN_EXE_meclaw` was not fixed, and every change in
the cli chain paid a second full binary link (S6 measured 1.3 s alone,
2.7-3.0 s under load). The release recipe built `-p meclaw-cli` while the
`meclaw` package called itself the shipped binary.

The lock is `bin-owner-lock` in `scripts/pipeline/locks.py`: it reads the
manifests with tomllib and the two recipes as text -- no cargo, milliseconds --
and fails closed: a `cargo build` or `cargo install` line that names no
literal crate (`-p "$PKG"`, `--workspace`, `${CRATE}`) is red, not skipped. It
runs in `precheck` and `strand.sh pre` on every diff at a manifest, a
`src/main.rs`, `src/bin/` or one of the two recipes. It used to run here, in
`gate-selftest`, through a class of its own; that made every manifest diff --
the merge's version bump included -- plan the ~326 s station (tb-s6b tempo
correction). This module still runs in `gate-selftest` like every kit test.
"""

import pathlib
import sys
import tempfile
import unittest
from scripts.tests import store_isolation

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
if str(SCRIPTS) not in sys.path:
    sys.path.insert(0, str(SCRIPTS))

from pipeline import locks  # noqa: E402
import precheck  # noqa: E402

REPO = pathlib.Path(__file__).resolve().parents[2]
WORKSPACE_MANIFEST = "Cargo.toml"
RELEASE_RECIPE = locks.BIN_RELEASE_RECIPE
SOURCE_INSTALL = locks.BIN_SOURCE_INSTALL
BIN = locks.BIN_NAME

# The reader exists once, in the lock; these tests pin it.
binary_targets = locks.binary_targets
bin_owners = locks.bin_owners
cli_binaries = locks.cli_binaries
release_build_crates = locks.release_build_crates
source_install_crates = locks.source_install_crates


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


class TestTheTree(unittest.TestCase):
    """The committed workspace and recipes."""

    def test_the_workspace_has_exactly_one_meclaw_binary(self):
        self.assertEqual(len(bin_owners(REPO)), 1,
                         "packages declaring a `meclaw` binary: %s" % bin_owners(REPO))

    def test_the_cli_library_builds_no_binary_of_its_own(self):
        self.assertEqual(cli_binaries(REPO), [])

    def test_the_release_builds_the_crate_that_owns_the_binary(self):
        owners = bin_owners(REPO)
        self.assertEqual(len(owners), 1, "packages declaring a `meclaw` binary: %s" % owners)
        text = (REPO / RELEASE_RECIPE).read_text(encoding="utf-8")
        self.assertEqual(set(release_build_crates(text)), set(owners))

    def test_the_source_install_names_the_crate_that_owns_the_binary(self):
        owners = bin_owners(REPO)
        self.assertEqual(len(owners), 1, "packages declaring a `meclaw` binary: %s" % owners)
        text = (REPO / SOURCE_INSTALL).read_text(encoding="utf-8")
        self.assertEqual(set(source_install_crates(text)), set(owners))


class TestTheReader(unittest.TestCase):
    """The reader on trees written for the test: what it must see."""

    def write_tree(self, crates):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = pathlib.Path(tmp.name)
        members = []
        for name, (manifest_tail, main_rs) in crates.items():
            crate_dir = root / "crates" / name
            (crate_dir / "src").mkdir(parents=True)
            (crate_dir / "Cargo.toml").write_text(
                '[package]\nname = "%s"\nversion = "0.1.0"\n%s' % (name, manifest_tail),
                encoding="utf-8")
            if main_rs:
                (crate_dir / "src" / "main.rs").write_text("fn main() {}\n", encoding="utf-8")
            members.append('"crates/%s"' % name)
        (root / WORKSPACE_MANIFEST).write_text(
            "[workspace]\nmembers = [%s]\n" % ", ".join(members), encoding="utf-8")
        return root

    def add_source(self, root, crate, rel):
        path = root / "crates" / crate / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("fn main() {}\n", encoding="utf-8")

    def test_two_declared_meclaw_binaries_are_two_owners(self):
        bin_decl = '[[bin]]\nname = "meclaw"\npath = "src/main.rs"\n'
        root = self.write_tree({"a": (bin_decl, True), "b": (bin_decl, True)})
        self.assertEqual(bin_owners(root), ["a", "b"])

    def test_an_undeclared_main_rs_is_a_binary_under_the_package_name(self):
        root = self.write_tree({"meclaw": ("", True), "lib-only": ("", False)})
        self.assertEqual(bin_owners(root), ["meclaw"])
        self.assertIn(("meclaw", "meclaw"), binary_targets(root))

    def test_autobins_false_hides_the_main_rs(self):
        root = self.write_tree({"meclaw": ("autobins = false\n", True)})
        self.assertEqual(bin_owners(root), [])

    def test_an_undeclared_cli_main_rs_is_a_cli_binary(self):
        bin_decl = '[[bin]]\nname = "meclaw"\npath = "src/main.rs"\n'
        root = self.write_tree({"meclaw": (bin_decl, True), "meclaw-cli": ("", True)})
        self.assertEqual(cli_binaries(root), ["meclaw-cli"])
        self.assertEqual(bin_owners(root), ["meclaw"])

    def test_autobins_false_keeps_the_cli_main_rs_out(self):
        root = self.write_tree({"meclaw-cli": ("autobins = false\n", True)})
        self.assertEqual(cli_binaries(root), [])

    def test_a_bin_file_named_meclaw_is_a_second_owner(self):
        root = self.write_tree({"meclaw": ("", True), "meclaw-cells": ("", False)})
        self.add_source(root, "meclaw-cells", "src/bin/meclaw.rs")
        self.assertEqual(bin_owners(root), ["meclaw", "meclaw-cells"])

    def test_a_bin_directory_named_meclaw_is_a_second_owner(self):
        root = self.write_tree({"meclaw": ("", True), "meclaw-cells": ("", False)})
        self.add_source(root, "meclaw-cells", "src/bin/meclaw/main.rs")
        self.assertEqual(bin_owners(root), ["meclaw", "meclaw-cells"])
        self.assertIn(("meclaw-cells", "meclaw"), binary_targets(root))

    def test_a_bin_directory_without_main_rs_is_no_binary(self):
        root = self.write_tree({"meclaw-cells": ("", False)})
        self.add_source(root, "meclaw-cells", "src/bin/helper/util.rs")
        self.assertEqual(binary_targets(root), [])

    def test_a_declared_bin_is_not_counted_twice(self):
        bin_decl = '[[bin]]\nname = "meclaw"\npath = "src/bin/meclaw/main.rs"\n'
        root = self.write_tree({"meclaw": (bin_decl, False)})
        self.add_source(root, "meclaw", "src/bin/meclaw/main.rs")
        self.assertEqual(bin_owners(root), ["meclaw"])

    def test_a_release_line_for_another_crate_is_seen(self):
        text = 'run: cargo build --release --target "$TARGET" -p meclaw-cli\n'
        self.assertEqual(release_build_crates(text), ["meclaw-cli"])

    def test_a_release_without_a_package_fails_closed(self):
        with self.assertRaises(ValueError):
            release_build_crates("run: cargo build --release\n")

    def test_every_release_build_line_needs_a_literal_package(self):
        good = 'run: cargo build --release --target "$TARGET" -p meclaw\n'
        for second in ('run: cargo build --release -p "$PKG"\n',
                       "run: cargo build --release -p ${{ matrix.pkg }}\n",
                       "run: cargo build --release --workspace\n",
                       "run: cargo build --release\n",
                       "run: cargo build -p meclaw -p meclaw-cli\n"):
            with self.subTest(second=second), self.assertRaises(ValueError):
                release_build_crates(good + second)

    def test_a_release_package_on_a_continuation_line_is_read(self):
        text = 'run: cargo build --release \\\n          -p meclaw-cli\n'
        self.assertEqual(release_build_crates(text), ["meclaw-cli"])

    def test_the_package_may_be_written_with_an_equals_sign(self):
        self.assertEqual(release_build_crates("run: cargo build --package=meclaw\n"),
                         ["meclaw"])

    def test_the_install_line_names_its_crate_after_the_git_url(self):
        line = 'say "    cargo install --git https://github.com/${REPO} meclaw"\n'
        self.assertEqual(source_install_crates(line), ["meclaw"])

    def test_an_install_line_with_a_variable_crate_fails_closed(self):
        for line in ('say "cargo install --git https://example.invalid/r ${CRATE}"\n',
                     'say "cargo install --git https://example.invalid/r $CRATE"\n'):
            with self.subTest(line=line), self.assertRaises(ValueError):
                source_install_crates(line)

    def test_every_install_line_needs_a_literal_crate(self):
        # m-N2: a literal line does not excuse a second one beside it.
        good = 'say "    cargo install --git https://example.invalid/r meclaw"\n'
        for second in ('say "cargo install --git https://example.invalid/r ${CRATE}"\n',
                       'say "cargo install --git https://example.invalid/r"\n',
                       'say "cargo install --git https://example.invalid/r meclaw meclaw-cli"\n'):
            with self.subTest(second=second), self.assertRaises(ValueError):
                source_install_crates(good + second)

    def test_an_install_line_without_a_crate_fails_closed(self):
        with self.assertRaises(ValueError):
            source_install_crates('say "cargo install --git https://example.invalid/r"\n')
        with self.assertRaises(ValueError):
            source_install_crates("echo nothing to install\n")



class TestTheLock(unittest.TestCase):
    """`bin-owner-lock` of `scripts/pipeline/locks.py`: the same reader as a
    cheap lock in `precheck` and `strand.sh pre`, on every diff at one of its
    inputs (tb-s6b tempo correction: no `gate-selftest` for a manifest diff)."""

    RELEASE = 'run: cargo build --release --target "$TARGET" -p %s\n'
    INSTALL = 'say "    cargo install --git https://example.invalid/r %s"\n'

    def write_tree(self, owners, release="meclaw", install="meclaw", cli=None):
        """`cli`: the manifest tail of a third member `meclaw-cli` with its
        own `src/main.rs`; None leaves the cli out."""
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = pathlib.Path(tmp.name)
        bin_decl = '[[bin]]\nname = "meclaw"\npath = "src/main.rs"\n'
        members = []
        names = ("meclaw", "meclaw-cells") + ((locks.BIN_CLI,) if cli is not None else ())
        for name in names:
            crate_dir = root / "crates" / name
            (crate_dir / "src").mkdir(parents=True)
            tail = (bin_decl if name in owners
                    else cli if name == locks.BIN_CLI else "autobins = false\n")
            (crate_dir / "Cargo.toml").write_text(
                '[package]\nname = "%s"\nversion = "0.1.0"\n%s' % (name, tail),
                encoding="utf-8")
            (crate_dir / "src" / "main.rs").write_text("fn main() {}\n", encoding="utf-8")
            members.append('"crates/%s"' % name)
        (root / WORKSPACE_MANIFEST).write_text(
            "[workspace]\nmembers = [%s]\n" % ", ".join(members), encoding="utf-8")
        (root / ".github" / "workflows").mkdir(parents=True)
        (root / RELEASE_RECIPE).write_text(self.RELEASE % release, encoding="utf-8")
        (root / "scripts").mkdir()
        (root / SOURCE_INSTALL).write_text(self.INSTALL % install, encoding="utf-8")
        return root

    @staticmethod
    def lock_findings(files, root):
        return [f for f in precheck.run(files, repo=str(root)) if f.check == "bin-owner-lock"]

    INPUTS = ("Cargo.toml", "crates/meclaw-cells/Cargo.toml", RELEASE_RECIPE,
              SOURCE_INSTALL, "crates/meclaw-cells/src/main.rs",
              "crates/meclaw-cells/src/bin/meclaw.rs",
              "crates/meclaw-cells/src/bin/meclaw/main.rs")

    def test_the_lock_is_registered(self):
        self.assertIn("bin-owner-lock", [name for name, _ in locks.LOCKS])

    def test_precheck_is_red_on_a_second_meclaw_binary_at_every_input(self):
        root = self.write_tree(owners=("meclaw", "meclaw-cells"))
        for path in self.INPUTS:
            with self.subTest(path=path):
                found = self.lock_findings([path], root)
                self.assertTrue(found, "no bin-owner-lock finding for %s" % path)
                self.assertTrue(all(f.level == "RED" for f in found), found)
                self.assertTrue(any("meclaw-cells" in f.message for f in found), found)

    def test_a_tree_with_one_owner_and_matching_recipes_is_green(self):
        root = self.write_tree(owners=("meclaw",))
        for path in self.INPUTS:
            with self.subTest(path=path):
                self.assertEqual(self.lock_findings([path], root), [])

    def test_a_recipe_that_builds_another_crate_is_red(self):
        for release, install in (("meclaw-cells", "meclaw"), ("meclaw", "meclaw-cells")):
            with self.subTest(release=release, install=install):
                root = self.write_tree(owners=("meclaw",), release=release, install=install)
                found = self.lock_findings([RELEASE_RECIPE, SOURCE_INSTALL], root)
                self.assertTrue(found)
                self.assertTrue(all(f.level == "RED" for f in found), found)

    def test_an_unreadable_recipe_fails_closed(self):
        root = self.write_tree(owners=("meclaw",), release='"$PKG"')
        found = self.lock_findings([RELEASE_RECIPE], root)
        self.assertTrue(found)
        self.assertTrue(all(f.level == "RED" for f in found), found)
        (root / SOURCE_INSTALL).unlink()
        self.assertTrue(self.lock_findings(["Cargo.toml"], root))

    def test_a_cli_main_rs_without_autobins_false_is_red(self):
        # M-N1 (Nach-Check e5a1341e1): the cli builds no binary. Its implicit
        # `src/main.rs` is named `meclaw-cli`, so the owner count stays 1 --
        # only the cli rule sees it.
        cli_manifest = "crates/%s/Cargo.toml" % locks.BIN_CLI
        root = self.write_tree(owners=("meclaw",), cli="")
        for path in (cli_manifest, "crates/%s/src/main.rs" % locks.BIN_CLI):
            with self.subTest(path=path):
                found = self.lock_findings([path], root)
                self.assertEqual(len(found), 1, found)
                self.assertEqual(found[0].level, "RED", found)
                self.assertIn(cli_manifest, found[0].message)
                self.assertIn("builds binaries %s" % locks.BIN_CLI, found[0].message)
        root = self.write_tree(owners=("meclaw",), cli="autobins = false\n")
        self.assertEqual(self.lock_findings([cli_manifest], root), [])

    def test_an_unreadable_manifest_fails_closed(self):
        # m-N1: a manifest tomllib cannot read is RED, never an empty answer.
        root = self.write_tree(owners=("meclaw",))
        (root / "crates" / "meclaw-cells" / "Cargo.toml").write_text("[package\n", encoding="utf-8")
        found = self.lock_findings(["crates/meclaw-cells/Cargo.toml"], root)
        self.assertTrue(found)
        self.assertTrue(all(f.level == "RED" for f in found), found)
        self.assertTrue(any("cannot read the binary targets" in f.message for f in found), found)

    def test_a_member_glob_fails_closed(self):
        # m-N1: the reader does not expand a glob. With the owner listed
        # explicitly the owner count is 1 -- a skipped glob would be green.
        root = self.write_tree(owners=("meclaw",))
        (root / WORKSPACE_MANIFEST).write_text(
            '[workspace]\nmembers = ["crates/meclaw", "crates/meclaw-c*"]\n', encoding="utf-8")
        found = self.lock_findings([WORKSPACE_MANIFEST], root)
        self.assertTrue(found)
        self.assertTrue(all(f.level == "RED" for f in found), found)
        self.assertTrue(any("member glob" in f.message for f in found), found)

    def test_a_diff_elsewhere_does_not_run_it(self):
        root = self.write_tree(owners=("meclaw", "meclaw-cells"))
        for path in ("docs/x.md", "crates/meclaw-cells/src/lib.rs", ".github/workflows/ci.yml"):
            with self.subTest(path=path):
                self.assertEqual(locks.check_bin_owner_lock([path], str(root)), [])

    def test_a_tree_without_a_workspace_says_nothing(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.assertEqual(locks.check_bin_owner_lock(["Cargo.toml"], tmp.name), [])

    def test_the_committed_tree_is_green(self):
        self.assertEqual(locks.check_bin_owner_lock(list(self.INPUTS), str(REPO)), [])


if __name__ == "__main__":
    unittest.main()
