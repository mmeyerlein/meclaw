"""Tests for `scripts/pipeline/backfill.py` -- the archive as finished runs.

The before-numbers of the dashboard are the archived gates: every v1 receipt
and every satellite `summary.txt` under `plans/*/receipts/` becomes one run in
the store. The fixtures under `fixtures/pipeline/` are three archived meclaw
receipts (anonymised) and two satellite summaries; their sums are fixed here,
so a reader that miscounts a run, a second, or a red pass is caught.
"""

import os
import pathlib
import shutil
import sys
import tempfile
import unittest
from unittest import mock
from scripts.tests import store_isolation

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO))

from scripts.pipeline import backfill, metrics, store  # noqa: E402

FIXTURES = REPO / "scripts" / "tests" / "fixtures" / "pipeline"


# gate-selftest loads every module into one process: no fixture event of this
# module may reach the metrics target of the caller's environment.
_METRICS_URL = mock.patch.dict(os.environ, {"MECLAW_PIPELINE_METRICS_URL": ""})


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()
    _METRICS_URL.start()


def tearDownModule():  # noqa: N802 -- unittest's name
    _METRICS_URL.stop()
    store_isolation.restore()


class BackfillCase(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.db = store.open(pathlib.Path(self.tmp.name) / "pipeline.db")
        self.root = pathlib.Path(self.tmp.name) / "repo"
        shutil.copytree(FIXTURES, self.root)

    def tearDown(self):
        store.close(self.db)
        self.tmp.cleanup()

    def runs(self, event="pass_ended"):
        return [r[0] for r in self.db.execute(
            "SELECT run FROM journal WHERE event=? ORDER BY seq", (event,))]

    def test_backfill_fixture_sums(self):
        recs = backfill.scan(self.root, runner_local="local")
        sums = backfill.sums(recs)
        self.assertEqual(sums["meclaw"], {"runs": 3, "secs": 6225, "red": 1})
        self.assertEqual(sums["soul2me"], {"runs": 1, "secs": 1813, "red": 1})
        self.assertEqual(sums["orga"], {"runs": 1, "secs": 0, "red": 0})
        self.assertEqual(backfill.result_lines(sums)[0],
                         "BACKFILL repo=meclaw runs=3 lane_hours=1.73 red=1")
        written = backfill.write(self.db, recs)
        self.assertEqual(written, {"written": 5, "known": 0})
        self.assertEqual(len(self.runs()), 5)
        # 13 + 21 + 31 stations of the meclaw receipts, 1 + 4 satellite lines
        self.assertEqual(len(self.runs("station_ended")), 13 + 21 + 31 + 1 + 4)
        red = self.db.execute(
            "SELECT count(*) FROM task_runs WHERE state='failed'").fetchone()[0]
        self.assertEqual(red, 1 + 1)  # one red station per red pass
        # the metric lines carry the original times, not the time of the backfill
        lines = metrics.lines(self.db)
        self.assertTrue(any(l.startswith("pipe_pass,repo=meclaw,mode=release,")
                            and l.endswith(" 1791471355000000000") for l in lines), lines[:3])

    def test_backfill_names_the_own_lane_by_the_one_rule(self):
        """Re-check tv-bn M7r B-M3: scan derived the lane name itself, without
        MECLAW_LANE_NAME -- the runs it wrote named another lane than the executor's."""
        with mock.patch.dict(os.environ, {"MECLAW_LANE_NAME": "lane-z"}):
            recs = backfill.scan(self.root)
        self.assertIn("lane-z", {r.get("runner") for r in recs})
        self.assertEqual(backfill.scan(self.root, runner_local="lane-z"), recs)

    def test_backfill_dedupes(self):
        # the same run archived twice (a copied run directory) and a second backfill
        src = next(self.root.glob("plans/wave-a/receipts/s1/*"))
        shutil.copytree(src, self.root / "plans" / "wave-c" / "receipts" / "s1" / src.name)
        recs = backfill.scan(self.root, runner_local="local")
        self.assertEqual(len(recs), 5)
        self.assertEqual(backfill.write(self.db, recs), {"written": 5, "known": 0})
        self.assertEqual(backfill.write(self.db, backfill.scan(self.root, runner_local="local")),
                         {"written": 0, "known": 5})
        self.assertEqual(len(self.runs()), 5)

    def test_backfill_reads_satellite_summary(self):
        recs = {r["repo"]: r for r in backfill.scan(self.root, runner_local="local")}
        soul = recs["soul2me"]
        self.assertEqual((soul["rev"], soul["secs"], soul["verdict"]), ("506a7e1", 1813, "RED"))
        self.assertEqual([(s["name"], s["verdict"], s["secs"]) for s in soul["stations"]],
                         [("unit", "GREEN", 1797), ("locomo_offline", "RED", 13),
                          ("inline", "GREEN", 0), ("set", "GREEN", 0)])
        # line 2 is data, never a station
        self.assertEqual(soul["data"]["branch"], "branch wave-b/t2 set 0.18 (hand run, not latest)")
        self.assertEqual(soul["strand"], "t2")

    def test_backfill_lane_from_path_and_token_log(self):
        log = pathlib.Path(self.tmp.name) / "tokens.log"
        log.write_text("2026-10-08T17:50:00Z\ttake\twave-a/s1\twave-a/s1\tlane lane-a\n")
        recs = {r["strand"]: r for r in backfill.scan(self.root, runner_local="local",
                                                     token_log=log)}
        self.assertEqual(recs["s1"]["runner"], "lane-a")  # remote, named by the token log
        self.assertEqual(recs["s2"]["runner"], "unknown")  # remote, no token line
        self.assertEqual(recs["t1"]["runner"], "local")    # a satellite runs on the host


    def test_backfill_writes_one_transaction_per_run(self):
        recs = backfill.scan(self.root, runner_local="local")
        real = backfill._events.emit
        calls = {"n": 0}

        def flaky(db, event, *a, **kw):
            if event == "station_ended":
                calls["n"] += 1
                if calls["n"] == 2:
                    raise RuntimeError("disk full")
            return real(db, event, *a, **kw)

        with mock.patch.object(backfill._events, "emit", flaky):
            with self.assertRaises(RuntimeError):
                backfill.write(self.db, recs[:1])
        # nothing of the broken run stays behind: no rows, no work package in `planned`
        self.assertEqual(self.db.execute("SELECT count(*) FROM journal").fetchone()[0], 0)
        self.assertEqual(self.db.execute("SELECT count(*) FROM items").fetchone()[0], 0)
        self.assertEqual(backfill.write(self.db, recs[:1]), {"written": 1, "known": 0})
        self.assertEqual(len(self.runs("pass_planned")), 1)

    def test_backfill_satellite_start_is_tz_independent(self):
        # the run id must not depend on the TZ of the machine that reads the archive
        import os
        import time
        saved = os.environ.get("TZ")
        try:
            os.environ["TZ"] = "Pacific/Auckland"
            time.tzset()
            a = backfill._dir_start("20261007-185850-8323901", 0)
            os.environ["TZ"] = "UTC"
            time.tzset()
            b = backfill._dir_start("20261007-185850-8323901", 0)
        finally:
            if saved is None:
                os.environ.pop("TZ", None)
            else:
                os.environ["TZ"] = saved
            time.tzset()
        self.assertEqual(a, b)
        self.assertEqual(a, "2026-10-07T16:58:50Z")  # local time of the archive, CEST

    def test_backfill_red_does_not_count_ask(self):
        recs = [{"repo": "meclaw", "secs": 10, "verdict": "ASK"},
                {"repo": "meclaw", "secs": 10, "verdict": "RED"},
                {"repo": "meclaw", "secs": 10, "verdict": "ABORTED"}]
        self.assertEqual(backfill.sums(recs)["meclaw"], {"runs": 3, "secs": 30, "red": 2})


if __name__ == "__main__":
    unittest.main()
