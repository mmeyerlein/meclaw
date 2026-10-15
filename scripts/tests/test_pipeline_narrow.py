"""The narrowing of a strand gate (OR-DT.W6.5, GH #1138): on by default, fail-closed.

The synthetic workspace and the coverage map are the ones of
`test_pipeline_impact` (two crates, a complete map of rev `fixture`).
"""

import contextlib
import copy
import datetime
import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS))
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import gate_plan  # noqa: E402
from pipeline import events, graph as graph_mod, impact, narrow, plan, store, versions, wave  # noqa: E402
from scripts.tests import store_isolation  # noqa: E402
from test_pipeline_impact import COVERAGE, NOW, Base  # noqa: E402
from test_pipeline_graph import FILES  # noqa: E402
import test_pipeline_impact  # noqa: E402
from test_pipeline_graph import FILES as GRAPH_FILES  # noqa: E402

CHANGED = ["crates/alpha/src/store.rs"]
FILES_CARGO = GRAPH_FILES["Cargo.toml"]
GIT_ENV = {"GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@example.invalid",
           "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@example.invalid",
           "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1"}


def git(root, *args):
    env = dict(os.environ, **GIT_ENV)
    return subprocess.run(["git", "-C", root] + list(args), check=True, env=env,
                          capture_output=True, text=True).stdout.strip()


def metadata(root):
    """`cargo metadata` of the synthetic workspace: what `gate` hands the plan."""
    pkgs = []
    for crate, tests in (("alpha", ("a1_reads_doc", "a2_template")),
                         ("beta", ("b1_uses_alpha", "b2_ignored"))):
        d = os.path.join(root, "crates", crate)
        targets = [{"kind": ["lib"], "name": crate, "src_path": os.path.join(d, "src/lib.rs")}]
        targets += [{"kind": ["test"], "name": t, "src_path": os.path.join(d, "tests/%s.rs" % t)}
                    for t in tests]
        deps = [{"name": "alpha"}] if crate == "beta" else []
        pkgs.append({"id": "%s 0.1.0" % crate, "name": crate, "targets": targets,
                     "manifest_path": os.path.join(d, "Cargo.toml"), "dependencies": deps})
    return {"workspace_members": [p["id"] for p in pkgs], "packages": pkgs}


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()  # and the metrics URL, as test_pipeline_impact
    test_pipeline_impact._METRICS_URL.start()


def tearDownModule():  # noqa: N802 -- unittest's name
    test_pipeline_impact._METRICS_URL.stop()
    store_isolation.restore()


def filter_terms(filt):
    """The summands of an executed nextest filter, minus its scenario subtraction.

    An oracle of its own (B-2): split at top-level ` + `, stop at top-level
    ` - `; never the narrowing's own bookkeeping."""
    out, depth, cur, i = [], 0, "", 0
    filt = filt or ""
    while i < len(filt):
        ch = filt[i]
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
        if depth == 0 and filt.startswith(" + ", i):
            out.append(cur)
            cur, i = "", i + 3
            continue
        if depth == 0 and filt.startswith(" - ", i):
            break
        cur += ch
        i += 1
    if cur:
        out.append(cur)
    return set(out)


class NarrowTest(Base):
    def setUp(self):
        super().setUp()
        git(self.root, "init", "-q", "-b", "master")
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", "map rev")
        self.map_rev = git(self.root, "rev-parse", "HEAD")
        self.cov = dict(COVERAGE, rev=self.map_rev)
        self.meta = metadata(self.root)

    def floor(self, paths=CHANGED):
        return gate_plan.test_select(paths, "strand", self.root)

    def run_narrow(self, cov=None, why=None, graph=True, paths=CHANGED, meta=True):
        cov = self.cov if cov is None else cov
        return narrow.narrow(self.floor(paths), paths, self.root, self.doc if graph else None,
                             cov or None, why, now=NOW, metadata=self.meta if meta else None)

    def commit(self, files=None, mv=None, rm=None):
        for rel, text in (files or {}).items():
            path = os.path.join(self.root, rel)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w", encoding="utf-8") as fh:
                fh.write(text)
        if mv:
            git(self.root, "mv", *mv)
        if rm:
            git(self.root, "rm", "-q", rm)
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", "after the map")
        gate_plan._SOURCE_CACHE.clear()

    def test_a_fresh_complete_map_replaces_rdeps(self):
        self.assertIn("alpha", self.floor()["rdeps"])
        sel, notes = self.run_narrow()
        self.assertNotIn("alpha", sel["rdeps"])
        self.assertIn("beta::b1_uses_alpha", sel["binaries"])
        self.assertTrue(any(n.startswith("narrowing: on, map %s" % self.map_rev[:9]) for n in notes),
                        notes)

    def test_the_selection_carries_the_narrowing_state(self):
        """Review Z-I gap C-A: the cache key reads the narrowing state (on or
        off, map rev) from the selection."""
        sel, _notes = self.run_narrow()
        self.assertEqual({"on": True, "map": self.map_rev}, sel["narrow"])
        off, _notes = self.run_narrow(cov=False, why="no coverage map in /x")
        self.assertEqual({"on": False, "map": ""}, off["narrow"])

    def test_no_map_keeps_rdeps_with_a_note(self):
        sel, notes = self.run_narrow(cov=False, why="no coverage map in /x")
        self.assertIn("alpha", sel["rdeps"])
        self.assertEqual(["narrowing: no coverage map in /x -- floor + graph, rdeps kept"], notes)

    def test_a_stale_map_keeps_rdeps_with_a_note(self):
        sel, notes = self.run_narrow(cov=dict(self.cov, built_at="2026-09-30T00:00:00Z"))
        self.assertIn("alpha", sel["rdeps"])
        self.assertTrue(notes and "days old (> 7)" in notes[0], notes)

    def test_a_partial_map_keeps_rdeps_with_a_note(self):
        sel, notes = self.run_narrow(cov=dict(self.cov, complete=False))
        self.assertIn("alpha", sel["rdeps"])
        self.assertIn("is not complete", notes[0])

    def test_a_map_of_an_older_rev_narrows_when_the_tree_fits(self):
        self.commit({"docs/guide.md": "Changed after the map.\n"})
        sel, _notes = self.run_narrow()
        self.assertNotIn("alpha", sel["rdeps"])

    # --- review Z-I1 C1: the map has to fit the tree ---------------------------

    def assert_fallback(self, sel, notes, phrase):
        self.assertIn("alpha", sel["rdeps"], notes)
        self.assertTrue(any(phrase in n for n in notes), (phrase, notes))
        self.assertFalse(any(n.startswith("narrowing: on") for n in notes), notes)

    def test_a_map_with_binaries_head_lacks_falls_back(self):
        # The W6 case: 1175 of 1355 ids gone after the harness fold.
        cov = copy.deepcopy(self.cov)
        cov["parts"].append({"binary": "alpha::folded_away", "module": None,
                             "files": {"crates/alpha/src/store.rs": 1}})
        sel, notes = self.run_narrow(cov=cov)
        self.assert_fallback(sel, notes, "1 binaries HEAD does not have (alpha::folded_away)")

    def test_without_metadata_the_map_cannot_be_checked(self):
        sel, notes = self.run_narrow(meta=False)
        self.assert_fallback(sel, notes, "no cargo metadata")

    def test_a_map_rev_the_repository_lacks_falls_back(self):
        sel, notes = self.run_narrow(cov=dict(self.cov, rev="b6d6e062b" + "0" * 31))
        self.assert_fallback(sel, notes, "is not in this repository")

    def test_a_renamed_test_since_the_map_falls_back(self):
        self.commit(mv=("crates/alpha/tests/a1_reads_doc.rs", "crates/alpha/tests/a1_moved.rs"))
        sel, notes = self.run_narrow()
        self.assert_fallback(sel, notes, "R crates/alpha/tests/a1_reads_doc.rs")

    def test_a_deleted_test_since_the_map_falls_back(self):
        self.commit(rm="crates/beta/tests/b1_uses_alpha.rs")
        sel, notes = self.run_narrow()
        self.assert_fallback(sel, notes, "D crates/beta/tests/b1_uses_alpha.rs")

    def test_a_test_changed_since_the_map_is_always_planned(self):
        self.commit({"crates/alpha/tests/a2_template.rs": "#[test]\nfn changed() {}\n"})
        sel, notes = self.run_narrow()
        self.assertNotIn("alpha", sel["rdeps"], notes)
        self.assertIn("alpha::a2_template", sel["binaries"])

    def test_a_source_changed_since_the_map_brings_its_cover(self):
        self.commit({"crates/alpha/src/net.rs": "pub fn net2() {}\n"})
        sel, notes = self.run_narrow()
        # M-4 (OR-TB.S1b.16): the cover rides along, and rdeps(alpha) stays --
        # the map knows the old net.rs (was: assertNotIn, cover only)
        self.assertIn("alpha", sel["rdeps"], notes)
        self.assertIn("alpha::a1_reads_doc", sel["binaries"])   # cidx[net.rs]

    # --- M-4 (welle-tb S1b, #1165): files changed since the map, by class -----

    def ran(self, sel):
        """The binaries a narrowed selection RUNS: the executed filter over the
        graph (B-2) -- never the fields `rdeps`/`binaries`, which only build."""
        return impact.expand(impact.graph_mod.Index(self.doc),
                             sorted(filter_terms(sel["filter"])))[0]

    def executed(self, paths=CHANGED, extra_rows=(), out=None):
        """`(select, filter, notes)` through `narrow.apply`: the filter the
        tests command really hands to `test-tier.sh filter`."""
        floor = self.floor(paths)
        rows = [{"name": "tests", "scope": floor["filter"],
                 "cmd": ["scripts/test-tier.sh", "filter", floor["filter"]], "select": floor}]
        rows += list(extra_rows)
        try:
            with mock.patch.object(narrow, "newest_map", return_value=(self.cov, None)), \
                    mock.patch("pipeline.graph.load", return_value=self.doc):
                notes = narrow.apply(rows, paths, self.root, env={narrow.ENV: "1"}, now=NOW,
                                     metadata=self.meta)
        finally:
            if out is not None:
                out[:] = rows   # what `apply` planned beside `tests`, also when it refused
        return rows[0]["select"], rows[0]["cmd"][-1], notes

    def assert_plan_is_run(self, sel, filt, notes):
        """Plan == execution (B-2): every rdeps the plan keeps is a summand of
        the executed filter, every binary it plans runs under it."""
        got = filter_terms(filt)
        self.assertFalse([n for n in notes if "plan != run" in n], notes)
        self.assertEqual(sel["filter"], filt, notes)
        for c in sel["rdeps"]:
            self.assertIn("rdeps(%s)" % c, got, (filt, notes))
        ran = impact.expand(impact.graph_mod.Index(self.doc), sorted(got))[0]
        self.assertLessEqual(set(sel["binaries"]), ran, (filt, notes))

    def test_b2_the_executed_filter_carries_rdeps_of_a_source_since_the_map(self):
        """B-2: rdeps(alpha) of net.rs since the map stood in the fields only;
        the filter test-tier.sh got had none of it (fail-open)."""
        self.commit({"crates/alpha/src/net.rs": "pub fn net2() {}\n"})
        sel, filt, notes = self.executed()
        self.assertIn("rdeps(alpha)", filter_terms(filt), (filt, notes))
        self.assert_plan_is_run(sel, filt, notes)

    def test_b2_the_executed_filter_carries_a_test_changed_since_the_map(self):
        """B-2, the `map_fits` half (Vorbestand): a test changed since the map
        is always planned -- and must run, not only be built."""
        self.commit({"crates/alpha/tests/a2_template.rs": "#[test]\nfn changed() {}\n"})
        sel, filt, notes = self.executed()
        self.assertIn("binary_id(=alpha::a2_template)", filter_terms(filt), (filt, notes))
        self.assert_plan_is_run(sel, filt, notes)

    def test_i_h1_a_test_name_git_quotes_since_the_map_is_planned(self):
        """Re-check I-h1 (h), the class of `git ... --name-*` without `-z`:
        `crates/alpha/tests/\\303\\244.rs` came quoted, sat in no crate and
        was neither a term nor a file -- the changed test planned nothing."""
        self.commit({"crates/alpha/tests/ä.rs": "#[test]\nfn quoted() {}\n"})
        ok, notes, files, terms = narrow.map_fits(self.cov, self.root, self.meta)
        self.assertTrue(ok, notes)
        self.assertIn("rdeps(alpha)", terms, (files, notes))

    def test_b2_the_run_guard_names_a_planned_term_the_filter_lacks(self):
        """The runtime half of the guard: a planned term missing from the
        filter is named (the narrowing then keeps the resolver's selection);
        the floor counts by its summands, the scenario subtraction never."""
        part = "(binary_id(=alpha) & test(/^net::/))"
        filt = "%s + rdeps(alpha) - (binary_id(=x) + test(e2e))" % gate_plan.T0_FLOOR
        scen = {"binary_id(=x)", "test(e2e)"}   # GH #1182: exempt by declaration
        self.assertEqual([part], narrow.unplanned_run(
            [gate_plan.T0_FLOOR, "rdeps(alpha)", part], filt, exempt=scen))
        self.assertEqual(["binary_id(=x)"], narrow.unplanned_run(["binary_id(=x)"], filt,
                                                                 exempt=scen))
        self.assertEqual([], narrow.unplanned_run([gate_plan.T0_FLOOR, "rdeps(alpha)"], filt,
                                                  exempt=scen))

    def test_gh1182_a_subtracted_unit_no_station_runs_is_unplanned(self):
        """GH #1182, the case of tb-s3 aa9454842: `tests` subtracted a browser
        unit that only the since-map rdeps reached, and no other station ran
        it. `summands` stopped at the first ` - `, so the guard took the
        subtraction for planned and stayed silent. L-1 of re-check tb-s3 nb:
        after the orphans are planned into `browser:display`, a row whose
        command is no `test-tier.sh filter` call (or that has none) runs
        nothing -- `elsewhere` then lacks the unit and the guard must name it."""
        unit = "binary_id(=meclaw-cells::gh703_the_phone_holds_in_webkit_browser)"
        scen = {"binary(/_demo$/)", "binary(/e2e/)"}
        filt = "%s + rdeps(meclaw-colony) - (binary(/_demo$/) + binary(/e2e/) + %s)" % (
            gate_plan.T0_FLOOR, unit)
        terms = [gate_plan.T0_FLOOR, "rdeps(meclaw-colony)"]
        floor = {"kind(lib)", "kind(bin)"}
        # the case itself: no station runs the unit, nothing exempts it
        self.assertIn(unit, narrow.unplanned_run(terms, filt))
        self.assertEqual((floor | {"rdeps(meclaw-colony)"}, scen | {unit}), narrow.summands(filt))
        self.assertEqual([unit], narrow.unplanned_run(terms, filt, exempt=scen))
        # L-1: browser:display carries nothing runnable for it
        self.assertEqual([unit], narrow.unplanned_run(terms, filt, elsewhere=set(), exempt=scen))
        # the station runs it: planned
        self.assertEqual([], narrow.unplanned_run(terms, filt, elsewhere={unit}, exempt=scen))
        # reach: a subtraction no planned summand reaches takes nothing away
        self.assertEqual([], narrow.unplanned_run(terms, filt, exempt=scen,
                                                  reaches=lambda s, t: False))
        self.assertEqual([unit], narrow.unplanned_run(
            terms, filt, exempt=scen, reaches=lambda s, t: s == "rdeps(meclaw-colony)"))
        # a second subtraction and a planned term the filter itself subtracts
        twice = "%s - (%s) - (binary_id(=beta::b1))" % (gate_plan.T0_FLOOR, unit)
        self.assertEqual((floor, {unit, "binary_id(=beta::b1)"}), narrow.summands(twice))
        self.assertEqual(["binary_id(=beta::b1)"], narrow.unplanned_run(
            [gate_plan.T0_FLOOR, "binary_id(=beta::b1)"], twice, elsewhere={unit}))
        # the policy subtraction stays exempt in the narrowing's own call
        self.assertEqual(scen, narrow.summands(" + ".join(sorted(scen)))[0])

    def test_gh1182_l1_a_leftover_orphan_is_named_from_the_rows(self):
        """L-1 of re-check tb-s3 nb in its row form: after the orphans are
        planned into `browser:display`, the guard reads what the other rows'
        commands really run. No row, a row without a command or with no
        `test-tier.sh filter` command runs nothing -- the subtracted unit is
        named; a filter that carries it (union and webkit tail) runs it."""
        unit = "binary_id(=beta::b_webkit_browser)"
        tests = {"name": "tests", "select": {"filter": "rdeps(alpha) - (binary(/e2e/) + %s)"
                                                       % unit}}
        def part(op):
            return ["scripts/test-tier.sh", "filter",
                    "(binary_id(=beta::b1) + %s) %s test(/webkit/)" % (unit, op), "--run-ignored",
                    "all"]
        cases = [([], [unit]),
                 ([{"name": "browser:display"}], [unit]),
                 ([{"name": "browser:display", "cmd": ["scripts/run-browser.sh", unit]}], [unit]),
                 # L-a of re-check tb-s1e h1: one engine part alone runs only its
                 # part of the union -- it counted as the whole union (fail-open)
                 ([{"name": "browser:display", "cmds": [part("&")]}], [unit]),
                 ([{"name": "browser:display", "cmds": [part("-")]}], [unit]),
                 ([{"name": "browser:display", "cmds": [part("-"), part("&")]}], []),
                 ([{"name": "browser:display", "cmds": [part("&")]},
                   {"name": "other", "cmds": [part("-")]}], []),
                 ([{"name": "browser:display",
                    "cmd": ["scripts/test-tier.sh", "filter", "binary_id(=beta::b1) + %s" % unit,
                            "--run-ignored", "all"]}], [])]
        with mock.patch.object(impact.gate_plan, "scenario_for", return_value="binary(/e2e/)"):
            for others, want in cases:
                with self.subTest(others=others):
                    self.assertEqual(want, narrow.unrun_subtractions([tests] + others, self.root))
            self.assertEqual([], narrow.unrun_subtractions([tests], self.root,
                                                           reaches=lambda s, t: False))
            self.assertEqual([], narrow.unrun_subtractions([{"name": "clippy"}], self.root))

    def test_gh1182_a_folded_unit_the_station_runs_is_run(self):
        """A folded browser unit is `(binary_id(=<c>::<harness>) & test(/^<stem>::/))`,
        one summand of the station's union. Opened up like a union it read as the
        harness cut by a test term, so the guard named a unit the station runs
        (real tree, `full_suite`: misc_6 gh703_the_chat_shows_its_newest_line and
        gh704_a_refused_channel_does_not_take_the_dock)."""
        unit = "(binary_id(=beta::misc_6) & test(/^b_webkit_browser::/))"
        tests = {"name": "tests", "select": {"filter": "rdeps(alpha) - (binary(/e2e/) + %s)"
                                                       % unit}}
        cmds = [["scripts/test-tier.sh", "filter",
                 "(binary_id(=beta::b1) + %s) %s test(/webkit/)" % (unit, op), "--run-ignored", "all"]
                for op in ("-", "&")]
        with mock.patch.object(impact.gate_plan, "scenario_for", return_value="binary(/e2e/)"):
            self.assertEqual([], narrow.unrun_subtractions(
                [tests, {"name": "browser:display", "cmds": cmds}], self.root))
            self.assertEqual([], narrow.unrun_subtractions(
                [tests, {"name": "browser:display",
                         "cmd": ["scripts/test-tier.sh", "filter", unit]}], self.root))
            self.assertEqual([unit], narrow.unrun_subtractions(
                [tests, {"name": "browser:display", "cmds": cmds[1:]}], self.root))

    def test_gh1182_the_narrowing_exempts_its_scenario_subtraction(self):
        """The narrowing's own call passes the scenario class `render_filter`
        subtracts as policy (GH #1182): without it every narrowed gate of a
        tree with a scenario binary fell back to the resolver floor."""
        scen = "binary(/e2e/) + binary(/_demo$/)"
        # rule 11 (GH #1161) renders the policy through `tests_minus_for`
        names = [n for n in ("scenario_for", "tests_minus_for") if hasattr(impact.gate_plan, n)]
        with contextlib.ExitStack() as stack:
            for n in names:
                stack.enter_context(mock.patch.object(impact.gate_plan, n, return_value=scen))
            sel, filt, notes = self.executed()
        self.assertTrue(filt.endswith(" - (%s)" % scen), (filt, notes))
        self.assert_plan_is_run(sel, filt, notes)

    def test_b2_plan_equals_execution_in_every_case(self):
        """The generic guard: whatever the narrowing plans, the executed filter
        holds. Red under the mutation "a term left out of the filter"."""
        cases = [({}, CHANGED),
                 ({"crates/alpha/src/net.rs": "pub fn net2() {}\n"}, CHANGED),
                 ({"crates/alpha/tests/a2_template.rs": "#[test]\nfn changed() {}\n"}, CHANGED),
                 ({"crates/beta/schemas/body.json": "{\"type\": \"object\"}\n"}, CHANGED),
                 ({".config/nextest.toml": "[profile.default]\nslow-timeout = \"60s\"\n"},
                  CHANGED),
                 ({"Cargo.lock": "# changed\n"}, CHANGED),
                 ({"crates/alpha/src/net.rs": "pub fn net3() {}\n",
                   "crates/alpha/tests/a2_template.rs": "#[test]\nfn again() {}\n"},
                  ["crates/alpha/src/store.rs", "crates/beta/src/lib.rs"])]
        for files, paths in cases:
            with self.subTest(files=sorted(files), paths=paths):
                if files:
                    self.commit(files)
                try:
                    sel, filt, notes = self.executed(paths)
                    self.assert_plan_is_run(sel, filt, notes)
                finally:
                    self.reset_to_map()

    def test_b2_a_fresh_map_runs_a_changed_test_and_rdeps_of_another_crate(self):
        """Fix-Runde 1b (Nachtrag Post-xhigh-4): with a fresh map, a test
        changed since the map (`binary_id`, from `map_fits`) and a source of
        another crate changed since the map (`rdeps`, from `since_plan`) stood
        in the plan fields but not in the filter -- impact() replaced
        rdeps(alpha) by `cidx[net.rs]`, and the filter was impact's. Both
        must be summands of what nextest gets."""
        self.commit({"crates/beta/tests/b2_ignored.rs": "#[test]\n#[ignore]\nfn slower() {}\n",
                     "crates/alpha/src/net.rs": "pub fn net4() {}\n"})
        sel, filt, notes = self.executed()
        got = filter_terms(filt)
        self.assertIn("binary_id(=beta::b2_ignored)", got, (filt, notes))
        self.assertIn("rdeps(alpha)", got, (filt, notes))
        self.assert_plan_is_run(sel, filt, notes)

    def lane_metadata(self, keep_root=True):
        """The metadata cache as a lane writes it: every path under the
        lane's tree, not this repository (the cache of 10.10. names
        /srv/target/tree in all seven files)."""
        meta = json.loads(json.dumps(self.meta).replace(self.root, "/srv/target/tree"))
        if keep_root:
            meta["workspace_root"] = "/srv/target/tree"
        return meta

    def test_a_test_since_the_map_is_found_with_lane_metadata(self):
        """Fix-Runde 1b, echte Daten: `_test_target_of` read `src_path`
        against the repository, the lane metadata names /srv/target/tree --
        a test changed since the map planned nothing (fail-open)."""
        self.commit({"crates/alpha/tests/a2_template.rs": "#[test]\nfn lane() {}\n"})
        self.meta = self.lane_metadata()
        sel, filt, notes = self.executed()
        self.assertIn("binary_id(=alpha::a2_template)", filter_terms(filt), (filt, notes))
        self.assert_plan_is_run(sel, filt, notes)

    def test_a_test_since_the_map_without_a_known_target_drops_the_map(self):
        """Fail-closed: a test file since the map that no target and no
        member of the metadata holds cannot be planned -- the map does not
        fit (floor + graph, rdeps kept), it never plans nothing."""
        self.commit({"crates/alpha/tests/a2_template.rs": "#[test]\nfn lost() {}\n"})
        self.meta = self.lane_metadata(keep_root=False)
        sel, notes = self.run_narrow()
        self.assertIn("alpha", sel["rdeps"], notes)
        self.assertTrue([n for n in notes if "no target of the metadata" in n], notes)

    def test_a_tests_dir_outside_the_crates_keeps_the_map(self):
        """The fail-closed rule is for a crate's tests only: scripts/tests
        (python, changed in nearly every strand) is no workspace member, and
        dropping the map for it would end the narrowing."""
        self.commit({"scripts/tests/test_probe.py": "pass\n"})
        self.meta = self.lane_metadata()
        sel, notes = self.run_narrow()
        self.assertFalse([n for n in notes if "no target of the metadata" in n], notes)
        self.assertNotIn("alpha", sel["rdeps"], notes)

    def reset_to_map(self):
        git(self.root, "reset", "-q", "--hard", self.map_rev)
        gate_plan._SOURCE_CACHE.clear()

    def test_m4_a_rust_source_since_the_map_keeps_rdeps_of_its_crate(self):
        """OR-TB.S1b.16: the map knows net.rs as it was at its rev. Its cover is
        no longer the truth for the file, so rdeps(alpha) stays -- the map
        replaces nothing of the crate (`compile` stays without alpha)."""
        self.commit({"crates/alpha/src/net.rs": "pub fn net2() {}\n"})
        sel, notes = self.run_narrow()
        self.assertIn("alpha", sel["rdeps"], notes)
        self.assertNotIn("alpha", sel.get("compile") or [], notes)
        self.assertIn("narrowing: crates/alpha/src/net.rs changed since coverage map %s -- "
                      "the map knows the old file, rdeps(alpha) kept" % self.map_rev[:9], notes)

    def test_drift_alone_keeping_rdeps_says_so_first_with_map_age_and_count(self):
        """Kit finding b (welle-val2 v2-flake-b01): a map older than a Rust change on the
        base keeps rdeps for a file the diff does not hold. That made a near-full suite
        (239 of 247 binaries) and said it only in one NOTE per drifted file. The first
        NOTE names the map's rev, its age, the drifted files and the rdeps kept, and the
        binary count; without drift rdeps there is no such NOTE."""
        sel, notes = self.run_narrow()
        self.assertFalse([n for n in notes if "predates" in n], notes)
        self.commit({"crates/alpha/src/net.rs": "pub fn net2() {}\n"})
        sel, notes = self.run_narrow()
        self.assertIn("alpha", sel["rdeps"], notes)
        age = (NOW - datetime.datetime.fromisoformat(
            self.cov["built_at"].replace("Z", "+00:00"))).total_seconds() / 3600.0
        self.assertRegex(notes[0], r"^narrowing: coverage map %s is %.1f h old and predates "
                         r"1 file\(s\) changed on the base since \(not in this diff\) -- "
                         r"rdeps\(alpha\) kept for that drift, \d+ of \d+ binaries; a fresh "
                         r"map \(`scripts/pipeline.py coverage build`\) narrows them$"
                         % (self.map_rev[:9], age))

    def test_drift_note_blames_only_rdeps_the_diff_does_not_keep_itself(self):
        """Re-check R2 M2 (welle-val2 v2-flake-b01): rdeps(alpha) is kept by the drift
        (net.rs changed on the base since the map) AND by the diff's own file (store.rs,
        not in the map). A fresh map would not narrow it, so the NOTE must not say the
        drift keeps it -- no drift NOTE at all when the diff keeps every drift rdeps."""
        cov = copy.deepcopy(self.cov)
        cov["parts"] = [p for p in cov["parts"] if "crates/alpha/src/store.rs" not in p["files"]]
        self.commit({"crates/alpha/src/net.rs": "pub fn net2() {}\n"})
        sel, notes = self.run_narrow(cov=cov)
        self.assertIn("alpha", sel["rdeps"], notes)
        self.assertIn("narrowing: crates/alpha/src/store.rs not in the coverage map -- "
                      "rdeps(alpha) kept", notes)
        self.assertFalse([n for n in notes if "kept for that drift" in n], notes)

    def test_m4_a_new_caller_since_the_map_plans_its_test(self):
        """Adversarial (OR-TB.S1b.16): since the map, net.rs registers a new
        handler that calls into store.rs, and the template test a2_template
        reaches it through that registration only. The map relates a2_template
        to neither file -- cidx[store.rs] and cidx[net.rs] both miss it -- and
        the strand changes store.rs: a2_template MUST run."""
        self.assertNotIn("alpha::a2_template", {p["binary"] for p in self.cov["parts"]})
        self.commit({"crates/alpha/src/net.rs": ("use crate::store::Db;\n\n"
                                                "pub fn handler() -> Db {\n    Db\n}\n")})
        sel, notes = self.run_narrow()
        self.assertIn("alpha::a2_template", self.ran(sel), notes)
        self.assertIn("beta::b1_uses_alpha", self.ran(sel), notes)   # rdeps(alpha) reaches beta

    def test_m4_a_crate_file_since_the_map_keeps_rdeps_of_its_crate(self):
        """Rule 10 for a file changed since the map: an `include_str!` schema
        the map cannot see keeps rdeps(beta) -- never replaced."""
        self.commit({"crates/beta/schemas/body.json": "{\"type\": \"object\"}\n"})
        sel, notes = self.run_narrow()
        self.assertIn("beta", sel["rdeps"], notes)
        self.assertFalse(sel["all"], notes)

    def test_m4_nextest_toml_since_the_map_plans_no_floor(self):
        """OR-TB.S1b.17: `.config/nextest.toml` (profiles, timeouts, groups,
        retries) shifts no cover: since the map it plans nothing -- the
        narrowed plan stays, with a NOTE. The whole-suite case of 7d75ec7a3."""
        self.commit({".config/nextest.toml": "[profile.default]\nslow-timeout = \"60s\"\n"})
        sel, notes = self.run_narrow()
        self.assertFalse(sel["all"], notes)
        self.assertNotIn("alpha", sel["rdeps"], notes)
        self.assertIn("beta::b1_uses_alpha", sel["binaries"])
        self.assertTrue(any(n.startswith("narrowing: .config/nextest.toml changed since coverage "
                                         "map %s -- shifts no cover" % self.map_rev[:9])
                            for n in notes), notes)

    def test_m4_deny_toml_since_the_map_plans_no_floor(self):
        """OR-TB.S1b.17: `deny.toml` is read by `cargo deny` alone."""
        self.commit({"deny.toml": "[licenses]\nallow = [\"MIT\"]\n"})
        sel, notes = self.run_narrow()
        self.assertFalse(sel["all"], notes)
        self.assertNotIn("alpha", sel["rdeps"], notes)

    def test_m4_a_nextest_toml_that_selects_or_wraps_keeps_all(self):
        """Fail-closed guard of the NO_SHIFT entry: a default filter, extra
        test-binary arguments, a wrapper or a setup script DO change what runs
        -- such a nextest.toml since the map is rule 1 again, all()."""
        for body in ('[profile.default]\ndefault-filter = "not test(slow)"\n',
                     '[[profile.default.overrides]]\nfilter = "all()"\nrun-extra-args = ["--ignored"]\n',
                     '[scripts.wrapper.w]\ncommand = "env X=1"\n',
                     '[[profile.default.scripts]]\nfilter = "all()"\nsetup = "db"\n',
                     # M-5: quoted, dotted and inline forms (valid TOML, `tomllib`)
                     '[profile.default]\n"default-filter" = "not test(slow)"\n',
                     "[profile.default]\n'default-filter' = 'not test(slow)'\n",
                     'profile.default.default-filter = "not test(slow)"\n',
                     '[profile]\ndefault = { default-filter = "not test(slow)" }\n',
                     '[profile.default]\nscripts = [{ filter = "all()", setup = "db" }]\n',
                     '[profile.default]\nscripts = [{ filter = "all()", run-wrapper = "w" }]\n',
                     '[profile.default]\n'
                     'overrides = [{ filter = "all()", run-extra-args = ["-q"] }]\n',
                     # M-5: unreadable as TOML -- fail-closed
                     '[profile.default\nslow-timeout = = "60s"\n'):
            with self.subTest(body=body):
                self.commit({".config/nextest.toml": body})
                sel, notes = self.run_narrow()
                self.assertTrue(sel["all"], (body, notes))
                self.reset_to_map()

    def test_m5_a_key_removed_since_the_map_keeps_all(self):
        """M-5: the guard reads the map's state too. A default filter or a setup
        script the map was built WITH and HEAD no longer has: the tests it kept
        out of the map run now, the narrowing never picks them -- all()."""
        for body in ('[profile.default]\ndefault-filter = "not binary_id(=alpha::a2_template)"\n',
                     '[scripts.setup.db]\ncommand = "true"\n'
                     '[[profile.default.scripts]]\nfilter = "all()"\nsetup = "db"\n'):
            with self.subTest(body=body):
                self.commit({".config/nextest.toml": body})
                old_rev, old_cov = self.map_rev, self.cov
                self.map_rev = git(self.root, "rev-parse", "HEAD")
                self.cov = dict(COVERAGE, rev=self.map_rev)
                try:
                    self.commit({".config/nextest.toml":
                                 "[profile.default]\nslow-timeout = \"60s\"\n"})
                    sel, notes = self.run_narrow()
                    self.assertTrue(sel["all"], (body, notes))
                finally:
                    self.map_rev, self.cov = old_rev, old_cov
                    self.reset_to_map()

    def test_m4_an_unknown_workspace_file_since_the_map_keeps_all(self):
        """Unknown class (not on NO_SHIFT): Cargo.lock, a toolchain or
        `.cargo/config.toml` since the map keep rule 1 -- all(). Behaviour as
        before M-4; red under the mutations `NO_SHIFT` + Cargo.lock and
        `workspace` without floor."""
        for rel in ("Cargo.lock", "rust-toolchain.toml", ".cargo/config.toml"):
            with self.subTest(rel=rel):
                self.commit({rel: "# changed after the map: %s\n" % rel})
                sel, notes = self.run_narrow()
                self.assertTrue(sel["all"], (rel, notes))
                self.reset_to_map()

    def test_m4_nextest_toml_in_the_diff_itself_keeps_all(self):
        """M-4 changes the files changed since the map only: a nextest.toml
        the strand itself changes keeps its floor -- rule 1, all()."""
        self.commit({".config/nextest.toml": "[profile.default]\nslow-timeout = \"60s\"\n"})
        paths = CHANGED + [".config/nextest.toml"]
        sel, notes = self.run_narrow(paths=paths)
        self.assertTrue(sel["all"], notes)

    def test_m4_the_no_shift_list_is_explicit(self):
        """No pattern in NO_SHIFT: every entry is one path with a reason."""
        self.assertEqual({".config/nextest.toml", "deny.toml"}, set(narrow.NO_SHIFT))
        for rel, why in narrow.NO_SHIFT.items():
            self.assertNotRegex(rel, r"[*?\[]")
            self.assertTrue(why.strip(), rel)

    # --- V2-A3 (K9): the merge point's version bump keeps the map -----------

    BUMP_BASE = {
        "Cargo.toml": '[workspace]\nmembers = ["crates/alpha", "crates/beta"]\n\n'
                      '[workspace.package]\nversion = "0.63.5"\n',
        "crates/beta/Cargo.toml": '[package]\nname = "beta"\nversion.workspace = true\n\n'
                                  '[dependencies]\n'
                                  'meclaw-alpha = { path = "../meclaw-alpha", version = "0.63.1" }\n\n'
                                  '[dev-dependencies]\nalpha = { path = "../alpha" }\n',
        "Cargo.lock": '# This file is automatically @generated by Cargo.\nversion = 4\n\n'
                      '[[package]]\nname = "alpha"\nversion = "0.63.5"\n\n'
                      '[[package]]\nname = "beta"\nversion = "0.63.5"\n'
                      'dependencies = [\n "alpha",\n "serde",\n]\n\n'
                      '[[package]]\nname = "serde"\nversion = "1.0.200"\n'
                      'source = "registry+https://github.com/rust-lang/crates.io-index"\n',
    }

    def bump_base(self):
        """The map's rev holds a workspace version, a lock file and an
        inter-crate pin -- the files the merge point's bump rewrites."""
        self.commit(self.BUMP_BASE)
        self.map_rev = git(self.root, "rev-parse", "HEAD")
        self.cov = dict(COVERAGE, rev=self.map_rev)

    def test_v2a3_a_version_bump_since_the_map_keeps_the_map(self):
        """K9 (0.64.0): the bump changed `crates/*/Cargo.toml` (pins), the root
        version and the lock's member versions since the map -- and the strand
        gate planned rdeps of every crate. Version lines change no code: the
        narrowed plan stays, with a NOTE per file."""
        self.bump_base()
        old, raised = versions.set_workspace_version(self.root, "0.64.0")
        self.assertEqual((old, [r["path"] for r in raised]), ("0.63.5", ["crates/beta/Cargo.toml"]))
        self.commit()
        sel, notes = self.run_narrow()
        self.assertFalse(sel["all"], notes)
        self.assertNotIn("alpha", sel["rdeps"], notes)
        self.assertNotIn("beta", sel["rdeps"], notes)
        self.assertIn("beta::b1_uses_alpha", sel["binaries"], notes)
        for rel in ("Cargo.lock", "Cargo.toml", "crates/beta/Cargo.toml"):
            self.assertIn("narrowing: %s changed since coverage map %s -- version lines only "
                          "(the merge point's bump), the map stays valid" % (rel, self.map_rev[:9]),
                          notes)

    def test_v2a3_any_other_cargo_change_since_the_map_keeps_rdeps(self):
        """Fail-closed: a dependency, a feature, a path or a registry version
        next to the bump is a code change -- rdeps (crate file) or all()
        (workspace file) as before."""
        beta = self.BUMP_BASE["crates/beta/Cargo.toml"]
        cases = (
            ("crates/beta/Cargo.toml", beta + "serde = \"1\"\n", "beta"),
            ("crates/beta/Cargo.toml", beta.replace('path = "../meclaw-alpha"',
                                                    'path = "../meclaw-gamma"'), "beta"),
            ("crates/beta/Cargo.toml", beta + "\n[features]\nextra = []\n", "beta"),
            ("crates/beta/Cargo.toml", beta.replace('alpha = { path = "../alpha" }',
                                                    'alpha = { path = "../alpha", version = "9" }'),
             "beta"),
            ("crates/alpha/Cargo.toml", GRAPH_FILES["crates/alpha/Cargo.toml"].replace(
                'serde = "1"', 'serde = "1.0.210"'), "alpha"),
            ("Cargo.toml", self.BUMP_BASE["Cargo.toml"] + "\n[workspace.dependencies]\n"
                                                         "serde = \"1\"\n", "all()"),
            ("Cargo.lock", self.BUMP_BASE["Cargo.lock"].replace('"1.0.200"', '"1.0.210"'),
             "all()"),
        )
        start = self.map_rev
        for rel, text, want in cases:
            with self.subTest(rel=rel, text=text):
                try:
                    self.bump_base()
                    versions.set_workspace_version(self.root, "0.64.0")
                    self.commit({rel: text})
                    sel, notes = self.run_narrow()
                    if want == "all()":
                        self.assertTrue(sel["all"], notes)
                    else:
                        self.assertFalse(sel["all"], notes)
                        self.assertIn(want, sel["rdeps"], notes)
                        self.assertIn("narrowing: %s changed since coverage map %s -- the map "
                                      "knows the old file, rdeps(%s) kept"
                                      % (rel, self.map_rev[:9], want), notes)
                finally:
                    self.map_rev = start
                    self.reset_to_map()

    # --- review Z-I gap I-A: a new unit test in src/** is planned ------------

    def test_a_source_without_unit_cover_plans_its_own_module(self):
        """`doc.rs` is covered only by an integration binary: a new
        `#[test]` there runs in the unit binary `alpha`, module `doc`."""
        cov = copy.deepcopy(self.cov)
        cov["parts"].append({"binary": "alpha::a1_reads_doc", "module": None,
                             "files": {"crates/alpha/src/doc.rs": 2}})
        paths = ["crates/alpha/src/doc.rs"]
        sel, notes = self.run_narrow(cov=cov, paths=paths)
        self.assertNotIn("alpha", sel["rdeps"], notes)
        self.assertIn("alpha", sel["binaries"], notes)
        self.assertIn("(binary_id(=alpha) & test(/^doc::/))", sel["filter"])

    def test_a_covered_source_plans_its_own_module_too(self):
        """`store.rs` is covered by the unit module `net` only: a new test
        in `store.rs` lives under `store::` and is planned as well."""
        sel, notes = self.run_narrow()
        self.assertIn("(binary_id(=alpha) & test(/^store::/))", sel["filter"], notes)
        self.assertIn("(binary_id(=alpha) & test(/^net::/))", sel["filter"], notes)

    def test_a_lib_root_plans_the_whole_unit_binary(self):
        sel, notes = self.run_narrow(paths=["crates/alpha/src/lib.rs"])
        self.assertRegex(sel["filter"], r"binary_id\(=alpha\)(?! &)", notes)   # whole, no module
        self.assertNotIn("alpha", sel["rdeps"], notes)

    def test_a_covered_bin_source_keeps_rdeps_with_a_note(self):
        cov = copy.deepcopy(self.cov)
        cov["parts"].append({"binary": "alpha::a1_reads_doc", "module": None,
                             "files": {"crates/alpha/src/bin/tool.rs": 1}})
        sel, notes = self.run_narrow(cov=cov, paths=["crates/alpha/src/bin/tool.rs"])
        self.assertIn("alpha", sel["rdeps"], notes)
        self.assertTrue(any("is no lib module" in n for n in notes), notes)

    def test_the_impact_notes_are_passed_on(self):
        _sel, notes = self.run_narrow()
        self.assertTrue(any(n.startswith("narrowing: impact: rdeps(alpha) replaced by coverage")
                            for n in notes), notes)

    def test_the_tests_command_carries_the_narrowed_filter(self):
        # Review Z-I1 I1: `cmd` kept the resolver's rdeps filter; a cache hit
        # ran the narrowed one -- the plan said two things.
        floor = self.floor()
        rows = [{"name": "tests", "scope": floor["filter"],
                 "cmd": ["scripts/test-tier.sh", "filter", floor["filter"]], "select": floor}]
        with mock.patch.object(narrow, "newest_map", return_value=(self.cov, None)), \
                mock.patch("pipeline.graph.load", return_value=self.doc):
            narrow.apply(rows, CHANGED, self.root, env={narrow.ENV: "1"}, now=NOW,
                         metadata=self.meta)
        sel = rows[0]["select"]
        self.assertNotIn("rdeps(alpha)", sel["filter"])
        self.assertEqual(["scripts/test-tier.sh", "filter", sel["filter"]], rows[0]["cmd"])
        self.assertEqual(sel["filter"], rows[0]["scope"])

    def test_the_replaced_rdeps_are_still_compiled(self):
        # Review Z-I1 I2: execution coverage is no compile dependency -- the
        # test binaries of rdeps(alpha) are built (no run), only fewer run.
        sel, _notes = self.run_narrow()
        self.assertEqual(["alpha"], sel.get("compile"))
        parts, build = plan.select_parts(sel, self.meta)
        self.assertEqual(["alpha", "beta"], build["packages"])
        self.assertNotIn("beta::b2_ignored", parts)

    def test_a_map_built_in_the_future_falls_back(self):
        sel, notes = self.run_narrow(cov=dict(self.cov, built_at="2026-10-09T12:00:00Z"))
        self.assert_fallback(sel, notes, "built in the future")

    def test_a_map_without_built_at_falls_back(self):
        cov = dict(self.cov)
        del cov["built_at"]
        sel, notes = self.run_narrow(cov=cov)
        self.assert_fallback(sel, notes, "no valid built_at")

    def test_an_unknown_stem_keeps_rdeps_of_its_crate_with_a_note(self):
        cov = copy.deepcopy(self.cov)
        cov["parts"] = [p for p in cov["parts"] if "crates/alpha/src/store.rs" not in p["files"]]
        sel, notes = self.run_narrow(cov=cov)
        self.assertIn("alpha", sel["rdeps"])
        self.assertIn("narrowing: crates/alpha/src/store.rs not in the coverage map -- "
                      "rdeps(alpha) kept", notes)

    def test_no_graph_keeps_the_resolver_floor(self):
        sel, notes = self.run_narrow(graph=False, why="no latest.json")
        self.assertEqual(self.floor(), sel)
        self.assertEqual(["narrowing: no graph (no latest.json) -- resolver floor"], notes)

    def test_the_switch_off_keeps_the_resolver_floor(self):
        rows = [{"name": "tests", "select": self.floor()}]
        before = copy.deepcopy(rows)
        notes = narrow.apply(rows, CHANGED, self.root, env={narrow.ENV: "0"}, metadata=self.meta)
        self.assertEqual(before, rows)
        self.assertEqual(["narrowing: off (MECLAW_GATE_NARROW=0) -- resolver floor"],
                         notes)

    def test_the_switch_is_on_by_default(self):
        # #1126/#1138: map f004e6c36 fits the tree (227/227) -- on by default;
        # `map_fits` stays the fail-closed guard, `=0` turns it off.
        self.assertTrue(narrow.enabled({}))
        self.assertTrue(narrow.enabled({narrow.ENV: ""}))
        self.assertTrue(narrow.enabled({narrow.ENV: "1"}))

    def test_zero_turns_the_switch_off(self):
        self.assertFalse(narrow.enabled({narrow.ENV: "0"}))
        self.assertFalse(narrow.enabled({narrow.ENV: " 0 "}))

    def test_apply_with_an_unset_switch_narrows(self):
        floor = self.floor()
        rows = [{"name": "tests", "select": floor}]
        with mock.patch.object(narrow, "newest_map", return_value=(self.cov, None)), \
                mock.patch("pipeline.graph.load", return_value=self.doc):
            notes = narrow.apply(rows, CHANGED, self.root, env={}, now=NOW, metadata=self.meta)
        self.assertNotIn("rdeps(alpha)", rows[0]["select"]["filter"])
        self.assertFalse(any(n.startswith("narrowing: off") for n in notes), notes)

    def test_a_graph_of_other_resolver_code_is_a_note(self):
        # Re-check tb-s2 Zusatz-Check m-3 (OR-TB.S2.11): after a resolver change
        # the old graph narrowed on silently; nothing rebuilds it by itself.
        for code, said in ((None, True), ("0" * 16, True), (graph_mod.code_hash(), False)):
            doc = dict(self.doc)
            doc.pop("code", None)
            if code is not None:
                doc["code"] = code
            rows = [{"name": "tests", "select": self.floor()}]
            with mock.patch.object(narrow, "newest_map", return_value=(self.cov, None)), \
                    mock.patch("pipeline.graph.load", return_value=doc):
                notes = narrow.apply(rows, CHANGED, self.root, env={}, now=NOW,
                                     metadata=self.meta)
            self.assertNotIn("rdeps(alpha)", rows[0]["select"]["filter"])
            self.assertEqual(said, any("graph built by other resolver code" in n
                                       for n in notes), (code, notes))

    def test_newest_map_picks_the_latest_build_and_says_why_without(self):
        with tempfile.TemporaryDirectory() as d:
            cov, why = narrow.newest_map(d)
            self.assertIsNone(cov)
            self.assertIn("no coverage map in", why)
            for rev, at in (("old", "2026-10-01T00:00:00Z"), ("new", "2026-10-08T00:00:00Z")):
                with open(os.path.join(d, "coverage-%s.json" % rev), "w", encoding="utf-8") as fh:
                    json.dump(dict(COVERAGE, rev=rev, built_at=at), fh)
            self.assertEqual("new", narrow.newest_map(d)[0]["rev"])

    def test_newest_map_skips_a_broken_file_and_says_so_without_a_good_one(self):
        # Review Z-I1 M3/M4: a broken map file was untested.
        with tempfile.TemporaryDirectory() as d:
            with open(os.path.join(d, "coverage-broken.json"), "w", encoding="utf-8") as fh:
                fh.write("{not json")
            cov, why = narrow.newest_map(d)
            self.assertIsNone(cov)
            self.assertIn("coverage-broken.json", why)


# --- #1160 part A: a diff without Rust source never grows by the narrowing ----
# py-only measured 79 binaries narrowed against 3 unnarrowed (V8). The growth
# came from `map_fits`: every source and test changed since the map rev brings
# its cover/binary -- useful only when the map REPLACES an rdeps(X). A Python
# or docs diff has no rdeps to replace, so the drift terms were pure addition.
TOOL_FILES = {
    "scripts/tool.py": "# a kit tool\n",
    "crates/beta/tests/b3_runs_tool.rs":
        '#[test]\nfn tool() {\n    let _ = std::process::Command::new("python3")'
        '.arg("scripts/tool.py");\n}\n',
}


class NonRustDiffTest(Base):
    files = dict(FILES, **TOOL_FILES)
    floor, run_narrow, commit = NarrowTest.floor, NarrowTest.run_narrow, NarrowTest.commit

    def setUp(self):
        super().setUp()
        git(self.root, "init", "-q", "-b", "master")
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", "map rev")
        self.cov = dict(COVERAGE, rev=git(self.root, "rev-parse", "HEAD"))
        self.meta = metadata(self.root)
        beta = next(p for p in self.meta["packages"] if p["name"] == "beta")
        beta["targets"].append({"kind": ["test"], "name": "b3_runs_tool",
                                "src_path": os.path.join(self.root,
                                                         "crates/beta/tests/b3_runs_tool.rs")})
        # drift since the map: a source with cover and a changed test binary
        self.commit({"crates/alpha/src/net.rs": "pub fn net2() {}\n",
                     "crates/alpha/tests/a2_template.rs":
                         '#[test]\nfn t() {\n    let _ = "templates/talky/config.json";\n}\n'
                         '#[test]\nfn changed() {}\n'})

    def bins(self, sel):
        return set((sel or {}).get("binaries") or [])

    def unnarrowed(self, paths):
        return self.bins(self.floor(paths))

    def test_python_only_diff_never_plans_more_binaries_than_unnarrowed_without_reader_term(self):
        for paths in (["scripts/other.py"], ["scripts/tool.py"]):
            sel, notes = self.run_narrow(paths=paths)
            grown = self.bins(sel) - self.unnarrowed(paths)
            readers = {b for b in grown
                       if any(("file:%s" % p) in w for p in paths for w in sel["why"])}
            self.assertEqual(set(), grown - readers, (paths, sel, notes))
            self.assertNotIn("alpha::a1_reads_doc", self.bins(sel), notes)   # cover of net.rs
            self.assertNotIn("alpha::a2_template", self.bins(sel), notes)    # changed test
            self.assertEqual([], (sel or {}).get("rdeps") or [])

    def test_python_reader_edge_keeps_its_binary_with_reason(self):
        sel, notes = self.run_narrow(paths=["scripts/tool.py"])
        self.assertIn("beta::b3_runs_tool", sel["binaries"], notes)
        self.assertIn("binary_id(=beta::b3_runs_tool)", sel["filter"])
        self.assertIn("beta::b3_runs_tool", self.unnarrowed(["scripts/tool.py"]))

    def test_docs_only_diff_plans_floor_only(self):
        paths = ["docs/guide.md"]
        floor = self.floor(paths)
        sel, notes = self.run_narrow(paths=paths)
        self.assertEqual(set(floor["binaries"]), set(sel["binaries"]), notes)
        self.assertIn("alpha::a1_reads_doc", sel["binaries"])    # the page's reader stays
        self.assertNotIn("alpha::a2_template", sel["binaries"], notes)
        self.assertTrue(any("no Rust source in the diff" in n for n in notes), notes)

    def test_a_file_of_unknown_class_keeps_todays_plan(self):
        """Fail-closed: a path outside the known non-Rust classes plans as before."""
        sel, notes = self.run_narrow(paths=["templates/talky/config.json"])
        self.assertIsNotNone(sel)
        self.assertIn("alpha::a1_reads_doc", sel["binaries"], notes)   # cover of net.rs
        self.assertFalse(any("no Rust source in the diff" in n for n in notes), notes)

    def test_a_rust_diff_still_brings_the_drift_terms(self):
        sel, notes = self.run_narrow()
        self.assertIn("alpha::a2_template", sel["binaries"], notes)
        self.assertIn("alpha::a1_reads_doc", sel["binaries"], notes)

    def test_any_file_of_the_kit_plans_no_drift_terms(self):
        """Kit finding a (welle-val2 v2-flake-b01): `scripts/pipeline/selftest_durations.json`
        in a scripts-only diff missed `NON_RUST`, which knew `.py`/`.sh` under scripts/
        only; the plan took the map and its drift -- 239 of 247 binaries, 10800 tests. A
        file under scripts/ carries no Rust class of the resolver, whatever its extension:
        a data file, a toml, a page. Each beside `scripts/tool.py`, whose reader edge
        gives the diff a floor to narrow (a floor of nothing returns before the map)."""
        for other in ("scripts/pipeline/selftest_durations.json", "scripts/lanes.toml",
                      "scripts/pipeline/README.md"):
            paths = ["scripts/tool.py", other]
            with self.subTest(paths=paths):
                sel, notes = self.run_narrow(paths=paths)
                self.assertTrue(any("no Rust source in the diff" in n for n in notes), notes)
                self.assertNotIn("alpha::a1_reads_doc", self.bins(sel), notes)   # cover of net.rs
                self.assertNotIn("alpha::a2_template", self.bins(sel), notes)    # changed test
                self.assertEqual([], (sel or {}).get("rdeps") or [])

    def test_non_rust_is_the_resolvers_rust_classes_read_backwards(self):
        """The class, not a list of extensions: a path is non-Rust when it lies under a
        root the narrowing accepts without the map (scripts/, docs/, plans/, a top-level
        page) and the resolver gives it none of the Rust classes (`narrow._RUST`)."""
        for rel in ("scripts/pipeline/selftest_durations.json", "scripts/lanes.toml",
                    "scripts/x/data.txt", "docs/guide.md", "docs/assets/a.svg",
                    "plans/w/berichte/s.md", "README.md"):
            with self.subTest(rel=rel):
                self.assertTrue(narrow.non_rust([rel], {}), rel)
        for rel in ("crates/alpha/src/net.rs", "crates/alpha/tests/a2_template.rs",
                    "crates/alpha/schemas/x.json", "Cargo.toml", ".config/nextest.toml",
                    "templates/talky/config.json", "workshop/tools/x.py", "Makefile"):
            with self.subTest(rel=rel):
                self.assertFalse(narrow.non_rust([rel], {}), rel)
        self.assertFalse(narrow.non_rust(["scripts/x.json"], {"rdeps": ["alpha"]}))
        self.assertFalse(narrow.non_rust([], {}))


class PlanWiringTest(Base):
    """`build_plan(narrow=True)` -- what the gate calls -- runs the narrowing in strand only."""

    def resolver(self, _repo, _mode, files):
        return {"stations": [{"name": "tests", "scope": "x",
                              "select": gate_plan.test_select(files, "strand", self.root)}]}

    def build(self, mode):
        with mock.patch.object(narrow, "apply", return_value=["narrowing: probe"]) as ap, \
                mock.patch.object(plan, "_git") as git:
            git.return_value.stdout = "rev\n"
            doc = plan.build_plan(self.root, mode, "base", files=CHANGED, resolver=self.resolver,
                                  narrow=True)
        return doc, ap

    def test_strand_asks_the_narrowing_and_carries_its_notes(self):
        doc, ap = self.build("strand")
        self.assertTrue(ap.called)
        self.assertEqual(["narrowing: probe"], doc.get("notes"))

    def test_integration_never_narrows(self):
        doc, ap = self.build("integration")
        self.assertFalse(ap.called)
        self.assertNotIn("notes", doc)


# --- merge and integration narrowed (welle-tb S1 parts B + C, GH #1159) -------
# A merge with ONE strand parent whose strand gate is GREEN on the same source
# tree plans like strand over merge base..merge; an integration plans like
# strand over the wave diff (master at the wave start..tip). Every doubt keeps
# the old plan (merge: the resolver floor, unnarrowed) or plans the full suite
# (integration, KR-TB-1).

STORE_RS = "crates/alpha/src/store.rs"


class StageBase(Base):
    def setUp(self):
        super().setUp()
        git(self.root, "init", "-q", "-b", "master")
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", "map rev")
        self.map_rev = git(self.root, "rev-parse", "HEAD")
        # A fresh map: `plan.build_plan` ages it on the wall clock when the case
        # passes no `now` (the `*_by_the_plans_clock` pins pass one), and the fixed `COVERAGE` built_at
        # 2026-10-08T02:00Z turned stale on 2026-10-15T02:00Z -- 9 StageBase
        # tests went red with "coverage map ... is 7.0 days old (> 7)"
        # (pre of v2-fix-c, fix round h). The map is built when the case starts.
        fresh = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
        self.cov = dict(COVERAGE, rev=self.map_rev, built_at=fresh)
        self.meta = metadata(self.root)
        self.tmpdb = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmpdb.cleanup)
        self.db = store.open(pathlib.Path(self.tmpdb.name) / "pipeline.db")
        self.addCleanup(self.db.close)

    def write(self, files):
        for rel, text in files.items():
            path = os.path.join(self.root, rel)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w", encoding="utf-8") as fh:
                fh.write(text)
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", "edit")
        gate_plan._SOURCE_CACHE.clear()
        return git(self.root, "rev-parse", "HEAD")

    def edit_store(self, line):
        with open(os.path.join(self.root, STORE_RS), encoding="utf-8") as fh:
            text = fh.read()
        return self.write({STORE_RS: text + line})

    def gate_run(self, rev, verdict="GREEN", mode="strand", dirty=False):
        run = "tr-%012d" % (len(self.db.execute("SELECT seq FROM journal").fetchall()) + 1)
        events.emit(self.db, "pass_planned", "status",
                    {"run": run, "mode": mode, "repo": "meclaw", "rev": rev, "base": self.map_rev,
                     "stations": ["tests"], "reason": {}, "cache_known": 0, "dirty": dirty},
                    run=run)
        events.emit(self.db, "pass_ended", "status",
                    {"run": run, "verdict": verdict, "secs": 1.0, "lane_seconds": 1.0,
                     "critical_path_s": 1.0, "rounds": 1, "cache_hits": 0, "stations": {}},
                    run=run)
        return run

    def resolver(self, repo, mode, files):
        sel = gate_plan.test_select(files, mode, repo)
        if sel is None:
            return {"stations": []}
        return {"stations": [{"name": "tests", "scope": sel["filter"],
                              "cmd": ["scripts/test-tier.sh", "filter", sel["filter"]],
                              "select": sel}]}

    def build(self, mode, base, now=None, **kw):
        with mock.patch.object(narrow, "newest_map",
                               return_value=(self.cov, getattr(self, "cov_why", None))), \
                mock.patch("pipeline.graph.load", return_value=self.doc), \
                mock.patch.dict(os.environ, {narrow.ENV: "1"}):
            doc = plan.build_plan(self.root, mode, base, now=now, resolver=self.resolver,
                                  metadata=self.meta, narrow=True, db=self.db, **kw)
        tests = next((s for s in doc["stations"] if s["name"] == "tests"), None)
        return doc, tests

    # GH #1199 B-3: every leg of `build_plan` hands its `now` to the
    # narrowing. The map is built on a fixed day; the plan's clock one day
    # later finds it fresh, 100 days later stale -- whatever the wall clock
    # says, a leg that drops `now` misses one of the two sides.
    CLOCK_MAP_DAY = datetime.datetime(2026, 1, 1, tzinfo=datetime.timezone.utc)

    def assert_aged_by_the_plans_clock(self, build, assert_narrowed):
        """`build(now)` -> (doc, tests); `assert_narrowed(doc, tests)` reads
        the leg's verdict on a fresh map."""
        self.cov = dict(self.cov, built_at="2026-01-01T00:00:00Z")
        doc, tests = build(self.CLOCK_MAP_DAY + datetime.timedelta(days=1))
        notes = doc.get("notes") or []
        self.assertFalse(any("days old" in n or "in the future" in n for n in notes), notes)
        assert_narrowed(doc, tests)
        doc, tests = build(self.CLOCK_MAP_DAY + datetime.timedelta(days=100))
        notes = doc.get("notes") or []
        self.assertTrue(any(n.startswith("narrowing: coverage map %s is 100.0 days old (> 7)"
                                          % self.cov["rev"][:9]) for n in notes), notes)

    def assert_unnarrowed(self, doc, tests, why):
        sel = tests["select"]
        self.assertIn("alpha", sel["rdeps"], doc.get("notes"))
        self.assertFalse((sel.get("narrow") or {}).get("on"), sel.get("narrow"))
        self.assertTrue(any(why in n for n in doc.get("notes") or []), (why, doc.get("notes")))

    def assert_full_suite(self, doc, tests, why):
        sel = tests["select"]
        self.assertTrue(sel["all"], doc.get("notes"))
        self.assertTrue(sel["filter"].startswith("all()"), sel["filter"])
        self.assertEqual([], sel["rdeps"])
        self.assertIn(sel["filter"], tests["cmd"])
        self.assertTrue(any(why in n for n in doc.get("notes") or []), (why, doc.get("notes")))


class MergeNarrowTest(StageBase):

    def setUp(self):
        super().setUp()
        # The merge gate names its wave (`merge.run_gate`); after the gate
        # only that wave's reports and strand files may move (OR-TB.S7.12).
        env = mock.patch.dict(os.environ, {narrow.WAVE_ENV: "w"})
        env.start()
        self.addCleanup(env.stop)

    def strand(self, name="welle/s", line="// strand\n", start=None):
        git(self.root, "checkout", "-q", "-b", name, start or self.map_rev)
        tip = self.edit_store(line)
        git(self.root, "checkout", "-q", "master")
        return tip

    def merge(self, *branches):
        git(self.root, "merge", "-q", "--no-ff", "-m", "merge", *branches)
        gate_plan._SOURCE_CACHE.clear()
        return git(self.root, "rev-parse", "HEAD")

    def test_merge_one_strand_parent_green_gate_plans_narrowed(self):
        gated = self.strand()
        # a report commit after the gate keeps the source closure (not the SHA)
        git(self.root, "checkout", "-q", "welle/s")
        tip = self.write({"plans/w/berichte/s.md": "# report\n"})
        git(self.root, "checkout", "-q", "master")
        run = self.gate_run(gated)
        base = git(self.root, "rev-parse", "master")
        self.merge("welle/s")
        doc, tests = self.build("merge", base, strand_run=run)
        sel = tests["select"]
        self.assertNotIn("alpha", sel["rdeps"], doc.get("notes"))
        self.assertIn("beta::b1_uses_alpha", sel["binaries"])
        term = "stage:merge narrowed parent=%s strand-run=%s" % (tip[:9], run)
        self.assertEqual(term, sel["narrow"]["narrowed"])
        self.assertTrue(sel["narrow"]["on"])
        self.assertIn("narrow: narrowing: %s" % term, sel["why"])
        self.assertTrue(any(term in n for n in doc["notes"]), doc["notes"])
        self.assertEqual("strand", doc["mode"])
        self.assertEqual("merge", doc["stage"])
        # the merge base is the strand's start, also when master moved since
        git(self.root, "reset", "-q", "--hard", base)
        moved = self.write({"docs/guide.md": "moved\n"})
        self.merge("welle/s")
        doc, tests = self.build("merge", moved, strand_run=run)
        self.assertNotIn("alpha", tests["select"]["rdeps"], doc.get("notes"))

    def test_merge_judges_the_maps_age_by_the_plans_clock(self):
        """GH #1199 B-3: the merge leg hands `build_plan(now=...)` to the narrowing."""
        gated = self.strand()
        run = self.gate_run(gated)
        base = git(self.root, "rev-parse", "master")
        self.merge("welle/s")

        def at(now):
            return self.build("merge", base, now=now, strand_run=run)

        def narrowed(doc, tests):
            self.assertNotIn("alpha", tests["select"]["rdeps"], doc.get("notes"))
            self.assertTrue(tests["select"]["narrow"]["on"], tests["select"]["narrow"])
        self.assert_aged_by_the_plans_clock(at, narrowed)
        doc, tests = at(self.CLOCK_MAP_DAY + datetime.timedelta(days=100))
        self.assert_unnarrowed(doc, tests, "the merge stays unnarrowed")

    def test_merge_plans_change_outside_reports_stays_unnarrowed(self):
        """re-check H2 m1: after the gate only reports and strand files of a
        wave may move -- Rust tests read other `plans/` files."""
        gated = self.strand()
        git(self.root, "checkout", "-q", "welle/s")
        self.write({"plans/w/expected_route_body.txt": "fn route() {}\n"})
        git(self.root, "checkout", "-q", "master")
        run = self.gate_run(gated)
        base = git(self.root, "rev-parse", "master")
        self.merge("welle/s")
        doc, tests = self.build("merge", base, strand_run=run)
        self.assert_unnarrowed(doc, tests, "differs from the gated tree")
        self.assertEqual("floor", doc["narrowing"]["tests"])
        self.assertEqual("merge", doc["narrowing"]["stage"])

    # OR-TB.S7.12: one source -- what may move after the gate is the
    # receipt-reuse rule (`turnaround.code_equal_revs`).

    def after_gate_unnarrowed(self, change):
        gated = self.strand()
        git(self.root, "checkout", "-q", "welle/s")
        change()
        git(self.root, "checkout", "-q", "master")
        run = self.gate_run(gated)
        base = git(self.root, "rev-parse", "master")
        self.merge("welle/s")
        doc, tests = self.build("merge", base, strand_run=run)
        self.assert_unnarrowed(doc, tests, "differs from the gated tree")

    def test_merge_a_symlink_report_after_the_gate_stays_unnarrowed(self):
        def change():
            os.makedirs(os.path.join(self.root, "plans/w/berichte"), exist_ok=True)
            os.symlink("s.md", os.path.join(self.root, "plans/w/berichte/l.md"))
            git(self.root, "add", "-A")
            git(self.root, "commit", "-q", "-m", "a link")
        self.after_gate_unnarrowed(change)

    def test_merge_another_waves_report_after_the_gate_stays_unnarrowed(self):
        self.after_gate_unnarrowed(lambda: self.write({"plans/other/berichte/s.md": "# r\n"}))

    def test_merge_scope_names_narrowed_and_its_base(self):
        gated = self.strand()
        git(self.root, "checkout", "-q", "welle/s")
        self.write({"plans/w/straenge/s.json": "{}\n"})
        git(self.root, "checkout", "-q", "master")
        run = self.gate_run(gated)
        base = git(self.root, "rev-parse", "master")
        self.merge("welle/s")
        doc, tests = self.build("merge", base, strand_run=run)
        self.assertEqual({"tests": "narrowed", "stage": "merge", "since": self.map_rev,
                          "wave": None, "map": self.map_rev, "why": None}, doc["narrowing"])

    def test_merge_scope_without_a_fitting_map_says_floor(self):
        """re-check m-N1: the merge decision holds but no map fits -- the tests
        ran as the resolver's floor, and SCOPE says so (not `narrowed`)."""
        gated = self.strand()
        run = self.gate_run(gated)
        base = git(self.root, "rev-parse", "master")
        self.merge("welle/s")
        self.cov, self.cov_why = None, "no coverage map in /nowhere"
        doc, tests = self.build("merge", base, strand_run=run)
        self.assertFalse((tests["select"].get("narrow") or {}).get("on"), tests["select"])
        scope = doc["narrowing"]
        self.assertEqual("floor", scope["tests"], scope)
        self.assertIsNone(scope["map"])
        self.assertTrue(scope["why"] and "no coverage map" in scope["why"], scope)

    def test_merge_revert_stays_unnarrowed(self):
        gated = self.strand()
        run = self.gate_run(gated)
        base = git(self.root, "rev-parse", "master")
        merged = self.merge("welle/s")
        git(self.root, "revert", "--no-edit", "-m", "1", merged)
        gate_plan._SOURCE_CACHE.clear()
        doc, tests = self.build("merge", merged, strand_run=run)
        self.assert_unnarrowed(doc, tests, "a revert")
        # a merge commit that says it reverts is no strand merge either
        git(self.root, "reset", "-q", "--hard", base)
        self.merge("welle/s")
        git(self.root, "commit", "-q", "--amend", "-m", "Revert \"x\"\n\nThis reverts commit %s." % base)
        doc, tests = self.build("merge", base, strand_run=run)
        self.assert_unnarrowed(doc, tests, "a revert")

    def test_merge_conflict_resolution_stays_unnarrowed(self):
        gated = self.strand(line="// strand\n")
        run = self.gate_run(gated)
        base = self.edit_store("// master\n")
        res = subprocess.run(["git", "-C", self.root, "merge", "-q", "--no-ff", "welle/s"],
                             env=dict(os.environ, **GIT_ENV), capture_output=True, text=True)
        self.assertNotEqual(0, res.returncode)
        self.write({STORE_RS: "pub fn resolved() {}\n"})   # the hand resolution
        doc, tests = self.build("merge", base, strand_run=run)
        self.assert_unnarrowed(doc, tests, "the parents conflict")
        # a clean merge with a Rust edit at the merge point: tree != union of the parents
        git(self.root, "reset", "-q", "--hard", base)
        git(self.root, "checkout", "-q", "-b", "welle/t", self.map_rev)
        gated = self.write({"crates/alpha/src/net.rs": "pub fn n() {}\n"})
        git(self.root, "checkout", "-q", "master")
        run = self.gate_run(gated)
        git(self.root, "merge", "-q", "--no-ff", "--no-commit", "welle/t")
        with open(os.path.join(self.root, "crates/beta/src/lib.rs"), "a", encoding="utf-8") as fh:
            fh.write("// evil\n")
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", "merge")
        gate_plan._SOURCE_CACHE.clear()
        doc, tests = self.build("merge", base, strand_run=run)
        self.assertTrue(any("not the union of its parents" in n for n in doc["notes"]), doc["notes"])
        self.assertFalse((tests["select"].get("narrow") or {}).get("on"))

    def test_i_h1_a_rust_edit_git_quotes_at_the_merge_point_stays_unnarrowed(self):
        """Re-check I-h1 (h): `git diff --name-only` without `-z` quoted
        `crates/beta/src/\\303\\244.rs`, the quoted name had no Rust class and
        the hand edit passed as the union of the parents (fail-open)."""
        git(self.root, "checkout", "-q", "-b", "welle/t", self.map_rev)
        gated = self.write({"crates/alpha/src/net.rs": "pub fn n() {}\n"})
        git(self.root, "checkout", "-q", "master")
        base = git(self.root, "rev-parse", "master")
        run = self.gate_run(gated)
        git(self.root, "merge", "-q", "--no-ff", "--no-commit", "welle/t")
        with open(os.path.join(self.root, "crates/beta/src/ä.rs"), "w",
                  encoding="utf-8") as fh:
            fh.write("// evil\n")
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", "merge")
        gate_plan._SOURCE_CACHE.clear()
        doc, tests = self.build("merge", base, strand_run=run)
        self.assertTrue(any("not the union of its parents" in n for n in doc["notes"]), doc["notes"])
        self.assertFalse((tests["select"].get("narrow") or {}).get("on"))

    def test_merge_with_a_crate_file_edit_at_the_merge_point_stays_unnarrowed(self):
        """Rule 10 (welle-tb S1b): an `include_str!` schema edited in the merge
        commit is compiled code the strand gate never saw -- as a Rust edit."""
        git(self.root, "checkout", "-q", "-b", "welle/t", self.map_rev)
        gated = self.write({"crates/alpha/src/net.rs": "pub fn n() {}\n"})
        git(self.root, "checkout", "-q", "master")
        base = git(self.root, "rev-parse", "master")
        run = self.gate_run(gated)
        git(self.root, "merge", "-q", "--no-ff", "--no-commit", "welle/t")
        os.makedirs(os.path.join(self.root, "crates/beta/schemas"), exist_ok=True)
        with open(os.path.join(self.root, "crates/beta/schemas/body.json"), "w",
                  encoding="utf-8") as fh:
            fh.write("{}\n")
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", "merge")
        gate_plan._SOURCE_CACHE.clear()
        doc, tests = self.build("merge", base, strand_run=run)
        self.assertTrue(any("not the union of its parents" in n for n in doc["notes"]), doc["notes"])
        self.assertFalse((tests["select"].get("narrow") or {}).get("on"))

    def test_merge_two_strand_parents_stays_unnarrowed(self):
        gated = self.strand("welle/a", "// a\n")
        run = self.gate_run(gated)
        git(self.root, "checkout", "-q", "-b", "welle/b", self.map_rev)
        self.write({"crates/alpha/src/net.rs": "pub fn n() {}\n"})
        git(self.root, "checkout", "-q", "master")
        base = git(self.root, "rev-parse", "master")
        self.merge("welle/a", "welle/b")
        doc, tests = self.build("merge", base, strand_run=run)
        self.assert_unnarrowed(doc, tests, "more than one strand parent")

    def test_merge_without_green_strand_receipt_stays_unnarrowed(self):
        gated = self.strand()
        base = git(self.root, "rev-parse", "master")
        self.merge("welle/s")
        for run, why in ((None, "no strand run"),
                         ("tr-unknown", "is not a GREEN strand gate"),
                         (self.gate_run(gated, verdict="RED"), "is not a GREEN strand gate"),
                         (self.gate_run(gated, mode="integration"), "is not a GREEN strand gate"),
                         (self.gate_run(gated, dirty=True), "is not a GREEN strand gate"),
                         (self.gate_run(self.map_rev), "differs from the gated tree")):
            doc, tests = self.build("merge", base, strand_run=run)
            self.assert_unnarrowed(doc, tests, why)
        # without a store nothing can be checked
        with mock.patch.object(narrow, "newest_map", return_value=(self.cov, None)), \
                mock.patch("pipeline.graph.load", return_value=self.doc), \
                mock.patch.object(narrow, "_open_store", return_value=None):
            doc = plan.build_plan(self.root, "merge", base, resolver=self.resolver,
                                  metadata=self.meta, narrow=True,
                                  strand_run=self.gate_run(gated))
        self.assertTrue(any("no store" in n for n in doc["notes"]), doc["notes"])

    def test_merge_map_not_fitting_stays_unnarrowed(self):
        gated = self.strand()
        run = self.gate_run(gated)
        base = git(self.root, "rev-parse", "master")
        self.merge("welle/s")
        self.cov = dict(self.cov, parts=self.cov["parts"] + [
            {"binary": "alpha::gone", "module": None, "files": {STORE_RS: 1}}])
        doc, tests = self.build("merge", base, strand_run=run)
        self.assert_unnarrowed(doc, tests, "merge stays unnarrowed")

    def test_narrowed_merge_with_no_rdeps_to_replace_is_still_on(self):
        """V12: the narrowing state, not only the coverage use, tells a
        narrowed merge from an unnarrowed one (the cache key reads `on`)."""
        # re-check m-N7: a Rust diff without rdeps (a crate test file) keeps
        # the map path; a python/docs-only one plans floor + graph (below)
        git(self.root, "checkout", "-q", "-b", "welle/d", self.map_rev)
        gated = self.write({"crates/alpha/tests/a2_template.rs":
                            '#[test]\nfn t() {\n    let _ = "templates/talky/config.json";\n}\n'
                            '#[test]\nfn changed() {}\n'})
        git(self.root, "checkout", "-q", "master")
        run = self.gate_run(gated)
        base = git(self.root, "rev-parse", "master")
        self.merge("welle/d")
        doc, tests = self.build("merge", base, strand_run=run)
        self.assertEqual([], tests["select"]["rdeps"], doc.get("notes"))
        self.assertEqual(narrow.state(True, self.map_rev, tests["select"]["narrow"]["narrowed"]),
                         tests["select"]["narrow"])

    def test_merge_docs_only_with_fitting_map_plans_floor_graph_like_strand(self):
        """re-check m-N7: a python/docs-only merge plans floor + graph without
        the map's drift terms -- never more than its strand, `on` false."""
        git(self.root, "checkout", "-q", "-b", "welle/d", self.map_rev)
        self.edit_store("// drift since the map\n")
        git(self.root, "checkout", "-q", "master")
        self.merge("welle/d")
        git(self.root, "checkout", "-q", "-b", "welle/p")
        gated = self.write({"docs/guide.md": "changed\n"})
        git(self.root, "checkout", "-q", "master")
        run = self.gate_run(gated)
        base = git(self.root, "rev-parse", "master")
        self.merge("welle/p")
        doc, tests = self.build("merge", base, strand_run=run)
        sel, notes = tests["select"], doc.get("notes") or []
        term = sel["narrow"]["narrowed"]
        self.assertTrue(term.startswith("stage:merge narrowed"), sel["narrow"])
        self.assertEqual(narrow.state(False, "", term), sel["narrow"])
        self.assertTrue(any("no Rust source in the diff" in n for n in notes), notes)
        self.assertFalse(any("does not fit" in n for n in notes), notes)
        sdoc, stests = self.build("strand", base)
        self.assertEqual(set(stests["select"]["binaries"]), set(sel["binaries"]), notes)
        self.assertIn("alpha::a1_reads_doc", sel["binaries"])


class IntegrationNarrowTest(StageBase):
    def start(self, rev, wave="welle-x"):
        events.emit(self.db, "wave_started", "status", {"wave": wave, "start": rev})

    def test_integration_since_wave_start_plans_narrowed(self):
        old = self.edit_store("// before the wave\n")
        self.cov = dict(self.cov, rev=old)
        self.start(old)
        self.edit_store("// in the wave\n")
        doc, tests = self.build("integration", self.map_rev, wave="welle-x")
        sel = tests["select"]
        self.assertNotIn("alpha", sel["rdeps"], doc.get("notes"))
        self.assertIn("beta::b1_uses_alpha", sel["binaries"])
        term = "stage:integration narrowed since=%s" % old[:9]
        self.assertEqual(term, sel["narrow"]["narrowed"])
        self.assertTrue(any(term in n for n in doc["notes"]), doc["notes"])
        self.assertEqual("integration", doc["mode"])
        self.assertEqual({"tests": "narrowed", "stage": "integration", "since": old,
                          "wave": "welle-x", "map": old, "why": None}, doc["narrowing"])
        # the wave diff, not the export base: a docs-only wave plans no alpha
        later = git(self.root, "rev-parse", "HEAD")
        self.start(later, wave="welle-y")
        self.write({"crates/beta/src/lib.rs": "// beta only\n"})
        doc, tests = self.build("integration", self.map_rev, wave="welle-y")
        self.assertEqual("stage:integration narrowed since=%s" % later[:9],
                         tests["select"]["narrow"]["narrowed"])
        self.assertFalse(tests["select"]["all"])
        self.assertIn("beta", tests["select"]["rdeps"])   # not in the map: rdeps kept

    def test_integration_judges_the_maps_age_by_the_plans_clock(self):
        """GH #1199 B-3: the integration leg hands `build_plan(now=...)` to the narrowing."""
        self.start(self.map_rev)
        self.edit_store("// in the wave\n")

        def at(now):
            return self.build("integration", self.map_rev, now=now, wave="welle-x")

        def narrowed(doc, tests):
            self.assertFalse(tests["select"]["all"], doc.get("notes"))
            self.assertNotIn("alpha", tests["select"]["rdeps"], doc.get("notes"))
            self.assertTrue(tests["select"]["narrow"]["on"], tests["select"]["narrow"])
        self.assert_aged_by_the_plans_clock(at, narrowed)

    def test_integration_wave_name_matches_with_or_without_date(self):
        """re-check H2 M1 (a): `strand.sh` passes the wave without its date
        (`wave_name`), the start may carry it -- one wave."""
        self.start(self.map_rev, wave="welle-x-2026-10-10")
        self.edit_store("// in the wave\n")
        for name in ("welle-x", "welle-x-2026-10-10"):
            doc, tests = self.build("integration", self.map_rev, wave=name)
            self.assertEqual("stage:integration narrowed since=%s" % self.map_rev[:9],
                             tests["select"]["narrow"]["narrowed"], (name, doc.get("notes")))

    def test_integration_dated_waves_of_one_name_stay_apart(self):
        """re-check m-N2: `welle-f-2026-10-01` and `welle-f-2026-10-20` are two
        waves; only the name without its date is ambiguous (full suite)."""
        self.start(self.map_rev, wave="welle-f-2026-10-01")
        later = self.edit_store("// next wave\n")
        self.assertEqual(0, wave.start("welle-f-2026-10-20", root=self.root, db=self.db,
                                       rev=later, out=lambda *_: None))
        self.assertEqual(later, narrow.wave_start(self.db, "welle-f-2026-10-20"))
        self.assertEqual(self.map_rev, narrow.wave_start(self.db, "welle-f-2026-10-01"))
        self.edit_store("// in the wave\n")
        doc, tests = self.build("integration", self.map_rev, wave="welle-f-2026-10-20")
        self.assertEqual("stage:integration narrowed since=%s" % later[:9],
                         tests["select"]["narrow"]["narrowed"], doc.get("notes"))
        doc, tests = self.build("integration", self.map_rev, wave="welle-f")
        self.assert_full_suite(doc, tests, "different starts")
        # a start without its date still is the start of the dated wave
        self.start(self.map_rev, wave="welle-g")
        self.assertEqual(self.map_rev, narrow.wave_start(self.db, "welle-g-2026-10-10"))

    def test_integration_first_wave_start_wins_contradiction_plans_full_suite(self):
        """re-check H2 M2 (KR-TB-1 b): the base is master at the FIRST start; a
        second, different start of the same wave is a contradiction -- full suite."""
        self.start(self.map_rev)
        later = self.edit_store("// in the wave\n")
        self.assertEqual(self.map_rev, narrow.wave_start(self.db, "welle-x"))
        self.start(later, wave="welle-x-2026-10-10")
        self.assertEqual(self.map_rev, narrow.wave_start(self.db, "welle-x"))
        doc, tests = self.build("integration", self.map_rev, wave="welle-x")
        self.assert_full_suite(doc, tests, "different starts")
        self.assertEqual("full-suite", doc["narrowing"]["tests"])
        self.assertIn("different starts", doc["narrowing"]["why"])

    def test_integration_unknown_wave_start_plans_full_suite(self):
        self.edit_store("// in the wave\n")
        for wave, why in ((None, "no wave named"), ("welle-none", "has no start in the store")):
            doc, tests = self.build("integration", self.map_rev, wave=wave)
            self.assert_full_suite(doc, tests, why)
            self.assertEqual("full-suite", doc["narrowing"]["tests"], doc["narrowing"])
            self.assertIn(why, doc["narrowing"]["why"])

    def test_integration_wave_start_not_ancestor_plans_full_suite(self):
        git(self.root, "checkout", "-q", "-b", "side")
        side = self.edit_store("// side\n")
        git(self.root, "checkout", "-q", "master")
        self.edit_store("// in the wave\n")
        # one start per wave: two starts of one wave are a contradiction (H2 M2)
        for start, name in ((side, "welle-x"), ("0" * 40, "welle-z")):
            self.start(start, wave=name)
            doc, tests = self.build("integration", self.map_rev, wave=name)
            self.assert_full_suite(doc, tests, "is not an ancestor of HEAD")

    def test_integration_map_not_fitting_plans_full_suite(self):
        self.start(self.map_rev)
        self.edit_store("// in the wave\n")
        self.cov = dict(self.cov, parts=self.cov["parts"] + [
            {"binary": "alpha::gone", "module": None, "files": {STORE_RS: 1}}])
        doc, tests = self.build("integration", self.map_rev, wave="welle-x")
        self.assert_full_suite(doc, tests, "names 1 binaries HEAD does not have")
        self.cov = dict(COVERAGE, rev=self.map_rev, complete=False)
        doc, tests = self.build("integration", self.map_rev, wave="welle-x")
        self.assert_full_suite(doc, tests, "is not complete")
        with mock.patch.object(narrow, "newest_map", return_value=(None, "no coverage map")), \
                mock.patch("pipeline.graph.load", return_value=self.doc), \
                mock.patch.dict(os.environ, {narrow.ENV: "1"}):
            doc = plan.build_plan(self.root, "integration", self.map_rev, resolver=self.resolver,
                                  metadata=self.meta, narrow=True, db=self.db, wave="welle-x")
        tests = next(s for s in doc["stations"] if s["name"] == "tests")
        self.assert_full_suite(doc, tests, "no coverage map")

    def test_integration_python_only_wave_plans_floor_graph_not_full_suite(self):
        """re-check H2 M3: a python/docs-only wave diff has nothing the map
        could replace -- floor + graph, unnarrowed (`on` false, the key stays
        the unnarrowed one), never the full suite, whether the map fits or not."""
        self.start(self.map_rev)
        self.write({"docs/guide.md": "in the wave\n", "scripts/other.py": "# wave\n"})
        term = "stage:integration narrowed since=%s" % self.map_rev[:9]
        for cov in (dict(self.cov), dict(self.cov, complete=False)):
            self.cov = cov
            doc, tests = self.build("integration", self.map_rev, wave="welle-x")
            sel, notes = tests["select"], doc.get("notes") or []
            self.assertFalse(sel["all"], notes)
            self.assertFalse(any("full suite" in n for n in notes), notes)
            self.assertIn("alpha::a1_reads_doc", sel["binaries"], notes)   # the page's reader
            self.assertEqual(narrow.state(False, "", term), sel["narrow"])
            self.assertTrue(any("no Rust source in the diff" in n for n in notes), notes)
            self.assertFalse(any("does not fit" in n for n in notes), notes)
            self.assertEqual("floor", doc["narrowing"]["tests"])

    def test_integration_unknown_reader_plans_full_suite(self):
        self.start(self.map_rev)
        self.write({STORE_RS: "// in the wave\n",
                    "crates/alpha/tests/fixtures/orphan.json": "{}\n"})
        doc, tests = self.build("integration", self.map_rev, wave="welle-x")
        self.assert_full_suite(doc, tests, "unknown reader")
        # a workspace file: the floor says all() -- full suite, never narrowed
        self.write({"Cargo.toml": FILES_CARGO + "# x\n"})
        doc, tests = self.build("integration", self.map_rev, wave="welle-x")
        self.assertTrue(tests["select"]["all"])

    def test_release_plans_everything_unchanged(self):
        self.start(self.map_rev)
        self.edit_store("// in the wave\n")
        res = impact.impact([STORE_RS], "release", self.doc, repo=self.root, coverage=self.cov,
                            now=NOW, narrowed="stage:integration narrowed since=x")
        self.assertEqual(["all()"], res["tests"])
        self.assertEqual({"all()": ["stage:release"]},
                         {k: v for k, v in res["reason"].items() if k == "all()"})

        def release_resolver(_repo, _mode, _files):
            return {"stations": [{"name": "tests", "scope": "all()", "cmd": ["t", "all()"],
                                  "select": {"binaries": [], "rdeps": [], "all": True,
                                             "floor": False, "extra": [], "minus": [],
                                             "filter": "all()"}}]}
        with mock.patch.object(narrow, "apply") as ap:
            doc = plan.build_plan(self.root, "release", self.map_rev, resolver=release_resolver,
                                  metadata=self.meta, narrow=True, db=self.db, wave="welle-x")
        self.assertFalse(ap.called)
        tests = next(s for s in doc["stations"] if s["name"] == "tests")
        self.assertEqual("all()", tests["select"]["filter"])
        self.assertNotIn("notes", doc)


class StrandClockTest(StageBase):
    def test_strand_judges_the_maps_age_by_the_plans_clock(self):
        """GH #1199 B-3: the strand leg hands `build_plan(now=...)` to the narrowing."""
        self.edit_store("// in the strand\n")

        def at(now):
            return self.build("strand", self.map_rev, now=now)

        def narrowed(doc, tests):
            self.assertNotIn("alpha", tests["select"]["rdeps"], doc.get("notes"))
            self.assertIn("beta::b1_uses_alpha", tests["select"]["binaries"], doc.get("notes"))
            self.assertTrue(tests["select"]["narrow"]["on"], tests["select"]["narrow"])
        self.assert_aged_by_the_plans_clock(at, narrowed)
        doc, tests = at(self.CLOCK_MAP_DAY + datetime.timedelta(days=100))
        self.assertIn("alpha", tests["select"]["rdeps"], doc.get("notes"))


class WaveStartTest(StageBase):
    def test_wave_start_records_an_ancestor_of_master_only(self):
        head = self.edit_store("// tip\n")
        self.assertEqual(0, wave.start("welle-m", root=self.root, db=self.db, rev=self.map_rev,
                                       out=lambda *_: None))
        self.assertEqual(self.map_rev, narrow.wave_start(self.db, "welle-m"))
        self.assertEqual(0, wave.start("welle-n", root=self.root, db=self.db, rev="master",
                                       out=lambda *_: None))
        self.assertEqual(head, narrow.wave_start(self.db, "welle-n"))
        git(self.root, "checkout", "-q", "-b", "side")
        side = self.edit_store("// side\n")
        git(self.root, "checkout", "-q", "master")
        self.assertEqual(2, wave.start("welle-o", root=self.root, db=self.db, rev=side,
                                       out=lambda *_: None))
        self.assertIsNone(narrow.wave_start(self.db, "welle-o"))
        self.assertEqual(2, wave.start("welle-p", root=self.root, db=self.db, rev="nope",
                                       out=lambda *_: None))

    def test_second_wave_start_is_refused_the_first_wins(self):
        """re-check H2 M2: a late or second `wave start` never moves the base."""
        head = self.edit_store("// tip\n")
        said = []
        self.assertEqual(0, wave.start("welle-m-2026-10-10", root=self.root, db=self.db,
                                       rev=self.map_rev, out=said.append))
        self.assertEqual(2, wave.start("welle-m", root=self.root, db=self.db, rev="master",
                                       out=said.append))
        self.assertTrue(any(self.map_rev[:9] in s and "refused" in s for s in said), said)
        self.assertEqual(self.map_rev, narrow.wave_start(self.db, "welle-m"))
        self.assertNotIn(head, narrow.wave_starts(self.db, "welle-m"))
        # the same start again is no contradiction (idempotent, no second event)
        self.assertEqual(0, wave.start("welle-m", root=self.root, db=self.db, rev=self.map_rev,
                                       out=said.append))
        self.assertEqual([self.map_rev], narrow.wave_starts(self.db, "welle-m"))
        n = self.db.execute("SELECT COUNT(*) FROM journal WHERE event='wave_started'").fetchone()[0]
        self.assertEqual(1, n)

    def test_wave_start_without_rev_is_refused(self):
        """re-check M-N1 (OR-TB.S1.22): without `--rev` a late first start would
        set the base on the tip after merges of the wave (fail-open) -- refused,
        the rev is always named."""
        self.edit_store("// tip\n")
        said = []
        self.assertEqual(2, wave.start("welle-q", root=self.root, db=self.db, out=said.append))
        self.assertTrue(any("--rev" in s and "refused" in s for s in said), said)
        self.assertEqual([], narrow.wave_starts(self.db, "welle-q"))
        with mock.patch.object(wave.store, "open", return_value=self.db), \
                mock.patch.object(wave.store, "close"), \
                mock.patch("builtins.print"):
            self.assertEqual(2, wave.main(["wave", "start", "welle-q"]))
        self.assertEqual([], narrow.wave_starts(self.db, "welle-q"))


# --- a browser unit the narrowing reaches beyond the diff (tb-s3-b, GH #1161) --
# `tests` subtracts every browser unit (rule 11, `gate_plan.tests_minus_for`),
# but `browser:display` takes only the ones the DIFF reaches
# (`gate_plan.browser_moved`). A crate changed only since the coverage map keeps
# its `rdeps` (M-4, `since_plan`) -- a browser unit of it ran in no station.

BROWSER_UNIT = "crates/alpha/tests/a9_the_page_draws_browser.rs"
BROWSER_TERM = "binary_id(=alpha::a9_the_page_draws_browser)"
BETA_SRC = "crates/beta/src/lib.rs"


# A unit with one runnable test that `gate_plan._test_counts` reads as (0, 0):
# the string continued with `\<newline>` ends the reader's string early, its
# closing quote opens one that swallows `#[test]` (F-1, the cut of
# gh703_the_phone_holds_in_webkit_browser.rs from its line 66 on).
# A unit the reader counts (0, 0): since the token lexer (welle-tb S4, #1162)
# a `\\`-newline string is read (B-neu-1's shape), and since tb-s10 A1 point 20
# a non-ASCII identifier too. Left: a unit whose tests a macro makes -- no test
# attribute the count can see.
LOST_UNIT = 'cells! {\n    draws => "ab",\n}\n'


def browser_metadata(root):
    meta = metadata(root)
    alpha = next(p for p in meta["packages"] if p["name"] == "alpha")
    alpha["targets"].append({"kind": ["test"], "name": "a9_the_page_draws_browser",
                             "src_path": os.path.join(root, BROWSER_UNIT)})
    return meta


class BrowserUnitBeyondTheDiffTest(Base):
    files = dict(FILES, **{BROWSER_UNIT: "#[test]\nfn draws() {}\n"})

    floor = NarrowTest.floor
    commit = NarrowTest.commit
    executed = NarrowTest.executed

    def setUp(self):  # NarrowTest's, with the browser unit's metadata
        super().setUp()
        gate_plan._BROWSER_CACHE.clear()
        git(self.root, "init", "-q", "-b", "master")
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", "map rev")
        self.map_rev = git(self.root, "rev-parse", "HEAD")
        self.cov = dict(COVERAGE, rev=self.map_rev)
        self.meta = browser_metadata(self.root)

    def tearDown(self):
        gate_plan._BROWSER_CACHE.clear()
        super().tearDown()

    def test_the_fixture_unit_is_a_browser_unit_the_diff_does_not_reach(self):
        self.assertIn(BROWSER_TERM, gate_plan.browser_terms(self.root))
        self.assertEqual([], gate_plan.browser_reached([BETA_SRC], "strand", self.root))

    def test_a_crate_changed_since_the_map_runs_its_browser_unit_somewhere(self):
        """Red before tb-s3-b: rdeps(alpha) of net.rs since the map reached the
        unit, `render_filter` subtracted it, and no `browser:display` planned it.
        Now the station is planned for exactly that unit -- booked (`NEEDS` by
        name; it reuses the build, `plan.REUSE`, no cargo lock of its own) and
        with a `reason` naming it: `tests` books no engine and runs without
        MECLAW_REQUIRE_BROWSERS (pipeline/plan.py NEEDS, executor `_env`), so a
        unit left there would SKIP green on a lane without the engine."""
        self.commit({"crates/alpha/src/net.rs": "pub fn net2() {}\n"})
        rows = []
        sel, filt, notes = self.executed([BETA_SRC], out=rows)
        self.assertIn("rdeps(alpha)", filter_terms(filt), (filt, notes))
        self.assertIn(BROWSER_TERM, filt.partition(" - ")[2], (filt, notes))
        station = next((r for r in rows if r["name"] == "browser:display"), None)
        self.assertIsNotNone(station, notes)
        self.assertEqual({BROWSER_TERM}, narrow.browser_planned(rows, self.root))
        self.assertEqual([["scripts/test-tier.sh", "filter", BROWSER_TERM, "--run-ignored", "all"]],
                         [station["cmd"]])
        self.assertTrue(station["cargo"])
        self.assertIn("browser_webkit", plan.NEEDS["browser:display"])
        self.assertIn("alpha::a9_the_page_draws_browser", station["select"]["extra"])
        self.assertEqual(["tests reaches %s beyond the plan -- booked here (tb-s3-b)"
                          % BROWSER_TERM], station["reason"])
        self.assertIn("browser:display", plan.REUSE)
        self.assertEqual([], narrow.browser_orphans(rows, self.root), (filt, notes))

    def test_an_orphan_joins_a_planned_station(self):
        """A station the diff planned without the unit takes it into both parts."""
        self.commit({"crates/alpha/src/net.rs": "pub fn net2() {}\n"})
        station = {"name": "browser:display", "cmds": gate_plan.browser_cmds(False)}
        rows = []
        _sel, filt, notes = self.executed([BETA_SRC], extra_rows=[station], out=rows)
        self.assertIn(BROWSER_TERM, filt.partition(" - ")[2], (filt, notes))
        self.assertEqual(2, len(station["cmds"]))
        for argv in station["cmds"]:
            self.assertIn(" + %s)" % BROWSER_TERM, argv[2])
        self.assertTrue(any(BROWSER_TERM in r for r in station["reason"]), station)
        self.assertEqual([], narrow.browser_orphans(rows, self.root))

    def test_the_guard_names_a_unit_that_runs_nowhere(self):
        """The generic guard: subtracted AND reached by a summand AND not in
        the station. A lib part reaches no `tests/` unit; a unit with every
        test `#[ignore]`d runs in `tests` neither way."""
        def rows(filt, *extra):
            return [{"name": "tests", "select": {"filter": filt}}] + list(extra)
        cut = "rdeps(alpha) - (%s)" % BROWSER_TERM
        self.assertEqual([BROWSER_TERM], narrow.browser_orphans(rows(cut), self.root))
        self.assertEqual([BROWSER_TERM], narrow.browser_orphans(
            rows("%s + rdeps(beta) - (binary(/x/) + %s)" % (BROWSER_TERM, BROWSER_TERM)),
            self.root))
        station = {"name": "browser:display",
                   "cmds": gate_plan.browser_cmds(False, moved=[BROWSER_TERM])}
        self.assertEqual([], narrow.browser_orphans(rows(cut, station), self.root))
        self.assertEqual([], narrow.browser_orphans(
            rows("(binary_id(=alpha) & test(/^net::/)) + rdeps(beta) - (%s)" % BROWSER_TERM),
            self.root))
        self.assertEqual([], narrow.browser_orphans(rows("rdeps(alpha)"), self.root))
        # a lock the integration's cut left out is the ruling's (browser_plan)
        with mock.patch.object(gate_plan, "BROWSER_LOCKS", frozenset({BROWSER_UNIT})):
            self.assertEqual([BROWSER_TERM], narrow.browser_orphans(rows(cut), self.root))
            self.assertEqual([], narrow.browser_orphans(rows(cut), self.root, locks=False))
        with open(os.path.join(self.root, BROWSER_UNIT), "w", encoding="utf-8") as fh:
            fh.write("#[test]\n#[ignore]\nfn draws() {}\n")
        gate_plan._SOURCE_CACHE.clear()
        gate_plan._BROWSER_CACHE.clear()
        self.assertEqual([], narrow.browser_orphans(rows(cut), self.root))

    def test_the_full_suite_books_its_browser_units(self):
        """Re-check n2 M-2: an integration in doubt plans the full suite
        (`full_suite`, KR-TB-1; a full pass the same). It subtracted only the
        scenario class, so every browser unit outside the export diff ran in
        `tests` -- no engine booked, no MECLAW_REQUIRE_BROWSERS, SKIP green on
        a lane without the engine. Now `tests` subtracts them all and
        `browser:display` runs them, planned for them when the export diff
        planned no station, joined to the station when it did."""
        def tests_row():
            return {"name": "tests", "select": {"filter": "rdeps(beta)", "rdeps": ["beta"]},
                    "cmd": ["scripts/test-tier.sh", "filter", "rdeps(beta)"]}
        rows = [tests_row()]
        notes = narrow.full_suite(rows, self.root, ["no coverage map"], env={})
        filt = rows[0]["select"]["filter"]
        self.assertTrue(filt.startswith("all()"), filt)
        self.assertIn(BROWSER_TERM, filt.partition(" - ")[2], (filt, notes))
        self.assertIn(filt, rows[0]["cmd"])
        self.assertEqual({BROWSER_TERM}, narrow.browser_planned(rows, self.root), notes)
        self.assertEqual([], narrow.browser_orphans(rows, self.root))
        station = {"name": "browser:display", "cmds": gate_plan.browser_cmds(False)}
        rows = [tests_row(), station]
        narrow.full_suite(rows, self.root, ["no coverage map"], env={})
        self.assertEqual(2, len(station["cmds"]))
        for argv in station["cmds"]:
            self.assertIn(" + %s)" % BROWSER_TERM, argv[2])
        self.assertEqual([], narrow.browser_orphans(rows, self.root))

    def test_a_unit_the_reader_counts_empty_is_an_orphan(self):
        """Re-check 003a717c3 B-neu-1: the reader (`gate_plan._test_counts`)
        lost a string with a line continuation (`"a\\<newline> b"`, F-1, read
        since the token lexer of S4; the unit here makes its tests by macro) and
        counted five browser units of the tree (0, 0) -- 744, gh695,
        gh703_phone, gh767_page_canvas, gh958, 11 runnable tests. Read as
        "no runnable test", `tests` subtracted them and `browser:display`
        planned none of them: they ran nowhere, in a narrowed integration and
        in the full suite alike. Only a unit whose tests the reader sees and
        finds every one `#[ignore]`d is none; (0, 0) is doubt, an orphan."""
        with open(os.path.join(self.root, BROWSER_UNIT), "w", encoding="utf-8") as fh:
            fh.write(LOST_UNIT)
        gate_plan._SOURCE_CACHE.clear()
        gate_plan._BROWSER_CACHE.clear()
        self.assertEqual((0, 0), gate_plan._test_counts(LOST_UNIT))  # the reader's miss
        cut = "rdeps(alpha) - (%s)" % BROWSER_TERM
        self.assertEqual([BROWSER_TERM], narrow.browser_orphans(
            [{"name": "tests", "select": {"filter": cut}}], self.root))
        rows = [{"name": "tests", "select": {"filter": "rdeps(beta)", "rdeps": ["beta"]},
                 "cmd": ["scripts/test-tier.sh", "filter", "rdeps(beta)"]}]
        notes = narrow.full_suite(rows, self.root, ["no coverage map"], env={})
        self.assertEqual({BROWSER_TERM}, narrow.browser_planned(rows, self.root), notes)
        self.assertEqual(["browser:display"], [r["name"] for r in rows[1:]], notes)

    # --- GH #1182: the guard over the whole plan, wired behind the orphans -----

    def test_gh1182_the_narrowing_refuses_a_subtracted_unit_no_station_runs(self):
        """GH #1182, the case of tb-s3 aa9454842, through `narrow.apply` and so
        through `narrow()`: rdeps(alpha) of net.rs since the map reaches the
        unit, `render_filter` subtracts it, and no station plans it (the orphan
        step as it stood at aa9454842: there was none). `narrow()` exempts the
        policy subtraction (OR-TB.S1e.1) and keeps its narrowed filter; the
        guard over the whole plan (`unrun_subtractions`, behind
        `plan_browser_orphans`) refuses the plan and names the unit. Red before
        the wiring: the plan went through silent (re-check tb-s1e h1 M-1, P1)."""
        self.commit({"crates/alpha/src/net.rs": "pub fn net2() {}\n"})
        rows = []
        with mock.patch.object(narrow, "plan_browser_orphans", return_value=[]), \
                self.assertRaises(ValueError) as cm:
            self.executed([BETA_SRC], out=rows)
        self.assertIsInstance(cm.exception, narrow.UnrunSubtraction)
        self.assertIn(BROWSER_TERM, str(cm.exception))
        filt = rows[0]["select"]["filter"]
        self.assertIn("rdeps(alpha)", filter_terms(filt), filt)
        self.assertIn(BROWSER_TERM, filt.partition(" - ")[2], filt)
        self.assertNotIn("browser:display", [r["name"] for r in rows])

    def test_gh1182_the_full_suite_refuses_a_unit_no_station_runs(self):
        """The second call: an integration in doubt (`full_suite`, also the full
        pass) subtracts every browser unit; a unit no station takes is named.
        No lock cut in doubt -- a lock proof stays under the guard."""
        def rows():
            return [{"name": "tests", "select": {"filter": "rdeps(beta)", "rdeps": ["beta"]},
                     "cmd": ["scripts/test-tier.sh", "filter", "rdeps(beta)"]}]
        for locks in (frozenset(), frozenset({BROWSER_UNIT})):
            with self.subTest(locks=sorted(locks)):
                with mock.patch.object(gate_plan, "BROWSER_LOCKS", locks), \
                        mock.patch.object(narrow, "plan_browser_orphans", return_value=[]), \
                        self.assertRaises(ValueError) as cm:
                    narrow.full_suite(rows(), self.root, ["no coverage map"], env={})
                self.assertIsInstance(cm.exception, narrow.UnrunSubtraction)
                self.assertIn(BROWSER_TERM, str(cm.exception))

    def test_gh1182_a_unit_with_every_test_ignored_is_no_unrun_subtraction(self):
        """Re-check tb-s1e h1 M-1 (a): a unit whose every test is `#[ignore]`d
        runs in `tests` neither way, so subtracting it takes nothing away. The
        orphan step skips it; the guard must as well, or every plan reaching
        meclaw-cells is refused (six such units there: the 710 colony half,
        gh867, gh961, gh1002, gh1003, gh1028). Narrowed and full suite."""
        self.commit({"crates/alpha/src/net.rs": "pub fn net2() {}\n",
                     BROWSER_UNIT: "#[test]\n#[ignore]\nfn draws() {}\n"})
        gate_plan._BROWSER_CACHE.clear()
        self.assertIn(BROWSER_TERM, gate_plan.browser_terms(self.root))
        rows = []
        _sel, filt, notes = self.executed([BETA_SRC], out=rows)
        self.assertIn(BROWSER_TERM, filt.partition(" - ")[2], (filt, notes))
        self.assertNotIn("browser:display", [r["name"] for r in rows])
        full = [{"name": "tests", "select": {"filter": "rdeps(beta)", "rdeps": ["beta"]},
                 "cmd": ["scripts/test-tier.sh", "filter", "rdeps(beta)"]}]
        narrow.full_suite(full, self.root, ["no coverage map"], env={})
        self.assertNotIn("browser:display", [r["name"] for r in full])

    def test_gh1182_a_lock_the_station_drives_by_its_binary_runs(self):
        """`browser_cmds` names a lock by its binary (`binary(/<stem>/)`), `tests`
        subtracts it by its id -- the guard reads the one for the other, as the
        station's `select` does (`gate_plan._browser_select`). Otherwise every
        plan with the station and a crate-wide `tests` term was refused."""
        lock = "binary(/a9_the_page_draws/)"
        two = [["scripts/test-tier.sh", "filter", "(%s) %s %s" % (lock, op, gate_plan.WEBKIT_TESTS),
                "--run-ignored", "all"] for op in ("-", "&")]
        one = [["scripts/test-tier.sh", "filter", lock, "--run-ignored", "all"]]
        for cmds in (two, one):
            with self.subTest(cmds=cmds):
                station = {"name": "browser:display", "cmds": copy.deepcopy(cmds)}
                rows = [{"name": "tests", "select": {"filter": "rdeps(beta)", "rdeps": ["beta"]},
                         "cmd": ["scripts/test-tier.sh", "filter", "rdeps(beta)"]}, station]
                with mock.patch.object(gate_plan, "BROWSER_LOCKS", frozenset({BROWSER_UNIT})):
                    narrow.full_suite(rows, self.root, ["no coverage map"], env={})
                self.assertIn(BROWSER_TERM, rows[0]["select"]["filter"].partition(" - ")[2])
                self.assertEqual(cmds, station["cmds"])   # planned as a lock, no orphan

    def test_gh1182_a_count_the_lexer_lost_is_doubt(self):
        """Re-check tb-s1e h1 F-1: `_test_counts` read (0, 0) for five real
        browser units with a runnable test (gh703 among them). (0, 0) is no
        reliable "all ignored" -- the unit runs in `browser:display` or the plan
        is refused, never nowhere (whichever the orphan step decides)."""
        real = gate_plan._test_counts
        self.commit({"crates/alpha/src/net.rs": "pub fn net2() {}\n",
                     BROWSER_UNIT: "#[test]\nfn draws() { /* lexer lost */ }\n"})
        gate_plan._BROWSER_CACHE.clear()
        rows = []
        with mock.patch.object(gate_plan, "_test_counts",
                               side_effect=lambda t: (0, 0) if "lexer lost" in t else real(t)):
            try:
                self.executed([BETA_SRC], out=rows)
            except narrow.UnrunSubtraction as exc:
                self.assertIn(BROWSER_TERM, str(exc))
            else:
                self.assertIn(BROWSER_TERM, narrow.browser_planned(rows, self.root))

    def test_gh1182_the_guard_reads_a_lost_count_as_doubt_on_its_own(self):
        """Since B-neu-1 (tb-s3 4991406c5) the orphan step plans a unit the
        reader counts (0, 0), so in the composed plan the guard never meets
        one: the mutant "(0, 0) exempt like all-ignored" (M7) stayed green on
        the w2 scaffold. The guard is the last line on its own -- should the
        orphan step miss such a unit, the plan is refused, narrowed and full
        suite alike, never run with the hole."""
        real = gate_plan._test_counts
        self.commit({"crates/alpha/src/net.rs": "pub fn net2() {}\n",
                     BROWSER_UNIT: "#[test]\nfn draws() { /* lexer lost */ }\n"})
        gate_plan._BROWSER_CACHE.clear()
        full = [{"name": "tests", "select": {"filter": "rdeps(beta)", "rdeps": ["beta"]},
                 "cmd": ["scripts/test-tier.sh", "filter", "rdeps(beta)"]}]
        with mock.patch.object(gate_plan, "_test_counts",
                               side_effect=lambda t: (0, 0) if "lexer lost" in t else real(t)), \
                mock.patch.object(narrow, "plan_browser_orphans", return_value=[]):
            with self.assertRaises(narrow.UnrunSubtraction) as cm:
                self.executed([BETA_SRC], out=[])
            self.assertIn(BROWSER_TERM, str(cm.exception))
            with self.assertRaises(narrow.UnrunSubtraction) as cm:
                narrow.full_suite(full, self.root, ["no coverage map"], env={})
            self.assertIn(BROWSER_TERM, str(cm.exception))

    def test_a_unit_the_station_plans_stays_subtracted(self):
        """The other half: a unit `browser:display` runs leaves `tests` (M-3)."""
        self.commit({"crates/alpha/src/net.rs": "pub fn net2() {}\n"})
        station = {"name": "browser:display",
                   "cmds": gate_plan.browser_cmds(False, moved=[BROWSER_TERM])}
        sel, filt, notes = self.executed([BETA_SRC], extra_rows=[station])
        self.assertIn(BROWSER_TERM, filt.partition(" - ")[2], (filt, notes))
        self.assertEqual([], narrow.browser_orphans(
            [{"name": "tests", "select": sel}, station], self.root))


# --- the scope over folded harnesses (welle-val val-narrow, GH #1203, #1202) -----
# Tempo-Beweis measured two plans far wider than their diff: one assert in
# `meclaw-core::misc` planned 10 411 tests (M-ROT-1-W, `tests` 784 s instead of
# ~17 s), one line of the development rules page the suite (M-STR-doku, `tests`
# 719 s instead of ~41 s). Both receipts name the same mechanism: the graph hangs
# its `reads` edges on the UNIT address of a folded module,
# `bin:<crate>::<harness>/<stem>` (`graph._unit_user_addrs`, the `reads` loop over
# `gate_plan.test_sources`), and `impact.checked_graph_terms` took that address
# for a binary -- no node of its own, so "gone from the tree", no folded form for
# a stem with a `/` -- and planned rdeps(<crate>) for a unit the tree holds.
#
# These locks plan over a small fixed tree with folded harnesses and the graph
# `graph.build` makes of it -- never over the machine's graph or map -- and read
# the executed nextest filter with an oracle of their own:
#
#     test touch   one unit of a harness -> that harness binary plus the units
#                  that read the file; no rdeps, no all(), no other binary
#     docs diff    a page in both language versions -> its readers only; no
#                  rdeps, no all(), and no cargo station but the ones its
#                  readers need
#     stale graph  a reader moved to another harness after the graph build ->
#                  its new unit, not rdeps; a reader deleted from the tree ->
#                  rdeps(<crate>) stays (fail-closed, welle-tb S1 part 0)
SCOPE_TOUCHED = "crates/alpha/tests/h1/touched.rs"
SCOPE_PAGES = ["docs/page.en.md", "docs/page.md"]

SCOPE_FILES = {
    "Cargo.toml": '[workspace]\nmembers = ["crates/alpha", "crates/beta"]\n',
    "crates/alpha/Cargo.toml": '[package]\nname = "alpha"\nversion = "0.1.0"\n',
    "crates/alpha/src/lib.rs": "pub fn a() {}\n",
    "crates/beta/Cargo.toml": '[package]\nname = "beta"\nversion = "0.1.0"\n\n'
                              '[dev-dependencies]\nalpha = { path = "../alpha" }\n',
    "crates/beta/src/lib.rs": "pub fn b() {}\n",
    # alpha::h1 -- the touched unit and a sibling
    "crates/alpha/tests/h1/main.rs": "mod sibling;\nmod touched;\n",
    SCOPE_TOUCHED: "#[test]\nfn t() {}\n",
    "crates/alpha/tests/h1/sibling.rs": "#[test]\nfn s() {}\n",
    # alpha::h2 -- one unit reads the page
    "crates/alpha/tests/h2/main.rs": "mod quiet;\nmod reads_page;\n",
    "crates/alpha/tests/h2/reads_page.rs":
        '#[test]\nfn r() {\n    let _ = std::fs::read_to_string("docs/page.md");\n}\n',
    "crates/alpha/tests/h2/quiet.rs": "#[test]\nfn q() {}\n",
    # beta::k1 -- one unit reads the touched test file
    "crates/beta/tests/k1/main.rs": "mod names_touched;\nmod other;\n",
    "crates/beta/tests/k1/names_touched.rs":
        '#[test]\nfn n() {\n    let _ = std::fs::read_to_string("%s");\n}\n' % SCOPE_TOUCHED,
    "crates/beta/tests/k1/other.rs": "#[test]\nfn o() {}\n",
    # beta::k2 -- reads nothing of either diff
    "crates/beta/tests/k2/main.rs": "mod unrelated;\n",
    "crates/beta/tests/k2/unrelated.rs": "#[test]\nfn u() {}\n",
    "docs/page.md": "# Page\n",
    "docs/page.en.md": "# Page\n",
}

SCOPE_TOUCHED_UNIT = "(binary_id(=alpha::h1) & test(/^touched::/))"
SCOPE_TOUCH_READER = "(binary_id(=beta::k1) & test(/^names_touched::/))"
SCOPE_PAGE_READER = "(binary_id(=alpha::h2) & test(/^reads_page::/))"


def binary_of(term):
    """`<crate>::<binary>` of a `binary_id` term or a folded part term, else None."""
    m = re.match(r"\(?binary_id\(=([^)]+)\)", term)
    return m.group(1) if m else None


class NarrowScopeBase(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = self.tmp.name
        self.write(SCOPE_FILES)
        self.doc = self.build_graph()

    def tearDown(self):
        self.clear()
        self.tmp.cleanup()

    @staticmethod
    def clear():
        gate_plan._SOURCE_CACHE.clear()
        gate_plan._NO_TEST_CACHE.clear()

    def write(self, files):
        for rel, text in files.items():
            path = pathlib.Path(self.root, rel)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text, encoding="utf-8")
        self.clear()

    def build_graph(self):
        self.clear()
        g, _ = graph_mod.build(self.root, stations=False)
        return g.to_json("fixture", "2026-10-14T00:00:00Z")

    def plan(self, paths):
        """`(terms, notes)` of the strand plan as the gate narrows it with no
        coverage map: floor + graph (the case of both measured receipts)."""
        self.clear()
        floor = gate_plan.test_select(paths, "strand", self.root)
        sel, notes = narrow.narrow(floor, paths, self.root, self.doc, None, "no coverage map")
        return filter_terms((sel or {}).get("filter")), notes

    def assertNoSuite(self, terms, notes):  # noqa: N802 -- unittest's style
        wide = sorted(t for t in terms if t == "all()" or t.startswith("rdeps("))
        self.assertEqual([], wide, notes)


class ScopeTestTouchTest(NarrowScopeBase):
    def test_a_test_touch_plans_its_harness_and_its_readers_only(self):
        terms, notes = self.plan([SCOPE_TOUCHED])
        self.assertNoSuite(terms, notes)
        self.assertIn(SCOPE_TOUCHED_UNIT, terms, notes)
        self.assertIn(SCOPE_TOUCH_READER, terms, notes)          # the `read:` reader runs
        self.assertLessEqual({binary_of(t) for t in terms}, {"alpha::h1", "beta::k1"},
                             (terms, notes))

    def test_a_test_touch_names_no_unit_of_the_tree_gone(self):
        _terms, notes = self.plan([SCOPE_TOUCHED])
        self.assertEqual([], [n for n in notes if "gone from the tree" in n])


class ScopeDocsDiffTest(NarrowScopeBase):
    def test_a_docs_diff_plans_its_readers_only(self):
        terms, notes = self.plan(SCOPE_PAGES)
        self.assertNoSuite(terms, notes)
        self.assertEqual({SCOPE_PAGE_READER}, terms, notes)

    def test_a_docs_diff_plans_no_cargo_station_its_readers_do_not_need(self):
        # `slow-tests` reads the times of the `tests` run, planned with it (GH #1046)
        self.clear()
        cargo = {st.name for st in gate_plan.plan(SCOPE_PAGES, "strand", self.root) if st.cargo}
        self.assertLessEqual(cargo, {"build", "tests", "slow-tests"}, cargo)
        self.clear()
        reader_free = ["docs/unread.md"]
        self.write({reader_free[0]: "# Nobody reads me\n"})
        cargo = {st.name for st in gate_plan.plan(reader_free, "strand", self.root) if st.cargo}
        self.assertEqual(set(), cargo)


class ScopeStaleGraphTest(NarrowScopeBase):
    def test_a_reader_moved_to_another_harness_plans_its_new_unit(self):
        os.remove(os.path.join(self.root, "crates/alpha/tests/h2/reads_page.rs"))
        self.write({"crates/alpha/tests/h2/main.rs": "mod quiet;\n",
                    "crates/alpha/tests/h3/main.rs": "mod reads_page;\n",
                    "crates/alpha/tests/h3/reads_page.rs":
                        SCOPE_FILES["crates/alpha/tests/h2/reads_page.rs"]})
        terms, notes = self.plan(SCOPE_PAGES)
        self.assertNoSuite(terms, notes)
        self.assertEqual({"(binary_id(=alpha::h3) & test(/^reads_page::/))"}, terms, notes)

    def test_a_reader_gone_from_the_tree_keeps_its_crate_rdeps(self):
        """The graph's reader is in no harness any more -- renamed or deleted,
        the plan cannot tell: rdeps of its crate (fail-closed). A reader the
        tree gained since the graph keeps the floor, and so the narrowing, on."""
        os.remove(os.path.join(self.root, "crates/alpha/tests/h2/reads_page.rs"))
        self.write({"crates/alpha/tests/h2/main.rs": "mod quiet;\nmod renamed_reader;\n",
                    "crates/alpha/tests/h2/renamed_reader.rs":
                        SCOPE_FILES["crates/alpha/tests/h2/reads_page.rs"]})
        terms, notes = self.plan(SCOPE_PAGES)
        self.assertIn("(binary_id(=alpha::h2) & test(/^renamed_reader::/))", terms, notes)
        self.assertIn("rdeps(alpha)", terms, notes)


# --- R-1 (welle-validierung-2, GH #1199): a unit whose file stays but no longer folds --
# `impact.checked_graph_terms` held the graph's unit `bin:alpha::h2/reads_page` for
# present when its FILE stood (`gate_plan._test_binary_exists`); a harness that
# no longer folds the module -- orphaned, `#[cfg(any())]`-gated, `#[path]`-renamed
# -- then planned `(binary_id(=alpha::h2) & test(/^reads_page::/))`, a term that
# selects 0 tests in silence. The unit counts only when its harness folds it the
# way `impact.folded_form` reads the tree; otherwise it is lost: its folded form
# elsewhere, else rdeps(alpha), with a note.
UNFOLDED_SHAPES = {
    "orphaned": "mod quiet;\n",
    "cfg": "mod quiet;\n#[cfg(any())]\nmod reads_page;\n",
    "path": 'mod quiet;\n#[path = "reads_page.rs"]\nmod renamed;\n',
}


class ScopeUnfoldedUnitTest(NarrowScopeBase):
    def assert_lost(self, shape):
        self.write({"crates/alpha/tests/h2/main.rs": UNFOLDED_SHAPES[shape]})
        terms, notes = self.plan(SCOPE_PAGES)
        self.assertNotIn(SCOPE_PAGE_READER, terms, (terms, notes))
        self.assertIn("rdeps(alpha)", terms, (terms, notes))
        self.assertTrue([n for n in notes if "alpha::h2/reads_page" in n], notes)

    def test_r1_an_orphaned_unit_file_plans_no_zero_test_term(self):
        self.assert_lost("orphaned")

    def test_r1_a_cfg_gated_unit_plans_no_zero_test_term(self):
        self.assert_lost("cfg")

    def test_r1_a_path_renamed_unit_plans_no_zero_test_term(self):
        self.assert_lost("path")

    def test_r1_a_unit_the_harness_still_folds_keeps_its_term(self):
        terms, notes = self.plan(SCOPE_PAGES)
        self.assertEqual({SCOPE_PAGE_READER}, terms, notes)
        self.assertEqual([], [n for n in notes if "alpha::h2/reads_page" in n], notes)



# --- 3a (welle-validierung-2, GH #1199): the narrowing builds the graph of the head --
# Part 1 narrowed on a graph 807 commits behind the head: only `pipeline graph
# build` (by hand) built it, `impact` only noted the rev. `narrow.apply` now asks
# `graph.fresh`: a latest build of another rev is rebuilt for the head, a build of
# the head is read, a failed build refuses the plan.
def _git_fx(root, *args):
    env = dict(os.environ, GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@t", GIT_COMMITTER_NAME="t",
               GIT_COMMITTER_EMAIL="t@t", GIT_CONFIG_GLOBAL=os.devnull,
               GIT_CONFIG_SYSTEM=os.devnull)
    for k in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"):
        env.pop(k, None)
    return subprocess.run(["git", "-C", str(root), *args], check=True, capture_output=True,
                          text=True, env=env).stdout.strip()


class GraphFreshTest(NarrowScopeBase):
    def setUp(self):
        super().setUp()
        self.gdir = pathlib.Path(self.root + "-graph")
        self.addCleanup(lambda: __import__("shutil").rmtree(self.gdir, ignore_errors=True))
        _git_fx(self.root, "init", "-q", "-b", "master")
        _git_fx(self.root, "add", "-A")
        _git_fx(self.root, "commit", "-q", "-m", "n")
        self.rev_n = _git_fx(self.root, "rev-parse", "HEAD")
        self.env = mock.patch.dict(os.environ, {"MECLAW_PIPELINE_GRAPH_DIR": str(self.gdir),
                                                "MECLAW_PIPELINE_GRAPH_AUTO": "1",
                                                "MECLAW_GATE_NARROW": "1"})
        self.env.start()
        self.addCleanup(self.env.stop)
        self.clear()
        g, _ = graph_mod.build(self.root, stations=False)
        graph_mod.save(g, self.rev_n, self.gdir)

    def commit_n1(self):
        self.write({"docs/page.md": "# Page, changed\n"})
        _git_fx(self.root, "commit", "-q", "-am", "n+1")
        return _git_fx(self.root, "rev-parse", "HEAD")

    def plan_doc(self):
        self.clear()
        return plan.build_plan(self.root, "strand", None, files=list(SCOPE_PAGES), narrow=True)

    @staticmethod
    def graph_built_rows():
        db = store.open_ro()
        try:
            rows = db.execute("SELECT data FROM journal WHERE event='graph_built' "
                              "ORDER BY seq").fetchall()
        finally:
            store.close(db)
        return [json.loads(r[0]) for r in rows]

    def test_3a_a_graph_behind_the_head_is_built_for_the_head(self):
        rev = self.commit_n1()
        before = len(self.graph_built_rows())
        doc = self.plan_doc()
        self.assertEqual(rev, (doc.get("graph_build") or {}).get("rev"), doc.get("notes"))
        self.assertEqual("auto", doc["graph_build"]["reason"])
        self.assertTrue(any("graph of %s built (auto" % rev[:12] in n
                            for n in doc.get("notes") or []), doc.get("notes"))
        self.assertEqual(rev, graph_mod.load()["rev"])
        self.assertTrue((self.gdir / ("%s.json" % rev)).is_file())
        self.assertEqual([("auto", rev)], [(r["reason"], r["rev"])
                                           for r in self.graph_built_rows()[before:]])

    def test_3a_a_graph_of_the_head_is_read_not_built(self):
        doc = self.plan_doc()
        self.assertNotIn("graph_build", doc, doc.get("notes"))
        with mock.patch.object(graph_mod, "build", side_effect=AssertionError("built")):
            doc = self.plan_doc()
        self.assertNotIn("graph_build", doc)

    def test_3a_the_second_gate_of_a_rev_reads_the_first_ones_build(self):
        rev = self.commit_n1()
        self.clear()
        g, _ = graph_mod.build(self.root, stations=False)
        doc = g.to_json(rev, "2026-10-14T00:00:00Z")
        (self.gdir / ("%s.json" % rev)).write_text(json.dumps(doc), encoding="utf-8")
        with mock.patch.object(graph_mod, "build", side_effect=AssertionError("built twice")):
            out = self.plan_doc()
        self.assertNotIn("graph_build", out)

    def test_3a_a_failed_build_refuses_the_plan(self):
        self.commit_n1()
        with mock.patch.object(graph_mod, "build", side_effect=OSError("disk gone")):
            with self.assertRaisesRegex(graph_mod.GraphBuildError, "disk gone"):
                self.plan_doc()

    def test_3a_a_dirty_tree_is_built_and_not_stored(self):
        rev = self.commit_n1()
        self.write({"docs/page.md": "# Page, dirty\n"})
        doc = self.plan_doc()
        self.assertEqual(rev, doc["graph_build"]["rev"])
        self.assertFalse(doc["graph_build"]["stored"])
        self.assertFalse((self.gdir / ("%s.json" % rev)).exists())

    def test_3a_an_untracked_report_leaves_the_rev_clean_built_once_and_read(self):
        """Re-check v2-kit c23 I-1: an untracked wave report is nothing the graph
        reads -- the first plan builds the rev under its lock and stores it, the
        second reads it and builds nothing."""
        rev = self.commit_n1()
        self.write({"plans/welle-x/berichte/s.md": "# a report of the strand\n",
                    "plans/welle-x/receipts/s/run/test.log": "PASS 1\n"})
        doc = self.plan_doc()
        self.assertEqual(rev, doc["graph_build"]["rev"], doc.get("notes"))
        self.assertTrue(doc["graph_build"]["stored"], doc.get("notes"))
        self.assertTrue((self.gdir / ("%s.json" % rev)).is_file())
        self.assertTrue((self.gdir / ("%s.lock" % rev)).is_file())   # built under the lock
        with mock.patch.object(graph_mod, "build", side_effect=AssertionError("built twice")):
            again = self.plan_doc()
        self.assertNotIn("graph_build", again, again.get("notes"))
        self.assertEqual([], graph_mod.graph_dirt(self.root))

    def test_3a_an_untracked_test_source_is_dirt_never_the_head_graph(self):
        """I-1: a new `tests/*.rs` is read by the graph -- the plan builds the
        working tree, stores nothing, and the stored graph of the head (which
        lacks the binary) is never its answer."""
        stored = sorted(p.name for p in self.gdir.iterdir())
        self.write({"crates/alpha/tests/fresh_unit.rs": "#[test]\nfn fresh() {}\n"})
        doc, built = graph_mod.fresh(self.root)
        self.assertIsNotNone(built, "the stored graph of the head answered a dirty tree")
        self.assertFalse(built["stored"])
        self.assertEqual(["crates/alpha/tests/fresh_unit.rs"], built["dirt"])
        self.assertIn("bin:alpha::fresh_unit", {n["addr"] for n in doc["nodes"]})
        self.assertEqual(stored, sorted(p.name for p in self.gdir.iterdir()
                                        if p.name != graph_mod.CACHE_FILE))
        note = self.plan_doc().get("notes") or []
        self.assertTrue(any("dirty tree (crates/alpha/tests/fresh_unit.rs): not stored" in n
                            for n in note), note)
        self.assertEqual(["crates/alpha/tests/fresh_unit.rs"], graph_mod.graph_dirt(self.root))

    def test_3a_an_untracked_report_a_test_names_is_dirt(self):
        """I-1, the other half of the rule: a file below plans/ that a test
        source names is read by the graph (a `reads` edge) -- dirt."""
        self.write({"crates/alpha/tests/names_it.rs":
                    '#[test]\nfn reads() { let _ = "plans/welle-x/fixture.txt"; }\n'})
        _git_fx(self.root, "add", "-A")
        _git_fx(self.root, "commit", "-q", "-m", "reader")
        self.write({"plans/welle-x/fixture.txt": "x\n", "plans/welle-x/berichte/s.md": "r\n"})
        self.assertEqual(["plans/welle-x/fixture.txt"], graph_mod.graph_dirt(self.root))

    def test_3a_a_tracked_change_below_plans_is_dirt(self):
        self.write({"plans/README.md": "# changed\n"})
        _git_fx(self.root, "add", "plans/README.md")
        _git_fx(self.root, "commit", "-q", "-m", "plans")
        self.write({"plans/README.md": "# changed again\n"})
        self.assertEqual(["plans/README.md"], graph_mod.graph_dirt(self.root))


class GraphDirPruneTest(unittest.TestCase):
    """Re-check v2-kit c23 M-1: saving a graph removes the files of every rev
    but the newest `GRAPH_KEEP_REVS`, never the rev saved, never the rev
    `latest.json` named before, never a rev whose lock is held."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = pathlib.Path(self.tmp.name)

    def rev_files(self, n):
        revs = ["%040x" % (i + 1) for i in range(n)]
        for i, rev in enumerate(revs):   # rev i is the i-th oldest
            for ext in ("json", "lock"):
                path = self.dir / ("%s.%s" % (rev, ext))
                path.write_text("{}", encoding="utf-8")
                os.utime(path, ns=(10 ** 9 * (i + 1), 10 ** 9 * (i + 1)))
        return revs

    def test_save_keeps_the_newest_revs_the_saved_and_the_one_read(self):
        keep = graph_mod.GRAPH_KEEP_REVS
        revs = self.rev_files(keep + 3)
        oldest = revs[0]
        (self.dir / "latest.json").write_text(json.dumps(
            {"rev": oldest, "path": str(self.dir / ("%s.json" % oldest))}), encoding="utf-8")
        new = "f" * 40
        graph_mod.save(graph_mod.Graph(), new, self.dir)
        left = {p.name.split(".")[0] for p in self.dir.iterdir() if p.name[0] in "0f"}
        self.assertEqual({new, oldest} | set(revs[-(keep - 1):]), left)
        self.assertEqual(new, json.loads((self.dir / "latest.json").read_text())["rev"])

    def test_a_held_lock_keeps_its_rev(self):
        import fcntl
        keep = graph_mod.GRAPH_KEEP_REVS
        revs = self.rev_files(keep + 2)
        with open(self.dir / ("%s.lock" % revs[1]), "a", encoding="utf-8") as held:
            fcntl.flock(held, fcntl.LOCK_EX)
            removed = graph_mod._prune_graph_dir(self.dir)
        names = {p.name for p in removed}
        self.assertNotIn("%s.lock" % revs[1], names)
        self.assertTrue((self.dir / ("%s.json" % revs[1])).exists())
        self.assertEqual({"%s.json" % revs[0], "%s.lock" % revs[0]}, names)


if __name__ == "__main__":
    unittest.main()
