"""Tests of the coverage map (`scripts/pipeline/coverage.py` and
`coverage_runner.sh`, W6, GH #1120).

No cargo and no LLVM here: the runner is driven with a fake test command and a
fake `llvm-profdata`, the parser reads an lcov fixture, and the staleness check
writes into a store in a temporary directory.
"""

import datetime
import io
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import types
import unittest
from unittest import mock

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS))

from pipeline import coverage, graph, store  # noqa: E402
from scripts.tests import kit_time, kit_tree, store_isolation  # noqa: E402

RUNNER = SCRIPTS / "pipeline" / "coverage_runner.sh"

LCOV = """SF:/w/tree/crates/alpha/src/net.rs
FN:3,f
DA:3,1
DA:4,0
LF:2
LH:1
end_of_record
SF:/w/tree/crates/alpha/src/store.rs
DA:1,0
DA:2,0
LF:2
LH:0
end_of_record
SF:/w/tree/crates/alpha/src/lib.rs
DA:1,5
DA:2,2
DA:3,0
end_of_record
SF:/home/u/.cargo/registry/src/serde-1.0/src/lib.rs
LF:10
LH:9
end_of_record
"""

FAKE_PROFDATA = """#!/bin/sh
# fake llvm-profdata: `merge -sparse <in...> -o <out>` writes the inputs' names
# next to <out>; the inputs precede `-o`, so they are collected first
out=""
ins=""
while [ $# -gt 0 ]; do
  case "$1" in -o) out=$2; shift 2 ;; merge|-sparse) shift ;; *) ins="$ins$1
"; shift ;; esac
done
[ -n "$out" ] || exit 2
printf '%s' "$ins" > "$out.inputs"
: > "$out"
"""


# gate-selftest loads every module into one process: no fixture event of this
# module may reach the metrics target of the caller's environment.
_METRICS_URL = mock.patch.dict(os.environ, {"MECLAW_PIPELINE_METRICS_URL": ""})


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()
    _METRICS_URL.start()


def tearDownModule():  # noqa: N802 -- unittest's name
    _METRICS_URL.stop()
    store_isolation.restore()


class CoverageTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = pathlib.Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def test_coverage_lcov_to_covers_edges(self):
        files = coverage.parse_lcov(LCOV, "/w/tree")
        self.assertEqual(files, {"crates/alpha/src/net.rs": 1, "crates/alpha/src/store.rs": 0,
                                 "crates/alpha/src/lib.rs": 2})
        cov = coverage.coverage_map("r1", "lane", [
            {"binary": "alpha", "module": "net", "files": {p: n for p, n in files.items() if n}},
            {"binary": "beta::b1", "module": None, "files": {"crates/alpha/src/net.rs": 4}},
        ], 12.0, 3, built_at="2026-10-08T00:00:00Z")
        self.assertEqual(cov["granularity"], "module")
        g = graph.Graph()
        graph.add_coverage(g, cov)
        covers = {(e["from"], e["to"]): e["data"] for e in g.edges.values() if e["kind"] == "covers"}
        self.assertEqual(covers, {
            ("test:alpha::net", "file:crates/alpha/src/net.rs"):
                {"lines": 1, "since": "2026-10-08T00:00:00Z"},
            ("test:alpha::net", "file:crates/alpha/src/lib.rs"):
                {"lines": 2, "since": "2026-10-08T00:00:00Z"},
            ("bin:beta::b1", "file:crates/alpha/src/net.rs"):
                {"lines": 4, "since": "2026-10-08T00:00:00Z"},
        })
        self.assertEqual(coverage.granularity_of("alpha::a1", set()), "binary")
        self.assertEqual(coverage.granularity_of("alpha", set()), "module")
        self.assertEqual(coverage.granularity_of("cells::harness", {"cells::harness"}), "module")

    def _run_runner(self, env_extra, argv):
        fake = self.dir / "llvm-profdata"
        fake.write_text(FAKE_PROFDATA)
        fake.chmod(0o755)
        base = self.dir / "profiles"
        env = {"PATH": os.environ.get("PATH", store_isolation.trapped_path()),
               "MECLAW_COVERAGE_PROFILE_DIR": str(base), "MECLAW_LLVM_PROFDATA": str(fake)}
        env.update(env_extra)
        cwd = self.dir / "cwd"
        cwd.mkdir(exist_ok=True)
        r = subprocess.run(["sh", str(RUNNER)] + argv, env=env, capture_output=True, text=True,
                           cwd=cwd)
        # nothing lands in the caller's working directory, only under the temp dir
        self.assertEqual(sorted(f.name for f in cwd.iterdir()), [])
        return r, base

    def test_coverage_runner_names_profile_by_binary_and_module(self):
        seen = self.dir / "seen"
        test_cmd = ["sh", "-c", 'printf "%s" "$LLVM_PROFILE_FILE" > "$0"; '
                                ': > "$(dirname "$LLVM_PROFILE_FILE")/4242-x.profraw"; exit 3',
                    str(seen)]
        r, base = self._run_runner({"NEXTEST_BINARY_ID": "meclaw-cells::harness_web",
                                    "NEXTEST_TEST_NAME": "gh974_case::the_case"}, test_cmd)
        self.assertEqual(r.returncode, 3, r.stderr)  # the test's exit code passes through
        want = base / "meclaw-cells::harness_web" / "gh974_case" / "gh974_case::the_case"
        # %c: continuous mode -- a test killed by SIGKILL still leaves its counters
        self.assertEqual(seen.read_text(), "%s/%%p-%%m%%c.profraw" % want)
        merged = pathlib.Path(str(want) + ".profdata")
        self.assertTrue(merged.exists())
        self.assertEqual(pathlib.Path(str(merged) + ".inputs").read_text().split(),
                         [str(want / "4242-x.profraw")])
        self.assertFalse(want.exists())  # the raw profiles went
        parts = coverage.profile_parts(base, {"meclaw-cells::harness_web"})
        self.assertEqual(list(parts), [("meclaw-cells::harness_web", "gh974_case")])

        # a bin target's id carries a `/` (`meclaw::bin/meclaw`): one directory
        # level, never two -- the map names the binary by its full id again
        r, base = self._run_runner({"NEXTEST_BINARY_ID": "meclaw::bin/meclaw",
                                    "NEXTEST_TEST_NAME": "tests::t"},
                                   ["sh", "-c", ': > "$(dirname "$LLVM_PROFILE_FILE")/1-x.profraw"'])
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertTrue((base / "meclaw::bin@meclaw" / "tests").is_dir())
        self.assertIn(("meclaw::bin/meclaw", None), coverage.profile_parts(base))
        self.assertEqual(coverage.profiles_per_binary(base)["meclaw::bin/meclaw"], 1)
        self.assertFalse((base / "meclaw::bin").exists())

        # fallback without the nextest variables: one directory per binary
        r, base = self._run_runner({}, ["sh", "-c", "exit 0", "/t/debug/deps/gh1_x-0123456789abcdef"])
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertTrue((base / "sh" / "_binary").is_dir())

        # a `--list` call keeps no per-test profile
        r, base = self._run_runner({"NEXTEST_BINARY_ID": "a::b"}, ["sh", "-c", "exit 0", "--list"])
        self.assertTrue((base / "_list").is_dir())
        self.assertFalse((base / "a::b").exists())

    def test_coverage_host_relocates_counters_for_continuous_mode(self):
        # `%c` (continuous mode) on Linux needs runtime counter relocation in the
        # instrumented build, or every profile write fails ("Neither
        # __llvm_profile_counter_bias ...") and the map has 0 profiles (run 1, build04)
        host = (SCRIPTS / "pipeline" / "coverage_host.sh").read_text()
        flags = [ln for ln in host.splitlines() if ln.startswith("export RUSTFLAGS=")]
        self.assertEqual(len(flags), 1, flags)
        for want in ("-C instrument-coverage", "-C llvm-args=-runtime-counter-relocation",
                     "--cfg tokio_unstable"):
            self.assertIn(want, flags[0])
        self.assertIn('LLVM_PROFILE_FILE="$C/profiles/_build/%p-%m%c.profraw"', host)
        self.assertIn('LLVM_PROFILE_FILE="$dir/%p-%m%c.profraw"', RUNNER.read_text())

    def test_coverage_stale_after_7_days_sets_finding(self):
        db_path = self.dir / "pipeline.db"
        now = datetime.datetime(2026, 10, 16, 3, 0, tzinfo=datetime.timezone.utc)
        fresh = {"rev": "r1", "built_at": "2026-10-10T02:00:00Z"}
        old = {"rev": "r0", "built_at": "2026-10-08T02:00:00Z"}
        self.assertFalse(coverage.check_stale(fresh, now=now, db_path=db_path))
        self.assertTrue(coverage.check_stale(old, now=now, db_path=db_path))
        db = store.open_ro(db_path)
        try:
            rows = db.execute("SELECT kind, event, data FROM journal").fetchall()
        finally:
            store.close(db)
        self.assertEqual(len(rows), 1)
        kind, event, data = rows[0][0], rows[0][1], json.loads(rows[0][2])
        self.assertEqual((kind, event), ("finding", "process_finding"))
        self.assertEqual((data["rule"], data["subject"], data["threshold"]),
                         ("coverage_stale", "r0", 7))
        cov = coverage.coverage_map("r2", "lane", [
            {"binary": "a", "module": "m", "files": {"x.rs": 1, "y.rs": 2}}], 5.0, 2)
        data = coverage.built(cov, db_path=db_path)
        self.assertEqual((data["files"], data["edges"], data["granularity"]), (2, 2, "module"))

    def test_coverage_built_names_the_browsers_opt_out(self):
        """Re-check tv-bn M7d M1: `lane_sync.sh cmd_coverage` writes the
        browsers opt-out of the run into coverage.json; the journal row
        `coverage_built` (the store's record of the map) carries it -- a map
        of a run without the browser proof never reads like a full one. Unset
        stays unset, an empty opt-out ("") is kept as one."""
        for n, given in enumerate(("0", "", None)):
            db_path = self.dir / ("built-%d.db" % n)
            cov = coverage.coverage_map("r%d" % n, "lane", [
                {"binary": "a", "module": "m", "files": {"x.rs": 1}}], 1.0, 1)
            if given is not None:
                cov["browsers_opt_out"] = given
            data = coverage.built(cov, db_path=db_path)
            db = store.open_ro(db_path)
            try:
                rows = [json.loads(r[0]) for r in
                        db.execute("SELECT data FROM journal WHERE event = 'coverage_built'")]
            finally:
                store.close(db)
            with self.subTest(given=given):
                self.assertEqual(1, len(rows))
                for got in (data, rows[0]):
                    if given is None:
                        self.assertNotIn("browsers_opt_out", got)
                    else:
                        self.assertEqual(given, got["browsers_opt_out"])

    def test_coverage_objects_from_nextest_list(self):
        doc = {"rust-build-meta": {"target-directory": "/t",
                                   "non-test-binaries": {"meclaw 0.1": [
                                       {"name": "meclaw", "kind": "bin", "path": "debug/meclaw"}]}},
               "rust-suites": {"alpha": {"binary-path": "/t/debug/deps/alpha-0123"},
                               "alpha::a1": {"binary-path": "/t/debug/deps/a1-0123"}}}
        self.assertEqual(coverage.objects_from_nextest_list(doc), {
            "alpha": "/t/debug/deps/alpha-0123", "alpha::a1": "/t/debug/deps/a1-0123",
            "_bins": ["/t/debug/meclaw"]})

    def test_coverage_map_complete_only_when_every_binary_measured(self):
        parts = [{"binary": "a", "module": "m", "files": {"x.rs": 1}},
                 {"binary": "b::t", "module": None, "files": {"y.rs": 1}}]
        cov = coverage.coverage_map("r1", "lane", parts, 1.0, 2, expected=2, errors=[], run_ok=True)
        self.assertTrue(cov["complete"])
        self.assertEqual((cov["binaries_expected"], cov["binaries_measured"], cov["errors"]),
                         (2, 2, 0))
        self.assertEqual(cov["rev"], "r1")
        short = coverage.coverage_map("r1", "lane", parts[:1], 1.0, 1, expected=2, errors=[],
                                      run_ok=True)
        self.assertFalse(short["complete"])
        broken = coverage.coverage_map("r1", "lane", parts, 1.0, 2, expected=2,
                                       errors=["export b::t: exit 1"], run_ok=True)
        self.assertFalse(broken["complete"])
        self.assertEqual(broken["errors"], 1)
        self.assertEqual(broken["error_list"], ["export b::t: exit 1"])
        killed = coverage.coverage_map("r1", "lane", parts, 1.0, 2, expected=2, errors=[],
                                       run_ok=False)
        self.assertFalse(killed["complete"])
        # a map without the field (written before the fix) is never complete
        self.assertFalse(coverage.is_complete({"rev": "r1", "parts": parts}))

    def test_coverage_map_complete_by_set_not_by_count(self):
        # review minor: two measured binaries for two expected ones is not
        # complete when one of them is a stranger and an expected one is missing
        parts = [{"binary": "a::t", "module": None, "files": {"x.rs": 1}},
                 {"binary": "stranger::t", "module": None, "files": {"y.rs": 1}}]
        cov = coverage.coverage_map("r1", "lane", parts, 1.0, 2, errors=[], run_ok=True,
                                    expected_ids=["a::t", "b::t"])
        self.assertEqual((cov["binaries_expected"], cov["binaries_measured"]), (2, 2))
        self.assertFalse(cov["complete"])
        self.assertTrue(any("1/2 binaries measured (missing: b::t)" == r for r in cov["incomplete"]),
                        cov["incomplete"])
        whole = coverage.coverage_map("r1", "lane", parts, 1.0, 2, errors=[], run_ok=True,
                                      expected_ids=["a::t", "stranger::t"])
        self.assertTrue(whole["complete"], whole["incomplete"])

    def test_a_broken_map_file_is_one_line_and_exit_2(self):
        # review minor: a broken coverage file was a traceback
        cases = {"truncated": '{"rev": ', "no parts": '{"rev": "r1"}', "a list": "[]"}
        for name, text in cases.items():
            path = self.dir / ("cov-%s.json" % name.replace(" ", "-"))
            path.write_text(text)
            with self.subTest(name):
                cov, why = coverage.load_map(str(path))
                self.assertIsNone(cov)
                self.assertIn("broken coverage map", why)
                for op in ("stale", "journal"):
                    r = subprocess.run([sys.executable, str(SCRIPTS / "pipeline.py"), "coverage",
                                        op, "--map", str(path), "--db", str(self.dir / "p.db")],
                                       capture_output=True, text=True)
                    self.assertEqual(r.returncode, 2, r.stderr)
                    self.assertNotIn("Traceback", r.stderr)
                    self.assertEqual(len(r.stderr.strip().splitlines()), 1, r.stderr)
        cov, why = coverage.load_map(str(self.dir / "absent.json"))
        self.assertIsNone(cov)
        self.assertIn("no coverage map", why)

    def test_coverage_runner_refuses_an_id_with_the_at_sign(self):
        # `@` stands for `/` in the profile directory: an id with its own `@`
        # would map back to another binary -- refused, nothing written
        r, base = self._run_runner({"NEXTEST_BINARY_ID": "odd::a@b",
                                    "NEXTEST_TEST_NAME": "tests::t"},
                                   ["sh", "-c", ': > "$(dirname "$LLVM_PROFILE_FILE")/1-x.profraw"'])
        self.assertEqual(r.returncode, 2, r.stderr)
        self.assertIn("refused", r.stderr)
        self.assertEqual(coverage.profile_parts(base), {})

    def test_coverage_map_needs_profiles_for_every_expected_binary(self):
        base = self.dir / "profiles"
        for b, n in (("a", 2), ("b::t", 1)):
            d = base / b / "m"
            d.mkdir(parents=True)
            for i in range(n):
                (d / ("t%d.profdata" % i)).write_text("")
        (base / "c::u" / "m").mkdir(parents=True)  # ran, left nothing
        (base / "_list").mkdir()
        (base / "_list" / "1.profraw").write_text("")  # a `--list` call never counts
        counts = coverage.profiles_per_binary(base)
        self.assertEqual(counts, {"a": 2, "b::t": 1})
        parts = [{"binary": "a", "module": "m", "files": {"x.rs": 1}},
                 {"binary": "b::t", "module": None, "files": {"y.rs": 1}}]
        ok = coverage.coverage_map("r1", "lane", parts, 1.0, 3, expected=2, errors=[],
                                   run_ok=True, expected_ids=["a", "b::t"], profiles=counts)
        self.assertTrue(ok["complete"])
        self.assertEqual(ok["incomplete"], [])
        # the count matches (an unexpected extra binary), but c::u left no profile
        extra = parts + [{"binary": "x::y", "module": None, "files": {"z.rs": 1}}]
        gap = coverage.coverage_map("r1", "lane", extra, 1.0, 3, expected=3, errors=[],
                                    run_ok=True, expected_ids=["a", "b::t", "c::u"],
                                    profiles=counts)
        self.assertFalse(gap["complete"])
        # by set, the stranger x::y never stands in for c::u (review minor)
        self.assertEqual(gap["incomplete"], ["2/3 binaries measured (missing: c::u)",
                                             "no profiles for 1/3 binaries: c::u"])
        # run 1 on build04: 0 profiles at all, nextest exit 100
        none = coverage.coverage_map("r1", "lane", [], 1.0, 0, expected=2, errors=[],
                                     run_ok=False, expected_ids=["a", "b::t"], profiles={},
                                     nextest_exit=100)
        self.assertFalse(none["complete"])
        self.assertEqual(none["incomplete"], [
            "nextest exit 100", "0/2 binaries measured (missing: a, b::t)", "0 profiles",
            "no profiles for 2/2 binaries: a, b::t"])

    def test_coverage_run_ok_only_for_a_finished_nextest(self):
        self.assertTrue(coverage.run_ok(0))
        # every other exit makes the map incomplete -- 100 (red tests) included:
        # run 1 on build04 ended with 100 and still claimed run_ok
        for rc in (100, 124, 137, 1, 2, 4, -9):
            self.assertFalse(coverage.run_ok(rc), rc)

    def test_coverage_expected_binaries_skip_all_ignored(self):
        doc = {"rust-suites": {
            "a": {"binary-path": "/t/a", "testcases": {"m::t": {"ignored": False}}},
            "b::x": {"binary-path": "/t/x", "testcases": {"t": {"ignored": True}}},
            "c::y": {"binary-path": "/t/y", "testcases": {}},
            "d::z": {"binary-path": "/t/z", "testcases": {
                "t": {"ignored": False, "filter-match": {"status": "mismatch"}}}}}}
        self.assertEqual(coverage.expected_binaries(doc), ["a"])

    def test_the_map_names_the_binaries_without_runnable_tests(self):
        # d59-2 V8 (#1487): the two `*_measure` binaries carry ignored tests only;
        # they stand in no map, and without this list the night was due every
        # night (Dossier 59 section 7a). A binary that carries only browser locks
        # is in the list too: the locks are `#[ignore]` in the main nextest call.
        import argparse
        from pipeline import browser_cover
        import gate_plan
        lock = browser_cover.binary_of(sorted(gate_plan.BROWSER_LOCKS)[0])
        measure = "meclaw-cells::gh1004_viewport_event_measure"
        doc = {"rust-suites": {
            "alpha": {"binary-path": "/t/a", "testcases": {"m::t": {"ignored": False}}},
            measure: {"binary-path": "/t/m", "testcases": {"measure": {"ignored": True}}},
            lock: {"binary-path": "/t/l", "testcases": {"t": {"ignored": True}}},
            "beta::empty": {"binary-path": "/t/e", "testcases": {}}}}
        self.assertEqual(coverage.binaries_without_tests(doc),
                         sorted(["beta::empty", lock, measure]))
        self.assertNotIn("alpha", coverage.binaries_without_tests(doc))
        objects = self.dir / "list.json"
        objects.write_text(json.dumps(doc), encoding="utf-8")
        (self.dir / "profiles").mkdir()
        out = self.dir / "coverage.json"
        parser = argparse.ArgumentParser()
        coverage.register(parser.add_subparsers())
        args = parser.parse_args(["coverage", "aggregate", "--profiles", str(self.dir / "profiles"),
                                  "--objects", str(objects), "--root", str(self.dir),
                                  "--rev", "r1", "--out", str(out)])
        with mock.patch.object(coverage, "aggregate", return_value=([], [])), \
                mock.patch.object(sys, "stdout", io.StringIO()):
            self.assertEqual(coverage._cmd_coverage(args), 0)
        cov = json.loads(out.read_text(encoding="utf-8"))
        self.assertEqual(cov["binaries_without_tests"], sorted(["beta::empty", lock, measure]))
        # a map built without the list (older maps, the tests' maps) has no field
        self.assertNotIn("binaries_without_tests",
                         coverage.coverage_map("r", "lane-a", [], 1.0, 0))

    def test_coverage_aggregate_counts_missing_objects_and_export_errors(self):
        fake = self.dir / "llvm-profdata"
        fake.write_text(FAKE_PROFDATA)
        fake.chmod(0o755)
        cov_tool = self.dir / "llvm-cov"
        cov_tool.write_text("#!/bin/sh\ncase \"$*\" in *bad*) exit 1 ;; esac\n"
                            "printf 'SF:/w/x.rs\\nDA:1,1\\nend_of_record\\n'\n")
        cov_tool.chmod(0o755)
        base = self.dir / "profiles"
        for b in ("good::t", "bad::t", "lost::t"):
            d = base / b / "_binary"
            d.mkdir(parents=True)
            (d / "run.profdata").write_text("")
        objects = {"good::t": "/t/good", "bad::t": "/t/bad", "_bins": []}
        parts, errors = coverage.aggregate(str(base), objects, "/w", jobs=1,
                                           profdata=str(fake), llvm_cov=str(cov_tool))
        self.assertEqual([p["binary"] for p in parts], ["good::t"])
        self.assertEqual(len(errors), 2, errors)
        self.assertTrue(any("bad::t" in e for e in errors), errors)
        self.assertTrue(any("lost::t" in e and "object" in e for e in errors), errors)

    def test_coverage_lane_comes_from_the_host_file(self):
        hosts = self.dir / "lanes"
        hosts.write_text("lane-a user@lane-a.invalid\nlocal-x local /p class=egon\n")
        self.assertEqual(coverage.lane_ssh("lane-a", str(hosts)), "user@lane-a.invalid")
        self.assertIsNone(coverage.lane_ssh("nope", str(hosts)))

    def build_args(self, lane):
        import argparse
        return argparse.Namespace(op="build", lane=lane, strand="welle-dev-tempo/dt-w6",
                                  out_dir=str(self.dir / "out"), db=str(self.dir / "p.db"),
                                  scope="full")   # the parser's default (S1b --scope)

    def test_coverage_lane_reads_the_kits_host_file_variable(self):
        # review I2: the kit reads MECLAW_LANES_FILE (strand.sh lanes_file).
        hosts = self.dir / "lanes-env"
        hosts.write_text("lane-b user@lane-b.invalid threads=8\n")
        with mock.patch.dict(os.environ, {"MECLAW_LANES_FILE": str(hosts)}):
            self.assertEqual(coverage.lane_ssh("lane-b"), "user@lane-b.invalid")

    def test_coverage_build_refuses_an_ssh_lane_of_class_egon_hard(self):
        # review I2, narrowed by d59-2 local V2 (R-59-2): the owner's machine is
        # reached only through transport `local`; an ssh lane of class egon
        # would run the map beside the live colony without the local limits.
        hosts = self.dir / "lanes"
        hosts.write_text("egon-x user@egon.invalid class=egon\n")
        with mock.patch.dict(os.environ, {"MECLAW_LANES_FILE": str(hosts)}), \
                mock.patch.object(coverage.subprocess, "run") as run:
            for lane in ("egon-x",):
                err = io.StringIO()
                with self.subTest(lane=lane), mock.patch.object(sys, "stderr", err):
                    self.assertEqual(coverage._cmd_coverage(self.build_args(lane)), 5)
                    self.assertIn("refused", err.getvalue())
            run.assert_not_called()
        self.assertFalse((self.dir / "p.db").exists())

    def test_coverage_build_asks_the_kit_for_the_token_first(self):
        hosts = self.dir / "lanes"
        hosts.write_text("lane-a user@lane-a.invalid\n")
        calls = []

        def fake_run(cmd, **kw):
            calls.append(cmd)
            return subprocess.CompletedProcess(cmd, 3)
        with mock.patch.dict(os.environ, {"MECLAW_LANES_FILE": str(hosts)}), \
                mock.patch.object(coverage, "lane_up", return_value=True), \
                mock.patch.object(coverage.subprocess, "run", side_effect=fake_run), \
                mock.patch.object(sys, "stderr", io.StringIO()):
            self.assertEqual(coverage._cmd_coverage(self.build_args("lane-a")), 3)
        self.assertEqual(len(calls), 1, calls)
        self.assertEqual(calls[0][:3], ["scripts/strand.sh", "token", "check"])
        self.assertEqual(calls[0][calls[0].index("--lane") + 1], "lane-a")

    def test_k9_coverage_says_seen_after_its_build(self):
        """K9 (welle-validierung-2): the build checks in with its pid before the
        lane run and said nothing after it -- `seen` stayed at the start and the
        idle reclaim (15 min) read the just-ended build as "idle". After the run,
        whatever its exit, the kit hears `token check --pid 0` for the lane."""
        hosts = self.dir / "lanes"
        hosts.write_text("lane-a user@lane-a.invalid\n")
        for build_rc in (0, 1):
            calls = []

            def fake_run(cmd, calls=calls, build_rc=build_rc, **kw):
                calls.append(cmd)
                return subprocess.CompletedProcess(cmd, build_rc if len(calls) == 2 else 0)
            with self.subTest(build_rc=build_rc), \
                    mock.patch.dict(os.environ, {"MECLAW_LANES_FILE": str(hosts)}), \
                    mock.patch.object(coverage, "lane_up", return_value=True), \
                    mock.patch.object(coverage, "load_map", return_value=(None, "no map")), \
                    mock.patch.object(coverage.subprocess, "run", side_effect=fake_run), \
                    mock.patch.object(sys, "stderr", io.StringIO()):
                coverage._cmd_coverage(self.build_args("lane-a"))
                self.assertEqual(3, len(calls), calls)
                self.assertEqual(["scripts/strand.sh", "token", "check", "--pid", "0",
                                  "--lane", "lane-a"], calls[2])

    def test_k9_an_aborted_coverage_build_says_seen_all_the_same(self):
        """Re-check v2-fix-b M4: `seen` after the lane run sat on the straight
        path only -- a ^C or an exception in the run left it at the build's
        start, and the dead process no longer guarded the lane. The abort goes
        on up unchanged, and the kit hears `token check --pid 0` first."""
        hosts = self.dir / "lanes"
        hosts.write_text("lane-a user@lane-a.invalid\n")
        for abort in (KeyboardInterrupt, OSError):
            calls = []

            def fake_run(cmd, calls=calls, abort=abort, **kw):
                calls.append(cmd)
                if len(calls) == 2:
                    raise abort("the lane run was cut short")
                return subprocess.CompletedProcess(cmd, 0)
            with self.subTest(abort=abort.__name__), \
                    mock.patch.dict(os.environ, {"MECLAW_LANES_FILE": str(hosts)}), \
                    mock.patch.object(coverage, "lane_up", return_value=True), \
                    mock.patch.object(coverage.subprocess, "run", side_effect=fake_run), \
                    mock.patch.object(sys, "stderr", io.StringIO()):
                with self.assertRaisesRegex(abort, "the lane run was cut short"):
                    coverage._cmd_coverage(self.build_args("lane-a"))
                self.assertEqual(3, len(calls), calls)
                self.assertEqual(["scripts/strand.sh", "token", "check", "--pid", "0",
                                  "--lane", "lane-a"], calls[2])

    def test_coverage_build_never_passes_a_foreign_exit_six(self):
        """re-check H2 m2: exit 6 means RED (the night journals coverage_red); a
        token check or lane script ending 6 (rsync 6) is a failed build, exit 1."""
        hosts = self.dir / "lanes"
        hosts.write_text("lane-a user@lane-a.invalid\n")
        for fail_at in (1, 2):
            calls = []

            def fake_run(cmd, calls=calls, fail_at=fail_at, **kw):
                calls.append(cmd)
                return subprocess.CompletedProcess(cmd, 6 if len(calls) == fail_at else 0)
            with mock.patch.dict(os.environ, {"MECLAW_LANES_FILE": str(hosts)}), \
                    mock.patch.object(coverage, "lane_up", return_value=True), \
                    mock.patch.object(coverage.subprocess, "run", side_effect=fake_run), \
                    mock.patch.object(sys, "stderr", io.StringIO()):
                rc = coverage._cmd_coverage(self.build_args("lane-a"))
            # K9: a build that ran is followed by the kit's `seen` (`--pid 0`).
            self.assertEqual(len(calls), 1 if fail_at == 1 else 3, calls)
            if fail_at == 2:
                self.assertEqual(calls[2][:5], ["scripts/strand.sh", "token", "check", "--pid", "0"])
            self.assertEqual(rc, 1, fail_at)
            self.assertNotEqual(rc, coverage.RED_EXIT)

    def test_coverage_build_without_llvm_tools_is_a_loud_own_exit(self):
        """V2-A2 (welle-val2): on a lane without llvm-tools the build ended with
        one stderr line and no map, and read as success. A missing tool is its
        own exit (`TOOL_EXIT`, never 0 and none of the other build exits), a
        `coverage_tool_missing` finding with the lane, a message naming it."""
        others = {0, 1, 2, 3, 5, coverage.RED_EXIT}
        self.assertNotIn(coverage.TOOL_EXIT, others)
        # the host part ends with exactly that exit when the tools are missing
        host = (SCRIPTS / "pipeline" / "coverage_host.sh").read_text()
        line = next(ln for ln in host.splitlines() if "llvm-tools missing" in ln)
        self.assertTrue(line.rstrip().endswith("exit %d; }" % coverage.TOOL_EXIT), line)
        hosts = self.dir / "lanes"
        hosts.write_text("lane-a user@lane-a.invalid\n")
        calls = []

        def fake_run(cmd, **kw):
            calls.append(cmd)
            return subprocess.CompletedProcess(cmd, 0 if len(calls) == 1 else coverage.TOOL_EXIT)
        err = io.StringIO()
        with mock.patch.dict(os.environ, {"MECLAW_LANES_FILE": str(hosts)}), \
                mock.patch.object(coverage, "lane_up", return_value=True), \
                mock.patch.object(coverage.subprocess, "run", side_effect=fake_run), \
                mock.patch.object(sys, "stderr", err):
            rc = coverage._cmd_coverage(self.build_args("lane-a"))
        self.assertEqual(rc, coverage.TOOL_EXIT)
        # check, lane run, and the kit's `seen` after it (K9)
        self.assertEqual(len(calls), 3, calls)
        self.assertIn("lane-a", err.getvalue())
        self.assertIn("llvm-tools", err.getvalue())
        self.assertIn("coverage_tool_missing", err.getvalue())
        db = store.open_ro(self.dir / "p.db")
        try:
            found = [json.loads(r[0]) for r in
                     db.execute("SELECT data FROM journal WHERE event = 'process_finding'")]
        finally:
            store.close(db)
        self.assertEqual([(f["rule"], f["subject"]) for f in found],
                         [("coverage_tool_missing", "lane-a")])

    def test_red_tests_take_a_last_try_fail_without_a_later_pass(self):
        """re-check H2 m4: with retries a test red in every try ends as `TRY n
        FAIL`; it is red unless a later FLAKY/PASS line names it."""
        log = self.dir / "retry.log"
        log.write_text(
            "  TRY 1 FAIL [   0.200s] meclaw::gate flaky::first_try\n"
            "  TRY 2 PASS [   0.300s] meclaw::gate flaky::first_try\n"
            "  TRY 1 FAIL [   0.200s] (3/9) meclaw::gate red::always\n"
            "  TRY 2 FAIL [   0.200s] (3/9) meclaw::gate red::always\n"
            "  TRY 1 FAIL [   0.200s] meclaw::gate flaky::second\n"
            "        FLAKY 2/2 [   0.300s] meclaw::gate flaky::second\n"
            "        FAIL [   2.300s] (2/4) meclaw-cells gh2::broken\n", encoding="utf-8")
        self.assertEqual(coverage.red_tests_from_log(log),
                         ["meclaw::gate red::always", "meclaw-cells gh2::broken"])

    def test_coverage_build_writes_the_map_where_narrowing_reads_it(self):
        # The nightly `coverage build` (no --out-dir) wrote `coverage-<rev>.json`
        # under graph/, `narrow.newest_map()` reads `narrow.coverage_dir()`:
        # the fresh map never reached the narrowing (measured 2026-10-10).
        import argparse
        from pipeline import narrow
        hosts = self.dir / "lanes"
        hosts.write_text("lane-a user@lane-a.invalid\n")
        out, maps = self.dir / "graph", self.dir / "coverage"
        out.mkdir()
        cov = coverage.coverage_map("7d75ec7", "lane-a", [
            {"binary": "alpha", "module": "net", "files": {"crates/alpha/src/net.rs": 1}},
        ], 3.0, 1, built_at="2026-10-10T01:00:00Z", expected_ids=["alpha"])
        cov["browsers_opt_out"] = "0"   # re-check M7d M1: the lane's opt-out travels to the journal
        (out / "coverage.json").write_text(json.dumps(cov), encoding="utf-8")
        parser = argparse.ArgumentParser()
        coverage.register(parser.add_subparsers())
        args = parser.parse_args(["coverage", "build", "--lane", "lane-a", "--out-dir", str(out),
                                  "--db", str(self.dir / "p.db")])
        with mock.patch.dict(os.environ, {"MECLAW_LANES_FILE": str(hosts),
                                          "MECLAW_PIPELINE_COVERAGE_DIR": str(maps)}), \
                mock.patch.object(coverage, "lane_up", return_value=True), \
                mock.patch.object(coverage, "resolve_rev", return_value="7d75ec7"), \
                mock.patch.object(coverage.subprocess, "run",
                                  side_effect=lambda cmd, **kw: subprocess.CompletedProcess(cmd, 0)), \
                mock.patch.object(sys, "stdout", io.StringIO()), \
                mock.patch.object(sys, "stderr", io.StringIO()):
            self.assertEqual(coverage._cmd_coverage(args), 0)
            found, why = narrow.newest_map()
        self.assertIsNotNone(found, why)
        self.assertEqual(found["rev"], "7d75ec7")
        self.assertTrue((maps / "coverage-7d75ec7.json").is_file())
        self.assertFalse((out / "coverage-7d75ec7.json").exists())
        self.assertTrue((out / "coverage.json").is_file())  # the lane output stays under graph/
        db = store.open_ro(self.dir / "p.db")
        try:
            built = [json.loads(r[0]) for r in
                     db.execute("SELECT data FROM journal WHERE event = 'coverage_built'")]
        finally:
            store.close(db)
        self.assertEqual(["0"], [b.get("browsers_opt_out") for b in built])

    def test_coverage_build_red_test_is_red_verdict_with_names(self):
        # welle-tb S1 C-Beleg (KR-TB-1 a): the map build on the end tip is the full
        # suite of the wave. Red tests (nextest exit 100) were only "incomplete":
        # exit 1, no test names, no finding. Now: verdict RED, the names, a finding.
        import argparse
        log = self.dir / "nextest.log"
        log.write_text(
            "        PASS [   0.100s] (1/4) meclaw-cells gh1::fine\n"
            "        FAIL [   2.300s] (2/4) meclaw-cells gh2::broken\n"
            "  TRY 1 FAIL [   0.200s] meclaw::gate flaky::first_try\n"
            "        FLAKY 2/2 [   0.300s] meclaw::gate flaky::first_try\n"
            "     TIMEOUT [  60.000s] meclaw::colony boot::hangs\n"
            "     Summary [  70.000s] 4 tests run: 2 passed, 2 failed\n"
            "        FAIL [   2.300s] meclaw-cells gh2::broken\n"
            "     TIMEOUT [  60.000s] meclaw::colony boot::hangs\n", encoding="utf-8")
        names = coverage.red_tests_from_log(log)
        self.assertEqual(names, ["meclaw-cells gh2::broken", "meclaw::colony boot::hangs"])
        self.assertIsNone(coverage.red_tests_from_log(self.dir / "missing.log"))
        parts = [{"binary": "meclaw-cells", "module": "gh2", "files": {"crates/x/src/a.rs": 1}}]
        red = coverage.coverage_map("r9", "lane", parts, 1.0, 3, run_ok=coverage.run_ok(100),
                                    expected_ids=["meclaw-cells"], nextest_exit=100,
                                    red_tests=names)
        self.assertEqual(red["verdict"], "RED")
        self.assertEqual(red["red_tests"], names)
        self.assertFalse(red["complete"])
        self.assertIn("2 red tests: meclaw-cells gh2::broken, meclaw::colony boot::hangs",
                      red["incomplete"])
        # fail-closed: exit 100 without a name in the log is still RED, and says so
        blind = coverage.coverage_map("r9", "lane", parts, 1.0, 3, run_ok=False,
                                      expected_ids=["meclaw-cells"], nextest_exit=100,
                                      red_tests=[])
        self.assertEqual(blind["verdict"], "RED")
        self.assertIn("red tests (no name found in the nextest log)", blind["incomplete"])
        # other exits stay INCOMPLETE, a whole green run GREEN; an old map without the
        # field is judged by `complete` only, never trusted as GREEN on its word
        killed = coverage.coverage_map("r9", "lane", parts, 1.0, 3, run_ok=False,
                                       expected_ids=["meclaw-cells"], nextest_exit=124,
                                       red_tests=[])
        self.assertEqual(killed["verdict"], "INCOMPLETE")
        green = coverage.coverage_map("r9", "lane", parts, 1.0, 3,
                                      expected_ids=["meclaw-cells"], red_tests=[])
        self.assertEqual(green["verdict"], "GREEN")
        self.assertEqual(coverage.verdict_of(dict(green, complete=False)), "INCOMPLETE")
        self.assertEqual(coverage.verdict_of({"rev": "r", "parts": [], "complete": True}), "GREEN")
        self.assertEqual(coverage.verdict_of({"rev": "r", "parts": []}), "INCOMPLETE")
        # the host part hands its nextest log to the aggregation
        host = (SCRIPTS / "pipeline" / "coverage_host.sh").read_text()
        self.assertIn('--nextest-log "$C/nextest.log"', host)
        # build: exit RED_EXIT, the names on stderr, a `coverage_red` finding with the names
        hosts = self.dir / "lanes"
        hosts.write_text("lane-a user@lane-a.invalid\n")
        out, maps, db_path = self.dir / "graph", self.dir / "coverage", self.dir / "p.db"
        out.mkdir()
        (out / "coverage.json").write_text(json.dumps(red), encoding="utf-8")
        parser = argparse.ArgumentParser()
        coverage.register(parser.add_subparsers())
        args = parser.parse_args(["coverage", "build", "--lane", "lane-a", "--out-dir", str(out),
                                  "--db", str(db_path)])
        err = io.StringIO()
        with mock.patch.dict(os.environ, {"MECLAW_LANES_FILE": str(hosts),
                                          "MECLAW_PIPELINE_COVERAGE_DIR": str(maps)}), \
                mock.patch.object(coverage, "lane_up", return_value=True), \
                mock.patch.object(coverage, "resolve_rev", return_value="r9"), \
                mock.patch.object(coverage.subprocess, "run",
                                  side_effect=lambda cmd, **kw: subprocess.CompletedProcess(cmd, 0)), \
                mock.patch.object(sys, "stdout", io.StringIO()), \
                mock.patch.object(sys, "stderr", err):
            rc = coverage._cmd_coverage(args)
        self.assertEqual(rc, coverage.RED_EXIT)
        self.assertNotIn(rc, (0, 1, 2, 3, 5))
        self.assertIn("RED", err.getvalue())
        self.assertIn("meclaw-cells gh2::broken", err.getvalue())
        self.assertTrue((maps / "coverage-r9.json").is_file())  # wave close reads it back
        db = store.open_ro(db_path)
        try:
            found = [json.loads(r[0]) for r in db.execute(
                "SELECT data FROM journal WHERE event='process_finding'")]
        finally:
            store.close(db)
        self.assertEqual([(f["rule"], f["subject"], f["value"]) for f in found],
                         [("coverage_red", "r9", 2)])
        self.assertEqual(found[0]["tests"], names)


def _git(repo, *args):
    return subprocess.run(["git", "-C", str(repo)] + list(args), capture_output=True,
                          text=True, check=True).stdout.strip()


class CarryTest(unittest.TestCase):
    """GH #1461: the night of 2026-11-10 re-measured 41 min on a lane for a diff
    of 324 files under `plans/` that the resolver selects no test for. A diff
    without a selected test carries the newest clean GREEN map to HEAD, no run."""

    BASE_AT = "2026-11-09T01:56:24Z"
    NOW = "2026-11-10T01:00:05Z"

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        d = pathlib.Path(tmp.name)
        self.repo = d / "repo"
        (self.repo / "crates/alpha/src").mkdir(parents=True)
        _git(self.repo, "init", "-q", "-b", "master")
        _git(self.repo, "config", "user.email", "t@example.invalid")
        _git(self.repo, "config", "user.name", "t")
        (self.repo / "crates/alpha/src/lib.rs").write_text("pub fn f() {}\n")
        _git(self.repo, "add", "-A")
        _git(self.repo, "commit", "-q", "-m", "base")
        self.base = _git(self.repo, "rev-parse", "HEAD")
        (self.repo / "plans/w").mkdir(parents=True)
        (self.repo / "plans/w/receipt.md").write_text("# receipt\n")
        _git(self.repo, "add", "-A")
        _git(self.repo, "commit", "-q", "-m", "close")
        self.head = _git(self.repo, "rev-parse", "HEAD")
        self.maps = d / "maps"
        self.maps.mkdir()
        self.db = store.open(d / "p.db")
        self.addCleanup(store.close, self.db)
        self.seen = []

    def _map(self, rev, verdict="GREEN", complete=True, built_at=BASE_AT):
        cov = coverage.coverage_map(rev, "build04",
                                    [{"binary": "alpha", "module": None,
                                      "files": {"crates/alpha/src/lib.rs": 3}}],
                                    1620.0, 12, built_at=built_at)
        cov["complete"], cov["verdict"] = complete, verdict
        coverage.map_path(rev, self.maps).write_text(json.dumps(cov), encoding="utf-8")
        return cov

    def _carry(self, terms, **kw):
        def fake(paths, repo=None):
            self.seen.append(sorted(paths))
            return terms
        with mock.patch("gate_plan.test_terms", fake):
            return coverage.carry(str(self.repo), map_dir=self.maps, db=self.db, now=self.NOW,
                                  **kw)

    def _built(self):
        return [json.loads(r[0]) for r in self.db.execute(
            "SELECT data FROM journal WHERE event='coverage_built'")]

    def test_a_diff_that_selects_no_test_carries_the_map_to_head(self):
        self._map(self.base)
        cov, line = self._carry({})
        self.assertEqual(self.seen, [["plans/w/receipt.md"]])
        self.assertEqual(cov["rev"], self.head)
        self.assertEqual(cov["built_at"], self.NOW)
        self.assertEqual(cov["carried_from"], {"rev": self.base, "built_at": self.BASE_AT,
                                               "runner": "build04", "secs": 1620.0})
        on_disk, why = coverage.load_map(coverage.map_path(self.head, self.maps))
        self.assertIsNone(why)
        self.assertEqual(on_disk, cov)
        self.assertEqual(coverage.verdict_of(on_disk), "GREEN")  # wave close reads this
        self.assertIn("carried", line)
        self.assertIn(self.base[:9], line)
        # review M3: a carry is no lane run -- runner and secs say so, the
        # measuring run's live on in carried_from
        self.assertEqual((cov["runner"], cov["secs"]), ("carry", 0.0))
        self.assertEqual((cov["carried_from"]["runner"], cov["carried_from"]["secs"]),
                         ("build04", 1620.0))
        built = self._built()
        self.assertEqual([(b["rev"], b["runner"], b["secs"], b["carried_from"]["rev"])
                          for b in built], [(self.head, "carry", 0.0, self.base)])

    def test_a_rename_is_judged_on_both_sides(self):
        # review I1: with git's rename detection the diff named the target only;
        # a template moved under plans/ selected nothing and carried, while its
        # old place selects 240 terms in the real tree
        self._map(self.base)
        _git(self.repo, "mv", "crates/alpha/src/lib.rs", "plans/w/lib.rs")
        _git(self.repo, "commit", "-q", "-m", "move")
        self._carry({})
        self.assertEqual(self.seen, [sorted(["crates/alpha/src/lib.rs", "plans/w/lib.rs",
                                             "plans/w/receipt.md"])])

    def test_tracked_changes_in_the_tree_are_judged_too(self):
        # review M2: the resolver reads the checked-out tree; a tracked file
        # changed there (not committed) is judged with the diff
        self._map(self.base)
        (self.repo / "crates/alpha/src/lib.rs").write_text("pub fn g() {}\n")
        (self.repo / "untracked.txt").write_text("x\n")
        self._carry({})
        self.assertEqual(self.seen, [["crates/alpha/src/lib.rs", "plans/w/receipt.md"]])

    def test_a_file_deleted_in_the_tree_carries_nothing(self):
        # re-check N1: the resolver reads the tree; a reader deleted there would
        # select nothing, so a deletion in the tree is never judged, only refused
        self._map(self.base)
        (self.repo / "crates/alpha/src/lib.rs").unlink()
        cov, why = self._carry({})
        self.assertIsNone(cov)
        self.assertIn("deletes tracked files (crates/alpha/src/lib.rs)", why)
        self.assertEqual(self.seen, [])

    def test_the_line_counts_commits_and_tree_apart(self):
        # re-check N2: the count of the carry line separates the diff from the tree
        self._map(self.base)
        (self.repo / "crates/alpha/src/lib.rs").write_text("pub fn g() {}\n")
        _cov, line = self._carry({})
        self.assertIn("(1 files changed + 1 in the tree,", line)

    def test_a_base_rev_outside_the_repository_carries_nothing(self):
        self._map("f" * 40)
        cov, why = self._carry({})
        self.assertIsNone(cov)
        self.assertIn("not in", why)
        self.assertEqual(self.seen, [])

    def test_a_changed_binary_list_carries_nothing(self):
        # review M1: the check lives in carry itself, so the CLI has it too
        # a binary the map names and the tree lacks: measured, never carried
        self._map(self.base)
        meta = {"workspace_members": ["alpha 0.1.0"],
                "packages": [{"id": "alpha 0.1.0", "name": "alpha",
                              "targets": [{"kind": ["test"], "name": "misc"}]}]}
        cov, why = self._carry({}, metadata=meta)
        self.assertIsNone(cov)
        self.assertIn("binary list", why)
        self.assertIn("gone alpha", why)
        self.assertEqual(self.seen, [])
        # a binary of the tree no map ever measured (the `*_measure` perf
        # binaries of the real tree) does not refuse: never carrying would be the cost
        meta["packages"][0]["targets"].insert(0, {"kind": ["lib"], "name": "alpha"})
        cov, _line = self._carry({}, metadata=meta)
        self.assertEqual(cov["rev"], self.head)

    def test_a_resolver_error_is_a_reason_in_the_cli(self):
        # review M6: a broken resolver input is exit 1 with the reason, no traceback
        self._map(self.base)
        args = types.SimpleNamespace(op="carry", map_dir=str(self.maps), db=None,
                                     repo=str(self.repo), branch=None)
        err = io.StringIO()

        def broken(paths, repo=None):
            raise ValueError("required-pins.toml: bad table")
        with mock.patch("gate_plan.test_terms", broken), mock.patch.object(sys, "stderr", err):
            self.assertEqual(coverage._cmd_coverage(args), 1)
        self.assertIn("bad table", err.getvalue())

    def test_a_diff_that_selects_tests_carries_nothing(self):
        self._map(self.base)
        cov, why = self._carry({"all()": ["rule:1 file:Cargo.lock"]})
        self.assertIsNone(cov)
        self.assertIn("all()", why)
        self.assertFalse(coverage.map_path(self.head, self.maps).exists())
        self.assertEqual(self._built(), [])

    def test_only_the_newest_clean_green_map_is_a_base(self):
        # a dirty map never: it names no commit (the night's own map of 10.11.)
        self._map(self.base)
        self._map(self.head + "-dirty", built_at="2026-11-10T01:45:51Z")
        cov, _line = self._carry({})
        self.assertEqual(cov["carried_from"]["rev"], self.base)
        # a newer clean map that is not GREEN stops the carry: an older GREEN
        # map never stands in for a measurement that found red tests
        coverage.map_path(self.head, self.maps).unlink()
        mid = _git(self.repo, "rev-parse", "HEAD~1")
        self._map(mid, verdict="RED", built_at="2026-11-09T12:00:00Z")
        cov, why = self._carry({})
        self.assertIsNone(cov)
        self.assertIn("RED", why)
        self._map(mid, complete=False, verdict="GREEN", built_at="2026-11-09T12:00:00Z")
        cov, why = self._carry({})
        self.assertIsNone(cov)
        self.assertIn("INCOMPLETE", why)

    def test_head_must_be_the_named_branch(self):
        # the resolver reads the checked-out tree: carrying to another rev would
        # judge a diff against files it never looked at
        self._map(self.base)
        _git(self.repo, "branch", "other", self.base)
        cov, why = self._carry({}, branch="other")
        self.assertIsNone(cov)
        self.assertIn("other", why)
        self.assertEqual(self.seen, [])

    def test_a_map_already_on_head_writes_nothing(self):
        before = self._map(self.head)
        cov, line = self._carry({})
        self.assertEqual(cov, before)
        self.assertIn("already", line)
        self.assertEqual(self.seen, [])
        self.assertEqual(self._built(), [])

    def test_the_cli_carries_and_says_why_not(self):
        self._map(self.base)
        args = types.SimpleNamespace(op="carry", map_dir=str(self.maps), db=None,
                                     repo=str(self.repo), branch=None)
        out, err = io.StringIO(), io.StringIO()
        with mock.patch("gate_plan.test_terms", lambda paths, repo=None: {"all()": ["x"]}), \
                mock.patch.object(sys, "stdout", out), mock.patch.object(sys, "stderr", err):
            self.assertEqual(coverage._cmd_coverage(args), 1)
        self.assertIn("all()", err.getvalue())
        with mock.patch("gate_plan.test_terms", lambda paths, repo=None: {}), \
                mock.patch.object(sys, "stdout", out):
            self.assertEqual(coverage._cmd_coverage(args), 0)
        self.assertIn("carried", out.getvalue())
        self.assertTrue(coverage.map_path(self.head, self.maps).is_file())


class BuildRevTest(unittest.TestCase):
    """d59-2 local V1 (O6, GH #1486): `coverage build` measures the commit `--rev`
    names (default HEAD), resolved once to a full SHA before anything leaves
    this tree; the map carries that SHA and is never `-dirty`. 25 of 72 maps in
    the store were `-dirty` (the night builds from the main tree, which always
    carries untracked files) and no reader ever took them."""

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.dir = pathlib.Path(tmp.name)
        self.repo = self.dir / "repo"
        (self.repo / "scripts").mkdir(parents=True)
        _git(self.repo, "init", "-q", "-b", "master")
        _git(self.repo, "config", "user.email", "t@example.invalid")
        _git(self.repo, "config", "user.name", "t")
        (self.repo / "a.txt").write_text("one\n")
        _git(self.repo, "add", "-A")
        _git(self.repo, "commit", "-q", "-m", "one")
        self.first = _git(self.repo, "rev-parse", "HEAD")
        (self.repo / "a.txt").write_text("two\n")
        _git(self.repo, "commit", "-q", "-am", "two")
        self.head = _git(self.repo, "rev-parse", "HEAD")
        # the overlay of the calling tree: a changed tracked file, an untracked one
        (self.repo / "a.txt").write_text("dirty\n")
        (self.repo / "untracked.txt").write_text("x\n")
        self.hosts = self.dir / "lanes"
        self.hosts.write_text("lane-a user@lane-a.invalid\n")
        self.out, self.maps = self.dir / "out", self.dir / "maps"
        self.out.mkdir()

    def build(self, *extra, lane_rev=None, lane="lane-a", env=None):
        """`coverage build --lane <lane> [extra]` from `self.repo`; the fake lane
        leaves a GREEN map whose rev is the SHA it was handed (or `lane_rev`).
        `(rc, lane calls, stderr)`; the environment of each lane call lands in
        `self.lane_envs`."""
        import argparse
        parser = argparse.ArgumentParser()
        coverage.register(parser.add_subparsers())
        args = parser.parse_args(["coverage", "build", "--lane", lane, "--out-dir",
                                  str(self.out), "--db", str(self.dir / "p.db")] + list(extra))
        lanes = []
        self.lane_envs = []

        def run(cmd, **kw):
            if any("coverage_lane.sh" in str(c) for c in cmd):
                lanes.append(cmd)
                self.lane_envs.append(kw.get("env"))
                rev = lane_rev or (cmd[-1] if cmd[-2] == "full" else "probe")
                cov = coverage.coverage_map(rev, "lane-a", [
                    {"binary": "alpha", "module": None, "files": {"a.txt": 1}}],
                    1.0, 1, built_at="2026-11-11T01:00:00Z", expected_ids=["alpha"])
                (self.out / "coverage.json").write_text(json.dumps(cov), encoding="utf-8")
            return subprocess.CompletedProcess(cmd, 0)
        err = io.StringIO()
        with mock.patch.dict(os.environ, dict({"MECLAW_LANES_FILE": str(self.hosts),
                                               "MECLAW_PIPELINE_COVERAGE_DIR": str(self.maps)},
                                              **(env or {}))), \
                mock.patch.object(coverage, "_SCRIPTS", str(self.repo / "scripts")), \
                mock.patch.object(coverage, "lane_up", return_value=True), \
                mock.patch.object(coverage.subprocess, "run", side_effect=run), \
                mock.patch.object(sys, "stdout", io.StringIO()), \
                mock.patch.object(sys, "stderr", err):
            rc = coverage._cmd_coverage(args)
        return rc, lanes, err.getvalue()

    def installed(self):
        return sorted(p.name for p in self.maps.iterdir()) if self.maps.is_dir() else []

    def test_build_measures_the_named_commit_and_never_a_dirty_tree(self):
        # the default is HEAD of the calling tree, as a full SHA -- its overlay stays here
        rc, lanes, err = self.build()
        self.assertEqual(0, rc, err)
        self.assertEqual(["full", self.head], lanes[0][-2:])
        self.assertEqual(["coverage-%s.json" % self.head], self.installed())
        # a named rev (short, as a person types it) is resolved before the push
        rc, lanes, err = self.build("--rev", self.first[:9])
        self.assertEqual(0, rc, err)
        self.assertEqual(["full", self.first], lanes[0][-2:])
        self.assertIn("coverage-%s.json" % self.first, self.installed())
        self.assertFalse([n for n in self.installed() if "dirty" in n], self.installed())

    def test_build_refuses_a_rev_the_repository_does_not_have(self):
        for rev in ("0" * 40, "no-such-branch", "HEAD~9"):
            with self.subTest(rev=rev):
                rc, lanes, err = self.build("--rev", rev)
                self.assertEqual(2, rc, err)
                self.assertEqual([], lanes)
                self.assertIn("rev %s" % rev, err)
        self.assertEqual([], self.installed())

    def test_the_browser_probe_refuses_a_rev(self):
        # the probe measures the tree with its overlay; a rev would claim a commit
        rc, lanes, err = self.build("--scope", "browser", "--rev", self.head)
        self.assertEqual(2, rc, err)
        self.assertEqual([], lanes)
        self.assertIn("--rev", err)

    def test_a_local_lane_is_not_refused_and_counts_as_up(self):
        # d59-2 local V2 (R-59-2): the map may be built on the owner's machine
        # when the host file names the lane with transport `local`; it answers
        # without ssh. An ssh lane of class egon stays refused (it would run
        # beside the live colony without the local limits of V3).
        self.assertIsNone(coverage.lane_refused("local", {"class": "egon"}))
        self.assertIsNone(coverage.lane_refused("local", {}))
        self.assertIsNotNone(coverage.lane_refused("user@egon.invalid", {"class": "egon"}))
        with mock.patch.object(coverage.subprocess, "run") as run:
            self.assertTrue(coverage.lane_up("local"))
            self.assertFalse(coverage.lane_up(None))
        run.assert_not_called()
        self.hosts.write_text("mm local %s class=egon threads=4 jobs=4\n" % (self.dir / "root"))
        rc, lanes, err = self.build(lane="mm")
        self.assertEqual(0, rc, err)
        self.assertEqual("local", lanes[0][lanes[0].index("scripts/pipeline/coverage_lane.sh") + 1])
        self.assertEqual(["coverage-%s.json" % self.head], self.installed())
        cov = json.loads((self.maps / ("coverage-%s.json" % self.head)).read_text())
        self.assertEqual("mm", cov["runner"])

    def test_the_coverage_run_gets_root_class_and_width_from_the_host_file(self):
        # d59-2 local V2: without the settings of the host file the engine fell
        # back to the root /srv/target and class `box`, and `local_needs_egon`
        # refused the local lane (exit 5) -- the same words as `strand.sh
        # lane_settings` and `merge._lane`; `mem=` caps the scope (V3 d).
        root = self.dir / "root"
        self.hosts.write_text("mm local %s class=egon threads=4 jobs=2 mem=24G\n"
                              "lane-a user@lane-a.invalid class=lane threads=6\n" % root)
        rc, lanes, err = self.build(lane="mm")
        self.assertEqual(0, rc, err)
        env = self.lane_envs[0]
        for name, want in (("MECLAW_LANE_ROOT", str(root)), ("MECLAW_LANE_CLASS", "egon"),
                           ("MECLAW_LANE_NAME", "mm"), ("MECLAW_LANE_JOBS", "2"),
                           ("MECLAW_LANE_THREADS", "4"), ("MECLAW_LANE_MEM_G", "24")):
            self.assertEqual(want, (env or {}).get(name), name)
        # V3 e: 4 h for `local`; the outer `timeout` carries the lock's wait
        # cap and the scope's margin on top, so the deadline counts from the lock
        self.assertEqual(["timeout", "--kill-after=60",
                          str(coverage.LOCAL_DEADLINE_S + coverage.LOCK_WAIT_S + 120 + 200)],
                         lanes[0][:3])
        rc, lanes, err = self.build(lane="mm", env={"MECLAW_COVERAGE_DEADLINE_S": "900",
                                                    "MECLAW_COVERAGE_LOCK_WAIT_S": "30"})
        self.assertEqual(str(900 + 30 + 120 + 200), lanes[0][2])
        self.assertEqual("900", self.lane_envs[0]["MECLAW_COVERAGE_DEADLINE_S"])
        # a build lane: its own name and width, never a root (its engine's
        # default) -- and the lane deadline of today
        rc, lanes, err = self.build()
        self.assertEqual(0, rc, err)
        env = self.lane_envs[0]
        self.assertEqual("lane-a", env["MECLAW_LANE_NAME"])
        self.assertEqual("lane", env["MECLAW_LANE_CLASS"])
        self.assertEqual("6", env["MECLAW_LANE_THREADS"])
        self.assertNotIn("MECLAW_LANE_ROOT", env)
        self.assertEqual(str(coverage.DEADLINE_S), lanes[0][2])

    def test_profiles_are_removed_after_the_aggregate_and_kept_when_it_fails(self):
        # d59-2 local V4: the profiles are 11 G of a 90 G disk (11.11.); once
        # `aggregate` has written coverage.json they go (lane and local alike);
        # a failed aggregate keeps them as before and one line says so
        tree = self.dir / "host-tree"
        pipe = tree / "scripts" / "pipeline"
        pipe.mkdir(parents=True)
        shutil.copy(SCRIPTS / "pipeline" / "coverage_host.sh", pipe / "coverage_host.sh")
        kit_tree.seed(tree / "scripts", "pipeline/coverage_host.sh")
        (pipe / "browser_cover.py").write_text(
            "import sys\nprint({'select': '-p c1 --test l1', 'filter': 'binary_id(=c1::l1)'}"
            "[sys.argv[1]])\n")
        knob = self.dir / "aggregate-fails"
        (tree / "scripts" / "pipeline.py").write_text(
            "import os, sys\nif os.path.exists(%r):\n    sys.exit(1)\n"
            "open(sys.argv[sys.argv.index('--out') + 1], 'w').write('{}')\n" % str(knob))
        _git(tree, "init", "-q")
        _git(tree, "config", "user.email", "t@example.invalid")
        _git(tree, "config", "user.name", "t")
        (tree / "kept.txt").write_text("x\n")
        _git(tree, "add", "kept.txt")
        _git(tree, "commit", "-q", "-m", "c")
        rev = _git(tree, "rev-parse", "HEAD")
        bindir, sysroot = self.dir / "bin", self.dir / "sysroot"
        tools = sysroot / "lib" / "rustlib" / "x86_64-unknown-linux-gnu" / "bin"
        tools.mkdir(parents=True)
        bindir.mkdir()
        for path, body in ((tools / "llvm-profdata", "#!/bin/sh\n"),
                           (bindir / "rustc", "#!/bin/sh\necho %s\n" % sysroot),
                           (bindir / "cargo", '#!/bin/sh\n[ "$2" != run ] || '
                                              ': >"$MECLAW_COVERAGE_PROFILE_DIR/p.profraw"\n')):
            path.write_text(body)
            path.chmod(0o755)
        env = dict(os.environ, PATH="%s:%s" % (bindir, os.environ.get("PATH", "")))
        profiles = self.dir / "cov" / "profiles"
        for fails in (False, True):
            with self.subTest(fails=fails):
                if fails:
                    knob.write_text("")
                res = subprocess.run(["bash", str(pipe / "coverage_host.sh"),
                                      str(self.dir / "cov" / "target"), "600", rev],
                                     cwd=str(tree), env=env, capture_output=True, text=True,
                                     timeout=kit_time.HANG)
                if fails:
                    self.assertNotEqual(0, res.returncode, res.stderr)
                    self.assertTrue((profiles / "p.profraw").exists())
                    self.assertIn("the profiles stay in %s" % profiles, res.stderr)
                else:
                    self.assertEqual(0, res.returncode, res.stderr)
                    self.assertTrue((self.dir / "cov" / "coverage.json").is_file())
                    self.assertFalse(profiles.exists())

    def test_the_map_is_installed_only_for_the_rev_the_lane_tree_holds(self):
        # a map that names another commit (or the old `-dirty` form) is never installed
        for lane_rev in (self.first, self.head + "-dirty"):
            with self.subTest(lane_rev=lane_rev):
                rc, lanes, err = self.build(lane_rev=lane_rev)
                self.assertEqual(2, rc, err)
                self.assertIn(self.head, err)
                self.assertEqual([], self.installed())
        # the host part refuses before its first cargo call when the tree is
        # not the commit it was handed (scope full)
        target = self.dir / "cov" / "target"
        res = subprocess.run(["bash", str(SCRIPTS / "pipeline" / "coverage_host.sh"),
                              str(target), "60", self.first], cwd=str(self.repo),
                             capture_output=True, text=True, timeout=kit_time.HANG)
        self.assertEqual(5, res.returncode, res.stdout + res.stderr)
        self.assertIn("not commit %s" % self.first, res.stderr)
        self.assertFalse((self.dir / "cov").exists())


if __name__ == "__main__":
    unittest.main()
