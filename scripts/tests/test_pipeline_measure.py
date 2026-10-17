"""Tests of `scripts/pipeline/measure.py` (`pipeline measure minimum`, M0).

No test starts cargo or ssh: every lane call goes through the `runner`
callable that `measure_atom` takes, and these tests hand it a fake.
"""
import contextlib
import io
import json
import os
import pathlib
import sqlite3
import sys
import tempfile
import unittest
from unittest import mock

SCRIPTS = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))

from pipeline import measure as m  # noqa: E402
from scripts.tests import store_isolation  # noqa: E402


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()


def tearDownModule():  # noqa: N802 -- unittest's name
    store_isolation.restore()


def fake_runner(samples):
    """A lane that answers each run with the next (load1, running, secs[, rc])."""
    it = iter(samples)

    def run(script, timeout):
        load1, running, secs, *rest = next(it)
        rc = rest[0] if rest else 0
        return 0, f"M0_LOAD {load1} {running + 1}/400\nM0_TIME 100.0 {100.0 + secs} {rc}\n"
    return run


class MeasureMinimumTest(unittest.TestCase):
    def test_report_table_with_no_accepted_run_names_a_rejected_log(self):
        """M0 nit: 0 accepted runs raised ValueError after the store write."""
        res = {"atom": "A3", "median": None, "values": [], "logs": [],
               "rejected": [{"log": "/l/A3-1.log", "reason": "failed", "secs": 0.0}]}
        table = m.report_table([res], "build01")
        self.assertIn("/l/A3-1.log", table)
        self.assertIn("(rejected: 1)", table)

    def test_a_timed_run_carries_the_verdict_names_set_here(self):
        """GH #1210 (tv-bn M5): ssh carries no environment; an atom whose filter
        holds a browser proof ran it without MECLAW_REQUIRE_BROWSERS -- a driver
        SKIP is a short green run, a floor that measured nothing."""
        env = {"MECLAW_REQUIRE_BROWSERS": "1", "MECLAW_TESTING_RULES_RULING": "OR-TV-8",
               "MECLAW_SOME_KEY": "never-sent"}
        with mock.patch.dict(os.environ, env):
            os.environ.pop("MECLAW_GATE_MODE", None)
            os.environ.pop("MECLAW_ALLOW_BUMP", None)
            script = m.timed_script("/t", "/tg", "", "true", "/l/x.log")
        first = script.splitlines()[0]
        self.assertIn(" MECLAW_REQUIRE_BROWSERS=1", first)
        self.assertIn(" MECLAW_TESTING_RULES_RULING=OR-TV-8", first)
        self.assertNotIn("never-sent", script)
        self.assertNotIn("MECLAW_GATE_MODE", script)

    def test_a_timed_run_requires_the_browsers_unless_told_otherwise(self):
        """GH #1209 (re-check tv-bn M7 M4): measure only handed the browser
        switch on and documented the green SKIP; unset, the run sets it, an
        opt-out is kept. Re-check M7c M1: the opt-out stood only on stderr --
        it is in the RESULT now, the atom's result and its row of the report."""
        for given, word, mark in ((None, " MECLAW_REQUIRE_BROWSERS=1", ""),
                                  ("0", " MECLAW_REQUIRE_BROWSERS=0", "(browsers opt-out: MECLAW_REQUIRE_BROWSERS='0')")):
            err = io.StringIO()
            with mock.patch.dict(os.environ, {}), contextlib.redirect_stderr(err):
                os.environ.pop("MECLAW_REQUIRE_BROWSERS", None)
                if given is not None:
                    os.environ["MECLAW_REQUIRE_BROWSERS"] = given
                script = m.timed_script("/t", "/tg", "", "true", "/l/x.log")
                res = m.measure_atom("A3", "true", "", 60, fake_runner([(0.1, 0, 2.0)]), "/t", "/tg",
                                     "/l", runs=1)
            row = m.report_table([res], "lane-x").splitlines()[-1]
            with self.subTest(given=given):
                self.assertIn(word, script.splitlines()[0])
                self.assertEqual(mark, res.get("browsers"))
                if mark:
                    self.assertIn(mark, row)
                else:
                    self.assertNotIn("opt-out", row)

    def test_the_console_line_of_an_atom_names_the_opt_out(self):
        """Re-check tv-bn M7d M1: `measure_atoms` prints one line per atom --
        the line a caller of `pipeline measure` reads without `--report`. With
        an opt-out it carries the mark of the result, without one no mark."""
        import argparse

        def lane_runner(target):
            runs = fake_runner([(0.1, 0, 2.0)])
            return lambda script, timeout: (0, "8\n") if script == "nproc" else runs(script, timeout)
        args = argparse.Namespace(lane="lane-x", tree="/t", root="/r", strand="s", target_dir="/tg",
                                  only="A3", runs=1)
        for given, mark in ((None, ""), ("0", "(browsers opt-out: MECLAW_REQUIRE_BROWSERS='0')")):
            out = io.StringIO()
            with mock.patch.dict(os.environ, {}), mock.patch.object(m, "lane_target", return_value="u@x"), \
                    mock.patch.object(m, "ssh_runner", side_effect=lane_runner), \
                    contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
                os.environ.pop("MECLAW_REQUIRE_BROWSERS", None)
                if given is not None:
                    os.environ["MECLAW_REQUIRE_BROWSERS"] = given
                res = m.measure_atoms(args)
            line = [ln for ln in out.getvalue().splitlines() if ln.startswith("measure: A3 median")]
            with self.subTest(given=given):
                self.assertEqual(1, len(line), out.getvalue())
                self.assertEqual(mark, res[0]["browsers"])
                if mark:
                    self.assertTrue(line[0].endswith(" " + mark), line[0])
                else:
                    self.assertNotIn("opt-out", line[0])

    def test_the_default_tree_is_the_gates_lane_tree(self):
        """GH #1199: `--tree` defaults to the gate's one rule for a lane tree
        (`schedule.lane_tree_name` under `admission_tree`), not a second spelling of
        `wt/<strand>`: the core's tree keeps the plain name."""
        import argparse
        args = argparse.Namespace(lane="lane-x", tree="", root="/r", strand="pass2", target_dir="/tg",
                                  only="A3", runs=1)
        trees = []

        def measure_atom(aid, cmd, prep, timeout, runner, tree, *a, **kw):
            trees.append(tree)
            return {"atom": aid, "median": 1.0, "values": [1.0], "logs": ["/l"], "browsers": ""}

        real = m._schedule.lane_tree_name
        for rule, want in ((real, "/r/wt/pass2"), (lambda repo, wt: "%s+%s" % (repo, wt), "/r/wt/meclaw+pass2")):
            trees.clear()
            with self.subTest(want=want), mock.patch.object(m, "lane_target", return_value="u@x"), \
                    mock.patch.object(m, "ssh_runner", return_value=lambda s, t: (0, "8\n")), \
                    mock.patch.object(m, "measure_atom", side_effect=measure_atom), \
                    mock.patch.object(m._schedule, "lane_tree_name", side_effect=rule), \
                    contextlib.redirect_stdout(io.StringIO()):
                m.measure_atoms(args)
                self.assertEqual([want], trees)

    def test_an_empty_run_of_a_hundredth_second_is_rejected(self):
        """M0 nit: a constant command that ran 0.01 s with rc 0 measured nothing."""
        runner = fake_runner([(0.1, 0, 0.01)] * 2)
        res = m.measure_atom("A3", "true", "", 60, runner, "/t", "/tg", "/l", runs=1)
        self.assertIsNone(res["median"])
        self.assertEqual(["empty", "empty"], [r["reason"] for r in res["rejected"]])

    def test_measure_minimum_critical_path_is_max_of_parallel_branches(self):
        # build -> run is one chain (30 + 40); clippy runs beside it (50)
        secs, path = m.critical_path({"build": 30.0, "run": 40.0, "clippy": 50.0}, {"run": ["build"]})
        self.assertEqual(secs, 70.0)
        self.assertEqual(path, ["build", "run"])
        secs, path = m.critical_path({"build": 30.0, "run": 40.0, "clippy": 90.0}, {"run": ["build"]})
        self.assertEqual((secs, path), (90.0, ["clippy"]))
        st = m.stage_minimum("strand-small", {"A3": 60.0, "A5:ab1103": 120.0, "A6:clippy": 80.0}, 15.0)
        self.assertEqual(st["seconds"], 180.0)
        self.assertEqual(st["critical_path"], ["build", "run"])

    def test_measure_x_from_overhead_plus_margin(self):
        # the plan's example: minimum 240 s, unavoidable 15 s -> x = 0.16
        self.assertEqual(m.x_from_overhead(15.0, 240.0), 0.16)
        self.assertEqual(m.x_from_overhead(0.0, 100.0), 0.10)
        with self.assertRaises(ValueError):
            m.x_from_overhead(5.0, 0.0)

    def test_measure_rejects_run_under_foreign_load(self):
        # first run of the pass: load1 above 2 is foreign; later runs judge the runnable count
        self.assertTrue(m.foreign_load({"load1": 2.5, "running": 0}, first=True))
        self.assertFalse(m.foreign_load({"load1": 2.5, "running": 0}, first=False))
        self.assertTrue(m.foreign_load({"load1": 0.1, "running": 3}, first=False))
        runner = fake_runner([(3.1, 0, 99.0), (0.2, 0, 10.0), (0.2, 0, 11.0), (0.3, 5, 50.0), (0.2, 0, 12.0)])
        res = m.measure_atom("A3", "true", "", 60, runner, "/t", "/tg", "/l", runs=3, first=True)
        self.assertEqual(res["values"], [10.0, 11.0, 12.0])
        self.assertEqual(len(res["rejected"]), 2)
        self.assertEqual(res["median"], 11.0)

    def test_measure_rejects_failed_and_empty_runs(self):
        # rc != 0 (the class of the 22 empty runs) and 0 s never count as a floor
        runner = fake_runner([(0.1, 0, 0.07, 4), (0.1, 0, 10.0), (0.1, 0, 0.0), (0.1, 0, 11.0), (0.1, 0, 12.0)])
        res = m.measure_atom("A3", "true", "", 60, runner, "/t", "/tg", "/l", runs=3)
        self.assertEqual(res["values"], [10.0, 11.0, 12.0])
        self.assertEqual([r["reason"] for r in res["rejected"]], ["failed", "failed"])
        self.assertEqual(res["median"], 11.0)

    def test_measure_fewer_accepted_runs_than_asked_give_no_median(self):
        runner = fake_runner([(0.1, 0, 10.0)] + [(0.1, 0, 0.07, 4)] * 5)
        res = m.measure_atom("A3", "true", "", 60, runner, "/t", "/tg", "/l", runs=3)
        self.assertEqual(res["values"], [10.0])
        self.assertIsNone(res["median"])
        runner = fake_runner([(0.1, 0, 9.0)])
        self.assertEqual(m.measure_atom("A4", "true", "", 60, runner, "/t", "/tg", "/l", runs=1)["median"], 9.0)

    def test_measure_median_of_three(self):
        self.assertEqual(m.median_of_three([30.0, 10.0, 20.0]), 20.0)
        self.assertEqual(m.median_of_three([5.0, 500.0, 6.0]), 6.0)  # never a mean
        with self.assertRaises(ValueError):
            m.median_of_three([1.0, 2.0])

    def test_measure_writes_minimum_rows_and_event(self):
        with tempfile.TemporaryDirectory() as d:
            db = m._store.open(pathlib.Path(d) / "pipeline.db")
            rows = [{"stage": "atom:A3", "seconds": 61.0, "critical_path": [60.0, 61.0, 62.0], "source": "logs/A3-1.log"},
                    m.stage_minimum("rerun", {"A3": 61.0, "A5:py": 9.0}, 12.0)]
            m.write_rows(db, rows, "lane:lane-a")
            got = [tuple(r) for r in
                   db.execute("SELECT stage, runner, repo, seconds, x FROM minimum ORDER BY stage").fetchall()]
            self.assertEqual(got, [("atom:A3", "lane:lane-a", "meclaw", 61.0, None),
                                   ("rerun", "lane:lane-a", "meclaw", 70.0, 0.27)])
            ev = db.execute("SELECT kind, event, data FROM journal ORDER BY seq").fetchall()
            self.assertEqual([(k, e) for k, e, _ in ev], [("status", "minimum_measured")] * 2)
            data = json.loads(ev[1][2])
            self.assertEqual(set(data), {"stage", "runner", "seconds", "critical_path", "overhead_s", "x"})
            lines = m.influx_lines(rows, "lane-a")
            self.assertEqual(len(lines), 1)  # atoms are not a pipe_minimum series
            self.assertTrue(lines[0].startswith("pipe_minimum,repo=meclaw,stage=rerun,runner=lane-a seconds=70.0,"))

    def test_measure_writes_through_the_w0_store_and_events(self):
        # store.open (WAL, schema_version) and events.emit (validation, HOOKS) -- no side door
        self.assertFalse(hasattr(m, "open_store"))
        self.assertFalse(hasattr(m, "store_path"))
        seen = []
        hook = lambda db, row: seen.append(row["event"])  # noqa: E731
        m._events.HOOKS.append(hook)
        try:
            with tempfile.TemporaryDirectory() as d:
                db = m._store.open(pathlib.Path(d) / "pipeline.db")
                m.write_rows(db, [m.stage_minimum("rerun", {"A3": 61.0, "A5:py": 9.0}, 12.0)], "lane:lane-a")
                self.assertEqual(db.execute("PRAGMA journal_mode").fetchone()[0], "wal")
                text = db.execute("SELECT text FROM journal").fetchone()[0]
                self.assertEqual(text, "minimum_measured stage=rerun runner=lane:lane-a seconds=70.0 overhead_s=12.0 x=0.27")
        finally:
            m._events.HOOKS.remove(hook)
        self.assertEqual(seen, ["minimum_measured"])

    def test_measure_report_table_has_sources(self):
        res = [{"atom": "A3", "median": 61.0, "values": [60.0, 61.0, 62.0], "rejected": [], "logs": ["/b/A3-1.log"]}]
        table = m.report_table(res, "lane-a")
        self.assertIn("| A3 | lane-a | 61.0 | 60.0 / 61.0 / 62.0 | `/b/A3-1.log` … |", table)
        self.assertTrue(table.startswith("| Atom | Lane | Median (s) | Values (s) | Source |"))
        table = m.report_table([dict(res[0], rejected=[{"reason": "failed"}])], "lane-a")
        self.assertIn("(rejected: 1)", table)
        with self.assertRaises(ValueError):
            m.report_table([dict(res[0], logs=[])], "lane-a")



# The M0 medians of build04 in stage keys (receipts/M0/atoms.tsv), trimmed to the critical paths.
M0_ATOMS = """# key\tseconds
A1\t450.23
A3\t6.46
A2:cells\t191.79
A2sel\t53.62
A2c\t54.47
A5:ab1103\t167.58
A5:zkfix\t205.12
A5:py\t6.81
A4:nproc\t469.71
A6:precheck\t30.69
A7:dry\t86.79
A7:warm\t274.92
A7:audit_full\t149.82
A8:browser_max\t416.34
A8:memory_max\t24.1
A9:max\t118.57
A10:pre\t2.0
A10:lane\t10.4
"""


class StageRowsTest(unittest.TestCase):
    def atoms_file(self, d, text=M0_ATOMS):
        f = pathlib.Path(d) / "atoms.tsv"
        f.write_text(text)
        return f

    def test_measure_load_atoms_reads_key_seconds_tsv(self):
        with tempfile.TemporaryDirectory() as d:
            atoms = m.load_atoms(self.atoms_file(d, "# c\nA3\t6.46\n\nA8:memory_max\t24.1\n"))
        self.assertEqual(atoms, {"A3": 6.46, "A8:memory_max": 24.1})

    def test_measure_stage_rows_one_per_stage_from_atom_medians(self):
        with tempfile.TemporaryDirectory() as d:
            rows = m.stage_rows(m.load_atoms(self.atoms_file(d)), source="atoms.tsv")
        by = {r["stage"]: r for r in rows}
        self.assertEqual(list(by), list(m.STAGES))
        # integration: the memory lab counts with its slowest case, so the browser proof is the path
        self.assertEqual((by["integration"]["seconds"], by["integration"]["x"]), (416.34, 0.12))
        self.assertEqual(by["integration"]["critical_path"], ["browser"])
        self.assertEqual((by["merge-cold"]["seconds"], by["merge-cold"]["x"]), (655.35, 0.12))
        self.assertEqual((by["pre"]["overhead_s"], by["rerun"]["overhead_s"]), (2.0, 10.4))
        self.assertTrue(all(r["source"] == "atoms.tsv" for r in rows))

    def test_measure_stage_rows_need_the_overhead_atoms(self):
        with self.assertRaises(ValueError):
            m.stage_rows({"A3": 6.46})

    def test_measure_stages_only_writes_stage_rows_without_a_lane(self):
        from pipeline import cli

        def no_lane(*_):
            raise AssertionError("stages-only must not reach a lane")
        saved = (m.lane_target, m.ssh_runner)
        m.lane_target = m.ssh_runner = no_lane
        try:
            with tempfile.TemporaryDirectory() as d:
                args = cli.parser().parse_args(["measure", "minimum", "--lane", "lane-a", "--stages-only",
                                                "--atoms-from", str(self.atoms_file(d))])
                self.assertEqual(args.func(args), 0)
        finally:
            m.lane_target, m.ssh_runner = saved
        db = m._store.open()
        try:
            got = [r[0] for r in db.execute("SELECT stage FROM minimum WHERE runner='lane:lane-a'")]
        finally:
            m._store.close(db)
        self.assertEqual(sorted(got), sorted(m.STAGES))

    def test_measure_stages_only_needs_atoms_from(self):
        from pipeline import cli
        args = cli.parser().parse_args(["measure", "minimum", "--lane", "lane-a", "--stages-only"])
        with self.assertRaises(SystemExit):
            args.func(args)


class CliTest(unittest.TestCase):
    def test_cli_registers_measure(self):
        from pipeline import cli
        self.assertIn("measure", cli.MODULES)
        args = cli.parser().parse_args(["measure", "minimum", "--lane", "build04", "--only", "A3"])
        self.assertIs(m.cmd_minimum, args.func)
        self.assertEqual("A3", args.only)


if __name__ == "__main__":
    unittest.main()
