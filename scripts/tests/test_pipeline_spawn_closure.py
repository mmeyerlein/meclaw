"""Tests of the spawned-binary closure (`scripts/pipeline/spawn_closure.py`, S1b, GH #1165).

The coverage map saw no code that runs only in a child process a test starts
with `CARGO_BIN_EXE_<name>`: the export never got the child's executable
(`objects_from_nextest_list` asked for kind `bin`, nextest says `bin-exe`).
The narrowing therefore dropped tests that spawn a binary whose code changed.

Build half: the export gets every executable of a bin name, the map carries a
`subprocess` marker per bin. Narrowing half: without a marker that proves a
spawner, every change in the bin's crate closure selects that spawner.
"""

import argparse
import datetime
import json
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
from pipeline import coverage, graph, impact, spawn_closure  # noqa: E402
from scripts.tests import store_isolation  # noqa: E402
from test_pipeline_graph import make_tree  # noqa: E402

_METRICS_URL = mock.patch.dict(os.environ, {"MECLAW_PIPELINE_METRICS_URL": ""})


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()
    _METRICS_URL.start()


def tearDownModule():  # noqa: N802 -- unittest's name
    _METRICS_URL.stop()
    store_isolation.restore()


SPAWN = 'std::process::Command::new(env!("CARGO_BIN_EXE_meclaw"))'
FILES = {
    "Cargo.toml": '[workspace]\nmembers = ["crates/cells", "crates/cli", "crates/pkg"]\n',
    "crates/cells/Cargo.toml": '[package]\nname = "cells"\nversion = "0.1.0"\n',
    "crates/cells/src/lib.rs": "pub mod wire;\n",
    "crates/cells/src/wire.rs": "pub fn send() {}\n",
    "crates/cells/tests/gh1_delivery.rs": "use cells::wire::send;\n\n"
                                          "#[test]\nfn d() {\n    send();\n}\n",
    # cli: the bin `meclaw` (src/main.rs) over its lib, which needs cells
    "crates/cli/Cargo.toml": '[package]\nname = "cli"\nversion = "0.1.0"\n\n'
                             '[[bin]]\nname = "meclaw"\npath = "src/main.rs"\n\n'
                             '[dependencies]\ncells = { path = "../cells" }\n',
    "crates/cli/src/lib.rs": "pub mod ask;\n\npub fn entry() {}\n",
    "crates/cli/src/ask.rs": "pub fn ask() {}\n",
    "crates/cli/src/main.rs": "fn main() {\n    cli::entry();\n}\n",
    "crates/cli/tests/spawn_demo.rs": "#[test]\nfn s() {\n    let _ = %s;\n}\n" % SPAWN,
    "crates/cli/tests/in_process.rs": "use cli::ask::ask;\n\n#[test]\nfn i() {\n    ask();\n}\n",
    "crates/cli/tests/prose.rs": "//! spawns `CARGO_BIN_EXE_<name>` -- in prose only\n\n"
                                 "#[test]\nfn p() {}\n",
    "crates/cli/tests/h/main.rs": "mod via_harness;\nmod quiet;\n",
    "crates/cli/tests/h/via_harness.rs": "#[test]\nfn v() {\n    let _ = %s;\n}\n" % SPAWN,
    "crates/cli/tests/h/quiet.rs": "#[test]\nfn q() {}\n",
    # pkg: a second package that emits the SAME bin name (the meclaw/meclaw-cli shape)
    "crates/pkg/Cargo.toml": '[package]\nname = "pkg"\nversion = "0.1.0"\n\n'
                             '[[bin]]\nname = "meclaw"\npath = "src/main.rs"\n\n'
                             '[dependencies]\ncli = { path = "../cli" }\n',
    "crates/pkg/src/main.rs": "fn main() {\n    cli::entry();\n}\n",
}
SPAWN_DEMO = "binary_id(=cli::spawn_demo)"
VIA_HARNESS = "(binary_id(=cli::h) & test(/^via_harness::/))"
MAP = {
    "schema": "meclaw.pipeline.coverage/1", "rev": "fixture", "built_at": "2026-10-10T02:00:00Z",
    "runner": "lane", "granularity": "module", "secs": 1.0, "tests": 5, "complete": True,
    "binaries_expected": 4, "binaries_measured": 4, "errors": 0,
    "parts": [
        {"binary": "cells::gh1_delivery", "module": None, "files": {"crates/cells/src/wire.rs": 1}},
        {"binary": "cli::in_process", "module": None,
         "files": {"crates/cli/src/ask.rs": 1, "crates/cli/src/lib.rs": 1}},
        # the map as it was: the spawning test shows its own file, nothing of the child
        {"binary": "cli::spawn_demo", "module": None,
         "files": {"crates/cli/tests/spawn_demo.rs": 1}},
        {"binary": "cli::h", "module": "via_harness",
         "files": {"crates/cli/tests/h/via_harness.rs": 1}},
        {"binary": "cli::h", "module": "quiet", "files": {"crates/cli/tests/h/quiet.rs": 1}},
        {"binary": "cli::prose", "module": None, "files": {"crates/cli/tests/prose.rs": 1}},
    ],
}
NOW = datetime.datetime(2026, 10, 10, 8, 0, tzinfo=datetime.timezone.utc)


def _with_parts(cov, *extra, subprocess=None):
    out = dict(cov, parts=list(cov["parts"]) + list(extra))
    if subprocess is not None:
        out["subprocess"] = subprocess
    return out


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = self.tmp.name
        make_tree(self.root, FILES)
        gate_plan._SOURCE_CACHE.clear()
        gate_plan._NO_TEST_CACHE.clear()
        spawn_closure.clear_caches()
        g, _ = graph.build(self.root, stations=False)
        self.doc = g.to_json("fixture", "2026-10-10T00:00:00Z")
        self.ix = graph.Index(self.doc)

    def tearDown(self):
        gate_plan._SOURCE_CACHE.clear()
        gate_plan._NO_TEST_CACHE.clear()
        spawn_closure.clear_caches()
        self.tmp.cleanup()

    def run_impact(self, paths, coverage=MAP, stage="strand", narrowed=None):
        return impact.impact(paths, stage, self.doc, repo=self.root, coverage=coverage, now=NOW,
                             narrowed=narrowed)

    def selected(self, res):
        bins, parts, _ = impact.expand(self.ix, res["tests"])
        return bins, parts


# --- the build half ------------------------------------------------------------

class BuildTest(Base):
    def test_objects_take_the_nextest_kind_bin_exe(self):
        """`cargo nextest list --message-format json` names a bin target
        `bin-exe` (measured on build04, map 7d75ec7a3): asking for `bin` left
        `_bins` empty and no child executable ever reached the export."""
        doc = {"rust-build-meta": {"target-directory": "/t", "non-test-binaries": {
                   "path+file:///w/crates/cli#0.1.0": [
                       {"name": "meclaw", "kind": "bin-exe", "path": "debug/meclaw"}]}},
               "rust-suites": {"cli::spawn_demo": {"binary-path": "/t/debug/deps/spawn_demo-0123"}}}
        self.assertEqual(coverage.objects_from_nextest_list(doc)["_bins"], ["/t/debug/meclaw"])

    def _target(self):
        t = pathlib.Path(self.root, "target", "debug")
        (t / "deps").mkdir(parents=True)
        a = t / "deps" / "meclaw-0123456789abcdef"
        b = t / "deps" / "meclaw-fedcba9876543210"
        for p in (a, b, t / "deps" / "line_json_server-0011223344556677"):
            p.write_text("elf")
            p.chmod(0o755)
        (t / "deps" / "meclaw-0123456789abcdef.d").write_text("dep-info")
        os.link(a, t / "meclaw")   # the uplift keeps ONE of the two packages' bins
        return t

    def test_child_objects_add_every_hashed_copy_once(self):
        t = self._target()
        got = spawn_closure.child_objects([str(t / "meclaw")])
        self.assertEqual(got, [str(t / "meclaw"), str(t / "deps" / "meclaw-fedcba9876543210")])

    def _fake_tools(self, child_files):
        """llvm-profdata merges by name; llvm-cov prints the child's lines only
        when an `-object` of the child executable is on its command line."""
        prof = pathlib.Path(self.root, "llvm-profdata")
        prof.write_text("#!/bin/sh\nwhile [ $# -gt 0 ]; do [ \"$1\" = -o ] && : > \"$2\"; "
                        "shift; done\n")
        prof.chmod(0o755)
        cov = pathlib.Path(self.root, "llvm-cov")
        child = "".join("SF:%s/%s\\nDA:1,1\\nend_of_record\\n" % (self.root, f)
                        for f in child_files)
        cov.write_text("#!/bin/sh\nprintf 'SF:%s/crates/cli/tests/spawn_demo.rs\\nDA:1,1\\n"
                       "end_of_record\\n'\n"
                       "case \"$*\" in *'-object '*meclaw*) printf '%s' ;; esac\n"
                       % (self.root, child))
        cov.chmod(0o755)
        return str(prof), str(cov)

    def _profiles(self):
        d = pathlib.Path(self.root, "profiles", "cli::spawn_demo", "s", "s")
        d.mkdir(parents=True)
        (d.parent / "s.profdata").write_text("")
        return str(pathlib.Path(self.root, "profiles"))

    def _list_doc(self, t):
        return {"rust-build-meta": {"target-directory": str(t.parent), "non-test-binaries": {
                    "cli": [{"name": "meclaw", "kind": "bin-exe", "path": "debug/meclaw"}]}},
                "rust-suites": {"cli::spawn_demo": {
                    "binary-path": str(t / "deps" / "spawn_demo-01"),
                    "testcases": {"s": {}}}}}

    def test_the_map_holds_the_childs_lines_for_the_spawning_test(self):
        """Contract 2, red before: the part of a test that spawns `meclaw`
        carries `ask.rs`, a file only the child ran."""
        t = self._target()
        prof, cov = self._fake_tools(["crates/cli/src/ask.rs", "crates/cli/src/main.rs"])
        objects = coverage.objects_from_nextest_list(self._list_doc(t))
        parts, errors = coverage.aggregate(self._profiles(), objects, self.root, jobs=1,
                                           profdata=prof, llvm_cov=cov)
        self.assertEqual(errors, [])
        files = {p["binary"]: p["files"] for p in parts}["cli::spawn_demo"]
        self.assertIn("crates/cli/src/ask.rs", files)

    def test_the_build_writes_the_marker_only_it_can_prove(self):
        """`coverage aggregate` writes `subprocess`: one entry per bin whose
        executable reached the export, `proven` = parts that ran its root."""
        t = self._target()
        prof, cov = self._fake_tools(["crates/cli/src/ask.rs", "crates/cli/src/main.rs"])
        listing = pathlib.Path(self.root, "list.json")
        listing.write_text(json.dumps(self._list_doc(t)))
        out = pathlib.Path(self.root, "coverage.json")
        args = argparse.Namespace(
            op="aggregate", objects=str(listing), harness="", profiles=self._profiles(),
            root=self.root, jobs=1, profdata=prof, llvm_cov=cov, rev="fixture", runner="lane",
            secs=1.0, nextest_exit=0, nextest_log=None, out=str(out))
        with mock.patch("sys.stdout"):
            self.assertEqual(coverage._cmd_coverage(args), 0)
        sub = json.loads(out.read_text())["subprocess"]
        self.assertEqual(sorted(sub), ["cli::bin/meclaw", "pkg::bin/meclaw"])
        self.assertEqual(sub["cli::bin/meclaw"]["proven"], [["cli::spawn_demo", None]])
        self.assertEqual(sub["cli::bin/meclaw"]["root"], "crates/cli/src/main.rs")
        self.assertEqual(sub["pkg::bin/meclaw"]["proven"], [])

    def test_no_object_no_marker(self):
        parts = [{"binary": "cli::spawn_demo", "module": None,
                  "files": {"crates/cli/src/main.rs": 3}}]
        self.assertEqual(spawn_closure.marker(parts, [], self.root), {})
        got = spawn_closure.marker(parts, ["/t/debug/deps/meclaw-0123456789abcdef"], self.root)
        self.assertEqual(got["cli::bin/meclaw"]["proven"], [["cli::spawn_demo", None]])

    def test_bin_targets_follow_cargos_target_rules(self):
        make_tree(self.root, {"crates/cells/src/bin/fixture.rs": "fn main() {}\n",
                              "crates/cells/src/bin/dirbin/main.rs": "fn main() {}\n"})
        spawn_closure.clear_caches()
        got = {k: v["root"] for k, v in spawn_closure.bin_targets(self.root).items()}
        self.assertEqual(got, {"cells::bin/dirbin": "crates/cells/src/bin/dirbin/main.rs",
                               "cells::bin/fixture": "crates/cells/src/bin/fixture.rs",
                               "cli::bin/meclaw": "crates/cli/src/main.rs",
                               "pkg::bin/meclaw": "crates/pkg/src/main.rs"})


# --- the narrowing half --------------------------------------------------------

class NarrowTest(Base):
    def assert_spawners(self, res, path, key="cli::bin/meclaw"):
        bins, parts = self.selected(res)
        self.assertIn(SPAWN_DEMO, parts, res["tests"])
        self.assertIn(VIA_HARNESS, parts, res["tests"])
        for t in (SPAWN_DEMO, VIA_HARNESS):
            self.assertIn("spawn:unproven %s file:%s" % (key, path), res["reason"].get(t, ()), t)

    def test_closure_change_selects_every_spawner_without_marker(self):
        """Contract 3, red before: `wire.rs` is in the closure of `meclaw`
        (cli needs cells); the map, blind to the child, planned only gh1."""
        res = self.run_impact(["crates/cells/src/wire.rs"])
        self.assertTrue(res["coverage"]["used"], res["notes"])
        self.assertNotIn("rdeps(cells)", res["tests"])
        self.assertIn("binary_id(=cells::gh1_delivery)", res["tests"])
        self.assert_spawners(res, "crates/cells/src/wire.rs")
        # module-exact for a folded spawner; prose and the quiet sibling stay out
        bins, parts = self.selected(res)
        self.assertNotIn("cli::prose", bins)
        self.assertNotIn("binary_id(=cli::h)", parts)

    def test_a_lib_file_of_the_bins_own_package_selects_the_spawners(self):
        res = self.run_impact(["crates/cli/src/ask.rs"])
        self.assertTrue(res["coverage"]["used"], res["notes"])
        self.assertIn("binary_id(=cli::in_process)", res["tests"])
        self.assert_spawners(res, "crates/cli/src/ask.rs")

    def test_marker_without_proof_stays_fail_closed(self):
        sub = {"cli::bin/meclaw": {"crate": "cli", "name": "meclaw",
                                   "root": "crates/cli/src/main.rs", "proven": []}}
        res = self.run_impact(["crates/cells/src/wire.rs"],
                              coverage=_with_parts(MAP, subprocess=sub))
        self.assert_spawners(res, "crates/cells/src/wire.rs")

    def test_a_proven_spawner_is_left_to_the_map(self):
        """With the marker proving both spawners, the map decides: the child
        never ran `wire.rs` -> not selected; it ran `ask.rs` -> selected by cover."""
        sub = {"cli::bin/meclaw": {"crate": "cli", "name": "meclaw",
                                   "root": "crates/cli/src/main.rs",
                                   "proven": [["cli::spawn_demo", None],
                                              ["cli::h", "via_harness"]]}}
        cov = _with_parts(dict(MAP, parts=[p for p in MAP["parts"]
                                           if p["binary"] != "cli::spawn_demo"]),
                          {"binary": "cli::spawn_demo", "module": None,
                           "files": {"crates/cli/src/main.rs": 1, "crates/cli/src/ask.rs": 2}},
                          subprocess=sub)
        res = self.run_impact(["crates/cells/src/wire.rs"], coverage=cov)
        self.assertNotIn(SPAWN_DEMO, res["tests"])
        self.assertNotIn(VIA_HARNESS, res["tests"])
        res = self.run_impact(["crates/cli/src/ask.rs"], coverage=cov)
        self.assertIn("cover:file:crates/cli/src/ask.rs", res["reason"][SPAWN_DEMO])
        self.assertFalse(any(r.startswith("spawn:") for r in res["reason"][SPAWN_DEMO]))

    def test_a_proof_for_the_other_same_named_bin_does_not_reach_its_root(self):
        """The uplift keeps one `debug/meclaw`: the proof may be for pkg's bin.
        It covers the shared closure, never cli's own `main.rs`."""
        sub = {"pkg::bin/meclaw": {"crate": "pkg", "name": "meclaw",
                                   "root": "crates/pkg/src/main.rs",
                                   "proven": [["cli::spawn_demo", None],
                                              ["cli::h", "via_harness"]]}}
        cov = _with_parts(MAP, subprocess=sub)
        res = self.run_impact(["crates/cells/src/wire.rs"], coverage=cov)
        self.assertNotIn(SPAWN_DEMO, res["tests"])
        terms = spawn_closure.fail_closed_terms(
            ["crates/cli/src/main.rs"], cov, self.root, lambda c: impact.rdeps(self.ix, c))
        self.assertIn("spawn:unproven cli::bin/meclaw file:crates/cli/src/main.rs",
                      terms[SPAWN_DEMO])

    def test_narrowed_merge_and_integration_get_it_too(self):
        for stage in ("merge", "integration"):
            res = self.run_impact(["crates/cells/src/wire.rs"], stage=stage,
                                  narrowed="narrow:%s fixture" % stage)
            self.assertTrue(res["coverage"]["used"], (stage, res["notes"]))
            self.assert_spawners(res, "crates/cells/src/wire.rs")

    def test_a_kept_rdeps_is_not_repeated(self):
        """rdeps(cli) still planned (a cli file the map does not know): its
        binaries are in, no spawn term on top."""
        make_tree(self.root, {"crates/cli/src/fresh.rs": "pub fn fresh() {}\n"})
        res = self.run_impact(["crates/cli/src/fresh.rs", "crates/cells/src/wire.rs"])
        self.assertIn("rdeps(cli)", res["tests"])
        self.assertFalse(any(r.startswith("spawn:") for rs in res["reason"].values() for r in rs),
                         res["reason"])

    def test_a_dynamic_name_is_never_proven(self):
        make_tree(self.root, {"crates/cli/tests/dyn_spawn.rs":
                              '#[test]\nfn d() {\n    let _ = std::env::var(concat!('
                              '"CARGO_BIN_EXE_", "meclaw"));\n}\n'})
        spawn_closure.clear_caches()
        gate_plan._SOURCE_CACHE.clear()
        sub = {"cli::bin/meclaw": {"crate": "cli", "name": "meclaw",
                                   "root": "crates/cli/src/main.rs",
                                   "proven": [["cli::dyn_spawn", None]]}}
        terms = spawn_closure.fail_closed_terms(
            ["crates/cells/src/wire.rs"], _with_parts(MAP, subprocess=sub), self.root,
            lambda c: impact.rdeps(self.ix, c))
        self.assertIn("spawn:unproven cli::bin/* file:crates/cells/src/wire.rs",
                      terms["binary_id(=cli::dyn_spawn)"])

    def test_a_file_outside_every_closure_adds_nothing(self):
        make_tree(self.root, {"crates/solo/Cargo.toml":
                              '[package]\nname = "solo"\nversion = "0.1.0"\n',
                              "crates/solo/src/lib.rs": "pub fn s() {}\n"})
        terms = spawn_closure.fail_closed_terms(
            ["crates/solo/src/lib.rs", "docs/x.md"], MAP, self.root,
            lambda c: impact.rdeps(self.ix, c))
        self.assertEqual(terms, {})


if __name__ == "__main__":
    unittest.main()
