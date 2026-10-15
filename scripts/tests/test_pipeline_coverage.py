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
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS))

from pipeline import coverage, graph, store  # noqa: E402
from scripts.tests import store_isolation  # noqa: E402

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

    def test_coverage_build_refuses_the_owner_machine_hard(self):
        # review I2: `local` and class egon are refused before anything runs.
        hosts = self.dir / "lanes"
        hosts.write_text("mm local\negon-x user@egon.invalid class=egon\n")
        self.assertFalse(coverage.lane_up("local"))
        with mock.patch.dict(os.environ, {"MECLAW_LANES_FILE": str(hosts)}), \
                mock.patch.object(coverage.subprocess, "run") as run:
            for lane in ("mm", "egon-x"):
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


if __name__ == "__main__":
    unittest.main()
