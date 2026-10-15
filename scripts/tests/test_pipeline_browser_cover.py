"""welle-tb S1b (#1165), owner's ruling 10.10.: the browser locks in the coverage map.

Point 1 -- the map build runs every browser lock (`--run-ignored all`, both
engines) under coverage, and one map field per lock says whether it was
measured. Only the build sets it, and only when the lock's profiles were merged
into a part AND its junit shows a clean run without a `SKIP ` line.
"""

import argparse
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
REPO = SCRIPTS.parent
sys.path.insert(0, str(SCRIPTS))
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import gate_plan  # noqa: E402
from pipeline import browser_cover, coverage, store, wave  # noqa: E402
from scripts.tests import kit_time, kit_tree, store_isolation  # noqa: E402


_METRICS_URL = mock.patch.dict(os.environ, {"MECLAW_PIPELINE_METRICS_URL": ""})


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()
    _METRICS_URL.start()


def tearDownModule():  # noqa: N802 -- unittest's name
    _METRICS_URL.stop()
    store_isolation.restore()


def _bid(lock):
    return "meclaw-cells::" + lock.rsplit("/", 1)[1].removesuffix(".rs")


def junit(cases):
    """A nextest junit: `cases` = [(binary_id, name, stdout or None, red)]."""
    suites = {}
    for bid, name, out, red in cases:
        body = ""
        if red:
            body += '<failure type="test failure">boom</failure>'
        if out is not None:
            body += "<system-out>%s</system-out>" % out
        suites.setdefault(bid, []).append(
            '<testcase name="%s" classname="%s" time="1.0">%s</testcase>' % (name, bid, body))
    return ('<?xml version="1.0"?><testsuites name="nextest-run">%s</testsuites>'
            % "".join('<testsuite name="%s" tests="%d">%s</testsuite>' % (b, len(c), "".join(c))
                      for b, c in suites.items()))


CLEAN = "running 1 test\nRESULT ok\ntest x ... ok\n"


class Base(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self._tmp.name)

    def tearDown(self):
        self._tmp.cleanup()

    def all_clean(self, skip=None, red=None, silent=None):
        cases = []
        for lock in sorted(gate_plan.BROWSER_LOCKS):
            b = _bid(lock)
            for engine in ("chromium", "webkit"):
                out = CLEAN
                if b == skip and engine == "webkit":
                    out = "running 1 test\nSKIP webkit is not installed\n"
                if b == silent:
                    out = None
                cases.append((b, "holds_in_%s" % engine, out, b == red))
        path = self.root / "junit.xml"
        path.write_text(junit(cases))
        return path

    def parts(self, without=()):
        return [{"binary": _bid(l), "module": None,
                 "files": {"crates/meclaw-cells/src/display/mod.rs": 12}}
                for l in sorted(gate_plan.BROWSER_LOCKS) if _bid(l) not in without]


class MarkerTest(Base):
    def test_every_lock_with_merged_profiles_and_a_clean_run_is_measured(self):
        got = browser_cover.marker(self.parts(), self.all_clean())
        self.assertEqual(sorted(got), sorted(gate_plan.BROWSER_LOCKS))
        for lock, entry in got.items():
            self.assertTrue(entry["measured"], (lock, entry))
            self.assertEqual(entry["binary"], _bid(lock))
            self.assertEqual(entry["tests"], 2)

    def test_a_skip_line_leaves_the_lock_unmeasured(self):
        lock = "crates/meclaw-cells/tests/710_the_colony_holds_in_both_engines_browser.rs"
        got = browser_cover.marker(self.parts(), self.all_clean(skip=_bid(lock)))
        self.assertFalse(got[lock]["measured"])
        self.assertIn("SKIP webkit is not installed", got[lock]["why"])
        others = [e["measured"] for k, e in got.items() if k != lock]
        self.assertTrue(all(others))

    def test_no_merged_profile_leaves_the_lock_unmeasured(self):
        lock = "crates/meclaw-cells/tests/gh1003_the_shell_marks_orientation_browser.rs"
        got = browser_cover.marker(self.parts(without=(_bid(lock),)), self.all_clean())
        self.assertFalse(got[lock]["measured"])
        self.assertIn("no merged profile", got[lock]["why"])
        empty = [dict(p, files={}) if p["binary"] == _bid(lock) else p for p in self.parts()]
        self.assertFalse(browser_cover.marker(empty, self.all_clean())[lock]["measured"])

    def test_red_or_unrecorded_output_leaves_the_lock_unmeasured(self):
        red = "crates/meclaw-cells/tests/gh961_motion_by_target_state_browser.rs"
        silent = "crates/meclaw-cells/tests/gh867_a_display_runs_under_a_strict_csp_browser.rs"
        got = browser_cover.marker(self.parts(),
                                   self.all_clean(red=_bid(red), silent=_bid(silent)))
        self.assertFalse(got[red]["measured"])
        self.assertIn("red", got[red]["why"])
        self.assertFalse(got[silent]["measured"])
        self.assertIn("no stored output", got[silent]["why"])

    def test_a_skipped_case_leaves_the_lock_unmeasured_even_with_output(self):
        """Re-check S1b m-3: `<skipped/>` is no measurement, stored output or not."""
        lock = "crates/meclaw-cells/tests/gh961_motion_by_target_state_browser.rs"
        path = self.all_clean()
        head = '<testcase name="holds_in_webkit" classname="%s" time="1.0">' % _bid(lock)
        text = path.read_text()
        self.assertIn(head, text)
        path.write_text(text.replace(head, head + "<skipped/>"))
        got = browser_cover.marker(self.parts(), path)
        self.assertFalse(got[lock]["measured"])
        self.assertIn("skipped: holds_in_webkit", got[lock]["why"])
        self.assertTrue(all(e["measured"] for k, e in got.items() if k != lock))

    def test_no_junit_or_no_run_leaves_every_lock_unmeasured(self):
        got = browser_cover.marker(self.parts(), self.root / "missing.xml")
        self.assertEqual(sorted(got), sorted(gate_plan.BROWSER_LOCKS))
        self.assertFalse(any(e["measured"] for e in got.values()))
        path = self.root / "junit.xml"
        path.write_text(junit([]))
        got = browser_cover.marker(self.parts(), path)
        self.assertFalse(any(e["measured"] for e in got.values()))
        self.assertTrue(all("not run" in e["why"] for e in got.values()))
        path.write_text("<testsuites><broken")
        self.assertFalse(any(e["measured"] for e in browser_cover.marker(self.parts(), path).values()))


class SelectTest(Base):
    def test_the_cargo_selection_and_the_filter_name_every_lock(self):
        stems = sorted(l.rsplit("/", 1)[1].removesuffix(".rs") for l in gate_plan.BROWSER_LOCKS)
        args = browser_cover.cargo_selection()
        self.assertEqual(args[:2], ["-p", "meclaw-cells"])
        self.assertEqual(sorted(args[i + 1] for i, a in enumerate(args) if a == "--test"), stems)
        flt = browser_cover.nextest_filter()
        self.assertEqual(sorted(t.strip() for t in flt.split(" + ")),
                         sorted("binary_id(=meclaw-cells::%s)" % s for s in stems))

    def test_the_cli_prints_both_for_the_host_script(self):
        for op, want in (("select", " ".join(browser_cover.cargo_selection())),
                         ("filter", browser_cover.nextest_filter())):
            res = subprocess.run([sys.executable, str(SCRIPTS / "pipeline" / "browser_cover.py"), op],
                                 capture_output=True, text=True, cwd=str(REPO))
            self.assertEqual(res.returncode, 0, res.stderr)
            self.assertEqual(res.stdout.strip(), want)


class HostTest(Base):
    def test_the_host_runs_the_locks_under_coverage_and_hands_their_junit_over(self):
        host = (SCRIPTS / "pipeline" / "coverage_host.sh").read_text()
        # the main run leaves the locks to the browser call (the sheet is not ignored)
        self.assertIn('LOCK_FILTER=$("${KIT_PY[@]}" scripts/pipeline/browser_cover.py filter)', host)
        self.assertIn('--run-ignored default -E "not ($LOCK_FILTER)"', host)
        # the browser call: every lock, ignored ones too, its own junit profile
        self.assertIn('LOCK_SEL=$("${KIT_PY[@]}" scripts/pipeline/browser_cover.py select)', host)
        self.assertIn("cargo nextest run $LOCK_SEL $FEAT_LOCK --no-fail-fast", host)
        self.assertIn("--profile coverage-browser --run-ignored all", host)
        # nextest stores junit under the WORKSPACE's `target/nextest/<profile>/`, not
        # under CARGO_TARGET_DIR (probe P-S1b-browser 10.10.: 22 green tests, 0/7
        # measured, "no junit of the browser run" -- the file sat in tree/target/)
        self.assertIn('BJ="$PWD/target/nextest/coverage-browser/junit.xml"', host)
        self.assertIn('rm -f "$BJ"', host)
        self.assertIn('--browser-junit "$BJ"', host)
        self.assertNotIn("$T/nextest/coverage-browser", host)
        # browser scope: the probe skips the main run and lists only the locks
        self.assertIn('SCOPE=${4:-full}', host)
        cfg = (REPO / ".config" / "nextest.toml").read_text()
        block = cfg.split("[profile.coverage-browser.junit]", 1)[1].split("[", 1)[0]
        self.assertIn("store-success-output = true", block)

    def test_lane_scripts_forward_the_scope(self):
        lane = (SCRIPTS / "pipeline" / "coverage_lane.sh").read_text()
        self.assertIn('scope=${4:-full}', lane)
        self.assertIn('"$DEADLINE_S" "$out" "$scope"', lane)


class LaneOverlayTest(unittest.TestCase):
    """Re-check S1b (probe finding): `coverage_lane.sh` judges the overlay of the
    tree that LEAVES for the lane. Probe run 1 (10.10.) read it after the run,
    from the local worktree five minutes later, and called a clean ab1f6fa11
    rev-dirty."""

    def lane(self, before, during, delete=False):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = pathlib.Path(tmp.name)
        env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
        git = ["git", "-C", str(root), "-c", "user.name=t", "-c", "user.email=t@example.invalid"]
        subprocess.run(git + ["init", "-q"], check=True, env=env)
        (root / "kept.txt").write_text("x\n")
        subprocess.run(git + ["add", "kept.txt"], check=True, env=env)
        subprocess.run(git + ["commit", "-q", "-m", "c"], check=True, env=env)
        if delete:
            (root / "kept.txt").unlink()
        (root / "scripts" / "pipeline").mkdir(parents=True)
        script = root / "scripts" / "pipeline" / "coverage_lane.sh"
        script.write_text((SCRIPTS / "pipeline" / "coverage_lane.sh").read_text())
        kit_tree.seed(root / "scripts", "pipeline/coverage_lane.sh")
        state = root / "overlay.state"
        state.write_text(before)
        stub = root / "scripts" / "lane_sync.sh"
        # `coverage` writes the lane's map and, like a build or an edit during
        # the run, changes the overlay; `files` prints the overlay NUL-separated
        stub.write_text("#!/usr/bin/env bash\ncase \"$1\" in\n"
                        "  coverage) printf '{\"rev\": \"abc\"}' > \"$5/coverage.json\"\n"
                        "            printf '%%s' '%s' > '%s' ;;\n"
                        "  files) tr '\\n' '\\0' < '%s' ;;\nesac\n" % (during, state, state))
        stub.chmod(0o755)
        out = root / "out"
        res = subprocess.run(["bash", str(script), "tgt", "s1", str(out)], capture_output=True,
                             text=True, env=env, cwd=str(root), timeout=2 * kit_time.HANG)
        self.assertEqual(0, res.returncode, res.stderr)
        return json.loads((out / "coverage.json").read_text())["rev"], res.stderr

    def test_an_overlay_born_during_the_run_leaves_the_map_clean(self):
        rev, err = self.lane("", "target-junk.txt\n")
        self.assertEqual("abc", rev, err)
        self.assertNotIn("rev-dirty", err)

    def test_an_overlay_that_travels_makes_the_map_dirty(self):
        rev, err = self.lane("crates/a/src/lib.rs\n", "")
        self.assertEqual("abc-dirty", rev, err)
        self.assertIn("rev-dirty", err)

    def test_a_deleted_tracked_file_makes_the_map_dirty(self):
        # `lane_sync.sh files` lists what the overlay sends, never a deletion
        rev, err = self.lane("", "", delete=True)
        self.assertEqual("abc-dirty", rev, err)


class AggregateTest(Base):
    def _args(self, junit_path):
        listing = self.root / "list.json"
        listing.write_text(json.dumps({"rust-suites": {}, "rust-build-meta": {}}))
        ns = dict(op="aggregate", objects=str(listing), harness="", profiles=str(self.root),
                  root=str(self.root), jobs=1, profdata="x", llvm_cov="x", rev="fixture",
                  runner="lane", secs=1.0, nextest_exit=0, nextest_log=None,
                  out=str(self.root / "coverage.json"))
        if junit_path is not None:
            ns["browser_junit"] = str(junit_path)
        return argparse.Namespace(**ns)

    def _run(self, args):
        with mock.patch.object(coverage, "aggregate", return_value=(self.parts(), [])), \
                mock.patch("sys.stdout"):
            self.assertEqual(coverage._cmd_coverage(args), 0)
        return json.loads((self.root / "coverage.json").read_text())

    def test_only_the_build_with_a_browser_junit_sets_the_field(self):
        cov = self._run(self._args(self.all_clean()))
        field = cov[browser_cover.FIELD]
        self.assertTrue(all(e["measured"] for e in field.values()))
        self.assertNotIn(browser_cover.FIELD, self._run(self._args(None)))

    def test_the_parser_takes_browser_junit_and_scope(self):
        parser = argparse.ArgumentParser()
        coverage.register(parser.add_subparsers())
        a = parser.parse_args(["coverage", "aggregate", "--browser-junit", "j.xml"])
        self.assertEqual(a.browser_junit, "j.xml")
        self.assertEqual(parser.parse_args(["coverage", "build"]).scope, "full")
        self.assertEqual(parser.parse_args(["coverage", "build", "--scope", "browser"]).scope,
                         "browser")


class ProbeTest(Base):
    """`coverage build --scope browser`: the probe of the locks alone. It never
    installs its map where narrowing reads, never journals `coverage_built`."""

    def _build(self, field):
        hosts = self.root / "lanes"
        hosts.write_text("lane-a user@lane-a.invalid\n")
        out, maps, db_path = self.root / "out", self.root / "maps", self.root / "p.db"
        out.mkdir()
        cov = {"rev": "r9", "parts": [], "built_at": "2026-10-10T08:00:00Z",
               browser_cover.FIELD: field}
        (out / "coverage.json").write_text(json.dumps(cov))
        parser = argparse.ArgumentParser()
        coverage.register(parser.add_subparsers())
        args = parser.parse_args(["coverage", "build", "--lane", "lane-a", "--scope", "browser",
                                  "--out-dir", str(out), "--db", str(db_path)])
        calls = []

        def run(cmd, **kw):
            calls.append(cmd)
            return subprocess.CompletedProcess(cmd, 0)
        so = io.StringIO()
        with mock.patch.dict(os.environ, {"MECLAW_LANES_FILE": str(hosts),
                                          "MECLAW_PIPELINE_COVERAGE_DIR": str(maps)}), \
                mock.patch.object(coverage, "lane_up", return_value=True), \
                mock.patch.object(coverage.subprocess, "run", side_effect=run), \
                mock.patch.object(sys, "stdout", so), \
                mock.patch.object(sys, "stderr", io.StringIO()):
            rc = coverage._cmd_coverage(args)
        return rc, calls, maps, db_path, so.getvalue()

    def test_the_probe_forwards_the_scope_and_never_installs_or_journals(self):
        field = {l: {"binary": _bid(l), "measured": True, "tests": 2}
                 for l in gate_plan.BROWSER_LOCKS}
        rc, calls, maps, db_path, out = self._build(field)
        self.assertEqual(rc, 0)
        lane = [c for c in calls if any("coverage_lane.sh" in str(x) for x in c)][0]
        self.assertEqual(lane[-1], "browser")
        self.assertFalse(maps.exists() and any(maps.iterdir()))
        if db_path.exists():
            db = store.open_ro(db_path)
            try:
                n = db.execute("SELECT count(*) FROM journal WHERE event='coverage_built'"
                               ).fetchone()[0]
            finally:
                store.close(db)
            self.assertEqual(n, 0)
        self.assertIn("measured 7/7", out)

    def test_an_unmeasured_lock_fails_the_probe(self):
        field = {l: {"binary": _bid(l), "measured": True, "tests": 2}
                 for l in gate_plan.BROWSER_LOCKS}
        lock = sorted(field)[0]
        field[lock] = {"binary": _bid(lock), "measured": False, "tests": 2, "why": "SKIP x"}
        rc, _calls, _maps, _db, out = self._build(field)
        self.assertEqual(rc, 1)
        self.assertIn("measured 6/7", out)


GIT_ENV = {"GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@example.invalid",
           "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@example.invalid",
           "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1"}
LOCK = sorted(gate_plan.BROWSER_LOCKS)[0]
STEM = LOCK.rsplit("/", 1)[1].removesuffix(".rs")


def _field(open_=None):
    f = {l: {"binary": _bid(l), "measured": True, "tests": 2} for l in gate_plan.BROWSER_LOCKS}
    for l, why in (open_ or {}).items():
        f[l] = {"binary": _bid(l), "measured": False, "tests": 2, "why": why}
    return f


def _full_build(root, cov):
    """`coverage build --lane lane-a` (scope full) over a lane that left `cov`:
    `(rc, stdout, maps, db_path)`."""
    hosts = root / "lanes"
    hosts.write_text("lane-a user@lane-a.invalid\n")
    out, maps, db_path = root / "out", root / "maps", root / "p.db"
    out.mkdir(exist_ok=True)
    (out / "coverage.json").write_text(json.dumps(cov))
    parser = argparse.ArgumentParser()
    coverage.register(parser.add_subparsers())
    args = parser.parse_args(["coverage", "build", "--lane", "lane-a", "--out-dir", str(out),
                              "--db", str(db_path)])
    so = io.StringIO()
    with mock.patch.dict(os.environ, {"MECLAW_LANES_FILE": str(hosts),
                                      "MECLAW_PIPELINE_COVERAGE_DIR": str(maps)}), \
            mock.patch.object(coverage, "lane_up", return_value=True), \
            mock.patch.object(coverage.subprocess, "run",
                              side_effect=lambda cmd, **kw: subprocess.CompletedProcess(cmd, 0)), \
            mock.patch.object(sys, "stdout", so), \
            mock.patch.object(sys, "stderr", io.StringIO()):
        rc = coverage._cmd_coverage(args)
    return rc, so.getvalue(), maps, db_path


class NightLineTest(Base):
    """Point 4: the night report names how many locks the built map measured."""

    def test_the_line_counts_the_measured_locks_and_names_the_others(self):
        line = browser_cover.measured_line({browser_cover.FIELD: _field({LOCK: "SKIP x"})})
        self.assertEqual("browser locks measured 6/7 -- not measured: %s (SKIP x)" % STEM, line)
        self.assertEqual("browser locks measured 7/7",
                         browser_cover.measured_line({browser_cover.FIELD: _field()}))
        self.assertEqual("browser locks measured 0/7 -- the map has no browser_locks",
                         browser_cover.measured_line({"parts": []}))

    def test_the_night_line_reads_the_map_the_build_installed(self):
        cov = coverage.coverage_map("r1", "lane", self.parts(), 1.0, 14)
        cov[browser_cover.FIELD] = _field({LOCK: "no merged profile"})
        rc, out, maps, _db = _full_build(self.root, cov)
        self.assertEqual(0, rc)
        self.assertEqual("browser locks measured 6/7 -- not measured: %s (no merged profile)"
                         % STEM, browser_cover.night_line(out, maps))
        # the aggregate's own JSON line is not an installed map
        self.assertIsNone(browser_cover.night_line('{"rev": "r1", "complete": true}\n', maps))
        self.assertIsNone(browser_cover.night_line("", maps))
        gone = out.replace('"r1"', '"r2"')
        self.assertIn("no coverage map", browser_cover.night_line(gone, maps))


class NightVerdictTest(Base):
    """Point 4, pinning (already true before S1b point 4): a red browser lock makes
    the night's map RED, a timed-out browser call never makes it GREEN, and
    `wave close` passes only a GREEN map of the end tip."""

    def _map(self, exit_, log):
        path = self.root / "nextest.log"
        path.write_text(log)
        return coverage.coverage_map("r1", "lane", self.parts(), 1.0, 14,
                                     run_ok=coverage.run_ok(exit_), nextest_exit=exit_,
                                     red_tests=coverage.red_tests_from_log(path))

    def _close(self, cov):
        repo = self.root / "repo"
        repo.mkdir(exist_ok=True)
        env = dict(os.environ, **GIT_ENV)
        for args in (["init", "-q", "-b", "master"], ["commit", "-q", "--allow-empty", "-m", "t"]):
            subprocess.run(["git", "-C", str(repo)] + args, check=True, env=env,
                           capture_output=True)
        tip = subprocess.run(["git", "-C", str(repo), "rev-parse", "HEAD"], check=True,
                             capture_output=True, text=True, env=env).stdout.strip()
        maps = self.root / "close-maps"
        maps.mkdir(exist_ok=True)
        (maps / ("coverage-%s.json" % tip)).write_text(json.dumps(dict(cov, rev=tip)))
        return wave.check_end_tip_coverage(repo, lambda _l: None, map_dir=maps)

    def test_the_host_appends_the_browser_call_and_counts_its_exit(self):
        host = (SCRIPTS / "pipeline" / "coverage_host.sh").read_text()
        self.assertIn('--profile coverage-browser --run-ignored all >>"$C/nextest.log" 2>&1'
                      ' || brc=$?', host)
        self.assertIn("brc=124\n", host)   # no deadline left counts as a timeout
        self.assertIn('[ "$rc" != 0 ] || rc=$brc', host)
        self.assertIn('--nextest-exit "$rc"', host)
        self.assertIn('--nextest-log "$C/nextest.log"', host)

    def test_a_red_browser_lock_makes_the_night_map_red(self):
        bid = _bid(LOCK)
        cov = self._map(100, "running\n        FAIL [  12.345s] %s holds_in_chromium\n" % bid)
        self.assertEqual("RED", coverage.verdict_of(cov))
        self.assertEqual(["%s holds_in_chromium" % bid], cov["red_tests"])
        rc, _out, _maps, db_path = _full_build(self.root, cov)
        self.assertEqual(coverage.RED_EXIT, rc)
        db = store.open_ro(db_path)
        try:
            rules = [json.loads(r[0]).get("rule") for r in db.execute(
                "SELECT data FROM journal WHERE event='process_finding'")]
        finally:
            store.close(db)
        self.assertIn("coverage_red", rules)
        self.assertEqual((False, "RED"), self._close(cov))

    def test_a_timed_out_browser_call_is_never_green(self):
        bid = _bid(LOCK)
        for exit_, log, verdict in ((124, "", "INCOMPLETE"), (137, "", "INCOMPLETE"),
                                    (100, "  TIMEOUT [ 900.001s] %s holds_in_webkit\n" % bid,
                                     "RED")):
            cov = self._map(exit_, log)
            self.assertEqual(verdict, coverage.verdict_of(cov), exit_)
            rc, _out, _maps, _db = _full_build(self.root, cov)
            self.assertNotEqual(0, rc, exit_)
            self.assertEqual((False, verdict), self._close(cov))


if __name__ == "__main__":
    unittest.main()
