"""Tests of the merge point's numbers and the pin cascade (`scripts/pipeline/versions.py`, W9).

The tree is the fixture of `test_pipeline_merge`: templates `a` (1.0.0),
`b` (2.0.0) and `c` (0.3.0, pins `a@1.0.0`), a scaffold and an example that
pin `a`, docs with a full version, a shorthand and a frozen plan archive.
`assign` runs on the main tree's working copy against `master`, as the merge
runs it on the candidate.
"""

import contextlib
import io
import json
import os
import pathlib
import re
import sys
import unittest
from unittest import mock

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS))

from pipeline import versions  # noqa: E402

from . import test_pipeline_merge as fx  # noqa: E402
from scripts.tests import store_isolation  # noqa: E402

# gate-selftest loads every module into one process: no fixture event of this
# module may reach the metrics target of the caller's environment.
_METRICS_URL = mock.patch.dict(os.environ, {"MECLAW_PIPELINE_METRICS_URL": ""})


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()
    _METRICS_URL.start()


def tearDownModule():  # noqa: N802 -- unittest's name
    _METRICS_URL.stop()
    store_isolation.restore()


class VersionsCase(unittest.TestCase):

    def setUp(self):
        self.f = fx.Fixture()
        self.addCleanup(self.f.close)
        self.root = self.f.root

    def touch(self, *rels):
        for rel in rels:
            fx.write(self.root, rel, "changed\n")
        return list(rels)

    def assign(self, changed, kinds=None):
        return versions.assign(self.root, "master", changed, kinds or {})


class BumpTest(VersionsCase):

    def test_versions_fix_bumps_third_digit_only_touched_templates(self):
        res = self.assign(self.touch("templates/a/README.md"))
        self.assertEqual(res["versions"]["a"], {"from": "1.0.0", "to": "1.0.1", "kind": "fix"})
        self.assertEqual(fx.version_of(self.root, "a"), "1.0.1")
        # b is untouched and stays; c only follows its pin (see the cascade case).
        self.assertNotIn("b", res["versions"])
        self.assertEqual(fx.version_of(self.root, "b"), "2.0.0")
        self.assertTrue(res["versions"]["c"].get("cascade"))
        self.assertNotIn("cascade", res["versions"]["a"])
        self.assertIsNone(res["workspace"])
        self.assertEqual(versions.version_line(res),
                         "Versions: a 1.0.0 -> 1.0.1 (fix), c 0.3.0 -> 0.3.1 (fix)")

    def test_versions_feature_bumps_second(self):
        res = self.assign(self.touch("templates/a/README.md", "templates/b/new.md"),
                          {"a": "feature", "b": "break"})
        self.assertEqual(res["versions"]["a"]["to"], "1.1.0")
        self.assertEqual(res["versions"]["b"]["to"], "3.0.0")
        self.assertEqual(fx.version_of(self.root, "a"), "1.1.0")
        # The pinner follows with a fix, whatever the kind of the pinned.
        self.assertEqual(res["versions"]["c"], {"from": "0.3.0", "to": "0.3.1", "kind": "fix",
                                                "cascade": True})
        self.assertEqual(versions.bump("0.3.4", "feature"), "0.4.0")
        self.assertEqual(versions.bump("0.3.4", "break"), "0.4.0")
        self.assertEqual(versions.bump("2.3.4", "break"), "3.0.0")
        self.assertEqual(versions.bump("2.3.4", "fix"), "2.3.5")
        with self.assertRaises(versions.VersionError):
            versions.bump("2.3.4", "minor")
        with self.assertRaises(versions.VersionError):
            fx.write(self.f.tmp, "s.json", json.dumps({"kind": {"a": "patch"}}))
            versions.read_strand(os.path.join(self.f.tmp, "s.json"))

    def test_broken_strand_file_is_an_error_not_a_default(self):
        path = os.path.join(self.f.tmp, "s.json")
        self.assertEqual(versions.read_strand(os.path.join(self.f.tmp, "none.json"))["kind"], {})
        for text in ('{"kind": {"a": "feature"', '["kind"]', '{"kind": ["a"]}'):
            fx.write(self.f.tmp, "s.json", text)
            with self.assertRaises(versions.VersionError, msg=text):
                versions.read_strand(path)

    def test_close_issue_is_a_boolean_defaulting_to_true(self):
        # V2-A1: `"close_issue": "false"` (a string) would close the carrier
        # issue silently -- a broken field is an error, never the default.
        path = os.path.join(self.f.tmp, "s.json")
        self.assertTrue(versions.close_issue_of(versions.read_strand(
            os.path.join(self.f.tmp, "none.json"))))
        fx.write(self.f.tmp, "s.json", '{"kind": {}, "close_issue": false}')
        self.assertFalse(versions.close_issue_of(versions.read_strand(path)))
        for text in ('{"close_issue": "false"}', '{"close_issue": 0}', '{"close_issue": null}'):
            fx.write(self.f.tmp, "s.json", text)
            with self.assertRaisesRegex(versions.VersionError,
                                        r"close_issue is .* \(expected true or false\)",
                                        msg=text):
                versions.read_strand(path)

    def test_merge_refuses_broken_strand_file(self):
        f = self.f
        f.strand("S1", 101, {"templates/a/foo.txt": "foo\n"})
        fx.write(f.wdir, "straenge/S1.json", '{"kind": {"a": "feature"')
        rc, _, err = f.merge(["S1"])
        self.assertEqual(rc, fx.merge.REFUSE_EXIT)
        self.assertIn("strand_file_invalid", err)

    def test_versions_never_renumbers_published(self):
        f = self.f
        # The strand numbered `a` itself from its old base ...
        s = f.strand("S1", 101, {"templates/a/foo.txt": "foo\n",
                                 "templates/a/template.json": fx.template_json("a", "1.0.1")})
        # ... and master published 1.0.1 meanwhile.
        f.master_commit({"templates/a/template.json": fx.template_json("a", "1.0.1")},
                        "a 1.0.1 published")
        fx.git(self.root, "merge", "--no-ff", "--no-commit", s["tip"])
        changed = fx.git(self.root, "diff", "--name-only", "--cached", "master").split()
        res = self.assign(changed)
        self.assertEqual(res["versions"]["a"], {"from": "1.0.1", "to": "1.0.2", "kind": "fix"})
        self.assertEqual(fx.version_of(self.root, "a"), "1.0.2")

    def test_versions_workspace_only_when_declared(self):
        fx.write(self.root, "Cargo.toml", '[workspace]\nmembers = ["x"]\n\n'
                 '[workspace.package]\nversion = "0.5.2"\nedition = "2021"\n')
        fx.write(self.root, "Cargo.lock", 'version = 4\n\n[[package]]\nname = "x"\n'
                 'version = "0.5.2"\n\n[[package]]\nname = "dep"\nversion = "0.5.2"\n'
                 'source = "registry+https://example.invalid"\n')
        fx.git(self.root, "add", "-A")
        fx.git(self.root, "commit", "-q", "-m", "workspace")
        res = self.assign([])
        self.assertIsNone(res["workspace"])
        res = self.assign([], {"workspace": "feature"})
        self.assertEqual(res["workspace"], {"from": "0.5.2", "to": "0.6.0", "kind": "feature"})
        cargo = fx.read(self.root, "Cargo.toml")
        self.assertIn('version = "0.6.0"', cargo)
        lock = fx.read(self.root, "Cargo.lock")
        self.assertIn('name = "x"\nversion = "0.6.0"', lock)
        self.assertIn('name = "dep"\nversion = "0.5.2"', lock)


    def test_versions_workspace_minor_raises_the_inter_crate_pins(self):
        # Fund V-5 (welle-validierung, 14.10.): the first minor bump through
        # the merge point set [workspace.package] to 0.64.0 but left the
        # path dependencies at `version = "0.63.1"`; ^0.63.1 does not admit
        # 0.64.0, and every cargo station of the merge candidate was red in
        # 22 s. A pin the new version no longer satisfies moves with it; a
        # registry dependency with the same number stays.
        fx.write(self.root, "Cargo.toml", '[workspace]\nmembers = ["crates/*"]\n\n'
                 '[workspace.package]\nversion = "0.5.2"\nedition = "2021"\n')
        fx.write(self.root, "crates/meclaw-x/Cargo.toml", '[package]\nname = "meclaw-x"\n')
        fx.write(self.root, "crates/meclaw-y/Cargo.toml",
                 '[package]\nname = "meclaw-y"\n\n[dependencies]\n'
                 'meclaw-x = { path = "../meclaw-x", version = "0.5.1" }\n'
                 'other = { version = "0.5.1" }\n')
        fx.git(self.root, "add", "-A")
        fx.git(self.root, "commit", "-q", "-m", "workspace")
        self.assign([], {"workspace": "fix"})
        y = fx.read(self.root, "crates/meclaw-y/Cargo.toml")
        self.assertIn('meclaw-x = { path = "../meclaw-x", version = "0.5.1" }', y,
                      "a patch bump keeps a pin ^0.5.1 still admits")
        self.assign([], {"workspace": "feature"})
        y = fx.read(self.root, "crates/meclaw-y/Cargo.toml")
        self.assertIn('meclaw-x = { path = "../meclaw-x", version = "0.6.0" }', y)
        self.assertIn('other = { version = "0.5.1" }', y)

    def test_versions_workspace_minor_names_the_raised_inter_crate_pins(self):
        # Cluster 3c (welle-validierung-2, GH #1199): the minor bump raised the
        # inter-crate pins (V-5), yet the merge line said `Pins: none` --
        # `pins_line` reads `result["pins"]`, and the workspace pins never
        # reached it. Each raised pin is one entry, path + crate, old -> new.
        fx.write(self.root, "Cargo.toml", '[workspace]\nmembers = ["crates/*"]\n\n'
                 '[workspace.package]\nversion = "0.5.2"\nedition = "2021"\n')
        fx.write(self.root, "crates/meclaw-x/Cargo.toml", '[package]\nname = "meclaw-x"\n')
        fx.write(self.root, "crates/meclaw-y/Cargo.toml",
                 '[package]\nname = "meclaw-y"\n\n[dependencies]\n'
                 'meclaw-x = { path = "../meclaw-x", version = "0.5.1" }\n'
                 'other = { version = "0.5.1" }\n')
        fx.write(self.root, "crates/meclaw-z/Cargo.toml",
                 '[package]\nname = "meclaw-z"\n\n[dependencies]\n'
                 'meclaw-x = { path = "../meclaw-x", version = "0.5.1" }\n'
                 'meclaw-y = { path = "../meclaw-y", version = "0.5.1" }\n')
        fx.git(self.root, "add", "-A")
        fx.git(self.root, "commit", "-q", "-m", "workspace")
        res = self.assign([], {"workspace": "fix"})
        self.assertEqual("Pins: none", versions.pins_line(res), "a patch bump raises no pin")
        res = self.assign([], {"workspace": "feature"})
        self.assertEqual(
            [("crates/meclaw-y/Cargo.toml", "meclaw-x", "0.5.1", "0.6.0"),
             ("crates/meclaw-z/Cargo.toml", "meclaw-x", "0.5.1", "0.6.0"),
             ("crates/meclaw-z/Cargo.toml", "meclaw-y", "0.5.1", "0.6.0")],
            [(p["path"], p["template"], p["from"], p["to"]) for p in res["pins"]])
        self.assertEqual("Pins: 3 raised in 2 files (crates/meclaw-y/Cargo.toml, "
                         "crates/meclaw-z/Cargo.toml)", versions.pins_line(res))
class CascadeTest(VersionsCase):

    def test_cascade_raises_scaffold_and_example_pins(self):
        res = self.assign(self.touch("templates/a/README.md"))
        self.assertEqual(json.loads(fx.read(self.root, fx.SCAFFOLD))["template"], "a@1.0.1")
        example = json.loads(fx.read(self.root, fx.EXAMPLE))
        self.assertEqual([c["template"] for c in example["cells"]], ["a@1.0.1", "b@2.0.0"])
        # The pinning template follows, transitively, as a cascade fix.
        c = json.loads(fx.read(self.root, "templates/c/template.json"))
        self.assertEqual(c["uses"], ["a@1.0.1"])
        self.assertEqual(c["version"], "0.3.1")
        self.assertEqual(res["versions"]["c"], {"from": "0.3.0", "to": "0.3.1", "kind": "fix",
                                                "cascade": True})
        # The full version in docs moves; the shorthand, the frozen plan archive
        # and the workshop measurement record stay.
        docs = fx.read(self.root, "docs/x.md")
        self.assertIn("runs a@1.0.1 today", docs)
        self.assertIn("Since a@1 the ledger", docs)
        self.assertEqual(fx.read(self.root, "docs/superpowers/plans/old.md"),
                         "Measured on a@1.0.0.\n")
        self.assertEqual(fx.read(self.root, "workshop/evals/run.md"), "Measured at a@1.0.0.\n")
        paths = sorted({p["path"] for p in res["pins"]})
        self.assertEqual(paths, sorted([fx.EXAMPLE, "docs/x.md", "templates/c/template.json",
                                        fx.SCAFFOLD]))
        line = versions.changelog_line(res, ["101"])
        self.assertEqual(line, "- **Template versions (#101):** third digit, `a@1.0.1`; "
                               "third digit for the pins that follow, `c@0.3.1`.")
        # A cascaded tree is pin-lock clean.
        self.assertEqual(versions.cascade_proposal(self.root), [])

    def test_cascade_moves_the_catalogue_row(self):
        # W9 probe: the gate's `catalogue` station (scripts/check_catalogue.py)
        # was red on every merge -- the table of templates/README.md names the
        # version a template ships at, and the merge point moved it nowhere.
        fx.write(self.root, "templates/README.md",
                 "| Template | Version | What |\n|---|---|---|\n"
                 "| [`a`](a/) | 1.0.0 | First. |\n| [`b`](b/) | 2.0.0 | Second. |\n"
                 "| [`c`](c/) | 0.3.0 | Pins a. |\n")
        fx.git(self.root, "add", "-A")
        fx.git(self.root, "commit", "-q", "-m", "catalogue")
        res = self.assign(self.touch("templates/a/foo.md"))
        table = fx.read(self.root, "templates/README.md")
        self.assertIn("| [`a`](a/) | 1.0.1 | First. |", table)
        self.assertIn("| [`b`](b/) | 2.0.0 | Second. |", table)
        self.assertIn("| [`c`](c/) | 0.3.1 | Pins a. |", table)
        self.assertIn("templates/README.md", {p["path"] for p in res["pins"]})

    def test_cascade_moves_pin_literals_in_rust_tests(self):
        # W9 probe: a lock test asserts the pin a shipped config carries
        # (`"builder@1.26.38"` in gh504_...rs) -- the merge gate went red on
        # every merge that moved that pin. Only a quoted literal at the OLD
        # number moves; comments and other numbers are history.
        rs = ("// Since `a@1.0.0` the pin reads a@1.0.0.\n"
              "assert_eq!(pin, \"a@1.0.0\");\nassert_eq!(old, \"a@0.9.0\");\n")
        fx.write(self.root, "crates/x/tests/pin.rs", rs)
        fx.write(self.root, "tests/top.rs", "let p = \"a@1.0.0\";\n")
        fx.git(self.root, "add", "-A")
        fx.git(self.root, "commit", "-q", "-m", "rust pins")
        res = self.assign(self.touch("templates/a/foo.md"))
        self.assertEqual(fx.read(self.root, "crates/x/tests/pin.rs"),
                         rs.replace('"a@1.0.0"', '"a@1.0.1"'))
        self.assertEqual(fx.read(self.root, "tests/top.rs"), "let p = \"a@1.0.1\";\n")
        self.assertIn("crates/x/tests/pin.rs", {p["path"] for p in res["pins"]})

    def test_cascade_moves_an_escaped_pin_in_an_inlined_script(self):
        # GH #1169: `DEFAULT_SETTINGS = {\"member_screen_template\":
        # \"display@2.10.8\"}` inside the builder recipes' `script_inline`
        # stayed behind on the display bump -- the escaped quote before the
        # closing one is a pin all the same -- at any depth, the one form the
        # net checks (tb-s8 re-check m-7).
        js = ('{"script_inline": "S = {\\"t\\": \\"a@1.0.0\\"}",\n'
              ' "deeper": "x = \\"y = \\\\\\"a@1.0.0\\\\\\"\\""}\n')
        fx.write(self.root, "examples/inline/config.json", js)
        fx.git(self.root, "add", "-A")
        fx.git(self.root, "commit", "-q", "-m", "inline")
        res = self.assign(self.touch("templates/a/foo.md"))
        self.assertEqual(fx.read(self.root, "examples/inline/config.json"),
                         js.replace("a@1.0.0", "a@1.0.1"))
        self.assertIn("examples/inline/config.json", {p["path"] for p in res["pins"]})
        self.assertEqual(versions.cascade_proposal(self.root), [])

    def test_pins_cascade_write_raises_an_escaped_pin(self):
        # GH #1169, queue RED of tb-s8: `pins --cascade --write` replaced the
        # literal `"name@old"` and missed the escaped form its own proposal
        # names -- it printed "raised" and left the pin standing.
        js = '{"script_inline": "S = {\\"t\\": \\"a@0.9.0\\"}"}\n'
        fx.write(self.root, "examples/inline/config.json", js)
        self.assertEqual([p["from"] for p in versions.cascade_proposal(self.root)], ["0.9.0"])
        args = mock.Mock(repo=self.root, write=True)
        with contextlib.redirect_stdout(io.StringIO()):
            versions.cmd_cascade(args)
        self.assertEqual(fx.read(self.root, "examples/inline/config.json"),
                         js.replace("a@0.9.0", "a@1.0.0"))
        self.assertEqual(versions.cascade_proposal(self.root), [])

    def test_cascade_prose_moves_only_the_old_version(self):
        fx.write(self.root, "docs/y.md", "Since a@0.9.0 it reads by message; data@1.0.0 "
                                         "and a@1.0.0.5 are not pins.\n")
        fx.git(self.root, "add", "-A")
        fx.git(self.root, "commit", "-q", "-m", "y")
        self.assign(self.touch("templates/a/README.md"))
        self.assertEqual(fx.read(self.root, "docs/y.md"),
                         "Since a@0.9.0 it reads by message; data@1.0.0 "
                         "and a@1.0.0.5 are not pins.\n")

    def test_cascade_grep_rules_match_dev_rules_4a(self):
        rules = SCRIPTS.parent / "docs" / "development-rules.md"
        if not rules.is_file():
            self.skipTest("no docs/development-rules.md in this tree")
        text = rules.read_text(encoding="utf-8")
        section = text.split("### 4a.", 1)[1].split("\n### ", 1)[0]
        greps = re.findall(r"grep -rnE '([^']+)' ((?:[a-z]+/ ?)+)", section)
        generic = [(rx, tuple(p.rstrip("/") for p in paths.split()))
                   for rx, paths in greps if rx.startswith(r"\b[a-z]")]
        self.assertGreaterEqual(len(generic), 2, generic)
        (short_rx, short_paths), (full_rx, full_paths) = generic[0], generic[1]
        self.assertEqual(short_rx, versions.SHORT_GREP)
        self.assertEqual(full_rx, versions.FULL_GREP)
        self.assertEqual(full_paths, versions.SURFACES)
        self.assertEqual(short_paths, versions.SURFACES)
        # What the cascade writes in prose is what the second grep finds.
        self.assertTrue(re.search(versions.FULL_GREP, "runs a@1.0.1 today"))
        self.assertFalse(re.search(versions.FULL_GREP, "since a@1 the"))
        self.assertTrue(re.search(versions.SHORT_GREP, "since a@1 the"))


LINK = "[#101](https://github.com/mmeyerlein/meclaw/issues/101)"
OLD = "[#1](https://github.com/mmeyerlein/meclaw/issues/1)"


def with_purpose(root, d, purpose):
    path = "templates/%s/template.json" % d
    doc = json.loads(fx.read(root, path))
    doc["description"] = {"purpose": purpose}
    fx.write(root, path, json.dumps(doc, indent=2) + "\n")


def purpose(root, d):
    return json.loads(fx.read(root, "templates/%s/template.json" % d))["description"]["purpose"]


class HistoryTest(VersionsCase):
    """F5 (gh1038 `a_cascade_writes_its_own_sentence_and_keeps_the_old_ones`): every
    number the merge point gives, by a strand's kind or by the cascade, gets a
    `Since <x.y.z> (` sentence of its own; the sentences before it stay as written."""

    def setUp(self):
        super().setUp()
        with_purpose(self.root, "a", "Template a. Since 1.0.0 (%s) it reads by message." % OLD)
        with_purpose(self.root, "c", "Template c. Since 0.3.0 (#2) the template pins `a@1.0.0`, "
                                     "see %s." % OLD)
        fx.git(self.root, "add", "-A")
        fx.git(self.root, "commit", "-q", "-m", "purposes")

    def assign(self, changed, kinds=None, **history):
        return versions.assign(self.root, "master", changed, kinds or {},
                               history=dict({"issues": ["101"]}, **history))

    def test_cascade_writes_a_sentence_for_every_bumped_version(self):
        self.assign(self.touch("templates/a/README.md"), {"a": "feature"})
        a, c = purpose(self.root, "a"), purpose(self.root, "c")
        self.assertEqual(a, "Template a. Since 1.0.0 (%s) it reads by message. Since 1.1.0 "
                            "(%s) the template carries this change; it is the second digit."
                         % (OLD, LINK))
        # The cascade's own number: its own sentence, the pins it follows; the
        # sentence before it keeps the pin it was written with.
        self.assertEqual(c, "Template c. Since 0.3.0 (#2) the template pins `a@1.0.0`, see %s. "
                            "Since 0.3.1 (%s) the template pins `a@1.1.0`; it is the third digit."
                         % (OLD, LINK))
        # Every other key of the file stays as it was.
        doc = json.loads(fx.read(self.root, "templates/c/template.json"))
        self.assertEqual((doc["version"], doc["uses"]), ("0.3.1", ["a@1.1.0"]))

    def test_strands_own_sentence_takes_the_merge_points_number(self):
        # The strand numbered a itself (1.0.7) and wrote its sentence under that number.
        path = "templates/a/template.json"
        doc = json.loads(fx.read(self.root, path))
        doc["version"] = "1.0.7"
        doc["description"]["purpose"] += " Since 1.0.7 (#101) it reads by tap."
        fx.write(self.root, path, json.dumps(doc, indent=2) + "\n")
        self.assign([path])
        a = purpose(self.root, "a")
        self.assertEqual(a, "Template a. Since 1.0.0 (%s) it reads by message. "
                            "Since 1.0.1 (#101) it reads by tap." % OLD)

    def test_revert_sentence_names_the_revert(self):
        self.assign(self.touch("templates/a/README.md"), revert=True)
        self.assertTrue(purpose(self.root, "a").endswith(
            "Since 1.0.1 (revert of %s) the template takes this change back; it is the third "
            "digit." % LINK), purpose(self.root, "a"))

    def test_without_history_no_sentence(self):
        versions.assign(self.root, "master", self.touch("templates/a/README.md"), {})
        self.assertEqual(purpose(self.root, "a"),
                         "Template a. Since 1.0.0 (%s) it reads by message." % OLD)

    def test_a_purpose_without_links_gets_a_plain_issue(self):
        # gh766: the browser template carries no address; a purpose that links
        # no issue gets the issue as plain `#N`.
        with_purpose(self.root, "b", "Template b.")
        fx.git(self.root, "add", "-A")
        fx.git(self.root, "commit", "-q", "-m", "b")
        self.assign(self.touch("templates/b/README.md"))
        self.assertEqual(purpose(self.root, "b"), "Template b. Since 2.0.1 (#101) the template "
                                                   "carries this change; it is the third digit.")

    def test_a_generated_purpose_moves_in_its_generator(self):
        # builder-librarian: template.json is written by a generator under
        # workshop/tools/ that holds the purpose as a literal; both move.
        with_purpose(self.root, "a", purpose(self.root, "a") + " \u00a7 4.")
        raw = json.dumps(purpose(self.root, "a"), ensure_ascii=False)
        fx.write(self.root, "workshop/tools/gen_a.py", "DOC = {\n    \"purpose\": %s,\n}\n" % raw)
        fx.git(self.root, "add", "-A")
        fx.git(self.root, "commit", "-q", "-m", "gen")
        self.assign(self.touch("templates/a/README.md"))
        gen = fx.read(self.root, "workshop/tools/gen_a.py")
        self.assertIn(json.dumps(purpose(self.root, "a"), ensure_ascii=False), gen)
        self.assertIn("\u00a7 4. Since 1.0.1", gen)
        self.assertIn("Since 1.0.1 (%s)" % LINK, gen)

    def test_a_template_without_purpose_is_left_alone(self):
        res = self.assign(self.touch("templates/b/README.md"))
        self.assertEqual(res["versions"]["b"]["to"], "2.0.1")
        self.assertNotIn("description", json.loads(fx.read(self.root, "templates/b/template.json")))


if __name__ == "__main__":
    unittest.main()
