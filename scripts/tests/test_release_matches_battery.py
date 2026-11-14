"""`scripts/release_matches_battery.py`: a release commit may differ from the
commit the battery measured (`core_commit` in the battery receipt) only in
version and CHANGELOG places (wave live-2, OR-LV-12, GH #1358), and its
`templates` tree must be the one the battery measured (`templates_tree`, G8a,
GH #1390).

Every case builds a throw-away git repository in a temporary directory: a base
commit that stands for the measured candidate, then a release commit on top.

Run: `scripts/strand.sh test --unittest scripts.tests.test_release_matches_battery`.
"""
import pathlib
import subprocess
import sys
import tempfile
import unittest

from scripts.tests import store_isolation

SCRIPT = pathlib.Path(__file__).resolve().parent.parent / "release_matches_battery.py"

BASE = {
    "Cargo.toml": (
        '[workspace]\nmembers = ["crates/*"]\n\n'
        '[workspace.package]\nversion = "0.76.6"\nedition = "2021"\n'
    ),
    "crates/meclaw-cells/Cargo.toml": (
        '[package]\nname = "meclaw-cells"\nversion.workspace = true\n\n'
        '[dependencies]\n'
        'meclaw-core = { path = "../meclaw-core", version = "0.76.6" }\n'
        'tokio = { version = "1", features = ["full"] }\n'
    ),
    "Cargo.lock": (
        '[[package]]\nname = "meclaw-cells"\nversion = "0.76.6"\n'
        'dependencies = [\n "meclaw-core",\n]\n'
    ),
    "CHANGELOG.md": "# Changelog\n\n## [Unreleased]\n\n- a fix\n",
    "templates/voice/template.json": (
        '{\n  "name": "voice",\n  "version": "2.7.0",\n  "description": "voice"\n}\n'
    ),
    "templates/voice/config.json": '{\n  "sample_rate": 16000\n}\n',
    "crates/meclaw-cells/src/lib.rs": "pub fn answer() -> u32 { 42 }\n",
    "ROADMAP.md": "# Roadmap\n",
    "docs/status.en.md": "Tests: 11633\n",
}


def git(repo, *args):
    return subprocess.run(
        ["git", "-C", str(repo), "-c", "user.name=kit", "-c", "user.email=kit@example.invalid",
         "-c", "commit.gpgsign=false", *args],
        capture_output=True, text=True, check=True).stdout.strip()


def write(repo, files):
    for rel, text in files.items():
        path = repo / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


class ReleaseMatchesBattery(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = pathlib.Path(self._tmp.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        git(self.repo, "init", "-q", "-b", "master")
        write(self.repo, BASE)
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-q", "-m", "candidate")
        self.core = git(self.repo, "rev-parse", "HEAD")

    def release(self, files):
        """A release commit on top of the candidate, changing `files`."""
        write(self.repo, files)
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-q", "-m", "release")

    def tree(self, rev):
        return git(self.repo, "rev-parse", rev + ":templates")

    def receipt(self, body=None):
        path = self.root / "receipt.md"
        if body is None:
            body = (
                "# Battery receipt\n\n## Round 1\n\n"
                "core_commit: %s\n"
                "lab_binary_sha256: %s\n"
                "manifest_sha256: %s manifests/soul2me-colony-0.45-rc/08-freeswitch.json\n"
                "templates_tree: %s\n"
                % (self.core, "a" * 64, "b" * 64, self.tree(self.core)))
        path.write_text(body)
        return path

    def run_check(self, receipt, *extra):
        return subprocess.run(
            [sys.executable, "-I", str(SCRIPT), str(receipt), "--repo", str(self.repo), *extra],
            capture_output=True, text=True)

    def bump(self):
        return {
            "Cargo.toml": BASE["Cargo.toml"].replace('"0.76.6"', '"0.77.0"'),
            "crates/meclaw-cells/Cargo.toml":
                BASE["crates/meclaw-cells/Cargo.toml"].replace('"0.76.6"', '"0.77.0"'),
            "Cargo.lock": BASE["Cargo.lock"].replace('"0.76.6"', '"0.77.0"'),
            "CHANGELOG.md": BASE["CHANGELOG.md"].replace(
                "## [Unreleased]\n", "## [Unreleased]\n\n## [0.77.0] - 2026-10-30\n"),
        }

    def test_a_version_and_changelog_release_is_clean(self):
        self.release(self.bump())
        res = self.run_check(self.receipt())
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn(self.core, res.stdout)
        # The measured hashes are passed through for the release receipt.
        self.assertIn("lab_binary_sha256: " + "a" * 64, res.stdout)
        self.assertIn("08-freeswitch.json", res.stdout)

    def test_the_release_cut_records_are_allowed(self):
        # The release records (RT2 fix round 1, `turnaround.release_offending`):
        # the status test count and a wave's berichte/straenge/receipts.
        files = self.bump()
        files.update({
            "docs/status.en.md": "Tests: 11700\n",
            "plans/live-2-2026-10-29/berichte/rel-0770.md": "# rel-0770\n",
            "plans/live-2-2026-10-29/straenge/rel-0770.json": "{}\n",
            "plans/live-2-2026-10-29/receipts/bat-r2b.md": "# bat\n",
        })
        self.release(files)
        res = self.run_check(self.receipt())
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)

    def test_roadmap_and_other_plans_paths_are_foreign(self):
        # Review C1: a wave ledger and the exporter under plans/ are no release
        # record, and neither is a ROADMAP.md change other than a new release's
        # own `- vX.Y.Z:` line under § Shipped (here: no § Shipped at all).
        # 0.77.0 wrote ROADMAP.md and the ledger after the battery
        # (6946a6ab..064ec541c): fail closed.
        files = self.bump()
        files.update({
            "ROADMAP.md": "# Roadmap\n\n- 0.77.0\n",
            "plans/live-2-2026-10-29/ledger.md": "# ledger\n",
            "plans/export-fixtures/make_export.py": "print(2)\n",
        })
        self.release(files)
        res = self.run_check(self.receipt())
        self.assertEqual(1, res.returncode, res.stdout + res.stderr)
        for path in files:
            if path in ("ROADMAP.md", "plans/live-2-2026-10-29/ledger.md",
                        "plans/export-fixtures/make_export.py"):
                self.assertIn("foreign: " + path, res.stdout)

    def test_a_shipped_line_in_the_roadmap_matches_the_battery(self):
        # GH #1518: `release.sh prepare` commits the release's line under
        # ROADMAP.md § Shipped on the cut, after the battery measured it. That
        # line is a release record; any other ROADMAP.md change stays foreign.
        roadmap = "# Roadmap\n\n## Now\n\n- a stream\n\n## Shipped\n\n- v0.76.6: the patch.\n"
        write(self.repo, {"ROADMAP.md": roadmap})
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-q", "-m", "candidate with shipped")
        self.core = git(self.repo, "rev-parse", "HEAD")
        files = self.bump()
        files["ROADMAP.md"] = roadmap.replace("- v0.76.6:", "- v0.77.0: the release.\n- v0.76.6:")
        self.release(files)
        res = self.run_check(self.receipt())
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        for text in (roadmap.replace("- a stream\n", "- a stream\n- another\n"),
                     roadmap.replace("- v0.76.6: the patch.\n", ""),
                     roadmap.replace("- v0.76.6:", "- v0.76.6: a twin.\n- v0.76.6:")):
            with self.subTest(text=text):
                git(self.repo, "checkout", "-q", "--detach", self.core)
                self.release(dict(self.bump(), **{"ROADMAP.md": text}))
                res = self.run_check(self.receipt())
                self.assertEqual(1, res.returncode, res.stdout + res.stderr)
                self.assertIn("foreign: ROADMAP.md", res.stdout)

    def test_a_foreign_file_in_the_diff_is_refused(self):
        files = self.bump()
        files["crates/meclaw-cells/src/lib.rs"] = "pub fn answer() -> u32 { 43 }\n"
        files["templates/voice/config.json"] = '{\n  "sample_rate": 8000\n}\n'
        self.release(files)
        res = self.run_check(self.receipt())
        self.assertNotEqual(0, res.returncode, res.stdout)
        self.assertIn("foreign: crates/meclaw-cells/src/lib.rs", res.stdout)
        self.assertIn("foreign: templates/voice/config.json", res.stdout)
        self.assertNotIn("foreign: Cargo.toml", res.stdout)
        self.assertNotIn("foreign: CHANGELOG.md", res.stdout)

    def test_a_non_version_line_in_a_version_file_is_refused(self):
        files = self.bump()
        files["Cargo.toml"] = files["Cargo.toml"].replace('"2021"', '"2024"')
        files["crates/meclaw-cells/Cargo.toml"] = files["crates/meclaw-cells/Cargo.toml"].replace(
            'version = "1"', 'version = "1.40"')
        files["templates/voice/template.json"] = BASE["templates/voice/template.json"].replace(
            '"2.7.0"', '"2.7.1"').replace('"description": "voice"', '"description": "voice, faster"')
        self.release(files)
        res = self.run_check(self.receipt())
        self.assertNotEqual(0, res.returncode, res.stdout)
        self.assertIn("foreign: Cargo.toml", res.stdout)
        # A third-party dependency's version is not the workspace version.
        self.assertIn("foreign: crates/meclaw-cells/Cargo.toml", res.stdout)
        self.assertIn("foreign: templates/voice/template.json", res.stdout)

    def test_a_deleted_or_added_file_is_foreign(self):
        files = self.bump()
        files["crates/meclaw-cells/src/new.rs"] = "pub fn x() {}\n"
        self.release(files)
        git(self.repo, "rm", "-q", "ROADMAP.md")
        git(self.repo, "rm", "-q", "templates/voice/config.json")
        git(self.repo, "commit", "-q", "-m", "drop")
        res = self.run_check(self.receipt())
        self.assertNotEqual(0, res.returncode, res.stdout)
        self.assertIn("foreign: crates/meclaw-cells/src/new.rs", res.stdout)
        self.assertIn("foreign: templates/voice/config.json", res.stdout)

    def test_a_receipt_without_core_commit_is_refused(self):
        self.release(self.bump())
        res = self.run_check(self.receipt("# Battery receipt\n\nlab_binary_sha256: %s\n" % ("a" * 64)))
        self.assertNotEqual(0, res.returncode, res.stdout)
        self.assertIn("core_commit", res.stdout + res.stderr)

    def test_an_unknown_core_commit_is_refused(self):
        self.release(self.bump())
        res = self.run_check(self.receipt("core_commit: %s\n" % ("0" * 40)))
        self.assertNotEqual(0, res.returncode, res.stdout)

    def test_the_last_round_of_the_receipt_counts(self):
        # Round 2 appends a new candidate; the release is checked against it.
        first = self.core
        self.release({"crates/meclaw-cells/src/lib.rs": "pub fn answer() -> u32 { 43 }\n"})
        self.core = git(self.repo, "rev-parse", "HEAD")
        body = ("## Round 1\n\ncore_commit: %s\nlab_binary_sha256: %s\ntemplates_tree: %s\n\n"
                "## Round 2\n\n- core_commit: `%s`\n- lab_binary_sha256: `%s`\n"
                "- `templates_tree`: `%s`\n"
                % (first, "a" * 64, self.tree(first), self.core, "c" * 64, self.tree(self.core)))
        self.release(self.bump())
        res = self.run_check(self.receipt(body))
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("lab_binary_sha256: " + "c" * 64, res.stdout)
        self.assertNotIn("a" * 64, res.stdout)

    def test_the_release_defaults_to_head_and_can_be_named(self):
        self.release(self.bump())
        clean = git(self.repo, "rev-parse", "HEAD")
        self.release({"crates/meclaw-cells/src/lib.rs": "pub fn answer() -> u32 { 43 }\n"})
        self.assertNotEqual(0, self.run_check(self.receipt()).returncode)
        res = self.run_check(self.receipt(), "--release", clean)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)

    # G8a (GH #1390): template content without a version bump is invisible to
    # the manifests, so the release must carry the templates tree the battery
    # measured, hash for hash.
    def test_a_release_with_the_measured_templates_tree_is_clean(self):
        self.release(self.bump())
        res = self.run_check(self.receipt())
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("templates_tree: " + self.tree(self.core), res.stdout)
        self.assertEqual(self.tree(self.core), self.tree("HEAD"))

    def test_a_template_change_without_a_bump_is_caught(self):
        measured = self.tree(self.core)
        files = self.bump()
        files["templates/voice/config.json"] = '{\n  "sample_rate": 8000\n}\n'
        self.release(files)
        released = self.tree("HEAD")
        self.assertIn('"2.7.0"', (self.repo / "templates/voice/template.json").read_text())
        res = self.run_check(self.receipt())
        self.assertEqual(1, res.returncode, res.stdout + res.stderr)
        self.assertIn("templates tree differs (%s != %s)" % (released, measured), res.stdout)

    def test_the_templates_tree_is_checked_even_when_the_diff_is_clean(self):
        # The lab measured templates the core commit does not hold (the G8 case
        # live-2 replaced by hand): no foreign file, still not the measured tree.
        self.release({"templates/voice/config.json": '{\n  "sample_rate": 8000\n}\n'})
        measured = self.tree("HEAD")
        git(self.repo, "revert", "--no-edit", "HEAD")
        self.release(self.bump())
        body = "core_commit: %s\ntemplates_tree: %s\n" % (self.core, measured)
        res = self.run_check(self.receipt(body))
        self.assertEqual(1, res.returncode, res.stdout + res.stderr)
        self.assertNotIn("foreign:", res.stdout)
        self.assertIn("templates tree differs (%s != %s)" % (self.tree("HEAD"), measured), res.stdout)

    def test_a_receipt_without_templates_tree_is_refused(self):
        self.release(self.bump())
        body = "core_commit: %s\nlab_binary_sha256: %s\n" % (self.core, "a" * 64)
        res = self.run_check(self.receipt(body))
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        self.assertIn("receipt without templates_tree", res.stdout)
        # The field belongs to the last candidate: round 1's line does not count for round 2.
        body = ("core_commit: %s\ntemplates_tree: %s\n\ncore_commit: %s\n"
                % (self.core, self.tree(self.core), self.core))
        res = self.run_check(self.receipt(body))
        self.assertEqual(2, res.returncode, res.stdout + res.stderr)
        self.assertIn("receipt without templates_tree", res.stdout)


if __name__ == "__main__":
    unittest.main()
