"""Tests for `scripts/pipeline/events.py` -- the event kinds of the journal.

An event the receipt is later rebuilt from must carry what the receipt needs,
so `emit` refuses an event without its required fields (V0 section 3) instead
of writing a row nobody can project. The executor streams the same objects as
JSONL; a stream that ends without `pass_ended` is a lost run, and the store
has to say so.
"""

import io
import os
import pathlib
import sys
import tempfile
import unittest
from unittest import mock
from scripts.tests import store_isolation

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO))

from scripts.pipeline import events, store  # noqa: E402

AT = "2026-10-08T12:00:00Z"
RUN = "run-0123456789ab"


# gate-selftest loads every module into one process: no fixture event of this
# module may reach the metrics target of the caller's environment.
_METRICS_URL = mock.patch.dict(os.environ, {"MECLAW_PIPELINE_METRICS_URL": ""})


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()
    _METRICS_URL.start()


def tearDownModule():  # noqa: N802 -- unittest's name
    _METRICS_URL.stop()
    store_isolation.restore()


class EventsCase(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.db = store.open(pathlib.Path(self.tmp.name) / "pipeline.db")

    def tearDown(self):
        store.close(self.db)
        self.tmp.cleanup()

    def test_event_requires_fields_per_kind(self):
        # The table is the contract: every V0 event is known, with its kinds.
        for name in ("pass_planned", "run_dispatched", "hygiene_checked",
                     "lane_fingerprint", "station_started", "station_log",
                     "test_result", "cache_hit", "station_ended", "flake_isolated",
                     "watchdog_fired", "workspace_changed", "pass_ended",
                     "metrics_failed", "lease_taken", "lease_released",
                     "lease_offered", "merge_queued", "merge_gated", "merge_done",
                     "merge_refused", "merge_abandoned", "revert_done", "process_finding",
                     "finding_resolved", "night_report", "graph_built",
                     "coverage_built", "satellite_pin_needed", "minimum_measured",
                     "lane_power_requested", "owner_decision"):
            self.assertIn(name, events.EVENTS, name)
        self.assertEqual(events.EVENTS["station_ended"].required,
                         ("run", "station", "verdict", "secs", "exit", "scope"))
        self.assertEqual(events.EVENTS["hygiene_checked"].kinds, ("note", "finding"))
        with self.assertRaisesRegex(events.EventError, "missing.*exit"):
            events.emit(self.db, "station_ended", "status",
                        {"run": RUN, "station": "fmt", "verdict": "GREEN", "secs": 1, "scope": "check"},
                        run=RUN, at=AT)
        with self.assertRaisesRegex(events.EventError, "kind"):
            events.emit(self.db, "station_ended", "note",
                        {"run": RUN, "station": "fmt", "verdict": "GREEN", "secs": 1,
                         "exit": 0, "scope": "check"}, run=RUN, at=AT)
        with self.assertRaisesRegex(events.EventError, "unknown event"):
            events.emit(self.db, "station_finished", "status", {}, run=RUN, at=AT)
        with self.assertRaisesRegex(events.EventError, "verdict"):
            events.emit(self.db, "station_ended", "status",
                        {"run": RUN, "station": "fmt", "verdict": "PURPLE", "secs": 1,
                         "exit": 0, "scope": "check"}, run=RUN, at=AT)
        seq = events.emit(self.db, "station_ended", "status",
                          {"run": RUN, "station": "fmt", "verdict": "GREEN", "secs": 1,
                           "exit": 0, "scope": "check"}, run=RUN, at=AT)
        rows = events.read_run(self.db, RUN)
        self.assertEqual([r["seq"] for r in rows], [seq])
        self.assertEqual(rows[0]["data"]["station"], "fmt")
        self.assertTrue(rows[0]["text"].isascii() and rows[0]["text"])

    def test_receipt_fields_are_required_not_defaulted(self):
        # `dirty` decides whether the export may ship a pass (make_export.py
        # refuses dirty receipts): an event without it must be refused, never
        # read as clean. `scope` is the `[<scope>]` of every GATE line.
        planned = {"run": RUN, "mode": "strand", "repo": "meclaw", "rev": "a" * 40,
                   "base": "b" * 40, "stations": ["fmt"], "reason": {}, "cache_known": 0}
        with self.assertRaisesRegex(events.EventError, "missing dirty"):
            events.emit(self.db, "pass_planned", "status", planned, run=RUN, at=AT)
        with self.assertRaisesRegex(events.EventError, "dirty"):
            events.emit(self.db, "pass_planned", "status", dict(planned, dirty=None),
                        run=RUN, at=AT)
        with self.assertRaisesRegex(events.EventError, "missing scope"):
            events.emit(self.db, "station_started", "status",
                        {"run": RUN, "station": "fmt", "needs": {}, "cmds": 1},
                        run=RUN, at=AT)
        with self.assertRaisesRegex(events.EventError, "missing scope"):
            events.emit(self.db, "station_ended", "status",
                        {"run": RUN, "station": "fmt", "verdict": "GREEN", "secs": 1,
                         "exit": 0}, run=RUN, at=AT)
        # lock_wait_secs, archive, narrowing (re-check H2 M1) and testing_ruling
        # (R-TV-W21 c) are documented options with defaults.
        self.assertEqual(events.OPTIONAL["pass_planned"],
                         {"lock_wait_secs": 0, "archive": None, "narrowing": None,
                          "testing_ruling": None})
        events.emit(self.db, "pass_planned", "status", dict(planned, dirty=True),
                    run=RUN, at=AT)

    def test_lost_stream_without_items_marks_every_dispatched_run(self):
        # Lanes know the run and its task_runs, never the items (V0 section 3):
        # the core hands over its dispatch list, and every run of it that is
        # still dispatched is lost -- also the stations that never started.
        wp = store.item_create(self.db, "workpackage", "strand pass", at=AT)
        trs = [store.run_open(self.db, store.item_create(
            self.db, "task", name, parent=wp["id"], at=AT)["id"], "lane-a", at=AT)
            for name in ("fmt", "clippy", "tests")]
        store.run_close(self.db, trs[0], "GREEN", at=AT)
        stream = [{"at": AT, "run": RUN, "kind": "status", "event": "station_started",
                   "data": {"run": RUN, "station": "clippy", "scope": "check",
                            "needs": {"cpu": 1}, "cmds": 1}}]
        result = events.ingest_stream(self.db, stream, RUN, task_runs=trs)
        self.assertEqual(result, {"lines": 1, "lost": True, "marked": sorted(trs[1:])})
        lost = dict(self.db.execute("SELECT id, lost FROM task_runs").fetchall())
        self.assertEqual(lost, {trs[0]: 0, trs[1]: 1, trs[2]: 1})

    def test_jsonl_round_trip_keeps_order(self):
        lines = [
            {"at": AT, "run": RUN, "kind": "status", "event": "pass_planned",
             "data": {"run": RUN, "mode": "strand", "repo": "meclaw", "rev": "a" * 40,
                      "base": "b" * 40, "stations": ["fmt"], "reason": {}, "cache_known": 0,
                      "dirty": False}},
            {"at": AT, "run": RUN, "kind": "note", "event": "station_log",
             "data": {"run": RUN, "station": "fmt", "path": "logs/fmt.log"}},
        ]
        buf = io.StringIO()
        events.write_jsonl(buf, lines)
        buf.seek(0)
        self.assertEqual(list(events.read_jsonl(buf)), lines)

    def test_events_ingest_marks_lost_without_pass_ended(self):
        wp = store.item_create(self.db, "workpackage", "strand pass", at=AT)
        task = store.item_create(self.db, "task", "fmt", parent=wp["id"], at=AT)
        tr = store.run_open(self.db, task["id"], "lane-a", at=AT)
        stream = [
            {"at": AT, "run": RUN, "item": task["id"], "kind": "status",
             "event": "station_started",
             "data": {"run": RUN, "station": "fmt", "scope": "check", "needs": {"cpu": 1},
                      "cmds": 1}},
        ]
        result = events.ingest_stream(self.db, stream, RUN, task_runs=[tr])
        self.assertEqual(result, {"lines": 1, "lost": True, "marked": [tr]})
        lost = self.db.execute("SELECT lost FROM task_runs WHERE id=?", (tr,)).fetchone()[0]
        self.assertEqual(lost, 1)

        # The same stream WITH its pass_ended is not lost.
        run2 = "run-ba9876543210"
        task2 = store.item_create(self.db, "task", "fmt", parent=wp["id"], at=AT)
        tr2 = store.run_open(self.db, task2["id"], "lane-a", at=AT)
        stream2 = [dict(stream[0], run=run2, item=task2["id"],
                        data=dict(stream[0]["data"], run=run2)),
                   {"at": AT, "run": run2, "kind": "status", "event": "pass_ended",
                    "data": {"run": run2, "verdict": "GREEN", "secs": 3, "lane_seconds": 3,
                             "critical_path_s": 3, "rounds": 1, "cache_hits": 0,
                             "stations": {"green": 1}}}]
        result = events.ingest_stream(self.db, stream2, run2, task_runs=[tr2])
        self.assertEqual(result, {"lines": 2, "lost": False, "marked": []})
        lost = self.db.execute("SELECT lost FROM task_runs WHERE id=?", (tr2,)).fetchone()[0]
        self.assertEqual(lost, 0)
        # A line of a foreign run is refused, not filed under this one.
        with self.assertRaisesRegex(events.EventError, "run"):
            events.ingest_stream(self.db, stream, run2)


if __name__ == "__main__":
    unittest.main()
