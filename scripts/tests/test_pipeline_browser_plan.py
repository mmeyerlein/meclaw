"""welle-tb S1b (#1165), owner's ruling 10.10., point 2: the browser locks of an integration.

An integration that narrows its tests runs a browser lock (the colony half and
the sheet among them) only when the wave can reach it, fail-closed: the wave
diff touches the lock or a file of its coverage parts, the lock is not
measured, the map does not fit, a non-Rust input of the station changed, or a
Rust file of the diff is not in the map. `release` runs every lock, a strand
plans as before, an integration in doubt runs every lock.
"""

import datetime
import os
import subprocess
import sys
import tempfile
import types
import unittest
import pathlib
from unittest import mock

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS))
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import gate_plan  # noqa: E402
from pipeline import browser_cover, browser_plan, narrow, plan, spawn_closure  # noqa: E402
from scripts.tests import store_isolation  # noqa: E402

NOW = datetime.datetime(2026, 10, 10, 12, 0, tzinfo=datetime.timezone.utc)
CRATE = "meclaw-cells"
LOCKS = sorted(gate_plan.BROWSER_LOCKS)
SHEET = next(l for l in LOCKS if "710_the_sheet_holds" in l)
COLONY = next(l for l in LOCKS if "710_the_colony_holds" in l)
CSP = next(l for l in LOCKS if "gh867_" in l)
MOTION = next(l for l in LOCKS if "gh961_" in l)
SRC = "crates/meclaw-cells/src/"
SUPPORT = "crates/meclaw-cells/tests/support/display_colony.rs"
GIT_ENV = {"GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@example.invalid",
           "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@example.invalid",
           "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1"}
ON = {narrow.ENV: "1"}


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()


def tearDownModule():  # noqa: N802 -- unittest's name
    store_isolation.restore()


def git(root, *args):
    return subprocess.run(["git", "-C", root] + list(args), check=True,
                          env=dict(os.environ, **GIT_ENV), capture_output=True,
                          text=True).stdout.strip()


def _stem(lock):
    return lock.rsplit("/", 1)[1].removesuffix(".rs")


def browser_row(colony=True):
    cmds = gate_plan.browser_cmds(colony)
    return {"name": "browser:display", "scope": "sheet+colony" if colony else "sheet",
            "cargo": True, "cmds": cmds,
            "select": gate_plan._browser_select(types.SimpleNamespace(cmds=cmds))}


def locks_of(row):
    """The lock paths a browser row drives (its `select`)."""
    extra = set((row.get("select") or {}).get("extra") or ())
    return {l for l in LOCKS if browser_cover.binary_of(l) in extra}


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = self.tmp.name
        files = {"crates/meclaw-cells/Cargo.toml": '[package]\nname = "meclaw-cells"\n',
                 SRC + "lib.rs": "pub mod sheet;\n", SRC + "sheet.rs": "// sheet\n",
                 SRC + "colony.rs": "// colony\n", SRC + "shared.rs": "// shared\n",
                 SRC + "other.rs": "// other\n", SRC + "new.rs": "// not in the map\n",
                 SUPPORT: "// support\n", "docs/x.md": "docs\n",
                 "templates/display/x.css": "a {}\n", "templates/web/y.json": "{}\n",
                 "workshop/tools/display-layout-browser.mjs": "//\n",
                 "workshop/tools/wkenv.sh": "#\n", ".config/nextest.toml": "#\n"}
        files.update({l: "#[test]\n#[ignore]\nfn t() {}\n" for l in LOCKS})
        self.write(files, commit=False)
        git(self.root, "init", "-q", "-b", "master")
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", "map rev")
        self.map_rev = git(self.root, "rev-parse", "HEAD")
        self.meta = self.metadata()
        self.cov = self.map()
        spawn_closure.clear_caches()
        gate_plan._SOURCE_CACHE.clear()

    def write(self, files, commit=True):
        for rel, text in files.items():
            path = os.path.join(self.root, rel)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w", encoding="utf-8") as fh:
                fh.write(text)
        if commit:
            git(self.root, "add", "-A")
            git(self.root, "commit", "-q", "-m", "edit")
            gate_plan._SOURCE_CACHE.clear()
            spawn_closure.clear_caches()
            return git(self.root, "rev-parse", "HEAD")
        return None

    def metadata(self):
        d = os.path.join(self.root, "crates", CRATE)
        targets = [{"kind": ["lib"], "name": "meclaw_cells",
                    "src_path": os.path.join(d, "src/lib.rs")}]
        targets += [{"kind": ["test"], "name": _stem(l), "src_path": os.path.join(self.root, l)}
                    for l in LOCKS]
        pkg = {"id": "%s 0.1.0" % CRATE, "name": CRATE, "targets": targets,
               "manifest_path": os.path.join(d, "Cargo.toml"), "dependencies": []}
        return {"workspace_members": [pkg["id"]], "packages": [pkg]}

    def map(self, **kw):
        parts = [{"binary": CRATE, "module": "tests",
                  "files": {SRC + "other.rs": 3, SRC + "lib.rs": 1}}]
        for l in LOCKS:
            files = {l: 4, SRC + "shared.rs": 1}
            if l == SHEET:
                files[SRC + "sheet.rs"] = 2
            if l == COLONY:
                files.update({SRC + "colony.rs": 5, SUPPORT: 7})
            parts.append({"binary": browser_cover.binary_of(l), "module": None, "files": files})
        cov = {"schema": "meclaw.pipeline.coverage/1", "rev": self.map_rev,
               "built_at": "2026-10-10T02:00:00Z", "runner": "lane", "granularity": "binary",
               "complete": True, "parts": parts,
               browser_cover.FIELD: {l: {"binary": browser_cover.binary_of(l), "measured": True,
                                         "tests": 2} for l in LOCKS}}
        cov.update(kw)
        return cov

    def apply(self, files, cov=None, meta=True, rows=None, colony=True):
        rows = rows if rows is not None else [{"name": "tests", "scope": "x"}, browser_row(colony)]
        notes = browser_plan.apply(rows, files, self.root, metadata=self.meta if meta else None,
                                   env=ON, now=NOW, cov=self.cov if cov is None else cov)
        row = next((r for r in rows if r["name"] == "browser:display"), None)
        return row, notes, rows

    def assert_all_run(self, row, notes, why):
        self.assertEqual(row["cmds"], gate_plan.browser_cmds(True), notes)
        for l in LOCKS:
            line = [n for n in notes if n.startswith("browser lock %s: " % _stem(l))]
            self.assertEqual(1, len(line), (l, notes))
            self.assertTrue(line[0].startswith("browser lock %s: runs (" % _stem(l)), line)
            self.assertIn(why, line[0])


class RuleTest(Base):
    def test_a_touched_cover_runs_the_lock_and_an_untouched_measured_lock_leaves(self):
        row, notes, _rows = self.apply([SRC + "colony.rs"])
        self.assertEqual({COLONY}, locks_of(row), notes)
        self.assertEqual([["scripts/test-tier.sh", "filter", gate_plan.COLONY_LOCK,
                           "--run-ignored", "all"]], [row["cmd"]] if "cmd" in row else row["cmds"])
        self.assertIn("browser lock %s: runs (covers %s)" % (_stem(COLONY), SRC + "colony.rs"),
                      notes)
        self.assertIn("browser lock %s: skipped (measured, untouched)" % _stem(CSP), notes)
        self.assertEqual(gate_plan.COLONY_LOCK, row["select"]["filter"])

    def test_the_lock_file_itself_runs_its_lock(self):
        row, notes, _rows = self.apply([MOTION])
        self.assertEqual({MOTION}, locks_of(row), notes)
        self.assertIn("browser lock %s: runs (covers %s)" % (_stem(MOTION), MOTION), notes)

    def test_an_unmeasured_lock_runs(self):
        field = dict(self.cov[browser_cover.FIELD])
        field[CSP] = dict(field[CSP], measured=False, why="SKIP no chromium")
        del field[MOTION]
        row, notes, _rows = self.apply([SRC + "other.rs"],
                                       cov=self.map(**{browser_cover.FIELD: field}))
        self.assertEqual({CSP, MOTION}, locks_of(row), notes)
        self.assertIn("browser lock %s: runs (not measured: SKIP no chromium)" % _stem(CSP), notes)
        self.assertIn("browser lock %s: runs (not measured: no entry in the map)" % _stem(MOTION),
                      notes)
        # a map without the field measured nothing
        cov = self.map()
        del cov[browser_cover.FIELD]
        row, notes, _rows = self.apply([SRC + "other.rs"], cov=cov)
        self.assert_all_run(row, notes, "not measured")

    def test_a_map_that_does_not_fit_runs_every_lock(self):
        cases = ((dict(self.cov, complete=False), True, "is not complete"),
                 (dict(self.cov, built_at="2026-09-30T00:00:00Z"), True, "days old"),
                 (dict(self.cov, built_at="2026-10-11T12:00:00Z"), True, "in the future"),
                 (dict(self.cov, built_at=None), True, "no valid built_at"),
                 (self.cov, False, "no cargo metadata"),
                 (dict(self.cov, rev="0" * 40), True, "not in this repository"))
        for cov, meta, why in cases:
            row, notes, _rows = self.apply([SRC + "other.rs"], cov=cov, meta=meta)
            self.assert_all_run(row, notes, why)
        with mock.patch.object(narrow, "newest_map", return_value=(None, "no coverage map in /x")):
            rows = [browser_row()]
            notes = browser_plan.apply(rows, [SRC + "other.rs"], self.root, metadata=self.meta,
                                       env=ON, now=NOW)
        self.assert_all_run(rows[0], notes, "no coverage map in /x")

    def test_a_station_input_outside_rust_runs_every_lock(self):
        for path in ("templates/display/x.css", "workshop/tools/display-layout-browser.mjs",
                     ".config/nextest.toml", "templates/web/y.json", "workshop/tools/wkenv.sh",
                     "workshop/tools/display-lab/fact_yield.py"):
            row, notes, _rows = self.apply([SRC + "other.rs", path])
            self.assert_all_run(row, notes, "station input %s" % path)

    def test_a_rust_file_outside_the_map_runs_every_lock(self):
        for path in (SRC + "new.rs", "crates/meclaw-cells/Cargo.toml",
                     "crates/meclaw-cells/build.rs", "crates/meclaw-cells/tests/gh9_new.rs"):
            row, notes, _rows = self.apply([path])
            self.assert_all_run(row, notes, "%s not in the coverage map" % path)
        for path in ("Cargo.lock", "Cargo.toml"):
            row, notes, _rows = self.apply([path])
            self.assert_all_run(row, notes, "workspace file %s" % path)

    def test_a_crate_file_the_map_cannot_see_runs_every_lock(self):
        # Re-check S1b (B1): `crates/meclaw-core/schemas/ubf-body.json` is compiled
        # into meclaw-core by `include_str!("../schemas/ubf-body.json")` -- it is
        # neither rust_src nor rust_test, the map (.rs only) never shows it, and
        # the rule took it for untouched: every lock left the plan.
        for path in ("crates/meclaw-core/schemas/ubf-body.json",
                     "crates/meclaw-cells/assets/x.js"):
            row, notes, _rows = self.apply([SRC + "other.rs", path])
            self.assertIsNotNone(row, notes)
            self.assert_all_run(row, notes, "%s not in the coverage map" % path)

    def test_the_colony_half_leaves_when_untouched_and_measured(self):
        row, notes, _rows = self.apply([SRC + "sheet.rs"])
        self.assertEqual({SHEET}, locks_of(row), notes)
        self.assertEqual(gate_plan.SHEET_LOCK_CMD, row["cmd"])
        self.assertNotIn("cmds", row)
        self.assertNotIn("710_the_colony_holds", str(row))
        self.assertIn("browser lock %s: skipped (measured, untouched)" % _stem(COLONY), notes)

    def test_nothing_touched_drops_the_row(self):
        for files in ([], ["docs/x.md"], [SRC + "other.rs"]):
            row, notes, rows = self.apply(files)
            self.assertIsNone(row, notes)
            self.assertEqual(["tests"], [r["name"] for r in rows])
            self.assertIn("browser:display: no lock runs -- the station leaves the plan", notes)
            self.assertEqual(len(LOCKS), sum(1 for n in notes if "skipped (measured, untouched)"
                                             in n), notes)

    def test_files_since_the_map_rev_count_as_wave_files(self):
        self.write({SRC + "colony.rs": "// changed after the map\n"})
        row, notes, _rows = self.apply([])
        self.assertEqual({COLONY}, locks_of(row), notes)
        # a lock source changed since the map: its binary is stale in the map
        self.write({CSP: "#[test]\n#[ignore]\nfn t2() {}\n"})
        row, notes, _rows = self.apply([])
        self.assertEqual({COLONY, CSP}, locks_of(row), notes)
        self.assertIn("browser lock %s: runs (changed since map %s: binary_id(=%s))"
                      % (_stem(CSP), self.map_rev[:9], browser_cover.binary_of(CSP)), notes)

    def test_a_lock_that_spawns_a_workspace_binary_runs_on_any_rust_change(self):
        self.write({SUPPORT: 'const B: &str = env!("CARGO_BIN_EXE_meclaw");\n'}, commit=False)
        row, notes, _rows = self.apply([SRC + "other.rs"])
        self.assertEqual({COLONY}, locks_of(row), notes)
        self.assertIn("browser lock %s: runs (spawns a workspace binary: %s)"
                      % (_stem(COLONY), SUPPORT), notes)

    def test_a_row_without_the_colony_half_keeps_it_out(self):
        row, notes, _rows = self.apply([SRC + "colony.rs", SRC + "sheet.rs"], colony=False)
        self.assertEqual({SHEET}, locks_of(row), notes)

    def test_the_switch_off_leaves_the_row(self):
        rows = [browser_row()]
        notes = browser_plan.apply(rows, [], self.root, metadata=self.meta,
                                   env={narrow.ENV: "0"}, now=NOW, cov=self.cov)
        self.assertEqual(gate_plan.browser_cmds(True), rows[0]["cmds"])
        self.assertTrue(notes and "off" in notes[0], notes)


class BrowserCmdsTest(unittest.TestCase):
    def test_keep_none_is_the_old_plan(self):
        for colony in (True, False):
            self.assertEqual(gate_plan.browser_cmds(colony),
                             gate_plan.browser_cmds(colony, keep=None))
            self.assertEqual(2, len(gate_plan.browser_cmds(colony)))

    def test_keep_filters_terms_and_the_sheet(self):
        self.assertEqual([], gate_plan.browser_cmds(True, keep=set()))
        self.assertEqual([gate_plan.SHEET_LOCK_CMD], gate_plan.browser_cmds(True, keep={SHEET}))
        both = gate_plan.browser_cmds(True, keep={COLONY, MOTION})
        self.assertEqual([["scripts/test-tier.sh", "filter",
                           gate_plan.COLONY_LOCK + " + " + gate_plan.MOTION_LOCK,
                           "--run-ignored", "all"]], both)
        # keep never adds the colony half to a plan without it
        self.assertEqual([], gate_plan.browser_cmds(False, keep={COLONY}))
        self.assertEqual(gate_plan.browser_cmds(True),
                         gate_plan.browser_cmds(True, keep=set(LOCKS)))


class PlanWiringTest(Base):
    """`plan.build_plan`: the rule hooks into a narrowed integration only."""

    def resolver(self, repo, mode, files):
        out = gate_plan.to_json(files, mode, repo, explain=True)
        return {"stations": [r for r in out["stations"] if r["name"] == "browser:display"]}

    def build(self, mode, files, term="stage:integration narrowed since=x"):
        decision = (term, self.map_rev, ["no wave named"])
        with mock.patch.object(narrow, "integration_decision", return_value=decision), \
                mock.patch.object(narrow, "newest_map", return_value=(self.cov, None)), \
                mock.patch.dict(os.environ, ON):
            doc = plan.build_plan(self.root, mode, self.map_rev, files=files,
                                  resolver=self.resolver, metadata=self.meta, narrow=True,
                                  db=object())
        st = next((s for s in doc["stations"] if s["name"] == "browser:display"), None)
        return doc, st

    def argvs(self, st):
        return [c["argv"] for c in st["cmds"]]

    def shared(self, colony):
        """`browser_cmds` without the locks that run alone (GH #1191, `plan.split_alone`)."""
        rows = [{"name": "browser:display", "cmds": gate_plan.browser_cmds(colony)}]
        plan.split_alone(rows)
        return rows[0]["cmds"]

    def test_a_narrowed_integration_drops_the_untouched_locks_release_runs_all(self):
        self.write({SRC + "sheet.rs": "// in the wave\n"})
        doc, st = self.build("integration", ["docs/x.md"])
        self.assertEqual([gate_plan.SHEET_LOCK_CMD], self.argvs(st), doc.get("notes"))
        self.assertTrue(any(n.startswith("browser lock ") for n in doc.get("notes") or ()))
        self.assertEqual(gate_plan.SHEET_LOCK_CMD[2], st["nextest"]["filter"])
        # release: the rule never applies -- colony half and every lock
        doc, st = self.build("release", ["docs/x.md"])
        self.assertEqual(self.shared(True), self.argvs(st))
        self.assertFalse(any(n.startswith("browser lock ") for n in doc.get("notes") or ()))

    def test_an_untouched_integration_plans_no_browser_station(self):
        doc, st = self.build("integration", ["docs/x.md"])
        self.assertIsNone(st, doc.get("notes"))
        self.assertIn("browser:display: no lock runs -- the station leaves the plan", doc["notes"])

    def test_an_integration_in_doubt_runs_every_lock(self):
        doc, st = self.build("integration", ["docs/x.md"], term=None)
        self.assertEqual(self.shared(True), self.argvs(st))
        alone = next(s for s in doc["stations"] if s["name"] == plan.ALONE)
        self.assertEqual([["scripts/test-tier.sh", "filter", gate_plan.SLOW3G_LOCK,
                           "--run-ignored", "all"]], self.argvs(alone))

    def test_a_strand_plans_as_before(self):
        doc, st = self.build("strand", ["templates/display/x.css"])
        self.assertEqual(self.shared(False), self.argvs(st))
        self.assertFalse(any(n.startswith("browser lock ") for n in doc.get("notes") or ()))


EXTRA = "crates/meclaw-cells/tests/zz_the_page_draws_browser.rs"
EXTRA_TERM = "binary_id(=meclaw-cells::zz_the_page_draws_browser)"


class OrphanWiringTest(Base):
    """Re-check tb-s3 nb m-1 (n2 m-1): the two calls that carry a non-lock browser
    unit through a cut integration -- `browser_plan.apply` keeps the units the wave
    diff reaches (`moved=`), `plan.build_plan` plans the units `tests` subtracts
    beyond the wave diff (`plan_browser_orphans(locks=False)`). Each one alone
    carries its case: without it the unit runs in no station."""

    def setUp(self):
        super().setUp()
        self.write({EXTRA: "#[test]\nfn draws() {}\n"}, commit=False)
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "--amend", "--no-edit")
        self.map_rev = git(self.root, "rev-parse", "HEAD")
        gate_plan._SOURCE_CACHE.clear()
        gate_plan._BROWSER_CACHE.clear()
        self.meta["packages"][0]["targets"].append(
            {"kind": ["test"], "name": "zz_the_page_draws_browser",
             "src_path": os.path.join(self.root, EXTRA)})
        self.cov = self.map()
        self.cov["parts"].append({"binary": browser_cover.binary_of(EXTRA), "module": None,
                                  "files": {EXTRA: 3}})
        self.addCleanup(gate_plan._BROWSER_CACHE.clear)

    resolver = PlanWiringTest.resolver
    build = PlanWiringTest.build
    argvs = PlanWiringTest.argvs

    def test_the_fixture_unit_is_a_non_lock_browser_unit(self):
        self.assertIn(EXTRA_TERM, gate_plan.browser_terms(self.root))
        self.assertNotIn(EXTRA, gate_plan.BROWSER_LOCKS)

    def test_the_cut_keeps_a_unit_the_wave_diff_reaches(self):
        """R4 (`browser_plan.py` `moved=()`): the wave touches only the unit, every
        lock is measured and untouched -- the cut keeps no lock, and the unit the
        wave reaches is all the station runs. `tests` reaches nothing here, so no
        later step could carry it."""
        self.write({EXTRA: "#[test]\nfn draws() { /* in the wave */ }\n"})
        doc, st = self.build("integration", ["docs/x.md"])
        self.assertIsNotNone(st, doc.get("notes"))
        self.assertEqual([[["scripts/test-tier.sh", "filter", EXTRA_TERM, "--run-ignored", "all"]]],
                         [self.argvs(st)], doc.get("notes"))
        self.assertTrue(any(n.startswith("browser lock ") and "skipped" in n
                            for n in doc.get("notes") or ()), doc.get("notes"))

    def test_a_unit_tests_reaches_beyond_the_wave_diff_is_planned(self):
        """R3 (`plan.py` without `plan_browser_orphans(locks=False)` after the cut):
        the export diff reaches the unit through `rdeps(meclaw-cells)`, `tests`
        subtracts it, the wave diff (a page) plans no test, so the export row stays
        -- the cut drops the whole station, and only the orphan step plans the unit
        again, booked in `browser:display`."""
        def resolver(repo, mode, files):
            out = gate_plan.to_json(files, mode, repo, explain=True)
            return {"stations": [r for r in out["stations"]
                                 if r["name"] in ("tests", "browser:display")]}
        self.resolver = resolver
        self.write({"docs/x.md": "the wave\n"})
        doc, st = self.build("integration", [SRC + "other.rs"])
        self.assertIn("narrowing: stage:integration narrowed since=x -- the wave diff plans "
                      "no test, the resolver row stays", doc["notes"])
        tests = next(s for s in doc["stations"] if s["name"] == "tests")
        self.assertIn(EXTRA_TERM, " ".join(tests["cmds"][0]["argv"]).partition(" - ")[2])
        self.assertIsNotNone(st, doc.get("notes"))
        self.assertEqual([["scripts/test-tier.sh", "filter", EXTRA_TERM, "--run-ignored", "all"]],
                         self.argvs(st), doc.get("notes"))
        self.assertIn("browser:display: no lock runs -- the station leaves the plan",
                      doc["notes"])
        self.assertEqual(0, st["needs"]["cargo"])
        self.assertEqual(1, st["needs"]["browser_webkit"])

    def export_resolver(self, repo, mode, files):
        out = gate_plan.to_json(files, mode, repo, explain=True)
        return {"stations": [r for r in out["stations"]
                             if r["name"] in ("tests", "browser:display")]}

    def test_gh1182_the_cut_integration_refuses_a_unit_no_station_runs(self):
        """GH #1182, the third call (`plan.py` after `plan_browser_orphans(locks=False)`):
        the case above with no orphan step (as at tb-s3 aa9454842) -- the wave diff
        plans no test, so `narrow.apply` and its guard never run; the export row
        subtracts the unit, the cut drops the station, and only the guard behind
        the cut sees that no station runs it. The plan is refused."""
        self.resolver = self.export_resolver
        self.write({"docs/x.md": "the wave\n"})
        with mock.patch.object(narrow, "plan_browser_orphans", return_value=[]), \
                self.assertRaises(ValueError) as cm:
            self.build("integration", [SRC + "other.rs"])
        self.assertIsInstance(cm.exception, narrow.UnrunSubtraction)
        self.assertIn(EXTRA_TERM, str(cm.exception))

    def test_gh1182_a_lock_the_cut_drops_is_the_rulings(self):
        """The lock cut behind the third call: a narrowed integration runs a lock
        only when the wave reaches it (owner's ruling 10.10., `browser_plan`), so
        a lock `tests` subtracts and the cut dropped is no unrun subtraction
        there (`locks=False`, as in the orphan step). The sheet runs a test that
        is not `#[ignore]`d -- only the cut exempts it, never the count."""
        self.write({SHEET: "#[test]\nfn t() {}\n"}, commit=False)
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "--amend", "--no-edit")
        self.map_rev = git(self.root, "rev-parse", "HEAD")
        gate_plan._SOURCE_CACHE.clear()
        gate_plan._BROWSER_CACHE.clear()
        self.cov = self.map()
        self.cov["parts"].append({"binary": browser_cover.binary_of(EXTRA), "module": None,
                                  "files": {EXTRA: 3}})
        self.resolver = self.export_resolver
        self.write({"docs/x.md": "the wave\n"})
        doc, st = self.build("integration", [SRC + "other.rs"])
        tests = next(s for s in doc["stations"] if s["name"] == "tests")
        sheet = "binary_id(=meclaw-cells::%s)" % _stem(SHEET)
        self.assertIn(sheet, " ".join(tests["cmds"][0]["argv"]).partition(" - ")[2])
        self.assertIn("browser:display: no lock runs -- the station leaves the plan",
                      doc["notes"])
        self.assertEqual([["scripts/test-tier.sh", "filter", EXTRA_TERM, "--run-ignored", "all"]],
                         self.argvs(st), doc.get("notes"))


if __name__ == "__main__":
    unittest.main()
