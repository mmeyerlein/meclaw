"""Unit tests for the cheap form station (`scripts/precheck.py`).

One fixture per check, and each one is written so that it is RED before the
check exists: a badly formatted source, a sheet over its ceiling, an ADR
without its anchor line, a script a test names but the export does not carry,
a documentation page changed without its twin -- plus the five findings that
are notes rather than judgements.

Every fixture is a throw-away tree. The real repository is never read: a
station that grades the tree it is tested against says nothing about the rule
it claims to carry.
"""

import io
import json
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
SCRIPTS = HERE.parent
REPO = SCRIPTS.parent
sys.path.insert(0, str(SCRIPTS))

import precheck as pc  # noqa: E402


def tree(case, files):
    """A throw-away root holding `files` ({relative path: text}).

    A value of `None` makes the parent directory and nothing else, which is how
    a fixture says "this path exists as a directory".
    """
    tmp = tempfile.TemporaryDirectory()
    case.addCleanup(tmp.cleanup)
    root = pathlib.Path(tmp.name)
    for rel, text in files.items():
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        if text is not None:
            path.write_text(text, encoding="utf-8")
    return root


def export_fixture(root_files=(), name_patterns=None, allowed=()):
    """A stand-in for `plans/export-fixtures/make_export.py`.

    What precheck borrows from the export and nothing else: what travels,
    which roots are carried, the name patterns and the hits the export
    explains. A fixture that carried the real module would test the export.
    """
    # A STAND-IN pattern, not the export's own: the real list is imported at
    # run time, and spelling it here would put the names it looks for into a
    # file that travels -- which is the thing R5 reads for.
    patterns = name_patterns if name_patterns is not None else [
        (r"a-person|a-domain\.example", "name/domain pattern")]
    return (
        "ROOT_FILES = %r\n"
        "NAME_PATTERNS = %r\n"
        "TREE_DIRS = ['crates', 'tests', 'examples']\n"
        "DOCS_MAP = {'docs/x.en.md': 'docs/x.md', 'docs/clean.en.md': 'docs/clean.md'}\n"
        "_ALLOWED = %r\n"
        "def is_forbidden(path):\n"
        "    return path.split('/')[0] in ('plans', 'ideas', 'archive', 'workshop')\n"
        "def _is_allowed(path, line):\n"
        "    return next((why for p, why in _ALLOWED if p == path), None)\n"
        % (list(root_files), patterns, list(allowed)))


def levels(findings, check=None):
    return [f.level for f in findings if check is None or f.check == check]


def of(findings, check):
    return [f for f in findings if f.check == check]


class Fmt(unittest.TestCase):
    """`rustfmt --check` on the changed sources -- no cargo, no lock.

    Four red strand runs of waves H2/H3/G0 were pure form, and one of them was
    a forgotten `cargo fmt`: 971 s of cargo behind the queue for two `assert!`
    rustfmt wanted on several lines.
    """

    def setUp(self):
        if not shutil.which("rustfmt"):
            self.skipTest("rustfmt is not installed")

    def test_a_badly_formatted_source_is_red(self):
        root = tree(self, {"crates/c/src/lib.rs": "fn  a( ) ->u8{1}\n"})
        found = of(pc.run(["crates/c/src/lib.rs"], repo=root), "fmt")
        self.assertEqual(["RED"], [f.level for f in found], found)
        self.assertIn("crates/c/src/lib.rs", found[0].message)

    def test_a_formatted_source_is_silent(self):
        root = tree(self, {"crates/c/src/lib.rs": "fn a() -> u8 {\n    1\n}\n"})
        self.assertEqual([], of(pc.run(["crates/c/src/lib.rs"], repo=root), "fmt"))

    def test_only_rust_files_are_offered_to_rustfmt(self):
        root = tree(self, {"docs/x.md": "# not rust\n"})
        self.assertEqual([], of(pc.run(["docs/x.md"], repo=root), "fmt"))

    def test_a_kata_fixture_is_not_a_workspace_member(self):
        """`workshop/evals/**` holds Rust nobody formats -- it is measured, not built."""
        path = "workshop/evals/coder-selfopt/katas/k02/tests/hidden.rs"
        root = tree(self, {path: "fn  a( ) ->u8{1}\n"})
        self.assertEqual([], of(pc.run([path], repo=root), "fmt"))

    def test_a_deleted_source_is_not_read(self):
        root = tree(self, {"crates/c/src/lib.rs": "fn a() -> u8 {\n    1\n}\n"})
        self.assertEqual([], of(pc.run(["crates/c/src/gone.rs"], repo=root), "fmt"))

    def test_the_named_path_is_relative_and_carries_no_line_number(self):
        """rustfmt prints `Diff in <absolute path>:<line>:` -- both belong to it.

        The message used to carry the absolute path of the worktree plus a
        trailing `:1:`, so a finding named a path no reader of the repository
        can type.
        """
        root = tree(self, {"crates/c/src/lib.rs": "fn  a( ) ->u8{1}\n"})
        found = of(pc.run(["crates/c/src/lib.rs"], repo=root), "fmt")
        self.assertIn("rewrite crates/c/src/lib.rs --", found[0].message)
        self.assertNotIn(str(root), found[0].message)

    def test_a_source_rustfmt_cannot_read_is_not_a_formatting_finding(self):
        """rustfmt exits 1 when it cannot format at all, and says why on stderr.

        A `.rs` with a syntax error or an unresolvable `mod` is the commonest
        shape of this, and answering it with "run `cargo fmt`" sends the strand
        after a fix that changes nothing.
        """
        root = tree(self, {"crates/c/src/mid.rs": "mod leaf;\nfn b() {}\n"})
        found = of(pc.run(["crates/c/src/mid.rs"], repo=root), "fmt")
        self.assertEqual(["RED"], [f.level for f in found], found)
        self.assertIn("could not read", found[0].message)
        self.assertIn("failed to resolve mod", found[0].message)
        self.assertNotIn("run `cargo fmt`", found[0].message)

    def test_the_edition_comes_from_the_workspace_manifest(self):
        root = tree(self, {"Cargo.toml": '[workspace.package]\nedition = "2024"\n'})
        self.assertEqual("2024", pc.workspace_edition(root))
        self.assertEqual(pc.EDITION_FALLBACK, pc.workspace_edition(tree(self, {})))


class SheetCap(unittest.TestCase):
    """The byte ceiling of the screen's sheet, measured before the lock.

    Wave H2 paid a 1 924 s strand run for `98 274 > 98 000`. The rule is the
    one `display_sheet_strip.py` carries: what travels is the sheet without
    its comments.
    """

    def sheet(self, size):
        body = "/* head */\n" + ("a{color:red}\n" * (size // 13))
        return tree(self, {pc.SHEET: body})

    def test_a_sheet_over_the_ceiling_is_red(self):
        root = self.sheet(pc.SHEET_CEILING - pc.SHEET_AIR + 2_000)
        found = of(pc.run([pc.SHEET], repo=root), "sheet-cap")
        self.assertEqual(["RED"], [f.level for f in found], found)
        self.assertIn(str(pc.SHEET_CEILING - pc.SHEET_AIR), found[0].message)

    def test_a_sheet_under_the_ceiling_is_silent(self):
        root = self.sheet(1_000)
        self.assertEqual([], of(pc.run([pc.SHEET], repo=root), "sheet-cap"))

    def test_the_comments_do_not_count(self):
        """Only the first comment travels -- the rest is measured away."""
        filler = "/* %s */\n" % ("x" * pc.SHEET_CEILING)
        root = tree(self, {pc.SHEET: "/* head */\na{color:red}\n" + filler})
        self.assertEqual([], of(pc.run([pc.SHEET], repo=root), "sheet-cap"))

    def test_a_diff_without_the_sheet_does_not_measure_it(self):
        root = self.sheet(pc.SHEET_CEILING)
        self.assertEqual([], of(pc.run(["docs/x.md"], repo=root), "sheet-cap"))


class AdrAnchor(unittest.TestCase):
    """An ADR without `Pinned-by:` -- the g0-blatt finding, seconds early."""

    def test_an_adr_without_the_line_is_red(self):
        root = tree(self, {"plans/adr/0042-a-decision.md": "# 0042\n\nStatus: Accepted\n"})
        found = of(pc.run(["plans/adr/0042-a-decision.md"], repo=root), "adr-pinned-by")
        self.assertEqual(["RED"], [f.level for f in found], found)
        self.assertIn("0042", found[0].message)

    def test_an_adr_with_the_line_is_silent(self):
        root = tree(self, {"plans/adr/0042-a-decision.md":
                           "# 0042\n\n- **Pinned-by:** `test:a_lock`\n"})
        self.assertEqual(
            [], of(pc.run(["plans/adr/0042-a-decision.md"], repo=root), "adr-pinned-by"))

    def test_the_adr_readme_is_not_an_adr(self):
        root = tree(self, {"plans/adr/README.md": "# how ADRs work\n"})
        self.assertEqual([], of(pc.run(["plans/adr/README.md"], repo=root), "adr-pinned-by"))


class RootFiles(unittest.TestCase):
    """R2c, before the export instead of after it (review finding g0-blatt F1).

    `scripts/display_sheet_strip.py` was named by a drift lock and missing from
    `ROOT_FILES`; locally green, red in the published tree an hour later.
    """

    def repo(self, root_files, lock=None):
        return tree(self, {
            "scripts/display_sheet_strip.py": "# a rule\n",
            "crates/meclaw-cells/Cargo.toml": '[package]\nname = "meclaw-cells"\n',
            "crates/meclaw-cells/tests/a_lock.rs": lock or
                'const STRIP: &str = "scripts/display_sheet_strip.py";\n'
                'fn main() { let _ = repo(STRIP); }\n',
            "plans/export-fixtures/make_export.py": export_fixture(root_files),
        })

    def test_a_script_a_test_reads_needs_an_entry(self):
        root = self.repo(["scripts/gate.sh"])
        found = of(pc.run(["scripts/display_sheet_strip.py"], repo=root), "root-files")
        self.assertEqual(["RED"], [f.level for f in found], found)
        self.assertIn("display_sheet_strip.py", found[0].message)
        self.assertIn("a_lock", found[0].message)

    def test_an_entry_answers_it(self):
        root = self.repo(["scripts/display_sheet_strip.py"])
        self.assertEqual(
            [], of(pc.run(["scripts/display_sheet_strip.py"], repo=root), "root-files"))

    def test_a_mention_in_a_failure_message_is_not_a_read(self):
        """Four tests name a script inside an assert message; none executes it."""
        root = self.repo([], lock='assert!(ok, "run `scripts/display_sheet_strip.py`");\n')
        self.assertEqual(
            [], of(pc.run(["scripts/display_sheet_strip.py"], repo=root), "root-files"))

    def test_a_mention_in_a_comment_is_not_a_read(self):
        root = self.repo([], lock='//! See "scripts/display_sheet_strip.py" for the rule.\n')
        self.assertEqual(
            [], of(pc.run(["scripts/display_sheet_strip.py"], repo=root), "root-files"))

    def test_a_guarded_read_skips_in_the_published_tree(self):
        """The answer the export accepts elsewhere: `.exists()` on the path."""
        root = self.repo([], lock=(
            'let script = root.join("scripts/display_sheet_strip.py");\n'
            'if !script.exists() { return; }\n'))
        self.assertEqual(
            [], of(pc.run(["scripts/display_sheet_strip.py"], repo=root), "root-files"))

    def test_a_guard_on_another_path_does_not_count(self):
        """The g0-blatt shape exactly: a guard, but not on the file it reads."""
        root = self.repo([], lock=(
            'const STRIP: &str = "scripts/display_sheet_strip.py";\n'
            'if !repo("templates/display/template.json").is_file() { return; }\n'
            'let _ = read(STRIP);\n'))
        self.assertEqual(
            ["RED"], levels(pc.run(["scripts/display_sheet_strip.py"], repo=root),
                            "root-files"))

    def test_a_script_no_test_names_is_not_a_finding(self):
        root = tree(self, {
            "scripts/private_tool.py": "# nobody reads me\n",
            "crates/meclaw-cells/Cargo.toml": '[package]\nname = "meclaw-cells"\n',
            "crates/meclaw-cells/tests/a_lock.rs": "fn main() {}\n",
            "plans/export-fixtures/make_export.py": export_fixture([]),
        })
        self.assertEqual([], of(pc.run(["scripts/private_tool.py"], repo=root), "root-files"))

    def test_a_new_test_that_reads_an_old_script_is_the_same_finding(self):
        """The other direction of F1, and the shape wave P8 will walk into.

        The script is old and exported by nobody; the TEST is what the diff
        adds. Grading only the `scripts/` paths of the diff makes that half
        invisible until the export runs an hour later.
        """
        root = self.repo([], lock='const STRIP: &str = "scripts/display_sheet_strip.py";\n'
                                  'fn main() { let _ = repo(STRIP); }\n')
        found = of(pc.run(["crates/meclaw-cells/tests/a_lock.rs"], repo=root), "root-files")
        self.assertEqual(["RED"], [f.level for f in found], found)
        self.assertIn("display_sheet_strip.py", found[0].message)

    def test_a_new_test_that_reads_an_exported_script_is_silent(self):
        root = self.repo(["scripts/display_sheet_strip.py"])
        self.assertEqual([], of(pc.run(["crates/meclaw-cells/tests/a_lock.rs"], repo=root),
                                "root-files"))

    def test_a_python_test_names_scripts_the_same_way(self):
        root = tree(self, {
            "scripts/display_sheet_strip.py": "# a rule\n",
            "scripts/tests/test_strip.py": 'STRIP = "scripts/display_sheet_strip.py"\n',
            "plans/export-fixtures/make_export.py": export_fixture([]),
        })
        self.assertEqual(["RED"], levels(pc.run(["scripts/tests/test_strip.py"], repo=root),
                                         "root-files"))

    def test_without_the_export_the_check_stays_quiet(self):
        """The published tree has no `plans/`; a check with no input is no verdict."""
        root = tree(self, {"scripts/x.py": "# x\n"})
        self.assertEqual([], of(pc.run(["scripts/x.py"], repo=root), "root-files"))


class DocsTwin(unittest.TestCase):
    """Both language faces move together, or the export is a red gate."""

    def repo(self):
        return tree(self, {"docs/config.md": "# de\n", "docs/config.en.md": "# en\n",
                           "docs/development-rules.md": "# one face only\n"})

    def test_one_face_alone_is_red(self):
        found = of(pc.run(["docs/config.md"], repo=self.repo()), "docs-twin")
        self.assertEqual(["RED"], [f.level for f in found], found)
        self.assertIn("docs/config.en.md", found[0].message)

    def test_the_other_face_alone_is_red_too(self):
        found = of(pc.run(["docs/config.en.md"], repo=self.repo()), "docs-twin")
        self.assertEqual(["RED"], [f.level for f in found], found)
        self.assertIn("docs/config.md", found[0].message)

    def test_both_faces_are_silent(self):
        self.assertEqual([], of(pc.run(["docs/config.md", "docs/config.en.md"],
                                       repo=self.repo()), "docs-twin"))

    def test_a_page_with_no_twin_is_silent(self):
        self.assertEqual([], of(pc.run(["docs/development-rules.md"],
                                       repo=self.repo()), "docs-twin"))


class ChangelogMark(unittest.TestCase):
    """A note, not a judgement: a release block that names no issue."""

    def repo(self, block):
        return tree(self, {
            "CHANGELOG.md": "# Changelog\n\n## [Unreleased]\n\n" + block,
            "crates/c/src/lib.rs": "fn a() {}\n"})

    def test_a_block_without_an_issue_mark_is_a_note(self):
        root = self.repo("## [0.40.0]\n\nThe screen got wider.\n")
        found = of(pc.run(["crates/c/src/lib.rs", "CHANGELOG.md"], repo=root),
                   "changelog-mark")
        self.assertEqual(["NOTE"], [f.level for f in found], found)

    def test_an_issue_mark_answers_it(self):
        root = self.repo("## [0.40.0]\n\nThe screen got wider (GH #777).\n")
        self.assertEqual([], of(pc.run(["crates/c/src/lib.rs", "CHANGELOG.md"], repo=root),
                                "changelog-mark"))

    def test_a_diff_without_the_changelog_is_not_graded_here(self):
        root = self.repo("## [0.40.0]\n\nThe screen got wider.\n")
        self.assertEqual([], of(pc.run(["crates/c/src/lib.rs"], repo=root),
                                "changelog-mark"))


class SinceSentence(unittest.TestCase):
    """The voice-grace finding: a since-sentence pulled forward one version."""

    def repo(self, readme):
        return tree(self, {"templates/voice/template.json": '{"version": "2.0.3"}\n',
                           "templates/voice/README.md": readme,
                           "templates/memory-hive/template.json": '{"version": "2.2.0"}\n'})

    def test_a_sentence_ahead_of_the_template_is_a_note(self):
        root = self.repo("The grace period is a lane since `2.0.4`.\n")
        found = of(pc.run(["templates/voice/README.md"], repo=root), "since-sentence")
        self.assertEqual(["NOTE"], [f.level for f in found], found)
        self.assertIn("2.0.4", found[0].message)

    def test_the_template_version_itself_is_fine(self):
        root = self.repo("The grace period is a lane since `2.0.3`.\n")
        self.assertEqual([], of(pc.run(["templates/voice/README.md"], repo=root),
                                "since-sentence"))

    def test_an_older_version_is_history_and_stays(self):
        root = self.repo("There was a third, and it is gone since `1.0.1`.\n")
        self.assertEqual([], of(pc.run(["templates/voice/README.md"], repo=root),
                                "since-sentence"))

    def test_a_named_version_counts_too(self):
        root = self.repo("A turn of its own since `voice@2.1.0`.\n")
        self.assertEqual(["NOTE"], levels(pc.run(["templates/voice/README.md"], repo=root),
                                          "since-sentence"))

    def test_a_sentence_about_another_template_is_about_that_one(self):
        """``memory-hive` has had since 2.2.0` is its version, not this README's."""
        root = self.repo("What `memory-hive` has had since 2.2.0 arrives here.\n")
        self.assertEqual([], of(pc.run(["templates/voice/README.md"], repo=root),
                                "since-sentence"))

    def test_an_issue_reference_is_not_a_version(self):
        root = self.repo("It lives in the front door since GH #556.\n")
        self.assertEqual([], of(pc.run(["templates/voice/README.md"], repo=root),
                                "since-sentence"))

    def test_a_capitalised_sentence_counts_too(self):
        """In the tree the sentence is nearly always sentence-initial.

        `templates/assistant/README.md:176`, `:225`, `:228`,
        `templates/builder-librarian/README.md:94`,
        `templates/archive-bridge/README.md:72`, `templates/clock/README.md:71`
        all write `Since <version>`; a case-sensitive pattern sees none of them.
        """
        root = self.repo("Since `2.0.9` the cell pushes silence.\n")
        self.assertEqual(["NOTE"], levels(pc.run(["templates/voice/README.md"], repo=root),
                                          "since-sentence"))


class SinceInDocs(unittest.TestCase):
    """The voice-grace finding itself: a since-sentence that moved with the bump.

    `docs/cell-types.md:1176` said ``since `voice@2.0.4` `` for a behaviour that
    shipped in 2.0.3, and the same diff bumped the template to 2.0.4. The README
    check cannot see that one: it grades `templates/<name>/README.md` and asks
    whether the version is NEWER than the template's own -- here it was equal,
    and the sentence stood in `docs/`.
    """

    def repo(self, line):
        return tree(self, {"docs/cell-types.md": line,
                           "templates/voice/template.json": '{"version": "2.0.4"}\n'})

    def test_the_version_this_diff_bumps_to_is_a_note(self):
        root = self.repo("The cell pushes silence since `voice@2.0.4`.\n")
        found = of(pc.run(["docs/cell-types.md", "templates/voice/template.json"],
                          repo=root), "since-doc")
        self.assertEqual(["NOTE"], [f.level for f in found], found)
        self.assertIn("2.0.4", found[0].message)

    def test_without_the_bump_in_the_diff_the_sentence_is_history(self):
        root = self.repo("The cell pushes silence since `voice@2.0.4`.\n")
        self.assertEqual([], of(pc.run(["docs/cell-types.md"], repo=root), "since-doc"))

    def test_a_version_the_template_has_not_reached_is_a_note_on_its_own(self):
        root = self.repo("The cell pushes silence since `voice@2.1.0`.\n")
        self.assertEqual(["NOTE"], levels(pc.run(["docs/cell-types.md"], repo=root),
                                          "since-doc"))

    def test_an_older_version_stays(self):
        root = self.repo("The cell pushes silence since `voice@2.0.3`.\n")
        self.assertEqual([], of(pc.run(["docs/cell-types.md"], repo=root), "since-doc"))

    def test_a_bare_version_has_nothing_to_measure_against(self):
        root = self.repo("The cell pushes silence since `2.0.9`.\n")
        self.assertEqual([], of(pc.run(["docs/cell-types.md"], repo=root), "since-doc"))

    def test_an_unknown_template_is_not_graded(self):
        root = self.repo("Gone since `no-such-cell@9.9.9`.\n")
        self.assertEqual([], of(pc.run(["docs/cell-types.md"], repo=root), "since-doc"))

    def test_a_template_readme_is_the_other_checks_business(self):
        root = tree(self, {"templates/voice/README.md": "Since `voice@2.0.4` it is so.\n",
                           "templates/voice/template.json": '{"version": "2.0.4"}\n'})
        self.assertEqual([], of(pc.run(["templates/voice/README.md",
                                        "templates/voice/template.json"], repo=root),
                                "since-doc"))


class RunArtefacts(unittest.TestCase):
    """`last_run.json` and stray symlinks: four incidents, no gate run lost."""

    def test_a_run_artefact_in_the_diff_is_a_note(self):
        path = "workshop/evals/scenarios/last_run.json"
        root = tree(self, {path: "{}\n"})
        self.assertEqual(["NOTE"], levels(pc.run([path], repo=root), "run-artefact"))

    def test_a_symlink_in_the_diff_is_a_note(self):
        root = tree(self, {"scripts/real.py": "# real\n"})
        (root / ".env").symlink_to(root / "scripts" / "real.py")
        self.assertEqual(["NOTE"], levels(pc.run([".env"], repo=root), "run-artefact"))

    def test_an_ordinary_file_is_silent(self):
        root = tree(self, {"scripts/real.py": "# real\n"})
        self.assertEqual([], of(pc.run(["scripts/real.py"], repo=root), "run-artefact"))


class PlanCap(unittest.TestCase):
    """The 600-line cap of development-rules § 1, as a note on the diff."""

    PART = "plans/welle-x-2026-01-01/plan-parts/X1-boden.md"

    def test_a_plan_part_over_the_cap_is_a_note(self):
        root = tree(self, {self.PART: "line\n" * 601})
        found = of(pc.run([self.PART], repo=root), "plan-cap")
        self.assertEqual(["NOTE"], [f.level for f in found])
        self.assertIn("601 lines", found[0].message)

    def test_a_plan_part_at_the_cap_is_silent(self):
        root = tree(self, {self.PART: "line\n" * 600})
        self.assertEqual([], of(pc.run([self.PART], repo=root), "plan-cap"))

    def test_the_record_copy_is_exempt(self):
        """`plan-parts/voll/` is where a wave keeps the un-shortened part.

        Wave G wrote the cap into its own tree that way: four parts under the
        cap beside four full copies kept as the record (OR-G0.1). Grading the
        record would make the exemption unusable.
        """
        path = "plans/welle-x-2026-01-01/plan-parts/voll/X1-boden.md"
        root = tree(self, {path: "line\n" * 4610})
        self.assertEqual([], of(pc.run([path], repo=root), "plan-cap"))

    def test_another_long_plan_file_is_silent(self):
        """The cap is about a strand's part, not about every file in plans/."""
        path = "plans/welle-x-2026-01-01/plan.md"
        root = tree(self, {path: "line\n" * 4610})
        self.assertEqual([], of(pc.run([path], repo=root), "plan-cap"))

    def test_a_part_that_is_gone_says_nothing(self):
        """A deleted path is in the diff and not on disk."""
        root = tree(self, {"plans/welle-x-2026-01-01/plan-parts": None})
        self.assertEqual([], of(pc.run([self.PART], repo=root), "plan-cap"))


class NamePatterns(unittest.TestCase):
    """R5, on the files that travel -- the rest of the tree is allowed to."""

    def repo(self, allowed=()):
        return tree(self, {
            "docs/x.en.md": "Written by a-person, on a-domain.example.\n",
            "plans/welle/report.md": "a-person said so.\n",
            "docs/clean.en.md": "Nothing to see.\n",
            "docs/link.en.md": "Filed at github.com/an-owner/meclaw/issues/1.\n",
            "plans/export-fixtures/make_export.py": export_fixture([], allowed=allowed),
        })

    def test_a_travelling_file_with_a_name_is_a_note(self):
        found = of(pc.run(["docs/x.en.md"], repo=self.repo()), "name-pattern")
        self.assertEqual(["NOTE"], [f.level for f in found], found)
        self.assertIn("docs/x.en.md", found[0].message)

    def test_a_file_that_never_travels_is_not_graded(self):
        self.assertEqual([], of(pc.run(["plans/welle/report.md"], repo=self.repo()),
                                "name-pattern"))

    def test_a_clean_file_is_silent(self):
        self.assertEqual([], of(pc.run(["docs/clean.en.md"], repo=self.repo()),
                                "name-pattern"))

    def test_the_projects_own_repository_url_is_not_a_leak(self):
        """The owner handle matches the pattern, and every issue link carries it."""
        self.assertEqual([], of(pc.run(["docs/link.en.md"], repo=self.repo()),
                                "name-pattern"))

    def test_every_burdened_file_is_named(self):
        """One finding per file, and the run does not stop at the first.

        A `return` in the generator ended the whole check after the first hit,
        so two of three burdened files stayed invisible until the next run
        uncovered them -- and the next run is a fix round later.
        """
        root = tree(self, {
            "examples/a.md": "By a-person.\n",
            "examples/b.md": "At a-domain.example.\n",
            "examples/c.md": "Also by a-person.\n",
            "plans/export-fixtures/make_export.py": export_fixture([]),
        })
        found = of(pc.run(["examples/a.md", "examples/b.md", "examples/c.md"], repo=root),
                   "name-pattern")
        self.assertEqual(3, len(found), found)
        for name in ("examples/a.md", "examples/b.md", "examples/c.md"):
            self.assertTrue(any(name in f.message for f in found), (name, found))

    def test_a_file_is_named_once_however_often_it_offends(self):
        root = tree(self, {
            "examples/a.md": "By a-person.\nStill a-person.\nAt a-domain.example.\n",
            "plans/export-fixtures/make_export.py": export_fixture([]),
        })
        self.assertEqual(1, len(of(pc.run(["examples/a.md"], repo=root), "name-pattern")))

    def test_a_hit_the_export_explains_is_silent(self):
        """The allowance is keyed on the PUBLISHED name (`DOCS_MAP`)."""
        root = self.repo(allowed=[("docs/x.md", "the project's own domain")])
        self.assertEqual([], of(pc.run(["docs/x.en.md"], repo=root), "name-pattern"))


class SheetNumbersMatchTheLock(unittest.TestCase):
    """The ceiling and the air are the Rust lock's numbers, read from it.

    Both were copies, and nothing held the two places together: a fourth raise
    of the ceiling in the lock would have let this station drift silently and
    keep waving through what the lock then stopped.
    """

    LOCK = (REPO / "crates" / "meclaw-cells" / "tests"
            / "gh669_every_class_the_screen_writes_has_a_rule.rs")

    def test_the_two_numbers_are_the_ones_the_lock_asserts(self):
        if not self.LOCK.is_file():
            self.skipTest("the lock is not in this tree")
        text = self.LOCK.read_text(encoding="utf-8")
        ceiling = re.search(r"raw < (\d[\d_]*)", text)
        air = re.search(r"(\d[\d_]*) - raw >= (\d[\d_]*)", text)
        self.assertIsNotNone(ceiling, "the lock no longer asserts a raw ceiling")
        self.assertIsNotNone(air, "the lock no longer reserves air under it")
        self.assertEqual(int(ceiling.group(1).replace("_", "")), pc.SHEET_CEILING)
        self.assertEqual(int(air.group(1).replace("_", "")), pc.SHEET_CEILING)
        self.assertEqual(int(air.group(2).replace("_", "")), pc.SHEET_AIR)


class Cli(unittest.TestCase):
    """The interface the gate station uses, and the exit code it grades on."""

    def call(self, root, *args, stdin=""):
        return subprocess.run(
            [sys.executable, str(SCRIPTS / "precheck.py"), "--repo", str(root)] + list(args),
            input=stdin, capture_output=True, text=True)

    def test_files_from_stdin(self):
        root = tree(self, {"plans/adr/0042-x.md": "# 0042\n"})
        res = self.call(root, "--files-from", "-", stdin="plans/adr/0042-x.md\n")
        self.assertEqual(1, res.returncode, res.stdout + res.stderr)
        self.assertIn("RED adr-pinned-by", res.stdout)

    def test_files_on_the_argv(self):
        root = tree(self, {"plans/adr/0042-x.md": "# 0042\n"})
        res = self.call(root, "--files", "plans/adr/0042-x.md")
        self.assertEqual(1, res.returncode, res.stdout + res.stderr)

    def test_notes_alone_leave_the_station_green(self):
        path = "workshop/evals/scenarios/last_run.json"
        root = tree(self, {path: "{}\n"})
        res = self.call(root, "--files", path)
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertIn("NOTE run-artefact", res.stdout)

    def test_an_empty_diff_is_green_and_silent(self):
        res = self.call(tree(self, {}), "--files-from", "-", stdin="")
        self.assertEqual(0, res.returncode, res.stdout + res.stderr)
        self.assertEqual("", res.stdout.strip())

    def test_every_line_is_a_level_a_check_and_a_message(self):
        root = tree(self, {"plans/adr/0042-x.md": "# 0042\n",
                           "workshop/evals/scenarios/last_run.json": "{}\n"})
        res = self.call(root, "--files", "plans/adr/0042-x.md",
                        "workshop/evals/scenarios/last_run.json")
        for line in res.stdout.strip().splitlines():
            level, check, rest = line.split(" ", 2)
            self.assertIn(level, ("RED", "NOTE"), line)
            self.assertTrue(check.endswith(":"), line)
            self.assertTrue(rest, line)


class TheRealTreeIsGreen(unittest.TestCase):
    """The station must be green on the tree it is committed into.

    Not a fixture: this is the one place the real repository is read, and it
    answers the question a new station always raises -- does it start red? A
    station that does is a station the next strand learns to read past.
    """

    # The one finding the tree carries and keeps: ADR-0002 was written before
    # development-rules § 2b asked for the anchor line, and the anchor gate
    # tolerates it. The precheck grades the ADRs a DIFF touches, so this never
    # reaches a strand -- it only shows up when the whole tree is offered.
    ALLOWED = {("adr-pinned-by", "plans/adr/0002-")}

    # `fmt` is left out here, and only here. Over the whole tree it asks
    # rustfmt about every source there is, which would make `gate-selftest`
    # red over a file outside the diff -- the verdict of the `fmt` station,
    # reported by the wrong one. On a diff it grades what the diff carries,
    # which is the whole point.
    def test_the_committed_tree_carries_no_red_finding(self):
        paths = subprocess.run(["git", "-C", str(REPO), "ls-files"],
                               capture_output=True, text=True,
                               check=True).stdout.splitlines()
        red = [f for f in pc.run(paths, repo=REPO)
               if f.level == "RED" and f.check != "fmt"
               and not any(f.check == c and w in f.message for c, w in self.ALLOWED)]
        self.assertEqual([], red, "\n".join("%s: %s" % (f.check, f.message) for f in red))


if __name__ == "__main__":
    unittest.main()
