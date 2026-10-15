"""Tests of the impact graph (`scripts/pipeline/graph.py`, W6, GH #1120).

Every case builds a small synthetic workspace in a temporary directory: two
crates (`alpha`, and `beta` with a dev-dependency on `alpha`), three
templates, one scaffold that pins two of them (one pin stale), two scenario
cases, a docs page a test reads and an ADR anchor.
"""

import json
import os
import pathlib
import sys
import tempfile
import unittest
from unittest import mock

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS))

import gate_plan  # noqa: E402
from pipeline import graph  # noqa: E402
from scripts.tests import store_isolation  # noqa: E402

FILES = {
    "Cargo.toml": '[workspace]\nmembers = ["crates/alpha", "crates/beta"]\n',
    "crates/alpha/Cargo.toml": '[package]\nname = "alpha"\nversion = "0.1.0"\n\n'
                               '[dependencies]\nserde = "1"\n',
    "crates/alpha/src/lib.rs": "pub mod net;\npub mod store;\n\n#[cfg(test)]\nmod tests {\n"
                               "    #[test]\n    fn root_ok() {}\n}\n",
    "crates/alpha/src/net.rs": "use crate::store::Db;\nuse serde::Deserialize;\n\n"
                               "pub fn f(_d: Db) {}\n\n#[cfg(test)]\nmod tests {\n"
                               "    #[test]\n    fn net_ok() {}\n}\n",
    "crates/alpha/src/store.rs": "pub struct Db;\n",
    "crates/alpha/tests/a1_reads_doc.rs": 'use alpha::net::f;\n\n#[test]\nfn reads() {\n'
                                          '    let _ = "docs/guide.md";\n}\n',
    "crates/alpha/tests/a2_template.rs": '#[test]\nfn t() {\n'
                                         '    let _ = "templates/talky/config.json";\n}\n',
    "crates/beta/Cargo.toml": '[package]\nname = "beta"\nversion = "0.1.0"\n\n'
                              '[dev-dependencies]\nalpha = { path = "../alpha" }\n',
    "crates/beta/src/lib.rs": "pub fn b() {}\n",
    "crates/beta/tests/b1_uses_alpha.rs": "use alpha::store::Db;\n\n#[test]\nfn b() {}\n",
    "crates/beta/tests/b2_ignored.rs": "#[test]\n#[ignore]\nfn slow() {}\n",
    "templates/talky/template.json": '{"name": "talky", "version": "1.2.0"}\n',
    "templates/talky/config.json": '{"type": "llm"}\n',
    "templates/cogny/template.json": '{"name": "cogny", "version": "2.0.0"}\n',
    "templates/memory-hive/template.json": '{"name": "memory-hive", "version": "0.3.0"}\n',
    "workshop/evals/builder-scenarios/scaffolds/eyes-min/main/os/config.json":
        '{"a": "talky@1.2.0",\n "b": "cogny@1.9.0"}\n',
    "workshop/evals/builder-scenarios/cases/A1-eye.json": '{"id": "A1", "scaffold": "eyes-min"}\n',
    "workshop/evals/scenarios/cases/C1-claim.json": '{"id": "C1"}\n',
    "docs/guide.md": "# Guide\n",
    ".github/gates/adr-anchors.tsv": "adr\tstatus\tanchor_home\ttarget\n"
                                     "0001-net.md\taccepted\tself\ttest:net_ok\n",
}

METADATA = {
    "workspace_members": ["alpha 0.1.0 (path+file:///w/crates/alpha)",
                          "beta 0.1.0 (path+file:///w/crates/beta)"],
    "packages": [
        {"id": "alpha 0.1.0 (path+file:///w/crates/alpha)", "name": "alpha",
         "manifest_path": "/w/crates/alpha/Cargo.toml",
         "dependencies": [{"name": "serde", "kind": None}]},
        {"id": "beta 0.1.0 (path+file:///w/crates/beta)", "name": "beta",
         "manifest_path": "/w/crates/beta/Cargo.toml",
         "dependencies": [{"name": "alpha", "kind": "dev"}]},
        {"id": "serde 1.0.0 (registry+https://example.invalid)", "name": "serde",
         "manifest_path": "/r/serde/Cargo.toml", "dependencies": []},
    ],
}


def make_tree(root, files=FILES):
    for rel, text in files.items():
        path = pathlib.Path(root, rel)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")


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
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = self.tmp.name
        make_tree(self.root)
        gate_plan._SOURCE_CACHE.clear()
        gate_plan._NO_TEST_CACHE.clear()

    def tearDown(self):
        gate_plan._SOURCE_CACHE.clear()
        gate_plan._NO_TEST_CACHE.clear()
        self.tmp.cleanup()

    def build(self, **kw):
        kw.setdefault("stations", False)
        g, st = graph.build(self.root, **kw)
        return g.to_json("fixture", "2026-10-08T00:00:00Z"), st

    @staticmethod
    def edges(doc, kind):
        return {(e["from"], e["to"]) for e in doc["edges"] if e["kind"] == kind}


class GraphTest(Base):
    def test_graph_crate_depends_from_metadata_fixture(self):
        doc, _ = self.build(metadata=METADATA)
        deps = [e for e in doc["edges"] if e["kind"] == "depends"]
        self.assertEqual([(e["from"], e["to"], e.get("data")) for e in deps],
                         [("crate:beta", "crate:alpha", {"dev": True})])
        # the manifests alone give the same edge (no cargo needed)
        doc2, _ = self.build()
        self.assertEqual(self.edges(doc2, "depends"), {("crate:beta", "crate:alpha")})

    def test_graph_reads_and_references_match_resolver_functions(self):
        doc, _ = self.build()
        reads = self.edges(doc, "reads")
        refs = self.edges(doc, "references")
        files = [r for r in FILES if not r.startswith("crates/")]
        for rel in files:
            want = {"bin:%s" % graph.binary_id_of_term(t)
                    for t in gate_plan._tests_reading_path(self.root, rel)}
            got = {src for src, dst in reads if dst == "file:%s" % rel}
            self.assertEqual(got, want, rel)
        for name in ("talky", "cogny", "memory-hive"):
            want = {"bin:%s" % graph.binary_id_of_term(t)
                    for t in gate_plan._tests_referencing_template(self.root, name)}
            got = {src for src, dst in refs if dst == "template:%s" % name
                   and src.startswith("bin:")}
            self.assertEqual(got, want, name)
        self.assertIn(("bin:alpha::a1_reads_doc", "file:docs/guide.md"), reads)
        self.assertIn(("bin:alpha::a2_template", "template:talky"), refs)

    def test_a_helper_without_main_rs_gets_no_bin_node(self):
        """tb-s1c-d (re-check m-2): a helper in a directory without main.rs
        (`tests/support/*.rs`, tb-s1c-c F4) walks a directory, so the runtime
        index lists it, but it has no binary -- a `bin:` node for it would be
        a binary_id nextest refuses. The floor plans its includers instead."""
        make_tree(self.root, {
            "crates/alpha/tests/support/walk.rs":
                "pub fn w(root: &std::path::Path) { let _ = root.join(\"docs\"); }\n",
            "crates/alpha/tests/gh_w.rs":
                '#[path = "support/walk.rs"]\nmod walk;\n#[test]\nfn t() {}\n',
        })
        gate_plan._SOURCE_CACHE.clear()
        roots = gate_plan.runtime_roots(self.root, {"docs"})
        self.assertIn(("alpha", "support/walk"), roots)   # the helper reads `docs`
        doc, _ = self.build()
        # a `bin:` address in a node or at either end of an edge
        bins = {n["addr"] for n in doc["nodes"]} | {e["from"] for e in doc["edges"]} \
            | {e["to"] for e in doc["edges"]}
        bins = {b for b in bins if b.startswith("bin:")}
        self.assertIn("bin:alpha::gh_w", bins)
        self.assertEqual([], sorted(b for b in bins if "support" in b))

    def test_graph_references_of_a_harness_match_rule_3_like_the_floor(self):
        """tb-s1c m-2: rules 4/5 plan the rule-3 set of every reader unit
        (`_unit_users`): a folded helper stands for its whole harness, a
        `#[path]`-shared file for every binary that compiles it in. The graph
        hung `references` on the unit alone -- parity with the floor now for
        folded and shared readers too, not only flat ones."""
        make_tree(self.root, {
            "crates/alpha/tests/h/main.rs": "mod helper;\nmod t1;\n",
            "crates/alpha/tests/h/helper.rs":
                'pub fn tpl() -> &\'static str { "templates/cogny/config.json" }\n'
                'pub fn ex() -> &\'static str { "examples/demo/README.md" }\n',
            "crates/alpha/tests/h/t1.rs": "use crate::helper;\n#[test]\nfn t() { helper::tpl(); }\n",
            "crates/alpha/tests/gh_s.rs":
                '#[test]\nfn t() {\n    let _ = "templates/memory-hive/template.json";\n'
                '    let _ = "examples/demo/README.md";\n}\n',
            "crates/alpha/tests/h2/main.rs": '#[path = "../gh_s.rs"]\nmod s;\n',
            "examples/demo/README.md": "# demo\n",
        })
        gate_plan._SOURCE_CACHE.clear()
        doc, _ = self.build()

        def term(addr):
            crate, unit = addr[len("bin:"):].split("::", 1)
            return gate_plan._binary_id(crate, unit) if "/" in unit else "binary_id(=%s)" % (
                addr[len("bin:"):])

        refs = self.edges(doc, "references")
        for kind, name, floor in (
                ("template", "cogny", gate_plan._tests_referencing_template),
                ("template", "memory-hive", gate_plan._tests_referencing_template),
                ("example", "demo", gate_plan._tests_referencing_example)):
            with self.subTest("%s:%s" % (kind, name)):
                want = set(floor(self.root, name))
                got = {term(src) for src, dst in refs
                       if dst == "%s:%s" % (kind, name) and src.startswith("bin:")}
                self.assertEqual(got, want)
                self.assertTrue(want & {"binary_id(=alpha::h)", "binary_id(=alpha::h2)"}, want)

    def test_graph_reads_skip_prose_comments_like_the_resolver(self):
        """#1160, V2-R1-1: a path a comment of any form names -- doc-comment
        code blocks and a unit that walks an ancestor included -- is no `reads`
        edge; in code (a literal, also after a `//` inside one, `include_str!`)
        it is -- and the graph agrees with `_tests_reading_path` on every case."""
        extra = {
            "crates/alpha/tests/c1_prose.rs":
                "//! not named `*_e2e`: `docs/guide.md` says why\n"
                "/* docs/guide.md */\n#[test]\nfn t() {} // docs/guide.md\n",
            "crates/alpha/tests/c2_doctest.rs":
                "//! ```\n//! std::fs::read(\"docs/guide.md\");\n//! ```\n#[test]\nfn t() {}\n",
            "crates/alpha/tests/c3_url.rs":
                'const U: &str = "http://h"; const R: &str = "docs/guide.md";\n'
                "#[test]\nfn t() {}\n",
            # V2-R1-1 (was tb-s2 B-1): prose + a walk of an ancestor reads nothing
            "crates/alpha/tests/c4_walk.rs":
                "//! walks the pages; `docs/guide.md` is one\n"
                '#[test]\nfn t() { let _ = std::fs::read_dir(root().join("docs")); }\n',
            # re-check r1-kit M5: an ignore reason is prose as well
            "crates/alpha/tests/c5_ignore.rs":
                '#[ignore = "browser lock: (docs/guide.md)"]\n#[test]\nfn t() {}\n',
            "crates/alpha/tests/c6_message.rs":
                '#[test]\nfn t() { assert!(1 == 1, "see docs/guide.md"); }\n',
            # V2-R1-1: `//`, `///` and `/* */`, an indented doc line -- prose
            "crates/alpha/tests/c7_comments.rs":
                "// docs/guide.md\n/// docs/guide.md\n///\n///     docs/guide.md\n"
                "/* docs/guide.md */\n#[test]\nfn t() {}\n",
            # the same comments plus code that opens or embeds the path: reads
            "crates/alpha/tests/c8_read.rs":
                "// docs/guide.md\n/// docs/guide.md\n/* docs/guide.md */\n"
                '#[test]\nfn t() { let _ = std::fs::read_to_string("docs/guide.md"); }\n',
            "crates/alpha/tests/c9_include.rs":
                "// docs/guide.md\n/// docs/guide.md\n/* docs/guide.md */\n"
                'const G: &str = include_str!("../../../docs/guide.md");\n#[test]\nfn t() {}\n',
        }
        make_tree(self.root, extra)
        gate_plan._SOURCE_CACHE.clear()
        doc, _ = self.build()
        got = {src for src, dst in self.edges(doc, "reads") if dst == "file:docs/guide.md"}
        want = {"bin:%s" % graph.binary_id_of_term(t)
                for t in gate_plan._tests_reading_path(self.root, "docs/guide.md")}
        self.assertEqual(got, want)
        self.assertNotIn("bin:alpha::c1_prose", got)
        self.assertNotIn("bin:alpha::c5_ignore", got)
        self.assertNotIn("bin:alpha::c6_message", got)
        self.assertNotIn("bin:alpha::c2_doctest", got)
        self.assertNotIn("bin:alpha::c4_walk", got)
        self.assertNotIn("bin:alpha::c7_comments", got)
        self.assertIn("bin:alpha::c3_url", got)
        self.assertIn("bin:alpha::c8_read", got)
        self.assertIn("bin:alpha::c9_include", got)

    def test_graph_catalogue_walkers_read_the_top_level_files(self):
        """V2-R1-1: a test that enumerates the catalogue reads templates/README.md
        (no template owns it) -- the same set as the resolver's runtime rule."""
        extra = {
            "templates/README.md": "# Library\n",
            "crates/alpha/tests/k1_wide.rs":
                '#[test]\nfn t() { for e in std::fs::read_dir(root().join("templates")) {} }\n',
            "crates/alpha/tests/k2_one.rs":
                '#[test]\nfn t() { let _ = root().join("templates").join("talky"); }\n',
        }
        make_tree(self.root, extra)
        gate_plan._SOURCE_CACHE.clear()
        doc, _ = self.build()
        got = {src for src, dst in self.edges(doc, "reads") if dst == "file:templates/README.md"}
        self.assertIn("bin:alpha::k1_wide", got)
        self.assertNotIn("bin:alpha::k2_one", got)
        want = {"bin:%s" % graph.binary_id_of_term(t)
                for t in gate_plan._tests_reading_at_runtime(self.root, "templates/README.md")}
        self.assertIn("bin:alpha::k1_wide", want)
        self.assertLessEqual(want, got)

    def test_graph_pins_find_scaffold_versions(self):
        doc, _ = self.build()
        pins = {(e["from"], e["to"]): e for e in doc["edges"] if e["kind"] == "pins"}
        rel = "workshop/evals/builder-scenarios/scaffolds/eyes-min/main/os/config.json"
        good = pins[("pin:%s#talky" % rel, "template:talky")]
        stale = pins[("pin:%s#cogny" % rel, "template:cogny")]
        self.assertEqual((good["state"], good["data"]), ("resolved", {"version": "1.2.0"}))
        self.assertEqual((stale["state"], stale["data"]), ("broken", {"version": "1.9.0"}))
        self.assertIn(stale, graph.broken(doc))
        # the builder case reaches both templates through its scaffold
        refs = self.edges(doc, "references")
        self.assertIn(("case:builder/A1", "template:talky"), refs)
        self.assertIn(("case:builder/A1", "template:cogny"), refs)
        self.assertIn(("case:builder/A1", "station:scenarios:builder"), self.edges(doc, "needs"))
        self.assertIn(("case:memory/C1", "station:scenarios:memory"), self.edges(doc, "needs"))

    def test_graph_use_resolves_within_crate_external_otherwise(self):
        doc, _ = self.build()
        use = {(e["from"], e["to"]): e["state"] for e in doc["edges"] if e["kind"] == "use"}
        self.assertEqual(use[("file:crates/alpha/src/net.rs", "file:crates/alpha/src/store.rs")],
                         "resolved")
        self.assertEqual(use[("file:crates/alpha/src/net.rs", "crate:serde")], "external")
        self.assertEqual(use[("file:crates/beta/tests/b1_uses_alpha.rs",
                              "file:crates/alpha/src/store.rs")], "resolved")
        self.assertEqual(use[("file:crates/alpha/tests/a1_reads_doc.rs",
                              "file:crates/alpha/src/net.rs")], "resolved")
        self.assertEqual(graph._expand_use("crate::{a::b, c::{d, self}}"),
                         [["crate", "a", "b"], ["crate", "c", "d"], ["crate", "c"]])

    def test_code_hash_moves_with_each_resolver_file(self):
        """Re-check tb-s2 m-N10: the hash covers the bytes of `gate_plan.py`
        AND `graph.py` -- one changed byte in either moves it."""
        files = {}
        for name, mod in (("gate_plan", gate_plan), ("graph", graph)):
            files[name] = pathlib.Path(self.root, name + ".py")
            files[name].write_bytes(pathlib.Path(mod.__file__).read_bytes())

        def hashed():
            with mock.patch.object(gate_plan, "__file__", str(files["gate_plan"])), \
                    mock.patch.object(graph, "__file__", str(files["graph"])):
                return graph.code_hash()

        base = hashed()
        self.assertEqual(base, graph.code_hash())
        for name, path in files.items():
            orig = path.read_bytes()
            path.write_bytes(orig + b"\n")
            self.assertNotEqual(base, hashed(), name)
            path.write_bytes(orig)
        self.assertEqual(base, hashed())

    def test_graph_json_schema_and_addresses(self):
        doc, _ = self.build()
        self.assertEqual(doc["schema"], "meclaw.pipeline.graph/1")
        self.assertEqual(set(doc), {"schema", "built_at", "rev", "code", "sources", "nodes", "edges"})
        self.assertEqual(graph.code_hash(), doc["code"])
        kinds = {"crate", "file", "bin", "test", "template", "example", "case", "pin", "doc",
                 "class", "station"}
        for n in doc["nodes"]:
            self.assertEqual(set(n), {"addr", "kind", "name", "source"})
            self.assertTrue(n["addr"].startswith(n["kind"] + ":"), n)
            self.assertIn(n["kind"], kinds)
        for e in doc["edges"]:
            self.assertIn(e["kind"], graph.EDGE_KINDS)
            self.assertIn(e["class"], graph.EDGE_CLASSES)
            self.assertIn(e["state"], graph.EDGE_STATES)
        for s in doc["sources"]:
            self.assertEqual(set(s), {"addr", "path", "kind", "mark"})
            self.assertRegex(s["mark"], r"^[0-9a-f]{40}$")
        addrs = {n["addr"] for n in doc["nodes"]}
        for a in ("crate:alpha", "bin:alpha", "bin:alpha::a1_reads_doc",
                  "test:alpha::a1_reads_doc::reads", "template:talky", "case:builder/A1",
                  "doc:plans/adr/0001-net.md", "file:crates/alpha/src/net.rs"):
            self.assertIn(a, addrs)
        self.assertIn(("doc:plans/adr/0001-net.md", "test:alpha::net::net_ok"),
                      self.edges(doc, "pinned_by"))
        self.assertEqual(graph.blob_sha(b"hello\n"), "ce013625030ba8dba906f756967f9e9ca394464a")

    def test_graph_dependents_depth(self):
        doc, _ = self.build()
        ix = graph.Index(doc)
        store = "file:crates/alpha/src/store.rs"
        one = graph.dependents(ix, store, 1, kinds={"use"})
        self.assertEqual(set(one), {"file:crates/alpha/src/net.rs",
                                    "file:crates/beta/tests/b1_uses_alpha.rs"})
        two = graph.dependents(ix, store, 2, kinds={"use"})
        self.assertIn("file:crates/alpha/tests/a1_reads_doc.rs", two)
        self.assertEqual(two["file:crates/alpha/tests/a1_reads_doc.rs"], 2)
        p = graph.path(ix, "bin:alpha::a1_reads_doc", "file:docs/guide.md")
        self.assertEqual([x[0] for x in p], ["bin:alpha::a1_reads_doc", "file:docs/guide.md"])
        self.assertEqual(graph.deps(ix, "crate:beta", kinds={"depends"}), ["crate:alpha"])
        st = graph.stats(doc)
        self.assertEqual(st["nodes"], len(doc["nodes"]))

    def test_graph_build_incremental_by_blob_sha(self):
        cache = {}
        _, first = self.build(cache=cache)
        self.assertEqual(first["extracted"], first["sources"])
        _, again = self.build(cache=cache)
        self.assertEqual(again["extracted"], 0)
        pathlib.Path(self.root, "crates/alpha/src/store.rs").write_text(
            "pub struct Db;\npub struct Other;\n", encoding="utf-8")
        _, third = self.build(cache=cache)
        self.assertEqual(third["extracted"], 1)
        # persistence round-trip keeps the cache and the latest pointer
        with tempfile.TemporaryDirectory() as out:
            g, _ = graph.build(self.root, cache=cache, stations=False)
            path = graph.save(g, "rev1", out, cache)
            self.assertEqual(graph.load(str(path))["rev"], "rev1")
            self.assertEqual(graph.load_cache(out), json.loads(json.dumps(cache)))

    def test_graph_save_writes_through_unique_tmp_files(self):
        # Fixed tmp names in the shared graph dir let two writers
        # replace each other's half-written file.
        class G:
            def to_json(self, rev, built_at):
                return {"rev": rev, "built_at": built_at, "nodes": [], "edges": []}

        seen, real = [], os.replace
        with tempfile.TemporaryDirectory() as out:
            def spy(src, dst):
                seen.append(os.fspath(src))
                real(src, dst)
            with mock.patch.object(graph.os, "replace", side_effect=spy):
                graph.save(G(), "rev1", out, {"k": 1})
                graph.save(G(), "rev1", out, {"k": 2})
            self.assertEqual(6, len(seen))
            self.assertEqual(len(seen), len(set(seen)), "every write has its own tmp file")
            for src in seen:
                self.assertEqual(os.path.realpath(out), os.path.realpath(os.path.dirname(src)))
            self.assertEqual(sorted(os.listdir(out)),
                             sorted(["rev1.json", "latest.json", graph.CACHE_FILE]))
            self.assertEqual({"k": 2}, graph.load_cache(out))

    def test_graph_station_probe_reads_the_resolver(self):
        doc, _ = self.build(stations=True)
        trig = self.edges(doc, "triggered_by")
        self.assertIn(("station:tests", "class:rust_src"), trig)
        self.assertIn(("station:fmt", "class:rust_src"), trig)


if __name__ == "__main__":
    unittest.main()
