"""Tests of the impact selection (`scripts/pipeline/impact.py`, W6, GH #1120).

The workspace is the synthetic one of `test_pipeline_graph` (two crates, three
templates, one scaffold, two cases). The replay fixtures are five mini trees in
the shapes of real red diffs: a lib change caught by an integration test, a
lib change caught by a unit test of the lib, a template change caught by a
test that references the template, a docs change caught by a test that reads
the page, and a shared test helper caught by a binary that compiles it in.
"""

import copy
import datetime
import os
import pathlib
import sys
import tempfile
import unittest
from unittest import mock

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS))
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import gate_plan  # noqa: E402
from pipeline import crate_root, graph, impact  # noqa: E402
from scripts.tests import store_isolation  # noqa: E402
from test_pipeline_graph import FILES, make_tree  # noqa: E402

NOW = datetime.datetime(2026, 10, 8, 12, 0, tzinfo=datetime.timezone.utc)
COVERAGE = {
    "schema": "meclaw.pipeline.coverage/1", "rev": "fixture", "built_at": "2026-10-08T02:00:00Z",
    "runner": "lane", "granularity": "module", "secs": 10.0, "tests": 4,
    "complete": True, "binaries_expected": 3, "binaries_measured": 3, "errors": 0,
    "parts": [
        {"binary": "alpha", "module": "net", "files": {"crates/alpha/src/net.rs": 3,
                                                       "crates/alpha/src/store.rs": 1}},
        {"binary": "alpha", "module": "tests", "files": {"crates/alpha/src/lib.rs": 1}},
        {"binary": "alpha::a1_reads_doc", "module": None,
         "files": {"crates/alpha/src/net.rs": 2}},
        {"binary": "beta::b1_uses_alpha", "module": None,
         "files": {"crates/alpha/src/store.rs": 1}},
    ],
}
REASON_PREFIXES = ("rule:", "graph:", "cover:", "new:", "stage:")


# gate-selftest loads every module into one process: no fixture event of this
# module may reach the metrics target of the caller's environment.
_METRICS_URL = mock.patch.dict(os.environ, {"MECLAW_PIPELINE_METRICS_URL": ""})


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()
    _METRICS_URL.start()


def tearDownModule():  # noqa: N802 -- unittest's name
    _METRICS_URL.stop()
    store_isolation.restore()


class Base(unittest.TestCase):
    files = FILES

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = self.tmp.name
        make_tree(self.root, self.files)
        gate_plan._SOURCE_CACHE.clear()
        gate_plan._NO_TEST_CACHE.clear()
        g, _ = graph.build(self.root, stations=False)
        self.doc = g.to_json("fixture", "2026-10-08T00:00:00Z")

    def tearDown(self):
        gate_plan._SOURCE_CACHE.clear()
        gate_plan._NO_TEST_CACHE.clear()
        self.tmp.cleanup()

    def run_impact(self, paths, stage="strand", coverage=COVERAGE):
        return impact.impact(paths, stage, self.doc, repo=self.root, coverage=coverage, now=NOW)


class ImpactTest(Base):
    def test_impact_strand_never_below_resolver_floor_for_new_files(self):
        new = {"crates/alpha/src/fresh.rs": "pub fn fresh() {}\n",
               "crates/alpha/tests/a3_new.rs": "#[test]\nfn n() {}\n"}
        make_tree(self.root, new)
        gate_plan._SOURCE_CACHE.clear()
        res = self.run_impact(sorted(new))
        floor = gate_plan.test_terms(sorted(new), self.root)
        self.assertTrue(set(floor) <= set(res["tests"]), (floor, res["tests"]))
        self.assertIn("rdeps(alpha)", res["tests"])
        self.assertIn("new:file:crates/alpha/src/fresh.rs", res["reason"]["rdeps(alpha)"])
        self.assertIn("binary_id(=alpha::a3_new)", res["tests"])
        self.assertIn("new:file:crates/alpha/tests/a3_new.rs",
                      res["reason"]["binary_id(=alpha::a3_new)"])

    def test_a_support_helper_keeps_its_crate_rdeps_with_a_complete_map(self):
        """Re-check tb-s1c L-6 (`support/warm_python.rs`): a diff only at a
        `tests/support/` helper the complete map knows from ONE binary keeps
        rdeps(<crate>) -- the map sees the helper's lines only where a test
        executed them (gh944), its other includers (curator, file_space) not.
        Only the `own_unit_term is None` branch of `impact` holds them."""
        helper = "crates/alpha/tests/support/warm.rs"
        make_tree(self.root, {helper: "pub fn warm() {}\n"})
        gate_plan._SOURCE_CACHE.clear()
        cov = copy.deepcopy(COVERAGE)
        cov["parts"][2]["files"][helper] = 1
        res = self.run_impact([helper], coverage=cov)
        self.assertIn("rdeps(alpha)", res["tests"])
        self.assertIn("cover:no-lib-module file:%s" % helper, res["reason"]["rdeps(alpha)"])

    def test_impact_strand_uses_coverage_when_all_files_known(self):
        res = self.run_impact(["crates/alpha/src/store.rs"])
        self.assertNotIn("rdeps(alpha)", res["tests"])
        self.assertTrue(res["coverage"]["used"])
        net = impact.part_term("alpha", "net")
        self.assertIn(net, res["tests"])
        self.assertIn("cover:file:crates/alpha/src/store.rs", res["reason"][net])
        self.assertIn("binary_id(=beta::b1_uses_alpha)", res["tests"])
        self.assertLess(res["binaries"]["selected"], res["binaries"]["total"])
        # the floor alone would have run every binary of alpha and beta
        floor = self.run_impact(["crates/alpha/src/store.rs"], coverage=None)
        self.assertIn("rdeps(alpha)", floor["tests"])

    def test_impact_strand_falls_back_to_rdeps_when_file_unknown(self):
        partial = dict(COVERAGE, parts=[p for p in COVERAGE["parts"]
                                        if "crates/alpha/src/store.rs" not in p["files"]])
        res = self.run_impact(["crates/alpha/src/store.rs"], coverage=partial)
        self.assertIn("rdeps(alpha)", res["tests"])
        self.assertIn("cover:unknown file:crates/alpha/src/store.rs", res["reason"]["rdeps(alpha)"])
        stale = dict(COVERAGE, built_at="2026-09-30T00:00:00Z")
        res = self.run_impact(["crates/alpha/src/store.rs"], coverage=stale)
        self.assertIn("rdeps(alpha)", res["tests"])
        self.assertTrue(res["coverage"]["stale"])

    def test_impact_strand_partial_map_falls_back_to_rdeps(self):
        """C1: a map nobody proved complete is no map -- a binary it never
        measured would otherwise be silently missing from the plan."""
        for partial in (dict(COVERAGE, complete=False),
                        {k: v for k, v in COVERAGE.items() if k != "complete"}):
            res = self.run_impact(["crates/alpha/src/store.rs"], coverage=partial)
            self.assertIn("rdeps(alpha)", res["tests"])
            self.assertFalse(res["coverage"]["used"])
            self.assertFalse(res["coverage"]["complete"])

    def test_impact_strand_map_of_another_rev_falls_back_to_rdeps(self):
        other = dict(COVERAGE, rev="0ther")
        res = self.run_impact(["crates/alpha/src/store.rs"], coverage=other)
        self.assertIn("rdeps(alpha)", res["tests"])
        self.assertFalse(res["coverage"]["used"])
        self.assertFalse(res["coverage"]["rev_match"])
        # the caller names the base of the diff: then that rev must match
        res = impact.impact(["crates/alpha/src/store.rs"], "strand", self.doc, repo=self.root,
                            coverage=other, now=NOW, rev="0ther")
        self.assertTrue(res["coverage"]["used"])

    def test_impact_merge_integration_ignore_coverage_except_note(self):
        for stage in ("merge", "integration"):
            res = self.run_impact(["crates/alpha/src/store.rs"], stage=stage)
            self.assertIn("rdeps(alpha)", res["tests"], stage)
            self.assertFalse(res["coverage"]["used"])
            self.assertTrue(any("would have spared" in n for n in res["notes"]), res["notes"])

    def test_impact_without_a_built_graph_says_so(self):
        """A host that never ran `graph build` gets a one-line answer and
        exit 2, not a traceback: `plan.py` reads the exit code."""
        import os
        import subprocess
        empty = pathlib.Path(self.tmp.name) / "no-graph"
        env = dict(os.environ, MECLAW_PIPELINE_GRAPH_DIR=str(empty))
        r = subprocess.run([sys.executable, str(SCRIPTS / "pipeline.py"), "impact", "--diff", "-"],
                           input="docs/guide.md\n", env=env, capture_output=True, text=True)
        self.assertEqual(r.returncode, 2, r.stderr)
        self.assertNotIn("Traceback", r.stderr)
        self.assertIn("graph build", r.stderr)

    def test_impact_graph_additions_skip_a_deleted_test_binary(self):
        # review I3: the graph still names a1_reads_doc, the tree no longer
        # has it -- nextest would reject the binary_id and take the station down.
        os.remove(os.path.join(self.root, "crates/alpha/tests/a1_reads_doc.rs"))
        gate_plan._SOURCE_CACHE.clear()
        res = self.run_impact(["docs/guide.md"], coverage=None)
        self.assertNotIn("binary_id(=alpha::a1_reads_doc)", res["tests"])
        self.assertTrue(any("a1_reads_doc" in n for n in res["notes"]), res["notes"])

    def test_impact_graph_additions_follow_rule_9(self):
        # review I3 / #997: a binary whose tests are all #[ignore] runs none.
        make_tree(self.root, {"crates/beta/tests/b2_ignored.rs":
                              '#[test]\n#[ignore]\nfn slow() {\n    let _ = "docs/guide.md";\n}\n'})
        gate_plan._SOURCE_CACHE.clear()
        g, _ = graph.build(self.root, stations=False)
        self.doc = g.to_json("fixture", "2026-10-08T00:00:00Z")
        self.assertIn("bin:beta::b2_ignored", graph.Index(self.doc).nodes)
        res = self.run_impact(["docs/guide.md"], coverage=None)
        self.assertNotIn("binary_id(=beta::b2_ignored)", res["tests"])
        self.assertIn("binary_id(=alpha::a1_reads_doc)", res["tests"])

    def test_impact_notes_a_graph_of_another_rev(self):
        res = impact.impact(["docs/guide.md"], "strand", self.doc, repo=self.root,
                            coverage=None, now=NOW, tree_rev="abc123")
        self.assertIn("graph rev fixture is not HEAD abc123", res["notes"])
        res = impact.impact(["docs/guide.md"], "strand", self.doc, repo=self.root,
                            coverage=None, now=NOW, tree_rev="fixture")
        self.assertFalse(any("graph rev" in n for n in res["notes"]), res["notes"])

    def test_graph_load_refuses_a_latest_pointer_of_another_rev(self):
        gdir = pathlib.Path(self.tmp.name) / "graphs"
        path = graph.save(graph.build(self.root, stations=False)[0], "r1", gdir)
        (gdir / "latest.json").write_text('{"rev": "r2", "path": "%s"}' % path)
        with mock.patch.dict(os.environ, {"MECLAW_PIPELINE_GRAPH_DIR": str(gdir)}):
            with self.assertRaisesRegex(ValueError, "r2"):
                graph.load()

    def test_impact_with_a_broken_graph_says_so(self):
        """review M4: a broken or mismatched graph file is one line and exit 2."""
        import subprocess
        gdir = pathlib.Path(self.tmp.name) / "broken"
        gdir.mkdir()
        good = graph.save(graph.build(self.root, stations=False)[0], "r1", gdir)
        cases = {"truncated": "{\"rev\": ", "no path": '{"rev": "r1"}',
                 "other rev": '{"rev": "r2", "path": "%s"}' % good}
        for name, latest in cases.items():
            with self.subTest(name):
                (gdir / "latest.json").write_text(latest)
                env = dict(os.environ, MECLAW_PIPELINE_GRAPH_DIR=str(gdir))
                r = subprocess.run([sys.executable, str(SCRIPTS / "pipeline.py"), "impact",
                                    "--diff", "-"], input="docs/guide.md\n", env=env,
                                   capture_output=True, text=True)
                self.assertEqual(r.returncode, 2, r.stderr)
                self.assertNotIn("Traceback", r.stderr)
                self.assertEqual(len(r.stderr.strip().splitlines()), 1, r.stderr)
                self.assertIn("graph build", r.stderr)

    def test_impact_with_a_broken_coverage_map_says_so(self):
        """review minor: a broken coverage file is one line and exit 2, no traceback."""
        import subprocess
        gdir = pathlib.Path(self.tmp.name) / "cov-broken"
        gdir.mkdir()
        graph.save(graph.build(self.root, stations=False)[0], "r1", gdir)
        bad = gdir / "coverage-r1.json"
        bad.write_text('{"rev": ')
        env = dict(os.environ, MECLAW_PIPELINE_GRAPH_DIR=str(gdir))
        r = subprocess.run([sys.executable, str(SCRIPTS / "pipeline.py"), "impact",
                            "--diff", "-", "--repo", str(self.root), "--coverage", str(bad)],
                           input="docs/guide.md\n", env=env, capture_output=True, text=True)
        self.assertEqual(r.returncode, 2, r.stderr)
        self.assertNotIn("Traceback", r.stderr)
        self.assertIn("broken coverage map", r.stderr)

    def test_impact_release_is_everything(self):
        res = self.run_impact(["docs/guide.md"], stage="release", coverage=None)
        self.assertEqual(res["tests"], ["all()"])
        self.assertEqual(res["binaries"]["selected"], res["binaries"]["total"])
        self.assertEqual(res["reason"]["all()"], ["stage:release"])

    def test_impact_reasons_name_rule_or_edge(self):
        diffs = [["crates/alpha/src/store.rs"], ["docs/guide.md"],
                 ["templates/talky/config.json"], ["crates/beta/tests/b1_uses_alpha.rs"]]
        for diff in diffs:
            res = self.run_impact(diff)
            self.assertTrue(res["tests"], diff)
            for t in res["tests"]:
                self.assertTrue(res["reason"].get(t), (diff, t))
                for r in res["reason"][t]:
                    self.assertTrue(r.startswith(REASON_PREFIXES), (diff, t, r))
            for st in res["stations"]:
                self.assertTrue(res["reason"].get(st), (diff, st))
        docs = self.run_impact(["docs/guide.md"])
        self.assertIn("rule:8 file:docs/guide.md",
                      docs["reason"]["binary_id(=alpha::a1_reads_doc)"])
        self.assertIn("graph:reads file:docs/guide.md",
                      docs["reason"]["binary_id(=alpha::a1_reads_doc)"])


# --- the replay fixtures -----------------------------------------------------

def _cells(extra):
    base = {
        "Cargo.toml": '[workspace]\nmembers = ["crates/cells", "crates/cli"]\n',
        "crates/cells/Cargo.toml": '[package]\nname = "cells"\nversion = "0.1.0"\n',
        "crates/cells/src/lib.rs": "pub mod web;\npub mod wire;\n",
        "crates/cells/src/web.rs": "pub fn serve() {}\n\n#[cfg(test)]\nmod tests {\n"
                                   "    #[test]\n    fn backlog_ratio() {}\n}\n",
        "crates/cells/src/wire.rs": "pub fn send() {}\n",
        "crates/cells/tests/gh1_delivery.rs": "use cells::wire::send;\n\n#[test]\nfn d() {}\n",
        "crates/cells/tests/gh2_viewer.rs": "use cells::web::serve;\n\n#[test]\nfn v() {}\n",
        "crates/cells/tests/gh3_readme.rs": '#[test]\nfn r() {\n    let _ = "README.md";\n}\n',
        "crates/cells/tests/gh4_shape.rs": '#[test]\nfn s() {\n'
                                           '    let _ = "templates/curator/config.json";\n}\n',
        "crates/cells/tests/mock_peer.rs": "pub fn peer() {}\n\n#[test]\nfn own() {}\n",
        "crates/cells/tests/gh5_peers.rs": "mod mock_peer;\n\n#[test]\nfn p() {}\n",
        "crates/cli/Cargo.toml": '[package]\nname = "cli"\nversion = "0.1.0"\n\n'
                                 '[dependencies]\ncells = { path = "../cells" }\n',
        "crates/cli/src/lib.rs": "pub fn run() {}\n",
        "crates/cli/tests/c1_matrix.rs": "#[test]\nfn m() {}\n",
        "templates/curator/template.json": '{"name": "curator", "version": "1.0.0"}\n',
        "templates/curator/config.json": '{"type": "llm"}\n',
        "README.md": "# cells\n",
    }
    base.update(extra)
    return base


REPLAY_COVERAGE = dict(COVERAGE, parts=[
    {"binary": "cells", "module": "web", "files": {"crates/cells/src/web.rs": 4}},
    {"binary": "cells::gh1_delivery", "module": None, "files": {"crates/cells/src/wire.rs": 2}},
    {"binary": "cells::gh2_viewer", "module": None, "files": {"crates/cells/src/web.rs": 2}},
])

REPLAY = [
    # (diff, red binary) -- shapes of real red strand gates
    (["crates/cells/src/wire.rs"], "cells::gh1_delivery"),       # lib touch, integration test
    (["crates/cells/src/web.rs"], "cells"),                      # lib touch, lib unit test
    (["templates/curator/config.json"], "cells::gh4_shape"),     # template shape lock
    (["README.md"], "cells::gh3_readme"),                        # rule 8, a test reads the page
    (["crates/cells/tests/mock_peer.rs"], "cells::gh5_peers"),   # shared helper, compiled in
]


class ReplayTest(Base):
    files = _cells({})

    def test_impact_replay_fixtures_zero_missed(self):
        ix = graph.Index(self.doc)
        missed = []
        for diff, red in REPLAY:
            res = self.run_impact(diff, coverage=REPLAY_COVERAGE)
            bins, _, universe = impact.expand(ix, res["tests"])
            if red not in bins:
                missed.append((diff, red, res["tests"]))
            self.assertLess(len(bins), len(universe), diff)
        self.assertEqual(missed, [])


# --- runtime read paths (GH #1138, rule 8 runtime) ---------------------------
# Tests that read files at RUNTIME without naming them: a directory walk, a
# generator they run, a crate fixture. A coverage map never sees such a read,
# so the narrowing alone would drop them (the W6 replay case R18:
# `librarian_seed_corpus` reads `docs/*.md` through the seed generator).
RUNTIME_FILES = {
    "crates/alpha/tests/a4_walks_docs.rs":
        '#[test]\nfn walk() {\n    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))'
        '.join("../../");\n    for e in std::fs::read_dir(root.join("docs")).unwrap() { let _ = e; }\n}\n',
    "crates/alpha/tests/a5_runs_seed.rs":
        'const GENERATOR: &str = "workshop/tools/build_librarian_seed.py";\n\n'
        '#[test]\nfn seed() {\n    let _ = std::process::Command::new("python3").arg(GENERATOR);\n}\n',
    "crates/alpha/tests/a6_fixture.rs":
        '#[test]\nfn fixture() {\n    let _ = concat!(env!("CARGO_MANIFEST_DIR"), '
        '"/tests/fixtures/sample.json");\n}\n',
    "crates/alpha/tests/a7_walks_talky.rs":
        '#[test]\nfn walk() {\n    let _ = std::fs::read_dir("templates/talky");\n}\n',
    "crates/alpha/tests/a8_probes_plans.rs":
        '#[test]\nfn probe() {\n    if !root().join("plans").is_dir() { return; }\n'
        '    let _axis = ["user", "plans"];\n    let _p = "predicate": "plans";\n}\n',
    "crates/alpha/tests/a9_doc_roots.rs":
        'const ROOTS: [&str; 2] = ["docs", "workshop"];\n\n#[test]\nfn roots() {}\n',
    "plans/notes.md": "# notes\n",
    "crates/alpha/tests/fixtures/sample.json": "{}\n",
    "crates/alpha/tests/fixtures/orphan.json": "{}\n",
    "workshop/tools/build_librarian_seed.py": "# the seed generator\n",
    "docs/cell-types.en.md": "# Cell types\n",
}
# Ground truth of the fixture: every test that reads the path at runtime.
RUNTIME_READERS = {
    "docs/cell-types.en.md": {"binary_id(=alpha::a4_walks_docs)",
                              "binary_id(=alpha::a5_runs_seed)", "binary_id(=alpha::a9_doc_roots)"},
    "docs/guide.md": {"binary_id(=alpha::a4_walks_docs)", "binary_id(=alpha::a1_reads_doc)",
                      "binary_id(=alpha::a9_doc_roots)"},
    "crates/alpha/tests/fixtures/sample.json": {"binary_id(=alpha::a6_fixture)"},
}
LIB = "crates/alpha/src/store.rs"   # rdeps(alpha) on the floor, replaced by the map


class RuntimeReadsTest(Base):
    files = dict(FILES, **RUNTIME_FILES)

    def test_rule8_runtime_readers_are_on_the_floor(self):
        for rel, readers in RUNTIME_READERS.items():
            terms = gate_plan.test_terms([rel], self.root)
            self.assertTrue(readers <= set(terms), (rel, terms))
            for t in readers - set(gate_plan._tests_reading_path(self.root, rel)):
                self.assertTrue(any(r.startswith("rule:8 runtime ") for r in terms[t]),
                                (rel, t, terms[t]))

    def test_rule8_runtime_reads_hang_in_the_graph(self):
        ix = graph.Index(self.doc)
        for rel, readers in RUNTIME_READERS.items():
            got = {"binary_id(=%s)" % b[len("bin:"):] for b in impact.graph_tests(ix, rel)}
            self.assertTrue(readers <= got, (rel, got))

    def test_narrowed_plan_holds_every_runtime_reader(self):
        """The evidence the release asks for (#1138): with the map replacing
        rdeps(alpha), the narrowed strand plan still runs every test that reads
        the changed path at runtime -- the R18 shape (lib touch + docs drift)."""
        for rel, readers in RUNTIME_READERS.items():
            res = self.run_impact([LIB, rel])
            self.assertNotIn("rdeps(alpha)", res["tests"], rel)
            self.assertTrue(res["coverage"]["used"], rel)
            self.assertTrue(readers <= set(res["tests"]), (rel, res["tests"]))

    def test_unread_crate_fixture_keeps_rdeps_fail_closed(self):
        """A file under `crates/<c>/tests/` no test names: the reader is
        unknown, so the crate stays whole -- on the floor AND after the map."""
        rel = "crates/alpha/tests/fixtures/orphan.json"
        terms = gate_plan.test_terms([rel], self.root)
        self.assertIn("rdeps(alpha)", terms)
        self.assertIn("rule:8 runtime unread file:%s" % rel, terms["rdeps(alpha)"])
        res = self.run_impact([LIB, rel])
        self.assertIn("rdeps(alpha)", res["tests"])
        self.assertTrue(any("rdeps(alpha) kept" in n for n in res["notes"]), res["notes"])

    def test_a_probe_or_a_json_word_is_no_read(self):
        """`root.join("plans").is_dir()` asks whether the tree is private, and
        `["user", "plans"]` is data: neither reads plans/ (measured: four tests
        of the tree would have put cargo on every strand report)."""
        self.assertEqual(gate_plan._tests_reading_at_runtime(self.root, "plans/notes.md"), {})

    def test_template_paths_stay_with_rules_4_and_5(self):
        """The directory rule leaves templates/ and examples/ to their own
        rules: a walk of `templates` would otherwise pull every catalogue test.
        The script shape still holds there: the seed generator reads configs."""
        self.assertEqual(gate_plan._tests_reading_at_runtime(
            self.root, "templates/talky/system.md"), {})
        self.assertEqual(set(gate_plan._tests_reading_at_runtime(
            self.root, "templates/talky/config.json")), {"binary_id(=alpha::a5_runs_seed)"})


# tb-s1c (#1177): a test that walks a directory under crates/ of any depth --
# `crates` (the text lock of gh523), `crates/<c>`, `crates/<c>/tests` -- reads
# every crate fixture below it, from any crate. With three `/` required, the
# floor and the graph dropped the walker: a fixture diff ran none of them.
WALKER_FILES = {
    "crates/alpha/tests/a10_walks_crates.rs":
        '#[test]\nfn walk() {\n    let _ = std::fs::read_dir(root().join("crates"));\n}\n',
    "crates/alpha/tests/a11_walks_alpha.rs":
        '#[test]\nfn walk() {\n    let _ = std::fs::read_dir(root().join("crates/alpha"));\n}\n',
    "crates/alpha/tests/a12_walks_alpha_tests.rs":
        '#[test]\nfn walk() {\n    let _ = std::fs::read_dir(root().join("crates/alpha/tests"));\n}\n',
}
WALKERS = {"binary_id(=alpha::a10_walks_crates)", "binary_id(=alpha::a11_walks_alpha)",
           "binary_id(=alpha::a12_walks_alpha_tests)"}


class ShortCrateWalkersTest(Base):
    files = dict(FILES, **RUNTIME_FILES, **WALKER_FILES)

    def test_every_walker_is_on_the_floor_in_the_graph_and_in_the_narrowed_plan(self):
        ix = graph.Index(self.doc)
        for rel in ("crates/alpha/tests/fixtures/sample.json",
                    "crates/alpha/tests/fixtures/orphan.json"):
            with self.subTest(rel):
                self.assertLessEqual(WALKERS, set(gate_plan.test_terms([rel], self.root)))
                got = {"binary_id(=%s)" % b[len("bin:"):] for b in impact.graph_tests(ix, rel)}
                self.assertLessEqual(WALKERS, got)
                self.assertLessEqual(WALKERS, set(self.run_impact([LIB, rel])["tests"]))

    def test_a_walker_never_makes_a_file_read_the_unread_net_holds(self):
        """tb-s1c M-1: the walkers plan along, but say nothing of the one file
        (gh523 reads json|rs|manifest, gh230 skips tests/). `orphan.json` has
        no reader of its own: rdeps(alpha) stays on the floor and after the
        map, and the integration plan runs the full suite (KR-TB-1 (c)) --
        in a tree WITH walkers, as the real one has two."""
        rel = "crates/alpha/tests/fixtures/orphan.json"
        terms = gate_plan.test_terms([rel], self.root)
        self.assertLessEqual(WALKERS, set(terms))
        self.assertIn("rule:8 runtime unread file:%s" % rel, terms.get("rdeps(alpha)", ()))
        res = self.run_impact([LIB, rel])
        self.assertIn("rdeps(alpha)", res["tests"])
        self.assertLessEqual(WALKERS, set(res["tests"]))
        self.assertTrue(any("rdeps(alpha) kept" in n for n in res["notes"]), res["notes"])
        since = "stage:integration narrowed since=abc"
        full = impact.impact([LIB, rel], "integration", self.doc, repo=self.root,
                             coverage=COVERAGE, now=NOW, narrowed=since)
        self.assertEqual(["all()"], full["tests"])
        self.assertTrue(any("unknown reader" in n for n in full["notes"]), full["notes"])
        # a fixture with a reader of its own stays narrow
        own = impact.impact([LIB, "crates/alpha/tests/fixtures/sample.json"], "integration",
                            self.doc, repo=self.root, coverage=COVERAGE, now=NOW, narrowed=since)
        self.assertNotIn("all()", own["tests"])

    def test_a_repo_relative_walker_reads_every_rust_source_below(self):
        """tb-s1c m-17: a walk written repo-relative reads the `.rs` below it
        too (gh523 locks a CEL fragment in every `.rs` below `crates`), on the
        floor, in the graph (`form: repo`) and in the narrowed plan, where
        the map replaces rdeps(alpha). `a12` walks crates/alpha/tests, not
        src/; before the fix none of the three ran for a src file."""
        ix = graph.Index(self.doc)
        want = WALKERS - {"binary_id(=alpha::a12_walks_alpha_tests)"}
        floor = set(gate_plan.test_terms([LIB], self.root))
        self.assertLessEqual(want, floor)
        self.assertNotIn("binary_id(=alpha::a12_walks_alpha_tests)", floor)
        got = {"binary_id(=%s)" % b[len("bin:"):] for b in impact.graph_tests(ix, LIB)}
        self.assertLessEqual(want, got)
        self.assertNotIn("binary_id(=alpha::a12_walks_alpha_tests)", got)
        narrowed = self.run_impact([LIB])
        self.assertNotIn("rdeps(alpha)", narrowed["tests"])
        self.assertLessEqual(want, set(narrowed["tests"]))


# Rule 10 (welle-tb S1b, follow-up of the re-check): a crate file outside src/
# and tests/ -- an `include_str!` schema, an example, a bench, a README a crate
# doc includes, a migration, a template or a toml inside the crate, a build.rs
# input. The map lists `.rs` lines only, so it can never see such a file: the
# crate stays whole in EVERY stage that narrows.
CRATE_FILE_PATHS = (
    "crates/alpha/schemas/body.json",
    "crates/alpha/examples/demo.rs",
    "crates/alpha/benches/hot.rs",
    "crates/alpha/README.md",
    "crates/alpha/migrations/0001_init.sql",
    "crates/alpha/templates/body/config.json",
    "crates/alpha/config.toml",
    "crates/alpha/build/gen.in",
)
NARROWED_STAGES = (
    ("pre", None),
    ("strand", None),
    ("merge", "stage:merge narrowed parent=abc strand-run=1"),
    ("integration", "stage:integration narrowed since=abc"),
)


class CrateFileTest(Base):
    def test_a_crate_file_alone_plans_rdeps_in_every_narrowing_stage(self):
        for rel in CRATE_FILE_PATHS:
            for stage, narrowed in NARROWED_STAGES:
                res = impact.impact([rel], stage, self.doc, repo=self.root,
                                    coverage=COVERAGE, now=NOW, narrowed=narrowed)
                self.assertTrue({"rdeps(alpha)", "all()"} & set(res["tests"]),
                                (rel, stage, res["tests"]))
                if "rdeps(alpha)" in res["tests"]:
                    self.assertIn("rule:10 crate-file:%s" % rel,
                                  res["reason"]["rdeps(alpha)"], (rel, stage))

    def test_the_map_never_replaces_the_rdeps_of_a_crate_file(self):
        """With a lib file the map knows, the map would replace rdeps(alpha) --
        the crate file next to it keeps the crate whole."""
        alone = self.run_impact(["crates/alpha/src/store.rs"])
        self.assertNotIn("rdeps(alpha)", alone["tests"])
        for rel in CRATE_FILE_PATHS:
            for stage, narrowed in NARROWED_STAGES:
                res = impact.impact(["crates/alpha/src/store.rs", rel], stage, self.doc,
                                    repo=self.root, coverage=COVERAGE, now=NOW,
                                    narrowed=narrowed)
                self.assertTrue({"rdeps(alpha)", "all()"} & set(res["tests"]),
                                (rel, stage, res["tests"]))
                if "rdeps(alpha)" in res["tests"]:
                    self.assertTrue(any("rdeps(alpha) kept" in n for n in res["notes"]),
                                    (rel, stage, res["notes"]))

    def test_the_rdeps_of_a_crate_file_reaches_the_reverse_dependencies(self):
        res = self.run_impact([CRATE_FILE_PATHS[0]])
        self.assertIn("rdeps(alpha)", res["tests"])
        bins = impact.expand(graph.Index(self.doc), res["tests"])[0]
        self.assertIn("beta::b1_uses_alpha", bins)            # the reverse dependency
        self.assertIn("alpha::a1_reads_doc", bins)            # the owning crate


# --- graph names from before the harness fold (welle-tb S1 part 0, GH #1159) --
# The graph of 28c948e15 still named binaries the harness fold (#1153) had
# folded away -- up to 398 "gone from the tree" NOTEs per case, each one a test
# that the graph said reads the diff and that was then silently NOT selected.
# A gone name maps to its folded term only when the fold is in the tree and the
# term hits >= 1 test; anything else plans rdeps of the crate (fail-closed).
GONE_FILES = _cells({"crates/cells/tests/gh6_grammar.rs":
                     "use cells::wire::send;\n\n#[test]\nfn g() {\n    send();\n}\n"})
FOLDED = "(binary_id(=cells::assemble) & test(/^gh6_grammar::/))"


class GoneBinaryTest(Base):
    files = GONE_FILES

    def fold(self, module_text):
        """The tree after the fold; the graph (built in setUp) is from before."""
        os.remove(os.path.join(self.root, "crates/cells/tests/gh6_grammar.rs"))
        if module_text is not None:
            make_tree(self.root, {
                "crates/cells/tests/assemble/main.rs": "mod gh6_grammar;\n",
                "crates/cells/tests/assemble/gh6_grammar.rs": module_text})
        gate_plan._SOURCE_CACHE.clear()
        gate_plan._NO_TEST_CACHE.clear()
        return self.run_impact(["crates/cells/src/wire.rs"], coverage=REPLAY_COVERAGE)

    def test_gone_binary_maps_to_folded_harness_term(self):
        res = self.fold("use cells::wire::send;\n\n#[test]\nfn g() {\n    send();\n}\n")
        self.assertIn(FOLDED, res["tests"])
        self.assertIn("graph:gone cells::gh6_grammar folded into cells::assemble",
                      res["reason"][FOLDED])
        self.assertNotIn("rdeps(cells)", res["tests"])   # the map still narrows

    def test_gone_binary_without_folded_form_falls_back_to_crate_rdeps(self):
        res = self.fold(None)
        self.assertIn("rdeps(cells)", res["tests"])
        self.assertIn("graph:gone cells::gh6_grammar -- no folded form",
                      res["reason"]["rdeps(cells)"])
        self.assertFalse(any("gh6_grammar" in t for t in res["tests"] if t != "rdeps(cells)"))

    def test_gone_binary_folded_term_matching_no_test_falls_back_to_crate_rdeps(self):
        res = self.fold("pub fn helper() {}\n")
        self.assertIn("rdeps(cells)", res["tests"])
        self.assertIn("graph:gone cells::gh6_grammar -- folded term cells::assemble "
                      "gh6_grammar:: hits no test", res["reason"]["rdeps(cells)"])
        self.assertNotIn(FOLDED, res["tests"])

    def test_integration_gone_name_falls_back_to_rdeps(self):
        """KR-TB-1 (c): in a narrowed integration a gone name without a folded
        form plans rdeps of its crate -- the map never replaces that rdeps."""
        os.remove(os.path.join(self.root, "crates/cells/tests/gh6_grammar.rs"))
        gate_plan._SOURCE_CACHE.clear()
        gate_plan._NO_TEST_CACHE.clear()
        term = "stage:integration narrowed since=0123456789"
        res = impact.impact(["crates/cells/src/wire.rs"], "integration", self.doc, repo=self.root,
                            coverage=REPLAY_COVERAGE, now=NOW, narrowed=term)
        self.assertIn("rdeps(cells)", res["tests"])
        self.assertIn("graph:gone cells::gh6_grammar -- no folded form",
                      res["reason"]["rdeps(cells)"])
        self.assertIn(term, res["reason"]["rdeps(cells)"])
        self.assertFalse(res["filter"].startswith("all()"), res["filter"])
        self.assertTrue(any("rdeps(cells) kept" in n for n in res["notes"]), res["notes"])

    def test_gone_binary_test_count_reads_tokio_and_quoted_json(self):
        """The count is the token lexer's: `#[tokio::test(..)]` is a test, and a
        `'` inside a string literal does not swallow the attributes after it."""
        module = ('fn tree() -> serde_json::Value {\n'
                  '    json!({"route": "\'in_tool\'", "when": "a == \'res\'"})\n}\n\n'
                  '#[tokio::test(flavor = "multi_thread", worker_threads = 4)]\n'
                  'async fn t() {}\n\n#[test]\n#[ignore]\nfn u() {}\n')
        self.assertEqual(crate_root.test_attribute_count(module), 2)
        self.assertEqual(crate_root.test_attribute_count("pub fn helper() {}\n"), 0)
        self.assertIsNone(crate_root.test_attribute_count("/* open\n"))
        res = self.fold(module)
        self.assertIn(FOLDED, res["tests"])


# --- crate roots (welle-tb S1 part A, GH #1159) --------------------------------
# lib.rs declares modules and runs no line of its own, so it is never in the
# map: today every lib.rs edit plans rdeps (HO V2: 8727 tests against 4056 for
# cell.rs). A POSITIVE list of edits -- whitespace, non-doc comments, bodies of
# non-const fns -- plans the map parts of the crate's modules instead.
ROOT_LIB = (
    "//! The cells crate.\n"
    "#![allow(dead_code)]\n\n"
    "pub mod web;\n"
    "pub mod wire;\n\n"
    "pub use wire::send;\n\n"
    "/// Doc of the helper.\n"
    "pub fn helper() -> u8 {\n"
    "    1\n"
    "}\n\n"
    "pub const LIMIT: u8 = 3;\n\n"
    "pub const fn twice(x: u8) -> u8 {\n"
    "    x * 2\n"
    "}\n\n"
    "macro_rules! noop {\n"
    "    () => {};\n"
    "}\n"
)
CELLS_LIB = "crates/cells/src/lib.rs"


def _git(root, *args):
    import subprocess
    env = dict(os.environ, GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@t", GIT_COMMITTER_NAME="t",
               GIT_COMMITTER_EMAIL="t@t")
    for k in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"):
        env.pop(k, None)
    return subprocess.run(["git", "-C", root, *args], check=True, capture_output=True,
                          text=True, env=env).stdout.strip()


class CrateRootTest(Base):
    files = _cells({CELLS_LIB: ROOT_LIB})

    def setUp(self):
        super().setUp()
        _git(self.root, "init", "-q", "-b", "main")
        _git(self.root, "add", "-A")
        _git(self.root, "commit", "-q", "-m", "map")
        self.map_rev = _git(self.root, "rev-parse", "HEAD")
        self.cov = dict(REPLAY_COVERAGE, rev=self.map_rev)

    def plan_after(self, old, new):
        """Commit `old` -> `new` in lib.rs and plan lib.rs against the map's rev."""
        path = os.path.join(self.root, CELLS_LIB)
        with open(path, encoding="utf-8") as fh:
            text = fh.read()
        self.assertIn(old, text)
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(text.replace(old, new, 1))
        _git(self.root, "commit", "-q", "-am", "edit")
        res = impact.impact([CELLS_LIB], "strand", self.doc, repo=self.root, coverage=self.cov,
                            now=NOW, rev=self.map_rev)
        _git(self.root, "reset", "-q", "--hard", self.map_rev)
        return res

    def assert_narrowed(self, res):
        self.assertNotIn("rdeps(cells)", res["tests"])
        self.assertIn(impact.part_term("cells", "web"), res["tests"])
        self.assertIn("binary_id(=cells::gh1_delivery)", res["tests"])
        self.assertIn("binary_id(=cells::gh2_viewer)", res["tests"])
        # lib.rs holds no test of its own: the whole lib binary is not planned
        self.assertNotIn("binary_id(=cells)", res["tests"])
        self.assertIn("cover:crate-root file:%s" % CELLS_LIB, res["reason"]["binary_id(=cells::gh1_delivery)"])

    def assert_rdeps(self, res, what):
        self.assertIn("rdeps(cells)", res["tests"], what)

    def test_lib_rs_comment_only_change_plans_crate_modules_not_rdeps(self):
        self.assert_narrowed(self.plan_after("pub mod web;\n", "// a note\npub mod web;\n"))
        self.assert_narrowed(self.plan_after("pub mod wire;\n", "pub mod wire; /* why */\n\n\n"))
        # a non-const fn body is runtime only: what runs it, the map knows
        self.assert_narrowed(self.plan_after("    1\n}", "    // two now\n    2\n}"))
        # a root WITH tests of its own plans its own unit binary as well
        with open(os.path.join(self.root, CELLS_LIB), "a", encoding="utf-8") as fh:
            fh.write("\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n")
        _git(self.root, "commit", "-q", "-am", "own tests")
        self.map_rev = _git(self.root, "rev-parse", "HEAD")
        self.cov = dict(REPLAY_COVERAGE, rev=self.map_rev)
        res = self.plan_after("pub mod web;\n", "// a note\npub mod web;\n")
        self.assertNotIn("rdeps(cells)", res["tests"])
        self.assertIn("cover:own-module file:%s" % CELLS_LIB, res["reason"]["binary_id(=cells)"])

    def test_lib_rs_doc_comment_change_falls_back_to_rdeps(self):
        self.assert_narrowed(self.plan_after("pub mod web;\n", "// a note\npub mod web;\n"))
        for old, new in (("/// Doc of the helper.", "/// Doc of the helper, changed."),
                         ("//! The cells crate.", "//! The cells crate.\n//! ```\n//! ```"),
                         ("pub mod web;", "/** doc */\npub mod web;")):
            self.assert_rdeps(self.plan_after(old, new), new)

    def test_lib_rs_item_or_attribute_change_falls_back_to_rdeps(self):
        self.assert_narrowed(self.plan_after("pub mod web;\n", "// a note\npub mod web;\n"))
        for old, new in (("LIMIT: u8 = 3", "LIMIT: u8 = 4"),                    # const
                         ("    x * 2\n", "    x * 3\n"),                         # const fn body
                         ("() => {};", "() => { 1 };"),                          # macro_rules!
                         ("#![allow(dead_code)]", "#![allow(unused)]"),         # #![...]
                         ("pub mod wire;", "#[cfg(test)]\npub mod wire;"),      # #[cfg]
                         ("pub use wire::send;", "use wire::send;"),            # use
                         ("pub fn helper() -> u8", "pub fn helper() -> u16"),   # fn signature
                         ("pub fn helper", "#[no_mangle]\npub fn helper"),      # attribute
                         ("    1\n}", "    impl Clone for X { }\n    1\n}"),    # impl in a body
                         ("pub const LIMIT", "pub static S: u8 = 1;\npub const LIMIT")):
            self.assert_rdeps(self.plan_after(old, new), new)

    def test_lib_rs_mod_added_falls_back_to_rdeps(self):
        self.assert_narrowed(self.plan_after("pub mod web;\n", "// a note\npub mod web;\n"))
        self.assert_rdeps(self.plan_after("pub mod wire;\n", "pub mod wire;\nmod extra;\n"), "mod")
        self.assert_rdeps(self.plan_after("pub mod wire;\n",
                                          "pub mod wire;\n#[path = \"x.rs\"]\nmod x;\n"), "#[path]")
        self.assert_rdeps(self.plan_after("pub mod wire;\n",
                                          "pub mod wire;\ninclude!(\"gen.rs\");\n"), "include!")

    def test_lib_rs_mod_removed_falls_back_to_rdeps(self):
        self.assert_narrowed(self.plan_after("pub mod web;\n", "// a note\npub mod web;\n"))
        self.assert_rdeps(self.plan_after("pub mod web;\n", ""), "mod removed")
        self.assert_rdeps(self.plan_after("pub mod web;\n", "// pub mod web;\n"), "commented out")

    def test_lib_rs_pub_use_changed_falls_back_to_rdeps(self):
        self.assert_narrowed(self.plan_after("pub mod web;\n", "// a note\npub mod web;\n"))
        self.assert_rdeps(self.plan_after("pub use wire::send;", "pub use web::serve;"), "pub use")
        self.assert_rdeps(self.plan_after("pub use wire::send;", ""), "pub use removed")
        self.assert_rdeps(self.plan_after("pub mod web;", "extern crate core;\npub mod web;"),
                          "extern crate")

    def test_crate_root_skeleton_edges_fail_closed(self):
        """The lexer edges of the positive list (pure, no git)."""
        ok = crate_root.body_only_change
        self.assertTrue(ok('extern "C" fn f() { 1 }\n', 'extern "C" fn f() { 2 }\n'))
        self.assertTrue(ok("impl X {\n    fn f(&self) { a(); }\n}\n",
                           "impl X {\n    fn f(&self) { b(); } // why\n}\n"))
        self.assertTrue(ok('fn f() { let s = "// no comment"; }\n',
                           'fn f() { let s = "// still none"; }\n'))
        self.assertFalse(ok('const S: &str = "a // b";\n', 'const S: &str = "a // c";\n'))
        self.assertFalse(ok("trait T { fn f(); }\n", "trait T { fn f(); fn g(); }\n"))
        self.assertFalse(ok("pub const unsafe fn f() -> u8 { 1 }\n",
                            "pub const unsafe fn f() -> u8 { 2 }\n"))
        self.assertFalse(ok("m! { fn f() { 1 } }\n", "m! { fn f() { 2 } }\n"))
        self.assertFalse(ok("fn f() -> A<{ 1 }> { 1 }\n", "fn f() -> A<{ 2 }> { 1 }\n"))
        self.assertFalse(ok("fn f() { 1 }\n", "fn f() { 1 }\n/* open\n"))
        self.assertFalse(ok("fn f() { 1 }\n", None))
        self.assertTrue(ok("//// plain\nfn f() {}\n", "//// other\nfn f() {}\n"))
        self.assertFalse(ok("/** doc */\nfn f() {}\n", "/** other */\nfn f() {}\n"))

    def test_crate_root_without_map_fit_keeps_rdeps(self):
        self.assert_narrowed(self.plan_after("pub mod web;\n", "// a note\npub mod web;\n"))
        for cov in (dict(self.cov, rev="0" * 40), dict(self.cov, complete=False),
                    dict(self.cov, built_at="2026-09-01T00:00:00Z"), None):
            path = os.path.join(self.root, CELLS_LIB)
            with open(path, encoding="utf-8") as fh:
                text = fh.read()
            with open(path, "w", encoding="utf-8") as fh:
                fh.write("// a note\n" + text)
            _git(self.root, "commit", "-q", "-am", "edit")
            res = impact.impact([CELLS_LIB], "strand", self.doc, repo=self.root, coverage=cov,
                                now=NOW, rev=self.map_rev)
            _git(self.root, "reset", "-q", "--hard", self.map_rev)
            self.assert_rdeps(res, cov and {k: cov[k] for k in ("rev", "complete", "built_at")})
        # map_fits=False in the narrowing: no map reaches impact at all
        from pipeline import narrow as narrow_mod
        with mock.patch.object(narrow_mod, "map_fits",
                               return_value=(False, ["narrowing: unfit"], [], [])):
            sel, notes = narrow_mod.narrow({"filter": "rdeps(cells)", "rdeps": ["cells"]}, [CELLS_LIB],
                                           self.root, self.doc, self.cov, now=NOW,
                                           metadata={})
        self.assertEqual(sel["rdeps"], ["cells"])
        self.assertIn("narrowing: unfit", notes)

    # --- fix round H1 (re-check tb-s1) --------------------------------------

    def plan_dirty(self, old, new):
        """Edit lib.rs WITHOUT a commit (the strand gate runs before the
        commit: HEAD is the map's rev, the plan reads the dirty tree)."""
        path = os.path.join(self.root, CELLS_LIB)
        with open(path, encoding="utf-8") as fh:
            text = fh.read()
        self.assertIn(old, text)
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(text.replace(old, new, 1))
        res = impact.impact([CELLS_LIB], "strand", self.doc, repo=self.root, coverage=self.cov,
                            now=NOW, rev=self.map_rev)
        _git(self.root, "reset", "-q", "--hard", self.map_rev)
        return res

    def test_crate_root_dirty_edit_keeps_rdeps(self):
        """B1: the positive list reads the new side where the plan reads it --
        the working tree. An uncommitted `mod`/`const`/`pub use` edit is no
        body-only change."""
        self.assert_narrowed(self.plan_dirty("pub mod web;\n", "// a note\npub mod web;\n"))
        for old, new in (("pub mod wire;\n", "pub mod wire;\nmod extra;\n"),
                         ("LIMIT: u8 = 3", "LIMIT: u8 = 4"),
                         ("pub use wire::send;", "")):
            self.assert_rdeps(self.plan_dirty(old, new), new)

    def test_crate_root_missing_in_working_tree_keeps_rdeps(self):
        os.remove(os.path.join(self.root, CELLS_LIB))
        res = impact.impact([CELLS_LIB], "strand", self.doc, repo=self.root, coverage=self.cov,
                            now=NOW, rev=self.map_rev)
        harmless = crate_root.root_change_is_harmless(self.root, self.map_rev, CELLS_LIB,
                                                      tree=True)
        _git(self.root, "reset", "-q", "--hard", self.map_rev)
        self.assert_rdeps(res, "lib.rs deleted")
        self.assertFalse(harmless)

    def test_crate_root_body_with_crate_wide_items_is_no_body_change(self):
        """M2: a fn body that exports a macro, pulls a file in (`#[path] mod`,
        `include!`) or carries linkage under `cfg_attr` changes more than the
        fn -- never harmless."""
        ok = crate_root.body_only_change
        for old, new in (
                ("fn f() { 1 }\n",
                 "fn f() { #[macro_export] macro_rules! m { () => {} } 1 }\n"),
                ('fn f() { #[path = "a.rs"] mod m; }\n', 'fn f() { #[path = "b.rs"] mod m; }\n'),
                ('fn f() { include!("a.rs"); }\n', 'fn f() { include!("b.rs"); }\n'),
                ("fn f() { 1 }\n",
                 "fn f() { #[cfg_attr(unix, no_mangle)] extern \"C\" fn g() {} 1 }\n"),
                ("fn f() { 1 }\n", "fn f() { #[derive(Clone)] struct S; 1 }\n"),
                ("fn f() { 1 }\n", "fn f() { mod inner {} 1 }\n")):
            self.assertFalse(ok(old, new), new)
        # attributes that stay inside the body keep it a body
        self.assertTrue(ok("fn f() { 1 }\n", "fn f() { #[allow(unused)] let x = 2; 1 }\n"))
        self.assertTrue(ok("fn f() { let v = vec![1]; }\n", "fn f() { let v = vec![2]; }\n"))
        # and through impact: the root then keeps rdeps
        self.assert_rdeps(self.plan_after("    1\n}", '    include!("gen.rs");\n    1\n}'),
                          "include! in a body")

    def test_crate_root_body_with_item_expanding_macros_is_no_body_change(self):
        """re-check m-N4: `global_asm!` and `inventory::submit!` in a body put
        items into the crate (a symbol, a registry entry) -- never harmless."""
        ok = crate_root.body_only_change
        for old, new in (
                ('fn f() { 1 }\n', 'fn f() { core::arch::global_asm!(".globl g"); 1 }\n'),
                ("fn f() { 1 }\n", "fn f() { inventory::submit! { Plugin }; 1 }\n")):
            self.assertFalse(ok(old, new), new)

    def test_crate_root_lexer_edges(self):
        """m1/m2: the edges of `crate_root.tokens` (S4 imports it)."""
        t = crate_root.tokens
        self.assertEqual(t('r#"a "q" b"# x'), ['r#"a "q" b"#', "x"])
        self.assertEqual(t('r##"x"#y"## z'), ['r##"x"#y"##', "z"])
        self.assertEqual(t('b"a" br#"b"# c"c"'), ['b"a"', 'br#"b"#', 'c"c"'])
        self.assertEqual(t("'a 'a' '\\'' "), ["'a", "'a'", "'\\''"])
        self.assertEqual(t("/* a /* b */ } */ x"), ["x"])
        self.assertEqual(t('"a\\\n b" x'), ['"a\\\n b"', "x"])
        self.assertEqual(t("//! inner\n/*! blk */ x"), ["//! inner", "/*! blk */", "x"])
        self.assertEqual(t("/***/ /**/ x"), ["x"])
        self.assertEqual(t("fn é<'ä>() {}"), ["fn", "é", "<", "'ä", ">", "(", ")", "{", "}"])
        # a non-ASCII identifier is Rust (tb-s10 A1 point 20); `²` is none
        for bad in ('let s = "open', 'b"open', 'r#"open"', "/* a /* b */", "fn a\u00b2() {}",
                    "'"):
            with self.assertRaises(crate_root.Doubt, msg=bad):
                t(bad)

    def test_test_attribute_count_whitelist(self):
        """m4: only test-making attributes count; a name that merely holds
        `test` does not."""
        n = crate_root.test_attribute_count
        self.assertEqual(n("#[attestation]\nfn a() {}\n#[test_helper]\nfn b() {}\n"), 0)
        self.assertEqual(n("#[test]\nfn a() {}\n#[tokio::test]\nasync fn b() {}\n"
                           "#[rstest]\nfn c() {}\n#[test_case(1)]\nfn d(x: u8) {}\n"
                           "#[quickcheck]\nfn e() {}\n#[sqlx::test]\nasync fn f() {}\n"), 6)

    def test_crate_root_rstest_counts_as_own_tests(self):
        """m5: a root whose own tests carry `#[rstest]` (no `test` token) plans
        its own unit binary."""
        with open(os.path.join(self.root, CELLS_LIB), "a", encoding="utf-8") as fh:
            fh.write("\n#[rstest]\nfn r() {}\n")
        _git(self.root, "commit", "-q", "-am", "own rstest")
        self.map_rev = _git(self.root, "rev-parse", "HEAD")
        self.cov = dict(REPLAY_COVERAGE, rev=self.map_rev)
        res = self.plan_after("pub mod web;\n", "// a note\npub mod web;\n")
        self.assertIn("binary_id(=cells)", res["tests"])
        self.assertIn("cover:own-module file:%s" % CELLS_LIB, res["reason"]["binary_id(=cells)"])


CELLS_MAIN = "crates/cells/src/main.rs"


class MainRsTest(Base):
    """M1: the map never sees main.rs (spawned binaries are not measured):
    a body-only edit of it keeps rdeps."""
    files = _cells({CELLS_LIB: ROOT_LIB, CELLS_MAIN: "fn main() {\n    cells::helper();\n}\n",
                    "crates/cells/src/bin/tool.rs": "fn main() {\n    let _ = 1;\n}\n"})

    def setUp(self):
        super().setUp()
        _git(self.root, "init", "-q", "-b", "main")
        _git(self.root, "add", "-A")
        _git(self.root, "commit", "-q", "-m", "map")
        self.map_rev = _git(self.root, "rev-parse", "HEAD")
        self.cov = dict(REPLAY_COVERAGE, rev=self.map_rev)

    def assert_rdeps(self, res, what):
        self.assertIn("rdeps(cells)", res["tests"], what)

    def test_main_rs_body_change_keeps_rdeps(self):
        for path, old, new in ((CELLS_MAIN, "cells::helper();", "cells::helper(); // twice"),
                               (CELLS_MAIN, "cells::helper();", "let _ = cells::helper();"),
                               ("crates/cells/src/bin/tool.rs", "let _ = 1;", "let _ = 2;")):
            full = os.path.join(self.root, path)
            with open(full, encoding="utf-8") as fh:
                text = fh.read()
            with open(full, "w", encoding="utf-8") as fh:
                fh.write(text.replace(old, new, 1))
            res = impact.impact([path], "strand", self.doc, repo=self.root, coverage=self.cov,
                                now=NOW, rev=self.map_rev)
            _git(self.root, "reset", "-q", "--hard", self.map_rev)
            self.assert_rdeps(res, (path, new))


class FoldedFormTest(Base):
    """m3/m7: the folded form is the harness_fold shape or nothing."""
    files = GONE_FILES

    def fold_into(self, mains):
        os.remove(os.path.join(self.root, "crates/cells/tests/gh6_grammar.rs"))
        module = "#[test]\nfn g() {}\n"
        tree = {}
        for h, main in mains.items():
            tree["crates/cells/tests/%s/main.rs" % h] = main
            tree["crates/cells/tests/%s/gh6_grammar.rs" % h] = module
        make_tree(self.root, tree)
        gate_plan._SOURCE_CACHE.clear()
        gate_plan._NO_TEST_CACHE.clear()
        return self.run_impact(["crates/cells/src/wire.rs"], coverage=REPLAY_COVERAGE)

    def test_gone_binary_folded_into_two_harnesses_falls_back_to_rdeps(self):
        res = self.fold_into({"assemble": "mod gh6_grammar;\n", "other": "mod gh6_grammar;\n"})
        self.assertIn("rdeps(cells)", res["tests"])
        self.assertIn("graph:gone cells::gh6_grammar -- folded into 2 harnesses "
                      "(assemble, other)", res["reason"]["rdeps(cells)"])
        self.assertFalse(any("gh6_grammar" in t for t in res["tests"] if t != "rdeps(cells)"))

    def test_gone_binary_gated_or_nested_mod_is_no_folded_form(self):
        for main in ("#[cfg(feature = \"x\")]\nmod gh6_grammar;\n",
                     "mod a {\n    mod gh6_grammar;\n}\n",
                     "#[path = \"../x.rs\"]\npub mod gh6_grammar;\n"):
            self.tearDown()
            self.setUp()
            res = self.fold_into({"assemble": main})
            self.assertIn("rdeps(cells)", res["tests"], main)
            self.assertNotIn(FOLDED, res["tests"], main)
        self.tearDown()
        self.setUp()
        res = self.fold_into({"assemble": "//! harness\n\npub mod gh6_grammar;\n"})
        self.assertIn(FOLDED, res["tests"])


if __name__ == "__main__":
    unittest.main()
